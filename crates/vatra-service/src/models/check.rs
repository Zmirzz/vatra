//! The short check: speed, two tool calls and one MCP call, on the real engine agent. It proves those three
//! things on this setup and nothing more.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};
use vatra_core::{Agent, Event, McpServerConfig, TurnRequest};
use vatra_engine::{ExpandVars, McpHub, Tool};
use vatra_models::label::{self, CheckResults, Label, LabelReason, Speed, StepOutcome, ToolScore};

use super::server::TurnGuard;

const STEP_TIMEOUT: Duration = Duration::from_secs(90);
const TOOL_TRIES: u8 = 2;
/// About 300 tokens of plain text, so prompt speed has something to measure.
const SPEED_TEXT: &str = "Vatra is a desktop app that brings conversations, agents and shared context into one place. \
It runs local models through llama.cpp and remote models through their APIs. The same extensions, such as skills and \
MCP servers, reach every model. A person can start a conversation with a small local model, hand it to a larger one \
when the task grows, and keep the history, decisions and files in the project. Models differ in what they can do: \
some call tools reliably, some only chat, some need a lot of memory. Vatra estimates what fits on the computer, \
measures what actually works, and says plainly what was measured and what was only estimated. ";
const SPEED_PROMPT: &str = "Summarize this text in five short bullet points.";

pub type BuildAgent = Arc<dyn Fn(Arc<McpHub>, Vec<Arc<dyn Tool>>) -> Result<Arc<dyn Agent>, String> + Send + Sync>;

/// Everything the check needs about one agent.
pub struct CheckTarget {
    pub agent: String,
    /// An OpenAI-compatible endpoint for the speed step: base URL, key and model. None skips the step.
    pub openai: Option<(String, Option<String>, String)>,
    pub ctx: Option<u32>,
    pub llama_build: Option<u32>,
    pub settings_json: String,
    /// The local server's start, when there is one.
    pub load: Option<StepOutcome>,
    pub build: BuildAgent,
    pub guard: Option<TurnGuard>,
    /// Variables the check's MCP server must not inherit, such as API keys.
    pub hidden_env: Vec<String>,
    /// The folder the check worked in, removed at the end.
    pub workdir: Option<std::path::PathBuf>,
}

