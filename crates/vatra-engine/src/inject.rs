//! Runs the commands in a skill's body before the model sees it: a fenced block that opens with ```! and `!`cmd``
//! spans. Each command goes through the turn's rules and approver, like a Bash call, and runs in the turn's
//! process tree. The output replaces the command, and it is not looked at again.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::json;
use vatra_core::find_bash;
use vatra_extensions::{BangForm, bang_spans};

use crate::Approver;
use crate::permissions::{Decision, PermissionPolicy, ToolClass};
use crate::tools::{Ran, ShellError, ToolContext, bash_command, run_shell};

/// The output of a command is kept up to this many characters.
const OUTPUT_LIMIT: usize = 50_000;
/// How long a command may run before it is stopped.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// What the commands of a skill run with: the turn's context (the working folder, the process tree, the hidden
/// variables), the rules and the approver, and the commands the person has approved in this session.
pub struct ShellCtx<'a> {
    pub context: &'a ToolContext,
    pub policy: &'a PermissionPolicy,
    pub approver: &'a dyn Approver,
    pub approved: &'a Mutex<HashSet<String>>,
    /// The skill whose body is run, for the approval.
    pub skill: &'a str,
    pub timeout: Duration,
}

/// Why a skill's commands could not all run. Any of them stops the skill, and the turn with it.
#[derive(Debug, thiserror::Error)]
pub enum InjectError {
    #[error("The command `{command}` failed: {message}")]
    Failed { command: String, message: String },
    #[error("A rule denies the skill's command `{0}`.")]
    Denied(String),
    #[error("The person refused the skill's command `{0}`.")]
    Refused(String),
    #[error("The skill's command timed out after {} ms and was stopped.", .0.as_millis())]
    TimedOut(Duration),
    #[error("The skill's command was stopped.")]
    Stopped,
    #[error("No bash was found on PATH, so the skill's commands can't run.")]
    NoShell,
}

/// The body with each command replaced by its output. The text is read once: output that looks like a command
/// stays as it is.
pub async fn run_injections(body: &str, shell: &ShellCtx<'_>) -> Result<String, InjectError> {
    let mut out = String::with_capacity(body.len());
    let mut last = 0;
    for span in bang_spans(body) {
        out.push_str(&body[last..span.range.start]);
        let output = run_command(&span.command, shell).await?;
        out.push_str(&output);
        if span.form == BangForm::Fenced && !output.is_empty() && !output.ends_with('\n') {
            out.push('\n');
        }
        last = span.range.end;
    }
    out.push_str(&body[last..]);
    Ok(out)
}

/// Runs one command after the rules and the approver have let it through, and returns its output.
async fn run_command(command: &str, shell: &ShellCtx<'_>) -> Result<String, InjectError> {
    if command.is_empty() {
        return Ok(String::new());
    }
    let arguments = json!({ "command": command });
    match shell
        .policy
        .decide("Bash", ToolClass::Execute, &arguments, shell.context)
    {
        Decision::Allow => {}
        Decision::Deny(_) => return Err(InjectError::Denied(command.into())),
        Decision::Ask => {
            // An ask rule asks every time: only an answer to the mode's question is remembered.
            let by_rule = shell.policy.asked_by_rule("Bash", &arguments, shell.context);
            if by_rule || !lock(shell.approved).contains(command) {
                let asked = json!({ "command": command, "skill": shell.skill });
                if !shell.approver.approve("Bash", &asked).await {
                    return Err(InjectError::Refused(command.into()));
                }
                if !by_rule {
                    lock(shell.approved).insert(command.into());
                }
            }
        }
    }
    let bash = find_bash(&std::env::var_os("PATH").unwrap_or_default()).ok_or(InjectError::NoShell)?;
    let process = bash_command(&bash, command, &shell.context.hidden_env, &shell.context.cwd);
    match run_shell(process, shell.timeout, &shell.context.tree).await {
        Ok(ran) => output(command, ran),
        Err(ShellError::TimedOut(after)) => Err(InjectError::TimedOut(after)),
        Err(error) => Err(InjectError::Failed {
            command: command.into(),
            message: error.to_string(),
        }),
    }
}

/// The text a command printed. Exit code 1 is accepted, since `grep` uses it for no match. Other codes fail.
fn output(command: &str, ran: Ran) -> Result<String, InjectError> {
    let mut text = String::from_utf8_lossy(&ran.stdout.bytes).trim_end().to_string();
    let errors = String::from_utf8_lossy(&ran.stderr.bytes);
    if !errors.trim_end().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(errors.trim_end());
    }
    match ran.status.code() {
        Some(0) | Some(1) => {}
        Some(code) => {
            return Err(InjectError::Failed {
                command: command.into(),
                message: format!("exit code {code}\n{text}"),
            });
        }
        None => return Err(InjectError::Stopped),
    }
    if text.chars().count() > OUTPUT_LIMIT {
        let kept: String = text.chars().take(OUTPUT_LIMIT).collect();
        return Ok(format!("{kept}\n(The output is cut at 50,000 characters.)"));
    }
    Ok(text)
}

fn lock(approved: &Mutex<HashSet<String>>) -> MutexGuard<'_, HashSet<String>> {
    approved.lock().unwrap_or_else(PoisonError::into_inner)
}
