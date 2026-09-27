//! Tool argument validation (OMP `packages/ai/src/utils/validation.ts`
//! `validateToolArguments`).
//!
//! Ported subset: parse-error reporting, optional-null stripping, scalar and
//! JSON-string coercion driven by validation issues, and the upstream error
//! format (`Validation failed for tool "<name>":` + `  - <path>: <message>`
//! lines + received arguments). The JSON Schema validator covers `type`,
//! `properties`, `required`, `additionalProperties: false`, `enum`, `const`,
//! `items`/`prefixItems`, local `$ref`, `allOf`, dependencies, boolean schemas,
//! `minimum`/`maximum`, `minLength`/`maxLength`, and `anyOf`/`oneOf`.
//! Not yet ported (open in the ledger): double-encoded key repair, flattened
//! array properties, enum/identifier whitespace normalization, single-string
//! remap, in-band arg-spill healing, ArkType-specific narrowing.

use crate::json::json_kind;
use crate::schema_draft::upgrade_json_schema;
use crate::types::{JsonObject, Tool};
use serde_json::Value;
use std::collections::HashSet;

const MAX_COERCION_PASSES: usize = 3;
const RAW_JSON_PREVIEW: usize = 512;
const ARG_STRING_PREVIEW: usize = 2000;
const MAX_VALIDATION_DEPTH: usize = 256;
const MAX_PRIMITIVE_REF_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
struct Issue {
    path: Vec<String>,
    message: String,
    expected: Option<Vec<String>>,
}

struct ValidationContext<'a> {
    root: &'a Value,
    ref_pairs: HashSet<(String, usize)>,
    primitive_ref_depth: usize,
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
    let schema = upgrade_json_schema(&tool.parameters, 0)
        .map_err(|()| format!("Validation failed for tool \"{call_name}\": schema nesting exceeds upgrade limit"))?;
    let original = Value::Object(args.clone());
    let mut value = original.clone();
    let mut changed = strip_optional_nulls(&schema, &mut value);

    let mut issues = validate(
        &schema,
        &value,
        &mut Vec::new(),
        &mut ValidationContext { root: &schema, ref_pairs: HashSet::new(), primitive_ref_depth: 0 },
        0,
    );
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
        strip_optional_nulls(&schema, &mut value);
        issues = validate(
            &schema,
            &value,
            &mut Vec::new(),
            &mut ValidationContext { root: &schema, ref_pairs: HashSet::new(), primitive_ref_depth: 0 },
            0,
        );
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

