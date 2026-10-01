//! Responses text and JSON function-call event state.
//!
//! Source: pinned OMP `openai-shared.ts::processResponsesStream` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d. Native reasoning replay
//! and provider-hosted tools remain separate parity gaps.

use crate::error::{ProviderError, envelope_message};
use crate::event::AssistantMessageEvent;
use crate::json::{JsonPrefixState, classify_json_prefix, parse_final_arguments};
use crate::types::{AssistantBlock, AssistantMessage, Model, StopReason, TextContent, ThinkingContent, ToolCall};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};

pub(crate) fn responses_endpoint_fingerprint(base_url: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, base_url.trim_end_matches('/').as_bytes());
    digest.as_ref().iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ItemKind {
    Text,
    Function,
    Reasoning,
}

struct OpenItem {
    kind: ItemKind,
    content_index: usize,
    output_index: Option<u64>,
    item_id: Option<String>,
    call_id: Option<String>,
    argument_bytes: String,
    final_arguments: Option<String>,
    identifierless_arguments_done: bool,
    summary_parts: Vec<String>,
    summary_parts_done: HashSet<usize>,
    raw_reasoning: String,
}

struct CompletedIdentifierless {
    output_index: Option<u64>,
    item_id: Option<String>,
    call_id: String,
    name: String,
    arguments: Option<Value>,
}

pub(crate) struct ResponsesStreamState {
    pub(crate) output: AssistantMessage,
    endpoint_fingerprint: String,
    open: HashMap<usize, OpenItem>,
    by_index: HashMap<u64, usize>,
    by_id: HashMap<String, usize>,
    by_call: HashMap<String, usize>,
    by_prefixed_call: HashMap<String, usize>,
    seen_call_ids: HashSet<String>,
    completed_identifierless: Vec<CompletedIdentifierless>,
    done_indices: HashSet<u64>,
    done_ids: HashSet<String>,
    completed_tool_args: Vec<bool>,
    native_items: BTreeMap<u64, Value>,
    native_bytes: usize,
    native_over_limit: bool,
    next_key: usize,
    identifierless_delta_target: Option<usize>,
    identifierless_scan_work: usize,
    pub(crate) terminal: bool,
    pub(crate) replay_unsafe_wire_event: bool,
    // Thinking commits the Provider stream but can be removed by Session
    // recovery. Tool and unknown native effects must still veto that recovery.
    pub(crate) same_route_unsafe_wire_event: bool,
}

impl ResponsesStreamState {
    pub(crate) fn new(model: &Model) -> Self {
        Self {
            output: AssistantMessage::empty(&model.api, &model.provider, &model.id),
            endpoint_fingerprint: responses_endpoint_fingerprint(&model.base_url),
            open: HashMap::new(),
            by_index: HashMap::new(),
            by_id: HashMap::new(),
            by_call: HashMap::new(),
            by_prefixed_call: HashMap::new(),
            seen_call_ids: HashSet::new(),
            completed_identifierless: Vec::new(),
            done_indices: HashSet::new(),
            done_ids: HashSet::new(),
            completed_tool_args: Vec::new(),
            native_items: BTreeMap::new(),
            native_bytes: 0,
            native_over_limit: false,
            next_key: 0,
            identifierless_delta_target: None,
            identifierless_scan_work: 0,
            terminal: false,
            replay_unsafe_wire_event: false,
            same_route_unsafe_wire_event: false,
        }
    }

    fn lookup(&self, event: &Value, aliases: bool) -> Result<Option<usize>, ProviderError> {
        let index = event.get("output_index").and_then(Value::as_u64);
        let id = event.get("item_id").and_then(Value::as_str);
        let indexed = index.and_then(|index| self.by_index.get(&index)).copied();
        let exact_id = id.and_then(|id| self.by_id.get(id)).copied();
        let call_alias = id.and_then(|id| {
            aliases.then(|| self.by_prefixed_call.get(id).or_else(|| self.by_call.get(id)).copied()).flatten()
        });
        if let Some(key) = indexed {
            if (exact_id.is_some() || call_alias.is_some()) && exact_id != Some(key) && call_alias != Some(key) {
                return Err(ProviderError::Stream(
                    "Responses event identifiers refer to different output items".into(),
                ));
            }
            return Ok(Some(key));
        }
        if let Some(key) = call_alias.or(exact_id) {
            return Ok(Some(key));
        }
        // A stale keyed event may not drift into a concurrent sibling.
        if index.is_some() || id.is_some() {
            return Ok(None);
        }
        Ok((self.open.len() == 1).then(|| *self.open.keys().next().expect("one open item")))
    }

    fn identifierless_function(&mut self, delta: Option<&str>) -> Result<Option<usize>, ProviderError> {
        let mut keys = self
            .open
            .iter()
            .filter_map(|(&key, item)| {
                (item.kind == ItemKind::Function && item.final_arguments.is_none()).then_some(key)
            })
            .collect::<Vec<_>>();
        keys.sort_unstable(); // Keys increase with output_item.added order.
        if delta.is_none() {
            return Ok(keys.first().copied());
        }
        let delta = delta.expect("checked above");
        if delta.trim_start_matches([' ', '\t', '\n', '\r']).starts_with('{') {
            // Classification scans the accumulated buffer; bound the total
            // work as well as the per-call argument bytes.
            let charge = keys.iter().fold(0usize, |sum, key| {
                sum.saturating_add(self.open[key].argument_bytes.len().saturating_mul(2).saturating_add(delta.len()))
            });
            self.identifierless_scan_work = self.identifierless_scan_work.saturating_add(charge);
            if self.identifierless_scan_work > 128 * 1024 * 1024 {
                return Err(ProviderError::Stream(
                    "Responses identifierless argument routing exceeded work limit".into(),
                ));
            }
        }
        if let Some(target) = self.identifierless_delta_target
            && let Some(position) = keys.iter().position(|key| *key == target)
            && (!self.should_advance_identifierless_delta(target, delta) || position + 1 == keys.len())
        {
            return Ok(Some(target));
        }
        let key = keys.iter().enumerate().find_map(|(index, key)| {
            (!self.should_advance_identifierless_delta(*key, delta) || index + 1 == keys.len()).then_some(*key)
        });
        let Some(key) = key else { return Ok(None) };
        self.identifierless_delta_target = Some(key);
        Ok(Some(key))
    }

    fn should_advance_identifierless_delta(&self, key: usize, delta: &str) -> bool {
        if !delta.trim_start_matches([' ', '\t', '\n', '\r']).starts_with('{') {
            return false;
        }
        let partial = &self.open[&key].argument_bytes;
        if partial.trim().is_empty() {
            return false;
        }
        if classify_json_prefix(partial) != JsonPrefixState::Prefix {
            return true;
        }
        let mut combined = String::with_capacity(partial.len() + delta.len());
        combined.push_str(partial);
        combined.push_str(delta);
        classify_json_prefix(&combined) == JsonPrefixState::Invalid
    }

