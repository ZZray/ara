//! Tool argument validation (OMP `packages/ai/src/utils/validation.ts`
//! `validateToolArguments`).
//!
//! Ported subset: parse-error reporting, optional-null stripping, scalar and
//! JSON-string coercion driven by validation issues, and the upstream error
//! format (`Validation failed for tool "<name>":` + `  - <path>: <message>`
//! lines + received arguments). The JSON Schema validator covers `type`,
//! `properties`, `required`, `additionalProperties: false`, `enum`, `const`,
//! `items`, `minimum`/`maximum`, `minLength`/`maxLength`, `anyOf`/`oneOf`.
//! Not yet ported (open in the ledger): double-encoded key repair, flattened
//! array properties, enum/identifier whitespace normalization, single-string
//! remap, in-band arg-spill healing, ArkType-specific narrowing.

use crate::json::json_kind;
use crate::types::{JsonObject, Tool};
use serde_json::Value;

const MAX_COERCION_PASSES: usize = 3;
const RAW_JSON_PREVIEW: usize = 512;
const ARG_STRING_PREVIEW: usize = 2000;

#[derive(Debug, Clone, PartialEq)]
struct Issue {
    path: Vec<String>,
    message: String,
    expected: Option<Vec<String>>,
}

/// Validate and normalize arguments; `Err` carries the model-facing message.
pub fn validate_tool_arguments(tool: &Tool, call_name: &str, args: &JsonObject) -> Result<JsonObject, String> {
    if let Some(parse_error) = args.get("__parseError") {
        let raw = args.get("__rawJson").and_then(Value::as_str).unwrap_or("");
        let preview = if raw.chars().count() <= RAW_JSON_PREVIEW {
            raw.to_string()
        } else {
            let head: String = raw.chars().take(RAW_JSON_PREVIEW).collect();
            format!("{head}… [truncated {} chars]", raw.chars().count() - RAW_JSON_PREVIEW)
        };
        let parse_error = parse_error.as_str().map(str::to_string).unwrap_or_else(|| parse_error.to_string());
        return Err(format!(
            "Validation failed for tool \"{call_name}\": Tool call arguments are not valid JSON.\nParse Error: {parse_error}\nRaw JSON:\n{preview}"
        ));
    }
    let schema = &tool.parameters;
    let original = Value::Object(args.clone());
    let mut value = original.clone();
    let mut changed = strip_optional_nulls(schema, &mut value);

    let mut issues = validate(schema, &value, &mut Vec::new());
    let mut pass = 0;
    while !issues.is_empty() && pass < MAX_COERCION_PASSES {
        pass += 1;
        let mut coerced = false;
        for issue in &issues {
            coerced |= coerce_at(&mut value, &issue.path, issue.expected.as_deref());
        }
        if !coerced {
            break;
        }
        changed = true;
        strip_optional_nulls(schema, &mut value);
        issues = validate(schema, &value, &mut Vec::new());
    }
    if issues.is_empty() {
        return match value {
            Value::Object(map) => Ok(map),
            other => Err(format!(
                "Validation failed for tool \"{call_name}\":\n  - root: expected object, got {}",
                json_kind(&other)
            )),
        };
    }
    let lines: Vec<String> = issues
        .iter()
        .map(|i| {
            format!("  - {}: {}", if i.path.is_empty() { "root".to_string() } else { i.path.join("/") }, i.message)
        })
        .collect();
    let received = if changed {
        serde_json::json!({"original": truncate_strings(&original), "normalized": truncate_strings(&value)})
    } else {
        truncate_strings(&original)
    };
    Err(format!(
        "Validation failed for tool \"{call_name}\":\n{}\n\nReceived arguments:\n{}",
        lines.join("\n"),
        serde_json::to_string_pretty(&received).unwrap_or_default()
    ))
}

fn truncate_strings(value: &Value) -> Value {
    match value {
        Value::String(s) if s.chars().count() > ARG_STRING_PREVIEW => {
            let head: String = s.chars().take(ARG_STRING_PREVIEW).collect();
            Value::String(format!("{head}… [truncated {} chars]", s.chars().count() - ARG_STRING_PREVIEW))
        }
        Value::Array(items) => Value::Array(items.iter().map(truncate_strings).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), truncate_strings(v))).collect()),
        other => other.clone(),
    }
}

