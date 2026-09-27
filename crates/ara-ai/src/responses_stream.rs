//! Responses text and JSON function-call event state.
//!
//! Source: pinned OMP `openai-shared.ts::processResponsesStream` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d. Native reasoning replay
//! and provider-hosted tools remain separate parity gaps.

use crate::error::{ProviderError, envelope_message};
use crate::event::AssistantMessageEvent;
use crate::json::parse_final_arguments;
use crate::types::{AssistantBlock, AssistantMessage, Model, StopReason, TextContent, ToolCall};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ItemKind {
    Text,
    Function,
}

struct OpenItem {
    kind: ItemKind,
    content_index: usize,
    output_index: Option<u64>,
    item_id: Option<String>,
    call_id: Option<String>,
    argument_bytes: String,
    final_arguments: Option<String>,
}

pub(crate) struct ResponsesStreamState {
    pub(crate) output: AssistantMessage,
    open: HashMap<usize, OpenItem>,
    by_index: HashMap<u64, usize>,
    by_id: HashMap<String, usize>,
    by_call: HashMap<String, usize>,
    by_prefixed_call: HashMap<String, usize>,
    done_indices: HashSet<u64>,
    done_ids: HashSet<String>,
    completed_tool_args: Vec<bool>,
    next_key: usize,
    pub(crate) terminal: bool,
    pub(crate) replay_unsafe_wire_event: bool,
}

