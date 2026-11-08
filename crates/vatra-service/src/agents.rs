use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use vatra_acp::{AcpAgent, AcpConfig, PermissionHandler};
use vatra_claude::{ClaudeAgent, ClaudeApprovals, ClaudeConfig};
use vatra_core::{
    Agent, AgentError, BriefFields, KeyringBackend, KeyringStore, McpServerConfig,
    MemorySecretStore, ProcessGroup, Rules, SecretStore, Summarizer,
};
use vatra_engine::model::{
    AnthropicDriver, DEFAULT_BASE_URL, ModelConfig, ModelDriver, OpenAiDriver, with_retries,
};
use vatra_engine::{
    Approver, Background, EngineAgent, EngineExtensions, EngineSetup, ExpandVars, HubPool, McpHub, McpTool,
    OpenAiSummarizer, PermissionPolicy, Questioner, SkillSession, SubagentRunner, SubagentSetup,
    ToolContext, ToolRegistry, agent_tool, apply_patch_tool, ask_tool, bash_tool, edit_tool,
    glob_tool, grep_tool, load_instructions, powershell_tool, read_tool, skill_tool,
    task_stop_tool, todo_tool, tool_search_tool, web_fetch_tool, web_search_tool, write_tool,
};
use vatra_extensions::{ActiveExtensions, Catalog, Scope, UsageCounts};

use crate::config::{AgentConfig, Config};
use crate::models::check::{BuildAgent, CheckTarget};
use vatra_models::label::StepOutcome;
use crate::models::manager::ConfiguredModel;
use crate::models::server::TurnGuard;
use crate::models::{AGENT_PREFIX, ModelManager};
use crate::service::PermissionMode;

/// The most model steps a turn takes unless `[engine]` sets another number.
pub(crate) const MAX_STEPS: usize = 200;
type ProxyHubs = Arc<Mutex<HashMap<String, (i64, Arc<McpHub>)>>>;

/// What a person picked for one turn in the model picker. Anything left empty keeps the agent's own choice.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TurnOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl TurnOptions {
    /// Checks what came from the app: a model and an effort are plain names.
    pub fn checked(model: Option<String>, effort: Option<String>) -> Result<TurnOptions, String> {
        let model = model.map(|text| text.trim().to_string()).filter(|text| !text.is_empty());
        if let Some(model) = &model
            && (model.len() > 100 || !model.chars().all(|c| c.is_ascii_alphanumeric() || "._-:/[]".contains(c)))
        {
            return Err("That model name isn't valid".into());
        }
        let effort = effort.filter(|text| !text.is_empty());
        if let Some(effort) = &effort
            && (effort.len() > 40 || !effort.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)))
        {
            return Err("That effort level isn't valid".into());
        }
        Ok(TurnOptions { model, effort })
    }
}

pub struct BuildRequest {
    pub name: String,
    pub session_id: i64,
    pub project_id: i64,
    pub approver: Arc<dyn Approver>,
    pub questioner: Arc<dyn Questioner>,
    pub mode: PermissionMode,
    pub cwd: Option<PathBuf>,
    pub options: TurnOptions,
}

#[async_trait]
pub trait AgentFactory: Send + Sync {
    fn names(&self) -> Vec<String>;
    fn file_agent_names(&self) -> Option<Vec<String>> { None }
    async fn build(&self, request: BuildRequest) -> Result<Arc<dyn Agent>, String>;
    /// A fresh fixture, without the user's catalog, MCP servers or memory.
    async fn build_parity(&self, request: BuildRequest, _task: &vatra_parity::Task) -> Result<Arc<dyn Agent>, String> {
        self.build(request).await
    }
    fn summarizer(&self) -> Arc<dyn Summarizer>;
    /// One answer from a model with no tools at all: what reads text that others wrote (an email, a web page)
    /// uses it, so that text can't make anything happen. `agent` names a model agent; None uses the summarizer's.
    async fn complete_text(&self, _agent: Option<&str>, _system: &str, _user: &str) -> Result<String, String> {
        Err("A model that answers without tools isn't available in this service.".into())
    }
    /// The MCP servers of the configuration, for their status and login.
    fn mcp_servers(&self) -> Vec<McpServerConfig> {
        Vec::new()
    }
    /// A bounded connection probe in an isolated temporary workspace.
    async fn probe_mcp(&self, _server:McpServerConfig)->Result<serde_json::Value,String>{
        Err("MCP connection testing is unavailable for this service configuration.".into())
    }
    /// Where keychain references are read from and OAuth tokens are saved to.
    fn secrets(&self) -> Arc<dyn SecretStore> {
        Arc::new(MemorySecretStore::default())
    }
    fn search_chain(&self, store: &vatra_core::Store) -> Result<vatra_engine::SearchChain, String> {
        crate::research::configured_chain(store, self.secrets().as_ref(), None)
    }
    /// The catalog of extensions, when the service keeps one.
    fn catalog(&self) -> Option<Arc<Catalog>> {
        None
    }
    /// The MCP servers of a project, for the prompts and resources they offer. `folder` is the project's folder when
    /// it has one. The hub works in the folder a turn would, so the same hub serves both. None without a configuration.
    fn mcp_hub(&self, _project_id: i64, _folder: Option<&Path>) -> Option<Arc<McpHub>> {
        None
    }
    /// The catalog changed for one project, or for every project when `project` is None. The next turn builds its
    /// hubs again; a turn that is running keeps what it started with.
    fn catalog_changed(&self, _project: Option<i64>) {}
    fn extension_route(&self, _name: &str) -> Option<&'static str> {
        None
    }
    fn picker_identity(&self, _name: &str) -> Option<(String, Option<String>)> { None }
    fn input_capabilities(&self,_name:&str,_model:Option<&str>)->crate::files::InputCapabilities {Default::default()}
    fn working_directory(&self,_session:i64,_project:Option<i64>,_path:Option<&Path>)->Option<PathBuf>{None}
    fn file_directory(&self,_session:i64)->Option<PathBuf>{None}
    fn subscription(&self, name: &str) -> bool {
        name == "claude"
    }
    /// What a person can choose for an agent that reports its own models and efforts, such as an ACP agent.
    async fn agent_options(&self, _name: &str, _project_id: i64, _model: Option<&str>) -> Result<serde_json::Value, String> {
        Err("This agent doesn't list models".into())
    }
    fn chatgpt(&self) -> Option<Arc<crate::chatgpt::ChatGptAuth>> {
        None
    }
    fn commands(&self, _project: i64) -> Vec<serde_json::Value> {
        Vec::new()
    }
    /// The local models, when the service runs them.
    fn models(&self) -> Option<Arc<ModelManager>> {
        None
    }
    /// The engine agents of the configuration, for their labels.
    fn configured_models(&self) -> Vec<ConfiguredModel> {
        Vec::new()
    }
    /// What the short check needs to run on an engine agent.
    async fn check_target(&self, name: &str) -> Result<CheckTarget, String> {
        Err(format!("`{name}` can't be checked here"))
    }
    /// Whether checking this agent sends requests a provider may charge for.
    fn check_costs(&self, _name: &str) -> bool {
        true
    }
    fn workspace_supported(&self, _name: &str) -> bool { true }
    fn proxy_hub(&self, _key: &str, _project: i64) -> Option<Arc<McpHub>> {
        None
    }
}

/// How Claude Code reaches the running service for approvals. The CLI has none.
#[derive(Clone, Debug)]
pub struct BridgeSettings {
    pub binary: PathBuf,
    pub url: String,
    pub token_file: PathBuf,
    pub data_dir: PathBuf,
}

/// Where an agent works when its project has no folder. Otherwise it would inherit the
/// service's own directory and could change files there.
pub fn project_workspace(data_dir: &Path, project_id: i64) -> std::io::Result<PathBuf> {
    let folder = if project_id > 0 {
        data_dir.join("workspaces").join(format!("project-{project_id}"))
    } else {
        data_dir.join("workspaces").join("chats")
    };
    fs::create_dir_all(&folder)?;
    Ok(folder)
}

/// A projectless conversation never shares its filesystem with another conversation.
pub fn session_workspace(data_dir: &Path, session_id: i64) -> std::io::Result<PathBuf> {
    if session_id <= 0 { return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput,"A real conversation is required.")); }
    let folder=data_dir.join("workspaces").join("conversations").join(session_id.to_string());
    fs::create_dir_all(&folder)?; Ok(folder)
}

