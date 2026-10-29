//! Model selection from fixed OMP 596f2da rpc-mode.ts:1301-1341 and
//! session/model-controls.ts:215-254,301-306,400-480. MIT; THIRD_PARTY_NOTICES.md.
//! Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc.
use super::*;
use ara_ai::{ReasoningEffort, ReasoningRequest};
use ara_cli::{
    daily_model_config::resolve_daily_role_selection,
    model_patch::HostModelRef,
    model_roles::{ModelMatchPreferences, model_role_value},
    retry_fallback::{ConfiguredThinkingLevel, RetryFallbackSelector, ThinkingLevel},
};

#[derive(Clone, Copy)]
enum SelectionMode {
    Explicit,
    CycleAvailable,
    CycleScoped(Option<ThinkingLevel>),
}

pub(super) fn stops_selection(command: &Command) -> bool {
    match command.kind.as_str() {
        "abort" => true,
        "abort_and_prompt" => command.message().is_ok(),
        "new_session" => command.frame.get("parentSession").is_none() || command.string("parentSession").is_ok(),
        "switch_session" => command.string("sessionPath").is_ok(),
        "branch" => command.string("entryId").is_ok(),
        _ => false,
    }
}

impl Host {
    pub(super) fn initialize_model_cycle_scope(&mut self) -> Result<()> {
        if let Some(startup) = &self.sessions.startup {
            self.model_cycle_scope = startup.scope.clone();
            return Ok(());
        }
        let Some(registry) = &self.sessions.provider.registry else {
            return Ok(());
        };
        let agent_dir = super::super::ara_home().join("agent");
        let patterns = super::super::rpc_host_settings::enabled_model_patterns(&agent_dir)?;
        if patterns.is_empty() {
            return Ok(());
        }
        let settings = super::super::rpc_host_settings::AutoThinkingSettings::load(&agent_dir)?;
        let usage_order = self.model_usage_order.get_model_usage_order().unwrap_or_else(|error| {
            eprintln!("ara: failed to read model preference: {error}");
            Vec::new()
        });
        let resolver = registry.model_role_resolver(ModelMatchPreferences {
            usage_order,
            provider_order: settings.provider_order,
            ..Default::default()
        })?;
        let roles = |role: &str| model_role_value(settings.roles.get(role));
        let default = match settings.default_thinking {
            Some(ConfiguredThinkingLevel::Concrete(level)) => Some(level),
            _ => None,
        };
        let scoped = ara_cli::model_scope::resolve_model_scope(&resolver, &patterns, &roles)?;
        // Native main.ts:838-851 fills the settings default only at the
        // startup scope owner, after explicit scope levels have been clamped.
        self.model_cycle_scope =
            ara_cli::model_scope::to_session_scoped_models(&scoped, default.map(ConfiguredThinkingLevel::Concrete));
        Ok(())
    }

