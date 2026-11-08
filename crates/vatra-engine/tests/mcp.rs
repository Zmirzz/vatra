use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use vatra_core::{McpServerConfig, PluginContext, claude_mcp_config};
use vatra_engine::{ExpandVars, HubPool, McpHub, ToolOutput, expand};

#[test]
fn projectless_mcp_hubs_are_scoped_to_the_conversation_directory(){
    let pool=HubPool::default();
    let first=pool.get(0,Path::new("/conversations/17"),&[]);
    let other=pool.get(0,Path::new("/conversations/18"),&[]);
    assert!(!Arc::ptr_eq(&first,&other));
    assert!(Arc::ptr_eq(&first,&pool.get(0,Path::new("/conversations/17"),&[])));
    pool.forget(Some(0));
    assert!(!Arc::ptr_eq(&first,&pool.get(0,Path::new("/conversations/17"),&[])));
}

fn fixture(name: &str) -> McpServerConfig {
    McpServerConfig {
        name: name.into(),
        command: Some(env!("CARGO_BIN_EXE_vatra-fixture-mcp").into()),
        ..Default::default()
    }
}

fn fixture_with_args(name: &str, args: &[&str]) -> McpServerConfig {
    McpServerConfig { args: args.iter().map(|arg| arg.to_string()).collect(), ..fixture(name) }
}

async fn started(servers: Vec<McpServerConfig>) -> McpHub {
    let hub = McpHub::new(servers, ExpandVars::default());
    hub.ensure_connected().await;
    hub
}

async fn environment_of(hub: &McpHub, tool: &str, variable: &str) -> String {
    hub.call(tool, json!({ "name": variable })).await.text
}

#[tokio::test]
async fn echo_round_trips_with_diacritics_and_uses_claude_style_names() {
    let hub = started(vec![fixture("demo")]).await;
    assert!(hub.tools().iter().any(|t| t.name == "mcp__demo__echo"));
    let out = hub.call("mcp__demo__echo", json!({"text": "ășțîâ"})).await;
    assert_eq!(out, ToolOutput { text: "ășțîâ".into(), is_error: false, images: vec![] });
}

#[tokio::test]
async fn management_reports_disabled_tools_and_disconnect_requires_explicit_reconnect() {
    let mut config=fixture("managed");config.disabled_tools=vec!["echo".into()];
    let hub=started(vec![config]).await;
    let status=hub.server_status("managed").unwrap();
    assert_eq!(status["connected"],true);
    assert!(status["tools"].as_array().unwrap().iter().any(|tool|tool["name"]=="echo"&&tool["enabled"]==false));
    assert!(!hub.tools().iter().any(|tool|tool.name=="mcp__managed__echo"));
    hub.disconnect("managed").await;
    assert_eq!(hub.server_status("managed").unwrap()["connected"],false);
    hub.ensure_connected().await;
    assert_eq!(hub.server_status("managed").unwrap()["connected"],false);
    hub.reconnect("managed").await;
    assert_eq!(hub.server_status("managed").unwrap()["connected"],true);
}

#[tokio::test]
async fn server_failure_keeps_others() {
    let broken = McpServerConfig { name: "rupt".into(), command: Some("nu-exista-binar".into()), ..Default::default() };
    let hub = started(vec![broken, fixture("demo")]).await;
    assert_eq!(hub.failures()[0].0, "rupt");
    assert!(!hub.call("mcp__demo__echo", json!({"text": "x"})).await.is_error);
}

#[tokio::test]
async fn crash_mid_call_returns_error_output_without_panicking() {
    let hub = started(vec![fixture("demo")]).await;
    assert!(hub.call("mcp__demo__crash", json!({})).await.is_error);
    assert!(!hub.call("mcp__demo__echo", json!({"text": "x"})).await.is_error);
}

#[tokio::test]
async fn server_crash_midcall_reports_and_reconnects() {
    let hub = started(vec![fixture_with_args("demo", &["--crash-after", "1"])]).await;
    assert!(!hub.call("mcp__demo__echo", json!({"text": "1"})).await.is_error);
    assert!(hub.call("mcp__demo__echo", json!({"text": "2"})).await.is_error);
    assert!(!hub.call("mcp__demo__echo", json!({"text": "3"})).await.is_error);
}

