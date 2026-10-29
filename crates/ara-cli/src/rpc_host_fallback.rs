//! Fixed OMP 596f2da turn-recovery.ts:1509-1563,1761-1908.
//! The serial Host owns adoption; Registry, authentication and provider state
//! retain their existing owners. Candidate preparation does not admit a route.
//! MIT; see THIRD_PARTY_NOTICES.md.
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
use super::*;
use ara_cli::retry_fallback::{
    ActiveRetryFallbackState, ConfiguredThinkingLevel, CurrentModel, RetryFallbackCandidateOptions,
    RetryFallbackModelLookup, RetryFallbackResolutionContext, ServingModel, ThinkingLevel, concrete_thinking_level,
    find_retry_fallback_candidates, format_retry_fallback_selector, parse_thinking_level,
    resolve_retry_fallback_chain_key,
};

/// Each waiting hook owns a cancellation guard. Dropping its future revokes
/// the serial Host operation even if the Run itself has not yet settled.
pub(super) struct AdmissionBinding {
    pub tx: mpsc::UnboundedSender<AdmissionRequest>,
    pub session: Arc<Session>,
    pub generation: u64,
    pub route_generation: Arc<std::sync::atomic::AtomicU64>,
}

pub(super) struct AdmissionRequest {
    pub(super) session: Arc<Session>,
    pub(super) generation: u64,
    pub(super) route_generation: u64,
    pub(super) model_call: bool,
    pub(super) snapshot: ExecutionSnapshot,
    pub(super) messages: Vec<AgentMessage>,
    pub(super) cancel: CancellationToken,
    pub(super) reply: tokio::sync::oneshot::Sender<Result<ExecutionSnapshot, ara_agent::AdmissionError>>,
}

impl AdmissionBinding {
    pub async fn request(
        &self,
        model_call: bool,
        snapshot: ExecutionSnapshot,
        messages: Vec<AgentMessage>,
        parent: &CancellationToken,
    ) -> Result<ExecutionSnapshot, ara_agent::AdmissionError> {
        let cancel = parent.child_token();
        let _guard = cancel.clone().drop_guard();
        let (reply, response) = tokio::sync::oneshot::channel();
        if cancel.is_cancelled() {
            return Err(ara_agent::AdmissionError::Cancelled);
        }
        self.tx
            .send(AdmissionRequest {
                session: self.session.clone(),
                generation: self.generation,
                route_generation: self.route_generation.load(std::sync::atomic::Ordering::Acquire),
                model_call,
                snapshot,
                messages,
                cancel: cancel.clone(),
                reply,
            })
            .map_err(|_| ara_agent::AdmissionError::Cancelled)?;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ara_agent::AdmissionError::Cancelled),
            result = response => result.unwrap_or(Err(ara_agent::AdmissionError::Cancelled)),
        }
    }
}

pub(super) struct AdmissionReady {
    session: Arc<Session>,
    generation: u64,
    route_generation: u64,
    snapshot: ExecutionSnapshot,
}

#[derive(Default)]
pub(super) struct FallbackState {
    pub active: Option<ActiveRetryFallbackState>,
    routed_for: Option<String>,
    last_served: Option<(String, ServingModel)>,
    ready: Option<AdmissionReady>,
    explicit_admission: Option<(Arc<Session>, u64, u64, u64)>,
}

impl FallbackState {
    pub(super) fn clear_ready(&mut self) {
        self.ready = None;
    }
    pub(super) fn take_ready(&mut self) -> Option<AdmissionReady> {
        self.ready.take()
    }
    pub fn serving(&self, session: &Value, bootstrap: String) -> ServingModel {
        let id = session["id"].as_str().unwrap_or("");
        self.last_served
            .as_ref()
            .filter(|(owner, _)| owner == id)
            .map(|(_, served)| served.clone())
            .unwrap_or(ServingModel { selector: bootstrap, is_fallback: self.routed_for.as_deref() == Some(id) })
    }
}

struct RegistryLookup<'a>(&'a ara_cli::model_registry::ModelRegistry);

impl RetryFallbackModelLookup for RegistryLookup<'_> {
    fn contains_model(&self, provider: &str, id: &str) -> bool {
        self.0.find_reference(&provider.into(), &id.into()).is_ok_and(|model| model.is_some())
    }
    fn has_provider(&self, provider: &str) -> bool {
        self.0.models_for_provider(&provider.into()).is_ok_and(|models| !models.is_empty())
    }
}

fn current_model(model: &ara_ai::Model, metadata: Option<&Value>) -> CurrentModel {
    let route = metadata.and_then(|metadata| metadata.get("routing")?.get("only")?.as_array());
    let single_upstream_route = if matches!(model.provider.as_str(), "openrouter" | "vercel-ai-gateway") {
        route
            .filter(|route| route.len() == 1)
            .and_then(|route| route[0].as_str())
            .filter(|route| !route.is_empty())
            .map(str::to_owned)
    } else {
        None
    };
    let id = metadata.and_then(|value| value["catalogModelId"].as_str()).unwrap_or(&model.id).to_owned();
    CurrentModel { provider: model.provider.clone(), id, single_upstream_route }
}

