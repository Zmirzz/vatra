use std::sync::Arc;

use vatra_engine::{AllowAll, NoQuestions, WebSearchConfig};
use vatra_service::config::{AgentConfig, Config};
use vatra_service::{AgentFactory, BridgeSettings, BuildRequest, ConfigAgents, PermissionMode};

fn agents(text: &str) -> ConfigAgents {
    ConfigAgents::new(Config::parse(text).unwrap())
}

fn request(name: &str) -> BuildRequest {
    BuildRequest {
        name: name.into(),
        session_id: 1,
        project_id: 1,
        approver: Arc::new(AllowAll),
        questioner: Arc::new(NoQuestions),
        mode: PermissionMode::FullAccess,
        cwd: None,
        options: Default::default(),
    }
}

#[tokio::test]
async fn anthropic_agent_builds_without_a_base_url() {
    let agents = agents(
        r#"
summarizer = "helper"

[agents.helper]
kind = "model"
driver = "anthropic"
model = "claude-test"
"#,
    );
    assert!(agents.build(request("helper")).await.is_ok());
}

#[tokio::test]
async fn openai_agent_needs_a_base_url() {
    // Config::parse rejects this, so the factory is given the config directly to check its own guard.
    let model = AgentConfig::Model {
        base_url: None,
        model: "m".into(),
        api_key_env: None,
        driver: Some("openai".into()),
        context_window: None,
        api_key: None,
    };
    let config = Config {
        agents: [("helper".to_string(), model)].into_iter().collect(),
        summarizer: "helper".into(),
        mcp: vec![],
        web_search: None,
        engine: Default::default(),
        local: Default::default(),
    };
    let error = ConfigAgents::new(config)
        .build(request("helper"))
        .await
        .err()
        .unwrap();
    assert_eq!(error, "agent `helper` needs a base_url");
}

const BASE_CONFIG: &str = r#"
summarizer = "helper"

[agents.helper]
kind = "model"
base_url = "http://localhost:1234/v1"
model = "m"
"#;

#[test]
fn web_search_section_defaults_to_brave() {
    let text = format!("{BASE_CONFIG}\n[web_search]\napi_key_env = \"BRAVE_API_KEY\"\n");
    let config = Config::parse(&text).unwrap();
    let expected = WebSearchConfig {
        provider: "brave".into(),
        api_key_env: "BRAVE_API_KEY".into(),
    };
    assert_eq!(config.web_search, Some(expected));
}

#[test]
fn web_search_refuses_other_providers_and_missing_key_variables() {
    let other =
        format!("{BASE_CONFIG}\n[web_search]\nprovider = \"bing\"\napi_key_env = \"KEY\"\n");
    let error = Config::parse(&other).unwrap_err().to_string();
    assert!(error.contains("bing"), "{error}");

    let missing = format!("{BASE_CONFIG}\n[web_search]\nprovider = \"brave\"\n");
    assert!(Config::parse(&missing).is_err());
}

#[tokio::test]
async fn engine_agent_keeps_its_session_and_memory_folders_in_the_data_dir() {
    let data = tempfile::TempDir::new().unwrap();
    let bridge = BridgeSettings {
        binary: "vatra".into(),
        url: "http://127.0.0.1:1".into(),
        token_file: data.path().join("token"),
        data_dir: data.path().to_path_buf(),
    };
    let agents = ConfigAgents::with_bridge(Config::parse(BASE_CONFIG).unwrap(), bridge);
    agents.build(request("helper")).await.unwrap();

    assert!(data.path().join("sessions").join("1").is_dir());
    assert!(
        data.path()
            .join("projects")
            .join("1")
            .join("memory")
            .is_dir()
    );
    assert!(data.path().join("workspaces").join("project-1").is_dir());
}

#[test]
fn engine_max_steps_is_read_from_the_engine_section() {
    let text = format!(
        "{BASE_CONFIG}
[engine]
max_steps = 5
"
    );
    assert_eq!(Config::parse(&text).unwrap().engine.max_steps, 5);
    assert_eq!(Config::parse(BASE_CONFIG).unwrap().engine.max_steps, 200);
}

