//! Graph history projection, fixed OMP 596f2da (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//!
//! Only newly synthesized request objects are allocated here. Original content,
//! arguments, result metadata, and symbols remain in their Session lineage.
use crate::Model;
use crate::actual_provider::{GraphBuilder, concat, trim_empty};
use ara_rpc::value_heap::{CompiledFunctionForm, CompiledFunctionSpec, ValueHeap, ValueKind, ValueRuntime};
use ara_rpc::{
    WireString,
    value_heap::{PropertyKey, ValueError, ValueHandle, ValueSnapshot},
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

type Result<T> = std::result::Result<T, ValueError>;

fn string(builder: &GraphBuilder, value: &ValueHandle, key: &str) -> Result<WireString> {
    builder.string(&builder.get(value, key)?)
}
fn equal(builder: &GraphBuilder, value: &ValueHandle, key: &str, expected: &str) -> Result<bool> {
    Ok(
        matches!(builder.0.heap().snapshot(&builder.get(value, key)?)?, ValueSnapshot::String(value) if value.equals_ascii(expected)),
    )
}
fn content(builder: &GraphBuilder, message: &ValueHandle) -> Result<Vec<ValueHandle>> {
    builder.members(&builder.get(message, "content")?)
}
fn array(builder: &GraphBuilder, values: impl IntoIterator<Item = ValueHandle>) -> Result<ValueHandle> {
    let result = builder.0.heap().array_with_length(0)?;
    for value in values {
        builder.push(&result, value)?;
    }
    Ok(result)
}
fn copy_with(builder: &GraphBuilder, source: &ValueHandle, key: &str, value: ValueHandle) -> Result<ValueHandle> {
    let copy = builder.0.heap().spread(source, builder.0.runtime())?;
    builder.define(&copy, key, value)?;
    Ok(copy)
}
fn malformed_field(builder: &GraphBuilder, block: &ValueHandle, key: &str) -> Result<bool> {
    let field = builder.get(block, key)?;
    if !builder.0.heap().truthy(&field)? {
        return Ok(true);
    }
    Ok(trim_empty(&builder.string(&field)?))
}
fn malformed(builder: &GraphBuilder, block: &ValueHandle) -> Result<bool> {
    Ok(malformed_field(builder, block, "id")? || malformed_field(builder, block, "name")?)
}
fn same_map_key(builder: &GraphBuilder, left: &ValueHandle, right: &ValueHandle) -> Result<bool> {
    if let (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) =
        (builder.0.heap().snapshot(left)?, builder.0.heap().snapshot(right)?)
    {
        if left == 0.0 && right == 0.0 {
            return Ok(true);
        }
    }
    builder.0.heap().object_is(left, right)
}
fn queue_index(
    builder: &GraphBuilder,
    queues: &[(ValueHandle, VecDeque<bool>)],
    id: &ValueHandle,
) -> Result<Option<usize>> {
    for (index, (key, _)) in queues.iter().enumerate() {
        if same_map_key(builder, key, id)? {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

/// Fixed transform-messages.ts 282–340. Queues describe concrete occurrences
/// within a contiguous assistant/result window, rather than globally banned ids.
fn sanitize_malformed(builder: &GraphBuilder, messages: &[ValueHandle]) -> Result<Vec<ValueHandle>> {
    let mut found = false;
    'scan: for message in messages {
        if !equal(builder, message, "role", "assistant")? {
            continue;
        }
        for block in content(builder, message)? {
            if equal(builder, &block, "type", "toolCall")? && malformed(builder, &block)? {
                found = true;
                break 'scan;
            }
        }
    }
    if !found {
        return Ok(messages.to_vec());
    }
    // A malformed undefined/null/empty id is still a Native Map key. Do not
    // coerce it to a string before matching its concrete rejection result.
    let mut queues: Vec<(ValueHandle, VecDeque<bool>)> = Vec::new();
    let mut result = Vec::new();
    for message in messages {
        if equal(builder, message, "role", "assistant")? {
            queues.clear();
            let original = content(builder, message)?;
            let mut filtered = Vec::new();
            for block in &original {
                if equal(builder, block, "type", "toolCall")? {
                    let drop = malformed(builder, block)?;
                    let id = builder.get(block, "id")?;
                    if let Some(index) = queue_index(builder, &queues, &id)? {
                        queues[index].1.push_back(drop);
                    } else {
                        queues.push((id, VecDeque::from([drop])));
                    }
                    if drop {
                        continue;
                    }
                }
                filtered.push(block.clone());
            }
            if filtered.is_empty() {
                continue;
            }
            result.push(if filtered.len() == original.len() {
                message.clone()
            } else {
                copy_with(builder, message, "content", array(builder, filtered)?)?
            });
        } else if equal(builder, message, "role", "toolResult")? {
            let id = builder.get(message, "toolCallId")?;
            let drop = if let Some(index) = queue_index(builder, &queues, &id)? {
                let drop = queues[index].1.pop_front().unwrap_or(false);
                if queues[index].1.is_empty() {
                    queues.remove(index);
                }
                drop
            } else {
                false
            };
            if !drop {
                result.push(message.clone());
            }
        } else {
            queues.clear();
            result.push(message.clone());
        }
    }
    Ok(result)
}

fn responses_family(api: &WireString) -> bool {
    ["openai-responses", "openai-codex-responses", "azure-openai-responses"].iter().any(|value| api.equals_ascii(value))
}
fn responses_message(builder: &GraphBuilder, message: &ValueHandle) -> Result<bool> {
    Ok(
        matches!(builder.0.heap().snapshot(&builder.get(message, "api")?)?, ValueSnapshot::String(api) if responses_family(&api)),
    )
}
fn call_component(id: &WireString) -> WireString {
    match id.units().iter().position(|unit| *unit == b'|' as u16) {
        Some(position) if position > 0 => id.slice_prefix(position),
        _ => id.clone(),
    }
}
#[derive(Default)]
struct OriginScope {
    responses: HashSet<WireString>,
    opaque: HashSet<WireString>,
}
impl OriginScope {
    fn collect(builder: &GraphBuilder, messages: &[ValueHandle]) -> Result<Self> {
        let mut result = Self::default();
        for message in messages {
            if !equal(builder, message, "role", "assistant")? {
                continue;
            }
            let responses = responses_message(builder, message)?;
            for block in content(builder, message)? {
                if !equal(builder, &block, "type", "toolCall")? {
                    continue;
                }
                let id = string(builder, &block, "id")?;
                if responses {
                    result.responses.insert(call_component(&id));
                } else if id.units().contains(&(b'|' as u16)) {
                    result.opaque.insert(id);
                }
            }
        }
        Ok(result)
    }
    fn key(&self, id: &WireString) -> WireString {
        if self.opaque.contains(id) {
            return id.clone();
        }
        let prefix = call_component(id);
        if prefix != *id && self.responses.contains(&prefix) { prefix } else { id.clone() }
    }
}
fn duplicate_id(id: &WireString, suffix: &str, max: usize) -> WireString {
    let suffix_units = suffix.encode_utf16().collect::<Vec<_>>();
    let mut units = Vec::new();
    for (index, segment) in id.units().split(|unit| *unit == b'|' as u16).enumerate() {
        if index > 0 {
            units.push(b'|' as u16);
        }
        let keep = if segment.len() + suffix_units.len() <= max {
            segment.len()
        } else {
            max.saturating_sub(suffix_units.len())
        };
        units.extend_from_slice(&segment[..keep]);
        units.extend_from_slice(&suffix_units);
    }
    WireString::from_units(units)
}

/// Fixed deduplicateToolCallIds 153–248: source handles are retained when no
/// rewrite occurs, and each reused occurrence enqueues its own result rewrite.
fn deduplicate(
    builder: &GraphBuilder,
    messages: Vec<ValueHandle>,
    origin: &OriginScope,
    max: usize,
    suffix_prefix: &str,
) -> Result<Vec<ValueHandle>> {
    let mut seen: HashMap<WireString, usize> = HashMap::new();
    let mut pending: HashMap<WireString, VecDeque<Option<WireString>>> = HashMap::new();
    let mut result = Vec::with_capacity(messages.len());
    for message in messages {
        if equal(builder, &message, "role", "toolResult")? {
            let key = origin.key(&string(builder, &message, "toolCallId")?);
            let rewrite = pending.get_mut(&key).and_then(VecDeque::pop_front).flatten();
            if pending.get(&key).is_some_and(VecDeque::is_empty) {
                pending.remove(&key);
            }
            result.push(match rewrite {
                Some(id) => copy_with(builder, &message, "toolCallId", builder.text(id)?)?,
                None => message,
            });
            continue;
        }
        if !equal(builder, &message, "role", "assistant")? {
            result.push(message);
            continue;
        }
        let mut touched = HashSet::new();
        let mut changed = false;
        let mut rewritten = Vec::new();
        for block in content(builder, &message)? {
            if !equal(builder, &block, "type", "toolCall")? {
                rewritten.push(block);
                continue;
            }
            let id = string(builder, &block, "id")?;
            let key = origin.key(&id);
            if touched.insert(key.clone()) {
                pending.remove(&key);
            }
            let count = seen.get(&key).copied().unwrap_or(0);
            if count == 0 {
                seen.insert(key.clone(), 1);
                pending.entry(key).or_default().push_back(None);
                rewritten.push(block);
                continue;
            }
            let mut index = count;
            let mut replacement = duplicate_id(&id, &format!("{suffix_prefix}{index}"), max);
            while seen.contains_key(&origin.key(&replacement)) {
                index += 1;
                replacement = duplicate_id(&id, &format!("{suffix_prefix}{index}"), max);
            }
            seen.insert(key.clone(), index + 1);
            seen.insert(origin.key(&replacement), 1);
            pending.entry(key).or_default().push_back(Some(replacement.clone()));
            rewritten.push(copy_with(builder, &block, "id", builder.text(replacement)?)?);
            changed = true;
        }
        result.push(if changed {
            copy_with(builder, &message, "content", array(builder, rewritten)?)?
        } else {
            message
        });
    }
    Ok(result)
}

pub(crate) fn truncated_thinking_only(builder: &GraphBuilder, message: &ValueHandle) -> Result<bool> {
    let truncated = equal(builder, message, "stopReason", "length")?
        || equal(builder, message, "stopReason", "error")?
        || equal(builder, message, "stopReason", "aborted")?;
    if !truncated {
        return Ok(false);
    }
    for block in content(builder, message)? {
        if equal(builder, &block, "type", "toolCall")? || equal(builder, &block, "type", "text")? {
            return Ok(false);
        }
    }
    Ok(true)
}

struct IndexedResult {
    index: usize,
    message: ValueHandle,
    consumed: bool,
}
struct PendingCall {
    block: ValueHandle,
    key: WireString,
}
struct Repair<'a> {
    builder: &'a GraphBuilder,
    origin: &'a OriginScope,
    real: HashMap<WireString, Vec<IndexedResult>>,
    emitted: HashSet<WireString>,
    pending: Vec<PendingCall>,
    pending_after: usize,
    aborted: Vec<PendingCall>,
    aborted_after: usize,
    aborted_timestamp: Option<ValueHandle>,
    result: Vec<ValueHandle>,
}
impl Repair<'_> {
    fn take_real(&mut self, key: &WireString, after: usize) -> Option<ValueHandle> {
        let entry = self.real.get_mut(key)?.iter_mut().find(|entry| !entry.consumed && entry.index > after)?;
        entry.consumed = true;
        Some(entry.message.clone())
    }
    fn synthetic(&self, block: &ValueHandle, text: &str, timestamp: &ValueHandle) -> Result<ValueHandle> {
        let text_block =
            self.builder.object([("type", self.builder.text("text")?), ("text", self.builder.text(text)?)])?;
        self.builder.object([
            ("role", self.builder.text("toolResult")?),
            ("toolCallId", self.builder.get(block, "id")?),
            ("toolName", self.builder.get(block, "name")?),
            ("content", array(self.builder, [text_block])?),
            ("isError", self.builder.0.heap().boolean(true)?),
            ("timestamp", timestamp.clone()),
        ])
    }
    fn flush(&mut self, timestamp: &ValueHandle) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        for call in std::mem::take(&mut self.pending) {
            if self.emitted.contains(&call.key) {
                continue;
            }
            let result = match self.take_real(&call.key, self.pending_after) {
                Some(real) => real,
                None => self.synthetic(&call.block, "No result provided", timestamp)?,
            };
            self.result.push(result);
            self.emitted.insert(call.key);
        }
        Ok(())
    }
    fn flush_aborted(&mut self) -> Result<()> {
        let Some(timestamp) = self.aborted_timestamp.take() else { return Ok(()) };
        for call in std::mem::take(&mut self.aborted) {
            if self.emitted.contains(&call.key) {
                continue;
            }
            let result = match self.take_real(&call.key, self.aborted_after) {
                Some(real) => real,
                None => self.synthetic(&call.block, "aborted", &timestamp)?,
            };
            self.result.push(result);
            self.emitted.insert(call.key);
        }
        Ok(())
    }
    fn calls(&self, message: &ValueHandle) -> Result<Vec<PendingCall>> {
        let mut calls = Vec::new();
        for block in content(self.builder, message)? {
            if equal(self.builder, &block, "type", "toolCall")? {
                calls.push(PendingCall { key: self.origin.key(&string(self.builder, &block, "id")?), block });
            }
        }
        Ok(calls)
    }
    fn timestamp(&self, message: &ValueHandle) -> Result<ValueHandle> {
        // Fixed source uses `"timestamp" in msg` before the two independent
        // timestamp Gets (`typeof` test, then conditional value).
        if self.builder.0.heap().has(message, &PropertyKey::from("timestamp"), self.builder.0.runtime())? {
            let timestamp = self.builder.get(message, "timestamp")?;
            if matches!(self.builder.0.heap().snapshot(&timestamp)?, ValueSnapshot::Number(_)) {
                return self.builder.get(message, "timestamp");
            }
        }
        self.builder.0.heap().number(crate::types::now_ms() as f64)
    }
    fn orphan(&self, message: &ValueHandle, timestamp: ValueHandle) -> Result<Option<ValueHandle>> {
        let mut text = WireString::from("");
        let mut count = 0;
        for block in content(self.builder, message)? {
            if !equal(self.builder, &block, "type", "text")? {
                continue;
            }
            let part = string(self.builder, &block, "text")?;
            if trim_empty(&part) {
                continue;
            }
            if count > 0 {
                text.append_str("\n");
            }
            text = concat(&text, &part);
            count += 1;
        }
        if count == 0 {
            return Ok(None);
        }
        let is_error = self.builder.0.heap().truthy(&self.builder.get(message, "isError")?)?;
        let name =
            self.builder.0.heap().to_string(&self.builder.get(message, "toolName")?, self.builder.0.runtime())?;
        let id =
            self.builder.0.heap().to_string(&self.builder.get(message, "toolCallId")?, self.builder.0.runtime())?;
        let mut opening = WireString::from("<stale-tool-result tool=\"");
        opening = concat(&opening, &name);
        opening.append_str("\" id=\"");
        opening = concat(&opening, &id);
        opening.append_str("\"");
        if is_error {
            opening.append_str(" is-error=\"true\"");
        }
        opening.append_str(">\n");
        let mut note = concat(&opening, &text);
        note.append_str("\n</stale-tool-result>");
        Ok(Some(self.builder.object([
            ("role", self.builder.text("user")?),
            ("content", self.builder.text(note)?),
            ("timestamp", timestamp),
        ])?))
    }
}

