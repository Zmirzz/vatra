//! Subagents: the Agent tool, the types it runs, and the fork of a skill. The model is scripted in Rust, so every
//! request a subagent sends can be read back.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use vatra_core::{Agent, Event, PermissionMode, ProcessGroup, Rules, TurnRequest};
use vatra_engine::model::{
    Completion, ContextWindow, Message, ModelDriver, ModelError, ModelRequest, RawToolCall, TokenUsage,
};
use vatra_engine::{
    AllowAll, Approver, Background, DenyAll, EngineAgent, EngineExtensions, EngineSetup, ExpandVars,
    LoadedInstructions, McpHub, NoQuestions, PermissionPolicy, SkillSession, SubagentRunner, SubagentSetup, Tool,
    ToolClass, ToolContext, ToolDef, ToolOutput, ToolRegistry, agent_tool, bash_tool, skill_tool, write_tool,
};
use vatra_extensions::{Skill, parse_skill};

/// The answer to one request: the completion, or the error the model gave.
type Answer = Box<dyn Fn(&ModelRequest) -> Result<Completion, ModelError> + Send + Sync>;

/// A model that answers with the given function. Every request is kept, and each step may take some time.
struct Scripted {
    tokens: u64,
    delay: Duration,
    answer: Answer,
    requests: Mutex<Vec<ModelRequest>>,
    in_flight: AtomicUsize,
    peak: AtomicUsize,
}

impl Scripted {
    fn new(answer: impl Fn(&ModelRequest) -> Result<Completion, ModelError> + Send + Sync + 'static) -> Arc<Scripted> {
        Scripted::timed(128_000, Duration::ZERO, answer)
    }

