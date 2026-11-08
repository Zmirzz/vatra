use std::path::{Component, Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use vatra_core::{Event, ProcessGroup};

use crate::task::{Check, Task};

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub passed: bool,
    pub failures: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub tool_calls: u32,
    pub tool_errors: u32,
    pub seconds: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
}

const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// Runs every check and collects a message for each one that fails.
pub async fn evaluate(
    task: &Task,
    fixture_dir: &Path,
    workdir: &Path,
    transcript: &[Event],
) -> Outcome {
    evaluate_with_timeout(task, fixture_dir, workdir, transcript, COMMAND_TIMEOUT).await
}

/// `evaluate` with the command timeout injectable, for tests.
pub async fn evaluate_with_timeout(
    task: &Task,
    fixture_dir: &Path,
    workdir: &Path,
    transcript: &[Event],
    timeout: Duration,
) -> Outcome {
    let mut failures = Vec::new();
    for check in &task.checks {
        if let Some(message) = check_failure(check, fixture_dir, workdir, transcript, timeout).await
        {
            failures.push(message);
        }
    }
    Outcome {
        passed: failures.is_empty(),
        failures,
    }
}

async fn check_failure(
    check: &Check,
    fixture_dir: &Path,
    workdir: &Path,
    transcript: &[Event],
    timeout: Duration,
) -> Option<String> {
    match check {
        Check::FileContains { path, text } => match read_text(workdir, path) {
            None => Some(format!("{path} is missing")),
            Some(body) if body.contains(text.as_str()) => None,
            Some(_) => Some(format!("{path} doesn't contain \"{text}\"")),
        },
        Check::FileNotContains { path, text } => match read_text(workdir, path) {
            None => Some(format!("{path} is missing")),
            Some(body) if !body.contains(text.as_str()) => None,
            Some(_) => Some(format!("{path} still contains \"{text}\"")),
        },
        Check::FileUnchanged { path } => {
            match (read_bytes(fixture_dir, path), read_bytes(workdir, path)) {
                (Ok(original), Ok(current)) if original == current => None,
                (Ok(_), Ok(_)) => Some(format!("{path} was changed")),
                (_, Err(_)) => Some(format!("{path} is missing")),
                (Err(e), _) => Some(format!("fixture file {path} can't be read: {e}")),
            }
        }
        Check::CommandPasses { command } => run_command(command, workdir, timeout).await,
        Check::ToolUsed { any_of } => {
            let used = all_events(transcript).into_iter().any(|event| match event {
                Event::ToolCall { name, .. } => any_of.iter().any(|want| tool_matches(name, want)),
                _ => false,
            });
            (!used).then(|| format!("none of these tools was used: {}", any_of.join(", ")))
        }
        Check::AnswerContains { text } => {
            let answer = transcript
                .iter()
                .filter_map(|event| match event {
                    Event::AssistantText { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!answer.to_lowercase().contains(&text.to_lowercase()))
                .then(|| format!("the answer doesn't mention \"{text}\""))
        }
    }
}

fn read_text(dir: &Path, path: &str) -> Option<String> {
    read_bytes(dir, path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn confined_file(dir: &Path, relative: &str) -> std::io::Result<PathBuf> {
    let path = Path::new(relative);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(std::io::Error::other(
            "check path must stay inside its fixture",
        ));
    }
    let root = dir.canonicalize()?;
    let file = root.join(path).canonicalize()?;
    if !file.starts_with(&root) {
        return Err(std::io::Error::other(
            "check path resolves outside its fixture",
        ));
    }
    Ok(file)
}

fn read_bytes(dir: &Path, path: &str) -> std::io::Result<Vec<u8>> {
    std::fs::read(confined_file(dir, path)?)
}

/// Matches `name` exactly, or as an MCP tool (`mcp__<server>__<name>`).
fn tool_matches(name: &str, want: &str) -> bool {
    name == want || name.ends_with(&format!("__{want}"))
}

/// Every event in the transcript, with the contents of Nested events included.
fn all_events(transcript: &[Event]) -> Vec<&Event> {
    let mut out = Vec::new();
    let mut stack: Vec<&Event> = transcript.iter().collect();
    while let Some(event) = stack.pop() {
        out.push(event);
        if let Event::Nested { event, .. } = event {
            stack.push(event);
        }
    }
    out
}

async fn run_command(command: &str, workdir: &Path, timeout: Duration) -> Option<String> {
    let tree = match ProcessGroup::new() {
        Ok(tree) => tree,
        Err(error) => return Some(format!("`{command}` couldn't be supervised: {error}")),
    };
    let mut cmd = shell(command);
    cmd.current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return Some(format!("`{command}` couldn't start: {e}")),
    };
    if let Err(error) = tree.adopt(&child)
        && !matches!(child.try_wait(), Ok(Some(_)))
    {
        let _ = child.kill().await;
        return Some(format!("`{command}` couldn't be supervised: {error}"));
    }

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Err(_) => {
            let _ = tree.kill_all();
            Some(format!(
                "`{command}` didn't finish in {} seconds",
                timeout.as_secs()
            ))
        }
        Ok(Err(e)) => Some(format!("`{command}` couldn't run: {e}")),
        Ok(Ok(output)) if output.status.success() => None,
        Ok(Ok(output)) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let tail = lines[lines.len().saturating_sub(5)..].join("\n");
            if tail.is_empty() {
                Some(format!(
                    "`{command}` still fails ({})",
                    exit_text(&output.status)
                ))
            } else {
                Some(format!("`{command}` still fails: {tail}"))
            }
        }
    }
}

fn exit_text(status: &ExitStatus) -> String {
    status
        .code()
        .map_or("no exit code".into(), |c| format!("exit code {c}"))
}

#[cfg(windows)]
fn shell(command: &str) -> tokio::process::Command {
    use std::os::windows::process::CommandExt as _;
    let mut cmd = tokio::process::Command::new("cmd");
    cmd.creation_flags(0x0800_0000);
    cmd.arg("/C");
    // Raw, so cmd receives the command text as written (no \" escaping).
    cmd.as_std_mut().raw_arg(command);
    cmd
}

#[cfg(not(windows))]
fn shell(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd
}

/// Recursively copies a fixture folder into `into`.
pub fn copy_fixture(fixture_dir: &Path, into: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(into)?;
    for entry in std::fs::read_dir(fixture_dir)? {
        let entry = entry?;
        let target = into.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(std::io::Error::other("fixture links are not allowed"));
        }
        if kind.is_dir() {
            copy_fixture(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Counts tool calls and failed results, and sums usage and cost. Nested events count too.
pub fn metrics(transcript: &[Event], seconds: f64) -> Metrics {
    let mut m = Metrics {
        seconds,
        ..Metrics::default()
    };
    let mut cost: Option<f64> = None;
    for event in all_events(transcript) {
        match event {
            Event::ToolCall { .. } => m.tool_calls += 1,
            Event::ToolResult { is_error: true, .. } => m.tool_errors += 1,
            Event::Usage {
                input_tokens,
                output_tokens,
                cost_usd,
            } => {
                m.input_tokens += input_tokens;
                m.output_tokens += output_tokens;
                if let Some(c) = cost_usd {
                    cost = Some(cost.unwrap_or(0.0) + c);
                }
            }
            _ => {}
        }
    }
    m.cost_usd = cost;
    m
}
