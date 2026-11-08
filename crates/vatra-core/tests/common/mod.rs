#![allow(dead_code)]

use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;
use vatra_core::{Agent, AgentError, Event, Store, TurnOutcome, TurnRequest};

pub struct Scripted {
    events: Vec<Event>,
    native: Option<String>,
    failure: Option<String>,
    last: Mutex<Option<TurnRequest>>,
}

impl Scripted {
    pub fn ok(events: Vec<Event>, native: Option<&str>) -> Scripted {
        Scripted { events, native: native.map(String::from), failure: None, last: Mutex::new(None) }
    }

    pub fn fail(events: Vec<Event>, message: &str) -> Scripted {
        Scripted { events, native: None, failure: Some(message.into()), last: Mutex::new(None) }
    }

    pub fn recording() -> Scripted {
        Scripted::ok(vec![], None)
    }

    pub fn last_request(&self) -> TurnRequest {
        self.last.lock().unwrap().clone().expect("agentul n-a rulat")
    }
}

#[async_trait]
impl Agent for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn run_turn(
        &self,
        request: TurnRequest,
        sink: UnboundedSender<Event>,
    ) -> Result<TurnOutcome, AgentError> {
        *self.last.lock().unwrap() = Some(request);
        for event in &self.events {
            sink.send(event.clone()).unwrap();
        }
        match &self.failure {
            Some(message) => Err(AgentError::Failed(message.clone())),
            None => Ok(TurnOutcome { native_session: self.native.clone() }),
        }
    }
}

pub fn fixture() -> (Store, i64) {
    let store = Store::open_in_memory().unwrap();
    let project = store.create_project("p").unwrap();
    let session = store.create_session(project, "scripted").unwrap();
    (store, session)
}
