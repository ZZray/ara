//! Pure startup helpers from fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d.
//! Source: coding-agent/src/config/model-resolver.ts:70-105,114-155,209-229.
//! Provider defaults derive from catalog/src/provider-models/descriptors.ts:615-617.
//! Saved selectors use the SDK's allowMaxSuffix + allowAutoAlias context. The
//! Host owns availability, auth observations, discovery, routing and persistence.
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//! MIT; see THIRD_PARTY_NOTICES.md.

use std::collections::HashMap;
use std::sync::Arc;

use ara_rpc::WireString;

use crate::model_identity::build_model_provider_priority_rank;
use crate::model_identity_wire::text;
use crate::model_patch::HostModelRef;
use crate::provider_models::descriptors::default_model_per_provider;
use crate::retry_fallback::{ConfiguredThinkingLevel, ThinkingLevel, parse_thinking_level};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSavedModel {
    pub provider: String,
    pub id: String,
    pub thinking_level: Option<ConfiguredThinkingLevel>,
}

/// Fixed SDK parseModelString context: split the first slash and only the last
/// recognized thinking suffix. Literal IDs protect max/auto, while strict
/// concrete suffixes are extracted before the literal lookup.
pub fn parse_saved_model(value: &str, is_literal: &dyn Fn(&str, &str) -> bool) -> Option<ParsedSavedModel> {
    let slash = value.find('/').filter(|slash| *slash > 0)?;
    let provider = &value[..slash];
    let id = &value[slash + 1..];
    let mut parsed = ParsedSavedModel { provider: provider.to_owned(), id: id.to_owned(), thinking_level: None };
    let Some(colon) = id.rfind(':') else {
        return Some(parsed);
    };
    let suffix = &id[colon + 1..];
    let concrete = parse_thinking_level(suffix);
    if let Some(level) = concrete.filter(|level| *level != ThinkingLevel::Max) {
        parsed.id = id[..colon].to_owned();
        parsed.thinking_level = Some(ConfiguredThinkingLevel::Concrete(level));
    } else {
        let guarded = concrete
            .map(ConfiguredThinkingLevel::Concrete)
            .or_else(|| (suffix == "auto").then_some(ConfiguredThinkingLevel::Auto));
        if let Some(level) = guarded
            && !is_literal(provider, id)
        {
            parsed.id = id[..colon].to_owned();
            parsed.thinking_level = Some(level);
        }
    }
    Some(parsed)
}

