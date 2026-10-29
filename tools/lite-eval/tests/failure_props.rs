//! Property 13: Failure_Record references and classification invariants
//! (requirements 5.1, 5.2, 5.3).

use std::path::Path;

use ara_agent_lite_harness::TaskStop;
use lite_eval::{
    failure::{
        auto_classify, last_event_refs, validate_refs, Classification, FailureClass, FailureContext, FailureRecord,
        OracleRecord,
    },
    metrics::{load_events, PathRecord},
};
use proptest::prelude::*;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug)]
enum Kind {
    Plain,
    ProviderError,
    ToolMissing,
    AgentEnd,
}

fn event(kind: Kind) -> Value {
    match kind {
        Kind::Plain => json!({"type": "text_delta", "text": "x"}),
        Kind::ProviderError => json!({"type": "model_result", "phase": "agent", "call": 1, "status": "received",
                                       "stop_reason": "error", "usage_metrics": {"status": "unknown"}}),
        Kind::ToolMissing => json!({"type": "tool_end", "step_id": "t", "title": "ws_delete", "status": "failed",
                                     "output": "Tool ws_delete not found"}),
        Kind::AgentEnd => json!({"type": "agent_end", "status": "completed", "code": 0, "detail": ""}),
    }
}

fn kind() -> impl Strategy<Value = Kind> {
    prop_oneof![6 => Just(Kind::Plain), 1 => Just(Kind::ProviderError), 1 => Just(Kind::ToolMissing), 2 => Just(Kind::AgentEnd)]
}

fn stop() -> impl Strategy<Value = Option<TaskStop>> {
    prop::option::of(prop::sample::select(vec![
        TaskStop::AwaitingHostAcceptance,
        TaskStop::NeedsChanges,
        TaskStop::BudgetExhausted,
        TaskStop::Cancelled,
        TaskStop::ReconciliationRequired,
    ]))
}

/// 0 = not executed, 1 = judged failed with raw output, 2 = judged failed
/// without the raw output file, 3 = no judgement.
fn oracle(dir: &Path, mode: u8) -> OracleRecord {
    let raw = dir.join("oracle-result.json");
    if mode == 1 {
        std::fs::write(&raw, b"{}").unwrap();
    }
    let none = OracleRecord {
        executed: false,
        passed: None,
        public: None,
        hidden: None,
        holdout: None,
        raw_output: None,
        reason: Some("Harness did not start".into()),
    };
    match mode {
        1 | 2 => OracleRecord { executed: true, passed: Some(false), raw_output: Some(raw), reason: None, ..none },
        3 => OracleRecord { executed: true, reason: Some("the Oracle gave no judgement".into()), ..none },
        _ => none,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn references_are_the_last_events_and_classes_have_existing_evidence(
        runs in prop::collection::vec(prop::collection::vec(kind(), 0..30), 0..4),
        stop in stop(),
        oracle_mode in 0u8..4,
        core_error in any::<bool>(),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let harness = dir.path().join("harness");
        std::fs::create_dir(&harness).unwrap();
        let mut expected = Vec::new();
        let mut events = Vec::new();
        for (index, kinds) in runs.iter().enumerate() {
            let path = harness.join(format!("run-{}-events.jsonl", index + 1));
            let mut text = String::new();
            for (n, kind) in kinds.iter().enumerate() {
                let seq = n as u64 + 1;
                let line = json!({"task_id": "S1", "run_id": format!("s1-run-{}", index + 1), "seq": seq, "event": event(*kind)});
                text.push_str(&format!("{line}\n"));
                expected.push((path.clone(), seq));
            }
            std::fs::write(&path, text).unwrap();
            events.push(load_events(&path));
        }
        if core_error {
            std::fs::write(harness.join("run-1-core-error.json"), b"{\"category\":\"core_returned_error\"}").unwrap();
        }
        let context = FailureContext {
            task_id: "S1".into(),
            run_ids: (1..=runs.len()).map(|n| format!("s1-run-{n}")).collect(),
            stop,
            detail: String::new(),
            error: None,
            events,
            harness_evidence: Some(harness.clone()),
            artifacts: PathRecord::of(&dir.path().join("workspace"), "not created"),
            oracle: oracle(dir.path(), oracle_mode),
            registered_tools: vec!["ws_read_file".into(), "ws_write_file".into()],
        };

        // References: exactly the last min(20, n) events, in order.
        let refs: Vec<(std::path::PathBuf, u64)> = last_event_refs(&context.events)
            .into_iter()
            .flat_map(|group| group.seq.into_iter().map(move |seq| (group.events_path.clone(), seq)))
            .collect();
        let tail = &expected[expected.len().saturating_sub(20)..];
        prop_assert_eq!(&refs[..], tail);

        let classification = auto_classify(&context);
        prop_assert!(validate_refs(&classification).is_ok(), "{classification:?}");
        let any = |kind: fn(&Kind) -> bool| runs.iter().flatten().any(kind);
        match &classification {
            Classification::Classified { class, evidence } => {
                prop_assert!(!evidence.is_empty());
                let want = if any(|k| matches!(k, Kind::ProviderError)) {
                    FailureClass::WiringGap
                } else if core_error {
                    FailureClass::CoreDefect
                } else if any(|k| matches!(k, Kind::ToolMissing)) {
                    FailureClass::ToolGap
                } else {
                    FailureClass::ModelGap
                };
                prop_assert_eq!(*class, want);
                if *class == FailureClass::ModelGap {
                    prop_assert!(oracle_mode == 1 && any(|k| matches!(k, Kind::AgentEnd)));
                    prop_assert!(matches!(stop, Some(TaskStop::AwaitingHostAcceptance | TaskStop::NeedsChanges)));
                }
            }
            Classification::Pending { missing } => {
                prop_assert!(!missing.is_empty());
                prop_assert!(!any(|k| matches!(k, Kind::ProviderError | Kind::ToolMissing)) && !core_error);
            }
        }
        let record = FailureRecord::build(&context);
        prop_assert_eq!(record.classification, classification);
        prop_assert_eq!(record.run_ids.len(), runs.len());
    }
}
