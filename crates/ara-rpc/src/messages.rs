// Ported from OMP rpc-messages.ts at 596f2da7101178214aa27a753529d15e6b7ad91d.
// Copyright and MIT license are retained in THIRD_PARTY.md.
//! Pure pagination of one stable host-owned public message snapshot.

use crate::{WireString, WireValue};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use std::fmt;

pub const DEFAULT_RPC_MESSAGE_PAGE_LIMIT: usize = 100;
pub const MAX_RPC_MESSAGE_PAGE_LIMIT: usize = 256;
pub const MAX_RPC_MESSAGE_PAGE_BYTES: usize = 768 * 1024;
pub const MAX_RPC_MESSAGE_CURSOR_CHARS: usize = 2048;
pub const RPC_MESSAGES_PAGE_BUSY_ERROR: &str = "Cannot page messages while the session is changing";
pub const RPC_MESSAGES_PAGE_STALE_ERROR: &str = "RPC message cursor is stale";
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
const INVALID_CURSOR: &str = "Invalid RPC message cursor";
const INVALID_LIMIT: &str = "RPC message page limit must be between 1 and 256";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcMessageSnapshot {
    pub session_id: WireString,
    pub leaf_id: Option<WireString>,
    pub message_count: usize,
}

/// Preserve the inbound wire values until validating their JavaScript types.
/// An absent or null limit uses the default; a present null cursor is invalid.
#[derive(Clone, Copy, Debug, Default)]
pub struct RpcMessagesPageOptions<'a> {
    pub cursor: Option<&'a WireValue>,
    pub limit: Option<&'a WireValue>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RpcMessagesPage {
    pub messages: Vec<WireValue>,
    pub next_cursor: Option<WireString>,
    pub total_messages: usize,
}

impl From<RpcMessagesPage> for WireValue {
    fn from(page: RpcMessagesPage) -> Self {
        let mut fields = vec![("messages", WireValue::Array(page.messages))];
        if let Some(cursor) = page.next_cursor {
            fields.push(("nextCursor", WireValue::String(cursor)));
        }
        fields.push(("totalMessages", WireValue::Number(page.total_messages as f64)));
        WireValue::object(fields)
    }
}

