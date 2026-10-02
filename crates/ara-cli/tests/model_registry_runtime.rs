//! Grouped fixed OMP registry runtime contracts. The controlled backend exposes
//! fetch counts, publication identities and held failures; it is not production
//! loader/auth/provider acceptance.

use ara_cli::{
    catalog_discovery::DiscoveryError,
    model_collapse::VariantSpec,
    model_identity_wire::text,
    model_manager::ModelRefreshStrategy,
    model_patch::{HeaderSlot, HostModel, HostModelRef, OrderedProviderSet},
    model_registry_runtime::{
        BuiltInDiscoveryResult, ConfiguredDiscovery, ModelRegistryRuntime, RegistryDiscoveryPublication,
        RegistryRuntimeBackend, RegistryRuntimeOperation, RegistryRuntimeResult,
    },
};
use ara_rpc::WireString;
use async_trait::async_trait;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::Poll,
    time::Duration,
};
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;

fn providers(values: &[&str]) -> OrderedProviderSet {
    let mut providers = OrderedProviderSet::default();
    for value in values {
        providers.insert((*value).into());
    }
    providers
}
fn model(provider: &str, id: &str) -> HostModelRef {
    HostModel::new(
        Arc::new(VariantSpec::from_json(&serde_json::json!({"provider":provider,"id":id}))),
        HeaderSlot::Absent,
    )
    .unwrap()
}
struct Config {
    identity: usize,
    model: HostModelRef,
    gate: Semaphore,
}
fn discovery(provider: &str, identity: usize, id: &str) -> ConfiguredDiscovery<Config> {
    ConfiguredDiscovery {
        provider: provider.into(),
        config: Arc::new(Config { identity, model: model(provider, id), gate: Semaphore::new(0) }),
    }
}
struct Publication {
    configured: Vec<usize>,
    touched: Vec<WireString>,
    replace_metrics: bool,
}
type Callback = Arc<dyn Fn() + Send + Sync>;
struct Backend {
    configured: Mutex<Vec<ConfiguredDiscovery<Config>>>,
    disabled: Mutex<OrderedProviderSet>,
    runtime: Mutex<OrderedProviderSet>,
    credential_scoped: Mutex<OrderedProviderSet>,
    reloads: AtomicUsize,
    policy_prepares: AtomicUsize,
    suppression_clears: Mutex<Vec<Option<WireString>>>,
    configured_calls: Mutex<Vec<(usize, ModelRefreshStrategy)>>,
    builtin_calls: Mutex<Vec<(ModelRefreshStrategy, Option<OrderedProviderSet>)>>,
    publications: Mutex<Vec<Publication>>,
    catalog: Mutex<Vec<HostModelRef>>,
    builtin_result: Mutex<BuiltInDiscoveryResult>,
    failures: Mutex<Vec<RegistryRuntimeOperation>>,
    fail_configured: AtomicBool,
    fail_builtin: AtomicBool,
    hold_builtin: AtomicBool,
    builtin_gate: Semaphore,
    hold_publication: AtomicBool,
    publication_entries: AtomicUsize,
    publication_gate: Semaphore,
    changed: Notify,
    reenter: Mutex<Option<Callback>>,
}
impl Backend {
    fn new(configured: Vec<ConfiguredDiscovery<Config>>) -> Arc<Self> {
        Arc::new(Self {
            configured: Mutex::new(configured),
            disabled: Mutex::new(OrderedProviderSet::default()),
            runtime: Mutex::new(OrderedProviderSet::default()),
            credential_scoped: Mutex::new(OrderedProviderSet::default()),
            reloads: AtomicUsize::new(0),
            policy_prepares: AtomicUsize::new(0),
            suppression_clears: Mutex::new(Vec::new()),
            configured_calls: Mutex::new(Vec::new()),
            builtin_calls: Mutex::new(Vec::new()),
            publications: Mutex::new(Vec::new()),
            catalog: Mutex::new(Vec::new()),
            builtin_result: Mutex::new(BuiltInDiscoveryResult::default()),
            failures: Mutex::new(Vec::new()),
            fail_configured: AtomicBool::new(false),
            fail_builtin: AtomicBool::new(false),
            hold_builtin: AtomicBool::new(false),
            builtin_gate: Semaphore::new(0),
            hold_publication: AtomicBool::new(false),
            publication_entries: AtomicUsize::new(0),
            publication_gate: Semaphore::new(0),
            changed: Notify::new(),
            reenter: Mutex::new(None),
        })
    }
    async fn wait_for(&self, predicate: impl Fn() -> bool) {
        bounded(async {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if predicate() {
                    return;
                }
                changed.await;
            }
        })
        .await;
    }
    async fn configured_count(&self, count: usize) {
        self.wait_for(|| self.configured_calls.lock().unwrap().len() >= count).await;
    }
    async fn builtin_count(&self, count: usize) {
        self.wait_for(|| self.builtin_calls.lock().unwrap().len() >= count).await;
    }
    async fn publication_count(&self, count: usize) {
        self.wait_for(|| self.publications.lock().unwrap().len() >= count).await;
    }
    fn callback(&self) {
        let callback = self.reenter.lock().unwrap().clone();
        if let Some(callback) = callback {
            callback();
        }
    }
    fn catalog_ids(&self, provider: &str) -> Vec<WireString> {
        self.catalog
            .lock()
            .unwrap()
            .iter()
            .filter(|model| text(model.spec(), "provider").is_some_and(|value| value.equals_ascii(provider)))
            .filter_map(|model| text(model.spec(), "id"))
            .collect()
    }
}
#[async_trait]
impl RegistryRuntimeBackend for Backend {
    type Config = Config;

