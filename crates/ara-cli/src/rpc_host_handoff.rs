//! Fixed OMP handoff side request and checked, same-Session adoption.
use super::*;

#[derive(Default)]
pub(super) struct HandoffControl {
    current: Option<CancellationToken>,
    current_allows_eof: bool,
    queued_stops: usize,
    input_closed: bool,
}

impl HandoffControl {
    pub(super) fn stops_handoff(command: &Command) -> bool {
        command.kind == "abort" || (command.kind == "abort_and_prompt" && command.message().is_ok())
    }

    pub(super) fn queue_stop(&mut self) {
        self.queued_stops = self.queued_stops.saturating_add(1);
        if let Some(cancel) = &self.current {
            cancel.cancel();
        }
    }

    pub(super) fn consume_stop(&mut self) {
        self.queued_stops = self.queued_stops.saturating_sub(1);
    }

    pub(super) fn close_input(&mut self) {
        self.input_closed = true;
        if let Some(cancel) = &self.current
            && !self.current_allows_eof
        {
            cancel.cancel();
        }
    }

    pub(super) fn stop_requested(&self) -> bool {
        self.queued_stops > 0 || self.input_closed
    }

    pub(super) fn abort_requested(&self) -> bool {
        self.queued_stops > 0
    }

    pub(super) fn begin(control: &Arc<Mutex<Self>>, connection: &CancellationToken, allow_eof: bool) -> HandoffGuard {
        let cancel = connection.child_token();
        let mut state = control.lock().unwrap();
        // Installing the slot and checking accepted stops share the reader's
        // lock, including Abort received before this side request begins.
        state.current = Some(cancel.clone());
        state.current_allows_eof = allow_eof;
        if state.queued_stops > 0 || (state.input_closed && !allow_eof) {
            cancel.cancel();
        }
        HandoffGuard { control: control.clone(), cancel }
    }
}

pub(super) struct HandoffGuard {
    control: Arc<Mutex<HandoffControl>>,
    pub(super) cancel: CancellationToken,
}

impl Drop for HandoffGuard {
    fn drop(&mut self) {
        // The serial Host owns only one handoff. Keep the slot installed until
        // journal commit and publication error handling have both settled.
        self.control.lock().unwrap().current = None;
    }
}

impl Host {
    pub(super) fn maintenance_guard(&self) -> HandoffGuard {
        HandoffControl::begin(&self.handoff_control, &self.connection, self.accepted_auto_allows_maintenance_eof())
    }

    pub(super) fn accepted_auto_allows_maintenance_eof(&self) -> bool {
        self.auto.preparing_accepted_input() && !self.retry_policy.usage_aware_fallback()
    }

    pub(super) async fn handoff_local_history(&mut self, focus: Option<&str>, auto: bool) -> Result<Option<Value>> {
        if self.session.persistence_error.lock().unwrap().is_some() {
            bail!("session persistence failed; restart from the journal before handoff");
        }
        let handoff = self.maintenance_guard();
        if handoff.cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        let session = self.session.clone();
        let snapshot = session.journal.lock().await.native_handoff_snapshot()?;
        let messages = self.agent.raw_messages().await;
        let body_receipts = self.session.projector.current_body_receipts(&messages)?;
        let provider = self
            .sessions
            .provider
            .route
            .bind_side_request(self.sessions.provider.client.clone(), &snapshot.projection.session_id);
        let deadline = Instant::now() + Duration::from_secs_f64(self.max_time.unwrap_or(120.0).clamp(0.0, 120.0));
        let prepared = if self.session.projector.actual_values().is_some() {
            ara_cli::handoff::prepare_actual_handoff(
                &snapshot,
                &messages,
                &self.config,
                provider.as_ref(),
                self.sessions.args.compact_keep_tokens,
                focus,
                auto,
                deadline,
                &handoff.cancel,
            )
            .await
        } else {
            ara_cli::handoff::prepare_handoff(
                &snapshot,
                &self.budget_messages(&messages)?,
                &self.config,
                provider.as_ref(),
                self.sessions.args.compact_keep_tokens,
                focus,
                auto,
                deadline,
                &handoff.cancel,
            )
            .await
        }?;
        if handoff.cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        let Some(prepared) = prepared else { return Ok(None) };
        let tokens_before =
            self.budget_message_tokens(&messages, ara_agent::tokenizer::MessageCountOptions::default())? as u64;
        if auto
            && self.compaction_policy.recovery_settings()?.handoff_save_to_disk
            && let Some(directory) = session.artifacts.directory()
            && let Err(error) = ara_cli::handoff::save_document(directory, &prepared.invocation.text).await
        {
            self.output.frame(json!({"type":"notice","level":"warning","source":"compaction",
                "message":format!("Failed to save handoff document to disk: {error}")}));
        }
        let mut journal = session.journal.lock().await;
        if handoff.cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        if body_receipts != self.session.projector.current_body_receipts(&self.agent.raw_messages().await)? {
            bail!("Context changed during handoff");
        }
        let old_leaf = journal.leaf_id().map(str::to_owned);
        let result = journal.commit_native_entry_handoff(
            &snapshot,
            &prepared.summary,
            &prepared.first_kept_entry_id,
            &prepared.window_source_entry_ids,
            tokens_before,
        );
        if let Err(error) = result {
            if error.history_published() {
                session.persistence_error.lock().unwrap().get_or_insert_with(|| error.to_string());
                self.connection.cancel();
                self.drain_queues = false;
                self.adopt_route_context(&journal).await?;
                match public_messages_after_publication(&journal, &session, &self.connection, &mut self.drain_queues) {
                    Ok(messages) => *session.messages.lock().unwrap() = messages,
                    Err(_) => return Err(error.into()),
                }
                self.config.provider = self
                    .sessions
                    .provider
                    .route
                    .bind_codex_session(self.sessions.provider.client.clone(), snapshot.projection.session_id.clone());
                self.publish_snapshot(&self.config);
            }
            return Err(error.into());
        }
        if let Err(error) = self.adopt_route_context(&journal).await {
            *session.persistence_error.lock().unwrap() = Some(error.to_string());
            self.connection.cancel();
            return Err(error);
        }
        *session.messages.lock().unwrap() =
            public_messages_after_publication(&journal, &session, &self.connection, &mut self.drain_queues)?;
        self.maintenance_bash_transition(old_leaf);
        self.config.provider = self
            .sessions
            .provider
            .route
            .bind_codex_session(self.sessions.provider.client.clone(), snapshot.projection.session_id.clone());
        self.publish_snapshot(&self.config);
        self.local_context_rebase = None;
        Ok(Some(json!({"summary":prepared.summary.summary,"method":"handoff",
            "firstKeptEntryId":prepared.first_kept_entry_id,"tokensBefore":tokens_before})))
    }
}
