use std::path::Path;

use vatra_parity::load_tasks;

fn repo_parity() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../parity"))
}

#[test]
fn six_tasks_load() {
    let tasks = load_tasks(&repo_parity().join("tasks")).expect("tasks load");
    let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "01-fix-test",
            "02-rename",
            "03-mcp",
            "04-skill",
            "05-search",
            "06-multi-step"
        ]
    );
    for task in &tasks {
        let fixture = repo_parity().join("fixtures").join(&task.fixture);
        assert!(
            fixture.is_dir(),
            "missing fixture folder {}",
            fixture.display()
        );
    }
}

#[test]
fn bad_task_file_is_named_in_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.toml"), "id = \"a\"\n").unwrap();
    let err = load_tasks(dir.path()).unwrap_err();
    assert!(err.contains("a.toml"), "error was: {err}");
}

#[test]
fn fixture_paths_cannot_escape_the_fixtures_folder() {
    let dir = tempfile::tempdir().unwrap();
    for fixture in ["../outside", "C:\\outside", "/outside", ""] {
        let task = vatra_parity::Task {
            id: "test".into(),
            title: "test".into(),
            prompt: "test".into(),
            fixture: fixture.into(),
            allowed_commands: vec![],
            needs_mcp: false,
            skill: None,
            checks: vec![],
        };
        std::fs::write(
            dir.path().join("task.toml"),
            toml::to_string(&task).unwrap(),
        )
        .unwrap();
        assert!(
            load_tasks(dir.path()).is_err(),
            "accepted fixture {fixture}"
        );
    }
}
