//! Streaming remote compaction, fixed OMP
//! `packages/agent/src/compaction/compaction-v2-streaming.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT).
//! SSE shares the Responses decoder. WS, Lite and Codex attestation metadata
//! remain explicit unported transport capabilities.

use crate::remote_compaction::{self as remote, RemoteAttempt, RemoteError, RemoteRequestOptions, RemoteResult};
use crate::{Model, ProviderError, Usage};
use futures::StreamExt;
use serde_json::{Value, json};
use std::time::Instant;

pub const RETAINED_MESSAGE_TOKEN_BUDGET: u64 = 64_000;
pub const MAX_RETRIES: usize = 2;
const IMAGE_TOKENS: u64 = 765;
const CONTEXTUAL_PREFIXES: &[&str] = &[
    "<environment_context>",
    "<user_instructions>",
    "<additional_context>",
    "<skills",
    "<token_budget>",
    "<model_switch>",
];

#[derive(Default)]
struct Collection {
    compaction_items: Vec<Value>,
    output_item_count: usize,
    completed: bool,
    usage: Usage,
}

impl Collection {
    fn event(&mut self, frame: crate::sse::SseEvent) -> Result<(), ProviderError> {
        if frame.data == "[DONE]" {
            return Ok(());
        }
        let value: Value = serde_json::from_str(&frame.data)
            .map_err(|_| ProviderError::Stream("V2 compaction stream parse failed".into()))?;
        let event_type = value["type"].as_str().or(frame.event.as_deref()).unwrap_or("");
        match event_type {
            "response.output_item.done" => {
                self.output_item_count = self.output_item_count.saturating_add(1);
                if value["item"]["type"] == "compaction" {
                    self.compaction_items.push(value["item"].clone());
                }
            }
            "response.completed" | "response.done" => {
                self.completed = true;
                self.usage = remote::parse_usage(&value["response"]);
            }
            "response.failed" | "response.incomplete" | "error" => {
                self.usage = remote::parse_usage(&value["response"]);
                let error = value.get("error").or_else(|| value.pointer("/response/error"));
                let code = error
                    .and_then(|error| error.get("code").or(error.get("type")))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let message = error.and_then(|error| error.get("message")).and_then(Value::as_str).unwrap_or("");
                return Err(ProviderError::Stream(format!("V2 compaction stream {event_type} ({code}): {message}")));
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(&self) -> Result<Value, ProviderError> {
        if !self.completed {
            return Err(ProviderError::Incomplete("V2 compaction stream closed before response.completed".into()));
        }
        if self.compaction_items.len() != 1 {
            return Err(ProviderError::Stream(format!(
                "V2 compaction expected exactly one compaction output item, got {} from {} output items",
                self.compaction_items.len(),
                self.output_item_count
            )));
        }
        let item = &self.compaction_items[0];
        if !remote::valid_compaction_item(item) {
            return Err(ProviderError::Stream("V2 compaction output lacks encrypted_content".into()));
        }
        Ok(item.clone())
    }
}

async fn attempt(
    client: &reqwest::Client,
    endpoint: &str,
    body: &Value,
    headers: &[(String, String)],
    collection: &mut Collection,
) -> Result<Value, ProviderError> {
    let response = remote::post(client, endpoint, body, headers).await?;
    let mut stream = response.bytes_stream();
    let mut decoder = crate::responses_sse::ResponsesSseDecoder::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ProviderError::Transport("V2 compaction stream read failed".into()))?;
        let frames =
            decoder.feed(&chunk).map_err(|_| ProviderError::Stream("V2 compaction stream parse failed".into()))?;
        for frame in frames {
            collection.event(frame)?;
        }
    }
    if let Some(frame) =
        decoder.finish().map_err(|_| ProviderError::Stream("V2 compaction stream parse failed".into()))?
    {
        collection.event(frame)?;
    }
    collection.finish()
}

fn retryable(cause: &ProviderError) -> bool {
    if remote::auth_failure(cause) || matches!(cause, ProviderError::Aborted) {
        return false;
    }
    if cause.is_retryable() {
        return true;
    }
    if let ProviderError::Stream(message) = cause {
        let lower = message.to_ascii_lowercase();
        return [
            "stream parse failed",
            "server_error",
            "internal_error",
            "overloaded",
            "service unavailable",
            "timeout",
        ]
        .iter()
        .any(|text| lower.contains(text));
    }
    false
}

pub(crate) async fn request_v2(
    client: &reqwest::Client,
    model: &Model,
    endpoint: &str,
    body: &Value,
    headers: &[(String, String)],
    options: &RemoteRequestOptions,
) -> Result<RemoteResult, RemoteError> {
    let mut attempts = Vec::new();
    for index in 0..=MAX_RETRIES {
        let start = Instant::now();
        let mut collection = Collection::default();
        let result = remote::with_watchdog(
            async {
                let compaction = attempt(client, endpoint, body, headers, &mut collection).await?;
                let input = body["input"].as_array().expect("V2 input array");
                let (history, _) = build_replacement_history(input, &compaction, options.retained_message_budget);
                remote::validate_replacement_for_route(model, &history, options.supports_images)?;
                Ok(compaction)
            },
            options,
        )
        .await;
        match result {
            Ok(compaction_item) => {
                let input = body["input"].as_array().expect("V2 input array");
                let (history, images) =
                    build_replacement_history(input, &compaction_item, options.retained_message_budget);
                let mut slot = json!({"version":"v2","provider":model.provider,"replacementHistory":history,"retainedImageCount":images});
                if let Some(input) = collection.usage.input {
                    slot["usedTokens"] = json!(input);
                }
                // The durable V2 usage shape is the upstream source shape.
                let mut usage = serde_json::Map::new();
                for (key, count) in [
                    ("inputTokens", collection.usage.input),
                    ("outputTokens", collection.usage.output),
                    ("totalTokens", collection.usage.total_tokens),
                    ("cachedInputTokens", collection.usage.cache_read),
                    ("reasoningOutputTokens", collection.usage.reasoning_tokens),
                ] {
                    if let Some(count) = count {
                        usage.insert(key.into(), json!(count));
                    }
                }
                if !usage.is_empty() {
                    slot["usage"] = Value::Object(usage);
                }
                attempts.push(RemoteAttempt {
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    usage: collection.usage.clone(),
                    error: None,
                    status: None,
                });
                return Ok(RemoteResult {
                    summary: remote::remote_summary(collection.usage.input),
                    short_summary: None,
                    preserve_data: Some(json!({remote::PRESERVE_KEY:slot})),
                    usage: collection.usage,
                    attempts,
                });
            }
            Err(cause) => {
                let auth_failed = remote::auth_failure(&cause);
                let retry = retryable(&cause) && index < MAX_RETRIES && !options.cancel.is_cancelled();
                let cause = remote::private_safe_error(cause, options);
                attempts.push(RemoteAttempt {
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    usage: collection.usage,
                    error: Some(cause.to_string()),
                    status: cause.status(),
                });
                if !retry {
                    return Err(RemoteError { cause, attempts, auth_failed });
                }
                let delay = options.retry_delay.saturating_mul(1 << index);
                tokio::select! {biased;
                    _ = options.cancel.cancelled() => return Err(RemoteError {cause:ProviderError::Aborted,attempts,auth_failed:false}),
                    _ = tokio::time::sleep(delay) => {}
                }
                if options.cancel.is_cancelled() {
                    return Err(RemoteError { cause: ProviderError::Aborted, attempts, auth_failed: false });
                }
            }
        }
    }
    unreachable!("bounded V2 attempts return a result")
}

pub fn resolve_retained_budget(value: u64) -> u64 {
    value.clamp(1, RETAINED_MESSAGE_TOKEN_BUDGET)
}

fn retained(item: &Value) -> bool {
    item.is_object()
        && item["type"] == "message"
        && item["role"] == "user"
        && !item["content"].as_array().is_some_and(|parts| {
            parts.iter().any(|part| {
                part["type"] == "input_text"
                    && part["text"].as_str().is_some_and(|text| {
                        let text = text.trim_start().to_lowercase();
                        CONTEXTUAL_PREFIXES.iter().any(|prefix| text.starts_with(prefix))
                    })
            })
        })
}

fn approx_tokens(text: &str) -> u64 {
    (text.encode_utf16().count() as u64).saturating_add(3) / 4
}

fn content_tokens(item: &Value) -> u64 {
    item["content"].as_array().map_or(0, |parts| {
        parts.iter().fold(0u64, |count, part| {
            count.saturating_add(match part["type"].as_str() {
                Some("input_image") => IMAGE_TOKENS,
                Some("input_text" | "output_text") => approx_tokens(part["text"].as_str().unwrap_or("")),
                _ => 0,
            })
        })
    })
}

fn prefix_utf16(text: &str, max: u64) -> String {
    let mut remaining = max;
    text.chars()
        .take_while(|ch| {
            let units = ch.len_utf16() as u64;
            if units > remaining {
                false
            } else {
                remaining -= units;
                true
            }
        })
        .collect()
}

fn suffix_utf16(text: &str, max: u64) -> String {
    let mut remaining = max;
    let mut chars: Vec<_> = text
        .chars()
        .rev()
        .take_while(|ch| {
            let units = ch.len_utf16() as u64;
            if units > remaining {
                false
            } else {
                remaining -= units;
                true
            }
        })
        .collect();
    chars.reverse();
    chars.into_iter().collect()
}

fn truncate_text(text: &str, budget: u64) -> String {
    let max_chars = budget.saturating_mul(4);
    if text.encode_utf16().count() as u64 <= max_chars {
        return text.into();
    }
    let omitted = approx_tokens(text).saturating_sub(budget).max(1);
    let marker = format!("…{omitted} tokens truncated…");
    let marker_len = marker.encode_utf16().count() as u64;
    if max_chars <= marker_len.saturating_add(2) {
        return prefix_utf16(text, max_chars);
    }
    let side = ((max_chars - marker_len) / 2).max(1);
    format!("{}{}{}", prefix_utf16(text, side), marker, suffix_utf16(text, side))
}

fn truncate_message(item: &Value, budget: u64) -> Option<Value> {
    let mut remaining = budget;
    let mut parts = Vec::new();
    for part in item["content"].as_array()? {
        match part["type"].as_str() {
            Some("input_image") if remaining >= IMAGE_TOKENS => {
                parts.push(part.clone());
                remaining -= IMAGE_TOKENS;
            }
            Some("input_text" | "output_text") if remaining > 0 => {
                let text = part["text"].as_str().unwrap_or("");
                let tokens = approx_tokens(text);
                if tokens <= remaining {
                    parts.push(part.clone());
                    remaining -= tokens;
                } else {
                    let text = truncate_text(text, remaining);
                    remaining = 0;
                    if !text.is_empty() {
                        let mut part = part.clone();
                        part["text"] = json!(text);
                        parts.push(part);
                    }
                }
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        return None;
    }
    let mut item = item.clone();
    item["content"] = json!(parts);
    Some(item)
}

/// Keep newest real user messages under the fixed approximate 64K ceiling.
/// Contextual user blocks, system/developer and assistant history are omitted.
pub fn build_replacement_history(input: &[Value], compaction: &Value, budget: u64) -> (Vec<Value>, u64) {
    let mut remaining = resolve_retained_budget(budget);
    let mut history = Vec::new();
    for item in input.iter().rev().filter(|item| retained(item)) {
        if remaining == 0 {
            continue;
        }
        let count = content_tokens(item).max(1);
        if count <= remaining {
            history.push(item.clone());
            remaining -= count;
        } else if let Some(item) = truncate_message(item, remaining) {
            history.push(item);
            remaining = 0;
        }
    }
    history.reverse();
    let images = history
        .iter()
        .filter_map(|item| item["content"].as_array())
        .flatten()
        .filter(|part| part["type"] == "input_image")
        .count() as u64;
    history.push(compaction.clone());
    (history, images)
}
