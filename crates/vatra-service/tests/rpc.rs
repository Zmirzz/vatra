mod common;
mod servers;
#[path = "../../vatra-engine/tests/support/mod.rs"]
mod support;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use servers::service_with;
use vatra_core::{Agent, McpServerConfig, MemorySecretStore, NewItem, SecretStore, Store, Summarizer};
use vatra_extensions::{Catalog, Scope};
use vatra_engine::{ExpandVars, McpHub};
use vatra_service::{AgentFactory, BuildRequest, Service, router};

const TOKEN: &str = "token-de-test-0123456789abcdef0123";

#[tokio::test]
async fn mcp_review_install_edit_and_uninstall_use_authenticated_rpc_without_plaintext_secrets() {
    let data=tempfile::tempdir().unwrap();let db=data.path().join("vatra.db");
    let store=Arc::new(Store::open(&db).unwrap());let secrets=Arc::new(MemorySecretStore::default());
    let catalog=Arc::new(Catalog::open(&db,data.path()).unwrap());
    let factory=vatra_service::ConfigAgents::new(vatra_service::config::Config::empty()).with_secret_store(secrets.clone()).with_catalog(catalog);
    let server=start_with(Service::new(store,Arc::new(factory)).unwrap()).await;let mut client=client_for(&server).await;
    let document=json!({"mcpServers":{"remote":{"type":"http","url":"https://example.org/mcp","headers":{"Authorization":"Bearer rpc-private"}}}}).to_string();
    let review=client.call("mcp.preview",json!({"document":document})).await.unwrap();
    assert!(!review.to_string().contains("rpc-private"));
    let saved=client.call("mcp.install",json!({"document":document,"reviewHash":review["servers"][0]["plan"]["reviewHash"]})).await.unwrap();
    let listed=client.call("catalog.list",json!({})).await.unwrap();assert_eq!(listed[0]["enabled"],false);assert!(!listed.to_string().contains("rpc-private"));
    let form=client.call("mcp.form",json!({"id":saved["id"]})).await.unwrap();assert!(!form.to_string().contains("rpc-private"));
    let removed=client.call("mcp.remove",json!({"id":saved["id"],"expectedHash":saved["hash"]})).await.unwrap();assert_eq!(removed["credentialCleanupFailed"],false);assert!(secrets.keys().is_empty());
}

#[tokio::test]
async fn skills_source_discovery_and_install_are_available_over_authenticated_rpc() {
    let data=tempfile::tempdir().unwrap();let source=data.path().join("notes");std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("SKILL.md"),"---\nname: notes\ndescription: Take notes\n---\nInstruction body.").unwrap();
    let db=data.path().join("vatra.db");
    let store=Arc::new(Store::open(&db).unwrap());
    let catalog=Arc::new(Catalog::open(&db,data.path()).unwrap());
    let factory=vatra_service::ConfigAgents::new(vatra_service::config::Config::empty()).with_catalog(catalog);
    let server=start_with(Service::new(store,Arc::new(factory)).unwrap()).await;
    let mut client=client_for(&server).await;
    let found=client.call("skills.set_sources",json!([{"path":source,"projectId":null}])).await.unwrap();
    assert_eq!(found[0]["name"],"notes");
    assert!(found[0].get("body").is_none());
    let target=found[0]["path"].clone();
    let inspected=client.call("skills.inspect",json!({"path":target})).await.unwrap();
    assert!(inspected["content"].as_str().unwrap().contains("Instruction body."));
    client.call("skills.install",json!({"path":target,"expectedHash":found[0]["hash"]})).await.unwrap();
    let installed=client.call("catalog.list",json!({})).await.unwrap();
    assert_eq!(installed[0]["kind"],"skill");
    client.call("skills.uninstall",json!({"id":installed[0]["id"]})).await.unwrap();
    assert!(source.join("SKILL.md").is_file());
}

#[tokio::test]
async fn manual_acp_settings_round_trip_without_dropping_arguments_or_timeouts() {
    let server = test_server().await;
    let mut client = client_for(&server).await;
    let input = json!({"kind":"acp", "name":"custom-agent", "node":"C:/Tools/agent.exe",
        "args":["acp","--profile","profile with spaces"], "startupTimeoutSec":45,
        "turnTimeoutSec":1800, "subscription":false});
    client.call("agents.save", input.clone()).await.unwrap();
    let form = client.call("agents.form", json!({"name":"custom-agent"})).await.unwrap();
    assert_eq!(form["args"], input["args"]);
    assert_eq!(form["startupTimeoutSec"], 45);
    assert_eq!(form["turnTimeoutSec"], 1800);
    assert!(form.get("script").is_none());
    client.call("agents.save", form).await.unwrap();
    let saved = server.service.store().configured_agents().unwrap();
    let saved: Value = serde_json::from_str(&saved.iter().find(|row| row.name == "custom-agent").unwrap().settings_json).unwrap();
    assert_eq!(saved["args"], input["args"]);
    assert_eq!(saved["binary"], input["node"]);
}

#[tokio::test]
async fn file_agent_stays_read_only_when_app_has_same_name() {
    let store=Arc::new(Store::open_in_memory().unwrap());
    let (row,_)=vatra_service::setup::agent_from_form(vatra_service::setup::AgentForm::Model {name:"same".into(),provider:"openrouter".into(),base_url:None,model:"app-model".into(),key:None},0).unwrap();
    store.save_configured_agent(&row).unwrap();
    let mut config=vatra_service::config::Config::empty();
    config.agents.insert("same".into(),vatra_service::config::AgentConfig::Model {base_url:Some("http://127.0.0.1:1/v1".into()),model:"file-model".into(),api_key_env:None,driver:Some("openai".into()),context_window:None,api_key:None});
    let factory=vatra_service::ConfigAgents::new(config).with_file_agents(vec!["same".into()]);
    let service=Service::new(store,Arc::new(factory)).unwrap();
    let server=start_with(service).await;
    let mut client=client_for(&server).await;
    let configured=client.call("agents.configured",json!({})).await.unwrap();
    assert_eq!(configured["agents"][0]["source"],"file");
    assert!(client.call("agents.form",json!({"name":"same"})).await.is_err());
}

