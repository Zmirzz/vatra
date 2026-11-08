use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures_util::future::join_all;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::UnboundedSender;
use vatra_core::{Agent, AgentError, AuditEntry, Event, ProcessGroup, Store, StoreError, TurnOutcome, TurnRequest};
use vatra_extensions::Skill;

use crate::compaction::{
    compact, compact_threshold, estimate_tokens, fit_result, regrowth_tokens, summary_message, trim_oldest_results,
};
use crate::instructions::{LoadedInstructions, covers};
use crate::mcp::{McpHub, ToolDef};
use crate::memory::memory_block;
use crate::narration::{MAX_NUDGES, NUDGE, PLAN_NUDGE, announces_unperformed_action, requests_plan};
use crate::model::{Completion, ContextWindow, ImagePart, Message, ModelDriver, ModelError, ModelRequest, RawToolCall};
use crate::permissions::{Decision, EXTERNAL_LIMIT_PER_HOUR, PermissionPolicy, ToolClass};
use crate::prompt::{COMPACT_TOOLS, PromptInput, SMALL_WINDOW_TOKENS, system_prompt};
use crate::skills::{EngineExtensions, invoke, listing_of, resolve_prompt, visible};
use crate::tools::{ExternalAction, TOOL_SEARCH, Tool, ToolContext, ToolOutput, ToolRegistry, deferred_split};

const DENIED_MESSAGE: &str = "The user refused this call.";
const INTERRUPTED_MESSAGE: &str = "This call was interrupted and has no result.";
/// The window in which the external actions of a connector are counted against the limit.
const EXTERNAL_WINDOW_SECS: i64 = 3600;
/// A call that fails the same way this many times in a row stops the turn.
const REPEAT_LIMIT: usize = 8;

/// The MCP tools that a turn keeps out of the request until ToolSearch loads them.
#[derive(Default)]
struct Deferred {
    /// Every deferred tool of the turn, loaded or not.
    names: HashSet<String>,
    /// The deferred tools that are loaded, in the order they were loaded. They come after the other tools.
    loaded: Vec<String>,
}

impl Deferred {
    /// A deferred tool that is not loaded yet. A call to it is refused.
    fn pending(&self, name: &str) -> bool {
        self.names.contains(name) && !self.loaded.iter().any(|loaded| loaded == name)
    }
}

#[async_trait]
pub trait Approver: Send + Sync {
    async fn approve(&self, name: &str, arguments: &Value) -> bool;

    /// False when the answer doesn't come from a person, as with an approver that says yes to everything. An
    /// action with an effect outside the computer is refused without running when the approver isn't interactive.
    fn interactive(&self) -> bool {
        true
    }
}

pub struct AllowAll;

#[async_trait]
impl Approver for AllowAll {
    async fn approve(&self, _name: &str, _arguments: &Value) -> bool {
        true
    }

    fn interactive(&self) -> bool {
        false
    }
}

pub struct DenyAll;

#[async_trait]
impl Approver for DenyAll {
    async fn approve(&self, _name: &str, _arguments: &Value) -> bool {
        false
    }
}

/// What an engine agent is built from.
pub struct EngineSetup {
    pub name: String,
    pub driver: Arc<dyn ModelDriver>,
    /// The MCP servers behind the MCP tools in the registry. Their failures are reported at each turn.
    pub hub: Arc<McpHub>,
    pub registry: ToolRegistry,
    /// Shared with the subagents of the turn, which follow the same rules.
    pub policy: Arc<PermissionPolicy>,
    pub approver: Arc<dyn Approver>,
    /// The parts of the tool context that last as long as the session. Each turn sets its own event sink
    /// and process tree on top of them.
    pub context: ToolContext,
    pub instructions: LoadedInstructions,
    /// The memory folder. Its index goes into the prompt at the start of each turn.
    pub memory: Option<PathBuf>,
    pub max_steps: usize,
    /// The skills and commands of the turn, when the session has any. The Skill tool in the registry reads them.
    pub extensions: Option<Arc<EngineExtensions>>,
    /// Where the external actions are audited. Without it they are still asked about every time, but nothing is
    /// recorded and the hourly limit is not kept.
    pub audit: Option<Arc<Store>>,
    /// The session the audit entries belong to.
    pub session_id: Option<i64>,
}

pub struct EngineAgent {
    name: String,
    driver: Arc<dyn ModelDriver>,
    hub: Arc<McpHub>,
    approver: Arc<dyn Approver>,
    max_steps: usize,
    tools: ToolRegistry,
    /// Shared with the skills of the turn, which grant and turn off tools through it.
    policy: Arc<PermissionPolicy>,
    context: ToolContext,
    instructions: LoadedInstructions,
    memory: Option<PathBuf>,
    extensions: Option<Arc<EngineExtensions>>,
    /// True for a subagent. It keeps the turn's grants and processes, and its context is the one of its call.
    subagent: bool,
    audit: Option<Arc<Store>>,
    session_id: Option<i64>,
    /// The effort a person picked for the turn. It wins over the effort a skill asks for.
    effort_override: Option<String>,
}

impl EngineAgent {
    /// Sets the effort the person picked for the turns of this agent.
    pub fn with_effort(mut self, effort: Option<String>) -> EngineAgent {
        self.effort_override = effort;
        self
    }

    pub fn new(setup: EngineSetup) -> EngineAgent {
        if let Some(extensions) = &setup.extensions {
            extensions.bind(Arc::clone(&setup.policy), Arc::clone(&setup.approver));
        }
        EngineAgent {
            name: setup.name,
            driver: setup.driver,
            hub: setup.hub,
            approver: setup.approver,
            max_steps: setup.max_steps,
            tools: setup.registry,
            policy: setup.policy,
            context: setup.context,
            instructions: setup.instructions,
            memory: setup.memory,
            extensions: setup.extensions,
            subagent: false,
            audit: setup.audit,
            session_id: setup.session_id,
            effort_override: None,
        }
    }

