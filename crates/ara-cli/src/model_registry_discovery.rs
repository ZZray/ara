//! Configured discovery protocols from fixed OMP model-discovery.ts at
//! 596f2da7101178214aa27a753529d15e6b7ad91d. Registry cache/state orchestration
//! and credential retries belong to the Host. Model-list order and duplicates
//! are retained; transport credentials never enter HostModel metadata.
//! MIT: Copyright (c) 2025 Mario Zechner, 2025-2026 Can Bölük,
//! 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.

use crate::{
    catalog_discovery::{
        CatalogContext, DiscoveryError, DiscoveryReply, DiscoveryRequest, DiscoverySignal, DiscoveryTransport,
        HttpMethod, js_lower, zero_cost,
    },
    js_regex::JsRegExp,
    model_collapse::{VariantSpec, trim, truthy},
    model_config_values::{HeaderConfigRecord, HeaderSource},
    model_identity_wire::{
        boolean, bundled_model_reference_index, inherit_reference_thinking, number, resolve_model_reference, text,
    },
    model_patch::{HeaderSlot, HostModelRef, build_host_model, to_host_model_spec},
    model_registry_loader::RegistryDiscoveryConfig,
    provider_models::openai_compat::{
        FetchLiteLLMRichModelsOptions, LmStudioNativeModelMetadataOptions,
        OPENAI_COMPAT_DISCOVERY_DEFAULT_CONTEXT_WINDOW, OPENAI_COMPAT_DISCOVERY_DEFAULT_MAX_TOKENS,
        fetch_litellm_rich_models, fetch_lm_studio_native_model_metadata, resolve_litellm_api,
    },
};
use ara_rpc::{WireString, WireValue};
use async_trait::async_trait;
use futures::future::join_all;
use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

