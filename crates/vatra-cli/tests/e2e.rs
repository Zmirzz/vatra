use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const SUMMARY: &str = r#"{"summary":"s","decisions":[],"remaining":[]}"#;

fn target_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let status = Command::new(env!("CARGO"))
            .args([
                "build",
                "-p",
                "vatra-engine",
                "-p",
                "vatra-claude",
                "--bins",
            ])
            .status()
            .unwrap();
        assert!(status.success());
        Path::new(env!("CARGO_BIN_EXE_vatra"))
            .parent()
            .unwrap()
            .to_path_buf()
    })
}

struct Output {
    success: bool,
    stdout: String,
    stderr: String,
}

impl Output {
    fn session_id(&self) -> String {
        self.stdout
            .lines()
            .next()
            .unwrap()
            .strip_prefix("session: ")
            .unwrap()
            .to_string()
    }
}

fn run_vatra(directory: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_vatra"))
        .arg("--db")
        .arg(directory.join("vatra-test.db"))
        .args(args)
        .env("APPDATA", directory.join("appdata"))
        .env("HOME", directory.join("home"))
        .env("USERPROFILE", directory.join("home"))
        .env("CLAUDE_CONFIG_DIR", directory.join("home/.claude"))
        .env("CODEX_HOME", directory.join("home/.codex"))
        .envs(env.iter().copied())
        .current_dir(directory)
        .output()
        .unwrap();
    Output {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn completion(message: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"choices": [{"message": message}]}))
}

struct E2e {
    directory: tempfile::TempDir,
    fake_directory: PathBuf,
    server: MockServer,
}

impl E2e {
    async fn start() -> E2e {
        let server = MockServer::start().await;
        let call = json!({"role": "assistant", "content": null, "tool_calls": [{
            "id": "c1", "type": "function",
            "function": {"name": "mcp__demo__echo", "arguments": "{\"text\":\"ășț\"}"},
        }]});
        Mock::given(method("POST"))
            .respond_with(completion(call))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let reply = json!({"role": "assistant", "content": SUMMARY});
        Mock::given(method("POST"))
            .respond_with(completion(reply))
            .mount(&server)
            .await;

        let directory = tempfile::tempdir().unwrap();
        let fake_directory = directory.path().join("claude cu spații");
        std::fs::create_dir_all(&fake_directory).unwrap();
        let fake = fake_directory.join("fake-claude.exe");
        std::fs::copy(target_dir().join("fake-claude.exe"), &fake).unwrap();
        let stream = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../vatra-claude/tests/fixtures/stream.jsonl");
        let fake_config = json!({ "stream": stream.display().to_string(), "exit": 0 });
        std::fs::write(
            fake_directory.join("fake-config.json"),
            fake_config.to_string(),
        )
        .unwrap();

        let config = format!(
            r#"
summarizer = "local"

[agents.local]
kind = "model"
base_url = "{uri}"
model = "m"

[agents.api]
kind = "model"
base_url = "{uri}"
model = "m"
api_key_env = "VATRA_TEST_KEY"

[agents.claude]
kind = "claude"
binary = '{claude}'

[[mcp]]
name = "demo"
command = '{mcp}'
"#,
            uri = server.uri(),
            claude = fake.display(),
            mcp = target_dir().join("vatra-fixture-mcp.exe").display(),
        );
        std::fs::write(directory.path().join("vatra.toml"), config).unwrap();
        E2e {
            directory,
            fake_directory,
            server,
        }
    }

    fn vatra(&self, args: &[&str]) -> Output {
        run_vatra(self.directory.path(), args, &[])
    }

