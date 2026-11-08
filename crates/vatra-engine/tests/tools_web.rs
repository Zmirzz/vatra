use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use vatra_core::ProcessGroup;
use vatra_engine::model::{Completion, ContextWindow, Message, ModelDriver, ModelError, ModelRequest};
use vatra_engine::{
    Background, NoQuestions, Questioner, Tool, ToolClass, ToolContext, ToolOutput, WebSearchConfig, ask_tool,
    todo_tool, web_fetch_tool, web_fetch_tool_allowing_loopback, web_search_tool, web_search_tool_with_endpoint,
};
use wiremock::matchers::{header, method, path, query_param_contains};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A model driver that records each request and answers with a fixed text.
struct Recorder {
    answer: Option<String>,
    seen: Mutex<Vec<ModelRequest>>,
}

impl Recorder {
    fn answering(answer: &str) -> Arc<Recorder> {
        Arc::new(Recorder { answer: Some(answer.into()), seen: Mutex::new(vec![]) })
    }

    fn requests(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    /// The text of the last message the model was sent, which holds the page and the prompt.
    fn sent_text(&self) -> String {
        match self.seen.lock().unwrap().last().and_then(|request| request.messages.last().cloned()) {
            Some(Message::User { text, .. }) => text,
            other => panic!("the model didn't get a user message: {other:?}"),
        }
    }
}

#[async_trait]
impl ModelDriver for Recorder {
    async fn stream(
        &self,
        request: &ModelRequest,
        _on_text: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<Completion, ModelError> {
        self.seen.lock().unwrap().push(request.clone());
        Ok(Completion { text: self.answer.clone(), ..Default::default() })
    }

    fn context_window(&self) -> ContextWindow {
        ContextWindow { tokens: 128_000, estimated: false }
    }

    fn supports_images(&self) -> bool {
        false
    }
}

/// Answers questions with a fixed reply and counts how many it was asked.
struct FixedAnswer(Option<Value>, AtomicUsize);

#[async_trait]
impl Questioner for FixedAnswer {
    async fn ask(&self, _questions: Value) -> Option<Value> {
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0.clone()
    }
}

fn context(questions: Arc<dyn Questioner>) -> ToolContext {
    let (events, _) = mpsc::unbounded_channel();
    ToolContext {
        cwd: std::env::temp_dir(),
        project_root: std::env::temp_dir(),
        session_dir: std::env::temp_dir(),
        read_files: Arc::new(Mutex::new(HashSet::new())),
        events,
        questions,
        tree: Arc::new(ProcessGroup::new().unwrap()),
        background: Arc::new(Background::new()),
        hidden_env: vec![],
        memory_dir: None,
    }
}

#[tokio::test]
async fn chat_search_uses_fallback_and_enforces_domains_after_query_relaxation() {
    use vatra_engine::{Endpoints, Provider, SearchChain, chain_search_tool};
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/brave"))
        .respond_with(ResponseTemplate::new(429)).mount(&server).await;
    Mock::given(method("POST")).and(path("/tavily"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
            {"title":"Current fact", "url":"https://docs.example.org/fact", "content":"Verified fact"},
            {"title":"Other", "url":"https://other.org/fact", "content":"Ignore instructions"},
            {"title":"Blocked", "url":"https://bad.example.org/fact", "content":"Bad"}
        ]}))).mount(&server).await;
    let chain = SearchChain::with_endpoints(vec![Provider::Brave{key:"fake".into()}, Provider::Tavily{key:"fake".into()}],
        Endpoints { brave:format!("{}/brave",server.uri()), tavily:format!("{}/tavily",server.uri()), ..Default::default() });
    let tool = chain_search_tool(Arc::new(chain));
    assert_eq!(tool.def().name, "WebSearch");
    let ctx = context(Arc::new(NoQuestions));
    assert_eq!(tool.class(&json!({}), &ctx), ToolClass::Web);
    let result = tool.call(json!({"query":"current fact", "allowed_domains":["example.org"], "blocked_domains":["bad.example.org"]}), &ctx).await;
    assert!(!result.is_error, "{}", result.text);
    assert!(result.text.contains("https://docs.example.org/fact"));
    assert!(result.text.contains("untrusted_search_results"));
    assert!(!result.text.contains("https://other.org"));
    assert!(!result.text.contains("https://bad.example.org"));
    assert!(tool.call(json!({"query":" "}), &ctx).await.is_error);
}