#[test]
fn engine_section_refuses_zero_steps() {
    let text = format!(
        "{BASE_CONFIG}
[engine]
max_steps = 0
"
    );
    let error = Config::parse(&text).unwrap_err().to_string();
    assert!(error.contains("max_steps"), "{error}");
}

#[test]
fn example_config_parses() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vatra.example.toml");
    let config = Config::load(&path).unwrap();
    assert_eq!(config.engine.max_steps, 200);
}

#[test]
fn acp_configuration_accepts_arbitrary_names_and_validates_timeouts() {
    let text = format!(
        "{BASE_CONFIG}\n[agents.external]\nkind = \"acp\"\nbinary = \"agent.exe\"\nargs = [\"--acp\"]\n"
    );
    let config = Config::parse(&text).unwrap();
    assert!(matches!(&config.agents["external"], AgentConfig::Acp {
        args, startup_timeout_sec: 30, turn_timeout_sec: 900, ..
    } if args == &["--acp"]));
    for value in [0, 86401, u64::MAX] {
        assert!(Config::parse(&format!("{text}turn_timeout_sec = {value}\n")).is_err());
    }
    assert!(Config::parse(&text.replace("agent.exe", "")).is_err());
    assert!(
        Config::parse(&text.replace("summarizer = \"helper\"", "summarizer = \"external\""))
            .is_err()
    );
}

#[tokio::test]
async fn acp_factory_builds_without_starting_a_process() {
    let data = tempfile::tempdir().unwrap();
    let text = format!(
        "{BASE_CONFIG}\n[agents.external]\nkind = \"acp\"\nbinary = \"not-installed-agent.exe\"\n"
    );
    let agents = ConfigAgents::with_bridge(
        Config::parse(&text).unwrap(),
        BridgeSettings {
            binary: "vatra".into(),
            url: "http://127.0.0.1:1".into(),
            token_file: data.path().join("token"),
            data_dir: data.path().into(),
        },
    );
    let agent = agents.build(request("external")).await.unwrap();
    assert_eq!(agent.name(), "external");
    assert!(data.path().join("workspaces/project-1").is_dir());
    assert!(agents.names().contains(&"external".into()));
}

#[test]
fn acp_working_folder_has_no_verbatim_prefix() {
    let folder = tempfile::tempdir().unwrap();
    let text =
        format!("{BASE_CONFIG}\n[agents.external]\nkind = \"acp\"\nbinary = \"agent.exe\"\n");
    let factory = agents(&text);
    let mut request = request("external");
    request.cwd = Some(folder.path().to_path_buf());
    let cwd = factory.acp_config(&request).unwrap().cwd;
    // Node-based agents mishandle the \\?\ form that std::fs::canonicalize returns on Windows.
    assert!(
        !cwd.display().to_string().starts_with(r"\\?\"),
        "{}",
        cwd.display()
    );
    assert_eq!(cwd, dunce::canonicalize(folder.path()).unwrap());
}

#[test]
fn hidden_variables_cover_model_search_and_external_agent_keys() {
    let text = r#"
summarizer = "helper"

[agents.helper]
kind = "model"
base_url = "http://localhost:1234/v1"
model = "m"
api_key_env = "HELPER_MODEL_KEY"

[agents.external]
kind = "acp"
binary = "agent.exe"

[web_search]
api_key_env = "BRAVE_API_KEY"
"#;
    let factory = agents(text);
    let expected = ["BRAVE_API_KEY", "HELPER_MODEL_KEY"];
    assert_eq!(factory.hidden_env(), expected);
    assert_eq!(
        factory.acp_config(&request("external")).unwrap().hidden_env,
        expected
    );
}

#[tokio::test]
async fn acp_factory_delivers_mcp_without_spawning() {
    let text = format!(
        "{BASE_CONFIG}\n[agents.external]\nkind = \"acp\"\nbinary = \"agent.exe\"\n[[mcp]]\nname = \"demo\"\ncommand = \"demo\"\n"
    );
    let factory = agents(&text);
    let config = factory.acp_config(&request("external")).unwrap();
    assert_eq!(config.extras.servers.len(), 1);
    assert_eq!(config.extras.servers[0].command.as_deref(), Some("demo"));
    assert!(factory.build(request("external")).await.is_ok());
}
