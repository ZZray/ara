//! JavaScript value semantics the upstream template engine relies on
//! (`Boolean()`, `String()`, `JSON.stringify`, `===`, relational operators,
//! the `\s` whitespace set), over `serde_json::Value`.
//!
//! `Value::Null` stands for JS `undefined`: a missing key and a host's
//! `Option::None` both arrive as `Null`, where the TypeScript contexts
//! upstream renders leave the property `undefined`. So `String(Null)` is
//! `"undefined"` and `ToNumber(Null)` is `NaN`; nested `null`s still print as
//! `null` in `JSON.stringify`.

use serde_json::{Number, Value};

/// JS `\s` / `trim` whitespace: WhiteSpace plus LineTerminator code points.
pub fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

pub fn trim_end(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

pub fn trim_start(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

pub fn trim(s: &str) -> &str {
    trim_end(trim_start(s))
}

fn number_f64(n: &Number) -> f64 {
    n.as_f64().unwrap_or(f64::NAN)
}

/// JS `Boolean(value)`.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            let f = number_f64(n);
            f != 0.0 && !f.is_nan()
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// JS `Number.prototype.toString()` for finite values in the usual range.
pub fn number_to_string(n: &Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    let f = number_f64(n);
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "Infinity".into() } else { "-Infinity".into() }
    } else if f == f.trunc() && f.abs() < 1e21 {
        format!("{f:.0}")
    } else {
        format!("{f}")
    }
}

/// JS `String(value)` (`undefined` → "undefined"; arrays join with `,`).
pub fn to_string(v: &Value) -> String {
    match v {
        Value::Null => "undefined".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number_to_string(n),
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| if item.is_null() { String::new() } else { to_string(item) })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// JS `JSON.stringify(value)` (compact; integral floats print as integers).
pub fn json_stringify(v: &Value) -> String {
    match v {
        Value::Number(n) => {
            let f = number_f64(n);
            if n.is_f64() && !f.is_finite() { "null".into() } else { number_to_string(n) }
        }
        Value::Array(items) => format!("[{}]", items.iter().map(json_stringify).collect::<Vec<_>>().join(",")),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}:{}", Value::String(k.clone()), json_stringify(v)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => other.to_string(),
    }
}

/// JS `===` for primitives; distinct objects/arrays are never identical.
pub fn strict_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => number_f64(x) == number_f64(y),
        (Value::String(x), Value::String(y)) => x == y,
        _ => false,
    }
}

/// JS `ToNumber`.
pub fn to_number(v: &Value) -> f64 {
    match v {
        Value::Null => f64::NAN,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => number_f64(n),
        Value::String(s) => string_to_number(s),
        Value::Array(_) => to_number(&Value::String(to_string(v))),
        Value::Object(_) => f64::NAN,
    }
}

/// JS `StringToNumber`: decimal literals, `Infinity`, and unsigned
/// `0x`/`0o`/`0b` integers; anything else is `NaN` (Rust's `inf`/`nan`
/// spellings are not JS numbers).
fn string_to_number(s: &str) -> f64 {
    let t = trim(s);
    if t.is_empty() {
        return 0.0;
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let radix = match t.get(..2) {
        Some("0x" | "0X") => Some(16),
        Some("0o" | "0O") => Some(8),
        Some("0b" | "0B") => Some(2),
        _ => None,
    };
    if let Some(radix) = radix {
        let digits = &t[2..];
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return f64::NAN;
        }
        return digits.chars().fold(0.0, |acc, c| acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0)));
    }
    if !t.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-')) {
        return f64::NAN;
    }
    t.parse().unwrap_or(f64::NAN)
}

/// JS `ToPrimitive` for plain data: arrays and objects become their `String()`.
fn to_primitive(v: &Value) -> Value {
    match v {
        Value::Array(_) | Value::Object(_) => Value::String(to_string(v)),
        other => other.clone(),
    }
}

/// JS abstract relational comparison: `None` when either side is `NaN`
/// (every `<`, `>`, `<=`, `>=` is then false). Strings compare by UTF-16 units.
pub fn compare(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    let (a, b) = (to_primitive(a), to_primitive(b));
    if let (Value::String(x), Value::String(y)) = (&a, &b) {
        return Some(x.encode_utf16().cmp(y.encode_utf16()));
    }
    to_number(&a).partial_cmp(&to_number(&b))
}

/// JS binary `+`: string concatenation when either primitive is a string.
pub fn add(a: &Value, b: &Value) -> Value {
    let (a, b) = (to_primitive(a), to_primitive(b));
    if a.is_string() || b.is_string() {
        return Value::String(format!("{}{}", to_string(&a), to_string(&b)));
    }
    number(to_number(&a) + to_number(&b))
}

/// JS `Number.parseInt(String(value), 10)`.
pub fn parse_int(v: &Value) -> f64 {
    let text = to_string(v);
    let text = trim_start(&text);
    let (sign, digits) = match text.as_bytes().first() {
        Some(b'-') => (-1.0, &text[1..]),
        Some(b'+') => (1.0, &text[1..]),
        _ => (1.0, text),
    };
    let end = digits.bytes().position(|b| !b.is_ascii_digit()).unwrap_or(digits.len());
    if end == 0 {
        return f64::NAN;
    }
    sign * digits[..end].parse::<f64>().unwrap_or(f64::NAN)
}

/// JS string `length` (UTF-16 code units).
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// A JSON number from an f64, integral values as integers.
pub fn number(f: f64) -> Value {
    if f == f.trunc() && f.abs() < 9.0e15 {
        Value::Number(Number::from(f as i64))
    } else {
        Number::from_f64(f).map_or(Value::Null, Value::Number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn js_semantics() {
        assert!(!truthy(&json!(0)) && !truthy(&json!("")) && truthy(&json!([])) && truthy(&json!({})));
        assert_eq!(to_string(&json!([1, null, "a", [2, 3]])), "1,,a,2,3");
        assert_eq!(to_string(&json!(null)), "undefined");
        assert!(to_number(&json!(null)).is_nan() && to_number(&json!([null])) == 0.0);
        assert_eq!(to_number(&json!([" 7 "])), 7.0);
        assert_eq!(to_number(&json!("0x1F")), 31.0);
        assert_eq!(to_number(&json!("-Infinity")), f64::NEG_INFINITY);
        assert!(to_number(&json!("inf")).is_nan() && to_number(&json!("1e")).is_nan());
        assert_eq!(to_number(&json!(".5e1")), 5.0);
        assert_eq!(to_string(&json!(2.0)), "2");
        assert_eq!(to_string(&json!(1.5)), "1.5");
        assert_eq!(json_stringify(&json!({"a": [1, 2.0, "x"], "b": null})), r#"{"a":[1,2,"x"],"b":null}"#);
        assert!(strict_eq(&json!(1), &json!(1.0)) && !strict_eq(&json!([]), &json!([])));
        use std::cmp::Ordering::*;
        assert_eq!(compare(&json!("a"), &json!("b")), Some(Less));
        assert_eq!(compare(&json!(1), &json!("2")), Some(Less));
        assert_eq!(compare(&json!("x"), &json!(1)), None);
        assert_eq!(compare(&json!(["b"]), &json!("a")), Some(Greater));
        assert_eq!(add(&json!(1), &json!(2)), json!(3));
        assert_eq!(add(&json!("a"), &json!(1)), json!("a1"));
        assert_eq!(parse_int(&json!(" -12px")), -12.0);
        assert!(parse_int(&json!("x1")).is_nan());
        assert_eq!(utf16_len("é😀"), 3);
    }
}
