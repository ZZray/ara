//! Anthropic Messages API-key path (WIP).
//!
//! Fixed OMP source: packages/ai/src/providers/anthropic.ts at
//! 596f2da7101178214aa27a753529d15e6b7ad91d, especially
//! buildParams, convertAnthropicMessages and the message SSE loop.
//! OAuth, provider-specific betas and caching remain open in AI-ANTHROPIC.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::error::ProviderError;
use crate::event::{AssistantMessageEvent as Event, AssistantStream, EventSink};
use crate::providers::openai_completions::{PostError, RetryPolicy, post_with_retry, post_with_retry_detailed};
use crate::schema_wire::postprocess_json_wire_schema;
use crate::sse::SseDecoder;
use crate::transform::transform_messages;
use crate::types::{
    AssistantBlock, AssistantMessage, Context, Message, Model, StopReason, TextContent, ThinkingContent, Tool,
    ToolCall, ToolChoice, Usage, UserBlock, UserContent,
};

pub const API: &str = "anthropic-messages";
// The CLI does not yet resolve OMP catalogue limits for every model.
const DEFAULT_MAX_TOKENS: u64 = 4096;
// Fixed OMP uses 64,000 when model.maxTokens is absent. Keep ARA's smaller
// default request, but do not silently cap an explicit request at that default.
const UNKNOWN_MODEL_MAX_TOKENS: u64 = 64_000;
const MAX_TOOL_JSON_BYTES: usize = 1024 * 1024;
// Snapshots are owned Rust values: keep the preview small when tiny SSE
// deltas would otherwise clone a growing tool argument on every event.
const MAX_TOOL_DISPLAY_BYTES: usize = 4 * 1024;
const MAX_TOOL_SCHEMA_DEPTH: usize = 128;
const MAX_STRICT_TOOLS: usize = 20;
const MAX_STRICT_OPTIONAL_PARAMETERS: usize = 24;
const MAX_STRICT_UNION_PARAMETERS: usize = 16;
const PING_PROGRESS_MAX_IDLE_MULTIPLIER: u32 = 3;

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
        UserContent::Blocks(blocks) => result_content(blocks),
    }
}

