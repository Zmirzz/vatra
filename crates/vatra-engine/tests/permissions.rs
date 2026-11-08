mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::*;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use vatra_core::{Agent, Event, McpServerConfig, PermissionMode, ProcessGroup, Rule, Rules, TurnRequest};
use vatra_engine::{
    AllowAll, Decision, EngineAgent, McpHub, McpTool, PermissionPolicy, Questioner, Tool, ToolClass, ToolContext,
    ToolOutput, ToolRegistry,
};

use PermissionMode::{AcceptEdits, FullAccess, Supervised};
use ToolClass::{EditInside, EditOutside, Execute, Free, Mcp, ReadOutside, Web};

struct NoQuestions;

#[async_trait]
impl Questioner for NoQuestions {
    async fn ask(&self, _questions: Value) -> Option<Value> {
        None
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).expect("a home folder")
}

fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn context() -> ToolContext {
    let (events, _) = mpsc::unbounded_channel();
    ToolContext {
        cwd: root(),
        project_root: root(),
        session_dir: PathBuf::new(),
        read_files: Arc::new(Mutex::new(HashSet::new())),
        events,
        questions: Arc::new(NoQuestions),
        tree: Arc::new(ProcessGroup::new().unwrap()),
        background: Arc::new(vatra_engine::Background::new()),
        hidden_env: vec![],
        memory_dir: None,
    }
}

fn policy(mode: PermissionMode, allow: &[&str], ask: &[&str], deny: &[&str]) -> PermissionPolicy {
    let (rules, errors) = Rules::parse(allow, ask, deny);
    assert!(errors.is_empty(), "{errors:?}");
    PermissionPolicy::new(mode, rules)
}

fn decide(policy: &PermissionPolicy, name: &str, class: ToolClass, arguments: Value) -> Decision {
    policy.decide(name, class, &arguments, &context())
}

fn bash(command: &str) -> Value {
    json!({ "command": command })
}

#[test]
fn mode_table() {
    let table = [
        (Free, [Decision::Allow, Decision::Allow, Decision::Allow]),
        (ReadOutside, [Decision::Ask, Decision::Ask, Decision::Allow]),
        (Web, [Decision::Ask, Decision::Ask, Decision::Allow]),
        (EditInside, [Decision::Ask, Decision::Allow, Decision::Allow]),
        (EditOutside, [Decision::Ask, Decision::Ask, Decision::Allow]),
        (Execute, [Decision::Ask, Decision::Ask, Decision::Allow]),
        (Mcp, [Decision::Ask, Decision::Ask, Decision::Allow]),
    ];
    for (class, expected) in table {
        for (mode, expected) in [Supervised, AcceptEdits, FullAccess].into_iter().zip(expected) {
            let policy = policy(mode, &[], &[], &[]);
            assert_eq!(decide(&policy, "Tool", class, json!({})), expected, "{class:?} in {mode:?}");
        }
    }
}

#[test]
fn deny_wins_over_allow() {
    let policy = policy(Supervised, &["Bash(ls *)"], &[], &["Bash(ls *)"]);
    assert!(matches!(decide(&policy, "Bash", Execute, bash("ls -la")), Decision::Deny(_)));
}

#[test]
fn ask_wins_over_allow() {
    let policy = policy(FullAccess, &["Bash(ls *)"], &["Bash(ls *)"], &[]);
    assert_eq!(decide(&policy, "Bash", Execute, bash("ls")), Decision::Ask);
}

#[test]
fn bash_prefix() {
    let policy = policy(Supervised, &["Bash(ls *)"], &[], &[]);
    assert_eq!(decide(&policy, "Bash", Execute, bash("ls")), Decision::Allow);
    assert_eq!(decide(&policy, "Bash", Execute, bash("ls -la")), Decision::Allow);
    assert_eq!(decide(&policy, "Bash", Execute, bash("lsof -i")), Decision::Ask);
}

