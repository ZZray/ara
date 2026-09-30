//! Fixed OMP endpoint classification and catalogue identity/reference helpers.
//!
//! Native port of catalog `hosts.ts`, `identity/{id,dialect,reference,bundled,
//! metrics,priority}.ts` and `provider-models/bundled-references.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d. MIT notice: data/LICENSE.omp-catalog.
//! These helpers preserve complete host metadata; they do not authorize routes.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{LazyLock, Mutex, OnceLock};

use regex::Regex;
use serde_json::{Map, Value};

use crate::catalog_rules::{ClassifyOptions, classify_model, compiled_rules};
use crate::model_catalog::ModelCatalog;

struct HostSpec {
    name: &'static str,
    providers: &'static [&'static str],
    prefixes: &'static [&'static str],
    markers: &'static [&'static str],
}

macro_rules! host {
    ($name:ident, [$($provider:literal),*], [$($marker:literal),*]) => {
        HostSpec { name: stringify!($name), providers: &[$($provider),*], prefixes: &[], markers: &[$($marker),*] }
    };
}

const HOSTS: &[HostSpec] = &[
    host!(openai, ["openai"], ["api.openai.com"]),
    host!(azureOpenAI, ["azure"], [".openai.azure.com", "azure.com/openai", "models.inference.ai.azure.com"]),
    host!(openrouter, ["openrouter"], ["openrouter.ai"]),
    host!(vercelAIGateway, ["vercel-ai-gateway"], ["ai-gateway.vercel.sh"]),
    host!(githubCopilot, ["github-copilot"], ["githubcopilot.com", "copilot-api."]),
    host!(anthropic, ["anthropic"], ["api.anthropic.com"]),
    host!(deepseekDirect, ["deepseek"], ["api.deepseek.com"]),
    host!(deepseekFamily, ["deepseek"], ["deepseek.com"]),
    host!(cerebras, ["cerebras"], ["cerebras.ai"]),
    host!(zai, ["zai"], ["api.z.ai"]),
    host!(zhipu, ["zhipu-coding-plan"], ["open.bigmodel.cn"]),
    host!(kilo, ["kilo"], ["api.kilo.ai"]),
    host!(alibabaDashscope, ["alibaba-coding-plan", "alibaba-token-plan"], ["dashscope", "token-plan."]),
    host!(umans, ["umans"], ["api.code.umans.ai"]),
    HostSpec {
        name: "xiaomi",
        providers: &["xiaomi"],
        prefixes: &["xiaomi-token-plan-"],
        markers: &["xiaomimimo.com"],
    },
    host!(xai, ["xai", "xai-oauth"], ["api.x.ai"]),
    host!(mistral, ["mistral"], ["mistral.ai"]),
    host!(together, ["together"], ["api.together.xyz"]),
    host!(baseten, ["baseten"], ["baseten.co"]),
    host!(fireworks, [], ["fireworks.ai"]),
    host!(groq, ["groq"], ["api.groq.com"]),
    host!(minimax, ["minimax", "minimax-code", "minimax-code-cn"], ["api.minimax.io", "api.minimaxi.com"]),
    host!(qwenPortal, ["qwen-portal"], ["portal.qwen.ai"]),
    host!(nvidia, ["nvidia"], ["integrate.api.nvidia.com"]),
    host!(venice, ["venice"], ["api.venice.ai"]),
    host!(moonshotNative, ["moonshot", "kimi-code"], ["api.moonshot.ai", "api.kimi.com"]),
    host!(googleAistudio, [], ["generativelanguage.googleapis.com"]),
    host!(opencode, ["opencode-go", "opencode-zen"], ["opencode.ai"]),
    host!(zenmux, ["zenmux"], ["zenmux.ai"]),
    host!(chutes, [], ["chutes.ai"]),
];

type UrlHostCache = HashMap<String, HashMap<String, bool>>;
static URL_HOST_CACHE: LazyLock<Mutex<UrlHostCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn known_hosts() -> impl Iterator<Item = &'static str> {
    HOSTS.iter().map(|spec| spec.name)
}

