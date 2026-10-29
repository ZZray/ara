// Feature: ara-lite-core-capability, Property 44: 进度台账状态
//! Random append sequences against a reference model: earlier bytes never
//! change, the current state of a module is its latest entry, `verified` is
//! accepted exactly when gate, reviews and diff all line up, and a changed
//! delivery diff demotes verified modules until they are re-verified.

use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

use lite_eval::{
    common::{sha256_hex, Credentials},
    digest::WORKTREE_DIFF_RULE,
    ledger::{current, Append, CodeVersion, Ledger, State},
    records::{
        Count, CountLabel, Disposition, Finding, GateCommand, GateRecord, ReviewGap, ReviewRecord, ReviewScope,
        GATE_COMMANDS, GIT_REVIEW, INDEPENDENT_REVIEWER, PROVIDER_REVIEW, RUST_CORE_REVIEW,
    },
};
use proptest::prelude::*;

const MODULES: [&str; 3] = ["M-P0-TASKSET", "M-P1-PY", "M-X-LEDGER"];
const AT: &str = "2026-10-29T13:00:00+08:00";

fn config() -> ProptestConfig {
    ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(256) }
}

fn version(diff: u8) -> CodeVersion {
    CodeVersion {
        head: "56d55d209ecea00e80dfefecdd3e9d47fb62c47a".into(),
        diff_sha256: Some(sha256_hex(&[diff])),
        diff_rule: WORKTREE_DIFF_RULE.into(),
    }
}

/// How the gate record of a verified attempt is built.
#[derive(Clone, Copy, Debug)]
enum GateCase {
    Pass,
    FailingCommand(usize),
    FailedTests(usize),
    MissingCommand(usize),
    OtherDiff,
    Absent,
}

/// How one required review record is built.
#[derive(Clone, Copy, Debug)]
enum ReviewCase {
    Complete,
    Gap,
    UndisposedDeferral,
    OtherDiff,
    Absent,
}

#[derive(Clone, Debug)]
enum Step {
    /// Plain open or done entry.
    Plain {
        module: usize,
        done: bool,
    },
    /// Attempt verified; `use_provider` swaps the Rust review for the provider review.
    Verify {
        module: usize,
        gate: GateCase,
        reviews: [ReviewCase; 3],
        use_provider: bool,
        stale_entry: bool,
    },
    /// The delivery diff changes, then revalidate runs.
    ChangeDiff,
    Revalidate,
}

fn gate_case() -> impl Strategy<Value = GateCase> {
    prop_oneof![
        4 => Just(GateCase::Pass),
        1 => (0..GATE_COMMANDS.len()).prop_map(GateCase::FailingCommand),
        1 => (0..GATE_COMMANDS.len()).prop_map(GateCase::FailedTests),
        1 => (0..GATE_COMMANDS.len()).prop_map(GateCase::MissingCommand),
        1 => Just(GateCase::OtherDiff),
        1 => Just(GateCase::Absent),
    ]
}

fn review_case() -> impl Strategy<Value = ReviewCase> {
    prop_oneof![
        8 => Just(ReviewCase::Complete),
        1 => Just(ReviewCase::Gap),
        1 => Just(ReviewCase::UndisposedDeferral),
        1 => Just(ReviewCase::OtherDiff),
        1 => Just(ReviewCase::Absent),
    ]
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => (0..MODULES.len(), any::<bool>()).prop_map(|(module, done)| Step::Plain { module, done }),
        5 => (0..MODULES.len(), gate_case(), [review_case(), review_case(), review_case()], any::<bool>(), prop::bool::weighted(0.1))
            .prop_map(|(module, gate, reviews, use_provider, stale_entry)| Step::Verify { module, gate, reviews, use_provider, stale_entry }),
        1 => Just(Step::ChangeDiff),
        1 => Just(Step::Revalidate),
    ]
}

