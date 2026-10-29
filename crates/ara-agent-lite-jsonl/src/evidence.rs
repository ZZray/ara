use crate::protocol::Reference;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn read_reference(path: &Path) -> Result<(Reference, Vec<u8>)> {
    let path = path.canonicalize()?;
    let mut file = File::open(&path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok((Reference { path, sha256: hash(&bytes) }, bytes))
}

pub fn write_new(path: &Path, value: &Value) -> Result<Reference> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    file.write_all(&bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    File::open(path.parent().context("evidence_parent_missing")?)?.sync_all()?;
    Ok(Reference { path: path.canonicalize()?, sha256: hash(&bytes) })
}

pub fn checked_candidate(candidate: &ara_agent_lite_harness::Candidate) -> Result<Reference> {
    let (reference, bytes) = read_reference(&candidate_path(candidate)?)?;
    let stored: ara_agent_lite_harness::Candidate = serde_json::from_slice(&bytes)?;
    let expected = serde_json::to_value(candidate)?;
    ensure!(
        serde_json::to_value(stored)? == expected && serde_json::from_slice::<Value>(&bytes)? == expected,
        "candidate_reference_content_mismatch"
    );
    Ok(reference)
}

fn bounded_reference(path: &Path) -> Result<(Reference, Vec<u8>)> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= 16 * 1024 * 1024,
        "budget_reference_not_bounded_file"
    );
    let path = path.canonicalize()?;
    let mut bytes = Vec::new();
    File::open(&path)?.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 16 * 1024 * 1024, "budget_reference_grew");
    Ok((Reference { path, sha256: hash(&bytes) }, bytes))
}

pub fn checked_value(path: &Path, expected: &Value) -> Result<Reference> {
    let (reference, bytes) = bounded_reference(path)?;
    ensure!(serde_json::from_slice::<Value>(&bytes)? == *expected, "budget_reference_content_mismatch");
    Ok(reference)
}

pub fn checked_budget_snapshot(snapshot: &ara_agent_lite_harness::BudgetStopSnapshot) -> Result<Reference> {
    ensure!(snapshot.termination.is_reviewable(), "budget_termination_not_reviewable");
    let directory = snapshot.basis.events_path.parent().context("budget_snapshot_parent_missing")?;
    let path = ara_agent_lite_harness::budget_snapshot_path(directory, snapshot.basis.run_number);
    let (reference, bytes) = bounded_reference(&path)?;
    let stored: ara_agent_lite_harness::BudgetStopSnapshot = serde_json::from_slice(&bytes)?;
    let expected = serde_json::to_value(snapshot)?;
    ensure!(
        serde_json::to_value(stored)? == expected && serde_json::from_slice::<Value>(&bytes)? == expected,
        "budget_snapshot_content_mismatch"
    );
    Ok(reference)
}

/// The Harness-persisted `run-N-stop-snapshot.json` still equals the reviewed snapshot.
pub fn checked_stop_snapshot(snapshot: &ara_agent_lite_harness::StopSnapshot) -> Result<Reference> {
    let directory = snapshot.basis.events_path.parent().context("stop_snapshot_parent_missing")?;
    let path = ara_agent_lite_harness::stop_snapshot_path(directory, snapshot.basis.run_number);
    let (reference, bytes) = bounded_reference(&path)?;
    let stored: ara_agent_lite_harness::StopSnapshot = serde_json::from_slice(&bytes)?;
    let expected = serde_json::to_value(snapshot)?;
    ensure!(
        serde_json::to_value(stored)? == expected && serde_json::from_slice::<Value>(&bytes)? == expected,
        "stop_snapshot_content_mismatch"
    );
    Ok(reference)
}

