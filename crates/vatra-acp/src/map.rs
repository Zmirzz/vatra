use std::collections::HashMap;

use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;
use vatra_core::{AgentError, Event};

use crate::AgentCommands;
use crate::transport::{MAX_FRAME, failed};

#[derive(Default)]
pub(crate) struct Updates {
    text: String,
    tools: HashMap<String, Value>,
    tool_bytes: usize,
    commands: AgentCommands,
    plan_seq: u64,
}

impl Updates {
    pub fn new(commands: AgentCommands) -> Self {
        Self {
            commands,
            ..Default::default()
        }
    }

    pub fn commands(&self, update: &Value) {
        if let Some(commands) = update["availableCommands"].as_array() {
            self.commands.replace(commands);
        }
    }
    pub fn permission_tool(&self, patch: &Value) -> Value {
        let mut tool = patch["toolCallId"]
            .as_str()
            .and_then(|id| self.tools.get(id))
            .cloned()
            .unwrap_or_else(|| json!({}));
        if let Some(fields) = patch.as_object() {
            for (key, value) in fields {
                if !value.is_null() {
                    tool[key] = value.clone();
                }
            }
        }
        tool
    }

    pub fn flush(&mut self, sink: &UnboundedSender<Event>) {
        if !self.text.is_empty() {
            let _ = sink.send(Event::AssistantText {
                text: std::mem::take(&mut self.text),
            });
        }
    }

    pub fn apply(
        &mut self,
        update: &Value,
        sink: &UnboundedSender<Event>,
    ) -> Result<(), AgentError> {
        match update["sessionUpdate"].as_str() {
            Some("agent_message_chunk") if update["content"]["type"] == "text" => {
                let text = update["content"]["text"]
                    .as_str()
                    .ok_or_else(|| failed("Invalid ACP text chunk."))?;
                if self.text.len() + text.len() > MAX_FRAME {
                    return Err(failed("The ACP assistant message exceeds the 1 MiB limit."));
                }
                self.text.push_str(text);
                let _ = sink.send(Event::AssistantDelta { text: text.into() });
            }
            Some("tool_call" | "tool_call_update") => {
                self.flush(sink);
                let id = update["toolCallId"]
                    .as_str()
                    .ok_or_else(|| failed("ACP tool call has no ID."))?;
                if !self.tools.contains_key(id) && self.tools.len() >= 4096 {
                    return Err(failed("The ACP agent exceeded the tool call limit."));
                }
                let old = self.tools.get(id).cloned();
                let tool = self.tools.entry(id.into()).or_insert_with(|| json!({}));
                for (key, value) in update
                    .as_object()
                    .ok_or_else(|| failed("Invalid ACP tool update."))?
                {
                    if !value.is_null() {
                        tool[key] = value.clone();
                    }
                }
                let new_size = tool.to_string().len();
                self.tool_bytes = self.tool_bytes
                    - old.as_ref().map_or(0, |old| old.to_string().len())
                    + new_size;
                if new_size > MAX_FRAME || self.tool_bytes > 16 * MAX_FRAME {
                    return Err(failed("The ACP agent exceeded the tool state size limit."));
                }
                if old.is_none()
                    || old.as_ref().map(|old| (&old["rawInput"], &old["content"]))
                        != Some((&tool["rawInput"], &tool["content"]))
                {
                    for (index, (name, arguments)) in tool_views(tool).into_iter().enumerate() {
                        let _ = sink.send(Event::ToolCall {
                            id: view_id(id, index),
                            name,
                            arguments,
                        });
                    }
                }
                let status = tool["status"].as_str().unwrap_or_default();
                if matches!(status, "completed" | "failed")
                    && old.as_ref().map(|old| &old["status"]) != Some(&tool["status"])
                {
                    for (index, _) in tool_views(tool).into_iter().enumerate() {
                        let _ = sink.send(Event::ToolResult {
                            id: view_id(id, index),
                            output: if index == 0 {
                                json!({"content":tool["content"],"rawOutput":tool["rawOutput"]})
                                    .to_string()
                            } else {
                                status.into()
                            },
                            is_error: status == "failed",
                        });
                    }
                }
            }
            Some("available_commands_update") => self.commands(update),
            Some("plan") if update["entries"].is_array() => {
                self.flush(sink);
                self.plan_seq += 1;
                let id = format!("acp-plan-{}", self.plan_seq);
                let todos: Vec<_> = update["entries"].as_array().unwrap().iter().map(|entry| json!({
                    "content":entry["content"], "status":entry["status"], "priority":entry["priority"]
                })).collect();
                let _ = sink.send(Event::ToolCall {
                    id: id.clone(),
                    name: "TodoWrite".into(),
                    arguments: json!({"todos":todos}),
                });
                let _ = sink.send(Event::ToolResult {
                    id,
                    output: "Plan updated".into(),
                    is_error: false,
                });
            }
            // These are not assistant answers. Loading history is handled by suppressing replay.
            Some("user_message_chunk" | "agent_thought_chunk") => {}
            _ => {
                let _ = sink.send(Event::Unmapped {
                    source: "acp".into(),
                    raw: update.to_string(),
                });
            }
        }
        Ok(())
    }
}

fn view_id(id: &str, index: usize) -> String {
    if index == 0 {
        id.into()
    } else {
        format!("{id}:diff:{index}")
    }
}

fn tool_views(tool: &Value) -> Vec<(String, Value)> {
    let diffs: Vec<_> = tool["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|content| content["type"] == "diff")
        .collect();
    if !diffs.is_empty() {
        return diffs.into_iter().enumerate().map(|(index, diff)| {
            let mut arguments = json!({"file_path":diff["path"], "old_string":diff["oldText"].as_str().unwrap_or_default(), "new_string":diff["newText"].as_str().unwrap_or_default()});
            if index == 0 { arguments["acp"] = tool.clone(); }
            ("Edit".into(), arguments)
        }).collect();
    }
    vec![(
        tool["name"]
            .as_str()
            .or_else(|| tool["title"].as_str())
            .unwrap_or("ACP tool")
            .into(),
        tool.clone(),
    )]
}
