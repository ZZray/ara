// Ported from Oh My Pi 596f2da7101178214aa27a753529d15e6b7ad91d.
// MIT License
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use bytes::{Buf, BytesMut};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    io,
    path::{Component, Path, PathBuf},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

pub type Versions = HashMap<String, u64>;

#[derive(Clone, Debug)]
pub struct FormatResult {
    pub content: String,
    pub failed: bool,
    pub unsupported: bool,
    pub external_write_unknown: bool,
}

pub(super) type ReadStream = Box<dyn AsyncRead + Unpin + Send>;
pub(super) type WriteStream = Box<dyn AsyncWrite + Unpin + Send>;

pub(super) fn error(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
pub(super) fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "LSP operation cancelled")
}
pub(super) fn check_cancel(cancel: Option<&CancellationToken>) -> io::Result<()> {
    if cancel.is_some_and(CancellationToken::is_cancelled) { Err(cancelled()) } else { Ok(()) }
}
pub(super) async fn cancellation(cancel: Option<&CancellationToken>) {
    match cancel {
        Some(cancel) => cancel.cancelled().await,
        None => std::future::pending().await,
    }
}

pub(super) fn absolute(path: &Path) -> PathBuf {
    let path =
        if path.is_absolute() { path.to_owned() } else { std::env::current_dir().unwrap_or_default().join(path) };
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(component.as_os_str()),
        }
    }
    if let Ok(real) = std::fs::canonicalize(&out) {
        return plain_canonical(real);
    }
    let mut ancestor = out.as_path();
    let mut suffix = Vec::new();
    while let Some(parent) = ancestor.parent() {
        if let Some(name) = ancestor.file_name() {
            suffix.push(name.to_os_string());
        }
        ancestor = parent;
        if let Ok(real) = std::fs::canonicalize(ancestor) {
            let mut real = plain_canonical(real);
            for name in suffix.into_iter().rev() {
                real.push(name);
            }
            return real;
        }
    }
    out
}
fn plain_canonical(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let raw = path.to_string_lossy();
        if let Some(rest) = raw.strip_prefix("\\\\?\\UNC\\") {
            return PathBuf::from(format!("\\\\{rest}"));
        }
        if let Some(rest) = raw.strip_prefix("\\\\?\\") {
            return PathBuf::from(rest);
        }
    }
    path
}
pub(super) fn file_uri(path: &Path) -> String {
    reqwest::Url::from_file_path(absolute(path))
        .map(|v| v.to_string())
        .unwrap_or_else(|_| path.to_string_lossy().into())
}
pub(super) fn uri_path(uri: &str) -> PathBuf {
    if !uri.starts_with("file://") {
        return PathBuf::from(uri);
    }
    if !uri.contains(['#', '?']) {
        if let Ok(url) = reqwest::Url::parse(uri) {
            if let Ok(path) = url.to_file_path() {
                return path;
            }
        }
    }
    let raw = &uri[7..];
    let mut decoded = Vec::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut index = 0;
    let mut valid = true;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if let Some(hex) = bytes
                .get(index + 1..index + 3)
                .and_then(|v| std::str::from_utf8(v).ok())
                .and_then(|v| u8::from_str_radix(v, 16).ok())
            {
                decoded.push(hex);
                index += 3;
                continue;
            }
            valid = false;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    let text = if valid { String::from_utf8(decoded).unwrap_or_else(|_| raw.into()) } else { raw.into() };
    #[cfg(windows)]
    let text =
        if text.starts_with('/') && text.as_bytes().get(2) == Some(&b':') { text[1..].to_string() } else { text };
    PathBuf::from(text)
}
pub(super) fn uri_key(uri: &str) -> String {
    if !uri.starts_with("file://") {
        return uri.into();
    }
    let key = absolute(&uri_path(uri)).to_string_lossy().into_owned();
    if cfg!(windows) { key.to_lowercase() } else { key }
}

