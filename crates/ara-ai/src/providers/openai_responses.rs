//! OpenAI-compatible Responses request encoding and transient replay state.
//!
//! Source: pinned OMP `packages/ai/src/providers/openai-responses.ts`
//! (`buildParams`, `convertTools`) and `openai-shared.ts`
//! (`buildResponsesInput`, `appendResponsesToolResultMessages`) at
//! 596f2da7101178214aa27a753529d15e6b7ad91d.
//!
//! Stream decoding and host selection remain separate. Native reasoning replay
//! is limited to same-model history after the provider session has warmed.

use crate::error::ProviderError;
use crate::event::{AssistantMessageEvent, AssistantStream, EventSink};
use crate::responses_sse::ResponsesSseDecoder;
use crate::responses_stream::{ResponsesStreamState, responses_endpoint_fingerprint};
use crate::transform::{ToolCallOriginScope, responses_call_component, transform_messages};
use crate::types::{
    AssistantBlock, AssistantMessage, Context, Message, Model, StopReason, ToolChoice, UserBlock, UserContent,
};
use futures::StreamExt;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use super::openai_completions::{RetryPolicy, post_with_retry};

pub const API: &str = "openai-responses";
const NON_VISION_IMAGE_PLACEHOLDER: &str = "[image omitted: model does not support vision]";

/// Request fields supported by the stateless Responses path.
#[derive(Clone, Debug, Default)]
pub struct RequestOptions {
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub tool_choice: Option<ToolChoice>,
    /// Host-confirmed input capability; unknown defaults to text only.
    pub supports_images: bool,
    /// `None` preserves stateless direct-call replay; a hosted cold session sets `false`.
    pub native_history_replay: Option<bool>,
}

/// Host-owned, transient replay warmup for one Responses session.
#[derive(Debug, Default)]
pub struct ProviderSessionState {
    warmed: Mutex<HashMap<String, bool>>,
}

impl ProviderSessionState {
    fn is_warmed(&self, provider: &str) -> bool {
        self.warmed.lock().unwrap_or_else(|error| error.into_inner()).get(provider).copied().unwrap_or(false)
    }

    fn warm(&self, provider: &str) {
        self.warmed.lock().unwrap_or_else(|error| error.into_inner()).insert(provider.to_owned(), true);
    }
}

fn content_parts(content: &UserContent, supports_images: bool) -> Vec<Value> {
    match content {
        UserContent::Text(text) if !text.trim().is_empty() => vec![json!({"type": "input_text", "text": text})],
        UserContent::Text(_) => Vec::new(),
        UserContent::Blocks(blocks) => {
            let mut parts = Vec::new();
            let mut omitted_images = false;
            for block in blocks {
                match block {
                    UserBlock::Text(text) if !text.text.trim().is_empty() => {
                        parts.push(json!({"type": "input_text", "text": text.text}));
                    }
                    UserBlock::Image(image) if supports_images => parts.push(json!({
                        "type": "input_image", "detail": "auto",
                        "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
                    })),
                    UserBlock::Image(_) => omitted_images = true,
                    UserBlock::Text(_) => {}
                }
            }
            if omitted_images {
                parts.push(json!({"type": "input_text", "text": NON_VISION_IMAGE_PLACEHOLDER}));
            }
            parts
        }
    }
}

