use std::time::Duration;

use serde_json::Value;
use thiserror::Error;

use crate::mcp::ToolDef;

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    System { text: String },
    User { text: String, images: Vec<ImagePart> },
    /// `opaque` holds provider blocks that must be replayed as they arrived, such as Anthropic's
    /// thinking blocks. Drivers that don't use them ignore them.
    Assistant { text: Option<String>, tool_calls: Vec<RawToolCall>, opaque: Vec<Value> },
    Tool { id: String, content: String, is_error: bool, images: Vec<ImagePart> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImagePart {
    pub media_type: String,
    pub base64: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    pub effort: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RawToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Completion {
    pub text: Option<String>,
    pub tool_calls: Vec<RawToolCall>,
    pub usage: TokenUsage,
    pub opaque: Vec<Value>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cached_input: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextWindow {
    pub tokens: u64,
    /// True when the window is a guess rather than something the server reported or the config set.
    pub estimated: bool,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum ModelError {
    #[error("the model's context is too large: {0}")]
    ContextTooLarge(String),
    #[error("the model is rate limited")]
    RateLimited { retry_after: Option<Duration> },
    /// The server doesn't take tools. The request can be sent again without them.
    #[error("{0}")]
    ToolsRejected(String),
    #[error("{0}")]
    Server(String),
    #[error("{0}")]
    Other(String),
}
