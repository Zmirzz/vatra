/// The name a tool goes by in Vatra. Codex and Claude Code use other names for some tools.
pub fn canonical_tool(name: &str) -> &str {
    match name {
        "shell" | "exec_command" => "Bash",
        "apply_patch" => "Edit",
        "update_plan" => "TodoWrite",
        "Task" => "Agent",
        other => other,
    }
}

/// Every Vatra tool a name stands for. `apply_patch` both edits and creates files, so it is `Edit` and `Write`.
pub fn canonical_tools(name: &str) -> Vec<&str> {
    match name {
        "apply_patch" => vec!["Edit", "Write"],
        other => vec![canonical_tool(other)],
    }
}