    fn timed(
        tokens: u64,
        delay: Duration,
        answer: impl Fn(&ModelRequest) -> Result<Completion, ModelError> + Send + Sync + 'static,
    ) -> Arc<Scripted> {
        Arc::new(Scripted {
            tokens,
            delay,
            answer: Box::new(answer),
            requests: Mutex::default(),
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        })
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ModelDriver for Scripted {
    async fn stream(
        &self,
        request: &ModelRequest,
        _on_text: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<Completion, ModelError> {
        self.requests.lock().unwrap().push(request.clone());
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        (self.answer)(request)
    }

    fn context_window(&self) -> ContextWindow {
        ContextWindow { tokens: self.tokens, estimated: false }
    }

    fn supports_images(&self) -> bool {
        false
    }
}

fn text(content: &str) -> Completion {
    Completion { text: Some(content.into()), tool_calls: vec![], usage: TokenUsage::default(), opaque: vec![] }
}

/// One answer that asks for the given calls, as (id, name, arguments).
fn calls(list: &[(&str, &str, Value)]) -> Completion {
    let tool_calls = list
        .iter()
        .map(|(id, name, arguments)| RawToolCall {
            id: (*id).into(),
            name: (*name).into(),
            arguments: arguments.to_string(),
        })
        .collect();
    Completion { text: None, tool_calls, usage: TokenUsage::default(), opaque: vec![] }
}

/// A subagent's request is one that doesn't offer the Agent tool. The parent always offers it.
fn is_subagent(request: &ModelRequest) -> bool {
    !request.tools.iter().any(|tool| tool.name == "Agent")
}

fn has_tool_message(request: &ModelRequest) -> bool {
    request.messages.iter().any(|message| matches!(message, Message::Tool { .. }))
}

fn tool_names(request: &ModelRequest) -> Vec<String> {
    request.tools.iter().map(|tool| tool.name.clone()).collect()
}

fn tool_contents(request: &ModelRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::Tool { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

/// A stand-in tool. It answers with its output, and with `parked` it waits forever, which lets a test stop it.
struct Fake {
    name: &'static str,
    class: ToolClass,
    output: &'static str,
    parked: Option<Arc<AtomicUsize>>,
}

/// Counts itself when it is dropped, which is when the future of the parked call that holds it is dropped.
struct CountOnDrop(Arc<AtomicUsize>);

impl Drop for CountOnDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl Tool for Fake {
    fn def(&self) -> ToolDef {
        ToolDef { name: self.name.into(), description: "A test tool.".into(), parameters: json!({"type": "object"}) }
    }

    fn class(&self, _arguments: &Value, _context: &ToolContext) -> ToolClass {
        self.class
    }

    fn parallel_safe(&self) -> bool {
        true
    }

    async fn call(&self, _arguments: Value, _context: &ToolContext) -> ToolOutput {
        if let Some(count) = &self.parked {
            let _guard = CountOnDrop(Arc::clone(count));
            std::future::pending::<()>().await;
        }
        ToolOutput::ok(self.output)
    }
}

fn fake(name: &'static str, class: ToolClass, output: &'static str) -> Arc<dyn Tool> {
    Arc::new(Fake { name, class, output, parked: None })
}

/// Records the tools that were asked about, and approves them.
#[derive(Default)]
struct Asked(Mutex<Vec<String>>);

#[async_trait]
impl Approver for Asked {
    async fn approve(&self, name: &str, _arguments: &Value) -> bool {
        self.0.lock().unwrap().push(name.into());
        true
    }
}

/// An agent whose tools are the given ones, with the Agent tool added the way the service adds it. The skills are
/// the turn's; with any, the Skill tool is there too.
fn agent(
    driver: Arc<Scripted>,
    approver: Arc<dyn Approver>,
    tools: Vec<Arc<dyn Tool>>,
    dir: &Path,
    skills: Vec<Skill>,
) -> EngineAgent {
    let hub = Arc::new(McpHub::new(vec![], ExpandVars::default()));
    let policy = Arc::new(PermissionPolicy::new(PermissionMode::Supervised, Rules::default()));
    let session = Arc::new(SkillSession::new("7"));
    let mut registry = ToolRegistry::default();
    for tool in tools {
        registry.register(tool);
    }
    let runner = Arc::new(SubagentRunner::new(SubagentSetup {
        driver: Arc::clone(&driver) as Arc<dyn ModelDriver>,
        hub: Arc::clone(&hub),
        tools: &registry,
        policy: Arc::clone(&policy),
        approver: Arc::clone(&approver),
        instructions: LoadedInstructions::default(),
        max_steps: 20,
        session: Arc::clone(&session),
        audit: None,
        session_id: None,
    }));
    let mut extensions = EngineExtensions::default();
    extensions.session = session;
    extensions.subagents = Some(Arc::clone(&runner));
    extensions.skills = skills;
    let extensions = Arc::new(extensions);
    if !extensions.skills.is_empty() {
        registry.register(skill_tool(Arc::clone(&extensions)));
    }
    registry.register(agent_tool(runner));
    registry.alias("Task", "Agent");
    let (events, _) = mpsc::unbounded_channel();
    EngineAgent::new(EngineSetup {
        name: "local".into(),
        driver,
        hub,
        registry,
        policy,
        approver,
        context: ToolContext {
            cwd: dir.into(),
            project_root: dir.into(),
            session_dir: dir.into(),
            read_files: Arc::new(Mutex::new(HashSet::new())),
            events,
            questions: Arc::new(NoQuestions),
            tree: Arc::new(ProcessGroup::new().unwrap()),
            background: Arc::new(Background::new()),
            hidden_env: vec![],
            memory_dir: None,
        },
        instructions: LoadedInstructions::default(),
        memory: None,
        max_steps: 20,
        extensions: Some(extensions),
        audit: None,
        session_id: None,
    })
}

/// Runs one turn and returns every event it sent.
async fn turn(agent: &EngineAgent, history: Vec<Event>, prompt: &str) -> Vec<Event> {
    let (sink, mut received) = mpsc::unbounded_channel();
    let request = TurnRequest { files: vec![], history, prompt: prompt.into(), native_session: None };
    agent.run_turn(request, sink).await.unwrap();
    let mut events = vec![];
    while let Some(event) = received.recv().await {
        events.push(event);
    }
    events
}

fn results(events: &[Event]) -> Vec<(&str, &str, bool)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::ToolResult { id, output, is_error } => Some((id.as_str(), output.as_str(), *is_error)),
            _ => None,
        })
        .collect()
}

fn position(events: &[Event], wanted: impl Fn(&Event) -> bool) -> usize {
    events.iter().position(wanted).expect("the event is there")
}

/// A skill folder `name` with the given SKILL.md text.
fn skill(root: &Path, name: &str, text: &str) -> Skill {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), text).unwrap();
    parse_skill(&dir).unwrap()
}

