//! A stand-in for `llama-server` in the tests. It answers `--version`, `--help` and `--list-devices` like the
//! real one, serves `/health`, `/props` and `/v1/chat/completions` with scripted behavior, and `mcp-probe` runs
//! Vatra's probe server so the check can find it without the CLI.
//!
//! The mode is read from `<model>.mode` beside the model file, so tests running together don't share it: `good` (default), `no-tools` (always answers in text), `flaky` (text on the first tool
//! request, then good), `wrong-arg` (asks for the wrong city), `forget` (calls the tool, then ignores the result),
//! `exit` (fails to load), `broken` (refuses requests with tools). The arguments of a server start are written to `<model>.args`.

use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Json;
use axum::routing::{get, post};
use serde_json::{Value, json};

static TOOL_REQUESTS: AtomicUsize = AtomicUsize::new(0);
static MODE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!("version: 0.8.0-dev (build 99, commit abc1234)");
            return;
        }
        Some("--help") => {
            println!("-fit,  --fit [on|off]   fit\n-fitt, --fit-target MiB\n-fitc, --fit-ctx N");
            println!("--jinja\n--list-devices\n--mmproj FILE\n--ui,  --webui, --no-ui, --no-webui\n--offline");
            return;
        }
        Some("--list-devices") => {
            println!("Available devices:\n  Vulkan0: Fake GPU (8000 MiB, 7000 MiB free)");
            return;
        }
        Some("mcp-probe") => {
            let _ = vatra_service::models::probe::serve().await;
            return;
        }
        _ => {}
    }
    let model = value_after(&args, "-m").unwrap_or_default();
    let _ = std::fs::write(format!("{model}.args"), args.join("\n"));
    let chosen = std::fs::read_to_string(format!("{model}.mode")).unwrap_or_else(|_| "good".into());
    let _ = MODE.set(chosen.trim().to_string());
    if mode() == "exit" {
        eprintln!("llama_model_load: error loading model: tensor data is not within the file bounds");
        std::process::exit(1);
    }
    let port = value_after(&args, "--port").unwrap_or_else(|| "8080".into());
    let ctx: u64 = value_after(&args, "-c").and_then(|value| value.parse().ok()).unwrap_or(4096);
    eprintln!("llama_model_load_from_file_impl: using device Vulkan0 (Fake GPU) - 7000 MiB free");
    eprintln!("load_tensors: offloaded 37/37 layers to GPU");
    let app = axum::Router::new()
        .route("/health", get(|| async { Json(json!({ "status": "ok" })) }))
        .route("/props", get(move || async move { Json(json!({ "default_generation_settings": { "n_ctx": ctx } })) }))
        .route("/v1/chat/completions", post(chat));
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await.expect("port");
    axum::serve(listener, app).await.expect("serve");
}

fn mode() -> &'static str {
    MODE.get().map_or("good", String::as_str)
}

fn value_after(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find(|pair| pair[0] == flag).map(|pair| pair[1].clone())
}

async fn chat(headers: axum::http::HeaderMap, Json(body): Json<Value>) -> axum::response::Response {
    use axum::response::IntoResponse;
    // Like llama.cpp with LLAMA_API_KEY set: requests without the key are refused.
    if let Ok(key) = std::env::var("LLAMA_API_KEY") {
        let sent = headers.get("authorization").and_then(|value| value.to_str().ok());
        if sent != Some(format!("Bearer {key}").as_str()) {
            return (axum::http::StatusCode::UNAUTHORIZED, "invalid api key").into_response();
        }
    }
    // `broken`: the server fails every request that offers tools, as an expired key or a crash would.
    if mode() == "broken" && body["tools"].as_array().is_some_and(|tools| !tools.is_empty()) {
        return (axum::http::StatusCode::UNAUTHORIZED, "invalid key").into_response();
    }
    answer(body).await.into_response()
}

async fn answer(body: Value) -> Json<Value> {
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    let last = messages.last().cloned().unwrap_or(Value::Null);
    let tools: Vec<String> = body["tools"]
        .as_array()
        .map(|tools| tools.iter().filter_map(|tool| tool["function"]["name"].as_str().map(String::from)).collect())
        .unwrap_or_default();
    if last["role"] == "tool" {
        let result = last["content"].as_str().unwrap_or_default();
        let answer = if mode() == "forget" {
            "Done.".to_string()
        } else {
            // The file says "The access code is CODE."; the probe returns the token alone.
            result.trim().trim_end_matches('.').rsplit([' ', '\t']).next().unwrap_or(result).to_string()
        };
        return text(&answer);
    }
    if tools.is_empty() {
        return Json(json!({
            "choices": [{ "message": { "role": "assistant", "content": "- one\n- two" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 300, "completion_tokens": 128 },
            "timings": { "prompt_per_second": 500.0, "predicted_per_second": 42.0 },
        }));
    }
    let prompt = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .and_then(|message| message["content"].as_str())
        .unwrap_or_default()
        .to_string();
    let first_try = TOOL_REQUESTS.fetch_add(1, Ordering::SeqCst) == 0;
    if mode() == "no-tools" || (mode() == "flaky" && first_try) {
        return text("I don't have that information.");
    }
    if prompt.contains("access-code.txt") {
        let file = if mode() == "wrong-arg" { "other.txt" } else { "access-code.txt" };
        return call("Read", json!({ "file_path": file }));
    }
    if prompt.contains("probe_token")
        && let Some(name) = tools.iter().find(|name| name.ends_with("probe_token"))
    {
        return call(name, json!({}));
    }
    text("Hello.")
}

fn text(content: &str) -> Json<Value> {
    Json(json!({
        "choices": [{ "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 3 },
    }))
}

fn call(name: &str, arguments: Value) -> Json<Value> {
    Json(json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{ "id": "call_1", "type": "function", "function": { "name": name, "arguments": arguments.to_string() } }],
            },
            "finish_reason": "tool_calls",
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 3 },
    }))
}
