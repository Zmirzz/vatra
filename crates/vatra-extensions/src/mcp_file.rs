use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde_json::{Map, Value};
use vatra_core::{McpOAuth, McpServerConfig, SecretRef};

use crate::FormatError;

/// Where a literal value was written: an environment variable, a header, or the client secret of OAuth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretPlace {
    Env,
    Header,
    OAuth,
}

/// A value written literally in an MCP file. The server keeps only its name, and the value waits in memory until
/// the user decides where it is stored.
#[derive(Clone, PartialEq)]
pub struct LiteralSecret {
    pub server: String,
    pub key: String,
    pub place: SecretPlace,
    value: String,
}

impl LiteralSecret {
    fn new(server: &str, key: &str, place: SecretPlace, value: String) -> LiteralSecret {
        LiteralSecret {
            server: server.to_string(),
            key: key.to_string(),
            place,
            value,
        }
    }

    /// The literal value. Only the code that stores it reads this; it never goes to logs, replies or reports.
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Debug for LiteralSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiteralSecret")
            .field("server", &self.server)
            .field("key", &self.key)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// What an MCP file gives: the servers that read, the literal values taken out of them, and one problem per server
/// that did not read. The other servers are kept.
#[derive(Debug, Default)]
pub struct McpFile {
    pub servers: Vec<McpServerConfig>,
    pub literal_secrets: Vec<LiteralSecret>,
    pub problems: Vec<String>,
}

/// Keys that only Codex writes. A server with one of them and a URL but no `type` is HTTP, not a mistake.
const CODEX_KEYS: [&str; 9] = [
    "bearer_token_env_var",
    "http_headers",
    "env_http_headers",
    "env_vars",
    "enabled",
    "enabled_tools",
    "disabled_tools",
    "startup_timeout_sec",
    "tool_timeout_sec",
];

/// Reads a `.mcp.json` file: the servers under `mcpServers`. Codex keys are read too, so a plugin copied from Codex
/// keeps its settings.
pub fn parse_mcp_json(text: &str) -> Result<McpFile, FormatError> {
    let value: Value = serde_json::from_str(text).map_err(|error| mcp(format!("invalid JSON: {error}")))?;
    match value.get("mcpServers") {
        Some(servers) => servers_from(servers, false),
        None => Ok(McpFile::default()),
    }
}

/// Reads a map of servers, from a file or inline in a plugin manifest.
pub(crate) fn servers_from_json(servers: &Value) -> Result<McpFile, FormatError> {
    servers_from(servers, false)
}

/// Reads the `[mcp_servers]` table of a Codex `config.toml`, keyed by server name.
pub fn parse_codex_mcp(table: &toml::Table) -> Result<McpFile, FormatError> {
    let value = serde_json::to_value(table).map_err(|error| mcp(format!("invalid TOML: {error}")))?;
    servers_from(&value, true)
}

/// Reads a map of servers one by one. A server that cannot be read becomes a problem, and the others are kept.
fn servers_from(servers: &Value, codex: bool) -> Result<McpFile, FormatError> {
    let servers = servers
        .as_object()
        .ok_or_else(|| mcp("`mcpServers` must be an object"))?;
    let mut file = McpFile::default();
    for (name, entry) in servers {
        match server_from(name, entry, codex) {
            Ok(Some((config, secrets))) => {
                file.servers.push(config);
                file.literal_secrets.extend(secrets);
            }
            Ok(None) => {}
            Err(error) => file.problems.push(error.to_string()),
        }
    }
    Ok(file)
}

/// One server, in either dialect. None when the server is disabled.
fn server_from(
    name: &str,
    entry: &Value,
    codex: bool,
) -> Result<Option<(McpServerConfig, Vec<LiteralSecret>)>, FormatError> {
    let object = entry
        .as_object()
        .ok_or_else(|| mcp(format!("server `{name}` must be an object")))?;
    let codex = codex || CODEX_KEYS.iter().any(|key| object.contains_key(*key));
    if !json_flag(object, "enabled", name)? {
        return Ok(None);
    }

    let mut secrets = Vec::new();
    let mut config = McpServerConfig {
        name: name.to_string(),
        ..Default::default()
    };
    config.command = json_text(object, &["command"], name)?;
    config.args = json_list(object, "args", name)?.unwrap_or_default();
    config.cwd = json_text(object, &["cwd"], name)?.map(PathBuf::from);
    config.url = json_text(object, &["url"], name)?;
    config.headers_helper = json_text(object, &["headersHelper"], name)?;
    config.enabled_tools = json_list(object, "enabled_tools", name)?;
    config.disabled_tools = json_list(object, "disabled_tools", name)?.unwrap_or_default();
    config.tool_timeout_sec = json_seconds(object, "tool_timeout_sec", name)?;
    config.startup_timeout_sec = json_seconds(object, "startup_timeout_sec", name)?;

    for (key, value) in json_pairs(object, "env", name)? {
        keep_or_report(&mut config.env, &mut secrets, name, SecretPlace::Env, key, value);
    }
    for variable in json_list(object, "env_vars", name)?.unwrap_or_default() {
        let reference = variable_reference(&mut config, &variable);
        config.env.insert(variable, reference);
    }

    for key in ["headers", "http_headers"] {
        for (header, value) in json_pairs(object, key, name)? {
            keep_or_report(&mut config.headers, &mut secrets, name, SecretPlace::Header, header, value);
        }
    }
    for (header, variable) in json_pairs(object, "env_http_headers", name)? {
        let reference = variable_reference(&mut config, &variable);
        config.headers.insert(header, reference);
    }
    if let Some(variable) = json_text(object, &["bearer_token_env_var"], name)? {
        let reference = variable_reference(&mut config, &variable);
        config
            .headers
            .insert("Authorization".to_string(), format!("Bearer {reference}"));
    }

    if let Some(oauth) = object.get("oauth") {
        let oauth = oauth
            .as_object()
            .ok_or_else(|| mcp(format!("`oauth` of server `{name}` must be an object")))?;
        config.oauth = Some(oauth_settings(oauth, name, &mut secrets)?);
    }

    check_transport(&config, object.get("type").and_then(Value::as_str), codex)?;
    config.validate().map_err(mcp)?;
    Ok(Some((config, secrets)))
}

fn oauth_settings(
    oauth: &Map<String, Value>,
    name: &str,
    secrets: &mut Vec<LiteralSecret>,
) -> Result<McpOAuth, FormatError> {
    let callback_port = match json_seconds(oauth, "callbackPort", name)?.or(json_seconds(oauth, "callback_port", name)?)
    {
        Some(port) => Some(
            u16::try_from(port).map_err(|_| mcp(format!("`callbackPort` of server `{name}` must be a port number")))?,
        ),
        None => None,
    };
    report_client_secret(
        name,
        json_text(oauth, &["clientSecret", "client_secret"], name)?,
        secrets,
    );
    Ok(McpOAuth {
        client_id: json_text(oauth, &["clientId", "client_id"], name)?,
        callback_port,
        auth_server_metadata_url: json_text(oauth, &["authServerMetadataUrl", "auth_server_metadata_url"], name)?,
        scopes: json_text(oauth, &["scopes"], name)?,
    })
}

/// Checks `type` against the fields it needs. Without a `type`, a URL is refused in the Claude form, because it could
/// be HTTP or SSE. Codex infers HTTP from the URL.
fn check_transport(config: &McpServerConfig, kind: Option<&str>, codex: bool) -> Result<(), FormatError> {
    let has_command = config.command.is_some();
    let has_url = config.url.is_some();
    let name = &config.name;
    match kind {
        None if has_url && !codex => Err(mcp(format!(
            "server `{name}` has a url but no type; set \"type\" to http or sse"
        ))),
        None => Ok(()),
        Some("stdio") if has_url => Err(mcp(format!("server `{name}` is stdio, so it can't have a url"))),
        Some("stdio") if !has_command => Err(mcp(format!("server `{name}` is stdio and needs a command"))),
        Some("stdio") => Ok(()),
        Some(kind @ ("http" | "streamable-http" | "sse")) if has_command => {
            Err(mcp(format!("server `{name}` is {kind}, so it can't have a command")))
        }
        Some(kind @ ("http" | "streamable-http" | "sse")) if !has_url => {
            Err(mcp(format!("server `{name}` is {kind} and needs a url")))
        }
        Some("http" | "streamable-http" | "sse") => Ok(()),
        Some(other) => Err(mcp(format!("server `{name}` has an unknown type `{other}`"))),
    }
}

/// A reference to a variable of the service, for the settings that Codex writes by name. The variable is registered
/// as a secret of the server, so the engine reads it from the environment.
fn variable_reference(config: &mut McpServerConfig, variable: &str) -> String {
    config
        .secrets
        .insert(variable.to_string(), SecretRef::Env(variable.to_string()));
    format!("${{{variable}}}")
}

/// Words in a setting's name that mark its literal value as a secret.
const SECRET_NAMES: [&str; 7] = ["key", "token", "secret", "password", "auth", "credential", "bearer"];

/// Prefixes of well-known token formats, which mark a literal value as a secret wherever it is.
const SECRET_PREFIXES: [&str; 3] = ["sk-", "ghp_", "xoxb-"];

/// Keeps a value that reads from a variable (`${NAME}`) or a plain literal value in the settings. A literal value is a
/// secret when its name says so or it looks like a token: it is taken out of the settings and reported by its key.
fn keep_or_report(
    kept: &mut BTreeMap<String, String>,
    secrets: &mut Vec<LiteralSecret>,
    server: &str,
    place: SecretPlace,
    key: String,
    value: String,
) {
    if value.contains("${") || !is_secret(&key, &value) {
        kept.insert(key, value);
    } else {
        secrets.push(LiteralSecret::new(server, &key, place, value));
    }
}

fn is_secret(name: &str, value: &str) -> bool {
    let name = name.to_lowercase();
    SECRET_NAMES.iter().any(|word| name.contains(word)) || looks_random(value)
}

/// A known token prefix, or a long value with no spaces that mixes letters and digits.
fn looks_random(value: &str) -> bool {
    if SECRET_PREFIXES.iter().any(|prefix| value.starts_with(prefix)) {
        return true;
    }
    value.chars().count() >= 32
        && !value.chars().any(char::is_whitespace)
        && value.chars().any(|c| c.is_ascii_alphabetic())
        && value.chars().any(|c| c.is_ascii_digit())
}

/// A literal client secret is reported. The OAuth settings have no field for it, so it is not stored.
fn report_client_secret(server: &str, secret: Option<String>, secrets: &mut Vec<LiteralSecret>) {
    if let Some(secret) = secret.filter(|secret| !secret.contains("${")) {
        secrets.push(LiteralSecret::new(server, "client_secret", SecretPlace::OAuth, secret));
    }
}

fn mcp(message: impl Into<String>) -> FormatError {
    FormatError::Mcp {
        message: message.into(),
    }
}

/// The text under the first of `keys` that is present.
fn json_text(object: &Map<String, Value>, keys: &[&str], server: &str) -> Result<Option<String>, FormatError> {
    let Some(key) = keys.iter().find(|key| object.contains_key(**key)) else {
        return Ok(None);
    };
    match object.get(*key) {
        Some(value) => value
            .as_str()
            .map(|text| Some(text.to_string()))
            .ok_or_else(|| mcp(format!("`{key}` of server `{server}` must be a string"))),
        None => Ok(None),
    }
}

fn json_flag(object: &Map<String, Value>, key: &str, server: &str) -> Result<bool, FormatError> {
    match object.get(key) {
        None => Ok(true),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| mcp(format!("`{key}` of server `{server}` must be true or false"))),
    }
}

fn json_seconds(object: &Map<String, Value>, key: &str, server: &str) -> Result<Option<u64>, FormatError> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or_else(|| {
            mcp(format!(
                "`{key}` of server `{server}` must be a whole number of seconds"
            ))
        }),
    }
}

fn json_list(object: &Map<String, Value>, key: &str, server: &str) -> Result<Option<Vec<String>>, FormatError> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_array()
            .and_then(|items| items.iter().map(|item| item.as_str().map(str::to_string)).collect())
            .map(Some)
            .ok_or_else(|| mcp(format!("`{key}` of server `{server}` must be a list of strings"))),
    }
}

fn json_pairs(object: &Map<String, Value>, key: &str, server: &str) -> Result<Vec<(String, String)>, FormatError> {
    let Some(value) = object.get(key) else {
        return Ok(Vec::new());
    };
    let pairs = value
        .as_object()
        .ok_or_else(|| mcp(format!("`{key}` of server `{server}` must be an object")))?;
    pairs
        .iter()
        .map(|(name, value)| {
            value
                .as_str()
                .map(|text| (name.clone(), text.to_string()))
                .ok_or_else(|| mcp(format!("`{key}.{name}` of server `{server}` must be a string")))
        })
        .collect()
}
