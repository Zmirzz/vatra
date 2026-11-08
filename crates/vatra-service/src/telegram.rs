//! Telegram: a bot the person owns, for two things. Results of scheduled tasks arrive as messages, and the person can
//! write to the bot to see their tasks, run one of them, or ask a question. Only the chat the person set up is
//! heard; anyone else's message is ignored. A question is answered by a model with no tools, so a message can
//! never make the computer do anything. `/run` starts a task the person already created, which still asks for
//! approval in the app before it does anything that needs it.

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::connectors::{ConnectorKind, connector_secret};
use crate::service::Service;

const DEFAULT_API: &str = "https://api.telegram.org";
const MAX_REPLY_CHARS: usize = 3500;

pub struct TelegramKind;

fn api_base(settings: &Value) -> Result<String, String> {
    match settings.get("api_base").and_then(Value::as_str).filter(|b| !b.is_empty()) {
        None => Ok(DEFAULT_API.into()),
        Some(base) => {
            // Another address is for a Telegram server of one's own on this machine; the token never goes elsewhere.
            let url = url::Url::parse(base).map_err(|_| "The Telegram address isn't valid.".to_string())?;
            if url.host_str().is_some_and(crate::email::net::is_local_host) {
                Ok(base.trim_end_matches('/').to_string())
            } else {
                Err("The Telegram address must be on this machine.".into())
            }
        }
    }
}

async fn call(base: &str, token: &str, method: &str, body: Value) -> Result<Value, String> {
    let response = reqwest::Client::new()
        .post(format!("{base}/bot{token}/{method}"))
        .timeout(Duration::from_secs(20))
        .json(&body)
        .send()
        .await
        .map_err(|_| "Telegram couldn't be reached.".to_string())?;
    let status = response.status();
    let value: Value = response.json().await.map_err(|_| "Telegram answered something unreadable.".to_string())?;
    if !status.is_success() || value["ok"] != true {
        return Err(format!("Telegram refused the request ({}).", status.as_u16()));
    }
    Ok(value["result"].clone())
}

#[async_trait]
impl ConnectorKind for TelegramKind {
    fn kind(&self) -> &'static str {
        "telegram"
    }
    fn secret_fields(&self) -> &'static [&'static str] {
        &["bot_token"]
    }
    async fn check(&self, settings: &Value, secrets: &BTreeMap<String, String>) -> Result<(), String> {
        if settings.get("chat_id").and_then(Value::as_i64).is_none() {
            return Err("Enter the chat number the bot may talk to.".into());
        }
        let token = secrets.get("bot_token").cloned().unwrap_or_default();
        call(&api_base(settings)?, &token, "getMe", json!({})).await.map(|_| ())
    }
}

struct Bot {
    id: String,
    base: String,
    token: String,
    chat_id: i64,
    agent: Option<String>,
    notify: bool,
}

fn bots(service: &Service) -> Vec<Bot> {
    let secrets = service.agents().secrets();
    service
        .store()
        .connectors()
        .unwrap_or_default()
        .into_iter()
        .filter(|row| row.kind == "telegram")
        .filter_map(|row| {
            let settings: Value = serde_json::from_str(&row.settings_json).ok()?;
            let token = secrets.get(&connector_secret(&row.id, "bot_token")).ok().flatten()?;
            Some(Bot {
                base: api_base(&settings).ok()?,
                token,
                chat_id: settings.get("chat_id").and_then(Value::as_i64)?,
                agent: settings.get("agent").and_then(Value::as_str).map(str::to_string),
                notify: settings.get("notify_tasks").and_then(Value::as_bool).unwrap_or(true),
                id: row.id,
            })
        })
        .collect()
}

fn shorten(text: &str) -> String {
    let text: String = crate::email::mime::clean(text);
    if text.chars().count() > MAX_REPLY_CHARS { text.chars().take(MAX_REPLY_CHARS).collect::<String>() + "…" } else { text }
}

async fn send(bot: &Bot, text: &str) {
    let _ = call(&bot.base, &bot.token, "sendMessage", json!({"chat_id": bot.chat_id, "text": shorten(text)})).await;
}

/// Tells the person, on every bot that has notifications on, how a scheduled task ended.
pub async fn notify_task(service: &Service, name: &str, ok: bool, summary: &str) {
    for bot in bots(service).into_iter().filter(|b| b.notify) {
        send(&bot, &format!("{} {name}\n\n{summary}", if ok { "Done:" } else { "Failed:" })).await;
    }
}

/// Reads the new messages of one bot and answers them. Returns how many messages were handled.
pub async fn poll_once(service: &Arc<Service>, id: &str) -> usize {
    let Some(bot) = bots(service).into_iter().find(|b| b.id == id) else { return 0 };
    let key = format!("telegram.{id}.offset");
    let offset: i64 = service.store().app_setting(&key).ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(0);
    let Ok(updates) = call(&bot.base, &bot.token, "getUpdates", json!({"offset": offset, "timeout": 0, "allowed_updates": ["message"]})).await else {
        return 0;
    };
    let mut handled = 0;
    let mut next = offset;
    for update in updates.as_array().cloned().unwrap_or_default() {
        next = next.max(update["update_id"].as_i64().unwrap_or(0) + 1);
        let message = &update["message"];
        // Only the chat that was set up is heard.
        if message["chat"]["id"].as_i64() != Some(bot.chat_id) {
            continue;
        }
        let Some(text) = message["text"].as_str().map(str::trim).filter(|t| !t.is_empty()) else { continue };
        handled += 1;
        let reply = answer(service, &bot, text).await;
        send(&bot, &reply).await;
    }
    if next != offset {
        let _ = service.store().set_app_setting(&key, Some(&next.to_string()));
    }
    handled
}

async fn answer(service: &Arc<Service>, bot: &Bot, text: &str) -> String {
    if text == "/start" || text == "/help" {
        return "Write a question and I'll answer it. /tasks lists your scheduled tasks; /run <number> starts one.".into();
    }
    if text == "/tasks" {
        let tasks = service.store().scheduled_tasks().unwrap_or_default();
        if tasks.is_empty() {
            return "You have no scheduled tasks.".into();
        }
        return tasks
            .iter()
            .map(|t| format!("{}. {} - {}", t.id, t.name, if t.enabled { "on" } else { "off" }))
            .collect::<Vec<_>>()
            .join("\n");
    }
    if let Some(number) = text.strip_prefix("/run") {
        let Ok(task_id) = number.trim().parse::<i64>() else { return "Say which task: /run 3".into() };
        return match crate::scheduler::handle(service, "schedule.run_now", json!({"id": task_id})).await {
            Ok(_) => "Started. The result will arrive here when it is done.".into(),
            Err((_, message)) => message,
        };
    }
    let system = "You answer short questions sent from a phone. Be brief. You have no tools and cannot do anything but write text.";
    match service.agents().complete_text(bot.agent.as_deref(), system, text).await {
        Ok(reply) => reply,
        Err(message) => message,
    }
}

pub struct TelegramLoop {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TelegramLoop {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn start(service: &Arc<Service>) -> TelegramLoop {
    let weak: Weak<Service> = Arc::downgrade(service);
    let task = tokio::spawn(async move {
        let mut ticks = tokio::time::interval(Duration::from_secs(4));
        loop {
            ticks.tick().await;
            let Some(service) = weak.upgrade() else { break };
            for bot in bots(&service) {
                poll_once(&service, &bot.id).await;
            }
        }
    });
    TelegramLoop { task }
}
