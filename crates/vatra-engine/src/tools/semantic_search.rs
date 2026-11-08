use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Tool, ToolClass, ToolContext, ToolOutput};
use crate::ToolDef;

/// Looks a question up by meaning and returns the passages as JSON text. The service provides it, because it owns the
/// embedding account.
pub type SemanticSearchFn = Arc<dyn Fn(String, usize) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>> + Send + Sync>;

pub fn semantic_search_tool(search: SemanticSearchFn) -> Arc<dyn Tool> {
    Arc::new(SemanticSearch { search })
}

struct SemanticSearch {
    search: SemanticSearchFn,
}

#[async_trait]
impl Tool for SemanticSearch {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "search_by_meaning".into(),
            description: "Find what the person has written or researched by what it means, not by the exact words: documents, notes, tasks and \
                          research reports kept in Vatra. Use it when search_workspace finds nothing because the words differ. Returns kind, id, \
                          title, the best passage and a score from 0 to 1; a score under 0.3 is probably not related."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "What to look for, as a short sentence."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 20}
                },
                "required": ["query"]
            }),
        }
    }

    fn class(&self, _: &Value, _: &ToolContext) -> ToolClass {
        ToolClass::Free
    }

    fn parallel_safe(&self) -> bool {
        true
    }

    async fn call(&self, args: Value, _: &ToolContext) -> ToolOutput {
        let Some(query) = args.get("query").and_then(Value::as_str).map(str::trim).filter(|q| !q.is_empty()) else {
            return ToolOutput::error("query is required");
        };
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(8).clamp(1, 20) as usize;
        match (self.search)(query.to_string(), limit).await {
            Ok(text) => ToolOutput::ok(text),
            Err(message) => ToolOutput::error(message),
        }
    }
}
