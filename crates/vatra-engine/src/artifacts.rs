//! Bounded observations of real files. A model mentioning a path never creates an artifact.
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use vatra_core::{MAX_FILE_BYTES, NewFile, Store};

pub type Snapshot = BTreeMap<PathBuf, Vec<u8>>;
pub struct Observation {
    pub files: Snapshot,
    pub incomplete: bool,
}
pub fn read_snapshot(path: &Path) -> Option<Vec<u8>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_FILE_BYTES as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= MAX_FILE_BYTES).then_some(bytes)
}
pub fn observe_workspace(root: &Path) -> Observation {
    let mut observation = Observation {
        files: BTreeMap::new(),
        incomplete: false,
    };
    let mut total = 0;
    let mut entries = 0;
    let start = std::time::Instant::now();
    for entry in ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .follow_links(false)
        .build()
    {
        entries += 1;
        if entries > 5000 || start.elapsed() > std::time::Duration::from_secs(2) {
            observation.incomplete = true;
            break;
        }
        let Ok(entry) = entry else {
            observation.incomplete = true;
            continue;
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if entry.path().components().any(|c| {
            ["node_modules", "target", ".git", ".vatra-session"]
                .contains(&c.as_os_str().to_str().unwrap_or(""))
        }) {
            continue;
        }
        let Some(bytes) = read_snapshot(entry.path()) else {
            observation.incomplete = true;
            continue;
        };
        total += bytes.len();
        if total > 100 * 1024 * 1024 {
            observation.incomplete = true;
            break;
        }
        observation.files.insert(entry.path().to_path_buf(), bytes);
    }
    observation
}
pub fn capture(
    store: &Store,
    session: i64,
    path: &Path,
    before: Option<&[u8]>,
    after: &[u8],
    operation: &str,
) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Invalid artifact path.")?;
    let ext = path
        .extension()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let media = match ext.as_str() {
        "pdf" if after.starts_with(b"%PDF-") => "application/pdf",
        "png" if after.starts_with(b"\x89PNG\r\n\x1a\n") => "image/png",
        "jpg" | "jpeg" if after.starts_with(&[0xff, 0xd8, 0xff]) => "image/jpeg",
        "webp" if after.starts_with(b"RIFF") && after.get(8..12) == Some(b"WEBP") => "image/webp",
        "docx" if after.starts_with(b"PK\x03\x04") => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }
        "xlsx" if after.starts_with(b"PK\x03\x04") => {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        }
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        _ if std::str::from_utf8(after).is_ok() => "text/plain",
        _ => "application/octet-stream",
    };
    let text = if media.starts_with("text/") || media == "application/json" {
        std::str::from_utf8(after).unwrap_or("").to_string()
    } else {
        String::new()
    };
    store
        .capture_file(
            session,
            &NewFile {
                name: name.into(),
                media_type: media.into(),
                origin: if before.is_some() {
                    "modified"
                } else {
                    "created"
                }
                .into(),
                source_path: Some(path.to_string_lossy().into_owned()),
                operation: Some(operation.into()),
                text,
            },
            before,
            after,
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
}
pub fn reconcile(
    store: &Store,
    session: i64,
    before: &Observation,
    after: &Observation,
    operation: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    for (path, bytes) in &after.files {
        let prior = before.files.get(path).map(Vec::as_slice);
        if prior == Some(bytes.as_slice()) {
            continue;
        }
        if let Err(e) = capture(store, session, path, prior, bytes, operation) {
            errors.push(e);
        }
    }
    if before.incomplete || after.incomplete {
        errors.push("File discovery reached its time, count or size limit. The catalog may not include every changed file.".into());
    }
    errors
}
