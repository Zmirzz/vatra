use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use vatra_core::{ProcessGroup, find_bash};

use super::{SAVE_OVER_CHARS, Tool, ToolClass, ToolContext, ToolOutput, save_long, str_arg};
use crate::mcp::ToolDef;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
/// Output kept per stream. Past this the bytes are dropped, but reading goes on, so a command never blocks
/// on a full pipe.
const CAPTURE_LIMIT: usize = 10 * 1024 * 1024;
/// How long output is still read after the shell has exited. A process left running in the background may
/// hold the pipe open, and it must not hold up the result.
const OUTPUT_GRACE: Duration = Duration::from_secs(1);
/// Variables that commands never see, whatever the environment holds.
const HIDDEN_VARIABLES: [&str; 1] = ["VATRA_API_KEY"];
/// Run before each PowerShell command, so its output comes back as UTF-8.
const POWERSHELL_SETUP: &str = "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; ";
const MISSING_BASH: &str = if cfg!(windows) {
    "No Git Bash was found on PATH. Install Git for Windows (https://git-scm.com/download/win) with its cmd folder \
     on PATH, then restart Vatra."
} else {
    "No bash was found on PATH. Install bash and put it on PATH, then restart Vatra."
};
const BASH_DESCRIPTION: &str = "Runs a command in Git Bash, in the working folder, and returns its output. Each call starts \
     a new shell, so directory changes and variables don't carry over. A non-zero exit code is reported as an error. \
     Output longer than 50,000 characters is saved to a file: the reply shows its start and the file's path. A \
     foreground command is stopped when its timeout runs out. With run_in_background the command starts at once and \
     the reply gives a task id; the result comes back on a later step, when the command ends. TaskStop stops it.";
const POWERSHELL_DESCRIPTION: &str = "Runs a command in Windows PowerShell, in the working folder, and returns its \
     output. Each call starts a new shell, so directory changes and variables don't carry over. A non-zero exit code \
     is reported as an error. Output longer than 50,000 characters is saved to a file: the reply shows its start and \
     the file's path. A foreground command is stopped when its timeout runs out. With run_in_background the command \
     starts at once and the reply gives a task id; the result comes back on a later step, when the command ends. \
     TaskStop stops it.";

enum Kind {
    /// The bash to run, or None when none was found.
    Bash(Option<PathBuf>),
    PowerShell,
}

struct ShellTool {
    kind: Kind,
}

/// What TaskStop found for an id.
pub enum StopOutcome {
    Stopped,
    /// The command had already ended.
    Finished,
    Unknown,
}

/// A background command that ended, as the next step reports it.
#[derive(Debug)]
pub struct Finished {
    pub id: String,
    pub text: String,
}

/// The background commands of a session. They run until they end or are stopped. Dropping the list stops the
/// ones still running.
#[derive(Default)]
pub struct Background {
    tasks: Mutex<HashMap<String, Task>>,
    finished: Arc<Mutex<Vec<Finished>>>,
    started: AtomicU64,
}

