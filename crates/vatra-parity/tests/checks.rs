use std::path::Path;

use vatra_parity::{Check, Task, evaluate};

fn task(checks: Vec<Check>) -> Task {
    Task {
        id: "t".into(),
        title: "t".into(),
        prompt: "t".into(),
        fixture: "t".into(),
        allowed_commands: vec![],
        needs_mcp: false,
        skill: None,
        checks,
    }
}

/// Fixture with `same.txt` and `changed.txt`; the workdir is a copy with `changed.txt` edited.
fn dirs(fixture: &Path, work: &Path) {
    std::fs::write(fixture.join("same.txt"), "hello world").unwrap();
    std::fs::write(fixture.join("changed.txt"), "before").unwrap();
    std::fs::write(work.join("same.txt"), "hello world").unwrap();
    std::fs::write(work.join("changed.txt"), "after").unwrap();
}

async fn run(checks: Vec<Check>, fixture: &Path, work: &Path) -> bool {
    evaluate(&task(checks), fixture, work, &[]).await.passed
}

#[tokio::test]
async fn file_contains() {
    let fixture = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    dirs(fixture.path(), work.path());
    let pass = Check::FileContains {
        path: "same.txt".into(),
        text: "world".into(),
    };
    let fail = Check::FileContains {
        path: "same.txt".into(),
        text: "mars".into(),
    };
    assert!(run(vec![pass], fixture.path(), work.path()).await);
    assert!(!run(vec![fail], fixture.path(), work.path()).await);
}

#[tokio::test]
async fn file_not_contains() {
    let fixture = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    dirs(fixture.path(), work.path());
    let pass = Check::FileNotContains {
        path: "same.txt".into(),
        text: "mars".into(),
    };
    let fail = Check::FileNotContains {
        path: "same.txt".into(),
        text: "world".into(),
    };
    assert!(run(vec![pass], fixture.path(), work.path()).await);
    assert!(!run(vec![fail], fixture.path(), work.path()).await);
}

#[tokio::test]
async fn file_unchanged() {
    let fixture = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    dirs(fixture.path(), work.path());
    let pass = Check::FileUnchanged {
        path: "same.txt".into(),
    };
    let fail = Check::FileUnchanged {
        path: "changed.txt".into(),
    };
    assert!(run(vec![pass], fixture.path(), work.path()).await);
    assert!(!run(vec![fail], fixture.path(), work.path()).await);
}

#[tokio::test]
async fn command_passes() {
    let fixture = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("ok.js"), "process.exit(0)").unwrap();
    std::fs::write(
        work.path().join("bad.js"),
        "console.log('nope'); process.exit(3)",
    )
    .unwrap();
    let pass = Check::CommandPasses {
        command: "node ok.js".into(),
    };
    let fail = Check::CommandPasses {
        command: "node bad.js".into(),
    };
    assert!(run(vec![pass], fixture.path(), work.path()).await);
    let outcome = evaluate(&task(vec![fail]), fixture.path(), work.path(), &[]).await;
    assert!(!outcome.passed);
    assert!(
        outcome.failures[0].contains("node bad.js"),
        "{:?}",
        outcome.failures
    );
    assert!(
        outcome.failures[0].contains("nope"),
        "{:?}",
        outcome.failures
    );
}

#[tokio::test]
async fn answer_contains() {
    use vatra_core::Event;
    let fixture = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let transcript = vec![Event::AssistantText {
        text: "It is in src/Retry.js".into(),
    }];
    let pass = Check::AnswerContains {
        text: "retry.js".into(),
    };
    let fail = Check::AnswerContains { text: "7".into() };
    let ok = evaluate(&task(vec![pass]), fixture.path(), work.path(), &transcript).await;
    assert!(ok.passed);
    let bad = evaluate(&task(vec![fail]), fixture.path(), work.path(), &transcript).await;
    assert!(!bad.passed);
}

#[tokio::test]
async fn file_checks_cannot_read_outside_the_workdir() {
    let root = tempfile::tempdir().unwrap();
    let work = root.path().join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(root.path().join("secret.txt"), "secret").unwrap();
    for path in [
        "../secret.txt".to_string(),
        root.path()
            .join("secret.txt")
            .to_string_lossy()
            .into_owned(),
    ] {
        for check in [
            Check::FileContains {
                path: path.clone(),
                text: "secret".into(),
            },
            Check::FileNotContains {
                path: path.clone(),
                text: "missing".into(),
            },
            Check::FileUnchanged { path },
        ] {
            assert!(
                !run(vec![check], &work, &work).await,
                "outside file accepted"
            );
        }
    }
}
