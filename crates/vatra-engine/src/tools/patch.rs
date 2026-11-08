// The patch grammar comes from openai/codex, codex-rs/apply-patch (https://github.com/openai/codex),
// which is licensed under the Apache License, Version 2.0. The parser and the apply step are ours.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Tool, ToolClass, ToolContext, ToolOutput, in_memory, mark_read, resolve, split_bom, str_arg, within_project};
use crate::mcp::ToolDef;

/// The files a patch leaves pending, keyed by path. None means the file is deleted.
type Pending = BTreeMap<PathBuf, Option<String>>;

const BEGIN_PATCH: &str = "*** Begin Patch";
const END_PATCH: &str = "*** End Patch";
const ADD_FILE: &str = "*** Add File: ";
const DELETE_FILE: &str = "*** Delete File: ";
const UPDATE_FILE: &str = "*** Update File: ";
const MOVE_TO: &str = "*** Move to: ";
const END_OF_FILE: &str = "*** End of File";

#[derive(Debug, PartialEq)]
enum Hunk {
    Add { path: String, contents: String },
    Delete { path: String },
    Update { path: String, moved_to: Option<String>, chunks: Vec<Chunk> },
}

impl Hunk {
    fn paths(&self) -> Vec<&str> {
        match self {
            Hunk::Add { path, .. } | Hunk::Delete { path } => vec![path],
            Hunk::Update { path, moved_to, .. } => std::iter::once(path.as_str()).chain(moved_to.as_deref()).collect(),
        }
    }
}

/// One place to change in an update. `old` is replaced by `new`, and both start with the same context lines.
#[derive(Debug, Default, PartialEq)]
struct Chunk {
    /// A line the change comes after, usually a function or class header.
    context: Option<String>,
    old: Vec<String>,
    new: Vec<String>,
    end_of_file: bool,
}

impl Chunk {
    fn has_lines(&self) -> bool {
        !self.old.is_empty() || !self.new.is_empty()
    }
}

struct ApplyPatchTool;

#[async_trait]
impl Tool for ApplyPatchTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "apply_patch".into(),
            description: "Adds, updates, moves and deletes files with one patch. The patch starts with \
                          '*** Begin Patch' and ends with '*** End Patch'. Each file gets '*** Add File: path' \
                          (lines start with +), '*** Delete File: path', or '*** Update File: path', optionally \
                          followed by '*** Move to: path'. An update is a list of changes. Each change may start \
                          with '@@' and a line to look after, then lines that start with a space (kept), - (removed) \
                          or + (added). Nothing is written unless the whole patch applies."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "The patch text."
                    }
                },
                "required": ["input"]
            }),
        }
    }

    fn class(&self, arguments: &Value, context: &ToolContext) -> ToolClass {
        let Some(hunks) = str_arg(arguments, "input").and_then(|input| parse(input).ok()) else {
            return ToolClass::EditOutside;
        };
        let paths: Vec<_> = hunks.iter().flat_map(Hunk::paths).map(|path| resolve(path, &context.cwd)).collect();
        // Memory files are free, like the Edit and Write calls on them.
        if !paths.is_empty() && paths.iter().all(|path| in_memory(path, context)) {
            ToolClass::Free
        } else if paths.iter().all(|path| within_project(path, context)) {
            ToolClass::EditInside
        } else {
            ToolClass::EditOutside
        }
    }

    fn parallel_safe(&self) -> bool {
        false
    }

    async fn call(&self, arguments: Value, context: &ToolContext) -> ToolOutput {
        let Some(input) = str_arg(&arguments, "input") else {
            return ToolOutput::error("input is required.");
        };
        let hunks = match parse(input) {
            Ok(hunks) => hunks,
            Err(error) => return ToolOutput::error(error),
        };
        let (summary, pending) = match plan(&hunks, &context.cwd) {
            Ok(planned) => planned,
            Err(error) => return ToolOutput::error(error),
        };
        for (path, state) in &pending {
            let result = match state {
                Some(contents) => write_file(path, contents),
                None if path.is_file() => fs::remove_file(path).map_err(|error| error.to_string()),
                None => Ok(()),
            };
            if let Err(error) = result {
                return ToolOutput::error(format!(
                    "Could not change {}: {error}. Changes to files before it in this patch were already written.",
                    path.display()
                ));
            }
            if state.is_some() {
                mark_read(context, path);
            }
        }
        ToolOutput::ok(summary.join("\n"))
    }
}

