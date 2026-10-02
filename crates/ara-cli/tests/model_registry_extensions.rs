//! One grouped fixed-OMP runtime registration/cleanup/projection contract.
//! All ports are controlled; registration never runs discovery or a process.

use ara_cli::{
    catalog_discovery::DiscoveryError,
    custom_models::CustomModelOverlay,
    model_collapse::{CollapseError, CollapseRuntime, VariantSpec},
    model_config_values::{
        CommandConfigCache, ConfigCommandExecutor, ConfigCommandFailure, ConfigValueClock, ConfigValueContext,
        ConfigValueResolver, HeaderConfigRecord, HeaderResolutionOptions, HeaderSource, ResolvedConfigHeaders,
        create_live_config_headers,
    },
    model_identity_wire::text,
    model_manager::RawModelValue,
    model_patch::{HeaderSlot, HostModel, HostModelRef, ModelPatch, ProviderOverride},
    model_registry_extensions::{
        ExtensionModifierHost, ExtensionRegistry, ExtensionRegistryHost, ExtensionResult, ModifierHeaders,
        ModifierModel, OpaqueExtensionHandle, RuntimeApiRegistration, RuntimeDefinitionFetcher, RuntimeModelModifier,
        RuntimeOAuthEntry, RuntimeOAuthRegistration, RuntimeProviderConfig,
    },
    model_registry_runtime::RegistryRuntimeResult,
};
use ara_rpc::{WireString, WireValue};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

fn config(fields: Value, key: Option<&str>) -> RuntimeProviderConfig {
    RuntimeProviderConfig::new(VariantSpec::from_json(&fields), HeaderSlot::Absent, key.map(str::to_owned)).unwrap()
}
fn definition(id: &str) -> ModelPatch {
    ModelPatch::new(VariantSpec::from_json(&json!({"id":id,"name":id})), HeaderSlot::Absent).unwrap()
}
fn handle(label: &str) -> OpaqueExtensionHandle {
    Arc::new(label.to_owned())
}
fn oauth(modifier: Option<Arc<dyn RuntimeModelModifier>>) -> RuntimeOAuthRegistration {
    RuntimeOAuthRegistration { handle: handle("controlled-oauth"), modifier }
}
fn identity_modifier() -> Arc<dyn RuntimeModelModifier> {
    Arc::new(|models: Vec<ModifierModel>, _: &OpaqueExtensionHandle| Ok(models))
}
fn plain(provider: &str, id: &str, headers: HeaderSlot) -> HostModelRef {
    HostModel::new(Arc::new(VariantSpec::from_json(&json!({"provider":provider,"id":id,"name":id}))), headers).unwrap()
}
fn register(
    registry: &ExtensionRegistry,
    host: &Port,
    provider: &str,
    value: &RuntimeProviderConfig,
    source: Option<&str>,
) -> ExtensionResult<()> {
    registry.register_provider(provider.into(), value, source.map(WireString::from), host)
}
fn overlay_ids(registry: &ExtensionRegistry) -> Vec<WireString> {
    registry.snapshot().model_overlays.iter().filter_map(|overlay| text(overlay.fields(), "id")).collect()
}

