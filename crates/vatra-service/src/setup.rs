//! Setting up agents from the app: the providers offered, the agents found on this computer, and saving an agent
//! with its key in the keychain. The agents themselves are built from these settings when the service starts.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use vatra_core::{ConfiguredAgentRow, SecretStore, Store};

use crate::config::{AgentConfig, agent_secret};

/// A provider the app offers for a model agent. `custom` takes any OpenAI-compatible address.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: Option<&'static str>,
    pub driver: &'static str,
    /// A public list of the provider's models, read without a key.
    pub models_url: Option<&'static str>,
    pub key_url: Option<&'static str>,
}

pub const PROVIDERS: [Provider; 8] = [
    Provider {
        id: "openrouter",
        name: "OpenRouter",
        base_url: Some("https://openrouter.ai/api/v1"),
        driver: "openai",
        models_url: Some("https://openrouter.ai/api/v1/models"),
        key_url: Some("https://openrouter.ai/settings/keys"),
    },
    Provider {
        id: "anthropic",
        name: "Anthropic",
        base_url: None,
        driver: "anthropic",
        models_url: None,
        key_url: Some("https://console.anthropic.com/settings/keys"),
    },
    Provider {
        id: "openai",
        name: "OpenAI",
        base_url: Some("https://api.openai.com/v1"),
        driver: "openai",
        models_url: None,
        key_url: Some("https://platform.openai.com/api-keys"),
    },
    // Servers the person runs themselves speak the same protocol and need no key.
    Provider {
        id: "ollama",
        name: "Ollama",
        base_url: Some("http://localhost:11434/v1"),
        driver: "openai",
        models_url: Some("http://localhost:11434/v1/models"),
        key_url: None,
    },
    Provider {
        id: "vllm",
        name: "vLLM",
        base_url: Some("http://localhost:8000/v1"),
        driver: "openai",
        models_url: Some("http://localhost:8000/v1/models"),
        key_url: None,
    },
    Provider {
        id: "lmstudio",
        name: "LM Studio",
        base_url: Some("http://localhost:1234/v1"),
        driver: "openai",
        models_url: Some("http://localhost:1234/v1/models"),
        key_url: None,
    },
    // Signs in with GitHub on the page; the token it gives is kept as the key.
    Provider { id: "copilot", name: "GitHub Copilot", base_url: Some("https://api.githubcopilot.com"), driver: "copilot", models_url: None, key_url: None },
    Provider { id: "custom", name: "Other", base_url: None, driver: "openai", models_url: None, key_url: None },
];

/// What the app asks for when it saves an agent.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AgentForm {
    #[serde(rename_all = "camelCase")]
    Model { name: String, provider: String, base_url: Option<String>, model: String, key: Option<String> },
    Claude { binary: PathBuf },
    #[serde(rename_all = "camelCase")]
    Acp {
        name: String,
        node: PathBuf,
        /// Legacy Node adapter entry point; absent for native ACP commands.
        #[serde(default)]
        script: Option<PathBuf>,
        #[serde(default)]
        args: Vec<String>,
        startup_timeout_sec: Option<u64>,
        turn_timeout_sec: Option<u64>,
        #[serde(default)]
        subscription: bool,
        /// Codex runs on the ChatGPT account connected in Settings, not on the Codex the person is signed in to.
        #[serde(default)]
        managed_account: bool,
        /// The official Codex program; needed with `managed_account`.
        #[serde(default)]
        codex_binary: Option<PathBuf>,
    },
}

/// An agent as the Agents page lists it. Keys are never in it, only whether one is saved.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentView {
    pub name: String,
    pub kind: String,
    /// `app` for agents set up here, `file` for the ones in vatra.toml.
    pub source: &'static str,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub has_key: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedAgents {
    pub claude: Option<PathBuf>,
    pub codex: Option<PathBuf>,
    pub node: Option<PathBuf>,
    /// ACP adapters installed with npm: the package name and its script.
    pub acp_scripts: Vec<(String, PathBuf)>,
}

pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|provider| provider.id == id)
}

