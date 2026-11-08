use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};
use vatra_core::{McpServerConfig, PluginContext};

use crate::mcp_file::{self, LiteralSecret, McpFile};
use crate::{FormatError, Skill, parse_command, parse_skill};

/// Which manifest a plugin folder has. The first one found wins, in the order of `MANIFESTS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestKind {
    Claude,
    Portable,
    Codex,
    None,
}

const MANIFESTS: [(&str, ManifestKind); 3] = [
    (".claude-plugin/plugin.json", ManifestKind::Claude),
    ("plugin.json", ManifestKind::Portable),
    (".codex-plugin/plugin.json", ManifestKind::Codex),
];

/// Folders that are never scanned for skills or commands.
const SKIPPED_FOLDERS: [&str; 2] = ["node_modules", ".git"];

/// Manifest keys for components Vatra does not run, with the reason shown to the user.
const UNSUPPORTED_KEYS: [(&str, &str); 6] = [
    ("lspServers", "Language servers don't run in Vatra"),
    ("outputStyles", "Output styles aren't supported in Vatra"),
    ("monitors", "Monitors aren't supported in Vatra"),
    ("channels", "Channels aren't supported in Vatra"),
    ("themes", "Themes aren't supported in Vatra"),
    ("workflows", "Workflows aren't supported in Vatra"),
];
const APP_REASON: &str =
    "Account connectors are tied to the ChatGPT account and don't move to Vatra";
const HOOKS_REASON: &str = "Hooks don't run in Vatra; they keep working in the native CLI";
const AGENTS_REASON: &str = "Subagents don't run in Vatra; they keep working in the native CLI";

/// A component that is in the plugin but does not run in Vatra.
#[derive(Clone, Debug, PartialEq)]
pub struct UnsupportedComponent {
    pub kind: String,
    pub reason: String,
}

/// One setting the user is asked for when the plugin is enabled, from the manifest's `userConfig`.
#[derive(Clone, Debug, PartialEq)]
pub struct UserOption {
    pub key: String,
    pub kind: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub required: bool,
    pub sensitive: bool,
    pub default: Option<Value>,
}

/// A plugin folder, read into the parts Vatra can run. Its hooks and subagents are only listed in `unsupported`.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginBundle {
    pub name: String,
    pub version: Option<String>,
    pub description: String,
    pub author: Option<String>,
    pub manifest_kind: ManifestKind,
    pub root: PathBuf,
    /// Skills and commands together. A command has `is_command` set.
    pub skills: Vec<Skill>,
    pub mcp_servers: Vec<McpServerConfig>,
    /// Literal values from the MCP files, kept out of the servers until the user decides where they are stored.
    pub literal_secrets: Vec<LiteralSecret>,
    pub user_config: Vec<UserOption>,
    pub unsupported: Vec<UnsupportedComponent>,
    /// Files and entries that could not be read. The rest of the plugin still loads.
    pub problems: Vec<String>,
}

/// The plugin folder, with its real location. A file counts as inside only when its location after links is under
/// the real folder.
struct Folder {
    root: PathBuf,
    real: PathBuf,
}

impl Folder {
    fn holds(&self, path: &Path) -> bool {
        fs::canonicalize(path).is_ok_and(|real| real.starts_with(&self.real))
    }
}

/// Reads a plugin folder: its manifest, skills, commands, MCP servers and the components Vatra does not run.
/// A file that cannot be read is recorded in `problems`, and the other parts still load.
pub fn read_plugin(root: &Path) -> Result<PluginBundle, FormatError> {
    if !root.is_dir() {
        return Err(FormatError::Read {
            path: root.to_path_buf(),
            source: io::Error::new(io::ErrorKind::NotFound, "not a plugin folder"),
        });
    }
    let real = fs::canonicalize(root).map_err(|source| FormatError::Read {
        path: root.to_path_buf(),
        source,
    })?;
    let folder = Folder {
        root: root.to_path_buf(),
        real,
    };
    let mut problems = Vec::new();
    let (manifest_kind, manifest) = read_manifest(&folder, &mut problems);
    let folder_name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = string_field(&manifest, "name", &mut problems).unwrap_or(folder_name);

    let mut skills = load_skills(&folder, &manifest, &mut problems);
    skills.extend(load_commands(&folder, &manifest, &mut problems));
    for skill in &mut skills {
        skill.plugin = Some(name.clone());
    }

    let (mut mcp_servers, literal_secrets) = load_mcp(&folder, &manifest, &mut problems);
    for server in &mut mcp_servers {
        server.plugin = Some(name.clone());
        server.plugin_context = Some(PluginContext {
            root: folder.root.clone(),
            ..Default::default()
        });
    }

    Ok(PluginBundle {
        version: string_field(&manifest, "version", &mut problems),
        description: string_field(&manifest, "description", &mut problems).unwrap_or_default(),
        author: author(&manifest, &mut problems),
        user_config: user_options(&manifest, &mut problems),
        unsupported: unsupported_components(&folder.root, &manifest),
        name,
        manifest_kind,
        root: folder.root,
        skills,
        mcp_servers,
        literal_secrets,
        problems,
    })
}

