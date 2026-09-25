//! OpenAI-compatible Chat Completions adapter.
//!
//! Ported from OMP `packages/ai/src/providers/openai-completions.ts`
//! (`streamOpenAICompletionsOnce`, `buildParams`, `convertMessages`,
//! `convertTools`, `mapStopReason`, `parseChunkUsage`) and
//! `packages/ai/src/utils/openai-http.ts` / `packages/utils/src/fetch-retry.ts`
//! at 596f2da7101178214aa27a753529d15e6b7ad91d.
//!
//! Ported: default-compat request shaping, one system message per prompt,
//! tool-call/tool-result replay, `stream_options.include_usage`, tool choice
//! guards, pre-stream transport retries (408/429/5xx, Retry-After), SSE chunk
//! handling (text, `reasoning_content`/`reasoning`/`reasoning_text`, indexed
//! tool-call deltas, in-band error envelopes, finish-reason mapping,
//! `stop`→`toolUse` promotion, `[DONE]`-terminated streams, premature close
//! detection, trailing usage chunk, post-finish grace), first-event and idle
//! timeouts that ignore keep-alive chunks, and caller cancellation.
//!
//! Not ported (open ledger items): per-host compat policy tables (Mistral ids,
//! DeepSeek token stripping, reasoning replay fields, strict tools, prompt cache
//! keys, OpenRouter routing), markup healing, object-shaped streamed arguments
//! merge beyond top-level keys, reasoning_details signatures, Copilot/Azure
//! setup, cost calculation, the replay-safe whole-stream retry wrapper.

use crate::error::{ProviderError, envelope_message, parse_error_envelope};
use crate::event::{AssistantMessageEvent, AssistantStream, EventSink};
use crate::json::{parse_final_arguments, parse_streaming_json};
use crate::sse::SseDecoder;
use crate::transform::transform_messages;
use crate::types::{
    AssistantBlock, AssistantMessage, Context, JsonObject, Message, Model, StopReason, TextContent, ThinkingContent,
    Tool, ToolCall, ToolChoice, Usage, UserBlock, UserContent,
};
use futures::StreamExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub const API: &str = "openai-completions";
pub const FIRST_EVENT_TIMEOUT_MESSAGE: &str = "OpenAI completions stream timed out while waiting for the first event";
pub const IDLE_TIMEOUT_MESSAGE: &str = "OpenAI completions stream stalled while waiting for the next event";
pub const INCOMPLETE_STREAM_MESSAGE: &str = "OpenAI completions stream closed before a finish_reason was received";
pub const EMPTY_STREAM_MESSAGE: &str = "OpenAI completions stream ended without any event";
pub const NON_VISION_IMAGE_PLACEHOLDER: &str = "[image omitted: model does not support vision]";
pub const POST_FINISH_GRACE: Duration = Duration::from_millis(2_500);
const REASONING_FIELDS: [&str; 3] = ["reasoning_content", "reasoning", "reasoning_text"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaxTokensField {
    MaxTokens,
    MaxCompletionTokens,
}

/// Host-selected wire compatibility flags (subset of OMP `ResolvedOpenAICompat`).
#[derive(Clone, Debug, PartialEq)]
pub struct OpenAICompat {
    pub supports_developer_role: bool,
    pub supports_usage_in_streaming: bool,
    pub supports_images: bool,
    pub max_tokens_field: MaxTokensField,
}

impl Default for OpenAICompat {
    fn default() -> Self {
        OpenAICompat {
            supports_developer_role: false,
            supports_usage_in_streaming: true,
            supports_images: true,
            max_tokens_field: MaxTokensField::MaxTokens,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RetryPolicy {
    /// Total HTTP attempts including the first (upstream default 6).
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy { max_attempts: 6, base_delay: Duration::from_millis(500), max_delay: Duration::from_secs(60) }
    }
}

#[derive(Clone, Debug)]
pub struct StreamOptions {
    pub api_key: Option<String>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub tool_choice: Option<ToolChoice>,
    pub cancel: CancellationToken,
    /// `None` disables the watchdog (upstream default 300 s).
    pub first_event_timeout: Option<Duration>,
    pub idle_timeout: Option<Duration>,
    pub extra_headers: Vec<(String, String)>,
    pub compat: OpenAICompat,
    pub retry: RetryPolicy,
}

impl Default for StreamOptions {
    fn default() -> Self {
        StreamOptions {
            api_key: None,
            max_tokens: None,
            temperature: None,
            tool_choice: None,
            cancel: CancellationToken::new(),
            first_event_timeout: Some(Duration::from_secs(300)),
            idle_timeout: Some(Duration::from_secs(300)),
            extra_headers: Vec::new(),
            compat: OpenAICompat::default(),
            retry: RetryPolicy::default(),
        }
    }
}

// ----------------------------------------------------------------- request

fn serialize_tool_arguments(args: &JsonObject) -> String {
    serde_json::to_string(args).unwrap_or_else(|_| "{}".into())
}

fn image_url(block: &crate::types::ImageContent) -> Value {
    json!({"type": "image_url", "image_url": {"url": format!("data:{};base64,{}", block.mime_type, block.data)}})
}

/// OMP `convertMessages` for the default compat profile.
pub fn convert_messages(model: &Model, context: &Context, compat: &OpenAICompat) -> Vec<Value> {
    let _ = model;
    let mut params: Vec<Value> = Vec::new();
    for prompt in context.system_prompt.iter().filter(|p| !p.trim().is_empty()) {
        params.push(json!({"role": "system", "content": prompt}));
    }
    let transformed = transform_messages(&context.messages);
    let mut generated_ids = 0usize;
    let mut remapped: HashMap<String, Vec<String>> = HashMap::new();
    let mut i = 0;
    while i < transformed.len() {
        match &transformed[i] {
            Message::User(u) => push_user(&mut params, "user", &u.content, compat),
            Message::Developer(d) => {
                let role = if compat.supports_developer_role { "developer" } else { "user" };
                push_user(&mut params, role, &d.content, compat)
            }
            Message::Assistant(a) => {
                let text: String = a
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        AssistantBlock::Text(t) if !t.text.trim().is_empty() => Some(t.text.as_str()),
                        _ => None,
                    })
                    .collect();
                let mut msg = json!({"role": "assistant", "content": Value::Null});
                if !text.is_empty() {
                    msg["content"] = Value::String(text);
                }
                let calls: Vec<Value> = a
                    .tool_calls()
                    .enumerate()
                    .map(|(n, tc)| {
                        let id = if tc.id.trim().is_empty() {
                            generated_ids += 1;
                            format!("call_ara_{i}_{n}_{generated_ids}")
                        } else {
                            tc.id.clone()
                        };
                        remapped.entry(tc.id.clone()).or_default().push(id.clone());
                        json!({"id": id, "type": "function", "function": {"name": tc.name, "arguments": serialize_tool_arguments(&tc.arguments)}})
                    })
                    .collect();
                if !calls.is_empty() {
                    msg["tool_calls"] = Value::Array(calls);
                    if msg["content"].is_null() {
                        msg["content"] = Value::String(String::new());
                    }
                }
                if msg["content"].is_null() && msg.get("tool_calls").is_none() {
                    i += 1;
                    continue;
                }
                params.push(msg);
            }
            Message::ToolResult(_) => {
                let mut images: Vec<Value> = Vec::new();
                while let Some(Message::ToolResult(r)) = transformed.get(i) {
                    let text = r
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            UserBlock::Text(t) => Some(t.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let has_images = r.content.iter().any(|b| matches!(b, UserBlock::Image(_)));
                    let content = if has_images && !compat.supports_images {
                        if text.is_empty() {
                            NON_VISION_IMAGE_PLACEHOLDER.to_string()
                        } else {
                            format!("{text}\n{NON_VISION_IMAGE_PLACEHOLDER}")
                        }
                    } else if !text.is_empty() {
                        text
                    } else if has_images {
                        "(see attached image)".to_string()
                    } else {
                        String::new()
                    };
                    let id = remapped
                        .get_mut(&r.tool_call_id)
                        .and_then(|q| if q.is_empty() { None } else { Some(q.remove(0)) })
                        .unwrap_or_else(|| r.tool_call_id.clone());
                    params.push(json!({"role": "tool", "content": content, "tool_call_id": id}));
                    if has_images && compat.supports_images {
                        for block in &r.content {
                            if let UserBlock::Image(img) = block {
                                images.push(image_url(img));
                            }
                        }
                    }
                    i += 1;
                }
                if !images.is_empty() {
                    let mut content = vec![json!({"type": "text", "text": "Attached image(s) from tool result:"})];
                    content.extend(images);
                    params.push(json!({"role": "user", "content": content}));
                }
                continue;
            }
        }
        i += 1;
    }
    params
}

fn push_user(params: &mut Vec<Value>, role: &str, content: &UserContent, compat: &OpenAICompat) {
    match content {
        UserContent::Text(text) => {
            if !text.trim().is_empty() {
                params.push(json!({"role": role, "content": text}));
            }
        }
        UserContent::Blocks(blocks) => {
            let mut parts: Vec<Value> = Vec::new();
            let mut omitted = false;
            for block in blocks {
                match block {
                    UserBlock::Text(t) if !t.text.trim().is_empty() => {
                        parts.push(json!({"type": "text", "text": t.text}))
                    }
                    UserBlock::Text(_) => {}
                    UserBlock::Image(img) if compat.supports_images => parts.push(image_url(img)),
                    UserBlock::Image(_) => omitted = true,
                }
            }
            if omitted {
                parts.push(json!({"type": "text", "text": NON_VISION_IMAGE_PLACEHOLDER}));
            }
            if !parts.is_empty() {
                params.push(json!({"role": "user", "content": parts}));
            }
        }
    }
}

fn convert_tools(tools: &[Tool]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}}))
        .collect()
}

