//! Executes the Host's native RegExp seam against retained Bun/JSC results.
use ara_cli::js_regex::JsRegExp;
use ara_rpc::WireString;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn units(value: &Value) -> WireString {
    WireString::from_units(serde_json::from_value(value.clone()).expect("UTF-16 corpus"))
}
fn capture(value: Option<&WireString>) -> Value {
    value.map_or_else(|| json!({"status":"undefined"}), |value| json!({"status":"string","units":value.units()}))
}
fn replay(case: &Value) -> Value {
    let mut regexp = match JsRegExp::new(units(&case["sourceUnits"]), case["flags"].as_str().unwrap()) {
        Ok(regexp) => regexp,
        Err(error) => {
            return json!({"status":"constructor-error","name":error.name,"message":error.message});
        }
    };
    let mut out = json!({"status":"ok","sourceUnits":regexp.source().units(),
        "flags":regexp.flags(),"initialLastIndex":regexp.last_index(),"steps":[]});
    let mut steps = Vec::new();
    for step in case["steps"].as_array().unwrap() {
        if let Some(value) = step.get("setLastIndex") {
            regexp.set_last_index(value.as_f64().unwrap());
        }
        let Some(found) = regexp.exec(&units(&step["inputUnits"])) else {
            steps.push(json!({"status":"no-match","lastIndex":regexp.last_index()}));
            continue;
        };
        let groups = found.groups.as_ref().map(|groups| {
            groups.iter().map(|(key, value)| (key.to_utf8().unwrap(), capture(value.as_ref()))).collect::<Map<_, _>>()
        });
        let indices = found.indices.as_ref().map(|ranges| {
            let groups = found.group_indices.as_ref().map(|groups| {
                groups.iter().map(|(key, range)| {
                    (key.to_utf8().unwrap(), json!(range.as_ref().map(|range| [range.start, range.end])))
                }).collect::<Map<_, _>>()
            });
            json!({"ranges":ranges.iter().map(|range| range.as_ref().map(|range| [range.start,range.end])).collect::<Vec<_>>(),"groups":groups})
        });
        steps.push(json!({"status":"match","lastIndex":regexp.last_index(),"match":{
            "wholeUnits":found.captures[0].as_ref().unwrap().units(),
            "captures":found.captures[1..].iter().map(|value|capture(value.as_ref())).collect::<Vec<_>>(),
            "index":found.index,"groups":groups,"indices":indices}}));
    }
    out["steps"] = json!(steps);
    out
}
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b)),
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len() && a.iter().all(|(key, value)| b.get(key).is_some_and(|other| same(value, other)))
        }
        _ => a == b,
    }
}
fn inventory(oracle: &Value) -> Result<(), &'static str> {
    if oracle["schemaVersion"] != 1 || oracle["runtime"]["name"] != "bun" || oracle["runtime"]["version"] != "1.4.0" {
        return Err("wrong runtime/schema");
    }
    let cases = oracle["cases"].as_array().ok_or("missing cases")?;
    let expected: BTreeMap<_, _> = [
        ("source", 10),
        ("flags", 2),
        ("invalid", 16),
        ("fold", 18),
        ("line", 22),
        ("capture", 5),
        ("lookaround", 4),
        ("unicode-set", 12),
        ("modifier", 3),
        ("state", 11),
        ("empty", 9),
        ("surrogate", 18),
    ]
    .into_iter()
    .collect();
    let mut counts = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for case in cases {
        *counts.entry(case["family"].as_str().ok_or("missing family")?).or_insert(0) += 1;
        if !ids.insert(case["id"].as_str().ok_or("missing id")?) {
            return Err("duplicate case");
        }
    }
    if counts != expected || cases.len() != 130 {
        return Err("incomplete/unknown corpus");
    }
    Ok(())
}
fn load() -> Value {
    let path = std::env::var_os("ARA_JS_REGEX_ORACLE").expect("generate scripts/js_regex_oracle.py first");
    let bytes = std::fs::read(path).unwrap();
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    let actual = digest.as_ref().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    assert_eq!(
        actual, "c596f0006a1c3a41b8e698668b4848726afc80d0592851a42676482372be4ff2",
        "complete frozen original corpus"
    );
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
#[ignore = "requires retained Bun 1.4.0 artifact in ARA_JS_REGEX_ORACLE"]
fn native_regexp_compares_with_bun() {
    let oracle = load();
    inventory(&oracle).expect("complete native regex corpus");
    let mut differences = Vec::new();
    for case in oracle["cases"].as_array().unwrap() {
        let actual = replay(case);
        if !same(&actual, &case["expected"]) {
            differences.push(json!({"id":case["id"],"input":case,"actual":actual}));
        }
    }
    if let Some(path) = std::env::var_os("ARA_JS_REGEX_MISMATCHES") {
        std::fs::write(path, serde_json::to_vec_pretty(&differences).unwrap()).unwrap();
    }
    assert!(
        differences.is_empty(),
        "{} native RegExp differences: {}",
        differences.len(),
        json!(differences.iter().map(|case| &case["id"]).collect::<Vec<_>>())
    );
}

#[test]
#[ignore = "requires retained Bun 1.4.0 artifact in ARA_JS_REGEX_ORACLE"]
fn malformed_native_regexp_corpus_is_rejected() {
    let mut oracle = load();
    inventory(&oracle).expect("baseline corpus");
    let original = oracle["cases"][0]["id"].clone();
    oracle["cases"][0]["id"] = oracle["cases"][1]["id"].clone();
    assert!(inventory(&oracle).is_err(), "duplicate id");
    oracle["cases"][0]["id"] = original;
    oracle["cases"].as_array_mut().unwrap().pop();
    assert!(inventory(&oracle).is_err(), "missing case");
}
