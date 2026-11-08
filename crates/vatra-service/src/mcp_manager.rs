//! Human-reviewed MCP configuration, backed by the existing catalog and keychain.
//! Previewing or saving never starts a command or connects to an external service.
use crate::Service;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use vatra_core::{AuditEntry, McpServerConfig, SecretRef, SecretStore};
use vatra_extensions::{CatalogEntry, ElementKind, Origin, Scope, SecretPlace, parse_mcp_json};

static WRITES: Mutex<()> = Mutex::new(());
const MAX_DOCUMENT: usize = 256 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistryOrigin {
    pub name: String,
    pub version: String,
    pub method: String,
    pub index: usize,
}

fn digest(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn scope(project: Option<i64>) -> Scope {
    project.map_or(Scope::Global, Scope::Project)
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPlan {
    pub config: McpServerConfig,
    pub secret_names: Vec<String>,
    pub review_hash: String,
    #[serde(skip)]
    values: BTreeMap<String, String>,
}

fn validate(config: &McpServerConfig) -> Result<(), String> {
    config.validate()?;
    if !valid_name(&config.name) {
        return Err(
            "Use a server name of at most 80 letters, digits, dots, underscores or dashes.".into(),
        );
    }
    if config.headers_helper.is_some() {
        return Err("Headers helper commands are not accepted by the installer. Use keychain-backed headers or OAuth.".into());
    }
    if config.args.len() > 128
        || config
            .args
            .iter()
            .any(|arg| arg.len() > 8192 || arg.contains('\0'))
    {
        return Err("MCP arguments are too large or invalid.".into());
    }
    for arg in &config.args {
        let lower = arg.to_ascii_lowercase();
        if [
            "api-key",
            "api_key",
            "token",
            "password",
            "secret",
            "authorization",
            "bearer",
        ]
        .iter()
        .any(|name| {
            lower.starts_with('-')
                && (lower.trim_start_matches('-') == *name
                    || lower
                        .trim_start_matches('-')
                        .starts_with(&format!("{name}=")))
        }) || ["sk-", "ghp_", "xoxb-"]
            .iter()
            .any(|prefix| lower.starts_with(prefix))
        {
            return Err("Keep credentials out of command arguments; use environment values or headers stored in the keychain.".into());
        }
    }
    if let Some(url) = &config.url {
        let url = url::Url::parse(url).map_err(|_| "Enter an absolute MCP HTTP URL.")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("MCP URLs must use HTTP(S), without embedded credentials, query parameters or fragments. Use secure headers for credentials.".into());
        }
        if url.scheme() == "http"
            && !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
        {
            return Err(
                "Remote MCP connections require HTTPS. HTTP is allowed only on loopback.".into(),
            );
        }
    }
    if config
        .command
        .as_ref()
        .is_some_and(|command| command.trim().is_empty() || command.contains('\0'))
    {
        return Err("Choose a valid stdio executable.".into());
    }
    if config.cwd.as_ref().is_some_and(|path| !path.is_absolute()) {
        return Err("MCP working directories must be absolute.".into());
    }
    for timeout in [config.tool_timeout_sec, config.startup_timeout_sec]
        .into_iter()
        .flatten()
    {
        if !(1..=86400).contains(&timeout) {
            return Err("MCP timeouts must be between 1 and 86400 seconds.".into());
        }
    }
    Ok(())
}

pub fn import_plans(document: &str, project_id: Option<i64>) -> Result<Vec<InstallPlan>, String> {
    if document.len() > MAX_DOCUMENT {
        return Err("MCP configuration exceeds 256 KiB.".into());
    }
    let input: Value =
        serde_json::from_str(document).map_err(|_| "MCP configuration must be valid JSON.")?;
    let raw = input["mcpServers"]
        .as_object()
        .ok_or("Expected an mcpServers object.")?;
    if raw.is_empty() || raw.len() > 50 {
        return Err("Import between 1 and 50 MCP servers at a time.".into());
    }
    // SSE-only servers cannot be silently relabelled Streamable HTTP.
    if raw.values().any(|server| server["type"] == "sse") {
        return Err(
            "SSE-only MCP servers are unsupported. Choose Streamable HTTP or stdio.".into(),
        );
    }
    let parsed = parse_mcp_json(document).map_err(|_| "MCP configuration could not be parsed.")?;
    if !parsed.problems.is_empty() {
        return Err(parsed.problems.join("\n"));
    }
    let mut plans = Vec::new();
    for mut config in parsed.servers {
        validate(&config)?;
        let mut values = BTreeMap::new();
        // Every literal environment/header value goes to the keychain, irrespective
        // of heuristic secret detection by the import parser.
        for (place, map) in [("env", &mut config.env), ("header", &mut config.headers)] {
            for (key, value) in map.iter_mut() {
                if !value.contains("${") {
                    let variable = format!(
                        "MCP_VALUE_{}",
                        &digest(format!("{place}:{key}").as_bytes())[..16]
                    );
                    values.insert(variable.clone(), value.clone());
                    *value = format!("${{{variable}}}");
                }
            }
        }
        for literal in parsed
            .literal_secrets
            .iter()
            .filter(|secret| secret.server == config.name)
        {
            let (place,map)=match literal.place {SecretPlace::Env=>("env",&mut config.env),SecretPlace::Header=>("header",&mut config.headers),SecretPlace::OAuth=>return Err("Use a public OAuth client with PKCE; client-secret authentication is unsupported here.".into())};
            let variable = format!(
                "MCP_VALUE_{}",
                &digest(format!("{place}:{}", literal.key).as_bytes())[..16]
            );
            values.insert(variable.clone(), literal.value().into());
            map.insert(literal.key.clone(), format!("${{{variable}}}"));
        }
        for (variable, value) in &values {
            config.secrets.insert(
                variable.clone(),
                SecretRef::Keychain(format!(
                    "vatra/mcp/{}/{}/{variable}/{}",
                    project_id.map_or_else(|| "global".into(), |id| id.to_string()),
                    config.name,
                    &digest(value.as_bytes())[..24]
                )),
            );
        }
        let review_hash = digest(
            &serde_json::to_vec(&json!({"config":config,"values":values,"project":project_id}))
                .map_err(|_| "Cannot prepare MCP review.")?,
        );
        plans.push(InstallPlan {
            secret_names: values.keys().cloned().collect(),
            config,
            values,
            review_hash,
        });
    }
    if plans.is_empty() {
        return Err("No enabled MCP configurations were found in this document.".into());
    }
    Ok(plans)
}

fn registry_values(items: &Value) -> Result<serde_json::Map<String, Value>, String> {
    let mut values = serde_json::Map::new();
    for item in items.as_array().into_iter().flatten() {
        let name = item["name"]
            .as_str()
            .ok_or("Registry configuration field is missing its name.")?;
        let value = item["value"]
            .as_str()
            .or(item["default"].as_str())
            .map(String::from)
            .or_else(|| (item["isRequired"] == true).then(|| format!("${{{name}}}")));
        if let Some(value) = value {
            let value = if value.contains('{') && !value.contains("${") {
                value.replace('{', "${")
            } else {
                value
            };
            values.insert(name.into(), Value::String(value));
        }
    }
    Ok(values)
}

/// Converts one advertised installation method; unsupported distributions fail
/// explicitly rather than generating a command with guessed semantics.
pub fn registry_document(
    server: &Value,
    name: &str,
    method: &str,
    index: usize,
) -> Result<Value, String> {
    if !valid_name(name) {
        return Err("Choose a valid local server name.".into());
    }
    let config = match method {
        "remote" => {
            let remote = server["remotes"]
                .as_array()
                .and_then(|list| list.get(index))
                .ok_or("Unknown remote distribution.")?;
            if remote["type"] != "streamable-http" {
                return Err("This remote is not Streamable HTTP.".into());
            }
            json!({"type":"http","url":remote["url"],"headers":registry_values(&remote["headers"])?})
        }
        "package" => {
            let package = server["packages"]
                .as_array()
                .and_then(|list| list.get(index))
                .ok_or("Unknown package distribution.")?;
            if package["registryType"] != "npm" || package["transport"]["type"] != "stdio" {
                return Err("This package requires manual configuration. Automatic configuration currently supports npm stdio packages.".into());
            }
            if package["registryBaseUrl"]
                .as_str()
                .is_some_and(|url| url.trim_end_matches('/') != "https://registry.npmjs.org")
            {
                return Err("Custom npm registries require manual configuration.".into());
            }
            let identifier = package["identifier"]
                .as_str()
                .ok_or("Missing npm package identifier.")?;
            let version = package["version"]
                .as_str()
                .ok_or("Missing exact npm version.")?;
            if identifier.is_empty()
                || identifier.starts_with('-')
                || !identifier
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"@/._-".contains(&c))
                || version == "latest"
                || !version
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._+-".contains(&c))
                || !version.starts_with(|c: char| c.is_ascii_digit())
            {
                return Err("Registry package identifier or exact version is invalid.".into());
            }
            let mut args = vec!["-y".to_string(), format!("{identifier}@{version}")];
            for argument in package["runtimeArguments"].as_array().into_iter().flatten() {
                if argument["value"] != "-y" {
                    return Err("Custom runtime arguments require manual configuration.".into());
                }
            }
            for argument in package["packageArguments"].as_array().into_iter().flatten() {
                if argument["isSecret"] == true {
                    return Err("Secret package arguments must be moved to secure environment configuration.".into());
                }
                let value = argument["value"].as_str().or(argument["default"].as_str());
                if let Some(value) = value {
                    if let Some(name) = argument["name"].as_str() {
                        args.push(name.into());
                    }
                    args.push(value.into());
                } else if argument["isRequired"] == true {
                    return Err("This package needs argument values. Use manual configuration after reviewing its requirements.".into());
                }
            }
            json!({"command":"npx","args":args,"env":registry_values(&package["environmentVariables"])?})
        }
        _ => return Err("Unknown registry installation method.".into()),
    };
    Ok(json!({"mcpServers":{name:config}}))
}

