mod common;

use std::time::Duration;

use common::*;
use serde_json::json;
use vatra_core::{Event, NewScheduledTask, Schedule};
use vatra_service::scheduler;
use wiremock::matchers::{header_exists, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn task(at: i64) -> NewScheduledTask {
    NewScheduledTask {
        name: "Rezumat zilnic".into(),
        prompt: "Rezuma ziua".into(),
        agent: "scripted".into(),
        project_id: None,
        schedule: Schedule::Once { at },
        kind: "agent".into(),
        options: json!({}),
    }
}

async fn wait_for_run(service: &std::sync::Arc<vatra_service::Service>, state: &str) -> vatra_core::ScheduledRun {
    for _ in 0..100 {
        let runs = service.store().scheduled_runs(None, 5).unwrap();
        if let Some(run) = runs.into_iter().find(|r| r.state == state) {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no run reached `{state}`");
}

#[tokio::test]
async fn a_due_task_runs_in_its_own_conversation_and_leaves_a_readable_result() {
    let (service, _) = with_agent(Scripted::text("Ziua a fost linistita."));
    let id = service.store().create_scheduled_task(&task(2000), 1000).unwrap();
    scheduler::tick(&service, 2001).await;
    let run = wait_for_run(&service, "done").await;
    assert_eq!(run.task_id, id);
    assert_eq!(run.summary.as_deref(), Some("Ziua a fost linistita."));
    let session = run.session_id.expect("the run keeps its conversation");
    let events = service.store().events(session).unwrap();
    assert!(events.iter().any(|e| matches!(&e.event, Event::UserMessage { text } if text.contains("Rezuma ziua"))));
    // The task was one-time: it is spent and does not run again.
    scheduler::tick(&service, 9999).await;
    assert_eq!(service.store().scheduled_runs(Some(id), 10).unwrap().len(), 1);
}

#[tokio::test]
async fn a_task_for_an_agent_that_is_gone_fails_with_a_reason() {
    let (service, _) = with_agent(Scripted::text("x"));
    let mut missing = task(2000);
    missing.agent = "disparut".into();
    service.store().create_scheduled_task(&missing, 1000).unwrap();
    scheduler::tick(&service, 2001).await;
    let run = wait_for_run(&service, "failed").await;
    assert!(run.summary.unwrap().contains("disparut"));
}

#[tokio::test]
async fn a_run_cut_short_by_a_restart_is_marked_and_not_run_again() {
    let (service, _) = with_agent(Scripted::blocking());
    let id = service
        .store()
        .create_scheduled_task(
            &NewScheduledTask { schedule: Schedule::Interval { every_secs: 3600 }, ..task(0) },
            1000,
        )
        .unwrap();
    let claimed = service.store().claim_due_tasks(1000 + 3601, 5).unwrap();
    assert_eq!(claimed.len(), 1);
    // The service stops here; a new one starts on the same data.
    let restarted = service.store().recover_runs(1000 + 3700).unwrap();
    assert_eq!(restarted, 1);
    let run = &service.store().scheduled_runs(Some(id), 5).unwrap()[0];
    assert_eq!(run.state, "interrupted");
}

#[tokio::test]
async fn events_are_saved_listed_exported_and_imported_through_the_rpc_methods() {
    let (service, _) = with_agent(Scripted::text("x"));
    let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a1\r\nDTSTART:20261110T080000Z\r\nDTEND:20261110T090000Z\r\nSUMMARY:Dentist\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let imported = scheduler::handle(&service, "calendar.import_ics", json!({"text": ics})).await.unwrap();
    assert_eq!(imported["imported"], 1);
    let from = vatra_core::parse_ical_time("20261109").unwrap().0;
    let listed = scheduler::handle(&service, "calendar.list", json!({"from": from, "to": from + 3 * 86_400})).await.unwrap();
    assert_eq!(listed[0]["title"], "Dentist");
    let saved = scheduler::handle(
        &service,
        "calendar.save",
        json!({"title": "Yoga", "starts_at": from + 100, "ends_at": from + 3700, "rrule": "FREQ=DAILY;COUNT=2"}),
    )
    .await
    .unwrap();
    assert!(saved["id"].as_i64().unwrap() > 0);
    let listed = scheduler::handle(&service, "calendar.list", json!({"from": from, "to": from + 3 * 86_400})).await.unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 3, "Dentist plus two Yoga days");
    let exported = scheduler::handle(&service, "calendar.export_ics", json!({})).await.unwrap();
    assert!(exported["text"].as_str().unwrap().contains("SUMMARY:Yoga"));
    assert!(scheduler::handle(&service, "calendar.list", json!({"from": 5, "to": 4})).await.is_err());
    assert!(scheduler::handle(&service, "calendar.save", json!({"title": "x", "starts_at": 9, "ends_at": 1})).await.is_err());
}

#[tokio::test]
async fn schedule_methods_validate_and_run_now() {
    let (service, _) = with_agent(Scripted::text("Gata acum."));
    let bad = scheduler::handle(
        &service,
        "schedule.save",
        json!({"name":"x","prompt":"y","agent":"scripted","schedule":{"kind":"interval","every_secs":5}}),
    )
    .await;
    assert!(bad.is_err());
    let unknown = scheduler::handle(
        &service,
        "schedule.save",
        json!({"name":"x","prompt":"y","agent":"nu-exista","schedule":{"kind":"daily","hour":9,"minute":0,"tz_offset_min":120}}),
    )
    .await;
    assert!(unknown.is_err());
    let saved = scheduler::handle(
        &service,
        "schedule.save",
        json!({"name":"Dimineata","prompt":"Spune buna","agent":"scripted","schedule":{"kind":"daily","hour":9,"minute":0,"tz_offset_min":120}}),
    )
    .await
    .unwrap();
    let id = saved["id"].as_i64().unwrap();
    scheduler::handle(&service, "schedule.run_now", json!({"id": id})).await.unwrap();
    let run = wait_for_run(&service, "done").await;
    assert_eq!(run.summary.as_deref(), Some("Gata acum."));
    let listing = scheduler::handle(&service, "schedule.list", json!({})).await.unwrap();
    assert_eq!(listing["tasks"][0]["enabled"], true);
    assert!(listing["tasks"][0]["next_run_at"].as_i64().is_some());
    scheduler::handle(&service, "schedule.delete", json!({"id": id})).await.unwrap();
    assert!(service.store().scheduled_tasks().unwrap().is_empty());
}

#[tokio::test]
async fn a_caldav_calendar_is_read_into_the_local_calendar_and_gone_events_leave() {
    let server = MockServer::start().await;
    let body = |uid: &str, title: &str| {
        format!(
            "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/cal/{uid}.ics</d:href><d:propstat><d:prop><d:getetag>\"{uid}\"</d:getetag><c:calendar-data xmlns:c=\"urn:ietf:params:xml:ns:caldav\">BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:{uid}\nDTSTART:20261120T100000Z\nDTEND:20261120T110000Z\nSUMMARY:{title}\nEND:VEVENT\nEND:VCALENDAR</c:calendar-data></d:prop></d:propstat></d:response></d:multistatus>"
        )
    };
    Mock::given(method("REPORT")).and(header_exists("authorization")).respond_with(ResponseTemplate::new(207).set_body_string(body("e1", "Din server"))).up_to_n_times(1).mount(&server).await;
    Mock::given(method("REPORT")).respond_with(ResponseTemplate::new(207).set_body_string("<d:multistatus xmlns:d=\"DAV:\"/>")).mount(&server).await;

    let account = vatra_service::calendar::Account {
        id: "cal-1".into(),
        url: format!("{}/cal/", server.uri()),
        username: "ana".into(),
        password: "parola".into(),
    };
    let events = vatra_service::calendar::fetch(&account).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].title, "Din server");
    assert_eq!(events[0].etag.as_deref(), Some("\"e1\""));
    assert!(events[0].href.as_deref().unwrap().ends_with("/cal/e1.ics"));
    assert_eq!(events[0].calendar, "cal-1");
    // The next read finds nothing, as when the event was deleted on the server.
    assert!(vatra_service::calendar::fetch(&account).await.unwrap().is_empty());

    let wrong = vatra_service::calendar::Account { url: "https://example.invalid.test/x".into(), ..account.clone() };
    assert!(vatra_service::calendar::fetch(&wrong).await.is_err());
    let plain = vatra_service::calendar::Account { url: "http://calendar.example.com/x".into(), ..account };
    assert!(vatra_service::calendar::fetch(&plain).await.unwrap_err().contains("https"));
}