impl ResponsesStreamState {
    pub(crate) fn new(model: &Model) -> Self {
        Self {
            output: AssistantMessage::empty(&model.api, &model.provider, &model.id),
            open: HashMap::new(),
            by_index: HashMap::new(),
            by_id: HashMap::new(),
            by_call: HashMap::new(),
            by_prefixed_call: HashMap::new(),
            done_indices: HashSet::new(),
            done_ids: HashSet::new(),
            completed_tool_args: Vec::new(),
            next_key: 0,
            terminal: false,
            replay_unsafe_wire_event: false,
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
        }
        if let Some(index) = index {
            self.by_index.insert(index, key);
        }
        if let Some(id) = &id {
            self.by_id.insert(id.clone(), key);
        }
        if let Some(call) = &call {
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
            },
        );
        Ok(key)
    }

    fn close(&mut self, key: usize) {
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

    fn done_item(
        &mut self,
        event: &Value,
        item: &Value,
        events: &mut Vec<AssistantMessageEvent>,
        terminal_fallback: bool,
    ) -> Result<(), ProviderError> {
        let index = event.get("output_index").and_then(Value::as_u64);
        let id = item.get("id").and_then(Value::as_str);
        if index.is_some_and(|index| self.done_indices.contains(&index))
            || id.is_some_and(|id| self.done_ids.contains(id))
        {
            return Ok(());
        }
        let kind = match item.get("type").and_then(Value::as_str) {
            Some("message") => ItemKind::Text,
            Some("function_call") => ItemKind::Function,
            Some("reasoning") => return Ok(()),
            _ => return Err(ProviderError::Stream("Unsupported Responses output item".into())),
        };
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
                self.output.content[content_index] = AssistantBlock::ToolCall(tool_call.clone());
                self.completed_tool_args.push(proven_complete && !tool_call.arguments.contains_key("__parseError"));
                events.push(AssistantMessageEvent::ToolcallEnd {
                    content_index,
                    tool_call,
                    partial: self.output.clone(),
                });
            }
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
                    Some("reasoning") => {}
                    _ => {
                        self.replay_unsafe_wire_event = true;
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
            "response.function_call_arguments.delta" => {
                let key = self.lookup(event, true)?;
                if key.is_none()
                    && event.get("output_index").is_none()
                    && event.get("item_id").is_none()
                    && self.open.len() > 1
                {
                    return Err(ProviderError::Stream("Ambiguous Responses function arguments delta".into()));
                }
                if let Some(key) = key {
                    let open = self.open.get_mut(&key).expect("indexed item exists");
                    if open.kind != ItemKind::Function {
                        return Err(ProviderError::Stream("Responses arguments routed to non-function item".into()));
                    }
                    let delta = event
                        .get("delta")
                        .and_then(Value::as_str)
                        .ok_or_else(|| ProviderError::Stream("Responses arguments delta is missing".into()))?;
                    if open.argument_bytes.len().saturating_add(delta.len()) > 8 * 1024 * 1024 {
                        return Err(ProviderError::Stream("Responses function arguments exceed size limit".into()));
                    }
                    open.argument_bytes.push_str(delta);
                    self.replay_unsafe_wire_event = true;
                    events.push(AssistantMessageEvent::ToolcallDelta {
                        content_index: open.content_index,
                        delta: delta.to_owned(),
                        partial: self.output.clone(),
                    });
                }
            }
            "response.function_call_arguments.done" => {
                let key = self.lookup(event, true)?;
                if key.is_none()
                    && event.get("output_index").is_none()
                    && event.get("item_id").is_none()
                    && self.open.len() > 1
                {
                    return Err(ProviderError::Stream("Ambiguous Responses final function arguments".into()));
                }
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
                    self.replay_unsafe_wire_event = true;
                }
            }
            "response.output_item.done" => {
                self.replay_unsafe_wire_event = true;
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
                        }
                    }
                    "incomplete"
                        if response.pointer("/incomplete_details/reason").and_then(Value::as_str)
                            == Some("max_output_tokens") =>
                    {
                        if self.open.values().any(|item| item.kind == ItemKind::Function) {
                            return Err(ProviderError::Incomplete(
                                "Responses terminal left a function call unfinished".into(),
                            ));
                        }
                        self.output.stop_reason = if !self.completed_tool_args.is_empty()
                            && self.completed_tool_args.iter().all(|complete| *complete)
                        {
                            StopReason::ToolUse
                        } else {
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
                        if self.open.values().any(|item| item.kind == ItemKind::Function) {
                            return Err(ProviderError::Incomplete(
                                "Responses terminal left a function call unfinished".into(),
                            ));
                        }
                        self.output.stop_reason = StopReason::Length;
                    }
                    "failed" | "cancelled" => return Err(ProviderError::Stream(response_error(response))),
                    _ => {
                        return Err(ProviderError::Stream(format!(
                            "Responses terminal has unsupported status {status}"
                        )));
                    }
                }
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
    response
        .get("error")
        .and_then(envelope_message)
        .or_else(|| envelope_message(response))
        .unwrap_or_else(|| "Responses provider returned an error".into())
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
            tokenizer: None,
        }
    }

    fn types(events: &[AssistantMessageEvent]) -> Vec<&'static str> {
        events.iter().map(AssistantMessageEvent::type_name).collect()
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
        let result = state.handle(&json!({"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}));
        assert!(matches!(result, Err(ProviderError::Incomplete(_))));
        assert!(!state.terminal);
    }

    #[test]
    fn content_filter_and_failed_response_are_errors() {
        let mut state = ResponsesStreamState::new(&model());
        let blocked = state.handle(&json!({"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"content_filter"}}}));
        assert!(matches!(blocked, Err(ProviderError::Stream(_))));
        let failed = state.handle(
            &json!({"type":"response.failed","response":{"status":"failed","error":{"message":"upstream failed"}}}),
        );
        assert!(matches!(failed, Err(ProviderError::Stream(message)) if message == "upstream failed"));
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
    fn a_complete_call_cannot_promote_an_open_partial_sibling() {
        let mut state = ResponsesStreamState::new(&model());
        state.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read"}})).unwrap();
        state.handle(&json!({"type":"response.function_call_arguments.done","output_index":0,"arguments":"{\"path\":\"a\"}"})).unwrap();
        state.handle(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read","arguments":""}})).unwrap();
        state.handle(&json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"call_b","name":"bash"}})).unwrap();
        state
            .handle(&json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"command\":"}))
            .unwrap();
        let result = state.handle(
            &json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}),
        );
        assert!(matches!(result, Err(ProviderError::Incomplete(_))));
        assert_ne!(state.output.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn ambiguous_parallel_arguments_and_changed_call_identity_fail_closed() {
        let mut state = ResponsesStreamState::new(&model());
        state.handle(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read"}})).unwrap();
        state.handle(&json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_b","call_id":"call_b","name":"bash"}})).unwrap();
        assert!(matches!(
            state.handle(&json!({"type":"response.function_call_arguments.done","arguments":"{}"})),
            Err(ProviderError::Stream(_))
        ));
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
