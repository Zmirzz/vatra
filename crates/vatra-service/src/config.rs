use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use vatra_core::McpServerConfig;
use vatra_engine::WebSearchConfig;

use crate::agents::MAX_STEPS;

const DRIVERS: [&str; 2] = ["openai", "anthropic"];

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("couldn't read `{path}`: {message}. Start from vatra.example.toml.")]
    Read { path: String, message: String },
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug, PartialEq, Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum AgentConfig {
    Model {
        base_url: Option<String>,
        model: String,
        api_key_env: Option<String>,
        driver: Option<String>,
        context_window: Option<u64>,
        /// The key of a server Vatra started itself. Never read from vatra.toml.
        #[serde(skip)]
        api_key: Option<String>,
    },
    Claude {
        binary: PathBuf,
    },
    Acp {
        binary: PathBuf,
        #[serde(default)]
        subscription: bool,
        /// Use Vatra's own Sign in with ChatGPT connection, never native Codex credentials.
        #[serde(default)]
        chatgpt_plan: bool,
        /// Official Codex executable started by the Vatra launch bridge.
        codex_binary: Option<PathBuf>,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default = "default_acp_startup_timeout")]
        startup_timeout_sec: u64,
        #[serde(default = "default_acp_turn_timeout")]
        turn_timeout_sec: u64,
    },
}

impl AgentConfig {
    /// Agent SDK adapters must use an explicitly supplied API key, rather than Claude Code's
    /// subscription login. This guard runs at build/start time; listing configuration stays inert.
    pub fn validate_acp_api_key(&self, key: Option<&std::ffi::OsStr>) -> Result<(), String> {
        if self.is_claude_acp() && key.is_none_or(std::ffi::OsStr::is_empty) {
            return Err("claude-agent-acp requires ANTHROPIC_API_KEY. Use kind = \"claude\" for the Claude Code subscription route.".into());
        }
        Ok(())
    }

    pub fn is_claude_acp(&self) -> bool {
        let Self::Acp { binary, args, .. } = self else {
            return false;
        };
        binary
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("claude-agent-acp")
            || args
                .iter()
                .any(|arg| arg.to_ascii_lowercase().contains("claude-agent-acp"))
    }
}

fn default_acp_startup_timeout() -> u64 {
    30
}

fn default_acp_turn_timeout() -> u64 {
    900
}

/// The `[engine]` section: how the engine agents run.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineConfig {
    /// The most model steps one turn takes before it stops.
    #[serde(default = "default_max_steps")]
    pub max_steps: usize,
}

impl Default for EngineConfig {
    fn default() -> EngineConfig {
        EngineConfig {
            max_steps: MAX_STEPS,
        }
    }
}

fn default_max_steps() -> usize {
    MAX_STEPS
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub agents: BTreeMap<String, AgentConfig>,
    pub summarizer: String,
    #[serde(default)]
    pub mcp: Vec<McpServerConfig>,
    /// The web search tool. It is only offered when the provider is set up and its key is in the environment.
    pub web_search: Option<WebSearchConfig>,
    #[serde(default)]
    pub engine: EngineConfig,
    /// Local models Vatra runs itself. Optional: llama.cpp is found on PATH or where winget installs it.
    #[serde(default)]
    pub local: LocalConfig,
}

/// The `[local]` section.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalConfig {
    /// The `llama-server` executable, when it isn't found on its own.
    pub llama_server: Option<PathBuf>,
    /// Where downloaded models go. Defaults to %LOCALAPPDATA%\Vatra\models.
    pub models_dir: Option<PathBuf>,
}

impl Config {
    /// No agents configured yet. The app opens and explains what to do.
    pub fn empty() -> Config {
        Config {
            agents: BTreeMap::new(),
            summarizer: String::new(),
            mcp: vec![],
            web_search: None,
            engine: EngineConfig::default(),
            local: LocalConfig::default(),
        }
    }

    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|error| ConfigError::Read {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
        Config::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let config: Config =
            toml::from_str(text).map_err(|error| ConfigError::Invalid(explain(&error)))?;
        config.validate()?;
        Ok(config)
    }

