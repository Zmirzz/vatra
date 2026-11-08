use std::ops::Range;

/// How a `!` command is written in a skill body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BangForm {
    /// `!`cmd`` on a line, after the start of the line or whitespace.
    Inline,
    /// A block that opens with a line starting with ```! and closes with ```.
    Fenced,
}

/// A command a skill body runs: its form, the bytes it covers in the body (the `!` and the backticks, or the whole
/// fenced block with its closing line), and the command text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BangSpan {
    pub form: BangForm,
    pub range: Range<usize>,
    pub command: String,
}

/// The commands of a skill body, in order. Text in any other fenced block, and text inside an inline code span, is
/// not a command. The runtime runs these spans and the catalog lists them, so both read the body with this function.
pub fn bang_spans(body: &str) -> Vec<BangSpan> {
    let lines = lines_with_offsets(body);
    let mut spans = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let (start, line) = lines[index];
        let trimmed = line.trim();
        if trimmed.starts_with("```!") {
            let mut command = String::new();
            index += 1;
            while index < lines.len() && lines[index].1.trim() != "```" {
                command.push_str(lines[index].1);
                index += 1;
            }
            // The closing line ends the span, and a block that is never closed runs to the end of the body.
            let end = lines.get(index).map_or(body.len(), |(at, closing)| at + closing.len());
            index += 1;
            if !command.trim().is_empty() {
                spans.push(BangSpan {
                    form: BangForm::Fenced,
                    range: start..end,
                    command: command.trim().into(),
                });
            }
            continue;
        }
        if let Some((marker, size)) = fence_opener(trimmed) {
            index += 1;
            while index < lines.len() && !closes_fence(lines[index].1.trim(), marker, size) {
                index += 1;
            }
            index += 1;
            continue;
        }
        scan_line(line, start, &mut spans);
        index += 1;
    }
    spans
}

/// Each line of the body with the byte offset where it starts. A line keeps its line break.
fn lines_with_offsets(body: &str) -> Vec<(usize, &str)> {
    let mut offset = 0;
    body.split_inclusive('\n')
        .map(|line| {
            let start = offset;
            offset += line.len();
            (start, line)
        })
        .collect()
}

/// The marker and length of a line that opens a fenced block of ``` or ~~~.
fn fence_opener(trimmed: &str) -> Option<(char, usize)> {
    let marker = trimmed.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let size = trimmed.chars().take_while(|c| *c == marker).count();
    (size >= 3).then_some((marker, size))
}

/// Whether a line closes a fenced block opened with `size` copies of `marker`: the same marker, at least as long, and
/// nothing else on the line.
fn closes_fence(trimmed: &str, marker: char, size: usize) -> bool {
    let run = trimmed.chars().take_while(|c| *c == marker).count();
    run >= size && trimmed[run..].trim().is_empty()
}

/// The inline commands of one line, and the code spans that hold text which is not a command. `start` is where the
/// line begins in the body.
fn scan_line(line: &str, start: usize, spans: &mut Vec<BangSpan>) {
    let mut at = 0;
    while let Some(found) = line[at..].find('`') {
        let open = at + found;
        let text_start = open + 1;
        let Some(found) = line[text_start..].find('`') else { break };
        let close = text_start + found;
        if starts_inline_command(line, open) {
            let command = line[text_start..close].trim();
            if !command.is_empty() {
                spans.push(BangSpan {
                    form: BangForm::Inline,
                    range: start + open - 1..start + close + 1,
                    command: command.into(),
                });
            }
        }
        at = close + 1;
    }
}

/// Whether the backtick at `open` follows a `!` that starts the line or follows whitespace. A `!` after a letter, a
/// digit or an underscore is text, so the backtick after it opens a code span instead.
fn starts_inline_command(line: &str, open: usize) -> bool {
    let Some(bang) = open.checked_sub(1) else { return false };
    if line.as_bytes()[bang] != b'!' {
        return false;
    }
    match line[..bang].chars().next_back() {
        None => true,
        Some(before) => before.is_whitespace(),
    }
}
