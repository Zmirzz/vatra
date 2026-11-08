use std::{
    io::Cursor,
    sync::atomic::{AtomicBool, Ordering},
};
use vatra_core::Store;
use vatra_import::{import_log, parse_claude_log, parse_codex_log};

#[test]
fn claude_user_assistant_text_only_and_corrupt_lines_skipped() {
    let lines = b"{\"type\":\"user\",\"sessionId\":\"c1\",\"cwd\":\"E:/project\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Hello\"},{\"type\":\"tool_result\",\"content\":\"secret tool output\"}]}}\n{\"type\":\"api-request-start\",\"message\":{\"content\":\"ignore\"}}\n{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Answer\"}]}}\n\xff\xfe\n{\"truncated\"";
    let parsed = parse_claude_log(Cursor::new(lines));
    assert_eq!(parsed.native_id, "c1");
    assert_eq!(parsed.cwd.as_deref(), Some("E:/project"));
    assert_eq!(parsed.messages.len(), 2);
    assert_eq!(parsed.messages[0].text, "Hello");
    assert_eq!(parsed.skipped_lines, 3);
}

#[test]
fn codex_response_items_and_session_meta_no_event_duplicates() {
    let lines = r#"{"type":"session_meta","payload":{"id":"x1","cwd":"E:/x"}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Question"}]}}
{"type":"event_msg","payload":{"type":"user_message","message":"Question"}}
{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Reply"}]}}
{"type":"response_item","payload":{"type":"function_call_output","output":"tool output"}}
"#;
    let parsed = parse_codex_log(Cursor::new(lines));
    assert_eq!(parsed.native_id, "x1");
    assert_eq!(parsed.messages.len(), 2);
    assert_eq!(parsed.messages[1].text, "Reply");
}

#[test]
fn import_same_log_no_duplicates_and_can_be_cancelled() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("log.jsonl");
    std::fs::write(&path,"{\"type\":\"user\",\"sessionId\":\"test\",\"message\":{\"content\":\"unique question\"}}\n").unwrap();
    let store = Store::open_in_memory().unwrap();
    let cancelled = AtomicBool::new(false);
    import_log(&path, "claude", &store, &[], &cancelled, &mut |_, _| {}).unwrap();
    import_log(&path, "claude", &store, &[], &cancelled, &mut |_, _| {}).unwrap();
    assert_eq!(store.search_history("unique", None, 20).unwrap().len(), 1);
    cancelled.store(true, Ordering::Relaxed);
    assert!(import_log(&path, "claude", &store, &[], &cancelled, &mut |_, _| {}).is_err());
    assert_eq!(store.search_history("unique", None, 20).unwrap().len(), 1);
}

fn log(path: &std::path::Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn past_conversations_are_listed_by_folder_without_reading_whole_logs() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = vatra_import::SourceDirs {
        claude: tmp.path().join(".claude"),
        codex: tmp.path().join(".codex"),
        agents: tmp.path().join(".claude").join("agents"),
        home: tmp.path().to_path_buf(),
    };
    let claude = |id: &str, cwd: &str, at: &str| {
        format!("{{\"type\":\"user\",\"sessionId\":\"{id}\",\"cwd\":{cwd:?},\"timestamp\":\"{at}\",\"message\":{{\"role\":\"user\",\"content\":\"hi\"}}}}\n")
    };
    log(&dirs.claude.join("projects/a/one.jsonl"), &claude("1", "E:/code/Shop", "2026-10-01T10:00:00Z"));
    log(&dirs.claude.join("projects/a/two.jsonl"), &claude("2", "e:/code/shop/", "2026-10-05T10:00:00Z"));
    log(
        &dirs.codex.join("sessions/2026/x.jsonl"),
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"c1\",\"cwd\":\"E:/code/notes\",\"timestamp\":\"2026-09-01T10:00:00Z\"}}\n",
    );
    log(&dirs.claude.join("projects/b/none.jsonl"), "{\"type\":\"user\",\"message\":{\"content\":\"no folder\"}}\n");
    let known = vec![vatra_core::Project { id: 7, name: "Shop".into(), path: Some("E:/code/shop".into()) }];

    let listed = vatra_import::history_projects(&dirs, &known);

    assert_eq!(listed.len(), 3, "{listed:?}");
    assert_eq!(listed[0].name, "Shop", "the newest folder comes first");
    assert_eq!(listed[0].conversations, 2, "both spellings of one folder are one project");
    assert!(listed[0].known);
    assert_eq!(listed[0].last_active, "2026-10-05T10:00:00Z");
    let notes = listed.iter().find(|project| project.name == "notes").unwrap();
    assert!(!notes.known);
    let loose = listed.iter().find(|project| project.key.is_empty()).unwrap();
    assert_eq!(loose.conversations, 1);

    let chosen = vatra_import::history_files_for(&dirs, &["e:/code/notes".to_string()]);
    assert_eq!(chosen.len(), 1);
    assert_eq!(chosen[0].1, "codex");
}