/// The actual Host owns bearer resolution, refresh/rotation and settlement of
/// already-started configuration helpers. Protocol probes supply only resolved
/// configuration headers and bounded per-request signals.
#[async_trait]
pub trait RegistryDiscoveryFetch: Send + Sync {
    async fn fetch(&self, provider: &WireString, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError>;
    /// Private successful-request receipt. Hosts return the original protocol
    /// header record with the successful bearer applied, excluding probe-only
    /// Accept/Content-Type additions. No receipt means the configured fallback.
    fn settled_request_headers(
        &self,
        _: &WireString,
        fallback: &[(WireString, WireString)],
    ) -> Vec<(WireString, WireString)> {
        fallback.to_vec()
    }
}

/// Explicit unauthenticated adapter, also useful for injected native transports.
/// Production Hosts requiring auth implement RegistryDiscoveryFetch themselves.
#[async_trait]
impl RegistryDiscoveryFetch for CatalogContext {
    async fn fetch(&self, _: &WireString, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        self.transport.fetch(request).await
    }
}

struct ConfiguredTransport {
    provider: WireString,
    fetch: Arc<dyn RegistryDiscoveryFetch>,
    context_id: u64,
}
#[async_trait]
impl DiscoveryTransport for ConfiguredTransport {
    fn context_id(&self) -> u64 {
        self.context_id
    }
    async fn fetch(&self, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        self.fetch.fetch(&self.provider, request).await
    }
}

pub async fn discover_configured_models(
    config: &RegistryDiscoveryConfig,
    context: &CatalogContext,
    fetch: Arc<dyn RegistryDiscoveryFetch>,
) -> Result<Vec<HostModelRef>, DiscoveryError> {
    let transport = Arc::new(ConfiguredTransport {
        provider: config.provider.clone(),
        fetch: fetch.clone(),
        context_id: context.transport.context_id(),
    });
    let mut native_context = context.clone();
    native_context.transport = transport.clone();
    match config.kind().and_then(|kind| kind.to_utf8().ok()).as_deref() {
        Some("lm-studio" | "openai-models-list") => {
            discover_openai_models_list(config, &native_context, fetch.as_ref()).await
        }
        Some("ollama") => discover_ollama_models(config, fetch.as_ref()).await,
        Some("proxy") => discover_proxy_models(config, fetch.as_ref()).await,
        Some("litellm") => discover_litellm_models(config, &native_context, &transport).await,
        Some("llama.cpp") => discover_llama_cpp_models(config, fetch.as_ref()).await,
        _ => Err(DiscoveryError::named("UnsupportedDiscovery", "configured discovery protocol is unknown")),
    }
}

fn configured_headers(config: &RegistryDiscoveryConfig) -> Result<Vec<(WireString, WireString)>, DiscoveryError> {
    match config.headers() {
        HeaderSlot::Source(HeaderSource::Config(headers)) => {
            Ok(headers.snapshot().into_iter().map(|(name, value)| (name.into(), value.into())).collect())
        }
        HeaderSlot::Source(HeaderSource::Live(_)) => {
            Err(DiscoveryError::new("configured discovery headers must be settled by the Host"))
        }
        _ => Ok(Vec::new()),
    }
}

fn literal_sidecar(headers: &[(WireString, WireString)]) -> Result<HeaderSlot, DiscoveryError> {
    let values = headers
        .iter()
        .map(|(name, value)| {
            Ok((
                name.to_utf8().map_err(|_| DiscoveryError::new("configured header name is not UTF-8"))?,
                value.to_utf8().map_err(|_| DiscoveryError::new("configured header value is not UTF-8"))?,
            ))
        })
        .collect::<Result<Vec<_>, DiscoveryError>>()?;
    Ok(HeaderSlot::Source(HeaderSource::Config(HeaderConfigRecord::from_pairs(values))))
}

fn settled_sidecar(
    config: &RegistryDiscoveryConfig,
    fetch: &dyn RegistryDiscoveryFetch,
    fallback: &[(WireString, WireString)],
) -> Result<HeaderSlot, DiscoveryError> {
    literal_sidecar(&fetch.settled_request_headers(&config.provider, fallback))
}

fn timeout_ms(config: &RegistryDiscoveryConfig, fallback: f64) -> f64 {
    number(&config.discovery, "timeoutMs").unwrap_or(fallback)
}

/// Independent deadline, including a transport that ignores its abort signal.
/// Timeout signals are endpoint scoped and do not abort unrelated probes.
async fn fetch_bounded(
    fetch: &dyn RegistryDiscoveryFetch,
    provider: &WireString,
    mut request: DiscoveryRequest,
    timeout_ms: f64,
) -> Result<DiscoveryReply, DiscoveryError> {
    let signal = DiscoverySignal::default();
    request.signal = Some(signal.clone());
    bounded(timeout_ms, &signal, fetch.fetch(provider, request)).await
}

async fn bounded<T>(
    timeout_ms: f64,
    signal: &DiscoverySignal,
    future: impl Future<Output = Result<T, DiscoveryError>>,
) -> Result<T, DiscoveryError> {
    let wait = Duration::from_secs_f64((timeout_ms.max(0.0) / 1000.0).min(9_223_372_036.0));
    tokio::select! {
        reply = future => reply,
        _ = tokio::time::sleep(wait) => {
            let error = DiscoveryError::named("TimeoutError", "The operation timed out.");
            signal.abort(error.clone());
            Err(error)
        }
    }
}

async fn fetch_json(
    fetch: &dyn RegistryDiscoveryFetch,
    provider: &WireString,
    request: DiscoveryRequest,
    timeout: f64,
) -> Result<VariantSpec, DiscoveryError> {
    let url = request.url.clone();
    let reply = fetch_bounded(fetch, provider, request, timeout).await?;
    if !reply.ok() {
        let mut units: Vec<u16> = format!("HTTP {} from ", reply.status).encode_utf16().collect();
        units.extend_from_slice(url.units());
        return Err(DiscoveryError::new(WireString::from_units(units)));
    }
    reply.json()
}

fn url_root(url: &reqwest::Url) -> String {
    let host = url.host_str().unwrap_or("");
    let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.into() };
    let port = url.port().map_or_else(String::new, |port| format!(":{port}"));
    format!("{}://{host}{port}", url.scheme())
}

pub fn normalize_openai_discovery_base_url(base: Option<&WireString>, inject_v1: bool) -> WireString {
    let default = if inject_v1 { "http://127.0.0.1:1234/v1" } else { "http://127.0.0.1:1234" };
    let raw =
        base.filter(|base| !base.is_empty()).and_then(|base| base.to_utf8().ok()).unwrap_or_else(|| default.into());
    if let Ok(mut url) = reqwest::Url::parse(&raw) {
        let mut path = url.path().trim_end_matches('/').to_owned();
        if inject_v1 && !path.ends_with("/v1") {
            path.push_str("/v1");
        }
        if inject_v1 {
            url.set_path(&path);
            path = url.path().into();
        }
        format!("{}{}", url_root(&url), path).into()
    } else if inject_v1 {
        raw.into()
    } else {
        raw.trim_end_matches('/').into()
    }
}

fn append_models_path(base: &WireString) -> WireString {
    let raw = String::from_utf16_lossy(base.units());
    if let Ok(mut url) = reqwest::Url::parse(&raw) {
        let path = format!("{}/models", url.path().trim_end_matches('/'));
        url.set_path(&path);
        url.as_str().into()
    } else {
        let mut url = base.clone();
        url.append_str("/models");
        url
    }
}

fn positive(value: Option<&WireValue>) -> Option<f64> {
    let value = match value? {
        WireValue::Number(value) => *value,
        WireValue::String(value) if !trim(value).is_empty() => {
            ara_prompt::js::to_number(&serde_json::Value::String(value.to_utf8().ok()?))
        }
        _ => return None,
    };
    (value.is_finite() && value > 0.0).then_some(value)
}

