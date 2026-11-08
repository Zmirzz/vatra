//! Reads an email message: headers, the readable text of the body, and the encodings in between. What comes out is
//! text a stranger wrote, so it is cleaned (no control or direction-changing characters) and cut to a safe size;
//! it is never treated as an instruction by anything.

use base64::Engine;

const MAX_TEXT_CHARS: usize = 60_000;
const MAX_DEPTH: usize = 5;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub date: String,
    pub message_id: String,
    pub references: String,
    pub text: String,
}

/// Header lines, names in lower case, folded lines joined.
pub fn headers(block: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(block).replace("\r\n", "\n");
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.split('\n') {
        if line.is_empty() {
            break;
        }
        if (line.starts_with(' ') || line.starts_with('\t')) && !out.is_empty() {
            let last = out.last_mut().expect("checked");
            last.1.push(' ');
            last.1.push_str(line.trim());
        } else if let Some((name, value)) = line.split_once(':') {
            out.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    out
}

fn header<'a>(list: &'a [(String, String)], name: &str) -> &'a str {
    list.iter().find(|(n, _)| n == name).map_or("", |(_, v)| v.as_str())
}

fn split_head_body(bytes: &[u8]) -> (&[u8], &[u8]) {
    for (i, window) in bytes.windows(4).enumerate() {
        if window == b"\r\n\r\n" {
            return (&bytes[..i], &bytes[i + 4..]);
        }
    }
    for (i, window) in bytes.windows(2).enumerate() {
        if window == b"\n\n" {
            return (&bytes[..i], &bytes[i + 2..]);
        }
    }
    (bytes, &[])
}

