//! Fixed OMP remote-compaction selection and host-independent transport port.
//!
//! Source: OMP 596f2da, packages/agent/src/compaction/compaction.ts (MIT;
//! see THIRD_PARTY_NOTICES.md). Hosts own credentials and persistence. Native
//! success preserves opaque history; a generic summary is a different result.

use ara_ai::remote_compaction::{
    RemoteConfig, RemoteError, RemoteGenericRequest, RemoteNativeRequest, RemoteResult, RemoteVersion, should_use_v1,
    should_use_v2,
};
use ara_ai::{
    AssistantMessage, AssistantMessageEvent, AssistantStream, CallOptions, Context, Model, ModelProvider,
    ProviderError, StopReason,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// One logical invocation uses the host's original route and auth resolver.
/// Protocol retries remain inside the wire helper; this port never runs tools.
#[async_trait]
pub trait RemoteCompactionTransport: Send + Sync {
    async fn native(
        &self,
        model: &Model,
        context: &Context,
        request: RemoteNativeRequest,
        cancel: &CancellationToken,
    ) -> Result<RemoteResult, RemoteError>;

    async fn generic(
        &self,
        model: &Model,
        endpoint: &str,
        request: RemoteGenericRequest,
        cancel: &CancellationToken,
    ) -> Result<RemoteResult, RemoteError>;
}

#[derive(Clone, Debug)]
pub struct RemoteSettings {
    pub enabled: bool,
    pub streaming_v2_enabled: bool,
    pub endpoint: Option<String>,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        Self { enabled: true, streaming_v2_enabled: true, endpoint: None }
    }
}

pub fn native_available(model: &Model, config: &RemoteConfig, settings: &RemoteSettings) -> bool {
    settings.enabled && (should_use_v1(model, config) || settings.streaming_v2_enabled && should_use_v2(model, config))
}

pub fn remote_available(model: &Model, config: &RemoteConfig, settings: &RemoteSettings) -> bool {
    settings.enabled
        && (settings.endpoint.as_ref().is_some_and(|endpoint| !endpoint.is_empty())
            || native_available(model, config, settings))
}

pub fn cancelled(error: &RemoteError) -> bool {
    matches!(error.cause, ProviderError::Aborted)
}

fn abort(attempts: Vec<ara_ai::remote_compaction::RemoteAttempt>) -> RemoteError {
    RemoteError { cause: ProviderError::Aborted, attempts, auth_failed: false }
}

/// Try V2 and then V1. Retain an earlier non-auth failure so a later auth
/// failure cannot falsely classify the complete operation as an auth failure.
/// Generic endpoints are selected explicitly by the caller after this result;
/// native failure alone does not authorize a local summary invocation.
#[allow(clippy::too_many_arguments)]
pub async fn compact_provider_native(
    transport: &dyn RemoteCompactionTransport,
    model: &Model,
    context: &Context,
    config: &RemoteConfig,
    settings: &RemoteSettings,
    instructions: &str,
    previous_replacement_history: Option<&[Value]>,
    cancel: &CancellationToken,
) -> Result<Option<RemoteResult>, RemoteError> {
    if cancel.is_cancelled() {
        return Err(abort(Vec::new()));
    }
    if !native_available(model, config, settings)
        || context.messages.is_empty() && previous_replacement_history.is_none_or(|items| items.is_empty())
    {
        return Ok(None);
    }
    let mut selected_error: Option<RemoteError> = None;
    let mut attempts = Vec::new();
    for version in [RemoteVersion::V2, RemoteVersion::V1] {
        let available = match version {
            RemoteVersion::V2 => settings.streaming_v2_enabled && should_use_v2(model, config),
            RemoteVersion::V1 => should_use_v1(model, config),
        };
        if !available {
            continue;
        }
        if cancel.is_cancelled() {
            return Err(abort(attempts));
        }
        let request = RemoteNativeRequest {
            config: config.clone(),
            version,
            instructions: instructions.to_owned(),
            previous_replacement_history: previous_replacement_history.map(<[Value]>::to_vec),
        };
        match transport.native(model, context, request, cancel).await {
            Ok(mut result) => {
                attempts.append(&mut result.attempts);
                if cancel.is_cancelled() {
                    return Err(abort(attempts));
                }
                if result.preserve_data.is_none() {
                    return Err(RemoteError {
                        cause: ProviderError::Incomplete("Remote native result has no replay data".into()),
                        attempts,
                        auth_failed: false,
                    });
                }
                result.attempts = attempts;
                return Ok(Some(result));
            }
            Err(mut error) => {
                attempts.append(&mut error.attempts);
                if cancel.is_cancelled() || cancelled(&error) {
                    return Err(abort(attempts));
                }
                if selected_error.as_ref().is_none_or(|previous| previous.auth_failed) {
                    selected_error = Some(error);
                }
            }
        }
    }
    match selected_error {
        Some(mut error) => {
            error.attempts = attempts;
            Err(error)
        }
        None => Ok(None),
    }
}