/// Incremental byte framing; preserve leftovers across the mux handshake.
pub(super) struct FramedReader {
    pub input: ReadStream,
    pending: BytesMut,
}
impl FramedReader {
    pub fn new(input: ReadStream) -> Self {
        Self { input, pending: BytesMut::new() }
    }
    pub async fn next(&mut self) -> io::Result<Option<Value>> {
        loop {
            if let Some(end) = self.pending.windows(4).position(|v| v == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&self.pending[..end]);
                let length = header.lines().find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.trim()
                        .eq_ignore_ascii_case("Content-Length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                });
                let Some(length) = length else {
                    self.pending.advance(end + 4);
                    continue;
                };
                let size = end
                    .checked_add(4)
                    .and_then(|v| v.checked_add(length))
                    .ok_or_else(|| error("LSP frame length overflow"))?;
                if self.pending.len() >= size {
                    let frame = self.pending.split_to(size);
                    match serde_json::from_slice(&frame[end + 4..]) {
                        Ok(message) => return Ok(Some(message)),
                        Err(_) => continue, // A malformed message does not discard later framed messages.
                    }
                }
            }
            if self.input.read_buf(&mut self.pending).await? == 0 {
                if self.pending.is_empty() {
                    return Ok(None);
                }
                return Err(error("LSP connection closed during a frame"));
            }
        }
    }
}
pub(super) async fn write_frame(output: &mut WriteStream, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    output.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes()).await?;
    output.write_all(&body).await?;
    output.flush().await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub(super) struct Position {
    pub line: usize,
    pub character: usize,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct Range {
    pub start: Position,
    pub end: Position,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TextEdit {
    pub range: Range,
    pub new_text: String,
    pub insert_text_format: Option<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Diagnostic {
    pub range: Range,
    #[serde(default)]
    pub severity: Option<u8>,
    pub message: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub code: Option<Value>,
}
#[derive(Clone, Debug)]
pub(super) struct PublishedDiagnostics {
    pub items: Vec<Diagnostic>,
    pub document_version: Option<u64>,
    pub publication: u64,
}

/// LSP positions are UTF-16 code units. Validate conflicts before mutating.
pub(super) fn apply_text_edits(content: &str, edits: Vec<TextEdit>) -> io::Result<String> {
    let mut indexed = edits.into_iter().enumerate().collect::<Vec<_>>();
    if indexed.iter().any(|(_, edit)| edit.insert_text_format == Some(2)) {
        return Err(error("snippet-formatted LSP edits are unsupported"));
    }
    indexed.sort_by(|(ai, a), (bi, b)| b.range.start.cmp(&a.range.start).then(bi.cmp(ai)));
    let mut unique: Vec<TextEdit> = Vec::new();
    for (_, edit) in indexed {
        if edit.range.start > edit.range.end {
            return Err(error("invalid LSP edit range"));
        }
        if unique.last().is_some_and(|prev| {
            edit.range.start != edit.range.end && prev.range == edit.range && prev.new_text == edit.new_text
        }) {
            continue;
        }
        if unique.last().is_some_and(|later| edit.range.end > later.range.start) {
            return Err(error("overlapping LSP edits"));
        }
        unique.push(edit);
    }
    let mut lines = content.split('\n').map(|line| line.encode_utf16().collect::<Vec<_>>()).collect::<Vec<_>>();
    for edit in unique {
        let Range { start, end } = edit.range;
        if end.line >= lines.len() {
            return Err(error("LSP edit line outside document"));
        }
        let start_col = start.character.min(lines[start.line].len());
        let end_col = end.character.min(lines[end.line].len());
        let mut changed = lines[start.line][..start_col].to_vec();
        changed.extend(edit.new_text.encode_utf16());
        changed.extend(&lines[end.line][end_col..]);
        let replacement = changed.split(|unit| *unit == 10).map(<[u16]>::to_vec).collect::<Vec<_>>();
        lines.splice(start.line..=end.line, replacement);
    }
    let units = lines.into_iter().enumerate().fold(Vec::new(), |mut acc, (index, line)| {
        if index > 0 {
            acc.push(10);
        }
        acc.extend(line);
        acc
    });
    String::from_utf16(&units).map_err(|_| error("LSP edit split a UTF-16 surrogate pair"))
}
