use std::collections::BTreeMap;
use std::sync::Mutex;

use vatra_core::{ChunkedStore, EntryBackend, MemorySecretStore, SecretError, SecretStore};

fn assert_send_sync<T: Send + Sync + ?Sized>() {}

#[test]
fn stores_are_usable_across_threads() {
    assert_send_sync::<dyn SecretStore>();
    assert_send_sync::<MemorySecretStore>();
}

#[test]
fn memory_store_keeps_the_last_value_and_forgets_deleted_ones() {
    let store = MemorySecretStore::default();
    assert_eq!(store.get("token").unwrap(), None);
    store.set("token", "first").unwrap();
    store.set("token", "second").unwrap();
    assert_eq!(store.get("token").unwrap().as_deref(), Some("second"));
    store.delete("token").unwrap();
    assert_eq!(store.get("token").unwrap(), None);
}

#[test]
fn deleting_a_missing_secret_is_not_an_error() {
    MemorySecretStore::default().delete("nothing").unwrap();
}

#[test]
fn keys_do_not_mix() {
    let store = MemorySecretStore::default();
    store.set("a", "1").unwrap();
    store.set("b", "2").unwrap();
    assert_eq!(store.get("a").unwrap().as_deref(), Some("1"));
    assert_eq!(store.get("b").unwrap().as_deref(), Some("2"));
}

/// An entry backend that refuses values longer than the Windows Credential Manager takes (1280 characters).
#[derive(Clone, Default)]
struct LimitedEntries {
    entries: std::sync::Arc<Mutex<BTreeMap<String, String>>>,
}

const WINDOWS_LIMIT_CHARS: usize = 1280;

impl EntryBackend for LimitedEntries {
    fn read(&self, name: &str) -> Result<Option<String>, SecretError> {
        Ok(self.entries.lock().unwrap().get(name).cloned())
    }

    fn write(&self, name: &str, value: &str) -> Result<(), SecretError> {
        if value.chars().count() > WINDOWS_LIMIT_CHARS {
            return Err(SecretError::Backend("the value is too long".into()));
        }
        self.entries.lock().unwrap().insert(name.to_string(), value.to_string());
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<(), SecretError> {
        self.entries.lock().unwrap().remove(name);
        Ok(())
    }
}

#[test]
fn long_credentials_survive_the_store_limit() {
    let entries = LimitedEntries::default();
    let store = ChunkedStore::new(entries.clone());
    // A token pair in the shape of the stored login: far beyond one entry.
    let credentials =
        format!("{{\"access_token\":\"{}\",\"refresh_token\":\"{}\"}}", "e".repeat(3000), "r".repeat(3000));
    assert!(entries.write("probe", &credentials).is_err());

    store.set("mcp-oauth:server", &credentials).unwrap();
    assert_eq!(store.get("mcp-oauth:server").unwrap().as_deref(), Some(credentials.as_str()));

    // A value with characters of several bytes is split between characters, not inside one.
    let accented = "ș".repeat(1500);
    store.set("accented", &accented).unwrap();
    assert_eq!(store.get("accented").unwrap().as_deref(), Some(accented.as_str()));

    // A shorter value replaces the longer one and leaves none of its parts behind.
    store.set("mcp-oauth:server", "short").unwrap();
    assert_eq!(store.get("mcp-oauth:server").unwrap().as_deref(), Some("short"));
    store.delete("accented").unwrap();
    store.delete("mcp-oauth:server").unwrap();
    assert!(entries.entries.lock().unwrap().is_empty());
}
