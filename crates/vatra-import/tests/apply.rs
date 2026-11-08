mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use support::{MARKER, dirs, synthetic_home, write_text};
use vatra_core::{MemorySecretStore, SecretStore};
use vatra_extensions::{Catalog, CatalogEntry, ElementKind};
use vatra_import::{
    ApplyOutcome, Found, FoundKind, GuardedFs, ImportChoice, ItemChoice, ItemState, Plan, Probes,
    ProjectRef, ScanResult, apply, item_key, plan, scan_claude, scan_project, scan_view,
};

/// A synthetic HOME, a project folder it names, and an empty catalog. Project 7 is the catalog project of that
/// folder, so project items land in `Scope::Project(7)`.
struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    data: PathBuf,
    catalog: Catalog,
    secrets: MemorySecretStore,
}

const PROJECT: i64 = 7;

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    synthetic_home(&home, &project);
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let catalog = Catalog::open(&data.join("vatra.db"), &data).unwrap();
    Env {
        _tmp: tmp,
        home,
        project,
        data,
        catalog,
        secrets: MemorySecretStore::default(),
    }
}

impl Env {
    /// What the import reads: the Claude Code folders, and the project's own folder as project 0.
    fn scan(&self) -> ScanResult {
        let fs = GuardedFs::new();
        let mut scan = scan_claude(&fs, &dirs(&self.home), std::slice::from_ref(&self.project));
        scan.found
            .extend(scan_project(&fs, &self.project, ProjectRef(0)).found);
        scan
    }

    fn apply(&self, scan: &ScanResult, choice: &ImportChoice) -> ApplyOutcome {
        apply(scan, choice, &self.catalog, &self.secrets)
    }
}

/// The key of the item of `kind` named `name` in a plan.
fn key(plan: &Plan, kind: FoundKind, name: &str) -> String {
    let found: &Found = plan
        .items
        .iter()
        .find(|item| item.kind == kind && item.name == name)
        .unwrap_or_else(|| panic!("no {name} in the scan"));
    item_key(found)
}

fn include(key: &str) -> ItemChoice {
    ItemChoice {
        key: key.into(),
        include: true,
        ..Default::default()
    }
}

fn choice(items: Vec<ItemChoice>) -> ImportChoice {
    ImportChoice {
        items,
        projects: vec![Some(PROJECT)],
        include_history: false,
        history_projects: vec![],
    }
}

fn skill_names(env: &Env) -> Vec<String> {
    let mut names: Vec<String> = vatra_extensions_active(env)
        .skills
        .into_iter()
        .map(|skill| skill.name)
        .collect();
    names.sort();
    names
}

fn vatra_extensions_active(env: &Env) -> vatra_extensions::ActiveExtensions {
    env.catalog.active_for(PROJECT).unwrap()
}

/// Writes a skill whose body runs a command, so approving it matters.
fn runner_skill(home: &Path, body: &str) {
    write_text(
        &home
            .join(".claude")
            .join("skills")
            .join("runner")
            .join("SKILL.md"),
        &format!("---\ndescription: Runs a command\n---\n{body}\n"),
    );
}

/// Every file under `dir`, with its bytes, so a change anywhere shows.
fn files_under(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.insert(path.clone(), fs::read(&path).unwrap());
            }
        }
    }
    files
}

fn contains_marker(bytes: &[u8]) -> bool {
    bytes
        .windows(MARKER.len())
        .any(|window| window == MARKER.as_bytes())
}

#[test]
fn includes_plain_by_default_code_needs_tick() {
    let env = env();
    runner_skill(&env.home, "Run it:\n!`echo run`");
    let scan = env.scan();
    let plan = plan(&scan);
    let plain = key(&plan, FoundKind::Skill, "personal");
    let code = key(&plan, FoundKind::Skill, "runner");

    env.apply(&scan, &choice(vec![include(&plain), include(&code)]));

    assert_eq!(skill_names(&env), vec!["personal".to_string()]);
    let mut approved = include(&code);
    approved.approve_code = true;
    env.apply(&scan, &choice(vec![approved]));
    assert!(skill_names(&env).contains(&"runner".to_string()));
}

#[test]
fn excluded_item_not_copied() {
    let env = env();
    let scan = env.scan();
    let plan = plan(&scan);
    let personal = key(&plan, FoundKind::Skill, "personal");
    let excluded = ItemChoice {
        key: personal,
        include: false,
        ..Default::default()
    };

    let outcome = env.apply(&scan, &choice(vec![excluded]));

    assert!(outcome.results.is_empty());
    assert!(
        env.catalog
            .list()
            .unwrap()
            .iter()
            .all(|entry| entry.name != "personal")
    );
}

