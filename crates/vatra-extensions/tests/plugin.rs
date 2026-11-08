use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use vatra_extensions::{ManifestKind, PluginBundle, read_plugin};

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/plugins")
        .join(path)
}

/// Writes each `(relative path, text)` pair under `root`, creating folders as needed.
fn write_files(root: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
}

fn skill_text(description: &str) -> String {
    format!("---\ndescription: {description}\n---\nBody.\n")
}

fn names(bundle: &PluginBundle) -> Vec<String> {
    let mut names: Vec<String> = bundle
        .skills
        .iter()
        .map(|skill| skill.name.clone())
        .collect();
    names.sort();
    names
}

#[test]
fn claude_default_layout() {
    let bundle = read_plugin(&fixture("claude-demo")).unwrap();
    assert_eq!(bundle.name, "demo");
    assert_eq!(bundle.version.as_deref(), Some("1.2.0"));
    assert_eq!(bundle.description, "Demo plugin");
    assert_eq!(bundle.author.as_deref(), Some("Ana"));
    assert_eq!(bundle.manifest_kind, ManifestKind::Claude);
    assert!(bundle.problems.is_empty(), "{:?}", bundle.problems);
    assert!(bundle.unsupported.is_empty());

    let review = bundle
        .skills
        .iter()
        .find(|skill| skill.name == "review")
        .unwrap();
    assert!(!review.is_command);
    assert_eq!(review.plugin.as_deref(), Some("demo"));
    assert_eq!(review.qualified_name(), "demo:review");

    let deploy = bundle.skills.iter().find(|skill| skill.is_command).unwrap();
    assert_eq!(deploy.name, "frontend:deploy");
    assert_eq!(deploy.qualified_name(), "demo:frontend:deploy");

    assert_eq!(bundle.mcp_servers.len(), 1);
    let docs = &bundle.mcp_servers[0];
    assert_eq!(docs.name, "docs");
    assert_eq!(docs.url.as_deref(), Some("https://docs.example.com/mcp"));
    assert_eq!(docs.plugin.as_deref(), Some("demo"));
    let context = docs.plugin_context.as_ref().unwrap();
    assert_eq!(context.root, fixture("claude-demo"));
    assert!(context.user_config.is_empty());
}

#[test]
fn manifest_is_optional() {
    let root = TempDir::new().unwrap();
    let plugin = root.path().join("plain");
    write_files(
        &plugin,
        &[("skills/review/SKILL.md", &skill_text("Reviews"))],
    );

    let bundle = read_plugin(&plugin).unwrap();
    assert_eq!(bundle.manifest_kind, ManifestKind::None);
    assert_eq!(bundle.name, "plain");
    assert!(bundle.version.is_none());
    assert_eq!(names(&bundle), vec!["review"]);
    assert!(bundle.problems.is_empty());
}

#[test]
fn custom_paths_skills_add_commands_replace() {
    let root = TempDir::new().unwrap();
    let plugin = root.path().join("demo");
    write_files(
        &plugin,
        &[
            (
                ".claude-plugin/plugin.json",
                r#"{"name": "demo", "skills": "./extra/skills", "commands": "./cmds"}"#,
            ),
            ("skills/review/SKILL.md", &skill_text("Reviews")),
            ("extra/skills/other/SKILL.md", &skill_text("Other")),
            ("commands/old.md", "Old command.\n"),
            ("cmds/new.md", "New command.\n"),
        ],
    );

    let bundle = read_plugin(&plugin).unwrap();
    assert!(bundle.problems.is_empty(), "{:?}", bundle.problems);
    let mut skills: Vec<&str> = bundle
        .skills
        .iter()
        .filter(|s| !s.is_command)
        .map(|s| s.name.as_str())
        .collect();
    skills.sort();
    assert_eq!(skills, vec!["other", "review"]);
    let commands: Vec<&str> = bundle
        .skills
        .iter()
        .filter(|s| s.is_command)
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(commands, vec!["new"]);
}

