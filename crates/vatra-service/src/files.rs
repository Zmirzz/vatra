//! Bounded file transport. Extraction is local in the desktop worker; persisted text is untrusted user material.
use crate::Service;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{io::Write, path::Path, sync::Arc};
use vatra_core::{FILE_CHUNK_BYTES, NewFile, Store};

#[derive(Clone, Debug, Serialize)]
pub struct InputCapabilities {
    pub images: String,
    pub document_text: bool,
    pub context_tokens: u64,
    pub estimated: bool,
    #[serde(skip)]
    pub local_probe: Option<String>,
}
impl Default for InputCapabilities {
    fn default() -> Self {
        Self {
            images: "unknown".into(),
            document_text: true,
            context_tokens: 8192,
            estimated: true,
            local_probe: None,
        }
    }
}
pub async fn capabilities(
    service: &Service,
    session: i64,
    model: Option<&str>,
) -> Result<InputCapabilities, String> {
    let name = service
        .store()
        .session_agent(session)
        .map_err(|e| e.to_string())?;
    let mut caps = service.agents().input_capabilities(&name, model);
    // Read-only localhost probe. Never starts a model or contacts a cloud provider.
    if let Some(base) = caps.local_probe.as_deref() {
        let root = base
            .trim_end_matches('/')
            .strip_suffix("/v1")
            .unwrap_or(base.trim_end_matches('/'));
        if let Ok(response) = reqwest::Client::new()
            .get(format!("{root}/props"))
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
            && response.status().is_success()
            && let Ok(props) = response.json::<Value>().await
        {
            if let Some(vision) = props["modalities"]["vision"].as_bool() {
                caps.images = if vision { "supported" } else { "unsupported" }.into();
            }
            if let Some(tokens) = props["default_generation_settings"]["n_ctx"]
                .as_u64()
                .or(props["n_ctx"].as_u64())
            {
                caps.context_tokens = tokens;
                caps.estimated = false;
            }
        }
    }
    Ok(caps)
}
#[derive(Clone, Debug, Deserialize, Default)]
pub struct FileSelection {
    pub id: i64,
    #[serde(default)]
    pub start: Option<usize>,
    #[serde(default)]
    pub end: Option<usize>,
}
pub fn prepare_inputs(
    store: &Store,
    session: i64,
    selections: &[FileSelection],
    caps: &InputCapabilities,
) -> Result<Vec<vatra_core::TurnFile>, String> {
    if selections.len() > 20 {
        return Err("Attach no more than 20 files to one message.".into());
    }
    let budget = 16_000.min(caps.context_tokens.saturating_sub(4096) / 4) as usize;
    let mut used = 0;
    let mut size = 0;
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for selection in selections {
        if !seen.insert(selection.id) {
            return Err("The same attachment was included twice.".into());
        }
        let f = store
            .file(session, selection.id)
            .map_err(|e| e.to_string())?;
        size += f.size;
        if size > 100 * 1024 * 1024 {
            return Err("Attachments exceed 100 MiB in one message.".into());
        }
        let mut text = store
            .file_text(session, f.id, None)
            .map_err(|e| e.to_string())?;
        if selection.start.is_some() || selection.end.is_some() {
            let chars: Vec<char> = text.chars().collect();
            let start = selection.start.unwrap_or(0);
            let end = selection.end.unwrap_or(chars.len());
            if start >= end || end > chars.len() {
                return Err("Choose a valid document section.".into());
            }
            text = format!(
                "[Selected characters {}–{} of {}]\n{}",
                start + 1,
                end,
                chars.len(),
                chars[start..end].iter().collect::<String>()
            );
        }
        used += text.len().div_ceil(3);
        if used > budget {
            return Err(format!(
                "Documents need about {used} tokens; the attachment budget is {budget}. Select a smaller section."
            ));
        }
        let image_base64 = if f.media_type.starts_with("image/") {
            if caps.images != "supported" {
                return Err("This route has no confirmed image input support. Choose a compatible vision model; your attachments are preserved.".into());
            }
            let mut bytes = Vec::with_capacity(f.size);
            for offset in (0..f.size).step_by(FILE_CHUNK_BYTES) {
                bytes.extend(
                    store
                        .file_bytes(session, f.id, None, offset, FILE_CHUNK_BYTES)
                        .map_err(|e| e.to_string())?,
                );
            }
            Some(STANDARD.encode(bytes))
        } else {
            if text.trim().is_empty() {
                return Err(format!(
                    "{} has no extractable document text. It can be viewed, but cannot be sent as readable context.",
                    f.name
                ));
            }
            None
        };
        result.push(vatra_core::TurnFile {
            id: f.id,
            revision: f.revision,
            name: f.name,
            media_type: f.media_type,
            text,
            image_base64,
            history_index: None,
        });
    }
    Ok(result)
}

