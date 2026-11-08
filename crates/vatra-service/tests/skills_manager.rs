use serde_json::json;
use std::fs;
use std::sync::Arc;
use vatra_core::Store;
use vatra_extensions::{Catalog, Scope};
use vatra_service::skills::{SkillSource, scan_sources};
use vatra_service::{ConfigAgents, Service, config::Config};

#[test]
fn install_reload_disable_remove_and_restart_preserve_sources_and_catalog_state() {
    let data = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    skill(root.path(), "notes", "Take notes");
    let db = data.path().join("vatra.db");
    let store = Arc::new(Store::open(&db).unwrap());
    let catalog = Arc::new(Catalog::open(&db, data.path()).unwrap());
    let factory = ConfigAgents::new(Config::empty()).with_catalog(catalog.clone());
    let service = Service::new(store.clone(), Arc::new(factory)).unwrap();
    let call = |method, params| vatra_service::skills::handle(&service, method, params);
    let discovered = call(
        "skills.set_sources",
        json!([{"path":root.path(),"projectId":null}]),
    )
    .unwrap();
    let item = &discovered[0];
    let path = item["path"].clone();
    let target = vatra_service::skills::source_target(
        data.path(),
        std::path::Path::new(path.as_str().unwrap()),
        None,
        false,
    )
    .unwrap();
    assert_eq!(target.file_name().unwrap(), "SKILL.md");
    assert!(
        vatra_service::skills::source_target(
            data.path(),
            &data.path().join("outside.exe"),
            None,
            false
        )
        .is_err()
    );
    let hash = item["hash"].clone();
    assert!(
        call(
            "skills.install",
            json!({"path":path,"expectedHash":"stale"})
        )
        .is_err()
    );
    call("skills.install", json!({"path":path,"expectedHash":hash})).unwrap();
    let entry = catalog
        .list()
        .unwrap()
        .into_iter()
        .find(|entry| entry.name == "notes")
        .unwrap();
    assert_eq!(entry.scope, Scope::Global);
    let active = catalog.active_for(1).unwrap();
    let listing = vatra_engine::skills_listing(
        &active.skills,
        vatra_engine::model::ContextWindow {
            tokens: 8192,
            estimated: false,
        },
        &vatra_extensions::UsageCounts::default(),
    );
    assert!(listing.contains("notes"));
    assert!(!listing.contains("Instructions."));
    assert!(
        vatra_engine::resolve_prompt("$notes", &active.skills).mentions[0]
            .body
            .contains("Instructions.")
    );
    assert_eq!(
        fs::read_to_string(root.path().join("notes/scripts/run.sh")).unwrap(),
        "DO NOT EXECUTE"
    );
    catalog.set_enabled(entry.id, false).unwrap();
    assert!(catalog.active_for(1).unwrap().skills.is_empty());
    let other = Store::open(&db).unwrap();
    assert!(
        other
            .app_setting("skills.sources")
            .unwrap()
            .unwrap()
            .contains("projectId")
    );
    assert!(
        !Catalog::open(&db, data.path())
            .unwrap()
            .list()
            .unwrap()
            .into_iter()
            .find(|item| item.id == entry.id)
            .unwrap()
            .enabled
    );
    fs::write(
        root.path().join("notes/SKILL.md"),
        "---\nname: notes\ndescription: Updated\n---\nNew instructions.",
    )
    .unwrap();
    assert!(call("skills.install", json!({"path":path,"expectedHash":hash})).is_err());
    let changed = call("skills.scan", json!({})).unwrap();
    call(
        "skills.install",
        json!({"path":path,"expectedHash":changed[0]["hash"],"replaceHash":entry.content_hash}),
    )
    .unwrap();
    assert!(
        !catalog
            .list()
            .unwrap()
            .into_iter()
            .find(|item| item.id == entry.id)
            .unwrap()
            .enabled
    );
    call("skills.uninstall", json!({"id":entry.id})).unwrap();
    assert!(catalog.list().unwrap().is_empty());
    assert!(root.path().join("notes/SKILL.md").is_file());
    assert!(entry.content_dir.join("SKILL.md").is_file());
    assert_eq!(
        store
            .audit_recent(10)
            .unwrap()
            .iter()
            .filter(|entry| entry.connector == "skills")
            .count(),
        3
    );
}

fn skill(root: &std::path::Path, name: &str, description: &str) {
    fs::create_dir_all(root.join(name).join("scripts")).unwrap();
    fs::write(
        root.join(name).join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\nInstructions.\n"),
    )
    .unwrap();
    fs::write(root.join(name).join("scripts/run.sh"), "DO NOT EXECUTE").unwrap();
}

#[test]
fn discovers_metadata_resources_and_duplicate_names_without_changing_sources() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    skill(a.path(), "notes", "Take notes");
    skill(b.path(), "notes", "Other notes");
    let sources = vec![
        SkillSource {
            path: a.path().into(),
            project_id: None,
        },
        SkillSource {
            path: b.path().into(),
            project_id: None,
        },
    ];
    let items = scan_sources(&sources).unwrap();
    assert_eq!(items.len(), 2);
    assert!(
        items
            .iter()
            .all(|item| item.errors.iter().any(|error| error.contains("Duplicate")))
    );
    assert!(items[0].resources.contains(&"scripts/run.sh".into()));
    assert!(
        !serde_json::to_string(&items)
            .unwrap()
            .contains("Instructions.")
    );
    assert_eq!(
        fs::read_to_string(a.path().join("notes/scripts/run.sh")).unwrap(),
        "DO NOT EXECUTE"
    );
}

#[test]
fn project_skills_do_not_conflict_with_global_skills_and_invalid_metadata_is_visible() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    skill(a.path(), "notes", "Take notes");
    skill(b.path(), "notes", "Project notes");
    skill(a.path(), "Bad_Name", "Bad name");
    let items = scan_sources(&[
        SkillSource {
            path: a.path().into(),
            project_id: None,
        },
        SkillSource {
            path: b.path().into(),
            project_id: Some(7),
        },
    ])
    .unwrap();
    assert_eq!(
        items
            .iter()
            .filter(|item| item.name == "notes" && item.errors.is_empty())
            .count(),
        2
    );
    assert!(
        items
            .iter()
            .find(|item| item.name == "Bad_Name")
            .unwrap()
            .errors
            .iter()
            .any(|error| error.contains("name"))
    );
    fs::write(
        b.path().join("notes/SKILL.md"),
        "---\nname: notes\ndescription: Changed\n---\nNew body",
    )
    .unwrap();
    let reloaded = scan_sources(&[SkillSource {
        path: b.path().into(),
        project_id: Some(7),
    }])
    .unwrap();
    assert_eq!(reloaded[0].description, "Changed");
}

#[test]
fn rejects_unbounded_skill_files() {
    let root = tempfile::tempdir().unwrap();
    skill(root.path(), "large", "Too big");
    fs::write(
        root.path().join("large/SKILL.md"),
        vec![b'a'; 512 * 1024 + 1],
    )
    .unwrap();
    let items = scan_sources(&[SkillSource {
        path: root.path().into(),
        project_id: None,
    }])
    .unwrap();
    assert!(items[0].errors.iter().any(|error| error.contains("large")));
}
