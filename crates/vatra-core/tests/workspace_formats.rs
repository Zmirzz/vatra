use serde::Deserialize;
use serde_json::json;
use vatra_core::{DocumentFormat, ItemChange, NewItem, Store, parse_workspace_csv, validate_document};
#[derive(Deserialize)]
struct Case {
    name: String,
    body: String,
    rows: Vec<Vec<String>>,
}

#[test]
fn csv_quotes_newlines_and_formulas_are_data() {
    let cases: Vec<Case> = serde_json::from_str(include_str!("../../../tests/fixtures/workspace-csv.json")).unwrap();
    for case in cases {
        assert_eq!(parse_workspace_csv(&case.body).unwrap(), case.rows, "{}", case.name);
    }
}
#[test]
fn malformed_or_unbounded_csv_is_refused() {
    for body in [
        "a,\"unterminated\n".into(),
        "a,b\nx\n".into(),
        "a\"b,c\n".into(),
        "x\n".repeat(10_001),
        vec!["x"; 101].join(","),
        (vec!["x"; 100].join(",") + "\n").repeat(501),
    ] {
        assert!(parse_workspace_csv(&body).is_err(), "invalid CSV accepted");
    }
    assert!(validate_document(DocumentFormat::Text, &"x".repeat(2 * 1024 * 1024 + 1)).is_err());
}
#[test]
fn workspace_edit_rejects_malformed_csv_without_changing_body() {
    let store = Store::open_in_memory().unwrap();
    let p = store.create_project("A").unwrap();
    let id = store
        .create_item(
            &NewItem {
                kind: "document".into(),
                title: "Table".into(),
                body: "a,b\nx,y\n".into(),
                fields: json!({"format":"csv"}),
                project_id: Some(p),
            },
            1,
        )
        .unwrap();
    assert!(
        store
            .save_workspace_item(
                p,
                &ItemChange {
                    id,
                    expected_revision: 1,
                    title: None,
                    body: Some("a,b\nx\n".into()),
                    fields: None
                },
                2
            )
            .is_err()
    );
    assert_eq!(store.workspace_item(p, id, false).unwrap().revision, 1);
}
