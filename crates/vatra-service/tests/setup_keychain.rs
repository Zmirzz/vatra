use vatra_core::{KeyringBackend, KeyringStore, SecretStore, Store};
use vatra_service::{
    config::{Config, agent_secret},
    setup::{AgentForm, agent_from_form, remove, save},
};

#[test]
#[ignore = "writes and deletes a unique synthetic agent entry in Windows Credential Manager"]
fn actual_agent_keychain_round_trip_without_database_secret() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vatra.db");
    let store = Store::open(&path).unwrap();
    let name = format!("test{}", uuid::Uuid::new_v4().simple());
    let secret = format!("synthetic-{}", uuid::Uuid::new_v4());
    let keyring = KeyringStore::new(KeyringBackend);
    let reference = agent_secret(&name);
    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = KeyringStore::new(KeyringBackend).delete(&self.0);
        }
    }
    let _cleanup = Cleanup(reference.clone());
    let (row, key) = agent_from_form(
        AgentForm::Model {
            name: name.clone(),
            provider: "openrouter".into(),
            base_url: None,
            model: "synthetic-model".into(),
            key: Some(secret.clone()),
        },
        0,
    )
    .unwrap();
    save(&store, &keyring, &row, key.as_deref()).unwrap();
    assert_eq!(
        keyring.get(&reference).unwrap().as_deref(),
        Some(secret.as_str())
    );
    let rows = store.configured_agents().unwrap();
    assert!(!serde_json::to_string(&rows).unwrap().contains(&secret));
    let (merged, warnings) = Config::empty().merged_with(&rows, None, &keyring);
    assert!(warnings.is_empty());
    match &merged.agents[&name] {
        vatra_service::config::AgentConfig::Model { api_key, .. } => {
            assert_eq!(api_key.as_deref(), Some(secret.as_str()))
        }
        _ => panic!("wrong agent"),
    }
    remove(&store, &keyring, &name).unwrap();
    assert!(keyring.get(&reference).unwrap().is_none());
    drop(store);
    assert!(
        !std::fs::read(path)
            .unwrap()
            .windows(secret.len())
            .any(|window| window == secret.as_bytes())
    );
}
