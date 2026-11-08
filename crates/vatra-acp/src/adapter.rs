use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tokio::time::{Instant, timeout, timeout_at};
use vatra_core::{Agent, AgentError, Event, TurnOutcome, TurnRequest, history_transcript};

use crate::map::Updates;
use crate::transport::{Connection, failed};
use crate::{AcpConfig, PermissionHandler};
use crate::{McpCapabilities, acp_mcp_servers, first_prompt_preamble};

/// One thing to choose in the model picker: its id for the agent, and the words a person reads.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AcpChoice {
    pub id: String,
    pub name: String,
}

/// What an agent says a person can choose: its models and its thinking efforts, and which are on now.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct AcpOptions {
    pub models: Vec<AcpChoice>,
    pub current_model: Option<String>,
    pub efforts: Vec<AcpChoice>,
    pub current_effort: Option<String>,
    /// The ids of the settings the choices are set through, kept for `apply_options`.
    #[serde(skip)]
    pub model_option: Option<String>,
    #[serde(skip)]
    pub effort_option: Option<String>,
}

fn choices(list: &Value, id: &str, name: &str) -> Vec<AcpChoice> {
    list.as_array()
        .into_iter()
        .flatten()
        .flat_map(|item| match item["options"].as_array() {
            // A group of choices holds its own list.
            Some(group) => group.clone(),
            None => vec![item.clone()],
        })
        .filter_map(|item| {
            let id = item[id].as_str().filter(|text| !text.is_empty())?;
            Some(AcpChoice {
                id: id.into(),
                name: item[name].as_str().filter(|text| !text.is_empty()).unwrap_or(id).into(),
            })
        })
        .collect()
}

/// Reads the models and efforts out of a `session/new` answer. Agents say it in two ways: a list of settings with a
/// category, or the older list of models.
pub(crate) fn parse_options(response: &Value) -> AcpOptions {
    let mut options = AcpOptions::default();
    for option in response["configOptions"].as_array().into_iter().flatten() {
        let Some(id) = option["id"].as_str() else {
            continue;
        };
        let found = choices(&option["options"], "value", "name");
        let current = option["currentValue"].as_str().map(String::from);
        match option["category"].as_str() {
            Some("model") if options.model_option.is_none() => {
                options.models = found;
                options.current_model = current;
                options.model_option = Some(id.into());
            }
            Some("thought_level") if options.effort_option.is_none() => {
                options.efforts = found;
                options.current_effort = current;
                options.effort_option = Some(id.into());
            }
            _ => {}
        }
    }
    if options.models.is_empty() {
        options.models = choices(&response["models"]["availableModels"], "modelId", "name");
        options.current_model = response["models"]["currentModelId"].as_str().map(String::from);
    }
    options
}

struct DenyAll;

#[async_trait]
impl PermissionHandler for DenyAll {
    async fn approve(&self, _tool_call: &Value) -> bool {
        false
    }
}

