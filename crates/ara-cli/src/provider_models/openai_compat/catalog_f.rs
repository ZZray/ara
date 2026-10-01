//! Fixed OMP Wafer, OpenCode gateway routing, and Fireworks control-plane catalogs.
use super::super::{
    cache_provider_id::{get_default_model_discovery_base_url, join, resolve_model_cache_provider_id},
    static_data::fixed_models,
};
use super::*;
use crate::{
    catalog_discovery::{DiscoveryRequest, zero_cost},
    catalog_rules,
    model_wire_policy::classify_wire,
};
use async_trait::async_trait;

fn glm_reasoning(provider: &WireString, id: &WireString) -> Result<bool, DiscoveryError> {
    let identity = classify_wire(provider, id, true)?;
    let revision =
        text(&identity, "revision").and_then(|r| r.to_utf8().ok()).and_then(|r| catalog_rules::parse_revision(&r));
    let family = text(&identity, "family");
    let floor = catalog_rules::parse_revision(if family.as_ref().is_some_and(|f| f.equals_ascii("flash")) {
        "5.3"
    } else {
        "4.5"
    });
    Ok(text(&identity, "class").is_some_and(|c| c.equals_ascii("glm"))
        && family.as_ref().is_none_or(|f| ["air", "turbo", "flash"].iter().any(|s| f.equals_ascii(s)))
        && revision.zip(floor).is_some_and(|(r, f)| catalog_rules::compare_revision(r, f) >= 0))
}
pub fn resolve_wafer_serverless_thinking_format(
    id: &WireString,
    upstream: Option<&WireValue>,
) -> Result<Option<WireString>, DiscoveryError> {
    let upstream = upstream
        .and_then(WireValue::as_string)
        .map(crate::catalog_discovery::js_trim)
        .map(|s| lower(&s))
        .unwrap_or_else(|| "".into());
    if !upstream.is_empty() {
        if ["zai", "z.ai", "z-ai"].iter().any(|s| upstream.equals_ascii(s))
            || ["zhipu", "moonshot", "kimi"].iter().any(|s| includes(&upstream, s))
        {
            return Ok(Some("zai".into()));
        }
        if ["qwen", "alibaba", "dashscope"].iter().any(|s| includes(&upstream, s)) {
            return Ok(Some("qwen".into()));
        }
        return Ok(None);
    }
    let identity = classify_wire(&"wafer-serverless".into(), id, true)?;
    Ok((glm_reasoning(&"wafer-serverless".into(), id)?
        || text(&identity, "class").is_some_and(|c| c.equals_ascii("kimi")))
    .then(|| "zai".into()))
}
pub fn wafer_serverless_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let mapper = Arc::new(|e: &VariantSpec, d: &VariantSpec, _: Option<&VariantSpec>| {
        let wafer = e.record("wafer").filter(|v| matches!(v.value, WireValue::Object(_) | WireValue::Array(_)));
        let caps = wafer.as_ref().and_then(|v| v.record("capabilities"));
        let price = wafer.as_ref().and_then(|v| v.record("pricing"));
        let reason = caps.as_ref().is_some_and(|v| wire::boolean(v, "reasoning") == Some(true));
        let vision = caps.as_ref().is_some_and(|v| wire::boolean(v, "vision") == Some(true));
        let window = positive(
            wafer.as_ref().and_then(|v| v.get("context_length")),
            Some(&positive(e.get("max_model_len"), d.get("contextWindow"))),
        );
        let mut m = d.clone();
        m.set(
            "name",
            wire::str_value(name(
                wafer.as_ref().and_then(|v| v.get("display_name")),
                &text(d, "name").unwrap_or_else(|| "".into()),
            )),
        );
        m.set("reasoning", WireValue::Bool(reason));
        m.set(
            "input",
            if vision { input(Some(&WireValue::Array(vec![wire::str_value("image")]))) } else { input(None) },
        );
        m.set(
            "cost",
            WireValue::object(
                [
                    ("input", "input_cents_per_million"),
                    ("output", "output_cents_per_million"),
                    ("cacheRead", "cache_read_cents_per_million"),
                ]
                .iter()
                .map(|(k, v)| {
                    (
                        *k,
                        WireValue::Number(
                            to_number(price.as_ref().and_then(|p| p.get(v))).filter(|n| *n > 0.0).unwrap_or(0.0)
                                * 125.0
                                / 10000.0,
                        ),
                    )
                })
                .chain(std::iter::once(("cacheWrite", WireValue::Number(0.0))))
                .collect(),
            ),
        );
        m.set("maxTokens", window.as_number().map(|n| WireValue::Number(n.min(65536.0))).unwrap_or(WireValue::Null));
        m.set("contextWindow", window);
        if caps.as_ref().is_some_and(|c| wire::boolean(c, "tools") == Some(false)) {
            m.set("supportsTools", WireValue::Bool(false));
        }
        let mut compat = wire::empty();
        if reason {
            if let Some(format) = resolve_wafer_serverless_thinking_format(
                &text(d, "id").unwrap_or_else(|| "".into()),
                wafer.as_ref().and_then(|w| w.get("provider")),
            )? {
                compat.set("thinkingFormat", wire::str_value(format));
            }
            compat.set("reasoningContentField", wire::str_value("reasoning_content"));
        }
        compat.set("supportsDeveloperRole", WireValue::Bool(false));
        m.set_record("compat", &compat);
        Ok(Some(m))
    });
    compatible_options(
        ctx,
        CompatibleBuilder {
            api: "openai-completions".into(),
            provider: "wafer-serverless".into(),
            default_base: "https://pass.wafer.ai/v1".into(),
            config,
            headers: Vec::new(),
            authoritative: false,
            require_api_key: true,
            drop_ids: None,
            map: mapper,
            filter: None,
        },
    )
}

