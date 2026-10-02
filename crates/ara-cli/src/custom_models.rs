//! Complete fixed OMP `config/custom-models.ts` composition helpers.
//! Source: 596f2da7101178214aa27a753529d15e6b7ad91d. Credentials/live
//! headers remain in the Host sidecar; model facts retain UTF-16/undefined.
//!
//! MIT License; Copyright (c) 2025 Mario Zechner;
//! Copyright (c) 2025-2026 Can Bölük; Copyright (c) 2026 Stencil Labs, Inc.
//! See LICENSE for the full license.

use crate::{
    model_collapse::{CollapseError, CollapseRuntime, VariantSpec, trim, truthy},
    model_config_values::HeaderResolutionOptions,
    model_identity_wire::{
        bundled_model_reference_index, copy_field, empty, inherit_reference_thinking, resolve_model_reference, text,
    },
    model_patch::{
        HeaderSlot, HostModelRef, ModelPatch, build_host_model, merge_compat, merge_header_sources,
        merge_remote_compaction_config,
    },
    retry_fallback::{ConfiguredThinkingLevel, ThinkingLevel, parse_thinking_level},
};
use ara_rpc::{WireString, WireValue};
use std::collections::HashMap;

/// Definitions and overlays have an open fact record and an opaque header
/// sidecar. Neither wrapper implements Debug or Serialize.
pub type CustomModelDefinitionLike = ModelPatch;
pub type CustomModelOverlay = ModelPatch;

#[derive(Clone, Copy, Default)]
pub struct CustomModelBuildOptions {
    pub use_defaults: bool,
}

fn merge_custom_model_headers(
    provider: &HeaderSlot,
    model: &HeaderSlot,
    auth_header: Option<bool>,
    api_key_config: Option<&str>,
) -> HeaderSlot {
    merge_auth_header_sources(&[provider.clone(), model.clone()], auth_header, api_key_config)
}

pub fn merge_auth_header_sources(
    sources: &[HeaderSlot],
    auth_header: Option<bool>,
    api_key_config: Option<&str>,
) -> HeaderSlot {
    merge_header_sources(
        sources,
        HeaderResolutionOptions {
            auth_header: auth_header == Some(true),
            api_key_config: api_key_config.map(str::to_owned),
        },
    )
}

fn resolve_custom_model_is_oauth(api: &WireString, provider_auth: Option<&WireString>) -> Option<bool> {
    if provider_auth.is_some_and(|auth| auth.equals_ascii("oauth")) {
        return Some(true);
    }
    if provider_auth.is_some() {
        return None;
    }
    api.equals_ascii("anthropic-messages").then_some(true)
}

fn assign(target: &mut VariantSpec, key: &str, value: Option<&VariantSpec>) {
    if let Some(value) = value {
        target.set_record(key, value);
    } else {
        target.set_undefined(key);
    }
}

fn non_null(model: &VariantSpec, key: &str) -> Option<VariantSpec> {
    model.record(key).filter(|record| !matches!(record.value, WireValue::Null))
}

fn selected(model: &VariantSpec, reference: Option<&VariantSpec>, key: &str) -> Option<VariantSpec> {
    non_null(model, key).or_else(|| reference.and_then(|reference| non_null(reference, key)))
}

#[allow(clippy::too_many_arguments)]
pub fn build_custom_model_overlay(
    provider_name: &WireString,
    provider_base_url: &WireString,
    provider_api: Option<&WireString>,
    provider_headers: &HeaderSlot,
    provider_api_key: Option<&str>,
    auth_header: Option<bool>,
    provider_compat: Option<&VariantSpec>,
    provider_auth: Option<&WireString>,
    provider_remote_compaction: Option<&VariantSpec>,
    model_def: &CustomModelDefinitionLike,
) -> Result<Option<CustomModelOverlay>, CollapseError> {
    let definition = model_def.fields();
    let model_api = non_null(definition, "api");
    let api = match &model_api {
        Some(record) => record
            .value
            .as_string()
            .cloned()
            .ok_or_else(|| CollapseError::new("Custom model API must be a string".into()))?,
        None => match provider_api {
            Some(api) => api.clone(),
            None => return Ok(None),
        },
    };
    if !truthy(&WireValue::String(api.clone())) {
        return Ok(None);
    }
    let mut overlay = empty();
    copy_field(&mut overlay, "id", definition, "id");
    overlay.set("provider", WireValue::String(provider_name.clone()));
    overlay.set("api", WireValue::String(api.clone()));
    assign(
        &mut overlay,
        "baseUrl",
        Some(
            &non_null(definition, "baseUrl")
                .unwrap_or_else(|| VariantSpec::from_wire(WireValue::String(provider_base_url.clone()))),
        ),
    );
    for key in [
        "name",
        "reasoning",
        "thinking",
        "input",
        "imageInputDecoder",
        "tokenizer",
        "supportsTools",
        "cost",
        "contextWindow",
        "maxTokens",
        "omitMaxOutputTokens",
        "preferWebsockets",
    ] {
        copy_field(&mut overlay, key, definition, key);
    }
    let headers = merge_custom_model_headers(provider_headers, model_def.headers(), auth_header, provider_api_key);
    let model_compat = definition.record("compat");
    let compat = merge_compat(provider_compat, model_compat.as_ref());
    assign(&mut overlay, "compat", compat.as_ref());
    for key in ["contextPromotionTarget", "compactionModel"] {
        copy_field(&mut overlay, key, definition, key);
    }
    let model_remote = definition.record("remoteCompaction");
    let remote = merge_remote_compaction_config(provider_remote_compaction, model_remote.as_ref());
    assign(&mut overlay, "remoteCompaction", remote.as_ref());
    copy_field(&mut overlay, "premiumMultiplier", definition, "premiumMultiplier");
    if let Some(value) = resolve_custom_model_is_oauth(&api, provider_auth) {
        overlay.set("isOAuth", WireValue::Bool(value));
    } else {
        overlay.set_undefined("isOAuth");
    }
    Ok(Some(ModelPatch::new(overlay, headers)?))
}

