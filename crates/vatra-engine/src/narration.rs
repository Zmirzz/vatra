/// How many times in a row a turn nudges a model that announced an action without calling a tool.
pub(crate) const MAX_NUDGES: usize = 2;

/// The message that asks the model to call the tool it described, or to say plainly that the work is done.
pub(crate) const NUDGE: &str = "You described an action but did not call a tool. Call the tool now. If the work is \
     already done, say so plainly; if you need the person's answer, ask them.";

pub(crate) const PLAN_NUDGE: &str = "The person asked you to plan first. Call TodoWrite now with your actual steps before using other tools. Do not describe a tool call in text; call the tool.";

/// Only direct instructions in the current prompt trigger planning, not quoted lines or fenced examples.
pub(crate) fn requests_plan(prompt: &str) -> bool {
    let mut fenced = false;
    for line in prompt.lines() {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced || line.starts_with(['>', '\'', '"']) { continue; }
        let plain = unquoted_text(line);
        for sentence in plain.split(['.', '!', '?']) {
            let lower = sentence.trim().to_lowercase();
            let instruction = lower.strip_prefix("please ").unwrap_or(&lower);
            if ["plan your steps", "plan first", "make a plan", "plan the work", "fă un plan", "fa un plan", "planifică pașii", "planifica pasii"]
                .iter().any(|prefix| instruction.strip_prefix(prefix).is_some_and(|rest|
                    rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || matches!(c, ',' | ':' | ';')))) { return true; }
        }
    }
    false
}

fn unquoted_text(line: &str) -> String {
    let mut plain = String::new();
    let mut closing = None;
    let mut escaped = false;
    let mut previous_letter = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        let apostrophe = c == '\'' && previous_letter && chars.peek().is_some_and(|c| c.is_alphanumeric());
        if let Some(end) = closing {
            if c == end && !escaped && !apostrophe { closing = None; }
            plain.push(' ');
        } else if !escaped && !apostrophe && matches!(c, '"' | '\'' | '`' | '“' | '„') {
            closing = Some(if matches!(c, '“' | '„') { '”' } else { c });
            plain.push(' ');
        } else {
            plain.push(c);
        }
        escaped = c == '\\' && !escaped;
        previous_letter = c.is_alphanumeric();
    }
    plain
}

/// Whether an answer without tool calls says that an action is about to happen, so it was not done.
pub(crate) fn announces_unperformed_action(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.ends_with('?') {
        return false;
    }
    // A curly apostrophe is the same as a straight one.
    let lowered = trimmed.to_lowercase().replace('\u{2019}', "'");
    const PHRASES: [&str; 19] = [
        "and will now ",
        "i will now ",
        "i'll now ",
        "now i will ",
        "now i'll ",
        "next, i will ",
        "let me write",
        "let me create",
        "let me run",
        "let me edit",
        "i will write",
        "i will create",
        "i will run",
        "i will edit",
        "i'll write",
        "i'll create",
        "i'll run",
        "i'll edit",
        "**action:**",
    ];
    PHRASES.iter().any(|phrase| lowered.contains(phrase))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announcements_are_detected() {
        assert!(announces_unperformed_action("I'll now run the tests"));
        assert!(announces_unperformed_action("Next, I will edit api.js"));
        assert!(announces_unperformed_action("**Action:** Writing"));
    }

    #[test]
    fn curly_apostrophes_are_detected() {
        assert!(announces_unperformed_action("I\u{2019}ll now run the tests"));
        assert!(announces_unperformed_action("I\u{2019}ll write the file"));
    }

    #[test]
    fn finished_answers_are_not_announcements() {
        assert!(!announces_unperformed_action("The tests all pass."));
    }

    #[test]
    fn questions_are_not_announcements() {
        assert!(!announces_unperformed_action(
            "I will write it next. Which folder should I use?"
        ));
    }
}
