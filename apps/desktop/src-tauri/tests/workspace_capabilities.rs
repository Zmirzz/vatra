//! Packaging contract: the compiled desktop must authorize only the native
//! commands used by its human-triggered close and file-export flows.
#[test]
fn workspace_close_and_file_dialog_commands_are_authorized() {
    let capability: serde_json::Value =
        serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
    let permissions = capability["permissions"].as_array().unwrap();
    for command in [
        "core:window:allow-close",
        "core:window:allow-destroy",
        "dialog:allow-open",
        "dialog:allow-save",
    ] {
        assert!(permissions.iter().any(|value| value.as_str() == Some(command)),
            "desktop packaging omits {command}");
    }
    assert_eq!(capability["windows"], serde_json::json!(["main"]));
    assert!(!permissions.iter().any(|value| value.as_str() == Some("dialog:default")));
}