fn model_rows(payload: &VariantSpec, field: &str) -> Result<Vec<VariantSpec>, DiscoveryError> {
    match payload.get(field) {
        None | Some(WireValue::Null) => Ok(Vec::new()),
        Some(WireValue::Array(rows)) => rows
            .iter()
            .map(|row| {
                if matches!(row, WireValue::Null) {
                    Err(DiscoveryError::named("TypeError", "configured model list contains a null row"))
                } else {
                    Ok(VariantSpec::from_wire(row.clone()))
                }
            })
            .collect(),
        _ => Err(DiscoveryError::named("TypeError", "configured model list is not an array")),
    }
}

fn row_id(row: &VariantSpec, field: &str) -> Result<Option<WireString>, DiscoveryError> {
    match row.get(field) {
        None => Ok(None),
        Some(value) if !truthy(value) => Ok(None),
        Some(WireValue::String(id)) => Ok(Some(id.clone())),
        _ => Err(DiscoveryError::named("TypeError", "configured model identifier is not a string")),
    }
}

fn capabilities(row: &VariantSpec) -> Option<WireValue> {
    let architecture = row.record("architecture");
    let mut modalities = Vec::new();
    for value in [
        row.get("input"),
        row.get("input_modalities"),
        architecture.as_ref().and_then(|row| row.get("input_modalities")),
    ] {
        for value in value.and_then(WireValue::as_array).into_iter().flatten() {
            if let Some(value) = value.as_string() {
                modalities.push(js_lower(value));
            }
        }
    }
    if modalities.is_empty() { None } else { Some(input(modalities.iter().any(|value| value.equals_ascii("image")))) }
}

fn input(image: bool) -> WireValue {
    WireValue::Array(if image {
        vec![WireValue::String("text".into()), WireValue::String("image".into())]
    } else {
        vec![WireValue::String("text".into())]
    })
}

fn discovery_default_max_tokens(api: &WireString) -> f64 {
    if api.equals_ascii("anthropic-messages") { 8192.0 } else { OPENAI_COMPAT_DISCOVERY_DEFAULT_MAX_TOKENS }
}

fn base_spec(config: &RegistryDiscoveryConfig, id: &WireString, base_url: &WireString) -> VariantSpec {
    let mut spec = VariantSpec::from_wire(WireValue::Object(Vec::new()));
    for (field, value) in
        [("id", id), ("name", id), ("provider", &config.provider), ("api", &config.api), ("baseUrl", base_url)]
    {
        spec.set(field, WireValue::String(value.clone()));
    }
    spec.set("reasoning", WireValue::Bool(false));
    spec.set("input", input(false));
    spec.set("cost", zero_cost());
    spec
}

async fn lm_studio_metadata(
    config: &RegistryDiscoveryConfig,
    base: &WireString,
    headers: &[(WireString, WireString)],
    context: &CatalogContext,
    timeout: f64,
) -> Result<Option<HashMap<WireString, VariantSpec>>, DiscoveryError> {
    let _ = config;
    let signal = DiscoverySignal::default();
    bounded(
        timeout,
        &signal,
        fetch_lm_studio_native_model_metadata(
            context,
            base,
            LmStudioNativeModelMetadataOptions { headers: headers.to_vec(), signal: Some(signal.clone()) },
        ),
    )
    .await
}

async fn discover_litellm_models(
    config: &RegistryDiscoveryConfig,
    context: &CatalogContext,
    transport: &ConfiguredTransport,
) -> Result<Vec<HostModelRef>, DiscoveryError> {
    let default: WireString = "http://localhost:4000/v1".into();
    let base = normalize_openai_discovery_base_url(config.base_url.as_ref().or(Some(&default)), true);
    let headers = configured_headers(config)?;
    let mut options = FetchLiteLLMRichModelsOptions::new(config.api.clone(), config.provider.clone(), base.clone());
    options.headers = headers.clone();
    options.reference_resolver = Some(Arc::new(|id| resolve_model_reference(id, bundled_model_reference_index())));
    let fallback = config.api.clone();
    options.resolve_api =
        Some(Arc::new(move |entry, id| Ok(Some(resolve_litellm_api(Some(entry), id, Some(&fallback))))));
    let signal = DiscoverySignal::default();
    options.signal = Some(signal.clone());
    let rich = bounded(timeout_ms(config, 10000.0), &signal, fetch_litellm_rich_models(context, &options)).await?;
    let Some(rich) = rich.filter(|models| !models.is_empty()) else {
        let mut fallback = config.clone();
        fallback.base_url = Some(base);
        return discover_openai_models_list(&fallback, context, transport.fetch.as_ref()).await;
    };
    let headers = settled_sidecar(config, transport.fetch.as_ref(), &headers)?;
    rich.into_iter()
        .map(|spec| {
            let mut spec = spec.as_ref().clone();
            spec.remove("headers");
            build_host_model(&spec, headers.clone()).map_err(Into::into)
        })
        .collect()
}

