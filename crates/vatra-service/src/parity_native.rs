//! Opt-in comparison through the original CLIs. No native configuration or credentials are written.
//! Native tool policies are not an OS sandbox; Codex is refused outright until its isolation is verified.
use std::{
    ffi::OsString,
    path::Path,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    Notification, Service,
    parity::{ParityRun, TaskResult},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vatra_core::{Event, ParityRunRow, ProcessGroup};
use vatra_parity::Task;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Native {
    ClaudeCode,
    Codex,
}

impl Native {
    pub fn id(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude_code",
            Self::Codex => "codex",
        }
    }
}

/// Arguments for the unmodified CLI, never a replacement login or a permission bypass.
pub fn native_args(kind: Native, model: &str, task: &Task, mcp_config: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = match kind {
        Native::ClaudeCode => {
            let mut tools = vec![
                "Read(/**)".to_string(),
                "Edit(/**)".into(),
                "Glob".into(),
                "Grep".into(),
                "TodoWrite".into(),
                "Skill".into(),
            ];
            tools.extend(
                task.allowed_commands
                    .iter()
                    .map(|command| format!("Bash({command})")),
            );
            if task.needs_mcp {
                tools.push("mcp__vatra-fixture-mcp__echo".into());
            }
            vec![
                "-p".into(),
                "--output-format".into(),
                "stream-json".into(),
                "--verbose".into(),
                "--model".into(),
                model.into(),
                "--permission-mode".into(),
                "default".into(),
                "--setting-sources".into(),
                "project".into(),
                "--tools".into(),
                "Read,Write,Edit,Glob,Grep,TodoWrite,Skill,Bash".into(),
                "--allowedTools".into(),
                tools.join(",").into(),
                "--strict-mcp-config".into(),
                "--mcp-config".into(),
                mcp_config.as_os_str().into(),
                "--settings".into(),
                mcp_config
                    .with_file_name("guard-settings.json")
                    .into_os_string(),
            ]
        }
        Native::Codex => vec![
            "exec".into(),
            "--json".into(),
            "--model".into(),
            model.into(),
            "--sandbox".into(),
            "workspace-write".into(),
            "--skip-git-repo-check".into(),
            "-c".into(),
            "approval_policy=\"never\"".into(),
            "-c".into(),
            "features.shell_tool=false".into(),
            "-c".into(),
            "mcp_servers={}".into(),
        ],
    };
    args.push(task.prompt.clone().into());
    args
}