#[test]
fn bash_compound_every_part_must_pass() {
    let policy = policy(Supervised, &["Bash(git *)"], &[], &[]);
    assert_eq!(decide(&policy, "Bash", Execute, bash("git status && rm -rf x")), Decision::Ask);
    assert_eq!(decide(&policy, "Bash", Execute, bash("git status; git log")), Decision::Allow);
    assert_eq!(decide(&policy, "Bash", Execute, bash("git status | head")), Decision::Ask);
    assert_eq!(decide(&policy, "Bash", Execute, bash("git status\nrm x")), Decision::Ask);
}

#[test]
fn bash_quotes_keep_separators_inside_one_part() {
    let policy = policy(Supervised, &["Bash(echo *)"], &[], &[]);
    assert_eq!(decide(&policy, "Bash", Execute, bash("echo 'a && rm y'")), Decision::Allow);
    assert_eq!(decide(&policy, "Bash", Execute, bash("echo \"a\" 2>&1")), Decision::Allow);
}

#[test]
fn bash_unparseable_asks() {
    let policy = policy(Supervised, &["Bash(echo *)"], &[], &[]);
    assert_eq!(decide(&policy, "Bash", Execute, bash("echo $(rm x)")), Decision::Ask);
    assert_eq!(decide(&policy, "Bash", Execute, bash("echo `rm x`")), Decision::Ask);
    assert_eq!(decide(&policy, "Bash", Execute, bash("echo <(ls)")), Decision::Ask);
}

#[test]
fn bash_wrappers_and_assignments_are_seen_through() {
    let allow = policy(Supervised, &["Bash(ls *)"], &[], &[]);
    assert_eq!(decide(&allow, "Bash", Execute, bash("timeout 5 ls -la")), Decision::Allow);
    assert_eq!(decide(&allow, "Bash", Execute, bash("LANG=C nohup ls")), Decision::Allow);
    let deny = policy(FullAccess, &[], &[], &["Bash(rm *)"]);
    assert!(matches!(decide(&deny, "Bash", Execute, bash("timeout 5 rm x")), Decision::Deny(_)));
}

#[test]
fn read_path_rule() {
    let absolute = format!("Read(//{}/src/**)", slashed(&root()));
    let project = "Read(/src/**)";
    let deny_home = format!("Read(~/{}/**)", ".ssh");
    let policy = policy(Supervised, &[absolute.as_str(), project], &[], &[deny_home.as_str()]);

    let inside = root().join("src").join("lib.rs").display().to_string();
    let outside = root().join("Cargo.toml").display().to_string();
    assert_eq!(decide(&policy, "Read", ReadOutside, json!({ "file_path": inside })), Decision::Allow);
    assert_eq!(decide(&policy, "Read", ReadOutside, json!({ "file_path": outside })), Decision::Ask);
    let secret = home().join(".ssh").join("id_rsa").display().to_string();
    assert!(matches!(decide(&policy, "Read", ReadOutside, json!({ "file_path": secret })), Decision::Deny(_)));
}

#[test]
fn relative_read_rule_is_one_segment_deep() {
    let policy = policy(Supervised, &["Read(src/*.rs)"], &[], &[]);
    assert_eq!(decide(&policy, "Read", ReadOutside, json!({ "file_path": "src/main.rs" })), Decision::Allow);
    assert_eq!(decide(&policy, "Read", ReadOutside, json!({ "file_path": "src/deep/x.rs" })), Decision::Ask);
}

#[test]
fn edit_rule() {
    let policy = policy(Supervised, &["Edit(/src/**)"], &[], &[]);
    assert_eq!(decide(&policy, "Write", EditInside, json!({ "file_path": "src/a.rs" })), Decision::Allow);
    assert_eq!(decide(&policy, "Edit", EditInside, json!({ "file_path": "src/a.rs" })), Decision::Allow);
    assert_eq!(decide(&policy, "NotebookEdit", EditInside, json!({ "notebook_path": "src/n.ipynb" })), Decision::Allow);
    assert_eq!(decide(&policy, "Edit", EditInside, json!({ "file_path": "Cargo.toml" })), Decision::Ask);
}

