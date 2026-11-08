use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use reqwest::header::{CONTENT_TYPE, RETRY_AFTER};
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use super::refuses_tools;
use super::sse::SseParser;
use super::{
    Completion, ContextWindow, ImagePart, Message, ModelDriver, ModelError, ModelRequest, RawToolCall, TokenUsage,
};
use crate::mcp::ToolDef;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;
const EXCERPT_CHARS: usize = 500;

#[derive(Clone, Debug)]
pub struct ModelConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub context_window: Option<u64>,
}

pub struct OpenAiDriver {
    http: reqwest::Client,
    config: ModelConfig,
    window: OnceCell<ContextWindow>,
    headers: Vec<(String, String)>,
}

impl OpenAiDriver {
    pub fn new(config: ModelConfig) -> OpenAiDriver {
        let http =
            reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build().expect("couldn't create the HTTP client");
        let window = OnceCell::new_with(config.context_window.map(|tokens| ContextWindow { tokens, estimated: false }));
        OpenAiDriver { http, config, window, headers: Vec::new() }
    }

    /// Headers sent with every request, for services that want to know the client.
    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> OpenAiDriver {
        self.headers = headers;
        self
    }

    fn base_url(&self) -> &str {
        self.config.base_url.trim_end_matches('/')
    }

    /// `context_window` can't wait for the server, so the window is read at the first request.
    async fn read_window(&self) -> ContextWindow {
        match self.probe_props().await {
            Some(tokens) => ContextWindow { tokens, estimated: false },
            None => ContextWindow { tokens: DEFAULT_CONTEXT_WINDOW, estimated: true },
        }
    }

    /// llama.cpp serves /props beside the /v1 of its base URL, not under it. The key isn't sent: this
    /// is a probe, and other servers don't have the route anyway.
    async fn probe_props(&self) -> Option<u64> {
        let root = self.base_url().strip_suffix("/v1").unwrap_or(self.base_url());
        let response = self.http.get(format!("{root}/props")).timeout(PROBE_TIMEOUT).send().await.ok()?;
        let props: Value = response.error_for_status().ok()?.json().await.ok()?;
        props["default_generation_settings"]["n_ctx"].as_u64().or(props["n_ctx"].as_u64())
    }

    fn body(&self, request: &ModelRequest) -> Value {
        let mut body = json!({
            "model": self.config.model,
            "messages": messages(&request.messages),
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        if !request.tools.is_empty() {
            body["tools"] = Value::Array(request.tools.iter().map(tool_json).collect());
        }
        if let Some(effort) = &request.effort {
            body["reasoning_effort"] = json!(effort);
        }
        body
    }
}

#[async_trait]
impl ModelDriver for OpenAiDriver {
    async fn stream(
        &self,
        request: &ModelRequest,
        on_text: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<Completion, ModelError> {
        self.window.get_or_init(|| self.read_window()).await;

        let url = format!("{}/chat/completions", self.base_url());
        let mut builder = self.http.post(url).json(&self.body(request));
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        if let Some(key) = &self.config.api_key {
            builder = builder.bearer_auth(key);
        }
        let mut response = builder.send().await.map_err(failed)?;
        let status = response.status();
        if !status.is_success() {
            let retry_after = retry_after(&response);
            let text = response.text().await.map_err(failed)?;
            return Err(status_error(status, retry_after, &text));
        }

        let is_json = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json"));
        let mut collector = Collector::default();
        if is_json {
            // A server that ignores `stream` answers with one whole response.
            let text = response.text().await.map_err(failed)?;
            let value: Value = serde_json::from_str(&text).map_err(|_| invalid())?;
            collector.whole(&value, on_text)?;
        } else {
            let mut parser = SseParser::default();
            while let Some(chunk) = response.chunk().await.map_err(failed)? {
                for data in parser.feed(&chunk) {
                    collector.data(&data, on_text)?;
                }
            }
            for data in parser.finish() {
                collector.data(&data, on_text)?;
            }
            if collector.chunks == 0 {
                return Err(invalid());
            }
        }
        Ok(collector.finish())
    }

    fn context_window(&self) -> ContextWindow {
        self.window.get().copied().unwrap_or(ContextWindow { tokens: DEFAULT_CONTEXT_WINDOW, estimated: true })
    }

    fn supports_images(&self) -> bool {
        true
    }
}

pub(super) fn failed(error: reqwest::Error) -> ModelError {
    ModelError::Other(format!("the request to the model failed: {}", error.without_url()))
}

pub(super) fn invalid() -> ModelError {
    ModelError::Other("invalid response from the model".into())
}

pub(super) fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    let seconds = response.headers().get(RETRY_AFTER)?.to_str().ok()?.trim().parse::<u64>().ok()?;
    Some(Duration::from_secs(seconds))
}

fn status_error(status: StatusCode, retry_after: Option<Duration>, body: &str) -> ModelError {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return ModelError::RateLimited { retry_after };
    }
    let excerpt: String = body.chars().take(EXCERPT_CHARS).collect();
    let message = format!("the model answered {status}: {excerpt}");
    if status.is_server_error() {
        return ModelError::Server(message);
    }
    let lowercase = body.to_lowercase();
    if status == StatusCode::BAD_REQUEST && lowercase.contains("exceed") && lowercase.contains("context") {
        return ModelError::ContextTooLarge(message);
    }
    if refuses_tools(status, body) {
        return ModelError::ToolsRejected(message);
    }
    ModelError::Other(message)
}

/// Gathers what the model says, from streamed chunks or from one whole response.
#[derive(Default)]
struct Collector {
    text: String,
    calls: BTreeMap<usize, PartialCall>,
    usage: TokenUsage,
    chunks: usize,
}

#[derive(Default)]
struct PartialCall {
    id: Option<String>,
    name: String,
    arguments: String,
}

impl Collector {
    fn data(&mut self, data: &str, on_text: &mut (dyn for<'a> FnMut(&'a str) + Send)) -> Result<(), ModelError> {
        if data == "[DONE]" {
            return Ok(());
        }
        let value: Value = serde_json::from_str(data).map_err(|_| invalid())?;
        self.chunk(&value, on_text)
    }

