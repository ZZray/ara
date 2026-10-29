//! Host driver for fixed OMP auth-retry.ts and error/auth-classify.ts at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; THIRD_PARTY_NOTICES.md).
//! Ordinary wire retry keeps its acquired lease. Only an observed rejection
//! before stream output enters this separate refresh/rotation sequence.

use crate::model_route::{
    AuthResolveError, AuthRetryAction, CredentialIdentity, RequestAuthFailure, RequestAuthFailureKind as Kind,
    RequestAuthLease, RequestAuthResolver,
};
use ara_ai::{
    AssistantMessage, Model, StopReason,
    graph_values::GraphValueFactory,
    retry_classification::{self, ProviderErrorKind, ProviderFailureEvidence, RetryFacts, flag},
};
use ara_rpc::value_heap::{ValueError, ValueHandle, ValueSnapshot};
use regex::Regex;
use std::{collections::BTreeSet, sync::LazyLock};
use tokio_util::sync::CancellationToken;

const MAX_ATTEMPTS: usize = 64;
static CONCURRENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?i)\btoo many\s+concurren\w*\s+(?:requests?|invocations?)\b|\bconcurren\w*\b[^\n]{0,60}\b(?:limit|quota|exceed\w*|reach\w*)\b|\b(?:limit|quota|exceed\w*|reach\w*)\b[^\n]{0,60}\bconcurren\w*\b|\bconcurren[a-z]*[-_](?:[a-z]+[_-])*(?:limit|quota|exceed\w*|reach\w*)"
).expect("fixed native concurrency exclusion")
});
static INVALIDATED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\binvalidated oauth token\b").unwrap());
static CODEX_MODEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\bThe ['"]([^'"\r\n]+)['"] model is not supported when using Codex with a ChatGPT account\."#)
        .unwrap()
});
static CURSOR_MARKER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bERROR_RATE_LIMITED_CHANGEABLE\b").unwrap());
static CURSOR_POLICY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bNamed models unavailable\b|\bModel unavailable on\b|\bFree plans can only use\b").unwrap()
});

fn bare_model(value: &str) -> Option<String> {
    let bare = value.rsplit('/').next()?.trim().to_lowercase();
    (!bare.is_empty() && bare.encode_utf16().count() <= 256 && !bare.contains('\0')).then_some(bare)
}

pub(crate) fn classify(model: &Model, message: &AssistantMessage) -> Option<RequestAuthFailure> {
    if message.stop_reason != StopReason::Error || !message.content.is_empty() {
        return None;
    }
    classify_facts(
        model,
        RetryFacts {
            stop_reason: message.stop_reason,
            error_message: message.error_message.as_deref(),
            error_status: message.error_status,
            failure_evidence: message.failure_evidence.as_ref(),
        },
    )
}

fn classify_facts(model: &Model, facts: RetryFacts<'_>) -> Option<RequestAuthFailure> {
    // Transport, partial stream and unknown dispatch outcomes cannot establish
    // a credential rejection. Never convert their text into replay authority.
    let evidence = facts.failure_evidence?;
    if evidence.kind != ProviderErrorKind::Http {
        return None;
    }
    let status = evidence.status.or(facts.error_status);
    let text = facts.error_message.unwrap_or("");
    let classification = retry_classification::classify_retry_facts(facts, &model.api);
    let denied =
        CODEX_MODEL.captures(text).and_then(|captures| captures.get(1)).and_then(|model| bare_model(model.as_str()));
    let kind = if let Some(denied) = denied {
        if model.provider != "openai-codex" || Some(denied) != bare_model(&model.id) {
            return None;
        }
        Kind::ModelAccountPolicy
    } else if model.provider == "cursor" && CURSOR_MARKER.is_match(text) && CURSOR_POLICY.is_match(text) {
        Kind::ModelAccountPolicy
    } else if classification.error_id & flag::ACCOUNT_POLICY != 0 {
        Kind::AccountPolicy
    } else if classification.usage_limit {
        Kind::Quota
    } else if INVALIDATED.is_match(text) {
        Kind::InvalidatedOAuth
    } else {
        if status.is_some_and(|status| matches!(status, 401 | 403 | 429)) && CONCURRENT.is_match(text) {
            return None;
        }
        if text.to_lowercase().contains("only available via cline product surfaces") {
            return None;
        }
        if evidence.replay_blocked {
            return None;
        }
        match status {
            Some(401) => Kind::Auth,
            Some(403) => Kind::Forbidden,
            _ => return None,
        }
    };
    // A real admission rejection blocks the adapter's ordinary replay but may
    // enter the Host's correlated reset path. Other blocked evidence (including
    // callback/receipt faults) must never acquire that exception from text.
    if kind == Kind::Quota
        && (evidence.replay_blocked || evidence.same_route_blocked)
        && evidence.context_recovery != Some(ara_ai::ContextRecoveryEvidence::UsageAdmission)
    {
        return None;
    }
    Some(RequestAuthFailure { kind, retry_after_ms: classification.wait_ms })
}

