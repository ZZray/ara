//! Read-only fixed OMP session-context.ts and compaction/messages.ts views.
//! Source commit 596f2da7101178214aa27a753529d15e6b7ad91d (MIT).
//! Derived Wire bodies never enter Agent context, persistence, or replay grants.
use crate::{AgentMessageBody, CompactionSummaryView, Entry, Result, SessionJournal, native_message, snapcompact};
use ara_rpc::{WireString, WireValue as W};
use std::collections::HashSet;

const SUPERSEDED_SUMMARY: &str = "[Superseded compaction summary elided after a newer compaction]";
const SUPERSEDED_SHORT_SUMMARY: &str = "Superseded compaction elided";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryViewPolicy {
    Provider,
    FullTranscript,
    CollapsedTranscript,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscriptViewMode {
    Full,
    Collapsed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TranscriptViewOptions {
    pub mode: TranscriptViewMode,
    /// Live display rebuilds retain an in-flight call; settled displays strip it.
    pub keep_dangling_tool_calls: bool,
}

#[derive(Debug, PartialEq)]
pub struct TranscriptViewMessage {
    /// Observation label only; this derived body supplies no source grant.
    pub entry_id: String,
    pub body: W,
}

#[derive(Debug, PartialEq)]
pub struct TranscriptView {
    pub messages: Vec<TranscriptViewMessage>,
    pub cache_miss_explained_at: Vec<bool>,
}

/// Fixed summary factory DTO; this is distinct from compaction_summary_source's
/// exact original body used by Host source equality. Only an already selected
/// checked summary may contribute native remote providerPayload.
pub fn compaction_summary_dto(
    entry: &Entry,
    policy: SummaryViewPolicy,
    active: bool,
    selected: Option<&CompactionSummaryView>,
) -> std::io::Result<W> {
    compaction_summary_dto_with_store(entry, policy, active, selected, None)
}

fn compaction_summary_dto_with_store(
    entry: &Entry,
    policy: SummaryViewPolicy,
    active: bool,
    selected: Option<&CompactionSummaryView>,
    store: Option<&crate::BlobStore>,
) -> std::io::Result<W> {
    if entry.kind != "compaction" {
        return Err(native_message::invalid("summary view requires a compaction entry"));
    }
    // Metadata is a bounded envelope view. Do not instantiate the complete
    // original archive-bearing equality body just to select display fields.
    let mut body = W::object(vec![("role", W::String("compactionSummary".into()))]);
    for field in ["summary", "shortSummary", "tokensBefore", "tokensAfter", "method", "warning"] {
        if let Some(value) = entry.raw.get(field) {
            body.insert(field, native_message::wire_from_serde(value)?);
        }
    }
    body.insert(
        "timestamp",
        W::Number(
            crate::entry_timestamp(&entry.raw)
                .ok_or_else(|| native_message::invalid("summary view requires a valid timestamp"))? as f64,
        ),
    );
    if !active {
        body.insert("summary", W::String(SUPERSEDED_SUMMARY.into()));
        body.insert("shortSummary", W::String(SUPERSEDED_SHORT_SUMMARY.into()));
    }
    if policy != SummaryViewPolicy::FullTranscript
        && active
        && let Some(remote) =
            selected.filter(|summary| summary.entry_id == entry.id).and_then(|summary| summary.remote.as_ref())
    {
        // remote belongs to the existing validated Session projection. Do not
        // read shaped preserveData directly to manufacture a replay payload.
        if let Some(payload) = ara_ai::remote_compaction::parse_remote_preserve_data(&remote.preserve_data)
            && let Some(provider) = payload.provider
        {
            body.insert(
                "providerPayload",
                native_message::wire_from_serde(&serde_json::json!({
                    "type":"openaiResponsesHistory", "provider":provider, "items":payload.replacement_history
                }))?,
            );
        }
    }
    if active
        && policy != SummaryViewPolicy::CollapsedTranscript
        && let Some(archive) = entry.archive_source().map(|source| source.exact_archive()).transpose()?.flatten()
    {
        let options = match policy {
            SummaryViewPolicy::Provider => {
                snapcompact::native_model_archive_history_options_with_store(&archive, store, None)?
            }
            SummaryViewPolicy::FullTranscript => ara_snapcompact::HistoryBlockOptions::default(),
            SummaryViewPolicy::CollapsedTranscript => unreachable!(),
        };
        let mut blocks = snapcompact::native_archive_history_blocks_with_store(&archive, options, store)?;
        // compactionFrame is the existing model converter's private image
        // marker, not part of fixed historyBlocks / summary display DTOs.
        for block in &mut blocks {
            remove(block, "compactionFrame");
        }
        let images = blocks
            .iter()
            .filter(|block| is(block, "type", "image"))
            .map(native_message::clone_wire)
            .collect::<Vec<_>>();
        if !blocks.is_empty() {
            body.insert("blocks", W::Array(blocks));
        }
        if !images.is_empty() {
            body.insert("images", W::Array(images));
        }
    }
    Ok(body)
}

fn is(body: &W, field: &str, value: &str) -> bool {
    body.get(field).and_then(W::as_string).is_some_and(|text| text.equals_ascii(value))
}
fn remove(body: &mut W, field: &str) {
    if let W::Object(fields) = body {
        fields.retain(|(key, _)| !key.equals_ascii(field));
    }
}
fn truthy(value: &W) -> bool {
    match value {
        W::Null => false,
        W::Bool(value) => *value,
        W::Number(value) => *value != 0.0 && !value.is_nan(),
        W::String(value) => !value.is_empty(),
        W::Array(_) | W::Object(_) => true,
    }
}

// Fixed messages.ts:645-696. This display predicate is deliberately not
// UserContent/provider validation: the original accepts any array content.
fn display_custom_body(entry: &Entry) -> std::io::Result<Option<W>> {
    let source = match &entry.native_message {
        Some(source) => native_message::clone_wire(source.observe()?.as_ref()),
        None => native_message::wire_from_serde(&entry.raw)?,
    };
    let Some(content) = source.get("content").filter(|value| matches!(value, W::String(_) | W::Array(_))) else {
        return Ok(None);
    };
    let custom_type = source
        .get("customType")
        .and_then(W::as_string)
        .filter(|value| !value.is_empty())
        .cloned()
        .unwrap_or_else(|| "custom-message".into());
    let mut body = W::object(vec![
        ("role", W::String("custom".into())),
        ("customType", W::String(custom_type)),
        ("content", native_message::clone_wire(content)),
        ("display", W::Bool(matches!(source.get("display"), Some(W::Bool(true))))),
        (
            "timestamp",
            W::Number(
                crate::entry_timestamp(&entry.raw)
                    .ok_or_else(|| native_message::invalid("custom display requires a valid timestamp"))?
                    as f64,
            ),
        ),
    ]);
    if let Some(details) = source.get("details") {
        body.insert("details", native_message::clone_wire(details));
    }
    if let Some(attribution) = source.get("attribution") {
        body.insert(
            "attribution",
            W::String(if attribution.as_string().is_some_and(|value| value.equals_ascii("user")) {
                "user".into()
            } else {
                "agent".into()
            }),
        );
    }
    Ok(Some(body))
}

struct Builder {
    view: TranscriptView,
    pending_reset: bool,
    current_mode: String,
    last_assistant_model: Option<(WireString, WireString)>,
}
impl Builder {
    fn new() -> Self {
        Self {
            view: TranscriptView { messages: Vec::new(), cache_miss_explained_at: Vec::new() },
            pending_reset: false,
            current_mode: "none".into(),
            last_assistant_model: None,
        }
    }
    fn handle_entry(&mut self, entry: &Entry) {
        match entry.kind.as_str() {
            "compaction" | "model_change" => self.pending_reset = true,
            "mode_change" => {
                let mode = entry.raw.get("mode").and_then(serde_json::Value::as_str).unwrap_or("none");
                if (mode == "plan") != (self.current_mode == "plan") {
                    self.pending_reset = true;
                }
                self.current_mode = mode.into();
            }
            _ => {}
        }
    }
    fn push(&mut self, entry: &Entry, body: W) {
        let explained = if is(&body, "role", "assistant") {
            let provider = body.get("provider").and_then(W::as_string).cloned().unwrap_or_else(|| "undefined".into());
            let model = body.get("model").and_then(W::as_string).cloned().unwrap_or_else(|| "undefined".into());
            let current = (provider, model);
            let changed = self.last_assistant_model.as_ref().is_some_and(|previous| previous != &current);
            self.last_assistant_model = Some(current);
            let explained = self.pending_reset || changed;
            self.pending_reset = false;
            explained
        } else {
            false
        };
        self.view.messages.push(TranscriptViewMessage { entry_id: entry.id.clone(), body });
        self.view.cache_miss_explained_at.push(explained);
    }
    fn append(&mut self, entry: &Entry) -> Result<()> {
        self.handle_entry(entry);
        if entry.kind == "custom_message" {
            if let Some(body) = display_custom_body(entry)? {
                self.push(entry, body);
            }
            return Ok(());
        }
        if entry.kind == "branch_summary" {
            let has_summary = match &entry.native_message {
                Some(source) => source.observe()?.get("summary").is_some_and(truthy),
                None => entry
                    .raw
                    .get("summary")
                    .map(native_message::wire_from_serde)
                    .transpose()?
                    .as_ref()
                    .is_some_and(truthy),
            };
            if !has_summary {
                return Ok(());
            }
        }
        if let Some(body) = entry.agent_message_body()? {
            let body = match body {
                AgentMessageBody::Native(source) => native_message::clone_wire(source.observe()?.as_ref()),
                AgentMessageBody::Legacy(body) => native_message::wire_from_serde(&body)?,
            };
            self.push(entry, body);
        }
        Ok(())
    }
    fn strip_dangling(&mut self) {
        let paired = self
            .view
            .messages
            .iter()
            .filter(|message| is(&message.body, "role", "toolResult"))
            .filter_map(|message| message.body.get("toolCallId").and_then(W::as_string).cloned())
            .collect::<HashSet<_>>();
        for message in &mut self.view.messages {
            if !is(&message.body, "role", "assistant") {
                continue;
            }
            let Some(content) = message.body.get("content").and_then(W::as_array) else { continue };
            let unpaired = |block: &W| {
                is(block, "type", "toolCall")
                    && !block.get("id").and_then(W::as_string).is_some_and(|id| paired.contains(id))
            };
            let count = content.iter().filter(|block| unpaired(block)).count();
            if count == 0 {
                continue;
            }
            let normalized = content
                .iter()
                .filter(|block| !unpaired(block) && !is(block, "type", "redactedThinking"))
                .map(|block| {
                    let mut block = native_message::clone_wire(block);
                    if is(&block, "type", "thinking") && block.get("thinkingSignature").is_some_and(truthy) {
                        remove(&mut block, "thinkingSignature");
                    }
                    block
                })
                .collect();
            message.body.insert("content", W::Array(normalized));
            message.body.insert("strippedToolCalls", W::Number(count as f64));
        }
    }
}

impl SessionJournal {
    /// Fixed display message contract only: full branch chronology or collapsed
    /// kept timeline. This does not change provider context or implement TUI.
    pub fn transcript_view(&self, options: TranscriptViewOptions) -> Result<TranscriptView> {
        let branch = self.branch();
        let latest = branch.iter().rposition(|entry| entry.kind == "compaction");
        let reset = branch.iter().rposition(|entry| entry.kind == "reset_boundary");
        let mut builder = Builder::new();
        match options.mode {
            TranscriptViewMode::Full => {
                for (index, entry) in branch.iter().enumerate() {
                    if entry.kind == "compaction" {
                        builder.handle_entry(entry);
                        builder.push(
                            entry,
                            compaction_summary_dto_with_store(
                                entry,
                                SummaryViewPolicy::FullTranscript,
                                Some(index) == latest,
                                None,
                                self.blob_store.as_ref(),
                            )?,
                        );
                    } else {
                        builder.append(entry)?;
                    }
                }
            }
            TranscriptViewMode::Collapsed if reset.is_some_and(|reset| latest.is_none_or(|latest| reset > latest)) => {
                for entry in &branch[reset.unwrap() + 1..] {
                    builder.append(entry)?;
                }
            }
            TranscriptViewMode::Collapsed if latest.is_some() => {
                let index = latest.unwrap();
                let compaction = branch[index];
                let first_kept = compaction.raw.get("firstKeptEntryId").and_then(serde_json::Value::as_str);
                // Fixed tolerant display emits no kept rows when the boundary
                // is unavailable; no native replay authority is inferred here.
                if let Some(kept) = branch[..index].iter().position(|entry| Some(entry.id.as_str()) == first_kept) {
                    for entry in &branch[kept..index] {
                        builder.append(entry)?;
                    }
                }
                builder.handle_entry(compaction);
                builder.push(
                    compaction,
                    compaction_summary_dto(compaction, SummaryViewPolicy::CollapsedTranscript, true, None)?,
                );
                for entry in &branch[index + 1..] {
                    builder.append(entry)?;
                }
            }
            TranscriptViewMode::Collapsed => {
                for entry in &branch {
                    builder.append(entry)?;
                }
            }
        }
        if !options.keep_dangling_tool_calls {
            builder.strip_dangling();
        }
        Ok(builder.view)
    }
}