#[test]
fn secret_without_consent_not_stored() {
    let env = env();
    let scan = env.scan();
    let plan = plan(&scan);
    let global = key(&plan, FoundKind::McpServer, "global");

    env.apply(&scan, &choice(vec![include(&global)]));

    assert!(env.secrets.keys().is_empty());
    let entry = env
        .catalog
        .list()
        .unwrap()
        .into_iter()
        .find(|entry| entry.name == "global")
        .unwrap();
    assert!(
        !entry.enabled,
        "a server whose secret was declined waits for its value"
    );
    assert!(
        !serde_json::to_string(&entry.details)
            .unwrap()
            .contains(MARKER)
    );
}

#[test]
fn secret_with_consent_goes_to_keychain_only() {
    let env = env();
    let scan = env.scan();
    let plan = plan(&scan);
    let global = key(&plan, FoundKind::McpServer, "global");
    let mut consent = include(&global);
    consent.secrets.insert("Authorization".into(), true);

    env.apply(&scan, &choice(vec![consent]));

    let stored: Vec<String> = env.secrets.keys();
    assert_eq!(stored.len(), 1, "{stored:?}");
    let value = env.secrets.get(&stored[0]).unwrap().unwrap();
    assert!(value.contains(MARKER));
    assert!(
        files_under(&env.data)
            .values()
            .all(|bytes| !contains_marker(bytes))
    );
}

#[test]
fn config_keeps_reference() {
    let env = env();
    let scan = env.scan();
    let plan = plan(&scan);
    let global = key(&plan, FoundKind::McpServer, "global");
    let mut consent = include(&global);
    consent.secrets.insert("Authorization".into(), true);

    env.apply(&scan, &choice(vec![consent]));

    let entry = env
        .catalog
        .list()
        .unwrap()
        .into_iter()
        .find(|entry| entry.name == "global")
        .unwrap();
    assert!(entry.enabled);
    assert_eq!(
        entry.details["headers"]["Authorization"],
        "${Authorization}"
    );
    assert!(
        entry.details["secrets"]["Authorization"]["keychain"]
            .as_str()
            .is_some()
    );
    assert!(
        !serde_json::to_string(&entry.details)
            .unwrap()
            .contains(MARKER)
    );
}

#[test]
fn deny_and_ask_rules_import_automatically() {
    let env = env();
    let scan = env.scan();
    let plan = plan(&scan);
    let permissions = key(&plan, FoundKind::Permission, "permissions:settings.json");

    env.apply(&scan, &choice(vec![include(&permissions)]));

    let active = vatra_extensions_active(&env);
    assert_eq!(active.rules.deny[0].specifier.as_deref(), Some("rm -rf *"));
    assert_eq!(active.rules.ask[0].tool, "Edit");
    assert!(active.rules.allow.is_empty());
}

#[test]
fn allow_rules_need_tick() {
    let env = env();
    let scan = env.scan();
    let plan = plan(&scan);
    let permissions = key(&plan, FoundKind::Permission, "permissions:settings.json");
    env.apply(&scan, &choice(vec![include(&permissions)]));
    assert!(vatra_extensions_active(&env).rules.allow.is_empty());

    let mut ticked = include(&permissions);
    ticked.allow_rules = true;
    env.apply(&scan, &choice(vec![ticked]));

    let allow: Vec<String> = vatra_extensions_active(&env)
        .rules
        .allow
        .iter()
        .map(|rule| rule.tool.clone())
        .collect();
    assert_eq!(allow, vec!["Read".to_string(), "Bash".to_string()]);
}

#[test]
fn reimport_shows_only_changes() {
    let env = env();
    let scan = env.scan();
    let plan = plan(&scan);
    let everything: Vec<ItemChoice> = plan
        .items
        .iter()
        .filter(|item| item.unsupported.is_none())
        .map(|item| include(&item_key(item)))
        .collect();
    env.apply(&scan, &choice(everything.clone()));

    write_text(
        &env.home
            .join(".claude")
            .join("skills")
            .join("personal")
            .join("SKILL.md"),
        "---\ndescription: Mine, edited\n---\nMine, edited.\n",
    );
    let changed = env.scan();
    let view = scan_view(
        &vatra_import::plan(&changed),
        &[Some(PROJECT)],
        Some(&env.catalog),
        &Probes::default(),
    )
    .unwrap();

    let changed_items: Vec<&str> = view
        .items
        .iter()
        .filter(|item| item.status == Some(vatra_import::Status::Changed))
        .map(|item| item.report.name.as_str())
        .collect();
    assert_eq!(changed_items, vec!["personal"]);
    let stored_items_unchanged = view
        .items
        .iter()
        .filter(|item| item.status == Some(vatra_import::Status::Unchanged))
        .count();
    assert!(stored_items_unchanged > 0);
}

