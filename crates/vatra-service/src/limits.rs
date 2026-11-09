//! How much of each subscription is left. Claude reports it with every turn, so the last report is kept. Codex answers
//! `/status` itself, so it is asked when a person refreshes. Nothing here runs a model.

use std::sync::Arc;

use serde_json::{Value, json};
use vatra_core::{LimitWindow, Store};

use crate::service::Service;

const KEY: &str = "limits.";

fn save(store: &Store, provider: &str, windows: Vec<Value>) {
    let value = json!({"at": crate::scheduler::now(), "windows": windows});
    let _ = store.set_app_setting(&format!("{KEY}{provider}"), Some(&value.to_string()));
}

fn read(store: &Store, provider: &str) -> Option<Value> {
    store.app_setting(&format!("{KEY}{provider}")).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok())
}

/// Keeps what Claude said at the end of a turn. Called for every rate-limit report.
pub fn record_claude(store: &Store, windows: &[LimitWindow]) {
    if windows.is_empty() {
        return;
    }
    let mut windows: Vec<&LimitWindow> = windows.iter().collect();
    windows.sort_by_key(|window| match window.name.as_str() {
        "five_hour" => 0,
        "seven_day" => 1,
        _ => 2,
    });
    save(store, "claude", windows.iter().map(|w| json!({"key": w.name, "left": (1.0 - w.used).clamp(0.0, 1.0), "resets_at": w.resets_at})).collect());
}

/// Reads the limit lines of Codex's `/status`, like `**codex 5h limit:** 99% left (resets 23:29)`.
pub fn parse_status(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("**")?;
            let (label, after) = rest.split_once(":**")?;
            let after = after.trim();
            let percent: f64 = after.split_once("% left")?.0.trim().parse().ok()?;
            let resets = after.split_once("(resets ").and_then(|(_, tail)| tail.split_once(')')).map(|(when, _)| when.trim().to_string());
            let label = label.strip_prefix("codex ").unwrap_or(label);
            Some(json!({"key": label, "left": (percent / 100.0).clamp(0.0, 1.0), "resets_text": resets}))
        })
        .collect()
}

/// The ACP agents that answer `/status` with limits, by provider.
fn status_agents(service: &Arc<Service>) -> Vec<(&'static str, String)> {
    let agents = service.agents();
    let mut found: Vec<(&'static str, String)> = Vec::new();
    for name in agents.names() {
        if agents.extension_route(&name) != Some("acp") {
            continue;
        }
        let provider = match agents.picker_identity(&name) {
            Some((p, _)) if p == "openai" => "codex",
            Some((p, _)) if p == "antigravity" => "antigravity",
            _ => continue,
        };
        if !found.iter().any(|(id, _)| *id == provider) {
            found.push((provider, name));
        }
    }
    found
}

pub fn get(service: &Arc<Service>) -> Value {
    let store = service.store();
    let claude = read(store, "claude");
    let mut providers = vec![json!({
        "id": "claude", "name": "Claude", "refreshable": false,
        "updated": claude.as_ref().map(|v| v["at"].clone()),
        "windows": claude.map(|v| v["windows"].clone()).unwrap_or(json!([])),
    })];
    let installed = status_agents(service);
    for (id, name) in [("codex", "Codex"), ("antigravity", "Antigravity")] {
        if !installed.iter().any(|(provider, _)| *provider == id) {
            continue;
        }
        let stored = read(store, id);
        providers.push(json!({
            "id": id, "name": name, "refreshable": true,
            "updated": stored.as_ref().map(|v| v["at"].clone()),
            "windows": stored.map(|v| v["windows"].clone()).unwrap_or(json!([])),
        }));
    }
    json!({"providers": providers})
}

async fn refresh(service: &Arc<Service>, provider: &str) -> Result<Value, String> {
    let name = status_agents(service).into_iter().find(|(id, _)| *id == provider).map(|(_, name)| name).ok_or_else(|| "That agent isn't set up.".to_string())?;
    let text = service.agents().agent_command(&name, "/status").await?;
    let windows = parse_status(&text);
    if windows.is_empty() {
        return Err("This agent didn't report its limits.".to_string());
    }
    save(service.store(), provider, windows);
    Ok(get(service))
}

pub async fn handle(service: &Arc<Service>, method: &str, params: Value) -> Result<Value, (i64, String)> {
    match method {
        "limits.get" => Ok(get(service)),
        "limits.refresh" => {
            let provider = params["provider"].as_str().unwrap_or_default().to_string();
            refresh(service, &provider).await.map_err(|message| (-32602, message))
        }
        _ => Err((-32601, format!("unknown method `{method}`"))),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_status;

    #[test]
    fn limit_lines_of_status_are_read() {
        let text = "**Model:** gpt\n**codex 5h limit:** 99% left (resets 23:29)  \n**codex Weekly limit:** 21% left (resets 20:43 on 12 nov.)\n**codex Credits:** 0\n**Limits:** data not available yet";
        let found = parse_status(text);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0]["key"], "5h limit");
        assert_eq!(found[1]["resets_text"], "20:43 on 12 nov.");
        assert!((found[1]["left"].as_f64().unwrap() - 0.21).abs() < 1e-9);
    }
}
