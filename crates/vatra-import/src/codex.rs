use std::path::{Path, PathBuf};

use toml::Table;
use vatra_extensions::{Origin, parse_codex_mcp};

use crate::guard::GuardedFs;
use crate::scan::{
    Found, FoundKind, ProjectRef, ScanResult, SourceDirs, child_folders, counted_files, exists, file_name, found,
    load_plugin, mcp_items, problem, skill_folders, stamp_modified, text_hash, text_item,
};

const AGENTS_REASON: &str = "Codex agents don't run in Vatra; they keep working in the native CLI";
const HOOKS_REASON: &str = "Codex hooks don't run in Vatra; they keep working in the native CLI";
/// The memory files Codex reads as a summary. The raw notes and the rollout summaries stay where they are.
const MEMORY_FILES: [&str; 2] = ["MEMORY.md", "memory_summary.md"];

/// `config.toml` of a Codex home or of a project: its file, its text hash, and its table, read once.
struct Config {
    file: PathBuf,
    hash: String,
    table: Table,
}

/// Reads what Codex keeps in its home folder, in the plugins of its cache, and in each project it is given: MCP
/// servers, plugins, skills, instructions and memories. Agents and hooks are counted and reported as not running here.
/// Nothing is written, and `auth.json` is never opened.
pub fn scan_codex(fs: &GuardedFs, dirs: &SourceDirs, project_paths: &[PathBuf]) -> ScanResult {
    let mut result = ScanResult::default();
    let guard = fs.with_config_roots(std::slice::from_ref(&dirs.codex));
    let fs = &guard;
    let config = read_config(fs, &dirs.codex.join("config.toml"), &mut result);
    let mut disabled = Vec::new();
    if let Some(config) = &config {
        if let Some(servers) = servers_table(&config.table) {
            codex_servers(servers, None, &config.file, &mut result);
        }
        disabled = disabled_skills(&config.table);
        plugins(fs, dirs, &enabled_plugins(&config.table), &mut result);
    }
    skill_folders(fs, &dirs.codex.join("skills"), Origin::Codex, &disabled, &mut result);
    skill_folders(
        fs,
        &dirs.home.join(".agents").join("skills"),
        Origin::Codex,
        &disabled,
        &mut result,
    );
    instructions(fs, &dirs.codex, &mut result);
    for name in MEMORY_FILES {
        let file = dirs.codex.join("memories").join(name);
        text_item(fs, Origin::Codex, FoundKind::Memory, &file, &mut result);
    }
    counted_files(
        fs,
        &dirs.codex.join("agents"),
        "toml",
        Origin::Codex,
        FoundKind::Agent,
        AGENTS_REASON,
        &mut result,
    );
    hooks(fs, dirs, config.as_ref(), &mut result);
    project_configs(fs, project_paths, &mut result);
    stamp_modified(fs, &mut result.found);
    result
}

fn read_config(fs: &GuardedFs, file: &Path, result: &mut ScanResult) -> Option<Config> {
    let text = match fs.read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.is_missing() => return None,
        Err(error) => {
            result.problems.push(problem(file, error.to_string()));
            return None;
        }
    };
    match toml::from_str::<Table>(&text) {
        Ok(table) => Some(Config {
            file: file.to_path_buf(),
            hash: text_hash(&text),
            table,
        }),
        // The message only: the parser's snippet of the line could show a value the file holds.
        Err(error) => {
            result.problems.push(problem(
                file,
                format!("invalid TOML in {}: {}", file_name(file), error.message()),
            ));
            None
        }
    }
}

fn servers_table(table: &Table) -> Option<&Table> {
    table.get("mcp_servers").and_then(|value| value.as_table())
}

/// The MCP servers of a `[mcp_servers]` table, in the Codex form.
fn codex_servers(servers: &Table, project: Option<ProjectRef>, source: &Path, result: &mut ScanResult) {
    match parse_codex_mcp(servers) {
        Ok(file) => mcp_items(Origin::Codex, file, project, source, result),
        Err(error) => result.problems.push(problem(source, error.to_string())),
    }
}

/// The plugins that `[plugins."name@market"]` turns on.
fn enabled_plugins(table: &Table) -> Vec<String> {
    table
        .get("plugins")
        .and_then(|value| value.as_table())
        .into_iter()
        .flatten()
        .filter(|(_, entry)| {
            entry
                .as_table()
                .and_then(|entry| entry.get("enabled"))
                .and_then(|value| value.as_bool())
                == Some(true)
        })
        .map(|(key, _)| key.clone())
        .collect()
}