/// The normalized list and the pre-normalization list must have the same
/// positional length. Truncated-thinking-only checks consult the original.
fn repair_results(
    builder: &GraphBuilder,
    original: &[ValueHandle],
    messages: Vec<ValueHandle>,
    origin: &OriginScope,
) -> Result<Vec<ValueHandle>> {
    let mut real: HashMap<WireString, Vec<IndexedResult>> = HashMap::new();
    let mut valid = HashSet::new();
    for (index, message) in messages.iter().enumerate() {
        if equal(builder, message, "role", "toolResult")? {
            real.entry(origin.key(&string(builder, message, "toolCallId")?)).or_default().push(IndexedResult {
                index,
                message: message.clone(),
                consumed: false,
            });
        }
    }
    for message in &messages {
        if !equal(builder, message, "role", "assistant")? {
            continue;
        }
        for block in content(builder, message)? {
            if equal(builder, &block, "type", "toolCall")? {
                valid.insert(origin.key(&string(builder, &block, "id")?));
            }
        }
    }
    let mut repair = Repair {
        builder,
        origin,
        real,
        emitted: HashSet::new(),
        pending: Vec::new(),
        pending_after: 0,
        aborted: Vec::new(),
        aborted_after: 0,
        aborted_timestamp: None,
        result: Vec::new(),
    };
    for (index, message) in messages.into_iter().enumerate() {
        let timestamp = repair.timestamp(&message)?;
        if equal(builder, &message, "role", "assistant")? {
            repair.flush(&timestamp)?;
            repair.flush_aborted()?;
            if equal(builder, &original[index], "role", "assistant")?
                && truncated_thinking_only(builder, &original[index])?
            {
                continue;
            }
            let calls = repair.calls(&message)?;
            if equal(builder, &message, "stopReason", "error")? || equal(builder, &message, "stopReason", "aborted")? {
                // Native Map preserves insertion order and updates an existing
                // duplicate key without moving it to the end.
                repair.aborted.clear();
                for call in calls {
                    if let Some(existing) = repair.aborted.iter_mut().find(|existing| existing.key == call.key) {
                        *existing = call;
                    } else {
                        repair.aborted.push(call);
                    }
                }
                let raw_timestamp = builder.get(&message, "timestamp")?;
                repair.aborted_timestamp =
                    if matches!(builder.0.heap().snapshot(&raw_timestamp)?, ValueSnapshot::Undefined) {
                        None
                    } else {
                        Some(raw_timestamp)
                    };
                repair.aborted_after = index;
            } else if !calls.is_empty() {
                repair.pending = calls;
                repair.pending_after = index;
            }
            repair.result.push(message);
        } else if equal(builder, &message, "role", "toolResult")? {
            let key = origin.key(&string(builder, &message, "toolCallId")?);
            if repair.emitted.contains(&key) {
                continue;
            }
            if let Some(position) = repair.aborted.iter().position(|call| call.key == key) {
                repair.aborted.remove(position);
                repair.emitted.insert(key);
                repair.result.push(message);
                continue;
            }
            if repair.pending.iter().any(|call| call.key == key) {
                repair.emitted.insert(key);
                repair.result.push(message);
                continue;
            }
            if !valid.contains(&key) {
                let unresolved = repair.pending.iter().any(|call| !repair.emitted.contains(&call.key));
                if unresolved || !repair.aborted.is_empty() {
                    continue;
                }
                if let Some(note) = repair.orphan(&message, timestamp)? {
                    repair.result.push(note);
                }
            }
        } else {
            repair.flush(&timestamp)?;
            repair.flush_aborted()?;
            repair.result.push(message);
        }
    }
    repair.flush(&builder.0.heap().number(crate::types::now_ms() as f64)?)?;
    repair.flush_aborted()?;
    Ok(repair.result)
}

/// The representation-sensitive normalization callback is local AI code; it
/// receives original graph handles, never a typed AssistantMessage snapshot.
pub(crate) fn transform_messages(
    builder: &GraphBuilder,
    messages: &[ValueHandle],
    max_id: usize,
    suffix_prefix: &str,
    normalize: impl FnOnce(&[ValueHandle]) -> Result<Vec<ValueHandle>>,
) -> Result<Vec<ValueHandle>> {
    let sanitized = sanitize_malformed(builder, messages)?;
    let normalized = normalize(&sanitized)?;
    if normalized.len() != sanitized.len() {
        return Err(ValueError::UnsupportedOperation("History normalization must retain positional turns"));
    }
    let origin = OriginScope::collect(builder, &normalized)?;
    let deduplicated = deduplicate(builder, normalized, &origin, max_id, suffix_prefix)?;
    repair_results(builder, &sanitized, deduplicated, &origin)
}

