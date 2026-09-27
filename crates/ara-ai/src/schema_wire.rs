//! Shared raw JSON tool-wire postprocessing from fixed OMP `utils/schema/wire.ts`.

use serde_json::{Value, json};

pub(crate) const SCHEMA_MAP_KEYS: &[&str] =
    &["properties", "patternProperties", "dependencies", "dependentSchemas", "$defs", "definitions"];
pub(crate) const SCHEMA_ARRAY_KEYS: &[&str] = &["anyOf", "oneOf", "allOf", "prefixItems"];
pub(crate) const SCHEMA_VALUE_KEYS: &[&str] = &[
    "items",
    "additionalItems",
    "contains",
    "contentSchema",
    "propertyNames",
    "if",
    "then",
    "else",
    "not",
    "additionalProperties",
    "unevaluatedItems",
    "unevaluatedProperties",
];

fn homogeneous_enum_type(values: &[Value]) -> Option<&'static str> {
    let kind = match values.first()? {
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        _ => return None,
    };
    values
        .iter()
        .all(|value| match kind {
            "string" => value.is_string(),
            "number" => value.is_number(),
            _ => value.is_boolean(),
        })
        .then_some(kind)
}

fn has_schema_defining_sibling(object: &serde_json::Map<String, Value>) -> bool {
    object.keys().any(|key| {
        matches!(
            key.as_str(),
            "$ref"
                | "additionalProperties"
                | "allOf"
                | "const"
                | "contains"
                | "enum"
                | "if"
                | "items"
                | "not"
                | "oneOf"
                | "patternProperties"
                | "prefixItems"
                | "properties"
                | "propertyNames"
                | "then"
                | "else"
                | "unevaluatedItems"
                | "unevaluatedProperties"
        )
    })
}

// Fixed OMP `wire.ts::postProcessJsonSchema` applies these before the
// provider-specific sanitizer. ARA visits schema positions only, preserving
// literal instance data and the original argument names used by validation.
pub(crate) fn postprocess_json_wire_schema(value: &Value, depth: usize) -> Result<Value, ()> {
    let mut output = value.clone();
    postprocess_json_wire_schema_in_place(&mut output, depth)?;
    Ok(output)
}