pub(super) fn muse_lineage(id: &WireString) -> Result<Option<VariantSpec>, DiscoveryError> {
    let identity = classify_wire(&"meta".into(), id, true)?;
    if !text(&identity, "family").is_some_and(|s| s.equals_ascii("muse-spark")) {
        return Ok(None);
    }
    let Some(revision) = text(&identity, "revision") else {
        return Ok(None);
    };
    let parsed = revision.to_utf8().ok().and_then(|s| catalog_rules::parse_revision(&s)).unwrap_or([0, 0, 0]);
    let revision = if parsed[2] == 0 { format!("{}.{}", parsed[0], parsed[1]).into() } else { revision };
    let contributor = catalog_rules::billing_variant_plain(&String::from_utf16_lossy(id.units())).is_some();
    let mut m = (*fixed_models("META_MUSE_STATIC_MODELS")[if contributor { 2 } else { 0 }]).clone();
    m.set(
        "id",
        wire::str_value(append(&join(&"muse-spark-".into(), &revision), if contributor { "-contributor" } else { "" })),
    );
    m.set(
        "name",
        wire::str_value(append(&join(&"Muse Spark ".into(), &revision), if contributor { " (C)" } else { "" })),
    );
    Ok(Some(m))
}
struct OpenCodeRoutes {
    provider: WireString,
    base: WireString,
    references: HashMap<WireString, SpecRef>,
    sibling: HashMap<WireString, SpecRef>,
}
impl OpenCodeRoutes {
    fn resolve(&self, id: &WireString, default: &WireString) -> WireString {
        let base = ["-contributor", "-free"]
            .iter()
            .find(|s| ends(id, s) && id.len() > s.len())
            .map(|s| id.slice_prefix(id.len() - s.len()));
        let pinned = |id: &WireString| {
            api_route_for(&self.provider, id).and_then(|r| text(&r, "api")).filter(|a| {
                ["openai-completions", "openai-responses", "anthropic-messages"].iter().any(|s| a.equals_ascii(s))
            })
        };
        pinned(id)
            .or_else(|| base.as_ref().and_then(pinned))
            .or_else(|| self.references.get(id).and_then(|r| text(r, "api")))
            .unwrap_or_else(|| {
                let hints = [
                    self.sibling.get(id),
                    base.as_ref().and_then(|id| self.references.get(id)),
                    base.as_ref().and_then(|id| self.sibling.get(id)),
                ];
                if hints
                    .iter()
                    .any(|r| r.and_then(|r| text(r, "api")).is_some_and(|s| s.equals_ascii("openai-responses")))
                {
                    "openai-responses".into()
                } else {
                    default.clone()
                }
            })
    }
    fn base_for(&self, api: &WireString) -> WireString {
        if api.equals_ascii("anthropic-messages") { self.base.clone() } else { append(&self.base, "/v1") }
    }
}
struct OpenCodeFallback {
    ctx: CatalogContext,
    host: ProviderFactoryHost,
    explicit: bool,
    routes: Arc<OpenCodeRoutes>,
}
#[async_trait]
impl crate::model_manager::ModelsDevFallback for OpenCodeFallback {
    async fn fetch(&self) -> Result<RawModelValue, DiscoveryError> {
        Ok(RawModelValue::Value(fetch_revalidated_with_timeout(&self.ctx, &self.host, self.explicit, 10000.0).await?))
    }
    fn map(&self, payload: RawModelValue, _: &WireString) -> Result<RawModelValue, DiscoveryError> {
        let RawModelValue::Value(payload) = payload else {
            return Ok(RawModelValue::models(Vec::new()));
        };
        if !matches!(payload.value, WireValue::Object(_)) {
            return Ok(RawModelValue::models(Vec::new()));
        }
        let descriptors: Vec<_> = models_dev_provider_descriptors()
            .into_iter()
            .filter(|d| d.provider_id.equals_ascii("opencode-go") || d.provider_id.equals_ascii("opencode-zen"))
            .collect();
        let mut out = Vec::new();
        for m in map_models_dev_to_models(&payload, &descriptors)? {
            if text(&m, "provider") != Some(self.routes.provider.clone()) {
                continue;
            }
            let mut m = (*m).clone();
            let api = self.routes.resolve(&text(&m, "id").unwrap_or_else(|| "".into()), &"openai-completions".into());
            m.set("baseUrl", wire::str_value(self.routes.base_for(&api)));
            m.set("api", wire::str_value(api));
            out.push(Arc::new(m));
        }
        Ok(RawModelValue::models(out))
    }
}
fn opencode_options(
    default: &CatalogContext,
    config: ModelManagerConfig,
    host: &ProviderFactoryHost,
    provider: &str,
) -> ModelManagerOptions {
    let provider: WireString = provider.into();
    let ctx = context(default, &config);
    let explicit = config.context.is_some();
    let fallback =
        get_default_model_discovery_base_url(&provider, host.environment.as_ref()).expect("OpenCode default");
    let fallback = fallback.slice_prefix(fallback.len() - 3);
    let base = config
        .base_url
        .as_ref()
        .map(crate::catalog_discovery::js_trim)
        .filter(|s| !s.is_empty())
        .map(|s| trim_one_slash(&s))
        .unwrap_or(fallback);
    let base = if ends(&base, "/v1") { base.slice_prefix(base.len() - 3) } else { base };
    let discovery_base = append(&base, "/v1");
    let routes = Arc::new(OpenCodeRoutes {
        provider: provider.clone(),
        base,
        references: wire::create_bundled_reference_map(&provider),
        sibling: wire::create_bundled_reference_map(&if provider.equals_ascii("opencode-go") {
            "opencode-zen".into()
        } else {
            "opencode-go".into()
        }),
    });
    let mut options = ModelManagerOptions::new(provider.clone());
    options.dynamic_models_authoritative = true;
    options.cache_provider_id = Some(resolve_model_cache_provider_id(
        &provider,
        config.api_key.as_ref(),
        Some(&discovery_base),
        host.environment.as_ref(),
    ));
    let mut drop = api_route_exact_model_ids(&provider);
    drop.push("glm-5.3-flash".into());
    if provider.equals_ascii("opencode-zen") {
        drop.push("gemini-3.7-flash".into());
    }
    options.drop_cached_model_ids_on_static_mismatch = Some(drop);
    options.models_dev =
        Some(Arc::new(OpenCodeFallback { ctx: ctx.clone(), host: host.clone(), explicit, routes: routes.clone() }));
    if config.api_key.as_ref().is_some_and(|s| !s.is_empty()) {
        let mut d = OpenAiCompatibleOptions::new("openai-completions", provider.clone(), discovery_base);
        d.api_key = config.api_key;
        d.map_model = Some(Arc::new(move |e, d, _| {
            let id = text(&d, "id").unwrap_or_else(|| "".into());
            let reference = routes.references.get(&id);
            let api = routes.resolve(&id, &text(&d, "api").unwrap_or_else(|| "".into()));
            let base = routes.base_for(&api);
            let lineage = muse_lineage(&id)?;
            let mut m = reference.map(|r| (**r).clone()).unwrap_or_else(|| (*d).clone());
            m.set("id", wire::str_value(id));
            m.set(
                "name",
                wire::str_value(name(
                    e.get("name"),
                    &reference.and_then(|r| text(r, "name")).or_else(|| text(&d, "name")).unwrap_or_else(|| "".into()),
                )),
            );
            m.set("api", wire::str_value(api));
            m.set("baseUrl", wire::str_value(base));
            if let Some(lineage) = lineage {
                m.set("provider", wire::str_value(routes.provider.clone()));
                m.set("reasoning", WireValue::Bool(true));
                for k in ["input", "thinking"] {
                    if let Some(value) = reference
                        .and_then(|r| r.get(k))
                        .filter(|v| !matches!(v, WireValue::Null))
                        .or_else(|| lineage.get(k))
                    {
                        m.set(k, value.clone());
                    }
                }
                m.set(
                    "contextWindow",
                    positive(
                        e.get("context_length"),
                        reference
                            .and_then(|r| r.get("contextWindow"))
                            .filter(|v| !matches!(v, WireValue::Null))
                            .or_else(|| lineage.get("contextWindow")),
                    ),
                );
                m.set(
                    "maxTokens",
                    positive(
                        e.get("max_completion_tokens"),
                        reference
                            .and_then(|r| r.get("maxTokens"))
                            .filter(|v| !matches!(v, WireValue::Null))
                            .or_else(|| lineage.get("maxTokens")),
                    ),
                );
            } else if let Some(r) = reference {
                m.set("contextWindow", positive(e.get("context_length"), r.get("contextWindow")));
                m.set("maxTokens", positive(e.get("max_completion_tokens"), r.get("maxTokens")));
            }
            Ok(Some(Arc::new(m)))
        }));
        options.dynamic_fetcher = Some(dynamic(move || {
            let ctx = ctx.clone();
            let d = d.clone();
            async move { RawModelValue::from_discovery(fetch_openai_compatible_models(&ctx, &d).await) }
        }));
    }
    options
}
pub fn opencode_go_model_manager_options(
    ctx: &CatalogContext,
    config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    opencode_options(ctx, config, host, "opencode-go")
}
pub fn opencode_zen_model_manager_options(
    ctx: &CatalogContext,
    config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    opencode_options(ctx, config, host, "opencode-zen")
}

