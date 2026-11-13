//! Agents set up in the app: stored in SQLite, keys in the keychain, joined with vatra.toml.

use vatra_core::{MemorySecretStore, SecretStore, Store};
use vatra_service::config::{AgentConfig, Config, agent_secret};
use vatra_service::setup::{AgentForm, agent_from_form, check_save, detect, list, provider_models, remove, save};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn openrouter(name: &str, key: Option<&str>) -> AgentForm {
    AgentForm::Model {
        name: name.into(),
        provider: "openrouter".into(),
        base_url: None,
        model: "meta/llama".into(),
        key: key.map(String::from),
    }
}

#[test]
fn key_goes_to_the_keychain_only() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    let (row, key) = agent_from_form(openrouter("router", Some(" sk-or-secret ")), 1).unwrap();
    assert!(!row.settings_json.contains("sk-or"), "{}", row.settings_json);
    save(&store, &secrets, &row, key.as_deref()).unwrap();
    assert_eq!(secrets.get(&agent_secret("router")).unwrap().as_deref(), Some("sk-or-secret"));
    let views = list(&store, &secrets, &[]);
    assert_eq!((views[0].provider.as_deref(), views[0].has_key, views[0].source), (Some("openrouter"), true, "app"));
    let text = serde_json::to_string(&views).unwrap();
    assert!(!text.contains("sk-or"));
    remove(&store, &secrets, "router").unwrap();
    assert_eq!(secrets.get(&agent_secret("router")).unwrap(), None);
    assert!(store.configured_agents().unwrap().is_empty());
}

#[test]
fn saving_again_without_a_key_keeps_the_old_one() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    let (row, key) = agent_from_form(openrouter("router", Some("first")), 1).unwrap();
    save(&store, &secrets, &row, key.as_deref()).unwrap();
    let (row, key) = agent_from_form(openrouter("router", None), 1).unwrap();
    save(&store, &secrets, &row, key.as_deref()).unwrap();
    assert_eq!(secrets.get(&agent_secret("router")).unwrap().as_deref(), Some("first"));
}

#[test]
fn merged_agents_take_the_key_and_the_file_wins() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    for (name, key) in [("router", "k1"), ("api", "k2")] {
        let (row, key) = agent_from_form(openrouter(name, Some(key)), 1).unwrap();
        save(&store, &secrets, &row, key.as_deref()).unwrap();
    }
    let file = Config::parse(
        "summarizer = \"\"\n[agents.api]\nkind = \"model\"\nbase_url = \"http://127.0.0.1:9/v1\"\nmodel = \"file-model\"\n",
    )
    .unwrap();
    let (config, warnings) = file.merged_with(&store.configured_agents().unwrap(), Some("router"), &secrets);
    let AgentConfig::Model { api_key, model, .. } = &config.agents["router"] else { panic!() };
    assert_eq!((api_key.as_deref(), model.as_str()), (Some("k1"), "meta/llama"));
    let AgentConfig::Model { model, api_key, .. } = &config.agents["api"] else { panic!() };
    assert_eq!((model.as_str(), api_key.as_deref()), ("file-model", None), "vatra.toml wins");
    assert!(warnings.iter().any(|warning| warning.contains("`api` is set in vatra.toml")));
    assert_eq!(config.summarizer, "router");
}

#[test]
fn summarizer_may_be_a_local_model_and_a_bad_one_is_dropped() {
    let (config, warnings) = Config::empty().merged_with(&[], Some("local:qwen"), &MemorySecretStore::default());
    assert_eq!((config.summarizer.as_str(), warnings.len()), ("local:qwen", 0));
    let (config, warnings) = Config::empty().merged_with(&[], Some("missing"), &MemorySecretStore::default());
    assert_eq!(config.summarizer, "");
    assert_eq!(warnings.len(), 1);
}

#[test]
fn broken_rows_are_warnings() {
    let row = vatra_core::ConfiguredAgentRow { name: "x".into(), kind: "model".into(), settings_json: "{".into(), created_at: 1 };
    let (config, warnings) = Config::empty().merged_with(&[row], None, &MemorySecretStore::default());
    assert!(config.agents.is_empty());
    assert!(warnings[0].contains("couldn't be read"));
}

