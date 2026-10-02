//! Provider-scoped references from fixed OMP `config/model-resolver.ts`.
//!
//! Source: 596f2da7101178214aa27a753529d15e6b7ad91d, lines 269–579.
//! A snapshot owns donor HostModel identities, including opaque headers. The
//! provider-scoped resolver is distinct from intrinsic/global donor inference.
//!
//! MIT License; Copyright (c) 2025 Mario Zechner;
//! Copyright (c) 2025-2026 Can Bölük; Copyright (c) 2026 Stencil Labs, Inc.
//! See LICENSE for the full license.

use crate::{
    catalog_rules,
    js_regex::JsRegExp,
    model_collapse::{CollapseError, CollapseRuntime, VariantSpec, lower, trim},
    model_identity_wire::{copy_field, empty, spread, str_value, text},
    model_patch::{HeaderSlot, HostModelRef, build_host_model},
    retry_fallback::{ThinkingLevel, parse_thinking_level},
};
use ara_rpc::{WireString, WireValue};
use std::{
    collections::{HashMap, HashSet, VecDeque, hash_map::Entry},
    sync::{Arc, Mutex, OnceLock},
};

type ProviderIndex = HashMap<WireString, Option<HostModelRef>>;
const BEDROCK_PROVIDER: &str = "amazon-bedrock";
const BEDROCK_PROFILE_ARN: &str = r"^arn:aws(?:-[a-z]+)*:bedrock:[a-z0-9-]+:[0-9]*:(?:application-inference-profile|inference-profile)/[a-z0-9][a-z0-9._:-]*$";

/// No Debug/Serialize: donor headers remain private and are never resolved by
/// indexing or lookup. Rebuild a snapshot when its model list changes.
pub struct ProviderModelReferenceIndex {
    models: Vec<HostModelRef>,
    exact: ProviderIndex,
    spelling: ProviderIndex,
    wire_routes: OnceLock<Result<ProviderIndex, CollapseError>>,
    collapse_runtime: Arc<Mutex<CollapseRuntime>>,
    bedrock_profile_arn: Mutex<JsRegExp>,
    openrouter_date_suffix: Mutex<JsRegExp>,
}

fn required_text(model: &VariantSpec, key: &str) -> Result<WireString, CollapseError> {
    text(model, key).ok_or_else(|| CollapseError::new(format!("Model {key} must be a string")))
}

// Keep the native NUL-delimited key, including its behavior for authored NULs.
fn provider_key(provider: &WireString, id: &WireString) -> WireString {
    let mut units = provider.units().to_vec();
    units.push(0);
    units.extend(id.units());
    WireString::from_units(units)
}

