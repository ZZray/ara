use ara_rpc::json::{WireString, WireValue};

fn round_trip(source: &str, expected: &str) -> WireValue {
    let value = WireValue::parse(source).unwrap();
    assert_eq!(value.stringify(), expected);
    value
}

#[test]
fn utf16_surrogates_and_slicing() {
    let lone_high = round_trip(r#""\ud800""#, r#""\ud800""#);
    assert!(lone_high.as_string().unwrap().to_utf8().is_err());
    round_trip(r#""\udc00""#, r#""\udc00""#);
    let pair = round_trip(r#""\ud83d\ude00""#, "\"😀\"");
    assert_eq!(pair.as_string().unwrap().to_utf8().unwrap(), "😀");
    let split = pair.as_string().unwrap().slice_prefix(1);
    assert_eq!(split.units(), &[0xd83d]);
    assert_eq!(WireValue::String(split).stringify(), r#""\ud83d""#);

    let mut appended = WireString::from("A");
    appended.append_str("😀");
    assert_eq!(appended.len(), 3);
    assert_eq!(appended.slice_prefix(2).units(), &[b'A' as u16, 0xd83d]);
}

#[test]
fn escaped_controls_and_utf16_keys() {
    let value = round_trip(
        r#"{"a\n":"\u0000\b\t\n\f\r\u001f\\\"\/","\ud800":7}"#,
        r#"{"a\n":"\u0000\b\t\n\f\r\u001f\\\"/","\ud800":7}"#,
    );
    assert_eq!(value.get("a\n").unwrap().as_string().unwrap().to_utf8().unwrap(), "\0\x08\t\n\x0c\r\x1f\\\"/");
    assert_eq!(value.entries().unwrap()[1].0.units(), &[0xd800]);
    assert!(value.entries().unwrap()[1].0.to_utf8().is_err());
}

#[test]
fn duplicate_keys_keep_first_slot_and_last_value() {
    let value = round_trip(
        r#"{"b":1,"2":2,"a":3,"1":4,"b":5,"2":6,"4294967295":7,"01":8,"0":9}"#,
        r#"{"0":9,"1":4,"2":6,"b":5,"a":3,"4294967295":7,"01":8}"#,
    );
    assert_eq!(value.get("b").unwrap().as_number(), Some(5.0));
    assert_eq!(value.entries().unwrap().len(), 7);
    let mut object = WireValue::object(vec![("z", WireValue::Bool(false)), ("2", WireValue::Null)]);
    object.insert("z", WireValue::Bool(true));
    assert_eq!(object.stringify(), r#"{"2":null,"z":true}"#);
    round_trip(r#"{"4294967295":1,"4294967294":2,"x":3,"\u0078":4}"#, r#"{"4294967294":2,"4294967295":1,"x":4}"#);
}

#[test]
fn proto_is_an_own_parsed_property() {
    let value = round_trip(r#"{"__proto__":{"x":1},"constructor":2}"#, r#"{"__proto__":{"x":1},"constructor":2}"#);
    assert!(value.get("__proto__").unwrap().is_object());
    assert!(value.get("x").is_none());
}

#[test]
fn javascript_double_precision_and_nonfinite_output() {
    let value = round_trip(
        r#"[9007199254740993,-0,1e400,1e21,1e-7,0.000001]"#,
        "[9007199254740992,0,null,1e+21,1e-7,0.000001]",
    );
    let numbers = value.as_array().unwrap();
    assert_eq!(numbers[0].as_number(), Some(9007199254740992.0));
    assert!(numbers[1].as_number().unwrap().is_sign_negative());
    assert!(numbers[2].as_number().unwrap().is_infinite());
    assert_eq!(WireValue::Number(f64::NAN).stringify(), "null");
    assert_eq!(WireValue::Number(f64::NEG_INFINITY).stringify(), "null");
}

#[test]
fn deep_nesting_parses_serializes_clones_and_drops() {
    let source = format!("{}0{}", "[".repeat(2_048), "]".repeat(2_048));
    let value = WireValue::parse(&source).unwrap();
    assert_eq!(value.stringify(), source);
    let copy = value.clone();
    assert!(value.deep_equal(&copy));
    drop(copy);
    drop(value);
}

#[test]
fn wide_object_keeps_all_keys() {
    let members = (0..2_048).map(|index| format!("\"k{index}\":{index}")).collect::<Vec<_>>().join(",");
    let source = format!("{{{members}}}");
    let value = WireValue::parse(&source).unwrap();
    assert_eq!(value.entries().unwrap().len(), 2_048);
    assert_eq!(value.stringify(), source);
    assert!(value.deep_equal(&value.clone()));
}

#[test]
fn object_equality_ignores_order() {
    let a = WireValue::parse(r#"{"x":[1,{"z":true}],"y":2}"#).unwrap();
    let b = WireValue::parse(r#"{"y":2,"x":[1,{"z":true}]}"#).unwrap();
    assert!(a.deep_equal(&b));
    assert!(!a.deep_equal(&WireValue::parse(r#"{"y":3,"x":[1,{"z":true}]}"#).unwrap()));
    assert!(!WireValue::Number(-0.0).deep_equal(&WireValue::Number(0.0)));
    assert!(WireValue::Number(f64::NAN).deep_equal(&WireValue::Number(f64::NAN)));
}

#[test]
fn rejects_invalid_json() {
    for text in [
        "",
        " ",
        "true false",
        "True",
        "undefined",
        "+1",
        "01",
        "-",
        "1.",
        "1e",
        "1e+",
        "[",
        "[,]",
        "[1,]",
        "[1 2]",
        "{",
        "{x:1}",
        "{\"x\" 1}",
        "{\"x\":1,}",
        "\"unterminated",
        "\"bad\\q\"",
        "\"bad\\u12x4\"",
        "\"bad\nraw\"",
        "null\u{a0}",
    ] {
        assert!(WireValue::parse(text).is_err(), "accepted invalid JSON: {text:?}");
    }
}
