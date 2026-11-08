use std::fs;
use std::sync::Arc;

use async_trait::async_trait;
use globset::{GlobBuilder, GlobMatcher};
use ignore::types::TypesBuilder;
use regex::{Regex, RegexBuilder};
use serde_json::{Value, json};

use super::{Tool, ToolClass, ToolContext, ToolOutput, class_of, path_arg, relative_path, str_arg, walker};
use crate::mcp::ToolDef;

const DEFAULT_HEAD_LIMIT: usize = 250;
const BINARY_CHECK_BYTES: usize = 8192;

struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "Grep".into(),
            description: "Searches file contents with a regular expression. output_mode picks what comes back: \
                          files_with_matches (the default) lists file paths, content lists matching lines with line \
                          numbers and can add context lines with -A, -B or -C, and count gives the number of matching \
                          lines per file. Ignored files from .gitignore and binary files are skipped. Use multiline \
                          for patterns that span lines."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "The regular expression to search for."
                    },
                    "path": {
                        "type": "string",
                        "description": "A file or folder to search. Defaults to the working folder."
                    },
                    "glob": {
                        "type": "string",
                        "description": "Only search files whose path matches this glob, such as *.ts."
                    },
                    "type": {
                        "type": "string",
                        "description": "Only search files of this type, such as rust, js or py."
                    },
                    "output_mode": {
                        "type": "string",
                        "enum": ["files_with_matches", "content", "count"],
                        "description": "What to return. Defaults to files_with_matches."
                    },
                    "-i": {
                        "type": "boolean",
                        "description": "Ignore case when matching."
                    },
                    "-n": {
                        "type": "boolean",
                        "description": "Show line numbers in content mode. Defaults to true."
                    },
                    "-A": {
                        "type": "integer",
                        "description": "Lines of context to show after each match in content mode."
                    },
                    "-B": {
                        "type": "integer",
                        "description": "Lines of context to show before each match in content mode."
                    },
                    "-C": {
                        "type": "integer",
                        "description": "Lines of context to show on both sides of each match in content mode."
                    },
                    "context": {
                        "type": "integer",
                        "description": "The same as -C. -A and -B override it for their side."
                    },
                    "multiline": {
                        "type": "boolean",
                        "description": "Let . match newlines and let a pattern span several lines."
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Skip this many results before listing."
                    },
                    "head_limit": {
                        "type": "integer",
                        "description": "Return at most this many results. Defaults to 250. Zero means no limit."
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
        let mode = str_arg(&arguments, "output_mode").unwrap_or("files_with_matches");
        if !matches!(mode, "files_with_matches" | "content" | "count") {
            return ToolOutput::error("output_mode must be files_with_matches, content or count.");
        }
        let multiline = arguments.get("multiline").and_then(Value::as_bool).unwrap_or(false);
        let regex = match RegexBuilder::new(pattern)
            .case_insensitive(arguments.get("-i").and_then(Value::as_bool).unwrap_or(false))
            .multi_line(multiline)
            .dot_matches_new_line(multiline)
            .build()
        {
            Ok(regex) => regex,
            Err(error) => return ToolOutput::error(format!("Invalid regular expression: {error}")),
        };
        let glob = match str_arg(&arguments, "glob").map(glob_matcher).transpose() {
            Ok(glob) => glob,
            Err(error) => return ToolOutput::error(error),
        };

        let mut search = walker(&base);
        if let Some(name) = str_arg(&arguments, "type") {
            let mut types = TypesBuilder::new();
            types.add_defaults().select(name);
            match types.build() {
                Ok(types) => {
                    search.types(types);
                }
                Err(error) => return ToolOutput::error(format!("Unknown file type {name}: {error}")),
            }
        }

        let count_arg = |name: &str| arguments.get(name).and_then(Value::as_u64).map(|count| count as usize);
        let around = count_arg("-C").or(count_arg("context")).unwrap_or(0);
        let before = count_arg("-B").unwrap_or(around);
        let after = count_arg("-A").unwrap_or(around);
        let numbers = arguments.get("-n").and_then(Value::as_bool).unwrap_or(true);

        let mut entries: Vec<String> = vec![];
        for entry in search.build().flatten() {
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            if glob.as_ref().is_some_and(|glob| !glob.is_match(relative_path(path, &base))) {
                continue;
            }
            let Ok(bytes) = fs::read(path) else { continue };
            if bytes[..bytes.len().min(BINARY_CHECK_BYTES)].contains(&0) {
                continue;
            }
            let Ok(text) = String::from_utf8(bytes) else { continue };

            let shown = path.display().to_string();
            let spans = find_spans(&regex, &text, multiline);
            match mode {
                "content" => entries.extend(content_entries(&shown, &text, &spans, before, after, numbers)),
                "count" if !spans.is_empty() => entries.push(format!("{shown}:{}", spans.len())),
                "files_with_matches" if !spans.is_empty() => entries.push(shown),
                _ => {}
            }
        }
        if entries.is_empty() {
            return ToolOutput::ok("No matches found.");
        }

        let total = entries.len();
        let offset = arguments.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let head_limit = match arguments.get("head_limit").and_then(Value::as_u64) {
            Some(0) => usize::MAX,
            Some(limit) => limit as usize,
            None => DEFAULT_HEAD_LIMIT,
        };
        let page: Vec<&String> = entries.iter().skip(offset).take(head_limit).collect();
        if page.is_empty() {
            return ToolOutput::ok(format!("No results at offset {offset}; there are {total} in all."));
        }
        let mut output = page.iter().map(|entry| entry.as_str()).collect::<Vec<_>>().join("\n");
        let shown_to = offset + page.len();
        if offset > 0 || shown_to < total {
            output.push_str(&format!(
                "\n\n(showing results {}-{shown_to} of {total}; use offset to see more)",
                offset + 1
            ));
        }
        ToolOutput::ok(output)
    }
}

