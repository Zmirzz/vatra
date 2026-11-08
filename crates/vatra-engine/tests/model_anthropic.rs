use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vatra_engine::ToolDef;
use vatra_engine::model::{
    AnthropicDriver, Completion, ContextWindow, ImagePart, Message, ModelConfig, ModelDriver, ModelError, ModelRequest,
    RawToolCall, TokenUsage, with_retries,
};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn driver(server: &MockServer) -> Arc<dyn ModelDriver> {
    driver_with_window(server, Some(8_192))
}

fn driver_with_window(server: &MockServer, context_window: Option<u64>) -> Arc<dyn ModelDriver> {
    Arc::new(AnthropicDriver::new(ModelConfig {
        base_url: server.uri(),
        model: "claude-test".into(),
        api_key: Some("sk-test-key".into()),
        context_window,
    }))
}

fn request(prompt: &str) -> ModelRequest {
    ModelRequest { messages: vec![Message::User { text: prompt.into(), images: vec![] }], tools: vec![], effort: None }
}

/// The events as `text/event-stream`, each with its `event:` line, as Anthropic sends them.
fn stream(events: &[Value]) -> ResponseTemplate {
    let mut body = String::new();
    for event in events {
        body += &format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap());
    }
    ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(body)
}

async fn serve(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST")).and(path("/v1/messages")).respond_with(response).mount(server).await;
}

/// Runs one stream and returns its result with every fragment `on_text` received.
async fn run(driver: &dyn ModelDriver, request: &ModelRequest) -> (Result<Completion, ModelError>, Vec<String>) {
    let mut fragments = Vec::new();
    let result = driver.stream(request, &mut |text: &str| fragments.push(text.to_string())).await;
    (result, fragments)
}

/// The body of the last request the server got.
async fn sent(server: &MockServer) -> Value {
    server.received_requests().await.unwrap().last().unwrap().body_json().unwrap()
}

fn image() -> ImagePart {
    ImagePart { media_type: "image/png".into(), base64: "AAAA".into() }
}

fn call(id: &str) -> RawToolCall {
    RawToolCall { id: id.into(), name: "mcp__demo__echo".into(), arguments: r#"{"text":"t"}"#.into() }
}

fn tool(name: &str) -> ToolDef {
    ToolDef { name: name.into(), description: format!("{name} description"), parameters: json!({"type": "object"}) }
}

fn message_start(input: u64) -> Value {
    json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant",
        "content": [], "model": "claude-test", "stop_reason": null, "stop_sequence": null,
        "usage": {"input_tokens": input, "output_tokens": 1}}})
}

fn text_start(index: u64) -> Value {
    json!({"type": "content_block_start", "index": index, "content_block": {"type": "text", "text": ""}})
}

fn text_delta(index: u64, text: &str) -> Value {
    json!({"type": "content_block_delta", "index": index, "delta": {"type": "text_delta", "text": text}})
}

