//! The scripted factory with the MCP servers and secret store of a configuration.

use std::sync::Arc;

use async_trait::async_trait;
use vatra_core::Store;
use vatra_core::{Agent, McpServerConfig, MemorySecretStore, SecretStore, Summarizer};
use vatra_service::{AgentFactory, BuildRequest, Service};

use crate::common::{Scripted, ScriptedFactory};

pub struct WithServers {
    pub inner: ScriptedFactory,
    pub servers: Vec<McpServerConfig>,
    pub secrets: Arc<MemorySecretStore>,
}

#[async_trait]
impl AgentFactory for WithServers {
    fn names(&self) -> Vec<String> {
        self.inner.names()
    }

    async fn build(&self, request: BuildRequest) -> Result<Arc<dyn Agent>, String> {
        self.inner.build(request).await
    }

    fn summarizer(&self) -> Arc<dyn Summarizer> {
        self.inner.summarizer()
    }

    fn mcp_servers(&self) -> Vec<McpServerConfig> {
        self.servers.clone()
    }

    fn secrets(&self) -> Arc<dyn SecretStore> {
        self.secrets.clone()
    }
}

pub fn service_with(servers: Vec<McpServerConfig>, secrets: Arc<MemorySecretStore>) -> Arc<Service> {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let factory = WithServers { inner: ScriptedFactory(Scripted::text("gata")), servers, secrets };
    Service::new(store, Arc::new(factory)).unwrap()
}
