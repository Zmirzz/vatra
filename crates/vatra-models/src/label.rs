//! What a model is known to do: from its checks when they ran, or from its metadata when they didn't.

use serde::{Deserialize, Serialize};

use crate::estimate::Fit;
use crate::gguf::ModelFacts;

/// Smallest context an agent can work with.
pub const MIN_AGENT_CTX: u32 = 16_384;
/// Generation below this many tokens per second feels slow.
pub const SLOW_TPS: f64 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Label {
    Complete,
    Partial,
    ChatOnly,
    NotChecked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LabelReason {
    ToolsUnreliable,
    McpFailed,
    ContextSmall,
    NoTools,
    Slow,
    TemplateHasNoTools,
    NoChatTemplate,
    NotEnoughMemory,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepOutcome {
    Passed,
    Failed { reason: String },
    Skipped { reason: String },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Speed {
    pub prompt_tps: Option<f64>,
    pub gen_tps: Option<f64>,
    pub seconds: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolScore {
    pub passed: u8,
    pub of: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResults {
    pub load: Option<StepOutcome>,
    pub speed: Option<Speed>,
    pub speed_step: Option<StepOutcome>,
    pub tools: ToolScore,
    pub tool_failures: Vec<String>,
    pub mcp: Option<StepOutcome>,
    pub context: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterModel {
    pub id: String,
    #[serde(default)]
    pub context_length: Option<u64>,
    #[serde(default)]
    pub supported_parameters: Vec<String>,
}

/// The label from the checks that ran on this machine. Nothing ran if the model did not load.
pub fn measured(results: &CheckResults) -> (Label, Vec<LabelReason>) {
    if matches!(results.load, Some(StepOutcome::Failed { .. })) {
        return (Label::NotChecked, vec![]);
    }
    if results.tools.passed == 0 {
        return (Label::ChatOnly, vec![LabelReason::NoTools]);
    }

    let mut reasons = Vec::new();
    if results.tools.passed < results.tools.of {
        reasons.push(LabelReason::ToolsUnreliable);
    }
    if !matches!(results.mcp, Some(StepOutcome::Passed)) {
        reasons.push(LabelReason::McpFailed);
    }
    if results.context.is_some_and(|c| c < MIN_AGENT_CTX) {
        reasons.push(LabelReason::ContextSmall);
    }
    let label = if reasons.is_empty() {
        Label::Complete
    } else {
        Label::Partial
    };
    // Slow is a note only; it never changes the label.
    if results
        .speed
        .and_then(|s| s.gen_tps)
        .is_some_and(|tps| tps < SLOW_TPS)
    {
        reasons.push(LabelReason::Slow);
    }
    (label, reasons)
}

/// The label from the model's metadata, before anything ran.
pub fn estimated(
    facts: Option<&ModelFacts>,
    fitted_ctx: Option<u32>,
    fit: Option<Fit>,
) -> (Label, Vec<LabelReason>) {
    let Some(facts) = facts else {
        return (Label::NotChecked, vec![]);
    };
    let (label, mut reasons) = if !facts.has_chat_template {
        (Label::ChatOnly, vec![LabelReason::NoChatTemplate])
    } else if !facts.chat_template_mentions_tools {
        (Label::ChatOnly, vec![LabelReason::TemplateHasNoTools])
    } else if fitted_ctx.is_some_and(|c| c < MIN_AGENT_CTX) {
        (Label::Partial, vec![LabelReason::ContextSmall])
    } else {
        (Label::Complete, vec![])
    };
    if fit == Some(Fit::TooLarge) {
        reasons.push(LabelReason::NotEnoughMemory);
    }
    (label, reasons)
}

/// The label for a hosted model, from the provider's listing.
pub fn api_estimate(
    driver: &str,
    openrouter: Option<&OpenRouterModel>,
) -> (Label, Vec<LabelReason>) {
    if driver == "anthropic" {
        return (Label::Complete, vec![]);
    }
    let Some(model) = openrouter else {
        return (Label::NotChecked, vec![]);
    };
    if !model.supported_parameters.iter().any(|p| p == "tools") {
        return (Label::ChatOnly, vec![LabelReason::NoTools]);
    }
    if model
        .context_length
        .is_some_and(|c| c < u64::from(MIN_AGENT_CTX))
    {
        (Label::Partial, vec![LabelReason::ContextSmall])
    } else {
        (Label::Complete, vec![])
    }
}
