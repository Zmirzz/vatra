use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Notify, broadcast, oneshot};
pub use vatra_core::PermissionMode;
use vatra_core::{
    Event, RunError, Store, StoreError, StoredEvent, create_handoff_session, run_handoff,
};
use vatra_engine::{McpHub, has_credentials, login};
use vatra_import::{
    ApplyOutcome, GuardedFs, ImportChoice, ItemState, ScanView, SourceDirs, SystemProbe,
    apply_reporting, approvals, probe_items, scan_view,
};

use crate::agents::{AgentFactory, BuildRequest, TurnOptions};
use crate::approvals::ServiceApprover;
use crate::questions::ServiceQuestioner;

const NOTIFICATION_CAPACITY: usize = 1024;
/// Larger events are saved in full, but clients get only their seq so one big result can't stall the socket.
const MAX_EVENT_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub session_id: i64,
    pub tool: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuestionRequest {
    pub id: String,
    pub session_id: i64,
    pub questions: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Notification {
    RemindersChanged { project_id: i64 },
    ScheduleChanged,
    CalendarChanged,
    ResearchChanged { id: i64 },
    WorkspaceChanged { project_id: i64, item_ids: Vec<i64> },
    WorkspaceProposalsChanged { project_id: i64, target_id: Option<i64> },
    ChatgptLogin {
        ok: bool,
        message: String,
    },
    Event(StoredEvent),
    EventTooLarge {
        session_id: i64,
        seq: i64,
    },
    TurnState {
        session_id: i64,
        running: bool,
    },
    Approval(ApprovalRequest),
    ApprovalResolved {
        id: String,
    },
    Question(QuestionRequest),
    QuestionResolved {
        id: String,
    },
    /// A failure the user should see, such as events that couldn't be saved.
    Problem {
        session_id: i64,
        message: String,
    },
    /// The result of an MCP login, which runs in the background.
    McpLogin {
        server: String,
        ok: bool,
        message: String,
    },
    /// A web address that the app opens in the browser, for an MCP login.
    OpenUrl {
        url: String,
    },
    /// Progress of an import being applied: how many items are done, out of how many, and the item now.
    ImportProgress {
        done: usize,
        total: usize,
        label: String,
    },
    /// Downloads, checks and the local model server: `method` is the notification name the app receives.
    Models {
        method: &'static str,
        params: Value,
    },
}

/// An MCP server of the configuration, as the app shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct McpServerStatus {
    pub name: String,
    pub http: bool,
    pub oauth: bool,
    pub logged_in: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("this session is busy")]
    Busy,
    #[error("this session doesn't exist")]
    UnknownSession,
    #[error("this approval request no longer exists")]
    UnknownApproval,
    #[error("this question no longer exists")]
    UnknownQuestion,
    #[error("this MCP server isn't configured")]
    UnknownMcpServer,
    #[error("this MCP server doesn't use OAuth")]
    NoOAuth,
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{0}")]
    Import(String),
    #[error("{0}")]
    Models(String),
}

struct PendingApproval {
    request: ApprovalRequest,
    responder: oneshot::Sender<bool>,
}

struct PendingQuestion {
    request: QuestionRequest,
    /// None when the turn ends before the user answers.
    responder: oneshot::Sender<Option<Value>>,
}

pub struct Service {
    reminder_notifier: Mutex<Arc<dyn crate::reminders::ReminderNotifier>>,
    pub(crate) parity_gate: tokio::sync::Mutex<()>,
    store: Arc<Store>,
    agents: Arc<dyn AgentFactory>,
    notifications: broadcast::Sender<Notification>,
    /// Stop signals for the sessions a turn is using. A handoff holds two sessions.
    running: Mutex<HashMap<i64, u64>>,
    stops: Mutex<HashMap<u64, Option<oneshot::Sender<()>>>>,
    approvals: Mutex<HashMap<String, PendingApproval>>,
    questions: Mutex<HashMap<String, PendingQuestion>>,
    /// The projects whose MCP servers completion has already started in the background.
    warming: Mutex<HashMap<i64, tokio::task::JoinHandle<()>>>,
    /// One import is applied at a time, so two never write the catalog together.
    import_lock: Arc<Mutex<()>>,
    import_cancelled: Arc<AtomicBool>,
    chatgpt_cancel: Mutex<Option<crate::chatgpt::Cancellation>>,
    /// The kinds of connector this service knows. It starts empty; kinds are registered with set_connector_registry.
    connectors: Mutex<Arc<crate::connectors::ConnectorRegistry>>,
    /// The folders an import reads: this machine's, except in tests.
    source_dirs: SourceDirs,
    /// What the last import applied by this service did.
    last_import: Mutex<Option<ApplyOutcome>>,
    next_id: AtomicU64,
    shutdown: Notify,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Drop for Service {
    fn drop(&mut self) {
        for (_, warming) in lock(&self.warming).drain() {
            warming.abort();
        }
    }
}

impl Service {
    pub fn new(
        store: Arc<Store>,
        agents: Arc<dyn AgentFactory>,
    ) -> Result<Arc<Service>, ServiceError> {
        Self::with_source_dirs(store, agents, SourceDirs::detect())
    }