/// Read-only classification facts, never a replacement message or transcript.
/// All Gets run against the original lineage and preserve Native exceptions.
pub(crate) struct ActualFailureFacts {
    error_message: Option<String>,
    error_status: Option<u16>,
    pub(crate) evidence: ProviderFailureEvidence,
}
impl ActualFailureFacts {
    pub(crate) fn metadata(&self, model: &Model) -> crate::model_route::ActualFailureMetadata {
        crate::model_route::ActualFailureMetadata {
            class: self.classify(model),
            error_message: self.error_message.clone(),
            context_recovery: self.evidence.context_recovery,
        }
    }

    fn facts(&self) -> RetryFacts<'_> {
        RetryFacts {
            stop_reason: StopReason::Error,
            error_message: self.error_message.as_deref(),
            error_status: self.error_status,
            failure_evidence: Some(&self.evidence),
        }
    }
    pub(crate) fn classify(&self, model: &Model) -> retry_classification::RetryClass {
        retry_classification::classify_retry_facts(self.facts(), &model.api)
    }
    pub(crate) fn auth_failure(
        &self,
        model: &Model,
        message: &ValueHandle,
        factory: &GraphValueFactory,
    ) -> Result<Option<RequestAuthFailure>, ValueError> {
        let content = actual_get(factory, message, "content")?;
        let length = actual_get(factory, &content, "length")?;
        if !matches!(factory.heap().snapshot(&length)?, ValueSnapshot::Number(value) if value == 0.0) {
            return Ok(None);
        }
        Ok(classify_facts(model, self.facts()))
    }
}

fn actual_get(factory: &GraphValueFactory, root: &ValueHandle, key: &str) -> Result<ValueHandle, ValueError> {
    factory.heap().get(root, &key.into(), factory.runtime())
}

fn actual_string(factory: &GraphValueFactory, value: &ValueHandle) -> Result<Option<String>, ValueError> {
    match factory.heap().snapshot(value)? {
        ValueSnapshot::Undefined | ValueSnapshot::Null => Ok(None),
        ValueSnapshot::String(value) => value.to_utf8().map(Some).map_err(|_| {
            // Rust's current scalar regex kernel cannot accept an isolated
            // surrogate. Keep the original value intact and fail explicitly;
            // never silently repair it into replay authority.
            ValueError::LegacyBridge("Auth classification requires a Unicode scalar metadata string")
        }),
        _ => Err(ValueError::TypeError("Auth classification metadata must be a string".into())),
    }
}

fn actual_number(factory: &GraphValueFactory, root: &ValueHandle, key: &str) -> Result<Option<f64>, ValueError> {
    let value = actual_get(factory, root, key)?;
    match factory.heap().snapshot(&value)? {
        ValueSnapshot::Undefined | ValueSnapshot::Null => Ok(None),
        ValueSnapshot::Number(value) => Ok(Some(value)),
        _ => Err(ValueError::TypeError("Auth classification metadata must be a number".into())),
    }
}

fn actual_status(factory: &GraphValueFactory, root: &ValueHandle, key: &str) -> Result<Option<u16>, ValueError> {
    actual_number(factory, root, key)?
        .map(|value| {
            if value.is_finite() && value.fract() == 0.0 && (0.0..=u16::MAX as f64).contains(&value) {
                Ok(value as u16)
            } else {
                Err(ValueError::TypeError("Auth classification status must be a u16 integer".into()))
            }
        })
        .transpose()
}

fn actual_bool(factory: &GraphValueFactory, root: &ValueHandle, key: &str) -> Result<bool, ValueError> {
    let value = actual_get(factory, root, key)?;
    match factory.heap().snapshot(&value)? {
        ValueSnapshot::Undefined => Ok(false),
        ValueSnapshot::Boolean(value) => Ok(value),
        _ => Err(ValueError::TypeError("Auth classification veto must be a boolean".into())),
    }
}

