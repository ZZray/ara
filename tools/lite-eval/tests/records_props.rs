//! Property 12: evidence paths are complete and never overwritten
//! (requirements 4.5, 36.1).

use std::path::Path;

use lite_eval::{
    common::{new_run_dir, write_json_new, Credentials},
    metrics::{checked_paths, PathRecord, REQUIRED_PATHS},
};
use proptest::prelude::*;
use serde_json::{json, Value};

/// Every `{"status": …}` object in `value`.
fn records(value: &Value, out: &mut Vec<Value>) {
    match value {
        Value::Object(map) if map.contains_key("status") => out.push(value.clone()),
        Value::Object(map) => map.values().for_each(|v| records(v, out)),
        Value::Array(items) => items.iter().for_each(|v| records(v, out)),
        _ => {}
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn every_path_exists_or_is_missing_with_a_reason(
        // (exists at record time, still exists at write time) per entry
        entries in prop::collection::vec((any::<bool>(), any::<bool>()), 0..8),
        keys in prop::collection::vec(prop::sample::select(vec!["artifacts", "session", "events", "oracle"]), 0..8),
        reruns in 1usize..5,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let mut paths = serde_json::Map::new();
        let mut events = Vec::new();
        for (index, (exists, survives)) in entries.iter().enumerate() {
            let path = dir.path().join(format!("p{index}"));
            if *exists {
                std::fs::write(&path, b"x").unwrap();
            }
            let record = serde_json::to_value(PathRecord::of(&path, "not produced")).unwrap();
            if *exists && !*survives {
                std::fs::remove_file(&path).unwrap();
            }
            match keys.get(index) {
                Some(&"events") | None => events.push(record),
                Some(key) => {
                    paths.insert((*key).to_owned(), record);
                }
            }
        }
        if !events.is_empty() {
            paths.insert("events".into(), Value::Array(events));
        }
        let checked = checked_paths(Value::Object(paths), "task did not run");
        for key in REQUIRED_PATHS {
            prop_assert!(!checked[key].is_null(), "{key} is always recorded");
        }
        let mut all = Vec::new();
        records(&checked, &mut all);
        prop_assert!(!all.is_empty());
        for record in all {
            match record["status"].as_str() {
                Some("present") => prop_assert!(Path::new(record["path"].as_str().unwrap()).exists()),
                Some("missing") => {
                    prop_assert!(record.get("path").is_none(), "a missing path names no path");
                    prop_assert!(!record["reason"].as_str().unwrap_or_default().is_empty());
                }
                other => prop_assert!(false, "unexpected status {other:?}"),
            }
        }

        // Reruns: a new directory each time, and an existing file is never replaced.
        let root = dir.path().join("evidence");
        let mut seen = std::collections::BTreeSet::new();
        let credentials = Credentials::default();
        for rerun in 0..reruns {
            let run = new_run_dir(&root).unwrap();
            prop_assert!(seen.insert(run.clone()), "directory reused");
            let file = run.join("result.json");
            write_json_new(&file, &json!({"rerun": rerun}), &credentials).unwrap();
            let before = std::fs::read(&file).unwrap();
            let again = write_json_new(&file, &json!({"rerun": "again"}), &credentials);
            prop_assert!(again.is_err(), "an existing evidence file was overwritten");
            prop_assert_eq!(std::fs::read(&file).unwrap(), before);
        }
        for run in &seen {
            let value: Value = serde_json::from_slice(&std::fs::read(run.join("result.json")).unwrap()).unwrap();
            prop_assert!(value["rerun"].is_u64());
        }
    }
}