    /// A service that reads the import sources from the given folders.
    pub fn with_source_dirs(
        store: Arc<Store>,
        agents: Arc<dyn AgentFactory>,
        source_dirs: SourceDirs,
    ) -> Result<Arc<Service>, ServiceError> {
        store.clear_workspace_contexts()?;
        store.recover_reminders()?;
        store.recover_email_drafts(crate::scheduler::now())?;
        store.recover_research(crate::scheduler::now())?;
        for session in store.unfinished_sessions()? {
            let agent = store.session_agent(session)?;
            let message = "This turn was interrupted when the service stopped.".to_string();
            store.append(session, &Event::TurnFailed { agent, message })?;
        }
        let (notifications, _) = broadcast::channel(NOTIFICATION_CAPACITY);
        if let Some(models) = agents.models() {
            models.set_notifier(notifications.clone());
        }
        Ok(Arc::new(Service {
            reminder_notifier: Mutex::new(Arc::new(crate::reminders::UnavailableNotifier)),
            parity_gate: tokio::sync::Mutex::new(()),
            store,
            agents,
            notifications,
            running: Mutex::new(HashMap::new()),
            stops: Mutex::new(HashMap::new()),
            approvals: Mutex::new(HashMap::new()),
            questions: Mutex::new(HashMap::new()),
            warming: Mutex::new(HashMap::new()),
            import_lock: Arc::new(Mutex::new(())),
            import_cancelled: Arc::new(AtomicBool::new(false)),
            chatgpt_cancel: Mutex::new(None),
            connectors: Mutex::new(Arc::new(crate::connectors::ConnectorRegistry::new())),
            source_dirs,
            last_import: Mutex::new(None),
            next_id: AtomicU64::new(1),
            shutdown: Notify::new(),
        }))
    }

    /// Starts the MCP servers of a project in the background, once. Completion uses it and never waits for them. The
    /// task is aborted when the service drops, so a server still connecting doesn't outlive it. A server that doesn't
    /// answer within its startup timeout is marked failed and isn't tried again by this warm-up.
    pub(crate) fn warm_up(&self, project_id: i64, hub: Arc<McpHub>) {
        lock(&self.warming)
            .entry(project_id)
            .or_insert_with(|| tokio::spawn(async move { hub.ensure_connected().await }));
    }

    /// Asks the process running this service to exit. The desktop app starts a fresh one.
    /// Runs the short check on an engine agent and keeps its result. A check that may cost money needs `consent_cost`.
    pub async fn check_model(
        &self,
        agent: &str,
        consent_cost: bool,
    ) -> Result<crate::models::check::CheckRecord, ServiceError> {
        let models = self.models()?;
        let costs = self.agents.check_costs(agent);
        if costs && !consent_cost {
            return Err(ServiceError::Models(
                "This check sends a few short requests your provider may charge for. Confirm to run it.".into(),
            ));
        }
        let _one_at_a_time = models.check_lock().await;
        let target = self.agents.check_target(agent).await.map_err(ServiceError::Models)?;
        let probe = models.probe_server();
        let record = crate::models::check::run_check(target, probe, &|progress| {
            models.notify_check(serde_json::json!(progress))
        })
        .await;
        // A run that couldn't finish says nothing about the model, so the last real result stays.
        if let Some(reason) = &record.incomplete {
            return Err(ServiceError::Models(format!("The check couldn't finish: {reason}")));
        }
        models.record_check(&record, costs).map_err(ServiceError::Models)?;
        Ok(record)
    }

    pub fn models(&self) -> Result<Arc<crate::models::ModelManager>, ServiceError> {
        self.agents
            .models()
            .ok_or_else(|| ServiceError::Models("local models aren't available here".into()))
    }

    pub fn request_shutdown(&self) {
        self.cancel_chatgpt();
        self.shutdown.notify_one();
    }

    pub async fn shutdown_requested(&self) {
        self.shutdown.notified().await;
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }
    pub fn reminder_notifier(&self) -> Arc<dyn crate::reminders::ReminderNotifier> { Arc::clone(&lock(&self.reminder_notifier)) }
    pub fn set_reminder_notifier(&self, notifier: Arc<dyn crate::reminders::ReminderNotifier>) { *lock(&self.reminder_notifier) = notifier; }