    async fn reload_static(&self) -> RegistryRuntimeResult<()> {
        self.reloads.fetch_add(1, Ordering::SeqCst);
        self.callback();
        Ok(())
    }
    fn clear_suppressed_selectors(&self, provider: Option<&WireString>) {
        self.suppression_clears.lock().unwrap().push(provider.cloned());
        self.callback();
    }
    fn configured_discoveries(&self) -> Vec<ConfiguredDiscovery<Config>> {
        self.configured.lock().unwrap().clone()
    }
    fn disabled_provider_ids(&self) -> OrderedProviderSet {
        self.disabled.lock().unwrap().clone()
    }
    fn runtime_provider_ids(&self) -> OrderedProviderSet {
        self.runtime.lock().unwrap().clone()
    }
    fn credential_scoped_provider_ids(&self) -> OrderedProviderSet {
        self.credential_scoped.lock().unwrap().clone()
    }
    async fn discover_configured(
        &self,
        config: Arc<Config>,
        strategy: ModelRefreshStrategy,
    ) -> RegistryRuntimeResult<Vec<HostModelRef>> {
        self.configured_calls.lock().unwrap().push((config.identity, strategy));
        self.changed.notify_waiters();
        config.gate.acquire().await.unwrap().forget();
        if self.fail_configured.swap(false, Ordering::SeqCst) {
            return Err(DiscoveryError::new("controlled configured failure"));
        }
        Ok(vec![config.model.clone()])
    }
    async fn discover_builtin(
        &self,
        strategy: ModelRefreshStrategy,
        filter: Option<OrderedProviderSet>,
    ) -> RegistryRuntimeResult<BuiltInDiscoveryResult> {
        self.builtin_calls.lock().unwrap().push((strategy, filter.clone()));
        self.changed.notify_waiters();
        if self.hold_builtin.load(Ordering::SeqCst) {
            self.builtin_gate.acquire().await.unwrap().forget();
        }
        if self.fail_builtin.swap(false, Ordering::SeqCst) {
            return Err(DiscoveryError::new("controlled built-in failure"));
        }
        let result = self.builtin_result.lock().unwrap();
        let mut authoritative_providers = OrderedProviderSet::default();
        for provider in result.authoritative_providers.iter() {
            if filter.as_ref().is_none_or(|filter| filter.contains(provider)) {
                authoritative_providers.insert(provider.clone());
            }
        }
        Ok(BuiltInDiscoveryResult {
            models: result
                .models
                .iter()
                .filter(|model| {
                    filter.as_ref().is_none_or(|filter| {
                        text(model.spec(), "provider").is_some_and(|provider| filter.contains(&provider))
                    })
                })
                .cloned()
                .collect(),
            authoritative_providers,
        })
    }
    async fn publish(&self, publication: RegistryDiscoveryPublication<Config>) -> RegistryRuntimeResult<()> {
        self.publication_entries.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_waiters();
        if self.hold_publication.swap(false, Ordering::SeqCst) {
            self.publication_gate.acquire().await.unwrap().forget();
        }
        // Actual production publication also owns an atomic membership check.
        let current = self.configured.lock().unwrap();
        let valid: Vec<_> = publication
            .configured_results
            .into_iter()
            .filter(|result| current.iter().any(|provider| Arc::ptr_eq(&provider.config, &result.config)))
            .collect();
        let mut touched = OrderedProviderSet::default();
        for model in valid.iter().flat_map(|result| &result.models).chain(&publication.built_in.models) {
            if let Some(provider) = text(model.spec(), "provider") {
                touched.insert(provider);
            }
        }
        for provider in publication.built_in.authoritative_providers.iter() {
            touched.insert(provider.clone());
        }
        let mut catalog = self.catalog.lock().unwrap();
        catalog.retain(|model| !text(model.spec(), "provider").is_some_and(|provider| touched.contains(&provider)));
        catalog.extend(valid.iter().flat_map(|result| result.models.iter().cloned()));
        catalog.extend(publication.built_in.models);
        self.publications.lock().unwrap().push(Publication {
            configured: valid.iter().map(|result| result.config.identity).collect(),
            touched: touched.iter().cloned().collect(),
            replace_metrics: publication.replace_catalog_metrics,
        });
        self.changed.notify_waiters();
        Ok(())
    }
    async fn prepare_policy_reapply(&self) -> RegistryRuntimeResult<()> {
        self.policy_prepares.fetch_add(1, Ordering::SeqCst);
        self.callback();
        Ok(())
    }
    fn report_error(&self, operation: RegistryRuntimeOperation, _: &DiscoveryError) {
        self.failures.lock().unwrap().push(operation);
        self.callback();
    }
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future).await.expect("controlled registry operation hung")
}
async fn prime<F: Future>(mut future: Pin<&mut F>) {
    futures::future::poll_fn(|context| match future.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(_) => panic!("held registry operation completed prematurely"),
    })
    .await;
}