/// Reads the first manifest found. A manifest that is not a JSON object is recorded, and the plugin reads with defaults.
fn read_manifest(
    folder: &Folder,
    problems: &mut Vec<String>,
) -> (ManifestKind, Map<String, Value>) {
    for (relative, kind) in MANIFESTS {
        let file = folder.root.join(relative);
        if !file.is_file() {
            continue;
        }
        if !folder.holds(&file) {
            problems.push(outside(&file));
            return (kind, Map::new());
        }
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) => {
                problems.push(format!("cannot read {relative}: {error}"));
                return (kind, Map::new());
            }
        };
        return match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(manifest)) => (kind, manifest),
            Ok(_) => {
                problems.push(format!("{relative} must be a JSON object"));
                (kind, Map::new())
            }
            Err(error) => {
                problems.push(format!("invalid JSON in {relative}: {error}"));
                (kind, Map::new())
            }
        };
    }
    (ManifestKind::None, Map::new())
}

fn string_field(
    manifest: &Map<String, Value>,
    key: &str,
    problems: &mut Vec<String>,
) -> Option<String> {
    match manifest.get(key)? {
        Value::String(text) => Some(text.clone()),
        _ => {
            problems.push(format!("`{key}` in the manifest must be a string"));
            None
        }
    }
}

fn author(manifest: &Map<String, Value>, problems: &mut Vec<String>) -> Option<String> {
    match manifest.get("author")? {
        Value::String(name) => Some(name.clone()),
        Value::Object(object) => object
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => {
            problems.push("`author` in the manifest must be a string or an object".to_string());
            None
        }
    }
}

/// Skill folders: the default `skills/` folder, and any folders the manifest adds. A manifest path may name one
/// skill folder or a folder of them.
fn load_skills(
    folder: &Folder,
    manifest: &Map<String, Value>,
    problems: &mut Vec<String>,
) -> Vec<Skill> {
    let default = folder.root.join("skills");
    let mut folders = skill_folders(folder, &default, problems);
    for custom in manifest_paths(folder, manifest, "skills", problems).unwrap_or_default() {
        if custom == default {
            continue;
        }
        if is_skill(folder, &custom, problems) {
            folders.push(custom);
        } else {
            folders.extend(skill_folders(folder, &custom, problems));
        }
    }

    let mut skills = Vec::new();
    for skill_folder in folders {
        match parse_skill(&skill_folder) {
            Ok(skill) => skills.push(skill),
            Err(error) => problems.push(error.to_string()),
        }
    }
    skills
}

/// Command files. The manifest's `commands` paths replace the default `commands/` folder.
fn load_commands(
    folder: &Folder,
    manifest: &Map<String, Value>,
    problems: &mut Vec<String>,
) -> Vec<Skill> {
    let paths = manifest_paths(folder, manifest, "commands", problems)
        .unwrap_or_else(|| vec![folder.root.join("commands")]);
    let mut commands = Vec::new();
    for path in paths {
        if path.is_file() {
            push_command(&path, &[], &mut commands, problems);
        } else {
            collect_commands(folder, &path, &[], &mut commands, problems);
        }
    }
    commands
}

