//! Fixed OMP 596f2da remote preparation: opaque native history and readable
//! generic text have distinct snapshots and atomic Session publication.
//! Source: packages/agent/src/compaction/compaction.ts (MIT; notices retained).

use anyhow::{Context as _, Result, bail};
use ara_agent::{
    AgentConfig,
    compaction::{self, SummaryOptions},
    remote::{self, RemoteCompactionTransport, RemoteSettings, RemoteSummaryProvider},
};
use ara_ai::{
    Context, Message, Model,
    remote_compaction::{
        RemoteAttempt, RemoteConfig, RemoteGenericRequest, parse_remote_preserve_data, validate_replacement_for_route,
    },
};
use ara_session::{NativeHandoffSummary, NativeRemoteError, NativeRemoteSnapshot, NativeRemoteSummary, SessionJournal};
use serde_json::Value;
use std::{sync::Arc, time::Instant};
use tokio_util::sync::CancellationToken;

pub fn model_config(metadata: Option<&Value>) -> Result<RemoteConfig> {
    match metadata.and_then(|metadata| metadata.get("remoteCompaction")).filter(|value| !value.is_null()) {
        Some(value) => serde_json::from_value(value.clone()).context("invalid model remoteCompaction configuration"),
        None => Ok(RemoteConfig::default()),
    }
}

pub fn replay_available(model: &Model, config: &RemoteConfig, settings: &RemoteSettings) -> bool {
    matches!(model.api.as_str(), "openai-responses" | "openai-codex-responses")
        && remote::native_available(model, config, settings)
}

