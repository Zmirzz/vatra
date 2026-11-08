/// Reads the `data:` lines of a `text/event-stream` body. Chunks may end anywhere.
#[derive(Default)]
pub struct SseParser {
    pending: Vec<u8>,
    data: Vec<String>,
}

impl SseParser {
    /// Feeds one chunk and returns the data of each event it completes.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(end) = self.pending.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(std::mem::take(&mut self.data).join("\n"));
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data.push(data.strip_prefix(' ').unwrap_or(data).to_string());
            }
        }
        events
    }

    /// Ends the body. An event without a blank line after it still counts.
    pub fn finish(mut self) -> Vec<String> {
        self.feed(b"\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::SseParser;

    #[test]
    fn events_split_across_chunks_are_joined() {
        let mut parser = SseParser::default();
        assert!(parser.feed(b"data: {\"a\"").is_empty());
        assert_eq!(parser.feed(b":1}\r\n\r\n: comment\nevent: x\ndata: second\n"), ["{\"a\":1}"]);
        assert_eq!(parser.finish(), ["second"]);
    }

    #[test]
    fn data_lines_of_one_event_are_joined_with_newlines() {
        let mut parser = SseParser::default();
        assert_eq!(parser.feed(b"data: x\ndata: y\n\n"), ["x\ny"]);
    }
}
