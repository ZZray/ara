//! CLI/startup selection from fixed OMP 596f2da main.ts:815-852,1143-1246,
//! 1293-1314 and sdk.ts:1495-1611,2245-2317,2620-2714.
//! MIT; Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! Extension registration/reclaim, runtime discovery and fallback controllers
//! remain distinct SDK consumers; this binds the currently registered Host.

use super::*;
use ara_cli::{
    model_patch::HostModelRef,
    model_roles::{ModelMatchPreferences, ModelRoleLookup, ModelRoleResolver, model_role_value},
    model_scope::{SessionScopedModel, resolve_allowed_models, resolve_scoped_models, to_session_scoped_models},
    retry_fallback::ConfiguredThinkingLevel,
    session_thinking::{InitialSelectionInputs, initial_selection_with_sources, parse_cli, parse_configured},
    startup_model::{parse_saved_model, pick_default_available_model},
};
use ara_rpc::{WireString, WireValue};

/// Fixed Native CLI selector rejection exits 1; infrastructure errors retain
/// their existing Host classification. Keep the exact resolver diagnostic.
#[derive(Debug)]
pub(super) struct StartupModelResolutionFailure(pub String);
impl std::fmt::Display for StartupModelResolutionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for StartupModelResolutionFailure {}

pub(super) struct StartupState {
    pub scope: Vec<SessionScopedModel>,
    pub thinking: Option<ConfiguredThinkingLevel>,
}

pub(super) struct StartupSelection {
    pub model: HostModelRef,
    pub state: StartupState,
}

pub(super) fn model_text(model: &HostModelRef, field: &str) -> Result<String> {
    model
        .spec()
        .get(field)
        .and_then(WireValue::as_string)
        .context("startup model identity is missing")?
        .to_utf8()
        .map_err(anyhow::Error::from)
}

fn js_trim(value: &str) -> &str {
    value.trim_matches(|ch| {
        matches!(ch, '\u{0009}'..='\u{000D}' | ' ' | '\u{00A0}' | '\u{1680}'
        | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}')
    })
}

/// Native args.ts keeps empty comma tokens and the final repeated flag.
fn cli_patterns(args: &Args) -> Option<Vec<String>> {
    args.models.last().map(|value| value.split(',').map(js_trim).map(str::to_owned).collect())
}

/// SDK normalizes deferred selectors independently of the CLI scope grammar.
fn normalized_deferred_patterns(values: Vec<String>) -> Vec<String> {
    values.into_iter().map(|value| js_trim(&value).to_owned()).filter(|value| !value.is_empty()).collect()
}

fn remembered_scoped_model(
    scoped: &[ara_cli::model_scope::ScopedModel],
    resolved: Option<&HostModelRef>,
    remembered: &str,
) -> Option<HostModelRef> {
    if let Some(model) = resolved {
        scoped
            .iter()
            .find(|entry| {
                entry.model.spec().get("provider") == model.spec().get("provider")
                    && entry.model.spec().get("id") == model.spec().get("id")
            })
            .map(|entry| entry.model.clone())
    } else {
        scoped
            .iter()
            .find(|entry| model_text(&entry.model, "id").is_ok_and(|id| id.to_lowercase() == remembered.to_lowercase()))
            .map(|entry| entry.model.clone())
    }
}

fn allowed_resolver(
    registry: &ara_cli::model_registry::ModelRegistry,
    patterns: &[String],
    roles: &dyn ModelRoleLookup,
    preferences: &ModelMatchPreferences,
) -> Result<ModelRoleResolver> {
    let resolver = registry.model_role_resolver(preferences.clone())?;
    let models = resolve_allowed_models(&resolver, patterns, roles)?;
    Ok(resolver.with_available_models(models, preferences.clone())?)
}

fn saved_candidate(
    registry: &ara_cli::model_registry::ModelRegistry,
    candidates: &[String],
) -> Result<Option<(usize, HostModelRef, Option<ConfiguredThinkingLevel>)>> {
    for (index, value) in candidates.iter().enumerate() {
        let parsed = parse_saved_model(value, &|provider, id| {
            registry.find_reference(&provider.into(), &id.into()).is_ok_and(|model| model.is_some())
        });
        let Some(parsed) = parsed else { continue };
        if let Some(model) = registry.find_reference(&parsed.provider.as_str().into(), &parsed.id.as_str().into())?
            && registry.has_configured_auth(&model)
        {
            return Ok(Some((index, model, parsed.thinking_level)));
        }
    }
    Ok(None)
}

