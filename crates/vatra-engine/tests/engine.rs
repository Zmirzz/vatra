mod common;

#[tokio::test]
async fn image_and_document_inputs_reach_the_real_driver_payload() {
    use vatra_core::{Agent,TurnRequest,TurnFile};
    use vatra_engine::{EngineAgent,AllowAll};
    let server=common::sequence(vec![common::text("read both")]).await;
    let setup=common::setup(&server,std::sync::Arc::new(AllowAll),None,vec![],4).await;
    let agent=EngineAgent::new(setup);
    // Keep the receiver alive while the real engine executes.
    let (sink,mut received)=tokio::sync::mpsc::unbounded_channel();
    let request=TurnRequest{history:vec![],prompt:"compare".into(),native_session:None,files:vec![
        TurnFile{id:1,revision:1,name:"image.png".into(),media_type:"image/png".into(),text:String::new(),image_base64:Some("aW1hZ2U=".into()),history_index:None},
        TurnFile{id:2,revision:1,name:"report.txt".into(),media_type:"text/plain".into(),text:"document evidence".into(),image_base64:None,history_index:None}
    ]};
    agent.run_turn(request,sink).await.unwrap();while received.try_recv().is_ok(){}
    let requests=server.received_requests().await.unwrap();let body:serde_json::Value=serde_json::from_slice(&requests.last().unwrap().body).unwrap();
    let user=body["messages"].as_array().unwrap().iter().find(|m|m["role"]=="user").unwrap();
    assert!(user["content"][0]["text"].as_str().unwrap().contains("document evidence"));
    assert_eq!(user["content"][1]["image_url"]["url"],"data:image/png;base64,aW1hZ2U=");
}

#[tokio::test]
async fn compaction_keeps_the_current_image_input(){
    use vatra_core::{Agent,Event,TurnRequest,TurnFile};
    use vatra_engine::{EngineAgent,AllowAll};
    let server=common::sequence(vec![common::text("condensed earlier conversation"),common::text("done")]).await;
    let setup=common::setup(&server,std::sync::Arc::new(AllowAll),None,vec![],4).await;
    let agent=EngineAgent::new(setup);let(sink,mut received)=tokio::sync::mpsc::unbounded_channel();
    let request=TurnRequest{history:vec![Event::UserMessage{text:"old context ".repeat(70_000)}],prompt:"inspect".into(),native_session:None,files:vec![TurnFile{id:1,revision:1,name:"image.png".into(),media_type:"image/png".into(),text:String::new(),image_base64:Some("aW1hZ2U=".into()),history_index:None}]};
    agent.run_turn(request,sink).await.unwrap();while received.try_recv().is_ok(){}
    let requests=server.received_requests().await.unwrap();
    let last:serde_json::Value=serde_json::from_slice(&requests.last().unwrap().body).unwrap();
    assert!(last.to_string().contains("data:image/png;base64,aW1hZ2U="));
}

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use common::*;
use serde_json::{Value, json};
use tokio::sync::Barrier;
use vatra_core::{Agent, AgentError, Event, TurnRequest};
use vatra_engine::{AllowAll, DenyAll, EngineAgent, McpHub, Tool, ToolClass, ToolContext, ToolDef, ToolOutput, write_tool};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A stand-in tool. Its output is the `text` argument, unless it has its own. Tests set what it does.
struct Fake {
    name: &'static str,
    class: ToolClass,
    parallel: bool,
    delay: Duration,
    barrier: Option<Arc<Barrier>>,
    log: Option<Arc<Mutex<Vec<String>>>>,
    output: Option<&'static str>,
}

fn fake(name: &'static str, class: ToolClass, parallel: bool) -> Fake {
    Fake { name, class, parallel, delay: Duration::ZERO, barrier: None, log: None, output: None }
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
        self.parallel
    }

    async fn call(&self, arguments: Value, _context: &ToolContext) -> ToolOutput {
        let label = arguments["text"].as_str().unwrap_or_default().to_string();
        if let Some(log) = &self.log {
            log.lock().unwrap().push(format!("start {label}"));
        }
        if let Some(barrier) = &self.barrier {
            barrier.wait().await;
        }
        tokio::time::sleep(self.delay).await;
        if let Some(log) = &self.log {
            log.lock().unwrap().push(format!("end {label}"));
        }
        ToolOutput::ok(self.output.map_or(label, str::to_string))
    }
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

