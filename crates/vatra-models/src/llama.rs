//! Finding llama.cpp and reading what the installed build can do.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::hardware::{Gpu, GpuSource};

const RUN_TIMEOUT: Duration = Duration::from_secs(10);
const MIB: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlamaInstall {
    pub server: PathBuf,
    pub fit_params: Option<PathBuf>,
    pub build: Option<u32>,
    pub commit: Option<String>,
    pub caps: LlamaCaps,
}

/// The options this build understands, read from its `--help`, not guessed from the version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlamaCaps {
    pub fit: bool,
    pub fit_target: bool,
    pub fit_ctx: bool,
    pub jinja: bool,
    pub list_devices: bool,
    pub mmproj: bool,
    pub no_webui: bool,
    pub offline: bool,
}

pub fn parse_help(help: &str) -> LlamaCaps {
    let has = |flag: &str| {
        help.split(|c: char| c.is_whitespace() || c == ',')
            .any(|word| word == flag)
    };
    LlamaCaps {
        fit: has("--fit"),
        fit_target: has("--fit-target"),
        fit_ctx: has("--fit-ctx"),
        jinja: has("--jinja"),
        list_devices: has("--list-devices"),
        mmproj: has("--mmproj"),
        no_webui: has("--no-webui"),
        offline: has("--offline"),
    }
}

/// `version: 0.8.0-dev (build 12019, commit 1052f7e1d)`; older builds print `version: 4567 (abc1234)`.
pub fn parse_version(text: &str) -> (Option<u32>, Option<String>) {
    let Some(line) = text.lines().find(|line| line.trim_start().starts_with("version:")) else {
        return (None, None);
    };
    let build = line
        .split("build ")
        .nth(1)
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|digits| digits.parse().ok())
        .or_else(|| {
            line.trim_start()
                .trim_start_matches("version:")
                .split_whitespace()
                .next()
                .and_then(|word| word.parse().ok())
        });
    let commit = line
        .split("commit ")
        .nth(1)
        .or_else(|| line.split('(').nth(1))
        .map(|rest| rest.trim_end_matches(')').split([')', ',']).next().unwrap_or("").trim().to_string())
        .filter(|commit| !commit.is_empty() && commit.chars().all(|c| c.is_ascii_hexdigit()));
    (build, commit)
}

/// Lines like `  Vulkan0: NVIDIA GeForce RTX 5060 (7895 MiB, 7127 MiB free)`.
pub fn parse_devices(text: &str) -> Vec<Gpu> {
    text.lines()
        .filter_map(|line| {
            let (backend, rest) = line.trim().split_once(": ")?;
            if backend.is_empty() || backend.contains(' ') {
                return None;
            }
            let open = rest.rfind('(')?;
            let name = rest[..open].trim().to_string();
            let numbers: Vec<u64> = rest[open + 1..]
                .trim_end_matches(')')
                .split(',')
                .filter_map(|part| part.split_whitespace().next()?.parse().ok())
                .collect();
            let total = *numbers.first()? * MIB;
            Some(Gpu {
                name,
                backend: Some(backend.to_string()),
                total,
                free: numbers.get(1).map(|free| free * MIB),
                source: GpuSource::Llama,
                integrated: false,
            })
        })
        .collect()
}

/// The configured path first, then PATH, then the folder winget installs into.
pub fn find(configured: Option<&Path>) -> Option<LlamaInstall> {
    let server = configured
        .map(Path::to_path_buf)
        .filter(|path| path.is_file())
        .or_else(|| on_path(SERVER))
        .or_else(winget_server)?;
    Some(inspect(server))
}

/// Reads the version and options of a known `llama-server`.
pub fn inspect(server: PathBuf) -> LlamaInstall {
    let (build, commit) = run(&server, &["--version"]).map(|out| parse_version(&out)).unwrap_or_default();
    let caps = run(&server, &["--help"]).map(|out| parse_help(&out)).unwrap_or_default();
    let fit_params = server
        .parent()
        .map(|dir| dir.join(FIT_PARAMS))
        .filter(|path| path.is_file());
    LlamaInstall { server, fit_params, build, commit, caps }
}

/// The devices llama.cpp can use, as it sees them. Empty when the build can't list them.
pub fn devices(install: &LlamaInstall) -> Vec<Gpu> {
    if !install.caps.list_devices {
        return Vec::new();
    }
    run(&install.server, &["--list-devices"]).map(|out| parse_devices(&out)).unwrap_or_default()
}

#[cfg(windows)]
const SERVER: &str = "llama-server.exe";
#[cfg(not(windows))]
const SERVER: &str = "llama-server";
#[cfg(windows)]
const FIT_PARAMS: &str = "llama-fit-params.exe";
#[cfg(not(windows))]
const FIT_PARAMS: &str = "llama-fit-params";

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

fn winget_server() -> Option<PathBuf> {
    let packages = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(r"Microsoft\WinGet\Packages");
    std::fs::read_dir(packages)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("ggml.llamacpp_"))
        .map(|entry| entry.path().join(SERVER))
        .find(|path| path.is_file())
}

/// Runs a short command and returns stdout and stderr together; None on failure or after the timeout.
pub(crate) fn run(program: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().ok()?;
    let readers = [
        child.stdout.take().map(|mut pipe| std::thread::spawn(move || read_all(&mut pipe))),
        child.stderr.take().map(|mut pipe| std::thread::spawn(move || read_all(&mut pipe))),
    ];
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < RUN_TIMEOUT => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut output = String::new();
    for reader in readers.into_iter().flatten() {
        output.push_str(&reader.join().unwrap_or_default());
    }
    Some(output)
}

fn read_all(pipe: &mut impl std::io::Read) -> String {
    let mut bytes = Vec::new();
    let _ = pipe.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}