#[tokio::test]
async fn configured_refreshes_share_identity_and_strategy_survive_drop_and_retry_after_failure() {
    let original = discovery("provider", 1, "original");
    let original_config = original.config.clone();
    let backend = Backend::new(vec![original]);
    let runtime = ModelRegistryRuntime::new(backend.clone());
    let mut full = Box::pin(runtime.refresh(ModelRefreshStrategy::Online));
    prime(full.as_mut()).await;
    backend.configured_count(1).await;
    let mut scoped =
        Box::pin(runtime.refresh_discoverable_providers(providers(&["provider"]), ModelRefreshStrategy::Online));
    prime(scoped.as_mut()).await;
    backend.builtin_count(2).await;
    assert_eq!(backend.configured_calls.lock().unwrap().len(), 1);
    original_config.gate.add_permits(1);
    bounded(full).await.unwrap();
    bounded(scoped).await.unwrap();
    assert_eq!(backend.reloads.load(Ordering::SeqCst), 1);
    assert_eq!(*backend.suppression_clears.lock().unwrap(), vec![None]);
    assert_eq!(backend.publications.lock().unwrap().iter().filter(|row| row.replace_metrics).count(), 1);

    let mut online =
        Box::pin(runtime.refresh_discoverable_providers(providers(&["provider"]), ModelRefreshStrategy::Online));
    let mut offline =
        Box::pin(runtime.refresh_discoverable_providers(providers(&["provider"]), ModelRefreshStrategy::Offline));
    prime(online.as_mut()).await;
    prime(offline.as_mut()).await;
    backend.configured_count(3).await;
    let strategies: Vec<_> = backend.configured_calls.lock().unwrap()[1..].iter().map(|row| row.1).collect();
    assert!(strategies.contains(&ModelRefreshStrategy::Online));
    assert!(strategies.contains(&ModelRefreshStrategy::Offline));
    original_config.gate.add_permits(2);
    bounded(online).await.unwrap();
    bounded(offline).await.unwrap();

    let mut stale =
        Box::pin(runtime.refresh_discoverable_providers(providers(&["provider"]), ModelRefreshStrategy::Online));
    prime(stale.as_mut()).await;
    backend.configured_count(4).await;
    let replacement_registration = discovery("provider", 2, "replacement");
    let replacement_config = replacement_registration.config.clone();
    *backend.configured.lock().unwrap() = vec![replacement_registration];
    let mut replacement =
        Box::pin(runtime.refresh_discoverable_providers(providers(&["provider"]), ModelRefreshStrategy::Online));
    prime(replacement.as_mut()).await;
    backend.configured_count(5).await;
    drop(stale);
    original_config.gate.add_permits(1);
    backend.publication_count(5).await;
    assert!(backend.publications.lock().unwrap()[4].configured.is_empty());
    assert!(backend.publications.lock().unwrap()[4].touched.is_empty());
    replacement_config.gate.add_permits(1);
    bounded(replacement).await.unwrap();
    assert_eq!(backend.catalog_ids("provider"), vec![WireString::from("replacement")]);
    assert_eq!(backend.publications.lock().unwrap().last().unwrap().configured, vec![2]);

    backend.fail_configured.store(true, Ordering::SeqCst);
    let mut failed =
        Box::pin(runtime.refresh_discoverable_providers(providers(&["provider"]), ModelRefreshStrategy::Online));
    prime(failed.as_mut()).await;
    backend.configured_count(6).await;
    replacement_config.gate.add_permits(1);
    assert!(bounded(failed).await.is_err());
    let mut retry =
        Box::pin(runtime.refresh_discoverable_providers(providers(&["provider"]), ModelRefreshStrategy::Online));
    prime(retry.as_mut()).await;
    backend.configured_count(7).await;
    replacement_config.gate.add_permits(1);
    bounded(retry).await.unwrap();
    assert_eq!(backend.configured_calls.lock().unwrap().len(), 7);

    // A configuration can change after the coordinator's late check while the
    // production backend waits for its commit lock. This must not let stale
    // touched providers remove models already present in the catalog.
    backend.hold_publication.store(true, Ordering::SeqCst);
    let publications_before = backend.publication_entries.load(Ordering::SeqCst);
    let mut late_replaced =
        Box::pin(runtime.refresh_discoverable_providers(providers(&["provider"]), ModelRefreshStrategy::Online));
    prime(late_replaced.as_mut()).await;
    backend.configured_count(8).await;
    replacement_config.gate.add_permits(1);
    backend.wait_for(|| backend.publication_entries.load(Ordering::SeqCst) > publications_before).await;
    *backend.configured.lock().unwrap() = vec![discovery("provider", 3, "next")];
    backend.publication_gate.add_permits(1);
    bounded(late_replaced).await.unwrap();
    {
        let publications = backend.publications.lock().unwrap();
        assert!(publications.last().unwrap().configured.is_empty());
        assert!(publications.last().unwrap().touched.is_empty());
    }
    assert_eq!(backend.catalog_ids("provider"), vec![WireString::from("replacement")]);
}

