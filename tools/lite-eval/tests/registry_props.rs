// Feature: ara-lite-core-capability, Property 1: Task_Set 结构校验与参考模型一致
//! Random registrations, written as JSON with fields omitted or empty, are
//! parsed and validated; the violation multiset must equal an independent
//! reference model (task count, duplicate IDs, missing fields, SHA format,
//! positive case counts, S3 public 10/hidden 48 and H2 hidden 128).

use std::collections::BTreeMap;

use lite_eval::registry::{validate, TaskSet, Violation, TASK_COUNT};
use proptest::prelude::*;
use serde_json::{json, Map, Value};

fn config() -> ProptestConfig {
    ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(512) }
}

/// One generated task; `None` means the key is left out of the JSON.
#[derive(Clone, Debug)]
struct Task {
    id: Option<String>,
    kind: Option<&'static str>,
    path: Option<String>,
    sha: Option<String>,
    oracle_type: Option<String>,
    public: Option<i64>,
    hidden: Option<i64>,
    holdout_cases: Option<i64>,
    holdout: Option<Holdout>,
}

#[derive(Clone, Debug)]
enum Holdout {
    New,
    NotNew(Vec<String>),
}

fn id() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(None),
        prop::sample::select(vec!["S1", "S2", "S3", "R1", "H2", "SH1", "X9", "", "  "])
            .prop_map(|s| Some(s.to_owned())),
    ]
}

fn sha() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(None),
        Just(Some(String::new())),
        "[0-9a-f]{64}".prop_map(Some),
        "[0-9A-F]{64}".prop_map(Some),
        "[0-9a-f]{1,63}".prop_map(Some),
        "[0-9a-f]{65}".prop_map(Some),
        "[0-9a-f]{63}[g-z]".prop_map(Some),
    ]
}

fn cases() -> impl Strategy<Value = Option<i64>> {
    prop_oneof![Just(None), prop::sample::select(vec![-3i64, 0, 1, 10, 13, 48, 127, 128]).prop_map(Some)]
}

fn task() -> impl Strategy<Value = Task> {
    (
        id(),
        prop::option::of(prop::sample::select(vec!["Coding", "Reasoning", "ReverseAnalysis"])),
        prop::option::of(prop::sample::select(vec!["", "D:/fixtures/a"]).prop_map(str::to_owned)),
        sha(),
        prop::option::of(prop::sample::select(vec!["", " ", "cpp"]).prop_map(str::to_owned)),
        (cases(), cases(), cases()),
        prop::option::of(prop_oneof![
            Just(Holdout::New),
            prop::collection::vec(prop::sample::select(vec!["", " ", "c1"]).prop_map(str::to_owned), 0..3)
                .prop_map(Holdout::NotNew),
        ]),
    )
        .prop_map(|(id, kind, path, sha, oracle_type, (public, hidden, holdout_cases), holdout)| Task {
            id,
            kind,
            path,
            sha,
            oracle_type,
            public,
            hidden,
            holdout_cases,
            holdout,
        })
}

fn to_json(task: &Task) -> Value {
    let mut entry = Map::new();
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(value) = value {
            entry.insert(key.into(), value);
        }
    };
    put("task_id", task.id.clone().map(Value::from));
    put("kind", task.kind.map(Value::from));
    put("fixture_path", task.path.clone().map(Value::from));
    put("fixture_sha256", task.sha.clone().map(Value::from));
    let mut oracle = Map::new();
    for (key, value) in [
        ("type", task.oracle_type.clone().map(Value::from)),
        ("public_cases", task.public.map(Value::from)),
        ("hidden_cases", task.hidden.map(Value::from)),
        ("holdout_cases", task.holdout_cases.map(Value::from)),
    ] {
        if let Some(value) = value {
            oracle.insert(key.into(), value);
        }
    }
    put("oracle", Some(Value::Object(oracle)));
    put(
        "holdout",
        task.holdout.as_ref().map(|holdout| match holdout {
            Holdout::New => json!({"status": "new holdout", "registered": "2026-10-28"}),
            Holdout::NotNew(changes) => {
                json!({"status": "not new holdout", "first_adaptive_use": "2026-10-23", "changes": changes})
            }
        }),
    );
    put("profile", Some(json!({"mode": "execute", "tools": ["ws_read_file"]})));
    Value::Object(entry)
}

