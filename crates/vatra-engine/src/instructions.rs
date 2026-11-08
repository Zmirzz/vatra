use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use globset::GlobBuilder;
use regex::Regex;

/// Imports nest this deep at most. The file at the top is level 0.
const MAX_IMPORT_DEPTH: usize = 4;
const WARN_OVER_BYTES: usize = 32 * 1024;
static HTML_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());

/// The instructions the project gives the agent, in the order they are read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadedInstructions {
    pub text: String,
    pub warnings: Vec<String>,
    pub files: Vec<PathBuf>,
    /// The rules that apply only to some files. Each joins the context once a file it covers is touched.
    pub deferred: Vec<PathRule>,
}

/// A rule with `paths` in its frontmatter.
#[derive(Clone, Debug, PartialEq)]
pub struct PathRule {
    pub label: String,
    pub patterns: Vec<String>,
    /// The rule as it goes into the context, under its label.
    pub text: String,
}

/// Whether the rule covers a file. The file must be under the project root, and the patterns are relative to it.
pub fn covers(rule: &PathRule, root: &Path, file: &Path) -> bool {
    let Ok(relative) = file.strip_prefix(root) else {
        return false;
    };
    let relative = slashed(relative);
    rule.patterns
        .iter()
        .filter_map(|pattern| GlobBuilder::new(pattern).literal_separator(true).build().ok())
        .any(|glob| glob.compile_matcher().is_match(&relative))
}

/// Loads the instruction files from the project root down to the working folder, and then the project's
/// rules. `extra` files come first. Nothing is loaded when the project root is empty.
pub fn load_instructions(project_root: &Path, cwd: &Path, extra: &[PathBuf]) -> LoadedInstructions {
    let mut loader = Loader::new(project_root);
    if project_root.as_os_str().is_empty() {
        return loader.finish();
    }
    for path in extra {
        loader.top(path);
    }
    for folder in folders(project_root, cwd) {
        for path in folder_files(&folder) {
            loader.top(&path);
        }
    }
    for rule in rule_files(project_root) {
        loader.rule(&rule);
    }
    loader.finish()
}

/// The folders from the project root down to the working folder, the root first.
fn folders(project_root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut folders = vec![project_root.to_path_buf()];
    if let Ok(rest) = cwd.strip_prefix(project_root) {
        let mut folder = project_root.to_path_buf();
        for part in rest.components() {
            folder.push(part);
            folders.push(folder.clone());
        }
    }
    folders
}

/// The instruction files of one folder, in the order they are read. AGENTS.override.md replaces AGENTS.md.
fn folder_files(folder: &Path) -> Vec<PathBuf> {
    let agents = if folder.join("AGENTS.override.md").is_file() { "AGENTS.override.md" } else { "AGENTS.md" };
    [agents, "CLAUDE.md", "CLAUDE.local.md"]
        .into_iter()
        .map(|name| folder.join(name))
        .filter(|path| path.is_file())
        .collect()
}

fn rule_files(project_root: &Path) -> Vec<PathBuf> {
    let mut files = vec![];
    collect_markdown(&project_root.join(".claude").join("rules"), &mut files);
    files.sort();
    files
}

fn collect_markdown(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_markdown(&path, files);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("md") {
            files.push(path);
        }
    }
}

struct Loader {
    /// The project root as given, which the labels and the path patterns are relative to.
    base: PathBuf,
    /// The project root with links resolved, which imports must stay inside.
    root: PathBuf,
    sections: Vec<String>,
    /// The bodies already included, so the same text is only used once.
    bodies: HashSet<String>,
    /// The files already included, so an import cycle stops.
    visited: HashSet<PathBuf>,
    files: Vec<PathBuf>,
    warnings: Vec<String>,
    deferred: Vec<PathRule>,
}

impl Loader {
    fn new(project_root: &Path) -> Loader {
        let root = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
        Loader {
            base: project_root.to_path_buf(),
            root,
            sections: vec![],
            bodies: HashSet::new(),
            visited: HashSet::new(),
            files: vec![],
            warnings: vec![],
            deferred: vec![],
        }
    }

    /// Includes a file that is read as it is, such as an instruction file of a folder.
    fn top(&mut self, path: &Path) {
        if !path.is_file() {
            return;
        }
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        self.include(path, &text);
    }

    /// Includes a rule that applies everywhere. A rule with `paths` is kept back until a file it covers is touched.
    fn rule(&mut self, path: &Path) {
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        let (frontmatter, body) = split_frontmatter(&text);
        let patterns = path_patterns(frontmatter);
        if patterns.is_empty() {
            self.include(path, body);
            return;
        }
        let dir = path.parent().unwrap_or(Path::new(""));
        let body = self.expand(&strip_comments(body), dir, 0);
        let label = self.label(path);
        let text = section(&label, body.trim());
        self.deferred.push(PathRule { label, patterns, text });
    }

