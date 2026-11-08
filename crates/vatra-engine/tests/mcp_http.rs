mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use support::{TestServer, approve, http_server, oauth_server};
use vatra_core::{McpServerConfig, MemorySecretStore, SecretRef, SecretStore};
use vatra_engine::{ExpandVars, McpHub, login};

async fn started(servers: Vec<McpServerConfig>) -> McpHub {
    let hub = McpHub::new(servers, ExpandVars::default());
    hub.ensure_connected().await;
    hub
}

#[tokio::test]
async fn http_lists_and_calls() {
    let server = TestServer::start(false).await;
    let hub = started(vec![http_server("remote", &server.url)]).await;
    assert!(hub.failures().is_empty(), "{:?}", hub.failures());
    assert!(hub.tools().iter().any(|tool| tool.name == "mcp__remote__echo"));
    let out = hub.call("mcp__remote__echo", json!({ "text": "hi over http" })).await;
    assert!(!out.is_error);
    assert_eq!(out.text, "hi over http");
}

#[tokio::test]
async fn headers_sent() {
    let server = TestServer::start(false).await;
    let remote = McpServerConfig {
        headers: [("X-Test-Header".to_string(), "value-1".to_string())].into(),
        ..http_server("remote", &server.url)
    };
    let _hub = started(vec![remote]).await;
    assert!(server.saw_header("x-test-header", "value-1"));
}

#[tokio::test]
async fn http_redirects_cannot_forward_custom_credentials_to_another_endpoint() {
    use wiremock::{Mock,MockServer,ResponseTemplate};
    use wiremock::matchers::any;
    let source=MockServer::start().await;let target=MockServer::start().await;
    Mock::given(any()).respond_with(ResponseTemplate::new(307).insert_header("Location",format!("{}/mcp",target.uri()))).mount(&source).await;
    Mock::given(any()).respond_with(ResponseTemplate::new(400)).expect(0).mount(&target).await;
    let config=McpServerConfig{headers:[("X-Api-Key".into(),"sensitive-test-value".into())].into(),..http_server("redirected",&format!("{}/mcp",source.uri()))};
    let hub=started(vec![config]).await;
    assert!(!hub.failures().is_empty());
    target.verify().await;
}

