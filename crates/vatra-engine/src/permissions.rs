use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde_json::Value;
use vatra_core::{PermissionMode, Rule, Rules};

use crate::tools::ToolContext;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolClass {
    Free,
    ReadOutside,
    Web,
    EditInside,
    EditOutside,
    Execute,
    Mcp,
    /// An action with an effect outside the computer, such as sending a message or deleting remote data.
    /// It asks the person in every mode, and allow rules and grants never skip the question.
    External,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Ask,
    Deny(String),
}

pub struct PermissionPolicy {
    mode: PermissionMode,
    rules: Rules,
    turn_grants: Mutex<Vec<Rule>>,
    turn_denies: Mutex<Vec<Rule>>,
}

const WRAPPERS: [&str; 8] = ["timeout", "time", "nice", "nohup", "stdbuf", "command", "builtin", "noglob"];
const EDIT_TOOLS: [&str; 3] = ["Edit", "Write", "NotebookEdit"];
const PATH_ARGUMENTS: [&str; 3] = ["file_path", "path", "notebook_path"];
/// Variables that can't change what a command runs. Any other assignment in front of a command
/// keeps allow rules from matching it.
const SAFE_VARIABLES: [&str; 5] = ["LANG", "TZ", "NO_COLOR", "TERM", "CI"];
/// How many external actions on one connector the person may approve in an hour.
pub const EXTERNAL_LIMIT_PER_HOUR: u32 = 20;

impl PermissionPolicy {
    pub fn new(mode: PermissionMode, rules: Rules) -> PermissionPolicy {
        PermissionPolicy { mode, rules, turn_grants: Mutex::new(vec![]), turn_denies: Mutex::new(vec![]) }
    }

    /// Allows the rule's calls until the turn ends. A grant never overrides a deny rule.
    pub fn grant_for_turn(&self, rule: Rule) {
        lock(&self.turn_grants).push(rule);
    }

    /// Denies the rule's calls until the turn ends, as a deny rule would. A skill's disallowed tools use it.
    pub fn deny_for_turn(&self, rule: Rule) {
        lock(&self.turn_denies).push(rule);
    }

    /// Whether an ask rule, rather than the mode, asks for this call. Such an answer is not remembered.
    pub fn asked_by_rule(&self, name: &str, arguments: &Value, context: &ToolContext) -> bool {
        let parts = Parts::of(name, arguments);
        let any = |rule: &Rule| parts.any(|part| rule_matches(rule, name, arguments, part, context));
        self.rules.ask.iter().any(&any)
    }

    /// Drops the grants and the turn denies, so a new turn starts with the configured rules alone.
    pub fn clear_turn_grants(&self) {
        lock(&self.turn_grants).clear();
        lock(&self.turn_denies).clear();
    }