fn schema_types(schema: &Value) -> Vec<String> {
    match schema.get("type") {
        Some(Value::String(t)) => vec![t.clone()],
        Some(Value::Array(ts)) => ts.iter().filter_map(|t| t.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

fn allows_null(schema: &Value) -> bool {
    if schema_types(schema).iter().any(|t| t == "null") {
        return true;
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(branches)) = schema.get(key)
            && branches.iter().any(allows_null)
        {
            return true;
        }
    }
    false
}

/// Remove `null` / `"null"` placeholders from optional properties whose schema
/// rejects null (OMP `normalizeOptionalNullsForSchema`).
fn strip_optional_nulls(schema: &Value, value: &mut Value) -> bool {
    let mut changed = false;
    let (Some(Value::Object(props)), Value::Object(map)) = (schema.get("properties"), value) else {
        return false;
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let keys: Vec<String> = map.keys().cloned().collect();
    for key in keys {
        let Some(prop_schema) = props.get(&key) else { continue };
        let is_null_placeholder = matches!(map.get(&key), Some(Value::Null))
            || matches!(map.get(&key), Some(Value::String(s)) if s == "null" && !schema_types(prop_schema).iter().any(|t| t == "string"));
        if is_null_placeholder && !required.contains(&key.as_str()) && !allows_null(prop_schema) {
            map.remove(&key);
            changed = true;
        } else if let Some(child) = map.get_mut(&key) {
            changed |= strip_optional_nulls(prop_schema, child);
        }
    }
    changed
}

fn type_matches(t: &str, value: &Value) -> bool {
    match t {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => {
            value.as_i64().is_some() || value.as_u64().is_some() || value.as_f64().is_some_and(|f| f.fract() == 0.0)
        }
        _ => true,
    }
}

fn validate(schema: &Value, value: &Value, path: &mut Vec<String>) -> Vec<Issue> {
    let mut issues = Vec::new();
    let issue = |path: &Vec<String>, message: String, expected: Option<Vec<String>>| Issue {
        path: path.clone(),
        message,
        expected,
    };
    if schema.as_bool() == Some(true) || schema.as_object().is_some_and(|o| o.is_empty()) {
        return issues;
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(branches)) = schema.get(key) {
            let matched = branches.iter().filter(|b| validate(b, value, &mut path.clone()).is_empty()).count();
            let ok = if key == "anyOf" { matched >= 1 } else { matched == 1 };
            if !ok {
                let expected: Vec<String> = branches.iter().flat_map(schema_types).collect();
                issues.push(issue(
                    path,
                    format!(
                        "must match {} schema in {key}",
                        if key == "anyOf" { "at least one" } else { "exactly one" }
                    ),
                    Some(expected),
                ));
                return issues;
            }
        }
    }
    let types = schema_types(schema);
    if !types.is_empty() && !types.iter().any(|t| type_matches(t, value)) {
        issues.push(issue(path, format!("must be {} (was {})", types.join(" | "), json_kind(value)), Some(types)));
        return issues;
    }
    if let Some(expected) = schema.get("const")
        && expected != value
    {
        issues.push(issue(path, format!("must be {expected}"), None));
    }
    if let Some(Value::Array(options)) = schema.get("enum")
        && !options.contains(value)
    {
        let rendered: Vec<String> = options.iter().map(Value::to_string).collect();
        issues.push(issue(path, format!("must be one of {}", rendered.join(", ")), None));
    }
    match value {
        Value::Object(map) => {
            let props = schema.get("properties").and_then(Value::as_object);
            if let Some(Value::Array(required)) = schema.get("required") {
                for key in required.iter().filter_map(Value::as_str) {
                    if !map.contains_key(key) {
                        path.push(key.to_string());
                        issues.push(issue(path, "is required".to_string(), None));
                        path.pop();
                    }
                }
            }
            for (key, child) in map {
                if let Some(child_schema) = props.and_then(|p| p.get(key)) {
                    path.push(key.clone());
                    issues.extend(validate(child_schema, child, path));
                    path.pop();
                } else if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                    path.push(key.clone());
                    issues.push(issue(path, "is not an allowed property".to_string(), None));
                    path.pop();
                } else if let Some(extra) = schema.get("additionalProperties").filter(|v| v.is_object()) {
                    path.push(key.clone());
                    issues.extend(validate(extra, child, path));
                    path.pop();
                }
            }
        }
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                for (i, item) in items.iter().enumerate() {
                    path.push(i.to_string());
                    issues.extend(validate(item_schema, item, path));
                    path.pop();
                }
            }
        }
        Value::String(s) => {
            let len = s.chars().count() as u64;
            if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
                && len < min
            {
                issues.push(issue(path, format!("must have at least {min} characters"), None));
            }
            if let Some(max) = schema.get("maxLength").and_then(Value::as_u64)
                && len > max
            {
                issues.push(issue(path, format!("must have at most {max} characters"), None));
            }
        }
        Value::Number(n) => {
            let v = n.as_f64().unwrap_or(0.0);
            if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
                && v < min
            {
                issues.push(issue(path, format!("must be >= {min}"), None));
            }
            if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
                && v > max
            {
                issues.push(issue(path, format!("must be <= {max}"), None));
            }
        }
        _ => {}
    }
    issues
}