async fn discover_openai_models_list(
    config: &RegistryDiscoveryConfig,
    context: &CatalogContext,
    fetch: &dyn RegistryDiscoveryFetch,
) -> Result<Vec<HostModelRef>, DiscoveryError> {
    let base = normalize_openai_discovery_base_url(
        config.base_url.as_ref(),
        boolean(&config.discovery, "injectV1") != Some(false),
    );
    let headers = configured_headers(config)?;
    let timeout = timeout_ms(config, 10000.0);
    let metadata = async {
        if config.kind().is_some_and(|kind| kind.equals_ascii("lm-studio")) {
            lm_studio_metadata(config, &base, &headers, context, timeout).await
        } else {
            Ok(None)
        }
    };
    let payload = fetch_json(
        fetch,
        &config.provider,
        DiscoveryRequest { url: append_models_path(&base), headers: headers.clone(), ..Default::default() },
        timeout,
    );
    // Native starts the LM Studio metadata promise before the /models fetch.
    let (metadata, payload) = tokio::try_join!(metadata, payload)?;
    let headers = settled_sidecar(config, fetch, &headers)?;
    let references = bundled_model_reference_index();
    let mut models = Vec::new();
    for row in model_rows(&payload, "data")? {
        let Some(id) = row_id(&row, "id")? else { continue };
        let native = metadata.as_ref().and_then(|metadata| metadata.get(&id));
        let reference = resolve_model_reference(&id, references);
        let reference = reference.as_deref();
        let api = if config.kind().is_some_and(|kind| kind.equals_ascii("litellm")) {
            resolve_litellm_api(None, &id, Some(&config.api))
        } else {
            config.api.clone()
        };
        let window = positive(row.get("max_model_len"))
            .or_else(|| positive(row.get("context_length")))
            .or_else(|| native.and_then(|metadata| number(metadata, "contextWindow")))
            .or_else(|| reference.and_then(|reference| number(reference, "contextWindow")))
            .unwrap_or(OPENAI_COMPAT_DISCOVERY_DEFAULT_CONTEXT_WINDOW);
        let mut spec = base_spec(config, &id, &base);
        spec.set("api", WireValue::String(api.clone()));
        if let Some(name) = reference.and_then(|reference| text(reference, "name")) {
            spec.set("name", WireValue::String(name));
        }
        spec.set(
            "reasoning",
            WireValue::Bool(reference.and_then(|reference| boolean(reference, "reasoning")).unwrap_or(false)),
        );
        if let Some(thinking) = inherit_reference_thinking(None, reference, &config.provider) {
            spec.set_record("thinking", &thinking);
        }
        let modality = native
            .and_then(|metadata| metadata.get("input"))
            .cloned()
            .or_else(|| capabilities(&row))
            .or_else(|| reference.and_then(|reference| reference.get("input")).cloned())
            .unwrap_or_else(|| input(false));
        spec.set("input", modality);
        if config.kind().is_some_and(|kind| kind.equals_ascii("lm-studio")) {
            spec.set("imageInputDecoder", WireValue::String("stb".into()));
        }
        spec.set("contextWindow", WireValue::Number(window));
        spec.set(
            "maxTokens",
            WireValue::Number(
                reference
                    .and_then(|reference| number(reference, "maxTokens"))
                    .unwrap_or_else(|| discovery_default_max_tokens(&api))
                    .min(window),
            ),
        );
        let reference_compat = reference.and_then(|reference| reference.record("compat"));
        let mut compat = basic_compat();
        compat.set(
            "supportsReasoningEffort",
            reference_compat
                .as_ref()
                .and_then(|compat| compat.get("supportsReasoningEffort"))
                .filter(|value| !matches!(value, WireValue::Null))
                .cloned()
                .unwrap_or(WireValue::Bool(false)),
        );
        if let Some(value) =
            reference_compat.as_ref().and_then(|compat| compat.get("reasoningEffortMap")).filter(|value| truthy(value))
        {
            compat.set("reasoningEffortMap", value.clone());
        }
        if let Some(value) = reference_compat.as_ref().and_then(|compat| compat.get("omitReasoningEffort")) {
            compat.set("omitReasoningEffort", value.clone());
        }
        spec.set_record("compat", &compat);
        models.push(build_host_model(&spec, headers.clone())?);
    }
    Ok(models)
}

fn basic_compat() -> VariantSpec {
    VariantSpec::from_wire(WireValue::Object(vec![
        ("supportsStore".into(), WireValue::Bool(false)),
        ("supportsDeveloperRole".into(), WireValue::Bool(false)),
        ("supportsReasoningEffort".into(), WireValue::Bool(false)),
    ]))
}

