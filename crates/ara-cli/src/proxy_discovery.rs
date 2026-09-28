//! Opt-in protocol selection for a dual-protocol proxy's `/v1/models` list.
//! Source: pinned OMP `model-discovery.ts::discoverProxyModels`.

use crate::Api;
use anyhow::{Result, bail};
use reqwest::header::{HeaderName, HeaderValue};
use serde_json::Value;
use std::time::Duration;

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_MODELS: usize = 4096;

pub(super) async fn discover_proxy_api(
    base_url: &str,
    model_id: &str,
    api_key: Option<&str>,
    extra_headers: &[(String, String)],
) -> Result<Api> {
    let base = base_url.trim_end_matches('/');
    let url = reqwest::Url::parse(base).map_err(|_| anyhow::anyhow!("proxy-auto requires a valid /v1 base URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.path().ends_with("/v1")
    {
        bail!("proxy-auto requires an HTTP(S) /v1 base URL without userinfo, query or fragment");
    }
    let models_url = reqwest::Url::parse(&format!("{base}/models"))
        .map_err(|_| anyhow::anyhow!("invalid proxy model discovery URL"))?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| anyhow::anyhow!("could not build proxy discovery client"))?;
    let mut request = client.get(models_url);
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    for (name, value) in extra_headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| anyhow::anyhow!("invalid proxy discovery header name"))?;
        if name == reqwest::header::AUTHORIZATION || name.as_str() == "x-api-key" {
            bail!("proxy-auto does not support custom authentication headers; select an explicit --api");
        }
        let value =
            HeaderValue::from_str(value).map_err(|_| anyhow::anyhow!("invalid proxy discovery header value"))?;
        request = request.header(name, value);
    }
    let mut response = request.send().await.map_err(|_| anyhow::anyhow!("proxy model discovery request failed"))?;
    if !response.status().is_success() {
        bail!("proxy model discovery returned HTTP {}", response.status().as_u16());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| anyhow::anyhow!("proxy model discovery body failed"))? {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            bail!("proxy model discovery response exceeds {MAX_RESPONSE_BYTES} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    let payload: Value =
        serde_json::from_slice(&body).map_err(|_| anyhow::anyhow!("proxy model discovery returned invalid JSON"))?;
    let models = payload
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("proxy model discovery has no data array"))?;
    if models.len() > MAX_MODELS {
        bail!("proxy model discovery returned too many models");
    }
    let mut selected = None;
    for model in models {
        if model.get("id").and_then(Value::as_str) != Some(model_id) {
            continue;
        }
        if selected.is_some() {
            bail!("proxy model discovery returned duplicate selected model IDs");
        }
        let endpoints = model
            .get("supported_endpoint_types")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("selected proxy model has no supported_endpoint_types array"))?;
        let has = |needle| endpoints.iter().any(|value| value.as_str() == Some(needle));
        selected = Some(if has("anthropic") {
            Api::AnthropicMessages
        } else if has("openai") {
            Api::OpenaiCompletions
        } else {
            bail!("selected proxy model has no supported Anthropic or OpenAI Chat endpoint");
        });
    }
    selected.ok_or_else(|| anyhow::anyhow!("selected model was not found in proxy discovery"))
}
