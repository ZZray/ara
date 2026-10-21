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

pub struct ModelSelection {
    pub route: Route,
    pub client: reqwest::Client,
    pub capabilities: crate::model_caps::ModelCaps,
    pub headers: BTreeMap<String, String>,
}
#[async_trait]
pub trait ModelHost: Send + Sync {
    async fn resolve(&self, route: Route, endpoint: &str, token: &str, run: &str) -> Result<ModelSelection>;
}
#[derive(Default)]
pub struct CatalogModelHost;
#[async_trait]
impl ModelHost for CatalogModelHost {
    async fn resolve(&self, route: Route, endpoint: &str, token: &str, run: &str) -> Result<ModelSelection> {
        let (route, client, capabilities) = resolve_catalog(route, endpoint, token, run).await?;
        Ok(ModelSelection { route, client, capabilities, headers: BTreeMap::new() })
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
