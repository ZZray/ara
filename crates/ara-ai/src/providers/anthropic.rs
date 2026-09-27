//! Anthropic Messages API-key path (WIP).
//!
//! Fixed OMP source: packages/ai/src/providers/anthropic.ts at
//! 596f2da7101178214aa27a753529d15e6b7ad91d, especially
//! buildParams, convertAnthropicMessages and the message SSE loop.
//! OAuth, provider-specific betas and caching remain open in AI-ANTHROPIC.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::error::ProviderError;
use crate::event::{AssistantMessageEvent as Event, AssistantStream, EventSink};
use crate::providers::openai_completions::{RetryPolicy, post_with_retry};
use crate::sse::SseDecoder;
use crate::transform::transform_messages;
use crate::types::{
    AssistantBlock, AssistantMessage, Context, Message, Model, StopReason, TextContent, ThinkingContent, Tool,
    ToolCall, ToolChoice, Usage, UserBlock, UserContent,
};

pub const API: &str = "anthropic-messages";
// The CLI does not yet resolve OMP catalogue limits for every model.
const DEFAULT_MAX_TOKENS: u64 = 4096;
const MAX_TOOL_JSON_BYTES: usize = 1024 * 1024;
const MAX_TOOL_SCHEMA_DEPTH: usize = 128;

#[derive(Clone, Debug)]
pub struct StreamOptions {
    pub api_key: Option<String>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub tool_choice: Option<ToolChoice>,
    pub cancel: CancellationToken,
    pub first_event_timeout: Option<Duration>,
    pub idle_timeout: Option<Duration>,
    pub extra_headers: Vec<(String, String)>,
    pub retry: RetryPolicy,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            api_key: None,
            max_tokens: None,
            temperature: None,
            tool_choice: None,
            cancel: CancellationToken::new(),
            first_event_timeout: Some(Duration::from_secs(300)),
            idle_timeout: Some(Duration::from_secs(300)),
            extra_headers: Vec::new(),
            retry: RetryPolicy::default(),
        }
    }
}

fn user_content(content: &UserContent) -> Value {
    match content {
        UserContent::Text(text) => Value::String(text.clone()),
        UserContent::Blocks(blocks) => Value::Array(
            blocks
                .iter()
                .map(|block| match block {
                    UserBlock::Text(text) => json!({"type":"text","text":text.text}),
                    UserBlock::Image(image) => json!({
                        "type":"image",
                        "source":{"type":"base64","media_type":image.mime_type,"data":image.data}
                    }),
                })
                .collect(),
        ),
    }
}

fn result_content(blocks: &[UserBlock]) -> Value {
    Value::Array(
        blocks
            .iter()
            .map(|block| match block {
                UserBlock::Text(text) => json!({"type":"text","text":text.text}),
                UserBlock::Image(image) => json!({
                    "type":"image",
                    "source":{"type":"base64","media_type":image.mime_type,"data":image.data}
                }),
            })
            .collect(),
    )
}

fn assistant_content(message: &AssistantMessage, model: &Model) -> Vec<Value> {
    let same_origin = message.api == API && message.provider == model.provider && message.model == model.id;
    let mut blocks: Vec<Value> = message
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantBlock::Text(text) if !text.text.trim().is_empty() => Some(json!({"type":"text","text":text.text})),
            AssistantBlock::Thinking(thinking) if !thinking.thinking.trim().is_empty() => {
                if same_origin && let Some(signature) = thinking.thinking_signature.as_deref().filter(|s| !s.is_empty())
                {
                    Some(json!({"type":"thinking","thinking":thinking.thinking,"signature":signature}))
                } else {
                    Some(json!({"type":"text","text":format!("<thinking>\n{}\n</thinking>", thinking.thinking)}))
                }
            }
            AssistantBlock::RedactedThinking { data } if same_origin => {
                Some(json!({"type":"redacted_thinking","data":data}))
            }
            AssistantBlock::ToolCall(call) if !call.id.is_empty() && !call.name.is_empty() => {
                Some(json!({"type":"tool_use","id":call.id,"name":call.name,"input":call.arguments}))
            }
            _ => None,
        })
        .collect();
    // Fixed OMP keeps tool_use at the tail of an assistant turn for replay.
    if let Some(first_tool) = blocks.iter().position(|block| block["type"] == "tool_use")
        && blocks[first_tool..].iter().any(|block| block["type"] != "tool_use")
    {
        let mut tools = Vec::new();
        blocks.retain(|block| {
            if block["type"] == "tool_use" {
                tools.push(block.clone());
                false
            } else {
                true
            }
        });
        blocks.extend(tools);
    }
    blocks
}