fn tool_start(index: u64, id: &str, name: &str) -> Value {
    json!({"type": "content_block_start", "index": index,
        "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}}})
}

fn json_delta(index: u64, partial_json: &str) -> Value {
    json!({"type": "content_block_delta", "index": index,
        "delta": {"type": "input_json_delta", "partial_json": partial_json}})
}

fn block_stop(index: u64) -> Value {
    json!({"type": "content_block_stop", "index": index})
}

fn message_delta(stop_reason: &str, output: u64) -> Value {
    json!({"type": "message_delta", "delta": {"stop_reason": stop_reason, "stop_sequence": null},
        "usage": {"output_tokens": output}})
}

fn message_stop() -> Value {
    json!({"type": "message_stop"})
}

/// A whole answer of one text block.
fn answer(text: &str) -> Vec<Value> {
    vec![
        message_start(5),
        text_start(0),
        text_delta(0, text),
        block_stop(0),
        message_delta("end_turn", 2),
        message_stop(),
    ]
}

#[tokio::test]
async fn maps_system_to_top_level() {
    let server = MockServer::start().await;
    serve(&server, stream(&answer("ok"))).await;
    let request = ModelRequest {
        messages: vec![
            Message::System { text: "sistem".into() },
            Message::User { text: "salut".into(), images: vec![] },
        ],
        tools: vec![],
        effort: None,
    };
    let driver = driver(&server);
    run(driver.as_ref(), &request).await.0.unwrap();

    let body = sent(&server).await;
    assert_eq!(body["system"][0]["text"], "sistem");
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"][0], json!({"type": "text", "text": "salut"}));
}

#[tokio::test]
async fn tool_result_becomes_user_block() {
    let server = MockServer::start().await;
    serve(&server, stream(&answer("gata"))).await;
    let request = ModelRequest {
        messages: vec![
            Message::User { text: "uite".into(), images: vec![] },
            Message::Assistant { text: None, tool_calls: vec![call("toolu_1")], opaque: vec![] },
            Message::Tool { id: "toolu_1".into(), content: "o poză".into(), is_error: false, images: vec![] },
        ],
        tools: vec![],
        effort: None,
    };
    let driver = driver(&server);
    run(driver.as_ref(), &request).await.0.unwrap();

    let body = sent(&server).await;
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(
        messages[1],
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_1", "name": "mcp__demo__echo", "input": {"text": "t"}}
        ]})
    );
    assert_eq!(
        messages[2],
        json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_1", "content": "o poză", "is_error": false}
        ]})
    );
}

#[tokio::test]
async fn consecutive_tool_results_merge_into_one_user_message() {
    let server = MockServer::start().await;
    serve(&server, stream(&answer("gata"))).await;
    let request = ModelRequest {
        messages: vec![
            Message::User { text: "uite".into(), images: vec![] },
            Message::Assistant { text: None, tool_calls: vec![call("c1"), call("c2")], opaque: vec![] },
            Message::Tool { id: "c1".into(), content: "bine".into(), is_error: false, images: vec![] },
            Message::Tool { id: "c2".into(), content: "eroare".into(), is_error: true, images: vec![] },
            Message::User { text: "și acum?".into(), images: vec![] },
        ],
        tools: vec![],
        effort: None,
    };
    let driver = driver(&server);
    run(driver.as_ref(), &request).await.0.unwrap();

    let body = sent(&server).await;
    let messages = body["messages"].as_array().unwrap();
    let roles: Vec<_> = messages.iter().map(|message| message["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant", "user"]);
    let results = messages[2]["content"].as_array().unwrap();
    let kinds: Vec<_> = results.iter().map(|block| block["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["tool_result", "tool_result", "text"]);
    assert_eq!(results[0]["tool_use_id"], "c1");
    assert_eq!(results[1]["tool_use_id"], "c2");
    assert_eq!(results[1]["is_error"], true);
}

#[tokio::test]
async fn streams_text_and_tool_input_json_delta() {
    let server = MockServer::start().await;
    serve(
        &server,
        stream(&[
            message_start(10),
            text_start(0),
            text_delta(0, "Caut"),
            text_delta(0, " fișierul"),
            block_stop(0),
            tool_start(1, "toolu_9", "mcp__demo__echo"),
            json_delta(1, "{\"te"),
            json_delta(1, "xt\": \"ola\"}"),
            block_stop(1),
            message_delta("tool_use", 12),
            message_stop(),
        ]),
    )
    .await;
    let driver = driver(&server);
    let (result, fragments) = run(driver.as_ref(), &request("salut")).await;
    assert_eq!(fragments, ["Caut", " fișierul"]);
    let completion = result.unwrap();
    assert_eq!(completion.text.as_deref(), Some("Caut fișierul"));
    assert_eq!(
        completion.tool_calls,
        [RawToolCall { id: "toolu_9".into(), name: "mcp__demo__echo".into(), arguments: r#"{"text": "ola"}"#.into() }]
    );
}

#[tokio::test]
async fn cache_control_on_last_system_and_tool_blocks() {
    let server = MockServer::start().await;
    serve(&server, stream(&answer("ok"))).await;
    let request = ModelRequest {
        messages: vec![
            Message::System { text: "unu".into() },
            Message::System { text: "doi".into() },
            Message::User { text: "salut".into(), images: vec![] },
        ],
        tools: vec![tool("a"), tool("b")],
        effort: None,
    };
    let driver = driver(&server);
    run(driver.as_ref(), &request).await.0.unwrap();

    let body = sent(&server).await;
    assert!(body["system"][0].get("cache_control").is_none());
    assert_eq!(body["system"][1]["cache_control"], json!({"type": "ephemeral"}));
    assert!(body["tools"][0].get("cache_control").is_none());
    assert_eq!(body["tools"][1]["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(body["tools"][1]["name"], "b");
    assert_eq!(body["tools"][1]["input_schema"], json!({"type": "object"}));
    assert_eq!(body.to_string().matches("cache_control").count(), 2);
}

#[tokio::test]
async fn thinking_blocks_are_preserved_on_replay_within_turn() {
    let server = MockServer::start().await;
    serve(
        &server,
        stream(&[
            message_start(10),
            json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
            json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "Trebuie "}}),
            json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "să caut."}}),
            json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "signature_delta", "signature": "sig-abc"}}),
            block_stop(0),
            tool_start(1, "toolu_2", "mcp__demo__echo"),
            json_delta(1, "{\"text\":\"x\"}"),
            block_stop(1),
            message_delta("tool_use", 12),
            message_stop(),
        ]),
    )
    .await;
    let driver = driver(&server);
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    let completion = result.unwrap();
    let thinking = json!({"type": "thinking", "thinking": "Trebuie să caut.", "signature": "sig-abc"});
    assert_eq!(completion.opaque.len(), 1);
    assert_eq!(completion.opaque[0], thinking);

    let replay = ModelRequest {
        messages: vec![
            Message::User { text: "salut".into(), images: vec![] },
            Message::Assistant {
                text: None,
                tool_calls: completion.tool_calls.clone(),
                opaque: completion.opaque.clone(),
            },
            Message::Tool { id: "toolu_2".into(), content: "ok".into(), is_error: false, images: vec![] },
        ],
        tools: vec![],
        effort: None,
    };
    run(driver.as_ref(), &replay).await.0.unwrap();

    let body = sent(&server).await;
    assert_eq!(body["messages"][1]["content"][0], thinking);
    assert_eq!(body["messages"][1]["content"][1]["type"], "tool_use");
}

