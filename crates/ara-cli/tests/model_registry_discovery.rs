//! Grouped configured-discovery scenarios at fixed OMP 596f2da. Responses and
//! authentication receipts are controlled fakes; this is not real-model proof.

use ara_cli::{
    catalog_discovery::{
        CatalogContext, DiscoveryError, DiscoveryReply, DiscoveryRequest, DiscoveryTransport, HttpMethod,
    },
    model_cache::WireCacheEntry,
    model_config_file::ModelsConfigFile,
    model_config_values::{ConfigValueContext, ConfigValueResolver, resolve_config_headers},
    model_identity_wire::{number, text},
    model_manager::ModelClock,
    model_patch::{HeaderSlot, HostModelRef, OrderedProviderSet},
    model_registry_discovery::{
        RegistryDiscoveryFetch, apply_llama_cpp_qwen_thinking, discover_configured_models, discovery_probe_timeout_ms,
    },
    model_registry_loader::{
        RegistryCachePort, RegistryDiscoveryConfig, RegistryLoadOptions, RegistryLoader, RegistryLoaderError,
        RegistryLoaderHost,
    },
};
use ara_rpc::{WireString, WireValue};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

struct NoCache;
impl RegistryCachePort for NoCache {
    fn read(&self, _: &WireString, _: f64, _: &dyn ModelClock) -> Result<Option<WireCacheEntry>, RegistryLoaderError> {
        Ok(None)
    }
    fn repair(
        &self,
        _: &WireString,
        _: &WireCacheEntry,
        _: &[HostModelRef],
        _: &HeaderSlot,
    ) -> Result<(), RegistryLoaderError> {
        Ok(())
    }
}
fn config(kind: &str, extra: Value) -> RegistryDiscoveryConfig {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("models.json");
    let mut provider = json!({"api":"openai-completions","baseUrl":"http://models.test/root",
        "headers":{"x-private":"literal-private"},"discovery":{"type":kind,"timeoutMs":200}});
    for (key, value) in extra.as_object().unwrap() {
        provider[key] = value.clone();
    }
    std::fs::write(&path, json!({"providers":{"fixture":provider}}).to_string()).unwrap();
    let mut file = ModelsConfigFile::new(path).unwrap();
    let resolver = ConfigValueResolver::new();
    let config_environment = |_: &str| None;
    let provider_environment = |_: &str| None;
    let mut disabled = OrderedProviderSet::default();
    for provider in ["ollama", "llama.cpp", "lm-studio"] {
        disabled.insert(provider.into());
    }
    let loader = RegistryLoader::load(
        &mut file,
        &RegistryLoadOptions { disabled_providers: disabled, ..Default::default() },
        &RegistryLoaderHost {
            project_dir: directory.path(),
            config_values: &resolver,
            config_environment: &config_environment,
            provider_environment: &provider_environment,
            cache: &NoCache,
            clock: &|| 1000.0,
            has_auth: &|_| false,
            install_config_key: &|_, _| {},
            continue_loading: &|| true,
        },
    )
    .unwrap();
    assert!(loader.loaded_config().error.is_none(), "controlled configuration must pass actual validation");
    loader.loaded_config().discoverable_providers[0].clone()
}

struct DirectTransportIsForbidden;
#[async_trait]
impl DiscoveryTransport for DirectTransportIsForbidden {
    fn context_id(&self) -> u64 {
        777
    }
    async fn fetch(&self, _: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        Err(DiscoveryError::new("configured protocol bypassed RegistryDiscoveryFetch"))
    }
}
fn context() -> CatalogContext {
    CatalogContext::new(Arc::new(DirectTransportIsForbidden)).unwrap()
}

