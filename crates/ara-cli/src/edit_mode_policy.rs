//! Host edit policy from pinned OMP b91db66e; the classifier is reused unchanged.
//! Sources: utils/edit-mode.ts:18-54, config/settings.ts:1004-1040,
//! edit/index.ts:126-130,312-333, utils/env.ts:424-442. MIT; THIRD_PARTY_NOTICES.md.
//! The Host supplies its ordered settings, environment and exact selected model
//! metadata. This module performs no configuration, authentication or tool I/O.

use anyhow::{Context as _, Result, bail};
use ara_cli::catalog_rules::{ClassifyOptions, classify_over_taxonomy};
use ara_edit::EditMode;
use ara_rpc::WireString;
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Clone, Debug, Default)]
pub struct EditModeSettings {
    /// Raw native edit.mode value; String(value) conversion belongs to resolution.
    pub mode: Option<Value>,
    /// The Host supplies native own-property order, including integer-key order.
    /// Invalid/non-string mode values are ignored, not rejected or re-sorted.
    pub model_variants: Vec<(String, Value)>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EditModeEnvironment<'a> {
    /// ARA_EDIT_VARIANT, renamed from the fixed native PI_EDIT_VARIANT.
    pub variant: Option<&'a str>,
    /// ARA_STRICT_EDIT_MODE, with the fixed native parseFlag truth values.
    pub strict: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedEditMode {
    /// Native EditTool can capture a constructor/environment override.
    pub tool_mode: EditMode,
    /// File-display helpers independently resolve the current model/settings.
    pub display_mode: EditMode,
}

/// `--edit-mode` preserves ARA's explicit constructor override for both modes.
/// Otherwise the dynamic display resolver is native modelVariants -> env ->
/// settings/default/adaptive, while an exposed EditTool captures valid env first.
/// `model_metadata` is the exact donor or DailySelection metadata: catalogModelId
/// (when present) is the native selector ID; id may be the execution wire ID.
pub fn resolve_edit_mode(
    cli_override: Option<EditMode>,
    settings: &EditModeSettings,
    environment: EditModeEnvironment<'_>,
    model_metadata: Option<&Value>,
    edit_enabled: bool,
) -> Result<ResolvedEditMode> {
    if let Some(mode) = cli_override {
        return Ok(ResolvedEditMode {
            tool_mode: mode,
            display_mode: mode,
        });
    }
    let _ = edit_enabled; // The target resolves live mode for every tool consumer.
    let selector = model_metadata.map(active_model_selector).transpose()?;
    let mode = resolve_display_mode(settings, environment, selector.as_deref())?;
    Ok(ResolvedEditMode {
        tool_mode: mode,
        display_mode: mode,
    })
}

fn active_model_selector(metadata: &Value) -> Result<String> {
    let provider = metadata
        .get("provider")
        .and_then(Value::as_str)
        .context("edit model provider is missing")?;
    let id = match metadata.get("catalogModelId") {
        Some(Value::String(id)) => id.as_str(),
        None | Some(Value::Null) => metadata
            .get("id")
            .and_then(Value::as_str)
            .context("edit model ID is missing")?,
        Some(_) => bail!("edit catalog model ID must be a string"),
    };
    Ok(format!("{provider}/{id}"))
}

fn resolve_display_mode(
    settings: &EditModeSettings,
    environment: EditModeEnvironment<'_>,
    selector: Option<&str>,
) -> Result<EditMode> {
    if let Some(selector) = selector {
        let model_lower = ara_cli::catalog_discovery::js_lower(&WireString::from(selector));
        for (pattern, raw_mode) in &settings.model_variants {
            let Some(mode) = raw_mode.as_str().and_then(EditMode::parse) else {
                continue;
            };
            let pattern_lower =
                ara_cli::catalog_discovery::js_lower(&WireString::from(pattern.as_str()));
            if pattern_lower.is_empty()
                || model_lower
                    .units()
                    .windows(pattern_lower.len())
                    .any(|part| part == pattern_lower.units())
            {
                return Ok(mode);
            }
        }
    }
    if let Some(mode) = environment.variant.and_then(parse_environment_mode) {
        return Ok(mode);
    }
    let mode = settings
        .mode
        .as_ref()
        .filter(|value| !value.is_null())
        .map(ara_prompt::js::to_string)
        .as_deref()
        .and_then(EditMode::parse)
        .unwrap_or(EditMode::Hashline);
    if mode == EditMode::Hashline
        && !native_flag(environment.strict)
        && let Some(selector) = selector
    {
        static TAXONOMY: OnceLock<Value> = OnceLock::new();
        let taxonomy = TAXONOMY.get_or_init(|| {
            serde_json::from_str(include_str!("../data/omp-edit-taxonomy.json"))
                .expect("pinned complete EDIT taxonomy")
        });
        let identity = classify_over_taxonomy(
            taxonomy,
            "",
            selector,
            ClassifyOptions {
                observed_at_ms: None,
                lenient: true,
            },
        )?;
        if matches!(
            identity.get("class").and_then(Value::as_str),
            Some("kimi" | "mimo" | "minimax" | "deepseek" | "stepfun")
        ) || identity.get("family").and_then(Value::as_str) == Some("codex-spark")
            || (identity.get("class").and_then(Value::as_str) == Some("glm")
                && identity.get("family").and_then(Value::as_str) == Some("flash")
                && identity.get("revision").and_then(Value::as_str) == Some("5.3.0"))
        {
            return Ok(EditMode::Replace);
        }
    }
    Ok(mode)
}

// config/registry.ts:588-603,626-633: only the outer text is trimmed.
// A decoded JSON string is validated directly, without a second trim.
fn parse_environment_mode(raw: &str) -> Option<EditMode> {
    let trimmed = ara_prompt::js::trim(raw);
    if trimmed.starts_with('"')
        && let Ok(Value::String(value)) = serde_json::from_str::<Value>(trimmed)
    {
        return EditMode::parse(&value);
    }
    EditMode::parse(trimmed)
}

fn native_flag(value: Option<&str>) -> bool {
    matches!(
        value,
        Some("1" | "Y" | "y" | "TRUE" | "true" | "YES" | "yes" | "ON" | "on")
    )
}

/// Fixed edit/index.ts constructor environment, renamed to the ARA prefix.
#[derive(Clone, Copy, Debug, Default)]
pub struct EditSettingsEnvironment<'a> {
    pub fuzzy: Option<&'a str>,
    pub fuzzy_threshold: Option<&'a str>,
}

