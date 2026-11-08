//! What an import shows: each item with its verdict, whether the catalog has it as new, changed or unchanged, the
//! project it goes to, and the secrets it needs by name. A value is never in this view.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Serialize;
use vatra_extensions::{Catalog, RunsCode, Verdict};

use crate::apply::{catalog_kind, item_key, scope_of, source_hash_of, stored_matches};
use crate::dedupe::{Conflict, dedupe};
use crate::probe::Probes;
use crate::report::{ReportItem, build_report};
use crate::scan::{Found, FoundKind, ProjectRef, ScanProblem, ScanResult};

/// The items of a scan after duplicates are merged, with the conflicts and problems the scan kept.
#[derive(Debug)]
pub struct Plan {
    pub items: Vec<Found>,
    pub conflicts: Vec<Conflict>,
    pub problems: Vec<ScanProblem>,
}

pub fn plan(scan: &ScanResult) -> Plan {
    let merged = dedupe(scan.found.clone());
    Plan {
        items: merged.items,
        conflicts: merged.conflicts,
        problems: scan.problems.clone(),
    }
}

/// Whether the catalog has an item, and if so whether its source changed since.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    New,
    Changed,
    Unchanged,
}

/// One secret an item needs. Only its names are here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SecretName {
    pub server: String,
    pub key: String,
}

/// One item of the import, as the app lists it.
#[derive(Clone, Debug, Serialize)]
pub struct ScanItem {
    pub runs_code: Vec<RunsCode>,
    pub key: String,
    pub source: PathBuf,
    /// The catalog project the item goes to, when it is one.
    pub project: Option<i64>,
    /// False when the item belongs to a project Vatra doesn't know. The item is listed, and not applied.
    pub project_known: bool,
    /// None when Vatra doesn't keep the item at all.
    pub status: Option<Status>,
    pub secrets: Vec<SecretName>,
    pub report: ReportItem,
}

