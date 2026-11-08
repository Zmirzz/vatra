use std::path::Path;

/// The values a skill body can refer to.
pub struct SubstituteInput<'a> {
    /// The raw argument text the user typed after the command name.
    pub arguments: &'a str,
    /// The skill's declared argument names, in order. `$name` takes the value at the same position.
    pub argument_names: &'a [String],
    pub skill_dir: &'a Path,
    pub session_id: &'a str,
    /// The `${CLAUDE_*}` values below are left as written when they are `None`.
    pub plugin_root: Option<&'a Path>,
    pub project_dir: Option<&'a Path>,
    pub plugin_data: Option<&'a Path>,
    pub effort: Option<&'a str>,
}

/// What one `$` placeholder becomes, and how many bytes after the `$` it takes.
struct Placeholder {
    value: String,
    len: usize,
    /// Whether it takes a value from the arguments, which keeps them from being appended.
    arguments: bool,
}

/// Fills in the placeholders of a skill body. The result is one pass, so a value that contains `$` is not
/// filled in again. `\$` gives a literal `$`. When the body takes no value from the arguments and some were
/// given, they are added at the end as `ARGUMENTS: <value>`.
pub fn substitute(body: &str, input: &SubstituteInput) -> String {
    let args = split_args(input.arguments);
    let mut out = String::with_capacity(body.len());
    let mut used_arguments = false;
    let mut rest = body;
    while let Some(pos) = rest.find(['$', '\\']) {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        if let Some(after) = tail.strip_prefix("\\$") {
            out.push('$');
            rest = after;
        } else if let Some(after) = tail.strip_prefix('\\') {
            out.push('\\');
            rest = after;
        } else {
            match placeholder(&tail[1..], input, &args) {
                Some(found) => {
                    out.push_str(&found.value);
                    used_arguments |= found.arguments;
                    rest = &tail[1 + found.len..];
                }
                None => {
                    out.push('$');
                    rest = &tail[1..];
                }
            }
        }
    }
    out.push_str(rest);
    if !used_arguments && !input.arguments.is_empty() {
        out.push_str("\n\nARGUMENTS: ");
        out.push_str(input.arguments);
    }
    out
}

/// The placeholder after a `$`, or `None` when the text is not one this skill can fill.
fn placeholder(tail: &str, input: &SubstituteInput, args: &[String]) -> Option<Placeholder> {
    if let Some(inner) = tail.strip_prefix('{') {
        let end = inner.find('}')?;
        let value = match &inner[..end] {
            "CLAUDE_SKILL_DIR" => input.skill_dir.to_string_lossy().into_owned(),
            "CLAUDE_SESSION_ID" => input.session_id.to_string(),
            "CLAUDE_PLUGIN_ROOT" => input.plugin_root?.to_string_lossy().into_owned(),
            "CLAUDE_PROJECT_DIR" => input.project_dir?.to_string_lossy().into_owned(),
            "CLAUDE_PLUGIN_DATA" => input.plugin_data?.to_string_lossy().into_owned(),
            "CLAUDE_EFFORT" => input.effort?.to_string(),
            _ => return None,
        };
        return Some(Placeholder {
            value,
            len: end + 2,
            arguments: false,
        });
    }

    let name_len = tail
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(tail.len());
    let name = &tail[..name_len];
    if name == "ARGUMENTS" {
        if let Some(index_tail) = tail[name_len..].strip_prefix('[') {
            let end = index_tail.find(']')?;
            let index = index_tail[..end].parse::<usize>().ok()?;
            let value = args.get(index).cloned().unwrap_or_default();
            return Some(Placeholder {
                value,
                len: name_len + end + 2,
                arguments: true,
            });
        }
        return Some(Placeholder {
            value: input.arguments.to_string(),
            len: name_len,
            arguments: true,
        });
    }
    if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
        let index = name.parse::<usize>().ok()?;
        let value = args.get(index).cloned().unwrap_or_default();
        return Some(Placeholder {
            value,
            len: name_len,
            arguments: true,
        });
    }
    let position = input
        .argument_names
        .iter()
        .position(|argument| argument == name)?;
    let value = args.get(position).cloned().unwrap_or_default();
    Some(Placeholder {
        value,
        len: name_len,
        arguments: true,
    })
}

/// Splits the arguments the way a shell does for words: spaces separate them, quotes group them, and a
/// backslash escapes the next character.
fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some('\'') => {
                if c == '\'' {
                    quote = None;
                } else {
                    current.push(c);
                }
            }
            Some(_) => match c {
                '"' => quote = None,
                '\\' if matches!(chars.peek(), Some('"' | '\\')) => {
                    current.push(chars.next().unwrap_or('\\'))
                }
                _ => current.push(c),
            },
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    started = true;
                }
                '\\' => {
                    started = true;
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                c if c.is_whitespace() => {
                    if started {
                        args.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                _ => {
                    current.push(c);
                    started = true;
                }
            },
        }
    }
    if started {
        args.push(current);
    }
    args
}
