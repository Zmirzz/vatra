mod common;
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use common::{sequence, text, tool_call};
use serde_json::{Value, json};
use support::{TestServer, http_server};
use tokio::sync::mpsc;
use vatra_core::{Agent, Event, McpServerConfig, PermissionMode, ProcessGroup, Rules, TurnRequest};
use vatra_engine::model::{
    Completion, ContextWindow, ModelConfig, ModelDriver, ModelError, ModelRequest, OpenAiDriver,
};
use vatra_engine::{
    AllowAll, Approver, Background, DenyAll, EngineAgent, EngineSetup, ExpandVars, LoadedInstructions, McpHub, McpTool,
    NoQuestions, PermissionPolicy, Tool, ToolContext, ToolDef, ToolRegistry, deferred_split, read_tool,
    tool_search_tool,
};
use wiremock::MockServer;

const FILLER: &str = "mcp__demo__filler_042";

fn fixture(name: &str) -> McpServerConfig {
    fixture_with(name, &[])
}

fn fixture_with(name: &str, args: &[&str]) -> McpServerConfig {
    McpServerConfig {
        name: name.into(),
        command: Some(env!("CARGO_BIN_EXE_vatra-fixture-mcp").into()),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        ..Default::default()
    }
}

/// The fixture with 200 filler tools, which is more than a tenth of a small window.
fn many() -> McpServerConfig {
    fixture_with("demo", &["--many", "200"])
}

fn window(tokens: u64) -> ContextWindow {
    ContextWindow { tokens, estimated: false }
}

fn def(name: &str, description: &str) -> ToolDef {
    ToolDef { name: name.into(), description: description.into(), parameters: json!({"type": "object"}) }
}

/// A folder for the files that a test turn saves. It starts empty.
fn session_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vatra-mcp-large-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// Lines `row N` with padding, about 45 characters each.
fn rows(count: usize) -> String {
    (0..count).map(|number| format!("row {number:04} {}\n", "x".repeat(35))).collect()
}

/// The driver as it is, but one that says the model can't see images.
struct NoVision(Arc<dyn ModelDriver>);

