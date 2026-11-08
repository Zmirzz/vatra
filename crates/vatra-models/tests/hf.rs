mod support;

use serde_json::json;
use support::build_header;
use url::Url;
use vatra_models::gguf::{GgufError, GgufValue};
use vatra_models::hf::{HfClient, HfError};
use wiremock::matchers::{header_exists, method, path, query_param};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const MIB: u64 = 1 << 20;

async fn client_for(server: &MockServer) -> HfClient {
    HfClient::new(Url::parse(&server.uri()).unwrap())
}

fn s(v: &str) -> GgufValue {
    GgufValue::String(v.to_string())
}

/// Answers Range requests by slicing `data`. Ranges past the end return what exists.
struct RangeFile(Vec<u8>);

impl Respond for RangeFile {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let range = req.headers.get("range").unwrap().to_str().unwrap();
        let (start, end) = range
            .strip_prefix("bytes=")
            .unwrap()
            .split_once('-')
            .unwrap();
        let start: usize = start.parse().unwrap();
        let end: usize = end.parse().unwrap();
        if start >= self.0.len() {
            return ResponseTemplate::new(416);
        }
        let end = end.min(self.0.len() - 1);
        ResponseTemplate::new(206).set_body_bytes(self.0[start..=end].to_vec())
    }
}

#[tokio::test]
async fn search_sends_gguf_filter_and_sort() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/models"))
        .and(query_param("search", "qwen coder"))
        .and(query_param("filter", "gguf"))
        .and(query_param("sort", "downloads"))
        .and(query_param("direction", "-1"))
        .and(query_param("limit", "20"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "a/one", "downloads": 10, "likes": 2, "gated": false, "lastModified": "2026-01-01T00:00:00.000Z"},
            {"modelId": "b/two", "downloads": 5, "likes": 1, "gated": "manual"}
        ])))
        .mount(&server)
        .await;

    let repos = client_for(&server)
        .await
        .search("qwen coder")
        .await
        .unwrap();
    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0].id, "a/one");
    assert_eq!(repos[0].downloads, 10);
    assert_eq!(
        repos[0].updated.as_deref(),
        Some("2026-01-01T00:00:00.000Z")
    );
    assert!(!repos[0].gated);
    assert_eq!(repos[1].id, "b/two");
    assert!(repos[1].gated);
    assert_eq!(repos[1].updated, None);
}

#[tokio::test]
async fn files_groups_variants_with_sha() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/models/owner/model"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "owner/model", "gated": false})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/models/owner/model/tree/main"))
        .and(query_param("recursive", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"type": "file", "path": "README.md", "size": 120},
            {"type": "directory", "path": "docs", "size": 0},
            {"type": "file", "path": "model-Q4_K_M.gguf", "size": 4000,
             "lfs": {"oid": "aa11", "size": 4100}},
            {"type": "file", "path": "model-Q8_0-00001-of-00002.gguf", "size": 150,
             "lfs": {"oid": "bb11", "size": 5000000}},
            {"type": "file", "path": "model-Q8_0-00002-of-00002.gguf", "size": 150,
             "lfs": {"oid": "cc11", "size": 3000000}}
        ])))
        .mount(&server)
        .await;

    let repo = client_for(&server)
        .await
        .files("owner/model")
        .await
        .unwrap();
    assert_eq!(repo.repo, "owner/model");
    assert!(!repo.gated);
    assert_eq!(repo.variants.len(), 2);

    let q4 = &repo.variants[1];
    assert_eq!(q4.name, "Q4_K_M");
    assert_eq!(q4.files[0].sha256.as_deref(), Some("aa11"));
    assert_eq!(q4.files[0].size, 4100);
    assert_eq!(q4.size, 4100);

    let q8 = &repo.variants[0];
    assert_eq!(q8.name, "Q8_0");
    assert_eq!(q8.files.len(), 2);
    assert_eq!(q8.files[0].sha256.as_deref(), Some("bb11"));
    assert_eq!(q8.files[0].size, 5000000);
    assert_eq!(q8.files[1].sha256.as_deref(), Some("cc11"));
    assert_eq!(q8.size, 8000000);
}

