mod common;
use serde_json::json;
use vatra_service::PermissionMode;
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::{method, path};

#[tokio::test]
async fn bridge_search_requires_auth_and_active_turn_and_uses_configured_provider() {
    let search = MockServer::start().await;
    Mock::given(method("GET")).and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results":[{"title":"Current fact", "url":"https://example.org/fact", "content":"Checked"}]})))
        .expect(1).mount(&search).await;
    let (service, session) = common::with_agent(common::Scripted::blocking());
    service.store().set_app_setting("search.duckduckgo", Some("off")).unwrap();
    service.store().save_connector(&vatra_core::ConnectorRow { id:"search".into(), kind:"websearch".into(), name:"search".into(),
        settings_json:json!({"provider":"searxng", "url":search.uri()}).to_string(), created_at:1 }).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/internal/web-search", listener.local_addr().unwrap());
    let app = vatra_service::router(service.clone(), "test-token".into());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    let args = json!({"session_id":session,"arguments":{"query":"current fact"}});
    assert_eq!(client.post(&url).json(&args).send().await.unwrap().status(), 401);
    assert_eq!(client.post(&url).bearer_auth("test-token").json(&args).send().await.unwrap().status(), 409);
    service.start_turn(session, "look up fact".into(), PermissionMode::FullAccess).unwrap();
    let result: serde_json::Value = client.post(&url).bearer_auth("test-token").json(&args).send().await.unwrap().json().await.unwrap();
    assert_eq!(result["is_error"], false, "{result}");
    assert!(result["text"].as_str().unwrap().contains("https://example.org/fact"));
    service.stop_turn(session);
    common::wait_until_stopped(&service, session).await;
    assert_eq!(client.post(&url).bearer_auth("test-token").json(&args).send().await.unwrap().status(), 409);
    server.abort();
}
