//! Catalogue metadata only. These requests never generate a model response.
use std::time::Duration;
use serde_json::{Value, json};

pub(crate) fn efforts(values: &Value) -> Vec<Value> {
    values.as_array().into_iter().flatten().filter_map(|v| {
        let id = v.as_str()?;
        Some(json!({"id":id,"name":id}))
    }).collect()
}

pub(crate) fn catalogue(body: &Value, openrouter: bool) -> Vec<Value> {
    body["data"].as_array().into_iter().flatten().filter_map(|model| {
        let id = model["id"].as_str().filter(|id| !id.is_empty())?;
        // OpenRouter explicitly lists tool support. Other /models APIs often omit capabilities.
        if openrouter && !model["supported_parameters"].as_array().is_some_and(|p| p.iter().any(|v| v == "tools")) {
            return None;
        }
        if model["lifecycle"] == "retired" { return None; }
        let mut levels = efforts(&model["supported_effort_levels"]);
        if levels.is_empty() && model["capabilities"]["effort"]["supported"] == true {
            levels = ["low","medium","high","xhigh","max"].into_iter()
                .filter(|level| model["capabilities"]["effort"][*level]["supported"] == true)
                .map(|id| json!({"id":id,"name":id})).collect();
        }
        if levels.is_empty() && openrouter && model["supported_parameters"].as_array().is_some_and(|p| p.iter().any(|v| v == "reasoning" || v == "reasoning_effort")) {
            levels = efforts(&json!(["low","medium","high"]));
        }
        Some(json!({"id":id,"name":model["name"].as_str().or(model["display_name"].as_str()).unwrap_or(id),"efforts":levels}))
    }).collect()
}

pub(crate) async fn api(base: &str, key: Option<&str>, anthropic: bool, current: &str) -> Result<Value, String> {
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(20))
        .build().map_err(|_| "Couldn't create the catalogue request".to_string())?;
    let openrouter = base.trim_end_matches('/') == "https://openrouter.ai/api/v1";
    let base = base.trim_end_matches('/');
    let url = if anthropic && !base.ends_with("/v1") { format!("{base}/v1/models") } else { format!("{base}/models") };
    let mut models = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..10 {
        let mut endpoint = reqwest::Url::parse(&url).map_err(|_| "Invalid provider address".to_string())?;
        if let Some(cursor) = &after { endpoint.query_pairs_mut().append_pair("after_id",cursor); }
        let mut request = client.get(endpoint);
        if anthropic {
            request = request.header("anthropic-version", "2023-06-01");
            if let Some(key) = key { request = request.header("x-api-key", key); }
        } else if let Some(key) = key { request = request.bearer_auth(key); }
        let mut response = request.send().await.map_err(|_| "Couldn't load this provider's model list".to_string())?;
        if !response.status().is_success() { return Err(format!("Couldn't load models (HTTP {}). Check the connection in Settings.",response.status().as_u16())); }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "Couldn't read the model list".to_string())? {
            if bytes.len() + chunk.len() > 8 * 1024 * 1024 { return Err("The model list is too large".into()); }
            bytes.extend_from_slice(&chunk);
        }
        let body: Value = serde_json::from_slice(&bytes).map_err(|_| "The provider returned an invalid model list".to_string())?;
        if !body["data"].is_array() { return Err("The provider didn't return a model list".into()); }
        models.extend(catalogue(&body,openrouter));
        if !anthropic || body["has_more"] != true { break; }
        after = body["last_id"].as_str().map(String::from);
        if after.is_none() { break; }
    }
    models.sort_by(|a,b| a["name"].as_str().cmp(&b["name"].as_str()));
    models.dedup_by(|a,b| a["id"] == b["id"]);
    Ok(json!({"models":models,"current_model":current,"efforts":[],"current_effort":null}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn openrouter_filters_non_tool_models_and_reports_only_declared_reasoning() {
        let found = catalogue(&json!({"data":[
            {"id":"reasoner","supported_parameters":["tools","reasoning"]},
            {"id":"chat","supported_parameters":["tools"]},
            {"id":"image","supported_parameters":[]}
        ]}),true);
        assert_eq!(found.len(),2);
        assert_eq!(found[0]["efforts"].as_array().unwrap().len(),3);
        assert_eq!(found[1]["efforts"],json!([]));
    }
}