/// A trusted index is correlated with the exact current receipt, never a scan
/// for an older successful Run. A missing index can represent a rejected review.
pub fn checked_budget_review(directory: &Path, receipt: &ara_agent_lite_harness::ReviewReceipt) -> Result<Reference> {
    let path = ara_agent_lite_harness::budget_review_index_path(directory, receipt.basis.run_number);
    let (reference, bytes) = bounded_reference(&path)?;
    let record: ara_agent_lite_harness::BudgetStopReviewRecord = serde_json::from_slice(&bytes)?;
    ensure!(record.version == 1 && record.kind == "closed_budget_snapshot_review", "budget_review_index_kind");
    ensure!(
        serde_json::from_slice::<Value>(&bytes)? == serde_json::to_value(&record)?,
        "budget_review_index_extra_fields"
    );
    ensure!(serde_json::to_value(&record.receipt)? == serde_json::to_value(receipt)?, "budget_review_receipt_mismatch");
    let (snapshot_reference, snapshot_bytes) = bounded_reference(&record.snapshot.path)?;
    ensure!(snapshot_reference.sha256 == record.snapshot.sha256, "budget_review_snapshot_hash_mismatch");
    let snapshot: ara_agent_lite_harness::BudgetStopSnapshot = serde_json::from_slice(&snapshot_bytes)?;
    ensure!(snapshot.basis == receipt.basis, "budget_review_snapshot_basis_mismatch");
    let expected = checked_budget_snapshot(&snapshot)?;
    ensure!(
        expected.path == snapshot_reference.path && expected.sha256 == snapshot_reference.sha256,
        "budget_review_snapshot_path_mismatch"
    );
    ensure!(
        receipt.committed
            && receipt.settlement == ara_agent_lite_harness::Settlement::Settled
            && receipt.verdict != ara_agent_lite_harness::Verdict::Unknown,
        "budget_review_not_committed_settled"
    );
    Ok(reference)
}

pub fn rpc_path(directory: &Path, rpc: &str, suffix: &str) -> PathBuf {
    directory.join(format!("jsonl-{rpc}-{suffix}.json"))
}