#[tokio::test]
async fn tool_call_loop_uses_mcp_and_finishes_with_text() {
    let server = sequence(vec![tool_call("c1", "mcp__demo__echo", r#"{"text":"ășț"}"#), text("gata")]).await;
    let events = run(&server, AllowAll, "salut").await.unwrap();
    assert!(events.contains(&Event::ToolResult { id: "c1".into(), output: "ășț".into(), is_error: false }));
    assert_eq!(events.last(), Some(&Event::AssistantText { text: "gata".into() }));
}

#[tokio::test]
async fn invalid_json_arguments_go_back_to_the_model_as_an_error() {
    let server = sequence(vec![tool_call("c1", "mcp__demo__echo", "{nu e json"), text("corectat")]).await;
    let events = run(&server, AllowAll, "salut").await.unwrap();
    assert!(events.iter().any(|e| matches!(e, Event::ToolResult { id, is_error: true, .. } if id == "c1")));
    assert_eq!(events.last(), Some(&Event::AssistantText { text: "corectat".into() }));
}

#[tokio::test]
async fn denied_tool_is_recorded_and_not_called() {
    let server = sequence(vec![tool_call("c1", "mcp__demo__crash", "{}"), text("ok")]).await;
    let events = run(&server, DenyAll, "salut").await.unwrap();
    assert!(events.contains(&Event::ToolDenied { id: "c1".into(), name: "mcp__demo__crash".into() }));
    assert!(!events.iter().any(|e| matches!(e, Event::ToolResult { .. })));
}

#[tokio::test]
async fn endless_tool_loop_stops_at_max_steps() {
    let server = repeating(tool_call("c1", "mcp__demo__echo", r#"{"text":"x"}"#)).await;
    assert!(matches!(run(&server, AllowAll, "salut").await, Err(AgentError::Failed(_))));
}

#[tokio::test]
async fn step_limit_message_is_clear() {
    let server = repeating(tool_call("c1", "mcp__demo__echo", r#"{"text":"x"}"#)).await;
    let setup = setup(&server, Arc::new(AllowAll), None, vec![], 3).await;
    let error = run_setup(setup, vec![], "salut").await.unwrap_err();
    let AgentError::Failed(message) = error else { panic!("expected a failed turn") };
    assert!(message.contains("3 steps"), "{message}");
    assert!(message.contains("max_steps"), "{message}");
}

#[tokio::test]
async fn same_call_repeated_8_times_stops_turn() {
    let server = repeating(tool_call("c1", "Nope", "{}")).await;
    let setup = setup(&server, Arc::new(AllowAll), None, vec![], 20).await;
    let error = run_setup(setup, vec![], "salut").await.unwrap_err();
    let AgentError::Failed(message) = error else { panic!("expected a failed turn") };
    assert!(message.contains("8 times"), "{message}");
    assert!(message.contains("unknown tool: Nope"), "{message}");
}

#[tokio::test]
async fn http_500_and_malformed_body_are_failed_turns() {
    assert!(matches!(run(&status(500).await, AllowAll, "x").await, Err(AgentError::Failed(_))));
    assert!(matches!(run(&body("nu e json").await, AllowAll, "x").await, Err(AgentError::Failed(_))));
}

#[tokio::test]
async fn api_key_is_sent_only_when_configured() {
    let server = expect_header("authorization", "Bearer k").await;
    run_with_history(&server, AllowAll, vec![], "x", Some("k")).await.unwrap();
}

#[tokio::test]
async fn history_is_sent_as_openai_messages_with_shared_tools() {
    let server = sequence(vec![text("ok")]).await;
    let history = vec![
        Event::UserMessage { text: "u".into() },
        Event::ToolCall { id: "c1".into(), name: "mcp__demo__echo".into(), arguments: json!({"text": "t"}) },
        Event::ToolResult { id: "c1".into(), output: "t".into(), is_error: false },
        Event::ToolCall { id: "c2".into(), name: "mcp__demo__crash".into(), arguments: json!({}) },
        Event::ToolDenied { id: "c2".into(), name: "mcp__demo__crash".into() },
        Event::AssistantText { text: "a".into() },
        Event::TurnCompleted { agent: "local".into() },
        Event::Unmapped { source: "x".into(), raw: "y".into() },
        Event::HandoffResult { to_session: 2, text: "rezolvat".into() },
    ];
    run_with_history(&server, AllowAll, history, "nou", None).await.unwrap();

    let request: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    let roles: Vec<_> =
        request["messages"].as_array().unwrap().iter().map(|message| message["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["system", "user", "assistant", "tool", "assistant", "tool", "assistant", "user", "user"]);
    assert!(request["messages"][7]["content"].as_str().unwrap().contains("rezolvat"));
    assert_eq!(request["messages"][8]["content"], "nou");
    assert_eq!(request["tools"][0]["type"], "function");
    assert!(request["tools"].as_array().unwrap().iter().any(|tool| tool["function"]["name"] == "mcp__demo__echo"));
}

#[tokio::test]
async fn interrupted_tool_call_gets_a_synthetic_result_on_replay() {
    let server = sequence(vec![text("ok")]).await;
    let history = vec![
        Event::ToolCall { id: "c1".into(), name: "mcp__demo__echo".into(), arguments: json!({"text": "t"}) },
        Event::UserMessage { text: "alt mesaj".into() },
        Event::ToolCall { id: "c2".into(), name: "mcp__demo__echo".into(), arguments: json!({"text": "t"}) },
    ];
    run_with_history(&server, AllowAll, history, "nou", None).await.unwrap();

    let request: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    let messages = request["messages"].as_array().unwrap();
    let roles: Vec<_> = messages.iter().map(|message| message["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["system", "assistant", "tool", "user", "assistant", "tool", "user"]);
    assert_eq!(messages[2]["tool_call_id"], "c1");
    assert_eq!(messages[5]["tool_call_id"], "c2");
}

#[tokio::test]
async fn a_broken_mcp_server_is_reported_as_a_warning() {
    let server = sequence(vec![text("ok")]).await;
    let broken = vatra_core::McpServerConfig {
        name: "rupt".into(),
        command: Some("nu-exista-binar".into()),
        ..Default::default()
    };
    let mut setup = setup(&server, Arc::new(AllowAll), None, vec![], 20).await;
    setup.hub = Arc::new(McpHub::new(vec![broken], vatra_engine::ExpandVars::default()));
    let agent = EngineAgent::new(setup);
    let (sink, mut received) = tokio::sync::mpsc::unbounded_channel();
    let request = TurnRequest { files: vec![], history: vec![], prompt: "x".into(), native_session: None };
    agent.run_turn(request, sink).await.unwrap();
    assert!(matches!(received.recv().await, Some(Event::Warning { message, .. }) if message.contains("rupt")));
}

#[tokio::test]
async fn streams_deltas_then_stores_text() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Bu\"}}]}\n\n\
        data: {\"choices\":[{\"delta\":{\"content\":\"na\"}}]}\n\n\
        data: [DONE]\n\n";
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let events = run(&server, AllowAll, "salut").await.unwrap();
    let deltas: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::AssistantDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, ["Bu", "na"]);
    assert_eq!(events.last(), Some(&Event::AssistantText { text: "Buna".into() }));
}

#[tokio::test]
async fn stop_during_stream_is_clean() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Bu\"}}]}\n\n\
        data: {\"choices\":[{\"delta\":{\"content\":\"na\"}}]}\n\n\
        data: [DONE]\n\n";
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    // The receiver is gone before the first fragment, as when the user stops the turn.
    let (sink, received) = tokio::sync::mpsc::unbounded_channel();
    drop(received);
    let agent = EngineAgent::new(setup(&server, Arc::new(AllowAll), None, vec![], 20).await);
    let request = TurnRequest { files: vec![], history: vec![], prompt: "salut".into(), native_session: None };
    let error = agent.run_turn(request, sink).await.unwrap_err();
    assert!(matches!(error, AgentError::Failed(message) if message.contains("stopped")));
}

#[tokio::test]
async fn usage_event_emitted() {
    let answer = json!({
        "choices": [{"message": {"role": "assistant", "content": "hola"}}],
        "usage": {"prompt_tokens": 12, "completion_tokens": 3}
    });
    let server = sequence(vec![answer]).await;
    let events = run(&server, AllowAll, "salut").await.unwrap();
    assert!(events.contains(&Event::Usage { input_tokens: 12, output_tokens: 3, cost_usd: None }));
}

#[tokio::test]
async fn prefix_is_stable_between_steps() {
    let server = sequence(vec![tool_call("c1", "mcp__demo__echo", r#"{"text":"x"}"#), text("ok")]).await;
    run(&server, AllowAll, "salut").await.unwrap();

    let requests = server.received_requests().await.unwrap();
    let first: Value = requests[0].body_json().unwrap();
    let second: Value = requests[1].body_json().unwrap();
    assert_eq!(first["messages"][0], second["messages"][0]);
    assert_eq!(first["messages"][0]["role"], "system");
    assert_eq!(first["tools"], second["tools"]);
}

#[tokio::test]
async fn read_only_tools_run_in_parallel() {
    // Both reads wait at the barrier, so they only finish if they run at the same time.
    let mut read = fake("Read", ToolClass::Free, true);
    read.barrier = Some(Arc::new(Barrier::new(2)));
    let server =
        sequence(vec![tool_calls(&[("c1", "Read", r#"{"text":"a"}"#), ("c2", "Read", r#"{"text":"b"}"#)]), text("ok")])
            .await;
    let setup = setup(&server, Arc::new(AllowAll), None, vec![Arc::new(read)], 20).await;
    let events = tokio::time::timeout(Duration::from_secs(5), run_setup(setup, vec![], "salut"))
        .await
        .expect("the two reads waited for each other")
        .unwrap();
    assert!(results(&events).contains(&("c2", "b", false)));
}

#[tokio::test]
async fn execute_tools_run_in_order() {
    let log = Arc::new(Mutex::new(vec![]));
    let mut write = fake("Write", ToolClass::Free, false);
    write.log = Some(Arc::clone(&log));
    write.delay = Duration::from_millis(20);
    let server = sequence(vec![
        tool_calls(&[("c1", "Write", r#"{"text":"a"}"#), ("c2", "Write", r#"{"text":"b"}"#)]),
        text("ok"),
    ])
    .await;
    let setup = setup(&server, Arc::new(AllowAll), None, vec![Arc::new(write)], 20).await;
    run_setup(setup, vec![], "salut").await.unwrap();
    let entries: Vec<String> = log.lock().unwrap().clone();
    assert_eq!(entries, ["start a", "end a", "start b", "end b"]);
}

#[tokio::test]
async fn results_keep_call_order() {
    // The first call is the slow one, so it finishes last, yet the model reads its result first.
    let mut slow = fake("Slow", ToolClass::Free, true);
    slow.delay = Duration::from_millis(100);
    let quick = fake("Quick", ToolClass::Free, true);
    let server = sequence(vec![
        tool_calls(&[("c1", "Slow", r#"{"text":"first"}"#), ("c2", "Quick", r#"{"text":"second"}"#)]),
        text("ok"),
    ])
    .await;
    let setup = setup(&server, Arc::new(AllowAll), None, vec![Arc::new(slow), Arc::new(quick)], 20).await;
    run_setup(setup, vec![], "salut").await.unwrap();

    let requests = server.received_requests().await.unwrap();
    let body: Value = requests[1].body_json().unwrap();
    let tool_ids: Vec<_> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["tool_call_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(tool_ids, ["c1", "c2"]);
    assert_eq!(tool_contents(&requests[1]), ["first", "second"]);
}

#[tokio::test]
async fn unknown_tool_tells_model() {
    let server = sequence(vec![tool_call("c1", "Nope", "{}"), text("ok")]).await;
    let events = run(&server, AllowAll, "salut").await.unwrap();
    assert!(results(&events).contains(&("c1", "unknown tool: Nope", true)));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(tool_contents(&requests[1]), ["unknown tool: Nope"]);
}

#[tokio::test]
async fn alias_call_runs_canonical() {
    let bash = fake("Bash", ToolClass::Execute, false);
    let server = sequence(vec![tool_call("c1", "exec_command", r#"{"text":"ls"}"#), text("ok")]).await;
    let mut setup =
        setup(&server, Arc::new(AllowAll), None, vec![Arc::new(Fake { output: Some("ran"), ..bash })], 20).await;
    setup.registry.alias("exec_command", "Bash");
    let events = run_setup(setup, vec![], "salut").await.unwrap();
    assert!(results(&events).contains(&("c1", "ran", false)));
}

#[tokio::test]
async fn denied_tells_model() {
    let server = sequence(vec![tool_call("c1", "mcp__demo__crash", "{}"), text("ok")]).await;
    run(&server, DenyAll, "salut").await.unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(tool_contents(&requests[1]), ["The user refused this call."]);
}

#[tokio::test]
async fn chat_only_model_answers_without_tools() {
    // Each phrasing is a real answer that a server gives when it doesn't take tools.
    let phrasings = [
        "tools param requires --jinja flag",
        "this model does not support tools",
        "tools are not supported by this model",
        "unsupported tools for this model",
        "unknown parameter: tools",
    ];
    for phrasing in phrasings {
        // The server refuses any request that carries tools, and answers the rest.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wiremock::matchers::body_string_contains("\"tools\":["))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": {"message": phrasing}})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(text("sin unelte")))
            .mount(&server)
            .await;

        let events = run(&server, AllowAll, "salut").await.unwrap();
        assert!(
            events.iter().any(|event| matches!(event, Event::Warning { message, .. } if message.contains("tools"))),
            "no warning for {phrasing}"
        );
        assert_eq!(events.last(), Some(&Event::AssistantText { text: "sin unelte".into() }), "{phrasing}");
        let requests = server.received_requests().await.unwrap();
        let retry: Value = requests.last().unwrap().body_json().unwrap();
        assert!(retry.get("tools").is_none(), "{phrasing}");
    }
}

#[tokio::test]
async fn bad_tool_schema_does_not_drop_tools() {
    // A 400 about a tool's schema is an error of the request, not a model without tools.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {"message": "tools[0].function.parameters is not a valid JSON schema"}
        })))
        .mount(&server)
        .await;
    let outcome = run(&server, AllowAll, "salut").await;
    assert!(matches!(outcome, Err(AgentError::Failed(_))));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "the request was sent again without tools");
}

#[tokio::test]
async fn parallel_batch_replays_cleanly() {
    let server = sequence(vec![
        tool_calls(&[("c1", "Quick", r#"{"text":"a"}"#), ("c2", "Quick", r#"{"text":"b"}"#)]),
        text("ok"),
        text("de nou"),
    ])
    .await;
    let quick = fake("Quick", ToolClass::Free, true);
    let first_setup = setup(&server, Arc::new(AllowAll), None, vec![Arc::new(quick)], 20).await;
    let first = run_setup(first_setup, vec![], "salut").await.unwrap();

    // The journal shows both calls, then both results, each in call order.
    let journal: Vec<String> = first
        .iter()
        .filter_map(|event| match event {
            Event::ToolCall { id, .. } => Some(format!("call {id}")),
            Event::ToolResult { id, .. } => Some(format!("result {id}")),
            _ => None,
        })
        .collect();
    assert_eq!(journal, ["call c1", "call c2", "result c1", "result c2"]);

    // The next turn replays that journal: one assistant message carries both calls, and each id has one result.
    let mut history = vec![Event::UserMessage { text: "salut".into() }];
    history.extend(first);
    let second_setup = setup(&server, Arc::new(AllowAll), None, vec![], 20).await;
    run_setup(second_setup, history, "de nou").await.unwrap();

    let requests = server.received_requests().await.unwrap();
    let body: Value = requests[2].body_json().unwrap();
    let messages = body["messages"].as_array().unwrap();
    let with_calls: Vec<_> = messages
        .iter()
        .filter(|m| m["role"] == "assistant" && m["tool_calls"].as_array().is_some_and(|calls| !calls.is_empty()))
        .collect();
    assert_eq!(with_calls.len(), 1, "{messages:?}");
    assert_eq!(with_calls[0]["tool_calls"].as_array().unwrap().len(), 2);
    let answered: Vec<_> =
        messages.iter().filter(|m| m["role"] == "tool").map(|m| m["tool_call_id"].as_str().unwrap()).collect();
    assert_eq!(answered, ["c1", "c2"]);
    assert!(!tool_contents(&requests[2]).iter().any(|content| content.contains("interrupted")));
}

#[tokio::test]
async fn announced_action_without_a_tool_call_gets_one_nudge() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("result.txt");
    let arguments = json!({"file_path": file.display().to_string(), "content": "parity-ok-417"}).to_string();
    let server = sequence(vec![
        text("I have echoed the text and will now write it into result.txt.\n\n**Action:** Writing"),
        tool_call("c1", "Write", &arguments),
        text("Done."),
    ])
    .await;
    let mut engine_setup = setup(&server, Arc::new(AllowAll), None, vec![write_tool()], 20).await;
    engine_setup.context.cwd = dir.path().to_path_buf();
    engine_setup.context.project_root = dir.path().to_path_buf();

    let events = run_setup(engine_setup, vec![], "scrie rezultatul").await.unwrap();

    assert_eq!(std::fs::read_to_string(&file).unwrap(), "parity-ok-417");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    let nudged: Value = requests[1].body_json().unwrap();
    assert!(nudged.to_string().contains("did not call a tool"), "{nudged}");
    // The nudge is sent to the model but never journaled.
    assert!(!events.iter().any(|event| format!("{event:?}").contains("did not call a tool")), "{events:?}");
}

#[tokio::test]
async fn joined_future_action_is_nudged_even_after_a_false_completion_claim() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("result.txt");
    let args = json!({"file_path": file.display().to_string(), "content": "echoed"}).to_string();
    let server = sequence(vec![
        text("I have echoed the text and will now write it to result.txt. I have written it."),
        tool_call("c1", "Write", &args),
        text("Done."),
    ]).await;
    let mut config = setup(&server, Arc::new(AllowAll), None, vec![write_tool()], 10).await;
    config.context.cwd = dir.path().to_path_buf();
    config.context.project_root = dir.path().to_path_buf();
    run_setup(config, vec![], "save the echoed text").await.unwrap();
    assert_eq!(std::fs::read_to_string(file).unwrap(), "echoed");
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn explicit_planning_request_reminds_before_other_tools_run() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("output.txt");
    let args = json!({"file_path":file,"content":"done"}).to_string();
    let todo = r#"{"todos":[{"content":"Write output","status":"in_progress","activeForm":"Writing output"}]}"#;
    let server = sequence(vec![tool_call("premature", "Write", &args), tool_call("plan", "TodoWrite", todo), tool_call("write", "Write", &args), text("Done.")]).await;
    let mut config = setup(&server, Arc::new(AllowAll), None, vec![write_tool(), vatra_engine::todo_tool()], 10).await;
    config.context.cwd = dir.path().to_path_buf();
    config.context.project_root = dir.path().to_path_buf();
    let events = run_setup(config, vec![], "Create output.txt. Plan your steps first, then write it.").await.unwrap();
    assert!(events.iter().any(|e| matches!(e, Event::ToolResult { id, is_error:true, .. } if id=="premature")));
    assert!(events.iter().any(|e| matches!(e, Event::ToolResult { id, is_error:false, .. } if id=="plan")));
    assert_eq!(std::fs::read_to_string(file).unwrap(), "done");
}

#[tokio::test]
async fn planning_reminders_are_bounded_and_never_invent_tool_calls() {
    let server = repeating(text("I have a plan.")).await;
    let config = setup(&server, Arc::new(AllowAll), None, vec![vatra_engine::todo_tool()], 10).await;
    let events = run_setup(config, vec![], "Make a plan for the work.").await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    assert!(!events.iter().any(|e| matches!(e, Event::ToolCall { .. })));
}

#[tokio::test]
async fn unavailable_planning_tool_and_quoted_instructions_do_not_trigger_reminders() {
    for prompt in ["Hello.", "Summarize this note: \"Plan your steps first\".", "> Plan your steps first\nSummarize the quotation.", "```\nPlan your steps first\n```\nExplain this text."] {
        let server = sequence(vec![text("Done.")]).await;
        let config = setup(&server, Arc::new(AllowAll), None, vec![vatra_engine::todo_tool()], 10).await;
        run_setup(config, vec![], prompt).await.unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 1, "{prompt}");
    }
    let server = sequence(vec![text("Done.")]).await;
    let config = setup(&server, Arc::new(AllowAll), None, vec![], 10).await;
    run_setup(config, vec![], "Make a plan for the work.").await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn inline_quoted_sentences_and_planet_requests_are_not_planning_instructions() {
    for prompt in ["Summarize this note: \"No action needed. Plan your steps first.\"", "Summarize this note: 'No action needed. Plan your steps first.'", "Make a planet model."] {
        let server = repeating(text("Done.")).await;
        let config = setup(&server,Arc::new(AllowAll),None,vec![vatra_engine::todo_tool()],10).await;
        run_setup(config,vec![],prompt).await.unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(),1,"{prompt}");
    }
}

#[tokio::test]
async fn a_finished_answer_is_not_nudged() {
    let server = sequence(vec![text("Done. The file contains parity-ok-417.")]).await;

    run(&server, AllowAll, "salut").await.unwrap();

    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_question_to_the_person_is_not_nudged() {
    let server = sequence(vec![text("I will write it next. Which folder should I use?")]).await;

    run(&server, AllowAll, "salut").await.unwrap();

    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn nudges_reset_after_a_tool_call() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("result.txt");
    let arguments = json!({"file_path": file.display().to_string(), "content": "x"}).to_string();
    let server = sequence(vec![
        text("I will now write the file."),
        tool_call("c1", "Write", &arguments),
        text("I will now write it again."),
        tool_call("c2", "Write", &arguments),
        text("I will now write it once more."),
        text("I will now write it once more, really."),
        text("I will now write it once more, truly."),
    ])
    .await;
    let mut engine_setup = setup(&server, Arc::new(AllowAll), None, vec![write_tool()], 20).await;
    engine_setup.context.cwd = dir.path().to_path_buf();
    engine_setup.context.project_root = dir.path().to_path_buf();

    run_setup(engine_setup, vec![], "scrie").await.unwrap();

    // Each announcement after a tool call is nudged again; the turn ends after two nudges with no tool call.
    assert_eq!(server.received_requests().await.unwrap().len(), 7);
}

#[tokio::test]
async fn nudging_stops_after_two_tries() {
    let server = repeating(text("I will now write the file.")).await;

    let events = run(&server, AllowAll, "salut").await.unwrap();

    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    assert_eq!(events.last(), Some(&Event::AssistantText { text: "I will now write the file.".into() }));
}

#[tokio::test]
async fn a_model_with_a_small_window_gets_a_short_prompt_and_only_the_core_tools() {
    use vatra_engine::model::{ModelConfig, OpenAiDriver};
    use vatra_engine::{glob_tool, task_stop_tool, todo_tool};
    let server = sequence(vec![text("ok")]).await;
    let mut setup = setup(&server, Arc::new(AllowAll), None, vec![todo_tool(), glob_tool(), task_stop_tool()], 5).await;
    setup.driver = Arc::new(OpenAiDriver::new(ModelConfig {
        base_url: server.uri(),
        model: "m".into(),
        api_key: None,
        context_window: Some(8_192),
    }));
    run_setup(setup, vec![], "salut").await.unwrap();
    let request: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    let names: Vec<String> = request["tools"].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap().to_string()).collect();
    assert!(names.contains(&"TodoWrite".to_string()) && names.contains(&"Glob".to_string()), "{names:?}");
    assert!(!names.contains(&"TaskStop".to_string()), "a tool outside the core set is left out: {names:?}");
    assert!(names.iter().any(|n| n.starts_with("mcp__")), "tools of MCP servers the person turned on stay");
    let system = request["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("Never say a file was changed unless"));
    assert!(!system.contains("# Memory"), "the long sections are left out");
}

#[tokio::test]
async fn a_large_window_keeps_every_tool_and_the_full_prompt() {
    use vatra_engine::{glob_tool, task_stop_tool, todo_tool};
    let server = sequence(vec![text("ok")]).await;
    let setup = setup(&server, Arc::new(AllowAll), None, vec![todo_tool(), glob_tool(), task_stop_tool()], 5).await;
    run_setup(setup, vec![], "salut").await.unwrap();
    let request: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    let names: Vec<String> = request["tools"].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap().to_string()).collect();
    assert!(names.contains(&"TaskStop".to_string()));
    assert!(request["messages"][0]["content"].as_str().unwrap().contains("# Memory"));
}
