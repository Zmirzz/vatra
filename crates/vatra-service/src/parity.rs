//! Six fixed tasks, run on disposable copies. Approval is a tool boundary, not an OS sandbox.
use crate::{BuildRequest, Notification, PermissionMode, Service};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use vatra_core::{ParityRunRow, Store, TurnRequest};
use vatra_engine::{Approver, NoQuestions};
use vatra_parity::{Metrics, Outcome, Task};

mod assets {
    include!(concat!(env!("OUT_DIR"), "/parity_assets.rs"));
}

pub fn fixture_assets() -> Result<tempfile::TempDir, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    for (name, bytes) in assets::FILES {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(path, bytes).map_err(|e| e.to_string())?;
    }
    Ok(dir)
}
pub fn tasks() -> Result<Vec<Task>, String> {
    let assets = fixture_assets()?;
    vatra_parity::load_tasks(&assets.path().join("tasks"))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResult {
    pub task_id: String,
    pub outcome: Outcome,
    pub metrics: Metrics,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParityRun {
    pub agent: String,
    pub native: Option<String>,
    pub model: String,
    pub build: String,
    pub started_at: i64,
    pub results: Vec<TaskResult>,
}

pub struct TaskApprover {
    root: PathBuf,
    commands: Vec<String>,
}
impl TaskApprover {
    pub fn new(root: &Path, commands: Vec<String>) -> Self {
        Self {
            root: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
            commands,
        }
    }
    fn inside(&self, input: &str) -> bool {
        let path = Path::new(input);
        let mut path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        if path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return false;
        }
        while !path.exists() {
            if !path.pop() {
                return false;
            }
        }
        path.canonicalize()
            .is_ok_and(|path| path.starts_with(&self.root))
    }
}
#[async_trait]
impl Approver for TaskApprover {
    async fn approve(&self, name: &str, args: &Value) -> bool {
        match name {
            "Bash" | "PowerShell" => {
                args["command"]
                    .as_str()
                    .is_some_and(|cmd| self.commands.iter().any(|allowed| allowed == cmd.trim()))
                    && !args["run_in_background"].as_bool().unwrap_or(false)
            }
            "Read" | "Write" | "Edit" => args
                .get("file_path")
                .or_else(|| args.get("path"))
                .and_then(Value::as_str)
                .is_some_and(|path| self.inside(path)),
            "Glob" | "Grep" => args
                .get("path")
                .and_then(Value::as_str)
                .is_none_or(|path| self.inside(path)),
            "TodoWrite" | "Skill" | "ToolSearch" | "mcp__vatra-fixture-mcp__echo" => true,
            _ => false,
        }
    }
}
pub async fn run_parity(
    service: &Arc<Service>,
    name: &str,
    task: Option<&str>,
    consent_cost: bool,
) -> Result<ParityRun, String> {
    if !service.agents().names().iter().any(|n| n == name) {
        return Err(format!("unknown agent `{name}`"));
    }
    if service.agents().check_costs(name) && !consent_cost {
        return Err("This agent requires explicit cost consent".into());
    }
    // One run per service, also released on cancellation.
    let _gate = service
        .parity_gate
        .try_lock()
        .map_err(|_| "A task run is already in progress")?;
    let assets = fixture_assets()?;
    let all = vatra_parity::load_tasks(&assets.path().join("tasks"))?;
    let selected: Vec<_> = all
        .into_iter()
        .filter(|t| task.is_none_or(|id| t.id == id))
        .collect();
    if selected.is_empty() {
        return Err("Unknown task".into());
    }
    let model = service
        .agents()
        .configured_models()
        .into_iter()
        .find(|m| m.name == name)
        .map(|m| m.model)
        .unwrap_or_else(|| name.into());
    let build = service
        .agents()
        .models()
        .filter(|_| !service.agents().check_costs(name))
        .and_then(|m| m.server().install().and_then(|i| i.build))
        .map(|b| format!("llama.cpp {b}"))
        .unwrap_or_else(|| format!("Vatra {}", env!("CARGO_PKG_VERSION")));
    let mut run = ParityRun {
        agent: name.into(),
        native: None,
        model,
        build,
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
        let request = BuildRequest {
            name: name.into(),
            session_id: -(index as i64 + 1),
            project_id: -(index as i64 + 1),
            approver: Arc::new(TaskApprover::new(
                workdir.path(),
                task.allowed_commands.clone(),
            )),
            questioner: Arc::new(NoQuestions),
            mode: PermissionMode::Supervised,
            cwd: Some(workdir.path().to_path_buf()),
            options: Default::default(),
        };
        let started = Instant::now();
        let agent = service.agents().build_parity(request, task).await?;
        let (sink, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let request = TurnRequest { files: vec![],
            history: Vec::new(),
            prompt: task.prompt.clone(),
            native_session: None,
        };
        let drain = async {
            let mut events = Vec::new();
            while let Some(event) = receiver.recv().await {
                events.push(event);
            }
            events
        };
        let (answer, events) = tokio::time::timeout(Duration::from_secs(600), async {
            tokio::join!(agent.run_turn(request, sink), drain)
        })
        .await
        .map_err(|_| format!("Task {} timed out after 10 minutes; run not saved", task.id))?;
        drop(agent); // releases the engine's process tree, MCP hub and local server lease
        // For diagnosing a run: `VATRA_PARITY_TRACE=<file>` keeps every event of every task, one JSON line each.
        if let Some(path) = std::env::var_os("VATRA_PARITY_TRACE")
            && let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path)
        {
            use std::io::Write as _;
            for event in &events {
                let _ = file.write_all(format!("{}
", json!({"task": task.id, "event": event})).as_bytes());
            }
        }
        let mut outcome = vatra_parity::evaluate(task, &fixture, workdir.path(), &events).await;
        if let Err(error) = answer {
            outcome.passed = false;
            outcome.failures.push(error.to_string());
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
            agent: run.agent.clone(),
            native: None,
            model: Some(run.model.clone()),
            build: Some(run.build.clone()),
            started_at: run.started_at,
            finished_at: crate::models::now(),
            results_json: serde_json::to_string(&run.results).map_err(|e| e.to_string())?,
            consented_cost: consent_cost,
        })
        .map_err(|e| e.to_string())?;
    service.notify(Notification::Models {
        method: "parity_progress",
        params: json!({"agent":name,"task":"done","done":selected.len(),"total":selected.len()}),
    });
    Ok(run)
}
pub fn results(store: &Store, agent: Option<&str>) -> Result<Vec<ParityRun>, String> {
    store
        .parity_runs(100)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|r| agent.is_none_or(|a| a == r.agent))
        .map(|r| {
            Ok(ParityRun {
                agent: r.agent,
                native: r.native,
                model: r.model.unwrap_or_default(),
                build: r.build.unwrap_or_default(),
                started_at: r.started_at,
                results: serde_json::from_str(&r.results_json).map_err(|e| e.to_string())?,
            })
        })
        .collect()
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TaskRate {
    pub agent: String,
    pub model: String,
    pub build: String,
    pub task_id: String,
    pub passed: u32,
    pub runs: u32,
}
/// Counts each task over the runs that share an agent, model and build, ordered by task id.
pub fn pass_rates(runs: &[ParityRun]) -> Vec<TaskRate> {
    let mut rates: Vec<TaskRate> = Vec::new();
    for run in runs {
        for result in &run.results {
            let rate = match rates.iter_mut().find(|r| {
                r.agent == run.agent
                    && r.model == run.model
                    && r.build == run.build
                    && r.task_id == result.task_id
            }) {
                Some(rate) => rate,
                None => {
                    rates.push(TaskRate {
                        agent: run.agent.clone(),
                        model: run.model.clone(),
                        build: run.build.clone(),
                        task_id: result.task_id.clone(),
                        passed: 0,
                        runs: 0,
                    });
                    rates.last_mut().unwrap()
                }
            };
            rate.runs += 1;
            rate.passed += u32::from(result.outcome.passed);
        }
    }
    rates.sort_by(|a, b| {
        (&a.task_id, &a.agent, &a.model, &a.build).cmp(&(&b.task_id, &b.agent, &b.model, &b.build))
    });
    rates
}
fn cell(text: &str) -> String {
    text.replace(['|', '\n', '\r'], " ")
}
pub fn export_matrix(runs: &[ParityRun]) -> String {
    let mut out = "# Fixed task results\n\nThis covers only the listed tasks and does not establish general agent equivalence. Times are Unix seconds (UTC).\n\n| Agent / model | Build | Date (UTC) | Coverage | Passed |\n|---|---|---|---|---|\n".to_string();
    for run in runs {
        out.push_str(&format!(
            "| {} / {} | {} | {} | {} tasks | {} |\n",
            cell(&run.agent),
            cell(&run.model),
            cell(&run.build),
            run.started_at,
            run.results.len(),
            run.results.iter().filter(|r| r.outcome.passed).count()
        ));
    }
    let mut groups: Vec<(&str, &str, &str)> = Vec::new();
    for run in runs {
        let key = (run.agent.as_str(), run.model.as_str(), run.build.as_str());
        if !groups.contains(&key) {
            groups.push(key);
        }
    }
    let same = |run: &ParityRun, key: (&str, &str, &str)| {
        run.agent == key.0 && run.model == key.1 && run.build == key.2
    };
    let repeated: Vec<(&str, &str, &str)> = groups
        .into_iter()
        .filter(|&key| runs.iter().filter(|run| same(run, key)).count() > 1)
        .collect();
    if !repeated.is_empty() {
        let rates = pass_rates(runs);
        out.push_str("\n## Pass rate per task\n");
        for key in repeated {
            out.push_str(&format!(
                "\n### {} / {} ({})\n\n| Task | Passed / runs |\n|---|---|\n",
                cell(key.0),
                cell(key.1),
                cell(key.2)
            ));
            for rate in rates.iter().filter(|r| (r.agent.as_str(), r.model.as_str(), r.build.as_str()) == key) {
                out.push_str(&format!("| {} | {} / {} |\n", cell(&rate.task_id), rate.passed, rate.runs));
            }
        }
    }
    for run in runs {
        out.push_str(&format!("\nTask details for {} / {} ({})\n",cell(&run.agent),cell(&run.model),run.started_at));
        for result in &run.results {
            out.push_str(&format!(
                "\n- {}: {}{}\n",
                cell(&result.task_id),
                if result.outcome.passed {
                    "passed"
                } else {
                    "failed: "
                },
                result.outcome.failures.iter().map(|failure| failure.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("; ")
            ));
        }
    }
    out
}
pub(crate) async fn handle(
    service: &Arc<Service>,
    method: &str,
    params: Value,
) -> Result<Value, (i64, String)> {
    let err = |e: String| (-32602, e);
    match method {
        "parity.native" => {
            if params["consentCost"].as_bool() != Some(true) {
                return Err(err(
                    "Native comparison requires explicit cost consent".into()
                ));
            }
            let kind = match params["kind"].as_str() {
                Some("claude") => crate::parity_native::Native::ClaudeCode,
                Some("codex") => crate::parity_native::Native::Codex,
                _ => return Err(err("kind must be claude or codex".into())),
            };
            let model = params["model"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| err("A model is required for native comparison".into()))?;
            let found = crate::setup::handle(service, "agents.detect", json!({})).await?;
            let field = if matches!(kind, crate::parity_native::Native::ClaudeCode) {
                "claude"
            } else {
                "codex"
            };
            let binary = found[field]
                .as_str()
                .ok_or_else(|| err(format!("{field} was not found on this computer")))?;
            crate::parity_native::run_native(
                service,
                kind,
                Path::new(binary),
                model,
                params["task"].as_str(),
                true,
            )
            .await
            .map(|v| json!(v))
            .map_err(err)
        }
        "parity.tasks" => tasks().map(|v| json!(v)).map_err(err),
        "parity.results" => results(service.store(), params["agent"].as_str())
            .map(|v| json!(v))
            .map_err(err),
        "parity.export" => results(service.store(), params["agent"].as_str())
            .map(|v| json!(export_matrix(&v)))
            .map_err(err),
        "parity.run" => {
            let agent = params["agent"]
                .as_str()
                .ok_or_else(|| err("agent is required".into()))?;
            run_parity(
                service,
                agent,
                params["task"].as_str(),
                params["consentCost"].as_bool().unwrap_or(false),
            )
            .await
            .map(|v| json!(v))
            .map_err(err)
        }
        _ => Err((-32601, "Unknown method".into())),
    }
}
