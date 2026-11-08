mod anthropic;
mod openai;
mod retry;
mod sse;
mod types;

use async_trait::async_trait;
use vatra_core::AgentError;

pub use anthropic::{AnthropicDriver, DEFAULT_BASE_URL};
pub use openai::{ModelConfig, OpenAiDriver};
pub use retry::with_retries;
pub use types::{Completion, ContextWindow, ImagePart, Message, ModelError, ModelRequest, RawToolCall, TokenUsage};

use reqwest::StatusCode;

/// Whether a client error says the model doesn't take tools, so the request can be sent again without them.
/// The phrases are the ones the servers in use answer with.
pub(crate) fn refuses_tools(status: StatusCode, body: &str) -> bool {
    if !status.is_client_error() {
        return false;
    }
    let body = body.to_lowercase();
    let phrases = ["--jinja", "not support", "unsupported", "does not support", "unknown parameter"];
    body.contains("tools") && phrases.iter().any(|phrase| body.contains(phrase))
}

#[async_trait]
pub trait ModelDriver: Send + Sync {
    /// Answers the request. `on_text` gets each piece of text as it arrives.
    async fn stream(
        &self,
        request: &ModelRequest,
        on_text: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<Completion, ModelError>;

    fn context_window(&self) -> ContextWindow;

    fn supports_images(&self) -> bool;
}

impl From<ModelError> for AgentError {
    fn from(error: ModelError) -> AgentError {
        AgentError::Failed(error.to_string())
    }
}