#[derive(Clone, Debug)]
struct Event {
    stage: String,
    owner: Option<WireString>,
    keys: usize,
    managers: usize,
    modifiers: usize,
    oauth: usize,
    retained_base: Option<WireString>,
}
struct Clock;
impl ConfigValueClock for Clock {
    fn now_millis(&self) -> u64 {
        1
    }
}
#[derive(Default)]
struct RejectExecutor(AtomicUsize);
impl ConfigCommandExecutor for RejectExecutor {
    fn directory_is_enterable(&self, _: &Path) -> bool {
        true
    }
    fn execute(&self, _: &str, _: &Path) -> Result<String, ConfigCommandFailure> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ConfigCommandFailure::NonZeroExit)
    }
}
struct Port {
    registry: Arc<ExtensionRegistry>,
    full: AtomicBool,
    failure: Mutex<Option<&'static str>>,
    events: Mutex<Vec<Event>>,
    usages: Mutex<Vec<WireString>>,
    credentials: Mutex<Vec<WireString>>,
    warnings: Mutex<Vec<(WireString, WireString)>>,
    materializations: AtomicUsize,
    executor: Arc<RejectExecutor>,
    resolver: ConfigValueResolver,
}
impl Port {
    fn new(registry: Arc<ExtensionRegistry>, full: bool) -> Self {
        let executor = Arc::new(RejectExecutor::default());
        Self {
            registry,
            full: AtomicBool::new(full),
            failure: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            usages: Mutex::new(Vec::new()),
            credentials: Mutex::new(Vec::new()),
            warnings: Mutex::new(Vec::new()),
            materializations: AtomicUsize::new(0),
            resolver: ConfigValueResolver::with_ports(
                Arc::new(CommandConfigCache::default()),
                Arc::new(Clock),
                executor.clone(),
            ),
            executor,
        }
    }
    // Every callback snapshots the same store, exercising the no-lock Host
    // boundary while recording native stage visibility, not just final state.
    fn record(&self, stage: impl Into<String>, provider: &WireString, retained: Option<&ProviderOverride>) {
        let snapshot = self.registry.snapshot();
        self.events.lock().unwrap().push(Event {
            stage: stage.into(),
            owner: snapshot
                .provider_sources
                .iter()
                .find(|(name, _)| name == provider)
                .map(|(_, source)| source.clone()),
            keys: snapshot.api_key_configs.len(),
            managers: snapshot.managers.len(),
            modifiers: snapshot.modifiers.len(),
            oauth: snapshot.custom_oauth.len(),
            retained_base: retained.and_then(|value| text(value.fields(), "baseUrl")),
        });
    }
    fn checkpoint(&self, stage: &'static str) -> ExtensionResult<()> {
        if *self.failure.lock().unwrap() == Some(stage) {
            Err(CollapseError::new(format!("controlled {stage} failure")))
        } else {
            Ok(())
        }
    }
    fn stages(&self) -> Vec<String> {
        self.events.lock().unwrap().iter().map(|event| event.stage.clone()).collect()
    }
    fn reset_events(&self) {
        self.events.lock().unwrap().clear();
    }
}
impl ExtensionRegistryHost for Port {
    fn custom_api_registered(&self, entry: &RuntimeApiRegistration) {
        self.record("api-register", &entry.api, None);
    }
    fn custom_api_removed(&self, api: &WireString) {
        self.record("api-remove", api, None);
    }
    fn oauth_registered(&self, entry: &RuntimeOAuthEntry) {
        self.record("oauth-register", &entry.provider, None);
    }
    fn oauth_removed(&self, provider: &WireString) {
        self.record("oauth-remove", provider, None);
    }
    fn set_runtime_usage(&self, provider: &WireString, _: &OpaqueExtensionHandle, _: Option<&str>) {
        self.record("usage-set", provider, None);
        let mut usages = self.usages.lock().unwrap();
        if !usages.contains(provider) {
            usages.push(provider.clone());
        }
    }
    fn remove_runtime_usage(&self, provider: &WireString) {
        self.record("usage-remove", provider, None);
        self.usages.lock().unwrap().retain(|name| name != provider);
    }
    fn install_api_key(&self, provider: &WireString, _: &str) -> ExtensionResult<()> {
        self.record("key-install", provider, None);
        self.checkpoint("key")
    }
    fn remove_config_api_key(&self, provider: &WireString) {
        self.record("key-remove", provider, None);
    }
    fn has_full_snapshot(&self) -> bool {
        self.record("has-full", &"p".into(), None);
        self.full.load(Ordering::SeqCst)
    }
    fn ensure_full_snapshot(&self) -> ExtensionResult<()> {
        self.record("ensure", &"p".into(), None);
        self.checkpoint("ensure")?;
        self.full.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn reload_static(&self, force: bool) -> ExtensionResult<()> {
        assert!(force);
        self.record("reload", &"p".into(), None);
        self.checkpoint("reload")
    }
    fn models_registered(
        &self,
        provider: &WireString,
        _: &[CustomModelOverlay],
        retained: Option<&ProviderOverride>,
    ) -> ExtensionResult<()> {
        self.record("models", provider, retained);
        self.checkpoint("models")
    }
    fn transport_applied(&self, provider: &WireString, incoming: &ProviderOverride) -> ExtensionResult<()> {
        self.record("transport", provider, Some(incoming));
        self.checkpoint("transport")
    }
    fn invalidate_provider(&self, provider: &WireString) {
        self.record("invalidate", provider, None);
    }
    fn invalidate_all_provider_lookups(&self) {
        self.record("invalidate-all", &"p".into(), None);
    }
}
impl ExtensionModifierHost for Port {
    fn oauth_credential(&self, provider: &WireString) -> Option<OpaqueExtensionHandle> {
        let _ = self.registry.snapshot();
        self.credentials.lock().unwrap().contains(provider).then(|| handle("controlled-credential"))
    }
    fn materialize_headers(&self, model: &HostModelRef) -> ExtensionResult<Option<ResolvedConfigHeaders>> {
        let _ = self.registry.snapshot();
        self.materializations.fetch_add(1, Ordering::SeqCst);
        let environment = |name: &str| (name == "HEADER_ENV").then(|| "!literal-result".to_owned());
        let context = ConfigValueContext { project_dir: Path::new("."), environment: &environment };
        match model.headers() {
            HeaderSlot::Source(HeaderSource::Live(headers)) => Ok(headers.snapshot(&self.resolver, &context)),
            // This scenario starts with Live. A successful hook creates literal
            // records, which subsequent hooks must clone without resolving.
            HeaderSlot::Source(HeaderSource::Config(_)) => panic!("materialized model header was reinterpreted"),
            _ => Ok(None),
        }
    }
    fn warn_modifier_failure(&self, provider: &WireString, error: &CollapseError) {
        let _ = self.registry.snapshot();
        self.warnings.lock().unwrap().push((provider.clone(), error.message_wire()));
    }
}
#[derive(Default)]
struct Fetcher(AtomicUsize);
#[async_trait]
impl RuntimeDefinitionFetcher for Fetcher {
    async fn fetch(&self, _: Option<WireString>) -> RegistryRuntimeResult<RawModelValue> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(DiscoveryError::new("registration must not discover"))
    }
}

