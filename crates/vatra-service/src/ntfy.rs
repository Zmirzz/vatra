//! Push notifications to a phone through ntfy (the public ntfy.sh or a server of one's own). Reminders and the results of
//! scheduled tasks arrive there as short messages. The topic is the only secret of the public server, so it is kept in the
//! keychain like a password.

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::connectors::{ConnectorKind, connector_secret};
use crate::reminders::{NotifyOutcome, ReminderNotice, ReminderNotifier};
use crate::service::Service;

const DEFAULT_SERVER: &str = "https://ntfy.sh";

pub struct NtfyKind;

fn server_of(settings: &Value) -> Result<String, String> {
    let server = settings.get("server").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_SERVER);
    let url = url::Url::parse(server).map_err(|_| "The server address isn't valid.".to_string())?;
    match url.scheme() {
        "https" => {}
        "http" if url.host_str().is_some_and(crate::email::net::is_local_host) => {}
        _ => return Err("Use an https:// address, or an address on this network.".into()),
    }
    Ok(server.trim_end_matches('/').to_string())
}

fn valid_topic(topic: &str) -> bool {
    (3..=64).contains(&topic.len()) && topic.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

async fn send(server: &str, topic: &str, token: Option<&str>, title: &str, body: &str) -> Result<(), String> {
    if !valid_topic(topic) {
        return Err("The topic is 3 to 64 letters, digits, dashes or underscores.".into());
    }
    let clean = |text: &str, max: usize| -> String { text.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect() };
    let mut request = reqwest::Client::new()
        .post(format!("{server}/{topic}"))
        .timeout(Duration::from_secs(15))
        .header("Title", clean(title, 120).replace('\n', " ").replace(|c: char| !c.is_ascii(), "?"))
        .body(clean(body, 1500));
    if let Some(token) = token.filter(|t| !t.is_empty()) {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|_| "The notification server couldn't be reached.".to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("The notification server answered {}.", response.status().as_u16()))
    }
}

#[async_trait]
impl ConnectorKind for NtfyKind {
    fn kind(&self) -> &'static str {
        "ntfy"
    }
    fn secret_fields(&self) -> &'static [&'static str] {
        &["token"]
    }
    async fn check(&self, settings: &Value, secrets: &BTreeMap<String, String>) -> Result<(), String> {
        let topic = settings.get("topic").and_then(Value::as_str).unwrap_or("");
        send(&server_of(settings)?, topic, secrets.get("token").map(String::as_str), "Vatra", "Notifications from Vatra are set up.").await
    }
}

/// Sends a message to every ntfy account.
pub async fn notify(service: &Service, title: &str, body: &str) {
    let secrets = service.agents().secrets();
    for row in service.store().connectors().unwrap_or_default().into_iter().filter(|r| r.kind == "ntfy") {
        let settings: Value = serde_json::from_str(&row.settings_json).unwrap_or(Value::Null);
        let Ok(server) = server_of(&settings) else { continue };
        let topic = settings.get("topic").and_then(Value::as_str).unwrap_or("").to_string();
        let token = secrets.get(&connector_secret(&row.id, "token")).ok().flatten();
        let result = send(&server, &topic, token.as_deref(), title, body).await;
        crate::health::record(service.store(), &format!("connector.{}", row.id), result.is_ok(), result.as_ref().err().map(String::as_str).unwrap_or(""));
    }
}

/// The desktop notice, and the same reminder on the phone.
pub struct PushNotifier {
    pub inner: Arc<dyn ReminderNotifier>,
    pub service: Weak<Service>,
    pub handle: tokio::runtime::Handle,
}

impl ReminderNotifier for PushNotifier {
    fn show(&self, notice: &ReminderNotice) -> Result<NotifyOutcome, String> {
        let outcome = self.inner.show(notice);
        if let Some(service) = self.service.upgrade() {
            let title = if notice.language == "ro" { "Reamintire" } else { "Reminder" }.to_string();
            let body = notice.title.clone();
            self.handle.spawn(async move { notify(&service, &title, &body).await });
        }
        outcome
    }
    fn status(&self) -> NotifyOutcome {
        self.inner.status()
    }
}
