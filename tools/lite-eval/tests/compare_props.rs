//! Property 28: before/after comparison conclusions (requirements 20.7,
//! 21.2, 21.3, 21.4, 21.5, 21.6).

use std::path::PathBuf;

use lite_eval::{
    compare::{
        compare, BaselineView, CodeView, Comparability, Conclusions, Delta, Loaded, ManifestRef, ManifestView,
        PolicyRef, Presence, SelectedView, Side, TaskView,
    },
    host::WriteCounts,
    metrics::{Completeness, CorrectnessMetrics, CostRecord, Counts, TaskMetrics, UsageTotals},
    policy::RunPolicy,
};
use proptest::prelude::*;
use serde_json::{json, Value};

const IDS: [&str; 3] = ["H2", "S1", "S3"];

fn policy() -> RunPolicy {
    let budget =
        json!({"max_runs": 2, "max_calls": 8, "deadline_seconds": 60, "run_max_calls": 4, "run_max_seconds": 30});
    serde_json::from_value(json!({
        "version": 1, "policy_id": "p",
        "model": {"id": "m", "protocol": "openai-completions",
                  "route": {"endpoint": "http://127.0.0.1:9/v1", "binding": "b", "decision": "d"},
                  "resolution": "catalog", "credential_env": "KEY"},
        "probe": {"timeout_ms": 1000},
        "budgets": {"H2": budget, "S1": budget, "S3": budget},
        "generation": {"max_output": 4096, "generation_policy": "provider_default", "length_recovery": "stop"},
        "price": {"currency": null, "input_per_million": null, "output_per_million": null},
        "oracle_timeout_seconds": 600, "consumer_task": "S1"
    }))
    .unwrap()
}

#[derive(Clone, Debug)]
struct Sample {
    judged: bool,
    success: bool,
    hidden: Option<u64>,
    calls: Option<usize>,
    wall: Option<u64>,
    output: Option<u64>,
}

fn sample() -> impl Strategy<Value = Sample> {
    (
        any::<bool>(),
        any::<bool>(),
        prop::option::of(0u64..4),
        prop::option::of(0usize..4),
        prop::option::of(0u64..4),
        prop::option::of(1u64..4),
    )
        .prop_map(|(judged, success, hidden, calls, wall, output)| Sample {
            judged,
            success: judged && success,
            hidden,
            calls,
            wall,
            output,
        })
}

fn metrics(sample: &Sample) -> TaskMetrics {
    let usage = match sample.output {
        Some(output) => UsageTotals {
            status: Completeness::Complete,
            input_tokens: Some(10),
            output_tokens: Some(output),
            unknown_calls: 0,
            reason: None,
            calls: Vec::new(),
        },
        None => UsageTotals {
            status: Completeness::Unknown,
            input_tokens: None,
            output_tokens: None,
            unknown_calls: 1,
            reason: Some("x".into()),
            calls: Vec::new(),
        },
    };
    TaskMetrics {
        success: sample.success,
        stop: Some("NeedsChanges".into()),
        correctness: CorrectnessMetrics {
            known: sample.judged,
            passed: sample.judged.then_some(sample.success),
            public: None,
            hidden: if sample.judged { sample.hidden.map(|passed| Counts { passed, total: 4 }) } else { None },
            holdout: None,
            reason: (!sample.judged).then(|| "Oracle error".into()),
        },
        logical_calls: sample.calls,
        wall_ms: sample.wall,
        usage,
        cost: CostRecord {
            status: Completeness::Unknown,
            amount: None,
            currency: None,
            basis: None,
            reason: Some("price unknown".into()),
        },
        write_counts: WriteCounts::default(),
        failure_class: (!sample.success).then(|| "pending".into()),
    }
}

fn manifest(id: &str, samples: &[(&str, Option<Sample>)]) -> ManifestView {
    ManifestView {
        manifest_id: id.into(),
        status: "completed".into(),
        run_policy: PolicyRef { sha256: "a".repeat(64) },
        policy: Some(policy()),
        code_version: CodeView {
            head: "c".repeat(40),
            dirty: false,
            worktree_diff_sha256: None,
            diff_path: None,
            capability_host_features: Vec::new(),
        },
        all_tasks: IDS
            .iter()
            .map(|id| TaskView {
                task_id: (*id).into(),
                fixture_sha256: Some(format!("{id}-sha")),
                holdout_status_at_start: json!({"status": "not new holdout"}),
            })
            .collect(),
        requested: IDS.iter().map(|id| (*id).to_owned()).collect(),
        selected: samples
            .iter()
            .map(|(task, sample)| SelectedView {
                task_id: (*task).into(),
                state: if sample.is_some() { "failed".into() } else { "refused".into() },
                metrics: sample.as_ref().map(metrics),
                failure_record: None,
            })
            .collect(),
        baseline: Some(BaselineView { usable: true }),
    }
}

