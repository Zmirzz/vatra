use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use vatra_acp::{AcpAgent, AcpConfig, PermissionHandler};
use vatra_core::{Agent, Event, TurnRequest};

#[derive(Default)]
struct Permissions {
    allow: bool,
    hang: bool,
    delay: Duration,
    seen: Mutex<Vec<Value>>,
}

#[async_trait]
impl PermissionHandler for Permissions {
    async fn approve(&self, tool: &Value) -> bool {
        self.seen.lock().unwrap().push(tool.clone());
        if self.hang {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(self.delay).await;
        self.allow
    }
}

struct ExpandingPermissions;

#[async_trait]
impl PermissionHandler for ExpandingPermissions {
    async fn approve(&self, _: &Value) -> bool {
        false
    }
    async fn skill_body(&self, _: &vatra_extensions::Skill) -> Result<String, String> {
        Ok("Gated command output then $ARGUMENTS".into())
    }
}

#[tokio::test]
async fn service_skill_expansion_hook_runs_before_substitution_and_is_journaled() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(
        folder.path().join("SKILL.md"),
        "---\nname: scripted\n---\n!`echo should-be-gated`",
    )
    .unwrap();
    let mut config = config(folder.path(), "normal");
    config
        .extras
        .skills
        .push(vatra_extensions::parse_skill(folder.path()).unwrap());
    let agent = AcpAgent::new(config, Arc::new(ExpandingPermissions));
    let mut request = request();
    request.prompt = "/scripted hello".into();
    let (sink, mut events) = mpsc::unbounded_channel();
    agent.run_turn(request, sink).await.unwrap();
    assert!(
        trace(folder.path())[2]["params"]["prompt"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Gated command output then hello")
    );
    let mut loaded = false;
    while let Some(event) = events.recv().await {
        loaded |= matches!(event, Event::SkillLoaded { name, text } if name == "scripted" && text.contains("Gated command output then hello"));
    }
    assert!(loaded);
}

fn config(folder: &Path, scenario: &str) -> AcpConfig {
    AcpConfig {
        child_setup: None,
        name: "external".into(),
        binary: env!("CARGO_BIN_EXE_fake-acp").into(),
        args: vec![
            scenario.into(),
            folder.join("trace.jsonl").display().to_string(),
        ],
        cwd: folder.to_path_buf(),
        hidden_env: vec!["USERPROFILE".into()],
        startup_timeout: Duration::from_secs(3),
        turn_timeout: Duration::from_secs(5),
        extras: Default::default(),
        commands: Default::default(),
        model: None,
        effort: None,
    }
}

#[tokio::test]
async fn extras_reach_wire_only_on_new_session() {
    let folder = tempfile::tempdir().unwrap();
    let mut config = config(folder.path(), "extras");
    config.extras.preamble = Some("Instructions that belong in the first prompt".into());
    config.extras.servers.push(vatra_core::McpServerConfig {
        name: "local".into(),
        command: Some("tool.exe".into()),
        env: [(
            "SERVER_KEY".into(),
            "synthetic-secret-must-not-be-journaled".into(),
        )]
        .into(),
        ..Default::default()
    });
    config.extras.servers.push(vatra_core::McpServerConfig {
        name: "remote".into(),
        url: Some("https://example.test".into()),
        ..Default::default()
    });
    let agent = AcpAgent::new(config.clone(), Arc::new(Permissions::default()));
    let (sink, mut events) = mpsc::unbounded_channel();
    agent.run_turn(request(), sink).await.unwrap();
    let first = trace(folder.path());
    assert_eq!(
        first[1]["params"]["mcpServers"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        first[1]["params"]["mcpServers"][0]["env"][0]["value"],
        "synthetic-secret-must-not-be-journaled"
    );
    assert!(
        !first[1]["params"]["mcpServers"][0]["args"]
            .to_string()
            .contains("synthetic-secret")
    );
    assert!(
        first[2]["params"]["prompt"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Instructions that belong")
    );
    let mut warned = false;
    while let Some(event) = events.recv().await {
        assert!(!format!("{event:?}").contains("synthetic-secret"));
        warned |= matches!(event, Event::Warning { message, .. } if message.contains("HTTP"));
    }
    assert!(warned);
    let mut resumed = request();
    resumed.native_session = Some("acp-session-1".into());
    let (sink, _) = mpsc::unbounded_channel();
    agent.run_turn(resumed, sink).await.unwrap();
    let all = trace(folder.path());
    assert!(
        !all.last().unwrap()["params"]["prompt"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Instructions that belong")
    );
}

#[tokio::test]
async fn resume_is_preferred_and_available_commands_are_retained() {
    let folder = tempfile::tempdir().unwrap();
    let config = config(folder.path(), "resume");
    let commands = config.commands.clone();
    let agent = AcpAgent::new(config, Arc::new(Permissions::default()));
    let mut req = request();
    req.native_session = Some("acp-session-1".into());
    let (sink, _) = mpsc::unbounded_channel();
    agent.run_turn(req, sink).await.unwrap();
    assert_eq!(trace(folder.path())[1]["method"], "session/resume");
    assert_eq!(commands.latest()[0]["name"], "compact");
    assert!(
        !trace(folder.path())
            .iter()
            .any(|entry| entry["method"] == "authenticate")
    );
}

#[tokio::test]
async fn plans_and_diffs_use_existing_views_but_usage_stays_unmapped() {
    let (result, events, _) = run("views", request()).await;
    result.unwrap();
    assert!(events.iter().any(|event| matches!(event, Event::ToolCall { name, arguments, .. } if name == "TodoWrite" && arguments["todos"][0]["content"] == "Implement")));
    assert!(events.iter().any(|event| matches!(event, Event::ToolCall { name, arguments, .. } if name == "Edit" && arguments["new_string"] == "new")));
    assert!(events.iter().any(|event| matches!(event, Event::ToolCall { name, arguments, .. } if name == "Edit" && arguments["file_path"] == "second.txt")));
    assert!(
        events.iter().any(
            |event| matches!(event, Event::Unmapped { raw, .. } if raw.contains("usage_update"))
        )
    );
}

fn request() -> TurnRequest {
    TurnRequest { files: vec![],
        history: vec![],
        prompt: "Test prompt\nwith a newline and diacritics: ăîșț".into(),
        native_session: None,
    }
}

fn trace(folder: &Path) -> Vec<Value> {
    std::fs::read_to_string(folder.join("trace.jsonl"))
        .unwrap_or_default()
        // Pollers may read while the fixture is still appending the final JSON frame.
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

async fn run(
    scenario: &str,
    request: TurnRequest,
) -> (
    Result<vatra_core::TurnOutcome, vatra_core::AgentError>,
    Vec<Event>,
    Vec<Value>,
) {
    let folder = tempfile::tempdir().unwrap();
    let agent = AcpAgent::new(
        config(folder.path(), scenario),
        Arc::new(Permissions::default()),
    );
    let (sink, mut events) = mpsc::unbounded_channel();
    let result = agent.run_turn(request, sink).await;
    let mut collected = vec![];
    while let Some(event) = events.recv().await {
        collected.push(event);
    }
    (result, collected, trace(folder.path()))
}

#[tokio::test]
async fn handshake_streaming_tools_and_unknown_updates() {
    let (result, events, trace) = run("normal", request()).await;
    assert_eq!(
        result.unwrap().native_session.as_deref(),
        Some("acp-session-1")
    );
    let methods: Vec<_> = trace
        .iter()
        .filter_map(|item| item["method"].as_str())
        .collect();
    assert_eq!(methods, ["initialize", "session/new", "session/prompt"]);
    assert_eq!(
        trace[0]["params"]["clientCapabilities"],
        serde_json::json!({})
    );
    assert_eq!(trace[2]["params"]["prompt"][0]["text"], request().prompt);
    let texts: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::AssistantDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["Hello ", "world"]);
    assert!(events.contains(&Event::AssistantText {
        text: "Hello world".into()
    }));
    assert!(events.iter().any(|event| matches!(event, Event::ToolResult { output, is_error: false, .. } if output.contains("file contents"))));
    assert!(events.iter().any(
        |event| matches!(event, Event::Unmapped { raw, .. } if raw.contains("future_extension"))
    ));
    assert!(matches!(&events[0], Event::NativeSession { .. }));
}

#[tokio::test]
async fn resume_suppresses_history_replay_and_keeps_new_context() {
    let mut req = request();
    req.native_session = Some("acp-session-1".into());
    req.history = vec![
        Event::AssistantText {
            text: "old answer".into(),
        },
        Event::TurnCompleted {
            agent: "external".into(),
        },
        Event::HandoffResult {
            to_session: 2,
            text: "new external context".into(),
        },
    ];
    let (result, events, trace) = run("normal", req).await;
    result.unwrap();
    assert_eq!(trace[1]["method"], "session/load");
    assert_eq!(trace[1]["params"]["sessionId"], "acp-session-1");
    let text = trace[2]["params"]["prompt"][0]["text"].as_str().unwrap();
    assert!(text.contains("new external context"));
    assert!(!text.contains("old answer"));
    assert!(!format!("{events:?}").contains("replayed text"));
}

#[tokio::test]
async fn no_load_uses_bounded_transcript_and_compaction() {
    let mut req = request();
    req.native_session = Some("old-session".into());
    req.history = vec![
        Event::UserMessage {
            text: "obsolete context".into(),
        },
        Event::Compacted {
            summary: "decisions retained".into(),
        },
        Event::AssistantText {
            text: "latest answer".into(),
        },
    ];
    let (result, events, trace) = run("no-load", req).await;
    result.unwrap();
    assert_eq!(trace[1]["method"], "session/new");
    let text = trace[2]["params"]["prompt"][0]["text"].as_str().unwrap();
    assert!(text.contains("decisions retained") && text.contains("latest answer"));
    assert!(!text.contains("obsolete context"));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Warning { .. }))
    );
}

#[tokio::test]
async fn permission_options_are_selected_once_and_include_cached_command() {
    for allow in [false, true] {
        let folder = tempfile::tempdir().unwrap();
        let permissions = Arc::new(Permissions {
            allow,
            ..Default::default()
        });
        let agent = AcpAgent::new(config(folder.path(), "permission"), permissions.clone());
        let (sink, _) = mpsc::unbounded_channel();
        agent.run_turn(request(), sink).await.unwrap();
        assert_eq!(
            permissions.seen.lock().unwrap()[0]["rawInput"]["command"],
            "echo hello"
        );
        let trace = trace(folder.path());
        let response = trace
            .iter()
            .find(|item| item["id"] == "permission-1")
            .unwrap();
        assert_eq!(
            response["result"]["outcome"]["optionId"],
            if allow { "yes" } else { "no" }
        );
    }
    let (result, _, trace) = run("always-only", request()).await;
    result.unwrap();
    assert_eq!(
        trace
            .iter()
            .find(|item| item["id"] == "permission-1")
            .unwrap()["result"]["outcome"]["optionId"],
        "no"
    );
}

#[tokio::test]
async fn unsupported_client_methods_are_rejected() {
    let (result, _, trace) = run("unsupported", request()).await;
    result.unwrap();
    let reply = trace.iter().find(|item| item["id"] == "fs-1").unwrap();
    assert_eq!(reply["error"]["code"], -32601);
}

#[tokio::test]
async fn errors_explain_failures_without_echoing_remote_secrets() {
    for (scenario, expected) in [
        ("v2", "version 1"),
        ("auth", "Sign in"),
        ("crash", "closed its output"),
        ("bad-json", "invalid JSON"),
        ("oversized", "1 MiB"),
        ("refusal", "refusal"),
    ] {
        let (result, _, _) = run(scenario, request()).await;
        let error = result.unwrap_err().to_string();
        assert!(error.contains(expected), "{scenario}: {error}");
        assert!(!error.contains("secret must never"));
    }
}

#[tokio::test]
async fn hides_model_keys_and_handles_missing_executable() {
    // Set in this process so the child would inherit them unless the adapter removes them.
    // The test value is not a real key, and the fixture reports presence only.
    unsafe {
        std::env::set_var("VATRA_API_KEY", "test-value-not-a-key");
        std::env::set_var("VATRA_TEST_HIDDEN_KEY", "test-value-not-a-key");
        std::env::set_var("VATRA_TEST_VISIBLE", "visible");
    }
    let folder = tempfile::tempdir().unwrap();
    let mut env_config = config(folder.path(), "env");
    env_config.hidden_env.push("VATRA_TEST_HIDDEN_KEY".into());
    let agent = AcpAgent::new(env_config, Arc::new(Permissions::default()));
    let (sink, _) = mpsc::unbounded_channel();
    agent.run_turn(request(), sink).await.unwrap();
    let env = trace(folder.path())
        .into_iter()
        .find(|item| item["env"].is_object())
        .unwrap();
    assert_eq!(
        env["env"],
        json!({"apiKey":false,"hidden":false,"userProfile":false,"visible":true})
    );

    let folder = tempfile::tempdir().unwrap();
    let mut config = config(folder.path(), "normal");
    config.binary = folder.path().join("not-installed.exe");
    let agent = AcpAgent::new(config, Arc::new(Permissions::default()));
    let (sink, _) = mpsc::unbounded_channel();
    assert!(matches!(
        agent.run_turn(request(), sink).await,
        Err(vatra_core::AgentError::NotInstalled(_))
    ));
}

async fn until(folder: &Path, predicate: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let values = trace(folder);
            if predicate(&values) {
                return values;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("ACP fixture did not reach the expected state")
}

#[cfg(windows)]
struct ProcessHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl ProcessHandle {
    fn open(pid: u64) -> Self {
        // SYNCHRONIZE only: no tasklist/WMI dependency, and no permission to change the process.
        let handle = unsafe {
            windows_sys::Win32::System::Threading::OpenProcess(0x00100000, 0, pid as u32)
        };
        assert!(
            !handle.is_null(),
            "Couldn't open fixture process {pid}: {}",
            std::io::Error::last_os_error()
        );
        Self(handle)
    }

    fn exited(&self) -> bool {
        let status =
            unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(self.0, 0) };
        assert!(
            status == 0 || status == 258,
            "Couldn't wait on fixture process: {status}"
        );
        status == 0
    }
}

#[cfg(windows)]
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[tokio::test]
async fn dropping_turn_sends_cancel_and_ends_process_tree() {
    let folder = tempfile::tempdir().unwrap();
    let agent = AcpAgent::new(
        config(folder.path(), "hang"),
        Arc::new(Permissions::default()),
    );
    let (sink, _) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move { agent.run_turn(request(), sink).await });
    let values = until(folder.path(), |values| {
        values.iter().any(|value| value["childPid"].is_u64())
    })
    .await;
    let child = values
        .iter()
        .find_map(|value| value["childPid"].as_u64())
        .unwrap();
    #[cfg(windows)]
    let processes = [
        ProcessHandle::open(child),
        ProcessHandle::open(
            values
                .iter()
                .find_map(|value| value["parentPid"].as_u64())
                .unwrap(),
        ),
    ];
    #[cfg(windows)]
    assert!(processes.iter().all(|process| !process.exited()));
    task.abort();
    let _ = task.await;
    until(folder.path(), |values| {
        values.iter().any(|value| value["cancelReceived"] == true)
    })
    .await;
    #[cfg(windows)]
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if processes.iter().all(ProcessHandle::exited) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("ACP descendant survived cancellation");
    #[cfg(not(windows))]
    let _ = child;
}