/// A glob without a folder part matches file names at any depth, as it does in ripgrep.
fn glob_matcher(glob: &str) -> Result<GlobMatcher, String> {
    let pattern = if glob.contains('/') { glob.to_string() } else { format!("**/{glob}") };
    GlobBuilder::new(&pattern)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|error| format!("Invalid glob {glob}: {error}"))
}

/// The lines each match covers, as zero-based inclusive (first, last) pairs. Without multiline each
/// matching line is one span. With multiline a span runs from the line a match starts on to the line it ends on.
fn find_spans(regex: &Regex, text: &str, multiline: bool) -> Vec<(usize, usize)> {
    if multiline {
        regex
            .find_iter(text)
            .map(|found| {
                let first = text[..found.start()].matches('\n').count();
                let last = first + found.as_str().trim_end_matches('\n').matches('\n').count();
                (first, last)
            })
            .collect()
    } else {
        text.lines().enumerate().filter(|(_, line)| regex.is_match(line)).map(|(index, _)| (index, index)).collect()
    }
}

/// The matching lines of a file with their context lines, formatted as ripgrep does. A matching line is
/// written as path:line:text and a context line as path-line-text. With context, a -- line separates groups
/// that don't touch. Without -n the line number is left out.
fn content_entries(
    shown: &str,
    text: &str,
    spans: &[(usize, usize)],
    before: usize,
    after: usize,
    numbers: bool,
) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut matching = vec![false; lines.len()];
    for &(first, last) in spans {
        for flag in matching.iter_mut().skip(first).take(last + 1 - first) {
            *flag = true;
        }
    }
    let mut selected = vec![false; lines.len()];
    for index in (0..lines.len()).filter(|&index| matching[index]) {
        let from = index.saturating_sub(before);
        let to = (index + after).min(lines.len() - 1);
        for flag in selected.iter_mut().take(to + 1).skip(from) {
            *flag = true;
        }
    }

    let separate = before > 0 || after > 0;
    let mut entries = vec![];
    let mut previous: Option<usize> = None;
    for index in (0..lines.len()).filter(|&index| selected[index]) {
        if separate && previous.is_some_and(|last| index > last + 1) {
            entries.push("--".to_string());
        }
        previous = Some(index);
        let sep = if matching[index] { ':' } else { '-' };
        let line = lines[index];
        entries.push(if numbers {
            format!("{shown}{sep}{}{sep}{line}", index + 1)
        } else {
            format!("{shown}{sep}{line}")
        });
    }
    entries
}

pub fn grep_tool() -> Arc<dyn Tool> {
    Arc::new(GrepTool)
}