pub(crate) fn demoted_thinking_key(builder: &GraphBuilder) -> Result<PropertyKey> {
    builder.0.heap().module_symbol("ara-ai.block-symbols", "provider.block.demotedThinking")
}

/// Request metadata owned by the provider/Host. Prefix binding describes the
/// selected model's signature contract; it does not carry product state.
pub(crate) struct OpenAIHistoryPolicy {
    pub prefix_binding: bool,
    pub reads_foreign_thinking: bool,
}

/// Fixed source calls String.prototype.toWellFormed only at this derived
/// conversation-text boundary. Original canonical UTF-16 handles stay intact.
pub(crate) fn well_formed(text: &WireString) -> WireString {
    WireString::from(String::from_utf16_lossy(text.units()))
}
fn unwrap_delimited(text: &[u16], open: &[u16], close: &[u16]) -> Vec<u16> {
    fn space(unit: u16) -> bool {
        matches!(unit, 9 | 10 | 11 | 12 | 13 | 32)
    }
    fn find(text: &[u16], needle: &[u16], start: usize) -> Option<usize> {
        text.get(start..)?.windows(needle.len()).position(|value| value == needle).map(|index| index + start)
    }
    let mut end = text.len();
    while end > 0 && space(text[end - 1]) {
        end -= 1;
    }
    let mut cursor = 0;
    while cursor < end && space(text[cursor]) {
        cursor += 1;
    }
    if cursor >= end || !text[cursor..].starts_with(open) {
        return text.to_vec();
    }
    let mut result = Vec::new();
    let mut count = 0;
    while cursor < end {
        if !text[cursor..].starts_with(open) {
            return text.to_vec();
        }
        let inner_start = cursor + open.len();
        let mut scan = inner_start;
        let mut depth = 1;
        let inner_end = loop {
            let Some(next_close) = find(text, close, scan).filter(|index| *index < end) else {
                return text.to_vec();
            };
            if let Some(next_open) = find(text, open, scan).filter(|index| *index < next_close) {
                depth += 1;
                scan = next_open + open.len();
                continue;
            }
            depth -= 1;
            if depth == 0 {
                break next_close;
            }
            scan = next_close + close.len();
        };
        let mut trim_end = inner_end;
        while trim_end > inner_start && space(text[trim_end - 1]) {
            trim_end -= 1;
        }
        let mut trim_start = inner_start;
        while trim_start < trim_end && space(text[trim_start]) {
            trim_start += 1;
        }
        if count > 0 {
            result.push(b'\n' as u16);
        }
        result.extend(unwrap_delimited(&text[trim_start..trim_end], open, close));
        count += 1;
        cursor = inner_end + close.len();
        while cursor < end && space(text[cursor]) {
            cursor += 1;
        }
    }
    result
}
pub(crate) fn render_demoted(text: &WireString, dialect: Option<crate::dialect::Dialect>) -> Result<WireString> {
    use crate::dialect::{Dialect, rendering};
    if text.is_empty() {
        return Ok("".into());
    }
    let text = well_formed(text);
    match dialect.ok_or(ValueError::UnsupportedOperation(
        "Host preferred dialect is required for cross-model thinking demotion",
    ))? {
        Dialect::Anthropic => Ok(text),
        Dialect::Gemini => {
            let mut result = WireString::from("```thinking\n");
            result = concat(&result, &text);
            result.append_str("\n```");
            Ok(result)
        }
        Dialect::Hermes => Ok(rendering::render_delimited_thinking_units(&text)),
        Dialect::Xml | Dialect::Minimax => {
            let open = "<thinking>".encode_utf16().collect::<Vec<_>>();
            let close = "</thinking>".encode_utf16().collect::<Vec<_>>();
            let mut units = open.clone();
            units.push(b'\n' as u16);
            units.extend(unwrap_delimited(text.units(), &open, &close));
            units.push(b'\n' as u16);
            units.extend(close);
            Ok(WireString::from_units(units))
        }
        Dialect::Glm | Dialect::Kimi | Dialect::Deepseek | Dialect::Qwen3 | Dialect::Harmony | Dialect::Gemma => {
            Ok(rendering::render_simple_thinking_units(&text))
        }
    }
}

fn truthy_property(builder: &GraphBuilder, source: &ValueHandle, key: &str) -> Result<bool> {
    builder.0.heap().truthy(&builder.get(source, key)?)
}
fn timestamp_le(builder: &GraphBuilder, left: &ValueHandle, right: &ValueHandle) -> Result<bool> {
    // The <= operator converts the left operand first, with number hint.
    let left = builder.0.heap().to_primitive(left, false, builder.0.runtime())?;
    let right = builder.0.heap().to_primitive(right, false, builder.0.runtime())?;
    match (builder.0.heap().snapshot(&left)?, builder.0.heap().snapshot(&right)?) {
        (ValueSnapshot::String(left), ValueSnapshot::String(right)) => Ok(left.units() <= right.units()),
        (ValueSnapshot::BigInt(left), ValueSnapshot::BigInt(right)) => {
            let left = left.as_str();
            let right = right.as_str();
            let (left_negative, left_digits) = left.strip_prefix('-').map_or((false, left), |digits| (true, digits));
            let (right_negative, right_digits) =
                right.strip_prefix('-').map_or((false, right), |digits| (true, digits));
            if left_negative != right_negative {
                return Ok(left_negative);
            }
            let ordering = left_digits.len().cmp(&right_digits.len()).then_with(|| left_digits.cmp(right_digits));
            Ok(if left_negative { !ordering.is_lt() } else { !ordering.is_gt() })
        }
        (ValueSnapshot::BigInt(_), _) | (_, ValueSnapshot::BigInt(_)) => Err(ValueError::UnsupportedOperation(
            "Mixed BigInt history timestamp comparison requires the shared Native relational primitive",
        )),
        _ => {
            let left = builder.0.heap().to_number(&left, builder.0.runtime())?;
            let right = builder.0.heap().to_number(&right, builder.0.runtime())?;
            Ok(left <= right)
        }
    }
}

