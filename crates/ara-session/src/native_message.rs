//! Canonical native message sources for the existing Session journal.
//!
//! A live source keeps its original graph handle and the current Host/runtime
//! encoder lease. Durable sources retain exact UTF-16 strings and Number bits.
//! The derived serde view is checked; it never becomes the canonical source.

use std::{fmt, io, sync::Arc};

use ara_rpc::value_heap::{ValueHandle, ValueHeap};
use ara_rpc::{WireString, WireValue};
use serde_json::Value;

use crate::BlobStore;

/// Narrow Host boundary. Implementations retain the current runtime lease.
/// `encode_entry` composes envelope metadata and the original `body` in that
/// same heap, then calls ActualPersistenceModule::line_for. `observe_body` is a
/// checked CURRENT observation for legacy/usage/recovery consumers, not a
/// cached typed DTO or an export used to replace the source graph. A Host that
/// cannot make that observation must return its actual error.
pub trait NativeJournalEncoder: Send + Sync {
    fn encode_entry(&self, envelope: &Value, body: &ValueHandle, blobs: Option<&BlobStore>) -> io::Result<WireString>;

    fn observe_body(&self, body: &ValueHandle) -> io::Result<WireValue>;

    /// Disposable model-message DTO for the existing compaction consumer.
    /// Hosts may select its known fields without exporting provider-private
    /// graph metadata. This never changes the strict raw observation or source.
    fn observe_compaction_message_body(&self, body: &ValueHandle) -> io::Result<WireValue> {
        self.observe_body(body)
    }

    /// Complete CURRENT Wire data observation for source comparison. Hosts may
    /// retain UTF-16 units and Number bits here without widening the explicit
    /// checked legacy observation. Native-only domains still return an error.
    fn observe_wire_body(&self, body: &ValueHandle) -> io::Result<WireValue> {
        self.observe_body(body)
    }

    /// Narrow current Get path for role/provider/model/usage/contextSnapshot.
    /// Absence differs from Null. Unrelated extensions are not traversed.
    fn observe_field(&self, body: &ValueHandle, path: &[&str]) -> io::Result<Option<WireValue>>;

    /// Current content/type/id/name observations. Never stringify arguments or
    /// executable values in order to decide pairing/recovery.
    fn observe_tool_calls(&self, body: &ValueHandle) -> io::Result<Vec<(WireString, WireString)>>;

    /// Prepare only an admitted toolResult content/prunedAt replacement in the
    /// original heap. The old root stays untouched until journal publication.
    fn prepare_tool_result_edit(
        &self,
        _body: &ValueHandle,
        _before: &Value,
        _after: &Value,
    ) -> io::Result<ValueHandle> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Host native tool-result edit is unavailable"))
    }
}

#[derive(Clone)]
pub enum NativeMessageSource {
    Live { body: ValueHandle, encoder: Arc<dyn NativeJournalEncoder> },
    Persisted { body: Arc<WireValue> },
}

impl fmt::Debug for NativeMessageSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Live { body, .. } => formatter.debug_struct("Live").field("body", body).finish_non_exhaustive(),
            Self::Persisted { body } => formatter.debug_struct("Persisted").field("body", body).finish(),
        }
    }
}

impl PartialEq for NativeMessageSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Live { body: left, encoder: a }, Self::Live { body: right, encoder: b }) => {
                left.same_identity(right) && Arc::ptr_eq(a, b)
            }
            (Self::Persisted { body: a }, Self::Persisted { body: b }) => a.deep_equal(b),
            _ => false,
        }
    }
}

impl NativeMessageSource {
    pub fn live(body: ValueHandle, encoder: Arc<dyn NativeJournalEncoder>) -> Self {
        Self::Live { body, encoder }
    }

    pub fn persisted(body: WireValue) -> Self {
        Self::Persisted { body: Arc::new(body) }
    }

