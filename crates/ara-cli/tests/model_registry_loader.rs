//! Grouped loader/cache scenarios from fixed OMP 596f2da model-registry
//! lazy-loading, command-values, cache-headers, default-config and startup tests.
//! File and SQLite cases exercise actual stores; helpers remain controlled fakes.

use ara_cli::{
    model_cache::{SqliteModelCache, WireCacheEntry, WireModelCacheWriteOptions},
    model_collapse::{CollapseError, VariantSpec},
    model_config_file::ModelsConfigFile,
    model_config_values::{
        CommandConfigCache, ConfigCommandExecutor, ConfigCommandFailure, ConfigValueClock, ConfigValueContext,
        ConfigValueResolver, HeaderConfigRecord, HeaderSource, resolve_config_headers,
    },
    model_identity_wire::text,
    model_manager::{ModelArray, ModelClock, ModelResolutionSource, fingerprint_static_models},
    model_patch::{HeaderSlot, HostModel, HostModelRef, OrderedProviderSet},
    model_registry_loader::{
        ProviderDiscoveryStatus, RegistryCachePort, RegistryLoadOptions, RegistryLoader, RegistryLoaderError,
        RegistryLoaderHost, STARTUP_CACHE_TTL_MS,
    },
};
use ara_rpc::{WireString, WireValue};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

struct Clock;
impl ConfigValueClock for Clock {
    fn now_millis(&self) -> u64 {
        1000
    }
}
struct Commands {
    calls: Mutex<Vec<String>>,
    enterable: AtomicBool,
    continue_loading: Arc<AtomicBool>,
}
impl ConfigCommandExecutor for Commands {
    fn directory_is_enterable(&self, _: &Path) -> bool {
        self.enterable.load(Ordering::SeqCst)
    }
    fn execute(&self, command: &str, _: &Path) -> Result<String, ConfigCommandFailure> {
        self.calls.lock().unwrap().push(command.into());
        match command {
            "header" => Ok("header-value".into()),
            "key" => {
                assert!(self.calls.lock().unwrap().iter().any(|command| command == "header"));
                Ok("installed-key".into())
            }
            "stop" => {
                self.continue_loading.store(false, Ordering::SeqCst);
                Ok("settled".into())
            }
            _ => Err(ConfigCommandFailure::NonZeroExit),
        }
    }
}
#[derive(Default)]
struct Cache {
    rows: RefCell<HashMap<WireString, WireCacheEntry>>,
    reads: RefCell<Vec<(WireString, f64)>>,
    repairs: RefCell<Vec<WireString>>,
    fail_reads: AtomicBool,
    fail_repairs: AtomicBool,
}
impl RegistryCachePort for Cache {
    fn read(
        &self,
        provider: &WireString,
        ttl: f64,
        _: &dyn ModelClock,
    ) -> Result<Option<WireCacheEntry>, RegistryLoaderError> {
        self.reads.borrow_mut().push((provider.clone(), ttl));
        if self.fail_reads.load(Ordering::SeqCst) {
            Err(RegistryLoaderError::CacheRead)
        } else {
            Ok(self.rows.borrow().get(provider).cloned())
        }
    }
    fn repair(
        &self,
        provider: &WireString,
        _: &WireCacheEntry,
        models: &[HostModelRef],
        _: &HeaderSlot,
    ) -> Result<(), RegistryLoaderError> {
        assert!(models.iter().all(|model| model.spec().get("headers").is_none()));
        self.repairs.borrow_mut().push(provider.clone());
        if self.fail_repairs.load(Ordering::SeqCst) { Err(RegistryLoaderError::CacheWrite) } else { Ok(()) }
    }
}

