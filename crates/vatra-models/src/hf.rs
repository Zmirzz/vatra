//! Hugging Face access: GGUF search, repo file listings, and GGUF headers read over HTTP Range.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use reqwest::header::{RANGE, RETRY_AFTER};
use reqwest::{Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::gguf::{self, GgufError, ModelFacts};
use crate::variant::{self, RepoFile, Variant};

const MIB: usize = 1 << 20;
/// Enough for the header of most models.
const FIRST_READ: usize = MIB;
/// Largest header we read. Matches the parser's cap.
const MAX_HEADER: usize = 64 * MIB;
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RETRY_WAIT: u64 = 5;

#[derive(Debug, thiserror::Error)]
pub enum HfError {
    #[error("this model needs a Hugging Face account, which Vatra doesn't support yet")]
    Gated,
    #[error("not found on Hugging Face")]
    NotFound,
    #[error("Hugging Face is limiting requests; try again in a minute")]
    RateLimited,
    #[error("couldn't reach Hugging Face: {0}")]
    Network(String),
    #[error("couldn't read the model's header: {0}")]
    Metadata(GgufError),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoSummary {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
    pub gated: bool,
    pub updated: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoFiles {
    pub repo: String,
    pub gated: bool,
    pub variants: Vec<Variant>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFacts {
    pub facts: ModelFacts,
    pub bytes_read: u64,
}

pub struct HfClient {
    http: reqwest::Client,
    base: Url,
    search_cache: Cache<Vec<RepoSummary>>,
    files_cache: Cache<RepoFiles>,
    /// The person's Hugging Face token, for models that need an account. Sent only to the base host.
    token: std::sync::RwLock<Option<String>>,
}

impl HfClient {
    /// `base` is like `https://huggingface.co/`. A missing trailing slash is added.
    pub fn new(mut base: Url) -> HfClient {
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(concat!("vatra/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("the HTTP client builds with default TLS settings");
        HfClient {
            http,
            base,
            search_cache: Cache::new(),
            files_cache: Cache::new(),
            token: std::sync::RwLock::new(None),
        }
    }

    pub fn set_token(&self, token: Option<String>) {
        *self.token.write().unwrap_or_else(std::sync::PoisonError::into_inner) = token.filter(|token| !token.trim().is_empty());
    }

    pub fn has_token(&self) -> bool {
        self.token.read().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
    }

    /// The token, for a URL on the base host only. Redirects to another host never get it: reqwest drops the
    /// header when a redirect changes the host.
    pub fn token_for(&self, url: &Url) -> Option<String> {
        if url.host_str() != self.base.host_str() || url.port_or_known_default() != self.base.port_or_known_default() {
            return None;
        }
        self.token.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub fn public() -> HfClient {
        HfClient::new(Url::parse("https://huggingface.co/").expect("constant URL is valid"))
    }

    pub async fn search(&self, query: &str) -> Result<Vec<RepoSummary>, HfError> {
        let mut url = self.endpoint(["api", "models"]);
        url.query_pairs_mut()
            .append_pair("search", query)
            .append_pair("filter", "gguf")
            .append_pair("sort", "downloads")
            .append_pair("direction", "-1")
            .append_pair("limit", "20");
        let key = url.to_string();
        if let Some(hit) = self.search_cache.get(&key) {
            return Ok(hit);
        }
        let models: Vec<RawModel> = self.get_json(url).await?;
        let repos: Vec<RepoSummary> = models
            .into_iter()
            .filter_map(RawModel::into_summary)
            .collect();
        self.search_cache.put(key, repos.clone());
        Ok(repos)
    }

    pub async fn files(&self, repo: &str) -> Result<RepoFiles, HfError> {
        let mut tree = self.endpoint(
            ["api", "models"]
                .into_iter()
                .chain(repo.split('/'))
                .chain(["tree", "main"]),
        );
        tree.query_pairs_mut().append_pair("recursive", "true");
        let key = tree.to_string();
        if let Some(hit) = self.files_cache.get(&key) {
            return Ok(hit);
        }
        let meta: RawMeta = self
            .get_json(self.endpoint(["api", "models"].into_iter().chain(repo.split('/'))))
            .await?;
        let entries: Vec<RawEntry> = self.get_json(tree).await?;
        let files: Vec<RepoFile> = entries
            .into_iter()
            .filter(|entry| entry.kind == "file")
            .map(RawEntry::into_file)
            .collect();
        let listing = RepoFiles {
            repo: repo.to_string(),
            gated: gated_flag(meta.gated.as_ref()),
            variants: variant::group(&files),
        };
        self.files_cache.put(key, listing.clone());
        Ok(listing)
    }

    /// Reads the GGUF header, asking for more bytes until it parses.
    pub async fn remote_facts(&self, repo: &str, file: &str) -> Result<RemoteFacts, HfError> {
        let url = self.resolve_url(repo, file);
        let mut buf = Vec::new();
        let mut received = 0u64;
        let mut stream = None;
        let mut want = FIRST_READ;
        loop {
            let eof = self
                .fill(&url, &mut buf, &mut received, &mut stream, want)
                .await?;
            match gguf::parse(&buf) {
                Ok(header) => {
                    return Ok(RemoteFacts {
                        facts: ModelFacts::from_header(&header),
                        bytes_read: received,
                    });
                }
                Err(GgufError::NeedMore { at_least }) => {
                    if eof {
                        return Err(HfError::Metadata(GgufError::Invalid(
                            "the file ends inside the header".into(),
                        )));
                    }
                    if buf.len() >= MAX_HEADER {
                        return Err(HfError::Metadata(GgufError::TooLarge));
                    }
                    want = at_least.max(buf.len() * 2).min(MAX_HEADER);
                }
                Err(err) => return Err(HfError::Metadata(err)),
            }
        }
    }

    pub fn resolve_url(&self, repo: &str, file: &str) -> Url {
        self.endpoint(
            repo.split('/')
                .chain(["resolve", "main"])
                .chain(file.split('/')),
        )
    }

    /// Brings `buf` up to `want` bytes. Returns true when the file ends first.
    async fn fill(
        &self,
        url: &Url,
        buf: &mut Vec<u8>,
        received: &mut u64,
        stream: &mut Option<Response>,
        want: usize,
    ) -> Result<bool, HfError> {
        if let Some(resp) = stream.as_mut() {
            return read_into(resp, buf, received, want).await;
        }
        let range = format!("bytes={}-{}", buf.len(), want - 1);
        let resp = self.send(url.clone(), Some(range)).await?;
        if resp.status() == StatusCode::RANGE_NOT_SATISFIABLE {
            return Ok(true);
        }
        let mut resp = check(resp)?;
        // A 200 means the server ignored Range, so the body starts at byte 0. Keep reading it.
        let full = resp.status() == StatusCode::OK;
        if full {
            buf.clear();
        }
        let done = read_into(&mut resp, buf, received, want).await?;
        if full && !done {
            *stream = Some(resp);
        }
        Ok(done)
    }

    /// Sends a GET with an optional Range header. A 429 is retried once after Retry-After.
    async fn send(&self, url: Url, range: Option<String>) -> Result<Response, HfError> {
        let mut retried = false;
        loop {
            let mut request = self.http.get(url.clone());
            if let Some(token) = self.token_for(&url) {
                request = request.bearer_auth(token);
            }
            if let Some(range) = &range {
                request = request.header(RANGE, range.as_str());
            }
            let resp = request.send().await.map_err(network)?;
            if resp.status() != StatusCode::TOO_MANY_REQUESTS {
                return Ok(resp);
            }
            if retried {
                return Err(HfError::RateLimited);
            }
            retried = true;
            tokio::time::sleep(retry_after(&resp)).await;
        }
    }

    async fn get_json<T: DeserializeOwned>(&self, url: Url) -> Result<T, HfError> {
        let resp = check(self.send(url, None).await?)?;
        resp.json().await.map_err(network)
    }

    /// A URL under the base, with each part percent-encoded.
    fn endpoint<'a>(&self, parts: impl IntoIterator<Item = &'a str>) -> Url {
        let mut url = self.base.clone();
        if let Ok(mut segments) = url.path_segments_mut() {
            segments.pop_if_empty().extend(parts);
        }
        url
    }
}

/// Reads body bytes into `buf` until it holds `want` bytes. Returns true if the body ends first.
async fn read_into(
    resp: &mut Response,
    buf: &mut Vec<u8>,
    received: &mut u64,
    want: usize,
) -> Result<bool, HfError> {
    while buf.len() < want {
        let Some(chunk) = resp.chunk().await.map_err(network)? else {
            return Ok(true);
        };
        let take = chunk.len().min(want - buf.len());
        buf.extend_from_slice(&chunk[..take]);
        *received += take as u64;
    }
    Ok(false)
}

fn check(resp: Response) -> Result<Response, HfError> {
    match resp.status() {
        status if status.is_success() => Ok(resp),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(HfError::Gated),
        StatusCode::NOT_FOUND => Err(HfError::NotFound),
        status => Err(HfError::Network(format!("Hugging Face answered {status}"))),
    }
}

fn retry_after(resp: &Response) -> Duration {
    let secs = resp
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(1);
    Duration::from_secs(secs.min(MAX_RETRY_WAIT))
}

fn network(err: reqwest::Error) -> HfError {
    HfError::Network(err.to_string())
}

/// Hugging Face sends `false`, or a string such as "auto" or "manual" when access is gated.
fn gated_flag(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Bool(true) | Value::String(_)))
}

/// Recent results by URL, kept for CACHE_TTL.
struct Cache<T> {
    entries: Mutex<HashMap<String, (Instant, T)>>,
}

impl<T: Clone> Cache<T> {
    fn new() -> Self {
        Cache {
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, key: &str) -> Option<T> {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries
            .get(key)
            .filter(|(at, _)| at.elapsed() < CACHE_TTL)
            .map(|(_, value)| value.clone())
    }

    fn put(&self, key: String, value: T) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
        entries.insert(key, (Instant::now(), value));
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawModel {
    id: Option<String>,
    model_id: Option<String>,
    downloads: Option<u64>,
    likes: Option<u64>,
    gated: Option<Value>,
    last_modified: Option<String>,
}

impl RawModel {
    fn into_summary(self) -> Option<RepoSummary> {
        Some(RepoSummary {
            id: self.id.or(self.model_id)?,
            downloads: self.downloads.unwrap_or(0),
            likes: self.likes.unwrap_or(0),
            gated: gated_flag(self.gated.as_ref()),
            updated: self.last_modified,
        })
    }
}

#[derive(Deserialize)]
struct RawMeta {
    gated: Option<Value>,
}

#[derive(Deserialize)]
struct RawEntry {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    size: Option<u64>,
    lfs: Option<RawLfs>,
}

#[derive(Deserialize)]
struct RawLfs {
    oid: Option<String>,
    size: Option<u64>,
}

impl RawEntry {
    fn into_file(self) -> RepoFile {
        let (sha256, lfs_size) = match self.lfs {
            Some(lfs) => (lfs.oid, lfs.size),
            None => (None, None),
        };
        RepoFile {
            path: self.path,
            size: lfs_size.or(self.size).unwrap_or(0),
            sha256,
        }
    }
}