/// Prefer concrete credential providers when any exist, then the first known
/// provider default in availability order. Canonical provider priority only
/// ranks the providers sharing that first default ID; unrelated defaults retain
/// their input order. Returns the original HostModelRef, including its sidecar.
pub fn pick_default_available_model(
    available: &[HostModelRef],
    has_concrete_auth: &dyn Fn(&WireString) -> bool,
) -> Option<HostModelRef> {
    let mut concrete_by_provider = HashMap::new();
    let concrete: Vec<_> = available
        .iter()
        .filter(|model| {
            let provider = text(model.spec(), "provider").unwrap_or_else(|| "".into());
            *concrete_by_provider.entry(provider.clone()).or_insert_with(|| has_concrete_auth(&provider))
        })
        .collect();
    let models: Vec<_> = if concrete.is_empty() { available.iter().collect() } else { concrete };
    let defaults: HashMap<_, _> = default_model_per_provider().into_iter().collect();
    let is_default = |model: &HostModelRef| {
        let provider = text(model.spec(), "provider").unwrap_or_else(|| "".into());
        let id = text(model.spec(), "id").unwrap_or_else(|| "".into());
        defaults.get(&provider) == Some(&id)
    };
    let Some(first_default) = models.iter().find(|model| is_default(model)) else {
        return models.first().map(|model| Arc::clone(model));
    };
    let first_id = text(first_default.spec(), "id").unwrap_or_else(|| "".into());
    let priority = build_model_provider_priority_rank(&[]);
    models
        .into_iter()
        .enumerate()
        .filter(|(_, model)| is_default(model) && text(model.spec(), "id").unwrap_or_else(|| "".into()) == first_id)
        .min_by_key(|(index, model)| {
            let rank = text(model.spec(), "provider")
                .and_then(|provider| provider.to_utf8().ok())
                .and_then(|provider| priority.get(&provider.to_lowercase()).and_then(serde_json::Value::as_u64))
                .unwrap_or(u64::MAX);
            (rank, *index)
        })
        .map(|(_, model)| Arc::clone(model))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use serde_json::json;

    use super::*;
    use crate::model_collapse::VariantSpec;
    use crate::model_patch::{HeaderSlot, build_host_model};

    fn model(provider: &str, id: &str) -> HostModelRef {
        build_host_model(
            &VariantSpec::from_json(&json!({
                "provider": provider,
                "id": id,
                "name": id,
                "api": "openai-completions",
                "baseUrl": "https://startup.test.invalid/v1",
                "reasoning": false,
                "input": ["text"],
                "contextWindow": 128_000,
                "maxTokens": 8192,
                "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}
            })),
            HeaderSlot::Absent,
        )
        .unwrap()
    }

    fn default_model(provider: &str) -> HostModelRef {
        let id = default_model_per_provider()
            .into_iter()
            .find(|(candidate, _)| candidate.equals_ascii(provider))
            .expect("fixed known provider")
            .1;
        model(provider, &id.to_utf8().unwrap())
    }

    fn assert_pick(available: &[HostModelRef], has_auth: &dyn Fn(&WireString) -> bool, expected: &HostModelRef) {
        let selected = pick_default_available_model(available, has_auth).expect("selected model");
        assert!(Arc::ptr_eq(&selected, expected));
    }

    mod saved_selector {
        use super::*;

        // Native model-resolver.test.ts:1819-1921, using the SDK's flags.
        #[test]
        fn rejects_only_missing_or_leading_slash_and_preserves_raw_parts() {
            for value in ["", "model", "model:high", "/model"] {
                assert!(parse_saved_model(value, &|_, _| false).is_none(), "{value}");
            }
            for (value, provider, id) in [
                ("anthropic/model", "anthropic", "model"),
                ("openrouter/qwen/model:exacto", "openrouter", "qwen/model:exacto"),
                ("provider/", "provider", ""),
                ("provider//nested", "provider", "/nested"),
                (" provider / model ", " provider ", " model "),
                ("供应商/模型", "供应商", "模型"),
            ] {
                assert_eq!(
                    parse_saved_model(value, &|_, _| false),
                    Some(ParsedSavedModel { provider: provider.into(), id: id.into(), thinking_level: None }),
                    "{value}"
                );
            }
        }

        #[test]
        fn extracts_strict_levels_and_prefixes_before_literal_lookup() {
            for (suffix, level) in [
                ("inherit", ThinkingLevel::Inherit),
                ("off", ThinkingLevel::Off),
                ("minimal", ThinkingLevel::Minimal),
                ("low", ThinkingLevel::Low),
                ("medium", ThinkingLevel::Medium),
                ("high", ThinkingLevel::High),
                ("xhigh", ThinkingLevel::XHigh),
                ("inh", ThinkingLevel::Inherit),
                ("mi", ThinkingLevel::Minimal),
                ("me", ThinkingLevel::Medium),
                ("hi", ThinkingLevel::High),
            ] {
                let value = format!("provider/nested/model:literal:{suffix}");
                let parsed =
                    parse_saved_model(&value, &|_, _| panic!("strict suffix does not probe literal ID")).unwrap();
                assert_eq!(parsed.provider, "provider");
                assert_eq!(parsed.id, "nested/model:literal");
                assert_eq!(parsed.thinking_level, Some(ConfiguredThinkingLevel::Concrete(level)));
            }
            let parsed = parse_saved_model("provider/:high", &|_, _| true).unwrap();
            assert_eq!(parsed.id, "");
            assert_eq!(parsed.thinking_level, Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::High)));
        }

        #[test]
        fn max_and_auto_keep_proven_literal_ids_or_extract_the_selector() {
            for (suffix, level) in [
                ("max", ConfiguredThinkingLevel::Concrete(ThinkingLevel::Max)),
                ("ma", ConfiguredThinkingLevel::Concrete(ThinkingLevel::Max)),
                ("auto", ConfiguredThinkingLevel::Auto),
            ] {
                let id = format!("nested/model:{suffix}");
                let value = format!("provider/{id}");
                let calls = RefCell::new(Vec::new());
                let literal = parse_saved_model(&value, &|provider, candidate| {
                    calls.borrow_mut().push((provider.to_owned(), candidate.to_owned()));
                    provider == "provider" && candidate == id
                })
                .unwrap();
                assert_eq!(literal.id, id);
                assert_eq!(literal.thinking_level, None);
                assert_eq!(*calls.borrow(), [("provider".to_owned(), id)]);
                let selected = parse_saved_model(&value, &|_, _| false).unwrap();
                assert_eq!(selected.id, "nested/model");
                assert_eq!(selected.thinking_level, Some(level));
            }
        }

        #[test]
        fn unrecognized_or_noncanonical_suffixes_remain_literal_without_a_probe() {
            for suffix in ["", "constructor", "extended", "exacto", "m", "au", "AUTO", "High", " high", "high "] {
                let id = format!("model:{suffix}");
                let parsed = parse_saved_model(&format!("provider/{id}"), &|_, _| {
                    panic!("invalid suffix does not probe literal ID")
                })
                .unwrap();
                assert_eq!(parsed.id, id);
                assert_eq!(parsed.thinking_level, None);
            }
        }
    }

    mod default_picker {
        use super::*;

        // Native model-resolver.test.ts:370-495 and sdk-model-selection:1292+.
        #[test]
        fn provider_default_beats_catalog_first_and_unknowns_do_not_become_defaults() {
            let earlier = model("anthropic", "catalog-first");
            let unknown = model("custom", "claude-opus-4-8");
            let default = default_model("anthropic");
            assert_pick(&[earlier, unknown, default.clone()], &|_| true, &default);
        }

        #[test]
        fn same_default_id_uses_canonical_oauth_priority() {
            for (paid_provider, oauth_provider) in [("openai", "openai-codex"), ("xai", "xai-oauth")] {
                let paid = default_model(paid_provider);
                let oauth = default_model(oauth_provider);
                assert_eq!(text(paid.spec(), "id"), text(oauth.spec(), "id"));
                assert_pick(&[paid.clone(), oauth.clone()], &|_| true, &oauth);
                assert_pick(&[oauth.clone(), paid.clone()], &|_| true, &oauth);
                assert_pick(std::slice::from_ref(&paid), &|_| true, &paid);
            }
        }

        #[test]
        fn unrelated_defaults_keep_input_order_and_only_known_defaults_share_priority() {
            let anthropic = default_model("anthropic");
            let openai = default_model("openai");
            let codex = default_model("openai-codex");
            assert_pick(&[anthropic.clone(), openai.clone(), codex.clone()], &|_| true, &anthropic);
            assert_pick(&[openai.clone(), anthropic, codex.clone()], &|_| true, &codex);
            let nondefault_codex =
                model("openai-codex", &text(default_model("xai").spec(), "id").unwrap().to_utf8().unwrap());
            let xai = default_model("xai");
            assert_pick(&[xai.clone(), nondefault_codex], &|_| true, &xai);
        }

        #[test]
        fn fixed_login_validated_default_beats_newer_unrelated_provider_default() {
            let zhipu = default_model("zhipu-coding-plan");
            let newer = model("zhipu-coding-plan", "glm-5.2");
            let zai = default_model("zai");
            assert_pick(&[zhipu.clone(), newer, zai], &|_| true, &zhipu);
        }

        #[test]
        fn concrete_provider_pool_wins_before_default_priority() {
            let bedrock = default_model("amazon-bedrock");
            let anthropic = default_model("anthropic");
            let models = [bedrock.clone(), anthropic.clone()];
            assert_pick(&models, &|_| false, &bedrock);
            assert_pick(&models, &|provider| provider.equals_ascii("anthropic"), &anthropic);
            let concrete_custom = model("custom", "not-a-known-default");
            assert_pick(
                &[bedrock, concrete_custom.clone(), anthropic],
                &|provider| provider.equals_ascii("custom"),
                &concrete_custom,
            );
        }

        #[test]
        fn ambient_only_provider_is_kept_and_concrete_probe_runs_once_per_provider() {
            let bedrock = default_model("amazon-bedrock");
            assert_pick(std::slice::from_ref(&bedrock), &|_| false, &bedrock);
            let another = model("amazon-bedrock", "another-model");
            let anthropic = default_model("anthropic");
            let calls = RefCell::new(Vec::new());
            assert_pick(
                &[bedrock, another, anthropic.clone()],
                &|provider| {
                    calls.borrow_mut().push(provider.clone());
                    provider.equals_ascii("anthropic")
                },
                &anthropic,
            );
            assert_eq!(*calls.borrow(), [WireString::from("amazon-bedrock"), WireString::from("anthropic")]);
        }

        #[test]
        fn empty_and_no_default_pools_preserve_availability_order_and_case() {
            assert!(pick_default_available_model(&[], &|_| panic!("empty pool does not probe auth")).is_none());
            let first = model("custom", "first");
            let second = model("custom", "second");
            assert_pick(&[first.clone(), second], &|_| false, &first);
            let uppercase =
                model("Anthropic", &text(default_model("anthropic").spec(), "id").unwrap().to_utf8().unwrap());
            let known = default_model("openai");
            assert_pick(&[uppercase, known.clone()], &|_| true, &known);
        }

        #[test]
        fn equal_rank_keeps_original_reference_and_sidecar() {
            let first = default_model("openai-codex");
            let second = default_model("openai-codex");
            assert!(!Arc::ptr_eq(&first, &second));
            assert_pick(&[first.clone(), second], &|_| true, &first);
        }
    }
}