fn collect_commands(
    folder: &Folder,
    dir: &Path,
    namespace: &[String],
    commands: &mut Vec<Skill>,
    problems: &mut Vec<String>,
) {
    for child in sorted_children(dir) {
        if is_skipped(&child) {
            continue;
        }
        if !folder.holds(&child) {
            problems.push(outside(&child));
            continue;
        }
        if child.is_dir() {
            let mut inner = namespace.to_vec();
            inner.push(file_name(&child));
            collect_commands(folder, &child, &inner, commands, problems);
        } else if child.extension().and_then(OsStr::to_str) == Some("md") {
            push_command(&child, namespace, commands, problems);
        }
    }
}

fn push_command(
    file: &Path,
    namespace: &[String],
    commands: &mut Vec<Skill>,
    problems: &mut Vec<String>,
) {
    match parse_command(file, namespace) {
        Ok(command) => commands.push(command),
        Err(error) => problems.push(error.to_string()),
    }
}

/// The MCP servers. Without a manifest key this is `.mcp.json` at the root. The key can name one file, a list of
/// files, or hold the servers inline. Problems of single servers are recorded, and the other servers are kept.
fn load_mcp(
    folder: &Folder,
    manifest: &Map<String, Value>,
    problems: &mut Vec<String>,
) -> (Vec<McpServerConfig>, Vec<LiteralSecret>) {
    let mut servers = Vec::new();
    let mut secrets = Vec::new();

    if let Some(inline @ Value::Object(_)) = manifest.get("mcpServers") {
        match mcp_file::servers_from_json(inline) {
            Ok(file) => absorb(
                file,
                "mcpServers in the manifest",
                &mut servers,
                &mut secrets,
                problems,
            ),
            Err(error) => problems.push(format!("mcpServers in the manifest: {error}")),
        }
        return (servers, secrets);
    }

    for relative in mcp_sources(folder, manifest, problems) {
        let Some(file) = resolve(folder, &relative, "mcpServers", problems) else {
            continue;
        };
        match read_mcp_file(&file, &relative) {
            Ok(parsed) => absorb(parsed, &relative, &mut servers, &mut secrets, problems),
            Err(message) => problems.push(message),
        }
    }
    (servers, secrets)
}

/// The MCP files a manifest names, relative to the root.
fn mcp_sources(
    folder: &Folder,
    manifest: &Map<String, Value>,
    problems: &mut Vec<String>,
) -> Vec<String> {
    match manifest.get("mcpServers") {
        None => {
            if folder.root.join(".mcp.json").is_file() {
                vec![".mcp.json".to_string()]
            } else {
                Vec::new()
            }
        }
        Some(Value::String(text)) => vec![text.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item.as_str() {
                Some(text) => Some(text.to_string()),
                None => {
                    problems
                        .push("`mcpServers` in the manifest must hold only strings".to_string());
                    None
                }
            })
            .collect(),
        Some(_) => {
            problems.push(
                "`mcpServers` in the manifest must be a string, a list or an object".to_string(),
            );
            Vec::new()
        }
    }
}

fn read_mcp_file(file: &Path, label: &str) -> Result<McpFile, String> {
    let text = fs::read_to_string(file).map_err(|error| format!("cannot read {label}: {error}"))?;
    mcp_file::parse_mcp_json(&text).map_err(|error| format!("{label}: {error}"))
}

fn absorb(
    file: McpFile,
    label: &str,
    servers: &mut Vec<McpServerConfig>,
    secrets: &mut Vec<LiteralSecret>,
    problems: &mut Vec<String>,
) {
    servers.extend(file.servers);
    secrets.extend(file.literal_secrets);
    problems.extend(
        file.problems
            .into_iter()
            .map(|problem| format!("{label}: {problem}")),
    );
}

fn user_options(manifest: &Map<String, Value>, problems: &mut Vec<String>) -> Vec<UserOption> {
    let Some(value) = manifest.get("userConfig") else {
        return Vec::new();
    };
    let Some(options) = value.as_object() else {
        problems.push("`userConfig` in the manifest must be an object".to_string());
        return Vec::new();
    };
    let mut parsed = Vec::new();
    for (key, option) in options {
        let Some(option) = option.as_object() else {
            problems.push(format!("`userConfig.{key}` must be an object"));
            continue;
        };
        let sensitive = option
            .get("sensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        parsed.push(UserOption {
            key: key.clone(),
            kind: option
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("string")
                .to_string(),
            title: option
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string),
            description: option
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
            required: option
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            // A sensitive default would be kept in plain text, so it is not kept.
            default: if sensitive {
                None
            } else {
                option.get("default").cloned()
            },
            sensitive,
        });
    }
    parsed
}

