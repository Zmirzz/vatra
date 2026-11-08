mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use common::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use vatra_core::{AuditEntry, Event, PermissionMode, Rules, Store};
use vatra_engine::{
    AllowAll, Approver, EXTERNAL_LIMIT_PER_HOUR, ExternalAction, LoadedInstructions, PermissionPolicy, SkillSession,
    SubagentRunner, SubagentSetup, Tool, ToolClass, ToolContext, ToolDef, ToolOutput, agent_tool,
};

const TOOL: &str = "mcp__demo__send";
const ARGS: &str = r#"{"to":"a@b.c","body":"SECRET-BODY-TEXT"}"#;
const SUMMARY: &str = "Send a message to a@b.c";

/// An action with an effect outside the computer. It counts how many times it ran.
#[derive(Default)]
struct Send {
    runs: AtomicUsize,
}

#[async_trait]
impl Tool for Send {
    fn def(&self) -> ToolDef {
        ToolDef { name: TOOL.into(), description: String::new(), parameters: json!({}) }
    }

    fn class(&self, _arguments: &Value, _context: &ToolContext) -> ToolClass {
        ToolClass::External
    }

    fn parallel_safe(&self) -> bool {
        false
    }

    async fn call(&self, _arguments: Value, _context: &ToolContext) -> ToolOutput {
        self.runs.fetch_add(1, Ordering::SeqCst);
        ToolOutput::ok("sent")
    }

    fn external(&self, _arguments: &Value) -> Option<ExternalAction> {
        Some(ExternalAction { connector: "mail".into(), summary: SUMMARY.into() })
    }
}

/// Answers every question with the same answer, and counts the questions.
struct Counting {
    answer: bool,
    asked: AtomicUsize,
}

#[async_trait]
impl Approver for Counting {
    async fn approve(&self, _name: &str, _arguments: &Value) -> bool {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.answer
    }
}

struct Outcome {
    events: Vec<Event>,
    asked: usize,
    runs: usize,
}

impl Outcome {
    /// The output and error flag of the result of the call `id`.
    fn result(&self, id: &str) -> (String, bool) {
        self.events
            .iter()
            .find_map(|event| match event {
                Event::ToolResult { id: found, output, is_error } if found == id => Some((output.clone(), *is_error)),
                _ => None,
            })
            .expect("a result for the call")
    }

    fn refused_by_person(&self, id: &str) -> bool {
        self.events.iter().any(|event| matches!(event, Event::ToolDenied { id: found, .. } if found == id))
    }
}

/// One turn in which the model calls the external action once, with the given rules and answer.
async fn send_once(
    mode: PermissionMode,
    allow: &[&str],
    deny: &[&str],
    answer: bool,
    store: Option<Arc<Store>>,
) -> Outcome {
    let server = sequence(vec![tool_call("c1", TOOL, ARGS), text("done")]).await;
    let tool = Arc::new(Send::default());
    let approver = Arc::new(Counting { answer, asked: AtomicUsize::new(0) });
    let mut setup = setup(&server, approver.clone(), None, vec![tool.clone() as Arc<dyn Tool>], 5).await;
    let (rules, errors) = Rules::parse(allow, &[], deny);
    assert!(errors.is_empty(), "{errors:?}");
    setup.policy = Arc::new(PermissionPolicy::new(mode, rules));
    setup.audit = store;
    setup.session_id = Some(7);
    let events = run_setup(setup, vec![], "send it").await.unwrap();
    Outcome { events, asked: approver.asked.load(Ordering::SeqCst), runs: tool.runs.load(Ordering::SeqCst) }
}

fn audit_store() -> Arc<Store> {
    Arc::new(Store::open_in_memory().unwrap())
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

#[tokio::test]
async fn an_external_action_asks_even_in_full_access() {
    let outcome = send_once(PermissionMode::FullAccess, &[], &[], true, None).await;
    assert_eq!(outcome.asked, 1);
    assert_eq!(outcome.runs, 1);
    assert!(!outcome.result("c1").1);
}

#[tokio::test]
async fn an_allow_rule_does_not_skip_the_question() {
    let outcome = send_once(PermissionMode::Supervised, &[TOOL], &[], true, None).await;
    assert_eq!(outcome.asked, 1);
    assert_eq!(outcome.runs, 1);
}

#[tokio::test]
async fn a_deny_rule_still_denies_without_asking() {
    let outcome = send_once(PermissionMode::FullAccess, &[], &[TOOL], true, Some(audit_store())).await;
    assert_eq!(outcome.asked, 0);
    assert_eq!(outcome.runs, 0);
    let (output, is_error) = outcome.result("c1");
    assert!(is_error);
    assert!(output.contains("denies"), "{output}");
}

#[tokio::test]
async fn a_denied_call_is_audited_and_never_runs() {
    let store = audit_store();
    let outcome = send_once(PermissionMode::Supervised, &[], &[], false, Some(Arc::clone(&store))).await;
    assert_eq!(outcome.asked, 1);
    assert_eq!(outcome.runs, 0);
    assert!(outcome.refused_by_person("c1"));
    let entries = store.audit_recent(10).unwrap();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].decision, "denied");
    assert_eq!(entries[0].tool, TOOL);
    assert_eq!(entries[0].connector, "mail");
    assert_eq!(entries[0].session_id, Some(7));
}

