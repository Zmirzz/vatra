//! What works and what doesn't, in one list a person can read: the search providers research uses, the accounts they
//! connected, the model server. Each part keeps its last outcome, so a failure is shown with its reason and a way to
//! try again, instead of a message that says only that something could not be loaded.

use std::sync::Arc;

use serde_json::{Value, json};
use vatra_core::Store;

use crate::service::Service;

const PREFIX: &str = "health.";

/// Keeps the last outcome of one part. `key` is like `search.brave` or `connector.email-1`.
pub fn record(store: &Store, key: &str, ok: bool, detail: &str) {
    let value = json!({"ok": ok, "detail": detail.chars().take(240).collect::<String>(), "at": crate::scheduler::now()});
    let _ = store.set_app_setting(&format!("{PREFIX}{key}"), Some(&value.to_string()));
}

fn last(store: &Store, key: &str) -> Option<Value> {
    store.app_setting(&format!("{PREFIX}{key}")).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok())
}

fn state_of(outcome: Option<Value>) -> (&'static str, String, Option<i64>) {
    match outcome {
        None => ("unknown", String::new(), None),
        Some(v) => {
            let at = v["at"].as_i64();
            if v["ok"].as_bool() == Some(true) {
                ("ok", String::new(), at)
            } else {
                ("down", v["detail"].as_str().unwrap_or("").to_string(), at)
            }
        }
    }
}

/// Every part with its state: `ok`, `down` (its last use failed) or `unknown` (not used yet).
pub fn status(service: &Arc<Service>) -> Value {
    let store = service.store();
    let mut items = Vec::new();
    let kinds: std::collections::BTreeMap<&str, &str> = [("caldav", "Calendar"), ("email", "Email"), ("websearch", "Web search"), ("telegram", "Telegram"), ("ntfy", "Phone notifications"), ("embeddings", "Search by meaning")].into_iter().collect();
    for row in store.connectors().unwrap_or_default() {
        let (state, detail, at) = state_of(last(store, &format!("connector.{}", row.id)));
        items.push(json!({
            "id": format!("connector.{}", row.id), "group": "accounts", "label": row.name, "kind": kinds.get(row.kind.as_str()).copied().unwrap_or("Account"),
            "state": state, "detail": detail, "at": at, "retry": {"method": "connectors.check", "params": {"id": row.id}},
        }));
    }
    let duckduckgo_on = store.app_setting("search.duckduckgo").ok().flatten().as_deref() != Some("off");
    if duckduckgo_on {
        let (state, detail, at) = state_of(last(store, "search.duckduckgo"));
        items.push(json!({"id": "search.duckduckgo", "group": "search", "label": "DuckDuckGo", "kind": "Web search", "state": state, "detail": detail, "at": at, "retry": null}));
    }
    for name in ["brave", "tavily", "serper", "searxng"] {
        if let Some(outcome) = last(store, &format!("search.{name}")) {
            let (state, detail, at) = state_of(Some(outcome));
            items.push(json!({"id": format!("search.{name}"), "group": "search", "label": name, "kind": "Search provider", "state": state, "detail": detail, "at": at, "retry": null}));
        }
    }
    if let Ok(models) = service.models() {
        let installed = models.llama_status()["installed"].as_bool().unwrap_or(false);
        items.push(json!({
            "id": "models.llama", "group": "models", "label": "Local model server", "kind": "llama.cpp",
            "state": if installed { "ok" } else { "down" }, "detail": if installed { "" } else { "llama.cpp isn't installed, so local models can't run." }, "at": null, "retry": null,
        }));
    }
    let down = items.iter().filter(|i| i["state"] == "down").count();
    json!({"items": items, "down": down})
}

/// A recommendation for how heavy a research this computer can take with a local model, from its memory and graphics card.
pub async fn research_profile(service: &Arc<Service>) -> Value {
    let Ok(models) = service.models() else {
        return json!({"tier": "unknown", "small_model": false, "reason": "This computer's hardware isn't known."});
    };
    let hardware = models.hardware().await;
    let gib = |bytes: u64| bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    let vram = hardware.gpus.iter().filter(|g| !g.integrated).map(|g| gib(g.total)).fold(0.0, f64::max);
    let ram = gib(hardware.ram.total);
    let (tier, small, reason) = tier_for(vram, ram);
    json!({"tier": tier, "small_model": small, "reason": reason, "vram_gib": (vram * 10.0).round() / 10.0, "ram_gib": (ram * 10.0).round() / 10.0})
}

/// The tier of a computer for local models: the graphics card decides when there is one, the memory when there is not.
pub fn tier_for(vram_gib: f64, ram_gib: f64) -> (&'static str, bool, &'static str) {
    let (large, medium) = if vram_gib > 0.0 { (vram_gib >= 20.0, vram_gib >= 10.0) } else { (ram_gib >= 64.0, ram_gib >= 32.0) };
    if large {
        ("large", false, "This computer can run large models, so research runs at full depth.")
    } else if medium {
        ("medium", false, "This computer can run medium models. Deep research works, but a smaller model answers faster.")
    } else {
        ("small", true, "This computer fits small models, so research runs in light mode: fewer, shorter steps.")
    }
}

pub async fn handle(service: &Arc<Service>, method: &str, _params: Value) -> Result<Value, (i64, String)> {
    match method {
        "health.status" => Ok(status(service)),
        _ => Err((-32601, format!("unknown method `{method}`"))),
    }
}

#[cfg(test)]
mod tests {
    use super::tier_for;

    #[test]
    fn the_graphics_card_decides_when_there_is_one() {
        assert_eq!(tier_for(7.7, 31.8).0, "small");
        assert!(tier_for(7.7, 31.8).1);
        assert_eq!(tier_for(12.0, 16.0).0, "medium");
        assert_eq!(tier_for(24.0, 16.0).0, "large");
    }

    #[test]
    fn memory_decides_without_a_card() {
        assert_eq!(tier_for(0.0, 16.0).0, "small");
        assert_eq!(tier_for(0.0, 32.0).0, "medium");
        assert_eq!(tier_for(0.0, 128.0).0, "large");
    }
}
