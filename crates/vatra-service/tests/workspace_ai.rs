mod common;
use serde_json::json;
use std::sync::Arc;
use vatra_core::{NewItem, Selection, Store};
use vatra_service::workspace::ai::{StartWorkspaceTurn, start_workspace_turn};
use vatra_service::{PermissionMode, Service};

fn setup(script: common::Scripted) -> (Arc<Service>, i64, i64, i64) {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let project = store.create_project("p").unwrap();
    let session = store.create_session(project, "scripted").unwrap();
    let id = store
        .create_item(
            &NewItem {
                kind: "note".into(),
                title: "Note".into(),
                body: "x😀y".into(),
                fields: json!({}),
                project_id: Some(project),
            },
            1,
        )
        .unwrap();
    let service = Service::new(store, Arc::new(common::ScriptedFactory(script))).unwrap();
    (service, project, session, id)
}
fn request(project: i64, session: i64, id: i64) -> StartWorkspaceTurn {
    StartWorkspaceTurn {
        session_id: session,
        project_id: project,
        item_id: id,
        expected_revision: 1,
        selection: None,
        instruction: "Improve".into(),
        consent_cost: true,
        mode: PermissionMode::Supervised,
    }
}

#[tokio::test]
async fn snapshot_is_bound_before_build_busy_refuses_and_stop_revokes_tools() {
    let (service, p, s, id) = setup(common::Scripted::blocking());
    let mut updates = service.subscribe();
    start_workspace_turn(&service, request(p, s, id)).unwrap();
    let scope = service.store().workspace_turn_context(s).unwrap().unwrap();
    assert_eq!(scope.base_body, "x😀y");
    assert!(start_workspace_turn(&service, request(p, s, id)).is_err());
    assert_eq!(
        service
            .store()
            .workspace_turn_context(s)
            .unwrap()
            .unwrap()
            .scope_id,
        scope.scope_id
    );
    service.stop_turn(s);
    common::collect_until_stopped(&mut updates).await;
    assert!(service.store().workspace_turn_context(s).unwrap().is_none());
    assert_eq!(
        service.store().workspace_item(p, id, false).unwrap().body,
        "x😀y"
    );
}

#[tokio::test]
async fn stale_foreign_or_split_surrogate_selections_fail_before_starting() {
    let (service, p, s, id) = setup(common::Scripted::blocking());
    for selection in [
        Selection {
            start_utf16: 1,
            end_utf16: 2,
            selected_text: "😀".into(),
        },
        Selection {
            start_utf16: 1,
            end_utf16: 3,
            selected_text: "wrong".into(),
        },
    ] {
        let mut r = request(p, s, id);
        r.selection = Some(selection);
        assert!(start_workspace_turn(&service, r).is_err());
    }
    let mut stale = request(p, s, id);
    stale.expected_revision = 99;
    assert!(start_workspace_turn(&service, stale).is_err());
    let mut foreign = request(p, s, id);
    foreign.project_id = p + 1;
    assert!(start_workspace_turn(&service, foreign).is_err());
    let mut cost = request(p, s, id);
    cost.consent_cost = false;
    assert!(start_workspace_turn(&service, cost).is_err());
    assert!(!service.is_running(s));
    assert!(service.store().workspace_turn_context(s).unwrap().is_none());
}

#[tokio::test]
async fn completed_turn_clears_context_but_keeps_pending_proposals() {
    let (service, p, s, id) = setup(common::Scripted::blocking());
    let mut updates = service.subscribe();
    start_workspace_turn(&service, request(p, s, id)).unwrap();
    let scope = service.store().workspace_turn_context(s).unwrap().unwrap();
    service
        .store()
        .propose_workspace_edit(&scope, "op", "new", 1)
        .unwrap();
    service.stop_turn(s);
    common::collect_until_stopped(&mut updates).await;
    assert_eq!(
        service
            .store()
            .workspace_proposals(p, Some(id), 100)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        service.store().workspace_item(p, id, false).unwrap().body,
        "x😀y"
    );
}