    fn chunk(&mut self, value: &Value, on_text: &mut (dyn for<'a> FnMut(&'a str) + Send)) -> Result<(), ModelError> {
        if let Some(error) = value.get("error").filter(|error| !error.is_null()) {
            let message = error["message"].as_str().map_or_else(|| error.to_string(), String::from);
            return Err(ModelError::Other(format!("the model's stream failed: {message}")));
        }
        self.read_usage(value);
        self.fragment(&value["choices"][0]["delta"], on_text);
        self.chunks += 1;
        Ok(())
    }

    fn whole(&mut self, value: &Value, on_text: &mut (dyn for<'a> FnMut(&'a str) + Send)) -> Result<(), ModelError> {
        let message = &value["choices"][0]["message"];
        if !message.is_object() {
            return Err(ModelError::Other("the model's response has no message".into()));
        }
        self.read_usage(value);
        self.fragment(message, on_text);
        Ok(())
    }

    fn read_usage(&mut self, value: &Value) {
        let usage = &value["usage"];
        if usage.is_object() {
            self.usage = TokenUsage {
                input: usage["prompt_tokens"].as_u64().unwrap_or_default(),
                output: usage["completion_tokens"].as_u64().unwrap_or_default(),
                cached_input: usage["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or_default(),
            };
        }
    }

    /// A delta of a stream, or the message of a whole response. Calls are keyed by `index`, so
    /// they come out in the order the model issued them.
    fn fragment(&mut self, fragment: &Value, on_text: &mut (dyn for<'a> FnMut(&'a str) + Send)) {
        if let Some(text) = fragment["content"].as_str().filter(|text| !text.is_empty()) {
            self.text.push_str(text);
            on_text(text);
        }
        let calls = fragment["tool_calls"].as_array().map(Vec::as_slice).unwrap_or_default();
        for (position, call) in calls.iter().enumerate() {
            let index = call["index"].as_u64().and_then(|index| usize::try_from(index).ok()).unwrap_or(position);
            let partial = self.calls.entry(index).or_default();
            if let Some(id) = call["id"].as_str() {
                partial.id = Some(id.to_string());
            }
            if partial.name.is_empty() {
                partial.name = call["function"]["name"].as_str().unwrap_or_default().to_string();
            }
            match &call["function"]["arguments"] {
                Value::String(arguments) => partial.arguments.push_str(arguments),
                Value::Null => {}
                // Some local servers send an object instead of a JSON string.
                other => partial.arguments.push_str(&other.to_string()),
            }
        }
    }

    fn finish(self) -> Completion {
        Completion {
            text: (!self.text.is_empty()).then_some(self.text),
            tool_calls: self
                .calls
                .into_iter()
                .map(|(index, call)| RawToolCall {
                    id: call.id.unwrap_or_else(|| format!("call_{index}")),
                    name: call.name,
                    arguments: call.arguments,
                })
                .collect(),
            usage: self.usage,
            opaque: vec![],
        }
    }
}

/// Chat Completions shape. A tool message can't carry images, and it must directly follow its
/// call, so a tool's images go in a user message after the whole batch of results.
fn messages(messages: &[Message]) -> Vec<Value> {
    let mut values = Vec::new();
    let mut tool_images: Vec<&ImagePart> = Vec::new();
    for message in messages {
        if !matches!(message, Message::Tool { .. }) && !tool_images.is_empty() {
            values.push(images_message(std::mem::take(&mut tool_images)));
        }
        let value = match message {
            Message::System { text } => json!({"role": "system", "content": text}),
            Message::User { text, images } => json!({"role": "user", "content": user_content(text, images)}),
            Message::Assistant { text, tool_calls, .. } => assistant_json(text.as_deref(), tool_calls),
            Message::Tool { id, content, images, .. } => {
                tool_images.extend(images);
                json!({"role": "tool", "tool_call_id": id, "content": content})
            }
        };
        values.push(value);
    }
    if !tool_images.is_empty() {
        values.push(images_message(tool_images));
    }
    values
}

fn user_content(text: &str, images: &[ImagePart]) -> Value {
    if images.is_empty() {
        return json!(text);
    }
    let mut parts = vec![json!({"type": "text", "text": text})];
    parts.extend(images.iter().map(image_part));
    Value::Array(parts)
}

fn assistant_json(text: Option<&str>, tool_calls: &[RawToolCall]) -> Value {
    let mut value = json!({"role": "assistant", "content": text});
    if !tool_calls.is_empty() {
        value["tool_calls"] = Value::Array(
            tool_calls
                .iter()
                .map(|call| {
                    json!({"id": call.id, "type": "function",
                        "function": {"name": call.name, "arguments": call.arguments}})
                })
                .collect(),
        );
    }
    value
}

fn images_message(images: Vec<&ImagePart>) -> Value {
    json!({"role": "user", "content": Value::Array(images.into_iter().map(image_part).collect())})
}

fn image_part(image: &ImagePart) -> Value {
    json!({"type": "image_url",
        "image_url": {"url": format!("data:{};base64,{}", image.media_type, image.base64)}})
}

fn tool_json(tool: &ToolDef) -> Value {
    json!({"type": "function", "function": {
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.parameters,
    }})
}
