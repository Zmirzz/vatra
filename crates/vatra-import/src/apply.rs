//! Applies an import: copies what the user chose into the catalog. A folder is copied into a temporary folder first,
//! with its literal secret values taken out, and the catalog takes it only when the copy is complete. Nothing is
//! written to a source, and a secret value goes only to the secret store.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use vatra_core::{McpServerConfig, SecretStore};
use vatra_extensions::{
    Catalog, CatalogEntry, ElementKind, LiteralSecret, Origin, PutOutcome, Scope, SecretPlace,
};

use crate::dedupe::dedupe;
use crate::scan::{Found, FoundKind, ProjectRef, ScanResult};

/// What the user chose for an import: one choice per item, by its key, and the catalog project of each project the
/// scan was given, by its position. `None` is a project Vatra doesn't know, whose items are listed but not applied.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ImportChoice {
    pub items: Vec<ItemChoice>,
    pub projects: Vec<Option<i64>>,
    /// Every past conversation, whatever its folder. The command line sets it; the app names folders instead.
    pub include_history: bool,
    /// The folders whose past conversations come with them, by `HistoryProject::key`. The app sets it.
    pub history_projects: Vec<String>,
}

/// The choice for one item. `include` copies it; `approve_code` lets the code it runs start; `secrets` says, by each
/// secret's key, whether its value is stored in the keychain; `allow_rules` takes the `allow` rules of a settings file.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ItemChoice {
    pub key: String,
    pub include: bool,
    pub approve_code: bool,
    pub secrets: BTreeMap<String, bool>,
    pub allow_rules: bool,
}

/// What each item did. Items that were not chosen have no result.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ApplyOutcome {
    pub results: Vec<ItemResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ItemResult {
    pub key: String,
    pub name: String,
    /// The catalog project the item was applied in, or None for everywhere.
    pub project: Option<i64>,
    pub state: ItemState,
}

/// What happened to one item. A failed item leaves the catalog as it was before the item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ItemState {
    Added,
    Changed,
    Unchanged,
    Skipped { reason: String },
    Failed { message: String },
}

/// The key that names an item in a choice: what the item is, where it comes from, and the hash of its source as the
/// scan read it. A source that changed after the user looked at it has another key, so the choice no longer matches
/// it and its code is not approved.
pub fn item_key(found: &Found) -> String {
    format!("{}@{}", item_identity(found), found.hash)
}

/// The key of an item without its hash: what the item is and where it comes from.
fn item_identity(found: &Found) -> String {
    format!(
        "{}/{}/{}/{}",
        text_of(found.origin),
        text_of(found.kind),
        found.name,
        found.source.display()
    )
}

/// The identity of a key, without the hash after its last `@`. A hash is hex, so it holds no `@`.
fn identity_of_key(key: &str) -> &str {
    key.rsplit_once('@').map_or(key, |(identity, _)| identity)
}

fn text_of(value: impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The scope an item is applied in: everywhere, or in its catalog project. A project Vatra doesn't know is an error.
pub(crate) fn scope_of(found: &Found, projects: &[Option<i64>]) -> Result<Scope, String> {
    match found.project {
        None => Ok(Scope::Global),
        Some(ProjectRef(index)) => match projects.get(index) {
            Some(Some(id)) => Ok(Scope::Project(*id)),
            _ => Err("its project isn't in Vatra".into()),
        },
    }
}

/// The plugin an item belongs to, by the folder it was read from. A plugin itself belongs to none.
pub(crate) fn plugin_of<'a>(found: &Found, items: &'a [Found]) -> Option<&'a Found> {
    if found.kind == FoundKind::Plugin {
        return None;
    }
    items
        .iter()
        .filter(|item| item.kind == FoundKind::Plugin)
        .find(|plugin| found.source.starts_with(&plugin.source))
}

