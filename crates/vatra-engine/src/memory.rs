use std::fs;
use std::path::Path;

const INDEX_FILE: &str = "MEMORY.md";
const INDEX_LINES: usize = 200;
const INDEX_BYTES: usize = 25 * 1024;

/// The memory folder as the prompt shows it: where it is, and the index of the notes in it. None when the
/// folder doesn't exist.
pub fn memory_block(dir: &Path) -> Option<String> {
    if !dir.is_dir() {
        return None;
    }
    let index = fs::read_to_string(dir.join(INDEX_FILE)).unwrap_or_default();
    let index = if index.trim().is_empty() { "(MEMORY.md is empty or missing.)".to_string() } else { capped(&index) };
    Some(format!("Memory folder: {}\n\nIndex (MEMORY.md):\n{index}", dir.display()))
}

/// The first 200 lines of the index, and no more than 25 KiB of them. Whatever is left out stays in the folder.
fn capped(index: &str) -> String {
    let mut text = index.lines().take(INDEX_LINES).collect::<Vec<_>>().join("\n");
    let mut cut = index.lines().count() > INDEX_LINES;
    if text.len() > INDEX_BYTES {
        let mut end = INDEX_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        // Cut at a line break, so the last line isn't half written.
        if let Some(newline) = text.rfind('\n') {
            text.truncate(newline);
        }
        cut = true;
    }
    if cut {
        text.push_str("\n(The index is cut at 200 lines or 25 KiB. Keep it short.)");
    }
    text
}