#[test]
fn forms_are_checked() {
    assert!(agent_from_form(openrouter("bad name", None), 1).is_err());
    assert!(agent_from_form(openrouter("local-x", None), 1).is_err(), "local names belong to local models");
    let custom = AgentForm::Model { name: "mine".into(), provider: "custom".into(), base_url: None, model: "m".into(), key: None };
    assert!(agent_from_form(custom, 1).is_err(), "a custom provider needs an address");
    let (row, _) = agent_from_form(AgentForm::Claude { binary: "C:/x/claude.exe".into() }, 1).unwrap();
    assert_eq!((row.name.as_str(), row.kind.as_str()), ("claude", "claude"));
    let anthropic = AgentForm::Model { name: "a".into(), provider: "anthropic".into(), base_url: None, model: "claude-haiku-5-5".into(), key: None };
    let (row, _) = agent_from_form(anthropic, 1).unwrap();
    let agent: AgentConfig = serde_json::from_str(&row.settings_json).unwrap();
    assert!(matches!(agent, AgentConfig::Model { base_url: None, driver: Some(ref driver), .. } if driver == "anthropic"));
}

#[test]
fn finds_installed_agents() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    let home = dir.path().join("Răzvan");
    let npm = dir.path().join("npm");
    for file in [bin.join("codex.exe"), bin.join("node.exe"), home.join(".local/bin/claude.exe")] {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, b"").unwrap();
    }
    let package = npm.join("@zed-industries").join("codex-acp");
    std::fs::create_dir_all(package.join("dist")).unwrap();
    std::fs::write(package.join("package.json"), r#"{"name":"@zed-industries/codex-acp","bin":{"codex-acp":"dist/index.js"}}"#).unwrap();
    std::fs::write(package.join("dist/index.js"), b"").unwrap();
    std::fs::create_dir_all(npm.join("typescript")).unwrap();
    std::fs::write(npm.join("typescript/package.json"), r#"{"name":"typescript","bin":"bin/tsc"}"#).unwrap();
    let path = std::env::join_paths([bin.clone()]).unwrap();
    let found = detect(&path, Some(&home), Some(&npm));
    assert_eq!(found.claude, Some(home.join(".local/bin/claude.exe")));
    assert_eq!(found.codex, Some(bin.join("codex.exe")));
    assert_eq!(found.node, Some(bin.join("node.exe")));
    assert_eq!(found.acp_scripts, vec![("@zed-industries/codex-acp".to_string(), package.join("dist/index.js"))]);
}

#[tokio::test]
async fn provider_models_are_read_without_a_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": [
            { "id": "a/free", "name": "Free", "context_length": 32768, "supported_parameters": ["tools"], "pricing": { "prompt": "0", "completion": "0" } },
            { "id": "b/paid", "name": "Paid", "context_length": 8192, "supported_parameters": [], "pricing": { "prompt": "0.1", "completion": "0.2" } }
        ] })))
        .mount(&server)
        .await;
    let models = provider_models(&format!("{}/api/v1/models", server.uri())).await.unwrap();
    assert_eq!(models[0]["tools"], true);
    assert_eq!(models[0]["free"], true);
    assert_eq!(models[1]["tools"], false);
    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].headers.get("authorization").is_none());
}

#[test]
fn the_dev_origin_is_only_for_debug_builds() {
    use vatra_service::origin_allowed;
    assert!(origin_allowed("http://tauri.localhost", false));
    assert!(origin_allowed("http://localhost:1420", true));
    assert!(!origin_allowed("http://localhost:1420", false));
    assert!(!origin_allowed("https://example.com", true));
}

#[test]
fn a_name_from_vatra_toml_is_refused_instead_of_silently_ignored() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    let (row, key) = agent_from_form(openrouter("router", Some("k")), 1).unwrap();
    let error = check_save(&store, &secrets, &["router".to_string()], &row, key.as_deref()).unwrap_err();
    assert!(error.contains("vatra.toml"), "{error}");
    check_save(&store, &secrets, &["other".to_string()], &row, key.as_deref()).unwrap();
}

