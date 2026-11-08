use serde_json::json;
use std::sync::Arc;
use vatra_core::{MemorySecretStore, SecretStore, Store};
use vatra_extensions::Catalog;
use vatra_service::mcp_manager::{import_plans, registry_document};
use vatra_service::{ConfigAgents, Service, config::Config};
#[path = "../../vatra-engine/tests/support/mod.rs"]
mod support;

fn service(data: &std::path::Path) -> (Arc<Service>, Arc<Catalog>, Arc<MemorySecretStore>) {
    let db = data.join("vatra.db");
    let store = Arc::new(Store::open(&db).unwrap());
    let catalog = Arc::new(Catalog::open(&db, data).unwrap());
    let secrets = Arc::new(MemorySecretStore::default());
    let factory = ConfigAgents::new(Config::empty())
        .with_secret_store(secrets.clone())
        .with_catalog(catalog.clone());
    (
        Service::new(store, Arc::new(factory)).unwrap(),
        catalog,
        secrets,
    )
}

#[test]
fn reviewed_install_persists_only_keychain_references_and_survives_editing() {
    let data = tempfile::tempdir().unwrap();
    let (service, catalog, secrets) = service(data.path());
    let call = |method, params| vatra_service::mcp_manager::configure(&service, method, params);
    let document=json!({"mcpServers":{"demo":{"type":"http","url":"https://example.org/mcp","headers":{"Authorization":"Bearer never-in-sqlite"}}}}).to_string();
    let reviewed = call("mcp.preview", json!({"document":document})).unwrap();
    assert!(!reviewed.to_string().contains("never-in-sqlite"));
    assert!(
        call(
            "mcp.install",
            json!({"document":document,"reviewHash":"stale"})
        )
        .is_err()
    );
    let saved = call(
        "mcp.install",
        json!({"document":document,"reviewHash":reviewed["servers"][0]["plan"]["reviewHash"]}),
    )
    .unwrap();
    let row = catalog
        .list()
        .unwrap()
        .into_iter()
        .find(|row| row.id == saved["id"].as_i64().unwrap())
        .unwrap();
    assert!(!row.enabled);
    assert!(!row.details.to_string().contains("never-in-sqlite"));
    assert!(
        !std::fs::read_to_string(row.content_dir.join("server.json"))
            .unwrap()
            .contains("never-in-sqlite")
    );
    assert!(
        secrets
            .keys()
            .iter()
            .any(|key| secrets.get(key).unwrap().as_deref() == Some("Bearer never-in-sqlite"))
    );
    let form = call("mcp.form", json!({"id":row.id})).unwrap();
    let next = call("mcp.preview", json!({"document":form["document"]})).unwrap();
    call("mcp.install",json!({"document":form["document"],"reviewHash":next["servers"][0]["plan"]["reviewHash"],"replaceHash":row.content_hash})).unwrap();
    let moved = form["document"]
        .as_str()
        .unwrap()
        .replace("example.org", "other.example.org");
    assert!(call("mcp.preview", json!({"document":moved})).is_err());
}

#[tokio::test]
async fn test_connection_discovers_real_http_tools_without_tool_execution() {
    let server = support::TestServer::start(false).await;
    let data = tempfile::tempdir().unwrap();
    let (service, catalog, _) = service(data.path());
    let document = json!({"mcpServers":{"test":{"type":"http","url":server.url}}}).to_string();
    let reviewed = vatra_service::mcp_manager::configure(
        &service,
        "mcp.preview",
        json!({"document":document}),
    )
    .unwrap();
    let saved = vatra_service::mcp_manager::configure(
        &service,
        "mcp.install",
        json!({"document":document,"reviewHash":reviewed["servers"][0]["plan"]["reviewHash"]}),
    )
    .unwrap();
    let result =
        vatra_service::mcp_manager::runtime(&service, "mcp.test", json!({"id":saved["id"]}))
            .await
            .unwrap();
    assert_eq!(result["connected"], true);
    assert_eq!(result["disconnectedAfterTest"], true);
    assert!(!result["tools"].as_array().unwrap().is_empty());
    assert!(!catalog.list().unwrap()[0].enabled);
}

