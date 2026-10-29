//! Production Host binding of the fixed OMP model registry (596f2da).
//!
//! The Host owns paths, credentials, fetch, clocks and process configuration.
//! Loader and discovery callbacks run outside the catalog publication mutex.
//! MIT License; Copyright (c) 2025 Mario Zechner;
//! Copyright (c) 2025-2026 Can Bölük; Copyright (c) 2026 Stencil Labs, Inc.

#[path = "model_registry_selected.rs"]
mod selected;

use crate::{
    auth_storage::{AuthRequestContext, AuthSessionScope, AuthStorage},
    catalog_discovery::{CatalogContext, DiscoveryError, DiscoveryReply, DiscoveryRequest},
    custom_models::{CustomModelBuildOptions, CustomModelOverlay, build_custom_model_overlay, finalize_custom_model},
    model_cache::SqliteModelCache,
    model_collapse::{CollapseError, VariantSpec},
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
        ModelCatalogProjection, StaticModelRegistry, StaticRegistryInputs, bundled_host_models,
        host_model_from_built_model, host_model_from_spec,
    },
};
use ara_rpc::{WireString, WireValue};
use async_trait::async_trait;
use futures::future::try_join_all;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

/// Authentication is a Host service, not authority inferred from catalog rows.
#[async_trait]
pub trait RegistryCredentials: Send + Sync {
    fn has_auth(&self, provider: &WireString) -> bool;
    /// Default hosts expose concrete keys only; ambient transports override this.
    fn has_concrete_auth(&self, provider: &WireString) -> bool {
        self.has_auth(provider)
    }
    fn peek_runtime_key(&self, _provider: &WireString) -> RegistryRuntimeResult<Option<WireString>> {
        Ok(None)
    }
    async fn peek_key(&self, provider: &WireString) -> RegistryRuntimeResult<Option<WireString>>;
    async fn resolve_key(&self, provider: &WireString) -> RegistryRuntimeResult<Option<WireString>> {
        self.peek_key(provider).await
    }
    async fn resolve_key_with_context(
        &self,
        provider: &WireString,
        _context: &AuthRequestContext,
        cancel: &CancellationToken,
    ) -> RegistryRuntimeResult<Option<WireString>> {
        if cancel.is_cancelled() {
            return Err(error("model authentication cancelled"));
        }
        let key = self.resolve_key(provider).await?;
        if cancel.is_cancelled() {
            return Err(error("model authentication cancelled"));
        }
        Ok(key)
    }
    async fn refresh_key(&self, _provider: &WireString) -> RegistryRuntimeResult<Option<WireString>> {
        Ok(None)
    }
    async fn refresh_rejected_key(
        &self,
        provider: &WireString,
        _rejected: &WireString,
    ) -> RegistryRuntimeResult<Option<WireString>> {
        self.refresh_key(provider).await
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
    fn has_concrete_auth(&self, provider: &WireString) -> bool {
        self.key(provider).is_some_and(|key| !key.equals_ascii("<authenticated>"))
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
    fn has_concrete_auth(&self, provider: &WireString) -> bool {
        (provider.equals_ascii("openai-codex") && self.auth.stored_oauth_snapshot().is_ok_and(|rows| !rows.is_empty()))
            || self.environment.has_concrete_auth(provider)
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
    fn has_auth(&self, provider: &WireString) -> bool {
        // Native AuthStorage.hasAuth observes the current configuration
        // override alongside stored/dedicated environment credentials. This
        // presence query must not resolve helpers or refresh accounts.
        let configured = self
            .extension_bindings
            .resolved_keys
            .lock()
            .expect("registry installed keys poisoned")
            .contains_key(provider);
        configured || self.credentials.has_auth(provider)
    }

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
    // Only Session key operations use this facade. Discovery/cache/backend stay shared.
    session_auth: Option<(AuthStorage, Arc<AuthSessionScope>)>,
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
    caller_cancel: Option<&CancellationToken>,
) -> RegistryRuntimeResult<Option<WireString>> {
    if let Some(raw) = host
        .extension_bindings
        .raw_key(provider)
        .or_else(|| generation.engine.configured_api_key(provider).map(str::to_owned))
    {
        let request_cancel = caller_cancel.cloned();
        let host_cancel = host.cancel.clone();
        let key = tokio::task::spawn_blocking(move || {
            let context =
                ConfigValueContext { project_dir: &host.project_dir, environment: host.config_environment.as_ref() };
            host.config_values
                .resolve_config_value_checked(&raw, &context, ResolveConfigValueOptions::default(), &|| {
                    !host.cancel.is_cancelled() && request_cancel.as_ref().is_none_or(|cancel| !cancel.is_cancelled())
                })
                .map_err(|_| error("discovery credential resolution cancelled"))
                .map(|key| key.filter(|key| !key.is_empty()).map(Into::into))
        })
        .await
        .map_err(|_| error("discovery credential resolution did not settle"))?;
        if caller_cancel.is_some_and(|cancel| cancel.is_cancelled() || host_cancel.is_cancelled()) {
            return Err(error("model authentication cancelled"));
        }
        return key;
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
    let has_auth = |provider: &WireString| host.has_auth(provider);
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
    let bundled_models = bundled_host_models()?.to_vec();
    #[cfg(feature = "test-fixture")]
    let bundled_models = {
        let mut models = bundled_models;
        if let Some(path) = std::env::var_os("ARA_TEST_PEER_MATERIALIZED_CATALOG") {
            // A materialized catalog donor is not a models.yml custom definition.
            // Only an explicit Peer fixture may supply its own private input file.
            if std::env::var_os("ARA_TEST_PEER_SCENARIO").is_none() {
                return Err(error("materialized Peer catalog requires an explicit fixture scenario"));
            }
            let root = std::env::var_os("ARA_TEST_CHILD_ARTIFACT_DIR")
                .ok_or_else(|| error("materialized Peer catalog requires a private artifact root"))?;
            let root = std::fs::canonicalize(root)
                .map_err(|_| error("materialized Peer catalog artifact root is unavailable"))?;
            let path = std::fs::canonicalize(path)
                .map_err(|_| error("materialized Peer catalog input is unavailable"))?;
            if path.parent() != Some(root.as_path())
                || path.file_name() != Some(std::ffi::OsStr::new("cold-materialized-model.json"))
            {
                return Err(error("materialized Peer catalog input must be inside its private fixture root"));
            }
            let bytes = std::fs::read(path)
                .map_err(|_| error("materialized Peer catalog input could not be read"))?;
            let spec: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|_| error("materialized Peer catalog input is not valid JSON"))?;
            models.push(crate::model_patch::HostModel::new(
                Arc::new(VariantSpec::from_json(&spec)), HeaderSlot::Absent,
            )?);
        }
        models
    };
    let engine = Arc::new(StaticModelRegistry::from_loader_inputs(
        config.as_ref(),
        StaticRegistryInputs {
            bundled_models,
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
    /// Replace an outer Session facade while retaining the exact global backend/runtime.
    pub fn with_session_scope(&self, storage: AuthStorage, scope: Arc<AuthSessionScope>) -> Self {
        Self { backend: self.backend.clone(), runtime: self.runtime.clone(), session_auth: Some((storage, scope)) }
    }

    #[cfg(feature="test-fixture")]
    pub fn session_scope_receipt(&self) -> serde_json::Value {
        serde_json::json!({"backend":Arc::as_ptr(&self.backend) as usize,
            "primarySessionId":self.session_auth.as_ref().map(|(_,scope)|scope.primary_session_id.clone())})
    }

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
        Ok(Self { runtime: ModelRegistryRuntime::new(backend.clone()), backend, session_auth: None })
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
    /// Fixed Native getAvailableForProviders. This only observes configured
    /// auth/keyless/disabled state; commands and OAuth refresh remain in actual
    /// request credential resolution. Static cache reads execute no helpers.
    pub fn get_available_for_providers(
        &self,
        providers: &OrderedProviderSet,
    ) -> RegistryRuntimeResult<Vec<HostModelRef>> {
        let generation = self.backend.generation();
        self.available_in_generation(&generation, providers)
    }
    pub fn get_available(&self) -> RegistryRuntimeResult<Vec<HostModelRef>> {
        let generation = self.backend.generation();
        let providers = generation.engine.known_provider_ids()?;
        self.available_in_generation(&generation, &providers)
    }
    /// Fixed Native getDiscoverableProviders: configured current-generation
    /// discoveries only, preserving order and excluding disabled providers.
    pub fn get_discoverable_providers(&self) -> Vec<WireString> {
        self.backend
            .generation()
            .discoveries
            .iter()
            .filter(|discovery| !self.backend.options.disabled_providers.contains(&discovery.provider))
            .map(|discovery| discovery.provider.clone())
            .collect()
    }
    pub fn is_keyless_provider(&self, provider: &WireString) -> bool {
        self.backend.generation().engine.is_keyless_provider(provider)
    }
    fn available_in_generation(
        &self,
        generation: &Arc<RegistryGeneration>,
        providers: &OrderedProviderSet,
    ) -> RegistryRuntimeResult<Vec<HostModelRef>> {
        let mut observations = HashMap::new();
        let requested: HashSet<_> = providers
            .iter()
            .map(|provider| crate::model_collapse::lower(&crate::model_collapse::trim(provider)))
            .filter(|provider| !provider.is_empty())
            .collect();
        // Observe the actual provider spelling, as Native does; requested-set
        // normalization must not rewrite credential/cache identity.
        for provider in generation.engine.known_provider_ids()?.iter() {
            if !requested.contains(&crate::model_collapse::lower(provider))
                || self.backend.options.disabled_providers.contains(provider)
            {
                continue;
            }
            let keyless = generation.engine.is_keyless_provider(provider);
            let authenticated = !keyless
                && *observations.entry(provider.clone()).or_insert_with(|| self.backend.host.has_auth(provider));
            if keyless || authenticated {
                self.backend.load_standard(generation, Some(provider))?;
            }
        }
        let has_auth = |provider: &WireString| {
            observations.get(provider).copied().unwrap_or_else(|| self.backend.host.has_auth(provider))
        };
        Ok(generation.engine.get_available_for_providers(providers, &has_auth)?)
    }
    /// Presence observation distinct from availability: a command-backed key
    /// counts as configured without executing it. It grants no request lease.
    pub fn has_configured_auth(&self, model: &HostModelRef) -> bool {
        let Some(provider) = text(model.spec(), "provider") else { return false };
        let generation = self.backend.generation();
        let runtime_key = self.backend.host.extension_bindings.raw_key(&provider);
        crate::model_config_values::is_command_config_value(
            runtime_key.as_deref().or_else(|| generation.engine.configured_api_key(&provider)),
        ) || generation.engine.is_keyless_provider(&provider)
            || self.backend.host.has_auth(&provider)
    }
    /// Fixed Native model-registry.ts:2303-2309. A command/keyless provider or
    /// concrete stored/environment key wins default selection over ambient auth.
    pub fn has_concrete_auth(&self, provider: &WireString) -> bool {
        let generation = self.backend.generation();
        let runtime_key = self.backend.host.extension_bindings.raw_key(provider);
        crate::model_config_values::is_command_config_value(
            runtime_key.as_deref().or_else(|| generation.engine.configured_api_key(provider)),
        ) || generation.engine.is_keyless_provider(provider)
            || self
                .backend
                .host
                .extension_bindings
                .resolved_keys
                .lock()
                .expect("registry installed keys poisoned")
                .contains_key(provider)
            || self.backend.host.credentials.has_concrete_auth(provider)
    }
    /// Fixed Native provider-level eligibility, without a model ranking context.
    pub async fn get_api_key_for_provider(
        &self,
        provider: &WireString,
        session_id: Option<&str>,
        cancel: &CancellationToken,
    ) -> RegistryRuntimeResult<Option<WireString>> {
        let context = AuthRequestContext { session_id: session_id.map(str::to_owned), ..Default::default() };
        self.get_api_key_with_context(provider, &context, cancel).await
    }

    /// Fixed Native model eligibility uses this exact model's URL and identity.
    pub async fn get_api_key(
        &self,
        model: &HostModelRef,
        session_id: Option<&str>,
        cancel: &CancellationToken,
    ) -> RegistryRuntimeResult<Option<WireString>> {
        let provider = text(model.spec(), "provider").ok_or_else(|| error("model has no authentication provider"))?;
        let context_text = |field| {
            text(model.spec(), field)
                .map(|value| value.to_utf8().map_err(|_| error("model authentication context is not UTF-8")))
                .transpose()
        };
        let context = AuthRequestContext {
            session_id: session_id.map(str::to_owned),
            model_id: context_text("id")?,
            base_url: context_text("baseUrl")?,
            ..Default::default()
        };
        self.get_api_key_with_context(&provider, &context, cancel).await
    }

    async fn get_api_key_with_context(
        &self,
        provider: &WireString,
        context: &AuthRequestContext,
        cancel: &CancellationToken,
    ) -> RegistryRuntimeResult<Option<WireString>> {
        let host = &self.backend.host;
        if cancel.is_cancelled() || host.cancel.is_cancelled() {
            return Err(error("model authentication cancelled"));
        }
        if let Some((storage, scope)) = &self.session_auth {
            let provider_text = provider.to_utf8().map_err(|_| error("model authentication provider is not UTF-8"))?;
            if scope.oauth_account_pools.contains_key(&provider_text) {
                // Mandatory before command/installed/config/keyless shortcuts. Caller context
                // cannot remove the captured scope or redirect a no-id call to global auth.
                let context = scope.context_for(&provider_text, context);
                let request_cancel = cancel.child_token();
                let resolution = storage.get_api_key(&provider_text, &context, &request_cancel);
                tokio::pin!(resolution);
                let key = tokio::select! {
                    biased;
                    _ = host.cancel.cancelled() => { request_cancel.cancel(); resolution.await }
                    key = &mut resolution => key,
                }.map_err(|_| error("scoped model authentication failed"))?;
                if cancel.is_cancelled() || host.cancel.is_cancelled() {
                    return Err(error("model authentication cancelled"));
                }
                return Ok(key.map(Into::into));
            }
        }
        let generation = self.backend.generation();
        let raw = host
            .extension_bindings
            .raw_key(provider)
            .or_else(|| generation.engine.configured_api_key(provider).map(str::to_owned));
        if crate::model_config_values::is_command_config_value(raw.as_deref()) {
            // A configured command owns the result even when it produces no key.
            // Await its settlement; cancelling this consumer must not replay it.
            let key = configured_key(host.clone(), &generation, provider, Some(cancel)).await?;
            let mut installed = host.extension_bindings.resolved_keys.lock().expect("registry installed keys poisoned");
            if cancel.is_cancelled() || host.cancel.is_cancelled() {
                return Err(error("model authentication cancelled"));
            }
            if let Some(key) = &key {
                installed.insert(provider.clone(), key.clone());
            } else {
                installed.remove(provider);
            }
            return Ok(key);
        }
        if let Some(key) = host.credentials.peek_runtime_key(provider)?.filter(|key| !key.is_empty()) {
            return Ok(Some(key));
        }
        let installed = host
            .extension_bindings
            .resolved_keys
            .lock()
            .expect("registry installed keys poisoned")
            .get(provider)
            .cloned()
            .or_else(|| generation.config_keys.get(provider).cloned())
            .filter(|key| !key.is_empty());
        if installed.is_some() {
            return Ok(installed);
        }
        if generation.engine.is_keyless_provider(provider) && !host.has_auth(provider) {
            return Ok(Some("N/A".into()));
        }
        let request_cancel = cancel.child_token();
        let resolution = host.credentials.resolve_key_with_context(provider, context, &request_cancel);
        tokio::pin!(resolution);
        let key = tokio::select! {
            biased;
            _ = host.cancel.cancelled() => {
                request_cancel.cancel();
                resolution.await
            }
            key = &mut resolution => key,
        }?;
        if cancel.is_cancelled() || host.cancel.is_cancelled() {
            return Err(error("model authentication cancelled"));
        }
        Ok(key)
    }
    /// Matching and availability share one Registry generation and the same
    /// module-lifetime learned alias runtime. No fresh default runtime is made.
    pub fn model_role_resolver(
        &self,
        preferences: crate::model_roles::ModelMatchPreferences,
    ) -> RegistryRuntimeResult<crate::model_roles::ModelRoleResolver> {
        let generation = self.backend.generation();
        let providers = generation.engine.known_provider_ids()?;
        let available = self.available_in_generation(&generation, &providers)?;
        Ok(crate::model_roles::ModelRoleResolver::new(available, generation.engine.collapse_runtime(), preferences)?)
    }
    pub fn models_for_provider(&self, provider: &WireString) -> RegistryRuntimeResult<Vec<HostModelRef>> {
        let generation = self.backend.generation();
        self.backend.load_standard(&generation, Some(provider))?;
        Ok(generation.engine.models_for_provider_lookup(provider)?)
    }

    /// Native model-registry.ts getProviderBaseUrl: first truthy provider URL,
    /// resolved from the current Registry generation rather than the call model.
    pub fn get_provider_base_url(&self, provider: &WireString) -> RegistryRuntimeResult<Option<WireString>> {
        Ok(self
            .models_for_provider(provider)?
            .into_iter()
            // The native warm full-snapshot lookup can return other providers.
            .filter(|model| text(model.spec(), "provider").as_ref() == Some(provider))
            .find_map(|model| text(model.spec(), "baseUrl").filter(|url| !url.is_empty())))
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
    /// Native explicit model selection clears only the chosen normalized
    /// selector, preserving all other provider/model cooldowns.
    pub fn clear_suppressed_selector(&self, selector: &WireString) -> RegistryRuntimeResult<()> {
        let models = self.selector_models(selector)?;
        let references = crate::provider_model_reference::ProviderModelReferenceIndex::new(&models)?;
        let live = |provider: &WireString, id: &WireString| {
            references.resolve(provider, id).is_ok_and(|model| model.is_some())
        };
        let collapse = self.backend.generation().engine.collapse_runtime();
        let mut runtime = collapse.lock().expect("registry collapse poisoned");
        self.backend.extensions.clear_suppressed_selector(selector, &mut runtime, Some(&live))?;
        Ok(())
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
        let bundled_by_provider: OnceLock<Result<HashMap<WireString, Vec<HostModelRef>>, CollapseError>> =
            OnceLock::new();
        let slices = loader
            .load_standard_providers(
                filter.as_ref(),
                &host,
                |provider| {
                    let base = generation.engine.provider_override(provider).and_then(|p| text(p.fields(), "baseUrl"));
                    let bundled = bundled_by_provider
                        .get_or_init(|| {
                            let mut grouped: HashMap<WireString, Vec<HostModelRef>> = HashMap::new();
                            for model in bundled_host_models()? {
                                if let Some(provider) = text(model.spec(), "provider") {
                                    grouped.entry(provider).or_default().push(model.clone());
                                }
                            }
                            Ok(grouped)
                        })
                        .as_ref()
                        .map_err(Clone::clone)?
                        .get(provider)
                        .cloned()
                        .unwrap_or_default();
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
            return configured_key(self.host.clone(), generation, provider, None).await;
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
            configured_key(self.host.clone(), &self.generation, provider, None).await?
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
            self.host.credentials.refresh_rejected_key(provider, key.as_ref().expect("rejected bearer checked")).await?
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
                    let spec = if model.get("provider").and_then(WireValue::as_string) == Some(&provider) {
                        Arc::clone(model)
                    } else {
                        let mut spec = model.as_ref().clone();
                        spec.set("provider", WireValue::String(provider.clone()));
                        Arc::new(spec)
                    };
                    host_model_from_built_model(&spec)
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

#[cfg(test)]
mod cold_scope_controls {
    use super::*;
    use crate::auth_storage::{AuthStorageError, AuthStorageOptions, OAuthProvider, ResolvedOAuth};
    use crate::auth_storage_registry::AuthStorageRegistryCredentials;
    use crate::catalog_discovery::DiscoveryTransport;
    use crate::credential_store::{AuthCredential, SqliteCredentialStore, StoredAuthCredential, resolve_credential_identity_key};
    use crate::model_config_values::{CommandConfigCache, ConfigCommandExecutor, ConfigCommandFailure, SystemConfigValueClock};
    use crate::model_registry_loader::startup_model_cache_provider_ids;
    use crate::model_route::{CredentialIdentity, RequestAuthLease};
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    const PROVIDER: &str = "cold-scope-fixture";
    const MODEL: &str = "scope-model";

    #[derive(Default)]
    struct Commands(AtomicUsize);
    impl ConfigCommandExecutor for Commands {
        fn directory_is_enterable(&self, _: &Path) -> bool { true }
        fn execute(&self, command: &str, _: &Path) -> Result<String, ConfigCommandFailure> {
            assert_eq!(command, "cold-scope-command");
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok("command-K".into())
        }
    }

    #[derive(Default)]
    struct NoDiscovery(AtomicUsize);
    #[async_trait]
    impl DiscoveryTransport for NoDiscovery {
        fn context_id(&self) -> u64 { self as *const Self as usize as u64 }
        async fn fetch(&self, _: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(error("scoped facade control unexpectedly dispatched discovery"))
        }
    }

    struct CountCredentials {
        actual: AuthStorageRegistryCredentials,
        runtime_peeks: AtomicUsize,
        resolves: AtomicUsize,
        presence: AtomicUsize,
    }
    #[async_trait]
    impl RegistryCredentials for CountCredentials {
        fn has_auth(&self, provider: &WireString) -> bool {
            self.presence.fetch_add(1, Ordering::SeqCst);
            self.actual.has_auth(provider)
        }
        fn has_concrete_auth(&self, provider: &WireString) -> bool { self.actual.has_concrete_auth(provider) }
        fn has_oauth_credentials(&self, provider: &WireString) -> bool { self.actual.has_oauth_credentials(provider) }
        fn oauth_credential(&self, provider: &WireString) -> Option<OpaqueExtensionHandle> { self.actual.oauth_credential(provider) }
        fn peek_runtime_key(&self, provider: &WireString) -> RegistryRuntimeResult<Option<WireString>> {
            self.runtime_peeks.fetch_add(1, Ordering::SeqCst);
            self.actual.peek_runtime_key(provider)
        }
        async fn peek_key(&self, provider: &WireString) -> RegistryRuntimeResult<Option<WireString>> {
            self.actual.peek_key(provider).await
        }
        async fn resolve_key_with_context(&self, provider: &WireString, context: &AuthRequestContext, cancel: &CancellationToken)
            -> RegistryRuntimeResult<Option<WireString>> {
            self.resolves.fetch_add(1, Ordering::SeqCst);
            self.actual.resolve_key_with_context(provider, context, cancel).await
        }
    }

    struct OAuth {
        store: Arc<Mutex<SqliteCredentialStore>>,
        replacement: Mutex<Option<String>>,
        refreshes: AtomicUsize,
    }
    #[async_trait]
    impl OAuthProvider for OAuth {
        fn lease(&self, row: &StoredAuthCredential) -> Result<RequestAuthLease, AuthStorageError> {
            let AuthCredential::OAuth { fields } = &row.credential else { return Err(AuthStorageError::Unavailable) };
            Ok(RequestAuthLease::new(CredentialIdentity::Stored { id: row.id, revision: row.revision },
                fields.get("access").and_then(serde_json::Value::as_str).map(str::to_owned)))
        }
        async fn resolve(&self, row: StoredAuthCredential, _: bool, _: &CancellationToken) -> Result<ResolvedOAuth, AuthStorageError> {
            self.refreshes.fetch_add(1, Ordering::SeqCst);
            let mut credential = row.credential.clone();
            if let AuthCredential::OAuth { fields } = &mut credential {
                let replacement = self.replacement.lock().unwrap().clone().unwrap_or_else(|| "A".into());
                fields.insert("accountId".into(), json!(replacement));
                fields.insert("email".into(), json!(format!("{replacement}@fixture")));
                fields.insert("access".into(), json!(format!("token-{replacement}")));
                fields.insert("expires".into(), json!(chrono::Utc::now().timestamp_millis() as f64 + 7_200_000.0));
            }
            let store = self.store.lock().unwrap();
            assert!(store.try_update_auth_credential_if_matches(row.id, &row.serialized_data, &credential, None).unwrap());
            let current = store.list_auth_credentials(Some(PROVIDER)).unwrap().into_iter().find(|current| current.id == row.id).unwrap();
            Ok(ResolvedOAuth { lease: self.lease(&current)?, row: current })
        }
    }

    fn oauth(account: &str, expired: bool) -> AuthCredential {
        AuthCredential::oauth(json!({"accountId":account,"email":format!("{account}@fixture"),
            "access":format!("token-{account}"),"refresh":"synthetic-refresh",
            "expires":chrono::Utc::now().timestamp_millis() as f64 + if expired {-1.0} else {7_200_000.0}})
            .as_object().unwrap().clone())
    }
    fn scope(primary: &str, accounts: &[&str]) -> Arc<AuthSessionScope> {
        let identities = accounts.iter().map(|account| resolve_credential_identity_key(PROVIDER, &oauth(account, false)).unwrap()).collect();
        AuthSessionScope::new(primary.into(), BTreeMap::from([(PROVIDER.into(), identities)]))
    }

    struct Fixture {
        _temporary: tempfile::TempDir,
        registry: ModelRegistry,
        model: HostModelRef,
        storage: AuthStorage,
        commands: Arc<Commands>,
        credentials: Arc<CountCredentials>,
        fetch: Arc<NoDiscovery>,
        oauth: Arc<OAuth>,
    }
    impl Fixture {
        async fn new(key: Option<&str>, expired_a: bool, include_b: bool) -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let store = Arc::new(Mutex::new(SqliteCredentialStore::open(temporary.path().join("auth.sqlite")).unwrap()));
            let mut rows = vec![oauth("A", expired_a)];
            if include_b { rows.push(oauth("B", false)); }
            rows.push(AuthCredential::api_key("stored-K"));
            store.lock().unwrap().replace_auth_credentials_for_provider(PROVIDER, &rows).unwrap();
            let storage = AuthStorage::new(store.clone(), None, AuthStorageOptions::default()).unwrap();
            let oauth = Arc::new(OAuth { store, replacement: Mutex::new(None), refreshes: AtomicUsize::new(0) });
            storage.register_oauth_provider(PROVIDER, oauth.clone()).unwrap();
            let credentials = Arc::new(CountCredentials { actual: AuthStorageRegistryCredentials::new(storage.clone(), CancellationToken::new()),
                runtime_peeks: AtomicUsize::new(0), resolves: AtomicUsize::new(0), presence: AtomicUsize::new(0) });
            let commands = Arc::new(Commands::default());
            let fetch = Arc::new(NoDiscovery::default());
            let environment: Arc<dyn ProviderEnvironment> = Arc::new(|_: &str| -> Option<WireString> { None });
            let host = ModelRegistryHost { project_dir: temporary.path().to_path_buf(),
                config_values: Arc::new(ConfigValueResolver::with_ports(Arc::new(CommandConfigCache::default()), Arc::new(SystemConfigValueClock), commands.clone())),
                config_environment: Arc::new(|_: &str| -> Option<String> { None }),
                factory: ProviderFactoryHost::new(environment, "ara", temporary.path().to_path_buf()),
                cache: Arc::new(Mutex::new(SqliteModelCache::for_path(temporary.path().join("models.sqlite")))),
                clock: Arc::new(|| chrono::Utc::now().timestamp_millis() as f64),
                context: CatalogContext::new(fetch.clone()).unwrap(), credentials: credentials.clone(),
                extension_bindings: Arc::new(RegistryExtensionBindings::default()), cancel: CancellationToken::new() };
            let mut provider = json!({"api":"openai-completions","baseUrl":"http://scope.invalid/v1","auth":"oauth","models":[{"id":MODEL}]});
            if let Some(key) = key { provider["apiKey"] = json!(key); }
            let config = json!({"providers":{"cold-scope-fixture":provider,"cold-keyless-control":{"api":"openai-completions", "baseUrl":"http://keyless.invalid/v1", "auth":"none", "models":[{"id":"keyless-model"}]}}});
            let path = temporary.path().join("models.json");
            std::fs::write(&path, config.to_string()).unwrap();
            let mut disabled = OrderedProviderSet::default();
            for provider in startup_model_cache_provider_ids() { disabled.insert(provider); }
            for provider in ["ollama", "llama.cpp", "lm-studio"] { disabled.insert(provider.into()); }
            let registry = ModelRegistry::open(ModelsConfigFile::new(path).unwrap(), RegistryLoadOptions {
                disabled_providers: disabled, ..Default::default()
            }, host).await.unwrap();
            let model = registry.find_exact(&PROVIDER.into(), &MODEL.into()).unwrap().unwrap();
            Self { _temporary: temporary, registry, model, storage, commands, credentials, fetch, oauth }
        }
        fn model(&self) -> HostModelRef { self.model.clone() }
        fn counters(&self) -> (usize, usize, usize, usize) {
            (self.commands.0.load(Ordering::SeqCst), self.credentials.runtime_peeks.load(Ordering::SeqCst),
                self.credentials.resolves.load(Ordering::SeqCst), self.credentials.presence.load(Ordering::SeqCst))
        }
        async fn both_entries(&self, registry: &ModelRegistry, expected: Option<&str>) {
            let model = self.model(); let cancel = CancellationToken::new();
            let mut key_escape_returns = 0;
            let mut disallowed_b_returns = 0;
            for id in [None, Some("explicit-side")] {
                for result in [registry.get_api_key(&model, id, &cancel).await.unwrap(),
                    registry.get_api_key_for_provider(&PROVIDER.into(), id, &cancel).await.unwrap()] {
                    let actual = result.as_ref().map(|value| value.to_utf8().unwrap());
                    if actual.as_deref().is_some_and(|value| value.ends_with("-K") || value == "N/A") { key_escape_returns += 1; }
                    if expected != Some("token-B") && actual.as_deref() == Some("token-B") { disallowed_b_returns += 1; }
                    assert_eq!(actual.as_deref(), expected);
                }
            }
            assert_eq!(key_escape_returns, 0);
            assert_eq!(disallowed_b_returns, 0);
        }
    }

    #[tokio::test]
    async fn scoped_model_and_provider_entries_reject_each_armed_key_escape() {
        for escape in ["command", "runtime", "installed", "config"] {
            let fixture = Fixture::new((escape == "config").then_some("config-K"), false, true).await;
            let registry = &fixture.registry; let provider: WireString = PROVIDER.into();
            let bindings = &registry.backend.host.extension_bindings;
            match escape {
                "command" => { bindings.raw_keys.lock().unwrap().insert(provider.clone(), "!cold-scope-command".into()); },
                "runtime" => fixture.storage.set_runtime_api_key(PROVIDER, "runtime-K".into()).unwrap(),
                "installed" => { bindings.resolved_keys.lock().unwrap().insert(provider.clone(), "installed-K".into()); },
                "config" => {
                    bindings.resolved_keys.lock().unwrap().remove(&provider);
                    assert_eq!(registry.backend.generation().config_keys.get(&provider), Some(&WireString::from("config-K")));
                },
                _ => unreachable!(),
            }
            let a = registry.with_session_scope(fixture.storage.clone(), scope("primary-A", &["A"]));
            let denied = registry.with_session_scope(fixture.storage.clone(), scope("primary-deny", &[]));
            let baseline = fixture.counters();
            fixture.both_entries(&a, Some("token-A")).await;
            fixture.both_entries(&denied, None).await;
            assert_eq!(fixture.counters(), baseline, "{escape}: scoped lookups never enter command/runtime/global credential legs");
            let expected = format!("{escape}-K");
            assert_eq!(registry.get_api_key(&fixture.model(), None, &CancellationToken::new()).await.unwrap(), Some(expected.clone().into()));
            assert_eq!(registry.get_api_key_for_provider(&provider, Some("root-control"), &CancellationToken::new()).await.unwrap(), Some(expected.into()));
            if escape == "command" { assert_eq!(fixture.commands.0.load(Ordering::SeqCst), baseline.0 + 1); }
            assert_eq!(fixture.fetch.0.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn scoped_keyless_denial_and_mandatory_context_cannot_be_weakened() {
        let fixture = Fixture::new(None, false, true).await;
        let captured = scope("captured-primary", &["A"]);
        let scoped = fixture.registry.with_session_scope(fixture.storage.clone(), captured.clone());
        let provider: WireString = PROVIDER.into();
        let hostile = scope("caller-primary", &["B"]);
        let before_context = fixture.counters();
        for caller_scope in [None, Some(hostile)] {
            for id in [None, Some("explicit-side")] {
                let context = AuthRequestContext { session_id: id.map(str::to_owned), session_scope: caller_scope.clone(),
                    environment_key: Some(crate::auth_storage::EnvironmentKey { variable: "SYNTHETIC_CALLER_KEY".into(), value: "caller-K".into() }),
                    ..Default::default() };
                assert_eq!(scoped.get_api_key_with_context(&provider, &context, &CancellationToken::new()).await.unwrap(), Some("token-A".into()));
            }
        }
        let deny_all = scoped.with_session_scope(fixture.storage.clone(), scope("deny-primary", &[]));
        let caller_allowed = captured.context_for(PROVIDER, &AuthRequestContext::default());
        assert!(deny_all.get_api_key_with_context(&provider, &caller_allowed, &CancellationToken::new()).await.unwrap().is_none());
        assert_eq!(fixture.counters(), before_context);
        for id in ["captured-primary", "explicit-side"] {
            let account = fixture.storage.get_oauth_account_identity(PROVIDER, Some(id)).unwrap().unwrap();
            assert_eq!(account.account_id.as_deref(), Some("A"));
        }
        let b = scoped.with_session_scope(fixture.storage.clone(), scope("nested-B", &["B"]));
        fixture.both_entries(&b, Some("token-B")).await;
        fixture.both_entries(&scoped, Some("token-A")).await;
        let keyless: WireString = "cold-keyless-control".into();
        let deny = fixture.registry.with_session_scope(fixture.storage.clone(), AuthSessionScope::new("keyless-primary".into(),
            BTreeMap::from([("cold-keyless-control".into(), BTreeSet::new())])));
        let keyless_model = fixture.registry.find_exact(&keyless, &"keyless-model".into()).unwrap().unwrap();
        assert!(fixture.registry.is_keyless_provider(&keyless));
        let before = fixture.counters();
        for id in [None, Some("keyless-side")] {
            assert!(deny.get_api_key(&keyless_model, id, &CancellationToken::new()).await.unwrap().is_none());
            assert!(deny.get_api_key_for_provider(&keyless, id, &CancellationToken::new()).await.unwrap().is_none());
        }
        assert_eq!(fixture.counters(), before);
        assert_eq!(fixture.registry.get_api_key(&keyless_model, None, &CancellationToken::new()).await.unwrap(), Some("N/A".into()));
        assert_eq!(fixture.registry.get_api_key_for_provider(&keyless, None, &CancellationToken::new()).await.unwrap(), Some("N/A".into()));
        assert!(fixture.credentials.presence.load(Ordering::SeqCst) > before.3);
        assert_eq!(fixture.fetch.0.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn scoped_facade_preserves_shared_storage_backend_cache_and_runtime_settlement() {
        let fixture = Fixture::new(None, false, true).await;
        let a = fixture.registry.with_session_scope(fixture.storage.clone(), scope("primary-A", &["A"]));
        let b = a.with_session_scope(fixture.storage.clone(), scope("primary-B", &["B"]));
        assert!(Arc::ptr_eq(&fixture.registry.backend, &a.backend));
        assert!(Arc::ptr_eq(&a.backend, &b.backend));
        assert!(Arc::ptr_eq(&fixture.registry.backend.host.cache, &b.backend.host.cache));
        a.session_auth.as_ref().unwrap().0.set_runtime_api_key("unlisted-shared-owner", "shared-mutation-K".into()).unwrap();
        assert_eq!(fixture.storage.runtime_api_key("unlisted-shared-owner").unwrap().as_deref(), Some("shared-mutation-K"));
        assert_eq!(b.session_auth.as_ref().unwrap().0.runtime_api_key("unlisted-shared-owner").unwrap().as_deref(), Some("shared-mutation-K"));
        let rows = fixture.storage.stored_oauth_snapshot(PROVIDER).unwrap();
        let b_id = rows.iter().find(|row| resolve_credential_identity_key(PROVIDER, &row.credential) == resolve_credential_identity_key(PROVIDER, &oauth("B", false))).unwrap().id;
        fixture.storage.pin_session_oauth_account(PROVIDER, "root-B", b_id, None).unwrap();
        assert_eq!(fixture.registry.get_api_key_for_provider(&PROVIDER.into(), Some("root-B"), &CancellationToken::new()).await.unwrap(), Some("token-B".into()));
        fixture.both_entries(&a, Some("token-A")).await;
        fixture.both_entries(&b, Some("token-B")).await;
        fixture.registry.runtime.refresh_in_background(ModelRefreshStrategy::Offline);
        fixture.registry.runtime.await_background_refresh().await;
        // A newly constructed runtime has no initial-settlement receipt. This
        // observes the original runtime's state through both Session clones.
        tokio::time::timeout(Duration::from_secs(2), a.runtime.await_initial_background_refresh(None)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), b.runtime.await_initial_background_refresh(None)).await.unwrap();
        assert_eq!(fixture.fetch.0.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn scoped_registry_returns_none_after_refresh_leaves_pool_without_key_fallback() {
        for replacement in ["A", "B"] {
            let fixture = Fixture::new(Some("config-K"), true, false).await;
            *fixture.oauth.replacement.lock().unwrap() = Some(replacement.into());
            let scoped = fixture.registry.with_session_scope(fixture.storage.clone(), scope("primary-A", &["A"]));
            let before = fixture.counters();
            fixture.both_entries(&scoped, (replacement == "A").then_some("token-A")).await;
            assert_eq!(fixture.oauth.refreshes.load(Ordering::SeqCst), 1);
            assert_eq!(fixture.counters(), before, "refresh denial must return from scoped branch, before installed/config key fallback");
            assert_eq!(fixture.registry.get_api_key_for_provider(&PROVIDER.into(), None, &CancellationToken::new()).await.unwrap(), Some("config-K".into()));
            let own = fixture.registry.with_session_scope(fixture.storage.clone(), scope("other-primary", &[replacement]));
            fixture.both_entries(&own, Some(if replacement == "A" { "token-A" } else { "token-B" })).await;
            assert_eq!(fixture.oauth.refreshes.load(Ordering::SeqCst), 1);
            assert_eq!(fixture.fetch.0.load(Ordering::SeqCst), 0);
        }
    }
}