/// The variables that hold the keys of the configured agents, which commands and MCP servers must not see.
fn hidden_names(config: &Config) -> Vec<String> {
    let mut names: BTreeSet<String> = config
        .agents
        .values()
        .filter_map(|agent| match agent {
            AgentConfig::Model { api_key_env, .. } => api_key_env.clone(),
            AgentConfig::Claude { .. } | AgentConfig::Acp { .. } => None,
        })
        .collect();
    names.extend(
        config
            .web_search
            .iter()
            .map(|search| search.api_key_env.clone()),
    );
    names.into_iter().collect()
}

impl BridgeSettings {
    fn server(&self, session_id: i64) -> McpServerConfig {
        McpServerConfig {
            name: "vatra".into(),
            command: Some(self.binary.display().to_string()),
            args: vec![
                "approval-bridge".into(),
                "--url".into(),
                self.url.clone(),
                "--token-file".into(),
                self.token_file.display().to_string(),
                "--session".into(),
                session_id.to_string(),
            ],
            ..Default::default()
        }
    }
}

/// What a session keeps between its turns. Agents are rebuilt for every turn, so the parts that must last
/// are held here.
#[derive(Default)]
struct SessionState {
    commands: vatra_acp::AgentCommands,
    project_id: i64,
    background: Arc<Background>,
    read_files: Arc<Mutex<HashSet<PathBuf>>>,
    skills: Arc<SkillSession>,
}

pub struct ConfigAgents {
    file_agents: Vec<String>,
    config: Config,
    hubs: HubPool,
    bridge: Option<BridgeSettings>,
    sessions: Mutex<HashMap<i64, Arc<SessionState>>>,
    secrets: Arc<dyn SecretStore>,
    catalog: Option<Arc<Catalog>>,
    catalog_synced: bool,
    /// What adding the configured servers to the catalog left out. It is shown at each turn.
    catalog_problems: Vec<String>,
    store: Option<Arc<vatra_core::Store>>,
    chatgpt: Option<Arc<crate::chatgpt::ChatGptAuth>>,
    proxy_hubs: ProxyHubs,
    next_snapshot: AtomicU64,
    models: Option<Arc<ModelManager>>,
}

impl ConfigAgents {
    /// Select the credential backend before exposing the factory to the service.
    pub fn with_secret_store(mut self, secrets:Arc<dyn SecretStore>)->Self{
        self.hubs=HubPool::new(hidden_names(&self.config)).with_secrets(Arc::clone(&secrets));
        self.secrets=secrets;self
    }
    pub fn new(config: Config) -> ConfigAgents {
        Self::build(config, None)
    }

    pub fn with_bridge(config: Config, bridge: BridgeSettings) -> ConfigAgents {
        Self::build(config, Some(bridge))
    }

    fn build(config: Config, bridge: Option<BridgeSettings>) -> ConfigAgents {
        let secrets: Arc<dyn SecretStore> = Arc::new(KeyringStore::new(KeyringBackend));
        let hubs = HubPool::new(hidden_names(&config)).with_secrets(Arc::clone(&secrets));
        ConfigAgents {
            file_agents: config.agents.keys().cloned().collect(),
            config,
            store: None,
            chatgpt: None,
            proxy_hubs: Arc::default(),
            next_snapshot: AtomicU64::new(1),
            models: None,
            hubs,
            bridge,
            sessions: Mutex::default(),
            secrets,
            catalog: None,
            catalog_synced: false,
            catalog_problems: Vec::new(),
        }
    }

    /// Keep the file's origin after merging app-saved agents into the runtime configuration.
    pub fn with_file_agents(mut self, names: Vec<String>) -> Self { self.file_agents=names; self }

    /// Attaches the catalog. The servers of this configuration are used for this process only; they are not written to
    /// the catalog, so a command run with another configuration leaves the app's servers alone.
    pub fn with_catalog(mut self, catalog: Arc<Catalog>) -> ConfigAgents {
        self.catalog = Some(catalog);
        self.catalog_synced = false;
        self
    }

    pub fn with_models(mut self, models: Arc<ModelManager>) -> Self {
        self.models = Some(models);
        self
    }

    /// A local model of the manager, as a model agent pointing at its running server. The configuration wins a
    /// name clash, so a local model never hides a configured agent.
    async fn local_agent(&self, name: &str) -> Result<Option<(AgentConfig, TurnGuard, Option<u32>)>, String> {
        let Some(models) = &self.models else { return Ok(None) };
        if self.config.agents.contains_key(name) || !name.starts_with(AGENT_PREFIX) {
            return Ok(None);
        }
        let (model, server, guard) = models.resolve(name).await?;
        let ctx = server.ctx.or(Some(model.settings.ctx));
        let config = AgentConfig::Model {
            base_url: Some(server.base_url),
            model: model.slug,
            api_key_env: None,
            driver: None,
            context_window: ctx.map(u64::from),
            api_key: Some(server.api_key.clone()),
        };
        Ok(Some((config, guard, ctx)))
    }

    pub fn with_store(mut self, store: Arc<vatra_core::Store>) -> Self {
        self.store = Some(store);
        self
    }
    pub fn with_chatgpt(mut self, auth: Arc<crate::chatgpt::ChatGptAuth>) -> Self {
        self.chatgpt = Some(auth);
        self
    }

    /// Attaches the catalog as the service does: the configuration's servers are catalog entries too, so the catalog
    /// lists them with the others, and a server the configuration no longer names leaves the catalog.
    pub fn with_catalog_synced(mut self, catalog: Arc<Catalog>) -> ConfigAgents {
        if let Err(error) = catalog.sync_servers(&self.config.mcp, Scope::Global) {
            self.catalog_problems.push(format!(
                "the configured MCP servers couldn't be added to the catalog: {error}"
            ));
        }
        self.catalog = Some(catalog);
        self.catalog_synced = true;
        self
    }

    /// What the catalog holds for a turn of a project: its active extensions and the run counts, and the problems
    /// that kept some out. Without a catalog there is nothing.
    fn catalog_view(&self, project_id: i64) -> (ActiveExtensions, UsageCounts, Vec<String>) {
        let mut problems = self.catalog_problems.clone();
        let Some(catalog) = &self.catalog else {
            return (
                ActiveExtensions::default(),
                UsageCounts::default(),
                problems,
            );
        };
        let mut active = match catalog.active_for(project_id) {
            Ok(active) => active,
            Err(error) => {
                problems.push(format!("the extensions couldn't be read: {error}"));
                ActiveExtensions::default()
            }
        };
        problems.append(&mut active.problems);
        let usage = match catalog.usage() {
            Ok(usage) => usage,
            Err(error) => {
                problems.push(format!("the run counts couldn't be read: {error}"));
                UsageCounts::default()
            }
        };
        (active, usage, problems)
    }