/// Case insensitive substring detection, including upstream host names in paths.
/// This vocabulary deliberately is not an auth-sensitive hostname validator.
pub fn host_matches_url(base_url: &str, host: &str) -> bool {
    if base_url.is_empty() {
        return false;
    }
    let Some(spec) = HOSTS.iter().find(|spec| spec.name == host) else {
        return false;
    };
    let mut cache = URL_HOST_CACHE.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(found) = cache.get(base_url).and_then(|matches| matches.get(host)) {
        return *found;
    }
    if !cache.contains_key(base_url) && cache.len() == 512 {
        cache.clear();
    }
    let found = spec.markers.iter().any(|marker| {
        base_url
            .as_bytes()
            .windows(marker.len())
            .any(|window| window.iter().zip(marker.bytes()).all(|(byte, lower)| (*byte | 0x20) == lower))
    });
    cache.entry(base_url.to_owned()).or_default().insert(host.to_owned(), found);
    found
}

pub fn model_matches_host(model: &Value, host: &str) -> bool {
    let Some(spec) = HOSTS.iter().find(|spec| spec.name == host) else {
        return false;
    };
    let provider = text(model, "provider");
    spec.providers.contains(&provider)
        || spec.prefixes.iter().any(|prefix| provider.starts_with(prefix))
        || host_matches_url(text(model, "baseUrl"), host)
}

pub fn resolve_vertex_endpoint_host(location: &str) -> String {
    match location {
        "global" => "aiplatform.googleapis.com".into(),
        "eu" | "us" => format!("aiplatform.{location}.rep.googleapis.com"),
        _ => format!("{location}-aiplatform.googleapis.com"),
    }
}

pub fn is_vertex_express_openai_url(base_url: &str) -> bool {
    base_url.contains("/endpoints/openapi")
}
pub fn is_vertex_raw_predict_url(base_url: &str) -> bool {
    base_url.contains(":streamRawPredict") || base_url.contains(":rawPredict")
}
pub fn is_azure_deployments_url(base_url: &str) -> bool {
    base_url.contains("/deployments/")
}
pub fn is_dashscope_compatible_mode_url(base_url: &str) -> bool {
    let lower = base_url.to_lowercase();
    lower.contains("dashscope") && lower.contains("aliyuncs.com") && lower.contains("/compatible-mode")
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}
fn js_whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000D}' | ' ' | '\u{00A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}'
        | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}')
}
fn js_trim(value: &str) -> &str {
    value.trim_matches(js_whitespace)
}
fn normalized_whitespace(value: &str) -> String {
    let mut result = String::new();
    let mut pending = false;
    for ch in js_trim(value).chars() {
        if js_whitespace(ch) {
            pending = true;
        } else {
            if pending {
                result.push(' ');
                pending = false;
            }
            result.push(ch);
        }
    }
    result
}

static BARE_ID_CACHE: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
pub fn bare_model_id(model_id: &str) -> String {
    let mut cache = BARE_ID_CACHE.lock().unwrap_or_else(|error| error.into_inner());
    cache.entry(model_id.into()).or_insert_with(|| model_id.rsplit('/').next().unwrap_or("").into()).clone()
}

static SEGMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z0-9.:-]+").unwrap());
static FAMILY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(claude|gemini|gpt|grok|glm|qwen|deepseek|kimi|mimo|doubao|ernie|gpt-oss|gemma|minimax|step|command|jamba|llama|o[1345])").unwrap()
});

pub fn get_model_like_id_segments(model_id: &str) -> Vec<String> {
    let normalized = normalized_whitespace(model_id).to_lowercase();
    let mut result: Vec<String> = SEGMENT
        .find_iter(&normalized)
        .map(|m| m.as_str())
        .filter(|s| FAMILY.is_match(s) && s.bytes().any(|b| b.is_ascii_digit()))
        .map(str::to_owned)
        .collect();
    result.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| ascii_segment_collation(a, b)));
    result.dedup();
    result
}