    pub(super) async fn cycle_rpc_model(&mut self) -> Result<WireValue> {
        let admission = quota::QuotaControl::begin(&self.model_selection_control, &self.connection);
        let cancel = &admission.cancel;
        let session = self.session.clone();
        let generation = self.prompt_generation;
        if cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        let Some(registry) = self.sessions.provider.registry.clone() else {
            return Ok(WireValue::Null);
        };
        let session_id = session.header["id"].as_str();
        let scoped = !self.model_cycle_scope.is_empty();
        let mut candidates = Vec::new();
        if scoped {
            let mut provider_keys = std::collections::HashMap::new();
            // Native checks every distinct provider before the <=1 guard,
            // even for a one-entry scope. An empty filtered scope stays scoped.
            for entry in &self.model_cycle_scope {
                let provider = entry
                    .model
                    .spec()
                    .get("provider")
                    .and_then(WireValue::as_string)
                    .context("scoped model provider is missing")?;
                let eligible = if let Some(eligible) = provider_keys.get(provider) {
                    *eligible
                } else {
                    let eligible = registry
                        .get_api_key_for_provider(provider, session_id, cancel)
                        .await?
                        .is_some_and(|key| !key.is_empty());
                    provider_keys.insert(provider.clone(), eligible);
                    eligible
                };
                if eligible {
                    candidates.push((entry.model.clone(), SelectionMode::CycleScoped(entry.thinking_level)));
                }
            }
        } else {
            // cycleModel's unscoped branch deliberately reads the whole
            // available catalogue, rather than getAvailableModels' UI filter.
            candidates =
                registry.get_available()?.into_iter().map(|model| (model, SelectionMode::CycleAvailable)).collect();
        }
        if cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        if candidates.len() <= 1 {
            return Ok(WireValue::Null);
        }
        let current_id = self
            .sessions
            .provider
            .metadata
            .as_ref()
            .and_then(|metadata| metadata["catalogModelId"].as_str())
            .unwrap_or(&self.config.model.id);
        let current_index = candidates
            .iter()
            .position(|(model, _)| {
                model
                    .spec()
                    .get("provider")
                    .and_then(WireValue::as_string)
                    .is_some_and(|provider| provider.to_utf8().ok().as_deref() == Some(&self.config.model.provider))
                    && model
                        .spec()
                        .get("id")
                        .and_then(WireValue::as_string)
                        .is_some_and(|id| id.to_utf8().ok().as_deref() == Some(current_id))
            })
            .unwrap_or(0);
        let (target, mode) = candidates[(current_index + 1) % candidates.len()].clone();
        if !scoped && !registry.get_api_key(&target, session_id, cancel).await?.is_some_and(|key| !key.is_empty()) {
            let spec = target.spec();
            let provider =
                spec.get("provider").and_then(WireValue::as_string).context("model provider is missing")?.to_utf8()?;
            let id = spec.get("id").and_then(WireValue::as_string).context("model id is missing")?.to_utf8()?;
            bail!("No API key for {provider}/{id}");
        }
        // Native cycling does not refresh selected metadata and returns the
        // selected catalogue object. The shared transaction still fences the
        // original Session/generation before publishing the prepared route.
        self.install_rpc_model(&target, mode, session, generation, cancel).await?;
        let mut result = wire(json!({"isScoped":scoped}));
        result.insert("model", target.spec().to_wire_json());
        if let Some(request) = self.auto.current_request() {
            if request.disable_reasoning {
                result.insert("thinkingLevel", wire(json!("off")));
            } else if let Some(effort) = request.effort {
                result.insert("thinkingLevel", wire(json!(effort.as_str())));
            }
        }
        Ok(result)
    }

    fn available_model_selection(&self) -> Result<Vec<HostModelRef>> {
        let Some(registry) = &self.sessions.provider.registry else {
            return Ok(Vec::new());
        };
        let agent_dir = super::super::ara_home().join("agent");
        let patterns = super::super::rpc_host_settings::enabled_model_patterns(&agent_dir)?;
        if patterns.is_empty() {
            return Ok(registry.get_available()?);
        }
        let settings = super::super::rpc_host_settings::AutoThinkingSettings::load(&agent_dir)?;
        let usage_order = self.model_usage_order.get_model_usage_order().unwrap_or_else(|error| {
            eprintln!("ara: failed to read model preference: {error}");
            Vec::new()
        });
        let resolver = registry.model_role_resolver(ModelMatchPreferences {
            usage_order,
            provider_order: settings.provider_order,
            ..Default::default()
        })?;
        let roles = |role: &str| model_role_value(settings.roles.get(role));
        Ok(ara_cli::model_scope::filter_available_models_by_enabled_patterns(&resolver, &patterns, &roles)?)
    }

    pub(super) async fn rpc_available_models(&self) -> Result<Vec<WireValue>> {
        if let Some(registry) = &self.sessions.provider.registry {
            registry.runtime().await_background_refresh().await;
        }
        Ok(self.available_model_selection()?.iter().map(|model| model.spec().to_wire_json()).collect())
    }

