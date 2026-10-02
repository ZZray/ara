//! Native models.dev descriptors, mapper, and additive fallback from OMP 596f2da.
use super::*;
use crate::{
    model_collapse::CollapseModelPolicy,
    model_wire_policy::{WireModelPolicy, classify_wire},
};
use async_trait::async_trait;
pub type ModelsDevFilter = Arc<dyn Fn(&WireString, &VariantSpec) -> Result<bool, DiscoveryError> + Send + Sync>;
pub type ModelsDevTransform =
    Arc<dyn Fn(VariantSpec, &WireString, &VariantSpec) -> Result<Vec<VariantSpec>, DiscoveryError> + Send + Sync>;
pub type ModelsDevApiResolver =
    Arc<dyn Fn(&WireString, &VariantSpec) -> Result<Option<(WireString, WireString)>, DiscoveryError> + Send + Sync>;
#[derive(Clone)]
pub struct ModelsDevProviderDescriptor {
    pub models_dev_key: WireString,
    pub provider_id: WireString,
    pub api: WireString,
    pub base_url: WireString,
    pub default_context_window: Option<f64>,
    pub default_max_tokens: Option<f64>,
    pub compat: Option<VariantSpec>,
    pub headers: Option<VariantSpec>,
    pub filter_model: Option<ModelsDevFilter>,
    pub transform_model: Option<ModelsDevTransform>,
    pub resolve_api: Option<ModelsDevApiResolver>,
}
impl ModelsDevProviderDescriptor {
    pub fn new(key: &str, provider: &str, api: &str, base: &str) -> Self {
        Self {
            models_dev_key: key.into(),
            provider_id: provider.into(),
            api: api.into(),
            base_url: base.into(),
            default_context_window: None,
            default_max_tokens: None,
            compat: None,
            headers: None,
            filter_model: None,
            transform_model: None,
            resolve_api: None,
        }
    }
}
pub fn map_models_dev_to_models(
    data: &VariantSpec,
    descriptors: &[ModelsDevProviderDescriptor],
) -> Result<Vec<SpecRef>, DiscoveryError> {
    let mut out = Vec::new();
    for desc in descriptors {
        let Some(provider) = data.record_key(&desc.models_dev_key).filter(|v| matches!(v.value, WireValue::Object(_)))
        else {
            continue;
        };
        let Some(models) = provider.record("models").filter(|v| matches!(v.value, WireValue::Object(_))) else {
            continue;
        };
        for id in models.own_keys() {
            let Some(raw) = models.record_key(&id) else {
                continue;
            };
            if !matches!(raw.value, WireValue::Object(_)) {
                continue;
            }
            let keep = match &desc.filter_model {
                Some(filter) => filter(&id, &raw)?,
                None => wire::boolean(&raw, "tool_call") == Some(true),
            };
            if !keep {
                continue;
            } // Null resolver results are nullish-fallbacks in the fixed source.
            let resolved = desc
                .resolve_api
                .as_ref()
                .map(|resolve| resolve(&id, &raw))
                .transpose()?
                .flatten()
                .unwrap_or_else(|| (desc.api.clone(), desc.base_url.clone()));
            let cost = raw.record("cost");
            let limits = raw.record("limit");
            let modalities = raw.record("modalities");
            let mut model = VariantSpec::from_wire(WireValue::object(vec![
                ("id", wire::str_value(id.clone())),
                ("name", wire::str_value(name(raw.get("name"), &id))),
                ("api", wire::str_value(resolved.0)),
                ("provider", wire::str_value(desc.provider_id.clone())),
                ("baseUrl", wire::str_value(resolved.1)),
                ("reasoning", WireValue::Bool(wire::boolean(&raw, "reasoning") == Some(true))),
                ("input", input(modalities.as_ref().and_then(|m| m.get("input")))),
                (
                    "cost",
                    WireValue::object(
                        [
                            ("input", "input"),
                            ("output", "output"),
                            ("cacheRead", "cache_read"),
                            ("cacheWrite", "cache_write"),
                        ]
                        .iter()
                        .map(|(target, source)| {
                            (
                                *target,
                                WireValue::Number(to_number(cost.as_ref().and_then(|c| c.get(source))).unwrap_or(0.0)),
                            )
                        })
                        .collect(),
                    ),
                ),
                (
                    "contextWindow",
                    positive(
                        limits.as_ref().and_then(|l| l.get("context")),
                        desc.default_context_window.map(WireValue::Number).as_ref(),
                    ),
                ),
                (
                    "maxTokens",
                    positive(
                        limits.as_ref().and_then(|l| l.get("output")),
                        desc.default_max_tokens.map(WireValue::Number).as_ref(),
                    ),
                ),
            ]));
            for key in ["int", "tps"] {
                if raw.get(key).is_some_and(|v| !matches!(v, WireValue::Null)) {
                    copy_field(&mut model, key, &raw, key);
                }
            }
            if wire::boolean(&raw, "tool_call") == Some(false) {
                model.set("supportsTools", WireValue::Bool(false));
            }
            if let Some(compat) = &desc.compat {
                model.set_record("compat", compat);
            }
            if let Some(headers) = &desc.headers {
                model.set_record("headers", headers);
            }
            let transformed = match &desc.transform_model {
                Some(transform) => transform(model, &id, &raw)?,
                None => vec![model],
            };
            out.extend(transformed.into_iter().map(Arc::new));
        }
    }
    Ok(out)
}
pub fn copilot_api_headers() -> VariantSpec {
    VariantSpec::from_json(
        &serde_json::json!({"User-Agent":"copilot/1.0.82","Editor-Version":"copilot/1.0.82","Copilot-Integration-Id":"copilot-developer-cli","Copilot-Harness-Id":"copilot-sdk","Openai-Intent":"conversation-agent","X-GitHub-Api-Version":"2026-08-01"}),
    )
}
pub fn xai_responses_reasoning_effort_map(id: &WireString) -> Result<VariantSpec, DiscoveryError> {
    let identity = classify_wire(&"xai".into(), id, true)?;
    let revision = text(&identity, "revision")
        .and_then(|r| r.to_utf8().ok())
        .and_then(|r| crate::catalog_rules::parse_revision(&r));
    let floor = crate::catalog_rules::parse_revision("4.6");
    let current = text(&identity, "class").is_some_and(|v| v.equals_ascii("xai"))
        && text(&identity, "family").is_some_and(|v| v.equals_ascii("grok"))
        && revision.zip(floor).is_some_and(|(r, f)| crate::catalog_rules::compare_revision(r, f) >= 0);
    Ok(VariantSpec::from_json(&if current {
        serde_json::json!({"minimal":"low"})
    } else {
        serde_json::json!({"minimal":"low","xhigh":"high","max":"high"})
    }))
}
pub fn apply_xai_responses_thinking_policy(mut model: VariantSpec) -> Result<VariantSpec, DiscoveryError> {
    let policy = WireModelPolicy.resolve(&model)?;
    let mut compat = model.record("compat").unwrap_or_else(wire::empty);
    let policy_compat = policy.record("compat").unwrap_or_else(wire::empty);
    let capable = compat
        .get("supportsReasoningEffort")
        .filter(|v| !matches!(v, WireValue::Null))
        .cloned()
        .or_else(|| policy_compat.get("supportsReasoningEffort").cloned());
    match capable {
        Some(value) => compat.set("supportsReasoningEffort", value),
        None => compat.set_undefined("supportsReasoningEffort"),
    };
    let capable = compat.get("supportsReasoningEffort").is_some_and(crate::model_collapse::truthy);
    let omit = compat
        .get("omitReasoningEffort")
        .filter(|v| !matches!(v, WireValue::Null))
        .cloned()
        .unwrap_or(WireValue::Bool(!capable));
    compat.set("omitReasoningEffort", omit);
    if capable {
        compat.set_record(
            "reasoningEffortMap",
            &xai_responses_reasoning_effort_map(&text(&model, "id").unwrap_or_else(|| "".into()))?,
        );
    } else {
        compat.remove("reasoningEffortMap");
    }
    model.set_record("compat", &compat);
    Ok(model)
}
pub fn is_kimi_k27_code_model_id(id: &WireString) -> Result<bool, DiscoveryError> {
    let identity = classify_wire(&"fireworks".into(), id, true)?;
    Ok(text(&identity, "class").is_some_and(|s| s.equals_ascii("kimi"))
        && text(&identity, "family").is_some_and(|s| s.equals_ascii("k2.7-code")))
}
pub const KIMI_K27_CODE_RECOMMENDED_MAX_TOKENS: f64 = 32768.0;
pub fn clamp_kimi_k27_code_max_tokens(id: &WireString, value: WireValue) -> Result<WireValue, DiscoveryError> {
    if matches!(value, WireValue::Null) {
        return Ok(value);
    }
    if is_kimi_k27_code_model_id(id)? {
        return Ok(WireValue::Number(js_min(
            value.as_number().unwrap_or(f64::NAN),
            KIMI_K27_CODE_RECOMMENDED_MAX_TOKENS,
        )));
    }
    Ok(value)
}
pub fn resolve_zai_api(id: &WireString) -> Result<(WireString, WireString), DiscoveryError> {
    let api =
        api_route_for(&"zai".into(), id).and_then(|r| text(&r, "api")).unwrap_or_else(|| "anthropic-messages".into());
    let base = if api.equals_ascii("anthropic-messages") {
        "https://api.z.ai/api/anthropic"
    } else if api.equals_ascii("openai-completions") {
        "https://api.z.ai/api/coding/paas/v4"
    } else {
        return Err(DiscoveryError::new(format!(
            "Unsupported Z.AI API route: {}",
            String::from_utf16_lossy(api.units())
        )));
    };
    Ok((api, base.into()))
}
fn active() -> ModelsDevFilter {
    Arc::new(|_, raw| {
        Ok(wire::boolean(raw, "tool_call") == Some(true)
            && text(raw, "status").is_none_or(|s| !s.equals_ascii("deprecated")))
    })
}
pub fn resolve_opencode_api(
    provider: &WireString,
    id: &WireString,
    raw: &VariantSpec,
    base: &WireString,
) -> (WireString, WireString) {
    let route = api_route_for(provider, id).and_then(|r| text(&r, "api"));
    let npm = raw.record("provider").and_then(|p| text(&p, "npm"));
    let api = route
        .filter(|a| ["openai-completions", "openai-responses", "anthropic-messages"].iter().any(|s| a.equals_ascii(s)))
        .unwrap_or_else(|| {
            if npm.as_ref().is_some_and(|n| n.equals_ascii("@ai-sdk/openai")) {
                "openai-responses".into()
            } else if npm.as_ref().is_some_and(|n| n.equals_ascii("@ai-sdk/anthropic")) {
                "anthropic-messages".into()
            } else if npm.as_ref().is_some_and(|n| n.equals_ascii("@ai-sdk/google")) {
                "google-generative-ai".into()
            } else {
                "openai-completions".into()
            }
        });
    let url = if api.equals_ascii("anthropic-messages") { base.clone() } else { append(base, "/v1") };
    (api, url)
}
const EFFORTS: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "max"];
fn cline_thinking(raw: &VariantSpec, model: &VariantSpec) -> Option<VariantSpec> {
    let options = raw.get("reasoning_options")?.as_array()?;
    let options: Vec<_> = options.iter().cloned().map(VariantSpec::from_wire).collect();
    if let Some(values) =
        options.iter().find(|o| text(o, "type").is_some_and(|t| t.equals_ascii("effort"))).and_then(|o| o.get("values"))
    {
        let efforts: Vec<_> = EFFORTS
            .iter()
            .copied()
            .filter(|e| {
                values.as_array().is_some_and(|v| v.iter().any(|v| v.as_string().is_some_and(|s| s.equals_ascii(e))))
            })
            .collect();
        return (!efforts.is_empty()).then(|| thinking("effort", &efforts));
    }
    let budget = options.iter().find(|o| text(o, "type").is_some_and(|t| t.equals_ascii("budget_tokens")))?;
    let cap = model.get("maxTokens").and_then(WireValue::as_number).unwrap_or(8192.0);
    let max = to_number(budget.get("max")).filter(|n| *n > 0.0).unwrap_or(cap);
    let min = to_number(budget.get("min")).filter(|n| *n > 0.0).unwrap_or(1.0);
    let mut config = thinking("budget", EFFORTS);
    config.set(
        "effortBudgets",
        WireValue::object(
            EFFORTS
                .iter()
                .zip([0.1, 0.2, 0.5, 0.8, 0.95, 1.0])
                .map(|(e, r)| (*e, WireValue::Number((cap.min(max) * r).floor().max(min).min(max))))
                .collect(),
        ),
    );
    Some(config)
}
pub fn models_dev_provider_descriptors() -> Vec<ModelsDevProviderDescriptor> {
    let mut all = Vec::new();
    let mut bedrock = ModelsDevProviderDescriptor::new(
        "amazon-bedrock",
        "amazon-bedrock",
        "bedrock-converse-stream",
        "https://bedrock-runtime.us-east-1.amazonaws.com",
    );
    bedrock.filter_model = Some(Arc::new(|id, raw| {
        Ok(wire::boolean(raw, "tool_call") == Some(true) && !is_excluded_model(&"amazon-bedrock".into(), id))
    }));
    bedrock.transform_model = Some(Arc::new(|mut model, id, raw| {
        let global = [
            "anthropic.claude-fable-5",
            "anthropic.claude-mythos-5",
            "anthropic.claude-haiku-4-5",
            "anthropic.claude-sonnet-4",
            "anthropic.claude-opus-4-5",
            "amazon.nova-2-lite",
            "cohere.embed-v4",
            "twelvelabs.pegasus-1-2",
        ]
        .iter()
        .any(|s| starts(id, s));
        let us = [
            "amazon.nova-lite",
            "amazon.nova-micro",
            "amazon.nova-premier",
            "amazon.nova-pro",
            "anthropic.claude-3-7-sonnet",
            "anthropic.claude-opus-4-1",
            "anthropic.claude-opus-4-20250514",
            "deepseek.r1",
            "meta.llama3-2",
            "meta.llama3-3",
            "meta.llama4",
        ]
        .iter()
        .any(|s| starts(id, s));
        let mapped = if global {
            super::super::cache_provider_id::join(&"global.".into(), id)
        } else if us {
            super::super::cache_provider_id::join(&"us.".into(), id)
        } else {
            id.clone()
        };
        model.set("id", wire::str_value(mapped.clone()));
        model.set("name", wire::str_value(name(raw.get("name"), &mapped)));
        let mut out = vec![model.clone()];
        if starts(id, "anthropic.") {
            let display = name(raw.get("name"), id);
            for (prefix, suffix) in [("eu.", " (EU)"), ("us-gov.", " (GovCloud)")] {
                let mut geo = model.clone();
                geo.set("id", wire::str_value(super::super::cache_provider_id::join(&prefix.into(), id)));
                geo.set("name", wire::str_value(append(&display, suffix)));
                out.push(geo);
            }
        }
        Ok(out)
    }));
    all.push(bedrock);
    let mut vertex = ModelsDevProviderDescriptor::new(
        "google-vertex",
        "google-vertex",
        "google-vertex",
        "https://{location}-aiplatform.googleapis.com",
    );
    vertex.filter_model = Some(active());
    vertex.resolve_api = Some(Arc::new(|id, raw| {
        let npm = raw.record("provider").and_then(|p| text(&p, "npm"));
        let pair = if npm.as_ref().is_some_and(|s| s.equals_ascii("@ai-sdk/google-vertex/anthropic")) {
            ("anthropic-messages".into(),replace_first(&"https://{location}-aiplatform.googleapis.com/v1/projects/{project}/locations/{location}/publishers/anthropic/models/{model}:streamRawPredict".into(),&"{model}".into(),id))
        } else if includes(id, "/") || npm.as_ref().is_some_and(|s| s.equals_ascii("@ai-sdk/openai-compatible")) {
            ("openai-completions".into(),"https://{location}-aiplatform.googleapis.com/v1/projects/{project}/locations/{location}/endpoints/openapi".into())
        } else {
            ("google-vertex".into(), "https://{location}-aiplatform.googleapis.com".into())
        };
        Ok(Some(pair))
    }));
    all.push(vertex);
    for (key, provider, api, base) in [
        ("anthropic", "anthropic", "anthropic-messages", "https://api.anthropic.com"),
        ("google", "google", "google-generative-ai", "https://generativelanguage.googleapis.com/v1beta"),
        ("openai", "openai", "openai-responses", "https://api.openai.com/v1"),
        ("groq", "groq", "openai-completions", "https://api.groq.com/openai/v1"),
        ("cerebras", "cerebras", "openai-completions", "https://api.cerebras.ai/v1"),
        ("cline-pass", "cline-pass", "openai-completions", "https://api.cline.bot/api/v1"),
        ("togetherai", "together", "openai-completions", "https://api.together.xyz/v1"),
        ("wandb", "coreweave", "openai-completions", "https://api.inference.wandb.ai/v1"),
        ("nvidia", "nvidia", "openai-completions", "https://integrate.api.nvidia.com/v1"),
        ("xai", "xai", "openai-responses", "https://api.x.ai/v1"),
        ("deepseek", "deepseek", "openai-completions", "https://api.deepseek.com"),
    ] {
        let mut d = ModelsDevProviderDescriptor::new(key, provider, api, base);
        match provider {
            "anthropic" => {
                d.filter_model = Some(Arc::new(|id, m| {
                    Ok(wire::boolean(m, "tool_call") == Some(true) && !is_excluded_model(&"anthropic".into(), id))
                }))
            }
            "cline-pass" => {
                d.transform_model = Some(Arc::new(|mut model, id, raw| {
                    let id = if starts(id, "cline-pass/") {
                        WireString::from_units(id.units()[11..].to_vec())
                    } else {
                        id.clone()
                    };
                    model.set("id", wire::str_value(id.clone()));
                    if let Some(metadata) = super::super::cline_pass::get_cline_pass_model_metadata(&id) {
                        for key in ["name", "contextWindow", "maxTokens", "input", "cost", "reasoning", "thinking"] {
                            copy_field(&mut model, key, &metadata, key);
                        }
                        if metadata.get("thinking").is_none() {
                            let mut compat = model.record("compat").unwrap_or_else(wire::empty);
                            compat.set("supportsReasoningEffort", WireValue::Bool(false));
                            model.set_record("compat", &compat);
                        } else if model.get("compat").is_none() {
                            model.set_undefined("compat");
                        }
                    } else {
                        if wire::boolean(&model, "reasoning") == Some(true) {
                            if let Some(config) = cline_thinking(raw, &model) {
                                model.set_record("thinking", &config);
                            } else {
                                model.set_undefined("thinking");
                            }
                        } else {
                            model.set_undefined("thinking");
                        }
                    }
                    Ok(vec![model])
                }))
            }
            "coreweave" => {
                d.transform_model = Some(Arc::new(|mut model, _, _| {
                    if text(
                        &classify_wire(&"coreweave".into(), &text(&model, "id").unwrap_or_else(|| "".into()), true)?,
                        "class",
                    )
                    .is_some_and(|c| c.equals_ascii("gpt-oss"))
                    {
                        model.set("reasoning", WireValue::Bool(true));
                    }
                    Ok(vec![model])
                }))
            }
            "nvidia" => d.default_context_window = Some(131072.0),
            "xai" => d.transform_model = Some(Arc::new(|m, _, _| Ok(vec![apply_xai_responses_thinking_policy(m)?]))),
            "deepseek" => {
                d.filter_model = Some(Arc::new(|id, m| {
                    let identity = classify_wire(&"deepseek".into(), id, true)?;
                    Ok(wire::boolean(m, "tool_call") == Some(true)
                        && text(&identity, "class").is_some_and(|c| c.equals_ascii("deepseek"))
                        && text(&identity, "family")
                            .is_some_and(|f| ["v4", "flash", "pro"].iter().any(|s| f.equals_ascii(s))))
                }));
                d.compat = Some(VariantSpec::from_json(
                    &serde_json::json!({"supportsDeveloperRole":false,"supportsReasoningEffort":true,"maxTokensField":"max_tokens","supportsToolChoice":false,"extraBody":{"thinking":{"type":"enabled"}},"reasoningContentField":"reasoning_content","requiresReasoningContentForToolCalls":true,"requiresAssistantContentForToolCalls":true}),
                ));
            }
            _ => {}
        }
        all.push(d);
    }
    for (key, provider, api, base) in [
        ("zai", "zai", "anthropic-messages", "https://api.z.ai/api/anthropic"),
        ("umans-ai", "umans", "anthropic-messages", "https://api.code.umans.ai"),
        ("xiaomi", "xiaomi", "openai-completions", "https://api.xiaomimimo.com/v1"),
        ("minimax-coding-plan", "minimax-code", "openai-completions", "https://api.minimax.io/v1"),
        ("minimax-cn-coding-plan", "minimax-code-cn", "openai-completions", "https://api.minimaxi.com/v1"),
        (
            "alibaba-coding-plan",
            "alibaba-coding-plan",
            "openai-completions",
            "https://coding-intl.dashscope.aliyuncs.com/v1",
        ),
        (
            "zhipuai-coding-plan",
            "zhipu-coding-plan",
            "openai-completions",
            "https://open.bigmodel.cn/api/coding/paas/v4",
        ),
    ] {
        let mut d = ModelsDevProviderDescriptor::new(key, provider, api, base);
        match provider {
            "zai" => d.resolve_api = Some(Arc::new(|id, _| Ok(Some(resolve_zai_api(id)?)))),
            "xiaomi" => {
                d.default_context_window = Some(262144.0);
                d.default_max_tokens = Some(8192.0);
                d.compat = Some(VariantSpec::from_json(
                    &serde_json::json!({"supportsStore":false,"thinkingFormat":"zai","reasoningContentField":"reasoning_content","requiresReasoningContentForToolCalls":true,"allowsSyntheticReasoningContentForToolCalls":false}),
                ));
            }
            "minimax-code" | "minimax-code-cn" => {
                d.compat = Some(VariantSpec::from_json(
                    &serde_json::json!({"supportsStore":false,"supportsDeveloperRole":false,"supportsReasoningEffort":false,"reasoningContentField":"reasoning_content"}),
                ))
            }
            "alibaba-coding-plan" => {
                d.compat = Some(VariantSpec::from_json(&serde_json::json!({"supportsDeveloperRole":false})))
            }
            "zhipu-coding-plan" => {
                d.compat = Some(VariantSpec::from_json(
                    &serde_json::json!({"thinkingFormat":"zai","reasoningContentField":"reasoning_content","supportsDeveloperRole":false}),
                ))
            }
            _ => {}
        }
        all.push(d);
    }
    for (key, provider, api, base) in [
        ("azure", "azure", "azure-openai-responses", ""),
        (
            "cloudflare-ai-gateway",
            "cloudflare-ai-gateway",
            "anthropic-messages",
            "https://gateway.ai.cloudflare.com/v1/<account>/<gateway>/anthropic",
        ),
        (
            "cloudflare-workers-ai",
            "cloudflare-ai-gateway",
            "openai-completions",
            "https://gateway.ai.cloudflare.com/v1/<account>/<gateway>/compat",
        ),
        ("mistral", "mistral", "openai-completions", "https://api.mistral.ai/v1"),
        ("opencode", "opencode-zen", "openai-completions", "https://opencode.ai/zen/v1"),
        ("opencode-go", "opencode-go", "openai-completions", "https://opencode.ai/zen/go/v1"),
        ("github-copilot", "github-copilot", "openai-completions", "https://api.githubcopilot.com"),
        ("minimax", "minimax", "anthropic-messages", "https://api.minimax.io/anthropic"),
        ("minimax-cn", "minimax-cn", "anthropic-messages", "https://api.minimaxi.com/anthropic"),
        ("huggingface", "huggingface", "openai-completions", "https://router.huggingface.co/v1"),
        ("kilo", "kilo", "openai-completions", "https://api.kilo.ai/api/gateway"),
        ("moonshotai", "moonshot", "openai-completions", "https://api.moonshot.ai/v1"),
        ("nano-gpt", "nanogpt", "openai-completions", "https://nano-gpt.com/api/v1"),
        ("synthetic", "synthetic", "openai-completions", "https://api.synthetic.new/openai/v1"),
        ("venice", "venice", "openai-completions", "https://api.venice.ai/api/v1"),
        ("ollama-cloud", "ollama-cloud", "ollama-chat", "https://ollama.com"),
        (
            "xiaomi-token-plan-ams",
            "xiaomi-token-plan-ams",
            "openai-completions",
            "https://token-plan-ams.xiaomimimo.com/v1",
        ),
        (
            "xiaomi-token-plan-cn",
            "xiaomi-token-plan-cn",
            "openai-completions",
            "https://token-plan-cn.xiaomimimo.com/v1",
        ),
        (
            "xiaomi-token-plan-sgp",
            "xiaomi-token-plan-sgp",
            "openai-completions",
            "https://token-plan-sgp.xiaomimimo.com/v1",
        ),
        ("qwen-portal", "qwen-portal", "openai-completions", "https://portal.qwen.ai/v1"),
        ("zenmux", "zenmux", "openai-completions", "https://zenmux.ai/api/v1"),
    ] {
        let mut d = ModelsDevProviderDescriptor::new(key, provider, api, base);
        match key {
            "azure" => {
                d.filter_model = Some(Arc::new(|id, m| {
                    Ok(wire::boolean(m, "tool_call") == Some(true)
                        && ["gpt-", "o1", "o3", "o4", "codex", "chatgpt"].iter().any(|s| starts(id, s)))
                }))
            }
            "cloudflare-workers-ai" => {
                d.filter_model = Some(active());
                d.transform_model = Some(Arc::new(|mut m, id, _| {
                    m.set("id", wire::str_value(super::super::cache_provider_id::join(&"workers-ai/".into(), id)));
                    Ok(vec![m])
                }));
            }
            "opencode" | "opencode-go" => {
                d.filter_model = Some(active());
                let provider = d.provider_id.clone();
                let base: WireString = if key == "opencode" {
                    "https://opencode.ai/zen".into()
                } else {
                    "https://opencode.ai/zen/go".into()
                };
                d.resolve_api =
                    Some(Arc::new(move |id, raw| Ok(Some(resolve_opencode_api(&provider, id, raw, &base)))));
            }
            "github-copilot" => {
                d.default_context_window = Some(128000.0);
                d.default_max_tokens = Some(8192.0);
                d.headers = Some(copilot_api_headers());
                d.filter_model = Some(active());
                d.resolve_api = Some(Arc::new(|id, _| {
                    let api = api_route_for(&"github-copilot".into(), id)
                        .and_then(|r| text(&r, "api"))
                        .filter(|a| a.equals_ascii("anthropic-messages") || a.equals_ascii("openai-responses"))
                        .unwrap_or_else(|| "openai-completions".into());
                    Ok(Some((api, "https://api.githubcopilot.com".into())))
                }));
                d.transform_model = Some(Arc::new(|mut m, _, _| {
                    if text(&m, "api").is_some_and(|a| a.equals_ascii("openai-completions")) {
                        m.set(
                            "compat",
                            WireValue::object(vec![
                                ("supportsStore", WireValue::Bool(false)),
                                ("supportsDeveloperRole", WireValue::Bool(false)),
                                ("supportsReasoningEffort", WireValue::Bool(false)),
                            ]),
                        );
                    }
                    Ok(vec![m])
                }));
            }
            "venice" => {
                d.transform_model = Some(Arc::new(|mut m, id, _| {
                    m.set(
                        "maxTokens",
                        clamp_kimi_k27_code_max_tokens(id, m.get("maxTokens").cloned().unwrap_or(WireValue::Null))?,
                    );
                    Ok(vec![m])
                }))
            }
            "qwen-portal" => {
                d.default_context_window = Some(128000.0);
                d.default_max_tokens = Some(8192.0);
            }
            "zenmux" => {
                d.filter_model = Some(active());
                d.resolve_api = Some(Arc::new(|id, _| {
                    Ok(Some(
                        if api_route_for(&"zenmux".into(), id)
                            .and_then(|r| text(&r, "api"))
                            .is_some_and(|a| a.equals_ascii("anthropic-messages"))
                        {
                            ("anthropic-messages".into(), "https://zenmux.ai/api/anthropic".into())
                        } else {
                            ("openai-completions".into(), "https://zenmux.ai/api/v1".into())
                        },
                    ))
                }));
            }
            _ => {}
        }
        all.push(d);
    }
    all
}
pub fn models_dev_catalog_provider_ids() -> Vec<WireString> {
    let mut out = Vec::new();
    for d in models_dev_provider_descriptors() {
        if !out.contains(&d.provider_id) {
            out.push(d.provider_id);
        }
    }
    out
}
struct CatalogFallback {
    context: CatalogContext,
    host: ProviderFactoryHost,
    explicit: bool,
    timeout: f64,
    descriptors: Vec<ModelsDevProviderDescriptor>,
}
#[async_trait]
impl crate::model_manager::ModelsDevFallback for CatalogFallback {
    async fn fetch(&self) -> Result<RawModelValue, DiscoveryError> {
        Ok(RawModelValue::SharedValue(
            super::super::catalog_session::fetch_revalidated_shared_with_timeout(
                &self.context,
                &self.host,
                self.explicit,
                self.timeout,
            )
            .await?,
        ))
    }
    fn map(&self, payload: RawModelValue, _: &WireString) -> Result<RawModelValue, DiscoveryError> {
        let value = match &payload {
            RawModelValue::Value(value) => Some(value),
            RawModelValue::SharedValue(value) => Some(value.as_ref()),
            _ => None,
        };
        let models = match value {
            Some(payload) if matches!(payload.value, WireValue::Object(_)) => {
                map_models_dev_to_models(payload, &self.descriptors)?
            }
            _ => Vec::new(),
        };
        Ok(RawModelValue::models(super::super::models_dev_policies::filter_models_dev_catalog_rows(&models)))
    }
    fn additive_only(&self) -> bool {
        true
    }
}
pub fn models_dev_catalog_fallback(
    provider: &WireString,
    context: &CatalogContext,
    host: &ProviderFactoryHost,
    explicit: bool,
    timeout: Option<f64>,
) -> Option<Arc<dyn crate::model_manager::ModelsDevFallback>> {
    let descriptors: Vec<_> =
        models_dev_provider_descriptors().into_iter().filter(|d| d.provider_id == *provider).collect();
    if descriptors.is_empty() {
        return None;
    }
    Some(Arc::new(CatalogFallback {
        context: context.clone(),
        host: host.clone(),
        explicit,
        timeout: timeout.unwrap_or(10000.0),
        descriptors,
    }))
}
