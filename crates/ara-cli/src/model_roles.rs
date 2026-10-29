//! Fixed OMP model role and selector resolution.
//!
//! Source 596f2da7101178214aa27a753529d15e6b7ad91d:
//! coding-agent/src/config/model-resolver.ts:114-415,582-699,740-1257,
//! 1403-1458,1646-1663,1866-2120; coding-agent/src/config/model-roles.ts;
//! coding-agent/src/priority.json; tui/src/fuzzy.ts:1-409.
//! Provider reference/alias/date/profile matching reuses its existing Rust port.
//! Availability is an observation supplied by the Registry, never a credential
//! resolution or an authorization to execute a model. Opaque headers retain
//! their actual donor identity through routed model copies.
//!
//! MIT License
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
//! of the Software, and to permit persons to whom the Software is furnished to
//! do so, subject to the following conditions:
//! The above copyright notice and this permission notice shall be included in
//! all copies or substantial portions of the Software.
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.

use crate::{
    catalog_discovery::CatalogCollation,
    catalog_rules,
    model_collapse::{CollapseError, CollapseRuntime, lower},
    model_identity::{build_model_provider_priority_rank, model_matches_host},
    model_identity_wire::{boolean, bundled_provider_models, empty, spread, text},
    model_patch::{HostModelRef, build_host_model},
    provider_model_reference::ProviderModelReferenceIndex,
    retry_fallback::{ConfiguredThinkingLevel, ThinkingLevel, parse_thinking_level},
};
use ara_rpc::{WireString, WireValue};
use regex::Regex;
use serde_json::Value;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
};

pub const MODEL_ROLE_IDS: &[&str] = &["default", "smol", "slow", "vision", "plan", "commit", "tiny", "task", "advisor"];

/// Mirrors Settings.getModelRole. A caller can bind its existing settings owner
/// with a closure; no settings persistence is introduced here.
pub trait ModelRoleLookup {
    fn get_model_role(&self, role: &str) -> Option<String>;
}
impl<F: Fn(&str) -> Option<String>> ModelRoleLookup for F {
    fn get_model_role(&self, role: &str) -> Option<String> {
        self(role)
    }
}

/// Native Settings string/all-string-array value observation. Invalid arrays
/// remain unconfigured, rather than silently retaining only their string rows.
pub fn model_role_value(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(value) = value.as_str() {
        return Some(value.to_owned());
    }
    value
        .as_array()?
        .iter()
        .map(|value| value.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .map(|values| values.join(","))
}

/// Comma entries retain author order. Arrays are flattened only at their comma
/// boundaries, matching the Native role-value normalization contract.
pub fn normalize_model_pattern_list(value: Option<&Value>) -> Vec<String> {
    model_role_value(value).map(|value| normalize_patterns(Some(&value))).unwrap_or_default()
}
fn js_trim(value: &str) -> &str {
    value.trim_matches(|ch| {
        matches!(ch, '\u{0009}'..='\u{000D}' | ' ' | '\u{00A0}' | '\u{1680}'
        | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}'
        | '\u{3000}' | '\u{FEFF}')
    })
}
fn normalize_patterns(value: Option<&str>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(js_trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn alias_prefix_length(value: &str) -> Option<usize> {
    if value == "*" || value.starts_with("*:") {
        Some(0)
    } else if value.starts_with('@') {
        Some(1)
    } else if value.starts_with("pi/") {
        Some(3)
    } else {
        None
    }
}
fn thinking_suffix(value: &str, allow_max_auto: bool) -> Option<ConfiguredThinkingLevel> {
    if allow_max_auto && value == "auto" {
        return Some(ConfiguredThinkingLevel::Auto);
    }
    parse_thinking_level(value)
        .filter(|value| allow_max_auto || *value != ThinkingLevel::Max)
        .map(ConfiguredThinkingLevel::Concrete)
}
fn split_thinking(
    value: &str,
    minimum_colon: Option<usize>,
    allow_max_auto: bool,
) -> (&str, Option<ConfiguredThinkingLevel>) {
    let Some(colon) = value.rfind(':').filter(|colon| minimum_colon.is_none_or(|minimum| *colon > minimum)) else {
        return (value, None);
    };
    match thinking_suffix(&value[colon + 1..], allow_max_auto) {
        Some(level) => (&value[..colon], Some(level)),
        None => (value, None),
    }
}
fn thinking_text(level: ConfiguredThinkingLevel) -> &'static str {
    match level {
        ConfiguredThinkingLevel::Auto => "auto",
        ConfiguredThinkingLevel::Concrete(level) => level.as_str(),
    }
}
fn role_alias(value: &str, settings: &dyn ModelRoleLookup) -> Option<String> {
    let value = js_trim(value);
    let prefix = alias_prefix_length(value)?;
    let role = if value == "*" { "default" } else { &value[prefix..] };
    (MODEL_ROLE_IDS.contains(&role) || settings.get_model_role(role).is_some()).then(|| role.to_owned())
}
pub fn resolve_explicit_model_role(value: Option<&Value>, settings: &dyn ModelRoleLookup) -> Option<String> {
    for pattern in normalize_model_pattern_list(value) {
        let Some(prefix) = alias_prefix_length(&pattern) else { continue };
        let (base, _) = split_thinking(&pattern, Some(prefix), true);
        if let Some(role) = role_alias(base, settings) {
            return Some(role);
        }
    }
    None
}

static PRIORITY: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../data/omp-model-role-priority.json")).expect("fixed role priorities")
});
fn role_defaults(role: &str) -> Vec<String> {
    let key = match role {
        "tiny" => "smol",
        "advisor" => "slow",
        role => role,
    };
    normalize_model_pattern_list(PRIORITY.get(key))
}
fn resolve_default_inherited_patterns(
    role: &str,
    defaults: &[String],
    settings: &dyn ModelRoleLookup,
    visited: &HashSet<String>,
) -> Vec<String> {
    if !matches!(role, "smol" | "slow") {
        return Vec::new();
    }
    let Some(configured_default) = settings.get_model_role("default").filter(|value| !js_trim(value).is_empty()) else {
        return Vec::new();
    };
    let mut resolved = Vec::new();
    for pattern in normalize_patterns(Some(&configured_default)) {
        let (alias, level) = split_thinking(&pattern, Some(alias_prefix_length(&pattern).unwrap_or(3)), true);
        let alias = role_alias(alias, settings);
        if alias.as_deref() == Some(role) {
            resolved.extend(defaults.iter().map(|pattern| match level {
                Some(level) => format!("{pattern}:{}", thinking_text(level)),
                None => pattern.clone(),
            }));
            continue;
        }
        if alias.as_ref().is_some_and(|alias| !visited.contains(alias))
            && let Some(recursed) = resolve_configured_role_pattern(&pattern, settings, visited.clone())
            && !recursed.is_empty()
        {
            resolved.extend(recursed);
            continue;
        }
        resolved.push(pattern);
    }
    resolved
}
fn resolve_configured_role_pattern(
    value: &str,
    settings: &dyn ModelRoleLookup,
    mut visited: HashSet<String>,
) -> Option<Vec<String>> {
    let value = js_trim(value);
    if value.is_empty() {
        return None;
    }
    let (alias, level) = split_thinking(value, Some(alias_prefix_length(value).unwrap_or(3)), true);
    let Some(role) = role_alias(alias, settings) else { return Some(vec![value.to_owned()]) };
    if !visited.insert(role.clone()) {
        return None;
    }
    let builtin = MODEL_ROLE_IDS.contains(&role.as_str());
    let defaults = if builtin { role_defaults(&role) } else { Vec::new() };
    let configured = settings.get_model_role(&role).filter(|value| !js_trim(value).is_empty());
    let mut resolved = match configured {
        Some(value) => normalize_patterns(Some(&value)),
        None if builtin => resolve_default_inherited_patterns(&role, &defaults, settings, &visited),
        None => defaults.clone(),
    };
    if resolved.is_empty() {
        resolved.extend(defaults);
    }
    if resolved.is_empty() {
        return None;
    }
    if let Some(level) = level {
        resolved = resolved.into_iter().map(|pattern| format!("{pattern}:{}", thinking_text(level))).collect();
    }
    Some(resolved)
}
pub fn expand_role_alias(value: &str, settings: &dyn ModelRoleLookup) -> String {
    if js_trim(value) == "default" {
        return settings.get_model_role("default").unwrap_or_else(|| value.to_owned());
    }
    resolve_configured_role_pattern(value, settings, HashSet::new())
        .and_then(|patterns| patterns.into_iter().next())
        .unwrap_or_else(|| value.to_owned())
}
pub fn resolve_configured_model_patterns(value: Option<&Value>, settings: &dyn ModelRoleLookup) -> Vec<String> {
    normalize_model_pattern_list(value)
        .into_iter()
        .flat_map(|pattern| resolve_configured_role_pattern(&pattern, settings, HashSet::new()).unwrap_or_default())
        .collect()
}