/// Native-only source normalization for the three currently supported OpenAI
/// target APIs. Other targets need their own provider implementation.
pub(crate) fn normalize_openai_history(
    builder: &GraphBuilder,
    messages: &[ValueHandle],
    model: &Model,
    policy: &OpenAIHistoryPolicy,
    normalize_id: impl Fn(&WireString, &ValueHandle) -> Result<WireString>,
    demote: impl Fn(&WireString) -> Result<WireString>,
) -> Result<Vec<ValueHandle>> {
    if model.api != "openai-completions"
        && model.api != "openai-responses"
        && model.api != "openai-codex-responses"
        && model.api != "azure-openai-responses"
    {
        return Err(ValueError::UnsupportedOperation("This graph history normalizer requires an OpenAI-family target"));
    }
    let mut id_map: HashMap<WireString, WireString> = HashMap::new();
    let mut composite_map = HashMap::new();
    // The fixed normalizer performs this read pass before prefix binding even
    // when the target's later branch does not use the Anthropic latest flag.
    for message in messages.iter().rev() {
        if equal(builder, message, "role", "assistant")? && !truncated_thinking_only(builder, message)? {
            break;
        }
    }
    let mut invalid_bound = HashSet::new();
    if policy.prefix_binding {
        let mut rewritten_at: Option<ValueHandle> = None;
        for (index, message) in messages.iter().enumerate() {
            let candidate = if equal(builder, message, "role", "user")?
                && !matches!(
                    builder.0.heap().snapshot(&builder.get(message, "historyRewriteAt")?)?,
                    ValueSnapshot::Undefined
                ) {
                Some(builder.get(message, "historyRewriteAt")?)
            } else if equal(builder, message, "role", "toolResult")?
                && !matches!(builder.0.heap().snapshot(&builder.get(message, "prunedAt")?)?, ValueSnapshot::Undefined)
            {
                Some(builder.get(message, "prunedAt")?)
            } else {
                None
            };
            if let Some(candidate) = candidate {
                rewritten_at = Some(if let Some(old) = rewritten_at {
                    let old = builder.0.heap().to_number(&old, builder.0.runtime())?;
                    let next = builder.0.heap().to_number(&candidate, builder.0.runtime())?;
                    builder.0.heap().number(if old.is_nan() || next.is_nan() {
                        f64::NAN
                    } else if old == 0.0 && next == 0.0 {
                        if old.is_sign_positive() || next.is_sign_positive() { 0.0 } else { -0.0 }
                    } else {
                        old.max(next)
                    })?
                } else {
                    candidate
                });
            } else if equal(builder, message, "role", "assistant")? {
                if let Some(rewritten_at) = &rewritten_at {
                    let timestamp = builder.get(message, "timestamp")?;
                    if timestamp_le(builder, &timestamp, rewritten_at)? {
                        invalid_bound.insert(index);
                    }
                }
            }
        }
    }
    let mut result = Vec::with_capacity(messages.len());
    for (message_index, message) in messages.iter().enumerate() {
        if equal(builder, message, "role", "user")? || equal(builder, message, "role", "developer")? {
            result.push(message.clone());
            continue;
        }
        if equal(builder, message, "role", "toolResult")? {
            let id = string(builder, message, "toolCallId")?;
            let replacement = id_map.get(&id).or_else(|| {
                if id.units().contains(&(b'|' as u16)) { composite_map.get(&call_component(&id)) } else { None }
            });
            result.push(match replacement {
                Some(replacement) if replacement != &id && !replacement.is_empty() => {
                    copy_with(builder, message, "toolCallId", builder.text(replacement.clone())?)?
                }
                _ => message.clone(),
            });
            continue;
        }
        if !equal(builder, message, "role", "assistant")? {
            result.push(message.clone());
            continue;
        }
        let same_model = equal(builder, message, "provider", &model.provider)?
            && equal(builder, message, "api", &model.api)?
            && equal(builder, message, "model", &model.id)?;
        let invalid_stop =
            equal(builder, message, "stopReason", "aborted")? || equal(builder, message, "stopReason", "error")?;
        let original_content = content(builder, message)?;
        let mut abandoned = false;
        if !invalid_stop && !equal(builder, message, "stopReason", "toolUse")? {
            for block in &original_content {
                if equal(builder, block, "type", "toolCall")? {
                    abandoned = true;
                    break;
                }
            }
        }
        let last_index = original_content.len().checked_sub(1);
        let mut transformed = Vec::new();
        for (index, block) in original_content.into_iter().enumerate() {
            if invalid_bound.contains(&message_index)
                && (equal(builder, &block, "type", "thinking")? || equal(builder, &block, "type", "redactedThinking")?)
            {
                continue;
            }
            if equal(builder, &block, "type", "thinking")? {
                let untrustworthy = abandoned || (invalid_stop && Some(index) == last_index);
                let mut sanitized = if untrustworthy && truthy_property(builder, &block, "thinkingSignature")? {
                    copy_with(builder, &block, "thinkingSignature", builder.0.heap().undefined()?)?
                } else {
                    block
                };
                if same_model && truthy_property(builder, &sanitized, "thinkingSignature")? {
                    transformed.push(sanitized);
                    continue;
                }
                if !truthy_property(builder, &sanitized, "thinking")? {
                    continue;
                }
                let thought = string(builder, &sanitized, "thinking")?;
                if trim_empty(&thought) {
                    continue;
                }
                if same_model {
                    transformed.push(sanitized);
                    continue;
                }
                if policy.reads_foreign_thinking {
                    if truthy_property(builder, &sanitized, "thinkingSignature")? {
                        sanitized = copy_with(builder, &sanitized, "thinkingSignature", builder.0.heap().undefined()?)?;
                    }
                    transformed.push(sanitized);
                    continue;
                }
                let demoted =
                    builder.object([("type", builder.text("text")?), ("text", builder.text(demote(&thought)?)?)])?;
                builder.0.heap().define_own_data(
                    &demoted,
                    demoted_thinking_key(builder)?,
                    builder.0.heap().boolean(true)?,
                    builder.0.runtime(),
                )?;
                transformed.push(demoted);
            } else if equal(builder, &block, "type", "redactedThinking")? {
                if same_model {
                    transformed.push(block);
                }
            } else if equal(builder, &block, "type", "anthropicServerTool")?
                || equal(builder, &block, "type", "fallback")?
                || equal(builder, &block, "type", "image")?
            {
                // These are native-only display/server artifacts. OpenAI
                // Responses replay payload is carried separately by its encoder.
            } else if equal(builder, &block, "type", "text")? {
                transformed.push(if same_model {
                    block
                } else {
                    builder.object([("type", builder.text("text")?), ("text", builder.get(&block, "text")?)])?
                });
            } else if equal(builder, &block, "type", "toolCall")? {
                let mut normalized = if !same_model && truthy_property(builder, &block, "thoughtSignature")? {
                    copy_with(builder, &block, "thoughtSignature", builder.0.heap().undefined()?)?
                } else {
                    block.clone()
                };
                if !same_model {
                    let id = string(builder, &block, "id")?;
                    let normalized_id = normalize_id(&id, message)?;
                    if normalized_id != id {
                        id_map.insert(id.clone(), normalized_id.clone());
                        normalized = copy_with(builder, &normalized, "id", builder.text(normalized_id.clone())?)?;
                    }
                    if responses_message(builder, message)? {
                        composite_map.insert(call_component(&id), normalized_id);
                    }
                }
                transformed.push(normalized);
            } else {
                transformed.push(block);
            }
        }
        if let Some(last) = transformed.last_mut() {
            if equal(builder, last, "type", "text")? {
                let marker = builder.0.heap().get(last, &demoted_thinking_key(builder)?, builder.0.runtime())?;
                if matches!(builder.0.heap().snapshot(&marker)?, ValueSnapshot::Boolean(true)) {
                    let text = string(builder, last, "text")?;
                    let length = text
                        .units()
                        .iter()
                        .rposition(|unit| !char::from_u32(u32::from(*unit)).is_some_and(ara_prompt::js::is_space))
                        .map_or(0, |index| index + 1);
                    *last = copy_with(builder, last, "text", builder.text(text.slice_prefix(length))?)?;
                }
            }
        }
        result.push(copy_with(builder, message, "content", array(builder, transformed)?)?);
    }
    Ok(result)
}