#[derive(Default)]
struct Fixture {
    replies: Mutex<HashMap<WireString, VecDeque<Result<DiscoveryReply, DiscoveryError>>>>,
    requests: Mutex<Vec<(WireString, DiscoveryRequest)>>,
    barrier: Option<(Vec<WireString>, Arc<tokio::sync::Barrier>)>,
    pending: Option<WireString>,
    receipt_bearer: bool,
}
impl Fixture {
    fn push(&self, url: &str, status: u16, payload: Value) {
        self.replies.lock().unwrap().entry(url.into()).or_default().push_back(Ok(DiscoveryReply {
            status,
            headers: Vec::new(),
            body: payload.to_string().into_bytes(),
            json_override: None,
        }));
    }
    fn calls(&self) -> Vec<String> {
        self.requests.lock().unwrap().iter().map(|(_, request)| request.url.to_utf8().unwrap()).collect()
    }
}
#[async_trait]
impl RegistryDiscoveryFetch for Fixture {
    async fn fetch(&self, provider: &WireString, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        self.requests.lock().unwrap().push((provider.clone(), request.clone()));
        if self.pending.as_ref() == Some(&request.url) {
            return futures::future::pending().await;
        }
        let reply =
            self.replies.lock().unwrap().get_mut(&request.url).and_then(VecDeque::pop_front).unwrap_or(Ok(
                DiscoveryReply { status: 404, headers: Vec::new(), body: Vec::new(), json_override: None },
            ));
        if let Some((urls, barrier)) = &self.barrier
            && urls.contains(&request.url)
        {
            barrier.wait().await;
        }
        reply
    }
    fn settled_request_headers(
        &self,
        _: &WireString,
        fallback: &[(WireString, WireString)],
    ) -> Vec<(WireString, WireString)> {
        let mut headers = fallback.to_vec();
        if self.receipt_bearer {
            headers.push(("Authorization".into(), "Bearer refreshed-private".into()));
        }
        headers
    }
}
fn ids(models: &[HostModelRef]) -> Vec<WireString> {
    models.iter().filter_map(|model| text(model.spec(), "id")).collect()
}
fn private_header(model: &HostModelRef, name: &str) -> Option<String> {
    let resolver = ConfigValueResolver::new();
    resolve_config_headers(
        model.headers().as_source(),
        &resolver,
        &ConfigValueContext { project_dir: std::path::Path::new("."), environment: &|_: &str| None },
    )
    .and_then(|headers| headers.get(name).map(str::to_owned))
}

#[tokio::test]
async fn openai_list_preserves_payload_order_duplicates_and_local_metadata_boundaries() {
    let mut config = config(
        "openai-models-list",
        json!({"api":"anthropic-messages","baseUrl":"https://catalog.test/v3/compat/?token=x#hash"}),
    );
    config.discovery.set("injectV1", WireValue::Bool(false));
    let fixture = Arc::new(Fixture { receipt_bearer: true, ..Default::default() });
    fixture.push(
        "https://catalog.test/v3/compat/models",
        200,
        json!({"data":[
            {"id":"unknown-z","max_model_len":"0x10000","input_modalities":["IMAGE"]},
            {"id":"unknown-a","context_length":4096,"architecture":{"input_modalities":["audio"]}},
            {"id":"unknown-z","context_length":"2000"},{"id":""},{}
        ]}),
    );
    let models = discover_configured_models(&config, &context(), fixture.clone()).await.unwrap();
    let expected: Vec<WireString> = ["unknown-z", "unknown-a", "unknown-z"].into_iter().map(Into::into).collect();
    assert_eq!(ids(&models), expected);
    assert_eq!(number(models[0].spec(), "contextWindow"), Some(65536.0));
    assert_eq!(number(models[0].spec(), "maxTokens"), Some(8192.0));
    assert_eq!(number(models[1].spec(), "maxTokens"), Some(4096.0));
    assert_eq!(
        models[0].spec().get("input"),
        Some(&WireValue::Array(vec![WireValue::String("text".into()), WireValue::String("image".into())]))
    );
    assert_eq!(private_header(&models[0], "Authorization"), Some("Bearer refreshed-private".into()));
    assert_eq!(private_header(&models[0], "x-private"), Some("literal-private".into()));
    for model in &models {
        let metadata = model.spec().to_wire_json().stringify();
        assert!(!metadata.contains("refreshed-private") && !metadata.contains("literal-private"));
        assert!(model.spec().get("headers").is_none() && model.spec().get("apiKey").is_none());
    }
    assert_eq!(fixture.calls(), ["https://catalog.test/v3/compat/models"]);
}

