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
    retry_classification::{self, ProviderErrorKind, flag},
};
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
    // Transport, partial stream and unknown dispatch outcomes cannot establish
    // a credential rejection. Never convert their text into replay authority.
    let evidence = message.failure_evidence.as_ref()?;
    if evidence.kind != ProviderErrorKind::Http {
        return None;
    }
    let status = evidence.status.or(message.error_status);
    let text = message.error_message.as_deref().unwrap_or("");
    let classification = retry_classification::classify_retry(message, &model.api);
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
    pub(crate) async fn next(
        &mut self,
        resolver: &dyn RequestAuthResolver,
        model: &Model,
        failed: &RequestAuthLease,
        failure: &RequestAuthFailure,
        cancel: &CancellationToken,
    ) -> Result<Option<RequestAuthLease>, AuthResolveError> {
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