#[tokio::test]
async fn an_approved_call_runs_and_is_audited_with_a_hash_not_the_body() {
    let store = audit_store();
    let outcome = send_once(PermissionMode::Supervised, &[], &[], true, Some(Arc::clone(&store))).await;
    assert_eq!(outcome.asked, 1);
    assert_eq!(outcome.runs, 1);
    let entries = store.audit_recent(10).unwrap();
    assert_eq!(entries.len(), 1, "{entries:?}");
    let entry: &AuditEntry = &entries[0];
    assert_eq!(entry.decision, "approved");
    assert_eq!(entry.summary, SUMMARY);
    assert_eq!(entry.payload_hash.len(), 64);
    assert!(entry.payload_hash.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)), "{}", entry.payload_hash);
    let arguments: Value = serde_json::from_str(ARGS).unwrap();
    let expected = format!("{:x}", Sha256::digest(serde_json::to_string(&arguments).unwrap()));
    assert_eq!(entry.payload_hash, expected);
    assert!(!format!("{entry:?}").contains("SECRET-BODY-TEXT"));
}

fn earlier_approvals(store: &Store, count: u32) {
    for _ in 0..count {
        store
            .record_audit(&AuditEntry {
                at: now(),
                session_id: Some(3),
                tool: TOOL.into(),
                connector: "mail".into(),
                summary: "Earlier".into(),
                decision: "approved".into(),
                payload_hash: "b".repeat(64),
            })
            .unwrap();
    }
}

struct SimultaneousApproval(Arc<tokio::sync::Barrier>);
#[async_trait]
impl Approver for SimultaneousApproval {
    async fn approve(&self, _: &str, _: &Value) -> bool {
        self.0.wait().await;
        true
    }
}

#[tokio::test]
async fn concurrent_human_approvals_cannot_exceed_the_hourly_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.db");
    let first_store = Arc::new(Store::open(&path).unwrap());
    let second_store = Arc::new(Store::open(&path).unwrap());
    earlier_approvals(&first_store, 19);
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let first_server = sequence(vec![tool_call("a",TOOL,ARGS),text("done")]).await;
    let second_server = sequence(vec![tool_call("b",TOOL,ARGS),text("done")]).await;
    let tool = Arc::new(Send::default());
    let mut first = setup(&first_server,Arc::new(SimultaneousApproval(barrier.clone())),None,vec![tool.clone()],5).await;
    let mut second = setup(&second_server,Arc::new(SimultaneousApproval(barrier)),None,vec![tool.clone()],5).await;
    first.audit = Some(first_store.clone());
    second.audit = Some(second_store);
    let (a,b) = tokio::join!(run_setup(first,vec![],"send"),run_setup(second,vec![],"send"));
    a.unwrap(); b.unwrap();
    assert_eq!(tool.runs.load(Ordering::SeqCst),1);
    assert_eq!(first_store.external_count_since("mail",now()-3600).unwrap(),20);
    assert_eq!(first_store.audit_recent(30).unwrap().iter().filter(|row| row.decision=="blocked").count(),1);
}

#[tokio::test]
async fn the_twentieth_call_in_an_hour_still_asks_and_runs() {
    let store = audit_store();
    earlier_approvals(&store, EXTERNAL_LIMIT_PER_HOUR - 1);
    let outcome = send_once(PermissionMode::Supervised, &[], &[], true, Some(Arc::clone(&store))).await;
    assert_eq!(outcome.asked, 1);
    assert_eq!(outcome.runs, 1);
    assert_eq!(store.audit_recent(1).unwrap()[0].decision, "approved");
}

#[tokio::test]
async fn the_twenty_first_call_in_an_hour_is_blocked_without_asking() {
    let store = audit_store();
    earlier_approvals(&store, EXTERNAL_LIMIT_PER_HOUR);
    let outcome = send_once(PermissionMode::FullAccess, &[], &[], true, Some(Arc::clone(&store))).await;
    assert_eq!(outcome.asked, 0);
    assert_eq!(outcome.runs, 0);
    let (output, is_error) = outcome.result("c1");
    assert!(is_error);
    assert!(output.contains("Too many actions on mail"), "{output}");
    let newest = store.audit_recent(1).unwrap();
    assert_eq!(newest[0].decision, "blocked");
    assert_eq!(newest[0].connector, "mail");
}

