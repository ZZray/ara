//! Production Host binding of the fixed OMP model registry (596f2da).
//!
//! The Host owns paths, credentials, fetch, clocks and process configuration.
//! Loader and discovery callbacks run outside the catalog publication mutex.
//! MIT License; Copyright (c) 2025 Mario Zechner;
//! Copyright (c) 2025-2026 Can Bölük; Copyright (c) 2026 Stencil Labs, Inc.

use crate::{
    catalog_discovery::{CatalogContext, DiscoveryError, DiscoveryReply, DiscoveryRequest},
    custom_models::{CustomModelBuildOptions, CustomModelOverlay, build_custom_model_overlay, finalize_custom_model},
    model_cache::SqliteModelCache,
    model_collapse::VariantSpec,
    model_config_file::ModelsConfigFile,
    model_config_values::{
        ConfigValueContext, ConfigValueEnvironment, ConfigValueResolver, HeaderConfigRecord, HeaderSource,
        ProcessConfigEnvironment, ResolveConfigValueOptions,
    },
    model_identity_wire::{boolean, text},
    model_manager::{
        DEFAULT_CACHE_TTL_MS, DynamicModelFetcher, ModelClock, ModelManager, ModelManagerHost, ModelManagerOptions,
        ModelRefreshStrategy, ModelsDevFallback, NON_AUTHORITATIVE_RETRY_MS, RawModelValue,
    },
    model_patch::{HeaderSlot, HostModelRef, ModelPatch, OrderedProviderSet, ProviderOverride},
    model_registry_discovery::{RegistryDiscoveryFetch, discover_configured_models},
    model_registry_extensions::{
        ExtensionModifierHost, ExtensionRegistry, ExtensionRegistryHost, ExtensionSnapshot, OpaqueExtensionHandle,
        RuntimeApiRegistration, RuntimeManagerRegistration, RuntimeOAuthEntry, RuntimeProviderConfig,
    },
    model_registry_loader::{
        ProviderDiscoveryState, ProviderDiscoveryStatus, RegistryDiscoveryConfig, RegistryLoadOptions, RegistryLoader,
        RegistryLoaderHost, STARTUP_CACHE_TTL_MS, normalize_discoverable, storage_spec,
    },
    model_registry_runtime::{
        BuiltInDiscoveryResult, ConfiguredDiscovery, ModelRegistryRuntime, RegistryDiscoveryPublication,
        RegistryRuntimeBackend, RegistryRuntimeOperation, RegistryRuntimeResult,
    },
    models_config::ModelsConfig,
    provider_models::{
        CodexAccountResolver, ModelManagerConfig, OpenAiCodexAccount, OpenAiCodexModelManagerConfig,
        ProviderEnvironment, ProviderFactoryHost, get_catalog_provider_entry,
        is_credential_scoped_model_cache_provider, models_dev_catalog_fallback, models_dev_catalog_provider_ids,
        openai_codex_model_manager_options, provider_descriptors, resolve_model_cache_provider_id,
    },
    static_model_registry::{
        ModelCatalogProjection, StaticModelRegistry, StaticRegistryInputs, bundled_host_models, host_model_from_spec,
    },
};
use ara_rpc::{WireString, WireValue};
use async_trait::async_trait;
use futures::future::try_join_all;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

/// Authentication is a Host service, not authority inferred from catalog rows.
#[async_trait]
pub trait RegistryCredentials: Send + Sync {
    fn has_auth(&self, provider: &WireString) -> bool;
    async fn peek_key(&self, provider: &WireString) -> RegistryRuntimeResult<Option<WireString>>;
    async fn resolve_key(&self, provider: &WireString) -> RegistryRuntimeResult<Option<WireString>> {
        self.peek_key(provider).await
    }
    async fn refresh_key(&self, _provider: &WireString) -> RegistryRuntimeResult<Option<WireString>> {
        Ok(None)
    }
    fn has_oauth_credentials(&self, _provider: &WireString) -> bool {
        false
    }
    fn oauth_credential(&self, _provider: &WireString) -> Option<OpaqueExtensionHandle> {
        None
    }
    async fn codex_accounts(
        &self,
        resolved_access_token: WireString,
        _suppress_stored_oauth: bool,
    ) -> RegistryRuntimeResult<Option<Vec<OpenAiCodexAccount>>> {
        Ok(Some(vec![OpenAiCodexAccount { access_token: resolved_access_token, account_id: None }]))
    }
}

/// Environment-only Hosts have no account refresh or persisted identity.
pub struct EnvironmentRegistryCredentials {
    environment: Arc<dyn ProviderEnvironment>,
}
impl EnvironmentRegistryCredentials {
    pub fn new(environment: Arc<dyn ProviderEnvironment>) -> Self {
        Self { environment }
    }
    fn key(&self, provider: &WireString) -> Option<WireString> {
        get_catalog_provider_entry(provider).and_then(|d| d.env_vars.clone()).into_iter().flatten().find_map(|name| {
            name.to_utf8().ok().and_then(|name| self.environment.get(&name)).filter(|key| !key.is_empty())
        })
    }
}
#[async_trait]
impl RegistryCredentials for EnvironmentRegistryCredentials {
    fn has_auth(&self, provider: &WireString) -> bool {
        self.key(provider).is_some()
    }
    async fn peek_key(&self, provider: &WireString) -> RegistryRuntimeResult<Option<WireString>> {
        Ok(self.key(provider))
    }
}

/// Reference CLI account binding. Auth refresh ownership is shared with the
/// request resolver; catalog metadata never owns credentials or account state.
pub struct OpenAiCodexRegistryCredentials {
    environment: EnvironmentRegistryCredentials,
    auth: Arc<crate::openai_codex_auth::OpenAiCodexAuth>,
    cancel: CancellationToken,
    round_robin: Mutex<Option<usize>>,
}
impl OpenAiCodexRegistryCredentials {
    pub fn new(
        environment: Arc<dyn ProviderEnvironment>,
        auth: Arc<crate::openai_codex_auth::OpenAiCodexAuth>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            environment: EnvironmentRegistryCredentials::new(environment),
            auth,
            cancel,
            round_robin: Mutex::new(None),
        }
    }
    async fn snapshot(&self) -> RegistryRuntimeResult<Vec<crate::credential_store::StoredAuthCredential>> {
        let auth = self.auth.clone();
        tokio::task::spawn_blocking(move || auth.stored_oauth_snapshot())
            .await
            .map_err(|_| error("account snapshot did not settle"))?
            .map_err(|_| error("account snapshot failed"))
    }
    async fn select(
        &self,
        rows: Vec<crate::credential_store::StoredAuthCredential>,
    ) -> RegistryRuntimeResult<Option<crate::credential_store::StoredAuthCredential>> {
        if rows.len() <= 1 {
            return Ok(rows.into_iter().next());
        }
        let index = {
            let mut current = self.round_robin.lock().expect("discovery account order poisoned");
            let next = current.map_or(0, |index| (index + 1) % rows.len());
            *current = Some(next);
            next
        };
        let blocked =
            self.auth.discovery_blocked_ids().await.map_err(|_| error("discovery account block observation failed"))?;
        let selected = (0..rows.len())
            .map(|offset| (index + offset) % rows.len())
            .find(|index| !blocked.contains(&rows[*index].id))
            .unwrap_or(index);
        Ok(rows.into_iter().nth(selected))
    }
    fn account(row: &crate::credential_store::StoredAuthCredential) -> Option<OpenAiCodexAccount> {
        let crate::credential_store::AuthCredential::OAuth { fields } = &row.credential else { return None };
        Some(OpenAiCodexAccount {
            access_token: fields
                .get("access")
                .and_then(serde_json::Value::as_str)
                .filter(|key| !key.is_empty())?
                .into(),
            account_id: fields.get("accountId").and_then(serde_json::Value::as_str).map(Into::into),
        })
    }
}
#[async_trait]
impl RegistryCredentials for OpenAiCodexRegistryCredentials {
    fn has_auth(&self, provider: &WireString) -> bool {
        let stored = provider.equals_ascii("openai-codex")
            && self.auth.stored_oauth_snapshot().is_ok_and(|rows| !rows.is_empty());
        stored || self.environment.has_auth(provider)
    }
    fn has_oauth_credentials(&self, provider: &WireString) -> bool {
        provider.equals_ascii("openai-codex") && self.auth.stored_oauth_snapshot().is_ok_and(|rows| !rows.is_empty())
    }
    fn oauth_credential(&self, provider: &WireString) -> Option<OpaqueExtensionHandle> {
        if !provider.equals_ascii("openai-codex") {
            return None;
        }
        self.auth
            .stored_oauth_snapshot()
            .ok()?
            .into_iter()
            .next()
            .map(|row| Arc::new(row.credential) as OpaqueExtensionHandle)
    }
    async fn peek_key(&self, provider: &WireString) -> RegistryRuntimeResult<Option<WireString>> {
        if provider.equals_ascii("openai-codex")
            && let Some(row) = self.select(self.snapshot().await?).await?
        {
            let crate::credential_store::AuthCredential::OAuth { fields } = &row.credential else { unreachable!() };
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |time| time.as_secs_f64() * 1000.0);
            if fields
                .get("expires")
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|expires| expires.is_finite() && expires > now)
            {
                return Ok(Self::account(&row).map(|account| account.access_token));
            }
        }
        self.environment.peek_key(provider).await
    }
    async fn resolve_key(&self, provider: &WireString) -> RegistryRuntimeResult<Option<WireString>> {
        if provider.equals_ascii("openai-codex")
            && let Some(row) = self.select(self.snapshot().await?).await?
        {
            let row = self
                .auth
                .resolve_discovery_account(row.id, &self.cancel)
                .await
                .map_err(|_| error("account access resolution failed"))?;
            return Ok(Self::account(&row).map(|account| account.access_token));
        }
        self.environment.resolve_key(provider).await
    }
    async fn codex_accounts(
        &self,
        resolved_access_token: WireString,
        suppress_stored_oauth: bool,
    ) -> RegistryRuntimeResult<Option<Vec<OpenAiCodexAccount>>> {
        let mut accounts = Vec::new();
        if !suppress_stored_oauth {
            let rows = self.snapshot().await?;
            let results = futures::future::join_all(
                rows.into_iter().map(|row| self.auth.resolve_discovery_account(row.id, &self.cancel)),
            )
            .await;
            for result in results {
                let Ok(row) = result else { return Ok(None) };
                let Some(account) = Self::account(&row) else { return Ok(None) };
                accounts.push(account);
            }
        }
        if !accounts.iter().any(|account| account.access_token == resolved_access_token) {
            let account_id = self
                .snapshot()
                .await?
                .iter()
                .filter_map(Self::account)
                .find(|account| account.access_token == resolved_access_token)
                .and_then(|account| account.account_id);
            accounts.push(OpenAiCodexAccount { access_token: resolved_access_token, account_id });
        }
        Ok(Some(accounts))
    }
}

