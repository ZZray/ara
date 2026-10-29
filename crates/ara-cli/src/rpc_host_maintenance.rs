//! Fixed OMP 596f2da same-route overflow and incomplete-output recovery.
//! Snapcompact and other unported method surfaces retain separate gates.
use super::super::rpc_host_retry::NativeRetryContextReceipt;
use super::terminal::{ContinuationKind, FactsPurpose, NativeContinuationGuard, RecoveryToken};
use super::*;
use ara_agent::compaction::{
    AcceptedSummary, SummaryOptions, select_native_entry_compaction_cut, summarize_native_entry_compaction_cut,
    summary_output_budget_tokens,
};
use ara_agent::tokenizer::MessageCountOptions;
use ara_ai::Model;
use ara_session::{FailedAssistantRecovery, FailedAssistantRecoveryOutcome, NativeProjectedCompactionSnapshot};

#[derive(Default)]
pub(super) struct MaintenanceOutcome {
    pub deferred_handoff: bool,
    pub continuation_scheduled: bool,
    pub automatic_continuation_blocked: bool,
    pub history_rewritten: bool,
}

pub(super) struct ActiveMaintenance {
    pub task: tokio::task::JoinHandle<Result<AcceptedSummary>>,
    cancel: CancellationToken,
    session: Arc<Session>,
    generation: u64,
    command: Command,
    snapshot: NativeProjectedCompactionSnapshot,
    first_kept: String,
    tokens_before: u64,
    expected_messages: Vec<Value>,
    recovery: RecoveryToken,
    native_context: Option<NativeRetryContextReceipt>,
    native_guard: Option<NativeContinuationGuard>,
    reserve_tokens: Option<f64>,
}

pub(super) struct PendingMaintenanceContinue {
    session: Arc<Session>,
    generation: u64,
    command: Command,
    expected_messages: Vec<Value>,
    reserve_tokens: Option<f64>,
    check_fit: bool,
    native_context: Option<NativeRetryContextReceipt>,
    native_guard: Option<NativeContinuationGuard>,
    pub deadline: Instant,
}

#[derive(Default)]
pub(super) struct TerminalRecoveryState {
    pub incomplete_attempts: usize,
    pub empty_attempts: usize,
    pub unexpected_attempts: usize,
}

fn safe_success_terminal(message: &AssistantMessage, api: &str) -> bool {
    !matches!(api, "openai-responses" | "openai-codex-responses")
        || message.terminal_context_recovery == Some(ara_ai::ContextRecoveryEvidence::ContentOnly)
}

fn produced_output(message: &AssistantMessage) -> bool {
    ara_session::assistant_turn_produced_output(message)
}

fn stored_tokens(model: &Model, messages: &[Message]) -> usize {
    ara_cli::context_budget::tokenizer(model)
        .count_messages(messages, MessageCountOptions { exclude_encrypted_reasoning: true })
}

// Known buckets are a lower bound, never a claim that unknown usage is zero.
fn reported_input(message: &AssistantMessage) -> (u64, Option<u64>) {
    let buckets = [message.usage.input, message.usage.cache_read, message.usage.cache_write];
    let lower = buckets.into_iter().flatten().fold(0u64, u64::saturating_add);
    (lower, buckets.iter().all(Option::is_some).then_some(lower))
}

fn persisted_assistant_id(active: &ActiveRun, message: &AssistantMessage) -> Result<String> {
    active
        .sink
        .entries
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|(id, candidate)| (candidate.as_assistant() == Some(message)).then(|| id.clone()))
        .context("recovery assistant has no persisted native entry ID")
}

pub(super) fn retry_context_capacity(window: Option<f64>, reserve: Option<f64>) -> Option<f64> {
    let window = window.filter(|window| window.is_finite() && *window > 0.0)?;
    // Fixed agent compaction.ts:303-333. Preserve explicit/default provenance.
    let proportional = (window * 0.15).floor().max(1.0);
    let effective = (window * 0.15).floor().max(reserve.unwrap_or(16_384.0));
    let resolved = if reserve.is_none() && effective >= window - proportional || effective >= window {
        proportional
    } else {
        effective
    };
    Some((window - resolved).max(0.0))
}

pub(super) fn retry_tokens_fit(window: Option<f64>, reserve: Option<f64>, tokens: usize) -> bool {
    retry_context_capacity(window, reserve).is_none_or(|capacity| tokens as f64 <= capacity)
}

impl Host {
    fn queue_terminal_continue(
        &mut self,
        active: &ActiveRun,
        messages: Vec<AgentMessage>,
        terminal: &RecoveryTerminal,
        kind: ContinuationKind,
    ) -> Result<()> {
        terminal.check_host(self)?;
        self.maintenance_continue = Some(PendingMaintenanceContinue {
            session: self.session.clone(),
            generation: self.prompt_generation,
            command: Command {
                id: active.command.id.clone(),
                kind: active.command.kind.clone(),
                frame: active.command.frame.clone(),
            },
            expected_messages: if terminal.actual().is_some() {
                Vec::new()
            } else {
                self.session.projector.current_body_receipts(&messages)?
            },
            reserve_tokens: None,
            check_fit: false,
            native_context: terminal.context_receipt(&messages)?,
            native_guard: NativeContinuationGuard::for_terminal(terminal, kind),
            deadline: Instant::now() + Duration::from_millis(100),
        });
        self.auto_compaction_pending = false;
        Ok(())
    }