    /// Adds the agents set up in the app. vatra.toml wins a name clash; a model agent's key comes from the
    /// keychain. The summarizer chosen in the app applies when vatra.toml names none. What couldn't be added comes
    /// back as warnings, so a broken entry never stops the service.
    pub fn merged_with(
        mut self,
        rows: &[vatra_core::ConfiguredAgentRow],
        summarizer: Option<&str>,
        secrets: &dyn vatra_core::SecretStore,
    ) -> (Config, Vec<String>) {
        let mut warnings = Vec::new();
        for row in rows {
            if self.agents.contains_key(&row.name) {
                warnings.push(format!("`{}` is set in vatra.toml, so the one set up in the app isn't used", row.name));
                continue;
            }
            let mut agent: AgentConfig = match serde_json::from_str(&row.settings_json) {
                Ok(agent) => agent,
                Err(error) => {
                    warnings.push(format!("`{}` couldn't be read: {error}", row.name));
                    continue;
                }
            };
            if let AgentConfig::Model { api_key, .. } = &mut agent {
                match secrets.get(&agent_secret(&row.name)) {
                    Ok(key) => *api_key = key,
                    Err(error) => warnings.push(format!("the key of `{}` couldn't be read: {error}", row.name)),
                }
            }
            self.agents.insert(row.name.clone(), agent);
        }
        if self.summarizer.is_empty()
            && let Some(name) = summarizer
        {
            self.summarizer = name.to_string();
        }
        if let Err(error) = self.validate() {
            warnings.push(error.to_string());
            self.summarizer.clear();
        }
        (self, warnings)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let config = self;
        for (name, agent) in &config.agents {
            if let AgentConfig::Acp {
                chatgpt_plan: true,
                subscription,
                codex_binary,
                ..
            } = agent
                && (!subscription
                    || codex_binary
                        .as_ref()
                        .is_none_or(|path| path.as_os_str().is_empty()))
            {
                return Err(ConfigError::Invalid(format!(
                    "agent `{name}` using chatgpt_plan needs subscription = true and the official codex_binary executable"
                )));
            }
            if let AgentConfig::Acp {
                binary,
                startup_timeout_sec,
                turn_timeout_sec,
                ..
            } = agent
                && (binary.as_os_str().is_empty()
                    || !(1..=86400).contains(startup_timeout_sec)
                    || !(1..=86400).contains(turn_timeout_sec))
            {
                return Err(ConfigError::Invalid(format!(
                    "ACP agent `{name}` needs an executable and startup and turn timeouts between 1 and 86400 seconds"
                )));
            }
            if matches!(agent, AgentConfig::Claude { .. }) && name != "claude" {
                return Err(ConfigError::Invalid(format!(
                    "agent `{name}` has kind claude, so it must be named `claude`"
                )));
            }
            if let AgentConfig::Model {
                driver: Some(driver),
                ..
            } = agent
                && !DRIVERS.contains(&driver.as_str())
            {
                return Err(ConfigError::Invalid(format!(
                    "agent `{name}` has driver `{driver}`; use openai or anthropic"
                )));
            }
            if let AgentConfig::Model {
                base_url: None,
                driver,
                ..
            } = agent
                && driver.as_deref() != Some("anthropic")
            {
                return Err(ConfigError::Invalid(format!(
                    "agent `{name}` needs a base_url; only the anthropic driver has a default"
                )));
            }
        }
        let mut names = BTreeSet::new();
        for server in &config.mcp {
            server.validate().map_err(ConfigError::Invalid)?;
            if !names.insert(server.name.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "two MCP servers are named `{}`",
                    server.name
                )));
            }
        }
        if config.engine.max_steps == 0 {
            return Err(ConfigError::Invalid(
                "engine.max_steps must be at least 1".into(),
            ));
        }
        if let Some(search) = &config.web_search
            && search.provider != "brave"
        {
            return Err(ConfigError::Invalid(format!(
                "web_search provider `{}` isn't supported; use brave",
                search.provider
            )));
        }
        // Empty means no automatic summaries; a `local:` name is a local model, checked when it is used.
        if !config.summarizer.is_empty()
            && !config.summarizer.starts_with(crate::models::AGENT_PREFIX)
            && !matches!(
                config.agents.get(&config.summarizer),
                Some(AgentConfig::Model { .. })
            )
        {
            return Err(ConfigError::Invalid(format!(
                "summarizer = `{}` must be a model agent",
                config.summarizer
            )));
        }
        Ok(())
    }
}

/// Where the key of an agent set up in the app is kept in the keychain.
pub fn agent_secret(name: &str) -> String {
    format!("vatra/agent/{name}")
}

fn explain(error: &toml::de::Error) -> String {
    let message = error.to_string();
    if message.contains("api_key`") {
        format!("API keys don't belong in this file; use api_key_env. {message}")
    } else {
        message
    }
}