#[derive(Clone, Default)]
pub struct ModelMatchPreferences {
    pub usage_order: Vec<String>,
    pub provider_order: Vec<String>,
    pub deprioritize_providers: Vec<String>,
}
#[derive(Default)]
struct PreferenceContext {
    model_usage: HashMap<WireString, usize>,
    provider_usage: HashMap<WireString, usize>,
    provider_priority: HashMap<WireString, usize>,
    deprioritized: HashSet<WireString>,
    model_order: HashMap<WireString, usize>,
}
fn model_key(model: &HostModelRef) -> WireString {
    let mut key = text(model.spec(), "provider").unwrap_or_else(|| "".into()).units().to_vec();
    key.push(b'/' as u16);
    key.extend(text(model.spec(), "id").unwrap_or_else(|| "".into()).units());
    WireString::from_units(key)
}
fn model_field(model: &HostModelRef, key: &str) -> WireString {
    text(model.spec(), key).unwrap_or_else(|| "".into())
}
fn preference_context(models: &[HostModelRef], preferences: &ModelMatchPreferences) -> PreferenceContext {
    let mut context = PreferenceContext::default();
    for (index, key) in preferences.usage_order.iter().enumerate() {
        context.model_usage.entry(key.as_str().into()).or_insert(index);
        if let Some((provider, _)) = key.split_once('/')
            && !provider.is_empty()
        {
            context.provider_usage.entry(provider.into()).or_insert(index);
        }
    }
    context.provider_priority = build_model_provider_priority_rank(&preferences.provider_order)
        .into_iter()
        .filter_map(|(key, rank)| rank.as_u64().map(|rank| (key.into(), rank as usize)))
        .collect();
    context.deprioritized = preferences.deprioritize_providers.iter().map(|value| value.as_str().into()).collect();
    for (index, model) in models.iter().enumerate() {
        context.model_order.insert(model_key(model), index);
    }
    context
}
fn compare_optional_rank(a: Option<&usize>, b: Option<&usize>) -> Ordering {
    a.copied().unwrap_or(usize::MAX).cmp(&b.copied().unwrap_or(usize::MAX))
}
impl PreferenceContext {
    fn preferred(&self, a: &HostModelRef, b: &HostModelRef) -> Ordering {
        let a_key = model_key(a);
        let b_key = model_key(b);
        let a_provider = model_field(a, "provider");
        let b_provider = model_field(b, "provider");
        for (a, b) in [
            (self.model_usage.get(&a_key), self.model_usage.get(&b_key)),
            (self.provider_priority.get(&lower(&a_provider)), self.provider_priority.get(&lower(&b_provider))),
            (self.provider_usage.get(&a_provider), self.provider_usage.get(&b_provider)),
        ] {
            if a.is_some() || b.is_some() {
                return compare_optional_rank(a, b);
            }
        }
        self.deprioritized.contains(&a_provider).cmp(&self.deprioritized.contains(&b_provider)).then_with(|| {
            self.model_order.get(&a_key).copied().unwrap_or(0).cmp(&self.model_order.get(&b_key).copied().unwrap_or(0))
        })
    }
    fn fuzzy_tie(&self, a: &HostModelRef, b: &HostModelRef) -> Ordering {
        let a_key = model_key(a);
        let b_key = model_key(b);
        compare_optional_rank(self.model_usage.get(&a_key), self.model_usage.get(&b_key))
            .then_with(|| {
                compare_optional_rank(
                    self.provider_usage.get(&model_field(a, "provider")),
                    self.provider_usage.get(&model_field(b, "provider")),
                )
            })
            .then_with(|| {
                self.model_order
                    .get(&a_key)
                    .copied()
                    .unwrap_or(0)
                    .cmp(&self.model_order.get(&b_key).copied().unwrap_or(0))
            })
    }
}

/// No Debug/Serialize: selected model references retain opaque donor headers.
pub struct ResolvedModelRoleValue {
    pub model: Option<HostModelRef>,
    pub thinking_level: Option<ConfiguredThinkingLevel>,
    pub matched_pattern_index: Option<usize>,
    pub explicit_thinking_level: bool,
    pub upstream: Option<String>,
    pub warning: Option<String>,
}
impl ResolvedModelRoleValue {
    fn empty() -> Self {
        Self {
            model: None,
            thinking_level: None,
            matched_pattern_index: None,
            explicit_thinking_level: false,
            upstream: None,
            warning: None,
        }
    }
    fn matched(model: HostModelRef) -> Self {
        Self { model: Some(model), ..Self::empty() }
    }
}
pub struct ResolvedRoleSelection {
    pub role: String,
    pub model: HostModelRef,
    pub thinking_level: Option<ConfiguredThinkingLevel>,
}

/// Fixed Native CLI selection metadata. The Host still validates credentials
/// and decides when to use deferred configured patterns. No secret formatting.
#[derive(Default)]
pub struct ResolvedCliModel {
    pub model: Option<HostModelRef>,
    pub configured_patterns: Option<Vec<String>>,
    pub configured_role: Option<String>,
    pub configured_pattern_index: Option<usize>,
    pub selector: Option<String>,
    pub thinking_level: Option<ConfiguredThinkingLevel>,
    pub warning: Option<String>,
    pub error: Option<String>,
}
impl ResolvedCliModel {
    fn exact(model: HostModelRef, thinking_level: Option<ConfiguredThinkingLevel>) -> Result<Self, CollapseError> {
        Ok(Self { selector: Some(cli_selector(&model)?), model: Some(model), thinking_level, ..Self::default() })
    }
}
fn cli_selector(model: &HostModelRef) -> Result<String, CollapseError> {
    model_key(model)
        .to_utf8()
        .map_err(|_| CollapseError::new("CLI model selector cannot be represented as UTF-8".into()))
}

/// One available-model snapshot and its matching shared Host alias runtime.
/// Construction and all methods are synchronous and never resolve credentials.
pub struct ModelRoleResolver {
    available: Vec<HostModelRef>,
    references: ProviderModelReferenceIndex,
    collapse: Arc<Mutex<CollapseRuntime>>,
    preference: PreferenceContext,
    collation: CatalogCollation,
    fuzzy_ids: Vec<FuzzyIndex>,
}
impl ModelRoleResolver {
    pub fn new(
        available: Vec<HostModelRef>,
        collapse: Arc<Mutex<CollapseRuntime>>,
        preferences: ModelMatchPreferences,
    ) -> Result<Self, CollapseError> {
        let references = ProviderModelReferenceIndex::with_collapse_runtime(&available, collapse.clone())?;
        let preference = preference_context(&available, &preferences);
        let collation = CatalogCollation::for_host().map_err(|error| CollapseError::new(error.to_string()))?;
        let fuzzy_ids = available.iter().map(|model| FuzzyIndex::new(&model_field(model, "id"))).collect();
        Ok(Self { available, references, collapse, preference, collation, fuzzy_ids })
    }
    pub fn available_models(&self) -> &[HostModelRef] {
        &self.available
    }
    /// Reuse the same catalog/collapse owner with a different Native preference
    /// context (enabled scope's direct patterns intentionally have no settings).
    pub fn with_preferences(&self, preferences: ModelMatchPreferences) -> Result<Self, CollapseError> {
        Self::new(self.available.clone(), self.collapse.clone(), preferences)
    }
    /// Build a scope-local matching context with its actual donors while
    /// preserving this Registry generation's shared alias runtime.
    pub fn with_available_models(
        &self,
        available: Vec<HostModelRef>,
        preferences: ModelMatchPreferences,
    ) -> Result<Self, CollapseError> {
        Self::new(available, self.collapse.clone(), preferences)
    }

    /// Fixed Native findExactCliModel. A provider/id reference is authoritative
    /// in this full catalog. Flat matches use preferred array order, without
    /// MRU ranking, and defer a catalog-only collision when a role may match.
    fn find_exact_cli_model(
        &self,
        selector: &str,
        preferred: &[HostModelRef],
        catalog_fallback: bool,
    ) -> Result<Option<HostModelRef>, CollapseError> {
        if let Some((provider, id)) = js_trim(selector).split_once('/')
            && !js_trim(provider).is_empty()
            && !js_trim(id).is_empty()
            && let Some(model) = self.references.resolve(&js_trim(provider).into(), &js_trim(id).into())?
        {
            return Ok(Some(model));
        }
        let normalized = lower(&selector.into());
        let flat_match = |model: &&HostModelRef| {
            (lower(&model_field(model, "id")) == normalized || lower(&model_key(model)) == normalized)
                && !provider_locked_cross_match(selector, model)
        };
        if let Some(model) = preferred.iter().find(flat_match) {
            return Ok(Some(model.clone()));
        }
        if !catalog_fallback {
            return Ok(None);
        }
        Ok(self.available.iter().find(flat_match).cloned())
    }

