mod common;
use serde_json::json;
use std::sync::Arc;
use vatra_core::{Event, Store};
use vatra_engine::search_history_tool;

#[tokio::test]
async fn returns_hits_with_snippets() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let project = store.create_project("p").unwrap();
    let session = store.create_session(project, "m").unwrap();
    store
        .append(
            session,
            &Event::UserMessage {
                text: "Earlier database decision: SQLite".into(),
            },
        )
        .unwrap();
    let tool = search_history_tool(store);
    let server = common::repeating(common::text("ok")).await;
    let context = common::setup(&server, Arc::new(vatra_engine::AllowAll), None, vec![], 2)
        .await
        .context;
    assert_eq!(
        tool.class(&json!({}), &context),
        vatra_engine::ToolClass::Free
    );
    let output = tool.call(json!({"query":"SQLite"}), &context).await;
    assert!(!output.is_error);
    assert!(output.text.contains("SQLite"));
}