/// Current API-key path retains the fixed OMP user/assistant/tool-result shape.
pub fn convert_messages(model: &Model, context: &Context) -> Vec<Value> {
    let messages = transform_messages(&context.messages);
    let mut wire = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        match &messages[i] {
            Message::User(user) => {
                let content = user_content(&user.content);
                if content != "" && content != json!([]) {
                    wire.push(json!({"role":"user","content":content}));
                }
            }
            Message::Developer(developer) => {
                // Without OMP's mid-conversation-system beta, developer text
                // is a user turn. Top-level context.system_prompt stays system.
                let content = user_content(&developer.content);
                if content != "" && content != json!([]) {
                    wire.push(json!({"role":"user","content":content}));
                }
            }
            Message::Assistant(assistant) => {
                let content = assistant_content(assistant, model);
                if !content.is_empty() {
                    if wire.last().is_some_and(|message| message["role"] == "assistant") {
                        wire.push(json!({"role":"user","content":"Continue."}));
                    }
                    wire.push(json!({"role":"assistant","content":content}));
                }
            }
            Message::ToolResult(_) => {
                let mut results = Vec::new();
                let mut hoisted_images = Vec::new();
                while let Some(Message::ToolResult(result)) = messages.get(i) {
                    let content = if result.is_error {
                        let text_blocks: Vec<UserBlock> = result
                            .content
                            .iter()
                            .filter_map(|block| match block {
                                UserBlock::Text(_) => Some(block.clone()),
                                UserBlock::Image(_) => {
                                    hoisted_images.push(block.clone());
                                    None
                                }
                            })
                            .collect();
                        if text_blocks.is_empty() {
                            Value::String("Tool failed with no output.".into())
                        } else {
                            result_content(&text_blocks)
                        }
                    } else {
                        result_content(&result.content)
                    };
                    results.push(json!({
                        "type":"tool_result",
                        "tool_use_id":result.tool_call_id,
                        "content":content,
                        "is_error":result.is_error
                    }));
                    i += 1;
                }
                if !hoisted_images.is_empty() {
                    results.push(json!({"type":"text","text":"Attached image(s) from the tool result(s) above:"}));
                    let Value::Array(images) = result_content(&hoisted_images) else { unreachable!() };
                    results.extend(images);
                }
                wire.push(json!({"role":"user","content":results}));
                continue;
            }
        }
        i += 1;
    }
    if wire.last().is_some_and(|message| message["role"] == "assistant") {
        wire.push(json!({"role":"user","content":"Continue."}));
    }
    wire
}

