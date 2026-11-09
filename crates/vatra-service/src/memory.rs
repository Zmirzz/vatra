//! The page in Settings where the person sees, adds, edits and removes the facts every agent knows about them.
use std::sync::Arc;

use serde_json::{Value, json};
use vatra_core::user_memory_now;

use crate::service::Service;

pub fn handles(method: &str) -> bool {
    method.starts_with("memory.")
}

fn list(service: &Arc<Service>) -> Result<Value, (i64, String)> {
    let store = service.store();
    let items = store.user_memories().map_err(|e| (-32603, e.to_string()))?;
    Ok(json!({"enabled": store.user_memory_enabled(), "items": items, "limit": vatra_core::MAX_MEMORIES, "max_chars": vatra_core::MAX_MEMORY_CHARS}))
}

fn bad(message: impl ToString) -> (i64, String) {
    (-32602, message.to_string())
}

pub async fn handle(service: &Arc<Service>, method: &str, params: Value) -> Result<Value, (i64, String)> {
    let store = service.store();
    match method {
        "memory.list" => list(service),
        "memory.add" => {
            let text = params["text"].as_str().unwrap_or_default();
            store.add_user_memory(text, params["category"].as_str(), "you", params["pinned"].as_bool().unwrap_or(false), user_memory_now()).map_err(bad)?;
            list(service)
        }
        "memory.update" => {
            let id = params["id"].as_i64().ok_or_else(|| bad("id is missing"))?;
            let found = store
                .update_user_memory(id, params["text"].as_str(), params["category"].as_str(), params["pinned"].as_bool(), user_memory_now())
                .map_err(bad)?;
            if found.is_none() {
                return Err(bad("That fact is already gone."));
            }
            list(service)
        }
        "memory.delete" => {
            let id = params["id"].as_i64().ok_or_else(|| bad("id is missing"))?;
            store.delete_user_memory(id).map_err(bad)?;
            list(service)
        }
        "memory.clear" => {
            store.clear_user_memories().map_err(bad)?;
            list(service)
        }
        "memory.set_enabled" => {
            store.set_user_memory_enabled(params["enabled"].as_bool().unwrap_or(true)).map_err(bad)?;
            list(service)
        }
        _ => Err((-32601, format!("unknown method `{method}`"))),
    }
}
