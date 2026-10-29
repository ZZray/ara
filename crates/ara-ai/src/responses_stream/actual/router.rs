//! Fixed original-value endpoint routing. Raw items and blocks retain identity.
//! OMP openai-shared.ts 2803-3440, codex 2159-2569, 596f2da (MIT).
use super::leaves::GraphResponsesLeaves;
use super::{GraphResponsesError, GraphResponsesOptions};
use crate::actual_provider::{concat, new_assistant, trim_empty};
use crate::graph_values::GraphValueFactory;
use crate::{ActualAssistantEvent, AssistantMessageEvent, ContextRecoveryEvidence, Model, StopReason};
use ara_rpc::{
    WireString, WireValue,
    value_heap::{ValueError, ValueHandle, ValueSnapshot},
};
use std::{collections::HashMap, sync::Arc};

type Result<T> = std::result::Result<T, GraphResponsesError>;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GraphResponsesFlavor {
    Compatible,
    Codex,
}
#[derive(Clone)]
struct OpenItem {
    item: ValueHandle,
    block: Option<ValueHandle>,
    kind: WireString,
    index: ValueHandle,
    output_index: Option<u64>,
    item_id: Option<WireString>,
    call_id: Option<WireString>,
    prefixed_call_id: Option<WireString>,
}
/// One response retains original item/block handles, never a typed message.
pub struct GraphResponsesStreamState {
    pub root: ValueHandle,
    pub content: ValueHandle,
    pub native_items: ValueHandle,
    pub terminal: bool,
    pub stop: StopReason,
    pub replay_unsafe: bool,
    pub(crate) context_recovery_evidence: ContextRecoveryEvidence,
    leaves: GraphResponsesLeaves,
    flavor: GraphResponsesFlavor,
    model: Model,
    options: GraphResponsesOptions,
    entries: HashMap<usize, OpenItem>,
    by_index: HashMap<u64, usize>,
    by_id: HashMap<WireString, usize>,
    by_alias: HashMap<WireString, usize>,
    order: Vec<usize>,
    last: Option<usize>,
    identifierless: Option<usize>,
    next: usize,
    saw_first: bool,
    web_search: bool,
    cutoff: super::cutoff::Cutoff,
    pending_summary: HashMap<usize, Vec<WireString>>,
}
fn number_key(value: f64) -> u64 {
    if value == 0.0 {
        0
    } else if value.is_nan() {
        f64::NAN.to_bits()
    } else {
        value.to_bits()
    }
}
/// Prefix grammar only distinguishes ASCII structural/control units. Every
/// other UTF-16 unit is an ordinary non-structural unit, including a lone
/// surrogate inside a JSON string. This observation never rewrites the buffer.
fn classify_units(value: &WireString) -> crate::json::JsonPrefixState {
    let ascii: String =
        value.units().iter().map(|unit| if *unit < 128 { char::from(*unit as u8) } else { '\u{80}' }).collect();
    crate::json::classify_json_prefix(&ascii)
}
impl GraphResponsesStreamState {
    pub fn new(
        model: &Model,
        factory: Arc<GraphValueFactory>,
        flavor: GraphResponsesFlavor,
        options: GraphResponsesOptions,
    ) -> std::result::Result<Self, ValueError> {
        let leaves = GraphResponsesLeaves::new(factory)?;
        let (root, content) = new_assistant(&leaves.builder, model)?;
        let native_items = leaves.builder.0.heap().array_with_length(0)?;
        let cutoff = super::cutoff::Cutoff::new(&leaves.builder)?;
        Ok(Self {
            root,
            content,
            native_items,
            terminal: false,
            stop: StopReason::Stop,
            replay_unsafe: false,
            context_recovery_evidence: ContextRecoveryEvidence::ContentOnly,
            leaves,
            flavor,
            model: model.clone(),
            options,
            entries: HashMap::new(),
            by_index: HashMap::new(),
            by_id: HashMap::new(),
            by_alias: HashMap::new(),
            order: Vec::new(),
            last: None,
            identifierless: None,
            next: 0,
            saw_first: false,
            web_search: false,
            cutoff,
            pending_summary: HashMap::new(),
        })
    }
    pub fn factory(&self) -> &Arc<GraphValueFactory> {
        &self.leaves.builder.0
    }
    pub fn parse_frame(&self, data: &str) -> std::result::Result<ValueHandle, ValueError> {
        let wire =
            WireValue::parse(data).map_err(|_| ValueError::TypeError("Malformed Responses SSE JSON frame".into()))?;
        self.leaves.builder.import_wire(wire)
    }
    fn get(&self, source: &ValueHandle, key: &str) -> std::result::Result<ValueHandle, ValueError> {
        self.leaves.builder.get(source, key)
    }
    fn kind(&self, source: &ValueHandle) -> std::result::Result<WireString, ValueError> {
        Ok(self.leaves.builder.optional_string(source, "type")?.unwrap_or_else(|| "".into()))
    }
    fn text_or_empty(&self, source: &ValueHandle, key: &str) -> std::result::Result<WireString, ValueError> {
        let value = self.get(source, key)?;
        match self.factory().heap().snapshot(&value)? {
            ValueSnapshot::String(text) => Ok(text),
            _ => Ok("".into()),
        }
    }
    fn optional_text(&self, source: &ValueHandle, key: &str) -> std::result::Result<Option<WireString>, ValueError> {
        Ok(match self.factory().heap().snapshot(&self.get(source, key)?)? {
            ValueSnapshot::String(text) => Some(text),
            _ => None,
        })
    }
    fn truthy(&self, value: &ValueHandle) -> std::result::Result<bool, ValueError> {
        self.factory().heap().truthy(value)
    }
    fn output_index(&self, event: &ValueHandle) -> std::result::Result<Option<u64>, ValueError> {
        Ok(match self.factory().heap().snapshot(&self.get(event, "output_index")?)? {
            ValueSnapshot::Number(value) if self.flavor == GraphResponsesFlavor::Compatible => Some(number_key(value)),
            ValueSnapshot::Number(value) if value.is_finite() => Some(number_key(value.trunc())),
            _ => None,
        })
    }
    fn has_key(&self, event: &ValueHandle) -> std::result::Result<bool, ValueError> {
        Ok(self.output_index(event)?.is_some()
            || !matches!(self.factory().heap().snapshot(&self.get(event, "item_id")?)?, ValueSnapshot::Undefined))
    }
    fn lookup(&self, event: &ValueHandle, alias: Option<&str>) -> std::result::Result<Option<usize>, ValueError> {
        let index = self.output_index(event)?;
        let id = self.optional_text(event, "item_id")?;
        if self.flavor == GraphResponsesFlavor::Codex {
            if let Some(id) = id.filter(|id| !id.is_empty()) {
                return Ok(self.by_id.get(&id).copied());
            }
            if let Some(index) = index {
                return Ok(self.by_index.get(&index).copied());
            }
            return Ok(self.last);
        }
        if let Some(found) = index.and_then(|index| self.by_index.get(&index)).copied() {
            return Ok(Some(found));
        }
        if let Some(id) = id.filter(|id| !id.is_empty()) {
            if let Some(kind) = alias {
                if let Some(found) = self
                    .by_alias
                    .get(&id)
                    .copied()
                    .filter(|found| self.entries.get(found).is_some_and(|entry| entry.kind.equals_ascii(kind)))
                {
                    return Ok(Some(found));
                }
            }
            if let Some(found) = self.by_id.get(&id) {
                return Ok(Some(*found));
            }
        }
        Ok(if self.has_key(event)? { None } else { self.last })
    }
    fn starts_object(&self, value: &ValueHandle) -> std::result::Result<bool, ValueError> {
        let ValueSnapshot::String(value) = self.factory().heap().snapshot(value)? else { return Ok(false) };
        Ok(value.units().iter().find(|unit| !matches!(**unit, 9 | 10 | 13 | 32)).is_some_and(|unit| *unit == 123))
    }
    fn unfinished(&self, entry: &OpenItem) -> std::result::Result<bool, ValueError> {
        if !entry.kind.equals_ascii("function_call") {
            return Ok(false);
        }
        let Some(block) = &entry.block else { return Ok(false) };
        Ok(!self.truthy(&self.leaves.get_symbol(block, &self.leaves.arguments_done)?)?)
    }
    fn later_function(&self, key: usize) -> std::result::Result<bool, ValueError> {
        let mut passed = false;
        for candidate in &self.order {
            if *candidate == key {
                passed = true;
                continue;
            }
            if passed && self.unfinished(&self.entries[candidate])? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn advance(&self, event: &ValueHandle, entry: &OpenItem) -> std::result::Result<bool, ValueError> {
        let delta = self.get(event, "delta")?;
        if self.has_key(event)? || !self.starts_object(&delta)? || !entry.kind.equals_ascii("function_call") {
            return Ok(false);
        }
        let Some(block) = &entry.block else { return Ok(false) };
        let partial = self.leaves.builder.string(&self.leaves.get_symbol(block, &self.leaves.partial)?)?;
        if trim_empty(&partial) {
            return Ok(false);
        }
        if classify_units(&partial) != crate::json::JsonPrefixState::Prefix {
            return Ok(true);
        }
        let combined = concat(&partial, &self.leaves.builder.string(&delta)?);
        Ok(classify_units(&combined) == crate::json::JsonPrefixState::Invalid)
    }
    fn function_entry(&mut self, event: &ValueHandle) -> std::result::Result<Option<usize>, ValueError> {
        if self.flavor == GraphResponsesFlavor::Codex || self.has_key(event)? {
            return self.lookup(event, Some("function_call"));
        }
        let can_continue =
            matches!(self.factory().heap().snapshot(&self.get(event, "delta")?)?, ValueSnapshot::String(_));
        if can_continue {
            if let Some(key) = self.identifierless {
                if let Some(entry) = self.entries.get(&key) {
                    if self.unfinished(entry)? && !(self.advance(event, entry)? && self.later_function(key)?) {
                        return Ok(Some(key));
                    }
                } else {
                    self.identifierless = None;
                }
            }
        }
        let mut skipped = false;
        for key in &self.order {
            let entry = &self.entries[key];
            if self.unfinished(entry)? {
                if self.advance(event, entry)? && self.later_function(*key)? {
                    skipped = true;
                    continue;
                }
                let key = *key;
                if can_continue {
                    self.identifierless = Some(key);
                }
                return Ok(Some(key));
            }
        }
        if skipped && self.starts_object(&self.get(event, "delta")?)? {
            return Ok(None);
        }
        Ok(self.last.filter(|key| self.entries.get(key).is_some_and(|entry| entry.kind.equals_ascii("function_call"))))
    }
    fn content_index(&self, entry: &OpenItem) -> std::result::Result<ValueHandle, ValueError> {
        if self.flavor == GraphResponsesFlavor::Codex {
            return Ok(entry.index.clone());
        }
        let block = entry.block.as_ref().ok_or(ValueError::TypeError("Missing original block".into()))?;
        self.original_block_index(block)
    }
    fn original_block_index(&self, block: &ValueHandle) -> std::result::Result<ValueHandle, ValueError> {
        let method = self.get(&self.content, "indexOf")?;
        // The source passes the exact method result into the event, including
        // -1 or any value returned by a programmable Native indexOf method.
        self.factory().heap().call(&method, &self.content, &[block.clone()], self.factory().runtime())
    }
    fn appended_index(&self) -> std::result::Result<ValueHandle, ValueError> {
        // `output.content.length - 1`: original Get, then Number subtraction.
        // Do not apply ToLength, cast to usize, or reject the source's -1.
        let length = self.get(&self.content, "length")?;
        let length = self.factory().heap().to_number(&length, self.factory().runtime())?;
        self.factory().heap().number(length - 1.0)
    }
    fn call_id(&self, item: &ValueHandle) -> std::result::Result<WireString, ValueError> {
        let call = self.leaves.text(item, "call_id")?;
        let id = self.optional_text(item, "id")?.filter(|id| !id.is_empty());
        let id = match id {
            Some(id) => id,
            None => {
                let hash = self
                    .options
                    .history_id_hash
                    .as_ref()
                    .ok_or(ValueError::UnsupportedOperation("Fixed Bun hash for Responses tool item id"))?
                    .hash_string_base36(&call)?;
                concat(&"fc_".into(), &hash)
            }
        };
        Ok(concat(&concat(&call, &"|".into()), &id))
    }
    fn signature(&self, item: &ValueHandle) -> std::result::Result<ValueHandle, ValueError> {
        let builder = &self.leaves.builder;
        let signature = builder.object([("v", self.factory().heap().number(1.0)?), ("id", self.get(item, "id")?)])?;
        let phase = self.get(item, "phase")?;
        if self.truthy(&phase)? {
            if self.flavor == GraphResponsesFlavor::Compatible
                || matches!(self.factory().heap().snapshot(&phase)?,ValueSnapshot::String(ref value) if value.equals_ascii("commentary")||value.equals_ascii("final_answer"))
            {
                builder.define(&signature, "phase", phase)?;
            }
        }
        builder.text(builder.plain_json(&signature)?)
    }
    fn make_tool(&self, item: &ValueHandle, arguments: ValueHandle) -> std::result::Result<ValueHandle, ValueError> {
        let builder = &self.leaves.builder;
        let kind = self.kind(item)?;
        let name = if kind.equals_ascii("computer_call") { builder.text("computer")? } else { self.get(item, "name")? };
        let block = builder.object([
            ("type", builder.text("toolCall")?),
            ("id", builder.text(self.call_id(item)?)?),
            ("name", name),
            ("arguments", arguments),
        ])?;
        if kind.equals_ascii("custom_tool_call") {
            builder.define(&block, "customWireName", self.get(item, "name")?)?;
        }
        if kind.equals_ascii("computer_call") {
            builder.define(&block, "providerMetadata", self.leaves.computer_metadata(item)?)?;
        }
        Ok(block)
    }
    fn register(
        &mut self,
        event: &ValueHandle,
        item: ValueHandle,
        block: Option<ValueHandle>,
        index: ValueHandle,
    ) -> std::result::Result<usize, ValueError> {
        let key = self.next;
        self.next += 1;
        let kind = self.kind(&item)?;
        let id = self.optional_text(&item, "id")?;
        let call = if kind.equals_ascii("function_call") || kind.equals_ascii("custom_tool_call") {
            self.optional_text(&item, "call_id")?
        } else {
            None
        };
        let prefixed = call.as_ref().filter(|call| !call.is_empty()).map(|call| concat(&"fc_".into(), call));
        let output_index = self.output_index(event)?;
        if let Some(index) = output_index {
            self.by_index.insert(index, key);
        }
        if let Some(id) = id.as_ref().filter(|id| !id.is_empty()) {
            self.by_id.insert(id.clone(), key);
        }
        if self.flavor == GraphResponsesFlavor::Compatible {
            if let Some(call) = call.as_ref().filter(|call| !call.is_empty() && Some(*call) != id.as_ref()) {
                self.by_id.insert(call.clone(), key);
            }
            if let Some(alias) =
                prefixed.as_ref().filter(|alias| Some(*alias) != id.as_ref() && Some(*alias) != call.as_ref())
            {
                self.by_alias.insert(alias.clone(), key);
            }
        }
        self.entries.insert(
            key,
            OpenItem { item, block, kind, index, output_index, item_id: id, call_id: call, prefixed_call_id: prefixed },
        );
        self.order.push(key);
        self.last = Some(key);
        Ok(key)
    }
    fn close(&mut self, event: &ValueHandle, item: &ValueHandle, key: Option<usize>) -> Result<()> {
        if self.flavor == GraphResponsesFlavor::Codex {
            if let Some(key) = key {
                if let Some(entry) = self.entries.get(&key) {
                    if let Some(index) = entry.output_index {
                        self.by_index.remove(&index);
                    }
                    if let Some(id) = &entry.item_id {
                        self.by_id.remove(id);
                    }
                }
            }
        } else {
            // Fixed Generic close deletes the final event's keys, even if the
            // endpoint changed IDs since added. A retained alias continues to
            // refer to the same original entry; removal from the ordered open
            // list cannot make that alias point to a deallocated Rust record.
            if let Some(index) = self.output_index(event)? {
                self.by_index.remove(&index);
            }
            let id = self.optional_text(item, "id")?.filter(|value| !value.is_empty());
            if let Some(id) = &id {
                self.by_id.remove(id);
            }
            let kind = self.kind(item)?;
            if kind.equals_ascii("function_call")
                || kind.equals_ascii("custom_tool_call")
                || kind.equals_ascii("computer_call")
            {
                if let Some(call) = self.optional_text(item, "call_id")?.filter(|value| !value.is_empty()) {
                    if id.as_ref() != Some(&call) {
                        self.by_id.remove(&call);
                    }
                    if !kind.equals_ascii("computer_call") {
                        let alias = concat(&"fc_".into(), &call);
                        if id.as_ref() != Some(&alias) && alias != call && self.by_alias.get(&alias).copied() == key {
                            self.by_alias.remove(&alias);
                        }
                    }
                }
            }
        }
        if let Some(key) = key {
            self.order.retain(|candidate| *candidate != key);
            if self.last == Some(key) {
                self.last = None;
            }
            if self.identifierless == Some(key) {
                self.identifierless = None;
            }
            self.pending_summary.remove(&key);
        }
        Ok(())
    }
    pub(crate) fn veto_context_recovery(&mut self, evidence: ContextRecoveryEvidence) {
        self.context_recovery_evidence = self.context_recovery_evidence.merge(evidence);
    }
    fn recovery_kind(&self, value: &ValueHandle) -> std::result::Result<WireString, ValueError> {
        if self.leaves.builder.is_nullish(value)? {
            return Ok("".into());
        }
        Ok(self.optional_text(value, "type")?.unwrap_or_else(|| "".into()))
    }
    fn content_only_part(&self, part: &ValueHandle) -> std::result::Result<bool, ValueError> {
        let kind = self.recovery_kind(part)?;
        let field = if ["output_text", "summary_text", "reasoning_text"].iter().any(|k| kind.equals_ascii(k)) {
            "text"
        } else if kind.equals_ascii("refusal") {
            "refusal"
        } else {
            return Ok(false);
        };
        Ok(matches!(self.factory().heap().snapshot(&self.get(part, field)?)?, ValueSnapshot::String(_)))
    }
    fn observe_context_recovery_item(&mut self, item: &ValueHandle) -> std::result::Result<(), ValueError> {
        let kind = self.recovery_kind(item)?;
        let mut content_only = kind.equals_ascii("reasoning");
        if kind.equals_ascii("message") {
            let content = self.get(item, "content")?;
            content_only = matches!(self.factory().heap().snapshot(&content)?, ValueSnapshot::Undefined);
            if matches!(self.factory().heap().snapshot(&content)?, ValueSnapshot::Array) {
                content_only = true;
                for part in self.leaves.builder.members(&content)? {
                    let kind = self.recovery_kind(&part)?;
                    if !(kind.equals_ascii("output_text") || kind.equals_ascii("refusal"))
                        || !self.content_only_part(&part)?
                    {
                        content_only = false;
                        break;
                    }
                }
            }
        }
        if !content_only {
            self.veto_context_recovery(ContextRecoveryEvidence::NativeOutput);
        }
        Ok(())
    }
    // Inspect original handles before routing/projection can omit native facts.
    // These observations only derive controls; they never rebuild the event.
    fn observe_context_recovery_event(
        &mut self,
        event: &ValueHandle,
        kind: &WireString,
    ) -> std::result::Result<(), ValueError> {
        let item = self.get(event, "item")?;
        let has_item = !matches!(self.factory().heap().snapshot(&item)?, ValueSnapshot::Undefined);
        if has_item {
            self.observe_context_recovery_item(&item)?;
        }
        let part = self.get(event, "part")?;
        let has_part = !matches!(self.factory().heap().snapshot(&part)?, ValueSnapshot::Undefined);
        if has_part && !self.content_only_part(&part)? {
            self.veto_context_recovery(ContextRecoveryEvidence::NativeOutput);
        }
        let response = self.get(event, "response")?;
        if !self.leaves.builder.is_nullish(&response)? {
            let output = self.get(&response, "output")?;
            match self.factory().heap().snapshot(&output)? {
                ValueSnapshot::Undefined => {}
                ValueSnapshot::Array => {
                    for item in self.leaves.builder.members(&output)? {
                        self.observe_context_recovery_item(&item)?;
                    }
                }
                _ => self.veto_context_recovery(ContextRecoveryEvidence::NativeOutput),
            }
        }
        let content_only = if kind.equals_ascii("response.output_item.added")
            || kind.equals_ascii("response.output_item.done")
        {
            has_item
        } else if kind.equals_ascii("response.content_part.added") || kind.equals_ascii("response.content_part.done") {
            let part_kind = self.recovery_kind(&part)?;
            has_part
                && (part_kind.equals_ascii("output_text") || part_kind.equals_ascii("refusal"))
                && self.content_only_part(&part)?
        } else {
            [
                "response.created",
                "response.in_progress",
                "response.queued",
                "response.completed",
                "response.incomplete",
                "response.done",
                "response.failed",
                "error",
                "response.output_text.delta",
                "response.output_text.done",
                "response.refusal.delta",
                "response.refusal.done",
                "response.reasoning_summary_part.added",
                "response.reasoning_summary_part.done",
                "response.reasoning_summary_text.delta",
                "response.reasoning_summary_text.done",
                "response.reasoning_text.delta",
                "response.reasoning_text.done",
            ]
            .iter()
            .any(|expected| kind.equals_ascii(expected))
        };
        if !content_only {
            self.veto_context_recovery(ContextRecoveryEvidence::NativeOutput);
        }
        Ok(())
    }
    pub(crate) fn failure_evidence(
        &self,
        cause: &crate::ProviderError,
        event: &ValueHandle,
    ) -> std::result::Result<crate::retry_classification::ProviderFailureEvidence, ValueError> {
        let response = self.get(event, "response")?;
        let body = if matches!(self.factory().heap().snapshot(&response)?, ValueSnapshot::Undefined) {
            event.clone()
        } else {
            response
        };
        let structured = if self.leaves.builder.is_nullish(&body)? {
            body
        } else {
            let error = self.get(&body, "error")?;
            if matches!(self.factory().heap().snapshot(&error)?, ValueSnapshot::Undefined) { body } else { error }
        };
        // Only admission/error scalar facts enter derived serde metadata. No
        // transcript, item, part, arguments or whole Graph is exported.
        let mut fields = serde_json::Map::new();
        if !self.leaves.builder.is_nullish(&structured)? {
            for key in ["code", "type"] {
                if let Some(value) = self.optional_text(&structured, key)? {
                    let value = value.to_utf8().map_err(|_| {
                        ValueError::UnsupportedOperation("Provider error evidence contains a lone surrogate")
                    })?;
                    fields.insert(key.into(), serde_json::Value::String(value));
                }
            }
        }
        Ok(crate::retry_classification::ProviderFailureEvidence::from_error_envelope(
            cause,
            &serde_json::json!({"error": fields}),
            false,
        ))
    }
    pub fn handle(&mut self, event: &ValueHandle, events: &mut Vec<ActualAssistantEvent>) -> Result<()> {
        self.factory().heap().adopt(event)?;
        let kind = self.kind(event)?;
        self.observe_context_recovery_event(event, &kind)?;
        if kind.equals_ascii("response.created") {
            let response = self.get(event, "response")?;
            self.leaves.builder.put(&self.root, "responseId", self.get(&response, "id")?)?;
        } else if kind.equals_ascii("response.output_item.added") {
            if !self.saw_first {
                self.saw_first = true;
                if let Some(hook) = &self.options.hooks {
                    hook.first_token()?;
                }
            }
            self.add_item(event, events)?;
        } else if kind.equals_ascii("response.output_item.done") {
            self.finish_item(event, events)?;
        } else if kind.equals_ascii("response.function_call_arguments.delta")
            || kind.equals_ascii("response.function_call_arguments.done")
        {
            if let Some(key) = self.function_entry(event)? {
                let entry = self.entries[&key].clone();
                if entry.kind.equals_ascii("function_call") {
                    if let Some(block) = &entry.block {
                        if kind.equals_ascii("response.function_call_arguments.delta") {
                            self.leaves.function_delta(
                                block,
                                self.get(event, "delta")?,
                                &self.root,
                                self.content_index(&entry)?,
                                events,
                            )?;
                        } else {
                            self.leaves.function_done(block, self.get(event, "arguments")?)?;
                            if self.flavor == GraphResponsesFlavor::Compatible {
                                self.leaves.set_symbol(
                                    block,
                                    &self.leaves.arguments_done,
                                    self.factory().heap().boolean(true)?,
                                )?;
                            }
                        }
                    }
                }
            }
        } else if kind.equals_ascii("response.custom_tool_call_input.delta")
            || kind.equals_ascii("response.custom_tool_call_input.done")
        {
            if let Some(key) = self.lookup(event, Some("custom_tool_call"))? {
                let entry = self.entries[&key].clone();
                if entry.kind.equals_ascii("custom_tool_call") {
                    if let Some(block) = &entry.block {
                        if kind.equals_ascii("response.custom_tool_call_input.delta") {
                            self.leaves.custom_delta(
                                block,
                                self.get(event, "delta")?,
                                &self.root,
                                self.content_index(&entry)?,
                                events,
                            )?;
                        } else {
                            self.leaves.custom_done(block, self.get(event, "input")?)?;
                            if self.flavor == GraphResponsesFlavor::Compatible {
                                self.leaves.set_symbol(
                                    block,
                                    &self.leaves.arguments_done,
                                    self.factory().heap().boolean(true)?,
                                )?;
                            }
                        }
                    }
                }
            }
        } else if kind.equals_ascii("response.completed")
            || kind.equals_ascii("response.incomplete")
            || kind.equals_ascii("response.done")
        {
            self.finish_response(event)?;
        } else if kind.equals_ascii("error") || kind.equals_ascii("response.failed") {
            let response = if kind.equals_ascii("error") { event.clone() } else { self.get(event, "response")? };
            if kind.equals_ascii("response.failed") {
                self.apply_usage(&response)?;
            }
            return Err(crate::error::ProviderError::Stream(self.response_error(&response)?).into());
        } else {
            self.content_event(event, &kind, events)?;
        }
        Ok(())
    }
    fn add_item(&mut self, event: &ValueHandle, events: &mut Vec<ActualAssistantEvent>) -> Result<()> {
        let item = self.get(event, "item")?;
        let kind = self.kind(&item)?;
        let b = &self.leaves.builder;
        let block = if kind.equals_ascii("reasoning") {
            Some(b.object([
                ("type", b.text("thinking")?),
                ("thinking", b.text("")?),
                ("itemId", self.get(&item, "id")?),
            ])?)
        } else if kind.equals_ascii("message") {
            Some(b.object([
                ("type", b.text("text")?),
                ("text", b.text("")?),
                ("textSignature", self.signature(&item)?),
            ])?)
        } else if kind.equals_ascii("function_call")
            || kind.equals_ascii("custom_tool_call")
            || kind.equals_ascii("computer_call")
        {
            let arguments = if kind.equals_ascii("custom_tool_call") {
                let input = self.get(&item, "input")?;
                let input = if b.is_nullish(&input)? { b.text("")? } else { input };
                b.object([("input", input)])?
            } else {
                self.factory().heap().object()?
            };
            let block = self.make_tool(&item, arguments)?;
            if !kind.equals_ascii("computer_call") {
                let field = if kind.equals_ascii("function_call") { "arguments" } else { "input" };
                let initial = self.get(&item, field)?;
                let initial = if kind.equals_ascii("custom_tool_call") {
                    if b.is_nullish(&initial)? { b.text("")? } else { initial }
                } else if self.truthy(&initial)? {
                    initial
                } else {
                    b.text("")?
                };
                self.leaves.set_symbol(&block, &self.leaves.partial, initial)?;
            }
            Some(block)
        } else {
            None
        };
        if block.is_none() && self.flavor == GraphResponsesFlavor::Compatible {
            return Ok(());
        }
        if let Some(block) = &block {
            b.push(&self.content, block.clone())?;
        }
        let index = if self.flavor == GraphResponsesFlavor::Codex && block.is_some() {
            self.appended_index()?
        } else {
            self.factory().heap().number(-1.0)?
        };
        let key = self.register(event, item, block, index)?;
        let entry = &self.entries[&key];
        if entry.block.is_some() {
            let index = self.content_index(entry)?;
            events.push(if kind.equals_ascii("reasoning") {
                AssistantMessageEvent::ThinkingStart { content_index: index, partial: self.root.clone() }
            } else if kind.equals_ascii("message") {
                AssistantMessageEvent::TextStart { content_index: index, partial: self.root.clone() }
            } else {
                AssistantMessageEvent::ToolcallStart { content_index: index, partial: self.root.clone() }
            });
        }
        Ok(())
    }
    fn summary_index(&self, event: &ValueHandle) -> std::result::Result<u32, ValueError> {
        let value = self.get(event, "summary_index")?;
        match self.factory().heap().snapshot(&value)? {
            ValueSnapshot::Undefined => Ok(0),
            ValueSnapshot::Number(index)
                if index >= 0.0 && index.is_finite() && index.fract() == 0.0 && index <= u32::MAX as f64 =>
            {
                Ok(index as u32)
            }
            _ => Err(ValueError::UnsupportedOperation("Native summary index cannot be projected into an array length")),
        }
    }
    fn content_event(
        &mut self,
        event: &ValueHandle,
        kind: &WireString,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<()> {
        if ![
            "response.reasoning_summary_part.added",
            "response.reasoning_summary_text.delta",
            "response.reasoning_summary_text.done",
            "response.reasoning_summary_part.done",
            "response.reasoning_text.delta",
            "response.content_part.added",
            "response.output_text.delta",
            "response.refusal.delta",
        ]
        .iter()
        .any(|expected| kind.equals_ascii(expected))
        {
            return Ok(());
        }
        let Some(key) = self.lookup(event, None)? else { return Ok(()) };
        let entry = self.entries[&key].clone();
        let Some(block) = &entry.block else { return Ok(()) };
        if entry.kind.equals_ascii("reasoning") {
            if self.flavor == GraphResponsesFlavor::Codex {
                if kind.equals_ascii("response.reasoning_summary_part.added") && self.options.sequential_cutoff {
                    return Ok(());
                }
                if kind.equals_ascii("response.reasoning_summary_text.delta") && self.options.sequential_cutoff {
                    let delta = self.text_or_empty(event, "delta")?;
                    if !delta.is_empty() {
                        self.pending_summary.entry(key).or_default().push(delta);
                    }
                    return Ok(());
                }
                if kind.equals_ascii("response.reasoning_summary_text.done") {
                    if !self.options.sequential_cutoff {
                        return Ok(());
                    }
                    self.pending_summary.remove(&key);
                    let text = self.text_or_empty(event, "text")?;
                    let summary_index = self.summary_index(event)?;
                    let empty_block = self.leaves.text(block, "thinking")?.is_empty();
                    let delta = self.cutoff.done(&self.leaves.builder, text, summary_index, empty_block)?;
                    if !delta.is_empty() {
                        self.leaves.thinking_delta(&self.root, block, delta, self.content_index(&entry)?, events)?;
                    }
                    return Ok(());
                }
                if kind.equals_ascii("response.reasoning_summary_part.done") && self.options.sequential_cutoff {
                    if let Some(pending) = self.pending_summary.get_mut(&key) {
                        pending.push("\n\n".into());
                    }
                    return Ok(());
                }
            }
            if kind.equals_ascii("response.reasoning_summary_part.added") {
                self.leaves.append_summary_part(&entry.item, self.get(event, "part")?)?;
            } else if kind.equals_ascii("response.reasoning_summary_text.delta") {
                let delta = self.leaves.text(event, "delta")?;
                let index = self.content_index(&entry)?;
                let summary_index =
                    if self.flavor == GraphResponsesFlavor::Compatible { self.summary_index(event)? } else { 0 };
                self.leaves.summary_delta(&entry.item, block, delta, summary_index, &self.root, index, events)?;
            } else if kind.equals_ascii("response.reasoning_summary_text.done") {
                self.leaves.summary_text_done(
                    &entry.item,
                    block,
                    self.leaves.text(event, "text")?,
                    self.summary_index(event)?,
                    &self.root,
                    self.content_index(&entry)?,
                    events,
                )?;
            } else if kind.equals_ascii("response.reasoning_summary_part.done") {
                self.leaves.summary_part_done(&entry.item, block, &self.root, self.content_index(&entry)?, events)?;
            } else if kind.equals_ascii("response.reasoning_text.delta") {
                self.leaves.thinking_delta(
                    &self.root,
                    block,
                    self.leaves.text(event, "delta")?,
                    self.content_index(&entry)?,
                    events,
                )?;
            }
        } else if entry.kind.equals_ascii("message") {
            if kind.equals_ascii("response.content_part.added") {
                self.leaves.append_message_part(&entry.item, self.get(event, "part")?)?;
            } else if kind.equals_ascii("response.output_text.delta") || kind.equals_ascii("response.refusal.delta") {
                let part = if kind.equals_ascii("response.output_text.delta") { "output_text" } else { "refusal" };
                self.leaves.message_delta(
                    &entry.item,
                    block,
                    self.leaves.text(event, "delta")?,
                    part,
                    &self.root,
                    self.content_index(&entry)?,
                    events,
                )?;
            }
        }
        Ok(())
    }
    fn finish_item(&mut self, event: &ValueHandle, events: &mut Vec<ActualAssistantEvent>) -> Result<()> {
        let original = self.get(event, "item")?;
        if self.flavor == GraphResponsesFlavor::Codex && self.leaves.builder.is_nullish(&original)? {
            return Ok(());
        }
        let item = self.factory().heap().structured_clone_json(&original, self.factory().runtime())?;
        if self.flavor == GraphResponsesFlavor::Compatible {
            if let Some(hooks) = &self.options.hooks {
                hooks.output_item_done(&item)?;
            }
        }
        let kind = self.kind(&item)?;
        if self.flavor == GraphResponsesFlavor::Codex
            && kind.equals_ascii("image_generation_call")
            && self.truthy(&self.get(&item, "result")?)?
        {
            self.leaves.builder.put(&item, "status", self.leaves.builder.text("completed")?)?;
        }
        self.leaves.builder.push(&self.native_items, item.clone())?;
        let key = if self.flavor == GraphResponsesFlavor::Codex {
            match self.optional_text(&item, "id")?.and_then(|id| self.by_id.get(&id).copied()) {
                Some(key) => Some(key),
                None => self.lookup(event, None)?,
            }
        } else {
            let lookup = self.leaves.builder.object([
                ("output_index", self.get(event, "output_index")?),
                ("item_id", {
                    let id = self.get(&item, "id")?;
                    if (kind.equals_ascii("function_call") || kind.equals_ascii("custom_tool_call"))
                        && self.leaves.builder.is_nullish(&id)?
                    {
                        self.get(&item, "call_id")?
                    } else {
                        id
                    }
                }),
            ])?;
            self.lookup(&lookup, None)?
        };
        let entry = key.and_then(|key| self.entries.get(&key)).cloned();
        let stored = entry.as_ref().and_then(|entry| entry.block.clone());
        // Codex evaluates this nullish fallback once before finalization.
        // A matched entry with no block keeps its stored -1; only a missing
        // entry observes output.content.length at this point.
        let codex_done_index = if self.flavor == GraphResponsesFlavor::Codex {
            Some(match &entry {
                Some(entry) => entry.index.clone(),
                None => self.appended_index()?,
            })
        } else {
            None
        };
        if kind.equals_ascii("reasoning") {
            let mut reasoning_block = None;
            if let Some(block) = stored {
                if self.kind(&block)?.equals_ascii("thinking") {
                    reasoning_block = Some(block);
                }
            }
            if reasoning_block.is_none() && self.flavor == GraphResponsesFlavor::Compatible {
                let found = crate::providers::openai_responses::actual::callbacks::apply(
                    &self.leaves.builder,
                    &self.content,
                    "find",
                    &crate::providers::openai_responses::actual::callbacks::THINKING_MATCH,
                    &[item.clone()],
                )?;
                if self.truthy(&found)? {
                    reasoning_block = Some(found);
                }
            }
            if let Some(block) = reasoning_block {
                if self.flavor == GraphResponsesFlavor::Codex {
                    if let Some(key) = key {
                        if let Some(pending) = self.pending_summary.remove(&key) {
                            for delta in pending {
                                self.leaves.thinking_delta(
                                    &self.root,
                                    &block,
                                    delta,
                                    self.content_index(entry.as_ref().unwrap())?,
                                    events,
                                )?;
                            }
                        }
                    }
                }
                let streamed = self.leaves.text(&block, "thinking")?;
                let text = if self.flavor == GraphResponsesFlavor::Codex && self.options.sequential_cutoff {
                    match self.cutoff.finalize(&self.leaves.builder, &item, &streamed)? {
                        Some(text) => text,
                        None => self.leaves.finalize_reasoning(&item, &streamed)?,
                    }
                } else {
                    self.leaves.finalize_reasoning(&item, &streamed)?
                };
                self.leaves.builder.put(&block, "thinking", self.leaves.builder.text(text.clone())?)?;
                self.leaves.builder.put(
                    &block,
                    "thinkingSignature",
                    self.leaves.builder.text(self.leaves.builder.plain_json(&item)?)?,
                )?;
                let index = if self.flavor == GraphResponsesFlavor::Compatible {
                    self.original_block_index(&block)?
                } else {
                    codex_done_index.as_ref().unwrap().clone()
                };
                events.push(AssistantMessageEvent::ThinkingEnd {
                    content_index: index,
                    content: text,
                    partial: self.root.clone(),
                });
            }
        } else if kind.equals_ascii("message") {
            if stored.is_some() || self.flavor == GraphResponsesFlavor::Compatible {
                let text = self.leaves.finalize_message(
                    &item,
                    &match &stored {
                        Some(block) => self.leaves.text(block, "text")?,
                        None => "".into(),
                    },
                )?;
                let block = if let Some(block) = stored {
                    block
                } else {
                    let block = self.leaves.builder.object([("type", self.leaves.builder.text("text")?)])?;
                    self.leaves.builder.push(&self.content, block.clone())?;
                    block
                };
                self.leaves.builder.put(&block, "text", self.leaves.builder.text(text.clone())?)?;
                self.leaves.builder.put(&block, "textSignature", self.signature(&item)?)?;
                let index = if let Some(index) = &codex_done_index {
                    index.clone()
                } else if let Some(entry) = &entry {
                    self.content_index(entry)?
                } else {
                    self.appended_index()?
                };
                events.push(AssistantMessageEvent::TextEnd {
                    content_index: index,
                    content: text,
                    partial: self.root.clone(),
                });
            }
        } else if kind.equals_ascii("function_call")
            || kind.equals_ascii("custom_tool_call")
            || kind.equals_ascii("computer_call")
        {
            let arguments = if kind.equals_ascii("computer_call") {
                self.factory().heap().object()?
            } else if kind.equals_ascii("custom_tool_call") {
                let partial = match &stored {
                    Some(block) => self.leaves.get_symbol(block, &self.leaves.partial)?,
                    None => self.factory().heap().undefined()?,
                };
                let input = if self.truthy(&partial)? {
                    partial
                } else {
                    let input = self.get(&item, "input")?;
                    if self.leaves.builder.is_nullish(&input)? { self.leaves.builder.text("")? } else { input }
                };
                self.leaves.builder.object([("input", input)])?
            } else {
                let done = match &stored {
                    Some(block) => self.truthy(&self.leaves.get_symbol(block, &self.leaves.arguments_done)?)?,
                    None => false,
                };
                if self.flavor == GraphResponsesFlavor::Compatible && done {
                    self.get(stored.as_ref().unwrap(), "arguments")?
                } else {
                    let raw = self.get(&item, "arguments")?;
                    let raw = if self.truthy(&raw)? {
                        raw
                    } else if self.flavor == GraphResponsesFlavor::Compatible {
                        let partial = match &stored {
                            Some(block) => self.leaves.get_symbol(block, &self.leaves.partial)?,
                            None => self.factory().heap().undefined()?,
                        };
                        if self.truthy(&partial)? { partial } else { self.leaves.builder.text("{}")? }
                    } else {
                        self.leaves.builder.text("{}")?
                    };
                    self.leaves.parse(&self.leaves.builder.string(&raw)?)?
                }
            };
            let call = self.make_tool(&item, arguments.clone())?;
            let index = if let Some(block) = stored {
                if kind.equals_ascii("computer_call") {
                    self.leaves.builder.put(&block, "id", self.get(&call, "id")?)?;
                    self.leaves.builder.put(&block, "providerMetadata", self.get(&call, "providerMetadata")?)?;
                } else {
                    self.leaves.builder.put(&block, "arguments", arguments)?;
                }
                self.leaves.clear_partial(&block)?;
                if let Some(index) = &codex_done_index {
                    index.clone()
                } else {
                    self.content_index(entry.as_ref().unwrap())?
                }
            } else if self.flavor == GraphResponsesFlavor::Compatible || kind.equals_ascii("computer_call") {
                self.leaves.builder.push(&self.content, call.clone())?;
                self.appended_index()?
            } else {
                codex_done_index.as_ref().unwrap().clone()
            };
            self.replay_unsafe = true;
            self.close(event, &item, key)?;
            events.push(AssistantMessageEvent::ToolcallEnd {
                content_index: index,
                tool_call: call,
                partial: self.root.clone(),
            });
            return Ok(());
        } else if kind.equals_ascii("web_search_call") {
            let status = self.get(&item, "status")?;
            if self.flavor == GraphResponsesFlavor::Compatible
                && (matches!(self.factory().heap().snapshot(&status)?, ValueSnapshot::Undefined)
                    || self.text_or_empty(&item, "status")?.equals_ascii("completed"))
            {
                self.web_search = true;
            }
        } else if kind.equals_ascii("image_generation_call")
            && self.text_or_empty(&item, "status")?.equals_ascii("completed")
            && self.truthy(&self.get(&item, "result")?)?
        {
            let result = self.leaves.text(&item, "result")?;
            let port = self
                .options
                .image_metadata
                .as_ref()
                .ok_or(ValueError::UnsupportedOperation("Responses Buffer/base64 image metadata Host consumer"))?;
            let mime = port.mime_from_base64(&result)?.unwrap_or_else(|| "image/png".into());
            let image = self.leaves.builder.object([
                ("type", self.leaves.builder.text("image")?),
                ("data", self.leaves.builder.text(result)?),
                ("mimeType", self.leaves.builder.text(mime)?),
            ])?;
            self.leaves.builder.push(&self.content, image.clone())?;
            events.push(AssistantMessageEvent::ImageEnd {
                content_index: self.appended_index()?,
                content: image,
                partial: self.root.clone(),
            });
        }
        self.close(event, &item, key)?;
        Ok(())
    }
    fn apply_usage(&self, response: &ValueHandle) -> std::result::Result<(), ValueError> {
        let usage = self.get(response, "usage")?;
        if self.leaves.builder.is_nullish(&usage)? {
            return Ok(());
        }
        super::accounting::populate(&self.leaves.builder, &self.root, &usage)?;
        if let Some(port) = &self.options.usage {
            port.populate_and_price(
                &self.model,
                &self.root,
                &usage,
                &self.get(response, "service_tier")?,
                self.options.request_service_tier.as_ref(),
                self.factory().as_ref(),
            )?;
        }
        Ok(())
    }
    fn executable_incomplete(&self) -> std::result::Result<bool, ValueError> {
        let mut seen = false;
        for block in self.leaves.builder.members(&self.content)? {
            if !self.kind(&block)?.equals_ascii("toolCall") {
                continue;
            }
            seen = true;
            let metadata = self.get(&block, "providerMetadata")?;
            if !self.leaves.builder.is_nullish(&metadata)? && self.kind(&metadata)?.equals_ascii("computer") {
                if self.factory().heap().array_length(&self.get(&metadata, "actions")?, self.factory().runtime())? == 0
                {
                    return Ok(false);
                }
                continue;
            }
            if self.truthy(&self.leaves.get_symbol(&block, &self.leaves.arguments_done)?)? {
                continue;
            }
            let partial = self.leaves.get_symbol(&block, &self.leaves.partial)?;
            if !matches!(
                self.factory().heap().snapshot(&self.get(&block, "customWireName")?)?,
                ValueSnapshot::Undefined
            ) || matches!(self.factory().heap().snapshot(&partial)?, ValueSnapshot::Undefined)
            {
                return Ok(false);
            }
            if classify_units(&self.leaves.builder.string(&partial)?) != crate::json::JsonPrefixState::Complete {
                return Ok(false);
            }
        }
        Ok(seen)
    }
    fn finish_response(&mut self, event: &ValueHandle) -> Result<()> {
        let response = self.get(event, "response")?;
        let absent = self.leaves.builder.is_nullish(&response)?;
        let status = if absent { "".into() } else { self.text_or_empty(&response, "status")? };
        let details =
            if absent { self.factory().heap().undefined()? } else { self.get(&response, "incomplete_details")? };
        let reason =
            if self.leaves.builder.is_nullish(&details)? { "".into() } else { self.text_or_empty(&details, "reason")? };
        let promote = status.equals_ascii("incomplete")
            && reason.equals_ascii("max_output_tokens")
            && self.executable_incomplete()?;
        self.leaves.finalize_pending(&self.content)?;
        if !absent {
            let id = self.get(&response, "id")?;
            if self.truthy(&id)? {
                self.leaves.builder.put(&self.root, "responseId", id)?;
            }
            self.apply_usage(&response)?;
        }
        self.stop = if status.equals_ascii("incomplete") {
            StopReason::Length
        } else if status.equals_ascii("failed") || status.equals_ascii("cancelled") {
            StopReason::Error
        } else {
            StopReason::Stop
        };
        self.leaves.builder.put(&self.root, "stopReason", self.leaves.builder.text(self.stop.as_str())?)?;
        if self.stop == StopReason::Error {
            return Err(crate::error::ProviderError::Stream(self.response_error(&response)?).into());
        }
        if status.equals_ascii("incomplete") && reason.equals_ascii("content_filter") {
            return Err(crate::error::ProviderError::Stream("incomplete: content_filter".into()).into());
        }
        let mut tool = false;
        for block in self.leaves.builder.members(&self.content)? {
            if self.kind(&block)?.equals_ascii("toolCall") {
                tool = true;
                break;
            }
        }
        if tool && (self.stop == StopReason::Stop || (promote && self.stop == StopReason::Length)) {
            self.stop = StopReason::ToolUse;
        }
        self.leaves.builder.put(&self.root, "stopReason", self.leaves.builder.text(self.stop.as_str())?)?;
        let paused = !absent
            && matches!(
                self.factory().heap().snapshot(&self.get(&response, "end_turn")?)?,
                ValueSnapshot::Boolean(false)
            );
        if self.stop == StopReason::Stop
            && (paused
                || (self.web_search && !crate::replay_safe_retry::actual::visible(&self.leaves.builder, &self.root)?))
        {
            self.leaves.builder.put(
                &self.root,
                "stopDetails",
                self.leaves.builder.object([("type", self.leaves.builder.text("pause_turn")?)])?,
            )?;
        }
        self.leaves.builder.put(
            &self.root,
            "providerPayload",
            self.leaves.builder.object([
                ("type", self.leaves.builder.text("openaiResponsesHistory")?),
                ("provider", self.leaves.builder.text(self.model.provider.clone())?),
                ("dt", self.factory().heap().boolean(true)?),
                ("items", self.native_items.clone()),
            ])?,
        )?;
        if let Some(hooks) = &self.options.hooks {
            hooks.completed()?;
        }
        self.terminal = true;
        Ok(())
    }
    fn response_error(&self, response: &ValueHandle) -> std::result::Result<String, ValueError> {
        if self.leaves.builder.is_nullish(response)? {
            return Ok("Unknown error (no error details in response)".into());
        }
        let error = self.get(response, "error")?;
        if !self.leaves.builder.is_nullish(&error)? {
            let code = self.text_or_empty(&error, "code")?;
            let message = self.text_or_empty(&error, "message")?;
            let code = if code.is_empty() { "unknown".into() } else { code };
            let message = if message.is_empty() { "no message".into() } else { message };
            return concat(&concat(&code, &": ".into()), &message)
                .to_utf8()
                .map_err(|_| ValueError::UnsupportedOperation("Provider error display contains a lone surrogate"));
        }
        let message = self.text_or_empty(response, "message")?;
        if !message.is_empty() {
            return message
                .to_utf8()
                .map_err(|_| ValueError::UnsupportedOperation("Provider error display contains a lone surrogate"));
        }
        Ok("Unknown error (no error details in response)".into())
    }
}