    pub fn live_body(&self) -> Option<&ValueHandle> {
        match self {
            Self::Live { body, .. } => Some(body),
            Self::Persisted { .. } => None,
        }
    }

    /// Reopen imports durable data into a fresh realm. No callback, executable
    /// pointer or previous Host grant is decoded from the file.
    pub fn bind_current(&self, heap: &Arc<ValueHeap>, encoder: Arc<dyn NativeJournalEncoder>) -> io::Result<Self> {
        let Self::Persisted { body } = self else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "live source must retain its original realm"));
        };
        let handle = heap.import_strict(clone_wire(body)).map_err(|error| invalid(error.to_string()))?;
        Ok(Self::Live { body: handle, encoder })
    }

    /// Explicit derived observation. The canonical body/lease remains intact.
    pub fn observe(&self) -> io::Result<Arc<WireValue>> {
        match self {
            Self::Live { body, encoder } => encoder.observe_wire_body(body).map(Arc::new),
            Self::Persisted { body } => Ok(body.clone()),
        }
    }

    pub fn checked_body(&self) -> io::Result<Value> {
        match self {
            // Preserve the established strict legacy bridge even when the Host
            // has an exact Wire observer for source comparison.
            Self::Live { body, encoder } => checked_serde(&encoder.observe_body(body)?),
            Self::Persisted { body } => checked_serde(body),
        }
    }

    pub(crate) fn checked_entry(&self, envelope: &Value) -> io::Result<Value> {
        entry_from_body(envelope, &self.checked_body()?)
    }

    /// A disposable, checked model view for compaction only. Its original
    /// graph source and ordinary checked_entry contract remain unchanged.
    pub(crate) fn checked_compaction_message_entry(&self, envelope: &Value) -> io::Result<Value> {
        let body = match self {
            Self::Live { body, encoder } => checked_serde(&encoder.observe_compaction_message_body(body)?)?,
            Self::Persisted { body } => checked_serde(body)?,
        };
        entry_from_body(envelope, &body)
    }

    /// Only fields consumed by the existing local-reduction planner. The
    /// resulting observation never replaces the original graph or extensions.
    pub fn checked_reduction_body(&self) -> io::Result<Value> {
        if !matches!(self, Self::Live { .. }) {
            return self.checked_body();
        }
        let mut fields = Vec::new();
        let mut tool_result = false;
        for name in [
            "role",
            "content",
            "timestamp",
            "toolCallId",
            "toolName",
            "prunedAt",
            "useless",
            "isError",
            "details",
            "files",
            "customType",
            "attribution",
            "display",
            "stopReason",
        ] {
            let value = if name == "details" && tool_result {
                self.checked_reduction_tool_details()?
            } else {
                self.observe_field(&[name])?
            };
            if let Some(value) = value {
                if name == "role" {
                    tool_result = value.as_string().is_some_and(|role| role.equals_ascii("toolResult"));
                }
                fields.push((name.into(), value));
            }
        }
        checked_serde(&WireValue::Object(fields))
    }

    /// The local planner consumes only recovery-source metadata and the image
    /// slot. Native MCP details also contain optional Undefined members, which
    /// remain valid original graph data and are unrelated to this observation.
    /// Retain their original descriptors/edges in the Host's edited candidate.
    fn checked_reduction_tool_details(&self) -> io::Result<Option<WireValue>> {
        let mut source = Vec::new();
        for name in ["type", "value"] {
            if let Some(value) = self.observe_field(&["details", "meta", "source", name])? {
                source.push((name.into(), value));
            }
        }
        let mut details = Vec::new();
        if !source.is_empty() {
            details.push(("meta".into(), WireValue::Object(vec![("source".into(), WireValue::Object(source))])));
        }
        if let Some(images) = self.observe_field(&["details", "images"])? {
            details.push(("images".into(), images));
        }
        Ok((!details.is_empty()).then_some(WireValue::Object(details)))
    }

    pub(crate) fn checked_reduction_entry(&self, envelope: &Value) -> io::Result<Value> {
        entry_from_body(envelope, &self.checked_reduction_body()?)
    }

    pub fn observe_field(&self, path: &[&str]) -> io::Result<Option<WireValue>> {
        match self {
            Self::Live { body, encoder } => encoder.observe_field(body, path),
            Self::Persisted { body } => {
                let mut value = body.as_ref();
                for name in path {
                    let Some(next) = value.get(name) else { return Ok(None) };
                    value = next;
                }
                Ok(Some(clone_wire(value)))
            }
        }
    }

    pub fn observe_tool_calls(&self) -> io::Result<Vec<(WireString, WireString)>> {
        match self {
            Self::Live { body, encoder } => encoder.observe_tool_calls(body),
            Self::Persisted { body } => wire_tool_calls(body, true),
        }
    }

    /// Select the established custom envelope without exporting its payload.
    /// Reserved own fields stay inside the ordinary message slot. Checking an
    /// own descriptor never reads a colliding getter or its arbitrary value.
    pub(crate) fn custom_entry_timestamp(&self, role: &WireString) -> io::Result<Option<String>> {
        if !role.equals_ascii("custom") {
            return Ok(None);
        }
        for name in ["id", "parentId", "type"] {
            let collision = match self {
                Self::Live { body, .. } => body
                    .heap()
                    .has_own(body, &name.into(), &ara_rpc::value_heap::NoRuntime)
                    .map_err(|error| invalid(error.to_string()))?,
                Self::Persisted { body } => body.get(name).is_some(),
            };
            if collision {
                return Ok(None);
            }
        }
        let Some(WireValue::Number(timestamp)) = self.observe_field(&["timestamp"])? else {
            return Err(invalid("invalid custom message timestamp"));
        };
        if !timestamp.is_finite()
            || timestamp.fract() != 0.0
            || timestamp < i64::MIN as f64
            || timestamp >= i64::MAX as f64
        {
            return Err(invalid("invalid custom message timestamp"));
        }
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp as i64)
            .ok_or_else(|| invalid("invalid custom message timestamp"))?
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        Ok(Some(timestamp))
    }

    pub(crate) fn encoded_line(&self, envelope: &Value, blobs: Option<&BlobStore>) -> io::Result<String> {
        match self {
            Self::Live { body, encoder } => {
                let line = encoder.encode_entry(envelope, body, blobs)?;
                let line = line.to_utf8().map_err(|_| invalid("native encoder emitted unescaped UTF16 units"))?;
                // Validate a complete JSONL record before writing any bytes.
                if !line.ends_with('\n') || line[..line.len() - 1].contains('\n') {
                    return Err(invalid("native encoder did not emit exactly one JSONL line"));
                }
                let parsed = WireValue::parse(&line[..line.len() - 1]).map_err(|error| invalid(error.to_string()))?;
                validate_encoded_envelope(envelope, &parsed)?;
                Ok(line)
            }
            Self::Persisted { body } => {
                let entry = wire_entry_from_body(envelope, body)?;
                validate_encoded_envelope(envelope, &entry)?;
                Ok(format!("{}\n", entry.stringify()))
            }
        }
    }
}