// The accepted tokens are ASCII only. ICU's default collation puts their
// punctuation before digits/letters; compare primary weights, as localeCompare.
fn ascii_segment_collation(left: &str, right: &str) -> std::cmp::Ordering {
    fn weight(c: u8) -> u8 {
        match c {
            b'-' => 0,
            b':' => 1,
            b'.' => 2,
            _ => c,
        }
    }
    left.bytes().map(weight).cmp(right.bytes().map(weight))
}

pub fn get_longest_model_like_id_segment(model_id: &str) -> Option<String> {
    get_model_like_id_segments(model_id).into_iter().next()
}

const WS: &str = r"[\t\n\x0B\x0C\r \u{A0}\u{FEFF}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}]";
static LEADING_AFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^(?:{WS}*(?:\[|【)[^\]】]+(?:\]|】){WS}*)+")).unwrap());
static TRAILING_AFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?:{WS}*(?:\[|【)[^\]】]+(?:\]|】){WS}*)+$")).unwrap());

pub fn get_bracket_stripped_model_id_candidates(model_id: &str) -> Vec<String> {
    if !model_id.contains(['[', ']', '【', '】']) {
        return Vec::new();
    }
    let normalized = normalized_whitespace(model_id);
    if normalized.is_empty() {
        return Vec::new();
    }
    let leading = LEADING_AFFIX.replace(&normalized, "");
    let both = normalized_whitespace(&TRAILING_AFFIX.replace(&leading, ""));
    let without_leading = normalized_whitespace(&leading);
    let trailing = normalized_whitespace(&TRAILING_AFFIX.replace(&normalized, ""));
    let mut result = Vec::new();
    for candidate in [both, without_leading, trailing] {
        if !candidate.is_empty() && candidate != normalized && !result.contains(&candidate) {
            result.push(candidate);
        }
    }
    result
}

pub fn strip_bracketed_model_id_affixes(model_id: &str) -> Option<String> {
    get_bracket_stripped_model_id_candidates(model_id).into_iter().next()
}

pub fn preferred_dialect(model_id: &str) -> &'static str {
    let identity = classify_model("", model_id, ClassifyOptions { lenient: true, observed_at_ms: None })
        .expect("lenient classification resolves ambiguous model identities");
    match text(&identity, "class") {
        "anthropic" => "anthropic",
        "glm" => "glm",
        "gemini" => "gemini",
        "gemma" => "gemma",
        "kimi" => "kimi",
        "qwen" => "qwen3",
        "deepseek" => "deepseek",
        "minimax" => "minimax",
        "openai" | "gpt-oss" => "harmony",
        _ => "xml",
    }
}

fn strip_reference_trailing_marker(candidate: &str) -> Option<&str> {
    let lower = candidate.to_lowercase();
    let rules = compiled_rules();
    let discovery = &rules["taxonomy"]["discovery"];
    let collapse = &rules["taxonomy"]["collapse"];
    let mut suffixes = Vec::new();
    for key in ["billingVariantSuffixes"] {
        if let Some(values) = discovery[key].as_array() {
            suffixes.extend(values.iter().filter_map(Value::as_str).map(str::to_owned));
        }
    }
    for key in ["trailingMarkers", "referenceOnlyTrailingMarkers"] {
        if let Some(values) = discovery[key].as_array() {
            for marker in values.iter().filter_map(Value::as_str) {
                suffixes.push(format!("-{marker}"));
                suffixes.push(format!(":{marker}"));
            }
        }
    }
    for key in ["suffixes", "routingVariants"] {
        if let Some(values) = collapse[key].as_array() {
            suffixes.extend(values.iter().filter_map(|value| value["suffix"].as_str()).map(str::to_owned));
        }
    }
    let suffix = suffixes.iter().filter(|suffix| lower.ends_with(suffix.as_str())).max_by_key(|s| s.len())?;
    // Compiled suffixes are ASCII; the matching suffix occupies the same bytes.
    if suffix.len() < candidate.len() { Some(&candidate[..candidate.len() - suffix.len()]) } else { None }
}

