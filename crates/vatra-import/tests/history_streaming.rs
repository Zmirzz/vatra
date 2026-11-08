use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use vatra_core::Store;
use vatra_import::import_log;

#[cfg(windows)]
fn peak_resident_bytes() -> Option<usize> {
    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut ProcessMemoryCounters,
            size: u32,
        ) -> i32;
    }
    let mut counters: ProcessMemoryCounters = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
    // The documented pseudo handle for this process avoids opening a separate process handle.
    let ok = unsafe {
        GetProcessMemoryInfo(
            -1_isize as *mut std::ffi::c_void,
            &mut counters,
            counters.cb,
        )
    };
    (ok != 0).then_some(counters.peak_working_set_size)
}

#[cfg(target_os = "linux")]
fn peak_resident_bytes() -> Option<usize> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")?
            .split_whitespace()
            .next()?
            .parse::<usize>()
            .ok()
            .map(|kb| kb * 1024)
    })
}

#[cfg(not(any(windows, target_os = "linux")))]
fn peak_resident_bytes() -> Option<usize> {
    None
}

#[test]
fn two_hundred_mb_log_import_stays_bounded_and_reaches_text_after_an_oversized_line() {
    const MIN_BYTES: u64 = 200 * 1024 * 1024;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("large.jsonl");
    let mut writer = BufWriter::new(File::create(&path).unwrap());
    writer.write_all(b"{\"type\":\"user\",\"sessionId\":\"large\",\"message\":{\"content\":\"streamheadneedle\"}}\n").unwrap();
    // Reuse one small line; the generated 200 MB input is never held in a fixture Vec/String.
    let ignored = format!(
        "{{\"type\":\"progress\",\"data\":\"{}\"}}\n",
        "x".repeat(64 * 1024)
    );
    let mut bytes = 0_u64;
    while bytes < MIN_BYTES {
        writer.write_all(ignored.as_bytes()).unwrap();
        bytes += ignored.len() as u64;
    }
    // Exceed the parser's 4 MB line limit without allocating a large line in the test itself.
    for _ in 0..1280 {
        writer.write_all(&[b'x'; 4096]).unwrap();
    }
    writer
        .write_all(b"\n{\"type\":\"assistant\",\"message\":{\"content\":\"streamtailneedle\"}}\n")
        .unwrap();
    writer.flush().unwrap();
    drop(writer);
    assert!(std::fs::metadata(&path).unwrap().len() >= MIN_BYTES);
    let before = peak_resident_bytes();
    let store = Store::open_in_memory().unwrap();
    let mut reading = 0;
    let mut indexing = 0;
    let id = import_log(
        &path,
        "claude",
        &store,
        &[],
        &AtomicBool::new(false),
        &mut |_, stage| match stage {
            "Reading past conversations" => reading += 1,
            "Indexing past conversations" => indexing += 1,
            _ => {}
        },
    )
    .unwrap();
    let after = peak_resident_bytes();
    if cfg!(windows) || cfg!(target_os = "linux") {
        let growth = after
            .expect("process memory sample after import")
            .saturating_sub(before.expect("process memory sample before import"));
        assert!(
            growth < 96 * 1024 * 1024,
            "import grew peak resident memory by {growth} bytes for a 200 MB input"
        );
    }
    assert!(
        reading >= 3 && indexing >= 3,
        "both passes report progress through the large input"
    );
    let messages = store.imported_session_messages(id).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].text, "streamheadneedle");
    assert_eq!(messages[1].text, "streamtailneedle");
    assert_eq!(
        store
            .search_history("streamtailneedle", None, 20)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn cancellation_during_indexing_rolls_back_written_replacement_rows_and_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cancel.jsonl");
    std::fs::write(&path, "{\"type\":\"user\",\"sessionId\":\"cancel-midway\",\"cwd\":\"C:/original\",\"message\":{\"content\":\"originalneedle\"}}\n").unwrap();
    let store = Store::open_in_memory().unwrap();
    let cancelled = AtomicBool::new(false);
    let id = import_log(&path, "claude", &store, &[], &cancelled, &mut |_, _| {}).unwrap();
    let original = store.open_imported(id).unwrap().unwrap();
    let mut writer = BufWriter::new(File::create(&path).unwrap());
    for _ in 0..2500 {
        writer.write_all(b"{\"type\":\"user\",\"sessionId\":\"cancel-midway\",\"cwd\":\"C:/replacement\",\"message\":{\"content\":\"replacementneedle\"}}\n").unwrap();
    }
    writer.flush().unwrap();
    drop(writer);
    let mut indexed_thousand = false;
    let result = import_log(
        &path,
        "claude",
        &store,
        &[],
        &cancelled,
        &mut |done, stage| {
            if stage == "Indexing past conversations" && done >= 1000 {
                indexed_thousand = true;
                cancelled.store(true, Ordering::Relaxed);
            }
        },
    );
    assert!(
        indexed_thousand,
        "cancel only after the transaction has processed 1000 lines"
    );
    assert!(
        result
            .unwrap_err()
            .to_ascii_lowercase()
            .contains("cancelled")
    );
    assert_eq!(store.open_imported(id).unwrap().unwrap(), original);
    assert_eq!(
        store
            .search_history("originalneedle", None, 20)
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .search_history("replacementneedle", None, 20)
            .unwrap()
            .is_empty()
    );
}