/// A model that asks for `count` subagents in its first answer, and answers the subagents and the rest of the turn.
/// Each step takes `delay`, so the subagents overlap unless the slots hold some back.
fn fan_out(tokens: u64, count: usize) -> Arc<Scripted> {
    Scripted::timed(tokens, Duration::from_millis(50), move |request| {
        if is_subagent(request) {
            return Ok(text("ok"));
        }
        if has_tool_message(request) {
            return Ok(text("done"));
        }
        let asks: Vec<(String, Value)> =
            (1..=count).map(|index| (format!("a{index}"), json!({"prompt": format!("task {index}")}))).collect();
        let list: Vec<(&str, &str, Value)> =
            asks.iter().map(|(id, args)| (id.as_str(), "Agent", args.clone())).collect();
        Ok(calls(&list))
    })
}

#[tokio::test]
async fn events_are_nested_under_call() {
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            return Ok(text("found"));
        }
        if has_tool_message(request) {
            return Ok(text("done"));
        }
        Ok(calls(&[("c1", "Agent", json!({"prompt": "look", "description": "look"}))]))
    });
    let dir = tempfile::tempdir().unwrap();
    let events = turn(&agent(driver, Arc::new(AllowAll), vec![], dir.path(), vec![]), vec![], "salut").await;

    let call = position(&events, |event| matches!(event, Event::ToolCall { id, .. } if id == "c1"));
    let started = position(&events, |event| matches!(event, Event::SubagentStarted { id, .. } if id == "c1"));
    let nested = position(&events, |event| {
        matches!(event, Event::Nested { parent_id, event } if parent_id == "c1"
            && matches!(event.as_ref(), Event::AssistantText { text } if text == "found"))
    });
    let finished = position(
        &events,
        |event| matches!(event, Event::SubagentFinished { id, status } if id == "c1" && status == "completed"),
    );
    let result = position(&events, |event| matches!(event, Event::ToolResult { id, .. } if id == "c1"));
    assert!(call < started && started < nested && nested < finished && finished < result, "{events:?}");
}

#[tokio::test]
async fn result_goes_back_to_parent() {
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            return Ok(text("found"));
        }
        if has_tool_message(request) {
            return Ok(text("done"));
        }
        Ok(calls(&[("c1", "Agent", json!({"prompt": "look"}))]))
    });
    let dir = tempfile::tempdir().unwrap();
    let events =
        turn(&agent(Arc::clone(&driver), Arc::new(AllowAll), vec![], dir.path(), vec![]), vec![], "salut").await;

    assert!(results(&events).contains(&("c1", "found", false)));
    let parent = driver.requests().into_iter().find(|request| !is_subagent(request) && has_tool_message(request));
    assert_eq!(tool_contents(&parent.unwrap()), ["found"]);
}