fn has_tool_history(messages: &[Message]) -> bool {
    messages.iter().any(|m| match m {
        Message::Assistant(a) => a.tool_calls().next().is_some(),
        Message::ToolResult(_) => true,
        _ => false,
    })
}

/// OMP `buildParams` for the default compat profile.
pub fn build_params(model: &Model, context: &Context, options: &StreamOptions) -> Value {
    let compat = &options.compat;
    let mut params = json!({"model": model.id, "messages": [], "stream": true});
    if compat.supports_usage_in_streaming {
        params["stream_options"] = json!({"include_usage": true});
    }
    if let Some(t) = options.temperature {
        params["temperature"] = json!(t);
    }
    match &context.tools {
        Some(tools) if !tools.is_empty() => params["tools"] = Value::Array(convert_tools(tools)),
        None if has_tool_history(&context.messages) => params["tools"] = json!([]),
        _ => {}
    }
    let tool_names: Vec<&str> = context.tools.iter().flatten().map(|t| t.name.as_str()).collect();
    if let Some(choice) = &options.tool_choice {
        let wire = match choice {
            ToolChoice::Auto => Some(json!("auto")),
            ToolChoice::None | ToolChoice::Required if tool_names.is_empty() => None,
            ToolChoice::None => Some(json!("none")),
            ToolChoice::Required => Some(json!("required")),
            ToolChoice::Tool(name) if tool_names.contains(&name.as_str()) => {
                Some(json!({"type": "function", "function": {"name": name}}))
            }
            ToolChoice::Tool(_) => None,
        };
        if let Some(wire) = wire {
            params["tool_choice"] = wire;
        }
    }
    params["messages"] = Value::Array(convert_messages(model, context, compat));
    if let Some(max) = options.max_tokens {
        let field = match compat.max_tokens_field {
            MaxTokensField::MaxTokens => "max_tokens",
            MaxTokensField::MaxCompletionTokens => "max_completion_tokens",
        };
        params[field] = json!(max);
    }
    params
}

// ----------------------------------------------------------------- response

/// OMP `mapStopReason`.
pub fn map_stop_reason(reason: Option<&str>) -> (StopReason, Option<String>) {
    let Some(reason) = reason else { return (StopReason::Stop, None) };
    match reason.to_ascii_lowercase().as_str() {
        "stop" | "end" => (StopReason::Stop, None),
        "length" | "max_tokens" => (StopReason::Length, None),
        "function_call" | "tool_calls" => (StopReason::ToolUse, None),
        "content_filter" => (StopReason::Error, Some("Provider finish_reason: content_filter".into())),
        "network_error" => (StopReason::Error, Some("Provider finish_reason: network_error".into())),
        "error" => (StopReason::Error, Some("Provider returned error finish_reason".into())),
        "insufficient_system_resource" => {
            (StopReason::Error, Some("Provider returned error finish_reason: insufficient_system_resource".into()))
        }
        _ => (StopReason::Error, Some(format!("Provider finish_reason: {reason}"))),
    }
}