/// Host registry for executable extension objects. Callers retain these bindings
/// and downcast handles to their transport, login and usage callback contracts.
/// Private values and callback objects never enter catalog metadata or SQLite.
#[derive(Default)]
pub struct RegistryExtensionBindings {
    apis: Mutex<Vec<RuntimeApiRegistration>>,
    oauth: Mutex<Vec<RuntimeOAuthEntry>>,
    usage: Mutex<Vec<(WireString, OpaqueExtensionHandle, Option<String>)>>,
    raw_keys: Mutex<HashMap<WireString, String>>,
    resolved_keys: Mutex<HashMap<WireString, WireString>>,
}
impl RegistryExtensionBindings {
    pub fn api(&self, api: &WireString) -> Option<OpaqueExtensionHandle> {
        self.apis
            .lock()
            .expect("extension API bindings poisoned")
            .iter()
            .find(|entry| &entry.api == api)
            .map(|entry| entry.handle.clone())
    }
    pub fn oauth(&self, provider: &WireString) -> Option<OpaqueExtensionHandle> {
        self.oauth
            .lock()
            .expect("extension OAuth bindings poisoned")
            .iter()
            .find(|entry| &entry.provider == provider)
            .map(|entry| entry.registration.handle.clone())
    }
    pub fn usage(&self, provider: &WireString) -> Option<(OpaqueExtensionHandle, Option<String>)> {
        self.usage
            .lock()
            .expect("extension usage bindings poisoned")
            .iter()
            .find(|(name, _, _)| name == provider)
            .map(|(_, handle, key)| (handle.clone(), key.clone()))
    }
    fn raw_key(&self, provider: &WireString) -> Option<String> {
        self.raw_keys.lock().expect("extension keys poisoned").get(provider).cloned()
    }
}

/// Secret-bearing dependencies deliberately have no Debug or serialization.
pub struct ModelRegistryHost {
    pub project_dir: PathBuf,
    pub config_values: Arc<ConfigValueResolver>,
    pub config_environment: Arc<dyn ConfigValueEnvironment + Send + Sync>,
    pub factory: ProviderFactoryHost,
    pub cache: Arc<Mutex<SqliteModelCache>>,
    pub clock: Arc<dyn ModelClock>,
    pub context: CatalogContext,
    pub credentials: Arc<dyn RegistryCredentials>,
    pub extension_bindings: Arc<RegistryExtensionBindings>,
    pub cancel: CancellationToken,
}
impl ModelRegistryHost {
    pub fn for_process(project_dir: PathBuf, cache_path: &Path) -> RegistryRuntimeResult<Self> {
        let environment: Arc<dyn ProviderEnvironment> = Arc::new(|name: &str| std::env::var(name).ok().map(Into::into));
        Ok(Self {
            project_dir: project_dir.clone(),
            config_values: Arc::new(ConfigValueResolver::new()),
            config_environment: Arc::new(ProcessConfigEnvironment),
            factory: ProviderFactoryHost::new(environment.clone(), "ara", project_dir),
            cache: Arc::new(Mutex::new(SqliteModelCache::for_path(cache_path))),
            clock: Arc::new(|| SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0)),
            context: CatalogContext::for_host()?,
            credentials: Arc::new(EnvironmentRegistryCredentials::new(environment)),
            extension_bindings: Arc::new(RegistryExtensionBindings::default()),
            cancel: CancellationToken::new(),
        })
    }
}

struct RegistryGeneration {
    loader: Mutex<RegistryLoader>,
    engine: Arc<StaticModelRegistry>,
    config: Option<ModelsConfig>,
    discoveries: Vec<Arc<RegistryDiscoveryConfig>>,
    config_keys: HashMap<WireString, WireString>,
    mtime: Option<f64>,
}
struct RegistryState {
    generation: Arc<RegistryGeneration>,
    force_reload: bool,
    discovered: Vec<HostModelRef>,
    authority: OrderedProviderSet,
    discovery_states: Vec<ProviderDiscoveryState>,
}
pub struct ModelRegistryBackend {
    host: Arc<ModelRegistryHost>,
    options: RegistryLoadOptions,
    file: Mutex<ModelsConfigFile>,
    state: Mutex<RegistryState>,
    reload_gate: Mutex<()>,
    extension_mutation_gate: Mutex<()>,
    extensions: Arc<ExtensionRegistry>,
    self_ref: Weak<ModelRegistryBackend>,
}
#[derive(Clone)]
pub struct ModelRegistry {
    backend: Arc<ModelRegistryBackend>,
    runtime: ModelRegistryRuntime<ModelRegistryBackend>,
}

fn error(message: &'static str) -> DiscoveryError {
    DiscoveryError::new(message)
}

fn is_discovery_bearer(key: &WireString) -> bool {
    !key.is_empty()
        && !["N/A", "llama-cpp-local", "lm-studio-local", "vllm-local"].iter().any(|value| key.equals_ascii(value))
}

fn is_authenticated(key: &WireString) -> bool {
    !key.is_empty() && !key.equals_ascii("N/A")
}

async fn configured_key(
    host: Arc<ModelRegistryHost>,
    generation: &RegistryGeneration,
    provider: &WireString,
) -> RegistryRuntimeResult<Option<WireString>> {
    if let Some(raw) = host
        .extension_bindings
        .raw_key(provider)
        .or_else(|| generation.engine.configured_api_key(provider).map(str::to_owned))
    {
        return tokio::task::spawn_blocking(move || {
            let context =
                ConfigValueContext { project_dir: &host.project_dir, environment: host.config_environment.as_ref() };
            host.config_values
                .resolve_config_value_checked(&raw, &context, ResolveConfigValueOptions::default(), &|| {
                    !host.cancel.is_cancelled()
                })
                .map_err(|_| error("discovery credential resolution cancelled"))
                .map(|key| key.filter(|key| !key.is_empty()).map(Into::into))
        })
        .await
        .map_err(|_| error("discovery credential resolution did not settle"))?;
    }
    Ok(generation.config_keys.get(provider).cloned())
}

