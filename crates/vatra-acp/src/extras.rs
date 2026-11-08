use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use vatra_core::McpServerConfig;
use vatra_engine::{ToolClass, resolve_prompt};
use vatra_extensions::{Skill, SubstituteInput, bang_spans, substitute};

#[derive(Clone, Debug, Default)]
pub struct AcpExtras {
    /// Already approved, expanded and deduplicated against the agent's native configuration.
    /// OAuth servers must have been rewritten to stdio using the service's MCP proxy.
    pub servers: Vec<McpServerConfig>,
    pub skills: Vec<Skill>,
    pub preamble: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct McpCapabilities {
    pub http: bool,
}

/// Last advertised command list. Clones share the same state across fresh adapter instances.
#[derive(Clone, Debug, Default)]
pub struct AgentCommands(Arc<Mutex<Vec<Value>>>);

impl AgentCommands {
    pub fn latest(&self) -> Vec<Value> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn replace(&self, commands: &[Value]) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = commands
            .iter()
            .filter(|command| {
                command["name"]
                    .as_str()
                    .is_some_and(|name| !name.is_empty())
            })
            .take(4096)
            .cloned()
            .collect();
    }
}

/// ACP stdio environment/header fields are named-value arrays, not maps. Warnings carry no secrets.
pub fn acp_mcp_servers(
    servers: &[McpServerConfig],
    capabilities: &McpCapabilities,
) -> (Vec<Value>, Vec<String>) {
    let mut accepted = vec![];
    let mut omitted = vec![];
    for server in servers {
        let reason = if server.validate().is_err() {
            Some("invalid configuration")
        } else if server.oauth.is_some()
            || server.headers_helper.is_some()
            || !server.secrets.is_empty()
        {
            Some("requires the Vatra MCP proxy or resolved secrets")
        } else if server.cwd.is_some() {
            Some("per-server working directories require the Vatra MCP proxy")
        } else if server.enabled_tools.is_some() || !server.disabled_tools.is_empty() {
            Some("tool filters require the Vatra MCP proxy")
        } else if server.url.is_some() && !capabilities.http {
            Some("agent does not advertise MCP HTTP support")
        } else {
            None
        };
        if let Some(reason) = reason {
            omitted.push(format!("MCP server `{}` omitted: {reason}.", server.name));
            continue;
        }
        let entry = if let Some(url) = &server.url {
            let headers: Vec<_> = server
                .headers
                .iter()
                .map(|(name, value)| json!({"name":name,"value":value}))
                .collect();
            json!({"type":"http","name":server.name,"url":url,"headers":headers})
        } else {
            let env: Vec<_> = server
                .env
                .iter()
                .map(|(name, value)| json!({"name":name,"value":value}))
                .collect();
            json!({"name":server.name,"command":server.command,"args":server.args,"env":env})
        };
        accepted.push(entry);
    }
    (accepted, omitted)
}

