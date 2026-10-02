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
    messages: Vec<Message>,
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

    pub(super) async fn context_tokens(&self, messages: &[Message], pending: &[Message]) -> usize {
        let journal = self.session.journal.lock().await;
        let latest_anchor_id = journal.raw_reduction_snapshot().ok().and_then(|snapshot| {
            let (index, _) = ara_cli::context_budget::anchor(&snapshot)?;
            Some(snapshot.entries[index].id.clone())
        });
        if let Some(rebase) = &self.local_context_rebase
            && messages.starts_with(&rebase.messages)
            && latest_anchor_id.as_ref().is_none_or(|id| rebase.invalidated_anchor_ids.contains(id))
        {
            let tail = ara_cli::context_budget::tokenizer(&self.config.model)
                .count_messages(&messages[rebase.messages.len()..], Default::default());
            let pending = ara_cli::context_budget::tokenizer(&self.config.model).count_messages(
                pending,
                ara_agent::tokenizer::MessageCountOptions { exclude_encrypted_reasoning: true },
            );
            let current_non_message = self.non_message_tokens();
            let anchored = rebase
                .prompt_tokens
                .saturating_add(current_non_message.saturating_sub(rebase.non_message))
                .saturating_add(tail)
                .saturating_add(pending);
            let floor = current_non_message
                .saturating_add(ara_cli::context_budget::tokenizer(&self.config.model).count_messages(
                    messages,
                    ara_agent::tokenizer::MessageCountOptions { exclude_encrypted_reasoning: true },
                ))
                .saturating_add(pending);
            return anchored.max(floor);
        }
        ara_cli::context_budget::context_tokens_for_model(
            &self.config.model,
            &journal,
            messages,
            pending,
            self.non_message_tokens(),
        )
    }

    async fn rebase_local_context(&mut self) {
        let messages = self.agent.messages().await;
        let non_message = self.non_message_tokens();
        let prompt_tokens = non_message.saturating_add(
            ara_cli::context_budget::tokenizer(&self.config.model).count_messages(&messages, Default::default()),
        );
        let invalidated_anchor_ids = self
            .session
            .journal
            .lock()
            .await
            .raw_reduction_snapshot()
            .map(|snapshot| {
                snapshot
                    .entries
                    .iter()
                    .filter(|entry| ara_cli::context_budget::prompt_tokens(entry).is_some())
                    .map(|entry| entry.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        self.local_context_rebase =
            Some(ContextRebase { messages, non_message, prompt_tokens, invalidated_anchor_ids });
    }

    async fn commit_local_reduction(
        &mut self,
        mut prepared: PreparedReduction,
        expected: &[Message],
        anchored: bool,
        recovery: Option<&mut ara_session::FailedAssistantRecovery>,
    ) -> Result<bool> {
        if prepared.edits.is_empty() {
            return Ok(false);
        }
        if self.connection.is_cancelled() {
            bail!("Request was aborted");
        }
        if self.agent.messages().await != expected {
            bail!("Context changed during local maintenance");
        }
        if anchored
            && let Some(edit) = ara_cli::context_budget::anchor_edit(
                &prepared.snapshot,
                &prepared.entry_tokens_freed,
                self.non_message_tokens(),
            )
        {
            prepared.edits.push(edit);
        }
        let session = self.session.clone();
        let mut journal = session.journal.lock().await;
        let preserve_rebase = self.local_context_rebase.as_ref().is_some_and(|rebase| {
            ara_cli::context_budget::anchor(&prepared.snapshot)
                .is_none_or(|(index, _)| rebase.invalidated_anchor_ids.contains(&prepared.snapshot.entries[index].id))
        });
        let committed = match recovery {
            Some(recovery) => journal.commit_reduction_during_recovery(&prepared.snapshot, &prepared.edits, recovery),
            None => journal.commit_reduction(&prepared.snapshot, &prepared.edits),
        };
        if let Err(error) = committed {
            if error.history_published() {
                if let Ok(context) = self.route_model_context(&journal) {
                    let _ = self.agent.replace_idle_messages(context);
                }
                *session.messages.lock().unwrap() = Session::public_messages(&journal);
                *session.persistence_error.lock().unwrap() = Some(error.to_string());
                self.connection.cancel();
            }
            return Err(error.into());
        }
        if let Err(error) = self.adopt_route_context(&journal) {
            *session.persistence_error.lock().unwrap() = Some(error.to_string());
            self.connection.cancel();
            return Err(error);
        }
        *session.messages.lock().unwrap() = Session::public_messages(&journal);
        self.config.provider = self.sessions.provider.build();
        self.publish_snapshot(&self.config);
        drop(journal);
        if preserve_rebase {
            self.rebase_local_context().await;
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
        let expected = self.agent.messages().await;
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
        if !self.compaction_policy.enabled()
            || self.agent.messages().await.last().and_then(Message::as_assistant).is_none_or(|assistant| {
                matches!(assistant.stop_reason, ara_ai::StopReason::Error | ara_ai::StopReason::Aborted)
            })
        {
            return Ok(freed);
        }
        let expected = self.agent.messages().await;
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
        let expected = self.agent.messages().await;
        let (snapshot, policy) = {
            let journal = self.session.journal.lock().await;
            let snapshot = journal.raw_reduction_snapshot()?;
            let policy = self.reduction_policy(&snapshot, &journal)?;
            (snapshot, policy)
        };
        let config = if rescue { ShakeConfig::rescue() } else { ShakeConfig::default() };
        let prepared =
            prepare_shake(snapshot, &self.config.model, &config, &policy, &self.session.artifacts, ara_ai::now_ms())
                .await?;
        self.commit_local_reduction(prepared, &expected, true, recovery).await
    }

    pub(super) async fn recovery_fits(
        &self,
        reserve: Option<f64>,
        threshold: Option<(usize, usize)>,
        pending: &[Message],
    ) -> bool {
        let messages = self.agent.messages().await;
        let tokens = self.context_tokens(&messages, pending).await;
        if let Some((threshold, prune_saved)) = threshold {
            let floor = self
                .non_message_tokens()
                .saturating_add(ara_cli::context_budget::tokenizer(&self.config.model).count_messages(
                    &messages,
                    ara_agent::tokenizer::MessageCountOptions { exclude_encrypted_reasoning: true },
                ))
                .saturating_add(ara_cli::context_budget::tokenizer(&self.config.model).count_messages(
                    pending,
                    ara_agent::tokenizer::MessageCountOptions { exclude_encrypted_reasoning: true },
                ));
            let correction = if self.local_context_rebase.is_some() { 0 } else { prune_saved };
            return tokens.saturating_sub(correction).max(floor) <= (threshold as f64 * 0.8).floor() as usize;
        }
        maintenance::retry_tokens_fit(self.config.model.context_window, reserve, tokens)
    }

    pub(super) async fn rescue_local_history(
        &mut self,
        reserve: Option<f64>,
        threshold: Option<(usize, usize)>,
        skip_elide: bool,
        pending: &[Message],
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
            && self.recovery_fits(reserve, threshold.map(|(limit, _)| (limit, 0)), pending).await
        {
            return Ok(true);
        }
        if !skip_elide {
            let changed = self.shake_local_history(true, recovery.as_deref_mut()).await?;
            if changed {
                self.rebase_local_context().await;
            }
            if changed && self.recovery_fits(reserve, threshold.map(|(limit, _)| (limit, 0)), pending).await {
                return Ok(true);
            }
        }
        if self.connection.is_cancelled() {
            return Ok(false);
        }
        let expected = self.agent.messages().await;
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
            self.rebase_local_context().await;
        }
        let fits = changed && self.recovery_fits(reserve, threshold.map(|(limit, _)| (limit, 0)), pending).await;
        if !fits && let Some(entry_id) = rescued_archive {
            self.stamp_frame_dead_end(&entry_id, threshold.is_none()).await?;
        }
        Ok(fits)
    }
}