pub async fn run_native(
    service: &Arc<Service>,
    kind: Native,
    binary: &Path,
    model: &str,
    task_id: Option<&str>,
    consent_cost: bool,
) -> Result<ParityRun, String> {
    if !consent_cost {
        return Err("Native comparisons require explicit cost/subscription consent".into());
    }
    if model.trim().is_empty() {
        return Err("The comparison requires an explicit model".into());
    }
    // Nothing was measured, so nothing is saved: a stored run would read as Codex failing every task.
    if kind == Native::Codex {
        return Err("Not run: native Codex tool/MCP isolation is not verified; this runner cannot safely enforce the fixed-task policy".into());
    }
    let _gate = service
        .parity_gate
        .try_lock()
        .map_err(|_| "A task run is already in progress")?;
    let assets = crate::parity::fixture_assets()?;
    let selected: Vec<_> = vatra_parity::load_tasks(&assets.path().join("tasks"))?
        .into_iter()
        .filter(|task| task_id.is_none_or(|id| task.id == id))
        .collect();
    if selected.is_empty() {
        return Err("Unknown task".into());
    }
    let name = format!("native:{}", kind.id());
    let mut run = ParityRun {
        agent: name.clone(),
        native: Some(kind.id().into()),
        model: model.into(),
        build: format!(
            "native CLI (version not reported); Vatra {}",
            env!("CARGO_PKG_VERSION")
        ),
        started_at: crate::models::now(),
        results: Vec::new(),
    };
    for (index, task) in selected.iter().enumerate() {
        service.notify(Notification::Models {
            method: "parity_progress",
            params: json!({"agent":name,"task":task.id,"done":index,"total":selected.len()}),
        });
        let fixture = assets.path().join("fixtures").join(&task.fixture);
        let workdir = tempfile::tempdir().map_err(|e| e.to_string())?;
        vatra_parity::copy_fixture(&fixture, workdir.path()).map_err(|e| e.to_string())?;
        let started = Instant::now();
        let control = guard_files(task, workdir.path())?;
        let mcp = control.path().join("fixture-mcp.json");
        let config = if task.needs_mcp {
            json!({"mcpServers":{"vatra-fixture-mcp":{"command":std::env::current_exe().map_err(|e|e.to_string())?,"args":["fixture-mcp"]}}})
        } else {
            json!({"mcpServers":{}})
        };
        std::fs::write(&mcp, config.to_string()).map_err(|e| e.to_string())?;
        let (events, error) = execute(
            kind,
            binary,
            model,
            task,
            &mcp,
            workdir.path(),
            Duration::from_secs(600),
        )
        .await?;
        let mut outcome = vatra_parity::evaluate(task, &fixture, workdir.path(), &events).await;
        if let Some(error) = error {
            outcome.passed = false;
            outcome.failures.push(error);
        }
        run.results.push(TaskResult {
            task_id: task.id.clone(),
            outcome,
            metrics: vatra_parity::metrics(&events, started.elapsed().as_secs_f64()),
        });
    }
    service
        .store()
        .insert_parity_run(&ParityRunRow {
            id: 0,
            agent: name.clone(),
            native: run.native.clone(),
            model: Some(run.model.clone()),
            build: Some(run.build.clone()),
            started_at: run.started_at,
            finished_at: crate::models::now(),
            results_json: serde_json::to_string(&run.results).map_err(|e| e.to_string())?,
            consented_cost: true,
        })
        .map_err(|e| e.to_string())?;
    service.notify(Notification::Models {
        method: "parity_progress",
        params: json!({"agent":name,"task":"done","done":selected.len(),"total":selected.len()}),
    });
    Ok(run)
}

/// Keep controls outside the disposable project so the agent's file tools cannot rewrite them.
fn guard_files(task: &Task, workdir: &Path) -> Result<tempfile::TempDir, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let policy = dir.path().join("guard-policy.json");
    std::fs::write(
        &policy,
        json!({"root":workdir,"commands":task.allowed_commands}).to_string(),
    )
    .map_err(|e| e.to_string())?;
    let command = guard_command(
        &std::env::current_exe().map_err(|e| e.to_string())?,
        &policy,
    );
    let settings = json!({"hooks":{"PreToolUse":[{"matcher":"*","hooks":[{"type":"command","command":command,"timeout":10}]}]}});
    std::fs::write(dir.path().join("guard-settings.json"), settings.to_string())
        .map_err(|e| e.to_string())?;
    Ok(dir)
}