fn postprocess_json_wire_schema_in_place(value: &mut Value, depth: usize) -> Result<(), ()> {
    if depth > 128 {
        return Err(());
    }
    let Some(output) = value.as_object_mut() else { return Ok(()) };

    if !output.contains_key("type")
        && !has_schema_defining_sibling(output)
        && let Some(variants) = output.get("anyOf").and_then(Value::as_array).cloned()
        && variants.len() == 2
    {
        let null_count = variants
            .iter()
            .filter(|variant| {
                variant
                    .as_object()
                    .is_some_and(|object| object.len() == 1 && object.get("type") == Some(&json!("null")))
            })
            .count();
        if null_count == 1
            && let Some(scalar) = variants.iter().find(|variant| variant["type"] != "null").and_then(Value::as_object)
            && let Some(kind) = scalar.get("type").and_then(Value::as_str)
            && matches!(kind, "string" | "number" | "integer" | "boolean")
        {
            output.remove("anyOf");
            for (key, child) in scalar {
                if !matches!(key.as_str(), "type" | "enum" | "const") {
                    output.entry(key.clone()).or_insert_with(|| child.clone());
                }
            }
            if let Some(constant) = scalar.get("const") {
                output.insert("enum".into(), json!([constant, null]));
            } else if let Some(values) = scalar.get("enum").and_then(Value::as_array) {
                let mut values = values.clone();
                if !values.contains(&Value::Null) {
                    values.push(Value::Null);
                }
                output.insert("enum".into(), Value::Array(values));
            }
            output.insert("type".into(), json!([kind, "null"]));
        }
    }

    if !output.contains_key("type")
        && let Some(values) = output.get("enum").and_then(Value::as_array)
        && let Some(kind) = homogeneous_enum_type(values)
    {
        output.insert("type".into(), json!(kind));
    }

    if !output.contains_key("type")
        && !has_schema_defining_sibling(output)
        && let Some(variants) = output.get("anyOf").and_then(Value::as_array).cloned()
        && variants.len() >= 2
    {
        let mut values = Vec::with_capacity(variants.len());
        let mut descriptions = Vec::with_capacity(variants.len());
        for variant in &variants {
            let Some(branch) = variant.as_object() else { break };
            if !branch.contains_key("const")
                || branch.keys().any(|key| !matches!(key.as_str(), "const" | "description"))
            {
                break;
            }
            let description = match branch.get("description") {
                Some(Value::String(text)) => Some(text.as_str()),
                Some(_) => break,
                None => None,
            };
            values.push(branch["const"].clone());
            descriptions.push(description);
        }
        if values.len() == variants.len()
            && descriptions.iter().all(|description| *description == descriptions[0])
            && let Some(kind) = homogeneous_enum_type(&values)
        {
            let shared = descriptions[0];
            let root = output.get("description").and_then(Value::as_str).map(str::to_owned);
            if shared.is_none() || root.is_none() || shared == root.as_deref() {
                output.remove("anyOf");
                output.insert("type".into(), json!(kind));
                output.insert("enum".into(), Value::Array(values));
                if let Some(shared) = shared
                    && root.is_none()
                {
                    output.insert("description".into(), json!(shared));
                }
            }
        }
    }

    for key in SCHEMA_MAP_KEYS {
        if let Some(map) = output.get_mut(*key).and_then(Value::as_object_mut) {
            for child in map.values_mut() {
                normalize_schema_child(child, depth + 1)?;
            }
        }
    }
    for key in SCHEMA_ARRAY_KEYS {
        if let Some(items) = output.get_mut(*key).and_then(Value::as_array_mut) {
            for child in items {
                normalize_schema_child(child, depth + 1)?;
            }
        }
    }
    for key in SCHEMA_VALUE_KEYS {
        if let Some(child) = output.get_mut(*key) {
            normalize_schema_child(child, depth + 1)?;
        }
    }
    Ok(())
}

// JSON Schema treats `{}` and `true` alike, but tool grammar samplers often
// interpret `{}` as an empty object. Only schema children are rewritten: the
// root, map containers, and literal instance data keep their original shape.
fn normalize_schema_child(value: &mut Value, depth: usize) -> Result<(), ()> {
    if depth > 128 {
        return Err(());
    }
    if value.as_object().is_some_and(serde_json::Map::is_empty) {
        *value = Value::Bool(true);
        Ok(())
    } else {
        postprocess_json_wire_schema_in_place(value, depth)
    }
}

#[cfg(test)]
mod tests {
    use super::postprocess_json_wire_schema;
    use serde_json::json;

    #[test]
    fn empty_schema_children_become_true_without_touching_literal_data_or_containers() {
        let source = json!({
            "type":"object",
            "properties":{
                "free":{},
                "nested":{"type":"array","items":{}},
                "literal":{"default":{},"examples":[{}],"const":{},"enum":[{}]}
            },
            "additionalProperties":{},
            "anyOf":[{}, {"type":"object","properties":{}}],
            "$defs":{}
        });
        let wire = postprocess_json_wire_schema(&source, 0).unwrap();
        assert_eq!(wire["properties"]["free"], true);
        assert_eq!(wire["properties"]["nested"]["items"], true);
        assert_eq!(wire["additionalProperties"], true);
        assert_eq!(wire["anyOf"][0], true);
        assert_eq!(wire["anyOf"][1]["properties"], json!({}));
        assert_eq!(wire["$defs"], json!({}));
        assert_eq!(wire["properties"]["literal"], source["properties"]["literal"]);
        assert_eq!(source["properties"]["free"], json!({}));
        assert_eq!(postprocess_json_wire_schema(&json!({}), 0).unwrap(), json!({}));
    }
}