#[tokio::test]
async fn explore_cannot_write() {
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            if has_tool_message(request) {
                return Ok(text("stopped"));
            }
            return Ok(calls(&[("w1", "Write", json!({"file_path": "made.txt", "content": "x"}))]));
        }
        if has_tool_message(request) {
            return Ok(text("done"));
        }
        Ok(calls(&[("c1", "Agent", json!({"prompt": "try", "subagent_type": "Explore"}))]))
    });
    let dir = tempfile::tempdir().unwrap();
    let tools = vec![write_tool(), bash_tool(), fake("Read", ToolClass::Free, "read")];
    let events =
        turn(&agent(Arc::clone(&driver), Arc::new(AllowAll), tools, dir.path(), vec![]), vec![], "salut").await;

    let sub = driver.requests().into_iter().find(is_subagent).unwrap();
    let offered = tool_names(&sub);
    for banned in ["Write", "Edit", "apply_patch", "Bash", "PowerShell"] {
        assert!(!offered.iter().any(|name| name == banned), "{banned} is offered to Explore");
    }
    assert!(offered.iter().any(|name| name == "Read"));
    assert!(!dir.path().join("made.txt").exists());
    // The subagent's steps are nested, so its answer to the Write call is in its second request.
    let subs: Vec<ModelRequest> = driver.requests().into_iter().filter(is_subagent).collect();
    assert_eq!(tool_contents(&subs[1]), ["unknown tool: Write"]);
    assert!(!results(&events).iter().any(|(id, _, _)| *id == "w1"));
}

#[tokio::test]
async fn no_agent_tool_inside_subagent() {
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            if has_tool_message(request) {
                return Ok(text("ok"));
            }
            return Ok(calls(&[("s1", "Agent", json!({"prompt": "nested"}))]));
        }
        if has_tool_message(request) {
            return Ok(text("done"));
        }
        Ok(calls(&[("c1", "Agent", json!({"prompt": "outer"}))]))
    });
    let dir = tempfile::tempdir().unwrap();
    let events =
        turn(&agent(Arc::clone(&driver), Arc::new(AllowAll), vec![], dir.path(), vec![]), vec![], "salut").await;

    let subs: Vec<ModelRequest> = driver.requests().into_iter().filter(is_subagent).collect();
    assert!(subs.iter().all(|request| !tool_names(request).iter().any(|name| name == "Agent")));
    assert_eq!(tool_contents(&subs[1]), ["unknown tool: Agent"]);
    assert!(results(&events).contains(&("c1", "ok", false)));
}

#[tokio::test]
async fn unknown_type_tells_model() {
    let driver = Scripted::new(|request| {
        if has_tool_message(request) {
            return Ok(text("done"));
        }
        Ok(calls(&[("c1", "Agent", json!({"prompt": "x", "subagent_type": "Plan"}))]))
    });
    let dir = tempfile::tempdir().unwrap();
    let events = turn(&agent(driver, Arc::new(AllowAll), vec![], dir.path(), vec![]), vec![], "salut").await;

    let (_, output, is_error) = results(&events).into_iter().find(|(id, _, _)| *id == "c1").unwrap();
    assert!(is_error);
    assert!(output.contains("Plan") && output.contains("general-purpose") && output.contains("Explore"), "{output}");
}

#[tokio::test]
async fn four_in_parallel_fifth_waits() {
    let driver = fan_out(128_000, 5);
    let dir = tempfile::tempdir().unwrap();
    let events = tokio::time::timeout(
        Duration::from_secs(10),
        turn(&agent(Arc::clone(&driver), Arc::new(AllowAll), vec![], dir.path(), vec![]), vec![], "salut"),
    )
    .await
    .expect("the fifth subagent waited for a slot that never opened");

    assert_eq!(driver.peak.load(Ordering::SeqCst), 4);
    assert_eq!(results(&events).iter().filter(|(_, output, is_error)| *output == "ok" && !is_error).count(), 5);
}