/// Checks the form and turns it into the agent and its settings row. The key is returned apart, for the keychain.
pub fn agent_from_form(form: AgentForm, now: i64) -> Result<(ConfiguredAgentRow, Option<String>), String> {
    let (name, agent, key) = match form {
        AgentForm::Model { name, provider: id, base_url, model, key } => {
            let provider = provider(&id).ok_or_else(|| format!("unknown provider `{id}`"))?;
            let base_url = match (provider.base_url, base_url.filter(|url| !url.trim().is_empty())) {
                (Some(fixed), _) => Some(fixed.to_string()),
                (None, Some(url)) => Some(url.trim().to_string()),
                (None, None) if provider.driver == "anthropic" => None,
                (None, None) => return Err("Enter the address of the service.".into()),
            };
            if model.trim().is_empty() {
                return Err("Choose a model.".into());
            }
            let agent = AgentConfig::Model {
                base_url,
                model: model.trim().to_string(),
                api_key_env: None,
                driver: Some(provider.driver.to_string()),
                context_window: None,
                api_key: None,
            };
            (name, agent, key.filter(|key| !key.trim().is_empty()).map(|key| key.trim().to_string()))
        }
        // vatra.toml has the same rule: the Claude Code agent is always called `claude`.
        AgentForm::Claude { binary } => ("claude".to_string(), AgentConfig::Claude { binary }, None),
        AgentForm::Acp { name, node, script, args, startup_timeout_sec, turn_timeout_sec, subscription, managed_account, codex_binary } => {
            if managed_account && codex_binary.as_ref().is_none_or(|path| path.as_os_str().is_empty()) {
                return Err("Codex wasn't found on this computer. Install it, or use the Codex you're already signed in to.".into());
            }
            let args: Vec<String> = script.into_iter().map(|script| script.to_string_lossy().into_owned()).chain(args).collect();
            let startup_timeout_sec = startup_timeout_sec.unwrap_or(30);
            let turn_timeout_sec = turn_timeout_sec.unwrap_or(900);
            if node.as_os_str().is_empty() || node.to_string_lossy().contains('\0') {
                return Err("Choose an ACP executable.".into());
            }
            if node.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")) {
                return Err("ACP needs an executable, not a .cmd or .bat shell shim. Use node.exe with the installed JavaScript entry point as an argument.".into());
            }
            if args.len() > 128 || args.iter().any(|arg| arg.contains('\0')) || args.iter().map(String::len).sum::<usize>() > 32768 {
                return Err("ACP arguments are invalid or too large. Keep credentials out of arguments.".into());
            }
            if !(1..=86400).contains(&startup_timeout_sec) || !(1..=86400).contains(&turn_timeout_sec) {
                return Err("ACP timeouts must be between 1 and 86400 seconds.".into());
            }
            let agent = AgentConfig::Acp {
                binary: node,
                // The managed account is a plan, so it carries the same notice as any subscription.
                subscription: subscription || managed_account,
                chatgpt_plan: managed_account,
                codex_binary: codex_binary.filter(|_| managed_account),
                args,
                startup_timeout_sec,
                turn_timeout_sec,
            };
            (name, agent, None)
        }
    };
    check_name(&name)?;
    let kind = match &agent {
        AgentConfig::Model { .. } => "model",
        AgentConfig::Claude { .. } => "claude",
        AgentConfig::Acp { .. } => "acp",
    };
    let settings_json = serde_json::to_string(&agent).map_err(|error| error.to_string())?;
    Ok((ConfiguredAgentRow { name, kind: kind.into(), settings_json, created_at: now }, key))
}

fn check_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= 40
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with(crate::models::AGENT_PREFIX.trim_end_matches(':'));
    if valid { Ok(()) } else { Err("Use a short name with letters, digits, dashes or dots.".into()) }
}

