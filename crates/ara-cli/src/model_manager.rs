//! Fixed OMP model manager, native lossless Host facade.
//! Source: OMP 596f2da, packages/catalog/src/model-manager.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.

use crate::{
    bun_hash,
    catalog_discovery::{CatalogContext, DiscoveryError, DiscoveryResult},
    model_cache::{SqliteModelCache, WireCacheEntry, WireModelCacheWriteOptions},
    model_collapse::{CollapseModelPolicy, SpecRef, VariantSpec, trim, truthy},
    model_identity_wire::{
        self as wire, boolean, copy_field, empty, equals, has_string, nullish, number, spread, text,
    },
    model_wire_policy::WireModelPolicy,
};
use ara_rpc::{WireString, WireValue};
use async_trait::async_trait;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, OnceLock, RwLock},
};

pub const DEFAULT_CACHE_TTL_MS: f64 = 2.0 * 60.0 * 60.0 * 1000.0;
pub const NON_AUTHORITATIVE_RETRY_MS: f64 = 5.0 * 60.0 * 1000.0;
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ModelRefreshStrategy {
    Online,
    Offline,
    #[default]
    OnlineIfUncached,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelResolutionSource {
    Bundled,
    Cache,
    ModelsDev,
    Provider,
}
impl ModelResolutionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bundled => "bundled",
            Self::Cache => "cache",
            Self::ModelsDev => "models.dev",
            Self::Provider => "provider",
        }
    }
}
#[derive(Clone)]
pub struct ModelResolutionResult {
    pub models: Vec<SpecRef>,
    pub stale: bool,
    pub source: ModelResolutionSource,
    pub updated_at: Option<f64>,
}

