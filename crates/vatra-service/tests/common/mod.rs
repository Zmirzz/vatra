#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedSender;
use vatra_core::{Agent, AgentError, BriefFields, Event, Store, StoredEvent, Summarizer, TurnOutcome, TurnRequest};
use vatra_engine::Approver;
use vatra_service::{AgentFactory, BuildRequest, Notification, Service};

#[derive(Clone)]
pub enum Scripted {
    Text(String),
    Events(Vec<Event>),
    Blocking,
    AsksApproval(String),
    /// Makes every later insert into the journal fail, the way a full disk would.
    BreaksWrites(PathBuf),
}

impl Scripted {
    pub fn text(text: &str) -> Scripted {
        Scripted::Text(text.into())
    }

    pub fn events(events: Vec<Event>) -> Scripted {
        Scripted::Events(events)
    }

    pub fn blocking() -> Scripted {
        Scripted::Blocking
    }

    pub fn asks_approval(tool: &str) -> Scripted {
        Scripted::AsksApproval(tool.into())
    }

    pub fn breaks_writes(path: PathBuf) -> Scripted {
        Scripted::BreaksWrites(path)
    }
}

struct ScriptedAgent {
    script: Scripted,
    approver: Arc<dyn Approver>,
}

#[async_trait]
impl Agent for ScriptedAgent {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn run_turn(
        &self,
        _request: TurnRequest,
        sink: UnboundedSender<Event>,
    ) -> Result<TurnOutcome, AgentError> {
        match &self.script {
            Scripted::Text(text) => {
                sink.send(Event::AssistantText { text: text.clone() }).unwrap();
            }
            Scripted::Events(events) => {
                for event in events {
                    sink.send(event.clone()).unwrap();
                }
            }
            Scripted::BreaksWrites(path) => {
                let connection = rusqlite::Connection::open(path).unwrap();
                let trigger = "CREATE TRIGGER refuse_writes BEFORE INSERT ON events \
                               BEGIN SELECT RAISE(ABORT, 'disk full'); END;";
                connection.execute_batch(trigger).unwrap();
                sink.send(Event::AssistantText { text: "pierdut".into() }).unwrap();
            }
            Scripted::Blocking => tokio::time::sleep(Duration::from_secs(600)).await,
            Scripted::AsksApproval(tool) => {
                let allowed = self.approver.approve(tool, &json!({"a": 1})).await;
                sink.send(Event::AssistantText { text: format!("aprobat: {allowed}") }).unwrap();
            }
        }
        Ok(TurnOutcome::default())
    }
}

struct NoSummary;

#[async_trait]
impl Summarizer for NoSummary {
    async fn summarize(&self, _transcript: &str) -> Result<BriefFields, AgentError> {
        Err(AgentError::Failed("indisponibil".into()))
    }
}

pub struct ScriptedFactory(pub Scripted);

#[async_trait]
impl AgentFactory for ScriptedFactory {
    fn names(&self) -> Vec<String> {
        vec!["scripted".into()]
    }

    async fn build(&self, request: BuildRequest) -> Result<Arc<dyn Agent>, String> {
        if request.name != "scripted" {
            return Err(format!("agent necunoscut `{}`", request.name));
        }
        Ok(Arc::new(ScriptedAgent { script: self.0.clone(), approver: request.approver }))
    }

    fn summarizer(&self) -> Arc<dyn Summarizer> {
        Arc::new(NoSummary)
    }
}

pub fn scripted_agents() -> Arc<dyn AgentFactory> {
    Arc::new(ScriptedFactory(Scripted::text("gata")))
}

pub fn with_agent(script: Scripted) -> (Arc<Service>, i64) {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let project = store.create_project("p").unwrap();
    let session = store.create_session(project, "scripted").unwrap();
    (Service::new(store, Arc::new(ScriptedFactory(script))).unwrap(), session)
}

pub fn store_with_unfinished_session() -> Arc<Store> {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let project = store.create_project("p").unwrap();
    let session = store.create_session(project, "scripted").unwrap();
    store.append(session, &Event::UserMessage { text: "a".into() }).unwrap();
    store
}

pub async fn collect_until_stopped(updates: &mut broadcast::Receiver<Notification>) -> Vec<StoredEvent> {
    let mut events = vec![];
    for notification in collect_notifications_until_stopped(updates).await {
        if let Notification::Event(event) = notification {
            events.push(event);
        }
    }
    events
}

pub async fn collect_notifications_until_stopped(updates: &mut broadcast::Receiver<Notification>) -> Vec<Notification> {
    let mut notifications = vec![];
    loop {
        let notification = tokio::time::timeout(Duration::from_secs(10), updates.recv()).await.unwrap().unwrap();
        if matches!(notification, Notification::TurnState { running: false, .. }) {
            return notifications;
        }
        notifications.push(notification);
    }
}

pub async fn wait_for_approval(service: &Service) {
    for _ in 0..500 {
        if !service.pending_approvals().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("nu a apărut nicio cerere de aprobare");
}

pub async fn wait_until_stopped(service: &Service, session: i64) {
    for _ in 0..500 {
        if !service.is_running(session) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("tura nu s-a oprit");
}

pub fn last_event(service: &Service, session: i64) -> Event {
    service.store().events(session).unwrap().pop().unwrap().event
}
