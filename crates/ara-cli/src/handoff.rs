//! Fixed OMP handoff Host preparation. Session owns checked publication and
//! Core owns the cache-preserving one-shot request and file-operation summary.
//! Source: OMP 596f2da, session-handoff.ts and session-maintenance.ts (MIT).

use anyhow::{Context as _, Result, bail};
use ara_agent::AgentConfig;
use ara_agent::compaction::select_native_entry_compaction_cut;
use ara_ai::{Context, Message, ModelProvider, UserMessage};
use ara_session::{NativeHandoffSnapshot, NativeHandoffSummary};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

pub struct PreparedHandoff {
    pub summary: NativeHandoffSummary,
    pub first_kept_entry_id: String,
    pub window_source_entry_ids: Vec<String>,
    pub invocation: ara_agent::compaction::AcceptedSummary,
}

pub const AUTO_FOCUS: &str =
    "Threshold-triggered maintenance: preserve critical implementation state and immediate next actions.";

#[derive(Debug)]
struct HandoffCancelled;

impl std::fmt::Display for HandoffCancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Handoff cancelled")
    }
}

impl std::error::Error for HandoffCancelled {}

pub fn cancelled() -> anyhow::Error {
    anyhow::Error::new(HandoffCancelled)
}

pub fn is_cancelled(error: &anyhow::Error) -> bool {
    error.downcast_ref::<HandoffCancelled>().is_some()
        || error
            .downcast_ref::<ara_agent::compaction::SummaryCallError>()
            .is_some_and(|error| error.kind == ara_agent::compaction::SummaryCallErrorKind::Cancelled)
}

/// Native automatic saving is best effort and separate from journal acceptance.
pub async fn save_document(directory: &Path, document: &str) -> Result<PathBuf> {
    tokio::fs::create_dir_all(directory).await?;
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true).replace([':', '.'], "-");
    let path = directory.join(format!("handoff-{timestamp}.md"));
    tokio::fs::write(&path, format!("{document}\n")).await?;
    Ok(path)
}

