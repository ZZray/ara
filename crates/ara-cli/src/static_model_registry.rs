//! Bounded pure static composition from fixed OMP `model-registry.ts`.
//!
//! Source: 596f2da7101178214aa27a753529d15e6b7ad91d. This adapter composes
//! bundled models, explicit already-resolved cache/runtime snapshots and local
//! overlays. It performs no config/cache I/O, discovery, key installation,
//! command execution, credential projection, runtime extension registration or
//! llama.cpp discovery fixups. Those Host-owned loader/runtime surfaces remain
//! separate. Config-only construction uses raw header presence rather than the
//! full loader's eagerly resolved header-presence test; no eager loader parity
//! is claimed. Catalog availability does not authorize an inference route.
//!
//! MIT License; Copyright (c) 2025 Mario Zechner;
//! Copyright (c) 2025-2026 Can Bölük; Copyright (c) 2026 Stencil Labs, Inc.
//! See LICENSE for the full license.

use crate::{
    custom_models::{
        CustomModelBuildOptions, CustomModelOverlay, build_custom_model_overlay, finalize_custom_model,
        resolve_model_override_with_aliases,
    },
    model_collapse::{CollapseError, CollapseRuntime, SpecRef, VariantSpec, lower, trim, truthy},
    model_config_values::{HeaderConfigRecord, HeaderResolutionOptions, HeaderSource},
    model_identity_wire::{
        CatalogMetricsIndex, apply_catalog_metrics, boolean, bundled_models, copy_field, empty, nullish, number,
        spread, text,
    },
    model_patch::{
        HeaderSlot, HostModel, HostModelRef, ModelPatch, ModelTransportPolicy, OrderedProviderSet, ProviderOverride,
        apply_model_override, apply_model_patch, build_host_model, drop_provider_models, merge_by_model_key,
        merge_compat, merge_discovered_model, merge_header_sources, merge_provider_remote_compaction_config,
        to_host_model_spec,
    },
    models_config::ModelsConfig,
};
use ara_rpc::{WireString, WireValue};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, OnceLock},
};

/// Every catalog row here is an already materialized Model, not a ModelSpec.
/// Cache/runtime inputs are explicit snapshots; this type does not read stores.
/// Secret-bearing model/overlay wrappers intentionally have no Debug/Serialize.
pub struct StaticRegistryInputs {
    pub bundled_models: Vec<HostModelRef>,
    pub pending_standard_providers: OrderedProviderSet,
    pub cached_standard_models: Vec<HostModelRef>,
    pub cached_discoverable_models: Vec<HostModelRef>,
    pub runtime_discovered_models: Vec<HostModelRef>,
    pub runtime_model_overlays: Vec<CustomModelOverlay>,
    pub cached_authoritative_providers: OrderedProviderSet,
    pub runtime_authoritative_providers: OrderedProviderSet,
    pub metrics_models: Vec<SpecRef>,
    pub extended_context: bool,
    pub disabled_providers: OrderedProviderSet,
    pub llama_cpp_providers: OrderedProviderSet,
}

impl Default for StaticRegistryInputs {
    fn default() -> Self {
        Self {
            bundled_models: Vec::new(),
            pending_standard_providers: OrderedProviderSet::default(),
            cached_standard_models: Vec::new(),
            cached_discoverable_models: Vec::new(),
            runtime_discovered_models: Vec::new(),
            runtime_model_overlays: Vec::new(),
            cached_authoritative_providers: OrderedProviderSet::default(),
            runtime_authoritative_providers: OrderedProviderSet::default(),
            metrics_models: Vec::new(),
            extended_context: true,
            disabled_providers: OrderedProviderSet::default(),
            llama_cpp_providers: OrderedProviderSet::default(),
        }
    }
}

struct ConfiguredProvider {
    headers: HeaderSlot,
    api_key_config: Option<String>,
}

#[derive(Default)]
struct ParsedConfig {
    providers: HashMap<WireString, ConfiguredProvider>,
    provider_overrides: HashMap<WireString, ProviderOverride>,
    model_overrides: HashMap<WireString, HashMap<WireString, ModelPatch>>,
    custom_models: Vec<CustomModelOverlay>,
    keyless_providers: OrderedProviderSet,
}

struct RegistryState {
    collapse: Arc<Mutex<CollapseRuntime>>,
    built_in_by_provider: HashMap<WireString, Vec<HostModelRef>>,
    interned: HashMap<WireString, HostModelRef>,
    provider_lookups: HashMap<WireString, Vec<HostModelRef>>,
    full_snapshot: Option<Vec<HostModelRef>>,
    unprojected_snapshot: Option<Vec<HostModelRef>>,
    loaded_standard_models: Option<Vec<HostModelRef>>,
    loaded_standard_authority: Option<OrderedProviderSet>,
    discovered_models: Option<Vec<HostModelRef>>,
    discovered_authority: Option<OrderedProviderSet>,
    live_metrics: Option<CatalogMetricsIndex>,
    runtime_overlays: Option<Vec<CustomModelOverlay>>,
    runtime_overrides: Vec<(WireString, ProviderOverride)>,
    revision: u64,
}

/// Whole-catalog Host projection. Callbacks execute outside composition locks.
pub trait ModelCatalogProjection: Send + Sync {
    fn has_modifiers(&self) -> bool;
    fn project(&self, models: &[HostModelRef]) -> Vec<HostModelRef>;
}

/// One fixed configuration/snapshot generation. Create a new adapter when the
/// Host replaces its configuration; raw live-header records remain shared.
pub struct StaticModelRegistry {
    config: ParsedConfig,
    inputs: StaticRegistryInputs,
    metrics: CatalogMetricsIndex,
    state: Mutex<RegistryState>,
    projection: Mutex<Option<Arc<dyn ModelCatalogProjection>>>,
}

fn fixed_error(message: &str) -> CollapseError {
    CollapseError::new(message.into())
}

fn header_record(value: &Value) -> Result<HeaderConfigRecord, CollapseError> {
    let mut pairs = Vec::new();
    match value {
        Value::Object(entries) => {
            for (key, value) in ara_prompt::js::entries(entries) {
                pairs.push((
                    key.clone(),
                    value.as_str().ok_or_else(|| fixed_error("Header values must be strings"))?.into(),
                ));
            }
        }
        Value::Array(entries) => {
            for (index, value) in entries.iter().enumerate() {
                pairs.push((
                    index.to_string(),
                    value.as_str().ok_or_else(|| fixed_error("Header values must be strings"))?.into(),
                ));
            }
        }
        _ => return Err(fixed_error("Headers must be a record")),
    }
    Ok(HeaderConfigRecord::from_pairs(pairs))
}

