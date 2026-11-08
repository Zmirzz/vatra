use std::{fs, path::Path};
use tempfile::tempdir;
use vatra_core::Project;
use vatra_import::{
    GuardedFs, SourceDirs, claude_project_key, import_memory, map_project, scan_claude, scan_codex,
};

#[test]
fn claude_key_examples() {
    assert_eq!(claude_project_key(Path::new(r"E:\code\x")), "E--code-x");
    assert_eq!(
        claude_project_key(Path::new("/home/test/project")),
        "-home-test-project"
    );
}

#[test]
fn project_mapping_case_insensitive_on_windows() {
    let projects = vec![Project {
        id: 7,
        name: "test".into(),
        path: Some(r"E:\Code\X".into()),
    }];
    assert_eq!(map_project(r"e:/code/x/", &projects), Some(7));
    assert_eq!(map_project("E--Code-X", &projects), Some(7));
    assert_eq!(map_project("unknown", &projects), None);
}

#[test]
fn claude_memory_copied_with_origin_header_and_index_merged_not_overwritten() {
    let temp = tempdir().unwrap();
    let home = temp.path();
    let dirs = SourceDirs::from_env(|key| {
        (key == "USERPROFILE" || key == "HOME").then(|| home.display().to_string())
    });
    let source = dirs.claude.join("projects/E--code-x/memory");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("MEMORY.md"), "Remember the database.").unwrap();
    fs::write(source.join("topic.md"), "Use SQLite.").unwrap();
    let scan = scan_claude(&GuardedFs::new(), &dirs, &[]);
    let memory: Vec<_> = scan
        .found
        .iter()
        .filter(|item| item.kind == vatra_import::FoundKind::Memory)
        .collect();
    assert_eq!(memory.len(), 2, "unknown project memory must remain listed");
    let target = home.join("output");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("MEMORY.md"), "Existing notes.\n").unwrap();
    for found in &memory {
        import_memory(found, &target).unwrap();
    }
    let index = fs::read_to_string(target.join("MEMORY.md")).unwrap();
    assert!(index.contains("Existing notes."));
    assert!(index.contains("claude"));
    assert!(index.contains("Remember the database."));
    assert!(index.contains("topic"));
    for found in &memory {
        import_memory(found, &target).unwrap();
    }
    assert_eq!(fs::read_to_string(target.join("MEMORY.md")).unwrap(), index);
    assert_eq!(
        fs::read_to_string(source.join("MEMORY.md")).unwrap(),
        "Remember the database."
    );
}

#[test]
fn codex_memory_md_copied_raw_memories_ignored() {
    let temp = tempdir().unwrap();
    let home = temp.path();
    let dirs = SourceDirs::from_env(|key| {
        (key == "USERPROFILE" || key == "HOME").then(|| home.display().to_string())
    });
    fs::create_dir_all(dirs.codex.join("memories/raw_memories")).unwrap();
    fs::write(dirs.codex.join("memories/MEMORY.md"), "Index.").unwrap();
    fs::write(dirs.codex.join("memories/raw_memories/private.md"), "raw").unwrap();
    let scan = scan_codex(&GuardedFs::new(), &dirs, &[]);
    assert!(scan.found.iter().any(|item| item.name == "MEMORY.md"));
    assert!(
        !scan
            .found
            .iter()
            .any(|item| item.source.to_string_lossy().contains("raw_memories"))
    );
}

