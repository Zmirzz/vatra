//! Connectors set up in the app: an account or integration with its settings in SQLite and its secrets in the
//! keychain. The kinds of connector (IMAP, CalDAV, web search) register here; none ship yet, so the registry is empty.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vatra_core::{ConnectorRow, SecretStore, Store};

use crate::setup::{failed, parse};

const MAX_ID_LEN: usize = 64;
// A failed save must not restore over another save/remove that completed in the meantime.
static CONNECTOR_WRITES: Mutex<()> = Mutex::new(());

/// One kind of connector. `secret_fields` names the only fields whose values go to the keychain.
#[async_trait]
pub trait ConnectorKind: Send + Sync {
    fn kind(&self) -> &'static str;
    fn secret_fields(&self) -> &'static [&'static str];
    /// Tries the connector with its settings and saved secrets. Upstream errors are never returned to the client.
    async fn check(&self, settings: &Value, secrets: &BTreeMap<String, String>) -> Result<(), String>;
}

#[derive(Default)]
pub struct ConnectorRegistry {
    kinds: BTreeMap<&'static str, Arc<dyn ConnectorKind>>,
}

impl ConnectorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The kinds that ship with Vatra: a CalDAV calendar and an email account.
    pub fn with_builtin() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(crate::calendar::CalDavKind));
        registry.register(Arc::new(crate::email::EmailKind));
        registry.register(Arc::new(crate::research::WebSearchKind));
        registry.register(Arc::new(crate::telegram::TelegramKind));
        registry.register(Arc::new(crate::ntfy::NtfyKind));
        registry.register(Arc::new(crate::semantic::EmbeddingsKind));
        registry
    }

    pub fn register(&mut self, kind: Arc<dyn ConnectorKind>) {
        self.kinds.insert(kind.kind(), kind);
    }

    /// The kinds' names, sorted.
    pub fn kinds(&self) -> Vec<&'static str> {
        self.kinds.keys().copied().collect()
    }

    pub fn get(&self, kind: &str) -> Option<Arc<dyn ConnectorKind>> {
        self.kinds.get(kind).cloned()
    }
}

/// The keychain name of one secret of a connector.
pub fn connector_secret(id: &str, field: &str) -> String {
    format!("vatra/connector/{id}/{field}")
}

/// What the app sends to save a connector. Secrets are never logged or sent back.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveRequest {
    /// Given when the connector is edited; a new one gets an id here.
    #[serde(default)]
    pub id: Option<String>,
    pub kind: String,
    pub name: String,
    pub settings: Value,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
}

/// A connector as the app lists it. Only whether each secret exists is shown, never its value.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorView {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub settings: Value,
    pub has_secret: BTreeMap<String, bool>,
}

fn check_id(id: &str) -> Result<(), String> {
    let valid = !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err("A connector's id uses lowercase letters, digits and dashes, and is at most 64 characters.".into())
    }
}

fn new_id(kind: &str, now: i64) -> String {
    format!("{kind}-{now:x}-{:08x}", rand::random::<u32>())
}

/// Saves the connector and its secrets. A secret that isn't given, or is empty, keeps the one saved before. The
/// secrets are written first and the row after them, so the row never points at a secret that wasn't written. If
/// the keychain refuses a secret, no row is saved. Saving doesn't run a check; that is a separate call.
pub fn save(
    store: &Store,
    secrets: &dyn SecretStore,
    registry: &ConnectorRegistry,
    request: SaveRequest,
    now: i64,
) -> Result<String, String> {
    let _guard = CONNECTOR_WRITES.lock().map_err(|_| "Connector changes are unavailable. Restart the service.")?;
    if request.name.trim().is_empty() {
        return Err("Give the connector a name.".into());
    }
    let kind = registry.get(&request.kind).ok_or_else(|| format!("unknown connector kind `{}`", request.kind))?;
    let id = match request.id {
        Some(id) => id,
        None => new_id(kind.kind(), now),
    };
    check_id(&id)?;
    let existing = store.connectors().map_err(|error| error.to_string())?.into_iter().find(|row| row.id == id);
    if existing.is_some_and(|old| old.kind != kind.kind()) {
        return Err("This connector belongs to another kind, so it can't be changed into this one.".into());
    }
    if !request.settings.is_object() {
        return Err("The settings must be a set of fields.".into());
    }
    let declared = kind.secret_fields();
    if declared.iter().any(|field| request.settings.get(*field).is_some()) {
        return Err("Keep secrets out of the settings: put secrets in the secrets field.".into());
    }
    if let Some(field) = request.secrets.keys().find(|field| !declared.contains(&field.as_str())) {
        return Err(format!("`{field}` isn't a secret of this kind."));
    }
    let settings_json = serde_json::to_string(&request.settings).map_err(|error| error.to_string())?;
    let updates: Vec<_> = request.secrets.iter().filter(|(_, value)| !value.is_empty())
        .map(|(field, value)| (connector_secret(&id, field), value)).collect();
    let previous: Vec<_> = updates.iter().map(|(key, _)| {
        secrets.get(key).map(|value| (key.clone(), value)).map_err(|_| "The saved secrets couldn't be read.")
    }).collect::<Result<_, _>>()?;
    for (index, (key, value)) in updates.iter().enumerate() {
        if secrets.set(key, value).is_err() {
            return Err(rollback_error(secrets, &previous[..=index]));
        }
    }
    let row = ConnectorRow { id: id.clone(), kind: kind.kind().into(), name: request.name.trim().into(), settings_json, created_at: now };
    if store.save_connector(&row).is_err() {
        return Err(rollback_error(secrets, &previous));
    }
    Ok(id)
}

