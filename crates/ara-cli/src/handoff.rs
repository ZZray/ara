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
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    let projection = &snapshot.projection;
    let sources = crate::native_compaction::sources_for_model(&projection.entries, &config.model);
    let previous = projection.previous_summary.as_ref();
    let cut = select_native_entry_compaction_cut(&sources, keep_tokens, previous.map(|entry| entry.summary.as_str()))
        .map_err(|error| anyhow::anyhow!("{error}"))?
        .context("Nothing to hand off (already compacted)")?;
    if cut.first_kept_index == 0 {
        bail!("Nothing to hand off (no earlier message prefix)");
    }
    let mut context = Context {
        system_prompt: config.system_prompt.clone(),
        messages: messages.to_vec(),
        tools: Some(config.tools.iter().map(|tool| tool.definition()).collect()),
    };
    context.messages.push(Message::User(UserMessage::text(ara_agent::handoff::render_handoff_prompt(focus))));
    let context = config.hooks.transform_provider_context(context, &config.model).await;
    let cap = (config.model.api != "openai-codex-responses").then_some(config.max_tokens).flatten();
    let invocation =
        ara_agent::handoff::generate_handoff_from_context(&context, &config.model, provider, cap, deadline, cancel)
            .await
            .map_err(anyhow::Error::new)
            .context("Handoff generation failed")?;
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
