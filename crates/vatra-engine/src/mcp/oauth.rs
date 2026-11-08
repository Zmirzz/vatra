//! The OAuth login of an MCP server. Vatra runs the flow itself: discovery, PKCE, and a callback on 127.0.0.1.
//! The tokens are kept only in the secret store, under the address of the server.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use rmcp::transport::auth::{AuthError, AuthorizationManager, CredentialStore, OAuthClientConfig, StoredCredentials};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;
use vatra_core::{McpServerConfig, SecretError, SecretStore};

/// How long the user has to finish the login in the browser.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);
const CLIENT_NAME: &str = "Vatra";
const KEY_PREFIX: &str = "mcp-oauth:";
const REQUEST_LIMIT: usize = 8 * 1024;
pub(crate) const NEEDS_LOGIN: &str = "it needs a login; authorize it with mcp.login";

#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("this server has no address to log in to")]
    NoUrl,
    #[error("the authorization server couldn't be found: {0}")]
    Discovery(String),
    #[error("the callback address couldn't be used: {0}")]
    Callback(String),
    #[error("the login wasn't finished in time")]
    Timeout,
    #[error("the browser returned with another state, so the login was refused")]
    StateMismatch,
    #[error("the login was refused: {0}")]
    Refused(String),
    #[error("the authorization address isn't a web address")]
    NotWeb,
    #[error("the token couldn't be obtained: {0}")]
    Token(String),
    #[error(transparent)]
    Store(#[from] SecretError),
    #[error("{NEEDS_LOGIN}")]
    NotLoggedIn,
}

fn secret_key(url: &str) -> String {
    format!("{KEY_PREFIX}{url}")
}

/// Whether the secret store holds a token for this server. The keychain blocks, so it is read off the runtime.
pub async fn has_credentials(url: &str, secrets: Arc<dyn SecretStore>) -> Result<bool, SecretError> {
    let key = secret_key(url);
    let stored = blocking(secrets, move |secrets| secrets.get(&key)).await?;
    Ok(stored
        .and_then(|text| serde_json::from_str::<StoredCredentials>(&text).ok())
        .is_some_and(|credentials| credentials.token_response.is_some()))
}

/// Runs a call of the secret store on a thread that may block.
pub(crate) async fn blocking<T, F>(secrets: Arc<dyn SecretStore>, call: F) -> Result<T, SecretError>
where
    T: Send + 'static,
    F: FnOnce(&dyn SecretStore) -> Result<T, SecretError> + Send + 'static,
{
    tokio::task::spawn_blocking(move || call(secrets.as_ref()))
        .await
        .map_err(|error| SecretError::Backend(error.to_string()))?
}

/// Runs the login of an OAuth server. `open_url` gets the authorization address, which must be opened in the
/// browser; the login waits for the browser to return to the callback address. The tokens are saved in
/// `secrets` only.
pub async fn login(
    server: &McpServerConfig,
    secrets: Arc<dyn SecretStore>,
    open_url: &(dyn Fn(&str) + Sync),
) -> Result<(), OAuthError> {
    let url = server.url.as_deref().ok_or(OAuthError::NoUrl)?;
    let settings = server.oauth.clone().unwrap_or_default();
    let listener = TcpListener::bind(("127.0.0.1", settings.callback_port.unwrap_or(0)))
        .await
        .map_err(|error| OAuthError::Callback(error.to_string()))?;
    let port = listener.local_addr().map_err(|error| OAuthError::Callback(error.to_string()))?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let scopes: Vec<String> =
        settings.scopes.as_deref().unwrap_or_default().split_whitespace().map(String::from).collect();
    let scope_refs: Vec<&str> = scopes.iter().map(String::as_str).collect();

    let slot = Arc::new(Mutex::new(None));
    let mut manager = AuthorizationManager::new(url).await.map_err(|error| OAuthError::Discovery(error.to_string()))?;
    manager.set_credential_store(Slot(Arc::clone(&slot)));
    let resolution = manager.resolve_metadata().await.map_err(|error| OAuthError::Discovery(error.to_string()))?;
    manager.set_metadata(resolution.metadata);
    match settings.client_id {
        Some(client_id) => {
            let config = OAuthClientConfig::new(client_id, redirect_uri.clone()).with_scopes(scopes.clone());
            manager.configure_client(config).map_err(|error| OAuthError::Token(error.to_string()))?;
        }
        None => {
            manager
                .register_client(CLIENT_NAME, &redirect_uri, &scope_refs)
                .await
                .map_err(|error| OAuthError::Token(error.to_string()))?;
        }
    }

    let authorization =
        manager.get_authorization_url(&scope_refs).await.map_err(|error| OAuthError::Token(error.to_string()))?;
    if !authorization.starts_with("https://") && !authorization.starts_with("http://") {
        return Err(OAuthError::NotWeb);
    }
    let state = query_value(&authorization, "state").ok_or(OAuthError::NotWeb)?;
    open_url(&authorization);
    let code = wait_for_callback(listener, &state).await?;

    manager.exchange_code_for_token(&code, &state).await.map_err(|error| OAuthError::Token(error.to_string()))?;
    let credentials = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
    let credentials = credentials.ok_or_else(|| OAuthError::Token("no token came back".into()))?;
    let text = serde_json::to_string(&credentials).map_err(|error| OAuthError::Token(error.to_string()))?;
    let key = secret_key(url);
    blocking(secrets, move |secrets| secrets.set(&key, &text)).await?;
    Ok(())
}

