//! Compaction: a long conversation is replaced by a summary of it, so the work stays inside the model's window.

use crate::mcp::ToolDef;
use crate::model::{Message, ModelDriver, ModelError, ModelRequest};

/// The share of the window that a conversation may use before it is compacted.
const COMPACT_SHARE: f64 = 0.80;
const CHARS_PER_TOKEN: usize = 4;
/// What one invoked skill keeps after a compaction, and what all of them keep together.
const SKILL_TOKEN_LIMIT: usize = 5_000;
const TOTAL_TOKEN_LIMIT: usize = 25_000;
const INSTRUCTIONS: &str = "You summarize a conversation so that an agent can carry on from the summary alone. \
Write what the user asked for and what they still want, the decisions made, the files read and changed with \
their state, the open problems, and the rules the user set. End with the next step. Reply with the summary only.";

/// How far the context must grow after a compaction before it is compacted again, as a share of the window.
const REGROWTH_SHARE: f64 = 0.10;

/// The tokens at which a conversation is compacted.
pub(crate) fn compact_threshold(window: u64) -> u64 {
    (window as f64 * COMPACT_SHARE) as u64
}

/// The growth of the context after a compaction that triggers the next one.
pub(crate) fn regrowth_tokens(window: u64) -> u64 {
    (window as f64 * REGROWTH_SHARE) as u64
}

/// The size of the messages and tool definitions in tokens, estimated from their characters.
pub(crate) fn estimate_tokens(messages: &[Message], tools: &[ToolDef]) -> u64 {
    let chars: usize = messages.iter().map(message_chars).sum::<usize>() + tools.iter().map(tool_chars).sum::<usize>();
    (chars / CHARS_PER_TOKEN) as u64
}

fn message_chars(message: &Message) -> usize {
    match message {
        Message::System { text } | Message::User { text, .. } => text.chars().count(),
        Message::Assistant { text, tool_calls, opaque } => {
            let text = text.as_ref().map_or(0, |text| text.chars().count());
            let calls: usize =
                tool_calls.iter().map(|call| call.name.chars().count() + call.arguments.chars().count()).sum();
            let opaque: usize = opaque.iter().map(|block| block.to_string().chars().count()).sum();
            text + calls + opaque
        }
        Message::Tool { content, .. } => content.chars().count(),
    }
}

fn tool_chars(tool: &ToolDef) -> usize {
    tool.name.chars().count() + tool.description.chars().count() + tool.parameters.to_string().chars().count()
}

/// Asks the model for a summary of the conversation. The summary comes back with the attachments kept after it.
pub async fn compact(
    driver: &dyn ModelDriver,
    messages: &[Message],
    attachments: &[String],
) -> Result<String, ModelError> {
    let request = ModelRequest {
        messages: vec![
            Message::System { text: INSTRUCTIONS.into() },
            Message::User {
                text: format!("<conversation>\n{}\n</conversation>", transcript(messages)),
                images: vec![],
            },
        ],
        tools: vec![],
        effort: None,
    };
    let completion = driver.stream(&request, &mut |_: &str| {}).await?;
    let summary = completion.text.unwrap_or_default();
    if summary.trim().is_empty() {
        return Err(ModelError::Other("the summary is empty".into()));
    }
    let kept = reattached(attachments);
    Ok(if kept.is_empty() { summary } else { format!("{summary}\n\n{kept}") })
}

/// The conversation as plain text, so the summary request doesn't depend on how the provider takes tool calls.
fn transcript(messages: &[Message]) -> String {
    let mut lines = vec![];
    for message in messages {
        match message {
            Message::System { .. } => {}
            Message::User { text, .. } => lines.push(format!("User: {text}")),
            Message::Assistant { text, tool_calls, .. } => {
                if let Some(text) = text {
                    lines.push(format!("Assistant: {text}"));
                }
                for call in tool_calls {
                    lines.push(format!("Call {}: {}", call.name, call.arguments));
                }
            }
            Message::Tool { content, is_error, .. } => {
                lines.push(format!("{}: {content}", if *is_error { "Tool error" } else { "Tool result" }));
            }
        }
    }
    lines.join("\n")
}

/// The attachments kept after a summary. Each one keeps its first 5,000 tokens, and the ones after the first
/// 25,000 tokens in all are left out.
fn reattached(attachments: &[String]) -> String {
    let per_attachment = SKILL_TOKEN_LIMIT * CHARS_PER_TOKEN;
    let mut budget = TOTAL_TOKEN_LIMIT * CHARS_PER_TOKEN;
    let mut kept = vec![];
    for attachment in attachments {
        if budget == 0 {
            break;
        }
        let text: String = attachment.chars().take(per_attachment.min(budget)).collect();
        budget -= text.chars().count();
        kept.push(text);
    }
    kept.join("\n\n")
}

/// The message that stands for everything before a summary.
pub(crate) fn summary_message(summary: &str) -> Message {
    Message::User {
        text: format!("This is a summary of the earlier conversation, written when it was compacted:\n{summary}"),
        images: vec![],
    }
}

const CUT_RESULT: &str = "[The output was cut to save room in the context.]";

/// Cuts the oldest tool results, one at a time, until the messages fit `target_tokens`. Returns whether they do.
pub(crate) fn trim_oldest_results(messages: &mut [Message], target_tokens: u64) -> bool {
    let mut next = 0;
    while estimate_tokens(messages, &[]) > target_tokens {
        let Some(offset) = messages[next..]
            .iter()
            .position(|message| matches!(message, Message::Tool { content, .. } if content.as_str() != CUT_RESULT))
        else {
            return false;
        };
        next += offset;
        if let Message::Tool { content, images, .. } = &mut messages[next] {
            *content = CUT_RESULT.into();
            images.clear();
        }
        next += 1;
    }
    true
}

/// A tool output bigger than the window is cut to half of it, and the model is told how much was left out.
pub(crate) fn fit_result(text: String, window: u64) -> String {
    let length = text.chars().count();
    if (length / CHARS_PER_TOKEN) as u64 <= window {
        return text;
    }
    let kept = (window / 2) as usize * CHARS_PER_TOKEN;
    let head: String = text.chars().take(kept).collect();
    format!(
        "{head}\n[The output was cut to fit the model's context window. {} characters were left out.]",
        length - kept
    )
}
