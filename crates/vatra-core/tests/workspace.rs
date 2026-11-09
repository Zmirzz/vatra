use rusqlite::Connection;
use serde_json::json;
use vatra_core::{ITEM_KINDS, Item, NewItem, Store};

fn new_item(kind: &str, title: &str) -> NewItem {
    NewItem {
        kind: kind.into(),
        title: title.into(),
        body: String::new(),
        fields: json!({}),
        project_id: None,
    }
}

fn count(path: &std::path::Path, sql: &str) -> i64 {
    let connection = Connection::open(path).unwrap();
    connection.query_row(sql, [], |row| row.get(0)).unwrap()
}

#[test]
fn create_read_update_and_soft_delete() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("elemente.db");
    let store = Store::open(&path).unwrap();
    let id = store
        .create_item(
            &NewItem {
                kind: "note".into(),
                title: "Ședință".into(),
                body: "bugetul trimestrului".into(),
                fields: json!({"tags": ["buget"]}),
                project_id: None,
            },
            100,
        )
        .unwrap();
    let item: Item = store.item(id).unwrap().unwrap();
    assert_eq!(item.kind, "note");
    assert_eq!(item.title, "Ședință");
    assert_eq!(item.body, "bugetul trimestrului");
    assert_eq!(item.fields, json!({"tags": ["buget"]}));
    assert_eq!(item.project_id, None);
    assert_eq!(item.created_at, 100);
    assert_eq!(item.updated_at, 100);
    assert_eq!(count(&path, "SELECT COUNT(*) FROM items_fts WHERE item_id = 1"), 1);

    assert!(store
        .update_item(id, Some("Ședință nouă"), None, Some(&json!({"done": true})), 200)
        .unwrap());
    let item = store.item(id).unwrap().unwrap();
    assert_eq!(item.title, "Ședință nouă");
    assert_eq!(item.body, "bugetul trimestrului", "None keeps the body");
    assert_eq!(item.fields, json!({"done": true}));
    assert_eq!(item.created_at, 100);
    assert_eq!(item.updated_at, 200);
    assert_eq!(count(&path, "SELECT COUNT(*) FROM items_fts"), 1, "update replaces the index row");
    assert_eq!(store.items_by_kind("note", 10).unwrap().len(), 1);

    assert!(store.delete_item(id, 300).unwrap());
    assert!(store.item(id).unwrap().is_none());
    assert!(store.items_by_kind("note", 10).unwrap().is_empty());
    assert_eq!(count(&path, "SELECT COUNT(*) FROM items_fts"), 0, "a deleted item leaves the index");
    assert_eq!(
        count(&path, "SELECT COUNT(*) FROM items WHERE deleted_at = 300"),
        1,
        "the row stays, marked deleted"
    );
    assert!(!store.delete_item(id, 301).unwrap(), "already deleted");
    assert!(!store.update_item(id, Some("x"), None, None, 302).unwrap(), "deleted items do not update");
    assert!(!store.delete_item(999, 303).unwrap(), "missing id");
    assert!(!store.update_item(999, Some("x"), None, None, 304).unwrap(), "missing id");
}

#[test]
fn items_by_kind_lists_newest_first_and_respects_the_limit() {
    let store = Store::open_in_memory().unwrap();
    let first = store.create_item(&new_item("task", "a"), 1).unwrap();
    let second = store.create_item(&new_item("task", "b"), 2).unwrap();
    store.create_item(&new_item("event", "c"), 3).unwrap();
    let listed: Vec<i64> = store.items_by_kind("task", 10).unwrap().iter().map(|i| i.id).collect();
    assert_eq!(listed, vec![second, first]);
    assert_eq!(store.items_by_kind("task", 1).unwrap().len(), 1);
}

#[test]
fn every_listed_kind_is_accepted() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(ITEM_KINDS.len(), 6);
    for kind in ITEM_KINDS {
        store.create_item(&new_item(kind, "t"), 1).unwrap();
    }
}

#[test]
fn an_unknown_kind_is_refused() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.create_item(&new_item("spreadsheet", "t"), 1).is_err());
    assert!(store.items_by_kind("spreadsheet", 10).unwrap().is_empty());
}

#[test]
fn links_connect_items_both_ways() {
    let store = Store::open_in_memory().unwrap();
    let note = store.create_item(&new_item("note", "n"), 1).unwrap();
    let task = store.create_item(&new_item("task", "t"), 1).unwrap();
    store.link_items(note, task, "despre").unwrap();
    store.link_items(note, task, "despre").unwrap();

    let from_note = store.linked_items(note).unwrap();
    assert_eq!(from_note.len(), 1);
    assert_eq!(from_note[0].0, "despre");
    assert_eq!(from_note[0].1.id, task);

    let from_task = store.linked_items(task).unwrap();
    assert_eq!(from_task.len(), 1);
    assert_eq!(from_task[0].0, "despre");
    assert_eq!(from_task[0].1.id, note);

    store.delete_item(task, 2).unwrap();
    assert!(store.linked_items(note).unwrap().is_empty(), "soft-deleted items are not returned");
}