#[test]
fn paths_outside_root_rejected() {
    let base = TempDir::new().unwrap();
    let plugin = base.path().join("plugin");
    write_files(
        base.path(),
        &[("escape/skills/leak/SKILL.md", &skill_text("Leaked"))],
    );
    let outside = base
        .path()
        .join("escape")
        .join("skills")
        .to_string_lossy()
        .replace('\\', "/");
    write_files(
        &plugin,
        &[(
            ".claude-plugin/plugin.json",
            &format!(
                r#"{{"name": "demo", "skills": "../escape/skills", "commands": "{outside}"}}"#
            ),
        )],
    );

    let bundle = read_plugin(&plugin).unwrap();
    assert!(bundle.skills.is_empty());
    assert_eq!(bundle.problems.len(), 2, "{:?}", bundle.problems);
    assert!(
        bundle
            .problems
            .iter()
            .all(|problem| problem.contains("outside")),
        "{:?}",
        bundle.problems
    );
}

#[test]
fn dual_manifest_reads_claude_first() {
    let root = TempDir::new().unwrap();
    let plugin = root.path().join("dual");
    write_files(
        &plugin,
        &[
            (".claude-plugin/plugin.json", r#"{"name": "claude-name"}"#),
            ("plugin.json", r#"{"name": "portable-name"}"#),
            (".codex-plugin/plugin.json", r#"{"name": "codex-name"}"#),
        ],
    );

    let bundle = read_plugin(&plugin).unwrap();
    assert_eq!(bundle.manifest_kind, ManifestKind::Claude);
    assert_eq!(bundle.name, "claude-name");
}

#[test]
fn portable_manifest_with_openai_extensions() {
    let root = TempDir::new().unwrap();
    let plugin = root.path().join("portable");
    write_files(
        &plugin,
        &[
            (
                "plugin.json",
                r#"{"name": "portable", "version": "2.0.0", "openai": {"interface": {"displayName": "P"}}, "x-openai": {"apps": "none"}}"#,
            ),
            ("skills/review/SKILL.md", &skill_text("Reviews")),
        ],
    );

    let bundle = read_plugin(&plugin).unwrap();
    assert_eq!(bundle.manifest_kind, ManifestKind::Portable);
    assert_eq!(bundle.name, "portable");
    assert_eq!(bundle.version.as_deref(), Some("2.0.0"));
    assert!(bundle.problems.is_empty(), "{:?}", bundle.problems);
    assert_eq!(names(&bundle), vec!["review"]);
}

#[test]
fn codex_manifest_with_mcp_and_hooks() {
    let bundle = read_plugin(&fixture("codex-demo")).unwrap();
    assert_eq!(bundle.manifest_kind, ManifestKind::Codex);
    assert_eq!(bundle.name, "codex-demo");
    assert!(bundle.problems.is_empty(), "{:?}", bundle.problems);
    assert_eq!(names(&bundle), vec!["brief"]);

    assert_eq!(bundle.mcp_servers.len(), 1);
    assert_eq!(
        bundle.mcp_servers[0].command.as_deref(),
        Some("notes-server")
    );
    assert_eq!(bundle.mcp_servers[0].plugin.as_deref(), Some("codex-demo"));

    let kinds: Vec<&str> = bundle
        .unsupported
        .iter()
        .map(|item| item.kind.as_str())
        .collect();
    assert!(kinds.contains(&"hooks"), "{kinds:?}");
    assert!(kinds.contains(&".app.json"), "{kinds:?}");
}

#[test]
fn unsupported_components_listed() {
    let root = TempDir::new().unwrap();
    let plugin = root.path().join("extras");
    write_files(
        &plugin,
        &[
            (
                ".claude-plugin/plugin.json",
                r#"{"name": "extras", "lspServers": {}, "outputStyles": "./styles", "monitors": [], "channels": {}, "themes": "./themes", "workflows": "./flows"}"#,
            ),
            (".app.json", r#"{"apps": []}"#),
        ],
    );

    let bundle = read_plugin(&plugin).unwrap();
    let mut kinds: Vec<&str> = bundle
        .unsupported
        .iter()
        .map(|item| item.kind.as_str())
        .collect();
    kinds.sort();
    assert_eq!(
        kinds,
        vec![
            ".app.json",
            "channels",
            "lspServers",
            "monitors",
            "outputStyles",
            "themes",
            "workflows"
        ]
    );
    assert!(
        bundle
            .unsupported
            .iter()
            .all(|item| !item.reason.is_empty())
    );
}

#[test]
fn hooks_and_agents_marked_unsupported() {
    let root = TempDir::new().unwrap();
    let plugin = root.path().join("hooky");
    write_files(
        &plugin,
        &[
            (".claude-plugin/plugin.json", r#"{"name": "hooky"}"#),
            ("hooks/hooks.json", r#"{"hooks": {}}"#),
            ("agents/reviewer.md", "---\nname: reviewer\n---\nReview.\n"),
        ],
    );

    let bundle = read_plugin(&plugin).unwrap();
    let kinds: Vec<&str> = bundle
        .unsupported
        .iter()
        .map(|item| item.kind.as_str())
        .collect();
    assert!(kinds.contains(&"hooks"), "{kinds:?}");
    assert!(kinds.contains(&"agents"), "{kinds:?}");
    assert!(bundle.skills.is_empty());
    assert!(bundle.problems.is_empty());
}

#[test]
fn user_config_parsed() {
    let root = TempDir::new().unwrap();
    let plugin = root.path().join("configured");
    write_files(
        &plugin,
        &[(
            ".claude-plugin/plugin.json",
            r#"{"name": "configured", "userConfig": {
                "region": {"type": "string", "title": "Region", "description": "Where to send", "required": true, "default": "eu"},
                "token": {"type": "string", "title": "Token", "sensitive": true}
            }}"#,
        )],
    );

    let bundle = read_plugin(&plugin).unwrap();
    assert_eq!(bundle.user_config.len(), 2);

    let region = bundle
        .user_config
        .iter()
        .find(|option| option.key == "region")
        .unwrap();
    assert_eq!(region.kind, "string");
    assert_eq!(region.title.as_deref(), Some("Region"));
    assert_eq!(region.description.as_deref(), Some("Where to send"));
    assert!(region.required);
    assert!(!region.sensitive);
    assert_eq!(region.default, Some(serde_json::json!("eu")));

    let token = bundle
        .user_config
        .iter()
        .find(|option| option.key == "token")
        .unwrap();
    assert!(token.sensitive);
    assert!(!token.required);
    assert!(token.default.is_none());
}

#[test]
fn broken_skill_recorded_others_kept() {
    let root = TempDir::new().unwrap();
    let plugin = root.path().join("mixed");
    write_files(
        &plugin,
        &[
            ("skills/good/SKILL.md", &skill_text("Fine")),
            ("skills/bad/SKILL.md", "---\nname: [unclosed\n---\nBody.\n"),
        ],
    );

    let bundle = read_plugin(&plugin).unwrap();
    assert_eq!(names(&bundle), vec!["good"]);
    assert_eq!(bundle.problems.len(), 1, "{:?}", bundle.problems);
    assert!(bundle.problems[0].contains("bad"), "{:?}", bundle.problems);
}

#[test]
fn namespaces_are_plugin_colon_name() {
    let root = TempDir::new().unwrap();
    let plugin = root.path().join("demo");
    write_files(
        &plugin,
        &[
            (".claude-plugin/plugin.json", r#"{"name": "demo"}"#),
            ("skills/review/SKILL.md", &skill_text("Reviews")),
            ("commands/a/b.md", "Nested.\n"),
        ],
    );

    let bundle = read_plugin(&plugin).unwrap();
    let qualified: Vec<String> = bundle
        .skills
        .iter()
        .map(|skill| skill.qualified_name())
        .collect();
    assert!(
        qualified.contains(&"demo:review".to_string()),
        "{qualified:?}"
    );
    assert!(qualified.contains(&"demo:a:b".to_string()), "{qualified:?}");
}

#[test]
fn node_modules_not_scanned() {
    let root = TempDir::new().unwrap();
    let plugin = root.path().join("deps");
    write_files(
        &plugin,
        &[
            (".claude-plugin/plugin.json", r#"{"name": "deps"}"#),
            ("commands/node_modules/pkg/readme.md", "Vendored.\n"),
            ("skills/node_modules/SKILL.md", &skill_text("Vendored")),
            (
                "node_modules/pkg/skills/x/SKILL.md",
                &skill_text("Vendored"),
            ),
        ],
    );

    let bundle = read_plugin(&plugin).unwrap();
    assert!(bundle.skills.is_empty(), "{:?}", names(&bundle));
    assert!(bundle.problems.is_empty(), "{:?}", bundle.problems);
}

#[test]
fn missing_root_is_error() {
    let root = TempDir::new().unwrap();
    assert!(read_plugin(&root.path().join("absent")).is_err());
}

/// Makes `link` a symbolic link to the folder `target`. False when the platform refuses.
#[cfg(unix)]
fn link_dir(link: &Path, target: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

/// Makes `link` a symbolic link to `target`, or a junction when a symbolic link needs a privilege. False when neither works.
#[cfg(windows)]
fn link_dir(link: &Path, target: &Path) -> bool {
    std::os::windows::fs::symlink_dir(target, link).is_ok()
        || std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .status()
            .is_ok_and(|status| status.success())
}

#[test]
fn symlink_out_of_plugin_is_refused() {
    let base = TempDir::new().unwrap();
    let plugin = base.path().join("plugin");
    write_files(
        base.path(),
        &[("outside/leaked/SKILL.md", &skill_text("Leaked"))],
    );
    write_files(
        &plugin,
        &[
            (".claude-plugin/plugin.json", r#"{"name": "demo"}"#),
            ("skills/review/SKILL.md", &skill_text("Reviews")),
        ],
    );
    if !link_dir(
        &plugin.join("skills").join("leaked"),
        &base.path().join("outside").join("leaked"),
    ) {
        eprintln!("skipped: neither a symbolic link nor a junction can be made here");
        return;
    }

    let bundle = read_plugin(&plugin).unwrap();
    assert_eq!(names(&bundle), vec!["review"]);
    assert!(
        bundle
            .problems
            .iter()
            .any(|problem| problem.contains("outside the plugin folder")),
        "{:?}",
        bundle.problems
    );
}

#[test]
fn inline_mcp_servers_in_manifest() {
    let root = TempDir::new().unwrap();
    let plugin = root.path().join("inline");
    write_files(
        &plugin,
        &[
            (
                ".claude-plugin/plugin.json",
                r#"{"name": "inline", "mcpServers": {"remote": {"type": "http", "url": "https://example.com/mcp"}}}"#,
            ),
            (
                ".mcp.json",
                r#"{"mcpServers": {"ignored": {"command": "tool"}}}"#,
            ),
        ],
    );

    let bundle = read_plugin(&plugin).unwrap();
    let names: Vec<&str> = bundle.mcp_servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["remote"]);
    assert_eq!(bundle.mcp_servers[0].plugin.as_deref(), Some("inline"));
    assert!(bundle.problems.is_empty(), "{:?}", bundle.problems);
}

#[test]
fn mcp_servers_path_list_reads_each_file() {
    let root = TempDir::new().unwrap();
    let plugin = root.path().join("listed");
    write_files(
        &plugin,
        &[
            (
                ".claude-plugin/plugin.json",
                r#"{"name": "listed", "mcpServers": ["./a.json", "./b.json", "./missing.json"]}"#,
            ),
            ("a.json", r#"{"mcpServers": {"alpha": {"command": "a"}}}"#),
            (
                "b.json",
                r#"{"mcpServers": {"beta": {"url": "https://b.example/mcp", "type": "sse"}}}"#,
            ),
        ],
    );

    let bundle = read_plugin(&plugin).unwrap();
    let mut names: Vec<&str> = bundle.mcp_servers.iter().map(|s| s.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["alpha", "beta"]);
    assert_eq!(bundle.problems.len(), 1, "{:?}", bundle.problems);
    assert!(
        bundle.problems[0].contains("missing.json"),
        "{:?}",
        bundle.problems
    );
}

#[test]
fn custom_skill_path_names_one_skill_folder() {
    let root = TempDir::new().unwrap();
    let plugin = root.path().join("picked");
    write_files(
        &plugin,
        &[
            (
                ".claude-plugin/plugin.json",
                r#"{"name": "picked", "skills": "./picked/brief"}"#,
            ),
            ("picked/brief/SKILL.md", &skill_text("Writes a brief")),
            ("skills/review/SKILL.md", &skill_text("Reviews")),
        ],
    );

    let bundle = read_plugin(&plugin).unwrap();
    assert_eq!(names(&bundle), vec!["brief", "review"]);
    assert!(bundle.problems.is_empty(), "{:?}", bundle.problems);
}

#[test]
fn missing_custom_path_is_problem() {
    let root = TempDir::new().unwrap();
    let plugin = root.path().join("gone");
    write_files(
        &plugin,
        &[(
            ".claude-plugin/plugin.json",
            r#"{"name": "gone", "commands": "./nowhere"}"#,
        )],
    );

    let bundle = read_plugin(&plugin).unwrap();
    assert!(bundle.skills.is_empty());
    assert_eq!(bundle.problems.len(), 1, "{:?}", bundle.problems);
    assert!(
        bundle.problems[0].contains("nowhere"),
        "{:?}",
        bundle.problems
    );
    assert!(
        bundle.problems[0].contains("does not exist"),
        "{:?}",
        bundle.problems
    );
}