fn first_positive(values: &[Option<u64>]) -> u64 {
    values.iter().flatten().copied().find(|v| *v > 0).unwrap_or(0)
}

/// OMP `parseChunkUsage` + `calculateOpenAIUsageAccounting` (no pricing).
pub fn parse_chunk_usage(raw: &Value) -> Usage {
    let num = |v: Option<&Value>| v.and_then(Value::as_u64);
    let prompt_details = raw.get("prompt_tokens_details");
    let completion_details = raw.get("completion_tokens_details");
    let prompt = num(raw.get("prompt_tokens"));
    let completion = num(raw.get("completion_tokens"));
    let cached = first_positive(&[
        num(raw.get("cached_tokens")),
        num(raw.get("prompt_cache_hit_tokens")),
        num(prompt_details.and_then(|d| d.get("cached_tokens"))),
        num(raw.get("cachedContentTokenCount")),
    ]);
    let reasoning = num(completion_details.and_then(|d| d.get("reasoning_tokens"))).unwrap_or(0);
    let cache_write_or = num(prompt_details.and_then(|d| d.get("cache_write_tokens")));
    let cache_miss_ds = num(raw.get("prompt_cache_miss_tokens"));
    let deepseek = raw.get("prompt_cache_hit_tokens").is_some_and(Value::is_u64)
        && cache_miss_ds.is_some()
        && cache_write_or.is_none()
        && cache_miss_ds.unwrap_or(0) > 0;
    let cache_write_tokens = cache_write_or.or(cache_miss_ds).unwrap_or(0);
    let cache_read_reported = [
        raw.get("cached_tokens"),
        raw.get("prompt_cache_hit_tokens"),
        prompt_details.and_then(|d| d.get("cached_tokens")),
        raw.get("cachedContentTokenCount"),
    ]
    .iter()
    .any(|v| v.is_some_and(Value::is_u64));
    let cache_write_reported = cache_write_or.is_some() || cache_miss_ds.is_some();
    if prompt.is_none() && completion.is_none() {
        return Usage::unknown();
    }
    let prompt_v = prompt.unwrap_or(0);
    let output = completion.unwrap_or(0);
    let input = if deepseek {
        prompt_v.saturating_sub(cached)
    } else {
        prompt_v.saturating_sub(cached).saturating_sub(cache_write_tokens)
    };
    let cache_write = if deepseek { 0 } else { cache_write_tokens };
    let cost = raw.get("cost").and_then(Value::as_f64).map(|total| crate::types::Cost { total, ..Default::default() });
    Usage {
        input: prompt.map(|_| input),
        output: completion,
        cache_read: cache_read_reported.then_some(cached),
        cache_write: cache_write_reported.then_some(cache_write),
        // A total is only known when both conversation buckets were reported.
        total_tokens: (prompt.is_some() && completion.is_some()).then_some(input + output + cached + cache_write),
        reasoning_tokens: (reasoning > 0).then_some(reasoning),
        cost,
    }
}

fn has_positive_cache_read(raw: &Value) -> bool {
    [
        raw.get("cached_tokens"),
        raw.get("prompt_cache_hit_tokens"),
        raw.pointer("/prompt_tokens_details/cached_tokens"),
        raw.get("cachedContentTokenCount"),
    ]
    .iter()
    .any(|v| v.and_then(Value::as_u64).is_some_and(|n| n > 0))
}

/// OMP `isOpenAICompletionsProgressChunk`: keep-alive chunks do not reset the idle timer.
pub fn is_progress_chunk(chunk: &Value) -> bool {
    if chunk.get("usage").is_some_and(|u| !u.is_null()) {
        return true;
    }
    let Some(choice) = chunk.get("choices").and_then(|c| c.get(0)) else { return false };
    if choice.get("finish_reason").is_some_and(|f| !f.is_null()) || choice.get("usage").is_some_and(|u| !u.is_null()) {
        return true;
    }
    let Some(delta) = choice.get("delta") else { return false };
    match delta.get("content") {
        Some(Value::String(s)) if !s.is_empty() => return true,
        Some(Value::Array(a)) if !a.is_empty() => return true,
        _ => {}
    }
    if delta.get("tool_calls").and_then(Value::as_array).is_some_and(|a| !a.is_empty()) {
        return true;
    }
    ["reasoning", "reasoning_content", "reasoning_text", "refusal"]
        .iter()
        .any(|k| delta.get(*k).and_then(Value::as_str).is_some_and(|s| !s.is_empty()))
}

fn stream_error(chunk: &Value) -> Option<ProviderError> {
    let error = chunk.get("error");
    let structured = error.is_some_and(Value::is_object);
    let flat = chunk.get("message").is_some_and(Value::is_string) && chunk.get("choices").is_none();
    if !structured && !error.is_some_and(Value::is_string) && !flat {
        return None;
    }
    let detail = envelope_message(chunk)
        .unwrap_or_else(|| "Provider returned an in-band OpenAI completions stream error".into());
    if !structured {
        return Some(ProviderError::Stream(detail));
    }
    let err = error.unwrap();
    let status = match err.get("code") {
        Some(Value::Number(n)) => n.as_u64().map(|v| v as u16),
        Some(Value::String(s)) if s.trim().len() == 3 => s.trim().parse::<u16>().ok(),
        _ => None,
    }
    .filter(|s| (400..=599).contains(s))
    .or_else(|| match err.get("type").and_then(Value::as_str).map(|t| t.trim().to_ascii_uppercase()).as_deref() {
        Some("SERVICE_UNAVAILABLE") => Some(503),
        Some("TOO_MANY_REQUESTS") => Some(429),
        Some("REQUEST_TIMEOUT") => Some(408),
        _ => None,
    });
    Some(match status {
        Some(status) => ProviderError::Http { status, detail },
        None => ProviderError::Stream(detail),
    })
}

fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| match p {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => o.get("text").and_then(Value::as_str).map(str::to_string),
                _ => None,
            })
            .collect(),
        _ => String::new(),
    }
}

/// Outcome of one decoded chunk.
#[derive(Debug, PartialEq)]
pub enum ChunkFlow {
    Continue,
    /// Response complete; stop reading.
    Break,
}

