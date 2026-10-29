//! Tool argument validation (OMP `packages/ai/src/utils/validation.ts`
//! `validateToolArguments`).
//!
//! Ported subset: parse-error reporting, optional-null stripping, scalar and
//! JSON-string coercion driven by validation issues, and the upstream error
//! format (`Validation failed for tool "<name>":` + `  - <path>: <message>`
//! lines + received arguments). Pure JSON Schema checking is delegated to
//! the complete fixed Native checker in `schema_validator`; this facade's
//! serde inputs convert only their representable domain. The normalizers and
//! argument-origin contract below remain the existing subset.
//! The separate Graph APIs below retain actual values through key/flattened
//! repair, schema and identifier normalizations, coercion rounds and spill.
//! The JSON-context Graph driver does not replace this existing serde facade.
//! Ark runtime and production Graph Session/dispatch remain open.

mod context;
mod graph;
pub use crate::schema_draft::upgrade_graph_json_schema_to_202012;
pub use context::{
    GraphValidationContext, get_graph_validation_context, graph_context_normalization_schema, validate_graph_context,
};
pub use graph::{
    GraphContextValidation, GraphNormalization, GraphToolValidationError, coerce_graph_args_from_issues,
    delete_graph_value_at_pointer, format_graph_issue_path, format_graph_issue_path_with_runtime,
    get_graph_value_at_pointer, heal_graph_inband_arg_spill, normalize_graph_double_encoded_keys,
    normalize_graph_enum_string_whitespace, normalize_graph_flattened_array_properties,
    normalize_graph_identifier_string_whitespace, normalize_graph_optional_nulls_for_schema,
    normalize_graph_single_string_field, normalize_graph_string_encoded_array_unions, path_to_graph_pointer,
    path_to_graph_pointer_with_runtime, set_graph_value_at_pointer, truncate_graph_args_for_error,
    try_parse_graph_json_for_types, validate_graph_json_context, validate_graph_tool_arguments,
    validate_graph_tool_call,
};

use crate::json::json_kind;
use crate::schema_draft::upgrade_json_schema;
use crate::schema_validator::{JsonSchemaPathSegment, validate_serde_json_schema_value};
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

/// Native reference identity, separate from Rust DTO allocation or value equality.
/// `Unproven` retains the declared gap for currently unported Native normalizers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolArgumentsOrigin {
    InputReference,
    NormalizedCopy,
    Unproven,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedToolArguments {
    pub args: JsonObject,
    pub origin: ToolArgumentsOrigin,
}

/// Validate and normalize arguments; `Err` carries the model-facing message.
pub fn validate_tool_arguments(tool: &Tool, call_name: &str, args: &JsonObject) -> Result<JsonObject, String> {
    validate_tool_arguments_with_origin(tool, call_name, args).map(|validated| validated.args)
}

