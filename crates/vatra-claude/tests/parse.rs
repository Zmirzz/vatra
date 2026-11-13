use serde_json::json;
use vatra_claude::{Parsed, parse_line};
use vatra_core::Event;

#[test]
fn every_fixture_line_maps_without_loss() {
    let parsed: Vec<_> = include_str!("fixtures/stream.jsonl").lines().flat_map(parse_line).collect();
    assert!(parsed.contains(&Parsed::NativeSession("sess-1".into())));
    assert!(parsed.contains(&Parsed::Event(Event::ToolCall {
        id: "t1".into(),
        name: "mcp__demo__echo".into(),
        arguments: json!({"text": "ășț"}),
    })));
    assert!(parsed.contains(&Parsed::Event(Event::ToolResult {
        id: "t1".into(),
        output: "ășț".into(),
        is_error: false,
    })));
    assert!(parsed.contains(&Parsed::Completed));
}

#[test]
fn unknown_and_invalid_lines_become_unmapped() {
    for line in [r#"{"type":"eveniment_nou","x":1}"#, "nu e json"] {
        assert!(matches!(&parse_line(line)[0], Parsed::Event(Event::Unmapped { raw, .. }) if raw == line));
    }
}

#[test]
fn unknown_content_block_keeps_known_siblings() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"h"},{"type":"text","text":"t"}]}}"#;
    let parsed = parse_line(line);
    assert!(parsed.iter().any(|p| matches!(p, Parsed::Event(Event::Unmapped { .. }))));
    assert!(parsed.contains(&Parsed::Event(Event::AssistantText { text: "t".into() })));
}

#[test]
fn result_error_is_failed() {
    let parsed = parse_line(r#"{"type":"result","is_error":true,"result":"limită"}"#);
    assert!(parsed.contains(&Parsed::Failed("limită".into())));
}

#[test]
fn a_lapsed_sign_in_says_how_to_sign_in_again() {
    let parsed = parse_line(
        r#"{"type":"result","is_error":true,"result":"Failed to authenticate: OAuth session expired and could not be refreshed"}"#,
    );
    let message = parsed.iter().find_map(|item| match item { Parsed::Failed(message) => Some(message.clone()), _ => None }).unwrap();
    assert!(message.starts_with("Failed to authenticate"), "{message}");
    assert!(message.contains("/login"), "{message}");
}

#[test]
fn tool_result_with_plain_string_content_and_error_flag_maps() {
    let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t9","content":"eroare","is_error":true}]}}"#;
    assert_eq!(parse_line(line), vec![Parsed::Event(Event::ToolResult {
        id: "t9".into(),
        output: "eroare".into(),
        is_error: true,
    })]);
}

#[test]
fn init_reports_the_catalog_and_warns_about_broken_mcp_servers() {
    let line = r#"{"type":"system","subtype":"init","session_id":"s","mcp_servers":[{"name":"bun","status":"connected"},{"name":"rupt","status":"failed"}]}"#;
    let parsed = parse_line(line);
    assert!(parsed.contains(&Parsed::NativeSession("s".into())));
    assert!(parsed.contains(&Parsed::Event(Event::Catalog { skills: 0, agents: 0, plugins: 0, mcp_servers: 2 })));
    let warnings: Vec<_> = parsed.iter().filter_map(|p| match p {
        Parsed::Event(Event::Warning { message, .. }) => Some(message),
        _ => None,
    }).collect();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("rupt"));
}

#[test]
fn result_without_usage_keeps_the_raw_line() {
    let line = r#"{"type":"result","is_error":false}"#;
    let parsed = parse_line(line);
    assert!(parsed.contains(&Parsed::Completed));
    assert!(parsed.iter().any(|p| matches!(p, Parsed::Event(Event::Unmapped { raw, .. }) if raw == line)));
}

#[test]
fn non_text_tool_result_parts_become_unmapped() {
    let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":[{"type":"image","data":"x"}]}]}}"#;
    let parsed = parse_line(line);
    assert!(parsed.iter().any(|p| matches!(p, Parsed::Event(Event::ToolResult { id, .. }) if id == "t")));
    assert!(parsed.iter().any(|p| matches!(p, Parsed::Event(Event::Unmapped { raw, .. }) if raw.contains("image"))));
}

fn parse_fixture(name: &str) -> Vec<Parsed> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read_to_string(path).unwrap().lines().flat_map(parse_line).collect()
}

fn event_type(parsed: &Parsed) -> Option<String> {
    match parsed {
        Parsed::Event(event) => serde_json::to_value(event).unwrap()["type"].as_str().map(String::from),
        _ => None,
    }
}

#[test]
fn subagent_lines_are_nested_under_their_parent() {
    let parsed = parse_fixture("full.jsonl");
    assert!(parsed.iter().any(|p| matches!(p, Parsed::Event(Event::Nested { event, .. })
        if matches!(**event, Event::ToolCall { .. }))));
    assert!(parsed.iter().any(|p| matches!(p, Parsed::Event(Event::SubagentStarted { .. }))));
    assert!(parsed.iter().any(|p| matches!(p, Parsed::Event(Event::SubagentFinished { status, .. }) if status == "completed")));
}

#[test]
fn hooks_usage_limits_and_catalog_have_their_own_events() {
    let parsed = parse_fixture("full.jsonl");
    for wanted in ["hook", "usage", "rate_limit", "catalog"] {
        assert!(parsed.iter().any(|p| event_type(p).as_deref() == Some(wanted)), "{wanted}");
    }
}

#[test]
fn permission_denied_becomes_tool_denied() {
    let line = r#"{"type":"system","subtype":"permission_denied","tool_name":"x","tool_use_id":"t"}"#;
    assert_eq!(parse_line(line), vec![Parsed::Event(Event::ToolDenied { id: "t".into(), name: "x".into() })]);
}

#[test]
fn missing_fields_fall_back_to_unmapped_not_panic() {
    let line = r#"{"type":"system","subtype":"task_started"}"#;
    assert!(matches!(&parse_line(line)[0], Parsed::Event(Event::Unmapped { .. })));
}

#[test]
fn rate_limit_and_usage_values_are_read() {
    let parsed = parse_line(r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":5}}"#);
    assert_eq!(parsed, vec![Parsed::Event(Event::RateLimit { status: "allowed".into(), resets_at: Some(5), windows: vec![] })]);
    let parsed = parse_line(r#"{"type":"result","is_error":false,"total_cost_usd":0.5,"usage":{"input_tokens":3,"output_tokens":4}}"#);
    assert!(parsed.contains(&Parsed::Event(Event::Usage { input_tokens: 3, output_tokens: 4, cost_usd: Some(0.5) })));
}

#[test]
fn rate_limit_windows_are_read() {
    let parsed = parse_line(r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":5,"unifiedWindows":{"five_hour":{"utilization":0.72,"resetsAt":5},"seven_day":{"utilization":0.09,"resetsAt":9}}}}"#);
    let Parsed::Event(Event::RateLimit { windows, .. }) = &parsed[0] else { panic!("{parsed:?}") };
    assert_eq!(windows.len(), 2);
    assert!(windows.iter().any(|w| w.name == "five_hour" && (w.used - 0.72).abs() < 1e-9 && w.resets_at == Some(5)));
}
