use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value, json};
use vatra_extensions::{Origin, parse_mcp_json};

use crate::guard::GuardedFs;
use crate::scan::{
    Found, FoundKind, ProjectRef, ScanResult, SourceDirs, command_files, counted_files, found,
    is_clear, load_plugin, mcp_items, permission_file, permissions_found, problem, same_folder,
    skill_folders, stamp_modified, text_hash,
};

const HOOKS_REASON: &str = "Hooks don't run in Vatra; they keep working in the native CLI";
const AGENTS_REASON: &str = "Subagents don't run in Vatra; they keep working in the native CLI";

/// What `settings.json` gave: its text hash, and its object, which is read once and dropped at the end of the scan.
struct Settings {
    hash: String,
    value: Map<String, Value>,
}

/// A plugin that loaded: its key in `enabledPlugins`, and the options its manifest marks as not sensitive.
struct Loaded {
    key: String,
    public_options: BTreeSet<String>,
}

/// `installed_plugins.json`: the folder each plugin key is installed in.
#[derive(Deserialize)]
struct Installed {
    #[serde(default)]
    plugins: BTreeMap<String, Vec<Install>>,
}

#[derive(Deserialize)]
struct Install {
    #[serde(rename = "installPath")]
    install_path: PathBuf,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default, rename = "projectPath")]
    project_path: Option<String>,
}

/// The parts of `.claude.json` that are kept: the user MCP servers and each project's MCP servers. Every other key
/// is skipped while parsing.
#[derive(Deserialize)]
struct ClaudeJson {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: Option<Value>,
    #[serde(default)]
    projects: BTreeMap<String, ProjectEntry>,
}

#[derive(Deserialize)]
struct ProjectEntry {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: Option<Value>,
}

/// Reads what Claude Code keeps on this machine, through the guard, and writes nothing. A source that cannot be read
/// is recorded as a problem, and the rest of the scan still runs. `project_paths` are the projects this scan is for:
/// their MCP servers and plugin installs are read, and items of a project carry its position in that list.
pub fn scan_claude(fs: &GuardedFs, dirs: &SourceDirs, project_paths: &[PathBuf]) -> ScanResult {
    let mut result = ScanResult::default();
    let guard = fs.with_config_roots(&[dirs.claude.clone(), dirs.codex.clone()]);
    let fs = &guard;
    let settings_file = dirs.claude.join("settings.json");
    let settings = read_settings(fs, &settings_file, &mut result);
    let plugins = plugins(fs, dirs, project_paths, settings.as_ref(), &mut result);
    if let Some(settings) = &settings {
        settings_items(settings, &settings_file, &plugins, &mut result);
    }
    permission_file(
        fs,
        &dirs.claude.join("settings.local.json"),
        Origin::Claude,
        &mut result,
    );
    skill_folders(
        fs,
        &dirs.claude.join("skills"),
        Origin::Claude,
        &[],
        &mut result,
    );
    let commands = dirs.claude.join("commands");
    if is_clear(fs, &commands, &mut result) {
        command_files(fs, &commands, Origin::Claude, &[], &mut result);
    }
    counted_files(
        fs,
        &dirs.agents,
        "md",
        Origin::Claude,
        FoundKind::Agent,
        AGENTS_REASON,
        &mut result,
    );
    claude_json(fs, dirs, project_paths, &mut result);
    crate::context::scan_claude_context(fs, &dirs.claude, project_paths, &mut result);
    stamp_modified(fs, &mut result.found);
    result
}

fn read_settings(fs: &GuardedFs, file: &Path, result: &mut ScanResult) -> Option<Settings> {
    let text = match fs.read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.is_missing() => return None,
        Err(error) => {
            result.problems.push(problem(file, error.to_string()));
            return None;
        }
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(value)) => Some(Settings {
            hash: text_hash(&text),
            value,
        }),
        Ok(_) => {
            result
                .problems
                .push(problem(file, "settings.json must be a JSON object"));
            None
        }
        Err(error) => {
            result.problems.push(problem(
                file,
                format!("invalid JSON in settings.json: {error}"),
            ));
            None
        }
    }
}

/// The plugins enabled in `settings.json`, each read from the folder of its installed version. A plugin that cannot
/// be read is a problem, and the others still load.
fn plugins(
    fs: &GuardedFs,
    dirs: &SourceDirs,
    project_paths: &[PathBuf],
    settings: Option<&Settings>,
    result: &mut ScanResult,
) -> Vec<Loaded> {
    let Some(settings) = settings else {
        return Vec::new();
    };
    let enabled: Vec<String> = match settings
        .value
        .get("enabledPlugins")
        .and_then(Value::as_object)
    {
        Some(map) => map
            .iter()
            .filter(|(_, on)| on.as_bool() == Some(true))
            .map(|(key, _)| key.clone())
            .collect(),
        None => return Vec::new(),
    };
    if enabled.is_empty() {
        return Vec::new();
    }
    let installed_file = dirs.claude.join("plugins").join("installed_plugins.json");
    let installed = read_installed(fs, &installed_file, result);

    let mut loaded = Vec::new();
    for key in enabled {
        let Some(installs) = installed.get(&key) else {
            result.problems.push(problem(
                &installed_file,
                format!("{key} is enabled but not installed"),
            ));
            continue;
        };
        let Some((install, project)) = pick_install(installs, project_paths) else {
            result.problems.push(problem(
                &installed_file,
                format!("{key} is installed for other projects only, so it is not available here"),
            ));
            continue;
        };
        if let Some(bundle) = load_plugin(
            fs,
            Origin::Claude,
            &key,
            &install.install_path,
            project,
            result,
        ) {
            loaded.push(Loaded {
                key,
                public_options: bundle
                    .user_config
                    .iter()
                    .filter(|option| !option.sensitive)
                    .map(|option| option.key.clone())
                    .collect(),
            });
        }
    }
    loaded
}

