//! Programs Vatra can install for the person through winget: llama.cpp for local models and the runtimes
//! extensions need. Only this fixed list, only the exact command shown to the person, and only after they agree.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const TAIL_LINES: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Package {
    /// The winget id.
    pub id: &'static str,
    pub name: &'static str,
    /// The commands it brings, used to tell whether it is already installed.
    pub provides: &'static [&'static str],
    /// What it is for, in the app's words.
    pub purpose: &'static str,
}

pub const PACKAGES: [Package; 5] = [
    Package { id: "ggml.llamacpp", name: "llama.cpp", provides: &["llama-server"], purpose: "localModels" },
    Package { id: "OpenJS.NodeJS.LTS", name: "Node.js", provides: &["node", "npx"], purpose: "extensions" },
    Package { id: "Python.Python.3.12", name: "Python", provides: &["python", "py"], purpose: "extensions" },
    Package { id: "astral-sh.uv", name: "uv", provides: &["uv", "uvx"], purpose: "extensions" },
    Package { id: "Git.Git", name: "Git", provides: &["git", "bash"], purpose: "extensions" },
];

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepStatus {
    #[serde(flatten)]
    pub package: Package,
    pub installed: bool,
    /// The exact command an install runs, shown before the person agrees.
    pub command: String,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DepError {
    #[error("Vatra only installs the programs on its list")]
    NotAllowed,
    #[error("winget isn't available on this computer. Install the program from its website instead.")]
    NoWinget,
    #[error("the install didn't finish (code {code}):\n{}", tail.join("\n"))]
    Failed { code: i32, tail: Vec<String> },
    #[error("the install took longer than 15 minutes and was stopped")]
    Timeout,
    #[error("couldn't start winget: {0}")]
    Spawn(String),
}

pub fn package(id: &str) -> Option<&'static Package> {
    PACKAGES.iter().find(|package| package.id == id)
}

/// The arguments of an install. `--accept-package-agreements` is only sent after the person agreed, and the app
/// says that it accepts the package's license.
pub fn install_args(id: &str) -> Vec<&str> {
    vec![
        "install",
        "--id",
        id,
        "--exact",
        "--scope",
        "user",
        "--accept-package-agreements",
        "--accept-source-agreements",
        "--silent",
        "--disable-interactivity",
    ]
}

pub fn status(path: &OsString) -> Vec<DepStatus> {
    let dirs: Vec<PathBuf> = std::env::split_paths(path).collect();
    PACKAGES
        .iter()
        .map(|package| {
            let installed = if package.id == "ggml.llamacpp" {
                vatra_models::llama::find(None).is_some()
            } else {
                package.provides.iter().any(|command| on_path(&dirs, command))
            };
            DepStatus { package: *package, installed, command: format!("winget {}", install_args(package.id).join(" ")) }
        })
        .collect()
}

fn on_path(dirs: &[PathBuf], command: &str) -> bool {
    dirs.iter().any(|dir| ["exe", "cmd", "bat", ""].iter().any(|ext| {
        let file = if ext.is_empty() { dir.join(command) } else { dir.join(format!("{command}.{ext}")) };
        file.is_file()
    }))
}

/// winget.exe from the PATH or from where Windows puts it.
pub fn find_winget() -> Option<PathBuf> {
    let from_path = std::env::var_os("PATH")
        .and_then(|path| std::env::split_paths(&path).map(|dir| dir.join("winget.exe")).find(|file| file.is_file()));
    from_path.or_else(|| {
        let file = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(r"Microsoft\WindowsApps\winget.exe");
        file.exists().then_some(file)
    })
}

/// Runs winget for one package of the list. Each line it prints goes to `progress`.
pub async fn install(
    winget: &Path,
    id: &str,
    timeout: Duration,
    progress: &(dyn Fn(String) + Send + Sync),
) -> Result<(), DepError> {
    package(id).ok_or(DepError::NotAllowed)?;
    let mut command = Command::new(winget);
    command.args(install_args(id)).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| DepError::Spawn(error.to_string()))?;
    let tree=vatra_core::ProcessGroup::new().map_err(|e|DepError::Spawn(e.to_string()))?;
    if let Err(error)=tree.adopt(&child) && !matches!(child.try_wait(),Ok(Some(_))) {
        let _=child.kill().await;
        return Err(DepError::Spawn(error.to_string()));
    }
    let (sender, mut lines) = tokio::sync::mpsc::unbounded_channel::<String>();
    for pipe in [
        child.stdout.take().map(|pipe| Box::new(pipe) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        child.stderr.take().map(|pipe| Box::new(pipe) as _),
    ]
    .into_iter()
    .flatten()
    {
        let sender = sender.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(pipe).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let _ = sender.send(line);
            }
        });
    }
    drop(sender);
    let mut tail: Vec<String> = Vec::new();
    let finished = tokio::time::timeout(timeout, async {
        while let Some(line) = lines.recv().await {
            // winget draws progress bars with carriage returns and block characters; keep the readable text.
            let line: String = line
                .split('\r')
                .map(|part| part.chars().filter(|c| !('\u{2580}'..='\u{259F}').contains(c)).collect::<String>())
                .map(|part| part.trim().to_string())
                .rfind(|part| !part.is_empty())
                .unwrap_or_default();
            // What's left of a bar, like "100%", says nothing the dialog doesn't show already.
            if !line.chars().any(char::is_alphabetic) {
                continue;
            }
            progress(line.clone());
            tail.push(line);
            if tail.len() > TAIL_LINES {
                tail.remove(0);
            }
        }
        child.wait().await
    })
    .await;
    match finished {
        Err(_) => {
            let _ = tree.kill_all();
            let _ = child.kill().await;
            Err(DepError::Timeout)
        }
        Ok(Err(error)) => Err(DepError::Spawn(error.to_string())),
        Ok(Ok(exit)) if exit.success() => Ok(()),
        Ok(Ok(exit)) => Err(DepError::Failed { code: exit.code().unwrap_or(-1), tail }),
    }
}

