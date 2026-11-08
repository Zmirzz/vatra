//! What the service offers about models: the hardware, Hugging Face, downloads, the local server and the labels.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;
use vatra_core::{McpServerConfig, Store};
use vatra_models::estimate::{self, Estimate, MemoryBudget};
use vatra_models::gguf::ModelFacts;
use vatra_models::hardware::{self, HardwareReport};
use vatra_models::hf::{HfClient, HfError, RepoSummary};
use vatra_models::label::{self, Label, LabelReason, OpenRouterModel};
use vatra_models::llama::{self, LlamaInstall};
use vatra_models::suggest::{self, Suggestion, Task};
use vatra_models::variant::Variant;

use super::download::{DownloadProgress, Downloader, Job, local_facts};
use super::server::{ModelServer, RunningServer, ServerError, TurnGuard};
use super::{AGENT_PREFIX, AddedBy, DEFAULT_CTX, LocalModel, ModelSettings, now, slug};
use crate::service::Notification;

const HARDWARE_TTL: Duration = Duration::from_secs(60);

/// A variant of a repo with its estimate for the default context.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantView {
    #[serde(flatten)]
    pub variant: Variant,
    pub estimate: Estimate,
    pub low_quality: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoView {
    pub repo: String,
    pub gated: bool,
    pub facts: Option<ModelFacts>,
    pub ctx: u32,
    pub variants: Vec<VariantView>,
    pub label: Label,
    pub reasons: Vec<LabelReason>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledView {
    #[serde(flatten)]
    pub model: LocalModel,
    pub agent: String,
    pub running: bool,
    pub missing: bool,
}

/// An agent's label as the app shows it: measured by the last check, or estimated.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelView {
    pub agent: String,
    pub label: Label,
    pub reasons: Vec<LabelReason>,
    pub estimated: bool,
    pub checked_at: Option<i64>,
    pub llama_build: Option<u32>,
    /// The check was made with other settings or another llama.cpp build than the current ones.
    pub stale: bool,
    pub results: Option<Value>,
}

/// An engine agent of the configuration, as far as labels need it.
pub struct ConfiguredModel {
    pub name: String,
    pub driver: String,
    pub base_url: Option<String>,
    pub model: String,
}

pub struct ModelManager {
    store: Arc<Store>,
    hf: HfClient,
    server: Arc<ModelServer>,
    downloader: Downloader,
    models_dir: PathBuf,
    probe: (PathBuf, Vec<String>),
    notifier: OnceLock<broadcast::Sender<Notification>>,
    hardware: Mutex<Option<(Instant, HardwareReport)>>,
    facts: Mutex<HashMap<String, Option<ModelFacts>>>,
    openrouter: Mutex<Option<Vec<OpenRouterModel>>>,
    checks: tokio::sync::Mutex<()>,
    openrouter_url: String,
    /// The llama-server path from vatra.toml, when set.
    llama_path: OnceLock<PathBuf>,
}

impl ModelManager {
    /// `probe` is the command that serves the check's MCP server: `vatra mcp-probe` in the app.
    pub fn new(store: Arc<Store>, models_dir: PathBuf, llama: Option<LlamaInstall>, probe: (PathBuf, Vec<String>)) -> Arc<ModelManager> {
        Self::with_sources(store, models_dir, llama, probe, HfClient::public(), "https://openrouter.ai/api/v1/models".into())
    }

    pub fn with_sources(
        store: Arc<Store>,
        models_dir: PathBuf,
        llama: Option<LlamaInstall>,
        probe: (PathBuf, Vec<String>),
        hf: HfClient,
        openrouter_url: String,
    ) -> Arc<ModelManager> {
        let server = ModelServer::new(llama);
        let manager = Arc::new(ModelManager {
            store,
            hf,
            server,
            downloader: Downloader::default(),
            models_dir,
            probe,
            notifier: OnceLock::new(),
            hardware: Mutex::new(None),
            facts: Mutex::default(),
            openrouter: Mutex::new(None),
            checks: tokio::sync::Mutex::new(()),
            openrouter_url,
            llama_path: OnceLock::new(),
        });
        let weak = Arc::downgrade(&manager);
        manager.server.on_state(Arc::new(move |state| {
            if let Some(manager) = weak.upgrade() {
                manager.notify("model_server", json!(state));
            }
        }));
        manager
    }

