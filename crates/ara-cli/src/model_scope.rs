//! Fixed OMP model scope resolution and startup scope preparation.
//!
//! Source 596f2da7101178214aa27a753529d15e6b7ad91d:
//! packages/coding-agent/src/config/model-resolver.ts:114-203,700-719,
//! 1694-1780,1830-1865. Registry availability, settings reads and authentication remain
//! Host-owned. The supplied resolver retains one catalogue/collapse snapshot.
//! Startup adapters follow main.ts:815-860,882-902; background waiting,
//! disposal, generation admission and Session application stay in the Host.
//!
//! MIT License; Copyright (c) 2025 Mario Zechner;
//! Copyright (c) 2025-2026 Can Bölük; Copyright (c) 2026 Stencil Labs, Inc.
//! See LICENSE for the full license.

use crate::{
    model_collapse::{CollapseError, lower},
    model_identity_wire::text,
    model_manager::ModelRefreshStrategy,
    model_patch::HostModelRef,
    model_registry::ModelRegistry,
    model_registry_runtime::RegistryRuntimeResult,
    model_roles::{ModelMatchPreferences, ModelRoleLookup, ModelRoleResolver, clamp_role_thinking},
    retry_fallback::{ConfiguredThinkingLevel, ThinkingLevel, parse_thinking_level},
};
use ara_rpc::WireString;
use globset::GlobBuilder;
use regex::RegexBuilder;
use std::collections::{HashMap, HashSet, hash_map::Entry};

/// Native cycling scope: a catalog donor plus a concrete per-model override.
/// Auto remains Session-owned and is never retained in this record.
#[derive(Clone)]
pub struct ScopedModel {
    pub model: HostModelRef,
    pub thinking_level: Option<ThinkingLevel>,
    pub explicit_thinking_level: bool,
}

/// Native Session Ctrl+P shape after configured default thinking is applied.
/// This records no role/source or explicitness state and keeps the model donor.
#[derive(Clone)]
pub struct SessionScopedModel {
    pub model: HostModelRef,
    pub thinking_level: Option<ThinkingLevel>,
}

/// Resolve only the currently published available catalog with its matching
/// generation's alias runtime. Background-rebuild callers await their existing
/// Registry flight and perform Host admission before/after using this helper.
pub fn resolve_current_scoped_models(
    registry: &ModelRegistry,
    patterns: &[String],
    preferences: &ModelMatchPreferences,
    settings: &dyn ModelRoleLookup,
) -> RegistryRuntimeResult<Vec<ScopedModel>> {
    if patterns.is_empty() {
        return Ok(Vec::new());
    }
    let resolver = registry.model_role_resolver(preferences.clone())?;
    Ok(resolve_model_scope(&resolver, patterns, settings)?)
}

/// Fixed Native main.ts:815-833. Only a completely empty configured scope with
/// known discoverable providers gets one cache-aware startup discovery pass.
/// A partial scope starts immediately; a still-empty result is retained for the
/// Host's deferred SDK/extension resolution and background rebuild.
pub async fn resolve_scoped_models(
    registry: &ModelRegistry,
    patterns: &[String],
    preferences: &ModelMatchPreferences,
    settings: &(dyn ModelRoleLookup + Sync),
) -> RegistryRuntimeResult<Vec<ScopedModel>> {
    let scoped = resolve_current_scoped_models(registry, patterns, preferences, settings)?;
    if patterns.is_empty() || !scoped.is_empty() || registry.get_discoverable_providers().is_empty() {
        return Ok(scoped);
    }
    registry.runtime().refresh(ModelRefreshStrategy::OnlineIfUncached).await?;
    resolve_current_scoped_models(registry, patterns, preferences, settings)
}

/// Fixed Native main.ts:838-852. Auto is Session-owned; it contributes no
/// concrete default here. Defaults are not model-clamped during this mapping.
pub fn to_session_scoped_models(
    scoped: &[ScopedModel],
    default_thinking: Option<ConfiguredThinkingLevel>,
) -> Vec<SessionScopedModel> {
    let default = match default_thinking {
        Some(ConfiguredThinkingLevel::Concrete(level)) => Some(level),
        Some(ConfiguredThinkingLevel::Auto) | None => None,
    };
    scoped
        .iter()
        .map(|entry| SessionScopedModel {
            model: entry.model.clone(),
            thinking_level: if entry.explicit_thinking_level { entry.thinking_level.or(default) } else { default },
        })
        .collect()
}

