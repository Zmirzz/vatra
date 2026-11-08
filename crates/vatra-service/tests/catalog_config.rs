use std::sync::Arc;
use vatra_core::McpServerConfig;
use vatra_extensions::{Catalog, Origin, Scope};
use vatra_service::{AgentFactory, ConfigAgents, config::Config};

fn configured() -> Config {
    let mut config = Config::empty();
    config.mcp.push(McpServerConfig {
        name: "demo".into(),
        command: Some("not-a-real-fixture-server".into()),
        ..Default::default()
    });
    config
}

#[tokio::test]
async fn independent_cli_config_is_not_suppressed_by_disabled_catalog_namesake() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Arc::new(Catalog::open(&dir.path().join("vatra.db"), dir.path()).unwrap());
    let config = configured();
    catalog
        .add_server(&config.mcp[0], Origin::Manual, Scope::Global, None, false)
        .unwrap();
    catalog
        .set_enabled(catalog.list().unwrap()[0].id, false)
        .unwrap();
    let agents = ConfigAgents::new(config).with_catalog(catalog);
    let hub = agents.mcp_hub(1, Some(dir.path())).unwrap();
    assert!(
        hub.ensure_server_connected("demo").await,
        "an explicit CLI configuration is independent of the service catalog"
    );
}

#[tokio::test]
async fn disabled_synced_service_server_stays_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Arc::new(Catalog::open(&dir.path().join("vatra.db"), dir.path()).unwrap());
    let agents = ConfigAgents::new(configured()).with_catalog_synced(catalog.clone());
    catalog
        .set_enabled(catalog.list().unwrap()[0].id, false)
        .unwrap();
    let hub = agents.mcp_hub(1, Some(dir.path())).unwrap();
    assert!(!hub.ensure_server_connected("demo").await);
}
