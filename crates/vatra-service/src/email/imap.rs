//! A small IMAP client: the few commands needed to read a mailbox. Every value that goes into a command is checked
//! first, so text from outside (a folder name, a flag) can never become another command.

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use super::net::{self, Endpoint, STEP_TIMEOUT, Stream};

const MAX_LITERAL: usize = 25 * 1024 * 1024;
const MAX_LINE: usize = 64 * 1024;

pub struct Imap {
    stream: BufReader<Stream>,
    tag: u32,
}

/// One untagged response: its text, with each literal replaced by a marker, and the literals.
#[derive(Debug, Default)]
pub struct Untagged {
    pub text: String,
    pub literals: Vec<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct Header {
    pub uid: u32,
    pub seen: bool,
    pub flagged: bool,
    pub raw: Vec<u8>,
}

fn quoted(value: &str) -> Result<String, String> {
    if value.chars().any(|c| c.is_control()) || value.len() > 256 {
        return Err("A value for the mail server isn't valid.".into());
    }
    Ok(format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")))
}

async fn within<T>(future: impl std::future::Future<Output = std::io::Result<T>>) -> Result<T, String> {
    tokio::time::timeout(STEP_TIMEOUT, future)
        .await
        .map_err(|_| "The mail server stopped answering.".to_string())?
        .map_err(|_| "The connection to the mail server was lost.".to_string())
}

impl Imap {
    pub async fn connect(endpoint: &Endpoint) -> Result<Imap, String> {
        let stream = net::connect(endpoint).await?;
        let mut imap = Imap { stream: BufReader::new(stream), tag: 0 };
        let greeting = imap.line().await?;
        if !greeting.starts_with("* OK") && !greeting.starts_with("* PREAUTH") {
            return Err("The mail server didn't accept the connection.".into());
        }
        Ok(imap)
    }

    async fn line(&mut self) -> Result<String, String> {
        let mut buffer = Vec::new();
        let read = within(async { (&mut self.stream).take(MAX_LINE as u64).read_until(b'\n', &mut buffer).await }).await?;
        if read == 0 {
            return Err("The mail server closed the connection.".into());
        }
        Ok(String::from_utf8_lossy(&buffer).trim_end_matches(['\r', '\n']).to_string())
    }

    /// Sends one command and reads up to its tagged answer. Err when the server answered NO or BAD.
    pub async fn command(&mut self, command: &str) -> Result<Vec<Untagged>, String> {
        self.tag += 1;
        let tag = format!("A{}", self.tag);
        if command.contains(['\r', '\n']) {
            return Err("A command for the mail server isn't valid.".into());
        }
        within(async {
            self.stream.write_all(format!("{tag} {command}\r\n").as_bytes()).await?;
            self.stream.flush().await
        })
        .await?;
        let mut responses = Vec::new();
        loop {
            let first = self.line().await?;
            // The server asks for more of a sign-in it did not accept; an empty answer ends it with a NO.
            if first.starts_with('+') {
                within(async {
                    self.stream.write_all(b"\r\n").await?;
                    self.stream.flush().await
                })
                .await?;
                continue;
            }
            if let Some(rest) = first.strip_prefix(&format!("{tag} ")) {
                return if rest.starts_with("OK") { Ok(responses) } else { Err(format!("The mail server refused the request ({}).", rest.chars().take(80).collect::<String>())) };
            }
            let mut current = Untagged::default();
            let mut line = first;
            loop {
                // A line that ends with {n} is followed by n bytes, then the rest of the response.
                let literal = line.strip_suffix('}').and_then(|l| l.rfind('{').map(|i| (l[..i].to_string(), l[i + 1..].to_string())));
                match literal.and_then(|(head, size)| size.parse::<usize>().ok().map(|n| (head, n))) {
                    Some((head, size)) => {
                        if size > MAX_LITERAL {
                            return Err("A message is too large to read.".into());
                        }
                        let mut bytes = vec![0u8; size];
                        within(async { self.stream.read_exact(&mut bytes).await }).await?;
                        current.text.push_str(&head);
                        current.text.push_str(&format!("\u{1}{}\u{1}", current.literals.len()));
                        current.literals.push(bytes);
                        line = self.line().await?;
                    }
                    None => {
                        current.text.push_str(&line);
                        break;
                    }
                }
            }
            responses.push(current);
        }
    }

    pub async fn login(&mut self, user: &str, password: &str) -> Result<(), String> {
        let command = match crate::oauth::access_token(password) {
            Some(token) => format!("AUTHENTICATE XOAUTH2 {}", crate::oauth::xoauth2(user, token)),
            None => format!("LOGIN {} {}", quoted(user)?, quoted(password)?),
        };
        self.command(&command)
            .await
            .map(|_| ())
            .map_err(|_| "The mail server refused the user name or password.".to_string())
    }

    pub async fn select(&mut self, folder: &str) -> Result<(), String> {
        self.command(&format!("SELECT {}", quoted(folder)?)).await.map(|_| ())
    }

    /// Every uid in the selected folder, oldest first.
    pub async fn uids(&mut self) -> Result<Vec<u32>, String> {
        let responses = self.command("UID SEARCH ALL").await?;
        let mut uids: Vec<u32> = responses
            .iter()
            .filter_map(|r| r.text.strip_prefix("* SEARCH"))
            .flat_map(|rest| rest.split_whitespace().filter_map(|n| n.parse().ok()).collect::<Vec<u32>>())
            .collect();
        uids.sort_unstable();
        Ok(uids)
    }

    pub async fn headers(&mut self, uids: &[u32]) -> Result<Vec<Header>, String> {
        if uids.is_empty() {
            return Ok(vec![]);
        }
        let set = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let responses = self
            .command(&format!("UID FETCH {set} (UID FLAGS BODY.PEEK[HEADER.FIELDS (FROM TO SUBJECT DATE MESSAGE-ID LIST-UNSUBSCRIBE LIST-ID PRECEDENCE)])"))
            .await?;
        let mut out = Vec::new();
        for response in responses {
            if !response.text.contains("FETCH") {
                continue;
            }
            let Some(uid) = number_after(&response.text, "UID ") else { continue };
            let flags = response.text.find("FLAGS (").map(|i| response.text[i + 7..].split(')').next().unwrap_or("").to_string()).unwrap_or_default();
            out.push(Header {
                uid,
                seen: flags.contains("\\Seen"),
                flagged: flags.contains("\\Flagged"),
                raw: response.literals.into_iter().next().unwrap_or_default(),
            });
        }
        Ok(out)
    }

    pub async fn full(&mut self, uid: u32) -> Result<Vec<u8>, String> {
        let responses = self.command(&format!("UID FETCH {uid} (BODY.PEEK[])")).await?;
        responses
            .into_iter()
            .find_map(|r| r.literals.into_iter().next())
            .ok_or_else(|| "That message isn't in the folder any more.".to_string())
    }

    pub async fn set_flag(&mut self, uid: u32, flag: &str, on: bool) -> Result<(), String> {
        if !matches!(flag, "\\Seen" | "\\Flagged") {
            return Err("That flag can't be changed.".into());
        }
        self.command(&format!("UID STORE {uid} {}FLAGS.SILENT ({flag})", if on { "+" } else { "-" })).await.map(|_| ())
    }

    pub async fn logout(&mut self) {
        let _ = self.command("LOGOUT").await;
    }
}

fn number_after(text: &str, marker: &str) -> Option<u32> {
    let start = text.find(marker)? + marker.len();
    text[start..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
}