/// Runs one turn with the given tool and approver. The audit store is the given one.
async fn run_with(approver: Arc<dyn Approver>, store: Arc<Store>) -> (Vec<Event>, usize) {
    let server = sequence(vec![tool_call("c1", TOOL, ARGS), text("done")]).await;
    let tool = Arc::new(Send::default());
    let mut setup = setup(&server, approver, None, vec![tool.clone() as Arc<dyn Tool>], 5).await;
    setup.audit = Some(store);
    setup.session_id = Some(7);
    let events = run_setup(setup, vec![], "send it").await.unwrap();
    (events, tool.runs.load(Ordering::SeqCst))
}

fn result_of(events: &[Event], id: &str) -> (String, bool) {
    events
        .iter()
        .find_map(|event| match event {
            Event::ToolResult { id: found, output, is_error } if found == id => Some((output.clone(), *is_error)),
            _ => None,
        })
        .expect("a result for the call")
}

#[tokio::test]
async fn an_approved_call_is_refused_when_the_audit_entry_cannot_be_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.db");
    drop(Store::open(&path).unwrap());
    let store = Arc::new(Store::open_read_only(&path).unwrap());
    let approver = Arc::new(Counting { answer: true, asked: AtomicUsize::new(0) });
    let (events, runs) = run_with(approver.clone(), store).await;
    assert_eq!(approver.asked.load(Ordering::SeqCst), 1);
    assert_eq!(runs, 0);
    let (output, is_error) = result_of(&events, "c1");
    assert!(is_error);
    assert!(output.contains("audit log"), "{output}");
}

#[tokio::test]
async fn a_call_is_refused_when_the_hourly_count_cannot_be_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.db");
    let store = Arc::new(Store::open(&path).unwrap());
    rusqlite::Connection::open(&path).unwrap().execute_batch("DROP TABLE audit_log").unwrap();
    let approver = Arc::new(Counting { answer: true, asked: AtomicUsize::new(0) });
    let (events, runs) = run_with(approver.clone(), store).await;
    assert_eq!(approver.asked.load(Ordering::SeqCst), 0);
    assert_eq!(runs, 0);
    let (output, is_error) = result_of(&events, "c1");
    assert!(is_error);
    assert!(output.contains("audit log couldn't be read"), "{output}");
}

#[tokio::test]
async fn an_external_action_is_refused_by_an_auto_approving_approver() {
    let store = audit_store();
    let (events, runs) = run_with(Arc::new(AllowAll), Arc::clone(&store)).await;
    assert_eq!(runs, 0);
    let (output, is_error) = result_of(&events, "c1");
    assert!(is_error);
    assert!(output.contains("needs the person's approval"), "{output}");
    let entries = store.audit_recent(10).unwrap();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].decision, "blocked");
}

#[tokio::test]
async fn a_subagent_is_gated_and_audited_for_external_tools() {
    // The turn asks for a subagent, the subagent calls the external action, and both finish.
    let server = sequence(vec![
        tool_call("a1", "Agent", r#"{"prompt":"send it"}"#),
        tool_call("c1", TOOL, ARGS),
        text("sent"),
        text("done"),
    ])
    .await;
    let tool = Arc::new(Send::default());
    let approver = Arc::new(Counting { answer: true, asked: AtomicUsize::new(0) });
    let store = audit_store();
    let mut setup = setup(&server, approver.clone(), None, vec![tool.clone() as Arc<dyn Tool>], 5).await;
    let runner = Arc::new(SubagentRunner::new(SubagentSetup {
        driver: Arc::clone(&setup.driver),
        hub: Arc::clone(&setup.hub),
        tools: &setup.registry,
        policy: Arc::clone(&setup.policy),
        approver: approver.clone(),
        instructions: LoadedInstructions::default(),
        max_steps: 5,
        session: Arc::new(SkillSession::new("7")),
        audit: Some(Arc::clone(&store)),
        session_id: Some(7),
    }));
    setup.registry.register(agent_tool(runner));
    let events = run_setup(setup, vec![], "send it").await.unwrap();
    assert_eq!(approver.asked.load(Ordering::SeqCst), 1, "{events:?}");
    assert_eq!(tool.runs.load(Ordering::SeqCst), 1);
    let entries = store.audit_recent(10).unwrap();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].decision, "approved");
    assert_eq!(entries[0].tool, TOOL);
    assert_eq!(entries[0].session_id, Some(7));
}
