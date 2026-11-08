use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tokio::sync::mpsc;
use vatra_claude::{ClaudeAgent, ClaudeApprovals, ClaudeConfig, claude_args, find_on_path};
use vatra_core::{Agent, AgentError, Event, McpServerConfig, TurnOutcome, TurnRequest};

struct FakeRun {
    outcome: Result<TurnOutcome, AgentError>,
    events: Vec<Event>,
    recorded_stdin: String,
    recorded_args: Vec<String>,
}

fn demo_server() -> McpServerConfig {
    McpServerConfig {
        name: "demo".into(),
        command: Some("demo-mcp".into()),
        ..Default::default()
    }
}

fn cfg(allow: bool) -> ClaudeConfig {
    ClaudeConfig {
        binary: PathBuf::from("claude"),
        cwd: None,
        servers: vec![demo_server()],
        approvals: if allow {
            ClaudeApprovals::Catalog
        } else {
            ClaudeApprovals::Deny
        },
        plugin_dir: None,
        append_system_prompt: None,
        removed_env: Vec::new(),
        model: None,
        effort: None,
    }
}

#[test]
fn generated_arguments_and_original_approval_bridge_coexist() {
    let mut config = cfg_with(ClaudeApprovals::Prompt {
        server: Box::new(McpServerConfig {
            name: "vatra".into(),
            command: Some("vatra".into()),
            ..Default::default()
        }),
        tool: "mcp__vatra__approve".into(),
        accept_edits: false,
    });
    config.plugin_dir = Some(PathBuf::from("C:/Răzvan Test/plugin"));
    config.append_system_prompt = Some("Vatra memory".into());
    let args = claude_args(&config, None);
    assert!(
        args.windows(2)
            .any(|w| w == ["--plugin-dir", "C:/Răzvan Test/plugin"])
    );
    assert!(
        args.windows(2)
            .any(|w| w == ["--append-system-prompt", "Vatra memory"])
    );
    assert!(
        args.windows(2)
            .any(|w| w == ["--permission-prompt-tool", "mcp__vatra__approve"])
    );
    let config: Value =
        serde_json::from_str(&args[args.iter().position(|v| v == "--mcp-config").unwrap() + 1])
            .unwrap();
    assert!(config["mcpServers"]["vatra"].is_object());
    assert!(!args.iter().any(|a| matches!(
        a.as_str(),
        "--agents" | "--strict-mcp-config" | "--bare" | "--setting-sources"
    )));
}

#[test]
fn plugin_mcp_servers_keep_canonical_names() {
    let mut config = cfg(true);
    config.servers[0].plugin = Some("tools".into());
    let args = claude_args(&config, None);
    let value: Value = serde_json::from_str(
        &args[args
            .iter()
            .position(|value| value == "--mcp-config")
            .unwrap()
            + 1],
    )
    .unwrap();
    assert!(value["mcpServers"]["plugin_tools_demo"].is_object());
    assert!(args.contains(&"mcp__plugin_tools_demo".to_string()));
}

#[tokio::test]
async fn configured_api_key_environment_is_removed_from_child() {
    let dir = tempfile::tempdir().unwrap();
    let binary = install_fake(dir.path(), "stream.jsonl", 0);
    // The parent definitely has PATH, so this proves env_remove without mutating process-global state.
    let agent = ClaudeAgent::new(ClaudeConfig {
        binary,
        removed_env: vec!["PATH".into()],
        ..cfg(true)
    });
    let (sink, _received) = mpsc::unbounded_channel();
    agent
        .run_turn(
            TurnRequest { files: vec![],
                history: vec![],
                prompt: "test".into(),
                native_session: None,
            },
            sink,
        )
        .await
        .unwrap();
    let record: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("fake-record.json")).unwrap(),
    )
    .unwrap();
    assert!(record["env"]["PATH"].is_null());
}

fn fixture_path(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .display()
        .to_string()
}

fn install_fake(directory: &Path, stream: &str, exit: i32) -> PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let binary = directory.join("fake-claude.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_fake-claude"), &binary).unwrap();
    let config = json!({ "stream": fixture_path(stream), "exit": exit });
    std::fs::write(directory.join("fake-config.json"), config.to_string()).unwrap();
    binary
}

