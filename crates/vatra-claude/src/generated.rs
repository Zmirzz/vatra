//! Per-turn files handed to the official, unmodified Claude Code binary.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use vatra_core::McpServerConfig;
use vatra_extensions::{ActiveExtensions, CatalogEntry, ElementKind, Origin, Skill};

#[derive(Default)]
pub struct ClaudeExtras {
    pub skills: Vec<Skill>,
    pub servers: Vec<McpServerConfig>,
    pub append_prompt: Option<String>,
}

impl ClaudeExtras {
    /// The active snapshot remains authoritative for approval and scope. Metadata only identifies items already
    /// loaded by Claude itself, so native plugins and their hooks are left to the official binary.
    pub fn from_active(active: &ActiveExtensions, entries: &[CatalogEntry]) -> Self {
        let native_skill_entries: Vec<_> = entries
            .iter()
            .filter(|entry| {
                entry.origin == Origin::Claude
                    && matches!(entry.kind, ElementKind::Skill | ElementKind::Command)
            })
            .collect();
        let is_native = |skill: &Skill| {
            native_skill_entries.iter().any(|entry| {
                skill.name == entry.name
                    && skill.plugin == entry.plugin
                    && skill.dir.starts_with(&entry.content_dir)
            })
        };
        let native_skills: Vec<_> = active
            .skills
            .iter()
            .filter(|skill| is_native(skill))
            .collect();
        let native_servers: HashSet<_> = entries
            .iter()
            .filter(|entry| entry.origin == Origin::Claude && entry.kind == ElementKind::McpServer)
            .map(|entry| (entry.name.as_str(), entry.plugin.as_deref()))
            .collect();
        Self {
            skills: active
                .skills
                .iter()
                .filter(|skill| {
                    !is_native(skill)
                        && !native_skills.iter().any(|native| same_skill(skill, native))
                })
                .cloned()
                .collect(),
            servers: active
                .mcp_servers
                .iter()
                .filter(|server| {
                    !native_servers.contains(&(server.name.as_str(), server.plugin.as_deref()))
                })
                .cloned()
                .collect(),
            append_prompt: None,
        }
    }
}

fn same_skill(left: &Skill, right: &Skill) -> bool {
    let mut right = right.clone();
    right.dir = left.dir.clone();
    right.plugin_root = left.plugin_root.clone();
    right.plugin_data = left.plugin_data.clone();
    left == &right
}

pub struct GeneratedDir {
    _directory: tempfile::TempDir,
    plugin_dir: PathBuf,
    names: Vec<String>,
    aliases: Vec<(String, String)>,
}

impl GeneratedDir {
    pub fn plugin_dir(&self) -> &Path {
        &self.plugin_dir
    }
    pub fn skill_names(&self) -> &[String] {
        &self.names
    }
    pub fn translate_prompt(&self, prompt: &str) -> String {
        translate(prompt, &self.aliases)
    }
}

/// Generates only a plugin with skills. Imported hooks and subagents never enter this plugin.
pub fn generate_session_dir(root: &Path, extensions: &ClaudeExtras) -> io::Result<GeneratedDir> {
    fs::create_dir_all(root)?;
    let directory = tempfile::Builder::new()
        .prefix("session-")
        .tempdir_in(root)?;
    let plugin_dir = directory.path().join("vatra-catalog");
    fs::create_dir_all(plugin_dir.join(".claude-plugin"))?;
    fs::write(
        plugin_dir.join(".claude-plugin/plugin.json"),
        r#"{"name":"vatra-catalog","version":"1.0.0","description":"Skills from the Vatra catalog"}"#,
    )?;
    let mut names = Vec::new();
    let mut aliases = Vec::new();
    for skill in &extensions.skills {
        let qualified = skill.qualified_name();
        let name: String = qualified
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        if name.is_empty() || names.contains(&name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "skill names collide in the generated plugin",
            ));
        }
        let target = plugin_dir.join("skills").join(&name);
        copy_skill(&skill.dir, &target)?;
        // Preserve the skill's policy without generating fork agents or an imported model override.
        let mut text = format!(
            "---\nname: {}\ndescription: {}\ndisable-model-invocation: {}\nuser-invocable: {}\n",
            yaml(&name),
            yaml(&skill.description),
            !skill.allow_implicit,
            skill.user_invocable
        );
        if let Some(hint) = &skill.argument_hint {
            text.push_str(&format!("argument-hint: {}\n", yaml(hint)));
        }
        if !skill.allowed_tools.is_empty() {
            text.push_str(&format!(
                "allowed-tools: {}\n",
                serde_json::to_string(&skill.allowed_tools)?
            ));
        }
        text.push_str("---\n");
        let mut body = skill.body.clone();
        if let Some(root) = &skill.plugin_root {
            body = body.replace("${CLAUDE_PLUGIN_ROOT}", &root.to_string_lossy());
        }
        if let Some(data) = &skill.plugin_data {
            body = body.replace("${CLAUDE_PLUGIN_DATA}", &data.to_string_lossy());
        }
        text.push_str(&body);
        fs::write(target.join("SKILL.md"), text)?;
        aliases.push((qualified, name.clone()));
        names.push(name);
    }
    Ok(GeneratedDir {
        _directory: directory,
        plugin_dir,
        names,
        aliases,
    })
}