#[tokio::test]
async fn onboarding_done_is_persisted_in_service_store() {
    let server=test_server().await;
    let mut client=client_for(&server).await;
    assert_eq!(client.call("onboarding.state",json!({})).await.unwrap()["done"],false);
    client.call("onboarding.done",json!({})).await.unwrap();
    assert_eq!(client.call("onboarding.state",json!({})).await.unwrap()["done"],true);
    assert_eq!(server.service.store().app_setting("onboarding.done").unwrap().as_deref(),Some("true"));
}

#[tokio::test]
async fn workspace_search_returns_the_matching_items() {
    let server=test_server().await;
    let mut client=client_for(&server).await;
    server.service.store().create_item(&NewItem {
        kind: "note".into(),
        title: "Ședință în țară".into(),
        body: "bugetul trimestrului".into(),
        fields: json!({}),
        project_id: None,
    }, 7).unwrap();
    server.service.store().create_item(&NewItem {
        kind: "task".into(),
        title: "Ședință".into(),
        body: String::new(),
        fields: json!({}),
        project_id: None,
    }, 8).unwrap();
    let hits=client.call("workspace.search",json!({"query":"sedinta","kinds":["note"],"limit":5})).await.unwrap();
    assert_eq!(hits.as_array().unwrap().len(),1);
    assert_eq!(hits[0]["kind"],"note");
    assert_eq!(hits[0]["title"],"Ședință în țară");
    assert_eq!(hits[0]["updated_at"],7);
    assert!(hits[0]["id"].is_i64());
    assert!(hits[0]["snippet"].is_string());
    let all=client.call("workspace.search",json!({"query":"sedinta"})).await.unwrap();
    assert_eq!(all.as_array().unwrap().len(),2);
    let missing=client.call("workspace.search",json!({})).await;
    assert!(missing.is_err(), "query is required");
}

#[tokio::test]
async fn workspace_export_and_backup_write_files_at_absolute_paths() {
    let server=test_server().await;
    let mut client=client_for(&server).await;
    server.service.store().create_item(&NewItem {
        kind: "note".into(),
        title: "Export".into(),
        body: String::new(),
        fields: json!({}),
        project_id: None,
    }, 3).unwrap();
    let dir=tempfile::tempdir().unwrap();
    let export=dir.path().join("items.jsonl");
    let result=client.call("workspace.export",json!({"path":export.to_str().unwrap()})).await.unwrap();
    assert_eq!(result["count"],1);
    assert_eq!(std::fs::read_to_string(&export).unwrap().lines().count(),1);
    let backup=dir.path().join("copia.db");
    assert_eq!(client.call("workspace.backup",json!({"path":backup.to_str().unwrap()})).await.unwrap(),json!({}));
    assert_eq!(Store::open(&backup).unwrap().items_by_kind("note",10).unwrap().len(),1);
    for (method,path) in [("workspace.export",&export),("workspace.backup",&backup)] {
        let error=client.call(method,json!({"path":path.to_str().unwrap()})).await.unwrap_err();
        assert_eq!(error["code"],-32602, "{method}: no overwrite, and the request is what is wrong: {error}");
    }
}

#[tokio::test]
async fn workspace_export_and_backup_refuse_relative_paths() {
    let mut client=connected_client().await;
    for method in ["workspace.export","workspace.backup"] {
        let error=client.call(method,json!({"path":"relativ.db"})).await.unwrap_err();
        assert_eq!(error["code"],-32602, "{method}");
        assert!(error["message"].as_str().unwrap().contains("absolute"), "{method}: {error}");
        assert!(client.call(method,json!({})).await.is_err(), "{method} needs a path");
    }
}

struct TestServer {
    address: std::net::SocketAddr,
    service: Arc<vatra_service::Service>,
    serving: tokio::task::JoinHandle<()>,
}

async fn start_server(script: Scripted) -> TestServer {
    let (service, _) = with_agent(script);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = router(service.clone(), TOKEN.into());
    let serving = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    TestServer { address, service, serving }
}

async fn start_with(service: Arc<vatra_service::Service>) -> TestServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = router(service.clone(), TOKEN.into());
    let serving = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    TestServer { address, service, serving }
}