async fn execute(
    binary: PathBuf,
    prompt: &str,
    native: Option<&str>,
) -> (Result<TurnOutcome, AgentError>, Vec<Event>) {
    let agent = ClaudeAgent::new(ClaudeConfig {
        binary,
        ..cfg(true)
    });
    let (sink, mut received) = mpsc::unbounded_channel();
    let request = TurnRequest { files: vec![],
        history: vec![],
        prompt: prompt.into(),
        native_session: native.map(String::from),
    };
    let outcome = agent.run_turn(request, sink).await;
    let mut events = vec![];
    while let Some(event) = received.recv().await {
        events.push(event);
    }
    (outcome, events)
}

async fn run_in(
    directory: &Path,
    stream: &str,
    exit: i32,
    prompt: &str,
    native: Option<&str>,
) -> FakeRun {
    let binary = install_fake(directory, stream, exit);
    let (outcome, events) = execute(binary, prompt, native).await;
    let record: Value =
        serde_json::from_str(&std::fs::read_to_string(directory.join("fake-record.json")).unwrap())
            .unwrap();
    FakeRun {
        outcome,
        events,
        recorded_stdin: record["stdin"].as_str().unwrap().to_string(),
        recorded_args: record["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap().to_string())
            .collect(),
    }
}

#[tokio::test]
async fn prompt_goes_through_stdin_intact_and_session_id_returns() {
    let dir = tempfile::tempdir().unwrap();
    let long = format!("ășțîâ {}", "x".repeat(200_000));
    let run = run_in(dir.path(), "stream.jsonl", 0, &long, None).await;
    assert_eq!(
        run.outcome.unwrap().native_session.as_deref(),
        Some("sess-1")
    );
    assert_eq!(run.recorded_stdin, long);
    assert!(!run.recorded_args.contains(&long));
}

#[tokio::test]
async fn mcp_config_and_resume_are_passed_and_no_auth_is_touched() {
    let dir = tempfile::tempdir().unwrap();
    let run = run_in(dir.path(), "stream.jsonl", 0, "x", Some("sess-0")).await;
    assert!(
        run.recorded_args
            .windows(2)
            .any(|w| w == ["--resume", "sess-0"])
    );
    assert!(run.recorded_args.iter().any(|a| a.contains("mcpServers")));
    assert!(
        !run.recorded_args
            .iter()
            .any(|a| a.to_lowercase().contains("token"))
    );
}

#[test]
fn subagent_text_is_requested_in_the_stream() {
    assert!(claude_args(&cfg(true), None).contains(&"--forward-subagent-text".to_string()));
}

#[test]
fn approval_flags_follow_the_setting() {
    assert!(claude_args(&cfg(true), None).contains(&"--allowedTools".to_string()));
    assert!(claude_args(&cfg(true), None).contains(&"mcp__demo".to_string()));
    assert!(
        claude_args(&cfg(false), None)
            .windows(2)
            .any(|w| w == ["--permission-prompts", "none"])
    );
}

#[tokio::test]
async fn missing_binary_is_not_installed_error() {
    let (outcome, _) = execute(PathBuf::from("nu-exista-claude"), "x", None).await;
    assert!(matches!(outcome, Err(AgentError::NotInstalled(_))));
}

#[tokio::test]
async fn nonzero_exit_without_result_fails_but_keeps_stream_events() {
    let dir = tempfile::tempdir().unwrap();
    let run = run_in(dir.path(), "assistant_only.jsonl", 3, "x", None).await;
    assert!(
        matches!(&run.outcome, Err(AgentError::Failed(message)) if message.contains("mesaj de pe stderr"))
    );
    assert!(
        run.events
            .iter()
            .any(|e| matches!(e, Event::AssistantText { .. }))
    );
}

#[tokio::test]
async fn clean_exit_without_result_is_still_a_failed_turn() {
    let dir = tempfile::tempdir().unwrap();
    let run = run_in(dir.path(), "assistant_only.jsonl", 0, "x", None).await;
    assert!(matches!(run.outcome, Err(AgentError::Failed(_))));
}

async fn run_request(directory: &Path, stream: &str, exit: i32, request: TurnRequest) -> FakeRun {
    let binary = install_fake(directory, stream, exit);
    let agent = ClaudeAgent::new(ClaudeConfig {
        binary,
        ..cfg(true)
    });
    let (sink, mut received) = mpsc::unbounded_channel();
    let outcome = agent.run_turn(request, sink).await;
    let mut events = vec![];
    while let Some(event) = received.recv().await {
        events.push(event);
    }
    let record: Value =
        serde_json::from_str(&std::fs::read_to_string(directory.join("fake-record.json")).unwrap())
            .unwrap();
    FakeRun {
        outcome,
        events,
        recorded_stdin: record["stdin"].as_str().unwrap().to_string(),
        recorded_args: vec![],
    }
}

#[tokio::test]
async fn failed_turn_still_reports_the_native_session() {
    let dir = tempfile::tempdir().unwrap();
    let run = run_in(dir.path(), "assistant_only.jsonl", 3, "x", None).await;
    assert!(run.events.contains(&Event::NativeSession {
        id: "sess-2".into()
    }));
}

#[tokio::test]
async fn history_without_a_native_session_is_rendered_into_the_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let request = TurnRequest { files: vec![],
        history: vec![Event::UserMessage {
            text: "cerere veche".into(),
        }],
        prompt: "nou".into(),
        native_session: None,
    };
    let run = run_request(dir.path(), "stream.jsonl", 0, request).await;
    assert!(run.recorded_stdin.contains("cerere veche"));
    assert!(run.recorded_stdin.ends_with("nou"));
}

