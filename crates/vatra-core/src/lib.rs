mod agent;
mod app_tables;
mod audit;
mod connectors;
mod event;
mod handoff;
mod history;
mod local_models;
mod mcp;
mod permissions;
mod process;
mod secrets;
mod store;
mod files;
pub use files::{FileRecord, FileVersion, NewFile, MAX_FILE_BYTES, FILE_CHUNK_BYTES};
mod workspace;
mod workspace_versions;
mod workspace_proposals;
mod workspace_formats;
mod workspace_ai;
mod reminders;
mod schedule;
mod semantic;
pub use semantic::{SemanticHit, SemanticSource, SemanticStats, chunk_text};
mod project_delete;
pub use project_delete::ProjectRemoval;
mod email_store;
mod research_store;
pub use research_store::{CompareEntry,CompareRun,ResearchRun};
pub use email_store::{EmailDraft,EmailMeta,NewEmailDraft,TRIAGE_CLASSES,valid_address,valid_label};
pub use schedule::{civil_from_days,days_from_civil,CalendarEvent,NewCalendarEvent,NewScheduledTask,Occurrence,Schedule,ScheduledRun,ScheduledTask,expand as expand_recurrence,format_ical_time,parse_ical_time,parse_rrule};
pub use reminders::{Reminder,ReminderResult};
pub use workspace_ai::{Selection, WorkspaceTurnContext, WorkspaceTaskInput};

pub use app_tables::{ConfiguredAgentRow, ParityRunRow};
pub use audit::AuditEntry;
pub use connectors::ConnectorRow;
pub use agent::{Agent, AgentError, RunError, TurnOutcome, TurnRequest, TurnFile, run_turn, run_turn_with_files, input_with_files};
pub use event::{Event, HandoffBrief, StoredEvent};
pub use handoff::{
    BriefFields, Summarizer, build_brief, create_handoff_session, handoff, history_transcript,
    relevant_files, render_handoff_prompt, run_handoff, transcript,
};
pub use local_models::{LocalModelRow, ModelCheckRow};
pub use history::{HistoryHit, HistoryMessage, ImportedSession, ImportedSessionInfo, SessionRef};
pub use mcp::{McpOAuth, McpServerConfig, PluginContext, SecretRef, claude_mcp_config};
pub use permissions::{PermissionMode, Rule, Rules};
pub use process::{ProcessGroup, find_bash, node_package_runner};
pub use rusqlite::Error as SqliteError;
pub use secrets::{
    ChunkedStore, EntryBackend, KeyringBackend, KeyringStore, MemorySecretStore, SecretError,
    SecretStore,
};
pub use store::{LastTurn, Project, SessionInfo, Store, StoreError};
pub use workspace::{ITEM_KINDS, Item, ItemHit, NewItem};
pub use workspace_versions::{WorkspaceError, ItemChange, DocumentFormat, DocumentFields, TaskFields, TaskStatus};
pub use workspace_proposals::{WorkspaceProposal, ProposalPayload, ProposalState, ProposalResult};
pub use workspace_formats::{MAX_DOCUMENT_BYTES,parse_workspace_csv,validate_document,safe_csv_export};
pub use workspace_versions::validate_workspace_content;
pub use workspace_versions::WorkspaceItemPreview;
pub use workspace_versions::TaskView;
