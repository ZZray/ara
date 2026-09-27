//! Streaming tool-call argument parsing (OMP `packages/utils/src/json-parse.ts`).
//!
//! Upstream `parseStreamingJson` repairs a partial buffer with a relaxed parser
//! and falls back to `{}`. ARA uses the repaired value only for live display.
//! The final arguments of a completed call are parsed strictly; invalid JSON
//! becomes `{__parseError, __rawJson}` so validation reports it instead of a
//! tool running on guessed arguments (intentional difference).

use crate::types::JsonObject;
use serde_json::Value;

/// Strict JSON state used only to route identifierless Responses deltas.
/// Source: pinned OMP `packages/utils/src/json-parse.ts::classifyJsonPrefix`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JsonPrefixState {
    Complete,
    Prefix,
    Invalid,
}

pub(crate) fn classify_json_prefix(text: &str) -> JsonPrefixState {
    use JsonPrefixState::{Complete, Invalid, Prefix};
    #[derive(Clone, Copy)]
    enum Expect {
        Value,
        ObjectKeyOrEnd,
        ObjectKey,
        ObjectColon,
        ObjectCommaOrEnd,
        ArrayValueOrEnd,
        ArrayCommaOrEnd,
        End,
    }
    fn after_value(stack: &[bool]) -> Expect {
        match stack.last() {
            Some(true) => Expect::ObjectCommaOrEnd,
            Some(false) => Expect::ArrayCommaOrEnd,
            None => Expect::End,
        }
    }
    fn scan_string(bytes: &[u8], i: &mut usize) -> Result<(), JsonPrefixState> {
        *i += 1;
        while *i < bytes.len() {
            match bytes[*i] {
                b'"' => {
                    *i += 1;
                    return Ok(());
                }
                b'\\' => {
                    *i += 1;
                    if *i == bytes.len() {
                        return Err(JsonPrefixState::Prefix);
                    }
                    let escape = bytes[*i];
                    if !matches!(escape, b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' | b'u') {
                        return Err(JsonPrefixState::Invalid);
                    }
                    *i += 1;
                    if escape == b'u' {
                        for _ in 0..4 {
                            if *i == bytes.len() {
                                return Err(JsonPrefixState::Prefix);
                            }
                            if !bytes[*i].is_ascii_hexdigit() {
                                return Err(JsonPrefixState::Invalid);
                            }
                            *i += 1;
                        }
                    }
                }
                0..=0x1f => return Err(JsonPrefixState::Invalid),
                _ => *i += 1,
            }
        }
        Err(JsonPrefixState::Prefix)
    }
    fn scan_number(bytes: &[u8], i: &mut usize) -> Result<(), JsonPrefixState> {
        if bytes[*i] == b'-' {
            *i += 1;
        }
        if *i == bytes.len() {
            return Err(JsonPrefixState::Prefix);
        }
        match bytes[*i] {
            b'0' => *i += 1,
            b'1'..=b'9' => {
                while *i < bytes.len() && bytes[*i].is_ascii_digit() {
                    *i += 1;
                }
            }
            _ => return Err(JsonPrefixState::Invalid),
        }
        if *i < bytes.len() && bytes[*i] == b'.' {
            *i += 1;
            if *i == bytes.len() {
                return Err(JsonPrefixState::Prefix);
            }
            if !bytes[*i].is_ascii_digit() {
                return Err(JsonPrefixState::Invalid);
            }
            while *i < bytes.len() && bytes[*i].is_ascii_digit() {
                *i += 1;
            }
        }
        if *i < bytes.len() && matches!(bytes[*i], b'e' | b'E') {
            *i += 1;
            if *i < bytes.len() && matches!(bytes[*i], b'+' | b'-') {
                *i += 1;
            }
            if *i == bytes.len() {
                return Err(JsonPrefixState::Prefix);
            }
            if !bytes[*i].is_ascii_digit() {
                return Err(JsonPrefixState::Invalid);
            }
            while *i < bytes.len() && bytes[*i].is_ascii_digit() {
                *i += 1;
            }
        }
        Ok(())
    }
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut stack = Vec::new(); // true: object, false: array
    let mut expect = Expect::Value;
    while i < bytes.len() {
        let c = bytes[i];
        if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
            continue;
        }
        match expect {
            Expect::Value | Expect::ArrayValueOrEnd => {
                if c == b']' && matches!(expect, Expect::ArrayValueOrEnd) {
                    stack.pop();
                    i += 1;
                    expect = after_value(&stack);
                    continue;
                }
                if c == b'{' {
                    stack.push(true);
                    i += 1;
                    expect = Expect::ObjectKeyOrEnd;
                    continue;
                }
                if c == b'[' {
                    stack.push(false);
                    i += 1;
                    expect = Expect::ArrayValueOrEnd;
                    continue;
                }
                let result = if c == b'"' {
                    scan_string(bytes, &mut i)
                } else if c == b'-' || c.is_ascii_digit() {
                    scan_number(bytes, &mut i)
                } else {
                    let word: &[u8] = match c {
                        b't' => b"true",
                        b'f' => b"false",
                        b'n' => b"null",
                        _ => return Invalid,
                    };
                    let available = word.len().min(bytes.len() - i);
                    if bytes[i..i + available] != word[..available] {
                        return Invalid;
                    }
                    i += available;
                    if available == word.len() { Ok(()) } else { Err(Prefix) }
                };
                if let Err(state) = result {
                    return state;
                }
                expect = after_value(&stack);
            }
            Expect::ObjectKeyOrEnd | Expect::ObjectKey => {
                if c == b'}' && matches!(expect, Expect::ObjectKeyOrEnd) {
                    stack.pop();
                    i += 1;
                    expect = after_value(&stack);
                    continue;
                }
                if c != b'"' {
                    return Invalid;
                }
                if let Err(state) = scan_string(bytes, &mut i) {
                    return state;
                }
                expect = Expect::ObjectColon;
            }
            Expect::ObjectColon => {
                if c != b':' {
                    return Invalid;
                }
                i += 1;
                expect = Expect::Value;
            }
            Expect::ObjectCommaOrEnd => {
                if c == b'}' {
                    stack.pop();
                    i += 1;
                    expect = after_value(&stack);
                } else if c == b',' {
                    i += 1;
                    expect = Expect::ObjectKey;
                } else {
                    return Invalid;
                }
            }
            Expect::ArrayCommaOrEnd => {
                if c == b']' {
                    stack.pop();
                    i += 1;
                    expect = after_value(&stack);
                } else if c == b',' {
                    i += 1;
                    expect = Expect::Value;
                } else {
                    return Invalid;
                }
            }
            Expect::End => return Invalid,
        }
    }
    if matches!(expect, Expect::End) { Complete } else { Prefix }
}