#[tokio::test]
async fn pending_handoff_result_reaches_a_resumed_session() {
    let dir = tempfile::tempdir().unwrap();
    let request = TurnRequest { files: vec![],
        history: vec![
            Event::HandoffResult {
                to_session: 1,
                text: "rezultat vechi".into(),
            },
            Event::TurnCompleted {
                agent: "claude".into(),
            },
            Event::HandoffResult {
                to_session: 2,
                text: "rezultat nou".into(),
            },
        ],
        prompt: "continuă".into(),
        native_session: Some("sess-0".into()),
    };
    let run = run_request(dir.path(), "stream.jsonl", 0, request).await;
    assert!(run.recorded_stdin.contains("rezultat nou"));
    assert!(!run.recorded_stdin.contains("rezultat vechi"));
    assert!(run.recorded_stdin.ends_with("continuă"));
}

#[tokio::test]
async fn non_utf8_stdout_line_becomes_unmapped_and_later_events_survive() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = b"\xff\xfe nu e utf8\n".to_vec();
    bytes.extend(std::fs::read(fixture_path("stream.jsonl")).unwrap());
    let stream = dir.path().join("flux.bin");
    std::fs::write(&stream, bytes).unwrap();
    let run = run_in(
        &dir.path().join("bin"),
        stream.to_str().unwrap(),
        0,
        "x",
        None,
    )
    .await;
    assert!(run.outcome.is_ok());
    assert!(
        run.events
            .iter()
            .any(|e| matches!(e, Event::Unmapped { raw, .. } if raw.contains("nu e utf8")))
    );
    assert!(run.events.contains(&Event::AssistantText {
        text: "gata".into()
    }));
}

#[tokio::test]
async fn non_utf8_stdout_followed_by_large_output_does_not_hang() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = b"\xff\xfe\n".to_vec();
    for _ in 0..40_000 {
        bytes.extend_from_slice(
            format!("{{\"type\":\"nou\",\"x\":\"{}\"}}\n", "y".repeat(100)).as_bytes(),
        );
    }
    bytes.extend_from_slice(b"{\"type\":\"result\",\"is_error\":false}\n");
    let stream = dir.path().join("flux.bin");
    std::fs::write(&stream, bytes).unwrap();
    let directory = dir.path().join("bin");
    let run = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        run_in(&directory, stream.to_str().unwrap(), 0, "x", None),
    )
    .await
    .expect("adaptorul s-a blocat");
    assert!(run.outcome.is_ok());
}

#[tokio::test]
async fn non_utf8_stderr_keeps_the_error_message() {
    let dir = tempfile::tempdir().unwrap();
    let directory = dir.path().join("bin");
    let binary = install_fake(&directory, "assistant_only.jsonl", 2);
    let stderr = dir.path().join("stderr.bin");
    std::fs::write(&stderr, b"mesaj \xe3 eroare").unwrap();
    let config = json!({
        "stream": fixture_path("assistant_only.jsonl"),
        "exit": 2,
        "stderr_file": stderr.display().to_string(),
    });
    std::fs::write(directory.join("fake-config.json"), config.to_string()).unwrap();
    let (outcome, _) = execute(binary, "x", None).await;
    assert!(matches!(outcome, Err(AgentError::Failed(message)) if message.contains("eroare")));
}

