//! Copies curated context into Vatra; native folders are always read through the guard.
use crate::scan::{found, problem, text_hash};
use crate::{Found, FoundKind, GuardedFs, ProjectRef, ScanResult};
use std::{fs, io, path::Path};
use vatra_core::Project;
use vatra_extensions::Origin;

pub fn claude_project_key(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn normalized(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

pub fn map_project(native_dir_or_cwd: &str, projects: &[Project]) -> Option<i64> {
    projects
        .iter()
        .find(|project| {
            project.path.as_ref().is_some_and(|path| {
                normalized(path) == normalized(native_dir_or_cwd)
                    || claude_project_key(Path::new(path)).eq_ignore_ascii_case(native_dir_or_cwd)
            })
        })
        .map(|project| project.id)
}

/// Adds a provenance-marked note or merges an index, preserving existing notes and avoiding duplicate reimports.
pub fn import_memory(item: &Found, project_memory: &Path) -> io::Result<()> {
    let text = GuardedFs::new()
        .read_to_string(&item.source)
        .map_err(io::Error::other)?;
    if text_hash(&text) != item.hash {
        return Err(io::Error::other("memory changed since the scan"));
    }
    merge_memory(
        &text,
        item.origin,
        &item.source.to_string_lossy(),
        project_memory,
    )
}

/// Copies an immutable catalog snapshot into the writable memory folder.
pub fn merge_memory(
    text: &str,
    origin: Origin,
    source: &str,
    project_memory: &Path,
) -> io::Result<()> {
    fs::create_dir_all(project_memory)?;
    let origin = serde_json::to_value(origin)
        .unwrap_or_default()
        .as_str()
        .unwrap_or("imported")
        .to_owned();
    let hash = text_hash(&format!("{origin}:{source}:{text}"));
    let marker = format!("<!-- imported-memory:{hash} -->");
    let index_path = project_memory.join("MEMORY.md");
    let mut index = match fs::read_to_string(&index_path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    if index.contains(&marker) {
        return Ok(());
    }
    let header = format!("{marker}\nImported from {origin}: {source}\n");
    let name = source.rsplit(['/', '\\']).next().unwrap_or("memory.md");
    if name.eq_ignore_ascii_case("MEMORY.md") || name.eq_ignore_ascii_case("memory_summary.md") {
        index.push_str(&format!("\n{header}\n{text}\n"));
    } else {
        let stem: String = name
            .chars()
            .filter(|c| c.is_alphanumeric() || ['-', '_', '.'].contains(c))
            .collect();
        let file = format!("{origin}-{}-{stem}", &hash[..12]);
        fs::write(project_memory.join(&file), format!("{header}\n{text}\n"))?;
        index.push_str(&format!("\n{marker}\n- [{name}]({file}) (from {origin})\n"));
    }
    // Publish a complete index in one rename, including on Windows where replacement needs an existing target removed.
    // A same-folder write avoids exposing a half-written index to an agent.
    let temp = project_memory.join(format!(".MEMORY-{hash}.tmp"));
    fs::write(&temp, index)?;
    if let Err(error) = fs::rename(&temp, &index_path) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}

pub(crate) fn scan_claude_context(
    fs: &GuardedFs,
    root: &Path,
    paths: &[std::path::PathBuf],
    result: &mut ScanResult,
) {
    crate::scan::text_item(
        fs,
        Origin::Claude,
        FoundKind::Instructions,
        &root.join("CLAUDE.md"),
        result,
    );
    for dir in crate::scan::child_folders(fs, &root.join("projects"), result) {
        let key = dir.file_name().unwrap_or_default().to_string_lossy();
        let project = paths
            .iter()
            .position(|path| claude_project_key(path).eq_ignore_ascii_case(&key))
            .map(ProjectRef);
        let memory = dir.join("memory");
        let files = match fs.walk(&memory) {
            Ok(walk) => walk.files,
            Err(error) if error.is_missing() => continue,
            Err(error) => {
                result.problems.push(problem(&memory, error.to_string()));
                continue;
            }
        };
        for file in files
            .into_iter()
            .filter(|file| file.extension().is_some_and(|ext| ext == "md"))
        {
            match fs.read_to_string(&file) {
                Ok(text) => {
                    let mut item = found(
                        Origin::Claude,
                        FoundKind::Memory,
                        format!(
                            "{key}/{}",
                            file.file_name().unwrap_or_default().to_string_lossy()
                        ),
                        &file,
                        &text_hash(&text),
                    );
                    item.project = project;
                    item.details = serde_json::json!({"native_project":key, "project_known":project.is_some()});
                    if token_like(&text) {
                        item.details["warning"] =
                            "This note may contain a secret. Review its source before importing."
                                .into();
                    }
                    result.found.push(item);
                }
                Err(error) => result.problems.push(problem(&file, error.to_string())),
            }
        }
    }
}

pub fn token_like(text: &str) -> bool {
    text.split_whitespace()
        .any(|word| word.trim_start_matches(['\'', '"', '`']).starts_with("sk-") && word.len() > 12)
}
