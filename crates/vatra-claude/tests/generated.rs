use std::path::PathBuf;

use serde_json::Value;
use vatra_claude::{ClaudeExtras, ProxySettings, generate_session_dir, translate_prompt};
use vatra_core::{McpOAuth, McpServerConfig};
use vatra_extensions::{Catalog, Origin, Scope};

#[test]
fn foreign_skills_only_and_identical_native_not_duplicated() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(&dir.path().join("catalog.db"), dir.path()).unwrap();
    for (name, origin) in [
        ("native", Origin::Claude),
        ("foreign", Origin::Codex),
        ("native", Origin::Codex),
    ] {
        let skill = dir.path().join(format!("source-{origin:?}")).join(name);
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Description\n---\nDo this.\n"),
        )
        .unwrap();
        catalog
            .add_folder(&skill, origin, Scope::Global, false)
            .unwrap();
    }
    let active = catalog.active_for(1).unwrap();
    let extras = ClaudeExtras::from_active(&active, &catalog.list().unwrap());
    assert_eq!(
        extras
            .skills
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["foreign"]
    );
    let generated = generate_session_dir(&dir.path().join("generated"), &extras).unwrap();
    assert_eq!(generated.skill_names(), ["foreign"]);
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(generated.plugin_dir().join(".claude-plugin/plugin.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["name"], "vatra-catalog");
    assert!(
        generated
            .plugin_dir()
            .join("skills/foreign/SKILL.md")
            .is_file()
    );
    assert!(!generated.plugin_dir().join("hooks").exists());
    let generated_root = generated.plugin_dir().parent().unwrap().to_path_buf();
    drop(generated);
    assert!(!generated_root.exists());
}

#[test]
fn foreign_mcp_config_omits_native_servers() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(&dir.path().join("catalog.db"), dir.path()).unwrap();
    for (name, origin) in [("native", Origin::Claude), ("foreign", Origin::Codex)] {
        catalog
            .add_server(
                &McpServerConfig {
                    name: name.into(),
                    url: Some("https://mcp.test".into()),
                    ..Default::default()
                },
                origin,
                Scope::Global,
                None,
                false,
            )
            .unwrap();
    }
    let extras =
        ClaudeExtras::from_active(&catalog.active_for(1).unwrap(), &catalog.list().unwrap());
    assert_eq!(
        extras
            .servers
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["foreign"]
    );
}

#[test]
fn oauth_proxy_keeps_name_and_token_is_file_only() {
    let settings = ProxySettings {
        binary: PathBuf::from("C:/Răzvan Test/vatra.exe"),
        url: "http://127.0.0.1:1234".into(),
        token_file: PathBuf::from("C:/Răzvan Test/token"),
        project_id: 7,
        snapshot: Some("pinned".into()),
    };
    let proxied = settings.server(&McpServerConfig {
        name: "demo".into(),
        oauth: Some(McpOAuth::default()),
        url: Some("https://mcp.test".into()),
        ..Default::default()
    });
    assert_eq!(proxied.name, "demo");
    assert!(
        proxied
            .args
            .windows(2)
            .any(|w| w == ["--token-file", "C:/Răzvan Test/token"])
    );
    assert!(proxied.args.windows(2).any(|w| w == ["--project", "7"]));
    assert!(
        proxied
            .args
            .windows(2)
            .any(|w| w == ["--snapshot", "pinned"])
    );
    assert!(proxied.oauth.is_none());
    assert!(proxied.url.is_none());
}

#[test]
fn dollar_translation_preserves_unknown_and_argument_text() {
    let names = vec!["review".into(), "deploy".into()];
    assert_eq!(
        translate_prompt("$review these files, $deploy", &names),
        "/vatra-catalog:review these files, /vatra-catalog:deploy"
    );
    assert_eq!(
        translate_prompt("$reviewer $missing a$review \\$review", &names),
        "$reviewer $missing a$review \\$review"
    );
}

#[test]
fn different_foreign_skill_with_native_name_still_delivered() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(&dir.path().join("catalog.db"), dir.path()).unwrap();
    for (origin, body) in [
        (Origin::Claude, "native body"),
        (Origin::Codex, "foreign body"),
    ] {
        let skill = dir.path().join(format!("source-{origin:?}")).join("review");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: review\ndescription: Review\n---\n{body}\n"),
        )
        .unwrap();
        catalog
            .add_folder(&skill, origin, Scope::Global, false)
            .unwrap();
    }
    let active = catalog.active_for(1).unwrap();
    let extras = ClaudeExtras::from_active(&active, &catalog.list().unwrap());
    assert_eq!(extras.skills.len(), 1);
    assert!(extras.skills[0].body.contains("foreign body"));
}

/// Free schema validation only. Never runs a prompt or starts a model.
#[test]
#[ignore = "manual local Claude plugin schema validation; no model invocation"]
fn generated_plugin_validates_with_official_cli() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("review");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: review\ndescription: Review a patch\n---\nRead the patch.",
    )
    .unwrap();
    let generated = generate_session_dir(
        dir.path(),
        &ClaudeExtras {
            skills: vec![vatra_extensions::parse_skill(&skill_dir).unwrap()],
            ..Default::default()
        },
    )
    .unwrap();
    let status = if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/c", "claude", "plugin", "validate"])
            .arg(generated.plugin_dir())
            .status()
            .unwrap()
    } else {
        std::process::Command::new("claude")
            .args(["plugin", "validate"])
            .arg(generated.plugin_dir())
            .status()
            .unwrap()
    };
    assert!(status.success());
}
