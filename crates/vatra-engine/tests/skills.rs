mod common;

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use common::*;
use serde_json::Value;
use tokio::sync::mpsc;
use vatra_core::{Agent, Event, PermissionMode, ProcessGroup, Rule, Rules, TurnRequest};
use vatra_engine::model::ContextWindow;
use vatra_engine::{
    AllowAll, Approver, Background, DenyAll, EngineAgent, EngineExtensions, EngineSetup, InjectError, NoQuestions,
    PermissionPolicy, ShellCtx, SkillSession, ToolContext, bash_tool, grep_tool, read_tool, resolve_prompt,
    run_injections, skill_tool, skills_listing,
};
use vatra_extensions::{Catalog, Origin, Scope, Skill, UsageCounts, parse_command, parse_skill};

fn window(tokens: u64) -> ContextWindow {
    ContextWindow {
        tokens,
        estimated: false,
    }
}

/// A skill folder `name` with the given SKILL.md text, read the way a catalog copy is read.
fn skill(root: &Path, name: &str, text: &str) -> Skill {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), text).unwrap();
    parse_skill(&dir).unwrap()
}

fn listed(listing: &str, name: &str) -> bool {
    listing
        .lines()
        .any(|line| line.trim_start_matches("- ").split(':').next() == Some(name))
}

/// The description a listed skill shows, or None when the listing shows only its name.
fn described(listing: &str, name: &str) -> Option<String> {
    listing.lines().find_map(|line| {
        let rest = line.strip_prefix("- ")?;
        let (listed, text) = rest.split_once(':')?;
        (listed == name).then(|| text.trim().to_string())
    })
}

fn extensions(skills: Vec<Skill>) -> Arc<EngineExtensions> {
    let mut extensions = EngineExtensions::default();
    extensions.skills = skills;
    extensions.session = Arc::new(SkillSession::new("7"));
    Arc::new(extensions)
}

/// The setup of an agent with the Skill tool and Bash, as a real turn gets them, with the project in `dir`.
async fn skill_setup(
    server: &wiremock::MockServer,
    dir: &Path,
    skills: Vec<Skill>,
    approver: Arc<dyn Approver>,
) -> EngineSetup {
    let ext = extensions(skills);
    let mut setup = setup(server, approver, None, vec![bash_tool(), read_tool(), grep_tool()], 20).await;
    setup.registry.register(skill_tool(Arc::clone(&ext)));
    setup.context.cwd = dir.to_path_buf();
    setup.context.project_root = dir.to_path_buf();
    setup.extensions = Some(ext);
    setup
}

/// The user and system texts of one request the mock server received.
fn texts(request: &wiremock::Request, role: &str) -> Vec<String> {
    let body: Value = request.body_json().unwrap();
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == role)
        .filter_map(|message| message["content"].as_str().map(str::to_string))
        .collect()
}