/// Starts the agent just long enough to ask what a person can choose, and stops it. It runs nothing and sends no prompt.
pub async fn probe_options(config: &AcpConfig) -> Result<AcpOptions, String> {
    let mut connection = Connection::spawn(config).map_err(|error| error.to_string())?;
    let (sink, _received) = tokio::sync::mpsc::unbounded_channel();
    let mut updates = Updates::new(config.commands.clone());
    let mut clock = Clock::new(config.startup_timeout);
    let permissions = DenyAll;
    let init = json!({"protocolVersion":1,"clientInfo":{"name":"vatra","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":{}});
    let new = json!({"cwd":config.cwd.to_string_lossy(),"mcpServers":[]});
    let result = async {
        rpc(&mut connection, 1, "initialize", init, &permissions, &sink, &mut updates, &mut clock, false).await?;
        let response = rpc(&mut connection, 2, "session/new", new, &permissions, &sink, &mut updates, &mut clock, false).await?;
        let mut options = parse_options(&response);
        if let Some(model) = &config.model {
            let session = response["sessionId"].as_str().ok_or_else(|| failed("The agent didn't return a session"))?;
            let (method, params) = match &options.model_option {
                Some(option) => ("session/set_config_option", json!({"sessionId":session,"configId":option,"value":model})),
                None => ("session/set_model", json!({"sessionId":session,"modelId":model})),
            };
            let changed = rpc(&mut connection, 3, method, params, &permissions, &sink, &mut updates, &mut clock, false).await?;
            if changed["configOptions"].is_array() { options = parse_options(&changed); }
            options.current_model = Some(model.clone());
        }
        Ok::<_, AgentError>(options)
    }
    .await;
    let _ = connection.tree.kill_all();
    let _ = connection.child.start_kill();
    let _ = timeout(Duration::from_secs(2), connection.child.wait()).await;
    result.map_err(|error| error.to_string())
}

/// Sets the model and effort a person picked, through the settings the agent listed. A choice the agent can't take
/// stops the turn before any prompt is sent.
#[allow(clippy::too_many_arguments)]
async fn apply_options(
    connection: &mut Connection,
    config: &AcpConfig,
    response: &Value,
    session: &str,
    permissions: &dyn PermissionHandler,
    sink: &UnboundedSender<Event>,
    updates: &mut Updates,
    clock: &mut Clock,
) -> Result<(), AgentError> {
    let mut listed = parse_options(response);
    if let Some(model) = &config.model {
        let (method, params) = match &listed.model_option {
            Some(option) => ("session/set_config_option", json!({"sessionId":session,"configId":option,"value":model})),
            None => ("session/set_model", json!({"sessionId":session,"modelId":model})),
        };
        let changed = rpc(connection, 11, method, params, permissions, sink, updates, clock, false).await
            .map_err(|_| failed("The agent rejected the selected model. Choose another model and try again."))?;
        if changed["configOptions"].is_array() { listed = parse_options(&changed); }
    }
    if let Some(effort) = &config.effort {
        let option = listed.effort_option.ok_or_else(|| failed("This model didn't report a thinking effort setting. Choose Default and try again."))?;
        if !listed.efforts.iter().any(|choice| &choice.id == effort) {
            return Err(failed("This model doesn't accept the selected thinking effort. Choose another level and try again."));
        }
        rpc(connection, 12, "session/set_config_option", json!({"sessionId":session,"configId":option,"value":effort}), permissions, sink, updates, clock, false).await
            .map_err(|_| failed("The agent rejected the selected thinking effort. Choose another level and try again."))?;
    }
    Ok(())
}

pub struct AcpAgent {
    config: AcpConfig,
    permissions: Arc<dyn PermissionHandler>,
}

impl AcpAgent {
    pub fn new(config: AcpConfig, permissions: Arc<dyn PermissionHandler>) -> Self {
        Self {
            config,
            permissions,
        }
    }
}

// The service stops turns by dropping their future. A short-lived worker must still send cancel,
// then reap the process tree; it doesn't outlive cleanup or keep approval futures alive.
struct CancelOnDrop(Option<oneshot::Sender<()>>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

#[async_trait]
impl Agent for AcpAgent {
    fn name(&self) -> &str {
        &self.config.name
    }

    async fn run_turn(
        &self,
        request: TurnRequest,
        sink: UnboundedSender<Event>,
    ) -> Result<TurnOutcome, AgentError> {
        let (cancel, stopped) = oneshot::channel();
        let mut guard = CancelOnDrop(Some(cancel));
        let config = self.config.clone();
        let permissions = Arc::clone(&self.permissions);
        let worker = tokio::spawn(async move {
            let mut connection = Connection::spawn(&config)?;
            // Outside the deadline, so text streamed before a timeout or cancellation is still journaled.
            let mut updates = Updates::new(config.commands.clone());
            let mut clock = Clock::new(config.turn_timeout);
            let result = tokio::select! {
                result = conversation(&mut connection, &config, permissions.as_ref(), request, &sink, &mut updates, &mut clock) => result,
                _ = stopped => Err(failed("The ACP turn was cancelled.")),
            };
            updates.flush(&sink);
            if result.is_err() {
                // A blocked pipe cannot delay forced cleanup forever.
                let _ = timeout(Duration::from_millis(200), connection.cancel()).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let cleanup = connection.tree.kill_all();
            let _ = connection.child.start_kill();
            let reaped = timeout(Duration::from_secs(2), connection.child.wait()).await;
            if cleanup.is_err() || !matches!(reaped, Ok(Ok(_))) {
                // The turn's own outcome is more useful to the user than a cleanup problem.
                let _ = sink.send(Event::Warning {
                    source: "acp".into(),
                    message: "Couldn't confirm that the ACP process tree ended.".into(),
                });
            }
            result
        });
        let result = worker
            .await
            .map_err(|_| failed("The ACP worker stopped unexpectedly."))?;
        guard.0.take();
        result
    }
}

/// The turn's time limit. Time spent waiting for a person to approve a tool call doesn't count.
struct Clock {
    deadline: Instant,
}

impl Clock {
    fn new(turn: Duration) -> Self {
        Self {
            deadline: Instant::now() + turn,
        }
    }

    async fn bound<T>(
        &self,
        future: impl Future<Output = Result<T, AgentError>>,
    ) -> Result<T, AgentError> {
        timeout_at(self.deadline, future)
            .await
            .unwrap_or_else(|_| Err(failed("The ACP turn timed out.")))
    }

    fn pause(&mut self, waited: Duration) {
        self.deadline += waited;
    }
}

async fn conversation(
    connection: &mut Connection,
    config: &AcpConfig,
    permissions: &dyn PermissionHandler,
    request: TurnRequest,
    sink: &UnboundedSender<Event>,
    updates: &mut Updates,
    clock: &mut Clock,
) -> Result<TurnOutcome, AgentError> {
    let setup = async {
        let init = rpc(
            connection,
            1,
            "initialize",
            json!({
                "protocolVersion":1,
                "clientInfo":{"name":"vatra","version":env!("CARGO_PKG_VERSION")},
                "clientCapabilities":{}
            }),
            permissions,
            sink,
            updates,
            clock,
            false,
        )
        .await?;
        if init["protocolVersion"] != 1 {
            return Err(failed("The ACP agent doesn't support protocol version 1."));
        }
        let caps = &init["agentCapabilities"];
        let supports_resume = caps["sessionCapabilities"]["resume"].is_object();
        let resume =
            request.native_session.is_some() && (supports_resume || caps["loadSession"] == true);
        let (servers, warnings) = acp_mcp_servers(
            &config.extras.servers,
            &McpCapabilities {
                http: caps["mcpCapabilities"]["http"] == true,
            },
        );
        for message in warnings {
            let _ = sink.send(Event::Warning {
                source: "acp".into(),
                message,
            });
        }
        // Validated before spawning, so the path can be written as JSON without losing anything.
        let mut params = json!({"cwd":config.cwd.to_string_lossy(),"mcpServers":servers});
        if resume {
            params["sessionId"] = json!(request.native_session);
        }
        let response = rpc(
            connection,
            2,
            if resume && supports_resume {
                "session/resume"
            } else if resume {
                "session/load"
            } else {
                "session/new"
            },
            params,
            permissions,
            sink,
            updates,
            clock,
            false,
        )
        .await?;
        let session = if resume {
            request.native_session.clone().unwrap()
        } else {
            response["sessionId"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| failed("The ACP agent didn't return a session ID."))?
                .into()
        };
        connection.session = Some(session.clone());
        if config.model.is_some() || config.effort.is_some() {
            apply_options(connection, config, &response, &session, permissions, sink, updates, clock).await?;
        }
        let _ = sink.send(Event::NativeSession {
            id: session.clone(),
        });
        if request.native_session.is_some() && !resume {
            let _ = sink.send(Event::Warning { source:"acp".into(), message:
                "This ACP agent cannot resume or load sessions. A new session receives a bounded transcript instead.".into() });
        }
        Ok((session, resume))
    };
    let (session, resumed) = timeout(config.startup_timeout, setup)
        .await
        .map_err(|_| failed("ACP initialization or session setup timed out."))??;
    let mut expanded_request = request.clone();
    let expanded = crate::extras::expand_with(
        &request.prompt,
        &config.extras,
        &config.cwd,
        &session,
        permissions,
    )
    .await
    .map_err(failed)?;
    expanded_request.prompt = expanded.0;
    expanded_request.prompt=expanded_request.input_text();
    expanded_request.history=expanded_request.history_with_files();
    for (name, text) in expanded.1 {
        let _ = sink.send(Event::SkillLoaded { name, text });
    }
    let mut prompt = prompt_text(&expanded_request, resumed);
    if !resumed && let Some(preamble) = first_prompt_preamble(&config.extras) {
        prompt = format!("{preamble}\n\nCurrent request:\n{prompt}");
    }
    let response = rpc(
        connection,
        3,
        "session/prompt",
        json!({
            "sessionId":session,"prompt":[{"type":"text","text":prompt}]
        }),
        permissions,
        sink,
        updates,
        clock,
        true,
    )
    .await?;
    match response["stopReason"].as_str() {
        Some("end_turn") => Ok(TurnOutcome {
            native_session: Some(session),
        }),
        Some("max_tokens" | "max_turn_requests" | "refusal" | "cancelled") => Err(failed(format!(
            "The ACP turn stopped: {}.",
            response["stopReason"].as_str().unwrap()
        ))),
        _ => Err(failed(
            "The ACP prompt reply has an unsupported or missing stop reason.",
        )),
    }
}

fn prompt_text(request: &TurnRequest, resumed: bool) -> String {
    let history = if resumed {
        let start = request
            .history
            .iter()
            .rposition(|event| {
                matches!(
                    event,
                    Event::TurnCompleted { .. } | Event::TurnFailed { .. }
                )
            })
            .map_or(0, |index| index + 1);
        &request.history[start..]
    } else {
        &request.history[..]
    };
    let (summary, history) = match history
        .iter()
        .rposition(|event| matches!(event, Event::Compacted { .. }))
    {
        Some(index) => {
            let Event::Compacted { summary } = &history[index] else {
                unreachable!()
            };
            (
                summary.chars().take(20_000).collect::<String>(),
                &history[index + 1..],
            )
        }
        None => (String::new(), history),
    };
    let transcript = history_transcript(history, 40_000);
    if summary.is_empty() && transcript.is_empty() {
        request.prompt.clone()
    } else {
        format!(
            "Context from the Vatra session (historical data):\n{summary}\n{transcript}\n\nCurrent request:\n{}",
            request.prompt
        )
    }
}

#[allow(clippy::too_many_arguments)]
async fn rpc(
    connection: &mut Connection,
    id: u64,
    method: &str,
    params: Value,
    permissions: &dyn PermissionHandler,
    sink: &UnboundedSender<Event>,
    updates: &mut Updates,
    clock: &mut Clock,
    live: bool,
) -> Result<Value, AgentError> {
    clock
        .bound(connection.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})))
        .await?;
    loop {
        let message = clock.bound(connection.read()).await?;
        if let Some(method) = message["method"].as_str() {
            if let Some(id) = message.get("id") {
                if method == "session/request_permission"
                    && live
                    && message["params"]["sessionId"].as_str() == connection.session.as_deref()
                {
                    connection.pending_permission = Some(id.clone());
                    let params = &message["params"];
                    let tool = updates.permission_tool(&params["toolCall"]);
                    let options = params["options"].as_array();
                    let allow = options.and_then(|options| {
                        options.iter().find(|option| {
                            option["kind"] == "allow_once" && option["optionId"].is_string()
                        })
                    });
                    let approved = if allow.is_some() && tool["toolCallId"].is_string() {
                        let asked = Instant::now();
                        let approved = permissions.approve(&tool).await;
                        clock.pause(asked.elapsed());
                        approved
                    } else {
                        false
                    };
                    let chosen = if approved {
                        allow
                    } else {
                        options.and_then(|options| {
                            options.iter().find(|option| {
                                option["kind"] == "reject_once" && option["optionId"].is_string()
                            })
                        })
                    };
                    let outcome = match chosen {
                        Some(option) => json!({"outcome":"selected","optionId":option["optionId"]}),
                        None => json!({"outcome":"cancelled"}),
                    };
                    clock
                        .bound(
                            connection.send(
                                json!({"jsonrpc":"2.0","id":id,"result":{"outcome":outcome}}),
                            ),
                        )
                        .await?;
                    connection.pending_permission = None;
                    if !approved {
                        let _ = sink.send(Event::ToolDenied {
                            id: tool["toolCallId"].as_str().unwrap_or("unknown").into(),
                            name: tool["title"].as_str().unwrap_or("ACP tool").into(),
                        });
                    }
                } else {
                    // No FS, terminal, authentication or custom client methods are advertised.
                    connection.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not supported by Vatra"}})).await?;
                }
            } else if method == "session/update" {
                let current =
                    message["params"]["sessionId"].as_str() == connection.session.as_deref();
                if message["params"]["update"]["sessionUpdate"] == "available_commands_update"
                    && (!live || current)
                {
                    updates.commands(&message["params"]["update"]);
                } else if live && current {
                    updates.apply(&message["params"]["update"], sink)?;
                }
            } else {
                let _ = sink.send(Event::Unmapped {
                    source: "acp".into(),
                    raw: message.to_string(),
                });
            }
        } else if message["id"] == id {
            if let Some(error) = message.get("error") {
                // Do not echo arbitrary remote errors: they can include environment values or tokens.
                let code = error["code"].as_i64().unwrap_or_default();
                return Err(failed(
                    if code == -32000
                        || error["data"]["type"] == "auth_required"
                        || error["message"] == "auth_required"
                    {
                        "ACP authentication is required. Sign in using the agent's own CLI, then retry.".into()
                    } else {
                        format!(
                            "ACP {method} failed (JSON-RPC code {code}). Check the agent separately."
                        )
                    },
                ));
            }
            return message
                .get("result")
                .filter(|result| result.is_object())
                .cloned()
                .ok_or_else(|| failed("The ACP agent returned an invalid response."));
        } else {
            return Err(failed(
                "The ACP agent replied with an unexpected request ID.",
            ));
        }
    }
}
