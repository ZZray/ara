//! Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/ai/src/auth-storage.ts:5995–6221. The Host owns permission to spend
//! a saved reset. These account operations do not select or rotate siblings.
//!
//! MIT License
//!
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.

use super::*;
use crate::codex_reset_receipts::{
    ResetOperationReceipt, ResetReceiptBegin, ResetReceiptCode, ResetReceiptState, ResetReceiptStore,
};
use crate::codex_usage::{
    CodexResetConsumeError, CodexResetConsumeRequest, CodexResetCredit, CodexUsageProvider,
    pick_soonest_expiring_credit,
};

#[derive(Clone, Default)]
pub struct ListResetCreditsOptions {
    pub provider: Option<String>,
    pub session_id: Option<String>,
    pub base_url_resolver: Option<CredentialBaseUrlResolver>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetCreditTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Clone, Default)]
pub struct RedeemResetCreditOptions {
    pub target: ResetCreditTarget,
    pub provider: Option<String>,
    pub credit_id: Option<String>,
    pub base_url_resolver: Option<CredentialBaseUrlResolver>,
    /// Optional Host admission, rechecked after asynchronous preparation and
    /// before journal admission/POST. Manual callers already have explicit
    /// authority; automatic Hosts can revoke an earlier consent or route.
    pub admit_consume: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetCreditAccountStatus {
    pub credential_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub active: bool,
    pub available_count: f64,
    pub credits: Vec<CodexResetCredit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetCreditRedeemOutcome {
    pub ok: bool,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_id: Option<String>,
    /// A confirmed upstream reset is retained when local settlement fails.
    /// This Host receipt extension never authorizes replay of the consume POST.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement_error: Option<String>,
}

/// Operation-aware Host projection. Unknown consumes retain the exact account,
/// selected credit and request UUID instead of losing them in a generic error.
#[derive(Clone, Debug, PartialEq)]
pub struct ResetRedemption {
    pub outcome: ResetCreditRedeemOutcome,
    pub operation: Option<ResetOperationReceipt>,
}

struct ExactOAuthResolution {
    row: StoredAuthCredential,
    result: Result<ResolvedOAuth, AuthStorageError>,
}

impl AuthStorage {
    /// Scope shared Host coordination and durable fences by the actual
    /// credential authority, provider and canonical consume endpoint. No
    /// private URL, local path or bearer is exposed in the returned digest.
    pub(crate) fn reset_operation_scope(
        &self,
        provider: &str,
        base_url: Option<&str>,
    ) -> Result<String, AuthStorageError> {
        let authority = match &self.inner.store {
            CredentialStoreOwner::Local(store) => store
                .lock()
                .map_err(|_| AuthStorageError::Storage)?
                .reset_receipt_authority()
                .map_err(|_| AuthStorageError::Storage)?,
            CredentialStoreOwner::Remote(store) => store.reset_receipt_authority(),
        };
        let endpoint = format!("{}/consume", crate::codex_usage::codex_reset_credits_url(base_url));
        let identity =
            serde_json::to_vec(&(authority, provider, endpoint)).map_err(|_| AuthStorageError::Configuration)?;
        let digest = ring::digest::digest(&ring::digest::SHA256, &identity);
        Ok(digest.as_ref().iter().map(|byte| format!("{byte:02x}")).collect())
    }

    pub(crate) fn reset_operation_account_key(
        &self,
        provider: &str,
        base_url: Option<&str>,
        credential_id: i64,
        account_id: Option<&str>,
        email: Option<&str>,
    ) -> Result<String, AuthStorageError> {
        let account = account_id
            .into_iter()
            .chain(email)
            .map(|identity| ara_prompt::js::trim(identity).to_lowercase())
            .find(|identity| !identity.is_empty())
            .unwrap_or_else(|| format!("credential:{credential_id}"));
        Ok(format!("{}:{account}", self.reset_operation_scope(provider, base_url)?))
    }

    // Keep original row identity on failures. Existing request accessor and
    // retry contracts continue using their established Option projections.
    async fn reset_account_accesses(&self, provider: &str) -> Result<Vec<ExactOAuthResolution>, AuthStorageError> {
        self.reset_account_accesses_with_cancel(provider, &CancellationToken::new()).await
    }

    async fn reset_account_accesses_with_cancel(
        &self,
        provider: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<ExactOAuthResolution>, AuthStorageError> {
        if self.suppressed(provider) {
            return Ok(Vec::new());
        }
        let rows = self.provider_rows(provider).await?;
        Ok(join_all(rows.into_iter().filter(|row| matches!(row.credential, AuthCredential::OAuth { .. })).map(
            |row| async move {
                let mut result = self.resolve_exact_oauth(provider, row.clone(), false, cancel).await;
                if let Err(error) = result
                    && let Err(settlement) = self.handle_oauth_failure(provider, &row, error, None).await
                {
                    result = Err(settlement);
                }
                ExactOAuthResolution { row, result }
            },
        ))
        .await)
    }

    fn reset_credit_client(&self) -> Result<CodexUsageProvider, AuthStorageError> {
        self.inner
            .options
            .reset_credit_client
            .clone()
            .map(Ok)
            .unwrap_or_else(|| CodexUsageProvider::new().map_err(|_| AuthStorageError::Configuration))
    }

    /// Dedicated live GET per stored account; never reads the usage cache.
    pub async fn list_reset_credits(
        &self,
        options: &ListResetCreditsOptions,
        cancel: &CancellationToken,
    ) -> Result<Vec<ResetCreditAccountStatus>, AuthStorageError> {
        let provider = options.provider.as_deref().unwrap_or("openai-codex");
        let accesses = self.reset_account_accesses(provider).await?;
        if accesses.is_empty() {
            return Ok(Vec::new());
        }
        let base_url = options.base_url_resolver.as_ref().and_then(|resolve| resolve(provider));
        let active = self.get_oauth_account_identity(provider, options.session_id.as_deref())?;
        let client = self.reset_credit_client()?;
        Ok(join_all(accesses.into_iter().map(|access| {
            let active = &active;
            let client = &client;
            let base_url = &base_url;
            async move {
                let row = access.result.as_ref().map(|resolved| &resolved.row).unwrap_or(&access.row);
                let account_id = text_field(row, "accountId");
                let email = text_field(row, "email");
                let is_active = active.as_ref().is_some_and(|active| {
                    active.account_id.as_ref().is_some_and(|id| !id.is_empty() && account_id.as_ref() == Some(id))
                        || active.email.as_ref().is_some_and(|id| !id.is_empty() && email.as_ref() == Some(id))
                });
                let mut status = ResetCreditAccountStatus {
                    credential_id: access.row.id,
                    account_id,
                    email,
                    active: is_active,
                    available_count: 0.0,
                    credits: Vec::new(),
                    error: None,
                };
                let resolved = match access.result {
                    Ok(resolved) => resolved,
                    Err(error) => {
                        status.error = Some(error.to_string());
                        return status;
                    }
                };
                match client
                    .list_reset_credits(
                        resolved.lease.api_key().unwrap_or_default(),
                        status.account_id.as_deref(),
                        base_url.as_deref(),
                        self.inner.options.usage_request_timeout,
                        cancel,
                    )
                    .await
                {
                    Some(list) => {
                        status.available_count = list.available_count;
                        status.credits = list.credits;
                    }
                    None => status.error = Some("Failed to load saved resets".into()),
                }
                status
            }
        }))
        .await)
    }

    /// Native OR target matching and business outcomes. No automatic POST retry
    /// and no implicit idempotency key shared across separate facade calls.
    pub async fn redeem_reset_credit(
        &self,
        options: &RedeemResetCreditOptions,
        cancel: &CancellationToken,
    ) -> Result<ResetCreditRedeemOutcome, AuthStorageError> {
        self.redeem_reset_credit_inner(options, None, cancel).await.map(|redemption| redemption.outcome)
    }

    /// The Host journal is separate from Local/Remote credential ownership.
    /// Once admitted, this operation has an owner even if the awaiting caller
    /// disappears. Automatic callers use an independent consume deadline;
    /// cancellation after dispatch remains an unknown external outcome.
    pub async fn redeem_reset_credit_observed(
        &self,
        options: &RedeemResetCreditOptions,
        receipts: Arc<dyn ResetReceiptStore>,
        cancel: &CancellationToken,
    ) -> Result<ResetRedemption, AuthStorageError> {
        if cancel.is_cancelled() {
            return Err(AuthStorageError::Cancelled);
        }
        self.inner.usage_pending.fetch_add(1, Ordering::AcqRel);
        let owner = self.clone();
        let options = options.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let _settlement = UsageSettlement(owner.clone());
            owner.redeem_reset_credit_inner(&options, Some(receipts), &cancel).await
        })
        .await
        .map_err(|_| AuthStorageError::OutcomeUnknown)?
    }

    async fn redeem_reset_credit_inner(
        &self,
        options: &RedeemResetCreditOptions,
        receipts: Option<Arc<dyn ResetReceiptStore>>,
        cancel: &CancellationToken,
    ) -> Result<ResetRedemption, AuthStorageError> {
        let provider = options.provider.as_deref().unwrap_or("openai-codex");
        let base_url = options.base_url_resolver.as_ref().and_then(|resolve| resolve(provider));
        let accesses = if receipts.is_some() {
            self.reset_account_accesses_with_cancel(provider, cancel).await?
        } else {
            self.reset_account_accesses(provider).await?
        };
        let matched =
            accesses.into_iter().find(|access| {
                let row = access.result.as_ref().map(|resolved| &resolved.row).unwrap_or(&access.row);
                options.target.credential_id == Some(access.row.id)
                    || options.target.account_id.as_ref().is_some_and(|target| {
                        !target.is_empty() && text_field(row, "accountId").as_ref() == Some(target)
                    })
                    || options
                        .target
                        .email
                        .as_ref()
                        .is_some_and(|target| !target.is_empty() && text_field(row, "email").as_ref() == Some(target))
            });
        let mut outcome = ResetCreditRedeemOutcome {
            ok: false,
            code: "no_account".into(),
            account_id: options.target.account_id.clone(),
            email: options.target.email.clone(),
            credit_id: None,
            settlement_error: None,
        };
        let Some(matched) = matched else {
            return Ok(ResetRedemption { outcome, operation: None });
        };
        let row = matched.result.as_ref().map(|resolved| &resolved.row).unwrap_or(&matched.row);
        outcome.account_id = text_field(row, "accountId");
        outcome.email = text_field(row, "email");
        let resolved = match matched.result {
            Ok(resolved) => resolved,
            Err(_) => {
                outcome.code = "account_unavailable".into();
                return Ok(ResetRedemption { outcome, operation: None });
            }
        };
        let client = self.reset_credit_client()?;
        let token = resolved.lease.api_key().unwrap_or_default();
        let credit_id = match options.credit_id.as_ref().filter(|id| !id.is_empty()) {
            Some(id) => id.clone(),
            None => {
                let Some(list) = client
                    .list_reset_credits(
                        token,
                        outcome.account_id.as_deref(),
                        base_url.as_deref(),
                        self.inner.options.usage_request_timeout,
                        cancel,
                    )
                    .await
                else {
                    outcome.code = "credit_list_failed".into();
                    return Ok(ResetRedemption { outcome, operation: None });
                };
                let Some(credit) = pick_soonest_expiring_credit(&list.credits) else {
                    outcome.code = "no_credit".into();
                    return Ok(ResetRedemption { outcome, operation: None });
                };
                credit.id.clone()
            }
        };
        if receipts.is_some() && options.admit_consume.as_ref().is_some_and(|admit| !admit()) {
            return Err(AuthStorageError::Cancelled);
        }
        let mut operation = if let Some(receipts) = &receipts {
            if cancel.is_cancelled() {
                return Err(AuthStorageError::Cancelled);
            }
            let account_key = self.reset_operation_account_key(
                provider,
                base_url.as_deref(),
                matched.row.id,
                outcome.account_id.as_deref(),
                outcome.email.as_deref(),
            )?;
            let endpoint = format!("{}/consume", crate::codex_usage::codex_reset_credits_url(base_url.as_deref()));
            let digest = ring::digest::digest(&ring::digest::SHA256, endpoint.as_bytes());
            let receipt = ResetOperationReceipt {
                request_id: uuid::Uuid::new_v4().to_string(),
                account_key,
                credential_id: matched.row.id,
                account_id: outcome.account_id.clone(),
                email: outcome.email.clone(),
                credit_id: credit_id.clone(),
                endpoint_fingerprint: digest.as_ref().iter().map(|byte| format!("{byte:02x}")).collect(),
                created_at: self.now(),
                updated_at: self.now(),
                state: ResetReceiptState::Pending,
                code: ResetReceiptCode::Pending,
            };
            match receipts.begin(&receipt).map_err(|_| AuthStorageError::Storage)? {
                ResetReceiptBegin::Started => Some(receipt),
                ResetReceiptBegin::Fenced(fence) => {
                    outcome.code = "outcome_unknown".into();
                    outcome.account_id = fence.account_id.clone();
                    outcome.email = fence.email.clone();
                    outcome.credit_id = Some(fence.credit_id.clone());
                    return Ok(ResetRedemption { outcome, operation: Some(fence) });
                }
            }
        } else {
            None
        };
        let result = client
            .consume_reset_credit(
                CodexResetConsumeRequest {
                    access_token: token,
                    account_id: outcome.account_id.as_deref(),
                    base_url: base_url.as_deref(),
                    credit_id: &credit_id,
                    redeem_request_id: operation.as_ref().map(|receipt| receipt.request_id.as_str()),
                },
                self.inner.options.usage_request_timeout,
                cancel,
            )
            .await;
        outcome.credit_id = Some(credit_id);
        if let Some(receipt) = &mut operation {
            receipt.updated_at = self.now();
            match &result {
                Ok(result) => {
                    // Deliberate ARA difference from native fallback-to-reset:
                    // a 5xx or malformed/unknown body does not confirm a spend.
                    let code = result.raw.as_ref().and_then(|raw| raw.get("code")).and_then(Value::as_str);
                    let known = if result.status < 500 {
                        match code {
                            Some("reset") if (200..300).contains(&result.status) => Some(ResetReceiptCode::Reset),
                            Some("already_redeemed") => Some(ResetReceiptCode::AlreadyRedeemed),
                            Some("no_credit") => Some(ResetReceiptCode::NoCredit),
                            Some("nothing_to_reset") => Some(ResetReceiptCode::NothingToReset),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    if let Some(code) = known {
                        receipt.state = ResetReceiptState::Known;
                        receipt.code = code;
                        outcome.ok = code == ResetReceiptCode::Reset;
                        outcome.code = result.code.clone();
                    } else {
                        receipt.state = ResetReceiptState::Unknown;
                        receipt.code = ResetReceiptCode::Unknown;
                        outcome.code = "outcome_unknown".into();
                    }
                }
                Err(CodexResetConsumeError::Cancelled | CodexResetConsumeError::InvalidEndpoint) => {
                    receipt.state = ResetReceiptState::NotDispatched;
                    receipt.code = ResetReceiptCode::Cancelled;
                    outcome.code = "cancelled".into();
                }
                Err(CodexResetConsumeError::OutcomeUnknown { redeem_request_id }) => {
                    debug_assert_eq!(redeem_request_id, &receipt.request_id);
                    receipt.state = ResetReceiptState::Unknown;
                    receipt.code = ResetReceiptCode::Unknown;
                    outcome.code = "outcome_unknown".into();
                }
            }
            if receipts.as_ref().expect("receipt owner").finish(receipt).is_err() {
                // Preserve the previous durable Pending fence, even when the
                // response itself positively confirms the spend.
                outcome.settlement_error = Some("saved reset receipt storage failed".into());
                // Publish the last durable state as replay authority. The
                // positive upstream observation remains in outcome, and its
                // owned local settlement still runs; Pending cannot authorize
                // a model retry or another consume.
                receipt.state = ResetReceiptState::Pending;
                receipt.code = ResetReceiptCode::Pending;
                receipt.updated_at = receipt.created_at;
            }
        } else {
            let result = result.map_err(|error| match error {
                CodexResetConsumeError::Cancelled => AuthStorageError::Cancelled,
                CodexResetConsumeError::InvalidEndpoint => AuthStorageError::Configuration,
                CodexResetConsumeError::OutcomeUnknown { .. } => AuthStorageError::OutcomeUnknown,
            })?;
            outcome.ok = result.ok;
            outcome.code = result.code;
        }
        if outcome.ok {
            // Native Promises retain this confirmed tail after a caller stops
            // awaiting. Keep store notification followed by local unblock in
            // an owned task, including when the Rust caller future is dropped.
            self.inner.usage_pending.fetch_add(1, Ordering::AcqRel);
            let owner = self.clone();
            let provider = provider.to_owned();
            let cancel = cancel.clone();
            let settlement = tokio::spawn(async move {
                let _settlement = UsageSettlement(owner.clone());
                let stale = owner.expire_oauth_usage_cache(&provider, base_url.as_deref());
                if let Some(hook) = &owner.inner.options.usage_store {
                    hook.invalidate_usage_cache();
                    let _ = hook.notify_usage_stale(&cancel).await;
                }
                let clear = owner
                    .database(move |store, state| {
                        let rows = Self::load_provider(store, state, &provider)?;
                        state.assignments.clear_credential_blocks(store, &provider, matched.row.id, &rows);
                        if state.assignments.persisted_block_store_damaged() {
                            Err(AuthStorageError::Storage)
                        } else {
                            Ok(())
                        }
                    })
                    .await;
                owner.bump_generation();
                stale.is_err() || clear.is_err()
            });
            if settlement.await.unwrap_or(true) {
                outcome.settlement_error = Some(AuthStorageError::Storage.to_string());
            }
        }
        Ok(ResetRedemption { outcome, operation })
    }
}
