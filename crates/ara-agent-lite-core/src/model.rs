//! Host model resolution; route binding/decision are opaque continuity keys.
use anyhow::{bail, ensure, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};
use url::{Host, Url};
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Route {
    pub model: String,
    pub protocol: String,
    pub binding: String,
    pub decision: String,
}

/// Explicit Host request policy. The default preserves the provider's existing behavior.
/// This selects a wire contract; it does not prove how a remote service executes it.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GenerationPolicy {
    #[default]
    ProviderDefault,
    /// Send only `thinking: { type: "disabled" }` on an OpenAI Chat route.
    /// The Host must verify that its selected endpoint supports this contract.
    ThinkingTypeDisabled,
}

/// Host permission for an additional native request after a closed text-only
/// Length response. This does not enlarge output/call budgets or prove effects.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LengthRecoveryPolicy {
    #[default]
    Stop,
    ReplanOnce,
}

impl GenerationPolicy {
    /// Validate the actual resolved route before admitting model or tool effects.
    pub fn validate_route(self, route: &Route) -> Result<()> {
        if self == Self::ThinkingTypeDisabled {
            ensure!(
                route.protocol == "openai-completions",
                "thinking_type_disabled requires the resolved openai-completions protocol"
            );
        }
        Ok(())
    }

    /// Apply a previously validated policy without exposing arbitrary sampling overrides.
    pub fn apply(self, model: &mut rpi_ai::Model) {
        if self == Self::ThinkingTypeDisabled {
            model
                .sampling_params
                .get_or_insert_with(Default::default)
                .insert("thinking".into(), serde_json::json!({"type":"disabled"}));
        }
    }
}

pub struct ModelSelection {
    pub route: Route,
    pub client: reqwest::Client,
    pub capabilities: crate::model_caps::ModelCaps,
    pub headers: BTreeMap<String, String>,
}
#[async_trait]
pub trait ModelHost: Send + Sync {
    async fn resolve(&self, route: Route, endpoint: &str, token: &str, run: &str) -> Result<ModelSelection>;

    /// Provenance of this Host's capability declaration, not proof of the
    /// remote model's behavior. Custom hosts do not imply a catalog lookup.
    fn capabilities_source(&self) -> &'static str {
        "host_model_configuration"
    }

    /// Pure admission hook for the resolved route. Implementations must not perform I/O
    /// or depend on mutable "last resolved model" state shared by concurrent Runs.
    fn generation_policy(&self, _route: &Route) -> Result<GenerationPolicy> {
        Ok(GenerationPolicy::ProviderDefault)
    }

    /// Pure policy for the actual resolved route. The Host remains responsible
    /// for external tool-effect settlement; native transcript pairing is not proof.
    fn length_recovery_policy(&self, _route: &Route) -> Result<LengthRecoveryPolicy> {
        Ok(LengthRecoveryPolicy::Stop)
    }

    /// Pure Host ceiling for one Run's output tokens. `None` keeps the
    /// builtin rule (see `output_limit`); `Some(0)` is refused before any request.
    fn output_ceiling(&self) -> Result<Option<u32>> {
        Ok(None)
    }
}
#[derive(Default)]
pub struct CatalogModelHost;
#[async_trait]
impl ModelHost for CatalogModelHost {
    async fn resolve(&self, route: Route, endpoint: &str, token: &str, run: &str) -> Result<ModelSelection> {
        let (route, client, capabilities) = resolve_catalog(route, endpoint, token, run).await?;
        Ok(ModelSelection { route, client, capabilities, headers: BTreeMap::new() })
    }

    fn capabilities_source(&self) -> &'static str {
        "hub_model_directory"
    }
}
pub fn validate_endpoint(url: &Url) -> Result<()> {
    ensure!(url.username().is_empty() && url.password().is_none(), "Model URL must not contain credentials");
    ensure!(
        matches!(url.scheme(), "https" | "wss") || plaintext_loopback(url),
        "public model connections require HTTPS/WSS with a valid certificate; HTTP/WS is allowed only for literal loopback development endpoints"
    );
    Ok(())
}

pub fn plaintext_loopback(url: &Url) -> bool {
    let loopback = match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        // Only literal loopback addresses qualify; DNS and proxies must not
        // turn an external hostname into a development transport exception.
        _ => false,
    };
    loopback && matches!(url.scheme(), "http" | "ws")
}