#[tokio::test]
async fn stdio_requires_command_approval_then_lists_capabilities_and_honors_tool_toggles() {
    let debug = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let binary = debug.join(if cfg!(windows) {
        "vatra-fixture-mcp.exe"
    } else {
        "vatra-fixture-mcp"
    });
    if !binary.exists() {
        assert!(
            std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
                .args(["build", "-p", "vatra-engine", "--bin", "vatra-fixture-mcp"])
                .status()
                .unwrap()
                .success()
        );
    }
    let data = tempfile::tempdir().unwrap();
    let (service, catalog, _) = service(data.path());
    let document = json!({"mcpServers":{"stdio":{"command":binary,"args":[]}}}).to_string();
    let reviewed = vatra_service::mcp_manager::configure(
        &service,
        "mcp.preview",
        json!({"document":document}),
    )
    .unwrap();
    let saved = vatra_service::mcp_manager::configure(
        &service,
        "mcp.install",
        json!({"document":document,"reviewHash":reviewed["servers"][0]["plan"]["reviewHash"]}),
    )
    .unwrap();
    let id = saved["id"].as_i64().unwrap();
    assert!(
        vatra_service::mcp_manager::runtime(&service, "mcp.test", json!({"id":id}))
            .await
            .is_err()
    );
    catalog
        .approve(id, saved["hash"].as_str().unwrap())
        .unwrap();
    let result = vatra_service::mcp_manager::runtime(&service, "mcp.test", json!({"id":id}))
        .await
        .unwrap();
    assert_eq!(result["connected"], true);
    assert!(!result["resources"].as_array().unwrap().is_empty());
    assert!(!result["prompts"].as_array().unwrap().is_empty());
    vatra_service::mcp_manager::configure(
        &service,
        "mcp.set_tools",
        json!({"id":id,"expectedHash":saved["hash"],"disabledTools":["echo"]}),
    )
    .unwrap();
    let result = vatra_service::mcp_manager::runtime(&service, "mcp.test", json!({"id":id}))
        .await
        .unwrap();
    assert!(
        result["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "echo" && tool["enabled"] == false)
    );
    let project = service.store().create_project("MCP integration").unwrap();
    catalog.set_enabled(id, true).unwrap();
    service.agents().catalog_changed(None);
    assert_eq!(
        vatra_service::mcp_manager::runtime(
            &service,
            "mcp.connect",
            json!({"id":id,"projectId":project})
        )
        .await
        .unwrap()["connected"],
        true
    );
    let hub = service.agents().mcp_hub(project, None).unwrap();
    assert!(
        hub.call("mcp__stdio__echo", json!({"text":"must not execute"}))
            .await
            .is_error
    );
    assert_eq!(
        vatra_service::mcp_manager::runtime(
            &service,
            "mcp.disconnect",
            json!({"id":id,"projectId":project})
        )
        .await
        .unwrap()["connected"],
        false
    );
    let current=catalog.list().unwrap().into_iter().find(|entry|entry.id==id).unwrap();
    vatra_service::mcp_manager::configure(&service,"mcp.set_tools",json!({"id":id,"expectedHash":current.content_hash,"disabledTools":[],"enabledTools":["echo"]})).unwrap();
    let allowed=vatra_service::mcp_manager::runtime(&service,"mcp.test",json!({"id":id})).await.unwrap();
    assert!(allowed["tools"].as_array().unwrap().iter().any(|tool|tool["name"]=="echo"&&tool["enabled"]==true));
}

#[tokio::test]
async fn installed_server_oauth_uses_the_existing_secure_login_flow() {
    use vatra_service::Notification;
    let server = support::TestServer::start(true).await;
    let data = tempfile::tempdir().unwrap();
    let (service, _, secrets) = service(data.path());
    let document =
        json!({"mcpServers":{"oauth":{"type":"http","url":server.url,"oauth":{}}}}).to_string();
    let reviewed = vatra_service::mcp_manager::configure(
        &service,
        "mcp.preview",
        json!({"document":document}),
    )
    .unwrap();
    let saved = vatra_service::mcp_manager::configure(
        &service,
        "mcp.install",
        json!({"document":document,"reviewHash":reviewed["servers"][0]["plan"]["reviewHash"]}),
    )
    .unwrap();
    let mut updates = service.subscribe();
    let browser = support::approve(server.shared.clone(), false);
    vatra_service::mcp_manager::runtime(&service, "mcp.login_installed", json!({"id":saved["id"]}))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            match updates.recv().await.unwrap() {
                Notification::OpenUrl { url } => browser(&url),
                Notification::McpLogin { ok, .. } => {
                    assert!(ok);
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert!(!secrets.keys().is_empty());
    assert_eq!(
        vatra_service::mcp_manager::runtime(&service, "mcp.test", json!({"id":saved["id"]}))
            .await
            .unwrap()["connected"],
        true
    );
}

#[test]
fn import_review_never_exposes_literal_credentials_and_preserves_arguments() {
    let text=json!({"mcpServers":{"demo":{"command":"node.exe","args":["C:/Tools/server.js","space in argument"],"env":{"API_KEY":"test-secret-value","MODE":"production"},"headers":{"Authorization":"Bearer test-token"}}}}).to_string();
    let plans = import_plans(&text, None).unwrap();
    let public = serde_json::to_string(&plans).unwrap();
    assert!(!public.contains("test-secret-value"));
    assert!(!public.contains("test-token"));
    assert!(!public.contains("production"));
    assert_eq!(plans[0].config.args[1], "space in argument");
    assert_eq!(plans[0].secret_names.len(), 3);
    assert_ne!(
        import_plans(&text.replace("test-secret-value", "changed-secret"), None).unwrap()[0]
            .review_hash,
        plans[0].review_hash
    );
}

#[test]
fn review_rejects_unsupported_transports_and_credentials_in_urls_or_arguments() {
    for server in [
        json!({"url":"https://example.org/mcp?token=secret","type":"http"}),
        json!({"url":"https://example.org/sse","type":"sse"}),
        json!({"command":"agent.exe","args":["--api-key","secret"]}),
        json!({"command":"agent.exe","headersHelper":"run something"}),
    ] {
        assert!(import_plans(&json!({"mcpServers":{"demo":server}}).to_string(), None).is_err());
    }
    assert!(import_plans("{}", None).is_err());
}

#[test]
fn registry_reference_and_owned_credentials_are_cleaned_up_on_uninstall() {
    let data = tempfile::tempdir().unwrap();
    let (service, catalog, secrets) = service(data.path());
    let document=json!({"mcpServers":{"demo":{"type":"http","url":"https://example.org/mcp","headers":{"Authorization":"Bearer cleanup-test"}}}}).to_string();
    let reviewed = vatra_service::mcp_manager::configure(
        &service,
        "mcp.preview",
        json!({"document":document}),
    )
    .unwrap();
    let saved=vatra_service::mcp_manager::configure(&service,"mcp.install",json!({"document":document,"reviewHash":reviewed["servers"][0]["plan"]["reviewHash"],"source":{"name":"io.example/demo","version":"1.2.3","method":"remote","index":0}})).unwrap();
    let sources =
        vatra_service::mcp_manager::configure(&service, "mcp.sources", json!({})).unwrap();
    assert_eq!(sources[0]["source"]["version"], "1.2.3");
    assert!(!secrets.keys().is_empty());
    vatra_service::mcp_manager::configure(
        &service,
        "mcp.remove",
        json!({"id":saved["id"],"expectedHash":saved["hash"]}),
    )
    .unwrap();
    assert!(catalog.list().unwrap().is_empty());
    assert!(secrets.keys().is_empty());
    assert_eq!(
        vatra_service::mcp_manager::configure(&service, "mcp.sources", json!({})).unwrap(),
        json!([])
    );
}

#[tokio::test]
#[ignore = "requires network access to the public DeepWiki MCP endpoint; lists capabilities only"]
async fn live_public_remote_mcp_capability_discovery() {
    let data = tempfile::tempdir().unwrap();
    let (service, _, _) = service(data.path());
    let document =
        json!({"mcpServers":{"deepwiki":{"type":"http","url":"https://mcp.deepwiki.com/mcp"}}})
            .to_string();
    let reviewed = vatra_service::mcp_manager::configure(
        &service,
        "mcp.preview",
        json!({"document":document}),
    )
    .unwrap();
    let saved = vatra_service::mcp_manager::configure(
        &service,
        "mcp.install",
        json!({"document":document,"reviewHash":reviewed["servers"][0]["plan"]["reviewHash"]}),
    )
    .unwrap();
    let result =
        vatra_service::mcp_manager::runtime(&service, "mcp.test", json!({"id":saved["id"]}))
            .await
            .unwrap();
    assert_eq!(result["connected"], true, "{result}");
    assert_eq!(result["disconnectedAfterTest"], true);
    assert!(!result["tools"].as_array().unwrap().is_empty());
}

#[test]
fn registry_installation_uses_pinned_npm_versions_and_real_remote_transports() {
    let server = json!({"name":"io.example/demo","version":"1.2.3","packages":[{"registryType":"npm","identifier":"@example/demo","version":"1.2.3","transport":{"type":"stdio"},"environmentVariables":[{"name":"API_KEY","isRequired":true,"isSecret":true}]}]});
    let document = registry_document(&server, "demo", "package", 0).unwrap();
    assert_eq!(
        document["mcpServers"]["demo"]["args"],
        json!(["-y", "@example/demo@1.2.3"])
    );
    assert_eq!(
        document["mcpServers"]["demo"]["env"]["API_KEY"],
        "${API_KEY}"
    );
    assert!(registry_document(&server, "demo", "package", 1).is_err());
    let server = json!({"name":"io.example/remote","version":"1","remotes":[{"type":"streamable-http","url":"https://example.org/mcp"}]});
    assert_eq!(
        registry_document(&server, "remote", "remote", 0).unwrap()["mcpServers"]["remote"]["url"],
        "https://example.org/mcp"
    );
}
