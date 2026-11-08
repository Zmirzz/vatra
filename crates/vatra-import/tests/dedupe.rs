mod support;

use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::Value;
use support::{codex_plugin, codex_plugin_cache, dirs, plugin_cache, write_json, write_text};
use vatra_extensions::{Origin, Scope};
use vatra_import::{Found, FoundKind, GuardedFs, ProjectRef, dedupe, scan_claude, scan_codex};

fn item(kind: FoundKind, name: &str, origin: Origin, hash: &str) -> Found {
    Found {
        kind,
        name: name.into(),
        origin,
        scope: Scope::Global,
        project: None,
        source: PathBuf::from(format!("{origin:?}/{name}")),
        hash: hash.into(),
        runs_code: Vec::new(),
        secrets: Vec::new(),
        unsupported: None,
        details: Value::Null,
        modified: None,
        also_from: Vec::new(),
    }
}

#[test]
fn same_name_same_content_merges_keeps_both_origins() {
    let found = vec![
        item(FoundKind::Skill, "review", Origin::Claude, "same"),
        item(FoundKind::Skill, "review", Origin::Codex, "same"),
    ];

    let result = dedupe(found);

    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].origin, Origin::Claude);
    assert_eq!(result.items[0].also_from, vec![Origin::Codex]);
    assert!(result.conflicts.is_empty());
}

#[test]
fn same_name_different_content_is_conflict_newest_default() {
    let mut old = item(FoundKind::Skill, "review", Origin::Claude, "old");
    old.modified = Some(UNIX_EPOCH + Duration::from_secs(1));
    let mut new = item(FoundKind::Skill, "review", Origin::Codex, "new");
    new.modified = Some(UNIX_EPOCH + Duration::from_secs(100));

    // The newer copy wins whichever order the scans gave.
    for found in [vec![old.clone(), new.clone()], vec![new.clone(), old.clone()]] {
        let result = dedupe(found);

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].hash, "new");
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].kept.hash, "new");
        assert_eq!(result.conflicts[0].dropped.len(), 1);
        assert_eq!(result.conflicts[0].dropped[0].hash, "old");
    }
}

#[test]
fn different_projects_are_not_duplicates() {
    let mut first = item(FoundKind::Skill, "lint", Origin::Project, "one");
    first.project = Some(ProjectRef(0));
    let mut second = item(FoundKind::Skill, "lint", Origin::Project, "two");
    second.project = Some(ProjectRef(1));

    let result = dedupe(vec![first, second]);

    assert_eq!(result.items.len(), 2);
    assert!(result.conflicts.is_empty());
}

#[test]
fn dual_manifest_plugin_once() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let write_dual = |root: &std::path::Path| {
        write_json(
            &root.join(".claude-plugin").join("plugin.json"),
            &serde_json::json!({ "name": "dual" }),
        );
        codex_plugin(root, "dual", "greet");
    };
    write_dual(&plugin_cache(home, "dual", "1.0.0"));
    write_dual(&codex_plugin_cache(home, "market", "dual", "1.0.0"));
    write_json(
        &home.join(".claude").join("settings.json"),
        &serde_json::json!({ "enabledPlugins": { "dual@market": true } }),
    );
    write_json(
        &home.join(".claude").join("plugins").join("installed_plugins.json"),
        &serde_json::json!({
            "version": 2,
            "plugins": {
                "dual@market": [{ "scope": "user", "installPath": plugin_cache(home, "dual", "1.0.0") }]
            }
        }),
    );
    write_text(
        &home.join(".codex").join("config.toml"),
        "[plugins.\"dual@market\"]\nenabled = true\n",
    );

    let claude = scan_claude(&GuardedFs::new(), &dirs(home), &[]);
    let codex = scan_codex(&GuardedFs::new(), &dirs(home), &[]);
    let mut found = claude.found;
    found.extend(codex.found);
    let result = dedupe(found);

    let plugins: Vec<_> = result
        .items
        .iter()
        .filter(|found| found.kind == FoundKind::Plugin && found.name == "dual")
        .collect();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].also_from, vec![Origin::Codex]);
    let skills = result.items.iter().filter(|found| found.name == "dual:greet").count();
    assert_eq!(skills, 1);
    assert!(result.conflicts.is_empty());
}
