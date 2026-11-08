use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Tool, ToolClass, ToolContext, ToolOutput, str_arg};
use crate::compaction::estimate_tokens;
use crate::mcp::{McpHub, ToolDef};
use crate::model::ContextWindow;

pub(crate) const NAME: &str = "ToolSearch";
/// Descriptions in the tool list are cut to this many characters.
const DESCRIPTION_LIMIT: usize = 4096;
const DEFAULT_RESULTS: usize = 5;
const MAX_RESULTS: usize = 20;
/// How much of a description a search result shows.
const SHOWN_DESCRIPTION: usize = 200;

/// Splits the tools of the MCP servers into the ones the request carries and the ones that wait for ToolSearch.
/// A list that takes up to a tenth of the window is sent whole. A bigger one is deferred as a whole, so the
/// first element is empty. The tools that are sent have their descriptions capped.
pub fn deferred_split(tools: Vec<ToolDef>, window: ContextWindow) -> (Vec<ToolDef>, Vec<ToolDef>) {
    if estimate_tokens(&[], &tools) > window.tokens / 10 {
        (vec![], tools)
    } else {
        (tools.into_iter().map(capped).collect(), vec![])
    }
}

/// The tool with its description cut to the limit of the tool list.
pub(crate) fn capped(mut def: ToolDef) -> ToolDef {
    if def.description.chars().count() > DESCRIPTION_LIMIT {
        def.description = def.description.chars().take(DESCRIPTION_LIMIT).collect();
    }
    def
}

/// The ToolSearch tool. It searches every tool of the hub, so it finds the ones that are not in the list yet.
pub fn tool_search_tool(hub: Arc<McpHub>) -> Arc<dyn Tool> {
    Arc::new(ToolSearchTool { hub })
}

struct ToolSearchTool {
    hub: Arc<McpHub>,
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: NAME.into(),
            description: "Finds tools of the MCP servers by words from their names and descriptions. Use it for a \
                          tool that is not in your tool list yet. The tools it finds join your tool list from the next \
                          step."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Words to look for. A full tool name finds that tool first."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "How many tools to return. 5 by default."
                    }
                },
                "required": ["query"]
            }),
        }
    }

    fn class(&self, _arguments: &Value, _context: &ToolContext) -> ToolClass {
        ToolClass::Free
    }

    fn parallel_safe(&self) -> bool {
        true
    }

    async fn call(&self, arguments: Value, _context: &ToolContext) -> ToolOutput {
        let Some(query) = str_arg(&arguments, "query") else {
            return ToolOutput::error("query is required.");
        };
        let tools = self.hub.tools();
        let found = search(&tools, query, limit(&arguments));
        if found.is_empty() {
            return ToolOutput::ok(format!("No tool matches `{query}`."));
        }
        let lines: Vec<String> = found
            .iter()
            .map(|tool| format!("- {}: {}", tool.name, shorten(&tool.description, SHOWN_DESCRIPTION)))
            .collect();
        ToolOutput::ok(format!(
            "Found {} tool(s). They are in your tool list from the next step.\n{}",
            found.len(),
            lines.join("\n")
        ))
    }

    fn loads(&self, arguments: &Value) -> Vec<String> {
        let Some(query) = str_arg(arguments, "query") else {
            return vec![];
        };
        let tools = self.hub.tools();
        search(&tools, query, limit(arguments)).into_iter().map(|tool| tool.name.clone()).collect()
    }
}

fn limit(arguments: &Value) -> usize {
    arguments
        .get("max_results")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_RESULTS, |results| (results as usize).clamp(1, MAX_RESULTS))
}

/// The tools that have a word of the query in their names or descriptions, best first. A whole name counts most,
/// then a word of the name, then a word of the description.
fn search<'a>(tools: &'a [ToolDef], query: &str, max: usize) -> Vec<&'a ToolDef> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut scored: Vec<(usize, &ToolDef)> = tools
        .iter()
        .filter_map(|tool| {
            let score = score(tool, &words);
            (score > 0).then_some((score, tool))
        })
        .collect();
    scored.sort_by(|(left, a), (right, b)| right.cmp(left).then_with(|| a.name.cmp(&b.name)));
    scored.into_iter().take(max).map(|(_, tool)| tool).collect()
}

fn score(tool: &ToolDef, words: &[String]) -> usize {
    let name = tool.name.to_lowercase();
    // The name without the server part, `filler_042` for `mcp__demo__filler_042`.
    let bare = name.splitn(3, "__").nth(2).unwrap_or(&name);
    let description = tool.description.to_lowercase();
    words
        .iter()
        .map(|word| {
            let word = word.as_str();
            if word == name || word == bare {
                10
            } else if bare.contains(word) {
                4
            } else if description.contains(word) {
                1
            } else {
                0
            }
        })
        .sum()
}

/// The text with its whitespace squeezed to single spaces, and cut to a number of characters.
fn shorten(text: &str, limit: usize) -> String {
    let squeezed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    squeezed.chars().take(limit).collect()
}