pub(crate) fn actual_failure_facts(
    message: &ValueHandle,
    factory: &GraphValueFactory,
) -> Result<Option<ActualFailureFacts>, ValueError> {
    factory.heap().adopt(message)?;
    let stop = actual_get(factory, message, "stopReason")?;
    if !matches!(factory.heap().snapshot(&stop)?, ValueSnapshot::String(value) if value.equals_ascii("error")) {
        return Ok(None);
    }
    let evidence = actual_get(factory, message, "failureEvidence")?;
    if matches!(factory.heap().snapshot(&evidence)?, ValueSnapshot::Undefined | ValueSnapshot::Null) {
        return Ok(None);
    }
    let kind = actual_get(factory, &evidence, "kind")?;
    let kind = match actual_string(factory, &kind)?.as_deref() {
        Some("http") => ProviderErrorKind::Http,
        Some("stream") => ProviderErrorKind::Stream,
        Some("transport") => ProviderErrorKind::Transport,
        Some("incomplete") => ProviderErrorKind::Incomplete,
        Some("timeout") => ProviderErrorKind::Timeout,
        Some("config") => ProviderErrorKind::Config,
        Some("aborted") => ProviderErrorKind::Aborted,
        _ => return Err(ValueError::TypeError("Unknown auth failure evidence kind".into())),
    };
    let context = actual_get(factory, &evidence, "contextRecovery")?;
    let context_recovery = match actual_string(factory, &context)?.as_deref() {
        None => None,
        Some("contentOnly") => Some(ara_ai::ContextRecoveryEvidence::ContentOnly),
        Some("nativeOutput") => Some(ara_ai::ContextRecoveryEvidence::NativeOutput),
        Some("usageAdmission") => Some(ara_ai::ContextRecoveryEvidence::UsageAdmission),
        Some("nativeValidation") => Some(ara_ai::ContextRecoveryEvidence::NativeValidation),
        _ => return Err(ValueError::TypeError("Unknown auth context recovery evidence".into())),
    };
    let flags = actual_number(factory, &evidence, "errorId")?.unwrap_or(0.0);
    if !flags.is_finite() || flags.fract() != 0.0 || !(0.0..=u32::MAX as f64).contains(&flags) {
        return Err(ValueError::TypeError("Auth failure flags must be a u32 integer".into()));
    }
    let error = actual_get(factory, message, "errorMessage")?;
    let code = actual_get(factory, &evidence, "code")?;
    Ok(Some(ActualFailureFacts {
        error_message: actual_string(factory, &error)?,
        error_status: actual_status(factory, message, "errorStatus")?,
        evidence: ProviderFailureEvidence {
            kind,
            status: actual_status(factory, &evidence, "status")?,
            code: actual_string(factory, &code)?,
            replay_blocked: actual_bool(factory, &evidence, "replayBlocked")?,
            same_route_blocked: actual_bool(factory, &evidence, "sameRouteBlocked")?,
            context_recovery,
            wait_ms: actual_number(factory, &evidence, "waitMs")?,
            header_wait_ms: actual_number(factory, &evidence, "headerWaitMs")?,
            error_id: flags as u32,
        },
    }))
}

fn stored_id(lease: &RequestAuthLease) -> Option<i64> {
    if let CredentialIdentity::Stored { id, .. } = lease.identity() { Some(*id) } else { None }
}

