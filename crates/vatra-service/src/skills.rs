//! Local skill sources are references, never writable targets. Installation copies
//! a reviewed skill through the shared catalog, leaving executable content unapproved.
use crate::Service;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use vatra_extensions::{
    Catalog, ElementKind, Origin, Scope, content_hash, parse_skill, validate_skill_standard,
};

const SOURCES: &str = "skills.sources";
const MAX_SKILLS: usize = 1000;
const MAX_FILE: u64 = 512 * 1024;
static WRITES: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillSource {
    pub path: PathBuf,
    pub project_id: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredSkill {
    pub path: PathBuf,
    pub project_id: Option<i64>,
    pub name: String,
    pub description: String,
    pub resources: Vec<String>,
    pub errors: Vec<String>,
    pub hash: Option<String>,
}

fn resources(
    root: &Path,
    dir: &Path,
    depth: usize,
    files: &mut Vec<String>,
    size: &mut u64,
) -> Result<(), String> {
    if depth > 8 {
        return Err("Skill resources exceed eight directory levels.".into());
    }
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            continue;
        }
        if [".git", "node_modules"]
            .iter()
            .any(|name| entry.file_name() == *name)
        {
            continue;
        }
        let path = entry.path();
        if !dunce::canonicalize(&path)
            .map_err(|e| e.to_string())?
            .starts_with(root)
        {
            return Err("A resource escapes its skill directory.".into());
        }
        if kind.is_dir() {
            resources(root, &path, depth + 1, files, size)?;
        } else if kind.is_file() {
            *size = size.saturating_add(entry.metadata().map_err(|e| e.to_string())?.len());
            if files.len() >= 1024 || *size > 64 * 1024 * 1024 {
                return Err("Skill resources are too large (1024 files / 64 MiB maximum).".into());
            }
            files.push(
                path.strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

fn inspect(path: PathBuf, project_id: Option<i64>) -> DiscoveredSkill {
    let mut item = DiscoveredSkill {
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        path,
        project_id,
        description: String::new(),
        resources: vec![],
        errors: vec![],
        hash: None,
    };
    let file = item.path.join("SKILL.md");
    match std::fs::symlink_metadata(&file) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= MAX_FILE => {}
        _ => {
            item.errors
                .push("SKILL.md is missing, linked, or too large (512 KiB maximum).".into());
            return item;
        }
    }
    match parse_skill(&item.path) {
        Ok(skill) => {
            item.name = skill.name;
            item.description = skill.description;
            item.errors.extend(skill.problems);
        }
        Err(error) => item.errors.push(error.to_string()),
    }
    match validate_skill_standard(&item.path) {
        Ok(errors) => item.errors.extend(errors),
        Err(error) => item.errors.push(error.to_string()),
    }
    if let Err(error) = resources(&item.path, &item.path, 0, &mut item.resources, &mut 0) {
        item.errors.push(error);
    }
    item.resources.sort();
    if item.errors.is_empty() {
        match content_hash(&item.path) {
            Ok(hash) => item.hash = Some(hash),
            Err(error) => item.errors.push(error.to_string()),
        }
    }
    item
}

pub fn scan_sources(sources: &[SkillSource]) -> Result<Vec<DiscoveredSkill>, String> {
    if sources.len() > 64 {
        return Err("Use at most 64 skill source directories.".into());
    }
    let mut found = Vec::new();
    for source in sources {
        let root = dunce::canonicalize(&source.path).map_err(|error| {
            format!(
                "Cannot read skill source {}: {error}",
                source.path.display()
            )
        })?;
        let mut folders = Vec::new();
        if root.join("SKILL.md").exists() {
            folders.push(root.clone());
        } else {
            for (index, entry) in std::fs::read_dir(&root)
                .map_err(|e| e.to_string())?
                .enumerate()
            {
                if index >= 4096 {
                    return Err("A skill source may contain at most 4096 directory entries.".into());
                }
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_type().map_err(|e| e.to_string())?.is_dir()
                    && entry.path().join("SKILL.md").exists()
                {
                    let path = dunce::canonicalize(entry.path()).map_err(|e| e.to_string())?;
                    if path.starts_with(&root) {
                        folders.push(path);
                    }
                }
                if folders.len() > MAX_SKILLS {
                    return Err("Too many skills in this source.".into());
                }
            }
        }
        folders.sort();
        for path in folders {
            if found.len() >= MAX_SKILLS {
                return Err("Use at most 1000 skills across all sources.".into());
            }
            found.push(inspect(path, source.project_id));
        }
    }
    let mut counts = HashMap::new();
    for item in &found {
        *counts
            .entry((item.project_id, item.name.clone()))
            .or_insert(0) += 1;
    }
    for item in &mut found {
        if counts[&(item.project_id, item.name.clone())] > 1 {
            item.errors.push(
                "Duplicate skill name in this scope; choose one source before installing.".into(),
            );
        }
    }
    Ok(found)
}

fn sources(service: &Service) -> Result<Vec<SkillSource>, String> {
    serde_json::from_str(
        &service
            .store()
            .app_setting(SOURCES)
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| "[]".into()),
    )
    .map_err(|e| e.to_string())
}

/// Native desktop opening is restricted to a discovered directory or its exact
/// SKILL.md. No general file-opening permission is granted to the webview.
pub fn source_target(
    data_dir: &Path,
    path: &Path,
    project_id: Option<i64>,
    folder: bool,
) -> Result<PathBuf, String> {
    let store =
        vatra_core::Store::open_read_only(&data_dir.join("vatra.db")).map_err(|e| e.to_string())?;
    let roots: Vec<SkillSource> = serde_json::from_str(
        &store
            .app_setting(SOURCES)
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| "[]".into()),
    )
    .map_err(|e| e.to_string())?;
    let item = scan_sources(&roots)?
        .into_iter()
        .find(|item| item.path == path && item.project_id == project_id)
        .ok_or("Not a configured skill directory.")?;
    if folder {
        return Ok(item.path);
    }
    let file = item.path.join("SKILL.md");
    let metadata = std::fs::symlink_metadata(&file).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Cannot open a linked SKILL.md.".into());
    }
    let file = dunce::canonicalize(file).map_err(|e| e.to_string())?;
    if !file.starts_with(&item.path) {
        return Err("Skill file escapes its directory.".into());
    }
    Ok(file)
}

