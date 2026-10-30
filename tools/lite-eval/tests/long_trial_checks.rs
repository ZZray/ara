//! `lite-eval long-trial` checks over pre-recorded trial files (task 60.2;
//! requirements 35.3–35.6). Each test writes one passing trial in the
//! layout of `long_trial/long_trial.py`, with a real Session journal, then
//! changes one file and checks the verdict, the failed check and the
//! Failure_Record.

use std::{
    fs,
    path::{Path, PathBuf},
};

use ara_agent_lite_harness::core::{context_memory, session_journal::Journal};
use lite_eval::{
    common::{sha256_file, sha256_hex, Credentials},
    long_trial::{check_trial, orphaned_sidecars, CheckStatus, LongTrialRecord, TrialVerdict, COVERAGE_INSUFFICIENT},
};
use serde_json::{json, Value};

const MAX_CALLS: usize = 160;
const TASK: &str = "T";
const AUTHORITY: &str = "authority-declaration-a";
const OPEN_CALL: &str = "call-9";

fn write(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn edit(path: &Path, change: impl FnOnce(&mut Value)) {
    let mut value = read(path);
    change(&mut value);
    write(path, &value);
}

fn events(path: &Path, run_id: &str, events: &[Value]) {
    let text: String = events
        .iter()
        .enumerate()
        .map(|(index, event)| {
            json!({"task_id": TASK, "run_id": run_id, "seq": index as u64 + 1, "event": event}).to_string() + "\n"
        })
        .collect();
    fs::write(path, text).unwrap();
}

fn log(path: &Path, lines: &[Value]) {
    let text: String = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let mut line = line.clone();
            line["seq"] = json!(index as u64 + 1);
            line.to_string() + "\n"
        })
        .collect();
    fs::write(path, text).unwrap();
}

struct Fixture {
    _temp: tempfile::TempDir,
    dir: PathBuf,
}

impl Fixture {
    fn path(&self, relative: &str) -> PathBuf {
        self.dir.join(relative)
    }

    fn check(&self) -> LongTrialRecord {
        check_trial(&self.dir, MAX_CALLS, &Credentials::default(), None, Vec::new()).unwrap().0
    }
}

fn admission(dir: &Path, number: usize, run_id: &str, session: &Path, remaining: u64) {
    write(
        &dir.join(format!("run-{number}-admission.json")),
        &json!({"task_id": TASK, "run_id": run_id, "run_number": number, "session_path": session,
                "remaining_calls": remaining}),
    );
}

fn observation(dir: &Path, number: usize, run_id: &str, observed: u64, total: u64) {
    write(
        &dir.join(format!("run-{number}-observation.json")),
        &json!({"task_id": TASK, "run_id": run_id, "observed_calls": observed, "total_calls": total}),
    );
}

fn needs_changes() -> Value {
    json!({"committed": true, "settlement": "Settled", "verdict": {"NeedsChanges": {"feedback": "public checks failed"}},
           "evidence": ["host-review"]})
}

