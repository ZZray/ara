// Ported from OMP rpc-frame.ts at 596f2da7101178214aa27a753529d15e6b7ad91d.
// Copyright and MIT license are retained in THIRD_PARTY.md.
use crate::{WireString, WireValue};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::borrow::Cow;
use std::fmt;

pub const MAX_RPC_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_RPC_REASSEMBLED_BYTES: usize = 64 * 1024 * 1024;
pub const RPC_CHUNK_PAYLOAD_BYTES: usize = 256 * 1024;
const PASSES: [(usize, usize, usize); 7] = [
    (256 * 1024, 512, 512),
    (64 * 1024, 256, 256),
    (16 * 1024, 128, 128),
    (4 * 1024, 64, 64),
    (1024, 32, 32),
    (256, 8, 16),
    (64, 1, 8),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError(pub String);
impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for RpcError {}
fn error(message: &str) -> RpcError {
    RpcError(message.into())
}
fn string(value: &str) -> WireValue {
    WireValue::String(value.into())
}
fn kind(frame: &WireValue, expected: &str) -> bool {
    frame.get("type").and_then(WireValue::as_string).is_some_and(|s| s.equals_ascii(expected))
}
fn shrink_string(value: &WireString, cap: usize) -> WireString {
    if value.len() <= cap {
        return value.clone();
    }
    let head = cap.saturating_sub(80);
    let mut result = value.slice_prefix(head);
    result.append_str(&format!("\n…[{} chars elided for RPC frame]", value.len() - head));
    result
}

fn shrink_value(value: &WireValue, pass: (usize, usize, usize)) -> WireValue {
    enum Task<'a> {
        Visit(&'a WireValue),
        Array(usize, usize),
        Object(Vec<WireString>, usize),
    }
    let mut tasks = vec![Task::Visit(value)];
    let mut results = Vec::new();
    while let Some(task) = tasks.pop() {
        match task {
            Task::Visit(WireValue::String(text)) => results.push(WireValue::String(shrink_string(text, pass.0))),
            Task::Visit(WireValue::Array(items)) => {
                let keep = items.len().min(pass.1);
                tasks.push(Task::Array(keep, items.len() - keep));
                tasks.extend(items[..keep].iter().rev().map(Task::Visit));
            }
            Task::Visit(object @ WireValue::Object(_)) => {
                let entries = object.entries().expect("object");
                let keep = entries.len().min(pass.2);
                let keys = entries[..keep].iter().map(|(key, _)| (*key).clone()).collect();
                tasks.push(Task::Object(keys, entries.len() - keep));
                tasks.extend(entries[..keep].iter().rev().map(|(_, item)| Task::Visit(item)));
            }
            Task::Visit(other) => results.push(other.clone()),
            Task::Array(keep, elided) => {
                let mut items = results.split_off(results.len() - keep);
                if elided > 0 {
                    items.push(string(&format!("…[{elided} items elided for RPC frame]")));
                }
                results.push(WireValue::Array(items));
            }
            Task::Object(keys, elided) => {
                let values = results.split_off(results.len() - keys.len());
                // Upstream assigns to {}, whose __proto__ setter creates no own
                // property. Preserve the serialized result without prototypes.
                let entries = keys.into_iter().zip(values).filter(|(key, _)| !key.equals_ascii("__proto__")).collect();
                let mut output = WireValue::Object(entries);
                if elided > 0 {
                    output.insert("rpcFrameElidedKeys", WireValue::Number(elided as f64));
                }
                results.push(output);
            }
        }
    }
    results.pop().expect("one shrink result")
}

fn snapshot(value: &WireValue) -> WireValue {
    WireValue::parse(&value.stringify()).expect("serialized JSON")
}
fn compact_terminal<'a>(frame: &'a WireValue, count: usize, messages: Option<&[WireValue]>) -> Cow<'a, WireValue> {
    let Some(items) = frame.get("messages").and_then(WireValue::as_array).filter(|_| kind(frame, "agent_end")) else {
        return Cow::Borrowed(frame);
    };
    let streamed = match messages {
        Some(messages) => messages.iter().zip(items).take_while(|(a, b)| a.deep_equal(&snapshot(b))).count(),
        None => {
            if count as u128 <= 9007199254740991 {
                count.min(items.len())
            } else {
                0
            }
        }
    };
    let WireValue::Object(entries) = frame else { unreachable!("terminal object") };
    let mut result = WireValue::Object(
        entries
            .iter()
            .map(|(key, value)| {
                let value = if key.equals_ascii("messages") {
                    WireValue::Array(items[streamed..].to_vec())
                } else {
                    value.clone()
                };
                (key.clone(), value)
            })
            .collect(),
    );
    result.insert("messageCount", WireValue::Number(items.len() as f64));
    Cow::Owned(result)
}
fn overflow(frame: &WireValue) -> WireValue {
    if kind(frame, "response") {
        let mut fields = Vec::new();
        if let Some(id) = frame.get("id").and_then(WireValue::as_string) {
            fields.push(("id", WireValue::String(shrink_string(id, 1024))));
        }
        fields.push(("type", string("response")));
        let command = frame
            .get("command")
            .and_then(WireValue::as_string)
            .map(|s| WireValue::String(shrink_string(s, 1024)))
            .unwrap_or_else(|| string("unknown"));
        fields.extend([
            ("command", command),
            ("success", WireValue::Bool(false)),
            ("error", string("RPC response exceeded the transport limit")),
        ]);
        return WireValue::object(fields);
    }
    if kind(frame, "agent_end") {
        return WireValue::object(vec![
            ("type", string("agent_end")),
            ("messages", WireValue::Array(Vec::new())),
            (
                "messageCount",
                frame
                    .get("messageCount")
                    .filter(|v| v.as_number().is_some())
                    .cloned()
                    .unwrap_or(WireValue::Number(0.0)),
            ),
        ]);
    }
    let mut fields = vec![("type", string("rpc_frame_error"))];
    if frame.is_object()
        && let Some(ty) = frame.get("type").and_then(WireValue::as_string)
    {
        fields.push(("originalType", WireValue::String(shrink_string(ty, 1024))));
    }
    fields.push(("error", string("RPC frame exceeded the transport limit")));
    WireValue::object(fields)
}
fn encode_v1(frame: &WireValue, json: String, count: usize, messages: Option<&[WireValue]>) -> String {
    if json.len() < MAX_RPC_FRAME_BYTES {
        return json + "\n";
    }
    if kind(frame, "response") {
        return overflow(frame).stringify() + "\n";
    }
    let compacted = compact_terminal(frame, count, messages);
    let json = compacted.stringify();
    if json.len() < MAX_RPC_FRAME_BYTES {
        return json + "\n";
    }
    for pass in PASSES {
        let json = shrink_value(&compacted, pass).stringify();
        if json.len() < MAX_RPC_FRAME_BYTES {
            return json + "\n";
        }
    }
    overflow(&compacted).stringify() + "\n"
}