    pub(super) async fn set_rpc_model(&mut self, provider_id: &str, model_id: &str) -> Result<WireValue> {
        let admission = quota::QuotaControl::begin(&self.model_selection_control, &self.connection);
        let cancel = &admission.cancel;
        if cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        let provider_key = ara_rpc::WireString::from(provider_id);
        let model_key = ara_rpc::WireString::from(model_id);
        let lookup = |models: Vec<HostModelRef>| {
            models.into_iter().find(|model| {
                model.spec().get("provider").and_then(WireValue::as_string) == Some(&provider_key)
                    && model.spec().get("id").and_then(WireValue::as_string) == Some(&model_key)
            })
        };
        let registry = self.sessions.provider.registry.clone();
        let mut found = lookup(self.available_model_selection()?);
        if found.is_none()
            && let Some(registry) = &registry
        {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ara_cli::handoff::cancelled()),
                _ = registry.runtime().await_background_refresh() => {},
            }
            found = lookup(self.available_model_selection()?);
        }
        let original = found.with_context(|| format!("Model not found: {provider_id}/{model_id}"))?;
        let registry = registry.context("model catalog is unavailable")?;
        if !registry.has_configured_auth(&original) {
            bail!("No API key for {provider_id}/{model_id}");
        }
        let session = self.session.clone();
        let generation = self.prompt_generation;
        let target = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ara_cli::handoff::cancelled()),
            target = registry.refresh_selected_model_metadata(&original) => target?,
        };
        self.install_rpc_model(&target, SelectionMode::Explicit, session, generation, cancel).await?;
        Ok(original.spec().to_wire_json())
    }

    async fn install_rpc_model(
        &mut self,
        target: &HostModelRef,
        mode: SelectionMode,
        session: Arc<Session>,
        generation: u64,
        cancel: &CancellationToken,
    ) -> Result<()> {
        if cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        let registry = self.sessions.provider.registry.clone().context("model catalog is unavailable")?;
        let provider = target
            .spec()
            .get("provider")
            .and_then(WireValue::as_string)
            .context("selected model provider is missing")?
            .to_utf8()?;
        let id = target
            .spec()
            .get("id")
            .and_then(WireValue::as_string)
            .context("selected model id is missing")?
            .to_utf8()?;
        let selector = RetryFallbackSelector {
            raw: format!("{provider}/{id}"),
            provider: provider.clone(),
            id: id.clone(),
            thinking_level: None,
        };
        let config = registry.config();
        let mut selection = resolve_daily_role_selection(
            config.as_ref(),
            &selector,
            &ara_cli::schema_policy::process_daily_environment,
            target,
        )?;
        if selection.api == ara_cli::daily_model_config::DailyApi::OpenAiCodexResponses
            && (self.sessions.explicit_generation.max_tokens.is_some()
                || self.sessions.explicit_generation.temperature.is_some())
        {
            bail!("codex generation: output caps and sampling are not supported by Codex");
        }
        let current_effective = if let Some(request) = self.auto.current_request() {
            if request.disable_reasoning {
                Some("off".to_owned())
            } else {
                request.effort.map(|effort| effort.as_str().to_owned())
            }
        } else {
            let journal = session.journal.lock().await;
            journal
                .branch()
                .into_iter()
                .rev()
                .find(|entry| entry.kind == "thinking_level_change")
                .and_then(|entry| entry.raw["thinkingLevel"].as_str().map(str::to_owned))
        };
        let current = if self.auto.enabled() {
            Some(ConfiguredThinkingLevel::Auto)
        } else {
            current_effective.as_deref().and_then(ara_cli::session_thinking::parse_configured)
        };
        let next = if self.auto.enabled() {
            Some(ConfiguredThinkingLevel::Auto)
        } else if let SelectionMode::CycleScoped(level) = mode {
            level.map(ConfiguredThinkingLevel::Concrete)
        } else if matches!(mode, SelectionMode::CycleAvailable) {
            current
        } else {
            selection.metadata["thinking"]["defaultLevel"]
                .as_str()
                .and_then(ara_cli::session_thinking::parse_configured)
                .or(current)
        };
        let request = prepare_selection_thinking(&mut selection.protocol, next, self.thinking_ceiling())?;
        let effective =
            if request.disable_reasoning { Some("off") } else { request.effort.map(ReasoningEffort::as_str) };
        let next_auto = next == Some(ConfiguredThinkingLevel::Auto);
        let options = selection.generation;
        let session_id = session.header["id"].as_str().map(str::to_owned);
        let prepared = self
            .sessions
            .provider
            .prepare_model_selection(selection, session_id.clone(), &self.sessions.cwd, cancel)
            .await?;
        let current_catalog_id = self
            .sessions
            .provider
            .metadata
            .as_ref()
            .and_then(|metadata| metadata["catalogModelId"].as_str())
            .unwrap_or(&self.config.model.id);
        let changed_model = self.config.model.provider != prepared.route.model().provider || current_catalog_id != id;
        let mut config = self.base_config();
        config.model = prepared.route.model().clone();
        config.provider = prepared.build();
        config.max_tokens = self.sessions.explicit_generation.max_tokens.or(options.max_tokens);
        config.temperature = self.sessions.explicit_generation.temperature.or(options.temperature);
        let overlay = self.overlay(&self.host_tools, &self.active_host_names()).await?;
        let retained_owner = session.tool_owner.lock().unwrap().clone();
        let candidate = self
            .sessions
            .config(
                &config,
                prepared.metadata.as_ref(),
                Some(&self.skills),
                false,
                session.values.clone(),
                session.native_session.clone(),
                overlay,
                Some(retained_owner),
            )
            .await?;
        let super::PreparedSessionConfig {
            mut config, skills, tool_owner, context_controller, native_publication, ..
        } = candidate;
        let mut journal = session.journal.lock().await;
        if cancel.is_cancelled()
            || self.connection.is_cancelled()
            || self.model_selection_control.lock().unwrap().stop_requested()
            || generation != self.prompt_generation
            || !Arc::ptr_eq(&session, &self.session)
        {
            return Err(ara_cli::handoff::cancelled());
        }
        let changed_thinking = self.auto.enabled() != next_auto || current_effective.as_deref() != effective;
        let model_selection = format!("{provider}/{id}");
        let thinking = changed_thinking.then_some((effective, if next_auto { Some("auto") } else { effective }));
        registry.clear_suppressed_selector(&ara_cli::model_roles::format_model_string_with_routing(target))?;
        if let Err(error) = journal.append_model_and_optional_thinking_change(
            &model_selection,
            matches!(mode, SelectionMode::Explicit).then_some("default"),
            false,
            thinking,
        ) {
            *session.persistence_error.lock().unwrap() = Some(format!("Model selection publication failed: {error}"));
            self.connection.cancel();
            return Err(error.into());
        }
        self.publish_native_after_journal(native_publication.as_ref(), &session)?;
        *session.tool_owner.lock().unwrap() = tool_owner.clone();
        self.clear_explicit_model_fallback();
        let previous_route = self.sessions.provider.route.generation();
        self.sessions.provider.adopt_daily_replacement(prepared, session_id.clone(), session_id);
        self.note_explicit_model_adoption(previous_route);
        config.provider = self.sessions.provider.build();
        self.sessions.tool_owner = tool_owner;
        self.sessions.context_controller = context_controller;
        self.config = config;
        self.skills = skills;
        // New calls bind after lifecycle admission; already started calls keep
        // their original epoch and request owner.
        self.adopt_model_thinking(next_auto, request, false);
        self.rebind_pending_model_selection();
        if changed_model {
            self.output.frame(json!({"type":"model_changed"}));
        }
        if changed_thinking {
            let mut event = json!({"type":"thinking_level_changed","thinkingLevel":effective});
            if next_auto {
                event["configured"] = json!("auto");
            }
            self.output.frame(event);
        }
        drop(journal);
        self.record_model_preference(&model_selection);
        Ok(())
    }
}

