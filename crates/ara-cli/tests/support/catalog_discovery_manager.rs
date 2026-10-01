//! Native replay for unchanged-source manager and cache namespace fixtures.
use super::*;
use ara_cli::{
    bun_hash,
    model_cache::{SqliteModelCache, WireCacheEntry, WireModelCacheWriteOptions},
    model_collapse::CollapseModelPolicy,
    model_manager::*,
    model_wire_policy::WireModelPolicy,
    provider_models::{cache_provider_id, descriptor_types::ProviderEnvironment},
};
use std::sync::atomic::{AtomicU64, Ordering};

#[tokio::test]
async fn static_models_nullish_falls_back_but_empty_array_overrides() {
    // Independently observed at fixed OMP 596f2da model-manager.ts with Bun:
    // review-manager/static-null-source-receipt.json, four original inputs.
    let expected = [
        "deepseek-r1-distill-llama-70b",
        "gemma2-9b-it",
        "groq/compound",
        "groq/compound-mini",
        "llama-3.1-8b-instant",
        "llama-3.3-70b-versatile",
        "llama3-70b-8192",
        "llama3-8b-8192",
        "meta-llama/llama-4-maverick-17b-128e-instruct",
        "meta-llama/llama-4-scout-17b-16e-instruct",
        "mistral-saba-24b",
        "moonshotai/kimi-k2-instruct",
        "moonshotai/kimi-k2-instruct-0905",
        "openai/gpt-oss-120b",
        "openai/gpt-oss-20b",
        "openai/gpt-oss-safeguard-20b",
        "qwen-qwq-32b",
        "qwen/qwen3-32b",
        "qwen/qwen3.6-27b",
        "qwen/qwen3.8-27b",
    ];
    let directory = tempfile::tempdir().unwrap();
    for (name, override_models, bundled) in [
        ("omitted", None, true),
        ("null", Some(RawModelValue::null()), true),
        ("undefined", Some(RawModelValue::Undefined), true),
        ("empty-array", Some(RawModelValue::models(Vec::new())), false),
    ] {
        let transport = Arc::new(FixtureTransport::new(&W::Object(Vec::new())));
        let context = CatalogContext::new(transport.clone()).unwrap();
        let mut options = ModelManagerOptions::new("groq");
        options.static_models = override_models;
        let manager = ModelManager::new(
            options,
            context,
            Arc::new(ModelManagerHost {
                cache: Arc::new(Mutex::new(SqliteModelCache::for_path(directory.path().join(name)))),
                clock: Arc::new(|| 1_740_000_000_000.0),
            }),
        );
        let result = manager.refresh(ModelRefreshStrategy::Offline).await.unwrap();
        let ids: Vec<_> = result.models.iter().map(|model| text(model.get("id").unwrap())).collect();
        let expected_ids = if bundled { expected.as_slice() } else { &[] };
        assert_eq!(ids, expected_ids, "staticModels input: {name}");
        assert_eq!(result.source, ModelResolutionSource::Bundled, "staticModels input: {name}");
        assert!(!result.stale, "staticModels input: {name}");
        assert!(transport.calls.lock().unwrap().is_empty());
    }
}

#[test]
fn models_dev_mapper_skips_own_undefined_values() {
    use ara_cli::provider_models::{ModelsDevProviderDescriptor, map_models_dev_to_models};

    let source = VariantSpec {
        value: W::object(vec![(
            "groq",
            W::object(vec![(
                "models",
                W::object(vec![("skip", W::Null), ("nil", W::Null), ("scalar", W::Number(3.0))]),
            )]),
        )]),
        undefined_paths: vec![vec!["groq".into(), "models".into(), "skip".into()]],
    };
    let descriptor = ModelsDevProviderDescriptor::new("groq", "groq", "openai-completions", "https://fixture.test");
    let models = map_models_dev_to_models(&source, &[descriptor]).unwrap();
    assert!(models.is_empty());
}

