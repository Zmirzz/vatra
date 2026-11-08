//! Connectors: an account or integration set up in the app. Settings live in SQLite, secrets only in the keychain.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use vatra_core::{MemorySecretStore, SecretError, SecretStore, Store};
use vatra_service::connectors::{
    ConnectorKind, ConnectorRegistry, SaveRequest, check, connector_secret, list, remove, save,
};

const SECRET: &str = "hunter2-very-secret";

/// A kind that exists only in tests. Its check passes only with the expected password.
struct Fake;

#[async_trait]
impl ConnectorKind for Fake {
    fn kind(&self) -> &'static str {
        "fake"
    }

    fn secret_fields(&self) -> &'static [&'static str] {
        &["password"]
    }

    async fn check(&self, _settings: &Value, secrets: &BTreeMap<String, String>) -> Result<(), String> {
        if secrets.get("password").map(String::as_str) == Some(SECRET) {
            Ok(())
        } else {
            Err("wrong password".into())
        }
    }
}

/// A keychain whose writes always fail, as when the keychain is locked.
#[derive(Default)]
struct LockedSecrets(MemorySecretStore);

impl SecretStore for LockedSecrets {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        self.0.get(key)
    }

    fn set(&self, _key: &str, _value: &str) -> Result<(), SecretError> {
        Err(SecretError::Backend("locked".into()))
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.0.delete(key)
    }
}

fn registry() -> ConnectorRegistry {
    let mut registry = ConnectorRegistry::new();
    registry.register(Arc::new(Fake));
    registry
}

fn request(id: Option<&str>, secrets: &[(&str, &str)], settings: Value) -> SaveRequest {
    SaveRequest {
        id: id.map(String::from),
        kind: "fake".into(),
        name: "Mail".into(),
        settings,
        secrets: secrets.iter().map(|(field, value)| (field.to_string(), value.to_string())).collect(),
    }
}

#[test]
fn a_secret_goes_to_the_keychain_and_never_to_the_database() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    let settings = json!({ "host": "mail.example.test" });
    let id = save(&store, &secrets, &registry(), request(Some("mail-1"), &[("password", SECRET)], settings.clone()), 10)
        .unwrap();
    assert_eq!(id, "mail-1");
    assert_eq!(connector_secret("mail-1", "password"), "vatra/connector/mail-1/password");
    assert_eq!(secrets.get(&connector_secret("mail-1", "password")).unwrap().as_deref(), Some(SECRET));

    let rows = store.connectors().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].settings_json, settings.to_string());
    let text = format!("{rows:?}{}", serde_json::to_string(&rows).unwrap());
    assert!(!text.contains(SECRET), "{text}");
}

#[test]
fn list_says_whether_a_secret_exists_but_never_shows_it() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    save(&store, &secrets, &registry(), request(Some("with"), &[("password", SECRET)], json!({})), 1).unwrap();
    save(&store, &secrets, &registry(), request(Some("without"), &[], json!({})), 2).unwrap();

    let views = list(&store, &secrets, &registry());
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].id, "with");
    assert_eq!(views[0].has_secret.get("password"), Some(&true));
    assert_eq!(views[1].has_secret.get("password"), Some(&false));
    let text = serde_json::to_string(&views).unwrap();
    assert!(!text.contains(SECRET), "{text}");
    assert!(text.contains("hasSecret"), "{text}");
}

#[test]
fn saving_without_a_secret_keeps_the_saved_one() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    save(&store, &secrets, &registry(), request(Some("mail-1"), &[("password", SECRET)], json!({})), 1).unwrap();
    save(&store, &secrets, &registry(), request(Some("mail-1"), &[("password", "")], json!({})), 2).unwrap();
    save(&store, &secrets, &registry(), request(Some("mail-1"), &[], json!({ "host": "new" })), 3).unwrap();
    assert_eq!(secrets.get(&connector_secret("mail-1", "password")).unwrap().as_deref(), Some(SECRET));
    assert_eq!(store.connectors().unwrap()[0].settings_json, json!({ "host": "new" }).to_string());
}

#[test]
fn removing_a_connector_deletes_its_secrets() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    save(&store, &secrets, &registry(), request(Some("mail-1"), &[("password", SECRET)], json!({})), 1).unwrap();
    remove(&store, &secrets, &registry(), "mail-1").unwrap();
    assert_eq!(secrets.get(&connector_secret("mail-1", "password")).unwrap(), None);
    assert!(store.connectors().unwrap().is_empty());
}

#[test]
fn an_unknown_kind_is_refused() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    let mut unknown = request(None, &[], json!({}));
    unknown.kind = "nope".into();
    assert!(save(&store, &secrets, &registry(), unknown, 1).is_err());
    assert!(store.connectors().unwrap().is_empty());
}

#[test]
fn a_secret_named_in_the_settings_is_refused() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    let error = save(&store, &secrets, &registry(), request(Some("mail-1"), &[], json!({ "password": SECRET })), 1)
        .unwrap_err();
    assert!(error.contains("put secrets in the secrets field"), "{error}");
    assert!(store.connectors().unwrap().is_empty());
    assert_eq!(secrets.get(&connector_secret("mail-1", "password")).unwrap(), None);
}