    /// The MCP servers of a turn: the configured ones, then the catalog's servers that have another name.
    fn mcp_servers(&self, active: &ActiveExtensions) -> Vec<McpServerConfig> {
        let entries = self
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.list().ok())
            .unwrap_or_default();
        let mut servers = active.mcp_servers.clone();
        for server in &self.config.mcp {
            let managed = self.catalog_synced
                && entries.iter().any(|entry| {
                    entry.origin == vatra_extensions::Origin::Manual
                        && entry.kind == vatra_extensions::ElementKind::McpServer
                        && entry.name == server.name
                });
            if !managed
                && !servers
                    .iter()
                    .any(|other| other.name == server.name && other.plugin == server.plugin)
            {
                servers.push(server.clone());
            }
        }
        servers
    }

    fn bridge_server(&self, bridge: &BridgeSettings, session_id: i64) -> McpServerConfig {
        let mut server = bridge.server(session_id);
        if let Some(store) = &self.store
            && let Ok(Some(scope)) = store.workspace_turn_context(session_id) {
                server.args.extend(["--workspace-scope".into(), scope.scope_id]);
            }
        server
    }
    fn claude_approvals(&self, session_id: i64, mode: PermissionMode) -> ClaudeApprovals {
        match (&self.bridge, mode) {
            (None, PermissionMode::FullAccess) => ClaudeApprovals::Catalog,
            (None, _) => ClaudeApprovals::Deny,
            (Some(_), PermissionMode::FullAccess) => ClaudeApprovals::FullAccess,
            (Some(bridge), mode) => ClaudeApprovals::Prompt {
                server: Box::new(self.bridge_server(bridge, session_id)),
                tool: "mcp__vatra__approve".into(),
                accept_edits: mode == PermissionMode::AcceptEdits,
            },
        }
    }

    fn find(&self, name: &str) -> Result<&AgentConfig, String> {
        self.config.agents.get(name).ok_or_else(|| {
            let known: Vec<_> = self.config.agents.keys().map(String::as_str).collect();
            format!("unknown agent `{name}`; configured: {}", known.join(", "))
        })
    }

    fn model_driver(&self, name: &str) -> Result<Arc<dyn ModelDriver>, String> {
        self.driver_for(name, self.find(name)?, None)
    }

    fn driver_for(&self, name: &str, agent: &AgentConfig, model_override: Option<&str>) -> Result<Arc<dyn ModelDriver>, String> {
        let AgentConfig::Model {
            base_url,
            model,
            api_key_env,
            driver,
            context_window,
            api_key: own_key,
        } = agent
        else {
            return Err(format!("`{name}` isn't a model agent"));
        };
        let kind = driver.as_deref().unwrap_or("openai");
        if !matches!(kind, "openai" | "anthropic" | "copilot") {
            return Err(format!(
                "the `{kind}` driver isn't supported; use openai, anthropic or copilot"
            ));
        }
        let base_url = match base_url {
            Some(base_url) => base_url.clone(),
            None if kind == "anthropic" => DEFAULT_BASE_URL.into(),
            None => return Err(format!("agent `{name}` needs a base_url")),
        };
        let api_key = match (own_key, api_key_env) {
            (Some(key), _) => Some(key.clone()),
            (None, Some(variable)) => Some(
                std::env::var(variable)
                    .map_err(|_| format!("environment variable `{variable}` isn't set"))?,
            ),
            (None, None) => None,
        };
        let config = ModelConfig {
            base_url,
            model: model_override.map(String::from).unwrap_or_else(|| model.clone()),
            api_key,
            context_window: if model_override.is_some_and(|picked| picked != model) { None } else { *context_window },
        };
        let driver: Arc<dyn ModelDriver> = match kind {
            "anthropic" => Arc::new(AnthropicDriver::new(config)),
            _ => Arc::new(OpenAiDriver::new(config)),
        };
        Ok(with_retries(driver))
    }

    fn session(&self, session_id: i64, project_id: i64) -> Arc<SessionState> {
        let mut sessions = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        let state = sessions.entry(session_id).or_insert_with(|| {
            Arc::new(SessionState {
                project_id,
                skills: Arc::new(SkillSession::new(session_id.to_string())),
                ..Default::default()
            })
        });
        Arc::clone(state)
    }

    /// The folder that holds the session folders and the memory of the agents. The CLI has no service
    /// folder, so it uses one in the temp directory.
    fn data_dir(&self) -> PathBuf {
        match &self.bridge {
            Some(bridge) => bridge.data_dir.clone(),
            None => std::env::temp_dir().join("vatra"),
        }
    }

    /// Every tool an engine agent may use. The MCP tools come from the shared hub.
    fn registry(&self, driver: &Arc<dyn ModelDriver>, hub: &Arc<McpHub>) -> ToolRegistry {
        let mut registry = base_registry(driver, hub, None);
        if let Some(tool) = self.chat_search_tool() { registry.register(tool); }
        registry
    }

    fn chat_search_tool(&self) -> Option<Arc<dyn vatra_engine::Tool>> {
        match &self.store {
            Some(store) => self.search_chain(store)
                .ok().map(|chain| vatra_engine::chain_search_tool(Arc::new(chain))),
            None => self.config.web_search.clone().and_then(web_search_tool),
        }
    }

    /// The engine agent a check runs on: the tools of a normal turn plus the check's own, in an empty folder,
    /// where only the check's tools are approved.
    fn check_builder(&self, name: &str, driver: Arc<dyn ModelDriver>) -> Result<(BuildAgent, PathBuf), String> {
        let dir = self
            .data_dir()
            .join("checks")
            .join(format!("{}-{}", crate::models::now(), rand::random::<u32>()));
        fs::create_dir_all(dir.join("session")).map_err(|error| error.to_string())?;
        let web_search = self.config.web_search.clone();
        let hidden = self.hidden_env();
        let name = name.to_string();
        let workdir = dir.clone();
        let build: BuildAgent = Arc::new(move |hub: Arc<McpHub>, extra: Vec<Arc<dyn vatra_engine::Tool>>| {
            let mut registry = base_registry(&driver, &hub, web_search.clone());
            for tool in extra {
                registry.register(tool);
            }
            let tree = ProcessGroup::new().map_err(|error| error.to_string())?;
            let (events, _) = tokio::sync::mpsc::unbounded_channel();
            let context = ToolContext {
                cwd: dir.clone(),
                project_root: dir.clone(),
                session_dir: dir.join("session"),
                read_files: Arc::default(),
                events,
                questions: Arc::new(vatra_engine::NoQuestions),
                tree: Arc::new(tree),
                background: Arc::new(Background::new()),
                hidden_env: hidden.clone(),
                memory_dir: None,
            };
            Ok(Arc::new(EngineAgent::new(EngineSetup {
                name: name.clone(),
                driver: Arc::clone(&driver),
                hub,
                registry,
                policy: Arc::new(PermissionPolicy::new(PermissionMode::Supervised, Rules::default())),
                approver: Arc::new(CheckApprover),
                context,
                instructions: Default::default(),
                memory: None,
                max_steps: CHECK_STEPS,
                extensions: None,
                audit: None,
                session_id: None,
            })) as Arc<dyn Agent>)
        });
        Ok((build, workdir))
    }
}

/// A check turn needs a call and an answer; a few steps more allow one retry.
const CHECK_STEPS: usize = 6;

/// Approves only the check's own tools.
struct CheckApprover;

#[async_trait]
impl Approver for CheckApprover {
    async fn approve(&self, name: &str, _arguments: &serde_json::Value) -> bool {
        name.ends_with("probe_token")
    }
}

fn base_registry(
    driver: &Arc<dyn ModelDriver>,
    hub: &Arc<McpHub>,
    web_search: Option<vatra_engine::WebSearchConfig>,
) -> ToolRegistry {
    {
        let mut registry = ToolRegistry::default();
        registry.register(read_tool());
        registry.register(write_tool());
        registry.register(edit_tool());
        registry.register(glob_tool());
        registry.register(grep_tool());
        registry.register(apply_patch_tool());
        registry.register(bash_tool());
        if let Some(tool) = powershell_tool() {
            registry.register(tool);
        }
        registry.register(task_stop_tool());
        registry.register(web_fetch_tool(Arc::clone(driver)));
        if let Some(tool) = web_search.and_then(web_search_tool) {
            registry.register(tool);
        }
        registry.register(todo_tool());
        registry.register(ask_tool());
        registry.register(tool_search_tool(Arc::clone(hub)));
        for tool in McpTool::all(hub) {
            registry.register(tool);
        }
        registry.alias("shell", "Bash");
        registry.alias("exec_command", "Bash");
        registry.alias("update_plan", "TodoWrite");
        registry
    }
}

impl ConfigAgents {

    /// The variables that commands, MCP servers and external agents must not inherit.
    pub fn hidden_env(&self) -> Vec<String> {
        hidden_names(&self.config)
    }

    /// The configuration of an ACP agent as a turn starts it: with the account it runs on when that is Vatra's own.
    async fn acp_ready_config(&self, request: &BuildRequest) -> Result<AcpConfig, String> {
        let mut config = self.acp_config(request)?;
        if let AgentConfig::Acp {
            chatgpt_plan: true,
            codex_binary: Some(binary),
            ..
        } = self.find(&request.name)?
        {
            let auth = self.chatgpt.as_ref().ok_or_else(|| {
                "Start the Vatra service and connect ChatGPT in Settings.".to_string()
            })?;
            let token = auth
                .access_token_for_child()
                .await
                .map_err(|error| error.to_string())?;
            let launcher = self
                .bridge
                .as_ref()
                .map(|bridge| bridge.binary.clone())
                .ok_or_else(|| {
                    "ChatGPT agents require the running Vatra service.".to_string()
                })?;
            let binary = binary.clone();
            config.child_setup = Some(vatra_acp::ChildSetup::new(move |command| {
                token.apply_to(command);
                command
                    .env("CODEX_PATH", &launcher)
                    .env("VATRA_CODEX_BINARY", &binary)
                    .env("MODEL_PROVIDER", "openai_chatgpt_plan");
                command.env_remove("DEFAULT_AUTH_REQUEST");
            }));
        }
        Ok(config)
    }

