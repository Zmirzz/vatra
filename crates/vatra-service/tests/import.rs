mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{Scripted, ScriptedFactory};
use serde_json::{Value, json};
use vatra_core::{Agent, MemorySecretStore, SecretStore, Store, Summarizer};
use vatra_extensions::{Catalog, ElementKind};
use vatra_import::{ImportChoice, ItemChoice, SourceDirs};
use vatra_service::{AgentFactory, BuildRequest, ConfigAgents, Notification, Service};

const MARKER: &str = "SECRET-MARKER-import";

/// The scripted factory of the other tests, with a catalog and a secret store of the service's own.
struct ImportFactory {
    inner: ScriptedFactory,
    catalog: Arc<Catalog>,
    secrets: Arc<MemorySecretStore>,
}

#[async_trait::async_trait]
impl AgentFactory for ImportFactory {
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

struct Fixture {
    _tmp: tempfile::TempDir,
    service: Arc<Service>,
    catalog: Arc<Catalog>,
    secrets: Arc<MemorySecretStore>,
}

/// A HOME with one personal skill, one MCP server whose header holds a literal secret, and one permission rule, and
/// a service that reads that HOME. Its one project has a folder.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let data = tmp.path().join("data");
    let project_folder = tmp.path().join("project");
    fs::create_dir_all(&project_folder).unwrap();
    write(
        &home
            .join(".claude")
            .join("skills")
            .join("personal")
            .join("SKILL.md"),
        "---\ndescription: Mine\n---\nMine.\n",
    );
    write(
        &home.join(".claude").join("settings.json"),
        &json!({ "permissions": { "deny": ["Bash(rm -rf *)"] } }).to_string(),
    );
    write(
        &home.join(".claude.json"),
        &json!({
            "mcpServers": {
                "global": {
                    "type": "http",
                    "url": "https://mcp.example.test/mcp",
                    "headers": { "Authorization": format!("Bearer {MARKER}") }
                }
            }
        })
        .to_string(),
    );
    fs::create_dir_all(&data).unwrap();
    let catalog = Arc::new(Catalog::open(&data.join("vatra.db"), &data).unwrap());
    let secrets = Arc::new(MemorySecretStore::default());
    let factory = ImportFactory {
        inner: ScriptedFactory(Scripted::text("gata")),
        catalog: Arc::clone(&catalog),
        secrets: Arc::clone(&secrets),
    };
    let store = Arc::new(Store::open_in_memory().unwrap());
    let project = store.create_project("p").unwrap();
    store
        .set_project_path(project, Some(&project_folder.display().to_string()))
        .unwrap();
    let dirs = SourceDirs {
        claude: home.join(".claude"),
        codex: home.join(".codex"),
        agents: home.join(".claude").join("agents"),
        home: home.clone(),
    };
    let service = Service::with_source_dirs(store, Arc::new(factory), dirs).unwrap();
    Fixture {
        _tmp: tmp,
        service,
        catalog,
        secrets,
    }
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// The key of the item named `name` in a scan view, as the app reads it.
fn key_of(view: &Value, name: &str) -> String {
    view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["report"]["name"] == name)
        .unwrap_or_else(|| panic!("no {name} in the view: {view}"))["key"]
        .as_str()
        .unwrap()
        .to_string()
}