fn rollback_error(secrets: &dyn SecretStore, previous: &[(String, Option<String>)]) -> String {
    let mut failed = false;
    for (key, value) in previous.iter().rev() {
        let result = match value {
            Some(value) => secrets.set(key, value),
            None => secrets.delete(key),
        };
        failed |= result.is_err();
    }
    if failed {
        "The connector couldn't be saved, and some previous secrets couldn't be restored. Re-enter its credentials before using it.".into()
    } else {
        "The connector couldn't be saved. Its previous secrets were restored.".into()
    }
}

/// The connectors of the app, oldest first. Secret values are never read into the views, only whether they exist.
pub fn list(store: &Store, secrets: &dyn SecretStore, registry: &ConnectorRegistry) -> Vec<ConnectorView> {
    store
        .connectors()
        .unwrap_or_default()
        .into_iter()
        .map(|row| {
            let has_secret: BTreeMap<String, bool> = registry
                .get(&row.kind)
                .map(|kind| {
                    kind.secret_fields()
                        .iter()
                        .map(|field| {
                            let saved = secrets.get(&connector_secret(&row.id, field)).ok().flatten().is_some();
                            (field.to_string(), saved)
                        })
                        .collect()
                })
                .unwrap_or_default();
            ConnectorView {
                settings: serde_json::from_str(&row.settings_json).unwrap_or(Value::Null),
                has_secret,
                id: row.id,
                kind: row.kind,
                name: row.name,
            }
        })
        .collect()
}

/// Deletes the secrets of the connector and then its row. The secrets of a kind that is no longer registered can't
/// be named, so they stay in the keychain while the row goes.
pub fn remove(store: &Store, secrets: &dyn SecretStore, registry: &ConnectorRegistry, id: &str) -> Result<(), String> {
    let _guard = CONNECTOR_WRITES.lock().map_err(|_| "Connector changes are unavailable. Restart the service.")?;
    let row = store.connectors().map_err(|error| error.to_string())?.into_iter().find(|row| row.id == id);
    if let Some(kind) = row.and_then(|row| registry.get(&row.kind)) {
        for field in kind.secret_fields() {
            secrets.delete(&connector_secret(id, field)).map_err(|error| error.to_string())?;
        }
    }
    store.remove_connector(id).map_err(|error| error.to_string())?;
    Ok(())
}

/// Runs the kind's check with the connector's settings and the secrets it has saved.
pub async fn check(store: &Store, secrets: &dyn SecretStore, registry: &ConnectorRegistry, id: &str) -> Result<(), String> {
    let row = store
        .connectors()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|row| row.id == id)
        .ok_or_else(|| "This connector doesn't exist.".to_string())?;
    let kind = registry.get(&row.kind).ok_or_else(|| format!("unknown connector kind `{}`", row.kind))?;
    let settings: Value = serde_json::from_str(&row.settings_json).map_err(|error| error.to_string())?;
    let mut values = BTreeMap::new();
    for field in kind.secret_fields() {
        if let Some(value) = secrets.get(&connector_secret(id, field)).map_err(|_| "The connector's credentials couldn't be read.")? {
            values.insert(field.to_string(), value);
        }
    }
    if let Some(stored) = values.get("password").cloned() {
        let resolved = crate::oauth::credential(store, secrets, id, &settings, stored).await?;
        values.insert("password".into(), resolved);
    }
    kind.check(&settings, &values).await.map_err(|_| "The connector check failed. Check its settings and credentials, then try again.".into())
}

type Reply = Result<Value, (i64, String)>;

/// The `connectors.*` requests.
pub(crate) async fn handle(service: &Arc<crate::Service>, method: &str, params: Value) -> Reply {
    let store = service.store();
    let secrets = service.agents().secrets();
    let registry = service.connector_registry();
    #[derive(Deserialize)]
    struct Id {
        id: String,
    }
    match method {
        "connectors.kinds" => Ok(json!(registry.kinds())),
        "connectors.list" => Ok(json!(list(store, secrets.as_ref(), &registry))),
        "connectors.save" => {
            let request: SaveRequest = parse(params)?;
            let id = save(store, secrets.as_ref(), &registry, request, crate::models::now()).map_err(failed)?;
            Ok(json!({ "id": id }))
        }
        "connectors.remove" => {
            let request: Id = parse(params)?;
            remove(store, secrets.as_ref(), &registry, &request.id).map_err(failed)?;
            Ok(json!({}))
        }
        "connectors.check" => {
            let request: Id = parse(params)?;
            let outcome = check(store, secrets.as_ref(), &registry, &request.id).await;
            crate::health::record(store, &format!("connector.{}", request.id), outcome.is_ok(), outcome.as_ref().err().map(String::as_str).unwrap_or(""));
            outcome.map_err(failed)?;
            Ok(json!({}))
        }
        _ => Err((-32601, format!("unknown method `{method}`"))),
    }
}
