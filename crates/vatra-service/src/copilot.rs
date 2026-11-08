//! GitHub Copilot as a model service. The person signs in on github.com with a short code; the GitHub token that
//! results is kept in the keychain like any other key, and each request trades it for a short-lived Copilot token.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vatra_engine::model::{
    Completion, ContextWindow, ModelConfig, ModelDriver, ModelError, ModelRequest, OpenAiDriver,
};

pub const BASE_URL: &str = "https://api.githubcopilot.com";
/// The public client id GitHub's own Copilot editor extensions sign in with.
const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const DEVICE_URL: &str = "https://github.com/login/device/code";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const EXCHANGE_URL: &str = "https://api.github.com/copilot_internal/v2/token";
const TIMEOUT: Duration = Duration::from_secs(20);
/// A token this close to its end is renewed before the request.
const RENEW_MARGIN: i64 = 120;

fn client_headers() -> Vec<(String, String)> {
    [
        ("Editor-Version", "vscode/1.99.0"),
        ("Editor-Plugin-Version", "copilot-chat/0.26.0"),
        ("Copilot-Integration-Id", "vscode-chat"),
        ("User-Agent", "GitHubCopilotChat/0.26.0"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect()
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs() as i64).unwrap_or(0)
}

/// What the person does on github.com to let Vatra use their Copilot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Poll {
    /// Not approved yet; ask again after `interval` seconds.
    Waiting { interval: u64 },
    /// Approved. The token goes to the keychain and is never sent back to the page again.
    Done { token: String },
    Expired,
    Denied,
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(TIMEOUT).build().expect("couldn't create the HTTP client")
}

pub async fn start() -> Result<DeviceCode, String> {
    let response = http()
        .post(DEVICE_URL)
        .header("Accept", "application/json")
        .json(&json!({"client_id": CLIENT_ID, "scope": "read:user"}))
        .send()
        .await
        .map_err(|error| error.without_url().to_string())?;
    if !response.status().is_success() {
        return Err("GitHub didn't give a sign-in code. Try again in a moment.".into());
    }
    let body: Value = response.json().await.map_err(|_| "GitHub's answer wasn't understood.".to_string())?;
    parse_device(&body)
}

pub(crate) fn parse_device(body: &Value) -> Result<DeviceCode, String> {
    let text = |key: &str| body[key].as_str().filter(|value| !value.is_empty()).map(String::from);
    match (text("device_code"), text("user_code"), text("verification_uri")) {
        (Some(device_code), Some(user_code), Some(verification_uri)) => Ok(DeviceCode {
            device_code,
            user_code,
            verification_uri,
            interval: body["interval"].as_u64().unwrap_or(5).clamp(1, 60),
            expires_in: body["expires_in"].as_u64().unwrap_or(900),
        }),
        _ => Err("GitHub's answer wasn't understood.".into()),
    }
}

pub async fn poll(device_code: &str) -> Result<Poll, String> {
    let response = http()
        .post(TOKEN_URL)
        .header("Accept", "application/json")
        .json(&json!({
            "client_id": CLIENT_ID,
            "device_code": device_code,
            "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
        }))
        .send()
        .await
        .map_err(|error| error.without_url().to_string())?;
    let body: Value = response.json().await.map_err(|_| "GitHub's answer wasn't understood.".to_string())?;
    parse_poll(&body)
}

pub(crate) fn parse_poll(body: &Value) -> Result<Poll, String> {
    if let Some(token) = body["access_token"].as_str().filter(|token| !token.is_empty()) {
        return Ok(Poll::Done { token: token.to_string() });
    }
    match body["error"].as_str() {
        Some("authorization_pending") => Ok(Poll::Waiting { interval: body["interval"].as_u64().unwrap_or(5).clamp(1, 60) }),
        Some("slow_down") => Ok(Poll::Waiting { interval: body["interval"].as_u64().unwrap_or(10).clamp(5, 60) }),
        Some("expired_token") => Ok(Poll::Expired),
        Some("access_denied") => Ok(Poll::Denied),
        _ => Err("GitHub's answer wasn't understood.".into()),
    }
}

/// A Copilot token and the second it stops working.
#[derive(Clone)]
struct Session {
    token: String,
    expires_at: i64,
}

async fn exchange(github_token: &str) -> Result<Session, String> {
    let mut request = http().get(EXCHANGE_URL).header("Authorization", format!("token {github_token}"));
    for (name, value) in client_headers() {
        request = request.header(name, value);
    }
    let response = request.send().await.map_err(|error| error.without_url().to_string())?;
    match response.status().as_u16() {
        200 => {}
        401 | 403 | 404 => {
            return Err("This GitHub account has no Copilot, or the sign-in ended. Sign in again in Settings.".into());
        }
        _ => return Err("GitHub Copilot isn't answering. Try again in a moment.".into()),
    }
    let body: Value = response.json().await.map_err(|_| "GitHub's answer wasn't understood.".to_string())?;
    match (body["token"].as_str(), body["expires_at"].as_i64()) {
        (Some(token), Some(expires_at)) => Ok(Session { token: token.to_string(), expires_at }),
        _ => Err("GitHub's answer wasn't understood.".into()),
    }
}