/// The install a plugin is read from: its user-scope install, else the install of one of `project_paths`. None when
/// every install belongs to another project.
fn pick_install<'a>(
    installs: &'a [Install],
    project_paths: &[PathBuf],
) -> Option<(&'a Install, Option<ProjectRef>)> {
    if let Some(user) = installs
        .iter()
        .find(|install| install.scope.as_deref() == Some("user"))
    {
        return Some((user, None));
    }
    project_paths
        .iter()
        .enumerate()
        .find_map(|(index, project)| {
            installs
                .iter()
                .find(|install| {
                    install
                        .project_path
                        .as_deref()
                        .is_some_and(|recorded| same_folder(recorded, project))
                })
                .map(|install| (install, Some(ProjectRef(index))))
        })
}

fn read_installed(
    fs: &GuardedFs,
    file: &Path,
    result: &mut ScanResult,
) -> BTreeMap<String, Vec<Install>> {
    let text = match fs.read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.is_missing() => return BTreeMap::new(),
        Err(error) => {
            result.problems.push(problem(file, error.to_string()));
            return BTreeMap::new();
        }
    };
    match serde_json::from_str::<Installed>(&text) {
        Ok(installed) => installed.plugins,
        Err(error) => {
            result.problems.push(problem(
                file,
                format!("invalid JSON in installed_plugins.json: {error}"),
            ));
            BTreeMap::new()
        }
    }
}

/// `settings.json` items: hooks (counted, not run), permissions, and the non-sensitive options of plugins.
fn settings_items(settings: &Settings, file: &Path, plugins: &[Loaded], result: &mut ScanResult) {
    if let Some(hooks) = settings.value.get("hooks") {
        let events: Vec<String> = hooks
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default();
        result.found.push(Found {
            unsupported: Some(HOOKS_REASON.to_string()),
            details: json!({ "events": events }),
            ..found(
                Origin::Claude,
                FoundKind::Hook,
                "hooks",
                file,
                &settings.hash,
            )
        });
    }

    if let Some(permissions) = settings.value.get("permissions").and_then(Value::as_object) {
        result.found.push(permissions_found(
            Origin::Claude,
            file,
            &settings.hash,
            permissions,
        ));
    }

    let Some(configs) = settings
        .value
        .get("pluginConfigs")
        .and_then(Value::as_object)
    else {
        return;
    };
    for (key, config) in configs {
        let Some(plugin) = plugins.iter().find(|plugin| &plugin.key == key) else {
            continue;
        };
        let options: Map<String, Value> = config
            .get("options")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter(|(name, _)| plugin.public_options.contains(name.as_str()))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        if options.is_empty() {
            continue;
        }
        result.found.push(Found {
            details: json!({ "options": options }),
            ..found(
                Origin::Claude,
                FoundKind::PluginConfig,
                key,
                file,
                &settings.hash,
            )
        });
    }
}

/// `.claude.json`: the user MCP servers, and the MCP servers of each project in `project_paths`.
fn claude_json(
    fs: &GuardedFs,
    dirs: &SourceDirs,
    project_paths: &[PathBuf],
    result: &mut ScanResult,
) {
    let file = claude_json_file(fs, dirs);
    let text = match fs.read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.is_missing() => return,
        Err(error) => {
            result.problems.push(problem(&file, error.to_string()));
            return;
        }
    };
    let parsed: ClaudeJson = match serde_json::from_str(&text) {
        Ok(parsed) => parsed,
        Err(error) => {
            result.problems.push(problem(
                &file,
                format!("invalid JSON in .claude.json: {error}"),
            ));
            return;
        }
    };
    drop(text);

    if let Some(servers) = parsed.mcp_servers {
        mcp_servers_from(servers, None, &file, result);
    }
    for (index, project) in project_paths.iter().enumerate() {
        let entry = parsed
            .projects
            .iter()
            .find(|(recorded, _)| same_folder(recorded, project));
        if let Some((
            _,
            ProjectEntry {
                mcp_servers: Some(servers),
            },
        )) = entry
        {
            mcp_servers_from(servers.clone(), Some(ProjectRef(index)), &file, result);
        }
    }
}

/// The `.claude.json` in the config folder when there is one, else the one in HOME.
fn claude_json_file(fs: &GuardedFs, dirs: &SourceDirs) -> PathBuf {
    let in_config = dirs.claude.join(".claude.json");
    if fs.metadata(&in_config).is_ok() {
        in_config
    } else {
        dirs.home.join(".claude.json")
    }
}

fn mcp_servers_from(
    servers: Value,
    project: Option<ProjectRef>,
    source: &Path,
    result: &mut ScanResult,
) {
    let text = json!({ "mcpServers": servers }).to_string();
    match parse_mcp_json(&text) {
        Ok(file) => mcp_items(Origin::Claude, file, project, source, result),
        Err(error) => result.problems.push(problem(source, error.to_string())),
    }
}