// Pinned OMP anthropic.ts::normalizeAnthropicToolSchema keeps only Messages
// input_schema keywords and moves unsupported constraints into descriptions.
fn normalize_tool_schema(schema: &Value, depth: usize, is_root: bool) -> Result<Value, ProviderError> {
    if depth > MAX_TOOL_SCHEMA_DEPTH {
        return Err(ProviderError::Config("Anthropic tool input schema is too deep".into()));
    }
    if let Value::Array(children) = schema {
        return Ok(Value::Array(
            children.iter().map(|child| normalize_tool_schema(child, depth + 1, false)).collect::<Result<_, _>>()?,
        ));
    }
    let Some(source) = schema.as_object() else { return Ok(schema.clone()) };
    if !is_root && source.is_empty() {
        return Ok(Value::Bool(true));
    }
    let scalar_type = source
        .get("type")
        .and_then(|kind| {
            kind.as_str().or_else(|| {
                kind.as_array().and_then(|kinds| kinds.iter().filter_map(Value::as_str).find(|kind| *kind != "null"))
            })
        })
        .or_else(|| source.get("properties").filter(|value| value.is_object()).map(|_| "object"))
        .or_else(|| {
            (source.contains_key("items") || source.get("prefixItems").is_some_and(Value::is_array)).then_some("array")
        });
    let mut result = Map::new();
    let mut spill = Vec::new();
    for (key, value) in source {
        let universal = matches!(
            key.as_str(),
            "$ref"
                | "$defs"
                | "$schema"
                | "definitions"
                | "type"
                | "anyOf"
                | "allOf"
                | "enum"
                | "const"
                | "description"
                | "title"
                | "default"
                | "nullable"
        );
        let typed = match scalar_type {
            Some("object") => matches!(key.as_str(), "properties" | "required" | "additionalProperties"),
            Some("array") => matches!(key.as_str(), "items" | "prefixItems" | "minItems"),
            Some("string") => key == "format",
            _ => false,
        };
        let root_combinator = is_root && matches!(key.as_str(), "anyOf" | "allOf" | "oneOf");
        if !root_combinator && (universal || typed) {
            result.insert(key.clone(), value.clone());
        } else {
            spill.push((key.clone(), value.clone()));
        }
    }
    if scalar_type == Some("string")
        && let Some(format) = result.get("format").and_then(Value::as_str)
        && !matches!(
            format,
            "date-time" | "time" | "date" | "duration" | "email" | "hostname" | "uri" | "ipv4" | "ipv6" | "uuid"
        )
    {
        let value = result.remove("format").expect("format exists");
        spill.push(("format".into(), value));
    }
    if scalar_type == Some("array")
        && let Some(value) = result.get("minItems")
        && value.as_f64() != Some(0.0)
        && value.as_f64() != Some(1.0)
    {
        let value = result.remove("minItems").expect("minItems exists");
        spill.push(("minItems".into(), value));
    }
    if scalar_type == Some("object") && !result.contains_key("additionalProperties") {
        result.insert("additionalProperties".into(), Value::Bool(false));
    }
    for key in ["properties", "$defs", "definitions"] {
        if let Some(Value::Object(children)) = result.get_mut(key) {
            for child in children.values_mut() {
                *child = normalize_tool_schema(child, depth + 1, false)?;
            }
        }
    }
    if let Some(value @ Value::Object(_)) = result.get_mut("additionalProperties") {
        let normalized = normalize_tool_schema(value, depth + 1, false)?;
        *value = if normalized.as_object().is_some_and(Map::is_empty) { Value::Bool(true) } else { normalized };
    }
    for key in ["items", "prefixItems", "anyOf", "allOf"] {
        if let Some(value) = result.get_mut(key) {
            match value {
                Value::Array(children) => {
                    for child in children {
                        *child = normalize_tool_schema(child, depth + 1, false)?;
                    }
                }
                Value::Object(_) if key == "items" => *value = normalize_tool_schema(value, depth + 1, false)?,
                _ => {}
            }
        }
    }
    append_schema_spill(&mut result, &spill);
    Ok(Value::Object(result))
}

fn append_schema_spill(result: &mut Map<String, Value>, spill: &[(String, Value)]) {
    if spill.is_empty() {
        return;
    }
    let entries: Vec<String> = spill.iter().map(|(key, value)| format!("{key}: {value}")).collect();
    let formatted = format!("{{{}}}", entries.join(", "));
    let existing = result.get("description").and_then(Value::as_str).unwrap_or("");
    let description = if existing.is_empty() { formatted } else { format!("{existing}\n\n{formatted}") };
    result.insert("description".into(), Value::String(description));
}

