//! Fixed OMP local snapcompact and tier-zero archive rescue on the live Session.
use super::*;
use ara_cli::snapcompact::{PreparedSnapcompact, SnapcompactPolicy};

pub(super) fn dead_end_warning(frames: Option<usize>) -> String {
    let remedies = frames.map_or_else(|| "shrink it (e.g. clear large tool output)".to_owned(), |count|
        format!("reduce archived image frames ({count} held) — providers often bill vision media separately from tokens; shrink it (e.g. clear large tool output)"));
    format!(
        "Compaction freed too little context to make progress — pausing automatic maintenance to avoid a compaction loop. The most recent turn alone is too large to reduce further; {remedies} or switch to a larger-context model."
    )
}

impl Host {
    async fn snapcompact_policy(&self, pending: &[Message]) -> Result<SnapcompactPolicy> {
        let settings = self.compaction_policy.recovery_settings()?;
        let options = ara_agent::tokenizer::MessageCountOptions { exclude_encrypted_reasoning: true };
        let pending_tokens = ara_cli::context_budget::tokenizer(&self.config.model).count_messages(pending, options);
        Ok(SnapcompactPolicy {
            shape: settings.snapcompact_shape,
            reserve_tokens: settings.reserve_tokens,
            non_message_tokens: self.non_message_tokens(),
            pending_tokens,
        })
    }

    async fn publish_snapcompact(
        &mut self,
        snapshot: &ara_session::NativeSnapcompactSnapshot,
        prepared: &PreparedSnapcompact,
        rescue: bool,
    ) -> Result<Value> {
        let session = self.session.clone();
        let mut journal = session.journal.lock().await;
        if self.maintenance_stop_requested() {
            return Err(ara_cli::handoff::cancelled());
        }
        let old_leaf = journal.leaf_id().map(str::to_owned);
        let committed = if rescue {
            journal.commit_native_snapcompact_rescue(snapshot, &prepared.summary, prepared.result.tokens_before as u64)
        } else {
            journal.commit_native_entry_snapcompact(
                snapshot,
                &prepared.summary,
                &prepared.result.first_kept_entry_id,
                &prepared.window_source_entry_ids,
                prepared.result.tokens_before as u64,
            )
        };
        let entry_id = match committed {
            Ok(entry_id) => entry_id,
            Err(error) => {
                if error.history_published() {
                    *session.persistence_error.lock().unwrap() = Some(error.to_string());
                    self.connection.cancel();
                    if let Ok(context) = self.route_model_context(&journal) {
                        let _ = self.agent.replace_idle_messages(context);
                    }
                    *session.messages.lock().unwrap() = Session::public_messages(&journal);
                    self.config.provider = self.sessions.provider.build();
                    self.publish_snapshot(&self.config);
                }
                return Err(error.into());
            }
        };
        if let Err(error) = self.adopt_route_context(&journal) {
            *session.persistence_error.lock().unwrap() = Some(error.to_string());
            self.connection.cancel();
            return Err(error);
        }
        *session.messages.lock().unwrap() = Session::public_messages(&journal);
        self.maintenance_bash_transition(old_leaf);
        self.config.provider = self
            .sessions
            .provider
            .route
            .bind_codex_session(self.sessions.provider.client.clone(), snapshot.projection.session_id.clone());
        self.publish_snapshot(&self.config);
        self.local_context_rebase = None;
        // Archive text and frame payload belong only in the durable journal.
        let mut response = json!({"summary":prepared.result.summary,"shortSummary":prepared.result.short_summary,
            "method":"snapcompact","entryId":entry_id,"firstKeptEntryId":prepared.result.first_kept_entry_id,
            "tokensBefore":prepared.result.tokens_before,"tokensAfter":prepared.tokens_after});
        if let Some(preserve) = ara_snapcompact::strip_preserved_archive(prepared.result.preserve_data.as_ref()) {
            response["preserveData"] = preserve;
        }
        Ok(response)
    }

