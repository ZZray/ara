//! Upgrade raw JSON tool schemas before the Responses-specific sanitizer.
//! Port of pinned OMP `utils/schema/draft.ts` for acyclic JSON values.

use serde_json::{Map, Value, json};

// Arrays are JSON containers in `anyOf`/tuple forms, so this is twice the
// Responses sanitizer's 128-level schema-node limit.
const MAX_UPGRADE_DEPTH: usize = 256;
const SCHEMA_MAP_KEYS: &[&str] = &["properties", "patternProperties", "dependentSchemas"];
const NON_SCHEMA_VALUE_KEYS: &[&str] =
    &["const", "default", "enum", "example", "examples", "required", "dependentRequired", "type"];

fn combine_schemas(left: Value, right: Value) -> Value {
    if left == Value::Bool(true) {
        return right;
    }
    if right == Value::Bool(true) {
        return left;
    }
    if left == Value::Bool(false) || right == Value::Bool(false) {
        return Value::Bool(false);
    }
    if left == right {
        return left;
    }
    json!({"allOf": [left, right]})
}

fn merge_schema_map(
    target: &mut Map<String, Value>,
    key: &str,
    source: &Map<String, Value>,
    depth: usize,
) -> Result<(), ()> {
    let entry = target.entry(key.to_owned()).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    let target_map = entry.as_object_mut().expect("replaced with object");
    for (name, schema) in source {
        target_map.insert(name.clone(), upgrade_json_schema(schema, depth + 1)?);
    }
    Ok(())
}

fn merge_dependent_required(target: &mut Map<String, Value>, key: &str, values: &[Value]) {
    let dependencies = target.entry("dependentRequired").or_insert_with(|| json!({}));
    if !dependencies.is_object() {
        *dependencies = json!({});
    }
    let dependencies = dependencies.as_object_mut().expect("replaced with object");
    if !dependencies.contains_key(key) {
        dependencies.insert(key.to_owned(), Value::Array(values.to_vec()));
    } else if let Some(existing) = dependencies.get_mut(key).and_then(Value::as_array_mut) {
        for value in values {
            if !existing.contains(value) {
                existing.push(value.clone());
            }
        }
    }
}

fn merge_dependent_schema(target: &mut Map<String, Value>, key: &str, schema: Value) {
    let dependencies = target.entry("dependentSchemas").or_insert_with(|| json!({}));
    if !dependencies.is_object() {
        *dependencies = json!({});
    }
    let dependencies = dependencies.as_object_mut().expect("replaced with object");
    let merged = match dependencies.remove(key) {
        Some(existing) => combine_schemas(existing, schema),
        None => schema,
    };
    dependencies.insert(key.to_owned(), merged);
}

fn is_null_variant(value: &Value) -> bool {
    let Some(kind) = value.get("type") else { return false };
    kind == "null" || kind.as_array().is_some_and(|kinds| kinds.iter().any(|kind| kind == "null"))
}

fn make_nullable(mut schema: Map<String, Value>) -> Value {
    match schema.get_mut("type") {
        Some(Value::String(kind)) if kind != "null" => {
            let kind = kind.clone();
            schema.insert("type".into(), json!([kind, "null"]));
            return Value::Object(schema);
        }
        Some(Value::String(_)) => return Value::Object(schema),
        Some(Value::Array(kinds)) => {
            if !kinds.iter().any(|kind| kind == "null") {
                kinds.push(json!("null"));
            }
            return Value::Object(schema);
        }
        _ => {}
    }
    if let Some(Value::Array(variants)) = schema.get_mut("anyOf") {
        if !variants.iter().any(is_null_variant) {
            variants.push(json!({"type": "null"}));
        }
        return Value::Object(schema);
    }
    json!({"anyOf": [Value::Object(schema), {"type": "null"}]})
}

fn draft_07_schema_uri(value: &str) -> bool {
    matches!(
        value,
        "http://json-schema.org/draft-07/schema#"
            | "https://json-schema.org/draft-07/schema#"
            | "http://json-schema.org/draft-07/schema"
            | "https://json-schema.org/draft-07/schema"
    )
}