fn tool_names(request: &wiremock::Request) -> Vec<String> {
    let body: Value = request.body_json().unwrap();
    body["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

async fn turn(agent: &EngineAgent, history: Vec<Event>, prompt: &str) -> Vec<Event> {
    let (sink, mut received) = mpsc::unbounded_channel();
    let request = TurnRequest { files: vec![],
        history,
        prompt: prompt.into(),
        native_session: None,
    };
    agent.run_turn(request, sink).await.unwrap();
    let mut events = vec![];
    while let Some(event) = received.recv().await {
        events.push(event);
    }
    events
}

// ---------------------------------------------------------------- listing

#[test]
fn names_always_listed() {
    let dir = tempfile::tempdir().unwrap();
    let long = "d".repeat(500);
    let skills: Vec<Skill> = (0..300)
        .map(|index| {
            skill(
                dir.path(),
                &format!("skill-{index}"),
                &format!("---\ndescription: {long}\n---\nbody\n"),
            )
        })
        .collect();
    let listing = skills_listing(&skills, window(10_000), &UsageCounts::default());
    for index in 0..300 {
        assert!(
            listed(&listing, &format!("skill-{index}")),
            "skill-{index} is missing from the listing"
        );
    }
}

#[test]
fn least_used_descriptions_dropped_first() {
    let dir = tempfile::tempdir().unwrap();
    let text = "x".repeat(1200);
    let alpha = skill(dir.path(), "alpha", &format!("---\ndescription: {text}\n---\nbody\n"));
    let beta = skill(dir.path(), "beta", &format!("---\ndescription: {text}\n---\nbody\n"));
    // 10,000 tokens leave 2,000 characters, which fit one description and not two.
    let usage = UsageCounts::from_pairs([("beta".to_string(), 5)]);
    let listing = skills_listing(&[alpha, beta], window(10_000), &usage);
    assert!(
        described(&listing, "beta").is_some_and(|text| text.len() == 1200),
        "{listing}"
    );
    assert_eq!(described(&listing, "alpha"), None, "{listing}");
}

#[test]
fn disable_model_invocation_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let hidden = skill(
        dir.path(),
        "hidden",
        "---\ndescription: hidden\ndisable-model-invocation: true\n---\nbody\n",
    );
    let listing = skills_listing(&[hidden], window(10_000), &UsageCounts::default());
    assert!(!listed(&listing, "hidden"), "{listing}");
}

#[test]
fn user_invocable_false_listed() {
    let dir = tempfile::tempdir().unwrap();
    let quiet = skill(
        dir.path(),
        "quiet",
        "---\ndescription: for the model only\nuser-invocable: false\n---\nbody\n",
    );
    let listing = skills_listing(&[quiet], window(10_000), &UsageCounts::default());
    assert!(listed(&listing, "quiet"), "{listing}");
}

#[test]
fn allow_implicit_false_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let codex = skill(dir.path(), "codex", "---\ndescription: codex policy\n---\nbody\n");
    let policy = dir.path().join("codex").join("agents");
    fs::create_dir_all(&policy).unwrap();
    fs::write(
        policy.join("openai.yaml"),
        "policy:\n  allow_implicit_invocation: false\n",
    )
    .unwrap();
    let codex = parse_skill(&codex.dir).unwrap();
    assert!(!codex.allow_implicit);
    let listing = skills_listing(&[codex], window(10_000), &UsageCounts::default());
    assert!(!listed(&listing, "codex"), "{listing}");
}

#[tokio::test]
async fn paths_skill_hidden_until_match() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("docs")).unwrap();
    fs::write(dir.path().join("docs").join("a.md"), "# a\n").unwrap();
    let docs = skill(
        dir.path(),
        "docs-style",
        "---\ndescription: style for docs\npaths: \"docs/**/*.md\"\n---\nbody\n",
    );
    let server = sequence(vec![text("no read yet")]).await;
    let setup = skill_setup(&server, dir.path(), vec![docs], Arc::new(AllowAll)).await;
    turn(&EngineAgent::new(setup), vec![], "hello").await;
    let requests = server.received_requests().await.unwrap();
    assert!(!texts(&requests[0], "system").join("\n").contains("docs-style"));
}

#[tokio::test]
async fn path_match_adds_at_next_step() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("docs")).unwrap();
    fs::write(dir.path().join("docs").join("a.md"), "# a\n").unwrap();
    let docs = skill(
        dir.path(),
        "docs-style",
        "---\ndescription: style for docs\npaths: \"docs/**/*.md\"\n---\nbody\n",
    );
    let server = sequence(vec![
        tool_call("r1", "Read", r#"{"file_path":"docs/a.md"}"#),
        text("read it"),
    ])
    .await;
    let setup = skill_setup(&server, dir.path(), vec![docs], Arc::new(AllowAll)).await;
    turn(&EngineAgent::new(setup), vec![], "read the docs").await;
    let requests = server.received_requests().await.unwrap();
    assert!(!texts(&requests[0], "system").join("\n").contains("docs-style"));
    assert!(texts(&requests[1], "system").join("\n").contains("docs-style"));
}

// ---------------------------------------------------------------- invocation

#[tokio::test]
async fn slash_expands_with_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let review = skill(
        dir.path(),
        "review",
        "---\ndescription: reviews code\n---\nReview $ARGUMENTS carefully.\n",
    );
    let server = sequence(vec![text("done")]).await;
    let setup = skill_setup(&server, dir.path(), vec![review], Arc::new(AllowAll)).await;
    let events = turn(&EngineAgent::new(setup), vec![], "/review src/a.rs").await;
    let requests = server.received_requests().await.unwrap();
    assert!(
        texts(&requests[0], "user")
            .iter()
            .any(|text| text.contains("Review src/a.rs carefully."))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::SkillLoaded { name, .. } if name == "review"))
    );
}