#[tokio::test]
async fn codex_account_setup_failure_returns_while_sibling_discovery_continues() {
    use ara_cli::provider_models::{
        CodexAccountResolver, OpenAiCodexAccount, OpenAiCodexModelManagerConfig, ProviderFactoryHost,
        openai_codex_model_manager_options,
    };
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    struct Accounts;
    #[async_trait]
    impl CodexAccountResolver for Accounts {
        async fn resolve_accounts(&self) -> Result<Option<Vec<OpenAiCodexAccount>>, DiscoveryError> {
            Ok(Some(vec![
                OpenAiCodexAccount { access_token: "bad\nkey".into(), account_id: Some("bad".into()) },
                OpenAiCodexAccount { access_token: "ok".into(), account_id: Some("good".into()) },
            ]))
        }
    }
    struct HeldCatalog {
        calls: AtomicU64,
        started: AtomicBool,
        completed: AtomicBool,
        release: DiscoverySignal,
        finished: DiscoverySignal,
    }
    #[async_trait]
    impl DiscoveryTransport for HeldCatalog {
        fn context_id(&self) -> u64 {
            31_337
        }
        async fn fetch(&self, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
            assert_eq!(
                request.url.to_utf8().unwrap(),
                "https://chatgpt.com/backend-api/codex/models?client_version=0.153.0"
            );
            assert!(
                request
                    .headers
                    .iter()
                    .any(|(key, value)| { key.equals_ascii("authorization") && value.equals_ascii("Bearer ok") })
            );
            assert!(
                request
                    .headers
                    .iter()
                    .any(|(key, value)| { key.equals_ascii("chatgpt-account-id") && value.equals_ascii("good") })
            );
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.store(true, Ordering::SeqCst);
            let _ = self.release.cancelled().await;
            self.completed.store(true, Ordering::SeqCst);
            self.finished.abort(DiscoveryError::new("sibling completed"));
            Ok(DiscoveryReply {
                status: 200,
                headers: Vec::new(),
                body: br#"{"models":[]}"#.to_vec(),
                json_override: None,
            })
        }
    }

    // Fixed source Promise.all rejects an invalid account header while its
    // already-started sibling stays pending. Source receipt: remaining19-source-probe-receipt.json.
    let transport = Arc::new(HeldCatalog {
        calls: AtomicU64::new(0),
        started: AtomicBool::new(false),
        completed: AtomicBool::new(false),
        release: DiscoverySignal::default(),
        finished: DiscoverySignal::default(),
    });
    let context = CatalogContext::new(transport.clone()).unwrap();
    let host = ProviderFactoryHost::new(Arc::new(|_: &str| None), "fixture", PathBuf::from("."));
    let options = openai_codex_model_manager_options(
        &context,
        OpenAiCodexModelManagerConfig { resolve_accounts: Some(Arc::new(Accounts)), ..Default::default() },
        host,
    );
    let result = tokio::time::timeout(Duration::from_secs(1), options.dynamic_fetcher.unwrap().fetch()).await;
    assert!(transport.started.load(Ordering::SeqCst), "every account must start before the first rejection");
    assert!(!transport.completed.load(Ordering::SeqCst), "sibling must still be held when discovery rejects");
    transport.release.abort(DiscoveryError::new("release sibling"));
    let error = match result.expect("invalid header must settle before releasing the sibling request") {
        Err(error) => error,
        Ok(_) => panic!("invalid account header must reject discovery"),
    };
    assert!(error.name.equals_ascii("TypeError"));
    assert!(error.message.to_utf8().unwrap().contains("Authorization"));
    tokio::time::timeout(Duration::from_secs(1), transport.finished.cancelled())
        .await
        .expect("already-started sibling must continue after discovery rejects");
    assert!(transport.completed.load(Ordering::SeqCst));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
}

