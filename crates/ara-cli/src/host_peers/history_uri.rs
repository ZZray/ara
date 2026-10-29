//! Read-only discovery index from OMP b91db66e history-protocol.ts:482-509
//! and registry-helpers.ts. Copyright (c) 2025 Mario Zechner;
//! (c) 2025-2026 Can Bölük; (c) 2026 Stencil Labs, Inc. MIT.
//! See THIRD_PARTY_NOTICES.md. Transcript/current/full routes remain unported.
//!
//! ARA Host adaptation: discovery is confined to the current root namespace,
//! rather than OMP's process-global registry. Disk IDs come from SessionJournal,
//! never filename stems, and parent provenance must reach that root journal.

use super::{PeerCoordinator, PeerKind, PeerRecord, PeerRoster, PeerStatus, now_ms};
use ara_agent::ToolError;
use ara_session::{SessionError, SessionJournal};
use ara_tools::{ContentUriPort, ContentUriRoute, UriFileResource, UriResource};
use async_trait::async_trait;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

// Same per-artifact-directory bound as fixed registry-helpers.ts.
const MAX_SCAN_DEPTH: usize = 8;

pub struct HistoryIndexPort {
    coordinator: Arc<PeerCoordinator>,
    root: String,
    session_file: Option<PathBuf>,
}

impl HistoryIndexPort {
    pub fn new(
        coordinator: Arc<PeerCoordinator>,
        root: String,
        session_file: Option<PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self { coordinator, root, session_file })
    }

    pub fn prompt_doc() -> &'static str {
        "Read history:// to list agents in this session's root namespace. The index includes registered agents and discoverable on-disk sessions. Only the bare index is supported; history://<id>, history://current and history://current/full are unavailable."
    }

    async fn render(&self, cancel: CancellationToken) -> Result<UriResource, ToolError> {
        check_cancel(&cancel)?;
        // Refresh the actual caller's root before snapshotting its registered refs.
        // If no caller journal exists, only an explicitly registered Main journal
        // can supply disk authority; a child ref never invents a caller root.
        let hint = self.session_file.clone().or_else(|| {
            self.coordinator.registry.list(&self.root).into_iter()
                .find(|record| record.kind == PeerKind::Main)
                .and_then(|record| record.session_file)
        });
        let root_file = match hint {
            Some(file) => Some(self.coordinator.roster.ensure(&self.root, &file).await.map_err(error)?),
            None => None,
        };
        check_cancel(&cancel)?;
        let refs = self.coordinator.registry.list(&self.root).into_iter()
            .filter(|record| record.kind != PeerKind::Advisor)
            .collect::<Vec<_>>();
        let disk = match root_file {
            Some(file) => {
                let sources = refs.iter().filter_map(|record| record.session_file.clone()).collect();
                let scan_cancel = cancel.clone();
                // SessionJournal::open is read-only, but synchronous. Keep its
                // filesystem work off the async executor; cancellation never
                // appends, rewrites, registers, or replays a transcript.
                tokio::task::spawn_blocking(move || disk_ids(file, sources, &scan_cancel))
                    .await.map_err(error)??
            }
            None => Vec::new(),
        };
        check_cancel(&cancel)?;
        let registered = refs.iter().map(|record| record.key.id.as_str()).collect::<HashSet<_>>();
        let mut lines = vec!["# Agents".to_owned(), String::new()];
        if refs.is_empty() && disk.is_empty() {
            lines.push("No agents registered.".into());
        } else {
            lines.push("| id | status | kind | parent | last activity |".into());
            lines.push("|---|---|---|---|---|".into());
            let now = now_ms();
            for record in &refs {
                lines.push(registered_row(record, now));
            }
            for id in disk {
                if !registered.contains(id.as_str()) {
                    lines.push(format!("| {id} | on disk | — | — | — |"));
                }
            }
            // The upstream transcript link would promise an unported route.
            lines.push(String::new());
            lines.push("This Host supports the `history://` index only; transcript routes are unavailable.".into());
        }
        Ok(UriResource {
            content: lines.join("\n") + "\n",
            content_type: "text/markdown".into(),
            notes: Vec::new(),
            immutable: false,
        })
    }
}

fn registered_row(record: &PeerRecord, now: u64) -> String {
    let status = match record.status {
        PeerStatus::Running => "running",
        PeerStatus::Idle => "idle",
        PeerStatus::Parked => "parked",
        PeerStatus::Aborted => "aborted",
    };
    let kind = match record.kind {
        PeerKind::Main => "main",
        PeerKind::Sub => "sub",
        PeerKind::Advisor => "advisor",
    };
    format!("| {} | {status} | {kind} | {} | {} |", record.key.id,
        record.parent_id.as_deref().unwrap_or("—"), format_ago(now, record.last_activity))
}