    /// Complete fixed Native resolveCliModel (model-resolver.ts:1922-2120).
    /// `self.available` is the preferred/authenticated set supplied by the
    /// Registry. The all-catalog and provider-local contexts share its alias
    /// runtime. Error help changes only `omp models` to the ARA command.
    pub fn resolve_cli_model(
        &self,
        cli_provider: Option<&str>,
        cli_model: Option<&str>,
        all_models: Vec<HostModelRef>,
        settings: &dyn ModelRoleLookup,
        preferences: ModelMatchPreferences,
    ) -> Result<ResolvedCliModel, CollapseError> {
        let Some(cli_model) = cli_model.filter(|value| !value.is_empty()) else {
            return Ok(ResolvedCliModel::default());
        };
        if all_models.is_empty() {
            return Ok(ResolvedCliModel {
                error: Some("No models available. Check your installation or add models to models.json.".into()),
                ..ResolvedCliModel::default()
            });
        }
        let cli_provider = cli_provider.filter(|value| !value.is_empty());
        let mut provider_map = HashMap::new();
        for model in &all_models {
            let provider = model_field(model, "provider");
            provider_map.insert(lower(&provider), provider);
        }
        let mut provider = cli_provider.and_then(|value| provider_map.get(&lower(&value.into())).cloned());
        if let Some(cli_provider) = cli_provider
            && provider.is_none()
        {
            return Ok(ResolvedCliModel {
                error: Some(format!(
                    "Unknown provider \"{cli_provider}\". Run \"ara models\" to see available providers/models."
                )),
                ..ResolvedCliModel::default()
            });
        }
        let catalog = self.with_available_models(all_models, preferences.clone())?;
        let trimmed_model = js_trim(cli_model);
        if provider.is_none() {
            if let Some(exact) = catalog.find_exact_cli_model(trimmed_model, &self.available, false)? {
                return ResolvedCliModel::exact(exact, None);
            }
            let (exact_base, exact_thinking) = split_thinking(trimmed_model, None, true);
            if let Some(level) = exact_thinking
                && let Some(exact) = catalog.find_exact_cli_model(exact_base, &self.available, false)?
            {
                return ResolvedCliModel::exact(exact, Some(level));
            }
        }
        let mut configured_patterns = None;
        if cli_provider.is_none() {
            let (bare_role_name, bare_role_thinking) = split_thinking(trimmed_model, None, true);
            let role_selector = if alias_prefix_length(trimmed_model).is_some() {
                Some(trimmed_model.to_owned())
            } else if settings.get_model_role(bare_role_name).is_some() {
                Some(match bare_role_thinking {
                    Some(level) => format!("@{bare_role_name}:{}", thinking_text(level)),
                    None => format!("@{bare_role_name}"),
                })
            } else {
                None
            };
            if let Some(role_selector) = role_selector {
                let (role_name, _) = split_thinking(&role_selector, alias_prefix_length(&role_selector), true);
                let configured_role = role_alias(role_name, settings);
                configured_patterns =
                    Some(resolve_configured_model_patterns(Some(&Value::String(role_selector.clone())), settings));
                let preferred = self.with_preferences(preferences.clone())?;
                let available_resolved = preferred.resolve_model_role_value(Some(&role_selector), settings)?;
                let resolved = if available_resolved.model.is_some() {
                    available_resolved
                } else {
                    catalog.resolve_model_role_value(Some(&role_selector), settings)?
                };
                if let Some(model) = resolved.model {
                    return Ok(ResolvedCliModel {
                        selector: Some(cli_selector(&model)?),
                        model: Some(model),
                        configured_patterns,
                        configured_role,
                        configured_pattern_index: resolved.matched_pattern_index,
                        thinking_level: resolved.thinking_level,
                        warning: resolved.warning,
                        error: None,
                    });
                }
                if configured_patterns.as_ref().is_some_and(|patterns| !patterns.is_empty()) {
                    return Ok(ResolvedCliModel {
                        configured_patterns,
                        configured_role,
                        warning: resolved.warning,
                        error: Some(format!(
                            "Model \"{trimmed_model}\" not found. Run \"ara models\" to see available models."
                        )),
                        ..ResolvedCliModel::default()
                    });
                }
            }
        }
        let mut pattern = trimmed_model.to_owned();
        if provider.is_none() {
            // Native infers this prefix from the original, untrimmed CLI value.
            if let Some((maybe_provider, suffix)) = cli_model.split_once('/')
                && let Some(canonical) = provider_map.get(&lower(&maybe_provider.into()))
            {
                provider = Some(canonical.clone());
                pattern = suffix.to_owned();
            }
        } else if let Some(provider) = &provider {
            let mut prefix = provider.units().to_vec();
            prefix.push(b'/' as u16);
            let prefix = WireString::from_units(prefix);
            if lower(&cli_model.into()).units().starts_with(lower(&prefix).units()) {
                pattern = String::from_utf16(&cli_model.encode_utf16().skip(prefix.len()).collect::<Vec<_>>())
                    .map_err(|_| CollapseError::new("CLI model pattern cannot be represented as UTF-8".into()))?;
            }
        }
        if let Some(provider) = &provider
            && let Some(exact) = catalog.references.resolve(provider, &pattern.as_str().into())?
        {
            return ResolvedCliModel::exact(exact, None);
        }
        let candidates = if let Some(provider) = &provider {
            catalog.available.iter().filter(|model| model_field(model, "provider") == *provider).cloned().collect()
        } else {
            self.available.clone()
        };
        let candidates = self.with_available_models(candidates, preferences)?;
        let mut parsed = candidates.parse_model_pattern(&pattern, false)?;
        if parsed.model.is_none() && provider.is_none() {
            parsed = catalog.parse_model_pattern(&pattern, false)?;
        }
        let Some(model) = parsed.model else {
            let display = match &provider {
                Some(provider) => format!(
                    "{}/{pattern}",
                    provider
                        .to_utf8()
                        .map_err(|_| CollapseError::new("CLI provider cannot be represented as UTF-8".into()))?
                ),
                None => cli_model.to_owned(),
            };
            return Ok(ResolvedCliModel {
                configured_patterns,
                warning: parsed.warning,
                error: Some(format!("Model \"{display}\" not found. Run \"ara models\" to see available models.")),
                ..ResolvedCliModel::default()
            });
        };
        let selector = if provider.is_some() {
            let selector = cli_selector(&model)?;
            Some(match parsed.upstream {
                Some(upstream) => format!("{selector}@{upstream}"),
                None => selector,
            })
        } else {
            None
        };
        Ok(ResolvedCliModel {
            model: Some(model),
            selector,
            thinking_level: parsed.thinking_level,
            warning: parsed.warning,
            ..ResolvedCliModel::default()
        })
    }
    /// Fixed enabled-scope direct matching omits role routing fallback.
    pub fn parse_scope_pattern(&self, pattern: &str) -> Result<ResolvedModelRoleValue, CollapseError> {
        self.parse_direct(pattern, true)
    }
    fn pick(&self, models: Vec<HostModelRef>) -> Option<HostModelRef> {
        // Stable sort retains Native array order when a preference comparator
        // returns equal (notably shared explicit usage/provider ranks).
        let mut models = models;
        models.sort_by(|a, b| self.preference.preferred(a, b));
        models.into_iter().next()
    }
    fn match_model(&self, pattern: &str, exact_only: bool) -> Result<Option<HostModelRef>, CollapseError> {
        if let Some((provider, id)) = js_trim(pattern).split_once('/')
            && !js_trim(provider).is_empty()
            && !js_trim(id).is_empty()
            && let Some(model) = self.references.resolve(&provider.into(), &id.into())?
        {
            return Ok(Some(model));
        }
        let normalized = lower(&pattern.into());
        let exact: Vec<_> =
            self.available.iter().filter(|model| lower(&model_field(model, "id")) == normalized).cloned().collect();
        if !exact.is_empty() {
            return Ok(
                self.pick(exact.into_iter().filter(|model| !provider_locked_cross_match(pattern, model)).collect())
            );
        }
        // Native profile ARN grammar is case-insensitive; keep its original
        // authored spelling when the reference index synthesizes the donor.
        if lower(&js_trim(pattern).into()).units().starts_with(&[b'a' as u16, b'r' as u16, b'n' as u16, b':' as u16])
            && let Some(model) = self.references.resolve(&"amazon-bedrock".into(), &pattern.into())?
        {
            return Ok(Some(model));
        }
        let alias = self
            .collapse
            .lock()
            .map_err(|_| CollapseError::new("Model alias state is unavailable".into()))?
            .resolve_bare_variant_selector(&pattern.into())?;
        let target = match &alias {
            Some(alias) => Some(alias.id.clone()),
            None => catalog_rules::strip_thinking_variant_suffix_utf16(pattern).map(WireString::from_units),
        };
        if let Some(target) = target {
            let matches: Vec<_> = self
                .available
                .iter()
                .filter(|model| lower(&model_field(model, "id")) == lower(&target))
                .cloned()
                .collect();
            if !matches.is_empty() {
                let preferred: Vec<_> = matches
                    .iter()
                    .filter(|model| {
                        alias.as_ref().is_some_and(|alias| alias.providers.contains(&model_field(model, "provider")))
                    })
                    .cloned()
                    .collect();
                return Ok(self.pick(if preferred.is_empty() { matches } else { preferred }));
            }
        }
        if exact_only {
            return Ok(None);
        }
        if let Some((provider, id)) = pattern.split_once('/') {
            let provider = lower(&provider.into());
            let candidates: Vec<_> = self
                .available
                .iter()
                .enumerate()
                .filter(|(_, model)| lower(&model_field(model, "provider")) == provider)
                .collect();
            if !candidates.is_empty() {
                if split_upstream_routing(id).is_some()
                    && candidates.iter().any(|(_, model)| supports_upstream_routing(model))
                {
                    return Ok(None);
                }
                let query = FuzzyIndex::new(&id.into());
                let mut scored: Vec<_> = candidates
                    .into_iter()
                    .filter_map(|(index, model)| {
                        fuzzy_score(&query, &self.fuzzy_ids[index]).map(|score| (score, model.clone()))
                    })
                    .collect();
                scored.sort_by(|(a_score, a), (b_score, b)| {
                    a_score.total_cmp(b_score).then_with(|| self.preference.fuzzy_tie(a, b))
                });
                return Ok(scored.into_iter().next().map(|(_, model)| model));
            }
        }
        let matches: Vec<_> = self
            .available
            .iter()
            .filter(|model| {
                contains_units(lower(&model_field(model, "id")).units(), normalized.units())
                    || text(model.spec(), "name")
                        .is_some_and(|name| contains_units(lower(&name).units(), normalized.units()))
            })
            .cloned()
            .collect();
        let aliases: Vec<_> = matches.iter().filter(|model| is_alias(&model_field(model, "id"))).cloned().collect();
        if !aliases.is_empty() {
            return Ok(self.pick(aliases));
        }
        let mut dated = matches;
        dated.sort_by(|a, b| self.collation.compare(&model_field(b, "id"), &model_field(a, "id")));
        let Some(first) = dated.first() else { return Ok(None) };
        let id = model_field(first, "id");
        Ok(self.pick(dated.into_iter().filter(|model| model_field(model, "id") == id).collect()))
    }
    fn parse_direct(&self, pattern: &str, allow_invalid_suffix: bool) -> Result<ResolvedModelRoleValue, CollapseError> {
        if let Some(model) = self.match_model(pattern, true)? {
            return Ok(ResolvedModelRoleValue::matched(model));
        }
        let (base, level) = split_thinking(pattern, None, true);
        if let Some(level) = level {
            if let Some(model) = self.match_model(pattern, false)?
                && lower(&model_field(&model, "id"))
                    .units()
                    .ends_with(&format!(":{}", thinking_text(level)).encode_utf16().collect::<Vec<_>>())
            {
                return Ok(ResolvedModelRoleValue::matched(model));
            }
            let mut result = self.parse_direct(base, allow_invalid_suffix)?;
            if result.model.is_some() {
                result.explicit_thinking_level = result.warning.is_none();
                result.thinking_level = result.explicit_thinking_level.then_some(level);
            }
            return Ok(result);
        }
        if let Some(model) = self.match_model(pattern, false)? {
            return Ok(ResolvedModelRoleValue::matched(model));
        }
        if !allow_invalid_suffix {
            return Ok(ResolvedModelRoleValue::empty());
        }
        let Some((prefix, suffix)) = pattern.rsplit_once(':') else { return Ok(ResolvedModelRoleValue::empty()) };
        let mut result = self.parse_direct(prefix, allow_invalid_suffix)?;
        if result.model.is_some() {
            result.thinking_level = None;
            result.explicit_thinking_level = false;
            result.warning =
                Some(format!("Invalid thinking level \"{suffix}\" in pattern \"{pattern}\". Using default instead."));
        }
        Ok(result)
    }
    pub fn parse_model_pattern(
        &self,
        pattern: &str,
        allow_invalid_thinking_selector_fallback: bool,
    ) -> Result<ResolvedModelRoleValue, CollapseError> {
        let direct = self.parse_direct(pattern, allow_invalid_thinking_selector_fallback)?;
        if direct.model.is_some() {
            return Ok(direct);
        }
        if let Some((base, upstream)) = split_upstream_routing(pattern) {
            let mut result = self.parse_direct(&base, allow_invalid_thinking_selector_fallback)?;
            if let Some(model) = &result.model
                && supports_upstream_routing(model)
            {
                result.model = Some(apply_upstream_routing(model, &upstream)?);
                result.upstream = Some(upstream);
                return Ok(result);
            }
        }
        Ok(direct)
    }
    pub fn resolve_model_role_value(
        &self,
        value: Option<&str>,
        settings: &dyn ModelRoleLookup,
    ) -> Result<ResolvedModelRoleValue, CollapseError> {
        let Some(value) = value.map(js_trim).filter(|value| !value.is_empty() && *value != "default") else {
            return Ok(ResolvedModelRoleValue::empty());
        };
        let patterns = resolve_configured_model_patterns(Some(&Value::String(value.to_owned())), settings);
        let mut warning = None;
        for (index, pattern) in patterns.iter().enumerate() {
            let mut result = self.parse_model_pattern(pattern, true)?;
            if let Some(model) = &result.model {
                result.matched_pattern_index = Some(index);
                if result.explicit_thinking_level {
                    result.thinking_level = clamp_role_thinking(model, result.thinking_level)?;
                }
                return Ok(result);
            }
            if warning.is_none() {
                warning = result.warning;
            }
        }
        Ok(ResolvedModelRoleValue { warning, ..ResolvedModelRoleValue::empty() })
    }
    /// Native resolveRoleSelection reads configured role values directly. An
    /// absent tiny/smol value is not @tiny/@smol and does not inherit the active
    /// model or automatically request the built-in priority chain.
    pub fn resolve_role_selection(
        &self,
        roles: &[&str],
        settings: &dyn ModelRoleLookup,
    ) -> Result<Option<ResolvedRoleSelection>, CollapseError> {
        for role in roles {
            let configured = settings.get_model_role(role);
            let result = self.resolve_model_role_value(configured.as_deref(), settings)?;
            if let Some(model) = result.model {
                return Ok(Some(ResolvedRoleSelection {
                    role: (*role).to_owned(),
                    model,
                    thinking_level: result.thinking_level,
                }));
            }
        }
        Ok(None)
    }
}

