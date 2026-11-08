use async_trait::async_trait;
use std::{path::Path, sync::Arc};
use vatra_core::{Agent, Store, Summarizer};
use vatra_service::parity_native::{Native, native_args, run_native};
use vatra_service::{AgentFactory, BuildRequest, Service};

struct Factory;
#[async_trait]
impl AgentFactory for Factory {
    fn names(&self) -> Vec<String> {
        vec![]
    }
    fn summarizer(&self) -> Arc<dyn Summarizer> {
        panic!("unused")
    }
    async fn build(&self, _: BuildRequest) -> Result<Arc<dyn Agent>, String> {
        panic!("unused")
    }
}
fn service() -> Arc<Service> {
    Service::new(
        Arc::new(Store::open_in_memory().unwrap()),
        Arc::new(Factory),
    )
    .unwrap()
}

#[tokio::test]
async fn native_requires_consent_before_spawning_even_an_invalid_binary() {
    let service = service();
    let error = run_native(
        &service,
        Native::ClaudeCode,
        Path::new("not-a-program"),
        "model",
        None,
        false,
    )
    .await
    .unwrap_err();
    assert!(error.contains("consent"));
    assert!(service.store().parity_runs(100).unwrap().is_empty());
}

#[test]
fn native_claude_uses_unmodified_binary_with_p_and_no_bypass() {
    let task = vatra_service::parity::tasks().unwrap().remove(0);
    let args = native_args(
        Native::ClaudeCode,
        "requested-model",
        &task,
        Path::new("fixture.json"),
    );
    let args: Vec<_> = args.iter().map(|arg| arg.to_str().unwrap()).collect();
    assert!(args.contains(&"-p"));
    assert!(
        args.windows(2)
            .any(|p| p == ["--output-format", "stream-json"])
    );
    assert!(args.windows(2).any(|p| p == ["--model", "requested-model"]));
    assert!(args.contains(&"--verbose"));
    assert!(args.contains(&"--strict-mcp-config"));
    assert!(
        args.windows(2)
            .any(|p| p == ["--settings", "guard-settings.json"])
    );
    let allowed = args.windows(2).find(|p| p[0] == "--allowedTools").unwrap()[1];
    assert!(
        !allowed
            .split(',')
            .any(|tool| matches!(tool, "Read" | "Write" | "Edit"))
    );
    assert!(allowed.contains("Edit(/**)"));
    assert!(!args.iter().any(|arg| arg.contains("dangerously")));
}

#[test]
fn native_codex_uses_exec_json_and_disables_shell() {
    let task = vatra_service::parity::tasks().unwrap().remove(0);
    let args = native_args(
        Native::Codex,
        "requested-model",
        &task,
        Path::new("fixture.json"),
    );
    let args: Vec<_> = args.iter().map(|arg| arg.to_str().unwrap()).collect();
    assert_eq!(args[0], "exec");
    assert!(args.contains(&"--json"));
    assert!(args.contains(&"features.shell_tool=false"));
    assert!(
        !args
            .iter()
            .any(|arg| arg.contains("bypass") || arg.contains("full-auto"))
    );
}

#[cfg(windows)]
fn fake_native(dir: &Path) -> std::path::PathBuf {
    let binary = dir.join("native.cmd");
    let calls = dir.join("calls.txt");
    let script = format!(
        "@echo off\r\necho %CD%>>\"{}\"\r\necho {{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"tool_use\",\"id\":\"r\",\"name\":\"Read\",\"input\":{{\"file_path\":\"src/net/retry.js\"}}}},{{\"type\":\"text\",\"text\":\"retry.js contains 7\"}}]}}}}\r\necho {{\"type\":\"result\",\"is_error\":false,\"usage\":{{\"input_tokens\":10,\"output_tokens\":5}},\"total_cost_usd\":0.01}}\r\n",
        calls.display()
    );
    std::fs::write(&binary, script).unwrap();
    binary
}

#[cfg(windows)]
#[tokio::test]
async fn fake_native_claude_stream_evaluated_and_saved() {
    let dir = tempfile::tempdir().unwrap();
    let binary = fake_native(dir.path());
    let service = service();
    let run = run_native(
        &service,
        Native::ClaudeCode,
        &binary,
        "test-model",
        Some("05-search"),
        true,
    )
    .await
    .unwrap();
    assert!(
        run.results[0].outcome.passed,
        "{:?}",
        run.results[0].outcome
    );
    assert_eq!(run.model, "test-model");
    assert_eq!(run.native.as_deref(), Some("claude_code"));
    assert_eq!(run.results[0].metrics.input_tokens, 10);
    assert_eq!(run.results[0].metrics.tool_calls, 1);
    assert_eq!(service.store().parity_runs(100).unwrap().len(), 1);
}

#[cfg(windows)]
#[tokio::test]
async fn each_native_task_uses_a_fresh_disposable_copy() {
    let dir = tempfile::tempdir().unwrap();
    let binary = fake_native(dir.path());
    let run = run_native(
        &service(),
        Native::ClaudeCode,
        &binary,
        "test-model",
        None,
        true,
    )
    .await
    .unwrap();
    assert_eq!(run.results.len(), 6);
    let text = std::fs::read_to_string(dir.path().join("calls.txt")).unwrap();
    let paths: std::collections::HashSet<_> = text.lines().collect();
    assert_eq!(paths.len(), 6);
    for path in paths {
        assert!(!Path::new(path).exists(), "temporary fixture not removed");
    }
}

#[tokio::test]
async fn codex_is_not_run_and_leaves_no_result_in_the_matrix() {
    let service = service();
    let error = run_native(&service, Native::Codex, Path::new("must-not-spawn"), "model", None, true)
        .await
        .unwrap_err();
    assert!(error.contains("Not run"), "{error}");
    assert!(service.store().parity_runs(10).unwrap().is_empty(), "an unmeasured comparison must not be saved as a failed run");
}
