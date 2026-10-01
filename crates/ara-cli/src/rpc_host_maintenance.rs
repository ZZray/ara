//! Fixed OMP 596f2da same-route soft overflow recovery. Other native methods,
//! Other compaction methods and dead-end rescue remain required parity work.
use super::*;
use ara_agent::compaction::{
    AcceptedSummary, SummarySource, select_whole_turn_cut, summarize_sources_with_instructions,
    summary_output_budget_tokens,
};
use ara_agent::tokenizer::{MessageCountOptions, count_messages};
use ara_session::{FailedAssistantRecovery, FailedAssistantRecoveryOutcome, ProjectedCompactionSnapshot};

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
    snapshot: ProjectedCompactionSnapshot,
    first_kept: String,
    tokens_before: u64,
    expected_messages: Vec<Message>,
    recovery: FailedAssistantRecovery,
    reserve_tokens: Option<f64>,
}

pub(super) struct PendingMaintenanceContinue {
    session: Arc<Session>,
    generation: u64,
    command: Command,
    expected_messages: Vec<Message>,
    reserve_tokens: Option<f64>,
    check_fit: bool,
    pub deadline: Instant,
}

#[derive(Default)]
pub(super) struct TerminalRecoveryState {
    pub incomplete_attempts: usize,
    pub empty_attempts: usize,
}

fn safe_success_terminal(message: &AssistantMessage, api: &str) -> bool {
    !matches!(api, "openai-responses" | "openai-codex-responses")
        || message.terminal_context_recovery == Some(ara_ai::ContextRecoveryEvidence::ContentOnly)
}

fn produced_output(message: &AssistantMessage) -> bool {
    if matches!(message.stop_reason, ara_ai::StopReason::Error | ara_ai::StopReason::Aborted) {
        return false;
    }
    let mut content = message.clone();
    content.stop_reason = ara_ai::StopReason::Stop;
    !ara_session::is_empty_assistant_stop(&content)
}

