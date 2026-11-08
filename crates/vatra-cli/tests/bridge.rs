use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::TokioChildProcess;
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use vatra_core::{
    Agent, AgentError, BriefFields, Event, Store, Summarizer, TurnOutcome, TurnRequest,
};
use vatra_service::{AgentFactory, BuildRequest, PermissionMode, Service, internal_router};

const TOKEN: &str = "secret-token-pentru-test-0123456789";

struct Waiting;

#[async_trait]
impl Agent for Waiting {
    fn name(&self) -> &str {
        "waiting"
    }

    async fn run_turn(
        &self,
        _: TurnRequest,
        _: UnboundedSender<Event>,
    ) -> Result<TurnOutcome, AgentError> {
        tokio::time::sleep(Duration::from_secs(600)).await;
        Ok(TurnOutcome::default())
    }
}

struct Factory;

#[async_trait]
impl AgentFactory for Factory {
    fn names(&self) -> Vec<String> {
        vec!["waiting".into()]
    }

    async fn build(&self, _: BuildRequest) -> Result<Arc<dyn Agent>, String> {
        Ok(Arc::new(Waiting))
    }

    fn summarizer(&self) -> Arc<dyn Summarizer> {
        struct None_;
        #[async_trait]
        impl Summarizer for None_ {
            async fn summarize(&self, _: &str) -> Result<BriefFields, AgentError> {
                Err(AgentError::Failed("x".into()))
            }
        }
        Arc::new(None_)
    }
}

