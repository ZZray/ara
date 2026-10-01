//! Fixed OMP provider-models/ollama.ts (cloud-native tags/show catalog).
use super::{common::*, descriptor_types::ModelManagerConfig};
use crate::{
    catalog_discovery::{CatalogContext, DiscoveryError, DiscoveryRequest, HttpMethod, js_trim, zero_cost},
    catalog_rules,
    model_collapse::{SpecRef, VariantSpec},
    model_identity_wire::{self as wire, text},
    model_manager::{ModelManagerOptions, RawModelValue},
    model_wire_policy::classify_wire,
};
use ara_rpc::{WireString, WireValue};
use std::sync::Arc;
pub const OLLAMA_CLOUD_MAX_OUTPUT_TOKENS: f64 = 65_536.0;
pub fn is_ollama_cloud_output_capped(id: &WireString) -> bool {
    let separator = id.units().iter().position(|u| *u == 58);
    let base = separator.filter(|i| *i > 0).map_or_else(|| id.clone(), |i| id.slice_prefix(i));
    base.equals_ascii("deepseek-v4-flash") || base.equals_ascii("deepseek-v4-pro")
}
pub fn normalize_ollama_cloud_base_url(base: Option<&WireString>) -> WireString {
    let value = base.map(js_trim).filter(|v| !v.is_empty()).unwrap_or_else(|| "https://ollama.com".into());
    let value = trim_one_slash(&value);
    if ends(&value, "/api") { value.slice_prefix(value.len() - 4) } else { value }
}
fn cloud_headers(key: &WireString) -> Vec<(WireString, WireString)> {
    vec![
        ("Accept".into(), "application/json".into()),
        ("Authorization".into(), super::cache_provider_id::join(&"Bearer ".into(), key)),
    ]
}
fn context_window(metadata: &VariantSpec) -> Option<f64> {
    metadata.record("model_info")?.value.entries()?.into_iter().find_map(|(key, value)| {
        if [".context_length", ".num_ctx", ".context_window"].iter().any(|suffix| ends(key, suffix)) {
            value.as_number()
        } else {
            None
        }
    })
}
fn thinking_config(id: &WireString, capabilities: &[WireString]) -> Result<Option<VariantSpec>, DiscoveryError> {
    if !capabilities.iter().any(|c| c.equals_ascii("thinking")) {
        return Ok(None);
    }
    let identity = classify_wire(&"ollama-cloud".into(), id, true)?;
    let class = text(&identity, "class");
    let family = text(&identity, "family");
    let revision =
        text(&identity, "revision").and_then(|r| r.to_utf8().ok()).and_then(|r| catalog_rules::parse_revision(&r));
    let floor = catalog_rules::parse_revision(if family.as_ref().is_some_and(|f| f.equals_ascii("flash")) {
        "5.3"
    } else {
        "5.2"
    });
    let effort = class.is_some_and(|c| c.equals_ascii("glm"))
        && family.as_ref().is_none_or(|f| ["air", "turbo", "flash"].iter().any(|s| f.equals_ascii(s)))
        && revision.zip(floor).is_some_and(|(revision, floor)| catalog_rules::compare_revision(revision, floor) >= 0);
    Ok(Some(thinking("effort", if effort { &["high", "max"] } else { &["minimal", "low", "medium", "high"] })))
}
fn cloud_capability(value: &WireValue, capability: &str) -> Result<bool, DiscoveryError> {
    match value {
        WireValue::String(s) => Ok(includes(s, capability)),
        WireValue::Array(values) => {
            Ok(values.iter().any(|v| v.as_string().is_some_and(|s| s.equals_ascii(capability))))
        }
        _ => Err(DiscoveryError::named(
            "TypeError",
            "capabilities.includes is not a function. (In 'capabilities.includes(\"thinking\")', 'capabilities.includes' is undefined)",
        )),
    }
}
pub fn ollama_cloud_model_manager_options(default: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let context = context(default, &config);
    let base = normalize_ollama_cloud_base_url(config.base_url.as_ref());
    let key = config.api_key;
    let resolver =
        Arc::new(wire::ReferenceResolver::lazy(|| wire::create_bundled_reference_map(&"ollama-cloud".into())));
    let provider = Arc::new(wire::create_bundled_reference_map(&"ollama-cloud".into()));
    let mut options = ModelManagerOptions::new("ollama-cloud");
    options.dynamic_fetcher = Some(dynamic(move || {
        let context = context.clone();
        let base = base.clone();
        let key = key.clone();
        let resolver = resolver.clone();
        let provider = provider.clone();
        async move {
            let Some(key) = key.filter(|key| !key.is_empty()) else {
                return Ok(RawModelValue::models(Vec::new()));
            };
            let response = super::retry::fetch_with_ollama_retry(
                &context,
                DiscoveryRequest {
                    url: append(&base, "/api/tags"),
                    headers: cloud_headers(&key),
                    ..Default::default()
                },
            )
            .await?;
            if !response.ok() {
                return Err(DiscoveryError::new(format!(
                    "HTTP {} from {}/api/tags",
                    response.status,
                    String::from_utf16_lossy(base.units())
                )));
            }
            let payload = response.json()?;
            if matches!(payload.value, WireValue::Null) {
                return Err(DiscoveryError::named(
                    "TypeError",
                    "null is not an object (evaluating '(await response.json()).models')",
                ));
            }
            let entries = payload
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
          return;
        const reference = resolveReference(id), providerReference = getProviderReferences().get(id);
        let metadata;
        try {
          metadata = await fetchShowMetadata(baseUrl, apiKey, id, config?.fetch);
        } catch {
          metadata = void 0;
        }
        const capabilities = metadata?.capabilities, discoveredContextWindow = getContextWindow(metadata?.model_info), contextWindow = discoveredContextWindow ?? 128000, reasoning = capabilities ? capabilities.includes("thinking") : reference?.reasoning ?? !1, thinking = capabilities ? getThinkingConfig(id, capabilities) : reference?.thinking, input = capabilities ? capabilities.includes("vision") ? ["text", "image"] : ["text"] : reference?.input ?? ["text"], resolvedName = entry.name && entry.name !== id ? entry.name : reference?.name ?? id;
        return {
          id,
          name: resolvedName,
          api: "ollama-chat",
          provider: "ollama-cloud",
          baseUrl,
          reasoning,
          thinking,
          input,
          cost: reference?.cost ?? { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
          contextWindow,
          maxTokens: isOllamaCloudOutputCapped(id) ? Math.min(contextWindow, OLLAMA_CLOUD_MAX_OUTPUT_TOKENS) : discoveredContextWindow !== null && discoveredContextWindow !== void 0 ? providerReference?.maxTokens ?? Math.min(contextWindow, 8192) : Math.min(contextWindow, 8192),
          omitMaxOutputTokens: !0
        };
      })', 'entries.map' is undefined)"#,
                ));
            };
            let tasks = entries.iter().cloned().map(|entry| {
                let context = context.clone();
                let base = base.clone();
                let key = key.clone();
                let resolver = resolver.clone();
                let provider = provider.clone();
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
                    let reference = id.as_string().and_then(|id| resolver.resolve(id));
                    let provider_ref = id.as_string().and_then(|id| provider.get(id));
                    let mut headers = cloud_headers(&key);
                    headers.push(("Content-Type".into(), "application/json".into()));
                    let metadata = context
                        .transport
                        .fetch(DiscoveryRequest {
                            url: append(&base, "/api/show"),
                            method: HttpMethod::Post,
                            headers,
                            body: Some(WireValue::object(vec![("model", id.clone())]).stringify().into_bytes()),
                            ..Default::default()
                        })
                        .await
                        .ok()
                        .filter(|r| r.ok())
                        .and_then(|r| r.json().ok());
                    let capabilities = metadata
                        .as_ref()
                        .and_then(|m| m.get("capabilities"))
                        .filter(|v| crate::model_collapse::truthy(v));
                    let reasoning =
                        capabilities.map(|caps| cloud_capability(caps, "thinking")).transpose()?.unwrap_or_else(|| {
                            reference.as_ref().and_then(|r| wire::boolean(r, "reasoning")).unwrap_or(false)
                        });
                    let model_thinking = if capabilities.is_some() {
                        let Some(model_id) = id.as_string() else {
                            return Err(DiscoveryError::named(
                                "TypeError",
                                if reasoning {
                                    "modelId.trim is not a function. (In 'modelId.trim()', 'modelId.trim' is undefined)"
                                } else {
                                    "id.indexOf is not a function. (In 'id.indexOf(\":\")', 'id.indexOf' is undefined)"
                                },
                            ));
                        };
                        thinking_config(
                            model_id,
                            &(if reasoning { vec![WireString::from("thinking")] } else { Vec::new() }),
                        )?
                    } else {
                        reference.as_ref().and_then(|r| r.record("thinking"))
                    };
                    let model_input = if let Some(caps) = capabilities {
                        if cloud_capability(caps, "vision")? {
                            input(Some(&WireValue::Array(vec![wire::str_value("image")])))
                        } else {
                            input(None)
                        }
                    } else {
                        reference.as_ref().and_then(|r| r.get("input")).cloned().unwrap_or_else(|| input(None))
                    };
                    let discovered = metadata.as_ref().and_then(context_window);
                    let window = discovered.unwrap_or(128000.0);
                    let resolved_name = entry
                        .get("name")
                        .filter(|name| crate::model_collapse::truthy(name) && **name != id)
                        .cloned()
                        .or_else(|| reference.as_ref().and_then(|r| r.get("name")).cloned())
                        .unwrap_or_else(|| id.clone());
                    let Some(model_id) = id.as_string() else {
                        return Err(DiscoveryError::named(
                            "TypeError",
                            "id.indexOf is not a function. (In 'id.indexOf(\":\")', 'id.indexOf' is undefined)",
                        ));
                    };
                    let max = if is_ollama_cloud_output_capped(model_id) {
                        js_min(window, OLLAMA_CLOUD_MAX_OUTPUT_TOKENS)
                    } else if discovered.is_some() {
                        provider_ref
                            .and_then(|r| wire::number(r, "maxTokens"))
                            .unwrap_or_else(|| js_min(window, 8192.0))
                    } else {
                        js_min(window, 8192.0)
                    };
                    let mut out = VariantSpec::from_wire(WireValue::object(vec![
                        ("id", id),
                        ("name", resolved_name),
                        ("api", wire::str_value("ollama-chat")),
                        ("provider", wire::str_value("ollama-cloud")),
                        ("baseUrl", wire::str_value(base)),
                        ("reasoning", WireValue::Bool(reasoning)),
                    ]));
                    if let Some(thinking) = model_thinking {
                        out.set_record("thinking", &thinking);
                    } else {
                        out.set_undefined("thinking");
                    }
                    out.set("input", model_input);
                    out.set("cost", reference.as_ref().and_then(|r| r.get("cost")).cloned().unwrap_or_else(zero_cost));
                    out.set("contextWindow", WireValue::Number(window));
                    out.set("maxTokens", WireValue::Number(max));
                    out.set("omitMaxOutputTokens", WireValue::Bool(true));
                    Ok::<Option<SpecRef>, DiscoveryError>(Some(Arc::new(out)))
                }
            });
            let mut models: Vec<SpecRef> = promise_all(tasks).await?.into_iter().flatten().collect();
            context.sort_by_id(&mut models);
            Ok(RawModelValue::models(models))
        }
    }));
    options
}
