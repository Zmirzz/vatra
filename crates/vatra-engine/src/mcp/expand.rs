use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What the `${...}` references of an MCP server can point at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExpandVars {
    /// What `${NAME}` reads from.
    pub env: BTreeMap<String, String>,
    /// Variables that servers must never read or inherit, such as the API keys of the agents.
    pub hidden: Vec<String>,
    pub plugin_root: Option<PathBuf>,
    pub plugin_data: Option<PathBuf>,
    pub project_dir: Option<PathBuf>,
    /// The settings of the plugin, which `${user_config.KEY}` reads from.
    pub user_config: BTreeMap<String, String>,
}

impl ExpandVars {
    pub fn from_env(project_dir: &Path, hidden: Vec<String>) -> ExpandVars {
        ExpandVars {
            env: std::env::vars().collect(),
            hidden,
            project_dir: Some(project_dir.to_path_buf()),
            ..Default::default()
        }
    }

    /// Whether `name` is a variable that servers can't read: the service's own `VATRA_` ones and the hidden ones.
    pub fn is_reserved(&self, name: &str) -> bool {
        name.starts_with("VATRA_") || self.hidden.iter().any(|hidden| hidden == name)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Expanded {
    pub text: String,
    /// The names that have no value and no default. Values never appear here.
    pub missing: Vec<String>,
    /// The names of reserved variables that a reference asked for. Their values are never used.
    pub reserved: Vec<String>,
}

/// Replaces `${NAME}`, `${NAME:-default}` and the plugin and project references. A reference with no value
/// and no default stays as it is, and its name goes to `missing`, or to `reserved` when it is a reserved name.
pub fn expand(text: &str, vars: &ExpandVars) -> Expanded {
    let mut out = String::new();
    let mut missing = Vec::new();
    let mut reserved = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let Some(offset) = rest[start..].find('}') else { break };
        let end = start + offset;
        out.push_str(&rest[..start]);
        let reference = &rest[start + 2..end];
        let (name, default) = match reference.split_once(":-") {
            Some((name, default)) => (name, Some(default)),
            None => (reference, None),
        };
        match (lookup(name, vars), default) {
            (Some(value), _) if !value.is_empty() => out.push_str(&value),
            (_, Some(default)) => out.push_str(default),
            (Some(value), None) => out.push_str(&value),
            (None, None) => {
                out.push_str(&rest[start..=end]);
                if vars.is_reserved(name) && vars.env.contains_key(name) {
                    reserved.push(name.to_string());
                } else {
                    missing.push(name.to_string());
                }
            }
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Expanded { text: out, missing, reserved }
}

fn lookup(name: &str, vars: &ExpandVars) -> Option<String> {
    let path = |path: &Option<PathBuf>| path.as_ref().map(|path| path.display().to_string());
    match name {
        "CLAUDE_PLUGIN_ROOT" => path(&vars.plugin_root),
        "CLAUDE_PLUGIN_DATA" => path(&vars.plugin_data),
        "CLAUDE_PROJECT_DIR" => path(&vars.project_dir),
        _ => match name.strip_prefix("user_config.") {
            Some(key) => vars.user_config.get(key).cloned(),
            None if vars.is_reserved(name) => None,
            None => vars.env.get(name).cloned(),
        },
    }
}
