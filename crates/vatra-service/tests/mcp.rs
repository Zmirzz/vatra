mod common;
mod servers;

use std::sync::Arc;
use std::time::Duration;

use servers::service_with;
use vatra_core::{McpOAuth, McpServerConfig, MemorySecretStore};
use vatra_service::{Notification, ServiceError};

fn remote(name: &str, url: &str) -> McpServerConfig {
    McpServerConfig { name: name.into(), url: Some(url.into()), oauth: Some(McpOAuth::default()), ..Default::default() }
}

#[tokio::test]
async fn status_lists_configured_servers_without_a_login() {
    let plain = McpServerConfig { name: "local".into(), command: Some("tool".into()), ..Default::default() };
    let service = service_with(vec![remote("remote", "http://127.0.0.1:9/mcp"), plain], Arc::new(MemorySecretStore::default()));
    let status = service.mcp_status().await;
    assert_eq!(status.len(), 2);
    assert_eq!(status[0].name, "remote");
    assert!(status[0].http && status[0].oauth && !status[0].logged_in);
    assert_eq!(status[1].name, "local");
    assert!(!status[1].http && !status[1].oauth && !status[1].logged_in);
}

#[tokio::test]
async fn login_of_an_unknown_server_is_refused() {
    let service = service_with(vec![], Arc::new(MemorySecretStore::default()));
    assert!(matches!(service.start_mcp_login("nope"), Err(ServiceError::UnknownMcpServer)));
}

#[tokio::test]
async fn login_of_a_server_without_oauth_is_refused() {
    let plain = McpServerConfig { name: "local".into(), command: Some("tool".into()), ..Default::default() };
    let service = service_with(vec![plain], Arc::new(MemorySecretStore::default()));
    assert!(matches!(service.start_mcp_login("local"), Err(ServiceError::NoOAuth)));
}

#[tokio::test]
async fn failed_login_is_reported_as_a_notification() {
    // Nothing listens on port 9, so the authorization server can't be found.
    let service = service_with(vec![remote("remote", "http://127.0.0.1:9/mcp")], Arc::new(MemorySecretStore::default()));
    let mut updates = service.subscribe();
    service.start_mcp_login("remote").unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(Notification::McpLogin { server, ok, message }) = updates.recv().await {
                return (server, ok, message);
            }
        }
    })
    .await
    .expect("the login should report back");
    assert_eq!(outcome.0, "remote");
    assert!(!outcome.1);
    assert!(!outcome.2.is_empty());
}
