mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use support::{MARKER, codex_plugin, codex_plugin_cache, dirs, write_json, write_text};
use vatra_extensions::Origin;
use vatra_import::{FoundKind, GuardedFs, ProjectRef, ScanResult, SourceDirs, scan_codex};

fn scan(home: &Path, projects: &[PathBuf]) -> ScanResult {
    scan_codex(&GuardedFs::new(), &dirs(home), projects)
}

fn names(result: &ScanResult, kind: FoundKind) -> Vec<String> {
    let mut names: Vec<String> = result
        .found
        .iter()
        .filter(|found| found.kind == kind)
        .map(|found| found.name.clone())
        .collect();
    names.sort();
    names
}

fn codex(home: &Path) -> PathBuf {
    home.join(".codex")
}

#[test]
fn config_mcp_servers() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_text(
        &codex(home).join("config.toml"),
        "[mcp_servers.docs]\nurl = \"https://docs.example.test/mcp\"\nbearer_token_env_var = \"DOCS_TOKEN\"\n\n\
         [mcp_servers.local]\ncommand = \"uvx\"\nargs = [\"local-tool\"]\nenv = { LOCAL_KEY = \"SECRET-MARKER-literal\" }\n",
    );

    let result = scan(home, &[]);

    assert_eq!(names(&result, FoundKind::McpServer), vec!["docs", "local"]);
    let docs = result.found.iter().find(|found| found.name == "docs").unwrap();
    assert_eq!(docs.origin, Origin::Codex);
    assert_eq!(docs.details["url"], "https://docs.example.test/mcp");
    let local = result.found.iter().find(|found| found.name == "local").unwrap();
    assert_eq!(local.secrets.len(), 1);
    assert_eq!(local.secrets[0].key, "LOCAL_KEY");
    assert_eq!(local.runs_code[0].command, "uvx local-tool");
    assert!(!format!("{:?}", result.found).contains(MARKER));
}

#[test]
fn plugins_enabled_in_config() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_text(
        &codex(home).join("config.toml"),
        "[plugins.\"demo@market\"]\nenabled = true\n\n[plugins.\"off@market\"]\nenabled = false\n",
    );
    codex_plugin(&codex_plugin_cache(home, "market", "demo", "1.0.0"), "demo", "greet");
    codex_plugin(&codex_plugin_cache(home, "market", "off", "1.0.0"), "off", "hidden");

    let result = scan(home, &[]);

    assert_eq!(names(&result, FoundKind::Plugin), vec!["demo"]);
    assert_eq!(names(&result, FoundKind::Skill), vec!["demo:greet"]);
    assert!(result.found.iter().all(|found| !found.name.contains("hidden")));
}

#[test]
fn cache_uses_active_version() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_text(
        &codex(home).join("config.toml"),
        "[plugins.\"demo@market\"]\nenabled = true\n",
    );
    codex_plugin(&codex_plugin_cache(home, "market", "demo", "1.0.0"), "demo", "old");
    sleep(Duration::from_millis(50));
    codex_plugin(&codex_plugin_cache(home, "market", "demo", "2.0.0"), "demo", "new");

    let result = scan(home, &[]);

    assert_eq!(names(&result, FoundKind::Skill), vec!["demo:new"]);
}

#[test]
fn skills_dir_and_agents_skills() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_text(
        &codex(home).join("skills").join("mine").join("SKILL.md"),
        "---\ndescription: Mine\n---\nMine.\n",
    );
    write_text(
        &home.join(".agents").join("skills").join("shared").join("SKILL.md"),
        "---\ndescription: Shared\n---\nShared.\n",
    );

    let result = scan(home, &[]);

    assert_eq!(names(&result, FoundKind::Skill), vec!["mine", "shared"]);
    assert!(result.found.iter().all(|found| found.origin == Origin::Codex));
}

#[test]
fn skills_dir_skips_system_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_text(
        &codex(home)
            .join("skills")
            .join(".system")
            .join("imagegen")
            .join("SKILL.md"),
        "---\ndescription: Bundled\n---\nBundled.\n",
    );

    let result = scan(home, &[]);

    assert!(names(&result, FoundKind::Skill).is_empty());
}

#[test]
fn skills_config_disabled_respected() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let quiet = codex(home).join("skills").join("quiet");
    write_text(&quiet.join("SKILL.md"), "---\ndescription: Quiet\n---\nQuiet.\n");
    write_text(
        &codex(home).join("skills").join("loud").join("SKILL.md"),
        "---\ndescription: Loud\n---\nLoud.\n",
    );
    write_text(
        &codex(home).join("config.toml"),
        &format!(
            "[[skills.config]]\npath = '{}'\nenabled = false\n",
            quiet.join("SKILL.md").display()
        ),
    );

    let result = scan(home, &[]);

    assert_eq!(names(&result, FoundKind::Skill), vec!["loud"]);
}

#[test]
fn agents_and_hooks_are_reported_unsupported() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_text(
        &codex(home).join("agents").join("reviewer.toml"),
        "name = \"reviewer\"\n",
    );
    write_json(&codex(home).join("hooks.json"), &serde_json::json!({ "hooks": {} }));
    write_text(&codex(home).join("config.toml"), "[hooks]\nstop = \"echo done\"\n");

    let result = scan(home, &[]);

    let agents: Vec<_> = result
        .found
        .iter()
        .filter(|found| found.kind == FoundKind::Agent)
        .collect();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].name, "reviewer");
    assert!(agents[0].unsupported.as_deref().unwrap().contains("Codex agents"));

    let hooks: Vec<_> = result
        .found
        .iter()
        .filter(|found| found.kind == FoundKind::Hook)
        .collect();
    assert_eq!(hooks.len(), 2);
    assert!(hooks.iter().all(|hook| hook.unsupported.is_some()));
    assert!(result.found.iter().all(|found| found.kind != FoundKind::Skill));
}

