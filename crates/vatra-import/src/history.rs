//! Bounded JSONL readers: only user and assistant text, never tool output or request diagnostics.
use crate::{GuardedFs, SourceDirs, map_project};
use serde::Serialize;
use serde_json::Value;
use std::{
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use vatra_core::{HistoryMessage, ImportedSessionInfo, Project, Store};

const MAX_LINE: usize = 4 * 1024 * 1024;

#[derive(Default, Debug, Serialize)]
pub struct ParsedLog {
    pub native_id: String,
    pub cwd: Option<String>,
    pub title: String,
    pub started_at: String,
    pub messages: Vec<HistoryMessage>,
    pub skipped_lines: usize,
}

pub fn parse_claude_log(reader: impl BufRead) -> ParsedLog {
    parse(reader, "claude")
}
pub fn parse_codex_log(reader: impl BufRead) -> ParsedLog {
    parse(reader, "codex")
}
fn parse(reader: impl BufRead, origin: &str) -> ParsedLog {
    let mut meta = ParsedLog::default();
    for value in JsonLines::new(reader) {
        match value {
            Ok(Some(value)) => {
                if let Some(message) = line(&value, origin, &mut meta) {
                    meta.messages.push(message);
                }
            }
            _ => meta.skipped_lines += 1,
        }
    }
    meta
}

struct JsonLines<R> {
    reader: R,
}
impl<R: BufRead> JsonLines<R> {
    fn new(reader: R) -> Self {
        Self { reader }
    }
}
impl<R: BufRead> Iterator for JsonLines<R> {
    type Item = io::Result<Option<Value>>;
    fn next(&mut self) -> Option<Self::Item> {
        let mut bytes = Vec::new();
        let mut overflow = false;
        let mut any = false;
        loop {
            let buf = match self.reader.fill_buf() {
                Ok(buf) => buf,
                Err(error) => return Some(Err(error)),
            };
            if buf.is_empty() {
                if !any {
                    return None;
                }
                break;
            }
            any = true;
            let end = buf.iter().position(|b| *b == b'\n').map(|i| i + 1);
            let used = end.unwrap_or(buf.len());
            if bytes.len() + used <= MAX_LINE && !overflow {
                bytes.extend_from_slice(&buf[..used]);
            } else {
                overflow = true;
                bytes.clear();
            }
            self.reader.consume(used);
            if end.is_some() {
                break;
            }
        }
        Some(Ok(if overflow {
            None
        } else {
            serde_json::from_slice(&bytes).ok()
        }))
    }
}

fn line(value: &Value, origin: &str, meta: &mut ParsedLog) -> Option<HistoryMessage> {
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    let at = value
        .get("timestamp")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let payload = if origin == "codex" {
        value.get("payload").unwrap_or(&Value::Null)
    } else {
        value
    };
    if origin == "codex" && kind == "session_meta" {
        meta.native_id = payload
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into();
        meta.cwd = payload
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::to_owned);
        meta.started_at = payload
            .get("timestamp")
            .and_then(Value::as_str)
            .unwrap_or(&at)
            .into();
        return None;
    }
    if origin == "claude" {
        if let Some(id) = value.get("sessionId").and_then(Value::as_str) {
            meta.native_id = id.into();
        }
        if let Some(cwd) = value.get("cwd").and_then(Value::as_str) {
            meta.cwd = Some(cwd.into());
        }
    }
    let message = if origin == "codex" {
        if kind != "response_item" || payload.get("type").and_then(Value::as_str) != Some("message")
        {
            meta.skipped_lines += 1;
            return None;
        }
        payload
    } else {
        if !["user", "assistant"].contains(&kind) {
            meta.skipped_lines += 1;
            return None;
        }
        value.get("message").unwrap_or(&Value::Null)
    };
    let role = message.get("role").and_then(Value::as_str).unwrap_or(kind);
    if !["user", "assistant"].contains(&role) {
        meta.skipped_lines += 1;
        return None;
    }
    let content = message.get("content").unwrap_or(&Value::Null);
    let text = if let Some(text) = content.as_str() {
        text.to_owned()
    } else {
        content
            .as_array()
            .into_iter()
            .flatten()
            .filter(|part| {
                matches!(
                    part.get("type").and_then(Value::as_str),
                    Some("text" | "input_text" | "output_text")
                )
            })
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    };
    if text.trim().is_empty() {
        return None;
    }
    if meta.title.is_empty() && role == "user" {
        meta.title = text.chars().take(100).collect();
    }
    if meta.started_at.is_empty() {
        meta.started_at = at.clone();
    }
    Some(HistoryMessage {
        role: role.into(),
        text,
        at,
    })
}