#[test]
fn a_database_at_version_5_gains_the_workspace_tables() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v5.db");
    drop(Store::open(&path).unwrap());
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "DROP TABLE item_links; DROP TABLE items_fts; DROP TABLE items; PRAGMA user_version = 5;",
            )
            .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let id = store.create_item(&new_item("document", "după migrare"), 1).unwrap();
    assert!(store.item(id).unwrap().is_some());
    drop(store);
    assert_eq!(count(&path, "PRAGMA user_version"), 15);
    assert_eq!(count(&path, "SELECT COUNT(*) FROM item_links"), 0);
}

#[test]
fn a_very_long_body_is_kept() {
    let store = Store::open_in_memory().unwrap();
    let body = "x".repeat(1 << 20);
    let id = store
        .create_item(
            &NewItem {
                kind: "document".into(),
                title: "lung".into(),
                body: body.clone(),
                fields: json!({}),
                project_id: None,
            },
            1,
        )
        .unwrap();
    assert_eq!(store.item(id).unwrap().unwrap().body, body);
}

fn note_with_body(title: &str, body: &str) -> NewItem {
    NewItem {
        kind: "note".into(),
        title: title.into(),
        body: body.into(),
        fields: json!({}),
        project_id: None,
    }
}

#[test]
fn search_finds_titles_and_bodies() {
    let store = Store::open_in_memory().unwrap();
    let note = store.create_item(&note_with_body("Planul Andreei", "bugetul trimestrului"), 10).unwrap();
    let task = store.create_item(&new_item("task", "Trimite bugetul"), 20).unwrap();

    let by_title = store.search_workspace("planul", &[], 10).unwrap();
    assert_eq!(by_title.len(), 1);
    assert_eq!(by_title[0].id, note);
    assert_eq!(by_title[0].kind, "note");
    assert_eq!(by_title[0].title, "Planul Andreei");
    assert_eq!(by_title[0].updated_at, 10);
    assert!(by_title[0].snippet.contains("Planul"), "snippet: {}", by_title[0].snippet);

    let by_body = store.search_workspace("trimestrului", &[], 10).unwrap();
    assert_eq!(by_body.iter().map(|h| h.id).collect::<Vec<_>>(), vec![note]);

    let mut both: Vec<i64> = store.search_workspace("bugetul", &[], 10).unwrap().iter().map(|h| h.id).collect();
    both.sort();
    let mut expected = vec![note, task];
    expected.sort();
    assert_eq!(both, expected);
}

#[test]
fn search_ignores_deleted_items() {
    let store = Store::open_in_memory().unwrap();
    let kept = store.create_item(&new_item("note", "Ascuns ieri"), 1).unwrap();
    let gone = store.create_item(&new_item("note", "Ascuns azi"), 2).unwrap();
    assert_eq!(store.search_workspace("ascuns", &[], 10).unwrap().len(), 2);

    assert!(store.delete_item(gone, 3).unwrap());
    let hits = store.search_workspace("ascuns", &[], 10).unwrap();
    assert_eq!(hits.iter().map(|h| h.id).collect::<Vec<_>>(), vec![kept]);
    assert!(store.search_workspace("azi", &[], 10).unwrap().is_empty());
}

#[test]
fn search_matches_romanian_text_without_diacritics() {
    let store = Store::open_in_memory().unwrap();
    let id = store.create_item(&new_item("event", "Ședință în țară"), 1).unwrap();
    let found = |query: &str| -> Vec<i64> {
        store.search_workspace(query, &[], 10).unwrap().iter().map(|h| h.id).collect()
    };
    assert_eq!(found("sedinta"), vec![id]);
    assert_eq!(found("tara"), vec![id]);
    assert_eq!(found("Ședință"), vec![id]);
}

#[test]
fn search_survives_fts_syntax_and_empty_queries() {
    let store = Store::open_in_memory().unwrap();
    store.create_item(&new_item("note", "a AND b NEAR c"), 1).unwrap();
    let long = "word ".repeat(1000);
    assert_eq!(long.chars().count(), 5000);
    for query in ["\"", "a AND", "*", "NEAR(", "", "   ", long.as_str(), "' OR 1=1 --"] {
        assert!(store.search_workspace(query, &[], 10).is_ok(), "query {query:?} must not fail");
    }
    assert!(store.search_workspace("", &[], 10).unwrap().is_empty());
    assert!(store.search_workspace("   ", &[], 10).unwrap().is_empty());
    assert!(store.search_workspace("*", &[], 10).unwrap().is_empty());
}

