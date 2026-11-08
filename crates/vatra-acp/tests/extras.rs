use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use vatra_acp::{
    AcpExtras, McpCapabilities, acp_mcp_servers, expand_prompt, first_prompt_preamble,
    permission_checks,
};
use vatra_core::{McpServerConfig, PermissionMode, Rules};
use vatra_engine::{Decision, PermissionPolicy, ToolClass, ToolContext};
use vatra_extensions::parse_skill;

#[test]
fn stdio_http_capability_and_secrets() {
    let stdio = McpServerConfig {
        name: "local".into(),
        command: Some(r"C:\Users\Răzvan Test\tool.exe".into()),
        args: vec!["--stdio".into(), r"C:\Users\Răzvan Test\config.json".into()],
        env: BTreeMap::from([("KEY".into(), "secret-value".into())]),
        ..Default::default()
    };
    let http = McpServerConfig {
        name: "remote".into(),
        url: Some("https://example.test/mcp".into()),
        ..Default::default()
    };
    let (servers, omitted) =
        acp_mcp_servers(&[stdio.clone(), http.clone()], &McpCapabilities::default());
    assert_eq!(servers.len(), 1);
    assert_eq!(
        servers[0]["env"],
        json!([{"name":"KEY","value":"secret-value"}])
    );
    assert_eq!(servers[0]["command"], r"C:\Users\Răzvan Test\tool.exe");
    assert_eq!(
        servers[0]["args"],
        json!(["--stdio", r"C:\Users\Răzvan Test\config.json"])
    );
    assert_eq!(omitted.len(), 1);
    let (servers, omitted) = acp_mcp_servers(&[http], &McpCapabilities { http: true });
    assert_eq!(servers[0]["type"], "http");
    assert!(omitted.is_empty());
}

#[test]
fn unresolved_oauth_is_omitted_instead_of_leaking_configuration() {
    let server = McpServerConfig {
        name: "oauth".into(),
        url: Some("https://example.test".into()),
        oauth: Some(Default::default()),
        ..Default::default()
    };
    let (servers, reasons) = acp_mcp_servers(&[server], &McpCapabilities { http: true });
    assert!(servers.is_empty());
    assert!(reasons[0].contains("proxy"));
    let filtered = McpServerConfig {
        name: "filtered".into(),
        command: Some("tool.exe".into()),
        disabled_tools: vec!["write".into()],
        ..Default::default()
    };
    let (servers, reasons) = acp_mcp_servers(&[filtered], &McpCapabilities::default());
    assert!(servers.is_empty());
    assert!(reasons[0].contains("proxy"));
}

#[test]
fn foreign_skills_expand_and_unknown_slash_stays_unchanged() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("SKILL.md"), "---\nname: review\ndescription: Review files\n---\nReview $ARGUMENTS at ${CLAUDE_SKILL_DIR}").unwrap();
    let skill = parse_skill(folder.path()).unwrap();
    let extras = AcpExtras {
        skills: vec![skill],
        preamble: Some("Project instructions and memory".into()),
        ..Default::default()
    };
    let preamble = first_prompt_preamble(&extras).unwrap();
    assert!(preamble.contains("review") && preamble.contains("Project instructions"));
    let expanded = expand_prompt("/review src", &extras, folder.path(), "session").unwrap();
    assert!(expanded.contains("Review src at"));
    assert!(
        expand_prompt("Use $review", &extras, folder.path(), "session")
            .unwrap()
            .contains("Review ")
    );
    assert_eq!(
        expand_prompt("/unknown src", &extras, folder.path(), "session").unwrap(),
        "/unknown src"
    );
}

