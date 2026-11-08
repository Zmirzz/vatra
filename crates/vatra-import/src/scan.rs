use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Serialize, Serializer};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use vatra_core::McpServerConfig;
use vatra_extensions::{
    BangForm, LiteralSecret, McpFile, Origin, PluginBundle, RunsCode, Scope, Skill, bang_spans, content_hash,
    parse_command, parse_skill, read_plugin,
};

use crate::guard::GuardedFs;

/// What a scan found, in the terms the import report uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FoundKind {
    Plugin,
    Skill,
    Command,
    McpServer,
    Permission,
    PluginConfig,
    Hook,
    Agent,
    /// Instructions the agent reads at the start, such as `AGENTS.md`.
    Instructions,
    /// Curated memory notes. Raw notes are not read.
    Memory,
    /// A part of a plugin that Vatra does not run, such as language servers.
    Unsupported,
}

/// A project the scan was given, by its position in the project paths passed to the scan. It is not a catalog id: the
/// caller maps it to one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ProjectRef(pub usize);

/// One item the scan read from a source. Items with `unsupported` set are counted and reported, not imported.
/// `scope` is always global here; an item that belongs to one project carries its `project` instead.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Found {
    pub kind: FoundKind,
    pub name: String,
    pub origin: Origin,
    pub scope: Scope,
    pub project: Option<ProjectRef>,
    /// The file or folder the item came from.
    pub source: PathBuf,
    /// The content hash of the source: a folder hash for folders, a text hash for single files.
    pub hash: String,
    pub runs_code: Vec<RunsCode>,
    /// Literal values from the source. Only the server and key are serialized; the value is never printed.
    #[serde(serialize_with = "keys_only")]
    pub secrets: Vec<LiteralSecret>,
    /// The reason Vatra does not run the item, shown in the report.
    pub unsupported: Option<String>,
    pub details: Value,
    /// When the source was last changed, set by the scan. A conflict keeps its newest copy.
    #[serde(skip)]
    pub modified: Option<SystemTime>,
    /// Other origins that hold the same content, set when duplicates merge.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub also_from: Vec<Origin>,
}

/// A file or entry that could not be read. The rest of the scan still runs.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ScanProblem {
    pub source: PathBuf,
    pub message: String,
}

/// What a scan of one source's folders found, and what it could not read.
#[derive(Debug, Default, Serialize)]
pub struct ScanResult {
    pub found: Vec<Found>,
    pub problems: Vec<ScanProblem>,
}

#[derive(Serialize)]
struct SecretKey<'a> {
    server: &'a str,
    key: &'a str,
}

fn keys_only<S: Serializer>(secrets: &[LiteralSecret], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(secrets.iter().map(|secret| SecretKey {
        server: &secret.server,
        key: &secret.key,
    }))
}

/// The folders a scan reads. `claude` is the Claude Code config folder, `codex` the Codex home, `agents` the
/// subagent folder, and `home` the folder that holds `.claude.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceDirs {
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub agents: PathBuf,
    pub home: PathBuf,
}

impl SourceDirs {
    /// The folders of this machine, from `CLAUDE_CONFIG_DIR` and `CODEX_HOME` when they are set.
    pub fn detect() -> SourceDirs {
        SourceDirs::from_env(|name| std::env::var(name).ok())
    }

    /// The folders for a given environment. Tests use this instead of changing the process environment.
    pub fn from_env(env: impl Fn(&str) -> Option<String>) -> SourceDirs {
        let home_keys = if cfg!(windows) {
            ["USERPROFILE", "HOME"]
        } else {
            ["HOME", "USERPROFILE"]
        };
        let home = home_keys
            .into_iter()
            .find_map(&env)
            .map(PathBuf::from)
            .unwrap_or_default();
        let claude = env("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude"));
        let codex = env("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        let agents = claude.join("agents");
        SourceDirs {
            claude,
            codex,
            agents,
            home,
        }
    }
}

/// A new item with no runs-code, secrets, unsupported reason or modification time. The scans set those after.
pub(crate) fn found(origin: Origin, kind: FoundKind, name: impl Into<String>, source: &Path, hash: &str) -> Found {
    Found {
        kind,
        name: name.into(),
        origin,
        scope: Scope::Global,
        project: None,
        source: source.to_path_buf(),
        hash: hash.to_string(),
        runs_code: Vec::new(),
        secrets: Vec::new(),
        unsupported: None,
        details: Value::Null,
        modified: None,
        also_from: Vec::new(),
    }
}

pub(crate) fn problem(source: &Path, message: impl Into<String>) -> ScanProblem {
    ScanProblem {
        source: source.to_path_buf(),
        message: message.into(),
    }
}

