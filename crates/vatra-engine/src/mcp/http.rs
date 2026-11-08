//! MCP servers reached over HTTP with the Streamable HTTP transport. The headers come from the config and from
//! `headers_helper`, and an OAuth server also gets the token of Vatra's own login.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderName, HeaderValue, RETRY_AFTER, WWW_AUTHENTICATE};
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::service::RunningService;
use rmcp::transport::auth::AuthClient;
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, HttpStatusError, InsufficientScopeError, StreamableHttpClient, StreamableHttpClientTransport,
    StreamableHttpClientTransportConfig, StreamableHttpError, StreamableHttpPostResponse,
};
use rmcp::{RoleClient, ServiceExt};
use sse_stream::{Sse, SseStream};
use vatra_core::{McpServerConfig, SecretStore};

use super::expand::{ExpandVars, expand};
use super::oauth::{self, NEEDS_LOGIN};

/// The longest a `headers_helper` may run before its headers are given up.
const HELPER_TIMEOUT: Duration = Duration::from_secs(10);
/// The most one response to a POST may be. rmcp's own limit applies only to each event of a stream.
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const TOO_LARGE: &str = "the response is larger than 16 MiB";
const SESSION_HEADER: &str = "mcp-session-id";

type HttpError = StreamableHttpError<reqwest::Error>;

/// Starts a session with an HTTP server. The headers, and the login for an OAuth server, are set up first.
pub(super) async fn connect(
    config: &McpServerConfig,
    url: &str,
    vars: &ExpandVars,
    secrets: Arc<dyn SecretStore>,
) -> Result<RunningService<RoleClient, ()>, String> {
    let values = expand_all(config.headers.values(), vars)?;
    let mut headers: BTreeMap<String, String> = config.headers.keys().cloned().zip(values).collect();
    if let Some(helper) = &config.headers_helper {
        let command = expand_all([helper], vars)?.remove(0);
        headers.extend(run_helper(&command, vars).await?);
    }
    let transport = StreamableHttpClientTransportConfig::with_uri(url).custom_headers(header_map(headers)?);
    let client = CappedClient::new();
    let refusal = client.refusal();
    let result = match config.oauth {
        None => serve(client, transport).await,
        Some(_) => {
            let manager = oauth::session(url, secrets).await.map_err(|error| error.to_string())?;
            serve(AuthClient::new(client, manager), transport).await
        }
    };
    result.map_err(|message| explain(message, last_refusal(&refusal), config.oauth.is_some()))
}

async fn serve<C: StreamableHttpClient>(
    client: C,
    config: StreamableHttpClientTransportConfig,
) -> Result<RunningService<RoleClient, ()>, String> {
    let transport = StreamableHttpClientTransport::with_client(client, config);
    ().serve(transport).await.map_err(|error| error.to_string())
}

/// Says what a refused connection means, when the status of the refusal tells more than rmcp's message.
fn explain(message: String, refused: Option<u16>, oauth: bool) -> String {
    match refused {
        Some(401) if oauth => NEEDS_LOGIN.into(),
        Some(status @ (404 | 405)) => format!(
            "this server doesn't speak Streamable HTTP: it answered HTTP {status} to the MCP request, \
             and SSE-only servers aren't supported"
        ),
        _ => message,
    }
}

fn last_refusal(refusal: &Mutex<Option<u16>>) -> Option<u16> {
    *refusal.lock().unwrap_or_else(PoisonError::into_inner)
}

/// rmcp's reqwest client, except that the responses to POST requests are read here, so that a JSON body is capped
/// at 16 MiB. Requests that read a stream or end a session go to rmcp's client unchanged.
#[derive(Clone)]
struct CappedClient {
    inner: reqwest::Client,
    /// The status of the last request the server refused, cleared by the next one it accepted.
    refusal: Arc<Mutex<Option<u16>>>,
}

impl CappedClient {
    fn new() -> CappedClient {
        CappedClient { inner: reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().expect("static MCP HTTP client configuration"), refusal: Arc::default() }
    }

    fn refusal(&self) -> Arc<Mutex<Option<u16>>> {
        Arc::clone(&self.refusal)
    }

    fn set_refusal(&self, status: Option<u16>) {
        *self.refusal.lock().unwrap_or_else(PoisonError::into_inner) = status;
    }
}

fn too_large() -> HttpError {
    StreamableHttpError::UnexpectedServerResponse(Cow::Borrowed(TOO_LARGE))
}

