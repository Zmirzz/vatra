use std::path::{Path, PathBuf};

use vatra_cli::config::{AgentConfig, Config};

fn parse(text: &str) -> Result<Config, vatra_cli::config::ConfigError> {
    Config::parse(text)
}

#[test]
fn api_key_in_file_is_rejected() {
    let error = parse(
        r#"
        summarizer = "api"
        [agents.api]
        kind = "model"
        base_url = "http://x"
        model = "m"
        api_key = "k"
    "#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("api_key_env"));
}

#[test]
fn full_config_parses() {
    let config = parse(
        r#"
        summarizer = "api"
        [agents.api]
        kind = "model"
        base_url = "http://x"
        model = "m"
        api_key_env = "CHEIE"
        [agents.claude]
        kind = "claude"
        binary = "claude"
        [[mcp]]
        name = "demo"
        command = "demo-mcp"
    "#,
    )
    .unwrap();
    assert_eq!(config.summarizer, "api");
    assert_eq!(config.mcp[0].name, "demo");
    assert!(matches!(
        config.agents["claude"],
        AgentConfig::Claude { .. }
    ));
}

#[test]
fn summarizer_must_be_a_model_agent() {
    let error = parse(
        r#"
        summarizer = "claude"
        [agents.claude]
        kind = "claude"
        binary = "claude"
    "#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("summarizer"));
}

#[test]
fn claude_agent_must_be_named_claude() {
    let error = parse(
        r#"
        summarizer = "api"
        [agents.api]
        kind = "model"
        base_url = "http://x"
        model = "m"
        [agents.altul]
        kind = "claude"
        binary = "claude"
    "#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("altul"));
}

#[test]
fn missing_file_points_to_the_example() {
    let error = Config::load(Path::new("nu-exista.toml")).unwrap_err();
    assert!(error.to_string().contains("vatra.example.toml"));
}

#[test]
fn driver_and_context_window_are_read() {
    let config = parse(
        r#"
        summarizer = "api"
        [agents.api]
        kind = "model"
        base_url = "http://x"
        model = "m"
        driver = "openai"
        context_window = 32000
    "#,
    )
    .unwrap();
    assert!(matches!(
        &config.agents["api"],
        AgentConfig::Model { driver: Some(driver), context_window: Some(32000), .. } if driver == "openai"
    ));
}

#[test]
fn unknown_driver_is_rejected() {
    let error = parse(
        r#"
        summarizer = "api"
        [agents.api]
        kind = "model"
        base_url = "http://x"
        model = "m"
        driver = "gemini"
    "#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("gemini"));
}

#[test]
fn model_without_base_url_is_rejected_at_parse() {
    let error = parse(
        r#"
        summarizer = "api"
        [agents.api]
        kind = "model"
        model = "m"
    "#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("`api`"));
    assert!(error.to_string().contains("base_url"));

    let error = parse(
        r#"
        summarizer = "api"
        [agents.api]
        kind = "model"
        model = "m"
        driver = "openai"
    "#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("base_url"));
}

#[test]
fn anthropic_without_base_url_parses() {
    let config = parse(
        r#"
        summarizer = "api"
        [agents.api]
        kind = "model"
        model = "claude-test"
        driver = "anthropic"
    "#,
    )
    .unwrap();
    assert!(matches!(
        &config.agents["api"],
        AgentConfig::Model { base_url: None, driver: Some(driver), .. } if driver == "anthropic"
    ));
}

#[test]
fn unknown_driver_is_rejected_at_parse() {
    let error = parse(
        r#"
        summarizer = "api"
        [agents.api]
        kind = "model"
        base_url = "http://x"
        model = "m"
        driver = "gemini"
    "#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("`gemini`"));
    assert!(error.to_string().contains("openai or anthropic"));
}

#[test]
fn old_toml_still_parses() {
    let config = parse(
        r#"
        summarizer = "api"
        [agents.api]
        kind = "model"
        base_url = "http://x"
        model = "m"
        [[mcp]]
        name = "demo"
        command = "demo-mcp"
        args = ["--stdio"]
        env = { CHEIE = "valoare" }
    "#,
    )
    .unwrap();
    assert_eq!(config.mcp[0].command.as_deref(), Some("demo-mcp"));
    assert_eq!(config.mcp[0].args, ["--stdio"]);
    assert_eq!(config.mcp[0].env["CHEIE"], "valoare");
}

#[test]
fn rejects_both_command_and_url() {
    let error = parse(
        r#"
        summarizer = "api"
        [agents.api]
        kind = "model"
        base_url = "http://x"
        model = "m"
        [[mcp]]
        name = "demo"
        command = "demo-mcp"
        url = "https://example.test/mcp"
    "#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("demo"));
    assert!(error.to_string().contains("url"));
}

#[test]
fn server_with_a_url_parses_with_headers_and_secrets() {
    let config = parse(
        r#"
        summarizer = "api"
        [agents.api]
        kind = "model"
        base_url = "http://x"
        model = "m"
        [[mcp]]
        name = "remote"
        url = "https://example.test/mcp"
        headers = { X-Mode = "rapid" }
        tool_timeout_sec = 20
        disabled_tools = ["sterge"]
        secrets = { TOKEN = { env = "MCP_TOKEN" } }
    "#,
    )
    .unwrap();
    let server = &config.mcp[0];
    assert_eq!(server.url.as_deref(), Some("https://example.test/mcp"));
    assert_eq!(server.tool_timeout_sec, Some(20));
    assert_eq!(
        server.secrets["TOKEN"],
        vatra_core::SecretRef::Env("MCP_TOKEN".into())
    );
}

#[test]
fn duplicate_server_names_are_rejected() {
    let error = parse(
        r#"
        summarizer = "api"
        [agents.api]
        kind = "model"
        base_url = "http://x"
        model = "m"
        [[mcp]]
        name = "demo"
        command = "demo-mcp"
        [[mcp]]
        name = "demo"
        command = "altul-mcp"
    "#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("named `demo`"));
}

#[test]
fn empty_command_is_rejected() {
    let error = parse(
        r#"
        summarizer = "api"
        [agents.api]
        kind = "model"
        base_url = "http://x"
        model = "m"
        [[mcp]]
        name = "demo"
        command = ""
    "#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("empty command"));
}

/// Runs `vatra` in `folder` with the environment of a synthetic machine: APPDATA, HOME and the Claude and Codex
/// folders all inside `root`, so nothing of the real machine is read or written.
fn vatra_in(
    root: &std::path::Path,
    folder: &std::path::Path,
    args: &[&str],
) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_vatra"))
        .args(args)
        .current_dir(folder)
        .env("APPDATA", root.join("appdata"))
        .env("HOME", root.join("home"))
        .env("USERPROFILE", root.join("home"))
        .env("CLAUDE_CONFIG_DIR", root.join("home").join(".claude"))
        .env("CODEX_HOME", root.join("home").join(".codex"))
        .output()
        .unwrap()
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn cli_dry_run_writes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("work");
    std::fs::create_dir_all(&folder).unwrap();

    let output = vatra_in(root.path(), &folder, &["import", "--dry-run"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout(&output).contains("dry run: nothing was written"));
    assert!(
        !root.path().join("appdata").exists(),
        "a dry run created the app folder"
    );

    // With an existing database, the dry run reads it and changes none of its bytes.
    let data = root.path().join("appdata").join("Vatra");
    std::fs::create_dir_all(&data).unwrap();
    let db = data.join("vatra.db");
    {
        let store = vatra_core::Store::open(&db).unwrap();
        store.create_project("kept").unwrap();
        let catalog = vatra_extensions::Catalog::open(&db, &data).unwrap();
        catalog
            .put_rules(
                vatra_extensions::Origin::Claude,
                vatra_extensions::Scope::Global,
                "permissions:settings.json",
                serde_json::json!({ "deny": ["Read"] }),
                "h",
            )
            .unwrap();
    }
    let before = std::fs::read(&db).unwrap();

    let output = vatra_in(root.path(), &folder, &["import", "--dry-run"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read(&db).unwrap(),
        before,
        "a dry run changed the database"
    );
}

#[test]
fn default_db_same_for_cli_and_service() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("work");
    std::fs::create_dir_all(&folder).unwrap();

    let output = vatra_in(root.path(), &folder, &["import", "--dry-run"]);

    let expected = root.path().join("appdata").join("Vatra").join("vatra.db");
    assert!(
        stdout(&output).contains(&format!("database: {}", expected.display())),
        "{}",
        stdout(&output)
    );
    // SAFETY: this is the only test in this binary that reads APPDATA in this process.
    unsafe { std::env::set_var("APPDATA", root.path().join("appdata")) };
    let service_folder = vatra_service::default_data_dir().unwrap();
    assert_eq!(service_folder.join("vatra.db"), expected);
}

#[test]
fn local_vatra_db_is_not_used_and_warns() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("work");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("vatra.db"), b"").unwrap();

    let output = vatra_in(root.path(), &folder, &["import", "--dry-run"]);

    assert!(String::from_utf8_lossy(&output.stderr).contains("vatra.db in this folder isn't used"));
    assert!(!stdout(&output).contains(&folder.join("vatra.db").display().to_string()));
}

#[test]
fn explicit_db_still_works() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("work");
    std::fs::create_dir_all(&folder).unwrap();
    let db = root.path().join("own").join("mine.db");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_vatra"))
        .args(["--db", &db.display().to_string(), "import", "--dry-run"])
        .current_dir(&folder)
        .env("APPDATA", root.path().join("appdata"))
        .env("HOME", root.path().join("home"))
        .env("USERPROFILE", root.path().join("home"))
        .env("CLAUDE_CONFIG_DIR", root.path().join("home/.claude"))
        .env("CODEX_HOME", root.path().join("home/.codex"))
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout(&output).contains(&format!("database: {}", db.display())));
}

/// `vatra` run on a blocking thread, so the mock model in the test still serves while it waits.
async fn vatra_async(root: PathBuf, folder: PathBuf, args: Vec<String>) -> std::process::Output {
    tokio::task::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        vatra_in(&root, &folder, &args)
    })
    .await
    .unwrap()
}

fn model_config(base_url: &str) -> String {
    format!(
        "summarizer = \"api\"\n[agents.api]\nkind = \"model\"\nbase_url = \"{base_url}\"\nmodel = \"m\"\n"
    )
}

/// A model that answers every request with a plain reply.
async fn plain_model() -> wiremock::MockServer {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"choices": [{"message": {"role": "assistant", "content": "done"}}]}),
        ))
        .mount(&server)
        .await;
    server
}

