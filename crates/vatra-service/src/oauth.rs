//! Signing in to Google and Microsoft for mail and calendar, without an app password. The person approves in
//! their browser; Vatra receives the answer on a one-time address on this computer (PKCE, no secret to leak), keeps
//! the long-lived refresh token in the keychain and trades it for short-lived access tokens when it needs one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use vatra_core::{SecretStore, Store};

use crate::Service;
use crate::connectors::{SaveRequest, connector_secret};
use crate::setup::{failed, parse};

/// A stored password that starts like this is an access token, to be used with XOAUTH2 or as a bearer token.
pub const TOKEN_PREFIX: &str = "oauth2:";
const FLOW_TIMEOUT: Duration = Duration::from_secs(300);
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Google,
    Microsoft,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Email,
    Calendar,
}

impl Provider {
    pub fn parse(id: &str) -> Option<Provider> {
        match id {
            "google" => Some(Provider::Google),
            "microsoft" => Some(Provider::Microsoft),
            _ => None,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Provider::Google => "google",
            Provider::Microsoft => "microsoft",
        }
    }
    fn auth_url(self) -> &'static str {
        match self {
            Provider::Google => "https://accounts.google.com/o/oauth2/v2/auth",
            Provider::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
        }
    }
    fn token_url(self) -> &'static str {
        match self {
            Provider::Google => "https://oauth2.googleapis.com/token",
            Provider::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/token",
        }
    }
    fn scope(self, purpose: Purpose) -> &'static str {
        match (self, purpose) {
            (Provider::Google, Purpose::Email) => "openid email https://mail.google.com/",
            (Provider::Google, Purpose::Calendar) => "openid email https://www.googleapis.com/auth/calendar",
            (Provider::Microsoft, _) => {
                "openid email offline_access https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send"
            }
        }
    }
    /// The id compiled into the release, when the maintainers registered one.
    fn built_in_client(self) -> Option<&'static str> {
        match self {
            Provider::Google => option_env!("VATRA_GOOGLE_CLIENT_ID"),
            Provider::Microsoft => option_env!("VATRA_MICROSOFT_CLIENT_ID"),
        }
        .filter(|id| !id.is_empty())
    }
}

#[derive(Clone)]
struct Client {
    id: String,
    secret: Option<String>,
}

fn client(store: &Store, provider: Provider) -> Option<Client> {
    let setting = |suffix: &str| {
        store
            .app_setting(&format!("oauth.{}.{suffix}", provider.id()))
            .ok()
            .flatten()
            .filter(|value| !value.trim().is_empty())
    };
    let id = setting("client_id").or_else(|| provider.built_in_client().map(String::from))?;
    let secret = setting("client_secret").or_else(|| {
        match provider {
            Provider::Google => option_env!("VATRA_GOOGLE_CLIENT_SECRET"),
            Provider::Microsoft => None,
        }
        .filter(|secret| !secret.is_empty())
        .map(String::from)
    });
    Some(Client { id, secret })
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(HTTP_TIMEOUT).build().expect("couldn't create the HTTP client")
}

/// The sign-in string for IMAP and SMTP.
pub fn xoauth2(user: &str, token: &str) -> String {
    STANDARD.encode(format!("user={user}\x01auth=Bearer {token}\x01\x01"))
}

/// The access token inside a stored password, when it is one.
pub fn access_token(password: &str) -> Option<&str> {
    password.strip_prefix(TOKEN_PREFIX)
}

#[derive(Debug, PartialEq)]
pub(crate) struct Tokens {
    pub access: String,
    pub refresh: Option<String>,
    pub expires_in: u64,
    pub email: Option<String>,
}

pub(crate) fn parse_tokens(body: &Value) -> Result<Tokens, String> {
    let Some(access) = body["access_token"].as_str().filter(|token| !token.is_empty()) else {
        return Err("The sign-in wasn't accepted. Try again.".into());
    };
    Ok(Tokens {
        access: access.to_string(),
        refresh: body["refresh_token"].as_str().filter(|token| !token.is_empty()).map(String::from),
        expires_in: body["expires_in"].as_u64().unwrap_or(3000),
        email: body["id_token"].as_str().and_then(email_of),
    })
}

/// The address in an identity token. The token comes straight from the provider over TLS, so it isn't verified again.
fn email_of(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    ["email", "preferred_username"]
        .iter()
        .filter_map(|key| claims[key].as_str())
        .find(|value| vatra_core::valid_address(value))
        .map(String::from)
}

