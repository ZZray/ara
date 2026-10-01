//! Fixed OMP model factories: public/simple catalogs, static lineages and native caps.
use super::super::{behavior::model_limits_for, static_data::fixed_models};
use super::*;
use crate::{catalog_discovery::zero_cost, catalog_rules, js_regex::JsRegExp, model_wire_policy::classify_wire};
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
fn normalized_v1(value: Option<&WireString>, fallback: &str) -> WireString {
    let mut value =
        value.map(crate::catalog_discovery::js_trim).filter(|v| !v.is_empty()).unwrap_or_else(|| fallback.into());
    while value.units().last() == Some(&47) {
        value = value.slice_prefix(value.len() - 1);
    }
    if ends(&value, "/v1") { value } else { append(&value, "/v1") }
}
fn seed_reference(seeds: &[SpecRef], id: &WireString) -> Option<SpecRef> {
    seeds.iter().find(|s| text(s, "id").as_ref() == Some(id)).cloned()
}
fn ids(seeds: &[SpecRef]) -> Vec<WireString> {
    seeds.iter().map(|s| text(s, "id").expect("seed id")).collect()
}
pub fn together_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    create_simple_openai_completions_options(ctx, "together", "https://api.together.xyz/v1", config)
}
pub fn qwen_portal_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    create_simple_openai_completions_options(ctx, "qwen-portal", "https://portal.qwen.ai/v1", config)
}
pub fn qianfan_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    create_simple_openai_completions_options(ctx, "qianfan", "https://qianfan.baidubce.com/v2", config)
}
pub fn mistral_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    create_simple_openai_completions_options(ctx, "mistral", "https://api.mistral.ai/v1", config)
}
pub fn coreweave_model_manager_options(
    default: &CatalogContext,
    config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    let context = context(default, &config);
    let mut options = ModelManagerOptions::new("coreweave");
    if config.api_key.as_ref().is_none_or(WireString::is_empty) {
        return options;
    }
    let environment = host.environment.clone();
    options.dynamic_fetcher = Some(dynamic(move || {
        let context = context.clone();
        let config = config.clone();
        let environment = environment.clone();
        async move {
            let clean = |key: &str| {
                environment.get(key).map(|s| crate::catalog_discovery::js_trim(&s)).filter(|s| !s.is_empty())
            };
            let project = clean("COREWEAVE_PROJECT").or_else(|| clean("WANDB_INFERENCE_PROJECT")).or_else(|| {
                let project = clean("WANDB_PROJECT")?;
                if includes(&project, "/") {
                    Some(project)
                } else {
                    clean("WANDB_ENTITY")
                        .map(|entity| super::super::cache_provider_id::join(&append(&entity, "/"), &project))
                }
            });
            let mut discovery = OpenAiCompatibleOptions::new(
                "openai-completions",
                "coreweave",
                config.base_url.unwrap_or_else(|| "https://api.inference.wandb.ai/v1".into()),
            );
            discovery.api_key = config.api_key;
            if let Some(project) = project {
                discovery.headers.push(("OpenAI-Project".into(), project));
            }
            let refs = wire::create_bundled_reference_map(&"coreweave".into());
            discovery.map_model = Some(Arc::new(move |entry, defaults, _| {
                Ok(Some(Arc::new(map_with_bundled_reference(
                    entry,
                    &defaults,
                    refs.get(&text(&defaults, "id").unwrap_or_else(|| "".into())).map(|s| s.as_ref()),
                ))))
            }));
            RawModelValue::from_discovery(fetch_openai_compatible_models(&context, &discovery).await)
        }
    }));
    options
}
pub fn meta_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let seeds = Arc::new(fixed_models("META_MUSE_STATIC_MODELS"));
    let map_seeds = seeds.clone();
    let mapper = Arc::new(move |entry: &VariantSpec, defaults: &VariantSpec, reference: Option<&VariantSpec>| {
        let id = text(defaults, "id").unwrap_or_else(|| "".into());
        let seeded = seed_reference(&map_seeds, &id);
        let lineage = if reference.is_none() && seeded.is_none() { super::catalog_f::muse_lineage(&id)? } else { None };
        Ok(Some(map_with_bundled_reference(entry, defaults, reference.or(seeded.as_deref()).or(lineage.as_ref()))))
    });
    let mut b = build("openai-responses", "meta", "https://api.meta.ai/v1", config, mapper);
    b.filter = Some(Arc::new(|_, model, _| {
        Ok(!is_excluded_model(&"meta".into(), &text(model, "id").unwrap_or_else(|| "".into())))
    }));
    let mut options = compatible_options(ctx, b);
    options.static_models = Some(RawModelValue::models((*seeds).clone()));
    options
}
pub fn bedrock_mantle_model_manager_options(
    default: &CatalogContext,
    config: ModelManagerConfig,
) -> ModelManagerOptions {
    let seeds = Arc::new(fixed_models("BEDROCK_MANTLE_STATIC_MODELS"));
    let mut options = ModelManagerOptions::new("bedrock-mantle");
    options.static_models = Some(RawModelValue::models((*seeds).clone()));
    options.dynamic_models_authoritative = true;
    if !config.authenticated {
        return options;
    }
    let context = context(default, &config);
    let inference = config.base_url.unwrap_or_else(|| "https://bedrock-mantle.{region}.api.aws/openai/v1".into());
    let trimmed = trim_one_slash(&inference);
    let discovery =
        if ends(&trimmed, "/openai/v1") { append(&trimmed.slice_prefix(trimmed.len() - 10), "/v1") } else { inference };
    options.dynamic_fetcher = Some(dynamic(move || {
        let context = context.clone();
        let seeds = seeds.clone();
        let discovery = discovery.clone();
        async move {
            let mut d = OpenAiCompatibleOptions::new("openai-responses", "bedrock-mantle", discovery);
            d.map_model = Some(Arc::new(move |entry, defaults, _| {
                let mut defaults = (*defaults).clone();
                defaults.set("baseUrl", wire::str_value("https://bedrock-mantle.{region}.api.aws/openai/v1"));
                Ok(Some(Arc::new(map_with_bundled_reference(
                    entry,
                    &defaults,
                    seed_reference(&seeds, &text(&defaults, "id").unwrap_or_else(|| "".into())).as_deref(),
                ))))
            }));
            RawModelValue::from_discovery(fetch_openai_compatible_models(&context, &d).await)
        }
    }));
    options
}
pub fn moonshot_model_manager_options(
    ctx: &CatalogContext,
    mut config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    if config.base_url.is_none() {
        config.base_url = host.environment.get("MOONSHOT_BASE_URL");
    }
    let mapper = Arc::new(|entry: &VariantSpec, defaults: &VariantSpec, reference: Option<&VariantSpec>| {
        let mut model = map_with_bundled_reference(entry, defaults, reference);
        let id = lower(&text(&model, "id").unwrap_or_else(|| "".into()));
        let identity = classify_wire(&"moonshot".into(), &id, true)?;
        if reference.is_none()
            && text(&identity, "class").is_some_and(|c| c.equals_ascii("kimi"))
            && text(&identity, "family").is_some_and(|f| f.equals_ascii("k3"))
        {
            model.set("reasoning", WireValue::Bool(true));
            if model.record("cost").is_some_and(|c| {
                ["input", "output", "cacheRead"].iter().all(|k| c.get(k).and_then(WireValue::as_number) == Some(0.0))
            }) {
                model.set(
                    "cost",
                    WireValue::object(vec![
                        ("input", WireValue::Number(3.0)),
                        ("output", WireValue::Number(15.0)),
                        ("cacheRead", WireValue::Number(0.3)),
                        ("cacheWrite", WireValue::Number(0.0)),
                    ]),
                );
            }
            let limits = model_limits_for(&"moonshot".into(), &id)
                .or_else(|| model_limits_for(&"moonshot".into(), &"kimi-k3".into()));
            for (target, source) in [("contextWindow", "context"), ("maxTokens", "maxTokens")] {
                if model.get(target).is_none_or(|v| matches!(v, WireValue::Null)) {
                    model.set(target, limits.as_ref().and_then(|l| l.get(source)).cloned().unwrap_or(WireValue::Null));
                }
            }
        } else {
            model.set(
                "reasoning",
                WireValue::Bool(
                    wire::boolean(&identity, "thinkingVariant") == Some(true)
                        || text(&identity, "class").is_some_and(|c| c.equals_ascii("kimi"))
                            && text(&identity, "family").is_some_and(|f| starts(&f, "k2"))
                        || wire::boolean(&model, "reasoning") == Some(true),
                ),
            );
        }
        Ok(Some(model))
    });
    compatible_options(ctx, build("openai-completions", "moonshot", "https://api.moonshot.ai/v1", config, mapper))
}
pub fn sakana_model_manager_options(
    ctx: &CatalogContext,
    mut config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    let base = config
        .base_url
        .clone()
        .or_else(|| host.environment.get("SAKANA_BASE_URL"))
        .or_else(|| host.environment.get("FUGU_BASE_URL"));
    config.base_url = Some(normalized_v1(base.as_ref(), "https://api.sakana.ai/v1"));
    let seeds = Arc::new(fixed_models("SAKANA_FUGU_STATIC_MODELS"));
    let map_seeds = seeds.clone();
    let mapper = Arc::new(move |entry: &VariantSpec, defaults: &VariantSpec, reference: Option<&VariantSpec>| {
        let id = text(defaults, "id").unwrap_or_else(|| "".into());
        let seed = seed_reference(&map_seeds, &id);
        let reference = reference.or(seed.as_deref());
        let mut model = map_with_bundled_reference(entry, defaults, reference);
        let normalized = lower(&id);
        if reference.is_none() && (normalized.equals_ascii("fugu") || starts(&normalized, "fugu-")) {
            model.set("reasoning", WireValue::Bool(true));
            model.set_record("thinking", &thinking("effort", &["high", "max"]));
            model.set(
                "compat",
                WireValue::object(vec![
                    ("includeEncryptedReasoning", WireValue::Bool(false)),
                    ("streamIdleTimeoutMs", WireValue::Number(0.0)),
                ]),
            );
        }
        Ok(Some(model))
    });
    let mut b = build("openai-responses", "sakana", "https://api.sakana.ai/v1", config, mapper);
    b.authoritative = true;
    b.drop_ids = Some(ids(&seeds));
    compatible_options(ctx, b)
}
pub fn aiand_model_manager_options(
    ctx: &CatalogContext,
    mut config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    let base = config.base_url.clone().or_else(|| host.environment.get("AIAND_BASE_URL"));
    config.base_url = Some(normalized_v1(base.as_ref(), "https://api.aiand.com/v1"));
    let mapper = Arc::new(|entry: &VariantSpec, defaults: &VariantSpec, _: Option<&VariantSpec>| {
        let mut model = defaults.clone();
        let capabilities = wire::list(entry, "capabilities");
        let reasoning = capabilities.iter().any(|c| c.equals_ascii("reasoning"));
        let description = text(entry, "description").filter(|s| !crate::catalog_discovery::js_trim(s).is_empty());
        model.set(
            "name",
            wire::str_value(
                description
                    .unwrap_or_else(|| name(entry.get("name"), &text(defaults, "name").unwrap_or_else(|| "".into()))),
            ),
        );
        model.set("reasoning", WireValue::Bool(reasoning));
        model.set(
            "input",
            if capabilities.iter().any(|c| c.equals_ascii("vision")) {
                input(Some(&WireValue::Array(vec![wire::str_value("image")])))
            } else {
                input(None)
            },
        );
        let cost = if text(entry, "currency").is_some_and(|s| !s.equals_ascii("usd")) {
            zero_cost()
        } else {
            WireValue::object(vec![
                ("input", positive(entry.get("input_per_1m"), Some(&WireValue::Number(0.0)))),
                ("output", positive(entry.get("output_per_1m"), Some(&WireValue::Number(0.0)))),
                ("cacheRead", WireValue::Number(0.0)),
                ("cacheWrite", WireValue::Number(0.0)),
            ])
        };
        model.set("cost", cost);
        model.set("contextWindow", positive(entry.get("context_window"), None));
        if reasoning {
            let efforts: Vec<_> = wire::list(entry, "reasoning_efforts")
                .into_iter()
                .filter(|e| ["minimal", "low", "medium", "high", "xhigh", "max"].iter().any(|s| e.equals_ascii(s)))
                .collect();
            if !efforts.is_empty() {
                let mut t = wire::empty();
                t.set("mode", wire::str_value("effort"));
                t.set("efforts", WireValue::Array(efforts.iter().cloned().map(WireValue::String).collect()));
                if let Some(default) = text(entry, "reasoning_effort_default").filter(|e| efforts.contains(e)) {
                    t.set("defaultLevel", wire::str_value(default));
                }
                model.set_record("thinking", &t);
            }
        }
        Ok(Some(model))
    });
    let mut b = build("openai-completions", "aiand", "https://api.aiand.com/v1", config, mapper);
    b.authoritative = true;
    b.drop_ids = Some(ids(&fixed_models("AIAND_STATIC_MODELS")));
    compatible_options(ctx, b)
}
pub fn abliteration_model_manager_options(ctx: &CatalogContext, mut config: ModelManagerConfig) -> ModelManagerOptions {
    config.base_url = Some(normalized_v1(config.base_url.as_ref(), "https://api.abliteration.ai/v1"));
    let seeds = Arc::new(fixed_models("ABLITERATION_STATIC_MODELS"));
    let map_seeds = seeds.clone();
    let mapper = Arc::new(move |entry: &VariantSpec, defaults: &VariantSpec, reference: Option<&VariantSpec>| {
        let seed = seed_reference(&map_seeds, &text(defaults, "id").unwrap_or_else(|| "".into()));
        let reference = reference.or(seed.as_deref());
        let mut model = map_with_bundled_reference(entry, defaults, reference);
        if reference.is_none() {
            model.set("reasoning", WireValue::Bool(true));
        }
        Ok(Some(model))
    });
    let mut b = build("openai-responses", "abliteration", "https://api.abliteration.ai/v1", config, mapper);
    b.authoritative = true;
    b.drop_ids = Some(ids(&seeds));
    compatible_options(ctx, b)
}
pub fn yolo_auto_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let mut references = wire::create_bundled_reference_map(&"yolo-auto".into());
    for seed in fixed_models("YOLO_AUTO_STATIC_MODELS") {
        let id = text(&seed, "id").expect("seed id");
        let previous = references.get(&id);
        let mut model = previous.map_or_else(wire::empty, |s| (**s).clone());
        model = wire::spread(&[&model, &seed]);
        model.set(
            "maxTokens",
            seed.get("maxTokens")
                .filter(|v| !matches!(v, WireValue::Null))
                .or_else(|| previous.and_then(|s| s.get("maxTokens")))
                .cloned()
                .unwrap_or(WireValue::Null),
        );
        if let Some(t) = seed
            .record("thinking")
            .filter(|v| !matches!(v.value, WireValue::Null))
            .or_else(|| previous.and_then(|s| s.record("thinking")))
        {
            model.set_record("thinking", &t);
        } else {
            model.set_undefined("thinking");
        }
        references.insert(id, Arc::new(model));
    }
    let resolver = wire::ReferenceResolver::lazy(move || references.clone());
    let mapper = Arc::new(move |entry: &VariantSpec, defaults: &VariantSpec, reference: Option<&VariantSpec>| {
        let resolved = resolver.resolve(&text(defaults, "id").unwrap_or_else(|| "".into()));
        let mut model = map_with_bundled_reference(entry, defaults, resolved.as_deref().or(reference));
        model.set("cost", zero_cost());
        let mut compat = model.record("compat").unwrap_or_else(wire::empty);
        compat.set("supportsStore", WireValue::Bool(false));
        compat.set("supportsDeveloperRole", WireValue::Bool(false));
        model.set_record("compat", &compat);
        Ok(Some(model))
    });
    let mut b = build("openai-completions", "yolo-auto", "https://yolo-auto.com/v1", config, mapper);
    b.authoritative = true;
    compatible_options(ctx, b)
}
pub fn venice_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let mapper = Arc::new(|e: &VariantSpec, d: &VariantSpec, r: Option<&VariantSpec>| {
        let mut model = map_with_bundled_reference(e, d, r);
        model.set(
            "maxTokens",
            clamp_kimi_k27_code_max_tokens(
                &text(d, "id").unwrap_or_else(|| "".into()),
                model.get("maxTokens").cloned().unwrap_or(WireValue::Null),
            )?,
        );
        let mut compat = model.record("compat").unwrap_or_else(wire::empty);
        compat.set("supportsUsageInStreaming", WireValue::Bool(false));
        model.set_record("compat", &compat);
        Ok(Some(model))
    });
    let mut b = build("openai-completions", "venice", "https://api.venice.ai/api/v1", config, mapper);
    b.require_api_key = false;
    compatible_options(ctx, b)
}
pub fn vercel_ai_gateway_model_manager_options(
    ctx: &CatalogContext,
    mut config: ModelManagerConfig,
) -> ModelManagerOptions {
    let mut base = config
        .base_url
        .as_ref()
        .map(crate::catalog_discovery::js_trim)
        .unwrap_or_else(|| "https://ai-gateway.vercel.sh".into());
    while base.units().last() == Some(&47) {
        base = base.slice_prefix(base.len() - 1);
    }
    let catalog = if base.is_empty() || ends(&base, "/v1") { base.clone() } else { append(&base, "/v1") };
    let inference = if ends(&base, "/v1") { base.slice_prefix(base.len() - 3) } else { base };
    config.base_url = Some(catalog);
    let mapper = Arc::new(move |e: &VariantSpec, d: &VariantSpec, _: Option<&VariantSpec>| {
        let mut m = d.clone();
        m.set("baseUrl", wire::str_value(inference.clone()));
        m.set("reasoning", WireValue::Bool(wire::has_string(e, "tags", "reasoning")));
        m.set(
            "input",
            if wire::has_string(e, "tags", "vision") {
                input(Some(&WireValue::Array(vec![wire::str_value("image")])))
            } else {
                input(None)
            },
        );
        let pricing = e.record("pricing");
        m.set(
            "cost",
            WireValue::object(
                [
                    ("input", "input"),
                    ("output", "output"),
                    ("cacheRead", "input_cache_read"),
                    ("cacheWrite", "input_cache_write"),
                ]
                .iter()
                .map(|(t, s)| {
                    (
                        *t,
                        WireValue::Number(
                            to_number(pricing.as_ref().and_then(|p| p.get(s))).unwrap_or(0.0) * 1000000.0,
                        ),
                    )
                })
                .collect(),
            ),
        );
        for (t, s) in [("contextWindow", "context_window"), ("maxTokens", "max_tokens")] {
            if e.get(s).and_then(WireValue::as_number).is_some() {
                copy_field(&mut m, t, e, s);
            }
        }
        if text(e, "id").or_else(|| text(d, "id")).is_some_and(|id| id.equals_ascii("meta/muse-spark-1.2-contributor"))
            && let Some(n) = m.get("maxTokens").and_then(WireValue::as_number)
        {
            m.set("maxTokens", WireValue::Number(n.min(131072.0)));
        }
        Ok(Some(m))
    });
    let mut b = build("anthropic-messages", "vercel-ai-gateway", "https://ai-gateway.vercel.sh/v1", config, mapper);
    b.require_api_key = false;
    b.filter = Some(Arc::new(|e, _, _| Ok(wire::has_string(e, "tags", "tool-use"))));
    compatible_options(ctx, b)
}
pub const KIMI_CODE_DEFAULT_MAX_TOKENS: f64 = 32000.0;
/// None is an omitted argument; Some(Null) preserves an explicitly null fallback.
pub fn kimi_code_max_tokens(id: &WireString, fallback: Option<WireValue>) -> WireValue {
    model_limits_for(&"kimi-code".into(), &lower(&crate::catalog_discovery::js_trim(id)))
        .and_then(|m| m.get("maxTokens").filter(|v| !matches!(v, WireValue::Null)).cloned())
        .unwrap_or_else(|| fallback.unwrap_or(WireValue::Number(KIMI_CODE_DEFAULT_MAX_TOKENS)))
}
pub fn kimi_code_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let mapper = Arc::new(|e: &VariantSpec, d: &VariantSpec, _: Option<&VariantSpec>| {
        let id = text(d, "id").unwrap_or_else(|| "".into());
        let supports = text(e, "supports_thinking_type");
        let reasoning = if supports.as_ref().is_some_and(|s| s.equals_ascii("only") || s.equals_ascii("both")) {
            true
        } else if supports.as_ref().is_some_and(|s| s.equals_ascii("no")) {
            false
        } else {
            wire::boolean(e, "supports_reasoning") == Some(true)
                || wire::boolean(&classify_wire(&"kimi-code".into(), &id, true)?, "thinkingVariant") == Some(true)
        };
        let mut t = None;
        if reasoning && let Some(raw) = e.record("think_efforts").filter(|r| wire::boolean(r, "support") == Some(true))
        {
            let efforts: Vec<_> = ["minimal", "low", "medium", "high", "xhigh", "max"]
                .iter()
                .copied()
                .filter(|s| wire::has_string(&raw, "valid_efforts", s))
                .collect();
            if !efforts.is_empty() {
                let mut thinking = thinking("effort", &efforts);
                if supports.as_ref().is_some_and(|s| s.equals_ascii("only")) {
                    thinking.set("requiresEffort", WireValue::Bool(true));
                }
                if let Some(default) =
                    text(&raw, "default_effort").filter(|e| efforts.iter().any(|s| e.equals_ascii(s)))
                {
                    thinking.set("defaultLevel", wire::str_value(default));
                }
                t = Some(thinking);
            }
        }
        let mut m = d.clone();
        if text(e, "display_name").is_some() {
            copy_field(&mut m, "name", e, "display_name");
        }
        m.set("reasoning", WireValue::Bool(reasoning));
        m.set(
            "input",
            if wire::boolean(e, "supports_image_in") == Some(true) {
                input(Some(&WireValue::Array(vec![wire::str_value("image")])))
            } else {
                input(None)
            },
        );
        m.set(
            "contextWindow",
            e.get("context_length").filter(|v| v.as_number().is_some()).cloned().unwrap_or(WireValue::Number(262144.0)),
        );
        m.set("maxTokens", kimi_code_max_tokens(&id, None));
        let mut compat = wire::empty();
        compat.set("thinkingFormat", wire::str_value(if t.is_some() { "kimi" } else { "zai" }));
        if text(e, "protocol").is_some_and(|s| s.equals_ascii("anthropic")) {
            compat.set("kimiApiFormat", wire::str_value("anthropic"));
        } else if matches!(e.get("protocol"), Some(WireValue::Null)) {
            compat.set("kimiApiFormat", wire::str_value("openai"));
        } else {
            compat.set_undefined("kimiApiFormat");
        }
        compat.set("reasoningContentField", wire::str_value("reasoning_content"));
        compat.set("supportsDeveloperRole", WireValue::Bool(false));
        if let Some(t) = t {
            m.set_record("thinking", &t);
        } else {
            m.set_undefined("thinking");
        }
        m.set_record("compat", &compat);
        Ok(Some(m))
    });
    let mut b = build("openai-completions", "kimi-code", "https://api.kimi.com/coding/v1", config, mapper);
    b.headers = vec![("User-Agent".into(), "KimiCLI/1.0".into()), ("X-Msh-Platform".into(), "kimi_cli".into())];
    compatible_options(ctx, b)
}
pub fn vllm_model_manager_options(
    ctx: &CatalogContext,
    config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    let base = config.base_url.clone().unwrap_or_else(|| "http://127.0.0.1:8000/v1".into());
    let key = super::super::cache_provider_id::resolve_model_cache_provider_id(
        &"vllm".into(),
        None,
        Some(&base),
        host.environment.as_ref(),
    );
    let mapper = Arc::new(|e: &VariantSpec, d: &VariantSpec, r: Option<&VariantSpec>| {
        let mut m = map_with_bundled_reference(e, d, r);
        m.set("contextWindow", positive(e.get("max_model_len"), m.get("contextWindow")));
        let identity = classify_wire(&"vllm".into(), &text(&m, "id").unwrap_or_else(|| "".into()), true)?;
        let revision =
            text(&identity, "revision").and_then(|r| r.to_utf8().ok()).and_then(|r| catalog_rules::parse_revision(&r));
        let floor = catalog_rules::parse_revision("3.8");
        if text(&identity, "class").is_some_and(|c| c.equals_ascii("qwen"))
            && revision.zip(floor).is_some_and(|(r, f)| catalog_rules::compare_revision(r, f) >= 0)
        {
            m.set("reasoning", WireValue::Bool(true));
        }
        Ok(Some(m))
    });
    let mut b = build("openai-completions", "vllm", "http://127.0.0.1:8000/v1", config, mapper);
    b.require_api_key = false;
    let mut options = compatible_options(ctx, b);
    options.cache_provider_id = Some(key);
    options
}
pub fn nano_gpt_model_manager_options(default: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let mut options = ModelManagerOptions::new("nanogpt");
    if config.api_key.as_ref().is_none_or(WireString::is_empty) {
        return options;
    }
    let context = context(default, &config);
    let resolver = Arc::new(wire::ReferenceResolver::lazy(|| wire::create_bundled_reference_map(&"nanogpt".into())));
    options.dynamic_fetcher = Some(dynamic(move || {
        let context = context.clone();
        let config = config.clone();
        let resolver = resolver.clone();
        async move {
            let bases = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
            let mut d = OpenAiCompatibleOptions::new(
                "openai-completions",
                "nanogpt",
                config.base_url.unwrap_or_else(|| "https://nano-gpt.com/api/v1".into()),
            );
            d.api_key = config.api_key;
            d.map_model = Some(Arc::new(move |e, defaults, _| {
                let r = resolver.resolve(&text(&defaults, "id").unwrap_or_else(|| "".into()));
                let mut m = map_with_bundled_reference(e, &defaults, r.as_deref());
                m.set("api", wire::str_value("openai-completions"));
                m.set("provider", wire::str_value("nanogpt"));
                Ok(Some(Arc::new(m)))
            }));
            let tracked = bases.clone();
            d.filter_model = Some(Arc::new(move |_, m| {
                let id = text(m, "id").unwrap_or_else(|| "".into());
                let mut regex = JsRegExp::new(":thinking(:[^:]+)?$".into(), "").expect("fixed regex");
                if let Some(matched) = regex.exec(&id) {
                    tracked.lock().expect("thinking IDs").insert(id.slice_prefix(matched.index));
                    return Ok(false);
                }
                let normalized = lower(&crate::catalog_discovery::js_trim(&id));
                Ok(!normalized.is_empty()
                    && regex.exec(&normalized).is_none()
                    && !is_excluded_model(&"nanogpt".into(), &normalized))
            }));
            let models = fetch_openai_compatible_models(&context, &d).await?;
            let Some(models) = models else {
                return Ok(RawModelValue::null());
            };
            let bases = bases.lock().expect("thinking IDs");
            Ok(RawModelValue::models(
                models
                    .into_iter()
                    .map(|m| {
                        if wire::boolean(&m, "reasoning") != Some(true)
                            && bases.contains(&text(&m, "id").unwrap_or_else(|| "".into()))
                        {
                            let mut out = (*m).clone();
                            out.set("reasoning", WireValue::Bool(true));
                            Arc::new(out)
                        } else {
                            m
                        }
                    })
                    .collect(),
            ))
        }
    }));
    options
}
