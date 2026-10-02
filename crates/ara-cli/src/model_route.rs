//! Complete host route binding, with authentication resolved once per logical
//! model call. Fixed OMP `model-registry.ts::getApiKey` / `auth-storage.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d are the ownership contract.
//!
//! Provider wire retries share the acquired credential. Catalogue selection,
//! OAuth refresh, rotation and native receipt persistence remain host services;
//! this module does not implement or silently substitute those services.

use ara_ai::{
    AnthropicMessagesProvider, AssistantMessage, AssistantMessageEvent, AssistantStream, CallOptions, Context, Model,
    ModelProvider, OpenAICompletionsProvider, OpenAIResponsesProvider, StopReason,
};
use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Actual credential origin. Only a native store row has a database identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialIdentity {
    Runtime,
    Config { provider: String },
    Environment { variable: String },
    Stored { id: i64, revision: i64 },
    Keyless,
}

/// Private, owned authentication snapshot. Intentionally has no Debug/Serialize.
#[derive(Clone)]
pub struct RequestAuthLease {
    identity: CredentialIdentity,
    api_key: Option<String>,
    headers: Vec<(String, String)>,
}

impl RequestAuthLease {
    pub fn new(identity: CredentialIdentity, api_key: Option<String>) -> Self {
        Self { identity, api_key, headers: Vec::new() }
    }

    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.headers = headers;
        self
    }

    pub fn identity(&self) -> &CredentialIdentity {
        &self.identity
    }
}

/// Safe failure categories. Resolver diagnostics must retain secrets privately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthResolveError {
    Cancelled,
    Unavailable,
    Storage,
    Command,
    Refresh,
}

#[async_trait]
pub trait RequestAuthResolver: Send + Sync {
    /// Called for each logical model call, never for ordinary availability queries.
    async fn resolve(&self, model: &Model, cancel: &CancellationToken) -> Result<RequestAuthLease, AuthResolveError>;
}

/// An already authorized host override; this does not claim native auth precedence.
pub struct FixedRequestAuth {
    lease: RequestAuthLease,
}

impl FixedRequestAuth {
    pub fn new(lease: RequestAuthLease) -> Self {
        Self { lease }
    }
}

#[async_trait]
impl RequestAuthResolver for FixedRequestAuth {
    async fn resolve(&self, _: &Model, cancel: &CancellationToken) -> Result<RequestAuthLease, AuthResolveError> {
        if cancel.is_cancelled() { Err(AuthResolveError::Cancelled) } else { Ok(self.lease.clone()) }
    }
}

/// Protocol options are part of the target route, rather than startup globals.
#[derive(Clone)]
pub enum ProtocolOptions {
    Completions(ara_ai::providers::openai_completions::StreamOptions),
    Responses(ara_ai::providers::openai_responses::StreamOptions),
    CodexResponses(ara_ai::providers::openai_codex_responses::StreamOptions),
    Anthropic(ara_ai::providers::anthropic::StreamOptions),
}

