mod common;

use async_trait::async_trait;
use common::Scripted;
use serde_json::json;
use vatra_core::{
    AgentError, BriefFields, Event, Store, StoredEvent, Summarizer, build_brief, handoff, render_handoff_prompt,
    transcript,
};

struct GoodSummary;

#[async_trait]
impl Summarizer for GoodSummary {
    async fn summarize(&self, _transcript: &str) -> Result<BriefFields, AgentError> {
        Ok(BriefFields {
            summary: "rezumat bun".into(),
            decisions: vec!["folosim Rust".into()],
            remaining: vec!["testele".into()],
        })
    }
}

struct FailingSummary;

#[async_trait]
impl Summarizer for FailingSummary {
    async fn summarize(&self, _transcript: &str) -> Result<BriefFields, AgentError> {
        Err(AgentError::Failed("indisponibil".into()))
    }
}

fn session_with_history() -> (Store, i64) {
    let store = Store::open_in_memory().unwrap();
    let project = store.create_project("p").unwrap();
    let session = store.create_session(project, "local").unwrap();
    for event in [
        Event::UserMessage { text: "repară bugul".into() },
        Event::ToolCall { id: "c1".into(), name: "citește".into(), arguments: json!({"path": "src/main.rs"}) },
        Event::ToolCall { id: "c2".into(), name: "citește".into(), arguments: json!({"file_path": "src/main.rs"}) },
        Event::AssistantText { text: "am găsit cauza".into() },
    ] {
        store.append(session, &event).unwrap();
    }
    (store, session)
}

#[tokio::test]
async fn handoff_creates_new_session_and_returns_result_to_source() {
    let (store, source) = session_with_history();
    let target = Scripted::ok(vec![Event::AssistantText { text: "am rezolvat".into() }], None);
    let new = handoff(&store, source, &target, &GoodSummary, "repară", &mut |_| {}).await.unwrap();
    assert!(matches!(store.events(new).unwrap()[0].event,
        Event::HandoffStarted { from_session, .. } if from_session == source));
    assert!(target.last_request().prompt.contains("repară"));
    assert!(target.last_request().prompt.contains("src/main.rs"));
    assert!(target.last_request().prompt.contains("folosim Rust"));
    assert_eq!(store.events(source).unwrap().last().unwrap().event,
        Event::HandoffResult { to_session: new, text: "am rezolvat".into() });
    assert_eq!(store.session_project(new).unwrap(), store.session_project(source).unwrap());
    assert_eq!(store.session_agent(new).unwrap(), "scripted");
}

#[tokio::test]
async fn failed_summary_falls_back_to_degraded_brief() {
    let (store, source) = session_with_history();
    let brief = build_brief(&store.events(source).unwrap(), &FailingSummary).await;
    assert!(brief.degraded);
    assert!(brief.summary.contains("am găsit cauza"));
    assert_eq!(brief.files, vec!["src/main.rs".to_string()]);
    assert!(render_handoff_prompt(&brief, "x").to_lowercase().contains("incomplete"));
}

#[tokio::test]
async fn failed_target_records_failure_in_source_session() {
    let (store, source) = session_with_history();
    let target = Scripted::fail(vec![], "indisponibil");
    assert!(handoff(&store, source, &target, &GoodSummary, "x", &mut |_| {}).await.is_err());
    assert!(matches!(&store.events(source).unwrap().last().unwrap().event,
        Event::HandoffResult { text, .. } if text.contains("indisponibil")));
}

#[test]
fn transcript_keeps_newest_and_respects_char_boundaries() {
    let mut events: Vec<StoredEvent> = (0..5_000)
        .map(|seq| StoredEvent {
            seq,
            session_id: 1,
            event: Event::AssistantText { text: format!("ășțîâ mesaj {seq}") },
        })
        .collect();
    events.push(StoredEvent {
        seq: 5_000,
        session_id: 1,
        event: Event::AssistantText { text: "ultimul mesaj".into() },
    });
    let text = transcript(&events, 40_000);
    assert!(text.chars().count() <= 40_000);
    assert!(text.contains("ultimul mesaj"));
    assert!(!text.contains("mesaj 0\n"));
}

fn stored(events: Vec<Event>) -> Vec<StoredEvent> {
    events
        .into_iter()
        .enumerate()
        .map(|(seq, event)| StoredEvent { seq: seq as i64, session_id: 1, event })
        .collect()
}

#[test]
fn oversized_event_is_truncated_and_older_events_survive() {
    let events = stored(vec![
        Event::UserMessage { text: "cererea inițială".into() },
        Event::ToolResult { id: "c".into(), output: "ă".repeat(50_000), is_error: false },
        Event::AssistantText { text: "gata".into() },
    ]);
    let text = transcript(&events, 40_000);
    assert!(text.chars().count() <= 40_000);
    assert!(text.contains("cererea inițială"));
    assert!(text.contains("gata"));
    assert!(text.contains("truncated"));
}

#[test]
fn single_oversized_last_event_still_produces_a_transcript() {
    let events = stored(vec![Event::AssistantText { text: "x".repeat(50_000) }]);
    assert!(!transcript(&events, 40_000).is_empty());
}

#[test]
fn background_results_are_in_the_transcript() {
    let events = stored(vec![Event::BackgroundResult {
        id: "task-1".into(),
        text: "Background command task-1 finished.
build-ok".into(),
    }]);
    assert!(transcript(&events, 40_000).contains("build-ok"));
}

#[test]
fn omitted_events_are_announced_in_the_transcript() {
    let events = stored((0..5_000).map(|n| Event::AssistantText { text: format!("mesaj {n}") }).collect());
    assert!(transcript(&events, 1_000).contains("omitted"));
}

struct EmptySummary;

#[async_trait]
impl Summarizer for EmptySummary {
    async fn summarize(&self, _transcript: &str) -> Result<BriefFields, AgentError> {
        Ok(BriefFields { summary: "  ".into(), decisions: vec![], remaining: vec![] })
    }
}

#[tokio::test]
async fn empty_model_summary_counts_as_degraded() {
    let (store, source) = session_with_history();
    let brief = build_brief(&store.events(source).unwrap(), &EmptySummary).await;
    assert!(brief.degraded);
    assert!(brief.summary.contains("am găsit cauza"));
}

#[tokio::test]
async fn degraded_summary_keeps_the_newest_text_and_the_reason() {
    let events = stored(vec![
        Event::UserMessage { text: "v".repeat(5_000) },
        Event::AssistantText { text: "ultimul mesaj".into() },
    ]);
    let brief = build_brief(&events, &FailingSummary).await;
    assert!(brief.summary.contains("ultimul mesaj"));
    assert!(brief.summary.contains("indisponibil"));
}

#[test]
fn handoff_ignores_compacted_correctly() {
    // The journal keeps every event, so the transcript is the full record. A summary is not one of its lines.
    let stored = |seq, event| StoredEvent { seq, session_id: 1, event };
    let events = [
        stored(1, Event::UserMessage { text: "repară bugul".into() }),
        stored(2, Event::Compacted { summary: "rezumat vechi".into() }),
        stored(3, Event::AssistantText { text: "am găsit cauza".into() }),
    ];
    let text = transcript(&events, 10_000);
    assert_eq!(text, "User: repară bugul\nAssistant: am găsit cauza");
}