    /// Deny rules win over ask rules, which win over allow rules and grants. Without a rule
    /// the mode decides.
    pub fn decide(&self, name: &str, class: ToolClass, arguments: &Value, context: &ToolContext) -> Decision {
        let parts = Parts::of(name, arguments);
        let any = |rule: &Rule| parts.any(|part| rule_matches(rule, name, arguments, part, context));
        let turn_denies = lock(&self.turn_denies);
        let denies = || self.rules.deny.iter().chain(turn_denies.iter());
        if class == ToolClass::External {
            // Only a deny rule stops an external action before the question. Every mode, allow rule and grant asks.
            if denies().any(&any) {
                return Decision::Deny(format!("A rule denies this call to {name}."));
            }
            return Decision::Ask;
        }
        if let Parts::Unparseable(raw) = &parts {
            // Only the raw text is known. A deny rule's command prefix anywhere in it is enough to deny,
            // and any Bash deny or ask rule makes it ask, in every mode.
            let by_prefix = |rule: &Rule| rule.tool == "Bash" && denies_by_prefix(rule, raw);
            if denies().any(&any) || denies().any(by_prefix) {
                return Decision::Deny(format!("A rule denies this call to {name}."));
            }
            let bash_rule = |rule: &Rule| rule.tool == "Bash";
            if self.rules.deny.iter().chain(&self.rules.ask).any(bash_rule) {
                return Decision::Ask;
            }
            return mode_decision(self.mode, class);
        }
        if denies().any(&any) {
            return Decision::Deny(format!("A rule denies this call to {name}."));
        }
        if self.rules.ask.iter().any(&any) {
            return Decision::Ask;
        }
        let all = |rule: &Rule| parts.all(|part| rule_matches(rule, name, arguments, part, context));
        let grants = lock(&self.turn_grants);
        if self.rules.allow.iter().any(&all) || grants.iter().any(&all) {
            return Decision::Allow;
        }
        mode_decision(self.mode, class)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn mode_decision(mode: PermissionMode, class: ToolClass) -> Decision {
    match (class, mode) {
        (ToolClass::External, _) => Decision::Ask,
        (ToolClass::Free, _) | (_, PermissionMode::FullAccess) => Decision::Allow,
        (ToolClass::EditInside, PermissionMode::AcceptEdits) => Decision::Allow,
        _ => Decision::Ask,
    }
}

/// One part of a Bash command, with the facts that allow rules must not ignore.
struct Command {
    /// The command with wrappers and assignments removed.
    text: String,
    /// An unquoted `>` or `<` that isn't a plain fd duplicate such as `2>&1`.
    redirects: bool,
    /// An assignment in front of the command that isn't one of the safe variables.
    unsafe_prefix: bool,
}

/// What a call is checked against. A Bash command is split into its parts, and each part must pass.
enum Parts {
    Single,
    Commands(Vec<Command>),
    /// Shell substitutions and `$'…'` can't be split, so only the raw text is known.
    Unparseable(String),
}

impl Parts {
    fn of(name: &str, arguments: &Value) -> Parts {
        if name != "Bash" {
            return Parts::Single;
        }
        let command = arguments.get("command").and_then(Value::as_str).unwrap_or_default();
        match split_commands(command) {
            Some(parts) => Parts::Commands(parts),
            None => Parts::Unparseable(command.to_string()),
        }
    }

    /// True when some part matches.
    fn any(&self, matches: impl Fn(Option<&Command>) -> bool) -> bool {
        match self {
            Parts::Single => matches(None),
            Parts::Commands(parts) => parts.iter().any(|part| matches(Some(part))),
            Parts::Unparseable(raw) => {
                // Never eligible for allow, so the flags are set to say so.
                matches(Some(&Command { text: raw.clone(), redirects: true, unsafe_prefix: true }))
            }
        }
    }

    /// True when every part matches and every part is plain enough for an allow rule.
    fn all(&self, matches: impl Fn(Option<&Command>) -> bool) -> bool {
        match self {
            Parts::Single => matches(None),
            Parts::Commands(parts) => {
                parts.iter().all(|part| !part.redirects && !part.unsafe_prefix && matches(Some(part)))
            }
            Parts::Unparseable(_) => false,
        }
    }
}

fn rule_matches(rule: &Rule, name: &str, arguments: &Value, part: Option<&Command>, context: &ToolContext) -> bool {
    let specifier = rule.specifier.as_deref();
    match rule.tool.as_str() {
        "Bash" => {
            name == "Bash" && specifier.is_none_or(|spec| part.is_some_and(|part| bash_matches(spec, &part.text)))
        }
        "Read" => name == "Read" && path_rule_matches(specifier, arguments, context),
        "Edit" | "Write" | "NotebookEdit" => {
            EDIT_TOOLS.contains(&name) && path_rule_matches(specifier, arguments, context)
        }
        "WebFetch" => {
            name == "WebFetch"
                && specifier.is_none_or(|spec| {
                    let domain = spec.strip_prefix("domain:").unwrap_or(spec).to_lowercase();
                    let url = arguments.get("url").and_then(Value::as_str).unwrap_or_default();
                    host_of(url).is_some_and(|host| glob_match(domain.trim_end_matches('.'), &host))
                })
        }
        "WebSearch" => name == "WebSearch",
        // Grep and Glob take a path, like Read, so a rule may name the folder they search.
        "Grep" | "Glob" => name == rule.tool.as_str() && path_rule_matches(specifier, arguments, context),
        "TodoWrite" => name == "TodoWrite",
        "Skill" => {
            name == "Skill"
                && specifier.is_none_or(|spec| {
                    let skill = arguments.get("skill").and_then(Value::as_str).unwrap_or_default();
                    glob_match(spec, skill)
                })
        }
        server_or_tool if server_or_tool.starts_with("mcp__") => {
            if server_or_tool.matches("__").count() == 1 {
                name == server_or_tool || name.starts_with(&format!("{server_or_tool}__"))
            } else {
                glob_match(server_or_tool, name)
            }
        }
        _ => false,
    }
}

/// `ls *` matches `ls` and `ls -la`, but not `lsof`. Without a `*` the command must be the same.
fn bash_matches(spec: &str, command: &str) -> bool {
    if spec == "*" {
        return true;
    }
    if let Some(prefix) = spec.strip_suffix(" *") {
        return command == prefix || command.starts_with(&format!("{prefix} "));
    }
    match spec.strip_suffix('*') {
        Some(prefix) => command.starts_with(prefix),
        None => command == spec,
    }
}

/// Whether a Bash deny rule's command prefix appears as consecutive words in an unparseable command.
fn denies_by_prefix(rule: &Rule, raw: &str) -> bool {
    let Some(spec) = rule.specifier.as_deref() else {
        return true;
    };
    let prefix: Vec<&str> = spec.trim_end_matches('*').split_whitespace().collect();
    if prefix.is_empty() {
        return true;
    }
    let words: Vec<&str> =
        raw.split(|c: char| !(c.is_alphanumeric() || "-_./".contains(c))).filter(|word| !word.is_empty()).collect();
    words.windows(prefix.len()).any(|window| window == prefix.as_slice())
}

fn path_rule_matches(specifier: Option<&str>, arguments: &Value, context: &ToolContext) -> bool {
    let Some(spec) = specifier else {
        return true;
    };
    let Some(path) = PATH_ARGUMENTS.iter().find_map(|key| arguments.get(*key).and_then(Value::as_str)) else {
        return false;
    };
    let candidate = normalize(&absolute(path, &context.cwd));
    let pattern = normalize(&rule_path(spec, context));
    glob_match(&pattern, &candidate)
}

/// Rule paths: `//x` is absolute (`//C:/x` for a drive), `~/x` is in the home folder,
/// `/x` is relative to the project root, and anything else is relative to the working folder.
fn rule_path(spec: &str, context: &ToolContext) -> String {
    let resolved = if let Some(rest) = spec.strip_prefix("//") {
        if rest.starts_with('/') || has_drive(rest) { PathBuf::from(rest) } else { PathBuf::from(format!("/{rest}")) }
    } else if let Some(rest) = spec.strip_prefix("~/") {
        match home_dir() {
            Some(home) => home.join(rest),
            None => return String::new(),
        }
    } else if let Some(rest) = spec.strip_prefix('/') {
        context.project_root.join(rest)
    } else {
        context.cwd.join(spec)
    };
    slashed(&resolved)
}

fn absolute(path: &str, cwd: &Path) -> String {
    let path = Path::new(path);
    if path.is_absolute() { slashed(path) } else { slashed(&cwd.join(path)) }
}

/// Resolves `.` and `..` lexically, so `src/../../x` can't pass as inside `src`. On Windows the
/// case is ignored as well.
fn normalize(path: &str) -> String {
    let (root, rest) = if has_drive(path) {
        match path[2..].strip_prefix('/') {
            Some(rest) => (&path[..3], rest),
            None => (&path[..2], &path[2..]),
        }
    } else if let Some(rest) = path.strip_prefix('/') {
        ("/", rest)
    } else {
        ("", path)
    };
    let mut segments: Vec<&str> = vec![];
    for segment in rest.split('/') {
        match segment {
            "" | "." => {}
            ".." => match segments.last() {
                Some(last) if *last != ".." => {
                    segments.pop();
                }
                // Above an absolute root there is nothing left to go up to.
                _ if root.is_empty() => segments.push(".."),
                _ => {}
            },
            other => segments.push(other),
        }
    }
    let normalized = format!("{root}{}", segments.join("/"));
    if cfg!(windows) { normalized.to_lowercase() } else { normalized }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

fn has_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// A canonical Windows path starts with `\\?\`. It names the same file as the plain path, so rules compare without it.
fn slashed(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if let Some(rest) = text.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else if let Some(rest) = text.strip_prefix("//?/") {
        rest.to_string()
    } else {
        text
    }
}

/// The host a URL connects to, lowercased and without a trailing dot. The URL parser reads the
/// host the way the client does, so rules see the same host the request goes to.
fn host_of(url: &str) -> Option<String> {
    let host = url::Url::parse(url).ok()?.host_str()?.to_lowercase();
    Some(host.trim_end_matches('.').to_string())
}

/// `*` matches within one path segment, `**` across segments.
fn glob_match(pattern: &str, text: &str) -> bool {
    glob_bytes(pattern.as_bytes(), text.as_bytes())
}

fn glob_bytes(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => {
            let (crosses_segments, rest) = match rest.split_first() {
                Some((b'*', tail)) => (true, tail),
                _ => (false, rest),
            };
            for end in 0..=text.len() {
                if glob_bytes(rest, &text[end..]) {
                    return true;
                }
                if end < text.len() && text[end] == b'/' && !crosses_segments {
                    break;
                }
            }
            false
        }
        Some((byte, rest)) => text.first() == Some(byte) && glob_bytes(rest, &text[1..]),
    }
}

/// Splits a shell command at `&&`, `||`, `;`, `|`, `&` and newlines, outside quotes.
/// `None` when the command has a substitution or ANSI-C quoting, which can hide commands from the check.
fn split_commands(command: &str) -> Option<Vec<Command>> {
    let chars: Vec<char> = command.chars().collect();
    let mut parts: Vec<(String, bool)> = vec![];
    let mut current = String::new();
    let mut redirects = false;
    let (mut single, mut double) = (false, false);
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let next = chars.get(index + 1).copied();
        if !single {
            let substitutes = c == '`'
                || (c == '$' && matches!(next, Some('(' | '\'' | '"')))
                || (matches!(c, '<' | '>') && next == Some('('));
            if substitutes {
                return None;
            }
        }
        if c == '\\' && !single {
            current.push(c);
            if let Some(escaped) = next {
                current.push(escaped);
            }
            index += 2;
            continue;
        }
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            _ => {}
        }
        if !single && !double && matches!(c, '<' | '>') && !fd_duplicate(&chars, index) {
            redirects = true;
        }
        let after_redirect =
            index.checked_sub(1).map(|prev| chars[prev]).is_some_and(|prev| prev == '>' || prev == '<');
        let separator = !single
            && !double
            && match c {
                // `&&`, `||` and `|&` are one separator, and `2>&1` or `&>` are redirects.
                '&' => !after_redirect && next != Some('>'),
                '|' | ';' | '\n' => true,
                _ => false,
            };
        if separator {
            parts.push((current.trim().to_string(), redirects));
            current.clear();
            redirects = false;
            let doubled = matches!((c, next), ('&', Some('&')) | ('|', Some('|' | '&')));
            index += if doubled { 2 } else { 1 };
        } else {
            current.push(c);
            index += 1;
        }
    }
    parts.push((current.trim().to_string(), redirects));
    parts.retain(|(text, _)| !text.is_empty());
    // An empty command must not pass as "every part passes", so it stays one empty part.
    if parts.is_empty() {
        parts.push((String::new(), false));
    }
    Some(
        parts
            .into_iter()
            .map(|(text, redirects)| {
                let (rest, unsafe_prefix) = strip_wrappers(&text);
                Command { text: rest.to_string(), redirects, unsafe_prefix }
            })
            .collect(),
    )
}