impl ProtocolOptions {
    fn api(&self) -> &'static str {
        match self {
            Self::Completions(_) => "openai-completions",
            Self::Responses(_) => "openai-responses",
            Self::CodexResponses(_) => "openai-codex-responses",
            Self::Anthropic(_) => "anthropic-messages",
        }
    }

    fn has_static_key(&self) -> bool {
        match self {
            Self::Completions(options) => {
                options.api_key.is_some() || options.extra_headers.iter().any(|(name, _)| is_credential_header(name))
            }
            Self::Responses(options) => {
                options.api_key.is_some() || options.extra_headers.iter().any(|(name, _)| is_credential_header(name))
            }
            Self::CodexResponses(options) => {
                options.api_key.is_some()
                    || options.extra_headers.iter().any(|(name, _)| {
                        is_credential_header(name)
                            || name.eq_ignore_ascii_case("chatgpt-account-id")
                            || name.eq_ignore_ascii_case("x-openai-internal-codex-residency")
                    })
            }
            Self::Anthropic(options) => {
                options.api_key.is_some() || options.extra_headers.iter().any(|(name, _)| is_credential_header(name))
            }
        }
    }

    fn provider(&self, client: reqwest::Client, lease: RequestAuthLease) -> Arc<dyn ModelProvider> {
        match self {
            Self::Completions(options) => {
                let mut base = options.clone();
                base.api_key = lease.api_key;
                base.extra_headers.extend(lease.headers);
                Arc::new(OpenAICompletionsProvider { client, base })
            }
            Self::Responses(options) => {
                let mut base = options.clone();
                base.api_key = lease.api_key;
                base.extra_headers.extend(lease.headers);
                Arc::new(OpenAIResponsesProvider { client, base })
            }
            Self::CodexResponses(options) => {
                let mut base = options.clone();
                base.api_key = lease.api_key;
                base.extra_headers.extend(lease.headers);
                Arc::new(ara_ai::providers::openai_codex_responses::OpenAICodexResponsesProvider { client, base })
            }
            Self::Anthropic(options) => {
                let mut base = options.clone();
                base.api_key = lease.api_key;
                base.extra_headers.extend(lease.headers);
                Arc::new(AnthropicMessagesProvider { client, base })
            }
        }
    }

    fn fresh_session(&self) -> Self {
        let mut options = self.clone();
        match &mut options {
            Self::Completions(_) => {}
            Self::CodexResponses(_) => {}
            Self::Responses(options) => {
                options.session_state = Some(Arc::new(Default::default()));
            }
            Self::Anthropic(options) => {
                options.provider_session_state = Some(Arc::new(Default::default()));
            }
        }
        options
    }
}