fn revision_spelling_key(id: &WireString) -> WireString {
    let lowered = lower(id);
    let mut units = lowered.units().to_vec();
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

fn build_provider_index(
    models: &[HostModelRef],
    id_key: impl Fn(&WireString) -> WireString,
) -> Result<ProviderIndex, CollapseError> {
    let mut index = ProviderIndex::new();
    for model in models {
        let provider = lower(&required_text(model.spec(), "provider")?);
        let id = id_key(&required_text(model.spec(), "id")?);
        match index.entry(provider_key(&provider, &id)) {
            Entry::Vacant(entry) => {
                entry.insert(Some(model.clone()));
            }
            // Even a repeated identical object is ambiguous in the exact and
            // spelling indexes. A third occurrence never restores a winner.
            Entry::Occupied(mut entry) => {
                entry.insert(None);
            }
        }
    }
    Ok(index)
}

fn build_wire_route_index(models: &[HostModelRef]) -> Result<ProviderIndex, CollapseError> {
    let mut index = ProviderIndex::new();
    for model in models {
        let Some(routing) = model.spec().record("thinking").and_then(|thinking| thinking.record("effortRouting"))
        else {
            continue;
        };
        let provider = lower(&required_text(model.spec(), "provider")?);
        let enumerable = spread(&[&routing]);
        for effort in enumerable.own_keys() {
            let Some(value) = enumerable.get_path(std::slice::from_ref(&effort)) else { continue };
            let wire_id =
                value.as_string().ok_or_else(|| CollapseError::new("Model effort route must be a string".into()))?;
            let key = provider_key(&provider, &lower(wire_id));
            match index.entry(key) {
                Entry::Vacant(entry) => {
                    entry.insert(Some(model.clone()));
                }
                Entry::Occupied(mut entry) => {
                    if entry.get().as_ref().is_some_and(|existing| !Arc::ptr_eq(existing, model)) {
                        entry.insert(None);
                    }
                }
            }
        }
    }
    Ok(index)
}

/// Only the thinking-variant arm of taxonomy `collapseVariantId` is needed
/// here. Effort-family matches short-circuit suffix grammar, and lane collapses
/// always have thinkingVariant=false. Operate on the existing compiled
/// vocabulary without projecting authored IDs through UTF-8/JSON.
fn thinking_variant_alias(provider: &WireString, id: &WireString) -> Option<WireString> {
    let vocabulary = catalog_rules::collapse_vocabulary();
    for family in vocabulary["effortFamilies"].as_array().into_iter().flatten() {
        if family["provider"].as_str().is_some_and(|value| provider.equals_ascii(value))
            && family["aliases"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .any(|alias| id.equals_ascii(alias))
        {
            return None;
        }
    }
    let bare_start = id.units().iter().rposition(|unit| *unit == b'/' as u16).map_or(0, |index| index + 1);
    let bare = &id.units()[bare_start..];
    let mut winner = None;
    let mut winner_length = 0;
    for rule in vocabulary["suffixes"].as_array().into_iter().flatten() {
        let suffix: Vec<_> = rule["suffix"].as_str().unwrap_or_default().encode_utf16().collect();
        if !id.units().ends_with(&suffix)
            || rule["exceptBarePrefix"]
                .as_str()
                .is_some_and(|prefix| bare.starts_with(&prefix.encode_utf16().collect::<Vec<_>>()))
        {
            continue;
        }
        if winner.is_none() || suffix.len() > winner_length {
            winner = Some(rule);
            winner_length = suffix.len();
        }
    }
    (winner?["thinking"] == true).then(|| id.slice_prefix(id.len().saturating_sub(winner_length)))
}

fn recognized_thinking_suffix(value: &WireString, allow_max_and_auto: bool) -> bool {
    if allow_max_and_auto && value.equals_ascii("auto") {
        return true;
    }
    value
        .to_utf8()
        .ok()
        .and_then(|value| parse_thinking_level(&value))
        .is_some_and(|level| allow_max_and_auto || level != ThinkingLevel::Max)
}

fn openrouter_route_base(id: &WireString) -> Option<WireString> {
    let colon = id.units().iter().rposition(|unit| *unit == b':' as u16)?;
    let suffix = trim(&WireString::from_units(id.units()[colon + 1..].to_vec()));
    (!suffix.is_empty() && !recognized_thinking_suffix(&suffix, true)).then(|| id.slice_prefix(colon))
}

impl ProviderModelReferenceIndex {
    pub fn new(models: &[HostModelRef]) -> Result<Self, CollapseError> {
        Self::with_collapse_runtime(models, Arc::new(Mutex::new(CollapseRuntime::new()?)))
    }

    /// Hosts pass their existing module-lifetime collapse runtime so aliases
    /// learned by discovery survive subsequent model snapshots.
    pub fn with_collapse_runtime(
        models: &[HostModelRef],
        collapse_runtime: Arc<Mutex<CollapseRuntime>>,
    ) -> Result<Self, CollapseError> {
        let regex =
            |pattern: &str| JsRegExp::new(pattern.into(), "i").map_err(|error| CollapseError::new(error.to_string()));
        Ok(Self {
            models: models.to_vec(),
            exact: build_provider_index(models, lower)?,
            spelling: build_provider_index(models, revision_spelling_key)?,
            wire_routes: OnceLock::new(),
            collapse_runtime,
            bedrock_profile_arn: Mutex::new(regex(BEDROCK_PROFILE_ARN)?),
            openrouter_date_suffix: Mutex::new(regex(r"-\d{8}(?=$|:)")?),
        })
    }

    fn is_bedrock_profile_arn(&self, id: &WireString) -> bool {
        self.bedrock_profile_arn.lock().unwrap_or_else(|error| error.into_inner()).exec(id).is_some()
    }

    fn bedrock_profile(&self, provider: &WireString, id: &WireString) -> Result<Option<HostModelRef>, CollapseError> {
        // The fixed helper receives the original provider, without trim.
        if !lower(provider).equals_ascii(BEDROCK_PROVIDER) {
            return Ok(None);
        }
        let requested = trim(id);
        if let Some(colon) = requested.units().iter().rposition(|unit| *unit == b':' as u16) {
            let suffix = WireString::from_units(requested.units()[colon + 1..].to_vec());
            if recognized_thinking_suffix(&suffix, false)
                && self.is_bedrock_profile_arn(&trim(&requested.slice_prefix(colon)))
            {
                return Ok(None);
            }
        }
        if !self.is_bedrock_profile_arn(&requested) {
            return Ok(None);
        }
        let Some(template) = self.models.iter().find(|model| {
            text(model.spec(), "provider").is_some_and(|provider| lower(&provider).equals_ascii(BEDROCK_PROVIDER))
        }) else {
            return Ok(None);
        };
        let mut spec = empty();
        spec.set("id", WireValue::String(requested));
        spec.set("name", str_value("Bedrock inference profile"));
        spec.set("api", str_value("bedrock-converse-stream"));
        spec.set("provider", str_value(BEDROCK_PROVIDER));
        copy_field(&mut spec, "baseUrl", template.spec(), "baseUrl");
        spec.set("reasoning", WireValue::Bool(false));
        spec.set("input", WireValue::Array(vec![str_value("text")]));
        spec.set(
            "cost",
            WireValue::object(vec![
                ("input", WireValue::Number(0.0)),
                ("output", WireValue::Number(0.0)),
                ("cacheRead", WireValue::Number(0.0)),
                ("cacheWrite", WireValue::Number(0.0)),
            ]),
        );
        spec.set("contextWindow", WireValue::Null);
        spec.set("maxTokens", WireValue::Null);
        for key in ["transport", "guardrailIdentifier", "guardrailVersion", "guardrailTrace", "requestMetadata"] {
            if let Some(value) = template.spec().record(key) {
                spec.set_record(key, &value);
            }
        }
        let headers = match template.headers() {
            HeaderSlot::Source(_) => template.headers().clone(),
            _ => HeaderSlot::Absent,
        };
        build_host_model(&spec, headers).map(Some)
    }

    fn openrouter_fallback_ids(&self, id: &WireString) -> Vec<WireString> {
        let mut candidates = Vec::new();
        let mut pending = VecDeque::from([id.clone()]);
        let mut seen = HashSet::new();
        while let Some(candidate) = pending.pop_front() {
            if candidate.is_empty() || !seen.insert(candidate.clone()) {
                continue;
            }
            candidates.push(candidate.clone());
            if let Some(base) = openrouter_route_base(&candidate) {
                pending.push_back(base);
            }
            if let Some(date) =
                self.openrouter_date_suffix.lock().unwrap_or_else(|error| error.into_inner()).exec(&candidate)
            {
                let mut units = candidate.units()[..date.index].to_vec();
                units.extend(&candidate.units()[date.end..]);
                pending.push_back(WireString::from_units(units));
            }
        }
        candidates
    }

    pub fn resolve(&self, provider: &WireString, id: &WireString) -> Result<Option<HostModelRef>, CollapseError> {
        let normalized_provider = lower(&trim(provider));
        let normalized_id = lower(&trim(id));
        if normalized_provider.is_empty() || normalized_id.is_empty() {
            return Ok(None);
        }
        let key = provider_key(&normalized_provider, &normalized_id);
        if let Some(exact) = self.exact.get(&key) {
            return Ok(exact.clone());
        }
        let alias = self
            .collapse_runtime
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .resolve_variant_selector(&normalized_provider, &normalized_id)?
            .or_else(|| thinking_variant_alias(&normalized_provider, &normalized_id));
        if let Some(alias) = alias.filter(|alias| !alias.is_empty())
            && let Some(Some(model)) = self.exact.get(&provider_key(&normalized_provider, &lower(&alias)))
        {
            return Ok(Some(model.clone()));
        }
        let routes = self
            .wire_routes
            .get_or_init(|| build_wire_route_index(&self.models))
            .as_ref()
            .map_err(|error| error.clone())?;
        if let Some(Some(model)) = routes.get(&key) {
            return Ok(Some(model.clone()));
        }
        if let Some(profile) = self.bedrock_profile(provider, id)? {
            return Ok(Some(profile));
        }
        if let Some(Some(model)) =
            self.spelling.get(&provider_key(&normalized_provider, &revision_spelling_key(&normalized_id)))
        {
            return Ok(Some(model.clone()));
        }
        if !normalized_provider.equals_ascii("openrouter") {
            return Ok(None);
        }
        for fallback_id in self.openrouter_fallback_ids(id).into_iter().skip(1) {
            if let Some(fallback) = self.exact.get(&provider_key(&normalized_provider, &lower(&fallback_id))) {
                let Some(fallback) = fallback else { return Ok(None) };
                let mut spec = (**fallback.spec()).clone();
                if text(&spec, "name") == text(&spec, "id") {
                    spec.set("name", WireValue::String(id.clone()));
                }
                spec.set("id", WireValue::String(id.clone()));
                return fallback.with_spec(Arc::new(spec)).map(Some);
            }
        }
        Ok(None)
    }
}
