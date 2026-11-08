use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

/// The name the keychain entries of Vatra are filed under.
const SERVICE: &str = "Vatra";
/// Longest part of a value that one entry takes. Windows Credential Manager stores 2560 bytes of UTF-16, which is
/// about 1280 characters, so a longer value is split into parts.
const PART_BYTES: usize = 1000;
/// The value of an entry that holds the parts of a longer value. It is followed by the number of parts.
const MANIFEST: &str = "vatra-parts:";

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("the keychain couldn't be used: {0}")]
    Backend(String),
}

/// Where the secrets of Vatra live: MCP secrets and tokens. Values are never logged or sent to the client.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError>;
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError>;
    fn delete(&self, key: &str) -> Result<(), SecretError>;
}

/// One named entry of a credential store, which may refuse values that are too long.
pub trait EntryBackend: Send + Sync {
    fn read(&self, name: &str) -> Result<Option<String>, SecretError>;
    fn write(&self, name: &str, value: &str) -> Result<(), SecretError>;
    fn remove(&self, name: &str) -> Result<(), SecretError>;
}

/// The operating system keychain: Windows Credential Manager, Keychain on macOS, or the Secret Service.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyringBackend;

fn backend(error: keyring::Error) -> SecretError {
    SecretError::Backend(error.to_string())
}

impl EntryBackend for KeyringBackend {
    fn read(&self, name: &str) -> Result<Option<String>, SecretError> {
        let entry = keyring::Entry::new(SERVICE, name).map_err(backend)?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(backend(error)),
        }
    }

    fn write(&self, name: &str, value: &str) -> Result<(), SecretError> {
        let entry = keyring::Entry::new(SERVICE, name).map_err(backend)?;
        entry.set_password(value).map_err(backend)
    }

    fn remove(&self, name: &str) -> Result<(), SecretError> {
        let entry = keyring::Entry::new(SERVICE, name).map_err(backend)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(backend(error)),
        }
    }
}

/// A secret store on the keychain. Values longer than one entry takes are split over numbered entries.
pub type KeyringStore = ChunkedStore<KeyringBackend>;

/// Keeps every value in as many entries of the backend as its length needs. The entry of the key holds either the
/// value or the manifest of its parts.
#[derive(Clone, Debug)]
pub struct ChunkedStore<B> {
    backend: B,
}

impl<B> ChunkedStore<B> {
    pub fn new(backend: B) -> ChunkedStore<B> {
        ChunkedStore { backend }
    }
}

fn part_name(key: &str, index: usize) -> String {
    format!("{key}#{index}")
}

/// The parts of a value, each at most `PART_BYTES` long and cut between characters. A short value is one part.
fn split(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = value;
    while rest.len() > PART_BYTES {
        let mut cut = PART_BYTES;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let (head, tail) = rest.split_at(cut);
        parts.push(head);
        rest = tail;
    }
    parts.push(rest);
    parts
}

impl<B: EntryBackend> ChunkedStore<B> {
    /// How many parts the value of the key has, or 0 when it is stored in one entry.
    fn part_count(&self, key: &str) -> Result<usize, SecretError> {
        let stored = self.backend.read(key)?;
        Ok(stored.and_then(|text| text.strip_prefix(MANIFEST).and_then(|count| count.parse().ok())).unwrap_or(0))
    }
}

impl<B: EntryBackend> SecretStore for ChunkedStore<B> {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        let Some(stored) = self.backend.read(key)? else {
            return Ok(None);
        };
        let Some(count) = stored.strip_prefix(MANIFEST) else {
            return Ok(Some(stored));
        };
        let count: usize = count.parse().map_err(|_| SecretError::Backend("a stored secret is damaged".into()))?;
        let mut value = String::new();
        for index in 0..count {
            let part = self
                .backend
                .read(&part_name(key, index))?
                .ok_or_else(|| SecretError::Backend("a part of a stored secret is missing".into()))?;
            value.push_str(&part);
        }
        Ok(Some(value))
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        let previous = self.part_count(key)?;
        let parts = split(value);
        if parts.len() == 1 {
            self.backend.write(key, value)?;
        } else {
            for (index, part) in parts.iter().enumerate() {
                self.backend.write(&part_name(key, index), part)?;
            }
            self.backend.write(key, &format!("{MANIFEST}{}", parts.len()))?;
        }
        let kept = if parts.len() == 1 { 0 } else { parts.len() };
        for index in kept..previous {
            self.backend.remove(&part_name(key, index))?;
        }
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        for index in 0..self.part_count(key)? {
            self.backend.remove(&part_name(key, index))?;
        }
        self.backend.remove(key)
    }
}

/// Secrets kept in memory, for tests and for a service that has no keychain.
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    values: Mutex<BTreeMap<String, String>>,
}

impl SecretStore for MemorySecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        Ok(self.values().get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        self.values().insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.values().remove(key);
        Ok(())
    }
}

impl MemorySecretStore {
    /// The keys that hold a value, without the values.
    pub fn keys(&self) -> Vec<String> {
        self.values().keys().cloned().collect()
    }

    fn values(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, String>> {
        self.values.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