fn gate_record(module: &str, case: GateCase, now: &CodeVersion, stale: &CodeVersion) -> GateRecord {
    let mut commands: Vec<GateCommand> = GATE_COMMANDS
        .iter()
        .map(|id| GateCommand {
            crate_name: id.crate_name.to_owned(),
            command: id.command.to_owned(),
            exit_code: Some(0),
            passed: if id.counts_tests { Count::Value(5) } else { Count::Label(CountLabel::NotApplicable) },
            failed: if id.counts_tests { Count::Value(0) } else { Count::Label(CountLabel::NotApplicable) },
            started_at: AT.into(),
            finished_at: AT.into(),
            stdout_path: None,
            stderr_path: None,
        })
        .collect();
    let mut code_version = now.clone();
    match case {
        GateCase::Pass | GateCase::Absent => {}
        GateCase::FailingCommand(index) => commands[index].exit_code = Some(1),
        GateCase::FailedTests(index) => {
            if GATE_COMMANDS[index].counts_tests {
                commands[index].failed = Count::Value(1);
            } else {
                commands[index].passed = Count::Value(0);
            }
        }
        GateCase::MissingCommand(index) => {
            commands.remove(index);
        }
        GateCase::OtherDiff => code_version = stale.clone(),
    }
    GateRecord { module: module.to_owned(), code_version, commands }
}

fn review_record(tool: &str, case: ReviewCase, now: &CodeVersion, stale: &CodeVersion) -> ReviewRecord {
    let independent = tool == INDEPENDENT_REVIEWER;
    let mut record = ReviewRecord {
        tool: tool.to_owned(),
        scope: ReviewScope { base: now.head.clone(), target: "worktree".into(), files: vec!["src/a.rs".into()] },
        diff_sha256: now.diff_sha256.clone(),
        session_id: independent.then(|| "review".to_owned()),
        implementation_session_id: independent.then(|| "implementation".to_owned()),
        findings: vec![Finding {
            location: "src/a.rs:3".into(),
            description: "finding".into(),
            disposition: Disposition::FalsePositive,
            reason: None,
        }],
        gap: None,
        started_at: AT.into(),
        finished_at: AT.into(),
    };
    match case {
        ReviewCase::Complete | ReviewCase::Absent => {}
        ReviewCase::Gap => record.gap = Some(ReviewGap { reason: "tool unavailable".into() }),
        ReviewCase::UndisposedDeferral => record.findings[0].disposition = Disposition::Deferred,
        ReviewCase::OtherDiff => record.diff_sha256 = stale.diff_sha256.clone(),
    }
    record
}

fn gate_ok(case: GateCase) -> bool {
    matches!(case, GateCase::Pass)
}

/// Reference model of a verified attempt.
fn model_accepts(gate: GateCase, reviews: [ReviewCase; 3], stale_entry: bool) -> bool {
    !stale_entry && gate_ok(gate) && reviews.iter().all(|case| matches!(case, ReviewCase::Complete))
}