fn chosen(key: String) -> ItemChoice {
    ItemChoice {
        key,
        include: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn scan_view_has_no_secret_values() {
    let fixture = fixture();

    let view = fixture.service.import_scan().await.unwrap();

    let text = serde_json::to_string(&view).unwrap();
    assert!(!text.contains(MARKER), "{text}");
    assert!(text.contains("global"));
}

#[tokio::test]
async fn apply_stores_the_secret_only_in_the_keychain_and_reports_progress() {
    let fixture = fixture();
    let mut updates = fixture.service.subscribe();
    let view = fixture.service.import_scan().await.unwrap();
    let mut server = chosen(key_of(&serde_json::to_value(&view).unwrap(), "global"));
    server.secrets.insert("Authorization".into(), true);
    let choice = ImportChoice {
        items: vec![
            server,
            chosen(key_of(&serde_json::to_value(&view).unwrap(), "personal")),
        ],
        ..Default::default()
    };

    let outcome = fixture.service.import_apply(choice).await.unwrap();

    assert_eq!(outcome.results.len(), 2);
    let stored: Vec<String> = fixture.secrets.keys();
    assert_eq!(stored.len(), 1);
    assert_eq!(
        fixture.secrets.get(&stored[0]).unwrap().as_deref(),
        Some(format!("Bearer {MARKER}").as_str())
    );
    let server = fixture
        .catalog
        .list()
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == ElementKind::McpServer && entry.name == "global")
        .unwrap();
    assert!(server.enabled);

    let mut events = Vec::new();
    while let Ok(event) = updates.try_recv() {
        events.push(event);
    }
    let progress: Vec<(usize, usize)> = events
        .iter()
        .filter_map(|event| match event {
            Notification::ImportProgress { done, total, .. } => Some((*done, *total)),
            _ => None,
        })
        .collect();
    assert!(progress.contains(&(2, 2)), "{progress:?}");
    let sent = serde_json::to_string(
        &events
            .iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(!sent.contains(MARKER));
}

#[tokio::test]
async fn import_report_keeps_the_last_apply() {
    let fixture = fixture();
    assert!(fixture.service.import_report().is_none());
    let view = serde_json::to_value(fixture.service.import_scan().await.unwrap()).unwrap();
    let choice = ImportChoice {
        items: vec![chosen(key_of(&view, "personal"))],
        ..Default::default()
    };

    fixture.service.import_apply(choice).await.unwrap();

    let report = serde_json::to_value(fixture.service.import_report().unwrap()).unwrap();
    assert_eq!(report["results"][0]["name"], "personal");
    assert_eq!(report["results"][0]["state"]["state"], "added");
}

#[tokio::test]
async fn second_scan_marks_unchanged_items_as_same() {
    let fixture = fixture();
    let view = serde_json::to_value(fixture.service.import_scan().await.unwrap()).unwrap();
    let choice = ImportChoice {
        items: vec![chosen(key_of(&view, "personal"))],
        ..Default::default()
    };
    fixture.service.import_apply(choice).await.unwrap();

    let again = serde_json::to_value(fixture.service.import_scan().await.unwrap()).unwrap();

    let personal = again["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["report"]["name"] == "personal")
        .unwrap();
    assert_eq!(personal["status"], "unchanged");
}

fn server_names(catalog: &Catalog) -> Vec<String> {
    let mut names: Vec<String> = catalog
        .list()
        .unwrap()
        .into_iter()
        .filter(|entry| entry.kind == ElementKind::McpServer)
        .map(|entry| entry.name)
        .collect();
    names.sort();
    names
}

#[test]
fn config_servers_removed_from_vatra_toml_leave_the_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = Arc::new(Catalog::open(&tmp.path().join("vatra.db"), tmp.path()).unwrap());
    let both = vatra_service::config::Config::parse(
        "summarizer = \"api\"\n[agents.api]\nkind = \"model\"\nbase_url = \"http://x\"\nmodel = \"m\"\n\
         [[mcp]]\nname = \"keep\"\ncommand = \"node\"\n[[mcp]]\nname = \"gone\"\nurl = \"https://x.example/mcp\"\n",
    )
    .unwrap();
    let _first = ConfigAgents::new(both).with_catalog_synced(Arc::clone(&catalog));
    assert_eq!(
        server_names(&catalog),
        vec!["gone".to_string(), "keep".to_string()]
    );

    let only_keep = vatra_service::config::Config::parse(
        "summarizer = \"api\"\n[agents.api]\nkind = \"model\"\nbase_url = \"http://x\"\nmodel = \"m\"\n\
         [[mcp]]\nname = \"keep\"\ncommand = \"node\"\n",
    )
    .unwrap();
    let _second = ConfigAgents::new(only_keep).with_catalog_synced(Arc::clone(&catalog));

    assert_eq!(server_names(&catalog), vec!["keep".to_string()]);
}

#[test]
fn catalog_change_drops_only_that_projects_hub() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = Arc::new(Catalog::open(&tmp.path().join("vatra.db"), tmp.path()).unwrap());
    let agents = ConfigAgents::new(vatra_service::config::Config::empty())
        .with_catalog(Arc::clone(&catalog));
    let folder = tmp.path().to_path_buf();
    let running = agents.mcp_hub(1, Some(&folder)).unwrap();
    let other = agents.mcp_hub(2, Some(&folder)).unwrap();

    agents.catalog_changed(Some(1));

    let next = agents.mcp_hub(1, Some(&folder)).unwrap();
    assert!(!Arc::ptr_eq(&running, &next), "project 1 gets a new hub");
    assert!(
        Arc::ptr_eq(&other, &agents.mcp_hub(2, Some(&folder)).unwrap()),
        "project 2 keeps its hub"
    );
}

/// A turn's model: each answer goes to one request in order, then every request gets a plain reply.
async fn model(answers: Vec<Value>) -> wiremock::MockServer {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    for answer in answers {
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"choices": [{"message": answer}]})),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"choices": [{"message": {"role": "assistant", "content": "done"}}]}),
        ))
        .mount(&server)
        .await;
    server
}

