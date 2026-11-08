use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{StopOutcome, Tool, ToolClass, ToolContext, ToolOutput, str_arg};
use crate::mcp::ToolDef;

struct TaskStopTool;

#[async_trait]
impl Tool for TaskStopTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "TaskStop".into(),
            description:
                "Stops a background command that Bash or PowerShell started with run_in_background, along with \
                          everything it started. A stopped command's result is not reported."
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "The task id that starting the background command returned."
                    }
                },
                "required": ["task_id"]
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
        let Some(id) = str_arg(&arguments, "task_id") else {
            return ToolOutput::error("task_id is required.");
        };
        match context.background.stop(id) {
            StopOutcome::Stopped => ToolOutput::ok(format!("Stopped {id}.")),
            StopOutcome::Finished => ToolOutput::ok(format!("{id} had already finished.")),
            StopOutcome::Unknown => ToolOutput::error(format!("No background command has the id {id}.")),
        }
    }
}

pub fn task_stop_tool() -> Arc<dyn Tool> {
    Arc::new(TaskStopTool)
}