/// A conflicting copy's location. Raw configuration and bodies never travel to the preview.
#[derive(Debug, Serialize)]
pub struct ScanSource {
    pub source: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct ScanConflict {
    pub kind: FoundKind,
    pub name: String,
    pub kept: ScanSource,
    pub dropped: Vec<ScanSource>,
}

/// The whole import as the app shows it.
#[derive(Debug, Serialize)]
pub struct ScanView {
    pub alternatives: Vec<ScanItem>,
    pub items: Vec<ScanItem>,
    pub conflicts: Vec<ScanConflict>,
    pub problems: Vec<ScanProblem>,
}

/// The items whose code the catalog already approves for their current content. The probes take these as approved.
pub fn approvals(
    plan: &Plan,
    projects: &[Option<i64>],
    catalog: Option<&Catalog>,
) -> Result<std::collections::BTreeSet<(PathBuf, String)>, String> {
    match catalog {
        Some(catalog) => crate::apply::approved_items(&plan.items, projects, catalog),
        None => Ok(Default::default()),
    }
}

/// The view of a plan: the verdicts from `probes`, and each item's status in the catalog. `projects` maps each project
/// the scan was given, by position, to its catalog id.
pub fn scan_view(
    plan: &Plan,
    projects: &[Option<i64>],
    catalog: Option<&Catalog>,
    probes: &Probes,
) -> Result<ScanView, String> {
    let report = build_report(&plan.items, probes);
    let (entries, sources) = match catalog {
        Some(catalog) => (
            catalog.list().map_err(|error| error.to_string())?,
            catalog.source_hashes().map_err(|error| error.to_string())?,
        ),
        None => (Vec::new(), HashMap::new()),
    };
    let matches = stored_matches(&plan.items, projects, &entries);

    let items = plan
        .items
        .iter()
        .zip(report.items)
        .map(|(item, mut report)| {
            for (_, verdict) in &mut report.routes {
                match verdict {
                    Verdict::Works => {}
                    Verdict::Limited(reason)
                    | Verdict::NeedsInput(reason)
                    | Verdict::NeedsApproval(reason)
                    | Verdict::Unavailable(reason) => {
                        *reason = redact_values(reason, item);
                    }
                }
            }
            let key = item_key(item);
            let status = catalog_kind(item.kind).map(|_| match matches.get(&key) {
                None => Status::New,
                Some(entry)
                    if sources.get(&entry.id).map(String::as_str)
                        == Some(source_hash_of(item, &plan.items)) =>
                {
                    Status::Unchanged
                }
                // A row with no source hash is from a database before the hash existed: nothing is imported yet.
                Some(entry) if sources.contains_key(&entry.id) => Status::Changed,
                Some(_) => Status::New,
            });
            let project = match item.project {
                Some(ProjectRef(index)) => projects.get(index).copied().flatten(),
                None => None,
            };
            ScanItem {
                runs_code: item
                    .runs_code
                    .iter()
                    .map(|run| RunsCode {
                        what: redact_values(&run.what, item),
                        command: redact_command(&run.command, item),
                    })
                    .collect(),
                key,
                source: item.source.clone(),
                project,
                project_known: scope_of(item, projects).is_ok(),
                status,
                secrets: item
                    .secrets
                    .iter()
                    .map(|secret| SecretName {
                        server: secret.server.clone(),
                        key: secret.key.clone(),
                    })
                    .collect(),
                report,
            }
        })
        .collect();

    let mut alternatives = Vec::new();
    if !plan.conflicts.is_empty() {
        let alternate_plan = Plan {
            items: plan
                .conflicts
                .iter()
                .flat_map(|conflict| conflict.dropped.clone())
                .collect(),
            conflicts: Vec::new(),
            problems: Vec::new(),
        };
        alternatives = scan_view(&alternate_plan, projects, catalog, probes)?.items;
    }
    Ok(ScanView {
        alternatives,
        items,
        conflicts: plan
            .conflicts
            .iter()
            .map(|conflict| ScanConflict {
                kind: conflict.kind,
                name: conflict.name.clone(),
                kept: ScanSource {
                    source: conflict.kept.source.clone(),
                },
                dropped: conflict
                    .dropped
                    .iter()
                    .map(|item| ScanSource {
                        source: item.source.clone(),
                    })
                    .collect(),
            })
            .collect(),
        problems: plan.problems.clone(),
    })
}

fn redact_values(text: &str, item: &Found) -> String {
    let mut values: Vec<&str> = item
        .secrets
        .iter()
        .map(|secret| secret.value())
        .filter(|value| !value.is_empty())
        .collect();
    values.sort_unstable_by_key(|value| std::cmp::Reverse(value.len()));
    values.into_iter().fold(text.to_owned(), |text, value| {
        text.replace(value, "<redacted>")
    })
}

fn sensitive_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["key", "token", "secret", "password", "authorization"]
        .iter()
        .any(|part| name.contains(part))
}

/// Keep the command readable, including Windows paths and quoted arguments. A secret-looking flag's
/// complete next argument is hidden, even if it contains spaces; literal values are hidden everywhere.
fn redact_command(command: &str, item: &Found) -> String {
    let mut spans = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in command.char_indices() {
        if start.is_none() && !ch.is_whitespace() {
            start = Some(index);
        }
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if Some(ch) == quote {
            quote = None;
        } else if quote.is_none() && (ch == '\'' || ch == '"') {
            quote = Some(ch);
        } else if quote.is_none()
            && ch.is_whitespace()
            && let Some(begin) = start.take()
        {
            spans.push((begin, index));
        }
    }
    if let Some(begin) = start {
        spans.push((begin, command.len()));
    }
    let mut output = String::new();
    let mut previous = 0;
    let mut hide_next = false;
    for (begin, end) in spans {
        output.push_str(&command[previous..begin]);
        let word = &command[begin..end];
        let plain = word.trim_matches(['\'', '"']);
        if std::mem::take(&mut hide_next) {
            output.push_str("<redacted>");
        } else if let Some((name, _)) = plain
            .split_once('=')
            .filter(|(name, _)| sensitive_name(name))
        {
            output.push_str(name);
            output.push_str("=<redacted>");
        } else if plain.to_ascii_lowercase().starts_with("authorization:") {
            output.push_str("<redacted>");
        } else {
            output.push_str(word);
            hide_next = (plain.starts_with('-') && sensitive_name(plain))
                || plain.eq_ignore_ascii_case("bearer");
        }
        previous = end;
    }
    output.push_str(&command[previous..]);
    redact_values(&output, item)
}
