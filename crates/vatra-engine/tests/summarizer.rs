mod common;

use common::*;
use vatra_core::Summarizer;
use vatra_engine::OpenAiSummarizer;

#[tokio::test]
async fn invalid_model_json_is_an_error_for_the_fallback_to_handle() {
    let server = sequence(vec![text("nu e json")]).await;
    assert!(OpenAiSummarizer::new(client(&server, None)).summarize("t").await.is_err());
}

#[tokio::test]
async fn json_wrapped_in_a_code_fence_is_accepted() {
    let reply = "```json\n{\"summary\":\"s\",\"decisions\":[\"d\"],\"remaining\":[]}\n```";
    let server = sequence(vec![text(reply)]).await;
    let fields = OpenAiSummarizer::new(client(&server, None)).summarize("t").await.unwrap();
    assert_eq!(fields.summary, "s");
    assert_eq!(fields.decisions, vec!["d".to_string()]);
}
