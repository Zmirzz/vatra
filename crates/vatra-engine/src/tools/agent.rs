use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Tool, ToolClass, ToolContext, ToolOutput, str_arg};
use crate::mcp::ToolDef;
use crate::subagents::{GENERAL_PURPOSE, SubagentRequest, SubagentRunner};

const DESCRIPTION: &str = "Starts a subagent on a task, with its own conversation. It doesn't see this conversation, \
so the prompt must hold everything it needs. Its steps show under this call, and the tool returns its final answer. \
general-purpose has every tool. Explore reads and searches, and can't change files or run commands. A subagent can't \
start another subagent or ask the person a question. Several calls to this tool in one answer run together, a few at \
a time.";

struct AgentTool {
    runner: Arc<SubagentRunner>,
}

/// The Agent tool: the model starts a subagent on a task, and gets the subagent's final answer back.
pub fn agent_tool(runner: Arc<SubagentRunner>) -> Arc<dyn Tool> {
    Arc::new(AgentTool { runner })
}

#[async_trait]
impl Tool for AgentTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "Agent".into(),
            description: DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The task in full. The subagent starts with this text only."
                    },
                    "description": {
                        "type": "string",
                        "description": "A short name for the task, shown while it runs."
                    },
                    "subagent_type": {
                        "type": "string",
                        "description": "general-purpose (the default), or Explore, which only reads and searches."
                    },
                    "model": {
                        "type": "string",
                        "description": "Not used: a subagent runs on the model of the turn."
                    }
                },
                "required": ["prompt"]
            }),
        }
    }

    fn class(&self, _arguments: &Value, _context: &ToolContext) -> ToolClass {
        ToolClass::Free
    }

    fn parallel_safe(&self) -> bool {
        true
    }

    async fn call(&self, arguments: Value, context: &ToolContext) -> ToolOutput {
        // Without a call id, the subagent's steps nest under an empty id. The engine always gives the id.
        self.call_with_id("", arguments, context).await
    }

    async fn call_with_id(&self, call_id: &str, arguments: Value, context: &ToolContext) -> ToolOutput {
        let Some(prompt) = str_arg(&arguments, "prompt").filter(|prompt| !prompt.trim().is_empty()) else {
            return ToolOutput::error("prompt is required.");
        };
        let request = SubagentRequest {
            prompt: prompt.into(),
            description: str_arg(&arguments, "description").unwrap_or_default().into(),
            agent_type: str_arg(&arguments, "subagent_type").unwrap_or(GENERAL_PURPOSE).into(),
            model: str_arg(&arguments, "model").map(String::from),
            context: context.clone(),
        };
        self.runner.run(request, call_id).await
    }
}