    /// The configuration of an ACP agent, with the working folder that the factory gives it.
    pub fn acp_config(&self, request: &BuildRequest) -> Result<AcpConfig, String> {
        let AgentConfig::Acp {
            binary,
            args,
            startup_timeout_sec,
            turn_timeout_sec,
            ..
        } = self.find(&request.name)?
        else {
            return Err(format!("`{}` isn't an ACP agent", request.name));
        };
        self.find(&request.name)?
            .validate_acp_api_key(std::env::var_os("ANTHROPIC_API_KEY").as_deref())?;
        // dunce keeps the path in the usual Windows form, which Node-based agents accept.
        let cwd =
            dunce::canonicalize(self.project_dir(request)?).map_err(|error| error.to_string())?;
        let (active, _, problems) = self.catalog_view(request.project_id);
        let entries = self.entries_for(request.project_id);
        let mut skills = active.skills.clone();
        let mut servers = self.mcp_servers(&active);
        // The Codex native route loads its own source. Other ACP agents receive the complete foreign set.
        let codex = binary.to_string_lossy().to_lowercase().contains("codex")
            || args
                .iter()
                .any(|arg| arg.to_lowercase().contains("codex-acp"));
        if codex {
            skills.retain(|skill| {
                !entries.iter().any(|entry| {
                    entry.origin == vatra_extensions::Origin::Codex
                        && entry.name == skill.name
                        && entry.plugin == skill.plugin
                        && skill.dir.starts_with(&entry.content_dir)
                })
            });
            servers.retain(|server| {
                !entries.iter().any(|entry| {
                    entry.origin == vatra_extensions::Origin::Codex
                        && entry.kind == vatra_extensions::ElementKind::McpServer
                        && entry.name == server.name
                        && entry.plugin == server.plugin
                })
            });
        }
        let preamble = self.imported_context(
            &active,
            &entries,
            if codex {
                Some(vatra_extensions::Origin::Codex)
            } else {
                None
            },
        )?;
        let mut servers = self.external_servers(servers, request, &active)?;
        if let Some(bridge) = &self.bridge {
            servers.push(self.bridge_server(bridge, request.session_id));
        }
        let mut hidden_env = self.hidden_env();
        if self.find(&request.name)?.is_claude_acp() {
            hidden_env.retain(|key| key != "ANTHROPIC_API_KEY");
        }
        let mut preamble = preamble;
        if self.bridge.is_some() {
            preamble.push_str("\n\n");
            preamble.push_str(vatra_engine::WEB_VERIFICATION);
        }
        if !problems.is_empty() {
            preamble.push_str(&format!("\nExtension notices:\n{}", problems.join("\n")));
        }
        Ok(AcpConfig {
            child_setup: None,
            name: request.name.clone(),
            binary: binary.clone(),
            args: args.clone(),
            cwd,
            hidden_env,
            startup_timeout: Duration::from_secs(*startup_timeout_sec),
            turn_timeout: Duration::from_secs(*turn_timeout_sec),
            extras: vatra_acp::AcpExtras {
                servers,
                skills,
                preamble: (!preamble.is_empty()).then_some(preamble),
            },
            commands: self
                .session(request.session_id, request.project_id)
                .commands
                .clone(),
            model: request.options.model.clone(),
            effort: request.options.effort.clone(),
        })
    }

    fn entries_for(&self, project: i64) -> Vec<vatra_extensions::CatalogEntry> {
        self.catalog
            .as_ref()
            .and_then(|catalog| catalog.list().ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| entry.scope == Scope::Global || entry.scope == Scope::Project(project))
            .collect()
    }

    fn imported_context(
        &self,
        active: &ActiveExtensions,
        entries: &[vatra_extensions::CatalogEntry],
        native: Option<vatra_extensions::Origin>,
    ) -> Result<String, String> {
        let mut sections = Vec::new();
        for path in active.instructions.iter().chain(active.memory.iter()) {
            let entry = context_entry(entries, path);
            if native.is_some() && entry.is_some_and(|entry| Some(entry.origin) == native) {
                continue;
            }
            let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
            // Bound imported context; full immutable copies remain available by path.
            let text: String = text
                .lines()
                .take(200)
                .collect::<Vec<_>>()
                .join("\n")
                .chars()
                .take(25 * 1024)
                .collect();
            sections.push(format!("Imported context from {}:\n{text}", path.display()));
        }
        Ok(sections.join("\n\n"))
    }

    fn external_servers(
        &self,
        servers: Vec<McpServerConfig>,
        request: &BuildRequest,
        active: &ActiveExtensions,
    ) -> Result<Vec<McpServerConfig>, String> {
        if servers.is_empty() {
            return Ok(servers);
        }
        let cwd = self.project_dir(request)?;
        let needs_proxy = |server: &McpServerConfig| {
            server.oauth.is_some()
                || !server.secrets.is_empty()
                || server.headers_helper.is_some()
                || server.cwd.is_some()
                || server.enabled_tools.is_some()
                || !server.disabled_tools.is_empty()
        };
        let mut proxy = None;
        if servers.iter().any(&needs_proxy)
            && let Some(bridge) = &self.bridge
        {
            let key = format!(
                "{}-{}",
                request.session_id,
                self.next_snapshot.fetch_add(1, Ordering::Relaxed)
            );
            let hub = self
                .hubs
                .get(request.project_id, &cwd, &self.mcp_servers(active));
            self.proxy_hubs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(key.clone(), (request.project_id, hub));
            proxy = Some(vatra_claude::ProxySettings {
                binary: bridge.binary.clone(),
                url: bridge.url.clone(),
                token_file: bridge.token_file.clone(),
                project_id: request.project_id,
                snapshot: Some(key),
            });
        }
        servers
            .into_iter()
            .map(|mut server| {
                if needs_proxy(&server) {
                    return proxy
                        .as_ref()
                        .map(|proxy| proxy.server(&server))
                        .ok_or_else(|| {
                            "This MCP server requires the running Vatra service proxy.".into()
                        });
                }
                let mut vars = vatra_engine::ExpandVars::from_env(&cwd, self.hidden_env());
                if let Some(plugin) = &server.plugin_context {
                    vars.plugin_root = Some(plugin.root.clone());
                    vars.plugin_data = Some(plugin.data.clone());
                    vars.user_config = plugin.user_config.clone();
                }
                let resolved = |text: &str| -> Result<String, String> {
                    let result = vatra_engine::expand(text, &vars);
                    if result.missing.is_empty() && result.reserved.is_empty() {
                        Ok(result.text)
                    } else {
                        Err("An MCP setting needs a value or the Vatra service proxy.".into())
                    }
                };
                server.command = server.command.map(|value| resolved(&value)).transpose()?;
                server.url = server.url.map(|value| resolved(&value)).transpose()?;
                server.args = server
                    .args
                    .iter()
                    .map(|value| resolved(value))
                    .collect::<Result<_, _>>()?;
                for value in server.env.values_mut().chain(server.headers.values_mut()) {
                    *value = resolved(value)?;
                }
                Ok(server)
            })
            .collect()
    }

    fn foreign_context(&self, request: &BuildRequest) -> Result<ToolContext, String> {
        let cwd = self.project_dir(request)?;
        let session_dir = self
            .data_dir()
            .join("sessions")
            .join(request.session_id.to_string());
        fs::create_dir_all(&session_dir).map_err(|e| e.to_string())?;
        let session = self.session(request.session_id, request.project_id);
        let (events, _) = tokio::sync::mpsc::unbounded_channel();
        Ok(ToolContext {
            cwd: cwd.clone(),
            project_root: cwd,
            session_dir,
            read_files: Arc::clone(&session.read_files),
            events,
            questions: Arc::clone(&request.questioner),
            tree: Arc::new(ProcessGroup::new().map_err(|e| e.to_string())?),
            background: Arc::clone(&session.background),
            hidden_env: self.hidden_env(),
            memory_dir: None,
        })
    }

    /// The folder the agents of a project work in. Its MCP servers see it as `${CLAUDE_PROJECT_DIR}`.
    fn project_dir(&self, request: &BuildRequest) -> Result<PathBuf, String> {
        if request.project_id == 0 {
            return session_workspace(&self.data_dir(),request.session_id).map_err(|e|e.to_string());
        }
        match (&request.cwd, &self.bridge) {
            (Some(cwd), _) => Ok(cwd.clone()),
            (None, Some(bridge)) => project_workspace(&bridge.data_dir, request.project_id)
                .map_err(|error| error.to_string()),
            (None, None) => std::env::current_dir().map_err(|error| error.to_string()),
        }
    }