    fn add(
        &mut self,
        kind: ItemKind,
        event: &Value,
        item: &Value,
        events: &mut Vec<AssistantMessageEvent>,
    ) -> Result<usize, ProviderError> {
        let index = event.get("output_index").and_then(Value::as_u64);
        let id = item.get("id").and_then(Value::as_str).map(str::to_owned);
        let call = item.get("call_id").and_then(Value::as_str).map(str::to_owned);
        if kind == ItemKind::Function && call.as_deref().is_none_or(str::is_empty) {
            return Err(ProviderError::Stream("Responses function call has no call_id".into()));
        }
        if call.as_ref().is_some_and(|call| self.seen_call_ids.contains(call)) {
            return Err(ProviderError::Stream("Responses function call_id was added twice".into()));
        }
        if index.is_some_and(|index| self.by_index.contains_key(&index))
            || id.as_ref().is_some_and(|id| self.by_id.contains_key(id))
        {
            return Err(ProviderError::Stream("Responses output item was added twice".into()));
        }
        let content_index = self.output.content.len();
        let key = self.next_key;
        self.next_key += 1;
        match kind {
            ItemKind::Text => {
                self.output
                    .content
                    .push(AssistantBlock::Text(TextContent { text: String::new(), text_signature: None }));
                events.push(AssistantMessageEvent::TextStart { content_index, partial: self.output.clone() });
            }
            ItemKind::Function => {
                self.replay_unsafe_wire_event = true;
                self.same_route_unsafe_wire_event = true;
                let call_id = call.as_deref().expect("checked above");
                let stable_id = id.clone().unwrap_or_else(|| format!("fc_ara_{key}"));
                self.output.content.push(AssistantBlock::ToolCall(ToolCall {
                    id: format!("{call_id}|{stable_id}"),
                    name: item.get("name").and_then(Value::as_str).unwrap_or("").to_owned(),
                    arguments: Default::default(),
                    thought_signature: None,
                }));
                events.push(AssistantMessageEvent::ToolcallStart { content_index, partial: self.output.clone() });
            }
            ItemKind::Reasoning => {
                self.output.content.push(AssistantBlock::Thinking(ThinkingContent {
                    thinking: String::new(),
                    thinking_signature: None,
                }));
                events.push(AssistantMessageEvent::ThinkingStart { content_index, partial: self.output.clone() });
            }
        }
        if let Some(index) = index {
            self.by_index.insert(index, key);
        }
        if let Some(id) = &id {
            self.by_id.insert(id.clone(), key);
        }
        if let Some(call) = &call {
            self.seen_call_ids.insert(call.clone());
            self.by_call.insert(call.clone(), key);
            self.by_prefixed_call.insert(format!("fc_{call}"), key);
        }
        self.open.insert(
            key,
            OpenItem {
                kind,
                content_index,
                output_index: index,
                item_id: id,
                call_id: call,
                argument_bytes: item.get("arguments").and_then(Value::as_str).unwrap_or("").to_owned(),
                final_arguments: None,
                identifierless_arguments_done: false,
                summary_parts: Vec::new(),
                summary_parts_done: HashSet::new(),
                raw_reasoning: String::new(),
            },
        );
        Ok(key)
    }

    fn close(&mut self, key: usize) {
        if self.identifierless_delta_target == Some(key) {
            self.identifierless_delta_target = None;
        }
        let Some(item) = self.open.remove(&key) else { return };
        if let Some(index) = item.output_index
            && self.by_index.get(&index) == Some(&key)
        {
            self.by_index.remove(&index);
        }
        if let Some(id) = item.item_id
            && self.by_id.get(&id) == Some(&key)
        {
            self.by_id.remove(&id);
        }
        if let Some(call) = item.call_id {
            if self.by_call.get(&call) == Some(&key) {
                self.by_call.remove(&call);
            }
            let alias = format!("fc_{call}");
            if self.by_prefixed_call.get(&alias) == Some(&key) {
                self.by_prefixed_call.remove(&alias);
            }
        }
    }

    fn finalize_partial_function_arguments(&mut self) {
        for item in self.open.values().filter(|item| item.kind == ItemKind::Function) {
            if let AssistantBlock::ToolCall(call) = &mut self.output.content[item.content_index] {
                // The stream has ended. A prefix that can be completed for a
                // live preview is still invalid as a final tool invocation.
                call.arguments = parse_final_arguments(&item.argument_bytes);
            }
        }
    }

    fn save_native_history(&mut self) -> Result<(), ProviderError> {
        if self.native_over_limit {
            return Ok(());
        }
        let items = self.native_items.values().cloned().collect::<Vec<_>>();
        let size = serde_json::to_vec(&items).map_err(|error| ProviderError::Stream(error.to_string()))?.len();
        if size <= 32 * 1024 * 1024 {
            self.output.provider_payload = Some(json!({
                "type": "openaiResponsesHistory", "provider": self.output.provider,
                "dt": true,
                "endpointSha256": self.endpoint_fingerprint,
                "items": items,
            }));
        }
        Ok(())
    }

