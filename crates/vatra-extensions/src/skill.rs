use std::fs;
use std::path::{Path, PathBuf};

use serde_yaml_ng::{Mapping, Value};

use crate::frontmatter::{self, Document};
use crate::{FormatError, canonical_tools};

/// Longer descriptions are cut to this many characters, the way the skill listing shows them.
const DESCRIPTION_LIMIT: usize = 1536;

/// Strict Agent Skills validation for new installations; legacy imports remain readable.
/// Uses the same YAML parser as the runtime, without executing supporting scripts.
pub fn validate_skill_standard(dir: &Path) -> Result<Vec<String>, FormatError> {
    let file = dir.join("SKILL.md");
    let document = frontmatter::split(&read(&file)?, &file)?;
    let mut errors = Vec::new();
    let meta = document.meta.unwrap_or_default();
    let name = meta.get(Value::String("name".into())).and_then(Value::as_str).unwrap_or_default();
    if name.is_empty() || name.len() > 64 || name.starts_with('-') || name.ends_with('-')
        || name.contains("--") || !name.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-') {
        errors.push("The name must use 1–64 lowercase letters, digits and single hyphens.".into());
    }
    if dir.file_name().is_none_or(|folder| folder != name) {
        errors.push("The name must match its skill directory.".into());
    }
    let description = meta.get(Value::String("description".into())).and_then(Value::as_str).unwrap_or_default();
    if description.trim().is_empty() || description.chars().count() > 1024 {
        errors.push("The description must contain 1–1024 characters.".into());
    }
    if let Some(value) = meta.get(Value::String("compatibility".into()))
        && value.as_str().is_none_or(|value| value.chars().count() > 500) {
        errors.push("Compatibility must be text of at most 500 characters.".into());
    }
    if let Some(value) = meta.get(Value::String("metadata".into()))
        && value.as_mapping().is_none_or(|mapping| mapping.iter().any(|(key,value)| !key.is_string() || !value.is_string())) {
        errors.push("Metadata must map text keys to text values.".into());
    }
    Ok(errors)
}

/// A skill folder, or a command file, read into the fields the engine needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub argument_hint: Option<String>,
    pub arguments: Vec<String>,
    pub disable_model_invocation: bool,
    pub user_invocable: bool,
    /// False when the model may not call the skill by itself: either `disable-model-invocation` is set, or
    /// the Codex `agents/openai.yaml` policy turns implicit invocation off.
    pub allow_implicit: bool,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub context_fork: bool,
    pub agent: Option<String>,
    pub paths: Vec<String>,
    pub shell: Option<String>,
    pub body: String,
    pub dir: PathBuf,
    pub plugin: Option<String>,
    /// The stored copy of the plugin this skill belongs to, and the folder for the plugin's data. Both are set when
    /// the catalog reads the skill, and are None for a standalone skill.
    pub plugin_root: Option<PathBuf>,
    pub plugin_data: Option<PathBuf>,
    pub is_command: bool,
    /// Frontmatter keys this parser does not know. They are kept, not dropped.
    pub unknown_fields: Vec<String>,
    /// Values that could not be read. Each one fell back to its default, and the skill still loads.
    pub problems: Vec<String>,
}

impl Skill {
    /// The name the user types: the plugin name, then the skill or command name.
    pub fn qualified_name(&self) -> String {
        match &self.plugin {
            Some(plugin) => format!("{plugin}:{}", self.name),
            None => self.name.clone(),
        }
    }

    fn new(dir: PathBuf, name: String, body: String, is_command: bool) -> Skill {
        Skill {
            name,
            description: String::new(),
            when_to_use: None,
            argument_hint: None,
            arguments: Vec::new(),
            disable_model_invocation: false,
            user_invocable: true,
            allow_implicit: true,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            model: None,
            effort: None,
            context_fork: false,
            agent: None,
            paths: Vec::new(),
            shell: None,
            body,
            dir,
            plugin: None,
            plugin_root: None,
            plugin_data: None,
            is_command,
            unknown_fields: Vec::new(),
            problems: Vec::new(),
        }
    }
}

