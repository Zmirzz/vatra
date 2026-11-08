//! Reads Claude Code's initialization metadata, without submitting a user message.
use std::{path::Path, process::Stdio, time::Duration};
use serde_json::{Value, json};
use tokio::{io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader}, process::Command, time::timeout};
use vatra_core::ProcessGroup;

fn parse_models(response: &Value) -> Value {
    let models: Vec<Value> = response["models"].as_array().into_iter().flatten().filter_map(|model| {
        let id = model["value"].as_str().or(model["id"].as_str())?;
        let levels: Vec<Value> = model["supportedEffortLevels"].as_array().into_iter().flatten().filter_map(|v| v.as_str())
            .map(|id| json!({"id":id,"name":id})).collect();
        Some(json!({"id":id,"name":model["displayName"].as_str().unwrap_or(id),"efforts":levels}))
    }).collect();
    json!({"models":models,"efforts":[],"current_model":null,"current_effort":null})
}

pub async fn probe_models(binary: &Path, hidden_env: &[String]) -> Result<Value, String> {
    let mut command = Command::new(binary);
    command.args(["--input-format","stream-json","--output-format","stream-json","--verbose",
        "--setting-sources","","--settings", r#"{"disableAllHooks":true}"#,
        "--strict-mcp-config","--mcp-config",r#"{"mcpServers":{}}"#]);
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    command.env_remove("VATRA_API_KEY");
    for name in hidden_env { command.env_remove(name); }
    #[cfg(windows)] command.creation_flags(0x08000000);
    #[cfg(unix)] command.process_group(0);
    let tree = ProcessGroup::new().map_err(|_| "Couldn't track Claude Code".to_string())?;
    let mut child = command.spawn().map_err(|_| "Couldn't start Claude Code to read its models".to_string())?;
    tree.adopt(&child).map_err(|_| "Couldn't track Claude Code".to_string())?;
    let result = timeout(Duration::from_secs(20), async {
        let mut input = child.stdin.take().ok_or("Claude Code has no input")?;
        let output = child.stdout.take().ok_or("Claude Code has no output")?;
        input.write_all(b"{\"type\":\"control_request\",\"request_id\":\"vatra-models\",\"request\":{\"subtype\":\"initialize\"}}\n")
            .await.map_err(|_| "Couldn't ask Claude Code for its models")?;
        let mut reader = BufReader::new(output);
        for _ in 0..100 {
            let mut line = String::new();
            let count = (&mut reader).take(1024 * 1024 + 1).read_line(&mut line).await.map_err(|_| "Couldn't read Claude Code's models")?;
            if count == 0 || count > 1024 * 1024 { break; }
            let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
            if message["type"] == "control_response" && message["response"]["request_id"] == "vatra-models" {
                if message["response"]["subtype"] == "error" { break; }
                return Ok(parse_models(&message["response"]["response"]));
            }
        }
        Err("Claude Code didn't report its models. Update Claude Code and try again.")
    }).await;
    let _ = tree.kill_all();
    let _ = child.start_kill();
    let _ = timeout(Duration::from_secs(2), child.wait()).await;
    result.map_err(|_| "Reading Claude Code's models timed out".to_string())?.map_err(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_efforts_come_from_cli_metadata() {
        let found = parse_models(&json!({"models":[
            {"value":"large","displayName":"Large","supportedEffortLevels":["low","high"]},
            {"value":"small","displayName":"Small"}
        ]}));
        assert_eq!(found["models"][0]["efforts"].as_array().unwrap().len(),2);
        assert_eq!(found["models"][1]["efforts"],json!([]));
    }
}