/// Encode protocol v1, with the optional already-streamed terminal prefix.
pub fn encode_rpc_frame(
    frame: &WireValue,
    streamed_message_count: usize,
    streamed_messages: Option<&[WireValue]>,
) -> String {
    encode_v1(frame, frame.stringify(), streamed_message_count, streamed_messages)
}

/// Owned, single-consumption physical lines for one logical frame.
pub struct EncodedFrames {
    state: Lines,
}
enum Lines {
    Single(Option<String>),
    Chunks { bytes: Vec<u8>, chunk_id: String, index: usize, count: usize },
}
impl Iterator for EncodedFrames {
    type Item = String;
    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.state {
            Lines::Single(line) => line.take(),
            Lines::Chunks { bytes, chunk_id, index, count } => {
                if *index == *count {
                    return None;
                }
                let start = *index * RPC_CHUNK_PAYLOAD_BYTES;
                let data = STANDARD.encode(&bytes[start..(start + RPC_CHUNK_PAYLOAD_BYTES).min(bytes.len())]);
                let chunk = WireValue::object(vec![
                    ("type", string("rpc_chunk")),
                    ("chunkId", string(chunk_id)),
                    ("index", WireValue::Number(*index as f64)),
                    ("count", WireValue::Number(*count as f64)),
                    ("byteLength", WireValue::Number(bytes.len() as f64)),
                    ("data", string(&data)),
                ]);
                *index += 1;
                let line = chunk.stringify() + "\n";
                assert!(line.len() <= MAX_RPC_FRAME_BYTES, "RPC chunk exceeded the transport limit");
                Some(line)
            }
        }
    }
}
impl std::iter::FusedIterator for EncodedFrames {}
impl EncodedFrames {
    fn single(line: String) -> Self {
        Self { state: Lines::Single(Some(line)) }
    }
}

