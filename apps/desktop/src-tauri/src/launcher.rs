use std::fs::File;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use vatra_service::{ServiceInfo, existing_service};

const READY_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const LOG_FILE: &str = "service.log";
const LOG_EXCERPT_CHARS: usize = 1_000;

/// Resolve an explicit profile before touching the filesystem or considering the live profile.
pub fn resolve_data_dir(override_path: Option<PathBuf>) -> Result<PathBuf, String> {
    let Some(path) = override_path else { return vatra_service::default_data_dir() };
    let spelling = path.to_string_lossy().replace('/', "\\");
    if !path.is_absolute() || spelling.starts_with("\\\\") || spelling.contains('\0') {
        return Err("--data-dir must be an absolute local folder without a network or extended-path prefix.".into());
    }
    match std::fs::metadata(&path) {
        Ok(metadata) if !metadata.is_dir() => Err("--data-dir must name a folder, not a file.".into()),
        Ok(_) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(path),
        Err(error) => Err(format!("The selected profile couldn't be accessed: {error}")),
    }
}

/// Selected once at startup; reconnection never falls back to another profile.
pub struct DesktopProfile {
    directory: PathBuf,
}

impl DesktopProfile {
    pub fn from_args(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut args = args.into_iter().skip(1);
        let mut selected = None;
        while let Some(argument) = args.next() {
            let path = if argument == "--data-dir" {
                args.next().map(PathBuf::from).ok_or("--data-dir requires a folder.")?
            } else if let Some(value) = argument.to_str().and_then(|arg| arg.strip_prefix("--data-dir=")) {
                PathBuf::from(value)
            } else {
                return Err("Unknown desktop argument. Use --data-dir <absolute-local-folder>.".into());
            };
            if selected.replace(path).is_some() {
                return Err("Give --data-dir only once.".into());
            }
        }
        Ok(Self { directory: resolve_data_dir(selected)? })
    }

    pub fn directory(&self) -> &Path { &self.directory }

    pub async fn ensure_service(&self, binary: &Path) -> Result<ServiceInfo, LauncherError> {
        ensure_service(&self.directory, binary).await
    }
}

#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

#[derive(Debug, thiserror::Error)]
pub enum LauncherError {
    #[error("couldn't start the Vatra service from `{0}`: {1}")]
    Spawn(PathBuf, String),
    #[error("the Vatra service didn't start in time. {0}")]
    NotReady(String),
}

pub async fn ensure_service(data_dir: &Path, binary: &Path) -> Result<ServiceInfo, LauncherError> {
    ensure_service_with_timeout(data_dir, binary, READY_TIMEOUT).await
}

pub async fn ensure_service_with_timeout(
    data_dir: &Path,
    binary: &Path,
    timeout: Duration,
) -> Result<ServiceInfo, LauncherError> {
    if let Some(info) = existing_service(data_dir).await {
        return Ok(info);
    }
    spawn_detached(data_dir, binary)?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(info) = existing_service(data_dir).await {
            return Ok(info);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let printed = std::fs::read_to_string(data_dir.join(LOG_FILE)).unwrap_or_default();
    let details: String = printed.trim().chars().take(LOG_EXCERPT_CHARS).collect();
    Err(LauncherError::NotReady(format!("Details: {details}")))
}

/// The service must outlive the window, so it isn't tied to this process in any way.
fn spawn_detached(data_dir: &Path, binary: &Path) -> Result<(), LauncherError> {
    let spawn_error = |error: std::io::Error| LauncherError::Spawn(binary.to_path_buf(), error.to_string());
    std::fs::create_dir_all(data_dir).map_err(spawn_error)?;
    let log = File::create(data_dir.join(LOG_FILE)).map_err(spawn_error)?;
    let log_for_errors = log.try_clone().map_err(spawn_error)?;
    let mut command = Command::new(binary);
    command
        .arg("serve")
        .arg("--data-dir")
        .arg(data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_for_errors));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    command.spawn().map(drop).map_err(spawn_error)
}