#[test]
fn webfetch_domain() {
    let policy = policy(Supervised, &["WebFetch(domain:*.example.com)"], &[], &[]);
    assert_eq!(decide(&policy, "WebFetch", Web, json!({ "url": "https://docs.example.com/page" })), Decision::Allow);
    assert_eq!(decide(&policy, "WebFetch", Web, json!({ "url": "https://example.org/" })), Decision::Ask);
    assert_eq!(decide(&policy, "WebFetch", Web, json!({ "url": "https://example.com.evil.net/" })), Decision::Ask);
}

#[test]
fn mcp_server_wildcard() {
    let tool_wildcard = policy(Supervised, &["mcp__github__*"], &[], &[]);
    assert_eq!(decide(&tool_wildcard, "mcp__github__create_issue", Mcp, json!({})), Decision::Allow);
    assert_eq!(decide(&tool_wildcard, "mcp__gitlab__create", Mcp, json!({})), Decision::Ask);

    let server = policy(Supervised, &["mcp__github"], &[], &[]);
    assert_eq!(decide(&server, "mcp__github__list", Mcp, json!({})), Decision::Allow);
    assert_eq!(decide(&server, "mcp__githubx__list", Mcp, json!({})), Decision::Ask);
}

#[test]
fn skill_rule() {
    let deny = policy(Supervised, &[], &[], &["Skill(danger)"]);
    assert!(matches!(decide(&deny, "Skill", Free, json!({ "skill": "danger" })), Decision::Deny(_)));
    assert_eq!(decide(&deny, "Skill", Free, json!({ "skill": "fine" })), Decision::Allow);

    let allow = policy(Supervised, &["Skill(commit)"], &[], &[]);
    assert_eq!(decide(&allow, "Skill", ReadOutside, json!({ "skill": "commit" })), Decision::Allow);
    assert_eq!(decide(&allow, "Skill", ReadOutside, json!({ "skill": "other" })), Decision::Ask);
}

#[test]
fn unknown_rule_is_reported_not_fatal() {
    assert!(Rule::parse("Frobnicate(x)").is_err());
    let (rules, errors) = Rules::parse(&["Frobnicate(x)", "Bash(ls *)"], &[], &[]);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("Frobnicate"), "{errors:?}");
    let policy = PermissionPolicy::new(Supervised, rules);
    assert_eq!(decide(&policy, "Bash", Execute, bash("ls")), Decision::Allow);
}

#[test]
fn turn_grant_applies_until_cleared() {
    let policy = policy(Supervised, &[], &[], &[]);
    assert_eq!(decide(&policy, "Bash", Execute, bash("make test")), Decision::Ask);

    policy.grant_for_turn(Rule::parse("Bash(make *)").unwrap());
    assert_eq!(decide(&policy, "Bash", Execute, bash("make test")), Decision::Allow);
    assert_eq!(decide(&policy, "Bash", Execute, bash("make test && rm x")), Decision::Ask);

    policy.clear_turn_grants();
    assert_eq!(decide(&policy, "Bash", Execute, bash("make test")), Decision::Ask);
}

#[test]
fn grant_never_overrides_deny() {
    let policy = policy(Supervised, &[], &[], &["Bash(rm *)"]);
    policy.grant_for_turn(Rule::parse("Bash(rm *)").unwrap());
    assert!(matches!(decide(&policy, "Bash", Execute, bash("rm x")), Decision::Deny(_)));
}

#[tokio::test]
async fn no_read_only_hint_shortcut() {
    let server = McpServerConfig {
        name: "demo".into(),
        command: Some(env!("CARGO_BIN_EXE_vatra-fixture-mcp").into()),
        ..Default::default()
    };
    let hub = Arc::new(McpHub::new(vec![server], vatra_engine::ExpandVars::default()));
    hub.ensure_connected().await;
    let tools = McpTool::all(&hub);
    let lookup = tools.iter().find(|tool| tool.def().name == "mcp__demo__lookup").expect("the lookup tool");
    assert_eq!(lookup.class(&json!({}), &context()), Mcp);
}

