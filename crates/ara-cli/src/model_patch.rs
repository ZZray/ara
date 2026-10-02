//! Host-owned port of fixed OMP `config/model-patch.ts`.
//!
//! Source: 596f2da7101178214aa27a753529d15e6b7ad91d. Open records retain
//! UTF-16, own undefined values and insertion order. Raw headers and API-key
//! configuration belong to an opaque sidecar, never public model metadata.
//!
//! MIT License; Copyright (c) 2025 Mario Zechner;
//! Copyright (c) 2025-2026 Can Bölük; Copyright (c) 2026 Stencil Labs, Inc.
//! See LICENSE for the full license.

use crate::{
    model_collapse::{CollapseError, CollapseModelPolicy, SpecRef, VariantSpec, truthy},
    model_config_values::{HeaderResolutionOptions, HeaderSource, create_live_config_headers},
    model_identity_wire::{copy_field, empty, nullish, spread, text, to_model_spec},
    model_wire_policy::WireModelPolicy,
    provider_models::descriptors::provider_descriptors,
};
use ara_rpc::{WireString, WireValue};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

/// Property presence is separate from the safe metadata object. Config sources
/// are literal records until a source-backed operation wraps them in Live.
#[derive(Clone, Default)]
pub enum HeaderSlot {
    #[default]
    Absent,
    Undefined,
    Null,
    Source(HeaderSource),
}

impl HeaderSlot {
    pub fn as_source(&self) -> Option<&HeaderSource> {
        match self {
            Self::Source(source) => Some(source),
            _ => None,
        }
    }

    pub fn is_own(&self) -> bool {
        !matches!(self, Self::Absent)
    }

    pub fn is_defined(&self) -> bool {
        !matches!(self, Self::Absent | Self::Undefined)
    }
}

/// This type deliberately has no Debug or Serialize implementation.
pub struct HostModel {
    spec: SpecRef,
    headers: HeaderSlot,
}

pub type HostModelRef = Arc<HostModel>;

fn check_safe_metadata(spec: &VariantSpec) -> Result<(), CollapseError> {
    if spec.own_keys().iter().any(|key| key.equals_ascii("headers") || key.equals_ascii("apiKey")) {
        return Err(CollapseError::new("Model transport credentials must use the opaque sidecar".into()));
    }
    Ok(())
}

impl HostModel {
    // Construction owns the opaque reference identity used by registry merges.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(spec: SpecRef, headers: HeaderSlot) -> Result<HostModelRef, CollapseError> {
        check_safe_metadata(&spec)?;
        Ok(Arc::new(Self { spec, headers }))
    }

    pub fn spec(&self) -> &SpecRef {
        &self.spec
    }

    pub fn headers(&self) -> &HeaderSlot {
        &self.headers
    }

    /// Used by a Host adapter with the actual source donor, not an ID lookup.
    pub fn with_spec(&self, spec: SpecRef) -> Result<HostModelRef, CollapseError> {
        Self::new(spec, self.headers.clone())
    }
}

/// The complete patchable field set stays lossless in `fields`. Unknown fields
/// may be retained for a custom-definition owner but are not applied as patches.
#[derive(Clone)]
pub struct ModelPatch {
    fields: VariantSpec,
    headers: HeaderSlot,
}

impl ModelPatch {
    pub fn new(fields: VariantSpec, headers: HeaderSlot) -> Result<Self, CollapseError> {
        check_safe_metadata(&fields)?;
        Ok(Self { fields, headers })
    }
    pub fn fields(&self) -> &VariantSpec {
        &self.fields
    }
    pub fn headers(&self) -> &HeaderSlot {
        &self.headers
    }
}

/// Includes baseUrl/authHeader/compat/remoteCompaction/transport and all four
/// guardrail/requestMetadata fields in `fields`; raw API key remains opaque.
#[derive(Clone)]
pub struct ProviderOverride {
    fields: VariantSpec,
    headers: HeaderSlot,
    api_key_config: Option<String>,
}

