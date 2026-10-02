//! Host seam between Session raw identities and Core compaction groups.
//!
//! Session and Core keep separate origin enums so neither owns the other.
//! Raw extension messages currently use the Session text projection as a
//! token estimate proxy; this is not full native tokenizer or wire-fit parity.

use ara_agent::compaction::{NativeEntryOrigin as CoreOrigin, NativeEntrySource};
use ara_agent::tokenizer::{MessageCountOptions, count_message};
use ara_session::{Entry, NativeCompactionEntry, NativeEntryOrigin as SessionOrigin};
use serde_json::{Value, json};

/// Public history preserves the native receipt, including custom images, rather
/// than substituting one provider fragment for a multi-fragment entry.
pub fn extension_event_message(entry: &Entry) -> Option<Value> {
    entry.model_messages()?;
    if entry.kind == "message" {
        let message = entry.raw.get("message")?;
        return matches!(
            message.get("role")?.as_str()?,
            "custom" | "hookMessage" | "fileMention" | "branchSummary" | "compactionSummary"
        )
        .then(|| message.clone());
    }
    let timestamp =
        chrono::DateTime::parse_from_rfc3339(entry.raw.get("timestamp")?.as_str()?).ok()?.timestamp_millis();
    match entry.kind.as_str() {
        "custom_message" => {
            let mut message = entry.raw.clone();
            let object = message.as_object_mut()?;
            for key in ["id", "parentId", "type"] {
                object.remove(key);
            }
            object.insert("role".into(), json!("custom"));
            object.insert("timestamp".into(), json!(timestamp));
            Some(message)
        }
        "branch_summary" if !entry.model_messages()?.is_empty() => Some(json!({
            "role":"branchSummary", "summary":entry.raw.get("summary")?,
            "fromId":entry.raw.get("fromId")?, "timestamp":timestamp,
        })),
        _ => None,
    }
}

pub fn sources(entries: &[NativeCompactionEntry]) -> Vec<NativeEntrySource<'_>> {
    entries
        .iter()
        .map(|entry| NativeEntrySource {
            entry_id: &entry.entry_id,
            origin: match entry.origin {
                SessionOrigin::User => CoreOrigin::User,
                SessionOrigin::Assistant => CoreOrigin::Assistant,
                SessionOrigin::ToolResult => CoreOrigin::ToolResult,
                SessionOrigin::Developer => CoreOrigin::Developer,
                SessionOrigin::BashExecution => CoreOrigin::BashExecution,
                SessionOrigin::HookMessage => CoreOrigin::HookMessage,
                SessionOrigin::FileMention => CoreOrigin::FileMention,
                SessionOrigin::LegacyCustomMessage => CoreOrigin::LegacyCustomMessage,
                SessionOrigin::LegacyBranchSummary => CoreOrigin::LegacyBranchSummary,
                SessionOrigin::LegacyCompactionSummary => CoreOrigin::LegacyCompactionSummary,
                SessionOrigin::CustomMessage => CoreOrigin::CustomMessage,
                SessionOrigin::UserSkill => CoreOrigin::UserSkill,
                SessionOrigin::SteeringUser => CoreOrigin::SteeringUser,
                SessionOrigin::LoopGuardNotice => CoreOrigin::LoopGuardNotice,
                SessionOrigin::BranchSummary => CoreOrigin::BranchSummary,
                SessionOrigin::Metadata => CoreOrigin::Metadata,
                SessionOrigin::CompactionBoundary => CoreOrigin::CompactionBoundary,
            },
            messages: &entry.messages,
            raw_message_tokens: entry
                .raw_token_message
                .as_ref()
                .map_or(0, |message| count_message(message, MessageCountOptions::default())),
        })
        .collect()
}