    /// The agent of a subagent. It runs under the rules, the approver and the process tree of the turn that started
    /// it, so the turn's grants and processes stay as they are while it runs.
    pub(crate) fn subagent(setup: EngineSetup) -> EngineAgent {
        EngineAgent { subagent: true, ..EngineAgent::new(setup) }
    }

    /// The tools that the skills of this turn turned off, by name.
    fn disallowed(&self) -> Vec<String> {
        self.extensions.as_ref().map(|extensions| extensions.session.disallowed()).unwrap_or_default()
    }

    /// The effort the last skill of this turn asked for.
    fn effort(&self) -> Option<String> {
        if let Some(effort) = &self.effort_override {
            return Some(effort.clone());
        }
        self.extensions.as_ref().and_then(|extensions| extensions.session.effort())
    }

    /// The instructions of the skills run in the session, which a summary keeps after it.
    fn attachments(&self) -> Vec<String> {
        self.extensions.as_ref().map(|extensions| extensions.session.invoked_texts()).unwrap_or_default()
    }

    /// The listing of the skills the model is shown, given the files touched so far. None when there is none.
    fn listing(&self, touched: &[PathBuf]) -> Option<String> {
        let extensions = self.extensions.as_ref()?;
        let skills: Vec<&Skill> = visible(&extensions.skills, &self.context.project_root, touched);
        let text = listing_of(skills.into_iter(), self.driver.context_window(), &extensions.usage);
        (!text.is_empty()).then_some(text)
    }

