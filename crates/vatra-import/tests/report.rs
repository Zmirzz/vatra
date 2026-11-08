mod support;

use std::path::PathBuf;

use serde_json::Value;
use support::{MARKER, codex_plugin, codex_plugin_cache, dirs, synthetic_home, write_text};
use vatra_extensions::{Origin, Route, RunsCode, Scope, Verdict};
use vatra_import::{
    Found, FoundKind, GuardedFs, ImportReport, ProbeResult, Probes, ReportItem, build_report, dedupe, scan_claude,
    scan_codex,
};

fn item(kind: FoundKind, name: &str, origin: Origin) -> Found {
    Found {
        kind,
        name: name.into(),
        origin,
        scope: Scope::Global,
        project: None,
        source: PathBuf::from(name),
        hash: "hash".into(),
        runs_code: Vec::new(),
        secrets: Vec::new(),
        unsupported: None,
        details: Value::Null,
        modified: None,
        also_from: Vec::new(),
    }
}

fn routes_of(report: &ImportReport, name: &str) -> Vec<(Route, Verdict)> {
    report
        .items
        .iter()
        .find(|report_item| report_item.name == name)
        .unwrap_or_else(|| panic!("no item named {name}"))
        .routes
        .to_vec()
}

fn verdict(routes: &[(Route, Verdict)], route: Route) -> Verdict {
    routes.iter().find(|(r, _)| *r == route).unwrap().1.clone()
}

#[test]
fn plain_skill_works_everywhere() {
    let skill = item(FoundKind::Skill, "plain", Origin::Claude);

    let report = build_report(&[skill], &Probes::default());

    let routes = routes_of(&report, "plain");
    assert_eq!(verdict(&routes, Route::Engine), Verdict::Works);
    assert_eq!(verdict(&routes, Route::Claude), Verdict::Works);
    // The delivery table keeps the ACP route as `Limited` until an agent is verified. A plain skill is still not
    // blocked there: it is neither unavailable nor waiting for input.
    assert!(matches!(verdict(&routes, Route::Acp), Verdict::Limited(_)));
}

#[test]
fn python_mcp_needs_input() {
    let mut server = item(FoundKind::McpServer, "scripts", Origin::Project);
    server.runs_code = vec![RunsCode {
        what: "MCP server".into(),
        command: "python server.py".into(),
    }];
    let mut probes = Probes::default();
    probes
        .programs
        .insert("python".into(), ProbeResult::Missing("python was not found".into()));

    let report = build_report(&[server], &probes);

    let routes = routes_of(&report, "scripts");
    for route in [Route::Engine, Route::Claude, Route::Acp] {
        assert_eq!(
            verdict(&routes, route),
            Verdict::NeedsInput("python was not found".into()),
            "{route:?}"
        );
    }
}

#[test]
fn unknown_field_is_limited() {
    let mut skill = item(FoundKind::Skill, "newer", Origin::Claude);
    skill.details = serde_json::json!({ "description": "Newer", "unknown_fields": ["model_hint"] });

    let report = build_report(&[skill], &Probes::default());

    let routes = routes_of(&report, "newer");
    assert_eq!(
        verdict(&routes, Route::Engine),
        Verdict::Limited("Vatra doesn't read these fields: model_hint".into())
    );
}

#[test]
fn account_connector_unavailable_with_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_text(
        &home.join(".codex").join("config.toml"),
        "[plugins.\"connected@market\"]\nenabled = true\n",
    );
    let root = codex_plugin_cache(home, "market", "connected", "1.0.0");
    codex_plugin(&root, "connected", "tool");
    write_text(&root.join(".app.json"), "{}");
    let scanned = scan_codex(&GuardedFs::new(), &dirs(home), &[]);

    let report = build_report(&dedupe(scanned.found).items, &Probes::default());

    let routes = routes_of(&report, "connected:.app.json");
    let reason = "Account connectors are tied to the ChatGPT account and don't move to Vatra";
    for route in [Route::Engine, Route::Claude, Route::Acp] {
        assert_eq!(
            verdict(&routes, route),
            Verdict::Unavailable(reason.into()),
            "{route:?}"
        );
    }
}

#[test]
fn report_never_contains_secret_values() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("work").join("app");
    synthetic_home(&home, &project);
    write_text(
        &home.join(".codex").join("config.toml"),
        &format!("[mcp_servers.local]\ncommand = \"node\"\nenv = {{ KEY = \"{MARKER}codex\" }}\n"),
    );
    let fs = GuardedFs::new();
    let mut found = scan_claude(&fs, &dirs(&home), std::slice::from_ref(&project)).found;
    found.extend(scan_codex(&fs, &dirs(&home), std::slice::from_ref(&project)).found);
    let items = dedupe(found).items;

    let report = build_report(&items, &Probes::default());
    let text = serde_json::to_string(&report).unwrap();

    assert!(!report.items.is_empty());
    assert!(!text.contains(MARKER));
    let needs_value: Vec<&ReportItem> = report
        .items
        .iter()
        .filter(|report_item| {
            report_item
                .routes
                .iter()
                .any(|(_, verdict)| matches!(verdict, Verdict::NeedsInput(_)))
        })
        .collect();
    assert!(!needs_value.is_empty());
}