/// Remove `null` / `"null"` placeholders from optional properties whose schema
/// declares them optional (OMP `normalizeOptionalNullsForSchema`).
fn strip_optional_nulls(schema: &Value, value: &mut Value) -> bool {
    let mut changed = false;
    if let Some(Value::Array(branches)) = schema.get("allOf") {
        for branch in branches {
            changed |= strip_optional_nulls(branch, value);
        }
    }
    let (Some(Value::Object(props)), Value::Object(map)) = (schema.get("properties"), value) else {
        return changed;
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
            || matches!(map.get(&key), Some(Value::String(s)) if s == "null");
        if is_null_placeholder && !required.contains(&key.as_str()) {
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

fn validate(
    schema: &Value,
    value: &Value,
    path: &mut Vec<String>,
    context: &mut ValidationContext<'_>,
    depth: usize,
) -> Vec<Issue> {
    let mut issues = Vec::new();
    let issue = |path: &Vec<String>, message: String, expected: Option<Vec<String>>| Issue {
        path: path.clone(),
        message,
        expected,
    };
    if depth > MAX_VALIDATION_DEPTH {
        issues.push(issue(path, "schema recursion limit exceeded".into(), None));
        return issues;
    }
    if schema.as_bool() == Some(true) || schema.as_object().is_some_and(|o| o.is_empty()) {
        return issues;
    }
    if schema.as_bool() == Some(false) {
        issues.push(issue(path, "must not match false schema".into(), None));
        return issues;
    }
    let Some(object) = schema.as_object() else {
        issues.push(issue(path, "schema must be an object or boolean".into(), None));
        return issues;
    };
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        let root = context.root;
        let resolved = reference.strip_prefix('#').and_then(|pointer| {
            if pointer.is_empty() || pointer.starts_with('/') { root.pointer(pointer) } else { None }
        });
        let Some(resolved) = resolved else {
            issues.push(issue(path, format!("unresolved reference {reference}"), None));
            return issues;
        };
        if value.is_object() || value.is_array() {
            let pair = (reference.to_owned(), value as *const Value as usize);
            if !context.ref_pairs.insert(pair.clone()) {
                return issues;
            }
            issues.extend(validate(resolved, value, path, context, depth + 1));
            context.ref_pairs.remove(&pair);
        } else if context.primitive_ref_depth >= MAX_PRIMITIVE_REF_DEPTH {
            issues.push(issue(path, "reference depth exceeded".into(), None));
        } else {
            context.primitive_ref_depth += 1;
            issues.extend(validate(resolved, value, path, context, depth + 1));
            context.primitive_ref_depth -= 1;
        }
        return issues;
    }
    if let Some(Value::Array(branches)) = object.get("allOf") {
        for branch in branches {
            issues.extend(validate(branch, value, &mut path.clone(), context, depth + 1));
        }
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(branches)) = schema.get(key) {
            let mut matched = 0;
            let mut first_branch_issues = None;
            for branch in branches {
                let branch_issues = validate(branch, value, &mut path.clone(), context, depth + 1);
                if branch_issues.is_empty() {
                    matched += 1;
                } else if first_branch_issues.is_none() {
                    first_branch_issues = Some(branch_issues);
                }
            }
            let ok = if key == "anyOf" { matched >= 1 } else { matched == 1 };
            if !ok {
                if matched == 0
                    && let Some(branch_issues) = first_branch_issues
                {
                    issues.extend(branch_issues);
                    return issues;
                }
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
            if let Some(dependencies) = schema.get("dependentRequired").and_then(Value::as_object) {
                for (key, required) in dependencies {
                    if !map.contains_key(key) {
                        continue;
                    }
                    if let Some(required) = required.as_array() {
                        for dependent in required.iter().filter_map(Value::as_str) {
                            if !map.contains_key(dependent) {
                                path.push(dependent.to_owned());
                                issues.push(issue(path, format!("is required when \"{key}\" is present"), None));
                                path.pop();
                            }
                        }
                    }
                }
            }
            if let Some(dependencies) = schema.get("dependentSchemas").and_then(Value::as_object) {
                for (key, dependent_schema) in dependencies {
                    if map.contains_key(key) {
                        issues.extend(validate(dependent_schema, value, &mut path.clone(), context, depth + 1));
                    }
                }
            }
            for (key, child) in map {
                if let Some(child_schema) = props.and_then(|p| p.get(key)) {
                    path.push(key.clone());
                    issues.extend(validate(child_schema, child, path, context, depth + 1));
                    path.pop();
                } else if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                    path.push(key.clone());
                    issues.push(issue(path, "is not an allowed property".to_string(), None));
                    path.pop();
                } else if let Some(extra) = schema.get("additionalProperties").filter(|v| v.is_object()) {
                    path.push(key.clone());
                    issues.extend(validate(extra, child, path, context, depth + 1));
                    path.pop();
                }
            }
        }
        Value::Array(items) => {
            let prefix = schema.get("prefixItems").and_then(Value::as_array);
            if schema.get("items").is_some_and(Value::is_array) {
                issues.push(issue(path, "array-valued items is not valid in JSON Schema 2020-12".into(), None));
            }
            for (index, item) in items.iter().enumerate() {
                let item_schema = prefix
                    .and_then(|prefix| prefix.get(index))
                    .or_else(|| schema.get("items").filter(|schema| !schema.is_array()));
                if let Some(item_schema) = item_schema {
                    path.push(index.to_string());
                    issues.extend(validate(item_schema, item, path, context, depth + 1));
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

    #[test]
    fn legacy_schema_validates_refs_nullable_and_tuple_tail_before_execution() {
        // Pinned OMP `tool-argument-coercion.test.ts` draft-07 fixture.
        let t = tool(json!({
            "type":"object",
            "properties":{
                "item":{"$ref":"#/definitions/Item"},
                "name":{"type":"string","nullable":true},
                "pair":{"type":"array","items":[{"type":"string"},{"type":"integer"}],"additionalItems":false}
            },
            "required":["item","name","pair"],
            "definitions":{"Item":{"type":"string"}}
        }));
        let valid = obj(json!({"item":"ok","name":null,"pair":["a",1]}));
        assert_eq!(validate_tool_arguments(&t, "legacy", &valid).unwrap(), valid);
        let bad_type = validate_tool_arguments(
            &t,
            "legacy",
            &obj(json!({
                "item":"ok","name":null,"pair":["a","not-an-integer"]
            })),
        )
        .unwrap_err();
        assert!(bad_type.contains("pair/1: must be integer"), "{bad_type}");
        let extra = validate_tool_arguments(
            &t,
            "legacy",
            &obj(json!({
                "item":"ok","name":null,"pair":["a",1,"extra"]
            })),
        )
        .unwrap_err();
        assert!(extra.contains("pair/2: must not match false schema"), "{extra}");
    }

    #[test]
    fn legacy_dependencies_merge_and_local_pointer_escapes_are_enforced() {
        let t = tool(json!({
            "type":"object",
            "properties":{"gate":{"type":"boolean"},"a":{"type":"string"},
                "b":{"type":"string"},"value":{"$ref":"#/definitions/a~1b"}},
            "definitions":{"a/b":{"type":"integer"}},
            "dependentRequired":{"gate":["a"]},
            "dependentSchemas":{"gate":{"required":["value"]}},
            "dependencies":{"gate":["b"]}
        }));
        let valid = obj(json!({"gate":true,"a":"x","b":"y","value":2}));
        assert_eq!(validate_tool_arguments(&t, "legacy", &valid).unwrap(), valid);
        let missing = validate_tool_arguments(&t, "legacy", &obj(json!({"gate":true}))).unwrap_err();
        assert!(missing.contains("a: is required when \"gate\" is present"), "{missing}");
        assert!(missing.contains("b: is required when \"gate\" is present"), "{missing}");
        assert!(missing.contains("value: is required"), "{missing}");
        let wrong_ref = validate_tool_arguments(
            &t,
            "legacy",
            &obj(json!({
                "gate":true,"a":"x","b":"y","value":"not-an-integer"
            })),
        )
        .unwrap_err();
        assert!(wrong_ref.contains("value: must be integer"), "{wrong_ref}");

        let merged_schemas = tool(json!({
            "type":"object",
            "dependentSchemas":{"gate":{"required":["a"]}},
            "dependencies":{"gate":{"required":["b"]}}
        }));
        let err = validate_tool_arguments(&merged_schemas, "merged", &obj(json!({"gate":true}))).unwrap_err();
        assert!(err.contains("a: is required"), "{err}");
        assert!(err.contains("b: is required"), "{err}");
        assert!(validate_tool_arguments(&merged_schemas, "merged", &obj(json!({"gate":true,"a":1,"b":1}))).is_ok());
    }

    #[test]
    fn recursive_local_refs_check_each_nested_value_and_unresolved_refs_fail() {
        let recursive = tool(json!({
            "type":"object","properties":{"node":{"$ref":"#/definitions/Node"}},
            "definitions":{"Node":{"type":"object","properties":{
                "name":{"type":"string"},"child":{"$ref":"#/definitions/Node"}
            },"required":["name"]}}
        }));
        let bad = validate_tool_arguments(
            &recursive,
            "tree",
            &obj(json!({
                "node":{"name":"root","child":{"name":123}}
            })),
        )
        .unwrap_err();
        assert!(bad.contains("node/child/name: must be string"), "{bad}");
        let missing = validate_tool_arguments(
            &recursive,
            "tree",
            &obj(json!({
                "node":{"name":"root","child":{}}
            })),
        )
        .unwrap_err();
        assert!(missing.contains("node/child/name: is required"), "{missing}");

        let external = tool(json!({"type":"object","properties":{"x":{"$ref":"https://example.test/schema"}}}));
        let err = validate_tool_arguments(&external, "external", &obj(json!({"x":1}))).unwrap_err();
        assert!(err.contains("unresolved reference https://example.test/schema"), "{err}");
        let loop_schema = json!({"$ref":"#"});
        let mut context = ValidationContext { root: &loop_schema, ref_pairs: HashSet::new(), primitive_ref_depth: 0 };
        let issues = validate(&loop_schema, &json!("x"), &mut Vec::new(), &mut context, 0);
        assert!(issues.iter().any(|issue| issue.message == "reference depth exceeded"));
    }

    #[test]
    fn optional_null_is_removed_even_when_legacy_schema_declares_nullable() {
        let t = tool(json!({"type":"object","properties":{"optional":{"type":"string","nullable":true}}}));
        assert_eq!(validate_tool_arguments(&t, "legacy", &obj(json!({"optional":null}))).unwrap(), obj(json!({})));
        assert_eq!(validate_tool_arguments(&t, "legacy", &obj(json!({"optional":"null"}))).unwrap(), obj(json!({})));
    }

    #[test]
    fn referenced_union_preserves_leaf_type_for_numeric_coercion() {
        let t = tool(json!({
            "type":"object",
            "properties":{"n":{"anyOf":[{"$ref":"#/$defs/N"},{"type":"null"}]}},
            "$defs":{"N":{"type":"integer"}}
        }));
        assert_eq!(validate_tool_arguments(&t, "union", &obj(json!({"n":"42"}))).unwrap(), obj(json!({"n":42})));
        let err = validate_tool_arguments(&t, "union", &obj(json!({"n":"not-a-number"}))).unwrap_err();
        assert!(err.contains("n: must be integer"), "{err}");
    }

    #[test]
    fn all_of_strips_optional_null_placeholders_before_validation() {
        let t = tool(json!({
            "type":"object",
            "allOf":[{"type":"object","properties":{"optional":{"type":"string"}}}]
        }));
        assert_eq!(validate_tool_arguments(&t, "allOf", &obj(json!({"optional":null}))).unwrap(), obj(json!({})));
        assert_eq!(validate_tool_arguments(&t, "allOf", &obj(json!({"optional":"null"}))).unwrap(), obj(json!({})));
    }
}
