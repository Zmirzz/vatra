mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use serde_json::json;
use vatra_core::{Event, Store};
use vatra_service::{Notification, PermissionMode, Service, ServiceError};

#[tokio::test]
async fn events_are_saved_then_broadcast_in_order() {
    let (service, session) = with_agent(Scripted::text("gata"));
    let mut updates = service.subscribe();
    service.start_turn(session, "salut".into(), PermissionMode::FullAccess).unwrap();
    let events = collect_until_stopped(&mut updates).await;
    assert!(events.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    assert_eq!(service.store().events(session).unwrap().len(), events.len());
    assert!(matches!(events.last().unwrap().event, Event::TurnCompleted { .. }));
}

#[tokio::test]
async fn second_turn_in_the_same_session_is_busy() {
    let (service, session) = with_agent(Scripted::blocking());
    service.start_turn(session, "a".into(), PermissionMode::FullAccess).unwrap();
    assert!(matches!(
        service.start_turn(session, "b".into(), PermissionMode::FullAccess),
        Err(ServiceError::Busy)
    ));
    service.stop_turn(session);
}

#[tokio::test]
async fn unknown_session_is_rejected() {
    let (service, _) = with_agent(Scripted::text("x"));
    assert!(matches!(
        service.start_turn(999, "a".into(), PermissionMode::FullAccess),
        Err(ServiceError::UnknownSession)
    ));
}

#[tokio::test]
async fn stop_ends_the_turn_records_failure_and_denies_pending_approvals() {
    let (service, session) = with_agent(Scripted::asks_approval("unealta"));
    service.start_turn(session, "a".into(), PermissionMode::Supervised).unwrap();
    wait_for_approval(&service).await;
    service.stop_turn(session);
    wait_until_stopped(&service, session).await;
    assert!(service.pending_approvals().is_empty());
    assert!(matches!(last_event(&service, session), Event::TurnFailed { message, .. } if message.contains("stopped")));
}

#[tokio::test]
async fn approval_survives_having_no_subscriber_and_can_be_listed_later() {
    let (service, session) = with_agent(Scripted::asks_approval("unealta"));
    service.start_turn(session, "a".into(), PermissionMode::Supervised).unwrap();
    wait_for_approval(&service).await;
    let pending = service.pending_approvals();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].tool, "unealta");
    service.resolve_approval(&pending[0].id, true).unwrap();
    wait_until_stopped(&service, session).await;
    assert!(service.pending_approvals().is_empty());
    let events = service.store().events(session).unwrap();
    assert!(events.iter().any(|e| e.event == Event::AssistantText { text: "aprobat: true".into() }));
}

#[tokio::test]
async fn refused_approval_reaches_the_agent() {
    let (service, session) = with_agent(Scripted::asks_approval("unealta"));
    service.start_turn(session, "a".into(), PermissionMode::Supervised).unwrap();
    wait_for_approval(&service).await;
    let id = service.pending_approvals()[0].id.clone();
    service.resolve_approval(&id, false).unwrap();
    wait_until_stopped(&service, session).await;
    let events = service.store().events(session).unwrap();
    assert!(events.iter().any(|e| e.event == Event::AssistantText { text: "aprobat: false".into() }));
}

#[tokio::test]
async fn approval_and_resolution_are_broadcast() {
    let (service, session) = with_agent(Scripted::asks_approval("unealta"));
    let mut updates = service.subscribe();
    service.start_turn(session, "a".into(), PermissionMode::Supervised).unwrap();
    wait_for_approval(&service).await;
    let id = service.pending_approvals()[0].id.clone();
    service.resolve_approval(&id, true).unwrap();
    wait_until_stopped(&service, session).await;
    let mut seen = vec![];
    while let Ok(update) = updates.try_recv() {
        seen.push(update);
    }
    assert!(seen.iter().any(|n| matches!(n, Notification::Approval(request) if request.id == id)));
    assert!(seen.iter().any(|n| matches!(n, Notification::ApprovalResolved { id: resolved } if *resolved == id)));
}

#[tokio::test]
async fn the_approver_always_asks_because_the_policy_decides_what_needs_asking() {
    let (service, session) = with_agent(Scripted::asks_approval("Bash"));
    service.start_turn(session, "a".into(), PermissionMode::FullAccess).unwrap();
    wait_for_approval(&service).await;
    service.stop_turn(session);

    let (service, session) = with_agent(Scripted::asks_approval("Bash"));
    service.start_turn(session, "a".into(), PermissionMode::AcceptEdits).unwrap();
    wait_for_approval(&service).await;
    service.stop_turn(session);
}

#[tokio::test]
async fn resolving_an_unknown_or_finished_approval_is_an_error() {
    let (service, _) = with_agent(Scripted::text("x"));
    assert!(matches!(service.resolve_approval("nu", true), Err(ServiceError::UnknownApproval)));
}

#[tokio::test]
async fn startup_marks_unfinished_turns_as_interrupted() {
    let store = store_with_unfinished_session();
    let service = Service::new(store.clone(), scripted_agents()).unwrap();
    assert!(matches!(last_event(&service, 1), Event::TurnFailed { message, .. } if message.contains("interrupted")));
}