/// The models this Copilot plan offers that can work with tools: id and name.
pub async fn models(github_token: &str) -> Result<Vec<Value>, String> {
    let session = exchange(github_token).await?;
    let mut request = http().get(format!("{BASE_URL}/models")).bearer_auth(&session.token);
    for (name, value) in client_headers() {
        request = request.header(name, value);
    }
    let body: Value = request
        .send()
        .await
        .map_err(|error| error.without_url().to_string())?
        .json()
        .await
        .map_err(|_| "GitHub's answer wasn't understood.".to_string())?;
    Ok(parse_models(&body))
}

pub(crate) fn parse_models(body: &Value) -> Vec<Value> {
    body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|model| model["model_picker_enabled"].as_bool().unwrap_or(true))
        .filter(|model| model["capabilities"]["type"].as_str().is_none_or(|kind| kind == "chat"))
        .map(|model| {
            json!({
                "id": model["id"],
                "name": model["name"],
                "tools": model["capabilities"]["supports"]["tool_calls"].as_bool().unwrap_or(false),
                "free": false,
                "contextLength": model["capabilities"]["limits"]["max_context_window_tokens"],
            })
        })
        .collect()
}

/// Talks to Copilot with the person's GitHub token, renewing the short-lived Copilot token when it runs out.
pub struct CopilotDriver {
    github_token: String,
    model: String,
    context_window: Option<u64>,
    session: tokio::sync::Mutex<Option<Session>>,
    window: Mutex<Option<ContextWindow>>,
}

impl CopilotDriver {
    pub fn new(github_token: String, model: String, context_window: Option<u64>) -> CopilotDriver {
        CopilotDriver { github_token, model, context_window, session: Default::default(), window: Default::default() }
    }

    async fn token(&self) -> Result<String, ModelError> {
        let mut session = self.session.lock().await;
        if let Some(current) = session.as_ref().filter(|current| current.expires_at - RENEW_MARGIN > now()) {
            return Ok(current.token.clone());
        }
        let fresh = exchange(&self.github_token).await.map_err(ModelError::Other)?;
        let token = fresh.token.clone();
        *session = Some(fresh);
        Ok(token)
    }
}

#[async_trait]
impl ModelDriver for CopilotDriver {
    async fn stream(
        &self,
        request: &ModelRequest,
        on_text: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<Completion, ModelError> {
        let token = self.token().await?;
        let inner = OpenAiDriver::new(ModelConfig {
            base_url: BASE_URL.into(),
            model: self.model.clone(),
            api_key: Some(token),
            context_window: self.context_window.or(Some(128_000)),
        })
        .with_headers(client_headers());
        let completion = inner.stream(request, on_text).await;
        if let Ok(mut window) = self.window.lock() {
            *window = Some(inner.context_window());
        }
        // A rejected token is dropped so the next try gets a new one.
        if matches!(&completion, Err(error) if error.to_string().contains("401")) {
            *self.session.lock().await = None;
        }
        completion
    }

    fn context_window(&self) -> ContextWindow {
        self.window
            .lock()
            .ok()
            .and_then(|window| *window)
            .unwrap_or(ContextWindow { tokens: self.context_window.unwrap_or(128_000), estimated: self.context_window.is_none() })
    }

    fn supports_images(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_answer_needs_all_three_fields() {
        let ok = parse_device(&json!({"device_code": "d", "user_code": "ABCD-1234", "verification_uri": "https://github.com/login/device", "interval": 5, "expires_in": 900})).unwrap();
        assert_eq!(ok.user_code, "ABCD-1234");
        assert!(parse_device(&json!({"device_code": "d"})).is_err());
    }

    #[test]
    fn polling_tells_waiting_from_done_expired_and_denied() {
        assert_eq!(parse_poll(&json!({"access_token": "gho_x"})).unwrap(), Poll::Done { token: "gho_x".into() });
        assert!(matches!(parse_poll(&json!({"error": "authorization_pending"})), Ok(Poll::Waiting { .. })));
        assert!(matches!(parse_poll(&json!({"error": "slow_down", "interval": 10})), Ok(Poll::Waiting { interval: 10 })));
        assert_eq!(parse_poll(&json!({"error": "expired_token"})).unwrap(), Poll::Expired);
        assert_eq!(parse_poll(&json!({"error": "access_denied"})).unwrap(), Poll::Denied);
        assert!(parse_poll(&json!({"error": "weird"})).is_err());
    }

    #[test]
    fn model_list_keeps_chat_models_and_marks_tool_support() {
        let body = json!({"data": [
            {"id": "gpt-4.1", "name": "GPT-4.1", "model_picker_enabled": true, "capabilities": {"type": "chat", "supports": {"tool_calls": true}, "limits": {"max_context_window_tokens": 128000}}},
            {"id": "embed", "name": "Embed", "capabilities": {"type": "embeddings"}},
            {"id": "hidden", "name": "Hidden", "model_picker_enabled": false, "capabilities": {"type": "chat"}}
        ]});
        let models = parse_models(&body);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["id"], "gpt-4.1");
        assert_eq!(models[0]["tools"], true);
    }
}
