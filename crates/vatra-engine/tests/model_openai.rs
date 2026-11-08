use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};
use vatra_engine::model::{
    Completion, ContextWindow, ImagePart, Message, ModelConfig, ModelDriver, ModelError, ModelRequest, OpenAiDriver,
    RawToolCall, TokenUsage, with_retries,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn driver(server: &MockServer) -> Arc<dyn ModelDriver> {
    driver_with_window(server, Some(8_192))
}

fn driver_with_window(server: &MockServer, context_window: Option<u64>) -> Arc<dyn ModelDriver> {
    Arc::new(OpenAiDriver::new(ModelConfig {
        base_url: server.uri(),
        model: "m".into(),
        api_key: None,
        context_window,
    }))
}

fn request(prompt: &str) -> ModelRequest {
    ModelRequest { messages: vec![Message::User { text: prompt.into(), images: vec![] }], tools: vec![], effort: None }
}

/// The chunks as `text/event-stream`, one `data:` event each, then `[DONE]`.
fn stream(chunks: &[Value]) -> ResponseTemplate {
    let mut body = String::new();
    for chunk in chunks {
        body += &format!("data: {chunk}\n\n");
    }
    body += "data: [DONE]\n\n";
    ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(body)
}

async fn serve(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST")).and(path("/chat/completions")).respond_with(response).mount(server).await;
}

/// Runs one stream and returns its result with every fragment `on_text` received.
async fn run(driver: &dyn ModelDriver, request: &ModelRequest) -> (Result<Completion, ModelError>, Vec<String>) {
    let mut fragments = Vec::new();
    let result = driver.stream(request, &mut |text: &str| fragments.push(text.to_string())).await;
    (result, fragments)
}

fn image() -> ImagePart {
    ImagePart { media_type: "image/png".into(), base64: "AAAA".into() }
}

fn call(id: &str) -> RawToolCall {
    RawToolCall { id: id.into(), name: "mcp__demo__echo".into(), arguments: r#"{"text":"t"}"#.into() }
}

#[tokio::test]
async fn streams_text_in_order() {
    let server = MockServer::start().await;
    serve(
        &server,
        stream(&[
            json!({"choices": [{"delta": {"role": "assistant", "content": "Bu"}}]}),
            json!({"choices": [{"delta": {"content": "na"}}]}),
            json!({"choices": [{"delta": {"content": " lume"}}]}),
            json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}),
        ]),
    )
    .await;
    let driver = driver(&server);
    let (result, fragments) = run(driver.as_ref(), &request("salut")).await;
    assert_eq!(fragments, ["Bu", "na", " lume"]);
    assert_eq!(result.unwrap().text.as_deref(), Some("Buna lume"));
}

