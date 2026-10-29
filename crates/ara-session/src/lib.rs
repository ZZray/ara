//! Session journal (OMP `packages/coding-agent/src/session/session-manager.ts`,
//! `session-entries.ts`, `session-title-slot.ts`, `session-loader.ts`,
//! `session-migrations.ts` at 596f2da7101178214aa27a753529d15e6b7ad91d).
//!
//! File layout (version 3): a fixed 256-byte title-slot line, the session
//! header line, then one JSON entry per line. Entries form a tree through
//! `id`/`parentId`; the leaf is the last appended entry. A new session is not
//! written until it holds an assistant message (lazy materialization); after
//! that every entry is appended before `append_*` returns.
//!
//! Ported: header/entry shapes, 8-hex entry ids, title slot, lazy creation,
//! per-entry append, malformed-record skipping with a rewrite before the next
//! append, corrupt-header rejection without touching the file, branch walk by
//! `parentId` from the leaf (stopping at a missing parent like OMP `pathTo`),
//! duplicate ids resolved to the last line, model-change entries.
//!
//! ARA additions (intentional): unsupported header versions are rejected
//! rather than half-read (migrations are not ported); the bytes of a torn file
//! are copied to a synced backup before any rewrite; appends are `fsync`ed and
//! rewrites are atomic with a directory sync where supported; a failed ordinary
//! per-entry append is rolled back in memory. Checked transactions distinguish
//! published receipts with an uncertain directory barrier; tool calls interrupted before their
//! results were recorded are paired with explicit "effect unknown" results on
//! resume instead of being replayed.
//!
//! Not ported (open): complete provider-native compaction replay, label editing,
//! v1/v2 migrations, SQL/Redis storage,
//! listing/search, moving, title generation, generic lossy persistence trimming.
//! Image/blob persistence is available when the Host binds its blob directory.

pub mod actual_persistence;
pub mod blob;
pub mod context_view;
mod handoff;
mod loop_guard_notice;
pub use loop_guard_notice::actual as actual_loop_guard_notice;
mod model_usage;
mod native_message;
mod native_recovery;
pub use native_recovery::{NativeFailedAssistantRecovery, NativeFailedAssistantRecoveryOutcome};
mod snapcompact_source;
pub use native_message::{AgentEntrySource, AgentMessageBody, NativeJournalEncoder, NativeMessageSource};
pub use native_message::{
    checked_serde as native_checked_serde, clone_wire as native_clone_wire, wire_from_serde as native_wire_from_serde,
};
pub use snapcompact_source::NativeSnapcompactArchiveSource;
mod reduction;
mod remote;
mod skill_prompt;
pub mod snapcompact;

pub use blob::{BlobStore, BlobWarning};
pub use handoff::{NativeCompactionFileDetails, NativeHandoffError, NativeHandoffSnapshot, NativeHandoffSummary};
pub use loop_guard_notice::{
    GEMINI_TOOL_CALL_REMINDER_TYPE, LoopGuardNotice, THINKING_LOOP_REDIRECT_TYPE, TOOL_CALL_LOOP_REDIRECT_TYPE,
};
pub use model_usage::{ModelUsageOwner, ModelUsageRecord};
pub use reduction::{
    SessionReductionAction, SessionReductionEdit, SessionReductionError, SessionReductionReceipt, SessionReductionSlot,
    SessionReductionSnapshot,
};
pub use remote::{NativeRemoteError, NativeRemoteReplay, NativeRemoteSnapshot, NativeRemoteSummary};
pub use skill_prompt::{SKILL_PROMPT_CUSTOM_TYPE, UserSkillPrompt};
pub use snapcompact::{NativeSnapcompactError, NativeSnapcompactSnapshot, NativeSnapcompactSummary};

use ara_ai::{
    AssistantBlock, AssistantMessage, DeveloperMessage, Message, StopReason, UserBlock, UserContent, UserMessage,
    now_ms,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CURRENT_SESSION_VERSION: u64 = 3;
pub const SESSION_TITLE_SLOT_BYTES: usize = 256;
pub const CORRUPT_HEADER_MESSAGE: &str = "session header is missing or malformed";
pub const UNKNOWN_EFFECT_TEXT: &str = "Tool call was interrupted before its result was recorded; its effects are unknown. Inspect the affected state before retrying it.";
pub const DISCARDED_ENTRY_BRANCH_MARKER: &str = "discarded-entry-branch";
pub const ACCEPTED_TERMINAL_EMPTY_STOP_MARKER: &str = "accepted-terminal-empty-stop";
/// Fixed OMP `session-entries.ts:33`: transient model changes retain their
/// receipt without selecting that role when the Session is restored.
pub const EPHEMERAL_MODEL_CHANGE_ROLE: &str = "fallback";

/// Native user command receipt (fixed OMP `session/messages.ts`). This stays
/// distinct from the model's ordinary user message and from Agent tool calls.
#[derive(Clone, Debug, PartialEq)]
pub struct BashExecutionMessage {
    pub command: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    pub meta: Option<Value>,
    pub timestamp: i64,
    pub exclude_from_context: Option<bool>,
}

impl BashExecutionMessage {
    pub fn event_message(&self) -> Value {
        let mut message = json!({
            "role":"bashExecution", "command":self.command, "output":self.output,
            "cancelled":self.cancelled, "truncated":self.truncated, "timestamp":self.timestamp,
        });
        if let Some(exit_code) = self.exit_code {
            message["exitCode"] = json!(exit_code);
        }
        if let Some(meta) = &self.meta {
            message["meta"] = meta.clone();
        }
        if let Some(exclude) = self.exclude_from_context {
            message["excludeFromContext"] = json!(exclude);
        }
        message
    }

    /// Fixed `bashExecutionToText` / `convertOne`, using one user text block.
    pub fn model_message(&self) -> Option<Message> {
        if self.exclude_from_context == Some(true) {
            return None;
        }
        let mut text = format!("Ran `{}`\n", self.command);
        if self.output.is_empty() {
            text.push_str("(no output)");
        } else {
            text.push_str(&format!("```\n{}\n```", self.output));
        }
        if self.cancelled {
            text.push_str("\n\n(command cancelled)");
        } else if let Some(exit_code) = self.exit_code.filter(|code| *code != 0) {
            text.push_str(&format!("\n\nCommand exited with code {exit_code}"));
        }
        text.push_str(&bash_output_notice(self.meta.as_ref()));
        Some(Message::User(UserMessage {
            content: UserContent::Blocks(vec![UserBlock::text(text)]),
            synthetic: None,
            timestamp: self.timestamp,
            provider_payload: None,
            provider_payload_origin: None,
        }))
    }

    fn from_entry(raw: &Value) -> Option<Self> {
        if raw.get("type")?.as_str()? != "message" {
            return None;
        }
        let message = raw.get("message")?;
        if message.get("role")?.as_str()? != "bashExecution" {
            return None;
        }
        let exit_code = match message.get("exitCode") {
            None | Some(Value::Null) => None,
            Some(code) => Some(i32::try_from(code.as_i64()?).ok()?),
        };
        let exclude_from_context = match message.get("excludeFromContext") {
            None | Some(Value::Null) => None,
            Some(exclude) => Some(exclude.as_bool()?),
        };
        Some(Self {
            command: message.get("command")?.as_str()?.into(),
            output: message.get("output")?.as_str()?.into(),
            exit_code,
            cancelled: message.get("cancelled")?.as_bool()?,
            truncated: message.get("truncated")?.as_bool()?,
            meta: message.get("meta").cloned(),
            timestamp: message.get("timestamp")?.as_i64()?,
            exclude_from_context,
        })
    }
}

/// Fixed OutputMetaBuilder `truncationFromSummary(summary,{direction:"tail"})`
/// and `get`. The caller supplies normalized OutputSummary counters, including
/// outputLines/outputBytes measured from the actual retained UTF-8 body.
pub fn bash_output_meta_from_summary(summary: &Value) -> Option<Value> {
    let mut meta = json!({});
    let column_max = summary.get("columnMax").and_then(Value::as_u64).unwrap_or(0);
    if column_max > 0 && summary.get("columnTruncatedLines").and_then(Value::as_u64).unwrap_or(0) > 0 {
        meta["limits"] = json!({"columnTruncated":{"maxColumn":column_max}});
    }
    if summary.get("truncated").and_then(Value::as_bool) == Some(true) {
        let total_lines = summary.get("totalLines")?.as_u64()?;
        let total_bytes = summary.get("totalBytes")?.as_u64()?;
        let output_lines = summary.get("outputLines")?.as_u64()?;
        let output_bytes = summary.get("outputBytes")?.as_u64()?;
        let mut truncation = json!({"totalLines":total_lines,"totalBytes":total_bytes,
            "outputLines":output_lines,"outputBytes":output_bytes});
        if let Some(elided_bytes) = summary.get("elidedBytes").and_then(Value::as_u64).filter(|bytes| *bytes > 0) {
            let elided_lines = summary
                .get("elidedLines")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| total_lines.saturating_sub(output_lines));
            let kept_lines = output_lines.saturating_sub(1);
            let head_lines = kept_lines.div_ceil(2);
            let tail_lines = kept_lines - head_lines;
            truncation["direction"] = json!("middle");
            truncation["truncatedBy"] = json!("middle");
            truncation["elidedBytes"] = json!(elided_bytes);
            truncation["elidedLines"] = json!(elided_lines);
            if head_lines > 0 {
                truncation["headRange"] = json!({"start":1,"end":head_lines});
            }
            if tail_lines > 0 {
                truncation["tailRange"] = json!({"start":total_lines-tail_lines+1,"end":total_lines});
            }
        } else {
            truncation["direction"] = json!("tail");
            truncation["truncatedBy"] = json!(if output_bytes < total_bytes {
                "bytes"
            } else if output_lines < total_lines {
                "lines"
            } else {
                "bytes"
            });
            truncation["shownRange"] = json!({"start":total_lines-output_lines+1,"end":total_lines});
        }
        if let Some(artifact_id) = summary.get("artifactId").filter(|value| !value.is_null()) {
            truncation["artifactId"] = artifact_id.clone();
        }
        meta["truncation"] = truncation;
    }
    (!meta.as_object().expect("meta object").is_empty()).then_some(meta)
}

fn output_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else {
        let (size, unit) = if bytes < 1024 * 1024 {
            (bytes as f64 / 1024.0, "KB")
        } else if bytes < 1024 * 1024 * 1024 {
            (bytes as f64 / (1024.0 * 1024.0), "MB")
        } else {
            (bytes as f64 / (1024.0 * 1024.0 * 1024.0), "GB")
        };
        format!("{:.1}{unit}", (size * 10.0).round() / 10.0)
    }
}

fn grouped_count(count: u64) -> String {
    let digits = count.to_string();
    let mut text = String::new();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            text.push(',');
        }
        text.push(character);
    }
    text
}

// Bash's outputMeta builder supplies truncation/column limits. Preserve other
// imported metadata raw; no tool renderer or LSP dependency belongs to Session.
fn bash_output_notice(meta: Option<&Value>) -> String {
    let Some(meta) = meta else {
        return String::new();
    };
    let mut parts = Vec::new();
    if let Some(truncation) = meta.get("truncation").filter(|value| value.is_object()) {
        let number = |field: &str| truncation.get(field).and_then(Value::as_u64).unwrap_or(0);
        let total_lines = number("totalLines");
        let output_lines = number("outputLines");
        let mut notice;
        if truncation.get("direction").and_then(Value::as_str) == Some("middle") {
            let head = truncation.get("headRange");
            let tail = truncation.get("tailRange");
            let elided_bytes = truncation
                .get("elidedBytes")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| number("totalBytes").saturating_sub(number("outputBytes")));
            let elided_lines = truncation
                .get("elidedLines")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| total_lines.saturating_sub(output_lines));
            notice = if let (Some(head), Some(tail)) = (head, tail) {
                format!(
                    "Showing lines {}-{} and {}-{} of {total_lines}; {} middle line{} ({}) elided",
                    head["start"],
                    head["end"],
                    tail["start"],
                    tail["end"],
                    grouped_count(elided_lines),
                    if elided_lines == 1 { "" } else { "s" },
                    output_bytes(elided_bytes)
                )
            } else if elided_bytes > 0 {
                format!(
                    "Showing head and tail bytes of {} line{}; {} elided",
                    grouped_count(total_lines),
                    if total_lines == 1 { "" } else { "s" },
                    output_bytes(elided_bytes)
                )
            } else {
                format!("Showing {} of {total_lines} lines; middle elided", output_lines.min(total_lines))
            };
        } else {
            let range = truncation.get("shownRange");
            notice = match range.and_then(|range| Some((range.get("start")?.as_u64()?, range.get("end")?.as_u64()?))) {
                Some((start, end)) if end >= start => {
                    format!("Showing lines {start}-{end} of {total_lines}")
                }
                _ => format!("Showing {output_lines} of {total_lines} lines"),
            };
            if truncation.get("truncatedBy").and_then(Value::as_str) == Some("bytes") {
                let max_bytes =
                    truncation.get("maxBytes").and_then(Value::as_u64).unwrap_or_else(|| number("outputBytes"));
                notice.push_str(&format!(" ({} limit)", output_bytes(max_bytes)));
            }
        }
        if let Some(offset) = truncation.get("nextOffset").filter(|value| !value.is_null()) {
            notice.push_str(&format!(". Use :{offset} to continue"));
        }
        if let Some(artifact) = truncation.get("artifactId").filter(|value| !value.is_null()) {
            let artifact = artifact.as_str().map(str::to_owned).unwrap_or_else(|| artifact.to_string());
            notice.push_str(&format!(". Read artifact://{artifact} for full output"));
        }
        parts.push(notice);
    }
    if let Some(limits) = meta.get("limits") {
        for (field, noun) in [("matchLimit", "matches"), ("resultLimit", "results"), ("headLimit", "results")] {
            if let Some(limit) = limits.get(field) {
                parts.push(format!(
                    "{} {noun} limit reached. Use limit={} for more",
                    limit["reached"], limit["suggestion"]
                ));
            }
        }
        if let Some(limit) = limits.get("columnTruncated") {
            parts.push(format!("Some lines truncated to {} chars", limit["maxColumn"]));
        }
    }
    if parts.is_empty() { String::new() } else { format!("\n\n[{}]", parts.join(". ")) }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{path}: {message}")]
    Corrupt { path: PathBuf, message: String },
}

pub type Result<T> = std::result::Result<T, SessionError>;

/// Model and thinking receipts share one publication boundary. An uncertain
/// directory barrier is distinct from a write that never published the pair.
#[derive(Debug, thiserror::Error)]
pub enum ModelAndThinkingChangeError {
    #[error(transparent)]
    Storage(#[from] SessionError),
    #[error(
        "model {model_entry_id} and thinking {thinking_entry_id} were published, but directory durability is unknown: {source}"
    )]
    PublishedButDurabilityUnknown {
        model_entry_id: String,
        thinking_entry_id: String,
        #[source]
        source: SessionError,
    },
}

impl ModelAndThinkingChangeError {
    /// The journal retains the complete published pair. The Host must stop
    /// adoption and reconcile that receipt before retrying this transition.
    pub fn history_published(&self) -> bool {
        matches!(self, Self::PublishedButDurabilityUnknown { .. })
    }
}

/// Publication result for a model change whose thinking receipt is optional.
/// `None` means no thinking entry was staged, including in an uncertain write.
#[derive(Debug, thiserror::Error)]
pub enum ModelAndOptionalThinkingChangeError {
    #[error(transparent)]
    Storage(#[from] SessionError),
    #[error(
        "model {model_entry_id} and optional thinking {thinking_entry_id:?} were published, but directory durability is unknown: {source}"
    )]
    PublishedButDurabilityUnknown {
        model_entry_id: String,
        thinking_entry_id: Option<String>,
        #[source]
        source: SessionError,
    },
}

impl ModelAndOptionalThinkingChangeError {
    /// The matching published view is retained; reconcile it before retrying.
    pub fn history_published(&self) -> bool {
        matches!(self, Self::PublishedButDurabilityUnknown { .. })
    }
}

/// An atomic rewrite may have published its candidate before its final
/// directory durability barrier fails. New checked transactions retain that
/// distinction; the older `rewrite` facade keeps its existing error API.
enum RewritePhaseError {
    BeforePublication(SessionError),
    PublishedButDurabilityUnknown(SessionError),
}

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn file_safe_timestamp(iso: &str) -> String {
    iso.replace([':', '.'], "-")
}

/// Mutable title stored in the fixed-width first line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TitleSlot {
    pub title: String,
    /// `auto` or `user` when known.
    pub source: Option<String>,
    pub updated_at: String,
}

/// OMP `SessionManager.#cleanTitle`: replace C0/C1 controls with spaces,
/// collapse ASCII spaces, then apply ECMAScript `String.trim` whitespace.
pub fn normalize_session_name(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    let mut previous_space = false;
    for character in name.chars() {
        let character =
            if character <= '\u{001f}' || ('\u{007f}'..='\u{009f}').contains(&character) { ' ' } else { character };
        if character == ' ' && previous_space {
            continue;
        }
        normalized.push(character);
        previous_space = character == ' ';
    }
    normalized
        .trim_matches(|character| {
            matches!(
                character,
                ' ' | '\u{00a0}' | '\u{1680}' | '\u{2000}'
                    ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
            )
        })
        .to_owned()
}

/// Fixed-width first line (OMP `serializeTitleSlot`), exactly 256 bytes including `\n`.
pub fn serialize_title_slot(slot: &TitleSlot) -> String {
    let line = |title: &str, pad: &str| {
        let mut v = json!({"type": "title", "v": 1, "title": title});
        if let Some(source) = &slot.source {
            v["source"] = json!(source);
        }
        v["updatedAt"] = json!(slot.updated_at);
        v["pad"] = json!(pad);
        format!("{v}\n")
    };
    let mut chars: Vec<char> = slot.title.chars().collect();
    while line(&chars.iter().collect::<String>(), "").len() > SESSION_TITLE_SLOT_BYTES && !chars.is_empty() {
        chars.pop();
    }
    let title: String = chars.into_iter().collect();
    let pad = SESSION_TITLE_SLOT_BYTES.saturating_sub(line(&title, "").len());
    line(&title, &" ".repeat(pad))
}

fn is_title_slot(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("title") && value.get("v").and_then(Value::as_u64) == Some(1)
}

fn is_valid_header(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("session")
        && value.get("id").and_then(Value::as_str).is_some_and(|s| !s.is_empty())
}

/// OMP `generateId`: last 8 hex chars of a random UUID, unique within the session.
fn generate_id(existing: &HashSet<String>) -> String {
    for _ in 0..100 {
        let id: String = uuid::Uuid::new_v4().simple().to_string()[24..].to_string();
        if !existing.contains(&id) {
            return id;
        }
    }
    uuid::Uuid::now_v7().simple().to_string()
}

fn sync_dir(dir: &Path) -> std::io::Result<()> {
    // Directory fsync makes a rename/create durable (POSIX). Not supported on Windows.
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// One journal line. Entry types ARA does not interpret are preserved verbatim.
#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: String,
    pub raw: Value,
    /// Canonical native source; raw may contain only an explicitly checked mirror.
    pub native_message: Option<NativeMessageSource>,
    /// Canonical compaction preserveData. Distinct from a live message owner.
    pub snapcompact_source: Option<NativeSnapcompactArchiveSource>,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        if self.id != other.id || self.parent_id != other.parent_id || self.kind != other.kind {
            return false;
        }
        if self.snapcompact_source.is_some() || other.snapcompact_source.is_some() {
            if NativeSnapcompactArchiveSource::envelope(&self.raw)
                != NativeSnapcompactArchiveSource::envelope(&other.raw)
            {
                return false;
            }
            let preserve = |entry: &Entry| {
                entry.snapcompact_source.clone().or_else(|| {
                    entry
                        .raw
                        .get("preserveData")
                        .and_then(|value| native_message::wire_from_serde(value).ok())
                        .map(NativeSnapcompactArchiveSource::from_wire)
                })
            };
            return preserve(self) == preserve(other) && self.native_message == other.native_message;
        }
        if self.native_message.is_none() && other.native_message.is_none() {
            return self.raw == other.raw;
        }
        let envelope = |entry: &Entry| native_message::entry_envelope(&entry.raw);
        if envelope(self) != envelope(other) {
            return false;
        }
        match (&self.native_message, &other.native_message) {
            (Some(left), Some(right)) => left == right,
            (Some(NativeMessageSource::Persisted { body }), None) => {
                other.legacy_native_body().is_some_and(|value| body.deep_equal(&value))
            }
            (None, Some(NativeMessageSource::Persisted { body })) => {
                self.legacy_native_body().is_some_and(|value| body.deep_equal(&value))
            }
            _ => false,
        }
    }
}

impl Entry {
    fn legacy_native_body(&self) -> Option<ara_rpc::WireValue> {
        native_message::body_from_entry(&native_message::wire_from_serde(&self.raw).ok()?).ok().flatten()
    }

    /// Canonical application body, with journal identity kept outside it.
    /// Known-role JSON is validated without replacing its extension fields.
    pub fn agent_message_body(&self) -> Result<Option<AgentMessageBody>> {
        if let Some(source) = &self.native_message {
            return Ok(Some(AgentMessageBody::Native(source.clone())));
        }
        let mut body = match self.kind.as_str() {
            "message" => self.raw.get("message").cloned().ok_or_else(|| agent_body_error("missing message body"))?,
            "custom_message" => {
                let mut body = self.raw.clone();
                let object = body.as_object_mut().ok_or_else(|| agent_body_error("invalid custom entry"))?;
                object.remove("type");
                object.remove("id");
                object.remove("parentId");
                object.insert("role".into(), json!("custom"));
                object.insert(
                    "timestamp".into(),
                    json!(entry_timestamp(&self.raw).ok_or_else(|| agent_body_error("invalid custom timestamp"))?),
                );
                body
            }
            "branch_summary" if !self.is_discarded_entry_branch_marker() => {
                let mut body = self.raw.clone();
                let object = body.as_object_mut().ok_or_else(|| agent_body_error("invalid branch summary"))?;
                object.remove("type");
                object.remove("id");
                object.remove("parentId");
                object.insert("role".into(), json!("branchSummary"));
                object.insert(
                    "timestamp".into(),
                    json!(entry_timestamp(&self.raw).ok_or_else(|| agent_body_error("invalid branch timestamp"))?),
                );
                body
            }
            _ => return Ok(None),
        };
        // Validation and conversion inspect the current body, never a saved
        // source projection. Unrecognized application roles have zero fragments.
        Self::project_agent_message_body(&body)?;
        Ok(Some(AgentMessageBody::Legacy(std::mem::take(&mut body))))
    }