/// A service whose agents are the configuration's own, on a model served by a mock, with a HOME that has one personal
/// skill and the given deny rules in its settings.
struct TurnFixture {
    _tmp: tempfile::TempDir,
    store: Arc<Store>,
    service: Arc<Service>,
    agents: Arc<ConfigAgents>,
    server: wiremock::MockServer,
    home: PathBuf,
    folder: PathBuf,
    project: i64,
}

async fn turn_fixture(deny: &[&str], answers: impl FnOnce(&Path) -> Vec<Value>) -> TurnFixture {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let folder = tmp.path().join("work");
    fs::create_dir_all(&folder).unwrap();
    let server = model(answers(&folder)).await;
    write(
        &home
            .join(".claude")
            .join("skills")
            .join("personal")
            .join("SKILL.md"),
        "---\ndescription: Mine\n---\nMine.\n",
    );
    if !deny.is_empty() {
        write(
            &home.join(".claude").join("settings.json"),
            &json!({ "permissions": { "deny": deny } }).to_string(),
        );
    }
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let catalog = Arc::new(Catalog::open(&data.join("vatra.db"), &data).unwrap());
    let config = vatra_service::config::Config::parse(&format!(
        "summarizer = \"api\"\n[agents.api]\nkind = \"model\"\nbase_url = \"{}\"\nmodel = \"m\"\n",
        server.uri()
    ))
    .unwrap();
    let agents = Arc::new(ConfigAgents::new(config).with_catalog(catalog));
    let store = Arc::new(Store::open_in_memory().unwrap());
    let project = store.create_project("p").unwrap();
    let dirs = SourceDirs {
        claude: home.join(".claude"),
        codex: home.join(".codex"),
        agents: home.join(".claude").join("agents"),
        home: home.clone(),
    };
    let service = Service::with_source_dirs(Arc::clone(&store), agents.clone(), dirs).unwrap();
    TurnFixture {
        _tmp: tmp,
        store,
        service,
        agents,
        server,
        home,
        folder,
        project,
    }
}

impl TurnFixture {
    /// Imports the items named `names`, each as the scan reads it now.
    async fn import(&self, names: &[&str]) {
        let view = serde_json::to_value(self.service.import_scan().await.unwrap()).unwrap();
        let items = names
            .iter()
            .map(|name| chosen(key_of(&view, name)))
            .collect();
        self.service
            .import_apply(ImportChoice {
                items,
                ..Default::default()
            })
            .await
            .unwrap();
    }

    /// A session, with an agent built now, which holds the catalog as it is at this moment.
    async fn session(&self) -> (i64, Arc<dyn Agent>) {
        let session = self.store.create_session(self.project, "api").unwrap();
        let agent = self
            .agents
            .build(BuildRequest {
                name: "api".into(),
                session_id: session,
                project_id: self.project,
                approver: Arc::new(vatra_engine::AllowAll),
                questioner: Arc::new(vatra_engine::NoQuestions),
                mode: vatra_core::PermissionMode::Supervised,
                cwd: Some(self.folder.clone()),
                options: Default::default(),
            })
            .await
            .unwrap();
        (session, agent)
    }

    async fn turn(&self, session: i64, agent: &dyn Agent) {
        vatra_core::run_turn(
            &self.store,
            agent,
            session,
            "hi",
            &mut |_: &vatra_core::StoredEvent| {},
        )
        .await
        .unwrap();
    }