fn configured_thinking(value: Option<&str>) -> Option<ConfiguredThinkingLevel> {
    match value {
        Some("auto") => Some(ConfiguredThinkingLevel::Auto),
        value => value.and_then(parse_thinking_level).map(ConfiguredThinkingLevel::Concrete),
    }
}

fn snapshots_match(left: &ExecutionSnapshot, right: &ExecutionSnapshot) -> bool {
    match (&left.model, &right.model) {
        (Some(left), Some(right)) => left.model == right.model && Arc::ptr_eq(&left.provider, &right.provider),
        (None, None) => true,
        _ => false,
    }
}

// A recorded null is the live model-controls undefined selection. Preserve it
// for selector/changed comparisons instead of the journal context's Off
// projection. An untouched branch retains the existing startup default.
fn fallback_effective_thinking(journal: &SessionJournal) -> Option<ThinkingLevel> {
    match journal.branch().into_iter().rev().find(|entry| entry.kind == "thinking_level_change") {
        Some(entry) => entry.raw["thinkingLevel"].as_str().and_then(parse_thinking_level),
        None => journal.current_thinking_level().as_deref().and_then(parse_thinking_level),
    }
}

struct PreparedThinking {
    effective: Option<ThinkingLevel>,
    request: ara_ai::ReasoningRequest,
}

/// Fixed model-controls.ts resolves configured Host selectors before the
/// provider normalizes mandatory/default wire behavior. Ordinary Off keeps
/// its disable flag; it must not become Codex's separate force-off input.
fn install_fixed_thinking(
    protocol: &mut ara_cli::model_route::ProtocolOptions,
    configured: Option<ConfiguredThinkingLevel>,
    ceiling: Option<ara_ai::ReasoningEffort>,
) -> Result<PreparedThinking> {
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
            bail!("fallback thinking requires a supported model route")
        }
    };
    let policy = policy.context("fallback route has no materialized thinking policy")?;
    let effective = match configured {
        Some(ConfiguredThinkingLevel::Auto) => {
            ara_cli::auto_thinking::provisional_effort(policy.supported_efforts(), policy.default_effort())
                .and_then(|level| parse_thinking_level(level.as_str()))
        }
        _ => match concrete_thinking_level(configured) {
            None | Some(ThinkingLevel::Inherit) => None,
            Some(ThinkingLevel::Off) => Some(ThinkingLevel::Off),
            Some(level) => ara_ai::ReasoningEffort::parse(level.as_str())
                .and_then(|requested| policy.clamp_effort(requested))
                .and_then(|level| parse_thinking_level(level.as_str())),
        },
    };
    let effort = ara_cli::auto_thinking::apply_session_ceiling(
        policy.supported_efforts(),
        effective.and_then(|level| ara_ai::ReasoningEffort::parse(level.as_str())),
        ceiling,
    );
    let request = ara_ai::ReasoningRequest {
        effort,
        disable_reasoning: effective == Some(ThinkingLevel::Off),
        ..Default::default()
    };
    if let Some(ceiling) = ceiling {
        if !policy.model_supports_effort_ceiling(ceiling) {
            bail!("fallback model has no effort within the Session ceiling");
        }
        let wire = policy.normalize_request(Some(&request)).map_err(|error| anyhow::anyhow!(error.to_string()))?;
        if wire.effort.is_some_and(|effort| effort > ceiling) {
            bail!("fallback model reasoning default exceeds the Session ceiling");
        }
    }
    let effective = if request.disable_reasoning {
        Some(ThinkingLevel::Off)
    } else {
        effort.and_then(|effort| parse_thinking_level(effort.as_str()))
    };
    *base = Some(request.clone());
    Ok(PreparedThinking { effective, request })
}

impl Host {
    pub(super) fn fallback_restore_pending(&self) -> bool {
        self.fallback.active.as_ref().is_some_and(|active| !active.pinned)
            && self.retry_policy.fallback_revert_policy()
                == ara_cli::retry_fallback::RetryFallbackRevertPolicy::CooldownExpiry
            && self.sessions.provider.registry.is_some()
    }

