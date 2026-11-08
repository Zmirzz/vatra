//! Reads what Claude Code and Codex keep on this machine, for an import. The app and the CLI both use these functions,
//! so they show and apply the same items.

use std::path::{Path, PathBuf};

use serde_json::Value;
use vatra_core::Store;
use vatra_import::{
    GuardedFs, ProjectRef, ScanResult, SourceDirs, scan_claude, scan_codex, scan_project,
};

/// The folders an import reads for projects, each with its catalog project. The projects Vatra knows that have a
/// folder come first; then the projects Claude Code knows that Vatra doesn't, with no catalog project. Those are
/// listed in the import, and never applied.
pub fn project_folders(
    store: &Store,
    dirs: &SourceDirs,
) -> Result<Vec<(PathBuf, Option<i64>)>, String> {
    let mut folders: Vec<(PathBuf, Option<i64>)> = store
        .list_projects()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter_map(|project| {
            project
                .path
                .map(|path| (PathBuf::from(path), Some(project.id)))
        })
        .collect();
    for path in claude_projects(dirs) {
        if !folders.iter().any(|(known, _)| same_folder(known, &path)) {
            folders.push((path, None));
        }
    }
    Ok(folders)
}

/// The scan of every folder, and the catalog project of each folder by position. Items of a folder carry its
/// position as their `ProjectRef`.
pub fn scan_folders(
    fs: &GuardedFs,
    dirs: &SourceDirs,
    folders: &[(PathBuf, Option<i64>)],
) -> (ScanResult, Vec<Option<i64>>) {
    let paths: Vec<PathBuf> = folders.iter().map(|(path, _)| path.clone()).collect();
    let mut scan = scan_claude(fs, dirs, &paths);
    merge(&mut scan, scan_codex(fs, dirs, &paths));
    for (index, path) in paths.iter().enumerate() {
        merge(&mut scan, scan_project(fs, path, ProjectRef(index)));
    }
    for item in &mut scan.found {
        if item.kind == vatra_import::FoundKind::Memory
            && item.project.is_some_and(|ProjectRef(index)| {
                folders.get(index).is_none_or(|(_, id)| id.is_none())
            })
        {
            item.project = None;
            item.details["project_known"] = false.into();
        }
    }
    let projects = folders.iter().map(|(_, project)| *project).collect();
    (scan, projects)
}

fn merge(into: &mut ScanResult, other: ScanResult) {
    into.found.extend(other.found);
    into.problems.extend(other.problems);
}

/// The project folders in `.claude.json`: the keys of its `projects` object. Only the keys are read.
fn claude_projects(dirs: &SourceDirs) -> Vec<PathBuf> {
    let fs = GuardedFs::new();
    let in_config = dirs.claude.join(".claude.json");
    let file = if fs.metadata(&in_config).is_ok() {
        in_config
    } else {
        dirs.home.join(".claude.json")
    };
    let Ok(text) = fs.read_to_string(&file) else {
        return Vec::new();
    };
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    root.get("projects")
        .and_then(Value::as_object)
        .map(|projects| projects.keys().map(PathBuf::from).collect())
        .unwrap_or_default()
}

/// Whether two folder paths name the same folder, as Windows compares them: no case, one separator.
fn same_folder(a: &Path, b: &Path) -> bool {
    normal(a) == normal(b)
}

fn normal(path: &Path) -> String {
    let text = path.display().to_string().replace('/', "\\");
    let text = text.trim_end_matches('\\');
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text.to_string()
    }
}
