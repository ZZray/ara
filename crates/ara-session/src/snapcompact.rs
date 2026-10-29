//! Fixed OMP 596f2da snapcompact archive projection and checked publication.
//! Source: agent compaction/messages.ts, compaction.ts, session-context.ts and
//! session-maintenance.ts (MIT; THIRD_PARTY_NOTICES.md).
use crate::{
    CompactionCommitError, CompactionSummaryView, Entry, NativeMessageSource, NativeProjectedCompactionSnapshot,
    NativeSnapcompactArchiveSource, RewritePhaseError, SessionError, SessionJournal, SessionReductionError,
    SessionReductionSnapshot,
};
use ara_ai::{ImageContent, Message, UserBlock, UserContent, UserMessage};
use ara_snapcompact::{HistoryBlock, HistoryBlockOptions};
use serde_json::{Value, json};
use std::path::Path;

pub const FRAME_DATA_BYTES_BUDGET: usize = 3_000_000;

#[derive(Clone, Debug, PartialEq)]
pub struct NativeSnapcompactSnapshot {
    pub projection: NativeProjectedCompactionSnapshot,
    pub raw: SessionReductionSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeSnapcompactSummary {
    pub summary: String,
    pub short_summary: Option<String>,
    pub preserve_data: Option<Value>,
    /// Exact canonical result; the scalar slot is only a checked mirror.
    pub preserve_source: Option<NativeSnapcompactArchiveSource>,
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum NativeSnapcompactError {
    #[error(transparent)]
    Validation(#[from] CompactionCommitError),
    #[error(transparent)]
    RawSnapshot(#[from] SessionReductionError),
    #[error("snapcompact result has no readable archive")]
    InvalidArchive,
    #[error("snapcompact file lists are not native read-only/modified lists")]
    InvalidFileDetails,
    #[error("snapcompact rescue no longer owns the active archive")]
    StaleArchive,
    #[error(transparent)]
    Storage(#[from] SessionError),
    #[error("snapcompact {entry_id} was published, but directory durability is unknown: {source}")]
    PublishedButDurabilityUnknown {
        entry_id: String,
        #[source]
        source: SessionError,
    },
}

impl NativeSnapcompactError {
    pub fn history_published(&self) -> bool {
        matches!(self, Self::PublishedButDurabilityUnknown { .. })
    }
}

impl From<std::io::Error> for NativeSnapcompactError {
    fn from(error: std::io::Error) -> Self {
        Self::Storage(SessionError::Io(error))
    }
}

// Fixed session-context.ts:26-65. This is Session request policy, not package
// archive admission. Transcript owners retain the original archive unchanged.
fn model_archive_history_options(
    has_legacy_frames: bool,
    frame_count: usize,
    frame_bytes: impl FnOnce() -> usize,
    truncated_chars: f64,
    archive_text_units: impl FnOnce() -> usize,
) -> HistoryBlockOptions {
    let risk = has_legacy_frames
        && (frame_count >= 16 || frame_bytes() >= FRAME_DATA_BYTES_BUDGET)
        && (frame_count >= 16 || truncated_chars >= 1_000_000.0 || archive_text_units() >= 250_000);
    HistoryBlockOptions { max_frame_data_bytes: Some(if risk { 0 } else { FRAME_DATA_BYTES_BUDGET }) }
}

pub(super) fn native_model_archive_history_options(
    archive: &ara_snapcompact::ArchiveUtf16,
) -> std::io::Result<HistoryBlockOptions> {
    native_model_archive_history_options_with_store(archive, None, None)
}

pub(super) fn native_model_archive_history_options_with_store(
    archive: &ara_snapcompact::ArchiveUtf16,
    store: Option<&crate::BlobStore>,
    requested: Option<HistoryBlockOptions>,
) -> std::io::Result<HistoryBlockOptions> {
    let mut has_legacy_frames = false;
    for frame in &archive.frames {
        // JSON null is a present value. Only absent fields correspond to the
        // fixed persisted-source font/variant === undefined checks.
        let source = ara_rpc::WireValue::parse(frame.source.get())
            .map_err(|error| crate::native_message::invalid(error.to_string()))?;
        if source.get("font").is_none() && source.get("variant").is_none() {
            has_legacy_frames = true;
            break;
        }
    }
    let bytes = if has_legacy_frames && archive.frames.len() < 16 {
        archive.frames.iter().try_fold(0usize, |total, frame| {
            let bytes = match store {
                Some(store) => store
                    .lazy_frame_data_utf16(&frame.data)
                    .map_err(|error| crate::native_message::invalid(error.to_string()))?
                    .map_or(frame.data.len(), |lazy| lazy.bytes),
                None => frame.data.len(),
            };
            Ok::<_, std::io::Error>(total.saturating_add(bytes))
        })?
    } else {
        0
    };
    let legacy = model_archive_history_options(
        has_legacy_frames,
        archive.frames.len(),
        || bytes,
        archive.truncated_chars,
        || ara_snapcompact::archive_source_text_utf16(archive).map_or(0, |text| text.len()),
    );
    if legacy.max_frame_data_bytes == Some(0) {
        return Ok(legacy);
    }
    Ok(requested.unwrap_or(legacy))
}

/// One converter is shared by pre-publication sizing and reopened replay.
/// Markers are runtime-only; archived frames never become original user input.
pub fn model_message_from_summary(summary: &str, preserve_data: Option<&Value>) -> Message {
    let mut content = vec![UserBlock::text(summary)];
    if let Some(archive) = ara_snapcompact::get_preserved_archive(preserve_data) {
        content.extend(
            ara_snapcompact::history_blocks(
                &archive,
                model_archive_history_options(
                    archive
                        .frames
                        .iter()
                        .any(|frame| !frame.metadata.contains_key("font") && !frame.metadata.contains_key("variant")),
                    archive.frames.len(),
                    || ara_snapcompact::frame_data_bytes(&archive.frames),
                    archive.truncated_chars,
                    || ara_snapcompact::archive_source_text(&archive).map_or(0, |text| text.encode_utf16().count()),
                ),
            )
            .into_iter()
            .map(|block| match block {
                HistoryBlock::Text { text } => UserBlock::text(text),
                HistoryBlock::Image { data, mime_type, detail } => {
                    UserBlock::Image(ImageContent { data, mime_type, detail, compaction_frame: true })
                }
            }),
        );
    }
    Message::User(UserMessage {
        content: UserContent::Blocks(content),
        synthetic: None,
        timestamp: 0,
        provider_payload: None,
        provider_payload_origin: None,
    })
}

pub(super) fn native_archive_history_blocks(
    archive: &ara_snapcompact::ArchiveUtf16,
    options: HistoryBlockOptions,
) -> std::io::Result<Vec<ara_rpc::WireValue>> {
    native_archive_history_blocks_with_store(archive, options, None)
}

pub(super) fn native_archive_history_blocks_with_store(
    archive: &ara_snapcompact::ArchiveUtf16,
    options: HistoryBlockOptions,
    store: Option<&crate::BlobStore>,
) -> std::io::Result<Vec<ara_rpc::WireValue>> {
    use ara_rpc::{WireString, WireValue as W};
    let blocks = match store {
        Some(store) => ara_snapcompact::history_blocks_with_resolver_utf16(archive, options, |data| {
            store.lazy_frame_data_utf16(data)
        }),
        None => ara_snapcompact::history_blocks_utf16(archive, options),
    }
    .map_err(|error| crate::native_message::invalid(error.to_string()))?;
    blocks
        .into_iter()
        .map(|block| {
            Ok(match block {
                ara_snapcompact::HistoryBlockUtf16::Text { text } => W::object(vec![
                    ("type", W::String("text".into())),
                    ("text", W::String(WireString::from_units(text.as_units().to_vec()))),
                ]),
                ara_snapcompact::HistoryBlockUtf16::Image { data, mime_type, detail } => {
                    let mut image = W::object(vec![
                        ("type", W::String("image".into())),
                        ("data", W::String(WireString::from_units(data.as_units().to_vec()))),
                        ("mimeType", W::String(WireString::from_units(mime_type.as_units().to_vec()))),
                        ("compactionFrame", W::Bool(true)),
                    ]);
                    if let Some(detail) = detail {
                        image.insert(
                            "detail",
                            W::parse(detail.get())
                                .map_err(|error| crate::native_message::invalid(error.to_string()))?,
                        );
                    }
                    image
                }
            })
        })
        .collect()
}

/// Exact model history on the existing archive/history algorithm. Wire strings
/// never pass through UserBlock's scalar String field before Graph import.
pub fn model_source_from_summary(
    summary: &str,
    preserve_data: Option<&serde_json::value::RawValue>,
) -> std::io::Result<NativeMessageSource> {
    model_source_from_summary_with_blob_store(summary, preserve_data, None, None, true, None)
}

pub fn model_source_from_summary_with_blob_store(
    summary: &str,
    preserve_data: Option<&serde_json::value::RawValue>,
    store: Option<&crate::BlobStore>,
    requested: Option<HistoryBlockOptions>,
    include_thinking: bool,
    details: Option<&ara_snapcompact::CompactionDetails>,
) -> std::io::Result<NativeMessageSource> {
    use ara_rpc::{WireString, WireValue as W};
    let text_block =
        |text: WireString| W::Object(vec![("type".into(), W::String("text".into())), ("text".into(), W::String(text))]);
    let mut view_summary = ara_snapcompact::Utf16Text::from(summary);
    let archive = ara_snapcompact::get_preserved_archive_utf16(preserve_data)
        .map_err(|error| crate::native_message::invalid(error.to_string()))?;
    let archive = if !include_thinking {
        archive
            .map(|archive| ara_snapcompact::view_without_thinking_utf16(&view_summary, &archive, details))
            .transpose()
            .map_err(|error| crate::native_message::invalid(error.to_string()))?
            .map(|view| {
                view_summary = view.summary;
                view.archive
            })
    } else {
        archive
    };
    let mut content = vec![text_block(WireString::from_units(view_summary.as_units().to_vec()))];
    if let Some(archive) = archive {
        let options = native_model_archive_history_options_with_store(&archive, store, requested)?;
        content.extend(native_archive_history_blocks_with_store(&archive, options, store)?);
    }
    Ok(NativeMessageSource::persisted(W::Object(vec![
        ("role".into(), W::String("user".into())),
        ("content".into(), W::Array(content)),
        ("timestamp".into(), W::Number(0.0)),
        ("attribution".into(), W::String("agent".into())),
    ])))
}

pub(super) fn model_message_from_summary_with_blob_store(
    summary: &str,
    preserve_data: Option<&Value>,
    store: Option<&crate::BlobStore>,
) -> std::io::Result<Message> {
    let preserve = preserve_data
        .map(serde_json::value::to_raw_value)
        .transpose()
        .map_err(|error| crate::native_message::invalid(error.to_string()))?;
    let source = model_source_from_summary_with_blob_store(summary, preserve.as_deref(), store, None, true, None)?;
    let value = crate::native_message::checked_serde(source.observe()?.as_ref())?;
    let mut message: Message =
        serde_json::from_value(value).map_err(|error| crate::native_message::invalid(error.to_string()))?;
    // This DTO is derived solely from the admitted archive history. The AI
    // serde boundary deliberately omits runtime-only compaction_frame, so
    // restore it here just as the original manual archive converter does.
    if let Message::User(user) = &mut message
        && let UserContent::Blocks(blocks) = &mut user.content
    {
        for block in blocks {
            if let UserBlock::Image(image) = block {
                image.compaction_frame = true;
            }
        }
    }
    Ok(message)
}

impl CompactionSummaryView {
    pub fn exact_preserve_json(&self) -> std::io::Result<Option<Box<serde_json::value::RawValue>>> {
        match &self.preserve_source {
            Some(source) => source.exact_preserve_json().map(Some),
            None => self
                .preserve_data
                .as_ref()
                .map(serde_json::value::to_raw_value)
                .transpose()
                .map_err(|error| crate::native_message::invalid(error.to_string())),
        }
    }
    pub fn exact_archive(&self) -> std::io::Result<Option<ara_snapcompact::ArchiveUtf16>> {
        ara_snapcompact::get_preserved_archive_utf16(self.exact_preserve_json()?.as_deref())
            .map_err(|error| crate::native_message::invalid(error.to_string()))
    }
    pub fn has_archive(&self) -> bool {
        self.exact_archive().ok().flatten().is_some()
    }
    pub fn archive_migration_text_utf16(&self) -> std::io::Result<Option<ara_snapcompact::Utf16Text>> {
        Ok(self.exact_archive()?.as_ref().and_then(ara_snapcompact::archive_source_text_utf16))
    }
    pub fn previous_summary_for_text_compaction_utf16(&self) -> std::io::Result<ara_snapcompact::Utf16Text> {
        let mut output = ara_snapcompact::Utf16Text::from(self.summary.as_str());
        if let Some(text) = self.archive_migration_text_utf16()? {
            output.push_str("\n\n");
            let prompt = include_str!("../prompts/snapcompact-archive-context.md");
            for (index, part) in prompt.split("{{archiveText}}").enumerate() {
                if index > 0 {
                    output.append(&text);
                }
                output.push_str(part);
            }
        }
        Ok(output)
    }
    pub fn preserve_data_without_archive_utf16(&self) -> std::io::Result<Option<Box<serde_json::value::RawValue>>> {
        ara_snapcompact::strip_preserved_archive_utf16(self.exact_preserve_json()?.as_deref())
            .map_err(|error| crate::native_message::invalid(error.to_string()))
    }
    /// Bounded retained archive source, including its imaged middle, supplied
    /// to a replacement summarizer rather than its display-only caption.
    pub fn archive_migration_text(&self) -> Option<String> {
        self.archive.as_ref().and_then(ara_snapcompact::archive_source_text)
    }

    pub fn previous_summary_for_text_compaction(&self) -> String {
        match self.archive_migration_text() {
            Some(text) => format!("{}\n\n{}", self.summary, archive_migration_text(&text)),
            None => self.summary.clone(),
        }
    }

    pub fn archive_migration_message(&self) -> Option<Message> {
        self.archive_migration_text().map(|text| Message::User(UserMessage::text(archive_migration_text(&text))))
    }

    pub fn preserve_data_without_archive(&self) -> Option<Value> {
        ara_snapcompact::strip_preserved_archive(self.preserve_data.as_ref())
    }
}

fn archive_migration_text(text: &str) -> String {
    include_str!("../prompts/snapcompact-archive-context.md").replace("{{archiveText}}", text)
}

impl SessionJournal {
    pub fn native_snapcompact_snapshot(&self) -> Result<NativeSnapcompactSnapshot, NativeSnapcompactError> {
        Ok(NativeSnapcompactSnapshot {
            projection: self.native_projected_compaction_snapshot().map_err(CompactionCommitError::from)?,
            raw: self.raw_reduction_snapshot()?,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_native_entry_snapcompact(
        &mut self,
        snapshot: &NativeSnapcompactSnapshot,
        prepared: &NativeSnapcompactSummary,
        first_kept_entry_id: &str,
        window_source_entry_ids: &[String],
        tokens_before: u64,
    ) -> Result<String, NativeSnapcompactError> {
        if snapshot.raw != self.raw_reduction_snapshot()? {
            return Err(CompactionCommitError::StaleSnapshot.into());
        }
        let current = self.native_projected_compaction_snapshot().map_err(CompactionCommitError::from)?;
        let sources = self.native_entry_compaction_sources_from_current(
            &snapshot.projection,
            &current,
            &prepared.summary,
            first_kept_entry_id,
            window_source_entry_ids,
            true,
        )?;
        self.publish_snapcompact(prepared, first_kept_entry_id, Some(&sources), None, tokens_before, crate::sync_dir)
    }

    /// Re-render the exact current archive without selecting a new cut. The
    /// original kept boundary/source IDs still describe the same raw prefix.
    pub fn commit_native_snapcompact_rescue(
        &mut self,
        snapshot: &NativeSnapcompactSnapshot,
        prepared: &NativeSnapcompactSummary,
        tokens_before: u64,
    ) -> Result<String, NativeSnapcompactError> {
        if snapshot.raw != self.raw_reduction_snapshot()? {
            return Err(CompactionCommitError::StaleSnapshot.into());
        }
        let current = self.native_projected_compaction_snapshot().map_err(CompactionCommitError::from)?;
        if current != snapshot.projection {
            return Err(CompactionCommitError::StaleSnapshot.into());
        }
        let previous = current
            .previous_summary
            .as_ref()
            .filter(|summary| summary.has_archive())
            .ok_or(NativeSnapcompactError::StaleArchive)?;
        self.publish_snapcompact(
            prepared,
            &previous.first_kept_entry_id,
            previous.source_entry_ids.as_deref(),
            Some(&previous.entry_id),
            tokens_before,
            crate::sync_dir,
        )
    }

    /// Only the actual published leaf receives the no-headroom receipt.
    pub fn stamp_native_snapcompact_warning(
        &mut self,
        entry_id: &str,
        warning: &str,
    ) -> Result<(), NativeSnapcompactError> {
        if self.leaf_id() != Some(entry_id) {
            return Err(NativeSnapcompactError::StaleArchive);
        }
        let mut candidate = self.entries.clone();
        let entry = candidate
            .iter_mut()
            .find(|entry| entry.id == entry_id)
            .filter(|entry| entry.kind == "compaction" && entry.has_archive())
            .ok_or(NativeSnapcompactError::StaleArchive)?;
        entry.raw["warning"] = json!(warning);
        self.publish_snapcompact_candidate(
            candidate,
            entry_id.to_owned(),
            self.leaf.clone(),
            self.ids.clone(),
            crate::sync_dir,
        )?;
        Ok(())
    }

    fn publish_snapcompact(
        &mut self,
        prepared: &NativeSnapcompactSummary,
        kept: &str,
        sources: Option<&[String]>,
        archive_source_entry_id: Option<&str>,
        tokens_before: u64,
        directory_sync: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<String, NativeSnapcompactError> {
        if prepared.summary.trim().is_empty() || prepared.summary.len() > 1_000_000 {
            return Err(CompactionCommitError::InvalidSummary.into());
        }
        let preserve_source = match (&prepared.preserve_source, &prepared.preserve_data) {
            (Some(source), Some(mirror)) => {
                let mirror = crate::native_message::wire_from_serde(mirror)?;
                if !source.observe().deep_equal(&mirror) {
                    return Err(NativeSnapcompactError::InvalidArchive);
                }
                Some(source.clone())
            }
            (Some(source), None) => Some(source.clone()),
            (None, Some(preserve)) => {
                Some(NativeSnapcompactArchiveSource::from_wire(crate::native_message::wire_from_serde(preserve)?))
            }
            (None, None) => None,
        };
        if preserve_source.as_ref().map(|source| source.exact_archive()).transpose()?.flatten().is_none()
            || preserve_source.as_ref().is_some_and(|source| source.observe().get("openaiRemoteCompaction").is_some())
        {
            return Err(NativeSnapcompactError::InvalidArchive);
        }
        let details = json!({"readFiles":prepared.read_files,"modifiedFiles":prepared.modified_files});
        if crate::handoff::native_file_details(&json!({"details":details})).is_none() {
            return Err(NativeSnapcompactError::InvalidFileDetails);
        }
        let id = crate::generate_id(&self.ids);
        let parent_id = self.leaf.clone();
        let mut raw = json!({"type":"compaction","id":id,"parentId":parent_id,"timestamp":crate::now_iso(),
            "method":"snapcompact","summary":prepared.summary,"firstKeptEntryId":kept,
            "tokensBefore":tokens_before,"fromExtension":false,
            "preserveData":prepared.preserve_data,"details":details});
        raw.as_object_mut().expect("compaction object").remove("preserveData");
        if let Some(source) = &preserve_source {
            if let Ok(preserve) = source.checked_preserve() {
                raw["preserveData"] = preserve;
            }
        }
        if let Some(sources) = sources {
            raw["sourceEntryIds"] = json!(sources);
        }
        if let Some(source) = archive_source_entry_id {
            raw["archiveSourceEntryId"] = json!(source);
        }
        if let Some(short) = &prepared.short_summary {
            raw["shortSummary"] = json!(short);
        }
        let mut candidate = self.entries.clone();
        candidate.push(Entry {
            id: id.clone(),
            parent_id,
            kind: "compaction".into(),
            raw,
            native_message: None,
            snapcompact_source: preserve_source,
        });
        let mut ids = self.ids.clone();
        ids.insert(id.clone());
        self.publish_snapcompact_candidate(candidate, id.clone(), Some(id), ids, directory_sync)
    }

    fn publish_snapcompact_candidate(
        &mut self,
        candidate: Vec<Entry>,
        entry_id: String,
        leaf: Option<String>,
        ids: std::collections::HashSet<String>,
        directory_sync: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<String, NativeSnapcompactError> {
        let old_entries = std::mem::replace(&mut self.entries, candidate);
        let old_leaf = std::mem::replace(&mut self.leaf, leaf);
        let old_ids = std::mem::replace(&mut self.ids, ids);
        let old_materialized = self.materialized;
        let old_rewrite = self.rewrite_required;
        match self.rewrite_with_directory_sync(directory_sync) {
            Ok(()) => Ok(entry_id),
            Err(RewritePhaseError::BeforePublication(source)) => {
                self.entries = old_entries;
                self.leaf = old_leaf;
                self.ids = old_ids;
                self.materialized = old_materialized;
                self.rewrite_required = old_rewrite;
                Err(NativeSnapcompactError::Storage(source))
            }
            Err(RewritePhaseError::PublishedButDurabilityUnknown(source)) => {
                self.materialized = true;
                self.rewrite_required = false;
                Err(NativeSnapcompactError::PublishedButDurabilityUnknown { entry_id, source })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_ai::{AssistantBlock, AssistantMessage};

    #[test]
    fn published_archive_survives_directory_durability_failure_and_is_not_rolled_back() {
        let directory = tempfile::tempdir().unwrap();
        let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
        let mut sources = Vec::new();
        let mut kept = String::new();
        for index in 0..2 {
            let user = journal.append_message(&Message::User(UserMessage::text(format!("question {index}")))).unwrap();
            let mut answer = AssistantMessage::empty("openai-completions", "fixture", "m");
            answer.content.push(AssistantBlock::text("answer"));
            let assistant = journal.append_message(&Message::Assistant(answer)).unwrap();
            if index == 0 {
                sources.extend([user, assistant]);
            } else {
                kept = user;
            }
        }
        let old = journal.entries.clone();
        let prepared = NativeSnapcompactSummary {
            summary: "text-only archive".into(),
            short_summary: None,
            preserve_data: Some(
                json!({"snapcompact":{"frames":[],"text":"full retained source","textHead":"full retained source"}}),
            ),
            preserve_source: None,
            read_files: vec![],
            modified_files: vec![],
        };
        let result = journal.publish_snapcompact(&prepared, &kept, Some(&sources), None, 100, |_| {
            Err(std::io::Error::other("controlled directory sync failure"))
        });
        let error = result.unwrap_err();
        assert!(error.history_published());
        let NativeSnapcompactError::PublishedButDurabilityUnknown { entry_id, .. } = error else {
            panic!("published phase")
        };
        assert_eq!(journal.leaf_id(), Some(entry_id.as_str()));
        assert_eq!(&journal.entries[..old.len()], old);
        let reopened = SessionJournal::open(journal.path()).unwrap();
        assert_eq!(reopened.entries(), journal.entries());
        assert_eq!(reopened.model_context().unwrap(), journal.model_context().unwrap());
        assert!(!journal.rewrite_required, "later writers must retain the published candidate");
    }
}
