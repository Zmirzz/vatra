//! A local MCP server over HTTP with an OAuth authorization server of its own. The service tests load this file
//! too, so that a login can be driven through the service.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;
use vatra_core::McpOAuth;
use vatra_core::McpServerConfig;

#[derive(Default)]
pub struct Shared {
    base: Mutex<String>,
    protected: bool,
    /// Headers of every request to `/mcp`, with lowercase names.
    pub seen: Mutex<Vec<Vec<(String, String)>>>,
    valid_tokens: Mutex<Vec<String>>,
    issued: AtomicUsize,
    pub registrations: AtomicUsize,
    challenge: Mutex<Option<String>>,
    pub auth_urls: Mutex<Vec<String>>,
    /// The status line that each callback to the browser's return address got.
    pub callbacks: Mutex<Vec<String>>,
    refuse_refresh: AtomicBool,
}

pub struct TestServer {
    pub url: String,
    pub shared: Arc<Shared>,
}

impl TestServer {
    /// Starts the server. With `protected`, `/mcp` needs a token that the authorization server issued.
    pub async fn start(protected: bool) -> TestServer {
        let shared = Arc::new(Shared { protected, ..Default::default() });
        let app = Router::new()
            .route("/mcp", any(mcp))
            .route("/legacy", any(legacy))
            .route("/.well-known/oauth-protected-resource", get(resource_metadata))
            .route("/.well-known/oauth-protected-resource/mcp", get(resource_metadata))
            .route("/.well-known/oauth-authorization-server", get(server_metadata))
            .route("/register", post(register))
            .route("/token", post(token))
            .with_state(Arc::clone(&shared));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        *shared.base.lock().unwrap() = base.clone();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        TestServer { url: format!("{base}/mcp"), shared }
    }

    pub fn base(&self) -> String {
        self.shared.base.lock().unwrap().clone()
    }

    pub fn saw_header(&self, name: &str, value: &str) -> bool {
        self.shared.seen.lock().unwrap().iter().any(|headers| headers.iter().any(|(n, v)| n == name && v == value))
    }

    pub fn revoke_tokens(&self) {
        self.shared.valid_tokens.lock().unwrap().clear();
    }

    /// Makes the refresh token of the authorization server useless from now on.
    pub fn refuse_refresh(&self) {
        self.shared.refuse_refresh.store(true, Ordering::SeqCst);
    }

    pub fn allow_refresh(&self) {
        self.shared.refuse_refresh.store(false, Ordering::SeqCst);
    }
}

fn issue_token(shared: &Shared) -> Value {
    let number = shared.issued.fetch_add(1, Ordering::SeqCst) + 1;
    let access = format!("access-{number}");
    shared.valid_tokens.lock().unwrap().push(access.clone());
    json!({ "access_token": access, "token_type": "Bearer", "expires_in": 3600, "refresh_token": "refresh-1" })
}

fn reply(id: &Value, result: Value) -> Response {
    let body = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    (
        [(header::CONTENT_TYPE, "application/json"), (header::HeaderName::from_static("mcp-session-id"), "session-1")],
        body.to_string(),
    )
        .into_response()
}

fn stream_reply(id: &Value, result: Value) -> Response {
    let message = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    ([(header::CONTENT_TYPE, "text/event-stream")], format!("data: {message}\n\n")).into_response()
}

async fn legacy(State(shared): State<Arc<Shared>>) -> Response {
    shared.seen.lock().unwrap().push(Vec::new());
    StatusCode::NOT_FOUND.into_response()
}

async fn mcp(State(shared): State<Arc<Shared>>, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    let seen = headers
        .iter()
        .map(|(name, value)| (name.as_str().to_string(), value.to_str().unwrap_or_default().to_string()))
        .collect();
    shared.seen.lock().unwrap().push(seen);
    if shared.protected {
        let bearer = headers.get(header::AUTHORIZATION).and_then(|value| value.to_str().ok()).unwrap_or_default();
        let token = bearer.strip_prefix("Bearer ").unwrap_or_default();
        if !shared.valid_tokens.lock().unwrap().iter().any(|valid| valid == token) {
            let challenge = format!(
                "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
                shared.base.lock().unwrap()
            );
            return (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, challenge)]).into_response();
        }
    }
    if method != Method::POST {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let Ok(request) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let id = request["id"].clone();
    match request["method"].as_str() {
        Some("initialize") => {
            let version = request["params"]["protocolVersion"].clone();
            reply(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "vatra-test-http", "version": "1.0.0" }
                }),
            )
        }
        Some("notifications/initialized") => StatusCode::ACCEPTED.into_response(),
        Some("tools/list") => reply(
            &id,
            json!({ "tools": [
                { "name": "echo", "description": "Returns its text",
                  "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } } },
                { "name": "big", "description": "Returns a text of the given size in megabytes",
                  "inputSchema": { "type": "object", "properties": { "megabytes": { "type": "integer" } } } }
            ] }),
        ),
        Some("tools/call") => {
            let arguments = &request["params"]["arguments"];
            let name = request["params"]["name"].as_str().unwrap_or_default();
            if name == "big" {
                let megabytes = arguments["megabytes"].as_u64().unwrap_or(17);
                let text = "a".repeat(megabytes as usize * 1024 * 1024);
                return reply(&id, json!({ "content": [{ "type": "text", "text": text }] }));
            }
            let text = arguments["text"].as_str().unwrap_or_default();
            let result = json!({ "content": [{ "type": "text", "text": text }] });
            if arguments["stream"] == json!(true) { stream_reply(&id, result) } else { reply(&id, result) }
        }
        Some(_) if id.is_null() => StatusCode::ACCEPTED.into_response(),
        _ => {
            let error = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "no such method" } });
            ([(header::CONTENT_TYPE, "application/json")], error.to_string()).into_response()
        }
    }
}