fn write_file(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, contents).map_err(|error| error.to_string())
}

fn is_header(line: &str) -> bool {
    line.starts_with("*** ")
}

fn parse(input: &str) -> Result<Vec<Hunk>, String> {
    let lines: Vec<&str> = input.trim().lines().collect();
    if lines.first().map(|line| line.trim()) != Some(BEGIN_PATCH) {
        return Err(format!("The first line of the patch must be '{BEGIN_PATCH}'."));
    }
    if lines.len() < 2 || lines.last().map(|line| line.trim()) != Some(END_PATCH) {
        return Err(format!("The last line of the patch must be '{END_PATCH}'."));
    }
    let body = &lines[1..lines.len() - 1];
    let mut hunks = vec![];
    let mut index = 0;
    while index < body.len() {
        let line = body[index];
        index += 1;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(path) = line.strip_prefix(ADD_FILE) {
            let mut contents = String::new();
            while let Some(text) = body.get(index).and_then(|next| next.strip_prefix('+')) {
                contents.push_str(text);
                contents.push('\n');
                index += 1;
            }
            hunks.push(Hunk::Add { path: path.trim().into(), contents });
        } else if let Some(path) = line.strip_prefix(DELETE_FILE) {
            hunks.push(Hunk::Delete { path: path.trim().into() });
        } else if let Some(path) = line.strip_prefix(UPDATE_FILE) {
            let moved_to = body.get(index).and_then(|next| next.strip_prefix(MOVE_TO)).map(|dest| dest.trim().into());
            if moved_to.is_some() {
                index += 1;
            }
            let (chunks, next) = parse_chunks(body, index)?;
            index = next;
            if chunks.is_empty() && moved_to.is_none() {
                return Err(format!("Update File hunk for path {} has no changes.", path.trim()));
            }
            hunks.push(Hunk::Update { path: path.trim().into(), moved_to, chunks });
        } else {
            return Err(format!("Unexpected line in the patch: {line}"));
        }
    }
    Ok(hunks)
}

/// The chunks of one update, and the index of the first line after them.
fn parse_chunks(body: &[&str], mut index: usize) -> Result<(Vec<Chunk>, usize), String> {
    let mut chunks = vec![];
    let mut current: Option<Chunk> = None;
    while let Some(&line) = body.get(index) {
        if line == END_OF_FILE {
            if let Some(chunk) = current.as_mut() {
                chunk.end_of_file = true;
            }
        } else if is_header(line) {
            break;
        } else if line == "@@" || line.starts_with("@@ ") {
            if let Some(chunk) = current.take().filter(Chunk::has_lines) {
                chunks.push(chunk);
            }
            let context = line.strip_prefix("@@").map(str::trim).filter(|context| !context.is_empty());
            current = Some(Chunk { context: context.map(String::from), ..Chunk::default() });
        } else {
            let chunk = current.get_or_insert_with(Chunk::default);
            match line.chars().next() {
                Some('+') => chunk.new.push(line[1..].into()),
                Some('-') => chunk.old.push(line[1..].into()),
                Some(' ') => {
                    chunk.old.push(line[1..].into());
                    chunk.new.push(line[1..].into());
                }
                None => {
                    chunk.old.push(String::new());
                    chunk.new.push(String::new());
                }
                Some(_) => return Err(format!("Unexpected line in an update: {line}")),
            }
        }
        index += 1;
    }
    if let Some(chunk) = current.filter(Chunk::has_lines) {
        chunks.push(chunk);
    }
    Ok((chunks, index))
}

