//! The llama.cpp server for local models: one at a time, on loopback, started when a turn needs it and stopped
//! after a while without turns.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use vatra_core::ProcessGroup;
use vatra_models::llama::LlamaInstall;

use super::{FitResult, LocalModel, MIN_AGENT_CTX};

const START_TIMEOUT: Duration = Duration::from_secs(120);
const LOG_LINES: usize = 200;
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningServer {
    pub slug: String,
    pub base_url: String,
    /// The context the server reports in `/props`.
    pub ctx: Option<u32>,
    pub device: Option<String>,
    pub gpu_layers: Option<String>,
    #[serde(skip)]
    pub started_at: Option<Instant>,
    /// Made for this start and given to llama.cpp through its environment, never on the command line.
    #[serde(skip)]
    pub api_key: String,
    pub load_seconds: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("llama.cpp isn't installed. Install it with: winget install ggml.llamacpp")]
    NoLlama,
    #[error("`{other}` is in use by another conversation; try again when it finishes")]
    Busy { other: String },
    #[error("llama.cpp stopped while loading the model:\n{}", log_tail.join("\n"))]
    Exited { log_tail: Vec<String> },
    #[error("llama.cpp didn't finish loading the model in time:\n{}", log_tail.join("\n"))]
    Timeout { log_tail: Vec<String> },
    #[error("couldn't start llama.cpp: {0}")]
    Spawn(String),
}

/// What the app shows about the server.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerState {
    pub slug: String,
    /// `starting`, `ready`, `stopped` or `failed`.
    pub state: &'static str,
    pub detail: Option<String>,
}

type Listener = Arc<dyn Fn(ServerState) + Send + Sync>;

struct Running {
    info: RunningServer,
    child: Child,
    group: Option<ProcessGroup>,
}

/// Keeps the server busy while a turn uses it; the idle timer starts when the last one ends.
pub struct TurnGuard {
    active: Arc<AtomicUsize>,
    last_used: Arc<Mutex<Instant>>,
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        *self.last_used.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct ModelServer {
    /// Set again when llama.cpp is installed while the service runs.
    install: Mutex<Option<LlamaInstall>>,
    running: tokio::sync::Mutex<Option<Running>>,
    log: Arc<Mutex<VecDeque<String>>>,
    active: Arc<AtomicUsize>,
    last_used: Arc<Mutex<Instant>>,
    listener: Mutex<Option<Listener>>,
    http: reqwest::Client,
}

impl ModelServer {
    pub fn new(install: Option<LlamaInstall>) -> Arc<ModelServer> {
        Arc::new(ModelServer {
            install: Mutex::new(install),
            running: tokio::sync::Mutex::new(None),
            log: Arc::default(),
            active: Arc::default(),
            last_used: Arc::new(Mutex::new(Instant::now())),
            listener: Mutex::new(None),
            http: reqwest::Client::new(),
        })
    }

