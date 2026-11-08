use crate::workspace::rpc::RpcError;
use crate::{Notification, Service};
use serde::Deserialize;
use serde_json::{Value, json};
use vatra_core::{StoreError, WorkspaceError};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    project_id: i64,
    limit: Option<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ack {
    project_id: i64,
    id: i64,
    generation: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    language: String,
}
fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(|e| RpcError::invalid(e.to_string()))
}
fn stored(error: StoreError) -> RpcError {
    match error {
        StoreError::Invalid(message) => RpcError::invalid(message),
        e => WorkspaceError::Store(e).into(),
    }
}
pub fn handle(service: &Service, method: &str, params: Value) -> Result<Value, RpcError> {
    let store = service.store();
    match method {
        "reminders.list" => {
            let r: List = parse(params)?;
            if !store
                .list_projects()
                .map_err(stored)?
                .iter()
                .any(|p| p.id == r.project_id)
            {
                return Err(WorkspaceError::NotFound.into());
            }
            let limit = r.limit.unwrap_or(50);
            if !(1..=100).contains(&limit) {
                return Err(RpcError::invalid("Choose a limit between 1 and 100."));
            }
            let reminders=store.reminder_inbox(r.project_id,limit).map_err(stored)?;
            let mut rows=Vec::new();
            for reminder in reminders {let title=store.workspace_item_title(r.project_id,reminder.item_id,true).map_err(stored)?;let mut row=json!(reminder);row["title"]=json!(title.unwrap_or_default());rows.push(row);}
            Ok(json!(rows))
        }
        "reminders.ack" => {
            let r: Ack = parse(params)?;
            store
                .ack_reminder(r.project_id, r.id, r.generation)
                .map_err(stored)?;
            service.notify(Notification::RemindersChanged {
                project_id: r.project_id,
            });
            Ok(json!({}))
        }
        "reminders.status" => {
            if params != json!({}) {
                return Err(RpcError::invalid("No parameters are accepted."));
            }
            let state=match service.reminder_notifier().status() {super::NotifyOutcome::Submitted=>"enabled",super::NotifyOutcome::Disabled=>"disabled",super::NotifyOutcome::Unavailable=>"unavailable"};
            Ok(json!({"state":state}))
        }
        "reminders.preferences.set" => {
            let r: Preferences = parse(params)?;
            if !matches!(r.language.as_str(), "en" | "ro") {
                return Err(RpcError::invalid("Choose en or ro."));
            }
            store
                .set_app_setting("reminders.language", Some(&r.language))
                .map_err(stored)?;
            Ok(json!({}))
        }
        _ => Err(RpcError {
            code: -32601,
            message: "method_not_found".into(),
            data: Value::Null,
        }),
    }
}
