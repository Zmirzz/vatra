use async_trait::async_trait;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use vatra_core::{Agent, AgentError, Event, Store, Summarizer, TurnOutcome, TurnRequest};
use vatra_engine::{Approver, NoQuestions};
use vatra_service::{AgentFactory, BuildRequest, Service};

struct Factory {
    builds: AtomicUsize,
    costs: bool,
}
struct Fake;
#[async_trait]
impl Agent for Fake {
    fn name(&self) -> &str {
        "fake"
    }
    async fn run_turn(
        &self,
        _: TurnRequest,
        sink: tokio::sync::mpsc::UnboundedSender<Event>,
    ) -> Result<TurnOutcome, AgentError> {
        sink.send(Event::AssistantText {
            text: "retry.js contains 7".into(),
        })
        .unwrap();
        Ok(TurnOutcome::default())
    }
}
#[async_trait]
impl AgentFactory for Factory {
    fn names(&self) -> Vec<String> {
        vec!["fake".into()]
    }
    fn summarizer(&self) -> Arc<dyn Summarizer> {
        panic!("not used")
    }
    fn check_costs(&self, _: &str) -> bool {
        self.costs
    }
    async fn build(&self, request: BuildRequest) -> Result<Arc<dyn Agent>, String> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        assert!(request.cwd.unwrap().is_dir());
        Ok(Arc::new(Fake))
    }
}
fn service(costs: bool) -> (Arc<Service>, Arc<Factory>) {
    let factory = Arc::new(Factory {
        builds: AtomicUsize::new(0),
        costs,
    });
    (
        Service::new(Arc::new(Store::open_in_memory().unwrap()), factory.clone()).unwrap(),
        factory,
    )
}
#[tokio::test]
async fn cost_agents_need_consent_before_building() {
    let (service, factory) = service(true);
    assert!(
        vatra_service::parity::run_parity(&service, "fake", None, false)
            .await
            .unwrap_err()
            .contains("consent")
    );
    assert_eq!(factory.builds.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn run_saved_and_matrix_reports_actual_coverage() {
    let (service, _) = service(false);
    let run = vatra_service::parity::run_parity(&service, "fake", Some("05-search"), false)
        .await
        .unwrap();
    assert_eq!(run.results.len(), 1);
    assert!(!run.results[0].outcome.passed); // answer alone does not prove tool use
    let saved = vatra_service::parity::results(service.store(), None).unwrap();
    assert_eq!(saved.len(), 1);
    let matrix = vatra_service::parity::export_matrix(&saved);
    assert!(matrix.contains("1 tasks"));
    assert!(matrix.contains("does not establish general"));
}

#[tokio::test]
async fn matrix_keeps_all_run_rows_together_before_task_details() {
    let (service,_) = service(false);
    let first = vatra_service::parity::run_parity(&service,"fake",Some("05-search"),false).await.unwrap();
    let mut second=first.clone();
    second.agent="second-agent".into();
    let matrix=vatra_service::parity::export_matrix(&[first,second]);
    assert!(matrix.find("| second-agent").unwrap() < matrix.find("\n- 05-search").unwrap(),"task details must not break the result table");
}
#[tokio::test]
async fn the_trace_keeps_every_event_when_asked() {
    let (service, _) = service(false);
    let file = tempfile::NamedTempFile::new().unwrap();
    // SAFETY: only this test sets the variable, and the other tests here don't read it.
    unsafe { std::env::set_var("VATRA_PARITY_TRACE", file.path()) };
    vatra_service::parity::run_parity(&service, "fake", Some("05-search"), false).await.unwrap();
    unsafe { std::env::remove_var("VATRA_PARITY_TRACE") };
    let text = std::fs::read_to_string(file.path()).unwrap();
    let first: serde_json::Value = serde_json::from_str(text.lines().next().expect("a traced event")).unwrap();
    assert_eq!(first["task"], "05-search");
    assert!(first["event"]["type"].is_string());
}
#[test]
fn a_failure_with_several_lines_stays_one_list_item() {
    use vatra_parity::{Metrics, Outcome};
    let run = vatra_service::parity::ParityRun {
        agent: "a".into(), native: None, model: "m".into(), build: "b".into(), started_at: 1,
        results: vec![vatra_service::parity::TaskResult {
            task_id: "t".into(),
            outcome: Outcome { passed: false, failures: vec!["`node check.js` still fails:   at one
    at two
Node.js v24".into()] },
            metrics: Metrics::default(),
        }],
    };
    let matrix = vatra_service::parity::export_matrix(&[run]);
    let item = matrix.lines().find(|line| line.starts_with("- t:")).unwrap();
    assert_eq!(item, "- t: failed: `node check.js` still fails: at one at two Node.js v24");
}
#[tokio::test]
async fn only_exact_listed_commands_approved_and_paths_stay_inside() {
    let dir = tempfile::tempdir().unwrap();
    let policy = vatra_service::parity::TaskApprover::new(dir.path(), vec!["node test.js".into()]);
    assert!(
        policy
            .approve("Bash", &serde_json::json!({"command":"node test.js"}))
            .await
    );
    assert!(
        !policy
            .approve(
                "Bash",
                &serde_json::json!({"command":"node test.js; whoami"})
            )
            .await
    );
    assert!(
        !policy
            .approve("Read", &serde_json::json!({"file_path":"../outside"}))
            .await
    );
    assert!(
        !policy
            .approve("mcp__other__echo", &serde_json::json!({}))
            .await
    );
    let _ = NoQuestions;
}

fn run_with_results(results: &[(&str, bool)]) -> vatra_service::parity::ParityRun {
    vatra_service::parity::ParityRun {
        agent: "a".into(),
        native: None,
        model: "m".into(),
        build: "b".into(),
        started_at: 1,
        results: results
            .iter()
            .map(|(id, passed)| vatra_service::parity::TaskResult {
                task_id: id.to_string(),
                outcome: vatra_parity::Outcome {
                    passed: *passed,
                    failures: Vec::new(),
                },
                metrics: vatra_parity::Metrics::default(),
            })
            .collect(),
    }
}
#[test]
fn pass_rates_count_each_task_over_repeated_runs() {
    let runs = vec![
        run_with_results(&[("02-rename", false), ("01-fix-test", true)]),
        run_with_results(&[("02-rename", false), ("01-fix-test", false)]),
        run_with_results(&[("02-rename", false), ("01-fix-test", true)]),
    ];
    let rates = vatra_service::parity::pass_rates(&runs);
    assert_eq!(rates.len(), 2);
    assert_eq!(rates[0].task_id, "01-fix-test");
    assert_eq!((rates[0].passed, rates[0].runs), (2, 3));
    assert_eq!(rates[1].task_id, "02-rename");
    assert_eq!((rates[1].passed, rates[1].runs), (0, 3));
}
#[test]
fn export_shows_a_rate_table_when_runs_repeat() {
    let runs = vec![
        run_with_results(&[("01-fix-test", true)]),
        run_with_results(&[("01-fix-test", false)]),
        run_with_results(&[("01-fix-test", true)]),
    ];
    let matrix = vatra_service::parity::export_matrix(&runs);
    assert!(matrix.contains("## Pass rate per task"));
    assert!(matrix.contains("| 01-fix-test | 2 / 3 |"));
}
#[test]
fn export_labels_each_repeated_group_and_skips_single_runs() {
    let repeated = |passed: bool| {
        let mut run = run_with_results(&[("01-fix-test", passed)]);
        run.agent = "a".into();
        run
    };
    let mut single = run_with_results(&[("01-fix-test", true)]);
    single.agent = "second".into();
    let matrix = vatra_service::parity::export_matrix(&[repeated(true), repeated(false), repeated(true), single]);
    assert!(matrix.contains("### a / m (b)\n\n| Task | Passed / runs |\n|---|---|\n| 01-fix-test | 2 / 3 |\n"));
    assert!(!matrix.contains("### second"));
    assert!(!matrix.contains("| 01-fix-test | 1 / 1 |"));
}
#[test]
fn export_has_no_rate_table_for_a_single_run() {
    let matrix = vatra_service::parity::export_matrix(&[run_with_results(&[("01-fix-test", true)])]);
    assert!(!matrix.contains("## Pass rate per task"));
}