#[tokio::test]
async fn usage_reads_cache_read_input_tokens() {
    let server = MockServer::start().await;
    serve(
        &server,
        stream(&[
            json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant",
                "content": [], "model": "claude-test", "stop_reason": null, "stop_sequence": null,
                "usage": {"input_tokens": 20, "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 100, "output_tokens": 1}}}),
            text_start(0),
            text_delta(0, "ok"),
            block_stop(0),
            message_delta("end_turn", 7),
            message_stop(),
        ]),
    )
    .await;
    let driver = driver(&server);
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert_eq!(result.unwrap().usage, TokenUsage { input: 120, output: 7, cached_input: 100 });
}

#[tokio::test]
async fn overloaded_529_is_retried_by_wrapper() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(529).set_body_json(
                json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
            ),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    serve(&server, stream(&answer("ok"))).await;

    let driver = with_retries(driver(&server));
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert_eq!(result.unwrap().text.as_deref(), Some("ok"));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn images_become_base64_source() {
    let server = MockServer::start().await;
    serve(&server, stream(&answer("ok"))).await;
    let request = ModelRequest {
        messages: vec![
            Message::User { text: "uite".into(), images: vec![image()] },
            Message::Assistant { text: None, tool_calls: vec![call("c1")], opaque: vec![] },
            Message::Tool { id: "c1".into(), content: "o poză".into(), is_error: false, images: vec![image()] },
        ],
        tools: vec![],
        effort: None,
    };
    let driver = driver(&server);
    run(driver.as_ref(), &request).await.0.unwrap();

    let body = sent(&server).await;
    let source = json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}});
    assert_eq!(body["messages"][0]["content"][1], source);
    assert_eq!(body["messages"][2]["content"][0]["content"][0], json!({"type": "text", "text": "o poză"}));
    assert_eq!(body["messages"][2]["content"][0]["content"][1], source);
}