fn configured_fallback(
    generation: &RegistryGeneration,
    provider: &WireString,
    host: &ModelRegistryHost,
) -> RegistryRuntimeResult<HeaderSlot> {
    let Some(overrides) = generation.engine.provider_override(provider).filter(|p| {
        boolean(p.fields(), "authHeader") == Some(true) && p.api_key_config().is_some_and(|key| !key.is_empty())
    }) else {
        return Ok(HeaderSlot::Absent);
    };
    let slot = crate::custom_models::merge_auth_header_sources(
        &[overrides.headers().clone()],
        Some(true),
        overrides.api_key_config(),
    );
    let context = ConfigValueContext { project_dir: &host.project_dir, environment: host.config_environment.as_ref() };
    let resolved = match slot.as_source() {
        Some(HeaderSource::Live(headers)) => headers
            .snapshot_checked(&host.config_values, &context, &|| !host.cancel.is_cancelled())
            .map_err(|_| error("discovery header resolution cancelled"))?,
        source => crate::model_config_values::resolve_config_headers(source, &host.config_values, &context),
    };
    Ok(resolved
        .filter(|headers| headers.get("Authorization").is_some_and(|value| !value.is_empty()))
        .map(|headers| HeaderSlot::Source(HeaderSource::Config(HeaderConfigRecord::from_pairs(headers.into_pairs()))))
        .unwrap_or(HeaderSlot::Absent))
}

fn load_generation(
    file: &mut ModelsConfigFile,
    options: &RegistryLoadOptions,
    host: &ModelRegistryHost,
) -> RegistryRuntimeResult<Arc<RegistryGeneration>> {
    let installed = Mutex::new(HashMap::new());
    let install = |provider: &WireString, key: Option<&str>| {
        let mut keys = installed.lock().expect("registry temporary key map poisoned");
        if let Some(key) = key {
            keys.insert(provider.clone(), WireString::from(key));
        } else {
            keys.remove(provider);
        }
        let mut current = host.extension_bindings.resolved_keys.lock().expect("registry installed keys poisoned");
        if let Some(key) = key {
            current.insert(provider.clone(), key.into());
        } else {
            current.remove(provider);
        }
    };
    let has_auth = |provider: &WireString| {
        let installed = {
            host.extension_bindings
                .resolved_keys
                .lock()
                .expect("registry installed keys poisoned")
                .contains_key(provider)
        };
        installed || host.credentials.has_auth(provider)
    };
    let alive = || !host.cancel.is_cancelled();
    let loader_host = RegistryLoaderHost {
        project_dir: &host.project_dir,
        config_values: &host.config_values,
        config_environment: host.config_environment.as_ref(),
        provider_environment: host.factory.environment.as_ref(),
        cache: host.cache.as_ref(),
        clock: host.clock.as_ref(),
        has_auth: &has_auth,
        install_config_key: &install,
        continue_loading: &alive,
    };
    let mut loader = RegistryLoader::load(file, options, &loader_host)
        .map_err(|_| error("model registry configuration loading failed"))?;
    let loaded = loader.loaded_config();
    for provider in &loaded.providers {
        if let Some(raw) = provider.key_config() {
            host.extension_bindings
                .raw_keys
                .lock()
                .expect("registry installed keys poisoned")
                .insert(provider.provider.clone(), raw.to_owned());
        }
    }
    let config = loaded.config().cloned();
    let mtime = loaded.mtime_ms;
    let discoveries: Vec<_> = loaded.discoverable_providers.iter().cloned().map(Arc::new).collect();
    let mut llama_cpp_providers = OrderedProviderSet::default();
    for discovery in &discoveries {
        if discovery.kind().is_some_and(|kind| kind.equals_ascii("llama.cpp")) {
            llama_cpp_providers.insert(discovery.provider.clone());
        }
    }
    let presence =
        loaded.providers.iter().map(|p| (p.provider.clone(), p.resolved_headers().as_source().is_some())).collect();
    // A temporary composition owner applies only the requested cached slices;
    // get_all() is never called during construction.
    let preparation =
        StaticModelRegistry::from_loader_inputs(config.as_ref(), StaticRegistryInputs::default(), Some(&presence))?;
    let cached = loader
        .load_discoverable_cache(&loader_host, |provider, models| {
            preparation.prepare_cached_discovery_models(provider, models)
        })
        .map_err(|_| error("configured model cache loading failed"))?;
    let engine = Arc::new(StaticModelRegistry::from_loader_inputs(
        config.as_ref(),
        StaticRegistryInputs {
            bundled_models: bundled_host_models()?.to_vec(),
            pending_standard_providers: loader.pending_standard_providers(),
            cached_discoverable_models: cached,
            disabled_providers: options.disabled_providers.clone(),
            llama_cpp_providers,
            ..Default::default()
        },
        Some(&presence),
    )?);
    Ok(Arc::new(RegistryGeneration {
        loader: Mutex::new(loader),
        engine,
        config,
        discoveries,
        config_keys: installed.into_inner().expect("registry temporary key map poisoned"),
        mtime,
    }))
}

impl ModelRegistry {
    pub async fn open(
        file: ModelsConfigFile,
        options: RegistryLoadOptions,
        host: ModelRegistryHost,
    ) -> RegistryRuntimeResult<Self> {
        let host = Arc::new(host);
        let worker_host = host.clone();
        let worker_options = options.clone();
        let (file, generation) = tokio::task::spawn_blocking(move || {
            let mut file = file;
            let generation = load_generation(&mut file, &worker_options, &worker_host)?;
            Ok::<_, DiscoveryError>((file, generation))
        })
        .await
        .map_err(|_| error("model registry loader did not settle"))??;
        let states = generation.loader.lock().expect("registry loader poisoned").discovery_states().to_vec();
        let backend = Arc::new_cyclic(|self_ref| ModelRegistryBackend {
            host,
            options,
            file: Mutex::new(file),
            state: Mutex::new(RegistryState {
                generation,
                force_reload: false,
                discovered: Vec::new(),
                authority: OrderedProviderSet::default(),
                discovery_states: states,
            }),
            reload_gate: Mutex::new(()),
            extension_mutation_gate: Mutex::new(()),
            extensions: Arc::new(ExtensionRegistry::new()),
            self_ref: self_ref.clone(),
        });
        backend.generation().engine.install_projection(Arc::new(RuntimeProjection(Arc::downgrade(&backend))));
        Ok(Self { runtime: ModelRegistryRuntime::new(backend.clone()), backend })
    }
    pub fn runtime(&self) -> &ModelRegistryRuntime<ModelRegistryBackend> {
        &self.runtime
    }
    pub fn config(&self) -> Option<ModelsConfig> {
        self.backend.generation().config.clone()
    }
    pub fn config_error(&self) -> Option<crate::model_config_file::ConfigFileError> {
        self.backend.generation().loader.lock().expect("registry loader poisoned").loaded_config().error.clone()
    }
    pub fn discovery_states(&self) -> Vec<ProviderDiscoveryState> {
        self.backend.state.lock().expect("registry state poisoned").discovery_states.clone()
    }
    pub fn find_exact(&self, provider: &WireString, id: &WireString) -> RegistryRuntimeResult<Option<HostModelRef>> {
        let generation = self.backend.generation();
        self.backend.load_standard(&generation, Some(provider))?;
        Ok(generation.engine.find_exact(provider, id)?)
    }
    pub fn find_reference(
        &self,
        provider: &WireString,
        id: &WireString,
    ) -> RegistryRuntimeResult<Option<HostModelRef>> {
        let generation = self.backend.generation();
        self.backend.load_standard(&generation, Some(provider))?;
        Ok(generation.engine.find_reference(provider, id)?)
    }
    pub fn get_all(&self) -> RegistryRuntimeResult<Vec<HostModelRef>> {
        let generation = self.backend.generation();
        self.backend.load_standard(&generation, None)?;
        Ok(generation.engine.get_all()?)
    }
    pub fn models_for_provider(&self, provider: &WireString) -> RegistryRuntimeResult<Vec<HostModelRef>> {
        let generation = self.backend.generation();
        self.backend.load_standard(&generation, Some(provider))?;
        Ok(generation.engine.models_for_provider_lookup(provider)?)
    }

