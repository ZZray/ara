//! Property 39 (requirement 28.6): input without `type`, with a non-string
//! `type`, or with a known `type` whose required field is missing or has the
//! wrong JSON type is rejected with an error naming the reason. Decoding
//! never panics and never falls back to `Unknown` or a default value.
use ara_agent_lite_core::{events::EventDecodeError, LiteEvent};
use proptest::prelude::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

fn samples() -> &'static [Value] {
    static SAMPLES: std::sync::OnceLock<Vec<Value>> = std::sync::OnceLock::new();
    SAMPLES.get_or_init(|| {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/events-v1");
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect();
        files.sort();
        files
            .iter()
            .flat_map(|path| std::fs::read_to_string(path).unwrap().lines().map(str::to_owned).collect::<Vec<_>>())
            .map(|line| serde_json::from_str(&line).unwrap())
            .collect()
    })
}

/// Per `type`: fields present in every sample (required) and the JSON kinds
/// each field takes across samples.
struct Fields {
    required: BTreeMap<String, BTreeSet<String>>,
    kinds: BTreeMap<String, BTreeMap<String, BTreeSet<&'static str>>>,
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn fields() -> &'static Fields {
    static FIELDS: std::sync::OnceLock<Fields> = std::sync::OnceLock::new();
    FIELDS.get_or_init(|| {
        let mut required: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut kinds: BTreeMap<String, BTreeMap<String, BTreeSet<&'static str>>> = BTreeMap::new();
        for sample in samples() {
            let object = sample.as_object().unwrap();
            let event_type = object["type"].as_str().unwrap().to_owned();
            let keys: BTreeSet<String> = object.keys().filter(|key| *key != "type").cloned().collect();
            required.entry(event_type.clone()).and_modify(|set| set.retain(|key| keys.contains(key))).or_insert(keys);
            for (key, value) in object.iter().filter(|(key, _)| *key != "type") {
                kinds.entry(event_type.clone()).or_default().entry(key.clone()).or_default().insert(kind(value));
            }
        }
        Fields { required, kinds }
    })
}

/// One value of each JSON kind; floats are non-integral.
fn values_of_kind(kind: &str) -> Value {
    match kind {
        "null" => Value::Null,
        "bool" => json!(true),
        "float" => json!(1.5),
        "int" => json!(7),
        "string" => json!("x"),
        "array" => json!(["x"]),
        _ => json!({"x": 1}),
    }
}

const KINDS: [&str; 7] = ["null", "bool", "float", "int", "string", "array", "object"];

fn assert_field_error(input: &Value, expected_type: &str) -> Result<(), TestCaseError> {
    match LiteEvent::try_from(input.clone()) {
        Err(EventDecodeError::Field { event_type, detail }) => {
            prop_assert_eq!(event_type, expected_type);
            prop_assert!(!detail.is_empty());
        }
        other => prop_assert!(false, "expected a field error for {input}, got {other:?}"),
    }
    // The serde entry point reports the same failure.
    prop_assert!(serde_json::from_value::<LiteEvent>(input.clone()).is_err());
    Ok(())
}

#[test]
fn every_known_type_has_required_fields_from_the_samples() {
    let fields = fields();
    assert_eq!(fields.required.len(), LiteEvent::KNOWN_TYPES.len());
    for event_type in LiteEvent::KNOWN_TYPES {
        assert!(!fields.required[*event_type].is_empty(), "{event_type}");
    }
}

#[test]
fn non_objects_and_missing_or_non_string_type_are_rejected() {
    for value in [json!(null), json!(1), json!("agent_start"), json!([{"type":"text"}]), json!({})] {
        assert_eq!(LiteEvent::try_from(value), Err(EventDecodeError::MissingType));
    }
    let error = serde_json::from_str::<LiteEvent>(r#"{"type":["text"],"output":"x"}"#).unwrap_err();
    assert!(error.to_string().contains("not a string"), "{error}");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn missing_type_is_rejected(index in any::<prop::sample::Index>()) {
        let mut input = samples()[index.index(samples().len())].clone();
        input.as_object_mut().unwrap().remove("type");
        prop_assert_eq!(LiteEvent::try_from(input.clone()), Err(EventDecodeError::MissingType));
        prop_assert!(serde_json::from_value::<LiteEvent>(input).is_err());
    }

    #[test]
    fn non_string_type_is_rejected(index in any::<prop::sample::Index>(), kind in 0usize..7) {
        prop_assume!(KINDS[kind] != "string");
        let mut input = samples()[index.index(samples().len())].clone();
        input["type"] = values_of_kind(KINDS[kind]);
        prop_assert_eq!(LiteEvent::try_from(input), Err(EventDecodeError::TypeNotString));
    }

    #[test]
    fn missing_required_field_is_rejected(index in any::<prop::sample::Index>(), pick in any::<prop::sample::Index>()) {
        let input = &samples()[index.index(samples().len())];
        let event_type = input["type"].as_str().unwrap();
        let required: Vec<_> = fields().required[event_type].iter().collect();
        let mut broken = input.clone();
        broken.as_object_mut().unwrap().remove(required[pick.index(required.len())]);
        assert_field_error(&broken, event_type)?;
    }

    #[test]
    fn wrong_json_type_is_rejected(
        index in any::<prop::sample::Index>(),
        pick in any::<prop::sample::Index>(),
        replacement in 0usize..7,
    ) {
        let input = &samples()[index.index(samples().len())];
        let event_type = input["type"].as_str().unwrap();
        let present: Vec<&String> = input.as_object().unwrap().keys().filter(|key| *key != "type").collect();
        let field = present[pick.index(present.len())];
        let replacement = KINDS[replacement];
        // Only a kind no sample of this type ever used for the field is wrong.
        prop_assume!(!fields().kinds[event_type][field].contains(replacement));
        let mut broken = input.clone();
        broken[field.as_str()] = values_of_kind(replacement);
        assert_field_error(&broken, event_type)?;
    }
}