#[tokio::test]
async fn scoped_refresh_finishes_while_unrelated_work_remains_and_provider_refresh_restores_runtime() {
    let target = discovery("target", 1, "target-live");
    let target_config = target.config.clone();
    let other = discovery("other", 2, "other-live");
    let other_config = other.config.clone();
    let backend = Backend::new(vec![target, other]);
    backend.catalog.lock().unwrap().push(model("other", "other-prior"));
    let runtime = ModelRegistryRuntime::new(backend.clone());
    let worker = runtime.clone();
    let full = tokio::spawn(async move { worker.refresh(ModelRefreshStrategy::Online).await });
    backend.configured_count(2).await;
    let mut scoped =
        Box::pin(runtime.refresh_discoverable_providers(providers(&["target"]), ModelRefreshStrategy::Online));
    prime(scoped.as_mut()).await;
    backend.builtin_count(2).await;
    target_config.gate.add_permits(1);
    bounded(scoped).await.unwrap();
    assert!(!full.is_finished());
    assert_eq!(backend.configured_calls.lock().unwrap().len(), 2);
    assert_eq!(backend.catalog_ids("other"), vec![WireString::from("other-prior")]);
    other_config.gate.add_permits(1);
    bounded(full).await.unwrap().unwrap();

    *backend.runtime.lock().unwrap() = providers(&["target", "runtime-a", "runtime-b"]);
    backend.catalog.lock().unwrap().push(model("runtime-b", "retired"));
    *backend.builtin_result.lock().unwrap() = BuiltInDiscoveryResult {
        models: vec![model("runtime-a", "runtime-live")],
        authoritative_providers: providers(&["runtime-b"]),
    };
    let mut selected = Box::pin(runtime.refresh_provider("target".into(), ModelRefreshStrategy::Online));
    prime(selected.as_mut()).await;
    backend.configured_count(3).await;
    target_config.gate.add_permits(1);
    bounded(selected).await.unwrap();
    let calls = backend.builtin_calls.lock().unwrap();
    assert_eq!(calls[calls.len() - 2].0, ModelRefreshStrategy::Online);
    assert_eq!(
        calls[calls.len() - 2].1.as_ref().unwrap().iter().cloned().collect::<Vec<_>>(),
        vec![WireString::from("target")]
    );
    assert_eq!(calls.last().unwrap().0, ModelRefreshStrategy::OnlineIfUncached);
    assert_eq!(
        calls.last().unwrap().1.as_ref().unwrap().iter().cloned().collect::<Vec<_>>(),
        vec![WireString::from("runtime-a"), WireString::from("runtime-b")]
    );
    assert_eq!(backend.catalog_ids("runtime-a"), vec![WireString::from("runtime-live")]);
    assert!(backend.catalog_ids("runtime-b").is_empty());
    assert_eq!(*backend.suppression_clears.lock().unwrap(), vec![None, Some("target".into())]);
}