/// `>&1`, `>&2` and `>&-` only duplicate a descriptor, so they don't redirect to a file. The
/// descriptor must end the word: `>&2.bashrc` writes a file.
fn fd_duplicate(chars: &[char], index: usize) -> bool {
    if chars[index] != '>' || chars.get(index + 1) != Some(&'&') {
        return false;
    }
    let mut end = index + 2;
    if chars.get(end) == Some(&'-') {
        end += 1;
    } else {
        let digits_start = end;
        while chars.get(end).is_some_and(char::is_ascii_digit) {
            end += 1;
        }
        if end == digits_start {
            return false;
        }
    }
    chars.get(end).is_none_or(|c| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '\n'))
}

/// Drops wrappers such as `timeout 5` and leading `NAME=value` assignments, so the command
/// underneath is what the rules see. The flag says whether an assignment isn't a safe variable.
fn strip_wrappers(part: &str) -> (&str, bool) {
    let mut rest = part.trim();
    let mut unsafe_prefix = false;
    loop {
        let (word, after) = split_word(rest);
        if is_assignment(word) {
            unsafe_prefix |= !is_safe_assignment(word);
            rest = after;
        } else if WRAPPERS.contains(&word) {
            rest = skip_options(after);
            if word == "timeout" {
                rest = split_word(rest).1;
            }
        } else {
            return (rest, unsafe_prefix);
        }
    }
}

fn is_safe_assignment(word: &str) -> bool {
    let name = word.split_once('=').map_or(word, |(name, _)| name);
    SAFE_VARIABLES.contains(&name) || name.starts_with("LC_")
}

fn split_word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    match text.find(char::is_whitespace) {
        Some(end) => (&text[..end], text[end..].trim_start()),
        None => (text, ""),
    }
}

fn skip_options(mut text: &str) -> &str {
    loop {
        let (word, after) = split_word(text);
        if !word.starts_with('-') || word.is_empty() {
            return text;
        }
        text = after;
    }
}

fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}
