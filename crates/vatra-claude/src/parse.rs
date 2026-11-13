use serde_json::Value;
use vatra_core::Event;

#[derive(Clone, Debug, PartialEq)]
pub enum Parsed {
    Event(Event),
    NativeSession(String),
    Completed,
    Failed(String),
}

fn unmapped(raw: impl Into<String>) -> Parsed {
    Parsed::Event(Event::Unmapped { source: "claude".into(), raw: raw.into() })
}

/// Maps one `stream-json` line. Whatever isn't recognized comes back as `Unmapped`.
pub fn parse_line(line: &str) -> Vec<Parsed> {
    if line.trim().is_empty() {
        return vec![];
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return vec![unmapped(line)];
    };
    match value["type"].as_str() {
        Some("system") => parse_system(&value).unwrap_or_else(|| vec![unmapped(line)]),
        Some("assistant") => nested(&value, blocks(&value, line, assistant_block)),
        Some("user") => nested(&value, blocks(&value, line, user_block)),
        Some("rate_limit_event") => parse_rate_limit(&value).map_or_else(|| vec![unmapped(line)], |event| vec![Parsed::Event(event)]),
        Some("result") => parse_result(&value, line),
        _ => vec![unmapped(line)],
    }
}

fn parse_system(value: &Value) -> Option<Vec<Parsed>> {
    let event = match value["subtype"].as_str()? {
        "init" => return Some(parse_init(value)),
        "hook_started" => Event::Hook {
            name: value["hook_name"].as_str()?.into(),
            phase: "started".into(),
            output: None,
        },
        "hook_response" => Event::Hook {
            name: value["hook_name"].as_str()?.into(),
            phase: value["outcome"].as_str()?.into(),
            output: value["output"].as_str().filter(|output| !output.is_empty()).map(String::from),
        },
        "task_started" => Event::SubagentStarted {
            id: value["tool_use_id"].as_str()?.into(),
            description: value["description"].as_str()?.into(),
        },
        "task_notification" => Event::SubagentFinished {
            id: value["tool_use_id"].as_str()?.into(),
            status: value["status"].as_str()?.into(),
        },
        "permission_denied" => Event::ToolDenied {
            id: value["tool_use_id"].as_str()?.into(),
            name: value["tool_name"].as_str()?.into(),
        },
        _ => return None,
    };
    Some(vec![Parsed::Event(event)])
}

fn count(value: &Value) -> u32 {
    value.as_array().map_or(0, |items| items.len() as u32)
}

fn parse_init(value: &Value) -> Vec<Parsed> {
    let mut parsed = vec![Parsed::Event(Event::Catalog {
        skills: count(&value["skills"]),
        agents: count(&value["agents"]),
        plugins: count(&value["plugins"]),
        mcp_servers: count(&value["mcp_servers"]),
    })];
    if let Some(session) = value["session_id"].as_str() {
        parsed.push(Parsed::NativeSession(session.into()));
    }
    // One notice for all of them: a long list of tools that are not signed in would bury the answer.
    let mut unavailable: Vec<&str> = Vec::new();
    for server in value["mcp_servers"].as_array().map(Vec::as_slice).unwrap_or_default() {
        if server["status"].as_str().unwrap_or("unknown") != "connected" {
            let name = server["name"].as_str().unwrap_or("unknown");
            if !unavailable.contains(&name) {
                unavailable.push(name);
            }
        }
    }
    if !unavailable.is_empty() {
        const SHOWN: usize = 4;
        let names: Vec<String> = unavailable.iter().take(SHOWN).map(|name| format!("`{name}`")).collect();
        let more = unavailable.len().saturating_sub(SHOWN);
        let list = if more > 0 { format!("{}, and {more} more", names.join(", ")) } else { names.join(", ") };
        let noun = if unavailable.len() == 1 { "tool isn't" } else { "tools aren't" };
        parsed.push(Parsed::Event(Event::Warning {
            source: "claude".into(),
            message: format!("{} connected {noun} available right now: {list}.", unavailable.len()),
        }));
    }
    parsed
}

