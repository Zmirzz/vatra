use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use vatra_extensions::{
    FormatError, Skill, canonical_tool, canonical_tools, parse_command, parse_skill,
};

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(path)
}

/// Writes a `SKILL.md` into a folder named `demo-skill` and returns the folder with the guard for it.
fn write_skill(text: &str) -> (TempDir, PathBuf) {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("demo-skill");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), text).unwrap();
    (root, dir)
}

fn parse(text: &str) -> Skill {
    let (_root, dir) = write_skill(text);
    parse_skill(&dir).unwrap()
}

#[test]
fn minimal_name_description() {
    let skill = parse("---\nname: review\ndescription: Reviews code\n---\nBody text\n");
    assert_eq!(skill.name, "review");
    assert_eq!(skill.description, "Reviews code");
    assert_eq!(skill.body, "Body text\n");
    assert!(!skill.is_command);
    assert!(skill.user_invocable);
    assert!(skill.allow_implicit);
}

#[test]
fn name_defaults_to_dir() {
    let skill = parse("---\ndescription: No name given\n---\nBody\n");
    assert_eq!(skill.name, "demo-skill");
}

#[test]
fn all_documented_fields() {
    let skill = parse_skill(&fixture("skills/all-fields")).unwrap();
    assert_eq!(skill.name, "release-notes");
    assert_eq!(
        skill.description,
        "Writes release notes from merged pull requests"
    );
    assert_eq!(
        skill.when_to_use.as_deref(),
        Some("When the user asks for release notes or a changelog")
    );
    assert_eq!(skill.argument_hint.as_deref(), Some("[from-tag] [to-tag]"));
    assert_eq!(skill.arguments, ["from", "to"]);
    assert!(skill.disable_model_invocation);
    assert!(!skill.allow_implicit);
    assert!(!skill.user_invocable);
    assert_eq!(skill.allowed_tools, ["Read", "Grep", "Bash(git log *)"]);
    assert_eq!(skill.disallowed_tools, ["Edit"]);
    assert_eq!(skill.model.as_deref(), Some("sonnet"));
    assert_eq!(skill.effort.as_deref(), Some("high"));
    assert!(skill.context_fork);
    assert_eq!(skill.agent.as_deref(), Some("general-purpose"));
    assert_eq!(skill.paths, ["docs/**/*.md", "CHANGELOG.md"]);
    assert_eq!(skill.shell.as_deref(), Some("bash"));
    assert_eq!(
        skill.body,
        "Write notes for the changes between $from and $to.\n"
    );
    assert_eq!(skill.dir, fixture("skills/all-fields"));
    assert!(skill.unknown_fields.is_empty());
}

#[test]
fn allowed_tools_as_string_and_list() {
    let from_string =
        parse("---\ndescription: d\nallowed-tools: Read, Grep Bash(git add *)\n---\n");
    let from_list = parse(
        "---\ndescription: d\nallowed-tools:\n  - Read\n  - Grep\n  - Bash(git add *)\n---\n",
    );
    assert_eq!(
        from_string.allowed_tools,
        ["Read", "Grep", "Bash(git add *)"]
    );
    assert_eq!(from_list.allowed_tools, from_string.allowed_tools);
}

#[test]
fn allowed_tools_canonicalized() {
    let skill = parse("---\ndescription: d\nallowed-tools: shell, apply_patch, Task\n---\n");
    assert_eq!(skill.allowed_tools, ["Bash", "Edit", "Write", "Agent"]);
}

#[test]
fn openai_yaml_policy_disables_implicit() {
    let skill = parse_skill(&fixture("skills/codex-policy")).unwrap();
    assert_eq!(skill.name, "lint-fixer");
    assert!(!skill.allow_implicit);
    assert!(!skill.disable_model_invocation);
    assert!(skill.user_invocable);
}

#[test]
fn opening_dashes_must_be_line_one() {
    let text = "\n---\nname: late\ndescription: not frontmatter\n---\nBody\n";
    let skill = parse(text);
    assert_eq!(skill.name, "demo-skill");
    assert_eq!(skill.description, "");
    assert_eq!(skill.body, text);
}

#[test]
fn crlf_frontmatter() {
    let skill = parse(
        "---\r\nname: crlf\r\ndescription: Windows line endings\r\n---\r\nBody\r\nLine two\r\n",
    );
    assert_eq!(skill.name, "crlf");
    assert_eq!(skill.description, "Windows line endings");
    assert_eq!(skill.body, "Body\nLine two\n");
}

#[test]
fn bom() {
    let skill =
        parse("\u{feff}---\nname: bommed\ndescription: Starts with a byte order mark\n---\nBody\n");
    assert_eq!(skill.name, "bommed");
    assert_eq!(skill.description, "Starts with a byte order mark");
}

#[test]
fn invalid_yaml_is_error_naming_file() {
    let (_root, dir) = write_skill("---\nname: [unclosed\n---\nBody\n");
    let error = parse_skill(&dir).unwrap_err();
    assert!(matches!(error, FormatError::Yaml { .. }));
    assert!(error.to_string().contains("SKILL.md"), "{error}");
}

#[test]
fn unknown_fields_recorded() {
    let skill = parse("---\ndescription: d\nhooks:\n  PreToolUse: []\ncustom-key: 1\n---\n");
    assert_eq!(skill.unknown_fields, ["hooks", "custom-key"]);
}

