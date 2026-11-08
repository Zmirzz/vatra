#![allow(dead_code)]

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::sync::mpsc;
use vatra_core::{Agent, AgentError, Event, McpServerConfig, PermissionMode, ProcessGroup, Rules, TurnRequest};
use vatra_engine::model::{ModelConfig, ModelDriver, OpenAiDriver};
use vatra_engine::{
    Approver, Background, EngineAgent, EngineSetup, ExpandVars, LoadedInstructions, McpHub, McpTool, NoQuestions,
    PermissionPolicy, Tool, ToolContext, ToolRegistry, tool_search_tool,
};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub fn fixture_server() -> McpServerConfig {
    McpServerConfig {
        name: "demo".into(),
        command: Some(env!("CARGO_BIN_EXE_vatra-fixture-mcp").into()),
        ..Default::default()
    }
}

pub fn text(content: &str) -> Value {
    json!({"choices": [{"message": {"role": "assistant", "content": content}}]})
}

pub fn tool_call(id: &str, name: &str, arguments: &str) -> Value {
    json!({"choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
        {"id": id, "type": "function", "function": {"name": name, "arguments": arguments}}
    ]}}]})
}

/// One answer that asks for several calls at once, given as (id, name, arguments).
pub fn tool_calls(calls: &[(&str, &str, &str)]) -> Value {
    let calls: Vec<Value> = calls
        .iter()
        .map(|(id, name, arguments)| json!({"id": id, "type": "function", "function": {"name": name, "arguments": arguments}}))
        .collect();
    json!({"choices": [{"message": {"role": "assistant", "content": null, "tool_calls": calls}}]})
}

pub async fn sequence(responses: Vec<Value>) -> MockServer {
    let server = MockServer::start().await;
    for response in responses {
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }
    server
}

pub async fn repeating(response: Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(&server)
        .await;
    server
}

pub async fn status(code: u16) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(code))
        .mount(&server)
        .await;
    server
}

pub async fn body(content: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(content))
        .mount(&server)
        .await;
    server
}

pub async fn expect_header(name: &'static str, value: &'static str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(header(name, value))
        .respond_with(ResponseTemplate::new(200).set_body_json(text("ok")))
        .expect(1)
        .mount(&server)
        .await;
    server
}

pub fn client(server: &MockServer, api_key: Option<&str>) -> Arc<dyn ModelDriver> {
    Arc::new(OpenAiDriver::new(ModelConfig {
        base_url: server.uri(),
        model: "m".into(),
        api_key: api_key.map(String::from),
        // Set, so the driver doesn't ask /props, which the mocks here would count as a request.
        context_window: Some(128_000),
    }))
}

/// The setup of an agent that talks to the mock server. It has the fixture MCP server, the given tools,
/// and the default permission mode. Tests change the fields they need.
pub async fn setup(
    server: &MockServer,
    approver: Arc<dyn Approver>,
    api_key: Option<&str>,
    tools: Vec<Arc<dyn Tool>>,
    max_steps: usize,
) -> EngineSetup {
    let hub = Arc::new(McpHub::new(vec![fixture_server()], ExpandVars::default()));
    hub.ensure_connected().await;
    let mut registry = ToolRegistry::default();
    for tool in tools {
        registry.register(tool);
    }
    registry.register(tool_search_tool(Arc::clone(&hub)));
    for tool in McpTool::all(&hub) {
        registry.register(tool);
    }
    let (events, _) = mpsc::unbounded_channel();
    EngineSetup {
        name: "local".into(),
        driver: client(server, api_key),
        hub,
        registry,
        policy: Arc::new(PermissionPolicy::new(PermissionMode::Supervised, Rules::default())),
        approver,
        context: ToolContext {
            cwd: PathBuf::new(),
            project_root: PathBuf::new(),
            session_dir: PathBuf::new(),
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
        max_steps,
        extensions: None,
        audit: None,
        session_id: None,
    }
}

/// Runs one turn of the agent and returns every event it sent.
pub async fn run_setup(setup: EngineSetup, history: Vec<Event>, prompt: &str) -> Result<Vec<Event>, AgentError> {
    let agent = EngineAgent::new(setup);
    let (sink, mut received) = mpsc::unbounded_channel();
    let request = TurnRequest { files: vec![],
        history,
        prompt: prompt.into(),
        native_session: None,
    };
    agent.run_turn(request, sink).await?;
    let mut events = vec![];
    while let Some(event) = received.recv().await {
        events.push(event);
    }
    Ok(events)
}

pub async fn run_with_history(
    server: &MockServer,
    approver: impl Approver + 'static,
    history: Vec<Event>,
    prompt: &str,
    api_key: Option<&str>,
) -> Result<Vec<Event>, AgentError> {
    let setup = setup(server, Arc::new(approver), api_key, vec![], 20).await;
    run_setup(setup, history, prompt).await
}

pub async fn run(
    server: &MockServer,
    approver: impl Approver + 'static,
    prompt: &str,
) -> Result<Vec<Event>, AgentError> {
    run_with_history(server, approver, vec![], prompt, None).await
}

/// The content of every tool message in one request the mock server received.
pub fn tool_contents(request: &wiremock::Request) -> Vec<String> {
    let body: Value = request.body_json().unwrap();
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["content"].as_str().unwrap().to_string())
        .collect()
}