    pub fn extension_snapshot(&self) -> ExtensionSnapshot {
        self.backend.extensions.snapshot()
    }
    pub async fn register_provider(
        &self,
        provider: WireString,
        config: RuntimeProviderConfig,
        source: Option<WireString>,
    ) -> RegistryRuntimeResult<()> {
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || {
            let _owner = backend.extension_mutation_gate.lock().expect("extension mutation owner poisoned");
            backend.extensions.register_provider(provider, &config, source, backend.as_ref()).map_err(Into::into)
        })
        .await
        .map_err(|_| error("extension registration did not settle"))?
    }
    pub async fn unregister_provider(&self, provider: WireString) -> RegistryRuntimeResult<()> {
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || {
            let _owner = backend.extension_mutation_gate.lock().expect("extension mutation owner poisoned");
            backend.extensions.unregister_provider(&provider, backend.as_ref()).map_err(Into::into)
        })
        .await
        .map_err(|_| error("extension removal did not settle"))?
    }
    pub async fn sync_extension_sources(&self, active: Vec<WireString>) -> RegistryRuntimeResult<()> {
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || {
            let _owner = backend.extension_mutation_gate.lock().expect("extension mutation owner poisoned");
            backend.extensions.sync_extension_sources(&active, backend.as_ref()).map_err(Into::into)
        })
        .await
        .map_err(|_| error("extension source synchronization did not settle"))?
    }
    pub fn suppress_selector(&self, selector: &WireString, until_ms: f64) -> RegistryRuntimeResult<()> {
        let models = self.selector_models(selector)?;
        let references = crate::provider_model_reference::ProviderModelReferenceIndex::new(&models)?;
        let live = |provider: &WireString, id: &WireString| {
            references.resolve(provider, id).is_ok_and(|model| model.is_some())
        };
        let collapse = self.backend.generation().engine.collapse_runtime();
        let mut runtime = collapse.lock().expect("registry collapse poisoned");
        self.backend.extensions.suppress_selector(selector, until_ms, &mut runtime, Some(&live))?;
        Ok(())
    }
    pub fn is_selector_suppressed(&self, selector: &WireString) -> RegistryRuntimeResult<bool> {
        let models = self.selector_models(selector)?;
        let references = crate::provider_model_reference::ProviderModelReferenceIndex::new(&models)?;
        let live = |provider: &WireString, id: &WireString| {
            references.resolve(provider, id).is_ok_and(|model| model.is_some())
        };
        let collapse = self.backend.generation().engine.collapse_runtime();
        let mut runtime = collapse.lock().expect("registry collapse poisoned");
        Ok(self.backend.extensions.is_selector_suppressed(
            selector,
            self.backend.host.clock.now(),
            &mut runtime,
            Some(&live),
        )?)
    }
    fn selector_models(&self, selector: &WireString) -> RegistryRuntimeResult<Vec<HostModelRef>> {
        let selector = crate::model_collapse::trim(selector);
        let Some(slash) = selector.units().iter().position(|unit| *unit == 47) else {
            return Ok(Vec::new());
        };
        self.models_for_provider(&WireString::from_units(selector.units()[..slash].to_vec()))
    }
}

impl ModelRegistryBackend {
    fn generation(&self) -> Arc<RegistryGeneration> {
        self.state.lock().expect("registry state poisoned").generation.clone()
    }
    fn sync_runtime_layers(&self) {
        let snapshot = self.extensions.snapshot();
        self.generation().engine.install_runtime_layers(snapshot.model_overlays, snapshot.provider_overrides);
    }
    fn reload_static_now(&self, force: bool) -> RegistryRuntimeResult<()> {
        let _serial = self.reload_gate.lock().expect("registry reload owner poisoned");
        let mut file = self.file.lock().expect("registry config file poisoned").clone();
        let mtime = file.get_mtime_ms().map_err(|_| error("model configuration metadata failed"))?;
        {
            let state = self.state.lock().expect("registry state poisoned");
            if !force && !state.force_reload && state.generation.mtime == mtime {
                return Ok(());
            }
        }
        self.host.extension_bindings.raw_keys.lock().expect("registry installed keys poisoned").clear();
        self.host.extension_bindings.resolved_keys.lock().expect("registry installed keys poisoned").clear();
        for (provider, key) in self.extensions.snapshot().api_key_configs {
            self.install_api_key(&provider, &key)?;
        }
        file.invalidate();
        let generation = load_generation(&mut file, &self.options, &self.host)?;
        let snapshot = self.extensions.snapshot();
        generation.engine.install_runtime_layers(snapshot.model_overlays, snapshot.provider_overrides);
        generation.engine.install_projection(Arc::new(RuntimeProjection(self.self_ref.clone())));
        let states = generation.loader.lock().expect("registry loader poisoned").discovery_states().to_vec();
        *self.file.lock().expect("registry config file poisoned") = file;
        let mut state = self.state.lock().expect("registry state poisoned");
        state.generation = generation;
        state.force_reload = false;
        state.discovered.clear();
        state.authority = OrderedProviderSet::default();
        state.discovery_states = states;
        Ok(())
    }
    fn loader_host<'a>(
        &'a self,
        has_auth: &'a dyn Fn(&WireString) -> bool,
        install: &'a dyn Fn(&WireString, Option<&str>),
        alive: &'a dyn Fn() -> bool,
    ) -> RegistryLoaderHost<'a> {
        RegistryLoaderHost {
            project_dir: &self.host.project_dir,
            config_values: &self.host.config_values,
            config_environment: self.host.config_environment.as_ref(),
            provider_environment: self.host.factory.environment.as_ref(),
            cache: self.host.cache.as_ref(),
            clock: self.host.clock.as_ref(),
            has_auth,
            install_config_key: install,
            continue_loading: alive,
        }
    }
    fn load_standard(
        &self,
        generation: &RegistryGeneration,
        provider: Option<&WireString>,
    ) -> RegistryRuntimeResult<()> {
        // Authentication observation precedes the loader lock. Standard cache
        // slices execute no helpers or network operations.
        let has_auth = |_provider: &WireString| false;
        let install = |_: &WireString, _: Option<&str>| {};
        let alive = || !self.host.cancel.is_cancelled();
        let host = self.loader_host(&has_auth, &install, &alive);
        let mut loader = generation.loader.lock().expect("registry loader poisoned");
        let provider = if self.extensions.snapshot().modifiers.is_empty() { provider } else { None };
        let filter = provider.map(|p| {
            let normalized = crate::model_collapse::lower(&crate::model_collapse::trim(p));
            let mut set = OrderedProviderSet::default();
            for candidate in loader.startup_roster() {
                if crate::model_collapse::lower(&crate::model_collapse::trim(candidate)) == normalized {
                    set.insert(candidate.clone());
                }
            }
            set
        });
        let slices = loader
            .load_standard_providers(
                filter.as_ref(),
                &host,
                |provider| {
                    let base = generation.engine.provider_override(provider).and_then(|p| text(p.fields(), "baseUrl"));
                    let bundled = bundled_host_models()?
                        .iter()
                        .filter(|m| text(m.spec(), "provider").as_ref() == Some(provider))
                        .cloned()
                        .collect();
                    Ok((base, bundled))
                },
                |provider, models| generation.engine.prepare_cached_provider_models(provider, models),
            )
            .map_err(|_| error("standard model cache loading failed"))?;
        let mut touched = OrderedProviderSet::default();
        for slice in &slices {
            if slice.newly_loaded {
                touched.insert(slice.provider.clone());
            }
        }
        let cached = loader.cached_standard_models_snapshot();
        let authority = loader.authoritative_provider_snapshot();
        drop(loader);
        if !touched.is_empty() {
            generation.engine.inject_standard_cache_snapshot(cached, authority, &touched)?;
        }
        Ok(())
    }
    async fn key(
        &self,
        generation: &RegistryGeneration,
        provider: &WireString,
        resolve: bool,
    ) -> RegistryRuntimeResult<Option<WireString>> {
        if generation.engine.configured_api_key(provider).is_some()
            || self.host.extension_bindings.raw_key(provider).is_some()
        {
            return configured_key(self.host.clone(), generation, provider).await;
        }
        if resolve {
            self.host.credentials.resolve_key(provider).await
        } else {
            self.host.credentials.peek_key(provider).await
        }
    }
    fn update_state(&self, config: Option<&Arc<RegistryDiscoveryConfig>>, state: ProviderDiscoveryState) {
        let mut owner = self.state.lock().expect("registry state poisoned");
        if config.is_some_and(|config| !owner.generation.discoveries.iter().any(|current| Arc::ptr_eq(current, config)))
        {
            return;
        }
        if let Some(old) = owner.discovery_states.iter_mut().find(|old| old.provider == state.provider) {
            *old = state;
        } else {
            owner.discovery_states.push(state);
        }
    }
    async fn resolve_builtin_key(
        &self,
        generation: &RegistryGeneration,
        provider: &WireString,
        strategy: ModelRefreshStrategy,
        authoritative: bool,
    ) -> RegistryRuntimeResult<Option<WireString>> {
        let peek = self.key(generation, provider, false).await?;
        if peek.as_ref().is_some_and(is_authenticated)
            || strategy == ModelRefreshStrategy::Offline
            || !self.host.credentials.has_oauth_credentials(provider)
        {
            return Ok(peek);
        }
        if strategy == ModelRefreshStrategy::OnlineIfUncached && !authoritative {
            let cache_id = resolve_model_cache_provider_id(
                provider,
                None,
                generation.engine.provider_override(provider).and_then(|p| text(p.fields(), "baseUrl")).as_ref(),
                self.host.factory.environment.as_ref(),
            );
            let cache = self
                .host
                .cache
                .lock()
                .expect("registry cache poisoned")
                .read_model_cache_wire(&cache_id, DEFAULT_CACHE_TTL_MS, || self.host.clock.now())
                .ok()
                .flatten();
            if cache.is_some_and(|entry| {
                entry.fresh
                    && (entry.authoritative || self.host.clock.now() - entry.updated_at < NON_AUTHORITATIVE_RETRY_MS)
            }) {
                return Ok(peek);
            }
        }
        self.key(generation, provider, true).await.or(Ok(peek))
    }
}