/// Run 1 compacts and stops at Length → length continuation to run 2;
/// runs 2 and 3 end with a NeedsChanges review → continuations to 3 and 4;
/// run 4 is killed in a write; the resumed run 5 is released.
fn passing() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("trial");
    fs::create_dir(&dir).unwrap();
    let state = dir.join("state");
    fs::create_dir(&state).unwrap();
    let session = state.join("session.json");
    fs::write(&session, b"{}").unwrap();
    let journal = Journal::for_session(&session);
    let messages: Vec<_> = (0..6).map(|i| context_memory::user(format!("message {i}"))).collect();
    journal.commit(json!({}), 0, &messages, &[]).unwrap();
    // The truncated assistant message of run 1 (any journal record serves).
    let typed = sha256_hex(&serde_json::to_vec(&journal.read(3, 4, usize::MAX).unwrap()[0]).unwrap());
    let proof = journal.proof(3, 4).unwrap();
    // Journal snapshot in the driver's format.
    let segment = fs::read_to_string(session.with_extension("journal").join("segment-00000000.jsonl")).unwrap();
    let records: Vec<Value> = segment
        .lines()
        .map(|line| {
            let row: Value = serde_json::from_str(line).unwrap();
            json!({"index": row["index"], "hash": row["hash"], "chain": row["chain"]})
        })
        .collect();
    let snapshot = dir.join("journal-001-before-resume.json");
    write(&snapshot, &json!({"label": "before-resume", "raw_count": 6, "records": records}));

    let first = dir.join("evidence-1");
    fs::create_dir(&first).unwrap();
    admission(&first, 1, "T-run-1", &session, 40);
    observation(&first, 1, "T-run-1", 5, 5);
    events(
        &first.join("run-1-events.jsonl"),
        "T-run-1",
        &[
            json!({"type": "model_call", "phase": "agent", "status": "reserved"}),
            json!({"type": "compact", "status": "completed", "covered_prefix": 2, "raw_messages": 4}),
            json!({"type": "agent_end", "status": "failed"}),
        ],
    );
    let stop_snapshot = first.join("run-1-stop-snapshot.json");
    write(
        &stop_snapshot,
        &json!({"basis": {"run_id": "T-run-1", "run_number": 1}, "diagnostic_output": "cut",
                "evidence": {"cause": "length"},
                "truncated": {"raw_index": 3, "typed_sha256": typed, "journal_proof": proof,
                              "effective_output_tokens": 2048}}),
    );
    write(
        &first.join("jsonl-rpc-1-request.json"),
        &json!({"kind": "prepare_stop_request", "rpc_id": "rpc-1", "run_id": "T-run-1",
                "stop_continuation": {"previous": {"evidence": {"cause": "length"}}, "receipt": needs_changes(),
                                      "next_run_number": 2}}),
    );
    write(&first.join("jsonl-rpc-1-reply.json"), &json!({"kind": "prepare_stop_reply", "run_id_next": "T-run-2"}));
    admission(&first, 2, "T-run-2", &session, 40);
    observation(&first, 2, "T-run-2", 4, 9);
    events(&first.join("run-2-events.jsonl"), "T-run-2", &[json!({"type": "agent_start", "status": "running"})]);
    // A continuation body written to a file and referenced, as for large frames.
    let continuation = first.join("jsonl-continuation-3.json");
    write(&continuation, &json!({"previous": {}, "receipt": needs_changes(), "next_run_number": 3}));
    write(
        &first.join("jsonl-rpc-2-request.json"),
        &json!({"kind": "prepare_request", "rpc_id": "rpc-2", "run_id": "T-run-2",
                "continuation_ref": {"path": continuation, "sha256": sha256_file(&continuation).unwrap()}}),
    );
    write(&first.join("jsonl-rpc-2-reply.json"), &json!({"kind": "prepare_reply", "run_id_next": "T-run-3"}));
    admission(&first, 3, "T-run-3", &session, 40);
    observation(&first, 3, "T-run-3", 3, 12);
    events(&first.join("run-3-events.jsonl"), "T-run-3", &[json!({"type": "agent_start", "status": "running"})]);
    write(
        &first.join("jsonl-rpc-3-request.json"),
        &json!({"kind": "prepare_request", "rpc_id": "rpc-3", "run_id": "T-run-3",
                "continuation": {"previous": {}, "receipt": needs_changes(), "next_run_number": 4}}),
    );
    write(&first.join("jsonl-rpc-3-reply.json"), &json!({"kind": "prepare_reply", "run_id_next": "T-run-4"}));
    admission(&first, 4, "T-run-4", &session, 40);
    events(&first.join("run-4-events.jsonl"), "T-run-4", &[json!({"type": "agent_start", "status": "running"})]);

    let fence = json!({"version": 1, "task_id": TASK, "session_path": session,
                       "authority": {"declaration_sha256": AUTHORITY}});
    write(&dir.join("fence-start.json"), &fence);
    write(&dir.join("fence-before-resume.json"), &fence);
    let second = dir.join("evidence-2");
    fs::create_dir(&second).unwrap();
    write(
        &second.join("resume.json"),
        &json!({"version": 1, "kind": "fenced_task_resume", "fence": fence, "consumed_runs": 4,
                "consumed_calls": 12 + 40,
                "interrupted_run": {"run_id": "T-run-4", "raw_count": 6,
                                    "open_tool_calls": [{"id": OPEN_CALL, "name": "ws_write_file",
                                                         "arguments": {"path": "a.txt"}}]}}),
    );
    admission(&second, 5, "T-resume", &session, 40);
    observation(&second, 5, "T-resume", 3, 55);
    events(
        &second.join("run-5-events.jsonl"),
        "T-resume",
        &[json!({"type": "model_call", "phase": "agent", "status": "reserved"})],
    );
    write(
        &second.join("released.json"),
        &json!({"task_id": TASK, "stop": "AwaitingHostAcceptance", "runs": 5,
                                                "observed_calls": 55}),
    );

    let ops = dir.join("ops-1");
    write(
        &ops.join("abc-stop-snapshot-checked.json"),
        &json!({"stage": "stop-snapshot-checked",
                "snapshot_ref": {"path": stop_snapshot, "sha256": sha256_file(&stop_snapshot).unwrap()}}),
    );
    let receipts = dir.join("host-receipts");
    let applied = receipts.join("0007-applied.json");
    write(
        &applied,
        &json!({"kind": "host_operation", "stage": "applied", "committed": true,
                            "native_call_id": OPEN_CALL, "tool_name": "ws_write_file"}),
    );
    let call = json!({"kind": "callback", "process": 1, "frame_kind": "tool_call", "native_call_id": OPEN_CALL,
                      "tool_name": "ws_write_file", "args_sha256": "args-a"});
    log(
        &dir.join("trial-log.jsonl"),
        &[
            call,
            json!({"kind": "host_receipt", "process": 1, "stage": "applied", "committed": true,
                   "native_call_id": OPEN_CALL, "path": applied}),
            json!({"kind": "marker", "process": 1}),
            json!({"kind": "kill", "process": 1, "pid": 1}),
            json!({"kind": "reconcile_reply", "process": 2}),
            json!({"kind": "event", "process": 2, "run_id": "T-resume", "event_seq": 1, "event_type": "model_call"}),
        ],
    );
    for number in [1, 2] {
        fs::write(dir.join(format!("sidecar-{number}.log")), b"{}\n--- stderr ---\n").unwrap();
    }
    fs::write(dir.join("run-manifest.json"), b"{}").unwrap();
    fs::create_dir(dir.join("workspace")).unwrap();
    write(
        &dir.join("trial.json"),
        &json!({
            "version": 1, "task_id": TASK, "session_path": session, "workspace": dir.join("workspace"),
            "trial_log": dir.join("trial-log.jsonl"), "host_receipts": receipts,
            "evidence_dirs": [first, second], "operation_dirs": [ops],
            "sidecar_logs": [dir.join("sidecar-1.log"), dir.join("sidecar-2.log")],
            "fences": {"start": dir.join("fence-start.json"), "before_resume": dir.join("fence-before-resume.json"),
                       "after_resume": null},
            "journal_snapshots": [snapshot], "tools": ["ws_write_file"],
            "kill": {"native_call_id": OPEN_CALL, "pid": 1},
            "resume": {"attempted": true, "refused": false, "error": null, "rejected": null},
            "stop": "AwaitingHostAcceptance", "error": null,
        }),
    );
    Fixture { _temp: temp, dir }
}