pub fn get_reference_candidate_ids(model_id: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([model_id.to_owned()]);
    while let Some(item) = queue.pop_front() {
        let candidate = js_trim(&item);
        if candidate.is_empty() || !seen.insert(candidate.to_owned()) {
            continue;
        }
        result.push(candidate.to_owned());
        queue.extend(get_bracket_stripped_model_id_candidates(candidate));
        queue.extend(get_model_like_id_segments(candidate));
        for suffix in [":cloud", "-cloud"] {
            if candidate.to_lowercase().ends_with(suffix) {
                queue.push_back(candidate[..candidate.len() - suffix.len()].into());
            }
        }
        if let Some((_, bare)) = candidate.rsplit_once('/') {
            queue.push_back(bare.into());
        }
        let dashed = candidate.replace(':', "-");
        if dashed != candidate {
            queue.push_back(dashed);
        }
        let lower = candidate.to_lowercase();
        if lower != candidate {
            queue.push_back(lower);
        }
        if let Some(stripped) = strip_reference_trailing_marker(candidate) {
            queue.push_back(stripped.into());
        }
    }
    result
}

pub fn is_zero_cost_xai_oauth_reference(model: &Value) -> bool {
    text(model, "provider") == "xai-oauth"
        && ["input", "output", "cacheRead", "cacheWrite"].iter().all(|key| model["cost"][key].as_f64() == Some(0.0))
}

fn number(model: &Value, key: &str) -> f64 {
    model[key].as_f64().unwrap_or(0.0)
}
fn replace_reference(existing: Option<&Value>, candidate: &Value, cache_pricing: bool) -> bool {
    let Some(existing) = existing else {
        return true;
    };
    for key in ["contextWindow", "maxTokens"] {
        // undefined and null are distinct in JS; an absent-to-null difference
        // still enters this branch even though both coalesce to zero.
        if !js_equal(existing.get(key), candidate.get(key)) {
            return number(candidate, key) > number(existing, key);
        }
    }
    if cache_pricing {
        let priced =
            |value: &Value| number(&value["cost"], "cacheRead") > 0.0 || number(&value["cost"], "cacheWrite") > 0.0;
        if priced(existing) != priced(candidate) {
            return priced(candidate);
        }
    }
    text(existing, "provider") != "openai" && text(candidate, "provider") == "openai"
}

#[derive(Debug, Clone, Default)]
pub struct ModelReferenceIndex {
    pub exact: Map<String, Value>,
    pub suffix_alias: Map<String, Value>,
}

pub fn build_model_reference_index(models: &[Value]) -> ModelReferenceIndex {
    let mut index = ModelReferenceIndex::default();
    for candidate in models {
        if is_zero_cost_xai_oauth_reference(candidate) {
            continue;
        }
        let key = js_trim(text(candidate, "id")).to_lowercase();
        if replace_reference(index.exact.get(&key), candidate, true) {
            index.exact.insert(key, candidate.clone());
        }
    }
    for reference in index.exact.values() {
        let Some((_, suffix)) = text(reference, "id").rsplit_once('/') else {
            continue;
        };
        let Some(alias) = get_longest_model_like_id_segment(suffix) else {
            continue;
        };
        if replace_reference(index.suffix_alias.get(&alias), reference, true) {
            index.suffix_alias.insert(alias, reference.clone());
        }
    }
    index
}

pub fn resolve_model_reference<'a>(model_id: &str, index: &'a ModelReferenceIndex) -> Option<&'a Value> {
    if model_id.starts_with('@') {
        return None;
    }
    for candidate in get_reference_candidate_ids(model_id) {
        let key = js_trim(&candidate).to_lowercase();
        if let Some(value) = index.exact.get(&key).or_else(|| index.suffix_alias.get(&key)) {
            return Some(value);
        }
    }
    None
}

/// Presence of explicit null also prevents inheritance, as JS !== undefined.
pub fn inherit_reference_thinking(
    model_thinking: Option<&Value>,
    reference: Option<&Value>,
    provider: &str,
) -> Option<Value> {
    if let Some(thinking) = model_thinking {
        return Some(thinking.clone());
    }
    let reference = reference?;
    if text(reference, "provider") != provider {
        return None;
    }
    reference.get("thinking").filter(|v| js_truthy(v)).cloned()
}

fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|n| n != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn canonical_key(id: &str) -> String {
    bare_model_id(id).to_lowercase().replace(['.', ':'], "-")
}
fn identities_agree(left: &Value, right: &Value) -> bool {
    if left.get("class") != right.get("class") || text(left, "class") == "unknown" {
        return false;
    }
    for key in ["family", "revision", "effort"] {
        if let (Some(left), Some(right)) = (left.get(key), right.get(key))
            && left != right
        {
            return false;
        }
    }
    left.get("thinkingVariant").filter(|v| !v.is_null()).unwrap_or(&Value::Bool(false))
        == right.get("thinkingVariant").filter(|v| !v.is_null()).unwrap_or(&Value::Bool(false))
}

pub fn catalog_metrics_of(model: &Value) -> Option<Value> {
    let mut metrics = Map::new();
    for key in ["int", "tps"] {
        if let Some(value) =
            model.get(key).filter(|value| value.as_f64().is_some_and(|n| n.is_finite() && (key != "tps" || n > 0.0)))
        {
            metrics.insert(key.into(), value.clone());
        }
    }
    (!metrics.is_empty()).then_some(Value::Object(metrics))
}

static STRIPPABLE_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:-v[0-9]+(?::[0-9]+)?|:[0-9]+|-[0-9]{2,})$").unwrap());
static DOTTED_PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z][a-z-]*\.").unwrap());
static METRIC_CANDIDATE_CACHE: LazyLock<Mutex<HashMap<String, Vec<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
fn metric_candidate_keys(model_id: &str) -> Vec<String> {
    if let Some(keys) = METRIC_CANDIDATE_CACHE.lock().unwrap_or_else(|e| e.into_inner()).get(model_id) {
        return keys.clone();
    }
    let mut keys = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from(get_reference_candidate_ids(model_id));
    while let Some(candidate) = queue.pop_front() {
        let lower = candidate.to_lowercase();
        let key = canonical_key(&lower);
        if seen.insert(key.clone()) {
            keys.push(key);
        }
        let bare = bare_model_id(&lower);
        for pattern in [&*DOTTED_PREFIX, &*STRIPPABLE_SUFFIX] {
            let stripped = pattern.replace(&bare, "");
            if !stripped.is_empty() && stripped != bare {
                queue.push_back(stripped.into_owned());
            }
        }
    }
    METRIC_CANDIDATE_CACHE.lock().unwrap_or_else(|e| e.into_inner()).insert(model_id.into(), keys.clone());
    keys
}

#[derive(Debug, Clone, Default)]
pub struct CatalogMetricsIndex {
    exact: Map<String, Value>,
    canonical: Map<String, Value>,
}

impl CatalogMetricsIndex {
    pub fn new(models: &[Value]) -> Self {
        let mut result = Self::default();
        result.add(models);
        result
    }
    pub fn is_empty(&self) -> bool {
        self.exact.is_empty()
    }
    pub fn add(&mut self, models: &[Value]) {
        for model in models {
            let Some(Value::Object(metrics)) = catalog_metrics_of(model) else {
                continue;
            };
            let exact_key = text(model, "id").to_lowercase();
            let mut merged = metrics.clone();
            if let Some(Value::Object(existing)) = self.exact.get(&exact_key) {
                merged.extend(existing.clone());
            }
            self.exact.insert(exact_key, Value::Object(merged));
            let key = canonical_key(text(model, "id"));
            match self.canonical.get(&key).and_then(Value::as_object) {
                None => {
                    let mut scored = metrics;
                    if let Some(identity) = model.get("identity") {
                        scored.insert("identity".into(), identity.clone());
                    }
                    self.canonical.insert(key, Value::Object(scored));
                }
                Some(scored) if !scored.contains_key("int") || !scored.contains_key("tps") => {
                    let mut merged = metrics;
                    merged.extend(scored.clone());
                    self.canonical.insert(key, Value::Object(merged));
                }
                Some(_) => {}
            }
        }
    }
    pub fn resolve(&self, model: &Value) -> Option<Value> {
        if let Some(exact) = self.exact.get(&text(model, "id").to_lowercase()) {
            return Some(exact.clone());
        }
        let identity = &model["identity"];
        let mut ids = vec![text(model, "id")];
        if let Some(logical) = identity["logicalId"].as_str().filter(|id| !id.is_empty()) {
            ids.push(logical);
        }
        for id in ids {
            for key in metric_candidate_keys(id) {
                if let Some(scored) = self.canonical.get(&key)
                    && identities_agree(identity, &scored["identity"])
                {
                    return Some(scored.clone());
                }
            }
        }
        None
    }
}

