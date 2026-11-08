use std::path::{Path, PathBuf};
use std::time::Duration;

use vatra_desktop::{DesktopProfile, LauncherError, ensure_service, ensure_service_with_timeout, resolve_data_dir};
use vatra_service::Runtime;
use vatra_service::config::Config;

fn exits_immediately() -> PathBuf {
    PathBuf::from(std::env::var("WINDIR").unwrap()).join("System32").join("whoami.exe")
}

#[tokio::test]
async fn an_existing_service_is_reused_without_spawning() {
    let dir = tempfile::tempdir().unwrap();
    let running = Runtime::start(dir.path(), Config::empty()).await.unwrap();
    let info = ensure_service(dir.path(), Path::new("nu-exista.exe")).await.unwrap();
    assert_eq!(info.port, running.port());
}

#[tokio::test]
async fn a_missing_binary_gives_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let error = ensure_service(dir.path(), Path::new("nu-exista.exe")).await.unwrap_err();
    assert!(error.to_string().contains("nu-exista.exe"));
}

#[tokio::test]
async fn a_service_that_never_becomes_ready_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let error = ensure_service_with_timeout(dir.path(), &exits_immediately(), Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(matches!(error, LauncherError::NotReady(_)));
}

#[tokio::test]
async fn a_failed_start_reports_what_the_service_printed() {
    let dir = tempfile::tempdir().unwrap();
    let error = ensure_service_with_timeout(dir.path(), &exits_immediately(), Duration::from_secs(2))
        .await
        .unwrap_err();
    assert!(dir.path().join("service.log").exists());
    assert!(!std::fs::read_to_string(dir.path().join("service.log")).unwrap().is_empty());
    assert!(error.to_string().len() > 60, "{error}");
}

#[tokio::test]
async fn isolated_desktop_reconnect_uses_the_same_directory() {
    let selected = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let profile = DesktopProfile::from_args([
        "vatra-desktop".into(), "--data-dir".into(), selected.path().as_os_str().to_owned(),
    ]).unwrap();
    let first = Runtime::start(selected.path(), Config::empty()).await.unwrap();
    let unrelated = Runtime::start(other.path(), Config::empty()).await.unwrap();
    for _ in 0..2 {
        let connection = profile.ensure_service(Path::new("must-not-spawn.exe")).await.unwrap();
        assert_eq!(connection.port, first.port());
        assert_ne!(connection.port, unrelated.port());
    }
    drop(first);
    let restarted = Runtime::start(selected.path(), Config::empty()).await.unwrap();
    assert_eq!(profile.ensure_service(Path::new("must-not-spawn.exe")).await.unwrap().port, restarted.port());
}

#[test]
fn an_invalid_override_never_falls_back_to_live_profile() {
    for path in ["relative", "", r"\\server\share\vatra", r"\\?\C:\vatra", "//server/share/vatra", r"C:relative"] {
        assert!(resolve_data_dir(Some(PathBuf::from(path))).is_err(), "{path}");
        assert!(DesktopProfile::from_args(["vatra-desktop".into(), "--data-dir".into(), path.into()]).is_err());
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not-a-directory");
    std::fs::write(&file,"data").unwrap();
    assert!(resolve_data_dir(Some(file)).is_err());
    assert!(DesktopProfile::from_args(["vatra-desktop".into(), "--data-dir".into()]).is_err());
    assert!(DesktopProfile::from_args(["vatra-desktop".into(), "--data-dir=relative".into()]).is_err());
}
