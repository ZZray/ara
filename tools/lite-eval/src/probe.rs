//! Environment probe before the first task (requirement 3.3, 3.4).
//!
//! Exactly one non-streaming `chat/completions` request with `max_tokens=1`
//! and a fixed short input; none when the credential is missing or empty.
//! The probe is outside every task: it consumes no `max_calls` or `max_runs`.

use std::time::{Duration, Instant};

use ara_agent_lite_harness::core::model::{plaintext_loopback, GenerationPolicy};
use serde::Serialize;
use serde_json::{json, Value};

use crate::{common::sha256_hex, policy::RunPolicy};

/// The fixed probe input.
pub const PROBE_PROMPT: &str = "Reply with OK only.";
/// Largest response body read.
const RESPONSE_LIMIT: usize = 1024 * 1024;

/// Failure classes of requirement 3.3. `detail` never contains the credential.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "category", rename_all = "snake_case")]
pub enum ProbeFailure {
    CredentialMissing,
    ModelMismatch { expected: String, echo: String },
    ProtocolMismatch { detail: String },
    Unreachable { detail: String },
    AuthRejected { http_status: u16 },
    Timeout { timeout_ms: u64 },
}

impl ProbeFailure {
    pub fn category(&self) -> &'static str {
        match self {
            Self::CredentialMissing => "credential_missing",
            Self::ModelMismatch { .. } => "model_mismatch",
            Self::ProtocolMismatch { .. } => "protocol_mismatch",
            Self::Unreachable { .. } => "unreachable",
            Self::AuthRejected { .. } => "auth_rejected",
            Self::Timeout { .. } => "timeout",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProbeReceipt {
    /// `model` of the response, verbatim.
    pub model_echo: String,
    /// `identical` when the echo equals the Run_Policy model ID; otherwise
    /// `accepted_alias_unverified` (listed in `model.accepted_echo`).
    pub identity: &'static str,
    pub http_status: u16,
    pub elapsed_ms: u64,
    pub response_sha256: String,
}

/// The probe request body, exactly as sent.
pub fn request_body(policy: &RunPolicy) -> Value {
    let mut body = json!({
        "model": policy.model.id,
        "messages": [{"role": "user", "content": PROBE_PROMPT}],
        "max_tokens": 1,
        "stream": false,
    });
    if policy.generation.generation_policy == GenerationPolicy::ThinkingTypeDisabled {
        body["thinking"] = json!({"type": "disabled"});
    }
    body
}

/// Send the probe. `token` is the credential read once from the variable
/// named by `model.credential_env`; empty means missing.
pub async fn probe(policy: &RunPolicy, token: &str) -> Result<ProbeReceipt, ProbeFailure> {
    if token.is_empty() {
        return Err(ProbeFailure::CredentialMissing);
    }
    let timeout_ms = policy.probe.timeout_ms;
    let endpoint = policy.model.route.endpoint.trim_end_matches('/');
    let url = url_of(endpoint)?;
    let builder = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .redirect(reqwest::redirect::Policy::none());
    let client = if plaintext_loopback(&url) { builder.no_proxy() } else { builder }
        .build()
        .map_err(|error| ProbeFailure::Unreachable { detail: format!("cannot build HTTP client: {error}") })?;
    let started = Instant::now();
    let classify = |error: reqwest::Error| {
        if error.is_timeout() {
            ProbeFailure::Timeout { timeout_ms }
        } else {
            // reqwest errors name the URL, never the bearer token.
            ProbeFailure::Unreachable { detail: error.without_url().to_string() }
        }
    };
    let mut response = client
        .post(format!("{endpoint}/chat/completions"))
        .bearer_auth(token)
        .json(&request_body(policy))
        .send()
        .await
        .map_err(classify)?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(classify)? {
        if bytes.len() + chunk.len() > RESPONSE_LIMIT {
            return Err(ProbeFailure::ProtocolMismatch { detail: "response body exceeds 1 MiB".into() });
        }
        bytes.extend_from_slice(&chunk);
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;
    if matches!(status, 401 | 403) {
        return Err(ProbeFailure::AuthRejected { http_status: status });
    }
    if !(200..300).contains(&status) {
        return Err(ProbeFailure::Unreachable { detail: format!("HTTP {status} (body withheld)") });
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ProbeFailure::ProtocolMismatch { detail: "response is not JSON".into() })?;
    let choice_ok = value["choices"].as_array().and_then(|choices| choices.first()).is_some_and(|choice| {
        choice["message"].is_object() && choice["message"]["role"].as_str().is_none_or(|role| role == "assistant")
    });
    if !value.is_object() || !choice_ok {
        return Err(ProbeFailure::ProtocolMismatch {
            detail: "response lacks choices[0].message of an OpenAI chat completion".into(),
        });
    }
    let Some(echo) = value["model"].as_str() else {
        return Err(ProbeFailure::ProtocolMismatch { detail: "response has no string `model`".into() });
    };
    let identity = if echo == policy.model.id {
        "identical"
    } else if policy.model.accepted_echo.iter().any(|alias| alias == echo) {
        "accepted_alias_unverified"
    } else {
        return Err(ProbeFailure::ModelMismatch { expected: policy.model.id.clone(), echo: echo.to_owned() });
    };
    Ok(ProbeReceipt {
        model_echo: echo.to_owned(),
        identity,
        http_status: status,
        elapsed_ms,
        response_sha256: sha256_hex(&bytes),
    })
}

fn url_of(endpoint: &str) -> Result<reqwest::Url, ProbeFailure> {
    let url = reqwest::Url::parse(endpoint)
        .map_err(|error| ProbeFailure::Unreachable { detail: format!("invalid endpoint: {error}") })?;
    if url.scheme() != "https" && !plaintext_loopback(&url) {
        return Err(ProbeFailure::Unreachable {
            detail: "endpoint must be HTTPS (plain HTTP only for literal loopback)".into(),
        });
    }
    Ok(url)
}