#[tokio::test]
async fn small_window_runs_two_in_parallel() {
    let driver = fan_out(16_000, 4);
    let dir = tempfile::tempdir().unwrap();
    turn(&agent(Arc::clone(&driver), Arc::new(AllowAll), vec![], dir.path(), vec![]), vec![], "salut").await;
    assert_eq!(driver.peak.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn approvals_reach_parent_session() {
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            if has_tool_message(request) {
                return Ok(text("probed"));
            }
            return Ok(calls(&[("p1", "Probe", json!({}))]));
        }
        if has_tool_message(request) {
            return Ok(text("done"));
        }
        Ok(calls(&[("c1", "Agent", json!({"prompt": "probe"}))]))
    });
    let asked = Arc::new(Asked::default());
    let dir = tempfile::tempdir().unwrap();
    let tools = vec![fake("Probe", ToolClass::Execute, "probed")];
    turn(&agent(driver, Arc::clone(&asked) as Arc<dyn Approver>, tools, dir.path(), vec![]), vec![], "salut").await;
    assert_eq!(asked.0.lock().unwrap().as_slice(), ["Probe"]);
}

#[tokio::test]
async fn stop_kills_all_subagents() {
    // Four subagents run at once, and each waits in a call that never ends.
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            return Ok(calls(&[("p1", "Parked", json!({}))]));
        }
        let asks: Vec<(String, Value)> =
            (1..=4).map(|index| (format!("c{index}"), json!({"prompt": "wait"}))).collect();
        let list: Vec<(&str, &str, Value)> =
            asks.iter().map(|(id, args)| (id.as_str(), "Agent", args.clone())).collect();
        Ok(calls(&list))
    });
    let parked = Arc::new(AtomicUsize::new(0));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Fake {
        name: "Parked",
        class: ToolClass::Free,
        output: "never",
        parked: Some(Arc::clone(&parked)),
    })];
    let dir = tempfile::tempdir().unwrap();
    let agent = agent(Arc::clone(&driver), Arc::new(AllowAll), tools, dir.path(), vec![]);
    let (sink, _received) = mpsc::unbounded_channel();
    let request = TurnRequest { files: vec![], history: vec![], prompt: "salut".into(), native_session: None };
    let stopped = tokio::time::timeout(Duration::from_millis(300), agent.run_turn(request, sink)).await;
    assert!(stopped.is_err(), "the turn should still be waiting on the subagents");
    assert_eq!(driver.requests().iter().filter(|request| is_subagent(request)).count(), 4);
    assert_eq!(parked.load(Ordering::SeqCst), 4, "not every subagent's call was ended with the turn");
}

#[tokio::test]
async fn explore_offers_only_read_tools() {
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            if has_tool_message(request) {
                return Ok(text("stopped"));
            }
            return Ok(calls(&[
                ("m1", "mcp__demo__echo", json!({"text": "x"})),
                ("t1", "ToolSearch", json!({"query": "x"})),
                ("k1", "TaskStop", json!({"id": "x"})),
            ]));
        }
        if has_tool_message(request) {
            return Ok(text("done"));
        }
        Ok(calls(&[("c1", "Agent", json!({"prompt": "look", "subagent_type": "Explore"}))]))
    });
    let tools = vec![
        fake("Read", ToolClass::Free, "read"),
        fake("Glob", ToolClass::Free, "glob"),
        fake("mcp__demo__echo", ToolClass::Mcp, "echoed"),
        fake("ToolSearch", ToolClass::Free, "found"),
        fake("TaskStop", ToolClass::Free, "stopped"),
    ];
    let dir = tempfile::tempdir().unwrap();
    turn(&agent(Arc::clone(&driver), Arc::new(AllowAll), tools, dir.path(), vec![]), vec![], "salut").await;

    let subs: Vec<ModelRequest> = driver.requests().into_iter().filter(is_subagent).collect();
    assert_eq!(tool_names(&subs[0]), ["Read", "Glob"]);
    assert_eq!(
        tool_contents(&subs[1]),
        ["unknown tool: mcp__demo__echo", "unknown tool: ToolSearch", "unknown tool: TaskStop"]
    );
}

