use std::time::Duration;
use vatra_cli::service_control::{ShutdownOutcome, shutdown_owned_service};

#[tokio::test]
async fn absent_profile_never_creates_files_or_starts_a_service() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing");
    assert_eq!(
        shutdown_owned_service(&path, &path.join("vatra.exe"))
            .await
            .unwrap(),
        ShutdownOutcome::NotRunning
    );
    assert!(!path.exists());
}

#[cfg(windows)]
#[test]
fn a_running_desktop_image_is_refused_without_changing_its_bytes() {
    assert!(
        vatra_cli::service_control::ensure_executable_unlocked(&std::env::current_exe().unwrap())
            .is_err()
    );
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file.exe");
    std::fs::write(&file, b"unchanged").unwrap();
    vatra_cli::service_control::ensure_executable_unlocked(&file).unwrap();
    assert_eq!(std::fs::read(file).unwrap(), b"unchanged");
}
#[cfg(windows)]
#[tokio::test]
async fn a_different_process_image_is_not_sent_a_shutdown_and_token_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("service.json"),
        serde_json::json!({"pid":std::process::id(),"port":9}).to_string(),
    )
    .unwrap();
    assert_eq!(
        shutdown_owned_service(
            dir.path(),
            std::path::Path::new(env!("CARGO_BIN_EXE_vatra"))
        )
        .await
        .unwrap(),
        ShutdownOutcome::NotOwned
    );
}
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn owned_child_service_exits_via_authenticated_rpc_without_a_process_kill() {
    let dir = tempfile::tempdir().unwrap();
    let exe = std::path::Path::new(env!("CARGO_BIN_EXE_vatra"));
    let mut command = tokio::process::Command::new(exe);
    command.args(["serve", "--data-dir"]).arg(dir.path());
    use std::os::windows::process::CommandExt;
    command.as_std_mut().creation_flags(0x08000000);
    let mut child = command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        while vatra_service::existing_service(dir.path()).await.is_none() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let unrelated = dir.path().join("another.exe");
    std::fs::write(&unrelated, "not the service").unwrap();
    let refused = shutdown_owned_service(dir.path(), &unrelated).await;
    let stopped = shutdown_owned_service(dir.path(), exe).await;
    let exited = tokio::time::timeout(Duration::from_secs(8), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refused.unwrap(), ShutdownOutcome::NotOwned);
    assert_eq!(stopped.unwrap(), ShutdownOutcome::Stopped);
    assert!(exited.success());
}
