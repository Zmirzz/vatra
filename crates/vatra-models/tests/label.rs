use vatra_models::estimate::Fit;
use vatra_models::gguf::ModelFacts;
use vatra_models::label::{
    CheckResults, Label, LabelReason, OpenRouterModel, Speed, StepOutcome, ToolScore, api_estimate,
    estimated, measured,
};

fn checks(tools: (u8, u8), mcp: Option<StepOutcome>, context: Option<u32>) -> CheckResults {
    CheckResults {
        load: Some(StepOutcome::Passed),
        tools: ToolScore {
            passed: tools.0,
            of: tools.1,
        },
        mcp,
        context,
        ..Default::default()
    }
}

fn passed() -> Option<StepOutcome> {
    Some(StepOutcome::Passed)
}

fn failed(reason: &str) -> Option<StepOutcome> {
    Some(StepOutcome::Failed {
        reason: reason.into(),
    })
}

fn tool_facts() -> ModelFacts {
    ModelFacts {
        has_chat_template: true,
        chat_template_mentions_tools: true,
        ..Default::default()
    }
}

#[test]
fn complete_needs_2of2_mcp_ctx16k() {
    let (label, reasons) = measured(&checks((2, 2), passed(), Some(16_384)));
    assert_eq!(label, Label::Complete);
    assert!(reasons.is_empty());
}

#[test]
fn one_of_two_is_partial_unreliable() {
    let (label, reasons) = measured(&checks((1, 2), passed(), Some(32_768)));
    assert_eq!(label, Label::Partial);
    assert_eq!(reasons, vec![LabelReason::ToolsUnreliable]);
}

#[test]
fn mcp_fail_is_partial() {
    let (label, reasons) = measured(&checks((2, 2), failed("timeout"), Some(32_768)));
    assert_eq!(label, Label::Partial);
    assert_eq!(reasons, vec![LabelReason::McpFailed]);

    let (label, reasons) = measured(&checks((2, 2), None, Some(32_768)));
    assert_eq!(label, Label::Partial);
    assert_eq!(reasons, vec![LabelReason::McpFailed]);
}

#[test]
fn ctx_8k_is_partial() {
    let (label, reasons) = measured(&checks((2, 2), passed(), Some(8_192)));
    assert_eq!(label, Label::Partial);
    assert_eq!(reasons, vec![LabelReason::ContextSmall]);
}

#[test]
fn zero_tools_is_chat_only() {
    let (label, reasons) = measured(&checks((0, 2), passed(), Some(32_768)));
    assert_eq!(label, Label::ChatOnly);
    assert_eq!(reasons, vec![LabelReason::NoTools]);
}

#[test]
fn slow_does_not_change_label() {
    let mut results = checks((2, 2), passed(), Some(32_768));
    results.speed = Some(Speed {
        prompt_tps: Some(120.0),
        gen_tps: Some(3.0),
        seconds: 12.0,
    });
    let (label, reasons) = measured(&results);
    assert_eq!(label, Label::Complete);
    assert!(reasons.contains(&LabelReason::Slow));
}

#[test]
fn failed_load_is_not_checked() {
    let mut results = checks((2, 2), passed(), Some(32_768));
    results.load = Some(StepOutcome::Failed {
        reason: "crashed on start".into(),
    });
    assert_eq!(measured(&results), (Label::NotChecked, vec![]));
}

#[test]
fn estimate_without_tool_template_is_chat_only() {
    let no_tools = ModelFacts {
        has_chat_template: true,
        chat_template_mentions_tools: false,
        ..Default::default()
    };
    assert_eq!(
        estimated(Some(&no_tools), Some(32_768), Some(Fit::Gpu)),
        (Label::ChatOnly, vec![LabelReason::TemplateHasNoTools])
    );

    let no_template = ModelFacts::default();
    assert_eq!(
        estimated(Some(&no_template), Some(32_768), Some(Fit::Gpu)),
        (Label::ChatOnly, vec![LabelReason::NoChatTemplate])
    );

    assert_eq!(
        estimated(None, Some(32_768), Some(Fit::Gpu)),
        (Label::NotChecked, vec![])
    );
}

#[test]
fn estimate_too_large_keeps_label_adds_reason() {
    assert_eq!(
        estimated(Some(&tool_facts()), Some(32_768), Some(Fit::TooLarge)),
        (Label::Complete, vec![LabelReason::NotEnoughMemory])
    );
    assert_eq!(
        estimated(Some(&tool_facts()), Some(4_096), Some(Fit::TooLarge)),
        (
            Label::Partial,
            vec![LabelReason::ContextSmall, LabelReason::NotEnoughMemory]
        )
    );
}

#[test]
fn anthropic_driver_estimates_complete() {
    assert_eq!(api_estimate("anthropic", None), (Label::Complete, vec![]));
}

#[test]
fn openrouter_without_tools_is_chat_only() {
    let model = OpenRouterModel {
        id: "some/model".into(),
        context_length: Some(131_072),
        supported_parameters: vec!["temperature".into()],
    };
    assert_eq!(
        api_estimate("openrouter", Some(&model)),
        (Label::ChatOnly, vec![LabelReason::NoTools])
    );
}

#[test]
fn openrouter_with_tools_small_ctx_is_partial() {
    let model = OpenRouterModel {
        id: "some/model".into(),
        context_length: Some(8_192),
        supported_parameters: vec!["tools".into(), "temperature".into()],
    };
    assert_eq!(
        api_estimate("openrouter", Some(&model)),
        (Label::Partial, vec![LabelReason::ContextSmall])
    );
}