fn format_ago(now: u64, timestamp: u64) -> String {
    let seconds = now.saturating_sub(timestamp) / 1_000;
    if seconds < 60 { return format!("{seconds}s ago"); }
    let minutes = seconds / 60;
    if minutes < 60 { return format!("{minutes}m ago"); }
    let hours = minutes / 60;
    if hours < 24 { return format!("{hours}h ago"); }
    format!("{}d ago", hours / 24)
}

#[derive(Clone)]
struct JournalIdentity {
    id: String,
    parent_file: Option<PathBuf>,
    parent_id: Option<String>,
}

fn identity(
    file: &Path,
    cache: &mut HashMap<PathBuf, Option<JournalIdentity>>,
) -> Result<Option<JournalIdentity>, ToolError> {
    if let Some(cached) = cache.get(file) { return Ok(cached.clone()); }
    let result = match SessionJournal::open(file) {
        Ok(journal) => {
            let id = journal.session_id();
            if id.is_empty() || matches!(id, "." | "..") || id.contains(['/', '\\']) {
                None
            } else {
                Some(JournalIdentity {
                    id: id.to_owned(),
                    parent_file: journal.header().get("parentSession")
                        .and_then(serde_json::Value::as_str).map(PathBuf::from),
                    parent_id: journal.entries().iter().rev()
                        .filter(|entry| entry.kind == "session_init")
                        .find_map(|entry| entry.raw.get("araChildParentId")
                            .and_then(serde_json::Value::as_str).map(str::to_owned)),
                })
            }
        }
        Err(SessionError::Corrupt { .. }) => None,
        Err(SessionError::Io(fault)) if missing(&fault) => None,
        Err(fault) => return Err(error(fault)),
    };
    cache.insert(file.to_path_buf(), result.clone());
    Ok(result)
}

fn canonical(file: &Path) -> Result<Option<PathBuf>, ToolError> {
    match std::fs::canonicalize(file) {
        Ok(file) => Ok(Some(file)),
        Err(fault) if missing(&fault) => Ok(None),
        Err(fault) => Err(error(fault)),
    }
}

fn missing(fault: &std::io::Error) -> bool {
    matches!(fault.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory)
}

fn excluded(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        name.starts_with("__advisor") || name.contains(".bak")
    })
}

// No registry mutation: even incomplete, valid-header journals may be listed as
// disk-only, but only when every retained parent receipt reaches the caller root.
fn proven_id(
    file: &Path,
    root: &Path,
    cache: &mut HashMap<PathBuf, Option<JournalIdentity>>,
    cancel: &CancellationToken,
) -> Result<Option<String>, ToolError> {
    let mut current = file.to_path_buf();
    let mut seen = HashSet::new();
    let mut result = None;
    loop {
        check_cancel(cancel)?;
        if !PeerRoster::belongs_to_root(&current, root) || !seen.insert(current.clone()) || excluded(&current) {
            return Ok(None);
        }
        let Some(meta) = identity(&current, cache)? else { return Ok(None); };
        if result.is_none() { result = Some(meta.id.clone()); }
        if current == root { return Ok(result); }
        let Some(parent) = meta.parent_file else { return Ok(None); };
        let Some(parent) = canonical(&parent)? else { return Ok(None); };
        if !PeerRoster::belongs_to_root(&parent, root) || excluded(&parent) { return Ok(None); }
        let Some(parent_meta) = identity(&parent, cache)? else { return Ok(None); };
        if meta.parent_id.as_deref().is_some_and(|id| id != parent_meta.id.as_str()) { return Ok(None); }
        current = parent;
    }
}

