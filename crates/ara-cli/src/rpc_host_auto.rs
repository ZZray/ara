//! Host-owned fixed OMP Auto thinking (model-controls.ts:501-666).
//! A classifier is a side call: it never adopts the main model or a new Session.
//! MIT; see THIRD_PARTY_NOTICES.md.
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.

use super::*;
use ara_ai::{CallOptions, ModelProvider, ReasoningEffort, ReasoningRequest};
use ara_cli::auto_thinking::{self as classifier, ClassifierAttemptCallback};
use ara_session::{ModelUsageOwner, ModelUsageRecord};

/// Reader admission only pauses the deferred start. The serial command handler
/// decides whether a validated mutation actually revokes the accepted input.
#[derive(Default)]
pub(super) struct StartControl {
    queued: usize,
}

impl StartControl {
    pub(super) fn pauses_start(command: &Command) -> bool {
        quota::QuotaControl::stops_quota(command)
            || matches!(
                command.kind.as_str(),
                "set_thinking_level"
                    | "cycle_model"
                    | "set_model"
                    | "prompt"
                    | "set_auto_compaction"
                    | "set_host_tools"
                    | "steer"
                    | "follow_up"
                    | "set_steering_mode"
                    | "set_follow_up_mode"
            )
    }
    pub(super) fn queue(&mut self) {
        self.queued = self.queued.saturating_add(1);
    }
    pub(super) fn consume(&mut self) {
        self.queued = self.queued.saturating_sub(1);
    }
    fn paused(&self) -> bool {
        self.queued > 0
    }
}

struct PendingStart {
    message: AgentMessage,
    command: Command,
    session: Arc<Session>,
    generation: u64,
    route_generation: u64,
    cancel: CancellationToken,
    restored: bool,
    drain_queues: bool,
    usage_ready: Option<fallback::AdmissionReady>,
    classification_done: bool,
    resolution: Option<Completion>,
    maintained: bool,
}

#[derive(Default)]
pub(super) struct AutoState {
    enabled: bool,
    pub control: Arc<Mutex<StartControl>>,
    pending: Option<PendingStart>,
    resuming: bool,
    pub done: Option<mpsc::UnboundedSender<Completion>>,
    jobs: Vec<JoinHandle<()>>,
    epoch: u64,
    effective: Option<ReasoningEffort>,
    last_resolved: Option<ReasoningEffort>,
    ceiling: Option<ReasoningEffort>,
    // Effort-only changes retain the same underlying provider/session state.
    inner: Option<Arc<dyn ModelProvider>>,
    installed: Option<Arc<dyn ModelProvider>>,
    request: Option<ReasoningRequest>,
    request_route_generation: Option<u64>,
    classification_cancel: Option<CancellationToken>,
}

pub(super) struct Completion {
    session: Arc<Session>,
    generation: u64,
    route_generation: u64,
    epoch: u64,
    result: Option<ReasoningEffort>,
}

struct ThinkingProvider {
    inner: Arc<dyn ModelProvider>,
    request: ReasoningRequest,
}

impl ModelProvider for ThinkingProvider {
    fn stream(
        &self,
        model: &ara_ai::Model,
        context: &ara_ai::Context,
        mut options: CallOptions,
    ) -> ara_ai::AssistantStream {
        options.reasoning = Some(self.request.clone());
        self.inner.stream(model, context, options)
    }
    fn stream_actual(
        &self,
        model: &ara_ai::Model,
        context: &ara_ai::GraphProviderContext,
        factory: Arc<ara_ai::graph_values::GraphValueFactory>,
        mut options: CallOptions,
    ) -> std::result::Result<ara_ai::ActualAssistantStream, ara_rpc::value_heap::ValueError> {
        options.reasoning = Some(self.request.clone());
        self.inner.stream_actual(model, context, factory, options)
    }
}