    pub(super) async fn restore_fallback_primary(&mut self, cancel: &CancellationToken) -> Result<bool> {
        use ara_cli::daily_model_config::{load_daily_config, resolve_daily_fallback_selection};
        use ara_cli::retry_fallback::parse_retry_fallback_selector;
        let Some(active) = self.fallback.active.clone() else {
            return Ok(false);
        };
        if !self.fallback_restore_pending() {
            return Ok(false);
        }
        let Some(registry) = self.sessions.provider.registry.clone() else {
            return Ok(false);
        };
        let selector = {
            let lookup = RegistryLookup(&registry);
            parse_retry_fallback_selector(&active.original_selector, Some(&lookup))
        };
        let Some(selector) = selector else {
            // Keep fallback attribution and last-served identity when only the
            // invalid lineage is discarded (fixed turn-recovery.ts:2021).
            self.fallback.active = None;
            return Ok(false);
        };
        let suppressed = registry.is_selector_suppressed(&selector.raw.as_str().into())?;
        if self.serving_selector().await == selector.raw {
            if !suppressed {
                self.fallback.active = None;
                self.fallback.routed_for = None;
                self.fallback.clear_ready();
            }
            return Ok(false);
        }
        if suppressed {
            return Ok(false);
        }
        let session = self.session.clone();
        let generation = self.prompt_generation;
        let configured =
            configured_thinking(session.journal.lock().await.current_configured_thinking_level().as_deref());
        let next_thinking = if configured == active.last_applied_fallback_thinking_level {
            active.original_thinking_level
        } else {
            configured
        };
        let path = self
            .sessions
            .args
            .models_config
            .clone()
            .unwrap_or_else(|| super::super::ara_home().join("agent/models.yml"));
        let models = load_daily_config(&path, self.sessions.args.models_config.is_some())?;
        let mut selection = match resolve_daily_fallback_selection(
            models.as_ref(),
            &selector,
            &ara_cli::schema_policy::process_daily_environment,
            Some(&registry),
        ) {
            Ok(selection) => selection,
            Err(_) => return Ok(false),
        };
        let prepared = match install_fixed_thinking(&mut selection.protocol, next_thinking, self.thinking_ceiling()) {
            Ok(prepared) => prepared,
            Err(_) => return Ok(false),
        };
        let effective = prepared.effective;
        let allow_keyless = registry.is_keyless_provider(&selection.model.provider.as_str().into());
        let to = format_retry_fallback_selector(&current_model(&selection.model, Some(&selection.metadata)), None);
        let options = selection.generation;
        let id = session.header["id"].as_str().map(str::to_owned);
        let provider = match self
            .sessions
            .provider
            .prepare_daily_replacement(selection, id.clone(), id.clone(), &self.sessions.cwd, cancel)
            .await
        {
            Ok(provider) => provider,
            Err(_) => return Ok(false),
        };
        if provider.route.check_fallback_auth(cancel, allow_keyless).await.is_err() {
            return Ok(false);
        }
        let mut config = self.base_config();
        config.model = provider.route.model().clone();
        config.provider = provider.build();
        config.max_tokens = self.sessions.explicit_generation.max_tokens.or(options.max_tokens);
        config.temperature = self.sessions.explicit_generation.temperature.or(options.temperature);
        let overlay = self.overlay(&self.host_tools, &self.active_host_names()).await?;
        let retained_owner = session.tool_owner.lock().unwrap().clone();
        let candidate = self
            .sessions
            .config(
                &config,
                provider.metadata.as_ref(),
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
        if !self.fallback_admission_valid(&session, generation, cancel) {
            return Err(ara_cli::handoff::cancelled());
        }
        let thinking = effective.map(ThinkingLevel::as_str);
        let was_auto = journal.current_configured_thinking_level().as_deref() == Some("auto");
        let next_auto = next_thinking == Some(ConfiguredThinkingLevel::Auto);
        let changed = was_auto != next_auto || fallback_effective_thinking(&journal) != effective;
        let change = changed.then_some((thinking, if next_auto { Some("auto") } else { thinking }));
        // Restoration intentionally has no context-fit rejection: its caller
        // must maintain the restored model's context before any dispatch.
        if let Err(error) = journal.append_model_and_optional_thinking_change(
            &to,
            Some(ara_session::EPHEMERAL_MODEL_CHANGE_ROLE),
            false,
            change,
        ) {
            *session.persistence_error.lock().unwrap() =
                Some(format!("Primary restoration model/thinking publication failed: {error}"));
            self.connection.cancel();
            return Err(error.into());
        }
        self.publish_native_after_journal(native_publication.as_ref(), &session)?;
        *session.tool_owner.lock().unwrap() = tool_owner.clone();
        self.fallback.active = None;
        self.fallback.routed_for = None;
        self.fallback.clear_ready();
        self.sessions.provider.adopt_daily_replacement(provider, id.clone(), id);
        config.provider = self.sessions.provider.build();
        self.sessions.tool_owner = tool_owner;
        self.sessions.context_controller = context_controller;
        self.config = config;
        self.skills = skills;
        self.adopt_model_thinking(next_auto, prepared.request, changed);
        drop(journal);
        self.record_model_preference(&to);
        Ok(true)
    }

    pub(super) async fn initial_usage_preflight(&mut self, messages: &[AgentMessage]) -> Result<()> {
        if !self.retry_policy.usage_aware_fallback() {
            return Ok(());
        }
        let guard = quota::QuotaControl::begin(&self.quota_control, &self.connection);
        let session = self.session.clone();
        let generation = self.prompt_generation;
        tokio::select! {
            biased;
            _ = guard.cancel.cancelled() => return Err(ara_cli::handoff::cancelled()),
            result = self.usage_preflight(messages, &session, generation, &guard.cancel) => result?,
        }
        if !self.fallback_admission_valid(&session, generation, &guard.cancel) {
            return Err(ara_cli::handoff::cancelled());
        }
        if self.retry_policy.usage_aware_fallback() {
            self.mark_usage_ready();
        }
        Ok(())
    }

    fn fallback_admission_valid(&self, session: &Arc<Session>, generation: u64, cancel: &CancellationToken) -> bool {
        !cancel.is_cancelled()
            && !self.connection.is_cancelled()
            && !self.quota_control.lock().unwrap().stop_requested()
            && Arc::ptr_eq(session, &self.session)
            && generation == self.prompt_generation
            && session.persistence_error.lock().unwrap().is_none()
    }

    pub(super) fn hook_admission_valid(
        &self,
        session: &Arc<Session>,
        generation: u64,
        cancel: &CancellationToken,
    ) -> bool {
        if self.retry_policy.usage_aware_fallback() {
            return self.fallback_admission_valid(session, generation, cancel);
        }
        !cancel.is_cancelled()
            && !self.connection.is_cancelled()
            && !self.quota_control.lock().unwrap().queued_stop_requested()
            && Arc::ptr_eq(session, &self.session)
            && generation == self.prompt_generation
            && session.persistence_error.lock().unwrap().is_none()
    }

    fn admission_ready_matches(&self, ready: &AdmissionReady) -> bool {
        Arc::ptr_eq(&ready.session, &self.session)
            && ready.generation == self.prompt_generation
            && ready.route_generation == self.sessions.provider.route.generation()
            && snapshots_match(&ready.snapshot, &self.snapshot.read().unwrap_or_else(|e| e.into_inner()))
    }

    pub(super) fn restore_usage_ready(&mut self, ready: Option<AdmissionReady>) {
        self.fallback.ready = ready.filter(|ready| self.admission_ready_matches(ready));
    }

    fn mark_usage_ready(&mut self) {
        self.fallback.ready = Some(AdmissionReady {
            session: self.session.clone(),
            generation: self.prompt_generation,
            route_generation: self.sessions.provider.route.generation(),
            snapshot: self.snapshot.read().unwrap_or_else(|e| e.into_inner()).clone(),
        });
    }

    pub(super) async fn handle_usage_admission(&mut self, request: AdmissionRequest) {
        use ara_agent::AdmissionError;
        let current_route = self.sessions.provider.route.generation();
        let explicit_rebase =
            self.fallback.explicit_admission.as_ref().is_some_and(|(session, generation, first, last)| {
                Arc::ptr_eq(session, &request.session)
                    && *generation == request.generation
                    && *last == current_route
                    && request.route_generation.wrapping_sub(*first) <= last.wrapping_sub(*first)
            });
        let valid = self.hook_admission_valid(&request.session, request.generation, &request.cancel)
            && !request.reply.is_closed()
            && self.active.as_ref().is_some_and(|run| Arc::ptr_eq(&run.sink.session, &request.session))
            && (explicit_rebase
                || (request.route_generation == current_route
                    && snapshots_match(&request.snapshot, &self.snapshot.read().unwrap_or_else(|e| e.into_inner()))));
        if !valid {
            self.fallback.clear_ready();
            let _ = request.reply.send(Err(AdmissionError::Cancelled));
            return;
        }
        let ready = self.fallback.ready.as_ref().is_some_and(|ready| self.admission_ready_matches(ready));
        let result = if !self.retry_policy.usage_aware_fallback() {
            Ok(())
        } else if ready {
            if request.model_call {
                self.fallback.clear_ready();
            }
            Ok(())
        } else {
            self.fallback.clear_ready();
            // One serial operation, one QuotaControl slot. The Run-owned child
            // token also revokes this operation when the hook future is dropped.
            let guard = quota::QuotaControl::begin(&self.quota_control, &self.connection);
            tokio::select! {
                biased;
                _ = request.cancel.cancelled() => Err(AdmissionError::Cancelled),
                _ = guard.cancel.cancelled() => Err(AdmissionError::Cancelled),
                result = self.usage_preflight(&request.messages, &request.session, request.generation, &request.cancel) => {
                    result.map_err(|error| {
                        if request.cancel.is_cancelled() || guard.cancel.is_cancelled() { AdmissionError::Cancelled }
                        else { AdmissionError::Blocked(error.to_string()) }
                    })
                }
            }
        };
        // No await between the final ownership check and reply publication.
        let result = if !self.hook_admission_valid(&request.session, request.generation, &request.cancel)
            || request.reply.is_closed()
        {
            Err(AdmissionError::Cancelled)
        } else {
            result.map(|()| {
                if !request.model_call {
                    self.mark_usage_ready();
                }
                self.snapshot.read().unwrap_or_else(|e| e.into_inner()).clone()
            })
        };
        if result.is_err() {
            self.fallback.clear_ready();
            self.drain_queues = false;
        }
        let _ = request.reply.send(result);
    }

    async fn usage_health(
        &self,
        model: &ara_ai::Model,
        catalog_id: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<ara_cli::auth_storage::ModelUsageHealth>> {
        let Some(account) = &self.sessions.provider.shared_account else {
            return Ok(None);
        };
        let options = ara_cli::auth_storage::ModelUsageHealthOptions {
            model_id: Some(catalog_id.to_owned()),
            session_id: self.session.header["id"].as_str().map(str::to_owned),
            base_url: Some(model.base_url.clone()),
            reserve_fraction: self.retry_policy.usage_reserve_pct()? / 100.0,
        };
        match account.get_model_usage_health(&model.provider, &options, cancel).await {
            Ok(health) => Ok(Some(health)),
            Err(_) if cancel.is_cancelled() => Err(ara_cli::handoff::cancelled()),
            // Native usage query faults fail open. Credential validation is a
            // separate requirement and still runs for each fallback candidate.
            Err(_) => Ok(None),
        }
    }

    fn release_unhealthy_sticky(
        &self,
        model: &ara_ai::Model,
        health: &ara_cli::auth_storage::ModelUsageHealth,
    ) -> Result<()> {
        use ara_cli::auth_storage::ModelUsageHealthState::Healthy;
        if health.state == Healthy
            && health.accounts.iter().any(|account| account.selected == Some(true) && account.state != Healthy)
            && health.accounts.iter().any(|account| account.state == Healthy)
            && let Some(account) = &self.sessions.provider.shared_account
            && let Some(id) = self.session.header["id"].as_str()
        {
            account.release_session_credential_for_reselection(&model.provider, id)?;
        }
        Ok(())
    }

    async fn usage_candidate_available(
        &self,
        model: &ara_ai::Model,
        catalog_id: &str,
        cancel: &CancellationToken,
    ) -> Result<bool> {
        use ara_cli::auth_storage::ModelUsageHealthState::{Depleted, Reserve};
        let Some(health) = self.usage_health(model, catalog_id, cancel).await? else {
            return Ok(true);
        };
        if cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        self.release_unhealthy_sticky(model, &health)?;
        Ok(!matches!(health.state, Depleted | Reserve))
    }

    async fn usage_preflight(
        &mut self,
        messages: &[AgentMessage],
        session: &Arc<Session>,
        generation: u64,
        cancel: &CancellationToken,
    ) -> Result<()> {
        use ara_cli::auth_storage::ModelUsageHealthState::{Healthy, Unknown};
        if !self.fallback_admission_valid(session, generation, cancel) {
            return Err(ara_cli::handoff::cancelled());
        }
        if !self.retry_policy.usage_aware_fallback() {
            return Ok(());
        }
        let model = current_model(&self.config.model, self.sessions.provider.metadata.as_ref());
        let health = self.usage_health(&self.config.model, &model.id, cancel).await?;
        if !self.fallback_admission_valid(session, generation, cancel) {
            return Err(ara_cli::handoff::cancelled());
        }
        let Some(health) = health else { return Ok(()) };
        if matches!(health.state, Healthy | Unknown) {
            self.release_unhealthy_sticky(&self.config.model, &health)?;
            return Ok(());
        }
        let (configured, effective, role) = {
            let journal = session.journal.lock().await;
            let role = journal
                .branch()
                .into_iter()
                .rev()
                .find(|entry| entry.kind == "model_change")
                .and_then(|entry| entry.raw["role"].as_str())
                .unwrap_or("default")
                .to_owned();
            (
                configured_thinking(journal.current_configured_thinking_level().as_deref()),
                fallback_effective_thinking(&journal),
                role,
            )
        };
        let from = format_retry_fallback_selector(&model, effective);
        if self.retry_policy.reserve_policy() == "fail-closed" {
            let condition = if health.state == ara_cli::auth_storage::ModelUsageHealthState::Reserve {
                "reserve reached"
            } else {
                "usage depleted"
            };
            bail!("Usage preflight blocked: {condition} for {from}; reserve policy is fail-closed.");
        }
        if !self.retry_policy.model_fallback() {
            return Ok(());
        }
        let Some(registry) = self.sessions.provider.registry.clone() else {
            return Ok(());
        };
        let candidates = self.fallback_candidates(&registry, &model, &from, &role)?;
        // This RPC host has no reserve confirmer. Fixed Native automatically
        // adopts a valid candidate for any non-fail-closed policy in that case.
        self.apply_fallback_candidates(
            candidates,
            messages,
            configured,
            from,
            session.clone(),
            generation,
            cancel,
            true,
        )
        .await?;
        if !self.fallback_admission_valid(session, generation, cancel) {
            return Err(ara_cli::handoff::cancelled());
        }
        Ok(())
    }

    fn fallback_candidates(
        &self,
        registry: &ara_cli::model_registry::ModelRegistry,
        model: &CurrentModel,
        from: &str,
        role_hint: &str,
    ) -> Result<Vec<(String, ara_cli::retry_fallback::RetryFallbackSelector)>> {
        {
            let chains = self.retry_policy.fallback_chains(&self.retry_policy.model_role_names());
            let lookup = RegistryLookup(registry);
            let role = |role: &str| self.retry_policy.model_role(role);
            let context =
                RetryFallbackResolutionContext { chains: &chains, get_model_role: &role, model_lookup: &lookup };
            let hint = (role_hint != ara_session::EPHEMERAL_MODEL_CHANGE_ROLE)
                .then(|| self.retry_policy.model_role(role_hint))
                .flatten()
                .and_then(|selector| ara_cli::retry_fallback::parse_retry_fallback_selector(&selector, Some(&lookup)))
                .filter(|selector| selector.provider == model.provider && selector.id == model.id)
                .map(|_| role_hint);
            let resolved_key = resolve_retry_fallback_chain_key(&context, from, Some(model), hint);
            let mut keys = self.fallback.active.as_ref().map(|state| vec![state.role.clone()]).unwrap_or_default();
            if let Some(key) = resolved_key
                && !keys.contains(&key)
            {
                keys.push(key);
            }
            let mut candidates = Vec::new();
            for key in keys {
                for candidate in find_retry_fallback_candidates(
                    &context,
                    &key,
                    from,
                    Some(model),
                    RetryFallbackCandidateOptions::default(),
                )? {
                    candidates.push((key.clone(), candidate));
                }
            }
            Ok(candidates)
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_fallback_candidates(
        &mut self,
        candidates: Vec<(String, ara_cli::retry_fallback::RetryFallbackSelector)>,
        messages: &[AgentMessage],
        configured: Option<ConfiguredThinkingLevel>,
        from: String,
        session: Arc<Session>,
        generation: u64,
        cancel: &CancellationToken,
        reserve: bool,
    ) -> Result<bool> {
        use ara_cli::daily_model_config::{load_daily_config, resolve_daily_fallback_selection};
        let Some(registry) = self.sessions.provider.registry.clone() else {
            return Ok(false);
        };
        let agent_dir = super::super::ara_home().join("agent");
        let path = self.sessions.args.models_config.clone().unwrap_or_else(|| agent_dir.join("models.yml"));
        let models = load_daily_config(&path, self.sessions.args.models_config.is_some())?;
        for (role, selector) in candidates {
            if !self.fallback_admission_valid(&session, generation, cancel) {
                return Ok(false);
            }
            if registry.is_selector_suppressed(&selector.raw.as_str().into())? {
                continue;
            }
            let mut selection = match resolve_daily_fallback_selection(
                models.as_ref(),
                &selector,
                &ara_cli::schema_policy::process_daily_environment,
                Some(&registry),
            ) {
                Ok(selection) => selection,
                Err(_) => continue,
            };
            let next_thinking = selector.thinking_level.map(ConfiguredThinkingLevel::Concrete).or(configured);
            let prepared = match install_fixed_thinking(&mut selection.protocol, next_thinking, self.thinking_ceiling())
            {
                Ok(prepared) => prepared,
                Err(_) => continue,
            };
            let effective_thinking = prepared.effective;
            let to = format_retry_fallback_selector(&current_model(&selection.model, Some(&selection.metadata)), None);
            let allow_keyless = models
                .as_ref()
                .and_then(|config| config.value().get("providers")?.get(&selector.provider)?.get("auth"))
                .and_then(Value::as_str)
                == Some("none");
            if reserve {
                let journal = session.journal.lock().await;
                session.projector.refresh_checked(&journal, messages, &self.config.model)?;
                let non_message = self.non_message_tokens();
                let tokens = ara_cli::context_budget::context_tokens_for_current(
                    &self.config.model,
                    &journal,
                    &session.projector,
                    messages,
                    &[],
                    non_message,
                )?;
                if !maintenance::retry_tokens_fit(
                    selection.model.context_window,
                    self.compaction_policy.recovery_settings()?.reserve_tokens,
                    tokens,
                ) {
                    continue;
                }
            }
            if reserve && !self.usage_candidate_available(&selection.model, &selector.id, cancel).await? {
                continue;
            }
            let options = selection.generation;
            let provider = match self
                .sessions
                .provider
                .prepare_daily_replacement(
                    selection,
                    self.session.header["id"].as_str().map(str::to_owned),
                    self.session.header["id"].as_str().map(str::to_owned),
                    &self.sessions.cwd,
                    cancel,
                )
                .await
            {
                Ok(provider) => provider,
                Err(_) => continue,
            };
            if provider.route.check_fallback_auth(cancel, allow_keyless).await.is_err() {
                continue;
            }
            let mut config = self.base_config();
            config.model = provider.route.model().clone();
            config.provider = provider.build();
            config.max_tokens = self.sessions.explicit_generation.max_tokens.or(options.max_tokens);
            config.temperature = self.sessions.explicit_generation.temperature.or(options.temperature);
            let overlay = self.overlay(&self.host_tools, &self.active_host_names()).await?;
            let retained_owner = session.tool_owner.lock().unwrap().clone();
            let candidate = self
                .sessions
                .config(
                    &config,
                    provider.metadata.as_ref(),
                    Some(&self.skills),
                    false,
                    session.values.clone(),
                    session.native_session.clone(),
                    overlay,
                    Some(retained_owner),
                )
                .await?;
            let super::PreparedSessionConfig {
                mut config,
                skills,
                tool_owner,
                context_controller,
                native_publication,
                ..
            } = candidate;
            let mut journal = session.journal.lock().await;
            if !self.fallback_admission_valid(&session, generation, cancel) {
                return Ok(false);
            }
            // Native candidate fit uses the current tokenizer/provider anchor
            // and excludes the failed turn only when replay will remove it.
            // The shared counter takes max(stored, retained provider residual).
            let non_message = ara_cli::context_budget::non_message_tokens(
                &self.config.model,
                &self.config.system_prompt,
                &self.config.tools,
            );
            session.projector.refresh_checked(&journal, messages, &self.config.model)?;
            let tokens = ara_cli::context_budget::context_tokens_for_current(
                &self.config.model,
                &journal,
                &session.projector,
                messages,
                &[],
                non_message,
            )?;
            if !maintenance::retry_tokens_fit(
                config.model.context_window,
                self.compaction_policy.recovery_settings()?.reserve_tokens,
                tokens,
            ) {
                continue;
            }
            let thinking = effective_thinking.map(ThinkingLevel::as_str);
            let previous_thinking = fallback_effective_thinking(&journal);
            let was_auto = journal.current_configured_thinking_level().as_deref() == Some("auto");
            let next_auto = next_thinking == Some(ConfiguredThinkingLevel::Auto);
            let changed = was_auto != next_auto || previous_thinking != effective_thinking;
            let thinking_change = changed.then_some((thinking, if next_auto { Some("auto") } else { thinking }));
            if let Err(error) = journal.append_model_and_optional_thinking_change(
                &to,
                Some(ara_session::EPHEMERAL_MODEL_CHANGE_ROLE),
                true,
                thinking_change,
            ) {
                let status =
                    if error.history_published() { "publication durability is unknown" } else { "publication failed" };
                *session.persistence_error.lock().unwrap() =
                    Some(format!("Retry fallback model/thinking {status}: {error}"));
                self.connection.cancel();
                return Err(error.into());
            }
            // The checked Native attachment is the final publication gate.
            // A failure after this durable receipt latches the connection;
            // successful attachment has no await before route/config adoption.
            self.publish_native_after_journal(native_publication.as_ref(), &session)?;
            *session.tool_owner.lock().unwrap() = tool_owner.clone();
            let session_id = session.header["id"].as_str().map(str::to_owned);
            self.sessions.provider.adopt_daily_replacement(provider, session_id.clone(), session_id.clone());
            config.provider = self.sessions.provider.build();
            self.fallback.routed_for = session_id;
            if let Some(state) = &mut self.fallback.active {
                state.last_applied_fallback_thinking_level = next_thinking;
                state.served = Some(false);
                state.pinned |= reserve;
            } else {
                self.fallback.active = Some(ActiveRetryFallbackState {
                    role: role.clone(),
                    original_selector: from.clone(),
                    original_thinking_level: configured,
                    last_applied_fallback_thinking_level: next_thinking,
                    pinned: reserve,
                    served: Some(false),
                });
            }
            self.fallback.clear_ready();
            self.sessions.tool_owner = tool_owner;
            self.sessions.context_controller = context_controller;
            self.config = config;
            self.skills = skills;
            self.adopt_model_thinking(next_auto, prepared.request, changed);
            drop(journal);
            self.record_model_preference(&to);
            self.output.frame(json!({"type":"retry_fallback_applied","from":from,"to":selector.raw,"role":role}));
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) fn record_model_preference(&self, selector: &str) {
        // Native records actual fallback/restore adoption, never startup or
        // ordinary calls. A best-effort MRU failure does not undo the swap.
        if let Err(error) = self.model_usage_order.record_model_usage(selector) {
            eprintln!("ara: failed to record model preference: {error}");
        }
    }
    pub(super) fn clear_explicit_model_fallback(&mut self) {
        self.fallback.active = None;
        self.fallback.routed_for = None;
        self.fallback.clear_ready();
    }
    pub(super) fn note_explicit_model_adoption(&mut self, previous_route: u64) {
        let first = self
            .fallback
            .explicit_admission
            .as_ref()
            .filter(|(session, generation, _, last)| {
                Arc::ptr_eq(session, &self.session) && *generation == self.prompt_generation && *last == previous_route
            })
            .map_or(previous_route, |(_, _, first, _)| *first);
        self.fallback.explicit_admission =
            Some((self.session.clone(), self.prompt_generation, first, self.sessions.provider.route.generation()));
    }
    pub(super) async fn serving_selector(&self) -> String {
        let journal = self.session.journal.lock().await;
        let effective = fallback_effective_thinking(&journal);
        format_retry_fallback_selector(
            &current_model(&self.config.model, self.sessions.provider.metadata.as_ref()),
            effective,
        )
    }

    pub(super) async fn note_served_model(&mut self, message: &AssistantMessage) {
        let _ = self.note_served_model_terminal(&RecoveryTerminal::Legacy(message.clone())).await;
    }

    pub(super) async fn note_served_model_terminal(&mut self, message: &RecoveryTerminal) -> Result<()> {
        if !message.facts(&self.config.model)?.produced_output {
            return Ok(());
        }
        let id = self.session.header["id"].as_str().unwrap_or("").to_owned();
        let routed = self.fallback.routed_for.as_deref() == Some(id.as_str());
        let selector = self.serving_selector().await;
        message.check_host(self)?;
        if !message.facts(&self.config.model)?.produced_output {
            return Ok(());
        }
        self.fallback.last_served = Some((id, ServingModel { selector: selector.clone(), is_fallback: routed }));
        if let Some(active) = &mut self.fallback.active
            && active.served != Some(true)
        {
            active.served = Some(true);
            self.output.frame(json!({"type":"retry_fallback_succeeded","model":selector,"role":active.role}));
        }
        Ok(())
    }

    pub(super) async fn try_retry_model_fallback(
        &mut self,
        active: &ActiveRun,
        failed: &AssistantMessage,
        replay: super::super::rpc_host_retry::RetryDisposition,
        parsed_retry_after_ms: Option<f64>,
        now_ms: f64,
    ) -> Result<bool> {
        self.try_retry_model_fallback_terminal(
            active,
            &RecoveryTerminal::Legacy(failed.clone()),
            replay,
            parsed_retry_after_ms,
            now_ms,
        )
        .await
    }

    pub(super) async fn try_retry_model_fallback_terminal(
        &mut self,
        active: &ActiveRun,
        failed: &RecoveryTerminal,
        replay: super::super::rpc_host_retry::RetryDisposition,
        parsed_retry_after_ms: Option<f64>,
        now_ms: f64,
    ) -> Result<bool> {
        let Some(registry) = self.sessions.provider.registry.clone() else {
            return Ok(false);
        };
        let admission = quota::QuotaControl::begin(&self.quota_control, &self.connection);
        let mut messages = self.agent.raw_messages().await;
        if replay == super::super::rpc_host_retry::RetryDisposition::RemoveFailed {
            if !failed.exact_tail(&messages)? {
                return Ok(false);
            }
            messages.pop();
        }
        let (configured, effective, role_hint) = {
            let journal = self.session.journal.lock().await;
            let effective = fallback_effective_thinking(&journal);
            let configured = configured_thinking(journal.current_configured_thinking_level().as_deref());
            let role = journal
                .branch()
                .iter()
                .rev()
                .find(|entry| entry.kind == "model_change")
                .and_then(|entry| entry.raw["role"].as_str())
                .unwrap_or("default")
                .to_owned();
            (configured, effective, role)
        };
        let model = current_model(&self.config.model, self.sessions.provider.metadata.as_ref());
        let from = format_retry_fallback_selector(&model, effective);
        // The pure lookup context borrows trait objects. Finish the complete
        // synchronous walk before any async candidate preparation so those
        // borrows never enter the serial Host's Send future.
        let candidates = self.fallback_candidates(&registry, &model, &from, &role_hint)?;
        let random =
            u64::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap()) as f64 / u64::MAX as f64;
        registry.suppress_selector(
            &from.as_str().into(),
            now_ms + failed.fallback_cooldown(parsed_retry_after_ms, now_ms, random)?,
        )?;
        if candidates.is_empty() {
            return Ok(false);
        }
        let session = active.sink.session.clone();
        let generation = self.prompt_generation;
        tokio::select! {
            biased;
            _ = active.cancel.cancelled() => Ok(false),
            _ = admission.cancel.cancelled() => Ok(false),
            result = self.apply_fallback_candidates(candidates, &messages, configured, from, session, generation, &admission.cancel, false) => result,
        }
    }
}