async fn discover_proxy_models(
    config: &RegistryDiscoveryConfig,
    fetch: &dyn RegistryDiscoveryFetch,
) -> Result<Vec<HostModelRef>, DiscoveryError> {
    let base = normalize_openai_discovery_base_url(config.base_url.as_ref(), true);
    let headers = configured_headers(config)?;
    let payload = fetch_json(
        fetch,
        &config.provider,
        DiscoveryRequest { url: append_models_path(&base), headers: headers.clone(), ..Default::default() },
        timeout_ms(config, 10000.0),
    )
    .await?;
    let headers = settled_sidecar(config, fetch, &headers)?;
    let mut models = Vec::new();
    for row in model_rows(&payload, "data")? {
        let Some(id) = row_id(&row, "id")? else { continue };
        let endpoints = row.get("supported_endpoint_types").and_then(WireValue::as_array).unwrap_or(&[]);
        let supports = |endpoint: &str| {
            endpoints.iter().any(|value| value.as_string().is_some_and(|value| value.equals_ascii(endpoint)))
        };
        let api = if supports("anthropic") {
            "anthropic-messages".into()
        } else if supports("openai") {
            "openai-completions".into()
        } else {
            config.api.clone()
        };
        let reference = resolve_model_reference(&id, bundled_model_reference_index());
        let reference = reference.as_deref();
        let display = text(&row, "name")
            .map(|name| trim(&name))
            .filter(|name| !name.is_empty() && name != &id)
            .or_else(|| reference.and_then(|reference| text(reference, "name")))
            .or_else(|| crate::model_identity_wire::strip_bracketed_model_id_affixes(&id))
            .unwrap_or_else(|| id.clone());
        let mut spec = base_spec(config, &id, &base);
        spec.set("name", WireValue::String(display));
        spec.set("api", WireValue::String(api.clone()));
        spec.set(
            "reasoning",
            WireValue::Bool(reference.and_then(|reference| boolean(reference, "reasoning")).unwrap_or(false)),
        );
        if let Some(thinking) = inherit_reference_thinking(None, reference, &config.provider) {
            spec.set_record("thinking", &thinking);
        }
        spec.set(
            "input",
            reference.and_then(|reference| reference.get("input")).cloned().unwrap_or_else(|| input(false)),
        );
        spec.set(
            "contextWindow",
            WireValue::Number(
                positive(row.get("context_length"))
                    .or_else(|| reference.and_then(|reference| number(reference, "contextWindow")))
                    .unwrap_or(OPENAI_COMPAT_DISCOVERY_DEFAULT_CONTEXT_WINDOW),
            ),
        );
        spec.set(
            "maxTokens",
            WireValue::Number(
                reference
                    .and_then(|reference| number(reference, "maxTokens"))
                    .unwrap_or_else(|| discovery_default_max_tokens(&api)),
            ),
        );
        if !api.equals_ascii("anthropic-messages") {
            spec.set_record("compat", &basic_compat());
        }
        models.push(build_host_model(&spec, headers.clone())?);
    }
    Ok(models)
}

pub fn discovery_probe_timeout_ms(base: &WireString, loopback_ms: f64, custom: Option<f64>) -> f64 {
    if let Some(custom) = custom.filter(|custom| custom.is_finite() && *custom > 0.0) {
        return custom;
    }
    let Ok(url) = reqwest::Url::parse(&String::from_utf16_lossy(base.units())) else { return loopback_ms };
    let hostname = url.host_str().unwrap_or("").trim_matches(['[', ']']);
    if ["localhost", "0.0.0.0", "::1"].contains(&hostname) || hostname.starts_with("127.") {
        loopback_ms
    } else {
        10000.0
    }
}

fn ollama_context(payload: &VariantSpec) -> Option<f64> {
    if let Some(parameters) = text(payload, "parameters") {
        let mut pattern =
            JsRegExp::new(r"(?:^|\n)\s*num_ctx\s+(\d+)\s*(?:$|\n)".into(), "m").expect("fixed Ollama pattern");
        if let Some(parsed) = pattern
            .exec(&parameters)
            .and_then(|found| found.captures.get(1).cloned().flatten())
            .and_then(|value| positive(Some(&WireValue::String(value))))
        {
            return Some(parsed);
        }
    }
    payload.record("model_info").filter(|info| matches!(info.value, WireValue::Object(_))).and_then(|info| {
        info.own_keys()
            .into_iter()
            .filter(|key| {
                key.equals_ascii("context_length")
                    || key.units().ends_with(&".context_length".encode_utf16().collect::<Vec<_>>())
            })
            .find_map(|key| positive(info.get_path(std::slice::from_ref(&key))))
    })
}

