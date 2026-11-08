//! What the message box offers after `/`, `$` and `@`: skills, MCP prompts, project files and MCP resources.
//! The text that a pick inserts is only a suggestion, the engine expands it when the turn runs.

use std::path::Path;
use std::sync::Arc;

use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use vatra_engine::McpHub;
use vatra_extensions::Skill;

use crate::service::Service;

/// How many items one request returns. The box shows a short list, so more would only be scrolled past.
const MAX_ITEMS: usize = 50;
/// How many files one request looks at, so a huge folder can't hold the service for long.
const MAX_SCANNED: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Trigger {
    #[serde(rename = "/")]
    Slash,
    #[serde(rename = "$")]
    Dollar,
    #[serde(rename = "@")]
    At,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CompletionItem {
    pub label: String,
    pub insert: String,
    pub detail: String,
    pub kind: &'static str,
}

/// The items for one trigger of a project. A query only filters: it never names a path, so a file can only be
/// listed from inside the project folder. Files come only from a project that has a folder. MCP servers are never
/// waited for: their prompts and resources are listed as far as they are already connected.
pub async fn complete(
    service: &Arc<Service>,
    project_id: i64,
    trigger: Trigger,
    query: &str,
) -> Result<Vec<CompletionItem>, String> {
    let needle = query.to_lowercase();
    let folder = project_folder(service, project_id);
    let mut items = match trigger {
        Trigger::Slash | Trigger::Dollar => skill_items(service, project_id, trigger, &needle)?,
        Trigger::At => match &folder {
            Some(root) => file_items(root, &needle).await?,
            None => Vec::new(),
        },
    };
    if trigger == Trigger::Slash {
        items.extend(
            service
                .agents()
                .commands(project_id)
                .into_iter()
                .filter_map(|command| {
                    let name = command.get("name")?.as_str()?;
                    if !name.to_lowercase().contains(&needle) {
                        return None;
                    }
                    Some(CompletionItem {
                        label: format!("/{name}"),
                        insert: format!("/{name} "),
                        detail: command
                            .get("description")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .into(),
                        kind: "command",
                    })
                }),
        );
    }
    if let Some(hub) = mcp_hub(service, project_id, folder.as_deref().map(Path::new)) {
        match trigger {
            Trigger::Slash => items.extend(prompt_items(&hub, &needle)),
            Trigger::At => items.extend(resource_items(&hub, &needle)),
            Trigger::Dollar => {}
        }
    }
    items.truncate(MAX_ITEMS);
    Ok(items)
}

fn skill_items(
    service: &Arc<Service>,
    project_id: i64,
    trigger: Trigger,
    needle: &str,
) -> Result<Vec<CompletionItem>, String> {
    let Some(catalog) = service.agents().catalog() else {
        return Ok(Vec::new());
    };
    let active = catalog
        .active_for(project_id)
        .map_err(|error| error.to_string())?;
    let prefix = if trigger == Trigger::Slash { "/" } else { "$" };
    let mut skills: Vec<&Skill> = active
        .skills
        .iter()
        .filter(|skill| skill.user_invocable)
        .collect();
    skills.sort_by_key(|skill| skill.qualified_name());
    Ok(skills
        .into_iter()
        .map(|skill| (skill.qualified_name(), skill))
        .filter(|(label, _)| label.to_lowercase().contains(needle))
        .map(|(label, skill)| CompletionItem {
            insert: format!("{prefix}{label}"),
            label,
            detail: skill.description.clone(),
            kind: if skill.is_command { "command" } else { "skill" },
        })
        .collect())
}

/// The hub of the project's MCP servers. The first request for a project also starts the servers in the background,
/// so the list fills in on a later request, and this request never waits for them.
fn mcp_hub(service: &Arc<Service>, project_id: i64, folder: Option<&Path>) -> Option<Arc<McpHub>> {
    let hub = service.agents().mcp_hub(project_id, folder)?;
    service.warm_up(project_id, Arc::clone(&hub));
    Some(hub)
}

fn prompt_items(hub: &McpHub, needle: &str) -> Vec<CompletionItem> {
    hub.prompts()
        .into_iter()
        .filter(|prompt| prompt.name.to_lowercase().contains(needle))
        .map(|prompt| CompletionItem {
            insert: format!("/{}", prompt.name),
            label: prompt.name,
            detail: prompt.description,
            kind: "prompt",
        })
        .collect()
}

fn resource_items(hub: &McpHub, needle: &str) -> Vec<CompletionItem> {
    hub.resources()
        .into_iter()
        .map(|resource| (format!("{}:{}", resource.server, resource.uri), resource))
        .filter(|(label, _)| label.to_lowercase().contains(needle))
        .map(|(label, resource)| CompletionItem {
            insert: format!("@{label}"),
            label,
            detail: resource.name,
            kind: "resource",
        })
        .collect()
}

async fn file_items(root: &str, needle: &str) -> Result<Vec<CompletionItem>, String> {
    let root = root.to_string();
    let needle = needle.to_string();
    tokio::task::spawn_blocking(move || project_files(Path::new(&root), &needle))
        .await
        .map_err(|error| error.to_string())
}

/// The files under the folder that match, with `/` separators and paths relative to it. The walk follows the
/// gitignore rules of the folder only, never leaves it (it doesn't follow links), and goes in a fixed order, so the
/// first items are the same on every request.
fn project_files(root: &Path, needle: &str) -> Vec<CompletionItem> {
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .parents(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    let mut items = Vec::new();
    for entry in walker.flatten().take(MAX_SCANNED) {
        if items.len() >= MAX_ITEMS {
            break;
        }
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        let label = relative.to_string_lossy().replace('\\', "/");
        if !label.to_lowercase().contains(needle) {
            continue;
        }
        items.push(CompletionItem {
            insert: format!("@{label}"),
            label,
            detail: String::new(),
            kind: "file",
        });
    }
    items
}

fn project_folder(service: &Arc<Service>, project_id: i64) -> Option<String> {
    let projects = service.store().list_projects().ok()?;
    projects
        .into_iter()
        .find(|project| project.id == project_id)?
        .path
}