pub(crate) fn wire_tool_calls(body: &WireValue, strict: bool) -> io::Result<Vec<(WireString, WireString)>> {
    let mut calls = Vec::new();
    if let Some(content) = body.get("content").and_then(WireValue::as_array) {
        for call in content {
            if !call.get("type").and_then(WireValue::as_string).is_some_and(|kind| kind.equals_ascii("toolCall")) {
                continue;
            }
            match (call.get("id").and_then(WireValue::as_string), call.get("name").and_then(WireValue::as_string)) {
                (Some(id), Some(name)) => calls.push((id.clone(), name.clone())),
                _ if strict => return Err(invalid("native tool call requires exact UTF16 id and name")),
                _ => {}
            }
        }
    }
    Ok(calls)
}

fn validate_encoded_envelope(envelope: &Value, encoded: &WireValue) -> io::Result<()> {
    let custom = envelope.get("type").and_then(Value::as_str) == Some("custom_message");
    if !encoded.is_object() || (!custom && encoded.get("message").is_none()) {
        return Err(invalid("native encoder omitted the message body"));
    }
    for key in ["type", "id", "parentId", "timestamp"] {
        let expected = envelope.get(key).ok_or_else(|| invalid(format!("missing native envelope {key}")))?;
        let expected = wire_from_serde(expected)?;
        if !encoded.get(key).is_some_and(|actual| actual.deep_equal(&expected)) {
            return Err(invalid(format!("native encoder changed journal envelope {key}")));
        }
    }
    Ok(())
}

