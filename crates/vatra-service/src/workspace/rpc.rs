//! Versioned workspace RPCs have structured conflicts without changing legacy RPC errors.
use super::files::{self, DocumentExportMode};
use crate::{Notification, Service};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use vatra_core::{DocumentFormat, ItemChange, NewItem, WorkspaceError, validate_workspace_content};

#[derive(Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Value,
}
impl RpcError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: "invalid_params".into(),
            data: json!({"detail":message.into()}),
        }
    }
    pub fn frame(self, id: Value) -> Value {
        json!({"jsonrpc":"2.0","id":id,"error":{"code":self.code,"message":self.message,"data":self.data}})
    }
}
impl From<WorkspaceError> for RpcError {
    fn from(error: WorkspaceError) -> Self {
        match error {
            WorkspaceError::Conflict(current) => Self {
                code: 1101,
                message: "workspace_conflict".into(),
                data: json!({"current":current}),
            },
            WorkspaceError::NotFound => Self {
                code: 1100,
                message: "not_found".into(),
                data: Value::Null,
            },
            WorkspaceError::Invalid(detail) => Self::invalid(detail),
            WorkspaceError::Store(_) => Self {
                code: 1000,
                message: "workspace_failed".into(),
                data: Value::Null,
            },
        }
    }
}
fn parse<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(|e| RpcError::invalid(e.to_string()))
}
pub fn handles(method: &str) -> bool {
    [
        "workspace.items.",
        "workspace.links.",
        "workspace.proposals.",
        "workspace.files.",
    ]
    .iter()
    .any(|p| method.starts_with(p))
}
pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    project_id: i64,
    #[serde(default)]
    kinds: Vec<String>,
    #[serde(default)]
    deleted: bool,
    after_id: Option<i64>,
    task_view: Option<vatra_core::TaskView>,
    today: Option<String>,
    limit: Option<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Get {
    project_id: i64,
    id: i64,
    #[serde(default)]
    include_deleted: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    project_id: i64,
    kind: String,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default = "empty_fields")]
    fields: Value,
}
fn empty_fields() -> Value {
    json!({})
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Save {
    project_id: i64,
    id: i64,
    expected_revision: u64,
    title: Option<String>,
    body: Option<String>,
    fields: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    project_id: i64,
    id: i64,
    expected_revision: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Link {
    project_id: i64,
    from_id: i64,
    to_id: i64,
    rel: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    project_id: i64,
    id: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposals {
    project_id: i64,
    target_id: Option<i64>,
    after_id: Option<i64>,
    limit: Option<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Import {
    project_id: i64,
    path: String,
    format: DocumentFormat,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Export {
    project_id: i64,
    id: i64,
    expected_revision: u64,
    path: String,
    #[serde(default)]
    mode: DocumentExportMode,
}
fn limit(value: Option<usize>) -> Result<usize, RpcError> {
    match value.unwrap_or(50) {
        n @ 1..=100 => Ok(n),
        _ => Err(RpcError::invalid("Choose a limit between 1 and 100.")),
    }
}

pub fn handle(service: &Arc<Service>, method: &str, params: Value) -> Result<Value, RpcError> {
    // Resolve the required project before any file access or mutation; never create it implicitly.
    let project = params
        .get("project_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| RpcError::invalid("project_id is required."))?;
    let store = service.store();
    if !store
        .list_projects()
        .map_err(WorkspaceError::from)?
        .iter()
        .any(|p| p.id == project)
    {
        return Err(WorkspaceError::NotFound.into());
    }
    let changed = |ids: Vec<i64>| {
        service.notify(Notification::WorkspaceChanged {
            project_id: project,
            item_ids: ids,
        })
    };
    match method {
        "workspace.items.list" => {
            let r: List = parse(params)?;
            let kinds: Vec<&str> = r.kinds.iter().map(String::as_str).collect();
            if let Some(view)=r.task_view {
                if kinds!=["task"] {return Err(RpcError::invalid("Task views require only task items."));}
                let today=r.today.as_deref().ok_or_else(||RpcError::invalid("A task view needs the local date."))?;
                return Ok(json!(store.workspace_task_previews(r.project_id,view,today,r.deleted,r.after_id,limit(r.limit)?)?));
            }
            if r.today.is_some() {return Err(RpcError::invalid("A local date requires a task view."));}
            Ok(json!(store.workspace_item_previews(
                r.project_id,
                &kinds,
                r.deleted,
                r.after_id,
                limit(r.limit)?
            )?))
        }
        "workspace.items.get" => {
            let r: Get = parse(params)?;
            let item=store.workspace_item(r.project_id, r.id, r.include_deleted)?;
            if item.body.len()>vatra_core::MAX_DOCUMENT_BYTES || serde_json::to_vec(&item.fields).map_err(WorkspaceError::from)?.len()>65536 || item.title.chars().count()>240 {
                return Err(RpcError::invalid("This older item exceeds the workspace editor limits. Export it before reducing its size."));
            }
            Ok(json!(item))
        }
        "workspace.items.create" => {
            let r: Create = parse(params)?;
            validate_workspace_content(&r.kind, &r.title, &r.body, &r.fields)?;
            let id = store
                .create_item(
                    &NewItem {
                        kind: r.kind,
                        title: r.title,
                        body: r.body,
                        fields: r.fields,
                        project_id: Some(r.project_id),
                    },
                    now(),
                )
                .map_err(WorkspaceError::from)?;
            let item = store.workspace_item(project, id, false)?;
            changed(vec![id]);
            Ok(json!(item))
        }
        "workspace.items.save" => {
            let r: Save = parse(params)?;
            let item = store.save_workspace_item(
                r.project_id,
                &ItemChange {
                    id: r.id,
                    expected_revision: r.expected_revision,
                    title: r.title,
                    body: r.body,
                    fields: r.fields,
                },
                now(),
            )?;
            changed(vec![item.id]);
            Ok(json!(item))
        }
        "workspace.items.delete" | "workspace.items.restore" => {
            let r: Change = parse(params)?;
            let item = if method.ends_with("delete") {
                store.delete_workspace_item(r.project_id, r.id, r.expected_revision, now())?
            } else {
                store.restore_workspace_item(r.project_id, r.id, r.expected_revision, now())?
            };
            changed(vec![item.id]);
            Ok(json!(item))
        }
        "workspace.links.list" => {
            let r: Get = parse(params)?;
            Ok(json!(store.workspace_links(r.project_id, r.id)?))
        }
        "workspace.links.add" | "workspace.links.remove" => {
            let r: Link = parse(params)?;
            if method.ends_with("add") {
                store.link_workspace_items(r.project_id, r.from_id, r.to_id, &r.rel)?;
            } else {
                store.unlink_workspace_items(r.project_id, r.from_id, r.to_id, &r.rel)?;
            }
            changed(vec![r.from_id, r.to_id]);
            Ok(json!({}))
        }
        "workspace.proposals.list" => {
            let r: Proposals = parse(params)?;
            if let Some(id) = r.target_id {
                store.workspace_item(project, id, true)?;
            }
            Ok(json!(store.workspace_proposals_page(
                r.project_id,
                r.target_id,
                r.after_id,
                limit(r.limit)?
            )?))
        }
        "workspace.proposals.accept" => {
            let r: Proposal = parse(params)?;
            let result = store.accept_workspace_proposal(r.project_id, r.id, now())?;
            changed(result.items.iter().map(|i| i.id).collect());
            service.notify(Notification::WorkspaceProposalsChanged {
                project_id: project,
                target_id: None,
            });
            Ok(json!(result))
        }
        "workspace.proposals.reject" => {
            let r: Proposal = parse(params)?;
            store.reject_workspace_proposal(r.project_id, r.id)?;
            service.notify(Notification::WorkspaceProposalsChanged {
                project_id: project,
                target_id: None,
            });
            Ok(json!({}))
        }
        "workspace.files.import" => {
            let r: Import = parse(params)?;
            let item =
                files::import_document(store, r.project_id, Path::new(&r.path), r.format, now())?;
            changed(vec![item.id]);
            Ok(json!(item))
        }
        "workspace.files.export" => {
            let r: Export = parse(params)?;
            files::export_document_with_mode(
                store,
                r.project_id,
                r.id,
                r.expected_revision,
                Path::new(&r.path),
                r.mode,
            )?;
            Ok(json!({}))
        }
        _ => Err(RpcError {
            code: -32601,
            message: "method_not_found".into(),
            data: Value::Null,
        }),
    }
}
