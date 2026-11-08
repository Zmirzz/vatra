//! The research side of the web: search providers in a chain, and the pages research reads.

use serde_json::json;
use vatra_engine::{Endpoints, Fetcher, Provider, SearchChain};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn endpoints(server: &MockServer) -> Endpoints {
    let base = server.uri();
    Endpoints {
        brave: format!("{base}/brave"),
        duckduckgo: format!("{base}/ddg/"),
        tavily: format!("{base}/tavily"),
        serper: format!("{base}/serper"),
        arxiv: format!("{base}/arxiv"),
        openalex: format!("{base}/openalex"),
    }
}

fn brave_body(urls: &[(&str, &str)]) -> serde_json::Value {
    json!({"web": {"results": urls.iter().map(|(t, u)| json!({"title": t, "url": u, "description": format!("about {t}")})).collect::<Vec<_>>()}})
}

#[tokio::test]
async fn a_failing_provider_hands_over_to_the_next_and_the_attempts_are_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/brave")).respond_with(ResponseTemplate::new(429)).mount(&server).await;
    Mock::given(method("POST")).and(path("/tavily")).and(header("authorization", "Bearer tk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [{"title": "Tokio runtime", "url": "https://t.example/a", "content": "the tokio runtime"}]})))
        .mount(&server).await;
    let chain = SearchChain::with_endpoints(vec![Provider::Brave { key: "k".into() }, Provider::Tavily { key: "tk".into() }], endpoints(&server));
    let outcome = chain.search("tokio runtime", "how does the tokio runtime work").await;
    assert_eq!(outcome.provider, Some("tavily"));
    assert_eq!(outcome.hits[0].url, "https://t.example/a");
    assert_eq!(outcome.attempts.len(), 2);
    assert!(outcome.attempts[0].1.as_ref().unwrap_err().contains("too many requests"));
    assert_eq!(outcome.attempts[1].1, Ok(1));
}

#[tokio::test]
async fn an_empty_answer_is_retried_with_a_looser_query_before_moving_on() {
    let server = MockServer::start().await;
    // The exact query finds nothing; the relaxed one does.
    Mock::given(method("GET")).and(path("/brave")).and(query_param("q", "\"exact phrase\" site:x.com tokio runtime internals"))
        .respond_with(ResponseTemplate::new(200).set_body_json(brave_body(&[]))).mount(&server).await;
    Mock::given(method("GET")).and(path("/brave")).and(query_param("q", "exact phrase tokio runtime internals"))
        .respond_with(ResponseTemplate::new(200).set_body_json(brave_body(&[("Tokio runtime internals", "https://r.example/x")]))).mount(&server).await;
    let chain = SearchChain::with_endpoints(vec![Provider::Brave { key: "k".into() }], endpoints(&server));
    let outcome = chain.search("\"exact phrase\" site:x.com tokio runtime internals", "tokio runtime internals").await;
    let seen: Vec<String> = server.received_requests().await.unwrap().iter().map(|r| r.url.query().unwrap_or("").to_string()).collect();
    assert_eq!(outcome.provider, Some("brave"), "{:?} {:?}", outcome.attempts, seen);
    assert_eq!(outcome.hits.len(), 1);
}

#[tokio::test]
async fn duckduckgo_needs_no_key_and_a_refused_key_is_explained() {
    let server = MockServer::start().await;
    let html = r#"<a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org%2Fa">Example page</a><a class="result__snippet" href="x">about the example</a>"#;
    Mock::given(method("GET")).and(path("/ddg/")).respond_with(ResponseTemplate::new(200).set_body_string(html)).mount(&server).await;
    Mock::given(method("POST")).and(path("/serper")).respond_with(ResponseTemplate::new(403)).mount(&server).await;
    let chain = SearchChain::with_endpoints(vec![Provider::Serper { key: "bad".into() }, Provider::DuckDuckGo], endpoints(&server));
    let outcome = chain.search("example page", "example page").await;
    assert_eq!(outcome.provider, Some("duckduckgo"));
    assert_eq!(outcome.hits[0].url, "https://example.org/a");
    assert!(outcome.attempts[0].1.as_ref().unwrap_err().contains("refused the key"));
}

#[tokio::test]
async fn every_provider_failing_gives_no_hits_and_the_reasons() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/ddg/")).respond_with(ResponseTemplate::new(200).set_body_string("<div class=\"anomaly-modal\">check</div>")).mount(&server).await;
    let chain = SearchChain::with_endpoints(vec![Provider::DuckDuckGo], endpoints(&server));
    let outcome = chain.search("anything", "anything").await;
    assert!(outcome.hits.is_empty());
    assert!(outcome.attempts[0].1.as_ref().unwrap_err().contains("only a person can pass"));
}