pub fn first_prompt_preamble(extras: &AcpExtras) -> Option<String> {
    let mut sections = vec![];
    if let Some(text) = &extras.preamble
        && !text.trim().is_empty()
    {
        sections.push(text.clone());
    }
    let skills: Vec<_> = extras
        .skills
        .iter()
        .filter(|skill| skill.allow_implicit)
        .map(|skill| {
            format!(
                "- {}: {} (instructions: {})",
                skill.qualified_name(),
                skill.description,
                skill
                    .dir
                    .join(if skill.is_command { "" } else { "SKILL.md" })
                    .display()
            )
        })
        .collect();
    if !skills.is_empty() {
        sections.push(format!("Vatra skills available for this project. Read the listed instructions when relevant; explicit /name and $name invocations are expanded by Vatra.\n{}", skills.join("\n")));
    }
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

/// Resolve foreign commands using the same name/parser semantics as the engine. Shell-bearing skills must
/// be expanded by the service's gated injection runner, never handed to a foreign agent as executable text.
pub fn expand_prompt(
    prompt: &str,
    extras: &AcpExtras,
    cwd: &Path,
    session: &str,
) -> Result<String, String> {
    let mut output = prompt.to_string();
    for (skill, arguments) in invocations(prompt, extras) {
        let body = checked_body(&skill)?;
        let body = substitute_body(&skill, &body, &arguments, cwd, session);
        output.push_str(&format!(
            "\n\nVatra skill {}:\n{body}",
            skill.qualified_name()
        ));
    }
    Ok(output)
}

pub(crate) fn checked_body(skill: &Skill) -> Result<String, String> {
    if !bang_spans(&skill.body).is_empty() {
        return Err(format!(
            "Skill `{}` needs command expansion by Vatra before sending it to ACP.",
            skill.qualified_name()
        ));
    }
    Ok(skill.body.clone())
}

pub(crate) async fn expand_with(
    prompt: &str,
    extras: &AcpExtras,
    cwd: &Path,
    session: &str,
    handler: &dyn crate::PermissionHandler,
) -> Result<(String, Vec<(String, String)>), String> {
    let mut output = prompt.to_string();
    let mut loaded = vec![];
    for (skill, arguments) in invocations(prompt, extras) {
        let body = handler.skill_body(&skill).await?;
        let body = substitute_body(&skill, &body, &arguments, cwd, session);
        output.push_str(&format!(
            "\n\nVatra skill {}:\n{body}",
            skill.qualified_name()
        ));
        loaded.push((skill.qualified_name(), body));
    }
    Ok((output, loaded))
}

fn substitute_body(
    skill: &Skill,
    body: &str,
    arguments: &str,
    cwd: &Path,
    session: &str,
) -> String {
    substitute(
        body,
        &SubstituteInput {
            arguments,
            argument_names: &skill.arguments,
            skill_dir: &skill.dir,
            session_id: session,
            plugin_root: skill.plugin_root.as_deref(),
            project_dir: Some(cwd),
            plugin_data: skill.plugin_data.as_deref(),
            effort: skill.effort.as_deref(),
        },
    )
}

fn invocations(prompt: &str, extras: &AcpExtras) -> Vec<(Skill, String)> {
    let resolved = resolve_prompt(prompt, &extras.skills);
    let mut loaded = vec![];
    if let Some(command) = resolved.command {
        loaded.push((command.skill, command.arguments));
    }
    for skill in resolved.mentions {
        if !loaded
            .iter()
            .any(|(known, _)| known.qualified_name() == skill.qualified_name())
        {
            loaded.push((skill, String::new()));
        }
    }
    loaded
}

pub struct PermissionCheck {
    pub name: String,
    pub arguments: Value,
    pub class: ToolClass,
}

/// Evaluate every edit location through the engine policy so deny rules win in every permission mode.
/// Missing or unresolvable locations ask as outside edits. Existing ancestors are canonicalized, including
/// symlinks/junctions; a new filename under a symlink outside the workspace remains outside.
pub fn permission_checks(tool: &Value, cwd: &Path) -> Vec<PermissionCheck> {
    let raw = tool["rawInput"].as_object().cloned().unwrap_or_default();
    match tool["kind"].as_str() {
        Some("edit" | "delete" | "move") => path_checks(tool, &raw, cwd, "Edit", |inside| {
            if inside {
                ToolClass::EditInside
            } else {
                ToolClass::EditOutside
            }
        }),
        // The mode still asks for these as for any foreign tool. The names let path rules apply to them.
        Some("read") => path_checks(tool, &raw, cwd, "Read", |_| ToolClass::Mcp),
        Some("search") => path_checks(tool, &raw, cwd, "Grep", |_| ToolClass::Mcp),
        Some("fetch") => vec![PermissionCheck {
            name: "WebFetch".into(),
            arguments: Value::Object(raw),
            class: ToolClass::Web,
        }],
        Some("execute") => {
            let command = command_text(tool, &raw);
            let mut arguments = Value::Object(raw);
            arguments["command"] = json!(command);
            vec![PermissionCheck {
                name: "Bash".into(),
                arguments,
                class: ToolClass::Execute,
            }]
        }
        _ => vec![PermissionCheck {
            name: tool["name"].as_str().unwrap_or("ACP tool").into(),
            arguments: Value::Object(raw),
            class: ToolClass::Mcp,
        }],
    }
}

/// One check for every name of every location the tool reports. A path is compared as the agent gave it and as it
/// resolves, also under the working folder when that folder is reached through a link, so a rule matches any of them.
fn path_checks(
    tool: &Value,
    raw: &serde_json::Map<String, Value>,
    cwd: &Path,
    name: &str,
    class: impl Fn(bool) -> ToolClass,
) -> Vec<PermissionCheck> {
    let root = std::fs::canonicalize(cwd).ok();
    let mut checks = Vec::new();
    for path in reported_paths(tool, raw) {
        let resolved = if path.is_empty() {
            None
        } else {
            resolve_path(&cwd.join(&path))
        };
        let inside = root
            .as_ref()
            .zip(resolved.as_ref())
            .and_then(|(root, path)| path.strip_prefix(root).ok());
        let mut names = vec![];
        match &resolved {
            Some(path) => names.push(path.display().to_string()),
            None => names.push(path.clone()),
        }
        if let Some(relative) = inside {
            names.push(cwd.join(relative).display().to_string());
        }
        if !path.is_empty() {
            names.push(cwd.join(&path).display().to_string());
        }
        names.dedup();
        for file in names {
            let mut arguments = Value::Object(raw.clone());
            arguments["file_path"] = json!(file);
            checks.push(PermissionCheck {
                name: name.into(),
                arguments,
                class: class(inside.is_some()),
            });
        }
    }
    checks
}

/// The paths of a tool call, or one empty path when it names none.
fn reported_paths(tool: &Value, raw: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut paths: Vec<String> = tool["locations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|location| location["path"].as_str().map(str::to_owned))
        .collect();
    for content in tool["content"].as_array().into_iter().flatten() {
        if content["type"] == "diff"
            && let Some(path) = content["path"].as_str()
            && !paths.iter().any(|p| p == path)
        {
            paths.push(path.into());
        }
    }
    for field in ["file_path", "path", "notebook_path"] {
        if let Some(path) = raw.get(field).and_then(Value::as_str)
            && !paths.iter().any(|p| p == path)
        {
            paths.push(path.into());
        }
    }
    if paths.is_empty() {
        paths.push(String::new());
    }
    paths
}

/// The shell text of a command, which an agent may send as a list such as `["bash", "-lc", "rm x"]`. When there is
/// none, the title is wrapped in a substitution: the policy can't split that, so it never passes as a harmless command.
fn command_text(tool: &Value, raw: &serde_json::Map<String, Value>) -> String {
    match raw.get("command") {
        Some(Value::String(command)) => command.clone(),
        Some(Value::Array(words)) => {
            let words: Vec<&str> = words.iter().filter_map(Value::as_str).collect();
            match words.as_slice() {
                [.., flag, script]
                    if (flag.starts_with('-') && flag.ends_with('c'))
                        || flag.eq_ignore_ascii_case("/c")
                        || flag.eq_ignore_ascii_case("-command") =>
                {
                    (*script).to_string()
                }
                _ => words.join(" "),
            }
        }
        _ => format!("$({})", tool["title"].as_str().unwrap_or("unknown command")),
    }
}

fn resolve_path(path: &Path) -> Option<PathBuf> {
    let mut ancestor = path;
    let mut missing = vec![];
    loop {
        match std::fs::canonicalize(ancestor) {
            Ok(mut real) => {
                for name in missing.iter().rev() {
                    real.push(name);
                }
                return Some(real);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(ancestor.file_name()?.to_os_string());
                ancestor = ancestor.parent()?;
            }
            Err(_) => return None,
        }
    }
}