#[test]
fn reimport_changed_code_loses_approval() {
    let env = env();
    runner_skill(&env.home, "!`echo one`");
    let scan = env.scan();
    let code = key(&plan(&scan), FoundKind::Skill, "runner");
    let mut approved = include(&code);
    approved.approve_code = true;
    env.apply(&scan, &choice(vec![approved]));
    assert!(skill_names(&env).contains(&"runner".to_string()));

    runner_skill(&env.home, "!`echo two`");
    let changed = env.scan();
    let code = key(&plan(&changed), FoundKind::Skill, "runner");
    env.apply(&changed, &choice(vec![include(&code)]));

    assert!(!skill_names(&env).contains(&"runner".to_string()));
}

#[test]
fn import_never_writes_to_source() {
    let env = env();
    let before = files_under(&env.home);
    let scan = env.scan();
    let everything: Vec<ItemChoice> = plan(&scan)
        .items
        .iter()
        .filter(|item| item.unsupported.is_none())
        .map(|item| {
            let mut item_choice = include(&item_key(item));
            item_choice.approve_code = true;
            item_choice.allow_rules = true;
            item_choice
        })
        .collect();

    env.apply(&scan, &choice(everything));

    assert_eq!(files_under(&env.home), before);
}

#[test]
fn failed_copy_leaves_catalog_consistent() {
    let env = env();
    let scan = env.scan();
    let plan = plan(&scan);
    let personal = key(&plan, FoundKind::Skill, "personal");
    let permissions = key(&plan, FoundKind::Permission, "permissions:settings.json");
    // A skill's copy is written under the extensions folder. That is a file here, so the write itself fails.
    fs::write(env.data.join("extensions"), b"in the way").unwrap();

    let outcome = env.apply(
        &scan,
        &choice(vec![include(&personal), include(&permissions)]),
    );

    let personal_result = outcome
        .results
        .iter()
        .find(|result| result.name == "personal")
        .unwrap();
    assert!(
        matches!(personal_result.state, ItemState::Failed { .. }),
        "{personal_result:?}"
    );
    assert!(
        env.catalog
            .list()
            .unwrap()
            .iter()
            .all(|entry| entry.name != "personal")
    );
    assert!(
        vatra_extensions_active(&env)
            .rules
            .deny
            .iter()
            .any(|rule| rule.tool == "Bash")
    );
}

#[test]
fn scan_view_has_no_secret_values() {
    let env = env();
    let scan = env.scan();
    let plan = plan(&scan);

    let view = scan_view(
        &plan,
        &[Some(PROJECT)],
        Some(&env.catalog),
        &Probes::default(),
    )
    .unwrap();

    let text = serde_json::to_string(&view).unwrap();
    assert!(!text.contains(MARKER));
    let global = view
        .items
        .iter()
        .find(|item| item.report.name == "global")
        .unwrap();
    assert_eq!(global.secrets[0].key, "Authorization");
}

#[test]
fn no_marker_in_db_or_events() {
    let env = env();
    let scan = env.scan();
    let plan = plan(&scan);
    let mut choices: Vec<ItemChoice> = plan
        .items
        .iter()
        .filter(|item| item.unsupported.is_none())
        .map(|item| {
            let mut item_choice = include(&item_key(item));
            item_choice.approve_code = true;
            item_choice.allow_rules = true;
            item_choice
        })
        .collect();
    for item in choices.iter_mut() {
        for name in ["Authorization", "PROJ_KEY", "TOKEN_VALUE"] {
            item.secrets.insert(name.into(), true);
        }
    }

    env.apply(&scan, &choice(choices));

    let stored = files_under(&env.data);
    assert!(!stored.is_empty());
    assert!(stored.values().all(|bytes| !contains_marker(bytes)));
    let names: Vec<String> = env
        .catalog
        .list()
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert!(names.contains(&"tools".to_string()), "{names:?}");
}

#[test]
fn plugin_literal_secret_declined_leaves_server_needing_a_value() {
    let env = env();
    let scan = env.scan();
    let plan = plan(&scan);
    let plugin = key(&plan, FoundKind::Plugin, "demo");

    env.apply(&scan, &choice(vec![include(&plugin)]));

    let server = env
        .catalog
        .list()
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == ElementKind::McpServer && entry.name == "tools")
        .unwrap();
    assert!(!server.enabled);
    assert_eq!(server.details["env"]["TOKEN_VALUE"], "${TOKEN_VALUE}");
    assert!(env.secrets.keys().is_empty());
}