impl ProviderOverride {
    pub fn new(
        fields: VariantSpec,
        headers: HeaderSlot,
        api_key_config: Option<String>,
    ) -> Result<Self, CollapseError> {
        check_safe_metadata(&fields)?;
        Ok(Self { fields, headers, api_key_config })
    }
    pub fn fields(&self) -> &VariantSpec {
        &self.fields
    }
    pub fn headers(&self) -> &HeaderSlot {
        &self.headers
    }
    pub fn api_key_config(&self) -> Option<&str> {
        self.api_key_config.as_deref()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ModelTransportPolicy {
    Merge,
    Replace,
}

pub fn to_host_model_spec(model: &HostModelRef) -> VariantSpec {
    to_model_spec(model.spec())
}

/// Build never reads or resolves transport credentials. The source sidecar is
/// retained across builder clones and catalog corrections.
pub fn build_host_model(spec: &VariantSpec, headers: HeaderSlot) -> Result<HostModelRef, CollapseError> {
    check_safe_metadata(spec)?;
    HostModel::new(Arc::new(WireModelPolicy.build(spec)?), headers)
}

/// An explicit live-header assignment has own-undefined presence when no live
/// source or auth option exists, matching createLiveConfigHeaders's return.
pub fn merge_header_sources(sources: &[HeaderSlot], options: HeaderResolutionOptions) -> HeaderSlot {
    let sources: Vec<_> = sources.iter().map(|slot| slot.as_source().cloned()).collect();
    create_live_config_headers(&sources, options)
        .map(|live| HeaderSlot::Source(HeaderSource::Live(live)))
        .unwrap_or(HeaderSlot::Undefined)
}

fn is_record(spec: &VariantSpec) -> bool {
    matches!(spec.value, WireValue::Object(_))
}
fn is_truthy(spec: &VariantSpec) -> bool {
    truthy(&spec.value)
}

/// JavaScript record-recursive merge; arrays, null, primitives and own
/// undefined values replace the old value instead of being deep-merged.
pub fn merge_compat(base: Option<&VariantSpec>, override_: Option<&VariantSpec>) -> Option<VariantSpec> {
    let Some(base) = base.filter(|value| is_truthy(value)) else {
        return override_.filter(|value| !matches!(value.value, WireValue::Null)).cloned();
    };
    let Some(override_) = override_.filter(|value| is_truthy(value)) else { return Some(base.clone()) };
    let mut merged = spread(&[base]);
    let override_entries = spread(&[override_]);
    for key in override_entries.own_keys() {
        let old = base.record_key(&key);
        let next = override_entries.record_key(&key);
        if let (Some(old), Some(next)) = (&old, &next)
            && is_record(old)
            && is_record(next)
        {
            merged.set_key_record(&key, &merge_compat(Some(old), Some(next)).expect("records are truthy"));
        } else if let Some(next) = next {
            merged.set_key_record(&key, &next);
        } else {
            merged.set_key_undefined(&key);
        }
    }
    Some(merged)
}

/// Ref form preserves direct-return object identity at the public boundary.
pub fn merge_compat_refs(base: Option<&SpecRef>, override_: Option<&SpecRef>) -> Option<SpecRef> {
    if base.is_none_or(|value| !is_truthy(value)) {
        return override_.filter(|value| !matches!(value.value, WireValue::Null)).cloned();
    }
    if override_.is_none_or(|value| !is_truthy(value)) {
        return base.cloned();
    }
    merge_compat(base.map(AsRef::as_ref), override_.map(AsRef::as_ref)).map(Arc::new)
}

pub fn merge_remote_compaction_config(
    base: Option<&VariantSpec>,
    override_: Option<&VariantSpec>,
) -> Option<VariantSpec> {
    let Some(base) = base.filter(|value| is_truthy(value)) else { return override_.cloned() };
    let Some(override_) = override_.filter(|value| is_truthy(value)) else { return Some(base.clone()) };
    Some(spread(&[base, override_]))
}

pub fn merge_remote_compaction_config_refs(base: Option<&SpecRef>, override_: Option<&SpecRef>) -> Option<SpecRef> {
    if base.is_none_or(|value| !is_truthy(value)) {
        return override_.cloned();
    }
    if override_.is_none_or(|value| !is_truthy(value)) {
        return base.cloned();
    }
    Some(Arc::new(spread(&[base.unwrap(), override_.unwrap()])))
}

pub fn merge_provider_remote_compaction_config(
    model: Option<&VariantSpec>,
    provider: Option<&VariantSpec>,
) -> Option<VariantSpec> {
    merge_remote_compaction_config(provider, model)
}

fn assign(target: &mut VariantSpec, key: &str, value: Option<&VariantSpec>) {
    if let Some(value) = value {
        target.set_record(key, value);
    } else {
        target.set_undefined(key);
    }
}

fn nullish_chain(records: &[Option<&VariantSpec>], key: &str) -> Option<VariantSpec> {
    for (index, record) in records.iter().enumerate() {
        let value = record.and_then(|record| record.record(key));
        if index + 1 == records.len() || value.as_ref().is_some_and(|value| !matches!(value.value, WireValue::Null)) {
            return value;
        }
    }
    None
}

pub fn merge_discovered_model(
    model: &HostModelRef,
    existing: Option<&HostModelRef>,
    provider_override: Option<&ProviderOverride>,
) -> Result<HostModelRef, CollapseError> {
    if existing.is_none() && provider_override.is_none() {
        return Ok(model.clone());
    }
    let mut spec = to_host_model_spec(model);
    let provider = provider_override.map(ProviderOverride::fields);
    let old = existing.map(|model| model.spec().as_ref());
    let discovered = model.spec().as_ref();
    let base_url = if existing.is_some() {
        nullish_chain(&[provider, Some(discovered), old], "baseUrl")
    } else {
        nullish_chain(&[provider, Some(discovered)], "baseUrl")
    };
    assign(&mut spec, "baseUrl", base_url.as_ref());
    let headers = merge_header_sources(
        &[
            existing.map(|model| model.headers().clone()).unwrap_or_default(),
            model.headers().clone(),
            provider_override.map(|value| value.headers().clone()).unwrap_or_default(),
        ],
        HeaderResolutionOptions {
            auth_header: provider.and_then(|value| value.get("authHeader")).is_some_and(truthy),
            api_key_config: provider_override.and_then(ProviderOverride::api_key_config).map(str::to_owned),
        },
    );
    if existing.is_some() {
        let transport = nullish_chain(&[provider, old, Some(discovered)], "transport");
        assign(&mut spec, "transport", transport.as_ref());
        let supports = nullish_chain(&[Some(discovered), old], "supportsTools");
        if let Some(supports) = supports {
            spec.set_record("supportsTools", &supports);
        }
    } else if let Some(provider) = provider
        && provider.get("transport").is_some()
    {
        copy_field(&mut spec, "transport", provider, "transport");
    }
    let existing_remote = old.and_then(|old| old.record("remoteCompaction"));
    let discovered_remote = discovered.record("remoteCompaction");
    let merged_remote = if existing.is_some() {
        merge_remote_compaction_config(existing_remote.as_ref(), discovered_remote.as_ref())
    } else {
        discovered_remote
    };
    let provider_remote = provider.and_then(|provider| provider.record("remoteCompaction"));
    let remote = merge_provider_remote_compaction_config(merged_remote.as_ref(), provider_remote.as_ref());
    assign(&mut spec, "remoteCompaction", remote.as_ref());
    let base_compat = discovered.record("compatConfig");
    let provider_compat = provider.and_then(|provider| provider.record("compat"));
    let compat = merge_compat(base_compat.as_ref(), provider_compat.as_ref());
    assign(&mut spec, "compat", compat.as_ref());
    build_host_model(&spec, headers)
}

/// Set iteration has first-insertion order, matching JavaScript Set.
#[derive(Clone, Default, Debug)]
pub struct OrderedProviderSet {
    providers: Vec<WireString>,
}
impl OrderedProviderSet {
    pub fn insert(&mut self, provider: WireString) -> bool {
        if self.contains(&provider) {
            return false;
        }
        self.providers.push(provider);
        true
    }
    pub fn contains(&self, provider: &WireString) -> bool {
        self.providers.contains(provider)
    }
    pub fn iter(&self) -> impl Iterator<Item = &WireString> {
        self.providers.iter()
    }
    pub fn len(&self) -> usize {
        self.providers.len()
    }
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

pub fn authoritative_runtime_catalog_providers() -> &'static OrderedProviderSet {
    static PROVIDERS: OnceLock<OrderedProviderSet> = OnceLock::new();
    PROVIDERS.get_or_init(|| {
        let mut providers = OrderedProviderSet::default();
        for descriptor in provider_descriptors() {
            if descriptor.dynamic_models_authoritative == Some(true) {
                providers.insert(descriptor.provider_id.clone());
            }
        }
        providers
    })
}

pub fn providers_with_authoritative_project_catalog(models: &[HostModelRef]) -> OrderedProviderSet {
    let mut providers = OrderedProviderSet::default();
    let endpoint: Vec<_> = "/endpoints/openapi".encode_utf16().collect();
    for model in models {
        let spec = model.spec();
        if text(spec, "provider").is_some_and(|value| value.equals_ascii("google-vertex"))
            && text(spec, "api").is_some_and(|value| value.equals_ascii("openai-completions"))
            && text(spec, "baseUrl")
                .is_some_and(|value| value.units().windows(endpoint.len()).any(|part| part == endpoint))
        {
            providers.insert(WireString::from("google-vertex"));
        }
    }
    providers
}

pub fn drop_provider_models(models: &[HostModelRef], providers: &OrderedProviderSet) -> Vec<HostModelRef> {
    models
        .iter()
        .filter(|model| text(model.spec(), "provider").is_none_or(|provider| !providers.contains(&provider)))
        .cloned()
        .collect()
}

fn model_key(provider: &WireString, id: &WireString) -> WireString {
    let mut units = provider.units().to_vec();
    units.push(0);
    units.extend_from_slice(id.units());
    WireString::from_units(units)
}

/// A callback lets incoming custom definitions retain their own opaque type.
/// Concatenation deliberately keeps the upstream embedded-NUL key collisions.
pub fn merge_by_model_key<T>(
    base: &[HostModelRef],
    incoming: &[T],
    key_of: impl Fn(&T) -> (WireString, WireString),
    mut combine: impl FnMut(Option<&HostModelRef>, &T) -> Result<HostModelRef, CollapseError>,
) -> Result<Vec<HostModelRef>, CollapseError> {
    let mut merged = base.to_vec();
    let mut index = HashMap::new();
    for (i, model) in merged.iter().enumerate() {
        let provider = text(model.spec(), "provider")
            .ok_or_else(|| CollapseError::new("Model provider must be a string".into()))?;
        let id = text(model.spec(), "id").ok_or_else(|| CollapseError::new("Model id must be a string".into()))?;
        index.insert(model_key(&provider, &id), i);
    }
    for entry in incoming {
        let (provider, id) = key_of(entry);
        let key = model_key(&provider, &id);
        if let Some(i) = index.get(&key).copied() {
            merged[i] = combine(Some(&merged[i]), entry)?;
        } else {
            let model = combine(None, entry)?;
            index.insert(key, merged.len());
            merged.push(model);
        }
    }
    Ok(merged)
}

fn cost_property(cost: Option<&VariantSpec>, key: &str) -> Result<Option<VariantSpec>, CollapseError> {
    let Some(cost) = cost.filter(|cost| !matches!(cost.value, WireValue::Null)) else {
        return Err(CollapseError::new("Cannot read a cost property of null or undefined".into()));
    };
    Ok(cost.record(key))
}

fn patch_cost(base: &VariantSpec, patch: &VariantSpec) -> Result<VariantSpec, CollapseError> {
    let mut cost = empty();
    let base_cost = base.record("cost");
    let long_context = if nullish(patch.get("longContext")) {
        cost_property(base_cost.as_ref(), "longContext")?
    } else {
        patch.record("longContext")
    };
    for key in ["input", "output", "cacheRead", "cacheWrite"] {
        let value = if nullish(patch.get(key)) { cost_property(base_cost.as_ref(), key)? } else { patch.record(key) };
        assign(&mut cost, key, value.as_ref());
    }
    if let Some(long_context) = long_context.filter(is_truthy) {
        cost.set_record("longContext", &long_context);
    }
    Ok(cost)
}

pub fn apply_model_patch(
    base: &HostModelRef,
    patch: &ModelPatch,
    transport: ModelTransportPolicy,
) -> Result<HostModelRef, CollapseError> {
    let fields = patch.fields();
    let mut result = base.spec().as_ref().clone();
    for key in [
        "name",
        "reasoning",
        "thinking",
        "input",
        "tokenizer",
        "imageInputDecoder",
        "supportsTools",
        "contextWindow",
        "maxTokens",
        "omitMaxOutputTokens",
        "preferWebsockets",
        "contextPromotionTarget",
        "compactionModel",
        "premiumMultiplier",
    ] {
        if fields.get(key).is_some() {
            copy_field(&mut result, key, fields, key);
        }
    }
    if let Some(remote) = fields.record("remoteCompaction") {
        let base_remote = base.spec().record("remoteCompaction");
        let remote = merge_remote_compaction_config(base_remote.as_ref(), Some(&remote));
        assign(&mut result, "remoteCompaction", remote.as_ref());
    }
    let authored_cost =
        fields.record("cost").filter(is_truthy).map(|cost| patch_cost(base.spec(), &cost)).transpose()?;
    if let Some(cost) = &authored_cost {
        result.set_record("cost", cost);
    }
    let (headers, compat) = match transport {
        ModelTransportPolicy::Merge => {
            let headers = if patch.headers().as_source().is_some() {
                merge_header_sources(
                    &[base.headers().clone(), patch.headers().clone()],
                    HeaderResolutionOptions::default(),
                )
            } else {
                base.headers().clone()
            };
            let base_compat = base.spec().record("compatConfig");
            let patch_compat = fields.record("compat");
            (headers, merge_compat(base_compat.as_ref(), patch_compat.as_ref()))
        }
        ModelTransportPolicy::Replace => {
            let headers = if matches!(patch.headers(), HeaderSlot::Absent) {
                HeaderSlot::Undefined
            } else {
                patch.headers().clone()
            };
            (headers, fields.record("compat"))
        }
    };
    let mut spec = to_model_spec(&result);
    assign(&mut spec, "compat", compat.as_ref());
    let mut built = WireModelPolicy.build(&spec)?;
    if fields.get("thinking").is_some() && built.get("thinking").is_some() {
        copy_field(&mut built, "thinking", fields, "thinking");
    }
    for key in ["contextWindow", "maxTokens", "input"] {
        if fields.get(key).is_some() {
            copy_field(&mut built, key, fields, key);
        }
    }
    if let Some(cost) = authored_cost {
        built.set_record("cost", &cost);
    }
    HostModel::new(Arc::new(built), headers)
}

pub fn apply_model_override(model: &HostModelRef, override_: &ModelPatch) -> Result<HostModelRef, CollapseError> {
    apply_model_patch(model, override_, ModelTransportPolicy::Merge)
}
