use std::fs::File;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use vatra_core::Store;
use vatra_extensions::{Catalog, CatalogError};

use crate::agents::{BridgeSettings, ConfigAgents};
use crate::config::Config;
use crate::rpc::router;
use crate::service::{Service, ServiceError};

const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub pid: u32,
    pub port: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("the Vatra service is already running")]
    AlreadyRunning,
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("couldn't open the database: {0}")]
    Store(#[from] vatra_core::StoreError),
    #[error("couldn't open the extension catalog: {0}")]
    Catalog(#[from] CatalogError),
    #[error(transparent)]
    Service(#[from] ServiceError),
}

pub struct RunningService {
    _reminders: crate::reminders::ReminderLoop,
    _scheduler: crate::scheduler::SchedulerLoop,
    _telegram: crate::telegram::TelegramLoop,
    _semantic: crate::semantic::SemanticLoop,
    address: SocketAddr,
    server: JoinHandle<()>,
    service: Arc<Service>,
    // Held for the life of the service. The OS releases it even after a crash.
    _lock: File,
}

impl RunningService {
    pub fn port(&self) -> u16 {
        self.address.port()
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn service(&self) -> &Arc<Service> {
        &self.service
    }

    pub async fn wait(mut self) {
        let service = Arc::clone(&self.service);
        tokio::select! {
            _ = &mut self.server => {}
            () = service.shutdown_requested() => {}
        }
    }
}

impl Drop for RunningService {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// The running service recorded in `data_dir`, if it really answers as this Vatra process.
pub async fn existing_service(data_dir: &Path) -> Option<ServiceInfo> {
    let text = std::fs::read_to_string(data_dir.join("service.json")).ok()?;
    let info: ServiceInfo = serde_json::from_str(&text).ok()?;
    let pid = tokio::time::timeout(HEALTH_TIMEOUT, health_pid(info.port))
        .await
        .ok()??;
    (pid == info.pid).then_some(info)
}

async fn health_pid(port: u16) -> Option<u32> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
    let request =
        format!("GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await.ok()?;
    let body: serde_json::Value = serde_json::from_str(response.split("\r\n\r\n").nth(1)?).ok()?;
    if body["service"] != "vatra" {
        return None;
    }
    body["pid"].as_u64().map(|pid| pid as u32)
}

fn new_token() -> String {
    let bytes: [u8; 32] = rand::random();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub struct Runtime;

impl Runtime {
    pub async fn start(data_dir: &Path, config: Config) -> Result<RunningService, RuntimeError> {
        Self::start_with_notifier(data_dir, config, Arc::new(crate::reminders::windows::WindowsReminderNotifier::new(crate::reminders::windows::VATRA_APP_ID))).await
    }
    pub async fn start_with_notifier(data_dir: &Path, config: Config, notifier: Arc<dyn crate::reminders::ReminderNotifier>) -> Result<RunningService, RuntimeError> {
        std::fs::create_dir_all(data_dir)?;
        let lock = File::create(data_dir.join("service.lock"))?;
        if lock.try_lock().is_err() {
            return Err(RuntimeError::AlreadyRunning);
        }

        let token_file = data_dir.join("token");
        let token: Arc<str> = new_token().into();
        std::fs::write(&token_file, token.as_bytes())?;

        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let bridge = BridgeSettings {
            binary: bridge_binary(),
            url: format!("http://{address}"),
            token_file,
            data_dir: data_dir.to_path_buf(),
        };
        let store = Arc::new(Store::open(&data_dir.join("vatra.db"))?);
        let catalog = Arc::new(Catalog::open(&data_dir.join("vatra.db"), data_dir)?);
        if let Err(error) = catalog.prune() {
            eprintln!("warning: old extension copies weren't removed: {error}");
        }
        let auth = Arc::new(
            crate::chatgpt::ChatGptAuth::new(
                data_dir,
                Arc::new(vatra_core::KeyringStore::new(vatra_core::KeyringBackend)),
            )
            .map_err(|error| std::io::Error::other(error.to_string()))?,
        );
        let models_dir = config.local.models_dir.clone().unwrap_or_else(default_models_dir);
        // Finding llama.cpp runs it twice; that waits on a blocking thread, not the runtime.
        let configured = config.local.llama_server.clone();
        let llama = tokio::task::spawn_blocking(move || crate::models::ModelManager::find_llama(configured.as_deref()))
            .await
            .unwrap_or(None);
        let models = crate::models::ModelManager::new(
            Arc::clone(&store),
            models_dir,
            llama,
            (bridge_binary(), vec!["mcp-probe".into()]),
        );
        let keyring = vatra_core::KeyringStore::new(vatra_core::KeyringBackend);
        if let Ok(Some(token)) = vatra_core::SecretStore::get(&keyring, crate::models::rpc::HF_SECRET) {
            models.set_hf_token(Some(token));
        }
        if let Some(path) = config.local.llama_server.clone() {
            models.set_llama_path(path);
        }
        models.watch_idle(crate::models::server::IDLE_TIMEOUT);
        // Agents set up in the app join the ones in vatra.toml; their keys come from the keychain.
        let rows = store.configured_agents().unwrap_or_default();
        let summarizer = store.app_setting("summarizer").ok().flatten();
        let keychain = vatra_core::KeyringStore::new(vatra_core::KeyringBackend);
        let file_agents = config.agents.keys().cloned().collect();
        let (config, warnings) = config.merged_with(&rows, summarizer.as_deref(), &keychain);
        for warning in warnings {
            eprintln!("warning: {warning}");
        }
        let agents = ConfigAgents::with_bridge(config, bridge)
            .with_file_agents(file_agents)
            .with_models(models)
            .with_chatgpt(auth)
            .with_catalog_synced(catalog)
            .with_store(Arc::clone(&store));
        let service = Service::new(store, Arc::new(agents))?;
        service.set_reminder_notifier(Arc::new(crate::ntfy::PushNotifier { inner: notifier, service: Arc::downgrade(&service), handle: tokio::runtime::Handle::current() }));
        service.set_connector_registry(crate::connectors::ConnectorRegistry::with_builtin());
        let scheduler = crate::scheduler::start(&service);
        let telegram = crate::telegram::start(&service);
        let semantic = crate::semantic::start(&service);
        let weak = Arc::downgrade(&service);
        let reminders = crate::reminders::ReminderDispatcher::new(Arc::clone(service.store()),service.reminder_notifier())
            .with_updates(Arc::new(move |project_id| {if let Some(service)=weak.upgrade() {service.notify(crate::Notification::RemindersChanged {project_id});}})).start();

        let app = router(service.clone(), token);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let info = ServiceInfo {
            pid: std::process::id(),
            port: address.port(),
        };
        std::fs::write(
            data_dir.join("service.json"),
            serde_json::to_string(&info).expect("serializabil"),
        )?;
        Ok(RunningService {
            _reminders: reminders,
            _scheduler: scheduler,
            _telegram: telegram,
            _semantic: semantic,
            address,
            server,
            service,
            _lock: lock,
        })
    }
}

/// The folder of the app's data: `%APPDATA%\Vatra`. The desktop app, the service and the CLI all default to it, so
/// they share one database.
pub fn default_data_dir() -> Result<PathBuf, String> {
    std::env::var_os("APPDATA")
        .map(|base| PathBuf::from(base).join("Vatra"))
        .ok_or_else(|| "APPDATA isn't set; use --data-dir".to_string())
}

/// Where downloaded models go: `%LOCALAPPDATA%\Vatra\models`, kept apart from the roaming app data.
pub fn default_models_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(|base| PathBuf::from(base).join("Vatra").join("models"))
        .unwrap_or_else(|| PathBuf::from("models"))
}

fn bridge_binary() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("vatra"))
}