    pub fn install(&self) -> Option<LlamaInstall> {
        self.install.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn set_install(&self, install: Option<LlamaInstall>) {
        *self.install.lock().unwrap_or_else(PoisonError::into_inner) = install;
    }

    pub fn on_state(&self, listener: Listener) {
        *self.listener.lock().unwrap_or_else(PoisonError::into_inner) = Some(listener);
    }

    fn emit(&self, slug: &str, state: &'static str, detail: Option<String>) {
        let listener = self.listener.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(listener) = listener {
            listener(ServerState { slug: slug.to_string(), state, detail });
        }
    }

    /// Stops the server after `idle` without turns. Runs until the server object is dropped.
    pub fn watch_idle(self: &Arc<Self>, idle: Duration) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep((idle / 4).max(Duration::from_millis(20))).await;
                let Some(server) = weak.upgrade() else { return };
                let quiet = server.last_used.lock().unwrap_or_else(PoisonError::into_inner).elapsed() >= idle;
                if quiet {
                    let _ = server.stop_unused().await;
                }
            }
        });
    }

    pub async fn current(&self) -> Option<RunningServer> {
        self.running.lock().await.as_ref().map(|running| running.info.clone())
    }

    /// The server for this model, started if needed. Another model's server is stopped first, unless a turn uses it.
    pub async fn ensure(&self, model: &LocalModel) -> Result<(RunningServer, TurnGuard), ServerError> {
        let mut running = self.running.lock().await;
        if let Some(current) = running.as_mut() {
            let exited = current.child.try_wait().ok().flatten().is_some();
            if current.info.slug == model.slug && !exited {
                return Ok((current.info.clone(), self.guard()));
            }
            if !exited && self.active.load(Ordering::SeqCst) > 0 {
                return Err(ServerError::Busy { other: format!("local:{}", current.info.slug) });
            }
            let previous = running.take().expect("checked above");
            self.kill(previous).await;
        }
        let install = self.install().ok_or(ServerError::NoLlama)?;
        self.emit(&model.slug, "starting", None);
        match self.start(&install, model).await {
            Ok(started) => {
                let info = started.info.clone();
                *running = Some(started);
                self.emit(&model.slug, "ready", info.device.clone());
                Ok((info, self.guard()))
            }
            Err(error) => {
                self.emit(&model.slug, "failed", Some(error.to_string()));
                Err(error)
            }
        }
    }

    fn guard(&self) -> TurnGuard {
        self.active.fetch_add(1, Ordering::SeqCst);
        TurnGuard { active: Arc::clone(&self.active), last_used: Arc::clone(&self.last_used) }
    }

    /// Stops the server whatever uses it: for the service shutting down.
    pub async fn stop(&self) {
        if let Some(running) = self.running.lock().await.take() {
            let slug = running.info.slug.clone();
            self.kill(running).await;
            self.emit(&slug, "stopped", None);
        }
    }

    /// Stops the server unless a turn uses it. Turn guards are only handed out under the same lock, so a turn
    /// can't start between the check and the stop.
    pub async fn stop_unused(&self) -> Result<(), ServerError> {
        let mut running = self.running.lock().await;
        let Some(current) = running.as_ref() else { return Ok(()) };
        if self.active.load(Ordering::SeqCst) > 0 {
            return Err(ServerError::Busy { other: format!("local:{}", current.info.slug) });
        }
        let current = running.take().expect("checked above");
        let slug = current.info.slug.clone();
        self.kill(current).await;
        self.emit(&slug, "stopped", None);
        Ok(())
    }

    async fn kill(&self, mut running: Running) {
        if let Some(group) = &running.group {
            let _ = group.kill_all();
        }
        let _ = running.child.kill().await;
    }

    /// The last lines llama.cpp printed.
    pub fn log_tail(&self, lines: usize) -> Vec<String> {
        let log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        log.iter().skip(log.len().saturating_sub(lines)).cloned().collect()
    }

    async fn start(&self, install: &LlamaInstall, model: &LocalModel) -> Result<Running, ServerError> {
        self.log.lock().unwrap_or_else(PoisonError::into_inner).clear();
        // The port is free when picked; if another program takes it first, llama.cpp exits and the next try picks again.
        let mut last = None;
        for _ in 0..2 {
            let port = free_port().map_err(|error| ServerError::Spawn(error.to_string()))?;
            match self.spawn(install, model, port).await {
                Err(ServerError::Exited { log_tail }) if log_tail.iter().any(|line| line.contains("bind")) => {
                    last = Some(ServerError::Exited { log_tail });
                }
                other => return other,
            }
        }
        Err(last.expect("two tries"))
    }

    async fn spawn(&self, install: &LlamaInstall, model: &LocalModel, port: u16) -> Result<Running, ServerError> {
        let started = Instant::now();
        let api_key: String = (0..32).map(|_| format!("{:x}", rand::random_range(0..16u8))).collect();
        let mut command = Command::new(&install.server);
        command
            .args(args(install, model, port, None))
            .env("LLAMA_API_KEY", &api_key)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child = command.spawn().map_err(|error| ServerError::Spawn(error.to_string()))?;
        let group = ProcessGroup::new().ok().filter(|group| group.adopt(&child).is_ok());
        for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Send + Unpin>), child.stderr.take().map(|p| Box::new(p) as _)].into_iter().flatten() {
            let log = Arc::clone(&self.log);
            tokio::spawn(async move {
                let mut lines = BufReader::new(pipe).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut log = log.lock().unwrap_or_else(PoisonError::into_inner);
                    if log.len() == LOG_LINES {
                        log.pop_front();
                    }
                    log.push_back(line);
                }
            });
        }
        let base = format!("http://127.0.0.1:{port}");
        let mut running = Running {
            info: RunningServer {
                slug: model.slug.clone(),
                base_url: format!("{base}/v1"),
                ctx: None,
                device: None,
                gpu_layers: None,
                started_at: Some(started),
                load_seconds: 0.0,
                api_key,
            },
            child,
            group,
        };
        loop {
            if running.child.try_wait().ok().flatten().is_some() {
                // Give the readers a moment to take the last lines.
                tokio::time::sleep(Duration::from_millis(100)).await;
                return Err(ServerError::Exited { log_tail: self.log_tail(20) });
            }
            if started.elapsed() > START_TIMEOUT {
                self.kill(running).await;
                return Err(ServerError::Timeout { log_tail: self.log_tail(20) });
            }
            let healthy = self
                .http
                .get(format!("{base}/health"))
                .timeout(Duration::from_secs(2))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success());
            if healthy {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        running.info.load_seconds = started.elapsed().as_secs_f64();
        running.info.ctx = self.props_ctx(&base, &running.info.api_key).await;
        let log = self.log_tail(LOG_LINES);
        running.info.device = device_from_log(&log);
        running.info.gpu_layers = layers_from_log(&log);
        Ok(running)
    }

    async fn props_ctx(&self, base: &str, key: &str) -> Option<u32> {
        let props: serde_json::Value = self
            .http
            .get(format!("{base}/props"))
            .bearer_auth(key)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        props["default_generation_settings"]["n_ctx"]
            .as_u64()
            .or(props["n_ctx"].as_u64())
            .and_then(|ctx| u32::try_from(ctx).ok())
    }
}

