use std::fs;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use super::{Tool, ToolClass, ToolContext, ToolOutput, file_class, mark_read, path_arg, split_bom, within_folder};
use crate::mcp::ToolDef;
use crate::model::ImagePart;

const DEFAULT_LIMIT: u64 = 2000;
/// A NUL byte in the first part of a file means it isn't text.
const BINARY_CHECK_BYTES: usize = 8192;

struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "Read".into(),
            description: "Reads a file and returns its lines, each prefixed with its line number. Use offset and \
                          limit to read part of a long file. Image files (PNG, JPEG, GIF, WebP) come back as images. \
                          Binary files can't be read. A file has to be read before it can be edited or overwritten."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "The file to read. A relative path starts from the working folder."
                    },
                    "offset": {
                        "type": "integer",
                        "description": "The line to start from, counting from 1."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "How many lines to return. Defaults to 2000."
                    }
                },
                "required": ["file_path"]
            }),
        }
    }

    fn class(&self, arguments: &Value, context: &ToolContext) -> ToolClass {
        let path = path_arg(arguments, "file_path", context);
        // A file that the session saved, such as a long output, is read without approval.
        match &path {
            Some(path) if within_folder(path, &context.session_dir) => ToolClass::Free,
            _ => file_class(path, context, ToolClass::Free, ToolClass::ReadOutside),
        }
    }

    fn parallel_safe(&self) -> bool {
        true
    }

    async fn call(&self, arguments: Value, context: &ToolContext) -> ToolOutput {
        let Some(file) = path_arg(&arguments, "file_path", context) else {
            return ToolOutput::error("file_path is required.");
        };
        if file.is_dir() {
            return ToolOutput::error(format!(
                "{} is a directory, not a file. Use Glob or Grep to look inside it.",
                file.display()
            ));
        }
        let bytes = match fs::read(&file) {
            Ok(bytes) => bytes,
            Err(error) => return ToolOutput::error(format!("Could not read {}: {error}", file.display())),
        };
        if let Some(media_type) = image_type(&file) {
            mark_read(context, &file);
            let mut output = ToolOutput::ok(format!("Image {}", file.display()));
            output.images.push(ImagePart { media_type: media_type.into(), base64: STANDARD.encode(&bytes) });
            return output;
        }
        if bytes[..bytes.len().min(BINARY_CHECK_BYTES)].contains(&0) {
            return ToolOutput::error(format!("{} is a binary file, so it can't be read as text.", file.display()));
        }
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => return ToolOutput::error(format!("{} is not UTF-8 text.", file.display())),
        };
        mark_read(context, &file);

        let (_, body) = split_bom(&text);
        let lines: Vec<&str> = body.lines().collect();
        let total = lines.len();
        if total == 0 {
            return ToolOutput::ok("(the file is empty)");
        }
        let offset = arguments.get("offset").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
        if offset > total {
            return ToolOutput::error(format!(
                "Offset {offset} is past the end of {}, which has {total} lines.",
                file.display()
            ));
        }
        let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(DEFAULT_LIMIT) as usize;
        let start = offset - 1;
        let end = start.saturating_add(limit).min(total);

        let mut output = lines[start..end]
            .iter()
            .enumerate()
            .map(|(index, line)| format!("{:>6}\t{line}", start + index + 1))
            .collect::<Vec<_>>()
            .join("\n");
        if start > 0 || end < total {
            output.push_str(&format!("\n\n(showing lines {}-{} of {total}; use offset to read more)", start + 1, end));
        }
        ToolOutput::ok(output)
    }
}

fn image_type(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

pub fn read_tool() -> Arc<dyn Tool> {
    Arc::new(ReadTool)
}