/// Chat Completions chunk state machine. Pure: returns the events to emit.
pub struct ChunkState {
    pub output: AssistantMessage,
    current: Option<usize>,
    tool_by_index: HashMap<u64, usize>,
    pending_tools: Vec<usize>,
    partial_args: HashMap<usize, String>,
    pub finished: bool,
    pub saw_usage: bool,
    await_trailing_usage: bool,
    pub saw_done: bool,
    /// At least one JSON `data:` frame was decoded.
    pub saw_frame: bool,
    pub first_token: Option<Instant>,
    last_display_parse: HashMap<usize, usize>,
}

impl ChunkState {
    pub fn new(model: &Model) -> Self {
        ChunkState {
            output: AssistantMessage::empty(API, &model.provider, &model.id),
            current: None,
            tool_by_index: HashMap::new(),
            pending_tools: Vec::new(),
            partial_args: HashMap::new(),
            finished: false,
            saw_usage: false,
            await_trailing_usage: false,
            saw_done: false,
            saw_frame: false,
            first_token: None,
            last_display_parse: HashMap::new(),
        }
    }

    fn snapshot(&self) -> AssistantMessage {
        self.output.clone()
    }

    fn is_tool(&self, idx: usize) -> bool {
        matches!(self.output.content.get(idx), Some(AssistantBlock::ToolCall(_)))
    }

    fn finish_tool(&mut self, idx: usize, events: &mut Vec<AssistantMessageEvent>) {
        let Some(raw) = self.partial_args.remove(&idx) else { return };
        self.last_display_parse.remove(&idx);
        self.pending_tools.retain(|i| *i != idx);
        self.tool_by_index.retain(|_, v| *v != idx);
        if let Some(AssistantBlock::ToolCall(call)) = self.output.content.get_mut(idx) {
            call.arguments = parse_final_arguments(&raw);
            let tool_call = call.clone();
            events.push(AssistantMessageEvent::ToolcallEnd {
                content_index: idx,
                tool_call,
                partial: self.output.clone(),
            });
        }
    }

    fn finish_block(&mut self, idx: Option<usize>, events: &mut Vec<AssistantMessageEvent>) {
        let Some(idx) = idx else { return };
        match self.output.content.get(idx) {
            Some(AssistantBlock::Text(t)) => {
                let content = t.text.clone();
                events.push(AssistantMessageEvent::TextEnd { content_index: idx, content, partial: self.snapshot() });
            }
            Some(AssistantBlock::Thinking(t)) => {
                let content = t.thinking.clone();
                events.push(AssistantMessageEvent::ThinkingEnd {
                    content_index: idx,
                    content,
                    partial: self.snapshot(),
                });
            }
            Some(AssistantBlock::ToolCall(_)) => self.finish_tool(idx, events),
            _ => {}
        }
    }

    fn finish_pending_tools(&mut self, events: &mut Vec<AssistantMessageEvent>) {
        for idx in self.pending_tools.clone() {
            self.finish_tool(idx, events);
        }
    }

    /// Close open blocks (normal end and error path alike).
    pub fn close_open_blocks(&mut self, events: &mut Vec<AssistantMessageEvent>) {
        let current = self.current.take();
        if let Some(idx) = current
            && !self.is_tool(idx)
        {
            self.finish_block(Some(idx), events);
        }
        self.finish_pending_tools(events);
    }

    fn append_text(&mut self, text: &str, events: &mut Vec<AssistantMessageEvent>) {
        if text.is_empty() {
            return;
        }
        self.first_token.get_or_insert_with(Instant::now);
        let is_text = self.current.is_some_and(|i| matches!(self.output.content.get(i), Some(AssistantBlock::Text(_))));
        if !is_text {
            // Tool-call blocks stay pending across text: index-only continuation chunks still find them.
            if let Some(i) = self.current
                && !self.is_tool(i)
            {
                self.finish_block(Some(i), events);
            }
            self.output.content.push(AssistantBlock::Text(TextContent { text: String::new(), text_signature: None }));
            let idx = self.output.content.len() - 1;
            self.current = Some(idx);
            events.push(AssistantMessageEvent::TextStart { content_index: idx, partial: self.snapshot() });
        }
        let idx = self.current.unwrap();
        if let Some(AssistantBlock::Text(t)) = self.output.content.get_mut(idx) {
            t.text.push_str(text);
        }
        events.push(AssistantMessageEvent::TextDelta {
            content_index: idx,
            delta: text.to_string(),
            partial: self.snapshot(),
        });
    }

    fn append_thinking(&mut self, text: &str, signature: &str, events: &mut Vec<AssistantMessageEvent>) {
        self.first_token.get_or_insert_with(Instant::now);
        let same = self.current.is_some_and(|i| {
            matches!(self.output.content.get(i), Some(AssistantBlock::Thinking(t)) if t.thinking_signature.as_deref() == Some(signature))
        });
        if !same {
            if let Some(i) = self.current
                && !self.is_tool(i)
            {
                self.finish_block(Some(i), events);
            }
            self.output.content.push(AssistantBlock::Thinking(ThinkingContent {
                thinking: String::new(),
                thinking_signature: Some(signature.to_string()),
            }));
            let idx = self.output.content.len() - 1;
            self.current = Some(idx);
            events.push(AssistantMessageEvent::ThinkingStart { content_index: idx, partial: self.snapshot() });
        }
        let idx = self.current.unwrap();
        if let Some(AssistantBlock::Thinking(t)) = self.output.content.get_mut(idx) {
            t.thinking.push_str(text);
        }
        events.push(AssistantMessageEvent::ThinkingDelta {
            content_index: idx,
            delta: text.to_string(),
            partial: self.snapshot(),
        });
    }

    fn apply_usage(&mut self, raw: &Value) {
        self.output.usage = parse_chunk_usage(raw);
        self.saw_usage = true;
        self.await_trailing_usage = !has_positive_cache_read(raw);
    }

