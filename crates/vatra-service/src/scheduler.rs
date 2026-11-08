//! Runs the tasks a person scheduled. A due task opens its own conversation with the chosen agent and sends its
//! prompt in supervised mode: every action that needs approval waits for the person, as in any conversation. The run
//! is saved, so it survives a restart and leaves a result that can be read later.

use std::sync::{Arc, Weak};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use vatra_core::{Event, NewCalendarEvent, NewScheduledTask, PermissionMode};

use crate::agents::TurnOptions;
use crate::service::{Notification, Service};

/// A run that has not ended after this long is stopped and marked failed.
const RUN_LIMIT: Duration = Duration::from_secs(6 * 3600);

pub fn now() -> i64 {
    crate::workspace::rpc::now()
}

/// The background loop. Dropping it stops the loop.
pub struct SchedulerLoop {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for SchedulerLoop {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn start(service: &Arc<Service>) -> SchedulerLoop {
    let weak: Weak<Service> = Arc::downgrade(service);
    let store = Arc::clone(service.store());
    // Runs that were going when the service stopped can't be resumed.
    let _ = store.recover_runs(now());
    let task = tokio::spawn(async move {
        let mut ticks = tokio::time::interval(Duration::from_secs(5));
        loop {
            ticks.tick().await;
            let Some(service) = weak.upgrade() else { break };
            tick(&service, now()).await;
        }
    });
    SchedulerLoop { task }
}

/// Starts every task that is due at `at`.
pub async fn tick(service: &Arc<Service>, at: i64) {
    let store = Arc::clone(service.store());
    let claimed = tokio::task::spawn_blocking(move || store.claim_due_tasks(at, 10)).await;
    let Ok(Ok(claimed)) = claimed else { return };
    for (task, run) in claimed {
        launch(service, task, run);
    }
}

fn launch(service: &Arc<Service>, task: vatra_core::ScheduledTask, run: i64) {
    let service = Arc::clone(service);
    tokio::spawn(async move {
        let outcome = execute(&service, &task, run).await;
        let (state, summary) = match outcome {
            Ok(summary) => ("done", summary),
            Err(message) => ("failed", message),
        };
        let _ = service.store().finish_run(run, state, Some(&summary), now());
        service.notify(Notification::ScheduleChanged);
        crate::telegram::notify_task(&service, &task.name, state == "done", &summary).await;
        crate::ntfy::notify(&service, &format!("{}: {}", if state == "done" { "Done" } else { "Failed" }, task.name), &summary).await;
    });
    // The run exists as soon as it is claimed.
}

/// A research task: the question is researched like one the person started, and the run keeps a link to the report.
async fn execute_research(service: &Arc<Service>, task: &vatra_core::ScheduledTask, run: i64) -> Result<String, String> {
    let depth = task.options.get("depth").and_then(|d| d.as_str()).unwrap_or("standard").to_string();
    let options = serde_json::json!({
        "small_model": task.options.get("small_model").and_then(|v| v.as_bool()).unwrap_or(false),
        "context": task.options.get("context").and_then(|v| v.as_str()).unwrap_or(""),
    });
    let agent = Some(task.agent.clone()).filter(|a| !a.is_empty());
    let id = crate::research::start(service, &task.prompt, &depth, options, agent).map_err(|(_, m)| m)?;
    service.store().attach_run_research(run, id).map_err(|e| e.to_string())?;
    // The research runs in the background; this only waits for it to end.
    let started = std::time::Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let current = service.store().research_run(id).map_err(|e| e.to_string())?.ok_or("The research disappeared.")?;
        match current.state.as_str() {
            "done" => {
                let text = current.report.unwrap_or_default();
                let head: String = text.split("\n\nSources\n").next().unwrap_or("").chars().take(1500).collect();
                return Ok(head);
            }
            "failed" | "cancelled" => return Err(current.error.unwrap_or_else(|| "The research did not finish.".into())),
            _ => {}
        }
        if started.elapsed() > RUN_LIMIT {
            return Err("The research ran for too long.".into());
        }
    }
}

async fn execute(service: &Arc<Service>, task: &vatra_core::ScheduledTask, run: i64) -> Result<String, String> {
    service.notify(Notification::ScheduleChanged);
    if task.kind == "research" {
        return execute_research(service, task, run).await;
    }
    if !service.agents().names().contains(&task.agent) {
        return Err(format!("The agent `{}` is no longer set up.", task.agent));
    }
    let session = service
        .store()
        .create_session(task.project_id, &task.agent)
        .map_err(|e| e.to_string())?;
    let _ = service.store().rename_session(session, &task.name);
    service.store().attach_run_session(run, session).map_err(|e| e.to_string())?;
    let prompt = format!(
        "This is a task scheduled by the person for this time: {}.\n\n{}",
        task.name, task.prompt
    );
    service
        .start_turn_with_options(session, prompt, PermissionMode::Supervised, TurnOptions::default())
        .map_err(|e| e.to_string())?;
    // The turn belongs to its conversation and is stopped like any other; this only waits for it to end.
    let started = std::time::Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(400)).await;
        if !service.is_running(session) {
            break;
        }
        if started.elapsed() > RUN_LIMIT {
            service.stop_turn(session);
            return Err("The task ran for too long and was stopped.".into());
        }
    }
    let events = service.store().events(session).map_err(|e| e.to_string())?;
    let mut text = String::new();
    for stored in events.iter().rev() {
        match &stored.event {
            Event::TurnFailed { message, .. } => return Err(message.clone()),
            Event::AssistantText { text: t } if text.is_empty() => text = t.clone(),
            Event::TurnCompleted { .. } => {}
            _ => {}
        }
        if !text.is_empty() {
            break;
        }
    }
    if text.is_empty() {
        text = "The task finished without a written answer.".into();
    }
    Ok(text)
}

