//! Production facade scenarios: real configuration/SQLite with injected fetch.
use ara_cli::{
    catalog_discovery::{CatalogContext, DiscoveryError, DiscoveryReply, DiscoveryRequest, DiscoveryTransport},
    credential_store::{AuthCredential, SqliteCredentialStore},
    daily_model_config::{DailyOverrides, resolve_daily_selection_with_registry},
    model_cache::{SqliteModelCache, WireModelCacheWriteOptions},
    model_collapse::VariantSpec,
    model_config_file::ModelsConfigFile,
    model_config_values::{
        CommandConfigCache, ConfigCommandExecutor, ConfigCommandFailure, ConfigValueContext, ConfigValueResolver,
        SystemConfigValueClock, resolve_config_headers,
    },
    model_identity_wire::text,
    model_manager::{ModelRefreshStrategy, RawModelValue},
    model_patch::{HeaderSlot, ModelPatch, OrderedProviderSet},
    model_registry::{ModelRegistry, ModelRegistryHost, OpenAiCodexRegistryCredentials, RegistryCredentials},
    model_registry_extensions::{
        ModifierModel, OpaqueExtensionHandle, RuntimeDefinitionFetcher, RuntimeModelModifier, RuntimeOAuthRegistration,
        RuntimeProviderConfig,
    },
    model_registry_loader::{RegistryLoadOptions, startup_model_cache_provider_ids},
    model_registry_runtime::RegistryRuntimeResult,
    openai_codex_auth::OpenAiCodexAuth,
    provider_models::{ProviderEnvironment, models_dev_catalog_provider_ids, provider_descriptors},
};
use ara_rpc::{WireString, WireValue};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Fetch {
    calls: Mutex<Vec<(String, Option<String>)>>,
    payload: Mutex<Value>,
    refresh: bool,
}
#[async_trait]
impl DiscoveryTransport for Fetch {
    fn context_id(&self) -> u64 {
        self as *const Self as usize as u64
    }
    async fn fetch(&self, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        let bearer = request
            .headers
            .iter()
            .find(|(name, _)| name.equals_ascii("Authorization"))
            .map(|(_, value)| value.to_utf8().unwrap());
        self.calls.lock().unwrap().push((request.url.to_utf8().unwrap(), bearer.clone()));
        let status = if self.refresh && bearer.as_deref() == Some("Bearer initial-test-key") { 401 } else { 200 };
        Ok(DiscoveryReply {
            status,
            headers: Vec::new(),
            body: serde_json::to_vec(&*self.payload.lock().unwrap()).unwrap(),
            json_override: None,
        })
    }
}
struct Keys(AtomicUsize);
impl ConfigCommandExecutor for Keys {
    fn directory_is_enterable(&self, _: &Path) -> bool {
        true
    }
    fn execute(&self, command: &str, _: &Path) -> Result<String, ConfigCommandFailure> {
        assert_eq!(command, "rotating-key");
        Ok(if self.0.fetch_add(1, Ordering::SeqCst) == 0 { "initial-test-key" } else { "refreshed-test-key" }.into())
    }
}
fn options() -> RegistryLoadOptions {
    let mut disabled = OrderedProviderSet::default();
    for provider in startup_model_cache_provider_ids() {
        disabled.insert(provider);
    }
    for provider in ["ollama", "llama.cpp", "lm-studio"] {
        disabled.insert(provider.into());
    }
    RegistryLoadOptions { disabled_providers: disabled, ..Default::default() }
}
fn provider_set(id: &str) -> OrderedProviderSet {
    let mut set = OrderedProviderSet::default();
    set.insert(id.into());
    set
}
fn configuration(command: bool, q_name: &str) -> Value {
    let mut p = json!({"api":"openai-completions","baseUrl":"http://catalog.test/v3/compat",
        "auth":"none", "discovery":{"type":"openai-models-list","injectV1":false}});
    if command {
        p["auth"] = json!("apiKey");
        p["apiKey"] = json!("!rotating-key");
        p["authHeader"] = json!(true);
    }
    json!({"providers":{"p":p,"q":{"api":"openai-completions","auth":"none","baseUrl":"http://q.test/v1",
        "models":[{"id":"q-model","name":q_name}]}}})
}
fn host(root: &Path, fetch: Arc<Fetch>) -> ModelRegistryHost {
    let mut host = ModelRegistryHost::for_process(root.to_path_buf(), &root.join("models.sqlite")).unwrap();
    host.config_values = Arc::new(ConfigValueResolver::isolated());
    host.context = CatalogContext::new(fetch).unwrap();
    host
}

