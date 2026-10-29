//! Same-prefix Session fork for A/B evaluation (requirement 6, optional).
//!
//! The source Session is only read. Each fork receives the first K raw journal
//! records byte for byte, the source `cwd`, `route` and settled Run IDs (so no
//! source Run ID can run again on a fork), and no pending Run or Goal. Nothing
//! here starts a Run; a failed fork keeps any target it already wrote and
//! reports it.
use crate::session_journal::{self, Journal};
use anyhow::{bail, ensure, Context, Result};
use rpi_agent::AgentMessage;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize)]
pub struct ForkReceipt {
    pub prefix_records: usize,
    /// SHA-256 of the source records 1..=K exact bytes, concatenated in order.
    pub prefix_sha256: String,
    /// Digest over every source Session file, taken before the fork.
    pub source_sha256_before: String,
    pub targets: [PathBuf; 2],
}

/// The fork receipt file of a target Session path.
pub fn receipt_path(target: &Path) -> PathBuf {
    target.with_extension("fork.json")
}

/// Fork `source` (a Session state path) at journal record count `k` into two
/// new Sessions at `targets`.
pub fn fork_session(source: &Path, cwd: &Path, k: usize, targets: [&Path; 2]) -> Result<ForkReceipt> {
    let before = source_digest(source)?;
    if let Some(fence) = harness_fence(source)? {
        bail!("fork refused: source Session has a Harness fence at {}", fence.display());
    }
    let journal = Journal::for_session(source);
    ensure!(journal.exists(), "fork refused: source Session has no journal at {}", source.display());
    let head = journal.inspect()?;
    if let Some(run) = head.metadata.get("pending_run").filter(|value| !value.is_null()) {
        bail!("fork refused: source Session has a pending Run {run}");
    }
    let source_cwd: PathBuf = serde_json::from_value(head.metadata["cwd"].clone()).context("source Session cwd")?;
    ensure!(source_cwd == cwd, "fork refused: source Session working directory does not match");
    ensure!(
        head.active_base == 0,
        "fork refused: records before {} are archived behind a summary; only fully active histories can be forked",
        head.active_base
    );
    ensure!((1..=head.raw_count).contains(&k), "fork point K={k} is outside 1..={}", head.raw_count);
    let records = journal.raw_prefix(&head, k)?;
    let messages = records
        .iter()
        .map(|record| serde_json::from_value::<AgentMessage>(record.message.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    crate::agent::validate_transcript(&messages)
        .with_context(|| format!("fork point K={k} does not end on a complete exchange"))?;

    let source_absolute = std::path::absolute(source)?;
    let absolute = [std::path::absolute(targets[0])?, std::path::absolute(targets[1])?];
    ensure!(
        absolute[0] != absolute[1] && !absolute.contains(&source_absolute),
        "fork targets must differ from each other and from the source"
    );
    for target in &absolute {
        for path in [
            target.clone(),
            session_journal::directory(target),
            receipt_path(target),
            crate::context_memory::path(target),
        ] {
            ensure!(!path.exists(), "fork target already exists: {}", path.display());
        }
    }

    let mut prefix = Sha256::new();
    for record in &records {
        prefix.update(&record.bytes);
    }
    let prefix_sha256 = format!("{:x}", prefix.finalize());
    let settled = journal.settled_runs(&head)?;
    let mut metadata = json!({"version":2,"cwd":cwd,"messages":[],"raw_base":0,"pending_run":null,
        "completed_runs":head.metadata["completed_runs"],"goal":null});
    if let Some(route) = head.metadata.get("route") {
        metadata["route"] = route.clone();
    }
    let mut checkpoint = metadata.clone();
    checkpoint["messages"] = Value::Array(records.iter().map(|record| record.message.clone()).collect());

    let mut written: Vec<PathBuf> = Vec::new();
    for (index, target) in absolute.iter().enumerate() {
        let receipt = json!({"version":1,"source":source_absolute,"sibling":absolute[1 - index],
            "prefix_records":k,"prefix_sha256":prefix_sha256,"source_sha256_before":before});
        let result = Journal::for_session(target)
            .create_fork(&records, metadata.clone(), &settled)
            .and_then(|_| write_new(target, &serde_json::to_vec(&checkpoint)?))
            .and_then(|_| write_new(&receipt_path(target), &serde_json::to_vec(&receipt)?));
        if let Err(error) = result {
            written.push(target.clone());
            return Err(error.context(format!(
                "fork write failed; partial or complete targets kept for inspection, never run: {written:?}"
            )));
        }
        written.push(target.clone());
    }
    let after = source_digest(source)?;
    ensure!(after == before, "source Session changed during fork; targets kept for inspection: {written:?}");
    Ok(ForkReceipt { prefix_records: k, prefix_sha256, source_sha256_before: before, targets: absolute })
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Digest over every file of a Session: the state file, its journal directory
/// and its context memory, as sorted `relative path` / SHA-256 pairs.
pub fn source_digest(source: &Path) -> Result<String> {
    let mut files = Vec::new();
    for root in [source.to_path_buf(), crate::context_memory::path(source)] {
        if root.is_file() {
            files.push(root);
        }
    }
    let mut pending = vec![session_journal::directory(source)];
    while let Some(directory) = pending.pop() {
        if !directory.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut manifest = Sha256::new();
    for file in files {
        let bytes = std::fs::read(&file)?;
        manifest.update(format!("{}\0{:x}\n", file.display(), Sha256::digest(&bytes)).as_bytes());
    }
    Ok(format!("{:x}", manifest.finalize()))
}

/// The Harness fence (`.ara-lite-harness-<sha256(path)>.fence.json`) beside
/// `source`, matched by its file name digest or its recorded `session_path`.
fn harness_fence(source: &Path) -> Result<Option<PathBuf>> {
    let parent = match source.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    if !parent.is_dir() {
        return Ok(None);
    }
    let identity = |path: &Path| {
        let text = path.to_string_lossy().into_owned();
        let text = if cfg!(windows) { text.to_lowercase() } else { text };
        format!("{:x}", Sha256::digest(text.as_bytes()))
    };
    let canonical = source.canonicalize().ok();
    let names = [identity(source), identity(&std::path::absolute(source)?)];
    for entry in std::fs::read_dir(&parent)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
        let Some(digest) = name.strip_prefix(".ara-lite-harness-").and_then(|rest| rest.strip_suffix(".fence.json"))
        else {
            continue;
        };
        if names.iter().any(|known| known == digest) {
            return Ok(Some(path));
        }
        let recorded = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| value["session_path"].as_str().map(PathBuf::from));
        if recorded.is_some_and(|recorded| canonical.is_some() && recorded.canonicalize().ok() == canonical) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}
