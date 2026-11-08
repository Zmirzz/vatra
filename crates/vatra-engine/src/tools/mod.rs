use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use ignore::WalkBuilder;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use vatra_core::{Event, ProcessGroup};

use crate::mcp::ToolDef;
use crate::model::ImagePart;

pub use crate::permissions::ToolClass;

mod agent;
mod ask;
mod bash;
mod edit;
mod glob;
mod grep;
mod patch;
mod read;
mod search_history;
mod search_workspace;
mod skill;
mod task_stop;
mod todo;
mod tool_search;
mod search_chain;
mod semantic_search;
mod web_fetch;
mod web_search;
mod write;
mod workspace;
pub use workspace::{workspace_tools, call_workspace_tool};

pub use agent::agent_tool;
pub use ask::ask_tool;
pub use bash::{Background, Finished, StopOutcome, bash_tool, bash_tool_with, powershell_tool};
pub(crate) use bash::{Ran, ShellError, bash_command, run_shell};
pub use edit::edit_tool;
pub use glob::glob_tool;
pub use grep::grep_tool;
pub use patch::apply_patch_tool;
pub use read::read_tool;
pub use search_history::search_history_tool;
pub use search_workspace::{search_workspace_tool, scoped_search_workspace_tool};
pub use skill::skill_tool;
pub use task_stop::task_stop_tool;
pub use todo::todo_tool;
pub(crate) use tool_search::{NAME as TOOL_SEARCH, capped};
pub use tool_search::{deferred_split, tool_search_tool};
pub use semantic_search::{SemanticSearchFn, semantic_search_tool};
pub use search_chain::{Endpoints, Provider, SearchChain, SearchOutcome};
pub use web_fetch::{caption_url, parse_transcript, Fetcher, HeadlessBrowser, PageRenderer, web_fetch_tool, web_fetch_tool_allowing_loopback};
pub use web_search::{SearchClient, SearchHit, WebSearchConfig, chain_search, chain_search_tool, web_search_tool, web_search_tool_with_endpoint};
pub use write::write_tool;

#[async_trait]
pub trait Tool: Send + Sync {
    fn def(&self) -> ToolDef;
    fn class(&self, arguments: &Value, context: &ToolContext) -> ToolClass;
    fn parallel_safe(&self) -> bool;
    async fn call(&self, arguments: Value, context: &ToolContext) -> ToolOutput;
    /// Runs a call, given the id the model gave it. Only the tools that report under their call, such as Agent,
    /// need it.
    async fn call_with_id(
        &self,
        _call_id: &str,
        arguments: Value,
        context: &ToolContext,
    ) -> ToolOutput {
        self.call(arguments, context).await
    }
    /// The deferred tools that a successful call brings into the tool list for the following steps.
    /// Only ToolSearch loads tools.
    fn loads(&self, _arguments: &Value) -> Vec<String> {
        vec![]
    }
    /// What an external action is, for the audit log and the question to the person. Only the calls of
    /// `ToolClass::External` need it.
    fn external(&self, _arguments: &Value) -> Option<ExternalAction> {
        None
    }
}

/// An action with an effect outside the computer. The audit log keeps the connector and the summary, never the body.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalAction {
    /// The service the action reaches, such as `mail`.
    pub connector: String,
    /// A short description of the action. It must not carry message text.
    pub summary: String,
}

#[async_trait]
pub trait Questioner: Send + Sync {
    async fn ask(&self, questions: Value) -> Option<Value>;
}

/// For turns where nobody can answer a question.
pub struct NoQuestions;

#[async_trait]
impl Questioner for NoQuestions {
    async fn ask(&self, _questions: Value) -> Option<Value> {
        None
    }
}

