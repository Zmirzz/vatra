use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Supervised,
    AcceptEdits,
    FullAccess,
}

/// One rule as written in settings, such as `Bash(git *)`, `Read(/src/**)` or `mcp__github__*`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub tool: String,
    pub specifier: Option<String>,
}

const TOOLS: [&str; 11] = [
    "Bash",
    "Read",
    "Edit",
    "Write",
    "NotebookEdit",
    "WebFetch",
    "WebSearch",
    "Skill",
    "Grep",
    "Glob",
    "TodoWrite",
];

impl Rule {
    pub fn parse(text: &str) -> Result<Rule, String> {
        let text = text.trim();
        let (tool, specifier) = match text.split_once('(') {
            None => (text, None),
            Some((tool, rest)) => {
                let Some(specifier) = rest.strip_suffix(')') else {
                    return Err(format!("the rule `{text}` is missing its closing `)`"));
                };
                if specifier.trim().is_empty() {
                    return Err(format!("the rule `{text}` has empty parentheses"));
                }
                (tool.trim_end(), Some(specifier.to_string()))
            }
        };
        let is_mcp = tool.starts_with("mcp__") && tool.len() > "mcp__".len();
        if !TOOLS.contains(&tool) && !is_mcp {
            return Err(format!("unknown tool in the rule `{text}`"));
        }
        if is_mcp && specifier.is_some() {
            return Err(format!("the rule `{text}` can't have parentheses: name the server or the tool"));
        }
        if tool == "WebFetch" && specifier.as_deref().is_some_and(|spec| !spec.starts_with("domain:")) {
            return Err(format!("the rule `{text}` must name a domain, as in `WebFetch(domain:example.com)`"));
        }
        if tool == "WebSearch" && specifier.is_some() {
            return Err(format!("the rule `{text}` can't have parentheses"));
        }
        Ok(Rule { tool: tool.to_string(), specifier })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    pub allow: Vec<Rule>,
    pub ask: Vec<Rule>,
    pub deny: Vec<Rule>,
}

impl Rules {
    /// Keeps every rule that parses. Each one that doesn't is returned as a message instead.
    pub fn parse<S: AsRef<str>>(allow: &[S], ask: &[S], deny: &[S]) -> (Rules, Vec<String>) {
        let mut errors = vec![];
        let mut parse_all = |texts: &[S]| -> Vec<Rule> {
            texts
                .iter()
                .filter_map(|text| match Rule::parse(text.as_ref()) {
                    Ok(rule) => Some(rule),
                    Err(message) => {
                        errors.push(message);
                        None
                    }
                })
                .collect()
        };
        let rules = Rules { allow: parse_all(allow), ask: parse_all(ask), deny: parse_all(deny) };
        (rules, errors)
    }
}