fn unsupported_components(root: &Path, manifest: &Map<String, Value>) -> Vec<UnsupportedComponent> {
    let mut found: Vec<(&str, &str)> = UNSUPPORTED_KEYS
        .iter()
        .filter(|(key, _)| manifest.contains_key(*key))
        .copied()
        .collect();
    if manifest.contains_key("apps") || root.join(".app.json").is_file() {
        found.push((".app.json", APP_REASON));
    }
    if manifest.contains_key("hooks") || root.join("hooks").join("hooks.json").is_file() {
        found.push(("hooks", HOOKS_REASON));
    }
    if manifest.contains_key("agents") || root.join("agents").is_dir() {
        found.push(("agents", AGENTS_REASON));
    }
    found
        .into_iter()
        .map(|(kind, reason)| UnsupportedComponent {
            kind: kind.to_string(),
            reason: reason.to_string(),
        })
        .collect()
}

/// The paths a manifest key names, or None when the key is absent.
fn manifest_paths(
    folder: &Folder,
    manifest: &Map<String, Value>,
    key: &str,
    problems: &mut Vec<String>,
) -> Option<Vec<PathBuf>> {
    let value = manifest.get(key)?;
    let relative: Vec<&str> = match value {
        Value::String(text) => vec![text.as_str()],
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item.as_str() {
                Some(text) => Some(text),
                None => {
                    problems.push(format!("`{key}` in the manifest must hold only strings"));
                    None
                }
            })
            .collect(),
        _ => {
            problems.push(format!(
                "`{key}` in the manifest must be a string or a list"
            ));
            Vec::new()
        }
    };
    let mut paths = Vec::new();
    for text in relative {
        if let Some(path) = resolve(folder, text, key, problems) {
            if !path.exists() {
                problems.push(format!("`{key}` path `{text}` does not exist"));
            }
            paths.push(path);
        }
    }
    Some(paths)
}

/// Joins a manifest path to the plugin root. A path that is absolute or climbs with `..` is refused, and so is one
/// that exists but leads outside the folder through a link.
fn resolve(
    folder: &Folder,
    relative: &str,
    key: &str,
    problems: &mut Vec<String>,
) -> Option<PathBuf> {
    let path = Path::new(relative);
    let inside = path
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    if !inside {
        problems.push(format!(
            "`{key}` path `{relative}` points outside the plugin folder"
        ));
        return None;
    }
    let joined = folder.root.join(path);
    if joined.exists() && !folder.holds(&joined) {
        problems.push(outside(&joined));
        return None;
    }
    Some(joined)
}

/// Whether `dir` is a skill folder: it has a `SKILL.md` that stays inside the plugin folder.
fn is_skill(folder: &Folder, dir: &Path, problems: &mut Vec<String>) -> bool {
    let file = dir.join("SKILL.md");
    if !file.is_file() {
        return false;
    }
    if folder.holds(&file) {
        return true;
    }
    problems.push(outside(&file));
    false
}

/// The child folders of `dir` that are skill folders, in name order. A child that leads outside the plugin is recorded.
fn skill_folders(folder: &Folder, dir: &Path, problems: &mut Vec<String>) -> Vec<PathBuf> {
    let mut skill_dirs = Vec::new();
    for child in sorted_children(dir) {
        if is_skipped(&child) || !child.is_dir() {
            continue;
        }
        if !folder.holds(&child) {
            problems.push(outside(&child));
            continue;
        }
        if is_skill(folder, &child, problems) {
            skill_dirs.push(child);
        }
    }
    skill_dirs
}

fn outside(path: &Path) -> String {
    format!(
        "{} points outside the plugin folder through a link",
        path.display()
    )
}

/// The entries of `dir`, sorted. A folder that cannot be read has none.
fn sorted_children(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut children: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    children.sort();
    children
}

fn is_skipped(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| SKIPPED_FOLDERS.contains(&name))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}