fn entry(service: &Service, id: i64) -> Result<CatalogEntry, String> {
    service
        .agents()
        .catalog()
        .ok_or("Extension catalog unavailable.")?
        .list()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|entry| entry.id == id && entry.kind == ElementKind::McpServer)
        .ok_or("Unknown installed MCP server.".into())
}

fn check_project(service: &Service, id: Option<i64>) -> Result<(), String> {
    if let Some(id) = id
        && !service
            .store()
            .list_projects()
            .map_err(|e| e.to_string())?
            .iter()
            .any(|project| project.id == id)
    {
        return Err("Unknown MCP project.".into());
    }
    Ok(())
}

fn document_for(config: &McpServerConfig) -> Value {
    let mut value = json!(config);
    let object = value.as_object_mut().expect("configuration object");
    object.remove("secrets");
    object.retain(|_, value| !value.is_null());
    if config.url.is_some() {
        value["type"] = json!("http");
    }
    json!({"mcpServers":{&config.name:value}})
}

fn retain_credentials(
    plans: &mut [InstallPlan],
    entries: &[CatalogEntry],
    project: Option<i64>,
) -> Result<(), String> {
    for plan in plans {
        let Some(entry) = entries.iter().find(|entry| {
            entry.kind == ElementKind::McpServer
                && entry.origin == Origin::Manual
                && entry.plugin.is_none()
                && entry.scope == scope(project)
                && entry.name == plan.config.name
        }) else {
            continue;
        };
        let old: McpServerConfig = serde_json::from_value(entry.details.clone())
            .map_err(|_| "Saved MCP configuration is invalid.")?;
        let same_destination = old.url == plan.config.url
            && old.command == plan.config.command
            && old.args == plan.config.args;
        for (name, reference) in &old.secrets {
            if plan
                .config
                .env
                .values()
                .chain(plan.config.headers.values())
                .any(|value| value.contains(&format!("${{{name}}}")))
                && !plan.values.contains_key(name)
            {
                if !same_destination {
                    return Err("The server destination changed. Re-enter its credential values before saving; saved credentials are not sent to a new destination.".into());
                }
                plan.config.secrets.insert(name.clone(), reference.clone());
            }
        }
        plan.review_hash = digest(
            &serde_json::to_vec(
                &json!({"config":plan.config,"values":plan.values,"project":project}),
            )
            .map_err(|_| "Cannot prepare MCP review.")?,
        );
    }
    Ok(())
}

