use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::mpsc;
use vatra_core::{Agent, Event, ProcessGroup, TurnRequest, find_bash};
use vatra_engine::{
    AllowAll, Background, EngineAgent, NoQuestions, Tool, ToolContext, ToolOutput, bash_tool,
    bash_tool_with, powershell_tool, task_stop_tool,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

/// A folder whose name holds non-ASCII text, so every path test covers it.
fn workspace() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("Răzvan Bash")
        .tempdir()
        .unwrap()
}

fn context(cwd: &Path, session_dir: &Path) -> ToolContext {
    let (events, _) = mpsc::unbounded_channel();
    ToolContext {
        cwd: cwd.to_path_buf(),
        project_root: cwd.to_path_buf(),
        session_dir: session_dir.to_path_buf(),
        read_files: Arc::new(Mutex::new(HashSet::new())),
        events,
        questions: Arc::new(NoQuestions),
        tree: Arc::new(ProcessGroup::new().unwrap()),
        background: Arc::new(Background::new()),
        hidden_env: vec![],
        memory_dir: None,
    }
}

async fn call(tool: Arc<dyn Tool>, context: &ToolContext, arguments: Value) -> ToolOutput {
    tool.call(arguments, context).await
}

static NEXT_MARKER: AtomicU64 = AtomicU64::new(1);

/// A sleep that only this test run starts, as (command, marker). The marker is a number unique to the run.
/// It shows in the command line of the shell and of the sleep, so the test finds its own processes without
/// going by name.
fn sleeping() -> (String, String) {
    let marker = format!(
        "300.{}{:04}",
        std::process::id(),
        NEXT_MARKER.fetch_add(1, Ordering::SeqCst)
    );
    (format!("sleep {marker}"), marker)
}

/// Process ids whose command line holds `marker`. The marker goes in an environment variable, so the
/// PowerShell process that searches doesn't match itself.
#[cfg(windows)]
fn running(marker: &str) -> Vec<u32> {
    let script = "Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like ('*' + $env:VATRA_TEST_MARKER + '*') } | ForEach-Object { $_.ProcessId }";
    let output = StdCommand::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("VATRA_TEST_MARKER", marker)
        .output()
        .expect("PowerShell runs");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

#[cfg(unix)]
fn running(marker: &str) -> Vec<u32> {
    let output = StdCommand::new("ps")
        .args(["-eo", "pid=,args="])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains(marker))
        .filter_map(|line| line.split_whitespace().next()?.parse().ok())
        .collect()
}

/// Ends the processes with the test's marker, by id, when the test ends, pass or fail.
struct Leftovers(String);

impl Drop for Leftovers {
    fn drop(&mut self) {
        for pid in running(&self.0) {
            #[cfg(windows)]
            let _ = StdCommand::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .output();
            #[cfg(unix)]
            let _ = StdCommand::new("kill")
                .args(["-9", &pid.to_string()])
                .output();
        }
    }
}

