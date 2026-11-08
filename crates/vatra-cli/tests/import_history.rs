use std::process::Command;

#[test]
fn history_import_is_explicit_and_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = home.join(".claude");
    std::fs::create_dir_all(claude.join("projects/project")).unwrap();
    std::fs::write(claude.join("projects/project/history.jsonl"), r#"{"type":"user","sessionId":"native-test","message":{"role":"user","content":"pastquartz unique phrase"}}"#).unwrap();
    let db = dir.path().join("vatra.db");
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_vatra"))
            .args(["--db"])
            .arg(&db)
            .args(args)
            .env("USERPROFILE", &home)
            .env("HOME", &home)
            .env("CLAUDE_CONFIG_DIR", &claude)
            .env("CODEX_HOME", home.join(".codex"))
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(&["import", "--history", "--dry-run"]);
    assert!(!db.exists());
    run(&["import"]);
    assert!(
        vatra_core::Store::open(&db)
            .unwrap()
            .search_history("pastquartz", None, 10)
            .unwrap()
            .is_empty()
    );
    run(&["import", "--history"]);
    assert_eq!(
        vatra_core::Store::open(&db)
            .unwrap()
            .search_history("pastquartz", None, 10)
            .unwrap()
            .len(),
        1
    );
}
