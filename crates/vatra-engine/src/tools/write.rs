use std::fs;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Tool, ToolClass, ToolContext, ToolOutput, file_class, is_read, mark_read, path_arg, str_arg};
use crate::mcp::ToolDef;

struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "Write".into(),
            description: "Creates a file, or replaces the whole contents of one, and creates missing parent folders. \
                          An existing file has to be read first in this session, so its contents are known before \
                          they are replaced. To change part of a file, use Edit."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "The file to write. A relative path starts from the working folder."
                    },
                    "content": {
                        "type": "string",
                        "description": "The complete new contents of the file."
                    }
                },
                "required": ["file_path", "content"]
            }),
        }
    }

    fn class(&self, arguments: &Value, context: &ToolContext) -> ToolClass {
        file_class(path_arg(arguments, "file_path", context), context, ToolClass::EditInside, ToolClass::EditOutside)
    }

    fn parallel_safe(&self) -> bool {
        false
    }

    async fn call(&self, arguments: Value, context: &ToolContext) -> ToolOutput {
        let Some(file) = path_arg(&arguments, "file_path", context) else {
            return ToolOutput::error("file_path is required.");
        };
        let Some(content) = str_arg(&arguments, "content") else {
            return ToolOutput::error("content is required.");
        };
        if file.is_dir() {
            return ToolOutput::error(format!("{} is a directory, so it can't be written as a file.", file.display()));
        }
        let exists = file.exists();
        if exists && !is_read(context, &file) {
            return ToolOutput::error(format!(
                "{} already exists and hasn't been read in this session. Read it first, then write it again.",
                file.display()
            ));
        }
        if let Some(parent) = file.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            return ToolOutput::error(format!("Could not create {}: {error}", parent.display()));
        }
        if let Err(error) = fs::write(&file, content) {
            return ToolOutput::error(format!("Could not write {}: {error}", file.display()));
        }
        mark_read(context, &file);
        let verb = if exists { "Overwrote" } else { "Created" };
        ToolOutput::ok(format!("{verb} {}", file.display()))
    }
}

pub fn write_tool() -> Arc<dyn Tool> {
    Arc::new(WriteTool)
}
