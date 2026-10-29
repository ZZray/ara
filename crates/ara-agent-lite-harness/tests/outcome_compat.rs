//! Property 31: TaskOutcome serialization compatibility (requirement 22.5).
//! The new `unknown_effects` / `budget_stop` fields round-trip, are omitted
//! when empty, and old JSON without them still parses with defaults.
use std::path::PathBuf;

use ara_agent_lite_harness::*;
use proptest::prelude::*;
use serde_json::Value;

fn record() -> impl Strategy<Value = UnknownEffectRecord> {
    let evidence = prop_oneof![
        ("[a-z]{1,8}", 0usize..64, "[0-9a-f]{64}").prop_map(|(name, raw_index, message_sha256)| {
            EffectEvidence::Journal {
                session_path: PathBuf::from(format!("/s/{name}.json")),
                raw_index,
                message_sha256,
            }
        }),
        ("[a-z]{1,8}", 1u64..512).prop_map(|(name, seq)| EffectEvidence::Event {
            events_path: PathBuf::from(format!("/e/{name}.jsonl")),
            seq
        }),
    ];
    let host = proptest::option::of(
        ("[a-z]{1,8}", "[0-9a-f]{64}")
            .prop_map(|(name, sha256)| HostReceiptRef { path: PathBuf::from(format!("/r/{name}.json")), sha256 }),
    );
    ("[a-z0-9-]{1,12}", "[a-z0-9-]{1,12}", "[a-z_]{1,12}", evidence, host).prop_map(
        |(run_id, native_call_id, tool, evidence, host_receipt)| UnknownEffectRecord {
            run_id,
            native_call_id,
            tool,
            receipt_ref: EffectReceiptRef { evidence, host_receipt },
        },
    )
}

fn stop() -> impl Strategy<Value = TaskStop> {
    prop_oneof![
        Just(TaskStop::AwaitingHostAcceptance),
        Just(TaskStop::NeedsChanges),
        Just(TaskStop::BudgetExhausted),
        Just(TaskStop::Cancelled),
        Just(TaskStop::GoalWaiting),
        Just(TaskStop::Informational),
        Just(TaskStop::ReconciliationRequired),
    ]
}

fn outcome() -> impl Strategy<Value = TaskOutcome> {
    let budget = proptest::option::of((0usize..64, 0usize..64).prop_map(|(task_max_calls, total_calls)| {
        BudgetStopDetail { trigger: BudgetStopTrigger::CompactionSummary, task_max_calls, total_calls }
    }));
    (
        "[a-z0-9-]{1,12}",
        stop(),
        0usize..8,
        0usize..64,
        "[ -~]{0,40}",
        proptest::option::of("[a-z]{1,8}"),
        proptest::collection::vec(record(), 0..4),
        budget,
    )
        .prop_map(|(task_id, stop, runs, observed_calls, detail, fence, unknown_effects, budget_stop)| {
            TaskOutcome {
                task_id,
                stop,
                runs,
                observed_calls,
                last_candidate: None,
                last_receipt: None,
                detail,
                task_accepted: false,
                fence_presence: if fence.is_some() { FencePresence::Present } else { FencePresence::Released },
                reconciliation_fence: fence.map(|name| PathBuf::from(format!("/f/{name}.fence.json"))),
                unknown_effects,
                budget_stop,
            }
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn outcome_round_trips_and_old_json_parses_with_defaults(outcome in outcome()) {
        let wire = serde_json::to_value(&outcome).unwrap();
        // Empty new fields are not serialized at all.
        prop_assert_eq!(wire.get("unknown_effects").is_some(), !outcome.unknown_effects.is_empty());
        prop_assert_eq!(wire.get("budget_stop").is_some(), outcome.budget_stop.is_some());
        // Round trip keeps values and list order.
        let back: TaskOutcome = serde_json::from_value(wire.clone()).unwrap();
        prop_assert_eq!(&back.unknown_effects, &outcome.unknown_effects);
        prop_assert_eq!(&back.budget_stop, &outcome.budget_stop);
        prop_assert_eq!(serde_json::to_value(&back).unwrap(), wire.clone());
        // Old format: the new keys removed.
        let mut old = wire.clone();
        let object = old.as_object_mut().unwrap();
        object.remove("unknown_effects");
        object.remove("budget_stop");
        let legacy: TaskOutcome = serde_json::from_value(old.clone()).unwrap();
        prop_assert!(legacy.unknown_effects.is_empty());
        prop_assert!(legacy.budget_stop.is_none());
        prop_assert_eq!(serde_json::to_value(&legacy).unwrap(), old);
        // Pretty bytes (as outcome.json is written) parse to the same value.
        let pretty = serde_json::to_vec_pretty(&outcome).unwrap();
        prop_assert_eq!(serde_json::from_slice::<Value>(&pretty).unwrap(), wire);
    }
}
