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
    /// The HTML version of the body, with embedded pictures filled in. It is untrusted: the app cleans it before showing it.
    pub html: String,
    pub attachments: Vec<Attachment>,
    /// One of `primary`, `promotions`, `social`, `updates`, `forums`.
    pub category: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attachment {
    pub name: String,
    pub kind: String,
    pub size: usize,
    /// A `data:` address for a picture small enough to show in the message.
    pub preview: Option<String>,
}

const MAX_HTML_CHARS: usize = 1_500_000;
const MAX_PART_BYTES: usize = 12_000_000;
const MAX_PREVIEW_BYTES: usize = 4_000_000;
const MAX_MEDIA_BYTES: usize = 16_000_000;

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
    decode_entities(&text)
}

/// `&amp;`, `&nbsp;`, `&#8211;` and `&#x2013;` become the characters they stand for; unknown names stay as written.
pub fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        let decoded = tail.find(';').filter(|end| *end <= 10).and_then(|end| {
            let name = &tail[..end];
            let ch = if let Some(num) = name.strip_prefix('#') {
                let code = match num.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => num.parse().ok(),
                };
                code.and_then(char::from_u32)
            } else {
                match name {
                    "nbsp" => Some(' '),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "amp" => Some('&'),
                    "apos" => Some('\''),
                    "zwnj" => Some('\u{200C}'),
                    "zwj" => Some('\u{200D}'),
                    "hellip" => Some('\u{2026}'),
                    "ndash" => Some('\u{2013}'),
                    "mdash" => Some('\u{2014}'),
                    "lsquo" => Some('\u{2018}'),
                    "rsquo" => Some('\u{2019}'),
                    "ldquo" => Some('\u{201C}'),
                    "rdquo" => Some('\u{201D}'),
                    "bull" => Some('\u{2022}'),
                    "copy" => Some('\u{A9}'),
                    "reg" => Some('\u{AE}'),
                    "trade" => Some('\u{2122}'),
                    "euro" => Some('\u{20AC}'),
                    _ => None,
                }
            };
            ch.map(|c| (c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Default)]
struct Found {
    plain: Option<String>,
    html: Option<String>,
    inline: Vec<(String, String, Vec<u8>)>,
    files: Vec<(String, String, Vec<u8>, usize)>,
    media_bytes: usize,
}

fn decode_body(head: &[(String, String)], body: &[u8]) -> Option<Vec<u8>> {
    if body.len() > MAX_PART_BYTES * 2 {
        return None;
    }
    match header(head, "content-transfer-encoding").to_ascii_lowercase().as_str() {
        "base64" => {
            let clean: Vec<u8> = body.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
            base64::engine::general_purpose::STANDARD.decode(clean).ok()
        }
        "quoted-printable" => Some(qp_decode(body)),
        _ => Some(body.to_vec()),
    }
}

fn walk(head: &[(String, String)], body: &[u8], depth: usize, found: &mut Found) {
    let content_type = header(head, "content-type");
    let kind = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let kind = if kind.is_empty() { "text/plain".to_string() } else { kind };
    if kind.starts_with("multipart/") {
        let Some(boundary) = param(content_type, "boundary") else { return };
        if depth >= MAX_DEPTH {
            return;
        }
        let marker = format!("--{boundary}");
        let text = String::from_utf8_lossy(body).into_owned();
        for part in text.split(&marker).skip(1) {
            if part.starts_with("--") {
                break;
            }
            let part = part.trim_start_matches(['\r', '\n']);
            let (phead, pbody) = split_head_body(part.as_bytes());
            walk(&headers(phead), pbody, depth + 1, found);
        }
        return;
    }
    let disposition = header(head, "content-disposition");
    let attached = disposition.to_ascii_lowercase().starts_with("attachment");
    let name = param(disposition, "filename")
        .or_else(|| param(content_type, "name"))
        .map(|n| clean(&decode_words(&n)).replace('\n', " "))
        .unwrap_or_default();
    if (kind == "text/plain" || kind == "text/html") && !attached && name.is_empty() {
        let Some(decoded) = decode_body(head, body) else { return };
        let text = charset_text(&decoded, &param(content_type, "charset").unwrap_or_else(|| "utf-8".into()));
        if kind == "text/html" {
            found.html.get_or_insert(text);
        } else {
            found.plain.get_or_insert(text);
        }
        return;
    }
    if kind.starts_with("message/") {
        return;
    }
    let size = body.len() / 4 * 3;
    let cid = header(head, "content-id").trim().trim_matches(['<', '>']).to_string();
    let wants_bytes = kind.starts_with("image/") && size <= MAX_PREVIEW_BYTES && found.media_bytes + size <= MAX_MEDIA_BYTES;
    let bytes = if wants_bytes { decode_body(head, body) } else { None };
    let size = bytes.as_ref().map_or(size, Vec::len);
    if let Some(bytes) = &bytes {
        found.media_bytes += bytes.len();
    }
    if !cid.is_empty() && kind.starts_with("image/") && !attached {
        if let Some(bytes) = bytes {
            found.inline.push((cid, kind, bytes));
        }
        return;
    }
    let shown = if name.is_empty() { "attachment".to_string() } else { name };
    found.files.push((shown, kind, bytes.unwrap_or_default(), size));
}

fn data_uri(kind: &str, bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    matches!(kind, "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp" | "image/bmp" | "image/avif")
        .then(|| format!("data:{kind};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// Keeps new lines and tabs, drops other control characters, and refuses a page too big to show.
fn clean_html(html: &str) -> String {
    if html.chars().count() > MAX_HTML_CHARS {
        return String::new();
    }
    html.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t' | '\r')).collect()
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
    let mut found = Found::default();
    walk(&head, body, 0, &mut found);
    let mut html = found.html.clone().unwrap_or_default();
    for (cid, kind, bytes) in &found.inline {
        if let Some(uri) = data_uri(kind, bytes) {
            html = html.replace(&format!("cid:{cid}"), &uri);
        }
    }
    let text = match (&found.plain, &found.html) {
        (Some(plain), _) => plain.clone(),
        (None, Some(html)) => strip_html(html),
        (None, None) => String::new(),
    };
    let attachments = found
        .files
        .into_iter()
        .take(30)
        .map(|(name, kind, bytes, size)| Attachment { name, preview: data_uri(&kind, &bytes), kind, size })
        .collect();
    let single = |name: &str| clean(&decode_words(header(&head, name))).replace('\n', " ");
    let (from, subject) = (single("from"), single("subject"));
    let category = categorize(&head, &from, &subject).to_string();
    Parsed {
        from,
        to: single("to"),
        subject,
        date: single("date"),
        message_id: header(&head, "message-id").trim().to_string(),
        references: header(&head, "references").trim().to_string(),
        text: clean(&text),
        html: clean_html(&html),
        attachments,
        category,
    }
}

/// Which tab of the inbox a message belongs in, decided from how it was sent and what it says, with no model involved.
pub fn categorize(head: &[(String, String)], from: &str, subject: &str) -> &'static str {
    let from = from.to_ascii_lowercase();
    let subject = subject.to_ascii_lowercase();
    let has = |words: &[&str], text: &str| words.iter().any(|w| text.contains(w));
    if has(
        &[
            "facebookmail", "facebook.com", "linkedin", "twitter.com", "x.com", "instagram", "tiktok", "pinterest", "reddit", "discord", "snapchat", "meetup.com",
            "tumblr", "quora", "mastodon", "bsky",
        ],
        &from,
    ) {
        return "social";
    }
    let bulk = !header(head, "list-unsubscribe").is_empty() || header(head, "precedence").to_ascii_lowercase().contains("bulk");
    if !header(head, "list-id").is_empty() && !has(&["marketing", "promo", "offers", "deals"], &from) {
        return "forums";
    }
    let transactional = has(
        &[
            "verify", "verif", "confirm", "receipt", "invoice", "order", "refund", "password", "parol", "security", "sign-in", "sign in", "login", "your code",
            "one-time", "reset", "payment", "plata", "plat\u{103}", "factur", "comand", "livrare", "delivery", "shipped", "tracking", "statement", "alert", "reminder",
            "booking", "rezerv", "ticket", "bilet", "your account", "ramburs",
        ],
        &subject,
    );
    let automated = has(
        &["noreply", "no-reply", "donotreply", "do-not-reply", "notification", "alerts@", "billing", "receipts@", "orders@", "service@", "support@", "security@"],
        &from,
    );
    let promo_subject = has(
        &[
            "% off", "sale", "deal", "discount", "offer", "save ", "free ", "coupon", "promo", "newsletter", "reducer", "ofert", "cupon", "premium", "upgrade",
            "last chance", "ends ", "limited", "new arrivals", "recommended", "insider", "what you earned", "trending", "don't miss", "weekly", "welcome to",
        ],
        &subject,
    ) || has(&["marketing", "promo", "newsletter", "news@", "offers", "deals", "hello@", "insider", "@mail."], &from);
    if bulk {
        return if transactional && !promo_subject { "updates" } else { "promotions" };
    }
    if promo_subject && !transactional {
        return "promotions";
    }
    if transactional || automated {
        return "updates";
    }
    "primary"
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

    #[test]
    fn html_body_keeps_markup_and_inlines_embedded_pictures() {
        let png = base64::engine::general_purpose::STANDARD.encode([137u8, 80, 78, 71, 1, 2, 3]);
        let raw = format!(
            "Subject: x\r\nContent-Type: multipart/related; boundary=\"R\"\r\n\r\n--R\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Hi</p><img src=\"cid:logo1\">\r\n--R\r\nContent-Type: image/png\r\nContent-ID: <logo1>\r\nContent-Transfer-Encoding: base64\r\n\r\n{png}\r\n--R\r\nContent-Type: application/pdf; name=\"plan.pdf\"\r\nContent-Disposition: attachment; filename=\"plan.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0=\r\n--R--\r\n"
        );
        let m = parse_message(raw.as_bytes());
        assert!(m.html.contains("<p>Hi</p>"));
        assert!(m.html.contains("src=\"data:image/png;base64,"), "{}", m.html);
        assert_eq!(m.attachments.len(), 1);
        assert_eq!(m.attachments[0].name, "plan.pdf");
        assert!(m.attachments[0].preview.is_none());
        assert_eq!(m.text, "Hi");
    }

    #[test]
    fn an_attached_picture_gets_a_preview() {
        let png = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3, 4]);
        let raw = format!(
            "Content-Type: multipart/mixed; boundary=\"M\"\r\n\r\n--M\r\nContent-Type: text/plain\r\n\r\nSee photo\r\n--M\r\nContent-Type: image/jpeg; name=\"cat.jpg\"\r\nContent-Disposition: attachment; filename=\"cat.jpg\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{png}\r\n--M--\r\n"
        );
        let m = parse_message(raw.as_bytes());
        assert_eq!(m.attachments[0].name, "cat.jpg");
        assert!(m.attachments[0].preview.as_deref().is_some_and(|p| p.starts_with("data:image/jpeg;base64,")));
    }

    #[test]
    fn entities_are_decoded() {
        assert_eq!(decode_entities("a&nbsp;&amp;&#8211;&#x2013;&zwnj;b &unknown; &"), "a &\u{2013}\u{2013}\u{200C}b &unknown; &");
    }

    #[test]
    fn mail_is_sorted_into_categories() {
        let cat = |raw: &str| parse_message(raw.as_bytes()).category;
        assert_eq!(cat("From: Facebook <notification@facebookmail.com>\r\nSubject: Ana wants to connect\r\n\r\nx"), "social");
        assert_eq!(cat("From: \"Marvel Insider\" <marvel@mail.marvel.com>\r\nSubject: See what you earned\r\nList-Unsubscribe: <mailto:u@x>\r\n\r\nx"), "promotions");
        assert_eq!(cat("From: JustWatch <noreply@justwatch.com>\r\nSubject: Verify your email\r\n\r\nx"), "updates");
        assert_eq!(cat("From: Rust list <users@lists.rust-lang.org>\r\nSubject: Question\r\nList-Id: <users.lists>\r\n\r\nx"), "forums");
        assert_eq!(cat("From: Ioana <ioana@example.com>\r\nSubject: Cina diseara?\r\n\r\nx"), "primary");
    }
}
