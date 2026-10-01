//! Fixed OMP local catalogs and Synthetic authoritative wire capabilities.
use super::*;
use crate::catalog_discovery::{DiscoveryRequest, DiscoverySignal, HttpMethod};
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use std::sync::Mutex;
pub fn resolve_synthetic_thinking(wire_efforts: &[WireString]) -> Option<VariantSpec> {
    let has_none = wire_efforts.iter().any(|e| e.equals_ascii("none"));
    let mut efforts: Vec<_> = ["minimal", "low", "medium", "high", "xhigh", "max"]
        .iter()
        .copied()
        .filter(|e| wire_efforts.iter().any(|s| s.equals_ascii(e)))
        .collect();
    if efforts.is_empty() {
        if !has_none {
            return None;
        }
        efforts.push("minimal");
    } else if has_none && !efforts.contains(&"minimal") {
        efforts.insert(0, "minimal");
    }
    let mut t = thinking("effort", &efforts);
    if has_none && efforts.first() == Some(&"minimal") {
        let authored_minimal = wire_efforts.iter().any(|e| e.equals_ascii("minimal"));
        if !authored_minimal {
            t.set("effortMap", WireValue::object(vec![("minimal", wire::str_value("none"))]));
        }
    }
    Some(t)
}
fn synthetic_price(value: Option<&WireValue>) -> Option<f64> {
    let mapped = value.map(|v| {
        if let WireValue::String(s) = v {
            let s = crate::catalog_discovery::js_trim(s);
            WireValue::String(if starts(&s, "$") { WireString::from_units(s.units()[1..].to_vec()) } else { s })
        } else {
            v.clone()
        }
    });
    let n = to_number(mapped.as_ref())?;
    if n < 0.0 {
        return None;
    }
    Some(math_round(n * 1e12) / 1e6)
}
pub fn synthetic_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let mapper = Arc::new(|e: &VariantSpec, d: &VariantSpec, r: Option<&VariantSpec>| {
        let efforts = e.record("reasoning_parameters").map(|r| wire::list(&r, "efforts")).unwrap_or_default();
        let t = resolve_synthetic_thinking(&efforts);
        let wire_reasoning = wire::has_string(e, "supported_features", "reasoning") || !efforts.is_empty();
        let named = t.as_ref().map(|t| wire::list(t, "efforts").len() as isize).unwrap_or(0)
            - isize::from(efforts.iter().any(|e| e.equals_ascii("none")));
        let reasoning = if wire_reasoning && named > 0 {
            true
        } else if !efforts.is_empty() {
            false
        } else {
            wire::boolean(e, "supports_reasoning") == Some(true)
                || r.is_some_and(|r| wire::boolean(r, "reasoning") == Some(true))
        };
        let mut m = r.cloned().unwrap_or_else(|| d.clone());
        copy_field(&mut m, "id", d, "id");
        copy_field(&mut m, "baseUrl", d, "baseUrl");
        m.set(
            "name",
            wire::str_value(name(
                e.get("name"),
                &r.and_then(|r| text(r, "name")).or_else(|| text(d, "name")).unwrap_or_else(|| "".into()),
            )),
        );
        m.set("reasoning", WireValue::Bool(reasoning));
        if let Some(t) = t {
            m.set_record("thinking", &t);
        }
        let modalities = wire::list(e, "input_modalities");
        let vision = if !modalities.is_empty() {
            modalities.iter().any(|s| s.equals_ascii("image"))
        } else {
            wire::boolean(e, "supports_vision") == Some(true)
                || r.is_some_and(|r| wire::has_string(r, "input", "image"))
        };
        m.set(
            "input",
            if vision { input(Some(&WireValue::Array(vec![wire::str_value("image")]))) } else { input(None) },
        );
        if e.get("supported_features").is_some()
            && !wire::has_string(e, "supported_features", "tools")
            && !r.is_some_and(|r| wire::boolean(r, "supportsTools") == Some(true))
            || r.is_some_and(|r| wire::boolean(r, "supportsTools") == Some(false))
        {
            m.set("supportsTools", WireValue::Bool(false));
        }
        if let Some(pricing) = e.record("pricing").filter(|p| matches!(p.value, WireValue::Object(_)))
            && let (Some(input), Some(output)) =
                (synthetic_price(pricing.get("prompt")), synthetic_price(pricing.get("completion")))
        {
            let fallback = m.record("cost").unwrap_or_else(wire::empty);
            m.set(
                "cost",
                WireValue::object(vec![
                    ("input", WireValue::Number(input)),
                    ("output", WireValue::Number(output)),
                    (
                        "cacheRead",
                        synthetic_price(pricing.get("input_cache_reads"))
                            .map(WireValue::Number)
                            .unwrap_or_else(|| fallback.get("cacheRead").cloned().unwrap_or(WireValue::Null)),
                    ),
                    (
                        "cacheWrite",
                        synthetic_price(pricing.get("input_cache_writes"))
                            .map(WireValue::Number)
                            .unwrap_or_else(|| fallback.get("cacheWrite").cloned().unwrap_or(WireValue::Null)),
                    ),
                ]),
            );
        }
        m.set(
            "contextWindow",
            positive(
                e.get("context_length"),
                r.and_then(|r| r.get("contextWindow")).or_else(|| d.get("contextWindow")),
            ),
        );
        m.set(
            "maxTokens",
            positive(
                e.get("max_output_length").filter(|v| !matches!(v, WireValue::Null)).or_else(|| e.get("max_tokens")),
                r.and_then(|r| r.get("maxTokens"))
                    .filter(|v| !matches!(v, WireValue::Null))
                    .or(Some(&WireValue::Number(8192.0))),
            ),
        );
        Ok(Some(m))
    });
    compatible_options(
        ctx,
        CompatibleBuilder {
            api: "openai-completions".into(),
            provider: "synthetic".into(),
            default_base: "https://api.synthetic.new/openai/v1".into(),
            config,
            headers: Vec::new(),
            authoritative: true,
            require_api_key: true,
            drop_ids: None,
            map: mapper,
            filter: None,
        },
    )
}
#[derive(Clone, Default)]
pub struct LmStudioNativeModelMetadataOptions {
    pub headers: Vec<(WireString, WireString)>,
    pub signal: Option<DiscoverySignal>,
}
pub async fn fetch_lm_studio_native_model_metadata(
    ctx: &CatalogContext,
    base: &WireString,
    options: LmStudioNativeModelMetadataOptions,
) -> Result<Option<HashMap<WireString, VariantSpec>>, DiscoveryError> {
    let normalized = trim_one_slash(&crate::catalog_discovery::js_trim(base));
    let base = if ends(&normalized, "/v1") { normalized.slice_prefix(normalized.len() - 3) } else { normalized };
    let mut headers = vec![("Accept".into(), "application/json".into())];
    for (k, v) in options.headers {
        crate::catalog_discovery::openai::set_record_header(&mut headers, k, v);
    }
    let request = DiscoveryRequest {
        url: append(&base, "/api/v0/models"),
        headers,
        signal: options.signal.clone(),
        ..Default::default()
    };
    let response = if options.signal.is_some() {
        ctx.transport.fetch(request).await
    } else {
        timed_fetch(ctx, request, 250.0).await
    };
    let payload = response.ok().filter(|r| r.ok()).and_then(|r| r.json().ok());
    let Some(payload) = payload.filter(|p| matches!(p.value, WireValue::Object(_))) else {
        return Ok(None);
    };
    let Some(entries) = payload.get("data").and_then(WireValue::as_array) else {
        return Ok(None);
    };
    let mut out = HashMap::new();
    for entry in entries {
        if !matches!(entry, WireValue::Object(_)) {
            continue;
        }
        let entry = VariantSpec::from_wire(entry.clone());
        let Some(id) = text(&entry, "id").filter(|s| !s.is_empty()) else {
            continue;
        };
        let vision = text(&entry, "type").is_some_and(|s| lower(&s).equals_ascii("vlm"))
            || wire::list(&entry, "capabilities")
                .iter()
                .any(|s| lower(s).equals_ascii("vision") || lower(s).equals_ascii("image"));
        let loaded = if text(&entry, "state").is_some_and(|s| s.equals_ascii("loaded")) {
            to_number(entry.get("loaded_context_length")).filter(|n| *n > 0.0)
        } else {
            None
        };
        let window = loaded.or_else(|| {
            ["max_context_length", "context_length", "max_model_len"]
                .iter()
                .find_map(|k| to_number(entry.get(k)).filter(|n| *n > 0.0))
        });
        let mut metadata = wire::empty();
        metadata.set(
            "input",
            if vision { input(Some(&WireValue::Array(vec![wire::str_value("image")]))) } else { input(None) },
        );
        if let Some(window) = window {
            metadata.set("contextWindow", WireValue::Number(window));
        }
        out.insert(id, metadata);
    }
    Ok(Some(out))
}
pub fn lm_studio_model_manager_options(
    default: &CatalogContext,
    config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    let ctx = context(default, &config);
    let base = config
        .base_url
        .or_else(|| host.environment.get("LM_STUDIO_BASE_URL"))
        .unwrap_or_else(|| "http://127.0.0.1:1234/v1".into());
    let key = config.api_key;
    let refs = Arc::new(wire::create_bundled_reference_map(&"lm-studio".into()));
    let mut options = ModelManagerOptions::new("lm-studio");
    options.dynamic_fetcher = Some(dynamic(move || {
        let ctx = ctx.clone();
        let base = base.clone();
        let key = key.clone();
        let refs = refs.clone();
        async move {
            let native_ctx = ctx.clone();
            let native_base = base.clone();
            let headers = key
                .as_ref()
                .filter(|s| !s.is_empty())
                .map(|key| {
                    vec![("Authorization".into(), super::super::cache_provider_id::join(&"Bearer ".into(), key))]
                })
                .unwrap_or_default();
            let native = eager_background(async move {
                fetch_lm_studio_native_model_metadata(
                    &native_ctx,
                    &native_base,
                    LmStudioNativeModelMetadataOptions { headers, signal: None },
                )
                .await
            })
            .await;
            let mut d = OpenAiCompatibleOptions::new("openai-completions", "lm-studio", base);
            d.api_key = key;
            d.map_model = Some(Arc::new(move |e, d, _| {
                Ok(Some(Arc::new(map_with_bundled_reference(
                    e,
                    &d,
                    refs.get(&text(&d, "id").unwrap_or_else(|| "".into())).map(|r| r.as_ref()),
                ))))
            }));
            let Some(models) = fetch_openai_compatible_models(&ctx, &d).await? else {
                return Ok(RawModelValue::null());
            };
            let metadata = native.await.ok().and_then(Result::ok).flatten();
            let Some(metadata) = metadata else {
                return Ok(RawModelValue::models(models));
            };
            Ok(RawModelValue::models(
                models
                    .into_iter()
                    .map(|m| {
                        let Some(meta) = metadata.get(&text(&m, "id").unwrap_or_else(|| "".into())) else {
                            return m;
                        };
                        let mut m = (*m).clone();
                        copy_field(&mut m, "input", meta, "input");
                        if meta.get("contextWindow").is_some() {
                            copy_field(&mut m, "contextWindow", meta, "contextWindow");
                        }
                        Arc::new(m)
                    })
                    .collect(),
            ))
        }
    }));
    options
}
type ShowFuture = Shared<BoxFuture<'static, Option<VariantSpec>>>;
#[derive(Clone, Hash, PartialEq, Eq)]
enum OllamaMetadataKey {
    String(WireString),
    Number(u64),
    Bool(bool),
    Object(u64),
}
impl OllamaMetadataKey {
    fn of(id: &WireValue) -> Self {
        static NEXT_OBJECT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        match id {
            WireValue::String(id) => Self::String(id.clone()),
            WireValue::Number(n) => Self::Number(if *n == 0.0 {
                0
            } else if n.is_nan() {
                f64::NAN.to_bits()
            } else {
                n.to_bits()
            }),
            WireValue::Bool(b) => Self::Bool(*b),
            _ => Self::Object(NEXT_OBJECT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)),
        }
    }
}
struct OllamaMetadataResolver {
    ctx: CatalogContext,
    base: WireString,
    cache: Arc<Mutex<HashMap<OllamaMetadataKey, ShowFuture>>>,
}
impl OllamaMetadataResolver {
    async fn resolve(&self, id: &WireString) -> VariantSpec {
        self.resolve_value(&WireValue::String(id.clone())).await
    }
    async fn resolve_value(&self, id: &WireValue) -> VariantSpec {
        let key = OllamaMetadataKey::of(id);
        let pending = {
            let mut cache = self.cache.lock().expect("ollama metadata cache");
            if let Some(pending) = cache.get(&key) {
                pending.clone()
            } else {
                let ctx = self.ctx.clone();
                let base = self.base.clone();
                let id = id.clone();
                let future_key = key.clone();
                let future_cache = self.cache.clone();
                let pending = async move {
                    let reply = ctx
                        .transport
                        .fetch(DiscoveryRequest {
                            url: append(&base, "/api/show"),
                            method: HttpMethod::Post,
                            headers: vec![
                                ("Content-Type".into(), "application/json".into()),
                                ("Accept".into(), "application/json".into()),
                            ],
                            body: Some(WireValue::object(vec![("model", id)]).stringify().into_bytes()),
                            ..Default::default()
                        })
                        .await;
                    let payload = reply.ok().filter(|r| r.ok()).and_then(|r| r.json().ok());
                    let Some(payload) = payload.filter(|p| !matches!(p.value, WireValue::Null)) else {
                        future_cache.lock().expect("ollama metadata cache").remove(&future_key);
                        return None;
                    };
                    let mut meta = wire::empty();
                    let window = payload.record("model_info").and_then(|info| {
                        info.value.entries().and_then(|entries| {
                            entries.into_iter().find_map(|(key, value)| {
                                value.as_number().filter(|n| {
                                    (n.is_nan() || *n > 0.0)
                                        && [".context_length", ".num_ctx", ".context_window"]
                                            .iter()
                                            .any(|suffix| ends(key, suffix))
                                })
                            })
                        })
                    });
                    meta.set("contextWindow", WireValue::Number(window.unwrap_or(128000.0)));
                    meta.set("maxTokens", WireValue::Number(8192.0));
                    if let Some(caps) = payload.get("capabilities").and_then(WireValue::as_array) {
                        let caps: Vec<_> = caps.iter().filter_map(WireValue::as_string).cloned().collect();
                        meta.set(
                            "capabilities",
                            WireValue::Array(caps.iter().cloned().map(WireValue::String).collect()),
                        );
                        meta.set("reasoning", WireValue::Bool(caps.iter().any(|s| s.equals_ascii("thinking"))));
                        meta.set(
                            "input",
                            if caps.iter().any(|s| s.equals_ascii("vision")) {
                                input(Some(&WireValue::Array(vec![wire::str_value("image")])))
                            } else {
                                input(None)
                            },
                        );
                    }
                    Some(meta)
                }
                .boxed()
                .shared();
                cache.insert(key, pending.clone());
                pending
            }
        };
        // Upstream promises keep fetching even when the caller stops awaiting.
        let _background = eager_background(pending.clone()).await;
        pending.await.unwrap_or_else(|| {
            VariantSpec::from_wire(WireValue::object(vec![
                ("contextWindow", WireValue::Number(128000.0)),
                ("maxTokens", WireValue::Number(8192.0)),
            ]))
        })
    }
}
pub fn ollama_model_manager_options(
    default: &CatalogContext,
    config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    let ctx = context(default, &config);
    let value = config
        .base_url
        .as_ref()
        .map(crate::catalog_discovery::js_trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:11434/v1".into());
    let value = trim_one_slash(&value);
    let base = if ends(&value, "/v1") { value } else { append(&value, "/v1") };
    let native = base.slice_prefix(base.len() - 3);
    let resolver = Arc::new(OllamaMetadataResolver {
        ctx: ctx.clone(),
        base: native.clone(),
        cache: Arc::new(Mutex::new(HashMap::new())),
    });
    let refs = Arc::new(wire::create_bundled_reference_map(&"ollama".into()));
    let mut options = ModelManagerOptions::new("ollama");
    options.cache_provider_id = Some(super::super::cache_provider_id::resolve_model_cache_provider_id(
        &"ollama".into(),
        None,
        Some(&base),
        host.environment.as_ref(),
    ));
    options.dynamic_fetcher = Some(dynamic(move || {
        let ctx = ctx.clone();
        let base = base.clone();
        let native = native.clone();
        let refs = refs.clone();
        let resolver = resolver.clone();
        let key = config.api_key.clone();
        async move {
            let mut d = OpenAiCompatibleOptions::new("openai-responses", "ollama", base.clone());
            d.api_key = key;
            d.map_model = Some(Arc::new(move |e, d, _| {
                let r = refs.get(&text(&d, "id").unwrap_or_else(|| "".into()));
                let mut m = map_with_bundled_reference(e, &d, r.map(|r| r.as_ref()));
                if r.is_none() {
                    m.set("contextWindow", WireValue::Number(128000.0));
                    m.set("maxTokens", WireValue::Number(8192.0));
                }
                Ok(Some(Arc::new(m)))
            }));
            let compatible = fetch_openai_compatible_models(&ctx, &d).await?;
            if let Some(models) = compatible.as_ref().filter(|models| !models.is_empty()) {
                let tasks = models.iter().cloned().map(|m| {
                    let resolver = resolver.clone();
                    async move {
                        let meta = resolver.resolve(&text(&m, "id").unwrap_or_else(|| "".into())).await;
                        let mut m = (*m).clone();
                        copy_field(&mut m, "contextWindow", &meta, "contextWindow");
                        if meta.get("reasoning").is_some() {
                            copy_field(&mut m, "reasoning", &meta, "reasoning");
                            m.set_undefined("thinking");
                        }
                        if meta.get("input").is_some() {
                            copy_field(&mut m, "input", &meta, "input");
                        }
                        Arc::new(m)
                    }
                });
                return Ok(RawModelValue::models(futures::future::join_all(tasks).await));
            }
            let tags_reply = ctx
                .transport
                .fetch(DiscoveryRequest {
                    url: append(&native, "/api/tags"),
                    headers: vec![("Accept".into(), "application/json".into())],
                    ..Default::default()
                })
                .await
                .ok()
                .filter(|r| r.ok());
            let tags = tags_reply.as_ref().map(|r| r.json()).transpose()?;
            if let Some(tags) = tags {
                if matches!(tags.value, WireValue::Null) {
                    return Err(DiscoveryError::named(
                        "TypeError",
                        "null is not an object (evaluating '(await response.json()).models')",
                    ));
                }
                let entries = tags
                    .get("models")
                    .filter(|v| !matches!(v, WireValue::Null))
                    .cloned()
                    .unwrap_or(WireValue::Array(Vec::new()));
                let Some(entries) = entries.as_array() else {
                    return Err(DiscoveryError::named(
                        "TypeError",
                        r#"entries.map is not a function. (In 'entries.map(async (entry) => {
    const id = entry.model ?? entry.name;
    if (!id)
      return null;
    const metadata = await resolveMetadata(id);
    return {
      id,
      name: entry.name ?? id,
      api: "openai-responses",
      provider: "ollama",
      baseUrl,
      reasoning: metadata.reasoning ?? !1,
      input: metadata.input ?? ["text"],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      contextWindow: metadata.contextWindow,
      maxTokens: metadata.maxTokens
    };
  })', 'entries.map' is undefined)"#,
                    ));
                };
                let tasks = entries.iter().cloned().map(|entry| {
                    let resolver = resolver.clone();
                    let base = base.clone();
                    async move {
                        if matches!(entry, WireValue::Null) {
                            return Err(DiscoveryError::named(
                                "TypeError",
                                "null is not an object (evaluating 'entry.model')",
                            ));
                        }
                        let entry = VariantSpec::from_wire(entry);
                        let id = entry
                            .get("model")
                            .filter(|v| !matches!(v, WireValue::Null))
                            .or_else(|| entry.get("name"))
                            .cloned();
                        let Some(id) = id.filter(crate::model_collapse::truthy) else {
                            return Ok(None);
                        };
                        let meta = resolver.resolve_value(&id).await;
                        let mut m = VariantSpec::from_wire(WireValue::object(vec![
                            ("id", id.clone()),
                            (
                                "name",
                                entry.get("name").filter(|v| !matches!(v, WireValue::Null)).cloned().unwrap_or(id),
                            ),
                            ("api", wire::str_value("openai-responses")),
                            ("provider", wire::str_value("ollama")),
                            ("baseUrl", wire::str_value(base)),
                            ("reasoning", meta.get("reasoning").cloned().unwrap_or(WireValue::Bool(false))),
                            ("input", meta.get("input").cloned().unwrap_or_else(|| input(None))),
                            ("cost", crate::catalog_discovery::zero_cost()),
                        ]));
                        for k in ["contextWindow", "maxTokens"] {
                            copy_field(&mut m, k, &meta, k);
                        }
                        Ok(Some(Arc::new(m)))
                    }
                });
                let mut models = Vec::new();
                for model in promise_all(tasks).await?.into_iter().flatten() {
                    models.push(model);
                }
                if models.len() > 1 {
                    let mut sort_error = None;
                    models.sort_by(|left,right|{
            let Some(id)=left.get("id").and_then(WireValue::as_string)else{sort_error=Some(DiscoveryError::named("TypeError","left.id.localeCompare is not a function. (In 'left.id.localeCompare(right.id)', 'left.id.localeCompare' is undefined)"));return std::cmp::Ordering::Equal;};
            ctx.locale.compare(id,&js_value_string(right.get("id").expect("native id")))
        });
                    if let Some(error) = sort_error {
                        return Err(error);
                    }
                }
                if !models.is_empty() {
                    return Ok(RawModelValue::models(models));
                }
            }
            Ok(compatible.map_or_else(RawModelValue::null, RawModelValue::models))
        }
    }));
    options
}