#[test]
fn global_agents_md_override_wins() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_text(&codex(home).join("AGENTS.md"), "base rules\n");
    write_text(&codex(home).join("AGENTS.override.md"), "override rules\n");

    let result = scan(home, &[]);

    let instructions: Vec<_> = result
        .found
        .iter()
        .filter(|found| found.kind == FoundKind::Instructions)
        .collect();
    assert_eq!(instructions.len(), 1);
    assert_eq!(instructions[0].name, "AGENTS.override.md");
    assert_eq!(instructions[0].source, codex(home).join("AGENTS.override.md"));
}

#[test]
fn memories_excludes_raw() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let memories = codex(home).join("memories");
    write_text(&memories.join("MEMORY.md"), "handbook\n");
    write_text(&memories.join("memory_summary.md"), "summary\n");
    write_text(&memories.join("raw_memories.md"), "SECRET-MARKER-raw\n");
    write_text(
        &memories.join("rollout_summaries").join("2026-11-01-chat.md"),
        "rollout\n",
    );

    let result = scan(home, &[]);

    assert_eq!(
        names(&result, FoundKind::Memory),
        vec!["MEMORY.md", "memory_summary.md"]
    );
    assert!(!format!("{:?}", result.found).contains(MARKER));
}

#[test]
fn codex_home_respected() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let custom = home.join("elsewhere").join("codex-home");
    write_text(
        &custom.join("skills").join("here").join("SKILL.md"),
        "---\ndescription: Here\n---\nHere.\n",
    );
    write_text(
        &codex(home).join("skills").join("decoy").join("SKILL.md"),
        "---\ndescription: Decoy\n---\nDecoy.\n",
    );
    let mut source = dirs(home);
    source.codex = custom;

    let result = scan_codex(&GuardedFs::new(), &source, &[]);

    assert_eq!(names(&result, FoundKind::Skill), vec!["here"]);
}

#[test]
fn invalid_toml_is_problem() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_text(
        &codex(home).join("config.toml"),
        "[mcp_servers.broken]\ncommand = \"SECRET-MARKER-cmd\"\nargs = [\n",
    );
    write_text(
        &codex(home).join("skills").join("kept").join("SKILL.md"),
        "---\ndescription: Kept\n---\nKept.\n",
    );

    let result = scan(home, &[]);

    assert_eq!(result.problems.len(), 1);
    assert!(result.problems[0].message.starts_with("invalid TOML in config.toml"));
    assert!(!result.problems[0].message.contains(MARKER));
    assert_eq!(names(&result, FoundKind::Skill), vec!["kept"]);
}

#[test]
fn auth_json_never_opened() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_json(
        &codex(home).join("auth.json"),
        &serde_json::json!({ "tokens": { "access": format!("{MARKER}codex") } }),
    );
    write_text(
        &codex(home).join("skills").join("plain").join("SKILL.md"),
        "---\ndescription: Plain\n---\nPlain.\n",
    );

    let result = scan(home, &[]);

    assert!(result.problems.is_empty());
    assert!(!format!("{:?}", result.found).contains(MARKER));
    assert_eq!(names(&result, FoundKind::Skill), vec!["plain"]);
}

#[test]
fn app_json_listed_unsupported() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_text(
        &codex(home).join("config.toml"),
        "[plugins.\"connected@market\"]\nenabled = true\n",
    );
    let root = codex_plugin_cache(home, "market", "connected", "1.0.0");
    codex_plugin(&root, "connected", "tool");
    write_text(&root.join(".app.json"), "{}");

    let result = scan(home, &[]);

    let app = result
        .found
        .iter()
        .find(|found| found.name == "connected:.app.json")
        .expect("the connector is listed");
    assert_eq!(app.kind, FoundKind::Unsupported);
    assert!(app.unsupported.as_deref().unwrap().contains("ChatGPT account"));
}

#[test]
fn project_codex_config_mcp() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let project = tmp.path().join("work").join("app");
    write_text(
        &project.join(".codex").join("config.toml"),
        "[mcp_servers.proj]\ncommand = \"node\"\nargs = [\"proj.js\"]\n",
    );

    let result = scan(home, std::slice::from_ref(&project));

    let server = result
        .found
        .iter()
        .find(|found| found.name == "proj")
        .expect("the project server is listed");
    assert_eq!(server.project, Some(ProjectRef(0)));
    assert_eq!(server.origin, Origin::Codex);
}

#[test]
fn missing_codex_home_is_empty() {
    let tmp = tempfile::tempdir().unwrap();

    let result = scan_codex(
        &GuardedFs::new(),
        &SourceDirs {
            claude: tmp.path().join(".claude"),
            codex: tmp.path().join("no-codex"),
            agents: tmp.path().join(".claude").join("agents"),
            home: tmp.path().to_path_buf(),
        },
        &[],
    );

    assert!(result.found.is_empty());
    assert!(result.problems.is_empty());
    assert!(fs::metadata(tmp.path().join("no-codex")).is_err());
}
