//! Local models in the service: the llama.cpp server, downloads and the short check, against a fake llama-server
//! and a mocked Hugging Face. Nothing here downloads a real model or runs a real one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};
use vatra_core::Store;
use vatra_models::hf::HfClient;
use vatra_models::label::{Label, LabelReason, StepOutcome};
use vatra_models::llama::{self, LlamaCaps, LlamaInstall};
use vatra_models::variant::RepoFile;
use vatra_service::models::download::{DownloadError, Downloader, Job};
use vatra_service::models::server::{ModelServer, ServerError, args, parse_fit_params};
use vatra_service::models::{AddedBy, LocalModel, ModelManager, ModelSettings};
use vatra_service::{ConfigAgents, Service};

/// A configuration whose only agent is never called.
const UNUSED_API: &str = "summarizer = \"api\"
[agents.api]
kind = \"model\"
base_url = \"http://127.0.0.1:9/v1\"
model = \"m\"
";
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-llama-server");

fn fake_install() -> LlamaInstall {
    llama::inspect(PathBuf::from(FAKE))
}

/// A model file for the fake server, with the mode it should play.
fn model_at(dir: &Path, name: &str, mode: &str) -> LocalModel {
    let file = dir.join(name);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"not really a model").unwrap();
    std::fs::write(format!("{}.mode", file.display()), mode).unwrap();
    LocalModel {
        slug: name.trim_end_matches(".gguf").to_lowercase().replace(' ', "-"),
        repo: None,
        variant: None,
        files: vec![file],
        sha256: vec![],
        size_bytes: 18,
        facts: None,
        added_by: AddedBy::Path,
        settings: ModelSettings { ctx: 32_768, mmproj: None, fit: None },
        created_at: 0,
    }
}

fn manager(dir: &Path, hf: Option<&str>) -> Arc<ModelManager> {
    let store = Arc::new(Store::open(&dir.join("vatra.db")).unwrap());
    let hf = hf.map_or_else(HfClient::public, |url| HfClient::new(url.parse().unwrap()));
    ModelManager::with_sources(
        store,
        dir.join("models"),
        Some(fake_install()),
        (PathBuf::from(FAKE), vec!["mcp-probe".into()]),
        hf,
        "http://127.0.0.1:9/none".into(),
    )
}

fn install(caps: LlamaCaps) -> LlamaInstall {
    LlamaInstall { server: "llama-server".into(), fit_params: None, build: Some(1), commit: None, caps }
}

#[test]
fn args_use_fit_when_supported() {
    let dir = tempfile::tempdir().unwrap();
    let model = model_at(dir.path(), "m.gguf", "good");
    let caps = LlamaCaps { fit: true, fit_ctx: true, jinja: true, no_webui: true, ..LlamaCaps::default() };
    let args: Vec<String> = args(&install(caps), &model, 4321, Some(20)).iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let joined = args.join(" ");
    assert!(joined.contains("--host 127.0.0.1 --port 4321 -c 32768 --jinja --fit on --fit-ctx 16384"), "{joined}");
    assert!(!args.contains(&"-ngl".to_string()));
    assert!(args.contains(&"--no-webui".to_string()));
}

#[test]
fn args_without_fit_use_ngl_and_keep_paths_whole() {
    let dir = tempfile::tempdir().unwrap();
    let model = model_at(&dir.path().join("Răzvan Test").join("modele"), "x y.gguf", "good");
    let args = args(&install(LlamaCaps { jinja: true, ..LlamaCaps::default() }), &model, 1, Some(20));
    assert_eq!(args[1], model.path().as_os_str(), "the path is one argument");
    let text: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    assert!(text.windows(2).any(|pair| pair == ["-ngl", "20"]));
    assert!(!text.contains(&"--fit".to_string()));
}

#[test]
fn fit_params_parsed_from_output() {
    let text = "llama_params_fit: fitting params to device memory\nmain: -c 32768 -ngl 30\n";
    let fit = parse_fit_params(text, 4096).unwrap();
    assert_eq!((fit.gpu_layers.as_str(), fit.ctx), ("30", 32768));
    assert!(parse_fit_params("nothing", 1).is_none());
}