/// The catalog kind an item is stored as, or None when Vatra does not keep it.
pub(crate) fn catalog_kind(kind: FoundKind) -> Option<ElementKind> {
    match kind {
        FoundKind::Plugin => Some(ElementKind::Plugin),
        FoundKind::Skill => Some(ElementKind::Skill),
        FoundKind::Command => Some(ElementKind::Command),
        FoundKind::McpServer => Some(ElementKind::McpServer),
        FoundKind::Instructions => Some(ElementKind::Instructions),
        FoundKind::Memory => Some(ElementKind::Memory),
        FoundKind::Permission => Some(ElementKind::Permission),
        FoundKind::PluginConfig | FoundKind::Hook | FoundKind::Agent | FoundKind::Unsupported => {
            None
        }
    }
}

/// The hash an item's source has in the catalog. A part of a plugin is stored with its plugin's folder hash, so its
/// own hash, which an MCP server takes from its settings, does not stand for it.
pub(crate) fn source_hash_of<'a>(item: &'a Found, items: &'a [Found]) -> &'a str {
    plugin_of(item, items).map_or(item.hash.as_str(), |plugin| plugin.hash.as_str())
}

/// The name an item has in the catalog. A plugin's part is named without its plugin's prefix, and an MCP server
/// without its plugin's either, since its settings carry the plain name.
pub(crate) fn catalog_name(found: &Found, plugin: Option<&Found>) -> String {
    let name = match found.kind {
        FoundKind::McpServer => found
            .details
            .get("name")
            .and_then(Value::as_str)
            .map_or_else(|| found.name.clone(), str::to_string),
        _ => found.name.clone(),
    };
    match plugin {
        Some(plugin) => name
            .strip_prefix(&format!("{}:", plugin.name))
            .map_or(name.clone(), str::to_string),
        None => name,
    }
}

/// Applies the choice. A test or a caller that needs no progress uses this.
pub fn apply(
    scan: &ScanResult,
    choice: &ImportChoice,
    catalog: &Catalog,
    secrets: &dyn SecretStore,
) -> ApplyOutcome {
    apply_reporting(scan, choice, catalog, secrets, &mut |_, _, _| {})
}

/// Applies the choice, calling `progress` with the items done, the total, and the name of the item being done.
pub fn apply_reporting(
    scan: &ScanResult,
    choice: &ImportChoice,
    catalog: &Catalog,
    secrets: &dyn SecretStore,
    progress: &mut dyn FnMut(usize, usize, &str),
) -> ApplyOutcome {
    let merged = dedupe(scan.found.clone());
    let mut items = merged.items;
    for conflict in merged.conflicts {
        if let Some(alternate) = conflict.dropped.into_iter().find(|item| {
            choice
                .items
                .iter()
                .any(|selected| selected.include && selected.key == item_key(item))
        }) && let Some(kept) = items
            .iter_mut()
            .find(|item| item_key(item) == item_key(&conflict.kept))
        {
            *kept = alternate;
        }
    }
    let chosen: HashMap<&str, &ItemChoice> = choice
        .items
        .iter()
        .map(|item| (identity_of_key(&item.key), item))
        .collect();
    let ctx = Context {
        choice,
        catalog,
        secrets,
        items: &items,
    };

    // Plugins first, so the parts that come with a plugin are known before the rest.
    let mut work: Vec<(&Found, &ItemChoice)> = items
        .iter()
        .filter_map(|item| {
            chosen
                .get(item_identity(item).as_str())
                .map(|choice| (item, *choice))
        })
        .filter(|(_, choice)| choice.include)
        .collect();
    work.sort_by_key(|(item, _)| item.kind != FoundKind::Plugin);

    let total = work.len();
    let mut outcome = ApplyOutcome::default();
    for (done, (item, item_choice)) in work.into_iter().enumerate() {
        progress(done, total, &item.name);
        // The choice names the source as the user saw it. A source that changed since is not applied, so its code is
        // never approved without being looked at again.
        let state = if item_choice.key != item_key(item) {
            ItemState::Skipped {
                reason: "changed since you looked at it; look at it again".into(),
            }
        } else {
            match apply_item(&ctx, item, item_choice) {
                Ok(state) => state,
                Err(message) => ItemState::Failed { message },
            }
        };
        outcome.results.push(ItemResult {
            key: item_key(item),
            name: item.name.clone(),
            project: project_of(item, &choice.projects),
            state,
        });
    }
    progress(total, total, "");
    outcome
}