struct Task {
    tree: Arc<ProcessGroup>,
    stopped: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl Background {
    pub fn new() -> Background {
        Background::default()
    }

    /// Starts a command in the background and returns its task id. Its result is queued for `take_finished`
    /// when it ends, unless it was stopped first.
    pub fn start(&self, mut command: Command, session_dir: &Path) -> io::Result<String> {
        let mut child = command.spawn()?;
        let pid = child.id();
        let tree = Arc::new(ProcessGroup::new()?);
        supervise(&tree, &mut child)?;
        let id = format!("task-{}", self.started.fetch_add(1, Ordering::SeqCst) + 1);
        let stopped = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let stdout = read_in_background(
            child.stdout.take().expect("stdout is configured"),
            CAPTURE_LIMIT,
        );
        let stderr = read_in_background(
            child.stderr.take().expect("stderr is configured"),
            CAPTURE_LIMIT,
        );
        let (queue, session_dir, label) = (
            Arc::clone(&self.finished),
            session_dir.to_path_buf(),
            id.clone(),
        );
        let (stopped_flag, done_flag, reaped_tree) =
            (Arc::clone(&stopped), Arc::clone(&done), Arc::clone(&tree));
        tokio::spawn(async move {
            let status = child.wait().await;
            if let Some(pid) = pid {
                reaped_tree.release(pid);
            }
            let stdout = stdout.finish().await;
            let stderr = stderr.finish().await;
            if !stopped_flag.load(Ordering::SeqCst) {
                let text = match status {
                    Ok(status) => render(status, stdout, stderr, &session_dir).text,
                    Err(error) => format!("Couldn't wait for the command: {error}"),
                };
                let text = format!("Background command {label} finished.\n{text}");
                lock(&queue).push(Finished { id: label, text });
            }
            done_flag.store(true, Ordering::SeqCst);
        });
        lock(&self.tasks).insert(
            id.clone(),
            Task {
                tree,
                stopped,
                done,
            },
        );
        Ok(id)
    }

    pub fn stop(&self, id: &str) -> StopOutcome {
        let Some(task) = lock(&self.tasks).remove(id) else {
            return StopOutcome::Unknown;
        };
        if task.done.load(Ordering::SeqCst) {
            return StopOutcome::Finished;
        }
        task.stopped.store(true, Ordering::SeqCst);
        // An error means the tree is already gone, which is what stopping wants.
        let _ = task.tree.kill_all();
        StopOutcome::Stopped
    }

    /// The results of the background commands that ended since the last call.
    pub fn take_finished(&self) -> Vec<Finished> {
        std::mem::take(&mut *lock(&self.finished))
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        for task in lock(&self.tasks).values() {
            let _ = task.tree.kill_all();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Puts a new child in its tree. A child that has already exited can't be added, and that is fine. Any other
/// failure stops the child, since a command that can't be supervised must not run unwatched.
fn supervise(tree: &ProcessGroup, child: &mut Child) -> io::Result<()> {
    match tree.adopt(child) {
        Ok(()) => Ok(()),
        Err(_) if matches!(child.try_wait(), Ok(Some(_))) => Ok(()),
        Err(error) => {
            let _ = child.start_kill();
            Err(error)
        }
    }
}

/// What a stream printed, up to a limit. `cut` says more was printed than was kept.
#[derive(Default)]
pub(crate) struct Captured {
    pub(crate) bytes: Vec<u8>,
    pub(crate) cut: bool,
}

/// A stream read in the background, so its output can be taken once the shell is done.
struct Pipe {
    captured: Arc<Mutex<Captured>>,
    reader: JoinHandle<()>,
}

impl Pipe {
    /// Gives the stream the grace period to end, then stops reading and returns what was read.
    async fn finish(mut self) -> Captured {
        let _ = tokio::time::timeout(OUTPUT_GRACE, &mut self.reader).await;
        std::mem::take(&mut *lock(&self.captured))
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

fn read_in_background(stream: impl AsyncRead + Unpin + Send + 'static, limit: usize) -> Pipe {
    let captured = Arc::new(Mutex::new(Captured::default()));
    let reader = tokio::spawn(read_into(stream, limit, Arc::clone(&captured)));
    Pipe { captured, reader }
}

/// Reads a stream to its end, keeping up to `limit` bytes.
async fn read_into(
    mut stream: impl AsyncRead + Unpin,
    limit: usize,
    captured: Arc<Mutex<Captured>>,
) {
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                let mut shared = lock(&captured);
                let room = limit - shared.bytes.len();
                shared.bytes.extend_from_slice(&chunk[..read.min(room)]);
                shared.cut |= read > room;
            }
        }
    }
}

/// The timeout argument in milliseconds: 120 seconds by default, and never more than 600 seconds.
fn timeout_of(arguments: &Value) -> Duration {
    let millis = arguments
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    Duration::from_millis(millis.clamp(1, MAX_TIMEOUT_MS))
}

/// The command that runs `text` in Git Bash, set up the way every shell command of the engine is: no hidden
/// variables, the working folder, and a group of its own.
pub(crate) fn bash_command(bash: &Path, text: &str, hidden: &[String], cwd: &Path) -> Command {
    let mut command = Command::new(bash);
    command.arg("-c").arg(text);
    finish_command(command, hidden, cwd)
}

fn finish_command(mut command: Command, hidden: &[String], cwd: &Path) -> Command {
    for name in HIDDEN_VARIABLES
        .iter()
        .copied()
        .chain(hidden.iter().map(String::as_str))
    {
        command.env_remove(name);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if !cwd.as_os_str().is_empty() {
        command.current_dir(cwd);
    }
    // A group of its own, so the tree can be ended on Unix as well.
    #[cfg(unix)]
    command.process_group(0);
    command
}

impl ShellTool {
    fn command(&self, text: &str, context: &ToolContext) -> Result<Command, String> {
        let command = match &self.kind {
            Kind::Bash(Some(bash)) => {
                let mut command = Command::new(bash);
                command.arg("-c").arg(text);
                command
            }
            Kind::Bash(None) => return Err(MISSING_BASH.into()),
            Kind::PowerShell => {
                let mut command = Command::new("powershell");
                command
                    .args(["-NoProfile", "-NonInteractive", "-Command"])
                    .arg(format!("{POWERSHELL_SETUP}{text}"));
                command
            }
        };
        Ok(finish_command(command, &context.hidden_env, &context.cwd))
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn def(&self) -> ToolDef {
        let (name, description) = match &self.kind {
            Kind::Bash(_) => ("Bash", BASH_DESCRIPTION),
            Kind::PowerShell => ("PowerShell", POWERSHELL_DESCRIPTION),
        };
        ToolDef {
            name: name.into(),
            description: description.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command to run."
                    },
                    "description": {
                        "type": "string",
                        "description": "A short description of what the command does."
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "How long a foreground command may run, in milliseconds. Defaults to 120000, and at most 600000.",
                        "default": 120000,
                        "maximum": 600000
                    },
                    "run_in_background": {
                        "type": "boolean",
                        "description": "Start the command and return at once with a task id. Its result comes back on a later step."
                    }
                },
                "required": ["command"]
            }),
        }
    }

    fn class(&self, _arguments: &Value, _context: &ToolContext) -> ToolClass {
        ToolClass::Execute
    }

    fn parallel_safe(&self) -> bool {
        false
    }

    async fn call(&self, arguments: Value, context: &ToolContext) -> ToolOutput {
        let Some(text) = str_arg(&arguments, "command") else {
            return ToolOutput::error("command is required.");
        };
        let command = match self.command(text, context) {
            Ok(command) => command,
            Err(message) => return ToolOutput::error(message),
        };
        if arguments
            .get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return match context.background.start(command, &context.session_dir) {
                Ok(id) => ToolOutput::ok(format!(
                    "Started {id} in the background. Its result comes back on a later step, once the command ends. \
                     To stop it, call TaskStop with task_id {id}."
                )),
                Err(error) => ToolOutput::error(format!("Couldn't start the command: {error}")),
            };
        }
        run_foreground(command, timeout_of(&arguments), context).await
    }
}