fn charset_text(bytes: &[u8], charset: &str) -> String {
    match charset.to_ascii_lowercase().as_str() {
        "iso-8859-1" | "latin1" | "iso-8859-15" | "windows-1252" | "us-ascii" | "ascii" => bytes.iter().map(|b| *b as char).collect(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// `=?utf-8?B?...?=` and `=?iso-8859-1?Q?...?=` words of a header.
pub fn decode_words(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    let mut last_was_word = false;
    while let Some(start) = rest.find("=?") {
        let (before, tail) = rest.split_at(start);
        let parts: Vec<&str> = tail[2..].splitn(4, '?').collect();
        if parts.len() == 4 && parts[3].starts_with('=') {
            let (charset, enc, data) = (parts[0], parts[1], parts[2]);
            let decoded = match enc.to_ascii_uppercase().as_str() {
                "B" => base64::engine::general_purpose::STANDARD.decode(data).ok().map(|b| charset_text(&b, charset)),
                "Q" => Some(charset_text(&q_decode(data), charset)),
                _ => None,
            };
            if let Some(decoded) = decoded {
                if !(last_was_word && before.trim().is_empty()) {
                    out.push_str(before);
                }
                out.push_str(&decoded);
                last_was_word = true;
                let consumed = start + 2 + charset.len() + 1 + enc.len() + 1 + data.len() + 2;
                rest = &rest[consumed..];
                continue;
            }
        }
        out.push_str(before);
        out.push_str("=?");
        rest = &tail[2..];
        last_was_word = false;
    }
    out.push_str(rest);
    out
}

fn q_decode(data: &str) -> Vec<u8> {
    let bytes = data.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'_' => out.push(b' '),
            b'=' if i + 3 <= bytes.len() => {
                if let Ok(b) = u8::from_str_radix(&data[i + 1..i + 3], 16) {
                    out.push(b);
                    i += 2;
                } else {
                    out.push(b'=');
                }
            }
            other => out.push(other),
        }
        i += 1;
    }
    out
}

fn qp_decode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'=' {
            if data.get(i + 1) == Some(&b'\r') && data.get(i + 2) == Some(&b'\n') {
                i += 3;
                continue;
            }
            if data.get(i + 1) == Some(&b'\n') {
                i += 2;
                continue;
            }
            if i + 2 < data.len()
                && let Ok(text) = std::str::from_utf8(&data[i + 1..i + 3])
                && let Ok(b) = u8::from_str_radix(text, 16)
            {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(data[i]);
        i += 1;
    }
    out
}

fn param(value: &str, name: &str) -> Option<String> {
    for part in value.split(';').skip(1) {
        if let Some((k, v)) = part.split_once('=')
            && k.trim().eq_ignore_ascii_case(name)
        {
            return Some(v.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn strip_html(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    'outer: while i < html.len() {
        if html.as_bytes()[i] == b'<' {
            for block in ["script", "style"] {
                if lower[i..].starts_with(&format!("<{block}"))
                    && let Some(end) = lower[i..].find(&format!("</{block}>"))
                {
                    i += end + block.len() + 3;
                    continue 'outer;
                }
            }
            if let Some(end) = html[i..].find('>') {
                let tag = lower[i + 1..i + end].trim_start_matches('/').to_string();
                if ["br", "p", "div", "li", "tr"].iter().any(|t| tag.starts_with(t)) {
                    text.push('\n');
                }
                i += end + 1;
                continue;
            }
        }
        let ch = html[i..].chars().next().unwrap_or(' ');
        text.push(ch);
        i += ch.len_utf8();
    }
    text.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

fn body_text(head: &[(String, String)], body: &[u8], depth: usize) -> Option<(String, bool)> {
    let content_type = header(head, "content-type");
    let kind = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let kind = if kind.is_empty() { "text/plain".to_string() } else { kind };
    if kind.starts_with("multipart/") {
        if depth >= MAX_DEPTH {
            return None;
        }
        let boundary = param(content_type, "boundary")?;
        let marker = format!("--{boundary}");
        let text = String::from_utf8_lossy(body).into_owned();
        let mut plain: Option<String> = None;
        let mut html: Option<String> = None;
        for part in text.split(&marker).skip(1) {
            if part.starts_with("--") {
                break;
            }
            let part = part.trim_start_matches(['\r', '\n']);
            let (phead, pbody) = split_head_body(part.as_bytes());
            let phead = headers(phead);
            if header(&phead, "content-disposition").to_ascii_lowercase().starts_with("attachment") {
                continue;
            }
            if let Some((found, is_html)) = body_text(&phead, pbody, depth + 1) {
                if is_html {
                    html.get_or_insert(found);
                } else {
                    plain.get_or_insert(found);
                }
            }
        }
        return plain.map(|p| (p, false)).or(html.map(|h| (h, true)));
    }
    if !(kind == "text/plain" || kind == "text/html") {
        return None;
    }
    let decoded = match header(head, "content-transfer-encoding").to_ascii_lowercase().as_str() {
        "base64" => {
            let clean: Vec<u8> = body.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
            base64::engine::general_purpose::STANDARD.decode(clean).ok()?
        }
        "quoted-printable" => qp_decode(body),
        _ => body.to_vec(),
    };
    let text = charset_text(&decoded, &param(content_type, "charset").unwrap_or_else(|| "utf-8".into()));
    Some((text, kind == "text/html"))
}

/// Removes what could hide or rewrite text on screen: control characters (but not new lines or tabs) and the
/// characters that change the direction of text. The result is cut to a size every consumer can take.
pub fn clean(text: &str) -> String {
    let mut out: String = text
        .chars()
        .filter(|c| {
            (!c.is_control() || matches!(c, '\n' | '\t'))
                && !matches!(*c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200B}'..='\u{200F}' | '\u{FEFF}')
        })
        .collect();
    out = out.replace("\r\n", "\n");
    if out.chars().count() > MAX_TEXT_CHARS {
        out = out.chars().take(MAX_TEXT_CHARS).collect::<String>() + "\n[The message is longer than this.]";
    }
    out.trim().to_string()
}

pub fn parse_message(bytes: &[u8]) -> Parsed {
    let (head_bytes, body) = split_head_body(bytes);
    let head = headers(head_bytes);
    let text = match body_text(&head, body, 0) {
        Some((text, true)) => strip_html(&text),
        Some((text, false)) => text,
        None => String::new(),
    };
    let single = |name: &str| clean(&decode_words(header(&head, name))).replace('\n', " ");
    Parsed {
        from: single("from"),
        to: single("to"),
        subject: single("subject"),
        date: single("date"),
        message_id: header(&head, "message-id").trim().to_string(),
        references: header(&head, "references").trim().to_string(),
        text: clean(&text),
    }
}

/// An RFC 2047 encoded word for a header that has non-ASCII letters.
pub fn encode_header(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }
    format!("=?utf-8?B?{}?=", base64::engine::general_purpose::STANDARD.encode(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_message_with_encoded_headers() {
        let raw = "From: =?utf-8?B?SW9hbmEgUG9wZXNjdQ==?= <ioana@example.com>\r\nSubject: =?iso-8859-1?Q?Caf=E9_bun?=\r\nMessage-ID: <a1@x>\r\n\r\nSalut,\r\nce faci?\r\n";
        let m = parse_message(raw.as_bytes());
        assert_eq!(m.from, "Ioana Popescu <ioana@example.com>");
        assert_eq!(m.subject, "Café bun");
        assert_eq!(m.message_id, "<a1@x>");
        assert_eq!(m.text, "Salut,\nce faci?");
    }

    #[test]
    fn multipart_prefers_plain_and_skips_attachments() {
        let raw = "Subject: x\r\nContent-Type: multipart/alternative; boundary=\"B\"\r\n\r\n--B\r\nContent-Type: text/html\r\n\r\n<p>Html <b>text</b></p>\r\n--B\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nPlain =C8=99i =\r\ncontinuat\r\n--B\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment\r\n\r\nJVBER\r\n--B--\r\n";
        let m = parse_message(raw.as_bytes());
        assert_eq!(m.text, "Plain și continuat");
    }

    #[test]
    fn html_only_is_turned_into_text_without_scripts() {
        let raw = "Content-Type: text/html\r\n\r\n<style>p{}</style><p>Buna</p><script>alert(1)</script><div>ziua &amp; seara</div>";
        let m = parse_message(raw.as_bytes());
        assert!(m.text.contains("Buna") && m.text.contains("ziua & seara"));
        assert!(!m.text.contains("alert") && !m.text.contains("p{}"));
    }

    #[test]
    fn base64_body_and_hidden_characters() {
        let body = base64::engine::general_purpose::STANDARD.encode("Salut\u{202E}evil\u{200B} mesaj");
        let raw = format!("Content-Type: text/plain\r\nContent-Transfer-Encoding: base64\r\n\r\n{body}");
        let m = parse_message(raw.as_bytes());
        assert_eq!(m.text, "Salutevil mesaj");
    }

    #[test]
    fn control_characters_are_removed() {
        assert_eq!(clean("a\u{0007}b"), "ab");
        assert_eq!(parse_message(b"Subject: ok\r\n\r\nbody").subject, "ok");
    }

    #[test]
    fn a_non_ascii_header_is_encoded() {
        assert_eq!(decode_words(&encode_header("Ședință")), "Ședință");
        assert_eq!(encode_header("simple"), "simple");
    }

    #[test]
    fn very_long_text_is_cut() {
        let long = "a".repeat(MAX_TEXT_CHARS + 500);
        assert!(clean(&long).ends_with("longer than this.]"));
    }
}