async fn resource_metadata(State(shared): State<Arc<Shared>>) -> Json<Value> {
    let base = shared.base.lock().unwrap().clone();
    Json(json!({ "resource": format!("{base}/mcp"), "authorization_servers": [base] }))
}

async fn server_metadata(State(shared): State<Arc<Shared>>) -> Json<Value> {
    let base = shared.base.lock().unwrap().clone();
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/authorize"),
        "token_endpoint": format!("{base}/token"),
        "registration_endpoint": format!("{base}/register"),
        "response_types_supported": ["code"],
        "code_challenge_methods_supported": ["S256"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": ["none"]
    }))
}

async fn register(State(shared): State<Arc<Shared>>, Json(request): Json<Value>) -> (StatusCode, Json<Value>) {
    shared.registrations.fetch_add(1, Ordering::SeqCst);
    let response = json!({
        "client_id": "dynamic-client",
        "redirect_uris": request["redirect_uris"],
        "token_endpoint_auth_method": "none"
    });
    (StatusCode::CREATED, Json(response))
}

async fn token(State(shared): State<Arc<Shared>>, body: Bytes) -> Response {
    let form: HashMap<String, String> = url::form_urlencoded::parse(&body).into_owned().collect();
    let refused = || (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_grant" }))).into_response();
    match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            let verifier = form.get("code_verifier").cloned().unwrap_or_default();
            let expected = shared.challenge.lock().unwrap().clone();
            let computed = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
            if form.get("code").map(String::as_str) != Some("code-1") || expected.as_deref() != Some(computed.as_str())
            {
                return refused();
            }
            Json(issue_token(&shared)).into_response()
        }
        Some("refresh_token")
            if form.get("refresh_token").map(String::as_str) == Some("refresh-1")
                && !shared.refuse_refresh.load(Ordering::SeqCst) =>
        {
            Json(issue_token(&shared)).into_response()
        }
        _ => refused(),
    }
}

/// Stands in for the browser: it records the authorization address and returns to the callback address with a
/// code. With `forged_first`, a callback with another state comes before the right one.
pub fn approve(shared: Arc<Shared>, forged_first: bool) -> impl Fn(&str) + Sync {
    move |authorization: &str| {
        shared.auth_urls.lock().unwrap().push(authorization.to_string());
        let query: HashMap<String, String> = Url::parse(authorization).unwrap().query_pairs().into_owned().collect();
        *shared.challenge.lock().unwrap() = query.get("code_challenge").cloned();
        let redirect = Url::parse(&query["redirect_uri"]).unwrap();
        let address = format!("{}:{}", redirect.host_str().unwrap(), redirect.port().unwrap());
        let mut requests = Vec::new();
        if forged_first {
            requests.push(callback(redirect.path(), "forged-state"));
        }
        requests.push(callback(redirect.path(), &query["state"]));
        let shared = Arc::clone(&shared);
        // The callbacks are read on another thread, since the login waits for them on the runtime of the caller.
        std::thread::spawn(move || {
            for request in requests {
                let mut stream = TcpStream::connect(&address).unwrap();
                stream.write_all(request.as_bytes()).unwrap();
                let mut answer = String::new();
                let _ = stream.read_to_string(&mut answer);
                shared.callbacks.lock().unwrap().push(answer.lines().next().unwrap_or_default().to_string());
            }
        });
    }
}

fn callback(path: &str, state: &str) -> String {
    format!("GET {path}?code=code-1&state={state} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
}

pub fn http_server(name: &str, url: &str) -> McpServerConfig {
    McpServerConfig { name: name.into(), url: Some(url.into()), ..Default::default() }
}

pub fn oauth_server(name: &str, url: &str) -> McpServerConfig {
    McpServerConfig { oauth: Some(McpOAuth::default()), ..http_server(name, url) }
}
