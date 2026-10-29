//! Host seam between Session raw identities and Core compaction groups.
//!
//! Session and Core keep separate origin enums so neither owns the other.
//! Raw extension messages currently use the Session text projection as a
//! token estimate proxy; this is not full native tokenizer or wire-fit parity.

use ara_agent::compaction::{NativeEntryOrigin as CoreOrigin, NativeEntrySource};
use ara_agent::tokenizer::{MessageCountOptions, count_message};
use ara_ai::{Message, Model};
use ara_session::{Entry, NativeCompactionEntry, NativeEntryOrigin as SessionOrigin, Result};
use serde_json::{Value, json};

/// Public history preserves the native receipt, including custom images, rather
/// than substituting one provider fragment for a multi-fragment entry.
pub fn extension_event_message(entry: &Entry) -> Result<Option<Value>> {
    let Some(projected) = entry.model_messages()? else {
        return Ok(None);
    };
    if entry.kind == "message" {
        // Explicit checked legacy observation of the current canonical body.
        // The raw mirror is not a source for native messages.
        let raw = entry.checked_raw()?;
        let Some(message) = raw.get("message") else {
            return Ok(None);
        };
        let Some(role) = message.get("role").and_then(Value::as_str) else {
            return Ok(None);
        };
        return Ok(matches!(role, "custom" | "hookMessage" | "fileMention" | "branchSummary" | "compactionSummary")
            .then(|| message.clone()));
    }
    let Some(timestamp) = entry.raw.get("timestamp").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Ok(timestamp) = chrono::DateTime::parse_from_rfc3339(timestamp) else {
        return Ok(None);
    };
    let timestamp = timestamp.timestamp_millis();
    Ok(match entry.kind.as_str() {
        "custom_message" => {
            let mut message = entry.checked_raw()?;
            let Some(object) = message.as_object_mut() else {
                return Ok(None);
            };
            for key in ["id", "parentId", "type"] {
                object.remove(key);
            }
            object.insert("role".into(), json!("custom"));
            object.insert("timestamp".into(), json!(timestamp));
            Some(message)
        }
        "branch_summary" if !projected.is_empty() => {
            let raw = entry.checked_raw()?;
            let (Some(summary), Some(from_id)) = (raw.get("summary"), raw.get("fromId")) else {
                return Ok(None);
            };
            Some(json!({"role":"branchSummary", "summary":summary, "fromId":from_id, "timestamp":timestamp}))
        }
        _ => None,
    })
}

pub fn sources(entries: &[NativeCompactionEntry]) -> Vec<NativeEntrySource<'_>> {
    sources_with_counter(entries, |message| count_message(message, MessageCountOptions::default()))
}

pub fn sources_for_model<'a>(entries: &'a [NativeCompactionEntry], model: &Model) -> Vec<NativeEntrySource<'a>> {
    let tokenizer = crate::context_budget::tokenizer(model);
    sources_with_counter(entries, |message| tokenizer.count_message(message, MessageCountOptions::default()))
}

fn sources_with_counter<'a>(
    entries: &'a [NativeCompactionEntry],
    count: impl Fn(&Message) -> usize,
) -> Vec<NativeEntrySource<'a>> {
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
            raw_message_tokens: entry.raw_token_message.as_ref().map_or(0, &count),
        })
        .collect()
}
