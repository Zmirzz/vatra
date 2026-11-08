//! Sending one message over SMTP. Nothing here decides whether a message should go: the caller has the person's
//! confirmation of exactly this message, and this only delivers it.

use base64::Engine;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::mime::encode_header;
use super::net::{self, Endpoint, STEP_TIMEOUT, Security, Stream};

pub struct Outgoing<'a> {
    pub from: &'a str,
    pub from_name: &'a str,
    pub to: &'a [String],
    pub subject: &'a str,
    pub body: &'a str,
    pub in_reply_to: Option<&'a str>,
    pub message_id: String,
    pub date: String,
}

fn wrap76(text: &str) -> String {
    text.as_bytes().chunks(76).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("\r\n")
}

/// The message as it goes on the wire: only plain text, the body encoded so no line can be mistaken for a command.
pub fn build(message: &Outgoing) -> String {
    let name = if message.from_name.is_empty() {
        message.from.to_string()
    } else {
        format!("{} <{}>", encode_header(&message.from_name.replace(['\r', '\n', '"', '<', '>'], " ")), message.from)
    };
    let mut out = String::new();
    out.push_str(&format!("From: {name}\r\nTo: {}\r\n", message.to.join(", ")));
    out.push_str(&format!("Subject: {}\r\nDate: {}\r\nMessage-ID: {}\r\n", encode_header(message.subject), message.date, message.message_id));
    if let Some(parent) = message.in_reply_to.filter(|p| !p.contains(['\r', '\n']) && p.starts_with('<') && p.ends_with('>')) {
        out.push_str(&format!("In-Reply-To: {parent}\r\nReferences: {parent}\r\n"));
    }
    out.push_str("MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n");
    out.push_str(&wrap76(&base64::engine::general_purpose::STANDARD.encode(message.body)));
    out.push_str("\r\n");
    out
}

struct Session {
    stream: BufReader<Stream>,
}

impl Session {
    async fn reply(&mut self) -> Result<(u16, String), String> {
        let mut all = String::new();
        loop {
            let mut line = String::new();
            let read = tokio::time::timeout(STEP_TIMEOUT, self.stream.read_line(&mut line))
                .await
                .map_err(|_| "The mail server stopped answering.".to_string())?
                .map_err(|_| "The connection to the mail server was lost.".to_string())?;
            if read == 0 {
                return Err("The mail server closed the connection.".into());
            }
            let code: u16 = line.get(..3).and_then(|c| c.parse().ok()).ok_or("The mail server answered something unexpected.")?;
            all.push_str(line.get(4..).unwrap_or("").trim_end());
            all.push('\n');
            if line.as_bytes().get(3) != Some(&b'-') {
                return Ok((code, all));
            }
        }
    }

    async fn send(&mut self, text: &str) -> Result<(), String> {
        tokio::time::timeout(STEP_TIMEOUT, async {
            self.stream.write_all(text.as_bytes()).await?;
            self.stream.flush().await
        })
        .await
        .map_err(|_| "The mail server stopped answering.".to_string())?
        .map_err(|_| "The connection to the mail server was lost.".to_string())
    }

    async fn expect(&mut self, ok: &[u16], what: &str) -> Result<String, String> {
        let (code, text) = self.reply().await?;
        if ok.contains(&code) { Ok(text) } else { Err(format!("The mail server refused {what} ({code}).")) }
    }

    async fn command(&mut self, line: &str, ok: &[u16], what: &str) -> Result<String, String> {
        self.send(&format!("{line}\r\n")).await?;
        self.expect(ok, what).await
    }
}

fn clean_address(address: &str) -> Result<&str, String> {
    if vatra_core::valid_address(address) { Ok(address) } else { Err("An address isn't valid.".into()) }
}

pub async fn send(endpoint: &Endpoint, user: &str, password: &str, message: &Outgoing<'_>) -> Result<(), String> {
    let from = clean_address(message.from)?;
    for to in message.to {
        clean_address(to)?;
    }
    let stream = net::connect(endpoint).await?;
    let mut session = Session { stream: BufReader::new(stream) };
    session.expect(&[220], "the connection").await?;
    let mut capabilities = session.command("EHLO vatra.local", &[250], "the greeting").await?;
    if endpoint.security == Security::StartTls {
        session.command("STARTTLS", &[220], "encryption").await?;
        let plain = session.stream.into_inner();
        let secured = net::upgrade(plain, &endpoint.host).await?;
        session = Session { stream: BufReader::new(secured) };
        capabilities = session.command("EHLO vatra.local", &[250], "the greeting").await?;
    }
    if !capabilities.to_ascii_uppercase().contains("AUTH") {
        return Err("The mail server doesn't offer sign-in.".into());
    }
    match crate::oauth::access_token(password) {
        Some(token) => {
            session.command(&format!("AUTH XOAUTH2 {}", crate::oauth::xoauth2(user, token)), &[235], "the sign-in").await?;
        }
        None => {
            let credentials = base64::engine::general_purpose::STANDARD.encode(format!("\0{user}\0{password}"));
            session.command(&format!("AUTH PLAIN {credentials}"), &[235], "the user name or password").await?;
        }
    }
    session.command(&format!("MAIL FROM:<{from}>"), &[250], "the sender").await?;
    for to in message.to {
        session.command(&format!("RCPT TO:<{to}>"), &[250, 251], "a recipient").await?;
    }
    session.command("DATA", &[354], "the message").await?;
    // Dot-stuffing: a line that starts with a dot gets a second one, so no line of the message ends the data.
    let body = build(message);
    let stuffed: String = body.split("\r\n").map(|l| if l.starts_with('.') { format!(".{l}") } else { l.to_string() }).collect::<Vec<_>>().join("\r\n");
    session.send(&format!("{stuffed}\r\n.\r\n")).await?;
    session.expect(&[250], "the message").await?;
    let _ = session.command("QUIT", &[221], "closing").await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_message_has_encoded_body_and_no_injected_headers() {
        let to = vec!["ana@example.com".to_string()];
        let text = build(&Outgoing {
            from: "me@example.com",
            from_name: "Vlad \"X\"\r\nBcc: evil@example.com",
            to: &to,
            subject: "Ședință",
            body: "Salut\r\n.\r\nBcc: evil@example.com",
            in_reply_to: Some("<a@x>\r\nBcc: y"),
            message_id: "<m1@vatra>".into(),
            date: "Sat, 07 Nov 2026 12:00:00 +0000".into(),
        });
        assert!(!text.lines().any(|l| l.starts_with("Bcc:")));
        assert!(!text.contains("In-Reply-To"), "an unsafe reference is dropped");
        assert!(text.contains("Subject: =?utf-8?B?"));
        assert!(text.contains("Content-Transfer-Encoding: base64"));
        assert!(text.lines().all(|l| l.len() <= 78));
    }
}
