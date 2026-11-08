use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, ErrorCode, GetPromptRequestParams, ReadResourceRequestParams,
    ResourceContents, Role,
};
use rmcp::service::{RunningService, ServiceError};
use rmcp::{RoleClient, ServiceExt};
use serde_json::Value;
use tokio::process::{Child, Command};
use vatra_core::{McpServerConfig, MemorySecretStore, ProcessGroup, SecretRef, SecretStore};

use super::ToolDef;
use super::expand::{ExpandVars, expand};
use super::schema::normalize_schema;
use super::shorten_output;
use super::{http, oauth};
use crate::model::ImagePart;
use crate::tools::ToolOutput;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const CALL_TIMEOUT: Duration = Duration::from_secs(300);
/// After this many crashes in a row a server stays down until `reconnect` starts it again.
const MAX_CRASHES: u32 = 5;
const NON_TEXT_PLACEHOLDER: &str = "[non-text content omitted]";

type Client = Arc<RunningService<RoleClient, ()>>;

/// A prompt that a server offers. Its name is qualified like a tool's, `mcp__<server>__<prompt>`.
#[derive(Clone, Debug, PartialEq)]
pub struct McpPrompt {
    pub name: String,
    pub description: String,
    pub arguments: Vec<McpPromptArgument>,
}

/// An argument that a prompt takes.
#[derive(Clone, Debug, PartialEq)]
pub struct McpPromptArgument {
    pub name: String,
    pub description: String,
    pub required: bool,
}

/// A resource that a server offers. It is read with `read_resource`, by the server's name and its uri.
#[derive(Clone, Debug, PartialEq)]
pub struct McpResource {
    pub server: String,
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: Option<String>,
}

/// What a server offers, as it was listed when it started. `problems` are the listings that failed.
#[derive(Default)]
struct Listing {
    tools: Vec<ToolDef>,
    all_tools: Vec<ToolDef>,
    prompts: Vec<McpPrompt>,
    resources: Vec<McpResource>,
    problems: Vec<String>,
}

/// The MCP servers of one project. They start when first needed, and a server that fails stays failed
/// until it is reconnected.
pub struct McpHub {
    servers: Vec<Server>,
    vars: ExpandVars,
    /// Where keychain references are read from and OAuth tokens are saved to.
    secrets: Arc<dyn SecretStore>,
}

struct Server {
    /// The part of the tool names that comes from the server: `demo`, or `plugin_<plugin>_<name>`.
    key: String,
    config: McpServerConfig,
    /// Keeps two calls from starting the same server at once.
    starting: tokio::sync::Mutex<()>,
    state: Mutex<State>,
}

/// The child process of a stdio server, held with its connection. Dropping it ends the child and the processes the
/// child started, so a server started through a wrapper such as `npx` leaves nothing behind.
struct Launched {
    _child: Child,
    _tree: ProcessGroup,
}

#[derive(Default)]
struct State {
    client: Option<Client>,
    process: Option<Launched>,
    tools: Vec<ToolDef>,
    all_tools: Vec<ToolDef>,
    disconnected: bool,
    prompts: Vec<McpPrompt>,
    resources: Vec<McpResource>,
    /// Listings that failed when the server started. The server still works without them.
    problems: Vec<String>,
    /// Why the server is down for good: it did not start, or it crashed too often.
    failed: Option<String>,
    /// Crashes since the last call that worked.
    crashes: u32,
    /// An OAuth server with no login yet. It is tried again on every turn, so a login made meanwhile applies.
    needs_login: bool,
}

impl Server {
    fn new(config: McpServerConfig) -> Server {
        let key = match &config.plugin {
            Some(plugin) => format!("plugin_{plugin}_{}", config.name),
            None => config.name.clone(),
        };
        Server {
            key,
            config,
            starting: tokio::sync::Mutex::new(()),
            state: Mutex::default(),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The process of this server died. Too many deaths in a row stop it for good.
    fn crashed(&self, client: &Client) {
        let mut state = self.state();
        if !state
            .client
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, client))
        {
            return;
        }
        state.client = None;
        state.process = None;
        state.crashes += 1;
        if state.crashes >= MAX_CRASHES {
            state.failed = Some(format!(
                "it crashed {} times in a row; reconnect it to try again",
                state.crashes
            ));
        }
    }
}