#[tokio::test]
async fn subagent_usage_is_top_level() {
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            return Ok(Completion {
                text: Some("found".into()),
                tool_calls: vec![],
                usage: TokenUsage { input: 7, output: 3, cached_input: 0 },
                opaque: vec![],
            });
        }
        if has_tool_message(request) {
            return Ok(text("done"));
        }
        Ok(calls(&[("c1", "Agent", json!({"prompt": "look"}))]))
    });
    let dir = tempfile::tempdir().unwrap();
    let events = turn(&agent(driver, Arc::new(AllowAll), vec![], dir.path(), vec![]), vec![], "salut").await;

    let usage = Event::Usage { input_tokens: 7, output_tokens: 3, cost_usd: None };
    assert_eq!(events.iter().filter(|event| **event == usage).count(), 1, "{events:?}");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Nested { event, .. } if matches!(event.as_ref(), Event::Usage { .. })))
    );
}

#[tokio::test]
async fn subagent_without_text_says_so() {
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            return Ok(calls(&[]));
        }
        if has_tool_message(request) {
            return Ok(text("done"));
        }
        Ok(calls(&[("c1", "Agent", json!({"prompt": "look"}))]))
    });
    let dir = tempfile::tempdir().unwrap();
    let events = turn(&agent(driver, Arc::new(AllowAll), vec![], dir.path(), vec![]), vec![], "salut").await;

    let (_, output, is_error) = results(&events).into_iter().find(|(id, _, _)| *id == "c1").unwrap();
    assert!(!is_error);
    assert_eq!(output, "The subagent finished without a text answer.");
}

#[tokio::test]
async fn failed_fork_warns_and_turn_goes_on() {
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            return Err(ModelError::Other("boom".into()));
        }
        Ok(text("parent done"))
    });
    let dir = tempfile::tempdir().unwrap();
    let forky = skill(
        dir.path(),
        "forky",
        "---\nname: forky\ndescription: Summarizes.\ncontext: fork\n---\nSummarize the repo.\n",
    );
    let agent = agent(driver, Arc::new(AllowAll), vec![], dir.path(), vec![forky]);
    // The turn returns Ok, so `turn` doesn't panic: the failure is a warning, and the model answers after it.
    let events = turn(&agent, vec![], "/forky").await;

    assert!(
        events.iter().any(|event| matches!(event, Event::Warning { message, .. } if message.contains("boom"))),
        "{events:?}"
    );
    assert_eq!(events.last(), Some(&Event::AssistantText { text: "parent done".into() }));
}

#[tokio::test]
async fn fork_skill_runs_as_subagent() {
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            return Ok(text("the summary"));
        }
        Ok(text("parent done"))
    });
    let dir = tempfile::tempdir().unwrap();
    let forky = skill(
        dir.path(),
        "forky",
        "---\nname: forky\ndescription: Summarizes.\ncontext: fork\n---\nSummarize the repo.\n",
    );
    let agent = agent(Arc::clone(&driver), Arc::new(AllowAll), vec![], dir.path(), vec![forky]);
    let events = turn(&agent, vec![], "/forky").await;

    assert!(events.contains(&Event::SkillLoaded { name: "forky".into(), text: "the summary".into() }), "{events:?}");
    let sub = driver.requests().into_iter().find(is_subagent).unwrap();
    let user: Vec<&str> = sub
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User { text, .. } => Some(text.trim()),
            _ => None,
        })
        .collect();
    assert_eq!(user, ["Summarize the repo."]);
}

