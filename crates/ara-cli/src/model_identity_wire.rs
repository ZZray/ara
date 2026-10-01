//! Lossless companions for fixed OMP catalog identity/reference/metrics.
//! Source: OMP 596f2da7101178214aa27a753529d15e6b7ad91d, packages/catalog/src/identity.
//! MIT License; Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE for the full license.

use crate::js_regex::JsRegExp;
use crate::model_collapse::{SpecRef, VariantSpec, lower, trim, truthy};
use ara_rpc::{WireString, WireValue};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, OnceLock};

pub fn text(model: &VariantSpec, key: &str) -> Option<WireString> {
    model.get(key).and_then(WireValue::as_string).cloned()
}
pub fn number(model: &VariantSpec, key: &str) -> Option<f64> {
    model.get(key).and_then(WireValue::as_number)
}
pub fn boolean(model: &VariantSpec, key: &str) -> Option<bool> {
    match model.get(key) {
        Some(WireValue::Bool(value)) => Some(*value),
        _ => None,
    }
}
pub fn empty() -> VariantSpec {
    VariantSpec::from_wire(WireValue::Object(Vec::new()))
}
pub fn str_value(value: impl Into<WireString>) -> WireValue {
    WireValue::String(value.into())
}
pub fn nullish(value: Option<&WireValue>) -> bool {
    value.is_none_or(|value| matches!(value, WireValue::Null))
}
pub fn equals(left: Option<&WireValue>, right: Option<&WireValue>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(WireValue::Number(a)), Some(WireValue::Number(b))) => a == b,
        (Some(a), Some(b)) => a.deep_equal(b),
        _ => false,
    }
}
pub fn copy_field(target: &mut VariantSpec, key: &str, source: &VariantSpec, source_key: &str) {
    if let Some(record) = source.record(source_key) {
        target.set_record(key, &record);
    } else {
        target.set_undefined(key);
    }
}
/// Object spread preserves own undefined values and insertion slots.
pub fn spread(sources: &[&VariantSpec]) -> VariantSpec {
    let mut out = empty();
    for source in sources {
        let keys = match &source.value {
            WireValue::Array(values) => (0..values.len()).map(|i| i.to_string().into()).collect(),
            WireValue::String(value) => (0..value.len()).map(|i| i.to_string().into()).collect(),
            _ => source.own_keys(),
        };
        for key in keys {
            if let WireValue::String(value) = &source.value {
                let index = key.to_utf8().expect("index").parse::<usize>().expect("index");
                out.set_key_record(
                    &key,
                    &VariantSpec::from_wire(WireValue::String(WireString::from_units(vec![value.units()[index]]))),
                );
                continue;
            }
            if let Some(record) = source.record_key(&key) {
                out.set_key_record(&key, &record);
            } else {
                out.set_key_undefined(&key);
            }
        }
    }
    out
}
pub fn list(model: &VariantSpec, key: &str) -> Vec<WireString> {
    model
        .get(key)
        .and_then(WireValue::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(WireValue::as_string)
        .cloned()
        .collect()
}
pub fn has_string(model: &VariantSpec, key: &str, expected: &str) -> bool {
    list(model, key).iter().any(|value| value.equals_ascii(expected))
}
pub fn to_model_spec(model: &VariantSpec) -> VariantSpec {
    let mut spec = model.clone();
    spec.remove("compat");
    spec.remove("supportsComputerUse");
    let compat = model.record("compatConfig");
    let computer = model.record("supportsComputerUseConfig");
    spec.remove("compatConfig");
    spec.remove("supportsComputerUseConfig");
    if let Some(value) = computer {
        spec.set_record("supportsComputerUse", &value);
    }
    if let Some(value) = compat {
        spec.set_record("compat", &value);
    } else {
        spec.set_undefined("compat");
    }
    spec
}
pub fn bundled_models() -> &'static [SpecRef] {
    static MODELS: OnceLock<Vec<SpecRef>> = OnceLock::new();
    MODELS.get_or_init(|| {
        crate::model_identity::bundled_model_list()
            .iter()
            .map(|model| Arc::new(VariantSpec::from_json(model)))
            .collect()
    })
}
pub fn bundled_provider_models(provider: &WireString) -> Vec<SpecRef> {
    bundled_models().iter().filter(|model| text(model, "provider").as_ref() == Some(provider)).cloned().collect()
}
pub fn create_bundled_reference_map(provider: &WireString) -> HashMap<WireString, SpecRef> {
    bundled_provider_models(provider)
        .iter()
        .filter_map(|model| text(model, "id").map(|id| (id, Arc::new(to_model_spec(model)))))
        .collect()
}
fn zero_xai_reference(model: &VariantSpec) -> bool {
    text(model, "provider").is_some_and(|provider| provider.equals_ascii("xai-oauth"))
        && model.record("cost").is_some_and(|cost| {
            ["input", "output", "cacheRead", "cacheWrite"].iter().all(|key| number(&cost, key) == Some(0.0))
        })
}
fn better(existing: &VariantSpec, candidate: &VariantSpec) -> bool {
    let existing_window = number(existing, "contextWindow").unwrap_or(0.0);
    let candidate_window = number(candidate, "contextWindow").unwrap_or(0.0);
    if !equals(existing.get("contextWindow"), candidate.get("contextWindow")) {
        return candidate_window > existing_window;
    }
    let existing_max = number(existing, "maxTokens").unwrap_or(0.0);
    let candidate_max = number(candidate, "maxTokens").unwrap_or(0.0);
    if !equals(existing.get("maxTokens"), candidate.get("maxTokens")) {
        return candidate_max > existing_max;
    }
    text(existing, "provider").is_none_or(|p| !p.equals_ascii("openai"))
        && text(candidate, "provider").is_some_and(|p| p.equals_ascii("openai"))
}
pub fn global_bundled_references() -> &'static HashMap<WireString, SpecRef> {
    static MODELS: OnceLock<HashMap<WireString, SpecRef>> = OnceLock::new();
    MODELS.get_or_init(|| {
        let mut out: HashMap<WireString, SpecRef> = HashMap::new();
        for model in bundled_models() {
            if text(model, "provider").is_some_and(|p| p.equals_ascii("cline-pass")) || zero_xai_reference(model) {
                continue;
            }
            if let Some(id) = text(model, "id")
                && out.get(&id).is_none_or(|previous| better(previous, model))
            {
                out.insert(id, model.clone());
            }
        }
        out
    })
}
pub struct ReferenceResolver {
    references: OnceLock<HashMap<WireString, SpecRef>>,
    source: Arc<dyn Fn() -> HashMap<WireString, SpecRef> + Send + Sync>,
}
impl ReferenceResolver {
    pub fn new(references: HashMap<WireString, SpecRef>) -> Self {
        Self { references: OnceLock::from(references), source: Arc::new(HashMap::new) }
    }
    pub fn lazy(source: impl Fn() -> HashMap<WireString, SpecRef> + Send + Sync + 'static) -> Self {
        Self { references: OnceLock::new(), source: Arc::new(source) }
    }
    pub fn resolve(&self, id: &WireString) -> Option<SpecRef> {
        self.references
            .get_or_init(|| (self.source)())
            .get(id)
            .cloned()
            .or_else(|| global_bundled_references().get(id).map(|model| Arc::new(to_model_spec(model))))
    }
}
fn whitespace(unit: u16) -> bool {
    matches!(unit, 0x9..=0xd | 0x20 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff)
}
fn normalized_whitespace(id: &WireString) -> WireString {
    let id = trim(id);
    let mut out = Vec::new();
    let mut pending = false;
    for &unit in id.units() {
        if whitespace(unit) {
            pending = true;
        } else {
            if pending {
                out.push(0x20);
                pending = false;
            }
            out.push(unit);
        }
    }
    WireString::from_units(out)
}
pub fn bare_model_id(id: &WireString) -> WireString {
    let start = id.units().iter().rposition(|&unit| unit == 0x2f).map_or(0, |index| index + 1);
    WireString::from_units(id.units()[start..].to_vec())
}
fn regex_remove(id: &WireString, pattern: &str) -> WireString {
    let mut regex = JsRegExp::new(pattern.into(), "").expect("fixed reference regex");
    let Some(matched) = regex.exec(id) else {
        return id.clone();
    };
    let mut units = id.units()[..matched.index].to_vec();
    units.extend_from_slice(&id.units()[matched.end..]);
    WireString::from_units(units)
}
fn segments(id: &WireString) -> Vec<WireString> {
    let id = lower(&normalized_whitespace(id));
    let mut pattern = JsRegExp::new("[a-z0-9.:-]+".into(), "g").expect("fixed reference regex");
    let mut family = JsRegExp::new("^(claude|gemini|gpt|grok|glm|qwen|deepseek|kimi|mimo|doubao|ernie|gpt-oss|gemma|minimax|step|command|jamba|llama|o[1345])".into(), "").expect("fixed reference family");
    let mut out = Vec::new();
    while let Some(found) = pattern.exec(&id) {
        let value = found.captures[0].clone().expect("full match");
        if family.exec(&value).is_some() && value.units().iter().any(|unit| (0x30..=0x39).contains(unit)) {
            out.push(value);
        }
    }
    fn weight(unit: u16) -> u16 {
        match unit {
            0x2d => 0,
            0x3a => 1,
            0x2e => 2,
            other => other,
        }
    }
    out.sort_by(|a, b| {
        b.len()
            .cmp(&a.len())
            .then_with(|| a.units().iter().copied().map(weight).cmp(b.units().iter().copied().map(weight)))
    });
    out.dedup();
    out
}
fn bracket_candidates(id: &WireString) -> Vec<WireString> {
    if !id.units().iter().any(|unit| matches!(unit, 0x5b | 0x5d | 0x3010 | 0x3011)) {
        return Vec::new();
    }
    let id = normalized_whitespace(id);
    let leading = regex_remove(&id, "^(?:\\s*(?:\\[|【)[^\\]】]+(?:\\]|】)\\s*)+");
    let trailing_pattern = "(?:\\s*(?:\\[|【)[^\\]】]+(?:\\]|】)\\s*)+$";
    let mut out = Vec::new();
    for value in [regex_remove(&leading, trailing_pattern), leading, regex_remove(&id, trailing_pattern)] {
        let value = normalized_whitespace(&value);
        if !value.is_empty() && value != id && !out.contains(&value) {
            out.push(value);
        }
    }
    out
}
fn suffixes() -> &'static Vec<WireString> {
    static SUFFIXES: OnceLock<Vec<WireString>> = OnceLock::new();
    SUFFIXES.get_or_init(|| {
        let rules = crate::catalog_rules::compiled_rules();
        let mut out = Vec::new();
        for key in ["billingVariantSuffixes"] {
            if let Some(values) = rules["taxonomy"]["discovery"][key].as_array() {
                out.extend(values.iter().filter_map(|v| v.as_str()).map(WireString::from));
            }
        }
        for key in ["trailingMarkers", "referenceOnlyTrailingMarkers"] {
            if let Some(values) = rules["taxonomy"]["discovery"][key].as_array() {
                for value in values.iter().filter_map(|v| v.as_str()) {
                    out.push(format!("-{value}").into());
                    out.push(format!(":{value}").into());
                }
            }
        }
        for key in ["suffixes", "routingVariants"] {
            if let Some(values) = rules["taxonomy"]["collapse"][key].as_array() {
                out.extend(values.iter().filter_map(|v| v["suffix"].as_str()).map(WireString::from));
            }
        }
        out
    })
}
pub fn reference_candidate_ids(id: &WireString) -> Vec<WireString> {
    let mut queue = VecDeque::from([id.clone()]);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    while let Some(candidate) = queue.pop_front() {
        let candidate = trim(&candidate);
        if candidate.is_empty() || !seen.insert(candidate.clone()) {
            continue;
        }
        out.push(candidate.clone());
        queue.extend(bracket_candidates(&candidate));
        queue.extend(segments(&candidate));
        let lowered = lower(&candidate);
        for suffix in [WireString::from(":cloud"), WireString::from("-cloud")] {
            if lowered.units().ends_with(suffix.units()) {
                queue.push_back(candidate.slice_prefix(candidate.len() - suffix.len()));
            }
        }
        if candidate.units().contains(&0x2f) {
            queue.push_back(bare_model_id(&candidate));
        }
        let dashed =
            WireString::from_units(candidate.units().iter().map(|&u| if u == 0x3a { 0x2d } else { u }).collect());
        if dashed != candidate {
            queue.push_back(dashed);
        }
        if lowered != candidate {
            queue.push_back(lowered.clone());
        }
        if let Some(suffix) = suffixes()
            .iter()
            .filter(|suffix| lowered.units().ends_with(suffix.units()))
            .max_by_key(|suffix| suffix.len())
            && suffix.len() < candidate.len()
        {
            queue.push_back(candidate.slice_prefix(candidate.len() - suffix.len()));
        }
    }
    out
}
fn canonical_key(id: &WireString) -> WireString {
    WireString::from_units(
        lower(&bare_model_id(id)).units().iter().map(|&u| if matches!(u, 0x2e | 0x3a) { 0x2d } else { u }).collect(),
    )
}
fn metric_candidates(id: &WireString) -> Vec<WireString> {
    let mut queue = VecDeque::from(reference_candidate_ids(id));
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    while let Some(candidate) = queue.pop_front() {
        let candidate = lower(&candidate);
        let key = canonical_key(&candidate);
        if seen.insert(key.clone()) {
            out.push(key);
        }
        let bare = bare_model_id(&candidate);
        for pattern in ["^[a-z][a-z-]*\\.", "(?:-v[0-9]+(?::[0-9]+)?|:[0-9]+|-[0-9]{2,})$"] {
            let stripped = regex_remove(&bare, pattern);
            if !stripped.is_empty() && stripped != bare {
                queue.push_back(stripped);
            }
        }
    }
    out
}
fn identities_agree(left: Option<VariantSpec>, right: Option<VariantSpec>) -> bool {
    let (Some(left), Some(right)) = (left, right) else {
        return false;
    };
    if !equals(left.get("class"), right.get("class")) || text(&left, "class").is_some_and(|v| v.equals_ascii("unknown"))
    {
        return false;
    }
    for key in ["family", "revision", "effort"] {
        if left.get(key).is_some() && right.get(key).is_some() && !equals(left.get(key), right.get(key)) {
            return false;
        }
    }
    let fallback = WireValue::Bool(false);
    let l = left.get("thinkingVariant").filter(|v| !matches!(v, WireValue::Null)).unwrap_or(&fallback);
    let r = right.get("thinkingVariant").filter(|v| !matches!(v, WireValue::Null)).unwrap_or(&fallback);
    equals(Some(l), Some(r))
}
pub fn catalog_metrics_of(model: &VariantSpec) -> Option<VariantSpec> {
    let mut out = empty();
    for key in ["int", "tps"] {
        if let Some(value) = number(model, key).filter(|value| value.is_finite() && (key != "tps" || *value > 0.0)) {
            out.set(key, WireValue::Number(value));
        }
    }
    (!out.own_keys().is_empty()).then_some(out)
}
#[derive(Default)]
pub struct CatalogMetricsIndex {
    exact: HashMap<WireString, VariantSpec>,
    canonical: HashMap<WireString, VariantSpec>,
}
impl CatalogMetricsIndex {
    pub fn new(models: &[SpecRef]) -> Self {
        let mut index = Self::default();
        index.add(models);
        index
    }
    pub fn is_empty(&self) -> bool {
        self.exact.is_empty()
    }
    pub fn add(&mut self, models: &[SpecRef]) {
        for model in models {
            let Some(metrics) = catalog_metrics_of(model) else {
                continue;
            };
            let id = text(model, "id").unwrap_or_else(|| "".into());
            let exact = lower(&id);
            let merged =
                self.exact.get(&exact).map_or_else(|| metrics.clone(), |existing| spread(&[&metrics, existing]));
            self.exact.insert(exact, merged);
            let canonical = canonical_key(&id);
            if let Some(existing) = self.canonical.get(&canonical) {
                if existing.get("int").is_none() || existing.get("tps").is_none() {
                    self.canonical.insert(canonical, spread(&[&metrics, existing]));
                }
            } else {
                let mut scored = metrics;
                if let Some(identity) = model.record("identity") {
                    scored.set_record("identity", &identity);
                }
                self.canonical.insert(canonical, scored);
            }
        }
    }
    pub fn resolve(&self, model: &VariantSpec) -> Option<VariantSpec> {
        let id = text(model, "id").unwrap_or_else(|| "".into());
        if let Some(metrics) = self.exact.get(&lower(&id)) {
            return Some(metrics.clone());
        }
        let identity = model.record("identity");
        let mut ids = vec![id];
        if let Some(logical) = identity.as_ref().and_then(|v| text(v, "logicalId")).filter(|id| !id.is_empty()) {
            ids.push(logical);
        }
        for id in ids {
            for key in metric_candidates(&id) {
                if let Some(metrics) = self.canonical.get(&key)
                    && identities_agree(identity.clone(), metrics.record("identity"))
                {
                    return Some(metrics.clone());
                }
            }
        }
        None
    }
}
pub fn apply_catalog_metrics(models: &[SpecRef], index: &CatalogMetricsIndex) -> Vec<SpecRef> {
    if index.is_empty() {
        return models.to_vec();
    }
    models
        .iter()
        .map(|model| {
            if !nullish(model.get("int")) && !nullish(model.get("tps")) {
                return model.clone();
            }
            let Some(metrics) = index.resolve(model) else {
                return model.clone();
            };
            let mut out = (**model).clone();
            let mut changed = false;
            for key in ["int", "tps"] {
                let next = metrics.get(key).filter(|v| !matches!(v, WireValue::Null)).or_else(|| model.get(key));
                if !equals(next, model.get(key)) {
                    changed = true;
                }
                if let Some(value) = next.filter(|v| !matches!(v, WireValue::Null)) {
                    out.set(key, value.clone());
                }
            }
            if changed { Arc::new(out) } else { model.clone() }
        })
        .collect()
}