async fn test_server() -> TestServer {
    start_server(Scripted::text("gata")).await
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn connect(server: &TestServer, token: Option<&str>, origin: Option<&str>) -> Result<Socket, u16> {
    let mut request = format!("ws://{}/rpc", server.address).into_client_request().unwrap();
    if let Some(token) = token {
        let protocol = format!("vatra.bearer.{token}");
        request.headers_mut().insert("sec-websocket-protocol", protocol.parse().unwrap());
    }
    if let Some(origin) = origin {
        request.headers_mut().insert("origin", origin.parse().unwrap());
    }
    match connect_async(request).await {
        Ok((socket, _)) => Ok(socket),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => Err(response.status().as_u16()),
        Err(other) => panic!("eroare neașteptată: {other}"),
    }
}

async fn post_approval(server: &TestServer, token: Option<&str>) -> u16 {
    let mut request = reqwest::Client::new()
        .post(format!("http://{}/internal/approval", server.address))
        .json(&json!({"session_id": 1, "tool_name": "x", "input": {}}));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    request.send().await.unwrap().status().as_u16()
}

struct Client {
    socket: Socket,
    next_id: i64,
    notifications: VecDeque<Value>,
}

impl Client {
    async fn read(&mut self) -> Value {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(10), self.socket.next()).await.unwrap().unwrap().unwrap();
            if let Message::Text(text) = message {
                return serde_json::from_str(text.as_str()).unwrap();
            }
        }
    }

    async fn raw(&mut self, text: &str) -> Value {
        self.socket.send(Message::Text(text.into())).await.unwrap();
        loop {
            let value = self.read().await;
            if value.get("method").is_some() {
                self.notifications.push_back(value);
            } else {
                return value;
            }
        }
    }

    async fn call_raw(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let request = json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params});
        let answer = self.raw(&request.to_string()).await;
        assert_eq!(answer["id"], self.next_id);
        answer
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        let mut answer = self.call_raw(method, params).await;
        match answer.get("error") {
            Some(error) => Err(error.clone()),
            None => Ok(answer["result"].take()),
        }
    }

    async fn next_notification(&mut self, method: &str) -> Value {
        loop {
            let value = match self.notifications.pop_front() {
                Some(value) => value,
                None => self.read().await,
            };
            if value["method"] == method {
                return value["params"].clone();
            }
        }
    }

    async fn create_session(&mut self) -> i64 {
        let project = self.call("projects.create", json!({"name": "proiect", "path": null})).await.unwrap();
        let session = self
            .call("sessions.create", json!({"project_id": project["id"], "agent": "scripted"}))
            .await
            .unwrap();
        session["id"].as_i64().unwrap()
    }

    async fn wait_until_stopped(&mut self, session: i64) {
        loop {
            let state = self.next_notification("turn_state").await;
            if state["session_id"] == session && state["running"] == false {
                return;
            }
        }
    }

    async fn session_with_finished_turn(&mut self) -> i64 {
        let session = self.create_session().await;
        let start = json!({"session_id": session, "prompt": "x", "mode": "full_access"});
        self.call("turn.start", start).await.unwrap();
        self.wait_until_stopped(session).await;
        session
    }
}

async fn client_for(server: &TestServer) -> Client {
    let socket = connect(server, Some(TOKEN), None).await.unwrap();
    Client { socket, next_id: 0, notifications: VecDeque::new() }
}

async fn connected_client() -> Client {
    client_for(&test_server().await).await
}

#[tokio::test]
async fn connection_without_a_valid_token_is_rejected() {
    let server = test_server().await;
    assert_eq!(connect(&server, None, None).await.err().unwrap(), 401);
    assert_eq!(connect(&server, Some("gresit"), None).await.err().unwrap(), 401);
}

#[tokio::test]
async fn connection_from_a_foreign_origin_is_rejected_even_with_a_token() {
    let server = test_server().await;
    assert_eq!(connect(&server, Some(TOKEN), Some("https://evil.example")).await.err().unwrap(), 403);
    assert_eq!(connect(&server, Some(TOKEN), Some("null")).await.err().unwrap(), 403);
}

#[tokio::test]
async fn the_app_origins_are_accepted() {
    let server = test_server().await;
    for origin in ["tauri://localhost", "http://tauri.localhost", "http://localhost:1420"] {
        assert!(connect(&server, Some(TOKEN), Some(origin)).await.is_ok(), "{origin}");
    }
}

#[tokio::test]
async fn internal_approval_requires_the_token() {
    let server = test_server().await;
    assert_eq!(post_approval(&server, None).await, 401);
    assert_eq!(post_approval(&server, Some("gresit")).await, 401);
}

#[tokio::test]
async fn turn_start_streams_events_as_notifications() {
    let mut client = connected_client().await;
    let session = client.create_session().await;
    let start = json!({"session_id": session, "prompt": "x", "mode": "full_access"});
    client.call("turn.start", start).await.unwrap();
    assert_eq!(client.next_notification("event").await["event"]["type"], "user_message");
}

#[tokio::test]
async fn events_after_seq_recovers_what_a_client_missed() {
    let mut client = connected_client().await;
    let session = client.session_with_finished_turn().await;
    let all = client.call("sessions.events", json!({"session_id": session, "after_seq": 0})).await.unwrap();
    let first = all[0]["seq"].as_i64().unwrap();
    let later = client.call("sessions.events", json!({"session_id": session, "after_seq": first})).await.unwrap();
    assert_eq!(later.as_array().unwrap().len(), all.as_array().unwrap().len() - 1);
}

#[tokio::test]
async fn bad_requests_get_json_rpc_errors_and_the_connection_survives() {
    let mut client = connected_client().await;
    assert_eq!(client.raw("nu e json").await["error"]["code"], -32700);
    assert_eq!(client.call_raw("nu.exista", json!({})).await["error"]["code"], -32601);
    assert_eq!(client.call_raw("turn.start", json!({"session_id": "text"})).await["error"]["code"], -32602);
    assert!(client.call("agents.list", json!({})).await.is_ok());
}

#[tokio::test]
async fn busy_turn_and_unknown_session_have_stable_error_codes() {
    let server = start_server(Scripted::blocking()).await;
    let mut client = client_for(&server).await;
    let session = client.create_session().await;
    let start = json!({"session_id": session, "prompt": "x", "mode": "full_access"});
    client.call("turn.start", start.clone()).await.unwrap();
    assert_eq!(client.call_raw("turn.start", start).await["error"]["code"], 1001);
    let unknown = json!({"session_id": 999, "prompt": "x", "mode": "full_access"});
    assert_eq!(client.call_raw("turn.start", unknown).await["error"]["code"], 1002);
    client.call("turn.stop", json!({"session_id": session})).await.unwrap();
}

#[tokio::test]
async fn sessions_list_reports_running_turns_after_a_reconnect() {
    let server = start_server(Scripted::blocking()).await;
    let mut first = client_for(&server).await;
    let session = first.create_session().await;
    first.call("turn.start", json!({"session_id": session, "prompt": "x", "mode": "full_access"})).await.unwrap();
    drop(first);
    let mut second = client_for(&server).await;
    let sessions = second.call("sessions.list", json!({})).await.unwrap();
    let found = sessions.as_array().unwrap().iter().find(|s| s["id"] == session).unwrap();
    assert_eq!(found["running"], true);
    server.service.stop_turn(session);
}