#[tokio::test]
async fn reconnect_gives_up_after_5() {
    let hub = started(vec![fixture_with_args("demo", &["--crash-after", "0"])]).await;
    for _ in 0..5 {
        assert!(hub.call("mcp__demo__echo", json!({"text": "x"})).await.is_error);
    }
    let out = hub.call("mcp__demo__echo", json!({"text": "x"})).await;
    assert!(out.is_error && out.text.contains("crashed 5 times"), "{}", out.text);
    assert!(hub.failures()[0].1.contains("crashed 5 times"));
}

#[tokio::test]
async fn reconnect_recovers_failed_server() {
    let ready = std::env::temp_dir().join(format!("vatra-ready-{}", std::process::id()));
    let _ = std::fs::remove_file(&ready);
    let ready_arg = ready.display().to_string();
    let hub = started(vec![fixture_with_args("demo", &["--ready-file", &ready_arg])]).await;
    assert_eq!(hub.failures()[0].0, "demo");

    std::fs::write(&ready, "").unwrap();
    hub.reconnect("demo").await;
    assert!(hub.failures().is_empty());
    assert!(!hub.call("mcp__demo__echo", json!({"text": "x"})).await.is_error);
    let _ = std::fs::remove_file(&ready);
}

#[tokio::test]
async fn broken_arg_schema_normalized() {
    let hub = started(vec![fixture("demo")]).await;
    let approve = hub.tools().into_iter().find(|tool| tool.name == "mcp__demo__approve").unwrap();
    assert!(approve.parameters["properties"]["input"].is_object(), "{}", approve.parameters);
}

#[tokio::test]
async fn unknown_tool_returns_error_output() {
    let hub = started(vec![]).await;
    assert!(hub.call("mcp__nimic__x", json!({})).await.is_error);
}

#[test]
fn claude_config_has_the_documented_shape() {
    let value = claude_mcp_config(&[fixture("demo")]);
    assert!(value["mcpServers"]["demo"]["command"].is_string());
}

#[test]
fn claude_config_writes_http_for_url_servers() {
    let remote =
        McpServerConfig { name: "remote".into(), url: Some("https://example.test/mcp".into()), ..Default::default() };
    let value = claude_mcp_config(&[remote]);
    assert_eq!(value["mcpServers"]["remote"]["type"], "http");
    assert_eq!(value["mcpServers"]["remote"]["url"], "https://example.test/mcp");
}

#[tokio::test]
async fn image_content_comes_back_as_an_image() {
    let hub = started(vec![fixture("demo")]).await;
    let out = hub.call("mcp__demo__image", json!({})).await;
    assert!(out.text.is_empty());
    assert_eq!(out.images.len(), 1);
    assert_eq!(out.images[0].media_type, "image/png");
    assert_eq!(out.images[0].base64, "aGVsbG8=");
}

#[tokio::test]
async fn hung_tool_times_out_and_the_server_stays_usable() {
    let hung = McpServerConfig { tool_timeout_sec: Some(1), ..fixture("demo") };
    let hub = started(vec![hung]).await;
    assert!(hub.call("mcp__demo__hang", json!({})).await.is_error);
    assert!(!hub.call("mcp__demo__echo", json!({"text": "x"})).await.is_error);
}