#[test]
fn a_cmd_shim_is_found_on_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("claude.cmd"), "@echo off\r\n").unwrap();
    let found = find_on_path(Path::new("claude"), dir.path().as_os_str()).unwrap();
    assert_eq!(found, dir.path().join("claude.cmd"));
}

#[test]
fn an_exe_wins_over_a_cmd_shim_and_paths_are_not_searched() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("claude.cmd"), "").unwrap();
    std::fs::write(dir.path().join("claude.exe"), "").unwrap();
    let found = find_on_path(Path::new("claude"), dir.path().as_os_str()).unwrap();
    assert_eq!(found, dir.path().join("claude.exe"));
    assert_eq!(
        find_on_path(&dir.path().join("claude"), dir.path().as_os_str()),
        None
    );
}

#[cfg(windows)]
#[tokio::test]
async fn a_cmd_shim_passes_arguments_and_stdin_intact() {
    let dir = tempfile::tempdir().unwrap();
    let directory = dir.path().join("dir cu spații");
    install_fake(&directory, "stream.jsonl", 0);
    let shim = directory.join("claude.cmd");
    // The form npm shims use: batch files aren't read as UTF-8, so the path comes from %~dp0.
    std::fs::write(&shim, "@\"%~dp0fake-claude.exe\" %*\r\n").unwrap();
    let (outcome, _) = execute(shim, "ășț", None).await;
    assert!(outcome.is_ok(), "{outcome:?}");
    let record: Value =
        serde_json::from_str(&std::fs::read_to_string(directory.join("fake-record.json")).unwrap())
            .unwrap();
    assert!(record["args"].as_array().unwrap().iter().any(|arg| {
        serde_json::from_str::<Value>(arg.as_str().unwrap())
            .is_ok_and(|value| value["mcpServers"].is_object())
    }));
}

#[tokio::test]
async fn binary_path_with_spaces_works() {
    let dir = tempfile::tempdir().unwrap();
    let run = run_in(
        &dir.path().join("dir cu spații"),
        "stream.jsonl",
        0,
        "x",
        None,
    )
    .await;
    assert!(run.outcome.is_ok());
}

fn bridge() -> McpServerConfig {
    McpServerConfig {
        name: "vatra".into(),
        command: Some("vatra.exe".into()),
        args: vec![
            "approval-bridge".into(),
            "--token-file".into(),
            "token".into(),
        ],
        ..Default::default()
    }
}

fn cfg_with(approvals: ClaudeApprovals) -> ClaudeConfig {
    ClaudeConfig {
        approvals,
        ..cfg(true)
    }
}

fn prompt_mode(accept_edits: bool) -> ClaudeApprovals {
    ClaudeApprovals::Prompt {
        server: Box::new(bridge()),
        tool: "mcp__vatra__approve".into(),
        accept_edits,
    }
}

#[test]
fn prompt_mode_passes_the_bridge_and_the_permission_prompt_tool() {
    let args = claude_args(&cfg_with(prompt_mode(false)), None);
    assert!(
        args.windows(2)
            .any(|w| w == ["--permission-prompt-tool", "mcp__vatra__approve"])
    );
    let config = args.iter().find(|a| a.contains("mcpServers")).unwrap();
    assert!(config.contains("\"vatra\"") && config.contains("\"demo\""));
    assert!(!args.contains(&"--allowedTools".to_string()));
    assert!(!args.contains(&"--permission-mode".to_string()));
}

#[test]
fn accept_edits_uses_the_native_mode_on_top_of_prompts() {
    let args = claude_args(&cfg_with(prompt_mode(true)), None);
    assert!(
        args.windows(2)
            .any(|w| w == ["--permission-mode", "acceptEdits"])
    );
    assert!(args.contains(&"--permission-prompt-tool".to_string()));
}

#[test]
fn the_token_itself_never_appears_in_the_arguments() {
    let args = claude_args(&cfg_with(prompt_mode(false)), None);
    assert!(!args.iter().any(|a| a.contains("secret-token")));
}