    fn engine(
        &self,
        request: BuildRequest,
        driver: Arc<dyn ModelDriver>,
        hub: Arc<McpHub>,
        mut extensions: EngineExtensions,
        rules: Rules,
        extra: Vec<PathBuf>,
    ) -> Result<EngineAgent, String> {
        let data_dir = self.data_dir();
        let effort = request.options.effort.clone();
        let session = self.session(request.session_id, request.project_id);
        extensions.session = Arc::clone(&session.skills);
        let cwd = match (request.cwd, &self.bridge) {
            (Some(cwd), _) => cwd,
            (None, _) if request.project_id == 0 => session_workspace(&data_dir, request.session_id)
                .map_err(|error| error.to_string())?,
            (None, Some(_)) => project_workspace(&data_dir, request.project_id)
                .map_err(|error| error.to_string())?,
            (None, None) => std::env::current_dir().map_err(|error| error.to_string())?,
        };
        let session_dir = data_dir
            .join("sessions")
            .join(request.session_id.to_string());
        fs::create_dir_all(&session_dir).map_err(|error| error.to_string())?;
        let memory = if request.project_id == 0 {
            session_dir.join("memory")
        } else { data_dir.join("projects").join(request.project_id.to_string()).join("memory") };
        fs::create_dir_all(&memory).map_err(|error| error.to_string())?;
        let tree =
            ProcessGroup::new().map_err(|error| format!("couldn't set up processes: {error}"))?;
        // The event sink and the process tree are set again by each turn, so these are only placeholders.
        let (events, _) = tokio::sync::mpsc::unbounded_channel();
        let mut registry = self.registry(&driver, &hub);
        if let Some(store) = &self.store {
            registry.register(vatra_engine::search_history_tool(Arc::clone(store)));
            // Search by meaning, when the person has an embedding account. In a workspace turn it stays in that project.
            if let Some(embedder) = crate::semantic::embedder(store, self.secrets().as_ref()) {
                let project = store.workspace_turn_context(request.session_id).ok().flatten().map(|scope| scope.project_id);
                let semantic_store = Arc::clone(store);
                registry.register(vatra_engine::semantic_search_tool(Arc::new(move |question: String, limit: usize| {
                    let (store, embedder) = (Arc::clone(&semantic_store), embedder.clone());
                    Box::pin(async move {
                        let hits = crate::semantic::search(&store, &embedder, &question, limit, &[], project).await?;
                        serde_json::to_string(&hits).map_err(|e| e.to_string())
                    })
                })));
            }
            if let Some(scope) = store.workspace_turn_context(request.session_id).map_err(|e|e.to_string())? {
                registry.register(vatra_engine::scoped_search_workspace_tool(Arc::clone(store),scope.clone()));
                for tool in vatra_engine::workspace_tools(Arc::clone(store),scope) { registry.register(tool); }
            } else {
                registry.register(vatra_engine::search_workspace_tool(Arc::clone(store)));
            }
        }
        let instructions = load_instructions(&cwd, &cwd, &extra);
        let policy = Arc::new(PermissionPolicy::new(request.mode, rules));
        // The subagents get the tools as they are here, so they have no Agent tool and no Skill tool.
        let runner = Arc::new(SubagentRunner::new(SubagentSetup {
            driver: Arc::clone(&driver),
            hub: Arc::clone(&hub),
            tools: &registry,
            policy: Arc::clone(&policy),
            approver: Arc::clone(&request.approver),
            instructions: instructions.clone(),
            max_steps: self.config.engine.max_steps,
            session: Arc::clone(&session.skills),
            audit: self.store.clone(),
            session_id: Some(request.session_id),
        }));
        extensions.subagents = Some(Arc::clone(&runner));
        registry.register(agent_tool(runner));
        registry.alias("Task", "Agent");
        let extensions = Arc::new(extensions);
        if !extensions.skills.is_empty() {
            registry.register(skill_tool(Arc::clone(&extensions)));
        }
        let context = ToolContext {
            cwd: cwd.clone(),
            project_root: cwd,
            session_dir,
            read_files: Arc::clone(&session.read_files),
            events,
            questions: request.questioner,
            tree: Arc::new(tree),
            background: Arc::clone(&session.background),
            hidden_env: self.hidden_env(),
            memory_dir: Some(memory.clone()),
        };
        Ok(EngineAgent::new(EngineSetup {
            name: request.name,
            driver,
            hub,
            registry,
            policy,
            approver: request.approver,
            context,
            instructions,
            memory: Some(memory),
            max_steps: self.config.engine.max_steps,
            extensions: Some(extensions),
            audit: self.store.clone(),
            session_id: Some(request.session_id),
        })
        .with_effort(effort))
    }
}