// ---------------------------------------------------------------------------------------------- RPC

type Rpc = Result<Value, (i64, String)>;
const INVALID: i64 = -32602;
const NOT_FOUND: i64 = 1100;

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, (i64, String)> {
    serde_json::from_value(params).map_err(|e| (INVALID, e.to_string()))
}

fn store_error(error: vatra_core::StoreError) -> (i64, String) {
    match error {
        vatra_core::StoreError::Invalid(message) => (INVALID, message),
        other => (1000, other.to_string()),
    }
}

pub fn handles(method: &str) -> bool {
    method.starts_with("schedule.") || method.starts_with("calendar.")
}

#[derive(Deserialize)]
struct Id {
    id: i64,
}

#[derive(Deserialize)]
struct Save {
    #[serde(default)]
    id: Option<i64>,
    #[serde(flatten)]
    task: NewScheduledTask,
    #[serde(default = "yes")]
    enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct Range {
    from: i64,
    to: i64,
}

pub async fn handle(service: &Arc<Service>, method: &str, params: Value) -> Rpc {
    let store = service.store();
    match method {
        "schedule.list" => Ok(json!({
            "tasks": store.scheduled_tasks().map_err(store_error)?,
            "runs": store.scheduled_runs(None, 50).map_err(store_error)?,
        })),
        "schedule.save" => {
            let request: Save = parse(params)?;
            // A research task may leave the agent empty: research then uses the default model.
            let research_default = request.task.kind == "research" && request.task.agent.is_empty();
            if !research_default && !service.agents().names().contains(&request.task.agent) {
                return Err((INVALID, format!("unknown agent `{}`", request.task.agent)));
            }
            if let Some(project) = request.task.project_id
                && !store.list_projects().map_err(store_error)?.iter().any(|p| p.id == project)
            {
                return Err((INVALID, format!("unknown project `{project}`")));
            }
            let id = match request.id {
                Some(id) => {
                    if !store.update_scheduled_task(id, &request.task, request.enabled, now()).map_err(store_error)? {
                        return Err((NOT_FOUND, "this task doesn't exist".into()));
                    }
                    id
                }
                None => store.create_scheduled_task(&request.task, now()).map_err(store_error)?,
            };
            service.notify(Notification::ScheduleChanged);
            Ok(json!({"id": id}))
        }
        "schedule.delete" => {
            let request: Id = parse(params)?;
            let removed = store.delete_scheduled_task(request.id).map_err(store_error)?;
            service.notify(Notification::ScheduleChanged);
            Ok(json!({"removed": removed}))
        }
        "schedule.run_now" => {
            let request: Id = parse(params)?;
            let task = store
                .scheduled_task(request.id)
                .map_err(store_error)?
                .ok_or((NOT_FOUND, "this task doesn't exist".into()))?;
            let run = store.claim_task_now(task.id, now()).map_err(store_error)?;
            let Some(run) = run else { return Err((1001, "this task is already running".into())) };
            launch(service, task, run);
            Ok(json!({"run": run}))
        }
        "schedule.runs" => {
            let request: Id = parse(params)?;
            Ok(json!(store.scheduled_runs(Some(request.id), 50).map_err(store_error)?))
        }
        "calendar.list" => {
            let request: Range = parse(params)?;
            if request.to <= request.from || request.to - request.from > 400 * 86_400 {
                return Err((INVALID, "the range of dates isn't valid".into()));
            }
            Ok(json!(store.calendar_occurrences(request.from, request.to).map_err(store_error)?))
        }
        "calendar.get" => {
            let request: Id = parse(params)?;
            store
                .calendar_event(request.id)
                .map_err(store_error)?
                .map(|event| json!(event))
                .ok_or((NOT_FOUND, "this event doesn't exist".into()))
        }
        "calendar.save" => {
            #[derive(Deserialize)]
            struct SaveEvent {
                #[serde(default)]
                id: Option<i64>,
                #[serde(flatten)]
                event: NewCalendarEvent,
            }
            let mut request: SaveEvent = parse(params)?;
            if let Some(id) = request.id {
                let existing = store
                    .calendar_event(id)
                    .map_err(store_error)?
                    .ok_or((NOT_FOUND, "this event doesn't exist".into()))?;
                // An edit keeps the identity the server knows the event by.
                request.event.uid = existing.uid;
                request.event.calendar = existing.calendar;
                request.event.source = existing.source;
                request.event.href = existing.href;
                request.event.etag = existing.etag;
            }
            let id = store.save_calendar_event(&request.event, now()).map_err(store_error)?;
            service.notify(Notification::CalendarChanged);
            crate::calendar::push_if_remote(service, id);
            Ok(json!({"id": id}))
        }
        "calendar.delete" => {
            let request: Id = parse(params)?;
            let existing = store.calendar_event(request.id).map_err(store_error)?;
            let removed = store.delete_calendar_event(request.id).map_err(store_error)?;
            if let Some(event) = existing {
                crate::calendar::delete_remote(service, event);
            }
            service.notify(Notification::CalendarChanged);
            Ok(json!({"removed": removed}))
        }
        "calendar.import_ics" => {
            #[derive(Deserialize)]
            struct Import {
                text: String,
            }
            let request: Import = parse(params)?;
            if request.text.len() > 4 * 1024 * 1024 {
                return Err((INVALID, "this calendar file is too large".into()));
            }
            let events = crate::calendar::parse_ics(&request.text);
            let mut saved = 0;
            for mut event in events {
                event.calendar = "local".into();
                event.source = "ics".into();
                if store.save_calendar_event(&event, now()).is_ok() {
                    saved += 1;
                }
            }
            service.notify(Notification::CalendarChanged);
            Ok(json!({"imported": saved}))
        }
        "calendar.export_ics" => Ok(json!({"text": crate::calendar::export_ics(store).map_err(store_error)?})),
        "calendar.accounts" | "calendar.sync" => crate::calendar::handle(service, method, params).await,
        _ => Err((-32601, format!("unknown method `{method}`"))),
    }
}
