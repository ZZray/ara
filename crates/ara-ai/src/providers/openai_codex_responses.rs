//! Codex subscription Responses over SSE, backed by host-owned OAuth leases.
//!
//! Source: fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! `providers/openai-codex-responses.ts` (request, URL and SSE headers),
//! `providers/openai-codex/request-transformer.ts` and `catalog/src/wire/codex.ts`.
//! This entry supports text and function tools. WS, Lite, native compaction,
//! account selection and OAuth are outside this transport's scope.

use super::openai_completions::RetryPolicy;
pub use super::openai_responses::RequestOptions;
use super::openai_responses::{self, ResponsesProtocol};
use crate::error::ProviderError;
use crate::{
    AssistantBlock, AssistantStream, CallOptions, Context, Message, Model, ModelProvider, UserBlock, UserContent,
};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const API: &str = "openai-codex-responses";
pub const BASE_URL: &str = "https://chatgpt.com/backend-api";
pub const CLIENT_VERSION: &str = "0.153.0";
pub const ACCOUNT_HEADER: &str = "chatgpt-account-id";
pub const RESIDENCY_HEADER: &str = "x-openai-internal-codex-residency";
pub(crate) type PreparedRequest = (String, Value, Vec<(String, String)>);

/// Private authentication material is never Debug or Serialize.
#[derive(Clone)]
pub struct StreamOptions {
    pub request: RequestOptions,
    pub api_key: Option<String>,
    pub cancel: CancellationToken,
    pub first_event_timeout: Option<Duration>,
    pub idle_timeout: Option<Duration>,
    /// Account/residency headers come from the same host lease as `api_key`.
    pub extra_headers: Vec<(String, String)>,
    pub retry: RetryPolicy,
    /// Stable logical host Session identity, including after journal resume.
    pub session_id: Option<String>,
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
            session_id: None,
        }
    }
}

pub struct OpenAICodexResponsesProvider {
    pub client: reqwest::Client,
    pub base: StreamOptions,
}

impl ModelProvider for OpenAICodexResponsesProvider {
    fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> AssistantStream {
        let mut base = self.base.clone();
        base.cancel = options.cancel;
        base.request.tool_choice = options.tool_choice.or(base.request.tool_choice);
        base.request.max_tokens = options.max_tokens.or(base.request.max_tokens);
        base.request.temperature = options.temperature.or(base.request.temperature);
        stream(self.client.clone(), model.clone(), context.clone(), base)
    }
}

pub fn stream(client: reqwest::Client, model: Model, context: Context, options: StreamOptions) -> AssistantStream {
    let protocol = ResponsesProtocol::Codex { session_id: options.session_id };
    let shared = openai_responses::StreamOptions {
        request: options.request,
        api_key: options.api_key,
        cancel: options.cancel,
        first_event_timeout: options.first_event_timeout,
        idle_timeout: options.idle_timeout,
        extra_headers: options.extra_headers,
        retry: options.retry,
        // Stateless Codex sends full encrypted history on every request,
        // including the first call on a freshly resumed host Session.
        session_state: None,
        stateful_responses: false,
    };
    openai_responses::stream_with_protocol(client, model, context, shared, protocol)
}

pub fn resolve_responses_url(base_url: &str) -> String {
    let raw = if base_url.trim().is_empty() { BASE_URL } else { base_url };
    let base = raw.trim_end_matches('/');
    if base.ends_with("/codex/responses") {
        base.into()
    } else if base.ends_with("/codex") {
        format!("{base}/responses")
    } else {
        format!("{base}/codex/responses")
    }
}

/// Fields tied to auth or protocol identity cannot be ordinary config headers.
pub fn is_reserved_header(name: &str) -> bool {
    [
        "authorization",
        "proxy-authorization",
        "x-api-key",
        ACCOUNT_HEADER,
        RESIDENCY_HEADER,
        "openai-beta",
        "originator",
        "version",
        "session_id",
        "conversation_id",
        "x-client-request-id",
        "x-codex-routing-hint",
        "accept",
        "content-type",
        "user-agent",
        "x-codex-turn-state",
        "x-openai-internal-codex-responses-lite",
        "x-oai-attestation",
        "session-id",
        "thread-id",
        "x-codex-installation-id",
        "x-codex-window-id",
        "x-codex-turn-metadata",
        "x-models-etag",
    ]
    .iter()
    .any(|header| name.eq_ignore_ascii_case(header))
}

fn text_only(content: &UserContent) -> bool {
    !matches!(content, UserContent::Blocks(blocks) if blocks.iter().any(|block| matches!(block, UserBlock::Image(_))))
}

fn supported_native_item(item: &Value, history: bool) -> bool {
    let kind = item.get("type").and_then(Value::as_str);
    let supported = matches!(kind, Some("message" | "reasoning" | "function_call"))
        || history
            && (kind == Some("function_call_output")
                || kind.is_none() && item.get("role").is_some_and(Value::is_string));
    supported
        && item.get("content").and_then(Value::as_array).is_none_or(|parts| {
            parts.iter().all(|part| {
                matches!(
                    part.get("type").and_then(Value::as_str),
                    Some("input_text" | "output_text" | "refusal" | "reasoning_text")
                )
            })
        })
        && item
            .get("output")
            .and_then(Value::as_array)
            .is_none_or(|parts| parts.iter().all(|part| part.get("type").and_then(Value::as_str) == Some("input_text")))
}