#[async_trait]
impl ModelDriver for NoVision {
    async fn stream(
        &self,
        request: &ModelRequest,
        on_text: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<Completion, ModelError> {
        self.0.stream(request, on_text).await
    }

    fn context_window(&self) -> ContextWindow {
        self.0.context_window()
    }

    fn supports_images(&self) -> bool {
        false
    }
}

/// Runs one turn against the mock server, with the servers, extra tools and history given. Calls are approved.
async fn turn(
    server: &MockServer,
    tokens: u64,
    servers: Vec<McpServerConfig>,
    extra: Vec<Arc<dyn Tool>>,
    vision: bool,
    history: Vec<Event>,
    session: &Path,
) -> Vec<Event> {
    turn_as(Arc::new(AllowAll), server, tokens, servers, extra, vision, history, session).await
}

/// Runs one turn like `turn`, with the approver given. It returns the events of the turn.
#[allow(clippy::too_many_arguments)]
async fn turn_as(
    approver: Arc<dyn Approver>,
    server: &MockServer,
    tokens: u64,
    servers: Vec<McpServerConfig>,
    extra: Vec<Arc<dyn Tool>>,
    vision: bool,
    history: Vec<Event>,
    session: &Path,
) -> Vec<Event> {
    let hub = Arc::new(McpHub::new(servers, ExpandVars::default()));
    hub.ensure_connected().await;
    let mut registry = ToolRegistry::default();
    registry.register(tool_search_tool(Arc::clone(&hub)));
    for tool in McpTool::all(&hub) {
        registry.register(tool);
    }
    for tool in extra {
        registry.register(tool);
    }
    let model: Arc<dyn ModelDriver> = Arc::new(OpenAiDriver::new(ModelConfig {
        base_url: server.uri(),
        model: "m".into(),
        api_key: None,
        context_window: Some(tokens),
    }));
    let driver = if vision { model } else { Arc::new(NoVision(model)) };
    let (events, _) = mpsc::unbounded_channel();
    let agent = EngineAgent::new(EngineSetup {
        name: "local".into(),
        driver,
        hub,
        registry,
        policy: Arc::new(PermissionPolicy::new(PermissionMode::Supervised, Rules::default())),
        approver,
        context: ToolContext {
            cwd: PathBuf::new(),
            project_root: PathBuf::new(),
            session_dir: session.to_path_buf(),
            read_files: Default::default(),
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
        extensions: None,
        audit: None,
        session_id: None,
    });
    let (sink, mut received) = mpsc::unbounded_channel();
    agent.run_turn(TurnRequest { files: vec![], history, prompt: "salut".into(), native_session: None }, sink).await.unwrap();
    let mut events = vec![];
    while let Ok(event) = received.try_recv() {
        events.push(event);
    }
    events
}

/// The body of the request that the mock server got at this index.
async fn body_of(server: &MockServer, index: usize) -> Value {
    server.received_requests().await.unwrap()[index].body_json().unwrap()
}

/// The names of the tools that a request carried, in order.
fn tool_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .map(|tools| {
            tools.iter().map(|tool| tool["function"]["name"].as_str().unwrap_or_default().to_string()).collect()
        })
        .unwrap_or_default()
}

/// The content of the tool message that answers a call.
fn tool_content(body: &Value, call: &str) -> String {
    let message = body["messages"].as_array().unwrap().iter().find(|message| message["tool_call_id"] == call);
    message.and_then(|message| message["content"].as_str()).unwrap_or_default().to_string()
}

fn results(events: &[Event]) -> Vec<(String, String, bool)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::ToolResult { id, output, is_error } => Some((id.clone(), output.clone(), *is_error)),
            _ => None,
        })
        .collect()
}

/// The result of one call of a turn.
fn result_of(events: &[Event], call: &str) -> (String, bool) {
    let (_, output, is_error) =
        results(events).into_iter().find(|(id, _, _)| id == call).expect("the call has a result");
    (output, is_error)
}

#[test]
fn small_tool_list_loads_upfront() {
    let tools = vec![def("mcp__demo__a", "Short"), def("mcp__demo__b", "Short too")];
    let (upfront, deferred) = deferred_split(tools.clone(), window(128_000));
    assert_eq!(upfront, tools);
    assert!(deferred.is_empty());
}

#[test]
fn over_10_percent_defers() {
    let tools: Vec<ToolDef> = (0..200).map(|n| def(&format!("mcp__demo__filler_{n:03}"), &"x".repeat(300))).collect();
    let (upfront, deferred) = deferred_split(tools, window(10_000));
    assert!(upfront.is_empty());
    assert_eq!(deferred.len(), 200);
}

#[tokio::test]
async fn descriptions_capped_at_4096_upfront() {
    // The split caps what it sends upfront.
    let long = def("mcp__demo__long", &"d".repeat(5_000));
    let (upfront, _) = deferred_split(vec![long], window(128_000));
    assert_eq!(upfront[0].description.chars().count(), 4096);

    // The tools that McpTool gives the engine are capped too. Filler 7 of the fixture has a long description.
    let hub = McpHub::new(vec![many()], ExpandVars::default());
    hub.ensure_connected().await;
    let tools = McpTool::all(&Arc::new(hub));
    let filler = tools.iter().map(|tool| tool.def()).find(|def| def.name == "mcp__demo__filler_007").unwrap();
    assert_eq!(filler.description.chars().count(), 4096);
}

#[tokio::test]
async fn deferred_tools_are_not_sent() {
    let server = sequence(vec![text("ok")]).await;
    turn(&server, 8_000, vec![many()], vec![], true, vec![], &session_dir("not-sent")).await;
    let names = tool_names(&body_of(&server, 0).await);
    assert_eq!(names, ["ToolSearch"]);
}

