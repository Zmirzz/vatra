//! Reads what Claude Code and Codex keep on this machine, and what the projects hold, through a guard that refuses
//! credential files, and reports what Vatra can run. Nothing here writes to the source folders.

mod apply;
mod claude;
mod codex;
mod context;
mod dedupe;
mod guard;
mod history;
mod probe;
mod project;
mod report;
mod scan;
mod view;

pub use apply::{
    ApplyOutcome, ImportChoice, ItemChoice, ItemResult, ItemState, apply, apply_reporting,
    approved_items, item_key,
};
pub use claude::scan_claude;
pub use codex::scan_codex;
pub use context::{claude_project_key, import_memory, map_project, merge_memory, token_like};
pub use dedupe::{Conflict, DedupeResult, dedupe};
pub use guard::{GuardError, GuardedFs, Walk, is_forbidden};
pub use history::{
    HistoryProject, ParsedLog, history_files, history_files_for, history_key, history_projects,
    import_log, parse_claude_log, parse_codex_log,
};
pub use probe::{Probe, ProbeResult, Probes, Program, SystemProbe, needed_programs, probe_items};
pub use project::scan_project;
pub use report::{ImportReport, ReportItem, build_report};
pub use scan::{Found, FoundKind, ProjectRef, ScanProblem, ScanResult, SourceDirs};
pub use view::{Plan, ScanItem, ScanView, SecretName, Status, approvals, plan, scan_view};
