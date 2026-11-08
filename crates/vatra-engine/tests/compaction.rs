mod common;

use std::sync::Arc;

use std::fs;

use async_trait::async_trait;
use common::*;
use serde_json::{Value, json};
use tempfile::TempDir;
use vatra_core::{AgentError, Event};
use vatra_engine::model::{ModelConfig, OpenAiDriver};
use vatra_engine::{
    AllowAll, EngineSetup, Tool, ToolClass, ToolContext, ToolDef, ToolOutput, compact, load_instructions,
};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The window of the model in these tests. Compaction starts at 80 percent of it.
const WINDOW: u64 = 20_000;

/// A setup whose model has a window of `window` tokens, with the given tools.
async fn window_setup(server: &MockServer, window: u64, tools: Vec<Arc<dyn Tool>>) -> EngineSetup {
    let mut setup = setup(server, Arc::new(AllowAll), None, tools, 20).await;
    setup.driver = Arc::new(OpenAiDriver::new(ModelConfig {
        base_url: server.uri(),
        model: "m".into(),
        api_key: None,
        context_window: Some(window),
    }));
    setup
}

/// Mounts one answer for each response, in order. A response is given as its status code and body.
async fn answers(responses: Vec<(u16, Value)>) -> MockServer {
    let server = MockServer::start().await;
    for (code, body) in responses {
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(code).set_body_json(body))
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }
    server
}

fn roles(request: &Value) -> Vec<&str> {
    request["messages"].as_array().unwrap().iter().map(|message| message["role"].as_str().unwrap()).collect()
}

/// A tool whose description is `size` characters long, so its definition is big.
struct Wordy {
    size: usize,
    name: &'static str,
}

#[async_trait]
impl Tool for Wordy {
    fn def(&self) -> ToolDef {
        ToolDef { name: self.name.into(), description: "x".repeat(self.size), parameters: json!({"type": "object"}) }
    }

    fn class(&self, _arguments: &Value, _context: &ToolContext) -> ToolClass {
        ToolClass::Free
    }

    fn parallel_safe(&self) -> bool {
        false
    }

    async fn call(&self, _arguments: Value, _context: &ToolContext) -> ToolOutput {
        ToolOutput::ok("done")
    }
}

/// A tool that answers with a long output.
struct Verbose {
    size: usize,
}

#[async_trait]
impl Tool for Verbose {
    fn def(&self) -> ToolDef {
        ToolDef { name: "Read".into(), description: "Reads.".into(), parameters: json!({"type": "object"}) }
    }

    fn class(&self, _arguments: &Value, _context: &ToolContext) -> ToolClass {
        ToolClass::Free
    }

    fn parallel_safe(&self) -> bool {
        false
    }

    async fn call(&self, _arguments: Value, _context: &ToolContext) -> ToolOutput {
        ToolOutput::ok("z".repeat(self.size))
    }
}

#[tokio::test]
async fn triggers_at_80_percent() {
    // The window is 20,000 tokens, so compaction starts at 16,000. The system prompt takes a small part of it.
    let below = answers(vec![(200, text("ok"))]).await;
    let events = run_setup(
        window_setup(&below, WINDOW, vec![]).await,
        vec![Event::UserMessage { text: "a".repeat(50_000) }],
        "nou",
    )
    .await
    .unwrap();
    assert!(!events.iter().any(|event| matches!(event, Event::Compacted { .. })));
    assert_eq!(below.received_requests().await.unwrap().len(), 1);

    let above = answers(vec![(200, text("rezumat")), (200, text("ok"))]).await;
    let events = run_setup(
        window_setup(&above, WINDOW, vec![]).await,
        vec![Event::UserMessage { text: "a".repeat(70_000) }],
        "nou",
    )
    .await
    .unwrap();
    assert!(events.contains(&Event::Compacted { summary: "rezumat".into() }));
    let requests = above.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2, "the summary is asked before the step");
    let step: Value = requests[1].body_json().unwrap();
    assert!(step.to_string().contains("rezumat"));
}

#[tokio::test]
async fn replays_from_last_compacted() {
    let server = sequence(vec![text("ok")]).await;
    let history = vec![
        Event::UserMessage { text: "primul".into() },
        Event::AssistantText { text: "vechi".into() },
        Event::Compacted { summary: "S1".into() },
        Event::UserMessage { text: "dupa S1".into() },
        Event::Compacted { summary: "S2".into() },
        Event::UserMessage { text: "recent".into() },
    ];
    run_with_history(&server, AllowAll, history, "nou", None).await.unwrap();

    let request: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    assert_eq!(roles(&request), ["system", "user", "user", "user"]);
    let conversation = Value::Array(request["messages"].as_array().unwrap()[1..].to_vec()).to_string();
    assert!(conversation.contains("S2"));
    assert!(conversation.contains("recent"));
    assert!(!conversation.contains("S1") && !conversation.contains("primul") && !conversation.contains("vechi"));
}

