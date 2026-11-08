mod common;
use serde_json::{Value, json};
use std::sync::Arc;
use vatra_core::{NewItem, Selection, Store};
use vatra_engine::workspace_tools;

#[tokio::test]
async fn tools_only_propose_for_the_active_snapshot_and_preserve_emoji_selection() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let project = store.create_project("p").unwrap();
    let session = store.create_session(project, "fixture").unwrap();
    let id = store
        .create_item(
            &NewItem {
                kind: "note".into(),
                title: "Note".into(),
                body: "a😀b".into(),
                fields: json!({}),
                project_id: Some(project),
            },
            1,
        )
        .unwrap();
    let scope = store
        .capture_workspace_context(
            session,
            project,
            id,
            1,
            Some(Selection {
                start_utf16: 1,
                end_utf16: 3,
                selected_text: "😀".into(),
            }),
        )
        .unwrap();
    store.activate_workspace_context(&scope).unwrap();
    let tools = workspace_tools(store.clone(), scope.clone());
    assert_eq!(
        tools.iter().map(|t| t.def().name).collect::<Vec<_>>(),
        vec![
            "read_workspace_item",
            "propose_workspace_edit",
            "propose_workspace_tasks"
        ]
    );
    let server = common::repeating(common::text("ok")).await;
    let context = common::setup(&server, Arc::new(vatra_engine::AllowAll), None, vec![], 2)
        .await
        .context;
    let read = tools[0].call(json!({"offset":0,"limit":2}), &context).await;
    let page: Value = serde_json::from_str(&read.text).unwrap();
    assert_eq!(page["body"], "a😀");
    assert_eq!(page["next_offset"], 2);
    let args = json!({"client_op_id":"edit","replacement":"X"});
    for _ in 0..2 {
        assert!(!tools[1].call(args.clone(), &context).await.is_error);
    }
    assert_eq!(
        store.workspace_item(project, id, false).unwrap().body,
        "a😀b"
    );
    let proposals = store.workspace_proposals(project, Some(id), 100).unwrap();
    assert_eq!(proposals.len(), 1);
    assert_eq!(
        serde_json::to_value(&proposals[0].payload).unwrap()["new_body"],
        "aXb"
    );
    assert!(
        tools[1]
            .call(
                json!({"client_op_id":"foreign","replacement":"x","project_id":999}),
                &context
            )
            .await
            .is_error
    );
    assert!(
        tools[2]
            .call(
                json!({"client_op_id":"foreign","tasks":[{"title":"bad","project_id":999}]}),
                &context
            )
            .await
            .is_error
    );
    let tasks=tools[2].call(json!({"client_op_id":"tasks","tasks":[{"title":"One"},{"title":"Two"},{"title":"Three"}]}),&context).await;
    assert!(!tasks.is_error, "{}", tasks.text);
    assert!(
        store
            .workspace_items(project, &["task"], false, None, 100)
            .unwrap()
            .is_empty()
    );
    store.clear_workspace_context(session).unwrap();
    assert!(
        tools[1]
            .call(
                json!({"client_op_id":"late","replacement":"late"}),
                &context
            )
            .await
            .is_error
    );
    // Even a new turn on an identical snapshot does not reactivate an old tool handle.
    let next = store
        .capture_workspace_context(session, project, id, 1, scope.selection)
        .unwrap();
    store.activate_workspace_context(&next).unwrap();
    assert!(
        tools[1]
            .call(
                json!({"client_op_id":"late2","replacement":"late"}),
                &context
            )
            .await
            .is_error
    );
}
