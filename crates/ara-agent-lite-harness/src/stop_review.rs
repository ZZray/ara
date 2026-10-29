//! Persistence of Length / NoProgress stop snapshots (design “P2-a 任务级长度截断续写”):
//! `run-N-stop-snapshot.json`, `run-N-stop-review.json`, `run-N-stop-review-index.json`.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Result};
use serde_json::Value;

use crate::{storage, StopSnapshot, StopSnapshotReference};

const SNAPSHOT_BYTES: u64 = 8 * 1024 * 1024;

pub fn stop_snapshot_path(evidence_dir: &Path, run_number: usize) -> PathBuf {
    evidence_dir.join(format!("run-{run_number}-stop-snapshot.json"))
}

pub fn stop_review_index_path(evidence_dir: &Path, run_number: usize) -> PathBuf {
    evidence_dir.join(format!("run-{run_number}-stop-review-index.json"))
}

pub(crate) fn persist(snapshot: &StopSnapshot, evidence_dir: &Path) -> Result<StopSnapshotReference> {
    let path = stop_snapshot_path(evidence_dir, snapshot.basis.run_number);
    storage::write_new(&path, snapshot)?;
    let reference = StopSnapshotReference { sha256: storage::file_hash(&path)?, path };
    check(snapshot, &reference)?;
    Ok(reference)
}

/// The persisted bytes still equal the reviewed snapshot, and the truncated
/// message (when present) is byte-identical in the journal (16.3).
pub(crate) fn check(snapshot: &StopSnapshot, reference: &StopSnapshotReference) -> Result<()> {
    let evidence = snapshot.basis.events_path.parent().context("snapshot evidence parent missing")?;
    ensure!(reference.path == stop_snapshot_path(evidence, snapshot.basis.run_number), "stop snapshot path changed");
    let mut bytes = Vec::new();
    File::open(&reference.path)?.take(SNAPSHOT_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= SNAPSHOT_BYTES, "stop snapshot exceeds byte limit");
    ensure!(storage::hash(&bytes) == reference.sha256, "stop snapshot bytes changed");
    let raw: Value = serde_json::from_slice(&bytes)?;
    ensure!(raw == serde_json::to_value(snapshot)?, "stop snapshot differs from reviewed source");
    if let Some(truncated) = &snapshot.truncated {
        let journal = ara_agent_lite_core::session_journal::Journal::for_session(&snapshot.basis.session_path);
        ensure!(
            journal.proof(truncated.raw_index, truncated.raw_index + 1)? == truncated.journal_proof,
            "truncated assistant message changed in the journal"
        );
    }
    Ok(())
}