    pub(super) async fn snapcompact_history(&mut self, pending: &[Message]) -> Result<Option<Value>> {
        if !ara_cli::snapcompact::supports_images(self.sessions.provider.metadata.as_ref()) {
            return Ok(None);
        }
        if self.session.persistence_error.lock().unwrap().is_some() {
            bail!("session persistence failed; restart from the journal before snapcompact")
        }
        let guard = handoff::HandoffControl::begin(&self.handoff_control, &self.connection);
        if guard.cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        let session = self.session.clone();
        let generation = self.prompt_generation;
        let route_generation = self.sessions.provider.route.generation();
        let model = self.config.model.clone();
        let expected = self.agent.messages().await;
        self.agent.replace_idle_messages(expected.clone())?;
        let snapshot = session.journal.lock().await.native_snapcompact_snapshot()?;
        let policy = self.snapcompact_policy(pending).await?;
        let tokens_before =
            ara_cli::context_budget::tokenizer(&self.config.model).count_messages(&expected, Default::default()) as u64;
        let keep_tokens = self.sessions.args.compact_keep_tokens;
        let source = snapshot.clone();
        let worker_model = model.clone();
        // Join the CPU worker before clearing the cancellation slot. Aborted
        // rendering can never publish later through a detached continuation.
        let prepared = tokio::task::spawn_blocking(move || {
            ara_cli::snapcompact::prepare_snapcompact(&source, &worker_model, keep_tokens, tokens_before, &policy)
        })
        .await?;
        if guard.cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        let Some(prepared) = prepared? else { return Ok(None) };
        if generation != self.prompt_generation
            || !Arc::ptr_eq(&session, &self.session)
            || route_generation != self.sessions.provider.route.generation()
            || model != self.config.model
            || expected != self.agent.messages().await
        {
            bail!("Context or route changed during snapcompact")
        }
        Ok(Some(self.publish_snapcompact(&snapshot, &prepared, false).await?))
    }

    pub(super) async fn rescue_snapcompact_frames(
        &mut self,
        reserve: Option<f64>,
        threshold: Option<usize>,
        pending: &[Message],
    ) -> Result<Option<String>> {
        if !ara_cli::snapcompact::supports_images(self.sessions.provider.metadata.as_ref())
            || self.maintenance_stop_requested()
        {
            return Ok(None);
        }
        let session = self.session.clone();
        if session.persistence_error.lock().unwrap().is_some() {
            bail!("session persistence failed before frame rescue")
        }
        let guard = handoff::HandoffControl::begin(&self.handoff_control, &self.connection);
        let expected = self.agent.messages().await;
        self.agent.replace_idle_messages(expected.clone())?;
        let snapshot = session.journal.lock().await.native_snapcompact_snapshot()?;
        let mut policy = self.snapcompact_policy(pending).await?;
        policy.reserve_tokens = reserve;
        let model = self.config.model.clone();
        let generation = self.prompt_generation;
        let route_generation = self.sessions.provider.route.generation();
        // Frame sizing always uses the configured threshold, including
        // overflow/incomplete retry. The caller's post-publication success
        // predicate still distinguishes threshold maintenance from retry.
        let configured = threshold.unwrap_or(self.sessions.args.compact_threshold);
        let limit = match model.context_window.filter(|window| window.is_finite() && *window > 0.0) {
            Some(window) if configured > 0 => (configured as f64).max(1.0).min(window - 1.0),
            Some(window) => ara_cli::snapcompact::prompt_budget(Some(window), reserve).min(window - 1.0).max(0.0),
            None => f64::INFINITY,
        };
        let source = snapshot.clone();
        let worker_model = model.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            ara_cli::snapcompact::prepare_frame_rescue(&source, &worker_model, &policy, limit)
        })
        .await?;
        if guard.cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        let prepared = match prepared {
            Ok(Some(prepared)) => prepared,
            Ok(None) => return Ok(None),
            Err(_) => return Ok(None), // fixed local-render failure falls through to elide/images
        };
        if generation != self.prompt_generation
            || !Arc::ptr_eq(&session, &self.session)
            || route_generation != self.sessions.provider.route.generation()
            || model != self.config.model
            || expected != self.agent.messages().await
        {
            bail!("Context or route changed during frame rescue")
        }
        let response = self.publish_snapcompact(&snapshot, &prepared, true).await?;
        self.output.frame(json!({"type":"notice","level":"info","source":"compaction",
            "message":"Rebuilt the retained snapcompact archive at a smaller frame budget"}));
        Ok(Some(response["entryId"].as_str().context("frame rescue omitted its published entry ID")?.to_owned()))
    }

    pub(super) async fn stamp_frame_dead_end(&mut self, entry_id: &str, emit_notice: bool) -> Result<()> {
        let session = self.session.clone();
        let mut journal = session.journal.lock().await;
        let frames = journal
            .entries()
            .iter()
            .find(|entry| entry.id == entry_id)
            .and_then(|entry| entry.raw["preserveData"]["snapcompact"]["frames"].as_array())
            .map(Vec::len);
        let warning = dead_end_warning(frames);
        if let Err(error) = journal.stamp_native_snapcompact_warning(entry_id, &warning) {
            // The rescue is already published. Do not fall back or restore the
            // failed assistant after a later warning-publication error.
            *session.persistence_error.lock().unwrap() = Some(error.to_string());
            self.connection.cancel();
            return Err(error.into());
        }
        if emit_notice {
            self.output.frame(json!({"type":"notice","level":"warning","source":"compaction","message":warning}));
        }
        Ok(())
    }
}