async fn ollama_metadata(
    config: &RegistryDiscoveryConfig,
    endpoint: &WireString,
    id: &WireString,
    headers: &[(WireString, WireString)],
    fetch: &dyn RegistryDiscoveryFetch,
) -> Option<VariantSpec> {
    let mut url = endpoint.clone();
    url.append_str("/api/show");
    let mut headers = headers.to_vec();
    crate::catalog_discovery::openai::set_record_header(&mut headers, "Content-Type".into(), "application/json".into());
    let payload = fetch_json(
        fetch,
        &config.provider,
        DiscoveryRequest {
            url,
            method: HttpMethod::Post,
            headers,
            body: Some(
                WireValue::Object(vec![("model".into(), WireValue::String(id.clone()))]).stringify().into_bytes(),
            ),
            ..Default::default()
        },
        discovery_probe_timeout_ms(endpoint, 150.0, number(&config.discovery, "timeoutMs")),
    )
    .await
    .ok()?;
    if !matches!(payload.value, WireValue::Object(_)) {
        return None;
    }
    let mut metadata = VariantSpec::from_wire(WireValue::Object(Vec::new()));
    if let Some(window) = ollama_context(&payload) {
        metadata.set("contextWindow", WireValue::Number(window));
    }
    let (reasoning, image) = match payload.get("capabilities") {
        Some(WireValue::Array(values)) => {
            let values: Vec<_> = values.iter().filter_map(WireValue::as_string).map(js_lower).collect();
            (
                values.iter().any(|value| value.equals_ascii("thinking")),
                values.iter().any(|value| value.equals_ascii("vision") || value.equals_ascii("image")),
            )
        }
        Some(WireValue::Object(_)) => {
            let capabilities = payload.record("capabilities")?;
            (
                boolean(&capabilities, "thinking") == Some(true),
                boolean(&capabilities, "vision") == Some(true) || boolean(&capabilities, "image") == Some(true),
            )
        }
        _ => (false, false),
    };
    metadata.set("reasoning", WireValue::Bool(reasoning));
    metadata.set("input", input(image));
    Some(metadata)
}

async fn discover_ollama_models(
    config: &RegistryDiscoveryConfig,
    fetch: &dyn RegistryDiscoveryFetch,
) -> Result<Vec<HostModelRef>, DiscoveryError> {
    let endpoint = config
        .base_url
        .as_ref()
        .and_then(|base| base.to_utf8().ok())
        .filter(|base| !base.is_empty())
        .and_then(|base| reqwest::Url::parse(&base).ok())
        .map(|url| WireString::from(url_root(&url)))
        .unwrap_or_else(|| "http://127.0.0.1:11434".into());
    let headers = configured_headers(config)?;
    let mut url = endpoint.clone();
    url.append_str("/api/tags");
    let payload = fetch_json(
        fetch,
        &config.provider,
        DiscoveryRequest { url, headers: headers.clone(), ..Default::default() },
        discovery_probe_timeout_ms(&endpoint, 250.0, number(&config.discovery, "timeoutMs")),
    )
    .await?;
    let mut entries = Vec::new();
    for row in model_rows(&payload, "models")? {
        let id = row_id(&row, "model")?.or(row_id(&row, "name")?);
        if let Some(id) = id {
            let name = row_id(&row, "name")?.unwrap_or_else(|| id.clone());
            entries.push((id, name));
        }
    }
    // Map semantics intentionally retain the metadata from the final duplicate
    // ID, while the returned list itself preserves every input entry and order.
    let metadata: HashMap<_, _> = join_all(
        entries
            .iter()
            .map(|(id, _)| async { (id.clone(), ollama_metadata(config, &endpoint, id, &headers, fetch).await) }),
    )
    .await
    .into_iter()
    .collect();
    let headers = settled_sidecar(config, fetch, &headers)?;
    let mut base = endpoint;
    base.append_str("/v1");
    entries
        .into_iter()
        .map(|(id, name)| {
            let metadata = metadata.get(&id).and_then(Option::as_ref);
            let window = metadata.and_then(|metadata| number(metadata, "contextWindow"));
            let mut spec = base_spec(config, &id, &base);
            spec.set("name", WireValue::String(name));
            spec.set(
                "reasoning",
                WireValue::Bool(metadata.and_then(|metadata| boolean(metadata, "reasoning")).unwrap_or(false)),
            );
            spec.set(
                "input",
                metadata.and_then(|metadata| metadata.get("input")).cloned().unwrap_or_else(|| input(false)),
            );
            spec.set("imageInputDecoder", WireValue::String("stb".into()));
            spec.set(
                "contextWindow",
                WireValue::Number(window.unwrap_or(OPENAI_COMPAT_DISCOVERY_DEFAULT_CONTEXT_WINDOW)),
            );
            spec.set(
                "maxTokens",
                WireValue::Number(window.unwrap_or(f64::INFINITY).min(OPENAI_COMPAT_DISCOVERY_DEFAULT_MAX_TOKENS)),
            );
            build_host_model(&spec, headers.clone()).map_err(Into::into)
        })
        .collect()
}