#[cfg(windows)]
fn guard_command(binary: &Path, policy: &Path) -> String {
    use base64::Engine as _;
    // Encoding avoids cmd/PowerShell quoting ambiguities in user paths containing quotes or shell syntax.
    let quote = |path: &Path| path.to_string_lossy().replace('\'', "''");
    let script = format!(
        "try {{ & '{}' parity-guard --policy '{}'; if ($LASTEXITCODE -ne 0) {{ exit 2 }} }} catch {{ exit 2 }}",
        quote(binary),
        quote(policy)
    );
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    format!(
        "powershell.exe -NoProfile -NonInteractive -EncodedCommand {}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

#[cfg(not(windows))]
fn guard_command(binary: &Path, policy: &Path) -> String {
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    format!(
        "{} parity-guard --policy {} || exit 2",
        quote(binary),
        quote(policy)
    )
}

async fn execute(
    kind: Native,
    binary: &Path,
    model: &str,
    task: &Task,
    mcp: &Path,
    workdir: &Path,
    timeout: Duration,
) -> Result<(Vec<Event>, Option<String>), String> {
    let tree = ProcessGroup::new().map_err(|e| e.to_string())?;
    let mut command = tokio::process::Command::new(binary);
    command
        .args(native_args(kind, model, task, mcp))
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot start native CLI: {e}"))?;
    if let Err(error) = tree.adopt(&child)
        && !matches!(child.try_wait(), Ok(Some(_)))
    {
        let _ = child.kill().await;
        return Err(format!("Cannot supervise native CLI: {error}"));
    }
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Err(_) => {
            let _ = tree.kill_all();
            return Err("Native task timed out; incomplete run was not saved".into());
        }
        Ok(output) => output.map_err(|e| e.to_string())?,
    };
    let mut events = Vec::new();
    let mut completed = false;
    let mut error = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        match kind {
            Native::ClaudeCode => {
                for parsed in vatra_claude::parse_line(line) {
                    match parsed {
                        vatra_claude::Parsed::Event(event) => events.push(event),
                        vatra_claude::Parsed::Completed => completed = true,
                        vatra_claude::Parsed::Failed(message) => error = Some(message),
                        _ => {}
                    }
                }
            }
            Native::Codex => {
                let Ok(value) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                match value["type"].as_str() {
                    Some("turn.completed") => {
                        completed = true;
                        events.push(Event::Usage {
                            input_tokens: value["usage"]["input_tokens"].as_u64().unwrap_or(0),
                            output_tokens: value["usage"]["output_tokens"].as_u64().unwrap_or(0),
                            cost_usd: None,
                        });
                    }
                    Some("turn.failed") | Some("error") => {
                        error = Some("Codex reported a failed task".into())
                    }
                    Some("item.completed") => {
                        let item = &value["item"];
                        match item["type"].as_str() {
                            Some("agent_message") => events.push(Event::AssistantText{text:item["text"].as_str().unwrap_or("").into()}),
                            Some("command_execution") => error=Some("Native CLI executed a shell command despite the runner's disabled shell policy".into()),
                            Some("file_change") => events.push(Event::ToolCall {id:item["id"].as_str().unwrap_or("").into(),name:"Edit".into(),arguments:json!({})}),
                            _=>{}
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    if !output.status.success() {
        error = Some(format!("Native CLI exited with {}", output.status));
    } else if !completed && error.is_none() {
        error = Some("Native CLI did not report a completed turn".into());
    }
    Ok((events, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_guard_policy_lives_outside_the_model_workspace() {
        let work = tempfile::tempdir().unwrap();
        let task = crate::parity::tasks().unwrap().remove(0);
        let control = guard_files(&task, work.path()).unwrap();
        assert!(!control.path().starts_with(work.path()));
        let policy: Value = serde_json::from_slice(
            &std::fs::read(control.path().join("guard-policy.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(policy["root"], json!(work.path()));
        assert_eq!(policy["commands"], json!(["node test.js"]));
        let settings: Value = serde_json::from_slice(
            &std::fs::read(control.path().join("guard-settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(settings["hooks"]["PreToolUse"][0]["matcher"], "*");
        assert_eq!(
            settings["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"],
            10
        );
        assert_eq!(
            settings["hooks"]["PreToolUse"][0]["hooks"][0]["type"],
            "command"
        );
    }

    #[cfg(windows)]
    #[test]
    fn hook_command_safely_encodes_shell_syntax_in_paths() {
        use base64::Engine as _;
        let binary = Path::new("C:\\Răzvan ' $(&x)\\vatra.exe");
        let policy = Path::new("C:\\Răzvan ' $(&x)\\policy.json");
        let command = guard_command(binary, policy);
        let encoded = command.split_whitespace().last().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        let script = String::from_utf16(&units).unwrap();
        assert!(script.contains("if ($LASTEXITCODE -ne 0) { exit 2 }"));
        assert_eq!(
            script,
            "try { & 'C:\\Răzvan '' $(&x)\\vatra.exe' parity-guard --policy 'C:\\Răzvan '' $(&x)\\policy.json'; if ($LASTEXITCODE -ne 0) { exit 2 } } catch { exit 2 }"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn timed_out_native_process_returns_within_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("hang.cmd");
        std::fs::write(
            &binary,
            "@echo off\r\nnode -e \"setInterval(()=>{},1000)\"\r\n",
        )
        .unwrap();
        let task = crate::parity::tasks().unwrap().remove(0);
        let started = Instant::now();
        let error = execute(
            Native::ClaudeCode,
            &binary,
            "m",
            &task,
            Path::new("mcp.json"),
            dir.path(),
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();
        assert!(error.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