/// The SHA-256 of one file's text, in hex. A folder is hashed with `content_hash`; a single file is hashed here.
pub(crate) fn text_hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A skill or command, with its `!` commands as runs-code and any frontmatter keys Vatra does not read.
pub(crate) fn skill_found(origin: Origin, skill: &Skill, hash: &str) -> Found {
    let kind = if skill.is_command {
        FoundKind::Command
    } else {
        FoundKind::Skill
    };
    let mut details = json!({ "description": skill.description });
    if !skill.unknown_fields.is_empty() {
        details["unknown_fields"] = json!(skill.unknown_fields);
    }
    Found {
        runs_code: bang_runs(&skill.body),
        details,
        ..found(origin, kind, skill.qualified_name(), &skill.dir, hash)
    }
}

/// The permission rules of one settings file, as one item named by the file, so each file keeps its own rules.
pub(crate) fn permissions_found(
    origin: Origin,
    file: &Path,
    hash: &str,
    permissions: &Map<String, Value>,
) -> Found {
    Found {
        details: json!({
            "allow": permission_list(permissions, "allow"),
            "deny": permission_list(permissions, "deny"),
            "ask": permission_list(permissions, "ask"),
        }),
        ..found(
            origin,
            FoundKind::Permission,
            format!("permissions:{}", file_name(file)),
            file,
            hash,
        )
    }
}

fn permission_list(object: &Map<String, Value>, key: &str) -> Vec<String> {
    object
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// The permissions of one settings file, when it has any. A file that cannot be read or parsed is a problem.
pub(crate) fn permission_file(fs: &GuardedFs, file: &Path, origin: Origin, result: &mut ScanResult) {
    let text = match fs.read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.is_missing() => return,
        Err(error) => {
            result.problems.push(problem(file, error.to_string()));
            return;
        }
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(value) => {
            if let Some(permissions) = value.get("permissions").and_then(Value::as_object) {
                result
                    .found
                    .push(permissions_found(origin, file, &text_hash(&text), permissions));
            }
        }
        Err(error) => result
            .problems
            .push(problem(file, format!("invalid JSON in {}: {error}", file_name(file)))),
    }
}

pub(crate) fn mcp_found(
    origin: Origin,
    server: &McpServerConfig,
    name: String,
    secrets: Vec<LiteralSecret>,
    source: &Path,
) -> Found {
    let details = serde_json::to_value(server).unwrap_or(Value::Null);
    let hash = text_hash(&details.to_string());
    Found {
        secrets,
        runs_code: server_runs(server),
        details,
        ..found(origin, FoundKind::McpServer, name, source, &hash)
    }
}

/// The servers of a parsed MCP file, each with the literal values that belong to it. The file's problems are kept.
pub(crate) fn mcp_items(
    origin: Origin,
    file: McpFile,
    project: Option<ProjectRef>,
    source: &Path,
    result: &mut ScanResult,
) {
    let McpFile {
        servers,
        literal_secrets,
        problems,
    } = file;
    result
        .problems
        .extend(problems.into_iter().map(|message| problem(source, message)));
    for server in servers {
        let secrets = secrets_of(&literal_secrets, &server.name);
        let name = server.name.clone();
        result.found.push(Found {
            project,
            ..mcp_found(origin, &server, name, secrets, source)
        });
    }
}

/// Whether a path exists. A path that cannot be looked at is a problem, and counts as not there.
pub(crate) fn exists(fs: &GuardedFs, path: &Path, result: &mut ScanResult) -> bool {
    match fs.metadata(path) {
        Ok(_) => true,
        Err(error) if error.is_missing() => false,
        Err(error) => {
            result.problems.push(problem(path, error.to_string()));
            false
        }
    }
}

/// Files that are counted and not read, such as subagents: one unsupported item per file with the extension, named by
/// its stem. A folder that cannot be read is a problem.
pub(crate) fn counted_files(
    fs: &GuardedFs,
    dir: &Path,
    extension: &str,
    origin: Origin,
    kind: FoundKind,
    reason: &str,
    result: &mut ScanResult,
) {
    let children = match fs.read_dir(dir) {
        Ok(children) => children,
        Err(error) if error.is_missing() => return,
        Err(error) => {
            result.problems.push(problem(dir, error.to_string()));
            return;
        }
    };
    for child in children {
        if child.extension().and_then(|ext| ext.to_str()) != Some(extension) {
            continue;
        }
        result.found.push(Found {
            unsupported: Some(reason.to_string()),
            ..found(origin, kind, file_stem(&child), &child, "")
        });
    }
}