    /// Finds llama.cpp: the configured path, PATH, then winget's folder.
    pub fn find_llama(configured: Option<&Path>) -> Option<LlamaInstall> {
        llama::find(configured)
    }

    /// Starts the timer that stops an unused server. Needs a Tokio runtime.
    pub fn watch_idle(&self, idle: Duration) {
        self.server.watch_idle(idle);
    }

    pub fn set_notifier(&self, sender: broadcast::Sender<Notification>) {
        let _ = self.notifier.set(sender);
    }

    fn notify(&self, method: &'static str, params: Value) {
        if let Some(sender) = self.notifier.get() {
            let _ = sender.send(Notification::Models { method, params });
        }
    }

    /// The llama-server path vatra.toml names, used when llama.cpp is looked for again.
    pub fn set_llama_path(&self, path: PathBuf) {
        let _ = self.llama_path.set(path);
    }

    /// Looks for llama.cpp again, after an install, and forgets the hardware read without it.
    pub fn redetect_llama(&self) {
        self.server.set_install(llama::find(self.llama_path.get().map(PathBuf::as_path)));
        *self.hardware.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    pub fn server(&self) -> &Arc<ModelServer> {
        &self.server
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    pub async fn hardware(&self) -> HardwareReport {
        if let Some((at, report)) = self.hardware.lock().unwrap_or_else(PoisonError::into_inner).as_ref()
            && at.elapsed() < HARDWARE_TTL
        {
            return report.clone();
        }
        let install = self.server.install();
        let dir = self.models_dir.clone();
        let report = tokio::task::spawn_blocking(move || hardware::detect(install.as_ref(), Some(&dir)))
            .await
            .unwrap_or_default();
        *self.hardware.lock().unwrap_or_else(PoisonError::into_inner) = Some((Instant::now(), report.clone()));
        report
    }

    pub async fn budget(&self) -> MemoryBudget {
        let report = self.hardware().await;
        MemoryBudget {
            vram_free: report.best_gpu().map(|gpu| gpu.free.unwrap_or(gpu.total)),
            ram_available: report.ram.available,
        }
    }

    pub fn llama_status(&self) -> Value {
        match self.server.install() {
            Some(install) => json!({ "installed": true, "install": install }),
            None => json!({ "installed": false, "installCommand": "winget install ggml.llamacpp" }),
        }
    }

    pub async fn search(&self, query: &str) -> Result<Vec<RepoSummary>, HfError> {
        self.hf.search(query).await
    }

    /// The facts of a repo, read from the header of one of its files and kept for this run.
    async fn repo_facts(&self, repo: &str, variants: &[Variant]) -> Option<ModelFacts> {
        if let Some(facts) = self.facts.lock().unwrap_or_else(PoisonError::into_inner).get(repo) {
            return facts.clone();
        }
        // The smallest variant's first file: every variant has the same architecture.
        let file = variants.iter().min_by_key(|variant| variant.size)?.files.first()?.path.clone();
        let facts = self.hf.remote_facts(repo, &file).await.ok().map(|remote| remote.facts);
        if facts.is_some() {
            self.facts.lock().unwrap_or_else(PoisonError::into_inner).insert(repo.to_string(), facts.clone());
        }
        facts
    }

    /// A repo's variants, each with its estimate, for `ctx` or the default context.
    pub async fn files(&self, repo: &str, ctx: Option<u32>) -> Result<RepoView, HfError> {
        let files = self.hf.files(repo).await?;
        let facts = self.repo_facts(repo, &files.variants).await;
        let ctx = ctx.unwrap_or_else(|| default_ctx(facts.as_ref()));
        let budget = self.budget().await;
        let variants: Vec<VariantView> = files
            .variants
            .into_iter()
            .map(|variant| VariantView {
                estimate: estimate::estimate(variant.size, facts.as_ref(), ctx, &budget),
                low_quality: vatra_models::variant::is_low_quant(&variant.name),
                variant,
            })
            .collect();
        let best_fit = variants.iter().map(|view| view.estimate.fit).min_by_key(|fit| *fit as u8);
        let fitted_ctx = facts.as_ref().and_then(|facts| {
            let smallest = variants.iter().map(|view| view.variant.size).min()?;
            estimate::max_gpu_ctx(smallest, facts, &budget, DEFAULT_CTX)
        });
        let (label, reasons) = label::estimated(facts.as_ref(), fitted_ctx.or(Some(ctx)), best_fit);
        Ok(RepoView { repo: files.repo, gated: files.gated, facts, ctx, variants, label, reasons })
    }

    pub async fn suggest(&self, task: Task) -> Vec<Suggestion> {
        let list = suggest::builtin();
        let budget = self.budget().await;
        let repos: Vec<String> =
            list.entries.iter().filter(|entry| entry.tasks.contains(&task)).map(|entry| entry.repo.clone()).collect();
        let fetched = futures_util::future::join_all(repos.iter().map(|repo| async move {
            let files = self.hf.files(repo).await.ok()?;
            let facts = self.repo_facts(repo, &files.variants).await;
            Some((repo.clone(), (files.variants, facts)))
        }))
        .await;
        let found: HashMap<String, (Vec<Variant>, Option<ModelFacts>)> = fetched.into_iter().flatten().collect();
        suggest::suggest(task, &budget, &list, &|repo| found.get(repo).cloned())
    }

    /// Downloads a variant the person chose, then records it. Progress goes out as notifications.
    pub async fn download(&self, repo: &str, variant: &str, with_mmproj: bool) -> Result<LocalModel, String> {
        let files = self.hf.files(repo).await.map_err(|error| error.to_string())?;
        if files.gated && !self.hf.has_token() {
            return Err(HfError::Gated.to_string());
        }
        let chosen = files
            .variants
            .iter()
            .find(|candidate| candidate.name == variant)
            .ok_or_else(|| format!("`{variant}` isn't in {repo}"))?;
        let installed = self.local_models();
        if installed.iter().any(|model| model.repo.as_deref() == Some(repo) && model.variant.as_deref() == Some(variant)) {
            return Err(format!("{repo} {variant} is already downloaded"));
        }
        let mut slug = slug(repo, variant);
        if installed.iter().any(|model| model.slug == slug) {
            slug = super::slug(&repo.replace('/', "-"), variant);
        }
        let dir = safe_join(&self.models_dir, repo)?;
        let mmproj = with_mmproj.then(|| chosen.mmproj.first().cloned()).flatten();
        let jobs: Vec<Job> = chosen
            .files
            .iter()
            .chain(mmproj.iter())
            .map(|file| {
                let url = self.hf.resolve_url(repo, &file.path);
                Ok(Job { token: self.hf.token_for(&url), url, file: file.clone(), dest: safe_join(&dir, &file.path)? })
            })
            .collect::<Result<_, String>>()?;
        let free = hardware::disk_free(&self.models_dir);
        let progress = |progress: DownloadProgress| self.notify("model_download", json!({ "state": "progress", "repo": repo, "variant": variant, "progress": progress }));
        if let Err(error) = self.downloader.run(&jobs, free, &progress).await {
            let state = if error == super::download::DownloadError::Cancelled { "cancelled" } else { "failed" };
            self.notify("model_download", json!({ "state": state, "repo": repo, "variant": variant, "message": error.to_string() }));
            return Err(error.to_string());
        }
        let model_files: Vec<PathBuf> = jobs.iter().take(chosen.files.len()).map(|job| job.dest.clone()).collect();
        let facts = local_facts(&model_files[0]).ok();
        let mut model = LocalModel {
            slug,
            repo: Some(repo.to_string()),
            variant: Some(variant.to_string()),
            sha256: chosen.files.iter().filter_map(|file| file.sha256.clone()).collect(),
            files: model_files,
            size_bytes: chosen.size.saturating_add(mmproj.as_ref().map_or(0, |file| file.size)),
            settings: ModelSettings {
                ctx: default_ctx(facts.as_ref()),
                mmproj: mmproj.as_ref().map(|_| jobs.last().expect("mmproj job").dest.clone()),
                fit: None,
            },
            facts,
            added_by: AddedBy::Download,
            created_at: now(),
        };
        self.measure_fit(&mut model).await;
        self.store.insert_local_model(&model.to_row()).map_err(|error| error.to_string())?;
        self.notify("model_download", json!({ "state": "done", "repo": repo, "variant": variant, "slug": model.slug }));
        Ok(model)
    }

    /// Uses a Hugging Face token, or none.
    pub fn set_hf_token(&self, token: Option<String>) {
        self.hf.set_token(token);
    }

    pub fn has_hf_token(&self) -> bool {
        self.hf.has_token()
    }

    pub fn cancel_download(&self) {
        self.downloader.cancel();
    }

    /// Records a GGUF file already on disk, without copying it.
    pub async fn add_path(&self, path: &Path) -> Result<LocalModel, String> {
        if !path.is_file() {
            return Err(format!("`{}` isn't a file", path.display()));
        }
        let facts = local_facts(path)?;
        let stem = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        let mut model = LocalModel {
            slug: slug(&stem, ""),
            repo: None,
            variant: None,
            files: vec![path.to_path_buf()],
            sha256: Vec::new(),
            size_bytes: std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0),
            settings: ModelSettings { ctx: default_ctx(Some(&facts)), mmproj: None, fit: None },
            facts: Some(facts),
            added_by: AddedBy::Path,
            created_at: now(),
        };
        if self.store.local_model(&model.slug).map_err(|error| error.to_string())?.is_some() {
            return Err(format!("a model named `{}` is already added", model.slug));
        }
        self.measure_fit(&mut model).await;
        self.store.insert_local_model(&model.to_row()).map_err(|error| error.to_string())?;
        Ok(model)
    }

    async fn measure_fit(&self, model: &mut LocalModel) {
        if let Some(install) = self.server.install() {
            model.settings.fit = super::server::fit_params(&install, model).await;
        }
    }

    pub fn local_models(&self) -> Vec<LocalModel> {
        self.store.local_models().unwrap_or_default().iter().filter_map(LocalModel::from_row).collect()
    }

    pub async fn installed(&self) -> Vec<InstalledView> {
        let running = self.server.current().await.map(|server| server.slug);
        self.local_models()
            .into_iter()
            .map(|model| InstalledView {
                agent: model.agent(),
                running: running.as_deref() == Some(model.slug.as_str()),
                missing: !model.path().is_file(),
                model,
            })
            .collect()
    }

    /// Removes a model. The files Vatra downloaded are deleted; a file added by path stays where it is.
    pub async fn remove(&self, slug: &str) -> Result<(), String> {
        let model = self
            .store
            .local_model(slug)
            .map_err(|error| error.to_string())?
            .and_then(|row| LocalModel::from_row(&row))
            .ok_or_else(|| format!("no model `{slug}`"))?;
        if self.server.current().await.is_some_and(|server| server.slug == slug) {
            self.server.stop_unused().await.map_err(|error| error.to_string())?;
        }
        if model.added_by == AddedBy::Download {
            let others = self.local_models();
            let shared = |file: &PathBuf| {
                others.iter().any(|other| other.slug != model.slug && other.settings.mmproj.as_ref() == Some(file))
            };
            for file in model.files.iter().chain(model.settings.mmproj.iter().filter(|file| !shared(file))) {
                let _ = std::fs::remove_file(file);
                // Empty folders up to the models folder go too.
                for dir in file.ancestors().skip(1).take_while(|dir| dir.starts_with(&self.models_dir) && *dir != self.models_dir) {
                    if std::fs::remove_dir(dir).is_err() {
                        break;
                    }
                }
            }
        }
        self.store.remove_local_model(slug, &model.agent()).map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn agent_names(&self) -> Vec<String> {
        self.local_models().iter().map(LocalModel::agent).collect()
    }

    pub fn model_for(&self, agent: &str) -> Option<LocalModel> {
        let slug = agent.strip_prefix(AGENT_PREFIX)?;
        self.store.local_model(slug).ok().flatten().and_then(|row| LocalModel::from_row(&row))
    }

    /// The running server for a local agent, started if needed, with a guard that keeps it while a turn uses it.
    pub async fn resolve(&self, agent: &str) -> Result<(LocalModel, RunningServer, TurnGuard), String> {
        let model = self.model_for(agent).ok_or_else(|| format!("unknown local model `{agent}`"))?;
        if !model.path().is_file() {
            return Err(format!("the model file is missing: {}", model.path().display()));
        }
        let (server, guard) = self.server.ensure(&model).await.map_err(|error: ServerError| error.to_string())?;
        Ok((model, server, guard))
    }

    pub async fn start(&self, slug: &str) -> Result<RunningServer, String> {
        let (_, server, _guard) = self.resolve(&format!("{AGENT_PREFIX}{slug}")).await?;
        Ok(server)
    }

    /// Stops the local server, unless a conversation is using it.
    pub async fn stop(&self) -> Result<(), String> {
        self.server.stop_unused().await.map_err(|error| error.to_string())
    }

    /// The MCP server the check calls, with a token made for this run.
    pub fn probe_server(&self) -> McpServerConfig {
        let token: String = (0..12).map(|_| char::from(b'a' + rand::random_range(0..26u8))).collect();
        McpServerConfig {
            name: "vatra_probe".into(),
            command: Some(self.probe.0.to_string_lossy().into_owned()),
            args: self.probe.1.clone(),
            env: [(super::probe::TOKEN_VAR.to_string(), token)].into(),
            cwd: None,
            url: None,
            headers: Default::default(),
            headers_helper: None,
            oauth: None,
            tool_timeout_sec: Some(30),
            startup_timeout_sec: Some(30),
            enabled_tools: None,
            disabled_tools: Vec::new(),
            always_load: true,
            plugin: None,
            secrets: Default::default(),
            plugin_context: None,
        }
    }

    /// The settings a check of a local model is compared against, to see whether it is stale.
    pub fn settings_key(&self, model: &LocalModel) -> String {
        json!({ "ctx": model.settings.ctx, "mmproj": model.settings.mmproj, "file": model.path() }).to_string()
    }

    pub fn record_check(&self, record: &super::check::CheckRecord, consented_cost: bool) -> Result<(), String> {
        let row = vatra_core::ModelCheckRow {
            agent: record.agent.clone(),
            started_at: record.started_at,
            finished_at: record.finished_at,
            llama_build: record.llama_build,
            settings_json: record.settings_json.clone(),
            results_json: json!({ "results": record.results, "reasons": record.reasons }).to_string(),
            label: serde_json::to_value(record.label).ok().and_then(|value| value.as_str().map(String::from)).unwrap_or_default(),
            consented_cost,
        };
        self.store.insert_model_check(&row).map(|_| ()).map_err(|error| error.to_string())
    }

    /// One check at a time, so a paid check can't run twice at once.
    pub async fn check_lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.checks.lock().await
    }

    pub fn notify_check(&self, params: Value) {
        self.notify("model_check", params);
    }

    async fn openrouter_model(&self, model: &str) -> Option<OpenRouterModel> {
        if self.openrouter.lock().unwrap_or_else(PoisonError::into_inner).is_none() {
            let list: Option<Vec<OpenRouterModel>> = async {
                let response = reqwest::Client::new()
                    .get(&self.openrouter_url)
                    .timeout(Duration::from_secs(20))
                    .send()
                    .await
                    .ok()?;
                let body: Value = response.json().await.ok()?;
                serde_json::from_value(body["data"].clone()).ok()
            }
            .await;
            *self.openrouter.lock().unwrap_or_else(PoisonError::into_inner) = Some(list.unwrap_or_default());
        }
        self.openrouter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()?
            .iter()
            .find(|candidate| candidate.id == model)
            .cloned()
    }

    /// The label of every engine agent: the last check when there is one, otherwise the estimate.
    pub async fn labels(&self, configured: &[ConfiguredModel]) -> Vec<LabelView> {
        let mut views = Vec::new();
        let build = self.server.install().and_then(|install| install.build);
        let budget = self.budget().await;
        for model in self.local_models() {
            let agent = model.agent();
            if configured.iter().any(|configured| configured.name == agent) {
                continue;
            }
            let measured = self.measured(&agent, Some(&self.settings_key(&model)), build);
            views.push(measured.unwrap_or_else(|| {
                let fitted = model.facts.as_ref().and_then(|facts| estimate::max_gpu_ctx(model.size_bytes, facts, &budget, model.settings.ctx));
                let fit = estimate::estimate(model.size_bytes, model.facts.as_ref(), model.settings.ctx, &budget).fit;
                let fitted = model.settings.fit.as_ref().map(|fit| fit.ctx).or(fitted).or(Some(model.settings.ctx));
                let (label, reasons) = label::estimated(model.facts.as_ref(), fitted, Some(fit));
                estimated(agent, label, reasons)
            }));
        }
        for model in configured {
            let key = api_settings_key(Some(&model.driver), model.base_url.as_deref(), &model.model);
            let measured = self.measured(&model.name, Some(&key), None);
            let view = match measured {
                Some(view) => view,
                None => {
                    let openrouter = match &model.base_url {
                        Some(url) if url.contains("openrouter.ai") => self.openrouter_model(&model.model).await,
                        _ => None,
                    };
                    let (label, reasons) = label::api_estimate(&model.driver, openrouter.as_ref());
                    estimated(model.name.clone(), label, reasons)
                }
            };
            views.push(view);
        }
        views
    }

    fn measured(&self, agent: &str, settings: Option<&str>, build: Option<u32>) -> Option<LabelView> {
        let check = self.store.latest_model_check(agent).ok()??;
        let results: Value = serde_json::from_str(&check.results_json).unwrap_or(Value::Null);
        let label: Label = serde_json::from_value(Value::String(check.label.clone())).ok()?;
        let reasons: Vec<LabelReason> = serde_json::from_value(results["reasons"].clone()).unwrap_or_default();
        let stale = settings.is_some_and(|settings| settings != check.settings_json)
            || (build.is_some() && check.llama_build.is_some() && build != check.llama_build);
        Some(LabelView {
            agent: agent.to_string(),
            label,
            reasons,
            estimated: false,
            checked_at: Some(check.finished_at),
            llama_build: check.llama_build,
            stale,
            results: Some(results["results"].clone()),
        })
    }
}

fn estimated(agent: String, label: Label, reasons: Vec<LabelReason>) -> LabelView {
    LabelView { agent, label, reasons, estimated: true, checked_at: None, llama_build: None, stale: false, results: None }
}

/// Joins a repo name or a repo file path under `base`, refusing anything that could leave it.
pub fn safe_join(base: &Path, relative: &str) -> Result<PathBuf, String> {
    let mut path = base.to_path_buf();
    for part in relative.split('/') {
        let unsafe_part = part.is_empty()
            || part == "."
            || part == ".."
            || part.contains(['\\', ':'])
            || part.chars().any(char::is_control);
        if unsafe_part {
            return Err(format!("refusing the file name `{relative}`"));
        }
        path.push(part);
    }
    Ok(path)
}

/// What an API check depends on: when any of it changes, the check is stale.
pub fn api_settings_key(driver: Option<&str>, base_url: Option<&str>, model: &str) -> String {
    json!({ "driver": driver.unwrap_or("openai"), "baseUrl": base_url, "model": model }).to_string()
}

fn default_ctx(facts: Option<&ModelFacts>) -> u32 {
    facts
        .and_then(|facts| facts.context_length)
        .map(|max| max.min(DEFAULT_CTX as u64) as u32)
        .unwrap_or(DEFAULT_CTX)
}
