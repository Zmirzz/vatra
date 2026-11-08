//! Scheduled research, the health list, phone notifications and the email contacts.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use vatra_core::{Agent, ConnectorRow, MemorySecretStore, NewScheduledTask, Schedule, SecretStore, Store, Summarizer};
use vatra_service::connectors::{ConnectorKind, connector_secret};
use vatra_service::ntfy::NtfyKind;
use vatra_service::research::WebSearchKind;
use vatra_service::{AgentFactory, BuildRequest, Service, health, scheduler};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Factory {
    secrets: Arc<MemorySecretStore>,
    asked: Mutex<Vec<String>>,
}

#[async_trait]
impl AgentFactory for Factory {
    fn names(&self) -> Vec<String> {
        vec!["alfa".into()]
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
    async fn complete_text(&self, _agent: Option<&str>, system: &str, user: &str) -> Result<String, String> {
        self.asked.lock().unwrap().push(system.to_string());
        if system.contains("search queries") {
            return Ok("pret abonament".into());
        }
        if system.contains("Extract from the web page") {
            return Ok(format!("Costa 49 lei ({}).", user.len()));
        }
        if system.contains("research report") {
            return Ok("Costa 49 lei [1].".into());
        }
        Ok("ok".into())
    }
}

fn service() -> (Arc<Service>, Arc<Factory>) {
    let factory = Arc::new(Factory { secrets: Arc::new(MemorySecretStore::default()), asked: Mutex::default() });
    (Service::new(Arc::new(Store::open_in_memory().unwrap()), factory.clone()).unwrap(), factory)
}

#[tokio::test]
async fn a_scheduled_research_runs_like_any_research_and_the_run_links_to_its_report() {
    let (service, _) = service();
    service.store().set_app_setting("search.duckduckgo", Some("off")).unwrap();
    // A search account pointing at a server on this machine.
    let server = MockServer::start().await;
    let page = format!("<html><body><p>{}</p></body></html>", "Abonamentul costa 49 lei pe luna pentru toate echipele. ".repeat(10));
    Mock::given(method("GET")).and(path("/p")).respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/html").set_body_string(page)).mount(&server).await;
    Mock::given(method("GET")).and(path("/search")).respond_with(ResponseTemplate::new(200).set_body_json(
        json!({"results": [{"title": "Pagina", "url": format!("{}/p", server.uri()), "content": "pret abonament"}]}))).mount(&server).await;
    service.store().save_connector(&ConnectorRow {
        id: "websearch-1".into(), kind: "websearch".into(), name: "Mine".into(),
        settings_json: json!({"provider": "searxng", "url": server.uri()}).to_string(), created_at: 1,
    }).unwrap();

    let id = service.store().create_scheduled_task(
        &NewScheduledTask {
            name: "Preturi".into(), prompt: "Cat costa abonamentul?".into(), agent: String::new(), project_id: None,
            schedule: Schedule::Once { at: 2000 }, kind: "research".into(), options: json!({"depth": "quick"}),
        },
        1000,
    ).unwrap();
    // The fetcher research uses refuses addresses on this machine, so the page can't be read here: the run must end
    // with a reason rather than hang, and still link to its research.
    scheduler::tick(&service, 2001).await;
    let mut run = None;
    for _ in 0..100 {
        run = service.store().scheduled_runs(Some(id), 5).unwrap().into_iter().find(|r| r.state != "running");
        if run.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let run = run.expect("the run ended");
    let research = run.research_id.expect("the run links to its research");
    let saved = service.store().research_run(research).unwrap().unwrap();
    assert_eq!(saved.question, "Cat costa abonamentul?");
    assert_eq!(run.state, "failed");
    assert!(run.summary.unwrap().to_lowercase().contains("private") || saved.error.is_some());
}

#[tokio::test]
async fn task_kinds_are_checked() {
    let (service, _) = service();
    let bad = |kind: &str, options: Value| NewScheduledTask {
        name: "x".into(), prompt: "y".into(), agent: "alfa".into(), project_id: None, schedule: Schedule::Once { at: 5 }, kind: kind.into(), options,
    };
    assert!(service.store().create_scheduled_task(&bad("magic", json!({})), 1).is_err());
    assert!(service.store().create_scheduled_task(&bad("research", json!({"depth": "bottomless"})), 1).is_err());
    assert!(service.store().create_scheduled_task(&bad("research", json!({"depth": "deep"})), 1).is_ok());
}

#[tokio::test]
async fn the_health_list_shows_what_failed_and_why() {
    let (service, _) = service();
    service.store().save_connector(&ConnectorRow { id: "email-1".into(), kind: "email".into(), name: "Personal".into(), settings_json: "{}".into(), created_at: 1 }).unwrap();
    let before = health::status(&service);
    let account = before["items"].as_array().unwrap().iter().find(|i| i["id"] == "connector.email-1").unwrap();
    assert_eq!(account["state"], "unknown");
    assert_eq!(account["retry"]["method"], "connectors.check");

    health::record(service.store(), "connector.email-1", false, "The mail server refused the user name or password.");
    health::record(service.store(), "search.duckduckgo", true, "");
    let after = health::status(&service);
    let account = after["items"].as_array().unwrap().iter().find(|i| i["id"] == "connector.email-1").unwrap();
    assert_eq!(account["state"], "down");
    assert!(account["detail"].as_str().unwrap().contains("refused"));
    assert_eq!(after["down"], 1);
    let ddg = after["items"].as_array().unwrap().iter().find(|i| i["id"] == "search.duckduckgo").unwrap();
    assert_eq!(ddg["state"], "ok");
    health::record(service.store(), "connector.email-1", true, "");
    assert_eq!(health::status(&service)["down"], 0);
}

#[tokio::test]
async fn phone_notifications_go_to_the_topic_with_the_token_and_the_check_explains_a_refusal() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/mytopic")).and(header("authorization", "Bearer tok")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
    Mock::given(method("POST")).and(path("/other")).respond_with(ResponseTemplate::new(403)).mount(&server).await;
    let secrets = |t: &str| BTreeMap::from([("token".to_string(), t.to_string())]);
    NtfyKind.check(&json!({"server": server.uri(), "topic": "mytopic"}), &secrets("tok")).await.unwrap();
    let refused = NtfyKind.check(&json!({"server": server.uri(), "topic": "other"}), &secrets("")).await.unwrap_err();
    assert!(refused.contains("403"));
    assert!(NtfyKind.check(&json!({"server": server.uri(), "topic": "a b"}), &secrets("")).await.is_err());
    assert!(NtfyKind.check(&json!({"server": "http://ntfy.example.com", "topic": "mytopic"}), &secrets("")).await.unwrap_err().contains("https"));

    // A reminder is sent through the saved account.
    let (service, factory) = service();
    service.store().save_connector(&ConnectorRow { id: "ntfy-1".into(), kind: "ntfy".into(), name: "Phone".into(), settings_json: json!({"server": server.uri(), "topic": "mytopic"}).to_string(), created_at: 1 }).unwrap();
    factory.secrets.set(&connector_secret("ntfy-1", "token"), "tok").unwrap();
    vatra_service::ntfy::notify(&service, "Reminder", "Suna-o pe Ana").await;
    let requests = server.received_requests().await.unwrap();
    let sent = requests.iter().find(|r| r.url.path() == "/mytopic" && String::from_utf8_lossy(&r.body).contains("Suna-o pe Ana"));
    assert!(sent.is_some());
    let item = health::status(&service)["items"].as_array().unwrap().iter().find(|i| i["id"] == "connector.ntfy-1").cloned().unwrap();
    assert_eq!(item["state"], "ok");
}

#[tokio::test]
async fn search_accounts_check_their_provider_and_refuse_unsafe_addresses() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/search")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [{"title": "t", "url": "https://x.example/a", "content": "weather today"}]}))).mount(&server).await;
    let none = BTreeMap::new();
    WebSearchKind.check(&json!({"provider": "searxng", "url": server.uri()}), &none).await.unwrap();
    assert!(WebSearchKind.check(&json!({"provider": "searxng", "url": "http://searx.example.com"}), &none).await.is_err());
    assert!(WebSearchKind.check(&json!({"provider": "tavily"}), &none).await.unwrap_err().contains("no key"));
    assert!(WebSearchKind.check(&json!({"provider": "bing"}), &none).await.is_err());
}

#[tokio::test]
async fn the_hardware_tiers_pick_light_mode_for_small_computers() {
    let (service, _) = service();
    // The test service has no model manager, so the recommendation says it does not know instead of guessing.
    let profile = vatra_service::health::research_profile(&service).await;
    assert_eq!(profile["tier"], "unknown");
    assert_eq!(profile["small_model"], false);
}
