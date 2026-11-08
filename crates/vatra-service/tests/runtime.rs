use std::collections::BTreeMap;
use std::path::Path;

use vatra_service::config::Config;
use vatra_service::{Runtime, RuntimeError, existing_service};

fn config() -> Config {
    Config {
        agents: BTreeMap::new(),
        summarizer: String::new(),
        mcp: vec![],
        web_search: None,
        engine: Default::default(),
        local: Default::default(),
    }
}

fn write_service_file(directory: &Path, pid: u32, port: u16) {
    let text = serde_json::json!({ "pid": pid, "port": port }).to_string();
    std::fs::write(directory.join("service.json"), text).unwrap();
}

#[tokio::test]
async fn second_start_reuses_the_running_service() {
    let dir = tempfile::tempdir().unwrap();
    let first = Runtime::start(dir.path(), config()).await.unwrap();
    assert_eq!(existing_service(dir.path()).await.unwrap().port, first.port());
    assert!(matches!(Runtime::start(dir.path(), config()).await, Err(RuntimeError::AlreadyRunning)));
}

#[tokio::test]
async fn stale_service_file_from_a_dead_process_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    write_service_file(dir.path(), 4_000_000, 1);
    assert!(existing_service(dir.path()).await.is_none());
    assert!(Runtime::start(dir.path(), config()).await.is_ok());
}

#[tokio::test]
async fn a_foreign_program_on_the_recorded_port_is_not_mistaken_for_vatra() {
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    write_service_file(dir.path(), std::process::id(), listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        loop {
            let _ = listener.accept().await;
        }
    });
    assert!(existing_service(dir.path()).await.is_none());
}

#[tokio::test]
async fn corrupt_service_file_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("service.json"), "{nu e json").unwrap();
    assert!(existing_service(dir.path()).await.is_none());
}

#[tokio::test]
async fn token_is_random_and_binding_is_loopback() {
    let first_dir = tempfile::tempdir().unwrap();
    let second_dir = tempfile::tempdir().unwrap();
    let service = Runtime::start(first_dir.path(), config()).await.unwrap();
    let other = Runtime::start(second_dir.path(), config()).await.unwrap();
    assert!(service.address().ip().is_loopback());
    let token = std::fs::read_to_string(first_dir.path().join("token")).unwrap();
    assert!(token.len() >= 32);
    assert_ne!(token, std::fs::read_to_string(second_dir.path().join("token")).unwrap());
    drop(other);
}

#[tokio::test]
async fn a_data_folder_with_spaces_and_diacritics_works() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("date cu spații");
    assert!(Runtime::start(&data, config()).await.is_ok());
    assert!(data.join("vatra.db").exists());
}

#[tokio::test]
async fn wait_returns_after_a_shutdown_request() {
    let dir = tempfile::tempdir().unwrap();
    let running = Runtime::start(dir.path(), config()).await.unwrap();
    running.service().request_shutdown();
    tokio::time::timeout(std::time::Duration::from_secs(5), running.wait()).await.unwrap();
}

#[test]
fn a_project_without_a_folder_works_in_its_own_folder_under_the_data_dir() {
    let dir = tempfile::tempdir().unwrap();
    let folder = vatra_service::project_workspace(dir.path(), 7).unwrap();
    assert!(folder.starts_with(dir.path()));
    assert!(folder.is_dir());
    assert_ne!(folder, vatra_service::project_workspace(dir.path(), 8).unwrap());
}

#[test]
fn projectless_chats_have_separate_session_workspaces() {
    let dir=tempfile::tempdir().unwrap();
    let a=vatra_service::session_workspace(dir.path(),17).unwrap();
    let b=vatra_service::session_workspace(dir.path(),18).unwrap();
    assert_ne!(a,b);
    assert!(a.starts_with(dir.path()));
    assert!(a.is_dir()&&b.is_dir());
    assert!(vatra_service::session_workspace(dir.path(),0).is_err());
}
