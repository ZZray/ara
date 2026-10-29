//! Requirement 6 same-prefix Session fork (optional M-P0-FORK).
//!
//! - Property 14: forks keep the first K raw records byte for byte, record the
//!   prefix SHA-256, never reuse a source Run ID, refuse out-of-range K or an
//!   existing target without writing, and never change the source.
//! - 16.3: Harness fence and pending Run refusals, a failed resume creates no
//!   Session, and running on one fork leaves the other fork and the source
//!   unchanged. Sessions are produced by real Runs against a loopback provider.
mod support;

use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
};

use ara_agent_lite_core::{
    execute,
    session_fork::{fork_session, receipt_path, source_digest},
    session_journal::{self, Journal},
};
use proptest::prelude::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use support::{call, run, text, Provider, ScriptedTool, TempDir};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Run one controlled Run per reply on `state` in `root`; returns the Run IDs.
fn runs(root: &Path, state: &Path, replies: &[String]) -> Vec<String> {
    let runtime = support::runtime();
    replies
        .iter()
        .map(|reply| {
            let provider = Provider::new(vec![text(reply)]);
            let mut request = support::request(root, &provider, &[], 4, "Answer briefly.");
            request.state_path = state.to_path_buf();
            let id = request.id.clone();
            let (result, _) = runtime.block_on(run(request));
            assert_eq!(result.code, 0, "{}", result.error);
            id
        })
        .collect()
}

/// Exact raw journal lines of a Session, in record order.
fn raw_records(state: &Path) -> Vec<Vec<u8>> {
    let directory = session_journal::directory(state);
    let mut segments: Vec<PathBuf> = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.file_name().unwrap().to_string_lossy().starts_with("segment-"))
        .collect();
    segments.sort();
    segments
        .iter()
        .flat_map(|segment| {
            std::fs::read(segment).unwrap().split_inclusive(|b| *b == b'\n').map(<[u8]>::to_vec).collect::<Vec<_>>()
        })
        .collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Paths a fork target owns; none may exist after a refused fork.
fn target_paths(target: &Path) -> [PathBuf; 3] {
    [target.to_path_buf(), session_journal::directory(target), receipt_path(target)]
}

fn assert_absent(targets: &[&Path]) {
    for target in targets {
        for path in target_paths(target) {
            assert!(!path.exists(), "refused fork wrote {}", path.display());
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 12, failure_persistence: None, ..ProptestConfig::default() })]

    /// Property 14: prefix bytes, prefix SHA-256, distinct paths, no Run ID
    /// reuse, refusals without writes, unchanged source.
    #[test]
    fn property_14_fork_keeps_prefix_and_source(
        replies in proptest::collection::vec("[a-zA-Z0-9 \u{4e2d}\u{6587}\u{1F600}]{1,40}", 1..=3),
        pick in any::<prop::sample::Index>(),
        out_of_range in prop_oneof![Just(0usize), Just(1usize)],
    ) {
        let root = TempDir::new();
        let source = root.path().join("session.json");
        let source_runs = runs(root.path(), &source, &replies);
        let source_records = raw_records(&source);
        let n = source_records.len();
        prop_assert!(n >= 2 * replies.len());
        let before = source_digest(&source).unwrap();
        let (a, b) = (root.path().join("fork-a.json"), root.path().join("fork-b.json"));

        // K outside 1..=N: refused, nothing written.
        let bad_k = if out_of_range == 0 { 0 } else { n + 1 };
        let refused = fork_session(&source, root.path(), bad_k, [&a, &b]);
        prop_assert!(refused.is_err());
        prop_assert!(refused.unwrap_err().to_string().contains("outside"));
        assert_absent(&[&a, &b]);
        prop_assert_eq!(&source_digest(&source).unwrap(), &before);

        // Existing target: refused, the existing file is untouched.
        std::fs::write(&b, b"occupied").unwrap();
        let k = pick.index(n) + 1;
        prop_assert!(fork_session(&source, root.path(), k, [&a, &b]).is_err());
        prop_assert_eq!(std::fs::read(&b).unwrap(), b"occupied".to_vec());
        assert_absent(&[&a]);
        std::fs::remove_file(&b).unwrap();
        prop_assert_eq!(&source_digest(&source).unwrap(), &before);

        // Valid K.
        let receipt = fork_session(&source, root.path(), k, [&a, &b]).unwrap();
        prop_assert_eq!(receipt.prefix_records, k);
        prop_assert_eq!(&receipt.source_sha256_before, &before);
        prop_assert_eq!(&source_digest(&source).unwrap(), &before);
        let prefix: Vec<u8> = source_records[..k].concat();
        prop_assert_eq!(&receipt.prefix_sha256, &sha256_hex(&prefix));
        prop_assert!(receipt.targets[0] != receipt.targets[1]);
        for target in [&a, &b] {
            prop_assert!(target.as_path() != source.as_path());
            prop_assert_eq!(raw_records(target), source_records[..k].to_vec());
            let recorded: Value = serde_json::from_slice(&std::fs::read(receipt_path(target)).unwrap()).unwrap();
            prop_assert_eq!(recorded["prefix_sha256"].as_str(), Some(receipt.prefix_sha256.as_str()));
            let journal = Journal::for_session(target);
            for id in &source_runs {
                prop_assert!(journal.contains_run(id).unwrap(), "fork may reuse source Run ID {}", id);
            }
            let head = journal.open().unwrap();
            prop_assert!(head.metadata["pending_run"].is_null());
            prop_assert!(head.metadata["goal"].is_null());
            prop_assert_eq!(&head.metadata["cwd"], &json!(root.path()));
        }
        // A fork at any K is a loadable Session that a new Run can continue.
        runs(root.path(), &a, &["continued".into()]);
        prop_assert_eq!(raw_records(&a)[..k].to_vec(), source_records[..k].to_vec());
        prop_assert_eq!(&source_digest(&source).unwrap(), &before);
    }
}

