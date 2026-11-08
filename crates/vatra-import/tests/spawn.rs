use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use vatra_extensions::{Origin, RunsCode, Scope};
use vatra_import::{Found, FoundKind, ProbeResult, SystemProbe, probe_items};

/// A skill item whose one command is `command`.
fn item(name: &str, command: &str) -> Found {
    Found {
        kind: FoundKind::Skill,
        name: name.into(),
        origin: Origin::Claude,
        scope: Scope::Global,
        project: None,
        source: PathBuf::from(name),
        hash: "hash".into(),
        runs_code: vec![RunsCode {
            what: "inline command".into(),
            command: command.into(),
        }],
        secrets: Vec::new(),
        unsupported: None,
        details: Value::Null,
        modified: None,
        also_from: Vec::new(),
    }
}

/// A command that writes the marker file when it runs: a `.cmd` on Windows, a shell script elsewhere.
fn write_script(dir: &Path, name: &str, marker: &Path) {
    if cfg!(windows) {
        fs::write(
            dir.join(format!("{name}.cmd")),
            format!("@echo off\r\necho ran> \"{}\"\r\n", marker.display()),
        )
        .unwrap();
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            fs::write(&path, format!("#!/bin/sh\necho ran > \"{}\"\n", marker.display())).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

/// Its own test binary, so no other test reads or changes PATH while this one runs.
#[tokio::test]
async fn imported_command_is_never_spawned() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let marker = tmp.path().join("ran.marker");
    write_script(&bin, "tool", &marker);
    let mut dirs = vec![bin.clone()];
    dirs.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    let path = std::env::join_paths(dirs).unwrap();
    // SAFETY: this binary has no other test, so nothing reads PATH on another thread meanwhile.
    unsafe { std::env::set_var("PATH", path) };

    let by_path = bin.join("tool");
    let items = vec![
        item("by-path", &format!("{} --run", by_path.display())),
        item("on-path", "tool --run"),
    ];

    let probes = probe_items(&SystemProbe::new(), &items).await;

    assert!(!marker.exists(), "an imported command was started");
    assert_eq!(
        probes.programs.get(&by_path.display().to_string()),
        Some(&ProbeResult::NotChecked("not checked until you approve it".into()))
    );
    assert_eq!(probes.programs.get("tool"), Some(&ProbeResult::Ok));
}
