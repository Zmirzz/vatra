use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use globset::GlobBuilder;
use serde_json::{Value, json};

use super::{Tool, ToolClass, ToolContext, ToolOutput, class_of, path_arg, relative_path, str_arg, walker};
use crate::mcp::ToolDef;

const MAX_RESULTS: usize = 100;

struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "Glob".into(),
            description: "Lists files whose paths match a glob, such as **/*.rs or src/*.ts. Ignored files from \
                          .gitignore are left out. The newest files come first, and at most 100 are listed."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "The glob to match against paths relative to the folder searched."
                    },
                    "path": {
                        "type": "string",
                        "description": "The folder to search. Defaults to the working folder."
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    fn class(&self, arguments: &Value, context: &ToolContext) -> ToolClass {
        let base = path_arg(arguments, "path", context).unwrap_or_else(|| context.cwd.clone());
        class_of(Some(base), context, ToolClass::Free, ToolClass::ReadOutside)
    }

    fn parallel_safe(&self) -> bool {
        true
    }

    async fn call(&self, arguments: Value, context: &ToolContext) -> ToolOutput {
        let Some(pattern) = str_arg(&arguments, "pattern") else {
            return ToolOutput::error("pattern is required.");
        };
        let base = path_arg(&arguments, "path", context).unwrap_or_else(|| context.cwd.clone());
        if !base.exists() {
            return ToolOutput::error(format!("{} does not exist.", base.display()));
        }
        let matcher = match GlobBuilder::new(pattern).literal_separator(true).build() {
            Ok(glob) => glob.compile_matcher(),
            Err(error) => return ToolOutput::error(format!("Invalid glob {pattern}: {error}")),
        };

        let mut matches: Vec<(SystemTime, PathBuf)> = vec![];
        for entry in walker(&base).build().flatten() {
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            if !matcher.is_match(relative_path(path, &base)) {
                continue;
            }
            let modified =
                fs::metadata(path).and_then(|metadata| metadata.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            matches.push((modified, path.to_path_buf()));
        }
        if matches.is_empty() {
            return ToolOutput::ok("No files matched.");
        }
        matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

        let total = matches.len();
        let mut lines: Vec<String> =
            matches.iter().take(MAX_RESULTS).map(|(_, path)| path.display().to_string()).collect();
        if total > MAX_RESULTS {
            lines.push(format!("(showing {MAX_RESULTS} of {total} matches, most recently modified first)"));
        }
        ToolOutput::ok(lines.join("\n"))
    }
}

pub fn glob_tool() -> Arc<dyn Tool> {
    Arc::new(GlobTool)
}