fn tool_output(blocks: &[UserBlock], supports_images: bool) -> Value {
    let has_images = blocks.iter().any(|block| matches!(block, UserBlock::Image(_)));
    if has_images && supports_images {
        Value::Array(
            blocks
                .iter()
                .map(|block| match block {
                    UserBlock::Text(text) => json!({"type": "input_text", "text": text.text}),
                    UserBlock::Image(image) => json!({
                        "type": "input_image",
                        "detail": "auto",
                        "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
                    }),
                })
                .collect(),
        )
    } else {
        let mut text = blocks
            .iter()
            .filter_map(|block| match block {
                UserBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if has_images {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(NON_VISION_IMAGE_PLACEHOLDER);
        }
        Value::String(text)
    }
}

fn call_id(id: &str, source_api: &str) -> String {
    let source = if matches!(source_api, "openai-responses" | "openai-codex-responses" | "azure-openai-responses") {
        responses_call_component(id)
    } else {
        id
    };
    source
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') { ch } else { '_' })
        .take(64)
        .collect()
}

fn unique_call_id(base: String, used: &mut HashSet<String>) -> String {
    if used.insert(base.clone()) {
        return base;
    }
    let mut suffix_number = 1usize;
    loop {
        let suffix = format!("_dup{suffix_number}");
        let prefix = base.chars().take(64 - suffix.len()).collect::<String>();
        let candidate = format!("{prefix}{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix_number += 1;
    }
}

fn same_responses_origin(message: &AssistantMessage, model: &Model) -> bool {
    message.api == API
        && message.provider == model.provider
        && message.model == model.id
        && matches!(message.stop_reason, StopReason::Stop | StopReason::ToolUse | StopReason::Length)
}

fn native_history(message: &AssistantMessage, model: &Model) -> Option<Vec<Option<Value>>> {
    if !same_responses_origin(message, model)
        || !message.content.iter().any(|block| {
            matches!(block, AssistantBlock::Text(text) if !text.text.trim().is_empty())
                || matches!(block, AssistantBlock::ToolCall(_))
        })
    {
        return None;
    }
    let payload = message.provider_payload.as_ref()?;
    if payload.get("type")?.as_str()? != "openaiResponsesHistory"
        || payload.get("provider")?.as_str()? != model.provider
        || payload.get("endpointSha256")?.as_str()? != responses_endpoint_fingerprint(&model.base_url)
    {
        return None;
    }
    // Old ARA payloads omitted `dt` but were incremental. A full snapshot
    // (`dt: false`) cannot be validated against one assistant turn here.
    if !matches!(payload.get("dt"), None | Some(Value::Bool(true))) {
        return None;
    }
    let items = payload.get("items")?.as_array()?;
    if items.len() > message.content.len() || message.content.len() > 1024 {
        return None;
    }
    let mut sanitized = Vec::with_capacity(message.content.len());
    let mut native_items = items.iter().peekable();
    for block in &message.content {
        if let AssistantBlock::ToolCall(call) = block
            && call.arguments.contains_key("__parseError")
        {
            if let Some(item) = native_items.peek()
                && item.get("type").and_then(Value::as_str) == Some("function_call")
                && item.get("call_id").and_then(Value::as_str) == Some(responses_call_component(&call.id))
                && item.get("name").and_then(Value::as_str) == Some(call.name.as_str())
            {
                // A done item can contain invalid arguments; an open partial
                // call has no done item at all. Both leave this block out of
                // native replay without shifting the following valid items.
                if item.get("arguments").and_then(Value::as_str)
                    != call.arguments.get("__rawJson").and_then(Value::as_str)
                {
                    return None;
                }
                native_items.next();
            }
            sanitized.push(None);
            continue;
        }
        let item = native_items.next()?;
        let wire = match (item.get("type")?.as_str()?, block) {
            ("reasoning", AssistantBlock::Thinking(thinking)) => {
                if serde_json::from_str::<Value>(thinking.thinking_signature.as_ref()?).ok()? != *item {
                    return None;
                }
                let mut wire = json!({"type": "reasoning"});
                for field in ["summary", "content", "encrypted_content"] {
                    if let Some(value) = item.get(field) {
                        let valid = if field == "encrypted_content" {
                            value.is_string() || value.is_null()
                        } else {
                            value.is_array()
                        };
                        if !valid {
                            return None;
                        }
                        wire[field] = value.clone();
                    }
                }
                wire
            }
            ("message", AssistantBlock::Text(text)) => {
                let parts = item.get("content")?.as_array()?;
                let native_text = parts
                    .iter()
                    .filter_map(|part| part.get("text").or_else(|| part.get("refusal")).and_then(Value::as_str))
                    .collect::<String>();
                if native_text != text.text {
                    return None;
                }
                let mut wire = json!({"type": "message", "role": "assistant", "content": parts});
                if let Some(phase) = item.get("phase").and_then(Value::as_str) {
                    wire["phase"] = json!(phase);
                }
                wire
            }
            ("function_call", AssistantBlock::ToolCall(call)) => {
                let id = item.get("call_id")?.as_str()?;
                let args = item.get("arguments")?.as_str()?;
                if id != responses_call_component(&call.id)
                    || item.get("name")?.as_str()? != call.name
                    || serde_json::from_str::<Value>(args).ok()? != json!(call.arguments)
                {
                    return None;
                }
                json!({"type": "function_call", "call_id": id, "name": call.name, "arguments": args})
            }
            _ => return None,
        };
        sanitized.push(Some(wire));
    }
    if native_items.next().is_some()
        || !sanitized.iter().zip(&message.content).any(|(item, block)| {
            item.is_some()
                && (matches!(block, AssistantBlock::Text(text) if !text.text.trim().is_empty())
                    || matches!(block, AssistantBlock::ToolCall(_)))
        })
    {
        return None;
    }
    Some(sanitized)
}

enum ReplayedCall {
    Function(String),
    Filtered(String),
}

fn orphan_tool_result(call_id: &str, output: Value) -> Value {
    let text = match output {
        Value::String(text) => text,
        other => other.to_string(),
    };
    let mut chars = text.chars();
    let bounded = chars.by_ref().take(16_000).collect::<String>();
    let suffix = if chars.next().is_some() { "\n...[truncated]" } else { "" };
    json!({
        "type": "message", "role": "assistant",
        "content": format!("[Orphan tool result; call_id={call_id}]: {bounded}{suffix}"),
    })
}

// Pinned OMP `sanitizeSchemaForOpenAIResponses` and
// `findStrictToolSchemaViolation`, for the JSON Schema / generic Responses
// route. Only schema-valued positions are traversed: `enum`, `default`, and
// `const` contain example data, not child schemas.
const SCHEMA_MAP_KEYS: &[&str] =
    &["properties", "patternProperties", "dependencies", "dependentSchemas", "$defs", "definitions"];
const SCHEMA_ARRAY_KEYS: &[&str] = &["anyOf", "oneOf", "allOf", "prefixItems"];
const SCHEMA_VALUE_KEYS: &[&str] = &[
    "items",
    "additionalItems",
    "contains",
    "contentSchema",
    "propertyNames",
    "if",
    "then",
    "else",
    "not",
    "additionalProperties",
    "unevaluatedItems",
    "unevaluatedProperties",
];

fn unsupported_regex_lookaround(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    for (index, pair) in bytes.windows(2).enumerate() {
        if pair != b"(?" {
            continue;
        }
        let escapes = bytes[..index].iter().rev().take_while(|byte| **byte == b'\\').count();
        if escapes % 2 != 0 {
            continue;
        }
        let suffix = &bytes[index + 2..];
        if suffix.starts_with(b"=")
            || suffix.starts_with(b"!")
            || suffix.starts_with(b"<=")
            || suffix.starts_with(b"<!")
        {
            return true;
        }
    }
    false
}

fn merge_fallback_pattern(existing: Value, next: Value) -> Value {
    match existing {
        Value::Object(mut object) if object.len() == 1 && object.get("anyOf").is_some_and(Value::is_array) => {
            object.get_mut("anyOf").and_then(Value::as_array_mut).expect("checked array").push(next);
            Value::Object(object)
        }
        existing => json!({"anyOf": [existing, next]}),
    }
}

fn sanitize_responses_schema(value: &Value, depth: usize) -> Result<Value, ()> {
    if depth > 128 {
        return Err(());
    }
    let Some(object) = value.as_object() else { return Ok(value.clone()) };
    if object.is_empty() {
        return Ok(Value::Bool(true));
    }
    let mut output = serde_json::Map::new();
    let mut one_of = None;
    for (key, child) in object {
        if key == "oneOf" && child.is_array() {
            one_of = Some(child);
            continue;
        }
        if key == "pattern" && child.as_str().is_some_and(unsupported_regex_lookaround) {
            continue;
        }
        let normalized = if SCHEMA_MAP_KEYS.contains(&key.as_str()) {
            if let Some(map) = child.as_object() {
                let mut normalized_map = serde_json::Map::new();
                for (name, schema) in map {
                    let schema = sanitize_responses_schema(schema, depth + 1)?;
                    let name = if key == "patternProperties" && unsupported_regex_lookaround(name) {
                        ".*"
                    } else {
                        name.as_str()
                    };
                    if name == ".*" && key == "patternProperties" {
                        let next = match normalized_map.remove(name) {
                            Some(existing) => merge_fallback_pattern(existing, schema),
                            None => schema,
                        };
                        normalized_map.insert(name.to_owned(), next);
                    } else {
                        normalized_map.insert(name.to_owned(), schema);
                    }
                }
                Value::Object(normalized_map)
            } else {
                child.clone()
            }
        } else if SCHEMA_ARRAY_KEYS.contains(&key.as_str()) {
            if let Some(items) = child.as_array() {
                Value::Array(
                    items.iter().map(|item| sanitize_responses_schema(item, depth + 1)).collect::<Result<_, _>>()?,
                )
            } else {
                child.clone()
            }
        } else if SCHEMA_VALUE_KEYS.contains(&key.as_str()) {
            sanitize_responses_schema(child, depth + 1)?
        } else {
            child.clone()
        };
        output.insert(key.clone(), normalized);
    }
    if let Some(variants) = one_of.and_then(Value::as_array) {
        let converted =
            variants.iter().map(|item| sanitize_responses_schema(item, depth + 1)).collect::<Result<Vec<_>, _>>()?;
        match output.get_mut("anyOf") {
            Some(Value::Array(existing)) => existing.extend(converted),
            _ => {
                output.insert("anyOf".into(), Value::Array(converted));
            }
        }
    }
    let has_object_type = object.get("type").is_some_and(|kind| {
        kind == "object" || kind.as_array().is_some_and(|kinds| kinds.iter().any(|kind| kind == "object"))
    });
    if has_object_type && !object.contains_key("properties") {
        output.insert("properties".into(), json!({}));
    }
    if output.is_empty() {
        return Ok(Value::Bool(true));
    }
    Ok(Value::Object(output))
}

fn json_value_matches_type(value: &Value, kind: &str) -> bool {
    match kind {
        "null" => value.is_null(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_f64().is_some_and(|number| number.is_finite() && number.fract() == 0.0),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true,
    }
}

fn incompatible_schema_path(value: &Value, path: &str, depth: usize) -> Option<String> {
    if depth > 128 {
        return Some(path.to_owned());
    }
    let object = value.as_object()?;
    let types = match object.get("type") {
        Some(Value::String(kind)) => vec![kind.as_str()],
        Some(Value::Array(kinds)) => kinds.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let known = types
        .into_iter()
        .filter(|kind| matches!(*kind, "null" | "string" | "number" | "integer" | "boolean" | "array" | "object"))
        .collect::<Vec<_>>();
    if !known.is_empty() {
        if object.get("enum").and_then(Value::as_array).is_some_and(|values| {
            values.iter().any(|value| !known.iter().any(|kind| json_value_matches_type(value, kind)))
        }) {
            return Some(format!("{path}/enum"));
        }
        if object.get("const").is_some_and(|value| !known.iter().any(|kind| json_value_matches_type(value, kind))) {
            return Some(format!("{path}/const"));
        }
    }
    for key in SCHEMA_MAP_KEYS {
        if let Some(map) = object.get(*key).and_then(Value::as_object) {
            for (name, child) in map {
                if let Some(found) = incompatible_schema_path(child, &format!("{path}/{key}/{name}"), depth + 1) {
                    return Some(found);
                }
            }
        }
    }
    for key in SCHEMA_VALUE_KEYS {
        if let Some(child) = object.get(*key)
            && let Some(found) = incompatible_schema_path(child, &format!("{path}/{key}"), depth + 1)
        {
            return Some(found);
        }
    }
    for key in SCHEMA_ARRAY_KEYS {
        if let Some(items) = object.get(*key).and_then(Value::as_array) {
            for (index, child) in items.iter().enumerate() {
                if let Some(found) = incompatible_schema_path(child, &format!("{path}/{key}/{index}"), depth + 1) {
                    return Some(found);
                }
            }
        }
    }
    None
}

fn report_quarantined_tool(name: &str, reason: &str) {
    let name = name.chars().take(120).collect::<String>();
    let reason = reason.chars().take(256).collect::<String>();
    eprintln!("ara: Responses tool {} omitted: {}", json!(name), json!(reason));
}

/// Build the outbound `/responses` body. No request or model state is retained.
pub fn build_request(model: &Model, context: &Context, options: &RequestOptions) -> Result<Value, ProviderError> {
    if model.api != API {
        return Err(ProviderError::Config(format!("Responses encoder requires {API} model API")));
    }
    if options.temperature.is_some_and(|value| !value.is_finite()) {
        return Err(ProviderError::Config("temperature must be finite".into()));
    }

    let mut input = Vec::new();
    let transformed = transform_messages(&context.messages);
    let scope = ToolCallOriginScope::collect(&transformed);
    let mut remapped: HashMap<String, VecDeque<ReplayedCall>> = HashMap::new();
    let mut used_call_ids = HashSet::new();
    for message in &transformed {
        match message {
            Message::User(user) => {
                let content = content_parts(&user.content, options.supports_images);
                if !content.is_empty() {
                    input.push(json!({"role": "user", "content": content}));
                }
            }
            Message::Developer(developer) => {
                let content = content_parts(&developer.content, options.supports_images);
                if !content.is_empty() {
                    input.push(json!({"role": "user", "content": content}));
                }
            }
            Message::Assistant(assistant) => {
                if options.native_history_replay.unwrap_or(true)
                    && let Some(items) = native_history(assistant, model)
                {
                    for (item, block) in items.into_iter().zip(&assistant.content) {
                        if let AssistantBlock::ToolCall(call) = block {
                            let base = call_id(&call.id, &assistant.api);
                            let replayed = if item.is_some() {
                                ReplayedCall::Function(unique_call_id(base, &mut used_call_ids))
                            } else {
                                ReplayedCall::Filtered(base)
                            };
                            remapped.entry(scope.pairing_key(&call.id).to_owned()).or_default().push_back(replayed);
                        }
                        if let Some(mut item) = item {
                            if let AssistantBlock::ToolCall(call) = block
                                && let Some(ReplayedCall::Function(wire_id)) =
                                    remapped.get(scope.pairing_key(&call.id)).and_then(VecDeque::back)
                            {
                                item["call_id"] = json!(wire_id);
                            }
                            input.push(item);
                        }
                    }
                    continue;
                }
                for block in &assistant.content {
                    match block {
                        AssistantBlock::Text(text) if !text.text.trim().is_empty() => input.push(json!({
                            "type": "message", "role": "assistant", "status": "completed",
                            "content": [{"type": "output_text", "text": text.text, "annotations": []}],
                        })),
                        AssistantBlock::ToolCall(call) => {
                            let base = call_id(&call.id, &assistant.api);
                            if base.is_empty() {
                                return Err(ProviderError::Config("tool call has no usable Responses call ID".into()));
                            }
                            let wire_id = unique_call_id(base, &mut used_call_ids);
                            remapped
                                .entry(scope.pairing_key(&call.id).to_owned())
                                .or_default()
                                .push_back(ReplayedCall::Function(wire_id.clone()));
                            input.push(json!({
                                "type": "function_call", "call_id": wire_id,
                                "name": call.name,
                                "arguments": serde_json::to_string(&call.arguments).map_err(|e| ProviderError::Config(e.to_string()))?,
                            }));
                        }
                        _ => {}
                    }
                }
            }
            Message::ToolResult(result) => {
                let replayed = remapped
                    .get_mut(scope.pairing_key(&result.tool_call_id))
                    .and_then(VecDeque::pop_front)
                    .ok_or_else(|| {
                        ProviderError::Config("tool result has no matching Responses function call".into())
                    })?;
                let output = tool_output(&result.content, options.supports_images);
                input.push(match replayed {
                    ReplayedCall::Function(wire_id) => json!({
                        "type": "function_call_output", "call_id": wire_id, "output": output,
                    }),
                    ReplayedCall::Filtered(wire_id) => orphan_tool_result(&wire_id, output),
                });
            }
        }
    }

    let mut body = json!({"model": model.id, "input": input, "stream": true, "store": false});
    if model.reasoning {
        body["include"] = json!(["reasoning.encrypted_content"]);
    }
    let instructions =
        context.system_prompt.iter().filter(|prompt| !prompt.trim().is_empty()).cloned().collect::<Vec<_>>();
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions.join("\n\n"));
    }
    if let Some(max) = options.max_tokens.or(model.max_tokens) {
        body["max_output_tokens"] = json!(max);
    }
    if let Some(temperature) = options.temperature {
        body["temperature"] = json!(temperature);
    }

    let tools = context.tools.as_ref().map(|tools| {
        tools
            .iter()
            .filter_map(|tool| {
                let upgraded = match crate::schema_draft::upgrade_json_schema(&tool.parameters, 0) {
                    Ok(schema) => schema,
                    Err(()) => {
                        report_quarantined_tool(&tool.name, "schema nesting exceeds upgrade limit");
                        return None;
                    }
                };
                let parameters = match sanitize_responses_schema(&upgraded, 0) {
                    Ok(parameters) => parameters,
                    Err(()) => {
                        report_quarantined_tool(&tool.name, "schema nesting exceeds 128 levels");
                        return None;
                    }
                };
                if let Some(path) = incompatible_schema_path(&parameters, "#", 0) {
                    report_quarantined_tool(
                        &tool.name,
                        &format!("enum or const conflicts with declared type at {path}"),
                    );
                    return None;
                }
                Some(json!({
                    "type": "function", "name": tool.name, "description": tool.description,
                    "parameters": parameters,
                }))
            })
            .collect::<Vec<_>>()
    });
    if let Some(tools) = tools.as_ref().filter(|tools| !tools.is_empty()) {
        body["tools"] = json!(tools);
    }
    if let Some(choice) = &options.tool_choice
        && tools.as_ref().is_some_and(|tools| !tools.is_empty())
    {
        let wire = match choice {
            ToolChoice::Auto => Some(json!("auto")),
            ToolChoice::None => Some(json!("none")),
            ToolChoice::Required => Some(json!("required")),
            ToolChoice::Tool(name)
                if tools.as_ref().is_some_and(|tools| tools.iter().any(|tool| tool["name"] == *name)) =>
            {
                Some(json!({"type": "function", "name": name}))
            }
            ToolChoice::Tool(_) => None,
        };
        if let Some(wire) = wire {
            body["tool_choice"] = wire;
        }
    }
    Ok(body)
}

#[derive(Clone, Debug)]
pub struct StreamOptions {
    pub request: RequestOptions,
    pub api_key: Option<String>,
    pub cancel: CancellationToken,
    pub first_event_timeout: Option<Duration>,
    pub idle_timeout: Option<Duration>,
    pub extra_headers: Vec<(String, String)>,
    pub retry: RetryPolicy,
    /// Shared only by calls in the same host session; never persisted.
    pub session_state: Option<Arc<ProviderSessionState>>,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            request: RequestOptions::default(),
            api_key: None,
            cancel: CancellationToken::new(),
            first_event_timeout: Some(Duration::from_secs(300)),
            idle_timeout: Some(Duration::from_secs(300)),
            extra_headers: Vec::new(),
            retry: RetryPolicy::default(),
            session_state: None,
        }
    }
}

/// Stream one stateless Responses request through the shared Agent event port.
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
    let (sink, events) = EventSink::channel();
    let error = Arc::new(Mutex::new(None));
    let recorded_error = error.clone();
    tokio::spawn(async move {
        let start = Instant::now();
        let mut state = ResponsesStreamState::new(&model);
        let mut retry_blocked = false;
        let result = run(&client, &model, &context, &options, &mut state, &sink, &mut retry_blocked).await;
        let mut output = state.output.clone();
        output.duration = Some(start.elapsed().as_millis() as u64);
        match result {
            Ok(()) => {
                if let Some(session_state) = &options.session_state
                    && native_history(&output, &model).is_some()
                {
                    session_state.warm(&model.provider);
                }
                let reason = output.stop_reason;
                let _ = sink.push(AssistantMessageEvent::Done { reason, message: output }).await;
            }
            Err(cause) => {
                *recorded_error.lock().expect("retry side channel") = Some(crate::replay_safe_retry::AttemptError {
                    cause: cause.clone(),
                    retry_blocked: retry_blocked || state.replay_unsafe_wire_event,
                });
                output.stop_reason = cause.stop_reason();
                output.error_status = cause.status();
                output.error_message = Some(cause.to_string());
                let reason = output.stop_reason;
                let _ = sink.push(AssistantMessageEvent::Error { reason, error: output }).await;
            }
        }
    });
    crate::replay_safe_retry::AttemptStream { events, error }
}

