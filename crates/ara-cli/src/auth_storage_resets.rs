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

struct ExactOAuthResolution {
    row: StoredAuthCredential,
    result: Result<ResolvedOAuth, AuthStorageError>,
}

impl AuthStorage {
    // Keep original row identity on failures. Existing request accessor and
    // retry contracts continue using their established Option projections.
    async fn reset_account_accesses(&self, provider: &str) -> Result<Vec<ExactOAuthResolution>, AuthStorageError> {
        if self.suppressed(provider) {
            return Ok(Vec::new());
        }
        let rows = self.provider_rows(provider).await?;
        let cancel = CancellationToken::new();
        Ok(join_all(rows.into_iter().filter(|row| matches!(row.credential, AuthCredential::OAuth { .. })).map(|row| {
            let cancel = &cancel;
            async move {
                let mut result = self.resolve_exact_oauth(provider, row.clone(), false, cancel).await;
                if let Err(error) = result
                    && let Err(settlement) = self.handle_oauth_failure(provider, &row, error, None).await
                {
                    result = Err(settlement);
                }
                ExactOAuthResolution { row, result }
            }
        }))
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
        let provider = options.provider.as_deref().unwrap_or("openai-codex");
        let base_url = options.base_url_resolver.as_ref().and_then(|resolve| resolve(provider));
        let accesses = self.reset_account_accesses(provider).await?;
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
            return Ok(outcome);
        };
        let row = matched.result.as_ref().map(|resolved| &resolved.row).unwrap_or(&matched.row);
        outcome.account_id = text_field(row, "accountId");
        outcome.email = text_field(row, "email");
        let resolved = match matched.result {
            Ok(resolved) => resolved,
            Err(_) => {
                outcome.code = "account_unavailable".into();
                return Ok(outcome);
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
                    return Ok(outcome);
                };
                let Some(credit) = pick_soonest_expiring_credit(&list.credits) else {
                    outcome.code = "no_credit".into();
                    return Ok(outcome);
                };
                credit.id.clone()
            }
        };
        let result = client
            .consume_reset_credit(
                CodexResetConsumeRequest {
                    access_token: token,
                    account_id: outcome.account_id.as_deref(),
                    base_url: base_url.as_deref(),
                    credit_id: &credit_id,
                    redeem_request_id: None,
                },
                self.inner.options.usage_request_timeout,
                cancel,
            )
            .await
            .map_err(|error| match error {
                CodexResetConsumeError::Cancelled => AuthStorageError::Cancelled,
                CodexResetConsumeError::InvalidEndpoint => AuthStorageError::Configuration,
                CodexResetConsumeError::OutcomeUnknown { .. } => AuthStorageError::OutcomeUnknown,
            })?;
        outcome.ok = result.ok;
        outcome.code = result.code;
        outcome.credit_id = Some(credit_id);
        if outcome.ok {
            // Caller cancellation after confirmation must not prevent local
            // settlement. Both effects are attempted, retaining upstream ok.
            let stale = self.expire_oauth_usage_cache(provider, base_url.as_deref());
            let clear = self
                .database({
                    let provider = provider.to_owned();
                    move |store, state| {
                        let rows = Self::load_provider(store, state, &provider)?;
                        state.assignments.clear_credential_blocks(store, &provider, matched.row.id, &rows);
                        if state.assignments.persisted_block_store_damaged() {
                            Err(AuthStorageError::Storage)
                        } else {
                            Ok(())
                        }
                    }
                })
                .await;
            self.bump_generation();
            if stale.is_err() || clear.is_err() {
                outcome.settlement_error = Some(AuthStorageError::Storage.to_string());
            }
        }
        Ok(outcome)
    }
}