fn tool_wire(tool: &Tool) -> Result<Value, ProviderError> {
    if !tool.parameters.is_object() {
        return Err(ProviderError::Config(format!("tool {} input schema must be an object", tool.name)));
    }
    let mut schema = crate::schema_draft::upgrade_json_schema(&tool.parameters, 0)
        .map_err(|_| ProviderError::Config(format!("tool {} input schema is too deep", tool.name)))?;
    let root = schema.as_object_mut().expect("checked input schema object");
    root.insert("type".into(), json!("object"));
    if !root.get("properties").is_some_and(Value::is_object) {
        root.insert("properties".into(), json!({}));
    }
    let required = root
        .get("required")
        .and_then(Value::as_array)
        .map(|entries| entries.iter().filter(|entry| entry.is_string()).cloned().collect())
        .unwrap_or_default();
    root.insert("required".into(), Value::Array(required));
    let input_schema = normalize_tool_schema(&schema, 0, true)?;
    Ok(json!({"name":tool.name,"description":tool.description,"input_schema":input_schema}))
}

pub fn build_params(model: &Model, context: &Context, options: &StreamOptions) -> Result<Value, ProviderError> {
    let ceiling = model.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
    let max_tokens = options.max_tokens.unwrap_or(ceiling).min(ceiling);
    if max_tokens == 0 {
        return Err(ProviderError::Config("Anthropic max_tokens must be positive".into()));
    }
    let mut params = json!({
        "model":model.id,
        "messages":convert_messages(model, context),
        "max_tokens":max_tokens,
        "stream":true
    });
    if !context.system_prompt.is_empty() {
        params["system"] = Value::Array(
            context
                .system_prompt
                .iter()
                .filter(|text| !text.trim().is_empty())
                .map(|text| json!({"type":"text","text":text}))
                .collect(),
        );
    }
    if let Some(tools) = &context.tools {
        params["tools"] = Value::Array(tools.iter().map(tool_wire).collect::<Result<_, _>>()?);
    }
    let available_tools = context.tools.as_deref().unwrap_or_default();
    if let Some(choice) = &options.tool_choice
        && match choice {
            ToolChoice::Required => !available_tools.is_empty(),
            ToolChoice::Tool(name) => available_tools.iter().any(|tool| tool.name == *name),
            _ => true,
        }
    {
        params["tool_choice"] = match choice {
            ToolChoice::Auto => json!({"type":"auto"}),
            ToolChoice::None => json!({"type":"none"}),
            ToolChoice::Required => json!({"type":"any"}),
            ToolChoice::Tool(name) => json!({"type":"tool","name":name}),
        };
    }
    if let Some(temperature) = options.temperature {
        params["temperature"] = json!(temperature);
    }
    Ok(params)
}

fn apply_usage(usage: &mut Usage, wire: &Value) {
    if let Some(value) = wire.get("input_tokens").and_then(Value::as_u64) {
        usage.input = Some(value);
    }
    if let Some(value) = wire.get("output_tokens").and_then(Value::as_u64) {
        usage.output = Some(value);
    }
    if let Some(value) = wire.get("cache_read_input_tokens").and_then(Value::as_u64) {
        usage.cache_read = Some(value);
    }
    if let Some(value) = wire.get("cache_creation_input_tokens").and_then(Value::as_u64) {
        usage.cache_write = Some(value);
    }
    if let (Some(input), Some(output)) = (usage.input, usage.output) {
        usage.total_tokens = Some(
            input
                .saturating_add(output)
                .saturating_add(usage.cache_read.unwrap_or(0))
                .saturating_add(usage.cache_write.unwrap_or(0)),
        );
    }
}

fn stop_reason(raw: &str) -> StopReason {
    match raw {
        "max_tokens" | "model_context_window_exceeded" => StopReason::Length,
        "tool_use" => StopReason::ToolUse,
        "refusal" | "sensitive" => StopReason::Error,
        _ => StopReason::Stop,
    }
}