fn json_headers(value: Option<&Value>) -> Result<HeaderSlot, CollapseError> {
    match value {
        None => Ok(HeaderSlot::Absent),
        Some(Value::Null) => Ok(HeaderSlot::Null),
        Some(value) => Ok(HeaderSlot::Source(HeaderSource::Config(header_record(value)?))),
    }
}

fn safe_json_fields(value: &Value, excluded: &[&str]) -> Result<VariantSpec, CollapseError> {
    let object = value.as_object().ok_or_else(|| fixed_error("Configuration entry must be a record"))?;
    let mut fields = empty();
    for (key, value) in ara_prompt::js::entries(object) {
        if !excluded.contains(&key.as_str()) {
            fields.set_record(key, &VariantSpec::from_json(value));
        }
    }
    Ok(fields)
}

fn json_patch(value: &Value) -> Result<ModelPatch, CollapseError> {
    ModelPatch::new(safe_json_fields(value, &["headers", "apiKey"])?, json_headers(value.get("headers"))?)
}

fn assign(target: &mut VariantSpec, key: &str, value: Option<&VariantSpec>) {
    if let Some(value) = value {
        target.set_record(key, value);
    } else {
        target.set_undefined(key);
    }
}

fn selected_field(primary: &VariantSpec, fallback: &VariantSpec, key: &str) -> Option<VariantSpec> {
    if nullish(primary.get(key)) { fallback.record(key) } else { primary.record(key) }
}

fn normalize_discovery_override_base_url(fields: &VariantSpec) -> Option<VariantSpec> {
    let discovery = fields.record("discovery");
    let kind = discovery.as_ref().and_then(|value| text(value, "type"));
    let bare = kind.as_ref().is_some_and(|kind| kind.equals_ascii("openai-models-list"))
        && discovery.as_ref().and_then(|value| boolean(value, "injectV1")) == Some(false);
    let litellm = kind.as_ref().is_some_and(|kind| kind.equals_ascii("litellm"));
    if !bare && !litellm {
        return fields.record("baseUrl");
    }
    let base = text(fields, "baseUrl").and_then(|value| value.to_utf8().ok());
    let raw = if bare {
        base.filter(|value| !value.is_empty()).unwrap_or_else(|| "http://127.0.0.1:1234".into())
    } else {
        // LiteLLM's outer ?? default precedes the normalizer's || default.
        let value = base.unwrap_or_else(|| "http://localhost:4000/v1".into());
        if value.is_empty() { "http://127.0.0.1:1234/v1".into() } else { value }
    };
    let normalized = if let Ok(mut url) = reqwest::Url::parse(&raw) {
        let mut path = url.path().trim_end_matches('/').to_owned();
        if !bare && !path.ends_with("/v1") {
            path.push_str("/v1");
        }
        // Setting the pathname mirrors WHATWG encoding for the injected path.
        if !bare {
            url.set_path(&path);
            path = url.path().to_owned();
        }
        let host = url.host_str().unwrap_or("");
        let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.into() };
        let port = url.port().map(|value| format!(":{value}")).unwrap_or_default();
        format!("{}://{host}{port}{path}", url.scheme())
    } else if bare {
        raw.trim_end_matches('/').into()
    } else {
        raw
    };
    Some(VariantSpec::from_wire(WireValue::String(normalized.into())))
}

fn parse_config(
    config: Option<&ModelsConfig>,
    resolved_header_presence: Option<&HashMap<WireString, bool>>,
) -> Result<ParsedConfig, CollapseError> {
    let mut parsed = ParsedConfig::default();
    let Some(providers) = config.and_then(|config| config.value().get("providers")).and_then(Value::as_object) else {
        return Ok(parsed);
    };
    for (provider, value) in ara_prompt::js::entries(providers) {
        let provider = WireString::from(provider.as_str());
        // Never project the whole provider tree, which includes nested secrets.
        let fields = safe_json_fields(value, &["headers", "apiKey", "models", "modelOverrides"])?;
        let headers = json_headers(value.get("headers"))?;
        let api_key = value.get("apiKey").and_then(Value::as_str).map(str::to_owned);
        let compat = fields.record("compat");
        let disable_strict = fields
            .get("disableStrictTools")
            .is_some_and(truthy)
            .then(|| VariantSpec::from_json(&serde_json::json!({"disableStrictTools":true})));
        let compat = merge_compat(compat.as_ref(), disable_strict.as_ref());
        let override_present = resolved_header_presence
            .and_then(|presence| presence.get(&provider).copied())
            .unwrap_or_else(|| headers.as_source().is_some())
            || api_key.as_ref().is_some_and(|value| !value.is_empty())
            || fields.get("authHeader").is_some()
            || [
                "baseUrl",
                "compat",
                "disableStrictTools",
                "guardrailIdentifier",
                "requestMetadata",
                "remoteCompaction",
                "transport",
            ]
            .iter()
            .any(|key| fields.get(key).is_some_and(truthy));
        if override_present {
            let mut override_fields = empty();
            let base_url = normalize_discovery_override_base_url(&fields);
            assign(&mut override_fields, "baseUrl", base_url.as_ref());
            copy_field(&mut override_fields, "authHeader", &fields, "authHeader");
            assign(&mut override_fields, "compat", compat.as_ref());
            for key in [
                "remoteCompaction",
                "transport",
                "guardrailIdentifier",
                "guardrailVersion",
                "guardrailTrace",
                "requestMetadata",
            ] {
                copy_field(&mut override_fields, key, &fields, key);
            }
            parsed
                .provider_overrides
                .insert(provider.clone(), ProviderOverride::new(override_fields, headers.clone(), api_key.clone())?);
        }
        if text(&fields, "auth").is_some_and(|value| value.equals_ascii("none")) {
            parsed.keyless_providers.insert(provider.clone());
        }
        if let Some(overrides) = value.get("modelOverrides").and_then(Value::as_object) {
            let mut models = HashMap::new();
            for (id, value) in ara_prompt::js::entries(overrides) {
                models.insert(id.as_str().into(), json_patch(value)?);
            }
            parsed.model_overrides.insert(provider.clone(), models);
        }
        let definitions = value.get("models").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        if !definitions.is_empty() {
            let base_url =
                text(&fields, "baseUrl").ok_or_else(|| fixed_error("Custom provider baseUrl must be a string"))?;
            let api = text(&fields, "api");
            let auth = text(&fields, "auth");
            let remote = fields.record("remoteCompaction");
            for definition in definitions {
                if let Some(overlay) = build_custom_model_overlay(
                    &provider,
                    &base_url,
                    api.as_ref(),
                    &headers,
                    api_key.as_deref(),
                    boolean(&fields, "authHeader"),
                    compat.as_ref(),
                    auth.as_ref(),
                    remote.as_ref(),
                    &json_patch(definition)?,
                )? {
                    parsed.custom_models.push(overlay);
                }
            }
        }
        parsed.providers.insert(provider, ConfiguredProvider { headers, api_key_config: api_key });
    }
    Ok(parsed)
}

