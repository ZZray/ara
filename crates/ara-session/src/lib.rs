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
//! rewrites are atomic with a directory sync where supported; a failed append is rolled back
//! so `Err` always means "not recorded"; tool calls interrupted before their
//! results were recorded are paired with explicit "effect unknown" results on
//! resume instead of being replayed.
//!
//! Not ported (open): compaction/branch-summary entries in live model context building,
//! label editing, general custom-entry semantics, v1/v2 migrations, SQL/Redis storage,
//! listing/search, moving, title generation, blob externalization.

mod skill_prompt;

pub use skill_prompt::{SKILL_PROMPT_CUSTOM_TYPE, UserSkillPrompt};

use ara_ai::{AssistantBlock, Message, ToolResultMessage, UserBlock, UserContent, UserMessage, now_ms};
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
    let Some(meta) = meta else { return String::new() };
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
                Some((start, end)) if end >= start => format!("Showing lines {start}-{end} of {total_lines}"),
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
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: String,
    pub raw: Value,
}

impl Entry {
    pub fn bash_execution(&self) -> Option<BashExecutionMessage> {
        BashExecutionMessage::from_entry(&self.raw)
    }

    fn is_excluded_bash_execution(&self) -> bool {
        self.bash_execution().is_some_and(|message| message.exclude_from_context == Some(true))
    }

    fn is_decodable_message(&self) -> bool {
        if self.role() == Some("bashExecution") { self.bash_execution().is_some() } else { self.message().is_some() }
    }

    /// Original user-invoked Skill identity, independent of its model projection.
    pub fn skill_prompt(&self) -> Option<UserSkillPrompt> {
        UserSkillPrompt::from_entry(&self.raw)
    }

    fn is_user_skill_prompt_candidate(&self) -> bool {
        self.kind == "custom_message"
            && self.raw.get("customType").and_then(Value::as_str) == Some(SKILL_PROMPT_CUSTOM_TYPE)
            && self.raw.get("attribution").and_then(Value::as_str) == Some("user")
    }

    /// Model-visible message, including a directly invoked Skill custom entry.
    pub fn message(&self) -> Option<Message> {
        match self.kind.as_str() {
            "message" if self.role() == Some("bashExecution") => self.bash_execution()?.model_message(),
            "message" => serde_json::from_value(self.raw.get("message")?.clone()).ok(),
            "custom_message" => self.skill_prompt().map(|prompt| prompt.model_message()),
            _ => None,
        }
    }

    fn role(&self) -> Option<&str> {
        if self.kind != "message" {
            return None;
        }
        self.raw.pointer("/message/role").and_then(Value::as_str)
    }
}

