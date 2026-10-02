//! Host-owned asynchronous orchestration from fixed OMP `model-registry.ts`.
//!
//! Source: 596f2da7101178214aa27a753529d15e6b7ad91d, lines 392–562,
//! 679–683 and 1436–1540. Discovery, credentials, static composition and
//! publication remain the production backend's responsibilities. This module
//! does not implement runtime extension registration or model modifiers.
//!
//! MIT License; Copyright (c) 2025 Mario Zechner;
//! Copyright (c) 2025-2026 Can Bölük; Copyright (c) 2026 Stencil Labs, Inc.
//! See LICENSE for the full license.

use crate::{
    catalog_discovery::DiscoveryError,
    model_identity_wire::text,
    model_manager::ModelRefreshStrategy,
    model_patch::{HostModelRef, OrderedProviderSet},
};
use ara_rpc::WireString;
use async_trait::async_trait;
use futures::{FutureExt, future::try_join_all};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub type RegistryRuntimeResult<T> = Result<T, DiscoveryError>;

/// One immutable configured-discovery registration. Reuse its Config Arc until
/// the configuration changes; provider names alone are not registration IDs.
/// Config and model values may own private material, so these types deliberately
/// have no Debug or Serialize implementation.
pub struct ConfiguredDiscovery<C> {
    pub provider: WireString,
    pub config: Arc<C>,
}
impl<C> Clone for ConfiguredDiscovery<C> {
    fn clone(&self) -> Self {
        Self { provider: self.provider.clone(), config: self.config.clone() }
    }
}

pub struct ConfiguredDiscoveryResult<C> {
    pub provider: WireString,
    pub config: Arc<C>,
    pub models: Vec<HostModelRef>,
}

#[derive(Clone, Default)]
pub struct BuiltInDiscoveryResult {
    pub models: Vec<HostModelRef>,
    pub authoritative_providers: OrderedProviderSet,
}

pub struct RegistryDiscoveryPublication<C> {
    pub configured_results: Vec<ConfiguredDiscoveryResult<C>>,
    pub built_in: BuiltInDiscoveryResult,
    pub touched_providers: OrderedProviderSet,
    /// Native captureCatalogMetrics uses replace for full discovery, merge for
    /// provider-scoped discovery, including a result with no touched providers.
    pub replace_catalog_metrics: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryRuntimeOperation {
    BackgroundRefresh,
    CredentialScopedHydration,
}

/// Production binding for the fixed loader, ModelManager and CatalogContext.
/// All callbacks run outside this coordinator's mutex. A synchronous callback
/// may reenter observation or start another refresh.
#[async_trait]
pub trait RegistryRuntimeBackend: Send + Sync + 'static {
    type Config: Send + Sync + 'static;

    async fn reload_static(&self) -> RegistryRuntimeResult<()>;
    fn clear_suppressed_selectors(&self, provider: Option<&WireString>);
    fn configured_discoveries(&self) -> Vec<ConfiguredDiscovery<Self::Config>>;
    fn disabled_provider_ids(&self) -> OrderedProviderSet;
    fn runtime_provider_ids(&self) -> OrderedProviderSet;
    fn credential_scoped_provider_ids(&self) -> OrderedProviderSet;

    async fn discover_configured(
        &self,
        config: Arc<Self::Config>,
        strategy: ModelRefreshStrategy,
    ) -> RegistryRuntimeResult<Vec<HostModelRef>>;

    /// Built-in managers retain their native provider-specific behavior; they
    /// do not acquire a registry-wide provider-name discovery flight.
    async fn discover_builtin(
        &self,
        strategy: ModelRefreshStrategy,
        provider_filter: Option<OrderedProviderSet>,
    ) -> RegistryRuntimeResult<BuiltInDiscoveryResult>;

    /// Recheck Config Arc membership atomically with the actual commit. The
    /// coordinator filters late results before this callback, but an async
    /// backend may yield before it acquires its own composition lock. Recompute
    /// touched providers from surviving configured models, built-in models and
    /// built-in authoritative providers after that check; the supplied set is
    /// the coordinator's earlier snapshot. Publish merges only those providers
    /// and retains all unrelated runtime layers.
    async fn publish(&self, publication: RegistryDiscoveryPublication<Self::Config>) -> RegistryRuntimeResult<()>;

    /// Force the next static reload past its mtime gate. The coordinator then
    /// performs the ordinary complete refresh with Offline strategy.
    async fn prepare_policy_reapply(&self) -> RegistryRuntimeResult<()>;