    /// Checked current-body projection shared by raw Host conversion and append.
    pub fn project_agent_message_body(body: &Value) -> Result<Vec<Message>> {
        let role = body
            .as_object()
            .and_then(|object| object.get("role"))
            .and_then(Value::as_str)
            .filter(|role| !role.is_empty())
            .ok_or_else(|| agent_body_error("message requires a nonempty role"))?;
        if matches!(role, "user" | "developer" | "assistant") {
            let message: Message = serde_json::from_value(body.clone())
                .map_err(|error| agent_body_error(format!("malformed {role}: {error}")))?;
            return Ok(vec![message]);
        }
        if role == "custom" {
            let custom_type = body
                .get("customType")
                .and_then(Value::as_str)
                .ok_or_else(|| agent_body_error("custom message requires customType"))?;
            let reserved = custom_type == SKILL_PROMPT_CUSTOM_TYPE && body["attribution"] == "user"
                || matches!(
                    custom_type,
                    THINKING_LOOP_REDIRECT_TYPE | GEMINI_TOOL_CALL_REMINDER_TYPE | TOOL_CALL_LOOP_REDIRECT_TYPE
                );
            if !reserved
                && serde_json::from_value::<UserContent>(body.get("content").cloned().unwrap_or(Value::Null)).is_err()
            {
                return Ok(Vec::new());
            }
        }
        if !matches!(
            role,
            "toolResult"
                | "custom"
                | "hookMessage"
                | "bashExecution"
                | "branchSummary"
                | "compactionSummary"
                | "fileMention"
        ) {
            return Ok(Vec::new());
        }
        let entry = Entry {
            id: "raw-agent-body".into(),
            parent_id: None,
            kind: "message".into(),
            raw: json!({"type":"message", "message":body}),
            native_message: None,
            snapcompact_source: None,
        };
        entry.model_messages()?.ok_or_else(|| agent_body_error(format!("malformed reserved {role} message")))
    }

    /// Native remote summaries stay one raw compactionSummary. A checked
    /// carrier is derived only for the active route's provider preparation.
    pub fn project_agent_message_body_for_route(
        body: &Value,
        model: &ara_ai::Model,
        native_replay_available: bool,
    ) -> Result<Vec<Message>> {
        // Native proof is supplied only by checked Session selection below.
        // A valid raw preserveData payload alone is not source/route evidence.
        let _ = (model, native_replay_available);
        Self::project_agent_message_body(body)
    }
    fn is_service_tier_change(&self) -> bool {
        self.kind == "service_tier_change"
            && self.raw.get("serviceTier").is_some_and(|tier| tier.is_null() || tier.is_object())
    }

    fn is_discarded_entry_branch_marker(&self) -> bool {
        self.kind == "branch_summary"
            && self.raw.get("summary").and_then(Value::as_str) == Some("")
            && self.raw.get("fromId").and_then(Value::as_str) == Some(self.parent_id.as_deref().unwrap_or("root"))
            && self.raw.pointer("/details/kind").and_then(Value::as_str) == Some(DISCARDED_ENTRY_BRANCH_MARKER)
            && self.raw.pointer("/details/discardedEntryId").and_then(Value::as_str).is_some_and(|id| !id.is_empty())
    }

    fn is_accepted_terminal_empty_stop_marker(&self) -> bool {
        self.kind == "custom"
            && self.raw.get("customType").and_then(Value::as_str) == Some(ACCEPTED_TERMINAL_EMPTY_STOP_MARKER)
            && self.raw.get("data").is_none()
    }

    pub fn bash_execution(&self) -> Result<Option<BashExecutionMessage>> {
        if self.native_message.is_some() {
            return Ok(BashExecutionMessage::from_entry(&self.checked_raw()?));
        }
        Ok(self.legacy_bash_execution())
    }

    fn legacy_bash_execution(&self) -> Option<BashExecutionMessage> {
        BashExecutionMessage::from_entry(&self.raw)
    }

    fn is_decodable_message(&self) -> bool {
        self.model_messages().is_ok_and(|messages| messages.is_some())
    }

    /// Original user-invoked Skill identity, independent of its model projection.
    pub fn skill_prompt(&self) -> Option<UserSkillPrompt> {
        if self.native_message.is_some() {
            return UserSkillPrompt::from_entry(&self.checked_raw().ok()?);
        }
        UserSkillPrompt::from_entry(&self.raw)
    }

    pub fn loop_guard_notice(&self) -> Option<LoopGuardNotice> {
        if self.native_message.is_some() {
            return LoopGuardNotice::from_entry(&self.checked_raw().ok()?);
        }
        LoopGuardNotice::from_entry(&self.raw)
    }

    fn is_user_skill_prompt_candidate(&self) -> bool {
        self.kind == "custom_message"
            && self.raw.get("customType").and_then(Value::as_str) == Some(SKILL_PROMPT_CUSTOM_TYPE)
            && self.raw.get("attribution").and_then(Value::as_str) == Some("user")
    }

    /// Compatibility projection for consumers that require exactly one
    /// message. Multi-fragment entries must use [`model_messages`](Self::model_messages).
    pub fn message(&self) -> Result<Option<Message>> {
        let Some(mut messages) = self.model_messages()? else {
            return Ok(None);
        };
        Ok((messages.len() == 1).then(|| messages.remove(0)))
    }

    /// Canonical ordered provider fragments. `None` identifies an unknown or
    /// malformed entry; an empty vector is a recognized non-context entry.
    pub fn model_messages(&self) -> Result<Option<Vec<Message>>> {
        // A journal compaction is a boundary, never an ordinary message. Its
        // preserve owner is checked only by summary admission/conversion.
        if self.kind == "compaction" {
            return Ok(self.legacy_model_messages());
        }
        if self.native_message.is_some() || self.snapcompact_source.is_some() {
            // Preserve the established malformed-entry classification after
            // the explicit checked bridge. Native representation/Host errors
            // propagate; a representable malformed reserved receipt is None.
            let mut observed = self.clone();
            observed.raw = self.checked_raw()?;
            observed.native_message = None;
            observed.snapcompact_source = None;
            return Ok(observed.legacy_model_messages());
        }
        Ok(self.legacy_model_messages())
    }

    fn observed_excluded_bash_execution(&self) -> Result<bool> {
        use ara_rpc::WireValue as W;
        if self.kind != "message" || !self.observed_role()?.is_some_and(|role| role.equals_ascii("bashExecution")) {
            return Ok(false);
        }
        // A flag on a malformed receipt is not a transparent recovery tail.
        // Read only the reserved fields used by BashExecutionMessage::from_entry;
        // arbitrary metadata/extension graphs are irrelevant to this decision.
        let exit = self.observed_message_field(&["exitCode"])?;
        if !match exit.as_ref() {
            None | Some(W::Null) => true,
            Some(W::Number(value)) => {
                value.is_finite()
                    && value.fract() == 0.0
                    && *value >= f64::from(i32::MIN)
                    && *value <= f64::from(i32::MAX)
            }
            _ => false,
        } {
            return Ok(false);
        }
        if !matches!(self.observed_message_field(&["excludeFromContext"])?, Some(W::Bool(true))) {
            return Ok(false);
        }
        for key in ["command", "output"] {
            if !matches!(self.observed_message_field(&[key])?, Some(W::String(_))) {
                return Ok(false);
            }
        }
        for key in ["cancelled", "truncated"] {
            if !matches!(self.observed_message_field(&[key])?, Some(W::Bool(_))) {
                return Ok(false);
            }
        }
        Ok(matches!(self.observed_message_field(&["timestamp"])?, Some(W::Number(value))
            if value.is_finite() && value.fract() == 0.0 && value >= i64::MIN as f64 && value < i64::MAX as f64))
    }

    fn legacy_model_messages(&self) -> Option<Vec<Message>> {
        match self.kind.as_str() {
            "message" if self.role() == Some("bashExecution") => {
                Some(self.legacy_bash_execution()?.model_message().into_iter().collect())
            }
            "message" if self.role() == Some("hookMessage") => custom_model_messages(self.raw.get("message")?),
            "message" if self.role() == Some("custom") => {
                let mut raw = self.raw.get("message")?.clone();
                let timestamp =
                    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(raw.get("timestamp")?.as_i64()?)?
                        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                        .to_string();
                raw["type"] = json!("custom_message");
                raw["timestamp"] = json!(timestamp);
                custom_entry_model_messages(&raw)
            }
            "message" if self.role() == Some("branchSummary") => {
                let message = self.raw.get("message")?;
                message.get("fromId")?.as_str()?;
                Some(vec![summary_model_message(message, "branch", message.get("timestamp")?.as_i64()?)?])
            }
            "message" if self.role() == Some("compactionSummary") => {
                let message = self.raw.get("message")?;
                message.get("tokensBefore")?.as_u64()?;
                Some(vec![summary_model_message(message, "compaction", message.get("timestamp")?.as_i64()?)?])
            }
            "message" if self.role() == Some("fileMention") => file_mention_model_messages(self.raw.get("message")?),
            "message" if self.role() == Some("toolResult") => {
                let mut message = self.raw.get("message")?.clone();
                if message.get("prunedAt").is_some() {
                    // Fixed compaction/messages.ts getPrunedToolResultContent:
                    // canonical projection only; original raw receipt stays intact.
                    let blocks = message.get("content")?.as_array()?;
                    let mut text = String::new();
                    for block in blocks.iter().filter(|block| block["type"] == "text") {
                        text.push_str(block["text"].as_str()?);
                    }
                    if text.is_empty() {
                        text.push_str("[Output truncated]");
                    }
                    let first = blocks.iter().position(|block| block["type"] == "text");
                    let mut content = Vec::new();
                    if first.is_none() {
                        content.push(json!({"type":"text","text":text}));
                    }
                    for (index, block) in blocks.iter().enumerate() {
                        if block["type"] != "text" {
                            content.push(block.clone());
                        } else if first == Some(index) {
                            content.push(json!({"type":"text","text":text}));
                        }
                    }
                    message["content"] = json!(content);
                }
                Some(vec![serde_json::from_value(message).ok()?])
            }
            "message" if self.role().is_some_and(|role| !matches!(role, "user" | "developer" | "assistant")) => {
                // Valid application-only records remain raw evidence, while
                // the default Host projection contributes no model fragment.
                let body = self.raw.get("message")?;
                body.as_object()?;
                body.get("role")?.as_str().filter(|role| !role.is_empty())?;
                Some(Vec::new())
            }
            "message" => Some(vec![serde_json::from_value(self.raw.get("message")?.clone()).ok()?]),
            "custom_message" => custom_entry_model_messages(&self.raw),
            "branch_summary" if self.is_discarded_entry_branch_marker() => Some(Vec::new()),
            "branch_summary" => {
                self.raw.get("fromId")?.as_str()?;
                Some(vec![summary_model_message(&self.raw, "branch", entry_timestamp(&self.raw)?)?])
            }
            "compaction" => Some(Vec::new()),
            _ if self.is_metadata() => Some(Vec::new()),
            _ => None,
        }
    }

    fn is_metadata(&self) -> bool {
        if self.kind == "custom"
            && self.raw.get("customType").and_then(Value::as_str) == Some(ACCEPTED_TERMINAL_EMPTY_STOP_MARKER)
        {
            return self.is_accepted_terminal_empty_stop_marker();
        }
        matches!(
            self.kind.as_str(),
            "model_change"
                | "label"
                | "title_change"
                | "thinking_level_change"
                | "ttsr_injection"
                | "session_init"
                | "mode_change"
                | "credential_pin"
                | "model_usage"
        ) || (self.kind == "custom" && self.raw.get("customType").and_then(Value::as_str).is_some())
            || self.is_service_tier_change()
            || self.is_accepted_terminal_empty_stop_marker()
    }

    fn native_compaction_entry(&self) -> std::result::Result<NativeCompactionEntry, CompactionSourceError> {
        if self.native_message.is_some() {
            let observed = if self.kind == "message" {
                self.native_message.as_ref().expect("native message source").checked_compaction_message_entry(&self.raw)
                    .map_err(SessionError::from)
            } else {
                // Custom envelopes and summaries keep their exact strict view.
                self.checked_raw()
            };
            let raw = observed.map_err(|_| CompactionSourceError::UndecodableMessage { id: self.id.clone() })?;
            let view = Entry {
                id: self.id.clone(),
                parent_id: self.parent_id.clone(),
                kind: self.kind.clone(),
                raw,
                native_message: None,
                snapcompact_source: None,
            };
            return view.native_compaction_entry();
        }
        // The ordinary custom-message converter retains its legacy empty
        // projection on invalid content. A compaction source must distinguish
        // that decode failure from an intentionally empty context entry.
        if self.kind == "custom_message"
            && serde_json::from_value::<UserContent>(self.raw.get("content").cloned().unwrap_or(Value::Null)).is_err()
        {
            return Err(CompactionSourceError::UnsupportedContextEntry {
                id: self.id.clone(),
                kind: self.kind.clone(),
            });
        }
        let messages = self
            .model_messages()
            .map_err(|_| CompactionSourceError::UndecodableMessage { id: self.id.clone() })?
            .ok_or_else(|| {
                if self.kind == "message" {
                    CompactionSourceError::UndecodableMessage { id: self.id.clone() }
                } else {
                    CompactionSourceError::UnsupportedContextEntry { id: self.id.clone(), kind: self.kind.clone() }
                }
            })?;
        let origin = match self.kind.as_str() {
            "message" => match self.role() {
                Some("user") => NativeEntryOrigin::User,
                Some("assistant") => NativeEntryOrigin::Assistant,
                Some("toolResult") => NativeEntryOrigin::ToolResult,
                Some("developer") => NativeEntryOrigin::Developer,
                Some("bashExecution") => NativeEntryOrigin::BashExecution,
                Some("hookMessage") => NativeEntryOrigin::HookMessage,
                Some("custom") => NativeEntryOrigin::LegacyCustomMessage,
                Some("branchSummary") => NativeEntryOrigin::LegacyBranchSummary,
                Some("compactionSummary") => NativeEntryOrigin::LegacyCompactionSummary,
                Some("fileMention") => NativeEntryOrigin::FileMention,
                _ => NativeEntryOrigin::Metadata,
            },
            "custom_message" if self.is_user_skill_prompt_candidate() => NativeEntryOrigin::UserSkill,
            "custom_message" if LoopGuardNotice::is_candidate(&self.raw) => NativeEntryOrigin::LoopGuardNotice,
            "custom_message" if self.raw["customType"] == "collab-prompt" && self.raw["attribution"] == "user" => {
                NativeEntryOrigin::SteeringUser
            }
            "custom_message" => NativeEntryOrigin::CustomMessage,
            "branch_summary" if self.is_discarded_entry_branch_marker() => NativeEntryOrigin::Metadata,
            "branch_summary" => NativeEntryOrigin::BranchSummary,
            "compaction" => NativeEntryOrigin::CompactionBoundary,
            _ => NativeEntryOrigin::Metadata,
        };
        // Count one proxy per raw `message`, never once per projected fragment.
        // Native Bash/legacy-role estimation remains a text projection proxy.
        let raw_token_message = if self.role() == Some("bashExecution") {
            let mut bash = self.legacy_bash_execution().expect("canonical Bash decoded");
            bash.exclude_from_context = None;
            bash.model_message()
        } else if self.kind == "message" {
            let mut projected = messages.clone();
            if projected.len() == 1 {
                projected.pop()
            } else if projected.is_empty() {
                None
            } else {
                let content: Vec<UserBlock> = projected
                    .into_iter()
                    .flat_map(|message| match message {
                        Message::Developer(message) => content_blocks(message.content),
                        Message::User(message) => content_blocks(message.content),
                        _ => unreachable!("only native custom projections have multiple fragments"),
                    })
                    .collect();
                Some(Message::Developer(DeveloperMessage {
                    content: UserContent::Blocks(content),
                    timestamp: self
                        .raw
                        .pointer("/message/timestamp")
                        .and_then(Value::as_i64)
                        .expect("native timestamp decoded"),
                    provider_payload: None,
                    provider_payload_origin: None,
                }))
            }
        } else {
            None
        };
        Ok(NativeCompactionEntry { entry_id: self.id.clone(), origin, messages, raw_token_message })
    }

    /// Checked derived full entry for consumers with a legacy JSON contract.
    /// Failure is explicit; this value does not replace native canonical data.
    pub fn checked_raw(&self) -> Result<Value> {
        if let Some(source) = &self.snapcompact_source {
            return source.checked_entry(&self.raw).map_err(Into::into);
        }
        if let Some(source) = &self.native_message {
            return source.checked_entry(&self.raw).map_err(Into::into);
        }
        Ok(self.raw.clone())
    }

    /// Selected local-reduction observation; the canonical native source is
    /// retained and only its Host-prepared edited candidate can be published.
    pub fn checked_reduction_raw(&self) -> Result<Value> {
        if let Some(source) = &self.snapcompact_source {
            return source.checked_entry(&self.raw).map_err(Into::into);
        }
        match &self.native_message {
            Some(source) => source.checked_reduction_entry(&self.raw).map_err(Into::into),
            None => Ok(self.raw.clone()),
        }
    }

    pub fn original_source(&self) -> AgentEntrySource {
        if let Some(preserve) = &self.snapcompact_source {
            return AgentEntrySource::Snapcompact {
                envelope: NativeSnapcompactArchiveSource::envelope(&self.raw),
                preserve: preserve.clone(),
            };
        }
        match &self.native_message {
            Some(body) => {
                let envelope = native_message::entry_envelope(&self.raw);
                AgentEntrySource::Native { envelope, body: body.clone() }
            }
            None => AgentEntrySource::Legacy(self.raw.clone()),
        }
    }

    /// Complete current summary body for Host source equality, including all
    /// original metadata. This is not a model-visible conversion or grant.
    pub fn compaction_summary_source(&self) -> std::io::Result<NativeMessageSource> {
        if self.kind != "compaction" {
            return Err(native_message::invalid("not a compaction entry"));
        }
        let mut raw = self.raw.clone();
        let fields = raw.as_object_mut().ok_or_else(|| native_message::invalid("invalid summary source"))?;
        fields.remove("type");
        fields.remove("id");
        fields.remove("parentId");
        fields.insert("role".into(), json!("compactionSummary"));
        fields.insert(
            "timestamp".into(),
            json!(entry_timestamp(&self.raw).ok_or_else(|| native_message::invalid("invalid summary timestamp"))?),
        );
        fields.insert(
            "method".into(),
            json!(self.raw.get("method").filter(|value| !value.is_null()).and_then(Value::as_str).unwrap_or("soft")),
        );
        let body = match &self.snapcompact_source {
            Some(source) => source.wire_entry(&raw)?,
            None => native_message::wire_from_serde(&raw)?,
        };
        Ok(NativeMessageSource::persisted(body))
    }

    fn archive_source(&self) -> Option<NativeSnapcompactArchiveSource> {
        self.snapcompact_source.clone().or_else(|| {
            self.raw
                .get("preserveData")
                .filter(|value| !value.is_null())
                .and_then(|value| native_message::wire_from_serde(value).ok())
                .map(NativeSnapcompactArchiveSource::from_wire)
        })
    }

    fn has_archive(&self) -> bool {
        self.archive_source().and_then(|source| source.exact_archive().ok().flatten()).is_some()
    }

    pub fn observed_message_body(&self) -> Result<Option<std::sync::Arc<ara_rpc::WireValue>>> {
        if let Some(source) = &self.native_message {
            return Ok(Some(source.observe()?));
        }
        self.raw
            .get("message")
            .map(native_message::wire_from_serde)
            .transpose()
            .map(|body| body.map(std::sync::Arc::new))
            .map_err(Into::into)
    }

    pub fn observed_message_field(&self, path: &[&str]) -> Result<Option<ara_rpc::WireValue>> {
        if let Some(source) = &self.native_message {
            return source.observe_field(path).map_err(Into::into);
        }
        let Some(body) = self.raw.get("message") else {
            return Ok(None);
        };
        let mut value = body;
        for key in path {
            let Some(next) = value.get(*key) else {
                return Ok(None);
            };
            value = next;
        }
        native_message::wire_from_serde(value).map(Some).map_err(Into::into)
    }

    pub fn observed_tool_calls(&self) -> Result<Vec<(ara_rpc::WireString, ara_rpc::WireString)>> {
        if let Some(source) = &self.native_message {
            return source.observe_tool_calls().map_err(Into::into);
        }
        Ok(self
            .raw
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|call| call.get("type").and_then(Value::as_str) == Some("toolCall"))
            .filter_map(|call| Some((call.get("id")?.as_str()?.into(), call.get("name")?.as_str()?.into())))
            .collect())
    }

    fn observed_role(&self) -> Result<Option<ara_rpc::WireString>> {
        Ok(self.observed_message_field(&["role"])?.and_then(|role| role.as_string().cloned()))
    }

    fn role(&self) -> Option<&str> {
        if self.kind != "message" {
            return None;
        }
        self.raw.pointer("/message/role").and_then(Value::as_str)
    }
}

fn entry_timestamp(raw: &Value) -> Option<i64> {
    Some(chrono::DateTime::parse_from_rfc3339(raw.get("timestamp")?.as_str()?).ok()?.timestamp_millis())
}

fn agent_body_error(message: impl Into<String>) -> SessionError {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into()).into()
}

/// Fixed session/messages.ts fileMention conversion: text files occupy the
/// Developer slot, image files the User slot, in that order. Do not persist
/// wrappers or manufacture text for an empty file list.
fn file_mention_model_messages(raw: &Value) -> Option<Vec<Message>> {
    let timestamp = raw.get("timestamp")?.as_i64()?;
    let files = raw.get("files")?.as_array()?;
    let mut text_files = Vec::new();
    let mut image_files = Vec::new();
    let mut images = Vec::new();
    for file in files {
        let path = file.get("path")?.as_str()?;
        let content = match file.get("content") {
            None | Some(Value::Null) => "",
            Some(content) => content.as_str()?,
        };
        let wrapped = if content.is_empty() {
            format!("<file path=\"{path}\">\n</file>")
        } else {
            format!("<file path=\"{path}\">\n{content}\n</file>")
        };
        match file.get("image") {
            None | Some(Value::Null) | Some(Value::Bool(false)) => text_files.push(wrapped),
            Some(image) => {
                let image: UserBlock = serde_json::from_value(image.clone()).ok()?;
                if !matches!(image, UserBlock::Image(_)) {
                    return None;
                }
                image_files.push(wrapped);
                images.push(image);
            }
        }
    }
    let mut messages = Vec::new();
    if !text_files.is_empty() {
        messages.push(Message::Developer(DeveloperMessage {
            content: UserContent::Blocks(vec![UserBlock::text(text_files.join("\n"))]),
            timestamp,
            provider_payload: None,
            provider_payload_origin: None,
        }));
    }
    if !image_files.is_empty() {
        let mut content = vec![UserBlock::text(image_files.join("\n"))];
        content.extend(images);
        messages.push(Message::User(UserMessage {
            content: UserContent::Blocks(content),
            synthetic: None,
            timestamp,
            provider_payload: None,
            provider_payload_origin: None,
        }));
    }
    Some(messages)
}