    fn include(&mut self, path: &Path, text: &str) {
        let Ok(canonical) = fs::canonicalize(path) else {
            return;
        };
        if !self.visited.insert(canonical) {
            return;
        }
        let dir = path.parent().unwrap_or(Path::new(""));
        let body = self.expand(&strip_comments(text), dir, 0);
        self.files.push(path.to_path_buf());
        self.add(path, &body);
    }

    /// Returns the text with each imported file placed after the line that names it. Code blocks are left alone.
    fn expand(&mut self, text: &str, dir: &Path, depth: usize) -> String {
        let mut out = String::new();
        let mut fenced = false;
        for line in text.lines() {
            out.push_str(line);
            out.push('\n');
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                fenced = !fenced;
                continue;
            }
            if fenced {
                continue;
            }
            for token in line.split_whitespace() {
                let Some(target) = token.strip_prefix('@') else {
                    continue;
                };
                let target = target.trim_end_matches([',', '.', ';', ':', ')', '!', '?']);
                if target.is_empty() {
                    continue;
                }
                if let Some(imported) = self.import(&dir.join(target), depth) {
                    out.push_str(&imported);
                    out.push('\n');
                }
            }
        }
        out
    }

    fn import(&mut self, path: &Path, depth: usize) -> Option<String> {
        let canonical = fs::canonicalize(path).ok().filter(|real| real.is_file())?;
        if !canonical.starts_with(&self.root) {
            self.warnings.push(format!("{} is outside the project, so it wasn't imported.", path.display()));
            return None;
        }
        if depth + 1 > MAX_IMPORT_DEPTH {
            self.warnings.push(format!(
                "{} is nested deeper than {MAX_IMPORT_DEPTH} imports, so it wasn't loaded.",
                path.display()
            ));
            return None;
        }
        if !self.visited.insert(canonical) {
            return None;
        }
        let text = fs::read_to_string(path).ok()?;
        self.files.push(path.to_path_buf());
        let dir = path.parent().unwrap_or(Path::new(""));
        Some(self.expand(&strip_comments(&text), dir, depth + 1))
    }

    fn add(&mut self, path: &Path, body: &str) {
        let body = body.trim();
        if body.is_empty() || !self.bodies.insert(body.to_string()) {
            return;
        }
        let label = self.label(path);
        self.sections.push(section(&label, body));
    }

    fn label(&self, path: &Path) -> String {
        slashed(path.strip_prefix(&self.base).unwrap_or(path))
    }

    fn finish(self) -> LoadedInstructions {
        let mut warnings = self.warnings;
        let text = self.sections.join("\n\n");
        if text.len() > WARN_OVER_BYTES {
            warnings.push(format!(
                "The instructions are {} KiB, over the 32 KiB the model reads well. Shorten them or move parts into \
                 imported files.",
                text.len() / 1024
            ));
        }
        LoadedInstructions { text, warnings, files: self.files, deferred: self.deferred }
    }
}

fn section(label: &str, body: &str) -> String {
    format!("Instructions from {label}:\n\n{body}")
}

fn slashed(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

fn strip_comments(text: &str) -> String {
    HTML_COMMENT.replace_all(text, "").into_owned()
}

/// The frontmatter of a rule, between its leading `---` lines, and the rest of the file.
fn split_frontmatter(text: &str) -> (&str, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return ("", text);
    };
    let Some(end) = rest.find("\n---") else {
        return ("", text);
    };
    let after = &rest[end + "\n---".len()..];
    (&rest[..end], after.strip_prefix('\n').unwrap_or(after))
}

/// The patterns of the `paths` key, written as a list in brackets, as comma-separated values, or as dashed lines.
fn path_patterns(frontmatter: &str) -> Vec<String> {
    let mut patterns = vec![];
    let mut in_list = false;
    for line in frontmatter.lines() {
        if let Some(value) = line.strip_prefix("paths:") {
            in_list = value.trim().is_empty();
            patterns.extend(inline_patterns(value));
        } else if let (true, Some(item)) = (in_list, line.trim_start().strip_prefix("- ")) {
            patterns.push(unquote(item));
        } else {
            in_list = false;
        }
    }
    patterns
}

fn inline_patterns(value: &str) -> Vec<String> {
    let value = value.trim();
    let value = value.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')).unwrap_or(value);
    value.split(',').map(unquote).filter(|pattern| !pattern.is_empty()).collect()
}

fn unquote(text: &str) -> String {
    text.trim().trim_matches(['"', '\'']).to_string()
}