pub const FIREWORKS_KIMI_MAX_TOKENS: f64 = 32768.0;
pub const FIREWORKS_KIMI_K27_CODE_MAX_TOKENS: f64 = 65536.0;
pub fn is_fireworks_kimi_k2_model_id(id: &WireString) -> Result<bool, DiscoveryError> {
    let identity = classify_wire(&"fireworks".into(), id, true)?;
    let family = text(&identity, "family");
    Ok(text(&identity, "class").is_some_and(|c| c.equals_ascii("kimi"))
        && family.is_some_and(|f| starts(&f, "k2") && !f.equals_ascii("k2.7-code")))
}
pub fn clamp_fireworks_kimi_max_tokens(id: &WireString, value: WireValue) -> Result<WireValue, DiscoveryError> {
    if matches!(value, WireValue::Null) {
        return Ok(value);
    }
    Ok(if is_fireworks_kimi_k2_model_id(id)? {
        WireValue::Number(js_min(value.as_number().unwrap_or(f64::NAN), FIREWORKS_KIMI_MAX_TOKENS))
    } else {
        value
    })
}
pub fn build_fireworks_fast_seed() -> Vec<SpecRef> {
    let refs = wire::create_bundled_reference_map(&"fireworks".into());
    [
        ("kimi-k2.7-code", "Kimi K2.7 Code Fast", 1.9, 8.0, 0.38),
        ("kimi-k2.6", "Kimi K2.6 Fast", 2.0, 8.0, 0.3),
        ("glm-5.1", "GLM-5.1 Fast", 2.8, 8.8, 0.52),
        ("glm-5.2", "GLM-5.2 Fast", 2.1, 6.6, 0.21),
    ]
    .into_iter()
    .filter_map(|(id, name, i, o, c)| {
        let mut m = (**refs.get(&WireString::from(id))?).clone();
        m.set("id", wire::str_value(format!("{id}-fast")));
        m.set("name", wire::str_value(name));
        m.set(
            "cost",
            WireValue::object(vec![
                ("input", WireValue::Number(i)),
                ("output", WireValue::Number(o)),
                ("cacheRead", WireValue::Number(c)),
                ("cacheWrite", WireValue::Number(0.0)),
            ]),
        );
        Some(Arc::new(m))
    })
    .collect()
}
pub fn strip_fireworks_deepseek_thinking_toggle(
    mut m: VariantSpec,
    id: &WireString,
) -> Result<VariantSpec, DiscoveryError> {
    let identity = classify_wire(&"fireworks".into(), id, true)?;
    let v4 = text(&identity, "class").is_some_and(|c| c.equals_ascii("deepseek"))
        && text(&identity, "family").is_some_and(|f| ["v4", "flash", "pro"].iter().any(|s| f.equals_ascii(s)));
    if !v4 {
        return Ok(m);
    }
    let Some(mut compat) = m.record("compat") else {
        return Ok(m);
    };
    let Some(mut extra) = compat.record("extraBody") else {
        return Ok(m);
    };
    if !extra.own_keys().iter().any(|k| k.equals_ascii("thinking")) {
        return Ok(m);
    }
    extra.remove("thinking");
    if extra.own_keys().is_empty() {
        compat.remove("extraBody");
    } else {
        compat.set_record("extraBody", &extra);
    }
    m.set_record("compat", &compat);
    Ok(m)
}
fn public_fireworks_id(id: &WireString) -> WireString {
    let prefix = "accounts/fireworks/models/";
    let mut units = if starts(id, prefix) { id.units()[prefix.len()..].to_vec() } else { id.units().to_vec() };
    for i in 1..units.len().saturating_sub(1) {
        if units[i] == 112 && (48..=57).contains(&units[i - 1]) && (48..=57).contains(&units[i + 1]) {
            units[i] = 46;
        }
    }
    WireString::from_units(units)
}
fn models_dev_references(models: Vec<SpecRef>) -> HashMap<WireString, SpecRef> {
    let mut refs: HashMap<WireString, SpecRef> = HashMap::new();
    for m in models {
        let Some(id) = text(&m, "id") else {
            continue;
        };
        let better = refs.get(&id).is_none_or(|r| {
            let c = m.get("contextWindow").and_then(WireValue::as_number).unwrap_or(0.0);
            let rc = r.get("contextWindow").and_then(WireValue::as_number).unwrap_or(0.0);
            c > rc
                || m.get("contextWindow") == r.get("contextWindow")
                    && m.get("maxTokens").and_then(WireValue::as_number).unwrap_or(0.0)
                        > r.get("maxTokens").and_then(WireValue::as_number).unwrap_or(0.0)
        });
        if better {
            refs.insert(id, m);
        }
    }
    refs
}
pub fn fireworks_model_manager_options(
    default: &CatalogContext,
    config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    let mut options = ModelManagerOptions::new("fireworks");
    let Some(key) = config.api_key.as_ref().filter(|s| !s.is_empty()).cloned() else {
        return options;
    };
    let ctx = context(default, &config);
    let explicit = config.context.is_some();
    let host = host.clone();
    let base = config.base_url.unwrap_or_else(|| "https://api.fireworks.ai/inference/v1".into());
    let bundled = Arc::new(wire::ReferenceResolver::lazy(|| wire::create_bundled_reference_map(&"fireworks".into())));
    options.dynamic_fetcher = Some(dynamic(move || {
        let ctx = ctx.clone();
        let host = host.clone();
        let base = base.clone();
        let key = key.clone();
        let bundled = bundled.clone();
        async move {
            let refs = fetch_well_known_models(&ctx, &host, explicit, None)
                .await
                .ok()
                .and_then(|p| map_models_dev_to_models(&p, &models_dev_provider_descriptors()).ok())
                .map(models_dev_references)
                .unwrap_or_default();
            let Ok(mut list) = reqwest::Url::parse(&String::from_utf16_lossy(base.units())) else {
                return Ok(RawModelValue::null());
            };
            let origin = list.origin().ascii_serialization();
            if origin == "null" {
                return Ok(RawModelValue::null());
            }
            list = reqwest::Url::parse(&format!("{origin}/v1/accounts/fireworks/models")).expect("Fireworks list URL");
            let mut models = Vec::<SpecRef>::new();
            let mut token: WireString = "".into();
            for _ in 0..25 {
                let mut url = list.clone();
                {
                    let mut query = url.query_pairs_mut();
                    query.append_pair("filter", "supports_serverless=true");
                    query.append_pair("pageSize", "200");
                    if !token.is_empty() {
                        query.append_pair("pageToken", &String::from_utf16_lossy(token.units()));
                    }
                }
                let reply = ctx
                    .transport
                    .fetch(DiscoveryRequest {
                        url: url.to_string().into(),
                        headers: vec![
                            ("Accept".into(), "application/json".into()),
                            ("Authorization".into(), join(&"Bearer ".into(), &key)),
                        ],
                        ..Default::default()
                    })
                    .await;
                let payload = reply
                    .ok()
                    .filter(|r| r.ok())
                    .and_then(|r| r.json().ok())
                    .filter(|p| matches!(p.value, WireValue::Object(_)));
                let Some(payload) = payload else {
                    return Ok(RawModelValue::null());
                };
                for e in payload.get("models").and_then(WireValue::as_array).unwrap_or(&[]) {
                    if !matches!(e, WireValue::Object(_)) {
                        continue;
                    }
                    let e = VariantSpec::from_wire(e.clone());
                    if wire::boolean(&e, "supportsServerless") != Some(true)
                        || text(&e, "state").is_some_and(|s| !s.equals_ascii("READY"))
                    {
                        continue;
                    }
                    let Some(id) = text(&e, "name")
                        .filter(|s| !s.is_empty())
                        .map(|s| public_fireworks_id(&s))
                        .filter(|s| !s.is_empty())
                    else {
                        continue;
                    };
                    let reference = refs.get(&id).cloned().or_else(|| bundled.resolve(&id));
                    let window =
                        positive(e.get("contextLength"), reference.as_ref().and_then(|r| r.get("contextWindow")));
                    let cap = if is_kimi_k27_code_model_id(&id)? {
                        WireValue::Number(65536.0)
                    } else if is_fireworks_kimi_k2_model_id(&id)? {
                        WireValue::Number(32768.0)
                    } else {
                        WireValue::Null
                    };
                    let cap = clamp_fireworks_kimi_max_tokens(
                        &id,
                        reference
                            .as_ref()
                            .and_then(|r| r.get("maxTokens"))
                            .filter(|v| !matches!(v, WireValue::Null))
                            .cloned()
                            .unwrap_or(cap),
                    )?;
                    let mut m = reference.as_ref().map(|r| (**r).clone()).unwrap_or_else(wire::empty);
                    m.set("id", wire::str_value(id.clone()));
                    m.set(
                        "name",
                        wire::str_value(name(
                            e.get("displayName"),
                            &reference.as_ref().and_then(|r| text(r, "name")).unwrap_or(id.clone()),
                        )),
                    );
                    m.set("api", wire::str_value("openai-completions"));
                    m.set("provider", wire::str_value("fireworks"));
                    m.set("baseUrl", wire::str_value(base.clone()));
                    m.set(
                        "reasoning",
                        reference
                            .as_ref()
                            .and_then(|r| r.get("reasoning"))
                            .filter(|v| !matches!(v, WireValue::Null))
                            .cloned()
                            .unwrap_or(WireValue::Bool(true)),
                    );
                    m.set(
                        "input",
                        if wire::boolean(&e, "supportsImageInput") == Some(true) {
                            input(Some(&WireValue::Array(vec![wire::str_value("image")])))
                        } else {
                            reference
                                .as_ref()
                                .and_then(|r| r.get("input"))
                                .filter(|v| !matches!(v, WireValue::Null))
                                .cloned()
                                .unwrap_or_else(|| input(None))
                        },
                    );
                    if reference.is_none() {
                        m.set("cost", zero_cost());
                    }
                    m.set("contextWindow", window);
                    m.set("maxTokens", cap);
                    if wire::boolean(&e, "supportsTools") == Some(false) {
                        m.set("supportsTools", WireValue::Bool(false));
                    }
                    let m = Arc::new(strip_fireworks_deepseek_thinking_toggle(m, &id)?);
                    if let Some(at) = models.iter().position(|m| text(m, "id") == Some(id.clone())) {
                        models[at] = m;
                    } else {
                        models.push(m);
                    }
                }
                token = text(&payload, "nextPageToken").unwrap_or_else(|| "".into());
                if token.is_empty() {
                    break;
                }
            }
            Ok(RawModelValue::models(models))
        }
    }));
    options
}
pub fn firepass_model_manager_options() -> ModelManagerOptions {
    ModelManagerOptions::new("firepass")
}