    async fn try_context_promotion(&mut self, active: &ActiveRun, message: &RecoveryTerminal) -> Result<bool> {
        use ara_cli::daily_model_config::{load_daily_config, resolve_daily_promotion_selection};
        let agent_dir = super::super::ara_home().join("agent");
        let Some(metadata) = self.sessions.provider.metadata.as_ref() else {
            return Ok(false);
        };
        let admission = quota::QuotaControl::begin(&self.quota_control, &self.connection);
        let prepared = async {
            if !super::super::rpc_host_settings::context_promotion_enabled(&agent_dir)? {
                return Ok(None);
            }
            let path = self.sessions.args.models_config.clone().unwrap_or_else(|| agent_dir.join("models.yml"));
            let models = load_daily_config(&path, self.sessions.args.models_config.is_some())?;
            let Some(selection) = resolve_daily_promotion_selection(
                models.as_ref(),
                metadata,
                &ara_cli::schema_policy::process_daily_environment,
            )?
            else {
                return Ok(None);
            };
            // Account RPC startup is still an explicit unsupported contract.
            if selection.api == ara_cli::daily_model_config::DailyApi::OpenAiCodexResponses {
                bail!("account promotion in RPC is not yet available");
            }
            let generation = selection.generation;
            let provider = self
                .sessions
                .provider
                .prepare_daily_replacement(
                    selection,
                    self.session.header["id"].as_str().map(str::to_owned),
                    self.session.header["id"].as_str().map(str::to_owned),
                    &self.sessions.cwd,
                    &admission.cancel,
                )
                .await?;
            provider
                .route
                .check_auth(&admission.cancel)
                .await
                .map_err(|error| anyhow::anyhow!("promotion authentication unavailable: {error:?}"))?;
            let mut config = self.base_config();
            config.model = provider.route.model().clone();
            config.provider = provider.build();
            config.max_tokens = self.sessions.args.max_tokens.or(generation.max_tokens);
            config.temperature = self.sessions.args.temperature.or(generation.temperature);
            let overlay = self.overlay(&self.host_tools, &self.active_host_names()).await?;
            let retained_owner = active.sink.session.tool_owner.lock().unwrap().clone();
            let candidate = self
                .sessions
                .config(
                    &config,
                    provider.metadata.as_ref(),
                    Some(&self.skills),
                    false,
                    active.sink.session.values.clone(),
                    active.sink.session.native_session.clone(),
                    overlay,
                    Some(retained_owner),
                )
                .await?;
            Ok::<_, anyhow::Error>(Some((provider, candidate)))
        }
        .await;
        let (provider, candidate) = match prepared {
            Ok(Some(prepared)) => prepared,
            Ok(None) => return Ok(false),
            Err(_) => {
                if admission.cancel.is_cancelled() || active.cancel.is_cancelled() {
                    return Ok(false);
                }
                self.output.frame(json!({"type":"notice","level":"warning","source":"context-promotion", "message":"Context promotion target could not be prepared; trying configured compaction."}));
                return Ok(false);
            }
        };
        let super::PreparedSessionConfig { config, skills, tool_owner, context_controller, native_publication, .. } =
            candidate;
        if active.cancel.is_cancelled() || admission.cancel.is_cancelled() || self.connection.is_cancelled() {
            return Ok(false);
        }
        let entry_id = message.persisted_id(active)?;
        let mut messages = self.agent.raw_messages().await;
        let original_messages = messages.clone();
        if !message.exact_tail(&messages)? {
            return Ok(false);
        }
        messages.pop();
        // Journal selection + model receipt are a single durable transaction.
        // The prepared snapshot is then published on the same Agent/Session.
        let session = self.session.clone();
        let mut journal = session.journal.lock().await;
        if active.cancel.is_cancelled()
            || admission.cancel.is_cancelled()
            || self.connection.is_cancelled()
            || !Arc::ptr_eq(&session, &self.session)
        {
            return Ok(false);
        }
        let leaf = journal.leaf_id().map(str::to_owned);
        let parent =
            journal.entries().iter().find(|entry| entry.id == entry_id).and_then(|entry| entry.parent_id.clone());
        message.require_current(self, ContinuationKind::Overflow)?;
        message.discard_promotion(
            &mut journal,
            leaf.as_deref(),
            &entry_id,
            &format!("{}/{}", config.model.provider, config.model.id),
        )?;
        let public = public_messages_after_publication(&journal, &session, &self.connection, &mut self.drain_queues)?;
        drop(journal);
        self.agent.replace_idle_raw_messages(messages.clone())?;
        if let Err(primary) = self.publish_native_after_journal(native_publication.as_ref(), &session) {
            if let Err(secondary) = self.agent.replace_idle_raw_messages(original_messages) {
                if let Some(receipt) = session.persistence_error.lock().unwrap().as_mut() {
                    receipt.push_str(&format!("; Agent rollback failed: {secondary}"));
                }
            }
            return Err(primary);
        }
        self.maintenance_bash_transition(parent);
        *session.messages.lock().unwrap() = public;
        *session.tool_owner.lock().unwrap() = tool_owner.clone();
        let session_id = self.session.header["id"].as_str().map(str::to_owned);
        self.sessions.provider.adopt_daily_replacement(provider, session_id.clone(), session_id);
        let mut config = config;
        config.provider = self.sessions.provider.build();
        self.sessions.tool_owner = tool_owner;
        self.sessions.context_controller = context_controller;
        self.publish_snapshot(&config);
        let from = format!("{}/{}", self.config.model.provider, self.config.model.id);
        let to = format!("{}/{}", config.model.provider, config.model.id);
        self.config = config;
        self.skills = skills;
        self.terminal_recovery.incomplete_attempts = 0;
        self.finish_retry(None, Some("Retry transferred to context promotion".into()), false).await;
        self.queue_terminal_continue(active, messages, message, ContinuationKind::Overflow)?;
        self.output.frame(json!({"type":"notice","level":"info","source":"context-promotion", "message":format!("Context model promoted from {from} to {to}")}));
        Ok(true)
    }

    pub(super) async fn begin_empty_stop_recovery(
        &mut self,
        active: &ActiveRun,
        message: &AssistantMessage,
    ) -> Result<MaintenanceOutcome> {
        self.begin_empty_stop_recovery_terminal(active, &RecoveryTerminal::Legacy(message.clone())).await
    }

