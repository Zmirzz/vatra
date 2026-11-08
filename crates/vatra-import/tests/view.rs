use std::path::PathBuf;

use serde_json::{Value, json};
use vatra_extensions::{Origin, RunsCode, Scope, parse_mcp_json};
use vatra_import::{Found, FoundKind, Probes, ScanResult, plan, scan_view};

fn item(source: &str, command: &str) -> Found {
    Found {
        kind: FoundKind::McpServer,
        name: "files".into(),
        origin: Origin::Codex,
        scope: Scope::Global,
        project: None,
        source: PathBuf::from(source),
        hash: source.into(),
        runs_code: vec![RunsCode {
            what: "start".into(),
            command: command.into(),
        }],
        secrets: Vec::new(),
        unsupported: None,
        details: Value::Null,
        modified: None,
        also_from: Vec::new(),
    }
}

#[test]
fn scan_commands_hide_secret_flags_and_keep_quoted_paths_readable() {
    let command = r#"node "C:\Program Files\server.js" --token "private value" --api-key=another-value --port 3000"#;
    let plan = plan(&ScanResult {
        found: vec![item("one", command)],
        problems: Vec::new(),
    });

    let view = scan_view(&plan, &[], None, &Probes::default()).unwrap();

    assert_eq!(
        view.items[0].runs_code[0].command,
        r#"node "C:\Program Files\server.js" --token <redacted> --api-key=<redacted> --port 3000"#
    );
    assert_eq!(
        plan.items[0].runs_code[0].command, command,
        "the source plan is untouched"
    );
}

#[test]
fn conflict_payload_and_alternatives_never_serialize_details_or_literal_values() {
    let mut first = item("one", "node server.js --token=first-token");
    first.details = json!({"body":"PRIVATE-BODY", "env":{"TOKEN":"PRIVATE-LITERAL"}});
    first.secrets = parse_mcp_json(
        r#"{"mcpServers":{"files":{"command":"node","env":{"TOKEN":"PRIVATE-LITERAL"}}}}"#,
    )
    .unwrap()
    .literal_secrets;
    let mut alternate = item("two", "node other.js --password='second private token'");
    alternate.details = json!({"nested":{"token":"PRIVATE-ALTERNATIVE"}});
    let plan = plan(&ScanResult {
        found: vec![first, alternate],
        problems: Vec::new(),
    });

    let view = scan_view(&plan, &[], None, &Probes::default()).unwrap();
    let serialized = serde_json::to_string(&view).unwrap();

    for value in [
        "PRIVATE-BODY",
        "PRIVATE-LITERAL",
        "PRIVATE-ALTERNATIVE",
        "first-token",
        "second private token",
    ] {
        assert!(!serialized.contains(value), "scan payload contains {value}");
    }
    assert!(
        serialized.contains("TOKEN"),
        "only the name of the needed value is shown"
    );
    assert_eq!(view.alternatives.len(), 1);
    assert_eq!(view.conflicts.len(), 1);
    assert_eq!(view.conflicts[0].kept.source, PathBuf::from("one"));
    assert_eq!(view.conflicts[0].dropped[0].source, PathBuf::from("two"));
}