#[tokio::test]
async fn pending_permission_is_cancelled_when_turn_is_dropped() {
    let folder = tempfile::tempdir().unwrap();
    let permissions = Arc::new(Permissions {
        hang: true,
        ..Default::default()
    });
    let agent = AcpAgent::new(
        config(folder.path(), "permission-hang"),
        permissions.clone(),
    );
    let (sink, _) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move { agent.run_turn(request(), sink).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while permissions.seen.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
    let values = until(folder.path(), |values| {
        values.iter().any(|value| value["id"] == "permission-1")
    })
    .await;
    let response = values
        .iter()
        .find(|value| value["id"] == "permission-1")
        .unwrap();
    assert_eq!(response["result"]["outcome"]["outcome"], "cancelled");
    until(folder.path(), |values| {
        values.iter().any(|value| value["cancelReceived"] == true)
    })
    .await;
    // Give the cleanup worker time to release the fixture's trace file before TempDir drops.
    tokio::time::sleep(Duration::from_millis(200)).await;
}

#[tokio::test]
async fn startup_timeout_is_bounded() {
    let folder = tempfile::tempdir().unwrap();
    let mut config = config(folder.path(), "startup-hang");
    config.startup_timeout = Duration::from_millis(100);
    let agent = AcpAgent::new(config, Arc::new(Permissions::default()));
    let (sink, _) = mpsc::unbounded_channel();
    let error = agent
        .run_turn(request(), sink)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("setup timed out"), "{error}");
}

