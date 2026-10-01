//! Fixed OMP gateway mapper formats, subscription credentials, Anthropic discovery.
pub const ALIBABA_TOKEN_PLAN_BASE_URL: &str = "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1";
use super::super::{
    behavior::model_limits_for,
    catalog_session::{fetch_revalidated_with_timeout, fetch_well_known_models},
    static_data::fixed_models,
};
use super::*;
use crate::model_wire_policy::classify_wire;
use async_trait::async_trait;
fn build(api: &str, provider: &str, base: &str, config: ModelManagerConfig, map: ProviderMapper) -> CompatibleBuilder {
    CompatibleBuilder {
        api: api.into(),
        provider: provider.into(),
        default_base: base.into(),
        config,
        headers: Vec::new(),
        authoritative: false,
        require_api_key: false,
        drop_ids: None,
        map,
        filter: None,
    }
}
pub fn map_openrouter_thinking(entry: &VariantSpec) -> Option<VariantSpec> {
    let raw = entry.record("reasoning")?;
    let efforts: Vec<_> = ["minimal", "low", "medium", "high", "xhigh", "max"]
        .iter()
        .copied()
        .filter(|e| wire::has_string(&raw, "supported_efforts", e))
        .collect();
    if efforts.is_empty() {
        return None;
    }
    let mut config = thinking("effort", &efforts);
    if let Some(default) = text(&raw, "default_effort").filter(|e| efforts.iter().any(|s| e.equals_ascii(s))) {
        config.set("defaultLevel", wire::str_value(default));
    }
    if wire::boolean(&raw, "mandatory") == Some(true) {
        config.set("requiresEffort", WireValue::Bool(true));
    }
    Some(config)
}
pub fn openrouter_model_manager_options(
    ctx: &CatalogContext,
    config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    let mapper = Arc::new(|e: &VariantSpec, d: &VariantSpec, r: Option<&VariantSpec>| {
        let mut m = map_with_bundled_reference(e, d, r);
        let pricing = e.record("pricing");
        m.set("reasoning", WireValue::Bool(wire::has_string(e, "supported_parameters", "reasoning")));
        if let Some(t) = map_openrouter_thinking(e) {
            m.set_record("thinking", &t);
        }
        let modality = e.record("architecture").and_then(|a| text(&a, "modality")).unwrap_or_else(|| "".into());
        m.set(
            "input",
            if includes(&modality, "image") {
                input(Some(&WireValue::Array(vec![wire::str_value("image")])))
            } else {
                input(None)
            },
        );
        m.set(
            "cost",
            WireValue::object(
                [
                    ("input", "prompt"),
                    ("output", "completion"),
                    ("cacheRead", "input_cache_read"),
                    ("cacheWrite", "input_cache_write"),
                ]
                .iter()
                .map(|(target, source)| {
                    (*target, WireValue::Number(parse_float(pricing.as_ref().and_then(|p| p.get(source))) * 1000000.0))
                })
                .collect(),
            ),
        );
        if e.get("context_length").and_then(WireValue::as_number).is_some() {
            copy_field(&mut m, "contextWindow", e, "context_length");
        }
        if let Some(top) = e.record("top_provider")
            && top.get("max_completion_tokens").and_then(WireValue::as_number).is_some()
        {
            copy_field(&mut m, "maxTokens", &top, "max_completion_tokens");
        }
        if !wire::has_string(e, "supported_parameters", "tool_choice") {
            let mut compat = m.record("compat").unwrap_or_else(wire::empty);
            compat.set("supportsToolChoice", WireValue::Bool(false));
            m.set_record("compat", &compat);
        }
        Ok(Some(m))
    });
    let mut b = build("openrouter", "openrouter", "https://openrouter.ai/api/v1", config, mapper);
    b.filter = Some(Arc::new(|e, _, _| Ok(wire::has_string(e, "supported_parameters", "tools"))));
    let mut options = compatible_options(ctx, b);
    options.cache_provider_id = Some(super::super::cache_provider_id::resolve_model_cache_provider_id(
        &"openrouter".into(),
        None,
        None,
        host.environment.as_ref(),
    ));
    options
}
fn zen_price(pricing: Option<&VariantSpec>, key: &str) -> f64 {
    pricing
        .and_then(|p| p.get(key))
        .and_then(WireValue::as_array)
        .and_then(|items| {
            items
                .iter()
                .find_map(|item| if matches!(item, WireValue::Object(_)) { to_number(item.get("value")) } else { None })
        })
        .unwrap_or(0.0)
}
pub fn zenmux_model_manager_options(ctx: &CatalogContext, mut config: ModelManagerConfig) -> ModelManagerOptions {
    let mut openai = config
        .base_url
        .as_ref()
        .map(crate::catalog_discovery::js_trim)
        .filter(|s| !s.is_empty())
        .map(|s| trim_one_slash(&s))
        .unwrap_or_else(|| "https://zenmux.ai/api/v1".into());
    if ends(&openai, "/api/anthropic") {
        openai = replace_first(&openai, &"/api/anthropic".into(), &"/api/v1".into());
    }
    let anthropic: WireString = openai
        .to_utf8()
        .ok()
        .and_then(|s| reqwest::Url::parse(&s).ok())
        .map(|mut u| {
            let path = u.path().trim_end_matches('/');
            let path = path
                .strip_suffix("/api/v1")
                .map_or_else(|| "/api/anthropic".to_owned(), |prefix| format!("{prefix}/api/anthropic"));
            u.set_path(&path);
            format!(
                "{}://{}{}{}",
                u.scheme(),
                u.host().map(|h| h.to_string()).unwrap_or_default(),
                u.port().map(|p| format!(":{p}")).unwrap_or_default(),
                u.path()
            )
            .into()
        })
        .unwrap_or_else(|| "https://zenmux.ai/api/anthropic".into());
    config.base_url = Some(openai.clone());
    let mapper = Arc::new(move |e: &VariantSpec, d: &VariantSpec, _: Option<&VariantSpec>| {
        let is_anthropic = text(e, "owned_by").is_some_and(|s| lower(&s).equals_ascii("anthropic"))
            || text(&classify_wire(&"zenmux".into(), &text(d, "id").unwrap_or_else(|| "".into()), true)?, "class")
                .is_some_and(|s| s.equals_ascii("anthropic"));
        let pricing = e.record("pricings").filter(|r| matches!(r.value, WireValue::Object(_)));
        let caps = e.record("capabilities");
        let one = zen_price(pricing.as_ref(), "input_cache_write_1_h");
        let five = zen_price(pricing.as_ref(), "input_cache_write_5_min");
        let mut m = d.clone();
        m.set("name", wire::str_value(name(e.get("display_name"), &text(d, "name").unwrap_or_else(|| "".into()))));
        m.set("api", wire::str_value(if is_anthropic { "anthropic-messages" } else { "openai-completions" }));
        m.set("baseUrl", wire::str_value(if is_anthropic { anthropic.clone() } else { openai.clone() }));
        m.set(
            "reasoning",
            WireValue::Bool(
                caps.as_ref().is_some_and(|c| wire::boolean(c, "reasoning") == Some(true))
                    || wire::boolean(d, "reasoning") == Some(true),
            ),
        );
        m.set("input", input(e.get("input_modalities")));
        m.set(
            "cost",
            WireValue::object(vec![
                ("input", WireValue::Number(zen_price(pricing.as_ref(), "prompt"))),
                ("output", WireValue::Number(zen_price(pricing.as_ref(), "completion"))),
                ("cacheRead", WireValue::Number(zen_price(pricing.as_ref(), "input_cache_read"))),
                (
                    "cacheWrite",
                    WireValue::Number(if one > 0.0 {
                        one
                    } else if five > 0.0 {
                        five
                    } else {
                        zen_price(pricing.as_ref(), "input_cache_write")
                    }),
                ),
            ]),
        );
        m.set("contextWindow", positive(e.get("context_length"), d.get("contextWindow")));
        m.set("maxTokens", positive(e.get("max_completion_tokens"), d.get("maxTokens")));
        Ok(Some(m))
    });
    compatible_options(ctx, build("openai-completions", "zenmux", "https://zenmux.ai/api/v1", config, mapper))
}
pub fn kilo_model_manager_options(default: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let context = context(default, &config);
    let mut d = OpenAiCompatibleOptions::new(
        "openai-completions",
        "kilo",
        config.base_url.unwrap_or_else(|| "https://api.kilo.ai/api/gateway".into()),
    );
    d.api_key = config.api_key;
    let mut options = ModelManagerOptions::new("kilo");
    options.dynamic_fetcher = Some(dynamic(move || {
        let context = context.clone();
        let d = d.clone();
        async move { RawModelValue::from_discovery(fetch_openai_compatible_models(&context, &d).await) }
    }));
    options
}
pub fn alibaba_coding_plan_model_manager_options(
    ctx: &CatalogContext,
    config: ModelManagerConfig,
) -> ModelManagerOptions {
    compatible_options(
        ctx,
        build(
            "openai-completions",
            "alibaba-coding-plan",
            "https://coding-intl.dashscope.aliyuncs.com/v1",
            config,
            bundled_mapper(),
        ),
    )
}
pub fn parse_alibaba_token_plan_credential(value: &WireString) -> Option<VariantSpec> {
    let trimmed = crate::catalog_discovery::js_trim(value);
    if trimmed.is_empty() {
        return None;
    }
    let mut regex =
        crate::js_regex::JsRegExp::new(r"^sk-[A-Za-z0-9._~+/-]+={0,2}$".into(), "").expect("fixed token regex");
    if !starts(&trimmed, "{") {
        return regex
            .exec(&trimmed)
            .map(|_| VariantSpec::from_wire(WireValue::object(vec![("token", wire::str_value(trimmed))])));
    }
    let parsed = WireValue::parse(&trimmed.to_utf8().ok()?).ok()?;
    let parsed = VariantSpec::from_wire(parsed);
    let token =
        text(&parsed, "token").map(|s| crate::catalog_discovery::js_trim(&s)).filter(|s| regex.exec(s).is_some())?;
    for key in ["cookie", "baseUrl"] {
        if parsed.get(key).is_some_and(|v| !matches!(v, WireValue::String(_))) {
            return None;
        }
    }
    let mut out = wire::empty();
    out.set("token", wire::str_value(token));
    for key in ["cookie", "baseUrl"] {
        if let Some(s) = text(&parsed, key).map(|s| crate::catalog_discovery::js_trim(&s)).filter(|s| !s.is_empty()) {
            out.set(key, wire::str_value(s));
        }
    }
    Some(out)
}
pub fn alibaba_token_plan_model_manager_options(
    ctx: &CatalogContext,
    mut config: ModelManagerConfig,
) -> ModelManagerOptions {
    let credential = config.api_key.as_ref().and_then(parse_alibaba_token_plan_credential);
    config.api_key = credential.as_ref().and_then(|c| text(c, "token"));
    config.base_url = credential.as_ref().and_then(|c| text(c, "baseUrl")).or(config.base_url);
    let seeds = Arc::new(fixed_models("ALIBABA_TOKEN_PLAN_STATIC_MODELS"));
    let map_seeds = seeds.clone();
    let mapper = Arc::new(move |_: &VariantSpec, d: &VariantSpec, _: Option<&VariantSpec>| {
        let id = text(d, "id").unwrap_or_else(|| "".into());
        if let Some(r) = map_seeds.iter().find(|r| text(r, "id").as_ref() == Some(&id)) {
            let mut m = (**r).clone();
            for key in ["id", "api", "provider", "baseUrl"] {
                copy_field(&mut m, key, d, key);
            }
            return Ok(Some(m));
        }
        let normalized = lower(&crate::catalog_discovery::js_trim(&id));
        let mut m = d.clone();
        if let Some(limits) = model_limits_for(&"alibaba-token-plan".into(), &normalized) {
            for (target, source) in [("contextWindow", "context"), ("maxTokens", "maxTokens")] {
                if limits.get(source).is_some_and(|v| !matches!(v, WireValue::Null)) {
                    copy_field(&mut m, target, &limits, source);
                }
            }
        }
        let identity = classify_wire(&"alibaba-token-plan".into(), &normalized, true)?;
        if text(&identity, "class").is_some_and(|c| c.equals_ascii("deepseek"))
            && text(&identity, "family").is_some_and(|f| ["v4", "flash", "pro"].iter().any(|s| f.equals_ascii(s)))
        {
            m.set("reasoning", WireValue::Bool(true));
            m.set_record("thinking", &thinking("effort", &["high", "max"]));
        }
        Ok(Some(m))
    });
    let mut b = build(
        "openai-completions",
        "alibaba-token-plan",
        "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
        config,
        mapper,
    );
    b.require_api_key = true;
    b.authoritative = true;
    b.filter = Some(Arc::new(|_, m, _| {
        let id = lower(&crate::catalog_discovery::js_trim(&text(m, "id").unwrap_or_else(|| "".into())));
        Ok(!id.is_empty() && !is_excluded_model(&"alibaba-token-plan".into(), &id))
    }));
    let mut options = compatible_options(ctx, b);
    options.static_models = Some(RawModelValue::models((*seeds).clone()));
    options
}
#[derive(Clone, Default)]
pub struct XiaomiModelManagerConfig {
    pub common: ModelManagerConfig,
    pub provider_id: Option<WireString>,
    pub token_plan_region: Option<WireString>,
}
pub fn xiaomi_model_manager_options(default: &CatalogContext, config: XiaomiModelManagerConfig) -> ModelManagerOptions {
    let provider = config.provider_id.unwrap_or_else(|| "xiaomi".into());
    let mut options = ModelManagerOptions::new(provider.clone());
    let key = config.common.api_key;
    if key.as_ref().is_none_or(WireString::is_empty) {
        return options;
    }
    let context = config.common.context.unwrap_or_else(|| default.clone());
    let urls: Vec<WireString> = if let Some(region) = config.token_plan_region {
        vec![format!("https://token-plan-{}.xiaomimimo.com/v1", String::from_utf16_lossy(region.units())).into()]
    } else {
        ["sgp", "ams", "cn"].iter().map(|r| format!("https://token-plan-{r}.xiaomimimo.com/v1").into()).collect()
    };
    let token_plan =
        starts(&provider, "xiaomi-token-plan-") || urls.len() == 1 || key.as_ref().is_some_and(|k| starts(k, "tp-"));
    let urls = if token_plan {
        urls
    } else {
        vec![config.common.base_url.unwrap_or_else(|| "https://api.xiaomimimo.com/v1".into())]
    };
    let refs = Arc::new(wire::create_bundled_reference_map(&"xiaomi".into()));
    options.dynamic_fetcher = Some(dynamic(move || {
        let context = context.clone();
        let urls = urls.clone();
        let provider = provider.clone();
        let key = key.clone();
        let refs = refs.clone();
        async move {
            for url in urls {
                let mut d = OpenAiCompatibleOptions::new("openai-completions", provider.clone(), url);
                d.api_key = key.clone();
                let refs = refs.clone();
                d.map_model = Some(Arc::new(move |e, d, _| {
                    let mut m = map_with_bundled_reference(
                        e,
                        &d,
                        refs.get(&text(&d, "id").unwrap_or_else(|| "".into())).map(|s| s.as_ref()),
                    );
                    m.set("api", wire::str_value("openai-completions"));
                    copy_field(&mut m, "provider", &d, "provider");
                    copy_field(&mut m, "baseUrl", &d, "baseUrl");
                    m.set(
                        "name",
                        wire::str_value(name(e.get("display_name"), &text(&m, "name").unwrap_or_else(|| "".into()))),
                    );
                    Ok(Some(Arc::new(m)))
                }));
                let p = provider.clone();
                d.filter_model =
                    Some(Arc::new(move |_, m| Ok(!is_excluded_model(&p, &text(m, "id").unwrap_or_else(|| "".into())))));
                if let Some(models) = fetch_openai_compatible_models(&context, &d).await? {
                    return Ok(RawModelValue::models(models));
                }
            }
            Ok(RawModelValue::null())
        }
    }));
    options
}
const ANTHROPIC_BETA: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,redact-thinking-2026-02-12,context-management-2025-06-27,prompt-caching-scope-2026-01-05,mid-conversation-system-2026-04-07,advanced-tool-use-2025-11-20,effort-2025-11-24,extended-cache-ttl-2025-04-11";
pub fn anthropic_discovery_headers(key: &WireString) -> Vec<(WireString, WireString)> {
    let mut h = vec![
        ("anthropic-version".into(), "2023-06-01".into()),
        ("anthropic-dangerous-direct-browser-access".into(), "true".into()),
        ("anthropic-beta".into(), ANTHROPIC_BETA.into()),
    ];
    h.push(if includes(key, "sk-ant-oat") {
        ("Authorization".into(), super::super::cache_provider_id::join(&"Bearer ".into(), key))
    } else {
        ("x-api-key".into(), key.clone())
    });
    h
}
fn anthropic_base(value: Option<&WireString>, fallback: &str) -> WireString {
    value
        .map(crate::catalog_discovery::js_trim)
        .filter(|s| !s.is_empty())
        .map(|s| trim_one_slash(&s))
        .unwrap_or_else(|| fallback.into())
}
pub fn create_simple_anthropic_provider_options(
    default: &CatalogContext,
    provider: &str,
    base: &str,
    config: ModelManagerConfig,
) -> ModelManagerOptions {
    let mut options = ModelManagerOptions::new(provider);
    let Some(key) = config.api_key.as_ref().filter(|s| !s.is_empty()).cloned() else {
        return options;
    };
    let context = context(default, &config);
    let base = anthropic_base(config.base_url.as_ref(), base);
    let url = if ends(&base, "/v1") { base.clone() } else { append(&base, "/v1") };
    let refs = Arc::new(wire::create_bundled_reference_map(&provider.into()));
    let provider: WireString = provider.into();
    options.dynamic_fetcher = Some(dynamic(move || {
        let context = context.clone();
        let base = base.clone();
        let url = url.clone();
        let key = key.clone();
        let refs = refs.clone();
        let provider = provider.clone();
        async move {
            let mut d = OpenAiCompatibleOptions::new("anthropic-messages", provider, url);
            d.headers = anthropic_discovery_headers(&key);
            d.map_model = Some(Arc::new(move |e, d, _| {
                let mut m = map_with_bundled_reference(
                    e,
                    &d,
                    refs.get(&text(&d, "id").unwrap_or_else(|| "".into())).map(|s| s.as_ref()),
                );
                m.set(
                    "name",
                    wire::str_value(name(e.get("display_name"), &text(&m, "name").unwrap_or_else(|| "".into()))),
                );
                m.set("baseUrl", wire::str_value(base.clone()));
                Ok(Some(Arc::new(m)))
            }));
            RawModelValue::from_discovery(fetch_openai_compatible_models(&context, &d).await)
        }
    }));
    options
}
pub fn cloudflare_ai_gateway_model_manager_options(
    ctx: &CatalogContext,
    config: ModelManagerConfig,
) -> ModelManagerOptions {
    create_simple_anthropic_provider_options(
        ctx,
        "cloudflare-ai-gateway",
        "https://gateway.ai.cloudflare.com/v1/<account>/<gateway>/anthropic",
        config,
    )
}
fn map_anthropic_models_dev(
    payload: &VariantSpec,
    base: &WireString,
    context: &CatalogContext,
) -> Result<Vec<SpecRef>, DiscoveryError> {
    let desc = ModelsDevProviderDescriptor::new(
        "anthropic",
        "anthropic",
        "anthropic-messages",
        &base.to_utf8().unwrap_or_else(|_| String::from_utf16_lossy(base.units())),
    );
    let mut desc = desc;
    desc.base_url = base.clone();
    let mut models = map_models_dev_to_models(payload, &[desc])?;
    context.sort_by_id(&mut models);
    Ok(models)
}
struct AnthropicFallback {
    context: CatalogContext,
    host: ProviderFactoryHost,
    base: WireString,
    explicit: bool,
}
#[async_trait]
impl crate::model_manager::ModelsDevFallback for AnthropicFallback {
    async fn fetch(&self) -> Result<RawModelValue, DiscoveryError> {
        Ok(RawModelValue::Value(
            fetch_revalidated_with_timeout(&self.context, &self.host, self.explicit, 10000.0).await?,
        ))
    }
    fn map(&self, payload: RawModelValue, _: &WireString) -> Result<RawModelValue, DiscoveryError> {
        let models = if let RawModelValue::Value(payload) = payload {
            map_anthropic_models_dev(&payload, &self.base, &self.context)?
        } else {
            Vec::new()
        };
        Ok(RawModelValue::models(models))
    }
}
pub fn anthropic_model_manager_options(
    default: &CatalogContext,
    config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    let context = context(default, &config);
    let explicit = config.context.is_some();
    let host = host.clone();
    let base = anthropic_base(config.base_url.as_ref(), "https://api.anthropic.com");
    let mut options = ModelManagerOptions::new("anthropic");
    options.models_dev = Some(Arc::new(AnthropicFallback {
        context: context.clone(),
        host: host.clone(),
        base: base.clone(),
        explicit,
    }));
    let Some(key) = config.api_key.filter(|k| !k.is_empty()) else {
        return options;
    };
    let url = if ends(&base, "/v1") { base.clone() } else { append(&base, "/v1") };
    options.dynamic_fetcher = Some(dynamic(move || {
        let context = context.clone();
        let host = host.clone();
        let base = base.clone();
        let key = key.clone();
        let url = url.clone();
        async move {
            let models = fetch_well_known_models(&context, &host, explicit, None)
                .await
                .ok()
                .and_then(|p| map_anthropic_models_dev(&p, &base, &context).ok())
                .unwrap_or_default();
            let mut refs: HashMap<_, _> = models.into_iter().filter_map(|m| text(&m, "id").map(|id| (id, m))).collect();
            for model in wire::bundled_provider_models(&"anthropic".into())
                .into_iter()
                .filter(|m| text(m, "api").is_some_and(|a| a.equals_ascii("anthropic-messages")))
            {
                refs.insert(text(&model, "id").expect("bundle id"), Arc::new(wire::to_model_spec(&model)));
            }
            let mut d = OpenAiCompatibleOptions::new("anthropic-messages", "anthropic", url);
            d.headers = anthropic_discovery_headers(&key);
            d.map_model = Some(Arc::new(move |e, d, _| {
                let id = text(&d, "id").unwrap_or_else(|| "".into());
                let mut m = refs.get(&id).map_or_else(|| (*d).clone(), |r| (**r).clone());
                for k in ["id", "api", "provider"] {
                    copy_field(&mut m, k, &d, k);
                }
                m.set(
                    "name",
                    e.get("display_name")
                        .filter(|v| matches!(v, WireValue::String(_)))
                        .cloned()
                        .unwrap_or_else(|| d.get("name").cloned().unwrap_or(WireValue::Null)),
                );
                m.set("baseUrl", wire::str_value(base.clone()));
                Ok(Some(Arc::new(m)))
            }));
            RawModelValue::from_discovery(fetch_openai_compatible_models(&context, &d).await)
        }
    }));
    options
}
pub fn baseten_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let mapper = Arc::new(|e: &VariantSpec, d: &VariantSpec, r: Option<&VariantSpec>| {
        let id = text(d, "id").unwrap_or_else(|| "".into());
        let identity = classify_wire(&"baseten".into(), &id, true)?;
        let family = text(&identity, "family");
        let revision = text(&identity, "revision")
            .and_then(|s| s.to_utf8().ok())
            .and_then(|s| crate::catalog_rules::parse_revision(&s));
        let floor = crate::catalog_rules::parse_revision(if family.as_ref().is_some_and(|f| f.equals_ascii("flash")) {
            "5.3"
        } else {
            "5.2"
        });
        let glm = text(&identity, "class").is_some_and(|c| c.equals_ascii("glm"))
            && family.as_ref().is_none_or(|f| ["air", "turbo", "flash"].iter().any(|s| f.equals_ascii(s)))
            && revision.zip(floor).is_some_and(|(r, f)| crate::catalog_rules::compare_revision(r, f) >= 0);
        let supported = glm
            || id.equals_ascii("openai/gpt-oss-120b")
            || id.equals_ascii("deepseek-ai/DeepSeek-V4-Pro")
            || text(&identity, "class").is_some_and(|c| c.equals_ascii("kimi"))
                && family.is_some_and(|f| f.equals_ascii("k3"));
        let mut m = map_with_bundled_reference(e, d, r);
        m.set(
            "reasoning",
            WireValue::Bool(
                supported
                    && (wire::has_string(e, "supported_features", "reasoning")
                        || wire::has_string(e, "supported_features", "reasoning_effort")),
            ),
        );
        m.set(
            "input",
            if wire::has_string(e, "input_modalities", "image")
                || r.is_some_and(|r| wire::has_string(r, "input", "image"))
            {
                input(Some(&WireValue::Array(vec![wire::str_value("image")])))
            } else {
                input(None)
            },
        );
        let pricing = e.record("pricing");
        m.set(
            "cost",
            WireValue::object(
                [("input", "prompt"), ("output", "completion"), ("cacheRead", "input_cache_read")]
                    .iter()
                    .map(|(t, s)| {
                        (
                            *t,
                            WireValue::Number(
                                to_number(pricing.as_ref().and_then(|p| p.get(s))).filter(|n| *n > 0.0).unwrap_or(0.0)
                                    * 1000000.0,
                            ),
                        )
                    })
                    .chain(std::iter::once(("cacheWrite", WireValue::Number(0.0))))
                    .collect(),
            ),
        );
        for (target, source) in [("contextWindow", "context_length"), ("maxTokens", "max_completion_tokens")] {
            m.set(target, positive(e.get(source), r.and_then(|r| r.get(target)).or_else(|| d.get(target))));
        }
        if !wire::has_string(e, "supported_features", "tools") {
            m.set("supportsTools", WireValue::Bool(false));
        }
        Ok(Some(m))
    });
    let mut b = build("openai-completions", "baseten", "https://inference.baseten.co/v1", config, mapper);
    b.require_api_key = true;
    b.authoritative = true;
    b.drop_ids = Some(vec!["zai-org/GLM-5.3".into(), "zai-org/GLM-5.3-Flash".into()]);
    compatible_options(ctx, b)
}