/// Reads the `SKILL.md` in a skill folder, with the Codex policy from `agents/openai.yaml` if there is one.
pub fn parse_skill(dir: &Path) -> Result<Skill, FormatError> {
    let file = dir.join("SKILL.md");
    let text = read(&file)?;
    let document = frontmatter::split(&text, &file)?;
    let name = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut skill = Skill::new(dir.to_path_buf(), name, document.body, false);
    apply(&mut skill, document.meta, &file, false)?;
    let implicit = implicit_allowed(dir, &mut skill.problems);
    skill.allow_implicit = implicit && !skill.disable_model_invocation;
    Ok(skill)
}

/// Reads a command file. `namespace` is the folders between the commands root and the file, so
/// `frontend/deploy.md` under the root has the namespace `["frontend"]` and the name `frontend:deploy`.
pub fn parse_command(file: &Path, namespace: &[String]) -> Result<Skill, FormatError> {
    let text = read(file)?;
    let Document { meta, body } = frontmatter::split(&text, file)?;
    let stem = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = namespace
        .iter()
        .map(String::as_str)
        .chain([stem.as_str()])
        .collect::<Vec<_>>()
        .join(":");
    let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut skill = Skill::new(dir, name, body, true);
    apply(&mut skill, meta, file, true)?;
    skill.allow_implicit = !skill.disable_model_invocation;
    Ok(skill)
}

fn read(file: &Path) -> Result<String, FormatError> {
    fs::read_to_string(file).map_err(|source| FormatError::Read {
        path: file.to_path_buf(),
        source,
    })
}

/// Sets the skill's fields from its frontmatter. A value of the wrong kind is left at its default and noted
/// in `problems`. A command has no `name` or `paths` of its own, so those keys are kept as unknown fields.
fn apply(
    skill: &mut Skill,
    meta: Option<Mapping>,
    file: &Path,
    command: bool,
) -> Result<(), FormatError> {
    let Some(meta) = meta else {
        return Ok(());
    };
    for (key, value) in meta {
        let Value::String(key) = key else {
            skill
                .problems
                .push(field_error(file, "frontmatter keys", "must be text").to_string());
            continue;
        };
        match set(skill, &key, value, file, command) {
            Ok(()) => {}
            Err(error @ FormatError::Field { .. }) => skill.problems.push(error.to_string()),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn set(
    skill: &mut Skill,
    key: &str,
    value: Value,
    file: &Path,
    command: bool,
) -> Result<(), FormatError> {
    match key {
        "name" if !command => skill.name = text(file, key, value)?,
        "description" => {
            skill.description = truncate(joined(file, key, value)?, DESCRIPTION_LIMIT);
        }
        "when_to_use" => skill.when_to_use = Some(text(file, key, value)?),
        "argument-hint" => skill.argument_hint = Some(joined(file, key, value)?),
        "arguments" => skill.arguments = list(file, key, value)?,
        "disable-model-invocation" => skill.disable_model_invocation = flag(file, key, value)?,
        "user-invocable" => skill.user_invocable = flag(file, key, value)?,
        "allowed-tools" => {
            skill.allowed_tools = list(file, key, value)?.iter().flat_map(|entry| tool_entries(entry)).collect()
        }
        "disallowed-tools" => {
            skill.disallowed_tools = list(file, key, value)?.iter().flat_map(|entry| tool_entries(entry)).collect()
        }
        "model" => skill.model = Some(text(file, key, value)?),
        "effort" => skill.effort = Some(text(file, key, value)?),
        "context" => skill.context_fork = context_fork(file, key, value)?,
        "agent" => skill.agent = Some(text(file, key, value)?),
        "paths" if !command => skill.paths = comma_list(file, key, value)?,
        "shell" => skill.shell = Some(text(file, key, value)?),
        _ => skill.unknown_fields.push(key.to_string()),
    }
    Ok(())
}

/// The Codex policy from `agents/openai.yaml`. A missing file or key allows implicit use. A file that cannot
/// be read or has a bad value is noted in `problems`, and implicit use is allowed.
fn implicit_allowed(dir: &Path, problems: &mut Vec<String>) -> bool {
    match implicit_policy(dir) {
        Ok(allowed) => allowed,
        Err(error) => {
            problems.push(error.to_string());
            true
        }
    }
}

fn implicit_policy(dir: &Path) -> Result<bool, FormatError> {
    let file = dir.join("agents").join("openai.yaml");
    let text = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(source) => return Err(FormatError::Read { path: file, source }),
    };
    let value: Value = serde_yaml_ng::from_str(&text).map_err(|source| FormatError::Yaml {
        path: file.clone(),
        source,
    })?;
    match value
        .get("policy")
        .and_then(|policy| policy.get("allow_implicit_invocation"))
    {
        None => Ok(true),
        Some(value) => boolean(value).ok_or_else(|| {
            field_error(
                &file,
                "policy.allow_implicit_invocation",
                "must be true or false",
            )
        }),
    }
}

/// A boolean, or the text `true` or `false`.
fn boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::String(text) if text == "true" => Some(true),
        Value::String(text) if text == "false" => Some(false),
        _ => None,
    }
}

