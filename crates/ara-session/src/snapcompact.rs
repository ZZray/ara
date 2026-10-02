//! Fixed OMP 596f2da snapcompact archive projection and checked publication.
//! Source: agent compaction/messages.ts, compaction.ts, session-context.ts and
//! session-maintenance.ts (MIT; THIRD_PARTY_NOTICES.md).
use crate::{
    CompactionCommitError, CompactionSummaryView, Entry, NativeProjectedCompactionSnapshot, RewritePhaseError,
    SessionError, SessionJournal, SessionReductionError, SessionReductionSnapshot,
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

/// One converter is shared by pre-publication sizing and reopened replay.
/// Markers are runtime-only; archived frames never become original user input.
pub fn model_message_from_summary(summary: &str, preserve_data: Option<&Value>) -> Message {
    let mut content = vec![UserBlock::text(summary)];
    if let Some(archive) = ara_snapcompact::get_preserved_archive(preserve_data) {
        content.extend(
            ara_snapcompact::history_blocks(
                &archive,
                HistoryBlockOptions { max_frame_data_bytes: Some(FRAME_DATA_BYTES_BUDGET) },
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
    Message::User(UserMessage { content: UserContent::Blocks(content), synthetic: None, timestamp: 0 })
}

impl CompactionSummaryView {
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
            .filter(|summary| summary.archive.is_some())
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
            .filter(|entry| {
                entry.kind == "compaction"
                    && ara_snapcompact::get_preserved_archive(entry.raw.get("preserveData")).is_some()
            })
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
        if ara_snapcompact::get_preserved_archive(prepared.preserve_data.as_ref()).is_none()
            || prepared.preserve_data.as_ref().and_then(|data| data.get("openaiRemoteCompaction")).is_some()
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
        candidate.push(Entry { id: id.clone(), parent_id, kind: "compaction".into(), raw });
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
        assert_eq!(reopened.model_context(), journal.model_context());
        assert!(!journal.rewrite_required, "later writers must retain the published candidate");
    }
}
