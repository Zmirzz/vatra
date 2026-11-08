use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Tool, ToolClass, ToolContext, ToolOutput};
use crate::mcp::ToolDef;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Pending,
    InProgress,
    Completed,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Todo {
    content: String,
    status: Status,
    active_form: String,
}

struct TodoTool;

#[async_trait]
impl Tool for TodoTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "TodoWrite".into(),
            description: "Writes the task list for the current work. Call it first for a task with three or more steps, \
                          then again as each step finishes. Send the whole list each time. Each item has \
                          its content, a status (pending, in_progress or completed) and an activeForm, the present \
                          tense shown while the item is in progress."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {
                                    "type": "string",
                                    "description": "The task, in the imperative."
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"]
                                },
                                "activeForm": {
                                    "type": "string",
                                    "description": "The task in the present continuous, such as Running the tests."
                                }
                            },
                            "required": ["content", "status", "activeForm"]
                        }
                    }
                },
                "required": ["todos"]
            }),
        }
    }

    fn class(&self, _arguments: &Value, _context: &ToolContext) -> ToolClass {
        ToolClass::Free
    }

    fn parallel_safe(&self) -> bool {
        true
    }

    async fn call(&self, arguments: Value, _context: &ToolContext) -> ToolOutput {
        let todos: Vec<Todo> = match arguments.get("todos").map(|todos| serde_json::from_value(todos.clone())) {
            Some(Ok(todos)) => todos,
            Some(Err(error)) => return ToolOutput::error(format!("The todos aren't valid: {error}.")),
            None => return ToolOutput::error("todos is required."),
        };
        let mut text = String::from("Todo list updated:");
        for todo in &todos {
            let (status, label) = match todo.status {
                Status::Pending => ("pending", &todo.content),
                Status::InProgress => ("in_progress", &todo.active_form),
                Status::Completed => ("completed", &todo.content),
            };
            text.push_str(&format!("\n[{status}] {label}"));
        }
        ToolOutput::ok(text)
    }
}

pub fn todo_tool() -> Arc<dyn Tool> {
    Arc::new(TodoTool)
}
