use std::fs;

use vatra_extensions::{
    CatalogEntry, ElementKind, Origin, Route, Scope, Verdict, read_plugin, unsupported_verdict, verdict,
};

const KINDS: [ElementKind; 6] = [
    ElementKind::Plugin,
    ElementKind::Skill,
    ElementKind::Command,
    ElementKind::McpServer,
    ElementKind::Instructions,
    ElementKind::Memory,
];
const ORIGINS: [Origin; 4] = [Origin::Claude, Origin::Codex, Origin::Project, Origin::Manual];
const ROUTES: [Route; 3] = [Route::Engine, Route::Claude, Route::Acp];

fn entry(origin: Origin, kind: ElementKind) -> CatalogEntry {
    CatalogEntry {
        id: 1,
        kind,
        name: "item".into(),
        origin,
        scope: Scope::Global,
        content_hash: "hash".into(),
        content_dir: "dir".into(),
        enabled: true,
        runs_code: Vec::new(),
        approved_hash: None,
        plugin: None,
        details: serde_json::json!({}),
    }
}

/// The delivery table: what each route does with each element, by where the element comes from.
fn expected(origin: Origin, kind: ElementKind, route: Route) -> Verdict {
    let foreign_skill = |text: &str| Verdict::Limited(text.into());
    match route {
        Route::Engine => Verdict::Works,
        Route::Claude => match (origin, kind) {
            (Origin::Claude, _) => Verdict::Works,
            (_, ElementKind::Skill | ElementKind::Command) => foreign_skill("not yet verified on Claude Code"),
            _ => Verdict::Works,
        },
        Route::Acp => match kind {
            ElementKind::Plugin => Verdict::Works,
            _ => foreign_skill("not yet verified on this agent"),
        },
    }
}

#[test]
fn route_table() {
    for origin in ORIGINS {
        for kind in KINDS {
            for route in ROUTES {
                let got = verdict(&entry(origin, kind), route);
                assert_eq!(got, expected(origin, kind, route), "{origin:?} {kind:?} {route:?}");
            }
        }
    }
}

#[test]
fn account_connectors_only_native() {
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join(".app.json"), "{}").unwrap();
    let bundle = read_plugin(source.path()).unwrap();
    let connector = bundle
        .unsupported
        .iter()
        .find(|component| component.kind == ".app.json")
        .unwrap();

    let verdict = unsupported_verdict(connector);

    assert_eq!(
        verdict,
        Verdict::Unavailable("Account connectors are tied to the ChatGPT account and don't move to Vatra".into())
    );
}
