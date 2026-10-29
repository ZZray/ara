//! Pure Session selector and startup selection from fixed OMP 596f2da.
//! Source: coding-agent/src/thinking.ts and sdk.ts:1585-1611; model clamp
//! follows catalog/src/model-thinking.ts:35-64. MIT; THIRD_PARTY_NOTICES.md.
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//! The Host owns persistence, Auto classification, route admission and wire
//! normalization. A selection alone does not prove that a ceiling is honored.

use ara_ai::{OpenAIReasoningPolicy, ReasoningEffort, ReasoningRequest};

use crate::auto_thinking::{apply_session_ceiling, provisional_effort};
use crate::retry_fallback::{ConfiguredThinkingLevel, ThinkingLevel, parse_thinking_level};

const CANONICAL_EFFORTS: [ReasoningEffort; 6] = [
    ReasoningEffort::Minimal,
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
    ReasoningEffort::XHigh,
    ReasoningEffort::Max,
];

/// Fixed thinking.ts:204-207: Auto is exact; concrete selectors retain the
/// existing unambiguous prefix grammar, with no trimming or case conversion.
pub fn parse_configured(value: &str) -> Option<ConfiguredThinkingLevel> {
    if value == "auto" {
        Some(ConfiguredThinkingLevel::Auto)
    } else {
        parse_thinking_level(value).map(ConfiguredThinkingLevel::Concrete)
    }
}

/// Fixed thinking.ts:221-224. Reject inherit, including prefixes such as
/// `in`/`inh`; other concrete abbreviations stay valid Native CLI selectors.
pub fn parse_cli(value: &str) -> Option<ConfiguredThinkingLevel> {
    match parse_configured(value) {
        Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::Inherit)) => None,
        level => level,
    }
}

pub fn configured_name(level: ConfiguredThinkingLevel) -> &'static str {
    match level {
        ConfiguredThinkingLevel::Auto => "auto",
        ConfiguredThinkingLevel::Concrete(level) => level.as_str(),
    }
}

/// Resolve the Session selector before provider normalization. Auto uses its
/// provisional pool before the Session ceiling. A concrete effort is capped
/// before model clamping, matching ModelControls' call order. Off remains an
/// explicit disable request; inherit leaves the request without an effort.
///
/// `None` policy is Native's absent-model case, where concrete efforts pass
/// through and the ceiling uses the canonical ladder. A present policy with
/// no controllable ladder resolves concrete efforts to None. An incompatible
/// sparse ladder may still snap above the ceiling: the Host must reject the
/// route after checking its actual mandatory provider normalization.
pub fn resolve_selection(
    policy: Option<&OpenAIReasoningPolicy>,
    configured: ConfiguredThinkingLevel,
    ceiling: Option<ReasoningEffort>,
) -> ReasoningRequest {
    let mut request = ReasoningRequest::default();
    match configured {
        ConfiguredThinkingLevel::Auto => {
            let provisional =
                policy.and_then(|policy| provisional_effort(policy.supported_efforts(), policy.default_effort()));
            let supported = policy.map(OpenAIReasoningPolicy::supported_efforts).unwrap_or(&[]);
            request.effort = apply_session_ceiling(supported, provisional, ceiling);
        }
        ConfiguredThinkingLevel::Concrete(ThinkingLevel::Off) => {
            request.disable_reasoning = true;
        }
        ConfiguredThinkingLevel::Concrete(ThinkingLevel::Inherit) => {}
        ConfiguredThinkingLevel::Concrete(level) => {
            let supported = policy.map(OpenAIReasoningPolicy::supported_efforts).unwrap_or(&CANONICAL_EFFORTS);
            let requested = apply_session_ceiling(supported, ReasoningEffort::parse(level.as_str()), ceiling);
            request.effort = match policy {
                Some(policy) => requested.and_then(|effort| policy.clamp_effort(effort)),
                None => requested,
            };
        }
    }
    request
}

/// Existing-entrypoint selection only: explicit, saved, selected model's
/// default, then settings. A saved/explicit inherit is a real selector and
/// stops fallback. SDK restored suffix and explicit default-role acquisition
/// remain Host integration requirements; this wrapper does not invent them.
pub fn initial_selection(
    explicit: Option<ConfiguredThinkingLevel>,
    saved: Option<ConfiguredThinkingLevel>,
    model_default: Option<ReasoningEffort>,
    settings: Option<ConfiguredThinkingLevel>,
) -> Option<ConfiguredThinkingLevel> {
    explicit.or(saved).or_else(|| model_default.map(concrete_selector)).or(settings)
}