/// Return source-backed Native identity for the admitted flat string-schema
/// shape. Other currently unported normalization shapes remain `Unproven`.
pub fn validate_tool_arguments_with_origin(
    tool: &Tool,
    call_name: &str,
    args: &JsonObject,
) -> Result<ValidatedToolArguments, String> {
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
    let native_identity_proven = native_flat_string_identity_proven(&schema, args);
    let original = Value::Object(args.clone());
    let mut value = original.clone();
    let mut changed = strip_optional_nulls(&schema, &mut value);

    let (mut success, mut issues) = validate(&schema, &value);
    let mut pass = 0;
    while !success && pass < MAX_COERCION_PASSES {
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
        (success, issues) = validate(&schema, &value);
    }
    if success {
        return match value {
            Value::Object(map) => Ok(ValidatedToolArguments {
                args: map,
                origin: if !native_identity_proven {
                    ToolArgumentsOrigin::Unproven
                } else if changed {
                    ToolArgumentsOrigin::NormalizedCopy
                } else {
                    ToolArgumentsOrigin::InputReference
                },
            }),
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

// Fixed Native validation.ts:1930-2021 runs copy-on-change normalizers before
// plain JSON-schema validation returns its input reference (1724-1725). The
// existing Rust subset cannot infer all their identities from `changed=false`.
// This admitted shape proves every unported normalizer inert; optional-null
// removal is already ported and supplies its actual branch-origin flag.
fn native_flat_string_identity_proven(schema: &Value, args: &JsonObject) -> bool {
    let Some(root) = schema.as_object() else {
        return false;
    };
    if root.get("type").and_then(Value::as_str) != Some("object")
        || root.keys().any(|key| {
            !matches!(
                key.as_str(),
                "type" | "properties" | "required" | "additionalProperties" | "description" | "title"
            )
        })
    {
        return false;
    }
    if root.get("additionalProperties").is_some_and(|value| !value.is_boolean()) {
        return false;
    }
    let Some(properties) = root.get("properties").and_then(Value::as_object) else {
        return false;
    };
    if properties.iter().any(|(key, property)| {
        !plain_identity_key(key)
            || property.as_object().is_none_or(|property| {
                property.get("type").and_then(Value::as_str) != Some("string")
                    || property.keys().any(|key| !matches!(key.as_str(), "type" | "description" | "title"))
            })
    }) {
        return false;
    }
    if root.get("required").is_some_and(|required| {
        required.as_array().is_none_or(|required| {
            required.iter().any(|key| key.as_str().is_none_or(|key| !properties.contains_key(key)))
        })
    }) {
        return false;
    }
    args.iter().all(|(key, value)| {
        if !plain_identity_key(key) {
            return false;
        }
        match value {
            Value::String(text) => {
                // Native's content keys are exempt; identifier keys trim only
                // trailing CR/LF, not spaces. Such missing-normalizer inputs
                // remain Unproven rather than silently assuming an alias.
                !matches!(
                    key.as_str(),
                    "path"
                        | "paths"
                        | "file"
                        | "file_path"
                        | "filePath"
                        | "filepath"
                        | "url"
                        | "uri"
                        | "title"
                        | "label"
                ) || !(text.ends_with('\r') || text.ends_with('\n'))
            }
            Value::Null => properties.contains_key(key),
            _ => false,
        }
    })
}

fn plain_identity_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
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
        let Some(prop_schema) = props.get(&key) else {
            continue;
        };
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

// This adapter retains the legacy facade's UTF-8 error/coercion interface.
// Every source string came from serde; the direct Wire API owns the wider
// UTF-16 domain and rich keyword/union provenance.
fn validate(schema: &Value, value: &Value) -> (bool, Vec<Issue>) {
    let result = validate_serde_json_schema_value(schema, value);
    let issues = result
        .issues
        .into_iter()
        .map(|issue| Issue {
            path: issue
                .path
                .into_iter()
                .map(|part| match part {
                    JsonSchemaPathSegment::Property(key) => key.to_utf8().expect("serde facade key is valid UTF-16"),
                    JsonSchemaPathSegment::Index(index) => index.to_string(),
                })
                .collect(),
            message: issue.message.to_utf8().expect("serde facade issue message is valid UTF-16"),
            expected: issue.expected_types.map(|types| {
                types.into_iter().map(|kind| kind.to_utf8().expect("serde facade type is valid UTF-16")).collect()
            }),
        })
        .collect();
    (result.success, issues)
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
    let Some(expected) = expected else {
        return false;
    };
    let Some(slot) = value_at_mut(root, path) else {
        return false;
    };
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
        Tool { name: "t".into(), description: String::new(), parameters: params, ..Default::default() }
    }

    fn obj(v: Value) -> JsonObject {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn accepts_valid_and_strips_optional_nulls() {
        graph::tests::aliases_defaults_entries_and_context();
        context::tests::cache_draft_and_live_fields();
        context::tests::authored_ark_context_family();
        crate::schema_wire::observe_graph_wire_family();
        graph::observe_remaining_normalizers_family();
        let t = tool(
            json!({"type": "object", "properties": {"path": {"type": "string"}, "limit": {"type": "number"}}, "required": ["path"]}),
        );
        let out = validate_tool_arguments(&t, "read", &obj(json!({"path": "a", "limit": null}))).unwrap();
        assert_eq!(Value::Object(out), json!({"path": "a"}));
        let same = validate_tool_arguments_with_origin(
            &tool(json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]})),
            "echo",
            &obj(json!({"text":"preserve content\n"})),
        )
        .unwrap();
        assert_eq!(same.origin, ToolArgumentsOrigin::InputReference);
        let copy = validate_tool_arguments_with_origin(
            &tool(json!({"type":"object","properties":{"text":{"type":"string"},"optional":{"type":"string"}},"required":["text"]})),
            "echo",
            &obj(json!({"text":"original","optional":null})),
        )
        .unwrap();
        assert_eq!(copy.origin, ToolArgumentsOrigin::NormalizedCopy);
        let unproven = validate_tool_arguments_with_origin(
            &tool(json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})),
            "read",
            &obj(json!({"path":"unported-identifier\n"})),
        )
        .unwrap();
        assert_eq!(unproven.origin, ToolArgumentsOrigin::Unproven);
    }

    #[test]
    fn coerces_numeric_and_boolean_strings() {
        graph::tests::numeric_and_array_domain();
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
        assert!(err.contains("  - file: must not be present"), "{err}");
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
        assert!(err.contains("  - n: expected integer, received string"), "{err}");
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
        assert!(bad_type.contains("pair/1: expected integer"), "{bad_type}");
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
        assert!(wrong_ref.contains("value: expected integer"), "{wrong_ref}");

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
        assert!(bad.contains("node/child/name: expected string"), "{bad}");
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
        let (success, issues) = validate(&loop_schema, &json!("x"));
        assert!(!success);
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
        assert!(err.contains("n: expected integer"), "{err}");
    }

    #[test]
    fn all_of_strips_optional_null_placeholders_before_validation() {
        graph::tests::unions_iterators_and_failures();
        context::tests::stamps_methods_and_failures();
        let t = tool(json!({
            "type":"object",
            "allOf":[{"type":"object","properties":{"optional":{"type":"string"}}}]
        }));
        assert_eq!(validate_tool_arguments(&t, "allOf", &obj(json!({"optional":null}))).unwrap(), obj(json!({})));
        assert_eq!(validate_tool_arguments(&t, "allOf", &obj(json!({"optional":"null"}))).unwrap(), obj(json!({})));
    }

    #[test]
    fn full_pure_checker_keywords_are_reached_through_legacy_facade() {
        let cases = [
            (json!({"propertyNames":{"pattern":"^[a-z]+$"}}), json!({"BAD":1}), "BAD: must match pattern"),
            (json!({"patternProperties":{"^x":{"type":"boolean"}}}), json!({"x":1}), "x: expected boolean"),
            (json!({"if":true,"then":false}), json!({}), "root: must not match false schema"),
            (
                json!({"properties":{"x":{"contains":{"type":"number"}}}}),
                json!({"x":["a"]}),
                "x: must contain at least 1",
            ),
            (json!({"properties":{"x":{"maxLength":1}}}), json!({"x":"😀"}), "x: must be at most 1 characters"),
        ];
        for (schema, args, expected) in cases {
            let error = validate_tool_arguments(&tool(schema), "facade", &obj(args)).unwrap_err();
            assert!(error.contains(expected), "{error}");
        }
    }
}
