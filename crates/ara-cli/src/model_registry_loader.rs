//! Host loading seams from fixed OMP `config/model-registry.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d, principally loadCustomModels,
//! loadModels and the two startup cache loaders. Composition, runtime discovery,
//! refresh orchestration and authentication authority remain Coordinator-owned.
//!
//! MIT License; Copyright (c) 2025 Mario Zechner;
//! Copyright (c) 2025-2026 Can Bölük; Copyright (c) 2026 Stencil Labs, Inc.
//! See LICENSE and THIRD_PARTY_NOTICES.md. Secret-bearing owners have no
//! Debug/Serialize implementation. Cache and composition locks never surround
//! configuration helpers; cache ports return owned snapshots before callbacks.

use crate::{
    custom_models::merge_auth_header_sources,
    model_cache::{SqliteModelCache, WireCacheEntry, WireModelCacheWriteOptions},
    model_collapse::{CollapseError, SpecRef, VariantSpec, trim, truthy},
    model_config_file::{ConfigFileError, ModelConfigLoad, ModelsConfigFile},
    model_config_values::{
        ConfigValueContext, ConfigValueEnvironment, ConfigValueResolver, HeaderConfigRecord, HeaderSource,
        ResolveConfigValueOptions, is_command_config_value, resolve_config_headers,
    },
    model_identity_wire::{boolean, empty, text},
    model_manager::{
        ModelArray, ModelClock, ModelResolutionSource, cached_header_restore_source_id, fingerprint_static_models,
    },
    model_patch::{
        HeaderSlot, HostModelRef, OrderedProviderSet, authoritative_runtime_catalog_providers, build_host_model,
        merge_compat, merge_provider_remote_compaction_config, providers_with_authoritative_project_catalog,
        to_host_model_spec,
    },
    models_config::ModelsConfig,
    provider_models::{
        ProviderEnvironment, is_credential_scoped_model_cache_provider, models_dev_catalog_provider_ids,
        provider_descriptors, resolve_model_cache_provider_id, resolve_ollama_model_cache_provider_id,
    },
};
use ara_rpc::{WireString, WireValue};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::Path,
    sync::{Arc, Mutex},
};

pub const STARTUP_CACHE_TTL_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryLoaderError {
    Cancelled,
    ConfigMtime,
    CacheRead,
    CacheWrite,
    InvalidCacheModels,
    InvalidHeaders,
    ModelPreparation,
}
impl fmt::Display for RegistryLoaderError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::Cancelled => "registry loading was cancelled",
            Self::ConfigMtime => "models configuration metadata could not be read",
            Self::CacheRead => "model cache could not be read",
            Self::CacheWrite => "model cache repair could not be written",
            Self::InvalidCacheModels => "model cache rows have an invalid shape",
            Self::InvalidHeaders => "model header source has an invalid shape",
            Self::ModelPreparation => "cached model preparation failed",
        })
    }
}
impl std::error::Error for RegistryLoaderError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderDiscoveryStatus {
    Idle,
    Ok,
    Empty,
    Cached,
    Unavailable,
    Unauthenticated,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderDiscoveryState {
    pub provider: WireString,
    pub status: ProviderDiscoveryStatus,
    pub optional: bool,
    pub stale: bool,
    pub fetched_at: Option<f64>,
    pub source: Option<ModelResolutionSource>,
    pub models: Vec<WireString>,
}

/// Safe failure category only; neither a command nor any credential is included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryLoaderNotice {
    pub provider: WireString,
    pub stage: RegistryLoaderError,
}

pub trait RegistryCachePort {
    fn read(
        &self,
        provider: &WireString,
        ttl_ms: f64,
        clock: &dyn ModelClock,
    ) -> Result<Option<WireCacheEntry>, RegistryLoaderError>;
    fn repair(
        &self,
        provider: &WireString,
        entry: &WireCacheEntry,
        models: &[HostModelRef],
        fallback: &HeaderSlot,
    ) -> Result<(), RegistryLoaderError>;
}

fn literal_headers(slot: &HeaderSlot) -> Result<Option<VariantSpec>, RegistryLoaderError> {
    match slot {
        HeaderSlot::Source(HeaderSource::Config(record)) => {
            let mut headers = empty();
            for (name, value) in record.snapshot() {
                headers.set(&name, WireValue::String(value.into()));
            }
            Ok(Some(headers))
        }
        HeaderSlot::Source(HeaderSource::Live(_)) => Err(RegistryLoaderError::InvalidHeaders),
        _ => Ok(None),
    }
}

fn cache_repair(
    cache: &SqliteModelCache,
    provider: &WireString,
    entry: &WireCacheEntry,
    models: &[HostModelRef],
    fallback: &HeaderSlot,
) -> Result<(), RegistryLoaderError> {
    // This transient storage projection is private. HostModel's safe spec never
    // receives headers; the existing cache writer strips every header before I/O.
    let raw: Vec<SpecRef> = models.iter().map(storage_spec).collect::<Result<_, RegistryLoaderError>>()?;
    let fallback = literal_headers(fallback)?;
    cache
        .write_model_cache_wire(
            provider,
            entry.updated_at,
            &raw,
            WireModelCacheWriteOptions {
                authoritative: entry.authoritative,
                static_fingerprint: &entry.static_fingerprint,
                static_header_sources: &[],
                restorable_header_fallback: fallback.as_ref(),
            },
        )
        .map_err(|_| RegistryLoaderError::CacheWrite)
}

