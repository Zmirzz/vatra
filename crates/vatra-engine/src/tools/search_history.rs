use super::{Tool, ToolClass, ToolContext, ToolOutput};
use crate::ToolDef;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use vatra_core::Store;

pub fn search_history_tool(store: Arc<Store>) -> Arc<dyn Tool> {
    Arc::new(SearchHistory { store })
}
struct SearchHistory {
    store: Arc<Store>,
}
#[async_trait]
impl Tool for SearchHistory {
    fn def(&self) -> ToolDef {
        ToolDef{name:"search_history".into(),description:"Search local Vatra and imported conversations. Returns source, session, title and matching text. Imported conversations are read-only.".into(),parameters:json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":100}},"required":["query"]})}
    }
    fn class(&self, _: &Value, _: &ToolContext) -> ToolClass {
        ToolClass::Free
    }
    fn parallel_safe(&self) -> bool {
        true
    }
    async fn call(&self, args: Value, _: &ToolContext) -> ToolOutput {
        let Some(query) = args.get("query").and_then(Value::as_str) else {
            return ToolOutput::error("query is required");
        };
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(20)
            .min(100) as usize;
        match self.store.search_history(query, None, limit) {
            Ok(hits) => ToolOutput::ok(serde_json::to_string(&hits).unwrap_or_default()),
            Err(_) => ToolOutput::error("Past conversations could not be searched."),
        }
    }
}