#[tokio::test]
async fn lm_studio_parallel_native_probe_is_best_effort_and_provider_values_win() {
    let config = config("lm-studio", json!({"baseUrl":"http://studio.test/v1"}));
    let fixture = Arc::new(Fixture {
        barrier: Some((
            vec!["http://studio.test/v1/models".into(), "http://studio.test/api/v0/models".into()],
            Arc::new(tokio::sync::Barrier::new(2)),
        )),
        receipt_bearer: true,
        ..Default::default()
    });
    fixture.push(
        "http://studio.test/v1/models",
        200,
        json!({"data":[
            {"id":"loaded-runtime"},{"id":"reported","max_model_len":12000,"input":["image"]}
        ]}),
    );
    fixture.push("http://studio.test/api/v0/models", 200, json!({"data":[
        {"id":"loaded-runtime","state":"loaded","loaded_context_length":"4096","max_context_length":64000,"type":"VLM"},
        {"id":"reported","max_context_length":96000,"type":"llm"}
    ]}));
    let models = discover_configured_models(&config, &context(), fixture.clone()).await.unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(number(models[0].spec(), "contextWindow"), Some(4096.0));
    assert_eq!(number(models[1].spec(), "contextWindow"), Some(12000.0));
    assert_eq!(
        models[1].spec().get("input"),
        Some(&WireValue::Array(vec![WireValue::String("text".into())])),
        "native modality takes priority over list modality"
    );
    assert_eq!(text(models[0].spec(), "imageInputDecoder"), Some("stb".into()));
    let native = fixture
        .requests
        .lock()
        .unwrap()
        .iter()
        .find(|(_, request)| request.url.equals_ascii("http://studio.test/api/v0/models"))
        .unwrap()
        .1
        .clone();
    assert!(
        native
            .headers
            .iter()
            .any(|(name, value)| name.equals_ascii("Accept") && value.equals_ascii("application/json"))
    );
    assert_eq!(
        private_header(&models[0], "Accept"),
        None,
        "probe-only Accept does not enter the successful original header record"
    );
    let failed = Arc::new(Fixture::default());
    failed.push("http://studio.test/v1/models", 200, json!({"data":[{"id":"fallback"}]}));
    failed.push("http://studio.test/api/v0/models", 503, json!({}));
    let models = discover_configured_models(&config, &context(), failed).await.unwrap();
    assert_eq!(number(models[0].spec(), "contextWindow"), Some(128000.0));
}

#[tokio::test]
async fn proxy_endpoint_precedence_display_affixes_and_output_limit_match_native() {
    let config = config("proxy", json!({"baseUrl":"http://proxy.test"}));
    let fixture = Arc::new(Fixture::default());
    fixture.push("http://proxy.test/v1/models", 200, json!({"data":[
        {"id":"[Reseller] alien-model [Promo]","name":"  ","supported_endpoint_types":["openai","anthropic"],"context_length":1024},
        {"id":"plain-model","name":" Custom Label ","supported_endpoint_types":["openai"],"context_length":"8192"},
        {"id":"fallback-model","supported_endpoint_types":[]}
    ]}));
    let models = discover_configured_models(&config, &context(), fixture).await.unwrap();
    assert_eq!(text(models[0].spec(), "api"), Some("anthropic-messages".into()));
    assert_eq!(text(models[0].spec(), "name"), Some("alien-model".into()));
    assert_eq!(
        number(models[0].spec(), "maxTokens"),
        Some(8192.0),
        "proxy original does not apply the list context cap to output"
    );
    assert_eq!(text(models[1].spec(), "name"), Some("Custom Label".into()));
    assert_eq!(text(models[2].spec(), "api"), Some("openai-completions".into()));
}