    /// Handle one decoded `data:` JSON chunk.
    pub fn handle_chunk(
        &mut self,
        chunk: &Value,
        events: &mut Vec<AssistantMessageEvent>,
    ) -> Result<ChunkFlow, ProviderError> {
        if !chunk.is_object() {
            return Ok(ChunkFlow::Continue);
        }
        self.saw_frame = true;
        if let Some(err) = stream_error(chunk) {
            return Err(err);
        }
        if self.output.response_id.is_none() {
            self.output.response_id =
                chunk.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
        }
        if self.output.upstream_provider.is_none() {
            self.output.upstream_provider =
                chunk.get("provider").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
        }
        let top_usage = chunk.get("usage").filter(|u| u.is_object());
        if let Some(u) = top_usage {
            self.apply_usage(u);
        }
        let Some(choice) = chunk.get("choices").and_then(|c| c.get(0)) else {
            if self.finished && self.saw_usage {
                return Ok(ChunkFlow::Break);
            }
            return Ok(ChunkFlow::Continue);
        };
        if top_usage.is_none()
            && let Some(u) = choice.get("usage").filter(|u| u.is_object())
        {
            self.apply_usage(u);
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str).filter(|r| !r.is_empty()) {
            let (stop, msg) = map_stop_reason(Some(reason));
            self.output.stop_reason = stop;
            if msg.is_some() {
                self.output.error_message = msg;
            }
            self.finished = true;
        }
        if let Some(delta) = choice.get("delta") {
            for field in REASONING_FIELDS {
                if let Some(text) = delta.get(field).and_then(Value::as_str).filter(|s| !s.is_empty()) {
                    self.append_thinking(text, field, events);
                    break;
                }
            }
            let text = content_text(delta.get("content"));
            self.append_text(&text, events);
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    self.tool_call_delta(call, events);
                }
            }
        }
        if self.finished && self.saw_usage && !self.await_trailing_usage {
            return Ok(ChunkFlow::Break);
        }
        Ok(ChunkFlow::Continue)
    }

    fn tool_call_delta(&mut self, call: &Value, events: &mut Vec<AssistantMessageEvent>) {
        let stream_index = call.get("index").and_then(Value::as_u64);
        let id = call.get("id").and_then(Value::as_str).filter(|s| !s.is_empty());
        let name = call.pointer("/function/name").and_then(Value::as_str).filter(|s| !s.is_empty());
        let mut target = stream_index.and_then(|i| self.tool_by_index.get(&i).copied());
        if target.is_none()
            && let Some(id) = id
        {
            target = self
                .pending_tools
                .iter()
                .copied()
                .find(|i| matches!(self.output.content.get(*i), Some(AssistantBlock::ToolCall(c)) if c.id == id));
        }
        if target.is_none()
            && let Some(cur) = self.current.filter(|i| self.partial_args.contains_key(i))
        {
            let cur_id = match &self.output.content[cur] {
                AssistantBlock::ToolCall(c) => c.id.clone(),
                _ => String::new(),
            };
            if id.is_none() || id == Some(cur_id.as_str()) {
                target = Some(cur);
            }
        }
        let idx = match target {
            Some(idx) => {
                if self.current != Some(idx)
                    && let Some(cur) = self.current
                    && !self.is_tool(cur)
                {
                    self.finish_block(Some(cur), events);
                }
                self.current = Some(idx);
                if let Some(si) = stream_index {
                    self.tool_by_index.entry(si).or_insert(idx);
                }
                idx
            }
            None => {
                if let Some(cur) = self.current
                    && !self.is_tool(cur)
                {
                    self.finish_block(Some(cur), events);
                }
                self.output.content.push(AssistantBlock::ToolCall(ToolCall {
                    id: id.unwrap_or("").to_string(),
                    name: name.unwrap_or("").to_string(),
                    arguments: JsonObject::new(),
                    thought_signature: None,
                }));
                let idx = self.output.content.len() - 1;
                if let Some(si) = stream_index {
                    self.tool_by_index.insert(si, idx);
                }
                self.pending_tools.push(idx);
                self.partial_args.insert(idx, String::new());
                self.current = Some(idx);
                events.push(AssistantMessageEvent::ToolcallStart { content_index: idx, partial: self.snapshot() });
                idx
            }
        };
        let mut delta = String::new();
        let buffer = self.partial_args.entry(idx).or_default();
        match call.pointer("/function/arguments") {
            Some(Value::String(s)) if !s.is_empty() => {
                buffer.push_str(s);
                delta = s.clone();
            }
            Some(Value::Object(obj)) => {
                // MiniMax-style object arguments: merge top-level keys (deep merge not ported).
                let mut merged = parse_streaming_json(buffer);
                for (k, v) in obj {
                    merged.insert(k.clone(), v.clone());
                }
                *buffer = serde_json::to_string(&merged).unwrap_or_default();
            }
            _ => {}
        }
        // Re-parse the growing buffer only after geometric growth (OMP
        // `parseStreamingJsonThrottled`); the final strict parse happens at toolcall_end.
        let last = self.last_display_parse.get(&idx).copied().unwrap_or(0);
        let display = if last == 0 || buffer.len() >= last + (last / 2).max(256) {
            self.last_display_parse.insert(idx, buffer.len().max(1));
            Some(parse_streaming_json(buffer))
        } else {
            None
        };
        if let Some(AssistantBlock::ToolCall(tc)) = self.output.content.get_mut(idx) {
            if let Some(id) = id {
                tc.id = id.to_string();
            }
            if let Some(name) = name {
                tc.name = name.to_string();
            }
            if let Some(display) = display {
                tc.arguments = display;
            }
        }
        self.first_token.get_or_insert_with(Instant::now);
        events.push(AssistantMessageEvent::ToolcallDelta { content_index: idx, delta, partial: self.snapshot() });
    }

    /// End-of-stream finalization. `Err` for premature close or error finish.
    pub fn finish(&mut self, events: &mut Vec<AssistantMessageEvent>) -> Result<(), ProviderError> {
        if !self.finished && !self.saw_done && !self.output.content.is_empty() {
            return Err(ProviderError::Incomplete(INCOMPLETE_STREAM_MESSAGE.into()));
        }
        // ARA difference: a 2xx body with no finish, no `[DONE]` and no content
        // (empty or non-SSE body) is an error, not a successful empty turn.
        // Upstream relies on its empty-completion retry wrapper instead.
        if !self.finished && !self.saw_done && self.output.content.is_empty() && !self.saw_frame {
            return Err(ProviderError::Incomplete(EMPTY_STREAM_MESSAGE.into()));
        }
        self.close_open_blocks(events);
        if self.output.stop_reason == StopReason::Stop && self.output.tool_calls().next().is_some() {
            self.output.stop_reason = StopReason::ToolUse;
        }
        if self.output.stop_reason == StopReason::Error {
            let msg =
                self.output.error_message.clone().unwrap_or_else(|| "Provider returned an error stop reason".into());
            return Err(ProviderError::Stream(msg));
        }
        self.output.error_message = None;
        Ok(())
    }
}

