mod common;

use common::{Scripted, fixture};
use vatra_core::{Event, StoredEvent, run_turn};

#[tokio::test]
async fn events_are_persisted_in_order_and_turn_completes() {
    let (store, session) = fixture();
    let agent = Scripted::ok(vec![Event::AssistantText { text: "gata".into() }], Some("n1"));
    run_turn(&store, &agent, session, "salut", &mut |_| {}).await.unwrap();
    let kinds: Vec<_> = store.events(session).unwrap().into_iter().map(|e| e.event).collect();
    assert_eq!(kinds, vec![
        Event::UserMessage { text: "salut".into() },
        Event::AssistantText { text: "gata".into() },
        Event::TurnCompleted { agent: "scripted".into() },
    ]);
    assert_eq!(store.native_session(session).unwrap().as_deref(), Some("n1"));
}

#[tokio::test]
async fn failure_keeps_earlier_events_and_records_turn_failed() {
    let (store, session) = fixture();
    let agent = Scripted::fail(vec![Event::AssistantText { text: "parțial".into() }], "a căzut");
    assert!(run_turn(&store, &agent, session, "salut", &mut |_| {}).await.is_err());
    let events: Vec<_> = store.events(session).unwrap().into_iter().map(|e| e.event).collect();
    assert_eq!(events[1], Event::AssistantText { text: "parțial".into() });
    assert_eq!(events[2], Event::TurnFailed { agent: "scripted".into(), message: "a căzut".into() });
}

#[tokio::test]
async fn agent_receives_prior_history_without_the_new_prompt() {
    let (store, session) = fixture();
    store.append(session, &Event::AssistantText { text: "vechi".into() }).unwrap();
    let agent = Scripted::recording();
    run_turn(&store, &agent, session, "nou", &mut |_| {}).await.unwrap();
    let request = agent.last_request();
    assert_eq!(request.history, vec![Event::AssistantText { text: "vechi".into() }]);
    assert_eq!(request.prompt, "nou");
}

#[tokio::test]
async fn failed_turn_keeps_the_native_session_reported_during_the_turn() {
    let (store, session) = fixture();
    let agent = Scripted::fail(vec![Event::NativeSession { id: "n1".into() }], "a căzut");
    assert!(run_turn(&store, &agent, session, "x", &mut |_| {}).await.is_err());
    assert_eq!(store.native_session(session).unwrap().as_deref(), Some("n1"));
}

#[tokio::test]
async fn delta_is_forwarded_but_not_stored() {
    let (store, session) = fixture();
    let agent = Scripted::ok(
        vec![
            Event::AssistantDelta { text: "ga".into() },
            Event::AssistantDelta { text: "ta".into() },
            Event::AssistantText { text: "gata".into() },
        ],
        None,
    );
    let mut forwarded: Vec<StoredEvent> = vec![];
    run_turn(&store, &agent, session, "salut", &mut |stored: &StoredEvent| forwarded.push(stored.clone()))
        .await
        .unwrap();
    let kinds: Vec<_> = store.events(session).unwrap().into_iter().map(|e| e.event).collect();
    assert_eq!(kinds, vec![
        Event::UserMessage { text: "salut".into() },
        Event::AssistantText { text: "gata".into() },
        Event::TurnCompleted { agent: "scripted".into() },
    ]);
    let deltas: Vec<_> =
        forwarded.iter().filter(|stored| matches!(stored.event, Event::AssistantDelta { .. })).collect();
    assert_eq!(deltas.len(), 2);
    assert!(deltas.iter().all(|stored| stored.seq == 0));
}