/// Host-owned raw Settings.get leaves. Missing differs from explicit null.
/// Carry the original YAML values without typed validation or conversion so an
/// environment branch can bypass a setting the fixed constructor never reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditSettingsSource {
    pub allow_fuzzy: Option<yaml_rust2::Yaml>,
    pub fuzzy_threshold: Option<yaml_rust2::Yaml>,
    pub enforce_seen_lines: Option<yaml_rust2::Yaml>,
    pub block_auto_generated: Option<yaml_rust2::Yaml>,
}

/// Called only when the Host constructs an enabled Edit tool. Resolve the fixed
/// env branch first, then consume only the settings leaves that branch uses.
/// The output is the existing immutable typed ToolContext snapshot.
pub fn resolve_edit_settings(
    source: &EditSettingsSource,
    environment: EditSettingsEnvironment<'_>,
) -> Result<ara_tools::edit::EditSettings> {
    let defaults = ara_tools::edit::EditSettings::default();
    let boolean = |raw: &Option<yaml_rust2::Yaml>, name: &str, default: bool| -> Result<bool> {
        match raw {
            None => Ok(default),
            Some(raw) => raw
                .as_bool()
                .with_context(|| format!("edit.{name} must be a boolean")),
        }
    };
    let allow_fuzzy = match environment.fuzzy.unwrap_or("auto") {
        "true" | "1" => true,
        "false" | "0" => false,
        _ => boolean(&source.allow_fuzzy, "fuzzyMatch", defaults.allow_fuzzy)?,
    };
    // The registry treats an invalid environment value as unset. A valid
    // Number.parseFloat prefix can bypass a setting the target never reads.
    let threshold_from_env = environment.fuzzy_threshold.and_then(|raw| {
        let mut expression = ara_cli::js_regex::JsRegExp::new(
            r"^[\s]*([+-]?(?:Infinity|(?:[0-9]+\.?[0-9]*|\.[0-9]+)(?:[eE][+-]?[0-9]+)?))".into(),
            "",
        )
        .expect("fixed edit threshold parseFloat regex");
        expression
            .exec(&WireString::from(raw))
            .and_then(|matched| matched.captures.get(1).cloned().flatten())
            .and_then(|prefix| prefix.to_utf8().ok())
            .and_then(|prefix| prefix.parse::<f64>().ok())
            .filter(|value| (0.0..=1.0).contains(value))
    });
    let fuzzy_threshold = match threshold_from_env {
        Some(value) => value,
        None => match &source.fuzzy_threshold {
            None => defaults.fuzzy_threshold,
            Some(raw) => raw
                .as_f64()
                .or_else(|| raw.as_i64().map(|value| value as f64))
                .context("edit.fuzzyThreshold must be a number")?,
        },
    };
    Ok(ara_tools::edit::EditSettings {
        allow_fuzzy,
        fuzzy_threshold,
        enforce_seen_lines: boolean(
            &source.enforce_seen_lines,
            "enforceSeenLines",
            defaults.enforce_seen_lines,
        )?,
        block_auto_generated: boolean(
            &source.block_auto_generated,
            "blockAutoGenerated",
            defaults.block_auto_generated,
        )?,
    })
}