/// Deliberately no Debug: attempted bearers remain private to the operation.
pub(crate) struct AuthRetryState {
    keys: BTreeSet<String>,
    attempts: usize,
    refreshed_current: bool,
    legacy_switch_used: bool,
    token_refresh_replay_used: bool,
    quota_reset_attempted: bool,
    quota_feedback_dispatched: bool,
}
impl AuthRetryState {
    pub(crate) fn new(initial: &RequestAuthLease) -> Self {
        Self {
            keys: initial.api_key().map(str::to_owned).into_iter().collect(),
            attempts: 1,
            refreshed_current: false,
            legacy_switch_used: false,
            token_refresh_replay_used: false,
            quota_reset_attempted: false,
            quota_feedback_dispatched: false,
        }
    }
    fn accept(
        &mut self,
        failed: &RequestAuthLease,
        next: RequestAuthLease,
        action: AuthRetryAction,
        refreshed: bool,
    ) -> Option<RequestAuthLease> {
        let key = next.api_key()?;
        if self.attempts >= MAX_ATTEMPTS || self.keys.contains(key) {
            return None;
        }
        // This is streamSimple's bearer driver, not withOAuthAccess's durable
        // identity driver. A peer may supply a new bearer for the original row
        // during last chance; its already-seen row ID does not form a cycle.
        if action == AuthRetryAction::RefreshSame
            && stored_id(failed).is_some()
            && stored_id(failed) != stored_id(&next)
        {
            return None;
        }
        self.keys.insert(key.to_owned());
        self.attempts += 1;
        self.refreshed_current = refreshed;
        Some(next)
    }
    pub(crate) fn quota_feedback_dispatched(&self) -> bool {
        self.quota_feedback_dispatched
    }
    pub(crate) async fn next(
        &mut self,
        resolver: &dyn RequestAuthResolver,
        model: &Model,
        failed: &RequestAuthLease,
        failure: &RequestAuthFailure,
        cancel: &CancellationToken,
    ) -> Result<Option<RequestAuthLease>, AuthResolveError> {
        self.quota_feedback_dispatched = false;
        if cancel.is_cancelled() {
            return Err(AuthResolveError::Cancelled);
        }
        if self.attempts >= MAX_ATTEMPTS {
            return Ok(None);
        }
        let direct = !matches!(failure.kind, Kind::Auth | Kind::TokenRefresh);
        if failure.kind == Kind::TokenRefresh {
            if self.token_refresh_replay_used {
                return Ok(None);
            }
            self.token_refresh_replay_used = true;
            self.refreshed_current = true;
            let next = retry(resolver, model, failed, failure, AuthRetryAction::RefreshSame, cancel).await?;
            return Ok(next.and_then(|next| self.accept(failed, next, AuthRetryAction::RefreshSame, true)));
        }
        if !direct {
            if self.legacy_switch_used {
                return Ok(None);
            }
            if !self.refreshed_current {
                self.refreshed_current = true;
                if let Some(next) =
                    retry(resolver, model, failed, failure, AuthRetryAction::RefreshSame, cancel).await?
                    && let Some(next) = self.accept(failed, next, AuthRetryAction::RefreshSame, true)
                {
                    return Ok(Some(next));
                }
            }
        }
        // Dispatch ownership is not a positive durable-write receipt. After
        // cancellation the result can be unknown and must not be re-recorded
        // by the separate Session quota observer.
        self.quota_feedback_dispatched = failure.kind == Kind::Quota;
        let next = retry(resolver, model, failed, failure, AuthRetryAction::RotateSibling, cancel).await?;
        let next = next.and_then(|next| self.accept(failed, next, AuthRetryAction::RotateSibling, !direct));
        if next.is_some() && !direct {
            self.legacy_switch_used = true
        }
        if next.is_none() && failure.kind == Kind::Quota && !self.quota_reset_attempted && self.attempts < MAX_ATTEMPTS
        {
            self.quota_reset_attempted = true;
            let reset = resolver.quota_reset(model, failed, failure, cancel).await;
            if cancel.is_cancelled() || matches!(reset, Err(AuthResolveError::Cancelled)) {
                return Err(AuthResolveError::Cancelled);
            }
            if let Ok(Some(reset)) = reset {
                // This exception does not clear the seen-key set or replenish
                // MAX_ATTEMPTS. A reused bearer must name the actual row whose
                // saved reset was positively acknowledged by this receipt.
                if !reset.request_id.is_empty()
                    && let Some(key) = reset.lease.api_key()
                {
                    if self.keys.contains(key) {
                        if stored_id(&reset.lease) == Some(reset.confirmed_credential_id) {
                            self.attempts += 1;
                            self.refreshed_current = false;
                            return Ok(Some(reset.lease));
                        }
                    } else {
                        return Ok(self.accept(failed, reset.lease, AuthRetryAction::RotateSibling, false));
                    }
                }
            }
        }
        Ok(next)
    }
}

async fn retry(
    resolver: &dyn RequestAuthResolver,
    model: &Model,
    failed: &RequestAuthLease,
    failure: &RequestAuthFailure,
    action: AuthRetryAction,
    cancel: &CancellationToken,
) -> Result<Option<RequestAuthLease>, AuthResolveError> {
    let result = resolver.retry(model, failed, failure, action, cancel).await;
    if cancel.is_cancelled() || matches!(result, Err(AuthResolveError::Cancelled)) {
        return Err(AuthResolveError::Cancelled);
    }
    // Native resolveRetryKey converts a resolver failure into an absent key;
    // the caller still publishes the original upstream rejection.
    Ok(result.unwrap_or(None))
}