#[test]
fn an_old_key_does_not_follow_the_agent_to_a_new_address() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    let (row, key) = agent_from_form(openrouter("router", Some("sk-first")), 1).unwrap();
    save(&store, &secrets, &row, key.as_deref()).unwrap();
    let elsewhere = AgentForm::Model {
        name: "router".into(),
        provider: "custom".into(),
        base_url: Some("https://example.test/v1".into()),
        model: "m".into(),
        key: None,
    };
    let (moved, none) = agent_from_form(elsewhere, 2).unwrap();
    let error = check_save(&store, &secrets, &[], &moved, none.as_deref()).unwrap_err();
    assert!(error.contains("key"), "{error}");
    // The same address, or a new key, is fine.
    let (same, none) = agent_from_form(openrouter("router", None), 3).unwrap();
    check_save(&store, &secrets, &[], &same, none.as_deref()).unwrap();
    let elsewhere = AgentForm::Model {
        name: "router".into(), provider: "custom".into(), base_url: Some("https://example.test/v1".into()), model: "m".into(), key: Some("sk-new".into()),
    };
    let (moved, new_key) = agent_from_form(elsewhere, 4).unwrap();
    check_save(&store, &secrets, &[], &moved, new_key.as_deref()).unwrap();
}

#[test]
fn a_hosted_service_needs_a_key_and_a_local_one_does_not() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    let anthropic = |key: Option<&str>| AgentForm::Model {
        name: "anthropic".into(), provider: "anthropic".into(), base_url: None, model: "claude-haiku-5-5".into(), key: key.map(Into::into),
    };
    let (row, none) = agent_from_form(anthropic(None), 1).unwrap();
    let error = check_save(&store, &secrets, &[], &row, none.as_deref()).unwrap_err();
    assert!(error.contains("key"), "{error}");
    let (row, key) = agent_from_form(anthropic(Some("sk-ant")), 2).unwrap();
    check_save(&store, &secrets, &[], &row, key.as_deref()).unwrap();
    save(&store, &secrets, &row, key.as_deref()).unwrap();
    // Saved once with a key, it can be saved again without typing it.
    let (again, none) = agent_from_form(anthropic(None), 3).unwrap();
    check_save(&store, &secrets, &[], &again, none.as_deref()).unwrap();
    let ollama = AgentForm::Model { name: "ollama".into(), provider: "ollama".into(), base_url: None, model: "qwen".into(), key: None };
    let (row, none) = agent_from_form(ollama, 4).unwrap();
    check_save(&store, &secrets, &[], &row, none.as_deref()).unwrap();
}

fn codex_form(managed: bool, codex: Option<&str>) -> AgentForm {
    AgentForm::Acp {
        name: "codex".into(),
        node: "C:/node/node.exe".into(),
        script: Some("C:/npm/codex-acp.js".into()),
        args: vec![],
        startup_timeout_sec: None,
        turn_timeout_sec: None,
        subscription: false,
        managed_account: managed,
        codex_binary: codex.map(Into::into),
    }
}

#[test]
fn manual_acp_commands_preserve_arguments_without_a_node_script() {
    let form: AgentForm = serde_json::from_value(serde_json::json!({
        "kind":"acp", "name":"gemini-work", "node":"C:/Tools With Spaces/agent.exe",
        "args":["--acp", "--profile", "work profile"], "startupTimeoutSec":45, "turnTimeoutSec":1800
    })).unwrap();
    let (row, key) = agent_from_form(form, 3).unwrap();
    assert!(key.is_none());
    assert_eq!(row.kind, "acp");
    let config: AgentConfig = serde_json::from_str(&row.settings_json).unwrap();
    let AgentConfig::Acp { binary, args, startup_timeout_sec, turn_timeout_sec, .. } = config else { panic!("not ACP") };
    assert_eq!(binary, std::path::PathBuf::from("C:/Tools With Spaces/agent.exe"));
    assert_eq!(args, ["--acp", "--profile", "work profile"]);
    assert_eq!(startup_timeout_sec, 45);
    assert_eq!(turn_timeout_sec, 1800);
}

#[test]
fn manual_acp_commands_reject_shell_shims_and_invalid_limits() {
    for (binary, timeout) in [("agent.cmd", 30), ("agent.bat", 30), ("", 30), ("agent.exe", 0)] {
        let form: AgentForm = serde_json::from_value(serde_json::json!({
            "kind":"acp", "name":"custom", "node":binary, "args":[], "startupTimeoutSec":timeout
        })).unwrap();
        assert!(agent_from_form(form, 1).is_err());
    }
}