#[tokio::test]
async fn approvals_are_listed_after_a_reconnect_and_can_be_resolved() {
    let server = start_server(Scripted::asks_approval("Bash")).await;
    let mut first = client_for(&server).await;
    let session = first.create_session().await;
    first.call("turn.start", json!({"session_id": session, "prompt": "x", "mode": "supervised"})).await.unwrap();
    wait_for_approval(&server.service).await;
    drop(first);

    let mut second = client_for(&server).await;
    let pending = second.call("approvals.list", json!({})).await.unwrap();
    assert_eq!(pending[0]["tool"], "Bash");
    let resolve = json!({"id": pending[0]["id"], "allow": true});
    second.call("approvals.resolve", resolve.clone()).await.unwrap();
    assert_eq!(second.call_raw("approvals.resolve", resolve).await["error"]["code"], 1003);
}

#[tokio::test]
async fn questions_are_listed_after_a_reconnect_and_can_be_answered() {
    let server = start_server(Scripted::blocking()).await;
    let mut first = client_for(&server).await;
    let session = first.create_session().await;
    first.call("turn.start", json!({"session_id": session, "prompt": "x", "mode": "supervised"})).await.unwrap();
    let _answer = server.service.request_question(session, json!([{"question": "Continui?"}]));
    drop(first);

    let mut second = client_for(&server).await;
    let pending = second.call("questions.list", json!({})).await.unwrap();
    assert_eq!(pending[0]["questions"][0]["question"], "Continui?");
    let answer = json!({"id": pending[0]["id"], "answers": {"0": "da"}});
    second.call("question.answer", answer.clone()).await.unwrap();
    assert_eq!(second.call_raw("question.answer", answer).await["error"]["code"], 1004);
    server.service.stop_turn(session);
}

#[tokio::test]
async fn handoff_returns_the_new_session() {
    let mut client = connected_client().await;
    let session = client.session_with_finished_turn().await;
    let handoff = json!({"from": session, "to": "scripted", "task": "t", "mode": "full_access"});
    let result = client.call("handoff.start", handoff).await.unwrap();
    assert_ne!(result["session_id"], session);
}

#[tokio::test]
async fn a_session_for_an_unknown_agent_is_refused() {
    let mut client = connected_client().await;
    let project = client.call("projects.create", json!({"name": "p2"})).await.unwrap();
    let request = json!({"project_id": project["id"], "agent": "nimeni"});
    assert_eq!(client.call_raw("sessions.create", request).await["error"]["code"], -32602);
}

#[tokio::test]
async fn service_shutdown_is_requested_over_rpc() {
    let server = test_server().await;
    let mut client = client_for(&server).await;
    client.call("service.shutdown", json!({})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), server.service.shutdown_requested()).await.unwrap();
}

#[tokio::test]
async fn a_project_folder_that_does_not_exist_is_refused() {
    let mut client = connected_client().await;
    let request = json!({"name": "rupt", "path": "E:/nu/exista/deloc"});
    assert_eq!(client.call_raw("projects.create", request).await["error"]["code"], -32602);
}

/// The login frames reach the client: the address to open, then the result, and the status shows the login.
#[tokio::test]
async fn mcp_login_frames_reach_the_client() {
    let oauth = support::TestServer::start(true).await;
    let secrets = Arc::new(vatra_core::MemorySecretStore::default());
    let service = service_with(vec![support::oauth_server("remote", &oauth.url)], secrets);
    let server = start_with(service).await;
    let mut client = client_for(&server).await;

    client.call("mcp.login", json!({"server": "remote"})).await.unwrap();
    let opened = client.next_notification("open_url").await;
    let url = opened["url"].as_str().unwrap().to_string();
    assert!(url.starts_with(&format!("{}/authorize?", oauth.base())), "{url}");
    support::approve(Arc::clone(&oauth.shared), false)(&url);

    let outcome = client.next_notification("mcp_login").await;
    assert_eq!(outcome["server"], "remote");
    assert_eq!(outcome["ok"], true);
    let status = client.call("mcp.status", json!({})).await.unwrap();
    assert_eq!(status[0]["name"], "remote");
    assert_eq!(status[0]["logged_in"], true);
}

struct CatalogFactory {
    inner: ScriptedFactory,
    catalog: Arc<Catalog>,
    secrets: Arc<MemorySecretStore>,
}

#[async_trait::async_trait]
impl AgentFactory for CatalogFactory {
    fn names(&self) -> Vec<String> {
        self.inner.names()
    }

    async fn build(&self, request: BuildRequest) -> Result<Arc<dyn Agent>, String> {
        self.inner.build(request).await
    }

    fn summarizer(&self) -> Arc<dyn Summarizer> {
        self.inner.summarizer()
    }

    fn catalog(&self) -> Option<Arc<Catalog>> {
        Some(Arc::clone(&self.catalog))
    }

    fn secrets(&self) -> Arc<dyn SecretStore> {
        self.secrets.clone()
    }
}

struct CatalogServer {
    _dir: tempfile::TempDir,
    server: TestServer,
    catalog: Arc<Catalog>,
    secrets: Arc<MemorySecretStore>,
}

/// A service with two catalog entries: a stdio server and a remote one, both with literal secrets in their settings.
async fn catalog_server() -> CatalogServer {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Arc::new(Catalog::open(&dir.path().join("vatra.db"), dir.path()).unwrap());
    let local = McpServerConfig {
        name: "local".into(),
        command: Some("node".into()),
        args: vec!["server.js".into(), "--api-key".into(), "arg-secret-7".into()],
        env: [("API_KEY".to_string(), "env-secret-8".to_string())].into(),
        ..Default::default()
    };
    let remote = McpServerConfig {
        name: "remote".into(),
        url: Some("https://docs.example.com/mcp?token=query-secret-9".into()),
        headers: [("Authorization".to_string(), "Bearer header-secret-10".to_string())].into(),
        ..Default::default()
    };
    catalog.add_servers(&[local, remote], Scope::Global).unwrap();
    let secrets = Arc::new(MemorySecretStore::default());
    let factory = CatalogFactory {
        inner: ScriptedFactory(Scripted::text("gata")),
        catalog: Arc::clone(&catalog),
        secrets: Arc::clone(&secrets),
    };
    let store = Arc::new(Store::open_in_memory().unwrap());
    let service = Service::new(store, Arc::new(factory)).unwrap();
    CatalogServer { _dir: dir, server: start_with(service).await, catalog, secrets }
}

