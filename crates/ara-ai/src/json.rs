//! Streaming tool-call argument parsing (OMP `packages/utils/src/json-parse.ts`).
//!
//! Upstream `parseStreamingJson` repairs a partial buffer with a relaxed parser
//! and falls back to `{}`. ARA uses the repaired value only for live display.
//! The final arguments of a completed call are parsed strictly; invalid JSON
//! becomes `{__parseError, __rawJson}` so validation reports it instead of a
//! tool running on guessed arguments (intentional difference).

use crate::types::JsonObject;
use serde_json::Value;

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