struct RuntimeProjection(Weak<ModelRegistryBackend>);
impl ModelCatalogProjection for RuntimeProjection {
    fn has_modifiers(&self) -> bool {
        self.0.upgrade().is_some_and(|backend| !backend.extensions.snapshot().modifiers.is_empty())
    }
    fn project(&self, models: &[HostModelRef]) -> Vec<HostModelRef> {
        self.0
            .upgrade()
            .map_or_else(|| models.to_vec(), |backend| backend.extensions.apply_modifiers(models, backend.as_ref()))
    }
}
impl ExtensionModifierHost for ModelRegistryBackend {
    fn oauth_credential(&self, provider: &WireString) -> Option<OpaqueExtensionHandle> {
        self.host.credentials.oauth_credential(provider)
    }
    fn materialize_headers(
        &self,
        model: &HostModelRef,
    ) -> Result<Option<crate::model_config_values::ResolvedConfigHeaders>, crate::model_collapse::CollapseError> {
        let context = ConfigValueContext {
            project_dir: &self.host.project_dir,
            environment: self.host.config_environment.as_ref(),
        };
        match model.headers().as_source() {
            Some(HeaderSource::Config(record)) => {
                Ok(Some(crate::model_config_values::ResolvedConfigHeaders::from_pairs(record.snapshot())))
            }
            Some(HeaderSource::Live(headers)) => headers
                .snapshot_checked(&self.host.config_values, &context, &|| !self.host.cancel.is_cancelled())
                .map_err(|_| crate::model_collapse::CollapseError::new("extension header snapshot cancelled".into())),
            None => Ok(None),
        }
    }
    fn warn_modifier_failure(&self, _provider: &WireString, _failure: &crate::model_collapse::CollapseError) {
        self.host
            .factory
            .warn("extension model modifier failed", VariantSpec::from_json(&serde_json::json!({"stage":"modifier"})));
    }
}
impl ExtensionRegistryHost for ModelRegistryBackend {
    fn custom_api_registered(&self, entry: &RuntimeApiRegistration) {
        let mut entries = self.host.extension_bindings.apis.lock().expect("extension API bindings poisoned");
        entries.retain(|current| current.api != entry.api);
        entries.push(entry.clone());
    }
    fn custom_api_removed(&self, api: &WireString) {
        self.host
            .extension_bindings
            .apis
            .lock()
            .expect("extension API bindings poisoned")
            .retain(|entry| &entry.api != api);
    }
    fn oauth_registered(&self, entry: &RuntimeOAuthEntry) {
        let mut entries = self.host.extension_bindings.oauth.lock().expect("extension OAuth bindings poisoned");
        entries.retain(|current| current.provider != entry.provider);
        entries.push(entry.clone());
    }
    fn oauth_removed(&self, provider: &WireString) {
        self.host
            .extension_bindings
            .oauth
            .lock()
            .expect("extension OAuth bindings poisoned")
            .retain(|entry| &entry.provider != provider);
    }
    fn set_runtime_usage(&self, provider: &WireString, usage: &OpaqueExtensionHandle, raw_key: Option<&str>) {
        let mut entries = self.host.extension_bindings.usage.lock().expect("extension usage bindings poisoned");
        entries.retain(|(name, _, _)| name != provider);
        entries.push((provider.clone(), usage.clone(), raw_key.map(str::to_owned)));
    }
    fn remove_runtime_usage(&self, provider: &WireString) {
        self.host
            .extension_bindings
            .usage
            .lock()
            .expect("extension usage bindings poisoned")
            .retain(|(name, _, _)| name != provider);
    }
    fn install_api_key(
        &self,
        provider: &WireString,
        raw_key: &str,
    ) -> Result<(), crate::model_collapse::CollapseError> {
        self.host
            .extension_bindings
            .raw_keys
            .lock()
            .expect("extension keys poisoned")
            .insert(provider.clone(), raw_key.to_owned());
        let context = ConfigValueContext {
            project_dir: &self.host.project_dir,
            environment: self.host.config_environment.as_ref(),
        };
        let resolved = self
            .host
            .config_values
            .resolve_config_value_checked(raw_key, &context, ResolveConfigValueOptions::default(), &|| {
                !self.host.cancel.is_cancelled()
            })
            .map_err(|_| {
                crate::model_collapse::CollapseError::new("extension credential resolution cancelled".into())
            })?;
        let mut keys = self.host.extension_bindings.resolved_keys.lock().expect("extension keys poisoned");
        if let Some(key) = resolved.filter(|key| !key.is_empty()) {
            keys.insert(provider.clone(), key.into());
        } else if raw_key.starts_with('!') {
            keys.remove(provider);
        }
        Ok(())
    }
    fn remove_config_api_key(&self, provider: &WireString) {
        self.host.extension_bindings.raw_keys.lock().expect("extension keys poisoned").remove(provider);
        self.host.extension_bindings.resolved_keys.lock().expect("extension keys poisoned").remove(provider);
    }
    fn has_full_snapshot(&self) -> bool {
        self.generation().engine.has_full_snapshot()
    }
    fn ensure_full_snapshot(&self) -> Result<(), crate::model_collapse::CollapseError> {
        self.sync_runtime_layers();
        let generation = self.generation();
        self.load_standard(&generation, None)
            .map_err(|_| crate::model_collapse::CollapseError::new("extension full catalog loading failed".into()))?;
        generation.engine.get_all()?;
        Ok(())
    }
    fn reload_static(&self, force: bool) -> Result<(), crate::model_collapse::CollapseError> {
        self.reload_static_now(force)
            .map_err(|_| crate::model_collapse::CollapseError::new("extension static reload failed".into()))
    }
    fn models_registered(
        &self,
        provider: &WireString,
        overlays: &[CustomModelOverlay],
        retained: Option<&ProviderOverride>,
    ) -> Result<(), crate::model_collapse::CollapseError> {
        self.sync_runtime_layers();
        self.generation().engine.register_runtime_models(provider, overlays, retained)
    }
    fn transport_applied(
        &self,
        provider: &WireString,
        incoming: &ProviderOverride,
    ) -> Result<(), crate::model_collapse::CollapseError> {
        self.sync_runtime_layers();
        self.generation().engine.apply_runtime_transport(provider, incoming)
    }
    fn invalidate_provider(&self, provider: &WireString) {
        self.sync_runtime_layers();
        self.generation().engine.invalidate_provider(provider);
    }
    fn invalidate_all_provider_lookups(&self) {
        self.sync_runtime_layers();
        self.generation().engine.invalidate_all_provider_lookups();
    }
}