fn wrap_bundled_model(spec: &SpecRef) -> Result<HostModelRef, CollapseError> {
    if spec.own_keys().iter().any(|key| key.equals_ascii("apiKey")) {
        return Err(fixed_error("Bundled model contains an unsupported credential field"));
    }
    let has_headers = spec.own_keys().iter().any(|key| key.equals_ascii("headers"));
    if !has_headers {
        return HostModel::new(spec.clone(), HeaderSlot::Absent);
    }
    let headers = match spec.record("headers") {
        None => HeaderSlot::Undefined,
        Some(record) if matches!(record.value, WireValue::Null) => HeaderSlot::Null,
        Some(record) => {
            let mut pairs = Vec::new();
            let projected = spread(&[&record]);
            for key in projected.own_keys() {
                let name = key
                    .to_utf8()
                    .map_err(|_| fixed_error("Header name cannot be represented by the Host header primitive"))?;
                let value = projected
                    .get_path(std::slice::from_ref(&key))
                    .and_then(WireValue::as_string)
                    .ok_or_else(|| fixed_error("Header values must be strings"))?
                    .to_utf8()
                    .map_err(|_| fixed_error("Header value cannot be represented by the Host header primitive"))?;
                pairs.push((name, value));
            }
            HeaderSlot::Source(HeaderSource::Config(HeaderConfigRecord::from_pairs(pairs)))
        }
    };
    let mut fields = spec.as_ref().clone();
    fields.remove("headers");
    HostModel::new(Arc::new(fields), headers)
}

/// Restore a sparse cache/discovery ModelSpec without putting headers into
/// its safe metadata. A bundled Model is already materialized and uses the
/// separate wrapper above.
pub(crate) fn host_model_from_spec(spec: &SpecRef) -> Result<HostModelRef, CollapseError> {
    let model = wrap_bundled_model(spec)?;
    build_host_model(model.spec(), model.headers().clone())
}

pub(crate) fn bundled_host_models() -> Result<&'static [HostModelRef], CollapseError> {
    static MODELS: OnceLock<Result<Vec<HostModelRef>, CollapseError>> = OnceLock::new();
    MODELS
        .get_or_init(|| bundled_models().iter().map(wrap_bundled_model).collect())
        .as_ref()
        .map(Vec::as_slice)
        .map_err(Clone::clone)
}

fn identity(model: &HostModelRef) -> Result<(WireString, WireString), CollapseError> {
    Ok((
        text(model.spec(), "provider").ok_or_else(|| fixed_error("Model provider must be a string"))?,
        text(model.spec(), "id").ok_or_else(|| fixed_error("Model ID must be a string"))?,
    ))
}

fn key(provider: &WireString, id: &WireString) -> WireString {
    let mut units = provider.units().to_vec();
    units.push(0);
    units.extend_from_slice(id.units());
    WireString::from_units(units)
}

fn select_models(models: &[HostModelRef], providers: Option<&OrderedProviderSet>) -> Vec<HostModelRef> {
    models
        .iter()
        .filter(|model| {
            providers.is_none_or(|providers| {
                text(model.spec(), "provider").is_some_and(|provider| providers.contains(&provider))
            })
        })
        .cloned()
        .collect()
}

fn apply_provider_transport(
    model: &HostModelRef,
    override_: &ProviderOverride,
    include_compat: bool,
) -> Result<HostModelRef, CollapseError> {
    let mut spec = to_host_model_spec(model);
    let fields = override_.fields();
    let base_url = selected_field(fields, &spec, "baseUrl");
    assign(&mut spec, "baseUrl", base_url.as_ref());
    let headers = merge_header_sources(
        &[model.headers().clone(), override_.headers().clone()],
        HeaderResolutionOptions {
            auth_header: fields.get("authHeader").is_some_and(truthy),
            api_key_config: override_.api_key_config().map(str::to_owned),
        },
    );
    if fields.get("transport").is_some() {
        copy_field(&mut spec, "transport", fields, "transport");
    }
    let model_remote = spec.record("remoteCompaction");
    let provider_remote = fields.record("remoteCompaction");
    let remote = merge_provider_remote_compaction_config(model_remote.as_ref(), provider_remote.as_ref());
    assign(&mut spec, "remoteCompaction", remote.as_ref());
    if include_compat {
        let model_compat = model.spec().record("compatConfig");
        let provider_compat = fields.record("compat");
        let compat = merge_compat(model_compat.as_ref(), provider_compat.as_ref());
        assign(&mut spec, "compat", compat.as_ref());
    }
    build_host_model(&spec, headers)
}

impl StaticModelRegistry {
    pub fn from_config(config: Option<&ModelsConfig>) -> Result<Self, CollapseError> {
        Self::from_inputs(
            config,
            StaticRegistryInputs { bundled_models: bundled_host_models()?.to_vec(), ..Default::default() },
        )
    }

    pub fn from_inputs(config: Option<&ModelsConfig>, inputs: StaticRegistryInputs) -> Result<Self, CollapseError> {
        Self::from_loader_inputs(config, inputs, None)
    }