/// Storage-side guard for a soft summary's replaced prefix. It mirrors the
/// Agent's `validate_completed_summary_span` without making Session depend on
/// the Agent crate: every tool call is paired with its result, the prefix does
/// not end with an unanswered prompt, and no unknown-effect result is hidden.
/// Turns that ended aborted, errored or length-stopped are summarized, as in
/// fixed OMP. The raw prefix stays available for audit.
fn safe_soft_summary_prefix(branch: &[&Entry]) -> bool {
    let mut saw_message = false;
    let mut pending: HashMap<String, String> = HashMap::new();
    let mut ends_with_user = false;
    for entry in branch {
        let Some(message) = entry.message() else { continue };
        if !saw_message && !matches!(message, Message::User(_)) {
            return false;
        }
        saw_message = true;
        match &message {
            Message::User(user) => {
                if !pending.is_empty()
                    || matches!(&user.content, UserContent::Blocks(blocks) if blocks.iter().any(|block| matches!(block, UserBlock::Image(_))))
                {
                    return false;
                }
                ends_with_user = true;
            }
            Message::Developer(_) => return false,
            Message::Assistant(assistant) => {
                if !pending.is_empty()
                    || assistant.content.iter().any(|block| matches!(block, AssistantBlock::Image(_)))
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
                if result.content.iter().any(|block| matches!(block, UserBlock::Image(_)))
                    || result.details.as_ref().is_some_and(|details| {
                        details.get("panicked").and_then(Value::as_bool) == Some(true)
                            || (details.get("__synthetic").and_then(Value::as_bool) == Some(true)
                                && details.get("source").and_then(Value::as_str) == Some("interrupted_unknown_effect")
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
    saw_message && pending.is_empty() && !ends_with_user
}

/// One decoded message tied to its actual journal entry ID. This is an
/// in-memory snapshot; a later writer must re-read and compare the leaf before
/// committing a compaction derived from it.
#[derive(Debug, Clone, PartialEq)]
pub struct SourcedMessage {
    pub entry_id: String,
    pub message: Message,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompactionSourceSnapshot {
    pub session_id: String,
    pub leaf_id: String,
    pub messages: Vec<SourcedMessage>,
}

/// A derived summary remains distinct from a user message. The host/Agent
/// owns conversion to a model-visible message and must keep its attribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionSummaryView {
    pub entry_id: String,
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    pub timestamp: String,
    /// `None` means an imported entry has no verifiable source list.
    pub source_entry_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompactedContextItem {
    Summary(CompactionSummaryView),
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

/// What `open` found and repaired.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadReport {
    pub malformed_records: usize,
    /// Backup of the original bytes written before the torn file is rewritten.
    pub backup: Option<PathBuf>,
}

/// Outcome of [`SessionJournal::recover_interrupted_tool_calls`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recovery {
    /// Calls paired with an "effect unknown" result right after their assistant turn.
    pub paired: Vec<String>,
    /// Earlier calls without results that can no longer be paired adjacently;
    /// left for the provider-side pairing guard and reported to the host.
    pub unpaired_earlier: Vec<String>,
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
                let Some(target) = entry.raw.get("targetId").and_then(Value::as_str) else { continue };
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
            let parsed: Option<Value> = serde_json::from_str(line).ok();
            let Some(value) = parsed.filter(Value::is_object) else {
                if header.is_none() {
                    return Err(corrupt(CORRUPT_HEADER_MESSAGE.into()));
                }
                malformed += 1;
                continue;
            };
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
                    entries.push(Entry { id, parent_id, kind, raw: value });
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
            report: LoadReport { malformed_records: malformed, backup: None },
        })
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

    fn file_body(&self) -> String {
        let mut body = serialize_title_slot(&self.title);
        body.push_str(&Self::line_for(&self.header));
        for e in &self.entries {
            body.push_str(&Self::line_for(&e.raw));
        }
        body
    }

    fn has_assistant(&self) -> bool {
        self.entries.iter().any(|e| e.role() == Some("assistant"))
    }

    fn dir(&self) -> PathBuf {
        self.path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
    }

    /// Atomic full rewrite: synced backup of damaged bytes, unique temp file,
    /// fsync, rename, and directory fsync where supported.
    fn rewrite(&mut self) -> Result<()> {
        if !self.persistent {
            return Ok(());
        }
        let dir = self.dir();
        fs::create_dir_all(&dir)?;
        if self.pending_backup && self.path.exists() {
            let backup = self.path.with_extension(format!("jsonl.torn-{}.bak", now_ms()));
            fs::copy(&self.path, &backup)?;
            OpenOptions::new().write(true).open(&backup)?.sync_all()?;
            sync_dir(&dir)?;
            self.report.backup = Some(backup);
            self.pending_backup = false;
        }
        let tmp =
            self.path.with_extension(format!("jsonl.tmp-{}-{}", std::process::id(), uuid::Uuid::new_v4().simple()));
        let written = (|| -> std::io::Result<()> {
            let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            f.write_all(self.file_body().as_bytes())?;
            f.sync_all()?;
            fs::rename(&tmp, &self.path)?;
            sync_dir(&dir)
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        self.materialized = true;
        self.rewrite_required = false;
        Ok(())
    }

    fn persist(&mut self, entry_raw: &Value) -> Result<()> {
        if !self.persistent {
            return Ok(());
        }
        if !self.materialized && !self.has_assistant() {
            return Ok(()); // lazy: nothing on disk until an assistant message exists
        }
        if !self.materialized || self.rewrite_required {
            return self.rewrite();
        }
        let mut f = OpenOptions::new().append(true).open(&self.path)?;
        f.write_all(Self::line_for(entry_raw).as_bytes())?;
        f.sync_data()?;
        Ok(())
    }

    fn append_raw(&mut self, kind: &str, mut fields: serde_json::Map<String, Value>) -> Result<String> {
        let id = generate_id(&self.ids);
        let mut raw = serde_json::Map::new();
        raw.insert("type".into(), json!(kind));
        raw.insert("id".into(), json!(id));
        raw.insert("parentId".into(), self.leaf.clone().map(Value::String).unwrap_or(Value::Null));
        raw.insert("timestamp".into(), json!(now_iso()));
        raw.append(&mut fields);
        let raw = Value::Object(raw);
        let previous_leaf = self.leaf.clone();
        self.entries.push(Entry { id: id.clone(), parent_id: self.leaf.clone(), kind: kind.into(), raw: raw.clone() });
        self.ids.insert(id.clone());
        self.leaf = Some(id.clone());
        if let Err(e) = self.persist(&raw) {
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

    pub fn append_message(&mut self, message: &Message) -> Result<String> {
        let mut f = serde_json::Map::new();
        f.insert("message".into(), serde_json::to_value(message).expect("message serializes"));
        self.append_raw("message", f)
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

    /// OMP `appendModelChange`: `model` in `provider/modelId` form.
    pub fn append_model_change(&mut self, model: &str) -> Result<String> {
        let mut f = serde_json::Map::new();
        f.insert("model".into(), json!(model));
        self.append_raw("model_change", f)
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
        let leaf_id = self.leaf.as_ref().expect("checked leaf").clone();
        let mut current = Some(leaf_id.as_str());
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
        let mut messages = Vec::new();
        for entry in self.strict_compaction_branch()? {
            match entry.kind.as_str() {
                "message" if entry.is_excluded_bash_execution() => {}
                "message" => messages.push(SourcedMessage {
                    entry_id: entry.id.clone(),
                    message: entry
                        .message()
                        .ok_or_else(|| CompactionSourceError::UndecodableMessage { id: entry.id.clone() })?,
                }),
                "custom_message" => {
                    let message = entry.message().ok_or_else(|| CompactionSourceError::UnsupportedContextEntry {
                        id: entry.id.clone(),
                        kind: entry.kind.clone(),
                    })?;
                    messages.push(SourcedMessage { entry_id: entry.id.clone(), message });
                }
                "model_change" | "label" | "title_change" => {}
                _ => {
                    return Err(CompactionSourceError::UnsupportedContextEntry {
                        id: entry.id.clone(),
                        kind: entry.kind.clone(),
                    });
                }
            }
        }
        Ok(CompactionSourceSnapshot {
            session_id: self.session_id().to_owned(),
            leaf_id: self.leaf.as_ref().expect("validated leaf").clone(),
            messages,
        })
    }

    /// Strict, read-only projection of the current branch after a soft
    /// compaction. Raw entries remain in the journal; the latest summary is a
    /// distinct item followed by kept and later messages. This does not turn
    /// the summary into a user message or make a provider request.
    pub fn compacted_context_projection(
        &self,
    ) -> std::result::Result<CompactedContextProjection, CompactionProjectionError> {
        let branch = self.strict_compaction_branch()?;
        let mut latest: Option<(usize, CompactionSummaryView)> = None;
        for (index, entry) in branch.iter().enumerate() {
            match entry.kind.as_str() {
                "message" => {
                    if !entry.is_decodable_message() {
                        return Err(CompactionSourceError::UndecodableMessage { id: entry.id.clone() }.into());
                    }
                }
                "custom_message" if entry.message().is_some() => {}
                "model_change" | "label" | "title_change" => {}
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
                        .position(|candidate| {
                            candidate.id == first_kept
                                && matches!(candidate.kind.as_str(), "message" | "custom_message")
                        })
                        .ok_or_else(|| CompactionProjectionError::MissingKeptMessage { id: entry.id.clone() })?;
                    if !matches!(branch[kept_index].message(), Some(Message::User(_)))
                        || !safe_soft_summary_prefix(&branch[..kept_index])
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
                    if let Some(method) = entry.raw.get("method").filter(|value| !value.is_null()) {
                        let method = method.as_str().ok_or_else(|| invalid("method"))?;
                        if method != "soft" {
                            return Err(CompactionProjectionError::UnsupportedMethod {
                                id: entry.id.clone(),
                                method: method.to_owned(),
                            });
                        }
                    }
                    if entry.raw.get("providerReplayThroughEntryId").is_some_and(|value| !value.is_null())
                        || entry.raw.get("fromExtension").and_then(Value::as_bool) == Some(true)
                        || entry.raw.get("preserveData").is_some_and(|value| match value {
                            Value::Null => false,
                            Value::Object(fields) => !fields.is_empty(),
                            _ => true,
                        })
                    {
                        return Err(CompactionProjectionError::UnsupportedReplayData { id: entry.id.clone() });
                    }
                    let source_entry_ids = match entry.raw.get("sourceEntryIds") {
                        None => None,
                        Some(Value::Array(ids)) => {
                            let ids: Vec<String> = ids
                                .iter()
                                .map(|id| id.as_str().filter(|id| !id.is_empty()).map(str::to_owned))
                                .collect::<Option<_>>()
                                .ok_or_else(|| invalid("sourceEntryIds"))?;
                            let expected: Vec<String> = branch[..kept_index]
                                .iter()
                                .filter(|candidate| {
                                    matches!(candidate.kind.as_str(), "message" | "custom_message")
                                        && !candidate.is_excluded_bash_execution()
                                })
                                .map(|candidate| candidate.id.clone())
                                .collect();
                            if ids.is_empty() || ids != expected {
                                return Err(CompactionProjectionError::SourceIdsMismatch { id: entry.id.clone() });
                            }
                            Some(ids)
                        }
                        Some(_) => return Err(invalid("sourceEntryIds")),
                    };
                    latest = Some((
                        kept_index,
                        CompactionSummaryView {
                            entry_id: entry.id.clone(),
                            summary: summary.to_owned(),
                            first_kept_entry_id: first_kept.to_owned(),
                            tokens_before,
                            timestamp: timestamp.to_owned(),
                            source_entry_ids,
                        },
                    ));
                }
                _ => {
                    return Err(CompactionSourceError::UnsupportedContextEntry {
                        id: entry.id.clone(),
                        kind: entry.kind.clone(),
                    }
                    .into());
                }
            }
        }

        let mut items = Vec::new();
        let start = if let Some((kept_index, summary)) = &latest {
            items.push(CompactedContextItem::Summary(summary.clone()));
            *kept_index
        } else {
            0
        };
        for entry in &branch[start..] {
            if matches!(entry.kind.as_str(), "message" | "custom_message")
                && let Some(message) = entry.message()
            {
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
    pub fn build_context(&self) -> Vec<Message> {
        self.branch().into_iter().filter_map(Entry::message).collect()
    }

    /// Append a soft-compaction summary. The summarized prefix stays raw in
    /// the journal; readers must use [`model_context`](Self::model_context)
    /// so the prefix is replaced by this summary. `first_kept_entry_id` must
    /// be the user message where raw context resumes, and `source_entry_ids`
    /// the summarized entry IDs for provenance.
    pub fn append_compaction(
        &mut self,
        summary: &str,
        first_kept_entry_id: &str,
        source_entry_ids: &[String],
        tokens_before: u64,
    ) -> Result<String> {
        let mut f = serde_json::Map::new();
        f.insert("method".into(), json!("soft"));
        f.insert("summary".into(), json!(summary));
        f.insert("firstKeptEntryId".into(), json!(first_kept_entry_id));
        f.insert("sourceEntryIds".into(), json!(source_entry_ids));
        f.insert("tokensBefore".into(), json!(tokens_before));
        self.append_raw("compaction", f)
    }

    /// Model-visible messages honoring the latest valid soft summary: the
    /// summarized prefix is replaced by the summary text (sent as a user
    /// message so the model treats it as lower-trust prior context), kept and
    /// later messages stay raw. Falls back to [`build_context`](Self::build_context)
    /// when no valid compaction entry exists.
    pub fn model_context(&self) -> Vec<Message> {
        let projection = match self.compacted_context_projection() {
            Ok(p) => p,
            Err(_) => return self.build_context(),
        };
        if !projection.items.iter().any(|i| matches!(i, CompactedContextItem::Summary(_))) {
            return self.build_context();
        }
        projection
            .items
            .into_iter()
            .map(|item| match item {
                CompactedContextItem::Summary(s) => Message::User(UserMessage::text(format!(
                    "[Compacted summary of earlier turns; source entries withheld]\n{}",
                    s.summary
                ))),
                CompactedContextItem::Message(m) => m.message.clone(),
            })
            .collect()
    }

    pub fn undecodable_messages(&self) -> usize {
        self.branch()
            .into_iter()
            .filter(|e| (e.kind == "message" || e.is_user_skill_prompt_candidate()) && !e.is_decodable_message())
            .count()
    }

    /// Last `model_change` on the branch.
    pub fn current_model(&self) -> Option<String> {
        self.branch()
            .into_iter()
            .rev()
            .find(|e| e.kind == "model_change")
            .and_then(|e| e.raw.get("model").and_then(Value::as_str).map(str::to_string))
    }

    /// Pair tool calls that never got a recorded result with explicit
    /// "effect unknown" error results. Works on the raw journal so an entry ARA
    /// cannot decode never hides a call or result. Only calls of the last
    /// assistant turn whose followers are all tool results or decodable Bash
    /// receipts excluded from model context can be paired
    /// adjacently; earlier gaps are reported, never replayed.
    pub fn recover_interrupted_tool_calls(&mut self) -> Result<Recovery> {
        let branch: Vec<Entry> = self.branch().into_iter().cloned().collect();
        let mut recovery = Recovery::default();
        let mut answered: HashSet<String> = HashSet::new();
        let mut last_assistant: Option<usize> = None;
        for (i, e) in branch.iter().enumerate() {
            match e.role() {
                Some("toolResult") => {
                    if let Some(id) = e.raw.pointer("/message/toolCallId").and_then(Value::as_str) {
                        answered.insert(id.to_string());
                    }
                }
                Some("assistant") => last_assistant = Some(i),
                _ => {}
            }
        }
        let calls_of = |e: &Entry| -> Vec<(String, String)> {
            e.raw
                .pointer("/message/content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("toolCall"))
                .filter_map(|b| Some((b.get("id")?.as_str()?.to_string(), b.get("name")?.as_str()?.to_string())))
                .collect()
        };
        for (i, e) in branch.iter().enumerate() {
            if e.role() != Some("assistant") {
                continue;
            }
            let missing: Vec<(String, String)> =
                calls_of(e).into_iter().filter(|(id, _)| !answered.contains(id)).collect();
            if missing.is_empty() {
                continue;
            }
            let tail_is_results_only = Some(i) == last_assistant
                && branch[i + 1..].iter().all(|f| {
                    (f.kind != "message" || f.role() == Some("toolResult") || f.is_excluded_bash_execution())
                        && !f.is_user_skill_prompt_candidate()
                });
            if !tail_is_results_only {
                recovery.unpaired_earlier.extend(missing.into_iter().map(|(id, _)| id));
                continue;
            }
            for (id, name) in missing {
                let result = ToolResultMessage {
                    tool_call_id: id.clone(),
                    tool_name: name,
                    content: vec![UserBlock::text(UNKNOWN_EFFECT_TEXT)],
                    details: Some(
                        json!({"__synthetic": true, "source": "interrupted_unknown_effect", "executed": "unknown"}),
                    ),
                    is_error: true,
                    timestamp: now_ms(),
                };
                self.append_message(&Message::ToolResult(result))?;
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
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else { continue };
        if best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, p));
        }
    }
    best.map(|(_, p)| p)
}