#[tokio::test]
async fn always_load_stays_upfront() {
    let pinned = McpServerConfig { always_load: true, ..fixture("pinned") };
    let server = sequence(vec![text("ok")]).await;
    turn(&server, 8_000, vec![pinned, many()], vec![], true, vec![], &session_dir("always")).await;
    let names = tool_names(&body_of(&server, 0).await);
    assert!(names.contains(&"mcp__pinned__echo".to_string()), "{names:?}");
    assert!(!names.iter().any(|name| name.starts_with("mcp__demo__filler_")), "{names:?}");
}

#[tokio::test]
async fn tool_search_finds_by_name_and_description() {
    for (query, expected) in [("filler_042", FILLER), ("invoices", FILLER)] {
        let server =
            sequence(vec![tool_call("c1", "ToolSearch", &json!({ "query": query }).to_string()), text("ok")]).await;
        turn(&server, 8_000, vec![many()], vec![], true, vec![], &session_dir("search")).await;
        let result = tool_content(&body_of(&server, 1).await, "c1");
        assert!(result.contains(expected), "query {query}: {result}");
    }
}

#[tokio::test]
async fn found_tool_is_loaded_for_following_steps() {
    let server = sequence(vec![
        tool_call("c1", "ToolSearch", r#"{"query":"filler_042"}"#),
        tool_call("c2", FILLER, "{}"),
        text("ok"),
    ])
    .await;
    let events = turn(&server, 8_000, vec![many()], vec![], true, vec![], &session_dir("loaded")).await;
    assert!(!tool_names(&body_of(&server, 0).await).contains(&FILLER.to_string()));
    assert_eq!(tool_names(&body_of(&server, 1).await).last().map(String::as_str), Some(FILLER));
    assert!(results(&events).contains(&("c2".into(), "filler filler_042".into(), false)), "{:?}", results(&events));
    assert!(events.contains(&Event::ToolsLoaded { names: vec![FILLER.into()] }));
}

#[tokio::test]
async fn prefix_stays_stable_after_load() {
    let server = sequence(vec![
        tool_call("c1", "ToolSearch", r#"{"query":"filler_042"}"#),
        tool_call("c2", FILLER, "{}"),
        text("ok"),
    ])
    .await;
    turn(&server, 8_000, vec![many()], vec![], true, vec![], &session_dir("prefix")).await;
    let first = tool_names(&body_of(&server, 0).await);
    let second = tool_names(&body_of(&server, 1).await);
    assert_eq!(&second[..first.len()], first.as_slice());
    assert_eq!(second.len(), first.len() + 1);
}

#[tokio::test]
async fn loaded_tools_from_history_stay_loaded() {
    let history = vec![Event::ToolsLoaded { names: vec![FILLER.into()] }];
    let server = sequence(vec![text("ok")]).await;
    turn(&server, 8_000, vec![many()], vec![], true, history, &session_dir("history")).await;
    assert_eq!(tool_names(&body_of(&server, 0).await).last().map(String::as_str), Some(FILLER));
}

#[tokio::test]
async fn call_to_a_tool_not_loaded_yet_is_refused() {
    let server = sequence(vec![tool_call("c1", FILLER, "{}"), text("ok")]).await;
    let events = turn(&server, 8_000, vec![many()], vec![], true, vec![], &session_dir("refused")).await;
    let (output, is_error) = result_of(&events, "c1");
    assert!(is_error && output.contains("ToolSearch"), "{output}");
}

#[tokio::test]
async fn image_result_reaches_vision_model() {
    let server = sequence(vec![tool_call("c1", "mcp__demo__image", "{}"), text("ok")]).await;
    turn(&server, 128_000, vec![fixture("demo")], vec![], true, vec![], &session_dir("vision")).await;
    let body = body_of(&server, 1).await.to_string();
    assert!(body.contains("data:image/png;base64,aGVsbG8="), "{body}");
}

#[tokio::test]
async fn image_result_saved_to_file_without_vision() {
    let session = session_dir("no-vision");
    let server = sequence(vec![tool_call("c1", "mcp__demo__image", "{}"), text("ok")]).await;
    turn(&server, 128_000, vec![fixture("demo")], vec![], false, vec![], &session).await;
    let body = body_of(&server, 1).await;
    assert!(!body.to_string().contains("image_url"));
    let answer = tool_content(&body, "c1");
    assert!(answer.contains("is saved at"), "{answer}");
    assert_eq!(fs::read(session.join("tool-image-c1-0.png")).unwrap(), b"hello");
}

#[tokio::test]
async fn text_over_50000_chars_saved_to_file() {
    let session = session_dir("long-text");
    let server = sequence(vec![tool_call("c1", "mcp__demo__long_text", r#"{"characters":60000}"#), text("ok")]).await;
    turn(&server, 128_000, vec![fixture("demo")], vec![], true, vec![], &session).await;
    let answer = tool_content(&body_of(&server, 1).await, "c1");
    assert!(answer.contains("The full output is saved at"), "{answer}");
    let saved: Vec<PathBuf> = fs::read_dir(&session).unwrap().map(|entry| entry.unwrap().path()).collect();
    let file = saved.iter().find(|path| path.to_string_lossy().contains("tool-output-")).expect("a saved output");
    assert!(fs::read_to_string(file).unwrap().chars().count() >= 60_000);
}

#[tokio::test]
async fn error_over_50000_chars_saved_to_file() {
    let session = session_dir("long-error");
    let server = sequence(vec![tool_call("c1", "mcp__demo__long_error", r#"{"characters":60000}"#), text("ok")]).await;
    let events = turn(&server, 128_000, vec![fixture("demo")], vec![], true, vec![], &session).await;
    let (output, is_error) = result_of(&events, "c1");
    assert!(is_error);
    assert!(output.contains("The full output is saved at"), "{}", &output[..100.min(output.len())]);
    assert!(output.starts_with("START"));
}

#[tokio::test]
async fn error_over_11000_keeps_ends() {
    let session = session_dir("errors");
    let server = sequence(vec![tool_call("c1", "mcp__demo__long_error", r#"{"characters":12000}"#), text("ok")]).await;
    let events = turn(&server, 128_000, vec![fixture("demo")], vec![], true, vec![], &session).await;
    let (output, is_error) = result_of(&events, "c1");
    assert!(is_error);
    assert!(output.starts_with("START"), "{}", &output[..20]);
    assert!(output.ends_with("END"), "{}", &output[output.len() - 20..]);
    assert!(output.contains("characters were left out"));
    assert!(output.chars().count() < 11_000);
}

#[tokio::test]
async fn large_read_returns_its_content() {
    let source = session_dir("large-read-source");
    fs::create_dir_all(&source).unwrap();
    let file = source.join("rows.txt");
    fs::write(&file, rows(2_000)).unwrap();
    let server = sequence(vec![
        tool_call("c1", "Read", &json!({ "file_path": file.display().to_string() }).to_string()),
        text("ok"),
    ])
    .await;
    let events = turn(&server, 128_000, vec![], vec![read_tool()], true, vec![], &session_dir("large-read")).await;
    let (output, is_error) = result_of(&events, "c1");
    assert!(!is_error);
    assert!(output.contains("row 1999 "), "the read was cut short");
    assert!(!output.contains("saved at"), "a read was saved to a file");
}

#[tokio::test]
async fn saved_file_can_be_read_without_approval() {
    let session = session_dir("read-saved");
    fs::create_dir_all(&session).unwrap();
    let saved = session.join("tool-output-saved.txt");
    fs::write(&saved, rows(2_000)).unwrap();
    let arguments = json!({ "file_path": saved.display().to_string(), "offset": 1, "limit": 2 });
    let server = sequence(vec![tool_call("c1", "Read", &arguments.to_string()), text("ok")]).await;
    // Nobody can approve a call here, so a read that needs approval is refused.
    let events = turn_as(Arc::new(DenyAll), &server, 128_000, vec![], vec![read_tool()], true, vec![], &session).await;
    let (output, is_error) = result_of(&events, "c1");
    assert!(!is_error, "{output}");
    assert!(output.contains("row 0000") && output.contains("row 0001"), "{output}");
    assert!(!output.contains("saved at"));
}

#[tokio::test]
async fn prompts_listed() {
    let hub = McpHub::new(vec![fixture("demo")], ExpandVars::default());
    hub.ensure_connected().await;
    let prompt = hub.prompts().into_iter().find(|prompt| prompt.name == "mcp__demo__greet").expect("the greet prompt");
    assert_eq!(prompt.arguments.len(), 1);
    assert_eq!(prompt.arguments[0].name, "name");
    assert_eq!(prompt.arguments[0].description, "Who to greet");
    assert!(prompt.arguments[0].required);
    let filled = hub.get_prompt("mcp__demo__greet", json!({ "name": "Ana" })).await.unwrap();
    assert_eq!(filled, "[user] Greet Ana");
}

#[tokio::test]
async fn non_text_prompt_message_is_said_plainly() {
    let hub = McpHub::new(vec![fixture("demo")], ExpandVars::default());
    hub.ensure_connected().await;
    let filled = hub.get_prompt("mcp__demo__show", json!({})).await.unwrap();
    assert_eq!(filled, "[user] [non-text content not included: image]");
}

#[tokio::test]
async fn prompt_listing_failure_is_reported() {
    let hub = McpHub::new(vec![fixture_with("demo", &["--prompts-fail"])], ExpandVars::default());
    hub.ensure_connected().await;
    let failures = hub.failures();
    assert!(failures.iter().any(|(server, message)| server == "demo" && message.contains("prompts")), "{failures:?}");
    // The server keeps its tools.
    assert_eq!(hub.call("mcp__demo__echo", json!({ "text": "still here" })).await.text, "still here");
}

#[tokio::test]
async fn resources_listed() {
    let hub = McpHub::new(vec![fixture("demo")], ExpandVars::default());
    hub.ensure_connected().await;
    let dir = session_dir("resources");
    assert!(hub.resources().iter().any(|resource| resource.server == "demo" && resource.uri == "memo://notes"));
    let notes = hub.read_resource("demo", "memo://notes", &dir).await.unwrap();
    assert_eq!(notes.text, "Remember the milk");
    let picture = hub.read_resource("demo", "memo://picture", &dir).await.unwrap();
    assert_eq!(picture.images.len(), 1);
    assert_eq!(picture.images[0].media_type, "image/png");
}

#[tokio::test]
async fn server_without_prompts_is_fine() {
    let server = TestServer::start(false).await;
    let hub = McpHub::new(vec![http_server("remote", &server.url)], ExpandVars::default());
    hub.ensure_connected().await;
    assert!(hub.failures().is_empty(), "{:?}", hub.failures());
    assert!(hub.tools().iter().any(|tool| tool.name == "mcp__remote__echo"));
    assert!(hub.prompts().is_empty());
    assert!(hub.resources().is_empty());
}

#[tokio::test]
async fn local_model_with_200_tools_still_gets_a_short_request() {
    let server = sequence(vec![text("ok")]).await;
    turn(&server, 4_000, vec![many()], vec![], true, vec![], &session_dir("short")).await;
    let body = body_of(&server, 0).await;
    assert_eq!(tool_names(&body), ["ToolSearch"]);
    assert!(body.to_string().len() < 8_000, "{} characters", body.to_string().len());
}