fn status(record: &LongTrialRecord, name: &str) -> CheckStatus {
    record.checks.iter().find(|check| check.name == name).unwrap().status
}

fn failed_only(record: &LongTrialRecord, name: &str) {
    let failed: Vec<&str> =
        record.checks.iter().filter(|check| check.status == CheckStatus::Failed).map(|check| check.name).collect();
    assert_eq!(failed, vec![name], "{:#?}", record.checks);
    assert!(matches!(record.verdict, TrialVerdict::Failed { .. }), "{:?}", record.verdict);
    let failure = record.failure_record.as_ref().expect("a failed trial writes a Failure_Record");
    let value = read(failure);
    assert_eq!(value["task_id"], TASK);
    assert!(value["detail"].as_str().unwrap().contains(name), "{value}");
}

fn class(record: &LongTrialRecord) -> String {
    let value = read(record.failure_record.as_ref().unwrap());
    value["classification"]["class"].as_str().unwrap_or("pending").to_owned()
}

#[test]
fn a_complete_trial_passes_with_every_event_counted() {
    let fixture = passing();
    let record = fixture.check();
    assert_eq!(record.verdict, TrialVerdict::Passed, "{:#?}", record.checks);
    assert_eq!(record.label, "passed");
    assert!(record.checks.iter().all(|check| check.status == CheckStatus::Passed), "{:#?}", record.checks);
    let counts = &record.coverage.counts;
    assert_eq!(
        (counts["compaction"], counts["host_review_continuation"], counts["length_continuation"], counts["resume"]),
        (1, 2, 1, 1)
    );
    let kinds: Vec<(&str, &str, Option<u64>)> =
        record.coverage.events.iter().map(|event| (event.event_type, event.run_id.as_str(), event.seq)).collect();
    assert!(kinds.contains(&("compaction", "T-run-1", Some(2))), "{kinds:?}");
    assert!(kinds.contains(&("length_continuation", "T-run-2", Some(1))), "{kinds:?}");
    assert!(kinds.contains(&("host_review_continuation", "T-run-4", Some(1))), "{kinds:?}");
    assert!(kinds.contains(&("resume", "T-resume", Some(1))), "{kinds:?}");
    assert!(record.failure_record.is_none());
    // Expected and actual of every check are recorded.
    let written = read(&fixture.path("long-trial.json"));
    for check in written["checks"].as_array().unwrap() {
        assert!(!check["expected"].is_null() && !check["actual"].is_null(), "{check}");
    }
    let calls = &written["checks"].as_array().unwrap().iter().find(|c| c["name"] == "logical_calls").unwrap()["actual"];
    assert_eq!(calls["interrupted_runs"][0]["admitted_calls"], 40);
    assert_eq!(calls["final"], 55);
    // A second check of the same directory never overwrites the first record.
    fixture.check();
    assert!(fixture.path("long-trial-2.json").is_file());
}

