use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde_json::{Value, json};

use super::openai::{failed, invalid, retry_after};
use super::refuses_tools;
use super::sse::SseParser;
use super::{
    Completion, ContextWindow, ImagePart, Message, ModelConfig, ModelDriver, ModelError, ModelRequest, RawToolCall,
    TokenUsage,
};
use crate::mcp::ToolDef;

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_TOKENS: u64 = 16_000;
const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;
const EXCERPT_CHARS: usize = 500;
const NO_KEY: &str = "this agent has no Anthropic API key. Add one in Settings, Agents, or use the Claude Code agent to \
     chat with a Claude subscription.";
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

pub struct AnthropicDriver {
    http: reqwest::Client,
    config: ModelConfig,
}

impl AnthropicDriver {
    pub fn new(config: ModelConfig) -> AnthropicDriver {
        let http =
            reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build().expect("couldn't create the HTTP client");
        AnthropicDriver { http, config }
    }

    fn base_url(&self) -> &str {
        self.config.base_url.trim_end_matches('/')
    }

    fn body(&self, request: &ModelRequest) -> Value {
        let mut body = json!({
            "model": self.config.model,
            "max_tokens": MAX_TOKENS,
            "stream": true,
            "messages": conversation(&request.messages),
        });
        if let Some(system) = system(&request.messages) {
            body["system"] = system;
        }
        if !request.tools.is_empty() {
            body["tools"] = tools(&request.tools);
        }
        // Models think by default and reject a budget, so only the effort level is sent.
        if let Some(effort) = request.effort.as_deref().filter(|effort| EFFORTS.contains(effort)) {
            body["output_config"] = json!({"effort": effort});
        }
        body
    }
}

#[async_trait]
impl ModelDriver for AnthropicDriver {
    async fn stream(
        &self,
        request: &ModelRequest,
        on_text: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<Completion, ModelError> {
        // Without a key Anthropic only answers "x-api-key header is required", which says nothing about the fix.
        // A proxy at another address may add its own credentials, so only Anthropic's own address is refused here.
        if self.config.api_key.is_none() && self.base_url() == DEFAULT_BASE_URL {
            return Err(ModelError::Other(NO_KEY.into()));
        }
        let url = format!("{}/v1/messages", self.base_url());
        let mut builder = self.http.post(url).header("anthropic-version", API_VERSION).json(&self.body(request));
        if let Some(key) = &self.config.api_key {
            builder = builder.header("x-api-key", key);
        }
        let mut response = builder.send().await.map_err(failed)?;
        let status = response.status();
        if !status.is_success() {
            let retry_after = retry_after(&response);
            let text = response.text().await.map_err(failed)?;
            return Err(status_error(status, retry_after, &text));
        }

        let mut parser = SseParser::default();
        let mut collector = Collector::default();
        while let Some(chunk) = response.chunk().await.map_err(failed)? {
            for data in parser.feed(&chunk) {
                collector.data(&data, on_text)?;
            }
        }
        for data in parser.finish() {
            collector.data(&data, on_text)?;
        }
        collector.finish()
    }

    fn context_window(&self) -> ContextWindow {
        match self.config.context_window {
            Some(tokens) => ContextWindow { tokens, estimated: false },
            None => ContextWindow { tokens: DEFAULT_CONTEXT_WINDOW, estimated: true },
        }
    }

    fn supports_images(&self) -> bool {
        true
    }
}

/// 529 is Anthropic's "overloaded", a server error, so `with_retries` repeats it.
fn status_error(status: StatusCode, retry_after: Option<Duration>, body: &str) -> ModelError {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return ModelError::RateLimited { retry_after };
    }
    let excerpt: String = body.chars().take(EXCERPT_CHARS).collect();
    let message = format!("the model answered {status}: {excerpt}");
    if status.is_server_error() {
        return ModelError::Server(message);
    }
    if status == StatusCode::BAD_REQUEST && body.contains("prompt is too long") {
        return ModelError::ContextTooLarge(message);
    }
    if refuses_tools(status, body) {
        return ModelError::ToolsRejected(message);
    }
    ModelError::Other(message)
}

fn stream_error(error: &Value) -> ModelError {
    let message = error["message"].as_str().map_or_else(|| error.to_string(), String::from);
    let message = format!("the model's stream failed: {message}");
    match error["type"].as_str() {
        Some("overloaded_error" | "api_error") => ModelError::Server(message),
        _ => ModelError::Other(message),
    }
}

