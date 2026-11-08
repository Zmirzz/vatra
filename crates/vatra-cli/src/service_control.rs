//! Stop only a verified owned process through its authenticated service RPC. Never kill a process.
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownOutcome {
    NotRunning,
    Stopped,
    NotOwned,
}

pub async fn shutdown_owned_service(
    data_dir: &Path,
    expected_exe: &Path,
) -> Result<ShutdownOutcome, String> {
    let raw = match std::fs::read(data_dir.join("service.json")) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ShutdownOutcome::NotRunning);
        }
        Err(_) => return Err("The service record could not be read.".into()),
    };
    let info: vatra_service::ServiceInfo =
        serde_json::from_slice(&raw).map_err(|_| "The service record is invalid.".to_string())?;
    let Some(process) = platform::Process::capture(info.pid)? else {
        return Ok(ShutdownOutcome::NotRunning);
    };
    if !process.is_running()? {
        return Ok(ShutdownOutcome::NotRunning);
    }
    let actual = std::fs::canonicalize(process.image()?)
        .map_err(|_| "The service executable could not be verified.".to_string())?;
    let expected = match std::fs::canonicalize(expected_exe) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ShutdownOutcome::NotOwned),
        Err(_) => return Err("The expected executable could not be verified.".into()),
    };
    if !same_path(&actual, &expected) {
        return Ok(ShutdownOutcome::NotOwned);
    }
    if vatra_service::existing_service(data_dir).await.as_ref() != Some(&info) {
        return Err("The owned process did not answer as the recorded Vatra service.".into());
    }
    // Read authentication only after ownership was verified. Never pass it as an argument or print it.
    let token = std::fs::read_to_string(data_dir.join("token"))
        .map_err(|_| "The service authentication file could not be read.".to_string())?;
    let mut request = format!("ws://127.0.0.1:{}/rpc", info.port)
        .into_client_request()
        .map_err(|_| "Invalid service address.".to_string())?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        format!("vatra.bearer.{}", token.trim())
            .parse()
            .map_err(|_| "Invalid service authentication.".to_string())?,
    );
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), connect_async(request))
        .await
        .map_err(|_| "The service connection timed out.".to_string())?
        .map_err(|_| "The service rejected its authenticated connection.".to_string())?;
    socket
        .send(Message::Text(
            json!({"jsonrpc":"2.0","id":1,"method":"service.shutdown","params":{}})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|_| "The shutdown request could not be sent.".to_string())?;
    let answer = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(Ok(message)) = socket.next().await {
            if let Message::Text(text) = message
                && let Ok(frame) = serde_json::from_str::<Value>(&text)
                && frame["id"] == 1
            {
                return Some(frame);
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    if answer.is_some_and(|v| v.get("error").is_some()) {
        return Err("The service refused shutdown.".into());
    }
    // Shutdown may close the socket before its reply. Only actual process exit counts as stopped.
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if !process.is_running()? {
                return Ok(ShutdownOutcome::Stopped);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| "The service did not exit; replacement was refused.".to_string())?
}
fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}
/// Open for writing without truncating or changing bytes. Windows refuses this for a mapped executable.
pub fn ensure_executable_unlocked(exe: &Path) -> Result<(), String> {
    match std::fs::OpenOptions::new().write(true).open(exe) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => {
            Err("Vatra is still using an installation file. Close this copy and retry.".into())
        }
    }
}
#[cfg(windows)]
mod platform {
    use super::*;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            QueryFullProcessImageNameW, WaitForSingleObject,
        },
    };
    pub struct Process {
        handle: HANDLE,
    }
    impl Drop for Process {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
    impl Process {
        pub fn capture(pid: u32) -> Result<Option<Self>, String> {
            let handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    pid,
                )
            };
            if handle.is_null() {
                return if unsafe { GetLastError() } == 87 {
                    Ok(None)
                } else {
                    Err("The recorded process could not be inspected safely.".into())
                };
            }
            Ok(Some(Self { handle }))
        }
        pub fn image(&self) -> Result<PathBuf, String> {
            let mut buffer = vec![0u16; 32768];
            let mut count = buffer.len() as u32;
            if unsafe {
                QueryFullProcessImageNameW(self.handle, 0, buffer.as_mut_ptr(), &mut count)
            } == 0
            {
                return Err("The process executable could not be inspected.".into());
            }
            use std::os::windows::ffi::OsStringExt;
            Ok(PathBuf::from(std::ffi::OsString::from_wide(
                &buffer[..count as usize],
            )))
        }
        pub fn is_running(&self) -> Result<bool, String> {
            match unsafe { WaitForSingleObject(self.handle, 0) } {
                WAIT_OBJECT_0 => Ok(false),
                WAIT_TIMEOUT => Ok(true),
                _ => Err("The owned process state could not be checked.".into()),
            }
        }
    }
}
#[cfg(not(windows))]
mod platform {
    use super::*;
    pub struct Process {
        pid: u32,
    }
    impl Process {
        pub fn capture(pid: u32) -> Result<Option<Self>, String> {
            Ok(Path::new(&format!("/proc/{pid}"))
                .exists()
                .then_some(Self { pid }))
        }
        pub fn image(&self) -> Result<PathBuf, String> {
            std::fs::read_link(format!("/proc/{}/exe", self.pid))
                .map_err(|_| "The process executable could not be inspected.".into())
        }
        pub fn is_running(&self) -> Result<bool, String> {
            Ok(Path::new(&format!("/proc/{}", self.pid)).exists())
        }
    }
}
