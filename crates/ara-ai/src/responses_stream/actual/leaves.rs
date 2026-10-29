//! Original Graph leaf mutations shared by the Responses/Codex routers.
//! Fixed OMP openai-shared.ts 2455-2801, 3471-3500 (596f2da, MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
use crate::AssistantMessageEvent;
use crate::actual_provider::{GraphBuilder, concat};
use crate::event::ActualAssistantEvent;
use crate::graph_values::GraphValueFactory;
use crate::relaxed_json::{FinalParseError, parse_streaming_json_units};
use ara_rpc::{
    WireString,
    value_heap::{PropertyKey, ValueError, ValueHandle, ValueSnapshot},
};
use std::sync::Arc;

type Result<T> = std::result::Result<T, ValueError>;

/// The source module's Symbols are shared by both endpoint routers and every
/// same-lineage consumer. Their fields remain on original blocks, not a typed
/// side table, and clearPartial keeps an own Symbol with value undefined.
pub(crate) struct GraphResponsesLeaves {
    pub(crate) builder: GraphBuilder,
    pub(crate) partial: PropertyKey,
    pub(crate) last_parse: PropertyKey,
    pub(crate) arguments_done: PropertyKey,
}
impl GraphResponsesLeaves {
    pub(crate) fn new(factory: Arc<GraphValueFactory>) -> Result<Self> {
        let heap = factory.heap();
        let partial = heap.module_symbol("ara-ai.block-symbols", "provider.block.partialJson")?;
        let last_parse = heap.module_symbol("ara-ai.block-symbols", "provider.block.lastParseLen")?;
        let arguments_done = heap.module_symbol("ara-ai.block-symbols", "provider.block.argumentsDone")?;
        Ok(Self { builder: GraphBuilder(factory), partial, last_parse, arguments_done })
    }
    pub(crate) fn get_symbol(&self, block: &ValueHandle, key: &PropertyKey) -> Result<ValueHandle> {
        self.builder.0.heap().get(block, key, self.builder.0.runtime())
    }
    pub(crate) fn set_symbol(&self, block: &ValueHandle, key: &PropertyKey, value: ValueHandle) -> Result<()> {
        self.builder.0.heap().assign(block, key.clone(), value, self.builder.0.runtime())
    }
    pub(crate) fn clear_partial(&self, block: &ValueHandle) -> Result<()> {
        if self.builder.0.heap().has_own(block, &self.partial, self.builder.0.runtime())? {
            self.set_symbol(block, &self.partial, self.builder.0.heap().undefined()?)?;
        }
        Ok(())
    }
    pub(crate) fn parse(&self, source: &WireString) -> Result<ValueHandle> {
        parse_streaming_json_units(source, self.builder.0.as_ref()).map_err(|error| match error {
            FinalParseError::Runtime(error) => error,
            FinalParseError::Syntax(error) => ValueError::TypeError(error.to_string()),
        })
    }
    pub(crate) fn text(&self, source: &ValueHandle, key: &str) -> Result<WireString> {
        self.builder.string(&self.builder.get(source, key)?)
    }
    fn array_or_fresh(&self, item: &ValueHandle, key: &str) -> Result<ValueHandle> {
        let value = self.builder.get(item, key)?;
        let value =
            if self.builder.0.heap().truthy(&value)? { value } else { self.builder.0.heap().array_with_length(0)? };
        self.builder.put(item, key, value.clone())?;
        Ok(value)
    }
    fn array_length(&self, array: &ValueHandle) -> Result<u32> {
        self.builder.0.heap().array_length(array, self.builder.0.runtime())
    }
    fn at(&self, array: &ValueHandle, index: u32) -> Result<ValueHandle> {
        self.builder.get(array, &index.to_string())
    }
    fn append_text(&self, target: &ValueHandle, key: &str, delta: &WireString) -> Result<WireString> {
        let value = concat(&self.text(target, key)?, delta);
        self.builder.put(target, key, self.builder.text(value.clone())?)?;
        Ok(value)
    }
    pub(crate) fn thinking_delta(
        &self,
        root: &ValueHandle,
        block: &ValueHandle,
        delta: WireString,
        index: ValueHandle,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<()> {
        self.append_text(block, "thinking", &delta)?;
        events.push(AssistantMessageEvent::ThinkingDelta { content_index: index, delta, partial: root.clone() });
        Ok(())
    }
    pub(crate) fn ensure_summary_part(&self, item: &ValueHandle, index: u32) -> Result<ValueHandle> {
        let summary = self.array_or_fresh(item, "summary")?;
        while self.array_length(&summary)? <= index {
            let part = self
                .builder
                .object([("type", self.builder.text("summary_text")?), ("text", self.builder.text("")?)])?;
            self.builder.push(&summary, part)?;
        }
        self.at(&summary, index)
    }
    pub(crate) fn append_summary_part(&self, item: &ValueHandle, part: ValueHandle) -> Result<()> {
        let summary = self.array_or_fresh(item, "summary")?;
        self.builder.push(&summary, part)
    }
    pub(crate) fn summary_delta(
        &self,
        item: &ValueHandle,
        block: &ValueHandle,
        delta: WireString,
        summary_index: u32,
        root: &ValueHandle,
        index: ValueHandle,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<()> {
        let part = self.ensure_summary_part(item, summary_index)?;
        // Fixed order: block then retained raw summary part then event.
        self.append_text(block, "thinking", &delta)?;
        self.append_text(&part, "text", &delta)?;
        events.push(AssistantMessageEvent::ThinkingDelta { content_index: index, delta, partial: root.clone() });
        Ok(())
    }
    pub(crate) fn summary_text_done(
        &self,
        item: &ValueHandle,
        block: &ValueHandle,
        text: WireString,
        summary_index: u32,
        root: &ValueHandle,
        index: ValueHandle,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<()> {
        let part = self.ensure_summary_part(item, summary_index)?;
        let previous = self.text(&part, "text")?;
        self.builder.put(&part, "text", self.builder.text(text.clone())?)?;
        if text.is_empty() || text == previous {
            return Ok(());
        }
        let thinking = self.text(block, "thinking")?;
        if thinking.is_empty() {
            self.builder.put(block, "thinking", self.builder.text(text.clone())?)?;
            events.push(AssistantMessageEvent::ThinkingDelta {
                content_index: index,
                delta: text,
                partial: root.clone(),
            });
        } else if text.units().starts_with(thinking.units()) {
            let delta = WireString::from_units(text.units()[thinking.len()..].to_vec());
            if !delta.is_empty() {
                self.thinking_delta(root, block, delta, index, events)?;
            }
        }
        Ok(())
    }
    pub(crate) fn summary_part_done(
        &self,
        item: &ValueHandle,
        block: &ValueHandle,
        root: &ValueHandle,
        index: ValueHandle,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<()> {
        let summary = self.array_or_fresh(item, "summary")?;
        let length = self.array_length(&summary)?;
        if length == 0 {
            return Ok(());
        }
        let part = self.at(&summary, length - 1)?;
        if !self.builder.0.heap().truthy(&part)? {
            return Ok(());
        }
        let delta = WireString::from("\n\n");
        self.append_text(block, "thinking", &delta)?;
        self.append_text(&part, "text", &delta)?;
        events.push(AssistantMessageEvent::ThinkingDelta { content_index: index, delta, partial: root.clone() });
        Ok(())
    }
    pub(crate) fn append_message_part(&self, item: &ValueHandle, part: ValueHandle) -> Result<()> {
        let content = self.array_or_fresh(item, "content")?;
        if !self.builder.0.heap().truthy(&part)? {
            return Ok(());
        }
        let kind = self.text(&part, "type")?;
        if kind.equals_ascii("output_text") || kind.equals_ascii("refusal") {
            self.builder.push(&content, part)?;
        }
        Ok(())
    }
    pub(crate) fn message_delta(
        &self,
        item: &ValueHandle,
        block: &ValueHandle,
        delta: WireString,
        part_type: &'static str,
        root: &ValueHandle,
        index: ValueHandle,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<()> {
        let content = self.array_or_fresh(item, "content")?;
        let length = self.array_length(&content)?;
        let last = if length == 0 { self.builder.0.heap().undefined()? } else { self.at(&content, length - 1)? };
        let is_part = !self.builder.is_nullish(&last)? && self.text(&last, "type")?.equals_ascii(part_type);
        let part = if is_part {
            last
        } else {
            let part = if part_type == "output_text" {
                self.builder.object([
                    ("type", self.builder.text("output_text")?),
                    ("text", self.builder.text("")?),
                    ("annotations", self.builder.0.heap().array_with_length(0)?),
                ])?
            } else {
                self.builder.object([("type", self.builder.text("refusal")?), ("refusal", self.builder.text("")?)])?
            };
            self.builder.push(&content, part.clone())?;
            part
        };
        self.append_text(block, "text", &delta)?;
        let field = if self.text(&part, "type")?.equals_ascii("output_text") { "text" } else { "refusal" };
        self.append_text(&part, field, &delta)?;
        events.push(AssistantMessageEvent::TextDelta { content_index: index, delta, partial: root.clone() });
        Ok(())
    }
    pub(crate) fn finalize_message(&self, item: &ValueHandle, streamed: &WireString) -> Result<WireString> {
        let content = self.builder.get(item, "content")?;
        if self.builder.is_nullish(&content)? || self.array_length(&content)? == 0 {
            return Ok(streamed.clone());
        }
        let mut output = WireString::from("");
        for part in self.builder.members(&content)? {
            let field = if self.text(&part, "type")?.equals_ascii("output_text") { "text" } else { "refusal" };
            let text = self.builder.get(&part, field)?;
            if !self.builder.is_nullish(&text)? {
                output = concat(&output, &self.builder.string(&text)?);
            }
        }
        Ok(output)
    }
    pub(crate) fn finalize_reasoning(&self, item: &ValueHandle, streamed: &WireString) -> Result<WireString> {
        let summary = self.builder.get(item, "summary")?;
        let mut output = WireString::from("");
        if !self.builder.is_nullish(&summary)? {
            for (index, part) in self.builder.members(&summary)?.iter().enumerate() {
                if index > 0 {
                    output.append_str("\n\n");
                }
                let text = self.builder.get(part, "text")?;
                if !self.builder.is_nullish(&text)? {
                    output = concat(&output, &self.builder.string(&text)?);
                }
            }
        }
        if !output.is_empty() {
            return Ok(output);
        }
        let content = self.builder.get(item, "content")?;
        if !self.builder.is_nullish(&content)? && self.array_length(&content)? > 0 {
            let part = self.at(&content, 0)?;
            if !self.builder.is_nullish(&part)? && self.text(&part, "type")?.equals_ascii("reasoning_text") {
                let text = self.builder.get(&part, "text")?;
                if !self.builder.is_nullish(&text)? {
                    let text = self.builder.string(&text)?;
                    if !text.is_empty() {
                        return Ok(text);
                    }
                }
            }
        }
        Ok(streamed.clone())
    }
    pub(crate) fn function_delta(
        &self,
        block: &ValueHandle,
        delta: ValueHandle,
        root: &ValueHandle,
        index: ValueHandle,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<()> {
        let partial = self.builder.string(&self.get_symbol(block, &self.partial)?)?;
        let raw_delta = self.builder.string(&delta)?;
        let partial = concat(&partial, &raw_delta);
        self.set_symbol(block, &self.partial, self.builder.text(partial.clone())?)?;
        let last = self.get_symbol(block, &self.last_parse)?;
        let last = if self.builder.is_nullish(&last)? {
            0.0
        } else {
            self.builder.0.heap().to_number(&last, self.builder.0.runtime())?
        };
        let len = partial.len();
        // Original throttle: max(256, len >> 5), not Chat's geometric gate.
        let growth = (256_i32).max((len as i32) >> 5) as f64;
        if len != 0 && !(last > 0.0 && len as f64 - last < growth) {
            self.builder.put(block, "arguments", self.parse(&partial)?)?;
            self.set_symbol(block, &self.last_parse, self.builder.0.heap().number(len as f64)?)?;
        }
        events.push(AssistantMessageEvent::ToolcallDelta { content_index: index, delta, partial: root.clone() });
        Ok(())
    }
    pub(crate) fn function_done(&self, block: &ValueHandle, arguments: ValueHandle) -> Result<()> {
        self.set_symbol(block, &self.partial, arguments.clone())?;
        self.builder.put(block, "arguments", self.parse(&self.builder.string(&arguments)?)?)?;
        self.clear_partial(block)
    }
    pub(crate) fn custom_delta(
        &self,
        block: &ValueHandle,
        delta: ValueHandle,
        root: &ValueHandle,
        index: ValueHandle,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<()> {
        let partial =
            concat(&self.builder.string(&self.get_symbol(block, &self.partial)?)?, &self.builder.string(&delta)?);
        self.set_symbol(block, &self.partial, self.builder.text(partial.clone())?)?;
        self.builder.put(block, "arguments", self.builder.object([("input", self.builder.text(partial)?)])?)?;
        events.push(AssistantMessageEvent::ToolcallDelta { content_index: index, delta, partial: root.clone() });
        Ok(())
    }
    pub(crate) fn custom_done(&self, block: &ValueHandle, input: ValueHandle) -> Result<()> {
        // Source deliberately does not rewrite its retained partial buffer.
        self.builder.put(block, "arguments", self.builder.object([("input", input)])?)
    }
    pub(crate) fn finalize_pending(&self, content: &ValueHandle) -> Result<()> {
        for block in self.builder.members(content)? {
            if !self.text(&block, "type")?.equals_ascii("toolCall") {
                continue;
            }
            let partial = self.get_symbol(&block, &self.partial)?;
            let done = self.get_symbol(&block, &self.arguments_done)?;
            if self.builder.0.heap().truthy(&partial)? && !self.builder.0.heap().truthy(&done)? {
                let custom = self.builder.get(&block, "customWireName")?;
                let arguments = if !matches!(self.builder.0.heap().snapshot(&custom)?, ValueSnapshot::Undefined) {
                    self.builder.object([("input", partial)])?
                } else {
                    self.parse(&self.builder.string(&partial)?)?
                };
                self.builder.put(&block, "arguments", arguments)?;
            }
            self.clear_partial(&block)?;
        }
        Ok(())
    }
    pub(crate) fn computer_metadata(&self, item: &ValueHandle) -> Result<ValueHandle> {
        let actions = self.builder.get(item, "actions")?;
        let actions = if !self.builder.is_nullish(&actions)? && self.array_length(&actions)? > 0 {
            actions
        } else {
            let action = self.builder.get(item, "action")?;
            let actions = self.builder.0.heap().array_with_length(0)?;
            if self.builder.0.heap().truthy(&action)? {
                self.builder.push(&actions, action)?;
            }
            actions
        };
        let checks = self.builder.get(item, "pending_safety_checks")?;
        let checks =
            if self.builder.is_nullish(&checks)? { self.builder.0.heap().array_with_length(0)? } else { checks };
        self.builder.object([
            ("type", self.builder.text("computer")?),
            ("providerItemId", self.builder.get(item, "id")?),
            ("actions", self.builder.0.heap().structured_clone_json(&actions, self.builder.0.runtime())?),
            ("pendingSafetyChecks", self.builder.0.heap().structured_clone_json(&checks, self.builder.0.runtime())?),
        ])
    }
}
