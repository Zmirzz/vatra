use std::fs;
use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

/// Folders that are never part of an extension's content.
const LEFT_OUT: [&str; 2] = ["node_modules", ".git"];

/// The SHA-256 of a folder: the relative path and the bytes of every file, in path order. Links are skipped, and so
/// are `node_modules` and `.git`. Two folders with the same files hash the same, wherever they are.
pub fn content_hash(dir: &Path) -> io::Result<String> {
    let mut hasher = Sha256::new();
    for relative in files(dir)? {
        let bytes = fs::read(dir.join(&relative))?;
        feed(&mut hasher, relative.as_bytes());
        feed(&mut hasher, &bytes);
    }
    Ok(hex(&hasher.finalize()))
}

/// The files under `dir`, as paths with `/` separators, sorted.
pub(crate) fn files(dir: &Path) -> io::Result<Vec<String>> {
    let mut found = Vec::new();
    collect(dir, "", &mut found)?;
    found.sort();
    Ok(found)
}

fn collect(dir: &Path, prefix: &str, found: &mut Vec<String>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = entry.file_type()?;
        let relative = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if kind.is_dir() {
            if !LEFT_OUT.contains(&name.as_str()) {
                collect(&entry.path(), &relative, found)?;
            }
        } else if kind.is_file() {
            found.push(relative);
        }
    }
    Ok(())
}

/// Length-prefixed, so a path can't run into the content after it.
fn feed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
