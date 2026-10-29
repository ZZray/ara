//! Examples of structured unknown-effect judgment (requirements 22.2, 22.6,
//! 22.7) over the real Core `execute` and a loopback provider.
mod common;

use std::sync::{atomic::Ordering, Arc};

use ara_agent_lite_harness::*;
use common::*;
use serde_json::{json, Value};

async fn wait(handle: TaskHandle) -> Arc<TaskOutcome> {
    tokio::time::timeout(std::time::Duration::from_secs(60), handle.wait()).await.unwrap()
}

/// A builtin effect tool fails with Core's own "partial effects may remain"
/// diagnostic. Core types it once; the Harness reads only the structured
/// status, cancels the Run, joins it and never replays the command.
#[tokio::test]
async fn builtin_exec_timeout_is_unknown_through_structured_status_only() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let marker = root.join("exec-marker.txt");
    let cmd = if cfg!(windows) {
        format!("Add-Content -LiteralPath '{}' -Value started; Start-Sleep -Seconds 30", marker.display())
    } else {
        format!("echo started >> '{}'; sleep 30", marker.display())
    };
    let provider = Provider::new(vec![
        call("exec-1", "exec", json!({"cmd":cmd,"timeout":1})),
        write_answer("must-not-matter"),
        text("Submitted."),
    ]);
    let host = Arc::new(Host::new(&root, &provider, Fault::None));
    let mut request = host.request("exec-run");
    request.enable_local_tools = true;
    let outcome = wait(start(host.spec("exec"), request, host.clone()).unwrap()).await;
    assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
    assert!(!outcome.task_accepted);
    assert_eq!(outcome.runs, 1);
    assert_eq!((host.reviews.load(Ordering::SeqCst), host.preparations.load(Ordering::SeqCst)), (0, 0));
    let rows = events(&root.join("exec/run-1-events.jsonl"));
    let end = rows
        .iter()
        .find(|row| row["event"]["type"] == "tool_end" && row["event"]["step_id"] == "exec-1")
        .expect("exec tool_end");
    assert_eq!(end["event"]["status"], "failed");
    assert_eq!(end["event"]["effects_unknown"], true, "{end}");
    let records: Vec<_> = outcome.unknown_effects.iter().filter(|record| record.native_call_id == "exec-1").collect();
    assert_eq!(records.len(), 1, "{outcome:?}");
    assert_eq!((records[0].run_id.as_str(), records[0].tool.as_str()), ("exec-run", "exec"));
    // When the result is durable, its typed status is Core's builtin contract.
    if let EffectEvidence::Journal { session_path, raw_index, .. } = &records[0].receipt_ref.evidence {
        let messages =
            core::session_journal::Journal::for_session(session_path).read(*raw_index, raw_index + 1, 1 << 23).unwrap();
        let details = serde_json::to_value(&messages[0]).unwrap().to_string();
        assert!(details.contains("core_builtin_error_contract"), "{details}");
    }
    // The original Run was joined; the command ran exactly once.
    let observation: Value =
        serde_json::from_slice(&std::fs::read(root.join("exec/run-1-observation.json")).unwrap()).unwrap();
    assert_eq!(observation["core_joined"], true);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let started = std::fs::read_to_string(&marker).unwrap_or_default();
    assert_eq!(started.lines().filter(|line| line.trim() == "started").count(), 1, "{started:?}");
    assert!(outcome.reconciliation_fence.as_ref().unwrap().is_file());
}

/// A literal phrase in a custom tool's successful result is data, not status.
#[tokio::test]
async fn custom_tool_phrase_text_stays_known() {
    let directory = tempfile::tempdir().unwrap();
    let provider =
        Provider::new(vec![call("read", "read_terms", json!({})), write_answer("correct"), text("Submitted.")]);
    let mut host = Host::new(directory.path(), &provider, Fault::None);
    host.read_tool = Some(Arc::new(ReadTerms::new(false)));
    let host = Arc::new(host);
    let outcome = wait(start(host.spec("phrase"), host.request("initial"), host.clone()).unwrap()).await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert!(outcome.unknown_effects.is_empty());
}

/// Reconciliation for a non-tool reason keeps its stop and detail and
/// publishes an empty list, which `outcome.json` omits entirely.
#[tokio::test]
async fn non_tool_reconciliation_has_an_empty_unknown_effect_list() {
    for (fault, name, detail) in [
        (Fault::Unknown, "review-unknown", "Host review is uncommitted, unknown, or bound to another candidate"),
        (
            Fault::Uncommitted,
            "review-uncommitted",
            "Host review is uncommitted, unknown, or bound to another candidate",
        ),
        (Fault::TamperEvents, "tampered", "harness stage failed; inspect original evidence"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let provider = Provider::new(vec![write_answer("correct"), text("Submitted.")]);
        let host = Arc::new(Host::new(directory.path(), &provider, fault));
        let outcome = wait(start(host.spec(name), host.request("initial"), host.clone()).unwrap()).await;
        assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{name}: {outcome:?}");
        assert_eq!(outcome.detail, detail, "{name}");
        assert!(outcome.unknown_effects.is_empty(), "{name}");
        assert!(outcome.budget_stop.is_none(), "{name}");
        let published: Value =
            serde_json::from_slice(&std::fs::read(directory.path().join(name).join("outcome.json")).unwrap()).unwrap();
        assert!(published.get("unknown_effects").is_none(), "{name}: {published}");
        assert!(published.get("budget_stop").is_none(), "{name}");
        assert_eq!(published["stop"], "ReconciliationRequired");
    }
}
