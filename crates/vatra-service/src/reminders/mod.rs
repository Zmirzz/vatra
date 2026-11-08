//! Service-owned reminder delivery. Persist the attempt before calling any OS API.
use serde::Serialize;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use vatra_core::{ReminderResult, Store, StoreError};
pub mod rpc;
pub mod windows;
#[derive(Clone, Debug, PartialEq)]
pub struct ReminderNotice {
    pub id: i64,
    pub generation: i64,
    pub title: String,
    pub language: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyOutcome {
    Submitted,
    Disabled,
    Unavailable,
}
pub trait ReminderNotifier: Send + Sync {
    fn show(&self, notice: &ReminderNotice) -> Result<NotifyOutcome, String>;
    fn status(&self) -> NotifyOutcome {
        NotifyOutcome::Unavailable
    }
}
pub struct UnavailableNotifier;
impl ReminderNotifier for UnavailableNotifier {
    fn show(&self, _: &ReminderNotice) -> Result<NotifyOutcome, String> {
        Ok(NotifyOutcome::Unavailable)
    }
}
pub struct ReminderDispatcher {
    store: Arc<Store>,
    notifier: Arc<dyn ReminderNotifier>,
    gate: Mutex<()>,
    cancelled: Arc<AtomicBool>,
    updates: Option<Arc<dyn Fn(i64) + Send + Sync>>,
}
impl ReminderDispatcher {
    pub fn new(store: Arc<Store>, notifier: Arc<dyn ReminderNotifier>) -> Self {
        Self {
            store,
            notifier,
            gate: Mutex::new(()),
            cancelled: Arc::new(AtomicBool::new(false)),
            updates: None,
        }
    }
    pub fn with_updates(mut self, updates: Arc<dyn Fn(i64) + Send + Sync>) -> Self {
        self.updates = Some(updates);
        self
    }
    pub fn tick(&self, now: i64) -> Result<(), StoreError> {
        let Ok(_guard) = self.gate.try_lock() else {
            return Ok(());
        };
        if self.cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        for reminder in self.store.claim_due_reminders(now, 50)? {
            if let Some(update) = &self.updates {
                update(reminder.project_id);
            }
            if self.cancelled.load(Ordering::Acquire) {
                return Ok(());
            }
            if !self
                .store
                .reminder_is_current(reminder.id, reminder.generation)?
            {
                continue;
            }
            let Ok(item) = self
                .store
                .workspace_item(reminder.project_id, reminder.item_id, false)
            else {
                continue;
            };
            let language = self
                .store
                .app_setting("reminders.language")?
                .filter(|v| v == "ro")
                .unwrap_or_else(|| "en".into());
            let notice = ReminderNotice {
                id: reminder.id,
                generation: reminder.generation,
                title: item.title.chars().take(240).collect(),
                language,
            };
            let result = match self.notifier.show(&notice) {
                Ok(NotifyOutcome::Submitted) => ReminderResult::Submitted,
                Ok(NotifyOutcome::Disabled) => ReminderResult::Blocked,
                Ok(NotifyOutcome::Unavailable) | Err(_) => ReminderResult::Failed,
            };
            self.store
                .finish_reminder(reminder.id, reminder.generation, result, now)?;
            if let Some(update) = &self.updates {
                update(reminder.project_id);
            }
        }
        Ok(())
    }
    pub fn start(self) -> ReminderLoop {
        let cancelled = self.cancelled.clone();
        let dispatcher = Arc::new(self);
        let task = tokio::spawn(async move {
            let mut ticks = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                ticks.tick().await;
                if dispatcher.cancelled.load(Ordering::Acquire) {
                    break;
                }
                let work = dispatcher.clone();
                let _ =
                    tokio::task::spawn_blocking(move || work.tick(crate::workspace::rpc::now()))
                        .await;
            }
        });
        ReminderLoop { cancelled, task }
    }
}
pub struct ReminderLoop {
    cancelled: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for ReminderLoop {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.task.abort();
    }
}