async fn run(
    client: &reqwest::Client,
    model: &Model,
    context: &Context,
    options: &StreamOptions,
    state: &mut ResponsesStreamState,
    sink: &EventSink,
    retry_blocked: &mut bool,
) -> Result<(), ProviderError> {
    if options.cancel.is_cancelled() {
        return Err(ProviderError::Aborted);
    }
    let base = model.base_url.trim_end_matches('/');
    if base.is_empty() {
        return Err(ProviderError::Config("Responses request has no base URL".into()));
    }
    let url = format!("{base}/responses");
    let mut request = options.request.clone();
    if let Some(session_state) = &options.session_state {
        request.native_history_replay = Some(session_state.is_warmed(&model.provider));
    }
    let body = build_request(model, context, &request)?;
    let mut headers = Vec::new();
    if let Some(key) = options.api_key.as_deref().filter(|key| !key.is_empty()) {
        headers.push(("Authorization".to_owned(), format!("Bearer {key}")));
    }
    headers.extend(options.extra_headers.iter().cloned());
    let started = Instant::now();
    let first_deadline = options.first_event_timeout.map(|duration| started + duration);
    let response = match first_deadline {
        Some(deadline) => tokio::select! {
            result = post_with_retry(client, &url, &headers, &body, &options.retry, &options.cancel, retry_blocked) => result?,
            _ = tokio::time::sleep_until(deadline.into()) => return Err(ProviderError::Timeout("Responses stream timed out before its first event".into())),
        },
        None => post_with_retry(client, &url, &headers, &body, &options.retry, &options.cancel, retry_blocked).await?,
    };
    if !sink.push_or_cancel(AssistantMessageEvent::Start { partial: state.output.clone() }, &options.cancel).await {
        return Err(ProviderError::Aborted);
    }
    let mut chunks = response.bytes_stream();
    let mut decoder = ResponsesSseDecoder::new();
    let mut progressed = false;
    let mut last_progress = Instant::now();
    loop {
        let deadline =
            if progressed { options.idle_timeout.map(|duration| last_progress + duration) } else { first_deadline };
        let next = match deadline {
            Some(deadline) => tokio::select! {
                next = chunks.next() => next,
                _ = options.cancel.cancelled() => return Err(ProviderError::Aborted),
                _ = tokio::time::sleep_until(deadline.into()) => return Err(ProviderError::Timeout("Responses stream stalled".into())),
            },
            None => tokio::select! {
                next = chunks.next() => next,
                _ = options.cancel.cancelled() => return Err(ProviderError::Aborted),
            },
        };
        let ended = next.is_none();
        let frames = match next {
            Some(Ok(bytes)) => decoder.feed(&bytes)?,
            Some(Err(error)) => return Err(ProviderError::Transport(format!("Responses stream read failed: {error}"))),
            None => decoder.finish()?.into_iter().collect(),
        };
        for frame in frames {
            if frame.data == "[DONE]" {
                return Err(ProviderError::Incomplete(
                    "Responses stream ended without a response terminal event".into(),
                ));
            }
            let event: Value = serde_json::from_str(&frame.data)
                .map_err(|_| ProviderError::Stream("Malformed Responses SSE JSON frame".into()))?;
            let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
            if matches!(
                event_type,
                "response.created"
                    | "response.output_item.added"
                    | "response.reasoning_summary_part.added"
                    | "response.reasoning_summary_text.delta"
                    | "response.reasoning_summary_text.done"
                    | "response.reasoning_summary_part.done"
                    | "response.reasoning_text.delta"
                    | "response.content_part.added"
                    | "response.output_text.delta"
                    | "response.refusal.delta"
                    | "response.function_call_arguments.delta"
                    | "response.function_call_arguments.done"
                    | "response.output_item.done"
                    | "response.completed"
                    | "response.incomplete"
                    | "response.failed"
                    | "response.done"
                    | "error"
            ) {
                progressed = true;
                last_progress = Instant::now();
            }
            let updates = state.handle(&event)?;
            for update in updates {
                if !sink.push_or_cancel(update, &options.cancel).await {
                    return Err(ProviderError::Aborted);
                }
            }
            if state.terminal {
                return Ok(());
            }
        }
        if ended {
            break;
        }
    }
    Err(ProviderError::Incomplete("Responses stream closed without a response terminal event".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AssistantMessage, Tool, ToolCall, ToolResultMessage, UserMessage, now_ms};

    fn model() -> Model {
        Model {
            id: "example-model".into(),
            api: API.into(),
            provider: "example".into(),
            base_url: "https://example.invalid/v1".into(),
            reasoning: false,
            max_tokens: None,
            tokenizer: None,
        }
    }

    fn call(id: &str) -> AssistantBlock {
        AssistantBlock::ToolCall(ToolCall {
            id: id.into(),
            name: "read".into(),
            arguments: serde_json::from_value(json!({"path": "a.txt"})).unwrap(),
            thought_signature: None,
        })
    }

    fn result(id: &str) -> Message {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: id.into(),
            tool_name: "read".into(),
            content: vec![UserBlock::text("file contents")],
            details: None,
            is_error: false,
            timestamp: now_ms(),
        })
    }

    #[test]
    fn canonical_stateless_request_preserves_roles_and_function_schema() {
        let context = Context {
            system_prompt: vec!["one".into(), "two".into()],
            messages: vec![Message::User(UserMessage::text("hello"))],
            tools: Some(vec![Tool {
                name: "read".into(),
                description: "Read a file".into(),
                parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            }]),
        };
        let body = build_request(
            &model(),
            &context,
            &RequestOptions {
                max_tokens: Some(100),
                temperature: Some(0.25),
                tool_choice: Some(ToolChoice::Tool("read".into())),
                supports_images: false,
                native_history_replay: None,
            },
        )
        .unwrap();
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert_eq!(body["instructions"], "one\n\ntwo");
        assert_eq!(body["max_output_tokens"], 100);
        assert_eq!(body["temperature"], 0.25);
        assert_eq!(body["input"], json!([{"role": "user", "content": [{"type": "input_text", "text": "hello"}]}]));
        assert_eq!(body["tools"][0]["name"], "read");
        assert_eq!(body["tools"][0]["parameters"]["properties"]["path"]["type"], "string");
        assert_eq!(body["tool_choice"], json!({"type": "function", "name": "read"}));
    }

    #[test]
    fn responses_schema_normalization_visits_only_schema_positions() {
        let schema = json!({
            "type":"object",
            "properties": {
                "oneOf": {"oneOf":[{"type":"object"},{"type":"string"}],
                    "anyOf":[{"type":"null"}]},
                "free": {},
                "patternOnly": {"pattern":"^(?!bad$)"},
                "literal": {"enum":[{"oneOf":[{}],"pattern":"(?=literal)"}],
                    "default":{"properties":{"x":{}}},
                    "examples":[{"oneOf":[{}]}]},
                "nullable": {"type":["object","null"]}
            },
            "patternProperties": {
                "(?=unsafe)": {"type":"number"},
                ".*": {"type":"string"},
                "\\(?=literal)": {"type":"boolean"}
            },
            "additionalProperties": false
        });
        let converted = sanitize_responses_schema(&schema, 0).unwrap();
        assert_eq!(converted["properties"]["free"], true);
        assert_eq!(converted["properties"]["patternOnly"], true);
        assert_eq!(converted["properties"]["oneOf"]["anyOf"].as_array().unwrap().len(), 3);
        assert!(converted["properties"]["oneOf"].get("oneOf").is_none());
        assert_eq!(converted["properties"]["oneOf"]["anyOf"][1]["properties"], json!({}));
        assert_eq!(converted["properties"]["nullable"]["properties"], json!({}));
        assert_eq!(converted["properties"]["literal"], schema["properties"]["literal"]);
        assert_eq!(converted["additionalProperties"], false);
        assert_eq!(converted["patternProperties"][".*"]["anyOf"].as_array().unwrap().len(), 2);
        assert_eq!(converted["patternProperties"]["\\(?=literal)"], json!({"type":"boolean"}));

        let reversed = json!({"patternProperties": {
            ".*": {"type":"string"}, "(?=unsafe)": {"type":"number"}
        }});
        assert_eq!(
            sanitize_responses_schema(&reversed, 0).unwrap()["patternProperties"][".*"]["anyOf"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn quarantined_responses_tool_cannot_leave_an_invalid_forced_choice() {
        let bad = Tool {
            name: "bad".into(),
            description: "bad enum".into(),
            parameters: json!({"type":"object","properties":{
                "choice":{"type":"integer","enum":["not an integer"]}
            }}),
        };
        let good = Tool {
            name: "good".into(),
            description: "good enum".into(),
            parameters: json!({"type":"object","properties":{
                "choice":{"type":["string","null"],"enum":["yes",null]},
                "open":{"enum":["anything"]}
            }}),
        };
        let context = Context { tools: Some(vec![bad.clone(), good]), ..Context::default() };
        let forced_bad = build_request(
            &model(),
            &context,
            &RequestOptions { tool_choice: Some(ToolChoice::Tool("bad".into())), ..RequestOptions::default() },
        )
        .unwrap();
        assert_eq!(forced_bad["tools"].as_array().unwrap().len(), 1);
        assert_eq!(forced_bad["tools"][0]["name"], "good");
        assert!(forced_bad.get("tool_choice").is_none());
        let forced_good = build_request(
            &model(),
            &context,
            &RequestOptions { tool_choice: Some(ToolChoice::Tool("good".into())), ..RequestOptions::default() },
        )
        .unwrap();
        assert_eq!(forced_good["tool_choice"], json!({"type":"function","name":"good"}));
        let bad_only = Context { tools: Some(vec![bad]), ..Context::default() };
        for choice in [ToolChoice::Required, ToolChoice::Auto, ToolChoice::None, ToolChoice::Tool("bad".into())] {
            let body = build_request(
                &model(),
                &bad_only,
                &RequestOptions { tool_choice: Some(choice), ..RequestOptions::default() },
            )
            .unwrap();
            assert!(body.get("tools").is_none());
            assert!(body.get("tool_choice").is_none());
        }
        assert_eq!(incompatible_schema_path(&json!({"type":"null","const":"bad"}), "#", 0), Some("#/const".into()));
        assert_eq!(incompatible_schema_path(&json!({"type":"integer","enum":[1.0]}), "#", 0), None);
    }

    #[test]
    fn responses_composite_call_and_result_replay_use_the_same_call_id() {
        let mut assistant = AssistantMessage::empty(API, "example", "example-model");
        assistant.content = vec![AssistantBlock::text("checking"), call("call_A|fc_X")];
        let context =
            Context { messages: vec![Message::Assistant(assistant), result("call_A|fc_X")], ..Context::default() };
        let input =
            build_request(&model(), &context, &RequestOptions::default()).unwrap()["input"].as_array().unwrap().clone();
        assert_eq!(input[0]["content"][0]["text"], "checking");
        assert_eq!(
            input[1],
            json!({"type": "function_call", "call_id": "call_A", "name": "read", "arguments": "{\"path\":\"a.txt\"}"})
        );
        assert_eq!(input[2], json!({"type": "function_call_output", "call_id": "call_A", "output": "file contents"}));
    }

    #[test]
    fn native_reasoning_replay_requires_matching_origin_and_unmodified_content() {
        let endpoint = model();
        let reasoning = json!({"type":"reasoning","id":"rs_1","encrypted_content":"opaque","status":"completed"});
        let mut assistant = AssistantMessage::empty(API, "example", "example-model");
        assistant.content = vec![
            AssistantBlock::Thinking(crate::types::ThinkingContent {
                thinking: "summary".into(),
                thinking_signature: Some(reasoning.to_string()),
            }),
            AssistantBlock::text("hello"),
        ];
        assistant.provider_payload = Some(
            json!({"type":"openaiResponsesHistory","provider":"example","endpointSha256":responses_endpoint_fingerprint(&endpoint.base_url),"items":[
                reasoning, {"type":"message","phase":"commentary","content":[{"type":"output_text","text":"hello"}]}
            ]}),
        );
        let input_for = |message: AssistantMessage, endpoint: &Model| {
            build_request(
                endpoint,
                &Context { messages: vec![Message::Assistant(message)], ..Context::default() },
                &RequestOptions::default(),
            )
            .unwrap()["input"]
                .as_array()
                .unwrap()
                .clone()
        };
        let input = input_for(assistant.clone(), &endpoint);
        assert_eq!(input[0]["encrypted_content"], "opaque");
        assert!(input[0].get("status").is_none());
        assert_eq!(input[1]["phase"], "commentary");
        let mut truncated = assistant.clone();
        truncated.stop_reason = StopReason::Length;
        assert_eq!(input_for(truncated, &endpoint)[0]["encrypted_content"], "opaque");
        let cold_input = build_request(
            &endpoint,
            &Context { messages: vec![Message::Assistant(assistant.clone())], ..Context::default() },
            &RequestOptions { native_history_replay: Some(false), ..RequestOptions::default() },
        )
        .unwrap()["input"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(cold_input.len(), 1);
        assert_eq!(cold_input[0]["content"][0]["text"], "hello");
        assert!(cold_input[0].get("phase").is_none());
        let mut changed = assistant.clone();
        changed.content[1] = AssistantBlock::text("edited");
        let changed_input = input_for(changed, &endpoint);
        assert!(changed_input.iter().all(|item| item["type"] != "reasoning"));
        for dt in [json!(false), json!("true")] {
            let mut unsupported_snapshot = assistant.clone();
            unsupported_snapshot.provider_payload.as_mut().unwrap()["dt"] = dt;
            assert!(input_for(unsupported_snapshot, &endpoint).iter().all(|item| item["type"] != "reasoning"));
        }
        let mut failed = assistant.clone();
        failed.stop_reason = StopReason::Error;
        assert!(input_for(failed, &endpoint).iter().all(|item| item["type"] != "reasoning"));
        let mut foreign = endpoint.clone();
        foreign.provider = "other".into();
        assert!(input_for(assistant.clone(), &foreign).iter().all(|item| item["type"] != "reasoning"));
        let mut moved = endpoint.clone();
        moved.base_url = "https://different.invalid/v1".into();
        assert!(input_for(assistant.clone(), &moved).iter().all(|item| item["type"] != "reasoning"));
        let mut malformed = assistant.clone();
        malformed.provider_payload.as_mut().unwrap()["items"][0]["encrypted_content"] = json!({"bad":"shape"});
        assert!(input_for(malformed, &endpoint).iter().all(|item| item["type"] != "reasoning"));
        let mut tampered = assistant;
        if let AssistantBlock::Thinking(thinking) = &mut tampered.content[0] {
            thinking.thinking_signature = Some("{\"type\":\"reasoning\",\"encrypted_content\":\"different\"}".into());
        }
        assert!(input_for(tampered, &endpoint).iter().all(|item| item["type"] != "reasoning"));
        let mut reasoning_model = endpoint;
        reasoning_model.reasoning = true;
        let body = build_request(&reasoning_model, &Context::default(), &RequestOptions::default()).unwrap();
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    }

    #[test]
    fn transient_replay_warmup_is_provider_scoped() {
        let state = ProviderSessionState::default();
        assert!(!state.is_warmed("one"));
        assert!(!state.is_warmed("two"));
        state.warm("one");
        assert!(state.is_warmed("one"));
        assert!(!state.is_warmed("two"));
        assert!(!ProviderSessionState::default().is_warmed("one"));
    }

    #[test]
    fn partial_function_arguments_cannot_become_native_history() {
        let endpoint = model();
        let mut assistant = AssistantMessage::empty(API, "example", "example-model");
        assistant.stop_reason = StopReason::Length;
        let mut partial = call("call_bad|fc_bad");
        if let AssistantBlock::ToolCall(call) = &mut partial {
            call.arguments =
                serde_json::from_value(json!({"__rawJson":"{\"path\":", "__parseError":"incomplete"})).unwrap();
        }
        assistant.content = vec![partial];
        assistant.provider_payload = Some(json!({
            "type":"openaiResponsesHistory", "provider":"example", "dt":true,
            "endpointSha256":responses_endpoint_fingerprint(&endpoint.base_url),
            "items":[{"type":"function_call","call_id":"call_bad","name":"read",
                "arguments":"{\"__parseError\":\"incomplete\",\"__rawJson\":\"{\\\"path\\\":\"}"}]
        }));
        assert!(native_history(&assistant, &endpoint).is_none());
    }

    fn mixed_partial_message(include_done_item: bool) -> AssistantMessage {
        let endpoint = model();
        let reasoning = json!({"type":"reasoning","encrypted_content":"opaque"});
        let mut assistant = AssistantMessage::empty(API, "example", "example-model");
        assistant.stop_reason = StopReason::Length;
        let mut partial = call("call_bad|fc_bad");
        if let AssistantBlock::ToolCall(call) = &mut partial {
            call.arguments = serde_json::from_value(json!({
                "__rawJson":"{\"path\":", "__parseError":"incomplete"
            }))
            .unwrap();
        }
        assistant.content = vec![
            AssistantBlock::Thinking(crate::types::ThinkingContent {
                thinking: "summary".into(),
                thinking_signature: Some(reasoning.to_string()),
            }),
            AssistantBlock::text("visible answer"),
            partial,
        ];
        let mut items =
            vec![reasoning, json!({"type":"message","content":[{"type":"output_text","text":"visible answer"}]})];
        if include_done_item {
            items.push(json!({"type":"function_call","call_id":"call_bad","name":"read",
                "arguments":"{\"path\":"}));
        }
        assistant.provider_payload = Some(json!({
            "type":"openaiResponsesHistory", "provider":"example", "dt":true,
            "endpointSha256":responses_endpoint_fingerprint(&endpoint.base_url), "items":items
        }));
        assistant
    }

    #[test]
    fn mixed_partial_call_keeps_native_text_and_reasoning_but_repairs_its_result() {
        for include_done_item in [false, true] {
            let assistant = mixed_partial_message(include_done_item);
            let context = Context {
                messages: vec![Message::Assistant(assistant.clone()), result("call_bad|fc_bad")],
                ..Context::default()
            };
            let input = build_request(&model(), &context, &RequestOptions::default()).unwrap()["input"]
                .as_array()
                .unwrap()
                .clone();
            assert_eq!(input.len(), 3);
            assert_eq!(input[0]["type"], "reasoning");
            assert_eq!(input[0]["encrypted_content"], "opaque");
            assert_eq!(input[1]["content"][0]["text"], "visible answer");
            assert_eq!(input[2]["type"], "message");
            assert_eq!(input[2]["role"], "assistant");
            assert_eq!(input[2]["content"], "[Orphan tool result; call_id=call_bad]: file contents");
            assert!(input.iter().all(|item| item["type"] != "function_call" && item["type"] != "function_call_output"));
            assert!(native_history(&assistant, &model()).is_some());

            let cold = build_request(
                &model(),
                &context,
                &RequestOptions { native_history_replay: Some(false), ..RequestOptions::default() },
            )
            .unwrap();
            assert!(cold["input"].as_array().unwrap().iter().all(|item| item["type"] != "reasoning"));
        }
    }

    #[test]
    fn mixed_partial_call_does_not_displace_valid_sibling_or_accept_tampered_items() {
        let mut assistant = mixed_partial_message(false);
        assistant.content.push(call("call_ok|fc_ok"));
        assistant.provider_payload.as_mut().unwrap()["items"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"function_call","call_id":"call_ok","name":"read",
                "arguments":"{\"path\":\"a.txt\"}"}));
        let context = Context {
            messages: vec![Message::Assistant(assistant.clone()), result("call_bad|fc_bad"), result("call_ok|fc_ok")],
            ..Context::default()
        };
        let input =
            build_request(&model(), &context, &RequestOptions::default()).unwrap()["input"].as_array().unwrap().clone();
        assert_eq!(input.len(), 5);
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "call_ok");
        assert_eq!(input[3]["type"], "message");
        assert_eq!(input[4], json!({"type":"function_call_output","call_id":"call_ok","output":"file contents"}));

        let mut changed_text = assistant.clone();
        changed_text.content[1] = AssistantBlock::text("changed");
        assert!(native_history(&changed_text, &model()).is_none());
        let mut wrong_raw = assistant.clone();
        wrong_raw.provider_payload.as_mut().unwrap()["items"].as_array_mut().unwrap().insert(
            2,
            json!({"type":"function_call","call_id":"call_bad","name":"read",
                "arguments":"different"}),
        );
        assert!(native_history(&wrong_raw, &model()).is_none());
        let mut wrong_order = assistant;
        wrong_order.provider_payload.as_mut().unwrap()["items"].as_array_mut().unwrap().swap(1, 2);
        assert!(native_history(&wrong_order, &model()).is_none());
    }

    #[test]
    fn foreign_opaque_ids_and_colliding_wire_ids_keep_results_paired() {
        let mut assistant = AssistantMessage::empty("openai-completions", "other", "example-model");
        assistant.content = vec![call("a|b"), call("a_b")];
        let context = Context {
            messages: vec![Message::Assistant(assistant), result("a|b"), result("a_b")],
            ..Context::default()
        };
        let input =
            build_request(&model(), &context, &RequestOptions::default()).unwrap()["input"].as_array().unwrap().clone();
        assert_eq!(input[0]["call_id"], "a_b");
        assert_eq!(input[1]["call_id"], "a_b_dup1");
        assert_eq!(input[2]["call_id"], "a_b");
        assert_eq!(input[3]["call_id"], "a_b_dup1");
    }

    #[test]
    fn unavailable_tool_choice_is_omitted_like_pinned_omp() {
        for choice in [ToolChoice::Auto, ToolChoice::None, ToolChoice::Required, ToolChoice::Tool("missing".into())] {
            let body = build_request(
                &model(),
                &Context::default(),
                &RequestOptions { tool_choice: Some(choice), ..RequestOptions::default() },
            )
            .unwrap();
            assert!(body.get("tool_choice").is_none());
        }
    }

    #[test]
    fn stored_developer_turn_uses_generic_responses_user_role() {
        let context = Context {
            messages: vec![Message::Developer(crate::types::DeveloperMessage {
                content: UserContent::Text("developer note".into()),
                timestamp: now_ms(),
            })],
            ..Context::default()
        };
        let body = build_request(&model(), &context, &RequestOptions::default()).unwrap();
        assert_eq!(
            body["input"][0],
            json!({"role": "user", "content": [{"type": "input_text", "text": "developer note"}]})
        );
    }

    #[test]
    fn images_are_native_only_with_confirmed_model_capability() {
        let image = UserBlock::Image(crate::types::ImageContent { data: "AQI=".into(), mime_type: "image/png".into() });
        let context = Context {
            messages: vec![Message::User(UserMessage {
                content: UserContent::Blocks(vec![UserBlock::text("look"), image.clone()]),
                synthetic: None,
                timestamp: now_ms(),
            })],
            ..Context::default()
        };
        let text_only = build_request(&model(), &context, &RequestOptions::default()).unwrap();
        assert_eq!(
            text_only["input"][0]["content"][1],
            json!({"type": "input_text", "text": NON_VISION_IMAGE_PLACEHOLDER})
        );
        let vision =
            build_request(&model(), &context, &RequestOptions { supports_images: true, ..RequestOptions::default() })
                .unwrap();
        assert_eq!(
            vision["input"][0]["content"][1],
            json!({
                "type": "input_image", "detail": "auto", "image_url": "data:image/png;base64,AQI="
            })
        );
        let mut tool_result = result("call_A");
        let Message::ToolResult(ref mut value) = tool_result else { unreachable!() };
        value.content.push(image);
        let mut assistant = AssistantMessage::empty(API, "example", "example-model");
        assistant.content.push(call("call_A|fc_X"));
        let tool_context = Context { messages: vec![Message::Assistant(assistant), tool_result], ..Context::default() };
        let text_only = build_request(&model(), &tool_context, &RequestOptions::default()).unwrap();
        assert_eq!(text_only["input"][1]["output"], format!("file contents\n{NON_VISION_IMAGE_PLACEHOLDER}"));
        let vision = build_request(
            &model(),
            &tool_context,
            &RequestOptions { supports_images: true, ..RequestOptions::default() },
        )
        .unwrap();
        assert_eq!(
            vision["input"][1]["output"][1],
            json!({
                "type": "input_image", "detail": "auto", "image_url": "data:image/png;base64,AQI="
            })
        );
    }
}