#[tokio::test]
async fn catalog_list_never_returns_secret_values() {
    let fixture = catalog_server().await;
    let mut client = client_for(&fixture.server).await;
    let id = fixture.catalog.list().unwrap()[0].id;
    client
        .call("catalog.set_secret", json!({"id": id, "key": "token", "value": "sk-live-secret-42"}))
        .await
        .unwrap();

    let listed = client.call("catalog.list", json!({})).await.unwrap();

    let text = listed.to_string();
    for secret in ["sk-live-secret-42", "arg-secret-7", "env-secret-8", "query-secret-9", "header-secret-10"] {
        assert!(!text.contains(secret), "{text}");
    }
    assert_eq!(listed[0]["name"], "local");
    assert_eq!(listed[1]["details"]["url"], "https://docs.example.com/mcp?<redacted>");
    assert_eq!(listed[0]["details"]["args"], json!(["server.js", "--api-key", "<redacted>"]));
    assert_eq!(listed[0]["details"]["env"], json!(["API_KEY"]));
}

#[tokio::test]
async fn set_secret_goes_to_store() {
    let fixture = catalog_server().await;
    let mut client = client_for(&fixture.server).await;
    let id = fixture.catalog.list().unwrap()[0].id;

    client
        .call("catalog.set_secret", json!({"id": id, "key": "token", "value": "sk-live-secret-42"}))
        .await
        .unwrap();

    let key = format!("extension:{id}:token");
    assert_eq!(fixture.secrets.get(&key).unwrap().as_deref(), Some("sk-live-secret-42"));
    let entry = fixture.catalog.list().unwrap().remove(0);
    assert_eq!(entry.details["secrets"]["token"]["keychain"], key);
}

/// A service with a project folder and one user-invocable skill and one hidden skill in the catalog.
async fn completion_server(folder: &std::path::Path) -> (tempfile::TempDir, TestServer) {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Arc::new(Catalog::open(&dir.path().join("vatra.db"), dir.path()).unwrap());
    let visible = folder.join("skills-src").join("resumat");
    std::fs::create_dir_all(&visible).unwrap();
    std::fs::write(visible.join("SKILL.md"), "---\nname: resumat\ndescription: Rezumă\n---\nCorp.\n").unwrap();
    let hidden = folder.join("skills-src").join("ascuns");
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(
        hidden.join("SKILL.md"),
        "---\nname: ascuns\ndescription: Nu se alege\nuser-invocable: false\n---\nCorp.\n",
    )
    .unwrap();
    catalog.add_folder(&visible, vatra_extensions::Origin::Manual, Scope::Global, false).unwrap();
    catalog.add_folder(&hidden, vatra_extensions::Origin::Manual, Scope::Global, false).unwrap();
    let factory = CatalogFactory {
        inner: ScriptedFactory(Scripted::text("gata")),
        catalog,
        secrets: Arc::new(MemorySecretStore::default()),
    };
    let store = Arc::new(Store::open_in_memory().unwrap());
    let service = Service::new(store, Arc::new(factory)).unwrap();
    (dir, start_with(service).await)
}

#[tokio::test]
async fn complete_slash_lists_user_invocable() {
    let folder = tempfile::tempdir().unwrap();
    let (_dir, server) = completion_server(folder.path()).await;
    let mut client = client_for(&server).await;
    let project = client
        .call("projects.create", json!({"name": "p", "path": folder.path().to_string_lossy()}))
        .await
        .unwrap();
    let items = client
        .call("composer.complete", json!({"project_id": project["id"], "trigger": "/", "query": ""}))
        .await
        .unwrap();
    let labels: Vec<&str> = items.as_array().unwrap().iter().map(|item| item["label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["resumat"]);
    assert_eq!(items[0]["insert"], "/resumat");
    assert_eq!(items[0]["kind"], "skill");
}

#[tokio::test]
async fn complete_at_respects_gitignore() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join(".gitignore"), "secret.txt\n").unwrap();
    std::fs::write(folder.path().join("secret.txt"), "x").unwrap();
    std::fs::write(folder.path().join("visibil.txt"), "x").unwrap();
    let (_dir, server) = completion_server(folder.path()).await;
    let mut client = client_for(&server).await;
    let project = client
        .call("projects.create", json!({"name": "p", "path": folder.path().to_string_lossy()}))
        .await
        .unwrap();
    let items = client
        .call("composer.complete", json!({"project_id": project["id"], "trigger": "@", "query": ""}))
        .await
        .unwrap();
    let labels: Vec<&str> = items.as_array().unwrap().iter().map(|item| item["label"].as_str().unwrap()).collect();
    assert!(labels.contains(&"visibil.txt"), "{labels:?}");
    assert!(!labels.iter().any(|label| label.contains("secret")), "{labels:?}");
}

#[tokio::test]
async fn complete_at_without_project_folder_is_empty() {
    let mut client = connected_client().await;
    let project = client.call("projects.create", json!({"name": "fara folder"})).await.unwrap();
    let items = client
        .call("composer.complete", json!({"project_id": project["id"], "trigger": "@", "query": ""}))
        .await
        .unwrap();
    assert_eq!(items, json!([]));
}