#[tokio::test]
async fn production_cache_discovery_identity_reload_and_daily_projection() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("models.json");
    std::fs::write(&path, configuration(false, "Q before").to_string()).unwrap();
    let fetch = Arc::new(Fetch {
        payload: Mutex::new(json!({"data":[{"id":"p-model","context_length":64000}]})),
        ..Default::default()
    });
    let registry =
        ModelRegistry::open(ModelsConfigFile::new(&path).unwrap(), options(), host(temp.path(), fetch.clone()))
            .await
            .unwrap();
    assert!(fetch.calls.lock().unwrap().is_empty(), "constructor must not perform discovery");
    let q = registry.find_exact(&"q".into(), &"q-model".into()).unwrap().unwrap();
    assert!(registry.find_exact(&"p".into(), &"p-model".into()).unwrap().is_none());
    registry.runtime().refresh_discoverable_providers(provider_set("p"), ModelRefreshStrategy::Online).await.unwrap();
    assert!(Arc::ptr_eq(&q, &registry.find_exact(&"q".into(), &"q-model".into()).unwrap().unwrap()));
    let p = registry.find_reference(&" P ".into(), &"P-MODEL".into()).unwrap().unwrap();
    assert_eq!(text(p.spec(), "id"), Some("p-model".into()));
    let config = registry.config();
    let selection = resolve_daily_selection_with_registry(
        config.as_ref(),
        &DailyOverrides { provider: Some("p".into()), model: Some("p-model".into()), ..Default::default() },
        &|_: &str| None,
        &registry,
    )
    .unwrap();
    assert_eq!(selection.model.context_window, Some(64000.0));
    assert_eq!(selection.model.base_url, "http://catalog.test/v3/compat");
    assert_eq!(fetch.calls.lock().unwrap().len(), 1);
    // Native provider lookup returns the whole catalog after get_all warms it;
    // header attribution still requires an exact provider match and truthy URL.
    registry.get_all().unwrap();
    assert_eq!(registry.get_provider_base_url(&"q".into()).unwrap(), text(q.spec(), "baseUrl"));
    assert_eq!(registry.get_provider_base_url(&"p".into()).unwrap(), Some("http://catalog.test/v3/compat".into()));
    assert!(registry.get_provider_base_url(&"missing-provider".into()).unwrap().is_none());
    let hot = ModelRegistry::open(ModelsConfigFile::new(&path).unwrap(), options(), host(temp.path(), fetch.clone()))
        .await
        .unwrap();
    assert!(hot.find_exact(&"p".into(), &"p-model".into()).unwrap().is_some());
    hot.runtime().refresh_discoverable_providers(provider_set("p"), ModelRefreshStrategy::Offline).await.unwrap();
    assert_eq!(fetch.calls.lock().unwrap().len(), 1, "hot offline must use actual SQLite");
    std::thread::sleep(std::time::Duration::from_millis(5));
    std::fs::write(&path, configuration(false, "Q after changed configuration").to_string()).unwrap();
    hot.runtime().refresh(ModelRefreshStrategy::Offline).await.unwrap();
    assert_eq!(
        text(hot.find_exact(&"q".into(), &"q-model".into()).unwrap().unwrap().spec(), "name"),
        Some("Q after changed configuration".into())
    );
    assert_eq!(fetch.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn production_command_refresh_reuses_new_key_and_keeps_private_header_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("models.json");
    std::fs::write(&path, configuration(true, "Q").to_string()).unwrap();
    let fetch = Arc::new(Fetch {
        refresh: true,
        payload: Mutex::new(json!({"data":[{"id":"p-model"}]})),
        ..Default::default()
    });
    let keys = Arc::new(Keys(AtomicUsize::new(0)));
    let mut supplied = host(temp.path(), fetch.clone());
    supplied.config_values = Arc::new(ConfigValueResolver::with_ports(
        Arc::new(CommandConfigCache::default()),
        Arc::new(SystemConfigValueClock),
        keys.clone(),
    ));
    let registry = ModelRegistry::open(ModelsConfigFile::new(&path).unwrap(), options(), supplied).await.unwrap();
    registry.runtime().refresh_discoverable_providers(provider_set("p"), ModelRefreshStrategy::Online).await.unwrap();
    registry.runtime().refresh_discoverable_providers(provider_set("p"), ModelRefreshStrategy::Online).await.unwrap();
    assert_eq!(keys.0.load(Ordering::SeqCst), 2);
    let calls = fetch.calls.lock().unwrap();
    assert_eq!(
        calls.iter().map(|(_, key)| key.as_deref()).collect::<Vec<_>>(),
        vec![Some("Bearer initial-test-key"), Some("Bearer refreshed-test-key"), Some("Bearer refreshed-test-key")]
    );
    drop(calls);
    let model = registry.find_exact(&"p".into(), &"p-model".into()).unwrap().unwrap();
    assert!(model.headers().as_source().is_some());
    let safe = model.spec().to_wire_json().stringify();
    assert!(!safe.contains("test-key") && !safe.contains("Authorization") && !safe.contains("headers"));
    let cache = SqliteModelCache::for_path(temp.path().join("models.sqlite"));
    let entry = cache
        .read_model_cache_wire(&WireString::from("p:openai-models-list-bare-context-v3"), 86400000.0, || 0.0)
        .unwrap()
        .unwrap();
    let persisted = entry.models.to_wire_json().stringify();
    assert!(!persisted.contains("test-key") && !persisted.contains("Authorization") && !persisted.contains("headers"));
}

struct RuntimeCredential(&'static str);
#[derive(Default)]
struct RuntimeCredentials {
    peeks: Mutex<Vec<WireString>>,
    resolves: AtomicUsize,
}
#[async_trait]
impl RegistryCredentials for RuntimeCredentials {
    fn has_auth(&self, provider: &WireString) -> bool {
        provider.equals_ascii("q")
    }
    async fn peek_key(&self, provider: &WireString) -> RegistryRuntimeResult<Option<WireString>> {
        self.peeks.lock().unwrap().push(provider.clone());
        Ok(provider.equals_ascii("q").then(|| "host-peek-key".into()))
    }
    async fn resolve_key(&self, _: &WireString) -> RegistryRuntimeResult<Option<WireString>> {
        self.resolves.fetch_add(1, Ordering::SeqCst);
        Ok(Some("unexpected-resolve-key".into()))
    }
    fn oauth_credential(&self, provider: &WireString) -> Option<OpaqueExtensionHandle> {
        provider.equals_ascii("q").then(|| Arc::new(RuntimeCredential("host-credential")) as OpaqueExtensionHandle)
    }
}
#[derive(Default)]
struct RuntimeDefinitions(Mutex<Vec<Option<WireString>>>);
#[async_trait]
impl RuntimeDefinitionFetcher for RuntimeDefinitions {
    async fn fetch(&self, key: Option<WireString>) -> RegistryRuntimeResult<RawModelValue> {
        self.0.lock().unwrap().push(key);
        // Sparse definitions: the production bridge must supply API/base/defaults.
        // The private-header row is intentionally not restorable from the cache.
        Ok(RawModelValue::models(vec![
            Arc::new(VariantSpec::from_json(
                &json!({"id":"q-dynamic","headers":{"X-Private-Extension":"private-runtime-header"}}),
            )),
            Arc::new(VariantSpec::from_json(&json!({"id":"q-hot"}))),
        ]))
    }
}
#[derive(Default)]
struct CollisionDefinitions(AtomicUsize);
#[async_trait]
impl RuntimeDefinitionFetcher for CollisionDefinitions {
    async fn fetch(&self, _: Option<WireString>) -> RegistryRuntimeResult<RawModelValue> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(RawModelValue::models(vec![Arc::new(VariantSpec::from_json(&json!({"id":"runtime-only"})))]))
    }
}
struct BoundRuntimeCallback {
    label: &'static str,
    calls: AtomicUsize,
}
impl BoundRuntimeCallback {
    fn invoke(&self, input: &str) -> String {
        self.calls.fetch_add(1, Ordering::SeqCst);
        format!("{}:{input}", self.label)
    }
}
fn runtime_callback(label: &'static str) -> OpaqueExtensionHandle {
    Arc::new(BoundRuntimeCallback { label, calls: AtomicUsize::new(0) })
}
fn runtime_definition(name: &str) -> ModelPatch {
    ModelPatch::new(VariantSpec::from_json(&json!({"id":"q-model","name":name})), HeaderSlot::Absent).unwrap()
}