#[test]
fn long_description_truncated_at_1536() {
    let long = "é".repeat(2000);
    let skill = parse(&format!("---\ndescription: {long}\n---\n"));
    assert_eq!(skill.description.chars().count(), 1536);
    assert!(skill.description.chars().all(|c| c == 'é'));
}

#[test]
fn command_namespace_from_subdirs() {
    let file = fixture("commands/frontend/deploy.md");
    let skill = parse_command(&file, &["frontend".to_string()]).unwrap();
    assert_eq!(skill.name, "frontend:deploy");
    assert_eq!(skill.qualified_name(), "frontend:deploy");
    assert!(skill.is_command);
    assert_eq!(skill.description, "Deploys the frontend to an environment");
    assert_eq!(skill.argument_hint.as_deref(), Some("[environment]"));
    assert_eq!(
        skill.allowed_tools,
        ["Bash(npm run build)", "Bash(npm run deploy)"]
    );
    assert_eq!(
        skill.body,
        "Deploy the frontend to $ARGUMENTS and report the URL.\n"
    );
    assert_eq!(skill.dir, fixture("commands/frontend"));

    let mut plugged = skill;
    plugged.plugin = Some("ops".to_string());
    assert_eq!(plugged.qualified_name(), "ops:frontend:deploy");
}

#[test]
fn command_has_no_name_or_paths() {
    let root = TempDir::new().unwrap();
    let file = root.path().join("deploy.md");
    fs::write(
        &file,
        "---\nname: other\npaths: src/**\ndescription: Deploys\n---\nDeploy.\n",
    )
    .unwrap();
    let skill = parse_command(&file, &[]).unwrap();
    assert_eq!(skill.name, "deploy");
    assert!(skill.paths.is_empty());
    assert_eq!(skill.unknown_fields, ["name", "paths"]);
}

#[test]
fn missing_skill_file_is_read_error() {
    let root = TempDir::new().unwrap();
    let error = parse_skill(root.path()).unwrap_err();
    assert!(matches!(error, FormatError::Read { .. }));
}

#[test]
fn canonical_tool_maps_aliases() {
    assert_eq!(canonical_tool("shell"), "Bash");
    assert_eq!(canonical_tool("exec_command"), "Bash");
    assert_eq!(canonical_tool("apply_patch"), "Edit");
    assert_eq!(canonical_tool("update_plan"), "TodoWrite");
    assert_eq!(canonical_tool("Task"), "Agent");
    assert_eq!(canonical_tool("Read"), "Read");
}

#[test]
fn canonical_tools_expands_patch() {
    assert_eq!(canonical_tools("apply_patch"), ["Edit", "Write"]);
    assert_eq!(canonical_tools("shell"), ["Bash"]);
    assert_eq!(canonical_tools("Read"), ["Read"]);
}

#[test]
fn unquoted_argument_hint_loads() {
    let skill = parse("---\ndescription: [a, b]\nargument-hint: [issue-number]\n---\nBody\n");
    assert_eq!(skill.argument_hint.as_deref(), Some("[issue-number]"));
    assert_eq!(skill.description, "[a, b]");
    assert!(skill.problems.is_empty());
}

#[test]
fn odd_field_values_keep_the_skill() {
    let skill = parse(
        "---\nname: odd\ndescription: Still loads\ncontext: other\ndisable-model-invocation: \"true\"\nuser-invocable: maybe\n---\nBody\n",
    );
    assert_eq!(skill.name, "odd");
    assert_eq!(skill.description, "Still loads");
    assert!(!skill.context_fork);
    assert!(skill.disable_model_invocation);
    assert!(!skill.allow_implicit);
    assert!(skill.user_invocable);
    assert_eq!(skill.problems.len(), 2, "{:?}", skill.problems);
    assert!(
        skill.problems[0].contains("`context`"),
        "{:?}",
        skill.problems
    );
    assert!(
        skill.problems[1].contains("`user-invocable`"),
        "{:?}",
        skill.problems
    );

    let root = TempDir::new().unwrap();
    let dir = root.path().join("policy-skill");
    fs::create_dir_all(dir.join("agents")).unwrap();
    fs::write(dir.join("SKILL.md"), "---\ndescription: d\n---\nBody\n").unwrap();
    fs::write(
        dir.join("agents").join("openai.yaml"),
        "policy:\n  allow_implicit_invocation: maybe\n",
    )
    .unwrap();
    let skill = parse_skill(&dir).unwrap();
    assert!(skill.allow_implicit);
    assert_eq!(skill.problems.len(), 1, "{:?}", skill.problems);
    assert!(
        skill.problems[0].contains("openai.yaml"),
        "{:?}",
        skill.problems
    );

    fs::write(dir.join("agents").join("openai.yaml"), "policy: [\n").unwrap();
    let skill = parse_skill(&dir).unwrap();
    assert!(skill.allow_implicit);
    assert!(
        skill.problems[0].contains("openai.yaml"),
        "{:?}",
        skill.problems
    );
}

#[test]
fn paths_split_on_commas_only() {
    let skill = parse("---\ndescription: d\npaths: src/main.rs, my dir/*.md\n---\n");
    assert_eq!(skill.paths, ["src/main.rs", "my dir/*.md"]);
}

#[test]
fn closing_fence_trailing_spaces() {
    let skill = parse("---\nname: spaced\ndescription: d\n---   \nBody\n");
    assert_eq!(skill.name, "spaced");
    assert_eq!(skill.body, "Body\n");
}
