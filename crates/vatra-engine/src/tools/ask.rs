use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Tool, ToolClass, ToolContext, ToolOutput};
use crate::mcp::ToolDef;

struct AskTool;

#[async_trait]
impl Tool for AskTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "AskUserQuestion".into(),
            description: "Asks the user one or more questions and waits for the answers. Use it when the work needs \
                          a decision only the user can make. A question can offer options, each with a label and an \
                          optional description."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": {
                                    "type": "string",
                                    "description": "The question, ending in a question mark."
                                },
                                "header": {
                                    "type": "string",
                                    "description": "A short label for the question."
                                },
                                "options": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": {"type": "string"},
                                            "description": {"type": "string"}
                                        },
                                        "required": ["label"]
                                    }
                                },
                                "multiSelect": {
                                    "type": "boolean",
                                    "description": "True when the user may pick more than one option."
                                }
                            },
                            "required": ["question"]
                        }
                    }
                },
                "required": ["questions"]
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
        let Some(questions) = arguments.get("questions").and_then(Value::as_array).filter(|list| !list.is_empty())
        else {
            return ToolOutput::error("questions must be a list with at least one question.");
        };
        if questions.iter().any(|question| question.get("question").and_then(Value::as_str).is_none()) {
            return ToolOutput::error("Each question needs its text in question.");
        }
        match context.questions.ask(arguments["questions"].clone()).await {
            Some(answers) => ToolOutput::ok(format!("The user answered: {answers}")),
            None => ToolOutput::error("The user didn't answer, so the questions have no answers."),
        }
    }
}

pub fn ask_tool() -> Arc<dyn Tool> {
    Arc::new(AskTool)
}