fn js_equal(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left, right) {
        (Some(Value::Number(a)), Some(Value::Number(b))) => a.as_f64() == b.as_f64(),
        _ => left == right,
    }
}

/// Borrow the original array when unchanged, preserving upstream cache identity.
pub fn apply_catalog_metrics<'a>(models: &'a [Value], index: &CatalogMetricsIndex) -> Cow<'a, [Value]> {
    if index.is_empty() {
        return Cow::Borrowed(models);
    }
    let mut changed: Option<Vec<Value>> = None;
    for (position, model) in models.iter().enumerate() {
        if model.get("int").is_some_and(|v| !v.is_null()) && model.get("tps").is_some_and(|v| !v.is_null()) {
            continue;
        }
        let Some(metrics) = index.resolve(model) else {
            continue;
        };
        let int = metrics.get("int").filter(|v| !v.is_null()).or_else(|| model.get("int"));
        let tps = metrics.get("tps").filter(|v| !v.is_null()).or_else(|| model.get("tps"));
        if js_equal(int, model.get("int")) && js_equal(tps, model.get("tps")) {
            continue;
        }
        let target = &mut changed.get_or_insert_with(|| models.to_vec())[position];
        if let Some(row) = target.as_object_mut() {
            if let Some(int) = int.filter(|v| !v.is_null()) {
                row.insert("int".into(), int.clone());
            }
            if let Some(tps) = tps.filter(|v| !v.is_null()) {
                row.insert("tps".into(), tps.clone());
            }
        }
    }
    changed.map(Cow::Owned).unwrap_or(Cow::Borrowed(models))
}

const DEFAULT_PROVIDER_ORDER: &[&str] = &[
    "openai-codex",
    "anthropic",
    "openai",
    "google-gemini-cli",
    "google",
    "google-vertex",
    "kimi-code",
    "moonshot",
    "qwen-portal",
    "alibaba-token-plan",
    "zai",
    "xai-oauth",
    "xai",
    "mistral",
    "deepseek",
    "groq",
    "fireworks",
    "cerebras",
    "baseten",
    "deepinfra",
    "openrouter",
    "aimlapi",
    "together",
    "alibaba-coding-plan",
    "umans",
    "google-antigravity",
    "opencode-zen",
    "gitlab-duo",
    "opencode-go",
    "kilo",
    "vercel-ai-gateway",
    "cloudflare-ai-gateway",
    "nanogpt",
    "github-copilot",
];

pub fn build_model_provider_priority_rank(configured: &[String]) -> Map<String, Value> {
    let mut ranks = Map::new();
    for provider in configured.iter().map(String::as_str).chain(DEFAULT_PROVIDER_ORDER.iter().copied()) {
        let normalized = js_trim(provider).to_lowercase();
        if !normalized.is_empty() && !ranks.contains_key(&normalized) {
            ranks.insert(normalized, Value::from(ranks.len()));
        }
    }
    ranks
}

static BUNDLED_MODELS: OnceLock<Vec<Value>> = OnceLock::new();
pub fn bundled_model_list() -> &'static [Value] {
    BUNDLED_MODELS.get_or_init(|| {
        ModelCatalog::bundled()
            .expect("pinned catalogue validated")
            .all()
            .iter()
            .map(|model| {
                let mut row = model.metadata().clone();
                if row.get("identity").is_none_or(Value::is_null) {
                    row["identity"] = classify_model(
                        model.provider(),
                        model.id(),
                        ClassifyOptions { lenient: true, observed_at_ms: None },
                    )
                    .expect("lenient bundled classification");
                }
                row
            })
            .collect()
    })
}

