use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// One MCP server from `vatra.toml`. It is either a `command` started on this machine or a `url` it is reached at.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpServerConfig {
    pub name: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub url: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub headers_helper: Option<String>,
    pub oauth: Option<McpOAuth>,
    pub tool_timeout_sec: Option<u64>,
    pub startup_timeout_sec: Option<u64>,
    pub enabled_tools: Option<Vec<String>>,
    pub disabled_tools: Vec<String>,
    pub always_load: bool,
    /// The plugin that brings this server. Its tools are named `mcp__plugin_<plugin>_<name>__<tool>`.
    pub plugin: Option<String>,
    /// Named values that `${NAME}` in this server's settings reads. They never appear in logs or replies.
    pub secrets: BTreeMap<String, SecretRef>,
    /// Where the plugin that brings this server lives. The loader sets it; it is never read from a file.
    #[serde(skip)]
    pub plugin_context: Option<PluginContext>,
}

/// The place of a plugin on disk and its settings, which the `${CLAUDE_PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}`
/// and `${user_config.KEY}` references of its servers read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginContext {
    pub root: PathBuf,
    pub data: PathBuf,
    pub user_config: BTreeMap<String, String>,
}

impl McpServerConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("an MCP server needs a name".into());
        }
        match (&self.command, &self.url) {
            (Some(command), _) if command.trim().is_empty() => {
                Err(format!("MCP server `{}` has an empty command", self.name))
            }
            (Some(_), Some(_)) => Err(format!("MCP server `{}` has both a command and a url; keep one", self.name)),
            (None, None) => Err(format!("MCP server `{}` needs a command or a url", self.name)),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpOAuth {
    pub client_id: Option<String>,
    pub callback_port: Option<u16>,
    pub auth_server_metadata_url: Option<String>,
    pub scopes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretRef {
    /// The value of an environment variable of the service.
    Env(String),
    /// An entry of the operating system keychain. It is read from Task 12 on.
    Keychain(String),
}

pub fn claude_mcp_config(servers: &[McpServerConfig]) -> Value {
    let servers: Map<String, Value> = servers
        .iter()
        .map(|server| {
            let entry = match (&server.url, &server.command) {
                (Some(url), _) => json!({ "type": "http", "url": url, "headers": server.headers }),
                (None, command) => {
                    let command = command.as_deref().unwrap_or_default();
                    json!({ "command": command, "args": server.args, "env": server.env })
                }
            };
            (server.name.clone(), entry)
        })
        .collect();
    json!({ "mcpServers": servers })
}