impl RegistryCachePort for SqliteModelCache {
    fn read(
        &self,
        provider: &WireString,
        ttl_ms: f64,
        clock: &dyn ModelClock,
    ) -> Result<Option<WireCacheEntry>, RegistryLoaderError> {
        self.read_model_cache_wire(provider, ttl_ms, || clock.now()).map_err(|_| RegistryLoaderError::CacheRead)
    }
    fn repair(
        &self,
        provider: &WireString,
        entry: &WireCacheEntry,
        models: &[HostModelRef],
        fallback: &HeaderSlot,
    ) -> Result<(), RegistryLoaderError> {
        cache_repair(self, provider, entry, models, fallback)
    }
}
impl RegistryCachePort for Mutex<SqliteModelCache> {
    fn read(
        &self,
        provider: &WireString,
        ttl_ms: f64,
        clock: &dyn ModelClock,
    ) -> Result<Option<WireCacheEntry>, RegistryLoaderError> {
        self.lock().map_err(|_| RegistryLoaderError::CacheRead)?.read(provider, ttl_ms, clock)
    }
    fn repair(
        &self,
        provider: &WireString,
        entry: &WireCacheEntry,
        models: &[HostModelRef],
        fallback: &HeaderSlot,
    ) -> Result<(), RegistryLoaderError> {
        self.lock().map_err(|_| RegistryLoaderError::CacheWrite)?.repair(provider, entry, models, fallback)
    }
}

pub struct RegistryLoaderHost<'a> {
    pub project_dir: &'a Path,
    pub config_values: &'a ConfigValueResolver,
    pub config_environment: &'a dyn ConfigValueEnvironment,
    pub provider_environment: &'a dyn ProviderEnvironment,
    pub cache: &'a dyn RegistryCachePort,
    pub clock: &'a dyn ModelClock,
    pub has_auth: &'a dyn Fn(&WireString) -> bool,
    /// Some installs a resolved key; None removes a failed command-backed key.
    /// The Coordinator's config layer is volatile, not persisted account data.
    pub install_config_key: &'a dyn Fn(&WireString, Option<&str>),
    pub continue_loading: &'a dyn Fn() -> bool,
}
impl RegistryLoaderHost<'_> {
    fn check(&self) -> Result<(), RegistryLoaderError> {
        if (self.continue_loading)() { Ok(()) } else { Err(RegistryLoaderError::Cancelled) }
    }
    fn context(&self) -> ConfigValueContext<'_> {
        ConfigValueContext { project_dir: self.project_dir, environment: self.config_environment }
    }
}

#[derive(Clone, Default)]
pub struct RegistryLoadOptions {
    pub ignore_local_model_config: bool,
    pub disabled_providers: OrderedProviderSet,
}

/// Includes raw and resolved private values; never serialize or format this owner.
pub struct LoadedProviderConfig {
    pub provider: WireString,
    key_config: Option<String>,
    eager_key: Option<String>,
    headers: HeaderSlot,
    resolved_headers: HeaderSlot,
    command_values: Vec<String>,
    fields: VariantSpec,
    override_present: bool,
}
impl LoadedProviderConfig {
    pub fn key_config(&self) -> Option<&str> {
        self.key_config.as_deref()
    }
    pub fn eager_key(&self) -> Option<&str> {
        self.eager_key.as_deref()
    }
    pub fn headers(&self) -> &HeaderSlot {
        &self.headers
    }
    pub fn resolved_headers(&self) -> &HeaderSlot {
        &self.resolved_headers
    }
    pub fn command_values(&self) -> &[String] {
        &self.command_values
    }
    pub fn fields(&self) -> &VariantSpec {
        &self.fields
    }
    pub fn override_present(&self) -> bool {
        self.override_present
    }
}

#[derive(Clone)]
pub struct RegistryDiscoveryConfig {
    pub provider: WireString,
    pub api: WireString,
    pub base_url: Option<WireString>,
    headers: HeaderSlot,
    pub compat: Option<VariantSpec>,
    pub remote_compaction: Option<VariantSpec>,
    pub discovery: VariantSpec,
    pub optional: bool,
}
impl RegistryDiscoveryConfig {
    pub fn headers(&self) -> &HeaderSlot {
        &self.headers
    }
    pub fn kind(&self) -> Option<WireString> {
        text(&self.discovery, "type")
    }
    pub fn cache_provider_id(&self) -> WireString {
        match self.kind().and_then(|kind| kind.to_utf8().ok()).as_deref() {
            Some("ollama") => resolve_ollama_model_cache_provider_id(&self.provider, self.base_url.as_ref()),
            Some("openai-models-list") => {
                let suffix = if boolean(&self.discovery, "injectV1") == Some(false) {
                    ":openai-models-list-bare-context-v3"
                } else {
                    ":openai-models-list-context-v3"
                };
                let mut key = self.provider.clone();
                key.append_str(suffix);
                key
            }
            Some("litellm") => {
                let mut key = self.provider.clone();
                key.append_str(":litellm-rich-v4");
                key
            }
            _ => self.provider.clone(),
        }
    }
}