    /// The files that the successful Read, Write and Edit calls of a history touched.
    fn history_touched(&self, history: &[Event]) -> Vec<PathBuf> {
        let succeeded: HashSet<&str> = history
            .iter()
            .filter_map(|event| match event {
                Event::ToolResult { id, is_error: false, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        history
            .iter()
            .filter_map(|event| match event {
                Event::ToolCall { id, name, arguments }
                    if matches!(name.as_str(), "Read" | "Write" | "Edit") && succeeded.contains(id.as_str()) =>
                {
                    arguments["file_path"].as_str().map(|path| self.absolute(path))
                }
                _ => None,
            })
            .collect()
    }

    fn absolute(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() { path.to_path_buf() } else { self.context.cwd.join(path) }
    }

    /// The system prompt for a step, with the listing that step shows.
    fn system(&self, date: &str, memory: Option<&str>, listing: Option<&str>, deferred_tools: bool) -> String {
        system_prompt(&PromptInput {
            cwd: &self.context.cwd,
            platform: std::env::consts::OS,
            date,
            instructions: &self.instructions.text,
            memory,
            skills_listing: listing,
            is_subagent: self.subagent,
            deferred_tools,
            compact: self.driver.context_window().tokens <= SMALL_WINDOW_TOKENS,
        })
    }

    /// Runs one call whose arguments were parsed. The result comes back with the event that journals it, and
    /// the caller sends the events in call order.
    async fn execute(
        &self,
        call: &RawToolCall,
        arguments: Result<Value, String>,
        context: &ToolContext,
        deferred: &Deferred,
    ) -> (Message, Event) {
        let arguments = match arguments {
            Ok(arguments) => arguments,
            Err(message) => return failure(call, message),
        };
        let Some(tool) = self.tools.get(&call.name) else {
            return failure(call, format!("unknown tool: {}", call.name));
        };
        if deferred.pending(&call.name) {
            return failure(
                call,
                format!("The tool `{}` isn't in your tool list yet. Find it with ToolSearch first.", call.name),
            );
        }
        if self.disallowed().contains(&call.name) {
            return failure(call, format!("The tool `{}` is turned off by the skill that is running.", call.name));
        }
        let class = tool.class(&arguments, context);
        let decision = self.policy.decide(&call.name, class, &arguments, context);
        if class == ToolClass::External {
            // An external action runs only when the person approves it here. Only a deny rule stops it earlier.
            let action = external_action(tool.as_ref(), &call.name, &arguments);
            match decision {
                Decision::Deny(message) => {
                    // Nothing runs on a refusal, so a failed write doesn't change the answer.
                    let _ = self.record_audit(call, &action, &arguments, "denied");
                    return failure(call, message);
                }
                Decision::Allow | Decision::Ask => match self.external_gate(call, &arguments, &action).await {
                    Gate::Run => {}
                    Gate::Denied => return denied(call),
                    Gate::Refused(message) => return failure(call, message),
                },
            }
        } else {
            match decision {
                Decision::Allow => {}
                Decision::Deny(message) => return failure(call, message),
                Decision::Ask => {
                    if !self.approver.approve(&call.name, &arguments).await {
                        return denied(call);
                    }
                }
            }
        }
        let mut paths=Vec::new();
        if matches!(call.name.as_str(),"Write"|"Edit") {
            if let Some(path)=arguments["file_path"].as_str(){let path=Path::new(path);paths.push(if path.is_absolute(){path.to_path_buf()}else{context.cwd.join(path)});}
        }else if call.name=="apply_patch"
            && let Some(patch)=arguments["patch"].as_str().or(arguments["input"].as_str()){
                for line in patch.lines(){for prefix in ["*** Add File: ","*** Update File: ","*** Move to: "]{if let Some(path)=line.strip_prefix(prefix){let path=Path::new(path);paths.push(if path.is_absolute(){path.to_path_buf()}else{context.cwd.join(path)});}}}
        }
        let before:Vec<_>=paths.iter().map(|p|crate::artifacts::read_snapshot(p)).collect();
        let output = tool.call_with_id(&call.id, arguments, context).await;
        if !output.is_error && let (Some(store),Some(session))=(&self.audit,self.session_id){
            for (path,before) in paths.iter().zip(&before){if let Some(after)=crate::artifacts::read_snapshot(path)
                && let Err(message)=crate::artifacts::capture(store,session,path,before.as_deref(),&after,&call.id){let _=context.events.send(Event::Warning{source:"files".into(),message});}}
        }
        let is_error = output.is_error;
        let (text, images) = self.deliver(call, output, context);
        let event = Event::ToolResult { id: call.id.clone(), output: text.clone(), is_error };
        let message = Message::Tool { id: call.id.clone(), content: text, is_error, images };
        (message, event)
    }

    /// The question for an external action. The hourly limit is checked first, without asking. An approved call is
    /// audited before it runs, and a call that can't be audited doesn't run. Without an audit store the person is
    /// still asked every time.
    async fn external_gate(&self, call: &RawToolCall, arguments: &Value, action: &ExternalAction) -> Gate {
        if !self.approver.interactive() {
            // Nobody can be asked, so the call is refused, and a failed write only loses the entry.
            let _ = self.record_audit(call, action, arguments, "blocked");
            return Gate::Refused("This action needs the person's approval, which isn't available in this mode.".into());
        }
        let Some(audit) = &self.audit else {
            return if self.approver.approve(&call.name, arguments).await { Gate::Run } else { Gate::Denied };
        };
        match audit.external_count_since(&action.connector, now_secs() - EXTERNAL_WINDOW_SECS) {
            Ok(count) if count >= EXTERNAL_LIMIT_PER_HOUR => {
                // The call is refused either way, so a failed write only loses the entry.
                let _ = self.record_audit(call, action, arguments, "blocked");
                return Gate::Refused(format!(
                    "Too many actions on {} in the last hour. Try again later.",
                    action.connector
                ));
            }
            Ok(_) => {}
            Err(error) => {
                return Gate::Refused(format!("The action wasn't run, because the audit log couldn't be read: {error}."));
            }
        }
        if self.approver.approve(&call.name, arguments).await {
            let at = now_secs();
            let entry = AuditEntry {
                at, session_id:self.session_id, tool:call.name.clone(), connector:action.connector.clone(),
                summary:action.summary.clone(), decision:"approved".into(), payload_hash:payload_hash(arguments),
            };
            match audit.try_record_external_approval(&entry, at - EXTERNAL_WINDOW_SECS, EXTERNAL_LIMIT_PER_HOUR) {
                Ok(true) => Gate::Run,
                Ok(false) => {
                    let _ = self.record_audit(call, action, arguments, "blocked");
                    Gate::Refused(format!("Too many actions on {} in the last hour. Try again later.",action.connector))
                }
                Err(error) => Gate::Refused(format!(
                    "The action wasn't run, because it couldn't be written to the audit log: {error}."
                )),
            }
        } else {
            // A refused action doesn't run, so a failed write doesn't change the answer.
            let _ = self.record_audit(call, action, arguments, "denied");
            Gate::Denied
        }
    }

    /// Writes an audit entry for a call of an external action. Nothing is written without an audit store.
    fn record_audit(
        &self,
        call: &RawToolCall,
        action: &ExternalAction,
        arguments: &Value,
        decision: &str,
    ) -> Result<(), StoreError> {
        let Some(audit) = &self.audit else {
            return Ok(());
        };
        audit.record_audit(&AuditEntry {
            at: now_secs(),
            session_id: self.session_id,
            tool: call.name.clone(),
            connector: action.connector.clone(),
            summary: action.summary.clone(),
            decision: decision.into(),
            payload_hash: payload_hash(arguments),
        })
    }

    /// The output of a call as the model gets it. Images go to a file when the model can't see them. A text is
    /// cut to fit the window.
    fn deliver(&self, call: &RawToolCall, output: ToolOutput, context: &ToolContext) -> (String, Vec<ImagePart>) {
        let mut text = output.text;
        let mut images = output.images;
        if !self.driver.supports_images() && !images.is_empty() {
            let notes: Vec<String> = images
                .iter()
                .enumerate()
                .map(|(index, image)| saved_image(image, &call.id, index, &context.session_dir))
                .collect();
            text = std::iter::once(text).chain(notes).filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n");
            images.clear();
        }
        (fit_result(text, self.driver.context_window().tokens), images)
    }

    /// Runs the calls of one answer and returns their results in the order of the calls. Consecutive calls to
    /// parallel-safe tools run together. Every other call waits for the calls before it.
    async fn run_calls(
        &self,
        calls: &[RawToolCall],
        parsed: &[Result<Value, String>],
        sink: &UnboundedSender<Event>,
        context: &ToolContext,
        deferred: &Deferred,
    ) -> Result<Vec<Message>, AgentError> {
        let mut messages = Vec::with_capacity(calls.len());
        let mut start = 0;
        while start < calls.len() {
            let end = if self.is_parallel(&calls[start].name) {
                let mut end = start + 1;
                while end < calls.len() && self.is_parallel(&calls[end].name) {
                    end += 1;
                }
                end
            } else {
                start + 1
            };
            // Every call of the batch is journaled before any result, in call order, as the model sent them.
            for (call, arguments) in calls[start..end].iter().zip(&parsed[start..end]) {
                let arguments = shown(call, arguments);
                emit(sink, Event::ToolCall { id: call.id.clone(), name: call.name.clone(), arguments })?;
            }
            let batch = calls[start..end]
                .iter()
                .zip(&parsed[start..end])
                .map(|(call, arguments)| self.execute(call, arguments.clone(), context, deferred));
            for (message, event) in join_all(batch).await {
                emit(sink, event)?;
                messages.push(message);
            }
            start = end;
        }
        Ok(messages)
    }

    fn is_parallel(&self, name: &str) -> bool {
        self.tools.get(name).is_some_and(|tool| tool.parallel_safe())
    }

    /// The MCP tools that wait for ToolSearch in this turn. Nothing waits when the list is small, when the
    /// registry has no ToolSearch to load them, or for a server that is always loaded.
    fn deferred_tools(&self, window: ContextWindow, history: &[Event]) -> Deferred {
        if self.tools.get(TOOL_SEARCH).is_none() {
            return Deferred::default();
        }
        let (_, deferred) = deferred_split(self.hub.tools(), window);
        let names: HashSet<String> =
            deferred.into_iter().map(|tool| tool.name).filter(|name| !self.hub.always_load(name)).collect();
        let loaded = loaded_tools(history).into_iter().filter(|name| names.contains(name)).collect();
        Deferred { names, loaded }
    }

    /// The tools that the model is sent: the registry's tools that are not deferred, then the loaded ones.
    fn visible_tools(&self, deferred: &Deferred) -> Vec<ToolDef> {
        let disallowed = self.disallowed();
        // A small window keeps the core tools: every schema costs part of a window that is already short.
        let small = self.driver.context_window().tokens <= SMALL_WINDOW_TOKENS;
        let mut tools: Vec<ToolDef> = self
            .tools
            .defs()
            .into_iter()
            .filter(|def| !small || def.name == TOOL_SEARCH || COMPACT_TOOLS.contains(&def.name.as_str()) || def.name.contains("workspace") || self.hub.tools().iter().any(|t| t.name == def.name))
            .filter(|def| !deferred.names.contains(&def.name) && !disallowed.contains(&def.name))
            // ToolSearch is listed only when there is something to search for.
            .filter(|def| def.name != TOOL_SEARCH || !deferred.names.is_empty())
            .collect();
        tools.extend(
            deferred
                .loaded
                .iter()
                .filter(|name| !disallowed.contains(name))
                .filter_map(|name| self.tools.get(name))
                .map(|tool| tool.def()),
        );
        tools
    }

    /// The deferred tools that the successful calls of this step load, and that are not loaded yet.
    fn found_tools(
        &self,
        calls: &[RawToolCall],
        parsed: &[Result<Value, String>],
        results: &[Message],
        deferred: &Deferred,
    ) -> Vec<String> {
        let mut found: Vec<String> = vec![];
        for ((call, arguments), result) in calls.iter().zip(parsed).zip(results) {
            let succeeded = matches!(result, Message::Tool { is_error: false, .. });
            let (Some(tool), Ok(arguments), true) = (self.tools.get(&call.name), arguments, succeeded) else {
                continue;
            };
            for name in tool.loads(arguments) {
                if deferred.pending(&name) && !found.contains(&name) {
                    found.push(name);
                }
            }
        }
        found
    }

    /// The path rules that the files of this step bring in for the first time. Each one is journaled as it
    /// joins, so a replay brings it back at the same place.
    fn activate_rules(
        &self,
        touched: &[PathBuf],
        active: &mut HashSet<String>,
        sink: &UnboundedSender<Event>,
    ) -> Result<Vec<Message>, AgentError> {
        let mut messages = vec![];
        for rule in &self.instructions.deferred {
            let covered = touched.iter().any(|file| covers(rule, &self.context.project_root, file));
            if !covered || active.contains(&rule.label) {
                continue;
            }
            active.insert(rule.label.clone());
            emit(sink, Event::RuleAdded { label: rule.label.clone(), text: rule.text.clone() })?;
            messages.push(Message::User { text: rule.text.clone(), images: vec![] });
        }
        Ok(messages)
    }

    /// Asks the model for the next step. A server that refuses tools gets the same request again without them.
    /// The rest of the turn then runs without tools, and the user sees a warning.
    async fn complete(
        &self,
        messages: &[Message],
        tools: &mut Vec<ToolDef>,
        sink: &UnboundedSender<Event>,
    ) -> Result<Completion, ModelError> {
        match self.stream(messages, tools, sink).await {
            Err(ModelError::ToolsRejected(message)) if !tools.is_empty() => {
                let message = format!("The model doesn't take tools, so this turn runs without them. {message}");
                // A closed sink ends the turn at the next event, so a failed send is not an error here.
                let _ = sink.send(Event::Warning { source: self.name.clone(), message });
                tools.clear();
                self.stream(messages, tools, sink).await
            }
            result => result,
        }
    }

    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        sink: &UnboundedSender<Event>,
    ) -> Result<Completion, ModelError> {
        let request = ModelRequest { messages: messages.to_vec(), tools: tools.to_vec(), effort: self.effort() };
        self.driver
            .stream(&request, &mut |fragment: &str| {
                // A closed sink ends the turn at the next event, so a failed send is not an error here.
                let _ = sink.send(Event::AssistantDelta { text: fragment.to_string() });
            })
            .await
    }

    /// Makes the conversation smaller. It is summarized, and when the summary fails, the oldest tool results are
    /// cut instead and the user is warned.
    async fn shrink(
        &self,
        messages: &mut Vec<Message>,
        prompt: &Message,
        active: &HashSet<String>,
        target: u64,
        sink: &UnboundedSender<Event>,
    ) -> Result<Shrunk, AgentError> {
        match compact(self.driver.as_ref(), messages, &self.attachments()).await {
            Ok(summary) => {
                let mut rebuilt = vec![messages[0].clone()];
                rebuilt.extend(self.journal_summary(&summary, active, sink)?);
                rebuilt.push(prompt.clone());
                *messages = rebuilt;
                Ok(Shrunk::Summarized)
            }
            Err(error) => {
                let message = format!(
                    "The conversation couldn't be summarized ({error}), so the oldest tool results were cut instead."
                );
                emit(sink, Event::Warning { source: self.name.clone(), message })?;
                Ok(Shrunk::Cut(trim_oldest_results(messages, target)))
            }
        }
    }

    /// Journals a summary and the rules that are active, so that the replay brings the rules back after it.
    /// Returns the messages that the summary and those rules stand for in the context.
    fn journal_summary(
        &self,
        summary: &str,
        active: &HashSet<String>,
        sink: &UnboundedSender<Event>,
    ) -> Result<Vec<Message>, AgentError> {
        emit(sink, Event::Compacted { summary: summary.into() })?;
        let mut messages = vec![summary_message(summary)];
        for rule in &self.instructions.deferred {
            if active.contains(&rule.label) {
                emit(sink, Event::RuleAdded { label: rule.label.clone(), text: rule.text.clone() })?;
                messages.push(Message::User { text: rule.text.clone(), images: vec![] });
            }
        }
        Ok(messages)
    }
}

#[async_trait]
impl Agent for EngineAgent {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_turn(&self, request: TurnRequest, sink: UnboundedSender<Event>) -> Result<TurnOutcome, AgentError> {
        self.hub.ensure_connected().await;
        // A subagent's warnings are the turn's, which already showed them.
        if !self.subagent {
            for (server, message) in self.hub.failures() {
                let message = format!("MCP server `{server}`: {message}");
                emit(&sink, Event::Warning { source: self.name.clone(), message })?;
            }
            if let Some(extensions) = &self.extensions {
                for problem in &extensions.problems {
                    emit(&sink, Event::Warning { source: self.name.clone(), message: problem.clone() })?;
                }
                extensions.session.start_turn(&request.history);
            }
            // Grants and turned-off tools of an earlier turn don't carry over.
            self.policy.clear_turn_grants();
        }
        let context_window = self.driver.context_window();
        let mut deferred = self.deferred_tools(context_window, &request.history);
        let deferred_tools = !deferred.names.is_empty();
        let memory = self.memory.as_deref().and_then(memory_block);
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        // The files the history touched decide which path-scoped skills the model is shown.
        let mut seen = self.history_touched(&request.history);
        let mut listing = self.listing(&seen);
        let system = self.system(&date, memory.as_deref(), listing.as_deref(), deferred_tools);
        // Rules that already joined the context in an earlier turn are in the history, so they don't join again.
        let mut active: HashSet<String> = request
            .history
            .iter()
            .filter_map(|event| match event {
                Event::RuleAdded { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect();
        let mut messages = vec![Message::System { text: system }];
        messages.extend(history_messages(&request.history,&request.files));
        if request.prompt.trim() == "/compact" {
            let summary = compact(self.driver.as_ref(), &messages, &self.attachments())
                .await
                .map_err(|error| AgentError::Failed(format!("The conversation couldn't be compacted: {error}")))?;
            self.journal_summary(&summary, &active, &sink)?;
            emit(&sink, Event::AssistantText { text: "The conversation was compacted.".into() })?;
            return Ok(TurnOutcome::default());
        }
        let window = context_window.tokens;
        let mut tools = self.visible_tools(&deferred);
        let mut tools_tokens = estimate_tokens(&[], &tools);
        if tools_tokens > window {
            return Err(AgentError::Failed(format!(
                "The tools take about {tools_tokens} tokens, more than the model's context window of {window} tokens. \
                 Turn off some tools or MCP servers, or use a model with a bigger window."
            )));
        }
        let current_files:Vec<_>=request.files.iter().filter(|f|f.history_index.is_none()).collect();
        let document_tokens=current_files.iter().map(|f|f.text.len().div_ceil(3) as u64).sum::<u64>();
        let document_budget=16_000.min(window.saturating_sub(tools_tokens+4096)/4);
        if document_tokens>document_budget {
            return Err(AgentError::Failed(format!("Attached documents need about {document_tokens} tokens; the available attachment budget is {document_budget}. Select a smaller section.")));
        }
        let images=current_files.iter().filter_map(|f|f.image_base64.as_ref().map(|b|ImagePart{media_type:f.media_type.clone(),base64:b.clone()})).collect();
        let current_input=Message::User { text: request.input_text(), images };
        messages.push(current_input.clone());
        // Foreground commands of this turn end with it, including when the turn is stopped.
        // A subagent ends with the turn that started it, so it uses that turn's processes.
        let tree = if self.subagent {
            Arc::clone(&self.context.tree)
        } else {
            Arc::new(
                ProcessGroup::new()
                    .map_err(|error| AgentError::Failed(format!("couldn't set up processes: {error}")))?,
            )
        };
        let context = ToolContext { events: sink.clone(), tree, ..self.context.clone() };
        // The skill the prompt names runs now. Its instructions join the conversation after the prompt, and are
        // journaled, so a replay gives the model the same text.
        if let Some(extensions) = &self.extensions {
            let resolved = resolve_prompt(&request.prompt, &extensions.skills);
            let invocations = resolved
                .command
                .into_iter()
                .map(|invocation| (invocation.skill, invocation.arguments))
                .chain(resolved.mentions.into_iter().map(|skill| (skill, String::new())));
            for (skill, arguments) in invocations {
                let text = invoke(extensions, &skill, &arguments, &context, true, "")
                    .await
                    .map_err(AgentError::Failed)?;
                emit(&sink, Event::SkillLoaded { name: skill.qualified_name(), text: text.clone() })?;
                messages.push(Message::User { text, images: vec![] });
            }
            // The skills may have turned tools off, which applies from the first step.
            tools = self.visible_tools(&deferred);
            tools_tokens = estimate_tokens(&[], &tools);
        }

        let mut repeats = Repeats::default();
        // The input tokens the model reported, and how many messages there were when it reported them.
        let mut measured: Option<(u64, usize)> = None;
        // The context size below which a proactive compaction waits: after one, it must grow again first.
        let mut next_compaction: u64 = 0;
        // The nudges since the last step that ran tool calls. A model that keeps announcing without acting is stopped.
        let mut nudges = 0;
        let needs_plan = requests_plan(&request.prompt);
        let mut planned = false;
        let mut plan_reminders = 0;
        for _ in 0..self.max_steps {
            // Results of background commands that ended since the last step are journaled, and go to the model now.
            for finished in context.background.take_finished() {
                emit(&sink, Event::BackgroundResult { id: finished.id, text: finished.text.clone() })?;
                messages.push(Message::User { text: finished.text, images: vec![] });
            }
            let used = match measured {
                Some((input, from)) => input + estimate_tokens(&messages[from.min(messages.len())..], &[]),
                None => estimate_tokens(&messages, &tools),
            };
            if used >= compact_threshold(window) && used >= next_compaction {
                let target = compact_threshold(window).saturating_sub(tools_tokens);
                self.shrink(&mut messages, &current_input, &active, target, &sink).await?;
                next_compaction = estimate_tokens(&messages, &tools) + regrowth_tokens(window);
            }
            let completion = match self.complete(&messages, &mut tools, &sink).await {
                Ok(completion) => completion,
                Err(ModelError::ContextTooLarge(_)) => {
                    // The model refused the request for its size. The conversation is shrunk once, and sent again.
                    let target = window.saturating_sub(tools_tokens);
                    let shrunk = self.shrink(&mut messages, &current_input, &active, target, &sink).await?;
                    if let Shrunk::Cut(false) = shrunk {
                        return Err(cut_did_not_fit());
                    }
                    self.complete(&messages, &mut tools, &sink).await.map_err(|error| match error {
                        ModelError::ContextTooLarge(_) => too_large_after_shrink(),
                        other => other.into(),
                    })?
                }
                Err(error) => return Err(error.into()),
            };
            let usage = completion.usage;
            emit(&sink, Event::Usage { input_tokens: usage.input, output_tokens: usage.output, cost_usd: None })?;
            measured = (usage.input > 0).then_some((usage.input, messages.len()));
            if let Some(text) = &completion.text {
                emit(&sink, Event::AssistantText { text: text.clone() })?;
            }
            let plan_pending = needs_plan && !planned && plan_reminders < MAX_NUDGES
                && tools.iter().any(|tool| tool.name == "TodoWrite");
            if completion.tool_calls.is_empty() {
                let question = completion.text.as_deref().is_some_and(|text| text.trim().ends_with('?'));
                if plan_pending && !question {
                    plan_reminders += 1;
                    messages.push(assistant_message(completion.text, &completion.tool_calls, &[], completion.opaque));
                    messages.push(Message::User { text: PLAN_NUDGE.into(), images: vec![] });
                    continue;
                }
                let announced = completion.text.as_deref().is_some_and(announces_unperformed_action);
                if !announced || nudges >= MAX_NUDGES {
                    return Ok(TurnOutcome::default());
                }
                // The nudge reaches the model but is not journaled: the answer above is what the person sees.
                nudges += 1;
                messages.push(assistant_message(completion.text, &completion.tool_calls, &[], completion.opaque));
                messages.push(Message::User { text: NUDGE.into(), images: vec![] });
                continue;
            }

            nudges = 0;
            let parsed: Vec<_> = completion.tool_calls.iter().map(parse_arguments).collect();
            messages.push(assistant_message(completion.text, &completion.tool_calls, &parsed, completion.opaque));
            let results = if plan_pending && completion.tool_calls.iter().any(|call| call.name != "TodoWrite") {
                plan_reminders += 1;
                let mut rejected = Vec::new();
                for (call, arguments) in completion.tool_calls.iter().zip(&parsed) {
                    emit(&sink, Event::ToolCall { id:call.id.clone(), name:call.name.clone(), arguments:shown(call, arguments) })?;
                    let (message, event) = failure(call, PLAN_NUDGE.into());
                    emit(&sink, event)?;
                    rejected.push(message);
                }
                rejected
            } else {
                self.run_calls(&completion.tool_calls, &parsed, &sink, &context, &deferred).await?
            };
            planned |= completion.tool_calls.iter().zip(&results).any(|(call, result)|
                call.name == "TodoWrite" && matches!(result, Message::Tool { is_error:false, .. }));
            for (call, result) in completion.tool_calls.iter().zip(&results) {
                repeats.record(call, result)?;
            }
            let found = self.found_tools(&completion.tool_calls, &parsed, &results, &deferred);
            if !found.is_empty() {
                emit(&sink, Event::ToolsLoaded { names: found.clone() })?;
                deferred.loaded.extend(found);
            }
            // A skill that ran in this step may have turned tools off, so the list is read again each step.
            tools = self.visible_tools(&deferred);
            tools_tokens = estimate_tokens(&[], &tools);
            let touched = touched_files(&completion.tool_calls, &parsed, &results, &self.context.cwd);
            messages.extend(results);
            messages.extend(self.activate_rules(&touched, &mut active, &sink)?);
            seen.extend(touched);
            // A file touched in this step shows its path-scoped skills from the next step.
            let next = self.listing(&seen);
            if next != listing {
                listing = next;
                messages[0] = Message::System {
                    text: self.system(&date, memory.as_deref(), listing.as_deref(), deferred_tools),
                };
            }
        }
        Err(AgentError::Failed(format!(
            "The turn stopped after {} steps, the limit set by max_steps in [engine]. Ask for a smaller piece of \
             work, or raise the limit.",
            self.max_steps
        )))
    }
}

/// What `shrink` did: a summary replaced the conversation, or the oldest tool results were cut and the flag says
/// whether that brought the conversation under the target.
enum Shrunk {
    Summarized,
    Cut(bool),
}

fn too_large_after_shrink() -> AgentError {
    AgentError::Failed(
        "The conversation is too large for the model's context window, even after it was compacted. Start a new \
         session, or ask for a smaller piece of work."
            .into(),
    )
}

fn cut_did_not_fit() -> AgentError {
    AgentError::Failed(
        "The conversation couldn't be summarized, and cutting the oldest tool results didn't make it fit the model's \
         context window. Start a new session, or ask for a smaller piece of work."
            .into(),
    )
}

/// The failed call the turn saw last, and how many times in a row it failed the same way.
#[derive(Default)]
struct Repeats {
    last: Option<(String, String, String)>,
    count: usize,
}

impl Repeats {
    fn record(&mut self, call: &RawToolCall, result: &Message) -> Result<(), AgentError> {
        let Message::Tool { content, is_error: true, .. } = result else {
            self.last = None;
            self.count = 0;
            return Ok(());
        };
        let key = (call.name.clone(), call.arguments.clone(), content.clone());
        self.count = if self.last.as_ref() == Some(&key) { self.count + 1 } else { 1 };
        self.last = Some(key);
        if self.count >= REPEAT_LIMIT {
            return Err(AgentError::Failed(format!(
                "The model made the same call {REPEAT_LIMIT} times in a row and got the same error each time, so \
                 the turn was stopped. The error: {content}"
            )));
        }
        Ok(())
    }
}

/// The files that the successful Read, Write and Edit calls of one step named, as paths.
fn touched_files(
    calls: &[RawToolCall],
    parsed: &[Result<Value, String>],
    results: &[Message],
    cwd: &Path,
) -> Vec<PathBuf> {
    let mut files = vec![];
    for ((call, arguments), result) in calls.iter().zip(parsed).zip(results) {
        let succeeded = matches!(result, Message::Tool { is_error: false, .. });
        if !succeeded || !matches!(call.name.as_str(), "Read" | "Write" | "Edit") {
            continue;
        }
        let Some(path) = arguments.as_ref().ok().and_then(|arguments| arguments["file_path"].as_str()) else {
            continue;
        };
        let path = Path::new(path);
        files.push(if path.is_absolute() { path.to_path_buf() } else { cwd.join(path) });
    }
    files
}

fn emit(sink: &UnboundedSender<Event>, event: Event) -> Result<(), AgentError> {
    sink.send(event).map_err(|_| AgentError::Failed("the turn was stopped".into()))
}

/// The arguments a call is journaled with. A call whose arguments didn't parse keeps them as the raw text.
fn shown(call: &RawToolCall, arguments: &Result<Value, String>) -> Value {
    arguments.clone().unwrap_or_else(|_| Value::String(call.arguments.clone()))
}

/// What the person decided about an external action.
enum Gate {
    Run,
    Denied,
    Refused(String),
}

/// The action a call stands for. A tool that says nothing about its action is listed under `unknown`.
fn external_action(tool: &dyn Tool, name: &str, arguments: &Value) -> ExternalAction {
    tool.external(arguments).unwrap_or_else(|| ExternalAction { connector: "unknown".into(), summary: name.into() })
}

/// The SHA-256 of the arguments as JSON, in lowercase hex. The audit log keeps this instead of the arguments.
fn payload_hash(arguments: &Value) -> String {
    format!("{:x}", Sha256::digest(arguments.to_string()))
}

fn now_secs() -> i64 {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs());
    i64::try_from(elapsed).unwrap_or(i64::MAX)
}

fn denied(call: &RawToolCall) -> (Message, Event) {
    let event = Event::ToolDenied { id: call.id.clone(), name: call.name.clone() };
    (tool_message(&call.id, DENIED_MESSAGE, true), event)
}

fn failure(call: &RawToolCall, message: String) -> (Message, Event) {
    let event = Event::ToolResult { id: call.id.clone(), output: message.clone(), is_error: true };
    (tool_message(&call.id, &message, true), event)
}

fn parse_arguments(call: &RawToolCall) -> Result<Value, String> {
    if call.arguments.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&call.arguments)
        .map_err(|error| format!("The arguments aren't valid JSON: {error}. Send the call again, corrected."))
}

fn assistant_message(
    text: Option<String>,
    calls: &[RawToolCall],
    parsed: &[Result<Value, String>],
    opaque: Vec<Value>,
) -> Message {
    let tool_calls = calls
        .iter()
        .zip(parsed)
        .map(|(call, arguments)| {
            // Invalid arguments are not echoed: some servers reject them when replayed.
            let arguments = arguments.as_ref().map_or_else(|_| "{}".to_string(), Value::to_string);
            RawToolCall { id: call.id.clone(), name: call.name.clone(), arguments }
        })
        .collect();
    Message::Assistant { text, tool_calls, opaque }
}

fn tool_message(id: &str, content: &str, is_error: bool) -> Message {
    Message::Tool { id: id.into(), content: content.into(), is_error, images: vec![] }
}

/// The deferred tools that the journal says were loaded, in the order they were loaded.
fn loaded_tools(history: &[Event]) -> Vec<String> {
    let mut loaded: Vec<String> = vec![];
    for event in history {
        if let Event::ToolsLoaded { names } = event {
            for name in names {
                if !loaded.contains(name) {
                    loaded.push(name.clone());
                }
            }
        }
    }
    loaded
}

/// The file extension of an image type. Types that aren't listed get `bin`.
fn image_extension(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpeg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    }
}

/// Saves an image that the model can't see to the session folder, and returns a note that says where it is.
fn saved_image(image: &ImagePart, call_id: &str, index: usize, session_dir: &Path) -> String {
    let bytes = match STANDARD.decode(&image.base64) {
        Ok(bytes) => bytes,
        Err(error) => return format!("[An image could not be read: {error}.]"),
    };
    let extension = image_extension(&image.media_type);
    let folder = if session_dir.as_os_str().is_empty() { std::env::temp_dir() } else { session_dir.to_path_buf() };
    let stem: String = call_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    let path = folder.join(format!("tool-image-{stem}-{index}.{extension}"));
    match fs::create_dir_all(&folder).and_then(|()| fs::write(&path, bytes)) {
        Ok(()) => format!(
            "[An image is saved at {}. The model can't see images, so it is not in this reply.]",
            path.display()
        ),
        Err(error) => format!("[An image could not be saved: {error}.]"),
    }
}

/// The index after the run of events from `start` that match.
fn run_end(history: &[Event], start: usize, matches: impl Fn(&Event) -> bool) -> usize {
    let mut end = start;
    while end < history.len() && matches(&history[end]) {
        end += 1;
    }
    end
}

/// The model's messages from the journal. The calls of one step become one assistant message, and each call
/// is answered by the result with its id. A call with no result, such as one a stopped turn left behind, gets
/// a synthetic result, since strict APIs reject a call that has none. A summary stands in for everything
/// before it, so the replay starts at the last one.
fn history_messages(history: &[Event],files:&[vatra_core::TurnFile]) -> Vec<Message> {
    let start = history.iter().rposition(|event| matches!(event, Event::Compacted { .. })).unwrap_or(0);
    let history = &history[start..];
    let mut messages = Vec::new();
    let mut index = 0;
    while index < history.len() {
        if !matches!(history[index], Event::ToolCall { .. }) {
            if let Event::UserMessage{text}=&history[index] {
                let attached:Vec<_>=files.iter().filter(|f|f.history_index==Some(start+index)).collect();
                let text=vatra_core::input_with_files(text,attached.iter().copied());
                let images=attached.iter().filter_map(|f|f.image_base64.as_ref().map(|b|ImagePart{media_type:f.media_type.clone(),base64:b.clone()})).collect();
                messages.push(Message::User{text,images});
            }else{messages.extend(replay(&history[index]));}
            index += 1;
            continue;
        }
        let calls_end = run_end(history, index, |event| matches!(event, Event::ToolCall { .. }));
        let results_end =
            run_end(history, calls_end, |event| matches!(event, Event::ToolResult { .. } | Event::ToolDenied { .. }));
        let calls: Vec<RawToolCall> = history[index..calls_end].iter().filter_map(replay_call).collect();
        let results: Vec<Message> = history[calls_end..results_end].iter().filter_map(replay).collect();
        messages.push(Message::Assistant { text: None, tool_calls: calls.clone(), opaque: vec![] });
        for call in &calls {
            let answer = results
                .iter()
                .find(|message| matches!(message, Message::Tool { id, .. } if *id == call.id))
                .cloned()
                .unwrap_or_else(|| tool_message(&call.id, INTERRUPTED_MESSAGE, true));
            messages.push(answer);
        }
        index = results_end;
    }
    messages
}

fn replay_call(event: &Event) -> Option<RawToolCall> {
    let Event::ToolCall { id, name, arguments } = event else {
        return None;
    };
    let arguments = if arguments.is_object() { arguments.to_string() } else { "{}".into() };
    Some(RawToolCall { id: id.clone(), name: name.clone(), arguments })
}

/// The message one journaled event stands for. Calls are handled by `history_messages`, and events that the
/// model never saw give None.
fn replay(event: &Event) -> Option<Message> {
    let message = match event {
        Event::UserMessage { text } => Message::User { text: text.clone(), images: vec![] },
        Event::AssistantText { text } => {
            Message::Assistant { text: Some(text.clone()), tool_calls: vec![], opaque: vec![] }
        }
        Event::ToolResult { id, output, is_error } => tool_message(id, output, *is_error),
        Event::ToolDenied { id, .. } => tool_message(id, DENIED_MESSAGE, true),
        Event::SkillLoaded { text, .. } => Message::User { text: text.clone(), images: vec![] },
        Event::HandoffResult { text, .. } => Message::User { text: format!("Handoff result: {text}"), images: vec![] },
        Event::BackgroundResult { text, .. } => Message::User { text: text.clone(), images: vec![] },
        Event::RuleAdded { text, .. } => Message::User { text: text.clone(), images: vec![] },
        Event::Compacted { summary } => summary_message(summary),
        // The handoff prompt is already in history as the UserMessage that follows it.
        Event::ToolCall { .. }
        | Event::HandoffStarted { .. }
        | Event::TurnCompleted { .. }
        | Event::TurnFailed { .. }
        | Event::Unmapped { .. }
        | Event::NativeSession { .. }
        | Event::Warning { .. }
        | Event::Nested { .. }
        | Event::SubagentStarted { .. }
        | Event::SubagentFinished { .. }
        | Event::Hook { .. }
        | Event::Usage { .. }
        | Event::RateLimit { .. }
        | Event::Catalog { .. }
        | Event::ToolsLoaded { .. }
        // Deltas are never journaled.
        | Event::AssistantDelta { .. } => return None,
    };
    Some(message)
}

#[cfg(test)]
mod tests {
    use super::image_extension;

    #[test]
    fn image_extension_comes_from_a_safe_map() {
        assert_eq!(image_extension("image/png"), "png");
        assert_eq!(image_extension("image/jpeg"), "jpeg");
        assert_eq!(image_extension("image/gif"), "gif");
        assert_eq!(image_extension("image/webp"), "webp");
        assert_eq!(image_extension("image/svg+xml"), "bin");
        assert_eq!(image_extension("image/../../escape"), "bin");
    }
}
