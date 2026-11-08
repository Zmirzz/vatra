//! The market of connections: a short list of well-known MCP servers a person can add with one click, for someone
//! who has nothing connected yet. The list is part of the app, so it needs no network and nobody can change it from
//! outside. Each entry says only what Vatra needs to connect it; the words a person reads come from the app.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use vatra_core::{McpOAuth, McpServerConfig};
use vatra_extensions::{ElementKind, Origin, Scope};

use crate::service::Service;

/// What a person gives to connect an entry, besides a click.
#[derive(Clone, Copy)]
enum Needs {
    /// Nothing: it works as soon as it is added.
    Nothing,
    /// A sign-in in the browser, started after it is added.
    Account,
    /// A key or token, pasted once and kept in the keychain.
    Token {
        /// The header or environment name the token is stored and sent under.
        key: &'static str,
        /// Put in front of the pasted text, such as `Bearer `.
        prefix: &'static str,
        /// A page where the person makes the token.
        get_url: &'static str,
    },
}

/// How an entry connects.
#[derive(Clone, Copy)]
enum Link {
    /// A server on the internet, reached by its address. It runs nothing on this computer.
    Address(&'static str),
    /// A program this computer starts, such as `npx`.
    Program(&'static str, &'static [&'static str]),
}

struct Listing {
    id: &'static str,
    /// The name shown when the app has no translated one.
    name: &'static str,
    link: Link,
    needs: Needs,
}

const LISTINGS: &[Listing] = &[
    Listing { id: "context7", name: "Context7", link: Link::Address("https://mcp.context7.com/mcp"), needs: Needs::Nothing },
    Listing { id: "deepwiki", name: "DeepWiki", link: Link::Address("https://mcp.deepwiki.com/mcp"), needs: Needs::Nothing },
    Listing { id: "cloudflare-docs", name: "Cloudflare Docs", link: Link::Address("https://docs.mcp.cloudflare.com/mcp"), needs: Needs::Nothing },
    Listing { id: "huggingface", name: "Hugging Face", link: Link::Address("https://huggingface.co/mcp"), needs: Needs::Nothing },
    Listing { id: "playwright", name: "Playwright", link: Link::Program("npx", &["-y", "@playwright/mcp@latest"]), needs: Needs::Nothing },
    Listing { id: "notion", name: "Notion", link: Link::Address("https://mcp.notion.com/mcp"), needs: Needs::Account },
    Listing { id: "linear", name: "Linear", link: Link::Address("https://mcp.linear.app/mcp"), needs: Needs::Account },
    Listing { id: "sentry", name: "Sentry", link: Link::Address("https://mcp.sentry.dev/mcp"), needs: Needs::Account },
    Listing { id: "atlassian", name: "Atlassian", link: Link::Address("https://mcp.atlassian.com/v1/sse"), needs: Needs::Account },
    Listing {
        id: "github",
        name: "GitHub",
        link: Link::Address("https://api.githubcopilot.com/mcp/"),
        needs: Needs::Token { key: "Authorization", prefix: "Bearer ", get_url: "https://github.com/settings/personal-access-tokens/new" },
    },
];

fn server(listing: &Listing) -> McpServerConfig {
    let mut server = McpServerConfig { name: listing.id.to_string(), ..Default::default() };
    match listing.link {
        Link::Address(url) => server.url = Some(url.to_string()),
        Link::Program(command, args) => {
            server.command = Some(command.to_string());
            server.args = args.iter().map(|arg| arg.to_string()).collect();
        }
    }
    match listing.needs {
        Needs::Nothing => {}
        Needs::Account => server.oauth = Some(McpOAuth::default()),
        Needs::Token { key, .. } => {
            // The header carries a reference; the token itself goes to the keychain.
            server.headers.insert(key.to_string(), format!("${{{key}}}"));
        }
    }
    server
}

/// The list, with what is already connected marked. `connected` is a catalog entry of the same name.
pub fn list(service: &Arc<Service>) -> Result<Value, String> {
    let installed: Vec<String> = match service.agents().catalog() {
        Some(catalog) => catalog
            .list()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|entry| entry.kind == ElementKind::McpServer)
            .map(|entry| entry.name)
            .collect(),
        None => Vec::new(),
    };
    Ok(json!(LISTINGS
        .iter()
        .map(|listing| {
            let (needs, token) = match listing.needs {
                Needs::Nothing => ("nothing", Value::Null),
                Needs::Account => ("account", Value::Null),
                Needs::Token { get_url, .. } => ("token", json!({ "getUrl": get_url })),
            };
            json!({
                "id": listing.id,
                "name": listing.name,
                "needs": needs,
                "token": token,
                "runsProgram": matches!(listing.link, Link::Program(..)),
                "connected": installed.iter().any(|name| name == listing.id),
            })
        })
        .collect::<Vec<_>>()))
}

#[derive(Deserialize)]
struct Install {
    id: String,
    /// The pasted token, for an entry that needs one.
    #[serde(default)]
    token: Option<String>,
}

/// Adds an entry to the catalog for every project. The person chose it by name, so a program of the list starts
/// without a second approval; an entry that waits for a token stays off until the token is stored.
pub fn install(service: &Arc<Service>, params: Value) -> Result<Value, (i64, String)> {
    let request: Install = serde_json::from_value(params)
        .map_err(|error| (-32602, format!("invalid parameters: {error}")))?;
    let listing = LISTINGS
        .iter()
        .find(|listing| listing.id == request.id)
        .ok_or_else(|| (-32602, "That connection isn't in the list".to_string()))?;
    let catalog = service
        .agents()
        .catalog()
        .ok_or_else(|| (-32603, "the extension catalog isn't available".to_string()))?;
    let token = request.token.as_deref().map(str::trim).filter(|token| !token.is_empty());
    if matches!(listing.needs, Needs::Token { .. }) && token.is_none() {
        return Err((-32602, "This connection needs a key or token".to_string()));
    }
    let bad = |error: String| (-32603, error);
    let waiting = matches!(listing.needs, Needs::Token { .. });
    catalog
        .add_server(&server(listing), Origin::Manual, Scope::Global, None, waiting)
        .map_err(|error| bad(error.to_string()))?;
    let entry = catalog
        .list()
        .map_err(|error| bad(error.to_string()))?
        .into_iter()
        .find(|entry| {
            entry.kind == ElementKind::McpServer
                && entry.name == listing.id
                && entry.origin == Origin::Manual
                && entry.scope == Scope::Global
                && entry.plugin.is_none()
        })
        .ok_or_else(|| bad("the connection was not saved".to_string()))?;
    if let (Needs::Token { key, prefix, .. }, Some(token)) = (listing.needs, token) {
        let secrets = service.agents().secrets();
        catalog
            .set_secret(entry.id, key, &format!("{prefix}{token}"), secrets.as_ref())
            .map_err(|error| bad(error.to_string()))?;
    }
    if !entry.runs_code.is_empty() {
        catalog
            .approve(entry.id, &entry.content_hash)
            .map_err(|error| bad(error.to_string()))?;
    }
    service.agents().catalog_changed(None);
    Ok(json!({ "name": listing.id, "login": matches!(listing.needs, Needs::Account) }))
}