/// Refuses a save that would be ignored or would send an old key somewhere new. An agent whose name vatra.toml
/// already uses is never loaded from the app, and a saved key follows the agent only while its service stays the same.
pub fn check_save(
    store: &Store,
    secrets: &dyn SecretStore,
    file_agents: &[String],
    row: &ConfiguredAgentRow,
    key: Option<&str>,
) -> Result<(), String> {
    if file_agents.contains(&row.name) {
        return Err(format!("`{}` is set in vatra.toml. Edit it there, or choose another name.", row.name));
    }
    let existing = store.configured_agents().map_err(|error| error.to_string())?.into_iter().find(|old| old.name == row.name);
    let Some(old) = existing else { return Ok(()) };
    let place = |json: &str| {
        let settings: Value = serde_json::from_str(json).unwrap_or(Value::Null);
        (settings["base_url"].clone(), settings["driver"].clone())
    };
    let has_key = secrets.get(&agent_secret(&row.name)).ok().flatten().is_some();
    if key.is_none() && has_key && place(&old.settings_json) != place(&row.settings_json) {
        return Err("The service address changed, so enter the key again. The saved key isn't sent to a new address.".into());
    }
    Ok(())
}

/// Saves the agent and its key. A key that isn't given keeps the one saved before.
pub fn save(store: &Store, secrets: &dyn SecretStore, row: &ConfiguredAgentRow, key: Option<&str>) -> Result<(), String> {
    if let Some(key) = key {
        secrets.set(&agent_secret(&row.name), key).map_err(|error| format!("the key couldn't be saved: {error}"))?;
    }
    store.save_configured_agent(row).map_err(|error| error.to_string())
}

pub fn remove(store: &Store, secrets: &dyn SecretStore, name: &str) -> Result<(), String> {
    store.remove_configured_agent(name).map_err(|error| error.to_string())?;
    secrets.delete(&agent_secret(name)).map_err(|error| error.to_string())
}

/// The agents of the app and of vatra.toml, for the Agents page.
pub fn list(store: &Store, secrets: &dyn SecretStore, file_agents: &[(String, String)]) -> Vec<AgentView> {
    let rows = store.configured_agents().unwrap_or_default();
    let mut views: Vec<AgentView> = file_agents
        .iter()
        .map(|(name, kind)| AgentView {
            name: name.clone(),
            kind: kind.clone(),
            source: "file",
            provider: None,
            model: None,
            has_key: false,
        })
        .collect();
    for row in rows {
        if views.iter().any(|view| view.name == row.name) {
            continue;
        }
        let settings: Value = serde_json::from_str(&row.settings_json).unwrap_or(Value::Null);
        let base = settings["base_url"].as_str();
        let driver = settings["driver"].as_str();
        let provider = PROVIDERS
            .iter()
            .find(|provider| provider.base_url == base && provider.base_url.is_some())
            .or_else(|| PROVIDERS.iter().find(|provider| base.is_none() && Some(provider.driver) == driver && provider.id == "anthropic"))
            .map(|provider| provider.id.to_string())
            .or_else(|| (row.kind == "model").then(|| "custom".to_string()));
        views.push(AgentView {
            has_key: secrets.get(&agent_secret(&row.name)).ok().flatten().is_some(),
            model: settings["model"].as_str().map(String::from),
            provider,
            name: row.name,
            kind: row.kind,
            source: "app",
        });
    }
    views
}

/// The models a provider lists publicly, read without a key: id, name, and whether they take tools.
pub async fn provider_models(url: &str) -> Result<Vec<Value>, String> {
    let body: Value = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|error| error.without_url().to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let models = body["data"].as_array().cloned().unwrap_or_default();
    Ok(models
        .into_iter()
        .map(|model| {
            let tools = model["supported_parameters"]
                .as_array()
                .is_some_and(|parameters| parameters.iter().any(|parameter| parameter == "tools"));
            let free = model["pricing"]["prompt"].as_str() == Some("0") && model["pricing"]["completion"].as_str() == Some("0");
            serde_json::json!({
                "id": model["id"],
                "name": model["name"],
                "tools": tools,
                "free": free,
                "contextLength": model["context_length"],
            })
        })
        .collect())
}