/// The catalog project an item is applied in: None for everywhere, or for a project Vatra doesn't know.
fn project_of(item: &Found, projects: &[Option<i64>]) -> Option<i64> {
    match scope_of(item, projects) {
        Ok(Scope::Project(id)) => Some(id),
        _ => None,
    }
}

/// The state every apply step shares: the catalog, the secret store, the items of the scan and the choice.
struct Context<'a> {
    choice: &'a ImportChoice,
    catalog: &'a Catalog,
    secrets: &'a dyn SecretStore,
    items: &'a [Found],
}

fn apply_item(ctx: &Context, item: &Found, choice: &ItemChoice) -> Result<ItemState, String> {
    if let Some(reason) = &item.unsupported {
        return Ok(ItemState::Skipped {
            reason: reason.clone(),
        });
    }
    if plugin_of(item, ctx.items).is_some() {
        return Ok(ItemState::Skipped {
            reason: "imported with its plugin".into(),
        });
    }
    let scope = match scope_of(item, &ctx.choice.projects) {
        Ok(scope) => scope,
        Err(reason) => return Ok(ItemState::Skipped { reason }),
    };
    match item.kind {
        FoundKind::Plugin => plugin(ctx, item, choice, scope),
        FoundKind::Skill => {
            let report = ctx
                .catalog
                .add_folder_as(
                    &item.source,
                    item.origin,
                    scope,
                    choice.approve_code,
                    Some(&item.hash),
                )
                .map_err(text)?;
            Ok(state(report.outcomes.first().copied()))
        }
        FoundKind::McpServer => mcp_server(ctx, item, choice, scope),
        FoundKind::Permission => permission(ctx, item, choice, scope),
        FoundKind::Instructions | FoundKind::Memory => file(ctx, item, scope),
        FoundKind::Command => command(ctx, item, choice, scope),
        FoundKind::PluginConfig => Ok(ItemState::Skipped {
            reason: "plugin options aren't imported yet".into(),
        }),
        FoundKind::Hook | FoundKind::Agent | FoundKind::Unsupported => Ok(ItemState::Skipped {
            reason: "Vatra doesn't run this".into(),
        }),
    }
}

/// A plugin folder, with its parts. Its code is approved by the plugin's own choice, and each server's secrets by the
/// choice of that server.
fn plugin(
    ctx: &Context,
    item: &Found,
    choice: &ItemChoice,
    scope: Scope,
) -> Result<ItemState, String> {
    let parts: Vec<&Found> = ctx
        .items
        .iter()
        .filter(|part| part.kind != FoundKind::Plugin && part.source.starts_with(&item.source))
        .collect();
    let literals: Vec<&LiteralSecret> = parts.iter().flat_map(|part| part.secrets.iter()).collect();
    let report = import_folder(
        ctx,
        &item.source,
        item.origin,
        scope,
        choice.approve_code,
        &item.hash,
        &literals,
    )?;
    for part in parts
        .iter()
        .filter(|part| part.kind == FoundKind::McpServer)
    {
        let part_choice = ctx
            .choice
            .items
            .iter()
            .find(|candidate| candidate.key == item_key(part));
        settle_secrets(
            ctx,
            part,
            part_choice,
            item.origin,
            scope,
            Some(item.name.as_str()),
        )?;
    }
    Ok(state(report.outcomes.first().copied()))
}

/// A folder copy. A folder with literal secret values is copied with them taken out, into a temporary folder that is
/// removed afterwards; the catalog keeps the source hash of the real folder.
fn import_folder(
    ctx: &Context,
    folder: &Path,
    origin: Origin,
    scope: Scope,
    approve: bool,
    source_hash: &str,
    literals: &[&LiteralSecret],
) -> Result<vatra_extensions::AddReport, String> {
    if literals.is_empty() {
        return ctx
            .catalog
            .add_folder_as(folder, origin, scope, approve, Some(source_hash))
            .map_err(text);
    }
    let staging = temp_folder()?;
    let result = scrubbed_copy(folder, &staging, literals).and_then(|()| {
        ctx.catalog
            .add_folder_as(&staging, origin, scope, approve, Some(source_hash))
            .map_err(text)
    });
    let _ = fs::remove_dir_all(&staging);
    result
}