/// Gathers the blocks of a streamed answer by their index, so they come out in the order they were written.
#[derive(Default)]
struct Collector {
    blocks: BTreeMap<u64, Block>,
    input: u64,
    cache_creation: u64,
    cache_read: u64,
    output: u64,
    stopped: bool,
}

enum Block {
    Text(String),
    Thinking { thinking: String, signature: String },
    RedactedThinking(String),
    ToolUse { id: String, name: String, arguments: String },
}

impl Collector {
    fn data(&mut self, data: &str, on_text: &mut (dyn for<'a> FnMut(&'a str) + Send)) -> Result<(), ModelError> {
        let value: Value = serde_json::from_str(data).map_err(|_| invalid())?;
        self.event(&value, on_text)
    }

    fn event(&mut self, value: &Value, on_text: &mut (dyn for<'a> FnMut(&'a str) + Send)) -> Result<(), ModelError> {
        match value["type"].as_str() {
            Some("message_start") => self.read_usage(&value["message"]["usage"]),
            Some("content_block_start") => self.start(value)?,
            Some("content_block_delta") => self.delta(value, on_text)?,
            Some("message_delta") => self.read_usage(&value["usage"]),
            Some("message_stop") => self.stopped = true,
            Some("error") => return Err(stream_error(&value["error"])),
            // Block stops and pings carry nothing the answer needs, and unknown events are skipped.
            _ => {}
        }
        Ok(())
    }

    /// Each event carries the counts so far, so a count that is present replaces the last one.
    fn read_usage(&mut self, usage: &Value) {
        if let Some(input) = usage["input_tokens"].as_u64() {
            self.input = input;
        }
        if let Some(created) = usage["cache_creation_input_tokens"].as_u64() {
            self.cache_creation = created;
        }
        if let Some(read) = usage["cache_read_input_tokens"].as_u64() {
            self.cache_read = read;
        }
        if let Some(output) = usage["output_tokens"].as_u64() {
            self.output = output;
        }
    }

    fn start(&mut self, value: &Value) -> Result<(), ModelError> {
        let index = index(value)?;
        let block = &value["content_block"];
        let started = match block["type"].as_str() {
            Some("text") => Block::Text(String::new()),
            Some("thinking") => Block::Thinking { thinking: String::new(), signature: String::new() },
            Some("redacted_thinking") => {
                Block::RedactedThinking(block["data"].as_str().unwrap_or_default().to_string())
            }
            Some("tool_use") => Block::ToolUse {
                id: block["id"].as_str().unwrap_or_default().to_string(),
                name: block["name"].as_str().unwrap_or_default().to_string(),
                arguments: String::new(),
            },
            // Other kinds of block aren't used yet, so they are skipped.
            _ => return Ok(()),
        };
        self.blocks.insert(index, started);
        Ok(())
    }

    fn delta(&mut self, value: &Value, on_text: &mut (dyn for<'a> FnMut(&'a str) + Send)) -> Result<(), ModelError> {
        let index = index(value)?;
        let delta = &value["delta"];
        let Some(block) = self.blocks.get_mut(&index) else {
            return Ok(());
        };
        match (block, delta["type"].as_str()) {
            (Block::Text(text), Some("text_delta")) => {
                let fragment = delta["text"].as_str().unwrap_or_default();
                text.push_str(fragment);
                if !fragment.is_empty() {
                    on_text(fragment);
                }
            }
            (Block::Thinking { thinking, .. }, Some("thinking_delta")) => {
                thinking.push_str(delta["thinking"].as_str().unwrap_or_default());
            }
            (Block::Thinking { signature, .. }, Some("signature_delta")) => {
                signature.push_str(delta["signature"].as_str().unwrap_or_default());
            }
            (Block::ToolUse { arguments, .. }, Some("input_json_delta")) => {
                arguments.push_str(delta["partial_json"].as_str().unwrap_or_default());
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(self) -> Result<Completion, ModelError> {
        if !self.stopped {
            return Err(ModelError::Other("the model's stream ended before the answer was complete".into()));
        }
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        let mut opaque = Vec::new();
        for block in self.blocks.into_values() {
            match block {
                Block::Text(part) => text.push_str(&part),
                Block::Thinking { thinking, signature } => {
                    opaque.push(json!({"type": "thinking", "thinking": thinking, "signature": signature}))
                }
                Block::RedactedThinking(data) => opaque.push(json!({"type": "redacted_thinking", "data": data})),
                Block::ToolUse { id, name, arguments } => tool_calls.push(RawToolCall { id, name, arguments }),
            }
        }
        Ok(Completion {
            text: (!text.is_empty()).then_some(text),
            tool_calls,
            // Input counts the cached tokens too, as the OpenAI driver's does.
            usage: TokenUsage {
                input: self.input + self.cache_creation + self.cache_read,
                output: self.output,
                cached_input: self.cache_read,
            },
            opaque,
        })
    }
}

fn index(value: &Value) -> Result<u64, ModelError> {
    value["index"].as_u64().ok_or_else(invalid)
}

/// Messages API shape. Consecutive messages of one role share a turn, so the results of parallel
/// calls form one user message, as the API requires.
fn conversation(messages: &[Message]) -> Vec<Value> {
    let mut turns: Vec<(&str, Vec<Value>)> = Vec::new();
    for message in messages {
        let (role, blocks) = match message {
            Message::System { .. } => continue,
            Message::User { text, images } => ("user", user_blocks(text, images)),
            Message::Assistant { text, tool_calls, opaque } => {
                ("assistant", assistant_blocks(text.as_deref(), tool_calls, opaque))
            }
            Message::Tool { id, content, is_error, images } => {
                ("user", vec![tool_result(id, content, *is_error, images)])
            }
        };
        match turns.last_mut() {
            Some((last, content)) if *last == role => content.extend(blocks),
            _ => turns.push((role, blocks)),
        }
    }
    turns.into_iter().map(|(role, content)| json!({"role": role, "content": content})).collect()
}

fn user_blocks(text: &str, images: &[ImagePart]) -> Vec<Value> {
    let mut blocks = Vec::new();
    if !text.is_empty() {
        blocks.push(json!({"type": "text", "text": text}));
    }
    blocks.extend(images.iter().map(image_block));
    blocks
}

/// Replayed thinking blocks come first, as the API requires, then the text and the calls.
fn assistant_blocks(text: Option<&str>, tool_calls: &[RawToolCall], opaque: &[Value]) -> Vec<Value> {
    let mut blocks = opaque.to_vec();
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        blocks.push(json!({"type": "text", "text": text}));
    }
    blocks.extend(
        tool_calls
            .iter()
            .map(|call| json!({"type": "tool_use", "id": call.id, "name": call.name, "input": input(&call.arguments)})),
    );
    blocks
}

/// The arguments as the object the API expects. Arguments that aren't an object become `{}`.
fn input(arguments: &str) -> Value {
    match serde_json::from_str(arguments) {
        Ok(object @ Value::Object(_)) => object,
        _ => json!({}),
    }
}

fn tool_result(id: &str, content: &str, is_error: bool, images: &[ImagePart]) -> Value {
    let content = if images.is_empty() {
        json!(content)
    } else {
        let mut blocks = vec![json!({"type": "text", "text": content})];
        blocks.extend(images.iter().map(image_block));
        Value::Array(blocks)
    };
    json!({"type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error})
}

fn image_block(image: &ImagePart) -> Value {
    json!({"type": "image", "source": {"type": "base64", "media_type": image.media_type, "data": image.base64}})
}

/// The system prompt is top-level. Its last block carries a cache breakpoint.
fn system(messages: &[Message]) -> Option<Value> {
    let mut blocks: Vec<Value> = messages
        .iter()
        .filter_map(|message| match message {
            Message::System { text } => Some(json!({"type": "text", "text": text})),
            _ => None,
        })
        .collect();
    let last = blocks.last_mut()?;
    last["cache_control"] = json!({"type": "ephemeral"});
    Some(Value::Array(blocks))
}

/// The tool list is top-level too. Its last tool carries the second cache breakpoint.
fn tools(tools: &[ToolDef]) -> Value {
    let mut values: Vec<Value> = tools
        .iter()
        .map(|tool| json!({"name": tool.name, "description": tool.description, "input_schema": tool.parameters}))
        .collect();
    if let Some(last) = values.last_mut() {
        last["cache_control"] = json!({"type": "ephemeral"});
    }
    Value::Array(values)
}