/// Finds Claude Code, Codex, node and ACP adapters. `path` is the PATH to search, `home` the user's folder, and
/// `npm_root` the global npm folder when known.
pub fn detect(path: &OsStr, home: Option<&Path>, npm_root: Option<&Path>) -> DetectedAgents {
    let dirs: Vec<PathBuf> = std::env::split_paths(path).collect();
    let find = |names: &[&str], extra: &[PathBuf]| {
        dirs.iter()
            .chain(extra)
            .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
            .find(|candidate| candidate.is_file())
    };
    let local_bin: Vec<PathBuf> = home.map(|home| home.join(".local").join("bin")).into_iter().collect();
    let acp_scripts = npm_root.map(acp_packages).unwrap_or_default();
    DetectedAgents {
        claude: find(&["claude.exe", "claude"], &local_bin),
        codex: find(&["codex.exe", "codex"], &[]),
        node: find(&["node.exe", "node"], &[]),
        acp_scripts,
    }
}

/// npm packages whose name mentions ACP, with the script their first `bin` entry runs.
fn acp_packages(root: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let mut folders: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('@') {
            folders.extend(std::fs::read_dir(&path).into_iter().flatten().flatten().map(|inner| inner.path()));
        } else {
            folders.push(path);
        }
    }
    for folder in folders {
        let Ok(text) = std::fs::read_to_string(folder.join("package.json")) else { continue };
        let Ok(package) = serde_json::from_str::<Value>(&text) else { continue };
        let name = package["name"].as_str().unwrap_or_default().to_string();
        if !name.to_ascii_lowercase().contains("acp") {
            continue;
        }
        let script = match &package["bin"] {
            Value::String(script) => Some(script.clone()),
            Value::Object(bins) => bins.values().find_map(|value| value.as_str().map(String::from)),
            _ => None,
        };
        if let Some(script) = script {
            let script = folder.join(script);
            if script.is_file() {
                found.push((name, script));
            }
        }
    }
    found.sort();
    found
}