/// Polls for up to 15 seconds until processes with the marker are present or gone.
async fn until(marker: &str, present: bool) -> bool {
    for _ in 0..30 {
        if running(marker).is_empty() != present {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

#[tokio::test]
async fn runs_and_returns_output() {
    let dir = workspace();
    let context = context(dir.path(), dir.path());

    let out = call(bash_tool(), &context, json!({"command": "echo hello"})).await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(out.text, "hello");
}

#[tokio::test]
async fn nonzero_exit_is_error_with_code() {
    let dir = workspace();
    let context = context(dir.path(), dir.path());

    let out = call(
        bash_tool(),
        &context,
        json!({"command": "echo oops >&2; exit 3"}),
    )
    .await;

    assert!(out.is_error);
    assert!(out.text.contains("Exit code 3"), "{}", out.text);
    assert!(out.text.contains("oops"), "{}", out.text);
}

#[tokio::test]
async fn timeout_default_120s_max_600s() {
    let schema = bash_tool().def().parameters["properties"]["timeout"].clone();
    assert_eq!(schema["default"], 120_000);
    assert_eq!(schema["maximum"], 600_000);

    // A short timeout stops a command that would run for minutes.
    let dir = workspace();
    let context = context(dir.path(), dir.path());
    let (command, marker) = sleeping();
    let _leftovers = Leftovers(marker.clone());
    let started = Instant::now();

    let out = call(
        bash_tool(),
        &context,
        json!({"command": command, "timeout": 300}),
    )
    .await;

    assert!(out.is_error);
    assert!(out.text.contains("timed out"), "{}", out.text);
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(
        until(&marker, false).await,
        "the timed out command is still running"
    );
}

#[tokio::test]
async fn output_over_limit_goes_to_file() {
    let dir = workspace();
    let sessions = dir.path().join("session");
    let context = context(dir.path(), &sessions);

    let out = call(
        bash_tool(),
        &context,
        json!({"command": "head -c 60000 /dev/zero | tr '\\0' a"}),
    )
    .await;

    assert!(!out.is_error, "{}", out.text);
    let saved: Vec<PathBuf> = fs::read_dir(&sessions)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert_eq!(
        fs::read_to_string(&saved[0]).unwrap().chars().count(),
        60_000
    );
    assert!(out.text.starts_with("aaaa"));
    assert!(
        out.text.chars().count() < 50_000,
        "the reply holds {} characters",
        out.text.chars().count()
    );
    assert!(
        out.text.contains(&saved[0].display().to_string()),
        "{}",
        out.text
    );
}

#[tokio::test]
async fn cwd_is_project() {
    let dir = workspace();
    fs::write(dir.path().join("marker.txt"), "in the project").unwrap();
    let context = context(dir.path(), dir.path());

    let out = call(bash_tool(), &context, json!({"command": "cat marker.txt"})).await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(out.text, "in the project");
}

#[tokio::test]
async fn path_with_spaces_and_diacritics() {
    let dir = workspace();
    let name = "fișier nou ăâ.txt";
    fs::write(dir.path().join(name), "ok here").unwrap();
    let context = context(dir.path(), dir.path());

    let out = call(
        bash_tool(),
        &context,
        json!({"command": format!("cat \"{name}\"")}),
    )
    .await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(out.text, "ok here");
}

#[cfg(windows)]
#[test]
fn finds_git_bash_not_wsl() {
    let root = tempfile::tempdir().unwrap();
    let with_bash = |folder: PathBuf| {
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("bash.exe"), b"").unwrap();
        folder
    };
    let system32 = with_bash(root.path().join("Windows").join("System32"));
    let windows_apps = with_bash(root.path().join("WindowsApps"));
    let git = root.path().join("Program Files").join("Git");
    let git_bin = with_bash(git.join("bin"));
    // Git for Windows puts only its cmd folder on PATH by default.
    let git_cmd = git.join("cmd");
    fs::create_dir_all(&git_cmd).unwrap();

    let path = std::env::join_paths([system32.clone(), windows_apps.clone(), git_cmd]).unwrap();
    assert_eq!(find_bash(&path), Some(git_bin.join("bash.exe")));

    let without_git = std::env::join_paths([system32, windows_apps]).unwrap();
    assert_eq!(find_bash(&without_git), None);
    assert_eq!(find_bash(OsStr::new("")), None);
}

#[tokio::test]
async fn no_bash_reports_how_to_fix() {
    let dir = workspace();
    let context = context(dir.path(), dir.path());
    let tool = bash_tool_with(find_bash(OsStr::new("")));

    let out = call(tool, &context, json!({"command": "echo hi"})).await;

    assert!(out.is_error);
    assert!(out.text.contains("PATH"), "{}", out.text);
}

#[tokio::test]
async fn stopping_turn_kills_tree() {
    let dir = workspace();
    let (sleep, marker) = sleeping();
    let _leftovers = Leftovers(marker.clone());
    let context = context(dir.path(), dir.path());
    let command = format!("{sleep} & {sleep}");

    let running_context = context.clone();
    let running = tokio::spawn(async move {
        call(bash_tool(), &running_context, json!({"command": command})).await
    });
    assert!(until(&marker, true).await, "the sleeps never started");
    // Cancelling the call must end its children even while the caller retains the turn context.
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    assert!(
        until(&marker, false).await,
        "cancelled command left processes running"
    );
    drop(context);
    assert!(
        until(&marker, false).await,
        "the turn's processes are still running"
    );
}

#[tokio::test]
async fn background_returns_immediately_with_id() {
    let dir = workspace();
    let (command, marker) = sleeping();
    let _leftovers = Leftovers(marker.clone());
    let context = context(dir.path(), dir.path());
    let started = Instant::now();

    let out = call(
        bash_tool(),
        &context,
        json!({"command": command, "run_in_background": true}),
    )
    .await;

    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("task-1"), "{}", out.text);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the call waited for the command"
    );
    assert!(
        until(&marker, true).await,
        "the background command never started"
    );
    drop(context);
    assert!(until(&marker, false).await);
}

