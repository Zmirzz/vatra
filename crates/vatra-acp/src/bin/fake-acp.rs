//! Synthetic ACP v1 peer for integration tests. Never connects to a model or reads credentials.
use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

fn send(message: Value) {
    println!("{message}");
    std::io::stdout().flush().unwrap();
}

fn reply(id: &Value, result: Value) {
    send(json!({"jsonrpc":"2.0","id":id,"result":result}));
}

fn update(session: &str, value: Value) {
    send(
        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":value}}),
    );
}

fn log(path: &Path, message: &Value) {
    writeln!(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap(),
        "{message}"
    )
    .unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let scenario = args.get(1).map(String::as_str).unwrap_or("normal");
    if scenario == "grandchild" {
        std::thread::sleep(Duration::from_secs(120));
        return;
    }
    let path = Path::new(&args[2]);
    let session = "acp-session-1";
    let mut prompt_id = Value::Null;
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let message: Value = serde_json::from_str(&line).unwrap();
        log(path, &message);
        let id = &message["id"];
        match message["method"].as_str() {
            Some("initialize") => {
                if scenario == "bad-json" {
                    println!("not json");
                    continue;
                }
                if scenario == "oversized" {
                    println!("{}", "x".repeat(1024 * 1024 + 1));
                    continue;
                }
                if scenario == "startup-hang" {
                    std::thread::sleep(Duration::from_secs(120));
                    continue;
                }
                if scenario == "exact-frame" {
                    // The reply is exactly the 1 MiB limit. The limit doesn't count the newline.
                    let prefix = format!(
                        r#"{{"jsonrpc":"2.0","id":{id},"result":{{"protocolVersion":1,"authMethods":[],"padding":""#
                    );
                    let suffix = r#""}}"#;
                    println!(
                        "{prefix}{}{suffix}",
                        "x".repeat(1024 * 1024 - prefix.len() - suffix.len())
                    );
                    continue;
                }
                if scenario == "env" {
                    // Report presence only, so the trace never contains values.
                    log(
                        path,
                        &json!({"env":{
                        "apiKey":std::env::var_os("VATRA_API_KEY").is_some(),
                        "hidden":std::env::var_os("VATRA_TEST_HIDDEN_KEY").is_some(),
                        "userProfile":std::env::var_os("USERPROFILE").is_some(),
                        "visible":std::env::var_os("VATRA_TEST_VISIBLE").is_some()}}),
                    );
                }
                reply(
                    id,
                    json!({"protocolVersion":if scenario == "v2" { 2 } else { 1 },
                    "agentCapabilities":{"loadSession":scenario != "no-load", "sessionCapabilities":if scenario == "resume" { json!({"resume":{}}) } else { json!({}) }},"authMethods":[]}),
                );
            }
            Some("session/new" | "session/load" | "session/resume") => {
                assert!(Path::new(message["params"]["cwd"].as_str().unwrap()).is_absolute());
                if scenario != "extras" {
                    assert_eq!(message["params"]["mcpServers"], json!([]));
                }
                if scenario == "auth" {
                    send(
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"secret must never be echoed"}}),
                    );
                } else {
                    if scenario == "resume" {
                        update(
                            session,
                            json!({"sessionUpdate":"available_commands_update","availableCommands":[{"name":"compact","description":"Compact context"}]}),
                        );
                    }
                    if message["method"] == "session/load" {
                        update(
                            session,
                            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"replayed text"}}),
                        );
                    }
                    reply(
                        id,
                        if message["method"] == "session/load"
                            || message["method"] == "session/resume"
                        {
                            json!({})
                        } else {
                            if scenario == "picker" {
                                json!({"sessionId":session,"configOptions":[
                                    {"id":"model","category":"model","currentValue":"large","options":[{"value":"large","name":"Large"}]},
                                    {"id":"thinking","category":"thought_level","currentValue":"high","options":[{"value":"high","name":"High"}]}
                                ]})
                            } else { json!({"sessionId":session}) }
                        },
                    );
                }
            }
            Some("session/set_config_option" | "session/set_model") => {
                if scenario == "reject-choice" {
                    send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"unsupported"}}));
                } else { reply(id, json!({})); }
            }
            Some("session/prompt") => {
                prompt_id = id.clone();
                if scenario == "views" {
                    update(
                        session,
                        json!({"sessionUpdate":"plan","entries":[{"content":"Implement","status":"in_progress","priority":"high"}]}),
                    );
                    update(
                        session,
                        json!({"sessionUpdate":"tool_call","toolCallId":"edit-1","title":"Change","kind":"edit","content":[{"type":"diff","path":"test.txt","oldText":"old","newText":"new"},{"type":"diff","path":"second.txt","oldText":null,"newText":"new file"}]}),
                    );
                    update(
                        session,
                        json!({"sessionUpdate":"usage_update","tokens":100}),
                    );
                }
                if scenario == "crash" {
                    std::process::exit(3);
                }
                if scenario == "hang" {
                    // The integration test deliberately verifies that the client's process group
                    // reaps this descendant even when the peer doesn't cooperate with cancellation.
                    #[allow(clippy::zombie_processes)]
                    let child = std::process::Command::new(std::env::current_exe().unwrap())
                        .arg("grandchild")
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .unwrap();
                    log(
                        path,
                        &json!({"childPid":child.id(),"parentPid":std::process::id()}),
                    );
                    continue;
                }
                if matches!(scenario, "permission" | "always-only" | "permission-hang") {
                    update(
                        session,
                        json!({"sessionUpdate":"tool_call","toolCallId":"tool-1","title":"Run command","kind":"execute","rawInput":{"command":"echo hello"}}),
                    );
                    send(
                        json!({"jsonrpc":"2.0","id":"permission-1","method":"session/request_permission",
                        "params":{"sessionId":session,"toolCall":{"toolCallId":"tool-1"},"options":[
                            {"optionId":"yes","kind":if scenario == "always-only" { "allow_always" } else { "allow_once" },"name":"Allow"},
                            {"optionId":"no","kind":"reject_once","name":"Reject"}]}}),
                    );
                    continue;
                }
                if scenario == "partial-hang" {
                    update(
                        session,
                        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Partial "}}),
                    );
                    continue;
                }
                if scenario == "unsupported" {
                    send(
                        json!({"jsonrpc":"2.0","id":"fs-1","method":"fs/write_text_file","params":{"path":"should-not-exist","content":"no"}}),
                    );
                    continue;
                }
                update(
                    session,
                    json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hello "}}),
                );
                update(
                    "wrong-session",
                    json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"WRONG"}}),
                );
                update(
                    session,
                    json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"world"}}),
                );
                update(
                    session,
                    json!({"sessionUpdate":"tool_call","toolCallId":"tool-1","title":"Read","kind":"read","status":"pending","rawInput":{"path":"test.txt"}}),
                );
                update(
                    session,
                    json!({"sessionUpdate":"tool_call_update","toolCallId":"tool-1","rawOutput":"file contents"}),
                );
                update(
                    session,
                    json!({"sessionUpdate":"tool_call_update","toolCallId":"tool-1","status":"completed"}),
                );
                update(
                    session,
                    json!({"sessionUpdate":"future_extension","data":42}),
                );
                reply(
                    id,
                    json!({"stopReason":if scenario == "refusal" { "refusal" } else { "end_turn" }}),
                );
            }
            Some("session/cancel") => {
                // Stay alive to ensure the client enforces cleanup, including descendants.
                log(path, &json!({"cancelReceived":true}));
            }
            None if id == "permission-1" || id == "fs-1" => {
                if message["result"]["outcome"]["outcome"] != "cancelled" {
                    reply(&prompt_id, json!({"stopReason":"end_turn"}));
                }
            }
            _ => panic!("Unexpected ACP client message"),
        }
    }
}