/// The Symbol fingerprint lives on this particular array object. Mutation does
/// not clear it, matching the source's reference memo rather than content memo.
#[derive(Default)]
pub struct ModelArray {
    models: RwLock<Vec<SpecRef>>,
    fingerprint: OnceLock<WireString>,
}
impl ModelArray {
    pub fn new(models: Vec<SpecRef>) -> Self {
        Self { models: RwLock::new(models), fingerprint: OnceLock::new() }
    }
    pub fn snapshot(&self) -> Vec<SpecRef> {
        self.models.read().expect("model array poisoned").clone()
    }
    pub fn replace(&self, models: Vec<SpecRef>) {
        *self.models.write().expect("model array poisoned") = models;
    }
}
#[derive(Clone)]
pub enum RawModelValue {
    Undefined,
    Value(VariantSpec),
    SharedValue(Arc<VariantSpec>),
    Models(Arc<ModelArray>),
}
impl RawModelValue {
    pub fn models(models: Vec<SpecRef>) -> Self {
        Self::Models(Arc::new(ModelArray::new(models)))
    }
    pub fn null() -> Self {
        Self::Value(VariantSpec::from_wire(WireValue::Null))
    }
    pub fn is_null(&self) -> bool {
        matches!(self,Self::Value(value) if matches!(value.value,WireValue::Null))
            || matches!(self,Self::SharedValue(value) if matches!(value.value,WireValue::Null))
    }
    pub fn from_discovery(result: DiscoveryResult) -> Result<Self, DiscoveryError> {
        result.map(|models| models.map_or_else(Self::null, Self::models))
    }
    pub(crate) fn rows(&self) -> Vec<SpecRef> {
        match self {
            Self::Models(models) => models.snapshot(),
            Self::SharedValue(value) => Self::Value(value.as_ref().clone()).rows(),
            Self::Value(value) if matches!(value.value, WireValue::Array(_)) => value
                .value
                .as_array()
                .expect("array")
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let path: WireString = index.to_string().into();
                    Arc::new(VariantSpec {
                        value: item.clone(),
                        undefined_paths: value
                            .undefined_paths
                            .iter()
                            .filter(|p| p.first() == Some(&path) && p.len() > 1)
                            .map(|p| p[1..].to_vec())
                            .collect(),
                    })
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}
#[async_trait]
pub trait DynamicModelFetcher: Send + Sync {
    async fn fetch(&self) -> Result<RawModelValue, DiscoveryError>;
}
#[async_trait]
pub trait ModelsDevFallback: Send + Sync {
    async fn fetch(&self) -> Result<RawModelValue, DiscoveryError>;
    fn map(&self, payload: RawModelValue, provider_id: &WireString) -> Result<RawModelValue, DiscoveryError>;
    fn additive_only(&self) -> bool {
        false
    }
}
pub trait ModelClock: Send + Sync {
    fn now(&self) -> f64;
}
impl<F: Fn() -> f64 + Send + Sync> ModelClock for F {
    fn now(&self) -> f64 {
        self()
    }
}
pub struct ModelManagerHost {
    pub cache: Arc<Mutex<SqliteModelCache>>,
    pub clock: Arc<dyn ModelClock>,
}
#[derive(Clone)]
pub struct ModelManagerOptions {
    pub provider_id: WireString,
    pub static_models: Option<RawModelValue>,
    pub cache_provider_id: Option<WireString>,
    pub cache_ttl_ms: Option<f64>,
    pub dynamic_models_authoritative: bool,
    pub drop_cached_model_ids_on_static_mismatch: Option<Vec<WireString>>,
    pub restorable_header_fallback: Option<VariantSpec>,
    pub dynamic_fetcher: Option<Arc<dyn DynamicModelFetcher>>,
    pub models_dev: Option<Arc<dyn ModelsDevFallback>>,
}
impl ModelManagerOptions {
    pub fn new(provider_id: impl Into<WireString>) -> Self {
        Self {
            provider_id: provider_id.into(),
            static_models: None,
            cache_provider_id: None,
            cache_ttl_ms: None,
            dynamic_models_authoritative: false,
            drop_cached_model_ids_on_static_mismatch: None,
            restorable_header_fallback: None,
            dynamic_fetcher: None,
            models_dev: None,
        }
    }
}
pub struct ModelManager {
    pub options: ModelManagerOptions,
    pub context: CatalogContext,
    pub host: Arc<ModelManagerHost>,
}
impl ModelManager {
    pub fn new(options: ModelManagerOptions, context: CatalogContext, host: Arc<ModelManagerHost>) -> Self {
        Self { options, context, host }
    }
    fn read_cache(&self, key: &WireString, ttl: f64) -> Option<WireCacheEntry> {
        self.host
            .cache
            .lock()
            .expect("model cache poisoned")
            .read_model_cache_wire(key, ttl, || self.host.clock.now())
            .ok()
            .flatten()
    }
    fn write_cache(
        &self,
        key: &WireString,
        at: f64,
        models: &[SpecRef],
        authoritative: bool,
        fingerprint: &WireString,
        static_models: &[SpecRef],
    ) {
        let _ = self.host.cache.lock().expect("model cache poisoned").write_model_cache_wire(
            key,
            at,
            models,
            WireModelCacheWriteOptions {
                authoritative,
                static_fingerprint: fingerprint,
                static_header_sources: static_models,
                restorable_header_fallback: self.options.restorable_header_fallback.as_ref(),
            },
        );
    }
    fn collapse(&self, models: &[SpecRef]) -> Result<Vec<SpecRef>, DiscoveryError> {
        self.context
            .runtime
            .collapse
            .lock()
            .expect("collapse poisoned")
            .collapse_built_variants(models)
            .map_err(Into::into)
    }
    pub async fn refresh(&self, strategy: ModelRefreshStrategy) -> Result<ModelResolutionResult, DiscoveryError> {
        let options = &self.options;
        let key = options.cache_provider_id.as_ref().unwrap_or(&options.provider_id);
        let ttl = options.cache_ttl_ms.unwrap_or(DEFAULT_CACHE_TTL_MS);
        let static_models = match &options.static_models {
            Some(value)
                if match value {
                    RawModelValue::Undefined => false,
                    RawModelValue::Value(spec) => !spec.undefined_paths.contains(&Vec::new()) && truthy(&spec.value),
                    RawModelValue::SharedValue(spec) => {
                        !spec.undefined_paths.contains(&Vec::new()) && truthy(&spec.value)
                    }
                    RawModelValue::Models(_) => true,
                } =>
            {
                pass_model_list(value)?
            }
            _ => wire::bundled_provider_models(&options.provider_id),
        };
        let additive_ids = options
            .models_dev
            .as_ref()
            .filter(|hook| hook.additive_only())
            .filter(|_| !static_models.is_empty())
            .map(|_| ids(&static_models));
        let cache = self.read_cache(key, ttl);
        let restored =
            restore_cached_model_headers(cache.as_ref(), &static_models, options.restorable_header_fallback.as_ref())?;
        let usable: Vec<_> =
            restored.models.iter().filter(|model| !restored.unresolved.contains(&id(model))).cloned().collect();
        let mut fingerprint =
            fingerprint_static_models(&ModelArray::new(static_models.clone()), options.dynamic_models_authoritative);
        if let Some(drop) = options.drop_cached_model_ids_on_static_mismatch.as_ref().filter(|ids| !ids.is_empty()) {
            let mut joined = Vec::new();
            for (index, id) in drop.iter().enumerate() {
                if index > 0 {
                    joined.push(0);
                }
                joined.extend_from_slice(id.units());
            }
            fingerprint = format!(
                "{}:drop:{}",
                fingerprint.to_utf8().expect("ASCII fingerprint"),
                bun_hash::hash_string_base36(&WireString::from_units(joined))
            )
            .into();
        }
        let fingerprint_matches =
            cache.as_ref().is_some_and(|cache| cache.static_fingerprint == fingerprint) && !fingerprint.is_empty();
        let migration = !fingerprint_matches
            && options
                .drop_cached_model_ids_on_static_mismatch
                .as_ref()
                .is_some_and(|drop| usable.iter().any(|model| drop.contains(&id(model))));
        let fresh = cache.as_ref().is_some_and(|cache| cache.fresh)
            && restored.unresolved.is_empty()
            && !migration
            && (!options.dynamic_models_authoritative || fingerprint_matches);
        let has_dynamic = options.dynamic_fetcher.is_some();
        let has_dev = options.models_dev.is_some();
        let has_remote = has_dynamic || has_dev;
        let authoritative = cache.as_ref().is_some_and(|cache| cache.authoritative) && fresh || !has_remote;
        let age = cache.as_ref().map_or(f64::INFINITY, |cache| self.host.clock.now() - cache.updated_at);
        let fetch = has_remote && should_fetch_remote_sources(strategy, fresh, authoritative, age);
        let filter_additive = |models: Vec<SpecRef>| -> Vec<SpecRef> {
            if let Some(ids) = &additive_ids {
                models.into_iter().filter(|model| !ids.contains(&id(model))).collect()
            } else {
                models
            }
        };
        if !fetch
            && cache.as_ref().is_some_and(|cache| cache.fresh)
            && authoritative
            && fingerprint_matches
            && restored.unresolved.is_empty()
        {
            let contribution = filter_additive(restored.models.clone());
            let models = if additive_ids.is_some() {
                merge_catalog_metrics(&merge_dynamic_models(&static_models, &contribution)?, &restored.models)
            } else {
                restored.models
            };
            let source =
                if contribution.is_empty() { ModelResolutionSource::Bundled } else { ModelResolutionSource::Cache };
            return Ok(ModelResolutionResult {
                models: self.collapse(&models)?,
                stale: false,
                source,
                updated_at: if source == ModelResolutionSource::Cache {
                    cache.as_ref().map(|c| c.updated_at)
                } else {
                    None
                },
            });
        }
        let (dev, dynamic) =
            if fetch { tokio::join!(biased;fetch_models_dev(options),fetch_dynamic(options)) } else { (None, None) };
        let dev_succeeded = dev.is_some();
        let dynamic_succeeded = dynamic.is_some();
        let any = dev_succeeded || dynamic_succeeded;
        // Source fetchModelsDev normalizes before returning, then the resolver
        // normalizes it again. Keep this second build rather than reusing rows.
        let normalized_dev = normalize_model_list(&RawModelValue::models(dev.unwrap_or_default()))?;
        let dev_models = filter_additive(normalized_dev.clone());
        let dynamic = dynamic.unwrap_or_default();
        let complete = options.dynamic_models_authoritative && dynamic_succeeded
            || has_remote && (!has_dev || dev_succeeded) && (!has_dynamic || dynamic_succeeded);
        let prepared = if complete {
            Vec::new()
        } else {
            prepare_cache_models_for_static_mismatch(
                &usable,
                &static_models,
                fingerprint_matches,
                options.drop_cached_model_ids_on_static_mismatch.as_deref(),
            )
        };
        let cache_models = filter_additive(prepared.clone());
        let cache_authoritative = if has_dynamic {
            dynamic_succeeded
                && !dynamic.is_empty()
                && (options.dynamic_models_authoritative || !has_dev || dev_succeeded)
        } else {
            dev_succeeded
        };
        let merged = merge_dynamic_models(&merge_dynamic_models(&static_models, &cache_models)?, &dev_models)?;
        let merged = if additive_ids.is_some() {
            merge_catalog_metrics(&merged, if dev_succeeded { &normalized_dev } else { &prepared })
        } else {
            merged
        };
        let merged = merge_dynamic_models(&merged, &dynamic)?;
        let merged = if options.dynamic_models_authoritative && dynamic_succeeded {
            let retained = ids(&dynamic);
            merged.into_iter().filter(|m| retained.contains(&id(m))).collect()
        } else {
            merged
        };
        let models = self.collapse(&merged)?;
        let resolution_authoritative =
            !has_remote || complete || strategy == ModelRefreshStrategy::OnlineIfUncached && fresh && authoritative;
        let remote_at = any.then(|| self.host.clock.now());
        if fetch {
            if let Some(at) = remote_at {
                self.write_cache(key, at, &models, cache_authoritative, &fingerprint, &static_models);
            } else {
                let latest = self.read_cache(key, ttl);
                let latest = latest.as_ref().or(cache.as_ref());
                let latest =
                    restore_cached_model_headers(latest, &static_models, options.restorable_header_fallback.as_ref())?;
                let latest: Vec<_> =
                    latest.models.into_iter().filter(|model| !latest.unresolved.contains(&id(model))).collect();
                let latest = filter_additive(prepare_cache_models_for_static_mismatch(
                    &latest,
                    &static_models,
                    fingerprint_matches,
                    options.drop_cached_model_ids_on_static_mismatch.as_deref(),
                ));
                let fallback = self
                    .collapse(&merge_dynamic_models(&merge_dynamic_models(&static_models, &latest)?, &dev_models)?)?;
                self.write_cache(key, self.host.clock.now(), &fallback, false, &fingerprint, &static_models);
            }
        }
        let source = if dynamic_succeeded {
            ModelResolutionSource::Provider
        } else if dev_succeeded {
            ModelResolutionSource::ModelsDev
        } else if !cache_models.is_empty() {
            ModelResolutionSource::Cache
        } else {
            ModelResolutionSource::Bundled
        };
        Ok(ModelResolutionResult {
            models,
            stale: !resolution_authoritative,
            source,
            updated_at: remote_at
                .or_else(|| (!cache_models.is_empty()).then(|| cache.as_ref().map(|cache| cache.updated_at)).flatten()),
        })
    }
}

pub fn fingerprint_static_models(models: &ModelArray, authoritative: bool) -> WireString {
    let rows = models.snapshot();
    if rows.is_empty() {
        return "merge-v3:empty".into();
    }
    if authoritative {
        return format!(
            "merge-v3:authoritative:{}",
            fingerprint_static_models(models, false).to_utf8().expect("ASCII fingerprint")
        )
        .into();
    }
    models
        .fingerprint
        .get_or_init(|| {
            format!(
                "merge-v3:{}",
                bun_hash::hash_string_base36(
                    &WireValue::Array(rows.iter().map(|model| model.to_wire_json()).collect()).stringify().into()
                )
            )
            .into()
        })
        .clone()
}
pub fn should_fetch_remote_sources(strategy: ModelRefreshStrategy, fresh: bool, authoritative: bool, age: f64) -> bool {
    match strategy {
        ModelRefreshStrategy::Offline => false,
        ModelRefreshStrategy::Online => true,
        ModelRefreshStrategy::OnlineIfUncached => !fresh || !authoritative && age >= NON_AUTHORITATIVE_RETRY_MS,
    }
}
fn id(model: &VariantSpec) -> WireString {
    text(model, "id").unwrap_or_else(|| "".into())
}
fn ids(models: &[SpecRef]) -> HashSet<WireString> {
    models.iter().map(|model| id(model)).collect()
}
pub fn pass_model_list(value: &RawModelValue) -> Result<Vec<SpecRef>, DiscoveryError> {
    value
        .rows()
        .into_iter()
        .filter(|item| matches!(item.value, WireValue::Object(_) | WireValue::Array(_)) && text(item, "id").is_some())
        .map(|item| WireModelPolicy.build(&item).map(Arc::new).map_err(Into::into))
        .collect()
}
pub fn normalize_model_list(value: &RawModelValue) -> Result<Vec<SpecRef>, DiscoveryError> {
    value
        .rows()
        .into_iter()
        .filter(|item| is_model_like(item))
        .map(|item| WireModelPolicy.build(&item).map(Arc::new).map_err(Into::into))
        .collect()
}
fn token_cost(value: &VariantSpec) -> bool {
    matches!(value.value, WireValue::Object(_))
        && ["input", "output", "cacheRead", "cacheWrite"]
            .iter()
            .all(|key| number(value, key).is_some_and(f64::is_finite))
}
pub fn is_model_like(value: &VariantSpec) -> bool {
    if !matches!(value.value, WireValue::Object(_)) {
        return false;
    }
    if !["id", "name", "api", "provider", "baseUrl"]
        .iter()
        .all(|key| text(value, key).is_some_and(|value| !value.is_empty()))
        || boolean(value, "reasoning").is_none()
    {
        return false;
    }
    if !value.get("input").and_then(WireValue::as_array).is_some_and(|input| {
        !input.is_empty()
            && input
                .iter()
                .all(|value| value.as_string().is_some_and(|s| s.equals_ascii("text") || s.equals_ascii("image")))
    }) {
        return false;
    }
    let Some(cost) = value.record("cost") else {
        return false;
    };
    if !token_cost(&cost) {
        return false;
    }
    if let Some(long) = cost.record("longContext")
        && (!token_cost(&long) || !number(&long, "inputThreshold").is_some_and(|n| n.is_finite() && n > 0.0))
    {
        return false;
    }
    ["contextWindow", "maxTokens"].iter().all(|key| {
        matches!(value.get(key), Some(WireValue::Null)) || number(value, key).is_some_and(|n| n.is_finite() && n > 0.0)
    })
}
async fn fetch_dynamic(options: &ModelManagerOptions) -> Option<Vec<SpecRef>> {
    let hook = options.dynamic_fetcher.as_ref()?;
    let fetched = hook.fetch().await;
    tokio::task::yield_now().await;
    let raw = fetched.ok()?;
    if raw.is_null() {
        return None;
    }
    normalize_model_list(&raw).ok()
}
async fn fetch_models_dev(options: &ModelManagerOptions) -> Option<Vec<SpecRef>> {
    let hook = options.models_dev.as_ref()?;
    let fetched = hook.fetch().await;
    tokio::task::yield_now().await;
    let payload = fetched.ok()?;
    let raw = hook.map(payload, &options.provider_id).ok()?;
    normalize_model_list(&raw).ok()
}
struct Restored {
    models: Vec<SpecRef>,
    unresolved: HashSet<WireString>,
}

/// Shared donor decision for the Manager and opaque Host startup loader.
/// Current markers prove both ordinary donors failed; only legacy markers
/// permit the historical request-model recovery.
pub(crate) fn cached_header_restore_source_id(
    cache: &WireCacheEntry,
    id: &WireString,
    request: Option<&WireString>,
    has_static: &dyn Fn(&WireString) -> bool,
) -> Option<WireString> {
    if cache.unrestorable_header_model_ids.contains(id) {
        if cache.legacy_header_restore_markers { request.filter(|id| has_static(id)).cloned() } else { None }
    } else {
        Some(id).filter(|id| has_static(id)).or_else(|| request.filter(|id| has_static(id))).cloned()
    }
}
fn restore_cached_model_headers(
    cache: Option<&WireCacheEntry>,
    static_models: &[SpecRef],
    fallback: Option<&VariantSpec>,
) -> Result<Restored, DiscoveryError> {
    let raw = cache.map_or(RawModelValue::models(Vec::new()), |cache| RawModelValue::Value(cache.models.clone()));
    let models = pass_model_list(&raw)?;
    let Some(cache) = cache.filter(|cache| !cache.header_omitted_model_ids.is_empty()) else {
        return Ok(Restored { models, unresolved: HashSet::new() });
    };
    let static_by_id: HashMap<_, _> = static_models.iter().map(|model| (id(model), model)).collect();
    let mut unresolved = HashSet::new();
    let models = models
        .into_iter()
        .map(|model| {
            let id = id(&model);
            if !cache.header_omitted_model_ids.contains(&id) {
                return model;
            }
            let unrestorable = cache.unrestorable_header_model_ids.contains(&id);
            let request = text(&model, "requestModelId").filter(|id| !id.is_empty());
            let source_id =
                cached_header_restore_source_id(cache, &id, request.as_ref(), &|id| static_by_id.contains_key(id));
            let source = source_id.as_ref().and_then(|id| static_by_id.get(id));
            let headers = source.and_then(|source| source.record("headers")).filter(|v| truthy(&v.value));
            let headers = headers.or_else(|| (!unrestorable).then(|| fallback.cloned()).flatten());
            if let Some(headers) = headers {
                let mut out = (*model).clone();
                out.set_record("headers", &headers);
                Arc::new(out)
            } else {
                unresolved.insert(id);
                model
            }
        })
        .collect();
    Ok(Restored { models, unresolved })
}
pub fn prepare_cache_models_for_static_mismatch(
    models: &[SpecRef],
    static_models: &[SpecRef],
    matches: bool,
    drop: Option<&[WireString]>,
) -> Vec<SpecRef> {
    if matches {
        return models.to_vec();
    }
    let static_ids = ids(static_models);
    models
        .iter()
        .filter(|model| !drop.is_some_and(|drop| drop.contains(&id(model))))
        .map(|model| {
            if static_ids.contains(&id(model)) {
                let mut out = (**model).clone();
                out.set("contextWindow", WireValue::Null);
                out.set("maxTokens", WireValue::Null);
                Arc::new(out)
            } else {
                model.clone()
            }
        })
        .collect()
}
pub fn merge_catalog_metrics(models: &[SpecRef], catalog: &[SpecRef]) -> Vec<SpecRef> {
    if models.is_empty() || catalog.is_empty() {
        models.to_vec()
    } else {
        wire::apply_catalog_metrics(models, &wire::CatalogMetricsIndex::new(catalog))
    }
}
pub fn merge_dynamic_models(base: &[SpecRef], dynamic: &[SpecRef]) -> Result<Vec<SpecRef>, DiscoveryError> {
    if dynamic.is_empty() {
        return Ok(base.to_vec());
    }
    if base.is_empty() {
        return Ok(dynamic.to_vec());
    }
    let mut out: Vec<SpecRef> = Vec::new();
    let mut positions: HashMap<WireString, usize> = HashMap::new();
    for model in base {
        let id = id(model);
        if let Some(&position) = positions.get(&id) {
            out[position] = model.clone();
        } else {
            positions.insert(id, out.len());
            out.push(model.clone());
        }
    }
    for model in dynamic {
        let id = id(model);
        if id.is_empty() {
            continue;
        }
        if let Some(&position) = positions.get(&id) {
            out[position] = Arc::new(merge_dynamic_model(&out[position], model)?);
        } else {
            positions.insert(id, out.len());
            out.push(model.clone());
        }
    }
    Ok(out)
}
pub fn prefer_discovery_limit(discovery: Option<&WireValue>, fallback: Option<&WireValue>) -> Option<WireValue> {
    match discovery.and_then(WireValue::as_number).filter(|n| n.is_finite() && *n > 0.0) {
        Some(4096.0) if fallback.and_then(WireValue::as_number).is_some_and(|n| n > 4096.0) => fallback.cloned(),
        Some(n) => Some(WireValue::Number(n)),
        None => fallback.cloned(),
    }
}
pub fn merge_dynamic_model(existing: &VariantSpec, dynamic: &VariantSpec) -> Result<VariantSpec, DiscoveryError> {
    let endpoint_changed = !equals(existing.get("baseUrl"), dynamic.get("baseUrl"));
    let both = |provider: &str| {
        text(existing, "provider").is_some_and(|p| p.equals_ascii(provider))
            && text(dynamic, "provider").is_some_and(|p| p.equals_ascii(provider))
    };
    let image = if endpoint_changed || both("github-copilot") || both("deepinfra") {
        has_string(dynamic, "input", "image")
    } else {
        has_string(existing, "input", "image") || has_string(dynamic, "input", "image")
    };
    let reasoning = if both("synthetic") {
        boolean(dynamic, "reasoning").unwrap_or(false)
    } else {
        boolean(existing, "reasoning").unwrap_or(false) || boolean(dynamic, "reasoning").unwrap_or(false)
    };
    let mut out = spread(&[existing, dynamic]);
    let dynamic_name = trim(&text(dynamic, "name").unwrap_or_else(|| "".into()));
    let existing_name = text(existing, "name").unwrap_or_else(|| "".into());
    let model_id = id(dynamic);
    out.set(
        "name",
        WireValue::String(if dynamic_name.is_empty() || dynamic_name == model_id && existing_name != model_id {
            existing_name
        } else {
            dynamic_name
        }),
    );
    out.set("reasoning", WireValue::Bool(reasoning));
    out.set(
        "input",
        WireValue::Array(if image {
            vec![wire::str_value("text"), wire::str_value("image")]
        } else {
            vec![wire::str_value("text")]
        }),
    );
    let ec = existing.record("cost").unwrap_or_else(empty);
    let dc = dynamic.record("cost").unwrap_or_else(empty);
    let mut cost = empty();
    for key in ["input", "output", "cacheRead", "cacheWrite"] {
        if let Some(value) = number(&dc, key).filter(|v| v.is_finite() && *v > 0.0) {
            cost.set(key, WireValue::Number(value));
        } else {
            copy_field(&mut cost, key, &ec, key);
        }
    }
    let long =
        dc.record("longContext").filter(|v| !matches!(v.value, WireValue::Null)).or_else(|| ec.record("longContext"));
    if let Some(long) = long.filter(|v| truthy(&v.value)) {
        cost.set_record("longContext", &long);
    }
    out.set_record("cost", &cost);
    for key in ["contextWindow", "maxTokens"] {
        if let Some(value) = prefer_discovery_limit(dynamic.get(key), existing.get(key)) {
            out.set(key, value);
        } else {
            out.set_undefined(key);
        }
    }
    if let Some(headers) = dynamic.record("headers").filter(|v| truthy(&v.value)) {
        out.set_record("headers", &spread(&[&existing.record("headers").unwrap_or_else(empty), &headers]));
    } else {
        copy_field(&mut out, "headers", existing, "headers");
    }
    for (target, source) in [("compat", "compatConfig"), ("contextPromotionTarget", "contextPromotionTarget")] {
        let chosen = if nullish(dynamic.get(source)) { existing } else { dynamic };
        copy_field(&mut out, target, chosen, source);
    }
    WireModelPolicy.build(&out).map_err(Into::into)
}