#[test]
fn codex_uses_the_persons_own_login_unless_the_managed_account_is_chosen() {
    let (row, _) = agent_from_form(codex_form(false, Some("C:/codex.exe")), 1).unwrap();
    let agent: AgentConfig = serde_json::from_str(&row.settings_json).unwrap();
    let AgentConfig::Acp { chatgpt_plan, codex_binary, .. } = agent else { panic!("not acp") };
    assert!(!chatgpt_plan);
    assert!(codex_binary.is_none(), "the person's own Codex login needs no launcher");
}

#[test]
fn a_managed_account_needs_the_official_codex_and_a_valid_configuration() {
    let (row, _) = agent_from_form(codex_form(true, Some("C:/codex.exe")), 1).unwrap();
    let agent: AgentConfig = serde_json::from_str(&row.settings_json).unwrap();
    let AgentConfig::Acp { chatgpt_plan, subscription, codex_binary, .. } = agent else { panic!("not acp") };
    assert!(chatgpt_plan && subscription);
    assert_eq!(codex_binary.as_deref(), Some(std::path::Path::new("C:/codex.exe")));
    // The same settings pass the configuration check the service runs at start.
    let config = Config::parse("summarizer = \"\"
[agents.api]
kind = \"model\"
base_url = \"http://127.0.0.1:9/v1\"
model = \"m\"
").unwrap();
    let secrets = MemorySecretStore::default();
    let (merged, warnings) = config.merged_with(&[row], None, &secrets);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(merged.agents.contains_key("codex"));
    let error = agent_from_form(codex_form(true, None), 1).unwrap_err();
    assert!(error.contains("Codex"), "{error}");
}

#[tokio::test]
async fn configured_api_catalogue_uses_its_saved_key_without_generating() {
    use vatra_service::{AgentFactory,ConfigAgents};
    use wiremock::matchers::header;
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v1/models")).and(header("authorization","Bearer synthetic-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":[{"id":"other","name":"Other"}]})))
        .expect(1).mount(&server).await;
    let mut config = Config::empty();
    config.agents.insert("api".into(), AgentConfig::Model { base_url:Some(format!("{}/v1",server.uri())), model:"original".into(), driver:None,
        api_key_env:None,api_key:Some("synthetic-key".into()),context_window:None });
    let options = ConfigAgents::new(config).agent_options("api",0,None).await.unwrap();
    assert_eq!(options["models"][0]["id"],"other");
    assert_eq!(options["current_model"],"original");
    assert_eq!(server.received_requests().await.unwrap().len(),1);
    assert!(!options.to_string().contains("synthetic-key"));
}

#[tokio::test]
async fn catalogue_errors_do_not_echo_provider_body_or_credentials() {
    use vatra_service::{AgentFactory,ConfigAgents};
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401).set_body_string("synthetic-secret-from-provider"))
        .mount(&server).await;
    let mut config = Config::empty();
    config.agents.insert("api".into(), AgentConfig::Model { base_url:Some(format!("{}/v1",server.uri())), model:"original".into(), driver:None,
        api_key_env:None,api_key:None,context_window:None });
    let error = ConfigAgents::new(config).agent_options("api",0,None).await.unwrap_err();
    assert!(error.contains("401"));
    assert!(!error.contains("synthetic-secret"));
}

#[test]
fn asking_an_acp_agent_for_its_models_works_outside_any_project() {
    use std::sync::Arc;
    use vatra_service::{BuildRequest, ConfigAgents, PermissionMode, TurnOptions};
    let mut config = Config::empty();
    config.agents.insert("codex-acp".into(), AgentConfig::Acp {
        binary: "node".into(), args: vec![], subscription: true, chatgpt_plan: false, codex_binary: None,
        startup_timeout_sec: 30, turn_timeout_sec: 900,
    });
    // The picker asks before any conversation exists: no project and no session yet.
    let request = BuildRequest {
        name: "codex-acp".into(), session_id: 0, project_id: 0,
        approver: Arc::new(vatra_engine::DenyAll), questioner: Arc::new(vatra_engine::NoQuestions),
        mode: PermissionMode::Supervised, cwd: None, options: TurnOptions::default(),
    };
    let found = ConfigAgents::new(config).acp_config(&request).unwrap();
    assert!(found.cwd.ends_with("chats"), "{:?}", found.cwd);
}