pub(crate) fn clamp_role_thinking(
    model: &HostModelRef,
    level: Option<ConfiguredThinkingLevel>,
) -> Result<Option<ConfiguredThinkingLevel>, CollapseError> {
    let Some(ConfiguredThinkingLevel::Concrete(concrete)) = level else { return Ok(level) };
    if matches!(concrete, ThinkingLevel::Inherit | ThinkingLevel::Off) {
        return Ok(level);
    }
    if !boolean(model.spec(), "reasoning").unwrap_or(false) {
        return Ok(level);
    }
    let Some(thinking) = model.spec().record("thinking") else { return Ok(level) };
    let levels = thinking
        .get("efforts")
        .and_then(WireValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(WireValue::as_string)
        .filter_map(|value| value.to_utf8().ok())
        .filter_map(|value| parse_thinking_level(&value))
        .collect::<Vec<_>>();
    if levels.contains(&concrete) {
        return Ok(level);
    }
    let order = |level| match level {
        ThinkingLevel::Minimal => 0,
        ThinkingLevel::Low => 1,
        ThinkingLevel::Medium => 2,
        ThinkingLevel::High => 3,
        ThinkingLevel::XHigh => 4,
        ThinkingLevel::Max => 5,
        _ => -1,
    };
    let mut clamped = None;
    for level in &levels {
        if order(*level) > order(concrete) {
            break;
        }
        clamped = Some(*level);
    }
    Ok(clamped.or_else(|| levels.first().copied()).map(ConfiguredThinkingLevel::Concrete).or(level))
}
fn revision_spelling_key(value: &WireString) -> WireString {
    let mut units = lower(value).units().to_vec();
    for index in 1..units.len().saturating_sub(1) {
        if units[index] == b'.' as u16
            && (b'0' as u16..=b'9' as u16).contains(&units[index - 1])
            && (b'0' as u16..=b'9' as u16).contains(&units[index + 1])
        {
            units[index] = b'-' as u16;
        }
    }
    WireString::from_units(units)
}
fn provider_locked_cross_match(pattern: &str, matched: &HostModelRef) -> bool {
    let Some((provider, id)) = pattern.split_once('/').filter(|(provider, _)| !provider.is_empty()) else {
        return false;
    };
    let provider = lower(&provider.into());
    if lower(&model_field(matched, "provider")) == provider {
        return false;
    }
    let target = revision_spelling_key(&id.into());
    bundled_provider_models(&provider)
        .iter()
        .any(|model| text(model, "id").is_some_and(|id| revision_spelling_key(&id) == target))
}
fn is_alias(id: &WireString) -> bool {
    let units = id.units();
    !(units.len() >= 9
        && units[units.len() - 9] == b'-' as u16
        && units[units.len() - 8..].iter().all(|unit| (b'0' as u16..=b'9' as u16).contains(unit)))
}
pub fn split_upstream_routing(pattern: &str) -> Option<(String, String)> {
    let at = pattern.rfind('@').filter(|at| *at > 0)?;
    let rest = &pattern[at + 1..];
    let (upstream, trailing) = rest.find(':').map(|colon| (&rest[..colon], &rest[colon..])).unwrap_or((rest, ""));
    let bytes = upstream.as_bytes();
    if bytes.is_empty()
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
        || !bytes.iter().all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
    {
        return None;
    }
    Some((format!("{}{trailing}", &pattern[..at]), upstream.to_owned()))
}
fn supports_upstream_routing(model: &HostModelRef) -> bool {
    is_host(model, "openrouter") || is_host(model, "vercelAIGateway")
}
/// Fixed config/model-resolver.ts:247-262. Catalog identity stays separate
/// from the execution request ID, and custom providers match by actual host.
pub fn format_model_string_with_routing(model: &HostModelRef) -> WireString {
    let mut units = model_field(model, "provider").units().to_vec();
    units.push(b'/' as u16);
    units.extend_from_slice(model_field(model, "id").units());
    let compat = model.spec().record("compatConfig");
    let has_key = |key: &str| compat.as_ref().is_some_and(|compat| compat.own_keys().contains(&key.into()));
    let routing_key = if is_host(model, "vercelAIGateway") && has_key("vercelGatewayRouting") {
        Some("vercelGatewayRouting")
    } else if is_host(model, "openrouter") && has_key("openRouterRouting") {
        Some("openRouterRouting")
    } else {
        None
    };
    if let Some(only) = routing_key
        .and_then(|key| model.spec().record("compatConfig")?.record(key))
        .and_then(|routing| routing.get("only").cloned())
        && let WireValue::Array(ref only) = only
        && only.len() == 1
        && let Some(upstream) = only[0].as_string().filter(|value| !value.units().is_empty())
    {
        units.push(b'@' as u16);
        units.extend_from_slice(upstream.units());
    }
    WireString::from_units(units)
}
fn is_host(model: &HostModelRef, host: &str) -> bool {
    model_matches_host(
        &serde_json::json!({
            "provider": String::from_utf16_lossy(model_field(model, "provider").units()),
            "baseUrl": String::from_utf16_lossy(model_field(model, "baseUrl").units()),
        }),
        host,
    )
}
fn apply_upstream_routing(model: &HostModelRef, upstream: &str) -> Result<HostModelRef, CollapseError> {
    let mut spec = model.spec().as_ref().clone();
    let mut compat = spread(&[&model.spec().record("compatConfig").unwrap_or_else(empty)]);
    compat.set(
        if is_host(model, "vercelAIGateway") { "vercelGatewayRouting" } else { "openRouterRouting" },
        WireValue::Object(vec![("only".into(), WireValue::Array(vec![WireValue::String(upstream.into())]))]),
    );
    spec.set_record("compat", &compat);
    build_host_model(&spec, model.headers().clone())
}

fn contains_units(haystack: &[u16], needle: &[u16]) -> bool {
    find_units(haystack, needle, 0).is_some()
}
fn find_units(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return (from <= haystack.len()).then_some(from);
    }
    haystack.get(from..)?.windows(needle.len()).position(|window| window == needle).map(|index| index + from)
}

