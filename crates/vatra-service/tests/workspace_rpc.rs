mod common;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use vatra_core::{NewItem, ProposalPayload, ProposalState, Store, WorkspaceProposal};
use vatra_service::{Service, router};

const TOKEN: &str = "workspace-test-only-token";
type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
struct Server {
    service: Arc<Service>,
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn new() -> Self {
        let store = Arc::new(Store::open_in_memory().unwrap());
        store.create_project("One").unwrap();
        store.create_project("Two").unwrap();
        let service = Service::new(store, common::scripted_agents()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/rpc", listener.local_addr().unwrap());
        let app = router(service.clone(), TOKEN.into());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { service, url, task }
    }
    async fn connect(&self) -> Socket {
        let mut request = self.url.clone().into_client_request().unwrap();
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            format!("vatra.bearer.{TOKEN}").parse().unwrap(),
        );
        connect_async(request).await.unwrap().0
    }
}
async fn call(socket: &mut Socket, method: &str, params: Value) -> Value {
    socket
        .send(Message::Text(
            json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Message::Text(text) = message {
            let frame: Value = serde_json::from_str(&text).unwrap();
            if frame["id"] == 1 {
                return frame;
            }
        }
    }
}
async fn create(socket: &mut Socket) -> Value {
    let frame = call(
        socket,
        "workspace.items.create",
        json!({"project_id":1,"kind":"note","title":"Budget","body":"old","fields":{}}),
    )
    .await;
    assert!(frame["error"].is_null(), "{frame}");
    frame["result"].clone()
}

#[tokio::test]
async fn crud_conflict_events_and_reconnect_keep_committed_data() {
    let server = Server::new().await;
    let mut socket = server.connect().await;
    let mut updates = server.service.subscribe();
    let item = create(&mut socket).await;
    let id = item["id"].as_i64().unwrap();
    assert_eq!(item["revision"], 1);
    let event = serde_json::to_value(updates.recv().await.unwrap()).unwrap();
    assert_eq!(event["kind"], "workspace_changed");
    assert_eq!(event["item_ids"], json!([id]));
    assert_eq!(
        server
            .service
            .store()
            .workspace_item(1, id, false)
            .unwrap()
            .body,
        "old"
    );
    let saved = call(
        &mut socket,
        "workspace.items.save",
        json!({"project_id":1,"id":id,"expected_revision":1,"body":"new"}),
    )
    .await;
    assert_eq!(saved["result"]["revision"], 2);
    let conflict = call(
        &mut socket,
        "workspace.items.save",
        json!({"project_id":1,"id":id,"expected_revision":1,"body":"lost"}),
    )
    .await;
    assert_eq!(conflict["error"]["code"], 1101);
    assert_eq!(conflict["error"]["message"], "workspace_conflict");
    assert_eq!(conflict["error"]["data"]["current"]["body"], "new");
    socket.close(None).await.unwrap();
    let mut socket = server.connect().await;
    let list = call(&mut socket, "workspace.items.list", json!({"project_id":1})).await;
    assert_eq!(list["result"][0]["body"], "new");
    assert_eq!(
        call(
            &mut socket,
            "workspace.items.delete",
            json!({"project_id":1,"id":id,"expected_revision":2})
        )
        .await["result"]["revision"],
        3
    );
    assert_eq!(
        call(
            &mut socket,
            "workspace.items.get",
            json!({"project_id":1,"id":id})
        )
        .await["error"]["code"],
        1100
    );
    assert_eq!(
        call(
            &mut socket,
            "workspace.items.restore",
            json!({"project_id":1,"id":id,"expected_revision":3})
        )
        .await["result"]["revision"],
        4
    );
    assert_eq!(server.service.store().list_projects().unwrap().len(), 2);
}

#[tokio::test]
async fn project_scope_invalid_inputs_and_deleted_links_are_refused() {
    let server = Server::new().await;
    let mut socket = server.connect().await;
    let item = create(&mut socket).await;
    let id = item["id"].clone();
    for params in [
        json!({"project_id":2,"id":id}),
        json!({"project_id":999,"id":id}),
    ] {
        assert_eq!(
            call(&mut socket, "workspace.items.get", params).await["error"]["code"],
            1100
        );
    }
    for params in [
        json!({}),
        json!({"project_id":1,"kinds":["unknown"]}),
        json!({"project_id":1,"limit":101}),
        json!({"project_id":1,"unexpected":true}),
    ] {
        assert_eq!(
            call(&mut socket, "workspace.items.list", params).await["error"]["code"],
            -32602
        );
    }
    assert_eq!(
        call(
            &mut socket,
            "workspace.items.create",
            json!({"project_id":1,"kind":"task","title":"","fields":{}})
        )
        .await["error"]["code"],
        -32602
    );
    assert_eq!(
        call(
            &mut socket,
            "workspace.items.create",
            json!({"project_id":1,"kind":"event","title":"x","fields":{}})
        )
        .await["error"]["code"],
        -32602
    );
    call(
        &mut socket,
        "workspace.items.delete",
        json!({"project_id":1,"id":id,"expected_revision":1}),
    )
    .await;
    assert_eq!(
        call(
            &mut socket,
            "workspace.links.add",
            json!({"project_id":1,"from_id":id,"to_id":id,"rel":"related"})
        )
        .await["error"]["code"],
        1100
    );
    assert!(server.service.store().list_sessions().unwrap().is_empty());
}

#[tokio::test]
async fn proposals_retry_and_scoped_search_do_not_duplicate_items() {
    let server = Server::new().await;
    let mut socket = server.connect().await;
    let item = create(&mut socket).await;
    let id = item["id"].as_i64().unwrap();
    let store = server.service.store();
    let session = store.create_session(1, "scripted").unwrap();
    let p = store
        .save_workspace_proposal(&WorkspaceProposal {
            id: 0,
            project_id: 1,
            session_id: session,
            client_op_id: "batch".into(),
            target_id: Some(id),
            base_revision: Some(1),
            payload: ProposalPayload::Tasks {
                items: (0..3)
                    .map(|n| NewItem {
                        kind: "task".into(),
                        title: format!("Task {n}"),
                        body: String::new(),
                        fields: json!({}),
                        project_id: Some(1),
                    })
                    .collect(),
            },
            state: ProposalState::Pending,
            created_at: 1,
        })
        .unwrap();
    assert_eq!(
        call(
            &mut socket,
            "workspace.proposals.list",
            json!({"project_id":1,"target_id":id})
        )
        .await["result"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let a = call(
        &mut socket,
        "workspace.proposals.accept",
        json!({"project_id":1,"id":p}),
    )
    .await;
    let b = call(
        &mut socket,
        "workspace.proposals.accept",
        json!({"project_id":1,"id":p}),
    )
    .await;
    assert_eq!(a["result"], b["result"]);
    assert_eq!(
        store
            .workspace_items(1, &["task"], false, None, 100)
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        call(
            &mut socket,
            "workspace.search",
            json!({"project_id":2,"query":"Budget"})
        )
        .await["result"],
        json!([])
    );
    assert_eq!(
        call(
            &mut socket,
            "workspace.search",
            json!({"project_id":1,"query":"Budget"})
        )
        .await["result"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn websocket_without_token_cannot_subscribe() {
    let server = Server::new().await;
    assert!(connect_async(&server.url).await.is_err());
}

#[tokio::test]
async fn task_view_rpc_requires_a_valid_local_date_and_task_kind() {
    let server=Server::new().await;let mut socket=server.connect().await;
    call(&mut socket,"workspace.items.create",json!({"project_id":1,"kind":"task","title":"Due","fields":{"due_date":"2026-11-05"}})).await;
    assert_eq!(call(&mut socket,"workspace.items.list",json!({"project_id":1,"kinds":["task"],"task_view":"today","today":"2026-11-05"})).await["result"].as_array().unwrap().len(),1);
    for params in [json!({"project_id":1,"kinds":["task"],"task_view":"today"}),json!({"project_id":1,"kinds":["task"],"task_view":"today","today":"2026-02-29"}),json!({"project_id":1,"kinds":["document"],"task_view":"today","today":"2026-11-05"})] {
        assert_eq!(call(&mut socket,"workspace.items.list",params).await["error"]["code"],-32602);
    }
}

#[tokio::test]
async fn workspace_agent_info_reports_cost_without_starting_or_changing_an_agent() {
    let server=Server::new().await;let mut socket=server.connect().await;
    let status=call(&mut socket,"workspace.ai.status",json!({"agent":"scripted"})).await;
    assert_eq!(status["result"]["costs"],true);assert_eq!(status["result"]["supported"],true);
    assert_eq!(call(&mut socket,"workspace.ai.status",json!({"agent":"missing"})).await["error"]["code"],-32602);
    let item=create(&mut socket).await;
    assert_eq!(call(&mut socket,"workspace.ai.state",json!({"project_id":1,"item_id":item["id"]})).await["result"],json!([]));
    assert_eq!(call(&mut socket,"workspace.ai.state",json!({"project_id":2,"item_id":item["id"]})).await["error"]["code"],1100);
    assert!(server.service.store().list_sessions().unwrap().is_empty());
}

#[tokio::test]
async fn item_lists_use_bounded_previews_while_get_keeps_the_exact_source() {
    let server=Server::new().await;let mut socket=server.connect().await;
    for body in ["a😀".repeat(3000),"a\0tail".into()] {
    let created=call(&mut socket,"workspace.items.create",json!({"project_id":1,"kind":"document","title":"Long","body":body,"fields":{"format":"markdown"}})).await;
    let list=call(&mut socket,"workspace.items.list",json!({"project_id":1})).await;
    assert_eq!(list["result"][0]["body_truncated"],true);assert!(list["result"][0]["body"].as_str().unwrap().len()<=2048);
    let exact=call(&mut socket,"workspace.items.get",json!({"project_id":1,"id":created["result"]["id"]})).await;assert_eq!(exact["result"]["body"],body);
    }
}

#[tokio::test]
async fn reminders_rpc_scopes_acknowledgements_and_persists_language() {
    let server = Server::new().await;
    let mut socket = server.connect().await;
    let item=call(&mut socket,"workspace.items.create",json!({"project_id":1,"kind":"task","title":"Meeting","fields":{"remind_at":100,"timezone":"Europe/Bucharest"}})).await;
    assert!(item["error"].is_null(), "{item}");
    let reminders = server.service.store().claim_due_reminders(100, 50).unwrap();
    assert_eq!(reminders.len(), 1);
    let reminder = &reminders[0];
    assert_eq!(
        call(&mut socket, "reminders.list", json!({"project_id":1})).await["result"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        call(&mut socket, "reminders.list", json!({"project_id":2})).await["result"],
        json!([])
    );
    assert!(
        call(
            &mut socket,
            "reminders.ack",
            json!({"project_id":2,"id":reminder.id,"generation":reminder.generation})
        )
        .await["error"]
            .is_object()
    );
    assert!(
        call(
            &mut socket,
            "reminders.preferences.set",
            json!({"language":"ro"})
        )
        .await["error"]
            .is_null()
    );
    assert_eq!(
        server
            .service
            .store()
            .app_setting("reminders.language")
            .unwrap()
            .as_deref(),
        Some("ro")
    );
    assert!(
        call(
            &mut socket,
            "reminders.preferences.set",
            json!({"language":"invalid"})
        )
        .await["error"]
            .is_object()
    );
    assert_eq!(
        call(&mut socket, "reminders.status", json!({})).await["result"]["state"],
        "unavailable"
    );
    call(
        &mut socket,
        "reminders.ack",
        json!({"project_id":1,"id":reminder.id,"generation":reminder.generation}),
    )
    .await;
    assert_eq!(
        server
            .service
            .store()
            .workspace_item(1, item["result"]["id"].as_i64().unwrap(), false)
            .unwrap()
            .fields
            .get("status"),
        None
    );
    assert_eq!(
        call(&mut socket, "reminders.list", json!({"project_id":1})).await["result"],
        json!([])
    );
}

#[tokio::test]
async fn file_routes_links_and_proposal_rejection_are_scoped() {
    let server = Server::new().await;
    let mut socket = server.connect().await;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.md");
    std::fs::write(&source, "# source").unwrap();
    let imported = call(
        &mut socket,
        "workspace.files.import",
        json!({"project_id":1,"path":source,"format":"markdown"}),
    )
    .await;
    assert_eq!(imported["result"]["body"], "# source");
    let id = imported["result"]["id"].clone();
    let other = create(&mut socket).await["id"].clone();
    for _ in 0..2 {
        assert!(
            call(
                &mut socket,
                "workspace.links.add",
                json!({"project_id":1,"from_id":id,"to_id":other,"rel":"related"})
            )
            .await["error"]
                .is_null()
        );
    }
    assert_eq!(
        call(
            &mut socket,
            "workspace.links.list",
            json!({"project_id":1,"id":id})
        )
        .await["result"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    call(
        &mut socket,
        "workspace.links.remove",
        json!({"project_id":1,"from_id":other,"to_id":id,"rel":"related"}),
    )
    .await;
    assert_eq!(call(&mut socket,"workspace.links.list",json!({"project_id":1,"id":id})).await["result"],json!([]));
    let output = dir.path().join("output.md");
    assert!(
        call(
            &mut socket,
            "workspace.files.export",
            json!({"project_id":1,"id":id,"expected_revision":1,"path":output})
        )
        .await["error"]
            .is_null()
    );
    assert_eq!(std::fs::read_to_string(output).unwrap(), "# source");
    let store = server.service.store();
    let session = store.create_session(1, "scripted").unwrap();
    let p = store
        .save_workspace_proposal(&WorkspaceProposal {
            id: 0,
            project_id: 1,
            session_id: session,
            client_op_id: "edit".into(),
            target_id: id.as_i64(),
            base_revision: Some(1),
            payload: ProposalPayload::Edit {
                base_body: "# source".into(),
                new_body: "different".into(),
            },
            state: ProposalState::Pending,
            created_at: 1,
        })
        .unwrap();
    assert_eq!(
        call(
            &mut socket,
            "workspace.proposals.reject",
            json!({"project_id":2,"id":p})
        )
        .await["error"]["code"],
        1100
    );
    for _ in 0..2 {
        assert!(
            call(
                &mut socket,
                "workspace.proposals.reject",
                json!({"project_id":1,"id":p})
            )
            .await["error"]
                .is_null()
        );
    }
    assert_eq!(
        store
            .workspace_item(1, id.as_i64().unwrap(), false)
            .unwrap()
            .body,
        "# source"
    );
}