pub fn get_bundled_model_reference_index() -> &'static ModelReferenceIndex {
    static INDEX: OnceLock<ModelReferenceIndex> = OnceLock::new();
    INDEX.get_or_init(|| build_model_reference_index(bundled_model_list()))
}

/// Restore the authored sparse configuration, removing materialized policy.
pub fn to_model_spec(model: &Value) -> Value {
    let Some(mut row) = model.as_object().cloned() else {
        return model.clone();
    };
    row.remove("compat");
    let compat = row.remove("compatConfig");
    row.remove("supportsComputerUse");
    if let Some(config) = row.remove("supportsComputerUseConfig") {
        row.insert("supportsComputerUse".into(), config);
    }
    if let Some(compat) = compat {
        row.insert("compat".into(), compat);
    }
    Value::Object(row)
}

pub fn create_bundled_reference_map(provider: &str) -> Map<String, Value> {
    bundled_model_list()
        .iter()
        .filter(|model| text(model, "provider") == provider)
        .map(|model| (text(model, "id").into(), to_model_spec(model)))
        .collect()
}

pub fn global_bundled_references() -> &'static Map<String, Value> {
    static GLOBAL: OnceLock<Map<String, Value>> = OnceLock::new();
    GLOBAL.get_or_init(|| {
        let mut references = Map::new();
        for candidate in bundled_model_list() {
            if text(candidate, "provider") == "cline-pass" || is_zero_cost_xai_oauth_reference(candidate) {
                continue;
            }
            let key = text(candidate, "id");
            if replace_reference(references.get(key), candidate, false) {
                references.insert(key.into(), candidate.clone());
            }
        }
        references
    })
}