// Original-container entry. The Vec API above remains for existing consumers;
// this path never imports or reconstructs the input messages container.
#[derive(Clone, Copy)]
pub(crate) struct NativeExecution<'a> {
    pub heap: &'a Arc<ValueHeap>,
    pub runtime: &'a dyn ValueRuntime,
}
impl<'a> NativeExecution<'a> {
    pub(crate) fn get(self, root: &ValueHandle, key: &str) -> Result<ValueHandle> {
        self.heap.get(root, &key.into(), self.runtime)
    }
    pub(crate) fn own(self, root: &ValueHandle, key: &str, value: ValueHandle) -> Result<()> {
        self.heap.define_own_data(root, key.into(), value, self.runtime)
    }
    pub(crate) fn put(self, root: &ValueHandle, key: &str, value: ValueHandle) -> Result<()> {
        self.heap.assign(root, key.into(), value, self.runtime)
    }
    pub(crate) fn text(self, text: impl Into<WireString>) -> Result<ValueHandle> {
        self.heap.string(text.into())
    }
    pub(crate) fn object(self, fields: impl IntoIterator<Item = (&'static str, ValueHandle)>) -> Result<ValueHandle> {
        let result = self.heap.object()?;
        for (key, value) in fields {
            self.own(&result, key, value)?;
        }
        Ok(result)
    }
    pub(crate) fn array(self, values: impl IntoIterator<Item = ValueHandle>) -> Result<ValueHandle> {
        self.heap.array(values.into_iter().collect())
    }
    pub(crate) fn method_lazy(
        self,
        receiver: &ValueHandle,
        key: &str,
        args: impl FnOnce() -> Result<Vec<ValueHandle>>,
    ) -> Result<ValueHandle> {
        let method = self.get(receiver, key)?;
        let arguments = args()?;
        self.heap.call(&method, receiver, &arguments, self.runtime)
    }
    pub(crate) fn apply(
        self,
        receiver: &ValueHandle,
        method: &str,
        spec: &'static CompiledFunctionSpec,
        captures: &[ValueHandle],
    ) -> Result<ValueHandle> {
        let selected = self.get(receiver, method)?;
        let callback = self.heap.fresh_compiled_function(spec, captures)?;
        self.heap.call(&selected, receiver, &[callback], self.runtime)
    }
    pub(crate) fn strict(self, left: &ValueHandle, right: &ValueHandle) -> Result<bool> {
        match (self.heap.snapshot(left)?, self.heap.snapshot(right)?) {
            (ValueSnapshot::Number(a), ValueSnapshot::Number(b)) => Ok(a == b),
            _ => self.heap.object_is(left, right),
        }
    }
    pub(crate) fn number_is(self, value: &ValueHandle, expected: f64) -> Result<bool> {
        Ok(matches!(self.heap.snapshot(value)?,ValueSnapshot::Number(n) if n==expected))
    }
    pub(crate) fn undefined(self, value: &ValueHandle) -> Result<bool> {
        Ok(self.heap.kind(value)? == ValueKind::Undefined)
    }
    pub(crate) fn nullish(self, value: &ValueHandle) -> Result<bool> {
        Ok(matches!(self.heap.kind(value)?, ValueKind::Null | ValueKind::Undefined))
    }
    pub(crate) fn eq(self, root: &ValueHandle, key: &str, expected: &str) -> Result<bool> {
        Ok(matches!(self.heap.snapshot(&self.get(root,key)?)?,ValueSnapshot::String(s) if s.equals_ascii(expected)))
    }
    pub(crate) fn string(self, value: &ValueHandle) -> Result<WireString> {
        match self.heap.snapshot(value)? {
            ValueSnapshot::String(s) => Ok(s),
            _ => Err(ValueError::TypeError("Fixed history string primitive expected".into())),
        }
    }
    pub(crate) fn index(self, array: &ValueHandle, index: f64) -> Result<ValueHandle> {
        let key = self.heap.to_string(&self.heap.number(index)?, self.runtime)?;
        self.heap.get(array, &key.into(), self.runtime)
    }
    pub(crate) fn length(self, array: &ValueHandle) -> Result<f64> {
        self.heap.to_number(&self.get(array, "length")?, self.runtime)
    }
    pub(crate) fn copy_lazy(
        self,
        source: &ValueHandle,
        key: &str,
        value: impl FnOnce() -> Result<ValueHandle>,
    ) -> Result<ValueHandle> {
        let copy = self.heap.spread(source, self.runtime)?;
        let value = value()?;
        self.own(&copy, key, value)?;
        Ok(copy)
    }
    pub(crate) fn for_of(self, source: &ValueHandle, mut body: impl FnMut(&ValueHandle) -> Result<bool>) -> Result<()> {
        let mut iterator = self.heap.get_iterator(source, self.runtime)?;
        while let Some(value) = self.heap.iterator_next(&mut iterator, self.runtime)? {
            match body(&value) {
                Ok(true) => {}
                Ok(false) => {
                    self.heap.iterator_close(&mut iterator, self.runtime)?;
                    break;
                }
                Err(original) => {
                    return Err(match self.heap.iterator_close(&mut iterator, self.runtime) {
                        Ok(()) => original,
                        Err(close) if original.is_native_exception() && close.is_native_exception() => original,
                        Err(close) => {
                            ValueError::IteratorClose { original: Box::new(original), close: Box::new(close) }
                        }
                    });
                }
            }
        }
        Ok(())
    }
    fn map(self) -> Result<ValueHandle> {
        self.heap.new_map(&self.heap.undefined()?, self.runtime)
    }
    fn set(self) -> Result<ValueHandle> {
        self.heap.new_set(&self.heap.undefined()?, self.runtime)
    }
    fn push_lazy(self, array: &ValueHandle, value: impl FnOnce() -> Result<ValueHandle>) -> Result<()> {
        let method = self.get(array, "push")?;
        let value = value()?;
        self.heap.call(&method, array, &[value], self.runtime).map(|_| ())
    }
}
fn nfirst(heap: &Arc<ValueHeap>, args: &[ValueHandle]) -> Result<ValueHandle> {
    args.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())
}
fn narg(heap: &Arc<ValueHeap>, args: &[ValueHandle], index: usize) -> Result<ValueHandle> {
    args.get(index).cloned().map(Ok).unwrap_or_else(|| heap.undefined())
}
macro_rules! nspec {
    ($name:ident,$source:literal,$length:literal,$body:ident,$captures:literal) => {
        static $name: CompiledFunctionSpec = CompiledFunctionSpec {
            source: $source,
            name: "",
            length: $length,
            form: CompiledFunctionForm::Arrow,
            capture_count: $captures,
            body: $body,
        };
    };
}
fn nmalformed(ex: NativeExecution<'_>, block: &ValueHandle) -> Result<bool> {
    for key in ["id", "name"] {
        let value = ex.get(block, key)?;
        if !ex.heap.truthy(&value)? {
            return Ok(true);
        }
        let trimmed = ex.method_lazy(&value, "trim", || Ok((&[]).to_vec()))?;
        if ex.number_is(&ex.get(&trimmed, "length")?, 0.0)? {
            return Ok(true);
        }
    }
    Ok(false)
}
fn nsanitize(ex: NativeExecution<'_>, messages: &ValueHandle) -> Result<ValueHandle> {
    let mut malformed = false;
    ex.for_of(messages, |msg| {
        if !ex.eq(msg, "role", "assistant")? {
            return Ok(true);
        }
        ex.for_of(&ex.get(msg, "content")?, |block| {
            if ex.eq(block, "type", "toolCall")? && nmalformed(ex, block)? {
                malformed = true;
                return Ok(false);
            }
            Ok(true)
        })?;
        Ok(!malformed)
    })?;
    if !malformed {
        return Ok(messages.clone());
    }
    let queues = ex.map()?;
    let result = ex.array([])?;
    ex.for_of(messages, |msg| {
        if ex.eq(msg, "role", "assistant")? {
            ex.method_lazy(&queues, "clear", || Ok((&[]).to_vec()))?;
            let filtered = ex.array([])?;
            ex.for_of(&ex.get(msg, "content")?, |block| {
                if ex.eq(block, "type", "toolCall")? {
                    let malformed = nmalformed(ex, block)?;
                    let queue = ex.method_lazy(&queues, "get", || Ok((&[ex.get(block, "id")?]).to_vec()))?;
                    if ex.heap.truthy(&queue)? {
                        ex.push_lazy(&queue, || Ok(ex.heap.boolean(malformed)?))?;
                    } else {
                        ex.method_lazy(&queues, "set", || {
                            Ok((&[ex.get(block, "id")?, ex.array([ex.heap.boolean(malformed)?])?]).to_vec())
                        })?;
                    }
                    if malformed {
                        return Ok(true);
                    }
                }
                ex.push_lazy(&filtered, || Ok(block.clone()))?;
                Ok(true)
            })?;
            if ex.number_is(&ex.get(&filtered, "length")?, 0.0)? {
                return Ok(true);
            }
            ex.push_lazy(&result, || {
                let unchanged =
                    ex.strict(&ex.get(&filtered, "length")?, &ex.get(&ex.get(msg, "content")?, "length")?)?;
                Ok(if unchanged { msg.clone() } else { ex.copy_lazy(msg, "content", || Ok(filtered))? })
            })?;
            return Ok(true);
        }
        if ex.eq(msg, "role", "toolResult")? {
            let queue = ex.method_lazy(&queues, "get", || Ok((&[ex.get(msg, "toolCallId")?]).to_vec()))?;
            if ex.heap.truthy(&queue)? && ex.length(&queue)? > 0.0 {
                let drop = ex.method_lazy(&queue, "shift", || Ok((&[]).to_vec()))?;
                let drop = matches!(ex.heap.snapshot(&drop)?, ValueSnapshot::Boolean(true));
                if ex.number_is(&ex.get(&queue, "length")?, 0.0)? {
                    ex.method_lazy(&queues, "delete", || Ok((&[ex.get(msg, "toolCallId")?]).to_vec()))?;
                }
                if drop {
                    return Ok(true);
                }
            }
            ex.push_lazy(&result, || Ok(msg.clone()))?;
            return Ok(true);
        }
        ex.method_lazy(&queues, "clear", || Ok((&[]).to_vec()))?;
        ex.push_lazy(&result, || Ok(msg.clone()))?;
        Ok(true)
    })?;
    Ok(result)
}
fn ntool_or_text(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    let value = nfirst(heap, args)?;
    heap.boolean(ex.eq(&value, "type", "toolCall")? || ex.eq(&value, "type", "text")?)
}
fn ntool(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.boolean(NativeExecution { heap, runtime }.eq(&nfirst(heap, args)?, "type", "toolCall")?)
}
fn nthinking(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.boolean(NativeExecution { heap, runtime }.eq(&nfirst(heap, args)?, "type", "thinking")?)
}
nspec!(N_TOOL_OR_TEXT, "transform-messages.ts:348", 1, ntool_or_text, 0);
nspec!(N_TOOL, "transform-messages.ts:728; 1164", 1, ntool, 0);
nspec!(N_THINKING, "transform-messages.ts:750", 1, nthinking, 0);
fn ntruncated(ex: NativeExecution<'_>, msg: &ValueHandle) -> Result<bool> {
    let truncated = ex.eq(msg, "stopReason", "length")?
        || ex.eq(msg, "stopReason", "error")?
        || ex.eq(msg, "stopReason", "aborted")?;
    if !truncated {
        return Ok(false);
    }
    let some = ex.apply(&ex.get(msg, "content")?, "some", &N_TOOL_OR_TEXT, &[])?;
    Ok(!ex.heap.truthy(&some)?)
}
fn nresponses(ex: NativeExecution<'_>, msg: &ValueHandle) -> Result<bool> {
    let api = ex.get(msg, "api")?;
    Ok(matches!(ex.heap.snapshot(&api)?,ValueSnapshot::String(s) if responses_family(&s)))
}
fn ncomponent(ex: NativeExecution<'_>, id: &ValueHandle) -> Result<ValueHandle> {
    let pipe = ex.method_lazy(id, "indexOf", || Ok((&[ex.text("|")?]).to_vec()))?;
    if ex.heap.to_number(&pipe, ex.runtime)? <= 0.0 {
        Ok(id.clone())
    } else {
        ex.method_lazy(id, "slice", || Ok((&[ex.heap.number(0.0)?, pipe]).to_vec()))
    }
}
fn norigin(ex: NativeExecution<'_>, messages: &ValueHandle) -> Result<ValueHandle> {
    let scope = ex.object([("responsesComponents", ex.set()?), ("opaqueCompositeCallIds", ex.set()?)])?;
    ex.for_of(messages, |msg| {
        if !ex.eq(msg, "role", "assistant")? {
            return Ok(true);
        }
        let responses = nresponses(ex, msg)?;
        ex.for_of(&ex.get(msg, "content")?, |block| {
            if !ex.eq(block, "type", "toolCall")? {
                return Ok(true);
            }
            if responses {
                ex.method_lazy(&ex.get(&scope, "responsesComponents")?, "add", || {
                    Ok((&[ncomponent(ex, &ex.get(block, "id")?)?]).to_vec())
                })?;
            } else {
                let includes = ex.method_lazy(&ex.get(block, "id")?, "includes", || Ok((&[ex.text("|")?]).to_vec()))?;
                if ex.heap.truthy(&includes)? {
                    ex.method_lazy(&ex.get(&scope, "opaqueCompositeCallIds")?, "add", || {
                        Ok((&[ex.get(block, "id")?]).to_vec())
                    })?;
                }
            }
            Ok(true)
        })?;
        Ok(true)
    })?;
    Ok(scope)
}
fn npair(ex: NativeExecution<'_>, id: &ValueHandle, scope: &ValueHandle) -> Result<ValueHandle> {
    let pipe = ex.method_lazy(id, "indexOf", || Ok((&[ex.text("|")?]).to_vec()))?;
    if ex.heap.to_number(&pipe, ex.runtime)? <= 0.0 {
        return Ok(id.clone());
    }
    if ex
        .heap
        .truthy(&ex.method_lazy(&ex.get(scope, "opaqueCompositeCallIds")?, "has", || Ok((&[id.clone()]).to_vec()))?)?
    {
        return Ok(id.clone());
    }
    let prefix = ex.method_lazy(id, "slice", || Ok((&[ex.heap.number(0.0)?, pipe]).to_vec()))?;
    if ex
        .heap
        .truthy(&ex.method_lazy(&ex.get(scope, "responsesComponents")?, "has", || Ok((&[prefix.clone()]).to_vec()))?)?
    {
        Ok(prefix)
    } else {
        Ok(id.clone())
    }
}
fn nplus_one(ex: NativeExecution<'_>, value: &ValueHandle) -> Result<ValueHandle> {
    ex.heap.add(value, &ex.heap.number(1.0)?, ex.runtime)
}
fn nappend_segment(
    ex: NativeExecution<'_>,
    segment: &ValueHandle,
    suffix: &ValueHandle,
    max: &ValueHandle,
) -> Result<ValueHandle> {
    let segment_len = ex.get(segment, "length")?;
    let suffix_len = ex.get(suffix, "length")?;
    let combined = ex.heap.add(&segment_len, &suffix_len, ex.runtime)?;
    if crate::schema_validator::graph_values_compare_with_execution(
        crate::graph_values::GraphExecution::new(ex.heap, ex.runtime),
        &combined,
        max,
        "<=",
    )? {
        return ex.text(concat(&ex.heap.to_string(segment, ex.runtime)?, &ex.heap.to_string(suffix, ex.runtime)?));
    }
    let budget =
        (ex.heap.to_number(max, ex.runtime)? - ex.heap.to_number(&ex.get(suffix, "length")?, ex.runtime)?).max(0.0);
    let prefix =
        ex.method_lazy(segment, "slice", || Ok((&[ex.heap.number(0.0)?, ex.heap.number(budget)?]).to_vec()))?;
    ex.text(concat(&ex.heap.to_string(&prefix, ex.runtime)?, &ex.heap.to_string(suffix, ex.runtime)?))
}
fn nsegment(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    nappend_segment(NativeExecution { heap, runtime }, &nfirst(heap, args)?, &captures[0], &captures[1])
}
nspec!(N_SEGMENT, "transform-messages.ts:134", 1, nsegment, 2);
fn nappend_duplicate(
    ex: NativeExecution<'_>,
    id: &ValueHandle,
    suffix: &ValueHandle,
    max: &ValueHandle,
) -> Result<ValueHandle> {
    if ex.heap.truthy(&ex.method_lazy(id, "includes", || Ok((&[ex.text("|")?]).to_vec()))?)? {
        let segments = ex.method_lazy(id, "split", || Ok((&[ex.text("|")?]).to_vec()))?;
        let suffixed = ex.apply(&segments, "map", &N_SEGMENT, &[suffix.clone(), max.clone()])?;
        ex.method_lazy(&suffixed, "join", || Ok((&[ex.text("|")?]).to_vec()))
    } else {
        nappend_segment(ex, id, suffix, max)
    }
}
fn ndedup_block(
    heap: &Arc<ValueHeap>,
    c: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    let block = nfirst(heap, args)?;
    if !ex.eq(&block, "type", "toolCall")? {
        return Ok(block);
    }
    let key = npair(ex, &ex.get(&block, "id")?, &c[2])?;
    if !heap.truthy(&ex.method_lazy(&c[3], "has", || Ok((&[key.clone()]).to_vec()))?)? {
        ex.method_lazy(&c[1], "delete", || Ok((&[key.clone()]).to_vec()))?;
        ex.method_lazy(&c[3], "add", || Ok((&[key.clone()]).to_vec()))?;
    }
    let previous = ex.method_lazy(&c[0], "get", || Ok((&[key.clone()]).to_vec()))?;
    let previous = if ex.nullish(&previous)? { heap.number(0.0)? } else { previous };
    let enqueue = |rewrite: ValueHandle| -> Result<()> {
        let queue = ex.method_lazy(&c[1], "get", || Ok((&[key.clone()]).to_vec()))?;
        if heap.truthy(&queue)? {
            ex.push_lazy(&queue, || Ok(rewrite))?;
        } else {
            ex.method_lazy(&c[1], "set", || Ok((&[key.clone(), ex.array([rewrite])?]).to_vec()))?;
        }
        Ok(())
    };
    if ex.number_is(&previous, 0.0)? {
        ex.method_lazy(&c[0], "set", || Ok((&[key.clone(), heap.number(1.0)?]).to_vec()))?;
        enqueue(heap.undefined()?)?;
        return Ok(block);
    }
    let mut duplicate = previous;
    let suffix = |index: &ValueHandle| -> Result<ValueHandle> {
        ex.text(concat(&heap.to_string(&c[5], runtime)?, &heap.to_string(index, runtime)?))
    };
    let mut replacement = nappend_duplicate(ex, &ex.get(&block, "id")?, &suffix(&duplicate)?, &c[4])?;
    while heap.truthy(&ex.method_lazy(&c[0], "has", || Ok((&[npair(ex, &replacement, &c[2])?]).to_vec()))?)? {
        duplicate = nplus_one(ex, &duplicate)?;
        replacement = nappend_duplicate(ex, &ex.get(&block, "id")?, &suffix(&duplicate)?, &c[4])?;
    }
    ex.method_lazy(&c[0], "set", || Ok((&[key.clone(), nplus_one(ex, &duplicate)?]).to_vec()))?;
    ex.method_lazy(&c[0], "set", || Ok((&[npair(ex, &replacement, &c[2])?, heap.number(1.0)?]).to_vec()))?;
    enqueue(ex.object([("replacementId", replacement.clone())])?)?;
    ex.put(&c[6], "changed", heap.boolean(true)?)?;
    ex.copy_lazy(&block, "id", || Ok(replacement))
}
nspec!(N_DEDUP_BLOCK, "transform-messages.ts:191", 1, ndedup_block, 7);
fn ndedup_message(
    heap: &Arc<ValueHeap>,
    c: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    let msg = nfirst(heap, args)?;
    if ex.eq(&msg, "role", "toolResult")? {
        let key = npair(ex, &ex.get(&msg, "toolCallId")?, &c[2])?;
        let queue = ex.method_lazy(&c[1], "get", || Ok((&[key.clone()]).to_vec()))?;
        if !heap.truthy(&queue)? || ex.number_is(&ex.get(&queue, "length")?, 0.0)? {
            return Ok(msg);
        }
        let rewrite = ex.method_lazy(&queue, "shift", || Ok((&[]).to_vec()))?;
        if ex.number_is(&ex.get(&queue, "length")?, 0.0)? {
            ex.method_lazy(&c[1], "delete", || Ok((&[key]).to_vec()))?;
        }
        return if heap.truthy(&rewrite)? {
            ex.copy_lazy(&msg, "toolCallId", || Ok(ex.get(&rewrite, "replacementId")?))
        } else {
            Ok(msg)
        };
    }
    if !ex.eq(&msg, "role", "assistant")? {
        return Ok(msg);
    }
    let touched = ex.set()?;
    let state = ex.object([("changed", heap.boolean(false)?)])?;
    let content = ex.apply(
        &ex.get(&msg, "content")?,
        "map",
        &N_DEDUP_BLOCK,
        &[c[0].clone(), c[1].clone(), c[2].clone(), touched, c[3].clone(), c[4].clone(), state.clone()],
    )?;
    if !heap.truthy(&ex.get(&state, "changed")?)? { Ok(msg) } else { ex.copy_lazy(&msg, "content", || Ok(content)) }
}
nspec!(N_DEDUP_MESSAGE, "transform-messages.ts:161", 1, ndedup_message, 5);