/// Best-effort object for a possibly incomplete JSON buffer.
pub fn parse_streaming_json(partial: &str) -> JsonObject {
    let trimmed = partial.trim_start();
    if trimmed.is_empty() {
        return JsonObject::new();
    }
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(trimmed) {
        return map;
    }
    match serde_json::from_str::<Value>(&complete_partial_json(trimmed)) {
        Ok(Value::Object(map)) => map,
        _ => JsonObject::new(),
    }
}

/// Final arguments of a completed tool call.
pub fn parse_final_arguments(raw: &str) -> JsonObject {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return JsonObject::new();
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(Value::Object(map)) => map,
        Ok(other) => parse_error_args(&format!("expected a JSON object, got {}", json_kind(&other)), raw),
        Err(err) => parse_error_args(&err.to_string(), raw),
    }
}

fn parse_error_args(error: &str, raw: &str) -> JsonObject {
    let mut map = JsonObject::new();
    map.insert("__parseError".into(), Value::String(error.to_string()));
    map.insert("__rawJson".into(), Value::String(raw.to_string()));
    map
}

pub fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Close open strings, arrays and objects of a truncated JSON document.
fn complete_partial_json(input: &str) -> String {
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for ch in input.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                stack.pop();
            }
            _ => {}
        }
    }
    let mut out = input.to_string();
    if in_string {
        if escaped {
            out.pop();
        }
        out.push('"');
    }
    let trimmed_len = out.trim_end().len();
    out.truncate(trimmed_len);
    if out.ends_with(',') {
        out.pop();
    }
    if out.ends_with(':') {
        out.push_str("null");
    }
    while let Some(close) = stack.pop() {
        out.push(close);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strict_prefix_matches_pinned_omp_cases() {
        use JsonPrefixState::{Complete, Invalid, Prefix};
        for (input, expected) in [
            ("", Prefix),
            (" \t\n\r", Prefix),
            (r#"{"command":"echo "#, Prefix),
            (r#"{"command":"echo {1..3}"}"#, Complete),
            (r#"{"a":[1,{"b":true},null]}"#, Complete),
            (r#"{"a":[1,{"b":"#, Prefix),
            ("{\"command\":\"echo hello\n", Invalid),
            ("{\"a\":1}{", Invalid),
            ("{1..3}", Invalid),
            ("{\"a\":\"\\", Prefix),
            ("{\"a\":\"\\u12", Prefix),
            ("{\"a\":\"\\q\"}", Invalid),
            ("{\"a\":01}", Invalid),
            ("12", Complete),
        ] {
            assert_eq!(classify_json_prefix(input), expected, "{input:?}");
        }
    }

    #[test]
    fn repairs_truncated_buffers_for_display() {
        assert_eq!(Value::Object(parse_streaming_json(r#"{"path": "a/b"#)), json!({"path": "a/b"}));
        assert_eq!(Value::Object(parse_streaming_json(r#"{"a": 1, "b": ["x","#)), json!({"a": 1, "b": ["x"]}));
        assert_eq!(Value::Object(parse_streaming_json(r#"{"a":"#)), json!({"a": null}));
        assert_eq!(Value::Object(parse_streaming_json("")), json!({}));
        assert_eq!(Value::Object(parse_streaming_json("not json")), json!({}));
    }

    #[test]
    fn final_arguments_are_strict() {
        assert_eq!(Value::Object(parse_final_arguments(r#"{"a":1}"#)), json!({"a": 1}));
        let bad = parse_final_arguments(r#"{"a":1"#);
        assert!(bad.contains_key("__parseError"));
        assert_eq!(bad["__rawJson"], json!(r#"{"a":1"#));
        assert!(parse_final_arguments("[1]").contains_key("__parseError"));
        assert!(parse_final_arguments("  ").is_empty());
    }
}