#[tokio::test]
async fn fork_gets_no_history() {
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            return Ok(text("the summary"));
        }
        Ok(text("parent done"))
    });
    let dir = tempfile::tempdir().unwrap();
    let forky = skill(
        dir.path(),
        "forky",
        "---\nname: forky\ndescription: Summarizes.\ncontext: fork\n---\nSummarize the repo.\n",
    );
    let history = vec![
        Event::UserMessage { text: "secret earlier".into() },
        Event::AssistantText { text: "earlier answer".into() },
    ];
    let agent = agent(Arc::clone(&driver), Arc::new(AllowAll), vec![], dir.path(), vec![forky]);
    turn(&agent, history, "/forky").await;

    let sub = driver.requests().into_iter().find(is_subagent).unwrap();
    let roles: Vec<&str> = sub
        .messages
        .iter()
        .map(|message| match message {
            Message::System { .. } => "system",
            Message::User { .. } => "user",
            Message::Assistant { .. } => "assistant",
            Message::Tool { .. } => "tool",
        })
        .collect();
    assert_eq!(roles, ["system", "user"]);
    let everything = format!("{:?}", sub.messages);
    assert!(!everything.contains("secret earlier") && !everything.contains("earlier answer"));
}

#[tokio::test]
async fn fork_grants_reach_subagent() {
    // The skill allows Bash, which needs approval. The approver refuses everything, so the call only runs if the
    // grant of the user's skill reaches the subagent.
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            if has_tool_message(request) {
                return Ok(text("done"));
            }
            return Ok(calls(&[("p1", "Bash", json!({"command": "echo probed"}))]));
        }
        Ok(text("parent done"))
    });
    let dir = tempfile::tempdir().unwrap();
    let forky = skill(
        dir.path(),
        "forky",
        "---\nname: forky\ndescription: Runs a command.\ncontext: fork\nallowed-tools: Bash\n---\nRun the command.\n",
    );
    let tools = vec![fake("Bash", ToolClass::Execute, "probed")];
    let agent = agent(Arc::clone(&driver), Arc::new(DenyAll), tools, dir.path(), vec![forky]);
    turn(&agent, vec![], "/forky").await;

    let sub = driver.requests().into_iter().rev().find(is_subagent).unwrap();
    assert_eq!(tool_contents(&sub), ["probed"]);
}

#[tokio::test]
async fn subagent_failure_is_tool_error_not_turn_failure() {
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            return Err(ModelError::Other("boom".into()));
        }
        if has_tool_message(request) {
            return Ok(text("fin"));
        }
        Ok(calls(&[("c1", "Agent", json!({"prompt": "fail"}))]))
    });
    let dir = tempfile::tempdir().unwrap();
    let events = turn(&agent(driver, Arc::new(AllowAll), vec![], dir.path(), vec![]), vec![], "salut").await;

    let (_, output, is_error) = results(&events).into_iter().find(|(id, _, _)| *id == "c1").unwrap();
    assert!(is_error && output.contains("boom"), "{output}");
    assert_eq!(events.last(), Some(&Event::AssistantText { text: "fin".into() }));
}

#[tokio::test]
async fn task_alias_works() {
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            return Ok(text("found"));
        }
        if has_tool_message(request) {
            return Ok(text("done"));
        }
        Ok(calls(&[("c1", "Task", json!({"prompt": "look"}))]))
    });
    let dir = tempfile::tempdir().unwrap();
    let events = turn(&agent(driver, Arc::new(AllowAll), vec![], dir.path(), vec![]), vec![], "salut").await;
    assert!(results(&events).contains(&("c1", "found", false)));
}

#[tokio::test]
async fn model_argument_is_ignored_with_warning() {
    let driver = Scripted::new(|request| {
        if is_subagent(request) {
            return Ok(text("found"));
        }
        if has_tool_message(request) {
            return Ok(text("done"));
        }
        Ok(calls(&[("c1", "Agent", json!({"prompt": "look", "model": "opus"}))]))
    });
    let dir = tempfile::tempdir().unwrap();
    let events = turn(&agent(driver, Arc::new(AllowAll), vec![], dir.path(), vec![]), vec![], "salut").await;
    assert!(
        events.iter().any(|event| matches!(event, Event::Warning { message, .. } if message.contains("opus"))),
        "{events:?}"
    );
    assert!(results(&events).contains(&("c1", "found", false)));
}
