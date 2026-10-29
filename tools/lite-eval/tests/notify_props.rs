// Feature: ara-lite-core-capability, Property 45: 通知决策
//! Random ledger changes and script outcomes against a substitute runner: no
//! call when the state digest repeats the latest notice or the latest
//! delivered/unconfirmed one, the severity mapping holds, every call carries
//! the fixed sender and start time, statuses follow the script outcome, and
//! failed content is never sent again on its own.

use std::{
    cell::RefCell,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use lite_eval::{
    common::{sha256_hex, Credentials},
    digest::WORKTREE_DIFF_RULE,
    ledger::{Append, Attempt, Blocker, CodeVersion, Ledger, State},
    notify::{state_digest, Notifier, RunOutcome, Runner, Severity, Status, SENDER, STARTED_AT},
};
use proptest::prelude::*;

const MODULES: [&str; 3] = ["M-P0-TASKSET", "M-P0-RUNNER", "M-X-NOTIFY"];

fn config() -> ProptestConfig {
    ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(256) }
}

#[derive(Clone, Copy, Debug)]
enum Script {
    Delivered,
    Rejected,
    Timeout,
    NetworkError,
    TooLarge,
    Spawn,
}

impl Script {
    fn outcome(self) -> RunOutcome {
        let exited = |code: i32, stdout: &str, stderr: &str| RunOutcome::Exited {
            code: Some(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        };
        match self {
            Self::Delivered => exited(0, "{\"delivered\":true,\"errcode\":\"0\"}\n", ""),
            Self::Rejected => {
                exited(1, "{\"delivered\":false,\"errcode\":\"300001\"}\n", "DingTalk rejected delivery.")
            }
            Self::Timeout => RunOutcome::TimedOut,
            Self::NetworkError => exited(1, "", "DingTalk delivery could not be confirmed. Exception type: X"),
            Self::TooLarge => exited(1, "", "Notification exceeds the local 16 KiB limit"),
            Self::Spawn => RunOutcome::SpawnFailed("no powershell".into()),
        }
    }

    fn status(self) -> Status {
        match self {
            Self::Delivered => Status::Delivered,
            Self::Rejected => Status::Undelivered,
            Self::Timeout | Self::NetworkError => Status::Unconfirmed,
            Self::TooLarge | Self::Spawn => Status::NotSent,
        }
    }
}

/// Records every call and answers with the next scripted outcome.
#[derive(Clone)]
struct Fake {
    calls: Rc<RefCell<Vec<Vec<String>>>>,
    next: Rc<RefCell<Script>>,
}

impl Runner for Fake {
    fn run(&self, args: &[OsString], _: Duration) -> RunOutcome {
        self.calls.borrow_mut().push(args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect());
        self.next.borrow().outcome()
    }
}

#[derive(Clone, Debug)]
struct Change {
    module: usize,
    state: u8,
    blocker: Option<u8>,
    script: Script,
    /// Re-notify the same ledger state (no new entry).
    repeat: bool,
}

fn script() -> impl Strategy<Value = Script> {
    prop_oneof![
        4 => Just(Script::Delivered),
        1 => Just(Script::Rejected),
        1 => Just(Script::Timeout),
        1 => Just(Script::NetworkError),
        1 => Just(Script::TooLarge),
        1 => Just(Script::Spawn),
    ]
}

fn change() -> impl Strategy<Value = Change> {
    (0..MODULES.len(), 0u8..3, prop::option::weighted(0.3, 0u8..2), script(), prop::bool::weighted(0.25))
        .prop_map(|(module, state, blocker, script, repeat)| Change { module, state, blocker, script, repeat })
}

fn value(args: &[String], name: &str) -> String {
    args[args.iter().position(|arg| arg == name).unwrap() + 1].clone()
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn notifier_decisions_follow_the_reference_model(changes in prop::collection::vec(change(), 1..12)) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("e.json"), b"{}\n").unwrap();
        let version = CodeVersion {
            head: "56d55d209ecea00e80dfefecdd3e9d47fb62c47a".into(),
            diff_sha256: Some(sha256_hex(b"d")),
            diff_rule: WORKTREE_DIFF_RULE.into(),
        };
        let fixed = version.clone();
        let ledger = Ledger::new(root.join("ledger.jsonl"), root)
            .with_version_source(Arc::new(move |_: &Path| Ok(fixed.clone())));
        let fake = Fake { calls: Rc::default(), next: Rc::new(RefCell::new(Script::Delivered)) };
        let notifier = Notifier::new(root.join("notifications.jsonl"), PathBuf::from("fake.ps1"), fake.clone(), Credentials::default());

