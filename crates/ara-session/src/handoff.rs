//! Checked native handoff persistence from fixed OMP 596f2da (MIT).
//! Source: session-maintenance.ts::handoff/handoffSummaryFromDocument and
//! agent/compaction/{messages.ts,prompts/handoff-summary-context.md}.
//! Core/Host prepare the document and cumulative file lists. Session owns
//! raw provenance, the retained cut, publication and the reopen projection.

use crate::{
    CompactionCommitError, CompactionSummaryView, Entry, NativeProjectedCompactionSnapshot, RewritePhaseError,
    SessionError, SessionJournal, SessionReductionError, SessionReductionSnapshot,
};
use ara_ai::Message;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeCompactionFileDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// `summary` already contains the native cumulative file-operations tag.
/// Session does not regenerate Core prompts, format file operations, or turn
/// the document into a fresh user observation. The model wrapper stays out
/// of the persisted summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeHandoffSummary {
    pub summary: String,
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// Pin both the native preparation projection and every original raw field.
/// A side request may finish after the live Session has changed; a matching
/// projected message is insufficient evidence that its source stayed exact.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeHandoffSnapshot {
    pub projection: NativeProjectedCompactionSnapshot,
    pub raw: SessionReductionSnapshot,
}

#[derive(Debug, thiserror::Error)]
pub enum NativeHandoffError {
    #[error(transparent)]
    Validation(#[from] CompactionCommitError),
    #[error(transparent)]
    RawSnapshot(#[from] SessionReductionError),
    #[error("handoff file details must contain unique read-only and modified file lists")]
    InvalidFileDetails,
    #[error(transparent)]
    Storage(#[from] SessionError),
    #[error("handoff {entry_id} was published, but directory durability is unknown: {source}")]
    PublishedButDurabilityUnknown {
        entry_id: String,
        #[source]
        source: SessionError,
    },
}

impl NativeHandoffError {
    /// Rebuild Host mirrors from the retained candidate and fail-stop after
    /// this error. A published document must not be rolled back or replayed.
    pub fn history_published(&self) -> bool {
        matches!(self, Self::PublishedButDurabilityUnknown { .. })
    }
}

fn valid_file_lists(read: &[String], modified: &[String]) -> bool {
    let reads: HashSet<&String> = read.iter().collect();
    let writes: HashSet<&String> = modified.iter().collect();
    reads.len() == read.len() && writes.len() == modified.len() && reads.is_disjoint(&writes)
}

/// Read native metadata only. Imported soft entries may have other details;
/// absent/unusable file lists stay unknown rather than being parsed from the
/// generated summary. Extension-owned lists are not native preparation data.
pub(super) fn native_file_details(raw: &Value) -> Option<NativeCompactionFileDetails> {
    if !matches!(raw.get("fromExtension"), None | Some(Value::Null | Value::Bool(false))) {
        return None;
    }
    let details = raw.get("details")?.as_object()?;
    let list = |name: &str| {
        details.get(name)?.as_array()?.iter().map(|value| value.as_str().map(str::to_owned)).collect::<Option<Vec<_>>>()
    };
    let read_files = list("readFiles")?;
    let modified_files = list("modifiedFiles")?;
    valid_file_lists(&read_files, &modified_files).then_some(NativeCompactionFileDetails { read_files, modified_files })
}

pub(super) fn model_message(summary: &CompactionSummaryView) -> Message {
    let raw = json!({"summary":summary.summary,"method":"handoff","timestamp":summary.timestamp});
    let timestamp = crate::entry_timestamp(&raw).expect("checked handoff timestamp");
    crate::summary_model_message(&raw, "compaction", timestamp).expect("checked text-only handoff summary")
}

impl SessionJournal {
    pub fn native_handoff_snapshot(&self) -> Result<NativeHandoffSnapshot, NativeHandoffError> {
        let raw = self.raw_reduction_snapshot()?;
        let projection = self.native_projected_compaction_snapshot().map_err(CompactionCommitError::from)?;
        Ok(NativeHandoffSnapshot { projection, raw })
    }

    /// Append one native handoff to this same Session. The full raw snapshot,
    /// original cut and cumulative source IDs are checked before publication.
    /// This text-only API never accepts provider preserveData or remote replay.
    /// A recovery owner may have selected a failed Assistant's parent: all
    /// parent entries stay exact, the failed receipt remains off branch, and
    /// the existing checked recovery finish sees this committed compaction.
    pub fn commit_native_entry_handoff(
        &mut self,
        snapshot: &NativeHandoffSnapshot,
        prepared: &NativeHandoffSummary,
        first_kept_entry_id: &str,
        window_source_entry_ids: &[String],
        tokens_before: u64,
    ) -> Result<String, NativeHandoffError> {
        self.commit_native_entry_handoff_with_sync(
            snapshot,
            prepared,
            first_kept_entry_id,
            window_source_entry_ids,
            tokens_before,
            crate::sync_dir,
        )
    }

    fn commit_native_entry_handoff_with_sync(
        &mut self,
        snapshot: &NativeHandoffSnapshot,
        prepared: &NativeHandoffSummary,
        first_kept_entry_id: &str,
        window_source_entry_ids: &[String],
        tokens_before: u64,
        directory_sync: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<String, NativeHandoffError> {
        if snapshot.raw != self.raw_reduction_snapshot()? {
            return Err(CompactionCommitError::StaleSnapshot.into());
        }
        let sources = self.native_entry_compaction_sources(
            &snapshot.projection,
            &prepared.summary,
            first_kept_entry_id,
            window_source_entry_ids,
        )?;
        if !valid_file_lists(&prepared.read_files, &prepared.modified_files) {
            return Err(NativeHandoffError::InvalidFileDetails);
        }
        let id = crate::generate_id(&self.ids);
        let parent_id = self.leaf.clone();
        let raw = json!({
            "type":"compaction", "id":id, "parentId":parent_id, "timestamp":crate::now_iso(),
            "method":"handoff", "summary":prepared.summary, "firstKeptEntryId":first_kept_entry_id,
            "sourceEntryIds":sources, "tokensBefore":tokens_before, "fromExtension":false,
            "details":{"readFiles":prepared.read_files,"modifiedFiles":prepared.modified_files},
        });
        let mut candidate = self.entries.clone();
        candidate.push(Entry { id: id.clone(), parent_id, kind: "compaction".into(), raw });
        let previous_entries = std::mem::replace(&mut self.entries, candidate);
        let previous_leaf = self.leaf.replace(id.clone());
        let mut candidate_ids = self.ids.clone();
        candidate_ids.insert(id.clone());
        let previous_ids = std::mem::replace(&mut self.ids, candidate_ids);
        let previous_materialized = self.materialized;
        let previous_rewrite_required = self.rewrite_required;
        match self.rewrite_with_directory_sync(directory_sync) {
            Ok(()) => Ok(id),
            Err(RewritePhaseError::BeforePublication(source)) => {
                self.entries = previous_entries;
                self.ids = previous_ids;
                self.leaf = previous_leaf;
                self.materialized = previous_materialized;
                self.rewrite_required = previous_rewrite_required;
                Err(NativeHandoffError::Storage(source))
            }
            Err(RewritePhaseError::PublishedButDurabilityUnknown(source)) => {
                self.materialized = true;
                self.rewrite_required = false;
                Err(NativeHandoffError::PublishedButDurabilityUnknown { entry_id: id, source })
            }
        }
    }
}