#[tokio::test]
async fn background_settlement_latches_failure_and_cancellation_releases_only_its_waiter() {
    let backend = Backend::new(Vec::new());
    backend.hold_builtin.store(true, Ordering::SeqCst);
    backend.fail_builtin.store(true, Ordering::SeqCst);
    let runtime = ModelRegistryRuntime::new(backend.clone());
    bounded(runtime.await_background_refresh()).await;
    let mut initial = Box::pin(runtime.await_initial_background_refresh(None));
    prime(initial.as_mut()).await;
    let cancel = CancellationToken::new();
    let mut cancelled = Box::pin(runtime.await_initial_background_refresh(Some(cancel.clone())));
    prime(cancelled.as_mut()).await;

    let reentries = Arc::new(AtomicUsize::new(0));
    let observer = runtime.clone();
    let observed = reentries.clone();
    *backend.reenter.lock().unwrap() = Some(Arc::new(move || {
        observed.fetch_add(1, Ordering::SeqCst);
        observer.refresh_in_background(ModelRefreshStrategy::Offline);
    }));
    runtime.refresh_in_background(ModelRefreshStrategy::Online);
    runtime.refresh_in_background(ModelRefreshStrategy::Offline);
    runtime.refresh_in_background(ModelRefreshStrategy::OnlineIfUncached);
    backend.builtin_count(1).await;
    assert_eq!(backend.builtin_calls.lock().unwrap().len(), 1);
    assert!(reentries.load(Ordering::SeqCst) > 0, "backend reentry was not exercised");
    cancel.cancel();
    bounded(cancelled).await;
    backend.builtin_gate.add_permits(1);
    bounded(runtime.await_background_refresh()).await;
    bounded(initial).await;
    bounded(runtime.await_initial_background_refresh(None)).await;
    assert_eq!(*backend.failures.lock().unwrap(), vec![RegistryRuntimeOperation::BackgroundRefresh]);

    runtime.refresh_in_background(ModelRefreshStrategy::OnlineIfUncached);
    backend.builtin_count(2).await;
    backend.builtin_gate.add_permits(1);
    bounded(runtime.await_background_refresh()).await;
    assert_eq!(backend.builtin_calls.lock().unwrap().len(), 2);
    backend.reenter.lock().unwrap().take();
}