pub fn normalize_llama_cpp_base_url(base: Option<&WireString>) -> WireString {
    let raw = base
        .filter(|base| !base.is_empty())
        .and_then(|base| base.to_utf8().ok())
        .unwrap_or_else(|| "http://127.0.0.1:8080".into());
    if let Ok(url) = reqwest::Url::parse(&raw) {
        format!("{}{}", url_root(&url), url.path().trim_end_matches('/')).into()
    } else {
        raw.into()
    }
}

pub fn ensure_llama_cpp_v1_base_url(base: &WireString) -> WireString {
    let mut base = base.clone();
    if !base.units().ends_with(&[47, 118, 49]) {
        base.append_str("/v1");
    }
    base
}

fn llama_native_base(base: &WireString) -> WireString {
    let mut base = normalize_llama_cpp_base_url(Some(base));
    if base.units().ends_with(&[47, 118, 49]) {
        base = base.slice_prefix(base.len() - 3);
    }
    base
}

/// Used for fresh discovery and upgraded cache rows by the Host's hardcoded
/// policy stage. This preserves the opaque transport-header owner.
pub fn apply_llama_cpp_qwen_thinking(model: &HostModelRef) -> Result<HostModelRef, DiscoveryError> {
    let qwen = model
        .spec()
        .record("identity")
        .and_then(|identity| text(&identity, "class"))
        .is_some_and(|class| class.equals_ascii("qwen"));
    let bonsai = text(model.spec(), "id").is_some_and(|id| {
        let id = js_lower(&id);
        let needle: Vec<_> = "bonsai-27b".encode_utf16().collect();
        id.units().windows(needle.len()).any(|part| part == needle)
    });
    if !qwen && !bonsai {
        return Ok(model.clone());
    }
    let mut spec = to_host_model_spec(model);
    spec.set("api", WireValue::String("openai-completions".into()));
    if !spec.get("transport").is_some_and(truthy) {
        let base = normalize_llama_cpp_base_url(text(&spec, "baseUrl").as_ref());
        spec.set("baseUrl", WireValue::String(ensure_llama_cpp_v1_base_url(&base)));
    }
    spec.set("reasoning", WireValue::Bool(true));
    let mut compat = model.spec().record("compatConfig").unwrap_or_else(basic_compat);
    for (field, value) in [
        ("supportsReasoningParams", WireValue::Bool(true)),
        ("thinkingFormat", WireValue::String("qwen-chat-template".into())),
        ("reasoningDisableMode", WireValue::String("qwen-template-false".into())),
        ("qwenPreserveThinking", WireValue::Bool(true)),
    ] {
        compat.set(field, value);
    }
    spec.set_record("compat", &compat);
    build_host_model(&spec, model.headers().clone()).map_err(Into::into)
}

fn llama_configured_window(row: &VariantSpec) -> Option<f64> {
    let status = row.record("status").filter(|status| matches!(status.value, WireValue::Object(_)))?;
    if let Some(args) = status.get("args").and_then(WireValue::as_array) {
        for (index, value) in args.iter().enumerate() {
            let Some(value) = value.as_string() else { continue };
            let eq = value.units().iter().position(|unit| *unit == 61);
            let flag = value.slice_prefix(eq.unwrap_or(value.len()));
            if !flag.equals_ascii("--ctx-size") && !flag.equals_ascii("-c") {
                continue;
            }
            let argument = eq
                .map(|eq| WireValue::String(WireString::from_units(value.units()[eq + 1..].to_vec())))
                .or_else(|| args.get(index + 1).cloned());
            if let Some(window) = positive(argument.as_ref()) {
                return Some(window);
            }
        }
    }
    let preset = text(&status, "preset")?;
    let mut pattern =
        JsRegExp::new(r"(?:^|\n)\s*ctx-size\s*=\s*(-?\d+)\s*(?:$|\n)".into(), "").expect("fixed llama.cpp INI pattern");
    pattern
        .exec(&preset)
        .and_then(|found| found.captures.get(1).cloned().flatten())
        .and_then(|value| positive(Some(&WireValue::String(value))))
}

fn llama_unlimited(payload: &VariantSpec) -> bool {
    let generation =
        payload.record("default_generation_settings").filter(|value| matches!(value.value, WireValue::Object(_)));
    let params = generation
        .as_ref()
        .and_then(|generation| generation.record("params"))
        .filter(|value| matches!(value.value, WireValue::Object(_)));
    [
        params.as_ref().and_then(|params| params.get("max_tokens")),
        params.as_ref().and_then(|params| params.get("n_predict")),
        generation.as_ref().and_then(|generation| generation.get("max_tokens")),
        generation.as_ref().and_then(|generation| generation.get("n_predict")),
        payload.get("max_tokens"),
        payload.get("n_predict"),
    ]
    .into_iter()
    .flatten()
    .any(|value| match value {
        WireValue::Number(value) => *value == -1.0,
        WireValue::String(value) if !trim(value).is_empty() => value
            .to_utf8()
            .ok()
            .is_some_and(|value| ara_prompt::js::to_number(&serde_json::Value::String(value)) == -1.0),
        _ => false,
    })
}

