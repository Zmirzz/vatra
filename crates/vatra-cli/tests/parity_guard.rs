use std::{
    io::Write,
    process::{Command, Stdio},
};
fn hook(tool: &str, input: serde_json::Value) -> serde_json::Value {
    let dir = tempfile::tempdir().unwrap();
    let policy = dir.path().join("policy.json");
    std::fs::write(
        &policy,
        serde_json::json!({"root":dir.path(),"commands":["node test.js"]}).to_string(),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_vatra"))
        .args(["parity-guard", "--policy"])
        .arg(policy)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    write!(
        child.stdin.take().unwrap(),
        "{}",
        serde_json::json!({"tool_name":tool,"tool_input":input})
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn hook_allows_exact_command_and_refuses_chaining_and_outside_paths() {
    assert_eq!(
        hook("Bash", serde_json::json!({"command":"node test.js"}))["hookSpecificOutput"]["permissionDecision"],
        "allow"
    );
    assert_eq!(
        hook(
            "Bash",
            serde_json::json!({"command":"node test.js && whoami"})
        )["hookSpecificOutput"]["permissionDecision"],
        "deny"
    );
    assert_eq!(
        hook(
            "Write",
            serde_json::json!({"file_path":"../outside","content":"x"})
        )["hookSpecificOutput"]["permissionDecision"],
        "deny"
    );
}
