//! Fixed b91db66e output-schema-validator/JTD conversion, MIT; THIRD_PARTY_NOTICES.md.
//! Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük; (c) 2026 Stencil Labs, Inc.
//! Declarations are durable JSON. Submitted data remains in the actual Graph.
use ara_ai::graph_values::GraphExecution;
use ara_rpc::{
    WireString,
    value_heap::{ValueError, ValueHandle, ValueHeap, ValueKind, ValueRuntime, ValueSnapshot},
};
use serde_json::{Map, Value, json};
use std::{collections::HashSet, sync::Arc};
pub(crate) type GResult<T> = Result<T, ValueError>;
#[derive(Debug)]
pub struct OutputSchema {
    pub normalized: Option<Value>,
    pub json_schema: Option<Value>,
    pub error: Option<String>,
    pub strict: bool,
    pub parameters: Value,
}
impl Clone for OutputSchema {
    fn clone(&self) -> Self {
        Self {
            normalized: self.normalized.as_ref().map(clone_json),
            json_schema: self.json_schema.as_ref().map(clone_json),
            error: self.error.clone(),
            strict: self.strict,
            parameters: clone_json(&self.parameters),
        }
    }
}
impl Drop for OutputSchema {
    fn drop(&mut self) {
        if let Some(v) = self.normalized.take() {
            drop_declaration(v)
        }
        if let Some(v) = self.json_schema.take() {
            drop_declaration(v)
        }
        drop_declaration(std::mem::replace(&mut self.parameters, Value::Null));
    }
}
fn primitive(s: &str) -> Option<&'static str> {
    Some(match s {
        "boolean" => "boolean",
        "string" | "timestamp" => "string",
        "float32" | "float64" => "number",
        "int8" | "uint8" | "int16" | "uint16" | "int32" | "uint32" => "integer",
        _ => return None,
    })
}
fn jtd_doc(v: &Value) -> bool {
    const KEYS: &[&str] = &[
        "definitions",
        "metadata",
        "nullable",
        "ref",
        "type",
        "enum",
        "elements",
        "properties",
        "optionalProperties",
        "additionalProperties",
        "values",
        "discriminator",
        "mapping",
    ];
    let mut pending = vec![v];
    while let Some(v) = pending.pop() {
        let Some(o) = v.as_object() else { return false };
        if o.keys().any(|k| !KEYS.contains(&k.as_str())) {
            return false;
        }
        if let Some(t) = o.get("type") {
            if !t.as_str().is_some_and(|s| primitive(s).is_some()) {
                return false;
            }
        }
        for k in ["nullable", "additionalProperties"] {
            if o.get(k).is_some_and(|x| !x.is_boolean()) {
                return false;
            }
        }
        for k in ["ref", "discriminator"] {
            if o.get(k).is_some_and(|x| !x.is_string()) {
                return false;
            }
        }
        if o.get("enum")
            .is_some_and(|x| !x.as_array().is_some_and(|a| a.iter().all(Value::is_string)))
        {
            return false;
        }
        for k in ["elements", "values"] {
            if let Some(x) = o.get(k) {
                pending.push(x)
            }
        }
        for k in ["properties", "optionalProperties", "definitions", "mapping"] {
            if let Some(x) = o.get(k) {
                let Some(m) = x.as_object() else { return false };
                pending.extend(m.values());
            }
        }
    }
    true
}
fn is_jtd(v: &Value) -> bool {
    v.as_object().is_some_and(|o| {
        [
            "elements",
            "values",
            "optionalProperties",
            "discriminator",
            "ref",
        ]
        .iter()
        .any(|k| o.contains_key(*k))
            || o.get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| primitive(t).is_some() && !matches!(t, "string" | "boolean"))
            || (o.contains_key("properties") && !o.contains_key("type") && jtd_doc(v))
    })
}
fn unambiguous_jtd(v: &Value) -> bool {
    let Some(o) = v.as_object() else { return false };
    ["elements", "values", "optionalProperties"]
        .iter()
        .any(|k| o.get(*k).is_some_and(Value::is_object))
        || o.get("ref").is_some_and(Value::is_string)
        || o.get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| primitive(t).is_some() && !matches!(t, "string" | "boolean"))
        || (o.get("discriminator").is_some_and(Value::is_string)
            && o.get("mapping")
                .and_then(Value::as_object)
                .is_some_and(|m| {
                    m.values().all(|x| {
                        x.as_object().is_some_and(|a| {
                            ["properties", "optionalProperties"]
                                .iter()
                                .any(|k| a.get(*k).is_some_and(Value::is_object))
                                && ["properties", "optionalProperties"]
                                    .iter()
                                    .all(|k| a.get(*k).is_none_or(Value::is_object))
                        })
                    })
                }))
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum WalkMode {
    Data,
    Normalize,
    Jtd,
    SchemaMap,
    JtdMap,
    SchemaArray,
    Strict,
    StrictMap,
    StrictArray,
}
enum Walk<'a> {
    Enter(&'a Value, WalkMode),
    Array(&'a Value, WalkMode, usize),
    Object(&'a Value, WalkMode, Vec<String>),
}
fn child_mode(mode: WalkMode, key: &str) -> WalkMode {
    match mode {
        WalkMode::SchemaMap => WalkMode::Normalize,
        WalkMode::JtdMap => WalkMode::Jtd,
        WalkMode::StrictMap => WalkMode::Strict,
        WalkMode::Strict => match key {
            "properties" | "$defs" | "definitions" => WalkMode::StrictMap,
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => WalkMode::StrictArray,
            "items" => WalkMode::Strict,
            _ => WalkMode::Data,
        },
        WalkMode::Normalize => match key {
            "not"
            | "if"
            | "then"
            | "else"
            | "items"
            | "contains"
            | "propertyNames"
            | "additionalProperties"
            | "unevaluatedProperties"
            | "unevaluatedItems"
            | "contentSchema" => {
                if mode == WalkMode::Strict {
                    WalkMode::Strict
                } else {
                    WalkMode::Normalize
                }
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                if mode == WalkMode::Strict {
                    WalkMode::StrictArray
                } else {
                    WalkMode::SchemaArray
                }
            }
            "properties" | "patternProperties" | "$defs" | "definitions" | "dependentSchemas" => {
                if mode == WalkMode::Strict {
                    WalkMode::StrictMap
                } else {
                    WalkMode::SchemaMap
                }
            }
            _ => WalkMode::Data,
        },
        WalkMode::Jtd => match key {
            "elements" | "values" => WalkMode::Jtd,
            "properties" | "optionalProperties" | "definitions" | "mapping" => WalkMode::JtdMap,
            _ => WalkMode::Data,
        },
        _ => WalkMode::Data,
    }
}
fn array_mode(mode: WalkMode) -> WalkMode {
    match mode {
        WalkMode::SchemaArray => WalkMode::Normalize,
        WalkMode::StrictArray => WalkMode::Strict,
        _ => WalkMode::Data,
    }
}
// Only schema positions are transformed; property names and data/extension
// payloads are copied verbatim. The explicit postorder stack avoids native
// recursion for cloning and JTD/provider-schema preparation.
fn walk_schema(root: &Value, mode: WalkMode) -> Option<Value> {
    let mut pending = vec![Walk::Enter(root, mode)];
    let mut output: Vec<Option<Value>> = Vec::new();
    while let Some(next) = pending.pop() {
        match next {
            Walk::Enter(value, mut mode) => {
                if mode == WalkMode::Normalize && unambiguous_jtd(value) {
                    mode = WalkMode::Jtd
                }
                match value {
                    Value::Array(a) => {
                        pending.push(Walk::Array(value, mode, a.len()));
                        for child in a.iter().rev() {
                            pending.push(Walk::Enter(child, array_mode(mode)));
                        }
                    }
                    Value::Object(o) => {
                        let keys = o.keys().cloned().collect::<Vec<_>>();
                        pending.push(Walk::Object(value, mode, keys));
                        for (key, child) in o.iter().rev() {
                            pending.push(Walk::Enter(child, child_mode(mode, key)));
                        }
                    }
                    _ => output.push(if mode == WalkMode::Strict {
                        None
                    } else {
                        Some(value.clone())
                    }),
                }
            }
            Walk::Array(original, mode, len) => {
                let children = output.split_off(output.len() - len);
                let rebuilt = collect_children(children).map(Value::Array);
                output.push(rebuilt.and_then(|v| finish_node(original, v, mode)));
            }
            Walk::Object(original, mode, keys) => {
                let children = output.split_off(output.len() - keys.len());
                let rebuilt = collect_children(children)
                    .map(|a| Value::Object(keys.into_iter().zip(a).collect()));
                output.push(rebuilt.and_then(|v| finish_node(original, v, mode)));
            }
        }
    }
    output.pop().flatten()
}
fn collect_children(children: Vec<Option<Value>>) -> Option<Vec<Value>> {
    if children.iter().any(Option::is_none) {
        for value in children.into_iter().flatten() {
            drop_declaration(value)
        }
        None
    } else {
        Some(children.into_iter().flatten().collect())
    }
}
fn finish_node(original: &Value, value: Value, mode: WalkMode) -> Option<Value> {
    match mode {
        WalkMode::Jtd => Some(convert_jtd_node(value)),
        WalkMode::Strict => strict_node(original, value),
        _ => Some(value),
    }
}
fn clone_json(value: &Value) -> Value {
    walk_schema(value, WalkMode::Data).expect("data copying cannot reject")
}
fn convert_jtd_node(value: Value) -> Value {
    let Value::Object(mut o) = value else {
        return json!({});
    };
    let mut out = if let Some(a) = o.remove("enum") {
        json!({"enum":a})
    } else if let Some(x) = o.remove("elements") {
        let mut n = Map::new();
        n.insert("type".into(), json!("array"));
        n.insert("items".into(), x);
        Value::Object(n)
    } else if let Some(t) = o.get("type").and_then(Value::as_str) {
        json!({"type":primitive(t).unwrap_or(t)})
    } else if let Some(x) = o.remove("values") {
        let mut n = Map::new();
        n.insert("type".into(), json!("object"));
        n.insert("additionalProperties".into(), x);
        Value::Object(n)
    } else if o.contains_key("properties") || o.contains_key("optionalProperties") {
        let mut props = Map::new();
        let mut required = Vec::new();
        for k in ["properties", "optionalProperties"] {
            if let Some(Value::Object(m)) = o.remove(k) {
                for (name, value) in m {
                    if k == "properties" {
                        required.push(json!(name));
                    }
                    props.insert(name, value);
                }
            }
        }
        let mut n = Map::new();
        n.insert("type".into(), json!("object"));
        n.insert("properties".into(), Value::Object(props));
        n.insert("additionalProperties".into(), json!(false));
        if !required.is_empty() {
            n.insert("required".into(), Value::Array(required));
        }
        Value::Object(n)
    } else if let (Some(tag), Some(Value::Object(mapping))) = (
        o.get("discriminator")
            .and_then(Value::as_str)
            .map(str::to_owned),
        o.remove("mapping"),
    ) {
        let mut variants = Vec::new();
        for (name, mut node) in mapping {
            if !node["properties"].is_object() {
                node["properties"] = json!({})
            }
            node["properties"][&tag] = json!({"const":name});
            let mut a = node["required"].as_array().cloned().unwrap_or_default();
            if !a.contains(&json!(tag)) {
                a.push(json!(tag))
            }
            node["required"] = json!(a);
            variants.push(node);
        }
        Value::Object(Map::from_iter([("oneOf".into(), Value::Array(variants))]))
    } else if let Some(s) = o.get("ref").and_then(Value::as_str) {
        json!({"$ref":format!("#/$defs/{s}")})
    } else {
        json!({})
    };
    if o.get("nullable") == Some(&Value::Bool(true)) {
        out = Value::Object(Map::from_iter([(
            "anyOf".into(),
            Value::Array(vec![out, json!({"type":"null"})]),
        )]));
    }
    drop_declaration(Value::Object(o));
    out
}
fn convert_jtd(v: &Value) -> Value {
    walk_schema(v, WalkMode::Jtd).expect("JTD conversion cannot reject")
}
fn normalize_node(v: &Value) -> Value {
    walk_schema(v, WalkMode::Normalize).expect("schema normalization cannot reject")
}
pub fn jtd_to_json_schema(v: &Value) -> Value {
    if is_jtd(v) {
        convert_jtd(v)
    } else {
        normalize_node(v)
    }
}
fn string_array(v: &Value, unique: bool) -> bool {
    v.as_array().is_some_and(|a| {
        a.iter().all(Value::is_string)
            && (!unique
                || a.iter()
                    .filter_map(Value::as_str)
                    .collect::<HashSet<_>>()
                    .len()
                    == a.len())
    })
}
pub fn valid_schema(v: &Value) -> bool {
    let mut pending = vec![v];
    while let Some(v) = pending.pop() {
        if !valid_schema_node(v, &mut pending) {
            return false;
        }
    }
    true
}
fn valid_schema_node<'a>(v: &'a Value, pending: &mut Vec<&'a Value>) -> bool {
    if v.is_boolean() {
        return true;
    }
    let Some(o) = v.as_object() else { return false };
    if let Some(t) = o.get("type") {
        let valid = |s: &Value| {
            s.as_str().is_some_and(|s| {
                matches!(
                    s,
                    "string" | "number" | "integer" | "boolean" | "object" | "array" | "null"
                )
            })
        };
        if !valid(t)
            && !t
                .as_array()
                .is_some_and(|a| !a.is_empty() && string_array(t, true) && a.iter().all(valid))
        {
            return false;
        }
    }
    for k in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if o.get(k).is_some_and(|x| {
            !x.as_array().is_some_and(|a| {
                a.iter().all(|v| {
                    pending.push(v);
                    true
                })
            })
        }) {
            return false;
        }
    }
    for k in [
        "not",
        "if",
        "then",
        "else",
        "propertyNames",
        "contains",
        "items",
        "additionalProperties",
        "unevaluatedProperties",
        "unevaluatedItems",
    ] {
        if o.get(k).is_some_and(|x| !{
            pending.push(x);
            true
        }) {
            return false;
        }
    }
    for k in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        if o.get(k).is_some_and(|x| {
            !x.as_object().is_some_and(|a| {
                a.values().all(|v| {
                    pending.push(v);
                    true
                })
            })
        }) {
            return false;
        }
    }
    if o.contains_key("additionalItems") || o.contains_key("dependencies") {
        return false;
    }
    if o.get("required").is_some_and(|x| !string_array(x, true)) {
        return false;
    }
    if let Some(d) = o.get("dependentRequired") {
        if !d
            .as_object()
            .is_some_and(|m| m.values().all(|x| string_array(x, false)))
        {
            return false;
        }
    }
    if let Some(e) = o.get("enum") {
        let Some(a) = e.as_array() else { return false };
        if a.is_empty() || a.iter().enumerate().any(|(i, x)| a[..i].contains(x)) {
            return false;
        }
    }
    for k in ["minimum", "maximum", "multipleOf"] {
        if o.get(k).is_some_and(|x| !x.is_number()) {
            return false;
        }
    }
    if o.get("multipleOf")
        .and_then(Value::as_f64)
        .is_some_and(|n| n <= 0.0)
    {
        return false;
    }
    for k in ["exclusiveMinimum", "exclusiveMaximum"] {
        if o.get(k).is_some_and(|x| !x.is_number() && !x.is_boolean()) {
            return false;
        }
    }
    for k in [
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
        "minProperties",
        "maxProperties",
        "minContains",
        "maxContains",
    ] {
        if o.get(k)
            .is_some_and(|x| !x.as_f64().is_some_and(|n| n >= 0.0 && n.fract() == 0.0))
        {
            return false;
        }
    }
    for k in [
        "uniqueItems",
        "nullable",
        "readOnly",
        "writeOnly",
        "deprecated",
    ] {
        if o.get(k).is_some_and(|x| !x.is_boolean()) {
            return false;
        }
    }
    for k in ["pattern", "format"] {
        if o.get(k).is_some_and(|x| !x.is_string()) {
            return false;
        }
    }
    if let Some(p) = o.get("pattern").and_then(Value::as_str) {
        if crate::js_regex::JsRegExp::new(p.into(), "").is_err() {
            return false;
        }
    }
    true
}
pub fn dereference(root: &Value) -> Value {
    if root.get("$defs").is_none() && root.get("definitions").is_none() {
        return clone_json(root);
    }
    enum Frame<'a> {
        Enter(&'a Value, HashSet<String>),
        Array(usize),
        Object(Vec<String>),
        Merge(&'a serde_json::Map<String, Value>),
    }
    let mut pending = vec![Frame::Enter(root, HashSet::new())];
    let mut values = Vec::new();
    while let Some(frame) = pending.pop() {
        match frame {
            Frame::Enter(value, seen) => match value {
                Value::Array(a) => {
                    pending.push(Frame::Array(a.len()));
                    for child in a.iter().rev() {
                        pending.push(Frame::Enter(child, seen.clone()));
                    }
                }
                Value::Object(o) => {
                    if let Some(reference) = o.get("$ref").and_then(Value::as_str) {
                        if seen.contains(reference) {
                            values.push(json!({}));
                            continue;
                        }
                        let key = reference
                            .strip_prefix("#/$defs/")
                            .map(|s| ("$defs", s))
                            .or_else(|| {
                                reference
                                    .strip_prefix("#/definitions/")
                                    .map(|s| ("definitions", s))
                            });
                        if let Some(target) = key
                            .and_then(|(defs, name)| root.get(defs).and_then(|defs| defs.get(name)))
                            .filter(|v| v.is_object())
                        {
                            let mut visiting = seen;
                            visiting.insert(reference.into());
                            pending.push(Frame::Merge(o));
                            pending.push(Frame::Enter(target, visiting));
                            continue;
                        }
                        values.push(clone_json(value));
                        continue;
                    }
                    let keys = o
                        .keys()
                        .filter(|k| !matches!(k.as_str(), "$defs" | "definitions"))
                        .cloned()
                        .collect::<Vec<_>>();
                    pending.push(Frame::Object(keys.clone()));
                    for key in keys.into_iter().rev() {
                        pending.push(Frame::Enter(&o[&key], seen.clone()));
                    }
                }
                _ => values.push(value.clone()),
            },
            Frame::Array(len) => {
                let children = values.split_off(values.len() - len);
                values.push(Value::Array(children));
            }
            Frame::Object(keys) => {
                let children = values.split_off(values.len() - keys.len());
                values.push(Value::Object(keys.into_iter().zip(children).collect()));
            }
            Frame::Merge(siblings) => {
                let mut value = values.pop().expect("reference child");
                if let Some(o) = value.as_object_mut() {
                    for (key, child) in siblings {
                        if key != "$ref" {
                            if let Some(old) = o.insert(key.clone(), clone_json(child)) {
                                drop_declaration(old)
                            }
                        }
                    }
                }
                values.push(value);
            }
        }
    }
    values.pop().expect("dereferenced root")
}
fn unresolved(value: &Value) -> bool {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(a) => pending.extend(a),
            Value::Object(o) => {
                if o.get("$ref").is_some_and(Value::is_string) {
                    return true;
                }
                pending.extend(
                    o.iter()
                        .filter(|(k, _)| {
                            !matches!(k.as_str(), "const" | "default" | "enum" | "examples")
                        })
                        .map(|(_, v)| v),
                );
            }
            _ => {}
        }
    }
    false
}
fn strict_schema(v: &Value) -> Option<Value> {
    walk_schema(v, WalkMode::Strict)
}
fn strict_node(original: &Value, value: Value) -> Option<Value> {
    let mut out = match value {
        Value::Object(out) => out,
        other => {
            drop_declaration(other);
            return None;
        }
    };
    if out.is_empty() {
        return None;
    }
    if let Some(types) = original.get("type").and_then(Value::as_array) {
        let mut variants = Vec::new();
        let source = Value::Object(out);
        for ty in types {
            let Some(name) = ty.as_str() else {
                drop_declaration(source);
                drop_declaration(Value::Array(variants));
                return None;
            };
            let Value::Object(mut branch) = clone_json(&source) else {
                unreachable!()
            };
            branch.insert("type".into(), ty.clone());
            if name != "object" {
                for k in [
                    "properties",
                    "required",
                    "additionalProperties",
                    "patternProperties",
                    "minProperties",
                    "maxProperties",
                    "dependentRequired",
                    "dependentSchemas",
                ] {
                    if let Some(v) = branch.remove(k) {
                        drop_declaration(v)
                    }
                }
            }
            if name != "array" {
                for k in [
                    "items",
                    "prefixItems",
                    "contains",
                    "minItems",
                    "maxItems",
                    "uniqueItems",
                    "minContains",
                    "maxContains",
                ] {
                    if let Some(v) = branch.remove(k) {
                        drop_declaration(v)
                    }
                }
            }
            // Children were already transformed by the postorder walker.
            let branch = Value::Object(branch);
            let transformed = strict_node(&branch, clone_json(&branch));
            drop_declaration(branch);
            let Some(transformed) = transformed else {
                drop_declaration(source);
                drop_declaration(Value::Array(variants));
                return None;
            };
            variants.push(transformed);
        }
        drop_declaration(source);
        return Some(Value::Object(Map::from_iter([(
            "anyOf".into(),
            Value::Array(variants),
        )])));
    }
    if out.contains_key("patternProperties")
        || out
            .get("additionalProperties")
            .is_some_and(|x| x != &Value::Bool(false))
    {
        drop_declaration(Value::Object(out));
        return None;
    }
    if let Some(Value::Object(props)) = out.remove("properties") {
        let required = original.get("required").and_then(Value::as_array);
        let mut p = Map::new();
        let names = props.keys().cloned().collect::<Vec<_>>();
        for (key, mut child) in props {
            if !required.is_some_and(|r| r.contains(&json!(key))) {
                child = Value::Object(Map::from_iter([(
                    "anyOf".into(),
                    Value::Array(vec![child, json!({"type":"null"})]),
                )]));
            }
            p.insert(key, child);
        }
        out.insert("type".into(), json!("object"));
        out.insert("properties".into(), Value::Object(p));
        out.insert("required".into(), json!(names));
        out.insert("additionalProperties".into(), json!(false));
    }
    if !out.contains_key("type")
        && !["anyOf", "oneOf", "allOf", "$ref", "enum", "const"]
            .iter()
            .any(|k| out.contains_key(*k))
    {
        drop_declaration(Value::Object(out));
        return None;
    }
    Some(Value::Object(out))
}
fn section_variants(schema: &Value) -> Value {
    if schema["type"] != "object" {
        return clone_json(schema);
    }
    let Some(props) = schema["properties"].as_object() else {
        return clone_json(schema);
    };
    let mut full = clone_json(schema);
    if let Some(o) = full.as_object_mut() {
        if let Some(description) = o.remove("description") {
            drop_declaration(description);
        }
    }
    let mut a = vec![full];
    for p in props.values() {
        for node in std::iter::once(p).chain((p["type"] == "array").then(|| &p["items"])) {
            if node.is_object() && !a.contains(node) {
                a.push(clone_json(node));
            }
        }
    }
    if a.len() == 1 {
        for value in a {
            drop_declaration(value)
        }
        return clone_json(schema);
    }
    let mut v = Value::Object(Map::from_iter([("anyOf".into(), Value::Array(a))]));
    if let Some(d) = schema.get("description") {
        v["description"] = clone_json(d)
    }
    v
}
// Rust reference Host resource boundary, not an OMP protocol depth limit.
// The source's JS try/catch falls back for the fixed 20,000-layer test. Keep
// ordinary/mid-depth schemas constrained; reject only this resource class
// before preparing duplicate trees or recursive host/provider consumers.
fn within_preprocess_depth(value: &Value) -> bool {
    let mut pending = vec![(value, 0usize)];
    while let Some((value, depth)) = pending.pop() {
        if depth >= 20_000 {
            return false;
        }
        match value {
            Value::Array(a) => pending.extend(a.iter().map(|v| (v, depth + 1))),
            Value::Object(o) => pending.extend(o.values().map(|v| (v, depth + 1))),
            _ => {}
        }
    }
    true
}
// Explicit worklist disposal prevents a rejected deep declaration from causing
// recursive serde_json::Value drop on the Native owner destruction path.
pub(crate) fn drop_declaration(value: Value) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(a) => pending.extend(a),
            Value::Object(o) => pending.extend(o.into_iter().map(|(_, v)| v)),
            _ => {}
        }
    }
}
pub fn build(schema: Option<&Value>) -> OutputSchema {
    if schema.is_some_and(|v| !within_preprocess_depth(v)) {
        let mut fallback = build(None);
        fallback.error = Some("output schema exceeds safe preprocessing depth".into());
        return fallback;
    }
    let (normalized, error) = match schema {
        None | Some(Value::Null) => (None, None),
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e.to_string())),
        },
        Some(v) => (Some(clone_json(v)), None),
    };
    let mut error = error;
    let mut converted = None;
    if let Some(n) = &normalized {
        let c = jtd_to_json_schema(n);
        if c == Value::Bool(false) {
            error = Some("boolean false schema rejects all outputs".into());
            drop_declaration(c);
        } else if c == Value::Bool(true) {
            drop_declaration(c);
        } else if !valid_schema(&c) {
            error = Some("invalid JSON schema".into());
            drop_declaration(c);
        } else {
            converted = Some(c);
        }
    }
    let constrained = error.is_none() && converted.is_some();
    let (mut strict, mut data) = (
        false,
        json!({"type":"object","additionalProperties":true,"description":"Structured JSON output (no schema specified)"}),
    );
    if constrained {
        let c = converted.as_ref().unwrap();
        let s = strict_schema(c);
        strict = s.is_some();
        let dereferenced = dereference(s.as_ref().unwrap_or(c));
        if let Some(strict_tree) = s {
            drop_declaration(strict_tree);
        }
        drop_declaration(std::mem::replace(&mut data, dereferenced));
        if unresolved(&data) {
            error = Some("schema contains unresolved $ref after dereferencing".into());
            strict = false;
            if let Some(rejected) = converted.take() {
                drop_declaration(rejected);
            }
            let loose = json!({"type":"object","additionalProperties":true});
            drop_declaration(std::mem::replace(&mut data, loose));
        } else {
            let variants = section_variants(&data);
            drop_declaration(std::mem::replace(&mut data, variants));
        }
    } else if normalized == Some(Value::Bool(true)) && error.is_none() {
        data = json!({})
    }
    let mut parameters = json!({"type":"object","additionalProperties":false,"description":"submit data or error","properties":{"type":{"anyOf":[{"type":"string"},{"type":"array","minItems":1,"items":{"type":"string"}}],"description":"Optional result type. A non-empty string array is incremental; a string is terminal."},"data":null,"error":{"type":"string","description":"Failure reason; mutually exclusive with data"}},"required":[]});
    parameters["properties"]["data"] = data;
    OutputSchema {
        normalized,
        json_schema: if constrained && error.is_none() {
            converted
        } else {
            None
        },
        error,
        strict,
        parameters,
    }
}