/// Source-aware inputs for SDK's pure precedence at sdk.ts:1591-1611. Presence
/// flags are separate from parsed values: an existing invalid thinking entry
/// still suppresses restored suffix/default-role fallback in the Native SDK.
/// Host callers must source and authorize each value; this module performs no
/// model resolution, Session replay or role/settings lookup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InitialSelectionInputs {
    pub explicit: Option<ConfiguredThinkingLevel>,
    pub saved: Option<ConfiguredThinkingLevel>,
    pub has_existing_session: bool,
    pub has_thinking_entry: bool,
    pub restored_suffix: Option<ConfiguredThinkingLevel>,
    pub has_explicit_model: bool,
    pub default_role: Option<ConfiguredThinkingLevel>,
    pub default_role_explicit: bool,
    pub model_default: Option<ReasoningEffort>,
    pub settings: Option<ConfiguredThinkingLevel>,
}

pub fn initial_selection_with_sources(inputs: InitialSelectionInputs) -> Option<ConfiguredThinkingLevel> {
    inputs
        .explicit
        .or_else(|| (inputs.has_existing_session && inputs.has_thinking_entry).then_some(inputs.saved).flatten())
        .or_else(|| (!inputs.has_thinking_entry).then_some(inputs.restored_suffix).flatten())
        .or_else(|| {
            (!inputs.has_explicit_model && !inputs.has_thinking_entry && inputs.default_role_explicit)
                .then_some(inputs.default_role)
                .flatten()
        })
        .or_else(|| inputs.model_default.map(concrete_selector))
        .or(inputs.settings)
}