/// Independent statement of requirements 1.1–1.3.
fn reference(tasks: &[Task]) -> Vec<Violation> {
    let mut expected = Vec::new();
    if tasks.len() != TASK_COUNT {
        expected.push(Violation::TaskCount { expected: 6, actual: tasks.len() });
    }
    let mut occurrences: BTreeMap<String, usize> = BTreeMap::new();
    for id in tasks.iter().filter_map(|task| task.id.clone()).filter(|id| !id.trim().is_empty()) {
        *occurrences.entry(id).or_default() += 1;
    }
    for (id, count) in occurrences {
        if count > 1 {
            expected.push(Violation::DuplicateTaskId { task_id: id });
        }
    }
    for (index, task) in tasks.iter().enumerate() {
        let id = task.id.clone().unwrap_or_default();
        let label = if id.trim().is_empty() { format!("#{}", index + 1) } else { id.clone() };
        let blank = |value: &Option<String>| value.as_deref().is_none_or(|text| text.is_empty());
        let mut missing = |field: &'static str| expected.push(Violation::MissingField { task: label.clone(), field });
        if id.trim().is_empty() {
            missing("task_id");
        }
        if task.kind.is_none() {
            missing("kind");
        }
        if blank(&task.path) {
            missing("fixture_path");
        }
        if blank(&task.sha) {
            missing("fixture_sha256");
        }
        if task.oracle_type.as_deref().is_none_or(|text| text.trim().is_empty()) {
            missing("oracle.type");
        }
        if task.public.is_none() && task.hidden.is_none() && task.holdout_cases.is_none() {
            missing("oracle.cases");
        }
        match &task.holdout {
            None => missing("holdout"),
            Some(Holdout::NotNew(changes)) if changes.iter().all(|change| change.trim().is_empty()) => {
                missing("holdout.changes")
            }
            Some(_) => {}
        }
        if let Some(sha) = task.sha.as_deref().filter(|sha| !sha.is_empty()) {
            let ok = sha.len() == 64 && sha.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
            if !ok {
                expected.push(Violation::MalformedSha256 { task: label.clone(), value: sha.to_owned() });
            }
        }
        for (field, value) in [
            ("oracle.public_cases", task.public),
            ("oracle.hidden_cases", task.hidden),
            ("oracle.holdout_cases", task.holdout_cases),
        ] {
            if let Some(value) = value.filter(|value| *value <= 0) {
                expected.push(Violation::NonPositiveCases { task: label.clone(), field, value });
            }
        }
        let required: &[(&'static str, i64, Option<i64>)] = match id.as_str() {
            "S3" => &[("oracle.public_cases", 10, task.public), ("oracle.hidden_cases", 48, task.hidden)],
            "H2" => &[("oracle.hidden_cases", 128, task.hidden)],
            _ => &[],
        };
        for &(field, number, actual) in required {
            if actual != Some(number) {
                expected.push(Violation::WrongCaseCount { task: label.clone(), field, expected: number, actual });
            }
        }
    }
    expected.sort();
    expected
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn validate_matches_the_reference_model(tasks in prop::collection::vec(task(), 0..9)) {
        let document = json!({
            "version": 1,
            "digest_rule": "lite-eval-tree-v1",
            "tasks": tasks.iter().map(to_json).collect::<Vec<_>>(),
        });
        let set: TaskSet = serde_json::from_value(document).expect("missing fields must parse");
        let mut actual = validate(&set);
        actual.sort();
        prop_assert_eq!(actual, reference(&tasks));
    }
}

#[test]
fn a_compliant_six_task_set_has_no_violations() {
    let tasks: Vec<Task> = ["S1", "S2", "S3", "R1", "H2", "SH1"]
        .into_iter()
        .map(|id| Task {
            id: Some(id.into()),
            kind: Some("Coding"),
            path: Some("D:/fixtures/a".into()),
            sha: Some("0".repeat(64)),
            oracle_type: Some("cpp".into()),
            public: (id == "S3").then_some(10),
            hidden: Some(match id {
                "S3" => 48,
                "H2" => 128,
                _ => 13,
            }),
            holdout_cases: None,
            holdout: Some(Holdout::NotNew(vec!["c1".into()])),
        })
        .collect();
    let document = json!({"version": 1, "digest_rule": "lite-eval-tree-v1",
                          "tasks": tasks.iter().map(to_json).collect::<Vec<_>>()});
    let set: TaskSet = serde_json::from_value(document).unwrap();
    assert_eq!(validate(&set), vec![]);
    assert_eq!(reference(&tasks), vec![]);
}