enum OpenBlock {
    Text(usize),
    Thinking(usize),
    Tool { index: usize, id: String, name: String, input: Map<String, Value>, json: String },
    Ignored,
}

struct MessageState {
    output: AssistantMessage,
    started: bool,
    stop_reason_seen: bool,
    stopped: bool,
    retry_blocked: bool,
    open: HashMap<usize, OpenBlock>,
    first_token: Option<Instant>,
}

impl MessageState {
    fn new(model: &Model) -> Self {
        Self {
            output: AssistantMessage::empty(API, &model.provider, &model.id),
            started: false,
            stop_reason_seen: false,
            stopped: false,
            retry_blocked: false,
            open: HashMap::new(),
            first_token: None,
        }
    }

    fn handle(&mut self, frame: &Value, events: &mut Vec<Event>) -> Result<bool, ProviderError> {
        let kind = frame.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "ping" {
            return Ok(false);
        }
        if kind == "error" {
            let detail = frame.pointer("/error/message").and_then(Value::as_str).unwrap_or("Anthropic stream error");
            return Err(ProviderError::Stream(detail.to_owned()));
        }
        if kind == "message_start" {
            if self.started {
                return Err(ProviderError::Stream("duplicate Anthropic message_start".into()));
            }
            self.started = true;
            self.output.response_id = frame.pointer("/message/id").and_then(Value::as_str).map(str::to_owned);
            if let Some(usage) = frame.pointer("/message/usage") {
                apply_usage(&mut self.output.usage, usage);
            }
            return Ok(true);
        }
        if !self.started {
            return Err(ProviderError::Stream(format!("Anthropic {kind} before message_start")));
        }
        if self.stopped {
            return Err(ProviderError::Stream(format!("Anthropic {kind} after message_stop")));
        }
        match kind {
            "content_block_start" => {
                let index = frame
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| ProviderError::Stream("content_block_start missing index".into()))?
                    as usize;
                if self.open.contains_key(&index) {
                    return Err(ProviderError::Stream(format!("duplicate Anthropic content block {index}")));
                }
                let block = &frame["content_block"];
                let content_index = self.output.content.len();
                let open = match block["type"].as_str().unwrap_or("") {
                    "text" => {
                        let text = block["text"].as_str().unwrap_or("").to_owned();
                        self.output.content.push(AssistantBlock::Text(TextContent { text, text_signature: None }));
                        events.push(Event::TextStart { content_index, partial: self.output.clone() });
                        OpenBlock::Text(content_index)
                    }
                    "thinking" => {
                        let thinking = block["thinking"].as_str().unwrap_or("").to_owned();
                        self.output.content.push(AssistantBlock::Thinking(ThinkingContent {
                            thinking: thinking.clone(),
                            thinking_signature: None,
                        }));
                        events.push(Event::ThinkingStart { content_index, partial: self.output.clone() });
                        if !thinking.is_empty() {
                            events.push(Event::ThinkingDelta {
                                content_index,
                                delta: thinking,
                                partial: self.output.clone(),
                            });
                        }
                        OpenBlock::Thinking(content_index)
                    }
                    "redacted_thinking" => {
                        self.output.content.push(AssistantBlock::RedactedThinking {
                            data: block["data"].as_str().unwrap_or("").to_owned(),
                        });
                        OpenBlock::Ignored
                    }
                    "tool_use" => {
                        let id = block["id"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .ok_or_else(|| ProviderError::Stream("tool_use missing id".into()))?
                            .to_owned();
                        let name = block["name"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .ok_or_else(|| ProviderError::Stream("tool_use missing name".into()))?
                            .to_owned();
                        let input = block["input"].as_object().cloned().unwrap_or_default();
                        events.push(Event::ToolcallStart { content_index, partial: self.output.clone() });
                        OpenBlock::Tool { index: content_index, id, name, input, json: String::new() }
                    }
                    other => return Err(ProviderError::Stream(format!("unsupported Anthropic content block {other}"))),
                };
                self.first_token.get_or_insert_with(Instant::now);
                self.open.insert(index, open);
            }
            "content_block_delta" => {
                let index = frame
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| ProviderError::Stream("content_block_delta missing index".into()))?
                    as usize;
                let open = self
                    .open
                    .get_mut(&index)
                    .ok_or_else(|| ProviderError::Stream(format!("delta for unopened Anthropic block {index}")))?;
                match (open, frame.pointer("/delta/type").and_then(Value::as_str).unwrap_or("")) {
                    (OpenBlock::Text(content_index), "text_delta") => {
                        let delta = frame.pointer("/delta/text").and_then(Value::as_str).unwrap_or("");
                        if let AssistantBlock::Text(text) = &mut self.output.content[*content_index] {
                            text.text.push_str(delta);
                        }
                        events.push(Event::TextDelta {
                            content_index: *content_index,
                            delta: delta.into(),
                            partial: self.output.clone(),
                        });
                    }
                    (OpenBlock::Thinking(content_index), "thinking_delta") => {
                        let delta = frame.pointer("/delta/thinking").and_then(Value::as_str).unwrap_or("");
                        if let AssistantBlock::Thinking(thinking) = &mut self.output.content[*content_index] {
                            thinking.thinking.push_str(delta);
                        }
                        events.push(Event::ThinkingDelta {
                            content_index: *content_index,
                            delta: delta.into(),
                            partial: self.output.clone(),
                        });
                    }
                    (OpenBlock::Thinking(content_index), "signature_delta") => {
                        let delta = frame.pointer("/delta/signature").and_then(Value::as_str).unwrap_or("");
                        if let AssistantBlock::Thinking(thinking) = &mut self.output.content[*content_index] {
                            thinking.thinking_signature.get_or_insert_with(String::new).push_str(delta);
                        }
                    }
                    (OpenBlock::Tool { index: content_index, json, .. }, "input_json_delta") => {
                        let delta = frame.pointer("/delta/partial_json").and_then(Value::as_str).unwrap_or("");
                        if json.len().saturating_add(delta.len()) > MAX_TOOL_JSON_BYTES {
                            return Err(ProviderError::Stream("Anthropic tool arguments exceeded limit".into()));
                        }
                        json.push_str(delta);
                        events.push(Event::ToolcallDelta {
                            content_index: *content_index,
                            delta: delta.into(),
                            partial: self.output.clone(),
                        });
                    }
                    (OpenBlock::Ignored, _) => {}
                    _ => return Err(ProviderError::Stream(format!("mismatched Anthropic delta for block {index}"))),
                }
            }
            "content_block_stop" => {
                let index = frame
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| ProviderError::Stream("content_block_stop missing index".into()))?
                    as usize;
                let open = self
                    .open
                    .remove(&index)
                    .ok_or_else(|| ProviderError::Stream(format!("stop for unopened Anthropic block {index}")))?;
                match open {
                    OpenBlock::Text(content_index) => {
                        let AssistantBlock::Text(text) = &self.output.content[content_index] else { unreachable!() };
                        events.push(Event::TextEnd {
                            content_index,
                            content: text.text.clone(),
                            partial: self.output.clone(),
                        });
                    }
                    OpenBlock::Thinking(content_index) => {
                        let AssistantBlock::Thinking(thinking) = &self.output.content[content_index] else {
                            unreachable!()
                        };
                        events.push(Event::ThinkingEnd {
                            content_index,
                            content: thinking.thinking.clone(),
                            partial: self.output.clone(),
                        });
                    }
                    OpenBlock::Tool { index: content_index, id, name, input, json } => {
                        let arguments = if json.is_empty() {
                            input
                        } else {
                            serde_json::from_str::<Value>(&json)
                                .map_err(|_| ProviderError::Stream(format!("invalid Anthropic tool input for {id}")))?
                                .as_object()
                                .cloned()
                                .ok_or_else(|| {
                                    ProviderError::Stream(format!("Anthropic tool input for {id} is not an object"))
                                })?
                        };
                        let call = ToolCall { id, name, arguments, thought_signature: None };
                        self.output.content.push(AssistantBlock::ToolCall(call.clone()));
                        events.push(Event::ToolcallEnd {
                            content_index,
                            tool_call: call,
                            partial: self.output.clone(),
                        });
                    }
                    OpenBlock::Ignored => {}
                }
            }
            "message_delta" => {
                if let Some(raw) = frame.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.output.stop_reason = stop_reason(raw);
                    self.stop_reason_seen = true;
                    if self.output.stop_reason == StopReason::Error {
                        self.output.stop_details =
                            frame.pointer("/delta/stop_details").cloned().or_else(|| Some(json!({"type":raw})));
                        self.output.error_message = Some(format!("Anthropic stopped with {raw}"));
                    }
                }
                if let Some(usage) = frame.get("usage") {
                    apply_usage(&mut self.output.usage, usage);
                }
            }
            "message_stop" => {
                if !self.open.is_empty() {
                    return Err(ProviderError::Incomplete(
                        "Anthropic stream stopped with an open content block".into(),
                    ));
                }
                self.stopped = true;
            }
            other => return Err(ProviderError::Stream(format!("unsupported Anthropic event {other}"))),
        }
        Ok(kind != "ping")
    }
}