#[tokio::test]
async fn ollama_parallel_show_keeps_duplicates_and_final_id_metadata_with_runtime_context() {
    let config = config("ollama", json!({"api":"openai-responses","baseUrl":"http://127.0.0.1:11434/custom/v1"}));
    let fixture = Arc::new(Fixture {
        barrier: Some((vec!["http://127.0.0.1:11434/api/show".into()], Arc::new(tokio::sync::Barrier::new(2)))),
        ..Default::default()
    });
    fixture.push(
        "http://127.0.0.1:11434/api/tags",
        200,
        json!({"models":[{"model":"duplicate","name":"First"},{"model":"duplicate","name":"Second"}]}),
    );
    fixture.push("http://127.0.0.1:11434/api/show", 200, json!({"parameters":"num_ctx 4096\n","capabilities":["thinking","VISION"],"model_info":{"family.context_length":96000}}));
    fixture.push("http://127.0.0.1:11434/api/show", 200, json!({"parameters":"num_ctx 2048\n","capabilities":{"thinking":false,"image":true},"model_info":{"context_length":32000}}));
    let models = discover_configured_models(&config, &context(), fixture.clone()).await.unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(text(models[0].spec(), "name"), Some("First".into()));
    assert_eq!(text(models[1].spec(), "name"), Some("Second".into()));
    assert_eq!(number(models[0].spec(), "contextWindow"), number(models[1].spec(), "contextWindow"));
    assert_eq!(
        number(models[0].spec(), "contextWindow"),
        Some(2048.0),
        "Promise.all preserves input order: final duplicate metadata wins even when the first duplicate settles last"
    );
    assert_eq!(text(models[0].spec(), "baseUrl"), Some("http://127.0.0.1:11434/v1".into()));
    assert_eq!(text(models[0].spec(), "imageInputDecoder"), Some("stb".into()));
    for (_, request) in fixture
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, request)| request.url.equals_ascii("http://127.0.0.1:11434/api/show"))
    {
        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.body.as_deref(), Some(br#"{"model":"duplicate"}"#.as_slice()));
        assert!(
            request
                .headers
                .iter()
                .any(|(name, value)| name.equals_ascii("Content-Type") && value.equals_ascii("application/json"))
        );
    }
}

#[tokio::test]
async fn llama_router_context_precedence_unlimited_generation_and_qwen_fixups() {
    let config = config("llama.cpp", json!({"api":"openai-responses","baseUrl":"http://llama.test"}));
    let fixture = Arc::new(Fixture::default());
    fixture.push("http://llama.test/models", 200, json!({"data":[
        {"id":"qwen3.6-27b","meta":{"n_ctx":70000,"n_ctx_train":96000},"status":{"args":["--ctx-size=8000"]},"architecture":{"input_modalities":["image"]}},
        {"id":"unloaded","meta":{"n_ctx":0,"n_ctx_train":96000},"status":{"args":["--ctx-size","8192"]}},
        {"id":"preset","status":{"args":["-c","0"],"preset":"[p]\nctx-size = 6000\n"}},
        {"id":"ternary-bonsai-27b"},null,{}
    ]}));
    fixture.push("http://llama.test/props", 200, json!({"default_generation_settings":{"n_ctx":32000,"params":{"n_predict":"-1"}},"modalities":{"vision":false}}));
    let models = discover_configured_models(&config, &context(), fixture).await.unwrap();
    assert_eq!(models.len(), 4);
    assert_eq!(number(models[0].spec(), "contextWindow"), Some(70000.0));
    assert_eq!(number(models[0].spec(), "maxTokens"), Some(70000.0));
    assert_eq!(text(models[0].spec(), "api"), Some("openai-completions".into()));
    assert_eq!(number(models[1].spec(), "contextWindow"), Some(8192.0));
    assert_eq!(number(models[2].spec(), "contextWindow"), Some(6000.0));
    assert_eq!(number(models[3].spec(), "contextWindow"), Some(32000.0));
    assert_eq!(text(models[3].spec(), "api"), Some("openai-completions".into()));
    assert!(
        Arc::ptr_eq(&models[1], &apply_llama_cpp_qwen_thinking(&models[1]).unwrap()),
        "non-Qwen models retain the same owner"
    );
}

