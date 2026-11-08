mod support;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use support::{MARKER, dirs, plugin_cache, synthetic_home, write_json, write_text};
use vatra_extensions::{Origin, Scope, content_hash};
use vatra_import::{FoundKind, GuardedFs, ProjectRef, ScanResult, SourceDirs, scan_claude};

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("work").join("app");
    fs::create_dir_all(&home).unwrap();
    synthetic_home(&home, &project);
    Fixture {
        _tmp: tmp,
        home,
        project,
    }
}

fn scan(fixture: &Fixture) -> ScanResult {
    scan_claude(
        &GuardedFs::new(),
        &dirs(&fixture.home),
        std::slice::from_ref(&fixture.project),
    )
}

fn names(result: &ScanResult, kind: FoundKind) -> Vec<String> {
    result
        .found
        .iter()
        .filter(|found| found.kind == kind)
        .map(|found| found.name.clone())
        .collect()
}

#[test]
fn enabled_plugins_only() {
    let fixture = fixture();
    let result = scan(&fixture);

    assert_eq!(names(&result, FoundKind::Plugin), vec!["demo"]);
    assert!(result.found.iter().all(|found| !found.name.contains("hidden")));
    assert!(result.found.iter().all(|found| !found.name.starts_with("off")));
}

#[test]
fn only_installed_version() {
    let fixture = fixture();
    let result = scan(&fixture);

    assert!(result.found.iter().any(|found| found.name == "demo:greet"));
    assert!(result.found.iter().all(|found| found.name != "demo:old"));
    let greet = result.found.iter().find(|found| found.name == "demo:greet").unwrap();
    assert!(greet.source.starts_with(plugin_cache(&fixture.home, "demo", "1.0.0")));
}

#[test]
fn missing_install_path_is_problem_rest_continues() {
    let fixture = fixture();
    let claude = fixture.home.join(".claude");
    let gone = fixture.home.join("gone");
    write_json(
        &claude.join("settings.json"),
        &json!({ "enabledPlugins": { "demo@market": true, "gone@market": true } }),
    );
    write_json(
        &claude.join("plugins").join("installed_plugins.json"),
        &json!({
            "version": 2,
            "plugins": {
                "demo@market": [{ "scope": "user", "installPath": plugin_cache(&fixture.home, "demo", "1.0.0") }],
                "gone@market": [{ "scope": "user", "installPath": gone }]
            }
        }),
    );

    let result = scan(&fixture);

    assert!(
        result
            .problems
            .iter()
            .any(|problem| problem.message.contains("gone@market"))
    );
    assert!(
        result
            .found
            .iter()
            .any(|found| found.kind == FoundKind::Plugin && found.name == "demo")
    );
}

#[test]
fn invalid_settings_json_is_problem_rest_continues() {
    let fixture = fixture();
    write_text(&fixture.home.join(".claude").join("settings.json"), "{ not json");

    let result = scan(&fixture);

    assert!(
        result
            .problems
            .iter()
            .any(|problem| problem.message.contains("settings.json"))
    );
    assert!(names(&result, FoundKind::Plugin).is_empty());
    assert!(names(&result, FoundKind::Skill).contains(&"personal".to_string()));
    assert!(names(&result, FoundKind::McpServer).contains(&"global".to_string()));
}

#[test]
fn personal_skills_agents_commands() {
    let fixture = fixture();
    let result = scan(&fixture);

    let skill = result
        .found
        .iter()
        .find(|found| found.kind == FoundKind::Skill && found.name == "personal")
        .expect("personal skill");
    assert_eq!(skill.origin, Origin::Claude);
    assert_eq!(skill.scope, Scope::Global);
    assert!(skill.unsupported.is_none());
    assert!(names(&result, FoundKind::Command).contains(&"review:deep".to_string()));

    let agent = result
        .found
        .iter()
        .find(|found| found.kind == FoundKind::Agent && found.name == "reviewer")
        .expect("agent is counted");
    assert!(agent.unsupported.is_some());
}

#[test]
fn mcp_user_and_project_from_claude_json() {
    let fixture = fixture();
    let result = scan(&fixture);

    let global = result
        .found
        .iter()
        .find(|found| found.kind == FoundKind::McpServer && found.name == "global")
        .expect("user MCP server");
    assert_eq!(global.scope, Scope::Global);
    assert_eq!(global.project, None);
    assert_eq!(global.secrets.len(), 1);
    assert_eq!(global.secrets[0].key, "Authorization");
    assert_eq!(global.secrets[0].value(), format!("Bearer {MARKER}literal"));

    let local = result
        .found
        .iter()
        .find(|found| found.kind == FoundKind::McpServer && found.name == "local")
        .expect("user stdio server");
    assert_eq!(local.runs_code.len(), 1);
    assert!(local.runs_code[0].command.starts_with("uvx"));

    let proj = result
        .found
        .iter()
        .find(|found| found.kind == FoundKind::McpServer && found.name == "proj")
        .expect("project MCP server");
    assert_eq!(proj.scope, Scope::Global);
    assert_eq!(proj.project, Some(ProjectRef(0)));

    assert!(result.found.iter().all(|found| found.name != "other"));
    assert!(names(&result, FoundKind::McpServer).contains(&"demo:tools".to_string()));
}

