//! Local native maintenance on the retained RPC Session.
use super::*;
use ara_agent::compaction::local_reduction::{PruneConfig, ShakeConfig, SupersedePruneConfig};
use ara_cli::local_reduction::{
    HostReductionPolicy, PreparedReduction, prepare_images_after_boundary, prepare_prune, prepare_shake, prepare_stale,
};

/// Fixed rebaseAfterCompaction invalidates the pre-rewrite usage prefix until
/// a fresh provider report arrives. This receipt describes the actual idle
/// transcript; it is an estimate, never a fabricated provider usage value.
pub(super) struct ContextRebase {
    messages: Vec<Value>,
    non_message: usize,
    prompt_tokens: usize,
    invalidated_anchor_ids: Vec<String>,
}

impl Host {
    fn reduction_policy(
        &self,
        snapshot: &ara_session::SessionReductionSnapshot,
        journal: &SessionJournal,
    ) -> Result<HostReductionPolicy> {
        let native_boundary = ara_cli::remote_compaction::native_reduction_boundary(
            journal,
            &self.config.model,
            self.sessions.provider.metadata.as_ref(),
            &self.compaction_policy.recovery_settings()?.remote,
            self.sessions.provider.route.remote_supports_images(),
        )?;
        Ok(HostReductionPolicy {
            keep_boundary_id: native_boundary.or_else(|| {
                snapshot
                    .entries
                    .iter()
                    .rev()
                    .find(|entry| entry.kind == "compaction")
                    .and_then(|entry| entry.raw["firstKeptEntryId"].as_str())
                    .map(str::to_owned)
            }),
            prefix_binding: self
                .sessions
                .provider
                .metadata
                .as_ref()
                .is_some_and(|model| model["thinking"]["prefixBinding"] == true),
            protected_read_paths: Vec::new(),
        })
    }

    pub(super) fn non_message_tokens(&self) -> usize {
        ara_cli::context_budget::non_message_tokens(&self.config.model, &self.config.system_prompt, &self.config.tools)
    }

    pub(super) async fn context_tokens(
        &self,
        raw_messages: &[AgentMessage],
        raw_pending: &[AgentMessage],
    ) -> Result<usize> {
        #[cfg(test)]
        if let Some(observation) = &self.fixed_snapcompact_observation {
            return Ok(observation.lock().unwrap().usage_tokens);
        }
        let journal = self.session.journal.lock().await;
        self.session.projector.refresh_checked(&journal, raw_messages, &self.config.model)?;
        let snapshot = journal.raw_reduction_snapshot()?;
        let latest_anchor_id =
            ara_cli::context_budget::anchor(&snapshot)?.map(|(index, _)| snapshot.entries[index].id.clone());
        if let Some(rebase) = &self.local_context_rebase
            && self.session.projector.reduction_receipts(raw_messages)?.starts_with(&rebase.messages)
            && latest_anchor_id.as_ref().is_none_or(|id| rebase.invalidated_anchor_ids.contains(id))
        {
            let tail = self.session.projector.count_current(
                &raw_messages[rebase.messages.len()..],
                &self.config.model,
                Default::default(),
            )?;
            let pending = self.session.projector.count_current(
                raw_pending,
                &self.config.model,
                ara_agent::tokenizer::MessageCountOptions { exclude_encrypted_reasoning: true },
            )?;
            let current_non_message = self.non_message_tokens();
            let anchored = rebase
                .prompt_tokens
                .saturating_add(current_non_message.saturating_sub(rebase.non_message))
                .saturating_add(tail)
                .saturating_add(pending);
            let floor = current_non_message
                .saturating_add(self.session.projector.count_current(
                    raw_messages,
                    &self.config.model,
                    ara_agent::tokenizer::MessageCountOptions { exclude_encrypted_reasoning: true },
                )?)
                .saturating_add(pending);
            return Ok(anchored.max(floor));
        }
        Ok(ara_cli::context_budget::context_tokens_for_current(
            &self.config.model,
            &journal,
            &self.session.projector,
            raw_messages,
            raw_pending,
            self.non_message_tokens(),
        )?)
    }

    async fn rebase_local_context(&mut self) -> Result<()> {
        let messages = self.agent.raw_messages().await;
        let journal = self.session.journal.lock().await;
        let message_tokens = match self
            .session
            .projector
            .refresh_checked(&journal, &messages, &self.config.model)
            .and_then(|()| self.session.projector.count_current(&messages, &self.config.model, Default::default()))
        {
            Ok(tokens) => tokens,
            Err(error) => {
                self.local_context_rebase = None;
                return Err(error.into());
            }
        };
        let non_message = self.non_message_tokens();
        let prompt_tokens = non_message.saturating_add(message_tokens);
        let snapshot = journal.raw_reduction_snapshot()?;
        let mut invalidated_anchor_ids = Vec::new();
        for entry in &snapshot.entries {
            if ara_cli::context_budget::prompt_tokens(entry)?.is_some() {
                invalidated_anchor_ids.push(entry.id.clone());
            }
        }
        self.local_context_rebase = Some(ContextRebase {
            messages: self.session.projector.reduction_receipts(&messages)?,
            non_message,
            prompt_tokens,
            invalidated_anchor_ids,
        });
        Ok(())
    }

