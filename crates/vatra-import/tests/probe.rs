use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use vatra_core::{McpServerConfig, SecretRef};
use vatra_extensions::{Origin, RunsCode, Scope, Verdict};
use vatra_import::{Found, FoundKind, Probe, ProbeResult, SystemProbe, build_report, needed_programs, probe_items};

/// Answers every check without starting anything: `python` is missing, a stdio server waits for approval.
struct FakeProbe;

#[async_trait]
impl Probe for FakeProbe {
    async fn runs(&self, program: &str, _args: &[&str]) -> ProbeResult {
        if program == "python" {
            ProbeResult::Missing("python was not found".into())
        } else {
            ProbeResult::Ok
        }
    }

    async fn on_path(&self, _program: &str) -> ProbeResult {
        ProbeResult::Ok
    }

    async fn mcp_starts(&self, server: &McpServerConfig) -> ProbeResult {
        if server.command.is_some() {
            ProbeResult::NotChecked("not checked until you approve it".into())
        } else {
            ProbeResult::Ok
        }
    }
}

#[tokio::test]
async fn probe_items_flow_into_report() {
    let server = McpServerConfig {
        name: "scripts".into(),
        command: Some("python".into()),
        args: vec!["server.py".into()],
        ..Default::default()
    };
    let mut found = item_running(&["python server.py"]);
    found.kind = FoundKind::McpServer;
    found.name = "scripts".into();
    found.source = PathBuf::from("scripts");
    found.details = serde_json::to_value(&server).unwrap();

    let mut probes = probe_items(&FakeProbe, std::slice::from_ref(&found)).await;
    assert_eq!(
        probes.programs["python"],
        ProbeResult::Missing("python was not found".into())
    );

    // Before approval the server waits for it, whatever else is missing.
    let waiting = build_report(std::slice::from_ref(&found), &probes);
    assert_eq!(
        waiting.items[0].routes[0].1,
        Verdict::NeedsApproval("not checked until you approve it".into())
    );

    // After approval the missing program is what the report says.
    probes.approved.insert((PathBuf::from("scripts"), "scripts".into()));
    let approved = build_report(std::slice::from_ref(&found), &probes);
    for (_, verdict) in &approved.items[0].routes {
        assert_eq!(*verdict, Verdict::NeedsInput("python was not found".into()));
    }
}

#[test]
fn needed_programs_skip_assignments_and_split_chains() {
    let found = item_running(&["NODE_ENV=prod node app.js && uvx tool; bun run x | python y.py"]);

    let programs: Vec<String> = needed_programs(&found)
        .into_iter()
        .map(|program| program.name)
        .collect();

    assert_eq!(programs, vec!["node", "uvx", "bun", "python"]);
}

#[test]
fn needed_programs_keep_redirections_together() {
    let found = item_running(&["cmd 2>&1 | findstr x && exit 0"]);

    let programs: Vec<String> = needed_programs(&found)
        .into_iter()
        .map(|program| program.name)
        .collect();

    assert_eq!(programs, vec!["cmd", "findstr"]);
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

fn item_running(commands: &[&str]) -> Found {
    Found {
        kind: FoundKind::Skill,
        name: "tools".into(),
        origin: Origin::Claude,
        scope: Scope::Global,
        project: None,
        source: PathBuf::from("tools"),
        hash: "hash".into(),
        runs_code: commands
            .iter()
            .map(|command| RunsCode {
                what: "inline command".into(),
                command: command.to_string(),
            })
            .collect(),
        secrets: Vec::new(),
        unsupported: None,
        details: Value::Null,
        modified: None,
        also_from: Vec::new(),
    }
}

#[tokio::test]
async fn broken_python_is_reported_missing() {
    let probe = SystemProbe::new();

    let not_found = probe.runs("vatra-no-such-program-xyz", &["--version"]).await;
    assert!(matches!(not_found, ProbeResult::Missing(_)), "{not_found:?}");

    let exits_with_one = if cfg!(windows) {
        probe.runs("cmd", &["/C", "exit", "1"]).await
    } else {
        probe.runs("sh", &["-c", "exit 1"]).await
    };
    assert!(matches!(exits_with_one, ProbeResult::Missing(_)), "{exits_with_one:?}");
}

#[tokio::test]
async fn found_program_ok() {
    let probe = SystemProbe::new();

    let result = probe.runs(&cargo(), &["--version"]).await;

    assert_eq!(result, ProbeResult::Ok);
}

#[tokio::test]
async fn hanging_program_times_out() {
    let probe = SystemProbe::with_timeout(Duration::from_millis(300));
    let started = Instant::now();

    let result = if cfg!(windows) {
        probe.runs("cmd", &["/C", "ping", "-n", "30", "127.0.0.1"]).await
    } else {
        probe.runs("sleep", &["30"]).await
    };

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the check waited for the program"
    );
    match result {
        ProbeResult::Missing(message) => assert!(message.contains("did not answer in time"), "{message}"),
        other => panic!("expected a timeout, got {other:?}"),
    }
}

#[tokio::test]
async fn mcp_needs_value_when_secret_missing() {
    let probe = SystemProbe::new();
    let remote = McpServerConfig {
        name: "docs".into(),
        url: Some("https://docs.example.test/mcp".into()),
        headers: [(
            "Authorization".to_string(),
            "Bearer ${VATRA_IMPORT_TEST_UNSET_TOKEN}".to_string(),
        )]
        .into(),
        ..Default::default()
    };
    let local = McpServerConfig {
        name: "local".into(),
        command: Some("vatra-no-such-program-xyz".into()),
        secrets: [("KEY".to_string(), SecretRef::Env("VATRA_IMPORT_TEST_UNSET_KEY".into()))].into(),
        ..Default::default()
    };

    let remote_result = probe.mcp_starts(&remote).await;
    let local_result = probe.mcp_starts(&local).await;

    assert!(
        matches!(&remote_result, ProbeResult::NeedsValue(message) if message.contains("VATRA_IMPORT_TEST_UNSET_TOKEN"))
    );
    assert!(
        matches!(&local_result, ProbeResult::NeedsValue(message) if message.contains("VATRA_IMPORT_TEST_UNSET_KEY"))
    );
}

#[tokio::test]
async fn stdio_server_not_started_until_approved() {
    let probe = SystemProbe::new();
    let server = McpServerConfig {
        name: "local".into(),
        command: Some(cargo()),
        args: vec!["--version".into()],
        ..Default::default()
    };

    let result = probe.mcp_starts(&server).await;

    assert_eq!(
        result,
        ProbeResult::NotChecked("not checked until you approve it".into())
    );
}

#[test]
fn needed_programs_from_commands_and_scripts() {
    let found = item_running(&[
        "uvx local-tool",
        "node server.js",
        "python scripts/check.py --fast",
        "echo done",
        "\"./hooks/run.sh\"",
        "node server.js",
    ]);

    let programs: Vec<String> = needed_programs(&found)
        .into_iter()
        .map(|program| program.name)
        .collect();

    assert_eq!(programs, vec!["uvx", "node", "python", "bash"]);
}
