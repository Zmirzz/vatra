//! Search by meaning, and the memory it gives the agents. An embedding account (any server that speaks the OpenAI
//! `/embeddings` protocol: a local llama.cpp or Ollama, or a hosted one) turns text into vectors. A background pass indexes
//! documents, notes, tasks and research reports as they change; a question is matched to them by meaning, not by words.
//! Nothing leaves the computer unless the account points at a hosted service the person chose.

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use vatra_core::{SecretStore, SemanticHit, Store};

use crate::connectors::{ConnectorKind, connector_secret};
use crate::service::Service;

const BATCH: usize = 16;
const PASS_SOURCES: usize = 12;

pub struct EmbeddingsKind;

#[derive(Clone)]
pub struct Embedder {
    base: String,
    model: String,
    key: Option<String>,
}

fn base_of(settings: &Value) -> Result<String, String> {
    let base = settings.get("base_url").and_then(Value::as_str).unwrap_or("").trim().trim_end_matches('/').to_string();
    let url = url::Url::parse(&base).map_err(|_| "Enter the address of the embedding server, like http://localhost:11434/v1.".to_string())?;
    match url.scheme() {
        "https" => Ok(base),
        "http" if url.host_str().is_some_and(crate::email::net::is_local_host) => Ok(base),
        _ => Err("Use an https:// address, or an address on this computer or network.".into()),
    }
}

impl Embedder {
    pub fn from_settings(settings: &Value, key: Option<String>) -> Result<Embedder, String> {
        let model = settings.get("model").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if model.is_empty() || model.len() > 120 {
            return Err("Enter the name of the embedding model.".into());
        }
        Ok(Embedder { base: base_of(settings)?, model, key: key.filter(|k| !k.trim().is_empty()) })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// One vector per text, in order.
    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH) {
            let mut request = reqwest::Client::new()
                .post(format!("{}/embeddings", self.base))
                .timeout(Duration::from_secs(60))
                .json(&json!({"model": self.model, "input": batch}));
            if let Some(key) = &self.key {
                request = request.bearer_auth(key);
            }
            let response = request.send().await.map_err(|_| "The embedding server couldn't be reached.".to_string())?;
            let status = response.status();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err("The embedding server refused the key.".into());
            }
            if !status.is_success() {
                return Err(format!("The embedding server answered {}. Check the model name.", status.as_u16()));
            }
            let body: Value = response.json().await.map_err(|_| "The embedding server answered something unreadable.".to_string())?;
            let mut rows: Vec<(usize, Vec<f32>)> = body["data"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .filter_map(|(i, row)| {
                    let vector: Vec<f32> = row["embedding"].as_array()?.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect();
                    (!vector.is_empty()).then_some((row["index"].as_u64().map_or(i, |n| n as usize), vector))
                })
                .collect();
            rows.sort_by_key(|(i, _)| *i);
            if rows.len() != batch.len() {
                return Err("The embedding server returned the wrong number of vectors.".into());
            }
            out.extend(rows.into_iter().map(|(_, v)| v));
        }
        Ok(out)
    }
}

#[async_trait]
impl ConnectorKind for EmbeddingsKind {
    fn kind(&self) -> &'static str {
        "embeddings"
    }
    fn secret_fields(&self) -> &'static [&'static str] {
        &["api_key"]
    }
    async fn check(&self, settings: &Value, secrets: &BTreeMap<String, String>) -> Result<(), String> {
        let embedder = Embedder::from_settings(settings, secrets.get("api_key").cloned())?;
        let vectors = embedder.embed(&["test".to_string()]).await?;
        match vectors.first() {
            Some(v) if v.len() >= 8 => Ok(()),
            _ => Err("The model gave no usable vector. Is it an embedding model?".into()),
        }
    }
}

/// The first embedding account, ready to use.
pub fn embedder(store: &Store, secrets: &dyn SecretStore) -> Option<Embedder> {
    let row = store.connectors().ok()?.into_iter().find(|r| r.kind == "embeddings")?;
    let settings: Value = serde_json::from_str(&row.settings_json).ok()?;
    let key = secrets.get(&connector_secret(&row.id, "api_key")).ok().flatten();
    Embedder::from_settings(&settings, key).ok()
}