/// The input is a `serde_json::Value`, so unlike OMP's JavaScript object graph
/// it is acyclic and needs no identity cache. A depth cap quarantines one bad
/// tool without affecting its neighbors.
pub(super) fn upgrade_json_schema(value: &Value, depth: usize) -> Result<Value, ()> {
    if depth > MAX_UPGRADE_DEPTH {
        return Err(());
    }
    let Some(source) = value.as_object() else {
        return match value {
            Value::Array(items) => Ok(Value::Array(
                items.iter().map(|item| upgrade_json_schema(item, depth + 1)).collect::<Result<_, _>>()?,
            )),
            _ => Ok(value.clone()),
        };
    };

    let mut output = Map::new();
    for (key, entry) in source {
        match key.as_str() {
            "definitions" | "$defs" | "dependencies" | "additionalItems" | "nullable" => {}
            key if SCHEMA_MAP_KEYS.contains(&key) => {
                if let Some(map) = entry.as_object() {
                    merge_schema_map(&mut output, key, map, depth)?;
                } else {
                    output.insert(key.to_owned(), entry.clone());
                }
            }
            key if NON_SCHEMA_VALUE_KEYS.contains(&key) => {
                output.insert(key.to_owned(), entry.clone());
            }
            "$schema" => {
                let schema = match entry.as_str() {
                    Some(uri) if draft_07_schema_uri(uri) => json!("https://json-schema.org/draft/2020-12/schema"),
                    _ => entry.clone(),
                };
                output.insert(key.clone(), schema);
            }
            "$ref" if entry.as_str().is_some_and(|reference| reference.starts_with("#/definitions/")) => {
                let reference = entry.as_str().expect("checked string");
                output.insert(key.clone(), json!(format!("#/$defs/{}", &reference["#/definitions/".len()..])));
            }
            "items" if entry.is_array() => {}
            _ => {
                output.insert(key.clone(), upgrade_json_schema(entry, depth + 1)?);
            }
        }
    }

    // The latest spelling wins if a JSON value supplies the same definition
    // under both names. `serde_json::Map` does not preserve OMP's JS insertion
    // order; choosing `$defs` here makes the result deterministic.
    for key in ["definitions", "$defs"] {
        if let Some(map) = source.get(key).and_then(Value::as_object) {
            merge_schema_map(&mut output, "$defs", map, depth)?;
        }
    }

    if let Some(items) = source.get("items").and_then(Value::as_array) {
        let upgraded = items.iter().map(|item| upgrade_json_schema(item, depth + 1)).collect::<Result<Vec<_>, _>>()?;
        let mut prefix = output.remove("prefixItems").and_then(|value| value.as_array().cloned()).unwrap_or_default();
        for (index, item) in upgraded.into_iter().enumerate() {
            if index < prefix.len() {
                prefix[index] = combine_schemas(prefix[index].clone(), item);
            } else {
                prefix.push(item);
            }
        }
        output.insert("prefixItems".into(), Value::Array(prefix));
        if let Some(additional) = source.get("additionalItems").filter(|additional| **additional != Value::Bool(true)) {
            output.insert("items".into(), upgrade_json_schema(additional, depth + 1)?);
        } else {
            output.remove("items");
        }
    }

    if let Some(dependencies) = source.get("dependencies").and_then(Value::as_object) {
        for (key, dependency) in dependencies {
            let converted = upgrade_json_schema(dependency, depth + 1)?;
            if let Some(required) = converted.as_array() {
                merge_dependent_required(&mut output, key, required);
            } else {
                merge_dependent_schema(&mut output, key, converted);
            }
        }
    }

    if source.get("nullable") == Some(&Value::Bool(true)) {
        Ok(make_nullable(output))
    } else {
        Ok(Value::Object(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_nested_legacy_keywords_without_uri_and_preserves_literal_data() {
        let raw = json!({
            "type": "object",
            "properties": {
                "definitions": {"type": "string"},
                "pair": {"type":"array","items":[{"type":"string"},{"type":"integer"}],"additionalItems":false},
                "gate": {"type":"object","dependencies":{"a":["b"],"c":{"required":["d"]}}},
                "item": {"$ref":"#/definitions/Item"},
                "other": {"$ref":"https://example.test/definitions/Item"},
                "literal": {"enum":[{"definitions":{"A":{}}}],"default":{"nullable":true},
                    "examples":[{"items":[{}]}]}
            },
            "definitions": {"Item":{"type":"string","nullable":true}}
        });
        let upgraded = upgrade_json_schema(&raw, 0).unwrap();
        assert_eq!(upgraded["properties"]["definitions"], json!({"type":"string"}));
        assert_eq!(
            upgraded["properties"]["pair"],
            json!({"type":"array","prefixItems":[
            {"type":"string"},{"type":"integer"}],"items":false})
        );
        assert_eq!(
            upgraded["properties"]["gate"],
            json!({"type":"object",
            "dependentRequired":{"a":["b"]},"dependentSchemas":{"c":{"required":["d"]}}})
        );
        assert_eq!(upgraded["properties"]["item"]["$ref"], "#/$defs/Item");
        assert_eq!(upgraded["properties"]["other"]["$ref"], "https://example.test/definitions/Item");
        assert_eq!(upgraded["properties"]["literal"], raw["properties"]["literal"]);
        assert_eq!(upgraded["$defs"]["Item"]["type"], json!(["string", "null"]));
        assert_eq!(raw["properties"]["pair"]["items"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn merges_existing_draft_2020_12_keywords_and_nullable_variants() {
        let raw = json!({
            "$schema":"https://json-schema.org/draft-07/schema#",
            "type":"object",
            "definitions":{"Same":{"type":"string"}},
            "$defs":{"Same":{"type":"integer"}},
            "properties": {
                "tuple":{"type":"array","prefixItems":[{"type":"string"},true],
                    "items":[{"type":"number"},{"type":"boolean"}],"additionalItems":true},
                "gated":{"dependentRequired":{"a":["b"]},"dependentSchemas":{"x":{"type":"string"},"y":false},
                    "dependencies":{"a":["b","c"],"x":{"minLength":2},"y":{"type":"object"}}},
                "nullableAny":{"anyOf":[{"type":"string"}],"nullable":true},
                "nullableWrapped":{"const":42,"nullable":true},
                "nullableFalse":{"type":"string","nullable":false}
            }
        });
        let upgraded = upgrade_json_schema(&raw, 0).unwrap();
        assert_eq!(upgraded["$schema"], "https://json-schema.org/draft/2020-12/schema");
        assert_eq!(upgraded["$defs"]["Same"]["type"], "integer");
        assert_eq!(
            upgraded["properties"]["tuple"]["prefixItems"][0],
            json!({"allOf":[{"type":"string"},{"type":"number"}]})
        );
        assert_eq!(upgraded["properties"]["tuple"]["prefixItems"][1], json!({"type":"boolean"}));
        assert!(upgraded["properties"]["tuple"].get("items").is_none());
        assert_eq!(upgraded["properties"]["gated"]["dependentRequired"]["a"], json!(["b", "c"]));
        assert_eq!(
            upgraded["properties"]["gated"]["dependentSchemas"]["x"],
            json!({"allOf":[{"type":"string"},{"minLength":2}]})
        );
        assert_eq!(upgraded["properties"]["gated"]["dependentSchemas"]["y"], false);
        assert_eq!(upgraded["properties"]["nullableAny"]["anyOf"][1], json!({"type":"null"}));
        assert_eq!(upgraded["properties"]["nullableWrapped"]["anyOf"][0], json!({"const":42}));
        assert!(upgraded["properties"]["nullableFalse"].get("nullable").is_none());
    }

    #[test]
    fn handles_legacy_keyword_extras_and_depth_limit() {
        let raw = json!({"items":{"type":"string"},"additionalItems":false,"nullable":false,
            "definitions":false,"dependencies":false});
        assert_eq!(upgrade_json_schema(&raw, 0).unwrap(), json!({"items":{"type":"string"}}));
        assert_eq!(
            upgrade_json_schema(&json!({"type":"array","items":[true]}), 0).unwrap(),
            json!({"type":"array","prefixItems":[true]})
        );
        assert_eq!(
            upgrade_json_schema(&json!({"type":"array","items":[],"additionalItems":{"type":"string"}}), 0).unwrap(),
            json!({"type":"array","prefixItems":[],"items":{"type":"string"}})
        );
        assert!(upgrade_json_schema(&json!({"type":"object"}), MAX_UPGRADE_DEPTH + 1).is_err());
    }

    #[test]
    fn nested_schema_arrays_fit_within_the_upgrade_depth_limit() {
        let mut schema = json!({"type":"string"});
        for _ in 0..65 {
            schema = json!({"anyOf":[schema]});
        }
        assert!(upgrade_json_schema(&schema, 0).is_ok());
    }
}