fn content_blocks(content: UserContent) -> Vec<UserBlock> {
    match content {
        UserContent::Text(text) => vec![UserBlock::text(text)],
        UserContent::Blocks(blocks) => blocks,
    }
}

fn custom_entry_model_messages(raw: &Value) -> Option<Vec<Message>> {
    if raw["customType"] == SKILL_PROMPT_CUSTOM_TYPE && raw["attribution"] == "user" {
        return Some(vec![UserSkillPrompt::from_entry(raw)?.model_message()]);
    }
    if LoopGuardNotice::is_candidate(raw) {
        return Some(vec![LoopGuardNotice::from_entry(raw)?.model_message()]);
    }
    if raw["customType"] == "collab-prompt" && raw["attribution"] == "user" {
        raw.get("display")?.as_bool()?;
        let content: UserContent = serde_json::from_value(raw.get("content")?.clone()).ok()?;
        let text = content.plain_text();
        let content = if text.is_empty() {
            content
        } else {
            let envelope = ara_prompt::prompt::render("<system-notice>\nUser interjection during work: priority; supersedes conflicting prior instructions. Re-read; ensure current work reflects user intent.\n</system-notice>\n{{message}}\n", &json!({"message":text})).ok()?;
            match content {
                UserContent::Text(_) => UserContent::Text(envelope),
                UserContent::Blocks(blocks) => {
                    let mut content = vec![UserBlock::text(envelope)];
                    content.extend(blocks.into_iter().filter(|block| matches!(block, UserBlock::Image(_))));
                    UserContent::Blocks(content)
                }
            }
        };
        return Some(vec![Message::User(UserMessage {
            content,
            synthetic: None,
            timestamp: entry_timestamp(raw)?,
            provider_payload: None,
            provider_payload_origin: None,
        })]);
    }
    let mut message = raw.clone();
    message["timestamp"] = json!(entry_timestamp(raw)?);
    custom_model_messages(&message)
}

/// Fixed coding-agent `convertImageBearingCustomMessage`: text remains a
/// Developer fragment, while images occupy the supported User content slot.
fn custom_model_messages(raw: &Value) -> Option<Vec<Message>> {
    let custom_type = raw.get("customType")?.as_str()?;
    raw.get("display")?.as_bool()?;
    if raw.get("attribution").is_some_and(|value| !matches!(value.as_str(), Some("agent" | "user"))) {
        return None;
    }
    let timestamp = raw.get("timestamp")?.as_i64()?;
    let content: UserContent = match serde_json::from_value(raw.get("content").cloned().unwrap_or(Value::Null)) {
        Ok(content) => content,
        Err(_) => return Some(Vec::new()),
    };
    let blocks = content_blocks(content);
    let (images, text): (Vec<UserBlock>, Vec<UserBlock>) =
        blocks.into_iter().partition(|block| matches!(block, UserBlock::Image(_)));
    if images.is_empty() {
        return Some(vec![Message::Developer(DeveloperMessage {
            content: UserContent::Blocks(text),
            timestamp,
            provider_payload: None,
            provider_payload_origin: None,
        })]);
    }
    let mut messages = Vec::new();
    if !text.is_empty() {
        messages.push(Message::Developer(DeveloperMessage {
            content: UserContent::Blocks(text),
            timestamp,
            provider_payload: None,
            provider_payload_origin: None,
        }));
    }
    let mut content = vec![UserBlock::text(format!("Images attached to {custom_type}."))];
    content.extend(images);
    messages.push(Message::User(UserMessage {
        content: UserContent::Blocks(content),
        synthetic: None,
        timestamp,
        provider_payload: None,
        provider_payload_origin: None,
    }));
    Some(messages)
}

fn summary_model_message(raw: &Value, kind: &str, timestamp: i64) -> Option<Message> {
    let summary = raw.get("summary")?.as_str()?;
    if kind == "compaction" && ara_snapcompact::get_preserved_archive(raw.get("preserveData")).is_some() {
        let mut message = snapcompact::model_message_from_summary(summary, raw.get("preserveData"));
        if let Message::User(user) = &mut message {
            user.timestamp = timestamp;
        }
        return Some(message);
    }
    let content = if kind == "branch" {
        vec![UserBlock::text(
            ara_prompt::prompt::render(
                "Branch-return summary:\n\n<summary>\n{{summary}}\n</summary>\n",
                &json!({"summary":summary}),
            )
            .ok()?,
        )]
    } else if let Some(blocks) = raw.get("blocks") {
        let mut content = vec![UserBlock::text(summary)];
        let mut blocks = serde_json::from_value::<Vec<UserBlock>>(blocks.clone()).ok()?;
        for block in &mut blocks {
            if let UserBlock::Image(image) = block {
                image.compaction_frame = true;
            }
        }
        content.extend(blocks);
        content
    } else {
        let template = if raw.get("method").and_then(Value::as_str) == Some("handoff") {
            "Context replaced. The <handoff> below is a handoff document a prior instance of you wrote from the full conversation. It is your own working memory, not user input.\n- First person inside it refers to you (the prior instance).\n- \"Next Steps\" is your own resumed plan; re-check it against the latest user message before acting.\n- The handoff already exists and is complete: NEVER write another handoff document unless the user explicitly asks.\nMUST build on prior work; NEVER duplicate prior work.\n\n<handoff>\n{{summary}}\n</handoff>\n"
        } else {
            "Prior model work/tool state available.\nMUST build on prior work; NEVER duplicate prior work.\n\n<summary>\n{{summary}}\n</summary>\n"
        };
        let text = ara_prompt::prompt::render(template, &json!({"summary":summary})).ok()?;
        let mut content = vec![UserBlock::text(text)];
        if let Some(images) = raw.get("images") {
            let mut images: Vec<UserBlock> = serde_json::from_value(images.clone()).ok()?;
            if images.iter().any(|block| !matches!(block, UserBlock::Image(_))) {
                return None;
            }
            for block in &mut images {
                if let UserBlock::Image(image) = block {
                    image.compaction_frame = true;
                }
            }
            content.extend(images);
        }
        content
    };
    Some(Message::User(UserMessage {
        content: UserContent::Blocks(content),
        synthetic: None,
        timestamp,
        provider_payload: None,
        provider_payload_origin: None,
    }))
}

/// Storage-side guard for a soft summary's replaced prefix. It mirrors the
/// Agent's `validate_completed_summary_span` without making Session depend on
/// the Agent crate: every tool call is paired with its result, the prefix does
/// not end with an unanswered prompt, and no unknown-effect result is hidden.
/// Turns that ended aborted, errored or length-stopped are summarized, as in
/// fixed OMP. The raw prefix stays available for audit.
/// A native Assistant cut may summarize the current user prompt before its
/// answer. Tool ownership and unknown effects still require complete pairs.
fn safe_summary_prefix(branch: &[&Entry], allow_unanswered_user: bool) -> bool {
    safe_compaction_prefix(branch, allow_unanswered_user, false)
}

/// Native replay preserves image-bearing messages; all raw ownership,
/// complete-pair and unknown-effect checks remain shared with text summaries.
fn safe_compaction_prefix(branch: &[&Entry], allow_unanswered_user: bool, native_replay: bool) -> bool {
    safe_compaction_prefix_with_archive(branch, allow_unanswered_user, native_replay, None)
}

/// Text replacement may migrate a previously verified archive. Only its
/// already covered raw prefix and that exact derived archive entry admit
/// pictures; newly consumed raw pictures keep the ordinary soft gate.
fn safe_compaction_prefix_with_archive(
    branch: &[&Entry],
    allow_unanswered_user: bool,
    native_replay: bool,
    previous_archive: Option<&CompactionSummaryView>,
) -> bool {
    let branch = &branch[active_context_start(branch)..];
    let previous_archive = previous_archive
        .filter(|summary| summary.source_entry_ids.is_some())
        .and_then(|summary| summary.archive_image_boundary.as_ref());
    let archive_prefix_end =
        previous_archive.and_then(|boundary| branch.iter().position(|entry| entry.id == boundary.first_kept_entry_id));
    let mut saw_message = false;
    let mut pending: HashMap<String, String> = HashMap::new();
    let mut ends_with_user = false;
    for (index, entry) in branch.iter().enumerate() {
        let images_allowed = native_replay
            || archive_prefix_end.is_some_and(|end| index < end)
            || previous_archive.is_some_and(|summary| summary.entry_id == entry.id);
        let Ok(group) = entry.native_compaction_entry() else {
            return false;
        };
        let historical_custom = matches!(
            group.origin,
            NativeEntryOrigin::CustomMessage
                | NativeEntryOrigin::HookMessage
                | NativeEntryOrigin::LegacyCustomMessage
                | NativeEntryOrigin::LoopGuardNotice
                | NativeEntryOrigin::FileMention
        );
        for message in group.messages {
            if !saw_message && !matches!(message, Message::User(_)) && !historical_custom {
                return false;
            }
            saw_message = true;
            match &message {
                Message::User(user) => {
                    if !pending.is_empty()
                        || !images_allowed
                            && matches!(&user.content, UserContent::Blocks(blocks) if blocks.iter().any(|block| matches!(block, UserBlock::Image(_))))
                    {
                        return false;
                    }
                    ends_with_user |= matches!(
                        group.origin,
                        NativeEntryOrigin::User
                            | NativeEntryOrigin::UserSkill
                            | NativeEntryOrigin::SteeringUser
                            | NativeEntryOrigin::BashExecution
                            | NativeEntryOrigin::LegacyCustomMessage
                    );
                }
                Message::Developer(developer) => {
                    if !historical_custom
                        || !pending.is_empty()
                        || !images_allowed
                            && matches!(&developer.content, UserContent::Blocks(blocks) if blocks.iter().any(|block| matches!(block, UserBlock::Image(_))))
                    {
                        return false;
                    }
                }
                Message::Assistant(assistant) => {
                    if !pending.is_empty()
                        || !images_allowed
                            && assistant.content.iter().any(|block| matches!(block, AssistantBlock::Image(_)))
                    {
                        return false;
                    }
                    ends_with_user = false;
                    for call in assistant.tool_calls() {
                        if pending.insert(call.id.clone(), call.name.clone()).is_some() {
                            return false;
                        }
                    }
                }
                Message::ToolResult(result) => {
                    if !images_allowed && result.content.iter().any(|block| matches!(block, UserBlock::Image(_)))
                        || result.details.as_ref().is_some_and(|details| {
                            details.get("panicked").and_then(Value::as_bool) == Some(true)
                                || (details.get("__synthetic").and_then(Value::as_bool) == Some(true)
                                    && details.get("source").and_then(Value::as_str)
                                        == Some("interrupted_unknown_effect")
                                    && details.get("executed").and_then(Value::as_str) == Some("unknown"))
                        })
                    {
                        return false;
                    }
                    match pending.remove(result.tool_call_id.as_str()) {
                        Some(name) if name == result.tool_name => {}
                        _ => return false,
                    }
                    ends_with_user = false;
                }
            }
        }
    }
    saw_message && pending.is_empty() && (allow_unanswered_user || !ends_with_user)
}

/// Fixed OMP `prepareCompaction` / `buildSessionContext`: `/clear` only
/// replaces the active conversation. Keep the full raw parent chain for
/// history and provenance checks before selecting this model-visible suffix.
fn active_context_start(branch: &[&Entry]) -> usize {
    branch.iter().rposition(|entry| entry.kind == "reset_boundary").map_or(0, |index| index + 1)
}

fn native_kept_boundary_supported(branch: &[&Entry], index: usize) -> bool {
    for entry in &branch[index..] {
        let Ok(group) = entry.native_compaction_entry() else {
            return false;
        };
        if group.origin == NativeEntryOrigin::Metadata {
            continue;
        }
        return !matches!(
            group.origin,
            NativeEntryOrigin::ToolResult
                | NativeEntryOrigin::Developer
                | NativeEntryOrigin::LegacyCustomMessage
                | NativeEntryOrigin::FileMention
                | NativeEntryOrigin::CompactionBoundary
        );
    }
    false
}

fn context_source_ids(branch: &[&Entry]) -> std::result::Result<Vec<String>, CompactionProjectionError> {
    let mut ids = Vec::new();
    for entry in branch {
        if !entry.native_compaction_entry()?.messages.is_empty() {
            ids.push(entry.id.clone());
        }
    }
    Ok(ids)
}

/// One decoded message tied to its actual journal entry ID. This is an
/// in-memory snapshot; a later writer must re-read and compare the leaf before
/// committing a compaction derived from it.
#[derive(Debug, Clone, PartialEq)]
pub struct SourcedMessage {
    pub entry_id: String,
    pub message: Message,
}

/// A single raw application record and its immutable Session source. One
/// record can yield zero, one or many provider fragments; no Agent dependency.
#[derive(Debug, Clone, PartialEq)]
pub struct RawAgentContextMessage {
    pub body: AgentMessageBody,
    pub source: AgentEntrySource,
    /// Temporary provider fragments from checked Session source selection.
    /// The Host consumes these into a weak projection cache, not a second
    /// canonical transcript. Serialization/reopen must derive proof anew.
    pub checked_projection: Option<Vec<Message>>,
}

fn raw_checked_summary_projection(
    summary: &CompactionSummaryView,
    model: &ara_ai::Model,
    supports_images: bool,
    blob_store: Option<&BlobStore>,
) -> Result<Message> {
    if summary.has_archive() && summary.archive.is_none() {
        return Err(agent_body_error("exact archive is not representable in scalar model context"));
    }
    if summary.remote.is_some() {
        return remote::checked_raw_model_message(summary, model, supports_images);
    }
    if summary.method == "handoff" {
        return Ok(handoff::model_message(summary));
    }
    let mut message = if summary.archive.is_some() {
        snapcompact::model_message_from_summary_with_blob_store(
            &summary.summary,
            summary.preserve_data.as_ref(),
            blob_store,
        )?
    } else {
        Message::User(UserMessage::text(format!(
            "[Compacted summary of earlier turns; source entries withheld]\n{}",
            summary.summary
        )))
    };
    if let Message::User(user) = &mut message {
        user.timestamp = chrono::DateTime::parse_from_rfc3339(&summary.timestamp)
            .map(|timestamp| timestamp.timestamp_millis())
            .unwrap_or(0);
    }
    Ok(message)
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompactionSourceSnapshot {
    pub session_id: String,
    pub leaf_id: String,
    pub messages: Vec<SourcedMessage>,
}

/// Current readable summary and its still-raw source window. A previous summary
/// is derived evidence, never a fabricated raw source message. Unlike the
/// single-level snapshot, this view also supports a native in-memory journal.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedCompactionSnapshot {
    pub session_id: String,
    pub leaf_id: String,
    pub previous_summary: Option<CompactionSummaryView>,
    pub messages: Vec<SourcedMessage>,
}

/// Verified source shape, independent of its provider-visible role. Only the
/// Session decoder assigns this origin; checked commits compare it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeEntryOrigin {
    User,
    Assistant,
    ToolResult,
    Developer,
    BashExecution,
    HookMessage,
    LegacyCustomMessage,
    LegacyBranchSummary,
    LegacyCompactionSummary,
    FileMention,
    CustomMessage,
    UserSkill,
    SteeringUser,
    LoopGuardNotice,
    BranchSummary,
    Metadata,
    CompactionBoundary,
}