impl McpHub {
    /// Holds the servers without starting them. Nothing runs until a turn needs a server.
    pub fn new(servers: Vec<McpServerConfig>, vars: ExpandVars) -> McpHub {
        let secrets = Arc::new(MemorySecretStore::default());
        McpHub {
            servers: servers.into_iter().map(Server::new).collect(),
            vars,
            secrets,
        }
    }

    /// The store that keychain references are read from and OAuth tokens are saved to.
    pub fn with_secrets(mut self, secrets: Arc<dyn SecretStore>) -> McpHub {
        self.secrets = secrets;
        self
    }

    /// Starts every server that is not running yet. A server that fails is kept in `failures`.
    pub async fn ensure_connected(&self) {
        for server in &self.servers {
            self.start(server).await;
        }
    }

    /// Starts only the selected server for a transport proxy; listing it must not launch unrelated servers.
    pub async fn ensure_server_connected(&self, key: &str) -> bool {
        let Some(server) = self.servers.iter().find(|server| server.key == key) else {
            return false;
        };
        self.start(server).await;
        true
    }

    pub fn tools(&self) -> Vec<ToolDef> {
        self.servers
            .iter()
            .flat_map(|server| server.state().tools.clone())
            .collect()
    }

    pub fn prompts(&self) -> Vec<McpPrompt> {
        self.servers
            .iter()
            .flat_map(|server| server.state().prompts.clone())
            .collect()
    }

    pub fn resources(&self) -> Vec<McpResource> {
        self.servers
            .iter()
            .flat_map(|server| server.state().resources.clone())
            .collect()
    }