#[test]
fn plugin_copy_has_no_literal_secret() {
    let env = env();
    let scan = env.scan();
    let plugin = key(&plan(&scan), FoundKind::Plugin, "demo");

    env.apply(&scan, &choice(vec![include(&plugin)]));

    assert!(
        files_under(&env.data)
            .values()
            .all(|bytes| !contains_marker(bytes))
    );
}

#[test]
fn project_item_for_unknown_project_is_listed_not_applied() {
    let env = env();
    let scan = env.scan();
    let plan = plan(&scan);
    let project_server = key(&plan, FoundKind::McpServer, "proj");
    let unknown = ImportChoice {
        items: vec![include(&project_server)],
        projects: vec![None],
        include_history: false,
        history_projects: vec![],
    };

    let outcome = env.apply(&scan, &unknown);

    let result = outcome
        .results
        .iter()
        .find(|result| result.name == "proj")
        .unwrap();
    assert!(
        matches!(result.state, ItemState::Skipped { .. }),
        "{result:?}"
    );
    assert!(
        env.catalog
            .list()
            .unwrap()
            .iter()
            .all(|entry| entry.name != "proj")
    );
}

#[test]
fn reimport_keeps_a_stored_secret() {
    let env = env();
    let scan = env.scan();
    let global = key(&plan(&scan), FoundKind::McpServer, "global");
    let mut consent = include(&global);
    consent.secrets.insert("Authorization".into(), true);
    env.apply(&scan, &choice(vec![consent]));

    env.apply(&env.scan(), &choice(vec![include(&global)]));

    let entry = env
        .catalog
        .list()
        .unwrap()
        .into_iter()
        .find(|entry| entry.name == "global")
        .unwrap();
    assert!(
        entry.enabled,
        "a reimport without the tick doesn't turn the server off"
    );
    assert_eq!(
        entry.details["headers"]["Authorization"],
        "${Authorization}"
    );
    assert!(
        entry.details["secrets"]["Authorization"]["keychain"]
            .as_str()
            .is_some()
    );
    assert_eq!(env.secrets.keys().len(), 1);
}

#[test]
fn changed_source_between_scan_and_apply_isnt_approved() {
    let env = env();
    runner_skill(&env.home, "!`echo one`");
    let seen = env.scan();
    let code = key(&plan(&seen), FoundKind::Skill, "runner");
    let mut approved = include(&code);
    approved.approve_code = true;

    runner_skill(&env.home, "!`echo two`");
    let applied = env.scan();
    let outcome = env.apply(&applied, &choice(vec![approved]));

    let result = outcome
        .results
        .iter()
        .find(|result| result.name == "runner")
        .unwrap();
    assert!(
        matches!(&result.state, ItemState::Skipped { reason } if reason.contains("changed")),
        "{result:?}"
    );
    assert!(!skill_names(&env).contains(&"runner".to_string()));
}

#[test]
fn project_settings_and_local_settings_keep_their_deny_rules() {
    let env = env();
    write_text(
        &env.project.join(".claude").join("settings.json"),
        "{\"permissions\": {\"deny\": [\"Bash(git push *)\"]}}",
    );
    write_text(
        &env.project.join(".claude").join("settings.local.json"),
        "{\"permissions\": {\"deny\": [\"Read(.env)\"]}}",
    );
    let scan = env.scan();
    let planned = plan(&scan);
    let project_items: Vec<ItemChoice> = planned
        .items
        .iter()
        .filter(|item| item.kind == FoundKind::Permission && item.source.starts_with(&env.project))
        .map(|item| include(&item_key(item)))
        .collect();
    assert_eq!(project_items.len(), 2);

    env.apply(&scan, &choice(project_items));

    let deny: Vec<String> = vatra_extensions_active(&env)
        .rules
        .deny
        .iter()
        .map(|rule| {
            format!(
                "{}({})",
                rule.tool,
                rule.specifier.clone().unwrap_or_default()
            )
        })
        .collect();
    assert!(deny.contains(&"Bash(git push *)".to_string()), "{deny:?}");
    assert!(deny.contains(&"Read(.env)".to_string()), "{deny:?}");
}