/// The body of a response, unless it is over the limit. The length is checked before the body is read.
async fn read_capped(mut response: reqwest::Response) -> Result<Vec<u8>, HttpError> {
    if response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) {
        return Err(too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(StreamableHttpError::Client)? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

impl StreamableHttpClient for CappedClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        let body = serde_json::to_vec(&message)?;
        let mut request = self
            .inner
            .post(uri.as_ref())
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .body(body);
        if let Some(session) = &session_id {
            request = request.header(SESSION_HEADER, session.as_ref());
        }
        if let Some(token) = auth_token {
            request = request.bearer_auth(token);
        }
        for (name, value) in custom_headers {
            request = request.header(name, value);
        }
        let response = request.send().await.map_err(StreamableHttpError::Client)?;
        let status = response.status();
        let headers = response.headers();
        let session_out = headers.get(SESSION_HEADER).and_then(|value| value.to_str().ok()).map(str::to_string);
        let challenge = headers.get(WWW_AUTHENTICATE).and_then(|value| value.to_str().ok()).map(str::to_string);
        let retry_after = headers.get(RETRY_AFTER).and_then(|value| value.to_str().ok()).map(str::to_string);
        let content_type = headers.get(CONTENT_TYPE).and_then(|value| value.to_str().ok()).map(str::to_string);
        let is_json = content_type.as_deref().is_some_and(|ct| ct.starts_with("application/json"));
        let is_stream = content_type.as_deref().is_some_and(|ct| ct.starts_with("text/event-stream"));
        self.set_refusal((!status.is_success()).then_some(status.as_u16()));

        if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if let Some(challenge) =
            challenge.filter(|_| status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN)
        {
            return Err(if status == StatusCode::UNAUTHORIZED {
                StreamableHttpError::AuthRequired(AuthRequiredError::new(challenge))
            } else {
                StreamableHttpError::InsufficientScope(InsufficientScopeError::new(challenge, None))
            });
        }
        if status == StatusCode::NOT_FOUND && session_id.is_some() {
            return Err(StreamableHttpError::SessionExpired);
        }
        if !status.is_success() {
            if is_json {
                // A JSON-RPC error that the server sent with its refusal is the answer to the request.
                let body = read_capped(response).await?;
                if let Ok(error @ ServerJsonRpcMessage::Error(_)) = serde_json::from_slice(&body) {
                    return Ok(StreamableHttpPostResponse::Json(error, session_out));
                }
            }
            return Err(StreamableHttpError::HttpStatus(HttpStatusError::new(status.as_u16(), retry_after)));
        }
        let answers = matches!(message, ClientJsonRpcMessage::Request(_));
        if !answers && response.content_length() == Some(0) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if is_stream {
            // The events are read as they come, and the stream ends when the total passes the limit.
            let mut total = 0usize;
            let bytes = response.bytes_stream().map(move |chunk| {
                let chunk = chunk.map_err(io::Error::other)?;
                total += chunk.len();
                if total > MAX_RESPONSE_BYTES {
                    return Err(io::Error::other(TOO_LARGE));
                }
                Ok(chunk)
            });
            let events: BoxStream<'static, Result<Sse, sse_stream::Error>> =
                SseStream::from_bytes_stream(bytes).boxed();
            return Ok(StreamableHttpPostResponse::Sse(events, session_out));
        }
        if is_json {
            let body = read_capped(response).await?;
            return match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
                Ok(message) => Ok(StreamableHttpPostResponse::Json(message, session_out)),
                // A notification or response does not wait for an answer, so an unusable body is not a failure.
                Err(_) if !answers => Ok(StreamableHttpPostResponse::Accepted),
                Err(_) => Err(StreamableHttpError::UnexpectedServerResponse(Cow::Borrowed(
                    "the server's JSON response isn't a JSON-RPC message",
                ))),
            };
        }
        Err(StreamableHttpError::UnexpectedContentType(content_type))
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), HttpError> {
        self.inner.delete_session(uri, session_id, auth_token, custom_headers).await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, sse_stream::Error>>, HttpError> {
        self.inner.get_stream(uri, session_id, last_event_id, auth_token, custom_headers).await
    }
}

/// Expands every text with the variables of the server. A reference to a reserved or unset variable fails the
/// server, and the values are never part of the message.
fn expand_all<'a>(texts: impl IntoIterator<Item = &'a String>, vars: &ExpandVars) -> Result<Vec<String>, String> {
    let mut expanded_texts = Vec::new();
    let mut missing = Vec::new();
    let mut reserved = Vec::new();
    for text in texts {
        let expanded = expand(text, vars);
        missing.extend(expanded.missing);
        reserved.extend(expanded.reserved);
        expanded_texts.push(expanded.text);
    }
    if !reserved.is_empty() {
        return Err(format!(
            "it uses reserved variables: {}; the service's own variables are never passed on",
            reserved.join(", ")
        ));
    }
    if !missing.is_empty() {
        return Err(format!("it uses unset variables: {}", missing.join(", ")));
    }
    Ok(expanded_texts)
}

fn header_map(headers: BTreeMap<String, String>) -> Result<HashMap<HeaderName, HeaderValue>, String> {
    headers
        .into_iter()
        .map(|(name, value)| {
            let header =
                HeaderName::from_bytes(name.as_bytes()).map_err(|_| format!("the header `{name}` isn't valid"))?;
            let value =
                HeaderValue::from_str(&value).map_err(|_| format!("the value of header `{name}` isn't valid"))?;
            Ok((header, value))
        })
        .collect()
}

/// Runs the `headers_helper` with the shell and reads a JSON object of header values from its output. The output
/// is a secret, so it never goes into a message.
async fn run_helper(command: &str, vars: &ExpandVars) -> Result<BTreeMap<String, String>, String> {
    let mut process = helper_command(command);
    process.stdin(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    for (name, _) in std::env::vars() {
        if vars.is_reserved(&name) {
            process.env_remove(&name);
        }
    }
    let output = tokio::time::timeout(HELPER_TIMEOUT, process.output())
        .await
        .map_err(|_| "its headers_helper didn't finish within 10 seconds".to_string())?
        .map_err(|_| "its headers_helper couldn't be started".to_string())?;
    if !output.status.success() {
        return Err("its headers_helper exited with an error".into());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| "the output of its headers_helper isn't a JSON object of strings".into())
}

fn helper_command(command: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // The command goes to cmd as it is, since its quotes are part of the JSON it prints.
        let mut process = std::process::Command::new("cmd");
        process.raw_arg(format!("/C {command}"));
        tokio::process::Command::from(process)
    }
    #[cfg(not(windows))]
    {
        let mut process = tokio::process::Command::new("sh");
        process.arg("-c").arg(command);
        process
    }
}
