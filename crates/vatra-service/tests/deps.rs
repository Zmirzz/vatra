//! Installing programs through winget, against a fake winget.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use vatra_service::deps::{DepError, PACKAGES, install, install_args, status};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-winget");

#[tokio::test]
async fn rejects_unknown_id() {
    let error = install(Path::new(FAKE), "Evil.Package", Duration::from_secs(5), &|_| {}).await.unwrap_err();
    assert_eq!(error, DepError::NotAllowed);
}

#[tokio::test]
async fn exact_winget_arguments_and_progress() {
    let lines = Mutex::new(Vec::new());
    install(Path::new(FAKE), "astral-sh.uv", Duration::from_secs(10), &|line| lines.lock().unwrap().push(line)).await.unwrap();
    let lines = lines.into_inner().unwrap();
    assert_eq!(
        lines[0],
        "ARGS install --id astral-sh.uv --exact --scope user --accept-package-agreements --accept-source-agreements --silent --disable-interactivity"
    );
    assert!(lines.contains(&"Successfully installed".to_string()));
    assert!(lines.iter().all(|line| !line.contains('█')), "progress bars are dropped");
}

#[tokio::test]
async fn failure_keeps_last_lines() {
    let Err(DepError::Failed { code, tail }) = install(Path::new(FAKE), "Git.Git", Duration::from_secs(10), &|_| {}).await else {
        panic!("expected a failure")
    };
    assert_eq!(code, 1);
    assert!(tail.iter().any(|line| line.contains("1603")));
}

#[tokio::test]
async fn stops_after_the_timeout() {
    let started = std::time::Instant::now();
    let error = install(Path::new(FAKE), "Python.Python.3.12", Duration::from_millis(800), &|_| {}).await.unwrap_err();
    assert_eq!(error, DepError::Timeout);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[cfg(windows)]
#[tokio::test]
async fn timeout_ends_installer_children() {
    let dir=tempfile::tempdir().unwrap();
    let pid_file=dir.path().join("child.pid");
    let marker=dir.path().join("survived");
    // The fake installer deliberately leaves a descendant behind after the parent is killed.
    let script=dir.path().join("installer.cmd");
    std::fs::write(&script,format!("@echo off\r\npowershell -NoProfile -NonInteractive -Command \"$p = Start-Process powershell -WindowStyle Hidden -PassThru -ArgumentList '-NoProfile -NonInteractive -Command Start-Sleep 2; Set-Content -LiteralPath ''{}'' survived'; Set-Content -LiteralPath '{}' $p.Id; Start-Sleep 30\"\r\n",marker.display(),pid_file.display())).unwrap();
    let error=install(&script,"astral-sh.uv",Duration::from_millis(1000),&|_|{}).await.unwrap_err();
    assert_eq!(error,DepError::Timeout);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!marker.exists(),"installer descendant survived timeout");
}

#[test]
fn status_finds_programs_on_the_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("uv.exe"), b"").unwrap();
    let list = status(&std::env::join_paths([dir.path()]).unwrap());
    assert_eq!(list.len(), PACKAGES.len());
    let uv = list.iter().find(|item| item.package.id == "astral-sh.uv").unwrap();
    assert!(uv.installed);
    assert_eq!(uv.command, format!("winget {}", install_args("astral-sh.uv").join(" ")));
    assert!(!list.iter().find(|item| item.package.id == "Git.Git").unwrap().installed);
}

#[cfg(windows)]
#[test]
fn fresh_path_keeps_the_current_entries() {
    let fresh = vatra_service::deps::fresh_path();
    let current = std::env::var_os("PATH").unwrap();
    let fresh: Vec<_> = std::env::split_paths(&fresh).map(|dir| dir.to_string_lossy().to_lowercase()).collect();
    for dir in std::env::split_paths(&current) {
        assert!(fresh.contains(&dir.to_string_lossy().to_lowercase()), "{dir:?}");
    }
}

#[test]
fn install_request_needs_the_persons_agreement() {
    let refused = vatra_service::deps::install_request(serde_json::json!({ "id": "astral-sh.uv" })).unwrap_err();
    assert!(refused.1.contains("agreement"));
    let refused = vatra_service::deps::install_request(serde_json::json!({ "id": "astral-sh.uv", "consent": false })).unwrap_err();
    assert_eq!(refused.0, -32602);
    assert_eq!(vatra_service::deps::install_request(serde_json::json!({ "id": "astral-sh.uv", "consent": true })).unwrap(), "astral-sh.uv");
}
