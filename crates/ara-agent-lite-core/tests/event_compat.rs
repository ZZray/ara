//! Requirements 28.1, 28.2, 28.7: every pre-change event `type` has samples
//! and a named variant; the version constant is positive; and `execute`,
//! still called through the `serde_json::Value` channel used before
//! `LiteEvent` existed, sends the same event count, order, `type` values and
//! field-name sets as the golden captured with the samples (DR-P4-1).
mod event_scenarios;
mod support;

use std::collections::BTreeSet;

use ara_agent_lite_core::{LiteEvent, EVENT_SCHEMA_VERSION};
use event_scenarios::*;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// SHA-256 of the committed sample set (DR-P4-1); the samples are never regenerated.
const SAMPLE_SHA256: &[(&str, &str)] = &[
    ("closed_budget.jsonl", "cac3a7a0643a4e30445e1ce30594076d975c748b2ce433c779630766b4857ce7"),
    ("compaction.jsonl", "579caa14bb264bb751cfc2185a35ebd99866aa3ecfac70acad51479a9498feb3"),
    ("compaction_refresh.jsonl", "5a24d1821c73d68529664c9dc4ae3cb377c779218d8fc942f2098c10b9f4805a"),
    ("completion.jsonl", "0485de05539718ec993e9cba3bc9168d9d1c9e2182dc9c5616e3ea91e251f6cc"),
    ("completion_gate.jsonl", "b7275859b63628e9166e852ca99c950564da434d2e4cc429e5e627558e56c521"),
    ("execute-golden.json", "20c62425232fdc9387c0d16dc67c221f78614881e6a59f56bdbd087e2abcb7dd"),
    ("generation_policy.jsonl", "1e49a1e42eeee12484b1756a2a0c5e84d1eac70e3da12bd25fd82481071a1f28"),
    ("goal.jsonl", "c307c4ce80cbf0ea5fd4631b9ef15ad23e6496fdcf2b4ad2342916649b0ee978"),
    ("length_replan.jsonl", "14c61b3a6d8428756d548048cb1e868ea420623f52a9d58c957ebaf4cd6896ca"),
    ("length_stop_evidence.jsonl", "30f0fa70edc89a1703cd71662145525ac7f5ec54634fa8699fe8ead40ed30d50"),
    ("output_limit.jsonl", "090b944343e087990ab8d1e66f6e29585d0334615d6fec27395cccacf86dd948"),
    ("stuck_detector.jsonl", "8ac630fb7641e9788e1818fdbf1f69bdf4cd7794fcb62756da592cb5f8051b13"),
    ("summary_budget_stop.jsonl", "4336320e30893a62caae0aebc0b3d703e30153abd85f26026f492b2a8c94544d"),
    ("tools.jsonl", "cf950a8df5f21a168885bc217a9a353fd5a8fcb87fc7971a430f9b163739c379"),
];

const _: () = assert!(EVENT_SCHEMA_VERSION >= 1);

fn read(name: &str) -> Vec<u8> {
    std::fs::read(fixtures().join(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn sample_events() -> Vec<Value> {
    SCENARIOS
        .iter()
        .flat_map(|name| {
            let text = String::from_utf8(read(&format!("{name}.jsonl"))).unwrap();
            text.lines().map(|line| serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>()
        })
        .collect()
}

/// The pinned stream adapter reports a turn's `model_result` (sent when the
/// stream settles) and its `text_delta` events (sent as the agent consumes
/// the stream) from two tasks, so their relative order is not fixed in the
/// code these samples come from. Canonical order: each `model_result` before
/// the `text_delta` run it raced with. All other order is compared as is.
fn canonical(run: &Value) -> Vec<Value> {
    let mut events = run.as_array().unwrap().clone();
    for index in 0..events.len() {
        if events[index]["type"] == "model_result" {
            let mut at = index;
            while at > 0 && events[at - 1]["type"] == "text_delta" {
                events.swap(at - 1, at);
                at -= 1;
            }
        }
    }
    events
}

#[test]
fn sample_files_are_the_recorded_capture() {
    let mut names: Vec<String> =
        std::fs::read_dir(fixtures()).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(names, SAMPLE_SHA256.iter().map(|(name, _)| name.to_string()).collect::<Vec<_>>());
    for (name, sha256) in SAMPLE_SHA256 {
        assert_eq!(format!("{:x}", Sha256::digest(read(name))), *sha256, "{name} changed");
    }
}

#[test]
fn every_event_type_has_samples_and_a_named_variant() {
    assert_eq!(LiteEvent::KNOWN_TYPES, EVENT_TYPES, "one named variant per DR-P4-1 type");
    let events = sample_events();
    let seen: BTreeSet<&str> = events.iter().map(|event| event["type"].as_str().unwrap()).collect();
    for kind in EVENT_TYPES {
        assert!(seen.contains(kind), "no sample for `{kind}`");
    }
    assert_eq!(seen.len(), EVENT_TYPES.len(), "a sampled type without a variant: {seen:?}");
    for event in events {
        let decoded = LiteEvent::try_from(event.clone()).unwrap();
        assert!(!matches!(decoded, LiteEvent::Unknown(_)), "{event}");
        assert_eq!(decoded.event_type(), event["type"].as_str());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn value_execute_matches_the_golden_event_shapes() {
    let golden: Value = serde_json::from_slice(&read("execute-golden.json")).unwrap();
    assert_eq!(golden["version"], 1);
    let scenarios = golden["scenarios"].as_object().unwrap();
    assert_eq!(scenarios.len(), SCENARIOS.len());
    for name in SCENARIOS {
        let runs = scenario(name).await;
        let expected = scenarios[*name].as_array().unwrap();
        assert_eq!(runs.len(), expected.len(), "{name}: run count");
        for (index, (events, expected)) in runs.iter().zip(expected).enumerate() {
            assert_eq!(canonical(&shape(events)), canonical(expected), "{name} run {index}");
            for event in events {
                let decoded = LiteEvent::try_from(event.clone()).unwrap_or_else(|error| panic!("{name}: {error}"));
                assert!(!matches!(decoded, LiteEvent::Unknown(_)), "{name}: {event}");
                assert_eq!(&Value::from(decoded), event, "{name}");
            }
        }
    }
}