#[tokio::test]
async fn tool_timeout_respected() {
    let hung = McpServerConfig { tool_timeout_sec: Some(1), ..fixture("demo") };
    let hub = started(vec![hung]).await;
    let started_at = std::time::Instant::now();
    let out = hub.call("mcp__demo__hang", json!({})).await;
    assert!(out.is_error && out.text.contains("1 seconds"), "{}", out.text);
    assert!(started_at.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn plugin_tool_name() {
    let plugin = McpServerConfig { plugin: Some("universal-modder".into()), ..fixture("fal") };
    let hub = started(vec![plugin]).await;
    assert!(hub.tools().iter().any(|t| t.name == "mcp__plugin_universal-modder_fal__echo"));
    assert!(!hub.call("mcp__plugin_universal-modder_fal__echo", json!({"text": "x"})).await.is_error);
}

#[tokio::test]
async fn tool_name_with_double_underscore_in_tool() {
    let hub = started(vec![fixture("demo")]).await;
    assert!(hub.tools().iter().any(|t| t.name == "mcp__demo__lookup__deep"));
    assert_eq!(hub.call("mcp__demo__lookup__deep", json!({})).await.text, "deep");
}

#[tokio::test]
async fn enabled_tools_filter() {
    let only_echo = McpServerConfig { enabled_tools: Some(vec!["echo".into()]), ..fixture("demo") };
    let hub = started(vec![only_echo]).await;
    let names: Vec<_> = hub.tools().into_iter().map(|t| t.name).collect();
    assert_eq!(names, ["mcp__demo__echo"]);
}

#[tokio::test]
async fn disabled_after_enabled() {
    let server = McpServerConfig {
        enabled_tools: Some(vec!["echo".into(), "lookup".into()]),
        disabled_tools: vec!["echo".into()],
        ..fixture("demo")
    };
    let hub = started(vec![server]).await;
    let names: Vec<_> = hub.tools().into_iter().map(|t| t.name).collect();
    assert_eq!(names, ["mcp__demo__lookup"]);
}

#[tokio::test]
async fn disabled_tool_call_is_refused() {
    let server = McpServerConfig { disabled_tools: vec!["echo".into()], ..fixture("demo") };
    let hub = started(vec![server]).await;
    let out = hub.call("mcp__demo__echo", json!({"text": "x"})).await;
    assert!(out.is_error && out.text.contains("disabled"), "{}", out.text);
}

#[tokio::test]
async fn lazy_connect_only_on_first_use() {
    let hub = McpHub::new(vec![fixture("demo")], ExpandVars::default());
    assert!(hub.tools().is_empty());
    assert!(hub.failures().is_empty());
    let out = hub.call("mcp__demo__echo", json!({"text": "x"})).await;
    assert!(!out.is_error);
    assert!(hub.tools().iter().any(|t| t.name == "mcp__demo__echo"));
}

#[tokio::test]
async fn projects_get_separate_hubs() {
    let pool = HubPool::default();
    let servers = vec![fixture("demo")];
    let first = pool.get(1, Path::new("/project-one"), &servers);
    let other = pool.get(2, Path::new("/project-two"), &servers);
    assert!(!Arc::ptr_eq(&first, &other));
    assert!(Arc::ptr_eq(&first, &pool.get(1, Path::new("/project-one"), &servers)));
}

#[tokio::test]
async fn project_and_plugin_variables_resolve() {
    let root = PathBuf::from("/plugins/fal");
    let data = PathBuf::from("/plugins/fal-data");
    let env = [
        ("P", "${CLAUDE_PROJECT_DIR}"),
        ("R", "${CLAUDE_PLUGIN_ROOT}"),
        ("D", "${CLAUDE_PLUGIN_DATA}"),
        ("U", "${user_config.region}"),
    ];
    let server = McpServerConfig {
        plugin: Some("fal-plugin".into()),
        env: env.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect(),
        plugin_context: Some(PluginContext {
            root: root.clone(),
            data: data.clone(),
            user_config: [("region".to_string(), "eu".to_string())].into(),
        }),
        ..fixture("tools")
    };
    let hub = McpHub::new(vec![server], ExpandVars::from_env(Path::new("/project"), vec![]));
    hub.ensure_connected().await;
    let tool = "mcp__plugin_fal-plugin_tools__environment";
    assert_eq!(environment_of(&hub, tool, "P").await, Path::new("/project").display().to_string());
    assert_eq!(environment_of(&hub, tool, "R").await, root.display().to_string());
    assert_eq!(environment_of(&hub, tool, "D").await, data.display().to_string());
    assert_eq!(environment_of(&hub, tool, "U").await, "eu");
}

#[tokio::test]
async fn api_key_cant_be_expanded() {
    let env: std::collections::BTreeMap<String, String> = [("VATRA_TEST_KEY".to_string(), "secret".to_string())].into();
    let vars = ExpandVars { env, hidden: vec!["PROBE_PROVIDER_KEY".into()], ..Default::default() };
    let leaked =
        McpServerConfig { env: [("K".to_string(), "${VATRA_TEST_KEY}".to_string())].into(), ..fixture("leak") };
    let hub = McpHub::new(vec![leaked], vars.clone());
    hub.ensure_connected().await;
    assert!(hub.failures()[0].1.contains("reserved"), "{:?}", hub.failures());

    let expanded = expand("${VATRA_TEST_KEY} ${PROBE_PROVIDER_KEY}", &vars);
    assert_eq!(expanded.text, "${VATRA_TEST_KEY} ${PROBE_PROVIDER_KEY}");
    assert_eq!(expanded.reserved, ["VATRA_TEST_KEY"]);
    assert_eq!(expanded.missing, ["PROBE_PROVIDER_KEY"]);
}

#[tokio::test]
async fn server_process_doesnt_inherit_api_key() {
    // SAFETY: the variables have names that no other test reads or writes.
    unsafe {
        std::env::set_var("VATRA_TEST_INHERIT", "leaked");
        std::env::set_var("PROBE_HIDDEN_KEY", "leaked");
    }
    let hub = McpHub::new(
        vec![fixture("demo")],
        ExpandVars::from_env(Path::new("/project"), vec!["PROBE_HIDDEN_KEY".into()]),
    );
    hub.ensure_connected().await;
    let tool = "mcp__demo__environment";
    assert_eq!(environment_of(&hub, tool, "VATRA_TEST_INHERIT").await, "absent");
    assert_eq!(environment_of(&hub, tool, "PROBE_HIDDEN_KEY").await, "absent");

    let named =
        McpServerConfig { env: [("VATRA_TEST_INHERIT".to_string(), "literal".to_string())].into(), ..fixture("named") };
    let hub = McpHub::new(vec![named], ExpandVars::from_env(Path::new("/project"), vec![]));
    hub.ensure_connected().await;
    assert_eq!(environment_of(&hub, "mcp__named__environment", "VATRA_TEST_INHERIT").await, "literal");
}

#[test]
fn expand_default() {
    let plain = expand("${NU_EXISTA_DELOC:-implicit}", &ExpandVars::default());
    assert_eq!(plain.text, "implicit");
    assert!(plain.missing.is_empty());

    let vars = ExpandVars { env: [("PORT".to_string(), "8080".to_string())].into(), ..Default::default() };
    assert_eq!(expand("http://x:${PORT:-80}", &vars).text, "http://x:8080");
}

#[test]
fn expand_missing_keeps_literal_and_reports() {
    let out = expand("a ${NU_EXISTA} b", &ExpandVars::default());
    assert_eq!(out.text, "a ${NU_EXISTA} b");
    assert_eq!(out.missing, ["NU_EXISTA"]);
}

#[test]
fn expand_plugin_and_user_references() {
    let vars = ExpandVars {
        plugin_root: Some("/plugins/fal".into()),
        user_config: [("region".to_string(), "eu".to_string())].into(),
        ..Default::default()
    };
    let out = expand("${CLAUDE_PLUGIN_ROOT}/bin ${user_config.region}", &vars);
    assert!(out.text.ends_with("/bin eu"), "{}", out.text);
    assert!(out.missing.is_empty());
}

/// A server that starts a child is ended with its hub, the child included. The child pulses into a file while it runs,
/// so the test sees whether it is alive from the file alone.
#[cfg(windows)]
#[tokio::test]
async fn server_tree_ends_with_the_hub() {
    let scratch = tempfile::tempdir().unwrap();
    let pulse = scratch.path().join("pulse.txt");
    let hub = started(vec![fixture_with_args("tree", &["--child-pulse", &pulse.to_string_lossy()])]).await;
    assert!(hub.tools().iter().any(|t| t.name == "mcp__tree__echo"));
    for _ in 0..100 {
        if pulse.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(pulse.exists(), "the server's child never started");

    drop(hub);
    let mut stopped = false;
    for _ in 0..40 {
        let before = std::fs::read_to_string(&pulse).ok();
        tokio::time::sleep(Duration::from_millis(150)).await;
        if std::fs::read_to_string(&pulse).ok() == before {
            stopped = true;
            break;
        }
    }
    assert!(stopped, "the child of the server is still running after its hub was dropped");
}