struct Setup {
    service: Arc<Service>,
    session: i64,
    url: String,
    token_file: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

async fn setup() -> Setup {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let project = store.create_project("p").unwrap();
    let session = store.create_session(project, "waiting").unwrap();
    let service = Service::new(store, Arc::new(Factory)).unwrap();
    service
        .start_turn(session, "x".into(), PermissionMode::Supervised)
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = internal_router(service.clone(), TOKEN.into());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let dir = tempfile::tempdir().unwrap();
    let token_file = dir.path().join("token");
    std::fs::write(&token_file, TOKEN).unwrap();
    Setup {
        service,
        session,
        url,
        token_file,
        _dir: dir,
    }
}

async fn call_approve(url: &str, token_file: &std::path::Path, session: i64) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vatra"));
    command.args(["approval-bridge", "--url", url, "--token-file"]);
    command
        .arg(token_file)
        .args(["--session", &session.to_string()]);
    let client = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    let arguments = json!({"tool_name": "Bash", "input": {"command": "cargo test"}});
    let request = CallToolRequestParams::new("approve")
        .with_arguments(arguments.as_object().unwrap().clone());
    let result = client.call_tool(request).await.unwrap();
    let text = result.content[0].as_text().unwrap().text.clone();
    serde_json::from_str(&text).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_tools_cross_the_real_stdio_bridge_without_applying_changes() {
    // Local protocol contract; no Claude/Codex subscription is started.
    let setup=setup().await;let store=setup.service.store();let project=store.session_project(setup.session).unwrap().unwrap();
    let id=store.create_item(&vatra_core::NewItem {kind:"note".into(),title:"Note".into(),body:"original".into(),fields:json!({}),project_id:Some(project)},1).unwrap();
    let scope=store.capture_workspace_context(setup.session,project,id,1,None).unwrap();store.activate_workspace_context(&scope).unwrap();
    let mut command=Command::new(env!("CARGO_BIN_EXE_vatra"));
    command.args(["approval-bridge","--url",&setup.url,"--token-file"]).arg(&setup.token_file).args(["--session",&setup.session.to_string(),"--workspace-scope",&scope.scope_id]);
    let client=().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    let tools=client.list_all_tools().await.unwrap();
    for name in ["read_workspace_item","propose_workspace_edit","propose_workspace_tasks"] {assert!(tools.iter().any(|t|t.name==name));}
    assert!(!tools.iter().any(|t|t.name.contains("apply")));
    let args=json!({"client_op_id":"stdio","replacement":"suggestion"});
    let result=client.call_tool(CallToolRequestParams::new("propose_workspace_edit").with_arguments(args.as_object().unwrap().clone())).await.unwrap();
    let response:Value=serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();assert_eq!(response["is_error"],false);
    assert_eq!(store.workspace_item(project,id,false).unwrap().body,"original");assert_eq!(store.workspace_proposals(project,Some(id),100).unwrap().len(),1);
    setup.service.stop_turn(setup.session);client.cancel().await.unwrap();
}

async fn wait_for_request(service: &Service) -> String {
    for _ in 0..500 {
        if let Some(request) = service.pending_approvals().pop() {
            return request.id;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("cererea n-a ajuns la serviciu");
}

#[tokio::test(flavor = "multi_thread")]
async fn approving_in_the_app_answers_claude_with_allow() {
    let setup = setup().await;
    let answer = tokio::spawn({
        let (url, file, session) = (setup.url.clone(), setup.token_file.clone(), setup.session);
        async move { call_approve(&url, &file, session).await }
    });
    let id = wait_for_request(&setup.service).await;
    let pending = setup.service.pending_approvals();
    assert_eq!(pending[0].tool, "Bash");
    assert_eq!(pending[0].arguments["command"], "cargo test");
    setup.service.resolve_approval(&id, true).unwrap();
    let answer = answer.await.unwrap();
    assert_eq!(
        answer,
        json!({"behavior": "allow", "updatedInput": {"command": "cargo test"}})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn refusing_in_the_app_answers_claude_with_deny() {
    let setup = setup().await;
    let answer = tokio::spawn({
        let (url, file, session) = (setup.url.clone(), setup.token_file.clone(), setup.session);
        async move { call_approve(&url, &file, session).await }
    });
    let id = wait_for_request(&setup.service).await;
    setup.service.resolve_approval(&id, false).unwrap();
    assert_eq!(answer.await.unwrap()["behavior"], "deny");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_service_means_deny() {
    let setup = setup().await;
    let answer = call_approve("http://127.0.0.1:9", &setup.token_file, setup.session).await;
    assert_eq!(answer["behavior"], "deny");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_token_means_deny_and_no_request_appears() {
    let setup = setup().await;
    std::fs::write(&setup.token_file, "gresit").unwrap();
    let answer = call_approve(&setup.url, &setup.token_file, setup.session).await;
    assert_eq!(answer["behavior"], "deny");
    assert!(setup.service.pending_approvals().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_without_a_running_turn_means_deny() {
    let setup = setup().await;
    let answer = call_approve(&setup.url, &setup.token_file, 999).await;
    assert_eq!(answer["behavior"], "deny");
    assert!(setup.service.pending_approvals().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_proxy_preserves_original_names_and_uses_token_file() {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_partial_json, header, method, path},
    };
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/internal/mcp/demo"))
        .and(header("authorization", format!("Bearer {TOKEN}")))
        .and(body_partial_json(json!({"project_id":7,"operation":"list","snapshot":"pinned"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"tools":[{"name":"original_echo","description":"Echo","inputSchema":{"type":"object"}}]})))
        .expect(1).mount(&server).await;
    Mock::given(method("POST")).and(path("/internal/mcp/demo"))
        .and(body_partial_json(json!({"project_id":7,"operation":"call","name":"original_echo","arguments":{"text":"hello"}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"content":[{"type":"text","text":"hello"}],"structuredContent":{"original":"value"},"isError":false})))
        .expect(1).mount(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let token_file = dir.path().join("secret-token-file");
    std::fs::write(&token_file, TOKEN).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_vatra"));
    command.args([
        "mcp-proxy",
        "--server",
        "demo",
        "--url",
        &server.uri(),
        "--project",
        "7",
        "--snapshot",
        "pinned",
        "--token-file",
    ]);
    command.arg(&token_file);
    let client = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools[0].name, "original_echo");
    let request = CallToolRequestParams::new("original_echo")
        .with_arguments(json!({"text":"hello"}).as_object().unwrap().clone());
    let result = client.call_tool(request).await.unwrap();
    assert_eq!(result.content[0].as_text().unwrap().text, "hello");
    assert_eq!(result.structured_content, Some(json!({"original":"value"})));
}