#[tokio::test]
async fn litellm_native_bridge_fallback_http_errors_deadlines_and_unknown_kind_are_distinct() {
    let config = config("litellm", json!({"baseUrl":"http://lite.test/root"}));
    let fixture = Arc::new(Fixture::default());
    for path in ["/model_group/info", "/v2/model/info", "/model/info", "/v1/model/info"] {
        fixture.push(&format!("http://lite.test/root{path}"), 401, json!({"error":"controlled auth failure"}));
    }
    fixture.push("http://lite.test/root/v1/models", 200, json!({"data":[{"id":"fallback-z"},{"id":"fallback-a"}]}));
    let models = discover_configured_models(&config, &context(), fixture.clone()).await.unwrap();
    let expected: Vec<WireString> = ["fallback-z", "fallback-a"].into_iter().map(Into::into).collect();
    assert_eq!(ids(&models), expected);
    assert!(
        fixture.calls().iter().any(|url| url.ends_with("/model_group/info")),
        "rich endpoints must cross the Registry fetch bridge"
    );
    let rich = Arc::new(Fixture { receipt_bearer: true, ..Default::default() });
    rich.push("http://lite.test/root/model_group/info", 200, json!({"data":[
        {"model_group":"rich-z","providers":["openai"],"supports_vision":false,"max_input_tokens":9000,"max_output_tokens":800},
        {"model_group":"rich-a","providers":["openai"],"supports_vision":true,"max_input_tokens":20000,"max_output_tokens":2000}
    ]}));
    let models = discover_configured_models(&config, &context(), rich.clone()).await.unwrap();
    let expected: Vec<WireString> = ["rich-a", "rich-z"].into_iter().map(Into::into).collect();
    assert_eq!(ids(&models), expected, "the existing rich helper retains its own native ordering");
    assert_eq!(number(models[1].spec(), "contextWindow"), Some(9000.0));
    assert_eq!(number(models[1].spec(), "maxTokens"), Some(800.0));
    assert_eq!(text(models[0].spec(), "api"), Some("openai-responses".into()));
    assert_eq!(private_header(&models[0], "Authorization"), Some("Bearer refreshed-private".into()));
    assert_eq!(rich.calls(), ["http://lite.test/root/model_group/info"]);
    let mut failed_config = config.clone();
    failed_config.discovery.set("type", WireValue::String("openai-models-list".into()));
    let failed = Arc::new(Fixture::default());
    failed.push("http://lite.test/root/v1/models", 503, json!({}));
    let error = discover_configured_models(&failed_config, &context(), failed).await.err().unwrap();
    assert!(error.message.to_utf8().unwrap().starts_with("HTTP 503 from "));
    let pending = Arc::new(Fixture { pending: Some("http://lite.test/root/v1/models".into()), ..Default::default() });
    failed_config.discovery.set("timeoutMs", WireValue::Number(10.0));
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        discover_configured_models(&failed_config, &context(), pending.clone()),
    )
    .await
    .unwrap()
    .err()
    .unwrap();
    assert!(error.name.equals_ascii("TimeoutError"));
    let signal = pending.requests.lock().unwrap()[0].1.signal.clone().unwrap();
    assert!(signal.is_aborted() && signal.reason().unwrap().name.equals_ascii("TimeoutError"));
    failed_config.discovery.set("type", WireValue::String("future-unsupported".into()));
    let empty = Arc::new(Fixture::default());
    assert!(
        discover_configured_models(&failed_config, &context(), empty.clone())
            .await
            .err()
            .unwrap()
            .name
            .equals_ascii("UnsupportedDiscovery")
    );
    assert!(empty.requests.lock().unwrap().is_empty());
    for (base, expected) in [
        ("http://localhost:8080", 250.0),
        ("http://127.42.1.2", 250.0),
        ("http://[::1]:11434", 250.0),
        ("http://lan.test", 10000.0),
    ] {
        assert_eq!(discovery_probe_timeout_ms(&base.into(), 250.0, None), expected);
    }
}
