//! Checked raw history rewrites for fixed OMP 596f2da local maintenance.
//! Source: session-maintenance.ts:463-729 and compaction/{pruning,shake}.ts.
//! Core plans raw-slot edits; Session preserves original IDs and every field
//! outside those edits. Provider conversion fragments never become raw input.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::{
    CompactionSourceError, Entry, FailedAssistantRecovery, LoopGuardNotice, RewritePhaseError, SessionError,
    SessionJournal, sync_dir,
};

#[derive(Clone, Debug, PartialEq)]
pub struct SessionReductionSnapshot {
    pub session_id: String,
    pub leaf_id: Option<String>,
    /// Complete selected raw branch, including entries before reset/compaction.
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SessionReductionSlot {
    MessageContent,
    CustomContent,
    ToolResultDetailsImages,
    FileMentionImage(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionReductionAction {
    ReplaceToolResultContent {
        text: String,
        pruned_at_ms: i64,
    },
    ElideToolResultText {
        text: String,
        pruned_at_ms: i64,
    },
    ReplaceTextRange {
        slot: SessionReductionSlot,
        block_index: Option<usize>,
        start_utf16: usize,
        end_utf16: usize,
        expected_text: Arc<str>,
        replacement: String,
    },
    /// Fixed thinking shake: only Assistant thinking/redactedThinking blocks.
    DropBlocks {
        slot: SessionReductionSlot,
        indexes: Vec<usize>,
    },
    DropImages {
        slot: SessionReductionSlot,
        indexes: Vec<usize>,
        placeholder_if_empty: bool,
    },
    /// Host supplies only net savings before its known provider usage anchor.
    /// Initial snapshot values require Host prompt/schema/epoch ownership.
    RecordAnchoredHistoryRewrite {
        tokens_removed: usize,
        snapshot_if_missing: Option<Value>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionReductionEdit {
    pub entry_id: String,
    pub action: SessionReductionAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionReductionReceipt {
    pub session_id: String,
    pub leaf_id: Option<String>,
    pub changed_entry_ids: Vec<String>,
    pub materialized: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionReductionError {
    #[error(transparent)]
    Source(#[from] CompactionSourceError),
    #[error("reduction snapshot no longer matches the current raw Session branch")]
    StaleSnapshot,
    #[error("invalid raw reduction edit for {entry_id}: {reason}")]
    InvalidEdit { entry_id: String, reason: &'static str },
    #[error(transparent)]
    Storage(#[from] SessionError),
    #[error("history rewrite was published, but directory durability is unknown: {source}")]
    PublishedButDurabilityUnknown {
        receipt: Box<SessionReductionReceipt>,
        #[source]
        source: SessionError,
    },
}

impl SessionReductionError {
    /// The Host must rebuild from the retained candidate and fail-stop new
    /// admission after this error. It must not roll back or replay tools.
    pub fn history_published(&self) -> bool {
        matches!(self, Self::PublishedButDurabilityUnknown { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ReductionTarget {
    Raw(SessionReductionSlot),
    AnchoredHistoryRewrite,
}

impl SessionJournal {
    /// Strict raw source for all local reducers. Fixed `/shake images` and
    /// thinking inspect the full branch; prune/elide apply their own retained
    /// compaction boundary. Empty and lazy journals need no fabricated leaf.
    pub fn raw_reduction_snapshot(&self) -> std::result::Result<SessionReductionSnapshot, SessionReductionError> {
        let entries = self.strict_raw_branch()?.into_iter().cloned().collect();
        Ok(SessionReductionSnapshot { session_id: self.session_id().to_owned(), leaf_id: self.leaf.clone(), entries })
    }

    /// Apply a complete pinned plan once. All validation and candidate edits
    /// precede the atomic file publication. Off-branch entries remain untouched.
    pub fn commit_reduction(
        &mut self,
        snapshot: &SessionReductionSnapshot,
        edits: &[SessionReductionEdit],
    ) -> std::result::Result<SessionReductionReceipt, SessionReductionError> {
        self.commit_reduction_with_directory_sync(snapshot, edits, sync_dir)
    }

    /// The failed-turn owner is exact before any edit. A real local rewrite
    /// atomically publishes the native discard marker with its changed slots,
    /// retaining the failed raw receipt off branch. The token then pins only
    /// the rewritten original parent entries, so existing strict `finish`
    /// sees the matching durable marker without admitting arbitrary rebases.
    pub fn commit_reduction_during_recovery(
        &mut self,
        snapshot: &SessionReductionSnapshot,
        edits: &[SessionReductionEdit],
        recovery: &mut FailedAssistantRecovery,
    ) -> std::result::Result<SessionReductionReceipt, SessionReductionError> {
        self.commit_reduction_transaction(snapshot, edits, Some(recovery), sync_dir)
    }

    fn commit_reduction_with_directory_sync(
        &mut self,
        snapshot: &SessionReductionSnapshot,
        edits: &[SessionReductionEdit],
        directory_sync: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> std::result::Result<SessionReductionReceipt, SessionReductionError> {
        self.commit_reduction_transaction(snapshot, edits, None, directory_sync)
    }

    fn commit_reduction_transaction(
        &mut self,
        snapshot: &SessionReductionSnapshot,
        edits: &[SessionReductionEdit],
        recovery: Option<&mut FailedAssistantRecovery>,
        directory_sync: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> std::result::Result<SessionReductionReceipt, SessionReductionError> {
        if snapshot != &self.raw_reduction_snapshot()? {
            return Err(SessionReductionError::StaleSnapshot);
        }
        let existing_recovery_marker = if let Some(owner) = recovery.as_deref() {
            let count = owner.parent_branch.len();
            let expected_count = count + usize::from(owner.local_history_rewritten);
            let valid_prefix =
                snapshot.entries.len() == expected_count && snapshot.entries[..count] == owner.parent_branch;
            let valid_tail = valid_prefix
                && snapshot.entries[count..].iter().all(|entry| {
                    entry.is_discarded_entry_branch_marker()
                        && entry.raw.pointer("/details/discardedEntryId").and_then(Value::as_str)
                            == Some(owner.failed_entry.id.as_str())
                });
            if owner.session_id != self.session_id()
                || !valid_prefix
                || !valid_tail
                || (snapshot.entries.len() == count && snapshot.leaf_id != owner.failed_entry.parent_id)
                || self.entries.iter().find(|entry| entry.id == owner.failed_entry.id) != Some(&owner.failed_entry)
            {
                return Err(invalid(
                    &owner.failed_entry.id,
                    "local recovery no longer owns its exact parent and failed receipt",
                ));
            }
            snapshot.entries.len() > count
        } else {
            false
        };
        let allowed: HashSet<&str> = snapshot.entries.iter().map(|entry| entry.id.as_str()).collect();
        let mut candidate = self.entries.clone();
        let candidate_indexes: HashMap<String, usize> =
            candidate.iter().enumerate().map(|(index, entry)| (entry.id.clone(), index)).collect();
        let mut targets: HashMap<(String, ReductionTarget), Vec<&SessionReductionAction>> = HashMap::new();
        for edit in edits {
            if !allowed.contains(edit.entry_id.as_str()) {
                return Err(invalid(&edit.entry_id, "entry is outside the pinned branch"));
            }
            let slot = action_slot(&edit.action);
            targets.entry((edit.entry_id.clone(), slot)).or_default().push(&edit.action);
        }
        // A content slot accepts either one whole-slot edit or a set of
        // non-overlapping ranges. Multiple slots of one entry stay independent.
        for ((entry_id, target), actions) in targets {
            let index = *candidate_indexes.get(&entry_id).expect("pinned branch entry exists");
            let entry = &mut candidate[index];
            if actions.iter().all(|action| matches!(action, SessionReductionAction::ReplaceTextRange { .. })) {
                let ReductionTarget::Raw(slot) = target else {
                    unreachable!("text edit has a raw slot");
                };
                replace_ranges(&mut entry.raw, &entry_id, &slot, &actions)?;
            } else {
                if actions.len() != 1 {
                    return Err(invalid(&entry_id, "overlapping whole-content edits"));
                }
                apply_action(&mut entry.raw, &entry_id, actions[0])?;
            }
        }
        let changed_entry_ids: Vec<String> = snapshot
            .entries
            .iter()
            .filter(|entry| candidate[candidate_indexes[&entry.id]].raw != entry.raw)
            .map(|entry| entry.id.clone())
            .collect();
        let mut receipt = SessionReductionReceipt {
            session_id: self.session_id().to_owned(),
            leaf_id: self.leaf.clone(),
            changed_entry_ids,
            materialized: self.materialized,
        };
        if receipt.changed_entry_ids.is_empty() {
            return Ok(receipt);
        }
        let marker = if !existing_recovery_marker && let Some(owner) = recovery.as_deref() {
            let id = crate::generate_id(&self.ids);
            let parent_id = self.leaf.clone();
            let raw = json!({"type":"branch_summary","id":id,"parentId":parent_id,"timestamp":crate::now_iso(),
                "fromId":parent_id.as_deref().unwrap_or("root"),"summary":"",
                "details":{"kind":crate::DISCARDED_ENTRY_BRANCH_MARKER,"discardedEntryId":owner.failed_entry.id}});
            candidate.push(Entry { id: id.clone(), parent_id, kind: "branch_summary".into(), raw });
            Some(id)
        } else {
            None
        };
        let previous_leaf = self.leaf.clone();
        let previous = std::mem::replace(&mut self.entries, candidate);
        if let Some(marker) = &marker {
            self.ids.insert(marker.clone());
            self.leaf = Some(marker.clone());
            receipt.leaf_id = self.leaf.clone();
        }
        let result = match self.rewrite_with_directory_sync(directory_sync) {
            Ok(()) => {
                receipt.materialized = self.materialized;
                Ok(receipt)
            }
            Err(RewritePhaseError::BeforePublication(error)) => {
                self.entries = previous;
                self.leaf = previous_leaf;
                if let Some(marker) = &marker {
                    self.ids.remove(marker);
                }
                Err(SessionReductionError::Storage(error))
            }
            Err(RewritePhaseError::PublishedButDurabilityUnknown(source)) => {
                // The candidate is now the actual file. Keep memory coherent;
                // the Host is responsible for surfacing this uncertain barrier.
                self.materialized = true;
                self.rewrite_required = false;
                receipt.materialized = true;
                Err(SessionReductionError::PublishedButDurabilityUnknown { receipt: Box::new(receipt), source })
            }
        };
        if (result.is_ok() || result.as_ref().is_err_and(|error| error.history_published()))
            && let Some(owner) = recovery
        {
            owner.parent_branch = snapshot.entries[..owner.parent_branch.len()]
                .iter()
                .map(|entry| self.entries[candidate_indexes[&entry.id]].clone())
                .collect();
            owner.local_history_rewritten = true;
        }
        result
    }
}

fn invalid(entry_id: &str, reason: &'static str) -> SessionReductionError {
    SessionReductionError::InvalidEdit { entry_id: entry_id.into(), reason }
}

fn action_slot(action: &SessionReductionAction) -> ReductionTarget {
    ReductionTarget::Raw(match action {
        SessionReductionAction::ReplaceToolResultContent { .. }
        | SessionReductionAction::ElideToolResultText { .. } => SessionReductionSlot::MessageContent,
        SessionReductionAction::ReplaceTextRange { slot, .. }
        | SessionReductionAction::DropBlocks { slot, .. }
        | SessionReductionAction::DropImages { slot, .. } => slot.clone(),
        SessionReductionAction::RecordAnchoredHistoryRewrite { .. } => return ReductionTarget::AnchoredHistoryRewrite,
    })
}

fn slot_value<'a>(raw: &'a Value, slot: &SessionReductionSlot) -> Option<&'a Value> {
    match slot {
        SessionReductionSlot::MessageContent if raw["type"] == "message" => raw.pointer("/message/content"),
        SessionReductionSlot::CustomContent if raw["type"] == "custom_message" => raw.get("content"),
        SessionReductionSlot::ToolResultDetailsImages
            if raw["type"] == "message" && raw["message"]["role"] == "toolResult" =>
        {
            raw.pointer("/message/details/images")
        }
        SessionReductionSlot::FileMentionImage(index)
            if raw["type"] == "message" && raw["message"]["role"] == "fileMention" =>
        {
            raw.pointer("/message/files")?.as_array()?.get(*index)?.get("image")
        }
        _ => None,
    }
}

fn slot_value_mut<'a>(raw: &'a mut Value, slot: &SessionReductionSlot) -> Option<&'a mut Value> {
    slot_value(raw, slot)?;
    match slot {
        SessionReductionSlot::MessageContent => raw.pointer_mut("/message/content"),
        SessionReductionSlot::CustomContent => raw.get_mut("content"),
        SessionReductionSlot::ToolResultDetailsImages => raw.pointer_mut("/message/details/images"),
        SessionReductionSlot::FileMentionImage(index) => {
            raw.pointer_mut("/message/files")?.as_array_mut()?.get_mut(*index)?.get_mut("image")
        }
    }
}

fn text_slot(value: &Value, block_index: Option<usize>) -> Option<&str> {
    match block_index {
        None => value.as_str(),
        Some(index) => {
            let block = value.as_array()?.get(index)?;
            (block["type"] == "text").then(|| block.get("text")?.as_str()).flatten()
        }
    }
}

fn utf16_byte_index(text: &str, index: usize) -> Option<usize> {
    let mut units = 0;
    for (byte, character) in text.char_indices() {
        if units == index {
            return Some(byte);
        }
        units += character.len_utf16();
        if units > index {
            return None;
        }
    }
    (units == index).then_some(text.len())
}

fn replace_ranges(
    raw: &mut Value,
    entry_id: &str,
    slot: &SessionReductionSlot,
    actions: &[&SessionReductionAction],
) -> std::result::Result<(), SessionReductionError> {
    if !matches!(slot, SessionReductionSlot::MessageContent | SessionReductionSlot::CustomContent) {
        return Err(invalid(entry_id, "text replacement requires an original content slot"));
    }
    let reserved_notice = LoopGuardNotice::is_candidate(raw).then(|| raw.clone());
    if reserved_notice.is_some()
        && (*slot != SessionReductionSlot::CustomContent
            || actions
                .iter()
                .any(|action| !matches!(action, SessionReductionAction::ReplaceTextRange { block_index: None, .. })))
    {
        return Err(invalid(entry_id, "reserved notice reduction requires its original string content"));
    }
    let original = slot_value(raw, slot).ok_or_else(|| invalid(entry_id, "missing original content slot"))?.clone();
    let mut ranges: HashMap<Option<usize>, Vec<(usize, usize, &str)>> = HashMap::new();
    for action in actions {
        let SessionReductionAction::ReplaceTextRange {
            block_index,
            start_utf16,
            end_utf16,
            expected_text,
            replacement,
            ..
        } = action
        else {
            unreachable!()
        };
        let text =
            text_slot(&original, *block_index).ok_or_else(|| invalid(entry_id, "missing original text block"))?;
        if text != expected_text.as_ref() {
            return Err(invalid(entry_id, "text region no longer matches its original slot"));
        }
        if start_utf16 > end_utf16 {
            return Err(invalid(entry_id, "reversed UTF-16 range"));
        }
        let start = utf16_byte_index(text, *start_utf16)
            .ok_or_else(|| invalid(entry_id, "UTF-16 start is not a Unicode boundary"))?;
        let end = utf16_byte_index(text, *end_utf16)
            .ok_or_else(|| invalid(entry_id, "UTF-16 end is not a Unicode boundary"))?;
        ranges.entry(*block_index).or_default().push((start, end, replacement));
    }
    let target = slot_value_mut(raw, slot).expect("validated raw content slot");
    for (block_index, mut ranges) in ranges {
        ranges.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
        for pair in ranges.windows(2) {
            if pair[1].1 > pair[0].0 || (pair[1].0 == pair[0].0 && pair[1].1 == pair[0].1) {
                return Err(invalid(entry_id, "text ranges overlap"));
            }
        }
        let mut text = text_slot(&original, block_index).expect("validated original text").to_owned();
        for (start, end, replacement) in ranges {
            text.replace_range(start..end, replacement);
        }
        match block_index {
            None => *target = json!(text),
            Some(index) => target.as_array_mut().expect("validated original array")[index]["text"] = json!(text),
        }
    }
    if let Some(original_notice) = reserved_notice {
        let ranges: Vec<_> = actions
            .iter()
            .map(|action| match action {
                SessionReductionAction::ReplaceTextRange { start_utf16, end_utf16, replacement, .. } => {
                    (*start_utf16, *end_utf16, replacement.as_str())
                }
                _ => unreachable!("text actions already validated"),
            })
            .collect();
        LoopGuardNotice::record_checked_reduction(&original_notice, raw, &ranges).ok_or_else(|| {
            invalid(entry_id, "reserved notice requires a valid original and complete native shake region proof")
        })?;
    }
    Ok(())
}

fn validate_indexes(
    entry_id: &str,
    blocks: &[Value],
    indexes: &[usize],
    allowed: impl Fn(&Value) -> bool,
) -> std::result::Result<HashSet<usize>, SessionReductionError> {
    let mut selected = HashSet::new();
    if indexes.is_empty() {
        return Err(invalid(entry_id, "empty block removal"));
    }
    for index in indexes {
        if !selected.insert(*index) || blocks.get(*index).is_none_or(|block| !allowed(block)) {
            return Err(invalid(entry_id, "block removal does not match original block types"));
        }
    }
    Ok(selected)
}

fn apply_action(
    raw: &mut Value,
    entry_id: &str,
    action: &SessionReductionAction,
) -> std::result::Result<(), SessionReductionError> {
    match action {
        SessionReductionAction::RecordAnchoredHistoryRewrite { tokens_removed, snapshot_if_missing } => {
            if raw["type"] != "message"
                || raw["message"]["role"] != "assistant"
                || *tokens_removed == 0
                || matches!(raw["message"]["stopReason"].as_str(), Some("aborted" | "error"))
            {
                return Err(invalid(
                    entry_id,
                    "history rewrite requires a settled assistant usage anchor and positive savings",
                ));
            }
            let message = raw.get_mut("message").and_then(Value::as_object_mut).expect("validated assistant object");
            if message.get("contextSnapshot").is_none_or(Value::is_null) {
                let initial = snapshot_if_missing
                    .as_ref()
                    .and_then(Value::as_object)
                    .ok_or_else(|| invalid(entry_id, "missing Host-owned initial context snapshot"))?;
                if ["promptTokens", "nonMessageTokens", "compactionEpoch"]
                    .iter()
                    .any(|field| initial.get(*field).and_then(Value::as_u64).is_none())
                {
                    return Err(invalid(
                        entry_id,
                        "initial anchor snapshot requires known prompt, non-message and epoch values",
                    ));
                }
                message.insert("contextSnapshot".into(), Value::Object(initial.clone()));
            }
            let snapshot = message
                .get_mut("contextSnapshot")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| invalid(entry_id, "context snapshot is not an object"))?;
            let previous = match snapshot.get("historyRewriteTokensRemoved") {
                None | Some(Value::Null) => 0,
                Some(value) => {
                    value.as_u64().ok_or_else(|| invalid(entry_id, "invalid prior anchored history correction"))?
                }
            };
            let removed = u64::try_from(*tokens_removed)
                .ok()
                .and_then(|removed| previous.checked_add(removed))
                .ok_or_else(|| invalid(entry_id, "anchored history correction overflow"))?;
            snapshot.insert("historyRewriteTokensRemoved".into(), json!(removed));
        }
        SessionReductionAction::ReplaceToolResultContent { text, pruned_at_ms }
        | SessionReductionAction::ElideToolResultText { text, pruned_at_ms } => {
            if raw["type"] != "message" || raw["message"]["role"] != "toolResult" {
                return Err(invalid(entry_id, "tool-result edit targets a different raw role"));
            }
            let content = raw["message"]["content"]
                .as_array()
                .ok_or_else(|| invalid(entry_id, "tool-result content is not an array"))?;
            let replacement = if matches!(action, SessionReductionAction::ReplaceToolResultContent { .. }) {
                vec![json!({"type":"text","text":text})]
            } else {
                let first = content
                    .iter()
                    .position(|block| {
                        block["type"] == "text" && block["text"].as_str().is_some_and(|text| !text.is_empty())
                    })
                    .ok_or_else(|| invalid(entry_id, "tool-result elision has no nonempty original text"))?;
                content
                    .iter()
                    .enumerate()
                    .filter_map(|(index, block)| {
                        if block["type"] != "text" {
                            Some(block.clone())
                        } else if index == first {
                            Some(json!({"type":"text","text":text}))
                        } else {
                            None
                        }
                    })
                    .collect()
            };
            raw["message"]["content"] = json!(replacement);
            raw["message"]["prunedAt"] = json!(pruned_at_ms);
        }
        SessionReductionAction::DropBlocks { slot, indexes } => {
            if *slot != SessionReductionSlot::MessageContent
                || raw["type"] != "message"
                || raw["message"]["role"] != "assistant"
            {
                return Err(invalid(entry_id, "thinking removal targets a different raw role"));
            }
            let blocks = slot_value(raw, slot)
                .and_then(Value::as_array)
                .ok_or_else(|| invalid(entry_id, "thinking content is not an array"))?;
            let selected = validate_indexes(entry_id, blocks, indexes, |block| {
                matches!(block["type"].as_str(), Some("thinking" | "redactedThinking"))
            })?;
            let kept: Vec<Value> = blocks
                .iter()
                .enumerate()
                .filter(|(index, _)| !selected.contains(index))
                .map(|(_, block)| block.clone())
                .collect();
            *slot_value_mut(raw, slot).expect("validated thinking slot") = json!(kept);
        }
        SessionReductionAction::DropImages { slot, indexes, placeholder_if_empty } => {
            if let SessionReductionSlot::FileMentionImage(index) = slot {
                if indexes != &[0] || *placeholder_if_empty || slot_value(raw, slot).is_none() {
                    return Err(invalid(entry_id, "invalid file-mention image removal"));
                }
                let file = raw
                    .pointer_mut("/message/files")
                    .and_then(Value::as_array_mut)
                    .and_then(|files| files.get_mut(*index))
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| invalid(entry_id, "missing original file-mention image"))?;
                file.remove("image");
                return Ok(());
            }
            let is_content = matches!(slot, SessionReductionSlot::MessageContent | SessionReductionSlot::CustomContent);
            if is_content
                && *slot == SessionReductionSlot::MessageContent
                && !matches!(
                    raw["message"]["role"].as_str(),
                    Some("user" | "developer" | "custom" | "hookMessage" | "toolResult")
                )
            {
                return Err(invalid(entry_id, "fixed image removal does not include this raw role"));
            }
            if *placeholder_if_empty != is_content {
                return Err(invalid(entry_id, "image placeholder does not match the native content slot"));
            }
            let blocks = slot_value(raw, slot)
                .and_then(Value::as_array)
                .ok_or_else(|| invalid(entry_id, "image slot is not an array"))?;
            let selected = validate_indexes(entry_id, blocks, indexes, |block| block["type"] == "image")?;
            let mut kept: Vec<Value> = blocks
                .iter()
                .enumerate()
                .filter(|(index, _)| !selected.contains(index))
                .map(|(_, block)| block.clone())
                .collect();
            if kept.is_empty() && *placeholder_if_empty {
                kept.push(json!({"type":"text","text":"[image removed]"}));
            }
            *slot_value_mut(raw, slot).expect("validated image slot") = json!(kept);
        }
        SessionReductionAction::ReplaceTextRange { .. } => unreachable!("text ranges are grouped before application"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_ai::{AssistantBlock, AssistantMessage, Message, UserMessage};
    use std::{fs, io};

    /// One publication-phase family checks actual file effects and memory,
    /// including the error after rename whose outcome must not be rolled back.
    #[test]
    fn checked_rewrite_phases_keep_disk_and_memory_coherent() {
        let directory = tempfile::tempdir().unwrap();
        let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
        let empty = journal.raw_reduction_snapshot().unwrap();
        assert!(empty.entries.is_empty());
        assert!(empty.leaf_id.is_none());
        assert!(!journal.commit_reduction(&empty, &[]).unwrap().materialized);
        assert!(!journal.path().exists());
        let id = journal.append_message(&Message::User(UserMessage::text("before"))).unwrap();
        let mut assistant = AssistantMessage::empty("openai-completions", "fixture", "m");
        assistant.content.push(AssistantBlock::text("done"));
        journal.append_message(&Message::Assistant(assistant)).unwrap();
        let snapshot = journal.raw_reduction_snapshot().unwrap();
        let edits = [SessionReductionEdit {
            entry_id: id,
            action: SessionReductionAction::ReplaceTextRange {
                slot: SessionReductionSlot::MessageContent,
                block_index: None,
                start_utf16: 0,
                end_utf16: 6,
                expected_text: Arc::from("before"),
                replacement: "after".into(),
            },
        }];
        let actual_path = journal.path.clone();
        let bytes = fs::read(&actual_path).unwrap();
        let entries = journal.entries.clone();
        let blocked = directory.path().join("blocked-parent");
        fs::write(&blocked, "not a directory").unwrap();
        journal.path = blocked.join("candidate.jsonl");
        let before =
            journal.commit_reduction_with_directory_sync(&snapshot, &edits, |_| panic!("publication never reached"));
        assert!(matches!(before, Err(SessionReductionError::Storage(_))));
        assert_eq!(journal.entries, entries);
        assert_eq!(fs::read(&actual_path).unwrap(), bytes);
        journal.path = actual_path;
        let after = journal.commit_reduction_with_directory_sync(&snapshot, &edits, |_| {
            Err(io::Error::other("injected directory-sync failure"))
        });
        let error = after.unwrap_err();
        assert!(error.history_published());
        let SessionReductionError::PublishedButDurabilityUnknown { receipt, .. } = error else {
            panic!("published outcome stays explicit");
        };
        assert!(receipt.materialized);
        assert_eq!(receipt.changed_entry_ids, [edits[0].entry_id.clone()]);
        assert_eq!(journal.entries[0].raw["message"]["content"], "after");
        assert_ne!(fs::read(journal.path()).unwrap(), bytes);
        let reopened = SessionJournal::open(journal.path()).unwrap();
        assert_eq!(journal.entries, reopened.entries);
        assert_eq!(journal.leaf, reopened.leaf);
        assert_eq!(journal.model_context(), reopened.model_context());
        assert_eq!(journal.raw_reduction_snapshot().unwrap(), reopened.raw_reduction_snapshot().unwrap());

        let mut failed = AssistantMessage::empty("openai-completions", "fixture", "m");
        failed.stop_reason = ara_ai::StopReason::Error;
        failed.content.push(AssistantBlock::text("failed receipt"));
        let failed_id = journal.append_message(&Message::Assistant(failed.clone())).unwrap();
        let mut owner = journal.begin_failed_assistant_recovery(&failed_id, &failed_id, &failed).unwrap();
        let parent = journal.raw_reduction_snapshot().unwrap();
        let unchanged = journal.commit_reduction_during_recovery(&parent, &[], &mut owner).unwrap();
        assert!(unchanged.changed_entry_ids.is_empty());
        assert!(!owner.local_history_rewritten());
        let rescue_edits = [SessionReductionEdit {
            entry_id: edits[0].entry_id.clone(),
            action: SessionReductionAction::ReplaceTextRange {
                slot: SessionReductionSlot::MessageContent,
                block_index: None,
                start_utf16: 0,
                end_utf16: 5,
                expected_text: Arc::from("after"),
                replacement: "rescued".into(),
            },
        }];
        let published = journal
            .commit_reduction_transaction(&parent, &rescue_edits, Some(&mut owner), |_| {
                Err(io::Error::other("recovery barrier"))
            })
            .unwrap_err();
        assert!(published.history_published());
        assert!(owner.local_history_rewritten());
        assert_eq!(owner.parent_branch[0].raw["message"]["content"], "rescued");
        let reopened = SessionJournal::open(journal.path()).unwrap();
        assert_eq!(reopened.entries, journal.entries);
        assert_eq!(reopened.leaf, journal.leaf);
        assert!(!reopened.branch().iter().any(|entry| entry.id == failed_id));
        assert_eq!(
            journal.finish_failed_assistant_recovery(owner, true).unwrap(),
            crate::FailedAssistantRecoveryOutcome::Committed
        );
    }
}