fn stored_tokens(messages: &[Message]) -> usize {
    count_messages(messages, MessageCountOptions { exclude_encrypted_reasoning: true })
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

fn retry_fit(window: Option<f64>, reserve: Option<f64>, messages: &[Message]) -> bool {
    let Some(window) = window.filter(|window| window.is_finite() && *window > 0.0) else { return true };
    // Fixed agent compaction.ts:303-333. Preserve explicit/default provenance.
    let proportional = (window * 0.15).floor().max(1.0);
    let effective = (window * 0.15).floor().max(reserve.unwrap_or(16_384.0));
    let resolved = if reserve.is_none() && effective >= window - proportional || effective >= window {
        proportional
    } else {
        effective
    };
    stored_tokens(messages) as f64 <= (window - resolved).max(0.0)
}

impl Host {
    fn queue_terminal_continue(&mut self, active: &ActiveRun, messages: Vec<Message>) {
        self.maintenance_continue = Some(PendingMaintenanceContinue {
            session: self.session.clone(),
            generation: self.prompt_generation,
            command: Command {
                id: active.command.id.clone(),
                kind: active.command.kind.clone(),
                frame: active.command.frame.clone(),
            },
            expected_messages: messages,
            reserve_tokens: None,
            check_fit: false,
            deadline: Instant::now() + Duration::from_millis(100),
        });
        self.auto_compaction_pending = false;
    }

    async fn try_context_promotion(&mut self, active: &ActiveRun, message: &AssistantMessage) -> Result<bool> {
        use ara_cli::daily_model_config::{load_daily_config, resolve_daily_promotion_selection};
        let agent_dir = super::super::ara_home().join("agent");
        let Some(metadata) = self.sessions.provider.metadata.as_ref() else { return Ok(false) };
        let prepared = async {
            if !super::super::rpc_host_settings::context_promotion_enabled(&agent_dir)? {
                return Ok(None);
            }
            let path = self.sessions.args.models_config.clone().unwrap_or_else(|| agent_dir.join("models.yml"));
            let models = load_daily_config(&path, self.sessions.args.models_config.is_some())?;
            let Some(selection) =
                resolve_daily_promotion_selection(models.as_ref(), metadata, &|name: &str| std::env::var(name).ok())?
            else {
                return Ok(None);
            };
            // Account RPC startup is still an explicit unsupported contract.
            if selection.api == ara_cli::daily_model_config::DailyApi::OpenAiCodexResponses {
                bail!("account promotion in RPC is not yet available");
            }
            let generation = selection.generation;
            let provider = super::super::ProviderFactory::daily(
                self.sessions.provider.client.clone(),
                selection,
                self.session.header["id"].as_str().map(str::to_owned),
            )
            .await?;
            provider
                .route
                .check_auth(&active.cancel)
                .await
                .map_err(|error| anyhow::anyhow!("promotion authentication unavailable: {error:?}"))?;
            let mut config = self.base_config();
            config.model = provider.route.model().clone();
            config.provider = provider.build();
            config.max_tokens = self.sessions.args.max_tokens.or(generation.max_tokens);
            config.temperature = self.sessions.args.temperature.or(generation.temperature);
            let overlay = self.overlay(&self.host_tools, &self.active_host_names());
            let (config, skills) = self.sessions.config(&config, false, overlay).await?;
            Ok::<_, anyhow::Error>(Some((provider, config, skills)))
        }
        .await;
        let (provider, config, skills) = match prepared {
            Ok(Some(prepared)) => prepared,
            Ok(None) => return Ok(false),
            Err(_) => {
                self.output.frame(json!({"type":"notice","level":"warning","source":"context-promotion", "message":"Context promotion target could not be prepared; trying configured compaction."}));
                return Ok(false);
            }
        };
        if active.cancel.is_cancelled() || self.connection.is_cancelled() {
            return Ok(false);
        }
        let entry_id = persisted_assistant_id(active, message)?;
        let mut messages = self.agent.messages().await;
        if messages.last().and_then(Message::as_assistant) != Some(message) {
            return Ok(false);
        }
        messages.pop();
        // Journal selection + model receipt are a single durable transaction.
        // The prepared snapshot is then published on the same Agent/Session.
        let session = self.session.clone();
        let mut journal = session.journal.lock().await;
        let leaf = journal.leaf_id().map(str::to_owned);
        let parent =
            journal.entries().iter().find(|entry| entry.id == entry_id).and_then(|entry| entry.parent_id.clone());
        journal.discard_failed_assistant_for_promotion(
            leaf.as_deref(),
            &entry_id,
            message,
            &format!("{}/{}", config.model.provider, config.model.id),
        )?;
        self.maintenance_bash_transition(parent);
        self.agent.replace_idle_messages(messages.clone())?;
        *session.messages.lock().unwrap() = Session::public_messages(&journal);
        drop(journal);
        self.publish_snapshot(&config);
        let from = format!("{}/{}", self.config.model.provider, self.config.model.id);
        let to = format!("{}/{}", config.model.provider, config.model.id);
        self.config = config;
        self.skills = skills;
        self.sessions.provider = provider;
        self.terminal_recovery.incomplete_attempts = 0;
        self.finish_retry(None, Some("Retry transferred to context promotion".into()), false).await;
        self.queue_terminal_continue(active, messages);
        self.output.frame(json!({"type":"notice","level":"info","source":"context-promotion", "message":format!("Context model promoted from {from} to {to}")}));
        Ok(true)
    }

    pub(super) async fn begin_empty_stop_recovery(
        &mut self,
        active: &ActiveRun,
        message: &AssistantMessage,
    ) -> Result<MaintenanceOutcome> {
        let none = MaintenanceOutcome::default();
        let api = &self.config.model.api;
        if produced_output(message) {
            self.terminal_recovery.incomplete_attempts = 0;
        }
        let class = ara_ai::retry_classification::classify_retry(message, api);
        let provider_empty = message.stop_reason == ara_ai::StopReason::Error
            && class.error_id & ara_ai::retry_classification::flag::EMPTY_RESPONSE != 0
            && message.content.iter().all(|block| {
                matches!(block, ara_ai::AssistantBlock::Thinking(_))
                    || matches!(block, ara_ai::AssistantBlock::Text(text) if text.text.trim().is_empty())
            });
        if !ara_session::is_empty_assistant_stop(message) && !provider_empty {
            self.terminal_recovery.empty_attempts = 0;
            return Ok(none);
        }
        if self.input_closed
            || active.cancel.is_cancelled()
            || self.connection.is_cancelled()
            || !Arc::ptr_eq(&active.sink.session, &self.session)
            || message.provider != self.config.model.provider
            || message.model != self.config.model.id
            || class.context_recovery_blocked
            || (!provider_empty && !safe_success_terminal(message, api))
        {
            return Ok(none);
        }
        let mut messages = self.agent.messages().await;
        if messages.last().and_then(Message::as_assistant) != Some(message) {
            return Ok(none);
        }
        let entry_id = persisted_assistant_id(active, message)?;
        let session = self.session.clone();
        let mut journal = session.journal.lock().await;
        let leaf = journal.leaf_id().map(str::to_owned);
        let parent =
            journal.entries().iter().find(|entry| entry.id == entry_id).and_then(|entry| entry.parent_id.clone());
        if provider_empty {
            journal.discard_entry_durably(leaf.as_deref(), &entry_id, message)?;
        } else {
            journal.discard_empty_stop_durably(leaf.as_deref(), &entry_id, message)?;
        }
        self.maintenance_bash_transition(parent);
        messages.pop();
        *session.messages.lock().unwrap() = Session::public_messages(&journal);
        drop(journal);
        self.terminal_recovery.empty_attempts += 1;
        if self.terminal_recovery.empty_attempts > 3 {
            self.terminal_recovery.empty_attempts = 0;
            self.agent.replace_idle_messages(messages)?;
            let error = "Assistant returned no final output after three recovery attempts; try switching models or raising output limits.";
            self.finish_retry(None, Some(error.into()), false).await;
            self.output.frame(json!({"type":"notice","level":"error","source":"empty-stop", "message":error}));
            return Ok(MaintenanceOutcome { automatic_continuation_blocked: true, history_rewritten: true, ..none });
        }
        // Fixed empty-stop-retry.md. Runtime-only developer guidance, not a new
        // user source or permission restriction on the continuing Agent.
        messages.push(Message::Developer(ara_ai::DeveloperMessage {
            content: UserContent::Text(format!("<system-injection>\nStopped without actionable output; task incomplete. Continue with a user-visible final answer or the next required tool call.\nAttempt #{}/3\n</system-injection>", self.terminal_recovery.empty_attempts)),
            timestamp: ara_ai::now_ms(),
        }));
        self.agent.replace_idle_messages(messages.clone())?;
        self.queue_terminal_continue(active, messages);
        Ok(MaintenanceOutcome { continuation_scheduled: true, history_rewritten: true, ..none })
    }
    // Caller holds the journal across the synchronous branch transition.
    // Existing jobs keep their original raw branch; later jobs get a fresh owner.
    fn maintenance_bash_transition(&mut self, parent: Option<String>) {
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
        let none = MaintenanceOutcome::default();
        if self.maintenance.is_some()
            || self.input_closed
            || active.cancel.is_cancelled()
            || self.connection.is_cancelled()
            || !Arc::ptr_eq(&active.sink.session, &self.session)
            || !matches!(message.stop_reason, ara_ai::StopReason::Error | ara_ai::StopReason::Length)
            || message.provider != self.config.model.provider
            || message.model != self.config.model.id
        {
            return Ok(none);
        }
        let messages = self.agent.messages().await;
        if messages.last().and_then(Message::as_assistant) != Some(message) || message.tool_calls().next().is_some() {
            return Ok(none);
        }
        let class = ara_ai::retry_classification::classify_retry(message, &self.config.model.api);
        let incomplete = message.stop_reason == ara_ai::StopReason::Length;
        if produced_output(message) {
            self.terminal_recovery.incomplete_attempts = 0;
        }
        if class.context_recovery_blocked || incomplete && !safe_success_terminal(message, &self.config.model.api) {
            return Ok(none);
        }
        let window = self.config.model.context_window.filter(|window| window.is_finite() && *window > 0.0);
        let (lower, reported) = reported_input(message);
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
        if latest.is_some_and(|timestamp| message.timestamp < timestamp) {
            return Ok(none);
        }
        let trusted_payload = payload
            && window.is_some_and(|window| {
                reported.map_or(!class.overflow && !usage_overflow, |reported| reported as f64 <= window)
                    && (stored_tokens(&messages) as f64) < window * 0.9
            });
        if payload && (trusted_payload || !class.overflow && window.is_none()) {
            let mut clean = messages;
            clean.pop();
            self.agent.replace_idle_messages(clean)?;
            self.output.frame(json!({"type":"notice","level":"warning","source":"compaction",
                "message":"Provider rejected request payload size; token compaction cannot establish a recovery. Reduce the payload or select another route."}));
            return Ok(MaintenanceOutcome { automatic_continuation_blocked: true, ..none });
        }
        if self.try_context_promotion(active, message).await? {
            return Ok(MaintenanceOutcome { continuation_scheduled: true, history_rewritten: true, ..none });
        }
        let settings = self.compaction_policy.recovery_settings()?;
        if !self.compaction_policy.enabled() || !settings.soft_available {
            return Ok(MaintenanceOutcome { automatic_continuation_blocked: true, ..none });
        }
        if incomplete {
            if self.terminal_recovery.incomplete_attempts >= 3 {
                self.terminal_recovery.incomplete_attempts = 0;
                let entry_id = persisted_assistant_id(active, message)?;
                let session = self.session.clone();
                let mut journal = session.journal.lock().await;
                let leaf = journal.leaf_id().map(str::to_owned);
                let parent = journal
                    .entries()
                    .iter()
                    .find(|entry| entry.id == entry_id)
                    .and_then(|entry| entry.parent_id.clone());
                journal.discard_entry_durably(leaf.as_deref(), &entry_id, message)?;
                self.maintenance_bash_transition(parent);
                let mut clean = messages;
                clean.pop();
                self.agent.replace_idle_messages(clean)?;
                *session.messages.lock().unwrap() = Session::public_messages(&journal);
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
        let summary_budget = summary_output_budget_tokens(settings.reserve_tokens)?;
        let max_tokens = self.config.max_tokens.map_or(summary_budget, |cap| cap.min(summary_budget));
        // Explicit takeover closes the previous saga exactly once.
        if self.retry.is_some() {
            self.finish_retry(None, Some("Retry transferred to context overflow recovery".into()), false).await;
        }
        self.header_continue = None;
        self.maintenance_continue = None;
        let entry_id = active
            .sink
            .entries
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|(id, candidate)| (candidate.as_assistant() == Some(message)).then(|| id.clone()))
            .context("overflow assistant has no persisted native entry ID")?;
        let session = self.session.clone();
        let (recovery, preparation) = {
            let mut journal = session.journal.lock().await;
            let leaf = journal.leaf_id().context("overflow recovery has no active branch")?.to_owned();
            let recovery = journal.begin_failed_assistant_recovery(&leaf, &entry_id, message)?;
            self.maintenance_bash_transition(Some(leaf));
            let preparation = journal.projected_compaction_snapshot();
            (recovery, preparation)
        };
        let mut clean = messages;
        clean.pop();
        if let Err(error) = self.agent.replace_idle_messages(clean.clone()) {
            self.restore_maintenance(recovery, &session).await?;
            return Err(error.into());
        }
        let prepared = (|| -> Result<_> {
            let snapshot = preparation?;
            let sources = snapshot
                .messages
                .iter()
                .map(|m| SummarySource { entry_id: &m.entry_id, message: &m.message })
                .collect::<Vec<_>>();
            let previous = snapshot.previous_summary.as_ref().map(|summary| summary.summary.as_str());
            let cut = select_whole_turn_cut(&sources, self.sessions.args.compact_keep_tokens, previous)?
                .context("No earlier completed turn can be compacted with the current keep-token budget")?;
            Ok((snapshot, cut.candidate))
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
        let first_kept_index = cut.first_kept_index;
        let task = tokio::spawn(async move {
            let sources = task_snapshot
                .messages
                .iter()
                .map(|m| SummarySource { entry_id: &m.entry_id, message: &m.message })
                .collect::<Vec<_>>();
            summarize_sources_with_instructions(
                &sources[..first_kept_index],
                task_snapshot.previous_summary.as_ref().map(|s| s.summary.as_str()),
                None,
                &model,
                provider.as_ref(),
                max_tokens,
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
            first_kept: cut.first_kept_entry_id,
            tokens_before: stored_tokens(&clean) as u64,
            expected_messages: clean,
            recovery,
            reserve_tokens: settings.reserve_tokens,
        });
        self.is_compacting = true;
        self.auto_compaction_pending = false;
        self.output.frame(json!({"type":"auto_compaction_start","reason":if incomplete { "incomplete" } else { "overflow" },"action":"context-full"}));
        Ok(MaintenanceOutcome { continuation_scheduled: true, ..none })
    }

    async fn restore_maintenance(&mut self, recovery: FailedAssistantRecovery, session: &Arc<Session>) -> Result<()> {
        let mut journal = session.journal.lock().await;
        let outcome = journal.finish_failed_assistant_recovery(recovery, false)?;
        let mut public = Session::public_messages(&journal);
        if let FailedAssistantRecoveryOutcome::Restored { message, appended_entry_id } = outcome {
            let message = *message;
            if appended_entry_id.is_none() {
                // Native empty errors live in the active transcript/public view,
                // while the raw receipt stays on its original branch.
                public.push(
                    AgentEvent::MessageEnd { message: Message::Assistant(message.clone()) }.full()["message"].clone(),
                );
            }
            let mut messages = journal.model_context();
            if messages.last().and_then(Message::as_assistant) != Some(&message) {
                messages.push(Message::Assistant(message));
            }
            self.agent.replace_idle_messages(messages)?;
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
            if self.agent.messages().await != active.expected_messages {
                bail!("Context changed during overflow recovery");
            }
            let mut journal = active.session.journal.lock().await;
            if let Err(error) = journal.commit_projected_compaction(
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
            if let Err(error) = self.agent.replace_idle_messages(journal.model_context()) {
                *active.session.persistence_error.lock().unwrap() = Some(error.to_string());
                self.connection.cancel();
                return Err(error.into());
            }
            *active.session.messages.lock().unwrap() = Session::public_messages(&journal);
            self.config.provider = self.sessions.provider.build();
            Ok::<_, anyhow::Error>(accepted)
        }
        .await;
        let finalized = if committed {
            active
                .session
                .journal
                .lock()
                .await
                .finish_failed_assistant_recovery(active.recovery, false)
                .map(|_| ())
                .map_err(anyhow::Error::from)
        } else {
            self.restore_maintenance(active.recovery, &active.session).await
        };
        if let Err(error) = finalized {
            *active.session.persistence_error.lock().unwrap() = Some(error.to_string());
            self.connection.cancel();
        }
        self.is_compacting = false;
        self.auto_compaction_pending = false;
        let mut event =
            json!({"type":"auto_compaction_end","action":"context-full","aborted":cancelled,"willRetry":false});
        match result {
            Ok(accepted) => {
                event["result"] = json!({"summary":accepted.text,"firstKeptEntryId":active.first_kept,"tokensBefore":active.tokens_before});
                let messages = self.agent.messages().await;
                if !cancelled
                    && !self.connection.is_cancelled()
                    && retry_fit(self.config.model.context_window, active.reserve_tokens, &messages)
                {
                    self.maintenance_continue = Some(PendingMaintenanceContinue {
                        session: active.session,
                        generation: active.generation,
                        command: active.command,
                        expected_messages: messages,
                        reserve_tokens: active.reserve_tokens,
                        check_fit: true,
                        deadline: Instant::now() + Duration::from_millis(100),
                    });
                    event["willRetry"] = json!(true);
                } else {
                    self.output.frame(json!({"type":"notice","level":"warning","source":"compaction",
                        "message":"Compaction could not free enough context. Clear large tool output or switch to a larger-context model."}));
                    self.drain_queues = !cancelled && self.agent.has_queued_messages();
                }
            }
            Err(error) => {
                event["errorMessage"] = json!(super::super::sanitize_text(&error.to_string()));
                self.drain_queues = !cancelled && self.agent.has_queued_messages();
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
            self.completed_maintenance(active, result).await;
        }
    }

    pub(super) async fn resume_maintenance_continue(&mut self) {
        let Some(pending) = self.maintenance_continue.take() else { return };
        if self.input_closed
            || self.connection.is_cancelled()
            || pending.generation != self.prompt_generation
            || !Arc::ptr_eq(&pending.session, &self.session)
            || self.agent.messages().await != pending.expected_messages
        {
            self.drain_queues = false;
            return;
        }
        // Explicit queued input takes ownership ahead of the automatic retry.
        if self.agent.has_queued_messages() {
            self.drain_queues = true;
            self.reconcile_queues().await;
        } else if pending.check_fit
            && !retry_fit(self.config.model.context_window, pending.reserve_tokens, &pending.expected_messages)
        {
            self.drain_queues = false;
            self.output.frame(json!({"type":"notice","level":"warning","source":"compaction",
                "message":"Context changed after compaction and no longer fits the model."}));
        } else if let Err(error) = self.start(None, pending.command) {
            self.drain_queues = false;
            self.output
                .frame(json!({"type":"notice","level":"warning","source":"compaction","message":error.to_string()}));
        }
    }

    pub(super) fn extend_maintenance_context(&mut self, session: &Arc<Session>, message: &Message) {
        if let Some(pending) = &mut self.maintenance_continue
            && pending.generation == self.prompt_generation
            && Arc::ptr_eq(&pending.session, session)
        {
            pending.expected_messages.push(message.clone());
        }
    }
}