/// The PATH Windows would give a new program: the system's, then the user's, from the registry, followed by
/// what this process already had. A program winget just installed is found without restarting anything.
pub fn fresh_path() -> OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        for (root, key) in [
            (HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
            (HKEY_CURRENT_USER, "Environment"),
        ] {
            if let Some(value) = registry::read_string(root, key, "Path") {
                dirs.extend(std::env::split_paths(&OsString::from(value)));
            }
        }
    }
    if let Some(current) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&current));
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|dir| seen.insert(dir.to_string_lossy().to_lowercase()));
    std::env::join_paths(dirs).unwrap_or_default()
}

/// Makes the fresh PATH this process's, so the programs it starts find what was just installed.
pub fn apply_fresh_path() {
    let path = fresh_path();
    // SAFETY: on Windows, std sets variables through SetEnvironmentVariableW, which the OS serializes; no code
    // in this process keeps a pointer into the environment block.
    unsafe { std::env::set_var("PATH", path) };
}

#[cfg(windows)]
mod registry {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
    use windows_sys::Win32::System::Registry::{HKEY, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RRF_NOEXPAND, RegGetValueW};

    /// A string value, with `%VARIABLES%` expanded.
    pub fn read_string(root: HKEY, key: &str, value: &str) -> Option<String> {
        let key: Vec<u16> = std::ffi::OsStr::new(key).encode_wide().chain(Some(0)).collect();
        let value: Vec<u16> = std::ffi::OsStr::new(value).encode_wide().chain(Some(0)).collect();
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
        let mut len = 0u32;
        let status = unsafe {
            RegGetValueW(root, key.as_ptr(), value.as_ptr(), flags, std::ptr::null_mut(), std::ptr::null_mut(), &mut len)
        };
        if status != ERROR_SUCCESS || len == 0 {
            return None;
        }
        let mut buffer = vec![0u16; (len as usize).div_ceil(2)];
        let status = unsafe {
            RegGetValueW(root, key.as_ptr(), value.as_ptr(), flags, std::ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut len)
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let raw: Vec<u16> = buffer.into_iter().take_while(|unit| *unit != 0).chain(Some(0)).collect();
        let needed = unsafe { ExpandEnvironmentStringsW(raw.as_ptr(), std::ptr::null_mut(), 0) };
        let mut expanded = vec![0u16; needed as usize];
        let written = unsafe { ExpandEnvironmentStringsW(raw.as_ptr(), expanded.as_mut_ptr(), needed) };
        if written == 0 {
            return Some(String::from_utf16_lossy(&raw[..raw.len() - 1]));
        }
        Some(String::from_utf16_lossy(&expanded[..written as usize - 1]))
    }
}

#[derive(serde::Deserialize)]
struct InstallRequest {
    id: String,
    /// Set by the app only after the person saw the program, its source and the command and agreed.
    #[serde(default)]
    consent: bool,
}

/// The package id of a `deps.install` request, refused unless the person's agreement came with it.
pub fn install_request(params: serde_json::Value) -> Result<String, (i64, String)> {
    let request: InstallRequest = serde_json::from_value(params).map_err(|error| (-32602, error.to_string()))?;
    if !request.consent {
        return Err((-32602, "Installing a program needs the person's agreement.".into()));
    }
    Ok(request.id)
}

/// `deps.status` and `deps.install`. An install reports each line as a `deps_install` notification, then makes
/// the new PATH the service's and looks for llama.cpp again.
pub(crate) async fn handle(
    service: &std::sync::Arc<crate::Service>,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, (i64, String)> {
    match method {
        "deps.status" => Ok(serde_json::json!({ "winget": find_winget().is_some(), "packages": status(&fresh_path()) })),
        "deps.install" => {
            let id = install_request(params)?;
            let notify = |state: &str, line: Option<String>| {
                service.notify(crate::Notification::Models {
                    method: "deps_install",
                    params: serde_json::json!({ "id": id, "state": state, "line": line }),
                })
            };
            let winget = find_winget().ok_or_else(|| (-32000, DepError::NoWinget.to_string()))?;
            notify("started", None);
            let result = install(&winget, &id, INSTALL_TIMEOUT, &|line| notify("progress", Some(line))).await;
            apply_fresh_path();
            if let Some(models) = service.agents().models() {
                models.redetect_llama();
            }
            match result {
                Ok(()) => {
                    notify("done", None);
                    Ok(serde_json::json!({ "packages": status(&fresh_path()) }))
                }
                Err(error) => {
                    notify("failed", Some(error.to_string()));
                    Err((-32000, error.to_string()))
                }
            }
        }
        _ => Err((-32601, format!("unknown method `{method}`"))),
    }
}