fn loaded(view: ManifestView) -> Loaded {
    Loaded {
        reference: ManifestRef {
            manifest: PathBuf::from(format!("{}.json", view.manifest_id)),
            manifest_id: view.manifest_id.clone(),
            sha256: "0".repeat(64),
        },
        view,
    }
}

/// One side of one task: per manifest, `None` = task refused (no metrics).
type Side3 = Vec<Vec<Option<Sample>>>;

fn sides() -> impl Strategy<Value = (Side3, Side3)> {
    let side = (1usize..=3)
        .prop_flat_map(|n| prop::collection::vec(prop::collection::vec(prop::option::weighted(0.85, sample()), 3), n));
    (side.clone(), side)
}

fn mutate(view: &mut ManifestView, which: u8) -> &'static str {
    let policy = view.policy.as_mut().unwrap();
    match which {
        0 => {
            view.run_policy.sha256 = "b".repeat(64);
            "run_policy.sha256"
        }
        1 => {
            policy.model.id = "other".into();
            "model.id"
        }
        2 => {
            policy.model.route.endpoint = "http://127.0.0.1:10/v1".into();
            "model.route"
        }
        3 => {
            policy.model.protocol = "openrouter-chat".into();
            "model.protocol"
        }
        4 => {
            policy.budgets.get_mut("S1").unwrap().max_calls = 9;
            "budgets.S1"
        }
        5 => {
            policy.generation.max_output = 8192;
            "generation"
        }
        6 => {
            view.requested.pop();
            "task_ids"
        }
        _ => {
            view.all_tasks[1].fixture_sha256 = Some("changed".into());
            "fixture_sha256.S1"
        }
    }
}