// Complete fixed fuzzy-match score dependency. Fuzzy state belongs to this
// resolver snapshot, so repeated fallback patterns reuse candidate indexes.
// Positions and character scoring use UTF-16 units, as the Native utility.
struct SearchWord {
    text: Vec<u16>,
    index: usize,
    ordinal: usize,
}
struct FuzzyIndex {
    normalized: Vec<u16>,
    compact: Vec<u16>,
    starts: HashSet<usize>,
    words: Vec<SearchWord>,
}
fn normalize_search(value: &WireString) -> String {
    static ACRONYM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([A-Z]+)([A-Z][a-z])").unwrap());
    static CAMEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([a-z0-9])([A-Z])").unwrap());
    static NON_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\p{Letter}\p{Mark}\p{Number}]+").unwrap());
    // Lone surrogates and replacement characters both lie outside Letter/Mark/
    // Number and become separators; all actual matching indexes remain lossless.
    let source = String::from_utf16_lossy(value.units());
    let source = ACRONYM.replace_all(&source, "$1 $2");
    let source = CAMEL.replace_all(&source, "$1 $2").to_lowercase();
    NON_WORD.replace_all(&source, " ").trim().to_owned()
}
impl FuzzyIndex {
    fn new(value: &WireString) -> Self {
        let normalized = normalize_search(value);
        let mut words = Vec::new();
        let mut starts = HashSet::new();
        let mut compact = Vec::new();
        let mut index = 0;
        for (ordinal, word) in normalized.split(' ').filter(|word| !word.is_empty()).enumerate() {
            let text: Vec<_> = word.encode_utf16().collect();
            starts.insert(compact.len());
            compact.extend(&text);
            index += text.len() + 1;
            words.push(SearchWord { index: index - text.len() - 1, text, ordinal });
        }
        Self { normalized: normalized.encode_utf16().collect(), compact, starts, words }
    }
}
fn boundary_phrase(text: &[u16], needle: &[u16]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let boundary = |at| {
        (at == 0 || text[at - 1] == b' ' as u16)
            && (at + needle.len() == text.len() || text[at + needle.len()] == b' ' as u16)
    };
    let first = find_units(text, needle, 0)?;
    if boundary(first) {
        return Some(first);
    }
    if first == 0 || text[first - 1] == b' ' as u16 || needle.len() < 3 {
        return None;
    }
    let mut from = first + 1;
    while let Some(at) = find_units(text, needle, from) {
        if boundary(at) {
            return Some(at);
        }
        from = at + 1;
    }
    None
}
fn compact_start(index: &FuzzyIndex, needle: &[u16]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let first = find_units(&index.compact, needle, 0)?;
    if index.starts.contains(&first) {
        return Some(first);
    }
    if needle.len() < 3 {
        return None;
    }
    let mut from = first + 1;
    while let Some(at) = find_units(&index.compact, needle, from) {
        if index.starts.contains(&at) {
            return Some(at);
        }
        from = at + 1;
    }
    None
}
fn character_score(query: &[u16], text: &[u16]) -> Option<(f64, usize)> {
    if query.is_empty() {
        return Some((0.0, 0));
    }
    if query.len() > text.len() {
        return None;
    }
    let mut cursor = 0;
    let mut first = None;
    let mut last = None;
    let mut consecutive = 0;
    let mut score = 0.0;
    for (index, unit) in text.iter().enumerate() {
        if cursor == query.len() {
            break;
        }
        if *unit != query[cursor] {
            continue;
        }
        first.get_or_insert(index);
        if last.is_some_and(|last| last + 1 == index) || (last.is_none() && index == 0) {
            consecutive += 1;
            score -= consecutive as f64 * 5.0;
        } else {
            consecutive = 0;
            if let Some(last) = last {
                score += (index - last - 1) as f64 * 2.0;
            }
        }
        score += index as f64 * 0.1;
        last = Some(index);
        cursor += 1;
    }
    (cursor == query.len()).then(|| (score, last.unwrap() - first.unwrap() + 1))
}
fn token_word(token: &[u16], word: &SearchWord) -> Option<f64> {
    let score = if word.text == token {
        -200.0
    } else if word.text.starts_with(token) {
        -170.0 + (word.text.len() - token.len()) as f64 * 0.5
    } else if token.starts_with(&word.text) && token.len() - word.text.len() <= 2 {
        -150.0 + (token.len() - word.text.len()) as f64
    } else if let Some(at) = find_units(&word.text, token, 0) {
        -20.0 + at as f64
    } else {
        let (score, span) = character_score(token, &word.text)?;
        if span > (token.len() + 2).max((token.len() as f64 * 1.8).ceil() as usize) {
            return None;
        }
        -40.0 + score
    };
    Some(score + word.index as f64 * 0.01)
}
fn token_direct(token: &[u16], index: &FuzzyIndex) -> Option<f64> {
    if token.is_empty() {
        return Some(0.0);
    }
    let mut best = compact_start(index, token).map(|at| -140.0 + at as f64 * 0.01);
    let mut admit = |score| {
        if best.is_none_or(|best| score < best) {
            best = Some(score);
        }
    };
    for word in &index.words {
        if let Some(score) = token_word(token, word) {
            admit(score);
        }
    }
    if (2..=4).contains(&token.len()) {
        let mut cursor = 0;
        let mut first = None;
        let mut last = None;
        let mut first_index = 0;
        for word in &index.words {
            if word.text.first() != token.get(cursor) {
                continue;
            }
            if first.is_none() {
                first = Some(word.ordinal);
                first_index = word.index;
            }
            last = Some(word.ordinal);
            cursor += 1;
            if cursor == token.len() {
                break;
            }
        }
        if cursor == token.len()
            && let (Some(first), Some(last)) = (first, last)
        {
            let span = last - first + 1;
            if span <= token.len() + 2 {
                admit(-30.0 + span as f64 * 4.0 - token.len() as f64 * 2.0 + first_index as f64 * 0.01);
            }
        }
    }
    best
}
fn token_score(token: &[u16], index: &FuzzyIndex) -> Option<f64> {
    if let Some(score) = token_direct(token, index) {
        return Some(score);
    }
    let alpha = |unit| (b'a' as u16..=b'z' as u16).contains(&unit);
    let digit = |unit| (b'0' as u16..=b'9' as u16).contains(&unit);
    let mut best = None;
    for at in 0..token.len().saturating_sub(1) {
        let (a, b) = (token[at], token[at + 1]);
        if !(alpha(a) && digit(b) || digit(a) && alpha(b)) {
            continue;
        }
        let mut variant = token.to_vec();
        variant.swap(at, at + 1);
        if let Some(score) = token_direct(&variant, index) {
            let score = score + 5.0;
            if best.is_none_or(|best| score < best) {
                best = Some(score);
            }
        }
    }
    best
}
fn fuzzy_score(query: &FuzzyIndex, index: &FuzzyIndex) -> Option<f64> {
    if query.normalized.is_empty() {
        return Some(0.0);
    }
    if index.words.is_empty() {
        return None;
    }
    let mut score = 0.0;
    if let Some(at) = boundary_phrase(&index.normalized, &query.normalized) {
        score -= 1000.0;
        score += at as f64 * 0.01;
    }
    if let Some(at) = compact_start(index, &query.compact) {
        score -= 1200.0;
        score += at as f64 * 0.01;
    }
    for token in &query.words {
        score += token_score(&token.text, index)?;
    }
    Some(score)
}

