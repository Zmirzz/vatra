use vatra_core::{McpServerConfig, SecretRef};
use vatra_extensions::{LiteralSecret, McpFile, parse_codex_mcp, parse_mcp_json};

/// Reads JSON that must have no problems, and returns the servers and literal secrets.
fn json(text: &str) -> (Vec<McpServerConfig>, Vec<LiteralSecret>) {
    let file = parse_mcp_json(text).unwrap();
    assert!(file.problems.is_empty(), "{:?}", file.problems);
    (file.servers, file.literal_secrets)
}

fn codex(table: &toml::Table) -> (Vec<McpServerConfig>, Vec<LiteralSecret>) {
    let file: McpFile = parse_codex_mcp(table).unwrap();
    assert!(file.problems.is_empty(), "{:?}", file.problems);
    (file.servers, file.literal_secrets)
}

fn server<'a>(servers: &'a [McpServerConfig], name: &str) -> &'a McpServerConfig {
    servers.iter().find(|server| server.name == name).unwrap()
}

#[test]
fn claude_stdio_http_sse() {
    let (servers, secrets) = json(
        r#"{"mcpServers": {
            "local": {"command": "node", "args": ["server.js"], "env": {"MODE": "${MODE}"}, "cwd": "tools"},
            "remote": {"type": "http", "url": "https://example.com/mcp", "headers": {"X-Team": "${TEAM}"}},
            "legacy": {"type": "sse", "url": "https://example.com/sse"}
        }}"#,
    );
    assert_eq!(servers.len(), 3);

    let local = server(&servers, "local");
    assert_eq!(local.command.as_deref(), Some("node"));
    assert_eq!(local.args, vec!["server.js"]);
    assert_eq!(local.env.get("MODE").map(String::as_str), Some("${MODE}"));
    assert_eq!(local.cwd.as_deref(), Some(std::path::Path::new("tools")));
    assert!(local.url.is_none());

    let remote = server(&servers, "remote");
    assert_eq!(remote.url.as_deref(), Some("https://example.com/mcp"));
    assert_eq!(remote.headers.get("X-Team").map(String::as_str), Some("${TEAM}"));
    assert!(remote.command.is_none());

    assert_eq!(
        server(&servers, "legacy").url.as_deref(),
        Some("https://example.com/sse")
    );
    assert!(secrets.is_empty());
}

#[test]
fn url_without_type_is_problem() {
    let file = parse_mcp_json(
        r#"{"mcpServers": {"remote": {"url": "https://example.com/mcp"}, "local": {"command": "node"}}}"#,
    )
    .unwrap();
    assert_eq!(file.problems.len(), 1, "{:?}", file.problems);
    assert!(file.problems[0].contains("remote"), "{:?}", file.problems);
    let names: Vec<&str> = file.servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["local"]);
}

#[test]
fn type_must_match_fields() {
    let file = parse_mcp_json(
        r#"{"mcpServers": {
            "a": {"type": "stdio", "url": "https://example.com/mcp"},
            "b": {"type": "stdio"},
            "c": {"type": "http", "command": "node"},
            "d": {"type": "http"},
            "e": {"type": "carrier-pigeon", "command": "node"},
            "ok": {"type": "stdio", "command": "node"}
        }}"#,
    )
    .unwrap();
    assert_eq!(file.problems.len(), 5, "{:?}", file.problems);
    let names: Vec<&str> = file.servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["ok"]);
}