    /// Background refresh errors are warnings; hydration errors are debug-level
    /// best-effort failures. The Host owns sanitization and logging.
    fn report_error(&self, operation: RegistryRuntimeOperation, error: &DiscoveryError);
}

struct SharedFlight<T> {
    result: Mutex<Option<RegistryRuntimeResult<T>>>,
    notify: Notify,
}
impl<T: Clone> SharedFlight<T> {
    fn new() -> Self {
        Self { result: Mutex::new(None), notify: Notify::new() }
    }
    fn finish(&self, result: RegistryRuntimeResult<T>) {
        *self.result.lock().expect("registry flight result poisoned") = Some(result);
        self.notify.notify_waiters();
    }
    async fn wait(&self) -> RegistryRuntimeResult<T> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // Register before observing the result, so completion between the
            // observation and await cannot lose a wakeup.
            notified.as_mut().enable();
            let result = self.result.lock().expect("registry flight result poisoned").clone();
            if let Some(result) = result {
                return result;
            }
            notified.await;
        }
    }
}

async fn settle<T>(operation: impl Future<Output = RegistryRuntimeResult<T>>) -> RegistryRuntimeResult<T> {
    AssertUnwindSafe(operation)
        .catch_unwind()
        .await
        .unwrap_or_else(|_| Err(DiscoveryError::new("model registry backend panicked")))
}