// ----------------------------------------------------------------- transport

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    if let Some(ms) =
        headers.get("retry-after-ms").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<f64>().ok())
    {
        return Some(Duration::from_millis(ms.max(0.0) as u64));
    }
    let value = headers.get("retry-after").and_then(|v| v.to_str().ok())?.trim();
    if let Ok(seconds) = value.parse::<f64>() {
        return Some(Duration::from_millis((seconds.max(0.0) * 1000.0) as u64));
    }
    httpdate::parse_http_date(value)
        .ok()
        .map(|when| when.duration_since(std::time::SystemTime::now()).unwrap_or(Duration::ZERO))
}

async fn sleep_or_cancel(delay: Duration, cancel: &CancellationToken) -> Result<(), ProviderError> {
    tokio::select! {
        _ = tokio::time::sleep(delay) => Ok(()),
        _ = cancel.cancelled() => Err(ProviderError::Aborted),
    }
}

/// POST with bounded retries before any stream byte is consumed
/// (OMP `fetchWithRetry`: 408/429/5xx and network errors, Retry-After aware).
async fn post_with_retry(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
    body: &Value,
    policy: &RetryPolicy,
    cancel: &CancellationToken,
) -> Result<reqwest::Response, ProviderError> {
    let bytes = serde_json::to_vec(body).map_err(|e| ProviderError::Config(e.to_string()))?;
    let mut attempt: u32 = 0;
    loop {
        let mut req = client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .body(bytes.clone());
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let result = tokio::select! {
            r = req.send() => r,
            _ = cancel.cancelled() => return Err(ProviderError::Aborted),
        };
        let last = attempt + 1 >= policy.max_attempts;
        let default_delay = policy.base_delay.saturating_mul(2u32.saturating_pow(attempt)).min(policy.max_delay);
        match result {
            Ok(resp) if resp.status().is_success() => return Ok(resp),
            Ok(resp) => {
                let status = resp.status().as_u16();
                let hint = retry_after(resp.headers());
                let retryable = matches!(status, 408 | 429 | 500..=599);
                let body = tokio::select! {
                    b = resp.text() => b.unwrap_or_default(),
                    _ = cancel.cancelled() => return Err(ProviderError::Aborted),
                };
                let admission_reject = body.contains("\"rate_limit_type\"") && body.contains("max_parallel_requests");
                let hint_too_long = hint.is_some_and(|h| h > policy.max_delay);
                if !retryable || last || admission_reject || hint_too_long {
                    return Err(ProviderError::Http { status, detail: parse_error_envelope(&body) });
                }
                sleep_or_cancel(hint.unwrap_or(default_delay), cancel).await?;
            }
            Err(err) => {
                if err.is_builder() {
                    return Err(ProviderError::Config(format!("invalid request: {err}")));
                }
                if last {
                    return Err(ProviderError::Transport(format!("request failed: {err}")));
                }
                sleep_or_cancel(default_delay, cancel).await?;
            }
        }
        attempt += 1;
    }
}

/// Start a streamed completion. Always yields exactly one terminal event.
pub fn stream(client: reqwest::Client, model: Model, context: Context, options: StreamOptions) -> AssistantStream {
    let (sink, rx) = EventSink::channel();
    tokio::spawn(async move {
        let start = Instant::now();
        let mut state = ChunkState::new(&model);
        let result = run(&client, &model, &context, &options, &mut state, &sink).await;
        let mut output = state.output.clone();
        output.duration = Some(start.elapsed().as_millis() as u64);
        output.ttft = state.first_token.map(|t| t.duration_since(start).as_millis() as u64);
        match result {
            Ok(()) => {
                let reason = output.stop_reason;
                sink.push(AssistantMessageEvent::Done { reason, message: output }).await;
            }
            Err(err) => {
                let mut events = Vec::new();
                state.close_open_blocks(&mut events);
                for e in events {
                    sink.push(e).await;
                }
                let mut output = state.output.clone();
                output.duration = Some(start.elapsed().as_millis() as u64);
                output.ttft = state.first_token.map(|t| t.duration_since(start).as_millis() as u64);
                output.stop_reason = err.stop_reason();
                output.error_status = err.status();
                output.error_message = Some(err.to_string());
                let reason = output.stop_reason;
                sink.push(AssistantMessageEvent::Error { reason, error: output }).await;
            }
        }
    });
    rx
}

