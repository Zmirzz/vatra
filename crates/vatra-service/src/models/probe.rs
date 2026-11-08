//! The MCP server the short check calls: one tool that returns a token chosen for that check run.
//! It is part of `vatra.exe` (`vatra mcp-probe`), so the check needs nothing installed.

use rmcp::model::{ServerCapabilities, ServerConfig};
use rmcp::transport::stdio;
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use rmcp::handler::server::wrapper::Parameters;

/// The variable the token is read from, so it never appears in a command line.
pub const TOKEN_VAR: &str = "VATRA_PROBE_TOKEN";

#[derive(Clone)]
struct Probe {
    token: String,
}

#[tool_router]
impl Probe {
    #[tool(description = "Returns the probe token for this check. Takes no arguments.")]
    fn probe_token(&self) -> String {
        self.token.clone()
    }
}

#[tool_handler]
impl ServerHandler for Probe {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}

/// Serves the probe over stdio until the client closes it.
pub async fn serve() -> Result<(), String> {
    let token = std::env::var(TOKEN_VAR).unwrap_or_default();
    let service = Probe { token }.serve(stdio()).await.map_err(|error| error.to_string())?;
    service.waiting().await.map_err(|error| error.to_string())?;
    Ok(())
}

#[derive(Clone)]
struct Fixture;
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct Echo { text: String }
#[tool_router]
impl Fixture {
    #[tool(description="Returns the given text unchanged.")]
    fn echo(&self, Parameters(args): Parameters<Echo>) -> String { args.text }
}
#[tool_handler]
impl ServerHandler for Fixture {
    fn get_info(&self) -> ServerConfig { ServerConfig::new(ServerCapabilities::builder().enable_tools().build()) }
}
pub async fn serve_fixture() -> Result<(), String> {
    let service = Fixture.serve(stdio()).await.map_err(|e|e.to_string())?;
    service.waiting().await.map_err(|e|e.to_string())?;
    Ok(())
}