#[test]
fn a_changed_session_path_fails_as_a_core_defect() {
    let fixture = passing();
    edit(&fixture.path("evidence-1/run-2-admission.json"), |value| {
        value["session_path"] = json!(fixture.path("state/other.json"))
    });
    let record = fixture.check();
    failed_only(&record, "session_path");
    assert_eq!(class(&record), "core_defect");
    // Every artifact is kept and listed.
    assert!(record.preserved.iter().any(|(label, _)| label == "sidecar_log"));
    assert!(fixture.path("state/session.json").is_file() && fixture.path("trial-log.jsonl").is_file());
}

#[test]
fn a_rewritten_journal_record_fails() {
    let fixture = passing();
    edit(&fixture.path("journal-001-before-resume.json"), |value| {
        value["records"][1]["hash"] = json!("0000");
    });
    let record = fixture.check();
    failed_only(&record, "journal_unchanged");
    assert_eq!(class(&record), "core_defect");
}

#[test]
fn a_changed_truncated_message_reference_fails() {
    let fixture = passing();
    edit(&fixture.path("evidence-1/run-1-stop-snapshot.json"), |value| {
        value["truncated"]["raw_index"] = json!(4);
    });
    let record = fixture.check();
    failed_only(&record, "journal_unchanged");
    let problems = &record.checks.iter().find(|check| check.name == "journal_unchanged").unwrap().problems;
    assert!(problems.iter().any(|problem| problem.contains("truncated message 4")), "{problems:?}");
    assert!(problems.iter().any(|problem| problem.contains("reviewed stop snapshot")), "{problems:?}");
}

#[test]
fn a_compaction_beyond_the_journal_fails() {
    let fixture = passing();
    let path = fixture.path("evidence-1/run-1-events.jsonl");
    let text = fs::read_to_string(&path).unwrap().replace("\"covered_prefix\":2", "\"covered_prefix\":9");
    fs::write(&path, text).unwrap();
    failed_only(&fixture.check(), "journal_unchanged");
}

#[test]
fn a_decreasing_call_count_fails() {
    let fixture = passing();
    edit(&fixture.path("evidence-1/run-3-observation.json"), |value| value["total_calls"] = json!(8));
    failed_only(&fixture.check(), "logical_calls");
}