#[test]
fn search_can_be_limited_to_kinds() {
    let store = Store::open_in_memory().unwrap();
    let note = store.create_item(&new_item("note", "Raport"), 1).unwrap();
    let task = store.create_item(&new_item("task", "Raport"), 2).unwrap();
    let document = store.create_item(&new_item("document", "Raport"), 3).unwrap();

    let mut two: Vec<i64> = store
        .search_workspace("raport", &["task", "document"], 10)
        .unwrap()
        .iter()
        .map(|h| h.id)
        .collect();
    two.sort();
    let mut expected = vec![task, document];
    expected.sort();
    assert_eq!(two, expected);
    assert_eq!(
        store.search_workspace("raport", &["note"], 10).unwrap().iter().map(|h| h.id).collect::<Vec<_>>(),
        vec![note]
    );
    assert_eq!(store.search_workspace("raport", &[], 10).unwrap().len(), 3, "empty kinds means all kinds");
    assert!(store.search_workspace("raport", &["note' OR '1'='1"], 10).is_err(), "invalid kinds are refused");
}

#[test]
fn search_rejects_unknown_or_unbounded_kinds_even_for_empty_queries() {
    let store = Store::open_in_memory().unwrap();
    for (query, limit) in [("report",10),("",10),("report",0)] {
        assert!(store.search_workspace(query,&["unknown"],limit).is_err());
        assert!(store.search_workspace(query,&["note";7],limit).is_err());
    }
}

#[test]
fn search_respects_the_limit_and_caps_it_at_100() {
    let store = Store::open_in_memory().unwrap();
    for i in 0..101 {
        store.create_item(&new_item("note", "raport"), i).unwrap();
    }
    assert!(store.search_workspace("raport", &[], 0).unwrap().is_empty());
    assert_eq!(store.search_workspace("raport", &[], 1).unwrap().len(), 1);
    assert_eq!(store.search_workspace("raport", &[], 500).unwrap().len(), 100);
}

#[test]
fn an_updated_item_is_found_by_its_new_text_only() {
    let store = Store::open_in_memory().unwrap();
    let id = store.create_item(&note_with_body("Vechi", "text vechi"), 1).unwrap();
    assert_eq!(store.search_workspace("vechi", &[], 10).unwrap().len(), 1);

    assert!(store.update_item(id, Some("Nou"), Some("text nou"), None, 2).unwrap());
    let found: Vec<i64> = store.search_workspace("nou", &[], 10).unwrap().iter().map(|h| h.id).collect();
    assert_eq!(found, vec![id]);
    assert!(store.search_workspace("vechi", &[], 10).unwrap().is_empty(), "old text is gone from the index");
}

fn read_lines(path: &std::path::Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.is_empty() || text.ends_with('\n'), "every line ends with a newline");
    text.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

#[test]
fn export_writes_one_line_per_live_item() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let kept_a = store.create_item(&new_item("note", "a"), 1).unwrap();
    let gone = store.create_item(&new_item("note", "șters"), 2).unwrap();
    let kept_b = store.create_item(&new_item("task", "b"), 3).unwrap();
    assert!(store.delete_item(gone, 4).unwrap());

    let path = dir.path().join("export.jsonl");
    let count = store.export_items_json(&path).unwrap();
    assert_eq!(count, 2);
    let lines = read_lines(&path);
    assert_eq!(lines.len(), count, "the count equals the lines");
    let ids: Vec<i64> = lines.iter().map(|line| line["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, vec![kept_a, kept_b], "ordered by id, deleted item absent");
    assert_eq!(lines[1]["kind"], "task");
    assert_eq!(lines[1]["title"], "b");
    assert_eq!(lines[1]["fields"], json!({}));
    assert_eq!(lines[1]["project_id"], serde_json::Value::Null);
    assert_eq!(lines[1]["created_at"], 3);
    assert_eq!(lines[1]["updated_at"], 3);
    assert_eq!(lines[1]["links"], json!([]));
}

#[test]
fn export_includes_links_in_both_directions() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let a = store.create_item(&new_item("note", "a"), 1).unwrap();
    let b = store.create_item(&new_item("task", "b"), 1).unwrap();
    let c = store.create_item(&new_item("document", "c"), 1).unwrap();
    let gone = store.create_item(&new_item("event", "gone"), 1).unwrap();
    store.link_items(a, b, "despre").unwrap();
    store.link_items(c, a, "citat").unwrap();
    store.link_items(a, gone, "legat").unwrap();
    assert!(store.delete_item(gone, 2).unwrap());

    let path = dir.path().join("links.jsonl");
    assert_eq!(store.export_items_json(&path).unwrap(), 3);
    let lines = read_lines(&path);
    let of_a = lines.iter().find(|line| line["id"] == a).unwrap();
    assert_eq!(
        of_a["links"],
        json!([
            {"rel": "despre", "to": b, "direction": "out"},
            {"rel": "citat", "to": c, "direction": "in"}
        ]),
        "links to deleted items are left out"
    );
    let of_b = lines.iter().find(|line| line["id"] == b).unwrap();
    assert_eq!(of_b["links"], json!([{"rel": "despre", "to": a, "direction": "in"}]));
}

