//! ACP v1 client for independently installed coding agents over newline-delimited JSON-RPC.
//! Protocol references and the intentionally limited capability set are in docs/acp.md.

mod adapter;
mod extras;
mod map;
mod transport;

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

pub use adapter::{AcpAgent, AcpChoice, AcpOptions, probe_command, probe_options};
pub use extras::{
    AcpExtras, AgentCommands, McpCapabilities, PermissionCheck, acp_mcp_servers, expand_prompt,
    first_prompt_preamble, permission_checks,
};

#[derive(Clone, Debug)]
pub struct AcpConfig {
    /// Backend-only process setup; credentials never enter serialized ACP configuration or debug output.
    pub child_setup: Option<ChildSetup>,
    pub name: String,
    pub binary: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Model API keys must not be inherited by an unrelated external agent.
    pub hidden_env: Vec<String>,
    pub startup_timeout: Duration,
    pub turn_timeout: Duration,
    pub extras: AcpExtras,
    /// Shared with the service's completion source; updated even during session setup.
    pub commands: AgentCommands,
    /// The model a person picked for the turn, or None for the agent's own.
    pub model: Option<String>,
    /// The thinking effort a person picked for the turn, or None for the agent's own.
    pub effort: Option<String>,
}

#[derive(Clone)]
pub struct ChildSetup(std::sync::Arc<dyn Fn(&mut tokio::process::Command) + Send + Sync>);
impl ChildSetup {
    pub fn new(setup: impl Fn(&mut tokio::process::Command) + Send + Sync + 'static) -> Self {
        Self(std::sync::Arc::new(setup))
    }
    pub(crate) fn apply(&self, command: &mut tokio::process::Command) {
        (self.0)(command);
    }
}
impl std::fmt::Debug for ChildSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChildSetup(<backend-only>)")
    }
}

/// The complete agent-supplied tool call is shown before selecting an `allow_once` option.
/// Persistent grants are never selected by this adapter.
#[async_trait]
pub trait PermissionHandler: Send + Sync {
    async fn approve(&self, tool_call: &Value) -> bool;

    /// Run authored command injections through Vatra's policy, approver and process tree before
    /// placeholder substitution. Standalone clients fail closed for command-bearing skills.
    async fn skill_body(&self, skill: &vatra_extensions::Skill) -> Result<String, String> {
        extras::checked_body(skill)
    }
}