#[test]
fn runtime_extensions_preserve_native_registration_cleanup_projection_and_suppression() {
    // Validation is atomic, including custom API/OAuth and usage registration.
    let registry = Arc::new(ExtensionRegistry::new());
    let host = Port::new(registry.clone(), true);
    let mut invalid = config(json!({"api":"extension-api"}), Some("controlled-key"));
    invalid.models = Some(vec![definition("bad")]);
    invalid.custom_api = Some(handle("api"));
    invalid.oauth = Some(oauth(Some(identity_modifier())));
    invalid.usage = Some(handle("usage"));
    assert!(register(&registry, &host, "p", &invalid, Some("a")).is_err());
    assert!(host.stages().is_empty());
    assert!(registry.snapshot().registered_sources.is_empty());
    assert!(registry.snapshot().custom_apis.is_empty());
    assert!(host.usages.lock().unwrap().is_empty());
    let mut reserved = config(json!({"api":"openai-responses"}), None);
    reserved.custom_api = Some(handle("api"));
    assert!(register(&registry, &host, "p", &reserved, Some("a")).is_err());
    assert!(registry.snapshot().registered_sources.is_empty());

    // Same-source replacement retains the old transport and manager; the
    // nonempty-model/no-fetcher early return skips the incoming transport.
    let fetcher = Arc::new(Fetcher::default());
    let definition_fetcher: Arc<dyn RuntimeDefinitionFetcher> = fetcher.clone();
    let mut first = RuntimeProviderConfig::new(
        VariantSpec::from_json(&json!({"api":"extension-api","baseUrl":"https://old.invalid","transport":"sse"})),
        HeaderSlot::Source(HeaderSource::Config(HeaderConfigRecord::from_pairs(vec![(
            "x-old-extension".into(),
            "old-header".into(),
        )]))),
        Some("controlled-key".into()),
    )
    .unwrap();
    first.models = Some(vec![definition("old")]);
    first.custom_api = Some(handle("first-api"));
    first.oauth = Some(oauth(Some(identity_modifier())));
    first.usage = Some(handle("usage"));
    first.dynamic_fetcher = Some(definition_fetcher.clone());
    register(&registry, &host, "p", &first, Some("a")).unwrap();
    let managers = registry.runtime_manager_options();
    assert_eq!(managers.len(), 1);
    assert_eq!(managers[0].options.cache_ttl_ms, Some(86_400_000.0));
    assert!(managers[0].options.dynamic_models_authoritative);
    assert!(
        matches!(&managers[0].options.static_models, Some(RawModelValue::Models(models)) if models.snapshot().is_empty())
    );
    assert!(
        managers[0].options.dynamic_fetcher.is_none(),
        "Host must install the wrapped adapter before manager invocation"
    );
    assert!(Arc::ptr_eq(managers[0].config.dynamic_fetcher.as_ref().unwrap(), &definition_fetcher));
    assert_eq!(text(managers[0].config.fields(), "baseUrl"), Some("https://old.invalid".into()));
    assert_eq!(managers[0].config.api_key_config(), Some("controlled-key"));
    assert_eq!(fetcher.0.load(Ordering::SeqCst), 0);
    host.reset_events();
    let mut replacement =
        config(json!({"api":"extension-api","baseUrl":"https://new.invalid"}), Some("replacement-key"));
    replacement.models = Some(vec![definition("new")]);
    replacement.oauth = Some(oauth(Some(identity_modifier())));
    register(&registry, &host, "p", &replacement, Some("a")).unwrap();
    assert_eq!(overlay_ids(&registry), vec![WireString::from("new")]);
    assert_eq!(
        text(registry.snapshot().provider_overrides[0].1.fields(), "baseUrl"),
        Some("https://old.invalid".into())
    );
    assert_eq!(
        host.events.lock().unwrap().iter().find(|event| event.stage == "models").unwrap().retained_base,
        Some("https://old.invalid".into())
    );
    assert!(!host.stages().contains(&"transport".into()));
    assert_eq!(registry.snapshot().managers.len(), 1);
    let mut empty = config(json!({}), None);
    empty.models = Some(Vec::new());
    register(&registry, &host, "p", &empty, Some("")).unwrap();
    register(&registry, &host, "p", &empty, None).unwrap();
    assert_eq!(overlay_ids(&registry), vec![WireString::from("new")]);
    assert_eq!(registry.snapshot().modifiers.len(), 1);
    assert_eq!(registry.snapshot().api_key_configs.len(), 1);
    assert_eq!(registry.snapshot().provider_sources[0].1, WireString::from("a"));
    assert_eq!(host.usages.lock().unwrap().len(), 1);

    // New registrations precede source handoff cleanup. Auth callbacks observe
    // the old owner but no old state; reload observes the installed new owner.
    host.reset_events();
    let mut handoff = config(json!({"api":"extension-api"}), None);
    handoff.custom_api = Some(handle("second-api"));
    handoff.oauth = Some(oauth(None));
    register(&registry, &host, "p", &handoff, Some("b")).unwrap();
    let events = host.events.lock().unwrap().clone();
    assert_eq!(&host.stages()[..5], &["api-register", "oauth-register", "key-remove", "usage-remove", "reload"]);
    let cleanup = events.iter().find(|event| event.stage == "key-remove").unwrap();
    assert_eq!(cleanup.owner, Some("a".into()));
    assert_eq!((cleanup.keys, cleanup.managers, cleanup.modifiers), (0, 0, 0));
    assert_eq!(events.iter().find(|event| event.stage == "reload").unwrap().owner, Some("b".into()));
    assert!(registry.snapshot().model_overlays.is_empty());
    assert!(registry.snapshot().provider_overrides.is_empty());
    assert!(host.usages.lock().unwrap().is_empty());
    registry.clear_source_registrations(&"a".into(), &host).unwrap();
    assert_eq!(registry.snapshot().custom_apis.len(), 1);
    assert_eq!(registry.snapshot().custom_oauth.len(), 1);
    registry.unregister_provider(&"p".into(), &host).unwrap();
    assert!(registry.snapshot().custom_oauth.is_empty());
    assert_eq!(registry.snapshot().custom_apis.len(), 1);
    host.reset_events();
    registry.clear_source_registrations(&"b".into(), &host).unwrap();
    assert_eq!(host.stages(), vec!["api-remove"]);
    assert!(registry.snapshot().custom_apis.is_empty());
    assert_eq!(registry.snapshot().registered_sources, vec![WireString::from("a"), WireString::from("b")]);
    registry.sync_extension_sources(&[], &host).unwrap();
    assert!(registry.snapshot().registered_sources.is_empty());

    // Native cleanup is phased, not transactional. Ensure failure retains
    // overlays/key/source but does not install the modifier or remove ownership.
    let partial = Arc::new(ExtensionRegistry::new());
    let partial_host = Port::new(partial.clone(), false);
    *partial_host.failure.lock().unwrap() = Some("ensure");
    assert!(register(&partial, &partial_host, "p", &first, Some("a")).is_err());
    assert_eq!(overlay_ids(&partial), vec![WireString::from("old")]);
    assert_eq!(partial.snapshot().api_key_configs.len(), 1);
    assert_eq!(partial.snapshot().custom_oauth.len(), 1);
    assert!(partial.snapshot().modifiers.is_empty());
    assert!(partial.snapshot().managers.is_empty());
    partial_host.reset_events();
    assert!(partial.clear_source_registrations(&"a".into(), &partial_host).is_err());
    assert_eq!(&partial_host.stages()[..3], &["api-remove", "oauth-remove", "ensure"]);
    let api_removal =
        partial_host.events.lock().unwrap().iter().find(|event| event.stage == "api-remove").unwrap().clone();
    assert_eq!(api_removal.oauth, 1, "OAuth cleanup follows custom API removal");
    assert_eq!(partial.snapshot().provider_sources.len(), 1);
    assert_eq!(partial.snapshot().api_key_configs.len(), 1);
    assert!(partial.unregister_provider(&"p".into(), &partial_host).is_err());
    assert!(partial.snapshot().provider_sources.is_empty());
    assert_eq!(partial.snapshot().api_key_configs.len(), 1);
    *partial_host.failure.lock().unwrap() = None;
    partial.sync_extension_sources(&[], &partial_host).unwrap();
    assert!(partial.snapshot().registered_sources.is_empty());

    // A later models failure leaves the preceding stages and overlays intact.
    let failed = Arc::new(ExtensionRegistry::new());
    let failed_host = Port::new(failed.clone(), true);
    *failed_host.failure.lock().unwrap() = Some("models");
    assert!(register(&failed, &failed_host, "p", &first, Some("a")).is_err());
    assert_eq!(overlay_ids(&failed), vec![WireString::from("old")]);
    assert_eq!(failed.snapshot().modifiers.len(), 1);
    assert!(failed.snapshot().managers.is_empty());
    assert!(failed.snapshot().provider_overrides.is_empty());
    *failed_host.failure.lock().unwrap() = Some("reload");
    assert!(register(&failed, &failed_host, "p", &handoff, Some("b")).is_err());
    assert_eq!(failed.snapshot().provider_sources[0].1, WireString::from("b"));
    assert!(failed.snapshot().api_key_configs.is_empty());
    assert_eq!(failed.snapshot().custom_apis[0].source_id, Some("b".into()));
    // Successful first-source cleanup followed by a failing later source stops
    // sync in insertion order, retaining the failing and untouched source IDs.
    let sync = Arc::new(ExtensionRegistry::new());
    let sync_host = Port::new(sync.clone(), true);
    register(&sync, &sync_host, "empty", &empty, Some("empty-source")).unwrap();
    sync.unregister_provider(&"empty".into(), &sync_host).unwrap();
    register(&sync, &sync_host, "p", &empty, Some("a")).unwrap();
    register(&sync, &sync_host, "q", &empty, Some("b")).unwrap();
    *sync_host.failure.lock().unwrap() = Some("ensure");
    assert!(sync.sync_extension_sources(&[], &sync_host).is_err());
    assert_eq!(sync.snapshot().registered_sources, vec![WireString::from("a"), WireString::from("b")]);

    // Whole-catalog projection deep-clones each hook input, skips missing
    // OAuth credentials, keeps the prior result after a mutating failure, and
    // continues later hooks without accumulating prior projections.
    let projected = Arc::new(ExtensionRegistry::new());
    let projection_host = Port::new(projected.clone(), true);
    let fail_mode = Arc::new(AtomicBool::new(true));
    let fail_hook_mode = fail_mode.clone();
    let seen_literal = Arc::new(AtomicUsize::new(0));
    let first_seen_literal = seen_literal.clone();
    let first_hook: Arc<dyn RuntimeModelModifier> =
        Arc::new(move |mut models: Vec<ModifierModel>, _: &OpaqueExtensionHandle| {
            assert_eq!(models.len(), 2);
            assert_eq!(text(&models[1].fields, "provider"), Some("foreign".into()));
            if let ModifierHeaders::Values(values) = &mut models[0].headers {
                assert_eq!(values[0].1, "!literal-result");
                first_seen_literal.fetch_add(1, Ordering::SeqCst);
                values.push(("x-mutated".into(), "literal".into()));
            } else {
                panic!("Live headers were not materialized");
            }
            models[0].fields.set("count", WireValue::Number(1.0));
            Ok(models)
        });
    let failing_hook: Arc<dyn RuntimeModelModifier> =
        Arc::new(move |mut models: Vec<ModifierModel>, _: &OpaqueExtensionHandle| {
            if fail_hook_mode.load(Ordering::SeqCst) {
                models[0].fields.set("count", WireValue::Number(999.0));
                models.clear();
                Err(CollapseError::new("same controlled failure".into()))
            } else {
                Ok(models)
            }
        });
    let last_hook: Arc<dyn RuntimeModelModifier> =
        Arc::new(|mut models: Vec<ModifierModel>, _: &OpaqueExtensionHandle| {
            assert_eq!(models.len(), 2);
            assert_eq!(models[0].fields.get("count"), Some(&WireValue::Number(1.0)));
            assert!(matches!(&models[0].headers, ModifierHeaders::Values(values) if values[0].1 == "!literal-result"));
            models[1].fields.set("name", WireValue::String("foreign-modified".into()));
            Ok(models)
        });
    let skipped_calls = Arc::new(AtomicUsize::new(0));
    let skipped_hook_calls = skipped_calls.clone();
    let skipped: Arc<dyn RuntimeModelModifier> =
        Arc::new(move |models: Vec<ModifierModel>, _: &OpaqueExtensionHandle| {
            skipped_hook_calls.fetch_add(1, Ordering::SeqCst);
            Ok(models)
        });
    for (provider, modifier) in
        [("first", first_hook), ("fail", failing_hook), ("last", last_hook), ("skipped", skipped)]
    {
        let mut value = config(json!({"api":"extension-api","baseUrl":"https://projection.invalid"}), None);
        value.models = Some(vec![definition(provider)]);
        value.oauth = Some(oauth(Some(modifier)));
        register(&projected, &projection_host, provider, &value, Some("projection")).unwrap();
    }
    *projection_host.credentials.lock().unwrap() = vec!["first".into(), "fail".into(), "last".into()];
    let original_headers = HeaderConfigRecord::from_pairs(vec![("x-initial".into(), "HEADER_ENV".into())]);
    let live = create_live_config_headers(
        &[Some(HeaderSource::Config(original_headers.clone()))],
        HeaderResolutionOptions::default(),
    )
    .unwrap();
    let unprojected = vec![
        plain("first", "initial", HeaderSlot::Source(HeaderSource::Live(live))),
        plain("foreign", "other", HeaderSlot::Null),
    ];
    for _ in 0..2 {
        let models = projected.apply_modifiers(&unprojected, &projection_host);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].spec().get("count"), Some(&WireValue::Number(1.0)));
        assert_eq!(text(models[1].spec(), "name"), Some("foreign-modified".into()));
        assert!(unprojected[0].spec().get("count").is_none());
        assert!(matches!(models[1].headers(), HeaderSlot::Null));
        assert!(
            matches!(models[0].headers(), HeaderSlot::Source(HeaderSource::Config(record)) if record.delete("x-mutated"))
        );
        assert!(!original_headers.delete("x-mutated"), "modifier mutated original configuration");
    }
    fail_mode.store(false, Ordering::SeqCst);
    projected.apply_modifiers(&unprojected, &projection_host);
    fail_mode.store(true, Ordering::SeqCst);
    projected.apply_modifiers(&unprojected, &projection_host);
    assert_eq!(projection_host.warnings.lock().unwrap().len(), 1, "success does not clear native warning dedup memo");
    assert_eq!(seen_literal.load(Ordering::SeqCst), 4);
    assert_eq!(skipped_calls.load(Ordering::SeqCst), 0);
    assert_eq!(projection_host.materializations.load(Ordering::SeqCst), 4);
    assert_eq!(projection_host.executor.0.load(Ordering::SeqCst), 0);

    // Suppression uses the shared selector grammar and learned aliases. Zero,
    // NaN and an expired timestamp are false; Infinity and literal :max work.
    let mut collapse = CollapseRuntime::new().unwrap();
    let has_live = |_: &WireString, id: &WireString| id.equals_ascii("literal:max");
    collapse
        .collapse_variants(
            &[Arc::new(VariantSpec::from_json(&json!({"provider":"learned","id":"logical","name":"Logical",
        "thinking":{"effortRouting":{"high":"retired-wire"}}})))],
            None,
        )
        .unwrap();
    registry.suppress_selector(&"learned/retired-wire".into(), 100.0, &mut collapse, None).unwrap();
    assert!(registry.is_selector_suppressed(&"learned/logical".into(), 1.0, &mut collapse, None).unwrap());
    registry
        .suppress_selector(&" google-antigravity/gemini-3-pro-high:high ".into(), 100.0, &mut collapse, Some(&has_live))
        .unwrap();
    assert!(
        registry
            .is_selector_suppressed(
                &"google-antigravity/gemini-3-pro-low:auto".into(),
                99.0,
                &mut collapse,
                Some(&has_live)
            )
            .unwrap()
    );
    assert!(
        !registry
            .is_selector_suppressed(&"google-antigravity/gemini-3-pro".into(), 100.0, &mut collapse, Some(&has_live))
            .unwrap()
    );
    assert!(
        !registry
            .is_selector_suppressed(&"google-antigravity/gemini-3-pro".into(), 0.0, &mut collapse, Some(&has_live))
            .unwrap()
    );
    for (selector, until) in [("p/zero", 0.0), ("p/negative-zero", -0.0), ("p/nan", f64::NAN)] {
        registry.suppress_selector(&selector.into(), until, &mut collapse, None).unwrap();
        assert!(!registry.is_selector_suppressed(&selector.into(), -1.0, &mut collapse, None).unwrap());
    }
    registry.suppress_selector(&"p/literal:max".into(), f64::INFINITY, &mut collapse, Some(&has_live)).unwrap();
    assert!(registry.is_selector_suppressed(&"p/literal:max".into(), 999.0, &mut collapse, Some(&has_live)).unwrap());
    assert!(!registry.is_selector_suppressed(&"p/literal".into(), 999.0, &mut collapse, Some(&has_live)).unwrap());
    registry.suppress_selector(&"pp/model".into(), 100.0, &mut collapse, None).unwrap();
    registry.clear_suppressed_selectors(Some(&"p".into()));
    assert!(registry.is_selector_suppressed(&"pp/model".into(), 1.0, &mut collapse, None).unwrap());
    registry.clear_suppressed_selector(&"pp/model:high".into(), &mut collapse, None).unwrap();
    assert!(!registry.is_selector_suppressed(&"pp/model".into(), 1.0, &mut collapse, None).unwrap());
    registry.suppress_selector(&"p/strict:HIGH".into(), 100.0, &mut collapse, None).unwrap();
    assert!(!registry.is_selector_suppressed(&"p/strict".into(), 1.0, &mut collapse, None).unwrap());
    register(&registry, &host, "p", &empty, Some("cleanup")).unwrap();
    registry.clear_source_registrations(&"cleanup".into(), &host).unwrap();
    assert!(
        registry.is_selector_suppressed(&"p/strict:HIGH".into(), 1.0, &mut collapse, None).unwrap(),
        "source cleanup does not clear suppression"
    );
    registry.clear_suppressed_selectors(None);
    assert!(!registry.is_selector_suppressed(&"p/strict:HIGH".into(), 1.0, &mut collapse, None).unwrap());
}
