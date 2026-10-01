//! Fixed OMP factories Umans/OpenAI/GMI/Novita/DeepInfra.
use super::*;
use crate::{
    catalog_discovery::{DiscoveryRequest, zero_cost},
    catalog_rules,
    model_collapse::truthy,
};
use std::collections::HashSet;
fn builder(
    api: &str,
    provider: &str,
    base: &str,
    config: ModelManagerConfig,
    map: ProviderMapper,
) -> CompatibleBuilder {
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
pub fn openai_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let mut options = builder("openai-responses", "openai", "https://api.openai.com/v1", config, bundled_mapper());
    options.require_api_key = true;
    options.filter = Some(Arc::new(|_, model, references| {
        let id = crate::catalog_discovery::js_trim(&text(model, "id").unwrap_or_else(|| "".into()));
        Ok(!id.is_empty() && (references.contains_key(&id) || super::super::behavior::likely_responses_id(&id)))
    }));
    compatible_options(ctx, options)
}
pub fn project_openai_pro_reasoning_aliases(models: &[SpecRef]) -> Vec<SpecRef> {
    let vocabulary = catalog_rules::discovery_vocabulary();
    let sweeps = vocabulary["proReasoningSweep"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let aliases = &vocabulary["proReasoningAliases"];
    let generated = |model: &SpecRef| {
        let provider = text(model, "provider").unwrap_or_else(|| "".into());
        let id = text(model, "id").unwrap_or_else(|| "".into());
        sweeps.iter().filter_map(|v| v.as_str()).any(|v| provider.equals_ascii(v))
            && model.get("reasoningMode").is_some()
            && ends(&id, "-pro")
            && aliases.as_object().is_some_and(|map| {
                map.values().any(|bases| {
                    bases.as_array().is_some_and(|bases| {
                        bases.iter().filter_map(|v| v.as_str()).any(|v| id.slice_prefix(id.len() - 4).equals_ascii(v))
                    })
                })
            })
    };
    let kept: Vec<_> = models.iter().filter(|m| !generated(m)).cloned().collect();
    let mut ids: HashSet<_> = kept
        .iter()
        .map(|m| (text(m, "provider").unwrap_or_else(|| "".into()), text(m, "id").unwrap_or_else(|| "".into())))
        .collect();
    let mut out = kept.clone();
    for model in kept {
        let provider = text(&model, "provider").unwrap_or_else(|| "".into());
        let id = text(&model, "id").unwrap_or_else(|| "".into());
        let bases = provider.to_utf8().ok().and_then(|p| aliases.get(p)).and_then(|v| v.as_array());
        if !bases.is_some_and(|bases| bases.iter().filter_map(|v| v.as_str()).any(|base| id.equals_ascii(base))) {
            continue;
        }
        let alias = append(&id, "-pro");
        if !ids.insert((provider, alias.clone())) {
            continue;
        }
        let mut projected = (*model).clone();
        projected.set("id", wire::str_value(alias));
        projected.set("name", wire::str_value(append(&text(&model, "name").unwrap_or_else(|| "".into()), " Pro")));
        projected.set("requestModelId", wire::str_value(id));
        projected.set("reasoningMode", wire::str_value("pro"));
        out.push(Arc::new(projected));
    }
    out
}
pub fn gmi_cloud_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let map = Arc::new(|entry: &VariantSpec, defaults: &VariantSpec, reference: Option<&VariantSpec>| {
        if let Some(reference) = reference {
            return Ok(Some(map_with_bundled_reference(entry, defaults, Some(reference))));
        }
        let canonical = wire::resolve_model_reference(
            &text(defaults, "id").unwrap_or_else(|| "".into()),
            wire::bundled_model_reference_index(),
        );
        let Some(canonical) = canonical else {
            return Ok(Some(map_with_bundled_reference(entry, defaults, None)));
        };
        let window = canonical
            .get("contextWindow")
            .filter(|v| !matches!(v, WireValue::Null))
            .or_else(|| defaults.get("contextWindow"))
            .cloned()
            .unwrap_or(WireValue::Null);
        let cap = canonical
            .get("maxTokens")
            .filter(|v| !matches!(v, WireValue::Null))
            .or_else(|| defaults.get("maxTokens"))
            .cloned()
            .unwrap_or(WireValue::Null);
        let cap = match (cap.as_number(), window.as_number()) {
            (Some(cap), Some(window)) => WireValue::Number(cap.min(window)),
            _ => cap,
        };
        let mut out = defaults.clone();
        out.set(
            "name",
            wire::str_value(name(
                entry.get("name"),
                &text(&canonical, "name").or_else(|| text(defaults, "name")).unwrap_or_else(|| "".into()),
            )),
        );
        for key in ["reasoning", "input"] {
            copy_field(&mut out, key, &canonical, key);
        }
        if let Some(thinking) = canonical.record("thinking").filter(|v| truthy(&v.value)) {
            out.set_record("thinking", &thinking);
        }
        out.set("contextWindow", window);
        out.set("maxTokens", cap);
        Ok(Some(out))
    });
    let mut options = builder("openai-completions", "gmi-cloud", "https://api.gmi-serving.com/v1", config, map);
    options.require_api_key = true;
    compatible_options(ctx, options)
}
fn novita_price(value: Option<&WireValue>) -> f64 {
    to_number(value).filter(|n| *n > 0.0).unwrap_or(0.0) / 10000.0
}
pub fn novita_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let map = Arc::new(|entry: &VariantSpec, defaults: &VariantSpec, reference: Option<&VariantSpec>| {
        let mut renamed = entry.clone();
        let chosen = ["display_name", "title", "name"]
            .iter()
            .find(|key| entry.get(key).is_some_and(|v| !matches!(v, WireValue::Null)));
        if let Some(key) = chosen {
            copy_field(&mut renamed, "name", entry, key);
        } else {
            renamed.set_undefined("name");
        }
        let mut out = map_with_bundled_reference(&renamed, defaults, reference);
        out.set("reasoning", WireValue::Bool(wire::has_string(entry, "features", "reasoning")));
        out.set("supportsTools", WireValue::Bool(wire::has_string(entry, "features", "function-calling")));
        out.set("input", input(entry.get("input_modalities")));
        let cache_read = entry
            .record("pricing")
            .and_then(|p| p.record("input_cache_read"))
            .map_or(0.0, |p| novita_price(p.get("price_per_m")));
        out.set(
            "cost",
            WireValue::object(vec![
                ("input", WireValue::Number(novita_price(entry.get("input_token_price_per_m")))),
                ("output", WireValue::Number(novita_price(entry.get("output_token_price_per_m")))),
                ("cacheRead", WireValue::Number(cache_read)),
                ("cacheWrite", WireValue::Number(0.0)),
            ]),
        );
        for (target, source) in [("contextWindow", "context_size"), ("maxTokens", "max_output_tokens")] {
            let value = positive(entry.get(source), out.get(target));
            out.set(target, value);
        }
        Ok(Some(out))
    });
    let mut options = builder("openai-completions", "novita", "https://api.novita.ai/openai/v1", config, map);
    options.authoritative = true;
    options.filter = Some(Arc::new(|entry, model, _| {
        Ok(entry.get("status").and_then(WireValue::as_number).is_none_or(|n| n == 1.0)
            && !starts(&lower(&text(model, "id").unwrap_or_else(|| "".into())), "ai_infer_test")
            && wire::has_string(entry, "endpoints", "chat/completions")
            && to_number(entry.get("max_output_tokens")).is_some_and(|n| n > 0.0))
    }));
    compatible_options(ctx, options)
}
fn umans_thinking(value: Option<&VariantSpec>) -> Option<VariantSpec> {
    let value = value?;
    let reasoning = if matches!(value.value, WireValue::Object(_)) {
        wire::boolean(value, "supported") == Some(true)
    } else {
        matches!(value.value, WireValue::Bool(true))
    };
    if !reasoning {
        return None;
    }
    let mut efforts = value
        .get("levels")
        .and_then(WireValue::as_array)
        .map(|values| {
            let mut efforts = Vec::new();
            for value in values.iter().filter_map(WireValue::as_string) {
                if ["minimal", "low", "medium", "high", "xhigh", "max"].iter().any(|level| value.equals_ascii(level))
                    && !efforts.contains(value)
                {
                    efforts.push(value.clone());
                }
            }
            efforts
        })
        .unwrap_or_default();
    if efforts.is_empty() {
        efforts = ["minimal", "low", "medium", "high", "xhigh"].iter().map(|s| (*s).into()).collect();
    }
    let mut out = wire::empty();
    out.set(
        "mode",
        wire::str_value(if wire::has_string(value, "levels", "max") { "anthropic-budget-effort" } else { "budget" }),
    );
    out.set("efforts", WireValue::Array(efforts.iter().cloned().map(WireValue::String).collect()));
    if wire::boolean(value, "can_disable") == Some(false) {
        out.set("requiresEffort", WireValue::Bool(true));
    }
    if let Some(default) = text(value, "default_level").filter(|level| efforts.contains(level)) {
        out.set("defaultLevel", wire::str_value(default));
    }
    Some(out)
}
pub fn umans_model_manager_options(default: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let context = context(default, &config);
    let base = config
        .base_url
        .as_ref()
        .map(crate::catalog_discovery::js_trim)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "https://api.code.umans.ai".into());
    let base = trim_one_slash(&base);
    let base = if ends(&base, "/v1") { base.slice_prefix(base.len() - 3) } else { base };
    let url = append(&base, "/v1/models/info");
    let key = config.api_key;
    let references = Arc::new(wire::create_bundled_reference_map(&"umans".into()));
    let mut options = ModelManagerOptions::new("umans");
    options.dynamic_models_authoritative = true;
    options.drop_cached_model_ids_on_static_mismatch = Some(vec!["umans-glm-5.1".into(), "umans-glm-5.2".into()]);
    options.dynamic_fetcher = Some(dynamic(move || {
        let context = context.clone();
        let base = base.clone();
        let url = url.clone();
        let key = key.clone();
        let references = references.clone();
        async move {
            let mut headers = vec![("Accept".into(), "application/json".into())];
            if let Some(key) = key.filter(|key| !key.is_empty()) {
                headers.push(("x-api-key".into(), key));
            }
            let response = context
                .transport
                .fetch(DiscoveryRequest { url, headers, ..Default::default() })
                .await
                .map_err(|_| DiscoveryError::new("Failed to fetch Umans models info"))?;
            if !response.ok() {
                return Ok(RawModelValue::null());
            }
            let payload = response.json().map_err(|_| DiscoveryError::new("Failed to fetch Umans models info"))?;
            if !matches!(payload.value, WireValue::Object(_)) {
                return Ok(RawModelValue::null());
            }
            let mut models = Vec::new();
            for id in payload.own_keys() {
                let Some(raw) = payload.record_key(&id) else { continue };
                if !matches!(raw.value, WireValue::Object(_)) || id.is_empty() {
                    continue;
                }
                let reference = references.get(&id);
                let capabilities = raw
                    .record("capabilities")
                    .filter(|v| matches!(v.value, WireValue::Object(_)))
                    .unwrap_or_else(wire::empty);
                let thinking = umans_thinking(capabilities.record("reasoning").as_ref());
                let mut out = reference.map_or_else(wire::empty, |reference| (**reference).clone());
                out.set("id", wire::str_value(id.clone()));
                out.set("name", wire::str_value(name(raw.get("display_name"), &name(raw.get("name"), &id))));
                out.set("api", wire::str_value("anthropic-messages"));
                out.set("provider", wire::str_value("umans"));
                out.set("baseUrl", wire::str_value(base.clone()));
                let mut compat = reference.and_then(|r| r.record("compat")).unwrap_or_else(wire::empty);
                compat.set("escapeBuiltinToolNames", WireValue::Bool(true));
                out.set_record("compat", &compat);
                out.set("reasoning", WireValue::Bool(thinking.is_some()));
                if let Some(thinking) = thinking {
                    out.set_record("thinking", &thinking);
                }
                out.set(
                    "input",
                    if wire::boolean(&capabilities, "supports_vision") == Some(true) {
                        input(Some(&WireValue::Array(vec![wire::str_value("image")])))
                    } else {
                        input(None)
                    },
                );
                if wire::boolean(&capabilities, "supports_tools") == Some(false) {
                    out.set("supportsTools", WireValue::Bool(false));
                }
                out.set("cost", reference.and_then(|r| r.get("cost")).cloned().unwrap_or_else(zero_cost));
                out.set(
                    "contextWindow",
                    positive(capabilities.get("context_window"), reference.and_then(|r| r.get("contextWindow"))),
                );
                let max =
                    positive(capabilities.get("max_completion_tokens"), reference.and_then(|r| r.get("maxTokens")));
                out.set("maxTokens", positive(capabilities.get("recommended_max_tokens"), Some(&max)));
                models.push(Arc::new(out));
            }
            context.sort_by_id(&mut models);
            Ok(RawModelValue::models(models))
        }
    }));
    options
}
pub const DEEPINFRA_BASE_URL: &str = "https://api.deepinfra.com/v1/openai";
pub fn map_deepinfra_model(
    entry: &VariantSpec,
    base: &WireString,
    reference: Option<&VariantSpec>,
) -> Option<VariantSpec> {
    let id = text(entry, "id").map(|id| crate::catalog_discovery::js_trim(&id)).filter(|id| !id.is_empty())?;
    let metadata =
        entry.record("metadata").filter(|v| matches!(v.value, WireValue::Object(_))).unwrap_or_else(wire::empty);
    let tags = wire::list(&metadata, "tags");
    let has = |tag: &str| tags.iter().any(|value| value.equals_ascii(tag));
    if !has("chat") {
        return None;
    }
    let pricing =
        metadata.record("pricing").filter(|v| matches!(v.value, WireValue::Object(_))).unwrap_or_else(wire::empty);
    let window = positive(metadata.get("context_length"), reference.and_then(|r| r.get("contextWindow")));
    let live = to_number(metadata.get("max_tokens")).filter(|v| *v > 0.0).unwrap_or(0.0);
    let cap = reference
        .and_then(|r| r.get("maxTokens"))
        .filter(|v| !matches!(v, WireValue::Null))
        .cloned()
        .unwrap_or(WireValue::Null);
    let max = if window.as_number().is_some_and(|window| live > 0.0 && live < window) {
        WireValue::Number(live)
    } else if let (Some(cap), Some(window)) = (cap.as_number(), window.as_number()) {
        WireValue::Number(cap.min(window))
    } else {
        cap
    };
    let mut out = reference.cloned().unwrap_or_else(wire::empty);
    out.set("id", wire::str_value(id.clone()));
    out.set("name", wire::str_value(reference.and_then(|r| text(r, "name")).unwrap_or(id)));
    out.set("api", wire::str_value("openai-completions"));
    out.set("provider", wire::str_value("deepinfra"));
    out.set("baseUrl", wire::str_value(base.clone()));
    out.set("reasoning", WireValue::Bool(has("reasoning") || has("reasoning_effort")));
    if has("reasoning_effort") {
        out.set_record("thinking", &thinking("effort", &["low", "medium", "high"]));
    }
    out.set(
        "input",
        if has("vision") || has("vlm") {
            input(Some(&WireValue::Array(vec![wire::str_value("image")])))
        } else {
            input(None)
        },
    );
    out.set(
        "cost",
        WireValue::object(vec![
            ("input", positive(pricing.get("input_tokens"), Some(&WireValue::Number(0.0)))),
            ("output", positive(pricing.get("output_tokens"), Some(&WireValue::Number(0.0)))),
            ("cacheRead", positive(pricing.get("cache_read_tokens"), Some(&WireValue::Number(0.0)))),
            ("cacheWrite", WireValue::Number(0.0)),
        ]),
    );
    out.set("contextWindow", window);
    out.set("maxTokens", max);
    Some(out)
}
pub fn deepinfra_model_manager_options(default: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let context = context(default, &config);
    let base = trim_one_slash(&config.base_url.clone().unwrap_or_else(|| DEEPINFRA_BASE_URL.into()));
    let key = config.api_key;
    let references = Arc::new(wire::create_bundled_reference_map(&"deepinfra".into()));
    let mut options = ModelManagerOptions::new("deepinfra");
    options.dynamic_models_authoritative = true;
    options.dynamic_fetcher = Some(dynamic(move || {
        let context = context.clone();
        let base = base.clone();
        let key = key.clone();
        let references = references.clone();
        async move {
            let mut headers = vec![("Accept".into(), "application/json".into())];
            if let Some(key) = key.filter(|key| !key.is_empty()) {
                headers.push(("Authorization".into(), super::super::cache_provider_id::join(&"Bearer ".into(), &key)));
            }
            let response = timed_fetch(
                &context,
                DiscoveryRequest {
                    url: append(&base, "/models?filter=with_meta&sort_by=omp"),
                    headers,
                    ..Default::default()
                },
                10000.0,
            )
            .await
            .ok()
            .filter(|r| r.ok());
            let payload = response.and_then(|r| r.json().ok());
            let Some(payload) = payload.filter(|p| matches!(p.value, WireValue::Object(_))) else {
                return Ok(RawModelValue::null());
            };
            let Some(entries) = payload.get("data").and_then(WireValue::as_array) else {
                return Ok(RawModelValue::null());
            };
            let mut seen = HashSet::new();
            let mut models = Vec::new();
            for entry in entries {
                let entry = VariantSpec::from_wire(entry.clone());
                if !matches!(entry.value, WireValue::Object(_)) {
                    continue;
                }
                let reference = text(&entry, "id").and_then(|id| references.get(&id)).map(|r| r.as_ref());
                if let Some(mapped) = map_deepinfra_model(&entry, &base, reference)
                    && seen.insert(text(&mapped, "id").expect("id"))
                {
                    models.push(Arc::new(mapped));
                }
            }
            Ok(RawModelValue::models(models))
        }
    }));
    options
}