fn apply_standalone_custom_model_policies(model: &VariantSpec) -> VariantSpec {
    if !text(model, "id").is_some_and(|id| id.equals_ascii("gpt-5.4"))
        || text(model, "provider").is_some_and(|provider| provider.equals_ascii("github-copilot"))
        || model.get("contextWindow").is_some()
    {
        return model.clone();
    }
    let mut model = model.clone();
    model.set("contextWindow", WireValue::Number(1_000_000.0));
    model
}

pub fn finalize_custom_model(
    model: &CustomModelOverlay,
    options: CustomModelBuildOptions,
) -> Result<HostModelRef, CollapseError> {
    let resolved = if options.use_defaults {
        apply_standalone_custom_model_policies(model.fields())
    } else {
        model.fields().clone()
    };
    let id = text(&resolved, "id").ok_or_else(|| CollapseError::new("Custom model ID must be a string".into()))?;
    let provider = text(&resolved, "provider")
        .ok_or_else(|| CollapseError::new("Custom model provider must be a string".into()))?;
    let reference =
        if options.use_defaults { resolve_model_reference(&id, bundled_model_reference_index()) } else { None };
    let reference = reference.as_deref();
    let cost = selected(&resolved, reference, "cost").or_else(|| {
        options.use_defaults.then(|| {
            VariantSpec::from_wire(WireValue::object(vec![
                ("input", WireValue::Number(0.0)),
                ("output", WireValue::Number(0.0)),
                ("cacheRead", WireValue::Number(0.0)),
                ("cacheWrite", WireValue::Number(0.0)),
            ]))
        })
    });
    let input = selected(&resolved, reference, "input").or_else(|| {
        options.use_defaults.then(|| VariantSpec::from_wire(WireValue::Array(vec![WireValue::String("text".into())])))
    });
    let supports_tools = selected(&resolved, reference, "supportsTools");
    let mut spec = empty();
    copy_field(&mut spec, "id", &resolved, "id");
    let name = non_null(&resolved, "name")
        .or_else(|| options.use_defaults.then(|| VariantSpec::from_wire(WireValue::String(id.clone()))));
    assign(&mut spec, "name", name.as_ref());
    for key in ["api", "provider", "baseUrl"] {
        copy_field(&mut spec, key, &resolved, key);
    }
    let reasoning = selected(&resolved, reference, "reasoning")
        .or_else(|| options.use_defaults.then(|| VariantSpec::from_wire(WireValue::Bool(false))));
    assign(&mut spec, "reasoning", reasoning.as_ref());
    let model_thinking = resolved.record("thinking");
    let thinking = inherit_reference_thinking(model_thinking.as_ref(), reference, &provider);
    assign(&mut spec, "thinking", thinking.as_ref());
    assign(&mut spec, "input", input.as_ref());
    copy_field(&mut spec, "imageInputDecoder", &resolved, "imageInputDecoder");
    if let Some(supports_tools) = supports_tools {
        spec.set_record("supportsTools", &supports_tools);
    }
    assign(&mut spec, "cost", cost.as_ref());
    for (key, fallback) in [("contextWindow", 128_000.0), ("maxTokens", 16_384.0)] {
        let value = selected(&resolved, reference, key).unwrap_or_else(|| {
            VariantSpec::from_wire(if options.use_defaults { WireValue::Number(fallback) } else { WireValue::Null })
        });
        spec.set_record(key, &value);
    }
    let omit = selected(&resolved, reference, "omitMaxOutputTokens");
    assign(&mut spec, "omitMaxOutputTokens", omit.as_ref());
    copy_field(&mut spec, "preferWebsockets", &resolved, "preferWebsockets");
    let reference_compat = reference.and_then(|reference| reference.record("compatConfig"));
    let model_compat = resolved.record("compat");
    let compat = merge_compat(reference_compat.as_ref(), model_compat.as_ref());
    assign(&mut spec, "compat", compat.as_ref());
    for key in
        ["tokenizer", "contextPromotionTarget", "compactionModel", "remoteCompaction", "premiumMultiplier", "isOAuth"]
    {
        copy_field(&mut spec, key, &resolved, key);
    }
    build_host_model(&spec, model.headers().clone())
}