    fn fake_record(&self) -> Value {
        let text = std::fs::read_to_string(self.fake_directory.join("fake-record.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    async fn last_authorization(&self) -> String {
        let requests = self.server.received_requests().await.unwrap();
        requests.last().unwrap().headers["authorization"]
            .to_str()
            .unwrap()
            .to_string()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn local_model_and_claude_share_mcp_and_context() {
    let env = E2e::start().await;
    let first = env.vatra(&[
        "run",
        "--project",
        "p",
        "--agent",
        "local",
        "--yes",
        "salut",
    ]);
    assert!(first.success, "{}", first.stderr);
    assert!(first.stdout.contains("mcp__demo__echo"));
    assert!(first.stdout.contains("ășț"));

    let session = first.session_id();
    let second = env.vatra(&[
        "handoff",
        "--from",
        &session,
        "--to",
        "claude",
        "--yes",
        "continuă",
    ]);
    assert!(second.success, "{}", second.stderr);
    let record = env.fake_record();
    assert!(record["stdin"].as_str().unwrap().contains("continuă"));
    assert!(record["args"].as_array().unwrap().iter().any(|arg| {
        let arg = arg.as_str().unwrap();
        arg.contains("mcpServers") && arg.contains("demo")
    }));

    let shown = env.vatra(&["show", &session]);
    assert!(shown.stdout.contains("handoff_result"));
}

#[tokio::test(flavor = "multi_thread")]
async fn api_key_comes_from_the_named_env_var_and_is_never_printed() {
    let env = E2e::start().await;
    let out = run_vatra(
        env.directory.path(),
        &["run", "--project", "p", "--agent", "api", "--yes", "x"],
        &[("VATRA_TEST_KEY", "secret-123")],
    );
    assert!(out.success, "{}", out.stderr);
    assert_eq!(env.last_authorization().await, "Bearer secret-123");
    assert!(!out.stdout.contains("secret-123") && !out.stderr.contains("secret-123"));
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_api_key_env_var_names_the_variable() {
    let env = E2e::start().await;
    let out = env.vatra(&["run", "--project", "p", "--agent", "api", "--yes", "x"]);
    assert!(!out.success);
    assert!(out.stderr.contains("VATRA_TEST_KEY"));
}

#[test]
fn missing_config_explains_what_to_do() {
    let directory = tempfile::tempdir().unwrap();
    let out = run_vatra(
        directory.path(),
        &["run", "--project", "p", "--agent", "local", "x"],
        &[],
    );
    assert!(!out.success);
    assert!(out.stderr.contains("vatra.example.toml"));
}

#[tokio::test(flavor = "multi_thread")]
async fn resuming_with_another_agent_points_to_handoff() {
    let env = E2e::start().await;
    let session = env
        .vatra(&["run", "--project", "p", "--agent", "local", "--yes", "x"])
        .session_id();
    let out = env.vatra(&[
        "run",
        "--project",
        "p",
        "--agent",
        "claude",
        "--session",
        &session,
        "y",
    ]);
    assert!(!out.success);
    assert!(out.stderr.contains("vatra handoff"));
}

#[tokio::test(flavor = "multi_thread")]
async fn without_yes_and_without_a_terminal_tools_are_denied() {
    let env = E2e::start().await;
    let out = env.vatra(&["run", "--project", "p", "--agent", "local", "salut"]);
    assert!(out.success, "{}", out.stderr);
    assert!(
        out.stdout.contains("tool_denied"),
        "stdout: {}\nstderr: {}",
        out.stdout,
        out.stderr
    );
    assert!(!out.stdout.contains("tool_result"));
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_agent_lists_the_configured_ones() {
    let env = E2e::start().await;
    let out = env.vatra(&["run", "--project", "p", "--agent", "nimeni", "x"]);
    assert!(!out.success);
    assert!(out.stderr.contains("local") && out.stderr.contains("claude"));
}

#[tokio::test(flavor = "multi_thread")]
async fn handoff_with_an_unavailable_summarizer_continues_degraded() {
    let env = E2e::start().await;
    let session = env
        .vatra(&[
            "run",
            "--project",
            "p",
            "--agent",
            "local",
            "--yes",
            "salut",
        ])
        .session_id();
    let path = env.directory.path().join("vatra.toml");
    let config = std::fs::read_to_string(&path)
        .unwrap()
        .replace("summarizer = \"local\"", "summarizer = \"api\"");
    std::fs::write(&path, config).unwrap();

    let out = env.vatra(&[
        "handoff",
        "--from",
        &session,
        "--to",
        "claude",
        "--yes",
        "continuă",
    ]);
    assert!(out.success, "{}", out.stderr);
    assert!(out.stdout.contains("\"degraded\":true"));
    assert!(out.stderr.contains("VATRA_TEST_KEY"));
}