#[derive(Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub project_root: PathBuf,
    pub session_dir: PathBuf,
    pub read_files: Arc<Mutex<HashSet<PathBuf>>>,
    pub events: UnboundedSender<Event>,
    pub questions: Arc<dyn Questioner>,
    /// The processes the turn's foreground commands started. Ending the turn ends them.
    pub tree: Arc<ProcessGroup>,
    /// The background commands of the session.
    pub background: Arc<Background>,
    /// Variables removed from every command, such as the API keys of configured agents.
    pub hidden_env: Vec<String>,
    /// The agent's memory folder. Read and Write calls inside it need no approval.
    pub memory_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
    pub images: Vec<ImagePart>,
}

impl ToolOutput {
    pub fn ok(text: impl Into<String>) -> ToolOutput {
        ToolOutput {
            text: text.into(),
            is_error: false,
            images: vec![],
        }
    }

    pub fn error(text: impl Into<String>) -> ToolOutput {
        ToolOutput {
            text: text.into(),
            is_error: true,
            images: vec![],
        }
    }
}

/// Text longer than this is saved to a file, and the reply keeps only its start.
pub(crate) const SAVE_OVER_CHARS: usize = 50_000;
/// How much of a saved text the reply keeps.
pub(crate) const PREVIEW_CHARS: usize = 10_000;

/// Saves a long text to a file in the session folder, and returns its start with the file's path. Without a
/// session folder the text goes to the temp folder, never to the working folder.
pub(crate) fn save_long(text: &str, session_dir: &Path, label: &str) -> String {
    let total = text.chars().count();
    let preview: String = text.chars().take(PREVIEW_CHARS).collect();
    match save(text, session_dir, label) {
        Ok(path) => format!(
            "{preview}\n\n(The output is {total} characters long, and only its start is shown. The full output is saved \
             at {}. Read that file to see more.)",
            path.display()
        ),
        Err(error) => format!(
            "{preview}\n\n(The output is {total} characters long, and only its start is shown. It could not be saved: \
             {error}.)"
        ),
    }
}

static NEXT_OUTPUT: AtomicU64 = AtomicU64::new(1);

fn save(text: &str, session_dir: &Path, label: &str) -> io::Result<PathBuf> {
    let folder = if session_dir.as_os_str().is_empty() {
        std::env::temp_dir()
    } else {
        session_dir.to_path_buf()
    };
    fs::create_dir_all(&folder)?;
    let number = NEXT_OUTPUT.fetch_add(1, Ordering::SeqCst);
    let path = folder.join(format!("{label}-{}-{number}.txt", std::process::id()));
    fs::write(&path, text)?;
    Ok(path)
}

/// Tools in the order they were registered, so the model always sees the same list.
#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
    aliases: HashMap<String, String>,
}

impl ToolRegistry {
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Makes `from` resolve to the tool registered as `to`.
    pub fn alias(&mut self, from: &str, to: &str) {
        self.aliases.insert(from.into(), to.into());
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        let name = self.aliases.get(name).map_or(name, String::as_str);
        self.tools
            .iter()
            .find(|tool| tool.def().name == name)
            .cloned()
    }

    pub fn defs(&self) -> Vec<ToolDef> {
        self.tools.iter().map(|tool| tool.def()).collect()
    }

    /// A registry with the tools whose names `keep` accepts, and the aliases that lead to them.
    pub fn filtered(&self, keep: impl Fn(&str) -> bool) -> ToolRegistry {
        let tools = self
            .tools
            .iter()
            .filter(|tool| keep(tool.def().name.as_str()))
            .cloned()
            .collect();
        let aliases = self
            .aliases
            .iter()
            .filter(|(_, to)| keep(to.as_str()))
            .map(|(from, to)| (from.clone(), to.clone()))
            .collect();
        ToolRegistry { tools, aliases }
    }
}

fn str_arg<'a>(arguments: &'a Value, name: &str) -> Option<&'a str> {
    arguments.get(name).and_then(Value::as_str)
}

