//! Incremental Server-Sent Events decoder (text/event-stream).

/// One dispatched SSE event.
#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Feeds raw bytes and yields complete events. Handles `\n`, `\r\n` and `\r`
/// line endings, multi-line `data:` fields, comments and chunk boundaries that
/// split lines or UTF-8 sequences.
#[derive(Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n' || b == b'\r') {
            // A lone '\r' at the end may be the first half of "\r\n".
            if self.buf[pos] == b'\r' && pos + 1 == self.buf.len() {
                break;
            }
            let line = String::from_utf8_lossy(&self.buf[..pos]).into_owned();
            let skip = if self.buf[pos] == b'\r' && self.buf.get(pos + 1) == Some(&b'\n') { 2 } else { 1 };
            self.buf.drain(..pos + skip);
            if let Some(event) = self.line(&line) {
                out.push(event);
            }
        }
        out
    }

    /// Dispatch any buffered event at end of stream.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.buf.is_empty() {
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned();
            let line = line.trim_end_matches('\r').to_string();
            if let Some(ev) = self.line(&line) {
                return Some(ev);
            }
        }
        self.dispatch()
    }

    fn line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.find(':') {
            Some(i) => {
                let v = &line[i + 1..];
                (&line[..i], v.strip_prefix(' ').unwrap_or(v))
            }
            None => (line, ""),
        };
        match field {
            "data" => self.data.push(value.to_string()),
            "event" => self.event = Some(value.to_string()),
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() {
            self.event = None;
            return None;
        }
        let data = std::mem::take(&mut self.data).join("\n");
        Some(SseEvent { event: self.event.take(), data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_across_chunks_and_line_endings() {
        let mut d = SseDecoder::new();
        assert!(d.feed(b"data: {\"a\"").is_empty());
        let evs = d.feed(b":1}\r\n\r\n: comment\n\ndata: x\rdata: y\r\r");
        // A trailing bare CR may be the first half of CRLF, so the second event waits.
        assert_eq!(evs, vec![SseEvent { event: None, data: "{\"a\":1}".into() }]);
        let evs = d.feed(b"event: e\ndata: [DONE]");
        assert_eq!(evs, vec![SseEvent { event: None, data: "x\ny".into() }]);
        assert_eq!(d.finish(), Some(SseEvent { event: Some("e".into()), data: "[DONE]".into() }));
    }

    #[test]
    fn utf8_split_is_reassembled() {
        let text = "data: 你好\n\n".as_bytes();
        let mut d = SseDecoder::new();
        assert!(d.feed(&text[..8]).is_empty());
        assert_eq!(d.feed(&text[8..])[0].data, "你好");
    }
}