/// The same availability is used by preparation, publication and ordinary
/// replay. A past image payload cannot bypass this route's encoder capability.
pub fn native_replay_available(
    journal: &SessionJournal,
    model: &Model,
    metadata: Option<&Value>,
    settings: &RemoteSettings,
    supports_images: bool,
) -> Result<bool> {
    if !replay_available(model, &model_config(metadata)?, settings) {
        return Ok(false);
    }
    // Fresh and cleared Sessions have no durable compaction source yet.
    // Ordinary route adoption must still permit their first real prompt.
    if !journal.is_on_disk() || journal.leaf_id().is_none() {
        journal.raw_reduction_snapshot()?;
        return Ok(false);
    }
    let projection = journal.native_projected_compaction_snapshot_for_route(model, true)?;
    if let Some(previous) = projection.previous_summary.and_then(|summary| summary.remote) {
        let data = parse_remote_preserve_data(&previous.preserve_data).context("invalid native remote replay")?;
        if validate_replacement_for_route(model, &data.replacement_history, supports_images).is_err() {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn route_context(
    journal: &SessionJournal,
    model: &Model,
    metadata: Option<&Value>,
    settings: &RemoteSettings,
    supports_images: bool,
) -> Result<Vec<Message>> {
    let available = native_replay_available(journal, model, metadata, settings, supports_images)?;
    Ok(journal.model_context_for_route(model, available))
}

/// Covered raw entries remain source evidence for a later readable fallback.
/// Local reduction only changes the raw tail after this opaque carrier.
pub fn native_reduction_boundary(
    journal: &SessionJournal,
    model: &Model,
    metadata: Option<&Value>,
    settings: &RemoteSettings,
    supports_images: bool,
) -> Result<Option<String>> {
    if !journal.is_on_disk() || journal.leaf_id().is_none() {
        journal.raw_reduction_snapshot()?;
        return Ok(None);
    }
    let available = native_replay_available(journal, model, metadata, settings, supports_images)?;
    let projection = journal.native_projected_compaction_snapshot_for_route(model, available)?;
    Ok(projection.previous_summary.filter(|summary| summary.remote.is_some()).map(|summary| summary.entry_id))
}

pub enum PreparedRemoteSummary {
    Native(NativeRemoteSummary),
    Text { summary: NativeHandoffSummary, short_summary: String },
}

pub struct PreparedRemote {
    pub summary: PreparedRemoteSummary,
    pub first_kept_entry_id: String,
    pub window_source_entry_ids: Vec<String>,
    pub replay_through_entry_id: String,
    pub attempts: Vec<RemoteAttempt>,
}

pub type RemoteCommitError = NativeRemoteError;

impl PreparedRemote {
    pub fn summary_text(&self) -> &str {
        match &self.summary {
            PreparedRemoteSummary::Native(summary) => &summary.summary,
            PreparedRemoteSummary::Text { summary, .. } => &summary.summary,
        }
    }

    pub fn commit(
        &self,
        journal: &mut SessionJournal,
        snapshot: &NativeRemoteSnapshot,
        tokens_before: u64,
    ) -> std::result::Result<String, RemoteCommitError> {
        match &self.summary {
            PreparedRemoteSummary::Native(summary) => journal.commit_native_entry_remote(
                snapshot,
                summary,
                &self.first_kept_entry_id,
                &self.window_source_entry_ids,
                &self.replay_through_entry_id,
                tokens_before,
            ),
            PreparedRemoteSummary::Text { summary, short_summary } => journal.commit_native_entry_remote_text(
                snapshot,
                &snapshot.readable,
                summary,
                short_summary,
                &self.first_kept_entry_id,
                &self.window_source_entry_ids,
                tokens_before,
            ),
        }
    }
}

#[derive(Debug)]
pub struct RemotePreparationError {
    pub cause: anyhow::Error,
    pub attempts: Vec<RemoteAttempt>,
}
impl std::fmt::Display for RemotePreparationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.cause.fmt(formatter)
    }
}
impl std::error::Error for RemotePreparationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

pub fn is_cancelled(error: &anyhow::Error) -> bool {
    if crate::handoff::is_cancelled(error) {
        return true;
    }
    if let Some(error) = error.downcast_ref::<RemotePreparationError>() {
        return is_cancelled(&error.cause);
    }
    error.downcast_ref::<ara_ai::remote_compaction::RemoteError>().is_some_and(remote::cancelled)
}

fn failed(cause: anyhow::Error, attempts: Vec<RemoteAttempt>) -> anyhow::Error {
    anyhow::Error::new(RemotePreparationError { cause, attempts })
}

fn file_summary(
    projection: &ara_session::NativeProjectedCompactionSnapshot,
    prefix: usize,
    summary: &str,
) -> NativeHandoffSummary {
    let context = Context {
        messages: projection.entries[..prefix].iter().flat_map(|entry| entry.messages.clone()).collect(),
        ..Context::default()
    };
    let previous =
        projection.previous_summary.as_ref().and_then(|summary| summary.file_details.as_ref()).map(|details| {
            ara_agent::handoff::HandoffFileDetails {
                read_files: details.read_files.clone(),
                modified_files: details.modified_files.clone(),
            }
        });
    let summary = ara_agent::handoff::prepare_handoff_summary(summary, &context, previous.as_ref());
    NativeHandoffSummary {
        summary: summary.summary,
        read_files: summary.read_files,
        modified_files: summary.modified_files,
    }
}

/// Deadline cancels the request, then awaits its resolver's settlement. In
/// particular an in-flight OAuth rotation must not be detached or replayed.
#[allow(clippy::too_many_arguments)]
async fn native_call(
    transport: &dyn RemoteCompactionTransport,
    config: &AgentConfig,
    context: &Context,
    remote_config: &RemoteConfig,
    settings: &RemoteSettings,
    previous: Option<&[Value]>,
    deadline: Instant,
    cancel: &CancellationToken,
) -> std::result::Result<Option<ara_ai::remote_compaction::RemoteResult>, ara_ai::remote_compaction::RemoteError> {
    let request_cancel = cancel.child_token();
    let _guard = request_cancel.clone().drop_guard();
    let instructions = config.system_prompt.join("\n\n");
    let call = remote::compact_provider_native(
        transport,
        &config.model,
        context,
        remote_config,
        settings,
        &instructions,
        previous,
        &request_cancel,
    );
    tokio::pin!(call);
    tokio::select! {
        biased;
        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => { request_cancel.cancel(); call.await },
        result = &mut call => result,
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn prepare_remote(
    snapshot: &NativeRemoteSnapshot,
    config: &AgentConfig,
    transport: Arc<dyn RemoteCompactionTransport>,
    remote_config: &RemoteConfig,
    settings: &RemoteSettings,
    keep_tokens: usize,
    focus: Option<&str>,
    reserve_tokens: Option<f64>,
    summary_options: SummaryOptions,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<Option<PreparedRemote>> {
    if cancel.is_cancelled() {
        return Err(crate::handoff::cancelled());
    }
    if !remote::remote_available(&config.model, remote_config, settings) {
        return Ok(None);
    }
    if snapshot.active_model != config.model {
        bail!("remote snapshot does not match the active route");
    }
    let mut attempts = Vec::new();
    let projection = &snapshot.projection;
    if snapshot.native_replay_available && replay_available(&config.model, remote_config, settings) {
        let sources = crate::native_compaction::sources(&projection.entries);
        let previous_summary = projection.previous_summary.as_ref().map(|summary| summary.summary.as_str());
        let previous_text = projection
            .previous_summary
            .as_ref()
            .filter(|summary| summary.remote.is_none())
            .map(|summary| summary.summary.as_str());
        if let Some(cut) = compaction::select_native_remote_compaction_cut(&sources, keep_tokens, previous_summary)?
            && cut.first_kept_index > 0
        {
            let mut context = Context {
                system_prompt: config.system_prompt.clone(),
                messages: projection.entries.iter().flat_map(|entry| entry.messages.clone()).collect(),
                tools: Some(config.tools.iter().map(|tool| tool.definition()).collect()),
            };
            if let Some(previous) = previous_text {
                context.messages.insert(
                    0,
                    Message::User(ara_ai::UserMessage::text(format!(
                        "[Compacted summary of earlier turns; source entries withheld]\n{previous}"
                    ))),
                );
            }
            let context = config.hooks.transform_provider_context(context, &config.model).await;
            let previous = projection
                .previous_summary
                .as_ref()
                .and_then(|summary| summary.remote.as_ref())
                .and_then(|remote| parse_remote_preserve_data(&remote.preserve_data));
            match native_call(
                transport.as_ref(),
                config,
                &context,
                remote_config,
                settings,
                previous.as_ref().map(|data| data.replacement_history.as_slice()),
                deadline,
                cancel,
            )
            .await
            {
                Ok(Some(result)) => {
                    if cancel.is_cancelled() || Instant::now() >= deadline {
                        return Err(failed(crate::handoff::cancelled(), result.attempts));
                    }
                    let files = file_summary(projection, cut.first_kept_index, &result.summary);
                    let through = projection
                        .entries
                        .iter()
                        .rev()
                        .find(|entry| !entry.messages.is_empty())
                        .context("native remote request has no raw tail")?
                        .entry_id
                        .clone();
                    return Ok(Some(PreparedRemote {
                        summary: PreparedRemoteSummary::Native(NativeRemoteSummary {
                            summary: files.summary,
                            short_summary: result.short_summary.or_else(|| Some("Remote compaction".into())),
                            preserve_data: result
                                .preserve_data
                                .context("native remote request omitted preserveData")?,
                            read_files: files.read_files,
                            modified_files: files.modified_files,
                        }),
                        first_kept_entry_id: cut.first_kept_entry_id,
                        window_source_entry_ids: projection.entries[..cut.first_kept_index]
                            .iter()
                            .filter(|entry| !entry.messages.is_empty())
                            .map(|entry| entry.entry_id.clone())
                            .collect(),
                        replay_through_entry_id: through,
                        attempts: result.attempts,
                    }));
                }
                Ok(None) => {}
                Err(error) => {
                    attempts.extend(error.attempts.clone());
                    if cancel.is_cancelled()
                        || remote::cancelled(&error)
                        || Instant::now() >= deadline
                        || settings.endpoint.as_deref().is_none_or(str::is_empty)
                    {
                        return Err(failed(anyhow::Error::new(error), attempts));
                    }
                }
            }
        }
    }
    let Some(endpoint) = settings.endpoint.as_ref().filter(|endpoint| !endpoint.is_empty()) else {
        return Ok(None);
    };
    if cancel.is_cancelled() || Instant::now() >= deadline {
        return Err(failed(crate::handoff::cancelled(), attempts));
    }
    // A text model must read real sources, never an opaque native placeholder.
    let projection = &snapshot.readable;
    let sources = crate::native_compaction::sources(&projection.entries);
    let previous = projection.previous_summary.as_ref().map(|summary| summary.summary.as_str());
    let Some(cut) = compaction::select_native_entry_compaction_cut(&sources, keep_tokens, previous)
        .map_err(|error| failed(anyhow::Error::new(error), attempts.clone()))?
    else {
        return Ok(None);
    };
    if cut.first_kept_index == 0 {
        return Ok(None);
    }
    // Refuse unsupported kept-tail input before a billable history call.
    let recent = compaction::serialize_native_entry_sources_for_summary(&sources[cut.first_kept_index..])
        .map_err(|error| failed(anyhow::Error::new(error), attempts.clone()))?;
    compaction::summary_output_budget_tokens(reserve_tokens)
        .map_err(|error| failed(anyhow::Error::new(error), attempts.clone()))?;
    let cap = (0.2 * reserve_tokens.unwrap_or(compaction::MAX_SUMMARY_TOKENS as f64)).floor().min(512.0) as u64;
    if cap == 0 {
        return Err(failed(anyhow::anyhow!("short summary output budget must be positive"), attempts));
    }
    let provider = RemoteSummaryProvider::new(transport.clone(), endpoint.clone());
    let accepted = compaction::summarize_native_entry_compaction_cut(
        &sources,
        &cut,
        previous,
        focus,
        &config.model,
        &provider,
        reserve_tokens,
        summary_options,
        deadline,
        cancel,
    )
    .await;
    provider.settle().await;
    attempts.extend(provider.attempts());
    let accepted = accepted.map_err(|error| failed(anyhow::Error::new(error), attempts.clone()))?;
    let prompt = format!(
        "<conversation>\n{recent}\n</conversation>\n\n<previous-summary>\n{}\n</previous-summary>\n\n{}",
        compaction::escape_summary_boundary_tags(&accepted.text),
        include_str!("../../ara-agent/prompts/compaction-short-summary.md")
    );
    let request = RemoteGenericRequest {
        system_prompt: include_str!("../../ara-agent/prompts/summarization-system.md").into(),
        prompt,
        max_tokens: Some(cap),
    };
    let request_cancel = cancel.child_token();
    let _guard = request_cancel.clone().drop_guard();
    let call = transport.generic(&config.model, endpoint, request, &request_cancel);
    tokio::pin!(call);
    let result = tokio::select! {
        biased;
        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => { request_cancel.cancel(); call.await },
        result = &mut call => result,
    };
    let result = match result {
        Ok(result) => {
            attempts.extend(result.attempts.clone());
            result
        }
        Err(error) => {
            attempts.extend(error.attempts.clone());
            return Err(failed(anyhow::Error::new(error), attempts));
        }
    };
    if cancel.is_cancelled() || request_cancel.is_cancelled() {
        return Err(failed(crate::handoff::cancelled(), attempts));
    }
    Ok(Some(PreparedRemote {
        summary: PreparedRemoteSummary::Text {
            summary: file_summary(projection, cut.first_kept_index, &accepted.text),
            short_summary: result.summary,
        },
        first_kept_entry_id: cut.first_kept_entry_id,
        window_source_entry_ids: accepted.window_source_entry_ids,
        replay_through_entry_id: String::new(),
        attempts,
    }))
}