/// The file contents after every hunk, as (path, contents) with None for a deleted file, plus a line
/// per change for the report. Nothing is written here, so a patch that fails changes nothing.
fn plan(hunks: &[Hunk], cwd: &Path) -> Result<(Vec<String>, Pending), String> {
    let mut pending = Pending::new();
    let mut summary = vec![];
    for hunk in hunks {
        match hunk {
            Hunk::Add { path, contents } => {
                let file = resolve(path, cwd);
                if current(&pending, &file)?.is_some() {
                    return Err(format!("Add File: {} already exists. Update it instead.", file.display()));
                }
                summary.push(format!("Added {}", file.display()));
                pending.insert(file, Some(contents.clone()));
            }
            Hunk::Delete { path } => {
                let file = resolve(path, cwd);
                if current(&pending, &file)?.is_none() {
                    return Err(format!("Delete File: {} does not exist.", file.display()));
                }
                summary.push(format!("Deleted {}", file.display()));
                pending.insert(file, None);
            }
            Hunk::Update { path, moved_to, chunks } => {
                let file = resolve(path, cwd);
                let Some(original) = current(&pending, &file)? else {
                    return Err(format!("Update File: {} does not exist.", file.display()));
                };
                let updated = if chunks.is_empty() { original } else { apply_chunks(&original, chunks, &file)? };
                match moved_to {
                    None => {
                        summary.push(format!("Updated {}", file.display()));
                        pending.insert(file, Some(updated));
                    }
                    Some(destination) => {
                        let dest = resolve(destination, cwd);
                        if dest != file && current(&pending, &dest)?.is_some() {
                            return Err(format!("Move to: {} already exists.", dest.display()));
                        }
                        summary.push(format!("Moved {} to {}", file.display(), dest.display()));
                        pending.insert(file, None);
                        pending.insert(dest, Some(updated));
                    }
                }
            }
        }
    }
    Ok((summary, pending))
}

/// What the file holds once the patch so far is applied: the pending state if a hunk touched it, else the disk.
fn current(pending: &Pending, file: &Path) -> Result<Option<String>, String> {
    if let Some(state) = pending.get(file) {
        return Ok(state.clone());
    }
    match fs::read(file) {
        Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| format!("{} is not UTF-8 text.", file.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Could not read {}: {error}", file.display())),
    }
}

fn apply_chunks(original: &str, chunks: &[Chunk], file: &Path) -> Result<String, String> {
    let (bom, body) = split_bom(original);
    let newline = if body.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = body.lines().map(String::from).collect();
    let mut cursor = 0;
    for chunk in chunks {
        if let Some(context) = &chunk.context {
            let Some(found) = seek(&lines, std::slice::from_ref(context), cursor, false) else {
                return Err(format!("Could not find the context line `{context}` in {}.", file.display()));
            };
            cursor = found + 1;
        }
        if chunk.old.is_empty() {
            // Without context, a change with nothing to replace goes at the end of the file.
            let at = if chunk.context.is_some() { cursor } else { lines.len() };
            lines.splice(at..at, chunk.new.iter().cloned());
            cursor = at + chunk.new.len();
            continue;
        }
        let Some(found) = seek(&lines, &chunk.old, cursor, chunk.end_of_file) else {
            return Err(format!(
                "Could not find these lines in {}, so nothing was changed:\n{}",
                file.display(),
                chunk.old.join("\n")
            ));
        };
        lines.splice(found..found + chunk.old.len(), chunk.new.iter().cloned());
        cursor = found + chunk.new.len();
    }
    let mut result = lines.join(newline);
    if !lines.is_empty() {
        result.push_str(newline);
    }
    Ok(format!("{bom}{result}"))
}

/// The first place at or after `start` where `pattern` matches. Lines match exactly first, then
/// ignoring trailing spaces, then ignoring all surrounding spaces. With `at_end` only the last place counts.
fn seek(lines: &[String], pattern: &[String], start: usize, at_end: bool) -> Option<usize> {
    if pattern.len() > lines.len() {
        return None;
    }
    let last = lines.len() - pattern.len();
    let positions: Vec<usize> =
        if at_end { Some(last).filter(|&at| at >= start).into_iter().collect() } else { (start..=last).collect() };
    let find = |normalize: fn(&str) -> &str| {
        positions.iter().copied().find(|&at| {
            pattern.iter().enumerate().all(|(offset, line)| normalize(line) == normalize(&lines[at + offset]))
        })
    };
    find(identity).or_else(|| find(str::trim_end)).or_else(|| find(str::trim))
}

fn identity(text: &str) -> &str {
    text
}

pub fn apply_patch_tool() -> Arc<dyn Tool> {
    Arc::new(ApplyPatchTool)
}