#[tokio::test]
async fn summary_comes_first_then_current_request() {
    let server = sequence(vec![text("ok")]).await;
    let history =
        vec![Event::UserMessage { text: "inainte".into() }, Event::Compacted { summary: "rezumat-test".into() }];
    run_with_history(&server, AllowAll, history, "nou", None).await.unwrap();

    let request: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    assert_eq!(roles(&request), ["system", "user", "user"]);
    assert!(request["messages"][1]["content"].as_str().unwrap().contains("rezumat-test"));
    assert_eq!(request["messages"][2]["content"], "nou");
}

#[tokio::test]
async fn reattaches_invoked_skills_within_limit() {
    // Each skill keeps at most 5,000 tokens (20,000 characters), and all of them at most 25,000 (100,000).
    let server = answers(vec![(200, text("RESUMAT"))]).await;
    let client = window_setup(&server, WINDOW, vec![]).await.driver;
    let skills: Vec<String> = "abcdef".chars().map(|letter| letter.to_string().repeat(30_000)).collect();
    let summary = compact(client.as_ref(), &[], &skills).await.unwrap();

    assert!(summary.starts_with("RESUMAT"));
    for letter in "abcde".chars() {
        assert_eq!(summary.matches(letter).count(), 20_000, "{letter}");
    }
    assert_eq!(summary.matches('f').count(), 0, "the total limit is reached by the fifth skill");
}

