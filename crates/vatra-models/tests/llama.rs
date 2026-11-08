use vatra_models::hardware::{self, GpuSource};
use vatra_models::llama::{find, parse_devices, parse_help, parse_version};

const HELP: &str = include_str!("fixtures/llama-help-b12019.txt");
const MIB: u64 = 1024 * 1024;

#[test]
fn help_b12019_has_fit() {
    let caps = parse_help(HELP);
    assert!(caps.fit && caps.fit_target && caps.fit_ctx && caps.jinja && caps.list_devices && caps.mmproj && caps.no_webui && caps.offline);
}

#[test]
fn help_without_fit() {
    let old = "-ngl,  --gpu-layers N   number of layers\n--jinja   use jinja\n-c, --ctx-size N\n";
    let caps = parse_help(old);
    assert!(!caps.fit && !caps.fit_ctx && !caps.list_devices);
    assert!(caps.jinja);
}

#[test]
fn devices_vulkan_line() {
    let gpus = parse_devices(include_str!("fixtures/list-devices-vulkan.txt"));
    assert_eq!(gpus.len(), 1);
    assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 5060");
    assert_eq!(gpus[0].backend.as_deref(), Some("Vulkan0"));
    assert_eq!(gpus[0].total, 7895 * MIB);
    assert_eq!(gpus[0].free, Some(7127 * MIB));
    assert_eq!(gpus[0].source, GpuSource::Llama);
}

#[test]
fn devices_cuda_and_cpu_only() {
    let text = "Available devices:\n  CUDA0: NVIDIA GeForce RTX 4090 (24563 MiB, 23012 MiB free)\n  CUDA1: NVIDIA RTX A2000 (6138 MiB, 6000 MiB free)\n";
    let gpus = parse_devices(text);
    assert_eq!(gpus.iter().map(|g| g.backend.clone().unwrap()).collect::<Vec<_>>(), ["CUDA0", "CUDA1"]);
    assert!(parse_devices("Available devices:\n").is_empty());
}

#[test]
fn version_line() {
    assert_eq!(
        parse_version(include_str!("fixtures/llama-version-b12019.txt")),
        (Some(12019), Some("1052f7e1d".into()))
    );
    assert_eq!(parse_version("version: 4567 (abc1234)\n"), (Some(4567), Some("abc1234".into())));
    assert_eq!(parse_version("nothing"), (None, None));
}

#[test]
fn find_prefers_configured_path() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("my llama ț").join("llama-server.exe");
    std::fs::create_dir_all(fake.parent().unwrap()).unwrap();
    std::fs::write(&fake, b"not a program").unwrap();
    let install = find(Some(&fake)).unwrap();
    assert_eq!(install.server, fake);
    assert_eq!(install.build, None);
}

#[cfg(windows)]
#[test]
fn detect_runs_without_llama() {
    let report = hardware::detect(None, Some(std::path::Path::new(".")));
    assert!(report.ram.total > 0 && report.ram.available <= report.ram.total);
    assert!(report.cpu.threads.unwrap() > 0);
    assert!(report.cpu.cores.unwrap() > 0);
    assert!(report.disk_free.unwrap() > 0);
}

/// Prints the real report. Run by hand: cargo test -p vatra-models --test llama -- --ignored --nocapture
#[test]
#[ignore]
fn real_hardware_matches_llama() {
    let install = find(None);
    println!("{install:#?}");
    let report = hardware::detect(install.as_ref(), Some(std::path::Path::new(".")));
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    println!("system only: {:#?}", hardware::detect(None, None).gpus);
}