/// Narrow complete pure dependency from model-resolver.ts:115-153,209-229.
/// String slicing stays on UTF-16; strict suffixes precede literal-ID checks.
pub type ModelIdPredicate<'a> = dyn Fn(&WireString, &WireString) -> bool + 'a;

#[derive(Default)]
pub struct ModelStringParseOptions<'a> {
    pub allow_max_suffix: bool,
    pub allow_auto_alias: bool,
    pub is_literal_model_id: Option<&'a ModelIdPredicate<'a>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedModelString {
    pub provider: WireString,
    pub id: WireString,
    pub thinking_level: Option<ConfiguredThinkingLevel>,
}

fn parse_thinking_suffix(value: &WireString, options: &ModelStringParseOptions<'_>) -> Option<ConfiguredThinkingLevel> {
    let text = value.to_utf8().ok()?;
    let level = parse_thinking_level(&text);
    if level == Some(ThinkingLevel::Max) {
        return options.allow_max_suffix.then_some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::Max));
    }
    if let Some(level) = level {
        return Some(ConfiguredThinkingLevel::Concrete(level));
    }
    (options.allow_auto_alias && value.equals_ascii("auto")).then_some(ConfiguredThinkingLevel::Auto)
}

fn split_thinking_suffix(
    pattern: &WireString,
    min_colon_index: isize,
    options: &ModelStringParseOptions<'_>,
) -> (WireString, Option<ConfiguredThinkingLevel>) {
    let Some(index) = pattern.units().iter().rposition(|unit| *unit == 58) else { return (pattern.clone(), None) };
    if index as isize <= min_colon_index {
        return (pattern.clone(), None);
    }
    let suffix = WireString::from_units(pattern.units()[index + 1..].to_vec());
    match parse_thinking_suffix(&suffix, options) {
        Some(level) => (WireString::from_units(pattern.units()[..index].to_vec()), Some(level)),
        None => (pattern.clone(), None),
    }
}

pub fn parse_model_string(model: &WireString, options: ModelStringParseOptions<'_>) -> Option<ParsedModelString> {
    let slash = model.units().iter().position(|unit| *unit == 47)?;
    if slash == 0 {
        return None;
    }
    let provider = WireString::from_units(model.units()[..slash].to_vec());
    let id = WireString::from_units(model.units()[slash + 1..].to_vec());
    let (strict, level) = split_thinking_suffix(&id, -1, &ModelStringParseOptions::default());
    if level.is_some() {
        return Some(ParsedModelString { provider, id: strict, thinking_level: level });
    }
    let (max_alias, level) = split_thinking_suffix(&id, -1, &options);
    if level.is_some() {
        return if options.is_literal_model_id.is_some_and(|literal| literal(&provider, &id)) {
            Some(ParsedModelString { provider, id, thinking_level: None })
        } else {
            Some(ParsedModelString { provider, id: max_alias, thinking_level: level })
        };
    }
    Some(ParsedModelString { provider, id, thinking_level: None })
}

pub fn normalize_suppressed_selector(
    selector: &WireString,
    runtime: &mut CollapseRuntime,
    has_live_model: Option<&ModelIdPredicate<'_>>,
) -> Result<WireString, CollapseError> {
    let trimmed = trim(selector);
    if trimmed.is_empty() {
        return Ok(trimmed);
    }
    let Some(parsed) = parse_model_string(
        &trimmed,
        ModelStringParseOptions { allow_max_suffix: true, allow_auto_alias: true, is_literal_model_id: has_live_model },
    ) else {
        return Ok(trimmed);
    };
    let id = runtime.resolve_variant_selector(&parsed.provider, &parsed.id)?.unwrap_or(parsed.id);
    let mut units = parsed.provider.units().to_vec();
    units.push(47);
    units.extend_from_slice(id.units());
    Ok(WireString::from_units(units))
}

pub fn resolve_model_override_with_aliases<'a>(
    overrides: &'a HashMap<WireString, ModelPatch>,
    model: &HostModelRef,
    runtime: &mut CollapseRuntime,
    has_live_model: &dyn Fn(&WireString, &WireString) -> bool,
) -> Result<Option<&'a ModelPatch>, CollapseError> {
    let id = text(model.spec(), "id").ok_or_else(|| CollapseError::new("Model ID must be a string".into()))?;
    let provider =
        text(model.spec(), "provider").ok_or_else(|| CollapseError::new("Model provider must be a string".into()))?;
    if let Some(direct) = overrides.get(&id) {
        return Ok(Some(direct));
    }
    for raw_id in runtime.get_variant_alias_sources(&provider, &id)?.snapshot() {
        if has_live_model(&provider, &raw_id) {
            continue;
        }
        if let Some(remapped) = overrides.get(&raw_id) {
            return Ok(Some(remapped));
        }
    }
    Ok(None)
}
