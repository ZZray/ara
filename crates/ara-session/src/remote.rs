//! Checked provider-native remote compaction from fixed OMP 596f2da (MIT).
//! Source: compaction.ts::{remotePreserveReusable,findReadableCompactionIndex,
//! prepareCompaction}, openai.ts preserve-data helpers and session-maintenance.
//! Raw replaced-prefix provenance and request-covered replay tail are distinct.

use crate::{
    CompactionCommitError, CompactionProjectionError, CompactionSummaryView, Entry, NativeHandoffSummary,
    NativeProjectedCompactionSnapshot, RewritePhaseError, SessionError, SessionJournal, SessionReductionError,
    SessionReductionSnapshot,
};
use ara_ai::remote_compaction::{native_replacement_payload, parse_remote_preserve_data};
use ara_ai::{AssistantMessage, Message, Model};
use serde_json::{Value, json};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeRemoteReplay {
    pub preserve_data: Value,
    /// Last original model-visible raw entry submitted in this request,
    /// including recent history. Never a sourceEntryIds replacement.
    pub replay_through_entry_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeRemoteSnapshot {
    pub projection: NativeProjectedCompactionSnapshot,
    /// Portable text methods must use original readable sources, even when
    /// the active provider can replay an opaque native boundary.
    pub readable: NativeProjectedCompactionSnapshot,
    pub raw: SessionReductionSnapshot,
    /// Host captures the effective active route before its side request.
    /// Host must still reject route adoption races before calling commit.
    pub active_model: Model,
    pub native_replay_available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeRemoteSummary {
    pub summary: String,
    pub short_summary: Option<String>,
    pub preserve_data: Value,
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum NativeRemoteError {
    #[error(transparent)]
    Validation(#[from] CompactionCommitError),
    #[error(transparent)]
    RawSnapshot(#[from] SessionReductionError),
    #[error("remote compaction replay does not match the captured native route")]
    InvalidReplay,
    #[error("remote compaction file lists are not native unique read-only/modified lists")]
    InvalidFileDetails,
    #[error("remote replay-through ID does not cover the exact submitted active raw tail")]
    InvalidReplayThrough,
    #[error(transparent)]
    Storage(#[from] SessionError),
    #[error("remote compaction {entry_id} was published, but directory durability is unknown: {source}")]
    PublishedButDurabilityUnknown {
        entry_id: String,
        #[source]
        source: SessionError,
    },
}

impl NativeRemoteError {
    pub fn history_published(&self) -> bool {
        matches!(self, Self::PublishedButDurabilityUnknown { .. })
    }
}

fn replay_through_valid(branch: &[&Entry], end: usize, kept: usize, through: &str) -> bool {
    let Some(index) = branch[..end].iter().position(|entry| entry.id == through) else { return false };
    index >= kept
        && !branch[index + 1..end].iter().any(|entry| entry.kind == "compaction")
        && branch[..end]
            .iter()
            .rfind(|entry| entry.model_messages().is_some_and(|messages| !messages.is_empty()))
            .is_some_and(|entry| entry.id == through)
}

pub(super) fn validate_remote_entry(
    branch: &[&Entry],
    index: usize,
    kept: usize,
    entry: &Entry,
) -> Result<Option<NativeRemoteReplay>, CompactionProjectionError> {
    let invalid = |field| CompactionProjectionError::InvalidField { id: entry.id.clone(), field };
    if crate::entry_timestamp(&entry.raw).is_none() {
        return Err(invalid("timestamp"));
    }
    if crate::handoff::native_file_details(&entry.raw).is_none() {
        return Err(invalid("details"));
    }
    if !matches!(entry.raw.get("fromExtension"), None | Some(Value::Null | Value::Bool(false)))
        || ["providerPayload", "blocks", "images"]
            .iter()
            .any(|field| entry.raw.get(*field).is_some_and(|value| !value.is_null()))
    {
        return Err(CompactionProjectionError::UnsupportedReplayData { id: entry.id.clone() });
    }
    let Some(preserve) = entry.raw.get("preserveData").filter(|data| !data.is_null()) else {
        if entry.raw.get("providerReplayThroughEntryId").is_some_and(|value| !value.is_null()) {
            return Err(invalid("providerReplayThroughEntryId"));
        }
        return Ok(None);
    };
    let preserve_data = Some(preserve)
        .filter(|data| parse_remote_preserve_data(data).is_some())
        .ok_or_else(|| invalid("preserveData"))?
        .clone();
    let through = entry
        .raw
        .get("providerReplayThroughEntryId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| invalid("providerReplayThroughEntryId"))?;
    if !replay_through_valid(branch, index, kept, through) {
        return Err(invalid("providerReplayThroughEntryId"));
    }
    Ok(Some(NativeRemoteReplay { preserve_data, replay_through_entry_id: through.to_owned() }))
}

pub(super) fn reusable(remote: &NativeRemoteReplay, route: Option<(&Model, bool)>) -> bool {
    let Some((model, true)) = route else { return false };
    parse_remote_preserve_data(&remote.preserve_data).is_some_and(|data| {
        data.provider.as_deref() == Some(model.provider.as_str())
            && native_replacement_payload(model, &data.replacement_history).is_some()
    })
}

pub(super) fn model_message(summary: &CompactionSummaryView, model: &Model) -> Message {
    let remote = summary.remote.as_ref().expect("checked readable remote summary");
    let data = parse_remote_preserve_data(&remote.preserve_data).expect("checked native preserve data");
    let mut carrier = AssistantMessage::empty(&model.api, &model.provider, &model.id);
    carrier.timestamp =
        crate::entry_timestamp(&json!({"timestamp":summary.timestamp})).expect("checked native remote timestamp");
    carrier.provider_payload = native_replacement_payload(model, &data.replacement_history);
    Message::Assistant(carrier)
}

impl SessionJournal {
    pub fn native_remote_snapshot(
        &self,
        active_model: &Model,
        native_replay_available: bool,
    ) -> Result<NativeRemoteSnapshot, NativeRemoteError> {
        Ok(NativeRemoteSnapshot {
            raw: self.raw_reduction_snapshot()?,
            projection: self
                .native_projected_compaction_snapshot_for_route(active_model, native_replay_available)
                .map_err(CompactionCommitError::from)?,
            readable: self.native_projected_compaction_snapshot().map_err(CompactionCommitError::from)?,
            active_model: active_model.clone(),
            native_replay_available,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_native_entry_remote(
        &mut self,
        snapshot: &NativeRemoteSnapshot,
        prepared: &NativeRemoteSummary,
        first_kept_entry_id: &str,
        window_source_entry_ids: &[String],
        replay_through_entry_id: &str,
        tokens_before: u64,
    ) -> Result<String, NativeRemoteError> {
        self.commit_native_entry_remote_with_sync(
            snapshot,
            prepared,
            first_kept_entry_id,
            window_source_entry_ids,
            replay_through_entry_id,
            tokens_before,
            crate::sync_dir,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_native_entry_remote_with_sync(
        &mut self,
        snapshot: &NativeRemoteSnapshot,
        prepared: &NativeRemoteSummary,
        first_kept_entry_id: &str,
        window_source_entry_ids: &[String],
        replay_through_entry_id: &str,
        tokens_before: u64,
        directory_sync: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<String, NativeRemoteError> {
        if snapshot.raw != self.raw_reduction_snapshot()? {
            return Err(CompactionCommitError::StaleSnapshot.into());
        }
        let current = self
            .native_projected_compaction_snapshot_for_route(&snapshot.active_model, snapshot.native_replay_available)
            .map_err(CompactionCommitError::from)?;
        let sources = self.native_entry_compaction_sources_from_current(
            &snapshot.projection,
            &current,
            &prepared.summary,
            first_kept_entry_id,
            window_source_entry_ids,
            true,
        )?;
        let replay = NativeRemoteReplay {
            preserve_data: prepared.preserve_data.clone(),
            replay_through_entry_id: replay_through_entry_id.to_owned(),
        };
        if !reusable(&replay, Some((&snapshot.active_model, snapshot.native_replay_available))) {
            return Err(NativeRemoteError::InvalidReplay);
        }
        let details = json!({"readFiles":prepared.read_files,"modifiedFiles":prepared.modified_files});
        if crate::handoff::native_file_details(&json!({"details":details})).is_none() {
            return Err(NativeRemoteError::InvalidFileDetails);
        }
        let raw_branch = self
            .strict_compaction_branch()
            .map_err(CompactionProjectionError::from)
            .map_err(CompactionCommitError::from)?;
        let branch = &raw_branch[crate::active_context_start(&raw_branch)..];
        let kept = branch
            .iter()
            .position(|entry| entry.id == first_kept_entry_id)
            .ok_or(NativeRemoteError::InvalidReplayThrough)?;
        if !replay_through_valid(branch, branch.len(), kept, replay_through_entry_id) {
            return Err(NativeRemoteError::InvalidReplayThrough);
        }
        let id = crate::generate_id(&self.ids);
        let parent_id = self.leaf.clone();
        let mut raw = json!({
            "type":"compaction", "id":id, "parentId":parent_id, "timestamp":crate::now_iso(),
            "method":"remote", "summary":prepared.summary, "firstKeptEntryId":first_kept_entry_id,
            "sourceEntryIds":sources, "tokensBefore":tokens_before, "fromExtension":false,
            "preserveData":prepared.preserve_data, "providerReplayThroughEntryId":replay_through_entry_id,
            "details":details,
        });
        if let Some(short_summary) = &prepared.short_summary {
            raw["shortSummary"] = json!(short_summary);
        }
        self.publish_remote_entry(raw, id, parent_id, directory_sync)
    }

    /// Publish a portable remote endpoint summary through the same checked,
    /// atomic boundary as native replay. Its source preparation must have
    /// expanded opaque history, not summarized a provider-only placeholder.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_native_entry_remote_text(
        &mut self,
        snapshot: &NativeRemoteSnapshot,
        readable: &NativeProjectedCompactionSnapshot,
        prepared: &NativeHandoffSummary,
        short_summary: &str,
        first_kept_entry_id: &str,
        window_source_entry_ids: &[String],
        tokens_before: u64,
    ) -> Result<String, NativeRemoteError> {
        if snapshot.raw != self.raw_reduction_snapshot()? || readable != &snapshot.readable {
            return Err(CompactionCommitError::StaleSnapshot.into());
        }
        let current = self.native_projected_compaction_snapshot().map_err(CompactionCommitError::from)?;
        let sources = self.native_entry_compaction_sources_from_current(
            readable,
            &current,
            &prepared.summary,
            first_kept_entry_id,
            window_source_entry_ids,
            false,
        )?;
        let details = json!({"readFiles":prepared.read_files,"modifiedFiles":prepared.modified_files});
        if crate::handoff::native_file_details(&json!({"details":details})).is_none() {
            return Err(NativeRemoteError::InvalidFileDetails);
        }
        let id = crate::generate_id(&self.ids);
        let parent_id = self.leaf.clone();
        let raw = json!({
            "type":"compaction", "id":id, "parentId":parent_id, "timestamp":crate::now_iso(),
            "method":"remote", "summary":prepared.summary, "shortSummary":short_summary,
            "firstKeptEntryId":first_kept_entry_id, "sourceEntryIds":sources,
            "tokensBefore":tokens_before, "fromExtension":false, "details":details,
        });
        self.publish_remote_entry(raw, id, parent_id, crate::sync_dir)
    }

    fn publish_remote_entry(
        &mut self,
        raw: Value,
        id: String,
        parent_id: Option<String>,
        directory_sync: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<String, NativeRemoteError> {
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
                Err(NativeRemoteError::Storage(source))
            }
            Err(RewritePhaseError::PublishedButDurabilityUnknown(source)) => {
                self.materialized = true;
                self.rewrite_required = false;
                Err(NativeRemoteError::PublishedButDurabilityUnknown { entry_id: id, source })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_ai::{AssistantBlock, UserMessage};
    use std::fs;

    #[test]
    fn remote_publication_phase_failures_retain_the_correct_disk_and_memory_candidate() {
        let directory = tempfile::tempdir().unwrap();
        for fail_after_publication in [false, true] {
            let home = directory.path().join(if fail_after_publication { "published" } else { "before" });
            fs::create_dir(&home).unwrap();
            let mut journal = SessionJournal::create(&home, &home).unwrap();
            let mut ids = Vec::new();
            for label in ["complete", "kept"] {
                ids.push(journal.append_message(&Message::User(UserMessage::text(label))).unwrap());
                let mut assistant = AssistantMessage::empty("openai-responses", "openai", "model");
                assistant.content.push(AssistantBlock::text(label));
                ids.push(journal.append_message(&Message::Assistant(assistant)).unwrap());
            }
            let model = Model {
                id: "model".into(),
                api: "openai-responses".into(),
                provider: "openai".into(),
                base_url: "http://fixture.invalid/v1".into(),
                reasoning: false,
                max_tokens: None,
                context_window: None,
                tokenizer: None,
            };
            let item = json!({"type":"compaction","encrypted_content":"enc"});
            let prepared = NativeRemoteSummary {
                summary: "Remote native history".into(),
                short_summary: None,
                preserve_data: json!({"openaiRemoteCompaction":{"provider":"openai","replacementHistory":[item],"compactionItem":item}}),
                read_files: vec![],
                modified_files: vec![],
            };
            let snapshot = journal.native_remote_snapshot(&model, true).unwrap();
            let original = journal.entries().to_vec();
            let path = journal.path().to_path_buf();
            let saved = path.with_extension("original");
            let original_bytes = fs::read(&path).unwrap();
            if !fail_after_publication {
                fs::rename(&path, &saved).unwrap();
                fs::create_dir(&path).unwrap();
            }
            let error = journal
                .commit_native_entry_remote_with_sync(&snapshot, &prepared, &ids[2], &ids[..2], &ids[3], 100, |_| {
                    Err(std::io::Error::other("injected directory sync failure"))
                })
                .unwrap_err();
            assert_eq!(error.history_published(), fail_after_publication);
            if fail_after_publication {
                assert!(matches!(error, NativeRemoteError::PublishedButDurabilityUnknown { .. }));
                assert_eq!(&journal.entries()[..original.len()], original);
                assert_eq!(journal.entries().len(), original.len() + 1);
                let reopened = SessionJournal::open(&path).unwrap();
                assert_eq!(reopened.entries(), journal.entries());
                assert_eq!(
                    reopened.model_context_for_route(&model, true),
                    journal.model_context_for_route(&model, true)
                );
            } else {
                assert!(matches!(error, NativeRemoteError::Storage(_)));
                assert_eq!(journal.entries(), original);
                assert_eq!(journal.native_remote_snapshot(&model, true).unwrap(), snapshot);
                assert_eq!(fs::read(&saved).unwrap(), original_bytes);
                fs::remove_dir(&path).unwrap();
                fs::rename(&saved, &path).unwrap();
                assert_eq!(SessionJournal::open(&path).unwrap().entries(), journal.entries());
            }
        }
    }
}