#[tokio::test]
async fn production_runtime_registration_refresh_projection_reload_and_source_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("models.json");
    let mut config = configuration(false, "Q before");
    config["providers"]["p"].as_object_mut().unwrap().remove("discovery");
    config["providers"]["p"]["models"] = json!([{"id":"p-model","name":"P before"}]);
    std::fs::write(&path, config.to_string()).unwrap();
    let cache = SqliteModelCache::for_path(temp.path().join("models.sqlite"));
    let cache_only =
        Arc::new(VariantSpec::from_json(&json!({"provider":"openai","id":"facade-cache-only","name":"cache-only",
        "api":"openai-completions","baseUrl":"http://cached.test/v1","reasoning":false,"input":["text"],
        "cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"contextWindow":8192,"maxTokens":1024})));
    cache
        .write_model_cache_wire(
            &"openai".into(),
            1000.0,
            &[cache_only],
            WireModelCacheWriteOptions {
                authoritative: true,
                static_fingerprint: &"fixture-cache-only".into(),
                static_header_sources: &[],
                restorable_header_fallback: None,
            },
        )
        .unwrap();
    let fetch = Arc::new(Fetch::default());
    let credentials = Arc::new(RuntimeCredentials::default());
    let definitions = Arc::new(RuntimeDefinitions::default());
    let mut supplied = host(temp.path(), fetch.clone());
    supplied.credentials = credentials.clone();
    supplied.config_environment = Arc::new(|_: &str| None);
    supplied.clock = Arc::new(|| 1000.0);
    let bindings = supplied.extension_bindings.clone();
    let values = supplied.config_values.clone();
    let registry = ModelRegistry::open(ModelsConfigFile::new(&path).unwrap(), options(), supplied).await.unwrap();
    assert_eq!(
        text(registry.find_exact(&"q".into(), &"q-model".into()).unwrap().unwrap().spec(), "name"),
        Some("Q before".into())
    );

    let modifier_calls = Arc::new(AtomicUsize::new(0));
    let observed_catalogs = Arc::new(Mutex::new(Vec::new()));
    let calls = modifier_calls.clone();
    let catalogs = observed_catalogs.clone();
    let modifier: Arc<dyn RuntimeModelModifier> =
        Arc::new(move |mut models: Vec<ModifierModel>, credential: &OpaqueExtensionHandle| {
            assert_eq!(
                credential.downcast_ref::<RuntimeCredential>().unwrap().0,
                "host-credential",
                "OAuth definition handles must not stand in for Host credentials"
            );
            calls.fetch_add(1, Ordering::SeqCst);
            catalogs.lock().unwrap().push(
                models
                    .iter()
                    .filter_map(|model| Some((text(&model.fields, "provider")?, text(&model.fields, "id")?)))
                    .collect::<Vec<_>>(),
            );
            for model in &mut models {
                if text(&model.fields, "provider")
                    .is_some_and(|provider| provider.equals_ascii("p") || provider.equals_ascii("q"))
                {
                    let name = text(&model.fields, "name").unwrap().to_utf8().unwrap();
                    model.fields.set("name", WireValue::String(format!("{name} [extension]").into()));
                }
            }
            Ok(models)
        });
    let api_a = runtime_callback("api-a");
    let oauth_a = runtime_callback("oauth-a");
    let usage_a = runtime_callback("usage-a");
    let mut runtime = RuntimeProviderConfig::new(
        VariantSpec::from_json(&json!({"api":"controlled-extension-api","baseUrl":"http://runtime.test/v1"})),
        HeaderSlot::Absent,
        None,
    )
    .unwrap();
    runtime.models = Some(vec![runtime_definition("Runtime Q")]);
    runtime.custom_api = Some(api_a.clone());
    runtime.oauth = Some(RuntimeOAuthRegistration { handle: oauth_a.clone(), modifier: Some(modifier) });
    runtime.usage = Some(usage_a.clone());
    runtime.dynamic_fetcher = Some(definitions.clone());
    registry.register_provider("q".into(), runtime.clone(), Some("source-a".into())).await.unwrap();
    assert!(fetch.calls.lock().unwrap().is_empty());
    assert!(definitions.0.lock().unwrap().is_empty(), "registration must not fetch definitions");
    assert!(credentials.peeks.lock().unwrap().is_empty());
    assert_eq!(registry.extension_snapshot().managers.len(), 1);
    assert!(Arc::ptr_eq(&bindings.api(&"controlled-extension-api".into()).unwrap(), &api_a));
    assert!(Arc::ptr_eq(&bindings.oauth(&"q".into()).unwrap(), &oauth_a));
    let (usage, raw_key) = bindings.usage(&"q".into()).unwrap();
    assert!(Arc::ptr_eq(&usage, &usage_a));
    assert!(raw_key.is_none());
    // Invoke actual opaque Host objects. This controlled callback contract does
    // not establish CLI automatic custom-API dispatch or a real OAuth login.
    assert_eq!(
        bindings.oauth(&"q".into()).unwrap().downcast_ref::<BoundRuntimeCallback>().unwrap().invoke("login-probe"),
        "oauth-a:login-probe"
    );
    assert_eq!(usage.downcast_ref::<BoundRuntimeCallback>().unwrap().invoke("usage-probe"), "usage-a:usage-probe");
    assert_eq!(
        text(registry.find_exact(&"p".into(), &"p-model".into()).unwrap().unwrap().spec(), "name"),
        Some("P before [extension]".into())
    );
    assert_eq!(
        text(registry.find_exact(&"q".into(), &"q-model".into()).unwrap().unwrap().spec(), "name"),
        Some("Runtime Q [extension]".into())
    );

    registry.runtime().refresh_runtime_providers(ModelRefreshStrategy::Online).await.unwrap();
    assert_eq!(*definitions.0.lock().unwrap(), vec![Some(WireString::from("host-peek-key"))]);
    assert_eq!(*credentials.peeks.lock().unwrap(), vec![WireString::from("q")]);
    assert_eq!(credentials.resolves.load(Ordering::SeqCst), 0);
    let dynamic = registry.find_exact(&"q".into(), &"q-dynamic".into()).unwrap().unwrap();
    assert_eq!(text(dynamic.spec(), "api"), Some("controlled-extension-api".into()));
    assert_eq!(text(dynamic.spec(), "baseUrl"), Some("http://runtime.test/v1".into()));
    assert_eq!(text(dynamic.spec(), "name"), Some("q-dynamic [extension]".into()));
    assert_eq!(
        text(registry.find_exact(&"q".into(), &"q-hot".into()).unwrap().unwrap().spec(), "name"),
        Some("q-hot [extension]".into())
    );
    let no_environment = |_: &str| None;
    let headers = resolve_config_headers(
        dynamic.headers().as_source(),
        &values,
        &ConfigValueContext { project_dir: temp.path(), environment: &no_environment },
    )
    .unwrap();
    assert_eq!(headers.get("X-Private-Extension"), Some("private-runtime-header"));
    let safe = dynamic.spec().to_wire_json().stringify();
    assert!(!safe.contains("headers") && !safe.contains("private-runtime-header") && !safe.contains("host-peek-key"));
    assert_eq!(
        bindings
            .api(&"controlled-extension-api".into())
            .unwrap()
            .downcast_ref::<BoundRuntimeCallback>()
            .unwrap()
            .invoke(&text(dynamic.spec(), "id").unwrap().to_utf8().unwrap()),
        "api-a:q-dynamic"
    );
    let cached = cache.read_model_cache_wire(&"q".into(), 86_400_000.0, || 1000.0).unwrap().unwrap();
    assert!(cached.authoritative);
    assert_eq!(cached.updated_at, 1000.0);
    assert_eq!(cached.models.value.as_array().unwrap().len(), 2);
    let cached_model = VariantSpec::from_wire(cached.models.value.as_array().unwrap()[0].clone());
    assert_eq!(text(&cached_model, "id"), Some("q-dynamic".into()));
    assert_eq!(text(&cached_model, "name"), Some("q-dynamic".into()), "the cache holds unprojected model definitions");
    assert_eq!(cached.header_omitted_model_ids, vec![WireString::from("q-dynamic")]);
    let persisted = cached.models.to_wire_json().stringify();
    assert!(
        !persisted.contains("headers")
            && !persisted.contains("private-runtime-header")
            && !persisted.contains("host-peek-key")
    );

    registry.suppress_selector(&"q/q-hot".into(), 2000.0).unwrap();
    registry.suppress_selector(&"p/p-model".into(), 2000.0).unwrap();
    registry.runtime().refresh_runtime_providers(ModelRefreshStrategy::Offline).await.unwrap();
    assert!(registry.is_selector_suppressed(&"q/q-hot".into()).unwrap());
    registry.runtime().refresh_provider("q".into(), ModelRefreshStrategy::Offline).await.unwrap();
    assert!(!registry.is_selector_suppressed(&"q/q-hot".into()).unwrap());
    assert!(
        registry.is_selector_suppressed(&"p/p-model".into()).unwrap(),
        "provider refresh must retain unrelated suppression"
    );
    assert_eq!(definitions.0.lock().unwrap().len(), 1, "offline runtime refresh must read production SQLite");
    assert_eq!(
        cache.read_model_cache_wire(&"q".into(), 86_400_000.0, || 1000.0).unwrap().unwrap().updated_at,
        cached.updated_at
    );

    config["providers"]["p"]["models"][0]["name"] = json!("P after reload");
    config["providers"]["q"]["models"][0]["name"] = json!("Q after reload");
    config["providers"]["q"]["apiKey"] = json!("file-key-a");
    std::fs::write(&path, config.to_string()).unwrap();
    registry.runtime().reapply_model_policies().await.unwrap();
    assert_eq!(registry.extension_snapshot().managers.len(), 1, "runtime registration survives static reload");
    let before_lazy = modifier_calls.load(Ordering::SeqCst);
    assert_eq!(
        text(registry.find_exact(&"p".into(), &"p-model".into()).unwrap().unwrap().spec(), "name"),
        Some("P after reload [extension]".into())
    );
    assert!(
        modifier_calls.load(Ordering::SeqCst) > before_lazy,
        "cold provider lookup projects the complete unmodified catalog"
    );
    assert!(
        observed_catalogs
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .contains(&(WireString::from("openai"), WireString::from("facade-cache-only"))),
        "whole-catalog modifier on a cold p lookup must receive another provider's SQLite-only row before filtering"
    );
    assert!(!registry.is_selector_suppressed(&"p/p-model".into()).unwrap());
    assert!(
        registry.find_exact(&"q".into(), &"q-dynamic".into()).unwrap().is_none(),
        "fixed runtime manager cache guards cannot restore model-specific private headers"
    );
    assert_eq!(
        text(registry.find_exact(&"q".into(), &"q-hot".into()).unwrap().unwrap().spec(), "name"),
        Some("q-hot [extension]".into())
    );
    let all = registry.get_all().unwrap();
    assert_eq!(
        text(
            all.iter()
                .find(|model| text(model.spec(), "provider") == Some("p".into())
                    && text(model.spec(), "id") == Some("p-model".into()))
                .unwrap()
                .spec(),
            "name"
        ),
        Some("P after reload [extension]".into())
    );
    let after_full = modifier_calls.load(Ordering::SeqCst);
    for _ in 0..3 {
        assert_eq!(
            text(registry.find_exact(&"p".into(), &"p-model".into()).unwrap().unwrap().spec(), "name"),
            Some("P after reload [extension]".into())
        );
        registry.get_all().unwrap();
    }
    assert_eq!(
        modifier_calls.load(Ordering::SeqCst),
        after_full,
        "repeated cached queries must not accumulate modifier effects"
    );
    assert!(
        observed_catalogs
            .lock()
            .unwrap()
            .iter()
            .all(|models| models.iter().any(|(provider, _)| provider.equals_ascii("p"))
                && models.iter().any(|(provider, _)| provider.equals_ascii("q")))
    );

    // Fixed reload order installs runtime keys before file keys. An immediate
    // same-source runtime registration replaces the current file key, while a
    // later static policy reload lets that file key become current again.
    registry.runtime().refresh_runtime_providers(ModelRefreshStrategy::Online).await.unwrap();
    assert_eq!(definitions.0.lock().unwrap().last(), Some(&Some(WireString::from("file-key-a"))));
    let mut keyed_runtime =
        RuntimeProviderConfig::new(runtime.fields().clone(), runtime.headers().clone(), Some("runtime-key-b".into()))
            .unwrap();
    keyed_runtime.models = runtime.models.clone();
    keyed_runtime.custom_api = runtime.custom_api.clone();
    keyed_runtime.oauth = runtime.oauth.clone();
    keyed_runtime.usage = runtime.usage.clone();
    keyed_runtime.dynamic_fetcher = runtime.dynamic_fetcher.clone();
    registry.register_provider("q".into(), keyed_runtime, Some("source-a".into())).await.unwrap();
    assert_eq!(definitions.0.lock().unwrap().len(), 2, "key registration performs no definition fetch");
    registry.runtime().refresh_runtime_providers(ModelRefreshStrategy::Online).await.unwrap();
    assert_eq!(definitions.0.lock().unwrap().last(), Some(&Some(WireString::from("runtime-key-b"))));
    registry.runtime().reapply_model_policies().await.unwrap();
    assert_eq!(
        registry.extension_snapshot().api_key_configs,
        vec![(WireString::from("q"), "runtime-key-b".to_owned())],
        "durable runtime config retains B while current auth follows reload order"
    );
    registry.runtime().refresh_runtime_providers(ModelRefreshStrategy::Online).await.unwrap();
    assert_eq!(
        *definitions.0.lock().unwrap(),
        vec![
            Some(WireString::from("host-peek-key")),
            Some("file-key-a".into()),
            Some("runtime-key-b".into()),
            Some("file-key-a".into())
        ]
    );
    assert_eq!(
        *credentials.peeks.lock().unwrap(),
        vec![WireString::from("q")],
        "configured current keys bypass Host account lookup"
    );
    let rekeyed = cache
        .read_model_cache_wire(&"q".into(), 86_400_000.0, || 1000.0)
        .unwrap()
        .unwrap()
        .models
        .to_wire_json()
        .stringify();
    assert!(!rekeyed.contains("file-key-a") && !rekeyed.contains("runtime-key-b"));

    let api_b = runtime_callback("api-b");
    let oauth_b = runtime_callback("oauth-b");
    let usage_b = runtime_callback("usage-b");
    let mut replacement = RuntimeProviderConfig::new(
        VariantSpec::from_json(&json!({"api":"controlled-extension-api","baseUrl":"http://replacement.test/v1"})),
        HeaderSlot::Absent,
        Some("runtime-second-key".into()),
    )
    .unwrap();
    replacement.models = Some(vec![runtime_definition("Runtime B")]);
    replacement.custom_api = Some(api_b.clone());
    replacement.oauth = Some(RuntimeOAuthRegistration { handle: oauth_b.clone(), modifier: None });
    replacement.usage = Some(usage_b.clone());
    registry.register_provider("q".into(), replacement, Some("source-b".into())).await.unwrap();
    let handoff = registry.extension_snapshot();
    assert_eq!(handoff.provider_sources, vec![(WireString::from("q"), WireString::from("source-b"))]);
    assert!(handoff.managers.is_empty() && handoff.modifiers.is_empty());
    assert!(registry.find_exact(&"q".into(), &"q-dynamic".into()).unwrap().is_none());
    assert!(registry.find_exact(&"q".into(), &"q-hot".into()).unwrap().is_none());
    assert_eq!(
        text(registry.find_exact(&"p".into(), &"p-model".into()).unwrap().unwrap().spec(), "name"),
        Some("P after reload".into())
    );
    assert_eq!(
        text(registry.find_exact(&"q".into(), &"q-model".into()).unwrap().unwrap().spec(), "name"),
        Some("Runtime B".into())
    );
    registry.sync_extension_sources(vec!["source-b".into()]).await.unwrap();
    assert_eq!(registry.extension_snapshot().registered_sources, vec![WireString::from("source-b")]);
    assert!(Arc::ptr_eq(&bindings.api(&"controlled-extension-api".into()).unwrap(), &api_b));
    assert!(
        Arc::ptr_eq(&bindings.oauth(&"q".into()).unwrap(), &oauth_b),
        "old-source cleanup must retain the current OAuth owner"
    );
    assert_eq!(
        bindings
            .api(&"controlled-extension-api".into())
            .unwrap()
            .downcast_ref::<BoundRuntimeCallback>()
            .unwrap()
            .invoke("q-model"),
        "api-b:q-model"
    );
    assert_eq!(
        bindings.oauth(&"q".into()).unwrap().downcast_ref::<BoundRuntimeCallback>().unwrap().invoke("login-probe"),
        "oauth-b:login-probe"
    );
    let (usage, key) = bindings.usage(&"q".into()).unwrap();
    assert!(Arc::ptr_eq(&usage, &usage_b));
    assert_eq!(key.as_deref(), Some("runtime-second-key"));
    assert_eq!(
        usage.downcast_ref::<BoundRuntimeCallback>().unwrap().invoke(key.as_deref().unwrap()),
        "usage-b:runtime-second-key"
    );
    registry.unregister_provider("q".into()).await.unwrap();
    let removed = registry.extension_snapshot();
    assert!(
        removed.model_overlays.is_empty() && removed.api_key_configs.is_empty() && removed.provider_sources.is_empty()
    );
    assert!(bindings.oauth(&"q".into()).is_none() && bindings.usage(&"q".into()).is_none());
    assert_eq!(
        text(registry.find_exact(&"q".into(), &"q-model".into()).unwrap().unwrap().spec(), "name"),
        Some("Q after reload".into())
    );
    assert!(
        bindings.api(&"controlled-extension-api".into()).is_some(),
        "custom API remains owned by its source until source cleanup"
    );
    registry.sync_extension_sources(Vec::new()).await.unwrap();
    assert!(bindings.api(&"controlled-extension-api".into()).is_none());
    assert!(registry.extension_snapshot().registered_sources.is_empty());
    assert_eq!(definitions.0.lock().unwrap().len(), 4);
    assert!(
        fetch.calls.lock().unwrap().is_empty(),
        "all network behavior is controlled and none is needed by this scenario"
    );

    // Standard and catalog-only manager slots are replaced by a same-name
    // runtime manager. Special OAuth slots intentionally use a separate rule.
    let standard = WireString::from("openai");
    let descriptors = provider_descriptors();
    assert!(descriptors.iter().any(|descriptor| descriptor.provider_id == standard));
    let bundled = ara_cli::model_identity_wire::bundled_models();
    let catalog_only = models_dev_catalog_provider_ids()
        .into_iter()
        .find(|provider| {
            !descriptors.iter().any(|descriptor| &descriptor.provider_id == provider)
                && !["google-antigravity", "google-gemini-cli", "openai-codex"]
                    .iter()
                    .any(|special| provider.equals_ascii(special))
                && bundled.iter().any(|model| text(model, "provider").as_ref() == Some(provider))
        })
        .expect("fixed provider catalog has a bundled catalog-only provider");
    let collision_dir = temp.path().join("runtime-manager-slots");
    std::fs::create_dir(&collision_dir).unwrap();
    let collision_path = collision_dir.join("models.json");
    std::fs::write(&collision_path, json!({"providers":{}}).to_string()).unwrap();
    let collision_cache = SqliteModelCache::open(collision_dir.join("models.sqlite")).unwrap();
    let audit = rusqlite::Connection::open(collision_dir.join("models.sqlite")).unwrap();
    audit.execute_batch("CREATE TABLE registry_cache_writes(provider_id TEXT); CREATE TRIGGER registry_cache_write AFTER INSERT ON model_cache BEGIN INSERT INTO registry_cache_writes VALUES(NEW.provider_id); END;").unwrap();
    let collision_fetch = Arc::new(Fetch::default());
    let collision_credentials = Arc::new(RuntimeCredentials::default());
    let mut collision_host = host(&collision_dir, collision_fetch.clone());
    collision_host.credentials = collision_credentials.clone();
    collision_host.config_environment = Arc::new(|_: &str| None);
    collision_host.clock = Arc::new(|| 1000.0);
    let mut collision_options = options();
    let mut disabled = OrderedProviderSet::default();
    for provider in collision_options
        .disabled_providers
        .iter()
        .filter(|provider| *provider != &standard && *provider != &catalog_only)
    {
        disabled.insert(provider.clone());
    }
    collision_options.disabled_providers = disabled;
    let collision =
        ModelRegistry::open(ModelsConfigFile::new(&collision_path).unwrap(), collision_options, collision_host)
            .await
            .unwrap();
    let mut selected = OrderedProviderSet::default();
    let mut collision_callbacks = Vec::new();
    for provider in [&standard, &catalog_only] {
        let callback = Arc::new(CollisionDefinitions::default());
        let mut registered = RuntimeProviderConfig::new(
            VariantSpec::from_json(&json!({"api":"openai-completions","baseUrl":"http://runtime-slot.test/v1"})),
            HeaderSlot::Absent,
            None,
        )
        .unwrap();
        registered.dynamic_fetcher = Some(callback.clone());
        collision.register_provider(provider.clone(), registered, None).await.unwrap();
        selected.insert(provider.clone());
        collision_callbacks.push(callback);
    }
    assert!(collision_credentials.peeks.lock().unwrap().is_empty() && collision_fetch.calls.lock().unwrap().is_empty());
    collision.runtime().refresh_discoverable_providers(selected, ModelRefreshStrategy::Online).await.unwrap();
    assert!(
        collision_fetch.calls.lock().unwrap().is_empty(),
        "same-name standard/catalog-only producers must not issue an extra endpoint or shared-catalog request"
    );
    let collision_peeks = collision_credentials.peeks.lock().unwrap();
    for provider in [&standard, &catalog_only] {
        assert_eq!(
            collision_peeks.iter().filter(|candidate| *candidate == provider).count(),
            1,
            "only the runtime callback adapter may preflight this provider"
        );
        let row = collision_cache.read_model_cache_wire(provider, 86_400_000.0, || 1000.0).unwrap().unwrap();
        assert!(row.authoritative);
        assert_eq!(row.models.value.as_array().unwrap().len(), 1);
        assert_eq!(
            text(&VariantSpec::from_wire(row.models.value.as_array().unwrap()[0].clone()), "id"),
            Some("runtime-only".into())
        );
    }
    assert_eq!(collision_peeks.len(), 2);
    drop(collision_peeks);
    assert!(collision_callbacks.iter().all(|callback| callback.0.load(Ordering::SeqCst) == 1));
    assert_eq!(
        audit.query_row("SELECT COUNT(*) FROM registry_cache_writes", [], |row| row.get::<_, i64>(0)).unwrap(),
        2,
        "one production SQLite write per runtime manager, with no duplicate standard/catalog writer"
    );
}

#[derive(Default)]
struct RejectAuthDns(AtomicUsize);
impl reqwest::dns::Resolve for RejectAuthDns {
    fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            let failure: Box<dyn std::error::Error + Send + Sync> =
                Box::new(std::io::Error::other("auth network is forbidden in the production account fixture"));
            Err(failure)
        })
    }
}
#[derive(Default)]
struct AccountCatalogFetch {
    calls: Mutex<Vec<(String, Option<String>, String)>>,
}
#[async_trait]
impl DiscoveryTransport for AccountCatalogFetch {
    fn context_id(&self) -> u64 {
        self as *const Self as usize as u64
    }
    async fn fetch(&self, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        let header = |name: &str| {
            request
                .headers
                .iter()
                .find(|(key, _)| key.to_utf8().is_ok_and(|key| key.eq_ignore_ascii_case(name)))
                .map(|(_, value)| value.to_utf8().unwrap())
        };
        let account = header("ChatGPT-account-id");
        let bearer = header("Authorization").unwrap();
        let url = request.url.to_utf8().unwrap();
        assert!(url.starts_with("https://chatgpt.com/backend-api/codex/models?"));
        let payload = match account.as_deref() {
            Some("workspace-a") => {
                assert_eq!(bearer, "Bearer registry-access-a");
                json!({"models":[{"slug":"account-a-only","display_name":"A only"},{"slug":"account-shared","display_name":"Shared A"}]})
            }
            Some("workspace-b") => {
                assert_eq!(bearer, "Bearer registry-access-b");
                json!({"models":[{"slug":"account-b-only","display_name":"B only"},{"slug":"account-shared","display_name":"Shared B"}]})
            }
            None => {
                assert_eq!(bearer, "Bearer registry-access-b-suffix");
                json!({"models":[{"slug":"explicit-only","display_name":"Explicit only"},{"slug":"account-shared","display_name":"Shared explicit"}]})
            }
            _ => panic!("unexpected account in controlled catalog transport"),
        };
        self.calls.lock().unwrap().push((url, account, bearer));
        Ok(DiscoveryReply {
            status: 200,
            headers: Vec::new(),
            body: serde_json::to_vec(&payload).unwrap(),
            json_override: None,
        })
    }
}