type ReferenceSource = Box<dyn FnOnce() -> Map<String, Value>>;
pub struct ReferenceResolver {
    references: Option<Map<String, Value>>,
    source: Option<ReferenceSource>,
}
impl ReferenceResolver {
    pub fn new(references: Map<String, Value>) -> Self {
        Self { references: Some(references), source: None }
    }
    pub fn lazy(source: impl FnOnce() -> Map<String, Value> + 'static) -> Self {
        Self { references: None, source: Some(Box::new(source)) }
    }
    pub fn resolve(&mut self, model_id: &str) -> Option<Value> {
        let provider =
            self.references.get_or_insert_with(|| self.source.take().expect("uninitialized reference source")());
        let global = global_bundled_references();
        provider.get(model_id).cloned().or_else(|| global.get(model_id).map(to_model_spec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hosts_keep_url_substring_and_provider_boundaries() {
        assert!(host_matches_url("https://proxy/PASS/API.OPENAI.COM/v1", "openai"));
        assert!(model_matches_host(&json!({"provider":"xiaomi-token-plan-ams","baseUrl":""}), "xiaomi"));
        assert!(!model_matches_host(&json!({"provider":"fireworks","baseUrl":"https://other"}), "fireworks"));
        assert!(!model_matches_host(&json!({"provider":"OpenAI","baseUrl":""}), "openai"));
        assert_eq!(known_hosts().count(), 30);
        assert_eq!(resolve_vertex_endpoint_host("eu"), "aiplatform.eu.rep.googleapis.com");
        assert_eq!(resolve_vertex_endpoint_host("EU"), "EU-aiplatform.googleapis.com");
    }

    #[test]
    fn affix_reference_candidates_preserve_order_and_opaque_ids() {
        assert_eq!(
            get_bracket_stripped_model_id_candidates(" [Kiro] claude-opus-4-8 [tag] "),
            ["claude-opus-4-8", "claude-opus-4-8 [tag]", "[Kiro] claude-opus-4-8"]
        );
        assert_eq!(strip_bracketed_model_id_affixes("[]gpt-5.4"), None);
        assert_eq!(bare_model_id("vendor/gpt-5.4"), "gpt-5.4");
        let row = json!({"id":"gpt-5.4","provider":"openai","contextWindow":200000,"maxTokens":10000,"cost":{"input":1,"output":2,"cacheRead":0,"cacheWrite":0}});
        let index = build_model_reference_index(&[row]);
        assert!(resolve_model_reference("[relay] gpt-5.4:cloud", &index).is_some());
        assert!(resolve_model_reference("@relay/gpt-5.4", &index).is_none());
    }

    #[test]
    fn reference_ranking_preserves_cache_pricing_and_thinking_provider() {
        let row = json!({"id":"gpt-5.4","provider":"proxy","contextWindow":100,"maxTokens":50,"cost":{"input":1,"output":2,"cacheRead":1,"cacheWrite":0}});
        let mut official = row.clone();
        official["provider"] = json!("openai");
        official["cost"]["cacheRead"] = json!(0);
        let index = build_model_reference_index(&[row.clone(), official]);
        assert_eq!(index.exact["gpt-5.4"]["provider"], "proxy");
        let reference = json!({"provider":"proxy","thinking":{"effortRouting":{"high":"wire"}}});
        assert_eq!(inherit_reference_thinking(None, Some(&reference), "openai"), None);
        assert!(inherit_reference_thinking(None, Some(&reference), "proxy").is_some());
        assert_eq!(inherit_reference_thinking(Some(&Value::Null), Some(&reference), "proxy"), Some(Value::Null));
    }

    #[test]
    fn metrics_merge_fields_and_require_identity_on_dialect_match() {
        let identity = json!({"class":"anthropic","family":"sonnet","revision":"4.6"});
        let row = json!({"id":"claude-sonnet-4-6","identity":identity,"int":83});
        let mut index = CatalogMetricsIndex::new(&[row]);
        index.add(&[json!({"id":"claude-sonnet-4-6","identity":identity,"int":91,"tps":110})]);
        let target = json!({"id":"global.anthropic.claude-sonnet-4-6-v1:0","identity":identity});
        assert_eq!(index.resolve(&target).unwrap()["int"], 83);
        assert_eq!(index.resolve(&target).unwrap()["tps"], 110);
        let mut wrong = target.clone();
        wrong["identity"]["revision"] = json!("4.5");
        assert!(index.resolve(&wrong).is_none());
        let targets = [target];
        assert!(matches!(apply_catalog_metrics(&targets, &index), Cow::Owned(_)));
        let filled = apply_catalog_metrics(&targets, &index).into_owned();
        assert!(matches!(apply_catalog_metrics(&filled, &index), Cow::Borrowed(_)));
    }

    #[test]
    fn provider_priority_and_sparse_projection_keep_explicit_values() {
        let ranks =
            build_model_provider_priority_rank(&[" CUSTOM ".into(), "custom".into(), " OPENAI ".into(), "".into()]);
        assert_eq!(ranks["custom"], 0);
        assert_eq!(ranks["openai"], 1);
        assert_eq!(ranks["openai-codex"], 2);
        let spec = to_model_spec(&json!({"id":"x","compat":{"materialized":true},"supportsComputerUse":true,
            "compatConfig":null,"supportsComputerUseConfig":false,"unknown":{"keep":1}}));
        assert_eq!(spec["compat"], Value::Null);
        assert_eq!(spec["supportsComputerUse"], false);
        assert_eq!(spec["unknown"]["keep"], 1);
        assert!(to_model_spec(&json!({"id":"x","compat":{"materialized":true}})).get("compat").is_none());
    }

    #[test]
    fn bundle_lookup_is_lazy_and_provider_local_references_win() {
        let models = bundled_model_list();
        assert_eq!(models.len(), 4776);
        assert!(std::ptr::eq(models, bundled_model_list()));
        let source = json!({"sentinel":{"id":"sentinel","compat":{"authored":true}}}).as_object().unwrap().clone();
        let mut resolver = ReferenceResolver::lazy(move || source);
        assert_eq!(resolver.resolve("sentinel").unwrap()["compat"]["authored"], true);
        assert!(resolver.resolve("gpt-5.4").is_some());
        assert!(resolver.resolve("not-a-model").is_none());
        assert_eq!(preferred_dialect("claude-sonnet-4-6"), "anthropic");
    }
}
