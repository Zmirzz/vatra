use std::path::Path;

use vatra_extensions::{Origin, parse_mcp_json};

use crate::guard::GuardedFs;
use crate::scan::{
    FoundKind, ProjectRef, ScanResult, child_folders, permission_file, command_files, file_name, is_clear, mcp_items, problem,
    skill_folders, stamp_modified, text_item,
};

/// Folders that are never searched for nested skills: dependencies, build output and hidden folders.
const SKIPPED_FOLDERS: [&str; 4] = ["node_modules", "target", "dist", "build"];

/// How many folders below the project are searched for `.claude/skills`, such as the packages of a monorepo.
const NESTED_DEPTH: usize = 2;

/// Reads one project folder: its MCP servers, instructions, skills and commands. Nothing outside the folder is read,
/// and a folder that is not there gives nothing at all. Each item carries `project`.
pub fn scan_project(fs: &GuardedFs, path: &Path, project: ProjectRef) -> ScanResult {
    let mut result = ScanResult::default();
    if !fs.metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
        return result;
    }
    let guard = fs.with_config_roots(&[path.join(".claude"), path.join(".codex")]);
    let fs = &guard;
    mcp_file(fs, &path.join(".mcp.json"), &mut result);
    let settings = path.join(".claude");
    permission_file(fs, &settings.join("settings.json"), Origin::Project, &mut result);
    permission_file(fs, &settings.join("settings.local.json"), Origin::Project, &mut result);
    text_item(
        fs,
        Origin::Project,
        FoundKind::Instructions,
        &path.join("AGENTS.md"),
        &mut result,
    );
    text_item(
        fs,
        Origin::Project,
        FoundKind::Instructions,
        &path.join("CLAUDE.md"),
        &mut result,
    );
    skill_folders(
        fs,
        &path.join(".claude").join("skills"),
        Origin::Project,
        &[],
        &mut result,
    );
    skill_folders(
        fs,
        &path.join(".agents").join("skills"),
        Origin::Project,
        &[],
        &mut result,
    );
    let commands = path.join(".claude").join("commands");
    if is_clear(fs, &commands, &mut result) {
        command_files(fs, &commands, Origin::Project, &[], &mut result);
    }
    nested_skills(fs, path, NESTED_DEPTH, &mut result);
    for item in &mut result.found {
        item.project = Some(project);
    }
    stamp_modified(fs, &mut result.found);
    result
}

/// The `.mcp.json` of the project. A missing file is nothing.
fn mcp_file(fs: &GuardedFs, file: &Path, result: &mut ScanResult) {
    let text = match fs.read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.is_missing() => return,
        Err(error) => {
            result.problems.push(problem(file, error.to_string()));
            return;
        }
    };
    match parse_mcp_json(&text) {
        Ok(parsed) => mcp_items(Origin::Project, parsed, None, file, result),
        Err(error) => result.problems.push(problem(file, error.to_string())),
    }
}

/// The `.claude/skills` folders of subfolders, down to `depth` levels, so a monorepo's packages keep their own skills.
fn nested_skills(fs: &GuardedFs, root: &Path, depth: usize, result: &mut ScanResult) {
    if depth == 0 {
        return;
    }
    for folder in child_folders(fs, root, result) {
        if is_skipped(&folder) {
            continue;
        }
        skill_folders(fs, &folder.join(".claude").join("skills"), Origin::Project, &[], result);
        nested_skills(fs, &folder, depth - 1, result);
    }
}

fn is_skipped(folder: &Path) -> bool {
    let name = file_name(folder);
    name.starts_with('.') || SKIPPED_FOLDERS.contains(&name.as_str())
}
