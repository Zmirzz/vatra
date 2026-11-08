mod common;
use serde_json::{Value, json};
use std::sync::Arc;
use vatra_core::{NewItem, Store};
use vatra_engine::search_workspace_tool;

#[tokio::test]
async fn workspace_turn_search_never_reads_another_project_and_is_revoked() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let project = store.create_project("Current").unwrap();
    let other = store.create_project("Other").unwrap();
    let session = store.create_session(project, "fixture").unwrap();
    let mut target = 0;
    for (p, title) in [(project, "Visible uniquephrase"), (other, "Hidden uniquephrase")] {
        let id = store.create_item(&NewItem {kind:"note".into(),title:title.into(),body:"uniquephrase".into(),fields:json!({}),project_id:Some(p)},1).unwrap();
        if p == project { target = id; }
    }
    let scope = store.capture_workspace_context(session,project,target,1,None).unwrap();
    store.activate_workspace_context(&scope).unwrap();
    let tool = vatra_engine::scoped_search_workspace_tool(store.clone(),scope);
    let server = common::repeating(common::text("ok")).await;
    let context = common::setup(&server,Arc::new(vatra_engine::AllowAll),None,vec![],2).await.context;
    let output = tool.call(json!({"query":"uniquephrase","project_id":other}),&context).await;
    assert!(!output.is_error,"{}",output.text);
    let hits: Value = serde_json::from_str(&output.text).unwrap();
    assert_eq!(hits.as_array().unwrap().len(),1);
    assert_eq!(hits[0]["title"],"Visible uniquephrase");
    store.clear_workspace_context(session).unwrap();
    assert!(tool.call(json!({"query":"uniquephrase"}),&context).await.is_error);
}

#[tokio::test]
async fn search_workspace_tool_returns_hits_as_json() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    store
        .create_item(
            &NewItem {
                kind: "note".into(),
                title: "Buget".into(),
                body: "plan trimestrial".into(),
                fields: json!({}),
                project_id: None,
            },
            1,
        )
        .unwrap();
    let tool = search_workspace_tool(store);
    let server = common::repeating(common::text("ok")).await;
    let context = common::setup(&server, Arc::new(vatra_engine::AllowAll), None, vec![], 2)
        .await
        .context;
    assert_eq!(
        tool.class(&json!({}), &context),
        vatra_engine::ToolClass::Free
    );
    let output = tool.call(json!({"query":"trimestrial"}), &context).await;
    assert!(!output.is_error);
    let hits: Value = serde_json::from_str(&output.text).unwrap();
    assert_eq!(hits[0]["kind"], "note");
    assert_eq!(hits[0]["title"], "Buget");
    assert_eq!(hits.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn invalid_workspace_kind_filters_never_fall_back_to_all_items() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    store.create_item(&NewItem { kind:"note".into(), title:"Report".into(), body:"private report".into(), fields:json!({}), project_id:None },1).unwrap();
    let tool = search_workspace_tool(store);
    let server = common::repeating(common::text("ok")).await;
    let context = common::setup(&server,Arc::new(vatra_engine::AllowAll),None,vec![],2).await.context;
    for kinds in [json!([12]),json!(["note",12]),json!("note"),json!(null),json!(["unknown"]),json!(vec!["note";7])] {
        let output = tool.call(json!({"query":"report","kinds":kinds}),&context).await;
        assert!(output.is_error,"filter {kinds} was accepted: {}",output.text);
        assert!(!output.text.contains("private report"));
    }
}