pub struct LoadedRegistryConfig {
    config: Option<ModelsConfig>,
    pub found: bool,
    pub error: Option<ConfigFileError>,
    pub mtime_ms: Option<f64>,
    pub providers: Vec<LoadedProviderConfig>,
    pub discoverable_providers: Vec<RegistryDiscoveryConfig>,
    pub keyless_providers: OrderedProviderSet,
    pub configured_providers: OrderedProviderSet,
}
impl LoadedRegistryConfig {
    pub fn config(&self) -> Option<&ModelsConfig> {
        self.config.as_ref()
    }
    pub fn provider(&self, id: &WireString) -> Option<&LoadedProviderConfig> {
        self.providers.iter().find(|provider| provider.provider == *id)
    }
}

pub struct StandardProviderSlice {
    pub provider: WireString,
    pub models: Vec<HostModelRef>,
    pub authoritative: bool,
    pub discovery_state: Option<ProviderDiscoveryState>,
    pub newly_loaded: bool,
}

pub struct RegistryLoader {
    config_file: ModelsConfigFile,
    loaded: LoadedRegistryConfig,
    startup_roster: Vec<WireString>,
    pending: HashSet<WireString>,
    standard: HashMap<WireString, Vec<HostModelRef>>,
    authoritative: OrderedProviderSet,
    discoverable: Vec<HostModelRef>,
    states: Vec<ProviderDiscoveryState>,
    notices: Vec<RegistryLoaderNotice>,
}

/// The roster is derived from the existing fixed descriptor callbacks, not an
/// independently maintained list of standard providers.
pub fn startup_model_cache_provider_ids() -> Vec<WireString> {
    let mut roster = OrderedProviderSet::default();
    for descriptor in provider_descriptors() {
        roster.insert(descriptor.provider_id);
    }
    for provider in ["google-antigravity", "google-gemini-cli", "openai-codex"] {
        roster.insert(provider.into());
    }
    for provider in models_dev_catalog_provider_ids() {
        roster.insert(provider);
    }
    roster.iter().cloned().collect()
}

fn json_headers(value: Option<&Value>) -> Result<HeaderSlot, RegistryLoaderError> {
    let Some(value) = value else { return Ok(HeaderSlot::Absent) };
    if value.is_null() {
        return Ok(HeaderSlot::Null);
    }
    let pairs: Vec<(String, &Value)> = match value {
        Value::Object(record) => {
            ara_prompt::js::entries(record).into_iter().map(|(key, value)| (key.clone(), value)).collect()
        }
        Value::Array(values) => values.iter().enumerate().map(|(index, value)| (index.to_string(), value)).collect(),
        _ => return Err(RegistryLoaderError::InvalidHeaders),
    };
    let pairs = pairs
        .into_iter()
        .map(|(key, value)| value.as_str().map(|value| (key, value.into())).ok_or(RegistryLoaderError::InvalidHeaders))
        .collect::<Result<_, _>>()?;
    Ok(HeaderSlot::Source(HeaderSource::Config(HeaderConfigRecord::from_pairs(pairs))))
}
fn config_commands(key: Option<&str>, headers: &HeaderSlot, commands: &mut Vec<String>) {
    let mut add = |value: &str| {
        if is_command_config_value(Some(value)) && !commands.iter().any(|old| old == value) {
            commands.push(value.into());
        }
    };
    if let Some(key) = key {
        add(key);
    }
    if let HeaderSlot::Source(HeaderSource::Config(record)) = headers {
        for (_, value) in record.snapshot() {
            add(&value);
        }
    }
}
fn safe_fields(value: &Value) -> VariantSpec {
    let mut fields = VariantSpec::from_json(value);
    for key in ["apiKey", "headers", "models", "modelOverrides"] {
        fields.remove(key);
    }
    fields
}
fn resolve_headers(slot: &HeaderSlot, host: &RegistryLoaderHost<'_>) -> Result<HeaderSlot, RegistryLoaderError> {
    // Check cancellation before each unstarted helper. No external lock is held.
    let HeaderSlot::Source(HeaderSource::Config(record)) = slot else { return Ok(HeaderSlot::Absent) };
    let mut resolved = Vec::new();
    for (name, config) in record.snapshot() {
        host.check()?;
        if let Some(value) = host
            .config_values
            .resolve_config_value_checked(&config, &host.context(), Default::default(), host.continue_loading)
            .map_err(|_| RegistryLoaderError::Cancelled)?
            .filter(|value| !value.is_empty())
        {
            resolved.push((name, value));
        }
    }
    host.check()?;
    Ok(if resolved.is_empty() {
        HeaderSlot::Absent
    } else {
        HeaderSlot::Source(HeaderSource::Config(HeaderConfigRecord::from_pairs(resolved)))
    })
}