fn value_at_mut<'a>(value: &'a mut Value, path: &[String]) -> Option<&'a mut Value> {
    let mut cur = value;
    for seg in path {
        cur = match cur {
            Value::Object(map) => map.get_mut(seg)?,
            Value::Array(items) => items.get_mut(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Coerce a scalar or JSON-string value toward the expected type
/// (OMP `coerceArgsFromIssues`, subset).
fn coerce_at(root: &mut Value, path: &[String], expected: Option<&[String]>) -> bool {
    let Some(expected) = expected else { return false };
    let Some(slot) = value_at_mut(root, path) else { return false };
    let Value::String(s) = slot else { return false };
    let trimmed = s.trim();
    for t in expected {
        let replacement = match t.as_str() {
            "number" => trimmed.parse::<f64>().ok().filter(|f| f.is_finite()).and_then(|f| {
                if f.fract() == 0.0 && f.abs() < 9.0e15 {
                    Some(Value::from(f as i64))
                } else {
                    serde_json::Number::from_f64(f).map(Value::Number)
                }
            }),
            "integer" => trimmed.parse::<i64>().ok().map(Value::from),
            "boolean" => match trimmed {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                _ => None,
            },
            "array" => serde_json::from_str::<Value>(trimmed).ok().filter(Value::is_array),
            "object" => serde_json::from_str::<Value>(trimmed).ok().filter(Value::is_object),
            _ => None,
        };
        if let Some(v) = replacement {
            *slot = v;
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(params: Value) -> Tool {
        Tool { name: "t".into(), description: String::new(), parameters: params }
    }

    fn obj(v: Value) -> JsonObject {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn accepts_valid_and_strips_optional_nulls() {
        let t = tool(
            json!({"type": "object", "properties": {"path": {"type": "string"}, "limit": {"type": "number"}}, "required": ["path"]}),
        );
        let out = validate_tool_arguments(&t, "read", &obj(json!({"path": "a", "limit": null}))).unwrap();
        assert_eq!(Value::Object(out), json!({"path": "a"}));
    }

    #[test]
    fn coerces_numeric_and_boolean_strings() {
        let t = tool(json!({"type": "object", "properties": {"n": {"type": "integer"}, "b": {"type": "boolean"}}}));
        let out = validate_tool_arguments(&t, "x", &obj(json!({"n": "42", "b": "true"}))).unwrap();
        assert_eq!(Value::Object(out), json!({"n": 42, "b": true}));
    }

    #[test]
    fn reports_missing_required_in_upstream_format() {
        let t = tool(
            json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"], "additionalProperties": false}),
        );
        let err = validate_tool_arguments(&t, "read", &obj(json!({"file": "a"}))).unwrap_err();
        assert!(err.starts_with("Validation failed for tool \"read\":\n"), "{err}");
        assert!(err.contains("  - path: is required"), "{err}");
        assert!(err.contains("  - file: is not an allowed property"), "{err}");
        assert!(err.contains("Received arguments:\n{\n  \"file\": \"a\"\n}"), "{err}");
    }

    #[test]
    fn reports_parse_errors() {
        let t = tool(json!({"type": "object"}));
        let args = crate::json::parse_final_arguments("{\"a\":");
        let err = validate_tool_arguments(&t, "write", &args).unwrap_err();
        assert!(
            err.starts_with(
                "Validation failed for tool \"write\": Tool call arguments are not valid JSON.\nParse Error: "
            ),
            "{err}"
        );
        assert!(err.ends_with("Raw JSON:\n{\"a\":"), "{err}");
    }

    #[test]
    fn type_mismatch_without_coercion_fails() {
        let t = tool(json!({"type": "object", "properties": {"n": {"type": "integer"}}}));
        let err = validate_tool_arguments(&t, "x", &obj(json!({"n": "abc"}))).unwrap_err();
        assert!(err.contains("  - n: must be integer (was string)"), "{err}");
    }
}
