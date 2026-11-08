use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use vatra_core::{
    Agent, AgentError, Event, McpServerConfig, TurnOutcome, TurnRequest, claude_mcp_config,
    history_transcript,
};

use crate::GeneratedDir;
use crate::parse::{Parsed, parse_line};

const STDERR_EXCERPT_CHARS: usize = 2_000;
const HISTORY_MAX_CHARS: usize = 40_000;
const PATH_EXTENSIONS: [&str; 3] = ["exe", "cmd", "bat"];

#[derive(Clone, Debug)]
pub enum ClaudeApprovals {
    /// Only the catalog's MCP servers run without a prompt; nobody can answer the rest.
    Catalog,
    Deny,
    /// Prompts go to an MCP tool, which asks the user in the app.
    Prompt {
        server: Box<McpServerConfig>,
        tool: String,
        accept_edits: bool,
    },
    FullAccess,
}

#[derive(Clone, Debug)]
pub struct ClaudeConfig {
    pub binary: PathBuf,
    pub cwd: Option<PathBuf>,
    pub servers: Vec<McpServerConfig>,
    pub approvals: ClaudeApprovals,
    pub plugin_dir: Option<PathBuf>,
    pub append_system_prompt: Option<String>,
    pub removed_env: Vec<String>,
    /// The model of the session, as `claude --model` takes it. None keeps Claude Code's own choice.
    pub model: Option<String>,
    /// The effort of the session: `low`, `medium`, `high`, `xhigh` or `max`. None keeps Claude Code's own choice.
    pub effort: Option<String>,
}

pub struct ClaudeAgent {
    config: ClaudeConfig,
    generated: Option<GeneratedDir>,
}

impl ClaudeAgent {
    pub fn new(config: ClaudeConfig) -> ClaudeAgent {
        ClaudeAgent {
            config,
            generated: None,
        }
    }

    pub fn with_generated(mut config: ClaudeConfig, generated: GeneratedDir) -> ClaudeAgent {
        if !generated.skill_names().is_empty() {
            config.plugin_dir = Some(generated.plugin_dir().to_path_buf());
        }
        ClaudeAgent {
            config,
            generated: Some(generated),
        }
    }
}

pub fn claude_args(config: &ClaudeConfig, native_session: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--forward-subagent-text",
        "--mcp-config",
    ]
    .map(String::from)
    .into();
    let mut servers = config.servers.clone();
    if let ClaudeApprovals::Prompt { server, .. } = &config.approvals {
        servers.push(server.as_ref().clone());
    }
    for server in &mut servers {
        server.name = mcp_name(server);
    }
    args.push(claude_mcp_config(&servers).to_string());
    if let Some(directory) = &config.plugin_dir {
        args.extend([
            "--plugin-dir".to_string(),
            directory.to_string_lossy().into_owned(),
        ]);
    }
    if let Some(prompt) = &config.append_system_prompt {
        args.extend(["--append-system-prompt".to_string(), prompt.clone()]);
    }
    if let Some(session) = native_session {
        args.extend(["--resume".to_string(), session.to_string()]);
    }
    if let Some(model) = &config.model {
        args.extend(["--model".to_string(), model.clone()]);
    }
    if let Some(effort) = &config.effort {
        args.extend(["--effort".to_string(), effort.clone()]);
    }
    match &config.approvals {
        ClaudeApprovals::Deny => {
            args.extend(["--permission-prompts".to_string(), "none".to_string()])
        }
        ClaudeApprovals::Catalog => {
            if !config.servers.is_empty() {
                args.push("--allowedTools".to_string());
                args.extend(
                    config
                        .servers
                        .iter()
                        .map(|server| format!("mcp__{}", mcp_name(server))),
                );
            }
        }
        ClaudeApprovals::Prompt {
            tool, accept_edits, ..
        } => {
            args.extend(["--permission-prompt-tool".to_string(), tool.clone()]);
            if *accept_edits {
                args.extend(["--permission-mode".to_string(), "acceptEdits".to_string()]);
            }
        }
        ClaudeApprovals::FullAccess => {
            args.extend([
                "--permission-mode".to_string(),
                "bypassPermissions".to_string(),
            ]);
        }
    }
    args
}

fn mcp_name(server: &McpServerConfig) -> String {
    server.plugin.as_ref().map_or_else(
        || server.name.clone(),
        |plugin| format!("plugin_{plugin}_{}", server.name),
    )
}

/// `Command` finds only `.exe` on Windows, while npm installs a `.cmd` shim.
pub fn find_on_path(binary: &Path, path_variable: &OsStr) -> Option<PathBuf> {
    if binary.components().count() != 1 || binary.extension().is_some() {
        return None;
    }
    std::env::split_paths(path_variable).find_map(|directory| {
        PATH_EXTENSIONS.iter().find_map(|extension| {
            let candidate = directory.join(binary).with_extension(extension);
            candidate.is_file().then_some(candidate)
        })
    })
}