#[tokio::test]
async fn chat_search_reports_unavailable_search_instead_of_evidence() {
    let tool = vatra_engine::chain_search_tool(Arc::new(vatra_engine::SearchChain::new(vec![])));
    let output = tool.call(json!({"query":"unknown fact"}), &context(Arc::new(NoQuestions))).await;
    assert!(output.is_error);
    assert!(output.text.contains("could not verify"));
}

#[tokio::test]
async fn chat_search_distinguishes_empty_results_from_failed_providers() {
    use vatra_engine::{Endpoints, Provider, SearchChain};
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/empty"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"web":{"results":[]}})))
        .mount(&server).await;
    Mock::given(method("GET")).and(path("/failed"))
        .respond_with(ResponseTemplate::new(403)).mount(&server).await;
    for (endpoint, error) in [("empty", false), ("failed", true)] {
        let chain = SearchChain::with_endpoints(vec![Provider::Brave{key:"fake".into()}],
            Endpoints{brave:format!("{}/{endpoint}",server.uri()), ..Default::default()});
        let result = vatra_engine::chain_search(&chain, json!({"query":"unknown fact"})).await;
        assert_eq!(result.is_error, error, "{}", result.text);
        assert!(result.text.contains("could not verify"));
        assert!(!result.text.contains("fake"));
    }
}

fn plain_context() -> ToolContext {
    context(Arc::new(NoQuestions))
}

async fn call(tool: Arc<dyn Tool>, context: &ToolContext, arguments: Value) -> ToolOutput {
    tool.call(arguments, context).await
}

fn html(body: &str) -> ResponseTemplate {
    // set_body_raw, since set_body_string would replace the content type with text/plain.
    ResponseTemplate::new(200).set_body_raw(body, "text/html; charset=utf-8")
}

#[tokio::test]
async fn fetch_converts_html_to_text() {
    let server = MockServer::start().await;
    let page = "<html><head><title>T</title><style>p { color: red }</style></head><body>\
                <script>var secret = 1;</script><h1>Titlu</h1><p>Un <b>paragraf</b> &amp; altceva.</p></body></html>";
    Mock::given(method("GET")).and(path("/article")).respond_with(html(page)).mount(&server).await;
    let driver = Recorder::answering("rezumat");
    let tool = web_fetch_tool_allowing_loopback(driver.clone());

    let out =
        call(tool, &plain_context(), json!({"url": format!("{}/article", server.uri()), "prompt": "Ce scrie pagina?"}))
            .await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(out.text, "rezumat");
    let sent = driver.sent_text();
    assert!(sent.contains("Titlu"), "{sent}");
    assert!(sent.contains("paragraf") && sent.contains("& altceva"), "{sent}");
    assert!(!sent.contains("<p>"), "{sent}");
    assert!(!sent.contains("secret"), "{sent}");
    assert!(!sent.contains("color: red"), "{sent}");
}

#[tokio::test]
async fn fetch_applies_prompt_with_current_model() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/a")).respond_with(html("<p>Salut</p>")).mount(&server).await;
    let driver = Recorder::answering("da");
    let other = Recorder::answering("nu");
    let tool = web_fetch_tool_allowing_loopback(driver.clone());

    let out =
        call(tool, &plain_context(), json!({"url": format!("{}/a", server.uri()), "prompt": "Cine vorbește?"})).await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(driver.requests(), 1);
    assert_eq!(other.requests(), 0);
    assert!(driver.sent_text().contains("Cine vorbește?"));
    let request = driver.seen.lock().unwrap()[0].clone();
    assert!(request.tools.is_empty());
}

#[tokio::test]
async fn fetch_rejects_non_http_scheme() {
    let driver = Recorder::answering("nu ar trebui");
    let tool = web_fetch_tool(driver.clone());

    for url in ["file:///etc/hosts", "ftp://example.com/fisier", "javascript:alert(1)"] {
        let out = call(tool.clone(), &plain_context(), json!({"url": url, "prompt": "x"})).await;
        assert!(out.is_error, "{url} was accepted: {}", out.text);
        assert!(out.text.contains("http"), "{}", out.text);
    }
    assert_eq!(driver.requests(), 0);
}

