use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use vatra_extensions::{Catalog, CatalogEntry, CatalogError, ElementKind, Scope};
use vatra_import::ImportChoice;

use crate::bridge::{internal_router, token_matches};
use crate::completion::{self, Trigger};
use crate::service::{Notification, PermissionMode, Service, ServiceError};

const PROTOCOL_PREFIX: &str = "vatra.bearer.";
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const ALLOWED_ORIGINS: [&str; 2] = ["tauri://localhost", "http://tauri.localhost"];
/// The Vite dev server. Only a debug build of the service lets it in.
const DEV_ORIGIN: &str = "http://localhost:1420";

/// Whether a page from `origin` may open the RPC connection.
pub fn origin_allowed(origin: &str, debug_build: bool) -> bool {
    ALLOWED_ORIGINS.contains(&origin) || (debug_build && origin == DEV_ORIGIN)
}

const PARSE_ERROR: i64 = -32700;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = 1000;
const BUSY: i64 = 1001;
const UNKNOWN_SESSION: i64 = 1002;
const UNKNOWN_APPROVAL: i64 = 1003;
const UNKNOWN_QUESTION: i64 = 1004;

#[derive(Clone)]
struct RpcState {
    service: Arc<Service>,
    token: Arc<str>,
}

pub fn router(service: Arc<Service>, token: Arc<str>) -> axum::Router {
    let state = RpcState {
        service: service.clone(),
        token: token.clone(),
    };
    axum::Router::new()
        .route("/rpc", get(upgrade))
        .route("/health", get(health))
        .with_state(state)
        .merge(internal_router(service, token))
}

async fn health() -> axum::Json<Value> {
    axum::Json(json!({ "service": "vatra", "pid": std::process::id() }))
}

async fn upgrade(
    State(state): State<RpcState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let offered = headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let protocol = offered.split(',').map(str::trim).find(|protocol| {
        protocol
            .strip_prefix(PROTOCOL_PREFIX)
            .is_some_and(|token| token_matches(&state.token, token))
    });
    let Some(protocol) = protocol.map(String::from) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    // A page in the user's browser can reach loopback, so the token alone isn't the only check.
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    if origin.is_some_and(|origin| !origin_allowed(origin, cfg!(debug_assertions))) {
        return StatusCode::FORBIDDEN.into_response();
    }
    upgrade
        .protocols([protocol])
        .max_message_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| serve_connection(socket, state.service))
}

/// Whether a request may run for long. It answers when it is done, so the connection keeps reading requests such as
/// `import.cancel` and keeps delivering notifications. All others are answered in the order they arrive.
fn runs_long(text: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok_and(|request| {
        matches!(
            request["method"].as_str(),
            Some(
                "import.scan"
                    | "import.apply"
                    | "import.history"
                    | "agents.options"
                    | "chatgpt.status"
                    | "chatgpt.logout"
                    | "hardware.get"
                    | "models.search"
                    | "models.files"
                    | "models.suggest"
                    | "models.download"
                    | "models.add_path"
                    | "models.start"
                    | "models.check"
                    | "models.labels"
                    | "models.installed"
                    | "models.stop"
                    | "models.remove"
                    | "agents.provider_models"
                    | "agents.detect"
                    | "registry.acp"
                    | "registry.mcp"
                    | "registry.acp_install"
                    | "registry.mcp_install"
                    | "skills.scan" | "skills.set_sources" | "skills.inspect" | "skills.install" | "skills.uninstall"
                    | "mcp.preview" | "mcp.install" | "mcp.remove"
                    | "mcp.test" | "mcp.connect" | "mcp.disconnect" | "mcp.reconnect" | "mcp.connection" | "mcp.form" | "mcp.set_tools" | "mcp.login_installed"
                    | "mcp.registry_config"
                    | "deps.status"
                    | "deps.install"
                    | "parity.run"
                    | "parity.native"
            )
        )
    })
}