/// Copies every readable file of `folder` into `target`, with each literal value replaced by its reference `${KEY}`.
/// The copy is refused when a value is still in it, so no secret value can reach the catalog this way.
fn scrubbed_copy(folder: &Path, target: &Path, literals: &[&LiteralSecret]) -> Result<(), String> {
    let walked = crate::guard::GuardedFs::new().walk(folder).map_err(text)?;
    for file in walked.files {
        let relative = file.strip_prefix(folder).map_err(text)?;
        let to = target.join(relative);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(text)?;
        }
        match fs::read_to_string(&file) {
            Ok(content) => fs::write(&to, scrub_text(&content, literals)).map_err(text)?,
            Err(_) => {
                fs::copy(&file, &to).map_err(text)?;
            }
        }
        if let Ok(copied) = fs::read_to_string(&to) {
            let residue = literals.iter().any(|secret| {
                let value = secret.value();
                !value.is_empty() && (copied.contains(value) || copied.contains(&escaped(value)))
            });
            if residue {
                return Err(format!(
                    "a secret value couldn't be taken out of {}",
                    relative.display()
                ));
            }
        }
    }
    Ok(())
}

fn scrub_text(content: &str, literals: &[&LiteralSecret]) -> String {
    literals
        .iter()
        .fold(content.to_string(), |content, secret| {
            let value = secret.value();
            if value.is_empty() {
                return content;
            }
            let reference = format!("${{{}}}", secret.key);
            content
                .replace(&escaped(value), &reference)
                .replace(value, &reference)
        })
}

/// A value as it reads inside a JSON string, where quotes and backslashes are escaped.
fn escaped(value: &str) -> String {
    serde_json::to_string(value)
        .map(|quoted| quoted[1..quoted.len() - 1].to_string())
        .unwrap_or_else(|_| value.to_string())
}

/// An MCP server that is not part of a plugin. Its literal values become references, and its secrets are settled.
fn mcp_server(
    ctx: &Context,
    item: &Found,
    choice: &ItemChoice,
    scope: Scope,
) -> Result<ItemState, String> {
    let mut server: McpServerConfig = serde_json::from_value(item.details.clone()).map_err(text)?;
    for secret in &item.secrets {
        let reference = format!("${{{}}}", secret.key);
        match secret.place {
            SecretPlace::Env => server.env.insert(secret.key.clone(), reference),
            SecretPlace::Header => server.headers.insert(secret.key.clone(), reference),
            SecretPlace::OAuth => None,
        };
    }
    // A server with secrets waits for them, so it is written held, and only the import's own hold is ever released.
    let hold = item
        .secrets
        .iter()
        .any(|secret| secret.place != SecretPlace::OAuth);
    let outcome = ctx
        .catalog
        .add_server(&server, item.origin, scope, Some(&item.hash), hold)
        .map_err(text)?;
    settle_secrets(ctx, item, Some(choice), item.origin, scope, None)?;
    Ok(state(Some(outcome)))
}

/// Stores the values the user allowed in the keychain, and holds the server while a value is awaited. A hold is the
/// import's own: it is released only when every secret the server needs has a value and the hold is set. A server the
/// user switched off is neither held nor released, so a reimport keeps it off. A failed write returns an error, and the
/// server stays held.
fn settle_secrets(
    ctx: &Context,
    item: &Found,
    choice: Option<&ItemChoice>,
    origin: Origin,
    scope: Scope,
    plugin: Option<&str>,
) -> Result<(), String> {
    let keys: Vec<&LiteralSecret> = item
        .secrets
        .iter()
        .filter(|secret| secret.place != SecretPlace::OAuth)
        .collect();
    if keys.is_empty() {
        return Ok(());
    }
    // A server's settings carry its plain name, so its catalog name needs no plugin prefix removed.
    let name = catalog_name(item, None);
    let entry = lookup_server(ctx.catalog, origin, &name, plugin, scope)?;
    let user_off = !entry.enabled && !ctx.catalog.held_for_secret(entry.id).map_err(text)?;
    let consented: Vec<&LiteralSecret> = keys
        .iter()
        .copied()
        .filter(|secret| choice.is_some_and(|choice| consents(choice, &secret.key)))
        .collect();
    let waiting = !consented.is_empty()
        || keys
            .iter()
            .any(|secret| !has_value(&entry.details, &secret.key));
    if waiting && !user_off {
        ctx.catalog.hold(entry.id).map_err(text)?;
    }
    for secret in consented {
        ctx.catalog
            .set_secret(entry.id, &secret.key, secret.value(), ctx.secrets)
            .map_err(text)?;
    }
    let entry = lookup_server(ctx.catalog, origin, &name, plugin, scope)?;
    let complete = keys
        .iter()
        .all(|secret| has_value(&entry.details, &secret.key));
    if complete && ctx.catalog.held_for_secret(entry.id).map_err(text)? {
        ctx.catalog.release(entry.id).map_err(text)?;
    }
    Ok(())
}