/// Indexes what is new or changed. Returns how many sources were done; an error stops the pass and is recorded.
pub async fn index_pass(store: &Store, embedder: &Embedder, limit: usize) -> Result<usize, String> {
    let _ = store.semantic_prune();
    let pending = store.semantic_pending(embedder.model(), limit).map_err(|e| e.to_string())?;
    let mut done = 0;
    for source in pending {
        let title = source.title.trim();
        let passages: Vec<String> = vatra_core::chunk_text(&format!("{title}\n\n{}", source.text));
        if passages.is_empty() {
            store.semantic_store(&source, embedder.model(), vec![], crate::scheduler::now()).map_err(|e| e.to_string())?;
            done += 1;
            continue;
        }
        let vectors = embedder.embed(&passages).await?;
        store
            .semantic_store(&source, embedder.model(), passages.into_iter().zip(vectors).collect(), crate::scheduler::now())
            .map_err(|e| e.to_string())?;
        done += 1;
    }
    Ok(done)
}

/// The passages that match a question by meaning, as hits.
pub async fn search(store: &Store, embedder: &Embedder, question: &str, limit: usize, kinds: &[String], project: Option<i64>) -> Result<Vec<SemanticHit>, String> {
    let question = question.trim();
    if question.is_empty() || question.chars().count() > 1000 {
        return Err("Ask something of up to 1000 characters.".into());
    }
    let vectors = embedder.embed(&[question.to_string()]).await?;
    let query = vectors.first().ok_or("The model gave no vector.")?;
    store.semantic_search(embedder.model(), query, limit, kinds, project).map_err(|e| e.to_string())
}

pub struct SemanticLoop {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for SemanticLoop {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Indexes in the background while an embedding account exists, a little at a time, so a large library never blocks anything.
pub fn start(service: &Arc<Service>) -> SemanticLoop {
    let weak: Weak<Service> = Arc::downgrade(service);
    let task = tokio::spawn(async move {
        let mut ticks = tokio::time::interval(Duration::from_secs(20));
        loop {
            ticks.tick().await;
            let Some(service) = weak.upgrade() else { break };
            let secrets = service.agents().secrets();
            let Some(embedder) = embedder(service.store(), secrets.as_ref()) else { continue };
            let result = index_pass(service.store(), &embedder, PASS_SOURCES).await;
            if let Some(row) = service.store().connectors().unwrap_or_default().into_iter().find(|r| r.kind == "embeddings") {
                // A pass with nothing to do says nothing; a failure is shown on the Status page.
                match &result {
                    Ok(n) if *n > 0 => crate::health::record(service.store(), &format!("connector.{}", row.id), true, ""),
                    Err(message) => crate::health::record(service.store(), &format!("connector.{}", row.id), false, message),
                    _ => {}
                }
            }
        }
    });
    SemanticLoop { task }
}

pub async fn handle(service: &Arc<Service>, method: &str, params: Value) -> Result<Value, (i64, String)> {
    let store = service.store();
    let secrets = service.agents().secrets();
    match method {
        "semantic.status" => match embedder(store, secrets.as_ref()) {
            None => Ok(json!({"configured": false})),
            Some(e) => {
                let stats = store.semantic_stats(e.model()).map_err(|err| (1000, err.to_string()))?;
                Ok(json!({"configured": true, "model": e.model(), "sources": stats.sources, "chunks": stats.chunks, "pending": stats.pending}))
            }
        },
        "semantic.reindex" => {
            store.semantic_reset().map_err(|e| (1000, e.to_string()))?;
            Ok(json!({}))
        }
        "semantic.search" => {
            let question = params.get("query").and_then(Value::as_str).unwrap_or("");
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(8) as usize;
            let kinds: Vec<String> = params.get("kinds").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
            let project = params.get("project_id").and_then(Value::as_i64);
            let embedder = embedder(store, secrets.as_ref()).ok_or((1001, "Add an embedding account in Settings first.".to_string()))?;
            let hits = search(store, &embedder, question, limit, &kinds, project).await.map_err(|m| (1001, m))?;
            Ok(json!(hits))
        }
        _ => Err((-32601, format!("unknown method `{method}`"))),
    }
}