fn catalog(service: &Service) -> Result<Arc<Catalog>, String> {
    service
        .agents()
        .catalog()
        .ok_or("Extension catalog is unavailable.".into())
}

fn scope(project: Option<i64>) -> Scope {
    project.map_or(Scope::Global, Scope::Project)
}

/// Only the authenticated UI calls this surface. No command execution happens here.
pub fn handle(service: &Arc<Service>, method: &str, params: Value) -> Result<Value, (i64, String)> {
    let run = || -> Result<Value, String> {
        let _guard = WRITES
            .lock()
            .map_err(|_| "Skill configuration is unavailable.")?;
        match method {
            "skills.sources" => Ok(json!(sources(service)?)),
            "skills.scan" => Ok(json!(scan_sources(&sources(service)?)?)),
            "skills.set_sources" => {
                let mut list: Vec<SkillSource> =
                    serde_json::from_value(params).map_err(|e| e.to_string())?;
                for source in &mut list {
                    if !source.path.is_absolute() {
                        return Err("Skill source paths must be absolute.".into());
                    }
                    source.path = dunce::canonicalize(&source.path).map_err(|e| e.to_string())?;
                    if let Some(id) = source.project_id
                        && !service
                            .store()
                            .list_projects()
                            .map_err(|e| e.to_string())?
                            .iter()
                            .any(|project| project.id == id)
                    {
                        return Err("Unknown skill project.".into());
                    }
                }
                let items = scan_sources(&list)?;
                service
                    .store()
                    .set_app_setting(
                        SOURCES,
                        Some(&serde_json::to_string(&list).map_err(|e| e.to_string())?),
                    )
                    .map_err(|e| e.to_string())?;
                Ok(json!(items))
            }
            "skills.inspect" | "skills.install" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct Request {
                    path: PathBuf,
                    project_id: Option<i64>,
                    expected_hash: Option<String>,
                    replace_hash: Option<String>,
                }
                let request: Request = serde_json::from_value(params).map_err(|e| e.to_string())?;
                let item = scan_sources(&sources(service)?)?
                    .into_iter()
                    .find(|item| item.path == request.path && item.project_id == request.project_id)
                    .ok_or("Skill is not in a configured source. Reload the source list.")?;
                if method == "skills.inspect" {
                    let file = item.path.join("SKILL.md");
                    let metadata = std::fs::symlink_metadata(&file).map_err(|e| e.to_string())?;
                    if !metadata.is_file() || metadata.file_type().is_symlink() {
                        return Err("Cannot inspect a linked SKILL.md file.".into());
                    }
                    let mut bytes = Vec::new();
                    std::fs::File::open(item.path.join("SKILL.md"))
                        .map_err(|e| e.to_string())?
                        .take(MAX_FILE + 1)
                        .read_to_end(&mut bytes)
                        .map_err(|e| e.to_string())?;
                    if bytes.len() as u64 > MAX_FILE {
                        return Err("SKILL.md is too large to inspect.".into());
                    }
                    return Ok(
                        json!({"metadata":item,"content":String::from_utf8(bytes).map_err(|_|"SKILL.md must be UTF-8.")?}),
                    );
                }
                if !item.errors.is_empty() {
                    return Err(item.errors.join("\n"));
                }
                if item.hash.is_none() || item.hash != request.expected_hash {
                    return Err("Skill changed since review. Reload and inspect it again.".into());
                }
                let catalog = catalog(service)?;
                let existing =
                    catalog
                        .list()
                        .map_err(|e| e.to_string())?
                        .into_iter()
                        .find(|entry| {
                            entry.kind == ElementKind::Skill
                                && entry.origin == Origin::Manual
                                && entry.scope == scope(item.project_id)
                                && entry.plugin.is_none()
                                && entry.name == item.name
                        });
                if existing
                    .as_ref()
                    .is_some_and(|entry| Some(&entry.content_hash) != request.replace_hash.as_ref())
                {
                    return Err("A skill with this name is already installed. Review its current hash before replacing it.".into());
                }
                service
                    .store()
                    .record_audit(&vatra_core::AuditEntry {
                        at: crate::models::now(),
                        session_id: None,
                        tool: "SkillInstall".into(),
                        connector: "skills".into(),
                        summary: format!("Install {}", item.name),
                        decision: "approved".into(),
                        payload_hash: item.hash.clone().unwrap_or_default(),
                    })
                    .map_err(|e| e.to_string())?;
                let report = catalog
                    .add_folder_checked(
                        &item.path,
                        Origin::Manual,
                        scope(item.project_id),
                        item.hash.as_deref().ok_or("Missing reviewed hash.")?,
                    )
                    .map_err(|e| e.to_string())?;
                service.agents().catalog_changed(item.project_id);
                Ok(json!({"problems":report.problems,"unapproved":report.unapproved}))
            }
            "skills.uninstall" => {
                let id = params["id"].as_i64().ok_or("Skill id is required.")?;
                let catalog = catalog(service)?;
                let item = catalog
                    .list()
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .find(|entry| entry.id == id && entry.kind == ElementKind::Skill)
                    .ok_or("Unknown installed skill.")?;
                service
                    .store()
                    .record_audit(&vatra_core::AuditEntry {
                        at: crate::models::now(),
                        session_id: None,
                        tool: "SkillUninstall".into(),
                        connector: "skills".into(),
                        summary: format!("Uninstall {}", item.name),
                        decision: "approved".into(),
                        payload_hash: item.content_hash.clone(),
                    })
                    .map_err(|e| e.to_string())?;
                catalog.remove_entry(id).map_err(|e| e.to_string())?;
                service.agents().catalog_changed(match item.scope {
                    Scope::Global => None,
                    Scope::Project(id) => Some(id),
                });
                Ok(json!({}))
            }
            _ => Err("Unknown skills operation.".into()),
        }
    };
    run().map_err(|message| (-32000, message))
}
