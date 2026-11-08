use std::sync::Arc;

use async_trait::async_trait;
use vatra_core::{AgentError, BriefFields, Summarizer};

use crate::model::{Message, ModelDriver, ModelRequest};

const INSTRUCTIONS: &str = "You summarize a conversation for another agent taking over the work. \
Reply only with a JSON object with these keys: \
\"summary\" (short text about what was done), \
\"decisions\" (a list of decisions made) and \
\"remaining\" (a list of what remains to do).";

pub struct OpenAiSummarizer {
    driver: Arc<dyn ModelDriver>,
}

impl OpenAiSummarizer {
    pub fn new(driver: Arc<dyn ModelDriver>) -> OpenAiSummarizer {
        OpenAiSummarizer { driver }
    }
}

#[async_trait]
impl Summarizer for OpenAiSummarizer {
    async fn summarize(&self, transcript: &str) -> Result<BriefFields, AgentError> {
        let request = ModelRequest {
            messages: vec![
                Message::System { text: INSTRUCTIONS.into() },
                Message::User { text: transcript.into(), images: vec![] },
            ],
            tools: vec![],
            effort: None,
        };
        let completion = self.driver.stream(&request, &mut |_: &str| {}).await?;
        let reply = completion.text.unwrap_or_default();
        parse_fields(&reply).map_err(AgentError::Failed)
    }
}

/// Local models often wrap the JSON in prose or a code fence, so only the outer object is parsed.
fn parse_fields(reply: &str) -> Result<BriefFields, String> {
    let start = reply.find('{').ok_or("the summary contains no JSON")?;
    let end = reply.rfind('}').ok_or("the summary contains no JSON")?;
    if end < start {
        return Err("the summary contains no JSON".into());
    }
    serde_json::from_str(&reply[start..=end]).map_err(|error| format!("invalid summary: {error}"))
}
