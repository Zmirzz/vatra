use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

mod common;

use common::{run_setup, sequence, setup, tool_call};
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::mpsc;
use vatra_core::ProcessGroup;
use vatra_engine::{
    AllowAll, Background, LoadedInstructions, NoQuestions, PromptInput, ToolClass, ToolContext, apply_patch_tool,
    edit_tool, load_instructions, memory_block, read_tool, system_prompt, write_tool,
};

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn load(root: &Path) -> LoadedInstructions {
    load_instructions(root, root, &[])
}

fn count(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

#[test]
fn loads_both_claude_and_agents() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("AGENTS.md"), "agents rule");
    write(&dir.path().join("CLAUDE.md"), "claude rule");
    let loaded = load(dir.path());
    assert!(loaded.text.contains("agents rule"));
    assert!(loaded.text.contains("claude rule"));
    assert_eq!(loaded.files.len(), 2);
}

#[test]
fn identical_content_used_once() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("AGENTS.md"), "same rule");
    write(&dir.path().join("CLAUDE.md"), "same rule");
    assert_eq!(count(&load(dir.path()).text, "same rule"), 1);
}

#[test]
fn root_first_order() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("CLAUDE.md"), "root rule");
    write(&dir.path().join("sub").join("CLAUDE.md"), "sub rule");
    let loaded = load_instructions(dir.path(), &dir.path().join("sub"), &[]);
    let text = loaded.text;
    assert!(text.find("root rule").unwrap() < text.find("sub rule").unwrap(), "{text}");
}

#[test]
fn claude_local_after_claude() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("CLAUDE.local.md"), "private rule");
    write(&dir.path().join("CLAUDE.md"), "shared rule");
    let text = load(dir.path()).text;
    assert!(text.find("shared rule").unwrap() < text.find("private rule").unwrap(), "{text}");
}

#[test]
fn agents_override_replaces_agents() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("AGENTS.md"), "plain agents");
    write(&dir.path().join("AGENTS.override.md"), "override agents");
    let text = load(dir.path()).text;
    assert!(text.contains("override agents"));
    assert!(!text.contains("plain agents"));
}

#[test]
fn imports_up_to_4_levels() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("CLAUDE.md"), "@one.md");
    write(&dir.path().join("one.md"), "level one\n@two.md");
    write(&dir.path().join("two.md"), "level two\n@three.md");
    write(&dir.path().join("three.md"), "level three\n@four.md");
    write(&dir.path().join("four.md"), "level four\n@five.md");
    write(&dir.path().join("five.md"), "level five");
    let text = load(dir.path()).text;
    assert!(text.contains("level four"), "{text}");
    assert!(!text.contains("level five"), "{text}");
}

#[test]
fn import_in_code_block_ignored() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("CLAUDE.md"), "```\n@secret.md\n```\nvisible");
    write(&dir.path().join("secret.md"), "top secret");
    let text = load(dir.path()).text;
    assert!(text.contains("visible"));
    assert!(!text.contains("top secret"), "{text}");
}

#[test]
fn import_cycle_stops() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("CLAUDE.md"), "start\n@a.md");
    write(&dir.path().join("a.md"), "alpha\n@b.md");
    write(&dir.path().join("b.md"), "beta\n@a.md");
    let text = load(dir.path()).text;
    assert_eq!(count(&text, "alpha"), 1, "{text}");
    assert_eq!(count(&text, "beta"), 1, "{text}");
}

#[test]
fn html_comments_stripped() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("CLAUDE.md"), "visible <!-- hidden note --> text");
    let text = load(dir.path()).text;
    assert!(text.contains("visible"));
    assert!(text.contains("text"));
    assert!(!text.contains("hidden note"), "{text}");
}

#[test]
fn rules_without_paths_load() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join(".claude").join("rules").join("plain.md"), "plain rule");
    assert!(load(dir.path()).text.contains("plain rule"));
}

#[tokio::test]
async fn rules_with_paths_wait_for_match() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join(".claude").join("rules").join("rust.md"), "---\npaths: [\"**/*.rs\"]\n---\nrust rule");
    let file = dir.path().join("src").join("main.rs");
    write(&file, "fn main() {}");

    // The matching file exists, yet the rule waits: nothing joins the context at the start.
    let loaded = load(dir.path());
    assert!(!loaded.text.contains("rust rule"));
    assert_eq!(loaded.deferred.len(), 1);

    // A Read of the matching file brings the rule in once, before the next model request.
    let server = sequence(vec![
        tool_call("c1", "Read", &json!({"file_path": file.display().to_string()}).to_string()),
        common::text("ok"),
    ])
    .await;
    let mut setup = setup(&server, std::sync::Arc::new(AllowAll), None, vec![read_tool()], 20).await;
    setup.context.cwd = dir.path().to_path_buf();
    setup.context.project_root = dir.path().to_path_buf();
    setup.instructions = loaded;
    run_setup(setup, vec![], "salut").await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(count(&String::from_utf8_lossy(&requests[0].body), "rust rule"), 0);
    assert_eq!(count(&String::from_utf8_lossy(&requests[1].body), "rust rule"), 1);
}

#[test]
fn over_32_kib_warns() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("CLAUDE.md"), &"a".repeat(33 * 1024));
    let loaded = load(dir.path());
    assert!(!loaded.warnings.is_empty());
}