/// Fixed Native main.ts:855-860. Length plus provider/ID-key membership only:
/// reordered donors, changed thinking and metadata alone do not trigger rebuild.
pub fn same_scoped_model_set(a: &[SessionScopedModel], b: &[SessionScopedModel]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let keys = a.iter().map(|entry| model_key(&entry.model)).collect::<HashSet<_>>();
    b.iter().all(|entry| keys.contains(&model_key(&entry.model)))
}

/// Fixed Native `resolveModelScope`. Patterns retain author order; a glob
/// retains available-catalog order. The first provider/ID occurrence owns its
/// thinking override, including an explicit absence. Empty patterns are empty.
/// Direct patterns use this resolver's preferences; role aliases contribute
/// the role's single selected concrete model rather than its fallback chain.
pub fn resolve_model_scope(
    resolver: &ModelRoleResolver,
    patterns: &[String],
    settings: &dyn ModelRoleLookup,
) -> Result<Vec<ScopedModel>, CollapseError> {
    let mut scoped = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |model: HostModelRef, level: Option<ThinkingLevel>, explicit: bool| {
        let provider = text(model.spec(), "provider")
            .ok_or_else(|| CollapseError::new("Scoped model provider must be a string".into()))?;
        let id =
            text(model.spec(), "id").ok_or_else(|| CollapseError::new("Scoped model ID must be a string".into()))?;
        if !seen.insert((provider, id)) {
            return Ok(());
        }
        let level = if explicit { clamp_scoped_thinking(&model, level)? } else { level };
        scoped.push(ScopedModel { model, thinking_level: level, explicit_thinking_level: explicit });
        Ok::<_, CollapseError>(())
    };
    for pattern in patterns {
        if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
            let (models, level, explicit) = resolve_glob_scope_pattern(pattern, resolver.available_models());
            for model in models {
                add(model, level, explicit)?;
            }
            continue;
        }
        let resolved = if pattern.starts_with('@') || pattern.starts_with("pi/") {
            resolver.resolve_model_role_value(Some(pattern), settings)?
        } else {
            resolver.parse_scope_pattern(pattern)?
        };
        let Some(model) = resolved.model else { continue };
        match resolved.thinking_level {
            Some(ConfiguredThinkingLevel::Auto) => add(model, None, false)?,
            Some(ConfiguredThinkingLevel::Concrete(level)) => {
                add(model, Some(level), resolved.explicit_thinking_level)?
            }
            None => add(model, None, resolved.explicit_thinking_level)?,
        }
    }
    Ok(scoped)
}

/// Reuse the role resolver's fixed Native effort ladder across thinking modes.
/// The helper preserves an authored selector without controllable efforts.
fn clamp_scoped_thinking(
    model: &HostModelRef,
    level: Option<ThinkingLevel>,
) -> Result<Option<ThinkingLevel>, CollapseError> {
    Ok(match clamp_role_thinking(model, level.map(ConfiguredThinkingLevel::Concrete))? {
        Some(ConfiguredThinkingLevel::Concrete(level)) => Some(level),
        Some(ConfiguredThinkingLevel::Auto) | None => None,
    })
}

/// Native `filterAvailableModelsByEnabledPatterns`. Ordinary patterns use the
/// default preference context; role aliases use the supplied settings resolver's
/// preferences. Neither model keys nor opaque header donors are reconstructed.
/// Typed settings patterns are UTF-8; arbitrary lone-surrogate patterns require
/// a separate lossless Host settings seam and are not represented by this API.
pub fn filter_available_models_by_enabled_patterns(
    resolver: &ModelRoleResolver,
    patterns: &[String],
    settings: &dyn ModelRoleLookup,
) -> Result<Vec<HostModelRef>, CollapseError> {
    let available = resolver.available_models();
    if patterns.is_empty() {
        return Ok(available.to_vec());
    }
    let direct = resolver.with_preferences(ModelMatchPreferences::default())?;
    let mut allowed = Vec::new();
    for pattern in patterns {
        if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
            allowed.extend(resolve_glob_scope(pattern, available));
        } else if pattern.starts_with('@') || pattern.starts_with("pi/") {
            if let Some(model) = resolver.resolve_model_role_value(Some(pattern), settings)?.model {
                allowed.push(model);
            }
        } else if let Some(model) = direct.parse_scope_pattern(pattern)?.model {
            allowed.push(model);
        }
    }
    Ok(include_synthetic_allowed(available, allowed))
}

