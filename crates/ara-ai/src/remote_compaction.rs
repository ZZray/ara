//! Fixed OMP remote compaction wire contracts.
//!
//! Ported from `packages/agent/src/compaction/openai.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; THIRD_PARTY_NOTICES.md).
//! Hosts own authentication and candidate fallback; these helpers perform one
//! selected native protocol or one explicit generic endpoint request.

use crate::providers::{openai_codex_responses as codex, openai_responses as responses};
use crate::{AssistantBlock, Context, Message, Model, ProviderError, StopReason, Usage, UserBlock, UserContent};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub const PRESERVE_KEY: &str = "openaiRemoteCompaction";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConfig {
    pub enabled: Option<bool>,
    pub api: Option<String>,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub v2_streaming_enabled: Option<bool>,
    pub v2_endpoint: Option<String>,
    pub streaming_endpoint: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteVersion {
    V1,
    V2,
}

/// Authentication stays in the host call binding and is never serializable.
#[derive(Clone)]
pub struct RemoteRequestOptions {
    pub api_key: Option<String>,
    pub extra_headers: Vec<(String, String)>,
    pub cancel: CancellationToken,
    /// `None` disables the watchdog. Default is 180 seconds per attempt.
    pub timeout: Option<Duration>,
    pub retry_delay: Duration,
    pub session_id: Option<String>,
    pub prompt_cache_key: Option<String>,
    pub retained_message_budget: u64,
    /// Resolved normal-turn reasoning policy, omitted for Off/unsupported.
    pub reasoning: Option<Value>,
    pub supports_images: bool,
    /// Generic wire model override; authentication still belongs to the
    /// originally selected Host route.
    pub generic_model: Option<String>,
}

impl Default for RemoteRequestOptions {
    fn default() -> Self {
        Self {
            api_key: None,
            extra_headers: Vec::new(),
            cancel: CancellationToken::new(),
            timeout: Some(DEFAULT_TIMEOUT),
            retry_delay: Duration::from_secs(1),
            session_id: None,
            prompt_cache_key: None,
            retained_message_budget: 64_000,
            reasoning: None,
            supports_images: false,
            generic_model: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RemoteNativeRequest {
    pub config: RemoteConfig,
    pub version: RemoteVersion,
    pub instructions: String,
    pub previous_replacement_history: Option<Vec<Value>>,
}

#[derive(Clone, Debug)]
pub struct RemoteGenericRequest {
    pub system_prompt: String,
    pub prompt: String,
    pub max_tokens: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAttempt {
    pub elapsed_ms: u64,
    pub usage: Usage,
    pub error: Option<String>,
    pub status: Option<u16>,
}

#[derive(Clone, Debug)]
pub struct RemoteResult {
    pub summary: String,
    pub short_summary: Option<String>,
    pub preserve_data: Option<Value>,
    pub usage: Usage,
    pub attempts: Vec<RemoteAttempt>,
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{cause}")]
pub struct RemoteError {
    #[source]
    pub cause: ProviderError,
    pub attempts: Vec<RemoteAttempt>,
    pub auth_failed: bool,
}

#[derive(Clone, Debug)]
pub struct NativePreserveData {
    pub provider: Option<String>,
    pub replacement_history: Vec<Value>,
    pub compaction_item: Value,
    pub version: RemoteVersion,
    pub used_tokens: Option<u64>,
}

pub type RemotePreserveData = NativePreserveData;

pub fn should_use_v1(model: &Model, config: &RemoteConfig) -> bool {
    if config.enabled == Some(false) {
        return false;
    }
    if matches!(model.provider.as_str(), "openai" | "openai-codex") {
        return true;
    }
    config.enabled == Some(true) && native_api(model, config)
}

pub fn should_use_v2(model: &Model, config: &RemoteConfig) -> bool {
    config.enabled != Some(false) && config.v2_streaming_enabled == Some(true) && native_api(model, config)
}

fn native_api(model: &Model, config: &RemoteConfig) -> bool {
    matches!(config.api.as_deref().unwrap_or(&model.api), "openai-responses" | "openai-codex-responses")
}

pub(crate) fn valid_compaction_item(item: &Value) -> bool {
    item.is_object()
        && match item.get("type").and_then(Value::as_str) {
            Some("compaction") => item.get("encrypted_content").is_some_and(Value::is_string),
            Some("compaction_summary") => true,
            _ => false,
        }
}

/// Read the durable fixed-OMP preserve slot. No usage bucket is fabricated.
pub fn parse_native_preserve_data(preserve: &Value) -> Option<NativePreserveData> {
    let candidate = preserve.get(PRESERVE_KEY)?.as_object()?;
    let history = candidate.get("replacementHistory")?.as_array()?;
    if history.is_empty()
        || history.len() > 1024
        || history.iter().any(|item| !item.is_object())
        || serde_json::to_vec(history).ok()?.len() > 32 * 1024 * 1024
    {
        return None;
    }
    let version = if candidate.get("version").and_then(Value::as_str) == Some("v2") {
        RemoteVersion::V2
    } else {
        RemoteVersion::V1
    };
    let item = match version {
        RemoteVersion::V1 => candidate.get("compactionItem")?,
        RemoteVersion::V2 => history.iter().rev().find(|item| valid_compaction_item(item))?,
    };
    if !valid_compaction_item(item) || !history.contains(item) {
        return None;
    }
    Some(NativePreserveData {
        provider: candidate.get("provider").and_then(Value::as_str).map(str::to_owned),
        replacement_history: history.clone(),
        compaction_item: item.clone(),
        version,
        used_tokens: candidate.get("usedTokens").and_then(Value::as_u64),
    })
}

pub fn parse_remote_preserve_data(preserve: &Value) -> Option<RemotePreserveData> {
    parse_native_preserve_data(preserve)
}

pub fn endpoint_fingerprint(endpoint: &str) -> String {
    crate::responses_stream::responses_endpoint_fingerprint(endpoint)
}

/// Construct a checked replay payload for a transient same-route Session view.
/// Image-bearing payloads require the host's separate capability check.
pub fn native_replacement_payload(model: &Model, items: &[Value]) -> Option<Value> {
    responses::checked_remote_replacement(model, items, true)
}

/// A checked model-only replay carrier is history, not a completed answer.
/// Ordinary Assistant turns and unresolved replay tool calls cannot continue.
pub fn is_native_compaction_carrier(message: &Message, model: &Model) -> bool {
    matches!(message, Message::Assistant(assistant) if responses::remote_compaction_carrier(assistant, model))
}

pub fn validate_replacement_for_route(
    model: &Model,
    items: &[Value],
    supports_images: bool,
) -> Result<(), ProviderError> {
    responses::checked_remote_replacement(model, items, supports_images).ok_or_else(|| {
        ProviderError::Config("native remote replacement history cannot be replayed by the active route".into())
    })?;
    Ok(())
}

pub fn resolve_native_endpoint(
    model: &Model,
    config: &RemoteConfig,
    version: RemoteVersion,
) -> Result<String, ProviderError> {
    if !native_api(model, config) && !matches!(model.provider.as_str(), "openai" | "openai-codex") {
        return Err(ProviderError::Config("remote compaction API is not supported".into()));
    }
    if config.api.as_deref().unwrap_or(&model.api) == "azure-openai-responses" {
        return Err(ProviderError::Config("Azure remote compaction is not ported".into()));
    }
    let configured = match version {
        RemoteVersion::V1 => config.endpoint.as_deref(),
        RemoteVersion::V2 => config.v2_endpoint.as_deref().or(config.streaming_endpoint.as_deref()),
    };
    if let Some(endpoint) = configured.filter(|endpoint| !endpoint.is_empty()) {
        return Ok(endpoint.into());
    }
    let is_codex = model.provider == "openai-codex" || config.api.as_deref().unwrap_or(&model.api) == codex::API;
    let raw = if model.base_url.is_empty() {
        if is_codex { codex::BASE_URL } else { "https://api.openai.com/v1" }
    } else {
        &model.base_url
    };
    let base = raw.trim_end_matches('/');
    Ok(match version {
        RemoteVersion::V2 if is_codex => codex::resolve_responses_url(base),
        RemoteVersion::V2 if base.ends_with("/responses") => base.into(),
        RemoteVersion::V2 if base.ends_with("/v1") => format!("{base}/responses"),
        RemoteVersion::V2 => format!("{base}/v1/responses"),
        RemoteVersion::V1 if is_codex => {
            // The pinned endpoint accepts /codex and /codex/vN bases.
            let tail = base.rsplit('/').next().unwrap_or("");
            if base.ends_with("/codex")
                || tail.strip_prefix('v').is_some_and(|suffix| {
                    !suffix.is_empty()
                        && suffix.bytes().all(|byte| byte.is_ascii_digit())
                        && base[..base.len() - tail.len()].trim_end_matches('/').ends_with("/codex")
                })
            {
                format!("{base}/responses/compact")
            } else {
                format!("{base}/codex/responses/compact")
            }
        }
        RemoteVersion::V1 if base.ends_with("/v1") => format!("{base}/responses/compact"),
        RemoteVersion::V1 => format!("{base}/v1/responses/compact"),
    })
}

fn content_parts(content: &UserContent) -> Vec<Value> {
    match content {
        UserContent::Text(text) => if text.trim().is_empty() { Vec::new() } else { vec![json!({"type":"input_text","text":text})] },
        UserContent::Blocks(blocks) => blocks.iter().filter_map(|block| match block {
            UserBlock::Text(text) if !text.text.trim().is_empty() => Some(json!({"type":"input_text","text":text.text})),
            UserBlock::Text(_) => None,
            UserBlock::Image(image) => Some(json!({"type":"input_image","detail":"auto","image_url":format!("data:{};base64,{}",image.mime_type,image.data)})),
        }).collect(),
    }
}

fn register_calls(items: &[Value], calls: &mut HashSet<String>) {
    for item in items {
        if item["type"] == "function_call"
            && let Some(id) = item["call_id"].as_str()
        {
            calls.insert(id.to_owned());
        }
    }
}

fn replay_item(item: &Value) -> Value {
    let mut item = item.clone();
    if let Some(object) = item.as_object_mut() {
        object.remove("status");
    }
    item
}

/// Full native history for the supported Rust message forms, without the
/// normal provider warmup/reasoning filter. Opaque unsupported items fail.
pub fn build_native_history(
    model: &Model,
    context: &Context,
    previous: Option<&[Value]>,
) -> Result<Vec<Value>, ProviderError> {
    let mut input = previous.unwrap_or_default().to_vec();
    if input.iter().any(|item| !item.is_object()) {
        return Err(ProviderError::Config("remote replacement history contains a non-object item".into()));
    }
    let mut calls = HashSet::new();
    register_calls(&input, &mut calls);
    for (index, message) in crate::transform::transform_messages(&context.messages).iter().enumerate() {
        match message {
            Message::User(user) => {
                let parts = content_parts(&user.content);
                if !parts.is_empty() {
                    input.push(json!({"type":"message","role":"user","content":parts}));
                }
            }
            Message::Developer(developer) => {
                let parts = content_parts(&developer.content);
                if !parts.is_empty() {
                    input.push(json!({"type":"message","role":"developer","content":parts}));
                }
            }
            Message::Assistant(assistant) => {
                let payload = assistant.provider_payload.as_ref().filter(|payload| {
                    payload["type"] == "openaiResponsesHistory"
                        && payload["provider"] == model.provider
                        && assistant.provider == model.provider
                        && payload
                            .get("endpointSha256")
                            .is_none_or(|fingerprint| fingerprint == &json!(endpoint_fingerprint(&model.base_url)))
                });
                if let Some(payload) = payload {
                    let items = payload["items"]
                        .as_array()
                        .ok_or_else(|| ProviderError::Config("native remote history lacks items".into()))?;
                    if items.iter().any(|item| !supported_remote_item(item)) {
                        return Err(ProviderError::Config("unsupported native remote history item".into()));
                    }
                    // ARA's pre-dt journals are incremental, unlike OMP's old
                    // absent-dt convention. Explicit false is a full snapshot.
                    if payload.get("dt") == Some(&Value::Bool(false)) {
                        input.clear();
                        calls.clear();
                    }
                    input.extend(items.iter().map(replay_item));
                    register_calls(items, &mut calls);
                    continue;
                }
                let mut turn = Vec::new();
                let mut turn_calls = Vec::new();
                for block in &assistant.content {
                    match block {
                        AssistantBlock::Thinking(thinking) if assistant.stop_reason != StopReason::Error => {
                            if let Some(signature) = &thinking.thinking_signature
                                && let Ok(item) = serde_json::from_str::<Value>(signature)
                                && item.is_object()
                                && item["type"] == "reasoning"
                            {
                                turn.push(replay_item(&item));
                            }
                        }
                        AssistantBlock::Text(text) if !text.text.trim().is_empty() => {
                            let signature = text
                                .text_signature
                                .as_deref()
                                .and_then(|value| serde_json::from_str::<Value>(value).ok());
                            let mut item = json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":text.text,"annotations":[]}],"id":format!("msg_{index}")});
                            if let Some(signature) = signature {
                                if let Some(id) =
                                    signature.get("id").and_then(Value::as_str).filter(|id| id.len() <= 64)
                                {
                                    item["id"] = json!(id);
                                }
                                if let Some(phase) = signature
                                    .get("phase")
                                    .filter(|phase| matches!(phase.as_str(), Some("commentary" | "final_answer")))
                                {
                                    item["phase"] = phase.clone();
                                }
                            }
                            turn.push(item);
                        }
                        AssistantBlock::ToolCall(call) => {
                            let call_id = crate::transform::responses_call_component(&call.id);
                            if call_id.is_empty() || call.name.is_empty() {
                                return Err(ProviderError::Config("remote function call has no id or name".into()));
                            }
                            calls.insert(call_id.to_owned());
                            let mut item = json!({"type":"function_call","call_id":call_id,"name":call.name,"arguments":serde_json::to_string(&call.arguments).map_err(|error| ProviderError::Config(error.to_string()))?});
                            if let Some((_, item_id)) = call.id.split_once('|') {
                                let changed_model = assistant.provider == model.provider
                                    && assistant.api == model.api
                                    && assistant.model != model.id;
                                if !item_id.is_empty()
                                    && !(changed_model && (item_id.starts_with("fc_") || item_id.starts_with("fcr_")))
                                {
                                    item["id"] = json!(item_id);
                                }
                            }
                            turn_calls.push(item);
                        }
                        _ => {}
                    }
                }
                // Interleaved assistant text must precede its whole call batch.
                input.extend(turn);
                input.extend(turn_calls);
            }
            Message::ToolResult(result) => {
                let call_id = crate::transform::responses_call_component(&result.tool_call_id);
                if !calls.contains(call_id) {
                    continue;
                }
                let blocks = UserContent::Blocks(result.content.clone());
                let output = if result.content.iter().any(|block| matches!(block, UserBlock::Image(_))) {
                    json!(content_parts(&blocks))
                } else {
                    json!(blocks.plain_text())
                };
                input.push(json!({"type":"function_call_output","call_id":call_id,"output":output}));
            }
        }
    }
    Ok(input.into_iter().map(|item| replay_item(&item)).collect())
}

fn supported_remote_item(item: &Value) -> bool {
    item.is_object()
        && (valid_compaction_item(item)
            || matches!(
                item.get("type").and_then(Value::as_str),
                Some("message" | "reasoning" | "function_call" | "function_call_output" | "item_reference")
            ))
}

pub fn build_native_request(
    model: &Model,
    context: &Context,
    request: &RemoteNativeRequest,
    options: &RemoteRequestOptions,
) -> Result<Value, ProviderError> {
    let mut input = build_native_history(model, context, request.previous_replacement_history.as_deref())?;
    let tools = if request.version == RemoteVersion::V2 {
        let mut tools_model = model.clone();
        tools_model.api = responses::API.into();
        responses::build_request(
            &tools_model,
            &Context { tools: context.tools.clone(), ..Default::default() },
            &responses::RequestOptions::default(),
        )?
        .get("tools")
        .cloned()
    } else {
        None
    };
    input = trim_native_input_to_context_window(model, &input, &request.instructions, tools.as_ref()).input;
    let mut body = json!({"model":request.config.model.as_deref().unwrap_or(&model.id),"input":input,"instructions":request.instructions});
    if request.version == RemoteVersion::V2 {
        body["input"].as_array_mut().expect("native input array").push(json!({"type":"compaction_trigger"}));
        body["stream"] = json!(true);
        body["store"] = json!(false);
        if let Some(reasoning) = &options.reasoning {
            body["reasoning"] = reasoning.clone();
            body["include"] = json!(["reasoning.encrypted_content"]);
        }
        if let Some(key) =
            options.prompt_cache_key.as_deref().or(options.session_id.as_deref()).filter(|key| !key.is_empty())
        {
            body["prompt_cache_key"] = json!(key);
        }
        if let Some(tools) = tools {
            body["tools"] = tools;
            body["tool_choice"] = json!("auto");
        }
    }
    Ok(body)
}

#[derive(Clone, Debug)]
pub struct TrimNativeInputResult {
    pub input: Vec<Value>,
    pub rewritten_outputs: usize,
    pub estimated_tokens_before: u64,
    pub estimated_tokens_after: u64,
    /// Only a selected exact tokenizer proves a token-window observation.
    pub exact_tokenizer: bool,
}

fn normalized_estimate(value: &Value) -> (Value, u64) {
    match value {
        Value::Array(items) => {
            let mut images = 0u64;
            let items = items
                .iter()
                .map(|item| {
                    let (item, count) = normalized_estimate(item);
                    images = images.saturating_add(count);
                    item
                })
                .collect();
            (Value::Array(items), images)
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("input_image") {
                let mut object = object.clone();
                object.insert("image_url".into(), json!("<image>"));
                return (Value::Object(object), 12_000);
            }
            let mut images = 0u64;
            let object = object
                .iter()
                .map(|(key, value)| {
                    let (value, count) = normalized_estimate(value);
                    images = images.saturating_add(count);
                    (key.clone(), value)
                })
                .collect();
            (Value::Object(object), images)
        }
        other => (other.clone(), 0),
    }
}

fn estimate_request(model: &Model, input: &[Value], instructions: &str, tools: Option<&Value>) -> (u64, bool) {
    let mut request = json!({"instructions":instructions,"input":input});
    if let Some(tools) = tools {
        request["tools"] = tools.clone();
    }
    let (normalized, images) = normalized_estimate(&request);
    let serialized = normalized.to_string();
    let (text, exact) = match crate::model_tokenizer::count_model_fragments(model, [serialized.as_str()]) {
        crate::model_tokenizer::ModelContentCount::Exact(count) => (count, true),
        _ => (serialized.len() as u64, false),
    };
    (text.saturating_add(images).saturating_add(256), exact)
}

/// Same trailing-output-only rewrite policy as OMP. Unknown tokenizers use a
/// UTF-8 sizing observation; this is not a proof of the model token window.
pub fn trim_native_input_to_context_window(
    model: &Model,
    input: &[Value],
    instructions: &str,
    tools: Option<&Value>,
) -> TrimNativeInputResult {
    let (before, exact) = estimate_request(model, input, instructions, tools);
    let mut result = TrimNativeInputResult {
        input: input.to_vec(),
        rewritten_outputs: 0,
        estimated_tokens_before: before,
        estimated_tokens_after: before,
        exact_tokenizer: exact,
    };
    let Some(window) = model.context_window.filter(|window| window.is_finite() && *window > 0.0) else { return result };
    if before as f64 <= window {
        return result;
    }
    let mut changed = input.to_vec();
    let mut after = before;
    let mut rewritten = 0;
    for index in (0..input.len()).rev() {
        if after as f64 <= window {
            break;
        }
        let item = &input[index];
        let image_attachment = item["type"] == "message"
            && item["role"] == "user"
            && item["content"].as_array().is_some_and(|parts| {
                parts.iter().any(|part| part["type"] == "input_image")
                    && parts.iter().any(|part| part["text"] == "Attached image(s) from tool result:")
            });
        if image_attachment {
            continue;
        }
        match item["type"].as_str() {
            Some("function_call_output" | "custom_tool_call_output") => {
                changed[index]["output"] = json!("Output: exceeded available model context → truncated.")
            }
            Some("tool_search_output") => changed[index]["tools"] = json!([]),
            _ => break,
        }
        rewritten += 1;
        after = estimate_request(model, &changed, instructions, tools).0;
    }
    if rewritten > 0 && after as f64 <= window {
        result.input = changed;
        result.rewritten_outputs = rewritten;
        result.estimated_tokens_after = after;
    }
    result
}

pub(crate) fn auth_failure(cause: &ProviderError) -> bool {
    if matches!(cause.status(), Some(401 | 403)) {
        return true;
    }
    match cause {
        ProviderError::Http { detail, .. } | ProviderError::Stream(detail) => {
            let lower = detail.to_ascii_lowercase();
            ["auth_unavailable", "invalid_api_key", "authentication_error", "invalid_authentication"]
                .iter()
                .any(|code| lower.contains(code))
        }
        _ => false,
    }
}

pub(crate) fn private_safe_error(cause: ProviderError, options: &RemoteRequestOptions) -> ProviderError {
    let redact = |text: String| {
        let mut text = text;
        if let Some(key) = options.api_key.as_deref().filter(|key| !key.is_empty()) {
            text = text.replace(key, "[redacted]");
        }
        for (_, value) in &options.extra_headers {
            if !value.is_empty() {
                text = text.replace(value, "[redacted]");
            }
        }
        text.chars().take(2000).collect::<String>()
    };
    match cause {
        ProviderError::Http { status, detail } => ProviderError::Http { status, detail: redact(detail) },
        ProviderError::Transport(text) => ProviderError::Transport(redact(text)),
        ProviderError::Stream(text) => ProviderError::Stream(redact(text)),
        ProviderError::Incomplete(text) => ProviderError::Incomplete(redact(text)),
        ProviderError::Timeout(text) => ProviderError::Timeout(redact(text)),
        ProviderError::Config(text) => ProviderError::Config(redact(text)),
        other => other,
    }
}

pub(crate) fn error_with_attempt(
    cause: ProviderError,
    elapsed: Duration,
    usage: Usage,
    options: &RemoteRequestOptions,
) -> RemoteError {
    let auth_failed = auth_failure(&cause);
    let cause = private_safe_error(cause, options);
    let attempt = RemoteAttempt {
        elapsed_ms: elapsed.as_millis() as u64,
        usage,
        error: Some(cause.to_string()),
        status: cause.status(),
    };
    RemoteError { cause, attempts: vec![attempt], auth_failed }
}

pub(crate) async fn with_watchdog<T>(
    future: impl std::future::Future<Output = Result<T, ProviderError>>,
    options: &RemoteRequestOptions,
) -> Result<T, ProviderError> {
    if options.cancel.is_cancelled() {
        return Err(ProviderError::Aborted);
    }
    tokio::select! { biased;
        _ = options.cancel.cancelled() => Err(ProviderError::Aborted),
        result = async {
            match options.timeout {
                Some(timeout) => tokio::time::timeout(timeout,future).await.map_err(|_| ProviderError::Timeout("remote compaction request timeout".into()))?,
                None => future.await,
            }
        } => {
            if options.cancel.is_cancelled() { Err(ProviderError::Aborted) } else { result }
        }
    }
}

pub(crate) async fn post(
    client: &reqwest::Client,
    endpoint: &str,
    body: &Value,
    headers: &[(String, String)],
) -> Result<reqwest::Response, ProviderError> {
    let mut request = client.post(endpoint).json(body);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let response =
        request.send().await.map_err(|_| ProviderError::Transport("remote compaction request failed".into()))?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = read_body(response).await?;
        let value = serde_json::from_slice::<Value>(&body).ok();
        // Retain the structured auth code even if its message lacks that code.
        let code = value
            .as_ref()
            .and_then(|value| {
                value
                    .pointer("/error/code")
                    .and_then(Value::as_str)
                    .or_else(|| value.pointer("/error/type").and_then(Value::as_str))
            })
            .unwrap_or("");
        let detail = crate::error::parse_error_envelope(&String::from_utf8_lossy(&body));
        return Err(ProviderError::Http {
            status,
            detail: if code.is_empty() { detail } else { format!("{code}: {detail}") },
        });
    }
    Ok(response)
}

pub(crate) async fn read_body(response: reqwest::Response) -> Result<Vec<u8>, ProviderError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(bytes) = stream.next().await {
        let bytes = bytes.map_err(|_| ProviderError::Transport("remote compaction response body failed".into()))?;
        if body.len().saturating_add(bytes.len()) > 32 * 1024 * 1024 {
            return Err(ProviderError::Stream("remote compaction JSON body exceeds size limit".into()));
        }
        body.extend_from_slice(&bytes);
    }
    Ok(body)
}

pub(crate) fn parse_usage(value: &Value) -> Usage {
    let usage = value.get("usage").unwrap_or(&Value::Null);
    Usage {
        input: usage["input_tokens"].as_u64().or(usage["prompt_tokens"].as_u64()),
        output: usage["output_tokens"].as_u64().or(usage["completion_tokens"].as_u64()),
        total_tokens: usage["total_tokens"].as_u64(),
        cache_read: usage
            .pointer("/input_tokens_details/cached_tokens")
            .or(usage.pointer("/prompt_tokens_details/cached_tokens"))
            .and_then(Value::as_u64),
        reasoning_tokens: usage
            .pointer("/output_tokens_details/reasoning_tokens")
            .or(usage.pointer("/completion_tokens_details/reasoning_tokens"))
            .and_then(Value::as_u64),
        ..Default::default()
    }
}

fn native_headers(
    model: &Model,
    request: &RemoteNativeRequest,
    options: &RemoteRequestOptions,
) -> Result<Vec<(String, String)>, ProviderError> {
    let is_codex =
        model.provider == "openai-codex" || request.config.api.as_deref().unwrap_or(&model.api) == codex::API;
    let mut headers = if is_codex {
        let mut route = model.clone();
        route.api = codex::API.into();
        route.id = request.config.model.clone().unwrap_or_else(|| model.id.clone());
        let (_, _, headers) = codex::prepare_request(
            &route,
            &Context::default(),
            &responses::RequestOptions::default(),
            options.api_key.as_deref(),
            &options.extra_headers,
            options.session_id.as_deref(),
        )?;
        headers
    } else {
        let mut headers = options.extra_headers.clone();
        if let Some(key) = options.api_key.as_deref().filter(|key| !key.is_empty()) {
            headers.push(("authorization".into(), format!("Bearer {key}")));
        }
        headers
    };
    headers.retain(|(name, _)| !name.eq_ignore_ascii_case("accept") && !name.eq_ignore_ascii_case("content-type"));
    headers.push(("content-type".into(), "application/json".into()));
    headers.push((
        "accept".into(),
        if request.version == RemoteVersion::V2 { "text/event-stream" } else { "application/json" }.into(),
    ));
    if request.version == RemoteVersion::V2 {
        if is_codex {
            headers.push(("x-codex-beta-features".into(), "remote_compaction_v2".into()));
        } else if let Some(id) =
            options.session_id.as_deref().or(options.prompt_cache_key.as_deref()).filter(|id| !id.is_empty())
        {
            headers.push(("session_id".into(), id.into()));
            headers.push(("x-client-request-id".into(), id.into()));
        }
    }
    Ok(headers)
}

pub async fn request_native(
    client: reqwest::Client,
    model: Model,
    context: Context,
    request: RemoteNativeRequest,
    mut options: RemoteRequestOptions,
) -> Result<RemoteResult, RemoteError> {
    // Native maintenance is awaited by the Host instead of its ordinary
    // spawned model stream. Keep the HTTP poll chain off the Windows main
    // thread's smaller stack, and await the same worker for cancellation receipts.
    let start = Instant::now();
    let cancel = options.cancel.child_token();
    options.cancel = cancel.clone();
    let _guard = cancel.clone().drop_guard();
    tokio::spawn(request_native_inner(client, model, context, request, options)).await.map_err(|_| RemoteError {
        cause: if cancel.is_cancelled() {
            ProviderError::Aborted
        } else {
            ProviderError::Transport("remote compaction worker failed".into())
        },
        attempts: vec![RemoteAttempt {
            elapsed_ms: start.elapsed().as_millis() as u64,
            usage: Usage::unknown(),
            error: Some("remote compaction worker failed".into()),
            status: None,
        }],
        auth_failed: false,
    })?
}

async fn request_native_inner(
    client: reqwest::Client,
    model: Model,
    context: Context,
    request: RemoteNativeRequest,
    options: RemoteRequestOptions,
) -> Result<RemoteResult, RemoteError> {
    let start = Instant::now();
    let prepared = (|| {
        Ok((
            resolve_native_endpoint(&model, &request.config, request.version)?,
            build_native_request(&model, &context, &request, &options)?,
            native_headers(&model, &request, &options)?,
        ))
    })();
    let (endpoint, body, headers) =
        prepared.map_err(|cause| error_with_attempt(cause, start.elapsed(), Usage::unknown(), &options))?;
    if request.version == RemoteVersion::V2 {
        return crate::remote_compaction_v2::request_v2(&client, &model, &endpoint, &body, &headers, &options).await;
    }
    let mut usage = Usage::unknown();
    let result = with_watchdog(async {
        let response = post(&client,&endpoint,&body,&headers).await?;
        let value:Value = serde_json::from_slice(&read_body(response).await?).map_err(|_| ProviderError::Stream("remote compaction response is invalid JSON".into()))?;
        usage = parse_usage(&value);
        let output = value["output"].as_array().ok_or_else(|| ProviderError::Stream("remote compaction response missing output".into()))?;
        let replacement:Vec<_> = output.iter().filter(|item| item.is_object() && (matches!(item["type"].as_str(),Some("compaction"|"compaction_summary")) || item["type"] == "message" && matches!(item["role"].as_str(),Some("assistant"|"user")))).cloned().collect();
        let compaction = replacement.iter().rev().find(|item| valid_compaction_item(item)).ok_or_else(|| ProviderError::Stream("remote compaction response missing compaction item".into()))?;
        validate_replacement_for_route(&model,&replacement,options.supports_images)?;
        Ok(json!({PRESERVE_KEY:{"provider":model.provider,"replacementHistory":replacement,"compactionItem":compaction}}))
    },&options).await;
    match result {
        Ok(preserve) => Ok(RemoteResult {
            summary: remote_summary(None),
            short_summary: None,
            preserve_data: Some(preserve),
            usage: usage.clone(),
            attempts: vec![RemoteAttempt {
                elapsed_ms: start.elapsed().as_millis() as u64,
                usage,
                error: None,
                status: None,
            }],
        }),
        Err(cause) => Err(error_with_attempt(cause, start.elapsed(), usage, &options)),
    }
}

pub(crate) fn remote_summary(input: Option<u64>) -> String {
    let mut summary = "Remote compaction preserved provider-native history for this session.".to_owned();
    if let Some(input) = input.filter(|input| *input > 0) {
        summary.push_str(&format!(" Compaction processed {input} input tokens."));
    }
    summary
}

pub async fn request_generic(
    client: reqwest::Client,
    model: Model,
    endpoint: String,
    request: RemoteGenericRequest,
    options: RemoteRequestOptions,
) -> Result<RemoteResult, RemoteError> {
    let start = Instant::now();
    let path = reqwest::Url::parse(&endpoint).ok().map(|url| url.path().to_owned()).unwrap_or_else(|| endpoint.clone());
    let chat = path.trim_end_matches('/').ends_with("/chat/completions");
    let mut headers = vec![("content-type".into(), "application/json".into())];
    let mut body = if chat {
        headers.extend(options.extra_headers.clone());
        if let Some(key) = options.api_key.as_deref().filter(|key| !key.is_empty()) {
            headers.push(("authorization".into(), format!("Bearer {key}")));
        }
        json!({"model":options.generic_model.as_deref().unwrap_or(&model.id),"messages":[{"role":"system","content":request.system_prompt},{"role":"user","content":request.prompt}],"stream":false})
    } else {
        json!({"systemPrompt":request.system_prompt,"prompt":request.prompt})
    };
    if let Some(max) = request.max_tokens {
        body[if chat { "max_tokens" } else { "maxTokens" }] = json!(max);
    }
    let mut usage = Usage::unknown();
    let result = with_watchdog(
        async {
            let response = post(&client, &endpoint, &body, &headers).await?;
            let value: Value = serde_json::from_slice(&read_body(response).await?)
                .map_err(|_| ProviderError::Stream("remote compaction response is invalid JSON".into()))?;
            usage = parse_usage(&value);
            let summary = if chat {
                match value.pointer("/choices/0/message/content") {
                    Some(Value::String(text)) => Some(text.clone()),
                    Some(Value::Array(parts)) => {
                        Some(parts.iter().filter_map(|part| part["text"].as_str()).collect::<String>())
                    }
                    _ => None,
                }
                .filter(|text| !text.is_empty())
                .ok_or_else(|| {
                    ProviderError::Stream("remote compaction response missing choices[0].message.content".into())
                })?
            } else {
                value["summary"]
                    .as_str()
                    .ok_or_else(|| ProviderError::Stream("remote compaction response missing summary".into()))?
                    .to_owned()
            };
            Ok((summary, if chat { None } else { value["shortSummary"].as_str().map(str::to_owned) }))
        },
        &options,
    )
    .await;
    match result {
        Ok((summary, short_summary)) => Ok(RemoteResult {
            summary,
            short_summary,
            preserve_data: None,
            usage: usage.clone(),
            attempts: vec![RemoteAttempt {
                elapsed_ms: start.elapsed().as_millis() as u64,
                usage,
                error: None,
                status: None,
            }],
        }),
        Err(cause) => Err(error_with_attempt(cause, start.elapsed(), usage, &options)),
    }
}