impl AutoState {
    pub(super) fn with_done(done: mpsc::UnboundedSender<Completion>) -> Self {
        Self { done: Some(done), ..Default::default() }
    }
    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }
    pub(super) fn current_request(&self) -> Option<&ReasoningRequest> {
        self.request.as_ref()
    }
    pub(super) fn busy(&self) -> bool {
        self.pending.is_some() || self.resuming
    }
    pub(super) fn preparing_accepted_input(&self) -> bool {
        self.resuming
    }

    pub(super) async fn settle_jobs(&mut self) {
        for job in self.jobs.drain(..) {
            let _ = job.await;
        }
    }
}

fn usage_callback(
    session: Arc<Session>,
    account: Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    owner: ModelUsageOwner,
    connection: CancellationToken,
) -> ClassifierAttemptCallback {
    let owner = Arc::new(tokio::sync::Mutex::new(owner));
    Arc::new(move |attempt| {
        let session = session.clone();
        let owner = owner.clone();
        let account = account.clone();
        let connection = connection.clone();
        Box::pin(async move {
            let message = attempt.message;
            let record = ModelUsageRecord {
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
            let written = session.journal.lock().await.append_model_usage(&record, &owner);
            match written {
                Ok(Some(id)) => owner.parent_id = Some(id),
                Ok(None) => {}
                Err(error) => {
                    *session.persistence_error.lock().unwrap() = Some(error.to_string());
                    connection.cancel();
                    return Err("classifier usage persistence failed".into());
                }
            }
            if let Some(account) = account {
                account.record_assistant_usage(&message);
            }
            Ok(())
        })
    })
}

impl Host {
    pub(super) async fn restore_auto_selection(&mut self) -> Result<()> {
        if let Some(cancel) = self.auto.classification_cancel.take() {
            cancel.cancel();
        }
        self.auto.epoch = self.auto.epoch.wrapping_add(1);
        self.auto.last_resolved = None;
        // Same-file adoption can retain this exact provider. Remove the old
        // effort wrapper before forgetting its identity, otherwise a new
        // outer wrapper would delegate to the old effort override.
        if self.auto.installed.as_ref().is_some_and(|installed| Arc::ptr_eq(installed, &self.config.provider))
            && let Some(inner) = &self.auto.inner
        {
            self.config.provider = inner.clone();
            self.publish_snapshot(&self.config);
        }
        self.auto.inner = None;
        self.auto.installed = None;
        self.auto.request = None;
        self.auto.request_route_generation = None;
        self.auto.effective = None;
        self.auto.enabled = false;
        let saved = {
            let journal = self.session.journal.lock().await;
            // A getter's default off is not a persisted Session choice.
            let has_entry = journal.branch().iter().any(|entry| entry.kind == "thinking_level_change");
            has_entry
                .then(|| {
                    journal
                        .current_configured_thinking_level()
                        .as_deref()
                        .and_then(ara_cli::session_thinking::parse_configured)
                        .or_else(|| {
                            journal
                                .current_thinking_level()
                                .as_deref()
                                .and_then(ara_cli::session_thinking::parse_configured)
                        })
                })
                .flatten()
        };
        let settings =
            super::super::rpc_host_settings::AutoThinkingSettings::load(&super::super::ara_home().join("agent"))?;
        let configured = ara_cli::session_thinking::initial_selection(
            None,
            saved,
            self.sessions.provider.route.reasoning_policy().and_then(|policy| policy.default_effort()),
            settings.default_thinking,
        );
        if let Some(configured) = configured {
            // Restore runtime selection without appending a user change.
            // Clamping on the launch route must not rewrite saved receipts.
            self.set_thinking_selection(ara_cli::session_thinking::configured_name(configured), false).await?;
        }
        Ok(())
    }

    pub(super) async fn initialize_thinking_selection(&mut self) -> Result<()> {
        self.auto.ceiling=self.session.journal.lock().await.latest_child_init()?
            .and_then(|init|init.extra.get("araTaskEffortCeiling").and_then(Value::as_str).and_then(ReasoningEffort::parse));
        if let Some(startup) = &self.sessions.startup {
            let thinking = startup.thinking;
            if let Some(level) = thinking {
                self.set_thinking_selection(ara_cli::session_thinking::configured_name(level), false).await?;
            }
        } else {
            self.restore_auto_selection().await?;
            if let Some(level) =
                self.sessions.args.thinking.last().and_then(|value| ara_cli::session_thinking::parse_cli(value))
            {
                self.set_thinking_selection(ara_cli::session_thinking::configured_name(level), false).await?;
            }
        }
        // Fixed SDK saves initial concrete thinking only on a new Session.
        // Restored Sessions retain original receipts even with a CLI override.
        if self.sessions.args.resume.is_none() && !self.sessions.args.continue_session && !self.auto.enabled() {
            let effective = self.auto.request.as_ref().and_then(|request| {
                if request.disable_reasoning { Some("off") } else { request.effort.map(ReasoningEffort::as_str) }
            });
            self.session.journal.lock().await.append_thinking_level_change(effective, None)?;
        }
        Ok(())
    }
    pub(super) async fn set_thinking_level(&mut self, configured: &str) -> Result<()> {
        self.set_thinking_selection(configured, true).await
    }

    pub(super) fn thinking_ceiling(&self) -> Option<ReasoningEffort> {
        self.auto.ceiling
    }

    /// ModelControls re-applies the configured selector on a route change,
    /// clearing its old per-turn resolution. The caller already published the
    /// model/thinking journal transaction and installed the prepared provider.
    pub(super) fn adopt_model_thinking(&mut self, auto: bool, request: ReasoningRequest, changed: bool) {
        if let Some(cancel) = self.auto.classification_cancel.take() {
            cancel.cancel();
        }
        self.auto.epoch = self.auto.epoch.wrapping_add(1);
        self.auto.last_resolved = None;
        self.auto.enabled = auto;
        self.auto.effective = request.effort;
        self.auto.inner = None;
        self.auto.installed = None;
        self.auto.request = None;
        self.auto.request_route_generation = None;
        let effective =
            if request.disable_reasoning { Some("off") } else { request.effort.map(|effort| effort.as_str()) };
        if auto {
            self.install_thinking(request);
        } else {
            self.auto.request = Some(request);
            self.auto.request_route_generation = Some(self.sessions.provider.route.generation());
            self.publish_snapshot(&self.config);
            self.fallback.clear_ready();
        }
        if changed {
            let mut event = json!({"type":"thinking_level_changed","thinkingLevel":effective});
            if auto {
                event["configured"] = json!("auto");
            }
            self.output.frame(event);
        }
    }

    /// A validated explicit model change supersedes the old classifier result,
    /// while the already accepted User retains its command and Session owner.
    pub(super) fn rebind_pending_model_selection(&mut self) {
        if let Some(start) = &mut self.auto.pending {
            start.route_generation = self.sessions.provider.route.generation();
            start.resolution = None;
            start.classification_done = true;
            start.maintained = false;
            start.usage_ready = None;
        }
    }

    async fn set_thinking_selection(&mut self, configured: &str, persist: bool) -> Result<()> {
        let configured = ara_cli::session_thinking::parse_configured(configured).context("unknown thinking level")?;
        let auto = configured == ara_cli::retry_fallback::ConfiguredThinkingLevel::Auto;
        let policy = self.sessions.provider.route.reasoning_policy().cloned();
        let request = super::super::host_thinking::route_request(policy.as_ref(), configured, self.auto.ceiling)
            .unwrap_or_else(|| ReasoningRequest {
                disable_reasoning: configured
                    == ara_cli::retry_fallback::ConfiguredThinkingLevel::Concrete(
                        ara_cli::retry_fallback::ThinkingLevel::Off,
                    ),
                ..Default::default()
            });
        let selected = request.effort;
        self.check_thinking_ceiling(&request)?;
        let previous_auto = self.auto.enabled();
        let effective = if request.disable_reasoning { Some("off") } else { selected.map(|level| level.as_str()) };
        let changed = {
            let journal = self.session.journal.lock().await;
            previous_auto != auto
                || journal.current_thinking_level().as_deref() != effective
                || journal.current_configured_thinking_level().as_deref() != if auto { Some("auto") } else { effective }
        };
        if changed
            && persist
            && let Err(error) = self
                .session
                .journal
                .lock()
                .await
                .append_thinking_level_change(effective, if auto { Some("auto") } else { effective })
        {
            *self.session.persistence_error.lock().unwrap() = Some(error.to_string());
            self.connection.cancel();
            return Err(error.into());
        }
        if let Some(cancel) = self.auto.classification_cancel.take() {
            cancel.cancel();
        }
        self.auto.epoch = self.auto.epoch.wrapping_add(1);
        self.auto.last_resolved = None;
        self.auto.enabled = auto;
        self.install_thinking(request);
        self.auto.effective = selected;
        if changed {
            let mut event = json!({"type":"thinking_level_changed","thinkingLevel":effective});
            if auto {
                event["configured"] = json!("auto");
            }
            self.output.frame(event);
        }
        Ok(())
    }

    fn check_thinking_ceiling(&self, request: &ReasoningRequest) -> Result<()> {
        if let (Some(ceiling), Some(policy)) = (self.auto.ceiling, self.sessions.provider.route.reasoning_policy()) {
            if !policy.model_supports_effort_ceiling(ceiling) {
                bail!("model has no effort within the Session ceiling");
            }
            let wire = policy.normalize_request(Some(request)).map_err(|error| anyhow::anyhow!(error.to_string()))?;
            if wire.effort.is_some_and(|effort| effort > ceiling) {
                bail!("model reasoning default exceeds the Session ceiling");
            }
        }
        Ok(())
    }

    fn install_thinking(&mut self, request: ReasoningRequest) {
        if !self.auto.installed.as_ref().is_some_and(|installed| Arc::ptr_eq(installed, &self.config.provider)) {
            self.auto.inner = Some(self.config.provider.clone());
        }
        self.auto.request = Some(request.clone());
        self.auto.request_route_generation = Some(self.sessions.provider.route.generation());
        let inner = self.auto.inner.as_ref().expect("captured provider").clone();
        let provider: Arc<dyn ModelProvider> = if self.sessions.provider.route.reasoning_policy().is_some() {
            Arc::new(ThinkingProvider { inner, request })
        } else {
            inner
        };
        self.config.provider = provider.clone();
        self.auto.installed = Some(provider);
        self.publish_snapshot(&self.config);
        self.fallback.clear_ready();
    }

    pub(super) fn rebind_context_thinking(&mut self) {
        // Context maintenance deliberately rebuilds provider replay state.
        // Native effort is separate Agent state and survives that rebuild.
        // Route adoption owns its own selection/reclamping; do not reapply an
        // old route's request to a newly adopted model here.
        if self.auto.request_route_generation == Some(self.sessions.provider.route.generation())
            && !self.auto.installed.as_ref().is_some_and(|installed| Arc::ptr_eq(installed, &self.config.provider))
            && let Some(request) = self.auto.request.clone()
        {
            self.install_thinking(request);
        }
    }

    /// Native initial User/Skill classification precedes preprompt maintenance
    /// and Agent entry. The pending slot keeps the serial Host responsive.
    pub(super) fn begin_auto_start(
        &mut self,
        message: AgentMessage,
        command: Command,
        restored: bool,
        drain_queues: bool,
    ) -> Result<()> {
        let deadline = tokio::time::Instant::now() + classifier::CLASSIFICATION_TIMEOUT;
        let prompt = if let Some(solution)=self.child_turn.as_ref().filter(|turn|!turn.is_continuation()).and_then(|turn|turn.classification_input()){
            // Fixed Task solutionSpace is classifier input, never user prose.
            solution
        } else if let Some(skill) = input_skill(&message)? {
            user_message_text(&skill.event_message())
        } else {
            user_message_text(&message.json()?)
        };
        let session = self.session.clone();
        let generation = self.prompt_generation;
        let route_generation = self.sessions.provider.route.generation();
        let cancel = self.connection.child_token();
        self.auto.pending = Some(PendingStart {
            message,
            command,
            session: session.clone(),
            generation,
            route_generation,
            cancel: cancel.clone(),
            restored,
            drain_queues,
            usage_ready: self.fallback.take_ready(),
            classification_done: false,
            resolution: None,
            maintained: false,
        });
        let Some(done) = self.auto.done.clone() else {
            self.complete_auto(Completion {
                session,
                generation,
                route_generation,
                epoch: self.auto.epoch,
                result: None,
            });
            return Ok(());
        };
        let epoch = self.auto.epoch;
        let target = self.sessions.provider.route.reasoning_policy().cloned();
        let factory = self.sessions.provider.borrowed_side_factory();
        let project = self.sessions.cwd.clone();
        let model_usage_order = self.model_usage_order.clone();
        let config_path = self.sessions.args.models_config.clone();
        let session_id = session.header["id"].as_str().unwrap_or("").to_owned();
        let usage_session = session.clone();
        let connection = self.connection.clone();
        let worker_cancel = cancel.child_token();
        self.auto.classification_cancel = Some(worker_cancel.clone());
        let pipeline_cancel = worker_cancel.clone();
        let job = tokio::spawn(async move {
            let mut worker = tokio::spawn(async move {
                // Owner capture is part of the same deadline, never after a
                // User/Skill append. A later branch/Bash does not retarget it.
                let owner = tokio::select! {
                    biased;
                    _ = pipeline_cancel.cancelled() => return Ok(None),
                    _ = tokio::time::sleep_until(deadline) => return Ok(None),
                    journal = usage_session.journal.lock() => journal.model_usage_owner(),
                };
                let usage = usage_callback(usage_session, factory.shared_account.clone(), owner, connection);
                super::super::host_thinking::classify(
                    target,
                    factory,
                    project,
                    config_path,
                    prompt,
                    session_id,
                    model_usage_order,
                    usage,
                    pipeline_cancel,
                )
                .await
            });
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => { worker_cancel.cancel(); None },
                _ = worker_cancel.cancelled() => None,
                _ = tokio::time::sleep_until(deadline) => { worker_cancel.cancel(); None },
                result = &mut worker => {
                    result.ok().and_then(|result| result.ok()).flatten()
                },
            };
            let _ = done.send(Completion { session, generation, route_generation, epoch, result });
            // A timeout stops installation immediately. Retain the worker so
            // an already observed terminal can settle on its original owner.
            if !worker.is_finished() {
                let _ = worker.await;
            }
        });
        self.auto.jobs.retain(|job| !job.is_finished());
        self.auto.jobs.push(job);
        Ok(())
    }

    pub(super) fn complete_auto(&mut self, completion: Completion) {
        if let Some(start) = &mut self.auto.pending
            && !start.classification_done
            && Arc::ptr_eq(&start.session, &completion.session)
            && start.generation == completion.generation
            && start.route_generation == completion.route_generation
        {
            start.classification_done = true;
            start.resolution = Some(completion);
        }
    }

    pub(super) fn cancel_auto_start(&mut self) {
        if let Some(start) = self.auto.pending.take() {
            start.cancel.cancel();
            self.output.prompt_skipped(&start.command);
        }
    }

    fn pending_start_valid(&self, start: &PendingStart) -> bool {
        Arc::ptr_eq(&start.session, &self.session)
            && start.generation == self.prompt_generation
            && start.route_generation == self.sessions.provider.route.generation()
            && !start.cancel.is_cancelled()
            && !self.connection.is_cancelled()
    }

    // Construct on the heap before entering the serial command future. This
    // preserves the Windows main-thread stack boundary used by start().
    pub(super) fn resume_auto_start(&mut self) -> Pin<Box<impl Future<Output = ()> + '_>> {
        Box::pin(self.resume_auto_start_inner())
    }

    async fn resume_auto_start_inner(&mut self) {
        if !self.auto.pending.as_ref().is_some_and(|start| start.classification_done)
            || self.auto.control.lock().unwrap().paused()
        {
            return;
        }
        let mut start = self.auto.pending.take().expect("ready initial prompt");
        // Maintenance's established idle abort must not revoke its own pending
        // input. Keep that owner local and retain busy/drain semantics.
        self.auto.resuming = true;
        let outcome = async {
            if !self.pending_start_valid(&start) { return Err(ara_cli::handoff::cancelled()); }
            if let Some(completion) = &start.resolution {
                let mut journal = start.session.journal.lock().await;
                let control = self.auto.control.clone();
                let gate = control.lock().unwrap();
                if gate.paused() { return Ok(false); }
                if !self.pending_start_valid(&start) { return Err(ara_cli::handoff::cancelled()); }
                if start.session.persistence_error.lock().unwrap().is_some() {
                    bail!("classifier usage persistence failed");
                }
                // A pin (including pin then re-enter Auto) supersedes this
                // result, while the accepted initial input still continues.
                if completion.epoch == self.auto.epoch && self.auto.enabled() {
                    let policy = self.sessions.provider.route.reasoning_policy().cloned();
                    let effort = policy.as_ref().and_then(|policy| classifier::resolve_auto_effort(
                        policy.supported_efforts(), completion.result, self.auto.last_resolved,
                        policy.default_effort(), self.auto.ceiling,
                    ));
                    if let Some(effort) = effort {
                        let thinking = ReasoningRequest { effort: Some(effort), ..Default::default() };
                        self.check_thinking_ceiling(&thinking)?;
                        if self.auto.effective != Some(effort)
                            && let Err(error) = journal.append_thinking_level_change(Some(effort.as_str()), Some("auto"))
                        {
                            *start.session.persistence_error.lock().unwrap() = Some(error.to_string());
                            self.connection.cancel();
                            return Err(error.into());
                        }
                        self.auto.effective = Some(effort);
                        self.auto.last_resolved = Some(effort);
                        self.install_thinking(thinking);
                        self.output.frame(json!({"type":"thinking_level_changed","thinkingLevel":effort.as_str(),"configured":"auto","resolved":effort.as_str()}));
                    }
                }
            }
            start.resolution = None;
            if !start.maintained {
                if self.auto.control.lock().unwrap().paused() { return Ok(false); }
                let pending = [start.message.clone()];
                let maintained = if start.restored {
                    self.maybe_auto_compact_for_restore(&pending).await
                } else {
                    self.maybe_auto_compact(&pending).await
                };
                if !maintained { return Err(ara_cli::handoff::cancelled()); }
                start.maintained = true;
                self.restore_usage_ready(start.usage_ready.take());
                self.auto_compaction_pending = false;
            }
            // Classification publication is not the main-request admission.
            // After the last await, use the reader's gate through synchronous
            // dispatch. A queued mutation is handled before resuming again.
            let control = self.auto.control.clone();
            let gate = control.lock().unwrap();
            if gate.paused() { return Ok(false); }
            if !self.pending_start_valid(&start) { return Err(ara_cli::handoff::cancelled()); }
            if start.session.persistence_error.lock().unwrap().is_some() {
                bail!("session persistence failed; restart from the journal before continuing");
            }
            self.drain_queues = start.drain_queues;
            self.dispatch(Some(start.message.clone()), Command {
                id: start.command.id.clone(), kind: start.command.kind.clone(), frame: start.command.frame.clone(),
            })?;
            Ok(true)
        }.await;
        self.auto.resuming = false;
        match outcome {
            Ok(true) => {}
            Ok(false) => self.auto.pending = Some(start),
            Err(error) => {
                start.cancel.cancel();
                self.fallback.clear_ready();
                self.drain_queues = false;
                self.auto_compaction_pending = false;
                if ara_cli::handoff::is_cancelled(&error) {
                    self.output.prompt_skipped(&start.command);
                } else {
                    self.output.response(&start.command, None, Some(error.to_string()));
                }
            }
        }
    }
}
