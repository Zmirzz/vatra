//! Subagents: a run of the model on one task, with its own conversation. The Agent tool and a skill that runs in a
//! fork start them. A subagent shares the rules, the approver and the processes of the turn that started it, and it
//! can't start another subagent, nor ask the person a question.

use std::sync::Arc;

use futures_util::future::join;
use tokio::sync::Semaphore;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use vatra_core::{Agent, Event, Store, TurnRequest};

use crate::engine::{Approver, EngineAgent, EngineSetup};
use crate::instructions::LoadedInstructions;
use crate::mcp::McpHub;
use crate::model::ModelDriver;
use crate::permissions::PermissionPolicy;
use crate::skills::SkillSession;
use crate::tools::{ToolContext, ToolOutput, ToolRegistry};

pub(crate) const GENERAL_PURPOSE: &str = "general-purpose";

/// The tools that no subagent gets. A subagent can't start another one, and can't ask the person.
const NEVER: [&str; 3] = ["Agent", "AskUserQuestion", "Skill"];
/// Explore reads and searches. Its list is an allowlist: no MCP tool, since a server's word that a tool only reads
/// isn't a guarantee, and no tool that changes files or runs commands.
const EXPLORE_TOOLS: [&str; 6] = ["Read", "Glob", "Grep", "WebFetch", "WebSearch", "TodoWrite"];
/// The answer of a subagent that finished without any text.
const NO_ANSWER: &str = "The subagent finished without a text answer.";

/// Which tools a subagent type may use.
enum Access {
    Everything,
    Only(&'static [&'static str]),
}

impl Access {
    fn allows(&self, name: &str) -> bool {
        match self {
            Access::Everything => true,
            Access::Only(names) => names.contains(&name),
        }
    }
}

/// The access of a subagent type, or None for a type that doesn't exist.
fn access_for(agent_type: &str) -> Option<Access> {
    match agent_type {
        GENERAL_PURPOSE => Some(Access::Everything),
        "Explore" => Some(Access::Only(&EXPLORE_TOOLS)),
        _ => None,
    }
}

/// Windows under this many tokens run two subagents at a time, and larger ones run four.
const SMALL_WINDOW: u64 = 32_000;

/// What a subagent is built from: the parts of the turn that starts it.
pub struct SubagentSetup<'a> {
    pub driver: Arc<dyn ModelDriver>,
    pub hub: Arc<McpHub>,
    /// The tools of the turn. The subagents get them, less the ones no subagent gets.
    pub tools: &'a ToolRegistry,
    pub policy: Arc<PermissionPolicy>,
    pub approver: Arc<dyn Approver>,
    pub instructions: LoadedInstructions,
    pub max_steps: usize,
    /// The skill state of the turn. The tools that a skill turned off stay off for the subagents too.
    pub session: Arc<SkillSession>,
    /// The audit log of the turn. A subagent's external actions are audited in it, as the turn's are.
    pub audit: Option<Arc<Store>>,
    /// The session the subagents' audit entries belong to.
    pub session_id: Option<i64>,
}

/// What the Agent tool, or a skill that runs in a fork, asks a subagent to do.
pub struct SubagentRequest {
    /// The task. The subagent starts with this text, not with the conversation.
    pub prompt: String,
    /// A short name for the task, shown while it runs.
    pub description: String,
    /// The type of subagent: general-purpose, or Explore.
    pub agent_type: String,
    /// The model the call asked for. Subagents run on the model of the turn, so it is reported and not used.
    pub model: Option<String>,
    /// The context of the call that starts the subagent. Its events go to the turn, and its processes end with it.
    pub context: ToolContext,
}

/// Runs subagents for the turn they belong to. Each run takes one of a few slots, so the others wait for it.
pub struct SubagentRunner {
    driver: Arc<dyn ModelDriver>,
    hub: Arc<McpHub>,
    tools: ToolRegistry,
    policy: Arc<PermissionPolicy>,
    approver: Arc<dyn Approver>,
    instructions: LoadedInstructions,
    max_steps: usize,
    session: Arc<SkillSession>,
    audit: Option<Arc<Store>>,
    session_id: Option<i64>,
    slots: Semaphore,
}