#[tokio::test]
async fn native_bridge_contract_requires_auth_and_the_exact_turn_capability() {
    // HTTP/MCP contract only: this never launches Claude or Codex or uses a subscription.
    let (service, p, s, id) = setup(common::Scripted::blocking());
    let mut updates = service.subscribe();
    start_workspace_turn(&service, request(p, s, id)).unwrap();
    let scope = service.store().workspace_turn_context(s).unwrap().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/internal/workspace",
        listener.local_addr().unwrap()
    );
    let app = vatra_service::router(service.clone(), "test-token".into());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();
    let args = json!({"session_id":s,"scope_id":scope.scope_id,"tool":"propose_workspace_edit","arguments":{"client_op_id":"bridge-op","replacement":"suggestion"}});
    assert_eq!(
        client.post(&url).json(&args).send().await.unwrap().status(),
        401
    );
    let mut wrong = args.clone();
    wrong["scope_id"] = json!("old-turn");
    assert_eq!(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&wrong)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let result: serde_json::Value = client
        .post(&url)
        .bearer_auth("test-token")
        .json(&args)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result["is_error"], false);
    assert_eq!(
        service.store().workspace_item(p, id, false).unwrap().body,
        "x😀y"
    );
    service.stop_turn(s);
    assert!(
        service
            .store()
            .propose_workspace_edit(&scope, "late", "late", 1)
            .is_err()
    );
    assert_eq!(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&args)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    common::collect_until_stopped(&mut updates).await;
    server.abort();
}

struct ProposalFactory {
    store: Arc<Store>,
}
struct ProposalAgent {
    store: Arc<Store>,
    scope: vatra_core::WorkspaceTurnContext,
}
#[async_trait::async_trait]
impl vatra_core::Agent for ProposalAgent {
    fn name(&self) -> &str {
        "scripted"
    }
    async fn run_turn(
        &self,
        _: vatra_core::TurnRequest,
        sink: tokio::sync::mpsc::UnboundedSender<vatra_core::Event>,
    ) -> Result<vatra_core::TurnOutcome, vatra_core::AgentError> {
        let arguments = json!({"client_op_id":"agent-op","replacement":"proposed by fixture"});
        sink.send(vatra_core::Event::ToolCall {
            id: "fixture-call".into(),
            name: "propose_workspace_edit".into(),
            arguments: arguments.clone(),
        })
        .unwrap();
        let output = vatra_engine::call_workspace_tool(
            self.store.clone(),
            self.scope.clone(),
            "propose_workspace_edit",
            arguments,
        );
        assert!(!output.is_error, "{}", output.text);
        sink.send(vatra_core::Event::ToolResult {
            id: "fixture-call".into(),
            output: output.text,
            is_error: false,
        })
        .unwrap();
        Ok(vatra_core::TurnOutcome::default())
    }
}
#[async_trait::async_trait]
impl vatra_service::AgentFactory for ProposalFactory {
    fn names(&self) -> Vec<String> {
        vec!["scripted".into()]
    }
    async fn build(
        &self,
        r: vatra_service::BuildRequest,
    ) -> Result<Arc<dyn vatra_core::Agent>, String> {
        Ok(Arc::new(ProposalAgent {
            store: self.store.clone(),
            scope: self
                .store
                .workspace_turn_context(r.session_id)
                .unwrap()
                .unwrap(),
        }))
    }
    fn summarizer(&self) -> Arc<dyn vatra_core::Summarizer> {
        <common::ScriptedFactory as vatra_service::AgentFactory>::summarizer(
            &common::ScriptedFactory(common::Scripted::text("unused")),
        )
    }
}
#[tokio::test]
async fn scripted_agent_saves_a_reviewable_proposal_without_editing_the_document() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let p = store.create_project("p").unwrap();
    let s = store.create_session(p, "scripted").unwrap();
    let id = store
        .create_item(
            &NewItem {
                kind: "note".into(),
                title: "Note".into(),
                body: "original".into(),
                fields: json!({}),
                project_id: Some(p),
            },
            1,
        )
        .unwrap();
    let service = Service::new(
        store.clone(),
        Arc::new(ProposalFactory {
            store: store.clone(),
        }),
    )
    .unwrap();
    let mut updates = service.subscribe();
    start_workspace_turn(&service, request(p, s, id)).unwrap();
    common::collect_until_stopped(&mut updates).await;
    assert_eq!(store.workspace_item(p, id, false).unwrap().body, "original");
    assert_eq!(
        store.workspace_proposals(p, Some(id), 100).unwrap().len(),
        1
    );
    assert!(store.workspace_turn_context(s).unwrap().is_none());
}