/// Why a try failed: the model's own answer, or the connection to it.
enum Failure {
    Model(String),
    Transport(String),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckProgress {
    pub agent: String,
    /// `load`, `speed`, `tools`, `mcp` or `done`.
    pub step: &'static str,
    pub outcome: Option<StepOutcome>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRecord {
    pub agent: String,
    pub results: CheckResults,
    pub label: Label,
    pub reasons: Vec<LabelReason>,
    pub llama_build: Option<u32>,
    pub settings_json: String,
    pub started_at: i64,
    pub finished_at: i64,
    /// Set when the check couldn't finish for reasons other than the model's answers, such as a timeout, a
    /// refused key or a model that didn't load. Such a run says nothing about the model and isn't kept.
    pub incomplete: Option<String>,
}

pub async fn run_check(
    target: CheckTarget,
    probe: McpServerConfig,
    progress: &(dyn Fn(CheckProgress) + Send + Sync),
) -> CheckRecord {
    let started_at = super::now();
    let report = |step, outcome: Option<StepOutcome>| progress(CheckProgress { agent: target.agent.clone(), step, outcome });
    let mut results = CheckResults { load: target.load.clone(), context: target.ctx, ..CheckResults::default() };
    report("load", target.load.clone());
    let mut incomplete = match &target.load {
        Some(StepOutcome::Failed { reason }) => Some(reason.clone()),
        _ => None,
    };
    let loaded = incomplete.is_none();
    if loaded {
        let (speed, outcome) = match &target.openai {
            Some((base, key, model)) => match speed(base, key.as_deref(), model).await {
                Ok(speed) => (Some(speed), StepOutcome::Passed),
                Err(reason) => (None, StepOutcome::Failed { reason }),
            },
            None => (None, StepOutcome::Skipped { reason: "speed is measured on OpenAI-compatible servers only".into() }),
        };
        results.speed = speed;
        results.speed_step = Some(outcome.clone());
        report("speed", Some(outcome));

        let empty = Arc::new(McpHub::new(Vec::new(), ExpandVars::default()));
        let mut score = ToolScore { passed: 0, of: TOOL_TRIES };
        for _ in 0..TOOL_TRIES {
            match tool_try(&target.build, &empty, target.workdir.as_deref()).await {
                Ok(()) => score.passed += 1,
                Err(Failure::Model(reason)) => results.tool_failures.push(reason),
                Err(Failure::Transport(reason)) => {
                    incomplete.get_or_insert(reason.clone());
                    results.tool_failures.push(reason);
                }
            }
        }
        results.tools = score;
        let tools_outcome = if score.passed == 0 {
            StepOutcome::Failed { reason: results.tool_failures.join("; ") }
        } else {
            StepOutcome::Passed
        };
        report("tools", Some(tools_outcome));

        let vars = ExpandVars { hidden: target.hidden_env.clone(), ..ExpandVars::default() };
        let mcp = match mcp_try(&target.build, probe, vars).await {
            Ok(()) => StepOutcome::Passed,
            Err(Failure::Model(reason)) => StepOutcome::Failed { reason },
            Err(Failure::Transport(reason)) => {
                incomplete.get_or_insert(reason.clone());
                StepOutcome::Failed { reason }
            }
        };
        results.mcp = Some(mcp.clone());
        report("mcp", Some(mcp));
    }
    let (label, reasons) = match incomplete {
        None => label::measured(&results),
        Some(_) => (Label::NotChecked, Vec::new()),
    };
    report("done", None);
    drop(target.guard);
    if let Some(dir) = &target.workdir {
        let _ = std::fs::remove_dir_all(dir);
    }
    CheckRecord {
        agent: target.agent,
        results,
        label,
        reasons,
        llama_build: target.llama_build,
        settings_json: target.settings_json,
        started_at,
        finished_at: super::now(),
        incomplete,
    }
}

async fn speed(base: &str, key: Option<&str>, model: &str) -> Result<Speed, String> {
    let body = json!({
        "model": model,
        "messages": [{ "role": "user", "content": format!("{SPEED_TEXT}\n\n{SPEED_PROMPT}") }],
        "max_tokens": 128,
        "stream": false,
    });
    let mut request = reqwest::Client::new().post(format!("{}/chat/completions", base.trim_end_matches('/'))).json(&body);
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    let started = Instant::now();
    let response = tokio::time::timeout(STEP_TIMEOUT, request.send())
        .await
        .map_err(|_| "no answer in 90 seconds".to_string())?
        .map_err(|error| error.without_url().to_string())?;
    if !response.status().is_success() {
        return Err(format!("the server answered {}", response.status()));
    }
    let reply: Value = response.json().await.map_err(|error| error.to_string())?;
    let seconds = started.elapsed().as_secs_f64();
    let text = reply["choices"][0]["message"]["content"].as_str().unwrap_or_default();
    let reasoning = reply["choices"][0]["message"]["reasoning_content"].as_str().unwrap_or_default();
    if text.trim().is_empty() && reasoning.trim().is_empty() {
        return Err("the answer was empty".into());
    }
    let timings = &reply["timings"];
    let gen_tps = timings["predicted_per_second"].as_f64().or_else(|| {
        let tokens = reply["usage"]["completion_tokens"].as_f64()?;
        (seconds > 0.0).then(|| tokens / seconds)
    });
    Ok(Speed { prompt_tps: timings["prompt_per_second"].as_f64(), gen_tps, seconds })
}

/// A random code the model can only know by calling the tool.
fn code() -> String {
    const LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    (0..6).map(|_| LETTERS[rand::random_range(0..LETTERS.len())] as char).collect()
}

/// The built-in tool the tool step asks for, and the file it reads.
pub const READ: &str = "Read";
const CODE_FILE: &str = "access-code.txt";

async fn tool_try(build: &BuildAgent, hub: &Arc<McpHub>, workdir: Option<&std::path::Path>) -> Result<(), Failure> {
    let code = code();
    let dir = workdir.ok_or_else(|| Failure::Transport("the check has no folder".into()))?;
    std::fs::write(dir.join(CODE_FILE), format!("The access code is {code}.\n"))
        .map_err(|error| Failure::Transport(error.to_string()))?;
    let agent = build(Arc::clone(hub), Vec::new()).map_err(Failure::Transport)?;
    let prompt = format!("Read the file {CODE_FILE} in the working folder with the Read tool. Then reply with only the access code it contains.");
    let events = turn(agent, &prompt).await?;
    let called = events.iter().any(|event| {
        matches!(event, Event::ToolCall { name, arguments, .. }
            if name == READ && arguments.to_string().contains(CODE_FILE))
    });
    if !called {
        return Err(Failure::Model(match events.iter().find_map(|event| match event {
            Event::ToolCall { name, .. } => Some(name.clone()),
            _ => None,
        }) {
            Some(other) => format!("called `{other}` instead of {READ} on {CODE_FILE}"),
            None => format!("answered without calling the tool: \"{}\"", said(&events)),
        }));
    }
    answered_with(&events, &code).map_err(Failure::Model)
}

async fn mcp_try(build: &BuildAgent, probe: McpServerConfig, vars: ExpandVars) -> Result<(), Failure> {
    let expected = probe.env.get(super::probe::TOKEN_VAR).cloned().unwrap_or_default();
    let hub = Arc::new(McpHub::new(vec![probe], vars));
    hub.ensure_connected().await;
    if hub.tools().is_empty() {
        return Err(Failure::Transport("the check's MCP server didn't start".into()));
    }
    let agent = build(Arc::clone(&hub), Vec::new()).map_err(Failure::Transport)?;
    let prompt = "Call the probe_token tool. Then reply with only the token it returns.";
    let events = turn(agent, prompt).await?;
    let called = events.iter().any(|event| matches!(event, Event::ToolCall { name, .. } if name.ends_with("probe_token")));
    if !called {
        return Err(Failure::Model("answered without calling the MCP tool".into()));
    }
    answered_with(&events, &expected).map_err(Failure::Model)
}

/// The start of what the model answered, so a failure shows it.
fn said(events: &[Event]) -> String {
    let text: String = events
        .iter()
        .filter_map(|event| match event {
            Event::AssistantText { text } => Some(text.as_str()),
            Event::TurnFailed { message, .. } | Event::Warning { message, .. } => Some(message.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    text.chars().take(200).collect()
}

fn answered_with(events: &[Event], expected: &str) -> Result<(), String> {
    let answer: String = events
        .iter()
        .filter_map(|event| match event {
            Event::AssistantText { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !expected.is_empty() && answer.contains(expected) {
        Ok(())
    } else {
        Err("called the tool but didn't repeat its result".into())
    }
}

/// A turn that ends in an error or a timeout is a transport failure: the model never gave its answer.
async fn turn(agent: Arc<dyn Agent>, prompt: &str) -> Result<Vec<Event>, Failure> {
    let (sink, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let request = TurnRequest { files: vec![], history: Vec::new(), prompt: prompt.into(), native_session: None };
    let run = tokio::time::timeout(STEP_TIMEOUT, agent.run_turn(request, sink)).await;
    let mut events = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        events.push(event);
    }
    match run {
        Err(_) => Err(Failure::Transport("no answer in 90 seconds".into())),
        Ok(Err(error)) => Err(Failure::Transport(error.to_string())),
        Ok(Ok(_)) => Ok(events),
    }
}