const ENVELOPE_KEYS: [&str; 4] = ["type", "id", "parentId", "timestamp"];

/// Strip only a derived payload mirror. Custom payload fields live at the
/// entry top level on disk, while their original body remains a separate owner.
pub(crate) fn entry_envelope(entry: &Value) -> Value {
    let mut envelope = entry.clone();
    if let Some(fields) = envelope.as_object_mut() {
        if fields.get("type").and_then(Value::as_str) == Some("custom_message") {
            fields.retain(|key, _| ENVELOPE_KEYS.contains(&key.as_str()));
        } else {
            fields.remove("message");
        }
    }
    envelope
}

pub(crate) fn entry_from_body(envelope: &Value, body: &Value) -> io::Result<Value> {
    let entry = wire_entry_from_body(envelope, &wire_from_serde(body)?)?;
    checked_serde(&entry)
}

fn wire_entry_from_body(envelope: &Value, body: &WireValue) -> io::Result<WireValue> {
    let mut entry = wire_from_serde(&entry_envelope(envelope))?;
    if envelope.get("type").and_then(Value::as_str) == Some("custom_message") {
        let WireValue::Object(fields) = body else {
            return Err(invalid("native custom body must be an object"));
        };
        let WireValue::Object(entry_fields) = &mut entry else {
            return Err(invalid("native custom envelope must be an object"));
        };
        for (key, value) in fields {
            if key.equals_ascii("role") || key.equals_ascii("timestamp") {
                continue;
            }
            if ["type", "id", "parentId"].iter().any(|reserved| key.equals_ascii(reserved)) {
                return Err(invalid("native custom body collides with its journal envelope"));
            }
            if let Some((_, previous)) = entry_fields.iter_mut().find(|(candidate, _)| candidate == key) {
                *previous = clone_wire(value);
            } else {
                entry_fields.push((key.clone(), clone_wire(value)));
            }
        }
    } else {
        entry.insert("message", clone_wire(body));
    }
    Ok(entry)
}

/// Reconstruct the initiating custom body at the durable boundary. Payload
/// Number/UTF16 values are copied directly; journal identity remains outside it.
pub(crate) fn body_from_entry(entry: &WireValue) -> io::Result<Option<WireValue>> {
    let kind = entry.get("type").and_then(WireValue::as_string);
    if kind.is_some_and(|kind| kind.equals_ascii("message")) {
        return Ok(entry.get("message").map(clone_wire));
    }
    if !kind.is_some_and(|kind| kind.equals_ascii("custom_message")) {
        return Ok(None);
    }
    let timestamp = entry
        .get("timestamp")
        .and_then(WireValue::as_string)
        .ok_or_else(|| invalid("invalid custom entry timestamp"))?
        .to_utf8()
        .map_err(|_| invalid("invalid custom entry timestamp"))?;
    let timestamp = chrono::DateTime::parse_from_rfc3339(&timestamp)
        .map_err(|_| invalid("invalid custom entry timestamp"))?
        .timestamp_millis();
    let WireValue::Object(fields) = entry else { return Err(invalid("invalid custom entry")) };
    let mut body = WireValue::Object(
        fields
            .iter()
            .filter(|(key, _)| !ENVELOPE_KEYS.iter().any(|reserved| key.equals_ascii(reserved)))
            .map(|(key, value)| (key.clone(), clone_wire(value)))
            .collect(),
    );
    body.insert("role", WireValue::String("custom".into()));
    body.insert("timestamp", WireValue::Number(timestamp as f64));
    Ok(Some(body))
}