async fn llama_server_metadata(
    config: &RegistryDiscoveryConfig,
    base: &WireString,
    headers: &[(WireString, WireString)],
    fetch: &dyn RegistryDiscoveryFetch,
) -> Option<VariantSpec> {
    let mut url = llama_native_base(base);
    url.append_str("/props");
    let payload = fetch_json(
        fetch,
        &config.provider,
        DiscoveryRequest { url, headers: headers.to_vec(), ..Default::default() },
        discovery_probe_timeout_ms(base, 150.0, number(&config.discovery, "timeoutMs")),
    )
    .await
    .ok()?;
    matches!(payload.value, WireValue::Object(_)).then_some(payload)
}

async fn discover_llama_cpp_models(
    config: &RegistryDiscoveryConfig,
    fetch: &dyn RegistryDiscoveryFetch,
) -> Result<Vec<HostModelRef>, DiscoveryError> {
    let base = normalize_llama_cpp_base_url(config.base_url.as_ref());
    let headers = configured_headers(config)?;
    let payload = fetch_json(
        fetch,
        &config.provider,
        DiscoveryRequest { url: append_models_path(&base), headers: headers.clone(), ..Default::default() },
        discovery_probe_timeout_ms(&base, 250.0, number(&config.discovery, "timeoutMs")),
    );
    let metadata = llama_server_metadata(config, &base, &headers, fetch);
    let (payload, metadata) = tokio::join!(payload, metadata);
    let payload = payload?;
    if !matches!(payload.value, WireValue::Object(_)) {
        return Ok(Vec::new());
    }
    let Some(rows) = payload.get("data").and_then(WireValue::as_array) else { return Ok(Vec::new()) };
    let generation = metadata
        .as_ref()
        .and_then(|metadata| metadata.record("default_generation_settings"))
        .filter(|value| matches!(value.value, WireValue::Object(_)));
    let server_window = generation
        .as_ref()
        .and_then(|generation| positive(generation.get("n_ctx")))
        .or_else(|| metadata.as_ref().and_then(|metadata| positive(metadata.get("n_ctx"))));
    let server_input = metadata
        .as_ref()
        .and_then(|metadata| metadata.record("modalities"))
        .filter(|value| matches!(value.value, WireValue::Object(_)))
        .map(|modalities| input(boolean(&modalities, "vision") == Some(true)));
    let unlimited = metadata.as_ref().is_some_and(llama_unlimited);
    let headers = settled_sidecar(config, fetch, &headers)?;
    let model_base = ensure_llama_cpp_v1_base_url(&base);
    let mut models = Vec::new();
    for row in rows {
        if !matches!(row, WireValue::Object(_)) {
            continue;
        }
        let row = VariantSpec::from_wire(row.clone());
        let Some(id) = text(&row, "id").filter(|id| !id.is_empty()) else { continue };
        let meta = row.record("meta").filter(|value| matches!(value.value, WireValue::Object(_)));
        let window = meta
            .as_ref()
            .and_then(|meta| positive(meta.get("n_ctx")))
            .or_else(|| llama_configured_window(&row))
            .or(server_window)
            .or_else(|| meta.as_ref().and_then(|meta| positive(meta.get("n_ctx_train"))))
            .unwrap_or(OPENAI_COMPAT_DISCOVERY_DEFAULT_CONTEXT_WINDOW);
        let modality = row
            .record("architecture")
            .filter(|value| matches!(value.value, WireValue::Object(_)))
            .and_then(|architecture| {
                architecture.get("input_modalities").and_then(WireValue::as_array).map(|modalities| {
                    input(
                        modalities
                            .iter()
                            .filter_map(WireValue::as_string)
                            .map(js_lower)
                            .any(|modality| modality.equals_ascii("image")),
                    )
                })
            })
            .or_else(|| server_input.clone())
            .unwrap_or_else(|| input(false));
        let mut spec = base_spec(config, &id, &model_base);
        spec.set("input", modality);
        spec.set("imageInputDecoder", WireValue::String("stb".into()));
        spec.set("contextWindow", WireValue::Number(window));
        spec.set(
            "maxTokens",
            WireValue::Number(if unlimited { window } else { window.min(OPENAI_COMPAT_DISCOVERY_DEFAULT_MAX_TOKENS) }),
        );
        spec.set_record("compat", &basic_compat());
        models.push(apply_llama_cpp_qwen_thinking(&build_host_model(&spec, headers.clone())?)?);
    }
    Ok(models)
}