#[tokio::test]
async fn dollar_expands() {
    let dir = tempfile::tempdir().unwrap();
    let review = skill(
        dir.path(),
        "review",
        "---\ndescription: reviews code\n---\nRules of the review.\n",
    );
    let server = sequence(vec![text("done")]).await;
    let setup = skill_setup(&server, dir.path(), vec![review], Arc::new(AllowAll)).await;
    turn(&EngineAgent::new(setup), vec![], "Please use $review on this file").await;
    let requests = server.received_requests().await.unwrap();
    let user = texts(&requests[0], "user");
    assert!(
        user.iter().any(|text| text.contains("Rules of the review.")),
        "{user:?}"
    );
    assert!(user.iter().any(|text| text == "Please use $review on this file"));
}

#[tokio::test]
async fn unknown_slash_goes_to_model_as_text() {
    let dir = tempfile::tempdir().unwrap();
    let review = skill(dir.path(), "review", "---\ndescription: reviews code\n---\nbody\n");
    let server = sequence(vec![text("done")]).await;
    let setup = skill_setup(&server, dir.path(), vec![review], Arc::new(AllowAll)).await;
    let events = turn(&EngineAgent::new(setup), vec![], "/nope x").await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(texts(&requests[0], "user"), vec!["/nope x".to_string()]);
    assert!(!events.iter().any(|event| matches!(event, Event::SkillLoaded { .. })));
}

