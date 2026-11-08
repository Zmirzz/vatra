use std::sync::{Arc, Weak};

use async_trait::async_trait;
use serde_json::Value;
use vatra_engine::Approver;

use crate::service::Service;

/// Asks the user in the service. The engine's policy decides which calls need asking.
pub struct ServiceApprover {
    service: Weak<Service>,
    session_id: i64,
}

impl ServiceApprover {
    pub fn new(service: &Arc<Service>, session_id: i64) -> ServiceApprover {
        ServiceApprover { service: Arc::downgrade(service), session_id }
    }
}

#[async_trait]
impl Approver for ServiceApprover {
    async fn approve(&self, name: &str, arguments: &Value) -> bool {
        let Some(service) = self.service.upgrade() else {
            return false;
        };
        let answer = service.request_approval(self.session_id, name, arguments.clone());
        answer.await.unwrap_or(false)
    }
}