    fn done_item(
        &mut self,
        event: &Value,
        item: &Value,
        events: &mut Vec<AssistantMessageEvent>,
        terminal_fallback: bool,
    ) -> Result<(), ProviderError> {
        let index = event.get("output_index").and_then(Value::as_u64);
        let id = item.get("id").and_then(Value::as_str);
        let call_id = item.get("call_id").and_then(Value::as_str);
        if let Some(done) = self.completed_identifierless.iter().find(|done| {
            index.is_some_and(|index| done.output_index == Some(index))
                || id.is_some_and(|id| done.item_id.as_deref() == Some(id))
                || call_id == Some(done.call_id.as_str())
        }) {
            let conflicts = index.is_some_and(|index| done.output_index.is_some_and(|known| known != index))
                || id.is_some_and(|id| done.item_id.as_deref().is_some_and(|known| known != id))
                || call_id.is_some_and(|call_id| call_id != done.call_id)
                || item.get("type").and_then(Value::as_str).is_some_and(|kind| kind != "function_call")
                || item.get("name").and_then(Value::as_str).is_some_and(|name| name != done.name)
                || item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .filter(|raw| !raw.is_empty())
                    .is_some_and(|raw| strict_arguments_value(raw) != done.arguments);
            if conflicts {
                return Err(ProviderError::Stream("Responses final item contradicts identifierless routing".into()));
            }
        }
        if index.is_some_and(|index| self.done_indices.contains(&index))
            || id.is_some_and(|id| self.done_ids.contains(id))
        {
            return Ok(());
        }
        let kind = match item.get("type").and_then(Value::as_str) {
            Some("message") => ItemKind::Text,
            Some("function_call") => ItemKind::Function,
            Some("reasoning") => ItemKind::Reasoning,
            _ => return Err(ProviderError::Stream("Unsupported Responses output item".into())),
        };
        if !self.native_over_limit {
            self.native_bytes = self.native_bytes.saturating_add(item.to_string().len());
            if self.native_bytes > 32 * 1024 * 1024 {
                self.native_over_limit = true;
                self.native_items.clear();
                for block in &mut self.output.content {
                    if let AssistantBlock::Thinking(thinking) = block {
                        thinking.thinking_signature = None;
                    }
                }
            }
        }
        // Done items use exact IDs/call IDs. The `fc_<call_id>` alias is only
        // for argument deltas: it can equal a sibling's real call_id.
        let call_id = item.get("call_id").and_then(Value::as_str);
        let exact = index
            .and_then(|index| self.by_index.get(&index))
            .copied()
            .or_else(|| id.and_then(|id| self.by_id.get(id)).copied())
            .or_else(|| call_id.and_then(|id| self.by_call.get(id)).copied());
        let key = match exact {
            Some(key) => key,
            None => self.add(kind, event, item, events)?,
        };
        let open = self.open.get(&key).ok_or_else(|| ProviderError::Stream("Responses item vanished".into()))?;
        if open.kind != kind {
            return Err(ProviderError::Stream("Responses output item type changed".into()));
        }
        if open.item_id.is_some() && id.is_some() && open.item_id.as_deref() != id {
            return Err(ProviderError::Stream("Responses output item id changed before completion".into()));
        }
        if kind == ItemKind::Function {
            if item.get("call_id").and_then(Value::as_str).is_some_and(|id| open.call_id.as_deref() != Some(id)) {
                return Err(ProviderError::Stream("Responses function call_id changed before completion".into()));
            }
            let original_name = match &self.output.content[open.content_index] {
                AssistantBlock::ToolCall(call) => call.name.as_str(),
                _ => return Err(ProviderError::Stream("Responses function block changed".into())),
            };
            if item.get("name").and_then(Value::as_str).is_some_and(|name| name != original_name) {
                return Err(ProviderError::Stream("Responses function name changed before completion".into()));
            }
            if open.identifierless_arguments_done
                && let Some(final_raw) = item.get("arguments").and_then(Value::as_str).filter(|raw| !raw.is_empty())
                && let Some(inferred_raw) = open.final_arguments.as_deref()
                && serde_json::from_str::<Value>(inferred_raw).ok() != serde_json::from_str::<Value>(final_raw).ok()
            {
                return Err(ProviderError::Stream(
                    "Responses final arguments contradict identifierless routing".into(),
                ));
            }
        }
        let content_index = open.content_index;
        match kind {
            ItemKind::Text => {
                let final_text = item.get("content").and_then(Value::as_array).and_then(|parts| {
                    (!parts.is_empty()).then(|| {
                        parts
                            .iter()
                            .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                                Some("refusal") => part.get("refusal").and_then(Value::as_str),
                                _ => part.get("text").and_then(Value::as_str),
                            })
                            .collect::<String>()
                    })
                });
                let text = final_text.unwrap_or_else(|| match &self.output.content[content_index] {
                    AssistantBlock::Text(text) => text.text.clone(),
                    _ => String::new(),
                });
                if let AssistantBlock::Text(block) = &mut self.output.content[content_index] {
                    block.text = text.clone();
                }
                events.push(AssistantMessageEvent::TextEnd {
                    content_index,
                    content: text,
                    partial: self.output.clone(),
                });
            }
            ItemKind::Function => {
                self.replay_unsafe_wire_event = true;
                self.same_route_unsafe_wire_event = true;
                let proven_complete = open.final_arguments.is_some()
                    || (terminal_fallback
                        && serde_json::from_str::<crate::types::JsonObject>(&open.argument_bytes).is_ok());
                let raw = open
                    .final_arguments
                    .as_deref()
                    .or_else(|| item.get("arguments").and_then(Value::as_str).filter(|raw| !raw.is_empty()))
                    .or_else(|| (!open.argument_bytes.is_empty()).then_some(open.argument_bytes.as_str()))
                    .ok_or_else(|| ProviderError::Stream("Responses function finished without arguments".into()))?;
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .or(open.call_id.as_deref())
                    .ok_or_else(|| ProviderError::Stream("Responses completed function has no call_id".into()))?;
                let stable_id =
                    id.or(open.item_id.as_deref()).map(str::to_owned).unwrap_or_else(|| format!("fc_ara_{key}"));
                let original_name = match &self.output.content[content_index] {
                    AssistantBlock::ToolCall(call) => call.name.as_str(),
                    _ => return Err(ProviderError::Stream("Responses function block changed".into())),
                };
                let tool_call = ToolCall {
                    id: format!("{call_id}|{stable_id}"),
                    name: item.get("name").and_then(Value::as_str).unwrap_or(original_name).to_owned(),
                    arguments: parse_final_arguments(raw),
                    thought_signature: None,
                };
                if open.identifierless_arguments_done {
                    self.completed_identifierless.push(CompletedIdentifierless {
                        output_index: open.output_index,
                        item_id: open.item_id.clone(),
                        call_id: call_id.to_owned(),
                        name: tool_call.name.clone(),
                        arguments: strict_arguments_value(raw),
                    });
                }
                self.output.content[content_index] = AssistantBlock::ToolCall(tool_call.clone());
                self.completed_tool_args.push(proven_complete && !tool_call.arguments.contains_key("__parseError"));
                events.push(AssistantMessageEvent::ToolcallEnd {
                    content_index,
                    tool_call,
                    partial: self.output.clone(),
                });
            }
            ItemKind::Reasoning => {
                let summary = item
                    .get("summary")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|part| part.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("\n\n")
                    })
                    .unwrap_or_default();
                let raw = item
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts.iter().filter_map(|part| part.get("text").and_then(Value::as_str)).collect::<String>()
                    })
                    .unwrap_or_default();
                let streamed = match &self.output.content[content_index] {
                    AssistantBlock::Thinking(block) => block.thinking.clone(),
                    _ => return Err(ProviderError::Stream("Responses reasoning block changed".into())),
                };
                let thinking = if !summary.is_empty() {
                    summary
                } else if !raw.is_empty() {
                    raw
                } else if !streamed.is_empty() {
                    streamed
                } else {
                    open.raw_reasoning.clone()
                };
                if let AssistantBlock::Thinking(block) = &mut self.output.content[content_index] {
                    block.thinking = thinking.clone();
                    block.thinking_signature = (!self.native_over_limit).then(|| item.to_string());
                }
                events.push(AssistantMessageEvent::ThinkingEnd {
                    content_index,
                    content: thinking,
                    partial: self.output.clone(),
                });
            }
        }
        let native_index = index.or(open.output_index).unwrap_or(content_index as u64);
        let mut native = item.clone();
        if kind == ItemKind::Function
            && let AssistantBlock::ToolCall(call) = &self.output.content[content_index]
        {
            native["arguments"] = if call.arguments.contains_key("__parseError") {
                json!(
                    open.final_arguments
                        .as_deref()
                        .or_else(|| item.get("arguments").and_then(Value::as_str))
                        .unwrap_or(&open.argument_bytes)
                )
            } else {
                json!(serde_json::to_string(&call.arguments).map_err(|error| ProviderError::Stream(error.to_string()))?)
            };
        }
        if !self.native_over_limit {
            self.native_items.insert(native_index, native);
        }
        if let Some(index) = index {
            self.done_indices.insert(index);
        }
        if let Some(id) = id {
            self.done_ids.insert(id.to_owned());
        }
        self.close(key);
        Ok(())
    }

    pub(crate) fn handle(&mut self, event: &Value) -> Result<Vec<AssistantMessageEvent>, ProviderError> {
        if self.terminal {
            return Err(ProviderError::Stream("Responses event arrived after terminal".into()));
        }
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Stream("Responses frame has no type".into()))?;
        let mut events = Vec::new();
        match kind {
            "response.created" => {
                self.output.response_id = event.pointer("/response/id").and_then(Value::as_str).map(str::to_owned)
            }
            "response.output_item.added" => {
                let item = event
                    .get("item")
                    .ok_or_else(|| ProviderError::Stream("Responses added event has no item".into()))?;
                match item.get("type").and_then(Value::as_str) {
                    Some("message") => {
                        self.add(ItemKind::Text, event, item, &mut events)?;
                    }
                    Some("function_call") => {
                        self.add(ItemKind::Function, event, item, &mut events)?;
                    }
                    Some("reasoning") => {
                        self.add(ItemKind::Reasoning, event, item, &mut events)?;
                    }
                    _ => {
                        self.replay_unsafe_wire_event = true;
                        self.same_route_unsafe_wire_event = true;
                        return Err(ProviderError::Stream("Unsupported Responses output item".into()));
                    }
                }
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                if let Some(key) = self.lookup(event, false)? {
                    let open = self.open.get(&key).expect("indexed item exists");
                    if open.kind != ItemKind::Text {
                        return Err(ProviderError::Stream("Responses text delta routed to non-text item".into()));
                    }
                    let index = open.content_index;
                    let delta = event
                        .get("delta")
                        .and_then(Value::as_str)
                        .ok_or_else(|| ProviderError::Stream("Responses text delta is missing".into()))?;
                    if let AssistantBlock::Text(block) = &mut self.output.content[index] {
                        block.text.push_str(delta);
                    }
                    events.push(AssistantMessageEvent::TextDelta {
                        content_index: index,
                        delta: delta.to_owned(),
                        partial: self.output.clone(),
                    });
                }
            }
            "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.delta" => {
                if let Some(key) = self.lookup(event, false)? {
                    let open = self.open.get_mut(&key).expect("indexed item exists");
                    if open.kind != ItemKind::Reasoning {
                        return Err(ProviderError::Stream(
                            "Responses reasoning event routed to non-reasoning item".into(),
                        ));
                    }
                    let index = open.content_index;
                    let part = event.get("summary_index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    if part > 1024 {
                        return Err(ProviderError::Stream("Responses reasoning has too many summary parts".into()));
                    }
                    if open.summary_parts.len() <= part {
                        open.summary_parts.resize(part + 1, String::new());
                    }
                    let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
                    let final_text = event.get("text").and_then(Value::as_str);
                    if kind == "response.reasoning_summary_part.done"
                        || !delta.is_empty()
                        || (kind == "response.reasoning_summary_text.done"
                            && final_text.is_some_and(|text| !text.is_empty()))
                    {
                        self.replay_unsafe_wire_event = true;
                    }
                    match kind {
                        "response.reasoning_summary_text.delta" => open.summary_parts[part].push_str(delta),
                        "response.reasoning_summary_text.done" => {
                            if let Some(text) = final_text {
                                open.summary_parts[part] = text.to_owned();
                            }
                        }
                        "response.reasoning_summary_part.done" => {
                            if open.summary_parts_done.insert(part)
                                && !open.summary_parts[part].is_empty()
                                && !open.summary_parts.iter().skip(part + 1).any(|part| !part.is_empty())
                            {
                                if let AssistantBlock::Thinking(block) = &mut self.output.content[index] {
                                    block.thinking.push_str("\n\n");
                                }
                                events.push(AssistantMessageEvent::ThinkingDelta {
                                    content_index: index,
                                    delta: "\n\n".into(),
                                    partial: self.output.clone(),
                                });
                                return Ok(events);
                            }
                        }
                        "response.reasoning_text.delta" => open.raw_reasoning.push_str(delta),
                        _ => {}
                    }
                    let thinking = if open.summary_parts.iter().any(|part| !part.is_empty()) {
                        open.summary_parts.join("\n\n")
                    } else {
                        open.raw_reasoning.clone()
                    };
                    let previous = match &self.output.content[index] {
                        AssistantBlock::Thinking(block) => block.thinking.clone(),
                        _ => return Err(ProviderError::Stream("Responses reasoning block changed".into())),
                    };
                    if previous != thinking
                        && let Some(suffix) = thinking.strip_prefix(&previous)
                    {
                        if let AssistantBlock::Thinking(block) = &mut self.output.content[index] {
                            block.thinking = thinking.clone();
                        }
                        events.push(AssistantMessageEvent::ThinkingDelta {
                            content_index: index,
                            delta: suffix.to_owned(),
                            partial: self.output.clone(),
                        });
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                let delta = event
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ProviderError::Stream("Responses arguments delta is missing".into()))?;
                let key = if event.get("output_index").is_none() && event.get("item_id").is_none() {
                    self.identifierless_function(Some(delta))?
                } else {
                    self.lookup(event, true)?
                };
                if let Some(key) = key {
                    let open = self.open.get_mut(&key).expect("indexed item exists");
                    if open.kind != ItemKind::Function {
                        return Err(ProviderError::Stream("Responses arguments routed to non-function item".into()));
                    }
                    if open.argument_bytes.len().saturating_add(delta.len()) > 8 * 1024 * 1024 {
                        return Err(ProviderError::Stream("Responses function arguments exceed size limit".into()));
                    }
                    open.argument_bytes.push_str(delta);
                    self.replay_unsafe_wire_event = true;
                    self.same_route_unsafe_wire_event = true;
                    events.push(AssistantMessageEvent::ToolcallDelta {
                        content_index: open.content_index,
                        delta: delta.to_owned(),
                        partial: self.output.clone(),
                    });
                }
            }
            "response.function_call_arguments.done" => {
                let identifierless = event.get("output_index").is_none() && event.get("item_id").is_none();
                let key = if identifierless { self.identifierless_function(None)? } else { self.lookup(event, true)? };
                if let Some(key) = key {
                    let open = self.open.get_mut(&key).expect("indexed item exists");
                    if open.kind != ItemKind::Function {
                        return Err(ProviderError::Stream(
                            "Responses final arguments routed to non-function item".into(),
                        ));
                    }
                    let raw = event
                        .get("arguments")
                        .and_then(Value::as_str)
                        .ok_or_else(|| ProviderError::Stream("Responses final arguments are missing".into()))?;
                    if raw.len() > 8 * 1024 * 1024 {
                        return Err(ProviderError::Stream("Responses function arguments exceed size limit".into()));
                    }
                    open.final_arguments = Some(raw.to_owned());
                    open.identifierless_arguments_done = identifierless;
                    self.replay_unsafe_wire_event = true;
                    self.same_route_unsafe_wire_event = true;
                }
            }
            "response.output_item.done" => {
                self.replay_unsafe_wire_event = true;
                if event.pointer("/item/type").and_then(Value::as_str) != Some("reasoning") {
                    self.same_route_unsafe_wire_event = true;
                }
                let item = event
                    .get("item")
                    .ok_or_else(|| ProviderError::Stream("Responses done event has no item".into()))?;
                self.done_item(event, item, &mut events, false)?;
            }
            "response.completed" | "response.incomplete" | "response.done" => {
                let response = event
                    .get("response")
                    .ok_or_else(|| ProviderError::Stream("Responses terminal has no response".into()))?;
                let default_status = if kind == "response.incomplete" { "incomplete" } else { "completed" };
                let status = response.get("status").and_then(Value::as_str).unwrap_or(default_status);
                if (kind == "response.completed" && status == "incomplete")
                    || (kind == "response.incomplete" && status == "completed")
                {
                    return Err(ProviderError::Stream("Responses terminal event contradicts response status".into()));
                }
                if let Some(output) = response.get("output").and_then(Value::as_array) {
                    for (index, item) in output.iter().enumerate() {
                        self.done_item(&json!({"output_index": index}), item, &mut events, status == "completed")?;
                    }
                }
                // A compatible upstream may omit output_item.done. Finalize
                // only calls with explicit done args or strictly complete
                // accumulated JSON; partial calls remain non-executable.
                let pending = self
                    .open
                    .iter()
                    .filter_map(|(&key, item)| (item.kind == ItemKind::Function).then_some(key))
                    .collect::<Vec<_>>();
                for key in pending {
                    let open = self.open.get(&key).expect("pending key exists");
                    let full_args = open.final_arguments.as_deref().or_else(|| {
                        serde_json::from_str::<crate::types::JsonObject>(&open.argument_bytes)
                            .ok()
                            .map(|_| open.argument_bytes.as_str())
                    });
                    let Some(full_args) = full_args else { continue };
                    let original = match &self.output.content[open.content_index] {
                        AssistantBlock::ToolCall(call) => call,
                        _ => return Err(ProviderError::Stream("Responses pending function block changed".into())),
                    };
                    let final_item = json!({
                        "type": "function_call", "id": open.item_id,
                        "call_id": open.call_id, "name": original.name,
                        "arguments": full_args,
                    });
                    let event = json!({"output_index": open.output_index});
                    self.done_item(&event, &final_item, &mut events, true)?;
                }
                if let Some(id) = response.get("id").and_then(Value::as_str) {
                    self.output.response_id = Some(id.to_owned());
                }
                self.read_usage(response.get("usage"));
                match status {
                    "completed" => {
                        if self.open.values().any(|item| item.kind == ItemKind::Function) {
                            return Err(ProviderError::Incomplete(
                                "Responses terminal left a function call unfinished".into(),
                            ));
                        }
                        self.output.stop_reason = if self.output.tool_calls().next().is_some() {
                            StopReason::ToolUse
                        } else {
                            StopReason::Stop
                        };
                    }
                    "incomplete"
                        if response.pointer("/incomplete_details/reason").and_then(Value::as_str)
                            == Some("max_output_tokens") =>
                    {
                        let has_unfinished = self.open.values().any(|item| item.kind == ItemKind::Function);
                        self.output.stop_reason = if !has_unfinished
                            && !self.completed_tool_args.is_empty()
                            && self.completed_tool_args.iter().all(|complete| *complete)
                        {
                            StopReason::ToolUse
                        } else {
                            self.finalize_partial_function_arguments();
                            StopReason::Length
                        }
                    }
                    "incomplete"
                        if response.pointer("/incomplete_details/reason").and_then(Value::as_str)
                            == Some("content_filter") =>
                    {
                        return Err(ProviderError::Stream("Responses output was blocked by content_filter".into()));
                    }
                    "incomplete" => {
                        self.finalize_partial_function_arguments();
                        self.output.stop_reason = StopReason::Length;
                    }
                    "failed" | "cancelled" => return Err(ProviderError::Stream(response_error(response))),
                    _ => {
                        return Err(ProviderError::Stream(format!(
                            "Responses terminal has unsupported status {status}"
                        )));
                    }
                }
                self.save_native_history()?;
                self.terminal = true;
            }
            "response.failed" => {
                self.read_usage(event.pointer("/response/usage"));
                return Err(ProviderError::Stream(response_error(event.get("response").unwrap_or(event))));
            }
            "error" => return Err(ProviderError::Stream(response_error(event))),
            _ => {}
        }
        Ok(events)
    }

    fn read_usage(&mut self, usage: Option<&Value>) {
        let Some(usage) = usage else { return };
        let cached = usage
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .or_else(|| usage.get("prompt_cache_hit_tokens").and_then(Value::as_u64));
        let deepseek_miss = usage.get("prompt_cache_miss_tokens").and_then(Value::as_u64);
        let deepseek_hit_and_miss = usage.get("prompt_cache_hit_tokens").is_some() && deepseek_miss.is_some();
        let explicit_written = usage.pointer("/input_tokens_details/cache_write_tokens").and_then(Value::as_u64);
        let written = if explicit_written.is_some() {
            explicit_written
        } else if deepseek_hit_and_miss && deepseek_miss.is_some_and(|miss| miss > 0) {
            Some(0)
        } else {
            deepseek_miss
        };
        self.output.usage.input = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .map(|total| total.saturating_sub(cached.unwrap_or(0)).saturating_sub(written.unwrap_or(0)));
        self.output.usage.output = usage.get("output_tokens").and_then(Value::as_u64);
        self.output.usage.total_tokens = usage.get("total_tokens").and_then(Value::as_u64);
        self.output.usage.cache_read = cached;
        self.output.usage.cache_write = written;
        self.output.usage.reasoning_tokens =
            usage.pointer("/output_tokens_details/reasoning_tokens").and_then(Value::as_u64);
    }
}