    /// A production loader observes eagerly resolved header presence while
    /// retaining the raw live source for subsequent requests.
    pub fn from_loader_inputs(
        config: Option<&ModelsConfig>,
        inputs: StaticRegistryInputs,
        resolved_header_presence: Option<&HashMap<WireString, bool>>,
    ) -> Result<Self, CollapseError> {
        for model in inputs
            .bundled_models
            .iter()
            .chain(&inputs.cached_standard_models)
            .chain(&inputs.cached_discoverable_models)
            .chain(&inputs.runtime_discovered_models)
        {
            identity(model)?;
        }
        for model in &inputs.metrics_models {
            if model.own_keys().iter().any(|key| key.equals_ascii("headers") || key.equals_ascii("apiKey")) {
                return Err(fixed_error("Catalog metrics input must contain safe metadata only"));
            }
        }
        let metrics = CatalogMetricsIndex::new(&inputs.metrics_models);
        Ok(Self {
            config: parse_config(config, resolved_header_presence)?,
            inputs,
            metrics,
            state: Mutex::new(RegistryState {
                collapse: Arc::new(Mutex::new(CollapseRuntime::new()?)),
                built_in_by_provider: HashMap::new(),
                interned: HashMap::new(),
                provider_lookups: HashMap::new(),
                full_snapshot: None,
                unprojected_snapshot: None,
                loaded_standard_models: None,
                loaded_standard_authority: None,
                discovered_models: None,
                discovered_authority: None,
                live_metrics: None,
                runtime_overlays: None,
                runtime_overrides: Vec::new(),
                revision: 0,
            }),
            projection: Mutex::new(None),
        })
    }

    pub fn is_keyless_provider(&self, provider: &WireString) -> bool {
        self.config.keyless_providers.contains(provider)
    }
    pub fn configured_api_key(&self, provider: &WireString) -> Option<&str> {
        self.config.providers.get(provider)?.api_key_config.as_deref().filter(|value| !value.is_empty())
    }
    pub fn configured_provider_headers(&self, provider: &WireString) -> Option<&HeaderSlot> {
        self.config.providers.get(provider).map(|value| &value.headers)
    }
    pub fn provider_override(&self, provider: &WireString) -> Option<&ProviderOverride> {
        self.config.provider_overrides.get(provider)
    }

    pub fn install_projection(&self, projection: Arc<dyn ModelCatalogProjection>) {
        *self.projection.lock().expect("registry projection poisoned") = Some(projection);
    }

    pub fn install_runtime_layers(
        &self,
        overlays: Vec<CustomModelOverlay>,
        overrides: Vec<(WireString, ProviderOverride)>,
    ) {
        let mut state = self.state.lock().expect("static registry poisoned");
        state.runtime_overlays = Some(overlays);
        state.runtime_overrides = overrides;
        state.revision = state.revision.wrapping_add(1);
    }

    pub fn has_full_snapshot(&self) -> bool {
        self.state.lock().expect("static registry poisoned").full_snapshot.is_some()
    }

    pub fn invalidate_provider(&self, provider: &WireString) {
        let mut touched = OrderedProviderSet::default();
        touched.insert(provider.clone());
        Self::invalidate_touched(&mut self.state.lock().expect("static registry poisoned"), &touched);
    }

    pub fn invalidate_all_provider_lookups(&self) {
        let mut state = self.state.lock().expect("static registry poisoned");
        state.provider_lookups.clear();
        state.interned.clear();
        state.revision = state.revision.wrapping_add(1);
    }

    pub fn collapse_runtime(&self) -> Arc<Mutex<CollapseRuntime>> {
        self.state.lock().expect("static registry poisoned").collapse.clone()
    }

    fn apply_llama_cpp_fixups(&self, models: Vec<HostModelRef>) -> Result<Vec<HostModelRef>, CollapseError> {
        use crate::model_registry_discovery::{
            apply_llama_cpp_qwen_thinking, ensure_llama_cpp_v1_base_url, normalize_llama_cpp_base_url,
        };
        models
            .into_iter()
            .map(|model| {
                if !text(model.spec(), "provider")
                    .is_some_and(|provider| self.inputs.llama_cpp_providers.contains(&provider))
                {
                    return Ok(model);
                }
                let model =
                    apply_llama_cpp_qwen_thinking(&model).map_err(|_| fixed_error("llama.cpp model fixups failed"))?;
                if !model.spec().get("transport").is_some_and(truthy)
                    && !text(model.spec(), "baseUrl")
                        .is_some_and(|base| base.units().ends_with(&"/v1".encode_utf16().collect::<Vec<_>>()))
                {
                    let mut fields = to_host_model_spec(&model);
                    let base = normalize_llama_cpp_base_url(text(model.spec(), "baseUrl").as_ref());
                    fields.set("baseUrl", WireValue::String(ensure_llama_cpp_v1_base_url(&base)));
                    build_host_model(&fields, model.headers().clone())
                } else {
                    Ok(model)
                }
            })
            .collect()
    }

    pub fn register_runtime_models(
        &self,
        provider: &WireString,
        overlays: &[CustomModelOverlay],
        retained: Option<&ProviderOverride>,
    ) -> Result<(), CollapseError> {
        let mut state = self.state.lock().expect("static registry poisoned");
        let mut models = state.unprojected_snapshot.clone().unwrap_or_default();
        models.retain(|model| text(model.spec(), "provider").as_ref() != Some(provider));
        for overlay in overlays {
            let model = finalize_custom_model(overlay, CustomModelBuildOptions { use_defaults: true })?;
            models.push(match retained {
                Some(override_) => apply_provider_transport(&model, override_, false)?,
                None => model,
            });
        }
        state.unprojected_snapshot = Some(self.apply_provider_bedrock_overrides(models)?);
        state.revision = state.revision.wrapping_add(1);
        let full = state.full_snapshot.is_some();
        drop(state);
        if full {
            self.reproject_full_snapshot(false)?;
        }
        Ok(())
    }