impl RegistryLoader {
    pub fn load(
        file: &mut ModelsConfigFile,
        options: &RegistryLoadOptions,
        host: &RegistryLoaderHost<'_>,
    ) -> Result<Self, RegistryLoaderError> {
        host.check()?;
        let result = if options.ignore_local_model_config { ModelConfigLoad::NotFound } else { file.try_load() };
        let (config, error, found) = match result {
            ModelConfigLoad::Ok(config) => (Some(config), None, true),
            ModelConfigLoad::Error(error) => (None, Some(error), true),
            ModelConfigLoad::NotFound => (None, None, false),
        };
        let mut loaded = LoadedRegistryConfig {
            config,
            found,
            error,
            mtime_ms: None,
            providers: Vec::new(),
            discoverable_providers: Vec::new(),
            keyless_providers: OrderedProviderSet::default(),
            configured_providers: OrderedProviderSet::default(),
        };
        if let Some(providers) =
            loaded.config.as_ref().and_then(|config| config.value().get("providers")).and_then(Value::as_object)
        {
            for (provider, config) in ara_prompt::js::entries(providers) {
                let provider: WireString = provider.as_str().into();
                loaded.configured_providers.insert(provider.clone());
                let headers = json_headers(config.get("headers"))?;
                // Fixed native ordering: headers can create input needed by the eager key command.
                let resolved_headers = resolve_headers(&headers, host)?;
                let key_config = config.get("apiKey").and_then(Value::as_str).map(str::to_owned);
                let mut commands = Vec::new();
                config_commands(key_config.as_deref(), &headers, &mut commands);
                for model in config.get("models").and_then(Value::as_array).into_iter().flatten() {
                    config_commands(None, &json_headers(model.get("headers"))?, &mut commands);
                }
                if let Some(overrides) = config.get("modelOverrides").and_then(Value::as_object) {
                    for (_, model) in ara_prompt::js::entries(overrides) {
                        config_commands(None, &json_headers(model.get("headers"))?, &mut commands);
                    }
                }
                let mut fields = safe_fields(config);
                let disable = fields
                    .get("disableStrictTools")
                    .is_some_and(truthy)
                    .then(|| VariantSpec::from_json(&serde_json::json!({"disableStrictTools":true})));
                let compat = merge_compat(fields.record("compat").as_ref(), disable.as_ref());
                let override_present = resolved_headers.as_source().is_some()
                    || key_config.as_ref().is_some_and(|key| !key.is_empty())
                    || config.get("authHeader").is_some()
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
                if text(&fields, "auth").is_some_and(|auth| auth.equals_ascii("none")) {
                    loaded.keyless_providers.insert(provider.clone());
                }
                if let Some(discovery) = fields.record("discovery")
                    && (text(&fields, "api").is_some_and(|api| !api.is_empty())
                        || text(&discovery, "type").is_some_and(|kind| kind.equals_ascii("proxy")))
                {
                    loaded.discoverable_providers.push(RegistryDiscoveryConfig {
                        provider: provider.clone(),
                        api: text(&fields, "api").unwrap_or_else(|| "openai-completions".into()),
                        base_url: text(&fields, "baseUrl"),
                        headers: resolved_headers.clone(),
                        compat: compat.clone(),
                        remote_compaction: fields.record("remoteCompaction"),
                        discovery,
                        optional: false,
                    });
                }
                if let Some(base) = normalize_override_base(&fields) {
                    fields.set("baseUrl", WireValue::String(base));
                }
                if let Some(compat) = compat {
                    fields.set_record("compat", &compat);
                }
                host.check()?;
                let eager_key = match key_config.as_deref().filter(|key| !key.is_empty()) {
                    Some(key) => host
                        .config_values
                        .resolve_config_value_checked(
                            key,
                            &host.context(),
                            ResolveConfigValueOptions::default(),
                            host.continue_loading,
                        )
                        .map_err(|_| RegistryLoaderError::Cancelled)?,
                    None => None,
                }
                .filter(|key| !key.is_empty());
                if let Some(key) = eager_key.as_deref() {
                    (host.install_config_key)(&provider, Some(key));
                } else if is_command_config_value(key_config.as_deref()) {
                    (host.install_config_key)(&provider, None);
                }
                host.check()?;
                loaded.providers.push(LoadedProviderConfig {
                    provider,
                    key_config,
                    eager_key,
                    headers,
                    resolved_headers,
                    command_values: commands,
                    fields,
                    override_present,
                });
            }
        }
        add_implicit_discovery(&mut loaded, options, host);
        loaded.mtime_ms = file.get_mtime_ms().map_err(|_| RegistryLoaderError::ConfigMtime)?;
        let startup_roster = startup_model_cache_provider_ids();
        let configured: HashSet<_> =
            loaded.discoverable_providers.iter().map(|provider| provider.provider.clone()).collect();
        let pending = startup_roster
            .iter()
            .filter(|provider| !configured.contains(*provider) && !is_credential_scoped_model_cache_provider(provider))
            .cloned()
            .collect();
        Ok(Self {
            config_file: file.clone(),
            loaded,
            startup_roster,
            pending,
            standard: HashMap::new(),
            authoritative: OrderedProviderSet::default(),
            discoverable: Vec::new(),
            states: Vec::new(),
            notices: Vec::new(),
        })
    }
    pub fn loaded_config(&self) -> &LoadedRegistryConfig {
        &self.loaded
    }
    pub fn startup_roster(&self) -> &[WireString] {
        &self.startup_roster
    }
    pub fn pending_standard_providers(&self) -> OrderedProviderSet {
        let mut out = OrderedProviderSet::default();
        for provider in &self.startup_roster {
            if self.pending.contains(provider) {
                out.insert(provider.clone());
            }
        }
        out
    }
    pub fn cached_standard_models_snapshot(&self) -> Vec<HostModelRef> {
        self.startup_roster.iter().filter_map(|provider| self.standard.get(provider)).flatten().cloned().collect()
    }
    pub fn authoritative_provider_snapshot(&self) -> OrderedProviderSet {
        self.authoritative.clone()
    }
    pub fn cached_discoverable_models_snapshot(&self) -> Vec<HostModelRef> {
        self.discoverable.clone()
    }
    pub fn discovery_states(&self) -> &[ProviderDiscoveryState] {
        &self.states
    }
    pub fn take_notices(&mut self) -> Vec<RegistryLoaderNotice> {
        std::mem::take(&mut self.notices)
    }
    fn state(&mut self, state: ProviderDiscoveryState) {
        if let Some(old) = self.states.iter_mut().find(|old| old.provider == state.provider) {
            *old = state;
        } else {
            self.states.push(state);
        }
    }
    fn read_cache(&mut self, key: &WireString, host: &RegistryLoaderHost<'_>) -> Option<WireCacheEntry> {
        match host.cache.read(key, STARTUP_CACHE_TTL_MS, host.clock) {
            Ok(cache) => cache,
            Err(stage) => {
                self.notices.push(RegistryLoaderNotice { provider: key.clone(), stage });
                None
            }
        }
    }
    pub fn load_standard_provider(
        &mut self,
        provider: &WireString,
        base_url: Option<&WireString>,
        bundled: &[HostModelRef],
        host: &RegistryLoaderHost<'_>,
        prepare: impl FnOnce(&WireString, Vec<HostModelRef>) -> Result<Vec<HostModelRef>, CollapseError>,
    ) -> Result<StandardProviderSlice, RegistryLoaderError> {
        host.check()?;
        if !self.pending.remove(provider) {
            return Ok(StandardProviderSlice {
                provider: provider.clone(),
                models: self.standard.get(provider).cloned().unwrap_or_default(),
                authoritative: self.authoritative.contains(provider),
                discovery_state: self.states.iter().find(|state| state.provider == *provider).cloned(),
                newly_loaded: false,
            });
        }
        let slice = self.load_standard_drained(provider, base_url, bundled, host, prepare)?;
        self.accept_standard_slice(&slice);
        Ok(slice)
    }

