use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::json;
use vatra_core::Event;
use vatra_parity::{Check, Task, evaluate, evaluate_with_timeout, metrics};

fn task(checks: Vec<Check>) -> Task {
    Task {
        id: "t".into(),
        title: "t".into(),
        prompt: "t".into(),
        fixture: "t".into(),
        allowed_commands: vec![],
        needs_mcp: false,
        skill: None,
        checks,
    }
}

fn call(name: &str) -> Event {
    Event::ToolCall { id: "1".into(), name: name.into(), arguments: json!({}) }
}

async fn tool_passes(any_of: &[&str], transcript: &[Event]) -> bool {
    let dir = Path::new(".");
    let check = Check::ToolUsed { any_of: any_of.iter().map(|s| s.to_string()).collect() };
    evaluate(&task(vec![check]), dir, dir, transcript).await.passed
}

#[tokio::test]
async fn tool_used_matches_mcp_names() {
    let transcript = [call("mcp__demo__echo")];
    assert!(tool_passes(&["echo"], &transcript).await);
    assert!(!tool_passes(&["echo_text"], &[call("mcp__demo__echo_text_x")]).await);
    assert!(!tool_passes(&["echo"], &[call("echoes")]).await);
}

#[tokio::test]
async fn tool_used_inside_nested() {
    let nested = Event::Nested {
        parent_id: "p".into(),
        event: Box::new(call("Grep")),
    };
    assert!(tool_passes(&["Read", "Grep"], &[nested]).await);
    assert!(!tool_passes(&["Bash"], &[call("Grep")]).await);
}

#[tokio::test]
async fn command_passes_runs_in_workdir_with_timeout() {
    let fixture = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("marker.js"), "process.exit(0)").unwrap();

    // Passes only if the command runs in the workdir, where marker.js lives.
    let ran_in_workdir = task(vec![Check::CommandPasses { command: "node marker.js".into() }]);
    assert!(evaluate(&ran_in_workdir, fixture.path(), work.path(), &[]).await.passed);

    let hangs = task(vec![Check::CommandPasses {
        command: "node -e \"setInterval(()=>{},1000)\"".into(),
    }]);
    let started = Instant::now();
    let outcome = evaluate_with_timeout(
        &hangs,
        fixture.path(),
        work.path(),
        &[],
        Duration::from_secs(1),
    )
    .await;
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(!outcome.passed);
    assert!(
        outcome.failures[0].contains("didn't finish in 1 seconds"),
        "{:?}",
        outcome.failures
    );
}

#[test]
fn metrics_from_events() {
    let transcript = vec![
        call("Bash"),
        Event::ToolResult { id: "1".into(), output: "ok".into(), is_error: false },
        Event::ToolResult { id: "1".into(), output: "boom".into(), is_error: true },
        Event::Nested {
            parent_id: "p".into(),
            event: Box::new(Event::Nested {
                parent_id: "q".into(),
                event: Box::new(call("Read")),
            }),
        },
        Event::Nested {
            parent_id: "p".into(),
            event: Box::new(Event::ToolResult { id: "2".into(), output: "x".into(), is_error: true }),
        },
        Event::Usage { input_tokens: 100, output_tokens: 20, cost_usd: Some(0.25) },
        Event::Usage { input_tokens: 50, output_tokens: 5, cost_usd: Some(0.5) },
    ];
    let m = metrics(&transcript, 2.5);
    assert_eq!(m.tool_calls, 2);
    assert_eq!(m.tool_errors, 2);
    assert_eq!(m.seconds, 2.5);
    assert_eq!(m.input_tokens, 150);
    assert_eq!(m.output_tokens, 25);
    assert_eq!(m.cost_usd, Some(0.75));
}

#[test]
fn metrics_cost_is_none_without_any_cost() {
    let transcript = vec![
        Event::Usage { input_tokens: 1, output_tokens: 2, cost_usd: None },
        Event::Usage { input_tokens: 3, output_tokens: 4, cost_usd: None },
    ];
    let m = metrics(&transcript, 0.0);
    assert_eq!(m.input_tokens, 4);
    assert_eq!(m.output_tokens, 6);
    assert_eq!(m.cost_usd, None);
}