async fn run(
    client: &reqwest::Client,
    model: &Model,
    context: &Context,
    options: &StreamOptions,
    state: &mut ChunkState,
    sink: &EventSink,
) -> Result<(), ProviderError> {
    let cancel = options.cancel.clone();
    if cancel.is_cancelled() {
        return Err(ProviderError::Aborted);
    }
    let base = model.base_url.trim_end_matches('/');
    if base.is_empty() {
        return Err(ProviderError::Config("OpenAI request setup did not resolve a base URL".into()));
    }
    let url = format!("{base}/chat/completions");
    let mut headers: Vec<(String, String)> = Vec::new();
    if let Some(key) = options.api_key.as_deref().filter(|k| !k.is_empty()) {
        headers.push(("Authorization".into(), format!("Bearer {key}")));
    }
    headers.extend(options.extra_headers.iter().cloned());
    let params = build_params(model, context, options);

    let started = Instant::now();
    let first_deadline = options.first_event_timeout.map(|d| started + d);
    let response = match first_deadline {
        Some(deadline) => tokio::select! {
            r = post_with_retry(client, &url, &headers, &params, &options.retry, &cancel) => r?,
            _ = tokio::time::sleep_until(deadline.into()) => return Err(ProviderError::Timeout(FIRST_EVENT_TIMEOUT_MESSAGE.into())),
        },
        None => post_with_retry(client, &url, &headers, &params, &options.retry, &cancel).await?,
    };
    if !sink.push_or_cancel(AssistantMessageEvent::Start { partial: state.output.clone() }, &cancel).await {
        return Err(ProviderError::Aborted);
    }

    let mut body = response.bytes_stream();
    let mut decoder = SseDecoder::new();
    let mut progressed = false;
    let mut last_progress = Instant::now();
    let mut finished_at: Option<Instant> = None;
    'outer: loop {
        let deadline = if let Some(f) = finished_at {
            Some(f + POST_FINISH_GRACE)
        } else if !progressed {
            first_deadline
        } else {
            options.idle_timeout.map(|d| last_progress + d)
        };
        let next = match deadline {
            Some(deadline) => tokio::select! {
                n = body.next() => n,
                _ = cancel.cancelled() => return Err(ProviderError::Aborted),
                _ = tokio::time::sleep_until(deadline.into()) => {
                    if finished_at.is_some() { break 'outer; }
                    let msg = if progressed { IDLE_TIMEOUT_MESSAGE } else { FIRST_EVENT_TIMEOUT_MESSAGE };
                    return Err(ProviderError::Timeout(msg.into()));
                }
            },
            None => tokio::select! {
                n = body.next() => n,
                _ = cancel.cancelled() => return Err(ProviderError::Aborted),
            },
        };
        let ended = next.is_none();
        let frames = match next {
            None => decoder.finish().into_iter().collect::<Vec<_>>(),
            Some(Ok(bytes)) => decoder.feed(&bytes),
            Some(Err(_)) if state.finished => break 'outer,
            Some(Err(err)) => return Err(ProviderError::Transport(format!("stream read failed: {err}"))),
        };
        for frame in frames {
            if frame.data == "[DONE]" {
                state.saw_done = true;
                break 'outer;
            }
            let chunk = serde_json::from_str::<Value>(&frame.data).map_err(|e| {
                let preview: String = frame.data.chars().take(200).collect();
                ProviderError::Stream(format!("Malformed OpenAI completions stream frame ({e}): {preview}"))
            })?;
            if is_progress_chunk(&chunk) {
                progressed = true;
                last_progress = Instant::now();
            }
            let mut events = Vec::new();
            let flow = state.handle_chunk(&chunk, &mut events);
            for e in events {
                if !sink.push_or_cancel(e, &cancel).await {
                    return Err(ProviderError::Aborted);
                }
            }
            if flow? == ChunkFlow::Break {
                break 'outer;
            }
            if state.finished && finished_at.is_none() {
                finished_at = Some(Instant::now());
            }
        }
        if ended {
            break;
        }
    }
    if cancel.is_cancelled() {
        return Err(ProviderError::Aborted);
    }
    let mut events = Vec::new();
    let finish = state.finish(&mut events);
    for e in events {
        sink.push(e).await;
    }
    finish
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ToolResultMessage, UserMessage};

    fn model() -> Model {
        Model {
            id: "m".into(),
            api: API.into(),
            provider: "test".into(),
            base_url: "http://x".into(),
            reasoning: false,
            max_tokens: None,
        }
    }

    fn run_chunks(chunks: &[Value], done: bool) -> (ChunkState, Vec<AssistantMessageEvent>, Result<(), ProviderError>) {
        let mut state = ChunkState::new(&model());
        let mut events = Vec::new();
        for c in chunks {
            match state.handle_chunk(c, &mut events) {
                Ok(ChunkFlow::Break) => break,
                Ok(ChunkFlow::Continue) => {}
                Err(e) => return (state, events, Err(e)),
            }
        }
        state.saw_done = done;
        let r = state.finish(&mut events);
        (state, events, r)
    }

    fn names(events: &[AssistantMessageEvent]) -> Vec<&'static str> {
        events.iter().map(|e| e.type_name()).collect()
    }

    #[test]
    fn text_reasoning_and_usage() {
        let chunks = vec![
            json!({"id": "r1", "provider": "Up", "choices": [{"delta": {"reasoning_content": "think"}}]}),
            json!({"id": "r1", "choices": [{"delta": {"content": "Hel"}}]}),
            json!({"id": "r1", "choices": [{"delta": {"content": "lo"}, "finish_reason": "stop"}]}),
            json!({"id": "r1", "choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 5, "prompt_tokens_details": {"cached_tokens": 4}}}),
        ];
        let (state, events, r) = run_chunks(&chunks, false);
        r.unwrap();
        assert_eq!(
            names(&events),
            vec![
                "thinking_start",
                "thinking_delta",
                "thinking_end",
                "text_start",
                "text_delta",
                "text_delta",
                "text_end"
            ]
        );
        let out = &state.output;
        assert_eq!(out.text(), "Hello");
        assert_eq!(out.response_id.as_deref(), Some("r1"));
        assert_eq!(out.upstream_provider.as_deref(), Some("Up"));
        assert_eq!(out.stop_reason, StopReason::Stop);
        assert_eq!(out.usage.input, Some(6));
        assert_eq!(out.usage.cache_read, Some(4));
        assert_eq!(out.usage.total_tokens, Some(15));
        match &out.content[0] {
            AssistantBlock::Thinking(t) => assert_eq!(t.thinking_signature.as_deref(), Some("reasoning_content")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn indexed_tool_calls_and_stop_promotion() {
        let chunks = vec![
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1", "function": {"name": "read", "arguments": "{\"pa"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 1, "id": "c2", "function": {"name": "bash", "arguments": "{}"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "th\":\"a\"}"}}]}}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ];
        let (state, events, r) = run_chunks(&chunks, true);
        r.unwrap();
        assert_eq!(state.output.stop_reason, StopReason::ToolUse);
        let calls: Vec<_> = state.output.tool_calls().collect();
        assert_eq!(calls.len(), 2);
        assert_eq!(Value::Object(calls[0].arguments.clone()), json!({"path": "a"}));
        assert_eq!(calls[1].name, "bash");
        assert_eq!(names(&events).iter().filter(|n| **n == "toolcall_end").count(), 2);
        assert!(state.output.usage.is_unknown());
    }

    #[test]
    fn premature_close_is_incomplete_but_done_sentinel_completes() {
        let chunks = vec![json!({"choices": [{"delta": {"content": "partial"}}]})];
        let (_, _, r) = run_chunks(&chunks, false);
        assert_eq!(r.unwrap_err(), ProviderError::Incomplete(INCOMPLETE_STREAM_MESSAGE.into()));
        let (state, _, r) = run_chunks(&chunks, true);
        r.unwrap();
        assert_eq!(state.output.stop_reason, StopReason::Stop);
    }

    #[test]
    fn error_envelopes_and_finish_reasons() {
        let (_, _, r) = run_chunks(&[json!({"error": {"message": "overloaded", "code": 503}})], false);
        assert_eq!(r.unwrap_err(), ProviderError::Http { status: 503, detail: "overloaded".into() });
        let (_, _, r) = run_chunks(&[json!({"error": {"message": "rl", "type": "too_many_requests"}})], false);
        assert_eq!(r.unwrap_err().status(), Some(429));
        let (_, _, r) =
            run_chunks(&[json!({"choices": [{"delta": {"content": "x"}, "finish_reason": "content_filter"}]})], false);
        assert_eq!(r.unwrap_err(), ProviderError::Stream("Provider finish_reason: content_filter".into()));
        assert_eq!(map_stop_reason(Some("MAX_TOKENS")).0, StopReason::Length);
        assert_eq!(map_stop_reason(None).0, StopReason::Stop);
        assert_eq!(map_stop_reason(Some("weird")).1.as_deref(), Some("Provider finish_reason: weird"));
    }

    #[test]
    fn invalid_final_arguments_are_marked() {
        let chunks = vec![
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1", "function": {"name": "write", "arguments": "{\"a\":"}}]}}]}),
            json!({"choices": [{"finish_reason": "tool_calls", "delta": {}}]}),
        ];
        let (state, _, r) = run_chunks(&chunks, true);
        r.unwrap();
        assert!(state.output.tool_calls().next().unwrap().arguments.contains_key("__parseError"));
    }

    #[test]
    fn empty_finish_reason_is_not_a_finish() {
        let chunks = vec![
            json!({"choices": [{"delta": {"content": "a"}, "finish_reason": ""}]}),
            json!({"choices": [{"delta": {"content": "b"}, "finish_reason": null}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ];
        let (state, _, r) = run_chunks(&chunks, true);
        r.unwrap();
        assert_eq!(state.output.text(), "ab");
    }

    #[test]
    fn unreported_usage_buckets_stay_unknown() {
        let u = parse_chunk_usage(&json!({"prompt_tokens": 12, "completion_tokens": 3}));
        assert_eq!((u.input, u.output, u.total_tokens), (Some(12), Some(3), Some(15)));
        assert_eq!((u.cache_read, u.cache_write), (None, None));
        let u = parse_chunk_usage(&json!({"completion_tokens": 3}));
        assert_eq!((u.input, u.output, u.total_tokens), (None, Some(3), None));
        assert!(parse_chunk_usage(&json!({})).is_unknown());
    }

    #[test]
    fn stream_without_any_frame_is_an_error() {
        let (_, _, r) = run_chunks(&[], false);
        assert_eq!(r.unwrap_err(), ProviderError::Incomplete(EMPTY_STREAM_MESSAGE.into()));
        let (state, _, r) = run_chunks(&[], true);
        r.unwrap();
        assert!(state.output.content.is_empty(), "[DONE] alone is an agreed empty completion");
    }

    #[test]
    fn keepalive_chunks_are_not_progress() {
        assert!(!is_progress_chunk(&json!({"choices": [{"delta": {"content": ""}}]})));
        assert!(!is_progress_chunk(&json!({"choices": []})));
        assert!(is_progress_chunk(&json!({"choices": [{"delta": {"content": "a"}}]})));
        assert!(is_progress_chunk(&json!({"usage": {"prompt_tokens": 1}})));
    }

    #[test]
    fn request_shape() {
        let mut assistant = AssistantMessage::empty(API, "test", "m");
        assistant.stop_reason = StopReason::ToolUse;
        assistant.content = vec![
            AssistantBlock::Thinking(ThinkingContent { thinking: "hidden".into(), thinking_signature: None }),
            AssistantBlock::ToolCall(ToolCall {
                id: "c1".into(),
                name: "read".into(),
                arguments: serde_json::from_value(json!({"path": "a"})).unwrap(),
                thought_signature: None,
            }),
        ];
        let context = Context {
            system_prompt: vec!["sys one".into(), "sys two".into()],
            messages: vec![
                Message::User(UserMessage::text("hi")),
                Message::Assistant(assistant),
                Message::ToolResult(ToolResultMessage {
                    tool_call_id: "c1".into(),
                    tool_name: "read".into(),
                    content: vec![UserBlock::text("body")],
                    details: None,
                    is_error: false,
                    timestamp: 2,
                }),
            ],
            tools: Some(vec![Tool {
                name: "read".into(),
                description: "Read".into(),
                parameters: json!({"type": "object"}),
            }]),
        };
        let opts = StreamOptions {
            tool_choice: Some(ToolChoice::Tool("missing".into())),
            max_tokens: Some(64),
            ..Default::default()
        };
        let params = build_params(&model(), &context, &opts);
        assert_eq!(params["stream"], json!(true));
        assert_eq!(params["stream_options"], json!({"include_usage": true}));
        assert_eq!(params["max_tokens"], json!(64));
        assert!(params.get("tool_choice").is_none(), "forced choice for an absent tool is dropped");
        let msgs = params["messages"].as_array().unwrap();
        assert_eq!(msgs[0], json!({"role": "system", "content": "sys one"}));
        assert_eq!(msgs[1], json!({"role": "system", "content": "sys two"}));
        assert_eq!(msgs[2], json!({"role": "user", "content": "hi"}));
        assert_eq!(
            msgs[3],
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"a\"}"}}]})
        );
        assert_eq!(msgs[4], json!({"role": "tool", "content": "body", "tool_call_id": "c1"}));
        assert_eq!(params["tools"][0]["function"]["name"], json!("read"));
    }

    #[test]
    fn tools_sentinel_when_unspecified_with_tool_history() {
        let mut assistant = AssistantMessage::empty(API, "test", "m");
        assistant.content.push(AssistantBlock::ToolCall(ToolCall {
            id: "c".into(),
            name: "x".into(),
            arguments: JsonObject::new(),
            thought_signature: None,
        }));
        let context = Context { system_prompt: vec![], messages: vec![Message::Assistant(assistant)], tools: None };
        let params = build_params(&model(), &context, &StreamOptions::default());
        assert_eq!(params["tools"], json!([]));
        let context = Context { tools: Some(vec![]), ..context };
        assert!(build_params(&model(), &context, &StreamOptions::default()).get("tools").is_none());
    }
}