    /// A full/selective query drains its entire requested pending set before
    /// reading rows, as OMP does. A failing row is not silently replayed and
    /// later rows from this failed batch are not read on the next query.
    pub fn load_standard_providers(
        &mut self,
        filter: Option<&OrderedProviderSet>,
        host: &RegistryLoaderHost<'_>,
        inputs: impl Fn(&WireString) -> Result<(Option<WireString>, Vec<HostModelRef>), CollapseError>,
        prepare: impl Fn(&WireString, Vec<HostModelRef>) -> Result<Vec<HostModelRef>, CollapseError>,
    ) -> Result<Vec<StandardProviderSlice>, RegistryLoaderError> {
        host.check()?;
        let providers: Vec<_> = self
            .startup_roster
            .iter()
            .filter(|provider| {
                self.pending.contains(*provider) && filter.is_none_or(|filter| filter.contains(provider))
            })
            .cloned()
            .collect();
        for provider in &providers {
            self.pending.remove(provider);
        }
        let mut slices = Vec::new();
        for provider in providers {
            host.check()?;
            let (base, bundled) = inputs(&provider).map_err(|_| RegistryLoaderError::ModelPreparation)?;
            slices.push(self.load_standard_drained(&provider, base.as_ref(), &bundled, host, &prepare)?);
        }
        // Native publishes the returned map only after the complete batch has
        // settled. Per-provider discovery notices/states may already exist,
        // but a later preparation failure must not expose partial cache slices.
        for slice in &slices {
            self.accept_standard_slice(slice);
        }
        Ok(slices)
    }

    fn accept_standard_slice(&mut self, slice: &StandardProviderSlice) {
        self.standard.insert(slice.provider.clone(), slice.models.clone());
        if slice.authoritative {
            self.authoritative.insert(slice.provider.clone());
        }
    }

