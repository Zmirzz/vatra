use serde_json::json;
use std::sync::{Arc, Barrier};
use vatra_core::{ItemChange, NewItem, Store, WorkspaceError};

fn create(store: &Store, project: i64, body: &str) -> i64 {
    store
        .create_item(
            &NewItem {
                kind: "note".into(),
                title: "Notiță".into(),
                body: body.into(),
                fields: json!({}),
                project_id: Some(project),
            },
            100,
        )
        .unwrap()
}
fn change(id: i64, revision: u64, body: &str) -> ItemChange {
    ItemChange {
        id,
        expected_revision: revision,
        title: None,
        body: Some(body.into()),
        fields: None,
    }
}

#[test]
fn same_timestamp_edits_still_conflict() {
    let store = Store::open_in_memory().unwrap();
    let project = store.create_project("A").unwrap();
    let id = create(&store, project, "original");
    assert_eq!(
        store
            .save_workspace_item(project, &change(id, 1, "X"), 100)
            .unwrap()
            .revision,
        2
    );
    match store.save_workspace_item(project, &change(id, 1, "Y"), 100) {
        Err(WorkspaceError::Conflict(item)) => assert_eq!(item.body, "X"),
        other => panic!("{other:?}"),
    }
    assert_eq!(store.workspace_item(project, id, false).unwrap().body, "X");
}

#[test]
fn two_connections_cannot_save_the_same_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.db");
    let store = Store::open(&path).unwrap();
    let project = store.create_project("A").unwrap();
    let id = create(&store, project, "original");
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = ["X", "Y"]
        .into_iter()
        .map(|body| {
            let other = Store::open(&path).unwrap();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                other.save_workspace_item(project, &change(id, 1, body), 100)
            })
        })
        .collect();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(WorkspaceError::Conflict(_))))
            .count(),
        1
    );
    assert_eq!(store.workspace_item(project, id, false).unwrap().revision, 2);
}

#[test]
fn trash_restore_reindexes_once_and_respects_project_scope() {
    let store = Store::open_in_memory().unwrap();
    let a = store.create_project("A").unwrap();
    let b = store.create_project("B").unwrap();
    let id = create(&store, a, "țară");
    assert!(matches!(
        store.workspace_item(b, id, false),
        Err(WorkspaceError::NotFound)
    ));
    let removed = store.delete_workspace_item(a, id, 1, 100).unwrap();
    assert_eq!(removed.revision, 2);
    assert_eq!(removed.deleted_at, Some(100));
    assert!(
        store
            .search_workspace_in_project(a, "tara", &[], 50)
            .unwrap()
            .is_empty()
    );
    assert!(store.workspace_items(a, &[], false, None, 50).unwrap().is_empty());
    assert_eq!(store.workspace_items(a, &[], true, None, 50).unwrap().len(), 1);
    let restored = store.restore_workspace_item(a, id, 2, 101).unwrap();
    assert_eq!(restored.revision, 3);
    assert_eq!(restored.deleted_at, None);
    assert_eq!(store.search_workspace_in_project(a, "tara", &[], 50).unwrap().len(), 1);
    assert!(store.restore_workspace_item(a, id, 3, 102).is_err());
}

#[test]
fn project_filter_applies_before_limit_and_links_stay_in_project() {
    let store = Store::open_in_memory().unwrap();
    let a = store.create_project("A").unwrap();
    let b = store.create_project("B").unwrap();
    let id = create(&store, a, "needle");
    let linked = create(&store, a, "linked");
    let foreign = create(&store, b, "needle");
    for _ in 0..110 {
        create(&store, b, "needle");
    }
    assert_eq!(
        store.search_workspace_in_project(a, "needle", &[], 1).unwrap()[0].id,
        id
    );
    assert_eq!(store.workspace_items(a, &[], false, None, 1).unwrap()[0].id, linked);
    assert_eq!(
        store.workspace_items(a, &[], false, Some(linked), 50).unwrap()[0].id,
        id
    );
    assert!(store.link_workspace_items(a, id, foreign, "derived_from").is_err());
    store.link_items(id, foreign, "legacy").unwrap();
    store.link_workspace_items(a, id, linked, "related").unwrap();
    assert_eq!(store.workspace_links(a, id).unwrap().len(), 1);
    store.unlink_workspace_items(a, id, linked, "related").unwrap();
    assert!(store.workspace_links(a, id).unwrap().is_empty());
}

#[test]
fn legacy_mutations_also_increment_revision() {
    let store = Store::open_in_memory().unwrap();
    let p = store.create_project("A").unwrap();
    let id = create(&store, p, "old");
    store.update_item(id, None, Some("new"), None, 100).unwrap();
    assert_eq!(store.item(id).unwrap().unwrap().revision, 2);
    store.delete_item(id, 100).unwrap();
    assert_eq!(store.workspace_item(p, id, true).unwrap().revision, 3);
}

#[test]
fn v6_migrates_to_v7_without_changing_item_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v6.db");
    let store = Store::open(&path).unwrap();
    let p = store.create_project("A").unwrap();
    let id = create(&store, p, "text păstrat\n");
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("DROP TABLE IF EXISTS reminder_inbox; DROP TABLE IF EXISTS reminders; DROP TABLE IF EXISTS workspace_proposals; DROP TABLE IF EXISTS workspace_turn_contexts; ALTER TABLE items DROP COLUMN revision; PRAGMA user_version=6;").unwrap();
    drop(connection);
    let store = Store::open(&path).unwrap();
    let item = store.workspace_item(p, id, false).unwrap();
    assert_eq!(item.body, "text păstrat\n");
    assert_eq!(item.revision, 1);
    let connection = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        14
    );
    for table in [
        "workspace_proposals",
        "workspace_turn_contexts",
        "reminders",
        "reminder_inbox",
    ] {
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1,
            "{table}"
        );
    }
}