        // Model: digests of the latest record and of the latest delivered/unconfirmed record.
        let mut last: Option<String> = None;
        let mut last_reached: Option<String> = None;
        let mut records = 0usize;
        for change in &changes {
            if !change.repeat || ledger.entries().unwrap().is_empty() {
                let state = [State::Open, State::Done, State::Open][change.state as usize];
                let blocker = change.blocker.filter(|_| state == State::Open).map(|kind| Blocker {
                    reason: format!("route {kind} unavailable"),
                    attempts: vec![Attempt { action: "probe".into(), result: "403".into() }],
                    decision_needed: "authorize fallback".into(),
                });
                ledger.append(Append {
                    module: MODULES[change.module].into(),
                    state,
                    evidence: "e.json".into(),
                    uncovered: String::new(),
                    code_version: version.clone(),
                    criteria_done: 0,
                    gate_record: None,
                    review_records: Vec::new(),
                    blocker,
                }, &Credentials::default()).unwrap();
            }
            let entries = ledger.entries().unwrap();
            let digest = state_digest(&entries, None);
            *fake.next.borrow_mut() = change.script;
            let calls_before = fake.calls.borrow().len();
            let result = notifier.on_ledger_change(&entries).unwrap();

            let repeats = last.as_deref() == Some(digest.as_str()) || last_reached.as_deref() == Some(digest.as_str());
            prop_assert_eq!(result.is_none(), repeats, "digest repeat decides whether the script runs");
            prop_assert_eq!(fake.calls.borrow().len() - calls_before, usize::from(!repeats));
            let Some(record) = result else { continue };
            records += 1;
            prop_assert_eq!(record.seq as usize, records);
            prop_assert_eq!(record.status, change.script.status());
            prop_assert_eq!(record.errcode.is_some(), matches!(change.script, Script::Delivered | Script::Rejected));
            prop_assert_eq!(record.reason_category.is_none(), matches!(change.script, Script::Delivered));

            let entry = entries.last().unwrap();
            let previous = entries[..entries.len() - 1].iter().rev().find(|earlier| earlier.module == entry.module);
            let expected = if entry.blocker.is_some() && previous.is_none_or(|p| p.blocker != entry.blocker) {
                Severity::Blocker
            } else if entry.state == State::Done && previous.is_none_or(|p| p.state != entry.state) {
                Severity::Milestone
            } else {
                Severity::Info
            };
            prop_assert_eq!(record.severity, expected);

            let calls = fake.calls.borrow();
            let args = calls.last().unwrap();
            prop_assert_eq!(value(args, "-Sender"), SENDER);
            prop_assert_eq!(value(args, "-StartedAt"), STARTED_AT);
            prop_assert_eq!(value(args, "-Severity"), format!("{expected:?}"));
            prop_assert!(value(args, "-OverallProgress").contains("/36"));
            prop_assert!(!value(args, "-Progress").is_empty());

            last = Some(digest.clone());
            if matches!(record.status, Status::Delivered | Status::Unconfirmed) {
                last_reached = Some(digest);
            }
        }
        prop_assert_eq!(notifier.records().unwrap().len(), records);
    }
}
