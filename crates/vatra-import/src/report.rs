use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use vatra_extensions::{CatalogEntry, ElementKind, Origin, Route, Verdict, verdict};

use crate::probe::{ProbeResult, Probes, needed_programs};
use crate::scan::{Found, FoundKind, ScanProblem};

const ROUTES: [Route; 3] = [Route::Engine, Route::Claude, Route::Acp];

/// What the import would do with each item, and the problems the scans kept.
#[derive(Debug, Serialize)]
pub struct ImportReport {
    pub items: Vec<ReportItem>,
    pub problems: Vec<ScanProblem>,
}

/// One item and its verdict on each route. `also_from` lists the other origins that hold the same content.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReportItem {
    pub warning: Option<String>,
    pub kind: FoundKind,
    pub name: String,
    pub origin: Origin,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub also_from: Vec<Origin>,
    pub routes: [(Route, Verdict); 3],
}

/// The report for `items`, with the checks in `probes`. The problems are left empty: the caller adds the problems
/// of the scans it ran.
///
/// The verdict of each route is the delivery table, except that: an item whose commands are not approved yet is
/// `NeedsApproval`; a missing program or value is `NeedsInput`; and an unknown field of a skill makes a working route
/// `Limited`. An approved item with nothing missing gets the route's normal verdict.
pub fn build_report(items: &[Found], probes: &Probes) -> ImportReport {
    ImportReport {
        items: items.iter().map(|item| report_item(item, probes)).collect(),
        problems: Vec::new(),
    }
}

fn report_item(item: &Found, probes: &Probes) -> ReportItem {
    ReportItem {
        warning: item
            .details
            .get("warning")
            .and_then(Value::as_str)
            .map(str::to_string),
        kind: item.kind,
        name: item.name.clone(),
        origin: item.origin,
        also_from: item.also_from.clone(),
        routes: ROUTES.map(|route| (route, route_verdict(item, route, probes))),
    }
}

fn route_verdict(item: &Found, route: Route, probes: &Probes) -> Verdict {
    if let Some(reason) = &item.unsupported {
        return Verdict::Unavailable(reason.clone());
    }
    if let Some(reason) = awaiting_approval(item, probes) {
        return Verdict::NeedsApproval(reason);
    }
    if let Some(reason) = missing(item, probes) {
        return Verdict::NeedsInput(reason);
    }
    let base = match element_kind(item.kind) {
        Some(kind) => verdict(&entry(item, kind), route),
        None => Verdict::Works,
    };
    match base {
        Verdict::Works
            if item
                .details
                .get("warning")
                .and_then(Value::as_str)
                .is_some() =>
        {
            Verdict::Limited(item.details["warning"].as_str().unwrap_or_default().into())
        }
        Verdict::Works => match unknown_fields(item) {
            Some(reason) => Verdict::Limited(reason),
            None => Verdict::Works,
        },
        other => other,
    }
}

/// The item's source and name: how the probes and approvals refer to it.
fn key(item: &Found) -> (PathBuf, String) {
    (item.source.clone(), item.name.clone())
}

/// Why the item cannot run until the user approves its code: a stdio MCP server, or a command that starts a program
/// by path. None once the item is approved, or when nothing it runs needs approval.
fn awaiting_approval(item: &Found, probes: &Probes) -> Option<String> {
    if probes.approved.contains(&key(item)) {
        return None;
    }
    if let Some(ProbeResult::NotChecked(message)) = probes.servers.get(&key(item)) {
        return Some(message.clone());
    }
    needed_programs(item)
        .into_iter()
        .find_map(|program| match probes.programs.get(&program.name) {
            Some(ProbeResult::NotChecked(message)) => Some(message.clone()),
            _ => None,
        })
}

/// What the item needs that is not there: a value that was written into the file, a program that does not run, or an
/// MCP server's value. None when nothing is missing.
fn missing(item: &Found, probes: &Probes) -> Option<String> {
    if !item.secrets.is_empty() {
        let keys: Vec<&str> = item
            .secrets
            .iter()
            .map(|secret| secret.key.as_str())
            .collect();
        return Some(format!(
            "{} needs a value for {}",
            item.name,
            keys.join(", ")
        ));
    }
    for program in needed_programs(item) {
        if let Some(ProbeResult::Missing(message)) = probes.programs.get(&program.name) {
            return Some(message.clone());
        }
    }
    match probes.servers.get(&key(item)) {
        Some(ProbeResult::NeedsValue(message)) => Some(message.clone()),
        _ => None,
    }
}

/// The fields of a skill that Vatra does not read, named in the report.
fn unknown_fields(item: &Found) -> Option<String> {
    let fields = match item.details.get("unknown_fields") {
        Some(Value::Array(fields)) if !fields.is_empty() => fields,
        _ => return None,
    };
    let names: Vec<&str> = fields.iter().filter_map(Value::as_str).collect();
    Some(format!(
        "Vatra doesn't read these fields: {}",
        names.join(", ")
    ))
}

/// The delivery table's element for a found item. Permissions and plugin settings are not elements: Vatra reads them
/// itself, so they have no route to check.
fn element_kind(kind: FoundKind) -> Option<ElementKind> {
    match kind {
        FoundKind::Plugin => Some(ElementKind::Plugin),
        FoundKind::Skill => Some(ElementKind::Skill),
        FoundKind::Command => Some(ElementKind::Command),
        FoundKind::McpServer => Some(ElementKind::McpServer),
        FoundKind::Instructions => Some(ElementKind::Instructions),
        FoundKind::Memory => Some(ElementKind::Memory),
        FoundKind::Permission
        | FoundKind::PluginConfig
        | FoundKind::Hook
        | FoundKind::Agent
        | FoundKind::Unsupported => None,
    }
}

fn entry(item: &Found, kind: ElementKind) -> CatalogEntry {
    CatalogEntry {
        id: 0,
        kind,
        name: item.name.clone(),
        origin: item.origin,
        scope: item.scope,
        content_hash: item.hash.clone(),
        content_dir: item.source.clone(),
        enabled: true,
        runs_code: item.runs_code.clone(),
        approved_hash: None,
        plugin: None,
        details: item.details.clone(),
    }
}