fn bool_prop(value: &W, key: &str) -> bool {
    matches!(value.get(key), Some(W::Bool(true)))
}
pub(super) fn finish(transport: &FixtureTransport, context: &CatalogContext, result: W) -> W {
    let mut result = transport.finish(result);
    let logs = context
        .runtime
        .diagnostics
        .lock()
        .unwrap()
        .iter()
        .map(|log| {
            let args = specs_value(&log.arguments.iter().cloned().map(Arc::new).collect::<Vec<_>>());
            W::object(vec![("level", s(log.level.clone())), ("args", encode(&args))])
        })
        .collect();
    result.insert("logs", W::Array(logs));
    result
}
pub(super) async fn encode_options(built: ModelManagerOptions, test: &W) -> Result<VariantSpec, DiscoveryError> {
    let mut result = VariantSpec::from_wire(W::object(vec![
        ("providerId", s(built.provider_id)),
        ("authoritative", W::Bool(built.dynamic_models_authoritative)),
        ("cacheProviderId", built.cache_provider_id.map(s).unwrap_or(W::Null)),
        (
            "dropIds",
            built
                .drop_cached_model_ids_on_static_mismatch
                .map(|ids| W::Array(ids.into_iter().map(s).collect()))
                .unwrap_or(W::Null),
        ),
    ]));
    if let Some(models) = built.static_models {
        insert_record(&mut result, "staticModels", &raw_spec(&models));
    } else {
        result.set("staticModels", W::Null);
    }
    result.set("hasDynamic", W::Bool(built.dynamic_fetcher.is_some()));
    if !matches!(test.get("runDynamic"), Some(W::Bool(false))) {
        if let Some(fetch) = built.dynamic_fetcher {
            insert_record(&mut result, "models", &raw_spec(&fetch.fetch().await?));
        } else {
            result.set_undefined("models");
        }
    } else {
        result.set_undefined("models");
    }
    if let Some(payload) = test.get("devPayload") {
        let hook = built.models_dev.expect("factory models.dev hook");
        let models = hook
            .map(RawModelValue::Value(VariantSpec::from_wire(payload.clone())), &string(&result.value, "providerId"))?;
        insert_record(&mut result, "devModels", &raw_spec(&models));
        result.set("devAdditive", if hook.additive_only() { W::Bool(true) } else { W::Null });
    }
    Ok(result)
}
struct RetainedSignalTransport {
    inner: Arc<FixtureTransport>,
    signals: Mutex<Vec<Option<DiscoverySignal>>>,
}
struct HeldShowTransport {
    inner: Arc<FixtureTransport>,
    started: std::sync::atomic::AtomicBool,
    completed: std::sync::atomic::AtomicBool,
    release: DiscoverySignal,
}
#[async_trait]
impl DiscoveryTransport for HeldShowTransport {
    fn context_id(&self) -> u64 {
        self.inner.context_id()
    }
    async fn fetch(&self, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        let show = request.url.units().ends_with(&"/api/show".encode_utf16().collect::<Vec<_>>());
        if show {
            self.started.store(true, Ordering::SeqCst);
        }
        let response = self.inner.fetch(request).await;
        if show {
            let _ = self.release.cancelled().await;
            self.completed.store(true, Ordering::SeqCst);
        }
        response
    }
}
#[async_trait]
impl DiscoveryTransport for RetainedSignalTransport {
    fn context_id(&self) -> u64 {
        self.inner.context_id()
    }
    async fn fetch(&self, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        self.signals.lock().unwrap().push(request.signal.clone());
        self.inner.fetch(request).await
    }
}
fn raw(value: Option<&W>) -> RawModelValue {
    value.map_or(RawModelValue::Undefined, |value| RawModelValue::Value(VariantSpec::from_wire(value.clone())))
}
fn raw_spec(value: &RawModelValue) -> VariantSpec {
    match value {
        RawModelValue::Undefined => VariantSpec { value: W::Null, undefined_paths: vec![Vec::new()] },
        RawModelValue::Value(value) => value.clone(),
        RawModelValue::Models(value) => specs_value(&value.snapshot()),
    }
}
fn spec_error(value: &W) -> DiscoveryError {
    DiscoveryError::named(
        value.get("name").and_then(W::as_string).cloned().unwrap_or_else(|| "Error".into()),
        string(value, "message"),
    )
}
struct Clock {
    now: AtomicU64,
    trace: Arc<Mutex<Vec<W>>>,
}
impl ModelClock for Clock {
    fn now(&self) -> f64 {
        let value = f64::from_bits(self.now.load(Ordering::SeqCst));
        self.trace.lock().unwrap().push(W::object(vec![("hook", s("now")), ("value", W::Number(value))]));
        value
    }
}
#[derive(Clone)]
struct Writer {
    cache: Arc<Mutex<SqliteModelCache>>,
    key: WireString,
    fingerprint: WireString,
    static_models: Vec<SpecRef>,
    fallback: Option<VariantSpec>,
    clock: Arc<Clock>,
}
impl Writer {
    fn write(&self, entry: &W) {
        let models = entry
            .get("models")
            .and_then(W::as_array)
            .unwrap_or_default()
            .iter()
            .map(|value| Arc::new(WireModelPolicy.build(&VariantSpec::from_wire(value.clone())).unwrap()))
            .collect::<Vec<_>>();
        let fingerprint = if bool_prop(entry, "matchingFingerprint") {
            self.fingerprint.clone()
        } else {
            entry.get("fingerprint").and_then(W::as_string).cloned().unwrap_or_else(|| "".into())
        };
        self.cache
            .lock()
            .unwrap()
            .write_model_cache_wire(
                &self.key,
                entry
                    .get("updatedAt")
                    .and_then(W::as_number)
                    .unwrap_or_else(|| f64::from_bits(self.clock.now.load(Ordering::SeqCst))),
                &models,
                WireModelCacheWriteOptions {
                    authoritative: bool_prop(entry, "authoritative"),
                    static_fingerprint: &fingerprint,
                    static_header_sources: &self.static_models,
                    restorable_header_fallback: self.fallback.as_ref(),
                },
            )
            .unwrap();
    }
}
struct Dynamic {
    steps: Mutex<VecDeque<W>>,
    last: W,
    index: Mutex<usize>,
    writer: Writer,
    trace: Arc<Mutex<Vec<W>>>,
}
#[async_trait]
impl DynamicModelFetcher for Dynamic {
    async fn fetch(&self) -> Result<RawModelValue, DiscoveryError> {
        let step = self.steps.lock().unwrap().pop_front().unwrap_or_else(|| self.last.clone());
        let mut index = self.index.lock().unwrap();
        self.trace.lock().unwrap().push(W::object(vec![("hook", s("dynamic")), ("index", n(*index))]));
        *index += 1;
        drop(index);
        if let Some(latest) = step.get("injectLatestCache") {
            self.writer.write(latest);
        }
        if let Some(error) = step.get("error") {
            return Err(spec_error(error));
        }
        Ok(if let Some(encoded) = step.get("encoded") {
            let decoded = decode(encoded);
            if decoded.undefined_paths.contains(&Vec::new()) {
                RawModelValue::Undefined
            } else {
                RawModelValue::Value(decoded)
            }
        } else if bool_prop(&step, "undefined") {
            RawModelValue::Undefined
        } else {
            raw(step.get("value"))
        })
    }
}
struct Dev {
    steps: Mutex<VecDeque<W>>,
    last: W,
    current: Mutex<Option<W>>,
    index: Mutex<usize>,
    additive: bool,
    trace: Arc<Mutex<Vec<W>>>,
}
#[async_trait]
impl ModelsDevFallback for Dev {
    async fn fetch(&self) -> Result<RawModelValue, DiscoveryError> {
        let step = self.steps.lock().unwrap().pop_front().unwrap_or_else(|| self.last.clone());
        let mut index = self.index.lock().unwrap();
        self.trace.lock().unwrap().push(W::object(vec![("hook", s("modelsDev.fetch")), ("index", n(*index))]));
        *index += 1;
        drop(index);
        *self.current.lock().unwrap() = Some(step.clone());
        if let Some(error) = step.get("error") {
            return Err(spec_error(error));
        }
        Ok(raw(step.get("payload")))
    }
    fn map(&self, payload: RawModelValue, provider: &WireString) -> Result<RawModelValue, DiscoveryError> {
        self.trace.lock().unwrap().push(W::object(vec![
            ("hook", s("modelsDev.map")),
            ("payload", encode(&raw_spec(&payload))),
            ("providerId", s(provider.clone())),
        ]));
        let step = self.current.lock().unwrap().clone().unwrap();
        if let Some(error) = step.get("mapError") {
            return Err(spec_error(error));
        }
        Ok(raw(step.get("mapped")))
    }
    fn additive_only(&self) -> bool {
        self.additive
    }
}
fn cache_value(cache: Option<WireCacheEntry>) -> VariantSpec {
    let Some(cache) = cache else {
        return VariantSpec::from_wire(W::Null);
    };
    let mut out = VariantSpec::from_wire(W::Object(Vec::new()));
    insert_record(&mut out, "models", &cache.models);
    out.set("fresh", W::Bool(cache.fresh));
    out.set("authoritative", W::Bool(cache.authoritative));
    out.set("updatedAt", W::Number(cache.updated_at));
    out.set("headerOmittedModelIds", W::Array(cache.header_omitted_model_ids.into_iter().map(s).collect()));
    out.set("unrestorableHeaderModelIds", W::Array(cache.unrestorable_header_model_ids.into_iter().map(s).collect()));
    out.set("legacyHeaderRestoreMarkers", W::Bool(cache.legacy_header_restore_markers));
    out.set("staticFingerprint", s(cache.static_fingerprint));
    out
}
pub async fn replay(test: &W) -> W {
    let transport = Arc::new(FixtureTransport::new(test));
    let context = CatalogContext::new(transport.clone()).unwrap();
    let empty_options = W::Object(Vec::new());
    let options = test.get("options").unwrap_or(&empty_options);
    if text(field(test, "family")) == "factory" {
        if text(field(test, "op")) == "public-alibaba-base-url" {
            return finish(
                &transport,
                &context,
                success(&VariantSpec::from_wire(s(ara_cli::provider_models::ALIBABA_TOKEN_PLAN_BASE_URL))),
            );
        }
        if text(field(test, "op")) == "reference-resolver" {
            use ara_cli::provider_models::bundled_references::{ProviderReferenceSource, create_reference_resolver};
            let models: HashMap<_, _> = array(field(options, "models"))
                .iter()
                .map(|model| (string(model, "id"), Arc::new(VariantSpec::from_wire(model.clone()))))
                .collect();
            let calls = Arc::new(AtomicU64::new(0));
            let saved = calls.clone();
            let source = if text(field(test, "mode")) == "lazy" {
                ProviderReferenceSource::Lazy(Arc::new(move || {
                    saved.fetch_add(1, Ordering::SeqCst);
                    models.clone()
                }))
            } else {
                ProviderReferenceSource::Map(models)
            };
            let resolve = create_reference_resolver(source);
            let mut result =
                VariantSpec::from_wire(W::object(vec![("callsBefore", n(calls.load(Ordering::SeqCst) as usize))]));
            let models = array(field(options, "ids"))
                .iter()
                .map(|id| {
                    resolve
                        .resolve(id.as_string().unwrap())
                        .unwrap_or_else(|| Arc::new(VariantSpec { value: W::Null, undefined_paths: vec![Vec::new()] }))
                })
                .collect::<Vec<_>>();
            insert_record(&mut result, "models", &specs_value(&models));
            result.set("callsAfter", n(calls.load(Ordering::SeqCst) as usize));
            return finish(&transport, &context, success(&result));
        }
        if text(field(test, "op")) == "public-helper" {
            use ara_cli::provider_models::*;
            let args = decode(field(test, "argsEncoded"));
            let args = array(&args.value);
            let id = args[0].as_string().unwrap();
            let value = match text(field(test, "helper")).as_str() {
                "isLikelyAimlApiChatModelId" => W::Bool(is_likely_aiml_api_chat_model_id(id)),
                "isLikelySiliconFlowChatModelId" => W::Bool(is_likely_siliconflow_chat_model_id(id)),
                "kimiCodeMaxTokens" => kimi_code_max_tokens(id, args.get(1).cloned()),
                "clampFireworksKimiMaxTokens" => match clamp_fireworks_kimi_max_tokens(id, args[1].clone()) {
                    Ok(v) => v,
                    Err(error) => return finish(&transport, &context, failure(error)),
                },
                "clampKimiK27CodeMaxTokens" => match clamp_kimi_k27_code_max_tokens(id, args[1].clone()) {
                    Ok(v) => v,
                    Err(error) => return finish(&transport, &context, failure(error)),
                },
                name => panic!("unknown helper {name}"),
            };
            return finish(&transport, &context, success(&VariantSpec::from_wire(value)));
        }
        if text(field(test, "op")) == "non-openai-public-values" {
            let mut result = VariantSpec::from_wire(W::object(Vec::new()));
            insert_record(
                &mut result,
                "DEVIN_STATIC_MODELS",
                &specs_value(&ara_cli::provider_models::DEVIN_STATIC_MODELS),
            );
            insert_record(
                &mut result,
                "CLINE_PASS_MODEL_METADATA",
                &ara_cli::provider_models::cline_pass::CLINE_PASS_MODEL_METADATA,
            );
            return finish(&transport, &context, success(&result));
        }
        if text(field(test, "op")) == "catalog-session-fetch-identity" {
            use ara_cli::provider_models::{
                catalog_session::fetch_well_known_models, descriptor_types::ProviderFactoryHost,
            };
            let a = CatalogContext::new(Arc::new(RetainedSignalTransport {
                inner: transport.clone(),
                signals: Mutex::new(Vec::new()),
            }))
            .unwrap();
            let b = CatalogContext::new(Arc::new(RetainedSignalTransport {
                inner: transport.clone(),
                signals: Mutex::new(Vec::new()),
            }))
            .unwrap();
            let host =
                ProviderFactoryHost::new(Arc::new(|_: &str| None), "omp/18.1.8", std::env::current_dir().unwrap());
            let mut result = VariantSpec::from_wire(W::object(Vec::new()));
            for (name, context) in [("a1", &a), ("a2", &a), ("b1", &b), ("b2", &b)] {
                match fetch_well_known_models(context, &host, true, None).await {
                    Ok(value) => insert_record(&mut result, name, &value),
                    Err(error) => return finish(&transport, context, failure(error)),
                };
            }
            return finish(&transport, &context, success(&result));
        }
        if text(field(test, "op")) == "ollama-first-rejection-background" {
            use ara_cli::provider_models::*;
            let held = Arc::new(HeldShowTransport {
                inner: transport.clone(),
                started: std::sync::atomic::AtomicBool::new(false),
                completed: std::sync::atomic::AtomicBool::new(false),
                release: DiscoverySignal::default(),
            });
            let context = CatalogContext::new(held.clone()).unwrap();
            let config = ModelManagerConfig {
                api_key: options.get("apiKey").and_then(W::as_string).cloned(),
                base_url: options.get("baseUrl").and_then(W::as_string).cloned(),
                context: Some(context.clone()),
                ..Default::default()
            };
            let host =
                ProviderFactoryHost::new(Arc::new(|_: &str| None), "omp/18.1.8", std::env::current_dir().unwrap());
            let built = if bool_prop(test, "cloud") {
                ollama_cloud_model_manager_options(&context, config)
            } else {
                ollama_model_manager_options(&context, config, &host)
            };
            let outcome = Arc::new(Mutex::new(None));
            let saved = outcome.clone();
            let fetch = built.dynamic_fetcher.unwrap();
            let pending = tokio::spawn(async move {
                *saved.lock().unwrap() = Some(match fetch.fetch().await {
                    Ok(models) => W::object(vec![("status", s("ok")), ("models", raw_spec(&models).value)]),
                    Err(error) => failure(error),
                });
            });
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let mut result = VariantSpec::from_wire(W::object(vec![
                ("showStarted", W::Bool(held.started.load(Ordering::SeqCst))),
                ("showCompletedBeforeRelease", W::Bool(held.completed.load(Ordering::SeqCst))),
                (
                    "rejectedBeforeRelease",
                    W::Bool(outcome.lock().unwrap().as_ref().is_some_and(|v| text(field(v, "status")) == "error")),
                ),
            ]));
            held.release.abort(DiscoveryError::new("fixture released show"));
            pending.await.unwrap();
            for _ in 0..100 {
                if held.completed.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
            assert!(held.completed.load(Ordering::SeqCst), "background Ollama show did not complete after release");
            result.set("outcome", outcome.lock().unwrap().clone().unwrap());
            return finish(&transport, &context, success(&result));
        }
        if text(field(test, "op")) == "public-values" {
            use ara_cli::provider_models::*;
            let mut result = VariantSpec::from_wire(W::object(vec![
                ("DEEPINFRA_BASE_URL", s(DEEPINFRA_BASE_URL)),
                ("KIMI_K27_CODE_RECOMMENDED_MAX_TOKENS", W::Number(KIMI_K27_CODE_RECOMMENDED_MAX_TOKENS)),
                ("KIMI_CODE_DEFAULT_MAX_TOKENS", W::Number(KIMI_CODE_DEFAULT_MAX_TOKENS)),
            ]));
            for (name, models) in [
                ("ANTHROPIC_CURATED_FALLBACK_MODELS", &*ANTHROPIC_CURATED_FALLBACK_MODELS),
                ("OPENAI_DAYBREAK_CURATED_FALLBACK_MODELS", &*OPENAI_DAYBREAK_CURATED_FALLBACK_MODELS),
                ("GMI_CLOUD_STATIC_MODELS", &*GMI_CLOUD_STATIC_MODELS),
                ("XAI_OAUTH_CURATED_MODELS", &*XAI_OAUTH_CURATED_MODELS),
                ("ALIBABA_TOKEN_PLAN_STATIC_MODELS", &*ALIBABA_TOKEN_PLAN_STATIC_MODELS),
                ("META_MUSE_STATIC_MODELS", &*META_MUSE_STATIC_MODELS),
                ("BEDROCK_MANTLE_STATIC_MODELS", &*BEDROCK_MANTLE_STATIC_MODELS),
                ("SAKANA_FUGU_STATIC_MODELS", &*SAKANA_FUGU_STATIC_MODELS),
                ("AIAND_STATIC_MODELS", &*AIAND_STATIC_MODELS),
                ("ABLITERATION_STATIC_MODELS", &*ABLITERATION_STATIC_MODELS),
                ("YOLO_AUTO_STATIC_MODELS", &*YOLO_AUTO_STATIC_MODELS),
            ] {
                insert_record(&mut result, name, &specs_value(models));
            }
            return finish(&transport, &context, success(&result));
        }
        if text(field(test, "op")) == "litellm-rich-retained-signals" {
            use ara_cli::provider_models::*;
            let retained =
                Arc::new(RetainedSignalTransport { inner: transport.clone(), signals: Mutex::new(Vec::new()) });
            let context = CatalogContext::new(retained.clone()).unwrap();
            let options = decode(field(test, "optionsEncoded"));
            let mut rich = FetchLiteLLMRichModelsOptions::new(
                string(&options.value, "api"),
                string(&options.value, "provider"),
                string(&options.value, "baseUrl"),
            );
            rich.timeout_ms = options.value.get("timeoutMs").and_then(W::as_number);
            if bool_prop(test, "callerSignal") {
                rich.signal = Some(DiscoverySignal::default());
            }
            let result = fetch_litellm_rich_models(&context, &rich).await;
            if let Some(delay) = test.get("settleMs").and_then(W::as_number).filter(|n| *n > 0.0) {
                tokio::time::sleep(std::time::Duration::from_millis(delay as u64)).await;
            }
            let result = result.map(|models| {
                let mut out = VariantSpec::from_wire(W::object(vec![("models", W::Null)]));
                if let Some(models) = models {
                    insert_record(&mut out, "models", &specs_value(&models));
                }
                out.set(
                    "signals",
                    W::Array(
                        retained
                            .signals
                            .lock()
                            .unwrap()
                            .iter()
                            .map(|signal| {
                                W::object(vec![
                                    ("present", W::Bool(signal.is_some())),
                                    ("aborted", W::Bool(signal.as_ref().is_some_and(DiscoverySignal::is_aborted))),
                                    (
                                        "reason",
                                        signal
                                            .as_ref()
                                            .and_then(DiscoverySignal::reason)
                                            .map(|e| s(e.name))
                                            .unwrap_or(W::Null),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                );
                out
            });
            return finish(
                &transport,
                &context,
                match result {
                    Ok(result) => success(&result),
                    Err(error) => failure(error),
                },
            );
        }
        if text(field(test, "op")) == "simple-headers" {
            use ara_cli::provider_models::*;
            let headers = decode(field(test, "headersEncoded"));
            let callback = text(field(test, "mode")) == "callback";
            let calls = Arc::new(AtomicU64::new(0));
            let number = if callback { array(&headers.value).len() } else { 1 };
            let calls_in = calls.clone();
            let headers = if callback {
                SimpleProviderDiscoveryHeaders::Callback(Arc::new(move || {
                    let index = calls_in.fetch_add(1, Ordering::SeqCst) as usize;
                    Ok(headers
                        .value
                        .as_array()
                        .and_then(|a| a.get(index))
                        .filter(|v| !matches!(v, W::Null))
                        .map(|v| pairs(Some(v))))
                }))
            } else {
                SimpleProviderDiscoveryHeaders::Record(pairs(Some(&headers.value)))
            };
            let config = SimpleProviderConfig {
                common: ModelManagerConfig {
                    api_key: options.get("apiKey").and_then(W::as_string).cloned(),
                    base_url: options.get("baseUrl").and_then(W::as_string).cloned(),
                    context: Some(context.clone()),
                    ..Default::default()
                },
                headers: Some(headers),
            };
            let built = create_simple_openai_completions_options(
                &context,
                string(options, "providerId"),
                string(options, "defaultBaseUrl"),
                config,
            );
            let mut result = VariantSpec::from_wire(W::object(vec![
                ("providerId", s(built.provider_id)),
                ("hasDynamic", W::Bool(built.dynamic_fetcher.is_some())),
                ("headerCalls", n(0)),
            ]));
            let mut models = Vec::new();
            for _ in 0..number {
                match built.dynamic_fetcher.as_ref().unwrap().fetch().await {
                    Ok(raw) => models.push(Arc::new(raw_spec(&raw))),
                    Err(error) => return finish(&transport, &context, failure(error)),
                };
            }
            result.set("headerCalls", n(calls.load(Ordering::SeqCst) as usize));
            insert_record(&mut result, "models", &specs_value(&models));
            return finish(&transport, &context, success(&result));
        }
        if text(field(test, "op")) == "models-dev-map" {
            let source = VariantSpec::from_wire(field(options, "data").clone());
            return finish(
                &transport,
                &context,
                match ara_cli::provider_models::map_models_dev_to_models(
                    &source,
                    &ara_cli::provider_models::models_dev_provider_descriptors(),
                ) {
                    Ok(models) => success(&specs_value(&models)),
                    Err(error) => failure(error),
                },
            );
        }
        if text(field(test, "op")) == "provider-options" {
            use ara_cli::provider_models::*;
            let env: HashMap<_, _> =
                pairs(test.get("env")).into_iter().map(|(k, v)| (k.to_utf8().unwrap(), v)).collect();
            let host = ProviderFactoryHost::new(
                Arc::new(move |key: &str| env.get(key).cloned()),
                "omp/18.1.8",
                std::env::current_dir().unwrap(),
            );
            let config = ModelManagerConfig {
                api_key: options.get("apiKey").and_then(W::as_string).cloned(),
                base_url: options.get("baseUrl").and_then(W::as_string).cloned(),
                authenticated: bool_prop(options, "authenticated"),
                context: Some(context.clone()),
            };
            let built = match text(field(test, "factory")).as_str() {
                "umansModelManagerOptions" => umans_model_manager_options(&context, config),
                "ollamaCloudModelManagerOptions" => ollama_cloud_model_manager_options(&context, config),
                "openaiModelManagerOptions" => openai_model_manager_options(&context, config),
                "gmiCloudModelManagerOptions" => gmi_cloud_model_manager_options(&context, config),
                "groqModelManagerOptions" => groq_model_manager_options(&context, config),
                "cerebrasModelManagerOptions" => cerebras_model_manager_options(&context, config),
                "huggingfaceModelManagerOptions" => huggingface_model_manager_options(&context, config),
                "nvidiaModelManagerOptions" => nvidia_model_manager_options(&context, config),
                "novitaModelManagerOptions" => novita_model_manager_options(&context, config),
                "deepinfraModelManagerOptions" => deepinfra_model_manager_options(&context, config),
                "xaiModelManagerOptions" => xai_model_manager_options(&context, config),
                "xaiOAuthModelManagerOptions" => match xai_oauth_model_manager_options(&context, config) {
                    Ok(options) => options,
                    Err(error) => return finish(&transport, &context, failure(error)),
                },
                "aimlApiModelManagerOptions" => aiml_api_model_manager_options(&context, config),
                "deepseekModelManagerOptions" => deepseek_model_manager_options(&context, config),
                "zhipuCodingPlanModelManagerOptions" => zhipu_coding_plan_model_manager_options(&context, config),
                "mistralModelManagerOptions" => mistral_model_manager_options(&context, config),
                "openrouterModelManagerOptions" => openrouter_model_manager_options(&context, config, &host),
                "zenmuxModelManagerOptions" => zenmux_model_manager_options(&context, config),
                "kiloModelManagerOptions" => kilo_model_manager_options(&context, config),
                "alibabaCodingPlanModelManagerOptions" => alibaba_coding_plan_model_manager_options(&context, config),
                "alibabaTokenPlanModelManagerOptions" => alibaba_token_plan_model_manager_options(&context, config),
                "vercelAiGatewayModelManagerOptions" => vercel_ai_gateway_model_manager_options(&context, config),
                "kimiCodeModelManagerOptions" => kimi_code_model_manager_options(&context, config),
                "veniceModelManagerOptions" => venice_model_manager_options(&context, config),
                "basetenModelManagerOptions" => baseten_model_manager_options(&context, config),
                "togetherModelManagerOptions" => together_model_manager_options(&context, config),
                "coreWeaveModelManagerOptions" => coreweave_model_manager_options(&context, config, &host),
                "bedrockMantleModelManagerOptions" => bedrock_mantle_model_manager_options(&context, config),
                "metaModelManagerOptions" => meta_model_manager_options(&context, config),
                "moonshotModelManagerOptions" => moonshot_model_manager_options(&context, config, &host),
                "sakanaModelManagerOptions" => sakana_model_manager_options(&context, config, &host),
                "aiandModelManagerOptions" => aiand_model_manager_options(&context, config, &host),
                "abliterationModelManagerOptions" => abliteration_model_manager_options(&context, config),
                "yoloAutoModelManagerOptions" => yolo_auto_model_manager_options(&context, config),
                "qwenPortalModelManagerOptions" => qwen_portal_model_manager_options(&context, config),
                "qianfanModelManagerOptions" => qianfan_model_manager_options(&context, config),
                "cloudflareAiGatewayModelManagerOptions" => {
                    cloudflare_ai_gateway_model_manager_options(&context, config)
                }
                "xiaomiModelManagerOptions" => xiaomi_model_manager_options(
                    &context,
                    XiaomiModelManagerConfig { common: config, ..Default::default() },
                ),
                "vllmModelManagerOptions" => vllm_model_manager_options(&context, config, &host),
                "nanoGptModelManagerOptions" => nano_gpt_model_manager_options(&context, config),
                "syntheticModelManagerOptions" => synthetic_model_manager_options(&context, config),
                "lmStudioModelManagerOptions" => lm_studio_model_manager_options(&context, config, &host),
                "ollamaModelManagerOptions" => ollama_model_manager_options(&context, config, &host),
                "waferServerlessModelManagerOptions" => wafer_serverless_model_manager_options(&context, config),
                "opencodeGoModelManagerOptions" => opencode_go_model_manager_options(&context, config, &host),
                "opencodeZenModelManagerOptions" => opencode_zen_model_manager_options(&context, config, &host),
                "fireworksModelManagerOptions" => fireworks_model_manager_options(&context, config, &host),
                "firepassModelManagerOptions" => firepass_model_manager_options(),
                "clinePassModelManagerOptions" => cline_pass_model_manager_options(&context, config),
                "litellmModelManagerOptions" => litellm_model_manager_options(&context, config, &host),
                "githubCopilotModelManagerOptions" => github_copilot_model_manager_options(&context, config, &host),
                name => panic!("unknown factory {name}"),
            };
            return finish(
                &transport,
                &context,
                match encode_options(built, test).await {
                    Ok(result) => success(&result),
                    Err(error) => failure(error),
                },
            );
        }
        if text(field(test, "op")) == "sqlite-wire-binding" {
            let directory = tempfile::tempdir().unwrap();
            let db_path = directory.path().join("wire.db");
            let cache = SqliteModelCache::for_path(&db_path);
            let models = array(field(options, "models"))
                .iter()
                .map(|value| Arc::new(WireModelPolicy.build(&VariantSpec::from_wire(value.clone())).unwrap()))
                .collect::<Vec<_>>();
            let key = string(options, "providerId");
            let fingerprint = string(options, "fingerprint");
            cache
                .write_model_cache_wire(
                    &key,
                    900.0,
                    &models,
                    WireModelCacheWriteOptions {
                        authoritative: true,
                        static_fingerprint: &fingerprint,
                        static_header_sources: &[],
                        restorable_header_fallback: None,
                    },
                )
                .unwrap();
            let db = rusqlite::Connection::open(&db_path).unwrap();
            let row=db.query_row("select provider_id,hex(provider_id),static_fingerprint,hex(static_fingerprint),models from model_cache",[],|row|{
                let decode_raw=|index|->rusqlite::Result<String>{match row.get_ref(index)?{rusqlite::types::ValueRef::Text(bytes)=>Ok(String::from_utf8_lossy(bytes).into_owned()),_=>row.get(index)}};
                Ok(W::object(vec![("provider_id",s(decode_raw(0)?)),("providerHex",s(row.get::<_,String>(1)?)),("static_fingerprint",s(decode_raw(2)?)),("fingerprintHex",s(row.get::<_,String>(3)?)),("models",s(row.get::<_,String>(4)?))]))
            }).unwrap();
            let read = cache_value(cache.read_model_cache_wire(&key, 7200000.0, || 1000.0).unwrap());
            let mut result = VariantSpec::from_wire(W::object(vec![("row", row)]));
            insert_record(&mut result, "read", &read);
            return finish(&transport, &context, success(&result));
        }
        let env: HashMap<String, WireString> =
            pairs(test.get("env")).into_iter().map(|(key, value)| (key.to_utf8().unwrap(), value)).collect();
        let environment = move |key: &str| env.get(key).cloned();
        let value = cache_provider_id::resolve_model_cache_provider_id(
            &string(options, "providerId"),
            options.get("apiKey").and_then(W::as_string),
            options.get("baseUrl").and_then(W::as_string),
            &environment as &dyn ProviderEnvironment,
        );
        return finish(&transport, &context, success(&VariantSpec::from_wire(s(value))));
    }
    if text(field(test, "op")) == "fingerprint" {
        let initial = array(field(options, "models"))
            .iter()
            .map(|value| Arc::new(VariantSpec::from_wire(value.clone())))
            .collect();
        let models = ModelArray::new(initial);
        let before = fingerprint_static_models(&models, false);
        let authoritative = fingerprint_static_models(&models, true);
        models.replace(
            array(field(options, "replacement"))
                .iter()
                .map(|value| Arc::new(VariantSpec::from_wire(value.clone())))
                .collect(),
        );
        let out = W::object(vec![
            ("before", s(before)),
            ("authoritative", s(authoritative)),
            ("mutated", s(fingerprint_static_models(&models, false))),
            ("newArray", s(fingerprint_static_models(&ModelArray::new(models.snapshot()), false))),
            ("empty", s(fingerprint_static_models(&ModelArray::default(), true))),
        ]);
        return finish(&transport, &context, success(&VariantSpec::from_wire(out)));
    }
    let directory = tempfile::tempdir().unwrap();
    let cache = Arc::new(Mutex::new(SqliteModelCache::for_path(directory.path().join("models.db"))));
    let trace = Arc::new(Mutex::new(Vec::new()));
    let clock = Arc::new(Clock {
        now: AtomicU64::new(options.get("now").and_then(W::as_number).unwrap_or(1000000.0).to_bits()),
        trace: trace.clone(),
    });
    let mut config = ModelManagerOptions::new(string(options, "providerId"));
    config.static_models = options.get("staticModels").map(|value| raw(Some(value)));
    config.cache_provider_id = options.get("cacheProviderId").and_then(W::as_string).cloned();
    config.cache_ttl_ms = options.get("cacheTtlMs").and_then(W::as_number);
    config.dynamic_models_authoritative = bool_prop(options, "dynamicModelsAuthoritative");
    config.restorable_header_fallback =
        options.get("restorableHeaderFallback").map(|value| VariantSpec::from_wire(value.clone()));
    config.drop_cached_model_ids_on_static_mismatch = options
        .get("dropCachedModelIdsOnStaticMismatch")
        .and_then(W::as_array)
        .map(|values| values.iter().map(|value| value.as_string().unwrap().clone()).collect());
    let static_models =
        pass_model_list(&config.static_models.clone().unwrap_or_else(|| RawModelValue::models(Vec::new()))).unwrap();
    let mut fingerprint =
        fingerprint_static_models(&ModelArray::new(static_models.clone()), config.dynamic_models_authoritative);
    if let Some(ids) = config.drop_cached_model_ids_on_static_mismatch.as_ref().filter(|ids| !ids.is_empty()) {
        let mut units = Vec::new();
        for (index, id) in ids.iter().enumerate() {
            if index > 0 {
                units.push(0);
            }
            units.extend_from_slice(id.units());
        }
        fingerprint = format!(
            "{}:drop:{}",
            fingerprint.to_utf8().unwrap(),
            bun_hash::hash_string_base36(&WireString::from_units(units))
        )
        .into();
    }
    let writer = Writer {
        cache: cache.clone(),
        key: config.cache_provider_id.clone().unwrap_or_else(|| config.provider_id.clone()),
        fingerprint,
        static_models,
        fallback: config.restorable_header_fallback.clone(),
        clock: clock.clone(),
    };
    if let Some(entry) = test.get("cache") {
        writer.write(entry);
    }
    if let Some(values) = test.get("dynamic").and_then(W::as_array) {
        config.dynamic_fetcher = Some(Arc::new(Dynamic {
            steps: Mutex::new(values.iter().cloned().collect()),
            last: values.last().unwrap().clone(),
            index: Mutex::new(0),
            writer: writer.clone(),
            trace: trace.clone(),
        }));
    }
    if let Some(values) = test.get("modelsDev").and_then(W::as_array) {
        config.models_dev = Some(Arc::new(Dev {
            steps: Mutex::new(values.iter().cloned().collect()),
            last: values.last().unwrap().clone(),
            current: Mutex::new(None),
            index: Mutex::new(0),
            additive: bool_prop(options, "additiveOnly"),
            trace: trace.clone(),
        }));
    }
    let manager = ModelManager::new(
        config,
        context.clone(),
        Arc::new(ModelManagerHost { cache: cache.clone(), clock: clock.clone() }),
    );
    let default = vec![W::Object(Vec::new())];
    let steps = test.get("steps").and_then(W::as_array).unwrap_or(&default);
    let mut results = Vec::new();
    let mut snapshots = Vec::new();
    for step in steps {
        if let Some(now) = step.get("now").and_then(W::as_number) {
            clock.now.store(now.to_bits(), Ordering::SeqCst);
        }
        let strategy = match step.get("strategy").and_then(W::as_string).and_then(|s| s.to_utf8().ok()).as_deref() {
            Some("offline") => ModelRefreshStrategy::Offline,
            Some("online") => ModelRefreshStrategy::Online,
            _ => ModelRefreshStrategy::OnlineIfUncached,
        };
        let resolved = match manager.refresh(strategy).await {
            Ok(value) => value,
            Err(error) => return finish(&transport, &context, failure(error)),
        };
        let mut result = VariantSpec::from_wire(W::Object(Vec::new()));
        insert_record(&mut result, "models", &specs_value(&resolved.models));
        result.set("stale", W::Bool(resolved.stale));
        result.set("source", s(resolved.source.as_str()));
        if let Some(at) = resolved.updated_at {
            result.set("updatedAt", W::Number(at));
        }
        results.push(Arc::new(result));
        snapshots.push(Arc::new(cache_value(
            cache
                .lock()
                .unwrap()
                .read_model_cache_wire(
                    &writer.key,
                    manager.options.cache_ttl_ms.unwrap_or(DEFAULT_CACHE_TTL_MS),
                    || f64::from_bits(clock.now.load(Ordering::SeqCst)),
                )
                .unwrap(),
        )));
    }
    let mut output = VariantSpec::from_wire(W::Object(Vec::new()));
    insert_record(&mut output, "results", &specs_value(&results));
    insert_record(&mut output, "snapshots", &specs_value(&snapshots));
    output.set("trace", W::Array(trace.lock().unwrap().clone()));
    finish(&transport, &context, success(&output))
}