fn context(cwd: &std::path::Path) -> ToolContext {
    let (events, _) = tokio::sync::mpsc::unbounded_channel();
    ToolContext {
        cwd: cwd.to_path_buf(),
        project_root: cwd.to_path_buf(),
        session_dir: Default::default(),
        read_files: Default::default(),
        events,
        questions: Arc::new(vatra_engine::NoQuestions),
        tree: Arc::new(vatra_core::ProcessGroup::new().unwrap()),
        background: Arc::new(vatra_engine::Background::new()),
        hidden_env: vec![],
        memory_dir: None,
    }
}

/// What the policy says for the whole call: one refusal in its checks refuses it.
fn denied_in_full_access(tool: serde_json::Value, deny: &[&str], cwd: &std::path::Path) -> bool {
    let (rules, errors) = Rules::parse(&[], &[], deny);
    assert!(errors.is_empty());
    let policy = PermissionPolicy::new(PermissionMode::FullAccess, rules);
    let context = context(cwd);
    permission_checks(&tool, cwd).iter().any(|check| {
        matches!(
            policy.decide(&check.name, check.class, &check.arguments, &context),
            Decision::Deny(_)
        )
    })
}

#[test]
fn deny_rules_win_over_full_access_for_edits() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::create_dir(folder.path().join("secrets")).unwrap();
    let tool = |path: serde_json::Value| json!({"kind":"edit","locations":[{"path":path}]});
    for path in [
        json!("secrets/a.txt"),
        json!(folder.path().join("secrets").join("a.txt")),
        json!(std::fs::canonicalize(folder.path()).unwrap().join("secrets").join("a.txt")),
    ] {
        assert!(denied_in_full_access(tool(path), &["Edit(secrets/**)"], folder.path()));
    }
    assert!(!denied_in_full_access(tool(json!("other.txt")), &["Edit(secrets/**)"], folder.path()));
}

#[test]
fn deny_rules_reach_reads_fetches_searches_and_commands() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let read = json!({"kind":"read","locations":[{"path":"secret.txt"}]});
    assert!(denied_in_full_access(read, &["Read(secret.txt)"], cwd));
    let fetch = json!({"kind":"fetch","rawInput":{"url":"https://evil.test/x"}});
    assert!(denied_in_full_access(fetch, &["WebFetch(domain:evil.test)"], cwd));
    let search = json!({"kind":"search","rawInput":{"path":"secret"}});
    assert!(denied_in_full_access(search, &["Grep(secret)"], cwd));
    for command in [json!("rm -rf x"), json!(["bash", "-lc", "rm -rf x"]), json!(["rm", "-rf", "x"])] {
        let execute = json!({"kind":"execute","rawInput":{"command":command}});
        assert!(denied_in_full_access(execute, &["Bash(rm *)"], cwd));
    }
}

#[test]
fn a_command_without_text_is_never_taken_for_a_harmless_one() {
    let folder = tempfile::tempdir().unwrap();
    let (rules, _) = Rules::parse(&[], &[], &["Bash(rm *)"]);
    let policy = PermissionPolicy::new(PermissionMode::FullAccess, rules);
    let context = context(folder.path());
    let checks = permission_checks(&json!({"kind":"execute","title":"clean up"}), folder.path());
    let check = &checks[0];
    assert_eq!(
        policy.decide(&check.name, check.class, &check.arguments, &context),
        Decision::Ask
    );
}

#[test]
fn edit_checks_are_inside_only_for_every_resolved_location() {
    let folder = tempfile::tempdir().unwrap();
    let inside = folder.path().join("new.txt");
    let check = permission_checks(
        &json!({"kind":"edit","locations":[{"path":inside}]}),
        folder.path(),
    );
    assert_eq!(check[0].name, "Edit");
    assert_eq!(check[0].class, ToolClass::EditInside);
    let check = permission_checks(
        &json!({"kind":"edit","locations":[{"path":"../outside.txt"}]}),
        folder.path(),
    );
    assert_eq!(check[0].class, ToolClass::EditOutside);
    let check = permission_checks(&json!({"kind":"edit"}), folder.path());
    assert_eq!(check[0].class, ToolClass::EditOutside);
}