    pub(super) async fn begin_empty_stop_recovery_terminal(
        &mut self,
        active: &ActiveRun,
        message: &RecoveryTerminal,
    ) -> Result<MaintenanceOutcome> {
        let none = MaintenanceOutcome::default();
        let facts = message.facts_for(&self.config.model, FactsPurpose::Empty)?;
        if facts.produced_output {
            self.terminal_recovery.incomplete_attempts = 0;
        }
        let class = message.class(&self.config.model)?;
        let provider_empty = facts.stop_reason == ara_ai::StopReason::Error
            && class.error_id & ara_ai::retry_classification::flag::EMPTY_RESPONSE != 0
            && facts.provider_empty_content;
        if !facts.empty_terminal && !provider_empty {
            self.terminal_recovery.empty_attempts = 0;
            return Ok(none);
        }
        if self.input_closed
            || active.cancel.is_cancelled()
            || self.connection.is_cancelled()
            || !Arc::ptr_eq(&active.sink.session, &self.session)
            || !facts.current_route
            || class.context_recovery_blocked
            || (!provider_empty && !facts.safe_success)
        {
            return Ok(none);
        }
        let mut messages = self.agent.raw_messages().await;
        if !message.exact_tail(&messages)? {
            return Ok(none);
        }
        let entry_id = message.persisted_id(active)?;
        let session = self.session.clone();
        let mut journal = session.journal.lock().await;
        message.check_host(self)?;
        message.require_current(self, ContinuationKind::Empty)?;
        let leaf = journal.leaf_id().map(str::to_owned);
        let parent =
            journal.entries().iter().find(|entry| entry.id == entry_id).and_then(|entry| entry.parent_id.clone());
        if provider_empty {
            message.discard(&mut journal, leaf.as_deref(), &entry_id, false)?;
        } else {
            message.discard(&mut journal, leaf.as_deref(), &entry_id, true)?;
        }
        self.maintenance_bash_transition(parent);
        messages.pop();
        *session.messages.lock().unwrap() =
            public_messages_after_publication(&journal, &session, &self.connection, &mut self.drain_queues)?;
        drop(journal);
        self.terminal_recovery.empty_attempts += 1;
        if self.terminal_recovery.empty_attempts > 3 {
            self.terminal_recovery.empty_attempts = 0;
            self.agent.replace_idle_raw_messages(messages)?;
            let error = "Assistant returned no final output after three recovery attempts; try switching models or raising output limits.";
            self.finish_retry(None, Some(error.into()), false).await;
            self.output.frame(json!({"type":"notice","level":"error","source":"empty-stop", "message":error}));
            return Ok(MaintenanceOutcome { automatic_continuation_blocked: true, history_rewritten: true, ..none });
        }
        // Fixed empty-stop-retry.md. Runtime-only developer guidance, not a new
        // user source or permission restriction on the continuing Agent.
        messages.push(self.session.projector.new_legacy_message(json!({"role":"developer", "content":format!("<system-injection>\nStopped without actionable output; task incomplete. Continue with a user-visible final answer or the next required tool call.\nAttempt #{}/3\n</system-injection>", self.terminal_recovery.empty_attempts), "timestamp":ara_ai::now_ms()}), None)?);
        self.agent.replace_idle_raw_messages(messages.clone())?;
        self.queue_terminal_continue(active, messages, message, ContinuationKind::Empty)?;
        Ok(MaintenanceOutcome { continuation_scheduled: true, history_rewritten: true, ..none })
    }

    pub(super) async fn begin_unexpected_stop_recovery(
        &mut self,
        active: &ActiveRun,
        message: &AssistantMessage,
    ) -> Result<MaintenanceOutcome> {
        self.begin_unexpected_stop_recovery_terminal(active, &RecoveryTerminal::Legacy(message.clone())).await
    }

    pub(super) async fn begin_unexpected_stop_recovery_terminal(
        &mut self,
        active: &ActiveRun,
        message: &RecoveryTerminal,
    ) -> Result<MaintenanceOutcome> {
        let none = MaintenanceOutcome::default();
        if self.unexpected_stop_mode == UnexpectedStopMode::None {
            return Ok(none);
        }
        let facts = message.facts_for(&self.config.model, FactsPurpose::Unexpected)?;
        let has_text = facts.has_text;
        let has_signed_thinking = facts.signed_thinking;
        if facts.stop_reason != ara_ai::StopReason::Stop || facts.has_tools || !has_text && !has_signed_thinking {
            self.terminal_recovery.unexpected_attempts = 0;
            return Ok(none);
        }
        if has_text {
            // Mechanical never classifies delivered text. Smart requires the
            // native tiny/smol auth route or local tiny worker, still open;
            // absent a real classifier its result is unknown, never true.
            self.terminal_recovery.unexpected_attempts = 0;
            return Ok(none);
        }
        if self.input_closed
            || active.cancel.is_cancelled()
            || self.connection.is_cancelled()
            || !Arc::ptr_eq(&active.sink.session, &self.session)
            || !facts.current_route
            || !facts.safe_success
            || message.class(&self.config.model)?.context_recovery_blocked
        {
            return Ok(none);
        }
        let mut messages = self.agent.raw_messages().await;
        if !message.exact_tail(&messages)? {
            return Ok(none);
        }
        message.check_host(self)?;
        message.require_current(self, ContinuationKind::Unexpected)?;
        self.terminal_recovery.unexpected_attempts += 1;
        if self.terminal_recovery.unexpected_attempts > 3 {
            self.terminal_recovery.unexpected_attempts = 0;
            return Ok(none);
        }
        // Keep the exact signed assistant in native and model history. Only
        // the recovery guidance is runtime-only, matching unexpected-stop-retry.md.
        messages.push(self.session.projector.new_legacy_message(json!({"role":"developer", "content":format!("<system-injection>\nYou said you would continue with a tool call or action but stopped. Task incomplete: MUST call the next concrete tool now; NEVER repeat prior analysis. No tool needed? Give the final answer now.\nAttempt #{}/3\n</system-injection>\n", self.terminal_recovery.unexpected_attempts), "timestamp":ara_ai::now_ms()}), None)?);
        self.agent.replace_idle_raw_messages(messages.clone())?;
        self.queue_terminal_continue(active, messages, message, ContinuationKind::Unexpected)?;
        Ok(MaintenanceOutcome { continuation_scheduled: true, ..none })
    }
    // Caller holds the journal across the synchronous branch transition.
    // Existing jobs keep their original raw branch; later jobs get a fresh owner.
    pub(super) fn maintenance_bash_transition(&mut self, parent: Option<String>) {
        let mut current = self.bash_dispatcher.current.lock().unwrap();
        for target in self.bash_targets.iter().filter_map(Weak::upgrade) {
            let mut target = target.lock().unwrap();
            if Arc::ptr_eq(&target.session, &self.session) && matches!(target.destination, BashDestination::Current) {
                target.destination = BashDestination::Branch { parent: parent.clone() };
            }
        }
        let target =
            Arc::new(Mutex::new(BashTarget { session: self.session.clone(), destination: BashDestination::Current }));
        *current = target.clone();
        self.bash_targets.retain(|target| target.strong_count() != 0);
        self.bash_targets.push(Arc::downgrade(&target));
        self.bash_target = target;
    }