#[tokio::test]
async fn an_agent_that_cannot_be_built_is_a_failed_turn_that_keeps_the_prompt() {
    let (service, _) = with_agent(Scripted::text("x"));
    let session = service.store().create_session(1, "lipsa").unwrap();
    service.start_turn(session, "promptul meu".into(), PermissionMode::FullAccess).unwrap();
    wait_until_stopped(&service, session).await;
    let events = service.store().events(session).unwrap();
    assert_eq!(events[0].event, Event::UserMessage { text: "promptul meu".into() });
    assert!(matches!(&events[1].event, Event::TurnFailed { message, .. } if message.contains("lipsa")));
}

#[tokio::test]
async fn handoff_is_busy_while_the_source_runs_and_returns_the_new_session() {
    let (service, session) = with_agent(Scripted::blocking());
    service.start_turn(session, "a".into(), PermissionMode::FullAccess).unwrap();
    assert!(matches!(
        service.start_handoff(session, "scripted".into(), "b".into(), PermissionMode::FullAccess),
        Err(ServiceError::Busy)
    ));
    service.stop_turn(session);
}

#[tokio::test]
async fn handoff_runs_in_a_new_session_and_reports_back() {
    let (service, session) = with_agent(Scripted::text("rezolvat"));
    service.start_turn(session, "a".into(), PermissionMode::FullAccess).unwrap();
    wait_until_stopped(&service, session).await;
    let new = service.start_handoff(session, "scripted".into(), "b".into(), PermissionMode::FullAccess).unwrap();
    assert_ne!(new, session);
    wait_until_stopped(&service, new).await;
    wait_until_stopped(&service, session).await;
    assert_eq!(
        last_event(&service, session),
        Event::HandoffResult { to_session: new, text: "rezolvat".into() }
    );
}

#[tokio::test]
async fn a_deleted_project_folder_is_a_clear_failed_turn() {
    let (service, session) = with_agent(Scripted::text("x"));
    service.store().set_project_path(1, Some("E:/nu/exista/deloc")).unwrap();
    service.start_turn(session, "a".into(), PermissionMode::FullAccess).unwrap();
    wait_until_stopped(&service, session).await;
    assert!(matches!(last_event(&service, session), Event::TurnFailed { message, .. } if message.contains("folder")));
}

#[tokio::test]
async fn the_agent_gets_the_project_id_and_folder() {
    let dir = tempfile::tempdir().unwrap();
    let (service, session) = with_agent(Scripted::text("x"));
    service.store().set_project_path(1, Some(dir.path().to_str().unwrap())).unwrap();
    service.start_turn(session, "a".into(), PermissionMode::FullAccess).unwrap();
    wait_until_stopped(&service, session).await;
    assert!(matches!(last_event(&service, session), Event::TurnCompleted { .. }));
}

#[tokio::test]
async fn oversized_event_is_announced_not_broadcast() {
    let output = "x".repeat(300 * 1024);
    let big = Event::ToolResult { id: "t1".into(), output: output.clone(), is_error: false };
    let (service, session) = with_agent(Scripted::events(vec![big.clone()]));
    let mut updates = service.subscribe();
    service.start_turn(session, "a".into(), PermissionMode::FullAccess).unwrap();
    let notifications = collect_notifications_until_stopped(&mut updates).await;
    assert!(!notifications.iter().any(|n| matches!(n, Notification::Event(stored) if stored.event == big)));
    let seq = notifications
        .iter()
        .find_map(|n| match n {
            Notification::EventTooLarge { seq, .. } => Some(*seq),
            _ => None,
        })
        .expect("el trebuia anunțat ca prea mare");
    let saved = service.store().events(session).unwrap().into_iter().find(|stored| stored.seq == seq).unwrap();
    assert_eq!(saved.event, big);
}

#[tokio::test]
async fn write_failure_reaches_clients() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vatra.db");
    let store = Arc::new(Store::open(&path).unwrap());
    let project = store.create_project("p").unwrap();
    let session = store.create_session(project, "scripted").unwrap();
    let agents = Arc::new(ScriptedFactory(Scripted::breaks_writes(path)));
    let service = Service::new(store, agents).unwrap();
    let mut updates = service.subscribe();
    service.start_turn(session, "a".into(), PermissionMode::FullAccess).unwrap();
    let notifications = collect_notifications_until_stopped(&mut updates).await;
    let problem = |n: &Notification| matches!(n, Notification::Problem { session_id: id, .. } if *id == session);
    assert!(notifications.iter().any(problem));
}

#[tokio::test]
async fn approval_after_release_is_denied() {
    let (service, session) = with_agent(Scripted::text("gata"));
    service.start_turn(session, "a".into(), PermissionMode::Supervised).unwrap();
    wait_until_stopped(&service, session).await;
    let answer = service.request_approval(session, "unealta", json!({}));
    let denied = tokio::time::timeout(Duration::from_secs(1), answer).await.unwrap().unwrap();
    assert!(!denied);
    assert!(service.pending_approvals().is_empty());
}

