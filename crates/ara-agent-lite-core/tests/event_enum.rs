//! Property 38 (requirements 28.3, 28.4, 28.5): every pre-change event sample,
//! and samples given extra unknown fields or an unknown `type`, decode to
//! `LiteEvent` and re-encode to JSON equal to the input (`Value` equality:
//! key order ignored, integers stay integers).
use ara_agent_lite_core::LiteEvent;
use proptest::prelude::*;
use serde_json::{Map, Value};

/// Every line of `tests/fixtures/events-v1/*.jsonl`.
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

fn round_trip(input: &Value) -> (LiteEvent, Value) {
    let event = LiteEvent::try_from(input.clone()).unwrap_or_else(|error| panic!("{error}: {input}"));
    // The serde impls agree with TryFrom/From. (Through `Value`: serde_json
    // text parsing without `float_roundtrip` may change a float's last bit.)
    let serialized = serde_json::to_value(&event).unwrap();
    assert_eq!(&serialized, &Value::from(event.clone()));
    assert_eq!(serde_json::from_value::<LiteEvent>(serialized).unwrap(), event);
    (event.clone(), Value::from(event))
}

#[test]
fn every_sample_line_round_trips_through_its_named_variant() {
    let samples = samples();
    assert!(samples.len() > 100, "sample set present");
    for sample in samples {
        let (event, output) = round_trip(sample);
        assert!(!matches!(event, LiteEvent::Unknown(_)), "{sample}");
        assert_eq!(event.event_type(), sample["type"].as_str());
        // `Number` equality also distinguishes 1 from 1.0.
        assert_eq!(&output, sample);
    }
}

fn json_leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        (-1.0e9f64..1.0e9).prop_map(Value::from),
        "[a-z0-9 _\u{4e2d}]{0,12}".prop_map(Value::String),
    ]
}

fn json_value() -> impl Strategy<Value = Value> {
    json_leaf().prop_recursive(3, 16, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map("[a-z_]{1,6}", inner, 0..4)
                .prop_map(|map| Value::Object(map.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

/// Field names no event uses: every known field is lowercase without `zz_`.
fn unknown_fields(min: usize) -> impl Strategy<Value = std::collections::BTreeMap<String, Value>> {
    prop::collection::btree_map("zz_[a-z0-9_]{1,8}", json_value(), min..4)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn unknown_fields_are_kept_on_the_named_variant(
        index in any::<prop::sample::Index>(),
        added in unknown_fields(1),
    ) {
        let samples = samples();
        let mut input = samples[index.index(samples.len())].clone();
        for (key, value) in &added {
            input[key] = value.clone();
        }
        let (event, output) = round_trip(&input);
        prop_assert!(!matches!(event, LiteEvent::Unknown(_)));
        prop_assert_eq!(event.event_type(), input["type"].as_str());
        prop_assert_eq!(&output, &input);
        let object = output.as_object().unwrap();
        for (key, value) in &added {
            prop_assert_eq!(object.get(key), Some(value));
        }
    }

    #[test]
    fn an_unknown_type_keeps_the_complete_original_json(
        index in any::<prop::sample::Index>(),
        kind in "[a-z_]{1,20}",
        added in unknown_fields(0),
    ) {
        prop_assume!(!LiteEvent::KNOWN_TYPES.contains(&kind.as_str()));
        let samples = samples();
        let mut input = samples[index.index(samples.len())].clone();
        input["type"] = Value::String(kind);
        for (key, value) in added {
            input[key] = value;
        }
        let (event, output) = round_trip(&input);
        prop_assert_eq!(&event, &LiteEvent::Unknown(input.clone()));
        prop_assert_eq!(&output, &input);
    }
}