#[test]
fn import_outside_project_warns_and_skips() {
    let base = TempDir::new().unwrap();
    let project = base.path().join("proj");
    write(&base.path().join("outside.md"), "outside secret");
    write(&project.join("CLAUDE.md"), "@../outside.md");
    let loaded = load(&project);
    assert!(!loaded.warnings.is_empty());
    assert!(!loaded.text.contains("outside secret"), "{}", loaded.text);
}

#[test]
fn index_capped_200_lines_or_25kb() {
    let dir = TempDir::new().unwrap();
    let lines: String = (1..=300).map(|n| format!("entry-{n:03}\n")).collect();
    write(&dir.path().join("MEMORY.md"), &lines);
    let block = memory_block(dir.path()).unwrap();
    assert!(block.contains("entry-200"));
    assert!(!block.contains("entry-201"));

    let wide: String = (1..=60).map(|n| format!("{n:03} {}\n", "x".repeat(1000))).collect();
    write(&dir.path().join("MEMORY.md"), &wide);
    let block = memory_block(dir.path()).unwrap();
    assert!(block.contains("001 "));
    assert!(!block.contains("060 "));
    assert!(block.len() < 26 * 1024, "{} bytes", block.len());
}

#[test]
fn missing_dir_gives_none() {
    let dir = TempDir::new().unwrap();
    assert!(memory_block(&dir.path().join("nope")).is_none());
    assert!(memory_block(dir.path()).is_some());
}

fn context(project: &Path, memory: &Path) -> ToolContext {
    let (events, _) = mpsc::unbounded_channel();
    ToolContext {
        cwd: project.to_path_buf(),
        project_root: project.to_path_buf(),
        session_dir: PathBuf::new(),
        read_files: Arc::new(Mutex::new(HashSet::new())),
        events,
        questions: Arc::new(NoQuestions),
        tree: Arc::new(ProcessGroup::new().unwrap()),
        background: Arc::new(Background::new()),
        hidden_env: vec![],
        memory_dir: Some(memory.to_path_buf()),
    }
}

#[test]
fn memory_tools_allowed_without_prompt() {
    let project = TempDir::new().unwrap();
    let memory = TempDir::new().unwrap();
    let elsewhere = TempDir::new().unwrap();
    let context = context(project.path(), memory.path());

    let note = memory.path().join("notes.md").display().to_string();
    let other = elsewhere.path().join("notes.md").display().to_string();
    let inside = project.path().join("src.rs").display().to_string();
    let write_arguments = |path: &str| json!({"file_path": path, "content": "x"});
    let read_arguments = |path: &str| json!({"file_path": path});

    assert_eq!(read_tool().class(&read_arguments(&note), &context), ToolClass::Free);
    assert_eq!(write_tool().class(&write_arguments(&note), &context), ToolClass::Free);
    assert_eq!(read_tool().class(&read_arguments(&other), &context), ToolClass::ReadOutside);
    assert_eq!(write_tool().class(&write_arguments(&other), &context), ToolClass::EditOutside);
    assert_eq!(write_tool().class(&write_arguments(&inside), &context), ToolClass::EditInside);

    // Edit and apply_patch are free in the memory folder too, and only there.
    let edit_arguments = |path: &str| json!({"file_path": path, "old_string": "a", "new_string": "b"});
    assert_eq!(edit_tool().class(&edit_arguments(&note), &context), ToolClass::Free);
    assert_eq!(edit_tool().class(&edit_arguments(&inside), &context), ToolClass::EditInside);
    let patch = |path: &str| json!({"input": format!("*** Begin Patch\n*** Add File: {path}\n+hi\n*** End Patch")});
    assert_eq!(apply_patch_tool().class(&patch(&note), &context), ToolClass::Free);
    assert_eq!(apply_patch_tool().class(&patch(&other), &context), ToolClass::EditOutside);
}

#[test]
fn shell_named_only_where_the_platform_has_git_bash() {
    let input = |platform| PromptInput {
        cwd: Path::new("/work/app"),
        platform,
        date: "2026-11-01",
        instructions: "",
        memory: None,
        skills_listing: None,
        is_subagent: false,
        deferred_tools: false,
        compact: false,
    };
    assert!(system_prompt(&input("windows")).contains("Git Bash"));
    assert!(!system_prompt(&input("linux")).contains("Git Bash"));
}

#[test]
fn system_prompt_names_folder_and_sections() {
    let input = PromptInput {
        cwd: Path::new("E:/work/app"),
        platform: "windows",
        date: "2026-11-01",
        instructions: "Use tabs in Go files.",
        memory: Some("Memory index entry."),
        skills_listing: None,
        is_subagent: false,
        deferred_tools: false,
        compact: false,
    };
    let text = system_prompt(&input);
    assert!(text.contains("E:/work/app"), "{text}");
    assert!(text.contains("Use tabs in Go files."), "{text}");
    assert!(text.contains("Memory index entry."), "{text}");
    assert!(text.split_whitespace().count() < 2000);
}

#[test]
fn system_prompt_for_subagent_does_not_ask_questions() {
    let input = PromptInput {
        cwd: Path::new("E:/work/app"),
        platform: "windows",
        date: "2026-11-01",
        instructions: "",
        memory: None,
        skills_listing: None,
        is_subagent: true,
        deferred_tools: false,
        compact: false,
    };
    let text = system_prompt(&input);
    assert!(text.contains("report in your final reply"), "{text}");
    assert!(!text.contains("When to ask"), "{text}");
}
