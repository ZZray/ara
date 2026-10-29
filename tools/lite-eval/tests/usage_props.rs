//! Property 11: usage and cost aggregation never put 0 in place of unknown
//! (requirements 4.3, 4.4, 4.6, 4.8).

use lite_eval::{
    host::WriteCounts,
    metrics::{
        aggregate_usage, call_tokens, cost, summarize, CallTokens, CallUsage, Completeness, CorrectnessMetrics, Known,
        TaskMetrics,
    },
    policy::Price,
};
use proptest::prelude::*;
use serde_json::json;

fn call() -> impl Strategy<Value = Option<(u64, u64)>> {
    prop::option::of((1u64..1_000_000, 1u64..100_000))
}

fn usage_of(calls: &[Option<(u64, u64)>]) -> Vec<CallUsage> {
    calls
        .iter()
        .enumerate()
        .map(|(index, call)| CallUsage {
            run: 1,
            call: index as u64 + 1,
            phase: "agent".into(),
            usage: match call {
                Some((input, output)) => {
                    Known::Known { value: CallTokens { input_tokens: *input, output_tokens: *output } }
                }
                None => Known::Unknown { reason: "adapter reported no positive usage counts".into() },
            },
        })
        .collect()
}

fn price() -> impl Strategy<Value = Price> {
    (prop::option::of(Just("CNY".to_owned())), prop::option::of(0.0f64..100.0), prop::option::of(0.0f64..100.0))
        .prop_map(|(currency, input_per_million, output_per_million)| Price {
            currency,
            input_per_million,
            output_per_million,
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn task_totals_and_costs_follow_the_known_parts(calls in prop::collection::vec(call(), 0..12), price in price()) {
        let usage = aggregate_usage(&usage_of(&calls));
        let known: Vec<(u64, u64)> = calls.iter().flatten().copied().collect();
        let unknown = calls.len() - known.len();
        prop_assert_eq!(usage.unknown_calls, unknown);
        prop_assert_eq!(usage.calls.len(), calls.len());
        let (input, output) = known.iter().fold((0, 0), |(a, b), (i, o)| (a + i, b + o));
        match usage.status {
            Completeness::Unknown => {
                prop_assert!(!calls.is_empty() && known.is_empty());
                prop_assert!(usage.input_tokens.is_none() && usage.output_tokens.is_none(), "no total, not 0");
                prop_assert!(usage.reason.is_some());
                let encoded = serde_json::to_value(&usage).unwrap();
                prop_assert!(encoded.get("input_tokens").is_none() && encoded.get("output_tokens").is_none());
            }
            Completeness::Incomplete => {
                prop_assert!(!known.is_empty() && unknown > 0);
                prop_assert_eq!(usage.input_tokens, Some(input));
                prop_assert_eq!(usage.output_tokens, Some(output));
            }
            Completeness::Complete => {
                prop_assert_eq!(unknown, 0);
                prop_assert_eq!(usage.input_tokens, Some(input));
                prop_assert_eq!(usage.output_tokens, Some(output));
            }
        }

        let record = cost(&usage, Some(&price));
        let price_known = price.currency.is_some() && price.input_per_million.is_some() && price.output_per_million.is_some();
        if !price_known || usage.status == Completeness::Unknown {
            prop_assert_eq!(record.status, Completeness::Unknown);
            prop_assert!(record.amount.is_none() && record.reason.is_some());
        } else {
            let expected = (input as f64 * price.input_per_million.unwrap()
                + output as f64 * price.output_per_million.unwrap()) / 1e6;
            prop_assert!((record.amount.unwrap() - expected).abs() <= expected.abs() * 1e-12 + 1e-12);
            prop_assert_eq!(record.status, usage.status, "incomplete usage gives an incomplete cost");
        }
        prop_assert_eq!(cost(&usage, None).status, Completeness::Unknown);
    }

    #[test]
    fn summaries_mark_incomplete_and_count_tasks_with_unknown(
        tasks in prop::collection::vec((prop::collection::vec(call(), 0..6), any::<bool>(), price()), 0..8),
    ) {
        let metrics: Vec<TaskMetrics> = tasks
            .iter()
            .map(|(calls, judged, price)| {
                let usage = aggregate_usage(&usage_of(calls));
                TaskMetrics {
                    success: false,
                    stop: None,
                    correctness: CorrectnessMetrics {
                        known: *judged,
                        passed: judged.then_some(false),
                        public: None,
                        hidden: None,
                        holdout: None,
                        reason: (!judged).then(|| "Oracle timed out".into()),
                    },
                    logical_calls: Some(calls.len()),
                    wall_ms: Some(1),
                    cost: cost(&usage, Some(price)),
                    usage,
                    write_counts: WriteCounts::default(),
                    failure_class: Some("pending".into()),
                }
            })
            .collect();
        let summary = summarize(&metrics);
        let not_complete = |status: Completeness| status != Completeness::Complete;
        let usage_unknown = metrics.iter().filter(|m| not_complete(m.usage.status)).count();
        prop_assert_eq!(summary.usage.tasks, metrics.len());
        prop_assert_eq!(summary.usage.tasks_with_unknown, usage_unknown);
        if metrics.is_empty() || metrics.iter().all(|m| m.usage.status == Completeness::Unknown) {
            prop_assert_eq!(summary.usage.status, Completeness::Unknown);
            prop_assert!(summary.usage.input_tokens.is_none());
        } else {
            let expected: u64 = metrics.iter().filter_map(|m| m.usage.input_tokens).sum();
            prop_assert_eq!(summary.usage.input_tokens, Some(expected));
            let status = if usage_unknown > 0 { Completeness::Incomplete } else { Completeness::Complete };
            prop_assert_eq!(summary.usage.status, status);
        }
        let cost_unknown = metrics.iter().filter(|m| not_complete(m.cost.status)).count();
        prop_assert_eq!(summary.cost.tasks_with_unknown, cost_unknown);
        if summary.cost.status == Completeness::Unknown {
            prop_assert!(summary.cost.amount.is_none());
        } else {
            let expected: f64 = metrics.iter().filter_map(|m| m.cost.amount).sum();
            prop_assert!((summary.cost.amount.unwrap() - expected).abs() <= expected.abs() * 1e-12 + 1e-12);
            prop_assert_eq!(cost_unknown > 0, summary.cost.status == Completeness::Incomplete);
        }
        let unjudged = metrics.iter().filter(|m| !m.correctness.known).count();
        prop_assert_eq!(summary.correctness.tasks_with_unknown, unjudged);
        prop_assert_eq!(summary.correctness.status == Completeness::Complete, !metrics.is_empty() && unjudged == 0);
    }

    #[test]
    fn only_positive_adapter_counts_are_known(
        input in prop::option::of(0u64..1000), output in prop::option::of(0u64..1000),
        cache in prop::option::of(0u64..1000), reported in any::<bool>(),
    ) {
        let status = if reported { "adapter_reported_positive_counts" } else { "unknown" };
        let metrics = json!({"status": status, "input_uncached_tokens": input, "output_tokens": output,
                             "cache_read_tokens": cache, "cost": null});
        let positive = |n: Option<u64>| n.filter(|n| *n > 0);
        let input_sum = positive(input).unwrap_or(0) + positive(cache).unwrap_or(0);
        match call_tokens(&metrics) {
            Known::Known { value } => {
                prop_assert!(reported);
                prop_assert_eq!(Some(value.output_tokens), positive(output));
                prop_assert_eq!(value.input_tokens, input_sum);
                prop_assert!(value.input_tokens > 0);
            }
            Known::Unknown { reason } => {
                prop_assert!(!reason.is_empty());
                prop_assert!(!reported || positive(output).is_none() || input_sum == 0);
            }
        }
    }
}