// captures: model, policy, normalizer, demoter, idMap, compositeMap,
// invalidBoundSet, message, index, sameModel, invalidStop, abandoned, lastIndex.
fn nnormalize_block(
    heap: &Arc<ValueHeap>,
    c: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    let block = nfirst(heap, args)?;
    let index = narg(heap, args, 1)?;
    let same = heap.truthy(&c[9])?;
    let invalid = heap.truthy(&c[10])?;
    let abandoned = heap.truthy(&c[11])?;
    if heap.truthy(&ex.method_lazy(&c[6], "has", || Ok((&[c[8].clone()]).to_vec()))?)?
        && (ex.eq(&block, "type", "thinking")? || ex.eq(&block, "type", "redactedThinking")?)
    {
        return ex.array([]);
    }
    if ex.eq(&block, "type", "thinking")? {
        let untrustworthy = abandoned || (invalid && ex.strict(&index, &c[12])?);
        let mut sanitized = if untrustworthy && heap.truthy(&ex.get(&block, "thinkingSignature")?)? {
            ex.copy_lazy(&block, "thinkingSignature", || Ok(heap.undefined()?))?
        } else {
            block
        };
        if same && heap.truthy(&ex.get(&sanitized, "thinkingSignature")?)? {
            return Ok(sanitized);
        }
        if !heap.truthy(&ex.get(&sanitized, "thinking")?)? {
            return ex.array([]);
        }
        let trimmed = ex.method_lazy(&ex.get(&sanitized, "thinking")?, "trim", || Ok((&[]).to_vec()))?;
        if ex.strict(&trimmed, &ex.text("")?)? {
            return ex.array([]);
        }
        if same {
            return Ok(sanitized);
        }
        if heap.truthy(&ex.get(&c[1], "readsForeignThinking")?)? {
            if heap.truthy(&ex.get(&sanitized, "thinkingSignature")?)? {
                sanitized = ex.copy_lazy(&sanitized, "thinkingSignature", || Ok(heap.undefined()?))?;
            }
            return Ok(sanitized);
        }
        let text =
            heap.call(&c[3], &heap.undefined()?, &[ex.get(&c[0], "id")?, ex.get(&sanitized, "thinking")?], runtime)?;
        let demoted = ex.object([("type", ex.text("text")?), ("text", text)])?;
        heap.define_own_data(
            &demoted,
            heap.module_symbol("ara-ai.block-symbols", "provider.block.demotedThinking")?,
            heap.boolean(true)?,
            runtime,
        )?;
        return Ok(demoted);
    }
    if ex.eq(&block, "type", "redactedThinking")? {
        return if same { Ok(block) } else { ex.array([]) };
    }
    if ex.eq(&block, "type", "anthropicServerTool")?
        || ex.eq(&block, "type", "fallback")?
        || ex.eq(&block, "type", "image")?
    {
        return ex.array([]);
    }
    if ex.eq(&block, "type", "text")? {
        return if same {
            Ok(block)
        } else {
            ex.object([("type", ex.text("text")?), ("text", ex.get(&block, "text")?)])
        };
    }
    if ex.eq(&block, "type", "toolCall")? {
        let mut normalized = if !same && heap.truthy(&ex.get(&block, "thoughtSignature")?)? {
            ex.copy_lazy(&block, "thoughtSignature", || Ok(heap.undefined()?))?
        } else {
            block.clone()
        };
        if !same {
            let id =
                heap.call(&c[2], &heap.undefined()?, &[ex.get(&block, "id")?, c[0].clone(), c[7].clone()], runtime)?;
            if !ex.undefined(&id)? {
                if !ex.strict(&id, &ex.get(&block, "id")?)? {
                    ex.method_lazy(&c[4], "set", || Ok((&[ex.get(&block, "id")?, id.clone()]).to_vec()))?;
                    normalized = ex.copy_lazy(&normalized, "id", || Ok(id.clone()))?;
                }
                if nresponses(ex, &c[7])? {
                    ex.method_lazy(&c[5], "set", || Ok((&[ncomponent(ex, &ex.get(&block, "id")?)?, id]).to_vec()))?;
                }
            }
        }
        return Ok(normalized);
    }
    Ok(block)
}
nspec!(N_NORMALIZE_BLOCK, "transform-messages.ts:760", 2, nnormalize_block, 13);
fn nnormalize_message(
    heap: &Arc<ValueHeap>,
    c: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    let msg = nfirst(heap, args)?;
    let index = narg(heap, args, 1)?;
    if ex.eq(&msg, "role", "user")? || ex.eq(&msg, "role", "developer")? {
        return Ok(msg);
    }
    if ex.eq(&msg, "role", "toolResult")? {
        let exact = ex.method_lazy(&c[4], "get", || Ok((&[ex.get(&msg, "toolCallId")?]).to_vec()))?;
        let normalized = if ex.nullish(&exact)? {
            if heap.truthy(
                &ex.method_lazy(&ex.get(&msg, "toolCallId")?, "includes", || Ok((&[ex.text("|")?]).to_vec()))?,
            )? {
                ex.method_lazy(&c[5], "get", || Ok((&[ncomponent(ex, &ex.get(&msg, "toolCallId")?)?]).to_vec()))?
            } else {
                heap.undefined()?
            }
        } else {
            exact
        };
        return if heap.truthy(&normalized)? && !ex.strict(&normalized, &ex.get(&msg, "toolCallId")?)? {
            ex.copy_lazy(&msg, "toolCallId", || Ok(normalized))
        } else {
            Ok(msg)
        };
    }
    if !ex.eq(&msg, "role", "assistant")? {
        return Ok(msg);
    }
    let same = ex.strict(&ex.get(&msg, "provider")?, &ex.get(&c[0], "provider")?)?
        && ex.strict(&ex.get(&msg, "api")?, &ex.get(&c[0], "api")?)?
        && ex.strict(&ex.get(&msg, "model")?, &ex.get(&c[0], "id")?)?;
    let invalid = ex.eq(&msg, "stopReason", "aborted")? || ex.eq(&msg, "stopReason", "error")?;
    let abandoned = !invalid
        && !ex.eq(&msg, "stopReason", "toolUse")?
        && heap.truthy(&ex.apply(&ex.get(&msg, "content")?, "some", &N_TOOL, &[])?)?;
    let last = heap.number(ex.length(&ex.get(&msg, "content")?)? - 1.0)?;
    // Fixed code evaluates this some() even when the target is not Anthropic.
    ex.apply(&ex.get(&msg, "content")?, "some", &N_THINKING, &[])?;
    let content = ex.apply(
        &ex.get(&msg, "content")?,
        "flatMap",
        &N_NORMALIZE_BLOCK,
        &[
            c[0].clone(),
            c[1].clone(),
            c[2].clone(),
            c[3].clone(),
            c[4].clone(),
            c[5].clone(),
            c[6].clone(),
            msg.clone(),
            index,
            heap.boolean(same)?,
            heap.boolean(invalid)?,
            heap.boolean(abandoned)?,
            last,
        ],
    )?;
    let final_index = ex.length(&content)? - 1.0;
    let final_block = ex.index(&content, final_index)?;
    if !ex.nullish(&final_block)? && ex.eq(&final_block, "type", "text")? {
        let marker = heap.get(
            &final_block,
            &heap.module_symbol("ara-ai.block-symbols", "provider.block.demotedThinking")?,
            runtime,
        )?;
        if matches!(heap.snapshot(&marker)?, ValueSnapshot::Boolean(true)) {
            let replacement = ex.copy_lazy(&final_block, "text", || {
                Ok(ex.method_lazy(&ex.get(&final_block, "text")?, "trimEnd", || Ok((&[]).to_vec()))?)
            })?;
            let index = ex.length(&content)? - 1.0;
            let key = heap.to_string(&heap.number(index)?, runtime)?;
            heap.assign(&content, key.into(), replacement, runtime)?;
        }
    }
    ex.copy_lazy(&msg, "content", || Ok(content))
}
nspec!(N_NORMALIZE_MESSAGE, "transform-messages.ts:629", 2, nnormalize_message, 7);
fn npending_match(
    heap: &Arc<ValueHeap>,
    c: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    heap.boolean(ex.strict(&npair(ex, &ex.get(&nfirst(heap, args)?, "id")?, &c[0])?, &c[1])?)
}
fn nunresolved(
    heap: &Arc<ValueHeap>,
    c: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    heap.boolean(!heap.truthy(
        &ex.method_lazy(&c[1], "has", || Ok((&[npair(ex, &ex.get(&nfirst(heap, args)?, "id")?, &c[0])?]).to_vec()))?,
    )?)
}
fn naborted_pair(
    heap: &Arc<ValueHeap>,
    c: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    let tc = nfirst(heap, args)?;
    ex.array([npair(ex, &ex.get(&tc, "id")?, &c[0])?, tc])
}
nspec!(N_PENDING_MATCH, "transform-messages.ts:1197", 1, npending_match, 2);
nspec!(N_UNRESOLVED, "transform-messages.ts:1222", 1, nunresolved, 2);
nspec!(N_ABORTED_PAIR, "transform-messages.ts:1173", 1, naborted_pair, 1);
fn ntake_real(
    ex: NativeExecution<'_>,
    real: &ValueHandle,
    scope: &ValueHandle,
    id: &ValueHandle,
    after: f64,
) -> Result<ValueHandle> {
    let entries = ex.method_lazy(real, "get", || Ok((&[npair(ex, id, scope)?]).to_vec()))?;
    if !ex.heap.truthy(&entries)? {
        return ex.heap.undefined();
    }
    let mut result = ex.heap.undefined()?;
    ex.for_of(&entries, |entry| {
        if ex.heap.truthy(&ex.get(entry, "consumed")?)?
            || ex.heap.to_number(&ex.get(entry, "index")?, ex.runtime)? <= after
        {
            return Ok(true);
        }
        ex.put(entry, "consumed", ex.heap.boolean(true)?)?;
        result = ex.get(entry, "msg")?;
        Ok(false)
    })?;
    Ok(result)
}
fn nsynthetic(ex: NativeExecution<'_>, tc: &ValueHandle, text: &str, timestamp: ValueHandle) -> Result<ValueHandle> {
    ex.object([
        ("role", ex.text("toolResult")?),
        ("toolCallId", ex.get(tc, "id")?),
        ("toolName", ex.get(tc, "name")?),
        ("content", ex.array([ex.object([("type", ex.text("text")?), ("text", ex.text(text)?)])?])?),
        ("isError", ex.heap.boolean(true)?),
        ("timestamp", timestamp),
    ])
}
fn nflush(
    ex: NativeExecution<'_>,
    state: &ValueHandle,
    scope: &ValueHandle,
    real: &ValueHandle,
    status: &ValueHandle,
    result: &ValueHandle,
    timestamp: &ValueHandle,
    aborted: bool,
) -> Result<()> {
    let calls = ex.get(state, if aborted { "aborted" } else { "pending" })?;
    let timestamp = if aborted {
        let ts = ex.get(state, "abortedTimestamp")?;
        if ex.undefined(&ts)? {
            return Ok(());
        }
        ts
    } else {
        if ex.number_is(&ex.get(&calls, "length")?, 0.0)? {
            return Ok(());
        }
        timestamp.clone()
    };
    let source = if aborted { ex.method_lazy(&calls, "values", || Ok((&[]).to_vec()))? } else { calls };
    ex.for_of(&source, |tc| {
        let key = npair(ex, &ex.get(tc, "id")?, scope)?;
        if ex.heap.truthy(&ex.method_lazy(status, "has", || Ok((&[key.clone()]).to_vec()))?)? {
            return Ok(true);
        }
        let after =
            ex.heap.to_number(&ex.get(state, if aborted { "abortedAfter" } else { "pendingAfter" })?, ex.runtime)?;
        let real_result = ntake_real(ex, real, scope, &ex.get(tc, "id")?, after)?;
        if ex.heap.truthy(&real_result)? {
            ex.push_lazy(result, || Ok(real_result))?;
            ex.method_lazy(status, "set", || Ok((&[key, ex.heap.number(1.0)?]).to_vec()))?;
            return Ok(true);
        }
        ex.push_lazy(result, || {
            Ok(nsynthetic(ex, tc, if aborted { "aborted" } else { "No result provided" }, timestamp.clone())?)
        })?;
        ex.method_lazy(status, "set", || Ok((&[key, ex.heap.number(if aborted { 2.0 } else { 1.0 })?]).to_vec()))?;
        Ok(true)
    })?;
    if aborted {
        ex.put(state, "aborted", ex.map()?)?;
        ex.put(state, "abortedTimestamp", ex.heap.undefined()?)?;
    } else {
        ex.put(state, "pending", ex.array([])?)?;
    }
    Ok(())
}
fn nrepair(
    ex: NativeExecution<'_>,
    original: &ValueHandle,
    transformed: &ValueHandle,
    scope: &ValueHandle,
) -> Result<ValueHandle> {
    let real = ex.map()?;
    let mut index = 0.0;
    while index < ex.length(transformed)? {
        let msg = ex.index(transformed, index)?;
        if ex.eq(&msg, "role", "toolResult")? {
            let entry = ex.object([
                ("index", ex.heap.number(index)?),
                ("msg", msg.clone()),
                ("consumed", ex.heap.boolean(false)?),
            ])?;
            let key = npair(ex, &ex.get(&msg, "toolCallId")?, scope)?;
            let entries = ex.method_lazy(&real, "get", || Ok((&[key.clone()]).to_vec()))?;
            if ex.heap.truthy(&entries)? {
                ex.push_lazy(&entries, || Ok(entry))?;
            } else {
                ex.method_lazy(&real, "set", || Ok((&[key, ex.array([entry])?]).to_vec()))?;
            }
        }
        index += 1.0;
    }
    let valid = ex.set()?;
    ex.for_of(transformed, |msg| {
        if !ex.eq(msg, "role", "assistant")? {
            return Ok(true);
        }
        ex.for_of(&ex.get(msg, "content")?, |block| {
            if ex.eq(block, "type", "toolCall")? {
                ex.method_lazy(&valid, "add", || Ok((&[npair(ex, &ex.get(block, "id")?, scope)?]).to_vec()))?;
            }
            Ok(true)
        })?;
        Ok(true)
    })?;
    let result = ex.array([])?;
    let status = ex.map()?;
    let state = ex.object([
        ("pending", ex.array([])?),
        ("pendingAfter", ex.heap.number(-1.0)?),
        ("aborted", ex.map()?),
        ("abortedTimestamp", ex.heap.undefined()?),
        ("abortedAfter", ex.heap.number(-1.0)?),
    ])?;
    let flush = |timestamp: &ValueHandle| -> Result<()> {
        nflush(ex, &state, scope, &real, &status, &result, timestamp, false)?;
        nflush(ex, &state, scope, &real, &status, &result, timestamp, true)
    };
    index = 0.0;
    while index < ex.length(transformed)? {
        let msg = ex.index(transformed, index)?;
        let timestamp = if ex.heap.has(&msg, &"timestamp".into(), ex.runtime)?
            && matches!(ex.heap.snapshot(&ex.get(&msg, "timestamp")?)?, ValueSnapshot::Number(_))
        {
            ex.get(&msg, "timestamp")?
        } else {
            ex.heap.number(crate::types::now_ms() as f64)?
        };
        if ex.eq(&msg, "role", "assistant")? {
            flush(&timestamp)?;
            let original_msg = ex.index(original, index)?;
            if ex.eq(&original_msg, "role", "assistant")? && ntruncated(ex, &original_msg)? {
                index += 1.0;
                continue;
            }
            let calls = ex.apply(&ex.get(&msg, "content")?, "filter", &N_TOOL, &[])?;
            if ex.eq(&msg, "stopReason", "error")? || ex.eq(&msg, "stopReason", "aborted")? {
                ex.push_lazy(&result, || Ok(msg.clone()))?;
                let pairs = ex.apply(&calls, "map", &N_ABORTED_PAIR, &[scope.clone()])?;
                ex.put(&state, "aborted", ex.heap.new_map(&pairs, ex.runtime)?)?;
                ex.put(&state, "abortedTimestamp", ex.get(&msg, "timestamp")?)?;
                ex.put(&state, "abortedAfter", ex.heap.number(index)?)?;
                index += 1.0;
                continue;
            }
            if ex.length(&calls)? > 0.0 {
                ex.put(&state, "pending", calls)?;
                ex.put(&state, "pendingAfter", ex.heap.number(index)?)?;
            }
            ex.push_lazy(&result, || Ok(msg))?;
        } else if ex.eq(&msg, "role", "toolResult")? {
            let key = npair(ex, &ex.get(&msg, "toolCallId")?, scope)?;
            if ex.heap.truthy(&ex.method_lazy(&status, "has", || Ok((&[key.clone()]).to_vec()))?)? {
                index += 1.0;
                continue;
            }
            let aborted = ex.get(&state, "aborted")?;
            if ex.heap.truthy(&ex.method_lazy(&aborted, "has", || Ok((&[key.clone()]).to_vec()))?)? {
                ex.method_lazy(&aborted, "delete", || Ok((&[key.clone()]).to_vec()))?;
                ex.method_lazy(&status, "set", || Ok((&[key, ex.heap.number(1.0)?]).to_vec()))?;
                ex.push_lazy(&result, || Ok(msg))?;
                index += 1.0;
                continue;
            }
            let pending = ex.get(&state, "pending")?;
            if ex.heap.truthy(&ex.apply(&pending, "some", &N_PENDING_MATCH, &[scope.clone(), key.clone()])?)? {
                ex.method_lazy(&status, "set", || Ok((&[key, ex.heap.number(1.0)?]).to_vec()))?;
                ex.push_lazy(&result, || Ok(msg))?;
                index += 1.0;
                continue;
            }
            if !ex.heap.truthy(&ex.method_lazy(&valid, "has", || Ok((&[key]).to_vec()))?)? {
                let unresolved = ex.apply(&pending, "some", &N_UNRESOLVED, &[scope.clone(), status.clone()])?;
                if ex.heap.truthy(&unresolved)? || ex.heap.to_number(&ex.get(&aborted, "size")?, ex.runtime)? > 0.0 {
                    index += 1.0;
                    continue;
                }
                let parts = ex.array([])?;
                ex.for_of(&ex.get(&msg, "content")?, |part| {
                    if ex.eq(part, "type", "text")?
                        && !ex.strict(
                            &ex.method_lazy(&ex.get(part, "text")?, "trim", || Ok((&[]).to_vec()))?,
                            &ex.text("")?,
                        )?
                    {
                        ex.push_lazy(&parts, || Ok(ex.get(part, "text")?))?;
                    }
                    Ok(true)
                })?;
                if ex.length(&parts)? > 0.0 {
                    let error = if ex.heap.truthy(&ex.get(&msg, "isError")?)? { " is-error=\"true\"" } else { "" };
                    ex.push_lazy(&result, || {
                        let mut note = WireString::from("<stale-tool-result tool=\"");
                        note = concat(&note, &ex.heap.to_string(&ex.get(&msg, "toolName")?, ex.runtime)?);
                        note.append_str("\" id=\"");
                        note = concat(&note, &ex.heap.to_string(&ex.get(&msg, "toolCallId")?, ex.runtime)?);
                        note.append_str(&format!("\"{error}>\n"));
                        let joined = ex.method_lazy(&parts, "join", || Ok((&[ex.text("\n")?]).to_vec()))?;
                        note = concat(&note, &ex.heap.to_string(&joined, ex.runtime)?);
                        note.append_str("\n</stale-tool-result>");
                        ex.object([("role", ex.text("user")?), ("content", ex.text(note)?), ("timestamp", timestamp)])
                    })?;
                }
            }
        } else {
            flush(&timestamp)?;
            ex.push_lazy(&result, || Ok(msg))?;
        }
        index += 1.0;
    }
    flush(&ex.heap.number(crate::types::now_ms() as f64)?)?;
    Ok(result)
}

