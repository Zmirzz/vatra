use std::sync::{Arc, Weak};

use async_trait::async_trait;
use serde_json::Value;
use vatra_engine::Questioner;

use crate::service::Service;

/// Asks the user in the service. Like the approvals, a question waits for the answer or for the turn to end.
pub struct ServiceQuestioner {
    service: Weak<Service>,
    session_id: i64,
}

impl ServiceQuestioner {
    pub fn new(service: &Arc<Service>, session_id: i64) -> ServiceQuestioner {
        ServiceQuestioner { service: Arc::downgrade(service), session_id }
    }
}

#[async_trait]
impl Questioner for ServiceQuestioner {
    async fn ask(&self, questions: Value) -> Option<Value> {
        let service = self.service.upgrade()?;
        let answer = service.request_question(self.session_id, questions);
        answer.await.ok().flatten()
    }
}
