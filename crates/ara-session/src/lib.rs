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
//! rewrites are atomic with a directory sync; a failed append is rolled back
//! so `Err` always means "not recorded"; tool calls interrupted before their
//! results were recorded are paired with explicit "effect unknown" results on
//! resume instead of being replayed.
//!
//! Not ported (open): compaction/branch-summary entries in context building,
//! labels, custom-entry semantics, v1/v2 migrations, SQL/Redis storage,
//! listing/search, forking, moving, title generation, blob externalization.

use ara_ai::{Message, ToolResultMessage, UserBlock, now_ms};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CURRENT_SESSION_VERSION: u64 = 3;
pub const SESSION_TITLE_SLOT_BYTES: usize = 256;
pub const CORRUPT_HEADER_MESSAGE: &str = "session header is missing or malformed";
pub const UNKNOWN_EFFECT_TEXT: &str = "Tool call was interrupted before its result was recorded; its effects are unknown. Inspect the affected state before retrying it.";

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
    /// Decoded message, or `None` for non-message entries and messages ARA cannot decode.
    pub fn message(&self) -> Option<Message> {
        if self.kind != "message" {
            return None;
        }
        serde_json::from_value(self.raw.get("message")?.clone()).ok()
    }

    fn role(&self) -> Option<&str> {
        if self.kind != "message" {
            return None;
        }
        self.raw.pointer("/message/role").and_then(Value::as_str)
    }
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
    header: Value,
    title: TitleSlot,
    entries: Vec<Entry>,
    ids: HashSet<String>,
    leaf: Option<String>,
    materialized: bool,
    rewrite_required: bool,
    pending_backup: bool,
    pub report: LoadReport,
}

impl SessionJournal {
    /// New lazy session in `session_dir` for `cwd` (file created on first assistant message).
    pub fn create(session_dir: &Path, cwd: &Path) -> Result<SessionJournal> {
        fs::create_dir_all(session_dir)?;
        let id = uuid::Uuid::now_v7().to_string();
        let timestamp = now_iso();
        let path = session_dir.join(format!("{}_{}.jsonl", file_safe_timestamp(&timestamp), id));
        let header = json!({
            "type": "session",
            "version": CURRENT_SESSION_VERSION,
            "id": id,
            "timestamp": timestamp,
            "cwd": cwd.to_string_lossy(),
        });
        Ok(SessionJournal {
            path,
            title: TitleSlot { updated_at: timestamp, ..Default::default() },
            header,
            entries: Vec::new(),
            ids: HashSet::new(),
            leaf: None,
            materialized: false,
            rewrite_required: false,
            pending_backup: false,
            report: LoadReport::default(),
        })
    }

    /// Load an existing session. Malformed records are skipped (the file is
    /// rewritten, after a backup, before the next append). A missing, corrupt
    /// or unsupported-version header is an error that leaves the file untouched.
    pub fn open(path: &Path) -> Result<SessionJournal> {
        let corrupt = |message: String| SessionError::Corrupt { path: path.into(), message };
        let bytes = fs::read(path)?;
        let text = String::from_utf8_lossy(&bytes);
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
            updated_at: header.get("timestamp").and_then(Value::as_str).unwrap_or_default().into(),
            ..Default::default()
        });
        let damaged = malformed > 0 || bytes.last().is_some_and(|b| *b != b'\n');
        let leaf = entries.last().map(|e| e.id.clone());
        Ok(SessionJournal {
            path: path.into(),
            header,
            title,
            entries,
            ids,
            leaf,
            materialized: true,
            rewrite_required: damaged,
            pending_backup: damaged,
            report: LoadReport { malformed_records: malformed, backup: None },
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
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
    /// fsync, rename, directory fsync.
    fn rewrite(&mut self) -> Result<()> {
        let dir = self.dir();
        fs::create_dir_all(&dir)?;
        if self.pending_backup && self.path.exists() {
            let backup = self.path.with_extension(format!("jsonl.torn-{}.bak", now_ms()));
            fs::copy(&self.path, &backup)?;
            File::open(&backup)?.sync_all()?;
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
        let by_id: HashMap<&str, &Entry> = self.entries.iter().map(|e| (e.id.as_str(), e)).collect();
        let mut out = Vec::new();
        let mut cur = self.leaf.as_deref();
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

    /// Model-visible messages on the current branch (subset of OMP
    /// `buildSessionContext`). Messages ARA cannot decode are skipped and
    /// counted by [`undecodable_messages`](Self::undecodable_messages).
    pub fn build_context(&self) -> Vec<Message> {
        self.branch().into_iter().filter_map(Entry::message).collect()
    }

    pub fn undecodable_messages(&self) -> usize {
        self.branch().into_iter().filter(|e| e.kind == "message" && e.message().is_none()).count()
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
    /// assistant turn whose followers are all tool results can be paired
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
                && branch[i + 1..].iter().all(|f| f.kind != "message" || f.role() == Some("toolResult"));
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