async fn serve_connection(socket: WebSocket, service: Arc<Service>) {
    let mut updates = service.subscribe();
    let (mut sender, mut receiver) = socket.split();
    let (finished, mut answers) = tokio::sync::mpsc::unbounded_channel::<Value>();
    loop {
        tokio::select! {
            message = receiver.next() => {
                let Some(Ok(message)) = message else { return };
                let text = match message {
                    Message::Text(text) => text,
                    Message::Close(_) => return,
                    _ => continue,
                };
                if runs_long(text.as_str()) {
                    let service = Arc::clone(&service);
                    let finished = finished.clone();
                    tokio::spawn(async move {
                        let _ = finished.send(answer_request(&service, text.as_str()).await);
                    });
                    continue;
                }
                let answer = answer_request(&service, text.as_str()).await;
                if sender.send(Message::Text(answer.to_string().into())).await.is_err() {
                    return;
                }
            }
            Some(answer) = answers.recv() => {
                if sender.send(Message::Text(answer.to_string().into())).await.is_err() {
                    return;
                }
            }
            update = updates.recv() => {
                let update = match update {
                    Ok(update) => update,
                    // The client lost events. Closing makes it reconnect and catch up by seq.
                    Err(RecvError::Lagged(_)) | Err(RecvError::Closed) => return,
                };
                let frame = notification_frame(&update).to_string();
                if sender.send(Message::Text(frame.into())).await.is_err() {
                    return;
                }
            }
        }
    }
}

