mod common;
mod servers;
#[path = "../../vatra-engine/tests/support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::Duration;

use servers::service_with;
use support::{TestServer, approve, oauth_server};
use vatra_core::{MemorySecretStore, SecretStore};
use vatra_service::Notification;

/// Every notification of a login, as the client gets them, must not carry the token or the refresh token.
#[tokio::test]
async fn tokens_never_appear_in_events() {
    let server = TestServer::start(true).await;
    let secrets = Arc::new(MemorySecretStore::default());
    let service = service_with(vec![oauth_server("remote", &server.url)], secrets.clone());
    let mut updates = service.subscribe();
    service.start_mcp_login("remote").unwrap();

    let browser = approve(Arc::clone(&server.shared), false);
    let mut events = Vec::new();
    let mut outcome = None;
    tokio::time::timeout(Duration::from_secs(30), async {
        while outcome.is_none() {
            let notification = updates.recv().await.unwrap();
            if let Notification::OpenUrl { url } = &notification {
                browser(url);
            }
            if let Notification::McpLogin { ok, .. } = &notification {
                outcome = Some(*ok);
            }
            events.push(serde_json::to_string(&notification).unwrap());
        }
    })
    .await
    .expect("the login should finish");
    assert_eq!(outcome, Some(true));
    assert!(events.iter().any(|event| event.contains("open_url")), "the login address was never sent");

    // The tokens are in the store, so the search below means something.
    let stored = secrets.get(&secrets.keys()[0]).unwrap().unwrap();
    assert!(stored.contains("access-1") && stored.contains("refresh-1"));
    let everything = events.join("\n");
    assert!(!everything.contains("access-1"), "{everything}");
    assert!(!everything.contains("refresh-1"), "{everything}");
}
