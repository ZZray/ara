//! OpenAI Codex device credential issuance without a persistence owner.
//! Source: OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/ai/src/registry/oauth/openai-codex.ts and auth-storage.ts:3017.
//! Hosts publish issued credentials through their shared AuthStorage owner.
//
// MIT License
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

use super::{
    AuthCredential, CLIENT_ID, CodexAuthError, DEVICE_AUTH_URL, DeviceLoginInfo, REDIRECT_URI, cancellable_sleep,
    interval_seconds, login_credential, required, response_json,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// No database, account snapshot, refresh owner or durable grant is held here.
/// The client must disable redirects to keep authorization codes at the
/// authorized endpoint. Private transport values have no Debug implementation.
#[derive(Clone)]
pub struct OpenAiCodexDeviceLogin {
    client: reqwest::Client,
    auth_base_url: String,
    fixture: bool,
}

impl OpenAiCodexDeviceLogin {
    pub fn new(client: reqwest::Client) -> Self {
        Self::from_transport(client, "https://auth.openai.com".into(), false)
    }

    /// Controlled loopback upstream only; unavailable in ordinary CLI builds.
    #[cfg(feature = "test-fixture")]
    pub fn with_endpoints(client: reqwest::Client, auth_base_url: &str) -> Result<Self, CodexAuthError> {
        let url = reqwest::Url::parse(auth_base_url).map_err(|_| CodexAuthError::InvalidEndpoint)?;
        if !super::loopback_url(&url) || !matches!(url.path(), "" | "/") {
            return Err(CodexAuthError::InvalidEndpoint);
        }
        Ok(Self::from_transport(client, auth_base_url.trim_end_matches('/').into(), true))
    }

    pub(super) fn from_transport(client: reqwest::Client, auth_base_url: String, fixture: bool) -> Self {
        Self { client, auth_base_url, fixture }
    }

    /// Issue one interactive credential without storing it. The Host is
    /// responsible for confirmed publication through its shared auth owner.
    /// Cancellation before token dispatch is Cancelled; an unconfirmed token
    /// exchange after dispatch is OutcomeUnknown and is never automatically
    /// replayed. Hosts should cancel and await this future for its receipt;
    /// dropping it can lose an issued grant and is not evidence of rejection.
    pub async fn issue(
        &self,
        cancel: &CancellationToken,
        on_auth: impl Fn(DeviceLoginInfo) + Send + Sync,
    ) -> Result<AuthCredential, CodexAuthError> {
        let init = self.post_json("/api/accounts/deviceauth/usercode", json!({"client_id": CLIENT_ID}), cancel).await?;
        let device_id = required(&init, "device_auth_id")?.to_owned();
        let user_code = required(&init, "user_code")?.to_owned();
        let seconds = interval_seconds(init.get("interval"));
        let interval = if self.fixture { Duration::from_millis(5) } else { Duration::from_secs_f64(seconds + 3.0) };
        on_auth(DeviceLoginInfo { verification_url: DEVICE_AUTH_URL, user_code: user_code.clone() });
        for poll in 0..120 {
            let delay = if poll == 0 { interval.min(Duration::from_secs(5)) } else { interval };
            cancellable_sleep(delay, cancel).await?;
            let result = self
                .post_json(
                    "/api/accounts/deviceauth/token",
                    json!({"device_auth_id": device_id, "user_code": user_code}),
                    cancel,
                )
                .await;
            let authorized = match result {
                Err(CodexAuthError::HttpStatus(403 | 404)) => continue,
                result => result?,
            };
            let code = required(&authorized, "authorization_code")?;
            let verifier = required(&authorized, "code_verifier")?;
            return self.exchange_credential(code, verifier, cancel).await;
        }
        Err(CodexAuthError::TimedOut)
    }

    async fn post_json(&self, path: &str, body: Value, cancel: &CancellationToken) -> Result<Value, CodexAuthError> {
        if cancel.is_cancelled() {
            return Err(CodexAuthError::Cancelled);
        }
        let request =
            self.client.post(format!("{}{path}", self.auth_base_url)).timeout(Duration::from_secs(15)).json(&body);
        response_json(request, cancel).await
    }

    async fn exchange_credential(
        &self,
        code: &str,
        verifier: &str,
        cancel: &CancellationToken,
    ) -> Result<AuthCredential, CodexAuthError> {
        if cancel.is_cancelled() {
            return Err(CodexAuthError::Cancelled);
        }
        // Build first: configuration/encoding failure cannot dispatch a grant.
        let request = self
            .client
            .post(format!("{}/oauth/token", self.auth_base_url))
            .timeout(Duration::from_secs(15))
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", CLIENT_ID),
                ("code", code),
                ("code_verifier", verifier),
                ("redirect_uri", REDIRECT_URI),
            ])
            .build()
            .map_err(|_| CodexAuthError::Transport)?;
        let dispatched = AtomicBool::new(false);
        let operation = async {
            // Once execute is polled, a connection/proxy error cannot prove
            // the authorization code was not consumed by the authorized peer.
            dispatched.store(true, Ordering::SeqCst);
            let response = self.client.execute(request).await.map_err(|_| CodexAuthError::OutcomeUnknown)?;
            if response.status().is_server_error() {
                return Err(CodexAuthError::OutcomeUnknown);
            }
            if !response.status().is_success() {
                return Err(CodexAuthError::HttpStatus(response.status().as_u16()));
            }
            let token = response.json::<Value>().await.map_err(|_| CodexAuthError::OutcomeUnknown)?;
            login_credential(&token).map_err(|_| CodexAuthError::OutcomeUnknown)
        };
        tokio::select! { biased;
            _ = cancel.cancelled() => Err(if dispatched.load(Ordering::SeqCst) { CodexAuthError::OutcomeUnknown } else { CodexAuthError::Cancelled }),
            result = operation => result,
        }
    }
}
