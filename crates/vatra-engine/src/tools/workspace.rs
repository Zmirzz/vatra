use super::{Tool, ToolClass, ToolContext, ToolOutput};
use crate::ToolDef;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use vatra_core::{Store, WorkspaceTaskInput, WorkspaceTurnContext};

pub fn workspace_tools(store: Arc<Store>, scope: WorkspaceTurnContext) -> Vec<Arc<dyn Tool>> {
    [
        "read_workspace_item",
        "propose_workspace_edit",
        "propose_workspace_tasks",
    ]
    .into_iter()
    .map(|name| {
        Arc::new(WorkspaceTool {
            store: store.clone(),
            scope: scope.clone(),
            name,
        }) as Arc<dyn Tool>
    })
    .collect()
}
pub fn call_workspace_tool(
    store: Arc<Store>,
    scope: WorkspaceTurnContext,
    name: &str,
    args: Value,
) -> ToolOutput {
    let Some(name) = [
        "read_workspace_item",
        "propose_workspace_edit",
        "propose_workspace_tasks",
    ]
    .into_iter()
    .find(|n| *n == name) else {
        return ToolOutput::error("Unknown workspace tool.");
    };
    match (WorkspaceTool { store, scope, name }).execute(args) {
        Ok(v) => ToolOutput::ok(v.to_string()),
        Err(e) => ToolOutput::error(e),
    }
}
struct WorkspaceTool {
    store: Arc<Store>,
    scope: WorkspaceTurnContext,
    name: &'static str,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    offset: Option<usize>,
    limit: Option<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    client_op_id: String,
    replacement: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tasks {
    client_op_id: String,
    tasks: Vec<WorkspaceTaskInput>,
}
impl WorkspaceTool {
    fn execute(&self, args: Value) -> Result<Value, String> {
        self.store
            .check_workspace_context(&self.scope)
            .map_err(|e| e.to_string())?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .min(i64::MAX as u64) as i64;
        match self.name {
            "read_workspace_item" => {
                let r: Read = serde_json::from_value(args).map_err(|e| e.to_string())?;
                let offset = r.offset.unwrap_or(0);
                let limit = r.limit.unwrap_or(8000);
                if !(1..=16000).contains(&limit) || offset > self.scope.base_body.chars().count() {
                    return Err(
                        "Choose an existing offset and a limit between 1 and 16000 characters."
                            .into(),
                    );
                }
                let body: String = self
                    .scope
                    .base_body
                    .chars()
                    .skip(offset)
                    .take(limit)
                    .collect();
                let next = offset + body.chars().count();
                Ok(
                    json!({"item_id":self.scope.item_id,"revision":self.scope.base_revision,"body":body,"next_offset":if next<self.scope.base_body.chars().count(){Some(next)}else{None},"selection":self.scope.selection}),
                )
            }
            "propose_workspace_edit" => {
                let r: Edit = serde_json::from_value(args).map_err(|e| e.to_string())?;
                let id = self
                    .store
                    .propose_workspace_edit(&self.scope, &r.client_op_id, &r.replacement, now)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"proposal_id":id,"state":"pending","applied":false}))
            }
            "propose_workspace_tasks" => {
                let r: Tasks = serde_json::from_value(args).map_err(|e| e.to_string())?;
                let id = self
                    .store
                    .propose_workspace_tasks(&self.scope, &r.client_op_id, r.tasks, now)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"proposal_id":id,"state":"pending","applied":false}))
            }
            _ => Err("Unknown workspace tool.".into()),
        }
    }
}
#[async_trait]
impl Tool for WorkspaceTool {
    fn def(&self) -> ToolDef {
        let (description, properties, required) = match self.name {
            "read_workspace_item" => (
                "Read a page of the fixed workspace document snapshot. Offset and limit count Unicode characters.",
                json!({"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":16000}}),
                json!([]),
            ),
            "propose_workspace_edit" => (
                "Save a pending suggestion for the user to review. replacement replaces only the selected text when a selection exists; otherwise it is the complete new document. Never applies changes. Reuse client_op_id only for an identical retry.",
                json!({"client_op_id":{"type":"string"},"replacement":{"type":"string"}}),
                json!(["client_op_id", "replacement"]),
            ),
            _ => (
                "Propose a batch of tasks derived from this document. The user must accept before any task is created. Reuse client_op_id only for an identical retry.",
                json!({"client_op_id":{"type":"string"},"tasks":{"type":"array","minItems":1,"maxItems":100,"items":{"type":"object","additionalProperties":false,"properties":{"title":{"type":"string"},"body":{"type":"string"},"fields":{"type":"object"}},"required":["title"]}}}),
                json!(["client_op_id", "tasks"]),
            ),
        };
        ToolDef {
            name: self.name.into(),
            description: description.into(),
            parameters: json!({"type":"object","additionalProperties":false,"properties":properties,"required":required}),
        }
    }
    fn class(&self, _: &Value, _: &ToolContext) -> ToolClass {
        ToolClass::Free
    }
    fn parallel_safe(&self) -> bool {
        true
    }
    async fn call(&self, args: Value, _: &ToolContext) -> ToolOutput {
        match self.execute(args) {
            Ok(v) => ToolOutput::ok(v.to_string()),
            Err(e) => ToolOutput::error(e),
        }
    }
}
