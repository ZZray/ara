//! Fixed OMP per-Session thinking selection for print and line REPL hosts.
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//! MIT; see THIRD_PARTY_NOTICES.md.

use super::*;
use ara_ai::{CallOptions, ReasoningEffort, ReasoningRequest};
use ara_cli::auto_thinking::{self as classifier, AutoThinkingBackend, ClassifierAttemptCallback};
use ara_cli::retry_fallback::ConfiguredThinkingLevel;
use ara_cli::session_thinking;
use serde_json::Value;

/// An execution route without materialized reasoning metadata has no bound
/// effort surface. Do not infer one from its model id or send an empty override.
pub(super) fn route_request(
    policy: Option<&ara_ai::OpenAIReasoningPolicy>,
    configured: ConfiguredThinkingLevel,
    ceiling: Option<ReasoningEffort>,
) -> Option<ReasoningRequest> {
    policy.map(|policy| session_thinking::resolve_selection(Some(policy), configured, ceiling))
}

struct ThinkingProvider {
    inner: Arc<dyn ModelProvider>,
    request: ReasoningRequest,
}

impl ModelProvider for ThinkingProvider {
    fn stream(&self, model: &Model, context: &ara_ai::Context, mut options: CallOptions) -> ara_ai::AssistantStream {
        options.reasoning = Some(self.request.clone());
        self.inner.stream(model, context, options)
    }
    fn stream_actual(
        &self,
        model: &Model,
        context: &ara_ai::GraphProviderContext,
        factory: Arc<ara_ai::graph_values::GraphValueFactory>,
        mut options: CallOptions,
    ) -> std::result::Result<ara_ai::ActualAssistantStream, ara_rpc::value_heap::ValueError> {
        options.reasoning = Some(self.request.clone());
        self.inner.stream_actual(model, context, factory, options)
    }
}

#[derive(Clone, Default)]
pub(super) struct CliThinking {
    configured: Option<ConfiguredThinkingLevel>,
    request: Option<ReasoningRequest>,
    last_resolved: Option<ReasoningEffort>,
}

impl CliThinking {
    pub(super) async fn initialize(
        args: &Args,
        factory: &ProviderFactory,
        sink: &HostSink,
        startup: Option<&startup_selection::StartupState>,
    ) -> Result<Self> {
        let settings = rpc_host_settings::AutoThinkingSettings::load(&ara_home().join("agent"))?;
        let mut journal = sink.journal.lock().await;
        let saved = journal
            .as_ref()
            .filter(|journal| {
                // The Native getter defaults to off even on a fresh branch;
                // only an actual thinking receipt is a saved startup selector.
                journal.branch().iter().any(|entry| entry.kind == "thinking_level_change")
            })
            .and_then(|journal| {
                journal
                    .current_configured_thinking_level()
                    .as_deref()
                    .and_then(session_thinking::parse_configured)
                    .or_else(|| {
                        journal.current_thinking_level().as_deref().and_then(session_thinking::parse_configured)
                    })
            });
        let configured = if let Some(startup) = startup {
            startup.thinking
        } else {
            session_thinking::initial_selection(
                args.thinking.last().and_then(|value| session_thinking::parse_cli(value)),
                saved,
                factory.route.reasoning_policy().and_then(|policy| policy.default_effort()),
                settings.default_thinking,
            )
        };
        let request = configured.and_then(|level| route_request(factory.route.reasoning_policy(), level, None));
        let result = Self { configured, request, last_resolved: None };
        if args.resume.is_none() && !args.continue_session && configured != Some(ConfiguredThinkingLevel::Auto) {
            let effective = result.effective_name();
            if let Some(journal) = journal.as_mut()
                && let Err(error) = journal.append_thinking_level_change(effective, None)
            {
                sink.persistence_failure(&error);
                return Err(error.into());
            }
        }
        Ok(result)
    }