#[async_trait]
impl AgentFactory for ConfigAgents {
    fn file_directory(&self,session:i64)->Option<PathBuf>{Some(self.data_dir().join("files").join(session.to_string()))}
    fn input_capabilities(&self,name:&str,picked:Option<&str>)->crate::files::InputCapabilities {
        let mut caps=crate::files::InputCapabilities::default();
        if !self.config.agents.contains_key(name) {
            if let Some(model)=self.models.as_ref().and_then(|m|m.model_for(name)) {
                caps.context_tokens=u64::from(model.settings.ctx);caps.estimated=false;
                caps.images=if model.settings.mmproj.as_ref().is_some_and(|p|p.is_file())&&self.models.as_ref().and_then(|m|m.server().install()).is_some_and(|i|i.caps.mmproj){"supported"}else{"unsupported"}.into();
            }
            return caps;
        }
        if let Ok(AgentConfig::Model{base_url,model,driver,context_window,..})=self.find(name){
            let model=picked.unwrap_or(model).to_lowercase();
            caps.context_tokens=context_window.unwrap_or(8192);caps.estimated=context_window.is_none();
            let native_openai=base_url.as_deref().is_some_and(|url|url.trim_end_matches('/')=="https://api.openai.com/v1");
            let native_anthropic=driver.as_deref()==Some("anthropic")&&base_url.as_deref().is_none_or(|u|u.trim_end_matches('/')=="https://api.anthropic.com");
            if native_anthropic&&model.starts_with("claude-") || native_openai&&["gpt-4o","gpt-4.1","gpt-5","o3","o4"].iter().any(|p|model.starts_with(p)) {caps.images="supported".into();}
            if let Some(url)=base_url.as_ref().and_then(|u|url::Url::parse(u).ok())
                && url.host_str().is_some_and(|h|h=="localhost"||h=="127.0.0.1"||h=="[::1]") {caps.local_probe=base_url.clone();}
        }
        caps
    }
    fn working_directory(&self,session:i64,project:Option<i64>,path:Option<&Path>)->Option<PathBuf>{
        match (project,path){(Some(_),Some(path))=>Some(path.to_path_buf()),(Some(id),None)=>project_workspace(&self.data_dir(),id).ok(),(None,_)=>session_workspace(&self.data_dir(),session).ok()}
    }
    fn file_agent_names(&self) -> Option<Vec<String>> { Some(self.file_agents.clone()) }
    async fn build_parity(&self, request: BuildRequest, task: &vatra_parity::Task) -> Result<Arc<dyn Agent>, String> {
        let local = self.local_agent(&request.name).await?;
        let config = match &local { Some((config,_,_)) => config, None => self.find(&request.name)? };
        if !matches!(config, AgentConfig::Model { .. }) {
            return Err("The isolated fixed-task runner supports local and API engine agents. Use native comparison with an explicit model for Claude Code; ACP isolation is not supported yet.".into());
        }
        let driver = self.driver_for(&request.name, config, None)?;
        let cwd = request.cwd.clone().ok_or("A fixture folder is required")?;
        let servers = if task.needs_mcp {
            vec![McpServerConfig { name:"vatra-fixture-mcp".into(), command:Some(std::env::current_exe().map_err(|e|e.to_string())?.to_string_lossy().into_owned()),
                args:vec!["fixture-mcp".into()], cwd:Some(cwd.clone()), ..Default::default() }]
        } else { Vec::new() };
        let hub = Arc::new(McpHub::new(servers, ExpandVars { hidden:self.hidden_env(), ..Default::default() }));
        hub.ensure_connected().await;
        let mut extensions = EngineExtensions::default();
        if let Some(skill) = &task.skill {
            extensions.skills.push(vatra_extensions::parse_skill(&cwd.join(".claude/skills").join(skill)).map_err(|e|e.to_string())?);
        }
        let mut registry = base_registry(&driver, &hub, None);
        let extensions = Arc::new(extensions);
        if !extensions.skills.is_empty() { registry.register(skill_tool(Arc::clone(&extensions))); }
        let (events,_) = tokio::sync::mpsc::unbounded_channel();
        let context = ToolContext { cwd:cwd.clone(), project_root:cwd.clone(), session_dir:cwd.join(".vatra-session"),
            read_files:Arc::default(), events, questions:request.questioner, tree:Arc::new(ProcessGroup::new().map_err(|e|e.to_string())?),
            background:Arc::new(Background::new()), hidden_env:self.hidden_env(), memory_dir:None };
        fs::create_dir_all(&context.session_dir).map_err(|e|e.to_string())?;
        let inner = Arc::new(EngineAgent::new(EngineSetup { name:request.name, driver, hub, registry,
            policy:Arc::new(PermissionPolicy::new(PermissionMode::Supervised,Rules::default())), approver:request.approver,
            context, instructions:Default::default(), memory:None, max_steps:40, extensions:Some(extensions), audit:None, session_id:None }));
        Ok(Arc::new(ParityAgent { inner, _server:local.map(|(_,guard,_)|guard) }))
    }
    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.config.agents.keys().cloned().collect();
        if let Some(models) = &self.models {
            names.extend(models.agent_names().into_iter().filter(|name| !self.config.agents.contains_key(name)));
        }
        names
    }

    async fn build(&self, request: BuildRequest) -> Result<Arc<dyn Agent>, String> {
        let prefix = format!("{}-", request.session_id);
        let previous: HashSet<_> = self
            .proxy_hubs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect();
        let mut lease = ProxyLease {
            hubs: Arc::clone(&self.proxy_hubs),
            prefix,
            previous,
            owned: None,
        };
        let local = self.local_agent(&request.name).await?;
        let agent_config = match &local {
            Some((config, _, _)) => config,
            None => self.find(&request.name)?,
        };
        let agent: Result<Arc<dyn Agent>, String> = match agent_config {
            AgentConfig::Acp { .. } => {
                let config = self.acp_ready_config(&request).await?;
                let context = self.foreign_context(&request)?;
                let (active, _, _) = self.catalog_view(request.project_id);
                let permissions = AcpPermissions {
                    approver: request.approver,
                    policy: PermissionPolicy::new(request.mode, active.rules),
                    context,
                    approved: Mutex::default(),
                };
                Ok(Arc::new(AcpAgent::new(config, Arc::new(permissions))))
            }
            AgentConfig::Claude { binary } => {
                let (active, _, _) = self.catalog_view(request.project_id);
                let entries = self.entries_for(request.project_id);
                let mut extras = vatra_claude::ClaudeExtras::from_active(&active, &entries);
                for server in &self.config.mcp {
                    if !extras
                        .servers
                        .iter()
                        .any(|other| other.name == server.name && other.plugin == server.plugin)
                    {
                        extras.servers.push(server.clone());
                    }
                }
                let mut context = self.imported_context(
                    &active,
                    &entries,
                    Some(vatra_extensions::Origin::Claude),
                )?;
                if self.bridge.is_some() {
                    context.push_str("\n\n");
                    context.push_str(vatra_engine::WEB_VERIFICATION);
                }
                extras.append_prompt = (!context.is_empty()).then_some(context);
                let mut servers =
                    self.external_servers(extras.servers.clone(), &request, &active)?;
                // Supervised turns receive the bridge through ClaudeApprovals::Prompt.
                // Full-access turns need it explicitly for history, workspace and web search.
                if (request.mode == PermissionMode::FullAccess || self.store.as_ref().is_some_and(|store|store.workspace_turn_context(request.session_id).ok().flatten().is_some()))
                    && let Some(bridge) = &self.bridge
                {
                    servers.push(self.bridge_server(bridge, request.session_id));
                }
                let generated = (!extras.skills.is_empty())
                    .then(|| {
                        vatra_claude::generate_session_dir(
                            &self.data_dir().join("generated"),
                            &extras,
                        )
                    })
                    .transpose()
                    .map_err(|e| e.to_string())?;
                let config = ClaudeConfig {
                    binary: binary.clone(),
                    cwd: Some(self.project_dir(&request)?),
                    servers,
                    approvals: self.claude_approvals(request.session_id, request.mode),
                    plugin_dir: generated.as_ref().map(|dir| dir.plugin_dir().to_path_buf()),
                    append_system_prompt: extras.append_prompt,
                    removed_env: self.hidden_env(),
                    model: request.options.model.clone(),
                    effort: request.options.effort.clone(),
                };
                Ok(match generated {
                    Some(dir) => Arc::new(ClaudeAgent::with_generated(config, dir)),
                    None => Arc::new(ClaudeAgent::new(config)),
                })
            }
            AgentConfig::Model { .. } => {
                let driver = self.driver_for(&request.name, agent_config, request.options.model.as_deref())?;
                let project_dir = self.project_dir(&request)?;
                // Agents are rebuilt for every turn, so the catalog is read once here, at the start of the turn.
                let (mut active, usage, problems) = self.catalog_view(request.project_id);
                let memory = if request.project_id==0{self.data_dir().join("sessions").join(request.session_id.to_string()).join("memory")}
                    else{self.data_dir().join("projects").join(request.project_id.to_string()).join("memory")};
                for path in &active.memory {
                    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
                    let entries = self
                        .catalog
                        .as_ref()
                        .and_then(|catalog| catalog.list().ok())
                        .unwrap_or_default();
                    if let Some(entry) = context_entry(&entries, path) {
                        let source = entry
                            .details
                            .get("source")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(&entry.name);
                        vatra_import::merge_memory(&text, entry.origin, source, &memory)
                            .map_err(|error| error.to_string())?;
                    }
                }
                let hub =
                    self.hubs
                        .get(request.project_id, &project_dir, &self.mcp_servers(&active));
                hub.ensure_connected().await;
                let mut extensions = EngineExtensions::default();
                extensions.skills = std::mem::take(&mut active.skills);
                extensions.usage = usage;
                extensions.catalog = self.catalog.clone();
                extensions.problems = problems;
                let rules = std::mem::take(&mut active.rules);
                let request = BuildRequest {
                    cwd: Some(project_dir),
                    ..request
                };
                Ok(Arc::new(self.engine(
                    request,
                    driver,
                    hub,
                    extensions,
                    rules,
                    active.instructions,
                )?))
            }
        };
        lease.owned = Some(
            self.proxy_hubs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .keys()
                .filter(|key| key.starts_with(&lease.prefix) && !lease.previous.contains(*key))
                .cloned()
                .collect(),
        );
        Ok(Arc::new(LeasedAgent {
            inner: agent?,
            _lease: lease,
            _server: local.map(|(_, guard, _)| guard),
        }))
    }

    fn mcp_servers(&self) -> Vec<McpServerConfig> {
        self.config.mcp.clone()
    }
    async fn probe_mcp(&self,server:McpServerConfig)->Result<serde_json::Value,String>{
        let directory=tempfile::tempdir().map_err(|_|"Couldn't create an isolated MCP test workspace.")?;
        let name=server.name.clone();
        let hub=McpHub::new(vec![server],ExpandVars::from_env(directory.path(),self.hidden_env())).with_secrets(Arc::clone(&self.secrets));
        hub.ensure_server_connected(&name).await;
        let mut result=hub.server_status(&name).ok_or("MCP test did not return a status.")?;
        hub.disconnect(&name).await;
        result["disconnectedAfterTest"]=serde_json::json!(true);
        Ok(result)
    }

    fn secrets(&self) -> Arc<dyn SecretStore> {
        Arc::clone(&self.secrets)
    }
    fn search_chain(&self, store: &vatra_core::Store) -> Result<vatra_engine::SearchChain, String> {
        crate::research::configured_chain(store, self.secrets.as_ref(), self.config.web_search.as_ref())
    }

    fn catalog(&self) -> Option<Arc<Catalog>> {
        self.catalog.clone()
    }
    fn chatgpt(&self) -> Option<Arc<crate::chatgpt::ChatGptAuth>> {
        self.chatgpt.clone()
    }

    fn catalog_changed(&self, project: Option<i64>) {
        self.hubs.forget(project);
    }

    fn models(&self) -> Option<Arc<ModelManager>> {
        self.models.clone()
    }

    fn configured_models(&self) -> Vec<ConfiguredModel> {
        self.config
            .agents
            .iter()
            .filter_map(|(name, agent)| match agent {
                AgentConfig::Model { base_url, model, driver, .. } => Some(ConfiguredModel {
                    name: name.clone(),
                    driver: driver.clone().unwrap_or_else(|| "openai".into()),
                    base_url: base_url.clone(),
                    model: model.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    fn check_costs(&self, name: &str) -> bool {
        // A local model costs nothing, unless the configuration uses its name for something else.
        !(name.starts_with(AGENT_PREFIX) && !self.config.agents.contains_key(name))
    }
    fn workspace_supported(&self, name: &str) -> bool {
        if name.starts_with(AGENT_PREFIX) && !self.config.agents.contains_key(name) { return self.store.is_some(); }
        match self.find(name) {Ok(AgentConfig::Model {..})=>self.store.is_some(),Ok(_)=>self.bridge.is_some(),Err(_)=>false}
    }

    async fn check_target(&self, name: &str) -> Result<CheckTarget, String> {
        let local_model = self
            .models
            .as_ref()
            .filter(|_| !self.config.agents.contains_key(name))
            .and_then(|models| models.model_for(name));
        let llama_build = self.models.as_ref().and_then(|models| models.server().install()?.build);
        let settings_json = match (&local_model, &self.models) {
            (Some(local), Some(models)) => models.settings_key(local),
            _ => "{}".to_string(),
        };
        let (config, guard, ctx, load) = match self.local_agent(name).await {
            Ok(Some((config, guard, ctx))) => (config, Some(guard), ctx, Some(StepOutcome::Passed)),
            Ok(None) => {
                let config = self.find(name)?.clone();
                if !matches!(config, AgentConfig::Model { .. }) {
                    return Err(format!(
                        "only local and API models can be checked; `{name}` runs its own harness"
                    ));
                }
                // A context declared in vatra.toml isn't measured, so it doesn't feed the measured label.
                (config, None, None, None)
            }
            Err(reason) if local_model.is_some() => {
                return Ok(CheckTarget {
                    agent: name.to_string(),
                    openai: None,
                    ctx: None,
                    llama_build,
                    settings_json,
                    load: Some(StepOutcome::Failed { reason }),
                    build: Arc::new(|_, _| Err("the model didn't load".to_string())),
                    guard: None,
                    hidden_env: self.hidden_env(),
                    workdir: None,
                });
            }
            Err(message) => return Err(message),
        };
        let driver = self.driver_for(name, &config, None)?;
        let AgentConfig::Model { base_url, model, api_key_env, driver: kind, api_key: own_key, .. } = &config else {
            unreachable!("only model agents get here")
        };
        let openai = match (kind.as_deref(), base_url) {
            (Some("anthropic" | "copilot"), _) | (_, None) => None,
            (_, Some(base_url)) => Some((
                base_url.clone(),
                own_key.clone().or_else(|| api_key_env.as_ref().and_then(|variable| std::env::var(variable).ok())),
                model.clone(),
            )),
        };
        let settings_json = match local_model {
            Some(_) => settings_json,
            None => crate::models::manager::api_settings_key(kind.as_deref(), base_url.as_deref(), model),
        };
        let (build, workdir) = self.check_builder(name, driver)?;
        Ok(CheckTarget {
            agent: name.to_string(),
            openai,
            ctx,
            llama_build,
            settings_json,
            load,
            build,
            guard,
            hidden_env: self.hidden_env(),
            workdir: Some(workdir),
        })
    }

    fn extension_route(&self, name: &str) -> Option<&'static str> {
        if !self.config.agents.contains_key(name) && name.starts_with(AGENT_PREFIX) {
            return self.models.as_ref()?.model_for(name).map(|_| "engine");
        }
        match self.config.agents.get(name)? {
            AgentConfig::Model { .. } => Some("engine"),
            AgentConfig::Claude { .. } => Some("claude"),
            AgentConfig::Acp { .. } => Some("acp"),
        }
    }
    async fn agent_options(&self, name: &str, project_id: i64, model: Option<&str>) -> Result<serde_json::Value, String> {
        match self.find(name)? {
            AgentConfig::Claude { binary } => return vatra_claude::probe_models(binary, &self.hidden_env()).await,
            AgentConfig::Model { base_url, model, driver, api_key, api_key_env, .. } => {
                let anthropic = driver.as_deref() == Some("anthropic");
                let base = base_url.as_deref().unwrap_or(if anthropic { DEFAULT_BASE_URL } else { "https://api.openai.com/v1" });
                let key = api_key.clone().or_else(|| api_key_env.as_ref().and_then(|name|std::env::var(name).ok()));
                return crate::picker_options::api(base, key.as_deref(), anthropic, model).await;
            }
            AgentConfig::Acp { .. } => {}
        }
        let request = BuildRequest {
            name: name.into(),
            session_id: 0,
            project_id,
            approver: Arc::new(vatra_engine::DenyAll),
            questioner: Arc::new(vatra_engine::NoQuestions),
            mode: PermissionMode::Supervised,
            cwd: None,
            options: TurnOptions::checked(model.map(String::from),None)?,
        };
        let config = self.acp_ready_config(&request).await?;
        serde_json::to_value(vatra_acp::probe_options(&config).await?).map_err(|_| "Could not read agent options".into())
    }
    fn picker_identity(&self, name: &str) -> Option<(String, Option<String>)> {
        match self.find(name).ok()? {
            AgentConfig::Model { base_url, model, driver, .. } => {
                let provider = crate::setup::PROVIDERS.iter().find(|p| p.base_url.is_some() && p.base_url == base_url.as_deref())
                    .map(|p|p.id).unwrap_or(if driver.as_deref() == Some("anthropic") { "anthropic" } else { "custom" });
                Some((provider.into(),Some(model.clone())))
            }
            AgentConfig::Claude { .. } => Some(("claude".into(),None)),
            AgentConfig::Acp { binary,args,codex_binary,chatgpt_plan,.. } => {
                let text = format!("{} {}",binary.display(),args.join(" ")).to_lowercase();
                let provider = if *chatgpt_plan || codex_binary.is_some() || text.contains("codex") { "openai" }
                    else if text.contains("antigravity") { "antigravity" } else { return None; };
                Some((provider.into(),None))
            }
        }
    }
    fn subscription(&self, name: &str) -> bool {
        matches!(
            self.config.agents.get(name),
            Some(
                AgentConfig::Claude { .. }
                    | AgentConfig::Acp {
                        subscription: true,
                        ..
                    }
            )
        )
    }
    fn commands(&self, project: i64) -> Vec<serde_json::Value> {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|state| state.project_id == project)
            .flat_map(|state| state.commands.latest())
            .collect()
    }
    fn proxy_hub(&self, key: &str, project: i64) -> Option<Arc<McpHub>> {
        self.proxy_hubs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .filter(|(id, _)| *id == project)
            .map(|(_, hub)| Arc::clone(hub))
    }

    fn mcp_hub(&self, project_id: i64, folder: Option<&Path>) -> Option<Arc<McpHub>> {
        if project_id == 0 { return None; }
        // The same folder as a turn of this project: its own folder, else the workspace, else the current folder.
        let project_dir = match (folder, &self.bridge) {
            (Some(folder), _) => folder.to_path_buf(),
            (None, Some(bridge)) => project_workspace(&bridge.data_dir, project_id).ok()?,
            (None, None) => std::env::current_dir().ok()?,
        };
        let (active, _, _) = self.catalog_view(project_id);
        Some(
            self.hubs
                .get(project_id, &project_dir, &self.mcp_servers(&active)),
        )
    }

    async fn complete_text(&self, agent: Option<&str>, system: &str, user: &str) -> Result<String, String> {
        let name = agent.unwrap_or(&self.config.summarizer);
        let driver: Arc<dyn ModelDriver>;
        let _guard;
        if let (Some(models), true) = (&self.models, name.starts_with(AGENT_PREFIX))
            && !self.config.agents.contains_key(name)
        {
            let (model, server, guard) = models.resolve(name).await?;
            _guard = Some(guard);
            driver = with_retries(Arc::new(OpenAiDriver::new(ModelConfig {
                base_url: server.base_url,
                model: model.slug,
                api_key: Some(server.api_key),
                context_window: server.ctx.map(u64::from),
            })));
        } else {
            _guard = None;
            driver = self.model_driver(name)?;
        }
        let request = vatra_engine::model::ModelRequest {
            messages: vec![
                vatra_engine::model::Message::System { text: system.into() },
                vatra_engine::model::Message::User { text: user.into(), images: vec![] },
            ],
            tools: vec![],
            effort: None,
        };
        let completion = driver.stream(&request, &mut |_: &str| {}).await.map_err(|e| e.to_string())?;
        completion.text.filter(|t| !t.trim().is_empty()).ok_or_else(|| "The model gave no answer.".to_string())
    }

    fn summarizer(&self) -> Arc<dyn Summarizer> {
        if let (Some(models), true) = (&self.models, self.config.summarizer.starts_with(AGENT_PREFIX))
            && !self.config.agents.contains_key(&self.config.summarizer)
        {
            return Arc::new(LocalSummarizer { models: Arc::clone(models), name: self.config.summarizer.clone() });
        }
        match self.model_driver(&self.config.summarizer) {
            Ok(driver) => Arc::new(OpenAiSummarizer::new(driver)),
            Err(message) => {
                eprintln!("warning: automatic summaries aren't available: {message}");
                Arc::new(UnavailableSummarizer(message))
            }
        }
    }
}

struct ProxyLease {
    hubs: ProxyHubs,
    prefix: String,
    previous: HashSet<String>,
    owned: Option<HashSet<String>>,
}
impl Drop for ProxyLease {
    fn drop(&mut self) {
        self.hubs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|key, _| match &self.owned {
                Some(owned) => !owned.contains(key),
                None => !key.starts_with(&self.prefix) || self.previous.contains(key),
            });
    }
}
struct LeasedAgent {
    inner: Arc<dyn Agent>,
    _lease: ProxyLease,
    /// Keeps a local model's server running while the agent is used.
    _server: Option<TurnGuard>,
}
struct ParityAgent { inner: Arc<dyn Agent>, _server: Option<TurnGuard> }
#[async_trait]
impl Agent for ParityAgent {
    fn name(&self) -> &str { self.inner.name() }
    async fn run_turn(&self, request:vatra_core::TurnRequest, sink:tokio::sync::mpsc::UnboundedSender<vatra_core::Event>) -> Result<vatra_core::TurnOutcome,AgentError> {
        self.inner.run_turn(request,sink).await
    }
}
#[async_trait]
impl Agent for LeasedAgent {
    fn name(&self) -> &str {
        self.inner.name()
    }
    async fn run_turn(
        &self,
        request: vatra_core::TurnRequest,
        sink: tokio::sync::mpsc::UnboundedSender<vatra_core::Event>,
    ) -> Result<vatra_core::TurnOutcome, AgentError> {
        self.inner.run_turn(request, sink).await
    }
}
struct AcpPermissions {
    approver: Arc<dyn Approver>,
    policy: PermissionPolicy,
    context: ToolContext,
    approved: Mutex<HashSet<String>>,
}

#[async_trait]
impl PermissionHandler for AcpPermissions {
    async fn approve(&self, tool_call: &serde_json::Value) -> bool {
        let checks = vatra_acp::permission_checks(tool_call, &self.context.cwd);
        let mut ask = false;
        for check in checks {
            match self
                .policy
                .decide(&check.name, check.class, &check.arguments, &self.context)
            {
                vatra_engine::Decision::Deny(_) => return false,
                vatra_engine::Decision::Ask => ask = true,
                vatra_engine::Decision::Allow => {}
            }
        }
        !ask || self.approver.approve("ACP tool", tool_call).await
    }
    async fn skill_body(&self, skill: &vatra_extensions::Skill) -> Result<String, String> {
        vatra_engine::run_injections(
            &skill.body,
            &vatra_engine::ShellCtx {
                context: &self.context,
                policy: &self.policy,
                approver: self.approver.as_ref(),
                approved: &self.approved,
                skill: &skill.qualified_name(),
                timeout: vatra_engine::COMMAND_TIMEOUT,
            },
        )
        .await
        .map_err(|e| e.to_string())
    }
}

/// The instructions or memory entry whose stored copy holds the file. Other kinds never match: a permission entry
/// has no folder, and every path starts with an empty one.
fn context_entry<'a>(
    entries: &'a [vatra_extensions::CatalogEntry],
    path: &std::path::Path,
) -> Option<&'a vatra_extensions::CatalogEntry> {
    entries.iter().find(|entry| {
        matches!(
            entry.kind,
            vatra_extensions::ElementKind::Instructions | vatra_extensions::ElementKind::Memory
        ) && !entry.content_dir.as_os_str().is_empty()
            && path.starts_with(&entry.content_dir)
    })
}