pub(crate) fn take_entry_source(entry: &mut WireValue) -> Option<NativeMessageSource> {
    let body = body_from_entry(entry).ok()??;
    let custom =
        entry.get("type").and_then(WireValue::as_string).is_some_and(|kind| kind.equals_ascii("custom_message"));
    let WireValue::Object(fields) = entry else { return None };
    if custom {
        fields.retain(|(key, _)| ENVELOPE_KEYS.iter().any(|reserved| key.equals_ascii(reserved)));
    } else {
        fields.retain(|(key, _)| !key.equals_ascii("message"));
    }
    Some(NativeMessageSource::persisted(body))
}

/// Body selected for the raw Agent converter. Native means the original graph
/// or original durable Wire source, never a Graph reconstructed from a DTO.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentMessageBody {
    Legacy(Value),
    Native(NativeMessageSource),
}

impl AgentMessageBody {
    pub fn checked_legacy(&self) -> io::Result<Value> {
        match self {
            Self::Legacy(body) => Ok(body.clone()),
            Self::Native(source) => source.checked_body(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentEntrySource {
    Legacy(Value),
    Native { envelope: Value, body: NativeMessageSource },
    Snapcompact { envelope: Value, preserve: crate::NativeSnapcompactArchiveSource },
}

impl AgentEntrySource {
    pub fn checked_legacy(&self) -> io::Result<Value> {
        match self {
            Self::Legacy(entry) => Ok(entry.clone()),
            Self::Native { envelope, body } => entry_from_body(envelope, &body.checked_body()?),
            Self::Snapcompact { envelope, preserve } => preserve.checked_entry(envelope),
        }
    }
}

/// Copy without stringifying: stringify normalizes -0 and non-finite values.
/// Work frames avoid making this clone's nesting depth a Rust stack boundary.
pub fn clone_wire(value: &WireValue) -> WireValue {
    enum Work<'a> {
        Visit(&'a WireValue),
        Array(usize),
        Object(Vec<WireString>),
    }
    let mut work = vec![Work::Visit(value)];
    let mut output = Vec::new();
    while let Some(next) = work.pop() {
        match next {
            Work::Visit(WireValue::Null) => output.push(WireValue::Null),
            Work::Visit(WireValue::Bool(value)) => output.push(WireValue::Bool(*value)),
            Work::Visit(WireValue::Number(value)) => output.push(WireValue::Number(*value)),
            Work::Visit(WireValue::String(value)) => output.push(WireValue::String(value.clone())),
            Work::Visit(WireValue::Array(values)) => {
                work.push(Work::Array(values.len()));
                work.extend(values.iter().rev().map(Work::Visit));
            }
            Work::Visit(WireValue::Object(values)) => {
                work.push(Work::Object(values.iter().map(|(key, _)| key.clone()).collect()));
                work.extend(values.iter().rev().map(|(_, value)| Work::Visit(value)));
            }
            Work::Array(count) => {
                let values = output.split_off(output.len() - count);
                output.push(WireValue::Array(values));
            }
            Work::Object(keys) => {
                let values = output.split_off(output.len() - keys.len());
                output.push(WireValue::Object(keys.into_iter().zip(values).collect()));
            }
        }
    }
    output.pop().expect("one wire source")
}

/// Exact checked projection. Invalid UTF-16, non-finite Number and values whose
/// Number bits cannot survive this representation produce an explicit error.
pub fn checked_serde(value: &WireValue) -> io::Result<Value> {
    enum Work<'a> {
        Visit(&'a WireValue),
        Array(usize),
        Object(Vec<String>),
    }
    let mut work = vec![Work::Visit(value)];
    let mut output = Vec::new();
    while let Some(next) = work.pop() {
        match next {
            Work::Visit(WireValue::Null) => output.push(Value::Null),
            Work::Visit(WireValue::Bool(value)) => output.push(Value::Bool(*value)),
            Work::Visit(WireValue::Number(value)) => {
                let candidate =
                    if *value >= 0.0 && *value < (u64::MAX as f64) && value.fract() == 0.0 && !value.is_sign_negative()
                    {
                        Some(serde_json::Number::from(*value as u64))
                    } else if *value < 0.0 && *value >= (i64::MIN as f64) && value.fract() == 0.0 {
                        Some(serde_json::Number::from(*value as i64))
                    } else {
                        serde_json::Number::from_f64(*value)
                    };
                let number = candidate
                    .filter(|number| number.as_f64().is_some_and(|projected| projected.to_bits() == value.to_bits()))
                    .ok_or_else(|| invalid("native Number is not representable in legacy context"))?;
                output.push(Value::Number(number));
            }
            Work::Visit(WireValue::String(value)) => output.push(Value::String(
                value.to_utf8().map_err(|_| invalid("native UTF16 string is not representable in legacy context"))?,
            )),
            Work::Visit(WireValue::Array(values)) => {
                work.push(Work::Array(values.len()));
                work.extend(values.iter().rev().map(Work::Visit));
            }
            Work::Visit(WireValue::Object(values)) => {
                let keys = values
                    .iter()
                    .map(|(key, _)| {
                        key.to_utf8().map_err(|_| invalid("native UTF16 key is not representable in legacy context"))
                    })
                    .collect::<io::Result<Vec<_>>>()?;
                work.push(Work::Object(keys));
                work.extend(values.iter().rev().map(|(_, value)| Work::Visit(value)));
            }
            Work::Array(count) => {
                let values = output.split_off(output.len() - count);
                output.push(Value::Array(values));
            }
            Work::Object(keys) => {
                let values = output.split_off(output.len() - keys.len());
                output.push(Value::Object(keys.into_iter().zip(values).collect()));
            }
        }
    }
    output.pop().ok_or_else(|| invalid("missing native projection"))
}

pub fn wire_from_serde(value: &Value) -> io::Result<WireValue> {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Number(number) => {
                let native = number.as_f64().ok_or_else(|| invalid("legacy number has no native Number"))?;
                let exact = if let Some(integer) = number.as_i64() {
                    native as i128 == i128::from(integer)
                } else if let Some(integer) = number.as_u64() {
                    native as u128 == u128::from(integer)
                } else {
                    native.is_finite()
                };
                if !exact {
                    return Err(invalid("legacy number cannot be represented exactly as native Number"));
                }
            }
            Value::Array(values) => pending.extend(values),
            Value::Object(values) => pending.extend(values.values()),
            _ => {}
        }
    }
    WireValue::parse(&value.to_string()).map_err(|error| invalid(error.to_string()))
}

impl NativeMessageSource {
    /// Map the checked journal entry back to its original message body before
    /// source validation. Custom payloads occupy entry top-level fields, while
    /// ordinary messages retain the message slot. Reuse the durable mapping.
    pub(crate) fn checked_edit_entry(&self, before: &Value, after: &Value) -> io::Result<Self> {
        fn body(entry: &Value) -> io::Result<Value> {
            let wire = wire_from_serde(entry)?;
            let body =
                body_from_entry(&wire)?.ok_or_else(|| invalid("native checked edit entry has no message body"))?;
            checked_serde(&body)
        }
        self.checked_edit(&body(before)?, &body(after)?)
    }

    /// Apply an admitted checked legacy edit to durable data. Unchanged Wire
    /// values retain their original keys/Number bits. The live Host prepares a
    /// separate current-heap candidate; journal publication owns adoption.
    pub(crate) fn checked_edit(&self, before: &Value, after: &Value) -> io::Result<Self> {
        if let Self::Live { body, encoder } = self {
            if !exact_serde_eq(&self.checked_reduction_body()?, before) {
                return Err(invalid("native checked edit lost its current source"));
            }
            let candidate = encoder.prepare_tool_result_edit(body, before, after)?;
            return Ok(Self::Live { body: candidate, encoder: encoder.clone() });
        }
        let Self::Persisted { body } = self else { unreachable!() };
        if !exact_serde_eq(&checked_serde(body)?, before) {
            return Err(invalid("native checked edit lost its current source"));
        }
        fn edit(original: &WireValue, before: &Value, after: &Value) -> io::Result<WireValue> {
            enum Work<'a> {
                Visit(&'a WireValue, &'a Value, &'a Value),
                New(&'a Value),
                Array(usize),
                Object(Vec<WireString>),
            }
            let mut work = vec![Work::Visit(original, before, after)];
            let mut output = Vec::new();
            while let Some(next) = work.pop() {
                match next {
                    Work::Visit(original, before, after) if exact_serde_eq(before, after) => {
                        output.push(clone_wire(original))
                    }
                    Work::Visit(WireValue::Object(original), Value::Object(before), Value::Object(after)) => {
                        let mut keys = Vec::new();
                        let mut children = Vec::new();
                        for (key, value) in original {
                            let name =
                                key.to_utf8().map_err(|_| invalid("native checked object key is not representable"))?;
                            if let Some(next) = after.get(&name) {
                                let previous = before
                                    .get(&name)
                                    .ok_or_else(|| invalid("native checked source key disappeared"))?;
                                keys.push(key.clone());
                                children.push(Work::Visit(value, previous, next));
                            }
                        }
                        for (key, value) in after {
                            if !before.contains_key(key) {
                                keys.push(key.as_str().into());
                                children.push(Work::New(value));
                            }
                        }
                        work.push(Work::Object(keys));
                        work.extend(children.into_iter().rev());
                    }
                    Work::Visit(WireValue::Array(original), Value::Array(before), Value::Array(after)) => {
                        work.push(Work::Array(after.len()));
                        for (index, value) in after.iter().enumerate().rev() {
                            work.push(match (original.get(index), before.get(index)) {
                                (Some(source), Some(previous)) => Work::Visit(source, previous, value),
                                _ => Work::New(value),
                            });
                        }
                    }
                    Work::Visit(_, _, after) | Work::New(after) => output.push(wire_from_serde(after)?),
                    Work::Array(count) => {
                        let values = output.split_off(output.len() - count);
                        output.push(WireValue::Array(values));
                    }
                    Work::Object(keys) => {
                        let values = output.split_off(output.len() - keys.len());
                        output.push(WireValue::Object(keys.into_iter().zip(values).collect()));
                    }
                }
            }
            output.pop().ok_or_else(|| invalid("missing native checked edit"))
        }
        Ok(Self::persisted(edit(body, before, after)?))
    }
}

pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn exact_serde_eq(left: &Value, right: &Value) -> bool {
    let mut work = vec![(left, right)];
    while let Some((left, right)) = work.pop() {
        match (left, right) {
            (Value::Number(left), Value::Number(right)) => {
                if left.as_f64().zip(right.as_f64()).is_none_or(|(left, right)| left.to_bits() != right.to_bits()) {
                    return false;
                }
            }
            (Value::Array(left), Value::Array(right)) if left.len() == right.len() => {
                work.extend(left.iter().zip(right))
            }
            (Value::Object(left), Value::Object(right)) if left.len() == right.len() => {
                for (key, value) in left {
                    let Some(other) = right.get(key) else { return false };
                    work.push((value, other));
                }
            }
            _ if left == right => {}
            _ => return false,
        }
    }
    true
}