fn prepare_selection_thinking(
    protocol: &mut ara_cli::model_route::ProtocolOptions,
    configured: Option<ConfiguredThinkingLevel>,
    ceiling: Option<ReasoningEffort>,
) -> Result<ReasoningRequest> {
    use ara_cli::model_route::ProtocolOptions;
    let (policy, base) = match protocol {
        ProtocolOptions::Completions(options) => (options.reasoning_policy.as_ref(), &mut options.reasoning),
        ProtocolOptions::Responses(options) => {
            (options.request.reasoning_policy.as_ref(), &mut options.request.reasoning)
        }
        ProtocolOptions::CodexResponses(options) => {
            (options.request.reasoning_policy.as_ref(), &mut options.request.reasoning)
        }
        ProtocolOptions::Anthropic(_) => {
            bail!("explicit model thinking requires a supported route")
        }
    };
    let request = configured
        .map(|configured| ara_cli::session_thinking::resolve_selection(policy, configured, ceiling))
        .unwrap_or_default();
    if let (Some(ceiling), Some(policy)) = (ceiling, policy) {
        if !policy.model_supports_effort_ceiling(ceiling) {
            bail!("model has no effort within the Session ceiling");
        }
        let normalized =
            policy.normalize_request(Some(&request)).map_err(|error| anyhow::anyhow!(error.to_string()))?;
        if normalized.effort.is_some_and(|effort| effort > ceiling) {
            bail!("model reasoning default exceeds the Session ceiling");
        }
    }
    *base = Some(request.clone());
    Ok(request)
}
