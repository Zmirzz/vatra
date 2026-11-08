use serde_json::json;
use vatra_core::McpServerConfig;
use vatra_engine::{ExpandVars, McpHub};

#[tokio::test]
async fn proxy_lists_only_selected_server_and_keeps_raw_content() {
    let hub = McpHub::new(
        vec![
            McpServerConfig {
                name: "demo".into(),
                command: Some(env!("CARGO_BIN_EXE_vatra-fixture-mcp").into()),
                disabled_tools: vec!["crash".into()],
                ..Default::default()
            },
            McpServerConfig {
                name: "unrelated".into(),
                command: Some("not-a-real-mcp-binary".into()),
                ..Default::default()
            },
        ],
        ExpandVars::default(),
    );
    assert!(hub.ensure_server_connected("demo").await);
    assert!(!hub.ensure_server_connected("missing").await);
    assert!(
        hub.failures().is_empty(),
        "listing the proxy must not launch unrelated servers"
    );
    let image = hub.call_raw("mcp__demo__image", json!({})).await.unwrap();
    assert_eq!(image.content[0].as_image().unwrap().data, "aGVsbG8=");
    assert!(
        hub.call_raw("mcp__demo__crash", json!({}))
            .await
            .unwrap_err()
            .contains("disabled")
    );
    let echo = hub
        .call_raw("mcp__demo__echo", json!({"text":"ășț"}))
        .await
        .unwrap();
    assert_eq!(echo.content[0].as_text().unwrap().text, "ășț");
}