struct Named(&'static str);

#[async_trait]
impl Tool for Named {
    fn def(&self) -> vatra_engine::ToolDef {
        vatra_engine::ToolDef { name: self.0.into(), description: String::new(), parameters: json!({}) }
    }

    fn class(&self, _arguments: &Value, _context: &ToolContext) -> ToolClass {
        Free
    }

    fn parallel_safe(&self) -> bool {
        true
    }

    async fn call(&self, _arguments: Value, _context: &ToolContext) -> ToolOutput {
        ToolOutput { text: String::new(), is_error: false, images: vec![] }
    }
}

#[test]
fn registry_resolves_aliases_and_lists_defs_in_order() {
    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(Named("read")));
    registry.register(Arc::new(Named("write")));
    registry.alias("Read", "read");

    assert_eq!(registry.get("read").unwrap().def().name, "read");
    assert_eq!(registry.get("Read").unwrap().def().name, "read");
    assert!(registry.get("nope").is_none());
    let names: Vec<_> = registry.defs().into_iter().map(|def| def.name).collect();
    assert_eq!(names, ["read", "write"]);
}

#[tokio::test]
async fn a_deny_rule_refuses_the_call_before_the_approver_is_asked() {
    let server = sequence(vec![tool_call("c1", "mcp__demo__echo", r#"{"text":"x"}"#), text("ok")]).await;
    let mut setup = setup(&server, Arc::new(AllowAll), None, vec![], 20).await;
    setup.policy = Arc::new(policy(Supervised, &[], &[], &["mcp__demo__*"]));
    let agent = EngineAgent::new(setup);

    let (sink, mut received) = mpsc::unbounded_channel();
    let request = TurnRequest { files: vec![], history: vec![], prompt: "salut".into(), native_session: None };
    agent.run_turn(request, sink).await.unwrap();
    let mut events = vec![];
    while let Some(event) = received.recv().await {
        events.push(event);
    }

    assert!(events.iter().any(|event| matches!(
        event,
        Event::ToolResult { id, output, is_error: true } if id == "c1" && output != "x"
    )));
    assert!(!events.iter().any(|event| matches!(event, Event::ToolDenied { .. })));
}

#[test]
fn ansi_c_and_locale_quotes_are_unparseable() {
    let rules = policy(Supervised, &["Bash(echo *)"], &[], &[]);
    assert_eq!(decide(&rules, "Bash", Execute, bash("echo $'a\\'b'; rm x")), Decision::Ask);
    assert_eq!(decide(&rules, "Bash", Execute, bash("echo $\"x\"")), Decision::Ask);

    let deny = policy(FullAccess, &[], &[], &["Bash(rm *)"]);
    assert!(matches!(decide(&deny, "Bash", Execute, bash("echo $'a\\'b'; rm x")), Decision::Deny(_)));
}

#[test]
fn redirects_are_not_covered_by_allow_rules() {
    let rules = policy(Supervised, &["Bash(echo *)"], &[], &[]);
    assert_eq!(decide(&rules, "Bash", Execute, bash("echo x > ~/.bashrc")), Decision::Ask);
    assert_eq!(decide(&rules, "Bash", Execute, bash("echo x >> out.txt")), Decision::Ask);
    assert_eq!(decide(&rules, "Bash", Execute, bash("echo x < in")), Decision::Ask);
    assert_eq!(decide(&rules, "Bash", Execute, bash("echo x 2>&1")), Decision::Allow);
    assert_eq!(decide(&rules, "Bash", Execute, bash("echo x >&2")), Decision::Allow);
    assert_eq!(decide(&rules, "Bash", Execute, bash("echo x >&2.bashrc")), Decision::Ask);
    assert_eq!(decide(&rules, "Bash", Execute, bash("echo x >&2;echo y")), Decision::Allow);
    assert_eq!(decide(&rules, "Bash", Execute, bash("echo '>' x")), Decision::Allow);

    let deny = policy(FullAccess, &[], &[], &["Bash(echo *)"]);
    assert!(matches!(decide(&deny, "Bash", Execute, bash("echo x > f")), Decision::Deny(_)));
}

#[test]
fn only_safe_assignments_keep_allow_rules_matching() {
    let rules = policy(Supervised, &["Bash(ls *)"], &[], &[]);
    assert_eq!(decide(&rules, "Bash", Execute, bash("LANG=C ls")), Decision::Allow);
    assert_eq!(decide(&rules, "Bash", Execute, bash("LC_ALL=C TZ=UTC ls -la")), Decision::Allow);
    assert_eq!(decide(&rules, "Bash", Execute, bash("PATH=./evil ls")), Decision::Ask);
    assert_eq!(decide(&rules, "Bash", Execute, bash("LD_PRELOAD=./x.so ls")), Decision::Ask);
    assert_eq!(decide(&rules, "Bash", Execute, bash("BASH_ENV=./x ls")), Decision::Ask);

    let deny = policy(FullAccess, &[], &[], &["Bash(ls *)"]);
    assert!(matches!(decide(&deny, "Bash", Execute, bash("PATH=./evil ls")), Decision::Deny(_)));
}

#[test]
fn dot_dot_segments_are_resolved_before_matching() {
    let edit = policy(Supervised, &["Edit(/src/**)"], &[], &[]);
    assert_eq!(decide(&edit, "Edit", EditInside, json!({ "file_path": "src/a/../b.rs" })), Decision::Allow);
    assert_eq!(decide(&edit, "Edit", EditInside, json!({ "file_path": "src/../../outside" })), Decision::Ask);

    let secrets = policy(Supervised, &[], &[], &["Read(~/.ssh/**)"]);
    let traversed = home().join("x").join("..").join(".ssh").join("id_rsa").display().to_string();
    assert!(matches!(decide(&secrets, "Read", ReadOutside, json!({ "file_path": traversed })), Decision::Deny(_)));
}

#[cfg(windows)]
#[test]
fn windows_paths_ignore_case() {
    let rules = policy(Supervised, &[], &[], &["Read(C:/secrets/**)"]);
    assert!(matches!(decide(&rules, "Read", ReadOutside, json!({ "file_path": "c:/Secrets/key" })), Decision::Deny(_)));
}

#[test]
fn hosts_are_compared_as_the_client_reads_them() {
    let allow = policy(Supervised, &["WebFetch(domain:docs.example.com)"], &[], &[]);
    let smuggled = "https://evil.com\\@docs.example.com/";
    assert_eq!(decide(&allow, "WebFetch", Web, json!({ "url": smuggled })), Decision::Ask);
    let evil = policy(FullAccess, &[], &[], &["WebFetch(domain:evil.com)"]);
    assert!(matches!(decide(&evil, "WebFetch", Web, json!({ "url": smuggled })), Decision::Deny(_)));

    let deny = policy(FullAccess, &[], &[], &["WebFetch(domain:example.com)"]);
    assert!(matches!(decide(&deny, "WebFetch", Web, json!({ "url": "https://EXAMPLE.com/" })), Decision::Deny(_)));
    assert!(matches!(decide(&deny, "WebFetch", Web, json!({ "url": "https://example.com./" })), Decision::Deny(_)));

    let wildcard = policy(FullAccess, &[], &[], &["WebFetch(domain:*.example.com)"]);
    assert!(matches!(
        decide(&wildcard, "WebFetch", Web, json!({ "url": "https://docs.example.com./" })),
        Decision::Deny(_)
    ));
}

#[test]
fn unparseable_commands_are_judged_by_their_deny_prefix() {
    let deny = policy(FullAccess, &[], &[], &["Bash(rm *)"]);
    assert!(matches!(decide(&deny, "Bash", Execute, bash("echo $(rm x)")), Decision::Deny(_)));
    assert_eq!(decide(&deny, "Bash", Execute, bash("echo $(ls x)")), Decision::Ask);

    let ask = policy(FullAccess, &[], &["Bash(git *)"], &[]);
    assert_eq!(decide(&ask, "Bash", Execute, bash("echo $(ls)")), Decision::Ask);

    let open = policy(FullAccess, &[], &[], &[]);
    assert_eq!(decide(&open, "Bash", Execute, bash("echo $(ls)")), Decision::Allow);
    let supervised = policy(Supervised, &[], &[], &[]);
    assert_eq!(decide(&supervised, "Bash", Execute, bash("echo $(ls)")), Decision::Ask);
}