pub fn candidate_path(candidate: &ara_agent_lite_harness::Candidate) -> Result<PathBuf> {
    Ok(candidate
        .basis
        .events_path
        .parent()
        .context("candidate_parent_missing")?
        .join(format!("run-{}-candidate.json", candidate.basis.run_number)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_agent_lite_harness::{Candidate, ReviewBasis};
    use serde_json::json;

    struct Fixture {
        directory: PathBuf,
        candidate: Candidate,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "ara-jsonl-candidate-{}-{}",
                std::process::id(),
                ara_agent_lite_harness::core::fresh_id()
            ));
            std::fs::create_dir(&directory).unwrap();
            let candidate = Candidate {
                basis: ReviewBasis {
                    task_id: "task-fixture".into(),
                    criteria_revision: "criteria-fixture".into(),
                    authorization_revision: "grant-fixture".into(),
                    run_id: "run-fixture".into(),
                    run_number: 1,
                    session_path: directory.join("session.json"),
                    events_path: directory.join("run-1-events.jsonl"),
                    events_sha256: "events-fixture".into(),
                    event_count: 4,
                    run_calls: 1,
                    total_calls: 1,
                    journal_head_sha256: "head-fixture".into(),
                    raw_start: 0,
                    raw_end: 2,
                    raw_proof: "proof-fixture".into(),
                },
                output: "actual candidate output".into(),
                goal: None,
            };
            Self { directory, candidate }
        }

        fn save(&self, value: Value) -> Vec<u8> {
            let mut bytes = serde_json::to_vec(&value).unwrap();
            bytes.push(b'\n');
            std::fs::write(candidate_path(&self.candidate).unwrap(), &bytes).unwrap();
            bytes
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only the single file and new empty directory owned by this test;
            // no recursive cleanup and no product/session files are involved.
            if let Ok(path) = candidate_path(&self.candidate) {
                let _ = std::fs::remove_file(path);
            }
            let _ = std::fs::remove_dir(&self.directory);
        }
    }

    #[test]
    fn candidate_reference_accepts_exact_file_and_hashes_the_same_bytes() {
        let fixture = Fixture::new();
        let bytes = fixture.save(serde_json::to_value(&fixture.candidate).unwrap());
        let reference = checked_candidate(&fixture.candidate).unwrap();
        assert_eq!(reference.path, candidate_path(&fixture.candidate).unwrap().canonicalize().unwrap());
        assert_eq!(reference.sha256, hash(&bytes));
        assert_eq!(std::fs::read(reference.path).unwrap(), bytes);
    }

    #[test]
    fn candidate_reference_refuses_changed_output_with_unchanged_basis() {
        let fixture = Fixture::new();
        let mut changed = serde_json::to_value(&fixture.candidate).unwrap();
        changed["output"] = json!("different disk output");
        assert_eq!(changed["basis"], serde_json::to_value(&fixture.candidate.basis).unwrap());
        fixture.save(changed);
        assert!(checked_candidate(&fixture.candidate).is_err());
    }

    #[test]
    fn candidate_reference_refuses_extra_raw_fields() {
        let fixture = Fixture::new();
        let mut changed = serde_json::to_value(&fixture.candidate).unwrap();
        changed["unexpected"] = json!("not part of the supplied candidate");
        fixture.save(changed);
        assert!(checked_candidate(&fixture.candidate).is_err());
    }

    fn budget_snapshot(fixture: &Fixture) -> ara_agent_lite_harness::BudgetStopSnapshot {
        ara_agent_lite_harness::BudgetStopSnapshot {
            basis: fixture.candidate.basis.clone(),
            diagnostic_output: "failed Run diagnostic, not a model final response".into(),
            goal: None,
            termination: serde_json::from_value(json!({"version":1,"cause":"call_budget_exhausted",
                "native_round_closed":true,"model_final_stop":false}))
            .unwrap(),
        }
    }

    #[test]
    fn budget_snapshot_checker_refuses_content_mutation_and_extended_raw_fields() {
        let fixture = Fixture::new();
        let snapshot = budget_snapshot(&fixture);
        let path = ara_agent_lite_harness::budget_snapshot_path(&fixture.directory, 1);
        let expected = serde_json::to_value(&snapshot).unwrap();
        let reference = write_new(&path, &expected).unwrap();
        assert_eq!(checked_budget_snapshot(&snapshot).unwrap().sha256, reference.sha256);
        let mut changed = expected.clone();
        changed["diagnostic_output"] = json!("changed");
        std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(checked_budget_snapshot(&snapshot).is_err());
        changed = expected;
        changed["effects_settled"] = json!(true);
        std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(checked_budget_snapshot(&snapshot).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn budget_review_index_is_bound_to_the_exact_receipt_and_snapshot() {
        use ara_agent_lite_harness::{
            BudgetStopReviewRecord, BudgetStopSnapshotReference, ReviewReceipt, Settlement, Verdict,
        };
        let fixture = Fixture::new();
        let snapshot = budget_snapshot(&fixture);
        let path = ara_agent_lite_harness::budget_snapshot_path(&fixture.directory, 1);
        let reference = write_new(&path, &serde_json::to_value(&snapshot).unwrap()).unwrap();
        let receipt = ReviewReceipt {
            basis: snapshot.basis.clone(),
            committed: true,
            settlement: Settlement::Settled,
            verdict: Verdict::Verified,
            evidence: vec!["host-owned actual file check".into()],
        };
        let record = BudgetStopReviewRecord {
            version: 1,
            kind: "closed_budget_snapshot_review".into(),
            snapshot: BudgetStopSnapshotReference { path: reference.path, sha256: reference.sha256 },
            receipt: receipt.clone(),
        };
        let index = ara_agent_lite_harness::budget_review_index_path(&fixture.directory, 1);
        write_new(&index, &serde_json::to_value(&record).unwrap()).unwrap();
        assert!(checked_budget_review(&fixture.directory, &receipt).is_ok());
        let mut changed = receipt.clone();
        changed.basis.authorization_revision = "another authority".into();
        assert!(checked_budget_review(&fixture.directory, &changed).is_err());
        let mut body = serde_json::to_value(&snapshot).unwrap();
        body["termination"]["model_final_stop"] = json!(true);
        std::fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();
        assert!(checked_budget_review(&fixture.directory, &receipt).is_err());
        std::fs::remove_file(index).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn budget_continuation_view_requires_the_complete_original_payload() {
        let fixture = Fixture::new();
        let path = fixture.directory.join("jsonl-budget-continuation-2.json");
        let payload = json!({"remaining_calls":1,"deadline_unix_ms":1234});
        let reference = write_new(&path, &payload).unwrap();
        assert_eq!(checked_value(&path, &payload).unwrap().sha256, reference.sha256);
        std::fs::write(&path, b"{\"remaining_calls\":99,\"deadline_unix_ms\":1234}").unwrap();
        assert!(checked_value(&path, &payload).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
