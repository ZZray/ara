//! Fixed OMP ClinePass authoritative subscription/free roster and optional live enrichment.
use super::*;
use crate::catalog_discovery::{DiscoveryRequest, zero_cost};
const CLINE_BASE: &str = "https://api.cline.bot/api/v1";
struct LiveCatalog {
    by_id: HashMap<WireString, VariantSpec>,
    by_slug: HashMap<WireString, VariantSpec>,
}
fn slug(id: &WireString) -> WireString {
    let at = id.units().iter().rposition(|c| *c == 47).map_or(0, |i| i + 1);
    WireString::from_units(id.units()[at..].to_vec())
}
pub fn cline_pass_client_headers() -> Vec<(WireString, WireString)> {
    [
        ("HTTP-Referer", "https://cline.bot"),
        ("X-Title", "Cline"),
        ("X-IS-MULTIROOT", "false"),
        ("X-CLIENT-TYPE", "cline-sdk"),
        ("User-Agent", "Cline/3.0.58"),
        ("X-CLIENT-VERSION", "3.0.58"),
        (
            "X-PLATFORM",
            if cfg!(target_os = "windows") {
                "win32"
            } else if cfg!(target_os = "macos") {
                "darwin"
            } else {
                "linux"
            },
        ),
        ("X-PLATFORM-VERSION", "3.0.54"),
        ("X-CORE-VERSION", "0.0.79"),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect()
}
fn fallback(id: &WireString) -> VariantSpec {
    VariantSpec::from_wire(WireValue::object(vec![
        ("id", wire::str_value(id.clone())),
        ("name", wire::str_value(id.clone())),
        ("api", wire::str_value("openai-completions")),
        ("provider", wire::str_value("cline-pass")),
        ("baseUrl", wire::str_value(CLINE_BASE)),
        ("reasoning", WireValue::Bool(true)),
        ("input", input(None)),
        ("cost", zero_cost()),
        ("contextWindow", WireValue::Number(128000.0)),
        ("maxTokens", WireValue::Number(8192.0)),
        ("compat", WireValue::object(vec![("supportsReasoningEffort", WireValue::Bool(false))])),
    ]))
}
fn curated(id: &WireString, tier: &str) -> Option<VariantSpec> {
    let meta = super::super::cline_pass::get_cline_pass_model_metadata(id)?;
    if !text(&meta, "tier").is_some_and(|s| s.equals_ascii(tier)) {
        return None;
    }
    let mut m = fallback(id);
    for key in ["name", "contextWindow", "maxTokens", "input", "cost", "reasoning", "thinking"] {
        copy_field(&mut m, key, &meta, key);
    }
    let mut compat = wire::empty();
    if meta.get("thinking").is_none() {
        compat.set("supportsReasoningEffort", WireValue::Bool(false));
    }
    if tier == "free" {
        compat.set("wireModelIdMode", wire::str_value("raw"));
    }
    m.set_record("compat", &compat);
    Some(m)
}
async fn live_catalog(ctx: &CatalogContext) -> Option<LiveCatalog> {
    let response = timed_fetch(
        ctx,
        DiscoveryRequest { url: "https://openrouter.ai/api/v1/models".into(), ..Default::default() },
        5000.0,
    )
    .await
    .ok()
    .filter(|r| r.ok())?;
    let payload = response.json().ok()?;
    if !matches!(payload.value, WireValue::Object(_)) {
        return None;
    }
    let entries = payload.get("data")?.as_array()?;
    let mut by_id = HashMap::new();
    let mut by_slug = HashMap::new();
    for raw in entries {
        if !matches!(raw, WireValue::Object(_)) {
            continue;
        }
        let raw = VariantSpec::from_wire(raw.clone());
        let Some(id) = text(&raw, "id").filter(|s| !s.is_empty()) else {
            continue;
        };
        let mut e = wire::empty();
        if let Some(name) = text(&raw, "name").filter(|s| !s.is_empty()) {
            e.set("name", wire::str_value(remove_regex_match(&name, r"^[^:]+:\s*", "")));
        }
        let top = raw.record("top_provider");
        for (key, v) in [
            ("contextWindow", raw.get("context_length")),
            ("maxTokens", top.as_ref().and_then(|p| p.get("max_completion_tokens"))),
        ] {
            if let Some(n) = to_number(v).filter(|n| *n > 0.0) {
                e.set(key, WireValue::Number(n));
            }
        }
        if let Some(price) = raw.record("pricing").filter(|p| matches!(p.value, WireValue::Object(_))) {
            e.set(
                "cost",
                WireValue::object(
                    [
                        ("input", "prompt"),
                        ("output", "completion"),
                        ("cacheRead", "input_cache_read"),
                        ("cacheWrite", "input_cache_write"),
                    ]
                    .iter()
                    .map(|(k, v)| (*k, WireValue::Number(parse_float(price.get(v)) * 1e6)))
                    .collect(),
                ),
            );
        }
        if let Some(params) = raw.get("supported_parameters").and_then(WireValue::as_array) {
            e.set(
                "reasoning",
                WireValue::Bool(params.iter().any(|s| s.as_string().is_some_and(|s| s.equals_ascii("reasoning")))),
            );
        }
        if let Some(architecture) = raw.record("architecture").filter(|a| matches!(a.value, WireValue::Object(_))) {
            let modality = architecture.get("modality").filter(|v| !matches!(v, WireValue::Null));
            let modality = modality.map(js_value_string).unwrap_or_else(|| "".into());
            if !modality.is_empty() {
                e.set(
                    "input",
                    if includes(&modality, "image") {
                        input(Some(&WireValue::Array(vec![wire::str_value("image")])))
                    } else {
                        input(None)
                    },
                );
            }
        }
        let last = slug(&id);
        if !last.is_empty() {
            by_slug.insert(last, e.clone());
        }
        by_id.insert(id, e);
    }
    if by_id.is_empty() { None } else { Some(LiveCatalog { by_id, by_slug }) }
}
fn upstream(id: &WireString) -> Option<SpecRef> {
    wire::resolve_model_reference(id, wire::bundled_model_reference_index())
        .filter(|r| !text(r, "provider").is_some_and(|p| p.equals_ascii("cline-pass")))
}
fn subscription(id: &WireString, refs: &HashMap<WireString, SpecRef>, live: Option<&LiveCatalog>) -> VariantSpec {
    if let Some(m) = curated(id, "subscription") {
        return m;
    }
    let known = refs.get(id);
    let mut base = known.map(|m| (**m).clone()).unwrap_or_else(|| fallback(id));
    if let Some(r) = upstream(id) {
        let placeholder = base.get("contextWindow").and_then(WireValue::as_number) == Some(128000.0)
            && base.get("maxTokens").and_then(WireValue::as_number) == Some(8192.0);
        if known.is_some() && !placeholder {
            copy_field(&mut base, "cost", &r, "cost");
            return base;
        }
        if known.is_none() {
            copy_field(&mut base, "name", &r, "name");
        }
        for k in ["reasoning", "input", "cost", "contextWindow", "maxTokens"] {
            copy_field(&mut base, k, &r, k);
        }
        if !r.get("reasoning").is_some_and(crate::model_collapse::truthy) {
            base.set_undefined("thinking");
        }
        return base;
    }
    if let Some(live) = live.and_then(|l| l.by_slug.get(id)) {
        for k in ["name", "contextWindow", "maxTokens", "input"] {
            if live.get(k).is_some_and(crate::model_collapse::truthy) {
                copy_field(&mut base, k, live, k);
            }
        }
        if live.get("reasoning").is_some() {
            copy_field(&mut base, "reasoning", live, "reasoning");
        }
        if live.record("cost").is_some_and(|c| {
            ["input", "output"].iter().any(|k| c.get(k).and_then(WireValue::as_number).is_some_and(|n| n > 0.0))
        }) {
            copy_field(&mut base, "cost", live, "cost");
        }
        if wire::boolean(live, "reasoning") == Some(false) {
            base.set_undefined("thinking");
        }
    }
    base
}
fn free_model(id: &WireString, live: Option<&LiveCatalog>) -> VariantSpec {
    if let Some(m) = curated(id, "free") {
        return m;
    }
    let up = upstream(id);
    let slug = slug(id);
    let live = live.and_then(|l| l.by_id.get(id).or_else(|| l.by_slug.get(&slug)));
    let mut m = fallback(id);
    let mut display =
        up.as_ref().and_then(|r| text(r, "name")).or_else(|| live.and_then(|r| text(r, "name"))).unwrap_or(slug);
    for pattern in [r"\s*\(free\)\s*$", r":free$"] {
        display = remove_regex_match(&display, pattern, "i");
    }
    display = crate::catalog_discovery::js_trim(&display);
    if display.is_empty() {
        display = id.clone();
    }
    m.set("name", wire::str_value(append(&display, " (free)")));
    for k in ["reasoning", "input", "contextWindow", "maxTokens"] {
        if let Some(value) = up
            .as_ref()
            .and_then(|r| r.get(k))
            .filter(|v| !matches!(v, WireValue::Null))
            .or_else(|| live.and_then(|r| r.get(k)).filter(|v| !matches!(v, WireValue::Null)))
        {
            m.set(k, value.clone());
        }
    }
    if !m.get("reasoning").is_some_and(crate::model_collapse::truthy) {
        m.set_undefined("thinking");
    }
    m.set(
        "compat",
        WireValue::object(vec![
            ("supportsReasoningEffort", WireValue::Bool(false)),
            ("wireModelIdMode", wire::str_value("raw")),
        ]),
    );
    m
}
pub fn cline_pass_model_manager_options(default: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let ctx = context(default, &config);
    let refs = Arc::new(wire::create_bundled_reference_map(&"cline-pass".into()));
    let mut options = ModelManagerOptions::new("cline-pass");
    options.dynamic_models_authoritative = true;
    options.dynamic_fetcher = Some(dynamic(move || {
        let ctx = ctx.clone();
        let refs = refs.clone();
        async move {
            let roster_ctx = ctx.clone();
            let response = eager_background(async move {
                timed_fetch(
                    &roster_ctx,
                    DiscoveryRequest {
                        url: format!("{CLINE_BASE}/ai/cline/recommended-models").into(),
                        headers: cline_pass_client_headers(),
                        ..Default::default()
                    },
                    5000.0,
                )
                .await
            })
            .await;
            let live_ctx = ctx.clone();
            let live = eager_background(async move { live_catalog(&live_ctx).await }).await;
            let response = response.await.map_err(|e| DiscoveryError::new(e.to_string()))??;
            let live = live.await.ok().flatten();
            if !response.ok() {
                return Err(DiscoveryError::new(format!("Failed to fetch ClinePass models: HTTP {}", response.status)));
            }
            let payload = response.json()?;
            if !matches!(payload.value, WireValue::Object(_))
                || payload.get("clinePass").and_then(WireValue::as_array).is_none()
            {
                return Err(DiscoveryError::new("ClinePass model catalog response is missing clinePass"));
            }
            let mut models: Vec<SpecRef> = Vec::new();
            for e in payload.get("clinePass").and_then(WireValue::as_array).expect("validated roster") {
                if !matches!(e, WireValue::Object(_)) {
                    continue;
                }
                let e = VariantSpec::from_wire(e.clone());
                let Some(id) =
                    text(&e, "id").map(|s| crate::catalog_discovery::js_trim(&s)).filter(|s| starts(s, "cline-pass/"))
                else {
                    continue;
                };
                let id = crate::catalog_discovery::js_trim(&WireString::from_units(id.units()[11..].to_vec()));
                if id.is_empty() {
                    continue;
                }
                let model = Arc::new(subscription(&id, &refs, live.as_ref()));
                if let Some(at) = models.iter().position(|m| text(m, "id") == Some(id.clone())) {
                    models[at] = model;
                } else {
                    models.push(model);
                }
            }
            if models.is_empty() {
                return Err(DiscoveryError::new("ClinePass model catalog contains no valid model IDs"));
            }
            for e in payload.get("free").and_then(WireValue::as_array).unwrap_or(&[]) {
                if !matches!(e, WireValue::Object(_)) {
                    continue;
                }
                let e = VariantSpec::from_wire(e.clone());
                let Some(id) = text(&e, "id")
                    .map(|s| crate::catalog_discovery::js_trim(&s))
                    .filter(|s| !s.is_empty() && !starts(s, "cline-pass/"))
                else {
                    continue;
                };
                if models.iter().any(|m| text(m, "id") == Some(id.clone())) {
                    continue;
                }
                models.push(Arc::new(free_model(&id, live.as_ref())));
            }
            Ok(RawModelValue::models(models))
        }
    }));
    options
}