fn response_error(response: &Value) -> String {
    if response.get("type").and_then(Value::as_str) == Some("error") {
        let error = response.get("error").filter(|value| !value.is_null()).unwrap_or(response);
        let code = error.get("code").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("unknown");
        let message = error.get("message").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("no message");
        return format!("Error Code {code}: {message}");
    }
    let error = response
        .get("error")
        .filter(|value| !value.is_null())
        .or_else(|| response.pointer("/status_details/error").filter(|value| !value.is_null()));
    match error {
        Some(Value::Object(error)) => {
            let code = error.get("code").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("unknown");
            let message =
                error.get("message").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("no message");
            return format!("{code}: {message}");
        }
        Some(Value::String(error)) if !error.is_empty() => return error.clone(),
        _ => {}
    }
    if let Some(reason) =
        response.pointer("/incomplete_details/reason").and_then(Value::as_str).filter(|s| !s.is_empty())
    {
        return format!("incomplete: {reason}");
    }
    if let Some(reason) = response.pointer("/status_details/reason").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        return format!("status_details: {reason}");
    }
    envelope_message(response).unwrap_or_else(|| "Unknown error (no error details in response)".into())
}

fn strict_arguments_value(raw: &str) -> Option<Value> {
    if raw.trim().is_empty() { Some(json!({})) } else { serde_json::from_str(raw).ok() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Model {
        Model {
            id: "m".into(),
            api: "openai-responses".into(),
            provider: "test".into(),
            base_url: "http://example.invalid/v1".into(),
            reasoning: false,
            max_tokens: None,
            context_window: None,
            tokenizer: None,
        }
    }

    fn types(events: &[AssistantMessageEvent]) -> Vec<&'static str> {
        events.iter().map(AssistantMessageEvent::type_name).collect()
    }

    #[test]
    fn reasoning_parts_keep_output_order_and_terminal_native_items() {
        let mut state = ResponsesStreamState::new(&model());
        let mut emitted = Vec::new();
        for frame in [
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning"}}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"reasoning"}}),
            json!({"type":"response.reasoning_summary_text.delta","output_index":1,"summary_index":0,"delta":"second"}),
            json!({"type":"response.reasoning_summary_text.delta","output_index":0,"summary_index":0,"delta":"first "}),
            json!({"type":"response.reasoning_summary_text.done","output_index":0,"summary_index":0,"text":"first "}),
            json!({"type":"response.output_item.done","output_index":1,"item":{"type":"reasoning","summary":[{"type":"summary_text","text":"second"}],"encrypted_content":"secret_2"}}),
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","summary":[{"type":"summary_text","text":"first "}],"encrypted_content":"secret_1"}}),
            json!({"type":"response.output_item.done","output_index":2,"item":{"type":"message","phase":"final_answer","content":[{"type":"output_text","text":"answer"}]}}),
            json!({"type":"response.completed","response":{"status":"completed"}}),
        ] {
            emitted.extend(state.handle(&frame).unwrap());
        }
        assert!(types(&emitted).contains(&"thinking_delta"));
        assert_eq!(state.output.content.len(), 3);
        assert!(
            matches!(&state.output.content[0], AssistantBlock::Thinking(block) if block.thinking == "first " && block.thinking_signature.as_ref().unwrap().contains("secret_1"))
        );
        assert!(
            matches!(&state.output.content[1], AssistantBlock::Thinking(block) if block.thinking == "second" && block.thinking_signature.as_ref().unwrap().contains("secret_2"))
        );
        let items = state.output.provider_payload.as_ref().unwrap()["items"].as_array().unwrap();
        assert_eq!(items[0]["encrypted_content"], "secret_1");
        assert_eq!(items[1]["encrypted_content"], "secret_2");
        assert_eq!(items[2]["phase"], "final_answer");
    }

    #[test]
    fn successful_hidden_and_incomplete_turns_capture_native_items() {
        let mut hidden = ResponsesStreamState::new(&model());
        hidden.handle(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","encrypted_content":"secret"}})).unwrap();
        hidden.handle(&json!({"type":"response.completed","response":{"status":"completed"}})).unwrap();
        assert_eq!(hidden.output.provider_payload.as_ref().unwrap()["items"][0]["encrypted_content"], "secret");
        let mut incomplete = ResponsesStreamState::new(&model());
        incomplete.handle(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","encrypted_content":"secret"}})).unwrap();
        incomplete.handle(&json!({"type":"response.output_item.done","output_index":1,"item":{"type":"message","content":[{"type":"output_text","text":"partial"}]}})).unwrap();
        incomplete.handle(&json!({"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}})).unwrap();
        assert_eq!(incomplete.output.provider_payload.as_ref().unwrap()["items"].as_array().unwrap().len(), 2);
        assert_eq!(incomplete.output.provider_payload.as_ref().unwrap()["dt"], true);
        assert_eq!(incomplete.output.stop_reason, StopReason::Length);
    }

    #[test]
    fn session_payload_fingerprints_endpoint_without_persisting_url_credentials() {
        let mut endpoint = model();
        endpoint.base_url = "https://sample-user:sample-password@example.invalid/v1?route=private".into();
        let mut state = ResponsesStreamState::new(&endpoint);
        state.handle(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","content":[{"type":"output_text","text":"answer"}]}})).unwrap();
        state.handle(&json!({"type":"response.completed","response":{"status":"completed"}})).unwrap();
        let saved = serde_json::to_string(&state.output).unwrap();
        assert!(!saved.contains("sample-password") && !saved.contains("route=private"));
        assert_eq!(state.output.provider_payload.as_ref().unwrap()["endpointSha256"].as_str().unwrap().len(), 64);
    }

    #[test]
    fn revised_reasoning_summary_does_not_emit_a_duplicate_delta() {
        let mut state = ResponsesStreamState::new(&model());
        state
            .handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning"}}))
            .unwrap();
        let first = state.handle(&json!({"type":"response.reasoning_summary_text.delta","output_index":0,"summary_index":0,"delta":"abc"})).unwrap();
        assert!(matches!(&first[0], AssistantMessageEvent::ThinkingDelta { delta, .. } if delta == "abc"));
        let revision = state
            .handle(
                &json!({"type":"response.reasoning_summary_text.done","output_index":0,"summary_index":0,"text":"abX"}),
            )
            .unwrap();
        assert!(revision.is_empty());
        assert!(matches!(&state.output.content[0], AssistantBlock::Thinking(block) if block.thinking == "abc"));
        let final_events = state.handle(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","summary":[{"type":"summary_text","text":"abX"}]}})).unwrap();
        assert!(matches!(&final_events[0], AssistantMessageEvent::ThinkingEnd { content, .. } if content == "abX"));
    }

    #[test]
    fn reasoning_summary_parts_emit_one_separator() {
        let mut state = ResponsesStreamState::new(&model());
        let mut deltas = String::new();
        for frame in [
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning"}}),
            json!({"type":"response.reasoning_summary_text.delta","output_index":0,"summary_index":0,"delta":"Plan"}),
            json!({"type":"response.reasoning_summary_part.done","output_index":0,"summary_index":0}),
            json!({"type":"response.reasoning_summary_part.added","output_index":0,"summary_index":1}),
            json!({"type":"response.reasoning_summary_text.delta","output_index":0,"summary_index":1,"delta":"Details"}),
        ] {
            for event in state.handle(&frame).unwrap() {
                if let AssistantMessageEvent::ThinkingDelta { delta, .. } = event {
                    deltas.push_str(&delta);
                }
            }
        }
        assert_eq!(deltas, "Plan\n\nDetails");
        let final_events = state.handle(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","summary":[{"type":"summary_text","text":"Plan"},{"type":"summary_text","text":"Details"}]}})).unwrap();
        assert!(
            matches!(&final_events[0], AssistantMessageEvent::ThinkingEnd { content, .. } if content == "Plan\n\nDetails")
        );
    }

    #[test]
    fn missing_final_reasoning_summary_keeps_streamed_part_boundary() {
        let mut state = ResponsesStreamState::new(&model());
        for frame in [
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning"}}),
            json!({"type":"response.reasoning_summary_text.delta","output_index":0,"summary_index":0,"delta":"Plan"}),
            json!({"type":"response.reasoning_summary_part.done","output_index":0,"summary_index":0}),
        ] {
            state.handle(&frame).unwrap();
        }
        let end = state
            .handle(
                &json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","summary":[]}}),
            )
            .unwrap();
        assert!(matches!(&end[0], AssistantMessageEvent::ThinkingEnd { content, .. } if content == "Plan\n\n"));
    }

    #[test]
    fn text_final_snapshot_and_usage_are_authoritative() {
        let mut state = ResponsesStreamState::new(&model());
        let mut emitted = Vec::new();
        for frame in [
            json!({"type":"response.created","response":{"id":"resp_1"}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_1"}}),
            json!({"type":"response.output_text.delta","output_index":0,"item_id":"msg_1","delta":"draft"}),
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_1","content":[{"type":"output_text","text":"final"}]}}),
            json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":7,"output_tokens":9,"total_tokens":16,"input_tokens_details":{"cached_tokens":2}}}}),
        ] {
            emitted.extend(state.handle(&frame).unwrap());
        }
        assert_eq!(types(&emitted), ["text_start", "text_delta", "text_end"]);
        assert_eq!(state.output.text(), "final");
        assert_eq!(state.output.response_id.as_deref(), Some("resp_1"));
        assert_eq!(state.output.usage.input, Some(5));
        assert_eq!(state.output.usage.cache_read, Some(2));
        assert_eq!(state.output.usage.output, Some(9));
        assert_eq!(state.output.usage.total_tokens, Some(16));
        assert!(state.terminal);
    }

    #[test]
    fn parallel_calls_route_deltas_and_done_arguments_to_original_blocks() {
        let mut state = ResponsesStreamState::new(&model());
        let mut emitted = Vec::new();
        for frame in [
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read"}}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"call_b","name":"bash"}}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"command\":"}),
            json!({"type":"response.function_call_arguments.delta","item_id":"fc_a","delta":"{\"path\":\"a\"}"}),
            json!({"type":"response.function_call_arguments.done","item_id":"fc_a","arguments":"{\"path\":\"b\"}"}),
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read","arguments":"{\"path\":\"wrong\"}"}}),
            json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"stale"}),
            json!({"type":"response.function_call_arguments.done","output_index":1,"arguments":"{\"command\":\"pwd\"}"}),
            json!({"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"call_b","name":"bash","arguments":"{}"}}),
            json!({"type":"response.completed","response":{"status":"completed"}}),
        ] {
            emitted.extend(state.handle(&frame).unwrap());
        }
        assert_eq!(state.output.stop_reason, StopReason::ToolUse);
        let calls = state.output.tool_calls().collect::<Vec<_>>();
        assert_eq!(calls[0].id, "call_a|fc_a");
        assert_eq!(calls[0].arguments["path"], "b");
        assert_eq!(calls[1].id, "call_b|fc_b");
        assert_eq!(calls[1].arguments["command"], "pwd");
        let ends = emitted
            .iter()
            .filter_map(|event| match event {
                AssistantMessageEvent::ToolcallEnd { content_index, tool_call, .. } => {
                    Some((*content_index, tool_call))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(ends.len(), 2);
        assert_eq!(ends[0].0, 0);
        assert_eq!(ends[1].0, 1);
        assert_eq!(ends[0].1.arguments, calls[0].arguments);
        assert_eq!(ends[1].1.arguments, calls[1].arguments);
    }

    #[test]
    fn incomplete_unfinished_tool_is_never_promoted_to_tool_use() {
        let mut state = ResponsesStreamState::new(&model());
        state.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read"}})).unwrap();
        state
            .handle(&json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"path\":"}))
            .unwrap();
        let events = state.handle(&json!({"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}})).unwrap();
        assert!(events.is_empty(), "partial arguments cannot emit toolcall_end");
        assert_eq!(state.output.stop_reason, StopReason::Length);
        assert!(state.terminal);
        let call = state.output.tool_calls().next().unwrap();
        assert_eq!(call.arguments["__rawJson"], "{\"path\":");
        assert!(call.arguments.contains_key("__parseError"));
    }

    #[test]
    fn content_filter_and_failed_response_are_errors() {
        let mut state = ResponsesStreamState::new(&model());
        let blocked = state.handle(&json!({"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"content_filter"}}}));
        assert!(matches!(blocked, Err(ProviderError::Stream(_))));
        let failed = state.handle(
            &json!({"type":"response.failed","response":{"status":"failed","error":{"message":"upstream failed"}}}),
        );
        assert!(matches!(failed, Err(ProviderError::Stream(message)) if message == "unknown: upstream failed"));
        assert!(state.output.provider_payload.is_none());
    }

    #[test]
    fn refusal_snapshot_survives_done_and_compat_cache_usage_is_accounted() {
        let mut state = ResponsesStreamState::new(&model());
        state
            .handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg"}}))
            .unwrap();
        state.handle(&json!({"type":"response.refusal.delta","output_index":0,"delta":"draft"})).unwrap();
        state.handle(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg","content":[{"type":"refusal","refusal":"final refusal"}]}})).unwrap();
        state.handle(&json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":10,"output_tokens":2,"prompt_cache_hit_tokens":4,"prompt_cache_miss_tokens":6}}})).unwrap();
        assert_eq!(state.output.text(), "final refusal");
        assert_eq!(state.output.usage.input, Some(6));
        assert_eq!(state.output.usage.cache_read, Some(4));
        assert_eq!(state.output.usage.cache_write, Some(0));
    }

    #[test]
    fn terminal_can_close_a_complete_call_without_output_item_done() {
        let mut state = ResponsesStreamState::new(&model());
        state.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read"}})).unwrap();
        state.handle(&json!({"type":"response.function_call_arguments.done","output_index":0,"arguments":"{\"path\":\"a\"}"})).unwrap();
        let events = state.handle(&json!({"type":"response.completed","response":{"status":"completed"}})).unwrap();
        assert_eq!(types(&events), ["toolcall_end"]);
        assert_eq!(state.output.stop_reason, StopReason::ToolUse);
        assert_eq!(state.output.tool_calls().next().unwrap().arguments["path"], "a");
    }

    #[test]
    fn incomplete_terminal_can_run_a_strictly_complete_open_call() {
        let mut state = ResponsesStreamState::new(&model());
        state.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read"}})).unwrap();
        state
            .handle(
                &json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"path\":\"a\"}"}),
            )
            .unwrap();
        state
            .handle(
                &json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}),
            )
            .unwrap();
        assert_eq!(state.output.stop_reason, StopReason::ToolUse);
        assert_eq!(state.output.tool_calls().next().unwrap().arguments["path"], "a");
    }

    #[test]
    fn incomplete_call_with_full_delta_but_no_arguments_done_is_not_executable() {
        let mut state = ResponsesStreamState::new(&model());
        state.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read"}})).unwrap();
        state
            .handle(
                &json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"path\":\"a\"}"}),
            )
            .unwrap();
        state.handle(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read","arguments":""}})).unwrap();
        state
            .handle(
                &json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}),
            )
            .unwrap();
        assert_eq!(state.output.stop_reason, StopReason::Length);
        assert_eq!(state.output.tool_calls().next().unwrap().arguments["path"], "a");
    }

    #[test]
    fn incomplete_terminal_snapshot_with_partial_arguments_stays_at_length() {
        let mut state = ResponsesStreamState::new(&model());
        state.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read"}})).unwrap();
        state
            .handle(
                &json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"path\":\"a\""}),
            )
            .unwrap();
        state.handle(&json!({"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read","arguments":"{\"path\":\"a\""}]}})).unwrap();
        assert_eq!(state.output.stop_reason, StopReason::Length);
        let call = state.output.tool_calls().next().unwrap();
        assert_eq!(call.arguments["__rawJson"], "{\"path\":\"a\"");
        assert!(call.arguments.contains_key("__parseError"));
        assert_eq!(state.output.provider_payload.as_ref().unwrap()["items"][0]["arguments"], "{\"path\":\"a\"");
    }

    #[test]
    fn open_partial_call_keeps_raw_arguments_even_when_preview_json_is_repairable() {
        let mut state = ResponsesStreamState::new(&model());
        state
            .handle(&json!({"type":"response.output_item.added","output_index":0,
            "item":{"type":"function_call","id":"fc_partial","call_id":"call_partial","name":"read"}}))
            .unwrap();
        state
            .handle(&json!({"type":"response.function_call_arguments.delta","output_index":0,
            "delta":"{\"path\":\"a.txt\","}))
            .unwrap();
        state
            .handle(&json!({"type":"response.incomplete","response":{
            "status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}))
            .unwrap();
        let call = state.output.tool_calls().next().unwrap();
        assert_eq!(state.output.stop_reason, StopReason::Length);
        assert_eq!(call.arguments["__rawJson"], "{\"path\":\"a.txt\",");
        assert!(call.arguments.contains_key("__parseError"));
        assert_eq!(state.output.provider_payload.as_ref().unwrap()["items"], json!([]));
    }

    #[test]
    fn a_complete_call_cannot_promote_an_open_partial_sibling() {
        let mut state = ResponsesStreamState::new(&model());
        state.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read"}})).unwrap();
        state.handle(&json!({"type":"response.function_call_arguments.done","output_index":0,"arguments":"{\"path\":\"a\"}"})).unwrap();
        state.handle(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read","arguments":""}})).unwrap();
        state.handle(&json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"call_b","name":"bash"}})).unwrap();
        state
            .handle(&json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"command\":"}))
            .unwrap();
        state
            .handle(
                &json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}),
            )
            .unwrap();
        assert_eq!(state.output.stop_reason, StopReason::Length);
        let calls = state.output.tool_calls().collect::<Vec<_>>();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments["path"], "a");
        assert_eq!(calls[1].arguments["__rawJson"], "{\"command\":");
        assert!(calls[1].arguments.contains_key("__parseError"));
    }

    #[test]
    fn changed_parallel_call_identity_fails_closed() {
        let mut state = ResponsesStreamState::new(&model());
        state.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read"}})).unwrap();
        state.handle(&json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"call_b","name":"bash"}})).unwrap();
        assert!(matches!(
            state.handle(&json!({"type":"response.function_call_arguments.done","output_index":0,"item_id":"fc_b","arguments":"{\"path\":\"wrong\"}"})),
            Err(ProviderError::Stream(_))
        ));
        assert!(matches!(
            state.handle(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_b","call_id":"call_a","name":"read","arguments":"{\"path\":\"wrong\"}"}})),
            Err(ProviderError::Stream(_))
        ));
        assert!(matches!(state.handle(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"changed","name":"read","arguments":"{}"}})), Err(ProviderError::Stream(_))));
    }

    #[test]
    fn identifierless_done_uses_added_order_and_rejects_conflicting_final_snapshot() {
        let mut state = ResponsesStreamState::new(&model());
        for frame in [
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"a","name":"write"}}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"b","name":"write"}}),
            json!({"type":"response.function_call_arguments.done","arguments":"{\"path\":\"a.txt\"}"}),
            json!({"type":"response.function_call_arguments.done","arguments":"{\"path\":\"b.txt\"}"}),
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"a","name":"write","arguments":""}}),
            json!({"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"b","name":"write","arguments":""}}),
            json!({"type":"response.completed","response":{"status":"completed"}}),
        ] {
            state.handle(&frame).unwrap();
        }
        let calls = state.output.tool_calls().collect::<Vec<_>>();
        assert_eq!(calls[0].arguments["path"], "a.txt");
        assert_eq!(calls[1].arguments["path"], "b.txt");

        let mut conflict = ResponsesStreamState::new(&model());
        conflict.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"a","name":"write"}})).unwrap();
        conflict.handle(&json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"b","name":"write"}})).unwrap();
        conflict
            .handle(&json!({"type":"response.function_call_arguments.done","arguments":"{\"path\":\"b.txt\"}"}))
            .unwrap();
        assert!(matches!(conflict.handle(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"a","name":"write","arguments":"{\"path\":\"a.txt\"}"}})), Err(ProviderError::Stream(_))));

        let mut snapshot = ResponsesStreamState::new(&model());
        snapshot.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"a","name":"write"}})).unwrap();
        snapshot
            .handle(&json!({"type":"response.function_call_arguments.done","arguments":"{\"path\":\"b.txt\"}"}))
            .unwrap();
        snapshot.handle(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"a","name":"write","arguments":""}})).unwrap();
        assert!(matches!(snapshot.handle(&json!({"type":"response.completed","response":{"status":"completed","output":[{"type":"function_call","id":"fc_a","call_id":"a","name":"write","arguments":"{\"path\":\"a.txt\"}"}]}})), Err(ProviderError::Stream(_))));

        let mut missing_call_id = ResponsesStreamState::new(&model());
        missing_call_id.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"a","name":"write"}})).unwrap();
        missing_call_id.handle(&json!({"type":"response.function_call_arguments.done","arguments":""})).unwrap();
        missing_call_id.handle(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"a","name":"write","arguments":""}})).unwrap();
        assert!(matches!(missing_call_id.handle(&json!({"type":"response.completed","response":{"status":"completed","output":[{"type":"function_call","id":"fc_a","name":"write","arguments":"{\"path\":\"surprise\"}"}]}})), Err(ProviderError::Stream(_))));
        assert!(matches!(missing_call_id.handle(&json!({"type":"response.completed","response":{"status":"completed","output":[{"type":"function_call","id":"fc_a","name":"bash","arguments":""}]}})), Err(ProviderError::Stream(_))));
        assert!(matches!(missing_call_id.handle(&json!({"type":"response.completed","response":{"status":"completed","output":[{"type":"message","id":"fc_a","content":[]}]}})), Err(ProviderError::Stream(_))));
    }

    #[test]
    fn identifierless_deltas_keep_braces_inside_strings_and_split_sibling_chunks() {
        let mut state = ResponsesStreamState::new(&model());
        state.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"a","name":"bash"}})).unwrap();
        state.handle(&json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"b","name":"write"}})).unwrap();
        let mut indexes = Vec::new();
        for delta in ["{\"command\":\"echo ", "{1..3}\"}", "{\"path\":\"b", ".txt\"}"] {
            for event in state.handle(&json!({"type":"response.function_call_arguments.delta","delta":delta})).unwrap()
            {
                if let AssistantMessageEvent::ToolcallDelta { content_index, .. } = event {
                    indexes.push(content_index);
                }
            }
        }
        assert_eq!(indexes, [0, 0, 1, 1]);
        for (index, id, call, name) in [(0, "fc_a", "a", "bash"), (1, "fc_b", "b", "write")] {
            state.handle(&json!({"type":"response.output_item.done","output_index":index,"item":{"type":"function_call","id":id,"call_id":call,"name":name,"arguments":""}})).unwrap();
        }
        state.handle(&json!({"type":"response.completed","response":{"status":"completed"}})).unwrap();
        let calls = state.output.tool_calls().collect::<Vec<_>>();
        assert_eq!(calls[0].arguments["command"], "echo {1..3}");
        assert_eq!(calls[1].arguments["path"], "b.txt");
    }

    #[test]
    fn identifierless_delta_advances_after_invalid_prefix_and_rejects_duplicate_call_id() {
        let mut state = ResponsesStreamState::new(&model());
        state.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"a","name":"bash"}})).unwrap();
        state.handle(&json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"b","name":"write"}})).unwrap();
        state.handle(&json!({"type":"response.function_call_arguments.delta","delta":"{\"command\":\"bad\n"})).unwrap();
        let events = state
            .handle(&json!({"type":"response.function_call_arguments.delta","delta":"{\"path\":\"b.txt\"}"}))
            .unwrap();
        assert!(matches!(&events[0], AssistantMessageEvent::ToolcallDelta { content_index: 1, .. }));
        assert!(matches!(state.handle(&json!({"type":"response.output_item.added","output_index":2,"item":{"type":"function_call","id":"fc_c","call_id":"b","name":"write"}})), Err(ProviderError::Stream(_))));
    }

    #[test]
    fn identifierless_delta_skips_two_complete_calls_when_keyed_and_unkeyed_mix() {
        let mut state = ResponsesStreamState::new(&model());
        for (index, id, call) in [(0, "fc_a", "a"), (1, "fc_b", "b"), (2, "fc_c", "c")] {
            state.handle(&json!({"type":"response.output_item.added","output_index":index,"item":{"type":"function_call","id":id,"call_id":call,"name":"write"}})).unwrap();
        }
        state.handle(&json!({"type":"response.function_call_arguments.delta","delta":"{\"path\":\"a.txt\"}"})).unwrap();
        state.handle(&json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"path\":\"b.txt\"}"})).unwrap();
        let events = state
            .handle(&json!({"type":"response.function_call_arguments.delta","delta":"{\"path\":\"c.txt\"}"}))
            .unwrap();
        assert!(matches!(&events[0], AssistantMessageEvent::ToolcallDelta { content_index: 2, .. }));
        for (index, id, call) in [(0, "fc_a", "a"), (1, "fc_b", "b"), (2, "fc_c", "c")] {
            state.handle(&json!({"type":"response.output_item.done","output_index":index,"item":{"type":"function_call","id":id,"call_id":call,"name":"write","arguments":""}})).unwrap();
        }
        state.handle(&json!({"type":"response.completed","response":{"status":"completed"}})).unwrap();
        let calls = state.output.tool_calls().collect::<Vec<_>>();
        assert_eq!(
            calls.iter().map(|call| call.arguments["path"].as_str().unwrap()).collect::<Vec<_>>(),
            ["a.txt", "b.txt", "c.txt"]
        );
    }

    #[test]
    fn identifierless_routing_has_a_cumulative_scan_bound() {
        let mut state = ResponsesStreamState::new(&model());
        state.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"a","name":"write"}})).unwrap();
        state.identifierless_scan_work = 128 * 1024 * 1024;
        assert!(matches!(
            state.handle(&json!({"type":"response.function_call_arguments.delta","delta":"{"})),
            Err(ProviderError::Stream(_))
        ));
    }

    #[test]
    fn prefixed_call_alias_wins_without_index_but_index_disambiguates_collision() {
        let mut state = ResponsesStreamState::new(&model());
        state.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"item_a","call_id":"x","name":"read"}})).unwrap();
        state.handle(&json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_x","call_id":"y","name":"bash"}})).unwrap();
        state.handle(&json!({"type":"response.function_call_arguments.done","item_id":"fc_x","arguments":"{\"path\":\"a\"}"})).unwrap();
        state.handle(&json!({"type":"response.function_call_arguments.done","output_index":1,"item_id":"fc_x","arguments":"{\"command\":\"pwd\"}"})).unwrap();
        state.handle(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"item_a","call_id":"x","name":"read","arguments":""}})).unwrap();
        state.handle(&json!({"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc_x","call_id":"y","name":"bash","arguments":""}})).unwrap();
        state.handle(&json!({"type":"response.completed","response":{"status":"completed"}})).unwrap();
        let calls = state.output.tool_calls().collect::<Vec<_>>();
        assert_eq!(calls[0].arguments["path"], "a");
        assert_eq!(calls[1].arguments["command"], "pwd");
    }
}