/// 6.4: a Harness fence beside the source refuses the fork, writing nothing.
#[test]
fn fence_refuses_fork_without_files() {
    let root = TempDir::new();
    let source = root.path().join("session.json");
    runs(root.path(), &source, &["first".into()]);
    let identity = source.to_str().unwrap();
    let identity = if cfg!(windows) { identity.to_lowercase() } else { identity.to_owned() };
    let fence = root.path().join(format!(".ara-lite-harness-{}.fence.json", sha256_hex(identity.as_bytes())));
    std::fs::write(&fence, serde_json::to_vec(&json!({"version":1,"session_path":source})).unwrap()).unwrap();
    let before = source_digest(&source).unwrap();
    let (a, b) = (root.path().join("fork-a.json"), root.path().join("fork-b.json"));
    let error = fork_session(&source, root.path(), 1, [&a, &b]).unwrap_err().to_string();
    assert!(error.contains("Harness fence"), "{error}");
    assert_absent(&[&a, &b]);
    assert_eq!(source_digest(&source).unwrap(), before);
}

/// Leave `source` with a pending Run, as after a crash mid-tool.
fn crashed_source(root: &Path) -> PathBuf {
    let source = root.join("session.json");
    runs(root, &source, &["settled".into()]);
    let started = Arc::new(AtomicBool::new(false));
    let tool =
        ScriptedTool::holding("slow_probe", Arc::new(AtomicBool::new(true)), started.clone(), |_| Ok("x".into()));
    let provider = Provider::new(vec![call("call_slow", "slow_probe", json!({}))]);
    let request = support::request(root, &provider, &[tool], 4, "Probe slowly.");
    support::crash_mid_tool(request, started);
    let head = Journal::for_session(&source).open().unwrap();
    assert!(!head.metadata["pending_run"].is_null(), "fixture must leave a pending Run");
    source
}

/// 6.4: a pending Run refuses the fork and names the reason.
#[test]
fn pending_run_refuses_fork_without_files() {
    let root = TempDir::new();
    let source = crashed_source(root.path());
    let before = source_digest(&source).unwrap();
    let (a, b) = (root.path().join("fork-a.json"), root.path().join("fork-b.json"));
    let error = fork_session(&source, root.path(), 1, [&a, &b]).unwrap_err().to_string();
    assert!(error.contains("pending Run"), "{error}");
    assert_absent(&[&a, &b]);
    assert_eq!(source_digest(&source).unwrap(), before);
}

/// Session state paths in `root`: `*.journal` directories and fork receipts.
fn session_entries(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".journal") || name.ends_with(".fork.json"))
        .collect();
    names.sort();
    names
}

/// 6.6: a failed resume is reported as a failure and no fork Session appears.
#[test]
fn failed_resume_creates_no_session() {
    let root = TempDir::new();
    let source = crashed_source(root.path());
    let sessions = session_entries(root.path());
    let provider = Provider::new(vec![text("must not run")]);
    let request = support::request(root.path(), &provider, &[], 4, "Resume.");
    let (sender, mut receiver) = mpsc::channel(256);
    let outcome = support::runtime().block_on(async move {
        tokio::spawn(async move { while receiver.recv().await.is_some() {} });
        execute(request, CancellationToken::new(), sender).await
    });
    match outcome {
        Err(error) => assert!(error.to_string().contains("incomplete previous run"), "{error}"),
        Ok(result) => assert_ne!(result.code, 0, "resume of a fenced Session must fail"),
    }
    assert_eq!(provider.request_count(), 0, "no model request after a failed resume");
    assert_eq!(session_entries(root.path()), sessions, "a failed resume must not create Sessions");
    assert!(!Journal::for_session(&source).open().unwrap().metadata["pending_run"].is_null());
}

/// 6.7: a Run on fork A changes neither fork B nor the source.
#[test]
fn run_on_one_fork_isolates_other_fork_and_source() {
    let root = TempDir::new();
    let source = root.path().join("session.json");
    runs(root.path(), &source, &["one".into(), "two".into()]);
    let (a, b) = (root.path().join("fork-a.json"), root.path().join("fork-b.json"));
    let k = raw_records(&source).len();
    fork_session(&source, root.path(), k, [&a, &b]).unwrap();
    let source_before = source_digest(&source).unwrap();
    let b_before = source_digest(&b).unwrap();
    let run_ids = runs(root.path(), &a, &["on fork a".into()]);
    assert_eq!(run_ids.len(), 1);
    assert!(raw_records(&a).len() > k, "the fork Run appended records");
    assert_eq!(raw_records(&a)[..k], raw_records(&source)[..]);
    assert_eq!(source_digest(&source).unwrap(), source_before);
    assert_eq!(source_digest(&b).unwrap(), b_before);
    assert!(!Journal::for_session(&source).contains_run(&run_ids[0]).unwrap());
    assert!(!Journal::for_session(&b).contains_run(&run_ids[0]).unwrap());
}