#[test]
fn full_access_uses_the_native_permission_mode() {
    let args = claude_args(&cfg_with(ClaudeApprovals::FullAccess), None);
    assert!(
        args.windows(2)
            .any(|w| w == ["--permission-mode", "bypassPermissions"])
    );
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

#[cfg(windows)]
#[tokio::test]
async fn turn_runs_without_a_console_window() {
    let directory = tempfile::tempdir().unwrap();
    let run = run_in(directory.path(), "stream.jsonl", 0, "background prompt", None).await;
    assert!(run.outcome.is_ok());
    assert_eq!(run.recorded_stdin, "background prompt");
    let record: Value = serde_json::from_str(
        &std::fs::read_to_string(directory.path().join("fake-record.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["has_console_window"], false);
    assert!(run.events.contains(&Event::AssistantText {
        text: "gata".into(),
    }));
}

#[cfg(windows)]
#[tokio::test]
async fn stopping_a_turn_kills_the_whole_process_tree() {
    let dir = tempfile::tempdir().unwrap();
    let binary = install_fake(dir.path(), "stream.jsonl", 0);
    let config = json!({ "stream": fixture_path("stream.jsonl"), "exit": 0, "spawn_grandchild_and_hang": true });
    std::fs::write(dir.path().join("fake-config.json"), config.to_string()).unwrap();

    let agent = ClaudeAgent::new(ClaudeConfig {
        binary,
        ..cfg(true)
    });
    let (sink, _received) = mpsc::unbounded_channel();
    let request = TurnRequest { files: vec![],
        history: vec![],
        prompt: "x".into(),
        native_session: None,
    };
    let mut turn = Box::pin(agent.run_turn(request, sink));

    let pid_file = dir.path().join("grandchild.pid");
    let mut pid = None;
    for _ in 0..200 {
        tokio::select! {
            _ = &mut turn => panic!("tura trebuia să ruleze"),
            () = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
        }
        if let Ok(text) = std::fs::read_to_string(&pid_file) {
            pid = text.trim().parse::<u32>().ok();
            if pid.is_some() {
                break;
            }
        }
    }
    let pid = pid.expect("nepotul n-a pornit");
    assert!(process_alive(pid));

    drop(turn);
    for _ in 0..100 {
        if !process_alive(pid) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("nepotul a supraviețuit opririi");
}

#[test]
fn model_and_effort_are_passed_as_separate_cli_arguments() {
    let mut config=cfg(true);
    config.model=Some("opus".into());config.effort=Some("high".into());
    let args=claude_args(&config,None);
    assert!(args.windows(2).any(|pair| pair == ["--model","opus"]));
    assert!(args.windows(2).any(|pair| pair == ["--effort","high"]));
}

#[tokio::test]
async fn model_discovery_only_sends_initialization_without_a_prompt() {
    let dir=tempfile::tempdir().unwrap();
    let binary=install_fake(dir.path(),"stream.jsonl",0);
    std::fs::write(dir.path().join("fake-config.json"),json!({"models":[{"value":"opus","displayName":"Opus","supportedEffortLevels":["low","high"]}]}).to_string()).unwrap();
    let options=vatra_claude::probe_models(&binary,&[]).await.unwrap();
    assert_eq!(options["models"][0]["id"],"opus");
    assert_eq!(options["models"][0]["efforts"][1]["id"],"high");
    let record:Value=serde_json::from_str(&std::fs::read_to_string(dir.path().join("fake-record.json")).unwrap()).unwrap();
    let request:Value=serde_json::from_str(record["stdin"].as_str().unwrap()).unwrap();
    assert_eq!(request["type"],"control_request");
    assert_eq!(request["request"]["subtype"],"initialize");
    assert!(request.get("message").is_none());
}

#[test]
fn tools_that_are_not_signed_in_are_one_notice_not_a_wall() {
    let init = json!({
        "type": "system", "subtype": "init", "session_id": "s",
        "mcp_servers": [
            {"name": "a", "status": "needs-auth"}, {"name": "b", "status": "failed"}, {"name": "a", "status": "failed"},
            {"name": "c", "status": "failed"}, {"name": "d", "status": "failed"}, {"name": "e", "status": "failed"},
            {"name": "ok", "status": "connected"},
        ],
    });
    let parsed = vatra_claude::parse_line(&init.to_string());
    let warnings: Vec<String> = parsed
        .iter()
        .filter_map(|item| match item {
            vatra_claude::Parsed::Event(Event::Warning { message, .. }) => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].starts_with("5 connected tools aren't available"), "{}", warnings[0]);
    assert!(warnings[0].contains("and 1 more"));
}