/// One real raw entry, with zero or more ordered model fragments. No cut,
/// token estimate or source ID may split or duplicate this group.
#[derive(Debug, Clone, PartialEq)]
pub struct NativeCompactionEntry {
    pub entry_id: String,
    pub origin: NativeEntryOrigin,
    pub messages: Vec<Message>,
    /// Counted once for raw `type=message`. Native extension-role estimates
    /// currently use a text model projection proxy, including excluded Bash.
    pub raw_token_message: Option<Message>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NativeProjectedCompactionSnapshot {
    pub session_id: String,
    pub leaf_id: String,
    pub previous_summary: Option<CompactionSummaryView>,
    pub entries: Vec<NativeCompactionEntry>,
}

/// A derived summary remains distinct from a user message. The host/Agent
/// owns conversion to a model-visible message and must keep its attribution.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionSummaryView {
    pub entry_id: String,
    pub summary: String,
    /// Generic remote endpoint's shorter display summary. Empty is a supplied
    /// value; absence remains distinct from an empty provider response field.
    pub short_summary: Option<String>,
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    pub timestamp: String,
    /// Missing/null legacy method is normalized to soft. Native methods retain
    /// their identity independently from stored document/placeholder text.
    pub method: String,
    /// Native cumulative file lists, never recovered by parsing model text.
    pub file_details: Option<NativeCompactionFileDetails>,
    /// Provider-native state covers the submitted recent tail as well as the
    /// replaced prefix. This remains derived state, never a raw model receipt.
    pub remote: Option<NativeRemoteReplay>,
    /// Typed bitmap archive, reconstructed from the original preserve slot.
    pub archive: Option<ara_snapcompact::Archive>,
    /// Derived from a source-validated archive and carried through later soft
    /// replacements. It exempts only images in that already covered prefix.
    pub archive_image_boundary: Option<ArchiveImageBoundary>,
    /// Original preserve state remains separate from its runtime projection.
    pub preserve_data: Option<Value>,
    pub preserve_source: Option<NativeSnapcompactArchiveSource>,
    /// `None` means an imported entry has no verifiable source list.
    pub source_entry_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveImageBoundary {
    pub entry_id: String,
    pub first_kept_entry_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompactedContextItem {
    Summary(Box<CompactionSummaryView>),
    Message(Box<SourcedMessage>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompactedContextProjection {
    pub session_id: String,
    pub leaf_id: String,
    pub items: Vec<CompactedContextItem>,
}

/// Strict read failures for provenance-sensitive compaction preparation.
/// Ordinary `branch()` and `build_context()` retain their tolerant OMP port.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompactionSourceError {
    #[error("session has no durable branch to compact")]
    NotDurable,
    #[error("session has unrepaired records; compaction source is incomplete")]
    UnrepairedJournal,
    #[error("session was loaded with invalid UTF-8; compaction source text is not exact")]
    InvalidUtf8,
    #[error("session entry ID {id} occurs more than once")]
    DuplicateEntryId { id: String },
    #[error("session entry has an empty ID")]
    EmptyEntryId,
    #[error("session branch references missing entry {id}")]
    MissingEntry { id: String },
    #[error("session entry {id} has an invalid parentId")]
    InvalidParentId { id: String },
    #[error("session branch has a parent cycle at {id}")]
    ParentCycle { id: String },
    #[error("session parent {parent_id} is not earlier than child {child_id}")]
    ParentNotEarlier { child_id: String, parent_id: String },
    #[error("session message entry {id} cannot be decoded")]
    UndecodableMessage { id: String },
    #[error("session entry {id} has multiple model fragments; use the grouped native snapshot")]
    MultipleMessageProjection { id: String },
    #[error("session entry {id} has unsupported context type {kind}")]
    UnsupportedContextEntry { id: String, kind: String },
}

/// Strict read failures for the source-preserving compaction projection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompactionProjectionError {
    #[error(transparent)]
    Source(#[from] CompactionSourceError),
    #[error("compaction entry {id} has invalid {field}")]
    InvalidField { id: String, field: &'static str },
    #[error("compaction entry {id} uses unsupported method {method}")]
    UnsupportedMethod { id: String, method: String },
    #[error("compaction entry {id} has no kept message on the current branch")]
    MissingKeptMessage { id: String },
    #[error("compaction entry {id} source IDs do not match the replaced message prefix")]
    SourceIdsMismatch { id: String },
    #[error("compaction entry {id} does not replace a complete, supported turn prefix")]
    UnsafeSummaryBoundary { id: String },
    #[error("compaction entry {id} has unsupported replay data")]
    UnsupportedReplayData { id: String },
}

/// Commit failures are returned before recording a summary, except storage
/// errors handled by the existing append rollback.
#[derive(Debug, thiserror::Error)]
pub enum CompactionCommitError {
    #[error(transparent)]
    Projection(#[from] CompactionProjectionError),
    #[error("compaction snapshot no longer matches the current Session branch")]
    StaleSnapshot,
    #[error("compaction summary is empty or exceeds the supported input size")]
    InvalidSummary,
    #[error("compaction window IDs must be the nonempty source prefix before the supported kept message")]
    InvalidWindow,
    #[error("previous compaction has no verifiable raw source IDs")]
    MissingPreviousSources,
    #[error("compaction does not replace a complete, supported turn prefix")]
    UnsafeSummaryBoundary,
    #[error(transparent)]
    Storage(#[from] SessionError),
}

/// What `open` found and repaired.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadReport {
    pub malformed_records: usize,
    /// Backup of the original bytes written before the torn file is rewritten.
    pub backup: Option<PathBuf>,
    /// Unresolved imported image references remain in the raw entry; the Host
    /// can surface these warnings without treating missing bytes as success.
    pub blob_warnings: Vec<BlobWarning>,
}

/// Outcome of [`SessionJournal::recover_interrupted_tool_calls`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recovery {
    /// Calls paired with an "effect unknown" result right after their assistant turn.
    pub paired: Vec<ara_rpc::WireString>,
    /// Earlier calls without results that can no longer be paired adjacently;
    /// left for the provider-side pairing guard and reported to the host.
    pub unpaired_earlier: Vec<ara_rpc::WireString>,
}

/// A checked, temporary branch selection for fixed turn-recovery.ts:998-1059.
/// This token is not persisted: only an actual summary/history rewrite or a
/// restored assistant makes the selected branch durable.
#[derive(Debug)]
pub struct FailedAssistantRecovery {
    session_id: String,
    parent_branch: Vec<Entry>,
    failed_entry: Entry,
    assistant: FailedAssistantRecoveryMessage,
    previous_compaction_id: Option<String>,
    local_history_rewritten: bool,
}

#[derive(Debug)]
enum FailedAssistantRecoveryMessage {
    Legacy(AssistantMessage),
    Native,
}

impl FailedAssistantRecovery {
    /// A checked local transaction has published the matching discard marker.
    /// Hosts retain this fact even when later rescue layers make no progress.
    pub fn local_history_rewritten(&self) -> bool {
        self.local_history_rewritten
    }
    /// Checked recovery receipt retains the original lossless journal source.
    pub fn failed_entry_source(&self) -> AgentEntrySource {
        self.failed_entry.original_source()
    }
    pub fn failed_message_body(&self) -> Result<AgentMessageBody> {
        self.failed_entry.agent_message_body()?.ok_or_else(|| agent_body_error("failed assistant has no original body"))
    }
}

#[derive(Debug, PartialEq)]
pub enum FailedAssistantRecoveryOutcome {
    Committed,
    /// Restore this exact message to the idle Agent before any queued continuation.
    /// Empty error turns are restored only there, not appended to native history.
    Restored {
        message: Box<AssistantMessage>,
        appended_entry_id: Option<String>,
    },
}

fn is_empty_error_turn(message: &AssistantMessage) -> bool {
    message.stop_reason == StopReason::Error
        && !message.content.iter().any(|block| match block {
            AssistantBlock::Text(text) => !ara_prompt::js::trim(&text.text).is_empty(),
            AssistantBlock::Thinking(thinking) => {
                !ara_prompt::js::trim(&thinking.thinking).is_empty()
                    || thinking
                        .thinking_signature
                        .as_deref()
                        .is_some_and(|signature| !ara_prompt::js::trim(signature).is_empty())
            }
            AssistantBlock::RedactedThinking { data } => !ara_prompt::js::trim(data).is_empty(),
            _ => true,
        })
}

/// Fixed OMP's actionable-content test, narrowed to tool-free `Stop` turns.
/// Unsigned thinking is not final output; signed or redacted thinking is.
pub fn is_empty_assistant_stop(message: &AssistantMessage) -> bool {
    message.stop_reason == StopReason::Stop && !message.content.iter().any(is_actionable_assistant_content)
}

fn is_actionable_assistant_content(block: &AssistantBlock) -> bool {
    match block {
        AssistantBlock::Text(text) => !ara_prompt::js::trim(&text.text).is_empty(),
        AssistantBlock::Thinking(thinking) => {
            thinking.thinking_signature.as_deref().is_some_and(|signature| !ara_prompt::js::trim(signature).is_empty())
        }
        _ => true,
    }
}

/// Fixed OMP messages.ts:543-560. Thinking or images cannot anchor an orphan
/// ToolUse terminal. This leaves the older Stop-only discard contract intact.
pub fn is_empty_assistant_terminal(message: &AssistantMessage) -> bool {
    match message.stop_reason {
        StopReason::Stop => is_empty_assistant_stop(message),
        StopReason::ToolUse => !message.content.iter().any(|block| match block {
            AssistantBlock::ToolCall(_) => true,
            AssistantBlock::Text(text) => !ara_prompt::js::trim(&text.text).is_empty(),
            _ => false,
        }),
        _ => false,
    }
}

/// Fixed OMP messages.ts:573-576. Partial failed output and an orphan ToolUse
/// do not prove that this model served a completed assistant turn.
pub fn assistant_turn_produced_output(message: &AssistantMessage) -> bool {
    !matches!(message.stop_reason, StopReason::Error | StopReason::Aborted)
        && !is_empty_assistant_terminal(message)
        && message.content.iter().any(is_actionable_assistant_content)
}

fn failed_recovery_error(message: impl Into<String>) -> SessionError {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into()).into()
}

/// Fixed b91 session_init wire contract. Credentials and Host authority are never persisted here.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedChildInit {
    #[serde(deserialize_with = "child_system_blocks")]
    pub system_prompt: Vec<String>,
    pub task: String,
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub model_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub resolved_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub retry_fallback: Option<ChildRetryFallback>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub read_only: Option<bool>,
    #[serde(default, deserialize_with = "child_present_json", skip_serializing_if = "Option::is_none")] pub output_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub output_schema_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub restrict_tool_names: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub spawns: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub read_summarize: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub advisor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub compaction_threshold: Option<ChildCompactionThreshold>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub isolated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub work_pool_yield_items: Option<Vec<ChildWorkPoolItem>>,
    /// Unknown future metadata survives extraction/republication; tree keys are excluded below.
    #[serde(default, flatten)] pub extra: serde_json::Map<String, Value>,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChildRetryFallback { pub primary: String, pub chain: Vec<String> }
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildCompactionThreshold { pub threshold_percent: f64, pub threshold_tokens: f64 }
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChildWorkPoolItem { pub id: String, pub index: usize }
fn child_present_json<'de,D:serde::Deserializer<'de>>(input:D)->std::result::Result<Option<Value>,D::Error>{ <Value as serde::Deserialize>::deserialize(input).map(Some) }
fn child_system_blocks<'de,D:serde::Deserializer<'de>>(input:D)->std::result::Result<Vec<String>,D::Error>{
    #[derive(serde::Deserialize)] #[serde(untagged)] enum Blocks { Legacy(String), Current(Vec<String>) }
    match <Blocks as serde::Deserialize>::deserialize(input)? { Blocks::Legacy(block)=>Ok(vec![block]), Blocks::Current(blocks)=>Ok(blocks) }
}

pub struct SessionJournal {
    path: PathBuf,
    persistent: bool,
    header: Value,
    title: TitleSlot,
    entries: Vec<Entry>,
    ids: HashSet<String>,
    leaf: Option<String>,
    materialized: bool,
    rewrite_required: bool,
    loaded_invalid_utf8: bool,
    pending_backup: bool,
    blob_store: Option<BlobStore>,
    pub report: LoadReport,
}

impl SessionJournal {
    /// New lazy session in `session_dir` for `cwd` (file created on first assistant message).
    pub fn create(session_dir: &Path, cwd: &Path) -> Result<SessionJournal> {
        Self::create_with_parent(session_dir, cwd, None)
    }

    /// Explicit new-session RPC uses the same journal with an optional parent.
    pub fn create_with_parent(session_dir: &Path, cwd: &Path, parent: Option<&str>) -> Result<SessionJournal> {
        fs::create_dir_all(session_dir)?;
        let id = uuid::Uuid::now_v7().to_string();
        let timestamp = now_iso();
        let path = session_dir.join(format!("{}_{}.jsonl", file_safe_timestamp(&timestamp), id));
        let mut header = json!({
            "type": "session",
            "version": CURRENT_SESSION_VERSION,
            "id": id,
            "timestamp": timestamp,
            "cwd": cwd.to_string_lossy(),
        });
        if let Some(parent) = parent.filter(|parent| !parent.is_empty()) {
            header["parentSession"] = json!(parent);
        }
        Ok(SessionJournal {
            path,
            persistent: true,
            title: TitleSlot { updated_at: timestamp, ..Default::default() },
            header,
            entries: Vec::new(),
            ids: HashSet::new(),
            leaf: None,
            materialized: false,
            rewrite_required: false,
            loaded_invalid_utf8: false,
            pending_backup: false,
            blob_store: None,
            report: LoadReport::default(),
        })
    }

    /// Native in-memory journal with the host's Session identity and no file effects.
    pub fn in_memory(header: Value) -> Result<SessionJournal> {
        if !is_valid_header(&header) || header.get("version").and_then(Value::as_u64) != Some(CURRENT_SESSION_VERSION) {
            return Err(SessionError::Corrupt { path: PathBuf::new(), message: CORRUPT_HEADER_MESSAGE.into() });
        }
        let title = TitleSlot {
            title: header.get("title").and_then(Value::as_str).unwrap_or_default().into(),
            source: header.get("titleSource").and_then(Value::as_str).map(str::to_string),
            updated_at: header.get("timestamp").and_then(Value::as_str).unwrap_or_default().into(),
        };
        Ok(SessionJournal {
            path: PathBuf::new(),
            persistent: false,
            header,
            title,
            entries: Vec::new(),
            ids: HashSet::new(),
            leaf: None,
            materialized: false,
            rewrite_required: false,
            loaded_invalid_utf8: false,
            pending_backup: false,
            blob_store: None,
            report: LoadReport::default(),
        })
    }

    /// Prepare a new native Session through `leaf`, leaving this source unchanged.
    /// `None` means the root-user branch flow: an empty Session and inherited
    /// name action. A directory forces a durable fork; no directory keeps it
    /// in memory. Missing parents and cycles retain fixed OMP's tolerant path walk.
    pub fn fork_at(&self, leaf: Option<&str>, session_dir: Option<&Path>) -> Result<SessionJournal> {
        let branch = self.path_to(leaf);
        if let Some(leaf) = leaf
            && branch.is_empty()
        {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("Entry {leaf} not found")).into());
        }
        let timestamp = now_iso();
        let id = uuid::Uuid::now_v7().to_string();
        let mut header = json!({
            "type": "session", "version": CURRENT_SESSION_VERSION,
            "id": id, "timestamp": timestamp, "cwd": self.header["cwd"],
        });
        if session_dir.is_some() && self.persistent {
            header["parentSession"] = json!(self.path);
        }
        // Fixed AgentSession's root flow calls newSession without workspace
        // options. createBranchedSession preserves the additional directories.
        if leaf.is_some()
            && let Some(directories) = self
                .header
                .get("additionalDirectories")
                .filter(|value| value.as_array().is_some_and(|directories| !directories.is_empty()))
        {
            header["additionalDirectories"] = directories.clone();
        }
        let mut fork = Self::in_memory(header)?;
        fork.blob_store = self.blob_store.clone();
        if leaf.is_none() {
            if !self.title.title.is_empty() {
                fork.set_session_name(&self.title.title, self.title.source.as_deref().unwrap_or("auto"))?;
            }
        } else {
            fork.title = TitleSlot { updated_at: timestamp.clone(), ..self.title.clone() };
            if !fork.title.title.is_empty() {
                fork.header["title"] = json!(fork.title.title);
            }
            if let Some(source) = &fork.title.source {
                fork.header["titleSource"] = json!(source);
            }
            fork.entries = branch.into_iter().filter(|entry| entry.kind != "label").cloned().collect();
            fork.ids = fork.entries.iter().map(|entry| entry.id.clone()).collect();
            fork.leaf = fork.entries.last().map(|entry| entry.id.clone());

            // SessionEntryIndex resolves labels over ALL entries, including
            // off-path edits. Vec preserves JS Map insertion order; deletion
            // followed by a new label moves that target to the end.
            let mut labels: Vec<(String, String)> = Vec::new();
            for entry in self.entries.iter().filter(|entry| entry.kind == "label") {
                let Some(target) = entry.raw.get("targetId").and_then(Value::as_str) else {
                    continue;
                };
                match entry.raw.get("label").and_then(Value::as_str).filter(|label| !label.is_empty()) {
                    Some(label) => {
                        if let Some((_, current)) = labels.iter_mut().find(|(id, _)| id.as_str() == target) {
                            *current = label.to_owned();
                        } else {
                            labels.push((target.to_owned(), label.to_owned()));
                        }
                    }
                    None => labels.retain(|(id, _)| id.as_str() != target),
                }
            }
            let labels_to_carry =
                labels.into_iter().filter(|(target, _)| fork.ids.contains(target)).collect::<Vec<_>>();
            for (target, label) in labels_to_carry {
                let mut fields = serde_json::Map::new();
                fields.insert("targetId".into(), json!(target));
                fields.insert("label".into(), json!(label));
                fork.append_raw("label", fields)?;
            }
        }
        if let Some(directory) = session_dir {
            fork.path = directory.join(format!("{}_{}.jsonl", file_safe_timestamp(&timestamp), id));
            fork.persistent = true;
            fork.materialize()?;
        }
        Ok(fork)
    }

    /// Load an existing session. Malformed records are skipped (the file is
    /// rewritten, after a backup, before the next append). A missing, corrupt
    /// or unsupported-version header is an error that leaves the file untouched.
    pub fn open(path: &Path) -> Result<SessionJournal> {
        let corrupt = |message: String| SessionError::Corrupt { path: path.into(), message };
        let bytes = fs::read(path)?;
        let text = String::from_utf8_lossy(&bytes);
        let loaded_invalid_utf8 = matches!(&text, std::borrow::Cow::Owned(_));
        let mut header: Option<Value> = None;
        let mut title: Option<TitleSlot> = None;
        let mut entries = Vec::new();
        let mut ids = HashSet::new();
        let mut malformed = 0;
        let mut first_record = true;
        for line in text.split('\n') {
            if line.trim().is_empty() {
                continue;
            }
            let parsed = ara_rpc::WireValue::parse(line).ok();
            let Some(mut wire) = parsed.filter(ara_rpc::WireValue::is_object) else {
                if header.is_none() {
                    return Err(corrupt(CORRUPT_HEADER_MESSAGE.into()));
                }
                malformed += 1;
                continue;
            };
            let native_message = native_message::take_entry_source(&mut wire);
            let snapcompact_source = snapcompact_source::take_entry_source(&mut wire);
            let mut value = match native_message::checked_serde(&wire) {
                Ok(value) => value,
                Err(_) if header.is_some() => {
                    malformed += 1;
                    continue;
                }
                Err(_) => return Err(corrupt(CORRUPT_HEADER_MESSAGE.into())),
            };
            if let Some(source) = &native_message {
                if let Ok(observed) = source.checked_entry(&value) {
                    value = observed;
                }
            }
            // Archive healing works on exact data before optional checked mirror.
            let snapcompact_source = match snapcompact_source {
                Some(source) => Some(source.repair_truncated()?),
                None => None,
            };
            if let Some(source) = &snapcompact_source {
                if let Ok(observed) = source.checked_entry(&value) {
                    value = observed;
                }
            }
            if first_record && is_title_slot(&value) {
                title = Some(TitleSlot {
                    title: value.get("title").and_then(Value::as_str).unwrap_or_default().into(),
                    source: value.get("source").and_then(Value::as_str).map(str::to_string),
                    updated_at: value.get("updatedAt").and_then(Value::as_str).unwrap_or_default().into(),
                });
                first_record = false;
                continue;
            }
            first_record = false;
            if header.is_none() {
                if !is_valid_header(&value) {
                    return Err(corrupt(CORRUPT_HEADER_MESSAGE.into()));
                }
                let version = value.get("version").and_then(Value::as_u64);
                if version != Some(CURRENT_SESSION_VERSION) {
                    let shown = version.map(|v| v.to_string()).unwrap_or_else(|| "1 (no version field)".into());
                    return Err(corrupt(format!("unsupported session version {shown}; migrations are not ported yet")));
                }
                header = Some(value);
                continue;
            }
            let id = value.get("id").and_then(Value::as_str).map(str::to_string);
            let kind = value.get("type").and_then(Value::as_str).map(str::to_string);
            match (id, kind) {
                (Some(id), Some(kind)) => {
                    let parent_id = value.get("parentId").and_then(Value::as_str).map(str::to_string);
                    ids.insert(id.clone());
                    // Duplicate ids keep both lines; lookups resolve to the last (OMP index semantics).
                    let mut value = value;
                    blob::repair_truncated_archive(&mut value);
                    entries.push(Entry { id, parent_id, kind, raw: value, native_message, snapcompact_source });
                }
                _ => malformed += 1,
            }
        }
        let Some(header) = header else {
            return Err(corrupt(CORRUPT_HEADER_MESSAGE.into()));
        };
        let title = title.unwrap_or_else(|| TitleSlot {
            title: header.get("title").and_then(Value::as_str).unwrap_or_default().into(),
            source: header.get("titleSource").and_then(Value::as_str).map(str::to_string),
            updated_at: header.get("timestamp").and_then(Value::as_str).unwrap_or_default().into(),
        });
        let damaged = malformed > 0 || bytes.last().is_some_and(|b| *b != b'\n');
        let leaf = entries.last().map(|e| e.id.clone());
        Ok(SessionJournal {
            path: path.into(),
            persistent: true,
            header,
            title,
            entries,
            ids,
            leaf,
            materialized: true,
            rewrite_required: damaged,
            loaded_invalid_utf8,
            pending_backup: damaged,
            blob_store: None,
            report: LoadReport { malformed_records: malformed, backup: None, blob_warnings: Vec::new() },
        })
    }

    /// The Host supplies the same global blob directory for every Session.
    /// Resolution completes before this constructor returns a usable journal.
    pub fn open_with_blob_directory(path: &Path, blob_directory: &Path) -> Result<SessionJournal> {
        let mut journal = Self::open(path)?;
        journal.bind_blob_directory(blob_directory)?;
        Ok(journal)
    }

    pub fn create_with_blob_directory(session_dir: &Path, cwd: &Path, blob_directory: &Path) -> Result<SessionJournal> {
        Self::create_with_parent_and_blob_directory(session_dir, cwd, None, blob_directory)
    }

    pub fn create_with_parent_and_blob_directory(
        session_dir: &Path,
        cwd: &Path,
        parent: Option<&str>,
        blob_directory: &Path,
    ) -> Result<SessionJournal> {
        let mut journal = Self::create_with_parent(session_dir, cwd, parent)?;
        journal.bind_blob_directory(blob_directory)?;
        Ok(journal)
    }

    /// Prepare all resolutions before adopting them, including for journals
    /// constructed in memory by an SDK Host. A read error leaves the view intact.
    pub fn bind_blob_directory(&mut self, blob_directory: &Path) -> Result<()> {
        let store = BlobStore::new(blob_directory);
        let mut entries = self.entries.clone();
        let mut warnings = Vec::new();
        for entry in &mut entries {
            blob::repair_truncated_archive(&mut entry.raw);
            if let Some(source) = &entry.snapcompact_source {
                let mut wire = source.repair_truncated()?.wire_entry(&entry.raw)?;
                blob::resolve_native_entry(&mut wire, &store, &mut warnings)?;
                let source = snapcompact_source::take_owned_entry_source(&mut wire).ok_or_else(|| {
                    native_message::invalid("exact archive source disappeared during blob resolution")
                })?;
                entry.raw = native_message::checked_serde(&wire)?;
                if let Ok(observed) = source.checked_entry(&entry.raw) {
                    entry.raw = observed;
                }
                entry.snapcompact_source = Some(source);
                // Resolve the complete entry once. A checked mirror of an
                // unresolved ref must not produce a second warning receipt.
                continue;
            }
            if let Some(source) = &entry.native_message {
                let NativeMessageSource::Persisted { body } = source else {
                    return Err(agent_body_error(
                        "bind_blob_directory requires durable native sources before realm binding",
                    ));
                };
                let mut wire = native_message::clone_wire(body);
                blob::resolve_native_entry(&mut wire, &store, &mut warnings)?;
                let source = NativeMessageSource::persisted(wire);
                entry.raw = native_message::entry_envelope(&entry.raw);
                if let Ok(observed) = source.checked_entry(&entry.raw) {
                    entry.raw = observed;
                }
                entry.native_message = Some(source);
            }
            blob::resolve_entry(&mut entry.raw, &store, &mut warnings)?;
        }
        self.entries = entries;
        self.report.blob_warnings.extend(warnings);
        self.blob_store = Some(store);
        Ok(())
    }

    pub fn blob_directory(&self) -> Option<&Path> {
        self.blob_store.as_ref().map(BlobStore::directory)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn is_persistent(&self) -> bool {
        self.persistent
    }
    pub fn session_id(&self) -> &str {
        self.header["id"].as_str().unwrap_or_default()
    }
    pub fn header(&self) -> &Value {
        &self.header
    }
    pub fn title(&self) -> &TitleSlot {
        &self.title
    }

    /// Apply an explicit (`user`) or generated (`auto`) Session name.
    /// Empty cleaned titles and an auto rename after a user name return false.
    /// Like fixed OMP, a rename alone preserves lazy materialization. On disk,
    /// this journal's atomic rewrite commits the slot, header and title-change
    /// receipt together; a failed write restores the public in-memory name.
    pub fn set_session_name(&mut self, name: &str, source: &str) -> Result<bool> {
        if self.title.source.as_deref() == Some("user") && source == "auto" {
            return Ok(false);
        }
        let name = normalize_session_name(name);
        if name.is_empty() {
            return Ok(false);
        }
        let previous_title = self.title.clone();
        let previous_header = self.header.clone();
        let timestamp = now_iso();
        self.title = TitleSlot { title: name.clone(), source: Some(source.into()), updated_at: timestamp.clone() };
        self.header["title"] = json!(name);
        self.header["titleSource"] = json!(source);
        let mut fields = serde_json::Map::new();
        fields.insert("title".into(), json!(name));
        fields.insert("source".into(), json!(source));
        fields.insert("timestamp".into(), json!(timestamp));
        if !previous_title.title.is_empty() {
            fields.insert("previousTitle".into(), json!(previous_title.title));
        }
        // append_raw alone only appends a receipt to an existing file. The
        // title slot and header need to change in the same durable operation.
        self.rewrite_required |= self.materialized;
        if let Err(error) = self.append_raw("title_change", fields) {
            self.title = previous_title;
            self.header = previous_header;
            return Err(error);
        }
        Ok(true)
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub fn leaf_id(&self) -> Option<&str> {
        self.leaf.as_deref()
    }
    pub fn is_on_disk(&self) -> bool {
        self.materialized
    }

    fn line_for(value: &Value) -> String {
        format!("{value}\n")
    }

    fn persisted_line(&self, entry: &Entry) -> std::io::Result<String> {
        if let Some(source) = &entry.snapcompact_source {
            return source.encoded_line(&entry.raw, self.blob_store.as_ref());
        }
        if let Some(source) = &entry.native_message {
            let envelope = native_message::entry_envelope(&entry.raw);
            return source.encoded_line(&envelope, self.blob_store.as_ref());
        }
        match &self.blob_store {
            Some(store) => Ok(Self::line_for(&blob::prepare_entry(&entry.raw, store)?)),
            None => Ok(Self::line_for(&entry.raw)),
        }
    }

    fn file_body(&self) -> std::io::Result<String> {
        let mut body = serialize_title_slot(&self.title);
        body.push_str(&Self::line_for(&self.header));
        for e in &self.entries {
            body.push_str(&self.persisted_line(e)?);
        }
        Ok(body)
    }

    fn has_assistant(&self) -> Result<bool> {
        for entry in &self.entries {
            if entry.observed_role()?.is_some_and(|role| role.equals_ascii("assistant")) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn dir(&self) -> PathBuf {
        self.path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
    }

    /// Atomic full rewrite: synced backup of damaged bytes, unique temp file,
    /// fsync, rename, and directory fsync where supported.
    fn rewrite(&mut self) -> Result<()> {
        self.rewrite_with_directory_sync(sync_dir).map_err(|error| match error {
            RewritePhaseError::BeforePublication(error) | RewritePhaseError::PublishedButDurabilityUnknown(error) => {
                error
            }
        })
    }

    fn rewrite_with_directory_sync(
        &mut self,
        directory_sync: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> std::result::Result<(), RewritePhaseError> {
        if !self.persistent {
            return Ok(());
        }
        let dir = self.dir();
        fs::create_dir_all(&dir).map_err(|error| RewritePhaseError::BeforePublication(error.into()))?;
        if self.pending_backup && self.path.exists() {
            let backup = self.path.with_extension(format!("jsonl.torn-{}.bak", now_ms()));
            let backed_up = (|| -> std::io::Result<()> {
                fs::copy(&self.path, &backup)?;
                OpenOptions::new().write(true).open(&backup)?.sync_all()?;
                sync_dir(&dir)
            })();
            backed_up.map_err(|error| RewritePhaseError::BeforePublication(error.into()))?;
            self.report.backup = Some(backup);
            self.pending_backup = false;
        }
        let tmp =
            self.path.with_extension(format!("jsonl.tmp-{}-{}", std::process::id(), uuid::Uuid::new_v4().simple()));
        let written = (|| -> std::io::Result<()> {
            let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            f.write_all(self.file_body()?.as_bytes())?;
            f.sync_all()?;
            fs::rename(&tmp, &self.path)
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&tmp);
            return Err(RewritePhaseError::BeforePublication(e.into()));
        }
        directory_sync(&dir).map_err(|error| RewritePhaseError::PublishedButDurabilityUnknown(error.into()))?;
        self.materialized = true;
        self.rewrite_required = false;
        Ok(())
    }

    fn persist(&mut self, entry: &Entry) -> Result<()> {
        if !self.persistent {
            return Ok(());
        }
        if !self.materialized && !self.has_assistant()? {
            return Ok(()); // lazy: nothing on disk until an assistant message exists
        }
        if !self.materialized || self.rewrite_required {
            return self.rewrite();
        }
        let line = self.persisted_line(entry)?;
        let mut f = OpenOptions::new().append(true).open(&self.path)?;
        f.write_all(line.as_bytes())?;
        f.sync_data()?;
        Ok(())
    }

    /// Stage a native entry without an independent persistence boundary.
    /// Only the caller may publish the candidate or roll it back as a unit.
    fn stage_raw(&mut self, kind: &str, mut fields: serde_json::Map<String, Value>) -> String {
        let id = generate_id(&self.ids);
        let mut raw = serde_json::Map::new();
        raw.insert("type".into(), json!(kind));
        raw.insert("id".into(), json!(id));
        raw.insert("parentId".into(), self.leaf.clone().map(Value::String).unwrap_or(Value::Null));
        raw.insert("timestamp".into(), json!(now_iso()));
        raw.append(&mut fields);
        let raw = Value::Object(raw);
        self.entries.push(Entry {
            id: id.clone(),
            parent_id: self.leaf.clone(),
            kind: kind.into(),
            raw,
            native_message: None,
            snapcompact_source: None,
        });
        self.ids.insert(id.clone());
        self.leaf = Some(id.clone());
        id
    }

    fn append_raw(&mut self, kind: &str, fields: serde_json::Map<String, Value>) -> Result<String> {
        self.append_raw_with_snapcompact_source(kind, fields, None)
    }

    fn append_raw_with_snapcompact_source(
        &mut self,
        kind: &str,
        fields: serde_json::Map<String, Value>,
        source: Option<NativeSnapcompactArchiveSource>,
    ) -> Result<String> {
        let previous_leaf = self.leaf.clone();
        let id = self.stage_raw(kind, fields);
        self.entries.last_mut().expect("staged entry").snapcompact_source = source;
        let entry = self.entries.last().expect("staged entry").clone();
        if let Err(e) = self.persist(&entry) {
            // Roll back so `Err` means "not recorded"; a partially written line is
            // replaced by the full rewrite the next append performs.
            self.entries.pop();
            self.ids.remove(&id);
            self.leaf = previous_leaf;
            self.rewrite_required = true;
            return Err(e);
        }
        Ok(id)
    }

    /// Latest loaded session_init, including entries on retained branches, as fixed extractSessionInit.
    pub fn latest_child_init(&self) -> Result<Option<PersistedChildInit>> {
        let Some(entry)=self.entries.iter().rev().find(|entry|entry.kind=="session_init") else {return Ok(None)};
        let mut raw=entry.raw.as_object().cloned().ok_or_else(||SessionError::Corrupt{path:self.path.clone(),message:"session_init is not an object".into()})?;
        for key in ["type","id","parentId","timestamp"] { raw.remove(key); }
        serde_json::from_value(Value::Object(raw)).map(Some).map_err(|error|SessionError::Corrupt{path:self.path.clone(),message:format!("invalid child session_init: {error}")})
    }
    /// Uses the same append/rollback/leaf bookkeeping as every native metadata entry.
    /// Lazy journals remain lazy; a Host requiring durable init first materializes its own child.
    pub fn append_child_init(&mut self, init:&PersistedChildInit) -> Result<String> {
        let value=serde_json::to_value(init).map_err(|error|SessionError::Corrupt{path:self.path.clone(),message:format!("invalid child session_init: {error}")})?;
        let mut fields=value.as_object().cloned().expect("child init serializes an object");
        for key in ["type","id","parentId","timestamp"] { fields.remove(key); }
        self.append_raw("session_init",fields)
    }

    /// Append the original native body, using the existing ID/branch/writer and
    /// rollback owner. This requires no successful legacy Message projection.
    pub fn append_native_message(&mut self, source: NativeMessageSource) -> Result<String> {
        let role = source.observe_field(&["role"])?;
        let Some(role) = role.as_ref().and_then(ara_rpc::WireValue::as_string).filter(|role| !role.is_empty()) else {
            return Err(agent_body_error("native message requires a nonempty role"));
        };
        let custom_timestamp = source.custom_entry_timestamp(role)?;
        let mut fields = serde_json::Map::new();
        if let Some(timestamp) = &custom_timestamp {
            fields.insert("timestamp".into(), json!(timestamp));
        }
        let previous_leaf = self.leaf.clone();
        let kind = if custom_timestamp.is_some() { "custom_message" } else { "message" };
        let id = self.stage_raw(kind, fields);
        let staged = self.entries.last_mut().expect("staged native entry");
        if matches!(&source, NativeMessageSource::Persisted { .. })
            && let Ok(observed) = source.checked_entry(&staged.raw)
        {
            staged.raw = observed;
        }
        staged.native_message = Some(source);
        let entry = self.entries.last().expect("staged native entry").clone();
        if let Err(error) = self.persist(&entry) {
            self.entries.pop();
            self.ids.remove(&id);
            self.leaf = previous_leaf;
            self.rewrite_required = true;
            return Err(error);
        }
        Ok(id)
    }

    /// Prepare an unpublished fork for a different logical Session realm.
    /// Encode Live bodies through their original durable boundary; keep IDs,
    /// parent links, native UTF16 and the source journal untouched. Callers then
    /// bind the completed candidate through bind_native_realm.
    pub fn prepare_native_fork_for_realm(&mut self) -> Result<()> {
        let mut entries = self.entries.clone();
        for entry in &mut entries {
            if let Some(source @ NativeMessageSource::Live { .. }) = &entry.native_message {
                let line = source.encoded_line(&entry.raw, self.blob_store.as_ref())?;
                let encoded = ara_rpc::WireValue::parse(line.trim_end_matches('\n'))
                    .map_err(|error| agent_body_error(error.to_string()))?;
                let body = native_message::body_from_entry(&encoded)?
                    .ok_or_else(|| agent_body_error("native fork has no durable body"))?;
                entry.native_message = Some(NativeMessageSource::persisted(body));
                entry.raw = native_message::entry_envelope(&entry.raw);
            }
        }
        self.entries = entries;
        Ok(())
    }

    /// Bind all durable message bodies in one candidate to the current Host.
    /// A failure leaves this journal's canonical view and IDs intact.
    pub fn bind_native_realm(
        &mut self,
        heap: &std::sync::Arc<ara_rpc::value_heap::ValueHeap>,
        encoder: std::sync::Arc<dyn NativeJournalEncoder>,
    ) -> Result<()> {
        let mut entries = self.entries.clone();
        for entry in &mut entries {
            if let Some(source) = &entry.native_message {
                if let NativeMessageSource::Live { body, .. } = source {
                    heap.adopt(body).map_err(|error| agent_body_error(error.to_string()))?;
                    continue;
                }
                entry.native_message = Some(source.bind_current(heap, encoder.clone())?);
                // Current graph observations are used by every checked consumer.
                entry.raw = native_message::entry_envelope(&entry.raw);
            }
        }
        self.entries = entries;
        Ok(())
    }

    pub fn append_message(&mut self, message: &Message) -> Result<String> {
        let mut f = serde_json::Map::new();
        f.insert("message".into(), serde_json::to_value(message).expect("message serializes"));
        self.append_raw("message", f)
    }

    /// Append the CURRENT raw body once using the existing rollback owner.
    /// Specialized custom envelopes retain extensions and the input timestamp.
    pub fn append_agent_message_body(&mut self, body: &Value) -> Result<String> {
        Entry::project_agent_message_body(body)?;
        if body["role"] == "custom" && !["id", "parentId", "type"].iter().any(|field| body.get(*field).is_some()) {
            let timestamp = body
                .get("timestamp")
                .and_then(Value::as_i64)
                .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
                .ok_or_else(|| agent_body_error("invalid custom message timestamp"))?
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string();
            let mut fields = body.as_object().expect("validated object").clone();
            fields.remove("role");
            fields.insert("timestamp".into(), json!(timestamp));
            self.append_raw("custom_message", fields)
        } else {
            // A raw extension named id/type/parentId must not overwrite the
            // journal envelope. Its full body stays under the message slot.
            let mut fields = serde_json::Map::new();
            fields.insert("message".into(), body.clone());
            self.append_raw("message", fields)
        }
    }

    /// Fixed OMP `appendResetBoundary`: retain Session identity, settings and
    /// raw history while starting model context after this durable marker.
    /// The host owns idle admission and resetting its live Agent state.
    pub fn append_reset_boundary(&mut self) -> Result<String> {
        self.append_raw("reset_boundary", serde_json::Map::new())
    }

    /// Select the exact failed assistant's parent only in memory. The original
    /// receipt and any off-branch children remain unchanged until real recovery.
    /// This first recovery surface is deliberately limited to tool-free errors
    /// and length stops; a failed tool call is not proof of absent side effects.
    pub fn begin_failed_assistant_recovery(
        &mut self,
        expected_leaf: &str,
        entry_id: &str,
        assistant: &AssistantMessage,
    ) -> Result<FailedAssistantRecovery> {
        if self.leaf_id() != Some(expected_leaf)
            || !matches!(assistant.stop_reason, StopReason::Error | StopReason::Length)
            || assistant.tool_calls().next().is_some()
        {
            return Err(failed_recovery_error("failed assistant recovery requires the exact tool-free failed branch"));
        }
        let branch = self.strict_compaction_branch().map_err(|error| failed_recovery_error(error.to_string()))?;
        let index = branch
            .iter()
            .position(|entry| entry.id == entry_id)
            .ok_or_else(|| failed_recovery_error("failed assistant entry is outside the current branch"))?;
        if branch[index].message()? != Some(Message::Assistant(assistant.clone()))
            || branch[index + 1..].iter().any(|entry| !entry.is_service_tier_change())
        {
            return Err(failed_recovery_error("failed assistant entry or metadata tail changed"));
        }
        let parent_branch = branch[..index].iter().map(|entry| (**entry).clone()).collect::<Vec<_>>();
        let parent_context = &branch[..index];
        let previous_compaction_id = parent_context[active_context_start(parent_context)..]
            .iter()
            .rev()
            .find(|entry| entry.kind == "compaction")
            .map(|entry| entry.id.clone());
        let token = FailedAssistantRecovery {
            session_id: self.session_id().to_owned(),
            parent_branch,
            failed_entry: (*branch[index]).clone(),
            assistant: FailedAssistantRecoveryMessage::Legacy(assistant.clone()),
            previous_compaction_id,
            local_history_rewritten: false,
        };
        self.leaf = token.failed_entry.parent_id.clone();
        Ok(token)
    }

    /// Finish the temporary selection after the host's owned recovery attempt.
    /// No-progress rollback appends a new native ID for contentful output and
    /// always returns the failed message for restoring the idle Agent's tail.
    pub fn finish_failed_assistant_recovery(
        &mut self,
        token: FailedAssistantRecovery,
        history_rewritten: bool,
    ) -> Result<FailedAssistantRecoveryOutcome> {
        let FailedAssistantRecoveryMessage::Legacy(assistant) = &token.assistant else {
            return Err(failed_recovery_error("native recovery requires its native finish API"));
        };
        let empty_error = is_empty_error_turn(assistant);
        let Some(appended_entry_id) =
            self.finish_failed_assistant_recovery_transaction(&token, history_rewritten, empty_error)?
        else {
            return Ok(FailedAssistantRecoveryOutcome::Committed);
        };
        let FailedAssistantRecoveryMessage::Legacy(assistant) = token.assistant else { unreachable!() };
        Ok(FailedAssistantRecoveryOutcome::Restored { message: Box::new(assistant), appended_entry_id })
    }

    // Shared branch publication/restore transaction, independent of a typed body.
    fn finish_failed_assistant_recovery_transaction(
        &mut self,
        token: &FailedAssistantRecovery,
        history_rewritten: bool,
        empty_error: bool,
    ) -> Result<Option<Option<String>>> {
        if self.session_id() != token.session_id {
            return Err(failed_recovery_error("failed assistant recovery belongs to another Session"));
        }
        let branch = if self.leaf.is_none() {
            Vec::new()
        } else {
            self.strict_compaction_branch().map_err(|error| failed_recovery_error(error.to_string()))?
        };
        if branch.len() < token.parent_branch.len()
            || branch.iter().zip(&token.parent_branch).any(|(current, expected)| *current != expected)
            || branch[token.parent_branch.len()..].iter().any(|entry| {
                entry.kind != "compaction"
                    && !entry.is_service_tier_change()
                    && !entry.is_discarded_entry_branch_marker()
            })
        {
            return Err(failed_recovery_error("failed assistant recovery owner branch changed"));
        }
        let latest_compaction = branch[active_context_start(&branch)..]
            .iter()
            .rev()
            .find(|entry| entry.kind == "compaction")
            .map(|entry| &entry.id);
        if latest_compaction.map(String::as_str) != token.previous_compaction_id.as_deref() {
            self.compacted_context_projection().map_err(|error| failed_recovery_error(error.to_string()))?;
            return Ok(None);
        }
        let durable_discard = branch[token.parent_branch.len()..].iter().any(|entry| {
            entry.is_discarded_entry_branch_marker()
                && entry.raw.pointer("/details/discardedEntryId").and_then(Value::as_str)
                    == Some(token.failed_entry.id.as_str())
        });
        if history_rewritten && durable_discard {
            return Ok(None);
        }
        if history_rewritten
            || durable_discard
            || self.entries.iter().find(|entry| entry.id == token.failed_entry.id) != Some(&token.failed_entry)
        {
            return Err(failed_recovery_error("failed assistant recovery has no matching committed history receipt"));
        }
        let appended_entry_id = if empty_error {
            None
        } else {
            Some(match &token.failed_entry.native_message {
                Some(source) => self.append_native_message(source.clone())?,
                None => self.append_agent_message_body(&token.failed_entry.raw["message"])?,
            })
        };
        Ok(Some(appended_entry_id))
    }

    /// Fixed session-manager.ts:2678-2715. Durably bypass this exact failed
    /// assistant. Known service-tier children are reparented in physical order;
    /// any potentially content-bearing subtree is retained off branch.
    pub fn discard_entry_durably(
        &mut self,
        expected_leaf: Option<&str>,
        entry_id: &str,
        assistant: &AssistantMessage,
    ) -> Result<Option<String>> {
        if !matches!(assistant.stop_reason, StopReason::Error | StopReason::Length)
            || assistant.tool_calls().next().is_some()
        {
            return Err(failed_recovery_error("durable discard requires a tool-free failed assistant"));
        }
        self.discard_assistant_durably(expected_leaf, entry_id, assistant, None)
    }

    /// Drop an exact empty Stop before continuation or after its retry cap.
    /// Other terminal reasons retain the existing failed-assistant API.
    pub fn discard_empty_stop_durably(
        &mut self,
        expected_leaf: Option<&str>,
        entry_id: &str,
        assistant: &AssistantMessage,
    ) -> Result<Option<String>> {
        if !is_empty_assistant_stop(assistant) {
            return Err(failed_recovery_error("durable empty-stop discard requires an empty Stop assistant"));
        }
        self.discard_assistant_durably(expected_leaf, entry_id, assistant, None)
    }

    /// Discard the exact native empty Stop or orphan ToolUse receipt before
    /// continuation. Existing failure and accepted-empty APIs remain narrow.
    pub fn discard_empty_assistant_terminal_durably(
        &mut self,
        expected_leaf: Option<&str>,
        entry_id: &str,
        assistant: &AssistantMessage,
    ) -> Result<Option<String>> {
        if !is_empty_assistant_terminal(assistant) {
            return Err(failed_recovery_error("durable terminal discard requires an empty Stop or orphan ToolUse"));
        }
        self.discard_assistant_durably(expected_leaf, entry_id, assistant, None)
    }

    /// Commit failed-turn discard and a prepared host model promotion together.
    /// There is no durable intermediate branch without its model-change receipt.
    pub fn discard_failed_assistant_for_promotion(
        &mut self,
        expected_leaf: Option<&str>,
        entry_id: &str,
        assistant: &AssistantMessage,
        target: &str,
    ) -> Result<Option<String>> {
        if !matches!(assistant.stop_reason, StopReason::Error | StopReason::Length)
            || assistant.tool_calls().next().is_some()
        {
            return Err(failed_recovery_error("model promotion requires a tool-free failed assistant"));
        }
        self.discard_assistant_durably(expected_leaf, entry_id, assistant, Some(target))
    }

    fn discard_assistant_durably(
        &mut self,
        expected_leaf: Option<&str>,
        entry_id: &str,
        assistant: &AssistantMessage,
        promotion_model: Option<&str>,
    ) -> Result<Option<String>> {
        if self.leaf_id() != expected_leaf {
            return Err(failed_recovery_error("durable discard branch changed"));
        }
        let Some(target) = self.entries.iter().find(|entry| entry.id == entry_id).cloned() else {
            return Ok(None);
        };
        if target.message()? != Some(Message::Assistant(assistant.clone())) {
            return Err(failed_recovery_error("durable discard requires the exact assistant receipt"));
        }
        self.discard_admitted_assistant_durably(expected_leaf, entry_id, target, promotion_model)
    }

    // Only branch mechanics are shared. Each caller admits its original receipt.
    fn discard_admitted_assistant_durably(
        &mut self,
        expected_leaf: Option<&str>,
        entry_id: &str,
        target: Entry,
        promotion_model: Option<&str>,
    ) -> Result<Option<String>> {
        if self.leaf_id() != expected_leaf {
            return Err(failed_recovery_error("durable discard branch changed"));
        }
        let branch = if self.leaf.is_none() {
            Vec::new()
        } else {
            self.strict_compaction_branch().map_err(|error| failed_recovery_error(error.to_string()))?
        };
        if let Some(index) = branch.iter().position(|entry| entry.id == entry_id) {
            if branch[index + 1..].iter().any(|entry| !entry.is_service_tier_change()) {
                return Err(failed_recovery_error("durable discard would bypass content on the current branch"));
            }
        } else if self.leaf != target.parent_id {
            return Err(failed_recovery_error("durable discard has no matching selected parent"));
        }
        let child_ids = self
            .entries
            .iter()
            .filter(|entry| entry.parent_id.as_deref() == Some(entry_id))
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        let reparent_children = child_ids
            .iter()
            .all(|id| self.entries.iter().find(|entry| &entry.id == id).is_some_and(Entry::is_service_tier_change));
        let previous_entries = self.entries.clone();
        let previous_ids = self.ids.clone();
        let previous_leaf = self.leaf.clone();
        let mut parent_id = target.parent_id;
        if reparent_children {
            for child_id in child_ids {
                let child = self.entries.iter_mut().find(|entry| entry.id == child_id).expect("captured child");
                child.parent_id = parent_id.clone();
                child.raw["parentId"] = parent_id.clone().map(Value::String).unwrap_or(Value::Null);
                parent_id = Some(child_id);
            }
            self.entries.retain(|entry| entry.id != entry_id);
            self.ids.remove(entry_id);
        }
        let marker_id = generate_id(&self.ids);
        let raw = json!({"type":"branch_summary", "id":marker_id, "parentId":parent_id,
            "timestamp":now_iso(), "fromId":parent_id.as_deref().unwrap_or("root"), "summary":"",
            "details":{"kind":DISCARDED_ENTRY_BRANCH_MARKER,"discardedEntryId":entry_id}});
        self.entries.push(Entry {
            id: marker_id.clone(),
            parent_id,
            kind: "branch_summary".into(),
            raw,
            native_message: None,
            snapcompact_source: None,
        });
        self.ids.insert(marker_id.clone());
        self.leaf = Some(marker_id.clone());
        if let Some(model) = promotion_model {
            let id = generate_id(&self.ids);
            let raw = json!({"type":"model_change", "id":id, "parentId":marker_id,
                "timestamp":now_iso(), "model":model,
                "role":EPHEMERAL_MODEL_CHANGE_ROLE,"resolvedModelIsFallback":false});
            self.entries.push(Entry {
                id: id.clone(),
                parent_id: Some(marker_id),
                kind: "model_change".into(),
                raw,
                native_message: None,
                snapcompact_source: None,
            });
            self.ids.insert(id.clone());
            self.leaf = Some(id);
        }
        if let Err(error) = self.rewrite() {
            self.entries = previous_entries;
            self.ids = previous_ids;
            self.leaf = previous_leaf;
            return Err(error);
        }
        Ok(self.leaf.clone())
    }

    /// Fixed turn-recovery.ts:1061-1093. Preserve the original raw receipts,
    /// bypassing only this empty Stop and its directly preceding custom prompt.
    pub fn discard_accepted_terminal_empty_stop(
        &mut self,
        expected_leaf: Option<&str>,
        entry_id: &str,
        assistant: &AssistantMessage,
    ) -> Result<Option<String>> {
        if self.leaf_id() != expected_leaf || !is_empty_assistant_stop(assistant) {
            return Err(failed_recovery_error("accepted empty-stop branch or assistant changed"));
        }
        let branch = self.strict_compaction_branch().map_err(|error| failed_recovery_error(error.to_string()))?;
        let index = branch
            .iter()
            .position(|entry| entry.id == entry_id)
            .ok_or_else(|| failed_recovery_error("accepted empty Stop is outside the current branch"))?;
        let target = branch[index];
        if target.message()? != Some(Message::Assistant(assistant.clone()))
            || branch[index + 1..].iter().any(|entry| !entry.is_service_tier_change())
        {
            return Err(failed_recovery_error("accepted empty-stop receipt or metadata tail changed"));
        }
        let parent = target.parent_id.as_deref().and_then(|id| branch.iter().find(|entry| entry.id == id));
        let target_parent = match parent {
            Some(parent) if parent.kind == "custom_message" => parent.parent_id.clone(),
            _ => target.parent_id.clone(),
        };
        let previous_leaf = self.leaf.clone();
        self.leaf = target_parent;
        let mut fields = serde_json::Map::new();
        fields.insert("customType".into(), json!(ACCEPTED_TERMINAL_EMPTY_STOP_MARKER));
        let result = self.append_raw("custom", fields).map(Some);
        if result.is_err() {
            self.leaf = previous_leaf;
        }
        result
    }

    /// Native metadata receipt, retained verbatim and excluded from messages.
    pub fn append_service_tier_change(&mut self, service_tier: &Value) -> Result<String> {
        if !service_tier.is_null() && !service_tier.is_object() {
            return Err(failed_recovery_error("service tier must be a native family mapping or null"));
        }
        let mut fields = serde_json::Map::new();
        fields.insert("serviceTier".into(), service_tier.clone());
        self.append_raw("service_tier_change", fields)
    }

    /// Fixed turn-recovery marks only the recorded failed entries on this
    /// branch. Pending retries do not invent a new persistent entry/state.
    /// Preserve every original receipt and atomically rewrite its metadata.
    pub fn update_retry_recovery(&mut self, updates: &[(String, ara_ai::AssistantRetryRecovery)]) -> Result<()> {
        let branch_ids: HashSet<String> = self.branch().iter().map(|entry| entry.id.clone()).collect();
        let mut seen = HashSet::new();
        for (id, recovery) in updates {
            if !seen.insert(id) || !branch_ids.contains(id) {
                return Err(SessionError::Corrupt {
                    path: self.path.clone(),
                    message: format!("retry recovery entry is repeated or outside the active branch: {id}"),
                });
            }
            let entry = self.entries.iter().find(|entry| &entry.id == id).expect("branch entry exists");
            if !matches!(entry.message()?,  Some(Message::Assistant(message))
                if matches!(message.stop_reason, ara_ai::StopReason::Error | ara_ai::StopReason::Aborted))
                || recovery.kind != "auto-retry"
                || !matches!(recovery.status.as_str(), "recovered" | "superseded")
            {
                return Err(SessionError::Corrupt {
                    path: self.path.clone(),
                    message: format!("retry recovery requires an actual failed assistant receipt: {id}"),
                });
            }
        }
        if updates.is_empty() {
            return Ok(());
        }
        let previous = self.entries.clone();
        let mut candidate = previous.clone();
        for (id, recovery) in updates {
            let entry = candidate.iter_mut().find(|entry| &entry.id == id).expect("validated entry");
            let before = entry.checked_raw()?;
            let mut after = before.clone();
            after["message"]["retryRecovery"] = serde_json::to_value(recovery).expect("retry recovery serializes");
            if let Some(source) = &entry.native_message {
                entry.native_message = Some(source.checked_edit(&before["message"], &after["message"])?);
            }
            entry.raw = after;
        }
        self.entries = candidate;
        if let Err(error) = self.rewrite() {
            self.entries = previous;
            self.rewrite_required = true;
            return Err(error);
        }
        Ok(())
    }

    pub fn append_bash_execution(&mut self, message: &BashExecutionMessage) -> Result<String> {
        let mut fields = serde_json::Map::new();
        fields.insert("message".into(), message.event_message());
        self.append_raw("message", fields)
    }

    /// Fixed appendMessageToBranch records a child of the retained owner and
    /// restores the active leaf. Existing append rollback retains native IDs
    /// and entries on failure; restoration also happens on that error path.
    pub fn append_bash_execution_to_branch(
        &mut self,
        message: &BashExecutionMessage,
        parent_id: Option<&str>,
    ) -> Result<String> {
        if let Some(parent_id) = parent_id
            && !self.ids.contains(parent_id)
        {
            return Err(
                std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("Entry {parent_id} not found")).into()
            );
        }
        let active_leaf = self.leaf.clone();
        self.leaf = parent_id.map(str::to_owned);
        let recorded = self.append_bash_execution(message);
        self.leaf = active_leaf;
        recorded
    }

    /// Persist one user-invoked Skill as its original custom message. The
    /// initiating timestamp is retained so the reopened model view is exact.
    pub fn append_skill_prompt(&mut self, prompt: &UserSkillPrompt) -> Result<String> {
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(prompt.timestamp)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid Skill prompt timestamp"))?
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        let mut f = serde_json::Map::new();
        f.insert("customType".into(), json!(SKILL_PROMPT_CUSTOM_TYPE));
        f.insert("content".into(), json!(prompt.content));
        f.insert("display".into(), json!(true));
        f.insert("attribution".into(), json!("user"));
        f.insert("timestamp".into(), json!(timestamp));
        if let Some(details) = &prompt.details {
            f.insert("details".into(), details.clone());
        }
        self.append_raw("custom_message", f)
    }

    /// One fixed agent-attributed hidden reminder per accepted interruption.
    /// Persist the custom identity; providers consume its Developer projection.
    pub fn append_loop_guard_notice(&mut self, notice: &LoopGuardNotice) -> Result<String> {
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(notice.timestamp())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid loop guard notice timestamp")
            })?
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        let mut fields = notice.persistence_event_message().as_object().expect("notice is an object").clone();
        fields.remove("role");
        fields.insert("timestamp".into(), json!(timestamp));
        self.append_raw("custom_message", fields)
    }

    /// Discard only the captured aborted assistant from the active branch,
    /// retaining its raw receipt. The new notice's parent bypasses that leaf;
    /// restart therefore sees the same model context and Session identity.
    pub fn append_gemini_reminder_after_aborted(
        &mut self,
        notice: &LoopGuardNotice,
        aborted_entry_id: &str,
    ) -> Result<String> {
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Gemini reminder requires the exact aborted thinking-only assistant leaf",
            )
        };
        if notice.custom_type() != GEMINI_TOOL_CALL_REMINDER_TYPE || self.leaf_id() != Some(aborted_entry_id) {
            return Err(invalid().into());
        }
        let target = self.entries.iter().find(|entry| entry.id == aborted_entry_id).ok_or_else(invalid)?;
        let Some(Message::Assistant(message)) = target.message()? else {
            return Err(invalid().into());
        };
        if message.stop_reason != ara_ai::StopReason::Aborted
            || message.content.iter().any(|block| !matches!(block, AssistantBlock::Thinking(_)))
        {
            return Err(invalid().into());
        }
        let previous_leaf = self.leaf.clone();
        self.leaf = target.parent_id.clone();
        let result = self.append_loop_guard_notice(notice);
        if result.is_err() {
            self.leaf = previous_leaf;
        }
        result
    }

    /// OMP `appendModelChange`: `model` in `provider/modelId` form.
    pub fn append_model_change(&mut self, model: &str) -> Result<String> {
        self.append_model_change_with_role(model, None, false)
    }

    /// Fixed `session-manager.ts:2377-2387`. A missing role means default;
    /// the fallback flag is historical attribution, not an ephemeral selector.
    pub fn append_model_change_with_role(
        &mut self,
        model: &str,
        role: Option<&str>,
        resolved_model_is_fallback: bool,
    ) -> Result<String> {
        self.append_raw("model_change", Self::model_change_fields(model, role, resolved_model_is_fallback))
    }

    fn model_change_fields(
        model: &str,
        role: Option<&str>,
        resolved_model_is_fallback: bool,
    ) -> serde_json::Map<String, Value> {
        let mut f = serde_json::Map::new();
        f.insert("model".into(), json!(model));
        if let Some(role) = role {
            f.insert("role".into(), json!(role));
        }
        f.insert("resolvedModelIsFallback".into(), json!(resolved_model_is_fallback));
        f
    }

    /// Fixed `session-manager.ts:2348-2358`: persist both the resolved level
    /// and configured selector (including auto), with absent values as null.
    pub fn append_thinking_level_change(
        &mut self,
        thinking_level: Option<&str>,
        configured: Option<&str>,
    ) -> Result<String> {
        self.append_raw("thinking_level_change", Self::thinking_level_change_fields(thinking_level, configured))
    }

    fn thinking_level_change_fields(
        thinking_level: Option<&str>,
        configured: Option<&str>,
    ) -> serde_json::Map<String, Value> {
        let mut f = serde_json::Map::new();
        f.insert("thinkingLevel".into(), json!(thinking_level));
        f.insert("configured".into(), json!(configured));
        f
    }

    /// Commit the native model and thinking receipts as one atomic pair.
    /// Entry shapes follow fixed `session-manager.ts:2348-2358,2377-2387`;
    /// the single durable publication is ARA's existing rewrite adaptation.
    ///
    /// The model entry follows the old leaf, and thinking follows model. Lazy
    /// and in-memory journals retain their ordinary no-file behavior. A failed
    /// write before publication restores entries, IDs and leaf. If rename has
    /// already published both entries but directory sync fails, the matching
    /// journal view is retained and the typed error requires Host reconciliation.
    /// Cancellation and candidate adoption remain the Host's responsibility.
    pub fn append_model_and_thinking_change(
        &mut self,
        model: &str,
        role: Option<&str>,
        resolved_model_is_fallback: bool,
        thinking_level: Option<&str>,
        configured: Option<&str>,
    ) -> std::result::Result<(String, String), ModelAndThinkingChangeError> {
        self.append_model_and_thinking_change_with_sync(
            model,
            role,
            resolved_model_is_fallback,
            thinking_level,
            configured,
            sync_dir,
        )
    }

    fn append_model_and_thinking_change_with_sync(
        &mut self,
        model: &str,
        role: Option<&str>,
        resolved_model_is_fallback: bool,
        thinking_level: Option<&str>,
        configured: Option<&str>,
        directory_sync: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> std::result::Result<(String, String), ModelAndThinkingChangeError> {
        self.append_model_and_optional_thinking_change_with_sync(
            model,
            role,
            resolved_model_is_fallback,
            Some((thinking_level, configured)),
            directory_sync,
        )
        .map(|(model, thinking)| (model, thinking.expect("mandatory thinking entry was staged")))
        .map_err(|error| match error {
            ModelAndOptionalThinkingChangeError::Storage(source) => ModelAndThinkingChangeError::Storage(source),
            ModelAndOptionalThinkingChangeError::PublishedButDurabilityUnknown {
                model_entry_id,
                thinking_entry_id,
                source,
            } => ModelAndThinkingChangeError::PublishedButDurabilityUnknown {
                model_entry_id,
                thinking_entry_id: thinking_entry_id.expect("mandatory thinking entry was published"),
                source,
            },
        })
    }

    /// Atomically record a model change with a thinking change only when the
    /// Host supplies one. Fixed `model-controls.ts:542,547-550` records thinking
    /// when leaving auto or changing the effective level; that decision remains
    /// with the Host. `None` stages no thinking receipt, whereas
    /// `Some((None, None))` explicitly records both native null fields.
    ///
    /// The return/error IDs name exactly the entries published by the shared
    /// transaction. Lazy materialization and publication-phase semantics match
    /// [`append_model_and_thinking_change`](Self::append_model_and_thinking_change).
    pub fn append_model_and_optional_thinking_change(
        &mut self,
        model: &str,
        role: Option<&str>,
        resolved_model_is_fallback: bool,
        thinking_change: Option<(Option<&str>, Option<&str>)>,
    ) -> std::result::Result<(String, Option<String>), ModelAndOptionalThinkingChangeError> {
        self.append_model_and_optional_thinking_change_with_sync(
            model,
            role,
            resolved_model_is_fallback,
            thinking_change,
            sync_dir,
        )
    }

    fn append_model_and_optional_thinking_change_with_sync(
        &mut self,
        model: &str,
        role: Option<&str>,
        resolved_model_is_fallback: bool,
        thinking_change: Option<(Option<&str>, Option<&str>)>,
        directory_sync: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> std::result::Result<(String, Option<String>), ModelAndOptionalThinkingChangeError> {
        let previous_len = self.entries.len();
        let previous_leaf = self.leaf.clone();
        let previous_materialized = self.materialized;
        let previous_rewrite_required = self.rewrite_required;
        let model_entry_id =
            self.stage_raw("model_change", Self::model_change_fields(model, role, resolved_model_is_fallback));
        let thinking_entry_id = thinking_change.map(|(level, configured)| {
            self.stage_raw("thinking_level_change", Self::thinking_level_change_fields(level, configured))
        });
        if !self.persistent || (!self.materialized && !self.has_assistant()?) {
            return Ok((model_entry_id, thinking_entry_id));
        }
        match self.rewrite_with_directory_sync(directory_sync) {
            Ok(()) => Ok((model_entry_id, thinking_entry_id)),
            Err(RewritePhaseError::BeforePublication(source)) => {
                self.entries.truncate(previous_len);
                self.ids.remove(&model_entry_id);
                if let Some(thinking_entry_id) = &thinking_entry_id {
                    self.ids.remove(thinking_entry_id);
                }
                self.leaf = previous_leaf;
                self.materialized = previous_materialized;
                self.rewrite_required = previous_rewrite_required;
                Err(ModelAndOptionalThinkingChangeError::Storage(source))
            }
            Err(RewritePhaseError::PublishedButDurabilityUnknown(source)) => {
                self.materialized = true;
                self.rewrite_required = false;
                Err(ModelAndOptionalThinkingChangeError::PublishedButDurabilityUnknown {
                    model_entry_id,
                    thinking_entry_id,
                    source,
                })
            }
        }
    }

    /// Force the file to exist even without an assistant message.
    pub fn materialize(&mut self) -> Result<()> {
        self.rewrite()
    }

    /// Entries on the branch from the root to the leaf. The walk stops at a
    /// missing parent (e.g. a dropped malformed record), like OMP `pathTo`.
    pub fn branch(&self) -> Vec<&Entry> {
        self.path_to(self.leaf.as_deref())
    }

    fn path_to(&self, leaf: Option<&str>) -> Vec<&Entry> {
        let by_id: HashMap<&str, &Entry> = self.entries.iter().map(|e| (e.id.as_str(), e)).collect();
        let mut out = Vec::new();
        let mut cur = leaf;
        let mut seen = HashSet::new();
        while let Some(id) = cur {
            if !seen.insert(id) {
                break;
            }
            let Some(e) = by_id.get(id) else { break };
            out.push(*e);
            cur = e.parent_id.as_deref();
        }
        out.reverse();
        out
    }

    fn strict_compaction_branch(&self) -> std::result::Result<Vec<&Entry>, CompactionSourceError> {
        if (self.persistent && !self.materialized) || self.leaf.is_none() {
            return Err(CompactionSourceError::NotDurable);
        }
        self.strict_raw_branch()
    }

    fn strict_raw_branch(&self) -> std::result::Result<Vec<&Entry>, CompactionSourceError> {
        if self.rewrite_required {
            return Err(CompactionSourceError::UnrepairedJournal);
        }
        if self.loaded_invalid_utf8 {
            return Err(CompactionSourceError::InvalidUtf8);
        }
        let mut by_id: HashMap<&str, (usize, &Entry)> = HashMap::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.id.is_empty() {
                return Err(CompactionSourceError::EmptyEntryId);
            }
            if by_id.insert(&entry.id, (index, entry)).is_some() {
                return Err(CompactionSourceError::DuplicateEntryId { id: entry.id.clone() });
            }
        }
        let mut current = self.leaf.as_deref();
        let mut seen = HashSet::new();
        let mut branch = Vec::new();
        while let Some(id) = current {
            if !seen.insert(id) {
                return Err(CompactionSourceError::ParentCycle { id: id.to_owned() });
            }
            let (index, entry) =
                by_id.get(id).copied().ok_or_else(|| CompactionSourceError::MissingEntry { id: id.to_owned() })?;
            let valid_parent = match (entry.raw.get("parentId"), entry.parent_id.as_deref()) {
                (Some(Value::Null), None) => true,
                (Some(Value::String(raw)), Some(parent)) => !raw.is_empty() && raw == parent,
                _ => false,
            };
            if !valid_parent {
                return Err(CompactionSourceError::InvalidParentId { id: entry.id.clone() });
            }
            if let Some(parent_id) = entry.parent_id.as_deref() {
                if seen.contains(parent_id) {
                    return Err(CompactionSourceError::ParentCycle { id: parent_id.to_owned() });
                }
                let (parent_index, _) = by_id
                    .get(parent_id)
                    .copied()
                    .ok_or_else(|| CompactionSourceError::MissingEntry { id: parent_id.to_owned() })?;
                if parent_index >= index {
                    return Err(CompactionSourceError::ParentNotEarlier {
                        child_id: entry.id.clone(),
                        parent_id: parent_id.to_owned(),
                    });
                }
            }
            branch.push(entry);
            current = entry.parent_id.as_deref();
        }
        branch.reverse();
        Ok(branch)
    }

    /// Provenance-preserving, read-only source snapshot for compaction. Unlike
    /// `branch()`, this rejects ambiguous or incomplete history. It skips only
    /// known non-context anchors; unsupported context entries need their own
    /// projection before compaction can safely use this branch.
    pub fn compaction_source_snapshot(&self) -> std::result::Result<CompactionSourceSnapshot, CompactionSourceError> {
        if !self.materialized {
            return Err(CompactionSourceError::NotDurable);
        }
        let branch = self.strict_compaction_branch()?;
        let mut messages = Vec::new();
        for entry in &branch[active_context_start(&branch)..] {
            if entry.kind == "compaction" {
                return Err(CompactionSourceError::UnsupportedContextEntry {
                    id: entry.id.clone(),
                    kind: entry.kind.clone(),
                });
            }
            let group = entry.native_compaction_entry()?;
            if group.messages.len() > 1 {
                return Err(CompactionSourceError::MultipleMessageProjection { id: entry.id.clone() });
            }
            messages.extend(
                group.messages.into_iter().map(|message| SourcedMessage { entry_id: entry.id.clone(), message }),
            );
        }
        Ok(CompactionSourceSnapshot {
            session_id: self.session_id().to_owned(),
            leaf_id: self.leaf.as_ref().expect("validated leaf").clone(),
            messages,
        })
    }

    /// Strict projected sources for a further soft summary. The projection
    /// validates the complete raw parent chain and every active compaction.
    /// Summaries cleared by the latest reset remain raw but are not reused.
    /// Native in-memory journals need no file materialization.
    pub fn projected_compaction_snapshot(
        &self,
    ) -> std::result::Result<ProjectedCompactionSnapshot, CompactionProjectionError> {
        let projection = self.compacted_context_projection()?;
        let mut previous_summary = None;
        let mut messages = Vec::new();
        let mut source_ids = HashSet::new();
        for item in projection.items {
            match item {
                CompactedContextItem::Summary(summary) => previous_summary = Some(*summary),
                CompactedContextItem::Message(message) => {
                    if !source_ids.insert(message.entry_id.clone()) {
                        return Err(
                            CompactionSourceError::MultipleMessageProjection { id: message.entry_id.clone() }.into()
                        );
                    }
                    messages.push(*message);
                }
            }
        }
        Ok(ProjectedCompactionSnapshot {
            session_id: projection.session_id,
            leaf_id: projection.leaf_id,
            previous_summary,
            messages,
        })
    }

    /// Native raw groups on the retained active branch. Metadata and prior
    /// compaction barriers stay present even though they project no messages.
    pub fn native_projected_compaction_snapshot(
        &self,
    ) -> std::result::Result<NativeProjectedCompactionSnapshot, CompactionProjectionError> {
        self.native_projected_compaction_snapshot_with_route(None)
    }

    /// Use exactly the active route's native replay availability for both
    /// preparation and model context. An unreadable remote boundary is skipped
    /// in favor of an earlier readable text summary or its original raw inputs.
    pub fn native_projected_compaction_snapshot_for_route(
        &self,
        active_model: &ara_ai::Model,
        native_replay_available: bool,
    ) -> std::result::Result<NativeProjectedCompactionSnapshot, CompactionProjectionError> {
        self.native_projected_compaction_snapshot_with_route(Some((active_model, native_replay_available)))
    }

    fn native_projected_compaction_snapshot_with_route(
        &self,
        route: Option<(&ara_ai::Model, bool)>,
    ) -> std::result::Result<NativeProjectedCompactionSnapshot, CompactionProjectionError> {
        let projection = self.compacted_context_projection_with_route(route)?;
        let previous_summary = projection.items.into_iter().find_map(|item| match item {
            CompactedContextItem::Summary(summary) => Some(*summary),
            _ => None,
        });
        let raw_branch = self.strict_compaction_branch()?;
        let branch = &raw_branch[active_context_start(&raw_branch)..];
        let start = previous_summary.as_ref().map_or(0, |summary| {
            if summary.remote.is_some() {
                return branch.iter().position(|entry| entry.id == summary.entry_id).expect("validated remote ID") + 1;
            }
            branch
                .iter()
                .position(|entry| entry.id == summary.first_kept_entry_id)
                .expect("projection validated raw kept ID")
        });
        let entries = branch[start..]
            .iter()
            .map(|entry| entry.native_compaction_entry())
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(NativeProjectedCompactionSnapshot {
            session_id: self.session_id().to_owned(),
            leaf_id: self.leaf.as_ref().expect("validated leaf").clone(),
            previous_summary,
            entries,
        })
    }

    /// Accept one summary derived from the exact current raw groups. A kept
    /// metadata ID is preserved; source IDs count each discarded context-
    /// bearing raw entry once, independently of fragment count.
    pub fn commit_native_entry_compaction(
        &mut self,
        snapshot: &NativeProjectedCompactionSnapshot,
        summary: &str,
        first_kept_entry_id: &str,
        window_source_entry_ids: &[String],
        tokens_before: u64,
    ) -> std::result::Result<String, CompactionCommitError> {
        let cumulative =
            self.native_entry_compaction_sources(snapshot, summary, first_kept_entry_id, window_source_entry_ids)?;
        let preserve_source = snapshot
            .previous_summary
            .as_ref()
            .map(|previous| previous.preserve_data_without_archive_utf16())
            .transpose()
            .map_err(SessionError::from)?
            .flatten()
            .map(|raw| NativeSnapcompactArchiveSource::from_raw(&raw))
            .transpose()
            .map_err(SessionError::from)?;
        let preserve_source = preserve_source.and_then(|source| {
            let mut state = native_message::clone_wire(source.observe());
            let ara_rpc::WireValue::Object(fields) = &mut state else {
                return None;
            };
            fields.retain(|(key, _)| !key.equals_ascii("openaiRemoteCompaction"));
            (!fields.is_empty()).then(|| NativeSnapcompactArchiveSource::from_wire(state))
        });
        let preserve = preserve_source.as_ref().and_then(|source| source.checked_preserve().ok());
        let archive_source = snapshot
            .previous_summary
            .as_ref()
            .and_then(|previous| previous.archive_image_boundary.as_ref())
            .map(|boundary| boundary.entry_id.as_str());
        Ok(self.append_compaction_with_preserve(
            summary,
            first_kept_entry_id,
            &cumulative,
            tokens_before,
            preserve.as_ref(),
            archive_source,
            preserve_source,
        )?)
    }

    /// Shared source/cut validation only. Publication stays method-specific:
    /// soft retains its existing append API; handoff publishes atomically.
    fn native_entry_compaction_sources(
        &self,
        snapshot: &NativeProjectedCompactionSnapshot,
        summary: &str,
        first_kept_entry_id: &str,
        window_source_entry_ids: &[String],
    ) -> std::result::Result<Vec<String>, CompactionCommitError> {
        let current = self.native_projected_compaction_snapshot()?;
        self.native_entry_compaction_sources_from_current(
            snapshot,
            &current,
            summary,
            first_kept_entry_id,
            window_source_entry_ids,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn native_entry_compaction_sources_from_current(
        &self,
        snapshot: &NativeProjectedCompactionSnapshot,
        current: &NativeProjectedCompactionSnapshot,
        summary: &str,
        first_kept_entry_id: &str,
        window_source_entry_ids: &[String],
        native_replay: bool,
    ) -> std::result::Result<Vec<String>, CompactionCommitError> {
        if snapshot != current {
            return Err(CompactionCommitError::StaleSnapshot);
        }
        if summary.trim().is_empty() || summary.len() > 1_000_000 {
            return Err(CompactionCommitError::InvalidSummary);
        }
        let kept = current
            .entries
            .iter()
            .position(|entry| entry.entry_id == first_kept_entry_id)
            .filter(|index| *index > 0)
            .ok_or(CompactionCommitError::InvalidWindow)?;
        let expected: Vec<String> = current.entries[..kept]
            .iter()
            .filter(|entry| !entry.messages.is_empty())
            .map(|entry| entry.entry_id.clone())
            .collect();
        if expected.is_empty() || window_source_entry_ids != expected.as_slice() {
            return Err(CompactionCommitError::InvalidWindow);
        }
        if current.previous_summary.as_ref().is_some_and(|summary| summary.source_entry_ids.is_none()) {
            return Err(CompactionCommitError::MissingPreviousSources);
        }
        let raw_branch = self.strict_compaction_branch().map_err(CompactionProjectionError::from)?;
        let branch = &raw_branch[active_context_start(&raw_branch)..];
        // A remote payload also covers its previous retained tail. Its original
        // sourceEntryIds retain their replaced-prefix meaning; the next summary
        // carries every now-consumed original ID forward without inventing raw
        // observations from the opaque placeholder.
        let raw_start = branch
            .iter()
            .position(|entry| entry.id == current.entries[0].entry_id)
            .ok_or(CompactionCommitError::InvalidWindow)?;
        let mut cumulative = context_source_ids(&branch[..raw_start])?;
        cumulative.extend(expected);
        let raw_kept = branch
            .iter()
            .position(|entry| entry.id == first_kept_entry_id)
            .ok_or(CompactionCommitError::InvalidWindow)?;
        // New native cuts backtrack to the first adjacent non-message.
        // Readers also accept older V1 summaries keeping User after title
        // metadata, so already accepted persisted history stays usable.
        let backtracked = raw_kept
            .checked_sub(1)
            .is_some_and(|previous| matches!(branch[previous].kind.as_str(), "message" | "compaction"));
        if !backtracked
            || !native_kept_boundary_supported(branch, raw_kept)
            || cumulative != context_source_ids(&branch[..raw_kept])?
        {
            return Err(CompactionCommitError::InvalidWindow);
        }
        let split = current.entries[kept].origin != NativeEntryOrigin::User;
        if !safe_compaction_prefix_with_archive(
            &branch[..raw_kept],
            split,
            native_replay,
            current.previous_summary.as_ref(),
        ) {
            return Err(CompactionCommitError::UnsafeSummaryBoundary);
        }
        Ok(cumulative)
    }

    /// Validate this exact projected window, then append one native soft
    /// compaction. Carry prior raw source IDs forward so readers can verify
    /// the cumulative replaced prefix without a new record or leaf format.
    pub fn commit_projected_compaction(
        &mut self,
        snapshot: &ProjectedCompactionSnapshot,
        summary: &str,
        first_kept_entry_id: &str,
        window_source_entry_ids: &[String],
        tokens_before: u64,
    ) -> std::result::Result<String, CompactionCommitError> {
        self.commit_projected_compaction_boundary(
            snapshot,
            summary,
            first_kept_entry_id,
            window_source_entry_ids,
            tokens_before,
            false,
        )
    }

    /// Commit a fixed OMP message cut, including a split turn before an
    /// Assistant. The persisted native entry needs no invented split flag:
    /// the actual kept message role determines the checked replay boundary.
    pub fn commit_native_projected_compaction(
        &mut self,
        snapshot: &ProjectedCompactionSnapshot,
        summary: &str,
        first_kept_entry_id: &str,
        window_source_entry_ids: &[String],
        tokens_before: u64,
    ) -> std::result::Result<String, CompactionCommitError> {
        self.commit_projected_compaction_boundary(
            snapshot,
            summary,
            first_kept_entry_id,
            window_source_entry_ids,
            tokens_before,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_projected_compaction_boundary(
        &mut self,
        snapshot: &ProjectedCompactionSnapshot,
        summary: &str,
        first_kept_entry_id: &str,
        window_source_entry_ids: &[String],
        tokens_before: u64,
        allow_assistant: bool,
    ) -> std::result::Result<String, CompactionCommitError> {
        let current = self.projected_compaction_snapshot()?;
        if snapshot != &current {
            return Err(CompactionCommitError::StaleSnapshot);
        }
        if summary.trim().is_empty() || summary.len() > 1_000_000 {
            return Err(CompactionCommitError::InvalidSummary);
        }
        let kept_index = current
            .messages
            .iter()
            .position(|message| message.entry_id == first_kept_entry_id)
            .filter(|index| {
                *index > 0
                    && match &current.messages[*index].message {
                        Message::User(_) => true,
                        Message::Assistant(_) => allow_assistant,
                        _ => false,
                    }
            })
            .ok_or(CompactionCommitError::InvalidWindow)?;
        let expected_window: Vec<String> =
            current.messages[..kept_index].iter().map(|message| message.entry_id.clone()).collect();
        if window_source_entry_ids != expected_window.as_slice() {
            return Err(CompactionCommitError::InvalidWindow);
        }
        let mut cumulative_sources = match &current.previous_summary {
            Some(previous) => previous.source_entry_ids.clone().ok_or(CompactionCommitError::MissingPreviousSources)?,
            None => Vec::new(),
        };
        cumulative_sources.extend(expected_window);
        let raw_branch = self.strict_compaction_branch().map_err(CompactionProjectionError::from)?;
        let branch = &raw_branch[active_context_start(&raw_branch)..];
        let raw_kept_index = branch
            .iter()
            .position(|entry| entry.id == first_kept_entry_id)
            .ok_or(CompactionCommitError::InvalidWindow)?;
        let raw_sources = context_source_ids(&branch[..raw_kept_index])?;
        if cumulative_sources != raw_sources {
            return Err(CompactionCommitError::InvalidWindow);
        }
        let split = matches!(&current.messages[kept_index].message, Message::Assistant(_));
        if !safe_summary_prefix(&branch[..raw_kept_index], split) {
            return Err(CompactionCommitError::UnsafeSummaryBoundary);
        }
        Ok(self.append_compaction(summary, first_kept_entry_id, &cumulative_sources, tokens_before)?)
    }

    /// Strict, read-only projection after the latest readable text compaction.
    /// Opaque remote boundaries are skipped without a native active route.
    /// Raw entries remain in the journal; the latest summary is a
    /// distinct item followed by kept and later messages. This does not turn
    /// the summary into a user message or make a provider request.
    pub fn compacted_context_projection(
        &self,
    ) -> std::result::Result<CompactedContextProjection, CompactionProjectionError> {
        self.compacted_context_projection_with_route(None)
    }

    pub fn compacted_context_projection_for_route(
        &self,
        active_model: &ara_ai::Model,
        native_replay_available: bool,
    ) -> std::result::Result<CompactedContextProjection, CompactionProjectionError> {
        self.compacted_context_projection_with_route(Some((active_model, native_replay_available)))
    }

    fn compacted_context_projection_with_route(
        &self,
        route: Option<(&ara_ai::Model, bool)>,
    ) -> std::result::Result<CompactedContextProjection, CompactionProjectionError> {
        let raw_branch = self.strict_compaction_branch()?;
        let branch = &raw_branch[active_context_start(&raw_branch)..];
        let mut latest: Option<(usize, CompactionSummaryView)> = None;
        for (index, entry) in branch.iter().enumerate() {
            match entry.kind.as_str() {
                "compaction" => {
                    let invalid = |field| CompactionProjectionError::InvalidField { id: entry.id.clone(), field };
                    let summary = entry.raw.get("summary").and_then(Value::as_str).ok_or_else(|| invalid("summary"))?;
                    if summary.trim().is_empty() || summary.len() > 1_000_000 {
                        return Err(invalid("summary"));
                    }
                    let first_kept = entry
                        .raw
                        .get("firstKeptEntryId")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| invalid("firstKeptEntryId"))?;
                    let kept_index = branch[..index]
                        .iter()
                        .position(|candidate| candidate.id == first_kept)
                        .ok_or_else(|| CompactionProjectionError::MissingKeptMessage { id: entry.id.clone() })?;
                    if !native_kept_boundary_supported(&branch[..index], kept_index) {
                        return Err(CompactionProjectionError::UnsafeSummaryBoundary { id: entry.id.clone() });
                    }
                    let allow_unanswered_user =
                        branch[kept_index].native_compaction_entry()?.origin != NativeEntryOrigin::User;
                    let native_replay = entry.raw["method"] == "remote"
                        && entry.raw.get("preserveData").is_some_and(|value| !value.is_null())
                        || entry.has_archive();
                    let soft =
                        matches!(entry.raw.get("method"), None | Some(Value::Null)) || entry.raw["method"] == "soft";
                    let migrated_archive = soft.then(|| latest.as_ref().map(|(_, summary)| summary)).flatten();
                    let migrated_boundary = migrated_archive
                        .filter(|summary| summary.source_entry_ids.is_some())
                        .and_then(|summary| summary.archive_image_boundary.clone());
                    // An imported archive can retain every raw entry without a replaced
                    // prefix. Keep its unknown provenance while validating the archive below.
                    let retained_all_archive =
                        kept_index == 0 && entry.has_archive() && entry.raw.get("sourceEntryIds").is_none();
                    if !retained_all_archive
                        && !safe_compaction_prefix_with_archive(
                            &branch[..kept_index],
                            allow_unanswered_user,
                            native_replay,
                            migrated_archive,
                        )
                    {
                        return Err(CompactionProjectionError::UnsafeSummaryBoundary { id: entry.id.clone() });
                    }
                    let tokens_before =
                        entry.raw.get("tokensBefore").and_then(Value::as_u64).ok_or_else(|| invalid("tokensBefore"))?;
                    let timestamp = entry
                        .raw
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| invalid("timestamp"))?;
                    let method = match entry.raw.get("method").filter(|value| !value.is_null()) {
                        Some(method) => {
                            let method = method.as_str().ok_or_else(|| invalid("method"))?;
                            if !matches!(method, "soft" | "handoff" | "remote" | "snapcompact") {
                                return Err(CompactionProjectionError::UnsupportedMethod {
                                    id: entry.id.clone(),
                                    method: method.to_owned(),
                                });
                            }
                            method
                        }
                        None => "soft",
                    };
                    let file_details = handoff::native_file_details(&entry.raw);
                    let archive = ara_snapcompact::get_preserved_archive(entry.raw.get("preserveData"));
                    let preserve_source = entry.archive_source();
                    let exact_archive = preserve_source
                        .as_ref()
                        .map(|source| source.exact_archive())
                        .transpose()
                        .map_err(|_| invalid("preserveData.snapcompact"))?
                        .flatten();
                    let has_archive = exact_archive.is_some();
                    let remote_preserve = preserve_source
                        .as_ref()
                        .is_some_and(|source| source.observe().get("openaiRemoteCompaction").is_some());
                    if has_archive && remote_preserve {
                        return Err(CompactionProjectionError::UnsupportedReplayData { id: entry.id.clone() });
                    }
                    let short_summary = match matches!(method, "remote" | "snapcompact")
                        .then(|| entry.raw.get("shortSummary"))
                        .flatten()
                        .filter(|value| !value.is_null())
                    {
                        Some(value) => Some(value.as_str().ok_or_else(|| invalid("shortSummary"))?.to_owned()),
                        None => None,
                    };
                    if method == "handoff" {
                        if entry_timestamp(&entry.raw).is_none() {
                            return Err(invalid("timestamp"));
                        }
                        if file_details.is_none() {
                            return Err(invalid("details"));
                        }
                        // Handoff is a text document, not provider replay or
                        // an extension/native image archive admission path.
                        if preserve_source.is_some()
                            || ["preserveData", "shortSummary", "providerPayload", "blocks", "images"]
                                .iter()
                                .any(|field| entry.raw.get(*field).is_some_and(|value| !value.is_null()))
                            || !matches!(entry.raw.get("fromExtension"), None | Some(Value::Null | Value::Bool(false)))
                        {
                            return Err(CompactionProjectionError::UnsupportedReplayData { id: entry.id.clone() });
                        }
                    }
                    if method != "remote"
                        && (entry.raw.get("providerReplayThroughEntryId").is_some_and(|value| !value.is_null())
                            || entry.raw.get("fromExtension").and_then(Value::as_bool) == Some(true)
                            || !has_archive
                                && preserve_source.as_ref().is_some_and(|source| match source.observe() {
                                    ara_rpc::WireValue::Null => false,
                                    ara_rpc::WireValue::Object(_) => {
                                        source.observe().get("snapcompact").is_some() || remote_preserve
                                    }
                                    _ => true,
                                }))
                    {
                        return Err(CompactionProjectionError::UnsupportedReplayData { id: entry.id.clone() });
                    }
                    if method == "snapcompact" && !has_archive {
                        return Err(invalid("preserveData.snapcompact"));
                    }
                    let source_entry_ids = match entry.raw.get("sourceEntryIds") {
                        None if !has_archive
                            && (allow_unanswered_user || matches!(method, "handoff" | "remote" | "snapcompact")) =>
                        {
                            return Err(CompactionProjectionError::SourceIdsMismatch { id: entry.id.clone() });
                        }
                        None => None,
                        Some(Value::Array(ids)) => {
                            let ids: Vec<String> = ids
                                .iter()
                                .map(|id| id.as_str().filter(|id| !id.is_empty()).map(str::to_owned))
                                .collect::<Option<_>>()
                                .ok_or_else(|| invalid("sourceEntryIds"))?;
                            let expected = context_source_ids(&branch[..kept_index])?;
                            if ids.is_empty() || ids != expected {
                                return Err(CompactionProjectionError::SourceIdsMismatch { id: entry.id.clone() });
                            }
                            Some(ids)
                        }
                        Some(_) => return Err(invalid("sourceEntryIds")),
                    };
                    if soft && let Some(source) = entry.raw.get("archiveSourceEntryId").filter(|value| !value.is_null())
                    {
                        let source = source.as_str().ok_or_else(|| invalid("archiveSourceEntryId"))?;
                        if migrated_boundary.as_ref().is_none_or(|boundary| boundary.entry_id != source) {
                            return Err(invalid("archiveSourceEntryId"));
                        }
                    }
                    let archive_image_boundary = if has_archive && source_entry_ids.is_some() {
                        Some(ArchiveImageBoundary {
                            entry_id: entry.id.clone(),
                            first_kept_entry_id: first_kept.to_owned(),
                        })
                    } else if soft && source_entry_ids.is_some() {
                        migrated_boundary
                    } else {
                        None
                    };
                    let remote = if method == "remote" {
                        remote::validate_remote_entry(branch, index, kept_index, entry)?
                    } else {
                        None
                    };
                    let start = if let Some(remote) = &remote {
                        if !remote::reusable(remote, route) {
                            continue;
                        }
                        branch[..index]
                            .iter()
                            .position(|entry| entry.id == remote.replay_through_entry_id)
                            .expect("validated replay-through ID")
                            + 1
                    } else {
                        kept_index
                    };
                    latest = Some((
                        start,
                        CompactionSummaryView {
                            entry_id: entry.id.clone(),
                            summary: summary.to_owned(),
                            short_summary,
                            first_kept_entry_id: first_kept.to_owned(),
                            tokens_before,
                            timestamp: timestamp.to_owned(),
                            method: method.to_owned(),
                            file_details,
                            remote,
                            archive,
                            archive_image_boundary,
                            preserve_data: entry.raw.get("preserveData").filter(|value| !value.is_null()).cloned(),
                            preserve_source,
                            source_entry_ids,
                        },
                    ));
                }
                _ => {
                    entry.native_compaction_entry()?;
                }
            }
        }

        let mut items = Vec::new();
        let start = if let Some((kept_index, summary)) = &latest {
            items.push(CompactedContextItem::Summary(Box::new(summary.clone())));
            *kept_index
        } else {
            0
        };
        for entry in &branch[start..] {
            for message in entry.native_compaction_entry()?.messages {
                items.push(CompactedContextItem::Message(Box::new(SourcedMessage {
                    entry_id: entry.id.clone(),
                    message,
                })));
            }
        }
        Ok(CompactedContextProjection {
            session_id: self.session_id().to_owned(),
            leaf_id: self.leaf.as_ref().expect("validated leaf").clone(),
            items,
        })
    }

    /// Model-visible messages on the current branch (subset of OMP
    /// `buildSessionContext`). Messages ARA cannot decode are skipped and
    /// counted by [`undecodable_messages`](Self::undecodable_messages).
    pub fn build_context(&self) -> Result<Vec<Message>> {
        let branch = self.branch();
        let mut messages = Vec::new();
        for entry in &branch[active_context_start(&branch)..] {
            if let Some(projected) = entry.model_messages()? {
                messages.extend(projected);
            }
        }
        Ok(messages)
    }

    /// Append a soft-compaction summary. The summarized prefix stays raw in
    /// the journal; readers must use [`model_context`](Self::model_context)
    /// so the prefix is replaced by this summary. `first_kept_entry_id` must
    /// be the supported message where raw context resumes, and `source_entry_ids`
    /// the summarized entry IDs for provenance.
    pub fn append_compaction(
        &mut self,
        summary: &str,
        first_kept_entry_id: &str,
        source_entry_ids: &[String],
        tokens_before: u64,
    ) -> Result<String> {
        self.append_compaction_with_preserve(
            summary,
            first_kept_entry_id,
            source_entry_ids,
            tokens_before,
            None,
            None,
            None,
        )
    }

    fn append_compaction_with_preserve(
        &mut self,
        summary: &str,
        first_kept_entry_id: &str,
        source_entry_ids: &[String],
        tokens_before: u64,
        preserve_data: Option<&Value>,
        archive_source_entry_id: Option<&str>,
        preserve_source: Option<NativeSnapcompactArchiveSource>,
    ) -> Result<String> {
        let mut f = serde_json::Map::new();
        f.insert("method".into(), json!("soft"));
        f.insert("summary".into(), json!(summary));
        f.insert("firstKeptEntryId".into(), json!(first_kept_entry_id));
        f.insert("sourceEntryIds".into(), json!(source_entry_ids));
        f.insert("tokensBefore".into(), json!(tokens_before));
        if let Some(preserve) = preserve_data {
            f.insert("preserveData".into(), preserve.clone());
        }
        if let Some(source) = archive_source_entry_id {
            f.insert("archiveSourceEntryId".into(), json!(source));
        }
        self.append_raw_with_snapcompact_source("compaction", f, preserve_source)
    }

    /// Model-visible messages honoring the latest valid soft/handoff summary: the
    /// summarized prefix is replaced by the summary text (sent as a user
    /// message so the model treats it as lower-trust prior context), kept and
    /// later messages stay raw. Falls back to [`build_context`](Self::build_context)
    /// when no valid compaction entry exists.
    pub fn model_context(&self) -> Result<Vec<Message>> {
        self.model_context_with_route(None)
    }

    /// A temporary active-route Assistant carrier exposes checked native replay
    /// to its provider encoder. It is not appended as an invented response or
    /// usage receipt. Other routes retain readable original source context.
    pub fn model_context_for_route(
        &self,
        active_model: &ara_ai::Model,
        native_replay_available: bool,
    ) -> Result<Vec<Message>> {
        self.model_context_with_route(Some((active_model, native_replay_available)))
    }

    /// Select the same reset/branch/summary/replay window as the existing
    /// projection, retaining each raw source exactly once (including excluded
    /// notifications). The converter is a separate Host owner.
    pub fn raw_agent_context_for_route(
        &self,
        active_model: &ara_ai::Model,
        native_replay_available: bool,
    ) -> Result<Vec<RawAgentContextMessage>> {
        self.raw_agent_context_for_route_with_images(active_model, native_replay_available, false)
    }

    pub fn raw_agent_context_for_route_with_images(
        &self,
        active_model: &ara_ai::Model,
        native_replay_available: bool,
        supports_images: bool,
    ) -> Result<Vec<RawAgentContextMessage>> {
        let branch = self.branch();
        let branch = &branch[active_context_start(&branch)..];
        let summary_projection = if branch.iter().any(|entry| entry.kind == "compaction") {
            let projection =
                self.compacted_context_projection_with_route(Some((active_model, native_replay_available)));
            if projection.is_err()
                && branch.iter().any(|entry| entry.native_message.is_some() && entry.model_messages().is_err())
            {
                return Err(agent_body_error("native compacted context requires a checked source window"));
            }
            projection.ok()
        } else {
            None
        };
        let summary = summary_projection.and_then(|projection| {
            projection.items.into_iter().find_map(|item| match item {
                CompactedContextItem::Summary(summary) => Some(summary),
                _ => None,
            })
        });
        let mut output = Vec::new();
        let start = if let Some(summary) = summary {
            let source = branch
                .iter()
                .find(|entry| entry.id == summary.entry_id)
                .ok_or_else(|| agent_body_error("summary source is outside the active branch"))?;
            let mut body = source.raw.clone();
            let fields = body.as_object_mut().ok_or_else(|| agent_body_error("invalid summary source"))?;
            fields.remove("type");
            fields.remove("id");
            fields.remove("parentId");
            fields.insert("role".into(), json!("compactionSummary"));
            fields.insert(
                "timestamp".into(),
                json!(entry_timestamp(&source.raw).ok_or_else(|| agent_body_error("invalid summary timestamp"))?),
            );
            fields.insert("method".into(), json!(summary.method));
            let (body, checked_projection) = if source.snapcompact_source.is_some() {
                (AgentMessageBody::Native(source.compaction_summary_source()?), None)
            } else {
                let checked_projection = Some(vec![raw_checked_summary_projection(
                    &summary,
                    active_model,
                    supports_images,
                    self.blob_store.as_ref(),
                )?]);
                Entry::project_agent_message_body(&body)?;
                (AgentMessageBody::Legacy(body), checked_projection)
            };
            output.push(RawAgentContextMessage { body, source: source.original_source(), checked_projection });
            if let Some(remote) = summary.remote {
                branch
                    .iter()
                    .position(|entry| entry.id == remote.replay_through_entry_id)
                    .ok_or_else(|| agent_body_error("remote replay boundary is outside the active branch"))?
                    + 1
            } else {
                branch
                    .iter()
                    .position(|entry| entry.id == summary.first_kept_entry_id)
                    .ok_or_else(|| agent_body_error("summary kept boundary is outside the active branch"))?
            }
        } else {
            0
        };
        for entry in &branch[start..] {
            if let Some(body) = entry.agent_message_body()? {
                output.push(RawAgentContextMessage { body, source: entry.original_source(), checked_projection: None });
            }
        }
        Ok(output)
    }

    /// Exact archive grant: current body equality plus current branch/route
    /// selection. Neither an archive-shaped value nor a marker grants replay.
    pub fn checked_snapcompact_source_for_route(
        &self,
        current: &ara_rpc::WireValue,
        source_entry_id: &str,
        model: &ara_ai::Model,
        native_replay_available: bool,
        supports_images: bool,
    ) -> Result<Option<NativeMessageSource>> {
        self.checked_snapcompact_source_for_route_with_options(
            current,
            source_entry_id,
            model,
            native_replay_available,
            supports_images,
            ara_snapcompact::HistoryBlockOptions { max_frame_data_bytes: Some(snapcompact::FRAME_DATA_BYTES_BUDGET) },
            true,
        )
    }

    /// Host supplies the current deployment's payload budget and reasoning
    /// policy; the journal retains branch/source checks and owns blob IO.
    #[allow(clippy::too_many_arguments)]
    pub fn checked_snapcompact_source_for_route_with_options(
        &self,
        current: &ara_rpc::WireValue,
        source_entry_id: &str,
        model: &ara_ai::Model,
        native_replay_available: bool,
        supports_images: bool,
        history_options: ara_snapcompact::HistoryBlockOptions,
        include_thinking: bool,
    ) -> Result<Option<NativeMessageSource>> {
        let branch = self.branch();
        let branch = &branch[active_context_start(&branch)..];
        let Some(source) = branch.iter().find(|entry| entry.id == source_entry_id && entry.kind == "compaction") else {
            return Ok(None);
        };
        if !source.compaction_summary_source()?.observe()?.deep_equal(current) {
            return Ok(None);
        }
        let projection = self
            .compacted_context_projection_with_route(Some((model, native_replay_available)))
            .map_err(|error| agent_body_error(format!("checked exact archive source projection: {error}")))?;
        let Some(summary) = projection.items.iter().find_map(|item| match item {
            CompactedContextItem::Summary(summary)
                if summary.entry_id == source_entry_id && summary.remote.is_none() && summary.method != "handoff" =>
            {
                Some(summary.as_ref())
            }
            _ => None,
        }) else {
            return Ok(None);
        };
        // The existing checked archive projection accepts attached frames
        // regardless of supports_images; provider preparation owns that policy.
        let _ = supports_images;
        let model_source = if summary.has_archive() {
            let details = source
                .raw
                .get("details")
                .cloned()
                .and_then(|value| serde_json::from_value::<ara_snapcompact::CompactionDetails>(value).ok());
            snapcompact::model_source_from_summary_with_blob_store(
                &summary.summary,
                summary.exact_preserve_json()?.as_deref(),
                self.blob_store.as_ref(),
                Some(history_options),
                include_thinking,
                details.as_ref(),
            )?
        } else {
            snapcompact::model_source_from_summary(
                &format!("[Compacted summary of earlier turns; source entries withheld]\n{}", summary.summary),
                None,
            )?
        };
        let mut body = native_message::clone_wire(model_source.observe()?.as_ref());
        let timestamp = chrono::DateTime::parse_from_rfc3339(&summary.timestamp)
            .map(|timestamp| timestamp.timestamp_millis())
            .unwrap_or(0);
        body.insert("timestamp", ara_rpc::WireValue::Number(timestamp as f64));
        body.insert("historyRewriteAt", ara_rpc::WireValue::Number(timestamp as f64));
        Ok(Some(NativeMessageSource::persisted(body)))
    }

    /// Host refresh after admission selects a new route while a Run is busy.
    /// Source identity is supplied by the Host's trusted weak registration;
    /// exact CURRENT body equality is required before any source projection.
    /// An edited summary remains editable raw input and receives no grant.
    pub fn checked_agent_message_projection_for_route(
        &self,
        body: &Value,
        source_entry_id: &str,
        model: &ara_ai::Model,
        native_replay_available: bool,
        supports_images: bool,
    ) -> Result<Option<Vec<Message>>> {
        if body["role"] != "compactionSummary" {
            return Ok(None);
        }
        let branch = self.branch();
        let branch = &branch[active_context_start(&branch)..];
        let Some(source) = branch.iter().find(|entry| entry.id == source_entry_id && entry.kind == "compaction") else {
            return Ok(None);
        };
        // A partial checked mirror without preserveData cannot receive a
        // source grant. Exact callers use the Wire equality entry point below.
        if source.snapcompact_source.as_ref().is_some_and(|source| source.checked_preserve().is_err()) {
            return Ok(None);
        }
        let method =
            source.raw.get("method").filter(|method| !method.is_null()).and_then(Value::as_str).unwrap_or("soft");
        let mut expected = source.raw.clone();
        let fields = expected.as_object_mut().ok_or_else(|| agent_body_error("invalid summary source"))?;
        fields.remove("type");
        fields.remove("id");
        fields.remove("parentId");
        fields.insert("role".into(), json!("compactionSummary"));
        fields.insert(
            "timestamp".into(),
            json!(entry_timestamp(&source.raw).ok_or_else(|| agent_body_error("invalid summary timestamp"))?),
        );
        fields.insert("method".into(), json!(method));
        if body != &expected {
            return Ok(None);
        }
        let projection = self
            .compacted_context_projection_with_route(Some((model, native_replay_available)))
            .map_err(|error| agent_body_error(format!("checked raw source projection: {error}")))?;
        if let Some(summary) = projection.items.iter().find_map(|item| match item {
            CompactedContextItem::Summary(summary) if summary.entry_id == source_entry_id => Some(summary.as_ref()),
            _ => None,
        }) {
            return Ok(Some(vec![raw_checked_summary_projection(
                summary,
                model,
                supports_images,
                self.blob_store.as_ref(),
            )?]));
        }
        // A non-reusable native remote record contributes the checked original
        // source window it covered, rather than borrowing foreign native proof
        // or duplicating the current Run's post-boundary tail.
        if method != "remote" {
            return Ok(None);
        }
        let Some(through) = source.raw.get("providerReplayThroughEntryId").and_then(Value::as_str) else {
            return Ok(None);
        };
        let through = branch
            .iter()
            .position(|entry| entry.id == through)
            .ok_or_else(|| agent_body_error("checked remote source boundary is unavailable"))?;
        let positions: HashMap<_, _> =
            branch.iter().enumerate().map(|(index, entry)| (entry.id.as_str(), index)).collect();
        let mut messages = Vec::new();
        for item in projection.items {
            match item {
                CompactedContextItem::Summary(summary)
                    if positions.get(summary.entry_id.as_str()).is_some_and(|index| *index <= through) =>
                {
                    messages.push(raw_checked_summary_projection(
                        &summary,
                        model,
                        supports_images,
                        self.blob_store.as_ref(),
                    )?)
                }
                CompactedContextItem::Message(message)
                    if positions.get(message.entry_id.as_str()).is_some_and(|index| *index <= through) =>
                {
                    messages.push(message.message)
                }
                _ => {}
            }
        }
        Ok(Some(messages))
    }

    fn model_context_with_route(&self, route: Option<(&ara_ai::Model, bool)>) -> Result<Vec<Message>> {
        let messages = match self.compacted_context_projection_with_route(route) {
            Ok(projection) if projection.items.iter().any(|item| matches!(item, CompactedContextItem::Summary(_))) => {
                if projection.items.iter().any(|item| {
                    matches!(item,
                    CompactedContextItem::Summary(summary) if summary.has_archive() && summary.archive.is_none())
                }) {
                    return Err(agent_body_error("exact archive requires native model context"));
                }
                projection
                    .items
                    .into_iter()
                    .map(|item| {
                        Ok(match item {
                            CompactedContextItem::Summary(s) if s.remote.is_some() => {
                                remote::model_message(&s, route.expect("readable native replay has a route").0)
                            }
                            CompactedContextItem::Summary(s) if s.method == "handoff" => handoff::model_message(&s),
                            CompactedContextItem::Summary(s) if s.archive.is_some() => {
                                let mut message = snapcompact::model_message_from_summary_with_blob_store(
                                    &s.summary,
                                    s.preserve_data.as_ref(),
                                    self.blob_store.as_ref(),
                                )?;
                                if let Message::User(user) = &mut message {
                                    user.timestamp = chrono::DateTime::parse_from_rfc3339(&s.timestamp)
                                        .map(|timestamp| timestamp.timestamp_millis())
                                        .unwrap_or(0);
                                }
                                message
                            }
                            CompactedContextItem::Summary(s) => Message::User(UserMessage::text(format!(
                                "[Compacted summary of earlier turns; source entries withheld]\n{}",
                                s.summary
                            ))),
                            CompactedContextItem::Message(m) => m.message.clone(),
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
            }
            _ => self.build_context()?,
        };
        // Fixed session-context.ts:346 preserves live/transcript receipts, but
        // never sends an empty error turn back to the model after native resume.
        // Source snapshots deliberately keep the exact original message/ID.
        Ok(messages
            .into_iter()
            .filter(|message| message.as_assistant().is_none_or(|assistant| !is_empty_error_turn(assistant)))
            .collect())
    }

    pub fn undecodable_messages(&self) -> usize {
        self.branch()
            .into_iter()
            .filter(|e| {
                matches!(e.kind.as_str(), "message" | "custom_message" | "branch_summary") && !e.is_decodable_message()
            })
            .count()
    }

    /// First native restore candidate. Transient fallback receipts do not
    /// replace the configured default; the Host still owns model availability.
    pub fn current_model(&self) -> Result<Option<String>> {
        Ok(self.restorable_model_candidates()?.into_iter().next())
    }

    /// Fixed `session-context.ts:91-109,239-280`: build the branch role map,
    /// preserve an explicit default against assistant inference, then try the
    /// last non-ephemeral role followed by default. Native legacy Sessions infer
    /// default from assistant provider/model only until an explicit default.
    pub fn restorable_model_candidates(&self) -> Result<Vec<String>> {
        let mut models = HashMap::new();
        let mut explicit_default = false;
        let mut last_role = None;
        for entry in self.branch() {
            if entry.kind == "model_change" {
                let role = entry.raw.get("role").and_then(Value::as_str).unwrap_or("default");
                last_role = Some(role.to_owned());
                if let Some(model) = entry.raw.get("model").and_then(Value::as_str).filter(|model| !model.is_empty()) {
                    models.insert(role.to_owned(), model.to_owned());
                    if role == "default" {
                        explicit_default = true;
                    }
                }
            } else if !explicit_default {
                if entry.observed_role()?.is_some_and(|role| role.equals_ascii("assistant")) {
                    if let (
                        Some(ara_rpc::WireValue::String(ref provider)),
                        Some(ara_rpc::WireValue::String(ref model)),
                    ) = (entry.observed_message_field(&["provider"])?, entry.observed_message_field(&["model"])?)
                    {
                        let provider = provider
                            .to_utf8()
                            .map_err(|_| agent_body_error("native provider identity requires a lossless route"))?;
                        let model = model
                            .to_utf8()
                            .map_err(|_| agent_body_error("native model identity requires a lossless route"))?;
                        models.insert("default".into(), format!("{provider}/{model}"));
                    }
                }
            }
        }
        let default = models.get("default");
        let role_model = last_role
            .as_deref()
            .filter(|role| !role.is_empty() && *role != "default" && *role != EPHEMERAL_MODEL_CHANGE_ROLE)
            .and_then(|role| models.get(role));
        let mut candidates = Vec::new();
        if let Some(model) = role_model {
            candidates.push(model.clone());
        }
        if let Some(model) = default.filter(|model| role_model != Some(*model)) {
            candidates.push(model.clone());
        }
        Ok(candidates)
    }

    /// Native effective thinking defaults to off, including an empty branch.
    /// Null/missing levels on a recorded change also mean off.
    pub fn current_thinking_level(&self) -> Option<String> {
        self.branch()
            .into_iter()
            .rev()
            .find(|entry| entry.kind == "thinking_level_change")
            .map(|entry| entry.raw.get("thinkingLevel").and_then(Value::as_str).unwrap_or("off").to_owned())
            .or_else(|| Some("off".into()))
    }

    /// Fixed `session-context.ts:257-259`: configured ?? thinkingLevel.
    /// Ephemeral model selection does not suppress thinking receipts.
    pub fn current_configured_thinking_level(&self) -> Option<String> {
        self.branch().into_iter().rev().find(|entry| entry.kind == "thinking_level_change").and_then(|entry| {
            entry
                .raw
                .get("configured")
                .and_then(Value::as_str)
                .or_else(|| entry.raw.get("thinkingLevel").and_then(Value::as_str))
                .map(str::to_owned)
        })
    }

    /// Pair tool calls that never got a recorded result with explicit
    /// "effect unknown" error results. Works on the raw journal so an entry ARA
    /// cannot decode never hides a call or result. Only calls of the last
    /// assistant turn whose followers are all tool results or decodable Bash
    /// receipts excluded from model context can be paired
    /// adjacently; earlier gaps are reported, never replayed.
    pub fn recover_interrupted_tool_calls(&mut self) -> Result<Recovery> {
        let raw_branch = self.branch();
        let branch: Vec<Entry> =
            raw_branch[active_context_start(&raw_branch)..].iter().map(|entry| (**entry).clone()).collect();
        // Narrow current role/ID/name Gets. Arguments and unrelated Graph
        // extension fields are not traversed for this decision.
        let roles = branch.iter().map(Entry::observed_role).collect::<Result<Vec<_>>>()?;
        let role_is = |index: usize, role: &str| roles[index].as_ref().is_some_and(|value| value.equals_ascii(role));
        let mut recovery = Recovery::default();
        let mut answered = HashSet::<ara_rpc::WireString>::new();
        let mut last_assistant = None;
        for (index, entry) in branch.iter().enumerate() {
            if role_is(index, "toolResult") {
                match entry.observed_message_field(&["toolCallId"])? {
                    Some(ara_rpc::WireValue::String(ref id)) => {
                        answered.insert(id.clone());
                    }
                    _ if entry.native_message.is_some() => {
                        return Err(agent_body_error("native result requires its exact toolCallId"));
                    }
                    _ => {}
                }
            } else if role_is(index, "assistant") {
                last_assistant = Some(index);
            }
        }
        for (index, entry) in branch.iter().enumerate() {
            if !role_is(index, "assistant") {
                continue;
            }
            let missing =
                entry.observed_tool_calls()?.into_iter().filter(|(id, _)| !answered.contains(id)).collect::<Vec<_>>();
            if missing.is_empty() {
                continue;
            }
            let mut tail_is_results_only = Some(index) == last_assistant;
            if tail_is_results_only {
                for next in index + 1..branch.len() {
                    let follower = &branch[next];
                    let allowed = if follower.kind == "message" {
                        role_is(next, "toolResult")
                            || (role_is(next, "bashExecution") && follower.observed_excluded_bash_execution()?)
                    } else {
                        follower.model_messages()?.is_some_and(|messages| messages.is_empty())
                    };
                    if !allowed {
                        tail_is_results_only = false;
                        break;
                    }
                }
            }
            if !tail_is_results_only {
                recovery.unpaired_earlier.extend(missing.into_iter().map(|(id, _)| id));
                continue;
            }
            for (id, name) in missing {
                use ara_rpc::WireValue as W;
                let result = W::object(vec![
                    ("role", W::String("toolResult".into())),
                    ("toolCallId", W::String(id.clone())),
                    ("toolName", W::String(name)),
                    (
                        "content",
                        W::Array(vec![W::object(vec![
                            ("type", W::String("text".into())),
                            ("text", W::String(UNKNOWN_EFFECT_TEXT.into())),
                        ])]),
                    ),
                    (
                        "details",
                        W::object(vec![
                            ("__synthetic", W::Bool(true)),
                            ("source", W::String("interrupted_unknown_effect".into())),
                            ("executed", W::String("unknown".into())),
                        ]),
                    ),
                    ("isError", W::Bool(true)),
                    ("timestamp", W::Number(now_ms() as f64)),
                ]);
                self.append_native_message(NativeMessageSource::persisted(result))?;
                recovery.paired.push(id);
            }
        }
        Ok(recovery)
    }
}

/// Most recently modified `.jsonl` session in `dir` (for `--continue`).
pub fn latest_session(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, p));
        }
    }
    best.map(|(_, p)| p)
}

#[cfg(test)]
mod model_change_publication_tests {
    use super::*;

    #[test]
    fn composite_directory_barrier_failure_retains_exact_published_receipts() {
        // Exercise both public APIs' complete commit paths using the existing
        // rewrite directory barrier. The injected failure occurs after the
        // real temp-file sync and rename, rather than simulating a failed write.
        for with_thinking in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
            journal.append_model_and_thinking_change("fake/primary", None, false, Some("low"), Some("low")).unwrap();
            journal.materialize().unwrap();
            let path = journal.path().to_path_buf();
            let previous_bytes = fs::read(&path).unwrap();
            let previous_len = journal.entries().len();
            let previous_leaf = journal.leaf_id().unwrap().to_owned();
            let mut barrier_reached = false;
            let fail_directory_sync = |directory: &Path| {
                assert_eq!(directory, dir.path());
                barrier_reached = true;
                // The complete candidate must already be observable on disk
                // when the existing primitive reports uncertain durability.
                let published = SessionJournal::open(&path).unwrap();
                assert_eq!(published.entries().len(), previous_len + if with_thinking { 2 } else { 1 });
                assert_eq!(published.current_model().unwrap().as_deref(), Some("fake/replaced"));
                Err(std::io::Error::other("injected model-change directory barrier failure"))
            };
            let (model_entry_id, thinking_entry_id) = if with_thinking {
                let error = journal
                    .append_model_and_thinking_change_with_sync(
                        "fake/replaced",
                        Some("default"),
                        true,
                        Some("high"),
                        Some("auto"),
                        fail_directory_sync,
                    )
                    .unwrap_err();
                assert!(error.history_published());
                let ModelAndThinkingChangeError::PublishedButDurabilityUnknown {
                    model_entry_id,
                    thinking_entry_id,
                    ..
                } = error
                else {
                    panic!("paired transaction lost its publication phase");
                };
                (model_entry_id, Some(thinking_entry_id))
            } else {
                let error = journal
                    .append_model_and_optional_thinking_change_with_sync(
                        "fake/replaced",
                        Some("default"),
                        true,
                        None,
                        fail_directory_sync,
                    )
                    .unwrap_err();
                assert!(error.history_published());
                let ModelAndOptionalThinkingChangeError::PublishedButDurabilityUnknown {
                    model_entry_id,
                    thinking_entry_id,
                    ..
                } = error
                else {
                    panic!("optional transaction lost its publication phase");
                };
                (model_entry_id, thinking_entry_id)
            };
            assert!(barrier_reached);
            assert_eq!(thinking_entry_id.is_some(), with_thinking);
            assert_eq!(journal.entries().len(), previous_len + if with_thinking { 2 } else { 1 });
            assert_eq!(journal.entries()[previous_len].id, model_entry_id);
            assert_eq!(journal.entries()[previous_len].parent_id.as_deref(), Some(previous_leaf.as_str()));
            if let Some(thinking_entry_id) = &thinking_entry_id {
                assert_eq!(journal.entries().last().unwrap().id, *thinking_entry_id);
                assert_eq!(journal.entries().last().unwrap().parent_id.as_deref(), Some(model_entry_id.as_str()));
                assert!(journal.ids.contains(thinking_entry_id));
            }
            assert_eq!(journal.leaf_id(), Some(thinking_entry_id.as_deref().unwrap_or(model_entry_id.as_str())));
            assert!(journal.ids.contains(&model_entry_id));
            assert!(journal.materialized);
            assert!(!journal.rewrite_required);
            assert_ne!(fs::read(&path).unwrap(), previous_bytes);
            let reopened = SessionJournal::open(&path).unwrap();
            assert_eq!(reopened.entries(), journal.entries());
            assert_eq!(reopened.leaf_id(), journal.leaf_id());
            assert_eq!(reopened.current_thinking_level().as_deref(), Some(if with_thinking { "high" } else { "low" }));
            assert_eq!(
                reopened.current_configured_thinking_level().as_deref(),
                Some(if with_thinking { "auto" } else { "low" })
            );
        }
    }
}

// Append to ara-session/src/lib.rs as an existing @lib test module.
#[cfg(test)]
mod raw_messages_r_group {
    use super::*;

    #[test]
    fn raw_lossless_append_reopen_reserved_fields_and_recovery_group() {
        let dir = tempfile::tempdir().unwrap();
        let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
        let user = json!({"role":"user","content":"source user","timestamp":1,"application":{"user":true}});
        journal.append_agent_message_body(&user).unwrap();
        let custom = json!({"role":"custom","customType":"extension-fixture","content":{"raw":[1,2]},"id":"body-id","type":"body-type","parentId":"body-parent","timestamp":2,"application":{"retained":true}});
        let custom_id = journal.append_agent_message_body(&custom).unwrap();
        let mut failed = AssistantMessage::empty("openai-responses", "openai", "raw-native");
        failed.stop_reason = StopReason::Error;
        failed.error_message = Some("source failure".into());
        failed.content.push(AssistantBlock::text("failed original output"));
        let mut body = serde_json::to_value(Message::Assistant(failed.clone())).unwrap();
        body["application"] = json!({"unknown":["preserved",3]});
        body["errorId"] = json!(0x0002_1000_u32);
        let id = journal.append_agent_message_body(&body).unwrap();
        let leaf = journal.leaf_id().unwrap().to_owned();
        let token = journal.begin_failed_assistant_recovery(&leaf, &id, &failed).unwrap();
        assert_eq!(token.failed_message_body().unwrap().checked_legacy().unwrap(), body);
        let FailedAssistantRecoveryOutcome::Restored { appended_entry_id: Some(restored), .. } =
            journal.finish_failed_assistant_recovery(token, false).unwrap()
        else {
            panic!("contentful rollback")
        };
        assert_ne!(restored, id);
        assert_eq!(
            journal.branch().last().unwrap().raw["message"],
            body,
            "rollback uses checked raw source, preserving errorId and application extensions"
        );
        let reopened = SessionJournal::open(journal.path()).unwrap();
        assert_eq!(reopened.entries(), journal.entries());
        assert_eq!(
            reopened
                .entries()
                .iter()
                .find(|entry| entry.id == custom_id)
                .unwrap()
                .agent_message_body()
                .unwrap()
                .map(|body| body.checked_legacy().unwrap()),
            Some(custom.clone())
        );
        let model = ara_ai::Model {
            id: "raw-native".into(),
            api: "openai-responses".into(),
            provider: "openai".into(),
            base_url: "http://fixture.invalid/v1".into(),
            reasoning: false,
            max_tokens: None,
            context_window: None,
            tokenizer: None,
        };
        let context = reopened.raw_agent_context_for_route(&model, false).unwrap();
        assert!(context.iter().any(|item| item.body.checked_legacy().unwrap() == user));
        assert!(context.iter().any(|item| item.body.checked_legacy().unwrap() == custom));
        assert_eq!(context.last().unwrap().body.checked_legacy().unwrap(), body);
        assert_eq!(
            context.last().unwrap().source.checked_legacy().unwrap()["id"],
            restored,
            "source is the retained Session envelope, outside editable body"
        );
    }
}