struct AdditiveFallback(Arc<dyn ModelsDevFallback>);
#[async_trait]
impl ModelsDevFallback for AdditiveFallback {
    async fn fetch(&self) -> RegistryRuntimeResult<RawModelValue> {
        self.0.fetch().await
    }
    fn map(&self, payload: RawModelValue, provider: &WireString) -> RegistryRuntimeResult<RawModelValue> {
        self.0.map(payload, provider)
    }
    fn additive_only(&self) -> bool {
        true
    }
}
struct RuntimeFetcher {
    backend: Weak<ModelRegistryBackend>,
    registration: RuntimeManagerRegistration,
}
#[async_trait]
impl DynamicModelFetcher for RuntimeFetcher {
    async fn fetch(&self) -> RegistryRuntimeResult<RawModelValue> {
        let backend = self.backend.upgrade().ok_or_else(|| error("runtime discovery owner was released"))?;
        let generation = backend.generation();
        let provider = &self.registration.provider;
        let key = backend.key(&generation, provider, false).await?.filter(is_authenticated);
        let config = &self.registration.config;
        let fetcher = config.dynamic_fetcher.clone().ok_or_else(|| error("runtime discovery callback is absent"))?;
        // The timer does not cancel the original extension callback producer.
        let worker = tokio::spawn(async move { fetcher.fetch(key).await });
        let definitions = tokio::time::timeout(std::time::Duration::from_secs(15), worker)
            .await
            .map_err(|_| error("runtime definition discovery timed out"))?
            .map_err(|_| error("runtime definition discovery did not settle"))??;
        if !(matches!(&definitions, RawModelValue::Models(_))
            || matches!(&definitions, RawModelValue::Value(value) if matches!(value.value, WireValue::Array(_))))
        {
            return Err(error("runtime definition discovery returned a non-array"));
        }
        let mut models = Vec::new();
        for definition in definitions.rows() {
            let mut fields = definition.as_ref().clone();
            let headers = crate::model_registry_loader::take_spec_headers(&mut fields)
                .map_err(|_| error("runtime model definition headers are invalid"))?;
            let definition = ModelPatch::new(fields, headers)?;
            let base = text(definition.fields(), "baseUrl")
                .or_else(|| text(config.fields(), "baseUrl"))
                .unwrap_or_else(|| "".into());
            let api = text(definition.fields(), "api").or_else(|| text(config.fields(), "api"));
            let compat = config.fields().record("compat");
            let remote = config.fields().record("remoteCompaction");
            if let Some(overlay) = build_custom_model_overlay(
                provider,
                &base,
                api.as_ref(),
                config.headers(),
                config.api_key_config(),
                boolean(config.fields(), "authHeader"),
                compat.as_ref(),
                None,
                remote.as_ref(),
                &definition,
            )? {
                let model = finalize_custom_model(&overlay, CustomModelBuildOptions { use_defaults: true })?;
                let model = match model.headers() {
                    HeaderSlot::Source(HeaderSource::Live(headers)) => {
                        let host = backend.host.clone();
                        let headers = headers.clone();
                        let resolved = tokio::task::spawn_blocking(move || {
                            let context = ConfigValueContext {
                                project_dir: &host.project_dir,
                                environment: host.config_environment.as_ref(),
                            };
                            headers.snapshot_checked(&host.config_values, &context, &|| !host.cancel.is_cancelled())
                        })
                        .await
                        .map_err(|_| error("runtime model headers did not settle"))?
                        .map_err(|_| error("runtime model headers cancelled"))?;
                        crate::model_patch::HostModel::new(
                            model.spec().clone(),
                            resolved
                                .map(|headers| {
                                    HeaderSlot::Source(HeaderSource::Config(HeaderConfigRecord::from_pairs(
                                        headers.into_pairs(),
                                    )))
                                })
                                .unwrap_or(HeaderSlot::Absent),
                        )?
                    }
                    _ => model,
                };
                models.push(storage_spec(&model).map_err(|_| error("runtime model definition conversion failed"))?);
            }
        }
        Ok(RawModelValue::models(models))
    }
}

struct RegistryCodexAccounts {
    credentials: Arc<dyn RegistryCredentials>,
    resolved_access_token: WireString,
    suppress_stored_oauth: bool,
}
#[async_trait]
impl CodexAccountResolver for RegistryCodexAccounts {
    async fn resolve_accounts(&self) -> RegistryRuntimeResult<Option<Vec<OpenAiCodexAccount>>> {
        self.credentials.codex_accounts(self.resolved_access_token.clone(), self.suppress_stored_oauth).await
    }
}

struct ConfiguredFetcher {
    host: Arc<ModelRegistryHost>,
    generation: Arc<RegistryGeneration>,
    config: Arc<RegistryDiscoveryConfig>,
    failure: Arc<Mutex<bool>>,
}
#[async_trait]
impl DynamicModelFetcher for ConfiguredFetcher {
    async fn fetch(&self) -> RegistryRuntimeResult<RawModelValue> {
        let fetch = AuthenticatedDiscoveryFetch {
            host: self.host.clone(),
            generation: self.generation.clone(),
            allow_bearer: !self.config.kind().is_some_and(|kind| kind.equals_ascii("ollama")),
            settled_headers: Mutex::new(HashMap::new()),
        };
        match discover_configured_models(&self.config, &self.host.context, Arc::new(fetch)).await {
            Ok(models) => {
                let prepared = self.generation.engine.prepare_cached_discovery_models(&self.config.provider, models)?;
                Ok(RawModelValue::models(
                    prepared
                        .iter()
                        .map(storage_spec)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| error("discovery storage projection failed"))?,
                ))
            }
            Err(_) => {
                *self.failure.lock().expect("registry discovery failure poisoned") = true;
                Ok(RawModelValue::null())
            }
        }
    }
}
struct AuthenticatedDiscoveryFetch {
    host: Arc<ModelRegistryHost>,
    generation: Arc<RegistryGeneration>,
    allow_bearer: bool,
    settled_headers: Mutex<HashMap<WireString, Vec<(WireString, WireString)>>>,
}
#[async_trait]
impl RegistryDiscoveryFetch for AuthenticatedDiscoveryFetch {
    fn settled_request_headers(
        &self,
        provider: &WireString,
        fallback: &[(WireString, WireString)],
    ) -> Vec<(WireString, WireString)> {
        let mut headers = fallback.to_vec();
        if let Some(actual) = self.settled_headers.lock().expect("discovery header receipts poisoned").get(provider) {
            for (name, value) in actual
                .iter()
                .filter(|(name, _)| name.to_utf8().is_ok_and(|name| name.eq_ignore_ascii_case("authorization")))
            {
                headers.retain(|(old, _)| !old.to_utf8().is_ok_and(|old| old.eq_ignore_ascii_case("authorization")));
                headers.push((name.clone(), value.clone()));
            }
        }
        headers
    }
    async fn fetch(
        &self,
        provider: &WireString,
        mut request: DiscoveryRequest,
    ) -> RegistryRuntimeResult<DiscoveryReply> {
        if !self.allow_bearer {
            return self.host.context.transport.fetch(request).await;
        }
        let key = if self.generation.engine.configured_api_key(provider).is_some()
            || self.host.extension_bindings.raw_key(provider).is_some()
        {
            configured_key(self.host.clone(), &self.generation, provider).await?
        } else {
            self.host.credentials.resolve_key(provider).await?
        };
        let attach = |request: &mut DiscoveryRequest, key: &Option<WireString>| {
            if let Some(key) = key.as_ref().filter(|key| is_discovery_bearer(key)) {
                request
                    .headers
                    .retain(|(name, _)| !name.to_utf8().is_ok_and(|name| name.eq_ignore_ascii_case("authorization")));
                let bearer =
                    WireString::from_units("Bearer ".encode_utf16().chain(key.units().iter().copied()).collect());
                request.headers.push(("Authorization".into(), bearer));
            }
        };
        attach(&mut request, &key);
        let reply = self.host.context.transport.fetch(request.clone()).await?;
        if reply.ok() {
            self.settled_headers
                .lock()
                .expect("discovery header receipts poisoned")
                .insert(provider.clone(), request.headers.clone());
        }
        if reply.status != 401 || key.as_ref().is_none_or(|key| !is_discovery_bearer(key)) {
            return Ok(reply);
        }
        let raw_key = self.generation.engine.configured_api_key(provider).map(str::to_owned);
        let refreshed = if let Some(raw_key) = raw_key.filter(|key| key.starts_with('!')) {
            let host = self.host.clone();
            tokio::task::spawn_blocking(move || {
                host.config_values.invalidate_command_config(Some(&raw_key));
                let context = ConfigValueContext {
                    project_dir: &host.project_dir,
                    environment: host.config_environment.as_ref(),
                };
                host.config_values
                    .resolve_config_value_checked(&raw_key, &context, ResolveConfigValueOptions::default(), &|| {
                        !host.cancel.is_cancelled()
                    })
                    .map_err(|_| error("discovery credential refresh cancelled"))
                    .map(|key| key.map(Into::into))
            })
            .await
            .map_err(|_| error("discovery credential refresh did not settle"))??
        } else {
            self.host.credentials.refresh_key(provider).await?
        };
        if refreshed.is_none() || refreshed == key {
            return Ok(reply);
        }
        attach(&mut request, &refreshed);
        let reply = self.host.context.transport.fetch(request.clone()).await?;
        if reply.ok() {
            self.settled_headers
                .lock()
                .expect("discovery header receipts poisoned")
                .insert(provider.clone(), request.headers);
        }
        Ok(reply)
    }
}

