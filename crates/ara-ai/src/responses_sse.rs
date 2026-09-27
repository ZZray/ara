//! Strict, bounded SSE framing for effect-bearing Responses tool calls.
//!
//! The existing Chat decoder retains its compatibility behavior. Responses
//! rejects malformed UTF-8 instead of replacing bytes inside tool arguments.

use crate::error::ProviderError;
use crate::sse::SseEvent;

const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;
const MAX_EVENTS_PER_FEED: usize = 4096;

#[derive(Default)]
pub(crate) struct ResponsesSseDecoder {
    line: Vec<u8>,
    data: Vec<String>,
    data_bytes: usize,
    event: Option<String>,
    pending_cr: bool,
}

impl ResponsesSseDecoder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, ProviderError> {
        let mut events = Vec::new();
        let mut start = 0;
        for (index, byte) in bytes.iter().copied().enumerate() {
            if self.pending_cr {
                self.pending_cr = false;
                if byte == b'\n' {
                    start = index + 1;
                    continue;
                }
            }
            if !matches!(byte, b'\n' | b'\r') {
                continue;
            }
            self.append_line_bytes(&bytes[start..index])?;
            if let Some(event) = self.finish_line()? {
                if events.len() >= MAX_EVENTS_PER_FEED {
                    return Err(ProviderError::Stream("Responses SSE chunk contains too many events".into()));
                }
                events.push(event);
            }
            self.pending_cr = byte == b'\r';
            start = index + 1;
        }
        self.append_line_bytes(&bytes[start..])?;
        Ok(events)
    }

    pub(crate) fn finish(&mut self) -> Result<Option<SseEvent>, ProviderError> {
        if !self.line.is_empty()
            && let Some(event) = self.finish_line()?
        {
            return Ok(Some(event));
        }
        Ok(self.dispatch())
    }

    fn append_line_bytes(&mut self, bytes: &[u8]) -> Result<(), ProviderError> {
        if self.line.len().saturating_add(bytes.len()) > MAX_LINE_BYTES {
            return Err(ProviderError::Stream("Responses SSE line exceeds size limit".into()));
        }
        self.line.extend_from_slice(bytes);
        Ok(())
    }

    fn finish_line(&mut self) -> Result<Option<SseEvent>, ProviderError> {
        let line = String::from_utf8(std::mem::take(&mut self.line))
            .map_err(|_| ProviderError::Stream("Responses SSE line is not valid UTF-8".into()))?;
        if line.is_empty() {
            return Ok(self.dispatch());
        }
        if line.starts_with(':') {
            return Ok(None);
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line.as_str(), ""),
        };
        match field {
            "data" => {
                let extra = value.len().saturating_add(usize::from(!self.data.is_empty()));
                if self.data_bytes.saturating_add(extra) > MAX_EVENT_BYTES {
                    return Err(ProviderError::Stream("Responses SSE event exceeds size limit".into()));
                }
                self.data_bytes += extra;
                self.data.push(value.to_owned());
            }
            "event" => self.event = Some(value.to_owned()),
            _ => {}
        }
        Ok(None)
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() {
            self.event = None;
            return None;
        }
        self.data_bytes = 0;
        Some(SseEvent { event: self.event.take(), data: std::mem::take(&mut self.data).join("\n") })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_utf8_and_crlf_are_exact() {
        let mut decoder = ResponsesSseDecoder::new();
        let source = "event: response.output_text.delta\r\ndata: {\"delta\":\"你好\"}\r\n\r\n".as_bytes();
        let split = source.windows(3).position(|window| window == "你".as_bytes()).unwrap() + 1;
        assert!(decoder.feed(&source[..split]).unwrap().is_empty());
        let events = decoder.feed(&source[split..]).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("response.output_text.delta"));
        assert_eq!(events[0].data, "{\"delta\":\"你好\"}");
    }

    #[test]
    fn malformed_utf8_fails_without_replacement() {
        let mut decoder = ResponsesSseDecoder::new();
        let err = decoder.feed(b"data: \xff\n\n").unwrap_err();
        assert!(matches!(err, ProviderError::Stream(_)));
    }

    #[test]
    fn oversize_line_fails_before_it_can_be_parsed_as_a_tool_call() {
        let mut decoder = ResponsesSseDecoder::new();
        let err = decoder.feed(&vec![b'x'; MAX_LINE_BYTES + 1]).unwrap_err();
        assert!(matches!(err, ProviderError::Stream(_)));
    }

    #[test]
    fn multiline_event_and_eof_dispatch_keep_exact_data() {
        let mut decoder = ResponsesSseDecoder::new();
        assert!(decoder.feed(b"data: one\ndata: two").unwrap().is_empty());
        assert_eq!(decoder.finish().unwrap(), Some(SseEvent { event: None, data: "one\ntwo".into() }));
    }

    #[test]
    fn one_transport_chunk_cannot_return_unbounded_events() {
        let mut decoder = ResponsesSseDecoder::new();
        let bytes = "data: {}\n\n".repeat(MAX_EVENTS_PER_FEED + 1);
        let err = decoder.feed(bytes.as_bytes()).unwrap_err();
        assert!(matches!(err, ProviderError::Stream(_)));
    }
}