#[test]
fn codex_keys_in_plugin_mcp_json() {
    let file = parse_mcp_json(
        r#"{"mcpServers": {
            "notes": {
                "command": "notes-server",
                "env_vars": ["NOTES_TOKEN"],
                "enabled_tools": ["search"],
                "startup_timeout_sec": 10,
                "tool_timeout_sec": 30
            },
            "docs": {"url": "https://docs.example.com/mcp", "bearer_token_env_var": "DOCS_TOKEN"},
            "off": {"command": "old-server", "enabled": false}
        }}"#,
    )
    .unwrap();
    assert!(file.problems.is_empty(), "{:?}", file.problems);
    let names: Vec<&str> = file.servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["docs", "notes"]);

    let notes = server(&file.servers, "notes");
    assert_eq!(notes.env.get("NOTES_TOKEN").map(String::as_str), Some("${NOTES_TOKEN}"));
    assert_eq!(
        notes.secrets.get("NOTES_TOKEN"),
        Some(&SecretRef::Env("NOTES_TOKEN".into()))
    );
    assert_eq!(notes.enabled_tools, Some(vec!["search".to_string()]));
    assert_eq!(notes.startup_timeout_sec, Some(10));
    assert_eq!(notes.tool_timeout_sec, Some(30));

    let docs = server(&file.servers, "docs");
    assert_eq!(
        docs.headers.get("Authorization").map(String::as_str),
        Some("Bearer ${DOCS_TOKEN}")
    );
    assert_eq!(
        docs.secrets.get("DOCS_TOKEN"),
        Some(&SecretRef::Env("DOCS_TOKEN".into()))
    );
    assert!(file.literal_secrets.is_empty());
}

#[test]
fn streamable_http_alias() {
    let (servers, _) =
        json(r#"{"mcpServers": {"remote": {"type": "streamable-http", "url": "https://example.com/mcp"}}}"#);
    assert_eq!(servers[0].url.as_deref(), Some("https://example.com/mcp"));
}

#[test]
fn codex_all_keys() {
    let table: toml::Table = toml::from_str(
        r#"
        [docs]
        url = "https://docs.example.com/mcp"
        bearer_token_env_var = "DOCS_TOKEN"
        http_headers = { "X-Plan-Secret" = "team" }
        env_http_headers = { "X-Org" = "DOCS_ORG" }
        enabled_tools = ["search"]
        disabled_tools = ["delete"]
        tool_timeout_sec = 30
        startup_timeout_sec = 10
        enabled = true

        [docs.oauth]
        client_id = "abc"
        callback_port = 8765
        scopes = "read"

        [local]
        command = "npx"
        args = ["-y", "tool"]
        env_vars = ["HOME_TOKEN"]
        cwd = "work"
        "#,
    )
    .unwrap();
    let (servers, secrets) = codex(&table);

    let docs = server(&servers, "docs");
    assert_eq!(docs.url.as_deref(), Some("https://docs.example.com/mcp"));
    assert_eq!(
        docs.headers.get("Authorization").map(String::as_str),
        Some("Bearer ${DOCS_TOKEN}")
    );
    assert_eq!(docs.headers.get("X-Org").map(String::as_str), Some("${DOCS_ORG}"));
    assert_eq!(docs.enabled_tools, Some(vec!["search".to_string()]));
    assert_eq!(docs.disabled_tools, vec!["delete"]);
    assert_eq!(docs.tool_timeout_sec, Some(30));
    assert_eq!(docs.startup_timeout_sec, Some(10));
    let oauth = docs.oauth.as_ref().unwrap();
    assert_eq!(oauth.client_id.as_deref(), Some("abc"));
    assert_eq!(oauth.callback_port, Some(8765));
    assert_eq!(oauth.scopes.as_deref(), Some("read"));
    assert!(!docs.headers.contains_key("X-Plan-Secret"));

    let local = server(&servers, "local");
    assert_eq!(local.command.as_deref(), Some("npx"));
    assert_eq!(local.args, vec!["-y", "tool"]);
    assert_eq!(local.env.get("HOME_TOKEN").map(String::as_str), Some("${HOME_TOKEN}"));
    assert_eq!(local.cwd.as_deref(), Some(std::path::Path::new("work")));

    assert_eq!(secrets.len(), 1);
    assert_eq!(secrets[0].server, "docs");
    assert_eq!(secrets[0].key, "X-Plan-Secret");
    assert_eq!(secrets[0].value(), "team");
}

#[test]
fn codex_disabled_server_is_skipped() {
    let table: toml::Table = toml::from_str(
        r#"
        [off]
        command = "npx"
        enabled = false
        "#,
    )
    .unwrap();
    let (servers, _) = codex(&table);
    assert!(servers.is_empty());
}

#[test]
fn literal_secret_reported_by_key_only() {
    let (servers, secrets) = json(
        r#"{"mcpServers": {"x": {
            "command": "tool",
            "env": {"API_KEY": "sk-live-SECRET123"},
            "headers": {"Authorization": "Bearer SECRET456"},
            "oauth": {"clientId": "id", "clientSecret": "SECRET789"}
        }}}"#,
    );
    let config = &servers[0];
    assert!(!config.env.contains_key("API_KEY"));
    assert!(!config.headers.contains_key("Authorization"));

    let debug = format!("{config:?}");
    let serialized = serde_json::to_string(config).unwrap();
    for text in [&debug, &serialized] {
        assert!(!text.contains("SECRET123"), "{text}");
        assert!(!text.contains("SECRET456"), "{text}");
        assert!(!text.contains("SECRET789"), "{text}");
    }

    let keys: Vec<(&str, &str)> = secrets.iter().map(|s| (s.server.as_str(), s.key.as_str())).collect();
    assert_eq!(
        keys,
        vec![("x", "API_KEY"), ("x", "Authorization"), ("x", "client_secret")]
    );
    assert_eq!(secrets[0].value(), "sk-live-SECRET123");

    let secret_debug = format!("{:?}", secrets);
    assert!(!secret_debug.contains("SECRET123"), "{secret_debug}");
    assert!(!secret_debug.contains("SECRET789"), "{secret_debug}");
}