#[tokio::test]
async fn scholarly_sources_come_from_arxiv_and_openalex() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/arxiv")).respond_with(ResponseTemplate::new(200).set_body_string(
        "<feed><entry><id>http://arxiv.org/abs/1</id><title>Graph neural networks</title><summary>We survey graph neural networks.</summary></entry></feed>")).mount(&server).await;
    Mock::given(method("GET")).and(path("/openalex")).respond_with(ResponseTemplate::new(200).set_body_json(
        json!({"results": [{"title": "Graph neural networks survey", "doi": "https://doi.org/10.1/z", "publication_year": 2023, "abstract_inverted_index": {"graph": [0], "neural": [1], "networks": [2]}}]}))).mount(&server).await;
    let chain = SearchChain::with_endpoints(vec![], endpoints(&server));
    let papers = chain.scholarly("graph neural networks").await;
    assert_eq!(papers.len(), 2);
    assert!(papers.iter().any(|p| p.url == "https://doi.org/10.1/z" && p.description == "graph neural networks"));
}

// ----------------------------------------------------------------------------------------------- pages

fn tiny_pdf(text: &str) -> Vec<u8> {
    let stream = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{}\nendstream", stream.len(), stream),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{}\nendobj\n", i + 1, body).as_bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

#[tokio::test]
async fn research_reads_pdfs_and_refuses_pages_that_say_nothing() {
    let server = MockServer::start().await;
    let sentence = "The annual subscription costs forty nine euros per month and includes every feature of the plan for all teams";
    Mock::given(method("GET")).and(path("/paper.pdf")).respond_with(ResponseTemplate::new(200).insert_header("content-type", "application/pdf").set_body_bytes(tiny_pdf(sentence))).mount(&server).await;
    Mock::given(method("GET")).and(path("/spa")).respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/html")
        .set_body_string("<html><body><div id=\"root\"></div><noscript>You need to enable JavaScript to run this app.</noscript><script src=\"a.js\"></script></body></html>")).mount(&server).await;
    Mock::given(method("GET")).and(path("/shield")).respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/html")
        .set_body_string("<html><body><h1>Just a moment...</h1><p>Checking your browser before accessing the site.</p></body></html>")).mount(&server).await;
    let long_page = format!("<html><body><p>{}</p></body></html>", "Real content about subscriptions and prices for teams. ".repeat(12));
    Mock::given(method("GET")).and(path("/ok")).respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/html").set_body_string(long_page)).mount(&server).await;

    let fetcher = Fetcher::allowing_loopback().strict();
    let pdf = fetcher.fetch_text(&format!("{}/paper.pdf", server.uri())).await.unwrap();
    assert!(pdf.contains("forty nine euros per month"), "{pdf}");
    let spa = fetcher.fetch_text(&format!("{}/spa", server.uri())).await.unwrap_err();
    assert!(spa.contains("JavaScript"), "{spa}");
    let shield = fetcher.fetch_text(&format!("{}/shield", server.uri())).await.unwrap_err();
    assert!(shield.contains("only a person can pass"), "{shield}");
    assert!(fetcher.fetch_text(&format!("{}/ok", server.uri())).await.unwrap().contains("Real content"));

    // Outside research the same pages are passed on as they are, so the web fetch tool behaves as before.
    let plain = Fetcher::allowing_loopback();
    assert!(plain.fetch_text(&format!("{}/shield", server.uri())).await.is_ok());
    assert!(plain.fetch_text(&format!("{}/paper.pdf", server.uri())).await.is_err());
}

#[test]
fn a_video_page_gives_its_caption_track_and_the_transcript_text() {
    let page = r#"<title>How tokio works - YouTube</title><script>var x={"captionTracks":[{"baseUrl":"https://www.youtube.com/api/timedtext?v=1&lang=fr","languageCode":"fr"},{"baseUrl":"https://www.youtube.com/api/timedtext?v=1&lang=en","languageCode":"en"}],"audioTracks":[]}</script>"#;
    assert_eq!(vatra_engine::caption_url(page).as_deref(), Some("https://www.youtube.com/api/timedtext?v=1&lang=en"));
    let body = r#"{"events":[{"segs":[{"utf8":"Hello"},{"utf8":" there"}]},{"segs":[{"utf8":"\n"}]},{"segs":[{"utf8":"welcome to tokio"}]}]}"#;
    assert_eq!(vatra_engine::parse_transcript(body).as_deref(), Some("Hello there welcome to tokio"));
    assert_eq!(vatra_engine::caption_url("<html>no captions</html>"), None);
}