#[derive(Deserialize)]
struct Input {
    session_id: i64,
    #[serde(default)]
    id: i64,
    #[serde(default)]
    revision: Option<i64>,
    #[serde(default)]
    expected_revision: i64,
    #[serde(default)]
    upload_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    base64: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    query: String,
    #[serde(default)]
    size: usize,
    #[serde(default)]
    offset: usize,
    #[serde(default = "chunk_size")]
    length: usize,
    #[serde(default)]
    message_seq: Option<i64>,
    #[serde(default)]
    model: Option<String>,
}
fn chunk_size() -> usize {
    FILE_CHUNK_BYTES
}

/// Type comes from both content and extension. Unknown binaries stay downloadable but are never claimed as extracted.
pub fn media_type(name: &str, bytes: &[u8]) -> Result<&'static str, String> {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let valid = match ext.as_str() {
        "pdf" if bytes.starts_with(b"%PDF-") => "application/pdf",
        "png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => "image/png",
        "jpg" | "jpeg" if bytes.starts_with(&[0xff, 0xd8, 0xff]) => "image/jpeg",
        "webp" if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") => "image/webp",
        "docx" if bytes.starts_with(b"PK\x03\x04") => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }
        "xlsx" if bytes.starts_with(b"PK\x03\x04") => {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        }
        "pdf" | "png" | "jpg" | "jpeg" | "webp" | "docx" | "xlsx" => {
            return Err("File contents do not match its extension.".into());
        }
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "txt" | "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "css" | "toml" | "yaml" | "yml"
        | "xml" | "sql" | "log" | "sh" | "ps1" => "text/plain",
        _ => "application/octet-stream",
    };
    if (valid.starts_with("text/") || valid == "application/json")
        && std::str::from_utf8(bytes).is_err()
    {
        return Err("Text files must contain valid UTF-8.".into());
    }
    if valid == "application/json" {
        serde_json::from_slice::<Value>(bytes).map_err(|e| format!("Invalid JSON: {e}"))?;
    }
    Ok(valid)
}