/// The global npm folder, from `npm root -g`; None when npm isn't installed.
async fn npm_root() -> Option<PathBuf> {
    let mut command = tokio::process::Command::new(if cfg!(windows) { "cmd" } else { "npm" });
    if cfg!(windows) {
        command.args(["/C", "npm", "root", "-g"]);
    } else {
        command.args(["root", "-g"]);
    }
    command.stdin(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = tokio::time::timeout(Duration::from_secs(10), command.output()).await.ok()?.ok()?;
    let root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    root.is_dir().then_some(root)
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct ProviderId {
    provider: String,
}

#[derive(Deserialize)]
struct Summarizer {
    name: Option<String>,
}

type Reply = Result<Value, (i64, String)>;

pub(crate) fn failed(message: impl std::fmt::Display) -> (i64, String) {
    (-32000, message.to_string())
}

pub(crate) fn parse<T: for<'de> Deserialize<'de>>(params: Value) -> Result<T, (i64, String)> {
    serde_json::from_value(params).map_err(|error| (-32602, error.to_string()))
}

/// The `agents.*` requests that set agents up. `agents.list` and `agents.describe` stay in rpc.rs.
pub(crate) async fn handle(service: &std::sync::Arc<crate::Service>, method: &str, params: Value) -> Reply {
    let store = service.store();
    let secrets = service.agents().secrets();
    match method {
        "agents.form" => {
            let request: Named = parse(params)?;
            if service.agents().file_agent_names().is_some_and(|names|names.contains(&request.name)) { return Err(failed("Agents from vatra.toml are read-only")); }
            let row = store.configured_agents().map_err(failed)?.into_iter().find(|r|r.name==request.name).ok_or_else(||failed("Only agents saved in the app can be edited"))?;
            let config:AgentConfig = serde_json::from_str(&row.settings_json).map_err(failed)?;
            let form = match config {
                AgentConfig::Model { base_url, model, driver, .. } => {
                    let provider = PROVIDERS.iter().find(|p|p.base_url==base_url.as_deref() && p.base_url.is_some())
                        .or_else(||PROVIDERS.iter().find(|p|p.id=="anthropic" && driver.as_deref()==Some("anthropic"))).map(|p|p.id).unwrap_or("custom");
                    serde_json::json!({"kind":"model","name":row.name,"provider":provider,"baseUrl":base_url,"model":model})
                }
                AgentConfig::Claude { binary } => serde_json::json!({"kind":"claude","binary":binary}),
                AgentConfig::Acp { binary,args,subscription,chatgpt_plan,codex_binary,startup_timeout_sec,turn_timeout_sec } => serde_json::json!({"kind":"acp","name":row.name,"node":binary,"args":args,"subscription":subscription,"managedAccount":chatgpt_plan,"codexBinary":codex_binary,"startupTimeoutSec":startup_timeout_sec,"turnTimeoutSec":turn_timeout_sec}),
            };
            Ok(form)
        }
        "agents.providers" => Ok(serde_json::json!(PROVIDERS)),
        "copilot.start" => Ok(serde_json::json!(crate::copilot::start().await.map_err(failed)?)),
        "copilot.poll" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Code { device_code: String }
            let request: Code = parse(params)?;
            Ok(serde_json::json!(crate::copilot::poll(&request.device_code).await.map_err(failed)?))
        }
        "copilot.models" => {
            #[derive(Deserialize)]
            struct Key { key: String }
            let request: Key = parse(params)?;
            Ok(Value::Array(crate::copilot::models(&request.key).await.map_err(failed)?))
        }
        "agents.provider_models" => {
            let request: ProviderId = parse(params)?;
            let url = provider(&request.provider).and_then(|provider| provider.models_url).ok_or_else(|| failed("this provider has no public model list"))?;
            Ok(Value::Array(provider_models(url).await.map_err(failed)?))
        }
        "agents.detect" => {
            let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
            let path = std::env::var_os("PATH").unwrap_or_default();
            let root = npm_root().await;
            Ok(serde_json::json!(detect(&path, home.as_deref(), root.as_deref())))
        }
        "agents.configured" => {
            let app: Vec<String> = store.configured_agents().unwrap_or_default().into_iter().map(|row| row.name).collect();
            let file: Vec<(String, String)> = service
                .agents()
                .names()
                .into_iter()
                .filter(|name| !name.starts_with(crate::models::AGENT_PREFIX) && service.agents().file_agent_names().map_or_else(||!app.contains(name),|names|names.contains(name)))
                .map(|name| {
                    let kind = match service.agents().extension_route(&name) {
                        Some("engine") => "model",
                        Some(route) => route,
                        None => "model",
                    };
                    (name, kind.to_string())
                })
                .collect();
            let summarizer = store.app_setting("summarizer").ok().flatten();
            let mut views = list(store, secrets.as_ref(), &file);
            for view in &mut views {
                if let Some((provider,model)) = service.agents().picker_identity(&view.name) {
                    view.provider = Some(provider); view.model = model;
                }
            }
            Ok(serde_json::json!({ "agents": views, "summarizer": summarizer }))
        }
        "agents.save" => {
            let form: AgentForm = parse(params)?;
            let (row, key) = agent_from_form(form, crate::models::now()).map_err(failed)?;
            let file_agents = service.agents().file_agent_names().unwrap_or_default();
            check_save(store, secrets.as_ref(), &file_agents, &row, key.as_deref()).map_err(failed)?;
            save(store, secrets.as_ref(), &row, key.as_deref()).map_err(failed)?;
            Ok(serde_json::json!({ "name": row.name, "restartNeeded": true, "runningTurns": service.running_sessions().len() }))
        }
        "agents.remove" => {
            let request: Named = parse(params)?;
            remove(store, secrets.as_ref(), &request.name).map_err(failed)?;
            Ok(serde_json::json!({ "restartNeeded": true, "runningTurns": service.running_sessions().len() }))
        }
        "agents.summarizer" => {
            let request: Summarizer = parse(params)?;
            store.set_app_setting("summarizer", request.name.as_deref()).map_err(failed)?;
            Ok(serde_json::json!({ "restartNeeded": true, "runningTurns": service.running_sessions().len() }))
        }
        _ => Err((-32601, format!("unknown method `{method}`"))),
    }
}
