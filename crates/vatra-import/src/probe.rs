use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::future::join_all;
use tokio::process::Command;
use tokio::time::timeout;
use vatra_core::{McpServerConfig, SecretRef};

use crate::scan::{Found, FoundKind};

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const VERSION_FLAG: &str = "--version";
pub const NOT_CHECKED: &str = "not checked until you approve it";

/// The only programs a check starts. Any other program name is looked up on the PATH and never run.
const KNOWN_RUNTIMES: [&str; 12] = [
    "node", "npx", "python", "python3", "py", "uv", "uvx", "bash", "pwsh", "git", "deno", "bun",
];

/// Commands that belong to the shell, so they need no program of their own.
const SHELL_BUILTINS: [&str; 13] = [
    "[", "cd", "echo", "exit", "export", "false", "printf", "pwd", "read", "set", "test", "true", "unset",
];

/// Script files by extension, and the program that runs each.
const SCRIPT_RUNNERS: [(&str, &str); 4] = [("py", "python"), ("sh", "bash"), ("mjs", "node"), ("js", "node")];

/// Extensions a program can have on Windows, as `Command` does not add `.cmd` and `.bat` by itself.
const PATH_EXTENSIONS: [&str; 3] = ["exe", "cmd", "bat"];

/// What a check found. `Missing` and `NeedsValue` say what is missing; `NotChecked` means nothing was started, and
/// the item needs the user's approval before it can be checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeResult {
    Ok,
    Missing(String),
    NeedsValue(String),
    NotChecked(String),
}

/// A program a catalog item runs, by the name it is started with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub name: String,
}

/// The checks of the import: whether a program runs, whether a name is on the PATH, and whether an MCP server can
/// start. A check that cannot finish is a failure, so it is reported as missing.
#[async_trait]
pub trait Probe: Send + Sync {
    /// Starts `program` with `args` and reports whether it exits cleanly.
    async fn runs(&self, program: &str, args: &[&str]) -> ProbeResult;
    /// Looks `program` up on the PATH without starting it.
    async fn on_path(&self, program: &str) -> ProbeResult;
    async fn mcp_starts(&self, server: &McpServerConfig) -> ProbeResult;
}

/// The checks of this machine. A program runs under a ten second timeout, in a process group on Windows, so a hang
/// ends with the check. A stdio MCP server is never started by a check: it waits until the user approves it.
pub struct SystemProbe {
    timeout: Duration,
}

impl SystemProbe {
    pub fn new() -> SystemProbe {
        SystemProbe::with_timeout(PROBE_TIMEOUT)
    }

    /// The same checks with another timeout. Tests use a short one.
    pub fn with_timeout(timeout: Duration) -> SystemProbe {
        SystemProbe { timeout }
    }
}

impl Default for SystemProbe {
    fn default() -> SystemProbe {
        SystemProbe::new()
    }
}

#[async_trait]
impl Probe for SystemProbe {
    async fn runs(&self, program: &str, args: &[&str]) -> ProbeResult {
        let mut command = Command::new(resolve(program));
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => return ProbeResult::Missing(format!("{program} was not found")),
        };
        // Lives as long as this check: when it ends, the program's whole process tree ends with it.
        #[cfg(windows)]
        let _tree = vatra_core::ProcessGroup::new()
            .and_then(|tree| tree.adopt(&child).map(|()| tree))
            .ok();
        match timeout(self.timeout, child.wait()).await {
            Ok(Ok(status)) if status.success() => ProbeResult::Ok,
            Ok(Ok(status)) => ProbeResult::Missing(format!("{program} did not run ({status})")),
            Ok(Err(error)) => ProbeResult::Missing(format!("{program} did not run: {error}")),
            Err(_) => ProbeResult::Missing(format!("{program} did not answer in time")),
        }
    }

    async fn on_path(&self, program: &str) -> ProbeResult {
        match locate(program) {
            Some(_) => ProbeResult::Ok,
            None => ProbeResult::Missing(format!("{program} was not found")),
        }
    }

    async fn mcp_starts(&self, server: &McpServerConfig) -> ProbeResult {
        if let Some(variable) = unset_values(server).first() {
            return ProbeResult::NeedsValue(format!("{} needs a value for {variable}", server.name));
        }
        match server.command {
            Some(_) => ProbeResult::NotChecked(NOT_CHECKED.into()),
            None => ProbeResult::Ok,
        }
    }
}

/// The file a program name resolves to on the PATH. On Windows `.exe`, `.cmd` and `.bat` are tried, since `Command`
/// finds only `.exe` and npm installs `.cmd` shims.
fn locate(program: &str) -> Option<PathBuf> {
    let path_variable = std::env::var_os("PATH")?;
    let extensions: &[&str] = if cfg!(windows) { &PATH_EXTENSIONS } else { &[] };
    std::env::split_paths(&path_variable).find_map(|directory| {
        if extensions.is_empty() {
            let candidate = directory.join(program);
            return candidate.is_file().then_some(candidate);
        }
        extensions.iter().find_map(|extension| {
            let candidate = directory.join(program).with_extension(extension);
            candidate.is_file().then_some(candidate)
        })
    })
}

fn resolve(program: &str) -> PathBuf {
    locate(program).unwrap_or_else(|| PathBuf::from(program))
}