#[tokio::test]
async fn ensure_waits_for_health_and_reads_props_ctx() {
    let dir = tempfile::tempdir().unwrap();
    let model = model_at(dir.path(), "m.gguf", "good");
    let server = ModelServer::new(Some(fake_install()));
    let (running, guard) = server.ensure(&model).await.unwrap();
    assert_eq!(running.ctx, Some(32_768));
    assert_eq!(running.gpu_layers.as_deref(), Some("37/37"));
    assert!(running.device.as_deref().unwrap().starts_with("Vulkan0"));
    let args = std::fs::read_to_string(format!("{}.args", model.path().display())).unwrap();
    assert!(args.contains("--fit\non") && args.contains("--offline"), "{args}");
    // A second turn reuses it.
    let (again, _second) = server.ensure(&model).await.unwrap();
    assert_eq!(again.base_url, running.base_url);
    drop(guard);
    server.stop().await;
    assert!(server.current().await.is_none());
}

#[tokio::test]
async fn exit_during_start_returns_log_tail() {
    let dir = tempfile::tempdir().unwrap();
    let model = model_at(dir.path(), "broken.gguf", "exit");
    let server = ModelServer::new(Some(fake_install()));
    let Err(ServerError::Exited { log_tail }) = server.ensure(&model).await else { panic!("expected an exit") };
    assert!(log_tail.iter().any(|line| line.contains("error loading model")), "{log_tail:?}");
}

#[tokio::test]
async fn second_model_reports_busy_during_turn() {
    let dir = tempfile::tempdir().unwrap();
    let first = model_at(dir.path(), "a.gguf", "good");
    let second = model_at(dir.path(), "b.gguf", "good");
    let server = ModelServer::new(Some(fake_install()));
    let (_, guard) = server.ensure(&first).await.unwrap();
    assert!(matches!(server.ensure(&second).await, Err(ServerError::Busy { .. })));
    drop(guard);
    let (running, _guard) = server.ensure(&second).await.unwrap();
    assert_eq!(running.slug, "b");
    server.stop().await;
}

#[tokio::test]
async fn idle_timeout_stops_server() {
    let dir = tempfile::tempdir().unwrap();
    let model = model_at(dir.path(), "m.gguf", "good");
    let server = ModelServer::new(Some(fake_install()));
    server.watch_idle(Duration::from_millis(300));
    drop(server.ensure(&model).await.unwrap());
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(server.current().await.is_none());
}

#[tokio::test]
async fn no_llama_says_how_to_install() {
    let dir = tempfile::tempdir().unwrap();
    let model = model_at(dir.path(), "m.gguf", "good");
    let error = ModelServer::new(None).ensure(&model).await.err().unwrap();
    assert!(error.to_string().contains("winget install ggml.llamacpp"));
}

/// Serves `data` and honors `Range`, cutting the first full response after `cut` bytes.
struct Ranged {
    data: Vec<u8>,
    cut: Option<usize>,
    seen: Arc<Mutex<Vec<Option<String>>>>,
}

impl Respond for Ranged {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let range = request.headers.get("range").map(|value| value.to_str().unwrap().to_string());
        let first = {
            let mut seen = self.seen.lock().unwrap();
            seen.push(range.clone());
            seen.len() == 1
        };
        let start = range
            .as_deref()
            .and_then(|range| range.strip_prefix("bytes="))
            .and_then(|range| range.trim_end_matches('-').parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = self.data[start..].to_vec();
        if let (true, Some(cut)) = (first, self.cut) {
            // A body shorter than the file stands for a connection that dropped.
            body.truncate(cut);
        }
        let status = if start > 0 { 206 } else { 200 };
        ResponseTemplate::new(status).set_body_bytes(body)
    }
}