    pub fn agents(&self) -> &Arc<dyn AgentFactory> {
        &self.agents
    }

    /// Replaces the connector kinds this service knows.
    pub fn set_connector_registry(&self, registry: crate::connectors::ConnectorRegistry) {
        *lock(&self.connectors) = Arc::new(registry);
    }

    pub(crate) fn connector_registry(&self) -> Arc<crate::connectors::ConnectorRegistry> {
        lock(&self.connectors).clone()
    }
    pub fn has_import_sources(&self) -> bool {
        self.source_dirs.claude.is_dir() || self.source_dirs.codex.is_dir()
    }
    pub fn cancel_chatgpt(&self) {
        if let Some(cancel) = lock(&self.chatgpt_cancel).as_ref() {
            cancel.cancel();
        }
    }
    pub fn connect_chatgpt(self: &Arc<Self>) -> Result<(), ServiceError> {
        let auth = self.agents.chatgpt().ok_or_else(|| {
            ServiceError::Import("ChatGPT connection is unavailable in this service.".into())
        })?;
        let cancel = crate::chatgpt::Cancellation::new();
        {
            let mut pending = lock(&self.chatgpt_cancel);
            if pending.is_some() {
                return Err(ServiceError::Busy);
            }
            *pending = Some(cancel.clone());
        }
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let result = auth
                .connect(&|url| service.notify(Notification::OpenUrl { url }), cancel)
                .await;
            *lock(&service.chatgpt_cancel) = None;
            service.notify(Notification::ChatgptLogin {
                ok: result.is_ok(),
                message: result
                    .err()
                    .map_or_else(|| "ChatGPT connected.".into(), |error| error.to_string()),
            });
        });
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.notifications.subscribe()
    }

    pub fn is_running(&self, session_id: i64) -> bool {
        lock(&self.running).contains_key(&session_id)
    }

    pub fn running_sessions(&self) -> Vec<i64> {
        lock(&self.running).keys().copied().collect()
    }

    pub(crate) fn notify(&self, notification: Notification) {
        // No subscribers is normal: the journal and pending list keep everything for later.
        let _ = self.notifications.send(notification);
    }

    /// Claims the sessions for one job, or fails if any of them is in use.
    fn claim(&self, sessions: &[i64]) -> Result<(u64, oneshot::Receiver<()>), ServiceError> {
        let mut running = lock(&self.running);
        if sessions.iter().any(|session| running.contains_key(session)) {
            return Err(ServiceError::Busy);
        }
        let job = self.next_id.fetch_add(1, Ordering::Relaxed);
        for session in sessions {
            running.insert(*session, job);
        }
        let (stop, stopped) = oneshot::channel();
        lock(&self.stops).insert(job, Some(stop));
        drop(running);
        for session in sessions {
            self.notify(Notification::TurnState {
                session_id: *session,
                running: true,
            });
        }
        Ok((job, stopped))
    }

    fn release(&self, job: u64) {
        let mut released = vec![];
        lock(&self.running).retain(|session, owner| {
            let keep = *owner != job;
            if !keep {
                let _ = self.store.clear_workspace_context(*session);
                released.push(*session);
            }
            keep
        });
        lock(&self.stops).remove(&job);
        for session in released {
            self.deny_approvals_for(session);
            self.cancel_questions_for(session);
            self.notify(Notification::TurnState {
                session_id: session,
                running: false,
            });
        }
    }

    fn deny_approvals_for(&self, session_id: i64) {
        let mut approvals = lock(&self.approvals);
        let ids: Vec<String> = approvals
            .values()
            .filter(|pending| pending.request.session_id == session_id)
            .map(|pending| pending.request.id.clone())
            .collect();
        for id in ids {
            if let Some(pending) = approvals.remove(&id) {
                let _ = pending.responder.send(false);
                self.notify(Notification::ApprovalResolved { id });
            }
        }
    }

    fn cancel_questions_for(&self, session_id: i64) {
        let mut questions = lock(&self.questions);
        let ids: Vec<String> = questions
            .values()
            .filter(|pending| pending.request.session_id == session_id)
            .map(|pending| pending.request.id.clone())
            .collect();
        for id in ids {
            if let Some(pending) = questions.remove(&id) {
                let _ = pending.responder.send(None);
                self.notify(Notification::QuestionResolved { id });
            }
        }
    }

    fn append(&self, session_id: i64, event: Event) -> Result<(), StoreError> {
        let seq = self.store.append(session_id, &event)?;
        self.announce(StoredEvent {
            seq,
            session_id,
            event,
        });
        Ok(())
    }

    /// Journals an event the service writes itself. A failed write is reported, since nothing else would show it.
    fn append_or_report(&self, session_id: i64, event: Event) {
        if let Err(error) = self.append(session_id, event) {
            let message = format!("The events couldn't be saved: {error}");
            self.notify(Notification::Problem {
                session_id,
                message,
            });
        }
    }

    fn announce(&self, stored: StoredEvent) {
        if let Event::RateLimit { windows, .. } = &stored.event {
            crate::limits::record_claude(&self.store, windows);
        }
        if matches!(stored.event, Event::ToolResult { .. })
            && let Ok(Some(scope)) = self.store.workspace_turn_context(stored.session_id) {
                self.notify(Notification::WorkspaceProposalsChanged { project_id: scope.project_id, target_id: Some(scope.item_id) });
            }
        // Transient events are exempt: they have no seq, so a client has no way to fetch them later.
        let oversized = !stored.event.is_transient()
            && serde_json::to_vec(&stored.event).is_ok_and(|bytes| bytes.len() > MAX_EVENT_BYTES);
        if oversized {
            self.notify(Notification::EventTooLarge {
                session_id: stored.session_id,
                seq: stored.seq,
            });
        } else {
            self.notify(Notification::Event(stored));
        }
    }

    pub fn start_turn(
        self: &Arc<Self>,
        session_id: i64,
        prompt: String,
        mode: PermissionMode,
    ) -> Result<(), ServiceError> {
        self.start_turn_with_context(session_id, prompt, mode, None)
    }

    /// Starts a turn with the model and effort a person picked for it.
    pub fn start_turn_with_options(
        self: &Arc<Self>,
        session_id: i64,
        prompt: String,
        mode: PermissionMode,
        options: TurnOptions,
    ) -> Result<(), ServiceError> {
        self.start_turn_inner(session_id, prompt, mode, None, options,vec![],None)
    }
    pub fn start_turn_with_files(self:&Arc<Self>,session_id:i64,prompt:String,mode:PermissionMode,options:TurnOptions,files:Vec<vatra_core::TurnFile>,images_supported:bool)->Result<(),ServiceError>{
        self.start_turn_inner(session_id,prompt,mode,None,options,files,Some(images_supported))
    }

    pub(crate) fn start_turn_with_context(
        self: &Arc<Self>, session_id: i64, prompt: String, mode: PermissionMode,
        context: Option<vatra_core::WorkspaceTurnContext>,
    ) -> Result<(), ServiceError> {
        self.start_turn_inner(session_id, prompt, mode, context, TurnOptions::default(),vec![],None)
    }

    #[allow(clippy::too_many_arguments)]
    fn start_turn_inner(
        self: &Arc<Self>, session_id: i64, prompt: String, mode: PermissionMode,
        context: Option<vatra_core::WorkspaceTurnContext>, options: TurnOptions,files:Vec<vatra_core::TurnFile>,
        images_supported: Option<bool>,
    ) -> Result<(), ServiceError> {
        let agent_name = self.session_agent(session_id)?;
        let images_supported = images_supported.unwrap_or_else(|| {
            self.agents().input_capabilities(&agent_name, options.model.as_deref()).images == "supported"
        });
        let (job, stopped) = self.claim(&[session_id])?;
        if let Some(context) = context
            && let Err(error) = self.store.activate_workspace_context(&context) {
                self.release(job);
                return Err(ServiceError::Models(error.to_string()));
        }
        if let Err(error) = self.store.set_app_setting(&format!("session.{session_id}.options"), Some(&serde_json::json!(options).to_string())) {
            self.release(job);
            return Err(ServiceError::Store(error));
        }
        let service = Arc::clone(self);
        tokio::spawn(async move {
            service
                .turn_job(session_id, agent_name, prompt, (mode,options), files, images_supported, stopped)
                .await;
            service.release(job);
        });
        Ok(())
    }

    fn session_agent(&self, session_id: i64) -> Result<String, ServiceError> {
        self.store
            .session_agent(session_id)
            .map_err(|error| match error {
                StoreError::Sqlite(rusqlite_error) if is_no_rows(&rusqlite_error) => {
                    ServiceError::UnknownSession
                }
                other => ServiceError::Store(other),
            })
    }

    #[allow(clippy::too_many_arguments)]
    async fn turn_job(
        self: &Arc<Self>,
        session_id: i64,
        agent_name: String,
        prompt: String,
        settings:(PermissionMode,TurnOptions),
        files:Vec<vatra_core::TurnFile>,
        images_supported:bool,
        stopped: oneshot::Receiver<()>,
    ) {
        let (mode,options)=settings;
        let workspace=self.workspace_directory(session_id);
        let before=if let Some(root)=workspace.clone(){tokio::task::spawn_blocking(move||vatra_engine::artifacts::observe_workspace(&root)).await.ok()}else{None};
        let work = async {
            let agent = match self.build(session_id, &agent_name, mode, options.clone()).await {
                Ok(agent) => agent,
                Err(message) => {
                    match self.store.append_message_with_files(session_id,&prompt,&files){
                        Ok(seq)=>self.announce(StoredEvent{seq,session_id,event:Event::UserMessage{text:prompt.clone()}}),
                        Err(error)=>self.notify(Notification::Problem{session_id,message:error.to_string()}),
                    }
                    self.append_or_report(
                        session_id,
                        Event::TurnFailed {
                            agent: agent_name.clone(),
                            message,
                        },
                    );
                    return;
                }
            };
            let service = Arc::clone(self);
            let mut forward = move |stored: &StoredEvent| {
                service.announce(stored.clone());
            };
            if let Err(RunError::Store(error)) = vatra_core::run_turn_with_files(
                &self.store,
                agent.as_ref(),
                session_id,
                &prompt,
                files,
                images_supported,
                &mut forward,
            )
            .await
            {
                let message = format!("The events couldn't be saved: {error}");
                self.notify(Notification::Problem {
                    session_id,
                    message,
                });
            }
        };
        tokio::select! {
            () = work => {}
            _ = stopped => {
                let message = "You stopped this turn.".to_string();
                self.append_or_report(session_id, Event::TurnFailed { agent: agent_name.clone(), message });
            }
        }
        if let (Some(root),Some(before))=(workspace,before){
            let store=Arc::clone(&self.store);
            let operation=format!("turn:{}",self.store.events(session_id).ok().and_then(|e|e.iter().rev().find(|e|matches!(e.event,Event::UserMessage{..})).map(|e|e.seq)).unwrap_or(0));
            if let Ok(messages)=tokio::task::spawn_blocking(move||{
                let after=vatra_engine::artifacts::observe_workspace(&root);
                vatra_engine::artifacts::reconcile(&store,session_id,&before,&after,&operation)
            }).await{for message in messages{self.append_or_report(session_id,Event::Warning{source:"files".into(),message});}}
        }
    }

    pub(crate) fn workspace_directory(&self,session:i64)->Option<std::path::PathBuf>{
        let project=self.store.session_project(session).ok()?;
        let path=project.and_then(|id|self.store.list_projects().ok()?.into_iter().find(|p|p.id==id)?.path);
        self.agents.working_directory(session,project,path.as_deref().map(std::path::Path::new))
    }

    async fn build(
        self: &Arc<Self>,
        session_id: i64,
        agent_name: &str,
        mode: PermissionMode,
        options: TurnOptions,
    ) -> Result<Arc<dyn vatra_core::Agent>, String> {
        let approver = Arc::new(ServiceApprover::new(self, session_id));
        let questioner = Arc::new(ServiceQuestioner::new(self, session_id));
        let project = self
            .store
            .session_project(session_id)
            .map_err(|error| error.to_string())?;
        let cwd: Option<std::path::PathBuf> = if let Some(proj_id) = project {
            self.store
                .list_projects()
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|candidate| candidate.id == proj_id)
                .and_then(|candidate| candidate.path)
                .map(Into::into)
        } else {
            None
        };
        if let Some(folder) = cwd.as_ref().filter(|folder| !folder.is_dir()) {
            return Err(format!(
                "The project folder doesn't exist: {}",
                folder.display()
            ));
        }
        self.agents
            .build(BuildRequest {
                name: agent_name.into(),
                session_id,
                project_id: project.unwrap_or(0),
                approver,
                questioner,
                mode,
                cwd,
                options,
            })
            .await
    }

    pub fn start_handoff(
        self: &Arc<Self>, from: i64, to: String, task: String, mode: PermissionMode,
    ) -> Result<i64, ServiceError> {
        self.start_handoff_with_options(from,to,task,mode,TurnOptions::default())
    }

    pub fn start_handoff_with_options(
        self: &Arc<Self>,
        from: i64,
        to: String,
        task: String,
        mode: PermissionMode,
        options: TurnOptions,
    ) -> Result<i64, ServiceError> {
        self.session_agent(from)?;
        if self.is_running(from) {
            return Err(ServiceError::Busy);
        }
        let session = create_handoff_session(&self.store, from, &to)?;
        self.store.set_app_setting(&format!("session.{session}.options"),Some(&serde_json::json!(options).to_string()))?;
        let (job, stopped) = self.claim(&[from, session])?;
        let service = Arc::clone(self);
        tokio::spawn(async move {
            service
                .handoff_job(from, session, to, task, (mode,options), stopped)
                .await;
            service.release(job);
        });
        Ok(session)
    }

    async fn handoff_job(
        self: &Arc<Self>,
        from: i64,
        session: i64,
        to: String,
        task: String,
        settings:(PermissionMode,TurnOptions),
        stopped: oneshot::Receiver<()>,
    ) {
        let (mode,options)=settings;
        let work = async {
            let target = match self.build(session, &to, mode, options.clone()).await {
                Ok(target) => target,
                Err(message) => {
                    let text = format!("The handoff failed: {message}");
                    self.append_or_report(
                        session,
                        Event::TurnFailed {
                            agent: to.clone(),
                            message,
                        },
                    );
                    self.append_or_report(
                        from,
                        Event::HandoffResult {
                            to_session: session,
                            text,
                        },
                    );
                    return;
                }
            };
            let summarizer = self.agents.summarizer();
            let service = Arc::clone(self);
            let mut forward = move |stored: &StoredEvent| {
                service.announce(stored.clone());
            };
            let outcome: Result<(), RunError> = run_handoff(
                &self.store,
                from,
                session,
                target.as_ref(),
                summarizer.as_ref(),
                &task,
                &mut forward,
            )
            .await;
            if let Err(RunError::Store(error)) = outcome {
                let message = format!("The events couldn't be saved: {error}");
                self.notify(Notification::Problem {
                    session_id: session,
                    message,
                });
            }
        };
        tokio::select! {
            () = work => {}
            _ = stopped => {
                let message = "You stopped this turn.".to_string();
                let text = format!("The handoff failed: {message}");
                self.append_or_report(session, Event::TurnFailed { agent: to.clone(), message });
                self.append_or_report(from, Event::HandoffResult { to_session: session, text });
            }
        }
    }

    pub fn stop_turn(&self, session_id: i64) {
        let running = lock(&self.running);
        let job = running.get(&session_id).copied();
        if job.is_some() { let _ = self.store.clear_workspace_context(session_id); }
        if let Some(job) = job
            && let Some(slot) = lock(&self.stops).get_mut(&job)
            && let Some(stop) = slot.take()
        {
            let _ = stop.send(());
        }
    }

    pub fn request_approval(
        &self,
        session_id: i64,
        tool: &str,
        arguments: Value,
    ) -> oneshot::Receiver<bool> {
        let (responder, answer) = oneshot::channel();
        // The running lock is held across the check and the insert, so a release can't land between them.
        let running = lock(&self.running);
        if !running.contains_key(&session_id) {
            let _ = responder.send(false);
            return answer;
        }
        let id = format!("aprobare-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let request = ApprovalRequest {
            id: id.clone(),
            session_id,
            tool: tool.into(),
            arguments,
        };
        lock(&self.approvals).insert(
            id,
            PendingApproval {
                request: request.clone(),
                responder,
            },
        );
        self.notify(Notification::Approval(request));
        answer
    }

    pub fn resolve_approval(&self, id: &str, allow: bool) -> Result<(), ServiceError> {
        let pending = lock(&self.approvals)
            .remove(id)
            .ok_or(ServiceError::UnknownApproval)?;
        let _ = pending.responder.send(allow);
        self.notify(Notification::ApprovalResolved { id: id.into() });
        Ok(())
    }

    pub fn pending_approvals(&self) -> Vec<ApprovalRequest> {
        let mut pending: Vec<_> = lock(&self.approvals)
            .values()
            .map(|p| p.request.clone())
            .collect();
        pending.sort_by(|a, b| a.id.cmp(&b.id));
        pending
    }

    /// Asks the user in a running turn. The receiver gets the answer, or None when the turn ends first.
    pub fn request_question(
        &self,
        session_id: i64,
        questions: Value,
    ) -> oneshot::Receiver<Option<Value>> {
        let (responder, answer) = oneshot::channel();
        // The running lock is held across the check and the insert, so a release can't land between them.
        let running = lock(&self.running);
        if !running.contains_key(&session_id) {
            let _ = responder.send(None);
            return answer;
        }
        let id = format!("intrebare-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let request = QuestionRequest {
            id: id.clone(),
            session_id,
            questions,
        };
        lock(&self.questions).insert(
            id,
            PendingQuestion {
                request: request.clone(),
                responder,
            },
        );
        self.notify(Notification::Question(request));
        answer
    }

    /// Records the user's answers, or None when the user gives none.
    pub fn answer_question(&self, id: &str, answers: Option<Value>) -> Result<(), ServiceError> {
        let pending = lock(&self.questions)
            .remove(id)
            .ok_or(ServiceError::UnknownQuestion)?;
        let _ = pending.responder.send(answers);
        self.notify(Notification::QuestionResolved { id: id.into() });
        Ok(())
    }

    pub fn pending_questions(&self) -> Vec<QuestionRequest> {
        let mut pending: Vec<_> = lock(&self.questions)
            .values()
            .map(|p| p.request.clone())
            .collect();
        pending.sort_by(|a, b| a.id.cmp(&b.id));
        pending
    }

    /// The items an import would bring, as the app lists them: each with its verdict and its status in the catalog.
    pub async fn import_scan(&self) -> Result<ScanView, ServiceError> {
        let catalog = self.agents.catalog();
        let store = Arc::clone(&self.store);
        let dirs = self.source_dirs.clone();
        let started = std::time::Instant::now();
        let (scanned, projects) = tokio::task::spawn_blocking(move || {
            let folders =
                crate::import::project_folders(&store, &dirs).map_err(ServiceError::Import)?;
            let (scan, projects) = crate::import::scan_folders(&GuardedFs::new(), &dirs, &folders);
            Ok::<_, ServiceError>((vatra_import::plan(&scan), projects))
        })
        .await
        .map_err(|error| ServiceError::Import(error.to_string()))??;
        let read = started.elapsed();
        let mut probes = probe_items(&SystemProbe::new(), &scanned.items).await;
        // A slow scan on someone's computer is otherwise a mystery: the log says which part took the time.
        eprintln!(
            "import scan: {} folders and {} items read in {:.1}s, programs checked in {:.1}s",
            projects.len(),
            scanned.items.len(),
            read.as_secs_f64(),
            (started.elapsed() - read).as_secs_f64(),
        );
        probes.approved =
            approvals(&scanned, &projects, catalog.as_deref()).map_err(ServiceError::Import)?;
        scan_view(&scanned, &projects, catalog.as_deref(), &probes).map_err(ServiceError::Import)
    }

    /// Applies an import choice: copies what it picks into the catalog, and reports each item's progress as it goes.
    /// The projects of the choice are the ones this service reads, so the caller does not set them.
    pub async fn import_apply(
        &self,
        mut choice: ImportChoice,
    ) -> Result<ApplyOutcome, ServiceError> {
        let catalog = self
            .agents
            .catalog()
            .ok_or_else(|| ServiceError::Import("the extension catalog isn't available".into()))?;
        let secrets = self.agents.secrets();
        let store = Arc::clone(&self.store);
        let notifications = self.notifications.clone();
        let import_lock = Arc::clone(&self.import_lock);
        let cancelled = Arc::clone(&self.import_cancelled);
        let dirs = self.source_dirs.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            let _one_at_a_time = import_lock.lock().unwrap_or_else(PoisonError::into_inner);
            cancelled.store(false, Ordering::Relaxed);
            let folders =
                crate::import::project_folders(&store, &dirs).map_err(ServiceError::Import)?;
            let (scan, projects) = crate::import::scan_folders(&GuardedFs::new(), &dirs, &folders);
            choice.projects = projects;
            let mut progress = |done: usize, total: usize, label: &str| {
                let _ = notifications.send(Notification::ImportProgress {
                    done,
                    total,
                    label: label.to_string(),
                });
            };
            let mut outcome =
                apply_reporting(&scan, &choice, &catalog, secrets.as_ref(), &mut progress);
            if choice.include_history || !choice.history_projects.is_empty() {
                let logs: Vec<_> = if choice.include_history {
                    vatra_import::history_files(&dirs)
                } else {
                    // A chosen folder comes as a project of its own when Vatra has none for it yet.
                    let chosen = vatra_import::history_files_for(&dirs, &choice.history_projects);
                    let mut known = store.list_projects()?;
                    for cwd in chosen.iter().filter_map(|(_, _, cwd)| cwd.as_deref()) {
                        if vatra_import::map_project(cwd, &known).is_none() {
                            let name = free_project_name(cwd, &known);
                            let id = store.create_project(&name)?;
                            store.set_project_path(id, Some(cwd))?;
                            known = store.list_projects()?;
                        }
                    }
                    chosen.into_iter().map(|(path, origin, _)| (path, origin)).collect()
                };
                let projects = store.list_projects()?;
                for (done, (path, origin)) in logs.iter().enumerate() {
                    progress(done, logs.len(), "Indexing past conversations");
                    let result = vatra_import::import_log(
                        path,
                        origin,
                        &store,
                        &projects,
                        &cancelled,
                        &mut |_, _| {},
                    );
                    outcome.results.push(vatra_import::ItemResult {
                        key: format!("history:{}", path.display()),
                        name: "Past conversation".into(),
                        project: None,
                        state: match result {
                            Ok(_) => ItemState::Added,
                            Err(message) => ItemState::Failed { message },
                        },
                    });
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }
                }
                progress(logs.len(), logs.len(), "");
            }
            Ok::<_, ServiceError>(outcome)
        })
        .await
        .map_err(|error| ServiceError::Import(error.to_string()))??;
        // Only the hubs the written items reach are dropped: a global item reaches every project.
        let written = outcome
            .results
            .iter()
            .filter(|result| matches!(result.state, ItemState::Added | ItemState::Changed));
        let mut projects = std::collections::BTreeSet::new();
        for result in written {
            match result.project {
                Some(project) => {
                    projects.insert(project);
                }
                None => {
                    self.agents.catalog_changed(None);
                    projects.clear();
                    break;
                }
            }
        }
        for project in projects {
            self.agents.catalog_changed(Some(project));
        }
        *lock(&self.last_import) = Some(outcome.clone());
        Ok(outcome)
    }

    /// The folders that have past conversations in Claude Code or Codex, for the import to offer.
    pub async fn import_history_projects(
        &self,
    ) -> Result<Vec<vatra_import::HistoryProject>, ServiceError> {
        let store = Arc::clone(&self.store);
        let dirs = self.source_dirs.clone();
        tokio::task::spawn_blocking(move || {
            let started = std::time::Instant::now();
            let projects = store.list_projects()?;
            let found = vatra_import::history_projects(&dirs, &projects);
            let conversations: usize = found.iter().map(|project| project.conversations).sum();
            eprintln!(
                "import history: {conversations} conversations in {} folders read in {:.1}s",
                found.len(),
                started.elapsed().as_secs_f64(),
            );
            Ok::<_, ServiceError>(found)
        })
        .await
        .map_err(|error| ServiceError::Import(error.to_string()))?
    }

    /// What the last import this service applied did, if it applied one.
    pub fn import_report(&self) -> Option<ApplyOutcome> {
        lock(&self.last_import).clone()
    }

    pub fn cancel_import(&self) {
        self.import_cancelled.store(true, Ordering::Relaxed);
    }

    pub async fn mcp_status(&self) -> Vec<McpServerStatus> {
        let secrets = self.agents.secrets();
        let mut status = Vec::new();
        for server in self.agents.mcp_servers() {
            let logged_in = match (&server.url, &server.oauth) {
                (Some(url), Some(_)) => has_credentials(url, Arc::clone(&secrets))
                    .await
                    .unwrap_or(false),
                _ => false,
            };
            status.push(McpServerStatus {
                name: server.name,
                http: server.url.is_some(),
                oauth: server.oauth.is_some(),
                logged_in,
            });
        }
        status
    }

    /// Starts the OAuth login of a server in the background. Its address goes to the app to open, and its result
    /// comes as a notification.
    pub fn start_mcp_login(self: &Arc<Self>, name: &str) -> Result<(), ServiceError> {
        let server = self
            .agents
            .mcp_servers()
            .into_iter()
            .find(|server| server.name == name)
            .ok_or(ServiceError::UnknownMcpServer)?;
        self.start_mcp_login_config(server)
    }

    pub(crate) fn start_mcp_login_config(self:&Arc<Self>, server:vatra_core::McpServerConfig)->Result<(),ServiceError>{
        if server.oauth.is_none() || server.url.is_none() {
            return Err(ServiceError::NoOAuth);
        }
        let secrets = self.agents.secrets();
        let service = Arc::clone(self);
        let name = server.name.clone();
        tokio::spawn(async move {
            let open = |url: &str| {
                service.notify(Notification::OpenUrl {
                    url: url.to_string(),
                })
            };
            let (ok, message) = match login(&server, secrets, &open).await {
                Ok(()) => (true, "The login is done.".to_string()),
                Err(error) => (false, error.to_string()),
            };
            service.notify(Notification::McpLogin {
                server: name,
                ok,
                message,
            });
        });
        Ok(())
    }
}

/// The name of a new project for a folder: the folder's own name, with a number when another folder already took it.
fn free_project_name(cwd: &str, known: &[vatra_core::Project]) -> String {
    let base = cwd
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("Imported")
        .to_string();
    let mut name = base.clone();
    let mut number = 1;
    while known.iter().any(|project| project.name == name) {
        number += 1;
        name = format!("{base} ({number})");
    }
    name
}

fn is_no_rows(error: &vatra_core::SqliteError) -> bool {
    matches!(error, vatra_core::SqliteError::QueryReturnedNoRows)
}
