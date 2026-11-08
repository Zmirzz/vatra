use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Tool, ToolClass, ToolContext, ToolOutput, str_arg};
use crate::mcp::ToolDef;
use crate::skills::{EngineExtensions, invoke};

const DESCRIPTION: &str = "Runs one of the skills or commands listed under Skills, and returns its instructions. \
Follow the instructions for the task. Pass the text after the name in args, when the skill takes arguments.";

struct SkillTool {
    extensions: Arc<EngineExtensions>,
}

/// The Skill tool: the model runs a skill or command from the listing, and gets its instructions back.
pub fn skill_tool(extensions: Arc<EngineExtensions>) -> Arc<dyn Tool> {
    Arc::new(SkillTool { extensions })
}

#[async_trait]
impl Tool for SkillTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "Skill".into(),
            description: DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "skill": {
                        "type": "string",
                        "description": "The name of the skill or command, as listed."
                    },
                    "args": {
                        "type": "string",
                        "description": "The arguments the skill takes, as one text."
                    }
                },
                "required": ["skill"]
            }),
        }
    }

    fn class(&self, _arguments: &Value, _context: &ToolContext) -> ToolClass {
        ToolClass::Free
    }

    fn parallel_safe(&self) -> bool {
        false
    }

    async fn call(&self, arguments: Value, context: &ToolContext) -> ToolOutput {
        self.call_with_id("", arguments, context).await
    }

    async fn call_with_id(&self, call_id: &str, arguments: Value, context: &ToolContext) -> ToolOutput {
        let Some(name) = str_arg(&arguments, "skill") else {
            return ToolOutput::error("skill is required.");
        };
        let Some(skill) = self.extensions.find(name) else {
            return ToolOutput::error(format!("There's no skill or command named `{name}` in your list."));
        };
        if !skill.allow_implicit {
            return ToolOutput::error(format!(
                "The skill `{name}` can only be run by the person, not by the model."
            ));
        }
        let args = str_arg(&arguments, "args").unwrap_or_default();
        // The model's call never grants the skill's tools: only the person's `/` or `$` does.
        match invoke(&self.extensions, skill, args, context, false, call_id).await {
            Ok(text) => ToolOutput::ok(text),
            Err(message) => ToolOutput::error(message),
        }
    }
}