fn sha(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

async fn served(data: Vec<u8>, cut: Option<usize>) -> (MockServer, Arc<Mutex<Vec<Option<String>>>>) {
    let server = MockServer::start().await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("GET"))
        .and(path("/m.gguf"))
        .respond_with(Ranged { data, cut, seen: Arc::clone(&seen) })
        .mount(&server)
        .await;
    (server, seen)
}

fn job(server: &MockServer, data: &[u8], dest: PathBuf, sha256: Option<String>) -> Job {
    Job {
        url: format!("{}/m.gguf", server.uri()).parse().unwrap(),
        file: RepoFile { path: "m.gguf".into(), size: data.len() as u64, sha256 },
        dest,
        token: None,
    }
}

#[tokio::test]
async fn download_resumes_from_part() {
    let data: Vec<u8> = (0..100_000u32).map(|n| n as u8).collect();
    let (server, seen) = served(data.clone(), Some(60_000)).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("Răzvan Test").join("m.gguf");
    let jobs = [job(&server, &data, dest.clone(), Some(sha(&data)))];
    Downloader::default().run(&jobs, None, &|_| {}).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), data);
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0], None, "{seen:?}");
    assert_eq!(seen[1].as_deref(), Some("bytes=60000-"), "{seen:?}");
}

#[tokio::test]
async fn checksum_mismatch_removes_part() {
    let data = vec![7u8; 5000];
    let (server, _) = served(data.clone(), None).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("m.gguf");
    let jobs = [job(&server, &data, dest.clone(), Some("00".repeat(32)))];
    let error = Downloader::default().run(&jobs, None, &|_| {}).await.unwrap_err();
    assert_eq!(error, DownloadError::Checksum { file: "m.gguf".into() });
    assert!(!dest.exists());
    assert!(!vatra_service::models::download::part_path(&dest).exists());
}

#[tokio::test]
async fn refuses_without_space() {
    let data = vec![1u8; 5000];
    let (server, seen) = served(data.clone(), None).await;
    let dir = tempfile::tempdir().unwrap();
    let jobs = [job(&server, &data, dir.path().join("m.gguf"), None)];
    let error = Downloader::default().run(&jobs, Some(100), &|_| {}).await.unwrap_err();
    assert_eq!(error, DownloadError::NoSpace { needed: 5000, free: 100 });
    assert!(seen.lock().unwrap().is_empty(), "nothing was requested");
}