impl SubagentRunner {
    pub fn new(setup: SubagentSetup<'_>) -> SubagentRunner {
        let slots = if setup.driver.context_window().tokens < SMALL_WINDOW { 2 } else { 4 };
        SubagentRunner {
            tools: setup.tools.filtered(|name| !NEVER.contains(&name)),
            driver: setup.driver,
            hub: setup.hub,
            policy: setup.policy,
            approver: setup.approver,
            instructions: setup.instructions,
            max_steps: setup.max_steps,
            session: setup.session,
            audit: setup.audit,
            session_id: setup.session_id,
            slots: Semaphore::new(slots),
        }
    }

    /// Runs a subagent and returns its final answer. Its steps reach the turn nested under `parent_id`, the id of the
    /// call that started it. A subagent that fails is an error result, and the turn goes on.
    pub async fn run(&self, request: SubagentRequest, parent_id: &str) -> ToolOutput {
        let Some(access) = access_for(&request.agent_type) else {
            return ToolOutput::error(format!(
                "There's no subagent type `{}`. The types are general-purpose and Explore.",
                request.agent_type
            ));
        };
        let events = request.context.events.clone();
        if let Some(model) = &request.model {
            let message = format!("The model `{model}` isn't used. A subagent runs on the model of the turn.");
            // A closed sink ends the turn at the next event, so a failed send is not an error here.
            let _ = events.send(Event::Warning { source: "Agent".into(), message });
        }
        let Ok(_slot) = self.slots.acquire().await else {
            return ToolOutput::error("The subagents are stopped.");
        };
        let disallowed = self.session.disallowed();
        let registry =
            self.tools.filtered(|name| access.allows(name) && !disallowed.iter().any(|turned| turned == name));
        let (sub_events, received) = unbounded_channel();
        let agent = EngineAgent::subagent(EngineSetup {
            name: request.agent_type.clone(),
            driver: Arc::clone(&self.driver),
            hub: Arc::clone(&self.hub),
            registry,
            policy: Arc::clone(&self.policy),
            approver: Arc::clone(&self.approver),
            context: request.context,
            instructions: self.instructions.clone(),
            memory: None,
            max_steps: self.max_steps,
            extensions: None,
            audit: self.audit.clone(),
            session_id: self.session_id,
        });
        // A closed sink ends the turn at the next event, so a failed send is not an error here.
        let _ = events.send(Event::SubagentStarted { id: parent_id.into(), description: request.description });
        let turn = TurnRequest { files: vec![], history: vec![], prompt: request.prompt, native_session: None };
        let (outcome, answer) = join(agent.run_turn(turn, sub_events), forward(received, &events, parent_id)).await;
        let status = if outcome.is_ok() { "completed" } else { "failed" };
        let _ = events.send(Event::SubagentFinished { id: parent_id.into(), status: status.into() });
        match outcome {
            Ok(_) => ToolOutput::ok(answer.filter(|text| !text.trim().is_empty()).unwrap_or_else(|| NO_ANSWER.into())),
            Err(error) => ToolOutput::error(error.to_string()),
        }
    }
}

/// Sends the events of a subagent to its turn. Its steps nest under the call, and its usage goes up as the turn's
/// own, so the turn's totals count it. Returns the last text the subagent said, which is its answer.
async fn forward(
    mut received: UnboundedReceiver<Event>,
    turn: &UnboundedSender<Event>,
    parent_id: &str,
) -> Option<String> {
    let mut answer = None;
    while let Some(event) = received.recv().await {
        // A closed sink ends the subagent at its next event, as it ends the turn.
        match event {
            Event::Usage { .. } => {
                let _ = turn.send(event);
            }
            event => {
                if let Event::AssistantText { text } = &event {
                    answer = Some(text.clone());
                }
                let _ = turn.send(Event::Nested { parent_id: parent_id.into(), event: Box::new(event) });
            }
        }
    }
    answer
}