    pub(super) async fn begin_overflow_recovery(
        &mut self,
        active: &ActiveRun,
        message: &AssistantMessage,
    ) -> Result<MaintenanceOutcome> {
        self.begin_overflow_recovery_terminal(active, &RecoveryTerminal::Legacy(message.clone())).await
    }

    pub(super) async fn begin_overflow_recovery_terminal(
        &mut self,
        active: &ActiveRun,
        message: &RecoveryTerminal,
    ) -> Result<MaintenanceOutcome> {
        let result = self.begin_overflow_recovery_owned(active, message).await;
        if self.maintenance.is_none() {
            self.session.failed_recovery_message.lock().unwrap().take();
        }
        result
    }

    async fn begin_overflow_recovery_owned(
        &mut self,
        active: &ActiveRun,
        message: &RecoveryTerminal,
    ) -> Result<MaintenanceOutcome> {
        let none = MaintenanceOutcome::default();
        let facts = message.facts_for(&self.config.model, FactsPurpose::Overflow)?;
        if self.maintenance.is_some()
            || self.input_closed
            || active.cancel.is_cancelled()
            || self.connection.is_cancelled()
            || !Arc::ptr_eq(&active.sink.session, &self.session)
            || !matches!(facts.stop_reason, ara_ai::StopReason::Error | ara_ai::StopReason::Length)
            || !facts.current_route
        {
            return Ok(none);
        }
        let messages = self.agent.raw_messages().await;
        if !message.exact_tail(&messages)? || facts.has_tools {
            return Ok(none);
        }
        let class = message.class(&self.config.model)?;
        let incomplete = facts.stop_reason == ara_ai::StopReason::Length;
        if facts.produced_output {
            self.terminal_recovery.incomplete_attempts = 0;
        }
        if class.context_recovery_blocked || incomplete && !facts.safe_success {
            return Ok(none);
        }
        let window = self.config.model.context_window.filter(|window| window.is_finite() && *window > 0.0);
        let (lower, reported) = message.input_usage()?;
        let usage_overflow = window.is_some_and(|window| lower as f64 > window);
        let payload = class.error_id & ara_ai::retry_classification::flag::PAYLOAD_REJECTED != 0;
        if !incomplete && !class.overflow && !usage_overflow && !payload {
            return Ok(none);
        }
        let latest = self
            .session
            .journal
            .lock()
            .await
            .branch()
            .into_iter()
            .rev()
            .find(|entry| entry.kind == "compaction")
            .and_then(|entry| entry.raw.get("timestamp"))
            .and_then(Value::as_str)
            .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
            .map(|timestamp| timestamp.timestamp_millis());
        if latest.is_some_and(|timestamp| facts.timestamp < timestamp as f64) {
            return Ok(none);
        }
        let stored_context_tokens =
            self.budget_message_tokens(&messages, MessageCountOptions { exclude_encrypted_reasoning: true })?;
        let trusted_payload = payload
            && window.is_some_and(|window| {
                reported.map_or(!class.overflow && !usage_overflow, |reported| reported as f64 <= window)
                    && (stored_context_tokens as f64) < window * 0.9
            });
        if payload && (trusted_payload || !class.overflow && window.is_none()) {
            let mut clean = messages;
            clean.pop();
            self.agent.replace_idle_raw_messages(clean)?;
            self.output.frame(json!({"type":"notice","level":"warning","source":"compaction",
                "message":"Provider rejected request payload size; token compaction cannot establish a recovery. Reduce the payload or select another route."}));
            return Ok(MaintenanceOutcome { automatic_continuation_blocked: true, ..none });
        }
        if self.try_context_promotion(active, message).await? {
            return Ok(MaintenanceOutcome { continuation_scheduled: true, history_rewritten: true, ..none });
        }
        let settings = self.compaction_policy.recovery_settings()?;
        if !self.compaction_policy.enabled() || settings.method_order.is_empty() {
            return Ok(MaintenanceOutcome { automatic_continuation_blocked: true, ..none });
        }
        if incomplete {
            if self.terminal_recovery.incomplete_attempts >= 3 {
                self.terminal_recovery.incomplete_attempts = 0;
                let entry_id = message.persisted_id(active)?;
                let session = self.session.clone();
                let mut journal = session.journal.lock().await;
                let leaf = journal.leaf_id().map(str::to_owned);
                let parent = journal
                    .entries()
                    .iter()
                    .find(|entry| entry.id == entry_id)
                    .and_then(|entry| entry.parent_id.clone());
                message.require_current(self, ContinuationKind::Overflow)?;
                message.discard(&mut journal, leaf.as_deref(), &entry_id, false)?;
                self.maintenance_bash_transition(parent);
                let mut clean = messages;
                clean.pop();
                self.agent.replace_idle_raw_messages(clean)?;
                *session.messages.lock().unwrap() =
                    public_messages_after_publication(&journal, &session, &self.connection, &mut self.drain_queues)?;
                self.output.frame(json!({"type":"notice","level":"error","source":"compaction", "message":"Compaction recovery gave up after three consecutive empty length responses; try switching models or raising max output tokens."}));
                return Ok(MaintenanceOutcome {
                    automatic_continuation_blocked: true,
                    history_rewritten: true,
                    ..none
                });
            }
            self.terminal_recovery.incomplete_attempts += 1;
        }
        // Validate the native output budget before changing the failed turn.
        if settings.soft_available {
            summary_output_budget_tokens(settings.reserve_tokens)?;
        }
        // Explicit takeover closes the previous saga exactly once.
        if self.retry.is_some() {
            self.finish_retry(None, Some("Retry transferred to context overflow recovery".into()), false).await;
        }
        self.header_continue = None;
        self.maintenance_continue = None;
        let entry_id = message.persisted_id(active)?;
        let session = self.session.clone();
        let mut recovery = {
            let mut journal = session.journal.lock().await;
            let leaf = journal.leaf_id().context("overflow recovery has no active branch")?.to_owned();
            message.require_current(self, ContinuationKind::Overflow)?;
            let recovery = message.begin_recovery(&mut journal, &leaf, &entry_id)?;
            self.maintenance_bash_transition(Some(leaf));
            recovery
        };
        let mut clean = messages;
        *session.failed_recovery_message.lock().unwrap() = clean.pop();
        if let Err(error) = self.agent.replace_idle_raw_messages(clean.clone()) {
            self.restore_maintenance(recovery, &session).await?;
            return Err(error.into());
        }
        // Native handoff is available for incomplete output. Remote and local
        // image archives accept overflow and incomplete recovery.
        for method in &settings.method_order {
            if method == "soft" {
                break;
            }
            if method == "remote" {
                let remote =
                    Box::pin(self.remote_history(
                        None,
                        SummaryOptions { oneshot_retry: None, max_tokens: self.config.max_tokens },
                    ))
                    .await;
                match remote {
                    Ok(Some(_)) => {
                        recovery.finish(&mut *session.journal.lock().await, true)?;
                        session.failed_recovery_message.lock().unwrap().take();
                        let fits = if self.recovery_fits(settings.reserve_tokens, None, &[]).await? {
                            true
                        } else {
                            self.rescue_local_history(settings.reserve_tokens, None, false, &[], None).await?
                        };
                        if fits && !self.maintenance_stop_requested() {
                            self.queue_terminal_continue(
                                active,
                                self.agent.raw_messages().await,
                                message,
                                ContinuationKind::Overflow,
                            )?;
                            return Ok(MaintenanceOutcome {
                                continuation_scheduled: true,
                                history_rewritten: true,
                                ..none
                            });
                        }
                        return Ok(MaintenanceOutcome {
                            automatic_continuation_blocked: true,
                            history_rewritten: true,
                            ..none
                        });
                    }
                    Err(error)
                        if self.connection.is_cancelled() || ara_cli::remote_compaction::is_cancelled(&error) =>
                    {
                        // Publication errors fail-stop; an unpublished cancel
                        // returns ownership of the failed turn to its saga.
                        if !self.connection.is_cancelled() {
                            self.restore_maintenance(recovery, &session).await?;
                        }
                        return Ok(MaintenanceOutcome { automatic_continuation_blocked: true, ..none });
                    }
                    Err(_) => self.output.frame(json!({"type":"notice","level":"warning","source":"compaction",
                        "message":"Remote compaction failed; trying the next preferred method"})),
                    Ok(None) => {}
                }
                continue;
            }
            if method == "snapcompact" {
                let compacted = Box::pin(self.snapcompact_history(&[])).await;
                match compacted {
                    Ok(Some(_)) => {
                        recovery.finish(&mut *session.journal.lock().await, true)?;
                        session.failed_recovery_message.lock().unwrap().take();
                        let fits = if self.recovery_fits(settings.reserve_tokens, None, &[]).await? {
                            true
                        } else {
                            self.rescue_local_history(settings.reserve_tokens, None, false, &[], None).await?
                        };
                        if fits && !self.maintenance_stop_requested() {
                            self.queue_terminal_continue(
                                active,
                                self.agent.raw_messages().await,
                                message,
                                ContinuationKind::Overflow,
                            )?;
                            return Ok(MaintenanceOutcome {
                                continuation_scheduled: true,
                                history_rewritten: true,
                                ..none
                            });
                        }
                        return Ok(MaintenanceOutcome {
                            automatic_continuation_blocked: true,
                            history_rewritten: true,
                            ..none
                        });
                    }
                    Err(error) if self.maintenance_stop_requested() || ara_cli::handoff::is_cancelled(&error) => {
                        if !self.connection.is_cancelled() {
                            self.restore_maintenance(recovery, &session).await?;
                        }
                        return Ok(MaintenanceOutcome { automatic_continuation_blocked: true, ..none });
                    }
                    Err(error) => self.output.frame(json!({"type":"notice","level":"warning","source":"compaction",
                        "message":format!("Snapcompact unavailable ({error}); trying the next preferred method")})),
                    Ok(None) => {}
                }
                continue;
            }
            if method == "handoff" && incomplete {
                let handoff = Box::pin(self.handoff_local_history(Some(ara_cli::handoff::AUTO_FOCUS), true)).await;
                match handoff {
                    Ok(Some(_)) => {
                        recovery.finish(&mut *session.journal.lock().await, true)?;
                        session.failed_recovery_message.lock().unwrap().take();
                        let fits = if self.recovery_fits(settings.reserve_tokens, None, &[]).await? {
                            true
                        } else {
                            self.rescue_local_history(settings.reserve_tokens, None, false, &[], None).await?
                        };
                        if fits {
                            self.queue_terminal_continue(
                                active,
                                self.agent.raw_messages().await,
                                message,
                                ContinuationKind::Overflow,
                            )?;
                            return Ok(MaintenanceOutcome {
                                continuation_scheduled: true,
                                history_rewritten: true,
                                ..none
                            });
                        }
                        return Ok(MaintenanceOutcome {
                            automatic_continuation_blocked: true,
                            history_rewritten: true,
                            ..none
                        });
                    }
                    Err(error) if self.connection.is_cancelled() || ara_cli::handoff::is_cancelled(&error) => {
                        if !self.connection.is_cancelled() {
                            self.restore_maintenance(recovery, &session).await?;
                        }
                        return Ok(MaintenanceOutcome { automatic_continuation_blocked: true, ..none });
                    }
                    Err(_) => {
                        self.output.frame(json!({"type":"notice","level":"warning","source":"compaction",
                            "message":"Handoff compaction failed; trying the next preferred method"}));
                    }
                    Ok(None) => {}
                }
                continue;
            }
            if method != "shake" {
                continue;
            }
            let reduced = self.shake_local_history(false, Some(recovery.transaction_mut())).await;
            match reduced {
                Ok(true) if self.recovery_fits(settings.reserve_tokens, None, &[]).await? => {
                    recovery.finish(&mut *session.journal.lock().await, true)?;
                    session.failed_recovery_message.lock().unwrap().take();
                    let messages = self.agent.raw_messages().await;
                    self.queue_terminal_continue(active, messages, message, ContinuationKind::Overflow)?;
                    self.output.frame(json!({"type":"notice","level":"info","source":"compaction", "message":"Local shake restored the retry context; original output is available through artifact recovery."}));
                    return Ok(MaintenanceOutcome { continuation_scheduled: true, history_rewritten: true, ..none });
                }
                Err(error) => {
                    if !self.connection.is_cancelled() {
                        self.restore_maintenance(recovery, &session).await?;
                    }
                    return Err(error);
                }
                _ => {}
            }
        }
        // Tier zero can publish a new compaction entry while the failed turn
        // is provisionally detached. Settle that exact owner once before the
        // lower tiers perform raw edits; never rebase an ownership receipt.
        let frame_rescue = self.rescue_snapcompact_frames(settings.reserve_tokens, None, &[]).await;
        let frame_rescue = match frame_rescue {
            Ok(result) => result,
            Err(error) => {
                if !self.connection.is_cancelled() {
                    self.restore_maintenance(recovery, &session).await?;
                }
                return Err(error);
            }
        };
        if let Some(entry_id) = frame_rescue {
            recovery.finish(&mut *session.journal.lock().await, true)?;
            session.failed_recovery_message.lock().unwrap().take();
            let fits = if self.recovery_fits(settings.reserve_tokens, None, &[]).await? {
                true
            } else {
                self.rescue_local_history(settings.reserve_tokens, None, false, &[], None).await?
            };
            if fits && !self.maintenance_stop_requested() {
                self.queue_terminal_continue(
                    active,
                    self.agent.raw_messages().await,
                    message,
                    ContinuationKind::Overflow,
                )?;
                return Ok(MaintenanceOutcome { continuation_scheduled: true, history_rewritten: true, ..none });
            }
            self.stamp_frame_dead_end(&entry_id, true).await?;
            return Ok(MaintenanceOutcome { automatic_continuation_blocked: true, history_rewritten: true, ..none });
        }
        if !settings.soft_available {
            let rescued = self
                .rescue_local_history(
                    settings.reserve_tokens,
                    None,
                    settings.method_order.iter().any(|method| method == "shake"),
                    &[],
                    Some(recovery.transaction_mut()),
                )
                .await;
            let rescued = match rescued {
                Ok(rescued) => rescued,
                Err(error) => {
                    if !self.connection.is_cancelled() {
                        self.restore_maintenance(recovery, &session).await?;
                    }
                    return Err(error);
                }
            };
            if rescued {
                recovery.finish(&mut *session.journal.lock().await, true)?;
                session.failed_recovery_message.lock().unwrap().take();
                self.queue_terminal_continue(
                    active,
                    self.agent.raw_messages().await,
                    message,
                    ContinuationKind::Overflow,
                )?;
                return Ok(MaintenanceOutcome { continuation_scheduled: true, history_rewritten: true, ..none });
            }
            self.restore_maintenance(recovery, &session).await?;
            return Ok(MaintenanceOutcome { automatic_continuation_blocked: true, history_rewritten: true, ..none });
        }
        clean = self.agent.raw_messages().await;
        let preparation = session.journal.lock().await.native_projected_compaction_snapshot();
        let prepared = (|| -> Result<_> {
            let snapshot = preparation?;
            let sources = ara_cli::native_compaction::sources_for_model(&snapshot.entries, &self.config.model);
            let previous =
                snapshot.previous_summary.as_ref().map(|summary| summary.previous_summary_for_text_compaction());
            let previous = previous.as_deref();
            let cut = select_native_entry_compaction_cut(&sources, self.sessions.args.compact_keep_tokens, previous)?
                .context("No native message prefix can be compacted with the current keep-token budget")?;
            Ok((snapshot, cut))
        })();
        let (snapshot, cut) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.restore_maintenance(recovery, &session).await?;
                self.output.frame(json!({"type":"notice","level":"warning","source":"compaction",
                    "message":format!("Context overflow recovery failed: {}", super::super::sanitize_text(&error.to_string()))}));
                return Ok(MaintenanceOutcome { automatic_continuation_blocked: true, ..none });
            }
        };
        let cancel = self.connection.child_token();
        let task_cancel = cancel.clone();
        let task_snapshot = snapshot.clone();
        let model = self.config.model.clone();
        let provider = self.config.provider.clone();
        let deadline = Instant::now() + Duration::from_secs_f64(self.max_time.unwrap_or(120.0).clamp(0.0, 120.0));
        let first_kept = cut.first_kept_entry_id.clone();
        let reserve_tokens = settings.reserve_tokens;
        let host_cap = self.config.max_tokens;
        let task = tokio::spawn(async move {
            let sources = ara_cli::native_compaction::sources_for_model(&task_snapshot.entries, &model);
            summarize_native_entry_compaction_cut(
                &sources,
                &cut,
                task_snapshot.previous_summary.as_ref().map(|s| s.previous_summary_for_text_compaction()).as_deref(),
                None,
                &model,
                provider.as_ref(),
                reserve_tokens,
                SummaryOptions { oneshot_retry: None, max_tokens: host_cap },
                deadline,
                &task_cancel,
            )
            .await
            .map_err(|error| anyhow::anyhow!("Context overflow recovery failed: {error}"))
        });
        self.maintenance = Some(ActiveMaintenance {
            task,
            cancel,
            session,
            generation: self.prompt_generation,
            command: Command {
                id: active.command.id.clone(),
                kind: active.command.kind.clone(),
                frame: active.command.frame.clone(),
            },
            snapshot,
            first_kept,
            tokens_before: self
                .budget_message_tokens(&clean, MessageCountOptions { exclude_encrypted_reasoning: true })?
                as u64,
            expected_messages: if message.actual().is_some() {
                Vec::new()
            } else {
                self.session.projector.current_body_receipts(&clean)?
            },
            recovery,
            native_context: message.context_receipt(&clean)?,
            native_guard: NativeContinuationGuard::for_terminal(message, ContinuationKind::Overflow),
            reserve_tokens: settings.reserve_tokens,
        });
        self.is_compacting = true;
        self.auto_compaction_pending = false;
        self.output.frame(json!({"type":"auto_compaction_start","reason":if incomplete { "incomplete" } else { "overflow" },"action":"context-full"}));
        Ok(MaintenanceOutcome { continuation_scheduled: true, ..none })
    }

    async fn restore_maintenance(&mut self, recovery: impl Into<RecoveryToken>, session: &Arc<Session>) -> Result<()> {
        let mut journal = session.journal.lock().await;
        let recovery = recovery.into();
        let rewritten = recovery.rewritten();
        let outcome = recovery.finish(&mut journal, rewritten)?;
        let mut public =
            public_messages_after_publication(&journal, session, &self.connection, &mut self.drain_queues)?;
        let retained = session.failed_recovery_message.lock().unwrap().take();
        if let Some((recovery_body, recovery_source, appended_entry_id, legacy_message)) = outcome {
            let raw = match retained {
                Some(raw) => raw,
                None => session.projector.message_from_body(recovery_body, recovery_source)?,
            };
            if let Some(id) = appended_entry_id {
                session.projector.record_entry_id(&raw, id);
            } else {
                public.push(
                    session.projector.public_event_wire(&AgentEvent::MessageEnd { message: raw.clone() }, true)?.get("message")
                        .cloned().unwrap_or(WireValue::Null),
                );
            }
            let selected = self.route_raw_context(&journal)?;
            let current = self.agent.raw_messages().await;
            let mut messages = session.projector.reconcile_owned_context(&current, selected)?;
            let duplicate = if let Some(message) = legacy_message.as_ref() {
                raw_last_assistant(&messages)?.as_ref() == Some(message.as_ref())
            } else {
                messages.last().is_some_and(|tail| tail.same_record(&raw)
                    || matches!((tail.body_snapshot(), raw.body_snapshot()),
                        (ara_agent::AgentMessageBody::Graph(current), ara_agent::AgentMessageBody::Graph(restored)) if current.same_identity(&restored)))
            };
            if duplicate {
                messages.pop();
            }
            messages.push(raw);
            self.agent.replace_idle_raw_messages(messages)?;
        }
        // Empty errors remain visible even when excluded from model replay.
        *session.messages.lock().unwrap() = public;
        Ok(())
    }

    pub(super) async fn completed_maintenance(
        &mut self,
        active: ActiveMaintenance,
        result: std::result::Result<Result<AcceptedSummary>, tokio::task::JoinError>,
    ) {
        let owner = Arc::ptr_eq(&active.session, &self.session) && active.generation == self.prompt_generation;
        let cancelled = active.cancel.is_cancelled() || self.connection.is_cancelled() || !owner;
        let result = match result {
            Ok(result) if !cancelled => result,
            Ok(_) => Err(anyhow::anyhow!("Context overflow recovery cancelled")),
            Err(error) => Err(anyhow::anyhow!("Context overflow recovery task failed: {error}")),
        };
        let mut committed = false;
        let result = async {
            let accepted = result?;
            if active.native_context.is_none()
                && self.session.projector.current_body_receipts(&self.agent.raw_messages().await)?
                    != active.expected_messages
            {
                bail!("Context changed during overflow recovery");
            }
            if let Some(receipt) = &active.native_context {
                let messages = self.agent.raw_messages().await;
                receipt
                    .recheck(&messages, active.session.values.as_ref().context("Native maintenance values missing")?)?;
            }
            if let Some(guard) = &active.native_guard {
                guard.recheck(self)?;
            }
            let mut journal = active.session.journal.lock().await;
            if let Err(error) = journal.commit_native_entry_compaction(
                &active.snapshot,
                &accepted.text,
                &active.first_kept,
                &accepted.window_source_entry_ids,
                active.tokens_before,
            ) {
                if matches!(error, ara_session::CompactionCommitError::Storage(_)) {
                    *active.session.persistence_error.lock().unwrap() = Some(error.to_string());
                    self.connection.cancel();
                }
                return Err(error.into());
            }
            committed = true;
            if let Err(error) = self.adopt_route_context(&journal).await {
                *active.session.persistence_error.lock().unwrap() = Some(error.to_string());
                self.connection.cancel();
                return Err(error);
            }
            *active.session.messages.lock().unwrap() =
                public_messages_after_publication(&journal, &active.session, &self.connection, &mut self.drain_queues)?;
            self.config.provider = self.sessions.provider.build();
            if let Some(controller) = &self.sessions.context_controller {
                controller.fresh_provider_session();
            }
            self.publish_snapshot(&self.config);
            Ok::<_, anyhow::Error>(accepted)
        }
        .await;
        let finalized = if committed {
            active.session.failed_recovery_message.lock().unwrap().take();
            active.recovery.finish(&mut *active.session.journal.lock().await, false).map(|_| ())
        } else {
            self.restore_maintenance(active.recovery, &active.session).await
        };
        if let Err(error) = finalized {
            active.session.persistence_error.lock().unwrap().get_or_insert_with(|| error.to_string());
            self.connection.cancel();
            self.drain_queues = false;
        }
        active.session.failed_recovery_message.lock().unwrap().take();
        let publication_error = active.session.persistence_error.lock().unwrap().clone();
        let result = match (result, publication_error) {
            (Ok(_), Some(error)) => Err(anyhow::anyhow!(error)),
            (result, _) => result,
        };
        self.is_compacting = false;
        self.auto_compaction_pending = false;
        let mut event =
            json!({"type":"auto_compaction_end","action":"context-full","aborted":cancelled,"willRetry":false});
        match result {
            Ok(accepted) => {
                event["result"] = json!({"summary":accepted.text,"firstKeptEntryId":active.first_kept,"tokensBefore":active.tokens_before});
                let continuation: Result<(bool, Vec<Value>, Option<NativeRetryContextReceipt>)> = async {
                    let fits = if self.recovery_fits(active.reserve_tokens, None, &[]).await? {
                        true
                    } else {
                        self.rescue_local_history(active.reserve_tokens, None, false, &[], None).await?
                    };
                    let messages = self.agent.raw_messages().await;
                    Ok((
                        fits,
                        if active.native_context.is_some() {
                            Vec::new()
                        } else {
                            self.session.projector.current_body_receipts(&messages)?
                        },
                        if active.native_guard.is_some() {
                            Some(NativeRetryContextReceipt::capture(
                                &messages,
                                self.session.values.clone().context("Native maintenance values missing")?,
                            )?)
                        } else {
                            None
                        },
                    ))
                }
                .await;
                let (fits, expected_messages, continuation_native_context) = match continuation {
                    Ok(continuation) => continuation,
                    Err(error) => {
                        event["errorMessage"] = json!(super::super::sanitize_text(&error.to_string()));
                        self.drain_queues = false;
                        self.output.response(&active.command, None, Some(error.to_string()));
                        self.output.frame(event);
                        return;
                    }
                };
                if !cancelled && !self.connection.is_cancelled() && fits {
                    self.maintenance_continue = Some(PendingMaintenanceContinue {
                        session: active.session,
                        generation: active.generation,
                        command: active.command,
                        expected_messages,
                        reserve_tokens: active.reserve_tokens,
                        check_fit: true,
                        native_context: continuation_native_context,
                        native_guard: active.native_guard,
                        deadline: Instant::now() + Duration::from_millis(100),
                    });
                    event["willRetry"] = json!(true);
                } else {
                    self.output.frame(json!({"type":"notice","level":"warning","source":"compaction",
                        "message":"Compaction could not free enough context. Clear large tool output or switch to a larger-context model."}));
                    self.drain_queues =
                        !cancelled && !self.connection.is_cancelled() && self.agent.has_queued_messages();
                }
            }
            Err(error) => {
                event["errorMessage"] = json!(super::super::sanitize_text(&error.to_string()));
                self.drain_queues = !cancelled && !self.connection.is_cancelled() && self.agent.has_queued_messages();
                self.output.response(&active.command, None, Some(error.to_string()));
            }
        }
        self.output.frame(event);
    }

    pub(super) async fn abort_maintenance(&mut self) {
        self.maintenance_continue = None;
        if let Some(mut active) = self.maintenance.take() {
            active.cancel.cancel();
            let result = (&mut active.task).await;
            // Settlement now contains tiered local rescue. Keep its future
            // out of the serial abort/new/switch state on Windows' main stack.
            Box::pin(self.completed_maintenance(active, result)).await;
        }
    }

    pub(super) async fn resume_maintenance_continue(&mut self) {
        let Some(pending) = self.maintenance_continue.take() else {
            return;
        };
        if self.input_closed
            || self.maintenance_stop_requested()
            || pending.generation != self.prompt_generation
            || !Arc::ptr_eq(&pending.session, &self.session)
        {
            self.drain_queues = false;
            return;
        }
        let current_messages = self.agent.raw_messages().await;
        let receipts = if pending.native_context.is_some() {
            Vec::new()
        } else {
            let receipts = match self.session.projector.current_body_receipts(&current_messages) {
                Ok(receipts) => receipts,
                Err(error) => {
                    self.drain_queues = false;
                    self.output.response(&pending.command, None, Some(error.to_string()));
                    return;
                }
            };
            receipts
        };
        let native_check = (|| -> Result<()> {
            if let Some(receipt) = &pending.native_context {
                receipt.recheck(
                    &current_messages,
                    pending.session.values.as_ref().context("Native maintenance values missing")?,
                )?;
            }
            if let Some(guard) = &pending.native_guard {
                guard.recheck(self)?;
            }
            Ok(())
        })();
        if let Err(error) = native_check {
            self.drain_queues = false;
            self.output.response(&pending.command, None, Some(error.to_string()));
            return;
        }
        if receipts != pending.expected_messages {
            self.drain_queues = false;
            return;
        }
        // Explicit queued input takes ownership ahead of the automatic retry.
        if self.agent.has_queued_messages() {
            self.drain_queues = true;
            self.reconcile_queues().await;
        } else if let Err(error) =
            self.start_after_maintenance(pending.command, pending.check_fit, pending.reserve_tokens).await
        {
            self.drain_queues = false;
            self.output
                .frame(json!({"type":"notice","level":"warning","source":"compaction","message":error.to_string()}));
        }
    }

    pub(super) fn extend_maintenance_context(&mut self, session: &Arc<Session>, message: &AgentMessage) -> Result<()> {
        if let Some(pending) = &mut self.maintenance_continue
            && pending.generation == self.prompt_generation
            && Arc::ptr_eq(&pending.session, session)
        {
            if let Some(receipt) = &mut pending.native_context {
                receipt.append(message)?;
            } else {
                pending
                    .expected_messages
                    .extend(session.projector.current_body_receipts(std::slice::from_ref(message))?);
            }
        }
        Ok(())
    }
}