#[test]
fn an_undeclared_secret_field_is_refused() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    assert!(save(&store, &secrets, &registry(), request(Some("mail-1"), &[("token", SECRET)], json!({})), 1).is_err());
    assert!(store.connectors().unwrap().is_empty());
    assert_eq!(secrets.get(&connector_secret("mail-1", "token")).unwrap(), None);
}

#[test]
fn a_failed_keychain_write_saves_nothing() {
    let store = Store::open_in_memory().unwrap();
    let secrets = LockedSecrets::default();
    assert!(save(&store, &secrets, &registry(), request(Some("mail-1"), &[("password", SECRET)], json!({})), 1).is_err());
    assert!(store.connectors().unwrap().is_empty());
}

#[tokio::test]
async fn check_gives_the_kind_its_secrets_and_reports_its_error() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    save(&store, &secrets, &registry(), request(Some("mail-1"), &[("password", SECRET)], json!({})), 1).unwrap();
    assert_eq!(check(&store, &secrets, &registry(), "mail-1").await, Ok(()));

    save(&store, &secrets, &registry(), request(Some("mail-1"), &[("password", "nope")], json!({})), 2).unwrap();
    assert_eq!(check(&store, &secrets, &registry(), "mail-1").await, Err("The connector check failed. Check its settings and credentials, then try again.".into()));

    assert!(check(&store, &secrets, &registry(), "missing").await.is_err());
}

#[test]
fn an_id_with_unsafe_characters_is_refused() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    let long = "a".repeat(65);
    for id in ["../x", "a/b", "", long.as_str()] {
        assert!(save(&store, &secrets, &registry(), request(Some(id), &[], json!({})), 1).is_err(), "{id}");
    }
    assert!(store.connectors().unwrap().is_empty());
}

struct Multi;
#[async_trait]
impl ConnectorKind for Multi {
    fn kind(&self) -> &'static str { "multi" }
    fn secret_fields(&self) -> &'static [&'static str] { &["first", "second"] }
    async fn check(&self, _: &Value, secrets: &BTreeMap<String,String>) -> Result<(),String> {
        Err(format!("Upstream echoed credentials: {secrets:?}"))
    }
}

fn multi_registry() -> ConnectorRegistry {
    let mut registry = registry();
    registry.register(Arc::new(Multi));
    registry
}

fn multi_request() -> SaveRequest {
    let mut request = request(Some("mail-1"), &[("first", "new-first"), ("second", "new-second")], json!({}));
    request.kind = "multi".into();
    request
}

#[derive(Default)]
struct FailSecond(MemorySecretStore);
impl SecretStore for FailSecond {
    fn get(&self, key:&str) -> Result<Option<String>,SecretError> { self.0.get(key) }
    fn set(&self, key:&str, value:&str) -> Result<(),SecretError> {
        if key.ends_with("/second") && value == "new-second" { return Err(SecretError::Backend("synthetic write failure".into())); }
        self.0.set(key, value)
    }
    fn delete(&self, key:&str) -> Result<(),SecretError> { self.0.delete(key) }
}

#[test]
fn blank_connector_names_are_rejected_before_secret_writes() {
    for name in ["", " \t\n"] {
        let store = Store::open_in_memory().unwrap();
        let secrets = MemorySecretStore::default();
        let mut form = request(Some("mail-1"), &[("password", SECRET)], json!({}));
        form.name = name.into();
        assert!(save(&store, &secrets, &registry(), form, 1).is_err());
        assert!(store.connectors().unwrap().is_empty());
        assert_eq!(secrets.get(&connector_secret("mail-1", "password")).unwrap(),None);
    }
}

#[tokio::test]
async fn connector_check_never_returns_upstream_secret_text() {
    let store = Store::open_in_memory().unwrap();
    let secrets = MemorySecretStore::default();
    save(&store,&secrets,&multi_registry(),multi_request(),1).unwrap();
    let error = check(&store,&secrets,&multi_registry(),"mail-1").await.unwrap_err();
    assert!(!error.contains("new-first") && !error.contains("new-second"), "check leaked synthetic secrets");
    assert!(!error.contains("Upstream"));
}

#[test]
fn partial_secret_write_restores_prior_values_or_removes_new_slots() {
    for previous in [None, Some("old-first")] {
        let store = Store::open_in_memory().unwrap();
        let secrets = FailSecond::default();
        let first = connector_secret("mail-1", "first");
        if let Some(value) = previous { secrets.set(&first,value).unwrap(); }
        assert!(save(&store,&secrets,&multi_registry(),multi_request(),1).is_err());
        assert_eq!(secrets.get(&first).unwrap().as_deref(), previous);
        assert_eq!(secrets.get(&connector_secret("mail-1","second")).unwrap(),None);
        assert!(store.connectors().unwrap().is_empty());
    }
}

#[test]
fn database_failure_rolls_back_all_secret_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connectors.db");
    let store = Store::open(&path).unwrap();
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.execute_batch("CREATE TRIGGER refuse_connectors BEFORE INSERT ON connectors BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;").unwrap();
    let secrets = MemorySecretStore::default();
    secrets.set(&connector_secret("mail-1","first"),"old-first").unwrap();
    assert!(save(&store,&secrets,&multi_registry(),multi_request(),1).is_err());
    assert_eq!(secrets.get(&connector_secret("mail-1","first")).unwrap().as_deref(),Some("old-first"));
    assert_eq!(secrets.get(&connector_secret("mail-1","second")).unwrap(),None);
    assert!(store.connectors().unwrap().is_empty());
}