#[tokio::test]
async fn assembles_tool_call_from_fragments() {
    let server = MockServer::start().await;
    serve(
        &server,
        stream(&[
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_1", "type": "function",
                "function": {"name": "mcp__demo__echo", "arguments": "{\"te"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "xt\": \"o"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "la\"}"}}]}}]}),
        ]),
    )
    .await;
    let driver = driver(&server);
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    let completion = result.unwrap();
    assert_eq!(completion.text, None);
    assert_eq!(
        completion.tool_calls,
        [RawToolCall { id: "call_1".into(), name: "mcp__demo__echo".into(), arguments: r#"{"text": "ola"}"#.into() }]
    );
}

#[tokio::test]
async fn two_parallel_calls_keep_order() {
    let server = MockServer::start().await;
    serve(
        &server,
        stream(&[
            json!({"choices": [{"delta": {"tool_calls": [{"index": 1, "id": "call_b", "type": "function",
                "function": {"name": "mcp__demo__echo", "arguments": "{\"text\":"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_a", "type": "function",
                "function": {"name": "mcp__demo__crash", "arguments": "{}"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 1, "function": {"arguments": "\"x\"}"}}]}}]}),
        ]),
    )
    .await;
    let driver = driver(&server);
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert_eq!(
        result.unwrap().tool_calls,
        [
            RawToolCall { id: "call_a".into(), name: "mcp__demo__crash".into(), arguments: "{}".into() },
            RawToolCall { id: "call_b".into(), name: "mcp__demo__echo".into(), arguments: r#"{"text":"x"}"#.into() },
        ]
    );
}

#[tokio::test]
async fn object_arguments_from_local_server() {
    let server = MockServer::start().await;
    let response = json!({"choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
        {"id": "c1", "type": "function", "function": {"name": "mcp__demo__echo", "arguments": {"text": "x"}}}
    ]}}]});
    serve(&server, ResponseTemplate::new(200).set_body_json(response)).await;
    let driver = driver(&server);
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert_eq!(result.unwrap().tool_calls[0].arguments, r#"{"text":"x"}"#);
}

#[tokio::test]
async fn object_arguments_in_a_stream_are_kept() {
    let server = MockServer::start().await;
    serve(
        &server,
        stream(&[json!({"choices": [{"delta": {"tool_calls": [
            {"index": 0, "id": "c1", "type": "function", "function": {"name": "mcp__demo__echo", "arguments": {"text": "x"}}}
        ]}}]})]),
    )
    .await;
    let driver = driver(&server);
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert_eq!(result.unwrap().tool_calls[0].arguments, r#"{"text":"x"}"#);
}

#[tokio::test]
async fn non_streaming_fallback() {
    let server = MockServer::start().await;
    serve(
        &server,
        ResponseTemplate::new(200)
            .set_body_json(json!({"choices": [{"message": {"role": "assistant", "content": "gata"}}]})),
    )
    .await;
    let driver = driver(&server);
    let (result, fragments) = run(driver.as_ref(), &request("salut")).await;
    assert_eq!(fragments, ["gata"]);
    assert_eq!(result.unwrap().text.as_deref(), Some("gata"));
}

#[tokio::test]
async fn usage_is_read_from_last_chunk() {
    let server = MockServer::start().await;
    serve(
        &server,
        stream(&[
            json!({"choices": [{"delta": {"content": "ok"}}], "usage": null}),
            json!({"choices": [], "usage": {"prompt_tokens": 120, "completion_tokens": 7,
                "prompt_tokens_details": {"cached_tokens": 100}}}),
        ]),
    )
    .await;
    let driver = driver(&server);
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert_eq!(result.unwrap().usage, TokenUsage { input: 120, output: 7, cached_input: 100 });

    let sent: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    assert_eq!(sent["stream"], true);
    assert_eq!(sent["stream_options"]["include_usage"], true);
}

#[tokio::test]
async fn messages_follow_the_openai_shape() {
    let server = MockServer::start().await;
    serve(&server, stream(&[json!({"choices": [{"delta": {"content": "ok"}}]})])).await;
    let request = ModelRequest {
        messages: vec![
            Message::System { text: "sistem".into() },
            Message::User { text: "uite".into(), images: vec![image()] },
            Message::Assistant { text: None, tool_calls: vec![call("c1"), call("c2")], opaque: vec![] },
            Message::Tool { id: "c1".into(), content: "o poză".into(), is_error: false, images: vec![image()] },
            Message::Tool { id: "c2".into(), content: "eroare".into(), is_error: true, images: vec![] },
            Message::User { text: "și acum?".into(), images: vec![] },
        ],
        tools: vec![],
        effort: Some("low".into()),
    };
    let driver = driver(&server);
    run(driver.as_ref(), &request).await.0.unwrap();

    let sent: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    let messages = sent["messages"].as_array().unwrap();
    let roles: Vec<_> = messages.iter().map(|message| message["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["system", "user", "assistant", "tool", "tool", "user", "user"]);
    assert_eq!(messages[1]["content"][0], json!({"type": "text", "text": "uite"}));
    assert_eq!(messages[1]["content"][1]["image_url"]["url"], "data:image/png;base64,AAAA");
    assert_eq!(messages[2]["content"], Value::Null);
    assert_eq!(messages[2]["tool_calls"][1]["id"], "c2");
    assert_eq!(messages[3]["tool_call_id"], "c1");
    // A tool's images follow the batch of its results, because a tool message must come right after the call.
    assert_eq!(messages[5]["content"][0]["type"], "image_url");
    assert_eq!(messages[6]["content"], "și acum?");
    assert_eq!(sent["reasoning_effort"], "low");
}

#[tokio::test]
async fn retries_429_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "2"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    serve(&server, stream(&[json!({"choices": [{"delta": {"content": "ok"}}]})])).await;

    let driver = with_retries(driver(&server));
    let started = Instant::now();
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert_eq!(result.unwrap().text.as_deref(), Some("ok"));
    // The server's Retry-After wins over the 1-second first pause.
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn retries_500_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    serve(&server, stream(&[json!({"choices": [{"delta": {"content": "ok"}}]})])).await;

    let driver = with_retries(driver(&server));
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert_eq!(result.unwrap().text.as_deref(), Some("ok"));
}

#[tokio::test]
async fn gives_up_after_5_attempts() {
    let server = MockServer::start().await;
    serve(&server, ResponseTemplate::new(429).insert_header("retry-after", "0")).await;

    let driver = with_retries(driver(&server));
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert!(matches!(result, Err(ModelError::RateLimited { .. })));
    assert_eq!(server.received_requests().await.unwrap().len(), 5);
}

/// Sends some text and then fails with a server error, which must not be repeated.
struct FailsAfterText {
    calls: AtomicUsize,
}

#[async_trait]
impl ModelDriver for FailsAfterText {
    async fn stream(
        &self,
        _request: &ModelRequest,
        on_text: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<Completion, ModelError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        on_text("pe jumătate");
        Err(ModelError::Server("the model answered 500".into()))
    }

    fn context_window(&self) -> ContextWindow {
        ContextWindow { tokens: 1, estimated: true }
    }

    fn supports_images(&self) -> bool {
        false
    }
}

#[tokio::test]
async fn text_already_sent_is_not_retried() {
    let inner = Arc::new(FailsAfterText { calls: AtomicUsize::new(0) });
    let driver = with_retries(inner.clone());
    let (result, fragments) = run(driver.as_ref(), &request("salut")).await;
    assert!(matches!(result, Err(ModelError::Server(_))));
    assert_eq!(fragments, ["pe jumătate"]);
    assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn detects_context_overflow() {
    let server = MockServer::start().await;
    serve(
        &server,
        ResponseTemplate::new(400).set_body_json(json!({"error": {
            "message": "the request exceeds the available context size, try increasing the context size"
        }})),
    )
    .await;
    let driver = driver(&server);
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert!(matches!(result, Err(ModelError::ContextTooLarge(_))));
}

#[tokio::test]
async fn reads_window_from_props() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/props"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"default_generation_settings": {"n_ctx": 4096}})))
        .mount(&server)
        .await;
    serve(&server, stream(&[json!({"choices": [{"delta": {"content": "ok"}}]})])).await;

    let driver = driver_with_window(&server, None);
    assert_eq!(driver.context_window(), ContextWindow { tokens: 128_000, estimated: true });
    run(driver.as_ref(), &request("salut")).await.0.unwrap();
    assert_eq!(driver.context_window(), ContextWindow { tokens: 4096, estimated: false });
}

#[tokio::test]
async fn window_defaults_to_an_estimate_without_props() {
    let server = MockServer::start().await;
    serve(&server, stream(&[json!({"choices": [{"delta": {"content": "ok"}}]})])).await;

    let driver = driver_with_window(&server, None);
    run(driver.as_ref(), &request("salut")).await.0.unwrap();
    assert_eq!(driver.context_window(), ContextWindow { tokens: 128_000, estimated: true });
}

#[tokio::test]
async fn configured_window_is_not_an_estimate() {
    let server = MockServer::start().await;
    let driver = driver_with_window(&server, Some(32_000));
    assert_eq!(driver.context_window(), ContextWindow { tokens: 32_000, estimated: false });
}

#[tokio::test]
async fn api_key_stays_out_of_errors() {
    let server = MockServer::start().await;
    serve(&server, ResponseTemplate::new(500).set_body_string("boom")).await;
    let driver = OpenAiDriver::new(ModelConfig {
        base_url: server.uri(),
        model: "m".into(),
        api_key: Some("sk-secret-key".into()),
        context_window: Some(8_192),
    });
    let (result, _) = run(&driver, &request("salut")).await;
    let error = result.unwrap_err();
    assert!(error.to_string().contains("boom"));
    assert!(!format!("{error:?}").contains("sk-secret-key"));
}

#[tokio::test]
async fn stream_error_event_fails_the_turn() {
    let server = MockServer::start().await;
    serve(&server, stream(&[json!({"error": {"message": "overloaded"}})])).await;
    let driver = driver(&server);
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert!(matches!(result, Err(ModelError::Other(message)) if message.contains("overloaded")));
}

#[tokio::test]
async fn empty_body_is_an_invalid_response() {
    let server = MockServer::start().await;
    serve(&server, ResponseTemplate::new(200).set_body_string("nu e json")).await;
    let driver = driver(&server);
    let (result, _) = run(driver.as_ref(), &request("salut")).await;
    assert!(matches!(result, Err(ModelError::Other(_))));
}