#[test]
fn token_like_memory_warning_reaches_the_report_without_displaying_the_token() {
    const TOKEN: &str = "sk-synthetic-private-token-marker-never-display";
    let temp = tempdir().unwrap();
    let home = temp.path();
    let dirs = SourceDirs::from_env(|key| {
        (key == "USERPROFILE" || key == "HOME").then(|| home.display().to_string())
    });
    let memory = dirs.claude.join("projects/E--unknown/memory");
    fs::create_dir_all(&memory).unwrap();
    fs::write(
        memory.join("MEMORY.md"),
        format!("A useful note with {TOKEN} inside."),
    )
    .unwrap();
    let scan = scan_claude(&GuardedFs::new(), &dirs, &[]);
    let found = scan
        .found
        .iter()
        .find(|item| item.kind == vatra_import::FoundKind::Memory)
        .unwrap();
    assert!(
        found.details["warning"]
            .as_str()
            .unwrap()
            .contains("Review")
    );
    let report = vatra_import::build_report(&scan.found, &vatra_import::Probes::default());
    let serialized = serde_json::to_value(&report).unwrap();
    let warning = serialized["items"][0]["warning"]
        .as_str()
        .expect("memory warning must reach the import report");
    assert!(warning.contains("Review"));
    assert!(!serialized.to_string().contains(TOKEN));
    assert!(!serde_json::to_string(&scan).unwrap().contains(TOKEN));
}

#[test]
fn mapped_memory_is_project_scoped_and_unmapped_memory_stays_inactive_until_linked() {
    use vatra_core::MemorySecretStore;
    use vatra_extensions::{Catalog, ElementKind, Scope};
    use vatra_import::{ImportChoice, ItemChoice, apply, item_key};
    let temp = tempdir().unwrap();
    let dirs = SourceDirs::from_env(|key| {
        (key == "USERPROFILE" || key == "HOME").then(|| temp.path().display().to_string())
    });
    let project_path = temp.path().join("work");
    fs::create_dir_all(&project_path).unwrap();
    let known_key = claude_project_key(&project_path);
    for (key, text) in [
        (&known_key[..], "Mapped project note."),
        ("E--unknown-work", "Unmapped project note."),
    ] {
        let folder = dirs.claude.join("projects").join(key).join("memory");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("MEMORY.md"), text).unwrap();
    }
    let scanned = scan_claude(
        &GuardedFs::new(),
        &dirs,
        std::slice::from_ref(&project_path),
    );
    let choice = ImportChoice {
        projects: vec![Some(7)],
        items: scanned
            .found
            .iter()
            .filter(|item| item.kind == vatra_import::FoundKind::Memory)
            .map(|item| ItemChoice {
                key: item_key(item),
                include: true,
                ..Default::default()
            })
            .collect(),
        include_history: false,
        history_projects: vec![],
    };
    let data = temp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let catalog = Catalog::open(&data.join("vatra.db"), &data).unwrap();
    let outcome = apply(&scanned, &choice, &catalog, &MemorySecretStore::default());
    assert_eq!(outcome.results.len(), 2);
    let entries = catalog.list().unwrap();
    let known = entries
        .iter()
        .find(|entry| entry.kind == ElementKind::Memory && entry.name.starts_with(&known_key))
        .unwrap();
    let unknown = entries
        .iter()
        .find(|entry| entry.name.starts_with("E--unknown-work/"))
        .unwrap();
    assert_eq!(known.scope, Scope::Project(7));
    assert!(known.enabled);
    assert_eq!(unknown.scope, Scope::Global);
    assert!(!unknown.enabled);
    assert_eq!(catalog.active_for(7).unwrap().memory.len(), 1);
    assert!(catalog.active_for(8).unwrap().memory.is_empty());
    assert!(
        catalog.set_enabled(unknown.id, true).is_err(),
        "unlinked memory cannot accidentally become global"
    );
    catalog.set_project(unknown.id, 8).unwrap();
    apply(&scanned, &choice, &catalog, &MemorySecretStore::default());
    assert_eq!(
        catalog.list().unwrap().len(),
        2,
        "reimport must retain the linked project without a new global copy"
    );
    assert_eq!(catalog.active_for(7).unwrap().memory.len(), 1);
    assert_eq!(catalog.active_for(8).unwrap().memory.len(), 1);
    assert_eq!(
        fs::read_to_string(
            dirs.claude
                .join("projects/E--unknown-work/memory/MEMORY.md")
        )
        .unwrap(),
        "Unmapped project note."
    );
}
