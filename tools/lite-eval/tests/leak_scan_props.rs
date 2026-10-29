// Feature: ara-lite-core-capability, Property 7: 泄漏扫描
//! For any tool-result texts and feature strings, the scan reports a hit iff
//! some feature occurs as a byte substring of some result, and every
//! reported location really contains its feature. `tool_end` outputs read
//! from a Harness events file are scanned the same way; other events are
//! never scanned.

use std::{collections::BTreeSet, fs, io::Write};

use lite_eval::sealed::{event_records, scan, LeakHit, LeakLocation, ScanRecord};
use proptest::prelude::*;

fn config() -> ProptestConfig {
    ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(256) }
}

/// Reference substring test, independent of `scan`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    (0..=haystack.len().saturating_sub(needle.len()))
        .any(|start| haystack.len() >= needle.len() && haystack[start..start + needle.len()] == *needle)
}

/// Small alphabets (one multi-byte character) so hits and near misses are common.
fn features() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec("[abé]{1,3}", 1..4)
}

fn location(index: usize) -> LeakLocation {
    if index.is_multiple_of(2) {
        LeakLocation::Event { run: format!("run-{index}"), seq: index as u64 }
    } else {
        LeakLocation::Journal { session: format!("session-{index}"), raw_index: index }
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn scan_hits_exactly_the_records_containing_a_feature(
        features in features(),
        records in prop::collection::vec(
            prop_oneof![
                "[abcé]{0,10}".prop_map(String::into_bytes),
                prop::collection::vec(prop::sample::select(vec![b'a', b'b', 0xC3, 0xA9, 0xFF]), 0..10),
            ],
            0..6,
        ),
    ) {
        let records: Vec<ScanRecord> = records
            .into_iter()
            .enumerate()
            .map(|(index, bytes)| ScanRecord { location: location(index), bytes })
            .collect();
        let hits = scan(&features, &records);
        let expected: BTreeSet<LeakHit> = records
            .iter()
            .flat_map(|record| {
                features.iter().enumerate().filter(move |(_, feature)| contains(&record.bytes, feature.as_bytes())).map(
                    move |(index, _)| LeakHit { feature: index + 1, location: record.location.clone() },
                )
            })
            .collect();
        prop_assert_eq!(hits.is_empty(), expected.is_empty());
        prop_assert_eq!(hits.len(), expected.len(), "a hit is reported twice");
        prop_assert_eq!(hits.iter().cloned().collect::<BTreeSet<_>>(), expected);
        for hit in &hits {
            let record = records.iter().find(|record| record.location == hit.location).unwrap();
            prop_assert!(contains(&record.bytes, features[hit.feature - 1].as_bytes()));
        }
    }

    #[test]
    fn only_tool_end_outputs_of_an_events_file_are_scanned(
        features in features(),
        events in prop::collection::vec((prop::bool::ANY, "[abcé]{0,10}"), 0..6),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run-1-events.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        for (seq, (tool_end, text)) in events.iter().enumerate() {
            let event = if *tool_end {
                serde_json::json!({"type": "tool_end", "tool_call_id": "c", "output": text})
            } else {
                serde_json::json!({"type": "text_delta", "delta": text})
            };
            let envelope = serde_json::json!({"run_id": "run-1", "seq": seq as u64 + 1, "event": event});
            writeln!(file, "{envelope}").unwrap();
        }
        drop(file);
        let hits: BTreeSet<LeakHit> = scan(&features, &event_records(&path).unwrap()).into_iter().collect();
        let expected: BTreeSet<LeakHit> = events
            .iter()
            .enumerate()
            .filter(|(_, (tool_end, _))| *tool_end)
            .flat_map(|(seq, (_, text))| {
                features.iter().enumerate().filter(move |(_, feature)| text.contains(feature.as_str())).map(
                    move |(index, _)| LeakHit {
                        feature: index + 1,
                        location: LeakLocation::Event { run: "run-1".into(), seq: seq as u64 + 1 },
                    },
                )
            })
            .collect();
        prop_assert_eq!(hits, expected);
    }
}
