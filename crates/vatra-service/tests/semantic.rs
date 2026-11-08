//! Search by meaning against a fake embedding server whose vectors follow topics, not words.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use vatra_core::{Agent, ConnectorRow, MemorySecretStore, NewItem, SecretStore, Store, Summarizer};
use vatra_service::connectors::{ConnectorKind, connector_secret};
use vatra_service::semantic::{self, EmbeddingsKind};
use vatra_service::{AgentFactory, BuildRequest, Service};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Words of one topic point the same way: food, money, travel. A text's vector counts the words of each topic.
struct Topics;

impl Respond for Topics {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let inputs: Vec<String> = match &body["input"] {
            Value::Array(list) => list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
            Value::String(one) => vec![one.clone()],
            _ => vec![],
        };
        let topics: [&[&str]; 3] = [&["supa", "ciorba", "pui", "mancare", "reteta"], &["chirie", "buget", "bani", "factura", "utilitati"], &["zbor", "hotel", "vacanta", "calatorie", "bilet"]];
        let data: Vec<Value> = inputs
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let lower = text.to_lowercase();
                let vector: Vec<f64> = topics.iter().map(|words| words.iter().map(|w| lower.matches(w).count() as f64).sum::<f64>() + 0.01).collect();
                json!({"index": i, "embedding": vector})
            })
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({"data": data}))
    }
}

struct Factory {
    secrets: Arc<MemorySecretStore>,
}

#[async_trait]
impl AgentFactory for Factory {
    fn names(&self) -> Vec<String> {
        vec![]
    }
    async fn build(&self, _r: BuildRequest) -> Result<Arc<dyn Agent>, String> {
        Err("none".into())
    }
    fn summarizer(&self) -> Arc<dyn Summarizer> {
        unreachable!()
    }
    fn secrets(&self) -> Arc<dyn SecretStore> {
        self.secrets.clone()
    }
}

async fn setup() -> (Arc<Service>, MockServer) {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v1/embeddings")).and(header("authorization", "Bearer emb-key")).respond_with(Topics).mount(&server).await;
    let factory = Arc::new(Factory { secrets: Arc::new(MemorySecretStore::default()) });
    let store = Arc::new(Store::open_in_memory().unwrap());
    store.save_connector(&ConnectorRow {
        id: "embeddings-1".into(), kind: "embeddings".into(), name: "Local".into(),
        settings_json: json!({"base_url": format!("{}/v1", server.uri()), "model": "topics-3"}).to_string(), created_at: 1,
    }).unwrap();
    factory.secrets.set(&connector_secret("embeddings-1", "api_key"), "emb-key").unwrap();
    (Service::new(store, factory).unwrap(), server)
}

fn note(service: &Service, project: i64, title: &str, body: &str) -> i64 {
    service.store().create_item(&NewItem { kind: "note".into(), title: title.into(), body: body.into(), fields: json!({"format": "text"}), project_id: Some(project) }, 1).unwrap()
}

#[tokio::test]
async fn a_question_finds_what_means_the_same_even_with_other_words() {
    let (service, _server) = setup().await;
    let project = service.store().create_project("Casa").unwrap();
    let food = note(&service, project, "Retete", "Supa de pui cu taietei si o ciorba de legume pentru duminica.");
    note(&service, project, "Buget", "Chirie, utilitati si factura la internet. Bani de pus deoparte.");
    note(&service, project, "Vacanta", "Zbor spre Lisabona, hotel aproape de centru si bilet de tren.");
    let report = service.store().create_research_run("Cat costa chiria in Cluj?", "quick", 1).unwrap();
    service.store().finish_research(report, "done", Some("Chiria medie este de 500 euro pe luna, plus utilitati [1]."), None, 2).unwrap();

    let secrets = service.agents().secrets();
    let embedder = semantic::embedder(service.store(), secrets.as_ref()).expect("an embedding account");
    assert_eq!(semantic::index_pass(service.store(), &embedder, 20).await.unwrap(), 4);
    assert_eq!(semantic::index_pass(service.store(), &embedder, 20).await.unwrap(), 0, "nothing changed, nothing is embedded again");

    // "mancare" never appears in the note, but it is about food.
    let hits = semantic::search(service.store(), &embedder, "ce pot sa gatesc de mancare", 3, &[], None).await.unwrap();
    assert_eq!(hits[0].title, "Retete");
    assert_eq!(hits[0].ref_id, food);
    assert!(hits[0].snippet.contains("Supa de pui"));

    // Money matches the note and the research report; the kinds filter picks one.
    let money = semantic::search(service.store(), &embedder, "bani pentru chirie", 5, &[], None).await.unwrap();
    let kinds: Vec<&str> = money.iter().take(2).map(|h| h.kind.as_str()).collect();
    assert!(kinds.contains(&"item") && kinds.contains(&"research"), "{money:?}");
    let only_reports = semantic::search(service.store(), &embedder, "bani pentru chirie", 5, &["research".into()], None).await.unwrap();
    assert!(only_reports.iter().all(|h| h.kind == "research"));
    assert_eq!(only_reports[0].title, "Cat costa chiria in Cluj?");
}