pub fn stream(client: reqwest::Client, model: Model, context: Context, options: StreamOptions) -> AssistantStream {
    let cancel = options.cancel.clone();
    crate::replay_safe_retry::with_replay_safe_stream_retry(model.clone(), cancel, false, move |cancel| {
        let mut options = options.clone();
        options.cancel = cancel;
        stream_once(client.clone(), model.clone(), context.clone(), options)
    })
}

fn stream_once(
    client: reqwest::Client,
    model: Model,
    context: Context,
    options: StreamOptions,
) -> crate::replay_safe_retry::AttemptStream {
    let (sink, rx) = EventSink::channel();
    let error = Arc::new(Mutex::new(None));
    let recorded_error = error.clone();
    tokio::spawn(async move {
        let started = Instant::now();
        let mut state = MessageState::new(&model);
        let result = run(&client, &model, &context, &options, &mut state, &sink).await;
        let mut output = state.output;
        let retry_blocked = state.retry_blocked;
        output.duration = Some(started.elapsed().as_millis() as u64);
        output.ttft = state.first_token.map(|time| time.duration_since(started).as_millis() as u64);
        let event = match result {
            Ok(()) if output.stop_reason != StopReason::Error => {
                Event::Done { reason: output.stop_reason, message: output }
            }
            result => {
                let err = match result {
                    Err(err) => err,
                    Ok(()) => ProviderError::Stream(
                        output.error_message.clone().unwrap_or_else(|| "Anthropic stopped with error".into()),
                    ),
                };
                *recorded_error.lock().unwrap() =
                    Some(crate::replay_safe_retry::AttemptError { cause: err.clone(), retry_blocked });
                output.stop_reason = err.stop_reason();
                output.error_status = err.status();
                output.error_message = Some(err.to_string());
                Event::Error { reason: output.stop_reason, error: output }
            }
        };
        let _ = sink.push(event).await;
    });
    crate::replay_safe_retry::AttemptStream { events: rx, error }
}

