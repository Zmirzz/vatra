use serde_json::json;
use vatra_service::registry::{acp_page, mcp_page};

#[tokio::test]
#[ignore = "requires network access to official public registries"]
async fn live_official_registries_and_mcp_pagination() {
    use vatra_service::registry::{RegistryRequest, discover};
    let acp = discover("acp", RegistryRequest::default()).await.unwrap();
    assert!(!acp.entries.is_empty());
    assert!(acp.entries.iter().any(|entry| entry.id == "codex-acp"));
    let mcp = discover(
        "mcp",
        RegistryRequest {
            limit: Some(2),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!mcp.entries.is_empty());
    let exact=vatra_service::registry::mcp_server(&mcp.entries[0].id,&mcp.entries[0].version).await.unwrap();
    assert_eq!(exact["name"],mcp.entries[0].id);assert_eq!(exact["version"],mcp.entries[0].version);
    let cursor = mcp
        .next_cursor
        .expect("registry contains more than two entries");
    let next = discover(
        "mcp",
        RegistryRequest {
            limit: Some(2),
            cursor: Some(cursor),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!next.entries.is_empty());
    assert_ne!(mcp.entries[0].id, next.entries[0].id);
}

#[test]
fn acp_discovery_preserves_distribution_and_does_not_claim_protocol_compatibility() {
    let data = json!({"version":"1.0.0", "agents":[
        {"id":"binary", "name":"Binary", "version":"1", "description":"An adapter",
         "authors":["Publisher"], "repository":"https://example.org/source",
         "distribution":{"binary":{"windows-x86_64":{"archive":"https://example.org/a.zip", "cmd":"agent.exe", "sha256":"abc"}}}},
        {"id":"package", "name":"Package", "version":"2", "distribution":{"npx":{"package":"@org/agent@2"}}},
        {"id":"linux", "name":"Linux", "version":"1", "distribution":{"binary":{"linux-x86_64":{"cmd":"agent"}}}}
    ]});
    let page = acp_page(data.clone(), "", "windows-x86_64", true).unwrap();
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.entries[0].publishers, ["Publisher"]);
    assert_eq!(
        page.entries[0].details["distribution"]["binary"]["windows-x86_64"]["cmd"],
        "agent.exe"
    );
    assert_eq!(
        acp_page(data, "PACKAGE", "windows-x86_64", false)
            .unwrap()
            .entries
            .len(),
        1
    );
}

#[test]
fn registry_rejects_invalid_envelopes_instead_of_showing_an_empty_success() {
    assert!(acp_page(json!({"agents": []}), "", "windows-x86_64", false).is_err());
    assert!(
        acp_page(
            json!({"version":"2.0.0", "agents": []}),
            "",
            "windows-x86_64",
            false
        )
        .is_err()
    );
    assert!(mcp_page(json!({"error":"upstream unavailable"})).is_err());
    assert!(mcp_page(json!({"servers":[{"server":{"name":"a"}}],"metadata":{}})).is_err());
}

#[test]
fn mcp_discovery_preserves_pagination_and_configuration_requirements() {
    let page = mcp_page(json!({"servers":[{"server":{
        "name":"io.example/server", "description":"Example", "version":"1.2",
        "repository":{"url":"https://example.org/repo"},
        "packages":[{"registryType":"npm", "identifier":"example", "environmentVariables":[{"name":"API_KEY", "isSecret":true}]}]
    },"_meta":{"io.modelcontextprotocol.registry/official":{"status":"active"}}}],"metadata":{"nextCursor":"io.example/server:1.2","count":1}})).unwrap();
    assert_eq!(page.next_cursor.as_deref(), Some("io.example/server:1.2"));
    assert_eq!(page.entries[0].publishers, ["io.example"]);
    assert_eq!(
        page.entries[0].details["packages"][0]["environmentVariables"][0]["isSecret"],
        true
    );
}