/// A text file read whole. Its name is the file name and its hash the text hash. A missing file is nothing.
pub(crate) fn text_item(fs: &GuardedFs, origin: Origin, kind: FoundKind, file: &Path, result: &mut ScanResult) {
    match fs.read_to_string(file) {
        Ok(text) => result
            .found
            .push(found(origin, kind, file_name(file), file, &text_hash(&text))),
        Err(error) if error.is_missing() => {}
        Err(error) => result.problems.push(problem(file, error.to_string())),
    }
}

/// Skill folders: each child folder of `root` with a `SKILL.md`. A folder listed in `disabled`, by its folder or its
/// `SKILL.md`, is left out. A folder with a forbidden file is not read.
pub(crate) fn skill_folders(
    fs: &GuardedFs,
    root: &Path,
    origin: Origin,
    disabled: &[PathBuf],
    result: &mut ScanResult,
) {
    for folder in child_folders(fs, root, result) {
        if fs.metadata(&folder.join("SKILL.md")).is_err() {
            continue;
        }
        let off = disabled.iter().any(|path| {
            same_folder(&path.display().to_string(), &folder)
                || same_folder(&path.display().to_string(), &folder.join("SKILL.md"))
        });
        if off || !is_clear(fs, &folder, result) {
            continue;
        }
        let skill = match parse_skill(&folder) {
            Ok(skill) => skill,
            Err(error) => {
                result.problems.push(problem(&folder, error.to_string()));
                continue;
            }
        };
        match content_hash(&folder) {
            Ok(hash) => result.found.push(skill_found(origin, &skill, &hash)),
            Err(error) => result.problems.push(problem(&folder, error.to_string())),
        }
    }
}

/// Command files: the Markdown files under `dir`, with the folders above each one as its namespace.
pub(crate) fn command_files(fs: &GuardedFs, dir: &Path, origin: Origin, namespace: &[String], result: &mut ScanResult) {
    let children = match fs.read_dir(dir) {
        Ok(children) => children,
        Err(error) if error.is_missing() => return,
        Err(error) => {
            result.problems.push(problem(dir, error.to_string()));
            return;
        }
    };
    for child in children {
        let Ok(metadata) = fs.metadata(&child) else {
            result.problems.push(problem(&child, "cannot read this entry"));
            continue;
        };
        if metadata.is_dir() {
            let mut inner = namespace.to_vec();
            inner.push(file_name(&child));
            command_files(fs, &child, origin, &inner, result);
        } else if child.extension().and_then(|ext| ext.to_str()) == Some("md") {
            add_command(fs, &child, origin, namespace, result);
        }
    }
}

fn add_command(fs: &GuardedFs, file: &Path, origin: Origin, namespace: &[String], result: &mut ScanResult) {
    let text = match fs.read_to_string(file) {
        Ok(text) => text,
        Err(error) => {
            result.problems.push(problem(file, error.to_string()));
            return;
        }
    };
    match parse_command(file, namespace) {
        Ok(command) => result.found.push(Found {
            // A command is its file, so the source is the file and not the folder it sits in.
            source: file.to_path_buf(),
            ..skill_found(origin, &command, &text_hash(&text))
        }),
        Err(error) => result.problems.push(problem(file, error.to_string())),
    }
}

/// Reads one plugin folder. Its items go into `result`, and its bundle is returned for the caller's own fields. A
/// folder with a forbidden file is not read at all: its content hash would read that file, so the plugin is reported
/// and left out.
pub(crate) fn load_plugin(
    fs: &GuardedFs,
    origin: Origin,
    key: &str,
    root: &Path,
    project: Option<ProjectRef>,
    result: &mut ScanResult,
) -> Option<PluginBundle> {
    let walked = match fs.walk(root) {
        Ok(walked) => walked,
        Err(error) => {
            result
                .problems
                .push(problem(root, format!("{key}: cannot read the plugin folder: {error}")));
            return None;
        }
    };
    if !walked.skipped.is_empty() {
        result.problems.push(problem(
            root,
            format!(
                "{key}: not loaded, it holds files that are not read: {}",
                list(&walked.skipped)
            ),
        ));
        return None;
    }
    let bundle = match read_plugin(root) {
        Ok(bundle) => bundle,
        Err(error) => {
            result.problems.push(problem(root, format!("{key}: {error}")));
            return None;
        }
    };
    let hash = match content_hash(root) {
        Ok(hash) => hash,
        Err(error) => {
            result
                .problems
                .push(problem(root, format!("{key}: cannot hash the plugin folder: {error}")));
            return None;
        }
    };
    result
        .problems
        .extend(bundle.problems.iter().map(|message| problem(root, message.clone())));
    add_plugin(origin, &bundle, &hash, project, result);
    Some(bundle)
}