async fn token_request(provider: Provider, client: &Client, mut form: Vec<(&str, String)>) -> Result<Tokens, String> {
    form.push(("client_id", client.id.clone()));
    if let Some(secret) = &client.secret {
        form.push(("client_secret", secret.clone()));
    }
    let response = http()
        .post(provider.token_url())
        .form(&form)
        .send()
        .await
        .map_err(|_| "The sign-in service didn't answer. Check your connection.".to_string())?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(match body["error"].as_str() {
            Some("invalid_grant") => "The sign-in ended. Sign in again in Settings.".into(),
            _ => "The sign-in wasn't accepted. Try again.".into(),
        });
    }
    parse_tokens(&body)
}

// ---- access tokens for saved accounts -------------------------------------------------------------------------

fn cache() -> &'static Mutex<HashMap<String, (String, Instant)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (String, Instant)>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// What a saved account signs in with. An account that uses a password gets it back unchanged; one that signed in
/// with Google or Microsoft gets a fresh access token, marked with [`TOKEN_PREFIX`].
pub async fn credential(store: &Store, secrets: &dyn SecretStore, connector_id: &str, settings: &Value, stored: String) -> Result<String, String> {
    let Some(provider) = settings.get("auth").and_then(Value::as_str).and_then(Provider::parse) else {
        return Ok(stored);
    };
    let cached = cache().lock().ok().and_then(|cache| cache.get(connector_id).cloned());
    if let Some((token, _)) = cached.filter(|(_, until)| *until > Instant::now()) {
        return Ok(format!("{TOKEN_PREFIX}{token}"));
    }
    if stored.is_empty() {
        return Err("This account needs to be signed in again in Settings.".into());
    }
    let client = client(store, provider).ok_or("Sign-in with this service isn't set up. Add its details in Settings.")?;
    let tokens = token_request(
        provider,
        &client,
        vec![("grant_type", "refresh_token".into()), ("refresh_token", stored.clone())],
    )
    .await?;
    // Microsoft hands out a new refresh token each time; keep the newest.
    if let Some(next) = tokens.refresh.as_ref().filter(|next| **next != stored) {
        let _ = secrets.set(&connector_secret(connector_id, "password"), next);
    }
    let until = Instant::now() + Duration::from_secs(tokens.expires_in.saturating_sub(120).max(60));
    if let Ok(mut cache) = cache().lock() {
        cache.insert(connector_id.to_string(), (tokens.access.clone(), until));
    }
    Ok(format!("{TOKEN_PREFIX}{}", tokens.access))
}

// ---- the sign-in in the browser -------------------------------------------------------------------------------

enum Flow {
    Pending,
    Done { refresh: String, address: String, provider: Provider },
    Failed(String),
}

fn flows() -> &'static Mutex<HashMap<String, Arc<Mutex<Flow>>>> {
    static FLOWS: OnceLock<Mutex<HashMap<String, Arc<Mutex<Flow>>>>> = OnceLock::new();
    FLOWS.get_or_init(Default::default)
}

fn random_token(bytes: usize) -> String {
    let data: Vec<u8> = (0..bytes).map(|_| rand::random::<u8>()).collect();
    URL_SAFE_NO_PAD.encode(data)
}

const PAGE: &str = "<!doctype html><meta charset=utf-8><title>Vatra</title><body style=\"font-family:system-ui;margin:4rem auto;max-width:28rem;text-align:center\"><h2>You can close this tab</h2><p>Go back to Vatra to finish.</p></body>";

fn query_of(request: &str) -> HashMap<String, String> {
    let target = request.lines().next().unwrap_or("").split_whitespace().nth(1).unwrap_or("");
    url::Url::parse(&format!("http://127.0.0.1{target}"))
        .map(|url| url.query_pairs().map(|(key, value)| (key.into_owned(), value.into_owned())).collect())
        .unwrap_or_default()
}

/// Starts a sign-in: listens on a one-time address, and returns the page the person must open.
pub async fn begin(store: &Store, provider: Provider, purpose: Purpose) -> Result<(String, String), String> {
    let client = client(store, provider).ok_or("Sign-in with this service isn't set up yet.")?;
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|_| "Vatra couldn't listen for the sign-in.".to_string())?;
    let port = listener.local_addr().map_err(|error| error.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let verifier = random_token(48);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_token(24);
    let mut url = url::Url::parse(provider.auth_url()).map_err(|error| error.to_string())?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("client_id", &client.id)
            .append_pair("redirect_uri", &redirect)
            .append_pair("response_type", "code")
            .append_pair("scope", provider.scope(purpose))
            .append_pair("state", &state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        if provider == Provider::Google {
            query.append_pair("access_type", "offline").append_pair("prompt", "consent");
        }
    }
    let flow_id = random_token(12);
    let flow = Arc::new(Mutex::new(Flow::Pending));
    flows().lock().map_err(|_| "Sign-in is unavailable.".to_string())?.insert(flow_id.clone(), Arc::clone(&flow));
    tokio::spawn(async move {
        let outcome = tokio::time::timeout(FLOW_TIMEOUT, listen(listener, &state, provider, &client, &redirect, &verifier)).await;
        let result = match outcome {
            Ok(Ok((refresh, address))) => Flow::Done { refresh, address, provider },
            Ok(Err(message)) => Flow::Failed(message),
            Err(_) => Flow::Failed("The sign-in took too long. Try again.".into()),
        };
        if let Ok(mut slot) = flow.lock() {
            *slot = result;
        }
    });
    Ok((flow_id, url.to_string()))
}

