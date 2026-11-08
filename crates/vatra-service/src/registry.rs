//! Official ecosystem discovery and installation. Remote metadata is untrusted data;
//! fetching an entry never authenticates or starts an agent. Installation writes
//! reviewed configuration and downloaded binaries only under the service data directory.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use zip::ZipArchive;

use crate::Service;
use crate::mcp_manager;
use vatra_core::node_package_runner;

pub const ACP_INDEX: &str = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
pub const MCP_INDEX: &str = "https://registry.modelcontextprotocol.io/v0.1/servers";
const MAX_BYTES: usize = 16 * 1024 * 1024;
const CACHE_TTL: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub publishers: Vec<String>,
    pub repository: Option<String>,
    /// True means a distribution is offered, not that it has passed a handshake.
    pub platform_available: bool,
    /// Complete registry requirements for a later explicit installation review.
    pub details: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryPage {
    pub entries: Vec<RegistryEntry>,
    pub next_cursor: Option<String>,
    pub source: String,
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("Registry entry is missing `{key}`."))
}

pub fn acp_page(
    data: Value,
    query: &str,
    platform: &str,
    compatible_only: bool,
) -> Result<RegistryPage, String> {
    if data["version"].as_str() != Some("1.0.0") {
        return Err("Unsupported ACP registry format. Expected version 1.0.0.".into());
    }
    let agents = data["agents"]
        .as_array()
        .ok_or("ACP registry is missing its agents array.")?;
    let query = query.to_lowercase();
    let mut entries = Vec::new();
    for agent in agents {
        let id = text(agent, "id")?.to_string();
        let name = text(agent, "name")?.to_string();
        let version = text(agent, "version")?.to_string();
        let description = agent["description"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let publishers: Vec<String> = agent["authors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect();
        let distribution = &agent["distribution"];
        let platform_available = distribution["binary"][platform].is_object()
            || distribution["npx"].is_object()
            || distribution["uvx"].is_object();
        if compatible_only && !platform_available {
            continue;
        }
        if !format!("{id} {name} {description} {}", publishers.join(" "))
            .to_lowercase()
            .contains(&query)
        {
            continue;
        }
        entries.push(RegistryEntry {
            id,
            name,
            version,
            description,
            publishers,
            repository: agent["repository"].as_str().map(String::from),
            platform_available,
            details: agent.clone(),
        });
    }
    Ok(RegistryPage {
        entries,
        next_cursor: None,
        source: ACP_INDEX.into(),
    })
}

pub fn mcp_page(data: Value) -> Result<RegistryPage, String> {
    let servers = data["servers"]
        .as_array()
        .ok_or("MCP registry is missing its servers array.")?;
    if !data["metadata"].is_object() {
        return Err("MCP registry is missing pagination metadata.".into());
    }
    let mut entries = Vec::new();
    for wrapper in servers {
        let server = &wrapper["server"];
        let id = text(server, "name")?.to_string();
        let version = text(server, "version")?.to_string();
        entries.push(RegistryEntry {
            name: server["title"].as_str().unwrap_or(&id).to_string(),
            publishers: vec![id.split('/').next().unwrap_or(&id).to_string()],
            description: server["description"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            repository: server["repository"]["url"].as_str().map(String::from),
            platform_available: true,
            id,
            version,
            details: server.clone(),
        });
    }
    let next_cursor = match &data["metadata"]["nextCursor"] {
        Value::Null => None,
        Value::String(cursor) if cursor.len() <= 2048 => Some(cursor.clone()),
        _ => return Err("MCP registry returned invalid pagination metadata.".into()),
    };
    Ok(RegistryPage {
        entries,
        next_cursor,
        source: MCP_INDEX.into(),
    })
}

pub fn host_platform() -> String {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else {
        std::env::consts::OS
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistryRequest {
    #[serde(default)]
    pub query: String,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub platform_only: bool,
}

type AcpCache = Mutex<Option<(Instant, Value)>>;
static ACP_CACHE: OnceLock<AcpCache> = OnceLock::new();

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("Vatra/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "Couldn't initialize the registry client.".into())
}

async fn fetch(request: reqwest::RequestBuilder) -> Result<Value, String> {
    let mut response = request
        .send()
        .await
        .map_err(|_| "Couldn't reach the official registry. Check your connection.")?;
    if !response.status().is_success() {
        return Err(format!(
            "Official registry returned HTTP {}.",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_BYTES as u64)
    {
        return Err("Registry response is too large.".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Registry response was interrupted.")?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
            return Err("Registry response is too large.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "Registry returned invalid JSON.".into())
}

/// Fixed HTTPS origins and encoded query parameters; no registry-supplied URL is fetched.
pub async fn discover(ecosystem: &str, request: RegistryRequest) -> Result<RegistryPage, String> {
    if request.query.len() > 256 || request.cursor.as_ref().is_some_and(|s| s.len() > 2048) {
        return Err("Registry search or cursor is too long.".into());
    }
    match ecosystem {
        "acp" => {
            let mut cache = ACP_CACHE.get_or_init(|| Mutex::new(None)).lock().await;
            if cache
                .as_ref()
                .is_none_or(|(at, _)| at.elapsed() >= CACHE_TTL)
            {
                let data = fetch(client()?.get(ACP_INDEX)).await?;
                acp_page(data.clone(), "", &host_platform(), false)?;
                *cache = Some((Instant::now(), data));
            }
            acp_page(
                cache.as_ref().expect("cache populated").1.clone(),
                &request.query,
                &host_platform(),
                request.platform_only,
            )
        }
        "mcp" => {
            let limit = request.limit.unwrap_or(30);
            if !(1..=100).contains(&limit) {
                return Err("Registry page size must be between 1 and 100.".into());
            }
            let mut params = vec![("limit", limit.to_string()), ("version", "latest".into())];
            if !request.query.trim().is_empty() {
                params.push(("search", request.query));
            }
            if let Some(cursor) = request.cursor {
                params.push(("cursor", cursor));
            }
            let mut url = url::Url::parse(MCP_INDEX).expect("fixed registry URL");
            url.query_pairs_mut().extend_pairs(params);
            mcp_page(fetch(client()?.get(url)).await?)
        }
        _ => Err("Unknown registry ecosystem.".into()),
    }
}

/// Exact-version metadata from the official registry; path segments are encoded
/// separately, so a name cannot replace the fixed registry origin or API path.
pub async fn mcp_server(name:&str,version:&str)->Result<Value,String>{
    if name.is_empty()||name.len()>256||version.is_empty()||version.len()>128{return Err("Invalid registry server name or version.".into());}
    let mut url=url::Url::parse(MCP_INDEX).expect("fixed registry URL");
    url.path_segments_mut().map_err(|_|"Invalid registry URL.")?.push(name).push("versions").push(version);
    let data=fetch(client()?.get(url)).await?;
    if data["server"]["name"]!=name||data["server"]["version"]!=version{return Err("Registry metadata does not match the requested server version.".into());}
    Ok(data["server"].clone())
}

/// Installs an ACP agent from the official ACP registry into the harness.
/// Resolves the distribution (npx, uvx or binary), downloads and verifies binaries when needed,
/// then saves the agent configuration exactly like `agents.save`.
pub async fn acp_install(
    service: &Arc<Service>,
    id: &str,
    version: &str,
    alias: &str,
) -> Result<Value, String> {
    let data = {
        let mut cache = ACP_CACHE.get_or_init(|| Mutex::new(None)).lock().await;
        if cache.as_ref().is_none_or(|(at, _)| at.elapsed() >= CACHE_TTL) {
            let data = fetch(client()?.get(ACP_INDEX)).await?;
            acp_page(data.clone(), "", &host_platform(), false)?;
            *cache = Some((Instant::now(), data));
        }
        cache.as_ref().expect("cache populated").1.clone()
    };

    let agents = data["agents"].as_array().ok_or("ACP registry is missing its agents array.")?;
    let agent = agents.iter().find(|a| a["id"].as_str() == Some(id))
        .ok_or_else(|| format!("Agent `{id}` not found in ACP registry."))?;

    let agent_version = agent["version"].as_str().unwrap_or("");
    if !version.is_empty() && agent_version != version {
        return Err(format!("Agent `{id}` has version `{agent_version}`, requested `{version}`."));
    }

    let distribution = &agent["distribution"];
    let platform = host_platform();

    let (binary, args): (PathBuf, Vec<String>) = if let Some(npx) = distribution["npx"].as_object() {
        let package = npx["package"].as_str().ok_or("Missing npx package in distribution.")?;
        let path = std::env::var_os("PATH").unwrap_or_default();
        let (node, native_args) = node_package_runner("npx", &[package.to_string()], &path)?
            .ok_or_else(|| format!("Couldn't resolve npx runner for `{id}`."))?;
        (node, native_args)
    } else if let Some(uvx) = distribution["uvx"].as_object() {
        let package = uvx["package"].as_str().unwrap_or(id);
        (PathBuf::from("uvx"), vec![package.to_string()])
    } else if let Some(binary_dist) = distribution["binary"][&platform].as_object() {
        let archive_url = binary_dist["archive"].as_str()
            .ok_or("Binary distribution missing archive URL.")?;
        let cmd_name = binary_dist["cmd"].as_str()
            .ok_or("Binary distribution missing cmd.")?;
        let expected_hash = binary_dist["sha256"].as_str().unwrap_or("");

        let download_dir = std::env::temp_dir().join("vatra-registry-downloads");
        fs::create_dir_all(&download_dir).map_err(|e| e.to_string())?;
        let archive_path = download_dir.join(format!("{id}-{agent_version}.zip"));

        let mut response = client()?.get(archive_url).send().await
            .map_err(|_| "Failed to download agent archive.")?;
        if !response.status().is_success() {
            return Err(format!("Failed to download agent archive: HTTP {}.", response.status().as_u16()));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "Download interrupted.")? {
            bytes.extend_from_slice(&chunk);
        }

        if !expected_hash.is_empty() {
            let actual_hash = format!("{:x}", Sha256::digest(&bytes));
            if !actual_hash.eq_ignore_ascii_case(expected_hash) {
                return Err("Downloaded agent archive SHA256 does not match registry.".into());
            }
        }

        fs::write(&archive_path, &bytes).map_err(|e| e.to_string())?;
        let extract_dir = download_dir.join(format!("{id}-{agent_version}-extracted"));
        fs::create_dir_all(&extract_dir).map_err(|e| e.to_string())?;

        let file = fs::File::open(&archive_path).map_err(|e| e.to_string())?;
        let mut archive = ZipArchive::new(file).map_err(|e| format!("Invalid zip archive: {e}"))?;
        archive.extract(&extract_dir).map_err(|e| format!("Failed to extract archive: {e}"))?;

        let binary_path = extract_dir.join(cmd_name);
        if !binary_path.is_file() {
            return Err(format!("Expected binary `{cmd_name}` not found in archive."));
        }

        (binary_path, vec![])
    } else {
        return Err("No installable distribution for this agent on this platform.".into());
    };

    let form = crate::setup::AgentForm::Acp {
        name: alias.to_string(),
        node: binary,
        script: None,
        args,
        startup_timeout_sec: Some(30),
        turn_timeout_sec: Some(900),
        subscription: false,
        managed_account: false,
        codex_binary: None,
    };

    let now = crate::models::now();
    let (row, key) = crate::setup::agent_from_form(form, now).map_err(|e| e.to_string())?;
    let file_agents = service.agents().file_agent_names().unwrap_or_default();
    crate::setup::check_save(service.store(), service.agents().secrets().as_ref(), &file_agents, &row, key.as_deref())
        .map_err(|e| e.to_string())?;
    crate::setup::save(service.store(), service.agents().secrets().as_ref(), &row, key.as_deref())
        .map_err(|e| e.to_string())?;

    Ok(json!({"name": row.name, "restartNeeded": true}))
}

/// Installs an MCP server from the official MCP registry into the harness.
/// Fetches server metadata, generates a configuration document, reviews it and installs it.
pub async fn mcp_install(
    service: &Arc<Service>,
    name: &str,
    version: &str,
    alias: &str,
    method: &str,
    index: usize,
) -> Result<Value, String> {
    let server = mcp_server(name, version).await.map_err(|e| e.to_string())?;
    let document = mcp_manager::registry_document(&server, alias, method, index).map_err(|e| e.to_string())?;
    let document_str = serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?;

    let preview = mcp_manager::configure(service, "mcp.preview", serde_json::json!({
        "document": document_str,
        "projectId": serde_json::Value::Null,
    })).map_err(|e| e.1)?;

    let review_hash = preview["servers"][0]["plan"]["reviewHash"].as_str()
        .ok_or("Missing review hash from preview.")?;

    let saved = mcp_manager::configure(service, "mcp.install", serde_json::json!({
        "document": document_str,
        "projectId": serde_json::Value::Null,
        "reviewHash": review_hash,
    })).map_err(|e| e.1)?;

    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn registry_http_rejects_redirects_and_does_not_fetch_the_target() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/registry"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}/private", server.uri())),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/private"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        assert!(
            fetch(client().unwrap().get(format!("{}/registry", server.uri())))
                .await
                .unwrap_err()
                .contains("302")
        );
    }

    #[tokio::test]
    async fn registry_http_validates_response_and_bounds_content() {
        let server = MockServer::start().await;
        Mock::given(path("/invalid"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>not JSON</html>"))
            .mount(&server)
            .await;
        Mock::given(path("/oversized"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b' '; MAX_BYTES + 1]))
            .mount(&server)
            .await;
        assert!(
            fetch(client().unwrap().get(format!("{}/invalid", server.uri())))
                .await
                .unwrap_err()
                .contains("invalid JSON")
        );
        assert!(
            fetch(client().unwrap().get(format!("{}/oversized", server.uri())))
                .await
                .unwrap_err()
                .contains("too large")
        );
    }

    #[tokio::test]
    async fn discovery_validates_user_input_before_network() {
        assert!(
            discover(
                "mcp",
                RegistryRequest {
                    limit: Some(0),
                    ..Default::default()
                }
            )
            .await
            .is_err()
        );
        assert!(
            discover(
                "mcp",
                RegistryRequest {
                    cursor: Some("x".repeat(2049)),
                    ..Default::default()
                }
            )
            .await
            .is_err()
        );
        assert!(
            discover(
                "acp",
                RegistryRequest {
                    query: "x".repeat(257),
                    ..Default::default()
                }
            )
            .await
            .is_err()
        );
    }
}
