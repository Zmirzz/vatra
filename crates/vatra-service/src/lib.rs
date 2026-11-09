pub mod deps;
pub mod setup;
pub mod parity;
pub mod parity_native;
mod agents;
mod picker_options;
mod approvals;
mod bridge;
pub mod chatgpt;
pub mod copilot;
pub mod oauth;
mod completion;
pub mod config;
pub mod connectors;
pub mod workspace;
pub mod reminders;
pub mod scheduler;
pub mod calendar;
pub mod email;
pub mod research;
pub mod semantic;
pub mod health;
pub mod limits;
pub mod memory;
pub mod ntfy;
pub mod telegram;
mod import;
pub mod market;
pub mod registry;
pub mod skills;
pub mod models;
pub mod mcp_proxy;
pub mod mcp_manager;
mod questions;
pub mod files;
mod rpc;
mod runtime;
mod service;

pub use agents::{AgentFactory, BridgeSettings, BuildRequest, ConfigAgents, TurnOptions, project_workspace, session_workspace};
pub use approvals::ServiceApprover;
pub use bridge::{bridge_response, internal_router, run_bridge, run_bridge_scoped, token_matches};
pub use import::{project_folders, scan_folders};
pub use mcp_proxy::{internal_mcp_router, run_mcp_proxy};
pub use questions::ServiceQuestioner;
pub use rpc::{origin_allowed, router};
pub use runtime::{
    RunningService, Runtime, RuntimeError, ServiceInfo, default_data_dir, default_models_dir,
    existing_service,
};
pub use service::{
    ApprovalRequest, McpServerStatus, Notification, PermissionMode, QuestionRequest, Service,
    ServiceError,
};