// ------------------------------------------------------------------------------------ pages drawn by scripts

struct FakeBrowser {
    html: String,
    asked: std::sync::Mutex<Vec<(String, std::net::IpAddr)>>,
}

#[async_trait::async_trait]
impl vatra_engine::PageRenderer for FakeBrowser {
    async fn render(&self, url: &url::Url, ip: std::net::IpAddr) -> Result<String, String> {
        self.asked.lock().unwrap().push((url.to_string(), ip));
        Ok(self.html.clone())
    }
}

#[tokio::test]
async fn a_page_that_needs_scripts_is_drawn_in_the_browser_when_there_is_one_and_a_human_check_is_not_worked_around() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/spa")).respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/html")
        .set_body_string("<html><body><div id=\"root\"></div><noscript>enable javascript</noscript><script src=\"a.js\"></script></body></html>")).mount(&server).await;
    Mock::given(method("GET")).and(path("/shield")).respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/html")
        .set_body_string("<html><body><h1>Just a moment...</h1><p>Checking your browser before accessing the site.</p></body></html>")).mount(&server).await;
    let drawn = format!("<html><body><main>{}</main></body></html>", "Prices drawn by the script: the plan costs forty nine euros a month. ".repeat(8));
    let browser = std::sync::Arc::new(FakeBrowser { html: drawn, asked: Default::default() });
    let fetcher = Fetcher::allowing_loopback().strict().with_renderer(browser.clone());

    let text = fetcher.fetch_text(&format!("{}/spa", server.uri())).await.unwrap();
    assert!(text.contains("forty nine euros"), "{text}");
    let asked = browser.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].1.is_loopback(), "the browser is told the address that was checked");

    let blocked = fetcher.fetch_text(&format!("{}/shield", server.uri())).await.unwrap_err();
    assert!(blocked.contains("only a person can pass"));
    assert_eq!(browser.asked.lock().unwrap().len(), 1, "the browser was not used against a human check");

    // Without a browser the page is refused, as before.
    let plain = Fetcher::allowing_loopback().strict();
    assert!(plain.fetch_text(&format!("{}/spa", server.uri())).await.unwrap_err().contains("JavaScript"));
}

#[tokio::test]
async fn a_browser_that_draws_nothing_leaves_the_page_refused() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/spa")).respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/html")
        .set_body_string("<html><body><noscript>enable javascript</noscript><script></script><script></script><script></script></body></html>")).mount(&server).await;
    let browser = std::sync::Arc::new(FakeBrowser { html: "<html><body></body></html>".into(), asked: Default::default() });
    let fetcher = Fetcher::allowing_loopback().strict().with_renderer(browser);
    let error = fetcher.fetch_text(&format!("{}/spa", server.uri())).await.unwrap_err();
    assert!(error.contains("even drawn in a browser"), "{error}");
}

#[test]
fn the_browser_command_is_headless_with_a_throwaway_profile_and_the_host_pinned() {
    let url = url::Url::parse("https://shop.example.com/prices?x=1").unwrap();
    let args = vatra_engine::HeadlessBrowser::args(&url, "93.184.216.34".parse().unwrap(), std::path::Path::new("C:/tmp/profile"));
    assert!(args.contains(&"--headless=new".to_string()) && args.contains(&"--dump-dom".to_string()));
    assert!(args.contains(&"--user-data-dir=C:/tmp/profile".to_string()));
    assert!(args.contains(&"--host-resolver-rules=MAP shop.example.com 93.184.216.34".to_string()));
    assert_eq!(args.last().unwrap(), "https://shop.example.com/prices?x=1");
}

/// Runs the real browser of this computer against a page that writes its text with a script. Run by hand:
/// `cargo test -p vatra-engine --test research_web real_browser -- --ignored`.
#[tokio::test]
#[ignore]
async fn real_browser_draws_a_page_written_by_a_script() {
    let Some(browser) = vatra_engine::HeadlessBrowser::find() else {
        eprintln!("no browser on this computer");
        return;
    };
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/spa")).respond_with(ResponseTemplate::new(200).set_body_raw(
        "<html><body><div id=\"root\"></div><script>document.getElementById('root').textContent = 'Abonamentul costa patruzeci si noua de euro pe luna si include toate functiile planului pentru echipe mici si mari. '.repeat(3);</script></body></html>", "text/html")).mount(&server).await;
    let fetcher = Fetcher::allowing_loopback().strict().with_renderer(std::sync::Arc::new(browser));
    let text = fetcher.fetch_text(&format!("{}/spa", server.uri())).await.unwrap();
    assert!(text.contains("patruzeci si noua"), "{text}");
}