pub(crate) fn validate_event(event: &Value) -> Result<(), ProviderError> {
    let supported = event.get("item").is_none_or(|item| supported_native_item(item, false))
        && event
            .pointer("/response/output")
            .and_then(Value::as_array)
            .is_none_or(|items| items.iter().all(|item| supported_native_item(item, false)))
        && event.get("part").is_none_or(|part| {
            matches!(
                part.get("type").and_then(Value::as_str),
                Some("output_text" | "refusal" | "summary_text" | "reasoning_text")
            )
        });
    if supported {
        Ok(())
    } else {
        Err(ProviderError::Stream("unsupported native Codex output item or content".into()))
    }
}

fn validate_history(model: &Model, context: &Context) -> Result<(), ProviderError> {
    for message in &context.messages {
        let supported = match message {
            Message::User(message) => text_only(&message.content),
            Message::Developer(message) => text_only(&message.content),
            Message::ToolResult(message) => message.content.iter().all(|block| matches!(block, UserBlock::Text(_))),
            Message::Assistant(message) => {
                if message
                    .content
                    .iter()
                    .any(|block| matches!(block, AssistantBlock::Image(_) | AssistantBlock::RedactedThinking { .. }))
                {
                    return Err(ProviderError::Config("Codex SSE supports only text and function tool history".into()));
                }
                if message.api == API
                    && message.provider == model.provider
                    && message.model == model.id
                    && let Some(items) = message
                        .provider_payload
                        .as_ref()
                        .and_then(|payload| payload.get("items"))
                        .and_then(Value::as_array)
                {
                    for item in items {
                        if !supported_native_item(item, true) {
                            return Err(ProviderError::Config("unsupported native Codex history item".into()));
                        }
                    }
                }
                true
            }
        };
        if !supported {
            return Err(ProviderError::Config("Codex SSE supports only text and function tools".into()));
        }
    }
    Ok(())
}

pub fn build_request(model: &Model, context: &Context, options: &RequestOptions) -> Result<Value, ProviderError> {
    if options.max_tokens.is_some() || options.temperature.is_some() {
        return Err(ProviderError::Config("Codex does not support output caps or temperature".into()));
    }
    if options.supports_images || options.native_history_replay == Some(false) {
        return Err(ProviderError::Config("Codex SSE requires text-only input and full native history replay".into()));
    }
    validate_history(model, context)?;
    let mut body = openai_responses::build_request_for_api(model, context, options, API)?;
    // Model.max_tokens describes model capacity. It is not a caller output cap.
    body.as_object_mut().expect("Responses request is an object").remove("max_output_tokens");
    body["include"] = json!(["reasoning.encrypted_content"]);
    let prompts = context.system_prompt.iter().filter(|prompt| !prompt.trim().is_empty()).collect::<Vec<_>>();
    body["instructions"] = json!(prompts.first().map(|prompt| prompt.as_str()).unwrap_or(""));
    if prompts.len() > 1 {
        let input = body["input"].as_array_mut().expect("Responses input is an array");
        let developer = prompts[1..]
            .iter()
            .map(|prompt| {
                json!({
                    "type": "message", "role": "developer", "content": [{"type": "input_text", "text": prompt}],
                })
            })
            .collect::<Vec<_>>();
        input.splice(0..0, developer);
    }
    Ok(body)
}

pub(crate) fn prepare_request(
    model: &Model,
    context: &Context,
    request: &RequestOptions,
    api_key: Option<&str>,
    extra_headers: &[(String, String)],
    session_id: Option<&str>,
) -> Result<PreparedRequest, ProviderError> {
    let mut body = build_request(model, context, request)?;
    let token = api_key
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| ProviderError::Config("Codex requires a host OAuth lease".into()))?;
    let accounts =
        extra_headers.iter().filter(|(name, _)| name.eq_ignore_ascii_case(ACCOUNT_HEADER)).collect::<Vec<_>>();
    if accounts.len() != 1 || accounts[0].1.trim().is_empty() {
        return Err(ProviderError::Config("Codex requires one account header from its host OAuth lease".into()));
    }
    for (name, _) in extra_headers {
        if is_reserved_header(name)
            && !name.eq_ignore_ascii_case(ACCOUNT_HEADER)
            && !name.eq_ignore_ascii_case(RESIDENCY_HEADER)
        {
            return Err(ProviderError::Config(format!("Codex protocol owns header {name}")));
        }
    }
    let mut headers = extra_headers.to_vec();
    headers.extend([
        ("Authorization".into(), format!("Bearer {token}")),
        ("OpenAI-Beta".into(), "responses=experimental".into()),
        ("originator".into(), "omp".into()),
        ("version".into(), CLIENT_VERSION.into()),
        ("x-codex-routing-hint".into(), format!("model={}", model.id)),
        ("accept".into(), "text/event-stream".into()),
        ("content-type".into(), "application/json".into()),
        ("user-agent".into(), concat!("ara/", env!("CARGO_PKG_VERSION")).into()),
    ]);
    if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
        body["prompt_cache_key"] = json!(request.prompt_cache_key.as_deref().unwrap_or(session_id));
        for name in ["session_id", "conversation_id", "x-client-request-id"] {
            headers.push((name.into(), session_id.into()));
        }
    }
    Ok((resolve_responses_url(&model.base_url), body, headers))
}
