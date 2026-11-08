use async_trait::async_trait;
use serde::Deserialize;

use crate::agent::{Agent, AgentError, RunError, run_turn};
use crate::event::{Event, HandoffBrief, StoredEvent};
use crate::store::{Store, StoreError};

const TRANSCRIPT_MAX_CHARS: usize = 40_000;
const DEGRADED_SUMMARY_TEXTS: usize = 3;
const DEGRADED_SUMMARY_MAX_CHARS: usize = 2_000;
const PATH_KEYS: [&str; 3] = ["path", "file_path", "notebook_path"];
const LINE_MAX_CHARS: usize = 8_000;
const TRUNCATION_MARKER_CHARS: usize = 48;
const OMITTED_MARKER_CHARS: usize = 64;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct BriefFields {
    pub summary: String,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub remaining: Vec<String>,
}

#[async_trait]
pub trait Summarizer: Send + Sync {
    async fn summarize(&self, transcript: &str) -> Result<BriefFields, AgentError>;
}

fn transcript_line(event: &Event) -> Option<String> {
    match event {
        Event::UserMessage { text } => Some(format!("User: {text}")),
        Event::AssistantText { text } => Some(format!("Assistant: {text}")),
        Event::ToolCall { name, arguments, .. } => Some(format!("Call {name}: {arguments}")),
        Event::ToolResult { output, is_error, .. } => {
            Some(format!("{}: {output}", if *is_error { "Tool error" } else { "Tool result" }))
        }
        Event::ToolDenied { name, .. } => Some(format!("Call refused: {name}")),
        Event::HandoffResult { text, .. } => Some(format!("Handoff result: {text}")),
        Event::BackgroundResult { text, .. } => Some(format!("Background result: {text}")),
        Event::TurnFailed { message, .. } => Some(format!("Failed turn: {message}")),
        Event::Nested { event, .. } => match event.as_ref() {
            Event::AssistantText { text } => Some(format!("Subagent: {text}")),
            _ => None,
        },
        _ => None,
    }
}

fn shorten_line(line: String, max_chars: usize) -> String {
    let length = line.chars().count();
    if length <= max_chars {
        return line;
    }
    let side = max_chars.saturating_sub(TRUNCATION_MARKER_CHARS) / 2;
    let head: String = line.chars().take(side).collect();
    let tail: String = line.chars().skip(length - side).collect();
    format!("{head}\n[... {} characters truncated ...]\n{tail}", length - 2 * side)
}

fn render_lines(lines: Vec<String>, max_chars: usize) -> String {
    let budget = max_chars.saturating_sub(OMITTED_MARKER_CHARS);
    let line_limit = LINE_MAX_CHARS.min(budget);
    let total = lines.len();
    let mut kept = Vec::new();
    let mut remaining = budget;
    for line in lines.into_iter().rev() {
        let line = shorten_line(line, line_limit);
        let cost = line.chars().count() + 1;
        if cost > remaining {
            break;
        }
        remaining -= cost;
        kept.push(line);
    }
    let omitted = total - kept.len();
    if omitted > 0 {
        kept.push(format!("[... {omitted} older events omitted ...]"));
    }
    kept.reverse();
    kept.join("\n")
}

/// Keeps the newest events that fit in `max_chars`, shortens oversized ones and says what it omitted.
pub fn transcript(events: &[StoredEvent], max_chars: usize) -> String {
    let lines = events.iter().filter_map(|stored| transcript_line(&stored.event)).collect();
    render_lines(lines, max_chars)
}

/// The same rendering, for an agent that receives history instead of stored events.
pub fn history_transcript(events: &[Event], max_chars: usize) -> String {
    render_lines(events.iter().filter_map(transcript_line).collect(), max_chars)
}

pub fn relevant_files(events: &[StoredEvent]) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for stored in events {
        if let Event::ToolCall { arguments, .. } = &stored.event {
            for key in PATH_KEYS {
                if let Some(path) = arguments[key].as_str()
                    && !files.iter().any(|known| known == path)
                {
                    files.push(path.to_string());
                }
            }
        }
    }
    files
}