/// Fixed Native resolveAllowedModels (startup/SDK): use the preference-aware
/// scope resolver, then retain catalog order and authored synthetic donors.
/// An authored scope with no matches remains empty.
pub fn resolve_allowed_models(
    resolver: &ModelRoleResolver,
    patterns: &[String],
    settings: &dyn ModelRoleLookup,
) -> Result<Vec<HostModelRef>, CollapseError> {
    if patterns.is_empty() {
        return Ok(resolver.available_models().to_vec());
    }
    let scoped = resolve_model_scope(resolver, patterns, settings)?;
    Ok(include_synthetic_allowed(resolver.available_models(), scoped.into_iter().map(|entry| entry.model).collect()))
}

fn model_key(model: &HostModelRef) -> WireString {
    let mut key = text(model.spec(), "provider").unwrap_or_else(|| "".into()).units().to_vec();
    key.push(b'/' as u16);
    key.extend(text(model.spec(), "id").unwrap_or_else(|| "".into()).units());
    WireString::from_units(key)
}

fn include_synthetic_allowed(available: &[HostModelRef], allowed: Vec<HostModelRef>) -> Vec<HostModelRef> {
    let mut by_key = HashMap::new();
    let mut ordered = Vec::new();
    for model in allowed {
        let key = model_key(&model);
        if let Entry::Vacant(entry) = by_key.entry(key) {
            entry.insert(ordered.len());
            ordered.push(Some(model));
        }
    }
    let mut result = Vec::new();
    for model in available {
        if let Some(index) = by_key.remove(&model_key(model)) {
            ordered[index] = None;
            result.push(model.clone());
        }
    }
    result.extend(ordered.into_iter().flatten());
    result
}

fn resolve_glob_scope(pattern: &str, available: &[HostModelRef]) -> Vec<HostModelRef> {
    resolve_glob_scope_pattern(pattern, available).0
}

fn resolve_glob_scope_pattern(
    pattern: &str,
    available: &[HostModelRef],
) -> (Vec<HostModelRef>, Option<ThinkingLevel>, bool) {
    if let Some((base, suffix)) = pattern.rsplit_once(':') {
        // Strict concrete suffixes strip before literal matching. `max` and
        // exact lowercase `auto` instead let actual tier ids win first.
        if let Some(level) = parse_thinking_level(suffix) {
            if level != ThinkingLevel::Max {
                return (matching_glob_models(base, available), Some(level), true);
            }
            let literal = matching_glob_models(pattern, available);
            return if literal.is_empty() {
                (matching_glob_models(base, available), Some(level), true)
            } else {
                (literal, None, false)
            };
        }
        if suffix == "auto" {
            let literal = matching_glob_models(pattern, available);
            return (if literal.is_empty() { matching_glob_models(base, available) } else { literal }, None, false);
        }
    }
    (matching_glob_models(pattern, available), None, false)
}