#[test]
fn claude_json_other_fields_never_kept() {
    let fixture = fixture();
    let result = scan(&fixture);
    let text = serde_json::to_string(&result).unwrap();

    assert!(!text.contains("userID"));
    assert!(!text.contains("oauthAccount"));
    assert!(!text.contains("machineID"));
    assert!(!text.contains("emailAddress"));
}

#[test]
fn settings_hooks_are_reported_unsupported() {
    let fixture = fixture();
    let result = scan(&fixture);

    let hooks: Vec<_> = result
        .found
        .iter()
        .filter(|found| found.kind == FoundKind::Hook)
        .collect();
    assert_eq!(hooks.len(), 1);
    assert!(hooks[0].unsupported.is_some());
    assert!(hooks[0].runs_code.is_empty());
}

#[test]
fn permissions_found() {
    let fixture = fixture();
    let result = scan(&fixture);

    let permission = result
        .found
        .iter()
        .find(|found| found.kind == FoundKind::Permission)
        .expect("permissions");
    assert_eq!(permission.details["allow"], json!(["Read", "Bash(git status)"]));
    assert_eq!(permission.details["deny"], json!(["Bash(rm -rf *)"]));
    assert_eq!(permission.details["ask"], json!(["Edit"]));
}

#[test]
fn plugin_configs_non_sensitive_only() {
    let fixture = fixture();
    let result = scan(&fixture);

    let config = result
        .found
        .iter()
        .find(|found| found.kind == FoundKind::PluginConfig)
        .expect("plugin config");
    assert_eq!(config.details["options"]["region"], "eu");
    assert!(config.details["options"].get("apiKey").is_none());
    let text = serde_json::to_string(&result).unwrap();
    assert!(!text.contains(&format!("{MARKER}config")));
}

#[test]
fn claude_config_dir_respected() {
    let fixture = fixture();
    let custom = fixture.home.join("custom-claude");
    write_text(
        &custom.join("skills").join("elsewhere").join("SKILL.md"),
        "---\ndescription: Custom\n---\nCustom.\n",
    );
    let home = fixture.home.clone();
    let env = move |name: &str| match name {
        "CLAUDE_CONFIG_DIR" => Some(custom.display().to_string()),
        "HOME" | "USERPROFILE" => Some(home.display().to_string()),
        _ => None,
    };

    let source = SourceDirs::from_env(env);
    let result = scan_claude(&GuardedFs::new(), &source, std::slice::from_ref(&fixture.project));

    assert!(names(&result, FoundKind::Skill).contains(&"elsewhere".to_string()));
    assert!(!names(&result, FoundKind::Skill).contains(&"personal".to_string()));
}

#[test]
fn no_marker_anywhere() {
    let fixture = fixture();
    let guarded = fixture
        .home
        .join(".claude")
        .join("plugins")
        .join("cache")
        .join("market")
        .join("guarded")
        .join("2.0.0");
    write_json(
        &fixture.home.join(".claude").join("settings.json"),
        &json!({
            "enabledPlugins": { "demo@market": true, "guarded@market": true },
            "permissions": { "allow": ["Read"] },
            "pluginConfigs": { "demo@market": { "options": { "apiKey": format!("{MARKER}config") } } }
        }),
    );
    write_json(
        &fixture
            .home
            .join(".claude")
            .join("plugins")
            .join("installed_plugins.json"),
        &json!({
            "version": 2,
            "plugins": {
                "demo@market": [{ "scope": "user", "installPath": plugin_cache(&fixture.home, "demo", "1.0.0") }],
                "guarded@market": [{ "scope": "user", "installPath": guarded }]
            }
        }),
    );
    write_json(
        &guarded.join(".claude-plugin").join("plugin.json"),
        &json!({ "name": "guarded", "version": "2.0.0" }),
    );
    write_json(
        &guarded.join("auth.json"),
        &json!({ "key": format!("{MARKER}plugin-auth") }),
    );

    let result = scan(&fixture);

    let json = serde_json::to_string(&result).unwrap();
    assert!(!json.contains(MARKER), "marker in serialized result");
    assert!(!format!("{result:?}").contains(MARKER), "marker in debug output");
    assert!(result.found.iter().all(|found| !found.name.starts_with("guarded")));
    assert!(
        result
            .problems
            .iter()
            .any(|problem| problem.message.contains("auth.json"))
    );
}

