use vatra_service::config::AgentConfig;

fn acp(binary: &str, args: &[&str]) -> AgentConfig {
    AgentConfig::Acp {
        subscription: false,
        chatgpt_plan: false,
        codex_binary: None,
        binary: binary.into(),
        args: args.iter().map(|s| (*s).into()).collect(),
        startup_timeout_sec: 30,
        turn_timeout_sec: 900,
    }
}

#[test]
fn claude_acp_requires_an_explicit_api_key_without_affecting_subscription_route() {
    for config in [
        acp("claude-agent-acp.exe", &[]),
        acp("node.exe", &[r"C:\tools\claude-agent-acp\dist\index.js"]),
    ] {
        assert!(
            config
                .validate_acp_api_key(None)
                .unwrap_err()
                .contains("ANTHROPIC_API_KEY")
        );
        assert!(
            config
                .validate_acp_api_key(Some(std::ffi::OsStr::new("")))
                .is_err()
        );
        assert!(
            config
                .validate_acp_api_key(Some(std::ffi::OsStr::new("test-not-a-real-key")))
                .is_ok()
        );
    }
    assert!(
        acp("generic-agent.exe", &[])
            .validate_acp_api_key(None)
            .is_ok()
    );
    assert!(
        AgentConfig::Claude {
            binary: "claude.exe".into()
        }
        .validate_acp_api_key(None)
        .is_ok()
    );
}

#[test]
fn codex_siwc_configuration_is_explicit_and_old_configs_still_work() {
    let text = r#"summarizer="local"
[agents.local]
kind="model"
base_url="http://127.0.0.1:8080/v1"
model="local"
[agents.codex]
kind="acp"
binary="node.exe"
args=['C:\tools\codex-acp\dist\index.js']
subscription=true
chatgpt_plan=true
codex_binary='C:\tools\codex.exe'
"#;
    let config = vatra_service::config::Config::parse(text).unwrap();
    assert!(matches!(
        &config.agents["codex"],
        AgentConfig::Acp {
            subscription: true,
            chatgpt_plan: true,
            ..
        }
    ));
    assert!(
        vatra_service::config::Config::parse(
            &text.replace("subscription=true", "subscription=false")
        )
        .is_err()
    );
    assert!(
        vatra_service::config::Config::parse(
            &text.replace("chatgpt_plan=true", "chatgpt_plan=false")
        )
        .is_ok()
    );
}