// Append inside rpc_host_maintenance.rs's existing @bin:ara module. The
// descendant can call the private method without widening production access.
#[cfg(test)]
mod raw_messages_r_rollback_group {
    use super::*;
    use ara_ai::AssistantBlock;

    #[tokio::test]
    async fn actual_raw_native_rollback_restores_same_record_and_extensions_group() {
        let (mut host, _) = super::super::tests::fixture();
        let mut failed =
            AssistantMessage::empty(&host.config.model.api, &host.config.model.provider, &host.config.model.id);
        failed.stop_reason = ara_ai::StopReason::Error;
        failed.error_message = Some("source fixture failure".into());
        failed.content.push(AssistantBlock::text("retained failed output"));
        let mut body = serde_json::to_value(Message::Assistant(failed.clone())).unwrap();
        body["application"] = json!({"rollback":"preserve"});
        body["errorId"] = json!(0x0002_1000_u32);
        let raw_failed = AgentMessage::from_raw(body.clone(), None).unwrap();
        host.agent.replace_idle_raw_messages(vec![raw_failed.clone()]).unwrap();
        let recovery = {
            let mut journal = host.session.journal.lock().await;
            let id = host.session.projector.append_current_body(&mut journal, &raw_failed).unwrap();
            let leaf = journal.leaf_id().unwrap().to_owned();
            journal.begin_failed_assistant_recovery(&leaf, &id, &failed).unwrap()
        };
        *host.session.failed_recovery_message.lock().unwrap() = Some(raw_failed.clone());
        let retained_session = host.session.clone();
        host.restore_maintenance(recovery, &retained_session).await.unwrap();
        assert!(host.agent.raw_messages().await.last().unwrap().same_record(&raw_failed));
        assert!(host.session.failed_recovery_message.lock().unwrap().is_none());
        assert_eq!(host.session.journal.lock().await.branch().last().unwrap().raw["message"], body);
    }
}