#[tokio::test]
async fn an_edited_note_is_indexed_again_and_a_deleted_one_leaves_the_results() {
    let (service, _server) = setup().await;
    let project = service.store().create_project("Casa").unwrap();
    let id = note(&service, project, "Idei", "Ceva despre zbor si hotel pentru vara.");
    let secrets = service.agents().secrets();
    let embedder = semantic::embedder(service.store(), secrets.as_ref()).unwrap();
    semantic::index_pass(service.store(), &embedder, 20).await.unwrap();
    let before = semantic::search(service.store(), &embedder, "vacanta", 3, &[], None).await.unwrap();
    assert_eq!(before[0].ref_id, id);

    // The same note, now about money: pending again, and it answers a money question.
    service.store().save_workspace_item(project, &vatra_core::ItemChange { id, expected_revision: 1, title: Some("Idei".into()), body: Some("Acum e despre chirie si buget lunar.".into()), fields: None }, 4).unwrap();
    assert_eq!(service.store().semantic_stats(embedder.model()).unwrap().pending, 1);
    semantic::index_pass(service.store(), &embedder, 20).await.unwrap();
    let after = semantic::search(service.store(), &embedder, "buget", 3, &[], None).await.unwrap();
    assert_eq!(after[0].ref_id, id);
    assert!(after[0].snippet.contains("chirie"));

    assert!(service.store().delete_item(id, 5).unwrap());
    semantic::index_pass(service.store(), &embedder, 20).await.unwrap();
    assert!(semantic::search(service.store(), &embedder, "buget", 3, &[], None).await.unwrap().is_empty());
}

#[tokio::test]
async fn the_rpc_methods_report_status_search_and_start_over() {
    let (service, _server) = setup().await;
    let project = service.store().create_project("Casa").unwrap();
    note(&service, project, "Retete", "Supa de pui.");
    let secrets = service.agents().secrets();
    let embedder = semantic::embedder(service.store(), secrets.as_ref()).unwrap();
    let before = semantic::handle(&service, "semantic.status", json!({})).await.unwrap();
    assert_eq!((before["configured"].as_bool(), before["pending"].as_i64()), (Some(true), Some(1)));
    semantic::index_pass(service.store(), &embedder, 20).await.unwrap();
    let after = semantic::handle(&service, "semantic.status", json!({})).await.unwrap();
    assert_eq!((after["sources"].as_i64(), after["pending"].as_i64()), (Some(1), Some(0)));
    let found = semantic::handle(&service, "semantic.search", json!({"query": "reteta de mancare", "limit": 3})).await.unwrap();
    assert_eq!(found[0]["title"], "Retete");
    assert!(semantic::handle(&service, "semantic.search", json!({"query": ""})).await.is_err());
    semantic::handle(&service, "semantic.reindex", json!({})).await.unwrap();
    assert_eq!(semantic::handle(&service, "semantic.status", json!({})).await.unwrap()["pending"], 1);
}

#[tokio::test]
async fn without_an_account_there_is_nothing_to_search_and_the_check_explains_problems() {
    let factory = Arc::new(Factory { secrets: Arc::new(MemorySecretStore::default()) });
    let service = Service::new(Arc::new(Store::open_in_memory().unwrap()), factory).unwrap();
    assert_eq!(semantic::handle(&service, "semantic.status", json!({})).await.unwrap()["configured"], false);
    assert!(semantic::handle(&service, "semantic.search", json!({"query": "ceva"})).await.unwrap_err().1.contains("embedding account"));

    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v1/embeddings")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
    let none = BTreeMap::new();
    let refused = EmbeddingsKind.check(&json!({"base_url": format!("{}/v1", server.uri()), "model": "nope"}), &none).await.unwrap_err();
    assert!(refused.contains("404") && refused.contains("model name"));
    assert!(EmbeddingsKind.check(&json!({"base_url": "http://embeddings.example.com/v1", "model": "m"}), &none).await.is_err());
    assert!(EmbeddingsKind.check(&json!({"base_url": server.uri(), "model": ""}), &none).await.is_err());
}