/// Keep the full live Context on the model call. Only the discarded source
/// prefix contributes cumulative file-operation details for the journal.
#[allow(clippy::too_many_arguments)]
pub async fn prepare_handoff(
    snapshot: &NativeHandoffSnapshot,
    messages: &[Message],
    config: &AgentConfig,
    provider: &dyn ModelProvider,
    keep_tokens: usize,
    focus: Option<&str>,
    auto: bool,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<Option<PreparedHandoff>> {
    prepare_handoff_request(
        snapshot,
        HandoffMessages::Typed(messages),
        config,
        provider,
        keep_tokens,
        focus,
        auto,
        deadline,
        cancel,
    )
    .await
}

/// Production side request keeps original Graph messages throughout conversion.
#[allow(clippy::too_many_arguments)]
pub async fn prepare_actual_handoff(
    snapshot: &NativeHandoffSnapshot,
    messages: &[ara_agent::AgentMessage],
    config: &AgentConfig,
    provider: &dyn ModelProvider,
    keep_tokens: usize,
    focus: Option<&str>,
    auto: bool,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<Option<PreparedHandoff>> {
    prepare_handoff_request(
        snapshot,
        HandoffMessages::Actual(messages),
        config,
        provider,
        keep_tokens,
        focus,
        auto,
        deadline,
        cancel,
    )
    .await
}

enum HandoffMessages<'a> {
    Typed(&'a [Message]),
    Actual(&'a [ara_agent::AgentMessage]),
}

#[allow(clippy::too_many_arguments)]
async fn prepare_handoff_request(
    snapshot: &NativeHandoffSnapshot,
    messages: HandoffMessages<'_>,
    config: &AgentConfig,
    provider: &dyn ModelProvider,
    keep_tokens: usize,
    focus: Option<&str>,
    auto: bool,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<Option<PreparedHandoff>> {
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    // One active owned snapshot supplies model, prompt, tools and context binding.
    let execution =
        config.hooks.execution_snapshot().unwrap_or_else(|| ara_agent::ExecutionSnapshot::from_config(config));
    let model = execution.model.as_ref().map(|selected| selected.model.clone()).unwrap_or_else(|| config.model.clone());
    let max_tokens = execution.model.as_ref().map_or(config.max_tokens, |selected| selected.max_tokens);
    let projection = &snapshot.projection;
    let sources = crate::native_compaction::sources_for_model(&projection.entries, &model);
    let previous = projection.previous_summary.as_ref();
    let cut = select_native_entry_compaction_cut(&sources, keep_tokens, previous.map(|entry| entry.summary.as_str()))
        .map_err(|error| anyhow::anyhow!("{error}"))?
        .context("Nothing to hand off (already compacted)")?;
    if cut.first_kept_index == 0 {
        bail!("Nothing to hand off (no earlier message prefix)");
    }
    let cap = (model.api != "openai-codex-responses").then_some(max_tokens).flatten();
    let invocation = match messages {
        HandoffMessages::Typed(messages) => {
            let mut side_messages = messages.to_vec();
            side_messages.push(Message::User(UserMessage::text(ara_agent::handoff::render_handoff_prompt(focus))));
            let context = ara_agent::build_side_request_context_from_snapshot(config, execution, side_messages, None)
                .await
                .map_err(anyhow::Error::new)?;
            ara_agent::handoff::generate_handoff_from_context(&context, &model, provider, cap, deadline, cancel)
                .await
                .map_err(anyhow::Error::new)
                .context("Handoff generation failed")?
        }
        HandoffMessages::Actual(messages) => {
            let values = execution
                .provider_context
                .as_ref()
                .and_then(|binding| binding.actual_values.clone())
                .context("Actual handoff has no Session values")?;
            // Fresh Host-created raw User, admitted before the same converter
            // that consumes the complete live history. It is request-only.
            let trailing = values
                .heap()
                .import_serde(&serde_json::json!({
                    "role":"user", "content":[{"type":"text", "text":ara_agent::handoff::render_handoff_prompt(focus)}],
                    "attribution":"agent", "timestamp":ara_ai::now_ms(),
                }))
                .map_err(anyhow::Error::new)?;
            let mut raw = messages.to_vec();
            raw.push(ara_agent::AgentMessage::from_graph(trailing, None).map_err(anyhow::Error::new)?);
            let prepare = async {
                let converted = config
                    .hooks
                    .convert_agent_messages_to_llm_actual(&raw, &values)
                    .await
                    .map_err(anyhow::Error::new)?;
                ara_agent::provider_context::build_actual_side_request_context_from_snapshot(
                    config,
                    execution,
                    converted,
                    Some(config.system_prompt.clone()),
                )
                .await
                .map_err(anyhow::Error::new)
            };
            let context = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(cancelled()),
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                    bail!("Handoff preparation deadline elapsed")
                }
                prepared = prepare => prepared?,
            };
            ara_agent::handoff::generate_handoff_from_actual_context(&context, &model, provider, cap, deadline, cancel)
                .await
                .map_err(|error| match error {
                    ara_agent::handoff::ActualHandoffError::Summary(error) => anyhow::Error::new(error),
                    ara_agent::handoff::ActualHandoffError::Native(error) => anyhow::Error::new(error),
                })
                .context("Handoff generation failed")?
        }
    };
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    if invocation.text.trim().is_empty() {
        if auto {
            return Ok(None);
        }
        bail!("Handoff generation produced no content");
    }
    let file_context = Context {
        messages: projection.entries[..cut.first_kept_index].iter().flat_map(|entry| entry.messages.clone()).collect(),
        ..Context::default()
    };
    let previous_files =
        previous.and_then(|entry| entry.file_details.as_ref()).map(|details| ara_agent::handoff::HandoffFileDetails {
            read_files: details.read_files.clone(),
            modified_files: details.modified_files.clone(),
        });
    let summary = ara_agent::handoff::prepare_handoff_summary(&invocation.text, &file_context, previous_files.as_ref());
    Ok(Some(PreparedHandoff {
        summary: NativeHandoffSummary {
            summary: summary.summary,
            read_files: summary.read_files,
            modified_files: summary.modified_files,
        },
        first_kept_entry_id: cut.first_kept_entry_id,
        window_source_entry_ids: projection.entries[..cut.first_kept_index]
            .iter()
            .filter(|entry| !entry.messages.is_empty())
            .map(|entry| entry.entry_id.clone())
            .collect(),
        invocation,
    }))
}