async fn run_foreground(command: Command, timeout: Duration, context: &ToolContext) -> ToolOutput {
    match run_shell(command, timeout, &context.tree).await {
        Ok(ran) => render(ran.status, ran.stdout, ran.stderr, &context.session_dir),
        Err(error) => ToolOutput::error(error.to_string()),
    }
}

/// How a shell command ended, with what it printed.
pub(crate) struct Ran {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Captured,
    pub(crate) stderr: Captured,
}

/// Why a shell command did not run to its end.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ShellError {
    #[error("Couldn't start the command: {0}")]
    Start(io::Error),
    #[error("The command can't be supervised, so it was stopped: {0}")]
    Supervise(io::Error),
    #[error("The command timed out after {} ms and was stopped.", .0.as_millis())]
    TimedOut(Duration),
    #[error("Couldn't wait for the command: {0}")]
    Wait(io::Error),
}

/// Runs a command in `tree` and waits for it, up to `timeout`. A command that runs out of time is stopped with
/// everything it started.
pub(crate) async fn run_shell(
    mut command: Command,
    timeout: Duration,
    tree: &ProcessGroup,
) -> Result<Ran, ShellError> {
    let mut child = command.spawn().map_err(ShellError::Start)?;
    let pid = child.id();
    supervise(tree, &mut child).map_err(ShellError::Supervise)?;
    struct StopOnCancel<'a> {
        tree: &'a ProcessGroup,
        armed: bool,
    }
    impl Drop for StopOnCancel<'_> {
        fn drop(&mut self) {
            if self.armed {
                let _ = self.tree.kill_all();
            }
        }
    }
    let mut cancel = StopOnCancel { tree, armed: true };
    let stdout = read_in_background(
        child.stdout.take().expect("stdout is configured"),
        CAPTURE_LIMIT,
    );
    let stderr = read_in_background(
        child.stderr.take().expect("stderr is configured"),
        CAPTURE_LIMIT,
    );
    let Ok(status) = tokio::time::timeout(timeout, child.wait()).await else {
        // The shell and everything it started end together.
        let _ = tree.kill_all();
        let _ = child.start_kill();
        return Err(ShellError::TimedOut(timeout));
    };
    let stdout = stdout.finish().await;
    let stderr = stderr.finish().await;
    let status = status.map_err(ShellError::Wait)?;
    if let Some(pid) = pid {
        tree.release(pid);
    }
    cancel.armed = false;
    Ok(Ran {
        status,
        stdout,
        stderr,
    })
}