#[tokio::test]
async fn core_journal_resumes_acp_without_storing_deltas_or_duplicate_replay() {
    let folder = tempfile::tempdir().unwrap();
    let store = vatra_core::Store::open_in_memory().unwrap();
    let project = store.create_project("ACP fixture").unwrap();
    let session = store.create_session(project, "external").unwrap();
    let agent = AcpAgent::new(
        config(folder.path(), "normal"),
        Arc::new(Permissions::default()),
    );
    for prompt in ["First turn", "Second turn"] {
        vatra_core::run_turn(&store, &agent, session, prompt, &mut |_| {})
            .await
            .unwrap();
    }
    assert_eq!(
        store.native_session(session).unwrap().as_deref(),
        Some("acp-session-1")
    );
    let events = store.events(session).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, Event::AssistantText { .. }))
            .count(),
        2
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.event, Event::AssistantDelta { .. }))
    );
    assert_eq!(
        trace(folder.path())
            .iter()
            .filter(|message| message["method"] == "session/load")
            .count(),
        1
    );
}

#[tokio::test]
async fn turn_timeout_cancels_a_stalled_agent() {
    let folder = tempfile::tempdir().unwrap();
    let mut config = config(folder.path(), "hang");
    config.turn_timeout = Duration::from_secs(2);
    let agent = AcpAgent::new(config, Arc::new(Permissions::default()));
    let (sink, _) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move { agent.run_turn(request(), sink).await });
    // Open the handles while the descendant is alive; the timeout is what ends it.
    let values = until(folder.path(), |values| {
        values.iter().any(|value| value["childPid"].is_u64())
    })
    .await;
    let child = values
        .iter()
        .find_map(|value| value["childPid"].as_u64())
        .unwrap();
    #[cfg(windows)]
    let processes = [
        ProcessHandle::open(child),
        ProcessHandle::open(
            values
                .iter()
                .find_map(|value| value["parentPid"].as_u64())
                .unwrap(),
        ),
    ];
    let error = task.await.unwrap().unwrap_err().to_string();
    assert!(error.contains("turn timed out"), "{error}");
    until(folder.path(), |values| {
        values.iter().any(|value| value["cancelReceived"] == true)
    })
    .await;
    #[cfg(windows)]
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if processes.iter().all(ProcessHandle::exited) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("ACP process tree survived the turn timeout");
    #[cfg(not(windows))]
    let _ = child;
}