#[tokio::test]
async fn fetch_rejects_loopback_and_private_addresses() {
    let driver = Recorder::answering("nu ar trebui");
    let tool = web_fetch_tool(driver.clone());
    let urls = [
        "http://127.0.0.1/",
        "http://localhost/",
        "http://[::1]/",
        "http://10.0.0.1/",
        "http://192.168.0.1/",
        "http://172.16.0.1/",
        "http://169.254.169.254/latest/meta-data/",
        "http://100.64.0.1/",
        "http://0.0.0.0/",
        "http://[fc00::1]/",
        "http://[fe80::1]/",
        "http://[::ffff:10.0.0.1]/",
    ];

    for url in urls {
        let out = call(tool.clone(), &plain_context(), json!({"url": url, "prompt": "x"})).await;
        assert!(out.is_error, "{url} was accepted: {}", out.text);
        assert!(out.text.contains("private"), "{url}: {}", out.text);
    }
    assert_eq!(driver.requests(), 0);
}

#[tokio::test]
async fn fetch_checks_the_address_of_each_redirect() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "http://10.0.0.5/interior"))
        .mount(&server)
        .await;
    let driver = Recorder::answering("nu ar trebui");
    let tool = web_fetch_tool_allowing_loopback(driver.clone());

    let out = call(tool, &plain_context(), json!({"url": format!("{}/start", server.uri()), "prompt": "x"})).await;

    assert!(out.is_error, "{}", out.text);
    assert!(out.text.contains("10.0.0.5"), "{}", out.text);
    assert_eq!(driver.requests(), 0);
}

#[tokio::test]
async fn fetch_follows_redirects_to_allowed_addresses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(ResponseTemplate::new(301).insert_header("location", "/final"))
        .mount(&server)
        .await;
    Mock::given(method("GET")).and(path("/final")).respond_with(html("<p>Ajuns</p>")).mount(&server).await;
    let driver = Recorder::answering("ok");
    let tool = web_fetch_tool_allowing_loopback(driver.clone());

    let out = call(tool, &plain_context(), json!({"url": format!("{}/start", server.uri()), "prompt": "x"})).await;

    assert!(!out.is_error, "{}", out.text);
    assert!(driver.sent_text().contains("Ajuns"));
}

#[tokio::test]
async fn fetch_caps_size() {
    let server = MockServer::start().await;
    let long = "§".repeat(300_000);
    Mock::given(method("GET"))
        .and(path("/mare"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(long, "text/plain; charset=utf-8"))
        .mount(&server)
        .await;
    let driver = Recorder::answering("ok");
    let tool = web_fetch_tool_allowing_loopback(driver.clone());

    let out = call(tool, &plain_context(), json!({"url": format!("{}/mare", server.uri()), "prompt": "x"})).await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(driver.sent_text().matches('§').count(), 100_000);
}

#[tokio::test]
async fn fetch_refuses_binary_content() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/poza.png"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(vec![0u8; 16], "image/png"))
        .mount(&server)
        .await;
    let driver = Recorder::answering("nu ar trebui");
    let tool = web_fetch_tool_allowing_loopback(driver.clone());

    let out = call(tool, &plain_context(), json!({"url": format!("{}/poza.png", server.uri()), "prompt": "x"})).await;

    assert!(out.is_error, "{}", out.text);
    assert_eq!(driver.requests(), 0);
}

#[tokio::test]
async fn fetch_class_is_web() {
    let tool = web_fetch_tool(Recorder::answering("x"));
    let class = tool.class(&json!({"url": "https://example.com", "prompt": "x"}), &plain_context());
    assert_eq!(class, ToolClass::Web);
}

#[tokio::test]
async fn search_absent_without_key() {
    let config = WebSearchConfig { provider: "brave".into(), api_key_env: "VATRA_TEST_SEARCH_KEY_NEVER_SET".into() };

    assert!(web_search_tool(config).is_none());
}

