//! Pure retry fallback selection for the reference host.
//!
//! Ported from fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d:
//! - packages/coding-agent/src/session/retry-fallback-chains.ts (chain helpers)
//! - packages/coding-agent/src/config/model-resolver.ts (selector parsing/formatting)
//! - packages/coding-agent/src/thinking.ts (selector abbreviations and auto)
//!
//! Model application, credential rotation, usage eligibility, cooldown and
//! restoration remain host responsibilities. The existing RPC retry owner
//! implements backoff; this module does not duplicate it.
//!
//! Malformed selected chain values produce an explicit Rust error instead of
//! JavaScript's incidental iterable/type errors. Validation retains the
//! original warnings, including deferred discovery warnings.
//!
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in
//! all copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
//! THE SOFTWARE.

use std::{cmp::Ordering, collections::HashSet, fmt};

use serde_json::{Map, Value};

/// Raw settings are retained so validation can report malformed chain values.
pub type RetryFallbackChains = Map<String, Value>;

/// Only the registry observations needed by selector and chain resolution.
pub trait RetryFallbackModelLookup {
    fn contains_model(&self, provider: &str, id: &str) -> bool;
    fn has_provider(&self, provider: &str) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThinkingLevel {
    Inherit,
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl ThinkingLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfiguredThinkingLevel {
    Concrete(ThinkingLevel),
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryFallbackRevertPolicy {
    Never,
    CooldownExpiry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryFallbackSelector {
    /// Trimmed original text; deduplication is by this text, not model identity.
    pub raw: String,
    pub provider: String,
    pub id: String,
    pub thinking_level: Option<ThinkingLevel>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryFallbackWildcard {
    pub provider: String,
    pub id_prefix: Option<String>,
}

/// Registry projection, independent of catalogue implementation types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentModel {
    pub provider: String,
    pub id: String,
    /// Host-validated single nonempty `routing.only` entry for an OpenRouter or
    /// Vercel gateway model. Multiple routes and other hosts project to `None`.
    pub single_upstream_route: Option<String>,
}

impl CurrentModel {
    fn plain_selector(&self) -> String {
        format!("{}/{}", self.provider, self.id)
    }
}

pub struct RetryFallbackResolutionContext<'a> {
    /// Pre-expanded chains; hosts can include extra roles such as subagents.
    pub chains: &'a RetryFallbackChains,
    pub get_model_role: &'a dyn Fn(&str) -> Option<String>,
    pub model_lookup: &'a dyn RetryFallbackModelLookup,
}

/// State representation only; this module does not apply or restore models.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveRetryFallbackState {
    pub role: String,
    pub original_selector: String,
    pub original_thinking_level: Option<ConfiguredThinkingLevel>,
    pub last_applied_fallback_thinking_level: Option<ConfiguredThinkingLevel>,
    pub pinned: bool,
    /// A routing decision is not evidence of produced work. The host sets this
    /// only after a turn on the fallback model settles successfully.
    pub served: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServingModel {
    pub selector: String,
    pub is_fallback: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RetryFallbackCandidateOptions {
    pub allow_missing_primary: bool,
    pub wrap_around: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetryFallbackError {
    ChainNotArray { key: String },
    NonStringSelector { key: String, index: usize },
}

impl fmt::Display for RetryFallbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChainNotArray { key } => write!(f, "fallback chain '{key}' is not an array"),
            Self::NonStringSelector { key, index } => {
                write!(f, "fallback chain '{key}' entry {index} is not a string")
            }
        }
    }
}

impl std::error::Error for RetryFallbackError {}

/// Exact selectors or unambiguous prefixes of at least two characters.
pub fn parse_thinking_level(value: &str) -> Option<ThinkingLevel> {
    const LEVELS: [ThinkingLevel; 8] = [
        ThinkingLevel::Inherit,
        ThinkingLevel::Off,
        ThinkingLevel::Minimal,
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
        ThinkingLevel::XHigh,
        ThinkingLevel::Max,
    ];
    if let Some(level) = LEVELS.iter().find(|level| level.as_str() == value) {
        return Some(*level);
    }
    if value.len() < 2 {
        return None;
    }
    let mut matches = LEVELS.iter().filter(|level| level.as_str().starts_with(value));
    let matched = matches.next().copied()?;
    matches.next().is_none().then_some(matched)
}

pub fn concrete_thinking_level(level: Option<ConfiguredThinkingLevel>) -> Option<ThinkingLevel> {
    match level {
        Some(ConfiguredThinkingLevel::Concrete(level)) => Some(level),
        Some(ConfiguredThinkingLevel::Auto) | None => None,
    }
}

/// OMP's ordinary model parser uses the first slash even when the registry
/// contains provider names with slashes; their special handling is wildcard-only.
pub fn parse_retry_fallback_selector(
    selector: &str,
    model_lookup: Option<&dyn RetryFallbackModelLookup>,
) -> Option<RetryFallbackSelector> {
    let raw = ara_prompt::js::trim(selector);
    let slash = raw.find('/')?;
    if slash == 0 {
        return None;
    }
    let provider = &raw[..slash];
    let mut id = &raw[slash + 1..];
    let mut thinking_level = None;
    if let Some(colon) = id.rfind(':') {
        let suffix = &id[colon + 1..];
        let parsed = parse_thinking_level(suffix);
        // Strict concrete suffixes take precedence over literal model ids.
        if let Some(level) = parsed.filter(|level| *level != ThinkingLevel::Max) {
            thinking_level = Some(level);
            id = &id[..colon];
        } else if (parsed == Some(ThinkingLevel::Max) || suffix == "auto")
            && !model_lookup.is_some_and(|lookup| lookup.contains_model(provider, id))
        {
            thinking_level = parsed;
            id = &id[..colon];
        }
    }
    Some(RetryFallbackSelector {
        raw: raw.to_owned(),
        provider: provider.to_owned(),
        id: id.to_owned(),
        thinking_level,
    })
}

pub fn is_retry_fallback_model_key(key: &str) -> bool {
    key.contains('/')
}

pub fn is_retry_fallback_wildcard_key(key: &str) -> bool {
    key.ends_with("/*")
}

/// Call with a key or entry ending in `/*`.
pub fn parse_retry_fallback_wildcard(key: &str, is_known_provider: impl Fn(&str) -> bool) -> RetryFallbackWildcard {
    let template = key.strip_suffix("/*").unwrap_or(key);
    if !template.contains('/') || is_known_provider(template) {
        RetryFallbackWildcard { provider: template.to_owned(), id_prefix: None }
    } else {
        let (provider, prefix) = template.split_once('/').expect("slash checked");
        RetryFallbackWildcard { provider: provider.to_owned(), id_prefix: Some(prefix.to_owned()) }
    }
}

fn format_selector_value(selector: String, thinking_level: Option<ThinkingLevel>) -> String {
    match thinking_level {
        Some(level) if level != ThinkingLevel::Inherit => {
            format!("{selector}:{}", level.as_str())
        }
        _ => selector,
    }
}

pub fn format_retry_fallback_selector(model: &CurrentModel, thinking_level: Option<ThinkingLevel>) -> String {
    let mut selector = model.plain_selector();
    if let Some(route) = model.single_upstream_route.as_deref().filter(|route| !route.is_empty()) {
        selector.push('@');
        selector.push_str(route);
    }
    format_selector_value(selector, thinking_level)
}

fn base_selector(selector: &RetryFallbackSelector) -> String {
    format!("{}/{}", selector.provider, selector.id)
}

// JS Object.keys/for-in enumerate canonical array indices first, then other
// own keys in insertion order. preserve_order alone does not supply this rule.
fn js_entries(chains: &RetryFallbackChains) -> Vec<(&String, &Value)> {
    fn index(key: &str) -> Option<u32> {
        let index = key.parse::<u32>().ok()?;
        (index != u32::MAX && index.to_string() == key).then_some(index)
    }
    let mut entries: Vec<_> = chains.iter().collect();
    entries.sort_by(|(left, _), (right, _)| match (index(left), index(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    entries
}

/// Returns an owned settings snapshot. Inherited arrays have equal values,
/// rather than JavaScript reference identity; rebuild after a settings change.
pub fn expand_default_retry_fallback_chains(
    configured: &RetryFallbackChains,
    role_names: &[String],
) -> RetryFallbackChains {
    let mut chains = configured.clone();
    let Some(default_chain) = chains.get("default").filter(|value| value.is_array()).cloned() else {
        return chains;
    };
    for role in role_names {
        if role != "default" && !chains.contains_key(role) {
            chains.insert(role.clone(), default_chain.clone());
        }
    }
    chains
}

/// Like OMP's settings reader, array settings are spread as numeric keys here;
/// validation independently rejects them. Invalid scalar settings become empty.
pub fn get_retry_fallback_chains(configured: Option<&Value>, role_names: &[String]) -> RetryFallbackChains {
    let chains = match configured {
        Some(Value::Object(chains)) => chains.clone(),
        Some(Value::Array(chains)) => {
            chains.iter().enumerate().map(|(index, value)| (index.to_string(), value.clone())).collect()
        }
        _ => Map::new(),
    };
    expand_default_retry_fallback_chains(&chains, role_names)
}

/// `None` represents an absent setting, unlike an explicitly configured null.
/// Re-run after discovery settles; logging and repeated-warning policy are host-owned.
pub fn validate_retry_fallback_chains(
    configured: Option<&Value>,
    model_lookup: &dyn RetryFallbackModelLookup,
    is_discovery_pending: Option<&dyn Fn(&str) -> bool>,
) -> Vec<String> {
    let Some(configured) = configured else {
        return Vec::new();
    };
    let Some(chains) = configured.as_object() else {
        return vec![
            "retry.fallbackChains must be a mapping of role names or model selectors to selector arrays.".to_owned(),
        ];
    };
    let pending = |provider: &str| is_discovery_pending.is_some_and(|check| check(provider));
    let mut warnings = Vec::new();
    for (key, chain) in js_entries(chains) {
        let kind = if is_retry_fallback_model_key(key) { "model" } else { "role" };
        if kind == "model" {
            if is_retry_fallback_wildcard_key(key) {
                let wildcard = parse_retry_fallback_wildcard(key, |provider| model_lookup.has_provider(provider));
                if !model_lookup.has_provider(&wildcard.provider) {
                    warnings.push(format!("retry.fallbackChains wildcard key references unknown provider: {key}"));
                }
            } else {
                match parse_retry_fallback_selector(key, Some(model_lookup)) {
                    None => warnings.push(format!("Invalid model selector key in retry.fallbackChains: {key}")),
                    Some(selector)
                        if !model_lookup.contains_model(&selector.provider, &selector.id)
                            && !pending(&selector.provider) =>
                    {
                        warnings.push(format!("retry.fallbackChains key references unknown model: {key}"));
                    }
                    _ => {}
                }
            }
        }
        let Some(chain) = chain.as_array() else {
            warnings.push(format!("Fallback chain for {kind} '{key}' must be an array of selector strings."));
            continue;
        };
        for entry in chain {
            let Some(entry) = entry.as_str() else {
                warnings.push(format!("Fallback chain for {kind} '{key}' contains a non-string selector."));
                continue;
            };
            if is_retry_fallback_wildcard_key(entry) {
                let wildcard = parse_retry_fallback_wildcard(entry, |provider| model_lookup.has_provider(provider));
                if !model_lookup.has_provider(&wildcard.provider) {
                    warnings.push(format!("Fallback chain for {kind} '{key}' references unknown provider: {entry}"));
                }
                continue;
            }
            match parse_retry_fallback_selector(entry, Some(model_lookup)) {
                None => warnings.push(format!("Invalid fallback selector format in {kind} '{key}': {entry}")),
                Some(selector)
                    if !model_lookup.contains_model(&selector.provider, &selector.id)
                        && !pending(&selector.provider) =>
                {
                    warnings.push(format!("Fallback chain for {kind} '{key}' references unknown model: {entry}"));
                }
                _ => {}
            }
        }
    }
    warnings
}

pub fn get_retry_fallback_revert_policy(configured: Option<&Value>) -> RetryFallbackRevertPolicy {
    if configured.and_then(Value::as_str) == Some("never") {
        RetryFallbackRevertPolicy::Never
    } else {
        RetryFallbackRevertPolicy::CooldownExpiry
    }
}

fn primary_selector(context: &RetryFallbackResolutionContext<'_>, key: &str) -> Option<RetryFallbackSelector> {
    if is_retry_fallback_wildcard_key(key) {
        None
    } else if is_retry_fallback_model_key(key) {
        parse_retry_fallback_selector(key, Some(context.model_lookup))
    } else {
        (context.get_model_role)(key)
            .and_then(|selector| parse_retry_fallback_selector(&selector, Some(context.model_lookup)))
    }
}

fn current_with_plain(
    context: &RetryFallbackResolutionContext<'_>,
    current_selector: &str,
    current_model: Option<&CurrentModel>,
) -> (Option<RetryFallbackSelector>, Option<String>, Option<String>) {
    let parsed_configured = parse_retry_fallback_selector(current_selector, Some(context.model_lookup));
    let plain = current_model.map(|model| {
        format_selector_value(
            model.plain_selector(),
            parsed_configured.as_ref().and_then(|parsed| parsed.thinking_level),
        )
    });
    let parsed_current = parsed_configured.or_else(|| {
        plain.as_deref().and_then(|selector| parse_retry_fallback_selector(selector, Some(context.model_lookup)))
    });
    let plain_base = plain.as_deref().filter(|plain| *plain != current_selector).and_then(|plain| {
        // Deliberately no lookup: the fixed upstream takes this parsing path.
        parse_retry_fallback_selector(plain, None).as_ref().or(parsed_current.as_ref()).map(base_selector)
    });
    (parsed_current, plain, plain_base)
}

fn matches_current(
    primary: Option<RetryFallbackSelector>,
    current_selector: &str,
    current_base: &str,
    plain_selector: Option<&str>,
    plain_base: Option<&str>,
) -> bool {
    let Some(primary) = primary else { return false };
    if primary.raw == current_selector || plain_selector == Some(primary.raw.as_str()) {
        return true;
    }
    let base = base_selector(&primary);
    base == current_base || plain_base == Some(base.as_str())
}

/// Specificity: exact model, longest wildcard, role hint, matching roles with
/// `default` preferred, then nonempty default without a parseable primary.
pub fn resolve_retry_fallback_chain_key(
    context: &RetryFallbackResolutionContext<'_>,
    current_selector: &str,
    current_model: Option<&CurrentModel>,
    role_hint: Option<&str>,
) -> Option<String> {
    let (current, plain, plain_base) = current_with_plain(context, current_selector, current_model);
    let hint = || role_hint.filter(|hint| !hint.is_empty() && context.chains.get(*hint).is_some_and(Value::is_array));
    let Some(current) = current else { return hint().map(str::to_owned) };
    let current_base = base_selector(&current);
    let matches = |key: &str| {
        matches_current(
            primary_selector(context, key),
            current_selector,
            &current_base,
            plain.as_deref(),
            plain_base.as_deref(),
        )
    };
    let entries = js_entries(context.chains);
    for (key, _) in &entries {
        if is_retry_fallback_model_key(key) && !is_retry_fallback_wildcard_key(key) && matches(key) {
            return Some((*key).clone());
        }
    }
    let mut wildcard_match = None;
    let mut wildcard_prefix_length = None;
    for (key, chain) in &entries {
        if !is_retry_fallback_wildcard_key(key) || !chain.is_array() {
            continue;
        }
        let wildcard = parse_retry_fallback_wildcard(key, |provider| context.model_lookup.has_provider(provider));
        if wildcard.provider != current.provider {
            continue;
        }
        if let Some(prefix) = &wildcard.id_prefix
            && !current.id.starts_with(&format!("{prefix}/"))
        {
            continue;
        }
        let length = wildcard.id_prefix.as_deref().map_or(0, |prefix| prefix.encode_utf16().count());
        if wildcard_prefix_length.is_none_or(|previous| length > previous) {
            wildcard_match = Some((*key).clone());
            wildcard_prefix_length = Some(length);
        }
    }
    if wildcard_match.is_some() {
        return wildcard_match;
    }
    if let Some(hint) = hint() {
        return Some(hint.to_owned());
    }
    let mut matched_role = None;
    for (key, _) in entries {
        if is_retry_fallback_model_key(key) || !matches(key) {
            continue;
        }
        if key == "default" {
            return Some(key.clone());
        }
        if matched_role.is_none() {
            matched_role = Some(key.clone());
        }
    }
    if matched_role.as_ref().is_some_and(|role| !role.is_empty()) {
        return matched_role;
    }
    if context.chains.get("default").and_then(Value::as_array).is_some_and(|chain| !chain.is_empty())
        && primary_selector(context, "default").is_none()
    {
        return Some("default".to_owned());
    }
    None
}

fn parse_chain_entry(
    context: &RetryFallbackResolutionContext<'_>,
    entry: &str,
    current: Option<&RetryFallbackSelector>,
) -> Option<RetryFallbackSelector> {
    if !is_retry_fallback_wildcard_key(entry) {
        return parse_retry_fallback_selector(entry, Some(context.model_lookup));
    }
    let current = current?;
    let wildcard = parse_retry_fallback_wildcard(entry, |provider| context.model_lookup.has_provider(provider));
    let bare_id = current.id.rsplit('/').next().unwrap_or(&current.id);
    let id = if let Some(prefix) = wildcard.id_prefix {
        format!("{prefix}/{bare_id}")
    } else if bare_id != current.id
        && !context.model_lookup.contains_model(&wildcard.provider, &current.id)
        && context.model_lookup.contains_model(&wildcard.provider, bare_id)
    {
        bare_id.to_owned()
    } else {
        current.id.clone()
    };
    Some(RetryFallbackSelector {
        raw: format!("{}/{id}", wildcard.provider),
        provider: wildcard.provider,
        id,
        thinking_level: None,
    })
}

fn effective_chain(
    context: &RetryFallbackResolutionContext<'_>,
    key: &str,
    current_selector: &str,
    current_model: Option<&CurrentModel>,
    allow_missing_primary: bool,
) -> Result<Vec<RetryFallbackSelector>, RetryFallbackError> {
    let current = parse_retry_fallback_selector(current_selector, Some(context.model_lookup)).or_else(|| {
        current_model
            .and_then(|model| parse_retry_fallback_selector(&model.plain_selector(), Some(context.model_lookup)))
    });
    let mut chain = Vec::new();
    if is_retry_fallback_wildcard_key(key) {
        if let Some(current) = &current {
            chain.push(current.clone());
        }
    } else if let Some(primary) = primary_selector(context, key) {
        chain.push(primary);
    } else if (key == "default" || allow_missing_primary) && current.is_some() {
        chain.push(current.clone().expect("current checked"));
    } else if !allow_missing_primary {
        return Ok(chain);
    }
    let mut seen: HashSet<_> = chain.iter().map(|selector| selector.raw.clone()).collect();
    if let Some(entries) = context.chains.get(key) {
        let entries = entries.as_array().ok_or_else(|| RetryFallbackError::ChainNotArray { key: key.to_owned() })?;
        for (index, entry) in entries.iter().enumerate() {
            let entry =
                entry.as_str().ok_or_else(|| RetryFallbackError::NonStringSelector { key: key.to_owned(), index })?;
            if let Some(selector) = parse_chain_entry(context, entry, current.as_ref())
                && seen.insert(selector.raw.clone())
            {
                chain.push(selector);
            }
        }
    }
    Ok(chain)
}

/// Returns entries after the current selector. Optional wrap appends earlier
/// entries and excludes only the matched current entry, preserving raw variants.
pub fn find_retry_fallback_candidates(
    context: &RetryFallbackResolutionContext<'_>,
    chain_key: &str,
    current_selector: &str,
    current_model: Option<&CurrentModel>,
    options: RetryFallbackCandidateOptions,
) -> Result<Vec<RetryFallbackSelector>, RetryFallbackError> {
    let chain = effective_chain(context, chain_key, current_selector, current_model, options.allow_missing_primary)?;
    let (current, plain, plain_base) = current_with_plain(context, current_selector, current_model);
    let Some(current) = current else { return Ok(chain) };
    if chain.len() <= 1 {
        return Ok(Vec::new());
    }
    let current_base = base_selector(&current);
    let index = chain
        .iter()
        .position(|selector| selector.raw == current_selector || plain.as_deref() == Some(selector.raw.as_str()))
        .or_else(|| {
            chain.iter().position(|selector| {
                let base = base_selector(selector);
                base == current_base || plain_base.as_deref() == Some(base.as_str())
            })
        });
    let Some(index) = index else { return Ok(chain[1..].to_vec()) };
    let mut candidates = chain[index + 1..].to_vec();
    if options.wrap_around {
        candidates.extend_from_slice(&chain[..index]);
    }
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Lookup {
        providers: HashSet<String>,
        models: HashSet<(String, String)>,
    }

    impl Lookup {
        fn new(models: &[(&str, &str)], extra_providers: &[&str]) -> Self {
            Self {
                providers: models
                    .iter()
                    .map(|(provider, _)| (*provider).to_owned())
                    .chain(extra_providers.iter().map(|provider| (*provider).to_owned()))
                    .collect(),
                models: models.iter().map(|(provider, id)| ((*provider).to_owned(), (*id).to_owned())).collect(),
            }
        }
    }

    impl RetryFallbackModelLookup for Lookup {
        fn contains_model(&self, provider: &str, id: &str) -> bool {
            self.models.contains(&(provider.to_owned(), id.to_owned()))
        }
        fn has_provider(&self, provider: &str) -> bool {
            self.providers.contains(provider)
        }
    }

    fn lookup() -> Lookup {
        Lookup::new(
            &[
                ("google", "gemini-2.5-flash"),
                ("google-vertex", "gemini-2.5-flash"),
                ("openrouter", "google/gemini-2.5-flash"),
                ("openai", "gpt-4o-mini"),
            ],
            &[],
        )
    }

    fn chains(value: Value) -> RetryFallbackChains {
        value.as_object().unwrap().clone()
    }

    fn raws(selectors: &[RetryFallbackSelector]) -> Vec<&str> {
        selectors.iter().map(|selector| selector.raw.as_str()).collect()
    }

    fn model(provider: &str, id: &str) -> CurrentModel {
        CurrentModel { provider: provider.to_owned(), id: id.to_owned(), single_upstream_route: None }
    }

    #[test]
    fn retry_fallback_selector_matrix_and_literal_guards() {
        let lookup = Lookup::new(&[("p", "router:max"), ("p", "runtime:auto"), ("p", "literal:high")], &[]);
        for (input, id, thinking) in [
            ("p/model", "model", None),
            ("p/model:off", "model", Some(ThinkingLevel::Off)),
            ("p/model:minimal", "model", Some(ThinkingLevel::Minimal)),
            ("p/model:low", "model", Some(ThinkingLevel::Low)),
            ("p/model:med", "model", Some(ThinkingLevel::Medium)),
            ("p/model:high", "model", Some(ThinkingLevel::High)),
            ("p/model:xhi", "model", Some(ThinkingLevel::XHigh)),
            ("p/model:ma", "model", Some(ThinkingLevel::Max)),
            ("p/model:inherit", "model", Some(ThinkingLevel::Inherit)),
            ("p/model:auto", "model", None),
            ("p/router:max", "router:max", None),
            ("p/runtime:auto", "runtime:auto", None),
            ("p/literal:high", "literal", Some(ThinkingLevel::High)),
            ("p/model:extended", "model:extended", None),
            ("p/model:constructor", "model:constructor", None),
            ("p/model:AUTO", "model:AUTO", None),
            ("p/model:m", "model:m", None),
            ("p/model:", "model:", None),
            ("p/vendor/model:tag:high", "vendor/model:tag", Some(ThinkingLevel::High)),
            ("p/", "", None),
            ("p/:high", "", Some(ThinkingLevel::High)),
            ("p/model@route:high", "model@route", Some(ThinkingLevel::High)),
        ] {
            let parsed = parse_retry_fallback_selector(input, Some(&lookup)).unwrap();
            assert_eq!((parsed.id.as_str(), parsed.thinking_level), (id, thinking), "{input}");
            assert_eq!(parsed.raw, input);
        }
        for input in ["", "   ", "/model", "model:high"] {
            assert_eq!(parse_retry_fallback_selector(input, Some(&lookup)), None, "{input}");
        }
        let parsed = parse_retry_fallback_selector("\u{feff} p/model \u{feff}", None).unwrap();
        assert_eq!(parsed.raw, "p/model");
        assert_eq!(parse_retry_fallback_selector("p/router:max", None).unwrap().id, "router");
    }

    #[test]
    fn retry_fallback_thinking_abbreviations_and_auto_projection() {
        assert_eq!(parse_thinking_level("mi"), Some(ThinkingLevel::Minimal));
        assert_eq!(parse_thinking_level("me"), Some(ThinkingLevel::Medium));
        assert_eq!(parse_thinking_level("m"), None);
        assert_eq!(parse_thinking_level("High"), None);
        assert_eq!(parse_thinking_level("constructor"), None);
        assert_eq!(concrete_thinking_level(Some(ConfiguredThinkingLevel::Auto)), None);
        assert_eq!(
            concrete_thinking_level(Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::Inherit))),
            Some(ThinkingLevel::Inherit)
        );
    }

    #[test]
    fn retry_fallback_wildcards_preserve_known_slash_providers() {
        let lookup = Lookup::new(&[], &["gateway/team", "openrouter"]);
        assert_eq!(
            parse_retry_fallback_wildcard("gateway/team/*", |p| lookup.has_provider(p)),
            RetryFallbackWildcard { provider: "gateway/team".into(), id_prefix: None }
        );
        assert_eq!(
            parse_retry_fallback_wildcard("openrouter/google/sub/*", |p| lookup.has_provider(p)),
            RetryFallbackWildcard { provider: "openrouter".into(), id_prefix: Some("google/sub".into()) }
        );
        // Ordinary parsing remains first-slash even for a known slash provider.
        assert_eq!(parse_retry_fallback_selector("gateway/team/model", Some(&lookup)).unwrap().provider, "gateway");
        let configured = chains(json!({"source/*": ["gateway/team/*"]}));
        let context =
            RetryFallbackResolutionContext { chains: &configured, get_model_role: &|_| None, model_lookup: &lookup };
        let candidates =
            find_retry_fallback_candidates(&context, "source/*", "source/model", None, Default::default()).unwrap();
        assert_eq!(candidates[0].provider, "gateway/team");
        assert_eq!(candidates[0].id, "model");
        assert!(
            validate_retry_fallback_chains(Some(&json!({"gateway/team/*": ["gateway/team/*"]})), &lookup, None)
                .is_empty()
        );
    }

    #[test]
    fn retry_fallback_routing_format_and_inherit() {
        let mut model = model("openrouter", "google/gemini");
        model.single_upstream_route = Some("vertex".into());
        assert_eq!(
            format_retry_fallback_selector(&model, Some(ThinkingLevel::High)),
            "openrouter/google/gemini@vertex:high"
        );
        assert_eq!(
            format_retry_fallback_selector(&model, Some(ThinkingLevel::Inherit)),
            "openrouter/google/gemini@vertex"
        );
        model.single_upstream_route = Some(String::new());
        assert_eq!(format_retry_fallback_selector(&model, None), "openrouter/google/gemini");
    }

    #[test]
    fn retry_fallback_default_expansion_preserves_explicit_chains() {
        let configured = chains(json!({"default": ["openai/gpt-4o-mini"], "vision": [], "task": null}));
        let expanded = expand_default_retry_fallback_chains(
            &configured,
            &["default".into(), "vision".into(), "task".into(), "assistant".into()],
        );
        assert_eq!(expanded["assistant"], configured["default"]);
        assert_eq!(expanded["vision"], json!([]));
        assert_eq!(expanded["task"], Value::Null);
        assert!(!configured.contains_key("assistant"));
        assert_eq!(
            expand_default_retry_fallback_chains(&chains(json!({"default": "invalid"})), &["task".into()]),
            chains(json!({"default": "invalid"}))
        );
    }

    #[test]
    fn retry_fallback_settings_reader_and_revert_policy() {
        assert!(get_retry_fallback_chains(None, &[]).is_empty());
        assert!(get_retry_fallback_chains(Some(&json!(false)), &[]).is_empty());
        assert_eq!(get_retry_fallback_chains(Some(&json!([["p/m"]])), &[]), chains(json!({"0": ["p/m"]})));
        assert_eq!(get_retry_fallback_revert_policy(Some(&json!("never"))), RetryFallbackRevertPolicy::Never);
        for value in [Value::Null, json!(false), json!("Never"), json!("cooldown-expiry")] {
            assert_eq!(get_retry_fallback_revert_policy(Some(&value)), RetryFallbackRevertPolicy::CooldownExpiry);
        }
        assert_eq!(get_retry_fallback_revert_policy(None), RetryFallbackRevertPolicy::CooldownExpiry);
    }

    #[test]
    fn retry_fallback_validation_warns_for_every_malformed_surface() {
        let lookup = lookup();
        let configured = json!({"/broken": [], "missing/*": ["other/*"], "p/missing": false,
            "task": [42, "no-slash", "openai/unknown", "openai/gpt-4o-mini"]});
        assert_eq!(
            validate_retry_fallback_chains(Some(&configured), &lookup, None),
            vec![
                "Invalid model selector key in retry.fallbackChains: /broken",
                "retry.fallbackChains wildcard key references unknown provider: missing/*",
                "Fallback chain for model 'missing/*' references unknown provider: other/*",
                "retry.fallbackChains key references unknown model: p/missing",
                "Fallback chain for model 'p/missing' must be an array of selector strings.",
                "Fallback chain for role 'task' contains a non-string selector.",
                "Invalid fallback selector format in role 'task': no-slash",
                "Fallback chain for role 'task' references unknown model: openai/unknown",
            ]
        );
    }

    #[test]
    fn retry_fallback_validation_distinguishes_absent_and_invalid_mapping() {
        let lookup = lookup();
        assert!(validate_retry_fallback_chains(None, &lookup, None).is_empty());
        for value in [Value::Null, json!([]), json!("bad"), json!(false), json!(1)] {
            assert_eq!(
                validate_retry_fallback_chains(Some(&value), &lookup, None),
                vec!["retry.fallbackChains must be a mapping of role names or model selectors to selector arrays."]
            );
        }
        assert!(validate_retry_fallback_chains(Some(&json!({})), &lookup, None).is_empty());
    }

    #[test]
    fn retry_fallback_validation_defers_only_unknown_models_during_discovery() {
        let lookup = lookup();
        let configured = json!({"litellm/Qwen3.8-27B": ["litellm/Qwen3.8-27B-hetzner"]});
        assert!(validate_retry_fallback_chains(Some(&configured), &lookup, Some(&|p| p == "litellm")).is_empty());
        assert_eq!(
            validate_retry_fallback_chains(Some(&configured), &lookup, None),
            vec![
                "retry.fallbackChains key references unknown model: litellm/Qwen3.8-27B",
                "Fallback chain for model 'litellm/Qwen3.8-27B' references unknown model: litellm/Qwen3.8-27B-hetzner",
            ]
        );
        assert_eq!(
            validate_retry_fallback_chains(
                Some(&json!({"litellm/*": ["litellm/*", 1, "broken"]})),
                &lookup,
                Some(&|_| true)
            )
            .len(),
            4
        );
    }

    #[test]
    fn retry_fallback_resolution_specificity_matrix() {
        let lookup = lookup();
        let role = |name: &str| match name {
            "default" => None,
            _ => Some("openrouter/google/gemini-2.5-flash".into()),
        };
        let mut configured = chains(json!({"default": ["openai/gpt-4o-mini"], "task": [],
            "openrouter/*": [], "openrouter/google/*": [],
            "openrouter/google/gemini-2.5-flash": []}));
        for (remove, expected) in [
            (None, Some("openrouter/google/gemini-2.5-flash")),
            (Some("openrouter/google/gemini-2.5-flash"), Some("openrouter/google/*")),
            (Some("openrouter/google/*"), Some("openrouter/*")),
            (Some("openrouter/*"), Some("task")),
            (Some("task"), Some("default")),
        ] {
            if let Some(remove) = remove {
                configured.remove(remove);
            }
            let context =
                RetryFallbackResolutionContext { chains: &configured, get_model_role: &role, model_lookup: &lookup };
            assert_eq!(
                resolve_retry_fallback_chain_key(
                    &context,
                    "openrouter/google/gemini-2.5-flash:high",
                    None,
                    Some("task")
                )
                .as_deref(),
                expected
            );
        }
    }

    #[test]
    fn retry_fallback_empty_role_hint_and_matched_role_follow_js_truthiness() {
        let lookup = Lookup::new(&[("p", "a"), ("p", "b"), ("p", "c")], &[]);
        let configured = chains(json!({"": ["p/b"], "default": ["p/c"]}));
        let no_roles = |_: &str| None;
        let context =
            RetryFallbackResolutionContext { chains: &configured, get_model_role: &no_roles, model_lookup: &lookup };
        assert_eq!(resolve_retry_fallback_chain_key(&context, "p/a", None, Some("")).as_deref(), Some("default"));
        assert_eq!(resolve_retry_fallback_chain_key(&context, "unqualified", None, Some("")), None);
        let empty_role = |role: &str| (role.is_empty()).then(|| "p/a".into());
        let context =
            RetryFallbackResolutionContext { chains: &configured, get_model_role: &empty_role, model_lookup: &lookup };
        assert_eq!(resolve_retry_fallback_chain_key(&context, "p/a", None, None).as_deref(), Some("default"));
    }

    #[test]
    fn retry_fallback_default_matching_beats_shared_role_insertion_order() {
        let lookup = lookup();
        let configured = chains(json!({"vision": ["google/gemini-2.5-flash"], "default": ["openai/gpt-4o-mini"]}));
        let role = |_: &str| Some("openrouter/google/gemini-2.5-flash".into());
        let context =
            RetryFallbackResolutionContext { chains: &configured, get_model_role: &role, model_lookup: &lookup };
        assert_eq!(
            resolve_retry_fallback_chain_key(&context, "openrouter/google/gemini-2.5-flash", None, None).as_deref(),
            Some("default")
        );
        assert_eq!(
            resolve_retry_fallback_chain_key(&context, "openrouter/google/gemini-2.5-flash", None, Some("vision"))
                .as_deref(),
            Some("vision")
        );
        assert_eq!(resolve_retry_fallback_chain_key(&context, "google/gemini-2.5-flash", None, None), None);
    }

    #[test]
    fn retry_fallback_missing_primary_and_unqualified_current() {
        let lookup = lookup();
        let configured = chains(json!({"task": ["google/gemini-2.5-flash", "openai/gpt-4o-mini", "google/*"]}));
        let context =
            RetryFallbackResolutionContext { chains: &configured, get_model_role: &|_| None, model_lookup: &lookup };
        assert_eq!(
            resolve_retry_fallback_chain_key(&context, "missing-model:high", None, Some("task")).as_deref(),
            Some("task")
        );
        assert_eq!(resolve_retry_fallback_chain_key(&context, "missing-model:high", None, None), None);
        assert!(
            find_retry_fallback_candidates(
                &context,
                "task",
                "openrouter/google/gemini-2.5-flash",
                None,
                Default::default()
            )
            .unwrap()
            .is_empty()
        );
        let options = RetryFallbackCandidateOptions { allow_missing_primary: true, ..Default::default() };
        assert_eq!(
            raws(&find_retry_fallback_candidates(&context, "task", "missing-model:high", None, options).unwrap()),
            ["google/gemini-2.5-flash", "openai/gpt-4o-mini"]
        );
        assert_eq!(
            raws(
                &find_retry_fallback_candidates(&context, "task", "openrouter/google/gemini-2.5-flash", None, options)
                    .unwrap()
            ),
            ["google/gemini-2.5-flash", "openai/gpt-4o-mini"]
        );
    }

    #[test]
    fn retry_fallback_aggregator_to_direct_and_prefixed_wildcard() {
        let lookup = lookup();
        let configured = chains(json!({"openrouter/*": ["google-vertex/*", "google/*", "openrouter/other/*"]}));
        let context =
            RetryFallbackResolutionContext { chains: &configured, get_model_role: &|_| None, model_lookup: &lookup };
        let result = find_retry_fallback_candidates(
            &context,
            "openrouter/*",
            "openrouter/google/gemini-2.5-flash:high",
            None,
            Default::default(),
        )
        .unwrap();
        assert_eq!(
            raws(&result),
            ["google-vertex/gemini-2.5-flash", "google/gemini-2.5-flash", "openrouter/other/gemini-2.5-flash"]
        );
        assert!(result.iter().all(|selector| selector.thinking_level.is_none()));
    }

    #[test]
    fn retry_fallback_wildcard_keeps_full_id_when_present_or_bare_missing() {
        let lookup = Lookup::new(&[("target", "vendor/model"), ("target", "model")], &["missing"]);
        let configured = chains(json!({"source/*": ["target/*", "missing/*"]}));
        let context =
            RetryFallbackResolutionContext { chains: &configured, get_model_role: &|_| None, model_lookup: &lookup };
        assert_eq!(
            raws(
                &find_retry_fallback_candidates(&context, "source/*", "source/vendor/model", None, Default::default())
                    .unwrap()
            ),
            ["target/vendor/model", "missing/vendor/model"]
        );
    }

    #[test]
    fn retry_fallback_raw_dedup_keeps_thinking_variants_and_wraps() {
        let lookup = lookup();
        let configured = chains(json!({"task": [" p/a:high ", "p/a:high", "p/a:low", "p/b", "p/c", "invalid"]}));
        let role = |_: &str| Some("p/a:high".into());
        let context =
            RetryFallbackResolutionContext { chains: &configured, get_model_role: &role, model_lookup: &lookup };
        let options = RetryFallbackCandidateOptions { wrap_around: true, ..Default::default() };
        assert_eq!(
            raws(&find_retry_fallback_candidates(&context, "task", "p/b", None, options).unwrap()),
            ["p/c", "p/a:high", "p/a:low"]
        );
        assert_eq!(
            raws(&find_retry_fallback_candidates(&context, "task", "p/a:low", None, Default::default()).unwrap()),
            ["p/b", "p/c"]
        );
        assert_eq!(
            raws(&find_retry_fallback_candidates(&context, "task", "p/a:max", None, Default::default()).unwrap()),
            ["p/a:low", "p/b", "p/c"]
        );
        assert_eq!(
            raws(&find_retry_fallback_candidates(&context, "task", "p/unknown", None, options).unwrap()),
            ["p/a:low", "p/b", "p/c"]
        );
    }

    #[test]
    fn retry_fallback_routed_current_matches_plain_and_uses_registry_fallback() {
        let lookup = lookup();
        let configured = chains(json!({"openrouter/google/gemini-2.5-flash:high": ["openai/gpt-4o-mini"]}));
        let context =
            RetryFallbackResolutionContext { chains: &configured, get_model_role: &|_| None, model_lookup: &lookup };
        let current_model = model("openrouter", "google/gemini-2.5-flash");
        let routed = "openrouter/google/gemini-2.5-flash@vertex:high";
        let key = resolve_retry_fallback_chain_key(&context, routed, Some(&current_model), None).unwrap();
        assert_eq!(key, "openrouter/google/gemini-2.5-flash:high");
        assert_eq!(
            raws(
                &find_retry_fallback_candidates(&context, &key, routed, Some(&current_model), Default::default())
                    .unwrap()
            ),
            ["openai/gpt-4o-mini"]
        );
        assert_eq!(resolve_retry_fallback_chain_key(&context, "unqualified", Some(&current_model), None), Some(key));
    }

    #[test]
    fn retry_fallback_numeric_roles_follow_js_key_order() {
        let lookup = lookup();
        let mut configured = Map::new();
        for key in ["10", "01", "2", "4294967295", "0"] {
            configured.insert(key.into(), json!([]));
        }
        let role = |_: &str| Some("p/m".into());
        let context =
            RetryFallbackResolutionContext { chains: &configured, get_model_role: &role, model_lookup: &lookup };
        assert_eq!(resolve_retry_fallback_chain_key(&context, "p/m", None, None).as_deref(), Some("0"));
        assert_eq!(
            js_entries(&configured).iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
            ["0", "2", "10", "01", "4294967295"]
        );
    }

    #[test]
    fn retry_fallback_malformed_selected_chains_fail_explicitly() {
        let lookup = lookup();
        let role = |_: &str| Some("p/a".into());
        for (value, expected) in [
            (json!({"p/a": null}), RetryFallbackError::ChainNotArray { key: "p/a".into() }),
            (json!({"p/a": ["p/b", 42]}), RetryFallbackError::NonStringSelector { key: "p/a".into(), index: 1 }),
        ] {
            let configured = chains(value);
            let context =
                RetryFallbackResolutionContext { chains: &configured, get_model_role: &role, model_lookup: &lookup };
            assert_eq!(resolve_retry_fallback_chain_key(&context, "p/a", None, None).as_deref(), Some("p/a"));
            assert_eq!(find_retry_fallback_candidates(&context, "p/a", "p/a", None, Default::default()), Err(expected));
        }
    }
}