#[tokio::test]
async fn stopping_a_turn_reports_when_its_failure_cannot_be_saved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vatra.db");
    let store = Arc::new(Store::open(&path).unwrap());
    let project = store.create_project("p").unwrap();
    let session = store.create_session(project, "scripted").unwrap();
    let service = Service::new(store, Arc::new(ScriptedFactory(Scripted::blocking()))).unwrap();
    let mut updates = service.subscribe();
    service.start_turn(session, "a".into(), PermissionMode::FullAccess).unwrap();
    loop {
        let notification = tokio::time::timeout(Duration::from_secs(10), updates.recv()).await.unwrap().unwrap();
        if matches!(notification, Notification::Event(stored) if matches!(stored.event, Event::UserMessage { .. })) {
            break;
        }
    }
    let connection = rusqlite::Connection::open(&path).unwrap();
    let trigger = "CREATE TRIGGER refuse_writes BEFORE INSERT ON events \
                   BEGIN SELECT RAISE(ABORT, 'disk full'); END;";
    connection.execute_batch(trigger).unwrap();
    service.stop_turn(session);
    let notifications = collect_notifications_until_stopped(&mut updates).await;
    let problem = |n: &Notification| matches!(n, Notification::Problem { session_id: id, .. } if *id == session);
    assert!(notifications.iter().any(problem));
}

#[tokio::test]
async fn ask_waits_for_answer() {
    let (service, session) = with_agent(Scripted::blocking());
    service.start_turn(session, "a".into(), PermissionMode::Supervised).unwrap();
    let mut answer = service.request_question(session, json!([{"question": "Continui?"}]));
    let pending = service.pending_questions();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].session_id, session);
    assert_eq!(pending[0].questions, json!([{"question": "Continui?"}]));
    // Nothing comes back until the user answers.
    assert!(tokio::time::timeout(Duration::from_millis(50), &mut answer).await.is_err());

    service.answer_question(&pending[0].id, Some(json!({"0": "da"}))).unwrap();

    let received = tokio::time::timeout(Duration::from_secs(1), answer).await.unwrap().unwrap();
    assert_eq!(received, Some(json!({"0": "da"})));
    assert!(service.pending_questions().is_empty());
    service.stop_turn(session);
    wait_until_stopped(&service, session).await;
}

#[tokio::test]
async fn ask_stopped_turn_returns_no_answer() {
    let (service, session) = with_agent(Scripted::blocking());
    service.start_turn(session, "a".into(), PermissionMode::Supervised).unwrap();
    let answer = service.request_question(session, json!([{"question": "Continui?"}]));

    service.stop_turn(session);

    let received = tokio::time::timeout(Duration::from_secs(1), answer).await.unwrap().unwrap();
    assert_eq!(received, None);
    wait_until_stopped(&service, session).await;
    assert!(service.pending_questions().is_empty());
}

#[tokio::test]
async fn question_after_release_gets_no_answer() {
    let (service, session) = with_agent(Scripted::text("gata"));
    service.start_turn(session, "a".into(), PermissionMode::Supervised).unwrap();
    wait_until_stopped(&service, session).await;
    let answer = service.request_question(session, json!([{"question": "Continui?"}]));
    let received = tokio::time::timeout(Duration::from_secs(1), answer).await.unwrap().unwrap();
    assert_eq!(received, None);
    assert!(service.pending_questions().is_empty());
}

#[tokio::test]
async fn question_survives_reconnect() {
    let (service, session) = with_agent(Scripted::blocking());
    let first = service.subscribe();
    service.start_turn(session, "a".into(), PermissionMode::Supervised).unwrap();
    let _answer = service.request_question(session, json!([{"question": "Continui?"}]));
    drop(first);

    let listed = service.pending_questions();

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].questions, json!([{"question": "Continui?"}]));
    service.stop_turn(session);
    wait_until_stopped(&service, session).await;
}

#[tokio::test]
async fn question_and_resolution_are_broadcast() {
    let (service, session) = with_agent(Scripted::blocking());
    let mut updates = service.subscribe();
    service.start_turn(session, "a".into(), PermissionMode::Supervised).unwrap();
    let _answer = service.request_question(session, json!([{"question": "Continui?"}]));
    let id = service.pending_questions()[0].id.clone();
    service.answer_question(&id, None).unwrap();
    service.stop_turn(session);
    let notifications = collect_notifications_until_stopped(&mut updates).await;
    assert!(notifications.iter().any(|n| matches!(n, Notification::Question(request) if request.id == id)));
    let resolved = |n: &Notification| matches!(n, Notification::QuestionResolved { id: r } if *r == id);
    assert!(notifications.iter().any(resolved));
}

#[tokio::test]
async fn resolving_an_unknown_question_is_an_error() {
    let (service, _) = with_agent(Scripted::text("gata"));
    assert!(matches!(service.answer_question("nu", None), Err(ServiceError::UnknownQuestion)));
}