pub(crate) fn get(
    heap: &Arc<ValueHeap>,
    o: &ValueHandle,
    k: &str,
    r: &dyn ValueRuntime,
) -> GResult<ValueHandle> {
    heap.get(o, &k.into(), r)
}
pub(crate) fn put(
    heap: &Arc<ValueHeap>,
    o: &ValueHandle,
    k: &str,
    v: ValueHandle,
    r: &dyn ValueRuntime,
) -> GResult<()> {
    heap.define_own_data(o, k.into(), v, r)
}
pub(crate) fn undefined(heap: &Arc<ValueHeap>, v: &ValueHandle) -> GResult<bool> {
    Ok(heap.kind(v)? == ValueKind::Undefined)
}
pub(crate) fn nullish(heap: &Arc<ValueHeap>, v: &ValueHandle) -> GResult<bool> {
    Ok(matches!(
        heap.kind(v)?,
        ValueKind::Undefined | ValueKind::Null
    ))
}
pub(crate) fn string(heap: &Arc<ValueHeap>, v: &ValueHandle) -> GResult<WireString> {
    match heap.snapshot(v)? {
        ValueSnapshot::String(s) => Ok(s),
        _ => Err(ValueError::TypeError("Expected a string".into())),
    }
}
pub(crate) fn members(
    heap: &Arc<ValueHeap>,
    a: &ValueHandle,
    r: &dyn ValueRuntime,
) -> GResult<Vec<ValueHandle>> {
    (0..heap.array_length(a, r)?)
        .map(|i| get(heap, a, &i.to_string(), r))
        .collect()
}
pub(crate) fn validate(
    heap: &Arc<ValueHeap>,
    schema: &ValueHandle,
    value: &ValueHandle,
    r: &dyn ValueRuntime,
) -> GResult<Option<String>> {
    if nullish(heap, schema)? {
        return Ok(None);
    }
    let result = ara_ai::schema_validator::validate_graph_json_schema_value_with_execution(
        GraphExecution::new(heap, r),
        schema,
        value,
    )?;
    if result.success {
        return Ok(None);
    }
    let mut messages = Vec::new();
    for issue in result.issues {
        let path = issue
            .path
            .iter()
            .map(|part| {
                heap.to_string(part, r).map(|s| {
                    s.to_utf8()
                        .unwrap_or_else(|_| String::from_utf16_lossy(s.units()))
                })
            })
            .collect::<GResult<Vec<_>>>()?
            .join("/");
        let message = issue
            .message
            .to_utf8()
            .unwrap_or_else(|_| String::from_utf16_lossy(issue.message.units()));
        messages.push(if path.is_empty() {
            message
        } else {
            format!("{path}: {message}")
        });
    }
    Ok(Some(if messages.is_empty() {
        "Unknown schema validation error.".into()
    } else {
        messages.join("; ")
    }))
}
pub(crate) fn normalize_graph(
    heap: &Arc<ValueHeap>,
    schema: &ValueHandle,
    value: &ValueHandle,
    r: &dyn ValueRuntime,
) -> GResult<ValueHandle> {
    if heap.kind(schema)? != ValueKind::Object {
        return Ok(value.clone());
    }
    let all = get(heap, schema, "allOf", r)?;
    let mut current = value.clone();
    if heap.kind(&all)? == ValueKind::Array {
        for branch in members(heap, &all, r)? {
            current = normalize_graph(heap, &branch, &current, r)?;
        }
    }
    for keyword in ["oneOf", "anyOf"] {
        let branches = get(heap, schema, keyword, r)?;
        if heap.kind(&branches)? == ValueKind::Array {
            if validate(heap, schema, &current, r)?.is_none() {
                return Ok(current);
            }
            for branch in members(heap, &branches, r)? {
                let candidate = normalize_graph(heap, &branch, &current, r)?;
                if !candidate.same_identity(&current)
                    && validate(heap, schema, &candidate, r)?.is_none()
                {
                    return Ok(candidate);
                }
            }
        }
    }
    if heap.kind(&current)? == ValueKind::Array {
        let prefix = get(heap, schema, "prefixItems", r)?;
        let items = get(heap, schema, "items", r)?;
        let mut changed = false;
        let mut values = Vec::new();
        for (i, old) in members(heap, &current, r)?.into_iter().enumerate() {
            let child = if heap.kind(&prefix)? == ValueKind::Array
                && (i as u32) < heap.array_length(&prefix, r)?
            {
                get(heap, &prefix, &i.to_string(), r)?
            } else {
                items.clone()
            };
            let next = normalize_graph(heap, &child, &old, r)?;
            changed |= !old.same_identity(&next);
            values.push(next);
        }
        return if changed {
            heap.array(values)
        } else {
            Ok(current)
        };
    }
    if heap.kind(&current)? != ValueKind::Object {
        return Ok(current);
    }
    let props = get(heap, schema, "properties", r)?;
    if heap.kind(&props)? != ValueKind::Object {
        return Ok(current);
    }
    let required = get(heap, schema, "required", r)?;
    let required = if heap.kind(&required)? == ValueKind::Array {
        members(heap, &required, r)?
    } else {
        Vec::new()
    };
    let mut out = None;
    for key in heap.own_enumerable_string_keys(&props, r)? {
        if heap
            .own_descriptor(&current, &key.clone().into(), r)?
            .is_none()
        {
            continue;
        }
        let child = heap.get(&props, &key.clone().into(), r)?;
        let old = heap.get(&current, &key.clone().into(), r)?;
        let is_required = required
            .iter()
            .any(|v| matches!(heap.snapshot(v),Ok(ValueSnapshot::String(s)) if s==key));
        let remove = heap.kind(&old)? == ValueKind::Null
            && !is_required
            && validate(heap, &child, &old, r)?.is_some();
        let next = if remove {
            None
        } else {
            Some(normalize_graph(heap, &child, &old, r)?)
        };
        if remove || next.as_ref().is_some_and(|n| !n.same_identity(&old)) {
            if out.is_none() {
                out = Some(heap.spread(&current, r)?)
            }
            let target = out.as_ref().unwrap();
            if let Some(next) = next {
                heap.define_own_data(target, key.into(), next, r)?;
            } else {
                heap.delete_own(target, &key.into(), r)?;
            }
        }
    }
    Ok(out.unwrap_or(current))
}
fn closed(
    heap: &Arc<ValueHeap>,
    s: &ValueHandle,
    r: &dyn ValueRuntime,
) -> GResult<Vec<ValueHandle>> {
    let mut out = Vec::new();
    if heap.kind(s)? != ValueKind::Object {
        return Ok(out);
    }
    if matches!(
        heap.snapshot(&get(heap, s, "additionalProperties", r)?)?,
        ValueSnapshot::Boolean(false)
    ) {
        out.push(s.clone())
    }
    let a = get(heap, s, "allOf", r)?;
    if heap.kind(&a)? == ValueKind::Array {
        for x in members(heap, &a, r)? {
            out.extend(closed(heap, &x, r)?);
        }
    }
    Ok(out)
}
fn accepts(
    heap: &Arc<ValueHeap>,
    s: &ValueHandle,
    label: &WireString,
    r: &dyn ValueRuntime,
) -> GResult<bool> {
    let p = get(heap, s, "properties", r)?;
    if heap.kind(&p)? == ValueKind::Object && heap.has(&p, &label.clone().into(), r)? {
        return Ok(true);
    }
    let patterns = get(heap, s, "patternProperties", r)?;
    if heap.kind(&patterns)? == ValueKind::Object {
        for pattern in heap.own_enumerable_string_keys(&patterns, r)? {
            if let Ok(re) = heap.regexp(pattern, "") {
                let test = get(heap, &re, "test", r)?;
                let answer = heap.call(&test, &re, &[heap.string(label.clone())?], r)?;
                if heap.truthy(&answer)? {
                    return Ok(true);
                }
            }
        }
    }
    Ok(!matches!(
        heap.snapshot(&get(heap, s, "additionalProperties", r)?)?,
        ValueSnapshot::Boolean(false)
    ))
}
pub(crate) fn known_label(
    heap: &Arc<ValueHeap>,
    s: &ValueHandle,
    label: &WireString,
    r: &dyn ValueRuntime,
) -> GResult<bool> {
    for c in closed(heap, s, r)? {
        if !accepts(heap, &c, label, r)? {
            return Ok(false);
        }
    }
    for key in ["oneOf", "anyOf"] {
        let a = get(heap, s, key, r)?;
        if heap.kind(&a)? == ValueKind::Array {
            let mut choices = Vec::new();
            let mut all_closed = true;
            for variant in members(heap, &a, r)? {
                let nodes = closed(heap, &variant, r)?;
                if nodes.is_empty() {
                    all_closed = false;
                    break;
                }
                choices.push(nodes)
            }
            if all_closed && !choices.is_empty() {
                let mut found = false;
                for nodes in choices {
                    let mut yes = true;
                    for c in nodes {
                        yes &= accepts(heap, &c, label, r)?;
                    }
                    found |= yes;
                }
                if !found {
                    return Ok(false);
                }
            }
        }
    }
    let a = get(heap, s, "allOf", r)?;
    if heap.kind(&a)? == ValueKind::Array {
        for c in members(heap, &a, r)? {
            if !known_label(heap, &c, label, r)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
pub(crate) fn section_schema(
    heap: &Arc<ValueHeap>,
    schema: &ValueHandle,
    label: &WireString,
    r: &dyn ValueRuntime,
) -> GResult<ValueHandle> {
    if heap.kind(schema)? != ValueKind::Object {
        return heap.undefined();
    }
    let props = get(heap, schema, "properties", r)?;
    if heap.kind(&props)? != ValueKind::Object {
        return heap.undefined();
    }
    let child = heap.get(&props, &label.clone().into(), r)?;
    if nullish(heap, &child)? {
        return heap.undefined();
    }
    let ty = get(heap, &child, "type", r)?;
    if matches!(heap.snapshot(&ty)?,ValueSnapshot::String(s) if s.equals_ascii("array")) {
        let item = get(heap, &child, "items", r)?;
        if !nullish(heap, &item)? {
            return Ok(item);
        }
    }
    Ok(child)
}