fn model_bodies(requests: Vec<wiremock::Request>) -> Vec<String> {
    requests
        .iter()
        .filter(|request| request.method == wiremock::http::Method::POST)
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn vatra_run_sees_a_catalog_skill() {
    let root = tempfile::tempdir().unwrap();
    let skill = root.path().join("hello");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\ndescription: Greets the user from the catalog\n---\nSay hello.\n",
    )
    .unwrap();
    let data = root.path().join("appdata").join("Vatra");
    std::fs::create_dir_all(&data).unwrap();
    vatra_extensions::Catalog::open(&data.join("vatra.db"), &data)
        .unwrap()
        .add_folder(
            &skill,
            vatra_extensions::Origin::Manual,
            vatra_extensions::Scope::Global,
            false,
        )
        .unwrap();
    let server = plain_model().await;
    let config = root.path().join("vatra.toml");
    std::fs::write(&config, model_config(&server.uri())).unwrap();
    let folder = root.path().join("work");
    std::fs::create_dir_all(&folder).unwrap();

    let output = vatra_async(
        root.path().to_path_buf(),
        folder,
        vec![
            "--config".into(),
            config.display().to_string(),
            "run".into(),
            "--project".into(),
            "p".into(),
            "--agent".into(),
            "api".into(),
            "hi".into(),
        ],
    )
    .await;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bodies = model_bodies(server.received_requests().await.unwrap());
    assert!(
        bodies
            .iter()
            .any(|body| body.contains("Greets the user from the catalog")),
        "{bodies:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_config_does_not_change_the_catalog_servers() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("appdata").join("Vatra");
    std::fs::create_dir_all(&data).unwrap();
    let catalog = std::sync::Arc::new(
        vatra_extensions::Catalog::open(&data.join("vatra.db"), &data).unwrap(),
    );
    // The app's own configuration, which the service syncs into the catalog.
    let app = vatra_service::config::Config::parse(&format!(
        "{}[[mcp]]\nname = \"app-server\"\ncommand = \"node\"\n",
        model_config("http://127.0.0.1:9")
    ))
    .unwrap();
    let _service_agents =
        vatra_service::ConfigAgents::new(app).with_catalog_synced(catalog.clone());
    assert!(
        catalog
            .list()
            .unwrap()
            .iter()
            .any(|entry| entry.name == "app-server")
    );

    let server = plain_model().await;
    let config = root.path().join("other.toml");
    std::fs::write(&config, model_config(&server.uri())).unwrap();
    let folder = root.path().join("work");
    std::fs::create_dir_all(&folder).unwrap();
    let output = vatra_async(
        root.path().to_path_buf(),
        folder,
        vec![
            "--config".into(),
            config.display().to_string(),
            "run".into(),
            "--project".into(),
            "p".into(),
            "--agent".into(),
            "api".into(),
            "hi".into(),
        ],
    )
    .await;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        catalog
            .list()
            .unwrap()
            .iter()
            .any(|entry| entry.name == "app-server")
    );
}