pub(super) async fn resolve(
    args: &Args,
    registry: &ara_cli::model_registry::ModelRegistry,
    journal: Option<&SessionJournal>,
    cancel: &CancellationToken,
) -> Result<StartupSelection> {
    let agent_dir = ara_home().join("agent");
    let settings = rpc_host_settings::AutoThinkingSettings::load(&agent_dir)?;
    let settings_patterns = rpc_host_settings::enabled_model_patterns(&agent_dir)?;
    let authored_scope = cli_patterns(args);
    let patterns = authored_scope.as_deref().unwrap_or(&settings_patterns);
    let usage_order = ara_cli::model_usage_order::SqliteModelUsageOrder::for_agent_dir(&agent_dir)
        .get_model_usage_order()
        .unwrap_or_else(|error| {
            eprintln!("ara: failed to read model preference: {error}");
            Vec::new()
        });
    let preferences =
        ModelMatchPreferences { usage_order, provider_order: settings.provider_order.clone(), ..Default::default() };
    let roles = |role: &str| model_role_value(settings.roles.get(role));
    let scoped = resolve_scoped_models(registry, patterns, &preferences, &roles).await?;
    let scope = to_session_scoped_models(&scoped, settings.default_thinking);
    let restoring = args.resume.is_some() || args.continue_session;
    let explicit_thinking = args.thinking.last().and_then(|value| parse_cli(value));
    let cli_model =
        args.model.clone().or_else(|| env_first(&["ARA_MODEL", "ARA_TEST_MODEL_ID"]).map(|(_, value)| value));
    let cli_provider = args.provider.clone().or_else(|| env_first(&["ARA_PROVIDER"]).map(|(_, value)| value));
    let mut selected = None;
    let mut option_thinking = None;
    let mut deferred = Vec::new();
    let mut deferred_default_role = false;
    let resolver = registry.model_role_resolver(preferences.clone())?;
    if let Some(pattern) = cli_model.as_deref().filter(|value| !value.is_empty()) {
        let resolved = resolver.resolve_cli_model(
            cli_provider.as_deref(),
            Some(pattern),
            registry.get_all()?,
            &roles,
            preferences.clone(),
        )?;
        if let Some(warning) = &resolved.warning {
            eprintln!("ara: warning: {}", sanitize_text(warning));
        }
        if resolved.configured_pattern_index.unwrap_or(0) > 0 {
            deferred.push(pattern.to_owned());
        } else if let Some(error) = resolved.error {
            if cli_provider.is_none()
                && (resolved.configured_patterns.as_ref().is_some_and(|values| !values.is_empty())
                    || !pattern.contains(':'))
            {
                deferred.push(pattern.to_owned());
            } else {
                return Err(StartupModelResolutionFailure(error).into());
            }
        } else {
            selected = resolved.model;
            option_thinking = resolved.thinking_level;
        }
    } else if !scoped.is_empty() && !restoring {
        let remembered = roles("default");
        if let Some(remembered) = remembered.as_deref().filter(|value| !value.is_empty()) {
            let scope_resolver = resolver
                .with_available_models(scoped.iter().map(|entry| entry.model.clone()).collect(), preferences.clone())?;
            let remembered_spec = scope_resolver.resolve_model_role_value(Some(remembered), &roles)?;
            selected = remembered_scoped_model(&scoped, remembered_spec.model.as_ref(), remembered);
            if selected.is_some() && remembered_spec.explicit_thinking_level {
                option_thinking = remembered_spec.thinking_level;
            }
        }
        deferred_default_role = selected.is_none()
            && remembered.is_some_and(|value| !value.is_empty())
            && authored_scope.as_ref().is_none_or(Vec::is_empty);
        if selected.is_none() && !deferred_default_role {
            selected = Some(scoped[0].model.clone());
        }
    } else if !restoring && let Some(patterns) = &authored_scope {
        deferred
            .extend(patterns.iter().map(|value| js_trim(value)).filter(|value| !value.is_empty()).map(str::to_owned));
    }
    // There is deliberately no explicit-model guard in Native's first-scope seed.
    if explicit_thinking.is_some() {
        option_thinking = explicit_thinking;
    } else if !restoring
        && !deferred_default_role
        && let Some(first) = scoped.first().filter(|entry| entry.explicit_thinking_level)
    {
        option_thinking = first.thinking_level.map(ConfiguredThinkingLevel::Concrete);
    }
    let deferred = normalized_deferred_patterns(deferred);
    let has_explicit_model = selected.is_some() || !deferred.is_empty();
    let candidates = if !has_explicit_model {
        journal.map(SessionJournal::restorable_model_candidates).transpose()?.unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut restored_suffix = None;
    if !candidates.is_empty() {
        let original = saved_candidate(registry, &candidates)?;
        let retry_limit = original.as_ref().map_or(candidates.len(), |(index, _, _)| *index);
        let discoverable = registry.get_discoverable_providers();
        let mut providers = ara_cli::model_patch::OrderedProviderSet::default();
        for value in candidates.iter().take(retry_limit) {
            if let Some(parsed) = parse_saved_model(value, &|provider, id| {
                registry.find_reference(&provider.into(), &id.into()).is_ok_and(|model| model.is_some())
            }) {
                let provider: WireString = parsed.provider.into();
                if discoverable.contains(&provider) {
                    providers.insert(provider);
                }
            }
        }
        registry
            .runtime()
            .refresh_discoverable_providers(providers, ara_cli::model_manager::ModelRefreshStrategy::OnlineIfUncached)
            .await?;
        if let Some((_, model, thinking)) = saved_candidate(registry, &candidates)? {
            selected = Some(model);
            restored_suffix = thinking;
        }
    }
    // Deferred selectors use the common CLI resolver after one cache-aware pass.
    // Actual extension registration/retry-fallback expansion remain SDK work.
    if selected.is_none() && !deferred.is_empty() {
        registry
            .runtime()
            .refresh_runtime_providers(ara_cli::model_manager::ModelRefreshStrategy::OnlineIfUncached)
            .await?;
        let resolve_deferred = || -> Result<Option<(HostModelRef, Option<ConfiguredThinkingLevel>)>> {
            let resolver = registry.model_role_resolver(preferences.clone())?;
            for pattern in &deferred {
                for selector in pattern.split(',').map(js_trim).filter(|value| !value.is_empty()) {
                    let result = resolver.resolve_cli_model(
                        None,
                        Some(selector),
                        registry.get_all()?,
                        &roles,
                        preferences.clone(),
                    )?;
                    if let Some(model) = result.model {
                        return Ok(Some((model, result.thinking_level)));
                    }
                }
            }
            Ok(None)
        };
        let mut matched = resolve_deferred()?;
        if matched.is_none() && !registry.get_discoverable_providers().is_empty() {
            registry.runtime().refresh(ara_cli::model_manager::ModelRefreshStrategy::OnlineIfUncached).await?;
            matched = resolve_deferred()?;
        }
        if let Some((model, thinking)) = matched {
            selected = Some(model);
            if option_thinking.is_none() {
                option_thinking = thinking;
            }
        } else {
            return Err(StartupModelResolutionFailure(format!(
                "Model \"{}\" not found. Run \"ara models\" to see available models.",
                deferred.join(",")
            ))
            .into());
        }
    }
    let mut allowed = allowed_resolver(registry, &settings_patterns, &roles, &preferences)?;
    let mut default_role = allowed.resolve_model_role_value(roles("default").as_deref(), &roles)?;
    if selected.is_none() && default_role.model.is_some() {
        selected = default_role.model.clone();
    }
    if selected.is_none() && !has_explicit_model {
        let mut pick =
            pick_default_available_model(allowed.available_models(), &|provider| registry.has_concrete_auth(provider));
        if (roles("default").is_some_and(|value| !value.is_empty()) || pick.is_none())
            && !registry.get_discoverable_providers().is_empty()
        {
            registry.runtime().refresh(ara_cli::model_manager::ModelRefreshStrategy::OnlineIfUncached).await?;
            allowed = allowed_resolver(registry, &settings_patterns, &roles, &preferences)?;
            default_role = allowed.resolve_model_role_value(roles("default").as_deref(), &roles)?;
            selected = default_role.model.clone();
            pick = pick_default_available_model(allowed.available_models(), &|provider| {
                registry.has_concrete_auth(provider)
            });
        }
        if selected.is_none() {
            selected = pick;
        }
    }
    if cancel.is_cancelled() {
        bail!("model startup cancelled");
    }
    let model = selected.context("No models available. Configure a provider or select --model.")?;
    let has_thinking_entry =
        journal.is_some_and(|journal| journal.branch().iter().any(|entry| entry.kind == "thinking_level_change"));
    let saved = if has_thinking_entry {
        journal.and_then(|journal| {
            journal
                .current_configured_thinking_level()
                .as_deref()
                .and_then(parse_configured)
                .or_else(|| journal.current_thinking_level().as_deref().and_then(parse_configured))
        })
    } else {
        None
    };
    let model_default = model
        .spec()
        .get("thinking")
        .and_then(|thinking| thinking.get("defaultLevel"))
        .and_then(WireValue::as_string)
        .and_then(|value| value.to_utf8().ok())
        .and_then(|value| ara_ai::ReasoningEffort::parse(&value));
    let thinking = initial_selection_with_sources(InitialSelectionInputs {
        explicit: option_thinking,
        saved,
        has_existing_session: journal.is_some_and(|journal| !journal.branch().is_empty()),
        has_thinking_entry,
        restored_suffix,
        has_explicit_model,
        default_role: default_role.thinking_level,
        default_role_explicit: default_role.explicit_thinking_level,
        model_default,
        settings: settings.default_thinking,
    });
    Ok(StartupSelection { model, state: StartupState { scope, thinking } })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cli_scope_source_keeps_empty_tokens_trim_grammar_and_last_assignment() {
        assert_eq!(cli_patterns(&Args::parse_from(["ara"])), None);
        for (flags, expected) in [
            (vec!["--models="], vec![""]),
            (vec!["--models", " a , ,b,"], vec!["a", "", "b", ""]),
            (vec!["--models", "old", "--models=new"], vec!["new"]),
            (vec!["--models=\u{FEFF}a\u{FEFF},\u{0085}b\u{0085}"], vec!["a", "\u{0085}b\u{0085}"]),
        ] {
            let args = Args::parse_from(std::iter::once("ara").chain(flags));
            assert_eq!(cli_patterns(&args), Some(expected.into_iter().map(str::to_owned).collect()));
        }
        assert_eq!(normalized_deferred_patterns(vec![" \t\u{FEFF}".into(), "".into()]), Vec::<String>::new());
        assert_eq!(
            normalized_deferred_patterns(vec![" \u{FEFF}a\u{FEFF} ".into(), " ".into(), "\u{0085}b\u{0085}".into()]),
            vec!["a", "\u{0085}b\u{0085}"]
        );
    }

    #[test]
    fn remembered_selection_preserves_scoped_donor_and_provider_identity() {
        use ara_cli::{
            model_collapse::VariantSpec,
            model_patch::{HeaderSlot, build_host_model},
            model_scope::ScopedModel,
        };
        use std::sync::Arc;

        let model = |provider: &str, routed: bool| {
            let mut spec = serde_json::json!({
                "provider":provider,"id":"catalog","name":"Catalog","api":"openai-completions",
                "baseUrl":"https://example.invalid/v1","reasoning":false,
                "contextWindow":131072,"maxTokens":8192,"input":["text"]
            });
            if routed {
                spec["compat"] = serde_json::json!({"openRouterRouting":{"only":["scope-route"]}});
            }
            build_host_model(&VariantSpec::from_json(&spec), HeaderSlot::Undefined).unwrap()
        };
        let other = model("other", false);
        let donor = model("openrouter", false);
        let routed = model("openrouter", true);
        let scopes: Vec<_> = [other.clone(), donor.clone()]
            .into_iter()
            .map(|model| ScopedModel { model, thinking_level: None, explicit_thinking_level: false })
            .collect();
        let selected = remembered_scoped_model(&scopes, Some(&routed), "openrouter/catalog@scope-route:high").unwrap();
        assert!(Arc::ptr_eq(&selected, &donor), "a resolved selector must return the scope's original donor");
        assert!(Arc::ptr_eq(&remembered_scoped_model(&scopes, None, "CATALOG").unwrap(), &other));
        assert!(remembered_scoped_model(&scopes, Some(&model("outside", false)), "catalog").is_none());
    }
}