#[tokio::test]
async fn resource_only_servers_do_not_receive_unadvertised_tool_requests() {
    use wiremock::{Mock,MockServer,ResponseTemplate};use wiremock::matchers::body_partial_json;
    let server=MockServer::start().await;
    Mock::given(body_partial_json(json!({"method":"initialize"}))).respond_with(ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":0,"result":{"protocolVersion":"2026-07-28","capabilities":{"resources":{},"prompts":{}},"serverInfo":{"name":"resources-only","version":"1"}}}))).mount(&server).await;
    Mock::given(body_partial_json(json!({"method":"notifications/initialized"}))).respond_with(ResponseTemplate::new(202)).mount(&server).await;
    // Requests are sequential: resources-only discovery skips tools/list.
    Mock::given(body_partial_json(json!({"method":"prompts/list"}))).respond_with(ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":{"prompts":[{"name":"greet","description":"A prompt"}]}}))).mount(&server).await;
    Mock::given(body_partial_json(json!({"method":"resources/list"}))).respond_with(ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":2,"result":{"resources":[{"uri":"test://resource","name":"A resource"}]}}))).mount(&server).await;
    Mock::given(body_partial_json(json!({"method":"tools/list"}))).respond_with(ResponseTemplate::new(400)).expect(0).mount(&server).await;
    let hub=started(vec![http_server("resources",&format!("{}/mcp",server.uri()))]).await;
    assert!(hub.failures().is_empty(),"{:?}",hub.failures());assert!(hub.tools().is_empty());assert_eq!(hub.prompts().len(),1);assert_eq!(hub.resources().len(),1);
}

#[tokio::test]
async fn headers_helper_runs_and_times_out_after_10s() {
    let server = TestServer::start(false).await;
    let echo = if cfg!(windows) { r#"echo {"X-Helper": "yes"}"# } else { r#"echo '{"X-Helper": "yes"}'"# };
    let helped = McpServerConfig { headers_helper: Some(echo.into()), ..http_server("helped", &server.url) };
    let hub = started(vec![helped]).await;
    assert!(hub.failures().is_empty(), "{:?}", hub.failures());
    assert!(server.saw_header("x-helper", "yes"));

    let sleep = if cfg!(windows) { "ping -n 30 127.0.0.1 > nul" } else { "sleep 30" };
    let slow = McpServerConfig { headers_helper: Some(sleep.into()), ..http_server("slow", &server.url) };
    let started_at = Instant::now();
    let hub = started(vec![slow]).await;
    let elapsed = started_at.elapsed();
    assert!(hub.failures()[0].1.contains("10 seconds"), "{:?}", hub.failures());
    assert!(elapsed >= Duration::from_secs(9) && elapsed < Duration::from_secs(25), "{elapsed:?}");
}

#[tokio::test]
async fn secret_header_from_keychain() {
    let server = TestServer::start(false).await;
    let secrets = Arc::new(MemorySecretStore::default());
    secrets.set("vatra-test-entry", "keychain-value").unwrap();
    let remote = McpServerConfig {
        headers: [("X-Key".to_string(), "${TOKEN}".to_string())].into(),
        secrets: [("TOKEN".to_string(), SecretRef::Keychain("vatra-test-entry".into()))].into(),
        ..http_server("remote", &server.url)
    };
    let hub = McpHub::new(vec![remote], ExpandVars::default()).with_secrets(secrets);
    hub.ensure_connected().await;
    assert!(hub.failures().is_empty(), "{:?}", hub.failures());
    assert!(server.saw_header("x-key", "keychain-value"));
}

#[tokio::test]
async fn env_header_from_env() {
    let server = TestServer::start(false).await;
    let env = [
        ("VATRA_TEST_NOT_READ".to_string(), "x".to_string()),
        ("TEST_HEADER_SOURCE".to_string(), "from-env".to_string()),
    ];
    let remote = McpServerConfig {
        headers: [("X-Env".to_string(), "${TEST_HEADER_SOURCE}".to_string())].into(),
        ..http_server("remote", &server.url)
    };
    let vars = ExpandVars { env: env.into_iter().collect(), ..Default::default() };
    let hub = McpHub::new(vec![remote], vars);
    hub.ensure_connected().await;
    assert!(hub.failures().is_empty(), "{:?}", hub.failures());
    assert!(server.saw_header("x-env", "from-env"));
}

#[tokio::test]
async fn header_cannot_read_a_service_variable() {
    let server = TestServer::start(false).await;
    let vars = ExpandVars { env: [("VATRA_KEY".to_string(), "service-key".to_string())].into(), ..Default::default() };
    let remote = McpServerConfig {
        headers: [("X-Leak".to_string(), "${VATRA_KEY}".to_string())].into(),
        ..http_server("remote", &server.url)
    };
    let hub = McpHub::new(vec![remote], vars);
    hub.ensure_connected().await;
    assert!(hub.failures()[0].1.contains("reserved"), "{:?}", hub.failures());
    assert!(!server.saw_header("x-leak", "service-key"));
}

#[tokio::test]
async fn sse_only_server_fails_clearly() {
    let server = TestServer::start(false).await;
    let legacy = format!("{}/legacy", server.base());
    let hub = started(vec![http_server("old", &legacy)]).await;
    let message = &hub.failures()[0].1;
    assert!(message.contains("Streamable HTTP"), "{message}");
    assert!(!message.contains("Transport closed"), "{message}");
}

#[tokio::test]
async fn large_response_under_16_mib_is_read() {
    let server = TestServer::start(false).await;
    let hub = started(vec![http_server("remote", &server.url)]).await;
    let out = hub.call("mcp__remote__big", json!({ "megabytes": 8 })).await;
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(out.text.len(), 8 * 1024 * 1024);
}

#[tokio::test]
async fn sse_response_is_read() {
    let server = TestServer::start(false).await;
    let hub = started(vec![http_server("remote", &server.url)]).await;
    let out = hub.call("mcp__remote__echo", json!({ "text": "in a stream", "stream": true })).await;
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(out.text, "in a stream");
}

#[tokio::test]
async fn oversized_response_over_16_mib_fails_clearly() {
    let server = TestServer::start(false).await;
    let hub = started(vec![http_server("remote", &server.url)]).await;
    let out = hub.call("mcp__remote__big", json!({ "megabytes": 17 })).await;
    assert!(out.is_error, "a response over 16 MiB was accepted");
    assert!(out.text.contains("16 MiB"), "{}", out.text);
    assert!(out.text.len() < 4096, "the error repeats the response");
}

#[tokio::test]
async fn login_discovers_metadata() {
    let server = TestServer::start(true).await;
    let secrets = Arc::new(MemorySecretStore::default());
    login(&oauth_server("remote", &server.url), secrets, &approve(Arc::clone(&server.shared), false)).await.unwrap();
    let urls = server.shared.auth_urls.lock().unwrap().clone();
    assert!(urls[0].starts_with(&format!("{}/authorize?", server.base())), "{urls:?}");
}

#[tokio::test]
async fn dynamic_client_registration() {
    let server = TestServer::start(true).await;
    let secrets = Arc::new(MemorySecretStore::default());
    login(&oauth_server("remote", &server.url), secrets, &approve(Arc::clone(&server.shared), false)).await.unwrap();
    assert_eq!(server.shared.registrations.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn configured_client_id_skips_dcr() {
    let server = TestServer::start(true).await;
    let secrets = Arc::new(MemorySecretStore::default());
    let config = McpServerConfig {
        oauth: Some(vatra_core::McpOAuth { client_id: Some("preset-client".into()), ..Default::default() }),
        ..http_server("remote", &server.url)
    };
    login(&config, secrets, &approve(Arc::clone(&server.shared), false)).await.unwrap();
    assert_eq!(server.shared.registrations.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(server.shared.auth_urls.lock().unwrap()[0].contains("client_id=preset-client"));
}

#[tokio::test]
async fn token_saved_to_secret_store_only() {
    let server = TestServer::start(true).await;
    let secrets = Arc::new(MemorySecretStore::default());
    login(&oauth_server("remote", &server.url), secrets.clone(), &approve(Arc::clone(&server.shared), false))
        .await
        .unwrap();
    assert_eq!(secrets.keys().len(), 1, "{:?}", secrets.keys());
    let value = secrets.get(&secrets.keys()[0]).unwrap().unwrap();
    assert!(value.contains("access-1"));
}

#[tokio::test]
async fn pkce_is_used() {
    let server = TestServer::start(true).await;
    let secrets = Arc::new(MemorySecretStore::default());
    login(&oauth_server("remote", &server.url), secrets, &approve(Arc::clone(&server.shared), false)).await.unwrap();
    let url = server.shared.auth_urls.lock().unwrap()[0].clone();
    assert!(url.contains("code_challenge_method=S256"), "{url}");
    assert!(url.contains("code_challenge="), "{url}");
}

/// A callback with another state gets a 400 and does not end the login, which keeps waiting for the right one.
#[tokio::test]
async fn state_mismatch_rejected() {
    let server = TestServer::start(true).await;
    let secrets = Arc::new(MemorySecretStore::default());
    let config = oauth_server("remote", &server.url);
    login(&config, secrets.clone(), &approve(Arc::clone(&server.shared), true)).await.unwrap();
    let callbacks = server.shared.callbacks.lock().unwrap().clone();
    assert!(callbacks[0].contains("400"), "{callbacks:?}");
    assert_eq!(secrets.keys().len(), 1);
    let value = secrets.get(&secrets.keys()[0]).unwrap().unwrap();
    assert!(value.contains("access-1"), "the token of the right callback is saved");
}

#[tokio::test]
async fn refresh_on_401() {
    let server = TestServer::start(true).await;
    let secrets = Arc::new(MemorySecretStore::default());
    let config = oauth_server("remote", &server.url);
    login(&config, secrets.clone(), &approve(Arc::clone(&server.shared), false)).await.unwrap();
    server.revoke_tokens();

    let hub = McpHub::new(vec![config], ExpandVars::default()).with_secrets(secrets.clone());
    hub.ensure_connected().await;
    let out = hub.call("mcp__remote__echo", json!({ "text": "after refresh" })).await;
    assert!(hub.failures().is_empty(), "{:?}", hub.failures());
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(out.text, "after refresh");
    let key = secrets.keys()[0].clone();
    assert!(secrets.get(&key).unwrap().unwrap().contains("access-2"));
}

/// A stored token that the authorization server no longer refreshes is a missing login. It is retried later, so
/// a login made in the meantime applies, and it is not a permanent failure.
#[tokio::test]
async fn expired_refresh_becomes_needs_login() {
    let server = TestServer::start(true).await;
    let secrets = Arc::new(MemorySecretStore::default());
    let config = oauth_server("remote", &server.url);
    login(&config, secrets.clone(), &approve(Arc::clone(&server.shared), false)).await.unwrap();
    server.revoke_tokens();
    server.refuse_refresh();

    let hub = McpHub::new(vec![config], ExpandVars::default()).with_secrets(secrets.clone());
    hub.ensure_connected().await;
    let failure = hub.failures();
    assert_eq!(failure.len(), 1);
    assert!(failure[0].1.contains("login"), "{failure:?}");

    server.allow_refresh();
    hub.ensure_connected().await;
    assert!(hub.failures().is_empty(), "{:?}", hub.failures());
    let out = hub.call("mcp__remote__echo", json!({ "text": "back" })).await;
    assert_eq!(out.text, "back");
}

#[tokio::test]
async fn needs_login_is_reported_not_fatal() {
    let server = TestServer::start(false).await;
    let hub = started(vec![oauth_server("locked", &server.url), http_server("open", &server.url)]).await;
    let failure = hub.failures().into_iter().find(|(name, _)| name == "locked").expect("the server should fail");
    assert!(failure.1.contains("login"), "{failure:?}");
    let out = hub.call("mcp__open__echo", json!({ "text": "still here" })).await;
    assert_eq!(out.text, "still here");
}

#[test]
fn no_read_of_native_token_stores() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let forbidden = [".credentials.json", "auth.json", ".codex", "Claude Code-credentials"];
    let mut pending = vec![src];
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            pending.extend(std::fs::read_dir(&path).unwrap().map(|entry| entry.unwrap().path()));
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for name in forbidden {
            assert!(!text.contains(name), "{} reads {name}", path.display());
        }
    }
}