    /// A connection snapshot; does not start a process or invoke any tool.
    pub fn server_status(&self, key:&str)->Option<Value>{
        let server=self.servers.iter().find(|server|server.key==key)?;
        let state=server.state();
        let prefix=format!("mcp__{}__",server.key);
        Some(serde_json::json!({
            "name":key,"connected":state.client.as_ref().is_some_and(|client|!client.is_closed()),
            "disconnected":state.disconnected,"error":state.failed,"problems":state.problems,"needsLogin":state.needs_login,
            "serverInfo":state.client.as_ref().and_then(|client|client.peer_info()),
            "tools":state.all_tools.iter().map(|tool|{let name=tool.name.strip_prefix(&prefix).unwrap_or(&tool.name);serde_json::json!({"name":name,"description":tool.description,"inputSchema":tool.parameters,"enabled":allowed(&server.config,name)})}).collect::<Vec<_>>(),
            "prompts":state.prompts.iter().map(|prompt|serde_json::json!({"name":prompt.name,"description":prompt.description,"arguments":prompt.arguments.iter().map(|arg|serde_json::json!({"name":arg.name,"description":arg.description,"required":arg.required})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "resources":state.resources.iter().map(|resource|serde_json::json!({"uri":resource.uri,"name":resource.name,"description":resource.description,"mimeType":resource.mime_type})).collect::<Vec<_>>()
        }))
    }

    /// Human-triggered shutdown. Calls on this hub cannot silently reconnect it;
    /// an explicit reconnect or a new configuration snapshot is required.
    pub async fn disconnect(&self,key:&str){
        let Some(server)=self.servers.iter().find(|server|server.key==key)else{return;};
        let _starting=server.starting.lock().await;
        let mut state=server.state();
        if let Some(client)=state.client.take(){client.cancellation_token().cancel();}
        state.process=None;state.tools.clear();state.prompts.clear();state.resources.clear();
        state.disconnected=true;
    }

    /// Whether the server of a tool is always loaded, so its tools never wait for ToolSearch.
    pub fn always_load(&self, name: &str) -> bool {
        self.split(name)
            .is_some_and(|(server, _)| server.config.always_load)
    }

    /// The servers that are down, with why, and the listings that failed when a server started.
    pub fn failures(&self) -> Vec<(String, String)> {
        self.servers
            .iter()
            .flat_map(|server| {
                let state = server.state();
                let messages: Vec<String> = state
                    .failed
                    .iter()
                    .chain(&state.problems)
                    .cloned()
                    .collect();
                messages
                    .into_iter()
                    .map(|message| (server.key.clone(), message))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Starts a server that is down, whether it failed or not. `server` is the name the tools carry.
    pub async fn reconnect(&self, server: &str) {
        let Some(server) = self
            .servers
            .iter()
            .find(|candidate| candidate.key == server)
        else {
            return;
        };
        {
            let mut state = server.state();
            state.client = None;
            state.process = None;
            state.failed = None;
            state.crashes = 0;
            state.needs_login = false;
            state.disconnected = false;
        }
        self.start(server).await;
    }

    pub async fn call(&self, name: &str, arguments: Value) -> ToolOutput {
        match self.call_raw(name, arguments).await {
            Ok(result) => {
                let mut images = vec![];
                let text = result
                    .content
                    .iter()
                    .filter_map(|block| {
                        if let Some(image) = block.as_image() {
                            images.push(ImagePart {
                                media_type: image.mime_type.clone(),
                                base64: image.data.clone(),
                            });
                            return None;
                        }
                        Some(
                            block
                                .as_text()
                                .map_or(NON_TEXT_PLACEHOLDER, |text| text.text.as_str()),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                ToolOutput {
                    text,
                    is_error: result.is_error.unwrap_or(false),
                    images,
                }
            }
            Err(message) => ToolOutput::error(message),
        }
    }

    /// The complete MCP result for a CLI transport proxy. The engine's text view is built separately, so images,
    /// embedded resources, annotations and structured content survive forwarding to the official client.
    pub async fn call_raw(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<rmcp::model::CallToolResult, String> {
        let Some((server, tool)) = self.split(name) else {
            return Err(format!("unknown tool name: {name}"));
        };
        if !allowed(&server.config, tool) {
            return Err(format!(
                "the tool `{tool}` is disabled for MCP server `{}`",
                server.key
            ));
        }
        let client = match self.client(server).await {
            Ok(client) => client,
            Err(message) => {
                return Err(format!(
                    "MCP server `{}` isn't connected: {message}",
                    server.key
                ));
            }
        };
        let Value::Object(arguments) = arguments else {
            return Err("arguments must be a JSON object".into());
        };
        let request = CallToolRequestParams::new(tool.to_string()).with_arguments(arguments);
        let limit = server
            .config
            .tool_timeout_sec
            .map_or(CALL_TIMEOUT, Duration::from_secs);
        match tokio::time::timeout(limit, client.call_tool(request)).await {
            Ok(Ok(result)) => {
                server.state().crashes = 0;
                Ok(result)
            }
            Ok(Err(error)) => {
                if !matches!(
                    error,
                    ServiceError::McpError(_) | ServiceError::Timeout { .. }
                ) {
                    server.crashed(&client);
                }
                Err(format!("the MCP call failed: {error}"))
            }
            Err(_) => Err(format!(
                "the MCP call passed the limit of {} seconds",
                limit.as_secs_f64()
            )),
        }
    }

    /// The text of a prompt, with its arguments filled in. The prompt is named as `prompts` lists it.
    pub async fn get_prompt(&self, name: &str, arguments: Value) -> Result<String, String> {
        let Some((server, prompt)) = self.split(name) else {
            return Err(format!("unknown prompt name: {name}"));
        };
        let client = self.client(server).await?;
        let Value::Object(arguments) = arguments else {
            return Err("arguments must be a JSON object".into());
        };
        let mut request = GetPromptRequestParams::new(prompt);
        request.arguments = Some(arguments);
        let result = client
            .get_prompt(request)
            .await
            .map_err(|error| format!("the MCP prompt failed: {error}"))?;
        // Each message keeps its role, and a message without text says so instead of leaving a gap.
        let messages: Vec<String> = result
            .messages
            .iter()
            .map(|message| {
                let role = match message.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };
                let body = match message.content.as_text() {
                    Some(text) => text.text.clone(),
                    None if message.content.as_image().is_some() => {
                        "[non-text content not included: image]".into()
                    }
                    None => "[non-text content not included]".into(),
                };
                format!("[{role}] {body}")
            })
            .collect();
        Ok(messages.join("\n\n"))
    }

    /// The contents of a resource of a server. Images come back as images, other binary content is left out. A long
    /// text is saved to the session folder, as a long tool result is.
    pub async fn read_resource(
        &self,
        server: &str,
        uri: &str,
        session_dir: &Path,
    ) -> Result<ToolOutput, String> {
        let Some(server) = self
            .servers
            .iter()
            .find(|candidate| candidate.key == server)
        else {
            return Err(format!("unknown MCP server: {server}"));
        };
        let client = self.client(server).await?;
        let result = client
            .read_resource(ReadResourceRequestParams::new(uri))
            .await
            .map_err(|error| format!("the MCP read failed: {error}"))?;
        let mut texts = vec![];
        let mut images = vec![];
        for contents in result.contents {
            match contents {
                ResourceContents::TextResourceContents { text, .. } => texts.push(text),
                ResourceContents::BlobResourceContents {
                    blob,
                    mime_type: Some(mime_type),
                    ..
                } if mime_type.starts_with("image/") => {
                    images.push(ImagePart {
                        media_type: mime_type,
                        base64: blob,
                    });
                }
                _ => texts.push(NON_TEXT_PLACEHOLDER.into()),
            }
        }
        Ok(shorten_output(
            ToolOutput {
                text: texts.join("\n"),
                is_error: false,
                images,
            },
            session_dir,
        ))
    }

    /// Finds the server of a tool name by the servers of this hub, since a name can hold `__` in either part.
    fn split<'a>(&'a self, name: &'a str) -> Option<(&'a Server, &'a str)> {
        self.servers
            .iter()
            .filter_map(|server| {
                let prefix = format!("mcp__{}__", server.key);
                name.strip_prefix(prefix.as_str())
                    .map(|tool| (server, tool))
            })
            .max_by_key(|(server, _)| server.key.len())
    }

    async fn client(&self, server: &Server) -> Result<Client, String> {
        self.start(server).await;
        let state = server.state();
        match &state.client {
            Some(client) => Ok(Arc::clone(client)),
            None => Err(state
                .failed
                .clone()
                .unwrap_or_else(|| "it isn't running".into())),
        }
    }

    /// Starts the server unless it runs or has failed for good.
    async fn start(&self, server: &Server) {
        let _turn = server.starting.lock().await;
        let closed = server
            .state()
            .client
            .clone()
            .filter(|client| client.is_closed());
        if let Some(client) = closed {
            server.crashed(&client);
        }
        {
            let state = server.state();
            if state.disconnected || state.client.is_some() || (state.failed.is_some() && !state.needs_login) {
                return;
            }
        }
        let limit = server
            .config
            .startup_timeout_sec
            .map_or(CONNECT_TIMEOUT, Duration::from_secs);
        let result = match tokio::time::timeout(limit, self.spawn(server)).await {
            Ok(result) => result,
            Err(_) => Err("timed out while connecting".into()),
        };
        // A missing or rejected login is retried on every turn, so a login made meanwhile applies.
        let needs_login = match &result {
            Err(message) => message == oauth::NEEDS_LOGIN || self.needs_login(server).await,
            Ok(_) => false,
        };
        let mut state = server.state();
        match result {
            Ok((client, process, listing)) => {
                state.client = Some(client);
                state.process = process;
                state.tools = listing.tools;
                state.all_tools = listing.all_tools;
                state.prompts = listing.prompts;
                state.resources = listing.resources;
                state.problems = listing.problems;
                state.failed = None;
                state.needs_login = false;
            }
            Err(message) => {
                state.failed = Some(message);
                state.needs_login = needs_login;
            }
        }
    }

    /// Whether an OAuth server has no token yet, so its failure is a missing login and not a fault.
    async fn needs_login(&self, server: &Server) -> bool {
        match (&server.config.url, &server.config.oauth) {
            (Some(url), Some(_)) => {
                matches!(
                    oauth::has_credentials(url, Arc::clone(&self.secrets)).await,
                    Ok(false)
                )
            }
            _ => false,
        }
    }

    async fn spawn(&self, server: &Server) -> Result<(Client, Option<Launched>, Listing), String> {
        let config = &server.config;
        let vars = self.server_vars(config).await?;
        if config.url.is_some() && self.needs_login(server).await {
            return Err(oauth::NEEDS_LOGIN.into());
        }
        let (client, process) = match (&config.url, &config.command) {
            (Some(url), _) => (
                http::connect(config, url, &vars, Arc::clone(&self.secrets)).await?,
                None,
            ),
            (None, Some(command)) => {
                let (client, process) = self.spawn_stdio(config, command, &vars).await?;
                (client, Some(process))
            }
            (None, None) => return Err("it has no command".into()),
        };
        let has_tools = client
            .peer_info()
            .map(|info| info.capabilities.tools.is_some())
            .unwrap_or(true); // assume tools when capabilities unknown (pre-2024 servers)
        let all_tools: Vec<ToolDef> = if has_tools {
            let listed = client
                .list_all_tools()
                .await
                .map_err(|error| error.to_string())?;
            listed
                .into_iter()
                .map(|tool| ToolDef {
                    name: format!("mcp__{}__{}", server.key, tool.name),
                    description: tool.description.as_deref().unwrap_or_default().to_string(),
                    parameters: normalize_schema(Value::Object(tool.input_schema.as_ref().clone())),
                })
                .collect()
        } else {
            vec![]
        };
        let prefix=format!("mcp__{}__",server.key);
        let tools=all_tools.iter().filter(|tool|allowed(config,tool.name.strip_prefix(&prefix).unwrap_or(&tool.name))).cloned().collect();
        let mut listing = list_prompts_and_resources(&client, server, tools).await;
        listing.all_tools=all_tools;
        Ok((Arc::new(client), process, listing))
    }

    /// Starts a server that runs as a process of this machine.
    async fn spawn_stdio(
        &self,
        config: &McpServerConfig,
        command: &str,
        vars: &ExpandVars,
    ) -> Result<(RunningService<RoleClient, ()>, Launched), String> {
        let mut missing = Vec::new();
        let mut reserved = Vec::new();
        let mut take = |text: &str| {
            let expanded = expand(text, vars);
            missing.extend(expanded.missing);
            reserved.extend(expanded.reserved);
            expanded.text
        };
        let program = take(command);
        let args: Vec<String> = config.args.iter().map(|arg| take(arg.as_str())).collect();
        let env: BTreeMap<String, String> = config
            .env
            .iter()
            .map(|(key, value)| (key.clone(), take(value.as_str())))
            .collect();
        let cwd = config
            .cwd
            .as_ref()
            .map(|cwd| PathBuf::from(take(cwd.to_string_lossy().as_ref())));
        if !reserved.is_empty() {
            return Err(format!(
                "it uses reserved variables: {}; the service's own variables are never passed on",
                reserved.join(", ")
            ));
        }
        if !missing.is_empty() {
            return Err(format!("it uses unset variables: {}", missing.join(", ")));
        }

        #[cfg(windows)]
        let (program,args)=vatra_core::node_package_runner(&program,&args,&std::env::var_os("PATH").unwrap_or_default())?.unwrap_or((PathBuf::from(program),args));
        #[cfg(windows)]
        if program.extension().is_some_and(|extension|extension.eq_ignore_ascii_case("cmd")||extension.eq_ignore_ascii_case("bat")){
            return Err("MCP needs a native executable. Configure node.exe and the installed JavaScript entry point instead of a shell shim.".into());
        }
        let mut process = Command::new(&program);
        #[cfg(windows)]
        process.creation_flags(0x0800_0000);
        // The service's own variables stay in the service, unless this server's config names them.
        for (name, _) in std::env::vars() {
            if self.vars.is_reserved(&name) {
                process.env_remove(&name);
            }
        }
        process.args(&args).envs(&env);
        if let Some(cwd) = cwd {
            process.current_dir(cwd);
        }
        // The server's children stay in the group the child starts in, so ending the group ends the whole tree. Dropping
        // the child kills it at once: the transport's own drop kills from a spawned task, which never runs when the
        // runtime is already shutting down.
        #[cfg(unix)]
        process.process_group(0);
        process
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = process.spawn().map_err(|error| error.to_string())?;
        let tree = ProcessGroup::new().map_err(|error| error.to_string())?;
        tree.adopt(&child).map_err(|error| error.to_string())?;
        let (Some(stdout), Some(stdin)) = (child.stdout.take(), child.stdin.take()) else {
            return Err("the server's pipes couldn't be opened".into());
        };
        let client = ().serve((stdout, stdin)).await.map_err(|error| error.to_string())?;
        Ok((
            client,
            Launched {
                _child: child,
                _tree: tree,
            },
        ))
    }

    /// The hub's variables with the plugin's context and this server's secrets added. A secret is named like
    /// a variable.
    async fn server_vars(&self, config: &McpServerConfig) -> Result<ExpandVars, String> {
        let mut vars = self.vars.clone();
        if let Some(context) = &config.plugin_context {
            vars.plugin_root = Some(context.root.clone());
            vars.plugin_data = Some(context.data.clone());
            vars.user_config = context.user_config.clone();
        }
        for (name, secret) in &config.secrets {
            match secret {
                SecretRef::Env(variable) => {
                    if self.vars.is_reserved(variable) {
                        return Err(format!(
                            "secret `{name}` reads the reserved variable `{variable}`"
                        ));
                    }
                    match self.vars.env.get(variable) {
                        Some(value) => {
                            vars.env.insert(name.clone(), value.clone());
                        }
                        None => {
                            return Err(format!(
                                "secret `{name}` needs the environment variable `{variable}`, which isn't set"
                            ));
                        }
                    }
                }
                SecretRef::Keychain(entry) => match oauth::blocking(Arc::clone(&self.secrets), {
                    let entry = entry.clone();
                    move |secrets| secrets.get(&entry)
                })
                .await
                {
                    Ok(Some(value)) => {
                        vars.env.insert(name.clone(), value);
                    }
                    Ok(None) => {
                        return Err(format!(
                            "secret `{name}` needs the keychain entry `{entry}`, which isn't set"
                        ));
                    }
                    Err(error) => return Err(format!("secret `{name}` couldn't be read: {error}")),
                },
            }
        }
        Ok(vars)
    }
}

/// The prompts and resources of a server, when it offers them. A server that fails to list them keeps its tools.
async fn list_prompts_and_resources(
    client: &RunningService<RoleClient, ()>,
    server: &Server,
    tools: Vec<ToolDef>,
) -> Listing {
    let capabilities = client
        .peer_info()
        .map(|info| info.capabilities.clone())
        .unwrap_or_default();
    let mut problems = vec![];
    let prompts = if capabilities.prompts.is_some() {
        listed(client.list_all_prompts().await, "prompts", &mut problems)
    } else {
        vec![]
    };
    let resources = if capabilities.resources.is_some() {
        listed(
            client.list_all_resources().await,
            "resources",
            &mut problems,
        )
    } else {
        vec![]
    };
    Listing {
        tools,
        all_tools:vec![],
        prompts: prompts
            .into_iter()
            .map(|prompt| McpPrompt {
                name: format!("mcp__{}__{}", server.key, prompt.name),
                description: prompt.description.unwrap_or_default(),
                arguments: prompt
                    .arguments
                    .unwrap_or_default()
                    .into_iter()
                    .map(|argument| McpPromptArgument {
                        name: argument.name,
                        description: argument.description.unwrap_or_default(),
                        required: argument.required.unwrap_or(false),
                    })
                    .collect(),
            })
            .collect(),
        resources: resources
            .into_iter()
            .map(|resource| McpResource {
                server: server.key.clone(),
                uri: resource.uri,
                name: resource.name,
                description: resource.description.unwrap_or_default(),
                mime_type: resource.mime_type,
            })
            .collect(),
        problems,
    }
}

/// The items of one listing. A server that doesn't have the listing answers "method not found", which is no problem.
/// Any other failure is recorded in `problems`, and the server keeps its other parts.
fn listed<T>(
    result: Result<Vec<T>, ServiceError>,
    what: &str,
    problems: &mut Vec<String>,
) -> Vec<T> {
    match result {
        Ok(items) => items,
        Err(ServiceError::McpError(error)) if error.code == ErrorCode::METHOD_NOT_FOUND => vec![],
        Err(error) => {
            problems.push(format!("couldn't list its {what}: {error}"));
            vec![]
        }
    }
}

fn allowed(config: &McpServerConfig, tool: &str) -> bool {
    let enabled = config
        .enabled_tools
        .as_ref()
        .is_none_or(|enabled| enabled.iter().any(|name| name == tool));
    enabled && !config.disabled_tools.iter().any(|name| name == tool)
}

/// One hub per workspace identity. Projectless conversations have distinct directories.
pub struct HubPool {
    hubs: Mutex<HashMap<(i64, std::path::PathBuf), Arc<McpHub>>>,
    /// The variables that servers must not read, from the configured agents.
    hidden: Vec<String>,
    secrets: Arc<dyn SecretStore>,
}

impl Default for HubPool {
    fn default() -> HubPool {
        HubPool::new(Vec::new())
    }
}

impl HubPool {
    pub fn new(hidden: Vec<String>) -> HubPool {
        HubPool {
            hubs: Mutex::default(),
            hidden,
            secrets: Arc::new(MemorySecretStore::default()),
        }
    }

    /// The store that the hubs of this pool read keychain references from and save OAuth tokens to.
    pub fn with_secrets(mut self, secrets: Arc<dyn SecretStore>) -> HubPool {
        self.secrets = secrets;
        self
    }

    pub fn get(
        &self,
        project_id: i64,
        project_dir: &Path,
        servers: &[McpServerConfig],
    ) -> Arc<McpHub> {
        let mut hubs = self.hubs.lock().unwrap_or_else(PoisonError::into_inner);
        let hub = hubs.entry((project_id,project_dir.to_path_buf())).or_insert_with(|| {
            let vars = ExpandVars::from_env(project_dir, self.hidden.clone());
            let hub = McpHub::new(servers.to_vec(), vars).with_secrets(Arc::clone(&self.secrets));
            Arc::new(hub)
        });
        Arc::clone(hub)
    }

    /// Drops the hub of one project, or of every project when `project` is None, so the next turn builds it from the
    /// catalog as it is now. A turn that holds a hub keeps it until it ends.
    pub fn forget(&self, project: Option<i64>) {
        let mut hubs = self.hubs.lock().unwrap_or_else(PoisonError::into_inner);
        match project {
            Some(project_id) => {
                  hubs.retain(|(id,_),_|*id!=project_id);
            }
            None => hubs.clear(),
        }
    }
}