/// The authorization of a server that has a login, ready for its transport. Refreshed tokens are saved back to
/// the secret store.
pub(crate) async fn session(url: &str, secrets: Arc<dyn SecretStore>) -> Result<AuthorizationManager, OAuthError> {
    let mut manager = AuthorizationManager::new(url).await.map_err(|error| OAuthError::Discovery(error.to_string()))?;
    manager.set_credential_store(SecretCredentials { key: secret_key(url), secrets });
    match manager.initialize_from_store().await {
        Ok(true) => Ok(manager),
        Ok(false) => Err(OAuthError::NotLoggedIn),
        Err(error) => Err(OAuthError::Token(error.to_string())),
    }
}

fn query_value(address: &str, name: &str) -> Option<String> {
    let url = Url::parse(address).ok()?;
    url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned())
}

/// Waits for the browser to return to `/callback` and gives back the code. A request to another path, or to the
/// callback with another state, is answered with an error page and ignored, so the login keeps waiting.
async fn wait_for_callback(listener: TcpListener, expected_state: &str) -> Result<String, OAuthError> {
    let outcome = tokio::time::timeout(CALLBACK_TIMEOUT, async {
        loop {
            let (mut stream, _) = listener.accept().await.map_err(|error| OAuthError::Callback(error.to_string()))?;
            let Some(target) = read_target(&mut stream).await else {
                respond(&mut stream, "400 Bad Request", "The request isn't a login.").await;
                continue;
            };
            let parsed = Url::parse(&format!("http://127.0.0.1{target}")).ok();
            let Some(parsed) = parsed.filter(|parsed| parsed.path() == "/callback") else {
                respond(&mut stream, "404 Not Found", "Not found.").await;
                continue;
            };
            let params: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
            if params.get("state").map(String::as_str) != Some(expected_state) {
                respond(&mut stream, "400 Bad Request", "This login doesn't match the request from Vatra.").await;
                continue;
            }
            if let Some(error) = params.get("error") {
                respond(&mut stream, "200 OK", "The login failed.").await;
                return Err(OAuthError::Refused(error.clone()));
            }
            let code = params.get("code").cloned().ok_or_else(|| OAuthError::Refused("no code came back".into()));
            let page = if code.is_ok() { "The login is done. You can close this window." } else { "The login failed." };
            respond(&mut stream, "200 OK", page).await;
            return code;
        }
    })
    .await;
    outcome.map_err(|_| OAuthError::Timeout)?
}

/// The target of the request line, or None when the request is not a GET.
async fn read_target(stream: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") && head.len() < REQUEST_LIMIT {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        head.extend_from_slice(&chunk[..read]);
    }
    let text = String::from_utf8_lossy(&head);
    let mut parts = text.lines().next()?.split_whitespace();
    (parts.next()? == "GET").then(|| parts.next().map(str::to_string)).flatten()
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let page = format!("<!doctype html><meta charset=\"utf-8\"><title>Vatra</title><p>{body}</p>");
    let answer = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
    let _ = stream.write_all(answer.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn storage(error: impl Display) -> AuthError {
    AuthError::CredentialStoreError(error.to_string())
}

/// Keeps the credentials of a server in the secret store, under the key of its address.
struct SecretCredentials {
    key: String,
    secrets: Arc<dyn SecretStore>,
}

#[async_trait]
impl CredentialStore for SecretCredentials {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let key = self.key.clone();
        let text = blocking(Arc::clone(&self.secrets), move |secrets| secrets.get(&key)).await.map_err(storage)?;
        Ok(text.and_then(|text| serde_json::from_str(&text).ok()))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let text = serde_json::to_string(&credentials).map_err(storage)?;
        let key = self.key.clone();
        blocking(Arc::clone(&self.secrets), move |secrets| secrets.set(&key, &text)).await.map_err(storage)
    }

    async fn clear(&self) -> Result<(), AuthError> {
        let key = self.key.clone();
        blocking(Arc::clone(&self.secrets), move |secrets| secrets.delete(&key)).await.map_err(storage)
    }
}

/// Holds the credentials of the login that is running, so that they can be saved once it succeeds.
struct Slot(Arc<Mutex<Option<StoredCredentials>>>);

#[async_trait]
impl CredentialStore for Slot {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        Ok(self.0.lock().unwrap_or_else(PoisonError::into_inner).clone())
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(credentials);
        Ok(())
    }

    async fn clear(&self) -> Result<(), AuthError> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = None;
        Ok(())
    }
}
