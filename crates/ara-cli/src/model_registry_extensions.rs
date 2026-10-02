//! Host-owned runtime extension state from fixed OMP model-registry.ts.
//!
//! Source: 596f2da7101178214aa27a753529d15e6b7ad91d, 811–844,
//! 2022–2064 and 2487–2821; ai/api-registry.ts and registry/oauth/index.ts.
//! Registration is ordered, with native early returns and partial side effects.
//! Custom API/OAuth entries overwrite by name; source cleanup removes only
//! the current entry's source, without inventing a historical restoration stack.
//!
//! MIT License; Copyright (c) 2025 Mario Zechner;
//! Copyright (c) 2025-2026 Can Bölük; Copyright (c) 2026 Stencil Labs, Inc.
//! See LICENSE for the full license.

use crate::{
    custom_models::{CustomModelOverlay, ModelIdPredicate, build_custom_model_overlay, normalize_suppressed_selector},
    model_collapse::{CollapseError, CollapseRuntime, VariantSpec, truthy},
    model_config_values::{HeaderConfigRecord, HeaderSource, ResolvedConfigHeaders},
    model_identity_wire::{boolean, empty, nullish, text},
    model_manager::{ModelManagerOptions, RawModelValue},
    model_patch::{
        HeaderSlot, HostModel, HostModelRef, ModelPatch, OrderedProviderSet, ProviderOverride, merge_compat,
        merge_header_sources, merge_remote_compaction_config,
    },
    model_registry_runtime::RegistryRuntimeResult,
};
use ara_rpc::WireString;
use async_trait::async_trait;
use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
};

pub type ExtensionResult<T> = Result<T, CollapseError>;
/// Actual Host callback/credential object. This sidecar is never Debug/Serialize.
pub type OpaqueExtensionHandle = Arc<dyn Any + Send + Sync>;

/// Original extension callback, before the Host's credential/timeout/model
/// finalization adapter. The key is a resolved, authenticated value or absent.
#[async_trait]
pub trait RuntimeDefinitionFetcher: Send + Sync {
    async fn fetch(&self, api_key: Option<WireString>) -> RegistryRuntimeResult<RawModelValue>;
}

#[derive(Clone, Default)]
pub enum ModifierHeaders {
    #[default]
    Absent,
    Undefined,
    Null,
    Values(Vec<(String, String)>),
}
#[derive(Clone)]
pub struct ModifierModel {
    pub fields: VariantSpec,
    pub headers: ModifierHeaders,
}
pub trait RuntimeModelModifier: Send + Sync {
    fn modify(
        &self,
        models: Vec<ModifierModel>,
        credential: &OpaqueExtensionHandle,
    ) -> ExtensionResult<Vec<ModifierModel>>;
}
impl<F> RuntimeModelModifier for F
where
    F: Fn(Vec<ModifierModel>, &OpaqueExtensionHandle) -> ExtensionResult<Vec<ModifierModel>> + Send + Sync,
{
    fn modify(
        &self,
        models: Vec<ModifierModel>,
        credential: &OpaqueExtensionHandle,
    ) -> ExtensionResult<Vec<ModifierModel>> {
        self(models, credential)
    }
}

#[derive(Clone)]
pub struct RuntimeOAuthRegistration {
    pub handle: OpaqueExtensionHandle,
    pub modifier: Option<Arc<dyn RuntimeModelModifier>>,
}
#[derive(Clone)]
pub struct RuntimeApiRegistration {
    pub api: WireString,
    /// Host object contains the actual stream/streamSimple functions.
    pub handle: OpaqueExtensionHandle,
    pub source_id: Option<WireString>,
}
#[derive(Clone)]
pub struct RuntimeOAuthEntry {
    pub provider: WireString,
    pub registration: RuntimeOAuthRegistration,
    pub source_id: Option<WireString>,
}

