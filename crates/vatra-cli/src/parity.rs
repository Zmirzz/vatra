use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::path::Path;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use vatra_service::{Runtime, config::Config, existing_service};

/// Sends `method` `runs` times over one connection and returns each reply's result, in order.
pub async fn call(
    directory: &Path,
    config: &Path,
    method: &str,
    params: Value,
    runs: u32,
) -> Result<Vec<Value>, String> {
    if runs == 0 {
        return Err("runs must be at least 1".into());
    }
    let mut owned = None;
    let port = match existing_service(directory).await {
        Some(info) => info.port,
        None => {
            let config = if config.is_file() {
                Config::load(config).map_err(|e| e.to_string())?
            } else {
                Config::empty()
            };
            let running = Runtime::start(directory, config)
                .await
                .map_err(|e| e.to_string())?;
            let port = running.port();
            owned = Some(running);
            port
        }
    };
    let token = std::fs::read_to_string(directory.join("token")).map_err(|e| e.to_string())?;
    let mut request = format!("ws://127.0.0.1:{port}/rpc")
        .into_client_request()
        .map_err(|e| e.to_string())?;
    request.headers_mut().insert(
        "sec-websocket-protocol",
        format!("vatra.bearer.{}", token.trim())
            .parse()
            .map_err(|_| "Invalid service credential")?,
    );
    let (mut socket, _) = connect_async(request).await.map_err(|e| e.to_string())?;
    let result: Result<Vec<Value>, String> = async {
        let mut results = Vec::new();
        for run in 1..=i64::from(runs) {
            if runs > 1 {
                eprintln!("Run {run} of {runs}");
            }
            socket
                .send(Message::Text(
                    json!({"jsonrpc":"2.0","id":run,"method":method,"params":params})
                        .to_string()
                        .into(),
                ))
                .await
                .map_err(|e| e.to_string())?;
            let reply = loop {
                let message = socket
                    .next()
                    .await
                    .ok_or("The service disconnected")?
                    .map_err(|e| e.to_string())?;
                if let Message::Text(text) = message {
                    let reply: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                    if reply["method"] == "parity_progress" {
                        eprintln!(
                            "Task {} / {}: {}",
                            reply["params"]["done"], reply["params"]["total"], reply["params"]["task"]
                        );
                    }
                    if reply["id"] == run {
                        if !reply["error"].is_null() {
                            break Err(reply["error"]["message"]
                                .as_str()
                                .unwrap_or("Task run failed")
                                .to_string());
                        }
                        break Ok(reply["result"].clone());
                    }
                }
            };
            results.push(reply?);
        }
        Ok(results)
    }
    .await;
    if let Some(runtime) = &owned
        && let Ok(models) = runtime.service().models()
    {
        models.server().stop().await;
    }
    drop(owned);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn zero_runs_is_an_error() {
        let dir = Path::new(".");
        assert!(call(dir, dir, "parity.run", json!({}), 0).await.is_err());
    }
}
