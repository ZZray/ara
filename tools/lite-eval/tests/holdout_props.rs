// Feature: ara-lite-core-capability, Property 3: Holdout 状态单调
//! Random sequences of marks (with blank or real change identifiers, on
//! registered or unknown tasks), direct edits of the registered status and
//! restores. Once a task is `not new holdout` its status and first-use date
//! derived from the on-disk log never change; refused operations write
//! nothing; a direct edit that disagrees with the log is reported; every
//! `marked_not_new` line carries a date and at least one change.

use std::{collections::BTreeMap, fs, path::PathBuf};

use chrono::NaiveDate;
use lite_eval::{
    digest::TREE_RULE,
    registry::{
        check_holdout_log, mark_not_new, HoldoutEventKind, HoldoutLog, HoldoutStatus, HostProfile, OracleSpec,
        ProfileMode, TaskEntry, TaskSet, Violation,
    },
};
use proptest::prelude::*;

const TASKS: [&str; 3] = ["A", "B", "C"];

fn config() -> ProptestConfig {
    ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(256) }
}

fn day(offset: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 20 + offset).unwrap()
}

#[derive(Clone, Debug)]
enum Op {
    Mark {
        task: &'static str,
        day: u32,
        changes: Vec<String>,
    },
    /// Edit the registered JSON status directly, bypassing the log.
    Edit {
        task: usize,
        status: Option<(bool, u32)>,
    },
    /// Put the registered status back to what the log says.
    Restore {
        task: usize,
    },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (
            prop::sample::select(vec!["A", "B", "C", "Z"]),
            0..5u32,
            prop::collection::vec(prop::sample::select(vec!["", "  ", "c1", "c2"]).prop_map(str::to_owned), 0..3),
        )
            .prop_map(|(task, day, changes)| Op::Mark { task, day, changes }),
        1 => (0..3usize, prop::option::of((any::<bool>(), 0..5u32))).prop_map(|(task, status)| Op::Edit { task, status }),
        1 => (0..3usize).prop_map(|task| Op::Restore { task }),
    ]
}

fn registration() -> TaskSet {
    TaskSet {
        version: 1,
        digest_rule: TREE_RULE.into(),
        sealed: None,
        tasks: TASKS
            .iter()
            .map(|id| TaskEntry {
                task_id: (*id).into(),
                kind: None,
                fixture_path: PathBuf::from("D:/fixtures"),
                fixture_sha256: String::new(),
                oracle: OracleSpec::default(),
                holdout: None,
                profile: HostProfile { mode: ProfileMode::Plan, tools: vec![], public_check: vec![] },
            })
            .collect(),
    }
}

/// Status per task derived directly from the log lines on disk.
fn derived(path: &std::path::Path) -> BTreeMap<String, HoldoutStatus> {
    let mut statuses = BTreeMap::new();
    for event in HoldoutLog::load(path).unwrap().events() {
        if event.event == HoldoutEventKind::MarkedNotNew {
            statuses
                .entry(event.task_id.clone())
                .or_insert(HoldoutStatus::NotNew { first_adaptive_use: event.date, changes: event.changes.clone() });
        }
    }
    statuses
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn not_new_holdout_never_reverts(ops in prop::collection::vec(op(), 1..24)) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holdout-log.jsonl");
        let mut set = registration();
        let mut log = HoldoutLog::load(&path).unwrap();
        // Reference model: first successful mark per task.
        let mut first: BTreeMap<String, HoldoutStatus> = BTreeMap::new();
        for op in ops {
            let before = fs::read(&path).ok();
            match op {
                Op::Mark { task, day: offset, changes } => {
                    let consistent = set.tasks.iter().all(|entry| entry.holdout.as_ref() == first.get(&entry.task_id));
                    let trimmed: Vec<String> =
                        changes.iter().map(|c| c.trim().to_owned()).filter(|c| !c.is_empty()).collect();
                    let registered = TASKS.contains(&task);
                    let expect_ok = consistent && !trimmed.is_empty() && registered && !first.contains_key(task);
                    let saved = set.clone();
                    let result = mark_not_new(&mut set, &mut log, task, day(offset), changes, None);
                    prop_assert_eq!(result.is_ok(), expect_ok, "mark {} {:?}", task, result.err());
                    if expect_ok {
                        let status = HoldoutStatus::NotNew { first_adaptive_use: day(offset), changes: trimmed };
                        first.insert(task.to_owned(), status.clone());
                        let entry = set.tasks.iter().find(|entry| entry.task_id == task).unwrap();
                        prop_assert_eq!(entry.holdout.as_ref(), Some(&status));
                    } else {
                        prop_assert_eq!(&set, &saved);
                        prop_assert_eq!(fs::read(&path).ok(), before);
                    }
                }
                Op::Edit { task, status } => {
                    set.tasks[task].holdout = status.map(|(new, offset)| {
                        if new {
                            HoldoutStatus::New { registered: day(offset) }
                        } else {
                            HoldoutStatus::NotNew { first_adaptive_use: day(offset), changes: vec!["edit".into()] }
                        }
                    });
                    prop_assert_eq!(fs::read(&path).ok(), before);
                }
                Op::Restore { task } => {
                    let id = set.tasks[task].task_id.clone();
                    set.tasks[task].holdout = first.get(&id).cloned();
                }
            }
            // The log on disk is the authority and agrees with the model.
            prop_assert_eq!(derived(&path), first.clone());
            let violations = check_holdout_log(&set, HoldoutLog::load(&path).unwrap().events());
            for entry in &set.tasks {
                let mismatch = violations.iter().any(|violation| {
                    matches!(violation, Violation::HoldoutMismatch { task, .. } if *task == entry.task_id)
                });
                prop_assert_eq!(mismatch, entry.holdout.as_ref() != first.get(&entry.task_id));
            }
            let only_mismatches =
                violations.iter().all(|violation| matches!(violation, Violation::HoldoutMismatch { .. }));
            prop_assert!(only_mismatches, "unexpected violations {:?}", violations);
            for event in HoldoutLog::load(&path).unwrap().events() {
                prop_assert_eq!(event.event, HoldoutEventKind::MarkedNotNew);
                prop_assert!(event.changes.iter().any(|change| !change.trim().is_empty()));
            }
        }
    }
}

#[test]
fn log_edits_that_revert_or_redate_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("holdout-log.jsonl");
    let mut set = registration();
    let mut log = HoldoutLog::load(&path).unwrap();
    mark_not_new(&mut set, &mut log, "A", day(3), vec!["c1".into()], None).unwrap();
    let line = fs::read_to_string(&path).unwrap();
    // A later registered_new for the same task, and a second mark with another date.
    let appended = [
        line.replace("\"seq\":1", "\"seq\":2").replace("marked_not_new", "registered_new"),
        line.replace("\"seq\":1", "\"seq\":2").replace("2026-10-23", "2026-10-21"),
    ];
    for extra in appended {
        fs::write(&path, format!("{line}{extra}")).unwrap();
        let violations = check_holdout_log(&set, HoldoutLog::load(&path).unwrap().events());
        assert!(matches!(violations.as_slice(), [Violation::HoldoutInvalidEvent { seq: 2, .. }]), "{violations:?}");
    }
    // Rewriting the only line to another date is a mismatch with the registration.
    fs::write(&path, line.replace("2026-10-23", "2026-10-21")).unwrap();
    let violations = check_holdout_log(&set, HoldoutLog::load(&path).unwrap().events());
    assert!(matches!(violations.as_slice(), [Violation::HoldoutMismatch { .. }]), "{violations:?}");
}
