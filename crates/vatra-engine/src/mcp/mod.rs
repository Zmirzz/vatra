mod expand;
mod http;
mod hub;
mod oauth;
mod schema;

pub use expand::{ExpandVars, Expanded, expand};
pub use hub::{HubPool, McpHub, McpPrompt, McpResource};
pub use oauth::{OAuthError, has_credentials, login};

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::tools::{SAVE_OVER_CHARS, Tool, ToolClass, ToolContext, ToolOutput, capped, save_long};

/// An error longer than this keeps its start and its end, and the middle is left out.
const ERROR_KEEP_OVER: usize = 11_000;
const ERROR_KEEP_ENDS: usize = 5_000;

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// An MCP tool as the engine sees it. Whatever the server says about it, it is checked like any Mcp call.
pub struct McpTool {
    def: ToolDef,
    hub: Arc<McpHub>,
}

impl McpTool {
    pub fn all(hub: &Arc<McpHub>) -> Vec<Arc<dyn Tool>> {
        hub.tools()
            .into_iter()
            .map(|def| Arc::new(McpTool { def: capped(def), hub: Arc::clone(hub) }) as Arc<dyn Tool>)
            .collect()
    }
}

#[async_trait]
impl Tool for McpTool {
    fn def(&self) -> ToolDef {
        self.def.clone()
    }

    fn class(&self, _arguments: &Value, _context: &ToolContext) -> ToolClass {
        ToolClass::Mcp
    }

    fn parallel_safe(&self) -> bool {
        false
    }

    async fn call(&self, arguments: Value, context: &ToolContext) -> ToolOutput {
        shorten_output(self.hub.call(&self.def.name, arguments).await, &context.session_dir)
    }
}

/// Keeps an MCP output that the model can take in one piece. A text longer than the save limit goes to a file, and
/// the reply keeps its start. An error of a middle length keeps its start and its end.
pub(crate) fn shorten_output(mut output: ToolOutput, session_dir: &Path) -> ToolOutput {
    let total = output.text.chars().count();
    if total > SAVE_OVER_CHARS {
        output.text = save_long(&output.text, session_dir, "tool-output");
    } else if output.is_error && total > ERROR_KEEP_OVER {
        let head: String = output.text.chars().take(ERROR_KEEP_ENDS).collect();
        let tail: String = output.text.chars().skip(total - ERROR_KEEP_ENDS).collect();
        let left_out = total - 2 * ERROR_KEEP_ENDS;
        output.text = format!("{head}\n[{left_out} characters were left out of the middle of the error.]\n{tail}");
    }
    output
}