#[tokio::test]
async fn failed_summary_trims_and_warns() {
    // Two results of 10,000 tokens each are over the 16,000 that compaction starts at. The summary fails.
    let server = answers(vec![(500, json!({"error": "down"})), (200, text("ok"))]).await;
    let history = vec![
        Event::ToolCall { id: "c1".into(), name: "Read".into(), arguments: json!({}) },
        Event::ToolResult { id: "c1".into(), output: "x".repeat(40_000), is_error: false },
        Event::ToolCall { id: "c2".into(), name: "Read".into(), arguments: json!({}) },
        Event::ToolResult { id: "c2".into(), output: "y".repeat(40_000), is_error: false },
    ];
    let setup = window_setup(&server, WINDOW, vec![]).await;
    let events = run_setup(setup, history, "nou").await.unwrap();

    assert!(events.iter().any(|event| matches!(event, Event::Warning { message, .. } if message.contains("summar"))));
    let requests = server.received_requests().await.unwrap();
    let step: Value = requests[1].body_json().unwrap();
    let contents: Vec<&str> = step["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["content"].as_str().unwrap())
        .collect();
    assert_eq!(contents.len(), 2);
    assert!(contents[0].len() < 1_000, "the oldest result is cut");
    assert_eq!(contents[1].len(), 40_000, "the newest result is kept");
}

#[tokio::test]
async fn context_too_large_error_compacts_once_then_retries() {
    let too_large = json!({"error": {"message": "This request exceeds the context length"}});
    let server = answers(vec![(400, too_large.clone()), (200, text("resumat")), (200, text("ok"))]).await;
    let events = run_setup(window_setup(&server, WINDOW, vec![]).await, vec![], "nou").await.unwrap();

    assert!(events.contains(&Event::Compacted { summary: "resumat".into() }));
    assert_eq!(events.last(), Some(&Event::AssistantText { text: "ok".into() }));
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn context_too_large_after_compaction_is_a_clear_failure() {
    let too_large = json!({"error": {"message": "This request exceeds the context length"}});
    let server = answers(vec![(400, too_large.clone()), (200, text("resumat")), (400, too_large)]).await;
    let error = run_setup(window_setup(&server, WINDOW, vec![]).await, vec![], "nou").await.unwrap_err();
    let AgentError::Failed(message) = error else { panic!("expected a failed turn") };
    assert!(message.contains("compact"), "{message}");
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn result_bigger_than_window_is_truncated_with_notice() {
    // The window is 8,000 tokens. A 40,000 character output is 10,000 tokens, so it is cut to half the window.
    let server = sequence(vec![tool_call("c1", "Read", "{}"), text("ok")]).await;
    let setup = window_setup(&server, 8_000, vec![Arc::new(Verbose { size: 40_000 })]).await;
    let events = run_setup(setup, vec![], "nou").await.unwrap();

    let output = events
        .iter()
        .find_map(|event| match event {
            Event::ToolResult { output, .. } => Some(output.clone()),
            _ => None,
        })
        .unwrap();
    assert!(output.contains("cut"), "{output}");
    assert!(output.chars().count() < 20_000, "{}", output.chars().count());
    let requests = server.received_requests().await.unwrap();
    assert!(tool_contents(&requests[1])[0].contains("cut"));
}

#[tokio::test]
async fn tools_bigger_than_window_gives_clear_failure() {
    // The tool definition alone is about 5,000 tokens, more than the 1,000 token window.
    let server = sequence(vec![text("ok")]).await;
    let setup = window_setup(&server, 1_000, vec![Arc::new(Wordy { size: 20_000, name: "Grep" })]).await;
    let error = run_setup(setup, vec![], "nou").await.unwrap_err();
    let AgentError::Failed(message) = error else { panic!("expected a failed turn") };
    assert!(message.contains("tools"), "{message}");
    assert!(!message.contains("answered"), "the server's raw error reached the user: {message}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn manual_compact_prompt() {
    let server = sequence(vec![text("resumat manual")]).await;
    let history = vec![Event::UserMessage { text: "primul".into() }, Event::AssistantText { text: "raspuns".into() }];
    let events = run_with_history(&server, AllowAll, history, "/compact", None).await.unwrap();

    assert!(events.contains(&Event::Compacted { summary: "resumat manual".into() }));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "/compact asks for the summary and no step");
    let request: Value = requests[0].body_json().unwrap();
    assert!(request["messages"].to_string().contains("primul"));
    assert!(request["tools"].as_array().is_none_or(|tools| tools.is_empty()));
}

#[tokio::test]
async fn rules_stay_active_after_compaction() {
    let dir = TempDir::new().unwrap();
    let rule = dir.path().join(".claude").join("rules").join("rust.md");
    fs::create_dir_all(rule.parent().unwrap()).unwrap();
    fs::write(&rule, "---\npaths: [\"**/*.rs\"]\n---\nRUST-RULE").unwrap();
    let server = sequence(vec![text("resumat"), text("ok")]).await;

    let mut compacting = setup(&server, Arc::new(AllowAll), None, vec![], 20).await;
    compacting.instructions = load_instructions(dir.path(), dir.path(), &[]);
    let label = compacting.instructions.deferred[0].label.clone();
    let rule_text = compacting.instructions.deferred[0].text.clone();
    // The rule joined the context earlier in this session.
    let mut journal = vec![
        Event::UserMessage { text: "primul".into() },
        Event::RuleAdded { label: label.clone(), text: rule_text.clone() },
    ];
    let events = run_setup(compacting, journal.clone(), "/compact").await.unwrap();
    let compacted = events.iter().position(|event| matches!(event, Event::Compacted { .. })).unwrap();
    assert!(events[compacted + 1..].contains(&Event::RuleAdded { label: label.clone(), text: rule_text.clone() }));

    // The next turn replays the summary and the rule once, and the rule does not join again.
    journal.push(Event::UserMessage { text: "/compact".into() });
    journal.extend(events);
    let mut next = setup(&server, Arc::new(AllowAll), None, vec![], 20).await;
    next.instructions = load_instructions(dir.path(), dir.path(), &[]);
    run_setup(next, journal, "nou").await.unwrap();
    let request: Value = server.received_requests().await.unwrap()[1].body_json().unwrap();
    let messages = request["messages"].as_array().unwrap();
    assert_eq!(messages.iter().filter(|message| message["content"] == rule_text).count(), 1);
}

#[tokio::test]
async fn tools_at_90_percent_summarize_at_most_once() {
    // The tool definitions alone are 90 percent of the window, so the context stays above the threshold.
    let server = answers(vec![
        (200, text("resumat")),
        (200, tool_call("c1", "Wordy", "{}")),
        (200, tool_call("c2", "Wordy", "{}")),
        (200, text("ok")),
    ])
    .await;
    let setup = window_setup(&server, WINDOW, vec![Arc::new(Wordy { size: 72_000, name: "Wordy" })]).await;
    let events = run_setup(setup, vec![], "nou").await.unwrap();

    assert_eq!(events.last(), Some(&Event::AssistantText { text: "ok".into() }));
    assert_eq!(events.iter().filter(|event| matches!(event, Event::Compacted { .. })).count(), 1);
    // The summary request has no tools, and the steps have them.
    let summaries = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| {
            let body: Value = request.body_json().unwrap();
            body["tools"].as_array().is_none_or(|tools| tools.is_empty())
        })
        .count();
    assert_eq!(summaries, 1);
}

#[tokio::test]
async fn failed_summary_without_room_says_so() {
    // The prompt alone is over the window, and there are no tool results to cut. Both summaries fail.
    let too_large = json!({"error": {"message": "This request exceeds the context length"}});
    let server =
        answers(vec![(500, json!({"error": "down"})), (400, too_large), (500, json!({"error": "down"}))]).await;
    let history = vec![Event::UserMessage { text: "a".repeat(84_000) }];
    let error = run_setup(window_setup(&server, WINDOW, vec![]).await, history, "nou").await.unwrap_err();

    let AgentError::Failed(message) = error else { panic!("expected a failed turn") };
    assert!(message.contains("summarized"), "{message}");
    assert!(!message.contains("even after"), "{message}");
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}