/// Runtime config facts exclude apiKey/headers; private values stay in sidecars.
/// A dynamic fetcher is the original extension definition callback. The Host
/// wraps it with credential peek, the 15-second discovery deadline, custom
/// definition finalization, and toModelSpec before a manager invokes it.
#[derive(Clone)]
pub struct RuntimeProviderConfig {
    fields: VariantSpec,
    headers: HeaderSlot,
    api_key_config: Option<String>,
    pub models: Option<Vec<ModelPatch>>,
    pub custom_api: Option<OpaqueExtensionHandle>,
    pub oauth: Option<RuntimeOAuthRegistration>,
    pub usage: Option<OpaqueExtensionHandle>,
    pub dynamic_fetcher: Option<Arc<dyn RuntimeDefinitionFetcher>>,
}
impl RuntimeProviderConfig {
    pub fn new(fields: VariantSpec, headers: HeaderSlot, api_key_config: Option<String>) -> ExtensionResult<Self> {
        // Reuse the existing safe metadata boundary rather than allowing raw
        // credentials into composition records or derived metadata.
        let safe = ModelPatch::new(fields, headers)?;
        Ok(Self {
            fields: safe.fields().clone(),
            headers: safe.headers().clone(),
            api_key_config,
            models: None,
            custom_api: None,
            oauth: None,
            usage: None,
            dynamic_fetcher: None,
        })
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

#[derive(Clone)]
pub struct RuntimeManagerRegistration {
    pub provider: WireString,
    /// The Host installs the wrapped fetcher in a clone of these options. The
    /// store never exposes an invocable, unwrapped manager fetcher.
    pub options: ModelManagerOptions,
    pub source_id: WireString,
    pub config: RuntimeProviderConfig,
}
/// Owned, secret-bearing composition snapshot; deliberately no Debug/Serialize.
#[derive(Clone, Default)]
pub struct ExtensionSnapshot {
    pub api_key_configs: Vec<(WireString, String)>,
    pub provider_overrides: Vec<(WireString, ProviderOverride)>,
    pub model_overlays: Vec<CustomModelOverlay>,
    pub managers: Vec<RuntimeManagerRegistration>,
    pub modifiers: Vec<(WireString, Arc<dyn RuntimeModelModifier>)>,
    pub custom_apis: Vec<RuntimeApiRegistration>,
    pub custom_oauth: Vec<RuntimeOAuthEntry>,
    pub provider_sources: Vec<(WireString, WireString)>,
    pub source_providers: Vec<(WireString, OrderedProviderSet)>,
    pub registered_sources: Vec<WireString>,
}

/// Callbacks own external registration/auth and catalog composition. Every
/// callback runs outside the store lock and may inspect/reenter this store.
/// Errors preserve all already completed registration stages.
pub trait ExtensionRegistryHost: Send + Sync {
    fn custom_api_registered(&self, entry: &RuntimeApiRegistration);
    fn custom_api_removed(&self, api: &WireString);
    fn oauth_registered(&self, entry: &RuntimeOAuthEntry);
    fn oauth_removed(&self, provider: &WireString);
    fn set_runtime_usage(&self, provider: &WireString, usage: &OpaqueExtensionHandle, raw_key: Option<&str>);
    fn remove_runtime_usage(&self, provider: &WireString);
    fn install_api_key(&self, provider: &WireString, raw_key: &str) -> ExtensionResult<()>;
    fn remove_config_api_key(&self, provider: &WireString);
    fn has_full_snapshot(&self) -> bool;
    fn ensure_full_snapshot(&self) -> ExtensionResult<()>;
    fn reload_static(&self, force: bool) -> ExtensionResult<()>;
    /// Replace this provider in the unprojected catalog, finalize overlays,
    /// apply retained transport/config Bedrock fields, then run all modifiers.
    fn models_registered(
        &self,
        provider: &WireString,
        overlays: &[CustomModelOverlay],
        retained_transport: Option<&ProviderOverride>,
    ) -> ExtensionResult<()>;
    /// Apply the incoming transport patch to unprojected models and reproject.
    fn transport_applied(&self, provider: &WireString, incoming: &ProviderOverride) -> ExtensionResult<()>;
    fn invalidate_provider(&self, provider: &WireString);
    fn invalidate_all_provider_lookups(&self);
}

pub trait ExtensionModifierHost: Send + Sync {
    fn oauth_credential(&self, provider: &WireString) -> Option<OpaqueExtensionHandle>;
    /// Return a private snapshot for Source-bearing Host headers. Config is an
    /// already literal model record: copy its strings without environment or
    /// command interpretation. Only Live invokes its configuration resolver.
    /// Never return the original mutable Config record or Live proxy.
    fn materialize_headers(&self, model: &HostModelRef) -> ExtensionResult<Option<ResolvedConfigHeaders>>;
    fn warn_modifier_failure(&self, provider: &WireString, error: &CollapseError);
}

#[derive(Default)]
struct ExtensionState {
    snapshot: ExtensionSnapshot,
    last_modifier_warnings: Vec<(WireString, WireString)>,
    suppressed: Vec<(WireString, f64)>,
}
#[derive(Default)]
pub struct ExtensionRegistry {
    state: Mutex<ExtensionState>,
}

fn set_entry<V>(entries: &mut Vec<(WireString, V)>, key: &WireString, value: V) {
    if let Some((_, current)) = entries.iter_mut().find(|(candidate, _)| candidate == key) {
        *current = value;
    } else {
        entries.push((key.clone(), value));
    }
}
fn remove_entry<V>(entries: &mut Vec<(WireString, V)>, key: &WireString) {
    entries.retain(|(candidate, _)| candidate != key);
}
fn wire_error(parts: &[WireString]) -> CollapseError {
    CollapseError::from_wire(WireString::from_units(
        parts.iter().flat_map(|part| part.units().iter().copied()).collect(),
    ))
}

// Runtime-register's business checks, over the lossless typed Host input. The
// file-schema validator is not applied to extension configs upstream.
fn validate(provider: &WireString, config: &RuntimeProviderConfig) -> ExtensionResult<()> {
    let has_api = config.fields.get("api").is_some_and(truthy);
    if config.custom_api.is_some() && !has_api {
        return Err(wire_error(&[
            "Provider ".into(),
            provider.clone(),
            ": \"api\" is required when registering streamSimple.".into(),
        ]));
    }
    let models = config.models.as_deref().unwrap_or_default();
    if !models.is_empty() {
        if !config.fields.get("baseUrl").is_some_and(truthy) {
            return Err(wire_error(&[
                "Provider ".into(),
                provider.clone(),
                ": \"baseUrl\" is required when defining custom models.".into(),
            ]));
        }
        if !config.api_key_config.as_ref().is_some_and(|key| !key.is_empty()) && config.oauth.is_none() {
            return Err(wire_error(&[
                "Provider ".into(),
                provider.clone(),
                ": \"apiKey\" or \"oauth\" is required when defining models.".into(),
            ]));
        }
    }
    for model in models {
        if !has_api && !model.fields().get("api").is_some_and(truthy) {
            return Err(wire_error(&[
                "Provider ".into(),
                provider.clone(),
                ", model ".into(),
                text(model.fields(), "id").unwrap_or_else(|| "undefined".into()),
                ": no \"api\" specified.".into(),
            ]));
        }
        if !model.fields().get("id").is_some_and(truthy) {
            return Err(wire_error(&["Provider ".into(), provider.clone(), ": model missing \"id\"".into()]));
        }
    }
    Ok(())
}

const BUILTIN_APIS: &[&str] = &[
    "openai-completions",
    "openai-responses",
    "openrouter",
    "openai-codex-responses",
    "azure-openai-responses",
    "anthropic-messages",
    "bedrock-converse-stream",
    "google-generative-ai",
    "google-gemini-cli",
    "google-vertex",
    "ollama-chat",
    "cursor-agent",
    "gitlab-duo-agent",
    "devin-agent",
];

fn merge_override(base: Option<&ProviderOverride>, incoming: &ProviderOverride) -> ExtensionResult<ProviderOverride> {
    let mut fields = empty();
    for key in ["baseUrl", "authHeader", "transport"] {
        let value = incoming
            .fields()
            .record(key)
            .filter(|value| !nullish(Some(&value.value)))
            .or_else(|| base.and_then(|base| base.fields().record(key)));
        if let Some(value) = value {
            fields.set_record(key, &value);
        } else {
            fields.set_undefined(key);
        }
    }
    let headers = if incoming.headers().as_source().is_some() {
        merge_header_sources(
            &[base.map(|base| base.headers().clone()).unwrap_or_default(), incoming.headers().clone()],
            Default::default(),
        )
    } else {
        base.map(|base| base.headers().clone()).unwrap_or(HeaderSlot::Undefined)
    };
    let base_compat = base.and_then(|base| base.fields().record("compat"));
    let incoming_compat = incoming.fields().record("compat").filter(|value| truthy(&value.value));
    let compat = if incoming_compat.is_some() {
        merge_compat(base_compat.as_ref(), incoming_compat.as_ref())
    } else {
        base_compat
    };
    if let Some(compat) = compat {
        fields.set_record("compat", &compat);
    } else {
        fields.set_undefined("compat");
    }
    let base_remote = base.and_then(|base| base.fields().record("remoteCompaction"));
    let incoming_remote = incoming.fields().record("remoteCompaction");
    if let Some(remote) = merge_remote_compaction_config(base_remote.as_ref(), incoming_remote.as_ref()) {
        fields.set_record("remoteCompaction", &remote);
    } else {
        fields.set_undefined("remoteCompaction");
    }
    ProviderOverride::new(
        fields,
        headers,
        incoming.api_key_config().or_else(|| base.and_then(ProviderOverride::api_key_config)).map(str::to_owned),
    )
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn snapshot(&self) -> ExtensionSnapshot {
        self.state.lock().expect("extension store poisoned").snapshot.clone()
    }
    pub fn runtime_provider_ids(&self) -> OrderedProviderSet {
        let mut providers = OrderedProviderSet::default();
        for manager in self.snapshot().managers {
            providers.insert(manager.provider);
        }
        providers
    }
    pub fn runtime_manager_options(&self) -> Vec<RuntimeManagerRegistration> {
        self.snapshot().managers
    }

    fn clear_provider_state(&self, provider: &WireString, host: &dyn ExtensionRegistryHost) {
        {
            let mut state = self.state.lock().expect("extension store poisoned");
            let snapshot = &mut state.snapshot;
            remove_entry(&mut snapshot.api_key_configs, provider);
            remove_entry(&mut snapshot.provider_overrides, provider);
            snapshot.model_overlays.retain(|overlay| text(overlay.fields(), "provider").as_ref() != Some(provider));
            snapshot.managers.retain(|manager| &manager.provider != provider);
            remove_entry(&mut snapshot.modifiers, provider);
            remove_entry(&mut state.last_modifier_warnings, provider);
        }
        host.remove_config_api_key(provider);
        host.remove_runtime_usage(provider);
    }

    fn detach_source(&self, provider: &WireString, remove_owner: bool) {
        let mut state = self.state.lock().expect("extension store poisoned");
        let snapshot = &mut state.snapshot;
        if let Some((_, source)) = snapshot.provider_sources.iter().find(|(name, _)| name == provider).cloned() {
            if let Some((_, providers)) = snapshot.source_providers.iter_mut().find(|(name, _)| name == &source) {
                let mut remaining = OrderedProviderSet::default();
                for candidate in providers.iter().filter(|candidate| *candidate != provider) {
                    remaining.insert(candidate.clone());
                }
                *providers = remaining;
            }
            snapshot.source_providers.retain(|(_, providers)| !providers.is_empty());
            if remove_owner {
                remove_entry(&mut snapshot.provider_sources, provider);
            }
        }
    }

    pub fn register_provider(
        &self,
        provider: WireString,
        config: &RuntimeProviderConfig,
        source_id: Option<WireString>,
        host: &dyn ExtensionRegistryHost,
    ) -> ExtensionResult<()> {
        validate(&provider, config)?;
        if let Some(handle) = &config.custom_api {
            let api = text(config.fields(), "api")
                .ok_or_else(|| CollapseError::new("Runtime API must be a string".into()))?;
            if BUILTIN_APIS.iter().any(|builtin| api.equals_ascii(builtin)) {
                return Err(wire_error(&[
                    "Cannot register custom API \"".into(),
                    api,
                    "\": built-in API names are reserved.".into(),
                ]));
            }
            let entry =
                RuntimeApiRegistration { api: api.clone(), handle: handle.clone(), source_id: source_id.clone() };
            {
                let mut state = self.state.lock().expect("extension store poisoned");
                if let Some(current) = state.snapshot.custom_apis.iter_mut().find(|current| current.api == api) {
                    *current = entry.clone();
                } else {
                    state.snapshot.custom_apis.push(entry.clone());
                }
            }
            host.custom_api_registered(&entry);
        }
        if let Some(registration) = &config.oauth {
            let entry = RuntimeOAuthEntry {
                provider: provider.clone(),
                registration: registration.clone(),
                source_id: source_id.clone(),
            };
            {
                let mut state = self.state.lock().expect("extension store poisoned");
                if let Some(current) =
                    state.snapshot.custom_oauth.iter_mut().find(|current| current.provider == provider)
                {
                    *current = entry.clone();
                } else {
                    state.snapshot.custom_oauth.push(entry.clone());
                }
            }
            host.oauth_registered(&entry);
        }
        let mut handoff = false;
        if let Some(source) = source_id.as_ref().filter(|source| !source.is_empty()) {
            let previous = {
                let mut state = self.state.lock().expect("extension store poisoned");
                if !state.snapshot.registered_sources.contains(source) {
                    state.snapshot.registered_sources.push(source.clone());
                }
                state
                    .snapshot
                    .provider_sources
                    .iter()
                    .find(|(name, _)| name == &provider)
                    .map(|(_, source)| source.clone())
            };
            if previous.as_ref().is_some_and(|previous| previous != source) {
                // Native handoff removes old membership first; the old owner
                // remains observable during auth cleanup until the new owner
                // is installed below. Unregistration removes both immediately.
                self.detach_source(&provider, false);
                self.clear_provider_state(&provider, host);
                handoff = true;
            }
            let mut state = self.state.lock().expect("extension store poisoned");
            let mut providers = state
                .snapshot
                .source_providers
                .iter()
                .find(|(name, _)| name == source)
                .map(|(_, providers)| providers.clone())
                .unwrap_or_default();
            providers.insert(provider.clone());
            set_entry(&mut state.snapshot.source_providers, source, providers);
            set_entry(&mut state.snapshot.provider_sources, &provider, source.clone());
        }
        if handoff {
            host.reload_static(true)?;
        }
        if let Some(usage) = &config.usage {
            host.set_runtime_usage(&provider, usage, config.api_key_config());
        }
        if let Some(key) = config.api_key_config().filter(|key| !key.is_empty()) {
            host.install_api_key(&provider, key)?;
            set_entry(
                &mut self.state.lock().expect("extension store poisoned").snapshot.api_key_configs,
                &provider,
                key.to_owned(),
            );
        }
        if let Some(definitions) = config.models.as_ref().filter(|models| !models.is_empty()) {
            let provider_base = text(config.fields(), "baseUrl").unwrap_or_else(|| "".into());
            let provider_api = text(config.fields(), "api");
            let compat = config.fields().record("compat");
            let remote = config.fields().record("remoteCompaction");
            let mut overlays = Vec::new();
            for definition in definitions {
                let overlay = build_custom_model_overlay(
                    &provider,
                    &provider_base,
                    provider_api.as_ref(),
                    config.headers(),
                    config.api_key_config(),
                    boolean(config.fields(), "authHeader"),
                    compat.as_ref(),
                    None,
                    remote.as_ref(),
                    definition,
                )?
                .ok_or_else(|| {
                    wire_error(&[
                        "Provider ".into(),
                        provider.clone(),
                        ", model ".into(),
                        text(definition.fields(), "id").unwrap_or_else(|| "undefined".into()),
                        ": no \"api\" specified.".into(),
                    ])
                })?;
                overlays.push(overlay);
            }
            {
                let mut state = self.state.lock().expect("extension store poisoned");
                state
                    .snapshot
                    .model_overlays
                    .retain(|overlay| text(overlay.fields(), "provider").as_ref() != Some(&provider));
                state.snapshot.model_overlays.extend(overlays.iter().cloned());
            }
            let modifier = config.oauth.as_ref().and_then(|oauth| oauth.modifier.clone());
            if modifier.is_some() && !host.has_full_snapshot() {
                host.ensure_full_snapshot()?;
            }
            {
                let mut state = self.state.lock().expect("extension store poisoned");
                if let Some(modifier) = modifier.clone() {
                    set_entry(&mut state.snapshot.modifiers, &provider, modifier);
                } else {
                    remove_entry(&mut state.snapshot.modifiers, &provider);
                }
            }
            if !host.has_full_snapshot() {
                if modifier.is_some() {
                    host.invalidate_all_provider_lookups();
                } else {
                    host.invalidate_provider(&provider);
                }
                if config.dynamic_fetcher.is_none() {
                    return Ok(());
                }
            }
            let retained = self
                .snapshot()
                .provider_overrides
                .into_iter()
                .find(|(name, _)| name == &provider)
                .map(|(_, value)| value);
            host.models_registered(&provider, &overlays, retained.as_ref())?;
            host.invalidate_provider(&provider);
            if config.dynamic_fetcher.is_none() {
                return Ok(());
            }
        }
        if config.dynamic_fetcher.is_some() {
            let mut options = ModelManagerOptions::new(provider.clone());
            options.static_models = Some(RawModelValue::models(Vec::new()));
            options.cache_ttl_ms = Some(24.0 * 60.0 * 60.0 * 1000.0);
            options.dynamic_models_authoritative = true;
            let manager = RuntimeManagerRegistration {
                provider: provider.clone(),
                options,
                source_id: source_id.unwrap_or_else(|| "".into()),
                config: config.clone(),
            };
            let mut state = self.state.lock().expect("extension store poisoned");
            if let Some(current) = state.snapshot.managers.iter_mut().find(|manager| manager.provider == provider) {
                *current = manager;
            } else {
                state.snapshot.managers.push(manager);
            }
        }
        let should_override = config.fields().get("baseUrl").is_some_and(truthy)
            || config.headers().as_source().is_some()
            || config.api_key_config().is_some_and(|key| !key.is_empty())
            || ["authHeader", "remoteCompaction", "transport"].iter().any(|key| config.fields().get(key).is_some());
        if should_override {
            let mut fields = empty();
            for key in ["baseUrl", "authHeader", "remoteCompaction", "transport"] {
                if let Some(value) = config.fields().record(key) {
                    fields.set_record(key, &value);
                } else {
                    fields.set_undefined(key);
                }
            }
            let incoming =
                ProviderOverride::new(fields, config.headers().clone(), config.api_key_config().map(str::to_owned))?;
            let previous = self
                .snapshot()
                .provider_overrides
                .into_iter()
                .find(|(name, _)| name == &provider)
                .map(|(_, value)| value);
            let merged = merge_override(previous.as_ref(), &incoming)?;
            set_entry(
                &mut self.state.lock().expect("extension store poisoned").snapshot.provider_overrides,
                &provider,
                merged,
            );
            if host.has_full_snapshot() {
                host.transport_applied(&provider, &incoming)?;
            }
            host.invalidate_provider(&provider);
        }
        Ok(())
    }

    pub fn clear_source_registrations(
        &self,
        source: &WireString,
        host: &dyn ExtensionRegistryHost,
    ) -> ExtensionResult<()> {
        let apis = {
            let mut state = self.state.lock().expect("extension store poisoned");
            let apis: Vec<_> = state
                .snapshot
                .custom_apis
                .iter()
                .filter(|entry| entry.source_id.as_ref() == Some(source))
                .map(|entry| entry.api.clone())
                .collect();
            state.snapshot.custom_apis.retain(|entry| entry.source_id.as_ref() != Some(source));
            apis
        };
        for api in apis {
            host.custom_api_removed(&api);
        }
        let oauth = {
            let mut state = self.state.lock().expect("extension store poisoned");
            let oauth: Vec<_> = state
                .snapshot
                .custom_oauth
                .iter()
                .filter(|entry| entry.source_id.as_ref() == Some(source))
                .map(|entry| entry.provider.clone())
                .collect();
            state.snapshot.custom_oauth.retain(|entry| entry.source_id.as_ref() != Some(source));
            oauth
        };
        for provider in oauth {
            host.oauth_removed(&provider);
        }
        let providers = self
            .snapshot()
            .source_providers
            .into_iter()
            .find(|(name, _)| name == source)
            .map(|(_, providers)| providers);
        let Some(providers) = providers.filter(|providers| !providers.is_empty()) else { return Ok(()) };
        host.ensure_full_snapshot()?;
        remove_entry(&mut self.state.lock().expect("extension store poisoned").snapshot.source_providers, source);
        for provider in providers.iter() {
            let owned =
                self.snapshot().provider_sources.iter().any(|(name, owner)| name == provider && owner == source);
            if owned {
                remove_entry(
                    &mut self.state.lock().expect("extension store poisoned").snapshot.provider_sources,
                    provider,
                );
                self.clear_provider_state(provider, host);
            }
        }
        host.reload_static(true)
    }

    pub fn unregister_provider(&self, provider: &WireString, host: &dyn ExtensionRegistryHost) -> ExtensionResult<()> {
        self.detach_source(provider, true);
        self.state
            .lock()
            .expect("extension store poisoned")
            .snapshot
            .custom_oauth
            .retain(|entry| &entry.provider != provider);
        host.oauth_removed(provider);
        host.ensure_full_snapshot()?;
        self.clear_provider_state(provider, host);
        host.reload_static(true)
    }
    pub fn sync_extension_sources(
        &self,
        active: &[WireString],
        host: &dyn ExtensionRegistryHost,
    ) -> ExtensionResult<()> {
        for source in self.snapshot().registered_sources {
            if active.contains(&source) {
                continue;
            }
            self.clear_source_registrations(&source, host)?;
            self.state
                .lock()
                .expect("extension store poisoned")
                .snapshot
                .registered_sources
                .retain(|registered| registered != &source);
        }
        Ok(())
    }

    /// Call with an unprojected whole catalog on every composition. Each hook
    /// gets an isolated deep clone; a failed hook retains the preceding valid
    /// catalog and later hooks still run. Successful hooks do not reset warning
    /// deduplication, matching the native per-provider last-error memo.
    pub fn apply_modifiers(&self, unprojected: &[HostModelRef], host: &dyn ExtensionModifierHost) -> Vec<HostModelRef> {
        let modifiers = self.snapshot().modifiers;
        let mut projected = unprojected.to_vec();
        // After a successful hook, headers are literal materialized strings.
        // Keep that record form between hooks so a resolved value is never
        // interpreted again as an environment/command configuration value.
        let mut projected_records: Option<Vec<ModifierModel>> = None;
        for (provider, modifier) in modifiers {
            let Some(credential) = host.oauth_credential(&provider) else { continue };
            let result =
                catch_unwind(AssertUnwindSafe(|| -> ExtensionResult<(Vec<HostModelRef>, Vec<ModifierModel>)> {
                    let isolated = if let Some(records) = &projected_records {
                        records.clone()
                    } else {
                        projected
                            .iter()
                            .map(|model| {
                                let headers = match model.headers() {
                                    HeaderSlot::Absent => ModifierHeaders::Absent,
                                    HeaderSlot::Undefined => ModifierHeaders::Undefined,
                                    HeaderSlot::Null => ModifierHeaders::Null,
                                    HeaderSlot::Source(_) => ModifierHeaders::Values(
                                        host.materialize_headers(model)?
                                            .map(ResolvedConfigHeaders::into_pairs)
                                            .unwrap_or_default(),
                                    ),
                                };
                                Ok(ModifierModel { fields: (**model.spec()).clone(), headers })
                            })
                            .collect::<ExtensionResult<Vec<_>>>()?
                    };
                    let records = modifier.modify(isolated, &credential)?;
                    let models = records
                        .iter()
                        .map(|model| {
                            let headers = match &model.headers {
                                ModifierHeaders::Absent => HeaderSlot::Absent,
                                ModifierHeaders::Undefined => HeaderSlot::Undefined,
                                ModifierHeaders::Null => HeaderSlot::Null,
                                ModifierHeaders::Values(values) => HeaderSlot::Source(HeaderSource::Config(
                                    HeaderConfigRecord::from_pairs(values.clone()),
                                )),
                            };
                            HostModel::new(Arc::new(model.fields.clone()), headers)
                        })
                        .collect::<ExtensionResult<Vec<_>>>()?;
                    Ok((models, records))
                }))
                .unwrap_or_else(|_| Err(CollapseError::new("extension model modifier panicked".into())));
            match result {
                Ok((models, records)) => {
                    projected = models;
                    projected_records = Some(records);
                }
                Err(error) => {
                    let warning = error.message_wire();
                    let fresh = {
                        let mut state = self.state.lock().expect("extension store poisoned");
                        let same = state
                            .last_modifier_warnings
                            .iter()
                            .any(|(name, previous)| name == &provider && previous == &warning);
                        if !same {
                            set_entry(&mut state.last_modifier_warnings, &provider, warning);
                        }
                        !same
                    };
                    if fresh {
                        host.warn_modifier_failure(&provider, &error);
                    }
                }
            }
        }
        projected
    }

    pub fn suppress_selector(
        &self,
        selector: &WireString,
        until_ms: f64,
        runtime: &mut CollapseRuntime,
        has_live_model: Option<&ModelIdPredicate<'_>>,
    ) -> ExtensionResult<()> {
        let normalized = normalize_suppressed_selector(selector, runtime, has_live_model)?;
        set_entry(&mut self.state.lock().expect("extension store poisoned").suppressed, &normalized, until_ms);
        Ok(())
    }
    pub fn is_selector_suppressed(
        &self,
        selector: &WireString,
        now_ms: f64,
        runtime: &mut CollapseRuntime,
        has_live_model: Option<&ModelIdPredicate<'_>>,
    ) -> ExtensionResult<bool> {
        let normalized = normalize_suppressed_selector(selector, runtime, has_live_model)?;
        let mut state = self.state.lock().expect("extension store poisoned");
        let Some((_, until)) = state.suppressed.iter().find(|(candidate, _)| candidate == &normalized) else {
            return Ok(false);
        };
        if *until == 0.0 || until.is_nan() {
            return Ok(false);
        }
        if *until <= now_ms {
            remove_entry(&mut state.suppressed, &normalized);
            return Ok(false);
        }
        Ok(true)
    }
    pub fn clear_suppressed_selector(
        &self,
        selector: &WireString,
        runtime: &mut CollapseRuntime,
        has_live_model: Option<&ModelIdPredicate<'_>>,
    ) -> ExtensionResult<()> {
        let normalized = normalize_suppressed_selector(selector, runtime, has_live_model)?;
        remove_entry(&mut self.state.lock().expect("extension store poisoned").suppressed, &normalized);
        Ok(())
    }
    /// Full refresh clears all; provider refresh clears its exact `provider/`
    /// prefix; no-reload discovery does not call this operation.
    pub fn clear_suppressed_selectors(&self, provider: Option<&WireString>) {
        let mut state = self.state.lock().expect("extension store poisoned");
        if let Some(provider) = provider {
            let mut prefix = provider.units().to_vec();
            prefix.push(47);
            state.suppressed.retain(|(selector, _)| !selector.units().starts_with(&prefix));
        } else {
            state.suppressed.clear();
        }
    }
}