/// The spawned producer retains its owner even if every waiting consumer is
/// dropped. A Rust callback panic is settled as an error rather than leaving a
/// permanently pending shared flight; the returned error carries no panic
/// payload. The process's configured panic hook remains in effect.
fn start_owned<T>(operation: impl Future<Output = RegistryRuntimeResult<T>> + Send + 'static) -> Arc<SharedFlight<T>>
where
    T: Clone + Send + 'static,
{
    let flight = Arc::new(SharedFlight::new());
    let producer = flight.clone();
    tokio::spawn(async move {
        producer.finish(settle(operation).await);
    });
    flight
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ConfiguredFlightKey {
    identity: usize,
    strategy: u8,
}
impl ConfiguredFlightKey {
    fn new<C>(config: &Arc<C>, strategy: ModelRefreshStrategy) -> Self {
        Self {
            identity: Arc::as_ptr(config) as usize,
            strategy: match strategy {
                ModelRefreshStrategy::Online => 0,
                ModelRefreshStrategy::Offline => 1,
                ModelRefreshStrategy::OnlineIfUncached => 2,
            },
        }
    }
}

struct ConfiguredFlight<C> {
    // Retain the identity while it is a map key, preventing address reuse.
    _config: Arc<C>,
    flight: Arc<SharedFlight<Vec<HostModelRef>>>,
}
#[derive(Clone, Copy)]
enum SharedOperation {
    Background,
    Hydration,
    Policy,
}
struct RuntimeState<C> {
    configured: HashMap<ConfiguredFlightKey, ConfiguredFlight<C>>,
    background: Option<Arc<SharedFlight<()>>>,
    hydration: Option<Arc<SharedFlight<()>>>,
    policy: Option<Arc<SharedFlight<()>>>,
    initial_refresh_settled: bool,
}
impl<C> RuntimeState<C> {
    fn new() -> Self {
        Self {
            configured: HashMap::new(),
            background: None,
            hydration: None,
            policy: None,
            initial_refresh_settled: false,
        }
    }
    fn slot(&mut self, operation: SharedOperation) -> &mut Option<Arc<SharedFlight<()>>> {
        match operation {
            SharedOperation::Background => &mut self.background,
            SharedOperation::Hydration => &mut self.hydration,
            SharedOperation::Policy => &mut self.policy,
        }
    }
}
struct RuntimeInner<B: RegistryRuntimeBackend> {
    backend: Arc<B>,
    state: Mutex<RuntimeState<B::Config>>,
    initial_refresh_notify: Notify,
}
pub struct ModelRegistryRuntime<B: RegistryRuntimeBackend> {
    inner: Arc<RuntimeInner<B>>,
}
impl<B: RegistryRuntimeBackend> Clone for ModelRegistryRuntime<B> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<B: RegistryRuntimeBackend> ModelRegistryRuntime<B> {
    /// The Host loads its synchronous initial layers before constructing this
    /// coordinator. Construction starts no discovery or credential work.
    pub fn new(backend: Arc<B>) -> Self {
        Self {
            inner: Arc::new(RuntimeInner {
                backend,
                state: Mutex::new(RuntimeState::new()),
                initial_refresh_notify: Notify::new(),
            }),
        }
    }

    fn configured_flight(
        &self,
        config: Arc<B::Config>,
        strategy: ModelRefreshStrategy,
    ) -> Arc<SharedFlight<Vec<HostModelRef>>> {
        let key = ConfiguredFlightKey::new(&config, strategy);
        let flight = {
            let mut state = self.inner.state.lock().expect("registry runtime state poisoned");
            if let Some(current) = state.configured.get(&key) {
                return current.flight.clone();
            }
            let flight = Arc::new(SharedFlight::new());
            state.configured.insert(key, ConfiguredFlight { _config: config.clone(), flight: flight.clone() });
            flight
        };
        let runtime = self.clone();
        let producer = flight.clone();
        tokio::spawn(async move {
            let result = settle(runtime.inner.backend.discover_configured(config, strategy)).await;
            {
                let mut state = runtime.inner.state.lock().expect("registry runtime state poisoned");
                if state.configured.get(&key).is_some_and(|current| Arc::ptr_eq(&current.flight, &producer)) {
                    state.configured.remove(&key);
                }
            }
            producer.finish(result);
        });
        flight
    }

    async fn refresh_runtime_discoveries(
        &self,
        strategy: ModelRefreshStrategy,
        provider_filter: Option<OrderedProviderSet>,
    ) -> RegistryRuntimeResult<()> {
        let backend = &self.inner.backend;
        let disabled = backend.disabled_provider_ids();
        let configured: Vec<_> = backend
            .configured_discoveries()
            .into_iter()
            .filter(|provider| provider_filter.as_ref().is_none_or(|filter| filter.contains(&provider.provider)))
            .filter(|provider| !disabled.contains(&provider.provider))
            .map(|provider| {
                let flight = self.configured_flight(provider.config.clone(), strategy);
                async move {
                    Ok::<_, DiscoveryError>(ConfiguredDiscoveryResult {
                        provider: provider.provider,
                        config: provider.config,
                        models: flight.wait().await?,
                    })
                }
            })
            .collect();
        // Like Promise.all, an error may settle this refresh before another
        // producer. Its work still owns its completion; dropping this waiter
        // does not cancel that work or manufacture a successful publication.
        let built_in_backend = backend.clone();
        let replace_catalog_metrics = provider_filter.is_none();
        let built_in = start_owned(async move { built_in_backend.discover_builtin(strategy, provider_filter).await });
        let (configured, built_in) = tokio::try_join!(try_join_all(configured), built_in.wait())?;
        let current: HashSet<_> = backend
            .configured_discoveries()
            .into_iter()
            .map(|provider| Arc::as_ptr(&provider.config) as usize)
            .collect();
        let configured_results: Vec<_> =
            configured.into_iter().filter(|result| current.contains(&(Arc::as_ptr(&result.config) as usize))).collect();
        let mut touched_providers = OrderedProviderSet::default();
        for model in configured_results.iter().flat_map(|result| &result.models).chain(&built_in.models) {
            if let Some(provider) = text(model.spec(), "provider") {
                touched_providers.insert(provider);
            }
        }
        for provider in built_in.authoritative_providers.iter() {
            touched_providers.insert(provider.clone());
        }
        backend
            .publish(RegistryDiscoveryPublication {
                configured_results,
                built_in,
                touched_providers,
                replace_catalog_metrics,
            })
            .await
    }

    async fn refresh_inner(&self, strategy: ModelRefreshStrategy) -> RegistryRuntimeResult<()> {
        self.inner.backend.reload_static().await?;
        self.inner.backend.clear_suppressed_selectors(None);
        self.refresh_runtime_discoveries(strategy, None).await
    }

    pub async fn refresh(&self, strategy: ModelRefreshStrategy) -> RegistryRuntimeResult<()> {
        let runtime = self.clone();
        start_owned(async move { runtime.refresh_inner(strategy).await }).wait().await
    }

    pub async fn refresh_provider(
        &self,
        provider: WireString,
        strategy: ModelRefreshStrategy,
    ) -> RegistryRuntimeResult<()> {
        let runtime = self.clone();
        start_owned(async move {
            runtime.inner.backend.reload_static().await?;
            runtime.inner.backend.clear_suppressed_selectors(Some(&provider));
            let mut selected = OrderedProviderSet::default();
            selected.insert(provider.clone());
            runtime.refresh_runtime_discoveries(strategy, Some(selected)).await?;
            let mut others = OrderedProviderSet::default();
            for candidate in runtime.inner.backend.runtime_provider_ids().iter() {
                if candidate != &provider {
                    others.insert(candidate.clone());
                }
            }
            if !others.is_empty() {
                runtime.refresh_runtime_discoveries(ModelRefreshStrategy::OnlineIfUncached, Some(others)).await?;
            }
            Ok(())
        })
        .wait()
        .await
    }

    /// No static reload, suppression clearing or discovery of unrelated runtime
    /// providers. An empty filter is an immediate no-op.
    pub async fn refresh_discoverable_providers(
        &self,
        provider_ids: OrderedProviderSet,
        strategy: ModelRefreshStrategy,
    ) -> RegistryRuntimeResult<()> {
        if provider_ids.is_empty() {
            return Ok(());
        }
        let runtime = self.clone();
        start_owned(async move { runtime.refresh_runtime_discoveries(strategy, Some(provider_ids)).await }).wait().await
    }

    pub async fn refresh_runtime_providers(&self, strategy: ModelRefreshStrategy) -> RegistryRuntimeResult<()> {
        let runtime = self.clone();
        start_owned(async move {
            let providers = runtime.inner.backend.runtime_provider_ids();
            if providers.is_empty() {
                return Ok(());
            }
            runtime.refresh_runtime_discoveries(strategy, Some(providers)).await
        })
        .wait()
        .await
    }

    fn start_shared<F, R>(&self, operation: SharedOperation, run: F) -> Arc<SharedFlight<()>>
    where
        F: FnOnce(Self) -> R + Send + 'static,
        R: Future<Output = RegistryRuntimeResult<()>> + Send + 'static,
    {
        let flight = {
            let mut state = self.inner.state.lock().expect("registry runtime state poisoned");
            if let Some(current) = state.slot(operation) {
                return current.clone();
            }
            let flight = Arc::new(SharedFlight::new());
            *state.slot(operation) = Some(flight.clone());
            flight
        };
        let runtime = self.clone();
        let producer = flight.clone();
        tokio::spawn(async move {
            let result = settle(async { run(runtime.clone()).await }).await;
            {
                let mut state = runtime.inner.state.lock().expect("registry runtime state poisoned");
                if state.slot(operation).as_ref().is_some_and(|current| Arc::ptr_eq(current, &producer)) {
                    *state.slot(operation) = None;
                }
                if matches!(operation, SharedOperation::Background) {
                    state.initial_refresh_settled = true;
                }
            }
            if matches!(operation, SharedOperation::Background) {
                runtime.inner.initial_refresh_notify.notify_waiters();
            }
            producer.finish(result);
        });
        flight
    }

    pub fn refresh_in_background(&self, strategy: ModelRefreshStrategy) {
        self.start_shared(SharedOperation::Background, move |runtime| async move {
            if let Err(error) = runtime.refresh_inner(strategy).await {
                runtime.inner.backend.report_error(RegistryRuntimeOperation::BackgroundRefresh, &error);
            }
            Ok(())
        });
    }

    pub async fn await_background_refresh(&self) {
        let flight = self.inner.state.lock().expect("registry runtime state poisoned").background.clone();
        if let Some(flight) = flight {
            let _ = flight.wait().await;
        }
    }

    /// A waiter armed before discovery starts observes its eventual settlement.
    /// Cancellation releases only this waiter, never the shared producer. When
    /// no initial refresh starts, an uncancelled waiter intentionally stays pending.
    pub async fn await_initial_background_refresh(&self, cancel: Option<CancellationToken>) {
        loop {
            let notified = self.inner.initial_refresh_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.inner.state.lock().expect("registry runtime state poisoned").initial_refresh_settled
                || cancel.as_ref().is_some_and(CancellationToken::is_cancelled)
            {
                return;
            }
            if let Some(cancel) = &cancel {
                tokio::select! { biased; _ = cancel.cancelled() => return, _ = notified => {} }
            } else {
                notified.await;
            }
        }
    }

    pub async fn hydrate_credential_scoped_model_caches(&self) {
        let flight = self.start_shared(SharedOperation::Hydration, |runtime| async move {
            let providers = runtime.inner.backend.credential_scoped_provider_ids();
            if let Err(error) =
                runtime.refresh_runtime_discoveries(ModelRefreshStrategy::Offline, Some(providers)).await
            {
                runtime.inner.backend.report_error(RegistryRuntimeOperation::CredentialScopedHydration, &error);
            }
            Ok(())
        });
        let _ = flight.wait().await;
    }

    pub async fn reapply_model_policies(&self) -> RegistryRuntimeResult<()> {
        self.start_shared(SharedOperation::Policy, |runtime| async move {
            runtime.inner.backend.prepare_policy_reapply().await?;
            runtime.refresh_inner(ModelRefreshStrategy::Offline).await
        })
        .wait()
        .await
    }
}