#[tokio::test]
async fn waiting_for_approval_doesnt_use_turn_time() {
    let folder = tempfile::tempdir().unwrap();
    let mut config = config(folder.path(), "permission");
    config.turn_timeout = Duration::from_millis(500);
    let permissions = Arc::new(Permissions {
        allow: true,
        delay: Duration::from_millis(1500),
        ..Default::default()
    });
    let agent = AcpAgent::new(config, permissions.clone());
    let (sink, _) = mpsc::unbounded_channel();
    agent.run_turn(request(), sink).await.unwrap();
    assert_eq!(permissions.seen.lock().unwrap().len(), 1);
    let response = trace(folder.path())
        .into_iter()
        .find(|item| item["id"] == "permission-1")
        .unwrap();
    assert_eq!(response["result"]["outcome"]["optionId"], "yes");
}

#[tokio::test]
async fn partial_text_is_kept_on_timeout() {
    let folder = tempfile::tempdir().unwrap();
    let mut config = config(folder.path(), "partial-hang");
    config.turn_timeout = Duration::from_millis(500);
    let agent = AcpAgent::new(config, Arc::new(Permissions::default()));
    let (sink, mut events) = mpsc::unbounded_channel();
    let error = agent
        .run_turn(request(), sink)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("turn timed out"), "{error}");
    let mut collected = vec![];
    while let Some(event) = events.recv().await {
        collected.push(event);
    }
    assert!(
        collected.contains(&Event::AssistantText {
            text: "Partial ".into()
        }),
        "{collected:?}"
    );
}