/// The arguments for `llama-server`. With `--fit`, llama.cpp places the layers itself; without it, `ngl` decides.
pub fn args(install: &LlamaInstall, model: &LocalModel, port: u16, ngl: Option<u32>) -> Vec<OsString> {
    let caps = install.caps;
    let mut args: Vec<OsString> = vec!["-m".into(), model.path().into(), "--host".into(), "127.0.0.1".into()];
    args.extend(["--port".into(), port.to_string().into(), "-c".into(), model.settings.ctx.to_string().into()]);
    if caps.jinja {
        args.push("--jinja".into());
    }
    if caps.fit {
        args.extend(["--fit".into(), "on".into()]);
        if caps.fit_ctx {
            args.extend(["--fit-ctx".into(), MIN_AGENT_CTX.to_string().into()]);
        }
    } else if let Some(layers) = ngl.or_else(|| model.settings.fit.as_ref().and_then(|fit| fit.gpu_layers.parse().ok())) {
        args.extend(["-ngl".into(), layers.to_string().into()]);
    }
    if let (true, Some(mmproj)) = (caps.mmproj, &model.settings.mmproj) {
        args.extend(["--mmproj".into(), mmproj.into()]);
    }
    if caps.no_webui {
        args.push("--no-webui".into());
    }
    if caps.offline {
        args.push("--offline".into());
    }
    args
}

/// Runs `llama-fit-params` on the real file: the GPU layers and context llama.cpp would pick.
pub async fn fit_params(install: &LlamaInstall, model: &LocalModel) -> Option<FitResult> {
    let program = install.fit_params.as_ref()?;
    let mut command = Command::new(program);
    command
        .args(["-m".as_ref(), model.path().as_os_str(), "-c".as_ref(), model.settings.ctx.to_string().as_ref()])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = tokio::time::timeout(Duration::from_secs(60), command.output()).await.ok()?.ok()?;
    let text = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    parse_fit_params(&text, model.settings.ctx)
}

/// `llama-fit-params` ends with the arguments it chose, like `-c 32768 -ngl 37`.
pub fn parse_fit_params(text: &str, ctx: u32) -> Option<FitResult> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let after = |flags: &[&str]| {
        words.windows(2).rev().find(|pair| flags.contains(&pair[0])).map(|pair| pair[1].trim_matches(['\'', '"']))
    };
    let gpu_layers = after(&["-ngl", "--n-gpu-layers", "--gpu-layers"])?.to_string();
    let ctx = after(&["-c", "--ctx-size"]).and_then(|value| value.parse().ok()).unwrap_or(ctx);
    Some(FitResult { gpu_layers, ctx })
}

/// Like `load_tensors: offloaded 37/37 layers to GPU`.
fn layers_from_log(log: &[String]) -> Option<String> {
    log.iter().rev().find_map(|line| {
        let rest = line.split("offloaded ").nth(1)?;
        Some(rest.split_whitespace().next()?.to_string())
    })
}

/// Like `llama_model_load_from_file_impl: using device Vulkan0 (NVIDIA GeForce RTX 5060) ...`.
fn device_from_log(log: &[String]) -> Option<String> {
    log.iter().find_map(|line| {
        let rest = line.split("using device ").nth(1)?;
        Some(rest.split(" - ").next()?.trim().to_string())
    })
}

fn free_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