/// Independent model of one side's value: None = unknown, Some(None) = absent.
fn expected_value(samples: &[&Sample], metric: &str) -> Option<Option<f64>> {
    let values: Vec<Option<Option<f64>>> = samples
        .iter()
        .map(|s| match metric {
            "success" => s.judged.then_some(Some(if s.success { 1.0 } else { 0.0 })),
            "hidden_passed" => s.judged.then_some(s.hidden.map(|n| n as f64)),
            "logical_calls" => s.calls.map(|n| Some(n as f64)),
            "wall_ms" => s.wall.map(|n| Some(n as f64)),
            "usage_input_tokens" => s.output.map(|_| Some(10.0)),
            "usage_output_tokens" => s.output.map(|n| Some(n as f64)),
            _ => None,
        })
        .collect();
    if values.iter().all(|v| *v == Some(None)) {
        return Some(None);
    }
    let known: Vec<f64> = values.iter().filter_map(|v| v.flatten()).collect();
    (known.len() == values.len()).then(|| Some(known.iter().sum::<f64>() / known.len() as f64))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn conclusions_follow_comparability_presence_and_known_values(
        (base, cur) in sides(),
        mismatch in prop::option::weighted(0.25, 0u8..8),
        unavailable in prop::option::weighted(0.1, any::<bool>()),
    ) {
        let build = |prefix: &str, side: &Side3| -> Vec<Loaded> {
            side.iter().enumerate().map(|(n, tasks)| {
                let samples: Vec<(&str, Option<Sample>)> = IDS.iter().copied().zip(tasks.iter().cloned()).collect();
                loaded(manifest(&format!("{prefix}{n}"), &samples))
            }).collect()
        };
        let mut bases = build("b", &base);
        let mut currents = build("c", &cur);
        let field = mismatch.map(|which| mutate(&mut currents[0].view, which));
        match unavailable {
            Some(true) => bases[0].view.status = "environment_unavailable".into(),
            Some(false) => currents[0].view.status = "environment_unavailable".into(),
            None => {}
        }
        let record = compare(&bases, &currents, "P2-a");

        if unavailable.is_some() {
            let unavailable = matches!(record.comparability, Comparability::EnvironmentUnavailable { .. });
            prop_assert!(unavailable, "environment unavailable");
            prop_assert!(record.tasks.is_empty());
            prop_assert_eq!(record.conclusions, Conclusions::None);
            return Ok(());
        }
        if let Some(field) = field {
            let Comparability::NotComparable { mismatches } = &record.comparability else {
                return Err(TestCaseError::fail(format!("{field} differs but {:?}", record.comparability)));
            };
            let hit = mismatches.iter().find(|m| m.field == field || m.field.starts_with(&format!("{field}@")));
            prop_assert!(hit.is_some(), "{field} not listed in {mismatches:?}");
            prop_assert_ne!(&hit.unwrap().baseline, &hit.unwrap().current);
            prop_assert!(record.tasks.is_empty(), "no conclusion when not comparable");
            prop_assert_eq!(record.conclusions, Conclusions::None);
            return Ok(());
        }
        prop_assert_eq!(&record.comparability, &Comparability::Comparable);
        prop_assert_eq!(record.conclusions, Conclusions::Valid);
        prop_assert_eq!(record.tasks.len(), IDS.len());
        for (index, task) in record.tasks.iter().enumerate() {
            prop_assert_eq!(task.task_id.as_str(), IDS[index]);
            let collect = |side: &Side3| side.iter().filter_map(|tasks| tasks[index].clone()).collect::<Vec<Sample>>();
            let (b, c) = (collect(&base), collect(&cur));
            prop_assert_eq!((task.samples.baseline, task.samples.current), (b.len(), c.len()));
            prop_assert_eq!(task.single_sample, b.len() == 1 || c.len() == 1);
            if b.is_empty() || c.is_empty() {
                let side = match (b.is_empty(), c.is_empty()) { (true, true) => Side::Both, (true, false) => Side::Baseline, _ => Side::Current };
                prop_assert_eq!(&task.presence, &Presence::Missing { side });
                prop_assert!(task.metrics.is_empty() && task.regressions.is_empty());
                continue;
            }
            prop_assert_eq!(&task.presence, &Presence::Both);
            let (b, c): (Vec<&Sample>, Vec<&Sample>) = (b.iter().collect(), c.iter().collect());
            let mut regressed = Vec::new();
            for name in ["success", "hidden_passed", "logical_calls", "wall_ms", "usage_input_tokens", "usage_output_tokens"] {
                let (bv, cv) = (expected_value(&b, name), expected_value(&c, name));
                let got = task.metrics.get(name);
                match (bv, cv) {
                    (Some(None), Some(None)) => prop_assert!(got.is_none(), "{name} absent on both sides"),
                    (Some(Some(x)), Some(Some(y))) => {
                        let got = got.unwrap();
                        let higher = matches!(name, "success" | "hidden_passed");
                        let want = if x == y { Delta::Unchanged } else if (y > x) == higher { Delta::Improved } else { Delta::Regressed };
                        prop_assert_eq!(got.delta, want, "{} {} -> {}", name, x, y);
                        prop_assert!(got.unknown_side.is_none());
                        if want == Delta::Regressed {
                            regressed.push(name);
                        }
                    }
                    _ => {
                        let got = got.unwrap();
                        prop_assert_eq!(got.delta, Delta::NotComparable);
                        let side = match (bv.flatten().is_none(), cv.flatten().is_none()) {
                            (true, true) => Side::Both,
                            (true, false) => Side::Baseline,
                            _ => Side::Current,
                        };
                        prop_assert_eq!(got.unknown_side, Some(side));
                        if bv.is_none() { prop_assert_eq!(&got.baseline, &Value::from("unknown")); }
                        if cv.is_none() { prop_assert_eq!(&got.current, &Value::from("unknown")); }
                    }
                }
            }
            let listed: Vec<String> = task.regressions.iter().map(|r| serde_json::to_value(r.metric).unwrap().as_str().unwrap().to_owned()).collect();
            prop_assert_eq!(listed, regressed.iter().map(|s| s.to_string()).collect::<Vec<_>>());
            let class = if c.iter().all(|s| s.success) { "not_applicable" } else { "pending" };
            for regression in &task.regressions {
                prop_assert_eq!(regression.failure_class.as_str(), class);
            }
            prop_assert!(task.metrics.get("cost").is_some_and(|m| m.delta == Delta::NotComparable && m.unknown_side == Some(Side::Both)));
        }
    }
}