pub async fn handle(service: &Arc<Service>, method: &str, params: Value) -> Result<Value, String> {
    let mut input: Input = serde_json::from_value(params).map_err(|e| e.to_string())?;
    if method == "files.capabilities" {
        return Ok(json!(
            capabilities(service, input.session_id, input.model.as_deref()).await?
        ));
    }
    if method == "files.materialize" {
        let file = service
            .store()
            .file(input.session_id, input.id)
            .map_err(|e| e.to_string())?;
        let root = service
            .agents()
            .file_directory(input.session_id)
            .ok_or("Managed file directory unavailable.")?;
        let folder = root.join(format!(
            "{}-{}-{}",
            file.id,
            input.revision.unwrap_or(file.revision),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        input.path = folder.join(file.name).to_string_lossy().into_owned();
    }
    let store = Arc::clone(service.store());
    let method = method.to_string();
    tokio::task::spawn_blocking(move || handle_store(&store, &method, input))
        .await
        .map_err(|e| e.to_string())?
}
fn handle_store(store: &Store, method: &str, p: Input) -> Result<Value, String> {
    store
        .session_agent(p.session_id)
        .map_err(|_| "Conversation not found.".to_string())?;
    let action = || -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Ok(match method {
            "files.upload.begin" => {
                let id = uuid::Uuid::new_v4().to_string();
                store.begin_file_upload(p.session_id, &id, p.size)?;
                json!({"upload_id":id})
            }
            "files.upload.chunk" => {
                if p.base64.len() > FILE_CHUNK_BYTES.div_ceil(3) * 4 {
                    return Err("Upload chunk too large.".into());
                }
                store.append_file_upload(
                    p.session_id,
                    &p.upload_id,
                    p.offset,
                    &STANDARD.decode(&p.base64)?,
                )?;
                json!({})
            }
            "files.upload.cancel" => {
                store.cancel_file_upload(p.session_id, &p.upload_id)?;
                json!({})
            }
            "files.upload.finish" => {
                let bytes = store.read_file_upload(p.session_id, &p.upload_id)?;
                let media = media_type(&p.name, &bytes)?;
                let text = if media.starts_with("image/") || media == "application/octet-stream" {
                    String::new()
                } else if media.starts_with("text/") && media != "text/html"
                    || media == "application/json"
                {
                    String::from_utf8(bytes.clone())?
                } else {
                    p.text
                };
                let file = store.import_file(
                    p.session_id,
                    &NewFile {
                        name: p.name,
                        media_type: media.into(),
                        origin: "attached".into(),
                        source_path: None,
                        operation: None,
                        text,
                    },
                    &bytes,
                )?;
                store.cancel_file_upload(p.session_id, &p.upload_id)?;
                json!(file)
            }
            "files.list" => json!(store.list_files(p.session_id)?),
            "files.search" => json!(store.search_files(p.session_id, &p.query)?),
            "files.get" => json!(store.file(p.session_id, p.id)?),
            "files.read" => {
                json!({"base64":STANDARD.encode(store.file_bytes(p.session_id,p.id,p.revision,p.offset,p.length)?)})
            }
            "files.text" => json!({"text":store.file_text(p.session_id,p.id,p.revision)?}),
            "files.prepare_text" => {
                store.prepare_file_text(
                    p.session_id,
                    p.id,
                    p.revision.ok_or("Version is required.")?,
                    &p.text,
                )?;
                json!({})
            }
            "files.versions" => json!(store.file_versions(p.session_id, p.id)?),
            "files.message" => json!(
                store.message_files(p.session_id, p.message_seq.ok_or("Message is required.")?)?
            ),
            "files.links" => json!(store.all_message_files(p.session_id)?),
            "files.rename" => {
                store.rename_file(p.session_id, p.id, &p.name)?;
                json!({})
            }
            "files.remove" => {
                store.remove_file(p.session_id, p.id)?;
                json!({})
            }
            "files.save" => {
                let file = store.file(p.session_id, p.id)?;
                if !(file.media_type.starts_with("text/") || file.media_type == "application/json")
                {
                    return Err("Only text files can be edited here.".into());
                }
                media_type(&file.name, p.text.as_bytes())?;
                json!(store.revise_file(
                    p.session_id,
                    p.id,
                    p.expected_revision,
                    &p.text,
                    p.text.as_bytes(),
                    "user.save"
                )?)
            }
            "files.export" | "files.materialize" => {
                let path = Path::new(&p.path);
                if !path.is_absolute()
                    || p.path.replace('/', "\\").starts_with("\\\\")
                    || p.path.contains('\0')
                {
                    return Err("Choose an absolute local file path.".into());
                }
                let file = store.file(p.session_id, p.id)?;
                let version = store
                    .file_versions(p.session_id, p.id)?
                    .into_iter()
                    .find(|v| p.revision.unwrap_or(file.revision) == v.revision)
                    .ok_or("Version not found.")?;
                let parent = path.parent().ok_or("Choose a destination folder.")?;
                let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
                for offset in (0..version.size).step_by(FILE_CHUNK_BYTES) {
                    tmp.write_all(&store.file_bytes(
                        p.session_id,
                        p.id,
                        Some(version.revision),
                        offset,
                        FILE_CHUNK_BYTES,
                    )?)?;
                }
                tmp.flush()?;
                tmp.as_file().sync_all()?;
                std::fs::hard_link(tmp.path(), path)?;
                json!({"path":p.path})
            }
            _ => return Err("Unknown file operation.".into()),
        })
    };
    action().map_err(|e| e.to_string())
}
