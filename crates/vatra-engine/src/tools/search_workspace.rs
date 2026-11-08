use super::{Tool, ToolClass, ToolContext, ToolOutput};
use crate::ToolDef;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use vatra_core::{ITEM_KINDS, Store, WorkspaceTurnContext};

pub fn search_workspace_tool(store: Arc<Store>) -> Arc<dyn Tool> {
    Arc::new(SearchWorkspace { store, scope: None })
}
pub fn scoped_search_workspace_tool(store: Arc<Store>, scope: WorkspaceTurnContext) -> Arc<dyn Tool> {
    Arc::new(SearchWorkspace { store, scope: Some(scope) })
}
struct SearchWorkspace {
    store: Arc<Store>,
    scope: Option<WorkspaceTurnContext>,
}
#[async_trait]
impl Tool for SearchWorkspace {
    fn def(&self) -> ToolDef {
        ToolDef{name:"search_workspace".into(),description:"Search the person's documents, notes, tasks, events, messages and drafts kept in Vatra. Returns kind, id, title and matching text.".into(),parameters:json!({"type":"object","properties":{"query":{"type":"string"},"kinds":{"type":"array","maxItems":6,"items":{"type":"string","enum":ITEM_KINDS}},"limit":{"type":"integer","minimum":1,"maximum":100}},"required":["query"]})}
    }
    fn class(&self, _: &Value, _: &ToolContext) -> ToolClass {
        ToolClass::Free
    }
    fn parallel_safe(&self) -> bool {
        true
    }
    async fn call(&self, args: Value, _: &ToolContext) -> ToolOutput {
        if let Some(scope) = &self.scope
            && self.store.check_workspace_context(scope).is_err()
        {
            return ToolOutput::error("The workspace turn is no longer active.");
        }
        let Some(query) = args.get("query").and_then(Value::as_str) else {
            return ToolOutput::error("query is required");
        };
        let kinds: Vec<&str> = match args.get("kinds") {
            None => vec![],
            Some(Value::Array(kinds)) if kinds.len() <= ITEM_KINDS.len() => {
                let Some(kinds) = kinds.iter().map(Value::as_str).collect::<Option<Vec<_>>>() else {
                    return ToolOutput::error("kinds must contain only valid workspace kind names.");
                };
                kinds
            }
            Some(_) => return ToolOutput::error("kinds must be an array of at most six workspace kind names."),
        };
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(20)
            .min(100) as usize;
        match self.store.search_workspace_scoped(self.scope.as_ref().map(|scope| scope.project_id), query, &kinds, limit) {
            Ok(hits) => ToolOutput::ok(serde_json::to_string(&hits).unwrap_or_default()),
            Err(_) => ToolOutput::error("The workspace could not be searched."),
        }
    }
}