/// Summaries on a local model: its server is started for the summary, like for a turn.
struct LocalSummarizer {
    models: Arc<ModelManager>,
    name: String,
}

#[async_trait]
impl Summarizer for LocalSummarizer {
    async fn summarize(&self, transcript: &str) -> Result<BriefFields, AgentError> {
        let (model, server, _guard) = self.models.resolve(&self.name).await.map_err(AgentError::Failed)?;
        let config = ModelConfig {
            base_url: server.base_url,
            model: model.slug,
            api_key: Some(server.api_key),
            context_window: server.ctx.map(u64::from),
        };
        let driver: Arc<dyn ModelDriver> = with_retries(Arc::new(OpenAiDriver::new(config)));
        OpenAiSummarizer::new(driver).summarize(transcript).await
    }
}

struct UnavailableSummarizer(String);

#[async_trait]
impl Summarizer for UnavailableSummarizer {
    async fn summarize(&self, _transcript: &str) -> Result<BriefFields, AgentError> {
        Err(AgentError::Failed(self.0.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vatra_extensions::{CatalogEntry, ElementKind, Origin};

    #[test]
    fn chat_and_research_share_provider_settings_and_honor_duckduckgo_off() {
        let store = Arc::new(vatra_core::Store::open_in_memory().unwrap());
        let secrets = vatra_core::MemorySecretStore::default();
        assert_eq!(crate::research::configured_chain(&store, &secrets, None).unwrap().names(), vec!["duckduckgo"]);
        store.set_app_setting("search.duckduckgo", Some("off")).unwrap();
        assert!(crate::research::configured_chain(&store, &secrets, None).is_err());
        let factory = ConfigAgents::new(Config::empty()).with_store(Arc::clone(&store));
        assert!(factory.chat_search_tool().is_none());
        store.set_app_setting("search.duckduckgo", Some("on")).unwrap();
        assert_eq!(factory.chat_search_tool().unwrap().def().name, "WebSearch");
        store.save_connector(&vatra_core::ConnectorRow {
            id:"brave-test".into(), kind:"websearch".into(), name:"test".into(),
            settings_json:serde_json::json!({"provider":"brave"}).to_string(), created_at:1,
        }).unwrap();
        // An account with a missing key is skipped, without disabling free search.
        assert_eq!(crate::research::configured_chain(&store, &secrets, None).unwrap().names(), vec!["duckduckgo"]);
        secrets.set(&crate::connectors::connector_secret("brave-test", "api_key"), "test-only-key").unwrap();
        assert_eq!(crate::research::configured_chain(&store, &secrets, None).unwrap().names(), vec!["brave", "duckduckgo"]);
        store.set_app_setting("search.duckduckgo", Some("off")).unwrap();
        assert_eq!(crate::research::configured_chain(&store, &secrets, None).unwrap().names(), vec!["brave"]);
    }

    fn entry(kind: ElementKind, origin: Origin, dir: &str) -> CatalogEntry {
        CatalogEntry {
            id: 0,
            kind,
            name: "x".into(),
            origin,
            scope: Scope::Global,
            content_hash: String::new(),
            content_dir: dir.into(),
            enabled: true,
            runs_code: vec![],
            approved_hash: None,
            plugin: None,
            details: serde_json::Value::Null,
        }
    }

    #[test]
    fn a_permission_entry_never_claims_a_context_file() {
        let entries = [
            entry(ElementKind::Permission, Origin::Claude, ""),
            entry(ElementKind::Instructions, Origin::Codex, "store/codex"),
        ];
        let found = context_entry(&entries, Path::new("store/codex/AGENTS.md")).unwrap();
        assert_eq!(found.origin, Origin::Codex);
        assert!(context_entry(&entries, Path::new("store/other/AGENTS.md")).is_none());
    }
}
