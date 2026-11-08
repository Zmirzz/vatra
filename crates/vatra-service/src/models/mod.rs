//! Local models: the ones Vatra downloaded or was pointed at, the llama.cpp server that runs them, and their checks.

pub mod check;
pub mod probe;
pub(crate) mod rpc;
pub mod download;
pub mod manager;
pub mod server;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use vatra_core::LocalModelRow;
use vatra_models::gguf::ModelFacts;

pub use manager::ModelManager;

/// Every local model is an agent named `local:<slug>`.
pub const AGENT_PREFIX: &str = "local:";
/// The context a model gets unless the person picks another, capped at what the model supports.
pub const DEFAULT_CTX: u32 = 32_768;
/// The smallest context `--fit` may lower to, and the least that counts as enough for agent work.
pub const MIN_AGENT_CTX: u32 = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AddedBy {
    Download,
    Path,
}

/// What `llama-fit-params` measured on the real file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitResult {
    pub gpu_layers: String,
    pub ctx: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSettings {
    pub ctx: u32,
    pub mmproj: Option<PathBuf>,
    #[serde(default)]
    pub fit: Option<FitResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModel {
    pub slug: String,
    pub repo: Option<String>,
    pub variant: Option<String>,
    /// Every part, in order; the first is the one llama.cpp opens.
    pub files: Vec<PathBuf>,
    pub sha256: Vec<String>,
    pub size_bytes: u64,
    pub facts: Option<ModelFacts>,
    pub added_by: AddedBy,
    pub settings: ModelSettings,
    pub created_at: i64,
}

impl LocalModel {
    pub fn agent(&self) -> String {
        format!("{AGENT_PREFIX}{}", self.slug)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.files[0]
    }

    pub fn to_row(&self) -> LocalModelRow {
        LocalModelRow {
            slug: self.slug.clone(),
            repo: self.repo.clone(),
            variant: self.variant.clone(),
            files_json: json(&self.files),
            sha256_json: json(&self.sha256),
            path: self.path().to_string_lossy().into_owned(),
            size_bytes: self.size_bytes,
            metadata_json: self.facts.as_ref().map(json),
            added_by: match self.added_by {
                AddedBy::Download => "download",
                AddedBy::Path => "path",
            }
            .into(),
            settings_json: json(&self.settings),
            created_at: self.created_at,
        }
    }

    pub fn from_row(row: &LocalModelRow) -> Option<LocalModel> {
        let files: Vec<PathBuf> = serde_json::from_str(&row.files_json).ok()?;
        if files.is_empty() {
            return None;
        }
        Some(LocalModel {
            slug: row.slug.clone(),
            repo: row.repo.clone(),
            variant: row.variant.clone(),
            files,
            sha256: serde_json::from_str(&row.sha256_json).unwrap_or_default(),
            size_bytes: row.size_bytes,
            facts: row.metadata_json.as_deref().and_then(|text| serde_json::from_str(text).ok()),
            added_by: if row.added_by == "path" { AddedBy::Path } else { AddedBy::Download },
            settings: serde_json::from_str(&row.settings_json).ok()?,
            created_at: row.created_at,
        })
    }
}

fn json(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("serializable")
}

/// A name usable in an agent name: lowercase letters, digits, `-`, `_` and `.`.
pub fn slug(name: &str, variant: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let base = base.strip_suffix("-GGUF").or_else(|| base.strip_suffix("-gguf")).unwrap_or(base);
    let mut slug = String::new();
    for c in format!("{base}-{variant}").chars().flat_map(char::to_lowercase) {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '_' | '.') { c } else { '-' };
        if !(c == '-' && slug.ends_with('-')) {
            slug.push(c);
        }
    }
    slug.trim_matches('-').to_string()
}

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn slugs() {
        assert_eq!(slug("unsloth/Qwen3-4B-Instruct-2507-GGUF", "Q4_K_M"), "qwen3-4b-instruct-2507-q4_k_m");
        assert_eq!(slug("Răzvan model.gguf", "x"), "r-zvan-model.gguf-x");
    }
}