impl RpcMessagesPage {
    pub fn into_wire(self) -> WireValue {
        self.into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcMessagesPageError {
    pub message: String,
    pub code: Option<&'static str>,
}

impl fmt::Display for RpcMessagesPageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RpcMessagesPageError {}

fn error(message: &str) -> RpcMessagesPageError {
    RpcMessagesPageError { message: message.into(), code: None }
}

struct Cursor {
    session_id: WireString,
    leaf_id: Option<WireString>,
    message_count: f64,
    offset: f64,
}

fn encode_cursor(snapshot: &RpcMessageSnapshot, offset: usize) -> WireString {
    let payload = WireValue::object(vec![
        ("version", WireValue::Number(1.0)),
        ("sessionId", WireValue::String(snapshot.session_id.clone())),
        ("leafId", snapshot.leaf_id.clone().map_or(WireValue::Null, WireValue::String)),
        ("messageCount", WireValue::Number(snapshot.message_count as f64)),
        ("offset", WireValue::Number(offset as f64)),
    ]);
    URL_SAFE_NO_PAD.encode(payload.stringify().as_bytes()).into()
}

fn safe_nonnegative_integer(value: Option<&WireValue>) -> Option<f64> {
    let number = value?.as_number()?;
    (number.is_finite() && number.fract() == 0.0 && (0.0..=MAX_SAFE_INTEGER).contains(&number)).then_some(number)
}

fn bounded_id(value: Option<&WireValue>) -> Option<WireString> {
    let value = value?.as_string()?;
    (!value.is_empty() && value.len() <= 256).then(|| value.clone())
}

fn decode_cursor(cursor: &WireValue) -> Result<Cursor, RpcMessagesPageError> {
    let cursor = cursor.as_string().ok_or_else(|| error(INVALID_CURSOR))?;
    if cursor.is_empty() || cursor.len() > MAX_RPC_MESSAGE_CURSOR_CHARS {
        return Err(error(INVALID_CURSOR));
    }
    let mut encoded = Vec::with_capacity(cursor.len());
    for &unit in cursor.units() {
        if !matches!(unit, 65..=90 | 97..=122 | 48..=57 | 95 | 45) {
            return Err(error(INVALID_CURSOR));
        }
        encoded.push(unit as u8);
    }
    let bytes = URL_SAFE_NO_PAD.decode(&encoded).map_err(|_| error(INVALID_CURSOR))?;
    if URL_SAFE_NO_PAD.encode(&bytes).as_bytes() != encoded {
        return Err(error(INVALID_CURSOR));
    }
    let decoded = std::str::from_utf8(&bytes).map_err(|_| error(INVALID_CURSOR))?;
    // TextDecoder("utf-8", { fatal: true }) consumes an initial UTF-8 BOM.
    let decoded = decoded.strip_prefix('\u{feff}').unwrap_or(decoded);
    let payload = WireValue::parse(decoded).map_err(|_| error(INVALID_CURSOR))?;
    if !payload.is_object() || payload.get("version").and_then(WireValue::as_number) != Some(1.0) {
        return Err(error(INVALID_CURSOR));
    }
    let session_id = bounded_id(payload.get("sessionId")).ok_or_else(|| error(INVALID_CURSOR))?;
    let leaf_id = match payload.get("leafId") {
        Some(WireValue::Null) => None,
        other => Some(bounded_id(other).ok_or_else(|| error(INVALID_CURSOR))?),
    };
    let message_count = safe_nonnegative_integer(payload.get("messageCount")).ok_or_else(|| error(INVALID_CURSOR))?;
    let offset = safe_nonnegative_integer(payload.get("offset")).ok_or_else(|| error(INVALID_CURSOR))?;
    if offset > message_count {
        return Err(error(INVALID_CURSOR));
    }
    Ok(Cursor { session_id, leaf_id, message_count, offset })
}

/// Port of fixed OMP `pageRpcMessages`. The first message is returned even if
/// it alone exceeds the page budget, allowing negotiated v2 framing to carry
/// it losslessly. The host owns the snapshot/change barrier.
pub fn page_rpc_messages(
    messages: &[WireValue],
    snapshot: &RpcMessageSnapshot,
    options: RpcMessagesPageOptions<'_>,
) -> Result<RpcMessagesPage, RpcMessagesPageError> {
    if snapshot.message_count != messages.len() {
        return Err(error("RPC message snapshot does not match current messages"));
    }
    let limit = match options.limit {
        None | Some(WireValue::Null) => DEFAULT_RPC_MESSAGE_PAGE_LIMIT,
        Some(value) => {
            let number = safe_nonnegative_integer(Some(value)).ok_or_else(|| error(INVALID_LIMIT))?;
            if !(1.0..=MAX_RPC_MESSAGE_PAGE_LIMIT as f64).contains(&number) {
                return Err(error(INVALID_LIMIT));
            }
            number as usize
        }
    };
    let mut offset = 0;
    if let Some(value) = options.cursor {
        let cursor = decode_cursor(value)?;
        if cursor.session_id != snapshot.session_id
            || cursor.leaf_id != snapshot.leaf_id
            || cursor.message_count != snapshot.message_count as f64
        {
            return Err(RpcMessagesPageError {
                message: RPC_MESSAGES_PAGE_STALE_ERROR.into(),
                code: Some("stale_cursor"),
            });
        }
        offset = cursor.offset as usize;
    }

    let mut page = Vec::new();
    let mut page_bytes = 2;
    while offset + page.len() < messages.len() && page.len() < limit {
        let message = &messages[offset + page.len()];
        let message_bytes = message.stringify().len() + usize::from(!page.is_empty());
        if !page.is_empty() && page_bytes + message_bytes > MAX_RPC_MESSAGE_PAGE_BYTES {
            break;
        }
        page.push(message.clone());
        page_bytes += message_bytes;
    }
    let next_offset = offset + page.len();
    Ok(RpcMessagesPage {
        messages: page,
        next_cursor: (next_offset < messages.len()).then(|| encode_cursor(snapshot, next_offset)),
        total_messages: messages.len(),
    })
}