#[tokio::test]
async fn partial_text_is_kept_when_the_turn_is_dropped() {
    let folder = tempfile::tempdir().unwrap();
    let agent = AcpAgent::new(
        config(folder.path(), "partial-hang"),
        Arc::new(Permissions::default()),
    );
    let (sink, mut events) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move { agent.run_turn(request(), sink).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            if matches!(event, Event::AssistantDelta { .. }) {
                return;
            }
        }
    })
    .await
    .expect("ACP fixture did not stream text");
    task.abort();
    let _ = task.await;
    let mut rest = vec![];
    while let Some(event) = events.recv().await {
        rest.push(event);
    }
    assert!(
        rest.contains(&Event::AssistantText {
            text: "Partial ".into()
        }),
        "{rest:?}"
    );
}

#[tokio::test]
async fn reply_of_exactly_the_frame_limit_is_accepted() {
    let (result, _, _) = run("exact-frame", request()).await;
    assert_eq!(
        result.unwrap().native_session.as_deref(),
        Some("acp-session-1")
    );
}

#[tokio::test]
async fn non_utf8_working_folder_is_refused_before_spawning() {
    let folder = tempfile::tempdir().unwrap();
    let mut config = config(folder.path(), "normal");
    config.cwd = non_utf8_folder();
    let agent = AcpAgent::new(config, Arc::new(Permissions::default()));
    let (sink, _) = mpsc::unbounded_channel();
    let error = agent
        .run_turn(request(), sink)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("UTF-8"), "{error}");
}