fn concrete_selector(effort: ReasoningEffort) -> ConfiguredThinkingLevel {
    ConfiguredThinkingLevel::Concrete(match effort {
        ReasoningEffort::Minimal => ThinkingLevel::Minimal,
        ReasoningEffort::Low => ThinkingLevel::Low,
        ReasoningEffort::Medium => ThinkingLevel::Medium,
        ReasoningEffort::High => ThinkingLevel::High,
        ReasoningEffort::XHigh => ThinkingLevel::XHigh,
        ReasoningEffort::Max => ThinkingLevel::Max,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn concrete(level: ThinkingLevel) -> ConfiguredThinkingLevel {
        ConfiguredThinkingLevel::Concrete(level)
    }

    fn policy(efforts: &[ReasoningEffort], default: Option<ReasoningEffort>) -> OpenAIReasoningPolicy {
        let mut spec = json!({"reasoning":true,"thinking":{"mode":"effort","efforts":efforts}});
        if let Some(default) = default {
            spec["thinking"]["defaultLevel"] = json!(default);
        }
        OpenAIReasoningPolicy::from_model_spec(&spec).expect("fixture has an ordered valid model ladder")
    }

    #[test]
    fn configured_and_cli_selectors_follow_native_prefix_rules() {
        let fixtures = [
            ("auto", Some(ConfiguredThinkingLevel::Auto)),
            ("off", Some(concrete(ThinkingLevel::Off))),
            ("of", Some(concrete(ThinkingLevel::Off))),
            ("mi", Some(concrete(ThinkingLevel::Minimal))),
            ("me", Some(concrete(ThinkingLevel::Medium))),
            ("hi", Some(concrete(ThinkingLevel::High))),
            ("xhi", Some(concrete(ThinkingLevel::XHigh))),
            ("ma", Some(concrete(ThinkingLevel::Max))),
            ("au", None),
            ("a", None),
            ("m", None),
            ("h", None),
            ("AUTO", None),
            ("high ", None),
            (" auto", None),
            ("toString", None),
        ];
        for (input, expected) in fixtures {
            assert_eq!(parse_configured(input), expected, "configured {input:?}");
            assert_eq!(parse_cli(input), expected, "CLI {input:?}");
            if let Some(level) = expected {
                assert_eq!(parse_configured(configured_name(level)), Some(level));
            }
        }
        for input in ["inherit", "in", "inh"] {
            assert_eq!(parse_configured(input), Some(concrete(ThinkingLevel::Inherit)));
            assert_eq!(parse_cli(input), None);
        }
    }

    #[test]
    fn startup_precedence_preserves_presence_and_explicit_inherit() {
        use ThinkingLevel::{High, Inherit, Low, Medium, Off};
        let settings = Some(ConfiguredThinkingLevel::Auto);
        assert_eq!(
            initial_selection(Some(concrete(Off)), Some(concrete(Low)), Some(ReasoningEffort::High), settings),
            Some(concrete(Off))
        );
        assert_eq!(
            initial_selection(None, Some(concrete(Inherit)), Some(ReasoningEffort::High), settings),
            Some(concrete(Inherit))
        );
        assert_eq!(initial_selection(None, None, Some(ReasoningEffort::High), settings), Some(concrete(High)));
        assert_eq!(initial_selection(None, None, None, settings), settings);

        let sources = InitialSelectionInputs {
            saved: Some(concrete(Low)),
            has_existing_session: true,
            has_thinking_entry: true,
            restored_suffix: Some(concrete(Medium)),
            default_role: Some(concrete(Off)),
            default_role_explicit: true,
            model_default: Some(ReasoningEffort::High),
            settings,
            ..Default::default()
        };
        assert_eq!(initial_selection_with_sources(sources), Some(concrete(Low)));
        assert_eq!(
            initial_selection_with_sources(InitialSelectionInputs { explicit: Some(concrete(Inherit)), ..sources }),
            Some(concrete(Inherit))
        );
        assert_eq!(
            initial_selection_with_sources(InitialSelectionInputs { saved: None, ..sources }),
            Some(concrete(High))
        );
        let no_entry = InitialSelectionInputs { has_thinking_entry: false, ..sources };
        assert_eq!(initial_selection_with_sources(no_entry), Some(concrete(Medium)));
        let no_suffix = InitialSelectionInputs { restored_suffix: None, ..no_entry };
        assert_eq!(initial_selection_with_sources(no_suffix), Some(concrete(Off)));
        assert_eq!(
            initial_selection_with_sources(InitialSelectionInputs { has_explicit_model: true, ..no_suffix }),
            Some(concrete(High))
        );
        assert_eq!(
            initial_selection_with_sources(InitialSelectionInputs {
                default_role_explicit: false,
                model_default: None,
                ..no_suffix
            }),
            settings
        );
    }

    #[test]
    fn selection_uses_native_ceiling_then_sparse_model_ladder() {
        use ReasoningEffort::{High, Low, Max, Medium, Minimal, XHigh};
        let sparse = policy(&[Minimal, Low, Max], Some(Max));
        assert_eq!(resolve_selection(Some(&sparse), ConfiguredThinkingLevel::Auto, None).effort, Some(Low));
        assert_eq!(resolve_selection(Some(&sparse), concrete(ThinkingLevel::Max), Some(XHigh)).effort, Some(Low));
        assert_eq!(resolve_selection(Some(&sparse), concrete(ThinkingLevel::Max), Some(Minimal)).effort, Some(Minimal));
        let no_auto_tier = policy(&[Minimal, Max], Some(Max));
        assert_eq!(resolve_selection(Some(&no_auto_tier), ConfiguredThinkingLevel::Auto, None).effort, None);
        let floor_above_ceiling = policy(&[High, Max], Some(High));
        // Selection must retain the source-supported upward snap so the Host
        // can reject the incompatible route, rather than inventing Medium.
        assert_eq!(
            resolve_selection(Some(&floor_above_ceiling), concrete(ThinkingLevel::Max), Some(Medium)).effort,
            Some(Max)
        );
        assert_eq!(
            resolve_selection(Some(&floor_above_ceiling), concrete(ThinkingLevel::Medium), Some(Medium)).effort,
            Some(High)
        );
        assert_eq!(resolve_selection(None, concrete(ThinkingLevel::Max), Some(Medium)).effort, Some(Medium));
        assert_eq!(resolve_selection(None, concrete(ThinkingLevel::High), None).effort, Some(High));
        assert_eq!(resolve_selection(None, ConfiguredThinkingLevel::Auto, None).effort, None);
        let no_reasoning = OpenAIReasoningPolicy::from_model_spec(&json!({"reasoning":false})).unwrap();
        assert_eq!(resolve_selection(Some(&no_reasoning), concrete(ThinkingLevel::High), None).effort, None);
        let off = resolve_selection(Some(&sparse), concrete(ThinkingLevel::Off), Some(Low));
        assert_eq!(off.effort, None);
        assert!(off.disable_reasoning);
        assert!(!off.force_reasoning_off);
        assert_eq!(
            resolve_selection(Some(&sparse), concrete(ThinkingLevel::Inherit), Some(Low)),
            ReasoningRequest::default()
        );
    }
}