    fn effective_name(&self) -> Option<&'static str> {
        if self.configured == Some(ConfiguredThinkingLevel::Concrete(ara_cli::retry_fallback::ThinkingLevel::Off)) {
            Some("off")
        } else {
            self.request.as_ref().and_then(|request| request.effort.map(ReasoningEffort::as_str))
        }
    }

    fn persist(&self, journal: Option<&mut SessionJournal>, sink: &HostSink) -> Result<()> {
        let Some(request) = &self.request else {
            return Ok(());
        };
        let effective =
            if request.disable_reasoning { Some("off") } else { request.effort.map(ReasoningEffort::as_str) };
        let configured = if self.configured == Some(ConfiguredThinkingLevel::Auto) { Some("auto") } else { effective };
        if let Some(journal) = journal
            && (journal.current_thinking_level().as_deref() != effective
                || journal.current_configured_thinking_level().as_deref() != configured)
            && let Err(error) = journal.append_thinking_level_change(effective, configured)
        {
            sink.persistence_failure(&error);
            return Err(error.into());
        }
        Ok(())
    }

    pub(super) fn bind(&self, provider: Arc<dyn ModelProvider>) -> Arc<dyn ModelProvider> {
        match &self.request {
            Some(request) => Arc::new(ThinkingProvider { inner: provider, request: request.clone() }),
            None => provider,
        }
    }

    pub(super) async fn new_session(&mut self, sink: &HostSink) -> Result<()> {
        // Native /new keeps ModelControls, including its last Auto resolution.
        let effective = self.effective_name();
        let configured = if self.configured == Some(ConfiguredThinkingLevel::Auto) { Some("auto") } else { effective };
        if let Some(journal) = sink.journal.lock().await.as_mut()
            && let Err(error) = journal.append_thinking_level_change(effective, configured)
        {
            sink.persistence_failure(&error);
            return Err(error.into());
        }
        Ok(())
    }

    pub(super) async fn prepare_turn(
        &mut self,
        args: &Args,
        factory: &ProviderFactory,
        project: &Path,
        prompt: String,
        sink: &Arc<HostSink>,
        cancel: &CancellationToken,
    ) -> Result<()> {
        if self.configured != Some(ConfiguredThinkingLevel::Auto) {
            return Ok(());
        }
        let Some(policy) = factory.route.reasoning_policy().cloned() else {
            return Ok(());
        };
        if policy.supported_efforts().is_empty() {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + classifier::CLASSIFICATION_TIMEOUT;
        let owner = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            _ = tokio::time::sleep_until(deadline) => return Ok(()),
            journal = sink.journal.lock() => journal.as_ref().context("Session unavailable")?.model_usage_owner(),
        };
        let session_id = owner.session_id.clone();
        let owner = Arc::new(tokio::sync::Mutex::new(owner));
        let captured = sink.clone();
        let account = factory.shared_account.clone();
        // Keep observed terminals even if the classification times out or is
        // cancelled. Installation is separately gated below.
        let usage: ClassifierAttemptCallback = Arc::new(move |attempt| {
            let captured = captured.clone();
            let account = account.clone();
            let owner = owner.clone();
            Box::pin(async move {
                let message = attempt.message;
                let record = ara_session::ModelUsageRecord {
                    purpose: "auto-thinking".into(),
                    role: Some(attempt.role),
                    api: message.api.clone(),
                    provider: message.provider.clone(),
                    model: message.model.clone(),
                    usage: message.usage.clone(),
                    stop_reason: message.stop_reason,
                    error_message: message.error_message.clone(),
                };
                let mut owner = owner.lock().await;
                let mut journal = captured.journal.lock().await;
                let journal = journal.as_mut().ok_or("classifier Session unavailable")?;
                match journal.append_model_usage(&record, &owner) {
                    Ok(Some(id)) => owner.parent_id = Some(id),
                    Ok(None) => {}
                    Err(error) => {
                        captured.persistence_failure(&error);
                        return Err("classifier usage persistence failed".into());
                    }
                }
                if let Some(account) = account {
                    account.record_assistant_usage(&message);
                }
                Ok(())
            })
        });
        let worker_cancel = cancel.child_token();
        let work_cancel = worker_cancel.clone();
        let target = Some(policy.clone());
        let side_factory = factory.borrowed_side_factory();
        let path = args.models_config.clone();
        let project = project.to_path_buf();
        let usage_order =
            Arc::new(ara_cli::model_usage_order::SqliteModelUsageOrder::for_agent_dir(ara_home().join("agent")));
        let mut worker = tokio::spawn(classify(
            target,
            side_factory,
            project,
            path,
            prompt,
            session_id,
            usage_order,
            usage,
            work_cancel,
        ));
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => { worker_cancel.cancel(); None },
            _ = tokio::time::sleep_until(deadline) => { worker_cancel.cancel(); None },
            result = &mut worker => result.ok().and_then(|result| result.ok()).flatten(),
        };
        // The reference print/REPL host is serial. Settle a cancelled side call
        // before replacing its journal or exiting; never detach observed usage.
        if !worker.is_finished() {
            let _ = worker.await;
        }
        if cancel.is_cancelled() || sink.persist_failed.load(Ordering::SeqCst) {
            return Ok(());
        }
        if let Some(effort) = classifier::resolve_auto_effort(
            policy.supported_efforts(),
            result,
            self.last_resolved,
            policy.default_effort(),
            None,
        ) {
            let changed = self.request.as_ref().and_then(|request| request.effort) != Some(effort);
            self.last_resolved = Some(effort);
            self.request = Some(ReasoningRequest { effort: Some(effort), ..Default::default() });
            if changed {
                self.persist(sink.journal.lock().await.as_mut(), sink)?;
            }
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn classify(
    target: Option<ara_ai::reasoning::OpenAIReasoningPolicy>,
    factory: ProviderFactory,
    project: PathBuf,
    config_path: Option<PathBuf>,
    prompt: String,
    session_id: String,
    model_usage_order: Arc<ara_cli::model_usage_order::SqliteModelUsageOrder>,
    usage: ClassifierAttemptCallback,
    cancel: CancellationToken,
) -> Result<Option<ReasoningEffort>> {
    let policy = target.context("Auto target has no reasoning policy")?;
    if policy.supported_efforts().is_empty() {
        return Ok(None);
    }
    let settings = super::rpc_host_settings::AutoThinkingSettings::load(&super::ara_home().join("agent"))?;
    if settings.ultrathink && classifier::has_ultrathink(&prompt) {
        return Ok(classifier::clamp_auto_effort(
            policy.supported_efforts(),
            ReasoningEffort::Max,
            ReasoningEffort::Max,
        ));
    }
    let backend = AutoThinkingBackend::parse(&settings.backend).context("unsupported Auto classifier backend")?;
    if backend != AutoThinkingBackend::Online {
        bail!("production Local classifier runtime remains required");
    }
    let registry = factory.registry.clone().context("Auto classifier registry is unavailable")?;
    let choice = {
        let lookup = |role: &str| ara_cli::model_roles::model_role_value(settings.roles.get(role));
        let resolver = registry.model_role_resolver(ara_cli::model_roles::ModelMatchPreferences {
            usage_order: model_usage_order.get_model_usage_order().unwrap_or_else(|error| {
                eprintln!("ara: failed to read model preference order: {error}");
                Vec::new()
            }),
            provider_order: settings.provider_order.clone(),
            ..Default::default()
        })?;
        resolver
            .resolve_role_selection(&["tiny", "smol"], &lookup)?
            .context("Auto classifier has no configured tiny/smol model")?
    };
    let spec: Value = serde_json::from_str(&choice.model.spec().to_wire_json().stringify())?;
    let provider = spec["provider"].as_str().context("classifier provider missing")?.to_owned();
    let id = spec["id"].as_str().context("classifier model missing")?.to_owned();
    let selector = ara_cli::retry_fallback::RetryFallbackSelector {
        raw: format!("{provider}/{id}"),
        provider,
        id,
        thinking_level: None,
    };
    // Native getApiKey returns its non-empty no-auth marker for
    // explicitly keyless providers. Preserve that admission while
    // still resolving the side route's actual authentication.
    let allow_keyless = registry.is_keyless_provider(&selector.provider.as_str().into());
    let path = config_path.clone().unwrap_or_else(|| super::ara_home().join("agent/models.yml"));
    let models = ara_cli::daily_model_config::load_daily_config(&path, config_path.is_some())?;
    let mut selection = ara_cli::daily_model_config::resolve_daily_role_selection(
        models.as_ref(),
        &selector,
        &ara_cli::schema_policy::process_daily_environment,
        &choice.model,
    )?;
    // CallOptions::None inherits the prepared base. Native Codex
    // deliberately omits classifier output caps on wire, so clear
    // this side route's base without changing the main route.
    if let ara_cli::model_route::ProtocolOptions::CodexResponses(options) = &mut selection.protocol {
        options.request.max_tokens = None;
    }
    let side = factory
        .prepare_daily_replacement(selection, Some(session_id.clone()), Some(session_id.clone()), &project, &cancel)
        .await?;
    side.route
        .check_fallback_auth(&cancel, allow_keyless)
        .await
        .map_err(|_| anyhow::anyhow!("classifier authentication unavailable"))?;
    let ceiling = classifier::auto_ceiling(backend, settings.allow_max, policy.supported_efforts());
    let result = classifier::classify_online(
        classifier::OnlineClassificationRequest {
            provider: side.route.bind_side_request(side.client.clone(), &session_id),
            model: side.route.model().clone(),
            options: CallOptions::default(),
            role: choice.role,
            prompt,
            allow_max: ceiling == ReasoningEffort::Max,
            cancel,
        },
        usage,
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok::<_, anyhow::Error>(
        result.and_then(|result| classifier::clamp_auto_effort(policy.supported_efforts(), result, ceiling)),
    )
}