#[tokio::test]
async fn path_query_cannot_escape_project() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("afara.txt"), "x").unwrap();
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("inauntru.txt"), "x").unwrap();
    let (_dir, server) = completion_server(folder.path()).await;
    let mut client = client_for(&server).await;
    let project = client
        .call("projects.create", json!({"name": "p", "path": folder.path().to_string_lossy()}))
        .await
        .unwrap();
    let everything = client
        .call("composer.complete", json!({"project_id": project["id"], "trigger": "@", "query": ""}))
        .await
        .unwrap();
    assert!(everything.to_string().contains("inauntru.txt"), "{everything}");
    let escape = format!("../{}/afara", outside.path().file_name().unwrap().to_string_lossy());
    for query in [escape.as_str(), outside.path().to_string_lossy().as_ref()] {
        let items = client
            .call("composer.complete", json!({"project_id": project["id"], "trigger": "@", "query": query}))
            .await
            .unwrap();
        let text = items.to_string();
        assert!(!text.contains("afara"), "{text}");
    }
}

#[tokio::test]
async fn complete_dollar_lists_skills_only() {
    let folder = tempfile::tempdir().unwrap();
    let (_dir, server) = completion_server(folder.path()).await;
    let mut client = client_for(&server).await;
    let project = client
        .call("projects.create", json!({"name": "p", "path": folder.path().to_string_lossy()}))
        .await
        .unwrap();
    let items = client
        .call("composer.complete", json!({"project_id": project["id"], "trigger": "$", "query": ""}))
        .await
        .unwrap();
    assert_eq!(items.as_array().unwrap().len(), 1, "{items}");
    assert_eq!(items[0]["insert"], "$resumat");
    assert_eq!(items[0]["kind"], "skill");
}

#[tokio::test]
async fn complete_at_ignores_a_gitignore_above_the_project() {
    let outer = tempfile::tempdir().unwrap();
    std::fs::write(outer.path().join(".gitignore"), "visibil.txt\n").unwrap();
    let folder = outer.path().join("proiect");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("visibil.txt"), "x").unwrap();
    let (_dir, server) = completion_server(&folder).await;
    let mut client = client_for(&server).await;
    let project = client
        .call("projects.create", json!({"name": "p", "path": folder.to_string_lossy()}))
        .await
        .unwrap();
    let items = client
        .call("composer.complete", json!({"project_id": project["id"], "trigger": "@", "query": "visibil"}))
        .await
        .unwrap();
    assert_eq!(items[0]["label"], "visibil.txt", "{items}");
}

/// The fixture MCP server lives in vatra-engine, which this package does not build. The test builds it when the
/// folder of the test binary doesn't have it yet.
fn fixture_mcp_binary() -> std::path::PathBuf {
    let debug = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    let name = if cfg!(windows) { "vatra-fixture-mcp.exe" } else { "vatra-fixture-mcp" };
    let path = debug.join(name);
    if !path.exists() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = std::process::Command::new(cargo)
            .args(["build", "-p", "vatra-engine", "--bin", "vatra-fixture-mcp"])
            .status()
            .unwrap();
        assert!(status.success(), "the fixture MCP server couldn't be built");
    }
    path
}

/// A service whose project hubs run the given MCP servers, as a configuration would give them.
async fn hub_server(servers: Vec<McpServerConfig>, catalog: Option<Arc<Catalog>>) -> TestServer {
    let hub = McpHub::new(servers, ExpandVars::from_env(&std::env::temp_dir(), Vec::new()));
    let factory = HubFactory { inner: ScriptedFactory(Scripted::text("gata")), hub: Arc::new(hub), catalog };
    let store = Arc::new(Store::open_in_memory().unwrap());
    start_with(Service::new(store, Arc::new(factory)).unwrap()).await
}

struct HubFactory {
    inner: ScriptedFactory,
    hub: Arc<McpHub>,
    catalog: Option<Arc<Catalog>>,
}

#[async_trait::async_trait]
impl AgentFactory for HubFactory {
    fn names(&self) -> Vec<String> {
        self.inner.names()
    }

    async fn build(&self, request: BuildRequest) -> Result<Arc<dyn Agent>, String> {
        self.inner.build(request).await
    }

    fn summarizer(&self) -> Arc<dyn Summarizer> {
        self.inner.summarizer()
    }

    fn catalog(&self) -> Option<Arc<Catalog>> {
        self.catalog.clone()
    }

    fn mcp_hub(&self, _project_id: i64, _folder: Option<&std::path::Path>) -> Option<Arc<McpHub>> {
        Some(Arc::clone(&self.hub))
    }
}

