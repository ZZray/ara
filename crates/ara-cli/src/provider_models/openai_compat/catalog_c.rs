//! Fixed first-party xAI curation and endpoint-owned SiliconFlow/Zhipu rosters.
use super::super::{catalog_session::fetch_well_known_models, static_data::fixed_literal};
use super::*;
use crate::{
    catalog_discovery::{DiscoverySignal, zero_cost},
    catalog_rules,
    model_collapse::{CollapseModelPolicy, truthy},
    model_wire_policy::{WireModelPolicy, classify_wire},
};
fn build(api: &str, provider: &str, base: &str, config: ModelManagerConfig, map: ProviderMapper) -> CompatibleBuilder {
    CompatibleBuilder {
        api: api.into(),
        provider: provider.into(),
        default_base: base.into(),
        config,
        headers: Vec::new(),
        authoritative: false,
        require_api_key: true,
        drop_ids: None,
        map,
        filter: None,
    }
}
fn priced(model: &VariantSpec) -> bool {
    model.record("cost").is_some_and(|c| {
        ["input", "output", "cacheRead", "cacheWrite"]
            .iter()
            .any(|k| c.get(k).and_then(WireValue::as_number).is_some_and(|n| n > 0.0))
    })
}
pub fn apply_xai_catalog_pricing(models: &[SpecRef]) -> Vec<SpecRef> {
    let prices: HashMap<_, _> = models
        .iter()
        .filter(|m| text(m, "provider").is_some_and(|p| p.equals_ascii("xai")) && priced(m))
        .filter_map(|m| Some((text(m, "id")?, m.record("cost")?)))
        .collect();
    models
        .iter()
        .map(|m| {
            if !text(m, "provider").is_some_and(|p| p.equals_ascii("xai-oauth")) || priced(m) {
                return m.clone();
            }
            let id = text(m, "id").unwrap_or_else(|| "".into());
            let peer = crate::catalog_rules::compiled_rules()["behavior"]["pricingPeers"]
                .as_array()
                .and_then(|tables| tables.iter().find(|t| t["provider"].as_str() == Some("xai-oauth")))
                .and_then(|table| table["aliases"].as_array())
                .and_then(|aliases| aliases.iter().find(|a| a["model"].as_str().is_some_and(|s| id.equals_ascii(s))))
                .and_then(|alias| alias["peerId"].as_str())
                .map(WireString::from);
            let cost = prices.get(&id).or_else(|| peer.as_ref().and_then(|id| prices.get(id)));
            if let Some(cost) = cost {
                let mut model = (**m).clone();
                model.set_record("cost", cost);
                Arc::new(model)
            } else {
                m.clone()
            }
        })
        .collect()
}
pub fn xai_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let mut options =
        compatible_options(ctx, build("openai-responses", "xai", "https://api.x.ai/v1", config, bundled_mapper()));
    options.drop_cached_model_ids_on_static_mismatch =
        Some(wire::bundled_provider_models(&"xai".into()).iter().filter_map(|m| text(m, "id")).collect());
    options
}
fn curated_rows() -> Vec<VariantSpec> {
    let data = fixed_literal("XAI_OAUTH_CURATED_MODELS");
    data.value.as_array().expect("curated rows").iter().cloned().map(VariantSpec::from_wire).collect()
}
fn merge_curated(mut base: VariantSpec, curated: &VariantSpec) -> Result<VariantSpec, DiscoveryError> {
    let mut probe = base.clone();
    copy_field(&mut probe, "id", curated, "id");
    probe.set("provider", wire::str_value("xai-oauth"));
    let policy = WireModelPolicy.resolve(&probe)?;
    let capable = curated
        .get("supportsReasoningEffort")
        .filter(|v| !matches!(v, WireValue::Null))
        .cloned()
        .or_else(|| policy.record("compat").and_then(|c| c.get("supportsReasoningEffort").cloned()));
    let mut compat = base.record("compat").unwrap_or_else(wire::empty);
    for (key, fallback) in
        [("includeEncryptedReasoning", true), ("filterReasoningHistory", false), ("supportsImageDetailOriginal", false)]
    {
        let value = if key == "filterReasoningHistory" {
            WireValue::Bool(false)
        } else {
            compat.get(key).filter(|v| !matches!(v, WireValue::Null)).cloned().unwrap_or(WireValue::Bool(fallback))
        };
        compat.set(key, value);
    }
    compat.set("omitReasoningEffort", WireValue::Bool(!capable.as_ref().is_some_and(truthy)));
    if let Some(capable) = capable {
        compat.set("supportsReasoningEffort", capable);
    } else {
        compat.set_undefined("supportsReasoningEffort");
    }
    if compat.get("supportsReasoningEffort").is_some_and(truthy) {
        compat.set_record(
            "reasoningEffortMap",
            &xai_responses_reasoning_effort_map(&text(curated, "id").unwrap_or_else(|| "".into()))?,
        );
    } else {
        compat.remove("reasoningEffortMap");
    }
    for key in ["contextWindow", "maxTokens"] {
        copy_field(&mut base, key, curated, "contextWindow");
    }
    for key in ["name", "input"] {
        if curated.get(key).is_some_and(|v| !matches!(v, WireValue::Null)) {
            copy_field(&mut base, key, curated, key);
        }
    }
    base.set(
        "reasoning",
        curated.get("reasoning").filter(|v| !matches!(v, WireValue::Null)).cloned().unwrap_or(WireValue::Bool(true)),
    );
    base.set_record("compat", &compat);
    Ok(base)
}
pub fn build_xai_oauth_static_seed(base: Option<&WireString>) -> Result<Vec<SpecRef>, DiscoveryError> {
    let base = base.cloned().unwrap_or_else(|| "https://api.x.ai/v1".into());
    curated_rows()
        .iter()
        .map(|curated| {
            let id = text(curated, "id").expect("curated id");
            let mut model = VariantSpec::from_wire(WireValue::object(vec![
                ("id", wire::str_value(id.clone())),
                ("name", wire::str_value(id.clone())),
                ("api", wire::str_value("openai-responses")),
                ("provider", wire::str_value("xai-oauth")),
                ("baseUrl", wire::str_value(base.clone())),
                ("reasoning", WireValue::Bool(true)),
                ("input", input(None)),
                ("cost", zero_cost()),
            ]));
            copy_field(&mut model, "contextWindow", curated, "contextWindow");
            copy_field(&mut model, "maxTokens", curated, "contextWindow");
            let mut compat = wire::empty();
            compat.set_record("reasoningEffortMap", &xai_responses_reasoning_effort_map(&id)?);
            model.set_record("compat", &compat);
            Ok(Arc::new(merge_curated(model, curated)?))
        })
        .collect()
}
pub fn apply_xai_oauth_curation(models: &[SpecRef]) -> Result<Vec<SpecRef>, DiscoveryError> {
    let filtered: Vec<_> = models
        .iter()
        .filter(|m| !is_excluded_model(&"xai-oauth".into(), &text(m, "id").unwrap_or_else(|| "".into())))
        .cloned()
        .collect();
    let curated = curated_rows();
    let mut by_id: HashMap<_, _> = filtered.iter().filter_map(|m| text(m, "id").map(|id| (id, m.clone()))).collect();
    for c in &curated {
        let id = text(c, "id").expect("curated id");
        if let Some(existing) = by_id.get(&id) {
            by_id.insert(id, Arc::new(merge_curated((**existing).clone(), c)?));
        }
    }
    if let Some(template) = filtered.first() {
        for c in &curated {
            let id = text(c, "id").expect("curated id");
            if let std::collections::hash_map::Entry::Vacant(entry) = by_id.entry(id.clone()) {
                let mut base = (**template).clone();
                base.set("id", wire::str_value(id.clone()));
                base.set("name", wire::str_value(id.clone()));
                entry.insert(Arc::new(merge_curated(base, c)?));
            }
        }
    }
    let mut out = Vec::new();
    for c in &curated {
        if let Some(model) = by_id.remove(&text(c, "id").expect("curated id")) {
            out.push(model);
        }
    }
    for model in filtered {
        if curated.iter().any(|c| text(c, "id") == text(&model, "id")) {
            continue;
        }
        let mut model = (*model).clone();
        let policy = WireModelPolicy.resolve(&model)?;
        let mut compat = model.record("compat").unwrap_or_else(wire::empty);
        for (key, fallback) in [
            ("includeEncryptedReasoning", true),
            ("filterReasoningHistory", false),
            ("supportsImageDetailOriginal", false),
        ] {
            let value =
                compat.get(key).filter(|v| !matches!(v, WireValue::Null)).cloned().unwrap_or(WireValue::Bool(fallback));
            compat.set(key, value);
        }
        if compat.get("omitReasoningEffort").is_none_or(|v| matches!(v, WireValue::Null)) {
            if let Some(value) = policy.record("compat").and_then(|c| c.get("omitReasoningEffort").cloned()) {
                compat.set("omitReasoningEffort", value);
            } else {
                compat.set_undefined("omitReasoningEffort");
            }
        }
        model.set_record("compat", &compat);
        out.push(Arc::new(model));
    }
    Ok(out)
}
pub fn xai_oauth_model_manager_options(
    ctx: &CatalogContext,
    config: ModelManagerConfig,
) -> Result<ModelManagerOptions, DiscoveryError> {
    let seeds = build_xai_oauth_static_seed(config.base_url.as_ref())?;
    let mut options = compatible_options(
        ctx,
        build("openai-responses", "xai-oauth", "https://api.x.ai/v1", config, bundled_mapper()),
    );
    options.static_models = Some(RawModelValue::models(seeds));
    if let Some(inner) = options.dynamic_fetcher.take() {
        options.dynamic_fetcher = Some(dynamic(move || {
            let inner = inner.clone();
            async move {
                let raw = inner.fetch().await?;
                match raw {
                    RawModelValue::Models(models) => {
                        Ok(RawModelValue::models(apply_xai_oauth_curation(&models.snapshot())?))
                    }
                    other => Ok(other),
                }
            }
        }));
    }
    Ok(options)
}
pub fn is_likely_aiml_api_chat_model_id(id: &WireString) -> bool {
    let id = lower(&crate::catalog_discovery::js_trim(id));
    !id.is_empty() && !is_excluded_model(&"aimlapi".into(), &id)
}
pub fn is_likely_siliconflow_chat_model_id(id: &WireString) -> bool {
    let id = lower(&crate::catalog_discovery::js_trim(id));
    !id.is_empty() && !is_excluded_model(&"siliconflow".into(), &id)
}
pub fn aiml_api_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let mut b = build("openai-completions", "aimlapi", "https://api.aimlapi.com/v1", config, bundled_mapper());
    b.authoritative = true;
    b.filter =
        Some(Arc::new(|_, m, _| Ok(is_likely_aiml_api_chat_model_id(&text(m, "id").unwrap_or_else(|| "".into())))));
    compatible_options(ctx, b)
}
pub fn deepseek_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    create_simple_openai_completions_options(ctx, "deepseek", "https://api.deepseek.com", config)
}
fn siliconflow_options(
    default: &CatalogContext,
    config: ModelManagerConfig,
    host: &ProviderFactoryHost,
    provider: &str,
    base: &str,
) -> ModelManagerOptions {
    let mut options = ModelManagerOptions::new(provider);
    options.dynamic_models_authoritative = true;
    if config.api_key.as_ref().is_none_or(WireString::is_empty) {
        return options;
    }
    let context = context(default, &config);
    let explicit = config.context.is_some();
    let host = host.clone();
    let provider: WireString = provider.into();
    let base = config.base_url.unwrap_or_else(|| base.into());
    let key = config.api_key;
    options.dynamic_fetcher = Some(dynamic(move || {
        let context = context.clone();
        let host = host.clone();
        let provider = provider.clone();
        let base = base.clone();
        let key = key.clone();
        async move {
            let signal = DiscoverySignal::default();
            let timer = start_discovery_timeout(&signal, 5000.0);
            let payload = fetch_well_known_models(&context, &host, explicit, Some(signal)).await;
            drop(timer);
            let mut descriptor = ModelsDevProviderDescriptor::new(
                if provider.equals_ascii("siliconflow-cn") { "siliconflow-cn" } else { "siliconflow" },
                if provider.equals_ascii("siliconflow-cn") { "siliconflow-cn" } else { "siliconflow" },
                "openai-completions",
                if provider.equals_ascii("siliconflow-cn") {
                    "https://api.siliconflow.cn/v1"
                } else {
                    "https://api.siliconflow.com/v1"
                },
            );
            descriptor.filter_model = Some(Arc::new(|_, _| Ok(true)));
            let refs: HashMap<_, _> = payload
                .ok()
                .and_then(|p| map_models_dev_to_models(&p, &[descriptor]).ok())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|m| text(&m, "id").map(|id| (id, m)))
                .collect();
            let mut d = OpenAiCompatibleOptions::new("openai-completions", provider, base);
            d.api_key = key;
            d.filter_model = Some(Arc::new(|_, m| {
                Ok(is_likely_siliconflow_chat_model_id(&text(m, "id").unwrap_or_else(|| "".into())))
            }));
            d.map_model = Some(Arc::new(move |e, d, _| {
                let id = text(&d, "id").unwrap_or_else(|| "".into());
                if let Some(r) = refs.get(&id) {
                    return Ok(Some(Arc::new(map_with_bundled_reference(e, &d, Some(r)))));
                }
                let Some(canonical) = wire::resolve_model_reference(&id, wire::bundled_model_reference_index()) else {
                    return Ok(Some(d));
                };
                let mut model = (*d).clone();
                model.set(
                    "name",
                    wire::str_value(name(
                        e.get("name"),
                        &text(&canonical, "name").or_else(|| text(&d, "name")).unwrap_or_else(|| "".into()),
                    )),
                );
                for k in ["reasoning", "input"] {
                    copy_field(&mut model, k, &canonical, k);
                }
                let window = canonical
                    .get("contextWindow")
                    .filter(|v| !matches!(v, WireValue::Null))
                    .or_else(|| d.get("contextWindow"))
                    .cloned()
                    .unwrap_or(WireValue::Null);
                let cap = canonical
                    .get("maxTokens")
                    .filter(|v| !matches!(v, WireValue::Null))
                    .or_else(|| d.get("maxTokens"))
                    .cloned()
                    .unwrap_or(WireValue::Null);
                model.set(
                    "maxTokens",
                    match (cap.as_number(), window.as_number()) {
                        (Some(c), Some(w)) => WireValue::Number(c.min(w)),
                        _ => cap,
                    },
                );
                model.set("contextWindow", window);
                Ok(Some(Arc::new(model)))
            }));
            RawModelValue::from_discovery(fetch_openai_compatible_models(&context, &d).await)
        }
    }));
    options
}
pub fn siliconflow_model_manager_options(
    ctx: &CatalogContext,
    config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    siliconflow_options(ctx, config, host, "siliconflow", "https://api.siliconflow.com/v1")
}
pub fn siliconflow_cn_model_manager_options(
    ctx: &CatalogContext,
    config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    siliconflow_options(ctx, config, host, "siliconflow-cn", "https://api.siliconflow.cn/v1")
}
pub fn zhipu_coding_plan_model_manager_options(
    ctx: &CatalogContext,
    config: ModelManagerConfig,
) -> ModelManagerOptions {
    let mapper = Arc::new(|_: &VariantSpec, d: &VariantSpec, _: Option<&VariantSpec>| {
        let id = text(d, "id").unwrap_or_else(|| "".into());
        let identity = classify_wire(&"zhipu-coding-plan".into(), &id, true)?;
        let revision =
            text(&identity, "revision").and_then(|r| r.to_utf8().ok()).and_then(|r| catalog_rules::parse_revision(&r));
        let family = text(&identity, "family");
        let floor = catalog_rules::parse_revision(if family.as_ref().is_some_and(|f| f.equals_ascii("flash")) {
            "5.3"
        } else {
            "4.5"
        });
        let glm = text(&identity, "class").is_some_and(|c| c.equals_ascii("glm"))
            && family.as_ref().is_none_or(|f| ["air", "turbo", "flash"].iter().any(|s| f.equals_ascii(s)))
            && revision.zip(floor).is_some_and(|(r, f)| catalog_rules::compare_revision(r, f) >= 0);
        let mut m = d.clone();
        m.set("reasoning", WireValue::Bool(glm || wire::boolean(&identity, "thinkingVariant") == Some(true)));
        m.set("input", input(None));
        m.set(
            "compat",
            WireValue::object(vec![
                ("thinkingFormat", wire::str_value("zai")),
                ("reasoningContentField", wire::str_value("reasoning_content")),
                ("supportsDeveloperRole", WireValue::Bool(false)),
            ]),
        );
        Ok(Some(m))
    });
    let mut b =
        build("openai-completions", "zhipu-coding-plan", "https://open.bigmodel.cn/api/coding/paas/v4", config, mapper);
    b.authoritative = true;
    compatible_options(ctx, b)
}