    pub fn apply_runtime_transport(
        &self,
        provider: &WireString,
        incoming: &ProviderOverride,
    ) -> Result<(), CollapseError> {
        let mut state = self.state.lock().expect("static registry poisoned");
        if state.full_snapshot.is_none() {
            return Ok(());
        }
        let models = state
            .unprojected_snapshot
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|model| {
                if text(model.spec(), "provider").as_ref() == Some(provider) {
                    apply_provider_transport(&model, incoming, false)
                } else {
                    Ok(model)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        state.unprojected_snapshot = Some(self.apply_llama_cpp_fixups(models)?);
        state.revision = state.revision.wrapping_add(1);
        drop(state);
        self.reproject_full_snapshot(false)
    }

    fn reproject_full_snapshot(&self, intern: bool) -> Result<(), CollapseError> {
        loop {
            let projection = self.projection.lock().expect("registry projection poisoned").clone();
            let (models, revision) = {
                let state = self.state.lock().expect("static registry poisoned");
                (state.unprojected_snapshot.clone().unwrap_or_default(), state.revision)
            };
            let projected = projection.map_or_else(|| models.clone(), |projection| projection.project(&models));
            let mut state = self.state.lock().expect("static registry poisoned");
            if state.revision != revision {
                continue;
            }
            state.full_snapshot = Some(self.project_metrics(&mut state, projected, intern)?);
            return Ok(());
        }
    }

    pub fn prepare_cached_provider_models(
        &self,
        provider: &WireString,
        models: Vec<HostModelRef>,
    ) -> Result<Vec<HostModelRef>, CollapseError> {
        let models = models
            .into_iter()
            .map(|model| match self.config.provider_overrides.get(provider) {
                Some(override_) => apply_provider_transport(&model, override_, true),
                None => Ok(model),
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.prepare_cached_discovery_models(provider, models)
    }

    pub fn prepare_cached_discovery_models(
        &self,
        _provider: &WireString,
        models: Vec<HostModelRef>,
    ) -> Result<Vec<HostModelRef>, CollapseError> {
        let mut state = self.state.lock().map_err(|_| fixed_error("Static registry state is unavailable"))?;
        let models = self.apply_model_overrides(&mut state, models)?;
        self.apply_hardcoded_model_policies(models)
    }

    fn invalidate_touched(state: &mut RegistryState, touched: &OrderedProviderSet) {
        state.revision = state.revision.wrapping_add(1);
        for provider in touched.iter() {
            let prefix = key(provider, &WireString::from(""));
            state.interned.retain(|key, _| !key.units().starts_with(prefix.units()));
            state.provider_lookups.remove(&lower(&trim(provider)));
        }
    }

    /// A cache slice arrives after its Host I/O has settled. Other model
    /// references and the lazy/full distinction survive the injection.
    pub fn inject_standard_cache_snapshot(
        &self,
        models: Vec<HostModelRef>,
        authoritative: OrderedProviderSet,
        touched: &OrderedProviderSet,
    ) -> Result<(), CollapseError> {
        for model in &models {
            identity(model)?;
        }
        let mut state = self.state.lock().map_err(|_| fixed_error("Static registry state is unavailable"))?;
        state.loaded_standard_models = Some(models);
        state.loaded_standard_authority = Some(authoritative);
        Self::invalidate_touched(&mut state, touched);
        if state.full_snapshot.is_some() {
            let unprojected = self.compose_unprojected_models(&mut state, None)?;
            state.unprojected_snapshot = Some(unprojected);
        }
        let full = state.full_snapshot.is_some();
        drop(state);
        if full {
            self.reproject_full_snapshot(true)?;
        }
        Ok(())
    }

    /// Match the native runtime merge against the old unprojected snapshot.
    /// Empty authoritative discoveries still touch and replace their provider.
    pub fn publish_discovery(
        &self,
        discovered: Vec<HostModelRef>,
        authoritative: OrderedProviderSet,
        touched: &OrderedProviderSet,
        metric_models: &[SpecRef],
        replace_metrics: bool,
    ) -> Result<(), CollapseError> {
        for model in &discovered {
            identity(model)?;
        }
        if metric_models
            .iter()
            .any(|model| model.own_keys().iter().any(|key| key.equals_ascii("headers") || key.equals_ascii("apiKey")))
        {
            return Err(fixed_error("Catalog metrics input must contain safe metadata only"));
        }
        let mut state = self.state.lock().map_err(|_| fixed_error("Static registry state is unavailable"))?;
        if replace_metrics {
            let incoming = CatalogMetricsIndex::new(metric_models);
            if !incoming.is_empty() {
                state.live_metrics = Some(incoming);
            }
        } else {
            state
                .live_metrics
                .get_or_insert_with(|| CatalogMetricsIndex::new(&self.inputs.metrics_models))
                .add(metric_models);
        }
        if discovered.is_empty() && authoritative.is_empty() {
            return Ok(());
        }
        let existing = match &state.unprojected_snapshot {
            Some(models) => models.clone(),
            None => self.compose_unprojected_models(&mut state, Some(touched))?,
        };
        let references = crate::provider_model_reference::ProviderModelReferenceIndex::with_collapse_runtime(
            &existing,
            state.collapse.clone(),
        )?;
        let merged = discovered
            .iter()
            .map(|model| {
                let (provider, id) = identity(model)?;
                let donor = references.resolve(&provider, &id)?;
                merge_discovered_model(model, donor.as_ref(), self.config.provider_overrides.get(&provider))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let merged = self.apply_hardcoded_model_policies(merged)?;
        let mut runtime = state
            .discovered_models
            .as_ref()
            .unwrap_or(&self.inputs.runtime_discovered_models)
            .iter()
            .filter(|model| text(model.spec(), "provider").is_none_or(|provider| !touched.contains(&provider)))
            .cloned()
            .collect::<Vec<_>>();
        runtime.extend(merged.iter().cloned());
        let mut all_authority = OrderedProviderSet::default();
        for provider in
            state.discovered_authority.as_ref().unwrap_or(&self.inputs.runtime_authoritative_providers).iter()
        {
            if !touched.contains(provider) {
                all_authority.insert(provider.clone());
            }
        }
        for provider in authoritative.iter() {
            all_authority.insert(provider.clone());
        }
        state.discovered_models = Some(runtime);
        state.discovered_authority = Some(all_authority);
        Self::invalidate_touched(&mut state, touched);
        if state.full_snapshot.is_some() {
            let base = drop_provider_models(&existing, &authoritative);
            let resolved = self.merge_resolved_models(&base, &merged)?;
            let unprojected = self.apply_overlay_layers(&mut state, &resolved, None)?;
            state.unprojected_snapshot = Some(unprojected);
        }
        let full = state.full_snapshot.is_some();
        drop(state);
        if full {
            self.reproject_full_snapshot(false)?;
        }
        Ok(())
    }

    pub fn known_provider_ids(&self) -> Result<OrderedProviderSet, CollapseError> {
        let state = self.state.lock().map_err(|_| fixed_error("Static registry state is unavailable"))?;
        Ok(self.known_static_providers(&state))
    }

    fn known_static_providers(&self, state: &RegistryState) -> OrderedProviderSet {
        let mut providers = OrderedProviderSet::default();
        for model in &self.inputs.bundled_models {
            if let Some(provider) = text(model.spec(), "provider") {
                providers.insert(provider);
            }
        }
        for provider in self.inputs.pending_standard_providers.iter() {
            providers.insert(provider.clone());
        }
        for models in [
            state.loaded_standard_models.as_ref().unwrap_or(&self.inputs.cached_standard_models),
            &self.inputs.cached_discoverable_models,
            state.discovered_models.as_ref().unwrap_or(&self.inputs.runtime_discovered_models),
        ] {
            for model in models {
                if let Some(provider) = text(model.spec(), "provider") {
                    providers.insert(provider);
                }
            }
        }
        for model in self
            .config
            .custom_models
            .iter()
            .chain(state.runtime_overlays.as_ref().unwrap_or(&self.inputs.runtime_model_overlays))
        {
            if let Some(provider) = text(model.fields(), "provider") {
                providers.insert(provider);
            }
        }
        providers
    }

    fn load_built_in_models(
        &self,
        state: &mut RegistryState,
        filter: Option<&OrderedProviderSet>,
    ) -> Result<Vec<HostModelRef>, CollapseError> {
        let mut providers = OrderedProviderSet::default();
        for model in &self.inputs.bundled_models {
            providers.insert(identity(model)?.0);
        }
        let mut out = Vec::new();
        for provider in providers.iter() {
            if filter.is_some_and(|filter| !filter.contains(provider)) {
                continue;
            }
            if !state.built_in_by_provider.contains_key(provider) {
                let models = self
                    .inputs
                    .bundled_models
                    .iter()
                    .filter(|model| text(model.spec(), "provider").as_ref() == Some(provider))
                    .map(|model| match self.config.provider_overrides.get(provider) {
                        Some(override_) => apply_provider_transport(model, override_, true),
                        None => Ok(model.clone()),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                state.built_in_by_provider.insert(provider.clone(), models);
            }
            out.extend(state.built_in_by_provider.get(provider).unwrap().iter().cloned());
        }
        Ok(out)
    }

    fn apply_hardcoded_model_policies(&self, models: Vec<HostModelRef>) -> Result<Vec<HostModelRef>, CollapseError> {
        models
            .into_iter()
            .map(|mut model| {
                let (provider, id) = identity(&model)?;
                if !self.inputs.extended_context && !provider.equals_ascii("xai-oauth") {
                    let threshold = model
                        .spec()
                        .record("cost")
                        .and_then(|cost| cost.record("longContext"))
                        .and_then(|tier| number(&tier, "inputThreshold"));
                    if let Some(threshold) = threshold
                        && number(model.spec(), "contextWindow").is_some_and(|window| window > threshold)
                    {
                        let patch = ModelPatch::new(
                            VariantSpec::from_json(&serde_json::json!({"contextWindow":threshold})),
                            HeaderSlot::Absent,
                        )?;
                        model = apply_model_override(&model, &patch)?;
                    }
                }
                if provider.equals_ascii("ollama-cloud")
                    && model.spec().get("omitMaxOutputTokens") != Some(&WireValue::Bool(true))
                {
                    model = apply_model_override(
                        &model,
                        &ModelPatch::new(
                            VariantSpec::from_json(&serde_json::json!({"omitMaxOutputTokens":true})),
                            HeaderSlot::Absent,
                        )?,
                    )?;
                }
                if id.equals_ascii("gpt-5.4") && !provider.equals_ascii("github-copilot") {
                    let authored = self.config.model_overrides.get(&provider).and_then(|overrides| overrides.get(&id));
                    let mut fields = empty();
                    fields.set(
                        "contextWindow",
                        authored
                            .and_then(|patch| patch.fields().get("contextWindow"))
                            .filter(|value| !matches!(value, WireValue::Null))
                            .cloned()
                            .unwrap_or(WireValue::Number(1_000_000.0)),
                    );
                    if let Some(authored) = authored {
                        fields = spread(&[&fields, authored.fields()]);
                    }
                    model = apply_model_override(
                        &model,
                        &ModelPatch::new(fields, authored.map(|patch| patch.headers().clone()).unwrap_or_default())?,
                    )?;
                }
                Ok(model)
            })
            .collect()
    }

    fn merge_resolved_models(
        &self,
        base: &[HostModelRef],
        replacements: &[HostModelRef],
    ) -> Result<Vec<HostModelRef>, CollapseError> {
        merge_by_model_key(
            base,
            replacements,
            |model| identity(model).expect("snapshot identities were validated"),
            |existing, replacement| {
                let Some(existing) = existing else { return Ok(replacement.clone()) };
                let mut fields = replacement.spec().as_ref().clone();
                for key in ["contextWindow", "maxTokens", "omitMaxOutputTokens"] {
                    let value = selected_field(replacement.spec(), existing.spec(), key);
                    assign(&mut fields, key, value.as_ref());
                }
                let supports = selected_field(replacement.spec(), existing.spec(), "supportsTools");
                if let Some(supports) = supports {
                    fields.set_record("supportsTools", &supports);
                }
                replacement.with_spec(Arc::new(fields))
            },
        )
    }

    fn merge_custom_models(
        &self,
        base: &[HostModelRef],
        overlays: &[CustomModelOverlay],
    ) -> Result<Vec<HostModelRef>, CollapseError> {
        for overlay in overlays {
            if text(overlay.fields(), "provider").is_none() || text(overlay.fields(), "id").is_none() {
                return Err(fixed_error("Custom overlay provider and ID must be strings"));
            }
        }
        merge_by_model_key(
            base,
            overlays,
            |overlay| (text(overlay.fields(), "provider").unwrap(), text(overlay.fields(), "id").unwrap()),
            |existing, overlay| {
                let Some(existing) = existing else {
                    return finalize_custom_model(overlay, CustomModelBuildOptions { use_defaults: true });
                };
                let mut fields = existing.spec().as_ref().clone();
                for key in ["id", "provider", "api", "baseUrl"] {
                    copy_field(&mut fields, key, overlay.fields(), key);
                }
                let base = existing.with_spec(Arc::new(fields))?;
                apply_model_patch(&base, overlay, ModelTransportPolicy::Replace)
            },
        )
    }

    fn collapse_models(
        &self,
        state: &mut RegistryState,
        models: &[HostModelRef],
    ) -> Result<Vec<HostModelRef>, CollapseError> {
        let mut owners: HashMap<usize, HostModelRef> = HashMap::new();
        let mut specs = Vec::with_capacity(models.len());
        for model in models {
            let original = Arc::as_ptr(model.spec()) as usize;
            let spec = if owners.get(&original).is_some_and(|owner| !Arc::ptr_eq(owner, model)) {
                Arc::new(model.spec().as_ref().clone())
            } else {
                model.spec().clone()
            };
            owners.insert(Arc::as_ptr(&spec) as usize, model.clone());
            specs.push(spec);
        }
        state
            .collapse
            .lock()
            .map_err(|_| fixed_error("Model alias state is unavailable"))?
            .collapse_built_variants_with_donors(&specs)?
            .into_iter()
            .map(|(output, donor)| {
                if let Some(model) = owners.get(&(Arc::as_ptr(&output) as usize)) {
                    return Ok(model.clone());
                }
                owners
                    .get(&(Arc::as_ptr(&donor) as usize))
                    .ok_or_else(|| fixed_error("Collapsed model has no actual input donor"))?
                    .with_spec(output)
            })
            .collect()
    }

    fn apply_model_overrides(
        &self,
        state: &mut RegistryState,
        models: Vec<HostModelRef>,
    ) -> Result<Vec<HostModelRef>, CollapseError> {
        if self.config.model_overrides.is_empty() {
            return Ok(models);
        }
        let live_keys: HashSet<_> = models
            .iter()
            .map(|model| identity(model).map(|(provider, id)| key(&provider, &id)))
            .collect::<Result<_, _>>()?;
        let has_live = |provider: &WireString, id: &WireString| live_keys.contains(&key(provider, id));
        models
            .into_iter()
            .map(|model| {
                let (provider, _) = identity(&model)?;
                let Some(overrides) = self.config.model_overrides.get(&provider) else { return Ok(model) };
                let Some(override_) = resolve_model_override_with_aliases(
                    overrides,
                    &model,
                    &mut *state.collapse.lock().map_err(|_| fixed_error("Model alias state is unavailable"))?,
                    &has_live,
                )?
                else {
                    return Ok(model);
                };
                apply_model_override(&model, override_)
            })
            .collect()
    }

    fn apply_provider_bedrock_overrides(&self, models: Vec<HostModelRef>) -> Result<Vec<HostModelRef>, CollapseError> {
        models
            .into_iter()
            .map(|model| {
                let (provider, _) = identity(&model)?;
                let Some(override_) = self.config.provider_overrides.get(&provider) else { return Ok(model) };
                let keys = ["guardrailIdentifier", "guardrailVersion", "guardrailTrace", "requestMetadata"];
                if keys.iter().all(|key| override_.fields().get(key).is_none()) {
                    return Ok(model);
                }
                let mut fields = to_host_model_spec(&model);
                for key in keys {
                    if override_.fields().get(key).is_some() {
                        copy_field(&mut fields, key, override_.fields(), key);
                    }
                }
                build_host_model(&fields, model.headers().clone())
            })
            .collect()
    }

    fn compose_unprojected_models(
        &self,
        state: &mut RegistryState,
        providers: Option<&OrderedProviderSet>,
    ) -> Result<Vec<HostModelRef>, CollapseError> {
        let built = self.load_built_in_models(state, providers)?;
        let built = self.apply_hardcoded_model_policies(built)?;
        let built = drop_provider_models(
            &built,
            state.loaded_standard_authority.as_ref().unwrap_or(&self.inputs.cached_authoritative_providers),
        );
        let standard = select_models(
            state.loaded_standard_models.as_ref().unwrap_or(&self.inputs.cached_standard_models),
            providers,
        );
        let defaults = self.merge_resolved_models(&built, &standard)?;
        let discoverable = select_models(&self.inputs.cached_discoverable_models, providers);
        let defaults = self.merge_resolved_models(&defaults, &discoverable)?;
        let defaults = drop_provider_models(
            &defaults,
            state.discovered_authority.as_ref().unwrap_or(&self.inputs.runtime_authoritative_providers),
        );
        let runtime = select_models(
            state.discovered_models.as_ref().unwrap_or(&self.inputs.runtime_discovered_models),
            providers,
        );
        let defaults = self.merge_resolved_models(&defaults, &runtime)?;
        self.apply_overlay_layers(state, &defaults, providers)
    }

    fn apply_overlay_layers(
        &self,
        state: &mut RegistryState,
        defaults: &[HostModelRef],
        providers: Option<&OrderedProviderSet>,
    ) -> Result<Vec<HostModelRef>, CollapseError> {
        let select_overlays = |overlays: &[CustomModelOverlay]| {
            overlays
                .iter()
                .filter(|overlay| {
                    providers.is_none_or(|providers| {
                        text(overlay.fields(), "provider").is_some_and(|provider| providers.contains(&provider))
                    })
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        let custom = self.merge_custom_models(defaults, &select_overlays(&self.config.custom_models))?;
        let runtime = self.merge_custom_models(
            &custom,
            &select_overlays(state.runtime_overlays.as_ref().unwrap_or(&self.inputs.runtime_model_overlays)),
        )?;
        let collapsed = self.collapse_models(state, &runtime)?;
        let overridden = self.apply_model_overrides(state, collapsed)?;
        let bedrock = self.apply_provider_bedrock_overrides(overridden)?;
        let runtime = bedrock
            .into_iter()
            .map(|model| {
                let (provider, _) = identity(&model)?;
                match state.runtime_overrides.iter().find(|(name, _)| name == &provider) {
                    Some((_, override_)) => apply_provider_transport(&model, override_, false),
                    None => Ok(model),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.apply_llama_cpp_fixups(runtime)
    }

    fn project_metrics(
        &self,
        state: &mut RegistryState,
        models: Vec<HostModelRef>,
        intern: bool,
    ) -> Result<Vec<HostModelRef>, CollapseError> {
        let specs: Vec<_> = models.iter().map(|model| model.spec().clone()).collect();
        let metrics = apply_catalog_metrics(&specs, state.live_metrics.as_ref().unwrap_or(&self.metrics));
        models
            .into_iter()
            .zip(metrics)
            .map(|(model, spec)| {
                let model = if Arc::ptr_eq(model.spec(), &spec) { model } else { model.with_spec(spec)? };
                let (provider, id) = identity(&model)?;
                if intern { Ok(state.interned.entry(key(&provider, &id)).or_insert(model).clone()) } else { Ok(model) }
            })
            .collect()
    }

    fn compose_projected(
        &self,
        providers: Option<&OrderedProviderSet>,
        lookup: Option<&WireString>,
        full: bool,
    ) -> Result<Vec<HostModelRef>, CollapseError> {
        loop {
            let projection = self.projection.lock().expect("registry projection poisoned").clone();
            let whole = projection.as_ref().is_some_and(|projection| projection.has_modifiers());
            let (unprojected, revision) = {
                let mut state = self.state.lock().expect("static registry poisoned");
                if let Some(models) = &state.full_snapshot {
                    return Ok(if lookup.is_some() { models.clone() } else { select_models(models, providers) });
                }
                if let Some(models) = lookup.and_then(|lookup| state.provider_lookups.get(lookup)) {
                    return Ok(models.clone());
                }
                let models = self.compose_unprojected_models(&mut state, if whole { None } else { providers })?;
                (models, state.revision)
            };
            let projected =
                projection.map_or_else(|| unprojected.clone(), |projection| projection.project(&unprojected));
            let mut state = self.state.lock().expect("static registry poisoned");
            if state.revision != revision {
                continue;
            }
            let projected = self.project_metrics(&mut state, projected, false)?;
            let selected = select_models(&projected, providers)
                .into_iter()
                .map(|model| {
                    let (provider, id) = identity(&model)?;
                    Ok(state.interned.entry(key(&provider, &id)).or_insert(model).clone())
                })
                .collect::<Result<Vec<_>, CollapseError>>()?;
            if full {
                state.unprojected_snapshot = Some(unprojected);
                state.full_snapshot = Some(selected.clone());
                state.provider_lookups.clear();
            } else if let Some(lookup) = lookup {
                state.provider_lookups.insert(lookup.clone(), selected.clone());
            }
            return Ok(selected);
        }
    }

    pub fn models_for_provider_lookup(&self, provider: &WireString) -> Result<Vec<HostModelRef>, CollapseError> {
        let normalized = lower(&trim(provider));
        if normalized.is_empty() {
            return Ok(Vec::new());
        }
        let mut matching = OrderedProviderSet::default();
        for candidate in self.known_provider_ids()?.iter() {
            if lower(candidate) == normalized {
                matching.insert(candidate.clone());
            }
        }
        self.compose_projected(Some(&matching), Some(&normalized), false)
    }

    /// Only exact provider/ID equality. General selector resolution is separate.
    pub fn find_exact(&self, provider: &WireString, id: &WireString) -> Result<Option<HostModelRef>, CollapseError> {
        Ok(self.models_for_provider_lookup(provider)?.into_iter().find(|model| {
            text(model.spec(), "provider").as_ref() == Some(provider) && text(model.spec(), "id").as_ref() == Some(id)
        }))
    }

    /// Exact lookup followed only by the native variant-alias table; this is not
    /// the full model-resolver/provider-reference selection surface.
    pub fn find_alias_exact(
        &self,
        provider: &WireString,
        id: &WireString,
    ) -> Result<Option<HostModelRef>, CollapseError> {
        let models = self.models_for_provider_lookup(provider)?;
        if let Some(model) = models.iter().find(|model| {
            text(model.spec(), "provider").as_ref() == Some(provider) && text(model.spec(), "id").as_ref() == Some(id)
        }) {
            return Ok(Some(model.clone()));
        }
        let Some(alias) = self
            .collapse_runtime()
            .lock()
            .map_err(|_| fixed_error("Model alias state is unavailable"))?
            .resolve_variant_selector(provider, id)?
        else {
            return Ok(None);
        };
        Ok(models.into_iter().find(|model| {
            text(model.spec(), "provider").as_ref() == Some(provider)
                && text(model.spec(), "id").as_ref() == Some(&alias)
        }))
    }

    pub fn find_reference(
        &self,
        provider: &WireString,
        id: &WireString,
    ) -> Result<Option<HostModelRef>, CollapseError> {
        let models = self.models_for_provider_lookup(provider)?;
        crate::provider_model_reference::ProviderModelReferenceIndex::with_collapse_runtime(
            &models,
            self.collapse_runtime(),
        )?
        .resolve(provider, id)
    }

    pub fn get_all(&self) -> Result<Vec<HostModelRef>, CollapseError> {
        self.compose_projected(None, None, true)
    }

    /// `has_auth` is a Host availability observation, memoized per provider for
    /// this call. It never grants a Gateway/provider execution permission.
    pub fn get_available_for_providers(
        &self,
        providers: &OrderedProviderSet,
        has_auth: &dyn Fn(&WireString) -> bool,
    ) -> Result<Vec<HostModelRef>, CollapseError> {
        let mut requested = OrderedProviderSet::default();
        for provider in providers.iter() {
            let provider = lower(&trim(provider));
            if !provider.is_empty() {
                requested.insert(provider);
            }
        }
        let mut availability = HashMap::new();
        let mut is_available = |provider: &WireString| {
            *availability.entry(provider.clone()).or_insert_with(|| {
                !self.inputs.disabled_providers.contains(provider)
                    && (self.is_keyless_provider(provider) || has_auth(provider))
            })
        };
        // Host authentication observations can re-enter catalog lookup. Keep
        // every such callback outside the registry's composition mutex.
        let full_snapshot =
            self.state.lock().map_err(|_| fixed_error("Static registry state is unavailable"))?.full_snapshot.clone();
        if let Some(models) = full_snapshot {
            return Ok(models
                .iter()
                .filter(|model| {
                    text(model.spec(), "provider")
                        .is_some_and(|provider| requested.contains(&lower(&provider)) && is_available(&provider))
                })
                .cloned()
                .collect());
        }
        let mut available = OrderedProviderSet::default();
        let known_providers = {
            let state = self.state.lock().map_err(|_| fixed_error("Static registry state is unavailable"))?;
            self.known_static_providers(&state)
        };
        for provider in known_providers.iter() {
            if requested.contains(&lower(provider)) && is_available(provider) {
                available.insert(provider.clone());
            }
        }
        self.compose_projected(Some(&available), None, false)
    }
}