async fn listen(
    listener: TcpListener,
    state: &str,
    provider: Provider,
    client: &Client,
    redirect: &str,
    verifier: &str,
) -> Result<(String, String), String> {
    loop {
        let (mut stream, _) = listener.accept().await.map_err(|_| "The sign-in connection failed.".to_string())?;
        let mut buffer = vec![0u8; 8192];
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer)).await.ok().and_then(Result::ok).unwrap_or(0);
        let request = String::from_utf8_lossy(&buffer[..read]).to_string();
        let query = query_of(&request);
        // Anything else that touches the address (a favicon, a scan) is ignored.
        if query.get("state").map(String::as_str) != Some(state) && !query.contains_key("error") {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            continue;
        }
        let page = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{PAGE}", PAGE.len());
        let _ = stream.write_all(page.as_bytes()).await;
        if query.contains_key("error") || query.get("state").map(String::as_str) != Some(state) {
            return Err("The sign-in was cancelled.".into());
        }
        let code = query.get("code").cloned().ok_or("The sign-in wasn't completed.")?;
        let tokens = token_request(
            provider,
            client,
            vec![
                ("grant_type", "authorization_code".into()),
                ("code", code),
                ("redirect_uri", redirect.into()),
                ("code_verifier", verifier.into()),
            ],
        )
        .await?;
        let refresh = tokens.refresh.ok_or("The sign-in didn't allow Vatra to stay signed in. Try again.")?;
        let address = tokens.email.ok_or("The sign-in didn't tell Vatra the address of the account.")?;
        return Ok((refresh, address));
    }
}

