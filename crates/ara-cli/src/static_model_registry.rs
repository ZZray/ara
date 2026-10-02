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
        merge_compat, merge_header_sources, merge_provider_remote_compaction_config, to_host_model_spec,
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
    collapse: CollapseRuntime,
    built_in_by_provider: HashMap<WireString, Vec<HostModelRef>>,
    interned: HashMap<WireString, HostModelRef>,
    provider_lookups: HashMap<WireString, Vec<HostModelRef>>,
    full_snapshot: Option<Vec<HostModelRef>>,
}

/// One fixed configuration/snapshot generation. Create a new adapter when the
/// Host replaces its configuration; raw live-header records remain shared.
pub struct StaticModelRegistry {
    config: ParsedConfig,
    inputs: StaticRegistryInputs,
    metrics: CatalogMetricsIndex,
    state: Mutex<RegistryState>,
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

fn parse_config(config: Option<&ModelsConfig>) -> Result<ParsedConfig, CollapseError> {
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
        let override_present = headers.as_source().is_some()
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

fn bundled_host_models() -> Result<&'static [HostModelRef], CollapseError> {
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
            config: parse_config(config)?,
            inputs,
            metrics,
            state: Mutex::new(RegistryState {
                collapse: CollapseRuntime::new()?,
                built_in_by_provider: HashMap::new(),
                interned: HashMap::new(),
                provider_lookups: HashMap::new(),
                full_snapshot: None,
            }),
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

    fn known_static_providers(&self) -> OrderedProviderSet {
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
            &self.inputs.cached_standard_models,
            &self.inputs.cached_discoverable_models,
            &self.inputs.runtime_discovered_models,
        ] {
            for model in models {
                if let Some(provider) = text(model.spec(), "provider") {
                    providers.insert(provider);
                }
            }
        }
        for model in self.config.custom_models.iter().chain(&self.inputs.runtime_model_overlays) {
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
                let Some(override_) =
                    resolve_model_override_with_aliases(overrides, &model, &mut state.collapse, &has_live)?
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

    fn compose_static_models(
        &self,
        state: &mut RegistryState,
        providers: Option<&OrderedProviderSet>,
    ) -> Result<Vec<HostModelRef>, CollapseError> {
        let built = self.load_built_in_models(state, providers)?;
        let built = self.apply_hardcoded_model_policies(built)?;
        let built = drop_provider_models(&built, &self.inputs.cached_authoritative_providers);
        let standard = select_models(&self.inputs.cached_standard_models, providers);
        let defaults = self.merge_resolved_models(&built, &standard)?;
        let discoverable = select_models(&self.inputs.cached_discoverable_models, providers);
        let defaults = self.merge_resolved_models(&defaults, &discoverable)?;
        let defaults = drop_provider_models(&defaults, &self.inputs.runtime_authoritative_providers);
        let runtime = select_models(&self.inputs.runtime_discovered_models, providers);
        let defaults = self.merge_resolved_models(&defaults, &runtime)?;
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
        let custom = self.merge_custom_models(&defaults, &select_overlays(&self.config.custom_models))?;
        let runtime = self.merge_custom_models(&custom, &select_overlays(&self.inputs.runtime_model_overlays))?;
        let collapsed = self.collapse_models(state, &runtime)?;
        let overridden = self.apply_model_overrides(state, collapsed)?;
        let bedrock = self.apply_provider_bedrock_overrides(overridden)?;
        let specs: Vec<_> = bedrock.iter().map(|model| model.spec().clone()).collect();
        let metrics = apply_catalog_metrics(&specs, &self.metrics);
        bedrock
            .into_iter()
            .zip(metrics)
            .map(|(model, spec)| {
                let model = if Arc::ptr_eq(model.spec(), &spec) { model } else { model.with_spec(spec)? };
                let (provider, id) = identity(&model)?;
                Ok(state.interned.entry(key(&provider, &id)).or_insert(model).clone())
            })
            .collect()
    }

    fn lookup_locked(
        &self,
        state: &mut RegistryState,
        provider: &WireString,
    ) -> Result<Vec<HostModelRef>, CollapseError> {
        if let Some(models) = &state.full_snapshot {
            return Ok(models.clone());
        }
        let normalized = lower(&trim(provider));
        if normalized.is_empty() {
            return Ok(Vec::new());
        }
        if let Some(models) = state.provider_lookups.get(&normalized) {
            return Ok(models.clone());
        }
        let mut matching = OrderedProviderSet::default();
        for candidate in self.known_static_providers().iter() {
            if lower(candidate) == normalized {
                matching.insert(candidate.clone());
            }
        }
        let models = self.compose_static_models(state, Some(&matching))?;
        state.provider_lookups.insert(normalized, models.clone());
        Ok(models)
    }

    pub fn models_for_provider_lookup(&self, provider: &WireString) -> Result<Vec<HostModelRef>, CollapseError> {
        let mut state = self.state.lock().map_err(|_| fixed_error("Static registry state is unavailable"))?;
        self.lookup_locked(&mut state, provider)
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
        let mut state = self.state.lock().map_err(|_| fixed_error("Static registry state is unavailable"))?;
        let models = self.lookup_locked(&mut state, provider)?;
        if let Some(model) = models.iter().find(|model| {
            text(model.spec(), "provider").as_ref() == Some(provider) && text(model.spec(), "id").as_ref() == Some(id)
        }) {
            return Ok(Some(model.clone()));
        }
        let Some(alias) = state.collapse.resolve_variant_selector(provider, id)? else { return Ok(None) };
        Ok(models.into_iter().find(|model| {
            text(model.spec(), "provider").as_ref() == Some(provider)
                && text(model.spec(), "id").as_ref() == Some(&alias)
        }))
    }

    pub fn get_all(&self) -> Result<Vec<HostModelRef>, CollapseError> {
        let mut state = self.state.lock().map_err(|_| fixed_error("Static registry state is unavailable"))?;
        if let Some(models) = &state.full_snapshot {
            return Ok(models.clone());
        }
        let models = self.compose_static_models(&mut state, None)?;
        state.full_snapshot = Some(models.clone());
        state.provider_lookups.clear();
        Ok(models)
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
        for provider in self.known_static_providers().iter() {
            if requested.contains(&lower(provider)) && is_available(provider) {
                available.insert(provider.clone());
            }
        }
        let mut state = self.state.lock().map_err(|_| fixed_error("Static registry state is unavailable"))?;
        if let Some(models) = &state.full_snapshot {
            return Ok(select_models(models, Some(&available)));
        }
        self.compose_static_models(&mut state, Some(&available))
    }
}