    /// Every model request (the POSTs), as text, in order.
    async fn requests(&self) -> Vec<String> {
        self.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| request.method == wiremock::http::Method::POST)
            .map(|request| String::from_utf8_lossy(&request.body).into_owned())
            .collect()
    }
}

fn personal_skill(home: &Path, description: &str) {
    write(
        &home
            .join(".claude")
            .join("skills")
            .join("personal")
            .join("SKILL.md"),
        &format!("---\ndescription: {description}\n---\n{description} body.\n"),
    );
}

#[tokio::test]
async fn running_turn_keeps_old_version() {
    let fixture = turn_fixture(&[], |_| vec![]).await;
    personal_skill(&fixture.home, "Mine");
    fixture.import(&["personal"]).await;
    let (session, running) = fixture.session().await;

    personal_skill(&fixture.home, "Newer");
    fixture.import(&["personal"]).await;
    fixture.turn(session, running.as_ref()).await;

    let requests = fixture.requests().await;
    assert!(
        requests[0].contains("Mine") && !requests[0].contains("Newer"),
        "{} {:?}",
        requests.len(),
        requests.iter().map(|r| r.len()).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn next_turn_uses_new_version() {
    let fixture = turn_fixture(&[], |_| vec![]).await;
    personal_skill(&fixture.home, "Mine");
    fixture.import(&["personal"]).await;
    personal_skill(&fixture.home, "Newer");
    fixture.import(&["personal"]).await;
    let (session, next) = fixture.session().await;

    fixture.turn(session, next.as_ref()).await;

    let requests = fixture.requests().await;
    assert!(
        requests.last().unwrap().contains("Newer"),
        "{}",
        requests.last().unwrap()
    );
}

fn write_call(path: &Path) -> Value {
    json!({"role": "assistant", "content": null, "tool_calls": [{
        "id": "w1", "type": "function",
        "function": {"name": "Write", "arguments": json!({"file_path": path, "content": "x"}).to_string()},
    }]})
}

#[tokio::test]
async fn imported_deny_refuses_a_real_tool_call() {
    let fixture = turn_fixture(&["Write"], |folder| {
        vec![write_call(&folder.join("out.txt"))]
    })
    .await;
    fixture.import(&["permissions:settings.json"]).await;
    let (session, agent) = fixture.session().await;

    fixture.turn(session, agent.as_ref()).await;

    assert!(
        !fixture.folder.join("out.txt").exists(),
        "the denied write happened"
    );
    assert_eq!(
        fixture.requests().await.len(),
        2,
        "the tool call should reach the model's second request"
    );
}

#[tokio::test]
async fn without_the_rule_the_same_call_writes() {
    let fixture = turn_fixture(&[], |folder| vec![write_call(&folder.join("out.txt"))]).await;
    let (session, agent) = fixture.session().await;

    fixture.turn(session, agent.as_ref()).await;

    assert!(
        fixture.folder.join("out.txt").exists(),
        "the write should happen without the rule"
    );
}

#[tokio::test]
async fn global_instructions_reach_engine_and_can_be_disabled() {
    let fixture = turn_fixture(&[], |_| vec![]).await;
    write(
        &fixture.home.join(".claude/CLAUDE.md"),
        "IMPORTED-GLOBAL-INSTRUCTION: keep the database local.",
    );
    fixture.import(&["CLAUDE.md"]).await;
    let (session, agent) = fixture.session().await;
    fixture.turn(session, agent.as_ref()).await;
    assert!(
        fixture
            .requests()
            .await
            .last()
            .unwrap()
            .contains("IMPORTED-GLOBAL-INSTRUCTION")
    );
    let catalog = fixture.agents.catalog().unwrap();
    let entry = catalog
        .list()
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == ElementKind::Instructions)
        .unwrap();
    catalog.set_enabled(entry.id, false).unwrap();
    let (session, agent) = fixture.session().await;
    fixture.turn(session, agent.as_ref()).await;
    assert!(
        !fixture
            .requests()
            .await
            .last()
            .unwrap()
            .contains("IMPORTED-GLOBAL-INSTRUCTION")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_reaches_a_running_import_over_its_own_connection() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

    const LOGS: usize = 3000;
    const TOKEN: &str = "token-de-test-0123456789abcdef0123";
    let f = fixture();
    let logs = f._tmp.path().join("home").join(".claude").join("projects").join("p");
    for index in 0..LOGS {
        let line = json!({"type":"user","sessionId":format!("s{index}"),"message":{"role":"user","content":"salut"}});
        write(&logs.join(format!("{index}.jsonl")), &format!("{line}\n"));
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = vatra_service::router(f.service.clone(), TOKEN.into());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut request = format!("ws://{address}/rpc").into_client_request().unwrap();
    request
        .headers_mut()
        .insert("sec-websocket-protocol", format!("vatra.bearer.{TOKEN}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let apply = json!({"jsonrpc":"2.0","id":1,"method":"import.apply","params":{"items":[],"projects":[],"include_history":true}});
    socket.send(Message::Text(apply.to_string().into())).await.unwrap();

    let mut answers = vec![];
    let mut asked = false;
    let result = loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(120), socket.next())
            .await
            .expect("the import must answer")
            .unwrap()
            .unwrap();
        let Message::Text(text) = frame else { continue };
        let value: Value = serde_json::from_str(text.as_str()).unwrap();
        if value["method"] == "import_progress" && !asked {
            asked = true;
            let cancel = json!({"jsonrpc":"2.0","id":2,"method":"import.cancel","params":{}});
            socket.send(Message::Text(cancel.to_string().into())).await.unwrap();
        }
        if let Some(id) = value["id"].as_i64() {
            answers.push(id);
            if id == 1 {
                break value;
            }
        }
    };
    assert_eq!(answers, vec![2, 1], "the cancel must be answered while the import runs");
    let imported = result["result"]["results"].as_array().unwrap().len();
    assert!(imported < LOGS, "the import must stop early, it did {imported}");
}

#[tokio::test]
async fn chosen_folders_bring_their_conversations_and_a_project() {
    let f = fixture();
    let home = f._tmp.path().join("home");
    let cwd = f._tmp.path().join("work").join("blog");
    write(
        &home.join(".claude").join("projects").join("x").join("one.jsonl"),
        &format!(
            "{}\n",
            json!({"type":"user","sessionId":"s1","cwd":cwd.display().to_string(),"timestamp":"2026-10-01T10:00:00Z","message":{"role":"user","content":"hello"}})
        ),
    );
    let listed = f.service.import_history_projects().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(!listed[0].known);

    let outcome = f
        .service
        .import_apply(ImportChoice { history_projects: vec![listed[0].key.clone()], ..Default::default() })
        .await
        .unwrap();

    assert_eq!(outcome.results.len(), 1, "{outcome:?}");
    let after = f.service.import_history_projects().await.unwrap();
    assert!(after[0].known, "the folder became a project");
}

#[tokio::test]
async fn the_market_adds_a_connection_and_keeps_its_token_out_of_the_catalog() {
    let f = fixture();
    let listed = vatra_service::market::list(&f.service).unwrap();
    assert!(listed.as_array().unwrap().iter().any(|item| item["id"] == "github" && item["needs"] == "token"));

    let missing = vatra_service::market::install(&f.service, json!({"id": "github"}));
    assert!(missing.is_err(), "a token is required");

    vatra_service::market::install(&f.service, json!({"id": "github", "token": "ghp_TOKEN-MARKER"})).unwrap();
    vatra_service::market::install(&f.service, json!({"id": "context7"})).unwrap();
    vatra_service::market::install(&f.service, json!({"id": "playwright"})).unwrap();

    let entries = f.catalog.list().unwrap();
    let github = entries.iter().find(|entry| entry.name == "github").unwrap();
    assert!(github.enabled, "a connection with its token is on");
    assert!(!github.details.to_string().contains("ghp_TOKEN-MARKER"), "the token stays in the keychain");
    assert!(f.secrets.keys().iter().any(|key| f.secrets.get(key).unwrap().as_deref() == Some("Bearer ghp_TOKEN-MARKER")));
    let playwright = entries.iter().find(|entry| entry.name == "playwright").unwrap();
    assert!(f.catalog.is_approved(playwright), "a program of the list needs no second approval");
    let after = vatra_service::market::list(&f.service).unwrap();
    assert!(after.as_array().unwrap().iter().filter(|item| item["connected"] == true).count() >= 3);
    assert!(vatra_service::market::install(&f.service, json!({"id": "nope"})).is_err());
}