#[tokio::test]
async fn background_result_arrives_on_next_step() {
    let dir = workspace();
    let context = context(dir.path(), dir.path());

    let out = call(
        bash_tool(),
        &context,
        json!({"command": "echo background-done", "run_in_background": true}),
    )
    .await;
    assert!(!out.is_error, "{}", out.text);
    assert!(!out.text.contains("background-done"), "{}", out.text);

    // The engine drains the queue at the start of each step, so the result shows up there.
    let mut results = vec![];
    for _ in 0..100 {
        results = context.background.take_finished();
        if !results.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(
        results[0].text.contains("background-done"),
        "{}",
        results[0].text
    );
}

#[tokio::test]
async fn task_stop_kills_background() {
    let dir = workspace();
    let (command, marker) = sleeping();
    let _leftovers = Leftovers(marker.clone());
    let context = context(dir.path(), dir.path());
    call(
        bash_tool(),
        &context,
        json!({"command": command, "run_in_background": true}),
    )
    .await;
    assert!(
        until(&marker, true).await,
        "the background command never started"
    );

    let out = call(task_stop_tool(), &context, json!({"task_id": "task-1"})).await;

    assert!(!out.is_error, "{}", out.text);
    assert!(
        until(&marker, false).await,
        "TaskStop left the command running"
    );
    // A stopped command reports nothing when it ends.
    assert!(context.background.take_finished().is_empty());
}

#[tokio::test]
async fn background_ends_with_session() {
    let dir = workspace();
    let (command, marker) = sleeping();
    let _leftovers = Leftovers(marker.clone());
    let background = Arc::new(Background::new());
    let mut context = context(dir.path(), dir.path());
    context.background = Arc::clone(&background);
    call(
        bash_tool(),
        &context,
        json!({"command": command, "run_in_background": true}),
    )
    .await;
    assert!(
        until(&marker, true).await,
        "the background command never started"
    );

    drop(context);
    drop(background);

    assert!(
        until(&marker, false).await,
        "the session ended but its background command is still running"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn powershell_only_on_windows() {
    let dir = workspace();
    let context = context(dir.path(), dir.path());
    let tool = powershell_tool().expect("PowerShell is on Windows");

    let out = call(tool, &context, json!({"command": "Write-Output hello-ps"})).await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(out.text, "hello-ps");
}

#[cfg(not(windows))]
#[test]
fn powershell_only_on_windows() {
    assert!(powershell_tool().is_none());
}

#[tokio::test]
async fn command_leaving_background_process_returns_promptly() {
    let dir = workspace();
    let (sleep, marker) = sleeping();
    let _leftovers = Leftovers(marker.clone());
    let context = context(dir.path(), dir.path());
    let started = Instant::now();

    let out = call(
        bash_tool(),
        &context,
        json!({"command": format!("echo hi; {sleep} &"), "timeout": 60_000}),
    )
    .await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(out.text, "hi");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the reply waited for the background process"
    );
    // The process is still running, so the test really left one behind.
    assert!(
        until(&marker, true).await,
        "the background process never started"
    );
    // On Windows the job ends the process with the context. Elsewhere the released process is ended by the
    // Leftovers guard, which goes by marker when the test ends.
    #[cfg(windows)]
    {
        drop(context);
        assert!(
            until(&marker, false).await,
            "the turn's processes are still running"
        );
    }
}

#[tokio::test]
async fn api_key_isnt_visible_to_commands() {
    // Set for this test process only. Commands must not see the key, nor the variable listed as hidden.
    unsafe {
        std::env::set_var("VATRA_API_KEY", "fake-key-for-this-test");
        std::env::set_var("VATRA_TEST_HIDDEN", "hidden");
        std::env::set_var("VATRA_TEST_VISIBLE", "shown");
    }
    let dir = workspace();
    let mut context = context(dir.path(), dir.path());
    context.hidden_env = vec!["VATRA_TEST_HIDDEN".into()];

    let out = call(bash_tool(), &context, json!({"command": "env"})).await;

    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains("VATRA_TEST_VISIBLE=shown"),
        "the control variable is missing"
    );
    assert!(
        !out.text.contains("VATRA_API_KEY"),
        "the API key reached the command"
    );
    assert!(
        !out.text.contains("VATRA_TEST_HIDDEN"),
        "a hidden variable reached the command"
    );
}

#[tokio::test]
async fn background_result_reaches_the_model_on_next_step() {
    let dir = workspace();
    let server = MockServer::start().await;
    // The first step answers slowly, so the background command has ended before the second step begins.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(common::tool_call("call-1", "Nothing", "{}"))
                .set_delay(Duration::from_secs(3)),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(common::text("done")))
        .mount(&server)
        .await;

    let background = Arc::new(Background::new());
    let bash = find_bash(&std::env::var_os("PATH").unwrap_or_default()).expect("bash is on PATH");
    let mut command = tokio::process::Command::new(bash);
    command
        .args(["-c", "echo background-done"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    background.start(command, dir.path()).unwrap();

    let mut setup = common::setup(&server, Arc::new(AllowAll), None, vec![], 5).await;
    setup.context.background = Arc::clone(&background);
    let agent = EngineAgent::new(setup);

    let (sink, mut received) = mpsc::unbounded_channel();
    agent
        .run_turn(
            TurnRequest { files: vec![],
                history: vec![],
                prompt: "start".into(),
                native_session: None,
            },
            sink,
        )
        .await
        .unwrap();
    let mut first_turn = vec![];
    while let Some(event) = received.recv().await {
        first_turn.push(event);
    }

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        String::from_utf8_lossy(&requests[1].body).contains("background-done"),
        "the step-2 request lacks the result"
    );
    assert!(
        first_turn
            .iter()
            .any(|event| matches!(event, Event::BackgroundResult { text, .. } if text.contains("background-done"))),
        "the result was not journaled: {first_turn:?}"
    );

    // The journaled result is replayed from history, so the next turn's model sees it too.
    let (sink, mut received) = mpsc::unbounded_channel();
    let request = TurnRequest { files: vec![],
        history: first_turn,
        prompt: "again".into(),
        native_session: None,
    };
    agent.run_turn(request, sink).await.unwrap();
    while received.recv().await.is_some() {}

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(
        String::from_utf8_lossy(&requests[2].body).contains("background-done"),
        "history lost the result"
    );
}