/// Complete original-container pipeline for an OpenAI-family target. The two
/// supplied source functions are retained heap callbacks, never Rust closures.
pub(crate) fn transform_original_openai_messages(
    builder: &GraphBuilder,
    messages: &ValueHandle,
    model: &ValueHandle,
    policy: &OpenAIHistoryPolicy,
    normalizer: &ValueHandle,
    demoter: &ValueHandle,
    max_id: usize,
    suffix: &str,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap: builder.0.heap(), runtime: builder.0.runtime() };
    ex.heap.adopt(messages)?;
    // Credential redaction's default-disabled identity path is maintained.
    // The mutable opt-in lexical module binding is a separately owned dependency.
    let sanitized = nsanitize(ex, messages)?;
    let id_map = ex.map()?;
    let composite_map = ex.map()?;
    let mut latest = ex.length(&sanitized)? - 1.0;
    while latest >= 0.0 {
        let msg = ex.index(&sanitized, latest)?;
        if ex.eq(&msg, "role", "assistant")? && !ntruncated(ex, &msg)? {
            break;
        }
        latest -= 1.0;
    }
    let invalid = ex.set()?;
    if policy.prefix_binding {
        let mut rewrite = ex.heap.undefined()?;
        let mut index = 0.0;
        while index < ex.length(&sanitized)? {
            let msg = ex.index(&sanitized, index)?;
            let candidate = if ex.eq(&msg, "role", "user")? && !ex.undefined(&ex.get(&msg, "historyRewriteAt")?)? {
                Some(ex.get(&msg, "historyRewriteAt")?)
            } else if ex.eq(&msg, "role", "toolResult")? && !ex.undefined(&ex.get(&msg, "prunedAt")?)? {
                Some(ex.get(&msg, "prunedAt")?)
            } else {
                None
            };
            if let Some(candidate) = candidate {
                rewrite = if ex.undefined(&rewrite)? {
                    candidate
                } else {
                    let left = ex.heap.to_number(&rewrite, ex.runtime)?;
                    let right = ex.heap.to_number(&candidate, ex.runtime)?;
                    ex.heap.number(if left.is_nan() || right.is_nan() {
                        f64::NAN
                    } else if left == 0.0 && right == 0.0 {
                        if left.is_sign_positive() || right.is_sign_positive() { 0.0 } else { -0.0 }
                    } else {
                        left.max(right)
                    })?
                };
            } else if ex.eq(&msg, "role", "assistant")? && !ex.undefined(&rewrite)? {
                let le = crate::schema_validator::graph_values_compare_with_execution(
                    crate::graph_values::GraphExecution::new(ex.heap, ex.runtime),
                    &ex.get(&msg, "timestamp")?,
                    &rewrite,
                    "<=",
                )?;
                if le {
                    ex.method_lazy(&invalid, "add", || Ok((&[ex.heap.number(index)?]).to_vec()))?;
                }
            }
            index += 1.0;
        }
    }
    let controls = ex.object([("readsForeignThinking", ex.heap.boolean(policy.reads_foreign_thinking)?)])?;
    let normalized = ex.apply(
        &sanitized,
        "map",
        &N_NORMALIZE_MESSAGE,
        &[model.clone(), controls, normalizer.clone(), demoter.clone(), id_map, composite_map, invalid],
    )?;
    let scope = norigin(ex, &normalized)?;
    let seen = ex.map()?;
    let pending = ex.map()?;
    let dedup = ex.apply(
        &normalized,
        "map",
        &N_DEDUP_MESSAGE,
        &[seen, pending, scope.clone(), ex.heap.number(max_id as f64)?, ex.text(suffix)?],
    )?;
    nrepair(ex, &sanitized, &dedup, &scope)
}