/// Called only by the authenticated administrative UI.
pub fn configure(
    service: &Arc<Service>,
    method: &str,
    params: Value,
) -> Result<Value, (i64, String)> {
    let run = || -> Result<Value, String> {
        let _guard = WRITES
            .lock()
            .map_err(|_| "MCP configuration is unavailable.")?;
        match method {
            "mcp.preview" | "mcp.install" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct Request {
                    document: String,
                    project_id: Option<i64>,
                    review_hash: Option<String>,
                    replace_hash: Option<String>,
                    source: Option<RegistryOrigin>,
                }
                let request: Request = serde_json::from_value(params)
                    .map_err(|_| "Invalid MCP installation request.")?;
                check_project(service, request.project_id)?;
                if let Some(source) = &request.source
                    && (source.name.len() > 256
                        || source.version.len() > 128
                        || !matches!(source.method.as_str(), "remote" | "package"))
                {
                    return Err("Invalid registry source reference.".into());
                }
                let mut plans = import_plans(&request.document, request.project_id)?;
                let catalog = service
                    .agents()
                    .catalog()
                    .ok_or("Extension catalog unavailable.")?;
                let existing = catalog.list().map_err(|e| e.to_string())?;
                retain_credentials(&mut plans, &existing, request.project_id)?;
                if method == "mcp.preview" {
                    return Ok(json!({"servers":plans.iter().map(|plan|{
                        let old=existing.iter().find(|entry|entry.kind==ElementKind::McpServer&&entry.origin==Origin::Manual&&entry.plugin.is_none()&&entry.scope==scope(request.project_id)&&entry.name==plan.config.name);
                        json!({"plan":plan,"existing":old.map(|entry|json!({"id":entry.id,"hash":entry.content_hash}))})
                    }).collect::<Vec<_>>()}));
                }
                let plan = plans
                    .into_iter()
                    .find(|plan| Some(&plan.review_hash) == request.review_hash.as_ref())
                    .ok_or("Configuration changed since review. Preview it again.")?;
                if existing.iter().any(|entry| {
                    entry.kind == ElementKind::McpServer
                        && entry.origin == Origin::Manual
                        && entry.plugin.is_none()
                        && entry.scope == scope(request.project_id)
                        && entry.name == plan.config.name
                        && Some(&entry.content_hash) != request.replace_hash.as_ref()
                }) {
                    return Err("Server already exists. Review its current configuration before replacing it.".into());
                }
                service
                    .store()
                    .record_audit(&AuditEntry {
                        at: crate::models::now(),
                        session_id: None,
                        tool: "McpInstall".into(),
                        connector: "mcp".into(),
                        summary: format!("Save {} disabled", plan.config.name),
                        decision: "approved".into(),
                        payload_hash: plan.review_hash.clone(),
                    })
                    .map_err(|e| e.to_string())?;
                let secrets = service.agents().secrets();
                let mut backups = Vec::new();
                for (name, value) in &plan.values {
                    let Some(SecretRef::Keychain(key)) = plan.config.secrets.get(name) else {
                        return Err("Missing secure credential reference.".into());
                    };
                    let old = match secrets.get(key) {
                        Ok(old) => old,
                        Err(_) => {
                            rollback(secrets.as_ref(), &backups)?;
                            return Err(
                                "Couldn't read the OS keychain. No configuration was saved.".into(),
                            );
                        }
                    };
                    backups.push((key.clone(), old));
                    if secrets.set(key, value).is_err() {
                        rollback(secrets.as_ref(), &backups)?;
                        return Err("Couldn't store MCP credentials in the OS keychain. No configuration was saved.".into());
                    }
                }
                if catalog
                    .add_server(
                        &plan.config,
                        Origin::Manual,
                        scope(request.project_id),
                        None,
                        true,
                    )
                    .is_err()
                {
                    rollback(secrets.as_ref(), &backups)?;
                    return Err(
                        "Couldn't save MCP configuration. Previous credentials were restored."
                            .into(),
                    );
                }
                let saved = catalog
                    .list()
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .find(|entry| {
                        entry.kind == ElementKind::McpServer
                            && entry.origin == Origin::Manual
                            && entry.plugin.is_none()
                            && entry.scope == scope(request.project_id)
                            && entry.name == plan.config.name
                    })
                    .ok_or("Saved MCP server could not be located.")?;
                catalog
                    .set_enabled(saved.id, false)
                    .map_err(|e| e.to_string())?;
                let credentials_key = format!("mcp.credentials.{}", saved.id);
                let mut owned: Vec<String> = service
                    .store()
                    .app_setting(&credentials_key)
                    .map_err(|e| e.to_string())?
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or_default();
                owned.extend(plan.config.secrets.values().filter_map(
                    |reference| match reference {
                        SecretRef::Keychain(key) if key.starts_with("vatra/mcp/") => {
                            Some(key.clone())
                        }
                        _ => None,
                    },
                ));
                owned.sort();
                owned.dedup();
                service.store().set_app_setting(&credentials_key,Some(&serde_json::to_string(&owned).map_err(|_|"Cannot encode credential references.")?)).map_err(|_|"MCP configuration was saved, but credential cleanup metadata could not be stored.")?;
                service.agents().catalog_changed(request.project_id);
                let source = request
                    .source
                    .map(|source| serde_json::to_string(&source))
                    .transpose()
                    .map_err(|_| "Cannot encode registry source.")?;
                service.store().set_app_setting(&format!("mcp.source.{}",saved.id),source.as_deref()).map_err(|_|"MCP configuration was saved, but its registry reference could not be stored.")?;
                Ok(
                    json!({"id":saved.id,"hash":saved.content_hash,"requiresApproval":!saved.runs_code.is_empty(),"enabled":false}),
                )
            }
            "mcp.sources" => {
                let catalog = service
                    .agents()
                    .catalog()
                    .ok_or("Extension catalog unavailable.")?;
                let mut list = Vec::new();
                for item in catalog
                    .list()
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .filter(|item| item.kind == ElementKind::McpServer)
                {
                    if let Some(source) = service
                        .store()
                        .app_setting(&format!("mcp.source.{}", item.id))
                        .map_err(|e| e.to_string())?
                        && let Ok(source) = serde_json::from_str::<RegistryOrigin>(&source)
                    {
                        list.push(json!({"id":item.id,"alias":item.name,"source":source}));
                    }
                }
                Ok(json!(list))
            }
            "mcp.form" => {
                let item = entry(
                    service,
                    params["id"].as_i64().ok_or("Server id is required.")?,
                )?;
                let config: McpServerConfig = serde_json::from_value(item.details.clone())
                    .map_err(|_| "Saved MCP configuration is invalid.")?;
                validate(&config)?;
                // Never send legacy literal credentials back to the webview.
                if config
                    .env
                    .values()
                    .chain(config.headers.values())
                    .any(|value| !value.contains("${"))
                {
                    return Err("This legacy configuration contains literal values. Re-import its original configuration to migrate them into the keychain.".into());
                }
                let source = service
                    .store()
                    .app_setting(&format!("mcp.source.{}", item.id))
                    .map_err(|e| e.to_string())?
                    .and_then(|text| serde_json::from_str::<RegistryOrigin>(&text).ok());
                Ok(
                    json!({"document":serde_json::to_string_pretty(&document_for(&config)).map_err(|_|"Cannot format configuration.")?,"projectId":match item.scope{Scope::Global=>None,Scope::Project(id)=>Some(id)},"source":source}),
                )
            }
            "mcp.set_tools" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct Request {
                    id: i64,
                    expected_hash: String,
                    disabled_tools: Vec<String>,
                    enabled_tools: Option<Vec<String>>,
                }
                let request: Request =
                    serde_json::from_value(params).map_err(|_| "Invalid tool selection.")?;
                if !service.running_sessions().is_empty() {
                    return Err("Stop active turns before changing MCP tool access.".into());
                }
                let item = entry(service, request.id)?;
                if item.plugin.is_some() {
                    return Err("Configure plugin tools through their parent plugin.".into());
                }
                if item.content_hash != request.expected_hash {
                    return Err("Server changed. Reload before changing tools.".into());
                }
                if request.disabled_tools.len() > 2000
                    || request
                        .disabled_tools
                        .iter()
                        .any(|name| name.len() > 256 || name.is_empty())
                {
                    return Err("Invalid MCP tool names.".into());
                }
                let catalog = service
                    .agents()
                    .catalog()
                    .ok_or("Extension catalog unavailable.")?;
                let approved = catalog.is_approved(&item);
                let mut config: McpServerConfig = serde_json::from_value(item.details.clone())
                    .map_err(|_| "Saved MCP configuration is invalid.")?;
                config.disabled_tools = request.disabled_tools;
                if let Some(enabled)=request.enabled_tools {
                    if enabled.len()>2000||enabled.iter().any(|name|name.is_empty()||name.len()>256){return Err("Invalid MCP tool allowlist.".into());}
                    config.enabled_tools=Some(enabled);
                }
                service.store().record_audit(&AuditEntry{at:crate::models::now(),session_id:None,tool:"McpToolPolicy".into(),connector:"mcp".into(),summary:format!("Change tool access for {}",item.name),decision:"approved".into(),payload_hash:digest(&serde_json::to_vec(&config).map_err(|_|"Cannot encode MCP tool policy.")?)}).map_err(|e|e.to_string())?;
                catalog
                    .add_server(&config, item.origin, item.scope, None, false)
                    .map_err(|e| e.to_string())?;
                let saved = entry(service, item.id)?;
                if approved && !saved.runs_code.is_empty() {
                    catalog
                        .approve(saved.id, &saved.content_hash)
                        .map_err(|e| e.to_string())?;
                }
                service.agents().catalog_changed(match item.scope {
                    Scope::Global => None,
                    Scope::Project(id) => Some(id),
                });
                Ok(json!({"hash":saved.content_hash}))
            }
            "mcp.remove" => {
                if !service.running_sessions().is_empty() {
                    return Err("Stop active turns before removing MCP servers.".into());
                }
                let item = entry(
                    service,
                    params["id"].as_i64().ok_or("Server id is required.")?,
                )?;
                let catalog = service
                    .agents()
                    .catalog()
                    .ok_or("Extension catalog unavailable.")?;
                if item.plugin.is_some() {
                    return Err("Manage plugin servers through their parent plugin.".into());
                }
                if params["expectedHash"].as_str() != Some(&item.content_hash) {
                    return Err("Server changed since review. Reload before removing it.".into());
                }
                service
                    .store()
                    .record_audit(&AuditEntry {
                        at: crate::models::now(),
                        session_id: None,
                        tool: "McpUninstall".into(),
                        connector: "mcp".into(),
                        summary: format!("Remove {}", item.name),
                        decision: "approved".into(),
                        payload_hash: item.content_hash.clone(),
                    })
                    .map_err(|e| e.to_string())?;
                catalog.remove_entry(item.id).map_err(|e| e.to_string())?;
                service
                    .store()
                    .set_app_setting(&format!("mcp.source.{}", item.id), None)
                    .map_err(|e| e.to_string())?;
                let remaining = catalog.list().map_err(|e| e.to_string())?;
                let secrets = service.agents().secrets();
                let mut cleanup_failed = false;
                let credentials_key = format!("mcp.credentials.{}", item.id);
                let mut owned: Vec<String> = service
                    .store()
                    .app_setting(&credentials_key)
                    .map_err(|e| e.to_string())?
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or_default();
                if let Ok(config) = serde_json::from_value::<McpServerConfig>(item.details.clone())
                {
                    owned.extend(
                        config
                            .secrets
                            .values()
                            .filter_map(|reference| match reference {
                                SecretRef::Keychain(key) => Some(key.clone()),
                                _ => None,
                            }),
                    );
                }
                owned.sort();
                owned.dedup();
                let owner_prefix = format!(
                    "vatra/mcp/{}/{}/",
                    match item.scope {
                        Scope::Global => "global".into(),
                        Scope::Project(id) => id.to_string(),
                    },
                    item.name
                );
                for key in &owned {
                    if !key.starts_with(&owner_prefix)
                        && !key.starts_with(&format!("extension:{}:", item.id))
                    {
                        continue;
                    }
                    let used = remaining.iter().any(|entry| {
                        entry.details["secrets"]
                            .as_object()
                            .is_some_and(|references| {
                                references
                                    .values()
                                    .any(|value| value["keychain"].as_str() == Some(key))
                            })
                    }) || service.agents().mcp_servers().iter().any(|server| {
                        server
                            .secrets
                            .values()
                            .any(|reference| reference == &SecretRef::Keychain(key.clone()))
                    });
                    if !used && secrets.delete(key).is_err() {
                        cleanup_failed = true;
                    }
                }
                if !cleanup_failed {
                    service
                        .store()
                        .set_app_setting(&credentials_key, None)
                        .map_err(|e| e.to_string())?;
                }
                service.agents().catalog_changed(match item.scope {
                    Scope::Global => None,
                    Scope::Project(id) => Some(id),
                });
                Ok(json!({"removed":true,"credentialCleanupFailed":cleanup_failed}))
            }
            _ => Err("Unknown MCP configuration operation.".into()),
        }
    };
    run().map_err(|message| (-32000, message))
}