fn write(root: &Path, relative: &str, value: &impl serde::Serialize) -> String {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    relative.to_owned()
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn ledger_states_follow_the_reference_model(steps in prop::collection::vec(step(), 1..14)) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("evidence.json"), b"{}\n").unwrap();
        let diff = Arc::new(Mutex::new(0u8));
        let source = diff.clone();
        let ledger = Ledger::new(root.join("progress-ledger.jsonl"), root)
            .with_version_source(Arc::new(move |_: &Path| Ok(version(*source.lock().unwrap()))));
        let credentials = Credentials::default();

        // Model: module -> (state, version of its latest entry).
        let mut model: BTreeMap<&str, (State, CodeVersion)> = BTreeMap::new();
        let mut appended = 0u64;
        let mut previous_bytes = Vec::new();
        for (index, step) in steps.iter().enumerate() {
            let now = version(*diff.lock().unwrap());
            let stale = version(diff.lock().unwrap().wrapping_add(100));
            let base = |module: &str, state: State| Append {
                module: module.to_owned(),
                state,
                evidence: "evidence.json".into(),
                uncovered: String::new(),
                code_version: now.clone(),
                criteria_done: 0,
                gate_record: None,
                review_records: Vec::new(),
                blocker: None,
            };
            match step {
                Step::Plain { module, done } => {
                    let state = if *done { State::Done } else { State::Open };
                    ledger.append(base(MODULES[*module], state), &credentials).unwrap();
                    appended += 1;
                    model.insert(MODULES[*module], (state, now.clone()));
                }
                Step::Verify { module, gate, reviews, use_provider, stale_entry } => {
                    let name = MODULES[*module];
                    let gate_path = (!matches!(gate, GateCase::Absent))
                        .then(|| write(root, &format!("gates/{index}.json"), &gate_record(name, *gate, &now, &stale)));
                    let rust = if *use_provider { PROVIDER_REVIEW } else { RUST_CORE_REVIEW };
                    let review_paths: Vec<String> = [INDEPENDENT_REVIEWER, GIT_REVIEW, rust]
                        .iter()
                        .zip(reviews)
                        .filter(|(_, case)| !matches!(case, ReviewCase::Absent))
                        .map(|(tool, case)| write(root, &format!("reviews/{index}-{tool}.json"), &review_record(tool, *case, &now, &stale)))
                        .collect();
                    let request = Append {
                        gate_record: gate_path,
                        review_records: review_paths,
                        code_version: if *stale_entry { stale.clone() } else { now.clone() },
                        ..base(name, State::Verified)
                    };
                    let result = ledger.append(request, &credentials);
                    let expected = model_accepts(*gate, *reviews, *stale_entry);
                    prop_assert_eq!(result.is_ok(), expected, "step {}: {:?} -> {:?}", index, step, result.err().map(|e| format!("{e:#}")));
                    if expected {
                        appended += 1;
                        model.insert(name, (State::Verified, now.clone()));
                    }
                }
                Step::ChangeDiff => {
                    let mut value = diff.lock().unwrap();
                    *value = value.wrapping_add(1);
                }
                Step::Revalidate => {
                    let demoted = ledger.revalidate(&credentials).unwrap();
                    let expected: Vec<&str> = model
                        .iter()
                        .filter(|(_, (state, at))| *state == State::Verified && *at != now)
                        .map(|(module, _)| *module)
                        .collect();
                    let mut got: Vec<&str> = demoted.iter().map(|entry| entry.module.as_str()).collect();
                    got.sort_unstable();
                    prop_assert_eq!(&got, &expected);
                    prop_assert!(demoted.iter().all(|entry| entry.state == State::Done && entry.code_version == now));
                    appended += demoted.len() as u64;
                    for module in expected {
                        model.insert(module, (State::Done, now.clone()));
                    }
                }
            }

            // Earlier bytes never change; seq counts every accepted append.
            let bytes = fs::read(ledger.path()).unwrap_or_default();
            prop_assert!(bytes.starts_with(&previous_bytes), "step {}: earlier ledger bytes changed", index);
            previous_bytes = bytes;
            let entries = ledger.entries().unwrap();
            prop_assert_eq!(entries.len() as u64, appended);
            prop_assert!(entries.iter().zip(1u64..).all(|(entry, seq)| entry.seq == seq));

            // Current state is the latest entry and matches the model.
            let states = current(&entries);
            prop_assert_eq!(states.len(), model.len());
            for (module, (state, at)) in &model {
                let entry = states[module];
                prop_assert_eq!(entry.state, *state, "module {}", module);
                prop_assert_eq!(&entry.code_version, at, "module {}", module);
                prop_assert_eq!(entry.seq, entries.iter().filter(|e| e.module == *module).map(|e| e.seq).max().unwrap());
            }
        }
    }
}
