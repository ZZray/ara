//! Fixed OMP RPC stdin line decoding, without command dispatch.
//!
//! `readLines` splits on LF, yields the final unterminated line, and does not
//! impose an input size limit. `readRpcInputFrames` decodes each complete line
//! with replacement UTF-8, trims JavaScript whitespace, and continues after a
//! malformed JSON line. The caller owns the stdin handle before host setup.

use std::io;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

use crate::json::WireValue;

#[derive(Debug)]
pub enum InputItem {
    Frame(WireValue),
    ParseError(String),
}

pub struct RpcInputReader<R> {
    reader: BufReader<R>,
    line: Vec<u8>,
}

impl<R: AsyncRead + Unpin> RpcInputReader<R> {
    pub fn new(reader: R) -> Self {
        Self { reader: BufReader::new(reader), line: Vec::new() }
    }

    /// Return the next JSON value or recoverable parse error. Only I/O errors
    /// stop the stream; a malformed line never consumes a later command.
    pub async fn read_next(&mut self) -> io::Result<Option<InputItem>> {
        loop {
            // fill_buf is cancellation-safe. Copy and consume synchronously,
            // keeping an unfinished line in self.line across a dropped read.
            let available = self.reader.fill_buf().await?;
            let eof = available.is_empty();
            let newline = available.iter().position(|&byte| byte == b'\n');
            let take = newline.map_or(available.len(), |position| position + 1);
            self.line.extend_from_slice(&available[..take]);
            self.reader.consume(take);
            if !eof && newline.is_none() {
                continue;
            }
            if self.line.is_empty() {
                return Ok(None);
            }
            if self.line.last() == Some(&b'\n') {
                self.line.pop();
            }
            let line = std::mem::take(&mut self.line);
            // TextDecoder.decode(line) uses replacement rather than rejecting
            // malformed UTF-8. It is invoked separately for each complete line.
            let decoded = String::from_utf8_lossy(&line);
            let text = decoded.trim_matches(js_whitespace);
            if text.is_empty() {
                continue;
            }
            return Ok(Some(match WireValue::parse(text) {
                Ok(value) => InputItem::Frame(value),
                // Bun's diagnostic wording varies by runtime release. Keep the
                // OMP response prefix and source parser's concrete reason.
                Err(error) => InputItem::ParseError(format!("Failed to parse command: {error}")),
            }));
        }
    }
}

fn js_whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\t' | '\n' | '\u{000B}' | '\u{000C}' | '\r' | ' ' | '\u{00A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}