/// The stored server of an item, which must be there once its folder or settings were written.
fn lookup_server(
    catalog: &Catalog,
    origin: Origin,
    name: &str,
    plugin: Option<&str>,
    scope: Scope,
) -> Result<CatalogEntry, String> {
    find_entry(catalog, origin, ElementKind::McpServer, name, plugin, scope)?
        .ok_or_else(|| format!("{name} isn't in the catalog"))
}

fn permission(
    ctx: &Context,
    item: &Found,
    choice: &ItemChoice,
    scope: Scope,
) -> Result<ItemState, String> {
    let list = |key: &str| item.details.get(key).cloned().unwrap_or_else(|| json!([]));
    let allow = if choice.allow_rules {
        list("allow")
    } else {
        json!([])
    };
    let details = json!({ "allow": allow, "ask": list("ask"), "deny": list("deny") });
    let outcome = ctx
        .catalog
        .put_rules(item.origin, scope, &item.name, details, &item.hash)
        .map_err(text)?;
    Ok(state(Some(outcome)))
}

/// A command file: stored as a command, with its file as the folder's `SKILL.md`, so it reads as the file reads.
fn command(
    ctx: &Context,
    item: &Found,
    choice: &ItemChoice,
    scope: Scope,
) -> Result<ItemState, String> {
    let body = crate::GuardedFs::new()
        .read_to_string(&item.source)
        .map_err(text)?;
    if crate::scan::text_hash(&body) != item.hash {
        return Err("source changed since the scan; scan again".into());
    }
    let staging = temp_folder()?;
    let result = fs::write(staging.join("SKILL.md"), &body)
        .map_err(text)
        .and_then(|()| {
            ctx.catalog
                .put_command(
                    &item.name,
                    item.origin,
                    scope,
                    &staging,
                    item.runs_code.clone(),
                    item.details.clone(),
                    choice.approve_code,
                    &item.hash,
                )
                .map_err(text)
        });
    let _ = fs::remove_dir_all(&staging);
    Ok(state(Some(result?)))
}

/// A text file that holds instructions or memory: a copy of the file, read as the file's own name.
fn file(ctx: &Context, item: &Found, scope: Scope) -> Result<ItemState, String> {
    let scope = if item.kind == FoundKind::Memory && scope == Scope::Global {
        ctx.catalog
            .list()
            .map_err(text)?
            .into_iter()
            .find(|entry| {
                entry.kind == ElementKind::Memory
                    && entry.origin == item.origin
                    && entry.name == item.name
                    && entry.details.get("source") == Some(&json!(item.source))
            })
            .map_or(scope, |entry| entry.scope)
    } else {
        scope
    };
    let staging = temp_folder()?;
    let name = item
        .source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let body = crate::GuardedFs::new()
        .read_to_string(&item.source)
        .map_err(text)?;
    if crate::scan::text_hash(&body) != item.hash {
        return Err("source changed since the scan; scan again".into());
    }
    let result = fs::write(staging.join(&name),body).map_err(text).and_then(|_| {
        let entry = CatalogEntry {
            id: 0,
            kind: catalog_kind(item.kind).ok_or_else(|| "not an instruction file".to_string())?,
            name: item.name.clone(),
            origin: item.origin,
            scope,
            content_hash: String::new(),
            content_dir: PathBuf::new(),
            enabled: matches!(scope,Scope::Project(_)) || item.details.get("project_known").and_then(Value::as_bool) != Some(false),
            runs_code: Vec::new(),
            approved_hash: None,
            plugin: None,
            details: json!({ "path": name, "source": item.source, "native_project":item.details.get("native_project"), "warning":item.details.get("warning") }),
        };
        ctx.catalog.put_from(entry, &staging, &item.hash).map_err(text)
    });
    let _ = fs::remove_dir_all(&staging);
    Ok(state(Some(result?)))
}

