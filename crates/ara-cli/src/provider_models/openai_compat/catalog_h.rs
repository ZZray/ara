//! Fixed OMP LiteLLM rich endpoint membership, metadata merge, and fallback.
use super::super::{
    behavior::likely_responses_id,
    cache_provider_id::{get_default_model_discovery_base_url, join, resolve_model_cache_provider_id},
};
use super::*;
use crate::{
    catalog_discovery::{DiscoveryRequest, DiscoverySignal, zero_cost},
    model_collapse::truthy,
};
use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
};
pub const OPENAI_COMPAT_DISCOVERY_DEFAULT_CONTEXT_WINDOW: f64 = 128000.0;
pub const OPENAI_COMPAT_DISCOVERY_DEFAULT_MAX_TOKENS: f64 = 32768.0;
pub type LiteLLMReferenceResolver = Arc<dyn Fn(&WireString) -> Option<SpecRef> + Send + Sync>;
pub type LiteLLMApiResolver =
    Arc<dyn Fn(&VariantSpec, &WireString) -> Result<Option<WireString>, DiscoveryError> + Send + Sync>;
#[derive(Clone)]
pub struct FetchLiteLLMRichModelsOptions {
    pub api: WireString,
    pub provider: WireString,
    pub base_url: WireString,
    pub api_key: Option<WireString>,
    pub headers: Vec<(WireString, WireString)>,
    pub signal: Option<DiscoverySignal>,
    pub timeout_ms: Option<f64>,
    pub reference_resolver: Option<LiteLLMReferenceResolver>,
    pub resolve_api: Option<LiteLLMApiResolver>,
}
impl FetchLiteLLMRichModelsOptions {
    pub fn new(api: impl Into<WireString>, provider: impl Into<WireString>, base_url: impl Into<WireString>) -> Self {
        Self {
            api: api.into(),
            provider: provider.into(),
            base_url: base_url.into(),
            api_key: None,
            headers: Vec::new(),
            signal: None,
            timeout_ms: None,
            reference_resolver: None,
            resolve_api: None,
        }
    }
}
pub fn normalize_litellm_management_base_url(base: &WireString) -> WireString {
    let trimmed = crate::catalog_discovery::trim_trailing_slashes(&crate::catalog_discovery::js_trim(base));
    if trimmed.is_empty() {
        return trimmed;
    }
    if let Ok(mut url) = reqwest::Url::parse(&String::from_utf16_lossy(trimmed.units())) {
        let path = url.path().trim_end_matches('/');
        let path = path.strip_suffix("/v1").unwrap_or(path).to_owned();
        url.set_path(if path.is_empty() { "/" } else { &path });
        let host = url.host_str().unwrap_or("");
        let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_owned() };
        let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
        return crate::catalog_discovery::trim_trailing_slashes(
            &format!("{}://{host}{port}{}", url.scheme(), url.path()).into(),
        );
    }
    if ends(&trimmed, "/v1") { trimmed.slice_prefix(trimmed.len() - 3) } else { trimmed }
}
fn nonempty(value: Option<&WireValue>) -> Option<WireString> {
    value.and_then(WireValue::as_string).map(crate::catalog_discovery::js_trim).filter(|s| !s.is_empty())
}
fn metadata(entry: &VariantSpec, key: &str) -> Option<WireValue> {
    entry.get(key).filter(|v| !matches!(v, WireValue::Null)).cloned().or_else(|| {
        entry.record("model_info").filter(|e| matches!(e.value, WireValue::Object(_))).and_then(|e| e.get(key).cloned())
    })
}
fn params(entry: &VariantSpec) -> Option<VariantSpec> {
    entry.record("litellm_params").filter(|e| matches!(e.value, WireValue::Object(_)))
}
fn supported_params(entry: &VariantSpec) -> Option<Vec<WireString>> {
    metadata(entry, "supported_openai_params")?
        .as_array()
        .map(|a| a.iter().filter_map(WireValue::as_string).cloned().collect())
}
fn providers(entry: &VariantSpec) -> Option<Vec<WireString>> {
    let values = entry.get("providers")?.as_array()?;
    let values: Vec<_> = values.iter().filter_map(|v| nonempty(Some(v))).map(|v| lower(&v)).collect();
    (!values.is_empty()).then_some(values)
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Route {
    Openai,
    Other,
    Unknown,
}
fn route(entry: Option<&VariantSpec>, id: &WireString) -> Route {
    if let Some(entry) = entry {
        if let Some(values) = providers(entry) {
            return if values.iter().all(|s| s.equals_ascii("openai")) { Route::Openai } else { Route::Other };
        }
        let params = params(entry);
        if let Some(p) = params.as_ref().and_then(|p| nonempty(p.get("custom_llm_provider"))).map(|s| lower(&s)) {
            return if p.equals_ascii("openai") { Route::Openai } else { Route::Other };
        }
        for model in
            [params.as_ref().and_then(|p| nonempty(p.get("model"))), nonempty(metadata(entry, "base_model").as_ref())]
        {
            if let Some(model) = model
                && let Some(at) = model.units().iter().position(|c| *c == 47).filter(|at| *at > 0)
            {
                return if lower(&model.slice_prefix(at)).equals_ascii("openai") {
                    Route::Openai
                } else {
                    Route::Other
                };
            }
        }
    }
    let model =
        if starts(&lower(id), "openai/") { WireString::from_units(id.units()[7..].to_vec()) } else { id.clone() };
    let model = crate::catalog_discovery::js_trim(&model);
    if !model.is_empty() && likely_responses_id(&model) { Route::Openai } else { Route::Unknown }
}
pub fn resolve_litellm_api(entry: Option<&VariantSpec>, id: &WireString, fallback: Option<&WireString>) -> WireString {
    if route(entry, id) == Route::Openai {
        "openai-responses".into()
    } else {
        fallback.cloned().unwrap_or_else(|| "openai-completions".into())
    }
}
fn sentinel(id: &WireString) -> bool {
    ["all-team-models", "all-proxy-models", "no-default-models"].iter().any(|s| id.equals_ascii(s))
}
fn unusable(entry: &VariantSpec) -> bool {
    if !["model_group", "id"].iter().any(|k| nonempty(entry.get(k)).is_some_and(|s| sentinel(&s))) {
        return false;
    }
    if entry.get("providers").is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty())) {
        return false;
    }
    if [
        nonempty(entry.get("model_name")),
        nonempty(entry.get("id")),
        params(entry).and_then(|p| nonempty(p.get("model"))),
    ]
    .iter()
    .flatten()
    .any(|s| !sentinel(s))
    {
        return false;
    }
    if ["max_input_tokens", "max_output_tokens"]
        .iter()
        .any(|k| to_number(metadata(entry, k).as_ref()).is_some_and(|n| n > 0.0))
    {
        return false;
    }
    if ["supports_vision", "supports_reasoning", "supports_function_calling", "supports_tools"]
        .iter()
        .any(|k| matches!(metadata(entry, k), Some(WireValue::Bool(true))))
    {
        return false;
    }
    !supported_params(entry).is_some_and(|p| !p.is_empty())
}
fn reported_cost(entry: &VariantSpec) -> VariantSpec {
    let mut out = wire::empty();
    for (k, v) in [
        ("input", "input_cost_per_token"),
        ("output", "output_cost_per_token"),
        ("cacheRead", "cache_read_input_token_cost"),
        ("cacheWrite", "cache_creation_input_token_cost"),
    ] {
        if let Some(n) = to_number(metadata(entry, v).as_ref()).filter(|n| *n > 0.0) {
            out.set(k, WireValue::Number(n * 1e6));
        }
    }
    out
}
fn cost(entry: &VariantSpec) -> Option<VariantSpec> {
    let reported = reported_cost(entry);
    if reported.get("input").is_none() && reported.get("output").is_none() {
        return None;
    }
    let mut out = VariantSpec::from_wire(zero_cost());
    for k in ["input", "output", "cacheRead", "cacheWrite"] {
        if reported.get(k).is_some() {
            copy_field(&mut out, k, &reported, k);
        }
    }
    Some(out)
}
fn extract(value: &WireValue) -> Option<Vec<VariantSpec>> {
    if let Some(a) = value.as_array() {
        return Some(
            a.iter().filter(|e| matches!(e, WireValue::Object(_))).cloned().map(VariantSpec::from_wire).collect(),
        );
    }
    if !matches!(value, WireValue::Object(_)) {
        return None;
    }
    for k in ["data", "models", "result", "items"] {
        if let Some(value) = value.get(k)
            && let Some(entries) = extract(value)
        {
            return Some(entries);
        }
    }
    None
}
fn display_name(entry: Option<WireString>, reference: Option<WireString>, id: &WireString) -> WireString {
    let clean = |s: &WireString| {
        let value = crate::catalog_discovery::js_trim(&remove_regex_match(s, r"\s+\(\d+(?:\.\d+)?[x×] usage\)$", "i"));
        if value.is_empty() { s.clone() } else { value }
    };
    let entry = entry.filter(|s| !s.is_empty()).map(|s| clean(&s));
    if let Some(entry) = entry.filter(|s| s != id) {
        return entry;
    }
    reference.filter(|s| !s.is_empty()).map(|s| clean(&s)).unwrap_or_else(|| id.clone())
}
#[derive(Clone)]
struct Rich {
    model: VariantSpec,
    route: Route,
    vision: Option<WireValue>,
    reasoning: Option<WireValue>,
    context: bool,
    max: bool,
    tools: bool,
    openai_params: bool,
    reported: VariantSpec,
}
fn map_entry(
    entry: &VariantSpec,
    options: &FetchLiteLLMRichModelsOptions,
    base: &WireString,
) -> Result<Option<Rich>, DiscoveryError> {
    if unusable(entry) {
        return Ok(None);
    }
    let Some(id) = ["model_group", "model_name", "id"]
        .iter()
        .find_map(|k| nonempty(entry.get(k)))
        .or_else(|| params(entry).and_then(|p| nonempty(p.get("model"))))
    else {
        return Ok(None);
    };
    let r = options.reference_resolver.as_ref().and_then(|f| f(&id));
    let window = positive(
        metadata(entry, "max_input_tokens").as_ref(),
        r.as_ref()
            .and_then(|r| r.get("contextWindow"))
            .filter(|v| !matches!(v, WireValue::Null))
            .or(Some(&WireValue::Number(128000.0))),
    );
    let cap = positive(
        metadata(entry, "max_output_tokens").as_ref(),
        r.as_ref()
            .and_then(|r| r.get("maxTokens"))
            .filter(|v| !matches!(v, WireValue::Null))
            .or(Some(&WireValue::Number(window.as_number().unwrap_or(f64::NAN).min(32768.0)))),
    );
    let vision = metadata(entry, "supports_vision");
    let reasoning = metadata(entry, "supports_reasoning");
    let function = metadata(entry, "supports_function_calling");
    let ps = supported_params(entry);
    let tools = match function.as_ref() {
        Some(WireValue::Bool(b)) => Some(WireValue::Bool(*b)),
        _ => {
            ps.as_ref()
                .map(|p| {
                    WireValue::Bool(p.iter().any(|s| {
                        ["tools", "tool_choice", "functions", "function_call"].iter().any(|k| s.equals_ascii(k))
                    }))
                })
                .or_else(|| r.as_ref().and_then(|r| r.get("supportsTools")).cloned())
        }
    };
    let mut compat = VariantSpec::from_wire(WireValue::object(vec![
        ("supportsStore", WireValue::Bool(false)),
        ("supportsDeveloperRole", WireValue::Bool(false)),
    ]));
    let rc = r.as_ref().and_then(|r| r.record("compat"));
    if let Some(ps) = &ps {
        compat.set("supportsReasoningEffort", WireValue::Bool(ps.iter().any(|s| s.equals_ascii("reasoning_effort"))));
    } else if let Some(value) = rc.as_ref().and_then(|r| r.get("supportsReasoningEffort")) {
        compat.set("supportsReasoningEffort", value.clone());
    }
    if let Some(value) = rc.as_ref().and_then(|r| r.get("reasoningEffortMap")).filter(|v| truthy(v)) {
        compat.set("reasoningEffortMap", value.clone());
    }
    if let Some(value) = rc.as_ref().and_then(|r| r.get("omitReasoningEffort")) {
        compat.set("omitReasoningEffort", value.clone());
    }
    let api = options
        .resolve_api
        .as_ref()
        .map(|f| f(entry, &id))
        .transpose()?
        .flatten()
        .unwrap_or_else(|| options.api.clone());
    let mut m = VariantSpec::from_wire(WireValue::object(vec![
        ("id", wire::str_value(id.clone())),
        (
            "name",
            wire::str_value(display_name(
                nonempty(entry.get("model_name")),
                r.as_ref().and_then(|r| text(r, "name")),
                &id,
            )),
        ),
        ("api", wire::str_value(api)),
        ("provider", wire::str_value(options.provider.clone())),
        ("baseUrl", wire::str_value(base.clone())),
        ("contextWindow", window),
        ("maxTokens", cap),
        (
            "input",
            match &vision {
                Some(WireValue::Bool(true)) => input(Some(&WireValue::Array(vec![wire::str_value("image")]))),
                Some(WireValue::Bool(false)) => input(None),
                _ => r
                    .as_ref()
                    .and_then(|r| r.get("input"))
                    .filter(|v| !matches!(v, WireValue::Null))
                    .cloned()
                    .unwrap_or_else(|| input(None)),
            },
        ),
        (
            "reasoning",
            match &reasoning {
                Some(WireValue::Bool(b)) => WireValue::Bool(*b),
                _ => r
                    .as_ref()
                    .and_then(|r| r.get("reasoning"))
                    .filter(|v| !matches!(v, WireValue::Null))
                    .cloned()
                    .unwrap_or(WireValue::Bool(false)),
            },
        ),
    ]));
    if let Some(value) = r.as_ref().and_then(|r| r.get("thinking")) {
        m.set("thinking", value.clone());
    } else {
        m.set_undefined("thinking");
    }
    if let Some(c) = cost(entry) {
        m.set_record("cost", &c);
    } else {
        m.set(
            "cost",
            r.as_ref()
                .and_then(|r| r.get("cost"))
                .filter(|v| !matches!(v, WireValue::Null))
                .cloned()
                .unwrap_or_else(zero_cost),
        );
    }
    if let Some(tools) = tools {
        m.set("supportsTools", tools);
    }
    m.set_record("compat", &compat);
    Ok(Some(Rich {
        model: m,
        route: route(Some(entry), &id),
        vision,
        reasoning,
        context: to_number(metadata(entry, "max_input_tokens").as_ref()).is_some_and(|n| n > 0.0),
        max: to_number(metadata(entry, "max_output_tokens").as_ref()).is_some_and(|n| n > 0.0),
        tools: matches!(function, Some(WireValue::Bool(_))) || ps.is_some(),
        openai_params: ps.is_some(),
        reported: reported_cost(entry),
    }))
}
fn merge(old: Rich, next: Rich) -> Rich {
    let route = if old.route == Route::Other || next.route == Route::Other {
        Route::Other
    } else if old.route == Route::Openai || next.route == Route::Openai {
        Route::Openai
    } else {
        Route::Unknown
    };
    let mut m = old.model.clone();
    if next.route == route {
        copy_field(&mut m, "api", &next.model, "api");
    }
    if text(&next.model, "name") != text(&next.model, "id") {
        copy_field(&mut m, "name", &next.model, "name");
    }
    for (k, enabled) in [
        ("contextWindow", next.context),
        ("maxTokens", next.max),
        ("input", matches!(next.vision, Some(WireValue::Bool(_)))),
        ("reasoning", matches!(next.reasoning, Some(WireValue::Bool(_)))),
        ("compat", next.openai_params),
        ("supportsTools", next.tools),
    ] {
        if enabled {
            copy_field(&mut m, k, &next.model, k);
        }
    }
    let mut cost = m.record("cost").unwrap_or_else(|| VariantSpec::from_wire(zero_cost()));
    for r in [&old.reported, &next.reported] {
        for k in ["input", "output", "cacheRead", "cacheWrite"] {
            if r.get(k).is_some() {
                copy_field(&mut cost, k, r, k);
            }
        }
    }
    m.set_record("cost", &cost);
    Rich {
        model: m,
        route,
        vision: next.vision,
        reasoning: next.reasoning,
        context: old.context || next.context,
        max: old.max || next.max,
        tools: old.tools || next.tools,
        openai_params: old.openai_params || next.openai_params,
        reported: wire::spread(&[&old.reported, &next.reported]),
    }
}
struct Failure {
    endpoint: &'static str,
    reason: &'static str,
    status: Option<u16>,
    error: Option<DiscoveryError>,
}
enum Endpoint {
    Models(Vec<Rich>),
    Failure(Failure),
    Empty,
}
async fn endpoint(
    ctx: &CatalogContext,
    name: &'static str,
    options: &FetchLiteLLMRichModelsOptions,
    management: &WireString,
    runtime: &WireString,
    signal: Option<DiscoverySignal>,
) -> Result<Endpoint, DiscoveryError> {
    let mut headers = vec![("Accept".into(), "application/json".into())];
    for (k, v) in &options.headers {
        crate::catalog_discovery::openai::set_record_header(&mut headers, k.clone(), v.clone());
    }
    if let Some(key) = options.api_key.as_ref().filter(|s| !s.is_empty()) {
        crate::catalog_discovery::openai::set_record_header(
            &mut headers,
            "Authorization".into(),
            join(&"Bearer ".into(), key),
        );
    }
    let reply = ctx
        .transport
        .fetch(DiscoveryRequest { url: append(management, name), headers, signal, ..Default::default() })
        .await;
    let response = match reply {
        Ok(r) => r,
        Err(error) => {
            return Ok(Endpoint::Failure(Failure {
                endpoint: name,
                reason: "network-error",
                status: None,
                error: Some(error),
            }));
        }
    };
    if !response.ok() {
        return Ok(if response.status == 404 {
            Endpoint::Empty
        } else {
            Endpoint::Failure(Failure {
                endpoint: name,
                reason: "http-status",
                status: Some(response.status),
                error: None,
            })
        });
    }
    let payload = match response.json() {
        Ok(p) => p,
        Err(error) => {
            return Ok(Endpoint::Failure(Failure {
                endpoint: name,
                reason: "invalid-json",
                status: Some(response.status),
                error: Some(error),
            }));
        }
    };
    let Some(entries) = extract(&payload.value) else {
        return Ok(Endpoint::Empty);
    };
    let mut rows: Vec<Rich> = Vec::new();
    for e in entries {
        if let Some(next) = map_entry(&e, options, runtime)? {
            if let Some(at) = rows.iter().position(|r| text(&r.model, "id") == text(&next.model, "id")) {
                rows[at] = merge(rows[at].clone(), next);
            } else {
                rows.push(next);
            }
        }
    }
    rows.sort_by(|a, b| {
        ctx.locale.compare(
            &text(&a.model, "id").unwrap_or_else(|| "".into()),
            &text(&b.model, "id").unwrap_or_else(|| "".into()),
        )
    });
    Ok(if rows.is_empty() { Endpoint::Empty } else { Endpoint::Models(rows) })
}
fn warn(ctx: &CatalogContext, base: &WireString, failure: &Failure) {
    type WarningStates = Vec<(std::sync::Weak<crate::catalog_discovery::DiscoveryRuntime>, HashSet<WireString>)>;
    static WARNED: OnceLock<Mutex<WarningStates>> = OnceLock::new();
    {
        let mut states = WARNED.get_or_init(|| Mutex::new(Vec::new())).lock().expect("LiteLLM warn once");
        states.retain(|(runtime, _)| runtime.strong_count() > 0);
        let index =
            states.iter().position(|(runtime, _)| runtime.ptr_eq(&Arc::downgrade(&ctx.runtime))).unwrap_or_else(|| {
                states.push((Arc::downgrade(&ctx.runtime), HashSet::new()));
                states.len() - 1
            });
        if !states[index].1.insert(base.clone()) {
            return;
        }
    }
    let mut data = VariantSpec::from_wire(WireValue::object(vec![
        ("endpoint", wire::str_value(append(base, failure.endpoint))),
        (
            "status",
            failure.status.map(|s| WireValue::Number(f64::from(s))).unwrap_or_else(|| wire::str_value("unavailable")),
        ),
        ("reason", wire::str_value(failure.reason)),
    ]));
    if failure.status == Some(403) {
        data.set(
            "requiredPermission",
            wire::str_value("Grant this LiteLLM key access to the model metadata endpoints"),
        );
    }
    if let Some(error) = &failure.error {
        data.set(
            "error",
            if failure.reason == "network-error" {
                WireValue::object(vec![("name", wire::str_value(error.name.clone()))])
            } else {
                WireValue::Object(Vec::new())
            },
        );
    }
    ctx.warn("LiteLLM rich model metadata unavailable; falling back to /v1/models", data)
}
async fn fetch_internal(
    ctx: &CatalogContext,
    options: &FetchLiteLLMRichModelsOptions,
    signal: Option<DiscoverySignal>,
) -> Result<Option<Vec<SpecRef>>, DiscoveryError> {
    let management = normalize_litellm_management_base_url(&options.base_url);
    let runtime = trim_one_slash(&crate::catalog_discovery::js_trim(&options.base_url));
    if management.is_empty() || runtime.is_empty() {
        return Ok(None);
    }
    let mut rows: Vec<Rich> = Vec::new();
    let mut failure: Option<Failure> = None;
    for name in ["/model_group/info", "/v2/model/info", "/model/info", "/v1/model/info"] {
        match endpoint(ctx, name, options, &management, &runtime, signal.clone()).await? {
            Endpoint::Empty => continue,
            Endpoint::Failure(next) => {
                if next.status != Some(401)
                    && (failure.is_none()
                        || failure.as_ref().is_some_and(|f| f.status != Some(403)) && next.status == Some(403))
                {
                    failure = Some(next);
                }
                continue;
            }
            Endpoint::Models(next) => {
                let had_prior = !rows.is_empty();
                for next in next {
                    if let Some(at) = rows.iter().position(|r| text(&r.model, "id") == text(&next.model, "id")) {
                        rows[at] = merge(rows[at].clone(), next);
                    } else if !had_prior {
                        rows.push(next);
                    }
                }
            }
        }
        let need = rows.iter().any(|r| {
            !matches!(r.vision, Some(WireValue::Bool(_)))
                || options.resolve_api.is_some() && r.route == Route::Unknown
                || !r.reported.own_keys().is_empty()
                    && ["input", "output", "cacheRead", "cacheWrite"].iter().any(|k| r.reported.get(k).is_none())
        });
        if !need {
            break;
        }
    }
    if rows.is_empty() {
        if let Some(failure) = failure {
            warn(ctx, &management, &failure);
        }
        return Ok(None);
    }
    let mut out: Vec<_> = rows.into_iter().map(|r| Arc::new(r.model)).collect();
    ctx.sort_by_id(&mut out);
    Ok(Some(out))
}
pub async fn fetch_litellm_rich_models(
    ctx: &CatalogContext,
    options: &FetchLiteLLMRichModelsOptions,
) -> Result<Option<Vec<SpecRef>>, DiscoveryError> {
    if let Some(signal) = &options.signal {
        return fetch_internal(ctx, options, Some(signal.clone())).await;
    }
    if let Some(timeout) = options.timeout_ms {
        let signal = DiscoverySignal::default();
        let timer = start_discovery_timeout(&signal, timeout);
        let result = fetch_internal(ctx, options, Some(signal)).await;
        drop(timer);
        result
    } else {
        fetch_internal(ctx, options, None).await
    }
}
pub fn litellm_model_manager_options(
    default: &CatalogContext,
    config: ModelManagerConfig,
    host: &ProviderFactoryHost,
) -> ModelManagerOptions {
    let ctx = context(default, &config);
    let explicit = config.context.is_some();
    let base = config.base_url.clone().unwrap_or_else(|| {
        get_default_model_discovery_base_url(&"litellm".into(), host.environment.as_ref()).expect("LiteLLM default")
    });
    let host = host.clone();
    let mut options = ModelManagerOptions::new("litellm");
    options.cache_provider_id =
        Some(resolve_model_cache_provider_id(&"litellm".into(), None, Some(&base), host.environment.as_ref()));
    options.dynamic_fetcher = Some(dynamic(move || {
        let ctx = ctx.clone();
        let base = base.clone();
        let host = host.clone();
        let key = config.api_key.clone();
        async move {
            let mut refs: HashMap<WireString, SpecRef> = HashMap::new();
            if let Ok(payload) = fetch_well_known_models(&ctx, &host, explicit, None).await {
                for m in map_models_dev_to_models(&payload, &models_dev_provider_descriptors()).unwrap_or_default() {
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
            }
            let resolver = Arc::new(wire::ReferenceResolver::new(refs));
            let rr = resolver.clone();
            let mut rich = FetchLiteLLMRichModelsOptions::new("openai-completions", "litellm", base.clone());
            rich.api_key = key.clone();
            rich.timeout_ms = Some(10000.0);
            rich.reference_resolver = Some(Arc::new(move |id| rr.resolve(id)));
            rich.resolve_api = Some(Arc::new(|e, id| Ok(Some(resolve_litellm_api(Some(e), id, None)))));
            if let Some(models) = fetch_litellm_rich_models(&ctx, &rich).await?.filter(|m| !m.is_empty()) {
                return Ok(RawModelValue::models(models));
            }
            let mut d = OpenAiCompatibleOptions::new("openai-completions", "litellm", base);
            d.api_key = key;
            d.map_model = Some(Arc::new(move |e, d, _| {
                let id = text(&d, "id").unwrap_or_else(|| "".into());
                let reference = resolver.resolve(&id);
                let mut m = map_with_bundled_reference(e, &d, reference.as_deref());
                m.set("api", wire::str_value(resolve_litellm_api(None, &id, None)));
                let name = text(&m, "name").unwrap_or_else(|| "".into());
                m.set("name", wire::str_value(display_name(Some(name), None, &id)));
                Ok(Some(Arc::new(m)))
            }));
            RawModelValue::from_discovery(fetch_openai_compatible_models(&ctx, &d).await)
        }
    }));
    options
}