fn resolve_binary(binary: &Path) -> PathBuf {
    let path_variable = std::env::var_os("PATH").unwrap_or_default();
    find_on_path(binary, &path_variable).unwrap_or_else(|| binary.to_path_buf())
}

/// `--resume` restores Claude's own conversation, so only what it can't know is added to the prompt.
fn stdin_text(request: &TurnRequest) -> String {
    let context = if request.native_session.is_some() {
        let turn_end = request
            .history
            .iter()
            .rposition(|event| {
                matches!(
                    event,
                    Event::TurnCompleted { .. } | Event::TurnFailed { .. }
                )
            })
            .map_or(0, |index| index + 1);
        history_transcript(&request.history[turn_end..], HISTORY_MAX_CHARS)
    } else {
        history_transcript(&request.history, HISTORY_MAX_CHARS)
    };
    if context.is_empty() {
        request.prompt.clone()
    } else {
        format!(
            "Context from the Vatra session:\n{context}\n\n{}",
            request.prompt
        )
    }
}

#[async_trait]
impl Agent for ClaudeAgent {
    fn name(&self) -> &str {
        "claude"
    }

    async fn run_turn(
        &self,
        request: TurnRequest,
        sink: UnboundedSender<Event>,
    ) -> Result<TurnOutcome, AgentError> {
        let mut request = request;
        request.history=request.history_with_files();
        request.prompt=request.input_text();
        if request.files.iter().any(|f|f.history_index.is_none()&&f.image_base64.is_some()) {
            return Err(AgentError::Failed("This Claude CLI route does not yet accept image attachments. Choose a compatible model route.".into()));
        }
        if let Some(generated) = &self.generated {
            request.prompt = generated.translate_prompt(&request.prompt);
        }
        let prompt = stdin_text(&request);
        let mut command = Command::new(resolve_binary(&self.config.binary));
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        command
            .args(claude_args(&self.config, request.native_session.as_deref()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for name in &self.config.removed_env {
            command.env_remove(name);
        }
        if let Some(cwd) = &self.config.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn().map_err(|error| match error.kind() {
            ErrorKind::NotFound => AgentError::NotInstalled(format!(
                "Claude Code not found at `{}` (searched PATH for .exe, .cmd and .bat)",
                self.config.binary.display()
            )),
            _ => AgentError::Failed(format!("couldn't start Claude Code: {error}")),
        })?;

        // Lives as long as this future: stopping a turn drops it and ends the whole process tree.
        #[cfg(windows)]
        let _tree = vatra_core::ProcessGroup::new()
            .and_then(|tree| tree.adopt(&child).map(|()| tree))
            .ok();

        let mut stdin = child.stdin.take().expect("stdin is configured");
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout is configured"));
        let mut stderr = child.stderr.take().expect("stderr is configured");

        let write_prompt = async {
            // A process that exits early closes stdin. Its exit code reports the real problem.
            let _ = stdin.write_all(prompt.as_bytes()).await;
            let _ = stdin.shutdown().await;
            drop(stdin);
        };
        let read_errors = async {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes).await;
            String::from_utf8_lossy(&bytes).into_owned()
        };
        let read_stream = async {
            let mut native_session = None;
            let mut completed = false;
            let mut failure = None;
            let mut buffer = Vec::new();
            // Read to the end no matter what: a child blocked on a full pipe would never exit.
            loop {
                buffer.clear();
                match stdout.read_until(b'\n', &mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let line = String::from_utf8_lossy(&buffer);
                for parsed in parse_line(line.trim_end_matches(['\r', '\n'])) {
                    let event = match parsed {
                        Parsed::Event(event) => event,
                        Parsed::NativeSession(session) => {
                            native_session = Some(session.clone());
                            Event::NativeSession { id: session }
                        }
                        Parsed::Completed => {
                            completed = true;
                            continue;
                        }
                        Parsed::Failed(message) => {
                            failure = Some(message);
                            continue;
                        }
                    };
                    if sink.send(event).is_err() && failure.is_none() {
                        failure = Some("the turn was stopped".to_string());
                    }
                }
            }
            (native_session, completed, failure)
        };
        let (_, errors, (native_session, completed, failure)) =
            tokio::join!(write_prompt, read_errors, read_stream);

        let status = child
            .wait()
            .await
            .map_err(|error| AgentError::Failed(format!("lost track of Claude Code: {error}")))?;
        if let Some(message) = failure {
            return Err(AgentError::Failed(message));
        }
        if !status.success() {
            let excerpt: String = errors.chars().take(STDERR_EXCERPT_CHARS).collect();
            return Err(AgentError::Failed(format!(
                "Claude Code exited with {status}: {excerpt}"
            )));
        }
        if !completed {
            return Err(AgentError::Failed(
                "Claude Code exited without a result".into(),
            ));
        }
        Ok(TurnOutcome { native_session })
    }
}