fn yaml(text: &str) -> String {
    serde_json::to_string(text).expect("strings serialize")
}

fn copy_skill(source: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        // A command's parent may contain siblings; no plugin control files or imported agents are copied.
        if matches!(name.to_str(), Some(".claude-plugin" | "hooks" | "agents")) {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a generated skill cannot contain symbolic links",
            ));
        }
        if kind.is_dir() {
            copy_skill(&entry.path(), &target.join(name))?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target.join(name))?;
        }
    }
    Ok(())
}

pub fn translate_prompt(prompt: &str, generated: &[String]) -> String {
    translate(
        prompt,
        &generated
            .iter()
            .map(|name| (name.clone(), name.clone()))
            .collect::<Vec<_>>(),
    )
}

fn translate(prompt: &str, aliases: &[(String, String)]) -> String {
    let mut result = String::with_capacity(prompt.len());
    let mut characters = prompt.char_indices().peekable();
    let mut previous = None;
    while let Some((offset, character)) = characters.next() {
        if character == '$'
            && previous.is_none_or(|c: char| c.is_whitespace() || matches!(c, '(' | '[' | '{'))
        {
            let start = offset + 1;
            let mut end = start;
            while let Some(&(index, c)) = characters.peek() {
                if !(c.is_alphanumeric() || matches!(c, '-' | '_' | ':')) {
                    break;
                }
                end = index + c.len_utf8();
                characters.next();
            }
            let name = &prompt[start..end];
            if let Some((_, generated)) = aliases.iter().find(|(alias, _)| alias == name) {
                result.push_str("/vatra-catalog:");
                result.push_str(generated);
            } else {
                result.push('$');
                result.push_str(name);
            }
            previous = name.chars().last().or(Some('$'));
        } else {
            result.push(character);
            previous = Some(character);
        }
    }
    result
}

/// Connection details carry a file name, never the service token or an OAuth token.
pub struct ProxySettings {
    pub binary: PathBuf,
    pub url: String,
    pub token_file: PathBuf,
    pub project_id: i64,
    pub snapshot: Option<String>,
}

impl ProxySettings {
    pub fn server(&self, server: &McpServerConfig) -> McpServerConfig {
        let key = server.plugin.as_ref().map_or_else(
            || server.name.clone(),
            |plugin| format!("plugin_{plugin}_{}", server.name),
        );
        let mut args = vec![
            "mcp-proxy".into(),
            "--server".into(),
            key,
            "--url".into(),
            self.url.clone(),
            "--token-file".into(),
            self.token_file.to_string_lossy().into_owned(),
            "--project".into(),
            self.project_id.to_string(),
        ];
        if let Some(snapshot) = &self.snapshot {
            args.extend(["--snapshot".into(), snapshot.clone()]);
        }
        McpServerConfig {
            name: server.name.clone(),
            plugin: server.plugin.clone(),
            command: Some(self.binary.to_string_lossy().into_owned()),
            args,
            ..Default::default()
        }
    }
}