/// The stored entry of an item, if the catalog holds one for it in `scope`.
pub(crate) fn find_entry(
    catalog: &Catalog,
    origin: Origin,
    kind: ElementKind,
    name: &str,
    plugin: Option<&str>,
    scope: Scope,
) -> Result<Option<CatalogEntry>, String> {
    let entries = catalog.list().map_err(text)?;
    Ok(entries.into_iter().find(|entry| {
        entry.origin == origin
            && entry.kind == kind
            && entry.name == name
            && entry.scope == scope
            && entry.plugin.as_deref() == plugin
    }))
}

/// The stored entries that match each item, by item key. An item with no stored entry is new to the catalog.
pub(crate) fn stored_matches<'a>(
    items: &[Found],
    projects: &[Option<i64>],
    entries: &'a [CatalogEntry],
) -> HashMap<String, &'a CatalogEntry> {
    let mut matches = HashMap::new();
    for item in items {
        let (Some(kind), Ok(scope)) = (catalog_kind(item.kind), scope_of(item, projects)) else {
            continue;
        };
        let plugin = plugin_of(item, items);
        let plugin_name = plugin.map(|plugin| plugin.name.as_str());
        let name = catalog_name(item, plugin);
        let found = entries.iter().find(|entry| {
            entry.kind == kind
                && entry.origin == item.origin
                && (entry.scope == scope
                    || (item.kind == FoundKind::Memory
                        && scope == Scope::Global
                        && entry.details.get("source") == Some(&json!(item.source))))
                && entry.name == name
                && entry.plugin.as_deref() == plugin_name
        });
        if let Some(entry) = found {
            matches.insert(item_key(item), entry);
        }
    }
    matches
}

/// The items whose code is approved: their stored entry approves the content the scan read.
pub fn approved_items(
    items: &[Found],
    projects: &[Option<i64>],
    catalog: &Catalog,
) -> Result<BTreeSet<(PathBuf, String)>, String> {
    let entries = catalog.list().map_err(text)?;
    let sources = catalog.source_hashes().map_err(text)?;
    let matches = stored_matches(items, projects, &entries);
    Ok(items
        .iter()
        .filter(|item| {
            matches.get(&item_key(item)).is_some_and(|entry| {
                entry.approved_hash.as_deref() == Some(entry.content_hash.as_str())
                    && sources.get(&entry.id).map(String::as_str)
                        == Some(source_hash_of(item, items))
            })
        })
        .map(|item| (item.source.clone(), item.name.clone()))
        .collect())
}

/// Whether a secret of an item was chosen to be stored.
fn consents(choice: &ItemChoice, key: &str) -> bool {
    choice.secrets.get(key).copied().unwrap_or(false)
}

/// Whether a stored server already holds a keychain value for `key`.
fn has_value(details: &Value, key: &str) -> bool {
    details
        .get("secrets")
        .and_then(Value::as_object)
        .is_some_and(|secrets: &Map<String, Value>| secrets.contains_key(key))
}

fn state(outcome: Option<PutOutcome>) -> ItemState {
    match outcome {
        Some(PutOutcome::Added) => ItemState::Added,
        Some(PutOutcome::Changed) => ItemState::Changed,
        Some(PutOutcome::Unchanged) | None => ItemState::Unchanged,
    }
}

/// A folder in the OS temporary folder, unique to this process and moment. Never in the repository or the data folder.
fn temp_folder() -> Result<PathBuf, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos());
    let folder = std::env::temp_dir().join(format!("vatra-import-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&folder).map_err(text)?;
    Ok(folder)
}

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}