pub async fn runtime(
    service: &Arc<Service>,
    method: &str,
    params: Value,
) -> Result<Value, (i64, String)> {
    async fn run(service: &Arc<Service>, method: &str, params: Value) -> Result<Value, String> {
        let id = params["id"].as_i64().ok_or("Server id is required.")?;
        let item = entry(service, id)?;
        let config: McpServerConfig = serde_json::from_value(item.details.clone())
            .map_err(|_| "Saved MCP configuration is invalid.")?;
        let catalog = service
            .agents()
            .catalog()
            .ok_or("Extension catalog unavailable.")?;
        if method == "mcp.login_installed" {
            service
                .start_mcp_login_config(config)
                .map_err(|e| e.to_string())?;
            return Ok(json!({}));
        }
        if !catalog.is_approved(&item) {
            return Err(
                "Review and approve this server's command before connecting or testing it.".into(),
            );
        }
        if method == "mcp.test" {
            return service.agents().probe_mcp(config).await;
        }
        let project=params["projectId"].as_i64().ok_or("Choose a project to manage its live MCP connection. Test Connection works without a project.")?;
        check_project(service, Some(project))?;
        if let Scope::Project(owner) = item.scope
            && owner != project
        {
            return Err("This MCP server belongs to another project.".into());
        }
        let known = service
            .store()
            .list_projects()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|known| known.id == project)
            .ok_or("Unknown project.")?;
        let hub = service
            .agents()
            .mcp_hub(project, known.path.as_deref().map(std::path::Path::new))
            .ok_or("Project MCP runtime is unavailable.")?;
        if hub.server_status(&config.name).is_none() {
            return Err("Enable this server and reload the project configuration first.".into());
        }
        match method {
            "mcp.connect" | "mcp.reconnect" => {
                hub.reconnect(&config.name).await;
            }
            "mcp.disconnect" => hub.disconnect(&config.name).await,
            "mcp.connection" => {}
            _ => return Err("Unknown MCP connection operation.".into()),
        }
        hub.server_status(&config.name)
            .ok_or("MCP connection is unavailable.".into())
    }
    run(service, method, params)
        .await
        .map_err(|message| (-32000, message))
}

fn rollback(secrets: &dyn SecretStore, backups: &[(String, Option<String>)]) -> Result<(), String> {
    for (key, value) in backups.iter().rev() {
        let result = match value {
            Some(value) => secrets.set(key, value),
            None => secrets.delete(key),
        };
        if result.is_err() {
            return Err("MCP save failed and a previous credential could not be restored. Reconfigure this server's credentials.".into());
        }
    }
    Ok(())
}