fn parse_rate_limit(value: &Value) -> Option<Event> {
    let info = &value["rate_limit_info"];
    let windows = info["unifiedWindows"].as_object().into_iter().flatten().filter_map(|(name, window)| {
        Some(vatra_core::LimitWindow { name: name.clone(), used: window["utilization"].as_f64()?.clamp(0.0, 1.0), resets_at: window["resetsAt"].as_i64() })
    }).collect();
    Some(Event::RateLimit { status: info["status"].as_str()?.into(), resets_at: info["resetsAt"].as_i64(), windows })
}

fn parse_result(value: &Value, line: &str) -> Vec<Parsed> {
    let usage = &value["usage"];
    let mut parsed = match (usage["input_tokens"].as_u64(), usage["output_tokens"].as_u64()) {
        (Some(input_tokens), Some(output_tokens)) => vec![Parsed::Event(Event::Usage {
            input_tokens,
            output_tokens,
            cost_usd: value["total_cost_usd"].as_f64(),
        })],
        _ => vec![unmapped(line)],
    };
    parsed.push(if value["is_error"].as_bool().unwrap_or(false) {
        let message = value["result"].as_str().unwrap_or("Claude Code reported an error");
        Parsed::Failed(with_login_hint(message))
    } else {
        Parsed::Completed
    });
    parsed
}

/// Claude Code keeps its own sign-in, which Vatra can't renew. When it has lapsed, say where to sign in again.
fn with_login_hint(message: &str) -> String {
    let lower = message.to_lowercase();
    let signed_out = lower.contains("failed to authenticate")
        || lower.contains("oauth")
        || lower.contains("invalid api key")
        || lower.contains("please run /login");
    if signed_out {
        format!("{message}. Claude Code's sign-in has lapsed: open a terminal, run `claude` and type /login, then try again.")
    } else {
        message.to_string()
    }
}

/// Lines from a subagent carry `parent_tool_use_id`, which ties them to the `Agent` call.
fn nested(value: &Value, parsed: Vec<Parsed>) -> Vec<Parsed> {
    let Some(parent) = value["parent_tool_use_id"].as_str() else {
        return parsed;
    };
    parsed
        .into_iter()
        .map(|item| match item {
            Parsed::Event(event) => Parsed::Event(Event::Nested {
                parent_id: parent.into(),
                event: Box::new(event),
            }),
            other => other,
        })
        .collect()
}

fn blocks(value: &Value, line: &str, map: fn(&Value) -> Option<Vec<Parsed>>) -> Vec<Parsed> {
    match value["message"]["content"].as_array() {
        Some(content) => content
            .iter()
            .flat_map(|block| map(block).unwrap_or_else(|| vec![unmapped(block.to_string())]))
            .collect(),
        None => vec![unmapped(line)],
    }
}

fn assistant_block(block: &Value) -> Option<Vec<Parsed>> {
    let event = match block["type"].as_str()? {
        "text" => Event::AssistantText { text: block["text"].as_str()?.into() },
        "tool_use" => Event::ToolCall {
            id: block["id"].as_str()?.into(),
            name: block["name"].as_str()?.into(),
            arguments: block["input"].clone(),
        },
        _ => return None,
    };
    Some(vec![Parsed::Event(event)])
}

fn user_block(block: &Value) -> Option<Vec<Parsed>> {
    if block["type"] != "tool_result" {
        return None;
    }
    let mut parsed = Vec::new();
    let output = match &block["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => {
            let mut texts = Vec::new();
            for part in parts {
                match part["text"].as_str() {
                    Some(text) => texts.push(text),
                    None => parsed.push(unmapped(part.to_string())),
                }
            }
            texts.join("\n")
        }
        _ => String::new(),
    };
    parsed.push(Parsed::Event(Event::ToolResult {
        id: block["tool_use_id"].as_str()?.into(),
        output,
        is_error: block["is_error"].as_bool().unwrap_or(false),
    }));
    Some(parsed)
}