#[test]
fn plain_values_stay_on_the_server() {
    let (servers, secrets) = json(
        r#"{"mcpServers": {"x": {
            "command": "tool",
            "env": {"LOG_LEVEL": "debug"},
            "headers": {"X-Region": "eu-west-1"}
        }}}"#,
    );

    assert!(secrets.is_empty());
    assert_eq!(servers[0].env.get("LOG_LEVEL").map(String::as_str), Some("debug"));
    assert_eq!(
        servers[0].headers.get("X-Region").map(String::as_str),
        Some("eu-west-1")
    );
}

#[test]
fn named_and_random_values_are_secrets() {
    let (servers, secrets) = json(
        r#"{"mcpServers": {"x": {
            "command": "tool",
            "env": {
                "API_TOKEN": "abc",
                "PLAIN": "sk-abc",
                "DATA": "aB3dE5gH7jK9mN1pQ3rS5tU7vW9yZ1bD",
                "WORDS": "onlyletterssonlyletterssonlyletters"
            }
        }}}"#,
    );

    let keys: Vec<&str> = secrets.iter().map(|secret| secret.key.as_str()).collect();
    assert_eq!(keys, vec!["API_TOKEN", "DATA", "PLAIN"]);
    assert_eq!(
        servers[0].env.get("WORDS").map(String::as_str),
        Some("onlyletterssonlyletterssonlyletters")
    );
    assert!(!servers[0].env.contains_key("API_TOKEN"));
}

#[test]
fn secret_value_stays_out_of_errors() {
    let file = parse_mcp_json(r#"{"mcpServers": {"x": {"env": {"API_KEY": "SECRET123"}}}}"#).unwrap();
    assert_eq!(file.problems.len(), 1, "{:?}", file.problems);
    assert!(!file.problems[0].contains("SECRET123"), "{:?}", file.problems);
    assert!(file.servers.is_empty());
    assert!(file.literal_secrets.is_empty());
}

#[test]
fn env_reference_is_not_secret() {
    let (servers, secrets) = json(r#"{"mcpServers": {"x": {"command": "tool", "env": {"TOKEN": "${MY_TOKEN}"}}}}"#);
    assert_eq!(servers[0].env.get("TOKEN").map(String::as_str), Some("${MY_TOKEN}"));
    assert!(secrets.is_empty());
}

#[test]
fn invalid_json_is_error() {
    assert!(parse_mcp_json("{not json").is_err());
}
