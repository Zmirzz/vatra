use std::fs;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Tool, ToolClass, ToolContext, ToolOutput, file_class, is_read, path_arg, split_bom, str_arg};
use crate::mcp::ToolDef;

struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "Edit".into(),
            description: "Replaces an exact piece of text in a file that has been read in this session. old_string \
                          has to match once, unless replace_all is set. Line endings and a byte order mark are kept. \
                          Keep old_string short, ideally one line or just the name to change, and copy it exactly \
                          as it is in the file, without the line numbers that Read shows. new_string replaces only old_string, so it is just the new text for that part, with no line \
                          numbers and never the whole file. To change a name in many places, use the name as \
                          old_string and set replace_all: old_string \"getUser\" and new_string \"fetchUser\"."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "The file to edit. A relative path starts from the working folder."
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The exact text to replace."
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The text that replaces only old_string, with no line numbers and never the whole file. For old_string getUser it is fetchUser."
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Replace every occurrence instead of requiring exactly one. Defaults to false."
                    }
                },
                "required": ["file_path", "old_string", "new_string"]
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
        let Some(old_string) = str_arg(&arguments, "old_string") else {
            return ToolOutput::error("old_string is required.");
        };
        let Some(new_string) = str_arg(&arguments, "new_string") else {
            return ToolOutput::error("new_string is required.");
        };
        let replace_all = arguments.get("replace_all").and_then(Value::as_bool).unwrap_or(false);
        if old_string.is_empty() {
            return ToolOutput::error("old_string must not be empty. Use Write to create a file or replace all of it.");
        }
        if old_string == new_string {
            return ToolOutput::error("old_string and new_string are the same, so there is nothing to change.");
        }
        if !file.is_file() {
            return ToolOutput::error(format!("{} does not exist or is not a file.", file.display()));
        }
        if !is_read(context, &file) {
            return ToolOutput::error(format!("Read {} before editing it.", file.display()));
        }
        let text = match fs::read(&file).map(String::from_utf8) {
            Ok(Ok(text)) => text,
            Ok(Err(_)) => return ToolOutput::error(format!("{} is not UTF-8 text.", file.display())),
            Err(error) => return ToolOutput::error(format!("Could not read {}: {error}", file.display())),
        };

        let (bom, body) = split_bom(&text);
        // A CRLF file gets CRLF in the replacement, so the arguments' LF newlines still match.
        let (old, new) = if body.contains("\r\n") {
            (to_crlf(old_string), to_crlf(new_string))
        } else {
            (old_string.to_string(), new_string.to_string())
        };
        let count = body.matches(old.as_str()).count();
        if count == 0 {
            return ToolOutput::error(format!(
                "old_string was not found in {}. Copy the text exactly as in the file, without the line numbers Read shows, \
                 and use a shorter old_string, such as one line or just the name. To change every occurrence of a name, \
                 set old_string to the name and replace_all to true.",
                file.display()
            ));
        }
        if count > 1 && !replace_all {
            return ToolOutput::error(format!(
                "old_string matches {count} places in {}. Add more of the surrounding text so it matches once, or set replace_all to true.",
                file.display()
            ));
        }
        if read_numbered(&new) && !read_numbered(&old) && !read_numbered(body) {
            return ToolOutput::error("new_string contains the line-number prefixes shown by Read. Remove those prefixes; send only the replacement text, then retry Edit.");
        }
        if whole_file_rename(body, &old, &new) {
            return ToolOutput::error("new_string looks like a whole file, but old_string is only a name. Send only the new name as new_string; use replace_all to rename every occurrence. To replace the whole file, use Write after reading it.");
        }
        let updated = format!("{bom}{}", body.replace(&old, &new));
        if let Err(error) = fs::write(&file, updated) {
            return ToolOutput::error(format!("Could not write {}: {error}", file.display()));
        }
        ToolOutput::ok(format!("Replaced {count} occurrence(s) in {}.", file.display()))
    }
}

/// Detect a renamed copy of the entire source being substituted for each occurrence of a short name.
/// Length alone is not evidence: an identifier may legitimately be replaced by a multiline expression.
fn whole_file_rename(body: &str, old: &str, new: &str) -> bool {
    let identifier = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    if !old.chars().all(identifier) || old.trim() == body.trim() { return false; }
    let Some((prefix, _)) = body.split_once(old) else { return false };
    let Some(rest) = new.strip_prefix(prefix) else { return false };
    let candidate: String = rest.chars().take_while(|c| identifier(*c)).collect();
    !candidate.is_empty() && candidate != old && body.replace(old, &candidate) == new
}

/// Read prints a six-column line number, followed by a tab. Small models sometimes drop the tab.
/// Require consecutive numbers on at least two lines to avoid mistaking ordinary numeric text for this display.
fn read_numbered(text: &str) -> bool {
    let mut expected = None;
    let mut count = 0;
    for line in text.lines().filter(|line| !line.is_empty()) {
        let Some(prefix) = line.get(..6) else { return false };
        if !prefix.starts_with(' ') { return false; }
        let Ok(number) = prefix.trim().parse::<usize>() else { return false };
        if number == 0 || expected.is_some_and(|next| number != next) { return false; }
        expected = number.checked_add(1);
        count += 1;
    }
    count >= 2
}

fn to_crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

pub fn edit_tool() -> Arc<dyn Tool> {
    Arc::new(EditTool)
}