/// Joins a path argument onto the working folder and resolves `.` and `..` without touching the disk,
/// so `src/../../x` can't pass as inside the project.
fn resolve(path: &str, cwd: &Path) -> PathBuf {
    let joined = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        cwd.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match resolved.components().next_back() {
                Some(Component::Normal(_)) => {
                    resolved.pop();
                }
                // Above a root there is nothing to go up to.
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => resolved.push(".."),
            },
            other => resolved.push(other),
        }
    }
    resolved
}

/// Picks the class of a call from the path it touches. A call without a path counts as outside,
/// since nothing shows it stays in the project.
fn class_of(
    path: Option<PathBuf>,
    context: &ToolContext,
    inside: ToolClass,
    outside: ToolClass,
) -> ToolClass {
    match path {
        Some(path) if within_project(&path, context) => inside,
        _ => outside,
    }
}

/// Like `class_of` for the calls that read or write one file. A file in the memory folder is always free.
fn file_class(
    path: Option<PathBuf>,
    context: &ToolContext,
    inside: ToolClass,
    outside: ToolClass,
) -> ToolClass {
    match &path {
        Some(path) if in_memory(path, context) => ToolClass::Free,
        _ => class_of(path, context, inside, outside),
    }
}

fn in_memory(path: &Path, context: &ToolContext) -> bool {
    context
        .memory_dir
        .as_deref()
        .is_some_and(|memory| within_folder(path, memory))
}

/// Whether a resolved path is inside the project once symlinks and junctions are followed.
fn within_project(path: &Path, context: &ToolContext) -> bool {
    within_folder(path, &context.project_root)
}

/// Whether a resolved path is inside a folder once symlinks and junctions are followed. The deepest
/// ancestor that exists is canonicalized and the rest of the path is put back on it. A path that can't be
/// canonicalized, such as a dangling link, counts as outside.
fn within_folder(path: &Path, folder: &Path) -> bool {
    let Ok(root) = fs::canonicalize(folder) else {
        return false;
    };
    let mut existing = path;
    let mut rest = PathBuf::new();
    loop {
        match fs::canonicalize(existing) {
            Ok(real) => return real.join(&rest).starts_with(&root),
            Err(_) if fs::symlink_metadata(existing).is_ok() => return false,
            Err(_) => match (existing.parent(), existing.file_name()) {
                (Some(parent), Some(name)) => {
                    rest = Path::new(name).join(&rest);
                    existing = parent;
                }
                _ => return false,
            },
        }
    }
}

/// The path a call names, resolved against the working folder.
fn path_arg(arguments: &Value, name: &str, context: &ToolContext) -> Option<PathBuf> {
    str_arg(arguments, name).map(|path| resolve(path, &context.cwd))
}

fn is_read(context: &ToolContext, path: &Path) -> bool {
    context
        .read_files
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(path)
}

fn mark_read(context: &ToolContext, path: &Path) {
    context
        .read_files
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(path.to_path_buf());
}

/// The byte order mark, if the text starts with one, and the rest of the text.
fn split_bom(text: &str) -> (&str, &str) {
    match text.strip_prefix('\u{feff}') {
        Some(body) => ("\u{feff}", body),
        None => ("", text),
    }
}

/// Walks a folder for the file tools. Honours `.gitignore` even outside a git repository, shows
/// dotfiles, and skips `.git`.
fn walker(base: &Path) -> WalkBuilder {
    let mut builder = WalkBuilder::new(base);
    builder
        .hidden(false)
        .require_git(false)
        .sort_by_file_path(|a, b| a.cmp(b));
    builder.filter_entry(|entry| entry.file_name() != ".git");
    builder
}

/// The path under the folder searched, with `/` separators, for matching against a glob.
fn relative_path(path: &Path, base: &Path) -> String {
    let relative = path
        .strip_prefix(base)
        .ok()
        .filter(|rest| !rest.as_os_str().is_empty())
        .or_else(|| path.file_name().map(Path::new))
        .unwrap_or(path);
    relative.to_string_lossy().replace('\\', "/")
}