/// Authentication overrides must share the identity and lifetime of the lease.
pub fn is_credential_header(name: &str) -> bool {
    ["authorization", "proxy-authorization", "x-api-key", "x-goog-api-key"]
        .iter()
        .any(|header| name.eq_ignore_ascii_case(header))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoutePrepareError {
    ProtocolMismatch,
    StaticCredential,
    ModelPolicy,
}

/// Resolve the fixed native identity and compatibility facts for this target.
fn resolved_loop_guard_metadata(model: &Model) -> Result<serde_json::Value, RoutePrepareError> {
    let spec = serde_json::json!({"provider":model.provider,"id":model.id,
        "api":model.api,"baseUrl":model.base_url,"reasoning":model.reasoning});
    crate::model_policy::resolve_model_policy(&spec).map_err(|_| RoutePrepareError::ModelPolicy)
}

/// Fixed OMP utils/thinking-loop.ts::isLoopGuardedModel uses defined compat
/// presence, including a defined false value, rather than its truthiness.
pub fn resolved_loop_guard_policy(model: &Model) -> Result<ara_ai::thinking_loop::LoopGuardPolicy, RoutePrepareError> {
    let metadata = resolved_loop_guard_metadata(model)?;
    let semantic_heuristics = match metadata.get("compat") {
        Some(compat) => compat.get("thinkingLoopGuard").is_some(),
        None => matches!(metadata["identity"]["class"].as_str(), Some("gemini" | "deepseek" | "xai")),
    };
    Ok(ara_ai::thinking_loop::LoopGuardPolicy { semantic_heuristics, ..Default::default() })
}

pub fn resolved_loop_guard_model_class(model: &Model) -> Result<String, RoutePrepareError> {
    Ok(resolved_loop_guard_metadata(model)?["identity"]["class"].as_str().unwrap_or("unknown").to_owned())
}

/// Fully prepared target. This is immutable for one provider/session binding.
#[derive(Clone)]
pub struct PreparedRoute {
    model: Model,
    protocol: ProtocolOptions,
    auth: Arc<dyn RequestAuthResolver>,
    generation: u64,
    loop_guard_policy: ara_ai::thinking_loop::LoopGuardPolicy,
}

impl PreparedRoute {
    pub fn new(
        model: Model,
        protocol: ProtocolOptions,
        auth: Arc<dyn RequestAuthResolver>,
        generation: u64,
    ) -> Result<Self, RoutePrepareError> {
        if model.api != protocol.api() {
            return Err(RoutePrepareError::ProtocolMismatch);
        }
        if protocol.has_static_key() {
            return Err(RoutePrepareError::StaticCredential);
        }
        let loop_guard_policy = resolved_loop_guard_policy(&model)?;
        Ok(Self { model, protocol, auth, generation, loop_guard_policy })
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    pub fn loop_guard_policy(&self) -> ara_ai::thinking_loop::LoopGuardPolicy {
        self.loop_guard_policy
    }

    pub fn with_loop_guard_policy(mut self, policy: ara_ai::thinking_loop::LoopGuardPolicy) -> Self {
        self.loop_guard_policy = policy;
        self
    }

    /// Host promotion checks account availability before publishing a route.
    /// The lease stays private; every later logical call still resolves afresh.
    pub async fn check_auth(&self, cancel: &CancellationToken) -> Result<(), AuthResolveError> {
        self.auth.resolve(&self.model, cancel).await.map(|_| ())
    }

    /// Allocate protocol session state once. Every call on this binding shares it;
    /// a new logical Session or adopted route gets independent state.
    pub fn bind(
        &self,
        client: reqwest::Client,
        observer: Option<Arc<dyn RequestReceiptObserver>>,
    ) -> Arc<dyn ModelProvider> {
        let mut route = self.clone();
        route.protocol = route.protocol.fresh_session();
        Arc::new(AuthenticatedRouteProvider { client, route, observer })
    }

    /// Rebind Codex attribution to a new Host Session, retaining the same
    /// private account resolver and its outstanding refresh settlement.
    pub fn bind_codex_session(&self, client: reqwest::Client, session_id: String) -> Arc<dyn ModelProvider> {
        let mut route = self.clone();
        if let ProtocolOptions::CodexResponses(options) = &mut route.protocol {
            options.session_id = Some(session_id);
        }
        route.bind(client, None)
    }

    /// A handoff reads the live cache prefix on independent protocol state.
    /// Keep the original resolver; only its transport Session is a side identity.
    pub fn bind_side_request(&self, client: reqwest::Client, session_id: &str) -> Arc<dyn ModelProvider> {
        let mut route = self.clone();
        if let ProtocolOptions::CodexResponses(options) = &mut route.protocol {
            options.request.prompt_cache_key =
                options.request.prompt_cache_key.clone().or_else(|| Some(session_id.to_owned()));
            options.session_id = Some(format!("{session_id}:side:{}", uuid::Uuid::now_v7()));
        }
        route.bind(client, None)
    }
}

/// Private attribution receipt, deliberately excluded from public events/journal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestIdentity {
    pub call_id: uuid::Uuid,
    pub route_generation: u64,
    pub provider: String,
    pub model_id: String,
    pub credential: CredentialIdentity,
}

#[derive(Clone, Copy, Debug)]
pub struct ReceiptError;

pub trait RequestReceiptObserver: Send + Sync {
    /// A failure here prevents the external request.
    fn started(&self, request: &RequestIdentity) -> Result<(), ReceiptError>;
    /// Recorded before publishing a model terminal to the Agent.
    fn settled(&self, request: &RequestIdentity, message: &AssistantMessage) -> Result<(), ReceiptError>;
    /// The receiver disappeared without a terminal proof. No success is implied.
    fn interrupted(&self, request: &RequestIdentity);
}

struct AuthenticatedRouteProvider {
    client: reqwest::Client,
    route: PreparedRoute,
    observer: Option<Arc<dyn RequestReceiptObserver>>,
}

fn error_event(model: &Model, reason: StopReason, text: &str) -> AssistantMessageEvent {
    let mut error = AssistantMessage::empty(&model.api, &model.provider, &model.id);
    error.stop_reason = reason;
    error.error_message = Some(text.into());
    AssistantMessageEvent::Error { reason, error }
}

impl ModelProvider for AuthenticatedRouteProvider {
    fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> AssistantStream {
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let route = self.route.clone();
        let client = self.client.clone();
        let model = model.clone();
        let context = context.clone();
        let observer = self.observer.clone();
        // Child cancellation stops this request if its consumer leaves, without
        // cancelling the entire parent Session/Run token.
        let cancel = options.cancel.child_token();
        tokio::spawn(async move {
            if model != route.model {
                let _ = tx
                    .send(error_event(&model, StopReason::Error, "model does not match the prepared host route"))
                    .await;
                return;
            }
            let resolving = route.auth.resolve(&model, &cancel);
            tokio::pin!(resolving);
            let lease = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    // An OAuth refresh may already have rotated a grant. Let
                    // this account resolver settle its durable row before the
                    // CLI can finish and shut down its runtime.
                    if model.api == "openai-codex-responses" { let _ = resolving.await; }
                    Err(AuthResolveError::Cancelled)
                },
                _ = tx.closed() => {
                    if model.api == "openai-codex-responses" {
                        cancel.cancel();
                        let _ = resolving.await;
                    }
                    return;
                },
                result = &mut resolving => result,
            };
            let lease = match lease {
                Ok(lease) => lease,
                Err(error) => {
                    let reason =
                        if error == AuthResolveError::Cancelled { StopReason::Aborted } else { StopReason::Error };
                    let text = match error {
                        AuthResolveError::Cancelled => "authentication cancelled",
                        AuthResolveError::Unavailable if model.api == "openai-codex-responses" => {
                            "OpenAI account is unavailable; run ara login"
                        }
                        AuthResolveError::Refresh if model.api == "openai-codex-responses" => {
                            "OpenAI account refresh failed; run ara login again"
                        }
                        AuthResolveError::Unavailable => "no configured authentication for this route",
                        AuthResolveError::Storage => "authentication storage failed",
                        AuthResolveError::Command => "authentication command failed",
                        AuthResolveError::Refresh => "authentication refresh failed",
                    };
                    let _ = tx.send(error_event(&model, reason, text)).await;
                    return;
                }
            };
            if cancel.is_cancelled() {
                let _ = tx.send(error_event(&model, StopReason::Aborted, "authentication cancelled")).await;
                return;
            }
            let identity = RequestIdentity {
                call_id: uuid::Uuid::now_v7(),
                route_generation: route.generation,
                provider: model.provider.clone(),
                model_id: model.id.clone(),
                credential: lease.identity.clone(),
            };
            if observer.as_ref().is_some_and(|observer| observer.started(&identity).is_err()) {
                let _ = tx.send(error_event(&model, StopReason::Error, "request attribution persistence failed")).await;
                return;
            }
            let provider = route.protocol.provider(client, lease);
            let mut options = options;
            options.cancel = cancel.clone();
            let mut inner =
                ara_ai::thinking_loop::with_thinking_loop_guard(&model, options, route.loop_guard_policy, |options| {
                    provider.stream(&model, &context, options)
                });
            loop {
                let event = tokio::select! {
                    event = inner.recv() => event,
                    _ = tx.closed() => {
                        cancel.cancel();
                        if let Some(observer) = &observer { observer.interrupted(&identity); }
                        return;
                    }
                };
                let Some(event) = event else {
                    if let Some(observer) = &observer {
                        observer.interrupted(&identity);
                    }
                    let _ = tx
                        .send(error_event(&model, StopReason::Error, "provider ended without a terminal receipt"))
                        .await;
                    return;
                };
                let terminal = event.is_terminal();
                if terminal
                    && observer.as_ref().is_some_and(|observer| observer.settled(&identity, event.partial()).is_err())
                {
                    cancel.cancel();
                    // Retain produced content, tool calls and observed usage as
                    // evidence. A failed Host receipt does not erase the actual
                    // provider terminal or authorize replay of its effects.
                    let mut error = event.partial().clone();
                    error.stop_reason = StopReason::Error;
                    error.error_message = Some(match error.error_message.take() {
                        Some(provider_error) => {
                            format!("request settlement persistence failed; provider error: {provider_error}")
                        }
                        None => "request settlement persistence failed".into(),
                    });
                    let failure = error.failure_evidence.get_or_insert_with(|| {
                        ara_ai::retry_classification::ProviderFailureEvidence::from_error(
                            &ara_ai::ProviderError::Config("request settlement persistence failed".into()),
                            true,
                        )
                    });
                    failure.replay_blocked = true;
                    failure.same_route_blocked = true;
                    let _ = tx.send(AssistantMessageEvent::Error { reason: StopReason::Error, error }).await;
                    return;
                }
                if tx.send(event).await.is_err() {
                    cancel.cancel();
                    if !terminal && let Some(observer) = &observer {
                        observer.interrupted(&identity);
                    }
                    return;
                }
                if terminal {
                    return;
                }
            }
        });
        rx
    }
}