#[tokio::test]
async fn cancel_leaves_no_part_and_second_download_is_busy() {
    let data = vec![3u8; 3_000_000];
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(data.clone()).set_delay(Duration::from_millis(400)))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("m.gguf");
    let downloader = Arc::new(Downloader::default());
    let jobs = vec![job(&server, &data, dest.clone(), None)];
    let running = {
        let downloader = Arc::clone(&downloader);
        tokio::spawn(async move { downloader.run(&jobs, None, &|_| {}).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(downloader.is_busy());
    let other = [job(&server, &data, dir.path().join("other.gguf"), None)];
    assert_eq!(downloader.run(&other, None, &|_| {}).await, Err(DownloadError::Busy));
    downloader.cancel();
    assert_eq!(running.await.unwrap(), Err(DownloadError::Cancelled));
    assert!(!dest.exists() && !vatra_service::models::download::part_path(&dest).exists());
}

#[tokio::test]
async fn progress_is_throttled() {
    let data = vec![9u8; 2_000_000];
    let (server, _) = served(data.clone(), None).await;
    let dir = tempfile::tempdir().unwrap();
    let jobs = [job(&server, &data, dir.path().join("m.gguf"), None)];
    let count = Mutex::new(0usize);
    let started = std::time::Instant::now();
    Downloader::default().run(&jobs, None, &|_| *count.lock().unwrap() += 1).await.unwrap();
    let allowed = (started.elapsed().as_millis() / 250) as usize + 2;
    assert!(*count.lock().unwrap() <= allowed);
}

/// A minimal GGUF header: enough for `local_facts`.
fn gguf_bytes() -> Vec<u8> {
    let mut out = b"GGUF".to_vec();
    out.extend(3u32.to_le_bytes());
    out.extend(0u64.to_le_bytes());
    out.extend(2u64.to_le_bytes());
    let key = |out: &mut Vec<u8>, key: &str, kind: u32| {
        out.extend((key.len() as u64).to_le_bytes());
        out.extend(key.as_bytes());
        out.extend(kind.to_le_bytes());
    };
    key(&mut out, "general.architecture", 8);
    out.extend(5u64.to_le_bytes());
    out.extend(b"llama");
    key(&mut out, "llama.context_length", 4);
    out.extend(40960u32.to_le_bytes());
    out
}

#[tokio::test]
async fn add_path_with_diacritics_then_remove_keeps_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Răzvan Test").join("modelul meu.gguf");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, gguf_bytes()).unwrap();
    let models = manager(dir.path(), None);
    let model = models.add_path(&file).await.unwrap();
    assert_eq!(model.facts.as_ref().unwrap().architecture.as_deref(), Some("llama"));
    assert_eq!(model.settings.ctx, 32_768, "the default context, within the model's 40960");
    assert_eq!(models.agent_names(), vec![format!("local:{}", model.slug)]);
    assert!(models.add_path(&file).await.is_err(), "added once");
    models.remove(&model.slug).await.unwrap();
    assert!(file.exists(), "a file added by path stays");
    assert!(models.agent_names().is_empty());
}

#[tokio::test]
async fn download_records_model_and_remove_deletes_files() {
    let data = gguf_bytes();
    let hf = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/models/owner/Tiny-GGUF"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "gated": false })))
        .mount(&hf)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/models/owner/Tiny-GGUF/tree/main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "type": "file", "path": "tiny-Q4_K_M.gguf", "size": data.len(), "lfs": { "oid": sha(&data), "size": data.len() } }
        ])))
        .mount(&hf)
        .await;
    Mock::given(method("GET"))
        .and(path("/owner/Tiny-GGUF/resolve/main/tiny-Q4_K_M.gguf"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(data.clone()))
        .mount(&hf)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let models = manager(dir.path(), Some(&format!("{}/", hf.uri())));
    let model = models.download("owner/Tiny-GGUF", "Q4_K_M", false).await.unwrap();
    assert_eq!(model.slug, "tiny-q4_k_m");
    let file = dir.path().join("models").join("owner").join("Tiny-GGUF").join("tiny-Q4_K_M.gguf");
    assert_eq!(model.files, vec![file.clone()]);
    assert!(file.exists());
    assert!(models.download("owner/Tiny-GGUF", "Q4_K_M", false).await.is_err(), "downloaded once");
    models.remove(&model.slug).await.unwrap();
    assert!(!file.exists());
    assert!(!dir.path().join("models").join("owner").exists(), "empty folders go too");
}

fn service_with(dir: &Path, config: &str, models: Arc<ModelManager>) -> Arc<Service> {
    let store = Arc::new(Store::open(&dir.join("vatra.db")).unwrap());
    let config = vatra_service::config::Config::parse(config).unwrap();
    let agents = ConfigAgents::new(config).with_models(models);
    Service::new(store, Arc::new(agents)).unwrap()
}

async fn check(mode: &str) -> vatra_service::models::check::CheckRecord {
    let dir = tempfile::tempdir().unwrap();
    let models = manager(dir.path(), None);
    let file = model_at(&dir.path().join("files"), "m.gguf", mode).files[0].clone();
    std::fs::write(&file, gguf_bytes()).unwrap();
    let model = models.add_path(&file).await.unwrap();
    let service = service_with(dir.path(), UNUSED_API, Arc::clone(&models));
    assert!(service.agents().names().contains(&model.agent()));
    let record = service.check_model(&model.agent(), false).await.unwrap();
    let _ = models.stop().await;
    record
}

#[tokio::test]
async fn all_pass_is_complete() {
    let record = check("good").await;
    assert_eq!(record.label, Label::Complete, "{record:#?}");
    assert_eq!(record.results.tools.passed, 2);
    assert_eq!(record.results.mcp, Some(StepOutcome::Passed));
    assert_eq!(record.results.speed.unwrap().gen_tps, Some(42.0));
    assert_eq!(record.llama_build, Some(99));
}