/// The reply for a command that ended: its output, then its errors, and an error result when it failed.
fn render(
    status: ExitStatus,
    stdout: Captured,
    stderr: Captured,
    session_dir: &Path,
) -> ToolOutput {
    let mut text = String::from_utf8_lossy(&stdout.bytes)
        .trim_end()
        .to_string();
    let errors = String::from_utf8_lossy(&stderr.bytes);
    if !errors.trim_end().is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str("stderr:\n");
        text.push_str(errors.trim_end());
    }
    if stdout.cut || stderr.cut {
        text.push_str(&format!(
            "\n\n(Output past {CAPTURE_LIMIT} bytes was dropped.)"
        ));
    }
    if text.is_empty() {
        text = "(no output)".into();
    }
    if text.chars().count() > SAVE_OVER_CHARS {
        text = save_long(&text, session_dir, "command-output");
    }
    if status.success() {
        return ToolOutput::ok(text);
    }
    let code = match status.code() {
        Some(code) => format!("Exit code {code}"),
        None => "The command was stopped by a signal.".into(),
    };
    ToolOutput::error(format!("{code}\n{text}"))
}

/// The Bash tool, with the bash found on PATH.
pub fn bash_tool() -> Arc<dyn Tool> {
    bash_tool_with(find_bash(&std::env::var_os("PATH").unwrap_or_default()))
}

/// The Bash tool with the bash it runs. Without one, every call says how to fix that.
pub fn bash_tool_with(bash: Option<PathBuf>) -> Arc<dyn Tool> {
    Arc::new(ShellTool {
        kind: Kind::Bash(bash),
    })
}

/// The PowerShell tool. It exists only on Windows.
pub fn powershell_tool() -> Option<Arc<dyn Tool>> {
    cfg!(windows).then(|| {
        Arc::new(ShellTool {
            kind: Kind::PowerShell,
        }) as Arc<dyn Tool>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_is_clamped() {
        assert_eq!(timeout_of(&json!({})), Duration::from_millis(120_000));
        assert_eq!(
            timeout_of(&json!({"timeout": 900_000})),
            Duration::from_millis(600_000)
        );
        assert_eq!(timeout_of(&json!({"timeout": 0})), Duration::from_millis(1));
    }

    #[tokio::test]
    async fn capture_keeps_up_to_the_limit() {
        let cut = read_in_background(&b"abcdefghij"[..], 4).finish().await;
        assert_eq!(cut.bytes, b"abcd");
        assert!(cut.cut);
        let whole = read_in_background(&b"abc"[..], 4).finish().await;
        assert_eq!(whole.bytes, b"abc");
        assert!(!whole.cut);
    }
}