    async fn commit_local_reduction(
        &mut self,
        mut prepared: PreparedReduction,
        expected: &[Value],
        anchored: bool,
        recovery: Option<&mut ara_session::FailedAssistantRecovery>,
    ) -> Result<bool> {
        if prepared.edits.is_empty() {
            return Ok(false);
        }
        if self.connection.is_cancelled() {
            bail!("Request was aborted");
        }
        if self.session.projector.reduction_receipts(&self.agent.raw_messages().await)? != expected {
            bail!("Context changed during local maintenance");
        }
        if anchored
            && let Some(edit) = ara_cli::context_budget::anchor_edit(
                &prepared.snapshot,
                &prepared.entry_tokens_freed,
                self.non_message_tokens(),
            )?
        {
            prepared.edits.push(edit);
        }
        let session = self.session.clone();
        let mut journal = session.journal.lock().await;
        let preserve_rebase = if let Some(rebase) = &self.local_context_rebase {
            ara_cli::context_budget::anchor(&prepared.snapshot)?
                .is_none_or(|(index, _)| rebase.invalidated_anchor_ids.contains(&prepared.snapshot.entries[index].id))
        } else {
            false
        };
        let committed = match recovery {
            Some(recovery) => journal.commit_reduction_during_recovery(&prepared.snapshot, &prepared.edits, recovery),
            None => journal.commit_reduction(&prepared.snapshot, &prepared.edits),
        };
        if let Err(error) = committed {
            if error.history_published() {
                session.persistence_error.lock().unwrap().get_or_insert_with(|| error.to_string());
                self.connection.cancel();
                self.drain_queues = false;
                let _ = self.adopt_route_context(&journal).await;
                match public_messages_after_publication(&journal, &session, &self.connection, &mut self.drain_queues) {
                    Ok(messages) => *session.messages.lock().unwrap() = messages,
                    Err(_) => return Err(error.into()),
                }
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
        self.config.provider = self.sessions.provider.build();
        self.publish_snapshot(&self.config);
        drop(journal);
        if preserve_rebase {
            self.rebase_local_context().await?;
        } else {
            self.local_context_rebase = None;
        }
        // Local rewrites preserve entry IDs/leaf and all Bash destinations.
        Ok(true)
    }

    /// Stale reads run independently of compaction.enabled. Age pruning only
    /// runs after a successful settled turn while no recovery saga owns it.
    pub(super) async fn prune_local_history(&mut self) -> Result<usize> {
        let settings = self.compaction_policy.recovery_settings()?;
        let expected = self.session.projector.reduction_receipts(&self.agent.raw_messages().await)?;
        let (snapshot, policy) = {
            let journal = self.session.journal.lock().await;
            let snapshot = journal.raw_reduction_snapshot()?;
            let policy = self.reduction_policy(&snapshot, &journal)?;
            (snapshot, policy)
        };
        let config = SupersedePruneConfig {
            prune_useless: settings.drop_useless,
            idle_flush_ms: 90.0 * 60_000.0,
            ..Default::default()
        };
        let prepared = prepare_stale(
            snapshot,
            &self.config.model,
            &config,
            settings.supersede_reads,
            &policy,
            ara_ai::now_ms() as f64,
            ara_ai::now_ms(),
        )?;
        let mut freed = prepared.tokens_freed;
        self.commit_local_reduction(prepared, &expected, false, None).await?;
        let settled = self.session.projector.reduction_receipts(&self.agent.raw_messages().await)?;
        if !self.compaction_policy.enabled()
            || settled.last().is_none_or(|assistant| {
                assistant["role"] != "assistant"
                    || matches!(assistant["stopReason"].as_str(), Some("error" | "aborted"))
            })
        {
            return Ok(freed);
        }
        let expected = self.session.projector.reduction_receipts(&self.agent.raw_messages().await)?;
        let (snapshot, policy) = {
            let journal = self.session.journal.lock().await;
            let snapshot = journal.raw_reduction_snapshot()?;
            let policy = self.reduction_policy(&snapshot, &journal)?;
            (snapshot, policy)
        };
        let config = PruneConfig {
            prune_useless: settings.drop_useless,
            cache_warm_suffix_tokens: Some(8_000.0),
            ..Default::default()
        };
        let prepared = prepare_prune(snapshot, &self.config.model, &config, &policy, ara_ai::now_ms())?;
        freed = freed.saturating_add(prepared.tokens_freed);
        self.commit_local_reduction(prepared, &expected, false, None).await?;
        Ok(freed)
    }

    pub(super) async fn shake_local_history(
        &mut self,
        rescue: bool,
        recovery: Option<&mut ara_session::FailedAssistantRecovery>,
    ) -> Result<bool> {
        let expected = self.session.projector.reduction_receipts(&self.agent.raw_messages().await)?;
        let (snapshot, policy) = {
            let journal = self.session.journal.lock().await;
            let snapshot = journal.raw_reduction_snapshot()?;
            let policy = self.reduction_policy(&snapshot, &journal)?;
            (snapshot, policy)
        };
        let config = if rescue { ShakeConfig::rescue() } else { ShakeConfig::default() };
        #[cfg(test)]
        if let Some(observation) = &self.fixed_snapcompact_observation {
            observation.lock().unwrap().shake_calls.push((rescue, config));
            return Ok(false);
        }
        let prepared =
            prepare_shake(snapshot, &self.config.model, &config, &policy, &self.session.artifacts, ara_ai::now_ms())
                .await?;
        self.commit_local_reduction(prepared, &expected, true, recovery).await
    }

    pub(super) async fn recovery_fits(
        &self,
        reserve: Option<f64>,
        threshold: Option<(usize, usize)>,
        pending: &[AgentMessage],
    ) -> Result<bool> {
        let messages = self.agent.raw_messages().await;
        let tokens = self.context_tokens(&messages, pending).await?;
        if let Some((threshold, prune_saved)) = threshold {
            let floor = self
                .non_message_tokens()
                .saturating_add(self.session.projector.count_current(
                    &messages,
                    &self.config.model,
                    ara_agent::tokenizer::MessageCountOptions { exclude_encrypted_reasoning: true },
                )?)
                .saturating_add(self.session.projector.count_current(
                    pending,
                    &self.config.model,
                    ara_agent::tokenizer::MessageCountOptions { exclude_encrypted_reasoning: true },
                )?);
            let correction = if self.local_context_rebase.is_some() { 0 } else { prune_saved };
            return Ok(tokens.saturating_sub(correction).max(floor) <= (threshold as f64 * 0.8).floor() as usize);
        }
        Ok(maintenance::retry_tokens_fit(self.config.model.context_window, reserve, tokens))
    }

    pub(super) async fn rescue_local_history(
        &mut self,
        reserve: Option<f64>,
        threshold: Option<(usize, usize)>,
        skip_elide: bool,
        pending: &[AgentMessage],
        mut recovery: Option<&mut ara_session::FailedAssistantRecovery>,
    ) -> Result<bool> {
        if self.connection.is_cancelled() {
            return Ok(false);
        }
        // A failed-assistant owner must be consumed after an archive append
        // before any further raw edits. Its call site performs tier zero first.
        let rescued_archive = if recovery.is_none() {
            self.rescue_snapcompact_frames(reserve, threshold.map(|(limit, _)| limit), pending).await?
        } else {
            None
        };
        if rescued_archive.is_some()
            && self.recovery_fits(reserve, threshold.map(|(limit, _)| (limit, 0)), pending).await?
        {
            return Ok(true);
        }
        if !skip_elide {
            let changed = self.shake_local_history(true, recovery.as_deref_mut()).await?;
            if changed {
                self.rebase_local_context().await?;
            }
            if changed && self.recovery_fits(reserve, threshold.map(|(limit, _)| (limit, 0)), pending).await? {
                return Ok(true);
            }
        }
        if self.connection.is_cancelled() {
            return Ok(false);
        }
        let expected = self.session.projector.reduction_receipts(&self.agent.raw_messages().await)?;
        let (snapshot, native_boundary) = {
            let journal = self.session.journal.lock().await;
            let snapshot = journal.raw_reduction_snapshot()?;
            let boundary = ara_cli::remote_compaction::native_reduction_boundary(
                &journal,
                &self.config.model,
                self.sessions.provider.metadata.as_ref(),
                &self.compaction_policy.recovery_settings()?.remote,
                self.sessions.provider.route.remote_supports_images(),
            )?;
            (snapshot, boundary)
        };
        let prepared = prepare_images_after_boundary(snapshot, &self.config.model, native_boundary.as_deref())?;
        let changed = self.commit_local_reduction(prepared, &expected, false, recovery).await?;
        if changed {
            self.rebase_local_context().await?;
        }
        let fits = changed && self.recovery_fits(reserve, threshold.map(|(limit, _)| (limit, 0)), pending).await?;
        if !fits && let Some(entry_id) = rescued_archive {
            self.stamp_frame_dead_end(&entry_id, threshold.is_none()).await?;
        }
        Ok(fits)
    }
}
