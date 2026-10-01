//! Fixed OMP GitHub Copilot key/endpoint resolution and opt-in context tiers.
use super::super::cache_provider_id::{join, resolve_model_cache_provider_id};
use super::*;
use crate::{catalog_discovery::DiscoveryRequest, catalog_rules, model_wire_policy::classify_wire};
pub const COPILOT_LONG_CONTEXT_ID_SUFFIX: &str = "-1m";
fn personal(base: &WireString) -> bool {
    base.equals_ascii("https://api.githubcopilot.com")
}
fn normalize_endpoint(value: &WireString) -> Option<WireString> {
    let value = crate::catalog_discovery::js_trim(value);
    if !starts(&value, "https://") {
        return None;
    }
    let url = reqwest::Url::parse(&String::from_utf16_lossy(value.units())).ok()?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return None;
    }
    Some(crate::catalog_discovery::trim_trailing_slashes(&value))
}
fn enterprise_domain(value: &WireString) -> Option<WireString> {
    let value = crate::catalog_discovery::js_trim(value);
    if value.is_empty() {
        return None;
    }
    let input = if includes(&value, "://") { value.clone() } else { join(&"https://".into(), &value) };
    let host = reqwest::Url::parse(&String::from_utf16_lossy(input.units()))
        .ok()
        .and_then(|u| u.host_str().map(WireString::from))
        .unwrap_or_else(|| lower(&value));
    if host.is_empty() || ["api.github.com", "github.com", "www.github.com"].iter().any(|s| host.equals_ascii(s)) {
        None
    } else {
        Some(host)
    }
}
fn enterprise_base(domain: &WireString) -> WireString {
    let Some(domain) = enterprise_domain(domain) else {
        return "https://api.githubcopilot.com".into();
    };
    join(
        &"https://".into(),
        &if starts(&domain, "copilot-api.") { domain } else { join(&"copilot-api.".into(), &domain) },
    )
}
struct Key {
    token: WireString,
    enterprise: Option<WireString>,
    endpoint: Option<WireString>,
}
fn parse_key(value: &WireString) -> Key {
    if let Ok(parsed) = WireValue::parse(&String::from_utf16_lossy(value.units())) {
        let parsed = VariantSpec::from_wire(parsed);
        if let Some(token) = text(&parsed, "token") {
            return Key {
                token,
                enterprise: text(&parsed, "enterpriseUrl").and_then(|s| enterprise_domain(&s)),
                endpoint: text(&parsed, "apiEndpoint").and_then(|s| normalize_endpoint(&s)),
            };
        }
    }
    Key { token: value.clone(), enterprise: None, endpoint: None }
}
fn merge_headers(headers: Option<&VariantSpec>) -> VariantSpec {
    let mut out = wire::empty();
    if let Some(headers) = headers {
        for key in headers.own_keys() {
            let lower = lower(&key);
            if [
                "user-agent",
                "editor-version",
                "copilot-integration-id",
                "copilot-harness-id",
                "openai-intent",
                "x-github-api-version",
                "x-initiator",
                "x-interaction-type",
            ]
            .iter()
            .any(|s| lower.equals_ascii(s))
            {
                continue;
            }
            if let Some(value) = headers.get_path(std::slice::from_ref(&key)) {
                out.set_key_record(&key, &VariantSpec::from_wire(value.clone()));
            }
        }
    }
    wire::spread(&[&out, &copilot_api_headers()])
}
fn record(entry: &VariantSpec, key: &str) -> Option<VariantSpec> {
    entry.record(key).filter(|r| matches!(r.value, WireValue::Object(_)))
}
fn tier_cost(tier: Option<&VariantSpec>, cache_write: Option<&WireValue>) -> Option<WireValue> {
    let tier = tier?;
    Some(WireValue::object(vec![
        ("input", WireValue::Number(to_number(tier.get("input_price"))? / 100.0)),
        ("output", WireValue::Number(to_number(tier.get("output_price"))? / 100.0)),
        ("cacheRead", WireValue::Number(to_number(tier.get("cache_price")).unwrap_or(0.0) / 100.0)),
        ("cacheWrite", cache_write.cloned().unwrap_or(WireValue::Null)),
    ]))
}
async fn discover_endpoint(ctx: &CatalogContext, key: &WireString) -> Option<WireString> {
    let response = timed_fetch(
        ctx,
        DiscoveryRequest {
            url: "https://api.github.com/copilot_internal/user".into(),
            headers: vec![
                ("Accept".into(), "application/json".into()),
                ("Authorization".into(), join(&"token ".into(), key)),
                ("User-Agent".into(), "copilot/1.0.82".into()),
            ],
            ..Default::default()
        },
        10000.0,
    )
    .await
    .ok()
    .filter(|r| r.ok())?;
    let payload = response.json().ok()?;
    record(&payload, "endpoints").and_then(|e| text(&e, "api")).and_then(|s| normalize_endpoint(&s))
}
pub fn github_copilot_model_manager_options(
    default: &CatalogContext,
    config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    let ctx = context(default, &config);
    let configured = config.base_url.unwrap_or_else(|| "https://api.githubcopilot.com".into());
    let parsed = config.api_key.as_ref().filter(|s| !s.is_empty()).map(parse_key);
    let base = if includes(&configured, "githubcopilot.com") {
        parsed
            .as_ref()
            .and_then(|k| k.endpoint.clone())
            .or_else(|| parsed.as_ref().and_then(|k| k.enterprise.as_ref()).map(enterprise_base))
            .unwrap_or(configured)
    } else {
        configured
    };
    let mut options = ModelManagerOptions::new("github-copilot");
    options.cache_provider_id = Some(resolve_model_cache_provider_id(
        &"github-copilot".into(),
        config.api_key.as_ref(),
        Some(&base),
        host.environment.as_ref(),
    ));
    options.drop_cached_model_ids_on_static_mismatch = Some(
        ["grok-4.5", "grok-4.5-1m", "grok-4.6", "grok-4.6-1m", "mai-code-1-flash-picker"]
            .iter()
            .map(|s| (*s).into())
            .collect(),
    );
    options.restorable_header_fallback = Some(copilot_api_headers());
    let Some(key) = parsed.map(|k| k.token).filter(|s| !s.is_empty()) else {
        return options;
    };
    let refs = Arc::new(wire::ReferenceResolver::lazy(|| wire::create_bundled_reference_map(&"github-copilot".into())));
    options.dynamic_fetcher = Some(dynamic(move || {
        let ctx = ctx.clone();
        let base = base.clone();
        let key = key.clone();
        let refs = refs.clone();
        async move {
            let base = if personal(&base) { discover_endpoint(&ctx, &key).await.unwrap_or(base) } else { base };
            let variants = Arc::new(std::sync::Mutex::new(Vec::<SpecRef>::new()));
            let mapped_variants = variants.clone();
            let mapped_base = base.clone();
            let mut d = OpenAiCompatibleOptions::new("openai-completions", "github-copilot", base.clone());
            d.api_key = Some(key);
            let headers = copilot_api_headers();
            d.headers = headers
                .value
                .entries()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_string()?.clone())))
                .collect();
            d.headers.push(("X-Initiator".into(), "user".into()));
            d.map_model = Some(Arc::new(move |entry, defaults, _| {
                let caps = record(entry, "capabilities");
                if caps.as_ref().and_then(|c| text(c, "type")).is_some_and(|s| !s.equals_ascii("chat")) {
                    return Ok(None);
                }
                let id = text(&defaults, "id").unwrap_or_else(|| "".into());
                let reference = refs.resolve(&id);
                let provider_reference = wire::create_bundled_reference_map(&"github-copilot".into()).get(&id).cloned();
                let limits = caps.as_ref().and_then(|c| record(c, "limits"));
                let get_limit = |key: &str| limits.as_ref().and_then(|l| to_number(l.get(key))).map(WireValue::Number);
                let window = positive(
                    get_limit("max_context_window_tokens").as_ref(),
                    Some(&positive(
                        entry.get("context_length"),
                        Some(&positive(
                            get_limit("max_prompt_tokens").as_ref(),
                            reference
                                .as_ref()
                                .and_then(|r| r.get("contextWindow"))
                                .filter(|v| !matches!(v, WireValue::Null))
                                .or_else(|| defaults.get("contextWindow")),
                        )),
                    )),
                );
                let max = positive(
                    get_limit("max_output_tokens").as_ref(),
                    Some(&positive(
                        entry.get("max_completion_tokens"),
                        Some(&positive(
                            get_limit("max_non_streaming_output_tokens").as_ref(),
                            reference
                                .as_ref()
                                .and_then(|r| r.get("maxTokens"))
                                .filter(|v| !matches!(v, WireValue::Null))
                                .or_else(|| defaults.get("maxTokens")),
                        )),
                    )),
                );
                let name = text(entry, "name")
                    .filter(|s| !crate::catalog_discovery::js_trim(s).is_empty())
                    .or_else(|| reference.as_ref().and_then(|r| text(r, "name")))
                    .or_else(|| text(&defaults, "name"))
                    .unwrap_or_else(|| "".into());
                let api = api_route_for(&"github-copilot".into(), &id)
                    .and_then(|r| text(&r, "api"))
                    .filter(|a| a.equals_ascii("anthropic-messages") || a.equals_ascii("openai-responses"))
                    .unwrap_or_else(|| "openai-completions".into());
                let vision =
                    caps.as_ref().and_then(|c| record(c, "supports")).and_then(|s| wire::boolean(&s, "vision"));
                let input = if vision == Some(true) {
                    input(Some(&WireValue::Array(vec![wire::str_value("image")])))
                } else if vision == Some(false) || !personal(&mapped_base) {
                    input(None)
                } else {
                    reference
                        .as_ref()
                        .and_then(|r| r.get("input"))
                        .filter(|v| !matches!(v, WireValue::Null))
                        .or_else(|| defaults.get("input"))
                        .cloned()
                        .unwrap_or_else(|| input(None))
                };
                let prices = record(entry, "billing").and_then(|b| record(&b, "token_prices"));
                let default_tier = prices.as_ref().and_then(|p| record(p, "default"));
                let long = prices.as_ref().and_then(|p| record(p, "long_context"));
                let default_max =
                    default_tier.as_ref().and_then(|p| to_number(p.get("context_max"))).filter(|n| *n > 0.0);
                let base_window = match (default_max, window.as_number(), max.as_number()) {
                    (Some(t), Some(w), Some(m)) => WireValue::Number(w.min(t + m)),
                    _ => window.clone(),
                };
                let mut m = reference.as_ref().map(|r| (**r).clone()).unwrap_or_else(|| (*defaults).clone());
                m.set("api", wire::str_value(api.clone()));
                if reference.is_some() {
                    m.set("provider", wire::str_value("github-copilot"));
                }
                m.set("baseUrl", wire::str_value(mapped_base.clone()));
                m.set("name", wire::str_value(name));
                m.set("input", input);
                m.set("contextWindow", base_window);
                m.set("maxTokens", max.clone());
                m.set_record(
                    "headers",
                    &merge_headers(provider_reference.as_ref().and_then(|r| r.record("headers")).as_ref()),
                );
                if reference.is_none() && api.equals_ascii("anthropic-messages") {
                    let identity = classify_wire(&"github-copilot".into(), &id, true)?;
                    let revision = text(&identity, "revision")
                        .and_then(|r| r.to_utf8().ok())
                        .and_then(|r| catalog_rules::parse_revision(&r));
                    if text(&identity, "class").is_some_and(|s| s.equals_ascii("anthropic"))
                        && revision.is_some_and(|r| catalog_rules::compare_revision(r, [3, 7, 0]) >= 0)
                    {
                        m.set("reasoning", WireValue::Bool(true));
                    }
                }
                if api.equals_ascii("openai-completions") {
                    m.set(
                        "compat",
                        WireValue::object(vec![
                            ("supportsStore", WireValue::Bool(false)),
                            ("supportsDeveloperRole", WireValue::Bool(false)),
                            ("supportsReasoningEffort", WireValue::Bool(false)),
                        ]),
                    );
                }
                let cache_write = m.record("cost").and_then(|c| c.get("cacheWrite").cloned());
                if let Some(cost) = tier_cost(default_tier.as_ref(), cache_write.as_ref()) {
                    m.set("cost", cost);
                }
                if let Some(t) = long.as_ref().and_then(|p| to_number(p.get("context_max"))).filter(|n| *n > 0.0)
                    && let (Some(window), Some(max), Some(base_window)) =
                        (window.as_number(), max.as_number(), m.get("contextWindow").and_then(WireValue::as_number))
                {
                    let variant_window = window.min(t + max);
                    if variant_window > base_window {
                        let mut variant = m.clone();
                        let variant_id = append(&text(&m, "id").unwrap_or_else(|| "".into()), "-1m");
                        variant.set("id", wire::str_value(variant_id.clone()));
                        copy_field(&mut variant, "requestModelId", &m, "id");
                        variant.set(
                            "name",
                            wire::str_value(append(&text(&m, "name").unwrap_or_else(|| "".into()), " (1M)")),
                        );
                        variant.set("contextWindow", WireValue::Number(variant_window));
                        if let Some(cost) = tier_cost(long.as_ref(), cache_write.as_ref()) {
                            variant.set("cost", cost);
                        }
                        variant.set_undefined("contextPromotionTarget");
                        mapped_variants.lock().expect("Copilot variants").push(Arc::new(variant));
                        if m.get("contextPromotionTarget").is_none_or(|v| matches!(v, WireValue::Null)) {
                            m.set(
                                "contextPromotionTarget",
                                wire::str_value(join(&"github-copilot/".into(), &variant_id)),
                            );
                        }
                    }
                }
                Ok(Some(Arc::new(m)))
            }));
            let Some(mut models) = fetch_openai_compatible_models(&ctx, &d).await? else {
                return Ok(RawModelValue::null());
            };
            for variant in variants.lock().expect("Copilot variants").iter() {
                if !models.iter().any(|m| text(m, "id") == text(variant, "id")) {
                    models.push(variant.clone());
                }
            }
            ctx.sort_by_id(&mut models);
            Ok(RawModelValue::models(models))
        }
    }));
    options
}
