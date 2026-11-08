#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use vatra_import::SourceDirs;

/// Every credential and literal value in the synthetic HOME starts with this text. A scan result must never hold it.
pub const MARKER: &str = "SECRET-MARKER-";

pub fn write_text(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

pub fn write_json(path: &Path, value: &Value) {
    write_text(path, &value.to_string());
}

/// The source folders for a synthetic HOME, without reading the environment.
pub fn dirs(home: &Path) -> SourceDirs {
    SourceDirs {
        claude: home.join(".claude"),
        codex: home.join(".codex"),
        agents: home.join(".claude").join("agents"),
        home: home.to_path_buf(),
    }
}

/// The cache folder of one plugin version, as Claude Code lays it out.
pub fn plugin_cache(home: &Path, plugin: &str, version: &str) -> PathBuf {
    home.join(".claude")
        .join("plugins")
        .join("cache")
        .join("market")
        .join(plugin)
        .join(version)
}

/// A HOME with Claude Code's folders: one enabled plugin (with an older version beside the installed one), one
/// disabled plugin, personal skills, commands and agents, credential files holding the marker, and a `.claude.json`
/// with user and project MCP servers.
pub fn synthetic_home(home: &Path, project: &Path) {
    let claude = home.join(".claude");
    let demo = plugin_cache(home, "demo", "1.0.0");
    let off = plugin_cache(home, "off", "1.0.0");

    write_json(
        &claude.join("settings.json"),
        &json!({
            "enabledPlugins": { "demo@market": true, "off@market": false },
            "permissions": {
                "allow": ["Read", "Bash(git status)"],
                "deny": ["Bash(rm -rf *)"],
                "ask": ["Edit"]
            },
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{ "type": "command", "command": format!("echo {MARKER}hook") }]
                }]
            },
            "pluginConfigs": {
                "demo@market": { "options": { "region": "eu", "apiKey": format!("{MARKER}config") } }
            }
        }),
    );
    write_json(
        &claude.join("plugins").join("installed_plugins.json"),
        &json!({
            "version": 2,
            "plugins": {
                "demo@market": [{ "scope": "user", "installPath": demo, "version": "1.0.0" }],
                "off@market": [{ "scope": "user", "installPath": off, "version": "1.0.0" }]
            }
        }),
    );

    write_json(
        &demo.join(".claude-plugin").join("plugin.json"),
        &json!({
            "name": "demo",
            "version": "1.0.0",
            "description": "Demo plugin",
            "userConfig": {
                "region": { "type": "string" },
                "apiKey": { "type": "string", "sensitive": true }
            }
        }),
    );
    write_text(
        &demo.join("skills").join("greet").join("SKILL.md"),
        "---\ndescription: Says hello\n---\nSay hello.\n",
    );
    write_text(
        &demo.join("commands").join("ship.md"),
        "---\ndescription: Ship it\n---\nCheck first:\n!`git status`\n",
    );
    write_json(
        &demo.join(".mcp.json"),
        &json!({
            "mcpServers": {
                "tools": {
                    "command": "node",
                    "args": ["server.js"],
                    "env": { "TOKEN_VALUE": format!("{MARKER}plugin") }
                }
            }
        }),
    );

    write_text(
        &plugin_cache(home, "demo", "0.9.0")
            .join("skills")
            .join("old")
            .join("SKILL.md"),
        "---\ndescription: OLD\n---\nOld version.\n",
    );
    write_text(
        &off.join("skills").join("hidden").join("SKILL.md"),
        "---\ndescription: Disabled\n---\nDisabled.\n",
    );

    write_text(
        &claude.join("skills").join("personal").join("SKILL.md"),
        "---\ndescription: Mine\n---\nMine.\n",
    );
    write_text(
        &claude.join("commands").join("review").join("deep.md"),
        "---\ndescription: Deep review\n---\nReview.\n",
    );
    write_text(
        &claude.join("agents").join("reviewer.md"),
        "---\nname: reviewer\n---\nReview code.\n",
    );

    write_json(
        &claude.join(".credentials.json"),
        &json!({ "claudeAiOauth": { "accessToken": format!("{MARKER}oauth") } }),
    );
    write_json(
        &home.join(".codex").join("auth.json"),
        &json!({ "tokens": { "access": format!("{MARKER}codex") } }),
    );

    write_json(
        &home.join(".claude.json"),
        &json!({
            "userID": format!("{MARKER}user"),
            "oauthAccount": { "emailAddress": format!("{MARKER}mail") },
            "machineID": format!("{MARKER}machine"),
            "mcpServers": {
                "global": {
                    "type": "http",
                    "url": "https://mcp.example.test/mcp",
                    "headers": { "Authorization": format!("Bearer {MARKER}literal") }
                },
                "local": {
                    "command": "uvx",
                    "args": ["local-tool"],
                    "env": { "LOCAL_KEY": "${LOCAL_KEY}" }
                }
            },
            "projects": {
                project.display().to_string(): {
                    "mcpServers": {
                        "proj": {
                            "command": "node",
                            "args": ["proj.js"],
                            "env": { "PROJ_KEY": format!("{MARKER}proj") }
                        }
                    }
                },
                home.join("elsewhere").display().to_string(): {
                    "mcpServers": { "other": { "command": "node" } }
                }
            }
        }),
    );
}

/// The cache folder of one Codex plugin version, as Codex lays it out under its home.
pub fn codex_plugin_cache(home: &Path, market: &str, plugin: &str, version: &str) -> PathBuf {
    home.join(".codex")
        .join("plugins")
        .join("cache")
        .join(market)
        .join(plugin)
        .join(version)
}

/// A plugin folder with a Codex manifest and one skill, named after the skill.
pub fn codex_plugin(root: &Path, plugin: &str, skill: &str) {
    write_json(
        &root.join(".codex-plugin").join("plugin.json"),
        &json!({ "name": plugin, "version": "1.0.0", "description": "Demo" }),
    );
    write_text(
        &root.join("skills").join(skill).join("SKILL.md"),
        &format!("---\ndescription: {skill}\n---\nDo {skill}.\n"),
    );
}