fn text(file: &Path, key: &str, value: Value) -> Result<String, FormatError> {
    match value {
        Value::String(text) => Ok(text),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        _ => Err(field_error(file, key, "must be text")),
    }
}

/// Text, or a YAML sequence rejoined as `[a, b]`. An unquoted `argument-hint: [issue-number]` is a sequence.
fn joined(file: &Path, key: &str, value: Value) -> Result<String, FormatError> {
    match value {
        Value::Sequence(items) => {
            let items = items
                .into_iter()
                .map(|item| text(file, key, item))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!("[{}]", items.join(", ")))
        }
        other => text(file, key, other),
    }
}

fn flag(file: &Path, key: &str, value: Value) -> Result<bool, FormatError> {
    boolean(&value).ok_or_else(|| field_error(file, key, "must be true or false"))
}

fn context_fork(file: &Path, key: &str, value: Value) -> Result<bool, FormatError> {
    match text(file, key, value)?.as_str() {
        "fork" => Ok(true),
        other => Err(field_error(
            file,
            key,
            &format!("unknown value `{other}`, expected `fork`"),
        )),
    }
}

/// A list written as a YAML sequence, or as one string such as `Read, Grep Bash(git add *)`.
fn list(file: &Path, key: &str, value: Value) -> Result<Vec<String>, FormatError> {
    match value {
        Value::String(text) => Ok(split_list(&text)),
        Value::Sequence(items) => items
            .into_iter()
            .map(|item| text(file, key, item))
            .collect(),
        Value::Null => Ok(Vec::new()),
        _ => Err(field_error(file, key, "must be a list or text")),
    }
}

/// A list written as one string split on commas only, such as `docs/**/*.md, CHANGELOG.md`. Spaces stay in
/// an item.
fn comma_list(file: &Path, key: &str, value: Value) -> Result<Vec<String>, FormatError> {
    match value {
        Value::String(text) => Ok(text
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect()),
        other => list(file, key, other),
    }
}

/// The tool entries a name stands for, with the Vatra names: `shell(git status)` is `Bash(git status)`, and
/// `apply_patch` is both `Edit` and `Write`.
fn tool_entries(entry: &str) -> Vec<String> {
    match entry.split_once('(') {
        Some((name, rest)) => canonical_tools(name.trim()).into_iter().map(|tool| format!("{tool}({rest}")).collect(),
        None => canonical_tools(entry.trim()).into_iter().map(str::to_string).collect(),
    }
}

/// Splits on commas and spaces, except inside parentheses, so `Bash(git add *)` stays one item.
fn split_list(text: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            c if depth == 0 && (c == ',' || c.is_whitespace()) => {
                if !current.is_empty() {
                    items.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        items.push(current);
    }
    items
}

fn truncate(text: String, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => text[..end].to_string(),
        None => text,
    }
}

fn field_error(file: &Path, field: &str, message: &str) -> FormatError {
    FormatError::Field {
        path: file.to_path_buf(),
        field: field.to_string(),
        message: message.to_string(),
    }
}