#[tokio::test]
async fn remote_facts_grows_range() {
    let mut tokens = Vec::new();
    for i in 0..200_000 {
        tokens.push(s(&format!("t{i:06}")));
    }
    let data = build_header(
        3,
        &[
            ("tokenizer.ggml.tokens", GgufValue::Array(tokens)),
            ("general.architecture", s("llama")),
            ("llama.block_count", GgufValue::U32(4)),
        ],
    );
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/owner/model/resolve/main/model.gguf"))
        .and(header_exists("range"))
        .respond_with(RangeFile(data.clone()))
        .mount(&server)
        .await;

    let remote = client_for(&server)
        .await
        .remote_facts("owner/model", "model.gguf")
        .await
        .unwrap();
    assert_eq!(remote.facts.architecture.as_deref(), Some("llama"));
    assert_eq!(remote.facts.block_count, Some(4));
    assert_eq!(remote.bytes_read, data.len() as u64);
    assert!(remote.bytes_read < 8 * MIB);

    let requests = server.received_requests().await.unwrap();
    assert!(
        (2..=3).contains(&requests.len()),
        "got {} requests",
        requests.len()
    );
    assert!(requests.iter().all(|r| r.headers.contains_key("range")));
}

#[tokio::test]
async fn remote_facts_stops_at_64mib() {
    // A key whose value claims to be 100 MiB long.
    let mut data = b"GGUF".to_vec();
    data.extend_from_slice(&3u32.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes());
    data.extend_from_slice(&1u64.to_le_bytes());
    data.extend_from_slice(&12u64.to_le_bytes());
    data.extend_from_slice(b"general.name");
    data.extend_from_slice(&8u32.to_le_bytes());
    data.extend_from_slice(&(100 * MIB).to_le_bytes());
    data.extend_from_slice(&[b'x'; 16]);

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/owner/model/resolve/main/big.gguf"))
        .respond_with(RangeFile(data))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .await
        .remote_facts("owner/model", "big.gguf")
        .await
        .unwrap_err();
    assert!(
        matches!(err, HfError::Metadata(GgufError::TooLarge)),
        "got {err:?}"
    );

    for request in server.received_requests().await.unwrap() {
        let range = request.headers.get("range").unwrap().to_str().unwrap();
        let end: u64 = range.rsplit('-').next().unwrap().parse().unwrap();
        assert!(end < 64 * MIB, "range {range} goes past 64 MiB");
    }
}

#[tokio::test]
async fn gated_401_maps_to_gated() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/owner/private/resolve/main/m.gguf"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/models"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let client = client_for(&server).await;
    assert!(matches!(
        client.remote_facts("owner/private", "m.gguf").await,
        Err(HfError::Gated)
    ));
    assert!(matches!(client.search("x").await, Err(HfError::Gated)));
}

#[tokio::test]
async fn retries_once_on_429_with_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/models"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let repos = client_for(&server).await.search("retry").await.unwrap();
    assert!(repos.is_empty());
}

#[tokio::test]
async fn two_429s_are_rate_limited() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/models"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .mount(&server)
        .await;

    let err = client_for(&server).await.search("busy").await.unwrap_err();
    assert!(matches!(err, HfError::RateLimited), "got {err:?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn server_ignoring_range_is_error_free() {
    let data = build_header(
        3,
        &[
            ("general.architecture", s("llama")),
            ("llama.block_count", GgufValue::U32(7)),
        ],
    );
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/owner/model/resolve/main/model.gguf"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(data.clone()))
        .mount(&server)
        .await;

    let remote = client_for(&server)
        .await
        .remote_facts("owner/model", "model.gguf")
        .await
        .unwrap();
    assert_eq!(remote.facts.architecture.as_deref(), Some("llama"));
    assert_eq!(remote.facts.block_count, Some(7));
    assert_eq!(remote.bytes_read, data.len() as u64);
}

#[test]
fn resolve_url_encodes_segments() {
    let client = HfClient::new(Url::parse("https://huggingface.co/").unwrap());
    let url = client.resolve_url("a/b", "Q8_0/m-Q8_0-00001-of-00002.gguf");
    assert!(
        url.as_str()
            .ends_with("/a/b/resolve/main/Q8_0/m-Q8_0-00001-of-00002.gguf"),
        "got {url}"
    );
}

#[tokio::test]
async fn token_goes_only_to_the_hugging_face_host() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let hub = MockServer::start().await;
    let cdn = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/models"))
        .and(header("authorization", "Bearer hf_secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&hub)
        .await;
    let client = vatra_models::hf::HfClient::new(format!("{}/", hub.uri()).parse().unwrap());
    client.set_token(Some("hf_secret".into()));
    assert!(client.search("x").await.unwrap().is_empty(), "the hub got the token");
    let elsewhere: url::Url = format!("{}/file.gguf", cdn.uri()).parse().unwrap();
    assert_eq!(client.token_for(&elsewhere), None);
    client.set_token(Some("  ".into()));
    assert!(!client.has_token());
}