#[tokio::test]
async fn hydration_and_policy_reapply_coalesce_separately_offline_and_release_failed_flights() {
    let backend = Backend::new(Vec::new());
    backend.hold_builtin.store(true, Ordering::SeqCst);
    backend.fail_builtin.store(true, Ordering::SeqCst);
    *backend.credential_scoped.lock().unwrap() = providers(&["scoped"]);
    let runtime = ModelRegistryRuntime::new(backend.clone());
    let mut hydration = Box::pin(runtime.hydrate_credential_scoped_model_caches());
    let mut hydration_peer = Box::pin(runtime.hydrate_credential_scoped_model_caches());
    prime(hydration.as_mut()).await;
    prime(hydration_peer.as_mut()).await;
    backend.builtin_count(1).await;
    assert_eq!(backend.reloads.load(Ordering::SeqCst), 0);
    let mut policy = Box::pin(runtime.reapply_model_policies());
    let mut policy_peer = Box::pin(runtime.reapply_model_policies());
    prime(policy.as_mut()).await;
    prime(policy_peer.as_mut()).await;
    backend.builtin_count(2).await;
    assert_eq!(backend.policy_prepares.load(Ordering::SeqCst), 1);
    assert_eq!(backend.reloads.load(Ordering::SeqCst), 1);
    {
        let calls = backend.builtin_calls.lock().unwrap();
        assert!(calls.iter().all(|call| call.0 == ModelRefreshStrategy::Offline));
        assert_eq!(calls[0].1.as_ref().unwrap().iter().cloned().collect::<Vec<_>>(), vec![WireString::from("scoped")]);
        assert!(calls[1].1.is_none());
    }
    drop(hydration);
    backend.builtin_gate.add_permits(1);
    bounded(hydration_peer).await;
    backend.builtin_gate.add_permits(1);
    bounded(policy).await.unwrap();
    bounded(policy_peer).await.unwrap();
    assert_eq!(*backend.failures.lock().unwrap(), vec![RegistryRuntimeOperation::CredentialScopedHydration]);

    backend.fail_builtin.store(true, Ordering::SeqCst);
    let mut failed_policy = Box::pin(runtime.reapply_model_policies());
    prime(failed_policy.as_mut()).await;
    backend.builtin_count(3).await;
    backend.builtin_gate.add_permits(1);
    assert!(bounded(failed_policy).await.is_err());
    let mut retry = Box::pin(runtime.reapply_model_policies());
    prime(retry.as_mut()).await;
    backend.builtin_count(4).await;
    backend.builtin_gate.add_permits(1);
    bounded(retry).await.unwrap();
    let mut hydration_retry = Box::pin(runtime.hydrate_credential_scoped_model_caches());
    prime(hydration_retry.as_mut()).await;
    backend.builtin_count(5).await;
    backend.builtin_gate.add_permits(1);
    bounded(hydration_retry).await;
    assert_eq!(backend.policy_prepares.load(Ordering::SeqCst), 3);
    assert_eq!(backend.reloads.load(Ordering::SeqCst), 3);
    assert_eq!(backend.builtin_calls.lock().unwrap().len(), 5);
}