#[test]
fn an_interrupted_run_not_charged_at_its_cap_fails() {
    let fixture = passing();
    edit(&fixture.path("evidence-2/resume.json"), |value| value["consumed_calls"] = json!(13));
    let record = fixture.check();
    failed_only(&record, "logical_calls");
    let check = record.checks.iter().find(|check| check.name == "logical_calls").unwrap();
    assert!(check.problems.iter().any(|problem| problem.contains("gives 52")), "{:?}", check.problems);
}

#[test]
fn a_call_count_above_max_calls_fails() {
    let fixture = passing();
    let record = check_trial(&fixture.dir, 54, &Credentials::default(), None, Vec::new()).unwrap().0;
    failed_only(&record, "logical_calls");
}

#[test]
fn a_changed_authority_hash_fails() {
    let fixture = passing();
    edit(&fixture.path("fence-before-resume.json"), |value| {
        value["authority"]["declaration_sha256"] = json!("authority-declaration-b")
    });
    failed_only(&fixture.check(), "authority_hash");
}

#[test]
fn a_missing_host_receipt_fails_as_a_wiring_gap() {
    let fixture = passing();
    fs::remove_file(fixture.path("host-receipts/0007-applied.json")).unwrap();
    let record = fixture.check();
    failed_only(&record, "effects_unknown_receipts");
    assert_eq!(class(&record), "wiring_gap");
}