struct Harness {
    directory: tempfile::TempDir,
    commands: Arc<Commands>,
    resolver: ConfigValueResolver,
    cache: Cache,
    env: HashMap<String, String>,
    installed: Mutex<Vec<(WireString, Option<String>)>>,
    continue_loading: Arc<AtomicBool>,
}
impl Harness {
    fn new() -> Self {
        let continue_loading = Arc::new(AtomicBool::new(true));
        let commands = Arc::new(Commands {
            calls: Mutex::new(Vec::new()),
            enterable: AtomicBool::new(true),
            continue_loading: continue_loading.clone(),
        });
        let resolver =
            ConfigValueResolver::with_ports(Arc::new(CommandConfigCache::default()), Arc::new(Clock), commands.clone());
        Self {
            directory: tempfile::tempdir().unwrap(),
            commands,
            resolver,
            cache: Cache::default(),
            env: HashMap::new(),
            installed: Mutex::new(Vec::new()),
            continue_loading,
        }
    }
    fn file(&self, value: Value) -> ModelsConfigFile {
        let path = self.directory.path().join("models.json");
        std::fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();
        ModelsConfigFile::new(path).unwrap()
    }
    fn host<R>(&self, cache: &dyn RegistryCachePort, run: impl FnOnce(&RegistryLoaderHost<'_>) -> R) -> R {
        let environment = &self.env;
        let config_env = |name: &str| environment.get(name).cloned();
        let provider_env = |name: &str| environment.get(name).map(|value| value.as_str().into());
        let install = |provider: &WireString, value: Option<&str>| {
            self.installed.lock().unwrap().push((provider.clone(), value.map(str::to_owned)))
        };
        let keep_going = || self.continue_loading.load(Ordering::SeqCst);
        run(&RegistryLoaderHost {
            project_dir: self.directory.path(),
            config_values: &self.resolver,
            config_environment: &config_env,
            provider_environment: &provider_env,
            cache,
            clock: &|| 1000.0,
            has_auth: &|_| false,
            install_config_key: &install,
            continue_loading: &keep_going,
        })
    }
}
fn identity(_: &WireString, models: Vec<HostModelRef>) -> Result<Vec<HostModelRef>, CollapseError> {
    Ok(models)
}
fn spec(provider: &str, id: &str) -> Value {
    json!({"provider":provider,"id":id,"name":id,"api":"openai-completions","baseUrl":"https://models.test/v1",
        "reasoning":false,"input":["text"],"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"contextWindow":8192,"maxTokens":1024})
}
fn row(provider: &str, ids: &[&str]) -> WireCacheEntry {
    WireCacheEntry {
        models: VariantSpec::from_json(&Value::Array(ids.iter().map(|id| spec(provider, id)).collect())),
        fresh: true,
        authoritative: true,
        updated_at: 1000.0,
        header_omitted_model_ids: Vec::new(),
        unrestorable_header_model_ids: Vec::new(),
        legacy_header_restore_markers: false,
        static_fingerprint: "".into(),
    }
}
fn header_slot(value: &str) -> HeaderSlot {
    HeaderSlot::Source(HeaderSource::Config(HeaderConfigRecord::from_pairs(vec![("x-bundle".into(), value.into())])))
}
fn model(provider: &str, id: &str, headers: HeaderSlot) -> HostModelRef {
    HostModel::new(Arc::new(VariantSpec::from_json(&spec(provider, id))), headers).unwrap()
}
fn header(h: &Harness, model: &HostModelRef, name: &str) -> Option<String> {
    let env = |_: &str| None;
    resolve_config_headers(
        model.headers().as_source(),
        &h.resolver,
        &ConfigValueContext { project_dir: h.directory.path(), environment: &env },
    )
    .and_then(|headers| headers.get(name).map(str::to_owned))
}
fn no_implicit() -> RegistryLoadOptions {
    let mut disabled = OrderedProviderSet::default();
    for provider in ["ollama", "llama.cpp", "lm-studio"] {
        disabled.insert(provider.into());
    }
    RegistryLoadOptions { disabled_providers: disabled, ..Default::default() }
}

#[test]
fn actual_config_loading_eager_order_failures_gateway_and_cancellation() {
    let h = Harness::new();
    let mut file = h.file(json!({"providers":{"probe":{"baseUrl":"https://config.test/v1","api":"openai-completions",
        "headers":{"x-auth":"!header"},"apiKey":"!key","models":[{"id":"custom","headers":{"x-custom":"!custom"}}],
        "modelOverrides":{"custom":{"headers":{"x-override":"!override","x-duplicate":"!custom"}}}}}}));
    h.host(&h.cache, |host| {
        let loader = RegistryLoader::load(&mut file, &no_implicit(), host).unwrap();
        let loaded = loader.loaded_config();
        assert!(loaded.found && loaded.error.is_none() && loaded.config().is_some() && loaded.mtime_ms.is_some());
        let provider = loaded.provider(&"probe".into()).unwrap();
        assert_eq!(provider.eager_key(), Some("installed-key"));
        assert_eq!(provider.command_values(), ["!key", "!header", "!custom", "!override"]);
        assert_eq!(*h.commands.calls.lock().unwrap(), ["header", "key"]);
        assert_eq!(*h.installed.lock().unwrap(), [("probe".into(), Some("installed-key".into()))]);
        assert!(provider.fields().get("apiKey").is_none() && provider.fields().get("headers").is_none());
        assert!(h.cache.reads.borrow().is_empty(), "config phase does not read unrelated rows");
    });

    let failed = Harness::new();
    let mut file = failed.file(json!({"providers":{"probe":{"apiKey":"!failed","headers":{"x-auth":"!failed"}}}}));
    failed.host(&failed.cache, |host| {
        let loaded = RegistryLoader::load(&mut file, &no_implicit(), host).unwrap();
        assert!(loaded.loaded_config().provider(&"probe".into()).unwrap().eager_key().is_none());
        file.invalidate();
        RegistryLoader::load(&mut file, &no_implicit(), host).unwrap();
        assert_eq!(
            *failed.commands.calls.lock().unwrap(),
            ["failed"],
            "failed helper is cached, not replayed by header then key or reload"
        );
        assert_eq!(*failed.installed.lock().unwrap(), [("probe".into(), None), ("probe".into(), None)]);
    });

    let gateway = Harness::new();
    let mut file = gateway.file(json!({"providers":{"probe":{"headers":{"x":"!header"},"apiKey":"!key"}}}));
    gateway.host(&gateway.cache, |host| {
        let mut options = no_implicit();
        options.ignore_local_model_config = true;
        let loader = RegistryLoader::load(&mut file, &options, host).unwrap();
        assert!(!loader.loaded_config().found && loader.loaded_config().config().is_none());
        assert!(gateway.commands.calls.lock().unwrap().is_empty() && gateway.installed.lock().unwrap().is_empty());
    });

    let cancelled = Harness::new();
    let mut file =
        cancelled.file(json!({"providers":{"probe":{"headers":{"first":"!stop","second":"!header"},"apiKey":"!key"}}}));
    cancelled.host(&cancelled.cache, |host| {
        assert!(matches!(RegistryLoader::load(&mut file, &no_implicit(), host), Err(RegistryLoaderError::Cancelled)));
        assert_eq!(*cancelled.commands.calls.lock().unwrap(), ["stop"]);
        assert!(cancelled.installed.lock().unwrap().is_empty());
    });
}

#[test]
fn missing_invalid_config_implicit_endpoints_and_pending_scope_are_distinct() {
    let mut h = Harness::new();
    h.env.insert("OLLAMA_HOST".into(), ":22000".into());
    h.env.insert("LM_STUDIO_BASE_URL".into(), "http://studio.test/root".into());
    let mut file = ModelsConfigFile::new(h.directory.path().join("missing.yml")).unwrap();
    h.host(&h.cache, |host| {
        let loader = RegistryLoader::load(&mut file, &Default::default(), host).unwrap();
        let loaded = loader.loaded_config();
        assert!(!loaded.found && loaded.error.is_none());
        let ollama =
            loaded.discoverable_providers.iter().find(|config| config.provider.equals_ascii("ollama")).unwrap();
        assert_eq!(ollama.base_url, Some("http://127.0.0.1:22000".into()));
        assert!(!ollama.optional && loaded.keyless_providers.contains(&"ollama".into()));
        let studio =
            loaded.discoverable_providers.iter().find(|config| config.provider.equals_ascii("lm-studio")).unwrap();
        assert_eq!(studio.base_url, Some("http://studio.test/root".into()));
        let pending = loader.pending_standard_providers();
        for provider in ["ollama", "opencode-go", "opencode-zen", "github-copilot"] {
            assert!(!pending.contains(&provider.into()));
        }
        assert!(pending.contains(&"anthropic".into()));
    });
    let path = h.directory.path().join("invalid.json");
    std::fs::write(&path, "{broken").unwrap();
    let mut file = ModelsConfigFile::new(path).unwrap();
    h.host(&h.cache, |host| {
        let loader = RegistryLoader::load(&mut file, &no_implicit(), host).unwrap();
        assert!(loader.loaded_config().found && loader.loaded_config().error.is_some());
        assert!(loader.loaded_config().config().is_none() && loader.loaded_config().providers.is_empty());
    });
    let mut file = h.file(json!({"providers":{"probe":{"headers":{"x":"!failed"}},"ollama":{"auth":"none"}}}));
    h.host(&h.cache, |host| {
        let loader = RegistryLoader::load(&mut file, &Default::default(), host).unwrap();
        assert!(!loader.loaded_config().provider(&"probe".into()).unwrap().override_present());
        assert!(
            !loader.loaded_config().discoverable_providers.iter().any(|config| config.provider.equals_ascii("ollama")),
            "an explicit provider without models or discovery suppresses implicit discovery"
        );
    });
}

#[test]
fn lazy_standard_cache_namespaces_markers_authority_and_callback_lock_boundary() {
    let h = Harness::new();
    let mut file = h.file(json!({}));
    let mut cache = row("wrong-provider", &["base", "variant", "blocked"]);
    let rows = cache.models.value.as_array().unwrap().to_vec();
    let mut variant = VariantSpec::from_wire(rows[1].clone());
    variant.set("requestModelId", WireValue::String("base".into()));
    cache.models =
        VariantSpec::from_wire(WireValue::Array(vec![rows[0].clone(), variant.to_wire_json(), rows[2].clone()]));
    cache.header_omitted_model_ids = vec!["base".into(), "variant".into(), "blocked".into()];
    cache.unrestorable_header_model_ids = vec!["blocked".into()];
    let base = model("openai", "base", header_slot("static-header"));
    // This group exercises header restoration against the same bundled
    // generation. A mismatched additive fingerprint intentionally drops base.
    let mut bundled = base.spec().as_ref().clone();
    bundled.set("headers", VariantSpec::from_json(&json!({"x-bundle":"static-header"})).to_wire_json());
    cache.static_fingerprint = fingerprint_static_models(&ModelArray::new(vec![Arc::new(bundled)]), false);
    h.cache.rows.borrow_mut().insert("openai".into(), cache.clone());
    h.host(&h.cache, |host| {
        let mut loader = RegistryLoader::load(&mut file, &no_implicit(), host).unwrap();
        assert!(h.cache.reads.borrow().is_empty());
        let slice =
            loader.load_standard_provider(&"openai".into(), None, std::slice::from_ref(&base), host, identity).unwrap();
        assert_eq!(
            slice.models.iter().filter_map(|model| text(model.spec(), "id")).collect::<Vec<_>>(),
            ["base".into(), "variant".into()]
        );
        assert_eq!(header(&h, &slice.models[1], "x-bundle"), Some("static-header".into()));
        assert!(
            slice.models.iter().all(|model| text(model.spec(), "provider") == Some("openai".into())
                && model.spec().get("headers").is_none())
        );
        let again = loader.load_standard_provider(&"openai".into(), None, &[], host, identity).unwrap();
        assert!(!again.newly_loaded && Arc::ptr_eq(&slice.models[0], &again.models[0]));
        assert_eq!(*h.cache.reads.borrow(), [("openai".into(), STARTUP_CACHE_TTL_MS)]);
        let credentials = loader.load_standard_provider(&"github-copilot".into(), None, &[], host, identity).unwrap();
        assert!(!credentials.newly_loaded && h.cache.reads.borrow().len() == 1);
    });

    cache.unrestorable_header_model_ids = vec!["variant".into()];
    cache.legacy_header_restore_markers = true;
    h.cache.rows.borrow_mut().insert("openai".into(), cache);
    h.host(&h.cache, |host| {
        let mut loader = RegistryLoader::load(&mut file, &no_implicit(), host).unwrap();
        let slice = loader.load_standard_provider(&"openai".into(), None, &[base], host, identity).unwrap();
        assert_eq!(slice.models.len(), 2, "legacy requestModelId marker can recover; a missing donor cannot");
    });

    let shared = Mutex::new(SqliteModelCache::open(h.directory.path().join("models.db")).unwrap());
    let mut project = VariantSpec::from_json(&spec("google-vertex", "project"));
    project.set("baseUrl", WireValue::String("https://vertex.test/projects/p/locations/l/endpoints/openapi".into()));
    shared
        .lock()
        .unwrap()
        .write_model_cache_wire(
            &"google-vertex".into(),
            1000.0,
            &[Arc::new(project)],
            WireModelCacheWriteOptions {
                authoritative: true,
                static_fingerprint: &"".into(),
                static_header_sources: &[],
                restorable_header_fallback: None,
            },
        )
        .unwrap();
    h.host(&shared, |host| {
        let mut loader = RegistryLoader::load(&mut file, &no_implicit(), host).unwrap();
        let slice = loader
            .load_standard_provider(&"google-vertex".into(), None, &[], host, |_, models| {
                assert!(shared.try_lock().is_ok(), "prepare callback must not hold the cache mutex");
                Ok(models)
            })
            .unwrap();
        assert!(slice.authoritative && loader.authoritative_provider_snapshot().contains(&"google-vertex".into()));
    });
}

#[test]
fn shared_catalog_fingerprints_and_failed_full_batch_keep_native_ordering() {
    let h = Harness::new();
    let mut file = h.file(json!({}));
    let bundle = model("anthropic", "same", HeaderSlot::Absent);
    let mut cache = row("anthropic", &["same", "addition"]);
    cache.static_fingerprint = "outdated".into();
    h.cache.rows.borrow_mut().insert("anthropic".into(), cache.clone());
    h.host(&h.cache, |host| {
        let mut loader = RegistryLoader::load(&mut file, &no_implicit(), host).unwrap();
        let slice = loader
            .load_standard_provider(&"anthropic".into(), None, std::slice::from_ref(&bundle), host, identity)
            .unwrap();
        assert_eq!(slice.models.len(), 1, "additive stale static fingerprint cannot replace a same-id bundled row");
        let state = slice.discovery_state.unwrap();
        assert_eq!(state.status, ProviderDiscoveryStatus::Cached);
        assert_eq!(state.source, Some(ModelResolutionSource::Cache));
        assert_eq!(state.models, ["addition".into()]);
    });
    let fingerprint = fingerprint_static_models(&ModelArray::new(vec![bundle.spec().clone()]), false);
    cache.static_fingerprint = {
        let mut fingerprint = fingerprint.clone();
        fingerprint.append_str(":drop:fixture");
        fingerprint
    };
    h.cache.rows.borrow_mut().insert("anthropic".into(), cache);
    h.host(&h.cache, |host| {
        let mut loader = RegistryLoader::load(&mut file, &no_implicit(), host).unwrap();
        let slice = loader.load_standard_provider(&"anthropic".into(), None, &[bundle], host, identity).unwrap();
        assert_eq!(slice.models.len(), 2, "matching :drop: fingerprint preserves provider endpoint metadata");
    });

    let mut malformed = row("openai", &[]);
    malformed.models = VariantSpec::from_json(&json!({"unexpected":"object"}));
    h.cache.rows.borrow_mut().insert("openai".into(), malformed);
    h.cache.reads.borrow_mut().clear();
    h.host(&h.cache, |host| {
        let mut loader = RegistryLoader::load(&mut file, &no_implicit(), host).unwrap();
        let mut filter = OrderedProviderSet::default();
        filter.insert("openai".into());
        filter.insert("anthropic".into());
        let selected: Vec<_> =
            loader.startup_roster().iter().filter(|provider| filter.contains(provider)).cloned().collect();
        // Put malformed rows at the first descriptor position to observe the failing batch gate.
        h.cache.rows.borrow_mut().insert(selected[0].clone(), {
            let mut entry = row("x", &[]);
            entry.models = VariantSpec::from_json(&json!({}));
            entry
        });
        assert!(matches!(
            loader.load_standard_providers(Some(&filter), host, |_| Ok((None, Vec::new())), identity),
            Err(RegistryLoaderError::InvalidCacheModels)
        ));
        assert_eq!(h.cache.reads.borrow().len(), 1);
        assert!(
            loader
                .load_standard_providers(Some(&filter), host, |_| Ok((None, Vec::new())), identity)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            h.cache.reads.borrow().len(),
            1,
            "the next query does not replay any member of the failed drained batch"
        );
    });

    h.host(&h.cache, |host| {
        let mut loader = RegistryLoader::load(&mut file, &no_implicit(), host).unwrap();
        let mut filter = OrderedProviderSet::default();
        filter.insert("openai".into());
        filter.insert("anthropic".into());
        let selected: Vec<_> =
            loader.startup_roster().iter().filter(|provider| filter.contains(provider)).cloned().collect();
        h.cache.rows.borrow_mut().insert(selected[0].clone(), row("fixture", &["first"]));
        h.cache.rows.borrow_mut().insert(selected[1].clone(), row("fixture", &["second"]));
        assert!(matches!(
            loader.load_standard_providers(
                Some(&filter),
                host,
                |_| Ok((None, Vec::new())),
                |provider, models| {
                    if provider == &selected[1] {
                        Err(CollapseError::new("controlled preparation failure".into()))
                    } else {
                        Ok(models)
                    }
                }
            ),
            Err(RegistryLoaderError::ModelPreparation)
        ));
        assert!(
            loader.cached_standard_models_snapshot().is_empty(),
            "a later failure prevents partial cache publication"
        );
        assert!(loader.authoritative_provider_snapshot().iter().next().is_none());
    });
}

#[test]
fn configured_cache_private_header_repair_normalization_staleness_and_best_effort_faults() {
    let mut h = Harness::new();
    h.env.insert("OLLAMA_CONTEXT_LENGTH".into(), "0x10000".into());
    let mut file = h.file(json!({"providers":{
        "probe":{"api":"openai-completions","baseUrl":"https://config.test/v1","apiKey":"configured-secret","authHeader":true,"discovery":{"type":"openai-models-list"}},
        "ollama":{"api":"openai-responses","baseUrl":"http://ollama.test:11434","discovery":{"type":"ollama"},"remoteCompaction":{"endpoint":"https://compact.test"}},
        "bare":{"api":"openai-completions","baseUrl":"https://bare.test/v3/compat/?query=x#hash","discovery":{"type":"openai-models-list","injectV1":false}},
        "litellm":{"api":"openai-completions","baseUrl":"https://lite.test/root","discovery":{"type":"litellm"}}
    }}));
    let database_path = h.directory.path().join("independent-cache.db");
    let cache = SqliteModelCache::open(&database_path).unwrap();
    let mut raw = VariantSpec::from_json(&spec("probe", "offline"));
    raw.set("headers", VariantSpec::from_json(&json!({"Authorization":"Bearer configured-secret"})).to_wire_json());
    cache
        .write_model_cache_wire(
            &"probe:openai-models-list-context-v3".into(),
            1000.0,
            &[Arc::new(raw)],
            WireModelCacheWriteOptions {
                authoritative: true,
                static_fingerprint: &"fingerprint".into(),
                static_header_sources: &[],
                restorable_header_fallback: None,
            },
        )
        .unwrap();
    cache
        .write_model_cache_wire(
            &"ollama:ollama-models-v1:unused".into(),
            1000.0,
            &[],
            WireModelCacheWriteOptions {
                authoritative: true,
                static_fingerprint: &"".into(),
                static_header_sources: &[],
                restorable_header_fallback: None,
            },
        )
        .unwrap();
    h.host(&cache, |host| {
        let mut loader = RegistryLoader::load(&mut file, &no_implicit(), host).unwrap();
        let loaded = loader.load_discoverable_cache(host, identity).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(header(&h, &loaded[0], "Authorization"), Some("Bearer configured-secret".into()));
        assert!(loaded[0].spec().get("headers").is_none() && loaded[0].spec().get("apiKey").is_none());
        let repaired = cache
            .read_model_cache_wire(&"probe:openai-models-list-context-v3".into(), STARTUP_CACHE_TTL_MS, || 1000.0)
            .unwrap()
            .unwrap();
        assert!(repaired.unrestorable_header_model_ids.is_empty());
        assert!(!repaired.models.to_wire_json().stringify().contains("configured-secret"));
        assert_eq!(repaired.header_omitted_model_ids, ["offline".into()]);
        assert!(
            loader.discovery_states().iter().find(|state| state.provider.equals_ascii("probe")).unwrap().stale,
            "configuration mtime is newer than this old cache"
        );
        let bare = loader
            .loaded_config()
            .discoverable_providers
            .iter()
            .find(|provider| provider.provider.equals_ascii("bare"))
            .unwrap();
        assert_eq!(bare.cache_provider_id(), "bare:openai-models-list-bare-context-v3".into());
        assert_eq!(
            text(loader.loaded_config().provider(&"bare".into()).unwrap().fields(), "baseUrl"),
            Some("https://bare.test/v3/compat".into())
        );
        let lite = loader
            .loaded_config()
            .discoverable_providers
            .iter()
            .find(|provider| provider.provider.equals_ascii("litellm"))
            .unwrap();
        assert_eq!(lite.cache_provider_id(), "litellm:litellm-rich-v4".into());
        let ollama = loader
            .loaded_config()
            .discoverable_providers
            .iter()
            .find(|provider| provider.provider.equals_ascii("ollama"))
            .unwrap();
        let normalized = ara_cli::model_registry_loader::normalize_discoverable(
            ollama,
            vec![model("ollama", "loaded", HeaderSlot::Absent)],
            host.provider_environment,
        )
        .unwrap();
        assert_eq!(text(normalized[0].spec(), "api"), Some("openai-responses".into()));
        assert_eq!(text(normalized[0].spec(), "imageInputDecoder"), Some("stb".into()));
        assert_eq!(normalized[0].spec().get("contextWindow"), Some(&WireValue::Number(65536.0)));
        assert_eq!(normalized[0].spec().get("maxTokens"), Some(&WireValue::Number(32768.0)));
    });
    h.cache.fail_reads.store(true, Ordering::SeqCst);
    h.host(&h.cache, |host| {
        let mut loader = RegistryLoader::load(&mut file, &no_implicit(), host).unwrap();
        assert!(loader.load_discoverable_cache(host, identity).unwrap().is_empty());
        assert!(loader.take_notices().iter().all(|notice| notice.stage == RegistryLoaderError::CacheRead));
        assert!(loader.discovery_states().iter().all(|state| state.status == ProviderDiscoveryStatus::Idle));
    });
    h.cache.fail_reads.store(false, Ordering::SeqCst);
    h.cache.fail_repairs.store(true, Ordering::SeqCst);
    let mut repair = row("probe", &["offline"]);
    repair.header_omitted_model_ids = vec!["offline".into()];
    repair.unrestorable_header_model_ids = vec!["offline".into()];
    h.cache.rows.borrow_mut().insert("probe:openai-models-list-context-v3".into(), repair);
    h.host(&h.cache, |host| {
        let mut loader = RegistryLoader::load(&mut file, &no_implicit(), host).unwrap();
        assert_eq!(
            loader.load_discoverable_cache(host, identity).unwrap().len(),
            1,
            "failed repair write does not discard recovered in-memory credentials"
        );
        assert_eq!(loader.take_notices()[0].stage, RegistryLoaderError::CacheWrite);
    });
}