/// Reference metadata inheritance does not transfer transport across providers.
pub fn inherit_reference_thinking(
    thinking: Option<&VariantSpec>,
    reference: Option<&VariantSpec>,
    provider: &WireString,
) -> Option<VariantSpec> {
    if let Some(thinking) = thinking {
        return Some(thinking.clone());
    }
    let reference = reference?;
    if text(reference, "provider").as_ref() != Some(provider) {
        return None;
    }
    reference.record("thinking").filter(|value| truthy(&value.value))
}

#[derive(Default)]
pub struct ModelReferenceIndex {
    pub exact: HashMap<WireString, SpecRef>,
    pub suffix_alias: HashMap<WireString, SpecRef>,
}
fn better_reference(existing: Option<&SpecRef>, candidate: &SpecRef) -> bool {
    let Some(existing) = existing else {
        return true;
    };
    for key in ["contextWindow", "maxTokens"] {
        if !equals(existing.get(key), candidate.get(key)) {
            return number(candidate, key).unwrap_or(0.0) > number(existing, key).unwrap_or(0.0);
        }
    }
    let priced = |model: &VariantSpec| {
        model.record("cost").is_some_and(|c| {
            number(&c, "cacheRead").is_some_and(|n| n > 0.0) || number(&c, "cacheWrite").is_some_and(|n| n > 0.0)
        })
    };
    if priced(existing) != priced(candidate) {
        return priced(candidate);
    }
    text(existing, "provider").is_none_or(|p| !p.equals_ascii("openai"))
        && text(candidate, "provider").is_some_and(|p| p.equals_ascii("openai"))
}
pub fn build_model_reference_index(models: &[SpecRef]) -> ModelReferenceIndex {
    let mut index = ModelReferenceIndex::default();
    let mut ordered = Vec::new();
    for model in models {
        if zero_xai_reference(model) {
            continue;
        }
        let key = lower(&trim(&text(model, "id").unwrap_or_else(|| "".into())));
        if better_reference(index.exact.get(&key), model) {
            if !index.exact.contains_key(&key) {
                ordered.push(key.clone());
            }
            index.exact.insert(key, model.clone());
        }
    }
    for key in ordered {
        let reference = index.exact.get(&key).expect("ordered reference");
        let id = text(reference, "id").unwrap_or_else(|| "".into());
        if !id.units().contains(&47) {
            continue;
        }
        if let Some(alias) = segments(&bare_model_id(&id)).into_iter().next()
            && better_reference(index.suffix_alias.get(&alias), reference)
        {
            index.suffix_alias.insert(alias, reference.clone());
        }
    }
    index
}
pub fn bundled_model_reference_index() -> &'static ModelReferenceIndex {
    static INDEX: OnceLock<ModelReferenceIndex> = OnceLock::new();
    INDEX.get_or_init(|| build_model_reference_index(bundled_models()))
}
pub fn resolve_model_reference(id: &WireString, index: &ModelReferenceIndex) -> Option<SpecRef> {
    if id.units().first() == Some(&64) {
        return None;
    }
    for candidate in reference_candidate_ids(id) {
        let key = lower(&trim(&candidate));
        if let Some(reference) = index.exact.get(&key).or_else(|| index.suffix_alias.get(&key)) {
            return Some(reference.clone());
        }
    }
    None
}