#[test]
fn commands_import_as_commands_with_code_off_until_approved() {
    let env = env();
    write_text(
        &env.home.join(".claude").join("commands").join("ship.md"),
        "---\ndescription: Ship it\n---\nCheck first:\n!`git status`\n",
    );
    let scan = env.scan();
    let ship = key(&plan(&scan), FoundKind::Command, "ship");

    env.apply(&scan, &choice(vec![include(&ship)]));
    assert!(!skill_names(&env).contains(&"ship".to_string()));

    let mut approved = include(&ship);
    approved.approve_code = true;
    env.apply(&scan, &choice(vec![approved]));
    let active = vatra_extensions_active(&env);
    let command = active
        .skills
        .iter()
        .find(|skill| skill.name == "ship")
        .unwrap();
    assert!(command.is_command);
}

/// A secret store that refuses every write, as a locked keychain does.
struct RefusingStore;

impl SecretStore for RefusingStore {
    fn get(&self, _key: &str) -> Result<Option<String>, vatra_core::SecretError> {
        Ok(None)
    }

    fn set(&self, _key: &str, _value: &str) -> Result<(), vatra_core::SecretError> {
        Err(vatra_core::SecretError::Backend(
            "the keychain is locked".into(),
        ))
    }

    fn delete(&self, _key: &str) -> Result<(), vatra_core::SecretError> {
        Ok(())
    }
}

#[test]
fn failed_secret_write_keeps_the_server_out() {
    let env = env();
    let scan = env.scan();
    let global = key(&plan(&scan), FoundKind::McpServer, "global");
    let mut consent = include(&global);
    consent.secrets.insert("Authorization".into(), true);

    let outcome = apply(&scan, &choice(vec![consent]), &env.catalog, &RefusingStore);

    let result = outcome
        .results
        .iter()
        .find(|result| result.name == "global")
        .unwrap();
    assert!(
        matches!(result.state, ItemState::Failed { .. }),
        "{result:?}"
    );
    let server = env
        .catalog
        .list()
        .unwrap()
        .into_iter()
        .find(|entry| entry.name == "global")
        .unwrap();
    assert!(!server.enabled, "a server without its value stays out");
    assert!(env.secrets.keys().is_empty());
}

#[test]
fn consented_value_reenables_a_disabled_server() {
    let env = env();
    let scan = env.scan();
    let global = key(&plan(&scan), FoundKind::McpServer, "global");
    env.apply(&scan, &choice(vec![include(&global)]));
    let disabled = env
        .catalog
        .list()
        .unwrap()
        .into_iter()
        .find(|entry| entry.name == "global")
        .unwrap();
    assert!(!disabled.enabled);

    let mut consent = include(&global);
    consent.secrets.insert("Authorization".into(), true);
    env.apply(&scan, &choice(vec![consent]));

    let server = env
        .catalog
        .list()
        .unwrap()
        .into_iter()
        .find(|entry| entry.name == "global")
        .unwrap();
    assert!(server.enabled);
}

/// The stored row of the MCP server named `name`.
fn server(env: &Env, name: &str) -> CatalogEntry {
    env.catalog
        .list()
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == ElementKind::McpServer && entry.name == name)
        .unwrap()
}

#[test]
fn reimport_keeps_a_user_disabled_server_off() {
    let env = env();
    let scan = env.scan();
    let global = key(&plan(&scan), FoundKind::McpServer, "global");
    let mut consent = include(&global);
    consent.secrets.insert("Authorization".into(), true);
    env.apply(&scan, &choice(vec![consent.clone()]));
    assert!(server(&env, "global").enabled);

    env.catalog
        .set_enabled(server(&env, "global").id, false)
        .unwrap();
    env.apply(&env.scan(), &choice(vec![consent]));
    assert!(
        !server(&env, "global").enabled,
        "a reimport with the value ticked turned it back on"
    );
    env.apply(&env.scan(), &choice(vec![include(&global)]));
    assert!(
        !server(&env, "global").enabled,
        "a plain reimport turned it back on"
    );
}

#[test]
fn plugin_commands_reach_the_catalog() {
    let env = env();
    let scan = env.scan();
    let plugin = key(&plan(&scan), FoundKind::Plugin, "demo");
    env.apply(&scan, &choice(vec![include(&plugin)]));

    let command = env
        .catalog
        .list()
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == ElementKind::Command && entry.name == "ship")
        .expect("the plugin's command is in the catalog");
    assert_eq!(command.plugin.as_deref(), Some("demo"));
    // Its code runs git, so it stays off until the plugin's code is approved.
    assert!(!skill_names(&env).contains(&"ship".to_string()));

    let mut approved = include(&plugin);
    approved.approve_code = true;
    env.apply(&scan, &choice(vec![approved]));
    let active = vatra_extensions_active(&env);
    assert!(
        active
            .skills
            .iter()
            .any(|skill| skill.name == "ship" && skill.is_command)
    );
}