/// The environment variables an MCP server reads that are not set: the ones its secrets name and the `${NAME}`
/// references in its env and headers. Plugin paths and plugin settings are not environment variables.
fn unset_values(server: &McpServerConfig) -> Vec<String> {
    let mut names: Vec<String> = server
        .secrets
        .values()
        .filter_map(|reference| match reference {
            SecretRef::Env(variable) => Some(variable.clone()),
            SecretRef::Keychain(_) => None,
        })
        .collect();
    names.extend(
        server
            .env
            .values()
            .chain(server.headers.values())
            .flat_map(|text| references(text)),
    );
    names.sort();
    names.dedup();
    names.retain(|name| std::env::var_os(name).is_none());
    names
}

/// The names in `${NAME}` references of a value. Only plain names count, so `${CLAUDE_PLUGIN_ROOT}` does not.
fn references(text: &str) -> Vec<String> {
    text.split("${")
        .skip(1)
        .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
        .filter(|name| !name.starts_with("CLAUDE_PLUGIN_") && is_env_name(name))
        .map(str::to_string)
        .collect()
}

fn is_env_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A command word that is a path, not a name on the PATH. A path is never started by a check.
fn is_path(program: &str) -> bool {
    program.contains(['/', '\\'])
}

/// `NAME=value` before a command, which sets a variable and is not the program.
fn is_assignment(word: &str) -> bool {
    word.split_once('=')
        .is_some_and(|(name, _)| is_env_name(name) && !name.starts_with(|c: char| c.is_ascii_digit()))
}

/// The programs a catalog item runs: the first word of each command after any `NAME=value` prefix, and the program of
/// each script file the command names by extension. Commands are split at `&&`, `||`, `|` and `;`, so a redirection
/// such as `2>&1` stays in its command. Shell builtins need no program. Hooks are not probed, since they are not run.
pub fn needed_programs(found: &Found) -> Vec<Program> {
    let mut names: Vec<String> = Vec::new();
    for run in &found.runs_code {
        let chained = run.command.replace("&&", ";").replace("||", ";");
        for segment in chained.split(['|', ';']) {
            let words: Vec<&str> = segment
                .split_whitespace()
                .map(|word| word.trim_matches(['"', '\'']))
                .collect();
            let mut candidates = Vec::new();
            if let Some(program) = words.iter().copied().find(|word| !is_assignment(word)) {
                candidates.push(script_runner(program).unwrap_or(program));
            }
            candidates.extend(words.iter().filter_map(|word| script_runner(word)));
            for program in candidates {
                if program.is_empty() || SHELL_BUILTINS.contains(&program) || names.iter().any(|name| name == program) {
                    continue;
                }
                names.push(program.to_string());
            }
        }
    }
    names.into_iter().map(|name| Program { name }).collect()
}

fn script_runner(word: &str) -> Option<&'static str> {
    let extension = Path::new(word).extension()?.to_str()?;
    SCRIPT_RUNNERS
        .iter()
        .find(|(known, _)| *known == extension)
        .map(|(_, program)| *program)
}

/// What the checks found for a set of items: each program by name, each MCP server by its source and name, and the
/// items the user has approved to run their commands, by source and name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Probes {
    pub programs: BTreeMap<String, ProbeResult>,
    pub servers: BTreeMap<(PathBuf, String), ProbeResult>,
    pub approved: BTreeSet<(PathBuf, String)>,
}

/// Runs the checks for `items`, all at once: each program they need once, and each MCP server's start check. A
/// program is started only when it is one of `KNOWN_RUNTIMES`; another bare name is only looked up on the PATH; a path
/// is never started and is `NotChecked`. Items that Vatra does not run are not checked.
pub async fn probe_items(probe: &dyn Probe, items: &[Found]) -> Probes {
    let runnable: Vec<&Found> = items.iter().filter(|item| item.unsupported.is_none()).collect();
    let programs: BTreeSet<String> = runnable
        .iter()
        .flat_map(|item| needed_programs(item))
        .map(|program| program.name)
        .collect();
    let servers: Vec<((PathBuf, String), McpServerConfig)> = runnable
        .iter()
        .filter(|item| item.kind == FoundKind::McpServer)
        .filter_map(|item| {
            let server = serde_json::from_value::<McpServerConfig>(item.details.clone()).ok()?;
            Some(((item.source.clone(), item.name.clone()), server))
        })
        .collect();

    let (program_results, server_results) = tokio::join!(
        join_all(
            programs
                .iter()
                .map(|name| async move { (name.clone(), check_program(probe, name).await) })
        ),
        join_all(
            servers
                .iter()
                .map(|(key, server)| async move { (key.clone(), probe.mcp_starts(server).await) })
        ),
    );
    Probes {
        programs: program_results.into_iter().collect(),
        servers: server_results.into_iter().collect(),
        approved: BTreeSet::new(),
    }
}

async fn check_program(probe: &dyn Probe, program: &str) -> ProbeResult {
    if is_path(program) {
        return ProbeResult::NotChecked(NOT_CHECKED.into());
    }
    if KNOWN_RUNTIMES.contains(&program) {
        return probe.runs(program, &[VERSION_FLAG]).await;
    }
    probe.on_path(program).await
}