    fn load_standard_drained(
        &mut self,
        provider: &WireString,
        base_url: Option<&WireString>,
        bundled: &[HostModelRef],
        host: &RegistryLoaderHost<'_>,
        prepare: impl FnOnce(&WireString, Vec<HostModelRef>) -> Result<Vec<HostModelRef>, CollapseError>,
    ) -> Result<StandardProviderSlice, RegistryLoaderError> {
        let configured_base = self.loaded.provider(provider).and_then(|config| text(config.fields(), "baseUrl"));
        let key = resolve_model_cache_provider_id(
            provider,
            None,
            base_url.or(configured_base.as_ref()),
            host.provider_environment,
        );
        let shared = models_dev_catalog_provider_ids().contains(provider);
        let additive = shared && !authoritative_runtime_catalog_providers().contains(provider);
        let Some(cache) = self.read_cache(&key, host) else {
            let state = shared.then(|| ProviderDiscoveryState {
                provider: provider.clone(),
                status: ProviderDiscoveryStatus::Idle,
                optional: false,
                stale: false,
                fetched_at: None,
                source: Some(ModelResolutionSource::Bundled),
                models: Vec::new(),
            });
            if let Some(state) = &state {
                self.state(state.clone());
            }
            return Ok(StandardProviderSlice {
                provider: provider.clone(),
                models: Vec::new(),
                authoritative: false,
                discovery_state: state,
                newly_loaded: true,
            });
        };
        let bundled_specs: Vec<_> = bundled.iter().map(storage_spec).collect::<Result<_, _>>()?;
        let fingerprint = fingerprint_static_models(&ModelArray::new(bundled_specs), shared && !additive);
        let matches = fingerprint_matches(&cache.static_fingerprint, &fingerprint);
        let bundled_by_id: HashMap<_, _> =
            bundled.iter().filter_map(|model| text(model.spec(), "id").map(|id| (id, model))).collect();
        let mut models = Vec::new();
        for mut spec in cached_rows(&cache)? {
            host.check()?;
            let id = text(&spec, "id").ok_or(RegistryLoaderError::InvalidCacheModels)?;
            spec.set("provider", WireValue::String(provider.clone()));
            if additive && !matches && bundled_by_id.contains_key(&id) {
                continue;
            }
            let original_headers = take_spec_headers(&mut spec)?;
            let headers = if cache.header_omitted_model_ids.contains(&id) {
                let request = text(&spec, "requestModelId").filter(|id| !id.is_empty());
                let source_id = cached_header_restore_source_id(&cache, &id, request.as_ref(), &|id| {
                    bundled_by_id.contains_key(id)
                });
                let source = source_id.as_ref().and_then(|id| bundled_by_id.get(id));
                let Some(source) = source.filter(|source| source.headers().as_source().is_some()) else { continue };
                source.headers().clone()
            } else {
                original_headers
            };
            models.push(build_host_model(&spec, headers).map_err(|_| RegistryLoaderError::ModelPreparation)?);
        }
        host.check()?;
        let models = prepare(provider, models).map_err(|_| RegistryLoaderError::ModelPreparation)?;
        let authoritative = cache.fresh
            && cache.authoritative
            && (providers_with_authoritative_project_catalog(&models).contains(provider)
                || authoritative_runtime_catalog_providers().contains(provider));
        let state = if shared {
            let raw_specs: Vec<_> = cached_rows(&cache)?.into_iter().map(Arc::new).collect();
            let snapshot_matches = fingerprint_static_models(&ModelArray::new(raw_specs), !additive) == fingerprint;
            let contributed = if additive {
                models.iter().any(|model| text(model.spec(), "id").is_some_and(|id| !bundled_by_id.contains_key(&id)))
            } else {
                !(matches && snapshot_matches)
            };
            let stale = !cache.fresh || !cache.authoritative;
            Some(ProviderDiscoveryState {
                provider: provider.clone(),
                status: if contributed {
                    ProviderDiscoveryStatus::Cached
                } else if stale {
                    ProviderDiscoveryStatus::Unavailable
                } else {
                    ProviderDiscoveryStatus::Idle
                },
                optional: false,
                stale,
                fetched_at: contributed.then_some(cache.updated_at),
                source: Some(if contributed { ModelResolutionSource::Cache } else { ModelResolutionSource::Bundled }),
                models: models.iter().filter_map(|model| text(model.spec(), "id")).collect(),
            })
        } else {
            None
        };
        if let Some(state) = &state {
            self.state(state.clone());
        }
        Ok(StandardProviderSlice {
            provider: provider.clone(),
            models,
            authoritative,
            discovery_state: state,
            newly_loaded: true,
        })
    }

    pub fn load_discoverable_cache(
        &mut self,
        host: &RegistryLoaderHost<'_>,
        prepare: impl Fn(&WireString, Vec<HostModelRef>) -> Result<Vec<HostModelRef>, CollapseError>,
    ) -> Result<Vec<HostModelRef>, RegistryLoaderError> {
        let mut all = Vec::new();
        for index in 0..self.loaded.discoverable_providers.len() {
            host.check()?;
            let key = self.loaded.discoverable_providers[index].cache_provider_id();
            let provider = self.loaded.discoverable_providers[index].provider.clone();
            let optional = self.loaded.discoverable_providers[index].optional;
            let Some(cache) = self.read_cache(&key, host) else {
                self.state(ProviderDiscoveryState {
                    provider,
                    status: ProviderDiscoveryStatus::Idle,
                    optional,
                    stale: false,
                    fetched_at: None,
                    source: None,
                    models: Vec::new(),
                });
                continue;
            };
            let fallback = self.configured_header_fallback(&provider, host)?;
            let has_fallback = fallback.as_source().is_some();
            let has_unrestored = !cache.header_omitted_model_ids.is_empty() && !has_fallback;
            let mut restored = Vec::new();
            for mut spec in cached_rows(&cache)? {
                let id = text(&spec, "id").ok_or(RegistryLoaderError::InvalidCacheModels)?;
                let original_headers = take_spec_headers(&mut spec)?;
                let headers = if cache.header_omitted_model_ids.contains(&id) {
                    if !has_fallback {
                        continue;
                    }
                    fallback.clone()
                } else {
                    original_headers
                };
                restored.push(build_host_model(&spec, headers).map_err(|_| RegistryLoaderError::ModelPreparation)?);
            }
            if has_fallback
                && !cache.unrestorable_header_model_ids.is_empty()
                && let Err(stage) = host.cache.repair(&key, &cache, &restored, &fallback)
            {
                self.notices.push(RegistryLoaderNotice { provider: provider.clone(), stage });
            }
            let discovery = &self.loaded.discoverable_providers[index];
            let models = normalize_discoverable(discovery, restored, host.provider_environment)?;
            host.check()?;
            let models = prepare(&provider, models).map_err(|_| RegistryLoaderError::ModelPreparation)?;
            let config_mtime = self.config_file.get_mtime_ms().map_err(|_| RegistryLoaderError::ConfigMtime)?;
            let config_stale = config_mtime.is_some_and(|mtime| cache.updated_at < mtime.floor());
            let llama = discovery.kind().is_some_and(|kind| kind.equals_ascii("llama.cpp"));
            self.state(ProviderDiscoveryState {
                provider,
                status: ProviderDiscoveryStatus::Cached,
                optional,
                stale: llama || !cache.fresh || !cache.authoritative || config_stale || has_unrestored,
                fetched_at: Some(cache.updated_at),
                source: None,
                models: models.iter().filter_map(|model| text(model.spec(), "id")).collect(),
            });
            all.extend(models);
        }
        self.loaded.mtime_ms = self.config_file.get_mtime_ms().map_err(|_| RegistryLoaderError::ConfigMtime)?;
        self.discoverable = all.clone();
        Ok(all)
    }
    pub(crate) fn configured_header_fallback(
        &self,
        provider: &WireString,
        host: &RegistryLoaderHost<'_>,
    ) -> Result<HeaderSlot, RegistryLoaderError> {
        let Some(config) = self.loaded.provider(provider).filter(|config| {
            config.override_present
                && boolean(config.fields(), "authHeader") == Some(true)
                && config.key_config().is_some_and(|key| !key.is_empty())
        }) else {
            return Ok(HeaderSlot::Absent);
        };
        host.check()?;
        let slot = merge_auth_header_sources(&[config.headers().clone()], Some(true), config.key_config());
        let resolved = match slot.as_source() {
            Some(HeaderSource::Live(headers)) => headers
                .snapshot_checked(host.config_values, &host.context(), &|| (host.continue_loading)())
                .map_err(|_| RegistryLoaderError::Cancelled)?,
            source => resolve_config_headers(source, host.config_values, &host.context()),
        };
        Ok(match resolved.filter(|headers| headers.get("Authorization").is_some_and(|value| !value.is_empty())) {
            Some(headers) => {
                HeaderSlot::Source(HeaderSource::Config(HeaderConfigRecord::from_pairs(headers.into_pairs())))
            }
            None => HeaderSlot::Absent,
        })
    }
}