fn degraded_summary(events: &[StoredEvent]) -> String {
    let mut texts: Vec<&str> = events
        .iter()
        .rev()
        .filter_map(|stored| match &stored.event {
            Event::UserMessage { text } | Event::AssistantText { text } => Some(text.as_str()),
            _ => None,
        })
        .take(DEGRADED_SUMMARY_TEXTS)
        .collect();
    texts.reverse();
    let joined = texts.join("\n");
    let skip = joined.chars().count().saturating_sub(DEGRADED_SUMMARY_MAX_CHARS);
    joined.chars().skip(skip).collect()
}

fn degraded_brief(events: &[StoredEvent], files: Vec<String>, reason: &str) -> HandoffBrief {
    HandoffBrief {
        summary: format!("(The automatic summary failed: {reason})\n{}", degraded_summary(events)),
        files,
        decisions: vec![],
        remaining: vec![],
        degraded: true,
    }
}

pub async fn build_brief(events: &[StoredEvent], summarizer: &dyn Summarizer) -> HandoffBrief {
    let files = relevant_files(events);
    match summarizer.summarize(&transcript(events, TRANSCRIPT_MAX_CHARS)).await {
        Ok(fields) if fields.summary.trim().is_empty() => {
            degraded_brief(events, files, "the model returned an empty summary")
        }
        Ok(fields) => HandoffBrief {
            summary: fields.summary,
            files,
            decisions: fields.decisions,
            remaining: fields.remaining,
            degraded: false,
        },
        Err(error) => degraded_brief(events, files, &error.to_string()),
    }
}

fn section(prompt: &mut String, title: &str, items: &[String]) {
    if !items.is_empty() {
        prompt.push_str(&format!("\n{title}:\n"));
        for item in items {
            prompt.push_str(&format!("- {item}\n"));
        }
    }
}

pub fn render_handoff_prompt(brief: &HandoffBrief, task: &str) -> String {
    let mut prompt = String::from("You're taking over a task from another agent.\n");
    if brief.degraded {
        prompt.push_str("Note: this summary is incomplete because it couldn't be generated. Check your assumptions.\n");
    }
    prompt.push_str(&format!("\nSummary:\n{}\n", brief.summary));
    section(&mut prompt, "Relevant files", &brief.files);
    section(&mut prompt, "Decisions made", &brief.decisions);
    section(&mut prompt, "What remains", &brief.remaining);
    prompt.push_str(&format!("\nYour task:\n{task}\n"));
    prompt
}

/// Creates the session a handoff runs in, in the same project as its source.
pub fn create_handoff_session(store: &Store, from_session: i64, target_name: &str) -> Result<i64, StoreError> {
    let project = store.session_project(from_session)?;
    store.create_session(project, target_name)
}

/// Runs `task` on `target` in a new session of the same project and reports the outcome back.
pub async fn handoff(
    store: &Store,
    from_session: i64,
    target: &dyn Agent,
    summarizer: &dyn Summarizer,
    task: &str,
    on_event: &mut (dyn FnMut(&StoredEvent) + Send),
) -> Result<i64, RunError> {
    let session = create_handoff_session(store, from_session, target.name())?;
    run_handoff(store, from_session, session, target, summarizer, task, on_event).await?;
    Ok(session)
}

pub async fn run_handoff(
    store: &Store,
    from_session: i64,
    session: i64,
    target: &dyn Agent,
    summarizer: &dyn Summarizer,
    task: &str,
    on_event: &mut (dyn FnMut(&StoredEvent) + Send),
) -> Result<(), RunError> {
    let events = store.events(from_session)?;
    let brief = build_brief(&events, summarizer).await;
    let prompt = render_handoff_prompt(&brief, task);

    let event = Event::HandoffStarted { from_session, to_agent: target.name().into(), brief };
    let seq = store.append(session, &event)?;
    on_event(&StoredEvent { seq, session_id: session, event });

    let outcome = run_turn(store, target, session, &prompt, on_event).await;
    let text = match &outcome {
        Ok(()) => last_assistant_text(&store.events(session)?),
        Err(error) => format!("The handoff failed: {error}"),
    };
    let event = Event::HandoffResult { to_session: session, text };
    let seq = store.append(from_session, &event)?;
    on_event(&StoredEvent { seq, session_id: from_session, event });
    outcome
}

fn last_assistant_text(events: &[StoredEvent]) -> String {
    events
        .iter()
        .rev()
        .find_map(|stored| match &stored.event {
            Event::AssistantText { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}