#[test]
fn source_unchanged_after_scan() {
    let fixture = fixture();
    let before = content_hash(&fixture.home).unwrap();

    let _ = scan(&fixture);

    assert_eq!(before, content_hash(&fixture.home).unwrap());
}

/// A plugin folder with a manifest and one skill.
fn plugin_folder(root: &Path, name: &str, skill: &str) {
    write_json(
        &root.join(".claude-plugin").join("plugin.json"),
        &json!({ "name": name, "version": "1.0.0" }),
    );
    write_text(
        &root.join("skills").join(skill).join("SKILL.md"),
        "---\ndescription: Test\n---\nTest.\n",
    );
}

/// Settings and installed_plugins.json that enable only `demo@market`, with the given installs.
fn only_demo(home: &Path, installs: serde_json::Value) {
    let claude = home.join(".claude");
    write_json(
        &claude.join("settings.json"),
        &json!({ "enabledPlugins": { "demo@market": true } }),
    );
    write_json(
        &claude.join("plugins").join("installed_plugins.json"),
        &json!({ "version": 2, "plugins": { "demo@market": installs } }),
    );
}

#[test]
fn plugin_with_token_named_file_loads() {
    let fixture = fixture();
    let root = plugin_cache(&fixture.home, "tokenplug", "1.0.0");
    plugin_folder(&root, "tokenplug", "check");
    write_text(&root.join("tests").join("token-usage.py"), "print('usage')\n");
    write_json(
        &fixture.home.join(".claude").join("settings.json"),
        &json!({ "enabledPlugins": { "tokenplug@market": true } }),
    );
    write_json(
        &fixture
            .home
            .join(".claude")
            .join("plugins")
            .join("installed_plugins.json"),
        &json!({
            "version": 2,
            "plugins": { "tokenplug@market": [{ "scope": "user", "installPath": root }] }
        }),
    );

    let result = scan(&fixture);

    assert!(names(&result, FoundKind::Plugin).contains(&"tokenplug".to_string()));
    assert!(names(&result, FoundKind::Skill).contains(&"tokenplug:check".to_string()));
    assert!(
        result
            .problems
            .iter()
            .all(|problem| !problem.message.contains("tokenplug"))
    );
}

#[test]
fn user_install_is_preferred_over_a_project_install() {
    let fixture = fixture();
    let user = plugin_cache(&fixture.home, "demo", "1.0.0");
    let project_install = fixture.home.join("project-install");
    plugin_folder(&project_install, "demo", "projonly");
    only_demo(
        &fixture.home,
        json!([
            { "scope": "project", "projectPath": fixture.project.display().to_string(), "installPath": project_install },
            { "scope": "user", "installPath": user }
        ]),
    );

    let result = scan(&fixture);

    let plugin = result
        .found
        .iter()
        .find(|found| found.kind == FoundKind::Plugin && found.name == "demo")
        .expect("demo plugin");
    assert_eq!(plugin.source, user);
    assert_eq!(plugin.project, None);
    assert!(
        names(&result, FoundKind::Skill)
            .iter()
            .all(|name| name != "demo:projonly")
    );
}

#[test]
fn project_install_is_used_for_its_own_project() {
    let fixture = fixture();
    let project_install = fixture.home.join("project-install");
    plugin_folder(&project_install, "demo", "projonly");
    only_demo(
        &fixture.home,
        json!([{ "scope": "project", "projectPath": fixture.project.display().to_string(), "installPath": project_install }]),
    );

    let result = scan(&fixture);

    let plugin = result
        .found
        .iter()
        .find(|found| found.kind == FoundKind::Plugin && found.name == "demo")
        .expect("demo plugin");
    assert_eq!(plugin.project, Some(ProjectRef(0)));
    assert!(names(&result, FoundKind::Skill).contains(&"demo:projonly".to_string()));
}

#[test]
fn plugin_installed_only_for_another_project_is_not_available() {
    let fixture = fixture();
    let project_install = fixture.home.join("project-install");
    plugin_folder(&project_install, "demo", "projonly");
    let elsewhere = fixture.home.join("elsewhere");
    only_demo(
        &fixture.home,
        json!([{ "scope": "project", "projectPath": elsewhere.display().to_string(), "installPath": project_install }]),
    );

    let result = scan(&fixture);

    assert!(names(&result, FoundKind::Plugin).is_empty());
    assert!(
        result
            .problems
            .iter()
            .any(|problem| problem.message.contains("demo@market") && problem.message.contains("not available"))
    );
}