fn fingerprint_matches(actual: &WireString, expected: &WireString) -> bool {
    actual == expected || {
        let mut prefix = expected.clone();
        prefix.append_str(":drop:");
        actual.units().starts_with(prefix.units())
    }
}
pub(crate) fn cached_rows(cache: &WireCacheEntry) -> Result<Vec<VariantSpec>, RegistryLoaderError> {
    cache
        .models
        .value
        .as_array()
        .ok_or(RegistryLoaderError::InvalidCacheModels)
        .map(|rows| rows.iter().cloned().map(VariantSpec::from_wire).collect())
}
pub(crate) fn take_spec_headers(spec: &mut VariantSpec) -> Result<HeaderSlot, RegistryLoaderError> {
    if spec.own_keys().iter().any(|key| key.equals_ascii("apiKey")) {
        return Err(RegistryLoaderError::InvalidCacheModels);
    }
    let headers = match spec.record("headers") {
        None if spec.own_keys().iter().any(|key| key.equals_ascii("headers")) => HeaderSlot::Undefined,
        None => HeaderSlot::Absent,
        Some(headers) if matches!(headers.value, WireValue::Null) => HeaderSlot::Null,
        Some(headers) => {
            let mut pairs = Vec::new();
            for key in headers.own_keys() {
                let name = key.to_utf8().map_err(|_| RegistryLoaderError::InvalidHeaders)?;
                let value = headers
                    .get_path(std::slice::from_ref(&key))
                    .and_then(WireValue::as_string)
                    .and_then(|value| value.to_utf8().ok())
                    .ok_or(RegistryLoaderError::InvalidHeaders)?;
                pairs.push((name, value));
            }
            HeaderSlot::Source(HeaderSource::Config(HeaderConfigRecord::from_pairs(pairs)))
        }
    };
    spec.remove("headers");
    Ok(headers)
}

pub fn normalize_discoverable(
    config: &RegistryDiscoveryConfig,
    models: Vec<HostModelRef>,
    env: &dyn ProviderEnvironment,
) -> Result<Vec<HostModelRef>, RegistryLoaderError> {
    let decoder = config
        .kind()
        .is_some_and(|kind| ["ollama", "llama.cpp", "lm-studio"].iter().any(|expected| kind.equals_ascii(expected)));
    let ollama = config.provider.equals_ascii("ollama") && config.api.equals_ascii("openai-responses");
    let context = ollama.then(|| env.get("OLLAMA_CONTEXT_LENGTH")).flatten().and_then(|raw| {
        let raw = trim(&raw);
        if raw.is_empty() {
            return None;
        }
        raw.to_utf8().ok().map(|raw| ara_prompt::js::to_number(&Value::String(raw))).filter(|number| {
            number.is_finite() && *number > 0.0 && number.fract() == 0.0 && *number <= 9_007_199_254_740_991.0
        })
    });
    models
        .into_iter()
        .map(|model| {
            let mut spec = to_host_model_spec(&model);
            let compat = merge_compat(model.spec().record("compatConfig").as_ref(), config.compat.as_ref());
            if let Some(compat) = compat {
                spec.set_record("compat", &compat);
            }
            if decoder {
                spec.set("imageInputDecoder", WireValue::String("stb".into()));
            }
            if let Some(remote) = &config.remote_compaction {
                let remote = merge_provider_remote_compaction_config(
                    model.spec().record("remoteCompaction").as_ref(),
                    Some(remote),
                );
                if let Some(remote) = remote {
                    spec.set_record("remoteCompaction", &remote);
                }
            }
            if ollama {
                if text(&spec, "api").is_some_and(|api| api.equals_ascii("openai-completions")) {
                    spec.set("api", WireValue::String("openai-responses".into()));
                }
                if let Some(context) = context {
                    spec.set("contextWindow", WireValue::Number(context));
                    spec.set("maxTokens", WireValue::Number(context.min(32768.0)));
                }
            }
            build_host_model(&spec, model.headers().clone()).map_err(|_| RegistryLoaderError::ModelPreparation)
        })
        .collect()
}