pub async fn resolve_catalog(
    route: Route,
    endpoint: &str,
    token: &str,
    _run: &str,
) -> Result<(Route, reqwest::Client, crate::model_caps::ModelCaps)> {
    resolve_catalog_with_headers(route, endpoint, token, _run, BTreeMap::new()).await
}
/// Host-supplied request headers apply to both catalogue validation and model calls.
/// This helper validates directory metadata but grants no model or tool authority.
pub async fn resolve_catalog_with_headers(
    mut route: Route,
    endpoint: &str,
    token: &str,
    _run: &str,
    headers: BTreeMap<String, String>,
) -> Result<(Route, reqwest::Client, crate::model_caps::ModelCaps)> {
    let endpoint_url = url::Url::parse(endpoint).context("invalid model endpoint")?;
    validate_endpoint(&endpoint_url)?;
    let mut h = reqwest::header::HeaderMap::new();
    for (key, value) in headers {
        h.insert(reqwest::header::HeaderName::from_bytes(key.as_bytes())?, value.parse()?);
    }
    let builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .default_headers(h);
    let client = if plaintext_loopback(&endpoint_url) { builder.no_proxy() } else { builder }.build()?;
    let mut response = client
        .get(format!("{}/models", endpoint.trim_end_matches('/')))
        .bearer_auth(token)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("model directory request failed"))?;
    ensure!(
        response.status().is_success(),
        "model directory returned HTTP {} (body withheld)",
        response.status().as_u16()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| anyhow::anyhow!("model directory read failed"))? {
        ensure!(bytes.len() + chunk.len() <= 2 * 1024 * 1024, "model directory exceeds 2 MiB");
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).context("invalid model directory JSON")?;
    let rows = value["data"].as_array().context("model directory has no data array")?;
    let selected: Vec<_> = rows.iter().filter(|r| r["id"].as_str() == Some(&route.model)).collect();
    ensure!(selected.len() == 1, "selected model is missing or duplicated in Hub directory");
    let row = selected[0];
    let mut protocols: Vec<&str> = match row.get("protocols") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => v
            .as_array()
            .context("invalid model protocols")?
            .iter()
            .map(|p| {
                let value = p.as_str().context("invalid model protocol")?.trim();
                ensure!(!value.is_empty(), "empty model protocol");
                Ok(value)
            })
            .collect::<Result<_>>()?,
    };
    protocols.sort_unstable();
    protocols.dedup();
    if protocols.is_empty() {
        protocols.push("openai-completions");
    }
    if route.protocol.is_empty() {
        ensure!(protocols.len() == 1, "model has ambiguous protocols; select a protocol explicitly");
        route.protocol = protocols[0].into();
    }
    ensure!(protocols.contains(&route.protocol.as_str()), "selected protocol is not advertised by the model");
    if !matches!(route.protocol.as_str(), "openai-completions" | "openrouter-chat") {
        bail!(
            "this compact node supports openai-completions/openrouter-chat; selected protocol is unsupported: {}",
            route.protocol
        );
    }
    let capabilities = crate::model_caps::ModelCaps::parse(row)?;
    Ok((route, client, capabilities))
}

#[cfg(test)]
mod generation_policy_tests {
    use super::*;
    use serde_json::json;

    fn model() -> rpi_ai::Model {
        rpi_ai::Model::new(
            "fixture-model",
            "Fixture",
            rpi_ai::Api::OpenaiCompletions,
            "fixture-provider",
            "https://example.invalid/v1",
        )
    }

    fn route(protocol: &str) -> Route {
        Route {
            model: "fixture-model".into(),
            protocol: protocol.into(),
            binding: "binding-fixture".into(),
            decision: "decision-fixture".into(),
        }
    }

    #[test]
    fn provider_default_and_existing_hosts_leave_models_unchanged() {
        let selected = route("openai-completions");
        assert_eq!(GenerationPolicy::default(), GenerationPolicy::ProviderDefault);
        assert_eq!(CatalogModelHost.generation_policy(&selected).unwrap(), GenerationPolicy::ProviderDefault);
        for params in
            [None, Some(json!({"thinking":{"type":"enabled"},"temperature":0.25}).as_object().unwrap().clone())]
        {
            let mut candidate = model();
            candidate.sampling_params = params;
            let original = candidate.clone();
            GenerationPolicy::ProviderDefault.apply(&mut candidate);
            assert_eq!(candidate, original);
        }
    }

    #[test]
    fn disabled_policy_changes_only_the_fixed_thinking_field() {
        let mut candidate = model();
        candidate.max_tokens = 2048;
        candidate.context_window = 32768;
        candidate.reasoning = true;
        candidate.headers = Some([("X-Fixture".into(), "kept".into())].into());
        candidate.sampling_params = Some(
            json!({"thinking":{"type":"enabled","budget_tokens":1000},
            "temperature":0.25,"response_format":{"type":"json_object"}})
            .as_object()
            .unwrap()
            .clone(),
        );
        let mut expected = candidate.clone();
        expected.sampling_params.as_mut().unwrap().insert("thinking".into(), json!({"type":"disabled"}));
        GenerationPolicy::ThinkingTypeDisabled.validate_route(&route("openai-completions")).unwrap();
        GenerationPolicy::ThinkingTypeDisabled.apply(&mut candidate);
        assert_eq!(candidate, expected);

        let mut empty = model();
        GenerationPolicy::ThinkingTypeDisabled.apply(&mut empty);
        assert_eq!(empty.sampling_params, Some(json!({"thinking":{"type":"disabled"}}).as_object().unwrap().clone()));
    }

    #[test]
    fn disabled_policy_rejects_other_resolved_protocols() {
        for protocol in ["", "openrouter-chat", "anthropic-messages", "openai-responses", "unknown"] {
            assert!(GenerationPolicy::ThinkingTypeDisabled.validate_route(&route(protocol)).is_err());
            // Default policy adds no new protocol gate; normal route admission still owns that check.
            GenerationPolicy::ProviderDefault.validate_route(&route(protocol)).unwrap();
        }
    }

    #[test]
    fn policy_json_accepts_only_the_two_explicit_variants() {
        assert_eq!(serde_json::to_value(GenerationPolicy::ProviderDefault).unwrap(), json!("provider_default"));
        assert_eq!(
            serde_json::from_value::<GenerationPolicy>(json!("thinking_type_disabled")).unwrap(),
            GenerationPolicy::ThinkingTypeDisabled
        );
        for value in [json!("off"), json!("unknown"), json!({"thinking":{"type":"disabled"}}), json!(null)] {
            assert!(serde_json::from_value::<GenerationPolicy>(value).is_err());
        }
    }

    #[test]
    fn policy_validation_preserves_the_four_route_fields() {
        let selected = route("openai-completions");
        let original = selected.clone();
        GenerationPolicy::ThinkingTypeDisabled.validate_route(&selected).unwrap();
        assert_eq!(selected, original);
        assert_eq!(
            serde_json::to_value(&selected).unwrap(),
            json!({"model":"fixture-model",
            "protocol":"openai-completions","binding":"binding-fixture","decision":"decision-fixture"})
        );
    }
}