#[tokio::test]
async fn text_instead_of_call_counts_as_fail() {
    let record = check("flaky").await;
    assert_eq!(record.label, Label::Partial, "{record:#?}");
    assert_eq!(record.reasons, vec![LabelReason::ToolsUnreliable]);
    assert!(record.results.tool_failures[0].contains("without calling"));
}

#[tokio::test]
async fn no_tools_is_chat_only() {
    let record = check("no-tools").await;
    assert_eq!(record.label, Label::ChatOnly);
}

#[tokio::test]
async fn wrong_argument_or_ignored_result_fails() {
    assert_eq!(check("wrong-arg").await.results.tools.passed, 0);
    let forget = check("forget").await;
    assert_eq!(forget.results.tools.passed, 0);
    assert!(matches!(forget.results.mcp, Some(StepOutcome::Failed { .. })));
}

#[tokio::test]
async fn unfinished_checks_keep_the_last_result() {
    for mode in ["exit", "broken"] {
        let dir = tempfile::tempdir().unwrap();
        let models = manager(dir.path(), None);
        let file = model_at(&dir.path().join("files"), "m.gguf", mode).files[0].clone();
        std::fs::write(&file, gguf_bytes()).unwrap();
        let model = models.add_path(&file).await.unwrap();
        let service = service_with(dir.path(), UNUSED_API, Arc::clone(&models));
        let error = service.check_model(&model.agent(), false).await.unwrap_err();
        assert!(error.to_string().starts_with("The check couldn't finish"), "{mode}: {error}");
        let labels = models.labels(&[]).await;
        assert!(labels[0].estimated && labels[0].checked_at.is_none(), "{mode}: nothing was stored as measured");
        let _ = models.stop().await;
        assert!(!dir.path().join("checks").read_dir().is_ok_and(|mut entries| entries.next().is_some()), "the check folder is removed");
    }
}

#[tokio::test]
async fn stop_refuses_while_a_turn_uses_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let model = model_at(dir.path(), "m.gguf", "good");
    let server = ModelServer::new(Some(fake_install()));
    let (_, guard) = server.ensure(&model).await.unwrap();
    assert!(matches!(server.stop_unused().await, Err(ServerError::Busy { .. })));
    drop(guard);
    server.stop_unused().await.unwrap();
    assert!(server.current().await.is_none());
}

#[tokio::test]
async fn api_check_requires_consent() {
    let api = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(500)).expect(0).mount(&api).await;
    let dir = tempfile::tempdir().unwrap();
    let config = format!(
        "summarizer = \"api\"\n[agents.api]\nkind = \"model\"\nbase_url = \"{}/v1\"\nmodel = \"m\"\n",
        api.uri()
    );
    let service = service_with(dir.path(), &config, manager(dir.path(), None));
    let error = service.check_model("api", false).await.unwrap_err();
    assert!(error.to_string().contains("charge"), "{error}");
    let labels = service.models().unwrap().labels(&service.agents().configured_models()).await;
    assert_eq!((labels[0].label, labels[0].estimated), (Label::NotChecked, true));
}

#[test]
fn local_only_configuration_needs_no_summarizer() {
    assert!(vatra_service::config::Config::parse("summarizer = \"\"\n[agents]\n").is_ok());
    assert!(vatra_service::config::Config::parse("summarizer = \"missing\"\n[agents]\n").is_err());
}

#[test]
fn repo_paths_cannot_leave_the_models_folder() {
    use vatra_service::models::manager::safe_join;
    let base = Path::new("C:/models");
    assert_eq!(safe_join(base, "owner/Repo-GGUF").unwrap(), base.join("owner").join("Repo-GGUF"));
    for bad in ["../x.gguf", "a/../../x.gguf", "C:/x.gguf", r"a\..\x.gguf", "/abs.gguf", "a//b", "./a"] {
        assert!(safe_join(base, bad).is_err(), "{bad}");
    }
}
