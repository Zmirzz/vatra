//! The `models.*`, `hardware.get` and `llama.status` requests.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use vatra_models::suggest::Task;

use crate::service::{Service, ServiceError};

const INVALID_PARAMS: i64 = -32602;
const FAILED: i64 = -32000;

type Reply = Result<Value, (i64, String)>;

fn parse<T: for<'de> Deserialize<'de>>(params: Value) -> Result<T, (i64, String)> {
    serde_json::from_value(params).map_err(|error| (INVALID_PARAMS, error.to_string()))
}

fn failed(message: impl std::fmt::Display) -> (i64, String) {
    (FAILED, message.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Search {
    query: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Files {
    repo: String,
    ctx: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Download {
    repo: String,
    variant: String,
    #[serde(default)]
    mmproj: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Slug {
    slug: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddPath {
    path: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Suggest {
    task: Task,
}

/// Where the Hugging Face token is kept in the keychain.
pub const HF_SECRET: &str = "vatra/huggingface";

#[derive(Deserialize)]
struct Token {
    token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Check {
    agent: String,
    #[serde(default)]
    consent_cost: bool,
}

pub(crate) async fn handle(service: &Arc<Service>, method: &str, params: Value) -> Reply {
    let models = service.models().map_err(failed)?;
    match method {
        "hardware.get" => Ok(json!(models.hardware().await)),
        "llama.status" => Ok(models.llama_status()),
        "models.search" => {
            let request: Search = parse(params)?;
            Ok(json!(models.search(&request.query).await.map_err(failed)?))
        }
        "models.files" => {
            let request: Files = parse(params)?;
            Ok(json!(models.files(&request.repo, request.ctx).await.map_err(failed)?))
        }
        "models.suggest" => {
            let request: Suggest = parse(params)?;
            Ok(json!(models.suggest(request.task).await))
        }
        "models.download" => {
            let request: Download = parse(params)?;
            Ok(json!(models.download(&request.repo, &request.variant, request.mmproj).await.map_err(failed)?))
        }
        "models.download_cancel" => {
            models.cancel_download();
            Ok(json!({}))
        }
        "models.add_path" => {
            let request: AddPath = parse(params)?;
            Ok(json!(models.add_path(&request.path).await.map_err(failed)?))
        }
        "models.installed" => Ok(json!(models.installed().await)),
        "models.remove" => {
            let request: Slug = parse(params)?;
            models.remove(&request.slug).await.map_err(failed)?;
            Ok(json!({}))
        }
        "models.start" => {
            let request: Slug = parse(params)?;
            Ok(json!(models.start(&request.slug).await.map_err(failed)?))
        }
        "models.stop" => {
            models.stop().await.map_err(failed)?;
            Ok(json!({}))
        }
        "models.check" => {
            let request: Check = parse(params)?;
            match service.check_model(&request.agent, request.consent_cost).await {
                Ok(record) => Ok(json!(record)),
                Err(ServiceError::Models(message)) => Err(failed(message)),
                Err(other) => Err(failed(other)),
            }
        }
        "hf.status" => Ok(json!({ "hasToken": models.has_hf_token() })),
        "hf.set_token" => {
            let request: Token = parse(params)?;
            let secrets = service.agents().secrets();
            let token = request.token.filter(|token| !token.trim().is_empty());
            match &token {
                Some(token) => secrets.set(HF_SECRET, token.trim()).map_err(failed)?,
                None => secrets.delete(HF_SECRET).map_err(failed)?,
            }
            models.set_hf_token(token.map(|token| token.trim().to_string()));
            Ok(json!({ "hasToken": models.has_hf_token() }))
        }
        "models.labels" => Ok(json!(models.labels(&service.agents().configured_models()).await)),
        _ => Err((-32601, format!("unknown method `{method}`"))),
    }
}