async fn run(
    client: &reqwest::Client,
    model: &Model,
    context: &Context,
    options: &StreamOptions,
    state: &mut MessageState,
    sink: &EventSink,
) -> Result<(), ProviderError> {
    let cancel = &options.cancel;
    if cancel.is_cancelled() {
        return Err(ProviderError::Aborted);
    }
    let base = model.base_url.trim_end_matches('/');
    if base.is_empty() {
        return Err(ProviderError::Config("Anthropic request setup did not resolve a base URL".into()));
    }
    let url = if base.ends_with("/v1") { format!("{base}/messages") } else { format!("{base}/v1/messages") };
    let key = options
        .api_key
        .as_deref()
        .filter(|key| !key.is_empty())
        .ok_or_else(|| ProviderError::Config("Anthropic API key is missing".into()))?;
    let mut headers =
        vec![("x-api-key".to_owned(), key.to_owned()), ("anthropic-version".to_owned(), "2023-06-01".to_owned())];
    headers.extend(options.extra_headers.iter().cloned());
    let params = build_params(model, context, options)?;
    let started = Instant::now();
    let first_deadline = options.first_event_timeout.map(|timeout| started + timeout);
    let response = match first_deadline {
        Some(deadline) => tokio::select! {
            result = post_with_retry(client, &url, &headers, &params, &options.retry, cancel, &mut state.retry_blocked) => result?,
            _ = tokio::time::sleep_until(deadline.into()) => return Err(ProviderError::Timeout("Anthropic stream timed out before first event".into())),
        },
        None => {
            post_with_retry(client, &url, &headers, &params, &options.retry, cancel, &mut state.retry_blocked).await?
        }
    };
    if !sink.push_or_cancel(Event::Start { partial: state.output.clone() }, cancel).await {
        return Err(ProviderError::Aborted);
    }
    let mut body = response.bytes_stream();
    let mut decoder = SseDecoder::new();
    let mut progressed = false;
    let mut last_progress = Instant::now();
    loop {
        let deadline =
            if progressed { options.idle_timeout.map(|timeout| last_progress + timeout) } else { first_deadline };
        let next = match deadline {
            Some(deadline) => tokio::select! {
                result = body.next() => result,
                _ = cancel.cancelled() => return Err(ProviderError::Aborted),
                _ = tokio::time::sleep_until(deadline.into()) => return Err(ProviderError::Timeout("Anthropic stream stalled".into())),
            },
            None => tokio::select! {
                result = body.next() => result,
                _ = cancel.cancelled() => return Err(ProviderError::Aborted),
            },
        };
        let ended = next.is_none();
        let frames = match next {
            None => decoder.finish().into_iter().collect::<Vec<_>>(),
            Some(Ok(bytes)) => decoder.feed(&bytes),
            Some(Err(err)) => return Err(ProviderError::Transport(format!("Anthropic stream read failed: {err}"))),
        };
        for frame in frames {
            let value: Value = serde_json::from_str(&frame.data)
                .map_err(|_| ProviderError::Stream("malformed Anthropic SSE JSON".into()))?;
            if let Some(name) = frame.event.as_deref()
                && name != value["type"].as_str().unwrap_or("")
            {
                return Err(ProviderError::Stream("Anthropic SSE event name did not match body type".into()));
            }
            let mut events = Vec::new();
            let is_progress = state.handle(&value, &mut events)?;
            if is_progress {
                progressed = true;
                last_progress = Instant::now();
            }
            for event in events {
                if !sink.push_or_cancel(event, cancel).await {
                    return Err(ProviderError::Aborted);
                }
            }
            if state.stopped {
                return if state.output.stop_reason == StopReason::Error {
                    Err(ProviderError::Stream(
                        state.output.error_message.clone().unwrap_or_else(|| "Anthropic stopped with error".into()),
                    ))
                } else {
                    Ok(())
                };
            }
        }
        if ended {
            break;
        }
    }
    if cancel.is_cancelled() {
        return Err(ProviderError::Aborted);
    }
    if state.stop_reason_seen && state.open.is_empty() {
        return if state.output.stop_reason == StopReason::Error {
            Err(ProviderError::Stream(
                state.output.error_message.clone().unwrap_or_else(|| "Anthropic stopped with error".into()),
            ))
        } else {
            Ok(())
        };
    }
    Err(ProviderError::Incomplete("Anthropic stream ended before message_stop".into()))
}
