use vatra_core::{ConnectorRow, Store};

fn row(id: &str, name: &str, settings_json: &str, created_at: i64) -> ConnectorRow {
    ConnectorRow {
        id: id.into(),
        kind: "fake".into(),
        name: name.into(),
        settings_json: settings_json.into(),
        created_at,
    }
}

#[test]
fn connectors_are_listed_in_creation_order_and_removed() {
    let store = Store::open_in_memory().unwrap();
    store.save_connector(&row("b", "Second", "{}", 2)).unwrap();
    store.save_connector(&row("a", "First", "{}", 1)).unwrap();
    let ids: Vec<String> = store.connectors().unwrap().into_iter().map(|row| row.id).collect();
    assert_eq!(ids, ["a", "b"]);
    assert!(store.remove_connector("a").unwrap());
    assert!(!store.remove_connector("a").unwrap());
    assert_eq!(store.connectors().unwrap(), vec![row("b", "Second", "{}", 2)]);
}

#[test]
fn saving_again_replaces_the_row_and_keeps_its_created_at() {
    let store = Store::open_in_memory().unwrap();
    store.save_connector(&row("a", "First", "{\"host\":\"one\"}", 1)).unwrap();
    store.save_connector(&row("a", "Renamed", "{\"host\":\"two\"}", 99)).unwrap();
    let rows = store.connectors().unwrap();
    assert_eq!(rows, vec![row("a", "Renamed", "{\"host\":\"two\"}", 1)]);
}