#[cfg(windows)]
fn non_utf8_folder() -> std::path::PathBuf {
    use std::os::windows::ffi::OsStringExt;
    let mut path = std::ffi::OsString::from("C:\\");
    path.push(std::ffi::OsString::from_wide(&[0xD800]));
    path.into()
}

#[cfg(unix)]
fn non_utf8_folder() -> std::path::PathBuf {
    use std::os::unix::ffi::OsStrExt;
    std::path::Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff")).to_path_buf()
}

#[tokio::test]
async fn chosen_model_and_effort_are_set_before_the_prompt() {
    let folder = tempfile::tempdir().unwrap();
    let mut config = config(folder.path(), "picker");
    config.model = Some("large".into());
    config.effort = Some("high".into());
    AcpAgent::new(config, Arc::new(Permissions::default()))
        .run_turn(request(), mpsc::unbounded_channel().0).await.unwrap();
    let messages = trace(folder.path());
    assert_eq!(messages[2]["method"], "session/set_config_option");
    assert_eq!(messages[2]["params"]["value"], "large");
    assert_eq!(messages[3]["params"]["configId"], "thinking");
    assert_eq!(messages[3]["params"]["value"], "high");
    assert_eq!(messages[4]["method"], "session/prompt");
}

#[tokio::test]
async fn discovery_reads_choices_without_sending_a_prompt() {
    let folder = tempfile::tempdir().unwrap();
    let options = vatra_acp::probe_options(&config(folder.path(), "picker")).await.unwrap();
    assert_eq!(options.models[0].id, "large");
    assert_eq!(options.efforts[0].id, "high");
    assert_eq!(trace(folder.path()).len(), 2);
}

#[tokio::test]
async fn rejected_model_does_not_silently_run_a_different_one() {
    let folder = tempfile::tempdir().unwrap();
    let mut config = config(folder.path(), "reject-choice");
    config.model = Some("large".into());
    let result = AcpAgent::new(config, Arc::new(Permissions::default()))
        .run_turn(request(), mpsc::unbounded_channel().0).await;
    assert!(result.is_err());
    assert!(!trace(folder.path()).iter().any(|item| item["method"] == "session/prompt"));
}
