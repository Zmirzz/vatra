use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use vatra_core::{
    DocumentFields, DocumentFormat, Item, MAX_DOCUMENT_BYTES, NewItem, Store, StoreError, WorkspaceError,
    safe_csv_export, validate_document, validate_workspace_content,
};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentExportMode {
    #[default]
    Safe,
    Exact,
}

fn io(error: std::io::Error) -> WorkspaceError {
    WorkspaceError::Store(StoreError::Io(error.to_string()))
}
fn validate_path(path: &Path, format: DocumentFormat) -> Result<(), WorkspaceError> {
    if !path.is_absolute()
        || path.to_string_lossy().replace('/', "\\").starts_with("\\\\")
        || path.to_string_lossy().contains('\0')
    {
        return Err(WorkspaceError::Invalid(
            "Choose an absolute local document path without a network or extended-path prefix.".into(),
        ));
    }
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let expected = match format {
        DocumentFormat::Text => "txt",
        DocumentFormat::Markdown => "md",
        DocumentFormat::Html => "html",
        DocumentFormat::Csv => "csv",
    };
    if extension != expected {
        return Err(WorkspaceError::Invalid(format!(
            "Choose a .{expected} file for this document format."
        )));
    }
    Ok(())
}

pub fn import_document(
    store: &Store,
    project: i64,
    path: &Path,
    format: DocumentFormat,
    now: i64,
) -> Result<Item, WorkspaceError> {
    validate_path(path, format)?;
    if !store.list_projects()?.iter().any(|p| p.id == project) {
        return Err(WorkspaceError::NotFound);
    }
    let file = File::open(path).map_err(io)?;
    if !file.metadata().map_err(io)?.is_file() {
        return Err(WorkspaceError::Invalid("Choose a document file.".into()));
    }
    let mut bytes = Vec::new();
    file.take((MAX_DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(WorkspaceError::Invalid("The document is larger than 2 MiB.".into()));
    }
    let body = String::from_utf8(bytes)
        .map_err(|_| WorkspaceError::Invalid("The document must contain UTF-8 text.".into()))?;
    validate_document(format, &body)?;
    let title: String = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Document")
        .chars()
        .take(240)
        .collect();
    let fields = json!({"format":format});
    validate_workspace_content("document", &title, &body, &fields)?;
    let id = store.create_item(
        &NewItem {
            kind: "document".into(),
            title,
            body,
            fields,
            project_id: Some(project),
        },
        now,
    )?;
    store.workspace_item(project, id, false)
}

pub fn export_document(
    store: &Store,
    project: i64,
    id: i64,
    expected_revision: u64,
    path: &Path,
) -> Result<(), WorkspaceError> {
    export_document_with_mode(store, project, id, expected_revision, path, DocumentExportMode::Safe)
}
pub fn export_document_with_mode(
    store: &Store,
    project: i64,
    id: i64,
    expected_revision: u64,
    path: &Path,
    mode: DocumentExportMode,
) -> Result<(), WorkspaceError> {
    let item = store.workspace_item(project, id, false)?;
    if item.revision != expected_revision {
        return Err(WorkspaceError::Conflict(Box::new(item)));
    }
    if !matches!(item.kind.as_str(), "document" | "note") {
        return Err(WorkspaceError::Invalid(
            "Only documents and notes can be exported as files.".into(),
        ));
    }
    let fields: DocumentFields = serde_json::from_value(item.fields.clone())
        .map_err(|_| WorkspaceError::Invalid("The document format is invalid.".into()))?;
    validate_path(path, fields.format)?;
    if path.symlink_metadata().is_ok() {
        return Err(WorkspaceError::Invalid(
            "A file already exists here. Choose another name.".into(),
        ));
    }
    validate_document(fields.format, &item.body)?;
    let body = if fields.format == DocumentFormat::Csv && matches!(mode, DocumentExportMode::Safe) {
        safe_csv_export(&item.body)?
    } else {
        item.body
    };
    let parent = path
        .parent()
        .ok_or_else(|| WorkspaceError::Invalid("Choose a folder for the exported file.".into()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(io)?;
    temporary.write_all(body.as_bytes()).map_err(io)?;
    temporary.flush().map_err(io)?;
    temporary.as_file().sync_all().map_err(io)?;
    publish_document(temporary.path(), path)
}

fn publish_document(temporary: &Path, destination: &Path) -> Result<(), WorkspaceError> {
    std::fs::hard_link(temporary, destination).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_export_never_overwrites_a_concurrent_destination() {
        let directory = tempfile::tempdir().unwrap();
        let completed = directory.path().join("complete.tmp");
        let destination = directory.path().join("export.md");
        std::fs::write(&completed, "new content").unwrap();
        std::fs::write(&destination, "concurrent winner").unwrap();
        assert!(publish_document(&completed, &destination).is_err());
        assert_eq!(std::fs::read_to_string(destination).unwrap(), "concurrent winner");
    }
}