/// Reuse the existing native summary renderer/window/split pipeline with an
/// explicitly configured generic HTTP summarizer. Its response supplies text,
/// never an Agent tool call or provider-native encrypted replay.
pub struct RemoteSummaryProvider {
    transport: Arc<dyn RemoteCompactionTransport>,
    endpoint: String,
    attempts: Arc<Mutex<Vec<ara_ai::remote_compaction::RemoteAttempt>>>,
    pending: Arc<AtomicUsize>,
    settled: Arc<tokio::sync::Notify>,
}

impl RemoteSummaryProvider {
    pub fn new(transport: Arc<dyn RemoteCompactionTransport>, endpoint: String) -> Self {
        Self {
            transport,
            endpoint,
            attempts: Arc::new(Mutex::new(Vec::new())),
            pending: Arc::new(AtomicUsize::new(0)),
            settled: Arc::new(tokio::sync::Notify::new()),
        }
    }

    pub fn attempts(&self) -> Vec<ara_ai::remote_compaction::RemoteAttempt> {
        self.attempts.lock().unwrap().clone()
    }

    pub async fn settle(&self) {
        loop {
            let changed = self.settled.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.pending.load(Ordering::Acquire) == 0 {
                return;
            }
            changed.await;
        }
    }
}

struct PendingSummary {
    pending: Arc<AtomicUsize>,
    settled: Arc<tokio::sync::Notify>,
}
impl Drop for PendingSummary {
    fn drop(&mut self) {
        self.pending.fetch_sub(1, Ordering::Release);
        self.settled.notify_waiters();
    }
}

impl ModelProvider for RemoteSummaryProvider {
    fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> AssistantStream {
        let (sender, receiver) = tokio::sync::mpsc::channel(4);
        let transport = self.transport.clone();
        let endpoint = self.endpoint.clone();
        let attempts = self.attempts.clone();
        self.pending.fetch_add(1, Ordering::Release);
        let pending = PendingSummary { pending: self.pending.clone(), settled: self.settled.clone() };
        let model = model.clone();
        let request = RemoteGenericRequest {
            system_prompt: context.system_prompt.join("\n\n"),
            prompt: context
                .messages
                .iter()
                .map(|message| match message {
                    ara_ai::Message::User(user) => user.content.plain_text(),
                    ara_ai::Message::Developer(developer) => developer.content.plain_text(),
                    ara_ai::Message::Assistant(assistant) => assistant.text(),
                    ara_ai::Message::ToolResult(result) => result
                        .content
                        .iter()
                        .filter_map(|block| {
                            if let ara_ai::UserBlock::Text(text) = block { Some(text.text.as_str()) } else { None }
                        })
                        .collect::<String>(),
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
            max_tokens: options.max_tokens,
        };
        let cancel = options.cancel.child_token();
        tokio::spawn(async move {
            let _pending = pending;
            let call = transport.generic(&model, &endpoint, request, &cancel);
            tokio::pin!(call);
            let result = tokio::select! {
                biased;
                _ = sender.closed() => { cancel.cancel(); call.await }
                result = &mut call => result,
            };
            attempts.lock().unwrap().extend(match &result {
                Ok(result) => result.attempts.clone(),
                Err(error) => error.attempts.clone(),
            });
            let mut message = AssistantMessage::empty(&model.api, &model.provider, &model.id);
            let event = match result {
                Ok(result) if !cancel.is_cancelled() => {
                    message.content.push(ara_ai::AssistantBlock::text(result.summary));
                    message.usage = result.usage;
                    message.stop_reason = StopReason::Stop;
                    AssistantMessageEvent::Done { reason: StopReason::Stop, message }
                }
                result => {
                    let cause = match result {
                        Ok(_) => ProviderError::Aborted,
                        Err(error) => {
                            // Preserve known failed-attempt usage. Missing
                            // buckets stay unknown instead of becoming zero.
                            if let Some(last) = error.attempts.last() {
                                message.usage = last.usage.clone();
                            }
                            error.cause
                        }
                    };
                    message.stop_reason = cause.stop_reason();
                    message.error_status = cause.status();
                    message.error_message = Some(cause.to_string());
                    AssistantMessageEvent::Error { reason: message.stop_reason, error: message }
                }
            };
            let _ = sender.send(event).await;
        });
        receiver
    }
}
