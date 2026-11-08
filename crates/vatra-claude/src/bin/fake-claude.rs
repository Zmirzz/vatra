//! Stand-in for the Claude Code binary, used by the tests. Reads `fake-config.json` beside
//! itself, records its arguments and stdin in `fake-record.json`, prints the configured stream.

use std::io::{Read, Write};

use serde_json::{Value, json};

#[cfg(windows)]
fn has_console_window() -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleWindow() -> *mut std::ffi::c_void;
    }
    unsafe { !GetConsoleWindow().is_null() }
}

fn main() {
    let directory = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(directory.join("fake-config.json")).unwrap())
            .unwrap();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--input-format") {
        let mut line=String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        std::fs::write(directory.join("fake-record.json"),json!({"args":args,"stdin":line}).to_string()).unwrap();
        println!("{}",json!({"type":"control_response","response":{"request_id":"vatra-models","subtype":"success","response":{"models":config["models"]}}}));
        std::io::stdout().flush().unwrap();
        return;
    }
    let mut stdin = String::new();
    std::io::stdin().read_to_string(&mut stdin).unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Only names used to test removal are recorded; never dump the caller's environment or API keys.
    let environment: std::collections::BTreeMap<String, String> = ["PATH"]
        .into_iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_string(), value))
        })
        .collect();
    let record = json!({ "args": args, "stdin": stdin, "env": environment });
    #[cfg(windows)]
    let record = {
        let mut record = record;
        record["has_console_window"] = json!(has_console_window());
        record
    };
    std::fs::write(directory.join("fake-record.json"), record.to_string()).unwrap();

    if config["spawn_grandchild_and_hang"]
        .as_bool()
        .unwrap_or(false)
    {
        let mut grandchild = std::process::Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::fs::write(
            directory.join("grandchild.pid"),
            grandchild.id().to_string(),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_secs(120));
        let _ = grandchild.wait();
    }
    std::io::stdout()
        .write_all(&std::fs::read(config["stream"].as_str().unwrap()).unwrap())
        .unwrap();
    match config["stderr_file"].as_str() {
        Some(path) => std::io::stderr()
            .write_all(&std::fs::read(path).unwrap())
            .unwrap(),
        None => eprintln!("mesaj de pe stderr"),
    }
    std::process::exit(config["exit"].as_i64().unwrap() as i32);
}
