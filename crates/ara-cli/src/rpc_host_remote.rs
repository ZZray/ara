//! Fixed OMP remote maintenance, bound to the live Host route and journal.
//! Source: packages/coding-agent/src/session/session-maintenance.ts at 596f2da
//! (MIT; copyright notices retained in THIRD_PARTY_NOTICES.md).
use super::*;

impl Host {
    pub(super) fn budget_messages(&self, messages: &[AgentMessage]) -> Result<Vec<Message>> {
        Ok(self.session.projector.estimate_current(messages, &self.config.model)?)
    }

    pub(super) fn budget_message_tokens(
        &self,
        messages: &[AgentMessage],
        options: ara_agent::tokenizer::MessageCountOptions,
    ) -> Result<usize> {
        Ok(self.session.projector.count_current(messages, &self.config.model, options)?)
    }

    pub(super) fn route_raw_context(&self, journal: &SessionJournal) -> Result<Vec<AgentMessage>> {
        ara_cli::remote_compaction::route_raw_context(
            journal,
            &self.session.projector,
            &self.config.model,
            self.sessions.provider.metadata.as_ref(),
            &self.compaction_policy.recovery_settings()?.remote,
            self.sessions.provider.route.remote_supports_images(),
        )
    }

    pub(super) fn maintenance_stop_requested(&self) -> bool {
        let control = self.handoff_control.lock().unwrap();
        self.connection.is_cancelled()
            || if self.accepted_auto_allows_maintenance_eof() {
                control.abort_requested()
            } else {
                control.stop_requested()
            }
    }

    pub(super) async fn adopt_route_context(&self, journal: &SessionJournal) -> Result<()> {
        let selected = self.route_raw_context(journal)?;
        let current = self.agent.raw_messages().await;
        let context = self.session.projector.reconcile_owned_context(&current, selected)?;
        self.agent.replace_idle_raw_messages(context)?;
        if let Some(plan) = &self.sessions.acp_plan {
            plan.reset_reference_after_history_rewrite();
        }
        Ok(())
    }

    /// Store bounded receipts without copying remote response text, endpoint,
    /// opaque preserve data, account identity or credentials into public RPC.
    async fn record_remote_attempts(
        &self,
        session: &Session,
        attempts: &[ara_ai::remote_compaction::RemoteAttempt],
    ) -> Result<Option<String>> {
        if attempts.is_empty() {
            return Ok(None);
        }
        let receipts: Vec<Value> = attempts
            .iter()
            .map(|attempt| {
                json!({
                    "elapsedMs":attempt.elapsed_ms,"usage":attempt.usage,
                    "status":attempt.status,"failed":attempt.error.is_some()
                })
            })
            .collect();
        let artifact = session
            .artifacts
            .save(&serde_json::to_string(&json!({"method":"remote","attempts":receipts}))?, "remote-compaction")
            .await?;
        Ok(Some(format!("artifact://{artifact}")))
    }

    pub(super) async fn remote_history(
        &mut self,
        focus: Option<&str>,
        options: ara_agent::compaction::SummaryOptions,
    ) -> Result<Option<Value>> {
        if self.session.persistence_error.lock().unwrap().is_some() {
            bail!("session persistence failed; restart from the journal before remote compaction");
        }
        if focus.is_some_and(|focus| focus.len() > 1_000_000) {
            bail!("compaction instructions exceed the summary input limit");
        }
        let settings = self.compaction_policy.recovery_settings()?;
        let remote_config = ara_cli::remote_compaction::model_config(self.sessions.provider.metadata.as_ref())?;
        if !ara_agent::remote::remote_available(&self.config.model, &remote_config, &settings.remote) {
            return Ok(None);
        }
        let guard = self.maintenance_guard();
        if guard.cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        let session = self.session.clone();
        let generation = self.prompt_generation;
        let route_generation = self.sessions.provider.route.generation();
        let model = self.config.model.clone();
        let messages = self.agent.raw_messages().await;
        let expected_bodies = self.session.projector.current_body_receipts(&messages)?;
        // Reject invalid Agent admission before issuing a billable side call.
        self.agent.replace_idle_raw_messages(messages.clone())?;
        let snapshot = {
            let journal = session.journal.lock().await;
            let available = ara_cli::remote_compaction::native_replay_available(
                &journal,
                &model,
                self.sessions.provider.metadata.as_ref(),
                &settings.remote,
                self.sessions.provider.route.remote_supports_images(),
            )?;
            journal.native_remote_snapshot(&model, available)?
        };
        let transport = self.sessions.provider.route.bind_remote(
            self.sessions.provider.client.clone(),
            ara_ai::remote_compaction::RemoteRequestOptions {
                session_id: Some(snapshot.projection.session_id.clone()),
                prompt_cache_key: Some(snapshot.projection.session_id.clone()),
                generic_model: remote_config.model.clone(),
                ..Default::default()
            },
        );
        let deadline = Instant::now() + Duration::from_secs_f64(self.max_time.unwrap_or(120.0).clamp(0.0, 120.0));
        let prepared = ara_cli::remote_compaction::prepare_remote(
            &snapshot,
            &self.config,
            transport,
            &remote_config,
            &settings.remote,
            self.sessions.args.compact_keep_tokens,
            focus,
            settings.reserve_tokens,
            options,
            deadline,
            &guard.cancel,
        )
        .await;
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                if let Some(failed) = error.downcast_ref::<ara_cli::remote_compaction::RemotePreparationError>() {
                    let receipt = self.record_remote_attempts(&session, &failed.attempts).await;
                    // Artifact storage cannot turn an accepted Abort into
                    // ordinary failure and accidentally authorize fallback.
                    if let Err(receipt_error) = receipt
                        && !guard.cancel.is_cancelled()
                        && !ara_cli::remote_compaction::is_cancelled(&error)
                    {
                        return Err(receipt_error);
                    }
                }
                // The raw error can include arbitrary upstream response text.
                // Preserve typed cancellation while making ordinary fallback safe.
                return if guard.cancel.is_cancelled() || ara_cli::remote_compaction::is_cancelled(&error) {
                    Err(ara_cli::handoff::cancelled())
                } else {
                    Err(anyhow::anyhow!("Remote compaction failed; review the Session attempt receipt"))
                };
            }
        };
        let Some(prepared) = prepared else { return Ok(None) };
        let receipt = self.record_remote_attempts(&session, &prepared.attempts).await;
        if guard.cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        let receipt = receipt?;
        if generation != self.prompt_generation
            || !Arc::ptr_eq(&session, &self.session)
            || route_generation != self.sessions.provider.route.generation()
            || model != self.config.model
            || expected_bodies != self.session.projector.current_body_receipts(&self.agent.raw_messages().await)?
        {
            bail!("Context or route changed during remote compaction");
        }
        let tokens_before =
            self.budget_message_tokens(&messages, ara_agent::tokenizer::MessageCountOptions::default())? as u64;
        let mut journal = session.journal.lock().await;
        if guard.cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        let old_leaf = journal.leaf_id().map(str::to_owned);
        if let Err(error) = prepared.commit(&mut journal, &snapshot, tokens_before) {
            if error.history_published() {
                session.persistence_error.lock().unwrap().get_or_insert_with(|| error.to_string());
                self.connection.cancel();
                self.drain_queues = false;
                let _ = self.adopt_route_context(&journal).await;
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
        Ok(Some(json!({"summary":prepared.summary_text(),"method":"remote",
            "firstKeptEntryId":prepared.first_kept_entry_id,"tokensBefore":tokens_before,
            "attemptReceipt":receipt})))
    }
}