#[test]
fn a_receipt_after_the_first_upstream_request_fails() {
    let fixture = passing();
    let path = fixture.path("trial-log.jsonl");
    let mut lines: Vec<Value> =
        fs::read_to_string(&path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let receipt = lines.remove(1);
    lines.push(receipt);
    log(&path, &lines);
    let record = fixture.check();
    failed_only(&record, "effects_unknown_receipts");
    let check = record.checks.iter().find(|check| check.name == "effects_unknown_receipts").unwrap();
    assert!(check.problems[0].contains("model_call"), "{:?}", check.problems);
}

#[test]
fn a_reissue_before_the_receipt_fails() {
    let fixture = passing();
    let path = fixture.path("trial-log.jsonl");
    let mut lines: Vec<Value> =
        fs::read_to_string(&path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let mut again = lines[0].clone();
    again["native_call_id"] = json!("call-10");
    lines.insert(1, again);
    log(&path, &lines);
    let record = fixture.check();
    failed_only(&record, "effects_unknown_receipts");
    let check = record.checks.iter().find(|check| check.name == "effects_unknown_receipts").unwrap();
    assert!(check.problems[0].contains("re-issued"), "{:?}", check.problems);
}

#[test]
fn a_refused_resume_fails_and_names_the_refusal() {
    let fixture = passing();
    write(
        &fixture.path("evidence-2/jsonl-resume-rejected.json"),
        &json!({"category": "receipt_uncommitted", "no_replay": true}),
    );
    edit(&fixture.path("trial.json"), |value| {
        value["resume"] = json!({"attempted": true, "refused": true,
                                 "error": "ReconciliationRequired: runtime disconnected", "rejected": null});
    });
    let record = fixture.check();
    failed_only(&record, "resume_accepted");
    assert!(matches!(record.verdict, TrialVerdict::Failed { resume_refused: true, .. }));
    assert_eq!(class(&record), "wiring_gap");
}

#[test]
fn too_few_events_without_a_failed_check_is_coverage_insufficient() {
    let fixture = passing();
    // No compaction, one review continuation, never killed or resumed.
    let path = fixture.path("evidence-1/run-1-events.jsonl");
    let text = fs::read_to_string(&path).unwrap().replace("\"type\":\"compact\"", "\"type\":\"context_status\"");
    fs::write(&path, text).unwrap();
    for name in ["jsonl-rpc-3-request.json", "jsonl-rpc-3-reply.json", "run-4-admission.json", "run-4-events.jsonl"] {
        fs::remove_file(fixture.path(&format!("evidence-1/{name}"))).unwrap();
    }
    write(
        &fixture.path("evidence-1/outcome.json"),
        &json!({"task_id": TASK, "stop": "BudgetExhausted",
                                                           "observed_calls": 12}),
    );
    edit(&fixture.path("trial.json"), |value| {
        value["evidence_dirs"] = json!([fixture.path("evidence-1")]);
        value["kill"] = Value::Null;
        value["resume"] = Value::Null;
        value["fences"]["before_resume"] = Value::Null;
    });
    let record = fixture.check();
    assert_eq!(record.label, COVERAGE_INSUFFICIENT);
    let TrialVerdict::CoverageInsufficient { missing, .. } = &record.verdict else {
        panic!("{:?} {:#?}", record.verdict, record.checks)
    };
    let missing: Vec<(&str, usize)> = missing.iter().map(|item| (item.event_type, item.actual)).collect();
    assert_eq!(missing, vec![("compaction", 0), ("host_review_continuation", 1), ("resume", 0)]);
    assert_eq!(status(&record, "effects_unknown_receipts"), CheckStatus::NotApplicable);
    assert_eq!(status(&record, "resume_accepted"), CheckStatus::NotApplicable);
    assert!(record.failure_record.is_none());
}

#[test]
fn a_missing_trial_record_fails_without_panicking() {
    let fixture = passing();
    fs::remove_file(fixture.path("trial.json")).unwrap();
    let record = fixture.check();
    assert!(matches!(record.verdict, TrialVerdict::Failed { .. }));
    assert_eq!(status(&record, "evidence_readable"), CheckStatus::Failed);
    assert!(record.failure_record.is_some());
}

#[test]
fn orphaned_sidecars_are_spawned_pids_without_a_later_end_or_kill() {
    let line = |kind: &str, pid: Value| json!({"kind": kind, "process": 1, "pid": pid}).to_string();
    let log = [
        line("sidecar_spawned", json!(11)),
        line("kill", json!(11)),
        line("sidecar_end", json!(11)),
        line("sidecar_spawned", json!(22)),
        line("sidecar_end", json!(22)),
        // The same PID spawned again after its end is open again.
        line("sidecar_spawned", json!(22)),
        line("sidecar_spawned", json!(33)),
        line("sidecar_spawned", json!(33)),
        line("sidecar_end", Value::Null),
        line("host_receipt", json!(44)),
        line("sidecar_spawned", json!(55)),
        line("kill", json!(55)),
        line("sidecar_spawned", json!(66)),
        // The torn last line of a killed driver.
        "{\"seq\": 99, \"kind\": \"sidecar_end\", \"pid\": 6".to_owned(),
    ]
    .join("\n");
    assert_eq!(orphaned_sidecars(&log), vec![22, 33, 66]);
    assert!(orphaned_sidecars("").is_empty());
}

/// LONG-TRIAL runs with `max_output` 2048; the recommended `--context-window`
/// leaves a 40-64 KiB projection budget and the sidecar accepts it.
#[test]
fn the_recommended_context_window_leaves_a_small_projection_budget() {
    use ara_agent_lite_harness::core::model_caps::ModelCaps;
    use lite_eval::{
        host::{cap_window, declared_budget},
        long_trial::{context_window_help, CONTEXT_WINDOW_CAP_RANGE, RECOMMENDED_CONTEXT_WINDOW},
    };
    assert!(CONTEXT_WINDOW_CAP_RANGE.contains(&RECOMMENDED_CONTEXT_WINDOW));
    let capped = ModelCaps {
        context_window: cap_window(Some(1_000_000), RECOMMENDED_CONTEXT_WINDOW),
        max_tokens: 2048,
        input: None,
    };
    let budget = capped.budget(2048).unwrap();
    assert_eq!((budget.projection_bytes, budget.request_bytes), (46080, 92160));
    assert!((40 * 1024..=64 * 1024).contains(&budget.projection_bytes));
    assert_eq!(declared_budget(RECOMMENDED_CONTEXT_WINDOW, 2048).unwrap().projection_bytes, 46080);
    let help = context_window_help();
    assert!(help.contains("Recommended 98304"), "{help}");
    assert!(help.contains("45 KiB projection / 90 KiB request"), "{help}");
}