// Score-only fixed TUI fuzzyFilter projection for non-model Host consumers.
// Keep input spelling/duplicates and stable input order on equal scores.
pub(crate) fn fuzzy_filter_texts(items: &[String], query: &str) -> Vec<String> {
    let query = FuzzyIndex::new(&query.into());
    let mut scored: Vec<_> = items.iter().filter_map(|item| {
        fuzzy_score(&query, &FuzzyIndex::new(&item.as_str().into())).map(|score| (score, item))
    }).collect();
    scored.sort_by(|(a, _), (b, _)| a.total_cmp(b));
    scored.into_iter().map(|(_, item)| String::from(item.as_str())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_patch::HeaderSlot;
    use serde_json::json;

    fn model(provider: &str, id: &str) -> HostModelRef {
        build_host_model(&crate::model_collapse::VariantSpec::from_json(&json!({
            "provider": provider, "id": id, "name": id, "api": "openai-completions", "baseUrl": "https://test.invalid",
            "reasoning": true, "input": ["text"], "contextWindow": 128000, "maxTokens": 8192,
            "thinking": {"mode": "effort", "efforts": ["low", "medium", "high"], "defaultLevel": "medium"}
        })), HeaderSlot::Absent).unwrap()
    }
    fn make_resolver(models: Vec<HostModelRef>) -> ModelRoleResolver {
        ModelRoleResolver::new(
            models,
            Arc::new(Mutex::new(CollapseRuntime::new().unwrap())),
            ModelMatchPreferences::default(),
        )
        .unwrap()
    }
    fn id(result: &ResolvedModelRoleValue) -> Option<String> {
        result.model.as_ref().and_then(|model| text(model.spec(), "id")).and_then(|id| id.to_utf8().ok())
    }
    #[test]
    fn role_values_alias_defaults_and_cycle_inheritance_form_one_ordered_family() {
        assert_eq!(normalize_model_pattern_list(Some(&json!([" a,b ", "", " c "]))), ["a", "b", "c"]);
        assert_eq!(model_role_value(Some(&json!(["a", 2]))), None);
        let settings = |role: &str| (role == "default").then(|| "local/custom".into());
        assert_eq!(resolve_configured_model_patterns(Some(&json!("@smol")), &settings), ["local/custom"]);
        let tiny = resolve_configured_model_patterns(Some(&json!("@tiny")), &settings);
        assert_eq!(tiny, role_defaults("smol"));
        assert!(!tiny.contains(&"local/custom".into()));
        let self_alias = |role: &str| (role == "default").then(|| "@smol:high".into());
        let patterns = resolve_configured_model_patterns(Some(&json!("@smol")), &self_alias);
        assert_eq!(patterns[0], format!("{}:high", role_defaults("smol")[0]));
        let cross_alias = |role: &str| match role {
            "default" => Some("@slow".into()),
            "slow" => Some("openai/strong".into()),
            _ => None,
        };
        assert_eq!(resolve_configured_model_patterns(Some(&json!("@smol")), &cross_alias), ["openai/strong"]);
        let cycle = |role: &str| (role == "default").then(|| "@default".into());
        assert!(
            resolve_configured_model_patterns(Some(&json!("@default")), &cycle)
                .iter()
                .all(|pattern| pattern == "@default")
        );
        assert_eq!(resolve_explicit_model_role(Some(&json!("openai/a,pi/smol:max")), &settings), Some("smol".into()));
    }
    #[test]
    fn explicit_role_selection_missing_values_and_fallback_order_are_native() {
        let resolver = make_resolver(vec![model("local", "custom"), model("openai", "small")]);
        let missing = |_role: &str| None;
        assert!(resolver.resolve_role_selection(&["tiny", "smol"], &missing).unwrap().is_none());
        let settings = |role: &str| match role {
            "tiny" => Some("missing/nope,openai/small:low".into()),
            "smol" => Some("local/custom".into()),
            _ => None,
        };
        let selected = resolver.resolve_role_selection(&["tiny", "smol"], &settings).unwrap().unwrap();
        assert_eq!(selected.role, "tiny");
        assert_eq!(model_field(&selected.model, "id"), "small".into());
        assert_eq!(selected.thinking_level, Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::Low)));
        let resolved = resolver.resolve_model_role_value(Some("missing/nope,openai/small:auto"), &missing).unwrap();
        assert_eq!(resolved.matched_pattern_index, Some(1));
        assert_eq!(resolved.thinking_level, Some(ConfiguredThinkingLevel::Auto));
    }
    #[test]
    fn suffix_literal_warning_fuzzy_and_routing_precedence_are_grouped() {
        let resolver = make_resolver(vec![
            model("openai", "kimi-for-coding"),
            model("openai", "kimi-for-coding-highspeed"),
            model("local", "glm:max"),
            model("openrouter", "vendor/router:low"),
            model("openrouter", "vendor/router"),
        ]);
        let fixed = resolver.parse_model_pattern("openai/kimi-for-coding:high", true).unwrap();
        assert_eq!(id(&fixed).as_deref(), Some("kimi-for-coding"));
        assert!(fixed.explicit_thinking_level);
        let literal = resolver.parse_model_pattern("local/glm:max", true).unwrap();
        assert!(!literal.explicit_thinking_level);
        let invalid = resolver.parse_model_pattern("openai/kimi-for-coding:bad:high", true).unwrap();
        assert_eq!(id(&invalid).as_deref(), Some("kimi-for-coding"));
        assert!(invalid.warning.is_some());
        assert!(!invalid.explicit_thinking_level);
        assert!(resolver.parse_model_pattern("openai/kimi-for-coding:bad", false).unwrap().model.is_none());
        let routed = resolver.parse_model_pattern("openrouter/vendor/router@cerebras:high", true).unwrap();
        assert_eq!(routed.upstream.as_deref(), Some("cerebras"));
        assert_eq!(routed.thinking_level, Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::High)));
        let compat = routed.model.unwrap().spec().record("compatConfig").unwrap();
        assert_eq!(
            compat.record("openRouterRouting").unwrap().get("only").unwrap(),
            &WireValue::Array(vec![WireValue::String("cerebras".into())])
        );
    }
    #[test]
    fn availability_order_preferences_aliases_and_dated_ids_are_grouped() {
        let no_settings = |_role: &str| None;
        let models = vec![model("openrouter", "gpt-5.4-mini"), model("openai-codex", "gpt-5.4-mini")];
        let resolver = ModelRoleResolver::new(
            models.clone(),
            Arc::new(Mutex::new(CollapseRuntime::new().unwrap())),
            ModelMatchPreferences::default(),
        )
        .unwrap();
        let tiny = resolver.resolve_model_role_value(Some("@tiny"), &no_settings).unwrap();
        assert_eq!(model_field(tiny.model.as_ref().unwrap(), "provider"), "openai-codex".into());
        let recent = ModelRoleResolver::new(
            models,
            Arc::new(Mutex::new(CollapseRuntime::new().unwrap())),
            ModelMatchPreferences { usage_order: vec!["openrouter/gpt-5.4-mini".into()], ..Default::default() },
        )
        .unwrap();
        assert_eq!(
            model_field(recent.parse_model_pattern("gpt-5.4-mini", true).unwrap().model.as_ref().unwrap(), "provider"),
            "openrouter".into()
        );
        let dated = make_resolver(vec![model("local", "alpha-20240101"), model("local", "alpha-20250101")]);
        assert_eq!(id(&dated.parse_model_pattern("alpha", true).unwrap()).as_deref(), Some("alpha-20250101"));
        let aliases = make_resolver(vec![model("local", "alpha-20250101"), model("local", "alpha-latest")]);
        assert_eq!(id(&aliases.parse_model_pattern("alpha", true).unwrap()).as_deref(), Some("alpha-latest"));

        let excluded = model("outside", "shared");
        let first = model("scope-a", "shared");
        let recent = model("scope-b", "shared");
        let router = model("openrouter", "vendor/router");
        use crate::model_config_values::{
            HeaderConfigRecord, HeaderResolutionOptions, HeaderSource, create_live_config_headers,
        };
        let private_donor = create_live_config_headers(
            &[Some(HeaderSource::Config(HeaderConfigRecord::default()))],
            HeaderResolutionOptions::default(),
        )
        .unwrap();
        let router = crate::model_patch::HostModel::new(
            router.spec().clone(),
            HeaderSlot::Source(HeaderSource::Live(private_donor.clone())),
        )
        .unwrap();
        let whole = make_resolver(vec![excluded, first.clone(), recent.clone(), router.clone()]);
        let scoped = whole
            .with_available_models(
                vec![first, recent.clone(), router.clone()],
                ModelMatchPreferences {
                    usage_order: vec!["outside/shared".into(), "scope-b/shared".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(Arc::ptr_eq(&whole.collapse, &scoped.collapse), "scoping must retain the Registry alias owner");
        let remembered = scoped.parse_model_pattern("shared", true).unwrap().model.unwrap();
        assert!(Arc::ptr_eq(&remembered, &recent), "resolve within the scope before choosing the same-id donor");
        assert!(scoped.parse_model_pattern("outside/shared", true).unwrap().model.is_none());
        assert!(Arc::ptr_eq(
            &scoped.parse_model_pattern("openrouter/vendor/router", true).unwrap().model.unwrap(),
            &router
        ));
        let routed = scoped.parse_model_pattern("openrouter/vendor/router@cerebras:high", true).unwrap();
        assert_eq!(
            format_model_string_with_routing(routed.model.as_ref().unwrap()).to_utf8().unwrap(),
            "openrouter/vendor/router@cerebras"
        );
        assert_eq!(routed.thinking_level, Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::High)));
        private_donor.set("X-Scoped-Donor", "same-owner");
        match routed.model.as_ref().unwrap().headers() {
            HeaderSlot::Source(HeaderSource::Live(headers)) => assert!(headers.delete("X-Scoped-Donor")),
            _ => panic!("scope-local routing lost its private header donor"),
        }
    }

    #[test]
    fn cli_exact_auth_catalog_and_role_precedence_are_grouped() {
        let cursor = model("cursor", "default");
        let first = model("first", "same");
        let second = model("second", "same");
        let role_target = model("local", "role-target");
        let named_task = model("native", "task");
        let mirror = model("openrouter", "not-a-provider/flat-model");
        let all = vec![
            cursor.clone(),
            first.clone(),
            second.clone(),
            role_target.clone(),
            named_task.clone(),
            mirror.clone(),
        ];
        let available = make_resolver(vec![first.clone(), second.clone(), role_target.clone(), mirror.clone()]);
        let preferences = ModelMatchPreferences { usage_order: vec!["second/same".into()], ..Default::default() };
        let settings = |role: &str| matches!(role, "default" | "task").then(|| "local/role-target".to_owned());
        for (selector, expected) in [
            ("same", &first),
            ("second/same", &second),
            ("native/task", &named_task),
            ("cursor/default", &cursor),
            ("default", &role_target),
            ("task", &role_target),
            ("not-a-provider/flat-model", &mirror),
        ] {
            let result =
                available.resolve_cli_model(None, Some(selector), all.clone(), &settings, preferences.clone()).unwrap();
            assert!(result.error.is_none(), "selector {selector}");
            assert!(Arc::ptr_eq(result.model.as_ref().unwrap(), expected), "selector {selector}");
        }
        let authenticated_task =
            available.with_available_models(vec![named_task.clone()], preferences.clone()).unwrap();
        for (selector, thinking) in
            [("task", None), ("task:xhigh", Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::XHigh)))]
        {
            let result = authenticated_task
                .resolve_cli_model(None, Some(selector), all.clone(), &settings, preferences.clone())
                .unwrap();
            assert!(Arc::ptr_eq(result.model.as_ref().unwrap(), &named_task));
            assert_eq!(result.thinking_level, thinking, "direct exact suffixes are not role-clamped");
            assert!(result.configured_role.is_none());
        }
        let no_roles = |_role: &str| None;
        let weak =
            available.resolve_cli_model(None, Some("default"), all.clone(), &no_roles, preferences.clone()).unwrap();
        assert!(Arc::ptr_eq(weak.model.as_ref().unwrap(), &cursor));
        assert!(weak.selector.is_none(), "unqualified generic catalog fallback has no pinned selector");
        for selector in [None, Some("")] {
            let absent =
                available.resolve_cli_model(Some("unknown"), selector, vec![], &settings, preferences.clone()).unwrap();
            assert!(absent.model.is_none() && absent.error.is_none());
        }
        let empty = available.resolve_cli_model(None, Some("same"), vec![], &settings, preferences.clone()).unwrap();
        assert_eq!(
            empty.error.as_deref(),
            Some("No models available. Check your installation or add models to models.json.")
        );
        let unknown = available.resolve_cli_model(Some("unknown"), Some("same"), all, &settings, preferences).unwrap();
        assert_eq!(
            unknown.error.as_deref(),
            Some("Unknown provider \"unknown\". Run \"ara models\" to see available providers/models.")
        );
        let catalog_reference = model("zai", "glm-5");
        let qualified_flat = model("vercel-ai-gateway", "zai/glm-5");
        let mirror = model("openrouter", "openai/flat-model");
        let ambiguous_catalog =
            vec![model("openai", "other"), qualified_flat.clone(), catalog_reference.clone(), mirror.clone()];
        let available = available
            .with_available_models(vec![qualified_flat, mirror.clone()], ModelMatchPreferences::default())
            .unwrap();
        for (selector, expected) in [("zai/glm-5", &catalog_reference), ("openai/flat-model", &mirror)] {
            let result = available
                .resolve_cli_model(
                    None,
                    Some(selector),
                    ambiguous_catalog.clone(),
                    &no_roles,
                    ModelMatchPreferences::default(),
                )
                .unwrap();
            assert!(
                Arc::ptr_eq(result.model.as_ref().unwrap(), expected),
                "catalog reference precedes flat match only when its provider carries that id"
            );
        }
    }

    #[test]
    fn cli_role_chains_aliases_preferences_clamp_and_unresolved_metadata_are_grouped() {
        let catalog_first = model("catalog", "role-primary");
        let usable = model("local", "role-target");
        let first = model("first", "same");
        let recent = model("second", "same");
        let all = vec![catalog_first.clone(), usable.clone(), first.clone(), recent.clone()];
        let available = make_resolver(vec![usable.clone(), first, recent.clone()]);
        let preferences = ModelMatchPreferences { usage_order: vec!["second/same".into()], ..Default::default() };
        let settings = |role: &str| match role {
            "task" => Some("catalog/role-primary:high,local/role-target:low".into()),
            "default" | "fable" => Some("local/role-target".into()),
            "preferred" => Some("same:high".into()),
            "role" => Some("missing/not-registered".into()),
            _ => None,
        };
        let chain =
            available.resolve_cli_model(None, Some("task"), all.clone(), &settings, preferences.clone()).unwrap();
        assert!(
            Arc::ptr_eq(chain.model.as_ref().unwrap(), &usable),
            "preferred pass considers the complete ordered chain"
        );
        assert_eq!(chain.configured_role.as_deref(), Some("task"));
        assert_eq!(chain.configured_pattern_index, Some(1));
        assert_eq!(
            chain.configured_patterns,
            Some(vec!["catalog/role-primary:high".into(), "local/role-target:low".into()])
        );
        assert_eq!(chain.thinking_level, Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::Low)));
        let unavailable = available.with_available_models(vec![], preferences.clone()).unwrap();
        let fallback =
            unavailable.resolve_cli_model(None, Some("task"), all.clone(), &settings, preferences.clone()).unwrap();
        assert!(Arc::ptr_eq(fallback.model.as_ref().unwrap(), &catalog_first));
        assert_eq!(fallback.configured_pattern_index, Some(0));
        for selector in ["@fable:xhi", "pi/fable:xhi", "fable:xhi", "*:xhi", "default:xhi"] {
            let result =
                available.resolve_cli_model(None, Some(selector), all.clone(), &settings, preferences.clone()).unwrap();
            assert!(Arc::ptr_eq(result.model.as_ref().unwrap(), &usable), "alias {selector}");
            assert_eq!(result.thinking_level, Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::High)));
            assert_eq!(result.configured_patterns, Some(vec!["local/role-target:xhigh".into()]));
        }
        let preferred =
            available.resolve_cli_model(None, Some("preferred"), all.clone(), &settings, preferences.clone()).unwrap();
        assert!(Arc::ptr_eq(preferred.model.as_ref().unwrap(), &recent), "role matching uses supplied preferences");
        let deferred =
            available.resolve_cli_model(None, Some("role"), all.clone(), &settings, preferences.clone()).unwrap();
        assert!(deferred.model.is_none(), "a configured but unresolved role must not fuzzy-match role-target");
        assert_eq!(deferred.configured_patterns, Some(vec!["missing/not-registered".into()]));
        assert_eq!(deferred.configured_role.as_deref(), Some("role"));
        assert_eq!(
            deferred.error.as_deref(),
            Some("Model \"role\" not found. Run \"ara models\" to see available models.")
        );
        let plain = available.resolve_cli_model(None, Some("role"), all, &|_role: &str| None, preferences).unwrap();
        assert!(plain.model.is_some(), "without a configured role the same input remains a generic pattern");
    }

    #[test]
    fn cli_provider_lock_literal_suffix_invalid_suffix_and_routed_selectors_are_grouped() {
        let base = model("local", "unit");
        let max = model("local", "unit:max");
        let auto = model("local", "unit:auto");
        let router = model("openrouter", "vendor/router");
        let bedrock = model("amazon-bedrock", "default");
        let all = vec![base.clone(), max.clone(), auto.clone(), router.clone(), bedrock];
        let available = make_resolver(all.clone());
        let no_roles = |_role: &str| None;
        for (provider, selector, expected, level) in [
            (None, "local/unit:max", &max, None),
            (Some("LOCAL"), "LOCAL/unit:auto", &auto, None),
            (Some("local"), "unit:high", &base, Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::High))),
            (None, "unit:high", &base, Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::High))),
        ] {
            let result = available
                .resolve_cli_model(provider, Some(selector), all.clone(), &no_roles, ModelMatchPreferences::default())
                .unwrap();
            assert!(Arc::ptr_eq(result.model.as_ref().unwrap(), expected), "selector {selector}");
            assert_eq!(result.thinking_level, level);
        }
        for (provider, selector) in
            [(Some("local"), "unit:bad"), (None, "unit:bad"), (Some("local"), "openrouter/vendor/router")]
        {
            let result = available
                .resolve_cli_model(provider, Some(selector), all.clone(), &no_roles, ModelMatchPreferences::default())
                .unwrap();
            assert!(result.model.is_none() && result.error.is_some(), "provider {provider:?}, selector {selector}");
            assert!(result.warning.is_none(), "invalid CLI thinking suffix never defaults with a warning");
        }
        for (provider, selector, expected_selector) in [
            (None, "openrouter/vendor/router@cerebras:high", Some("openrouter/vendor/router@cerebras")),
            (Some("openrouter"), "vendor/router@cerebras:high", Some("openrouter/vendor/router@cerebras")),
            (None, "router@cerebras:high", None),
        ] {
            let result = available
                .resolve_cli_model(provider, Some(selector), all.clone(), &no_roles, ModelMatchPreferences::default())
                .unwrap();
            assert_eq!(result.selector.as_deref(), expected_selector);
            assert_eq!(model_field(result.model.as_ref().unwrap(), "id"), "vendor/router".into());
            assert_eq!(
                format_model_string_with_routing(result.model.as_ref().unwrap()).to_utf8().unwrap(),
                "openrouter/vendor/router@cerebras"
            );
            assert_eq!(result.thinking_level, Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::High)));
        }
        let raw_route = available
            .resolve_cli_model(
                None,
                Some("openrouter/vendor/router-20251222:nitro"),
                all.clone(),
                &no_roles,
                ModelMatchPreferences::default(),
            )
            .unwrap();
        assert_eq!(model_field(raw_route.model.as_ref().unwrap(), "id"), "vendor/router-20251222:nitro".into());
        assert!(raw_route.thinking_level.is_none());
        let profile = "arn:aws:bedrock:us-east-2:1234567890:application-inference-profile/company-opus-48";
        let synthesized = available
            .resolve_cli_model(
                Some("amazon-bedrock"),
                Some(&format!("{profile}:off")),
                all.clone(),
                &no_roles,
                ModelMatchPreferences::default(),
            )
            .unwrap();
        assert_eq!(model_field(synthesized.model.as_ref().unwrap(), "id"), profile.into());
        assert_eq!(synthesized.thinking_level, Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::Off)));
        let uppercase_profile = profile.replacen("arn:", "ARN:", 1);
        let bare_profile = available
            .resolve_cli_model(None, Some(&uppercase_profile), all, &no_roles, ModelMatchPreferences::default())
            .unwrap();
        assert_eq!(
            model_field(bare_profile.model.as_ref().unwrap(), "id"),
            uppercase_profile.as_str().into(),
            "the i-grammar must preserve authored ARN spelling"
        );
        assert!(bare_profile.thinking_level.is_none());
        let mirror = model("openrouter", "anthropic/claude-opus-5");
        let locked = make_resolver(vec![])
            .resolve_cli_model(
                None,
                Some("anthropic/claude-opus-5"),
                vec![mirror],
                &no_roles,
                ModelMatchPreferences::default(),
            )
            .unwrap();
        assert!(
            locked.model.is_none() && locked.error.is_some(),
            "a known bundled provider id cannot cross-match a flat aggregator id"
        );
    }
    #[test]
    fn fuzzy_dependency_word_local_shadow_rescan_and_swapped_numbers_are_grouped() {
        let score =
            |query: &str, text: &str| fuzzy_score(&FuzzyIndex::new(&query.into()), &FuzzyIndex::new(&text.into()));
        assert!(score("image provider", "image provider").is_some());
        assert!(score("image provider", "i make a good example of unrelated words and provider").is_none());
        assert!(score("image", "reimage image provider").unwrap() < score("image", "reimage provider").unwrap());
        assert!(score("gpt4", "GPT4").unwrap() < score("gpt4", "gpt5").unwrap_or(f64::INFINITY));
        assert!(score("4gpt", "g4pt").is_some());
        assert!(score("\u{1F600}", "anything").is_some());
    }
}