#[tokio::test]
async fn disable_model_invocation_refused_for_model() {
    let dir = tempfile::tempdir().unwrap();
    let secret = skill(
        dir.path(),
        "secret",
        "---\ndescription: person only\ndisable-model-invocation: true\n---\nTOP-SECRET-BODY\n",
    );
    let server = sequence(vec![tool_call("s1", "Skill", r#"{"skill":"secret"}"#), text("refused")]).await;
    let setup = skill_setup(&server, dir.path(), vec![secret], Arc::new(AllowAll)).await;
    turn(&EngineAgent::new(setup), vec![], "use it").await;
    let requests = server.received_requests().await.unwrap();
    let results = tool_contents(&requests[1]);
    assert_eq!(results.len(), 1);
    assert!(results[0].contains("can only be run by the person"), "{results:?}");
    assert!(!results[0].contains("TOP-SECRET-BODY"));
}

#[tokio::test]
async fn full_body_loaded_only_on_invocation() {
    let dir = tempfile::tempdir().unwrap();
    let review = skill(
        dir.path(),
        "review",
        "---\ndescription: reviews code\n---\nFULL-BODY-TEXT\n",
    );
    let server = sequence(vec![text("done")]).await;
    let setup = skill_setup(&server, dir.path(), vec![review], Arc::new(AllowAll)).await;
    turn(&EngineAgent::new(setup), vec![], "hello").await;
    let requests = server.received_requests().await.unwrap();
    let system = texts(&requests[0], "system").join("\n");
    assert!(system.contains("reviews code"));
    assert!(!system.contains("FULL-BODY-TEXT"));
}

#[test]
fn plugin_namespace() {
    let dir = tempfile::tempdir().unwrap();
    let mut review = skill(dir.path(), "review", "---\ndescription: reviews code\n---\nbody\n");
    review.plugin = Some("demo".into());
    let resolved = resolve_prompt("/demo:review src", &[review]);
    let invocation = resolved.command.expect("the plugin skill is found");
    assert_eq!(invocation.skill.qualified_name(), "demo:review");
    assert_eq!(invocation.arguments, "src");
}

#[test]
fn skill_beats_command() {
    let dir = tempfile::tempdir().unwrap();
    let as_skill = skill(dir.path(), "review", "---\ndescription: a skill\n---\nSKILL-BODY\n");
    let commands = dir.path().join("commands");
    fs::create_dir_all(&commands).unwrap();
    fs::write(
        commands.join("review.md"),
        "---\ndescription: a command\n---\nCOMMAND-BODY\n",
    )
    .unwrap();
    let as_command = parse_command(&commands.join("review.md"), &[]).unwrap();
    let resolved = resolve_prompt("/review", &[as_command, as_skill]);
    let invocation = resolved.command.expect("one of them is found");
    assert!(!invocation.skill.is_command);
    assert!(invocation.skill.body.contains("SKILL-BODY"));
}

#[tokio::test]
async fn allowed_tools_grant_clears_next_turn() {
    let dir = tempfile::tempdir().unwrap();
    let grant = skill(
        dir.path(),
        "grant",
        "---\ndescription: runs echo\nallowed-tools: Bash(echo *)\n---\nBody.\n",
    );
    let server = sequence(vec![
        tool_call("b1", "Bash", r#"{"command":"echo granted"}"#),
        text("first done"),
        tool_call("b2", "Bash", r#"{"command":"echo granted"}"#),
        text("second done"),
    ])
    .await;
    let setup = skill_setup(&server, dir.path(), vec![grant], Arc::new(DenyAll)).await;
    let agent = EngineAgent::new(setup);
    turn(&agent, vec![], "/grant").await;
    turn(&agent, vec![], "again").await;
    let requests = server.received_requests().await.unwrap();
    assert!(
        tool_contents(&requests[1])[0].contains("granted"),
        "the first turn's call ran"
    );
    assert!(
        tool_contents(&requests[3])[0].contains("refused"),
        "the next turn asks again"
    );
}

#[tokio::test]
async fn disallowed_tools_removed_for_turn() {
    let dir = tempfile::tempdir().unwrap();
    let nobash = skill(
        dir.path(),
        "nobash",
        "---\ndescription: no shell\ndisallowed-tools: Bash\n---\nBody.\n",
    );
    let server = sequence(vec![tool_call("s1", "Skill", r#"{"skill":"nobash"}"#), text("done")]).await;
    let setup = skill_setup(&server, dir.path(), vec![nobash], Arc::new(AllowAll)).await;
    turn(&EngineAgent::new(setup), vec![], "use it").await;
    let requests = server.received_requests().await.unwrap();
    assert!(tool_names(&requests[0]).contains(&"Bash".to_string()));
    assert!(!tool_names(&requests[1]).contains(&"Bash".to_string()));
}

#[tokio::test]
async fn skill_from_disabled_extension_grants_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(&state.path().join("vatra.db"), state.path()).unwrap();
    // Both skills allow Bash. One is switched off, the other runs code that nobody approved.
    let off = skill(
        source.path(),
        "off",
        "---\ndescription: off\nallowed-tools: Bash\n---\nBody.\n",
    );
    let unapproved = skill(
        source.path(),
        "unapproved",
        "---\ndescription: code\nallowed-tools: Bash\n---\nNow:\n!`echo hi`\n",
    );
    catalog
        .add_folder(&off.dir, Origin::Manual, Scope::Global, false)
        .unwrap();
    catalog
        .add_folder(&unapproved.dir, Origin::Manual, Scope::Global, false)
        .unwrap();
    let id = catalog
        .list()
        .unwrap()
        .iter()
        .find(|entry| entry.name == "off")
        .unwrap()
        .id;
    catalog.set_enabled(id, false).unwrap();
    let active = catalog.active_for(1).unwrap();
    assert!(active.skills.is_empty(), "the catalog left out both skills");

    let mut extensions = EngineExtensions::default();
    extensions.skills = active.skills;
    extensions.session = Arc::new(SkillSession::new("7"));
    let extensions = Arc::new(extensions);
    let server = sequence(vec![
        tool_call("s1", "Skill", r#"{"skill":"off"}"#),
        tool_call("s2", "Skill", r#"{"skill":"unapproved"}"#),
        tool_call("b1", "Bash", r#"{"command":"echo hi"}"#),
        text("done"),
    ])
    .await;
    let mut built = setup(&server, Arc::new(DenyAll), None, vec![bash_tool()], 20).await;
    built.registry.register(skill_tool(Arc::clone(&extensions)));
    built.context.cwd = dir.path().to_path_buf();
    built.context.project_root = dir.path().to_path_buf();
    built.extensions = Some(extensions);
    turn(&EngineAgent::new(built), vec![], "use it").await;
    let requests = server.received_requests().await.unwrap();
    let results = tool_contents(&requests[3]);
    assert!(results[0].contains("`off`"), "{results:?}");
    assert!(results[1].contains("`unapproved`"), "{results:?}");
    assert!(results[2].contains("refused"), "{results:?}");
}

#[tokio::test]
async fn invoked_skill_survives_compaction() {
    let dir = tempfile::tempdir().unwrap();
    let server = sequence(vec![text("the summary")]).await;
    let setup = skill_setup(&server, dir.path(), vec![], Arc::new(AllowAll)).await;
    let history = vec![Event::SkillLoaded {
        name: "review".into(),
        text: "KEEP-THIS-RULE".into(),
    }];
    let events = turn(&EngineAgent::new(setup), history, "/compact").await;
    let summary = events
        .iter()
        .find_map(|event| match event {
            Event::Compacted { summary } => Some(summary.clone()),
            _ => None,
        })
        .expect("the conversation was compacted");
    assert!(summary.contains("KEEP-THIS-RULE"), "{summary}");
}

// ---------------------------------------------------------------- injection

/// A turn's shell, with the rules and the approver the test gives it.
struct Shell {
    context: ToolContext,
    policy: PermissionPolicy,
    approved: Mutex<HashSet<String>>,
}

impl Shell {
    fn new(dir: &Path, mode: PermissionMode, rules: Rules) -> Shell {
        let (events, _) = mpsc::unbounded_channel();
        let context = ToolContext {
            cwd: dir.to_path_buf(),
            project_root: dir.to_path_buf(),
            session_dir: dir.to_path_buf(),
            read_files: Arc::new(Mutex::new(HashSet::new())),
            events,
            questions: Arc::new(NoQuestions),
            tree: Arc::new(ProcessGroup::new().unwrap()),
            background: Arc::new(Background::new()),
            hidden_env: vec![],
            memory_dir: None,
        };
        Shell {
            context,
            policy: PermissionPolicy::new(mode, rules),
            approved: Mutex::default(),
        }
    }

    fn ctx<'a>(&'a self, approver: &'a dyn Approver, timeout: Duration) -> ShellCtx<'a> {
        ShellCtx {
            context: &self.context,
            policy: &self.policy,
            approver,
            approved: &self.approved,
            skill: "test",
            timeout,
        }
    }

    async fn run(&self, body: &str, approver: &dyn Approver) -> Result<String, InjectError> {
        run_injections(body, &self.ctx(approver, Duration::from_secs(120))).await
    }

    async fn run_timeout(&self, body: &str, timeout: Duration) -> Result<String, InjectError> {
        run_injections(body, &self.ctx(&AllowAll, timeout)).await
    }
}

struct Counting(Arc<AtomicUsize>, bool);

#[async_trait]
impl Approver for Counting {
    async fn approve(&self, _name: &str, _arguments: &Value) -> bool {
        self.0.fetch_add(1, Ordering::SeqCst);
        self.1
    }
}

fn deny(text: &str) -> Rules {
    Rules {
        deny: vec![Rule::parse(text).unwrap()],
        ..Default::default()
    }
}

#[tokio::test]
async fn bang_replaced_with_output() {
    let dir = tempfile::tempdir().unwrap();
    let shell = Shell::new(dir.path(), PermissionMode::FullAccess, Rules::default());
    let out = shell.run("Now: !`echo hello`!", &AllowAll).await.unwrap();
    assert_eq!(out, "Now: hello!");
}

#[tokio::test]
async fn fenced_bang_block() {
    let dir = tempfile::tempdir().unwrap();
    let shell = Shell::new(dir.path(), PermissionMode::FullAccess, Rules::default());
    let out = shell
        .run("Before\n```!\necho one\n```\nAfter\n", &AllowAll)
        .await
        .unwrap();
    assert_eq!(out, "Before\none\nAfter\n");
}

#[tokio::test]
async fn bang_not_inside_word() {
    let dir = tempfile::tempdir().unwrap();
    let shell = Shell::new(dir.path(), PermissionMode::FullAccess, Rules::default());
    let out = shell.run("wow!`echo hi`", &AllowAll).await.unwrap();
    assert_eq!(out, "wow!`echo hi`");
}

#[tokio::test]
async fn prose_exclamation_before_backtick_is_not_run() {
    let dir = tempfile::tempdir().unwrap();
    let shell = Shell::new(dir.path(), PermissionMode::FullAccess, Rules::default());
    let line = "- **Special characters:** Escape shell metacharacters (`&`, `$`, `(`, `)`, `!`): `adb shell input text \"AT\\&T\"`\n";
    assert_eq!(shell.run(line, &AllowAll).await.unwrap(), line);
}

#[tokio::test]
async fn runner_replaces_only_the_parsed_commands() {
    let dir = tempfile::tempdir().unwrap();
    let shell = Shell::new(dir.path(), PermissionMode::FullAccess, Rules::default());
    let out = shell
        .run("A !`echo one` B `!`echo two`` C\n", &AllowAll)
        .await
        .unwrap();
    assert_eq!(out, "A one B `!`echo two`` C\n");
}

#[tokio::test]
async fn failure_aborts_with_message() {
    let dir = tempfile::tempdir().unwrap();
    let shell = Shell::new(dir.path(), PermissionMode::FullAccess, Rules::default());
    let error = shell.run("!`exit 2`", &AllowAll).await.unwrap_err();
    assert!(error.to_string().contains("2"), "{error}");
}

#[tokio::test]
async fn exit_one_from_grep_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let shell = Shell::new(dir.path(), PermissionMode::FullAccess, Rules::default());
    let out = shell.run("Now: !`echo abc | grep zzz`", &AllowAll).await.unwrap();
    assert_eq!(out, "Now: ");
}

#[tokio::test]
async fn unapproved_supervised_asks_first_time() {
    let dir = tempfile::tempdir().unwrap();
    let shell = Shell::new(dir.path(), PermissionMode::Supervised, Rules::default());
    let asked = Arc::new(AtomicUsize::new(0));
    let approver = Counting(Arc::clone(&asked), true);
    assert_eq!(shell.run("!`echo one`", &approver).await.unwrap(), "one");
    assert_eq!(shell.run("!`echo one`", &approver).await.unwrap(), "one");
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn deny_rule_aborts() {
    let dir = tempfile::tempdir().unwrap();
    let shell = Shell::new(dir.path(), PermissionMode::FullAccess, deny("Bash(touch *)"));
    let error = shell.run("!`touch denied.txt`", &AllowAll).await.unwrap_err();
    assert!(matches!(error, InjectError::Denied(_)), "{error}");
    assert!(!dir.path().join("denied.txt").exists());
}

#[tokio::test]
async fn hanging_command_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let shell = Shell::new(dir.path(), PermissionMode::FullAccess, Rules::default());
    let started = Instant::now();
    let error = shell
        .run_timeout("!`sleep 30`", Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(matches!(error, InjectError::TimedOut(_)), "{error}");
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[tokio::test]
async fn huge_output_is_capped() {
    let dir = tempfile::tempdir().unwrap();
    let shell = Shell::new(dir.path(), PermissionMode::FullAccess, Rules::default());
    let out = shell
        .run("!`head -c 60000 /dev/zero | tr '\\0' a`", &AllowAll)
        .await
        .unwrap();
    assert!(out.chars().count() <= 50_000 + 100, "{}", out.chars().count());
    assert!(out.contains("50,000"));
}

#[tokio::test]
async fn stop_turn_kills_injection() {
    let dir = tempfile::tempdir().unwrap();
    let shell = Shell::new(dir.path(), PermissionMode::FullAccess, Rules::default());
    // Dropping the injection at one second kills bash, so the touch after the four-second sleep never runs.
    let stopped = tokio::time::timeout(
        Duration::from_secs(1),
        shell.run("!`sleep 4; touch alive.txt`", &AllowAll),
    )
    .await;
    assert!(stopped.is_err());
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(!dir.path().join("alive.txt").exists());
}

#[tokio::test]
async fn no_rerun_of_output() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("note.txt"), "!`touch marker.txt`").unwrap();
    let shell = Shell::new(dir.path(), PermissionMode::FullAccess, Rules::default());
    let out = shell.run("!`cat note.txt`", &AllowAll).await.unwrap();
    assert_eq!(out, "!`touch marker.txt`");
    assert!(!dir.path().join("marker.txt").exists());
}

#[test]
fn dollar_in_code_or_email_is_not_invocation() {
    let dir = tempfile::tempdir().unwrap();
    let review = skill(dir.path(), "review", "---\ndescription: reviews code\n---\nbody\n");
    let resolved = resolve_prompt("write `$review` and mail me@host$review", std::slice::from_ref(&review));
    assert!(resolved.mentions.is_empty());
    let resolved = resolve_prompt("now $review, then", &[review]);
    assert_eq!(resolved.mentions.len(), 1);
}

#[tokio::test]
async fn model_invoked_skill_grants_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let echo = skill(
        dir.path(),
        "echo",
        "---\ndescription: runs echo\nallowed-tools: Bash(echo *)\n---\nBody.\n",
    );
    let server = sequence(vec![
        tool_call("s1", "Skill", r#"{"skill":"echo"}"#),
        tool_call("b1", "Bash", r#"{"command":"echo granted"}"#),
        text("done"),
    ])
    .await;
    let setup = skill_setup(&server, dir.path(), vec![echo], Arc::new(DenyAll)).await;
    turn(&EngineAgent::new(setup), vec![], "use it").await;
    let requests = server.received_requests().await.unwrap();
    let results = tool_contents(&requests[2]);
    assert!(results[1].contains("refused"), "{results:?}");
}

#[tokio::test]
async fn granted_grep_is_allowed_quietly() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("a.txt"), "hello\n").unwrap();
    let search = skill(
        dir.path(),
        "search",
        "---\ndescription: searches\nallowed-tools: Grep\n---\nBody.\n",
    );
    let args = serde_json::json!({"pattern": "hello", "path": outside.path()}).to_string();
    let server = sequence(vec![tool_call("g1", "Grep", &args), text("done")]).await;
    let setup = skill_setup(&server, dir.path(), vec![search], Arc::new(DenyAll)).await;
    turn(&EngineAgent::new(setup), vec![], "/search").await;
    let requests = server.received_requests().await.unwrap();
    let results = tool_contents(&requests[1]);
    assert!(!results[0].contains("refused"), "{results:?}");
}

#[tokio::test]
async fn dollar_skips_not_invocable() {
    let dir = tempfile::tempdir().unwrap();
    let quiet = skill(
        dir.path(),
        "quiet",
        "---\ndescription: model only\nuser-invocable: false\n---\nbody\n",
    );
    let resolved = resolve_prompt("use $quiet now", &[quiet]);
    assert!(resolved.mentions.is_empty());
}

#[tokio::test]
async fn ask_rule_is_asked_every_time() {
    let dir = tempfile::tempdir().unwrap();
    let rules = Rules {
        ask: vec![Rule::parse("Bash(echo *)").unwrap()],
        ..Default::default()
    };
    let shell = Shell::new(dir.path(), PermissionMode::FullAccess, rules);
    let asked = Arc::new(AtomicUsize::new(0));
    let approver = Counting(Arc::clone(&asked), true);
    shell.run("!`echo one`", &approver).await.unwrap();
    shell.run("!`echo one`", &approver).await.unwrap();
    assert_eq!(asked.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn invoked_skill_keeps_its_latest_body() {
    let dir = tempfile::tempdir().unwrap();
    let server = sequence(vec![text("the summary")]).await;
    let setup = skill_setup(&server, dir.path(), vec![], Arc::new(AllowAll)).await;
    let history = vec![
        Event::SkillLoaded {
            name: "review".into(),
            text: "OLD-RULE".into(),
        },
        Event::SkillLoaded {
            name: "review".into(),
            text: "NEW-RULE".into(),
        },
    ];
    let events = turn(&EngineAgent::new(setup), history, "/compact").await;
    let summary = events
        .iter()
        .find_map(|event| match event {
            Event::Compacted { summary } => Some(summary.clone()),
            _ => None,
        })
        .expect("the conversation was compacted");
    assert!(
        summary.contains("NEW-RULE") && !summary.contains("OLD-RULE"),
        "{summary}"
    );
}
