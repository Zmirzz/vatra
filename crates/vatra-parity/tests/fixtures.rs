use std::path::Path;

use vatra_parity::{Check, copy_fixture, evaluate, load_tasks};

#[tokio::test]
async fn fixtures_fail_before_solution() {
    let parity = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../parity"));
    let tasks = load_tasks(&parity.join("tasks")).unwrap();
    for id in ["01-fix-test", "02-rename", "06-multi-step"] {
        let mut task = tasks.iter().find(|t| t.id == id).expect(id).clone();
        // Only the command check: the other checks describe the finished work.
        task.checks.retain(|c| matches!(c, Check::CommandPasses { .. }));
        let fixture = parity.join("fixtures").join(&task.fixture);
        let work = tempfile::tempdir().unwrap();
        copy_fixture(&fixture, work.path()).unwrap();
        let outcome = evaluate(&task, &fixture, work.path(), &[]).await;
        assert!(!outcome.passed, "{id} passes before any work is done");
    }
}

#[test]
fn copy_fixture_is_recursive() {
    let src = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(src.path().join("a/b")).unwrap();
    std::fs::write(src.path().join("a/b/c.txt"), "deep").unwrap();
    let dst = tempfile::tempdir().unwrap();
    copy_fixture(src.path(), dst.path()).unwrap();
    assert_eq!(std::fs::read_to_string(dst.path().join("a/b/c.txt")).unwrap(), "deep");
}