#[test]
fn export_keeps_unicode_and_newlines() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let body = "ț\n\"quote\"\\";
    store.create_item(&note_with_body("Ședință", body), 1).unwrap();

    let path = dir.path().join("unicode.jsonl");
    assert_eq!(store.export_items_json(&path).unwrap(), 1);
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 1, "newlines inside a body are escaped");
    let lines = read_lines(&path);
    assert_eq!(lines[0]["body"], body);
    assert_eq!(lines[0]["title"], "Ședință");
}

#[test]
fn export_refuses_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    store.create_item(&new_item("note", "a"), 1).unwrap();
    let path = dir.path().join("exists.jsonl");
    std::fs::write(&path, "nu o atinge\n").unwrap();
    assert!(store.export_items_json(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "nu o atinge\n");
}

#[test]
fn export_and_backup_refuse_unexpected_extensions_before_creating_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    for name in ["export.exe", "export", "export.db", "export.jsonl.exe"] {
        let path = dir.path().join(name);
        assert!(store.export_items_json(&path).is_err(), "{name}");
        assert!(!path.exists());
    }
    for name in ["backup.exe", "backup", "backup.json", "backup.db.exe"] {
        let path = dir.path().join(name);
        assert!(store.backup_to(&path).is_err(), "{name}");
        assert!(!path.exists());
    }
    assert_eq!(store.export_items_json(&dir.path().join("export.JSON")).unwrap(), 0);
    store.backup_to(&dir.path().join("backup.DB")).unwrap();
}

#[test]
fn backup_opens_as_a_store_with_the_same_items() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("live.db")).unwrap();
    let note = store.create_item(&note_with_body("Copie", "text de păstrat"), 5).unwrap();
    let task = store.create_item(&new_item("task", "sarcină"), 6).unwrap();
    store.link_items(note, task, "despre").unwrap();

    let copy = dir.path().join("copy.db");
    store.backup_to(&copy).unwrap();
    let restored = Store::open(&copy).unwrap();
    let ids: Vec<i64> = restored.items_by_kind("note", 10).unwrap().iter().map(|i| i.id).collect();
    assert_eq!(ids, vec![note]);
    assert_eq!(restored.item(task).unwrap().unwrap().title, "sarcină");
    assert_eq!(restored.linked_items(note).unwrap().len(), 1);
    assert_eq!(restored.search_workspace("păstrat", &[], 10).unwrap().len(), 1);
    drop(restored);
    assert_eq!(count(&copy, "PRAGMA user_version"), 15);
}

#[test]
fn backup_refuses_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    store.create_item(&new_item("note", "a"), 1).unwrap();
    let path = dir.path().join("exists.db");
    std::fs::write(&path, "nu sunt bază de date").unwrap();
    assert!(store.backup_to(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "nu sunt bază de date");
}

#[test]
fn backup_path_with_quotes_and_spaces_works() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("a 'b' c");
    std::fs::create_dir(&folder).unwrap();
    let store = Store::open_in_memory().unwrap();
    store.create_item(&new_item("note", "a"), 1).unwrap();
    let copy = folder.join("copie 'nouă'.db");
    store.backup_to(&copy).unwrap();
    assert_eq!(Store::open(&copy).unwrap().items_by_kind("note", 10).unwrap().len(), 1);
}

fn leftovers(folder: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect()
}

#[test]
fn backup_leaves_no_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    store.create_item(&new_item("note", "a"), 1).unwrap();
    store.backup_to(&dir.path().join("ok.db")).unwrap();
    assert!(leftovers(dir.path()).is_empty(), "{:?}", leftovers(dir.path()));

    let taken = dir.path().join("taken.db");
    std::fs::write(&taken, "x").unwrap();
    assert!(store.backup_to(&taken).is_err());
    assert!(leftovers(dir.path()).is_empty(), "{:?}", leftovers(dir.path()));

    let missing = dir.path().join("nu există").join("b.db");
    assert!(store.backup_to(&missing).is_err());
    assert!(leftovers(dir.path()).is_empty());
}