fn notification_frame(update: &Notification) -> Value {
    let (method, params) = match update {
        Notification::RemindersChanged { project_id } => ("reminders_changed",json!({"project_id":project_id})),
        Notification::ScheduleChanged => ("schedule_changed",json!({})),
        Notification::CalendarChanged => ("calendar_changed",json!({})),
        Notification::ResearchChanged { id } => ("research_changed",json!({"id":id})),
        Notification::WorkspaceChanged { project_id, item_ids } => ("workspace_changed", json!({"project_id":project_id,"item_ids":item_ids})),
        Notification::WorkspaceProposalsChanged { project_id, target_id } => ("workspace_proposals_changed", json!({"project_id":project_id,"target_id":target_id})),
        Notification::ChatgptLogin { ok, message } => {
            ("chatgpt_login", json!({"ok":ok,"message":message}))
        }
        Notification::Event(event) => ("event", json!(event)),
        Notification::EventTooLarge { session_id, seq } => (
            "event_too_large",
            json!({ "session_id": session_id, "seq": seq }),
        ),
        Notification::TurnState {
            session_id,
            running,
        } => (
            "turn_state",
            json!({ "session_id": session_id, "running": running }),
        ),
        Notification::Approval(request) => ("approval", json!(request)),
        Notification::ApprovalResolved { id } => ("approval_resolved", json!({ "id": id })),
        Notification::Question(request) => ("question", json!(request)),
        Notification::QuestionResolved { id } => ("question_resolved", json!({ "id": id })),
        Notification::Problem {
            session_id,
            message,
        } => (
            "problem",
            json!({ "session_id": session_id, "message": message }),
        ),
        Notification::McpLogin {
            server,
            ok,
            message,
        } => (
            "mcp_login",
            json!({ "server": server, "ok": ok, "message": message }),
        ),
        Notification::OpenUrl { url } => ("open_url", json!({ "url": url })),
        Notification::ImportProgress { done, total, label } => (
            "import_progress",
            json!({ "done": done, "total": total, "label": label }),
        ),
        Notification::Models { method, params } => (*method, params.clone()),
    };
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

async fn answer_request(service: &Arc<Service>, text: &str) -> Value {
    let Ok(request) = serde_json::from_str::<Value>(text) else {
        return error_frame(Value::Null, PARSE_ERROR, "the message isn't valid JSON");
    };
    let id = request["id"].clone();
    let Some(method) = request["method"].as_str() else {
        return error_frame(id, INVALID_PARAMS, "method is missing");
    };
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    if method.starts_with("files.") {
        return match crate::files::handle(service,method,params).await {
            Ok(result)=>json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(message)=>error_frame(id,INVALID_PARAMS,&message),
        };
    }
    if matches!(method,"workspace.ai.status"|"workspace.ai.state") {
        return match crate::workspace::ai::read(service,method,params) {Ok(result)=>json!({"jsonrpc":"2.0","id":id,"result":result}),Err(error)=>error.frame(id)};
    }
    if method.starts_with("reminders.") {
        return match crate::reminders::rpc::handle(service,method,params) {
            Ok(result)=>json!({"jsonrpc":"2.0","id":id,"result":result}), Err(error)=>error.frame(id),
        };
    }
    if method == "workspace.ai.start" {
        let result = serde_json::from_value(params).map_err(|error| crate::workspace::rpc::RpcError::invalid(error.to_string()))
            .and_then(|request| crate::workspace::ai::start_workspace_turn(service, request));
        return match result {
            Ok(()) => json!({"jsonrpc":"2.0","id":id,"result":{}}),
            Err(error) => error.frame(id),
        };
    }
    if crate::workspace::rpc::handles(method) {
        return match crate::workspace::rpc::handle(service, method, params) {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(error) => error.frame(id),
        };
    }
    match dispatch(service, method, params).await {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => error_frame(id, code, &message),
    }
}

fn error_frame(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

type RpcResult = Result<Value, (i64, String)>;

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, (i64, String)> {
    serde_json::from_value(params)
        .map_err(|error| (INVALID_PARAMS, format!("invalid parameters: {error}")))
}

fn service_error(error: ServiceError) -> (i64, String) {
    let code = match error {
        ServiceError::Busy => BUSY,
        ServiceError::UnknownSession => UNKNOWN_SESSION,
        ServiceError::UnknownApproval => UNKNOWN_APPROVAL,
        ServiceError::UnknownQuestion => UNKNOWN_QUESTION,
        ServiceError::UnknownMcpServer | ServiceError::NoOAuth => INVALID_PARAMS,
        ServiceError::Store(_) | ServiceError::Import(_) | ServiceError::Models(_) => INTERNAL_ERROR,
    };
    (code, error.to_string())
}

/// Files written at a caller's path must name the full path, never one relative to the service's folder.
fn absolute_path(text: &str) -> Result<std::path::PathBuf, (i64, String)> {
    let path = std::path::PathBuf::from(text);
    if !path.is_absolute() {
        return Err((INVALID_PARAMS, format!("the path must be absolute: {text}")));
    }
    Ok(path)
}

fn internal(error: impl std::fmt::Display) -> (i64, String) {
    (INTERNAL_ERROR, error.to_string())
}

/// A refusal the caller caused, such as a file that already exists, is invalid params. Any other failure is ours.
fn store_error(error: vatra_core::StoreError) -> (i64, String) {
    match error {
        vatra_core::StoreError::Invalid(_) => (INVALID_PARAMS, error.to_string()),
        other => internal(other),
    }
}

fn catalog(service: &Arc<Service>) -> Result<Arc<Catalog>, (i64, String)> {
    service
        .agents()
        .catalog()
        .ok_or_else(|| internal("the extension catalog isn't available"))
}

fn catalog_error(error: CatalogError) -> (i64, String) {
    match error {
        CatalogError::UnknownEntry(_) | CatalogError::Invalid(_) => {
            (INVALID_PARAMS, error.to_string())
        }
        other => internal(other),
    }
}

/// Drops the hubs that a change to one catalog entry reaches: its project's, or every project's when it is global.
fn forget_hubs(service: &Arc<Service>, id: i64) {
    let scope = catalog(service)
        .ok()
        .and_then(|catalog| catalog.list().ok())
        .and_then(|entries| entries.into_iter().find(|entry| entry.id == id))
        .map(|entry| entry.scope);
    service.agents().catalog_changed(match scope {
        Some(Scope::Project(project)) => Some(project),
        _ => None,
    });
}

/// An entry as clients see it. An MCP server's environment and header values stay on the service, and only their
/// names are listed. A query string in its URL, and the value after a secret-looking flag in any command, are replaced
/// too.
fn public_entry(entry: &CatalogEntry) -> Value {
    let mut value = json!(entry);
    value["routes"] = json!(
        [
            vatra_extensions::Route::Engine,
            vatra_extensions::Route::Claude,
            vatra_extensions::Route::Acp
        ]
        .map(|route| (route, vatra_extensions::verdict(entry, route)))
    );
    let mut secrets = Vec::new();
    if entry.kind == ElementKind::McpServer {
        for map in ["env", "headers"] {
            for (key, content) in entry
                .details
                .get(map)
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                if content.as_str().is_some_and(|text| text.contains("${"))
                    && entry
                        .details
                        .get("secrets")
                        .and_then(|s| s.get(key))
                        .is_none()
                {
                    secrets.push(json!({"server":entry.name,"key":key}));
                }
            }
        }
    }
    value["secrets"] = json!(secrets);
    if entry.kind == ElementKind::McpServer {
        for key in ["env", "headers"] {
            if let Some(map) = entry.details.get(key).and_then(Value::as_object) {
                let names: Vec<&String> = map.keys().collect();
                value["details"][key] = json!(names);
            }
        }
        if let Some(url) = entry.details.get("url").and_then(Value::as_str) {
            value["details"]["url"] = json!(redact_url(url));
        }
        if let Some(args) = entry.details.get("args").and_then(Value::as_array) {
            let args: Vec<&str> = args.iter().filter_map(Value::as_str).collect();
            value["details"]["args"] = json!(redact_args(&args));
        }
    }
    if let Some(runs) = value["runs_code"].as_array_mut() {
        for run in runs {
            if let Some(command) = run["command"].as_str().map(str::to_string) {
                let words: Vec<&str> = command.split_whitespace().collect();
                run["command"] = json!(redact_args(&words).join(" "));
            }
        }
    }
    value
}

/// The URL without its query string or fragment, which is where tokens are usually written.
fn redact_url(url: &str) -> String {
    match url.find(['?', '#']) {
        Some(cut) => format!("{}?<redacted>", &url[..cut]),
        None => url.to_string(),
    }
}

/// Arguments with the values of secret-looking flags, as `--api-key VALUE` or `--token=VALUE`, replaced.
fn redact_args(args: &[&str]) -> Vec<String> {
    let mut redacted = Vec::new();
    let mut hide_next = false;
    for arg in args {
        if std::mem::take(&mut hide_next) {
            redacted.push("<redacted>".to_string());
            continue;
        }
        if let Some((flag, _)) = arg.split_once('=').filter(|(flag, _)| sensitive_flag(flag)) {
            redacted.push(format!("{flag}=<redacted>"));
        } else {
            hide_next = sensitive_flag(arg);
            redacted.push(arg.to_string());
        }
    }
    redacted
}

fn sensitive_flag(flag: &str) -> bool {
    let lower = flag.to_ascii_lowercase();
    flag.starts_with('-')
        && ["key", "token", "secret", "password"]
            .iter()
            .any(|word| lower.contains(word))
}

#[derive(Deserialize)]
struct CreateProject {
    name: String,
    path: Option<String>,
}

#[derive(Deserialize)]
struct CreateSession {
    #[serde(default)]
    project_id: Option<i64>,
    agent: String,
}

#[derive(Deserialize)]
struct SessionEvents {
    session_id: i64,
    after_seq: i64,
}

#[derive(Deserialize)]
struct StartTurn {
    session_id: i64,
    prompt: String,
    mode: PermissionMode,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    attachments:Vec<crate::files::FileSelection>,
}

#[derive(Deserialize)]
struct StopTurn {
    session_id: i64,
}

#[derive(Deserialize)]
struct ResolveApproval {
    id: String,
    allow: bool,
}

#[derive(Deserialize)]
struct AnswerQuestion {
    id: String,
    answers: Option<Value>,
}

#[derive(Deserialize)]
struct McpLogin {
    server: String,
}

#[derive(Deserialize)]
struct SetEnabled {
    id: i64,
    enabled: bool,
}

#[derive(Deserialize)]
struct Approve {
    id: i64,
    expected_hash: String,
}

#[derive(Deserialize)]
struct SetSecret {
    id: i64,
    key: String,
    value: String,
}

#[derive(Deserialize)]
struct CompleteInput {
    project_id: i64,
    trigger: Trigger,
    query: String,
}

#[derive(Deserialize)]
struct StartHandoff {
    model: Option<String>,
    effort: Option<String>,
    from: i64,
    to: String,
    task: String,
    mode: PermissionMode,
}

async fn dispatch(service: &Arc<Service>, method: &str, params: Value) -> RpcResult {
    let store = service.store();
    match method {
        "projects.list" => Ok(json!(store.list_projects().map_err(internal)?)),
        "projects.delete" => {
            #[derive(Deserialize)] struct Delete { id: i64 }
            let request: Delete = parse(params)?;
            // A running turn belongs to its conversation; wait for it to end before the conversation goes.
            if store.project_sessions(request.id).map_err(internal)?.into_iter().any(|session| service.is_running(session)) {
                return Err((BUSY, "A conversation in this project is still running. Stop it first.".into()));
            }
            let removal = store.delete_project(request.id).map_err(internal)?.ok_or((INVALID_PARAMS, "This project doesn't exist.".to_string()))?;
            Ok(json!(removal))
        }
        "projects.rename" => {
            #[derive(Deserialize)] struct Rename { id: i64, name: String }
            let request: Rename = parse(params)?;
            store.rename_project(request.id, &request.name).map_err(|e| (INVALID_PARAMS, e.to_string()))?;
            Ok(json!({}))
        }
        "projects.create" => {
            let request: CreateProject = parse(params)?;
            if let Some(path) = request.path.as_deref().filter(|path| !std::path::Path::new(path).is_dir()) {
                return Err((INVALID_PARAMS, format!("This folder doesn't exist: {path}")));
            }
            let id = store.create_project(&request.name).map_err(internal)?;
            if request.path.is_some() {
                store.set_project_path(id, request.path.as_deref()).map_err(internal)?;
            }
            let projects = store.list_projects().map_err(internal)?;
            let project = projects.into_iter().find(|project| project.id == id);
            Ok(json!(project))
        }
        "sessions.list" => {
            let sessions = store.list_sessions().map_err(internal)?;
            let running = service.running_sessions();
            let sessions: Vec<Value> = sessions
                .into_iter()
                .map(|session| {
                    let is_running = running.contains(&session.id);
                    let mut value = json!(session);
                    value["running"] = json!(is_running);
                    value["options"] = store.app_setting(&format!("session.{}.options",session.id)).ok().flatten()
                        .and_then(|text| serde_json::from_str::<Value>(&text).ok()).unwrap_or(json!({}));
                    value["extension_route"] = json!(service.agents().extension_route(&session.agent));
                    value
                })
                .collect();
            Ok(json!(sessions))
        }
        "conversations.ui.get"=>{
            let state=store.app_setting("conversations.ui").map_err(store_error)?.and_then(|text|serde_json::from_str::<Value>(&text).ok()).unwrap_or(json!({}));
            Ok(state)
        }
        "conversations.ui.set"=>{
            let text=serde_json::to_string(&params).map_err(internal)?;
            if text.len()>2*1024*1024||!params.is_object(){return Err((INVALID_PARAMS,"Conversation drafts exceed the 2 MiB limit.".into()));}
            store.set_app_setting("conversations.ui",Some(&text)).map_err(store_error)?;Ok(json!({}))
        }
        "sessions.branch"=>{
            #[derive(Deserialize)]struct Branch{session_id:i64,through_seq:i64,#[serde(default)]draft_from_seq:Option<i64>}
            let request:Branch=parse(params)?;
            if service.is_running(request.session_id){return Err((BUSY,"Stop the current turn before branching.".into()));}
            let id=store.branch_session(request.session_id,request.through_seq).map_err(store_error)?;
            let mut attachments=Vec::new();
            if let Some(seq)=request.draft_from_seq{
                for (file,revision) in store.message_files(request.session_id,seq).map_err(store_error)?{
                    let original=store.file(request.session_id,file).map_err(store_error)?;
                    let text=store.message_file_text(request.session_id,seq,file).map_err(store_error)?;
                    let mut bytes=Vec::new();for offset in (0..original.size).step_by(vatra_core::FILE_CHUNK_BYTES){bytes.extend(store.file_bytes(request.session_id,file,Some(revision),offset,vatra_core::FILE_CHUNK_BYTES).map_err(store_error)?);}
                    let copy=store.import_file(id,&vatra_core::NewFile{name:original.name,media_type:original.media_type,origin:"attached".into(),source_path:None,operation:Some(format!("branch:{seq}")),text},&bytes).map_err(store_error)?;
                    attachments.push(json!({"id":copy.id}));
                }
            }
            if let Some(options)=store.app_setting(&format!("session.{}.options",request.session_id)).map_err(store_error)?{store.set_app_setting(&format!("session.{id}.options"),Some(&options)).map_err(store_error)?;}
            Ok(json!({"id":id,"attachments":attachments}))
        }
        "sessions.rename"=>{
            #[derive(Deserialize)]struct Rename{session_id:i64,title:String}
            let request:Rename=parse(params)?;store.rename_session(request.session_id,&request.title).map_err(store_error)?;Ok(json!({}))
        }
        "sessions.options" => {
            #[derive(Deserialize)] struct Options { session_id: i64, model: Option<String>, effort: Option<String> }
            let request: Options = parse(params)?;
            store.session_agent(request.session_id).map_err(internal)?;
            if service.is_running(request.session_id) { return Err((1001,"Wait until this turn finishes before changing the model".into())); }
            let options = crate::TurnOptions::checked(request.model, request.effort).map_err(|message| (INVALID_PARAMS,message))?;
            store.set_app_setting(&format!("session.{}.options",request.session_id),Some(&json!(options).to_string())).map_err(internal)?;
            Ok(json!({}))
        }
        "sessions.create" => {
            let request: CreateSession = parse(params)?;
            if !service.agents().names().contains(&request.agent) {
                return Err((INVALID_PARAMS, format!("unknown agent `{}`", request.agent)));
            }
            if let Some(pid) = request.project_id {
                let known = store.list_projects().map_err(internal)?.iter().any(|p| p.id == pid);
                if !known {
                    return Err((INVALID_PARAMS, format!("unknown project `{}`", pid)));
                }
            }
            let id = store.create_session(request.project_id, &request.agent).map_err(internal)?;
            Ok(json!({ "id": id,"extension_route":service.agents().extension_route(&request.agent) }))
        }
        "agents.options" => {
            #[derive(Deserialize)] struct Options { name: String, #[serde(default)] project_id: i64, model: Option<String> }
            let request: Options = parse(params)?;
            if !service.agents().names().contains(&request.name) {
                return Err((INVALID_PARAMS, format!("unknown agent `{}`", request.name)));
            }
            let found = service.agents().agent_options(&request.name, request.project_id, request.model.as_deref()).await.map_err(|message| (INVALID_PARAMS, message))?;
            Ok(json!(found))
        }
        "sessions.delete" => {
            #[derive(Deserialize)] struct Delete { session_id: i64 }
            let request: Delete = parse(params)?;
            if service.running_sessions().contains(&request.session_id) {
                return Err((BUSY, "This conversation is still working. Stop it first.".into()));
            }
            if !store.delete_session(request.session_id).map_err(store_error)? {
                return Err((UNKNOWN_SESSION, "Unknown session".into()));
            }
            Ok(json!({}))
        }
        "sessions.events" => {
            let request: SessionEvents = parse(params)?;
            Ok(json!(store.events_after(request.session_id, request.after_seq).map_err(internal)?))
        }
        "agents.list" => Ok(json!(service.agents().names())),
        "chatgpt.status"=>{
            let auth=service.agents().chatgpt().ok_or_else(||internal("ChatGPT connection is unavailable"))?;
            Ok(json!(auth.status().await.map_err(internal)?))
        }
        "chatgpt.accounts" => {
            let auth=service.agents().chatgpt().ok_or_else(||internal("ChatGPT connection is unavailable"))?;
            Ok(json!(auth.accounts().await.map_err(internal)?))
        }
        "chatgpt.select" => {
            let id=params["id"].as_str().ok_or_else(||internal("account id is required"))?;
            let auth=service.agents().chatgpt().ok_or_else(||internal("ChatGPT connection is unavailable"))?;
            auth.select(id).await.map_err(internal)?;
            Ok(json!({}))
        }
        "chatgpt.login"=>{service.connect_chatgpt().map_err(service_error)?;Ok(json!({}))}
        "chatgpt.cancel"=>{service.cancel_chatgpt();Ok(json!({}))}
        "chatgpt.logout"=>{
            service.cancel_chatgpt();
            let auth=service.agents().chatgpt().ok_or_else(||internal("ChatGPT connection is unavailable"))?;
            Ok(json!({"revoked":auth.disconnect().await.map_err(internal)?}))
        }
        "agents.describe" => Ok(json!(service.agents().names().iter().map(|name|json!({"name":name,"route":service.agents().extension_route(name),"subscription":service.agents().subscription(name)})).collect::<Vec<_>>())),
        "mcp.status" => Ok(json!(service.mcp_status().await)),
        "mcp.login" => {
            let request: McpLogin = parse(params)?;
            service.start_mcp_login(&request.server).map_err(service_error)?;
            Ok(json!({}))
        }
        "catalog.list" => {
            let entries = catalog(service)?.list().map_err(catalog_error)?;
            Ok(json!(entries.iter().map(public_entry).collect::<Vec<Value>>()))
        }
        "catalog.set_enabled" => {
            let request: SetEnabled = parse(params)?;
            catalog(service)?.set_enabled(request.id, request.enabled).map_err(catalog_error)?;
            forget_hubs(service, request.id);
            Ok(json!({}))
        }
        "catalog.approve" => {
            let request: Approve = parse(params)?;
            catalog(service)?
                .approve(request.id, &request.expected_hash)
                .map_err(catalog_error)?;
            forget_hubs(service, request.id);
            Ok(json!({}))
        }
        "catalog.set_secret" => {
            let request: SetSecret = parse(params)?;
            let secrets = service.agents().secrets();
            catalog(service)?
                .set_secret(request.id, &request.key, &request.value, secrets.as_ref())
                .map_err(catalog_error)?;
            forget_hubs(service, request.id);
            Ok(json!({}))
        }
        "import.scan" => Ok(json!(service.import_scan().await.map_err(service_error)?)),
        "import.apply" => {
            let choice: ImportChoice = parse(params)?;
            Ok(json!(service.import_apply(choice).await.map_err(service_error)?))
        }
        "import.report" => Ok(json!(service.import_report())),
        "import.history" => Ok(json!(service.import_history_projects().await.map_err(service_error)?)),
        "market.list" => crate::market::list(service).map_err(internal),
        "registry.acp" | "registry.mcp" => {
            let request = parse(params)?;
            let ecosystem = if method == "registry.acp" { "acp" } else { "mcp" };
            Ok(json!(crate::registry::discover(ecosystem, request).await.map_err(internal)?))
        }
        "registry.acp_install" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct AcpInstall { id: String, version: String, alias: String }
            let request: AcpInstall = parse(params)?;
            Ok(json!(crate::registry::acp_install(service, &request.id, &request.version, &request.alias).await.map_err(internal)?))
        }
        "registry.mcp_install" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct McpInstall { name: String, version: String, alias: String, method: String, index: usize }
            let request: McpInstall = parse(params)?;
            Ok(json!(crate::registry::mcp_install(service, &request.name, &request.version, &request.alias, &request.method, request.index).await.map_err(internal)?))
        }
        "market.install" => crate::market::install(service, params),
        "mcp.registry_config"=>{
            #[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
            struct RegistryConfig{name:String,version:String,alias:String,method:String,index:usize}
            let request:RegistryConfig=parse(params)?;
            let server=crate::registry::mcp_server(&request.name,&request.version).await.map_err(internal)?;
            let document=crate::mcp_manager::registry_document(&server,&request.alias,&request.method,request.index).map_err(internal)?;
            Ok(json!({"document":serde_json::to_string_pretty(&document).map_err(internal)?,"source":server}))
        },
        "mcp.test"|"mcp.connect"|"mcp.disconnect"|"mcp.reconnect"|"mcp.connection"|"mcp.login_installed"=>crate::mcp_manager::runtime(service,method,params).await,
        "mcp.preview"|"mcp.install"|"mcp.remove"|"mcp.form"|"mcp.set_tools"|"mcp.sources"=>{
            let service=Arc::clone(service);let method=method.to_string();
            tokio::task::spawn_blocking(move||crate::mcp_manager::configure(&service,&method,params)).await.map_err(internal)?
        },
        "skills.sources"|"skills.scan"|"skills.set_sources"|"skills.inspect"|"skills.install"|"skills.uninstall"=>{
            let service=Arc::clone(service); let method=method.to_string();
            tokio::task::spawn_blocking(move||crate::skills::handle(&service,&method,params)).await.map_err(internal)?
        },
        "agents.providers" | "copilot.start" | "copilot.poll" | "copilot.models" | "agents.provider_models" | "agents.detect" | "agents.configured" | "agents.save"
        | "agents.remove" | "agents.summarizer" | "agents.form" => crate::setup::handle(service, method, params).await,
        method if method.starts_with("semantic.") => crate::semantic::handle(service, method, params).await,
        method if method.starts_with("health.") => crate::health::handle(service, method, params).await,
        method if crate::oauth::handles(method) => crate::oauth::handle(service, method, params).await,
        method if crate::research::handles(method) => crate::research::handle(service, method, params).await,
        method if crate::email::handles(method) => crate::email::handle(service, method, params).await,
        method if crate::scheduler::handles(method) => crate::scheduler::handle(service, method, params).await,
        "connectors.kinds" | "connectors.list" | "connectors.save" | "connectors.remove" | "connectors.check" => {
            crate::connectors::handle(service, method, params).await
        }
        "parity.tasks" | "parity.run" | "parity.native" | "parity.results" | "parity.export" => crate::parity::handle(service,method,params).await,
        "onboarding.state" => Ok(json!({"done":store.app_setting("onboarding.done").map_err(internal)?.as_deref()==Some("true"),"sources":service.has_import_sources()})),
        "onboarding.done" => { store.set_app_setting("onboarding.done",Some("true")).map_err(internal)?; Ok(json!({})) },
        "deps.status" | "deps.install" => crate::deps::handle(service, method, params).await,
        method if method.starts_with("models.") || matches!(method, "hardware.get" | "llama.status" | "hf.status" | "hf.set_token") => {
            crate::models::rpc::handle(service, method, params).await
        }
        "import.cancel" => { service.cancel_import(); Ok(json!({})) },
        "history.search" => {
            #[derive(Deserialize)] struct Search {query:String,project_id:Option<i64>,limit:Option<usize>}
            let request:Search=parse(params)?;
            Ok(json!(store.search_history(&request.query,request.project_id,request.limit.unwrap_or(20)).map_err(internal)?))
        }
        "workspace.search" => {
            #[derive(Deserialize)] struct Search {query:String,project_id:Option<i64>,#[serde(default)] kinds:Vec<String>,limit:Option<usize>}
            let request:Search=parse(params)?;
            let kinds:Vec<&str>=request.kinds.iter().map(String::as_str).collect();
            if let Some(project)=request.project_id
                && !store.list_projects().map_err(internal)?.iter().any(|p|p.id==project) {return Err((1100,"not_found".into()));
            }
            Ok(json!(store.search_workspace_scoped(request.project_id,&request.query,&kinds,request.limit.unwrap_or(20)).map_err(store_error)?))
        }
        "workspace.export" => {
            #[derive(Deserialize)] struct Target {path:String}
            let request:Target=parse(params)?;
            let count=store.export_items_json(&absolute_path(&request.path)?).map_err(store_error)?;
            Ok(json!({"count":count}))
        }
        "workspace.backup" => {
            #[derive(Deserialize)] struct Target {path:String}
            let request:Target=parse(params)?;
            store.backup_to(&absolute_path(&request.path)?).map_err(store_error)?;
            Ok(json!({}))
        }
        "history.open_imported" => {
            #[derive(Deserialize)] struct Open {id:i64}
            let request:Open=parse(params)?;
            Ok(json!(store.open_imported(request.id).map_err(internal)?))
        }
        "catalog.set_project" => {
            #[derive(Deserialize)] struct Link {id:i64,project_id:i64}
            let request:Link=parse(params)?;
            if !store.list_projects().map_err(internal)?.iter().any(|p|p.id==request.project_id) {return Err((INVALID_PARAMS,"Unknown project".into()));}
            catalog(service)?.set_project(request.id,request.project_id).map_err(catalog_error)?;
            service.agents().catalog_changed(None);
            Ok(json!({}))
        }
        "composer.complete" => {
            let request: CompleteInput = parse(params)?;
            let items = completion::complete(service, request.project_id, request.trigger, &request.query)
                .await
                .map_err(internal)?;
            Ok(json!(items))
        }
        "turn.start" => {
            let request: StartTurn = parse(params)?;
            if store.session_agent(request.session_id).is_err(){return Err((UNKNOWN_SESSION,"this session doesn't exist".into()));}
            if service.is_running(request.session_id){return Err((BUSY,"this session is busy".into()));}
            let options = crate::agents::TurnOptions::checked(request.model, request.effort).map_err(|message| (INVALID_PARAMS, message))?;
            let caps=crate::files::capabilities(service,request.session_id,options.model.as_deref()).await.map_err(|e|(INVALID_PARAMS,e))?;
            let files=crate::files::prepare_inputs(service.store(),request.session_id,&request.attachments,&caps).map_err(|e|(INVALID_PARAMS,e))?;
            service.start_turn_with_files(request.session_id, request.prompt, request.mode, options,files,caps.images=="supported").map_err(service_error)?;
            Ok(json!({}))
        }
        "turn.stop" => {
            let request: StopTurn = parse(params)?;
            service.stop_turn(request.session_id);
            Ok(json!({}))
        }
        "approvals.list" => Ok(json!(service.pending_approvals())),
        "approvals.resolve" => {
            let request: ResolveApproval = parse(params)?;
            service.resolve_approval(&request.id, request.allow).map_err(service_error)?;
            Ok(json!({}))
        }
        "questions.list" => Ok(json!(service.pending_questions())),
        "question.answer" => {
            let request: AnswerQuestion = parse(params)?;
            service.answer_question(&request.id, request.answers).map_err(service_error)?;
            Ok(json!({}))
        }
        "handoff.start" => {
            let request: StartHandoff = parse(params)?;
            let session = service
                .start_handoff_with_options(request.from, request.to, request.task, request.mode, crate::TurnOptions::checked(request.model,request.effort).map_err(|message| (INVALID_PARAMS,message))?)
                .map_err(service_error)?;
            Ok(json!({ "session_id": session }))
        }
        "service.shutdown" => {
            service.request_shutdown();
            Ok(json!({}))
        }
        _ => Err((METHOD_NOT_FOUND, format!("unknown method `{method}`"))),
    }
}
