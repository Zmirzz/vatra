//! `remember` and `forget`: how a model adds to or trims the list of facts about the person that every agent shares.
use super::{Tool, ToolClass, ToolContext, ToolOutput};
use crate::ToolDef;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use vatra_core::Store;

/// The two tools, named for the agent that uses them so the list shows who saved each fact.
pub fn user_memory_tools(store: Arc<Store>, agent: String) -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(Remember { store: Arc::clone(&store), agent }), Arc::new(Forget { store })]
}

struct Remember {
    store: Arc<Store>,
    agent: String,
}
struct Forget {
    store: Arc<Store>,
}

#[async_trait]
impl Tool for Remember {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "remember".into(),
            description: "Save one lasting fact about the person (who they are, what they like or dislike, how they want answers) so every assistant knows it later. One short fact per call, in their words. Never save secrets.".into(),
            parameters: json!({"type":"object","properties":{
                "text":{"type":"string","description":"The fact, one sentence"},
                "category":{"type":"string","enum":vatra_core::MEMORY_CATEGORIES}
            },"required":["text"]}),
        }
    }
    fn class(&self, _: &Value, _: &ToolContext) -> ToolClass {
        ToolClass::Free
    }
    fn parallel_safe(&self) -> bool {
        false
    }
    async fn call(&self, args: Value, _: &ToolContext) -> ToolOutput {
        let Some(text) = args.get("text").and_then(Value::as_str) else {
            return ToolOutput::error("text is required");
        };
        let (message, failed) = self.store.agent_remember(text, args.get("category").and_then(Value::as_str), &self.agent);
        if failed { ToolOutput::error(message) } else { ToolOutput::ok(message) }
    }
}

#[async_trait]
impl Tool for Forget {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "forget".into(),
            description: "Remove a saved fact about the person when they ask you to. Give enough of its words to match exactly one fact.".into(),
            parameters: json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}),
        }
    }
    fn class(&self, _: &Value, _: &ToolContext) -> ToolClass {
        ToolClass::Free
    }
    fn parallel_safe(&self) -> bool {
        false
    }
    async fn call(&self, args: Value, _: &ToolContext) -> ToolOutput {
        let Some(text) = args.get("text").and_then(Value::as_str) else {
            return ToolOutput::error("text is required");
        };
        let (message, failed) = self.store.agent_forget(text);
        if failed { ToolOutput::error(message) } else { ToolOutput::ok(message) }
    }
}