#[async_trait]
impl RegistryRuntimeBackend for ModelRegistryBackend {
    type Config = RegistryDiscoveryConfig;
    async fn reload_static(&self) -> RegistryRuntimeResult<()> {
        let backend = self.self_ref.upgrade().ok_or_else(|| error("registry reload owner was released"))?;
        tokio::task::spawn_blocking(move || {
            let _owner = backend.extension_mutation_gate.lock().expect("extension mutation owner poisoned");
            backend.reload_static_now(false)
        })
        .await
        .map_err(|_| error("model configuration reload did not settle"))?
    }
    fn clear_suppressed_selectors(&self, provider: Option<&WireString>) {
        self.extensions.clear_suppressed_selectors(provider);
    }
    fn configured_discoveries(&self) -> Vec<ConfiguredDiscovery<Self::Config>> {
        self.generation()
            .discoveries
            .iter()
            .map(|config| ConfiguredDiscovery { provider: config.provider.clone(), config: config.clone() })
            .collect()
    }
    fn disabled_provider_ids(&self) -> OrderedProviderSet {
        self.options.disabled_providers.clone()
    }
    fn runtime_provider_ids(&self) -> OrderedProviderSet {
        self.extensions.runtime_provider_ids()
    }
    fn credential_scoped_provider_ids(&self) -> OrderedProviderSet {
        let mut providers = OrderedProviderSet::default();
        let generation = self.generation();
        for provider in generation.loader.lock().expect("registry loader poisoned").startup_roster() {
            if is_credential_scoped_model_cache_provider(provider) {
                providers.insert(provider.clone());
            }
        }
        providers
    }
    async fn discover_configured(
        &self,
        config: Arc<Self::Config>,
        strategy: ModelRefreshStrategy,
    ) -> RegistryRuntimeResult<Vec<HostModelRef>> {
        let generation = self.generation();
        let key = config.cache_provider_id();
        let cached = self
            .host
            .cache
            .lock()
            .expect("registry cache poisoned")
            .read_model_cache_wire(&key, STARTUP_CACHE_TTL_MS, || self.host.clock.now())
            .ok()
            .flatten();
        let config_mtime = self
            .file
            .lock()
            .expect("registry config file poisoned")
            .get_mtime_ms()
            .map_err(|_| error("model configuration metadata failed"))?;
        let older = cached.as_ref().is_some_and(|c| config_mtime.is_some_and(|mtime| c.updated_at < mtime.floor()));
        let bypass = config.kind().is_some_and(|kind| kind.equals_ascii("llama.cpp"))
            && strategy == ModelRefreshStrategy::OnlineIfUncached;
        let effective = if strategy == ModelRefreshStrategy::OnlineIfUncached && (older || bypass) {
            ModelRefreshStrategy::Online
        } else {
            strategy
        };
        let keyless = generation
            .loader
            .lock()
            .expect("registry loader poisoned")
            .loaded_config()
            .keyless_providers
            .contains(&config.provider);
        if !keyless && self.key(&generation, &config.provider, false).await?.is_none_or(|key| !is_authenticated(&key)) {
            let models = cached
                .as_ref()
                .map(|c| {
                    c.models
                        .value
                        .as_array()
                        .unwrap_or_default()
                        .iter()
                        .map(|v| host_model_from_spec(&Arc::new(VariantSpec::from_wire(v.clone()))))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?
                .unwrap_or_default();
            let models = normalize_discoverable(&config, models, self.host.factory.environment.as_ref())
                .map_err(|_| error("cached discovery normalization failed"))?;
            self.update_state(
                Some(&config),
                ProviderDiscoveryState {
                    provider: config.provider.clone(),
                    status: ProviderDiscoveryStatus::Unauthenticated,
                    optional: config.optional,
                    stale: cached.is_some(),
                    fetched_at: cached.as_ref().map(|c| c.updated_at),
                    source: None,
                    models: models.iter().filter_map(|m| text(m.spec(), "id")).collect(),
                },
            );
            return Ok(models);
        }
        let fallback_host = self.host.clone();
        let fallback_generation = generation.clone();
        let fallback_provider = config.provider.clone();
        let fallback = tokio::task::spawn_blocking(move || {
            configured_fallback(&fallback_generation, &fallback_provider, &fallback_host)
        })
        .await
        .map_err(|_| error("configured discovery headers did not settle"))??;
        let fallback = match fallback {
            HeaderSlot::Source(crate::model_config_values::HeaderSource::Config(record)) => {
                Some(VariantSpec::from_json(&serde_json::Value::Object(
                    record.snapshot().into_iter().map(|(k, v)| (k, serde_json::Value::String(v))).collect(),
                )))
            }
            _ => None,
        };
        let failure = Arc::new(Mutex::new(false));
        let mut options = ModelManagerOptions::new(config.provider.clone());
        options.static_models = Some(RawModelValue::models(Vec::new()));
        options.cache_provider_id = Some(key);
        options.cache_ttl_ms = Some(STARTUP_CACHE_TTL_MS);
        options.restorable_header_fallback = fallback;
        options.dynamic_fetcher = Some(Arc::new(ConfiguredFetcher {
            host: self.host.clone(),
            generation: generation.clone(),
            config: config.clone(),
            failure: failure.clone(),
        }));
        let manager = ModelManager::new(
            options,
            self.host.context.clone(),
            Arc::new(ModelManagerHost { cache: self.host.cache.clone(), clock: self.host.clock.clone() }),
        );
        let result = manager.refresh(effective).await?;
        let failed = *failure.lock().expect("registry discovery failure poisoned");
        let status = if failed {
            if result.models.is_empty() {
                ProviderDiscoveryStatus::Unavailable
            } else {
                ProviderDiscoveryStatus::Cached
            }
        } else if effective == ModelRefreshStrategy::Offline {
            if cached.is_some() { ProviderDiscoveryStatus::Cached } else { ProviderDiscoveryStatus::Idle }
        } else if result.models.is_empty() {
            ProviderDiscoveryStatus::Empty
        } else {
            ProviderDiscoveryStatus::Ok
        };
        self.update_state(
            Some(&config),
            ProviderDiscoveryState {
                provider: config.provider.clone(),
                status,
                optional: config.optional,
                stale: result.stale
                    || status == ProviderDiscoveryStatus::Cached
                    || (older || bypass) && status != ProviderDiscoveryStatus::Ok,
                fetched_at: if failed { cached.map(|c| c.updated_at) } else { Some(self.host.clock.now()) },
                source: None,
                models: result.models.iter().filter_map(|m| text(m, "id")).collect(),
            },
        );
        let models = result.models.iter().map(host_model_from_spec).collect::<Result<Vec<_>, _>>()?;
        let models = normalize_discoverable(&config, models, self.host.factory.environment.as_ref())
            .map_err(|_| error("discovery normalization failed"))?;
        Ok(generation.engine.prepare_cached_discovery_models(&config.provider, models)?)
    }
    async fn discover_builtin(
        &self,
        strategy: ModelRefreshStrategy,
        filter: Option<OrderedProviderSet>,
    ) -> RegistryRuntimeResult<BuiltInDiscoveryResult> {
        let generation = self.generation();
        let configured: HashSet<_> = generation.discoveries.iter().map(|c| c.provider.clone()).collect();
        let runtime = self.extensions.runtime_provider_ids();
        let selected = |p: &WireString| {
            !self.options.disabled_providers.contains(p)
                && !configured.contains(p)
                && filter.as_ref().is_none_or(|f| f.contains(p))
        };
        let descriptors: Vec<_> = provider_descriptors()
            .into_iter()
            .filter(|d| selected(&d.provider_id) && !runtime.contains(&d.provider_id))
            .collect();
        let keys = try_join_all(descriptors.iter().map(|d| {
            self.resolve_builtin_key(
                &generation,
                &d.provider_id,
                strategy,
                d.dynamic_models_authoritative.unwrap_or(false),
            )
        }))
        .await?;
        let mut options = Vec::new();
        let shared = models_dev_catalog_provider_ids();
        for (descriptor, key) in descriptors.into_iter().zip(keys) {
            let provider = &descriptor.provider_id;
            let authenticated = key.as_ref().is_some_and(is_authenticated);
            let explicit_vllm = provider.equals_ascii("vllm")
                && (generation.engine.provider_override(provider).is_some()
                    || generation.engine.is_keyless_provider(provider));
            if !authenticated
                && !descriptor.allow_unauthenticated.unwrap_or(false)
                && !explicit_vllm
                && !(shared.contains(provider) && !descriptor.dynamic_models_authoritative.unwrap_or(false))
            {
                continue;
            }
            let mut option = descriptor.create_model_manager_options.create(
                &self.host.context,
                ModelManagerConfig {
                    api_key: key.filter(is_discovery_bearer),
                    authenticated,
                    base_url: generation.engine.provider_override(provider).and_then(|p| text(p.fields(), "baseUrl")),
                    context: Some(self.host.context.clone()),
                },
                &self.host.factory,
            )?;
            option.models_dev = if let Some(fallback) = option.models_dev {
                Some(Arc::new(AdditiveFallback(fallback)))
            } else {
                models_dev_catalog_fallback(provider, &self.host.context, &self.host.factory, false, None)
            };
            options.push(option);
        }
        let built_in: HashSet<_> = provider_descriptors().into_iter().map(|d| d.provider_id).collect();
        for provider in shared {
            if !selected(&provider) || built_in.contains(&provider) || runtime.contains(&provider) {
                continue;
            }
            let Some(fallback) =
                models_dev_catalog_fallback(&provider, &self.host.context, &self.host.factory, false, None)
            else {
                continue;
            };
            let mut option = ModelManagerOptions::new(provider.clone());
            option.cache_provider_id = Some(resolve_model_cache_provider_id(
                &provider,
                None,
                generation.engine.provider_override(&provider).and_then(|p| text(p.fields(), "baseUrl")).as_ref(),
                self.host.factory.environment.as_ref(),
            ));
            option.models_dev = Some(fallback);
            options.push(option);
        }
        let codex = WireString::from("openai-codex");
        if selected(&codex) {
            let key = self.resolve_builtin_key(&generation, &codex, strategy, true).await?;
            if let Some(resolved_access_token) = key.filter(is_authenticated) {
                let suppress_stored_oauth = generation.engine.configured_api_key(&codex).is_some()
                    || self.host.extension_bindings.raw_key(&codex).is_some();
                options.push(openai_codex_model_manager_options(
                    &self.host.context,
                    OpenAiCodexModelManagerConfig {
                        resolve_accounts: Some(Arc::new(RegistryCodexAccounts {
                            credentials: self.host.credentials.clone(),
                            resolved_access_token,
                            suppress_stored_oauth,
                        })),
                        context: Some(self.host.context.clone()),
                        ..Default::default()
                    },
                    self.host.factory.clone(),
                ));
            }
        }
        for registration in self.extensions.runtime_manager_options() {
            if configured.contains(&registration.provider)
                || filter.as_ref().is_some_and(|filter| !filter.contains(&registration.provider))
            {
                continue;
            }
            let mut option = registration.options.clone();
            option.dynamic_fetcher = Some(Arc::new(RuntimeFetcher { backend: self.self_ref.clone(), registration }));
            options.push(option);
        }
        let results = futures::future::join_all(options.into_iter().map(|options| async {
            let provider = options.provider_id.clone();
            let authoritative = options.dynamic_models_authoritative;
            let manager = ModelManager::new(
                options,
                self.host.context.clone(),
                Arc::new(ModelManagerHost { cache: self.host.cache.clone(), clock: self.host.clock.clone() }),
            );
            // Promise.race leaves the native producer alive when its timer wins.
            let worker = tokio::spawn(async move { manager.refresh(strategy).await });
            let result = tokio::time::timeout(std::time::Duration::from_secs(15), worker)
                .await
                .map_err(|_| error("model manager discovery timed out"))?
                .map_err(|_| error("model manager discovery did not settle"))??;
            let models = result
                .models
                .iter()
                .map(|model| {
                    let mut spec = model.as_ref().clone();
                    spec.set("provider", WireValue::String(provider.clone()));
                    host_model_from_spec(&Arc::new(spec))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok::<_, DiscoveryError>((provider, models, authoritative && !result.stale))
        }))
        .await;
        let mut out = BuiltInDiscoveryResult::default();
        for result in results {
            match result {
                Ok((provider, models, authoritative)) => {
                    out.models.extend(models);
                    if authoritative {
                        out.authoritative_providers.insert(provider);
                    }
                }
                Err(_) => self.host.factory.warn(
                    "model discovery failed for provider",
                    VariantSpec::from_json(&serde_json::json!({"stage":"manager"})),
                ),
            }
        }
        Ok(out)
    }
    async fn publish(&self, publication: RegistryDiscoveryPublication<Self::Config>) -> RegistryRuntimeResult<()> {
        let mut state = self.state.lock().expect("registry state poisoned");
        let mut models = Vec::new();
        for result in publication.configured_results {
            if state.generation.discoveries.iter().any(|config| Arc::ptr_eq(config, &result.config)) {
                models.extend(result.models);
            }
        }
        let metric_specs: Vec<_> = publication.built_in.models.iter().map(|m| m.spec().clone()).collect();
        models.extend(publication.built_in.models);
        let authority = publication.built_in.authoritative_providers;
        let mut touched = OrderedProviderSet::default();
        for model in &models {
            if let Some(provider) = text(model.spec(), "provider") {
                touched.insert(provider);
            }
        }
        for provider in authority.iter() {
            touched.insert(provider.clone());
        }
        let generation = state.generation.clone();
        state.discovered.retain(|m| text(m.spec(), "provider").is_none_or(|provider| !touched.contains(&provider)));
        state.discovered.extend(models.iter().cloned());
        let mut combined = OrderedProviderSet::default();
        for provider in state.authority.iter() {
            if !touched.contains(provider) {
                combined.insert(provider.clone());
            }
        }
        for provider in authority.iter() {
            combined.insert(provider.clone());
        }
        state.authority = combined;
        drop(state);
        generation.engine.publish_discovery(
            models,
            authority,
            &touched,
            &metric_specs,
            publication.replace_catalog_metrics,
        )?;
        Ok(())
    }
    async fn prepare_policy_reapply(&self) -> RegistryRuntimeResult<()> {
        self.state.lock().expect("registry state poisoned").force_reload = true;
        Ok(())
    }
    fn report_error(&self, _operation: RegistryRuntimeOperation, _error: &DiscoveryError) {
        self.host.factory.warn(
            "model registry background operation failed",
            VariantSpec::from_json(&serde_json::json!({"stage":"registry"})),
        );
    }
}