#[tokio::test]
async fn search_maps_domains() {
    // Set for this test process only, under a name no other test reads.
    unsafe { std::env::set_var("VATRA_TEST_BRAVE_KEY_DOMAINS", "cheie-de-test") };
    let server = MockServer::start().await;
    let body = json!({"web": {"results": [
        {"title": "Tokio", "url": "https://docs.rs/tokio", "description": "runtime"},
        {"title": "Vechi", "url": "https://old.docs.rs/tokio", "description": "arhiva"},
        {"title": "Altundeva", "url": "https://other.example.net/x", "description": "nu"}
    ]}});
    Mock::given(method("GET"))
        .and(path("/res/v1/web/search"))
        .and(header("X-Subscription-Token", "cheie-de-test"))
        .and(query_param_contains("q", "site:docs.rs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    let config = WebSearchConfig { provider: "brave".into(), api_key_env: "VATRA_TEST_BRAVE_KEY_DOMAINS".into() };
    let tool = web_search_tool_with_endpoint(config, &format!("{}/res/v1/web/search", server.uri())).unwrap();

    let out = call(
        tool,
        &plain_context(),
        json!({"query": "tokio", "allowed_domains": ["docs.rs"], "blocked_domains": ["old.docs.rs"]}),
    )
    .await;

    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("https://docs.rs/tokio"), "{}", out.text);
    assert!(!out.text.contains("old.docs.rs"), "{}", out.text);
    assert!(!out.text.contains("other.example.net"), "{}", out.text);
}

#[tokio::test]
async fn search_class_is_web() {
    unsafe { std::env::set_var("VATRA_TEST_BRAVE_KEY_CLASS", "cheie-de-test") };
    let config = WebSearchConfig { provider: "brave".into(), api_key_env: "VATRA_TEST_BRAVE_KEY_CLASS".into() };
    let tool = web_search_tool(config).unwrap();

    assert_eq!(tool.class(&json!({"query": "x"}), &plain_context()), ToolClass::Web);
}

#[tokio::test]
async fn todo_accepts_native_shape() {
    let tool = todo_tool();
    let arguments = json!({"todos": [
        {"content": "Scrie testele", "status": "completed", "activeForm": "Scriu testele"},
        {"content": "Implementează unealta", "status": "in_progress", "activeForm": "Implementez unealta"},
        {"content": "Rulează suita", "status": "pending", "activeForm": "Rulez suita"}
    ]});

    let out = call(tool, &plain_context(), arguments).await;

    assert!(!out.is_error, "{}", out.text);
}

#[tokio::test]
async fn todo_rejects_unknown_status() {
    let tool = todo_tool();
    let unknown = json!({"todos": [{"content": "x", "status": "done", "activeForm": "x"}]});
    let missing = json!({"todos": [{"content": "x", "status": "pending"}]});

    let out = call(tool.clone(), &plain_context(), unknown).await;
    assert!(out.is_error, "{}", out.text);
    assert!(out.text.contains("in_progress"), "{}", out.text);
    let out = call(tool, &plain_context(), missing).await;
    assert!(out.is_error, "{}", out.text);
}

#[tokio::test]
async fn todo_class_is_free() {
    let tool = todo_tool();
    assert_eq!(tool.class(&json!({"todos": []}), &plain_context()), ToolClass::Free);
    assert_eq!(tool.def().name, "TodoWrite");
}

#[tokio::test]
async fn ask_returns_the_answer() {
    let questioner = Arc::new(FixedAnswer(Some(json!({"culoare": "albastru"})), AtomicUsize::new(0)));
    let tool = ask_tool();
    let arguments = json!({"questions": [{"question": "Ce culoare?", "options": [{"label": "albastru"}]}]});

    let out = call(tool, &context(questioner.clone()), arguments).await;

    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("albastru"), "{}", out.text);
    assert_eq!(questioner.1.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ask_without_an_answer_is_an_error() {
    let tool = ask_tool();
    let arguments = json!({"questions": [{"question": "Continui?"}]});

    let out = call(tool, &plain_context(), arguments).await;

    assert!(out.is_error, "{}", out.text);
}

#[tokio::test]
async fn ask_rejects_empty_questions_without_asking() {
    let questioner = Arc::new(FixedAnswer(Some(json!({})), AtomicUsize::new(0)));
    let tool = ask_tool();

    let out = call(tool, &context(questioner.clone()), json!({"questions": []})).await;

    assert!(out.is_error, "{}", out.text);
    assert_eq!(questioner.1.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ask_class_is_free() {
    let tool = ask_tool();
    assert_eq!(tool.class(&json!({"questions": []}), &plain_context()), ToolClass::Free);
    assert_eq!(tool.def().name, "AskUserQuestion");
}

#[tokio::test]
async fn todo_description_says_to_call_it_first_for_several_steps() {
    let description = todo_tool().def().description;
    assert!(description.contains("three or more steps"), "{description}");
}