#[tokio::test]
async fn production_codex_accounts_union_failure_fallback_and_explicit_key_ownership() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("models.json");
    let mut config = json!({"providers":{}});
    std::fs::write(&path, config.to_string()).unwrap();
    let auth_path = temp.path().join("auth.sqlite");
    let store = SqliteCredentialStore::open(&auth_path).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    let a_fields = json!({"access":"registry-access-a","accountId":"workspace-a","expires":now+600_000,
        "email":"a@example.test","unknownFields":{"privateNote":"registry-private-unknown","array":[1,true]}})
    .as_object()
    .unwrap()
    .clone();
    let b_fields = json!({"access":"registry-access-b","accountId":"workspace-b","expires":now+600_000,
        "authorizedAt":now-1,"email":"b@example.test"})
    .as_object()
    .unwrap()
    .clone();
    store.upsert_auth_credential_for_provider("openai-codex", &AuthCredential::oauth(a_fields.clone())).unwrap();
    let seeded =
        store.upsert_auth_credential_for_provider("openai-codex", &AuthCredential::oauth(b_fields.clone())).unwrap();
    let b_id = seeded.iter().find(|row| matches!(&row.credential, AuthCredential::OAuth { fields } if fields.get("accountId").and_then(Value::as_str) == Some("workspace-b"))).unwrap().id;
    assert!(
        !a_fields.contains_key("authorizedAt"),
        "legacy first account intentionally has no interactive authorization timestamp"
    );
    let no_auth_network = Arc::new(RejectAuthDns::default());
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(no_auth_network.clone())
        .build()
        .unwrap();
    let auth = Arc::new(OpenAiCodexAuth::open(auth_path, client).await.unwrap());
    let environment: Arc<dyn ProviderEnvironment> = Arc::new(|_: &str| None);
    let credentials =
        Arc::new(OpenAiCodexRegistryCredentials::new(environment.clone(), auth.clone(), CancellationToken::new()));
    let codex = WireString::from("openai-codex");
    let revision = store.revision().unwrap();
    let original_rows = store
        .list_auth_credentials(Some("openai-codex"))
        .unwrap()
        .into_iter()
        .map(|row| (row.id, row.serialized_data))
        .collect::<Vec<_>>();
    assert!(credentials.has_auth(&codex) && credentials.has_oauth_credentials(&codex));
    assert_eq!(credentials.peek_key(&codex).await.unwrap(), Some("registry-access-a".into()));
    assert_eq!(credentials.peek_key(&codex).await.unwrap(), Some("registry-access-b".into()));
    assert_eq!(credentials.peek_key(&codex).await.unwrap(), Some("registry-access-a".into()));
    assert_eq!(credentials.peek_key(&codex).await.unwrap(), Some("registry-access-b".into()));
    assert_eq!(store.revision().unwrap(), revision);
    assert_eq!(no_auth_network.0.load(Ordering::SeqCst), 0);

    let fetch = Arc::new(AccountCatalogFetch::default());
    let clock = Arc::new(AtomicUsize::new(1000));
    let mut supplied = host(temp.path(), Arc::new(Fetch::default()));
    supplied.credentials = credentials.clone();
    supplied.config_environment = Arc::new(|_: &str| None);
    supplied.factory.environment = environment;
    supplied.context = CatalogContext::new(fetch.clone()).unwrap();
    let catalog_clock = clock.clone();
    supplied.clock = Arc::new(move || catalog_clock.load(Ordering::SeqCst) as f64);
    let mut enabled_codex = options();
    let mut disabled = OrderedProviderSet::default();
    for provider in enabled_codex.disabled_providers.iter().filter(|provider| *provider != &codex) {
        disabled.insert(provider.clone());
    }
    enabled_codex.disabled_providers = disabled;
    let registry = ModelRegistry::open(ModelsConfigFile::new(&path).unwrap(), enabled_codex, supplied).await.unwrap();
    let observed_credentials = Arc::new(Mutex::new(Vec::new()));
    let observed = observed_credentials.clone();
    let modifier: Arc<dyn RuntimeModelModifier> =
        Arc::new(move |models: Vec<ModifierModel>, credential: &OpaqueExtensionHandle| {
            let AuthCredential::OAuth { fields } = credential
                .downcast_ref::<AuthCredential>()
                .expect("production modifier receives a complete native AuthCredential")
            else {
                panic!("expected native OAuth credential")
            };
            observed.lock().unwrap().push(fields.clone());
            Ok(models)
        });
    let mut extension = RuntimeProviderConfig::new(
        VariantSpec::from_json(&json!({"api":"openai-codex-responses","baseUrl":"https://chatgpt.com/backend-api"})),
        HeaderSlot::Absent,
        None,
    )
    .unwrap();
    extension.models = Some(vec![
        ModelPatch::new(
            VariantSpec::from_json(&json!({"id":"modifier-anchor","name":"Modifier anchor"})),
            HeaderSlot::Absent,
        )
        .unwrap(),
    ]);
    extension.oauth = Some(RuntimeOAuthRegistration {
        handle: runtime_callback("controlled-codex-oauth-definition"),
        modifier: Some(modifier),
    });
    registry.register_provider(codex.clone(), extension, Some("codex-modifier".into())).await.unwrap();
    assert!(fetch.calls.lock().unwrap().is_empty());
    assert_eq!(
        observed_credentials.lock().unwrap().first(),
        Some(&a_fields),
        "modifier selects first stored legacy A, retaining unknown fields rather than latest interactive B"
    );

    registry
        .runtime()
        .refresh_discoverable_providers(provider_set("openai-codex"), ModelRefreshStrategy::Online)
        .await
        .unwrap();
    {
        let calls = fetch.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(
            calls.iter().any(|(_, account, bearer)| account.as_deref() == Some("workspace-a")
                && bearer == "Bearer registry-access-a")
        );
        assert!(
            calls.iter().any(|(_, account, bearer)| account.as_deref() == Some("workspace-b")
                && bearer == "Bearer registry-access-b")
        );
    }
    let catalog_ids = || {
        registry
            .models_for_provider(&codex)
            .unwrap()
            .iter()
            .filter_map(|model| text(model.spec(), "id"))
            .collect::<Vec<_>>()
    };
    let complete_catalog = catalog_ids();
    for id in ["account-a-only", "account-b-only", "account-shared"] {
        assert!(complete_catalog.contains(&id.into()));
    }
    assert_eq!(complete_catalog.iter().filter(|id| id.equals_ascii("account-shared")).count(), 1);
    assert_eq!(
        text(registry.find_exact(&codex, &"account-shared".into()).unwrap().unwrap().spec(), "name"),
        Some("Shared A".into()),
        "union uses the first account's shared model exactly once"
    );
    let cache = SqliteModelCache::for_path(temp.path().join("models.sqlite"));
    let cache_ids = |models: &VariantSpec| {
        models
            .value
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|row| text(&VariantSpec::from_wire(row.clone()), "id"))
            .collect::<Vec<_>>()
    };
    let complete_cache = cache.read_model_cache_wire(&codex, 86_400_000.0, || 1000.0).unwrap().unwrap();
    assert!(complete_cache.authoritative);
    assert_eq!(cache_ids(&complete_cache.models).len(), 3);
    let persisted = complete_cache.models.to_wire_json().stringify();
    assert!(
        !persisted.contains("registry-access-")
            && !persisted.contains("workspace-")
            && !persisted.contains("registry-private-unknown")
    );
    assert_eq!(store.revision().unwrap(), revision);
    assert_eq!(no_auth_network.0.load(Ordering::SeqCst), 0);
    assert_eq!(
        store
            .list_auth_credentials(Some("openai-codex"))
            .unwrap()
            .into_iter()
            .map(|row| (row.id, row.serialized_data))
            .collect::<Vec<_>>(),
        original_rows
    );

    let mut expired_b = b_fields.clone();
    expired_b.insert("expires".into(), json!(now - 1));
    expired_b.remove("refresh");
    store.update_auth_credential(b_id, &AuthCredential::oauth(expired_b)).unwrap();
    let expired_revision = store.revision().unwrap();
    let expired_rows = store
        .list_auth_credentials(Some("openai-codex"))
        .unwrap()
        .into_iter()
        .map(|row| (row.id, row.serialized_data))
        .collect::<Vec<_>>();
    for _ in 0..4 {
        credentials.peek_key(&codex).await.unwrap();
        assert!(registry.find_exact(&codex, &"account-b-only".into()).unwrap().is_some());
    }
    assert_eq!(fetch.calls.lock().unwrap().len(), 2, "lookup/peek must not start catalog discovery");
    assert_eq!(store.revision().unwrap(), expired_revision, "lookup/peek must not refresh expired B");
    assert!(
        credentials.codex_accounts("registry-access-a".into(), false).await.unwrap().is_none(),
        "one unresolvable stored account aborts the complete discovery account set"
    );
    clock.store(2000, Ordering::SeqCst);
    registry
        .runtime()
        .refresh_discoverable_providers(provider_set("openai-codex"), ModelRefreshStrategy::Online)
        .await
        .unwrap();
    assert_eq!(fetch.calls.lock().unwrap().len(), 2, "an incomplete account set must not fetch a partial catalog");
    let retained = catalog_ids();
    assert!(complete_catalog.iter().all(|id| retained.contains(id)));
    let fallback_cache = cache.read_model_cache_wire(&codex, 86_400_000.0, || 2000.0).unwrap().unwrap();
    let retained_cache = cache_ids(&fallback_cache.models);
    for id in ["account-a-only", "account-b-only", "account-shared"] {
        assert!(
            retained_cache.contains(&id.into()),
            "failed sibling resolution must not replace SQLite with only account A"
        );
    }
    assert_eq!(
        no_auth_network.0.load(Ordering::SeqCst),
        0,
        "missing refresh is rejected before any auth DNS/network attempt"
    );
    assert_eq!(store.revision().unwrap(), expired_revision);

    // Explicit config tokens suppress stored OAuth resolution. Even expired B
    // supplies its account ID only when its stored access string matches exactly.
    config["providers"]["openai-codex"] = json!({"api":"openai-codex-responses","baseUrl":"https://chatgpt.com/backend-api","apiKey":"registry-access-b"});
    std::fs::write(&path, config.to_string()).unwrap();
    registry.runtime().reapply_model_policies().await.unwrap();
    registry
        .runtime()
        .refresh_discoverable_providers(provider_set("openai-codex"), ModelRefreshStrategy::Online)
        .await
        .unwrap();
    {
        let calls = fetch.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[2].1.as_deref(), Some("workspace-b"));
        assert_eq!(calls[2].2, "Bearer registry-access-b");
    }
    let explicit_b = cache.read_model_cache_wire(&codex, 86_400_000.0, || 2000.0).unwrap().unwrap();
    assert_eq!(
        cache_ids(&explicit_b.models),
        vec![WireString::from("account-b-only"), WireString::from("account-shared")]
    );
    config["providers"]["openai-codex"]["apiKey"] = json!("registry-access-b-suffix");
    std::fs::write(&path, config.to_string()).unwrap();
    registry.runtime().reapply_model_policies().await.unwrap();
    registry
        .runtime()
        .refresh_discoverable_providers(provider_set("openai-codex"), ModelRefreshStrategy::Online)
        .await
        .unwrap();
    let calls = fetch.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert!(calls[3].1.is_none(), "prefix token matches must not inherit a stored account ID");
    assert_eq!(calls[3].2, "Bearer registry-access-b-suffix");
    drop(calls);
    let explicit = cache.read_model_cache_wire(&codex, 86_400_000.0, || 2000.0).unwrap().unwrap();
    assert_eq!(
        cache_ids(&explicit.models),
        vec![WireString::from("account-shared"), WireString::from("explicit-only")]
    );
    assert!(observed_credentials.lock().unwrap().iter().all(|fields| fields == &a_fields));
    assert_eq!(store.revision().unwrap(), expired_revision);
    assert_eq!(no_auth_network.0.load(Ordering::SeqCst), 0);
    assert_eq!(
        store
            .list_auth_credentials(Some("openai-codex"))
            .unwrap()
            .into_iter()
            .map(|row| (row.id, row.serialized_data))
            .collect::<Vec<_>>(),
        expired_rows
    );
}