fn result_content(blocks: &[UserBlock]) -> Value {
    let mut converted = Vec::new();
    let mut saw_text = false;
    let mut saw_image = false;
    for block in blocks {
        match block {
            UserBlock::Text(text) if !text.text.trim().is_empty() => {
                saw_text = true;
                converted.push(json!({"type":"text","text":text.text}));
            }
            UserBlock::Text(_) => {}
            UserBlock::Image(image) => {
                let media_type = image.mime_type.trim().to_ascii_lowercase();
                let media_type = if media_type == "image/jpg" { "image/jpeg" } else { &media_type };
                if !matches!(media_type, "image/jpeg" | "image/png" | "image/gif" | "image/webp") {
                    converted.push(json!({"type":"text","text":format!("[unsupported image: {}]", image.mime_type)}));
                    continue;
                }
                saw_image = true;
                converted.push(json!({
                    "type":"image",
                    "source":{"type":"base64","media_type":media_type,"data":image.data}
                }));
            }
        }
    }
    if saw_image && !saw_text {
        converted.insert(0, json!({"type":"text","text":"(see attached image)"}));
    }
    Value::Array(converted)
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
                if content.as_str().is_none_or(|text| !text.trim().is_empty()) && content != json!([]) {
                    wire.push(json!({"role":"user","content":content}));
                }
            }
            Message::Developer(developer) => {
                // Without OMP's mid-conversation-system beta, developer text
                // is a user turn. Top-level context.system_prompt stays system.
                let content = user_content(&developer.content);
                if content.as_str().is_none_or(|text| !text.trim().is_empty()) && content != json!([]) {
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
                    let mut content = result_content(&result.content);
                    if result.is_error
                        && let Value::Array(blocks) = &mut content
                    {
                        blocks.retain(|block| {
                            if block["type"] == "image" {
                                hoisted_images.push(block.clone());
                                false
                            } else {
                                true
                            }
                        });
                    }
                    if content == json!([]) {
                        content = Value::String(String::new());
                    }
                    if result.is_error && content == "" {
                        content = Value::String("Tool failed with no output.".into());
                    }
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
                    results.extend(hoisted_images);
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
    let upgraded = crate::schema_draft::upgrade_json_schema(&tool.parameters, 0)
        .map_err(|_| ProviderError::Config(format!("tool {} input schema is too deep", tool.name)))?;
    let mut schema = postprocess_json_wire_schema(&upgraded, 0)
        .map_err(|_| ProviderError::Config("Anthropic tool input schema is too deep".into()))?;
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

fn official_api_key_route(model: &Model) -> bool {
    let Ok(url) = reqwest::Url::parse(&model.base_url) else { return false };
    url.scheme() == "https"
        && url.host_str().is_some_and(|host| host.eq_ignore_ascii_case("api.anthropic.com"))
        && url.port().is_none_or(|port| port == 443)
        && matches!(url.path(), "/" | "/v1" | "/v1/")
        && url.query().is_none()
        && url.fragment().is_none()
}

fn strict_incompatible_keyword(value: &Value, depth: usize) -> bool {
    if depth > MAX_TOOL_SCHEMA_DEPTH {
        return true;
    }
    match value {
        Value::Array(entries) => entries.iter().any(|entry| strict_incompatible_keyword(entry, depth + 1)),
        Value::Object(fields) => {
            ["oneOf", "allOf", "$ref", "patternProperties", "propertyNames"].iter().any(|key| fields.contains_key(*key))
                || fields.values().any(|entry| strict_incompatible_keyword(entry, depth + 1))
        }
        _ => false,
    }
}

#[derive(Clone, Copy)]
struct StrictBudget {
    optional_remaining: usize,
    union_remaining: usize,
}

fn strict_schema_node(value: &Value, budget: &mut StrictBudget, depth: usize) -> Option<Value> {
    if depth > MAX_TOOL_SCHEMA_DEPTH {
        return None;
    }
    let Value::Object(fields) = value else { return Some(value.clone()) };
    let defining = [
        "type",
        "properties",
        "additionalProperties",
        "items",
        "prefixItems",
        "enum",
        "const",
        "$ref",
        "anyOf",
        "allOf",
        "oneOf",
        "$defs",
        "definitions",
    ]
    .iter()
    .any(|key| fields.contains_key(*key));
    if !defining {
        return None;
    }
    let object_node = fields.get("type") == Some(&json!("object"))
        || fields.contains_key("properties")
        || fields.contains_key("additionalProperties");
    if object_node && fields.get("additionalProperties") != Some(&Value::Bool(false)) {
        return None;
    }
    let mut result = fields.clone();
    if fields.get("type").is_some_and(Value::is_array) || fields.get("anyOf").is_some_and(Value::is_array) {
        budget.union_remaining = budget.union_remaining.checked_sub(1)?;
    }
    if let Some(Value::Object(properties)) = fields.get("properties") {
        let original_required: HashSet<&str> =
            fields.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        let mut normalized = Map::new();
        let mut required = Vec::new();
        for (name, property) in properties {
            let mut property = strict_schema_node(property, budget, depth + 1)?;
            if original_required.contains(name.as_str()) {
                required.push(json!(name));
            } else if budget.optional_remaining > 0 {
                budget.optional_remaining -= 1;
            } else {
                let has_null = property
                    .get("type")
                    .and_then(Value::as_array)
                    .is_some_and(|types| types.iter().any(|value| value == "null"))
                    || property
                        .get("anyOf")
                        .and_then(Value::as_array)
                        .is_some_and(|types| types.iter().any(|value| value["type"] == "null"));
                if !has_null {
                    if let Some(types) = property.get_mut("type").and_then(Value::as_array_mut) {
                        types.push(json!("null"));
                    } else if let Some(types) = property.get_mut("anyOf").and_then(Value::as_array_mut) {
                        types.push(json!({"type":"null"}));
                    } else {
                        budget.union_remaining = budget.union_remaining.checked_sub(1)?;
                        property = json!({"anyOf":[property,{"type":"null"}]});
                    }
                }
                required.push(json!(name));
            }
            normalized.insert(name.clone(), property);
        }
        result.insert("properties".into(), Value::Object(normalized));
        result.insert("required".into(), Value::Array(required));
    }
    for key in ["items", "prefixItems", "anyOf", "allOf", "oneOf"] {
        if let Some(child) = result.get_mut(key) {
            *child = match child {
                Value::Array(entries) => Value::Array(
                    entries
                        .iter()
                        .map(|entry| strict_schema_node(entry, budget, depth + 1))
                        .collect::<Option<Vec<_>>>()?,
                ),
                Value::Object(_) => strict_schema_node(child, budget, depth + 1)?,
                _ => child.clone(),
            };
        }
    }
    for key in ["$defs", "definitions"] {
        if let Some(Value::Object(definitions)) = result.get_mut(key) {
            for definition in definitions.values_mut() {
                *definition = strict_schema_node(definition, budget, depth + 1)?;
            }
        }
    }
    Some(Value::Object(result))
}

fn tool_wires(tools: &[Tool], strict_enabled: bool) -> Result<Vec<Value>, ProviderError> {
    let mut result = tools.iter().map(tool_wire).collect::<Result<Vec<_>, _>>()?;
    if !strict_enabled {
        return Ok(result);
    }
    let mut budget = StrictBudget {
        optional_remaining: MAX_STRICT_OPTIONAL_PARAMETERS,
        union_remaining: MAX_STRICT_UNION_PARAMETERS,
    };
    let mut strict_count = 0;
    for (tool, wire) in tools.iter().zip(&mut result) {
        if strict_count == MAX_STRICT_TOOLS
            || !matches!(tool.name.as_str(), "bash" | "python" | "edit" | "find")
            || strict_incompatible_keyword(&tool.parameters, 0)
        {
            continue;
        }
        let mut candidate_budget = budget;
        let Some(schema) = strict_schema_node(&wire["input_schema"], &mut candidate_budget, 0) else {
            continue;
        };
        wire["input_schema"] = schema;
        wire["strict"] = Value::Bool(true);
        budget = candidate_budget;
        strict_count += 1;
    }
    Ok(result)
}

fn build_params_with_strict(
    model: &Model,
    context: &Context,
    options: &StreamOptions,
    strict_enabled: bool,
) -> Result<Value, ProviderError> {
    let ceiling = model.max_tokens.unwrap_or(UNKNOWN_MODEL_MAX_TOKENS);
    let requested = options.max_tokens.unwrap_or(model.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS));
    let max_tokens = requested.min(ceiling);
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
        params["tools"] = Value::Array(tool_wires(tools, strict_enabled)?);
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

pub fn build_params(model: &Model, context: &Context, options: &StreamOptions) -> Result<Value, ProviderError> {
    build_params_with_strict(model, context, options, official_api_key_route(model))
}

fn has_strict_tools(params: &Value) -> bool {
    params["tools"].as_array().is_some_and(|tools| tools.iter().any(|tool| tool["strict"] == true))
}

fn strict_rejection(error: &PostError) -> bool {
    let ProviderError::Http { status: 400, detail } = &error.cause else { return false };
    let detail = detail.to_ascii_lowercase();
    (error.code.as_deref().is_some_and(|code| code.eq_ignore_ascii_case("invalid_request_error"))
        && ((detail.contains("compiled grammar") && detail.contains("too large"))
            || (detail.contains("schema") && detail.contains("too complex") && detail.contains("compil"))))
        || (detail.contains("structured_outputs")
            || detail.contains("structured outputs")
            || detail.contains("structured-outputs")
            || detail.contains("structured output")
            || detail.contains("structured-output"))
            && (detail.contains("unsupported")
                || detail.contains("not supported")
                || detail.contains("not available")
                || detail.contains("not enabled")
                || detail.contains("does not support")
                || detail.contains("doesn't support"))
        || (detail.contains("custom.strict") && detail.contains("extra input") && detail.contains("not permitted"))
}

async fn post_with_strict_fallback(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
    params: &Value,
    fallback: Option<&Value>,
    options: &StreamOptions,
    retry_blocked: &mut bool,
) -> Result<reqwest::Response, ProviderError> {
    match post_with_retry_detailed(client, url, headers, params, &options.retry, &options.cancel, retry_blocked).await {
        Err(error) if fallback.is_some() && has_strict_tools(params) && strict_rejection(&error) => {
            post_with_retry(
                client,
                url,
                headers,
                fallback.expect("checked fallback"),
                &options.retry,
                &options.cancel,
                retry_blocked,
            )
            .await
        }
        other => other.map_err(|error| error.cause),
    }
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

fn sse_error_detail(frame: &Value, fallback: &str) -> String {
    let Some(message) = frame.pointer("/error/message").and_then(Value::as_str) else {
        return fallback.to_owned();
    };
    match frame.pointer("/error/type").and_then(Value::as_str) {
        Some(error_type) => format!("Anthropic stream error ({error_type}): {message}"),
        None => format!("Anthropic stream error: {message}"),
    }
}

fn request_headers(model: &Model, options: &StreamOptions, key: &str) -> Result<Vec<(String, String)>, ProviderError> {
    let url =
        reqwest::Url::parse(&model.base_url).map_err(|_| ProviderError::Config("Invalid Anthropic base URL".into()))?;
    let official_host =
        url.scheme() == "https" && url.host_str().is_some_and(|host| host.eq_ignore_ascii_case("api.anthropic.com"));
    if official_host && url.port().is_some_and(|port| port != 443) {
        // The CLI currently selects ANTHROPIC_API_KEY for this hostname even
        // with a custom port. Reject it before sending any credentials.
        return Err(ProviderError::Config("Anthropic official host with a nonstandard port is unsupported".into()));
    }
    let api_key_proxy = matches!(model.provider.as_str(), "opencode-go" | "opencode-zen" | "umans");
    let caller_header = |name: &str| {
        options
            .extra_headers
            .iter()
            .rev()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    };
    let mut headers = Vec::with_capacity(options.extra_headers.len() + 2);
    if official_host || api_key_proxy {
        headers.push(("x-api-key".into(), caller_header("x-api-key").unwrap_or(key).to_owned()));
        if official_host && let Some(value) = caller_header("authorization") {
            headers.push(("authorization".into(), value.to_owned()));
        }
    } else {
        headers.push((
            "authorization".into(),
            caller_header("authorization").map(str::to_owned).unwrap_or_else(|| format!("Bearer {key}")),
        ));
        if let Some(value) = caller_header("x-api-key") {
            headers.push(("x-api-key".into(), value.to_owned()));
        }
    }
    headers.push(("anthropic-version".into(), "2023-06-01".into()));
    headers.extend(
        options
            .extra_headers
            .iter()
            .filter(|(name, _)| !name.eq_ignore_ascii_case("authorization") && !name.eq_ignore_ascii_case("x-api-key"))
            .cloned(),
    );
    Ok(headers)
}

enum OpenBlock {
    Text(usize),
    Thinking(usize),
    Redacted,
    Tool { index: usize, id: String, name: String, input: Map<String, Value>, json: String, last_parsed_len: usize },
    Ignored,
}

struct MessageState {
    output: AssistantMessage,
    started: bool,
    stop_reason_seen: bool,
    stopped: bool,
    retry_blocked: bool,
    open: HashMap<usize, OpenBlock>,
    closed_indexes: HashSet<usize>,
    saw_spliced_envelope: bool,
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
            closed_indexes: HashSet::new(),
            saw_spliced_envelope: false,
            first_token: None,
        }
    }

    fn handle(&mut self, frame: &Value, events: &mut Vec<Event>) -> Result<bool, ProviderError> {
        let kind = frame.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "ping" {
            return Ok(false);
        }
        if kind == "error" {
            return Err(ProviderError::Stream(sse_error_detail(frame, "Anthropic stream error")));
        }
        if kind == "message_start" {
            if self.started {
                self.saw_spliced_envelope = true;
                return Ok(true);
            }
            self.started = true;
            self.output.response_id = frame.pointer("/message/id").and_then(Value::as_str).map(str::to_owned);
            if let Some(usage) = frame.pointer("/message/usage") {
                apply_usage(&mut self.output.usage, usage);
            }
            return Ok(true);
        }
        if !matches!(
            kind,
            "content_block_start" | "content_block_delta" | "content_block_stop" | "message_delta" | "message_stop"
        ) {
            return Ok(false);
        }
        if !self.started {
            return Err(ProviderError::Stream(format!("Anthropic {kind} before message_start")));
        }
        if self.stopped {
            return Ok(false);
        }
        if self.stop_reason_seen && kind != "message_stop" {
            return Ok(false);
        }
        match kind {
            "content_block_start" => {
                let Some(index) =
                    frame.get("index").and_then(Value::as_u64).and_then(|index| usize::try_from(index).ok())
                else {
                    return Ok(false);
                };
                if self.open.contains_key(&index) {
                    return Ok(false);
                }
                if self.saw_spliced_envelope && self.closed_indexes.contains(&index) {
                    self.open.insert(index, OpenBlock::Ignored);
                    return Ok(false);
                }
                let block = &frame["content_block"];
                if !block["type"].is_string() {
                    return Ok(false);
                }
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
                        OpenBlock::Redacted
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
                        let display_input = serde_json::to_vec(&input)
                            .ok()
                            .filter(|bytes| bytes.len() <= MAX_TOOL_DISPLAY_BYTES)
                            .map(|_| input.clone())
                            .unwrap_or_default();
                        self.output.content.push(AssistantBlock::ToolCall(ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            arguments: display_input,
                            thought_signature: None,
                        }));
                        events.push(Event::ToolcallStart { content_index, partial: self.output.clone() });
                        OpenBlock::Tool {
                            index: content_index,
                            id,
                            name,
                            input,
                            json: String::new(),
                            last_parsed_len: 0,
                        }
                    }
                    _ => OpenBlock::Ignored,
                };
                self.first_token.get_or_insert_with(Instant::now);
                self.open.insert(index, open);
            }
            "content_block_delta" => {
                let Some(index) =
                    frame.get("index").and_then(Value::as_u64).and_then(|index| usize::try_from(index).ok())
                else {
                    return Ok(false);
                };
                let Some(open) = self.open.get_mut(&index) else {
                    return Ok(false);
                };
                if !frame["delta"]["type"].is_string() {
                    return Ok(false);
                }
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
                    (OpenBlock::Tool { index: content_index, json, last_parsed_len, .. }, "input_json_delta") => {
                        let delta = frame.pointer("/delta/partial_json").and_then(Value::as_str).unwrap_or("");
                        if json.len().saturating_add(delta.len()) > MAX_TOOL_JSON_BYTES {
                            return Err(ProviderError::Stream("Anthropic tool arguments exceeded limit".into()));
                        }
                        json.push_str(delta);
                        if json.len() > MAX_TOOL_DISPLAY_BYTES {
                            if let AssistantBlock::ToolCall(call) = &mut self.output.content[*content_index] {
                                call.arguments.clear();
                            }
                        } else if !json.is_empty()
                            && (*last_parsed_len == 0 || json.len() - *last_parsed_len >= (json.len() / 32).max(256))
                        {
                            if let AssistantBlock::ToolCall(call) = &mut self.output.content[*content_index] {
                                call.arguments = crate::json::parse_streaming_json(json);
                            }
                            *last_parsed_len = json.len();
                        }
                        events.push(Event::ToolcallDelta {
                            content_index: *content_index,
                            delta: delta.into(),
                            partial: self.output.clone(),
                        });
                    }
                    (OpenBlock::Redacted | OpenBlock::Ignored, _) => {}
                    _ => return Ok(false),
                }
            }
            "content_block_stop" => {
                let Some(index) =
                    frame.get("index").and_then(Value::as_u64).and_then(|index| usize::try_from(index).ok())
                else {
                    return Ok(false);
                };
                let Some(open) = self.open.remove(&index) else {
                    return Ok(false);
                };
                if !matches!(&open, OpenBlock::Ignored) {
                    self.closed_indexes.insert(index);
                }
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
                    OpenBlock::Tool { index: content_index, id, name, input, json, .. } => {
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
                        self.output.content[content_index] = AssistantBlock::ToolCall(call.clone());
                        events.push(Event::ToolcallEnd {
                            content_index,
                            tool_call: call,
                            partial: self.output.clone(),
                        });
                    }
                    OpenBlock::Redacted => {}
                    OpenBlock::Ignored => {}
                }
            }
            "message_delta" => {
                if let Some(raw) = frame.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.output.stop_reason = stop_reason(raw);
                    self.stop_reason_seen = true;
                    if self.output.stop_reason == StopReason::Error {
                        let wire_details = frame.pointer("/delta/stop_details").filter(|value| !value.is_null());
                        let details = wire_details.cloned().unwrap_or_else(|| json!({"type":raw}));
                        self.output.error_message =
                            Some(if wire_details.is_some_and(|value| value["type"] == "refusal") {
                                let category = details["category"].as_str().filter(|value| !value.is_empty());
                                let label =
                                    category.map_or_else(|| "Refusal".to_owned(), |value| format!("Refusal ({value})"));
                                match details["explanation"].as_str().map(str::trim).filter(|value| !value.is_empty()) {
                                    Some(explanation) => format!("{label}: {explanation}"),
                                    None => label,
                                }
                            } else if raw == "refusal" {
                                "Refusal (no details provided)".into()
                            } else if raw == "sensitive" {
                                "Content flagged by safety filters".into()
                            } else {
                                format!("Anthropic stream ended with stop_reason: {raw}")
                            });
                        self.output.stop_details = Some(details);
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
            _ => unreachable!(),
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
    let headers = request_headers(model, options, key)?;
    let params = build_params(model, context, options)?;
    let fallback =
        if has_strict_tools(&params) { Some(build_params_with_strict(model, context, options, false)?) } else { None };
    let started = Instant::now();
    let first_deadline = options.first_event_timeout.map(|timeout| started + timeout);
    let response = match first_deadline {
        Some(deadline) => tokio::select! {
            result = post_with_strict_fallback(client, &url, &headers, &params, fallback.as_ref(), options, &mut state.retry_blocked) => result?,
            _ = tokio::time::sleep_until(deadline.into()) => return Err(ProviderError::Timeout("Anthropic stream timed out before first event".into())),
        },
        None => {
            post_with_strict_fallback(
                client,
                &url,
                &headers,
                &params,
                fallback.as_ref(),
                options,
                &mut state.retry_blocked,
            )
            .await?
        }
    };
    if !sink.push_or_cancel(Event::Start { partial: state.output.clone() }, cancel).await {
        return Err(ProviderError::Aborted);
    }
    let mut body = response.bytes_stream();
    let mut decoder = SseDecoder::new();
    let mut progressed = false;
    let mut last_semantic_progress = Instant::now();
    let mut last_liveness = last_semantic_progress;
    loop {
        let deadline =
            if progressed { options.idle_timeout.map(|timeout| last_liveness + timeout) } else { first_deadline };
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
            let name = frame.event.as_deref().unwrap_or("");
            if name == "error" {
                let value = serde_json::from_str::<Value>(&frame.data).unwrap_or(Value::Null);
                return Err(ProviderError::Stream(sse_error_detail(&value, &frame.data)));
            }
            if name != "ping"
                && !matches!(
                    name,
                    "message_start"
                        | "message_delta"
                        | "message_stop"
                        | "content_block_start"
                        | "content_block_delta"
                        | "content_block_stop"
                )
            {
                continue;
            }
            let value: Value = if name == "ping" {
                json!({"type":"ping"})
            } else {
                let Ok(value) = serde_json::from_str(&frame.data) else {
                    eprintln!("anthropic: skipping malformed SSE JSON for {name}");
                    continue;
                };
                value
            };
            if name != "ping" && name != value["type"].as_str().unwrap_or("") {
                eprintln!("anthropic: SSE event {name} does not match body type");
            }
            let mut events = Vec::new();
            let is_progress = state.handle(&value, &mut events)?;
            let now = Instant::now();
            if is_progress {
                progressed = true;
                last_semantic_progress = now;
                last_liveness = now;
            } else if value["type"] == "ping"
                && progressed
                && options.idle_timeout.is_some_and(|timeout| {
                    now.duration_since(last_semantic_progress)
                        < timeout.saturating_mul(PING_PROGRESS_MAX_IDLE_MULTIPLIER)
                })
            {
                last_liveness = now;
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

#[cfg(test)]
mod auth_header_tests {
    use super::*;

    fn model(base_url: &str, provider: &str) -> Model {
        Model {
            id: "claude-fixture".into(),
            api: API.into(),
            provider: provider.into(),
            base_url: base_url.into(),
            reasoning: false,
            max_tokens: None,
            tokenizer: None,
        }
    }

    fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }

    fn count(headers: &[(String, String)], name: &str) -> usize {
        headers.iter().filter(|(key, _)| key.eq_ignore_ascii_case(name)).count()
    }

    #[test]
    fn official_and_generic_routes_choose_distinct_default_credentials() {
        let opts = StreamOptions::default();
        for url in ["https://api.anthropic.com", "https://api.anthropic.com/v1", "https://api.anthropic.com:443/v1"] {
            let headers = request_headers(&model(url, "anthropic"), &opts, "test-key").unwrap();
            assert_eq!(header(&headers, "x-api-key"), Some("test-key"), "{url}");
            assert_eq!(header(&headers, "authorization"), None, "{url}");
        }
        for url in
            ["https://proxy.example/v1", "https://api.anthropic.com.evil.example/v1", "http://api.anthropic.com/v1"]
        {
            let headers = request_headers(&model(url, "anthropic"), &opts, "test-key").unwrap();
            assert_eq!(header(&headers, "authorization"), Some("Bearer test-key"), "{url}");
            assert_eq!(header(&headers, "x-api-key"), None, "{url}");
        }
        assert!(matches!(
            request_headers(&model("https://api.anthropic.com:8443/v1", "anthropic"), &opts, "test-key"),
            Err(ProviderError::Config(message)) if message.contains("nonstandard port")
        ));
    }

    #[test]
    fn caller_auth_overrides_are_case_insensitive_and_not_duplicated() {
        let opts = StreamOptions {
            extra_headers: vec![
                ("AUTHORIZATION".into(), "Bearer first".into()),
                ("authorization".into(), "Custom last".into()),
                ("X-API-KEY".into(), "first-key".into()),
                ("x-api-key".into(), "last-key".into()),
            ],
            ..Default::default()
        };
        for url in ["https://api.anthropic.com/v1", "https://proxy.example/v1"] {
            let headers = request_headers(&model(url, "anthropic"), &opts, "test-key").unwrap();
            assert_eq!(header(&headers, "authorization"), Some("Custom last"));
            assert_eq!(header(&headers, "x-api-key"), Some("last-key"));
            assert_eq!(count(&headers, "authorization"), 1);
            assert_eq!(count(&headers, "x-api-key"), 1);
        }
        for provider in ["opencode-go", "opencode-zen", "umans"] {
            let headers = request_headers(&model("https://proxy.example/v1", provider), &opts, "test-key").unwrap();
            assert_eq!(header(&headers, "x-api-key"), Some("last-key"), "{provider}");
            assert_eq!(header(&headers, "authorization"), None, "{provider}");
            assert_eq!(count(&headers, "x-api-key"), 1);
        }
    }
}

#[cfg(test)]
mod strict_tool_tests {
    use super::*;
    use ara_testkit::{FakeUpstream, Script};

    fn script(error_type: &str, first_error: &str) -> Script {
        serde_json::from_value(json!({"responses":[
            {"status":400,"body":json!({"type":"error","error":{"type":error_type,"message":first_error}}).to_string()},
            {"events":[{"raw":"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_ok\"}}\n\n"}]}
        ]}))
        .unwrap()
    }

    fn official_params() -> (Value, Value) {
        let model = Model {
            id: "claude-fixture".into(),
            api: API.into(),
            provider: "anthropic".into(),
            base_url: "https://api.anthropic.com/v1".into(),
            reasoning: false,
            max_tokens: None,
            tokenizer: None,
        };
        let context = Context {
            messages: vec![Message::User(crate::types::UserMessage::text("edit"))],
            tools: Some(vec![Tool {
                name: "edit".into(),
                description: "Edit a file".into(),
                parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
            }]),
            ..Default::default()
        };
        let options = StreamOptions::default();
        (
            build_params(&model, &context, &options).unwrap(),
            build_params_with_strict(&model, &context, &options, false).unwrap(),
        )
    }

    #[test]
    fn strict_rejection_requires_a_specific_pre_stream_http_400() {
        let error = |status, code: Option<&str>, detail: &str| PostError {
            cause: ProviderError::Http { status, detail: detail.into() },
            code: code.map(str::to_owned),
        };
        assert!(strict_rejection(&error(400, Some("invalid_request_error"), "The compiled grammar is too large")));
        assert!(strict_rejection(&error(400, None, "structured_outputs not supported")));
        assert!(strict_rejection(&error(400, None, "structured_outputs not enabled")));
        assert!(strict_rejection(&error(400, None, "structured-output not available")));
        assert!(strict_rejection(&error(400, None, "model does not support structured outputs")));
        assert!(strict_rejection(&error(400, None, "tools.0.custom.strict: extra inputs are not permitted")));
        assert!(!strict_rejection(&error(400, Some("other"), "The compiled grammar is too large")));
        assert!(!strict_rejection(&error(500, Some("invalid_request_error"), "The compiled grammar is too large")));
        assert!(!strict_rejection(&error(400, Some("invalid_request_error"), "Some other validation error")));
    }

    #[tokio::test]
    async fn strict_grammar_rejection_retries_once_before_the_stream_starts() {
        let server = FakeUpstream::start(
            script("invalid_request_error", "The compiled grammar is too large, which would cause performance issues."),
            None,
        )
        .await
        .unwrap();
        let (params, fallback) = official_params();
        let mut retry_blocked = false;
        let response = post_with_strict_fallback(
            &reqwest::Client::new(),
            &format!("{}/messages", server.base_url()),
            &[],
            &params,
            Some(&fallback),
            &StreamOptions { retry: RetryPolicy { max_attempts: 1, ..Default::default() }, ..Default::default() },
            &mut retry_blocked,
        )
        .await
        .unwrap();
        assert!(response.status().is_success());
        assert_eq!(server.served(), 2);
        let requests = server.requests.lock().await;
        assert_eq!(requests[0]["body"]["tools"][0]["strict"], true);
        assert!(requests[1]["body"]["tools"][0].get("strict").is_none());
        assert_eq!(requests[1]["body"]["tools"][0]["input_schema"], fallback["tools"][0]["input_schema"]);
    }

    #[tokio::test]
    async fn unsupported_structured_outputs_retries_without_strict_tools() {
        let server = FakeUpstream::start(
            script("invalid_request_error", "This model does not support structured outputs"),
            None,
        )
        .await
        .unwrap();
        let (params, fallback) = official_params();
        let mut retry_blocked = false;
        let response = post_with_strict_fallback(
            &reqwest::Client::new(),
            &format!("{}/messages", server.base_url()),
            &[],
            &params,
            Some(&fallback),
            &StreamOptions { retry: RetryPolicy { max_attempts: 1, ..Default::default() }, ..Default::default() },
            &mut retry_blocked,
        )
        .await
        .unwrap();
        assert!(response.status().is_success());
        assert_eq!(server.served(), 2);
        assert!(server.requests.lock().await[1]["body"]["tools"][0].get("strict").is_none());
    }

    #[tokio::test]
    async fn unrelated_bad_request_does_not_replay_a_strict_request() {
        let server =
            FakeUpstream::start(script("invalid_request_error", "Some other validation error."), None).await.unwrap();
        let (params, fallback) = official_params();
        let mut retry_blocked = false;
        let error = post_with_strict_fallback(
            &reqwest::Client::new(),
            &format!("{}/messages", server.base_url()),
            &[],
            &params,
            Some(&fallback),
            &StreamOptions { retry: RetryPolicy { max_attempts: 1, ..Default::default() }, ..Default::default() },
            &mut retry_blocked,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ProviderError::Http { status: 400, .. }));
        assert_eq!(server.served(), 1);
    }
}