fn normalize_override_base(fields: &VariantSpec) -> Option<WireString> {
    let discovery = fields.record("discovery")?;
    let bare = text(&discovery, "type").is_some_and(|kind| kind.equals_ascii("openai-models-list"))
        && boolean(&discovery, "injectV1") == Some(false);
    let litellm = text(&discovery, "type").is_some_and(|kind| kind.equals_ascii("litellm"));
    if !bare && !litellm {
        return None;
    }
    let base = text(fields, "baseUrl").and_then(|base| base.to_utf8().ok());
    let raw = if bare {
        base.filter(|base| !base.is_empty()).unwrap_or_else(|| "http://127.0.0.1:1234".into())
    } else {
        base.unwrap_or_else(|| "http://localhost:4000/v1".into())
    };
    let raw = if raw.is_empty() { "http://127.0.0.1:1234/v1".into() } else { raw };
    Some(if let Ok(mut url) = reqwest::Url::parse(&raw) {
        let mut path = url.path().trim_end_matches('/').to_owned();
        if !bare && !path.ends_with("/v1") {
            path.push_str("/v1");
        }
        if !bare {
            url.set_path(&path);
            path = url.path().into();
        }
        format!("{}{}", url_root(&url), path).into()
    } else if bare {
        raw.trim_end_matches('/').into()
    } else {
        raw.into()
    })
}

fn add_implicit_discovery(
    loaded: &mut LoadedRegistryConfig,
    options: &RegistryLoadOptions,
    host: &RegistryLoaderHost<'_>,
) {
    for (provider, api, env_name, default) in [
        ("ollama", "openai-responses", "OLLAMA_BASE_URL", "http://127.0.0.1:11434"),
        ("llama.cpp", "openai-responses", "LLAMA_CPP_BASE_URL", "http://127.0.0.1:8080"),
        ("lm-studio", "openai-completions", "LM_STUDIO_BASE_URL", "http://127.0.0.1:1234/v1"),
    ] {
        let provider: WireString = provider.into();
        if loaded.configured_providers.contains(&provider) || options.disabled_providers.contains(&provider) {
            continue;
        }
        let raw = host.provider_environment.get(env_name);
        let (base_url, optional) = if provider.equals_ascii("ollama") {
            let direct = raw.as_ref().map(trim).filter(|base| !base.is_empty());
            let ollama_host = host.provider_environment.get("OLLAMA_HOST");
            let has_override = direct.is_some() || ollama_host.as_ref().map(trim).is_some_and(|host| !host.is_empty());
            let base = direct.or_else(|| ollama_host.and_then(normalize_ollama_host)).unwrap_or_else(|| default.into());
            (base, !has_override)
        } else {
            (
                raw.filter(|base| !base.is_empty()).unwrap_or_else(|| default.into()),
                !host.provider_environment.get(env_name).is_some_and(|base| !base.is_empty()),
            )
        };
        if !provider.equals_ascii("llama.cpp") || !(host.has_auth)(&provider) {
            loaded.keyless_providers.insert(provider.clone());
        }
        loaded.discoverable_providers.push(RegistryDiscoveryConfig {
            provider: provider.clone(),
            api: api.into(),
            base_url: Some(base_url),
            headers: HeaderSlot::Absent,
            compat: None,
            remote_compaction: None,
            discovery: VariantSpec::from_json(
                &serde_json::json!({"type":provider.to_utf8().expect("literal provider")}),
            ),
            optional,
        });
    }
}
fn normalize_ollama_host(value: WireString) -> Option<WireString> {
    let value = trim(&value).to_utf8().ok()?;
    if value.is_empty() {
        return None;
    }
    let candidate = if value.contains("://") {
        value
    } else if value.starts_with("//") {
        format!("http:{value}")
    } else if value.starts_with(':') {
        format!("http://127.0.0.1{value}")
    } else {
        format!("http://{value}")
    };
    let mut url = reqwest::Url::parse(&candidate).ok()?;
    if url.host_str().is_none() || !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if url.port().is_none() && url.scheme() == "http" {
        url.set_port(Some(11434)).ok()?;
    }
    Some(url_root(&url).into())
}

fn url_root(url: &reqwest::Url) -> String {
    let host = url.host_str().unwrap_or("");
    let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.into() };
    let port = url.port().map_or_else(String::new, |port| format!(":{port}"));
    format!("{}://{host}{port}", url.scheme())
}

pub(crate) fn storage_spec(model: &HostModelRef) -> Result<SpecRef, RegistryLoaderError> {
    let mut spec = model.spec().as_ref().clone();
    match model.headers() {
        HeaderSlot::Null => spec.set("headers", WireValue::Null),
        HeaderSlot::Undefined => spec.set_undefined("headers"),
        _ => {
            if let Some(headers) = literal_headers(model.headers())? {
                spec.set_record("headers", &headers);
            }
        }
    }
    Ok(Arc::new(spec))
}