#[derive(Default)]
pub struct RpcFrameEncoder {
    streamed: Vec<WireValue>,
    protocol: u8,
    chunk_counter: u64,
}
impl RpcFrameEncoder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_protocol_version(&mut self, version: u8) -> Result<(), RpcError> {
        if version != 1 && version != 2 {
            return Err(error(&format!("Unsupported RPC protocol version: {version}")));
        }
        self.protocol = version;
        Ok(())
    }
    /// Bookkeeping is eager. Fully consume the returned iterator exactly once.
    pub fn encode_frames(&mut self, frame: &WireValue) -> EncodedFrames {
        if kind(frame, "agent_start") {
            self.streamed.clear();
        }
        let json = frame.stringify();
        let v2_message = if self.protocol == 2 && kind(frame, "message_end") && frame.get("message").is_some() {
            WireValue::parse(&json).expect("serialized frame").get("message").cloned()
        } else {
            None
        };
        let mut single = None;
        let frames = if self.protocol == 2 && json.len() >= MAX_RPC_FRAME_BYTES {
            let compacted = compact_terminal(frame, self.streamed.len(), Some(&self.streamed));
            let compacted_json = match &compacted {
                Cow::Borrowed(_) => json,
                Cow::Owned(_) => compacted.stringify(),
            };
            if compacted_json.len() >= MAX_RPC_FRAME_BYTES {
                self.chunk_counter += 1;
                if compacted_json.len() > MAX_RPC_REASSEMBLED_BYTES {
                    EncodedFrames::single(overflow(&compacted).stringify() + "\n")
                } else {
                    let bytes = compacted_json.into_bytes();
                    let count = bytes.len().div_ceil(RPC_CHUNK_PAYLOAD_BYTES);
                    EncodedFrames {
                        state: Lines::Chunks {
                            bytes,
                            count,
                            index: 0,
                            chunk_id: format!("rpc-{}", self.chunk_counter),
                        },
                    }
                }
            } else {
                let line = compacted_json + "\n";
                single = Some(line.clone());
                EncodedFrames::single(line)
            }
        } else {
            let line = encode_v1(frame, json, self.streamed.len(), Some(&self.streamed));
            single = Some(line.clone());
            EncodedFrames::single(line)
        };
        if kind(frame, "message_end") {
            if let Some(message) = v2_message {
                self.streamed.push(message);
            } else if let Some(encoded) = single.as_deref() {
                let encoded = WireValue::parse(encoded).expect("serialized frame");
                if kind(&encoded, "message_end")
                    && let Some(message) = encoded.get("message")
                {
                    self.streamed.push(message.clone());
                }
            }
        } else if kind(frame, "agent_end") && frame.get("willContinue") != Some(&WireValue::Bool(true)) {
            self.streamed.clear();
        }
        frames
    }
    pub fn encode(&mut self, frame: &WireValue) -> String {
        self.encode_frames(frame).collect()
    }
}

struct Pending {
    chunk_id: WireString,
    count: usize,
    byte_length: usize,
    next_index: usize,
    bytes: Vec<u8>,
}
/// Output-side decoder. Protocol errors are fatal to its client's connection.
#[derive(Default)]
pub struct RpcFrameDecoder {
    pending: Option<Pending>,
}
impl RpcFrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, value: WireValue) -> Result<Option<WireValue>, RpcError> {
        if !kind(&value, "rpc_chunk") {
            if self.pending.is_some() {
                return Err(error("rpc chunk sequence interrupted"));
            }
            if !value.is_object() {
                return Err(error("rpc frame must be an object"));
            }
            return Ok(Some(value));
        }
        fn integer(value: Option<&WireValue>) -> Option<usize> {
            let n = value?.as_number()?;
            (n.is_finite() && n.fract() == 0.0 && (0.0..=9007199254740991.0).contains(&n)).then_some(n as usize)
        }
        let chunk_id = value.get("chunkId").and_then(WireValue::as_string);
        let index = integer(value.get("index"));
        let count = integer(value.get("count"));
        let length = integer(value.get("byteLength"));
        let (Some(chunk_id), Some(index), Some(count), Some(length)) = (chunk_id, index, count, length) else {
            return Err(error("invalid rpc chunk metadata"));
        };
        if chunk_id.is_empty()
            || chunk_id.len() > 128
            || !(2..=MAX_RPC_REASSEMBLED_BYTES.div_ceil(RPC_CHUNK_PAYLOAD_BYTES)).contains(&count)
            || index >= count
            || !(MAX_RPC_FRAME_BYTES..=MAX_RPC_REASSEMBLED_BYTES).contains(&length)
        {
            return Err(error("invalid rpc chunk metadata"));
        }
        let data = value
            .get("data")
            .and_then(WireValue::as_string)
            .and_then(|s| s.to_utf8().ok())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| error("invalid rpc chunk data"))?;
        let bytes = STANDARD.decode(&data).map_err(|_| error("invalid rpc chunk data"))?;
        if STANDARD.encode(&bytes) != data {
            return Err(error("invalid rpc chunk data"));
        }
        if bytes.len() > RPC_CHUNK_PAYLOAD_BYTES {
            return Err(error("rpc chunk payload exceeds the transport limit"));
        }
        if self.pending.is_none() {
            if index != 0 {
                return Err(error("rpc chunk sequence must start at index 0"));
            }
            self.pending = Some(Pending {
                chunk_id: chunk_id.clone(),
                count,
                byte_length: length,
                next_index: 0,
                bytes: Vec::new(),
            });
        }
        let pending = self.pending.as_mut().expect("pending");
        if pending.chunk_id != *chunk_id
            || pending.count != count
            || pending.byte_length != length
            || pending.next_index != index
        {
            return Err(error("rpc chunk sequence mismatch"));
        }
        pending.bytes.extend(bytes);
        pending.next_index += 1;
        if pending.bytes.len() > pending.byte_length {
            return Err(error("rpc chunk sequence exceeds declared length"));
        }
        if pending.next_index < pending.count {
            return Ok(None);
        }
        if pending.bytes.len() != pending.byte_length {
            return Err(error("rpc chunk sequence length mismatch"));
        }
        let pending = self.pending.take().expect("pending");
        let text = std::str::from_utf8(&pending.bytes)
            .map_err(|_| error("The encoded data was not valid for encoding utf-8"))?;
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let frame = WireValue::parse(text).map_err(|e| error(&e.to_string()))?;
        if !frame.is_object() {
            return Err(error("rpc frame must be an object"));
        }
        Ok(Some(frame))
    }
}