fn matching_glob_models(pattern: &str, available: &[HostModelRef]) -> Vec<HostModelRef> {
    let pattern = String::from_utf16_lossy(lower(&pattern.into()).units());
    let bangs = pattern.bytes().take_while(|byte| *byte == b'!').count();
    let negated = bangs % 2 == 1;
    let pattern = &pattern[bangs..];
    // Native Bun.Glob parses escapes on Windows too. globset's normal matcher
    // operates on bytes, so `?` would consume one byte of a Unicode scalar.
    // Reuse its parser, then bind its expression to Unicode strings instead.
    let expression = escaped_classes(pattern)
        .and_then(|(pattern, classes)| {
            let glob = GlobBuilder::new(&pattern)
                .literal_separator(true)
                .backslash_escape(true)
                .empty_alternates(true)
                .build()
                .ok()?;
            let mut expression = unicode_expression(glob.regex())?;
            for (placeholder, class) in classes {
                expression = expression.replace(&format!("\\u{{{:x}}}", placeholder as u32), &class);
            }
            Some(expression)
        })
        .and_then(|expression| RegexBuilder::new(&expression).dot_matches_new_line(true).build().ok());
    available
        .iter()
        .filter(|model| {
            let full = String::from_utf16_lossy(lower(&model_key(model)).units());
            let id = text(model.spec(), "id").unwrap_or_else(|| "".into());
            let id = String::from_utf16_lossy(lower(&id).units());
            let matches = |subject: &str| {
                #[cfg(windows)]
                let subject = subject.replace('\\', "/");
                #[cfg(windows)]
                let subject = subject.as_str();
                let matched = expression.as_ref().is_some_and(|expression| expression.is_match(subject));
                matched != negated
            };
            matches(&full) || matches(&id)
        })
        .cloned()
        .collect()
}

/// Bun permits escapes inside character classes; globset treats those slashes
/// as literals. Keep globset's path/brace parser and supply Unicode classes.
fn escaped_classes(pattern: &str) -> Option<(String, Vec<(char, String)>)> {
    let mut parsed = String::new();
    let mut classes = Vec::new();
    let mut input = pattern.chars().peekable();
    let mut placeholder = 0xf0000;
    while let Some(scalar) = input.next() {
        if scalar == '\\' {
            parsed.push(scalar);
            parsed.push(input.next()?);
            continue;
        }
        if scalar != '[' {
            parsed.push(scalar);
            continue;
        }
        let negated = input.peek().is_some_and(|scalar| *scalar == '!' || *scalar == '^');
        if negated {
            input.next();
        }
        let mut members = Vec::new();
        if input.peek() == Some(&']') {
            input.next();
            members.push((']', false));
        }
        loop {
            match input.next()? {
                ']' => break,
                '\\' => members.push((input.next()?, true)),
                scalar => members.push((scalar, false)),
            }
        }
        let mut body = String::new();
        let mut index = 0;
        while index < members.len() {
            let start = members[index].0;
            if members.get(index + 1) == Some(&('-', false)) && index + 2 < members.len() {
                let end = members[index + 2].0;
                // Native invalid ranges contribute no members; other class
                // members still match (for example `[a--c]` matches `c`).
                if start <= end {
                    body.push_str(&format!("\\u{{{:x}}}-\\u{{{:x}}}", start as u32, end as u32));
                }
                index += 3;
            } else {
                body.push_str(&format!("\\u{{{:x}}}", start as u32));
                index += 1;
            }
        }
        let class = if body.is_empty() {
            if negated { "[\\s\\S]".to_owned() } else { "[^\\s\\S]".to_owned() }
        } else {
            format!("[{}{body}]", if negated { "^" } else { "" })
        };
        let marker = loop {
            let marker = char::from_u32(placeholder)?;
            placeholder += 1;
            if !pattern.contains(marker) {
                break marker;
            }
        };
        parsed.push(marker);
        classes.push((marker, class));
    }
    Some((parsed, classes))
}

/// globset encodes non-ASCII literals as consecutive UTF-8 byte escapes. Decode
/// only those emitted byte runs; leave escaped ASCII metacharacters untouched.
fn unicode_expression(expression: &str) -> Option<String> {
    let expression = expression.strip_prefix("(?-u)")?;
    let bytes = expression.as_bytes();
    let mut result = String::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && bytes.get(index + 1) == Some(&b'x') {
            let mut literal = Vec::new();
            while bytes.get(index) == Some(&b'\\') && bytes.get(index + 1) == Some(&b'x') {
                let hex = expression.get(index + 2..index + 4)?;
                literal.push(u8::from_str_radix(hex, 16).ok()?);
                index += 4;
            }
            for scalar in std::str::from_utf8(&literal).ok()?.chars() {
                result.push_str(&format!("\\u{{{:x}}}", scalar as u32));
            }
        } else if bytes[index] == b'\\' {
            let escaped = expression.get(index..index + 2)?;
            result.push_str(escaped);
            index += 2;
        } else {
            result.push(bytes[index] as char);
            index += 1;
        }
    }
    Some(result)
}