/// Asks until the answer has what `ready` wants: the first request starts the servers, which take a moment.
async fn complete_until(
    client: &mut Client,
    project: i64,
    trigger: &str,
    query: &str,
    ready: impl Fn(&Value) -> bool,
) -> Value {
    for _ in 0..200 {
        let items = client
            .call("composer.complete", json!({"project_id": project, "trigger": trigger, "query": query}))
            .await
            .unwrap();
        if ready(&items) {
            return items;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the completion never listed what it should");
}

#[tokio::test]
async fn completion_lists_mcp_prompts_and_resources() {
    let fixture = McpServerConfig {
        name: "fixture".into(),
        command: Some(fixture_mcp_binary().to_string_lossy().into_owned()),
        ..Default::default()
    };
    let server = hub_server(vec![fixture], None).await;
    let mut client = client_for(&server).await;
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("lista.txt"), "x").unwrap();
    let bare = client.call("projects.create", json!({"name": "fara folder"})).await.unwrap();
    let with_folder = client
        .call("projects.create", json!({"name": "cu folder", "path": folder.path().to_string_lossy()}))
        .await
        .unwrap();
    let bare_id = bare["id"].as_i64().unwrap();
    let folder_id = with_folder["id"].as_i64().unwrap();

    let prompts = complete_until(&mut client, bare_id, "/", "greet", |items| !items.as_array().unwrap().is_empty()).await;
    assert_eq!(prompts[0]["insert"], "/mcp__fixture__greet");
    assert_eq!(prompts[0]["kind"], "prompt");

    // A project without a folder gets its MCP resources, but no files.
    let resources = complete_until(&mut client, bare_id, "@", "", |items| !items.as_array().unwrap().is_empty()).await;
    assert!(resources.as_array().unwrap().iter().all(|item| item["kind"] == "resource"), "{resources}");
    assert!(resources[0]["label"].as_str().unwrap().starts_with("fixture:"), "{resources}");

    // A project with a folder gets its files too.
    let files = complete_until(&mut client, folder_id, "@", "lista", |items| !items.as_array().unwrap().is_empty()).await;
    assert_eq!(files[0]["insert"], "@lista.txt");
    assert_eq!(files[0]["kind"], "file");
}

/// A server that never answers its handshake. It writes a pulse file while it runs, so the test can see whether it
/// is still alive after the service is gone.
const HUNG_SERVER: &str = "setInterval(() => require('fs').writeFileSync(process.env.VATRA_PULSE, String(Date.now())), 50)";

/// The pulse file's content, or None before the server has written it.
fn pulse(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// A server that never answers its handshake. Completion must answer at once, with what the rest offers, and when the
/// service goes away the server must go with it, with no minutes of waiting.
#[tokio::test]
async fn hung_mcp_server_doesnt_delay_completion() {
    let scratch = tempfile::tempdir().unwrap();
    let pulse_file = scratch.path().join("pulse.txt");
    let hung = McpServerConfig {
        name: "hung".into(),
        command: Some("node".into()),
        args: vec!["-e".into(), HUNG_SERVER.into()],
        env: [("VATRA_PULSE".to_string(), pulse_file.to_string_lossy().into_owned())].into(),
        ..Default::default()
    };
    let catalog_dir = tempfile::tempdir().unwrap();
    let catalog = Arc::new(Catalog::open(&catalog_dir.path().join("vatra.db"), catalog_dir.path()).unwrap());
    let folder = tempfile::tempdir().unwrap();
    let skill = folder.path().join("resumat");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "---\nname: resumat\ndescription: Rezuma\n---\nCorp.\n").unwrap();
    catalog.add_folder(&skill, vatra_extensions::Origin::Manual, Scope::Global, false).unwrap();
    let server = hub_server(vec![hung], Some(catalog)).await;
    let watch = Arc::downgrade(&server.service);
    let mut client = client_for(&server).await;
    let project = client.call("projects.create", json!({"name": "p"})).await.unwrap();

    for trigger in ["/", "@"] {
        let answer = tokio::time::timeout(
            Duration::from_secs(3),
            client.call("composer.complete", json!({"project_id": project["id"], "trigger": trigger, "query": ""})),
        )
        .await
        .expect("completion waited for a hung MCP server")
        .unwrap();
        assert!(answer.is_array(), "{answer}");
    }
    let skills = client
        .call("composer.complete", json!({"project_id": project["id"], "trigger": "/", "query": ""}))
        .await
        .unwrap();
    assert_eq!(skills[0]["insert"], "/resumat", "{skills}");

    // The warm-up has started the server: its pulse is there before the teardown begins.
    for _ in 0..100 {
        if pulse(&pulse_file).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(pulse(&pulse_file).is_some(), "the hung server never started");

    // Tear the service down: stop serving, close the connection, and let go of the last reference.
    let TestServer { service, serving, .. } = server;
    serving.abort();
    let _ = serving.await;
    drop(client);
    drop(service);
    for _ in 0..60 {
        if watch.upgrade().is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(watch.upgrade().is_none(), "the service is still alive after its teardown");

    // The child must be gone too: its pulse stops changing.
    let mut stopped = false;
    for _ in 0..40 {
        let before = pulse(&pulse_file);
        tokio::time::sleep(Duration::from_millis(150)).await;
        if pulse(&pulse_file) == before {
            stopped = true;
            break;
        }
    }
    assert!(stopped, "the hung MCP server is still running after its service was dropped");
}

#[tokio::test]
async fn connectors_routes_answer_with_an_empty_registry() {
    let server = test_server().await;
    let mut client = client_for(&server).await;
    assert_eq!(client.call("connectors.kinds", json!({})).await.unwrap(), json!([]));
    assert_eq!(client.call("connectors.list", json!({})).await.unwrap(), json!([]));
    assert!(client.call("connectors.save", json!({"kind": "imap", "name": "Mail", "settings": {}})).await.is_err());
    assert!(client.call("connectors.check", json!({"id": "missing"})).await.is_err());
    client.call("connectors.remove", json!({"id": "missing"})).await.unwrap();
}

#[tokio::test]
async fn workspace_rpc_rejects_null_kind_filters() {
    let server = test_server().await;
    let mut client = client_for(&server).await;
    assert!(client.call("workspace.search",json!({"query":"report","kinds":null})).await.is_err());
    assert!(client.call("workspace.search",json!({"query":"report","kinds":[12]})).await.is_err());
    assert!(client.call("workspace.search",json!({"query":"report","kinds":["unknown"]})).await.is_err());
    assert!(client.call("workspace.search",json!({"query":"report"})).await.unwrap().is_array());
}

#[tokio::test]
async fn deleting_a_conversation_removes_it_and_its_events() {
    let mut client = connected_client().await;
    let session = client.session_with_finished_turn().await;
    let other = client.create_session().await;
    client.call("sessions.delete", json!({"session_id": session})).await.unwrap();
    let left = client.call("sessions.list", json!({})).await.unwrap();
    let ids: Vec<i64> = left.as_array().unwrap().iter().map(|item| item["id"].as_i64().unwrap()).collect();
    assert!(ids.contains(&other) && !ids.contains(&session), "{ids:?}");
    let error = client.call("sessions.delete", json!({"session_id": session})).await.unwrap_err();
    assert_eq!(error["code"], 1002, "a second delete finds nothing");
}

#[tokio::test]
async fn picker_options_survive_reconnect_and_reach_turn_and_handoff() {
    #[derive(Default)] struct Capture(std::sync::Mutex<Vec<vatra_service::TurnOptions>>);
    #[async_trait::async_trait]
    impl AgentFactory for Capture {
        fn names(&self)->Vec<String>{ vec!["scripted".into()] }
        async fn build(&self, request: BuildRequest)->Result<Arc<dyn Agent>,String>{
            self.0.lock().unwrap().push(request.options.clone());
            ScriptedFactory(Scripted::text("ok")).build(request).await
        }
        fn summarizer(&self)->Arc<dyn Summarizer>{ ScriptedFactory(Scripted::text("ok")).summarizer() }
    }
    let factory=Arc::new(Capture::default());
    let store=Arc::new(Store::open_in_memory().unwrap());
    let service=Service::new(store,factory.clone()).unwrap();
    let server=start_with(service).await;
    let mut client=client_for(&server).await;
    let id=client.create_session().await;
    client.call("sessions.options",json!({"session_id":id,"model":"picked/model","effort":"high"})).await.unwrap();
    let mut reconnected=client_for(&server).await;
    let sessions=reconnected.call("sessions.list",json!({})).await.unwrap();
    assert_eq!(sessions[0]["options"],json!({"model":"picked/model","effort":"high"}));
    client.call("turn.start",json!({"session_id":id,"prompt":"x","mode":"supervised","model":"picked/model","effort":"high"})).await.unwrap();
    client.wait_until_stopped(id).await;
    assert_eq!(factory.0.lock().unwrap()[0].model.as_deref(),Some("picked/model"));
    assert_eq!(factory.0.lock().unwrap()[0].effort.as_deref(),Some("high"));
    let handoff=client.call("handoff.start",json!({"from":id,"to":"scripted","task":"continue","mode":"supervised","model":"second","effort":"low"})).await.unwrap();
    client.wait_until_stopped(handoff["session_id"].as_i64().unwrap()).await;
    assert_eq!(factory.0.lock().unwrap().last().unwrap().model.as_deref(),Some("second"));
    client.call("sessions.options",json!({"session_id":id})).await.unwrap();
    let sessions=reconnected.call("sessions.list",json!({})).await.unwrap();
    assert_eq!(sessions.as_array().unwrap().iter().find(|s|s["id"]==id).unwrap()["options"],json!({}));
    assert!(client.call("sessions.options",json!({"session_id":id,"model":"bad\nmodel"})).await.is_err());
}
#[tokio::test]
async fn file_rpc_upload_preview_versions_and_session_isolation() {
    let server=test_server().await;let mut client=client_for(&server).await;
    let a=server.service.store().create_session(None,"scripted").unwrap();
    let b=server.service.store().create_session(None,"scripted").unwrap();
    let begin=client.call("files.upload.begin",json!({"session_id":a,"size":5})).await.unwrap();
    let upload=begin["upload_id"].as_str().unwrap();
    client.call("files.upload.chunk",json!({"session_id":a,"upload_id":upload,"offset":0,"base64":"aGVsbG8="})).await.unwrap();
    let file=client.call("files.upload.finish",json!({"session_id":a,"upload_id":upload,"name":"hello.md","text":"hello"})).await.unwrap();
    let id=file["id"].as_i64().unwrap();
    assert_eq!(file["media_type"],"text/markdown");
    assert!(client.call("files.read",json!({"session_id":b,"id":id})).await.is_err());
    assert_eq!(client.call("files.text",json!({"session_id":a,"id":id})).await.unwrap()["text"],"hello");
    client.call("files.save",json!({"session_id":a,"id":id,"expected_revision":1,"text":"updated"})).await.unwrap();
    assert!(client.call("files.save",json!({"session_id":a,"id":id,"expected_revision":1,"text":"stale"})).await.is_err());
    assert_eq!(client.call("files.versions",json!({"session_id":a,"id":id})).await.unwrap().as_array().unwrap().len(),2);
    assert_eq!(client.call("files.read",json!({"session_id":a,"id":id,"revision":1})).await.unwrap()["base64"],"aGVsbG8=");
    client.call("files.remove",json!({"session_id":a,"id":id})).await.unwrap();
    assert!(client.call("files.list",json!({"session_id":a})).await.unwrap().as_array().unwrap().is_empty());
}

#[tokio::test]
async fn file_rpc_rejects_disguised_content_and_oversized_chunks() {
    let server=test_server().await;let mut client=client_for(&server).await;
    let session=server.service.store().create_session(None,"scripted").unwrap();
    let upload=client.call("files.upload.begin",json!({"session_id":session,"size":5})).await.unwrap();
    let id=&upload["upload_id"];
    client.call("files.upload.chunk",json!({"session_id":session,"upload_id":id,"offset":0,"base64":"aGVsbG8="})).await.unwrap();
    assert!(client.call("files.upload.finish",json!({"session_id":session,"upload_id":id,"name":"fake.pdf","text":"hello"})).await.is_err());
    assert!(client.call("files.upload.begin",json!({"session_id":session,"size":999999999})).await.is_err());
}


#[tokio::test]
async fn a_project_is_renamed_and_deleted_with_its_conversations_over_rpc() {
    let (service, session) = with_agent(Scripted::blocking());
    let server = start_with(service.clone()).await;
    let mut client = client_for(&server).await;
    let project = service.store().project_sessions(1).unwrap();
    assert!(project.contains(&session));
    // A running turn keeps the project from being removed.
    service.start_turn(session, "a".into(), vatra_service::PermissionMode::FullAccess).unwrap();
    assert!(client.call("projects.delete", json!({"id": 1})).await.is_err());
    service.stop_turn(session);
    for _ in 0..50 {
        if !service.is_running(session) { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    client.call("projects.rename", json!({"id": 1, "name": "Casa"})).await.unwrap();
    assert_eq!(client.call("projects.list", json!({})).await.unwrap()[0]["name"], "Casa");
    let removal = client.call("projects.delete", json!({"id": 1})).await.unwrap();
    assert_eq!(removal["conversations"], 1);
    assert_eq!(client.call("projects.list", json!({})).await.unwrap(), json!([]));
    assert!(client.call("projects.delete", json!({"id": 1})).await.is_err());
}