/// The items of one plugin: the plugin itself, its skills and commands, its MCP servers, and its unsupported parts.
fn add_plugin(origin: Origin, bundle: &PluginBundle, hash: &str, project: Option<ProjectRef>, result: &mut ScanResult) {
    let plugin = bundle.name.as_str();
    let start = result.found.len();
    result.found.push(Found {
        details: json!({
            "description": bundle.description,
            "version": bundle.version,
            "author": bundle.author,
        }),
        ..found(origin, FoundKind::Plugin, plugin, &bundle.root, hash)
    });
    for skill in &bundle.skills {
        result.found.push(skill_found(origin, skill, hash));
    }
    for server in &bundle.mcp_servers {
        let name = format!("{plugin}:{}", server.name);
        let secrets = secrets_of(&bundle.literal_secrets, &server.name);
        result
            .found
            .push(mcp_found(origin, server, name, secrets, &bundle.root));
    }
    for part in &bundle.unsupported {
        result.found.push(Found {
            unsupported: Some(part.reason.clone()),
            ..found(
                origin,
                FoundKind::Unsupported,
                format!("{plugin}:{}", part.kind),
                &bundle.root,
                hash,
            )
        });
    }
    for item in &mut result.found[start..] {
        item.project = project;
    }
}

/// Records when each item's source was last changed, so a conflict can keep the newest copy.
pub(crate) fn stamp_modified(fs: &GuardedFs, found: &mut [Found]) {
    for item in found {
        item.modified = fs
            .metadata(&item.source)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
    }
}

fn bang_runs(body: &str) -> Vec<RunsCode> {
    bang_spans(body)
        .into_iter()
        .map(|span| RunsCode {
            what: match span.form {
                BangForm::Inline => "inline command".into(),
                BangForm::Fenced => "fenced command".into(),
            },
            command: span.command,
        })
        .collect()
}

/// What an MCP server runs on this machine, as the catalog lists it: its program and its headers helper.
fn server_runs(server: &McpServerConfig) -> Vec<RunsCode> {
    let mut runs = Vec::new();
    if let Some(command) = &server.command {
        let line: Vec<&str> = std::iter::once(command.as_str())
            .chain(server.args.iter().map(String::as_str))
            .collect();
        runs.push(RunsCode {
            what: "MCP server".into(),
            command: line.join(" "),
        });
    }
    if let Some(helper) = &server.headers_helper {
        runs.push(RunsCode {
            what: "MCP headers helper".into(),
            command: helper.clone(),
        });
    }
    runs
}

pub(crate) fn secrets_of(all: &[LiteralSecret], server: &str) -> Vec<LiteralSecret> {
    all.iter().filter(|secret| secret.server == server).cloned().collect()
}

/// The child folders of `root`. A child that cannot be looked at is a problem.
pub(crate) fn child_folders(fs: &GuardedFs, root: &Path, result: &mut ScanResult) -> Vec<PathBuf> {
    let children = match fs.read_dir(root) {
        Ok(children) => children,
        Err(error) if error.is_missing() => return Vec::new(),
        Err(error) => {
            result.problems.push(problem(root, error.to_string()));
            return Vec::new();
        }
    };
    let mut folders = Vec::new();
    for child in children {
        match fs.metadata(&child) {
            Ok(metadata) if metadata.is_dir() => folders.push(child),
            Ok(_) => {}
            Err(error) => result.problems.push(problem(&child, error.to_string())),
        }
    }
    folders
}

/// Whether a folder can be read: none of its files is forbidden. A folder that is missing is not a problem here.
pub(crate) fn is_clear(fs: &GuardedFs, root: &Path, result: &mut ScanResult) -> bool {
    match fs.walk(root) {
        Ok(walked) if walked.skipped.is_empty() => true,
        Ok(walked) => {
            result.problems.push(problem(
                root,
                format!("not read, it holds files that are not read: {}", list(&walked.skipped)),
            ));
            false
        }
        Err(error) if error.is_missing() => false,
        Err(error) => {
            result.problems.push(problem(root, error.to_string()));
            false
        }
    }
}

pub(crate) fn same_folder(recorded: &str, project: &Path) -> bool {
    normal_folder(recorded) == normal_folder(&project.display().to_string())
}

/// A folder path with one separator and, on Windows, no case, so the same folder compares equal.
fn normal_folder(text: &str) -> String {
    let text = text.replace('/', "\\");
    let text = text.trim_end_matches('\\');
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text.to_string()
    }
}

pub(crate) fn list(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(crate) fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}
