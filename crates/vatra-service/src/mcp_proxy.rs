//! A stdio MCP server for one project server whose OAuth credentials remain in Vatra's keychain.

use std::path::Path;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::post;
use axum::{Json, Router};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::stdio;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Service, token_matches};

#[derive(Clone)]
struct ProxyState {
    service: Arc<Service>,
    token: Arc<str>,
}

#[derive(Deserialize, Serialize)]
struct ProxyRequest {
    project_id: i64,
    operation: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: Value,
    #[serde(default)]
    snapshot: Option<String>,
}

/// Kept separate so the public RPC router never accidentally exposes this service-token endpoint.
pub fn internal_mcp_router(service: Arc<Service>, token: Arc<str>) -> Router {
    Router::new()
        .route("/internal/mcp/{server}", post(internal_mcp))
        .with_state(ProxyState { service, token })
}

async fn internal_mcp(
    State(state): State<ProxyState>,
    AxumPath(server): AxumPath<String>,
    headers: HeaderMap,
    Json(request): Json<ProxyRequest>,
) -> Result<Json<Value>, StatusCode> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !token_matches(&state.token, token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let project = state
        .service
        .store()
        .list_projects()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .into_iter()
        .find(|project| project.id == request.project_id)
        .ok_or(StatusCode::NOT_FOUND)?;
    let folder = project.path.as_ref().map(std::path::Path::new);
    let hub = match request.snapshot.as_deref() {
        Some(snapshot) => state.service.agents().proxy_hub(snapshot, project.id),
        None => state.service.agents().mcp_hub(project.id, folder),
    }
    .ok_or(StatusCode::NOT_FOUND)?;
    let prefix = format!("mcp__{server}__");
    match request.operation.as_str() {
        "list" => {
            if !hub.ensure_server_connected(&server).await {
                return Err(StatusCode::NOT_FOUND);
            }
            let tools: Vec<Value> = hub.tools().into_iter().filter_map(|tool| {
                let name = tool.name.strip_prefix(&prefix)?;
                Some(json!({ "name": name, "description": tool.description, "inputSchema": tool.parameters }))
            }).collect();
            // Do not turn a missing OAuth login or a broken server into a misleading empty tool list.
            if let Some((_, message)) = hub.failures().into_iter().find(|(name, _)| name == &server)
            {
                return Ok(Json(json!({ "error": message })));
            }
            Ok(Json(json!({ "tools": tools })))
        }
        "call" => {
            let qualified = format!("{prefix}{}", request.name);
            let result = hub.call_raw(&qualified, request.arguments).await;
            Ok(Json(match result {
                Ok(result) => {
                    serde_json::to_value(result).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                }
                Err(message) => json!({ "error": message }),
            }))
        }
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

#[derive(Clone)]
struct McpProxy {
    http: reqwest::Client,
    url: String,
    server: String,
    token: String,
    project_id: i64,
    snapshot: Option<String>,
}

impl McpProxy {
    async fn forward(
        &self,
        operation: &str,
        name: String,
        arguments: Value,
    ) -> Result<Value, ErrorData> {
        let mut url = reqwest::Url::parse(&self.url).map_err(|_| proxy_error())?;
        // URL segments are escaped even when an imported server name contains spaces, slashes or diacritics.
        url.path_segments_mut()
            .map_err(|_| proxy_error())?
            .pop_if_empty()
            .extend(["internal", "mcp", &self.server]);
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .json(&ProxyRequest {
                project_id: self.project_id,
                operation: operation.into(),
                name,
                arguments,
                snapshot: self.snapshot.clone(),
            })
            .send()
            .await
            .map_err(|_| proxy_error())?;
        if !response.status().is_success() {
            return Err(proxy_error());
        }
        let result: Value = response.json().await.map_err(|_| proxy_error())?;
        if result.get("error").is_some() {
            // The upstream failure can include headers or response bodies. Keep it within the service.
            return Err(proxy_error());
        }
        Ok(result)
    }
}

fn proxy_error() -> ErrorData {
    ErrorData::internal_error(
        "Vatra couldn't reach this MCP server. Check its connection and login in Vatra.",
        None,
    )
}

impl ServerHandler for McpProxy {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let result = self.forward("list", String::new(), Value::Null).await?;
        let tools: Vec<Tool> =
            serde_json::from_value(result.get("tools").cloned().unwrap_or_else(|| json!([])))
                .map_err(|_| proxy_error())?;
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let result = self
            .forward(
                "call",
                request.name.into_owned(),
                Value::Object(request.arguments.unwrap_or_default()),
            )
            .await?;
        let result: CallToolResult = serde_json::from_value(result).map_err(|_| proxy_error())?;
        Ok(CallToolResponse::Complete(result))
    }
}

pub async fn run_mcp_proxy(
    server: &str,
    url: &str,
    token_file: &Path,
    project_id: i64,
    snapshot: Option<&str>,
) -> Result<(), String> {
    let token = std::fs::read_to_string(token_file)
        .map_err(|_| "The MCP proxy token file couldn't be read.".to_string())?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("The MCP proxy token file is empty.".into());
    }
    // The token is sent only to the loopback service; never let a command point it at a remote host.
    let address = reqwest::Url::parse(url)
        .map_err(|_| "The MCP proxy service address is invalid.".to_string())?;
    if address.scheme() != "http"
        || !matches!(
            address.host_str(),
            Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
        )
        || !address.username().is_empty()
        || address.password().is_some()
    {
        return Err("The MCP proxy must connect to the local Vatra service.".into());
    }
    let proxy = McpProxy {
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "The MCP proxy HTTP client couldn't start.".to_string())?,
        url: url.into(),
        server: server.into(),
        token,
        project_id,
        snapshot: snapshot.map(str::to_string),
    };
    let service = proxy
        .serve(stdio())
        .await
        .map_err(|_| "The MCP proxy couldn't start.".to_string())?;
    service
        .waiting()
        .await
        .map_err(|_| "The MCP proxy connection closed.".to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentFactory, BuildRequest};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use vatra_core::{Agent, AgentError, BriefFields, Store, Summarizer};
    use vatra_engine::{ExpandVars, McpHub};

    #[derive(Default)]
    struct Factory {
        fallback: AtomicUsize,
    }

    #[async_trait]
    impl Summarizer for Factory {
        async fn summarize(&self, _: &str) -> Result<BriefFields, AgentError> {
            unreachable!()
        }
    }

    #[async_trait]
    impl AgentFactory for Factory {
        fn names(&self) -> Vec<String> {
            Vec::new()
        }
        async fn build(&self, _: BuildRequest) -> Result<Arc<dyn Agent>, String> {
            unreachable!()
        }
        fn summarizer(&self) -> Arc<dyn Summarizer> {
            Arc::new(Factory::default())
        }
        fn proxy_hub(&self, key: &str, project: i64) -> Option<Arc<McpHub>> {
            (key == "pinned" && project == 1)
                .then(|| Arc::new(McpHub::new(Vec::new(), ExpandVars::default())))
        }
        fn mcp_hub(&self, _: i64, _: Option<&Path>) -> Option<Arc<McpHub>> {
            self.fallback.fetch_add(1, Ordering::Relaxed);
            Some(Arc::new(McpHub::new(Vec::new(), ExpandVars::default())))
        }
    }

    #[tokio::test]
    async fn proxy_auth_and_snapshot_project_scope_fail_closed() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let first = store.create_project("one").unwrap();
        let second = store.create_project("two").unwrap();
        assert_eq!(first, 1);
        let factory = Arc::new(Factory::default());
        let service = Service::new(store, factory.clone()).unwrap();
        let state = ProxyState {
            service,
            token: "local-test-token".into(),
        };
        let request = |project, snapshot: &str| ProxyRequest {
            project_id: project,
            operation: "call".into(),
            name: "echo".into(),
            arguments: json!({}),
            snapshot: Some(snapshot.into()),
        };
        let denied = internal_mcp(
            State(state.clone()),
            AxumPath("demo".into()),
            HeaderMap::new(),
            Json(request(first, "pinned")),
        )
        .await;
        assert_eq!(denied.unwrap_err(), StatusCode::UNAUTHORIZED);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer local-test-token".parse().unwrap(),
        );
        for (project, snapshot) in [(first, "missing"), (second, "pinned")] {
            let result = internal_mcp(
                State(state.clone()),
                AxumPath("demo".into()),
                headers.clone(),
                Json(request(project, snapshot)),
            )
            .await;
            assert_eq!(result.unwrap_err(), StatusCode::NOT_FOUND);
        }
        let result = internal_mcp(
            State(state),
            AxumPath("demo".into()),
            headers,
            Json(request(first, "pinned")),
        )
        .await
        .unwrap();
        assert!(result.0["error"].is_string());
        assert_eq!(factory.fallback.load(Ordering::Relaxed), 0);
    }
}