fn disk_ids(
    root_file: PathBuf,
    session_files: Vec<PathBuf>,
    cancel: &CancellationToken,
) -> Result<Vec<String>, ToolError> {
    check_cancel(cancel)?;
    let Some(root_file) = canonical(&root_file)? else { return Ok(Vec::new()); };
    let artifact_root = root_file.with_extension("");
    let mut dirs = vec![artifact_root.clone()];
    for file in session_files {
        let Some(file) = canonical(&file)? else { continue; };
        if !PeerRoster::belongs_to_root(&file, &root_file) { continue; }
        // The adopted artifact directory and each child write-time directory
        // are the same two sources as registry-helpers, confined to this root.
        if let Some(parent) = file.parent() {
            if parent.starts_with(&artifact_root) && !dirs.iter().any(|dir| dir.as_path() == parent) {
                dirs.push(parent.to_path_buf());
            }
        }
        let own = file.with_extension("");
        if !dirs.contains(&own) { dirs.push(own); }
    }
    let mut cache = HashMap::new();
    let mut seen_dirs = HashSet::new();
    let mut seen_ids = HashSet::new();
    let mut found = Vec::new();
    for dir in dirs {
        // Caller directory first; each listing is walked depth-first before
        // later sources. First actual journal ID wins across all sources.
        check_cancel(cancel)?;
        if excluded(&dir) { continue; }
        let Some(actual) = canonical(&dir)? else { continue; };
        if actual != dir || !actual.starts_with(&artifact_root) || !seen_dirs.insert(actual.clone()) {
            continue;
        }
        let entries = match std::fs::read_dir(&actual) {
            Ok(entries) => entries.collect::<std::io::Result<Vec<_>>>().map_err(error)?,
            Err(fault) if missing(&fault) => continue,
            Err(fault) => return Err(error(fault)),
        };
        walk_entries(entries, 0, &root_file, &artifact_root, cancel,
            &mut seen_dirs, &mut cache, &mut seen_ids, &mut found)?;
    }
    Ok(found)
}

#[allow(clippy::too_many_arguments)]
fn walk_entries(
    entries: Vec<std::fs::DirEntry>,
    depth: usize,
    root: &Path,
    artifact_root: &Path,
    cancel: &CancellationToken,
    seen_dirs: &mut HashSet<PathBuf>,
    cache: &mut HashMap<PathBuf, Option<JournalIdentity>>,
    seen_ids: &mut HashSet<String>,
    found: &mut Vec<String>,
) -> Result<(), ToolError> {
    for entry in entries {
        check_cancel(cancel)?;
        let path = entry.path();
        if excluded(&path) { continue; }
        let kind = entry.file_type().map_err(error)?;
        if kind.is_dir() && depth < MAX_SCAN_DEPTH {
            let Some(actual) = canonical(&path)? else { continue; };
            if actual != path || !actual.starts_with(artifact_root) || !seen_dirs.insert(actual.clone()) {
                continue;
            }
            let entries = match std::fs::read_dir(actual) {
                Ok(entries) => entries.collect::<std::io::Result<Vec<_>>>().map_err(error)?,
                Err(fault) if missing(&fault) => continue,
                Err(fault) => return Err(error(fault)),
            };
            walk_entries(entries, depth + 1, root, artifact_root, cancel,
                seen_dirs, cache, seen_ids, found)?;
        } else if kind.is_file() && path.extension().is_some_and(|extension| extension == "jsonl") {
            let Some(actual) = canonical(&path)? else { continue; };
            if !actual.starts_with(artifact_root) { continue; }
            if let Some(id) = proven_id(&actual, root, cache, cancel)? {
                if seen_ids.insert(id.clone()) { found.push(id); }
            }
        }
    }
    Ok(())
}

fn error(value: impl ToString) -> ToolError { ToolError(value.to_string()) }
fn check_cancel(cancel: &CancellationToken) -> Result<(), ToolError> {
    if cancel.is_cancelled() { Err(error("history index read cancelled")) } else { Ok(()) }
}
fn require_index(url: &str) -> Result<(), ToolError> {
    match url.split_once("://") {
        Some((scheme, "")) if scheme.eq_ignore_ascii_case("history") => Ok(()),
        Some((scheme, _)) if scheme.eq_ignore_ascii_case("history") => Err(error(
            "Only the bare history:// index is supported; transcript/current/full routes are unavailable.")),
        _ => Err(error("Expected history://")),
    }
}

#[async_trait]
impl ContentUriPort for HistoryIndexPort {
    fn route(&self, url: &str) -> ContentUriRoute {
        if url.split_once("://").is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("history")) {
            ContentUriRoute::Registered { writable: false }
        } else {
            ContentUriRoute::Unregistered
        }
    }

    async fn read_file(&self, url: &str, cancel: CancellationToken) -> Result<Option<UriFileResource>, ToolError> {
        check_cancel(&cancel)?;
        require_index(url)?;
        Ok(None)
    }

    async fn read(&self, url: &str, cancel: CancellationToken) -> Result<UriResource, ToolError> {
        check_cancel(&cancel)?;
        require_index(url)?;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(error("history index read cancelled")),
            result = self.render(cancel.clone()) => result,
        }
    }

    async fn write(&self, _url: &str, _content: &str, cancel: CancellationToken) -> Result<(), ToolError> {
        check_cancel(&cancel)?;
        Err(error("history:// is read-only; only its discovery index is supported."))
    }
}