/// The skill paths that `[[skills.config]]` turns off.
fn disabled_skills(table: &Table) -> Vec<PathBuf> {
    table
        .get("skills")
        .and_then(|value| value.as_table())
        .and_then(|skills| skills.get("config"))
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.as_table())
        .filter(|entry| entry.get("enabled").and_then(|value| value.as_bool()) == Some(false))
        .filter_map(|entry| entry.get("path").and_then(|value| value.as_str()))
        .map(PathBuf::from)
        .collect()
}

/// The enabled plugins, each read from the newest version in the plugin cache. A name that is not a plain
/// `plugin@market` pair, or a plugin that is not cached, is a problem.
fn plugins(fs: &GuardedFs, dirs: &SourceDirs, enabled: &[String], result: &mut ScanResult) {
    let cache = dirs.codex.join("plugins").join("cache");
    for key in enabled {
        let Some((plugin, market)) = key.rsplit_once('@') else {
            result
                .problems
                .push(problem(&cache, format!("{key} is not a plugin@market name")));
            continue;
        };
        if !is_plain(plugin) || !is_plain(market) {
            result
                .problems
                .push(problem(&cache, format!("{key} is not a plugin@market name")));
            continue;
        }
        let folder = cache.join(market).join(plugin);
        let versions = child_folders(fs, &folder, result);
        let Some(version) = active_version(fs, versions) else {
            result.problems.push(problem(
                &folder,
                format!("{key} is enabled but not in the plugin cache"),
            ));
            continue;
        };
        load_plugin(fs, Origin::Codex, key, &version, None, result);
    }
}

/// The version Codex runs: the newest folder of the plugin in the cache.
fn active_version(fs: &GuardedFs, versions: Vec<PathBuf>) -> Option<PathBuf> {
    versions
        .into_iter()
        .max_by_key(|version| fs.metadata(version).ok().and_then(|metadata| metadata.modified().ok()))
}

/// A name from `config.toml` that stays inside the cache folder.
fn is_plain(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', ':'])
}

/// `AGENTS.override.md` replaces `AGENTS.md` when it is there, as Codex reads it.
fn instructions(fs: &GuardedFs, dir: &Path, result: &mut ScanResult) {
    let override_file = dir.join("AGENTS.override.md");
    let file = if exists(fs, &override_file, result) {
        override_file
    } else {
        dir.join("AGENTS.md")
    };
    text_item(fs, Origin::Codex, FoundKind::Instructions, &file, result);
}

/// Hooks are counted and not run: `hooks.json`, and a `[hooks]` table in `config.toml`.
fn hooks(fs: &GuardedFs, dirs: &SourceDirs, config: Option<&Config>, result: &mut ScanResult) {
    let file = dirs.codex.join("hooks.json");
    match fs.read_to_string(&file) {
        Ok(text) => result.found.push(Found {
            unsupported: Some(HOOKS_REASON.to_string()),
            ..found(Origin::Codex, FoundKind::Hook, "hooks.json", &file, &text_hash(&text))
        }),
        Err(error) if error.is_missing() => {}
        Err(error) => result.problems.push(problem(&file, error.to_string())),
    }
    if let Some(config) = config.filter(|config| config.table.contains_key("hooks")) {
        result.found.push(Found {
            unsupported: Some(HOOKS_REASON.to_string()),
            ..found(
                Origin::Codex,
                FoundKind::Hook,
                "config.toml hooks",
                &config.file,
                &config.hash,
            )
        });
    }
}

/// The projects this scan is for: each project's own `.codex/config.toml`, with its MCP servers tagged by project.
fn project_configs(fs: &GuardedFs, project_paths: &[PathBuf], result: &mut ScanResult) {
    for (index, project) in project_paths.iter().enumerate() {
        let file = project.join(".codex").join("config.toml");
        if !exists(fs, &file, result) {
            continue;
        }
        let Some(config) = read_config(fs, &file, result) else {
            continue;
        };
        if let Some(servers) = servers_table(&config.table) {
            codex_servers(servers, Some(ProjectRef(index)), &config.file, result);
        }
    }
}