/// Reads metadata first without retaining messages, then streams the import within one transaction.
pub fn import_log(
    path: &Path,
    origin: &str,
    store: &Store,
    projects: &[Project],
    cancelled: &AtomicBool,
    progress: &mut dyn FnMut(usize, &str),
) -> Result<i64, String> {
    let guard = GuardedFs::new();
    let mut meta = ParsedLog::default();
    for (n, value) in
        JsonLines::new(BufReader::new(guard.open(path).map_err(|e| e.to_string())?)).enumerate()
    {
        if cancelled.load(Ordering::Relaxed) {
            return Err("History import cancelled".into());
        }
        if let Some(value) = value.map_err(|e| e.to_string())? {
            let _ = line(&value, origin, &mut meta);
        }
        if n % 1000 == 0 {
            progress(n, "Reading past conversations");
        }
    }
    if meta.native_id.is_empty() {
        meta.native_id = crate::scan::text_hash(&path.to_string_lossy());
    }
    let info = ImportedSessionInfo {
        id: 0,
        origin: origin.into(),
        native_id: meta.native_id.clone(),
        project_id: meta
            .cwd
            .as_deref()
            .and_then(|cwd| map_project(cwd, projects)),
        cwd: meta.cwd.clone(),
        title: if meta.title.is_empty() {
            "Imported conversation".into()
        } else {
            meta.title.clone()
        },
        started_at: meta.started_at.clone(),
    };
    let lines = JsonLines::new(BufReader::new(guard.open(path).map_err(|e| e.to_string())?));
    let mut metadata = ParsedLog::default();
    let messages = lines.enumerate().filter_map(|(n, value)| {
        if cancelled.load(Ordering::Relaxed) {
            return Some(Err("History import cancelled".into()));
        }
        if n % 1000 == 0 {
            progress(n, "Indexing past conversations");
        }
        match value {
            Err(error) => Some(Err(error.to_string())),
            Ok(Some(value)) => line(&value, origin, &mut metadata).map(Ok),
            Ok(None) => None,
        }
    });
    store
        .import_history(&info, messages)
        .map_err(|e| e.to_string())
}

/// A folder whose past conversations can be brought in, as the import lists it. `key` is what a choice names it by:
/// the folder in lower case with one kind of separator, or empty for conversations that carry no folder.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HistoryProject {
    pub key: String,
    pub name: String,
    pub path: Option<String>,
    pub conversations: usize,
    /// The start of the newest conversation, as the log wrote it.
    pub last_active: String,
    /// Whether Vatra already has a project for this folder.
    pub known: bool,
}

/// The key a folder is chosen by. Windows compares paths without case and with one separator.
pub fn history_key(cwd: &str) -> String {
    cwd.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// How many lines of a log are read to learn its folder and start. Both come early in a log.
const PEEK_LINES: usize = 200;

/// The folder and the start of a conversation, from the first lines of its log only.
fn peek_log(path: &Path, origin: &str) -> Option<ParsedLog> {
    let file = GuardedFs::new().open(path).ok()?;
    let mut meta = ParsedLog::default();
    for value in JsonLines::new(BufReader::new(file)).take(PEEK_LINES) {
        if let Ok(Some(value)) = value {
            let _ = line(&value, origin, &mut meta);
        }
        if meta.cwd.is_some() && !meta.started_at.is_empty() {
            break;
        }
    }
    Some(meta)
}

/// The folders that have past conversations, newest first. Each log is read only as far as its folder.
pub fn history_projects(dirs: &SourceDirs, projects: &[Project]) -> Vec<HistoryProject> {
    let mut found: Vec<HistoryProject> = Vec::new();
    for (path, origin) in history_files(dirs) {
        let Some(meta) = peek_log(&path, origin) else {
            continue;
        };
        let key = meta.cwd.as_deref().map(history_key).unwrap_or_default();
        let at = meta.started_at.clone();
        match found.iter_mut().find(|project| project.key == key) {
            Some(project) => {
                project.conversations += 1;
                if at > project.last_active {
                    project.last_active = at;
                }
            }
            None => {
                let name = meta
                    .cwd
                    .as_deref()
                    .and_then(|cwd| cwd.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next())
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_default();
                found.push(HistoryProject {
                    known: meta
                        .cwd
                        .as_deref()
                        .is_some_and(|cwd| map_project(cwd, projects).is_some()),
                    key,
                    name,
                    path: meta.cwd.clone(),
                    conversations: 1,
                    last_active: at,
                });
            }
        }
    }
    found.sort_by(|a, b| b.last_active.cmp(&a.last_active).then(a.name.cmp(&b.name)));
    found
}

/// The logs of the chosen folders, with each log's folder. A log that cannot be read is left out here and fails
/// nothing: the import of the others goes on.
pub fn history_files_for(
    dirs: &SourceDirs,
    keys: &[String],
) -> Vec<(PathBuf, &'static str, Option<String>)> {
    history_files(dirs)
        .into_iter()
        .filter_map(|(path, origin)| {
            let cwd = peek_log(&path, origin)?.cwd;
            let key = cwd.as_deref().map(history_key).unwrap_or_default();
            keys.contains(&key).then_some((path, origin, cwd))
        })
        .collect()
}

/// Enumerates filenames only. Logs are not opened unless history was explicitly selected.
pub fn history_files(dirs: &SourceDirs) -> Vec<(PathBuf, &'static str)> {
    let guard = GuardedFs::new();
    let mut result = Vec::new();
    for (root, origin) in [
        (dirs.claude.join("projects"), "claude"),
        (dirs.codex.join("sessions"), "codex"),
    ] {
        if let Ok(walk) = guard.walk(&root) {
            result.extend(
                walk.files
                    .into_iter()
                    .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
                    .map(|p| (p, origin)),
            );
        }
    }
    result
}
