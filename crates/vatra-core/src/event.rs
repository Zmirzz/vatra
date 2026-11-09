use serde::{Deserialize, Serialize};

/// One usage window of a subscription: `used` is a fraction from 0 to 1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimitWindow {
    pub name: String,
    pub used: f64,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HandoffBrief {
    pub summary: String,
    pub files: Vec<String>,
    pub decisions: Vec<String>,
    pub remaining: Vec<String>,
    pub degraded: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    UserMessage { text: String },
    AssistantText { text: String },
    AssistantDelta { text: String },
    ToolCall { id: String, name: String, arguments: serde_json::Value },
    ToolResult { id: String, output: String, is_error: bool },
    ToolDenied { id: String, name: String },
    TurnCompleted { agent: String },
    TurnFailed { agent: String, message: String },
    HandoffStarted { from_session: i64, to_agent: String, brief: HandoffBrief },
    HandoffResult { to_session: i64, text: String },
    /// A background command ended. Its text reaches the model on the next step, and history replays it.
    BackgroundResult { id: String, text: String },
    /// A path-scoped rule joined the context because a file it covers was touched. History replays it.
    RuleAdded { label: String, text: String },
    /// Deferred MCP tools that ToolSearch found, so they join the tool list from the next step. History replays it.
    ToolsLoaded { names: Vec<String> },
    /// A skill or command the person ran, with its instructions as the model got them. History replays it, and
    /// compaction keeps it.
    SkillLoaded { name: String, text: String },
    Unmapped { source: String, raw: String },
    NativeSession { id: String },
    Warning { source: String, message: String },
    Nested { parent_id: String, event: Box<Event> },
    SubagentStarted { id: String, description: String },
    SubagentFinished { id: String, status: String },
    Hook { name: String, phase: String, output: Option<String> },
    Usage { input_tokens: u64, output_tokens: u64, cost_usd: Option<f64> },
    RateLimit {
        status: String,
        resets_at: Option<i64>,
        /// How much of each window (five hours, a week) is used, when the agent says.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        windows: Vec<LimitWindow>,
    },
    Catalog { skills: u32, agents: u32, plugins: u32, mcp_servers: u32 },
    /// Stands in for the history before it. Its position in the journal marks the cut-off.
    Compacted { summary: String },
}

impl Event {
    /// Shown while the turn runs but never journaled, so it has no seq and can't be replayed.
    pub fn is_transient(&self) -> bool {
        matches!(self, Event::AssistantDelta { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredEvent {
    pub seq: i64,
    pub session_id: i64,
    pub event: Event,
}