#[tokio::test]
async fn sends_key_version_and_streaming_flag() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-test-key"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(stream(&answer("ok")))
        .mount(&server)
        .await;
    let driver = driver(&server);
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert_eq!(result.unwrap().text.as_deref(), Some("ok"));

    let body = sent(&server).await;
    assert_eq!(body["stream"], true);
    assert_eq!(body["model"], "claude-test");
    assert_eq!(body["max_tokens"], 16_000);
}

#[tokio::test]
async fn retries_429_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "2"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    serve(&server, stream(&answer("ok"))).await;

    let driver = with_retries(driver(&server));
    let started = Instant::now();
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert_eq!(result.unwrap().text.as_deref(), Some("ok"));
    assert!(started.elapsed() >= Duration::from_secs(2));
}

#[tokio::test]
async fn prompt_too_long_is_context_overflow() {
    let server = MockServer::start().await;
    serve(
        &server,
        ResponseTemplate::new(400).set_body_json(json!({"type": "error", "error": {
            "type": "invalid_request_error", "message": "prompt is too long: 209234 tokens > 200000 maximum"
        }})),
    )
    .await;
    let driver = driver(&server);
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert!(matches!(result, Err(ModelError::ContextTooLarge(_))));
}

#[tokio::test]
async fn stream_error_event_is_retryable_when_overloaded() {
    let server = MockServer::start().await;
    serve(&server, stream(&[json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}})]))
        .await;
    let driver = driver(&server);
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert!(matches!(result, Err(ModelError::Server(message)) if message.contains("Overloaded")));
}

#[tokio::test]
async fn truncated_stream_is_an_error() {
    let server = MockServer::start().await;
    serve(&server, stream(&[message_start(5), text_start(0), text_delta(0, "a meda")])).await;
    let driver = driver(&server);
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert!(matches!(result, Err(ModelError::Other(_))));
}

#[tokio::test]
async fn api_key_stays_out_of_errors() {
    let server = MockServer::start().await;
    serve(&server, ResponseTemplate::new(401).set_body_string("boom")).await;
    let driver = AnthropicDriver::new(ModelConfig {
        base_url: server.uri(),
        model: "claude-test".into(),
        api_key: Some("sk-secret-key".into()),
        context_window: Some(8_192),
    });
    let (result, _) = run(&driver, &request("salut")).await;
    let error = result.unwrap_err();
    assert!(error.to_string().contains("boom"));
    assert!(!format!("{error:?}").contains("sk-secret-key"));
}

#[tokio::test]
async fn window_is_configured_or_an_estimate() {
    let server = MockServer::start().await;
    assert_eq!(
        driver_with_window(&server, Some(32_000)).context_window(),
        ContextWindow { tokens: 32_000, estimated: false }
    );
    assert_eq!(driver_with_window(&server, None).context_window(), ContextWindow { tokens: 200_000, estimated: true });
}

#[tokio::test]
async fn effort_maps_to_output_config() {
    let server = MockServer::start().await;
    serve(&server, stream(&answer("ok"))).await;
    let driver = driver(&server);
    let with_effort = |effort: Option<&str>| ModelRequest {
        messages: vec![Message::User { text: "salut".into(), images: vec![] }],
        tools: vec![],
        effort: effort.map(String::from),
    };

    run(driver.as_ref(), &with_effort(Some("high"))).await.0.unwrap();
    let body = sent(&server).await;
    assert_eq!(body["output_config"], json!({"effort": "high"}));
    assert!(body.get("thinking").is_none());

    run(driver.as_ref(), &with_effort(None)).await.0.unwrap();
    assert!(sent(&server).await.get("output_config").is_none());

    run(driver.as_ref(), &with_effort(Some("extreme"))).await.0.unwrap();
    assert!(sent(&server).await.get("output_config").is_none());
}