/// The saved settings of an account that signed in with `provider`.
pub(crate) fn account_settings(provider: Provider, purpose: Purpose, address: &str) -> Value {
    match (purpose, provider) {
        (Purpose::Email, Provider::Google) => json!({
            "auth": "google", "address": address, "username": address,
            "imap_host": "imap.gmail.com", "imap_port": 993, "imap_security": "tls",
            "smtp_host": "smtp.gmail.com", "smtp_port": 465, "smtp_security": "tls",
        }),
        (Purpose::Email, Provider::Microsoft) => json!({
            "auth": "microsoft", "address": address, "username": address,
            "imap_host": "outlook.office365.com", "imap_port": 993, "imap_security": "tls",
            "smtp_host": "smtp.office365.com", "smtp_port": 587, "smtp_security": "starttls",
        }),
        (Purpose::Calendar, _) => json!({
            "auth": "google", "username": address,
            "url": format!("https://apidata.googleapis.com/caldav/v2/{address}/events"),
        }),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartRequest {
    provider: String,
    purpose: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FinishRequest {
    flow_id: String,
    purpose: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigureRequest {
    provider: String,
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
}

pub fn handles(method: &str) -> bool {
    method.starts_with("oauth.")
}

type Reply = Result<Value, (i64, String)>;

fn purpose(text: &str) -> Result<Purpose, (i64, String)> {
    match text {
        "email" => Ok(Purpose::Email),
        "calendar" => Ok(Purpose::Calendar),
        _ => Err(failed("Unknown sign-in purpose.")),
    }
}

pub(crate) async fn handle(service: &Arc<Service>, method: &str, params: Value) -> Reply {
    match method {
        "oauth.providers" => Ok(json!([
            {"id": "google", "ready": client(service.store(), Provider::Google).is_some(), "email": true, "calendar": true},
            {"id": "microsoft", "ready": client(service.store(), Provider::Microsoft).is_some(), "email": true, "calendar": false},
        ])),
        "oauth.configure" => {
            let request: ConfigureRequest = parse(params)?;
            let provider = Provider::parse(&request.provider).ok_or_else(|| failed("Unknown service."))?;
            let id = request.client_id.trim();
            if id.chars().any(char::is_control) || id.len() > 300 {
                return Err(failed("That ID isn't valid."));
            }
            let store = service.store();
            let key = |suffix: &str| format!("oauth.{}.{suffix}", provider.id());
            store.set_app_setting(&key("client_id"), Some(id).filter(|id| !id.is_empty())).map_err(failed)?;
            if let Some(secret) = request.client_secret.as_deref().map(str::trim) {
                store.set_app_setting(&key("client_secret"), Some(secret).filter(|secret| !secret.is_empty())).map_err(failed)?;
            }
            Ok(json!({}))
        }
        "oauth.start" => {
            let request: StartRequest = parse(params)?;
            let provider = Provider::parse(&request.provider).ok_or_else(|| failed("Unknown service."))?;
            let purpose = purpose(&request.purpose)?;
            if provider == Provider::Microsoft && purpose == Purpose::Calendar {
                return Err(failed("Microsoft calendars can't be added this way yet."));
            }
            let (flow_id, url) = begin(service.store(), provider, purpose).await.map_err(failed)?;
            Ok(json!({"flowId": flow_id, "url": url}))
        }
        "oauth.poll" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Poll {
                flow_id: String,
            }
            let request: Poll = parse(params)?;
            let flow = flows().lock().map_err(|_| failed("Sign-in is unavailable."))?.get(&request.flow_id).cloned();
            let Some(flow) = flow else { return Err(failed("This sign-in is no longer open.")) };
            let slot = flow.lock().map_err(|_| failed("Sign-in is unavailable."))?;
            Ok(match &*slot {
                Flow::Pending => json!({"state": "pending"}),
                Flow::Done { address, .. } => json!({"state": "done", "address": address}),
                Flow::Failed(message) => json!({"state": "failed", "error": message}),
            })
        }
        "oauth.finish" => {
            let request: FinishRequest = parse(params)?;
            let purpose = purpose(&request.purpose)?;
            let flow = flows().lock().map_err(|_| failed("Sign-in is unavailable."))?.remove(&request.flow_id);
            let Some(flow) = flow else { return Err(failed("This sign-in is no longer open.")) };
            let (refresh, address, provider) = match &*flow.lock().map_err(|_| failed("Sign-in is unavailable."))? {
                Flow::Done { refresh, address, provider } => (refresh.clone(), address.clone(), *provider),
                Flow::Pending => return Err(failed("The sign-in isn't finished yet.")),
                Flow::Failed(message) => return Err(failed(message)),
            };
            let kind = match purpose {
                Purpose::Email => "email",
                Purpose::Calendar => "caldav",
            };
            let name = request.name.filter(|name| !name.trim().is_empty()).unwrap_or_else(|| address.clone());
            let secrets = std::collections::BTreeMap::from([("password".to_string(), refresh)]);
            let save = SaveRequest { id: None, kind: kind.into(), name, settings: account_settings(provider, purpose, &address), secrets };
            let registry = service.connector_registry();
            let id = crate::connectors::save(service.store(), service.agents().secrets().as_ref(), &registry, save, crate::models::now())
                .map_err(failed)?;
            Ok(json!({"id": id, "address": address}))
        }
        _ => Err((-32601, format!("unknown method `{method}`"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xoauth2_has_the_shape_servers_expect() {
        let decoded = String::from_utf8(STANDARD.decode(xoauth2("a@b.c", "tok")).unwrap()).unwrap();
        assert_eq!(decoded, "user=a@b.c\x01auth=Bearer tok\x01\x01");
    }

    #[test]
    fn a_stored_password_is_only_a_token_with_the_prefix() {
        assert_eq!(access_token("oauth2:abc"), Some("abc"));
        assert_eq!(access_token("hunter2"), None);
    }

    #[test]
    fn tokens_are_read_with_the_address_from_the_identity_token() {
        let claims = URL_SAFE_NO_PAD.encode(r#"{"email":"me@example.com"}"#);
        let body = json!({"access_token": "a", "refresh_token": "r", "expires_in": 3599, "id_token": format!("h.{claims}.s")});
        let tokens = parse_tokens(&body).unwrap();
        assert_eq!(tokens, Tokens { access: "a".into(), refresh: Some("r".into()), expires_in: 3599, email: Some("me@example.com".into()) });
        assert!(parse_tokens(&json!({"error": "invalid_grant"})).is_err());
    }

    #[test]
    fn the_redirect_is_read_for_its_state_and_code() {
        let query = query_of("GET /?code=abc&state=xyz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        assert_eq!(query.get("code").map(String::as_str), Some("abc"));
        assert_eq!(query.get("state").map(String::as_str), Some("xyz"));
        assert!(!query_of("GET /favicon.ico HTTP/1.1").contains_key("state"));
    }

    #[test]
    fn each_kind_of_account_gets_the_servers_of_its_provider() {
        let mail = account_settings(Provider::Microsoft, Purpose::Email, "me@outlook.com");
        assert_eq!(mail["imap_host"], "outlook.office365.com");
        assert_eq!(mail["smtp_security"], "starttls");
        let calendar = account_settings(Provider::Google, Purpose::Calendar, "me@gmail.com");
        assert_eq!(calendar["url"], "https://apidata.googleapis.com/caldav/v2/me@gmail.com/events");
        assert_eq!(calendar["auth"], "google");
    }
}
