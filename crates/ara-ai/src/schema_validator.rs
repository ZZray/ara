//! Fixed OMP JSON Schema checker with one Wire/actual Graph engine.
//!
//! Source: OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! `packages/ai/src/utils/schema/json-schema-validator.ts` and `equality.ts`.
//! Wire inputs are immutable trees; actual inputs retain arena aliases, getters,
//! methods and iterator effects. This module does not normalize arguments or
//! implement authored runtime schemas. The serde facade expresses only its
//! representable scalar domain; it is not a lone-surrogate entrypoint.
/*
MIT License

Copyright (c) 2025 Mario Zechner
Copyright (c) 2025-2026 Can Bölük
Copyright (c) 2026 Stencil Labs, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
*/

mod view;
pub(crate) mod property_owner;
pub use property_owner::{graph_schema_defines_property_in, schema_defines_property};

use crate::graph_values::{GraphExecution, GraphValueFactory};
use ara_rpc::json::{WireString, WireValue};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueHeap, ValueRuntime,
};
use regress::{Flags, Regex};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use view::{LoopControl, NodeIdentity, ReadContext, ValueRef, number_message};

const MAX_REF_DEPTH: usize = 64;
const UNEVALUATED_WARNING: &str = "JSON Schema unevaluatedProperties/unevaluatedItems are not enforced by the in-tree validator; treating as permissive";
static SEEN_UNEVALUATED_WARNING: AtomicBool = AtomicBool::new(false);

// Each fixed source occurrence keeps its own fresh anonymous Arrow definition.
// Only source graph values are captured; readers and equality scratch are borrowed.
static TAG_ENUM_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:json-schema-validator.ts:93",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: tag_enum_callback,
};
static TYPE_FILTER_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:json-schema-validator.ts:148",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: type_filter_callback,
};
static REQUIRED_EVERY_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:json-schema-validator.ts:179",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: required_every_callback,
};
static TYPE_MATCH_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:json-schema-validator.ts:241",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: type_match_callback,
};
static ENUM_VALUE_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:json-schema-validator.ts:254",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: enum_value_callback,
};
fn checker_argument(heap: &Arc<ValueHeap>, arguments: &[ValueHandle]) -> Result<ValueHandle, ValueError> {
    arguments.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())
}
fn checker_string_predicate(
    heap: &Arc<ValueHeap>,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let entry = checker_argument(heap, arguments)?;
    let reader = ReadContext::Graph { factory: GraphExecution::new(heap, runtime), runtime };
    heap.boolean(reader.string(&ValueRef::Graph(entry))?.is_some())
}
fn checker_equal_predicate(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let entry = checker_argument(heap, arguments)?;
    let reader = ReadContext::Graph { factory: GraphExecution::new(heap, runtime), runtime };
    heap.boolean(reader.equal(&ValueRef::Graph(entry), &ValueRef::Graph(captures[0].clone()))?)
}
fn tag_enum_callback(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _receiver: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    checker_equal_predicate(heap, captures, arguments, runtime)
}
fn type_filter_callback(
    heap: &Arc<ValueHeap>,
    _captures: &[ValueHandle],
    _receiver: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    checker_string_predicate(heap, arguments, runtime)
}
fn required_every_callback(
    heap: &Arc<ValueHeap>,
    _captures: &[ValueHandle],
    _receiver: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    checker_string_predicate(heap, arguments, runtime)
}
fn type_match_callback(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _receiver: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let kind = checker_argument(heap, arguments)?;
    let reader = ReadContext::Graph { factory: GraphExecution::new(heap, runtime), runtime };
    heap.boolean(reader.matches_type(&ValueRef::Graph(captures[0].clone()), &ValueRef::Graph(kind))?)
}
fn enum_value_callback(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _receiver: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    checker_equal_predicate(heap, captures, arguments, runtime)
}

/// Native property paths distinguish string keys from numeric array indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonSchemaPathSegment {
    Property(WireString),
    Index(usize),
}

/// Defaults retain the public immutable Wire API. Actual issues retain the
/// selected expectedTypes value and original path values in the same arena.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonSchemaValidationIssue<E = Vec<WireString>, P = JsonSchemaPathSegment> {
    pub path: Vec<P>,
    pub message: WireString,
    pub expected_types: Option<E>,
    pub keyword: Option<&'static str>,
    pub from_union_branch: bool,
}

impl JsonSchemaValidationIssue {
    pub fn to_wire_value(&self) -> WireValue {
        let mut result = WireValue::object(vec![
            (
                "path",
                WireValue::Array(
                    self.path
                        .iter()
                        .map(|part| match part {
                            JsonSchemaPathSegment::Property(key) => WireValue::String(key.clone()),
                            JsonSchemaPathSegment::Index(index) => WireValue::Number(*index as f64),
                        })
                        .collect(),
                ),
            ),
            ("message", WireValue::String(self.message.clone())),
        ]);
        if let Some(types) = &self.expected_types {
            result.insert("expectedTypes", WireValue::Array(types.iter().cloned().map(WireValue::String).collect()));
        }
        if let Some(keyword) = self.keyword {
            result.insert("keyword", WireValue::String(keyword.into()));
        }
        if self.from_union_branch {
            result.insert("fromUnionBranch", WireValue::Bool(true));
        }
        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonSchemaValidationResult<E = Vec<WireString>, P = JsonSchemaPathSegment> {
    pub success: bool,
    pub issues: Vec<JsonSchemaValidationIssue<E, P>>,
}

impl JsonSchemaValidationResult {
    pub fn to_wire_value(&self) -> WireValue {
        WireValue::object(vec![
            ("success", WireValue::Bool(self.success)),
            ("issues", WireValue::Array(self.issues.iter().map(JsonSchemaValidationIssue::to_wire_value).collect())),
        ])
    }
}

#[derive(Clone, Debug)]
enum PathPart<'a> {
    Property(WireString),
    Index(usize),
    Actual(ValueRef<'a>),
}
type InternalIssue<'a> = JsonSchemaValidationIssue<ValueRef<'a>, PathPart<'a>>;

struct ValidationContext<'a, 'w> {
    root: ValueRef<'a>,
    seen_pairs: HashSet<(WireString, NodeIdentity)>,
    ref_depth: usize,
    warning_seen: &'w AtomicBool,
    warn: &'w mut dyn FnMut(&str),
}

/// Validate without mutation. Each call owns fresh ref and issue state.
pub fn validate_json_schema_value(schema: &WireValue, value: &WireValue) -> JsonSchemaValidationResult {
    validate_with_warning(schema, value, &SEEN_UNEVALUATED_WARNING, &mut |message| eprintln!("{message}"))
}

pub fn is_json_schema_value_valid(schema: &WireValue, value: &WireValue) -> bool {
    validate_json_schema_value(schema, value).success
}

/// Reuse the fixed equality engine for Graph draft merge rules.
// Retained contributor facade; borrowed consumers share the same engine.
#[allow(dead_code)]
pub(crate) fn graph_json_values_equal(
    factory: &GraphValueFactory,
    left: &ValueHandle,
    right: &ValueHandle,
) -> Result<bool, ValueError> {
    graph_json_values_equal_with_runtime(factory, left, right, factory.runtime())
}

/// A predicate callback must retain its effective operation-local runtime.
// Retained contributor facade; borrowed consumers share the same engine.
#[allow(dead_code)]
pub(crate) fn graph_json_values_equal_with_runtime(
    factory: &GraphValueFactory,
    left: &ValueHandle,
    right: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<bool, ValueError> {
    graph_json_values_equal_with_execution(GraphExecution::new(factory.heap(), runtime), left, right)
}

pub(crate) fn graph_json_values_equal_with_execution(
    execution: GraphExecution<'_>,
    left: &ValueHandle,
    right: &ValueHandle,
) -> Result<bool, ValueError> {
    let factory = execution;
    let runtime = execution.runtime();
    let left = ValueRef::Graph(factory.heap().adopt(left)?);
    let right = ValueRef::Graph(factory.heap().adopt(right)?);
    ReadContext::Graph { factory, runtime }.equal(&left, &right)
}

/// Fixed stamps.once compares an actual mutable slot with a Native Number
/// epoch. Forward to existing relational semantics, including ToPrimitive,
/// mixed BigInt/Number and NaN, without a second comparison implementation.
// Retained contributor facade; borrowed consumers share the same engine.
#[allow(dead_code)]
pub(crate) fn graph_value_greater_equal_number_with_runtime(
    factory: &GraphValueFactory,
    left: &ValueHandle,
    right: f64,
    runtime: &dyn ValueRuntime,
) -> Result<bool, ValueError> {
    graph_value_greater_equal_number_with_execution(GraphExecution::new(factory.heap(), runtime), left, right)
}

pub(crate) fn graph_value_greater_equal_number_with_execution(
    execution: GraphExecution<'_>,
    left: &ValueHandle,
    right: f64,
) -> Result<bool, ValueError> {
    let factory = execution;
    let runtime = execution.runtime();
    let left = ValueRef::Graph(factory.heap().adopt(left)?);
    ReadContext::Graph { factory, runtime }.compare(&left, &ValueRef::Number(right), ">=")
}

/// P15 Native pair-decoding compares an actual iterator length to Number(2).
pub(crate) fn graph_value_less_than_number_with_runtime(
    factory: &GraphValueFactory,
    value: &ValueHandle,
    number: f64,
    runtime: &dyn ValueRuntime,
) -> Result<bool, ValueError> {
    graph_value_less_than_number_with_execution(GraphExecution::new(factory.heap(), runtime), value, number)
}

pub(crate) fn graph_value_less_than_number_with_execution(
    execution: GraphExecution<'_>,
    value: &ValueHandle,
    number: f64,
) -> Result<bool, ValueError> {
    let factory = execution;
    let runtime = execution.runtime();
    let value = ValueRef::Graph(factory.heap().adopt(value)?);
    ReadContext::Graph { factory, runtime }.compare(&value, &ValueRef::Number(number), "<")
}

/// Compare a Native numeric loop index to an actual Graph value, preserving
/// number-hint ToPrimitive and exact mixed Number/BigInt relational semantics.
pub(crate) fn graph_numeric_less_than(
    factory: &GraphValueFactory,
    left: f64,
    right: &ValueHandle,
) -> Result<bool, ValueError> {
    graph_numeric_less_than_with_runtime(factory, left, right, factory.runtime())
}

pub(crate) fn graph_numeric_less_than_with_runtime(
    factory: &GraphValueFactory,
    left: f64,
    right: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<bool, ValueError> {
    graph_numeric_less_than_with_execution(GraphExecution::new(factory.heap(), runtime), left, right)
}

pub(crate) fn graph_numeric_less_than_with_execution(
    execution: GraphExecution<'_>,
    left: f64,
    right: &ValueHandle,
) -> Result<bool, ValueError> {
    let factory = execution;
    let runtime = execution.runtime();
    let right = ValueRef::Graph(factory.heap().adopt(right)?);
    ReadContext::Graph { factory, runtime }.compare(&ValueRef::Number(left), &right, "<")
}

/// Two actual operands use the existing relational engine and effective runtime.
pub(crate) fn graph_values_compare_with_runtime(
    factory: &GraphValueFactory,
    left: &ValueHandle,
    right: &ValueHandle,
    operator: &str,
    runtime: &dyn ValueRuntime,
) -> Result<bool, ValueError> {
    graph_values_compare_with_execution(GraphExecution::new(factory.heap(), runtime), left, right, operator)
}

pub(crate) fn graph_values_compare_with_execution(
    execution: GraphExecution<'_>,
    left: &ValueHandle,
    right: &ValueHandle,
    operator: &str,
) -> Result<bool, ValueError> {
    let factory = execution;
    let runtime = execution.runtime();
    let left = ValueRef::Graph(factory.heap().adopt(left)?);
    let right = ValueRef::Graph(factory.heap().adopt(right)?);
    ReadContext::Graph { factory, runtime }.compare(&left, &right, operator)
}

/// Actual entrypoint: no serialization, tree materialization or alias inference.
/// Errors remain errors; success=false is reserved for ordinary schema rejection.
pub fn validate_graph_json_schema_value(
    factory: &GraphValueFactory,
    schema: &ValueHandle,
    value: &ValueHandle,
) -> Result<JsonSchemaValidationResult<ValueHandle, ValueHandle>, ValueError> {
    validate_graph_json_schema_value_with_runtime(factory, schema, value, factory.runtime())
}

/// Preserve the current operation-local callback resolver through validation.
pub(crate) fn validate_graph_json_schema_value_with_runtime(
    factory: &GraphValueFactory,
    schema: &ValueHandle,
    value: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<JsonSchemaValidationResult<ValueHandle, ValueHandle>, ValueError> {
    validate_graph_json_schema_value_with_execution(GraphExecution::new(factory.heap(), runtime), schema, value)
}

/// Validate with the operation-local GraphExecution resolver retained by the caller.
pub fn validate_graph_json_schema_value_with_execution(
    execution: GraphExecution<'_>,
    schema: &ValueHandle,
    value: &ValueHandle,
) -> Result<JsonSchemaValidationResult<ValueHandle, ValueHandle>, ValueError> {
    let factory = execution;
    let runtime = execution.runtime();
    let read = ReadContext::Graph { factory, runtime };
    // Admission happens before executing any getter or callback.
    let schema = ValueRef::Graph(factory.heap().adopt(schema)?);
    let value = ValueRef::Graph(factory.heap().adopt(value)?);
    let mut issues = Vec::new();
    let mut warn = |message: &str| eprintln!("{message}");
    let mut context = ValidationContext {
        root: schema.clone(),
        seen_pairs: HashSet::new(),
        ref_depth: 0,
        warning_seen: &SEEN_UNEVALUATED_WARNING,
        warn: &mut warn,
    };
    let success = validate_node(schema, value, &[], &read, &mut context, &mut issues)?;
    let issues = issues
        .into_iter()
        .map(|issue| {
            let path = issue
                .path
                .into_iter()
                .map(|part| {
                    let value = match part {
                        PathPart::Property(key) => ValueRef::String(key),
                        PathPart::Index(index) => ValueRef::Number(index as f64),
                        PathPart::Actual(value) => value,
                    };
                    read.graph_handle(&value)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let expected_types = issue.expected_types.as_ref().map(|value| read.graph_handle(value)).transpose()?;
            Ok(JsonSchemaValidationIssue {
                path,
                message: issue.message,
                expected_types,
                keyword: issue.keyword,
                from_union_branch: issue.from_union_branch,
            })
        })
        .collect::<Result<Vec<_>, ValueError>>()?;
    Ok(JsonSchemaValidationResult { success, issues })
}

fn validate_with_warning(
    schema: &WireValue,
    value: &WireValue,
    warning_seen: &AtomicBool,
    warn: &mut dyn FnMut(&str),
) -> JsonSchemaValidationResult {
    let read = ReadContext::Wire;
    let mut issues = Vec::new();
    let mut context = ValidationContext {
        root: ValueRef::Wire(schema),
        seen_pairs: HashSet::new(),
        ref_depth: 0,
        warning_seen,
        warn,
    };
    let success = validate_node(ValueRef::Wire(schema), ValueRef::Wire(value), &[], &read, &mut context, &mut issues)
        .expect("Wire checker has no runtime boundary");
    let issues = issues
        .into_iter()
        .map(|issue| JsonSchemaValidationIssue {
            path: issue
                .path
                .into_iter()
                .map(|part| match part {
                    PathPart::Property(key) => JsonSchemaPathSegment::Property(key),
                    PathPart::Index(index) => JsonSchemaPathSegment::Index(index),
                    PathPart::Actual(value) => {
                        if let Some(key) = read.string(&value).expect("Wire read") {
                            JsonSchemaPathSegment::Property(key)
                        } else {
                            JsonSchemaPathSegment::Index(
                                read.number(&value).expect("Wire read").expect("Wire suffix index is numeric") as usize,
                            )
                        }
                    }
                })
                .collect(),
            message: issue.message,
            expected_types: issue.expected_types.map(|value| read.wire_types(value)),
            keyword: issue.keyword,
            from_union_branch: issue.from_union_branch,
        })
        .collect();
    JsonSchemaValidationResult { success, issues }
}

/// serde supplies its stored key order and only its representable scalar domain.
pub fn validate_serde_json_schema_value(
    schema: &serde_json::Value,
    value: &serde_json::Value,
) -> JsonSchemaValidationResult {
    fn convert(value: &serde_json::Value) -> WireValue {
        match value {
            serde_json::Value::Null => WireValue::Null,
            serde_json::Value::Bool(value) => WireValue::Bool(*value),
            serde_json::Value::Number(value) => {
                WireValue::Number(value.as_f64().expect("serde JSON number has an f64 representation"))
            }
            serde_json::Value::String(value) => WireValue::String(value.as_str().into()),
            serde_json::Value::Array(values) => WireValue::Array(values.iter().map(convert).collect()),
            serde_json::Value::Object(values) => {
                WireValue::Object(values.iter().map(|(key, value)| (key.as_str().into(), convert(value))).collect())
            }
        }
    }
    validate_json_schema_value(&convert(schema), &convert(value))
}

fn push_issue<'a>(
    issues: &mut Vec<InternalIssue<'a>>,
    path: &[PathPart<'a>],
    message: impl Into<WireString>,
    keyword: &'static str,
) {
    issues.push(JsonSchemaValidationIssue {
        path: path.to_vec(),
        message: message.into(),
        expected_types: None,
        keyword: Some(keyword),
        from_union_branch: false,
    });
}
fn property_path<'a>(path: &[PathPart<'a>], key: &WireString) -> Vec<PathPart<'a>> {
    let mut result = path.to_vec();
    result.push(PathPart::Property(key.clone()));
    result
}
fn index_path<'a>(path: &[PathPart<'a>], index: usize) -> Vec<PathPart<'a>> {
    let mut result = path.to_vec();
    result.push(PathPart::Index(index));
    result
}
fn actual_path<'a>(path: &[PathPart<'a>], value: ValueRef<'a>) -> Vec<PathPart<'a>> {
    let mut result = path.to_vec();
    result.push(PathPart::Actual(value));
    result
}
fn interpolated(prefix: &str, value: &WireString, suffix: &str) -> WireString {
    let mut units: Vec<u16> = prefix.encode_utf16().collect();
    units.extend_from_slice(value.units());
    units.extend(suffix.encode_utf16());
    WireString::from_units(units)
}

fn schema_types<'a>(schema: &ValueRef<'a>, read: &ReadContext<'_>) -> Result<ValueRef<'a>, ValueError> {
    let raw = read.get(schema, "type")?;
    let types = if read.string(&raw)?.is_some() {
        read.make_array(vec![raw])?
    } else if read.is_array(&raw)? {
        read.predicate(&raw, "filter", &TYPE_FILTER_CALLBACK, &[], |entry, read| Ok(read.string(&entry)?.is_some()))?
    } else {
        read.make_array(Vec::new())?
    };
    if read.boolean(&read.get(schema, "nullable")?)? == Some(true)
        && !read.truthy(&read.call(&types, "includes", &[ValueRef::String("null".into())])?)?
    {
        return read.spread_append(&types, ValueRef::String("null".into()));
    }
    Ok(types)
}

fn decode_pointer_token(token: &[u16]) -> WireString {
    fn replace(units: &[u16], second: u16, replacement: u16) -> Vec<u16> {
        let mut result = Vec::new();
        let mut index = 0;
        while index < units.len() {
            if units[index] == u16::from(b'~') && units.get(index + 1) == Some(&second) {
                result.push(replacement);
                index += 2;
            } else {
                result.push(units[index]);
                index += 1;
            }
        }
        result
    }
    WireString::from_units(replace(&replace(token, u16::from(b'1'), u16::from(b'/')), u16::from(b'0'), u16::from(b'~')))
}
fn resolve_local_ref<'a>(
    root: ValueRef<'a>,
    reference: &WireString,
    read: &ReadContext<'_>,
) -> Result<ValueRef<'a>, ValueError> {
    if reference.equals_ascii("#") {
        return Ok(root);
    }
    if !reference.units().starts_with(&[u16::from(b'#'), u16::from(b'/')]) {
        return Ok(ValueRef::Undefined);
    }
    let mut current = root;
    for token in reference.units()[2..].split(|&unit| unit == u16::from(b'/')) {
        if !read.is_object(&current)? && !read.is_array(&current)? {
            return Ok(ValueRef::Undefined);
        }
        current = read.property(&current, &decode_pointer_token(token))?;
    }
    Ok(current)
}
fn is_required_set(value: &ValueRef<'_>, read: &ReadContext<'_>) -> Result<bool, ValueError> {
    Ok(read.is_array(value)?
        && read.truthy(&read.predicate(value, "every", &REQUIRED_EVERY_CALLBACK, &[], |entry, read| {
            Ok(read.string(&entry)?.is_some())
        })?)?)
}
fn is_tag_selected_branch(
    branch: &ValueRef<'_>,
    value: &ValueRef<'_>,
    read: &ReadContext<'_>,
) -> Result<bool, ValueError> {
    if !read.is_object(branch)? || !read.is_object(value)? {
        return Ok(false);
    }
    let properties = read.get(branch, "properties")?;
    if !read.is_object(&properties)? {
        return Ok(false);
    }
    let mut matched = false;
    for key in read.for_in_keys(&properties)? {
        let schema = read.property(&properties, &key)?;
        if !read.is_object(&schema)? {
            continue;
        }
        let has_const = read.has_own(&schema, &WireString::from("const"))?;
        let enums =
            if read.is_array(&read.get(&schema, "enum")?)? { read.get(&schema, "enum")? } else { ValueRef::Undefined };
        if !has_const && !read.truthy(&enums)? {
            continue;
        }
        if !read.has_own(value, &key)? {
            return Ok(false);
        }
        let candidate = read.property(value, &key)?;
        if has_const {
            if !read.equal(&candidate, &read.get(&schema, "const")?)? {
                return Ok(false);
            }
        } else if read.truthy(&enums)?
            && !read.truthy(&read.predicate(
                &enums,
                "some",
                &TAG_ENUM_CALLBACK,
                std::slice::from_ref(&candidate),
                |entry, read| read.equal(&entry, &candidate),
            )?)?
        {
            return Ok(false);
        }
        matched = true;
    }
    Ok(matched)
}

// Keep nonrecursive keyword temporaries out of the recursive dispatch frame.
// Observable Native property/method/iterator order is unchanged.
#[inline(never)]
fn validate_type_keyword<'a>(
    schema: &ValueRef<'a>,
    value: &ValueRef<'a>,
    path: &[PathPart<'a>],
    read: &ReadContext<'_>,
    issues: &mut Vec<InternalIssue<'a>>,
) -> Result<bool, ValueError> {
    let types = schema_types(schema, read)?;
    if read.compare(&read.get(&types, "length")?, &ValueRef::Number(0.0), ">")? {
        let candidate = (*value).clone();
        if !read.truthy(&read.predicate(
            &types,
            "some",
            &TYPE_MATCH_CALLBACK,
            std::slice::from_ref(&candidate),
            |kind, read| read.matches_type(&candidate, &kind),
        )?)? {
            let joined = read.call(&types, "join", &[ValueRef::String(" or ".into())])?;
            let message = interpolated(
                "expected ",
                &read.implicit_string(&joined)?,
                &format!(", received {}", read.received_type(value)?),
            );
            issues.push(JsonSchemaValidationIssue {
                path: path.to_vec(),
                message,
                expected_types: Some(types),
                keyword: Some("type"),
                from_union_branch: false,
            });
            return Ok(false);
        }
    }
    Ok(true)
}

#[inline(never)]
fn validate_const_enum_keywords<'a>(
    schema: &ValueRef<'a>,
    value: &ValueRef<'a>,
    path: &[PathPart<'a>],
    read: &ReadContext<'_>,
    issues: &mut Vec<InternalIssue<'a>>,
) -> Result<bool, ValueError> {
    let mut valid = true;
    if read.has(schema, &WireString::from("const"))? && !read.equal(value, &read.get(schema, "const")?)? {
        push_issue(issues, path, "must equal const value", "const");
        valid = false;
    }
    if read.is_array(&read.get(schema, "enum")?)? {
        let enums = read.get(schema, "enum")?;
        let candidate = (*value).clone();
        if !read.truthy(&read.predicate(
            &enums,
            "some",
            &ENUM_VALUE_CALLBACK,
            std::slice::from_ref(&candidate),
            |entry, read| read.equal(&entry, &candidate),
        )?)? {
            push_issue(issues, path, "must be one of the allowed enum values", "enum");
            valid = false;
        }
    }
    Ok(valid)
}

// Explicit depth-first continuations preserve Graph aliases and borrowed reads.
// No native call stack is retained while descending a child schema.
enum ValidationCursor<'a> {
    Wire { value: ValueRef<'a>, length: usize, next: usize },
    Graph(ara_rpc::value_heap::IteratorRecord),
}
impl<'a> ValidationCursor<'a> {
    fn new(value: ValueRef<'a>, read: &ReadContext<'_>) -> Result<Self, ValueError> {
        if let ValueRef::Graph(value) = &value {
            let ReadContext::Graph { factory, runtime } = *read else { unreachable!() };
            return Ok(Self::Graph(factory.heap().get_iterator(value, runtime)?));
        }
        let length = read.number(&read.get(&value, "length")?)?
            .ok_or_else(|| ValueError::TypeError("Value is not iterable".into()))? as usize;
        Ok(Self::Wire { value, length, next: 0 })
    }
    fn next(&mut self, read: &ReadContext<'_>) -> Result<Option<ValueRef<'a>>, ValueError> {
        match self {
            Self::Wire { value, length, next } => {
                if *next >= *length { return Ok(None); }
                let value = read.index(value, *next)?;
                *next += 1;
                Ok(Some(value))
            }
            Self::Graph(iterator) => {
                let ReadContext::Graph { factory, runtime } = *read else { unreachable!() };
                factory.heap().iterator_next(iterator, runtime).map(|value| value.map(ValueRef::Graph))
            }
        }
    }
    fn body_error(&mut self, original: ValueError, read: &ReadContext<'_>) -> ValueError {
        let Self::Graph(iterator) = self else { return original };
        let ReadContext::Graph { factory, runtime } = *read else { unreachable!() };
        match factory.heap().iterator_close(iterator, runtime) {
            Ok(()) => original,
            Err(close) if original.is_native_exception() && close.is_native_exception() => original,
            Err(close) => ValueError::IteratorClose { original: Box::new(original), close: Box::new(close) },
        }
    }
}
enum ReferenceLease { Pair(WireString, NodeIdentity), Primitive }
#[derive(Clone, Copy)]
enum ValidationPhase {
    Start, ReferenceChild, Keywords, KeywordNext, AllOfChild, UnionChild,
    Not, NotChild, If, IfChild, ConditionalChild, Warning,
    Object, Properties, PropertyChild, PropertyNames, PropertyNameNext, PropertyNameChild,
    Patterns, PatternNext, PatternValues, PatternChild,
    DependentRequired, DependentSchemas, DependentSchemaNext, DependentSchemaChild,
    Additional, AdditionalNext, AdditionalChild, ObjectBounds,
    Array, Prefix, PrefixChild, Suffix, SuffixNext, SuffixChild,
    Items, ItemChild, Contains, ContainsNext, ContainsChild, Primitive,
}
struct UnionWork<'a> {
    keyword: &'static str, matches: usize,
    first_issues: Option<Vec<InternalIssue<'a>>>,
    selected_issues: Option<Vec<InternalIssue<'a>>>,
    selected_count: usize, branch: Option<ValueRef<'a>>,
}
struct ObjectWork<'a> {
    properties: ValueRef<'a>, keys: Vec<WireString>, next: usize,
    known: HashSet<WireString>,
    patterns: ValueRef<'a>, pattern_keys: Vec<WireString>, next_pattern: usize,
    pattern: Option<Regex>, pattern_schema: ValueRef<'a>,
    dependencies: ValueRef<'a>, additional: ValueRef<'a>,
}
struct ArrayWork<'a> {
    prefix: ValueRef<'a>, items: ValueRef<'a>, limit: f64, index: usize,
    initial: ValueRef<'a>, suffix_index: f64,
    minimum: ValueRef<'a>, maximum: ValueRef<'a>, count: usize,
}
struct ValidationFrame<'a> {
    schema: ValueRef<'a>, value: ValueRef<'a>, path: Vec<PathPart<'a>>,
    valid: bool, issues: Vec<InternalIssue<'a>>, phase: ValidationPhase, keyword: usize,
    reference: Option<ReferenceLease>, cursor: Option<ValidationCursor<'a>>,
    // Only errors in a suspended iterator body close it, never next() errors.
    cursor_body: bool,
    union: Option<UnionWork<'a>>, object: Option<ObjectWork<'a>>, array: Option<ArrayWork<'a>>,
}
struct ValidationReturn<'a> { result: Result<bool, ValueError>, issues: Vec<InternalIssue<'a>> }
enum ValidationAction<'a> { Again, Child(ValueRef<'a>, ValueRef<'a>, Vec<PathPart<'a>>), Done }
impl<'a> ValidationFrame<'a> {
    fn new(schema: ValueRef<'a>, value: ValueRef<'a>, path: Vec<PathPart<'a>>) -> Self {
        Self { schema, value, path, valid: true, issues: Vec::new(), phase: ValidationPhase::Start,
            keyword: 0, reference: None, cursor: None, cursor_body: false,
            union: None, object: None, array: None }
    }
    fn release_reference(&mut self, context: &mut ValidationContext<'a, '_>) {
        match self.reference.take() {
            Some(ReferenceLease::Pair(reference, identity)) => { context.seen_pairs.remove(&(reference, identity)); }
            Some(ReferenceLease::Primitive) => { context.ref_depth -= 1; }
            None => {}
        }
    }
    fn unwind(&mut self, error: ValueError, read: &ReadContext<'_>, context: &mut ValidationContext<'a, '_>) -> ValueError {
        self.release_reference(context);
        if self.cursor_body {
            if let Some(cursor) = self.cursor.as_mut() { return cursor.body_error(error, read); }
        }
        error
    }
    fn child_result(&mut self, child: ValidationReturn<'a>) -> Result<bool, ValueError> {
        self.issues.extend(child.issues);
        child.result
    }
}
fn validate_node<'a>(
    schema: ValueRef<'a>, value: ValueRef<'a>, path: &[PathPart<'a>],
    read: &ReadContext<'_>, context: &mut ValidationContext<'a, '_>, issues: &mut Vec<InternalIssue<'a>>,
) -> Result<bool, ValueError> {
    let mut frames = vec![ValidationFrame::new(schema, value, path.to_vec())];
    let mut returned = None;
    while let Some(mut frame) = frames.pop() {
        match advance_validation(&mut frame, returned.take(), read, context) {
            Ok(ValidationAction::Again) => frames.push(frame),
            Ok(ValidationAction::Child(schema, value, path)) => {
                frames.push(frame);
                frames.push(ValidationFrame::new(schema, value, path));
            }
            Ok(ValidationAction::Done) => returned = Some(ValidationReturn { result: Ok(frame.valid), issues: frame.issues }),
            Err(error) => {
                let error = frame.unwind(error, read, context);
                returned = Some(ValidationReturn { result: Err(error), issues: frame.issues });
            }
        }
    }
    let returned = returned.expect("validation root completion");
    issues.extend(returned.issues);
    returned.result
}
fn advance_validation<'a>(
    frame: &mut ValidationFrame<'a>, child: Option<ValidationReturn<'a>>,
    read: &ReadContext<'_>, context: &mut ValidationContext<'a, '_>,
) -> Result<ValidationAction<'a>, ValueError> {
    use ValidationAction::{Again, Child, Done};
    use ValidationPhase as P;
    match frame.phase {
        P::Start => {
            if let Some(boolean) = read.boolean(&frame.schema)? {
                if !boolean { push_issue(&mut frame.issues, &frame.path, "must not match false schema", "false"); }
                frame.valid = boolean;
                return Ok(Done);
            }
            if !read.is_object(&frame.schema)? {
                push_issue(&mut frame.issues, &frame.path, "schema must be an object or boolean", "schema");
                frame.valid = false;
                return Ok(Done);
            }
            if let Some(reference) = read.string(&read.get(&frame.schema, "$ref")?)? {
                let resolved = resolve_local_ref(context.root.clone(), &reference, read)?;
                if read.is_undefined(&resolved)? {
                    push_issue(&mut frame.issues, &frame.path, interpolated("unresolved reference ", &reference, ""), "$ref");
                    frame.valid = false;
                    return Ok(Done);
                }
                if let Some(identity) = read.identity(&frame.value)? {
                    if !context.seen_pairs.insert((reference.clone(), identity)) { return Ok(Done); }
                    frame.reference = Some(ReferenceLease::Pair(reference, identity));
                } else {
                    if context.ref_depth >= MAX_REF_DEPTH {
                        push_issue(&mut frame.issues, &frame.path, "reference depth exceeded", "$ref");
                        frame.valid = false;
                        return Ok(Done);
                    }
                    context.ref_depth += 1;
                    frame.reference = Some(ReferenceLease::Primitive);
                }
                frame.phase = P::ReferenceChild;
                return Ok(Child(resolved, std::mem::replace(&mut frame.value, ValueRef::Undefined), frame.path.clone()));
            }
            if read.is_null(&frame.value)? && read.boolean(&read.get(&frame.schema, "nullable")?)? == Some(true) { return Ok(Done); }
            if !validate_type_keyword(&frame.schema, &frame.value, &frame.path, read, &mut frame.issues)? {
                frame.valid = false;
                return Ok(Done);
            }
            frame.valid = validate_const_enum_keywords(&frame.schema, &frame.value, &frame.path, read, &mut frame.issues)?;
            frame.phase = P::Keywords;
        }
        P::ReferenceChild => {
            frame.release_reference(context);
            frame.valid = frame.child_result(child.expect("reference result"))?;
            return Ok(Done);
        }
        P::Keywords => {
            const KEYWORDS: [&str; 3] = ["anyOf", "oneOf", "allOf"];
            if frame.keyword == KEYWORDS.len() { frame.phase = P::Not; return Ok(Again); }
            let keyword = KEYWORDS[frame.keyword]; frame.keyword += 1;
            let branches = read.get(&frame.schema, keyword)?;
            if !read.is_array(&branches)? { return Ok(Again); }
            frame.cursor = Some(ValidationCursor::new(branches, read)?);
            if keyword != "allOf" {
                frame.union = Some(UnionWork { keyword, matches: 0, first_issues: None, selected_issues: None, selected_count: 0, branch: None });
            }
            frame.phase = P::KeywordNext;
        }
        P::KeywordNext => {
            if let Some(branch) = frame.cursor.as_mut().expect("keyword cursor").next(read)? {
                frame.cursor_body = true;
                frame.phase = if let Some(union) = frame.union.as_mut() {
                    union.branch = Some(branch.clone()); P::UnionChild
                } else { P::AllOfChild };
                return Ok(Child(branch, frame.value.clone(), frame.path.clone()));
            }
            frame.cursor = None;
            if let Some(union) = frame.union.take() {
                let valid = if union.keyword == "anyOf" { union.matches > 0 } else { union.matches == 1 };
                if !valid {
                    if union.matches == 0 && union.selected_count == 1 && union.selected_issues.as_ref().is_some_and(|v| !v.is_empty()) {
                        frame.issues.extend(union.selected_issues.expect("selected branch"));
                    } else if union.matches == 0 && union.first_issues.as_ref().is_some_and(|v| !v.is_empty()) {
                        frame.issues.extend(union.first_issues.expect("first branch").into_iter().map(|mut issue| {
                            issue.from_union_branch = true; issue
                        }));
                    } else {
                        push_issue(&mut frame.issues, &frame.path,
                            if union.keyword == "anyOf" { "must match at least one schema" } else { "must match exactly one schema" }, union.keyword);
                    }
                    frame.valid = false;
                }
            }
            frame.phase = P::Keywords;
        }
        P::AllOfChild => {
            let valid = frame.child_result(child.expect("allOf result"))?;
            frame.valid = valid && frame.valid;
            frame.cursor_body = false; frame.phase = P::KeywordNext;
        }
        P::UnionChild => {
            let child = child.expect("union result");
            let valid = child.result?;
            let union = frame.union.as_mut().expect("union state");
            let branch = union.branch.take().expect("union branch");
            if valid { union.matches += 1; }
            else {
                if union.first_issues.is_none() { union.first_issues = Some(child.issues.clone()); }
                if is_tag_selected_branch(&branch, &frame.value, read)? {
                    union.selected_count += 1;
                    if union.selected_count == 1 { union.selected_issues = Some(child.issues); }
                }
            }
            frame.cursor_body = false; frame.phase = P::KeywordNext;
        }
        P::Not => {
            if read.has(&frame.schema, &WireString::from("not"))? {
                let schema = read.get(&frame.schema, "not")?;
                frame.phase = P::NotChild;
                return Ok(Child(schema, frame.value.clone(), frame.path.clone()));
            }
            frame.phase = P::If;
        }
        P::NotChild => {
            if child.expect("not result").result? {
                push_issue(&mut frame.issues, &frame.path, "must not match excluded schema", "not"); frame.valid = false;
            }
            frame.phase = P::If;
        }
        P::If => {
            if read.has(&frame.schema, &WireString::from("if"))? {
                let schema = read.get(&frame.schema, "if")?;
                frame.phase = P::IfChild;
                return Ok(Child(schema, frame.value.clone(), frame.path.clone()));
            }
            frame.phase = P::Warning;
        }
        P::IfChild => {
            let matches = child.expect("if result").result?;
            let branch = read.get(&frame.schema, if matches { "then" } else { "else" })?;
            if !read.is_undefined(&branch)? {
                frame.phase = P::ConditionalChild;
                return Ok(Child(branch, frame.value.clone(), frame.path.clone()));
            }
            frame.phase = P::Warning;
        }
        P::ConditionalChild => {
            let valid = frame.child_result(child.expect("conditional result"))?;
            frame.valid = valid && frame.valid; frame.phase = P::Warning;
        }
        P::Warning => {
            if (read.has(&frame.schema, &WireString::from("unevaluatedProperties"))?
                || read.has(&frame.schema, &WireString::from("unevaluatedItems"))?)
                && !context.warning_seen.swap(true, Ordering::Relaxed) { (context.warn)(UNEVALUATED_WARNING); }
            frame.phase = P::Object;
        }
        P::Object => {
            if !read.is_object(&frame.value)? { frame.phase = P::Array; return Ok(Again); }
            let properties = if read.is_object(&read.get(&frame.schema, "properties")?)? {
                read.get(&frame.schema, "properties")?
            } else { read.empty_object()? };
            if is_required_set(&read.get(&frame.schema, "required")?, read)? {
                let required = read.get(&frame.schema, "required")?;
                read.for_of(&required, |key| {
                    if !read.has_value_key(&frame.value, &key)? {
                        push_issue(&mut frame.issues, &actual_path(&frame.path, key), "is required", "required");
                        frame.valid = false;
                    }
                    Ok(LoopControl::Continue)
                })?;
            }
            let keys = read.for_in_keys(&properties)?;
            frame.object = Some(ObjectWork { properties, keys, next: 0, known: HashSet::new(),
                patterns: ValueRef::Undefined, pattern_keys: Vec::new(), next_pattern: 0,
                pattern: None, pattern_schema: ValueRef::Undefined,
                dependencies: ValueRef::Undefined, additional: ValueRef::Undefined });
            frame.phase = P::Properties;
        }
        P::Properties => {
            let work = frame.object.as_mut().expect("object state");
            while work.next < work.keys.len() {
                let key = work.keys[work.next].clone(); work.next += 1;
                if !read.has(&frame.value, &key)? { continue; }
                let schema = read.property(&work.properties, &key)?;
                let value = read.property(&frame.value, &key)?;
                frame.phase = P::PropertyChild;
                return Ok(Child(schema, value, property_path(&frame.path, &key)));
            }
            frame.phase = P::PropertyNames;
        }
        P::PropertyChild => {
            let valid = frame.child_result(child.expect("property result"))?;
            frame.valid = valid && frame.valid; frame.phase = P::Properties;
        }
        P::PropertyNames => {
            if !read.is_undefined(&read.get(&frame.schema, "propertyNames")?)? {
                let work = frame.object.as_mut().expect("object state");
                work.keys = read.own_keys(&frame.value)?; work.next = 0; frame.phase = P::PropertyNameNext;
            } else { frame.phase = P::Patterns; }
        }
        P::PropertyNameNext => {
            let work = frame.object.as_mut().expect("object state");
            if work.next < work.keys.len() {
                let key = work.keys[work.next].clone(); work.next += 1;
                let schema = read.get(&frame.schema, "propertyNames")?;
                frame.phase = P::PropertyNameChild;
                return Ok(Child(schema, ValueRef::String(key.clone()), property_path(&frame.path, &key)));
            }
            frame.phase = P::Patterns;
        }
        P::PropertyNameChild => {
            let valid = frame.child_result(child.expect("propertyNames result"))?;
            frame.valid = valid && frame.valid; frame.phase = P::PropertyNameNext;
        }
        P::Patterns => {
            let work = frame.object.as_mut().expect("object state");
            work.known = read.own_keys(&work.properties)?.into_iter().collect();
            if read.is_object(&read.get(&frame.schema, "patternProperties")?)? {
                work.patterns = read.get(&frame.schema, "patternProperties")?;
                work.pattern_keys = read.for_in_keys(&work.patterns)?; frame.phase = P::PatternNext;
            } else { frame.phase = P::DependentRequired; }
        }
        P::PatternNext => {
            let work = frame.object.as_mut().expect("object state");
            while work.next_pattern < work.pattern_keys.len() {
                let pattern = work.pattern_keys[work.next_pattern].clone(); work.next_pattern += 1;
                work.pattern_schema = read.property(&work.patterns, &pattern)?;
                let Ok(regex) = compile_pattern(&pattern) else {
                    push_issue(&mut frame.issues, &frame.path, interpolated("invalid patternProperties regex ", &pattern, ""), "patternProperties");
                    frame.valid = false; continue;
                };
                work.pattern = Some(regex); work.keys = read.for_in_keys(&frame.value)?; work.next = 0;
                frame.phase = P::PatternValues; return Ok(Again);
            }
            frame.phase = P::DependentRequired;
        }
        P::PatternValues => {
            let work = frame.object.as_mut().expect("object state");
            while work.next < work.keys.len() {
                let key = work.keys[work.next].clone(); work.next += 1;
                if work.pattern.as_ref().expect("pattern").find_from_ucs2(key.units(), 0).next().is_none() { continue; }
                work.known.insert(key.clone());
                let value = read.property(&frame.value, &key)?;
                frame.phase = P::PatternChild;
                return Ok(Child(work.pattern_schema.clone(), value, property_path(&frame.path, &key)));
            }
            work.pattern = None; frame.phase = P::PatternNext;
        }
        P::PatternChild => {
            let valid = frame.child_result(child.expect("pattern result"))?;
            frame.valid = valid && frame.valid; frame.phase = P::PatternValues;
        }
        P::DependentRequired => {
            if read.is_object(&read.get(&frame.schema, "dependentRequired")?)? {
                let dependencies = read.get(&frame.schema, "dependentRequired")?;
                for key in read.for_in_keys(&dependencies)? {
                    let deps = read.property(&dependencies, &key)?;
                    if !read.has(&frame.value, &key)? || !read.is_array(&deps)? { continue; }
                    read.for_of(&deps, |dependency| {
                        let Some(dependency) = read.string(&dependency)? else { return Ok(LoopControl::Continue) };
                        if !read.has(&frame.value, &dependency)? {
                            push_issue(&mut frame.issues, &property_path(&frame.path, &dependency),
                                interpolated("is required when \"", &key, "\" is present"), "dependentRequired");
                            frame.valid = false;
                        }
                        Ok(LoopControl::Continue)
                    })?;
                }
            }
            frame.phase = P::DependentSchemas;
        }
        P::DependentSchemas => {
            if read.is_object(&read.get(&frame.schema, "dependentSchemas")?)? {
                let work = frame.object.as_mut().expect("object state");
                work.dependencies = read.get(&frame.schema, "dependentSchemas")?;
                work.keys = read.for_in_keys(&work.dependencies)?; work.next = 0; frame.phase = P::DependentSchemaNext;
            } else { frame.phase = P::Additional; }
        }
        P::DependentSchemaNext => {
            let work = frame.object.as_mut().expect("object state");
            while work.next < work.keys.len() {
                let key = work.keys[work.next].clone(); work.next += 1;
                if !read.has(&frame.value, &key)? { continue; }
                let schema = read.property(&work.dependencies, &key)?;
                frame.phase = P::DependentSchemaChild;
                return Ok(Child(schema, frame.value.clone(), frame.path.clone()));
            }
            frame.phase = P::Additional;
        }
        P::DependentSchemaChild => {
            let valid = frame.child_result(child.expect("dependent schema result"))?;
            frame.valid = valid && frame.valid; frame.phase = P::DependentSchemaNext;
        }
        P::Additional => {
            let additional = read.get(&frame.schema, "additionalProperties")?;
            let work = frame.object.as_mut().expect("object state");
            if read.boolean(&additional)? == Some(false) {
                for key in read.own_keys(&frame.value)? {
                    if !work.known.contains(&key) {
                        push_issue(&mut frame.issues, &property_path(&frame.path, &key), "must not be present", "additionalProperties");
                        frame.valid = false;
                    }
                }
                frame.phase = P::ObjectBounds;
            } else if !read.is_undefined(&additional)? && read.boolean(&additional)? != Some(true) {
                work.additional = additional; work.keys = read.for_in_keys(&frame.value)?; work.next = 0; frame.phase = P::AdditionalNext;
            } else { frame.phase = P::ObjectBounds; }
        }
        P::AdditionalNext => {
            let work = frame.object.as_mut().expect("object state");
            while work.next < work.keys.len() {
                let key = work.keys[work.next].clone(); work.next += 1;
                if work.known.contains(&key) { continue; }
                let value = read.property(&frame.value, &key)?;
                frame.phase = P::AdditionalChild;
                return Ok(Child(work.additional.clone(), value, property_path(&frame.path, &key)));
            }
            frame.phase = P::ObjectBounds;
        }
        P::AdditionalChild => {
            let valid = frame.child_result(child.expect("additional property result"))?;
            frame.valid = valid && frame.valid; frame.phase = P::AdditionalNext;
        }
        P::ObjectBounds => {
            for (keyword, prefix, operator) in [("minProperties", "must have at least ", "<"), ("maxProperties", "must have at most ", ">")] {
                if read.number(&read.get(&frame.schema, keyword)?)?.is_some() {
                    let count = ValueRef::Number(read.own_keys(&frame.value)?.len() as f64);
                    if read.compare(&count, &read.get(&frame.schema, keyword)?, operator)? {
                        let bound = read.implicit_string(&read.get(&frame.schema, keyword)?)?;
                        push_issue(&mut frame.issues, &frame.path, interpolated(prefix, &bound, " properties"), keyword); frame.valid = false;
                    }
                }
            }
            frame.object = None; frame.phase = P::Array;
        }
        P::Array => {
            if !read.is_array(&frame.value)? { frame.phase = P::Primitive; return Ok(Again); }
            for (keyword, prefix, operator) in [("minItems", "must have at least ", "<"), ("maxItems", "must have at most ", ">")] {
                if read.number(&read.get(&frame.schema, keyword)?)?.is_some() {
                    let count = read.get(&frame.value, "length")?;
                    if read.compare(&count, &read.get(&frame.schema, keyword)?, operator)? {
                        let bound = read.implicit_string(&read.get(&frame.schema, keyword)?)?;
                        push_issue(&mut frame.issues, &frame.path, interpolated(prefix, &bound, " items"), keyword); frame.valid = false;
                    }
                }
            }
            if read.boolean(&read.get(&frame.schema, "uniqueItems")?)? == Some(true) {
                let mut i = 0_usize;
                while read.length_less(i, &frame.value)? {
                    let mut j = i.checked_add(1).ok_or(ValueError::UnsupportedOperation("Checker array index overflow"))?;
                    while read.length_less(j, &frame.value)? {
                        let left = read.index(&frame.value, i)?;
                        let right = read.index(&frame.value, j)?;
                        if read.equal(&left, &right)? {
                            push_issue(&mut frame.issues, &index_path(&frame.path, j), "must be unique", "uniqueItems"); frame.valid = false;
                        }
                        j = j.checked_add(1).ok_or(ValueError::UnsupportedOperation("Checker array index overflow"))?;
                    }
                    i = i.checked_add(1).ok_or(ValueError::UnsupportedOperation("Checker array index overflow"))?;
                }
            }
            let prefix = if read.is_array(&read.get(&frame.schema, "prefixItems")?)? { read.get(&frame.schema, "prefixItems")? } else { ValueRef::Undefined };
            let items = read.get(&frame.schema, "items")?;
            let mut work = ArrayWork { prefix, items, limit: 0.0, index: 0, initial: ValueRef::Undefined,
                suffix_index: 0.0, minimum: ValueRef::Undefined, maximum: ValueRef::Undefined, count: 0 };
            if read.is_array(&work.items)? {
                push_issue(&mut frame.issues, &frame.path, "array-valued items is not valid in JSON Schema 2020-12; use prefixItems", "items");
                frame.valid = false; frame.phase = P::Contains;
            } else if read.truthy(&work.prefix)? {
                let prefix_length = read.get(&work.prefix, "length")?;
                let value_length = read.get(&frame.value, "length")?;
                let prefix_length = read.numeric_value(&prefix_length)?;
                let value_length = read.numeric_value(&value_length)?;
                work.limit = if prefix_length.is_nan() || value_length.is_nan() { f64::NAN } else { prefix_length.min(value_length) };
                frame.phase = P::Prefix;
            } else if !read.is_undefined(&work.items)? { frame.phase = P::Items; }
            else { frame.phase = P::Contains; }
            frame.array = Some(work);
        }
        P::Prefix => {
            let work = frame.array.as_ref().expect("array state");
            if (work.index as f64) < work.limit {
                let schema = read.index(&work.prefix, work.index)?;
                let item = read.index(&frame.value, work.index)?;
                frame.phase = P::PrefixChild;
                return Ok(Child(schema, item, index_path(&frame.path, work.index)));
            }
            frame.phase = P::Suffix;
        }
        P::PrefixChild => {
            let valid = frame.child_result(child.expect("prefix result"))?; frame.valid = valid && frame.valid;
            let work = frame.array.as_mut().expect("array state");
            work.index = work.index.checked_add(1).ok_or(ValueError::UnsupportedOperation("Checker array index overflow"))?;
            frame.phase = P::Prefix;
        }
        P::Suffix => {
            let work = frame.array.as_mut().expect("array state");
            if !read.is_undefined(&work.items)? {
                work.initial = read.get(&work.prefix, "length")?;
                let Some(index) = read.number(&work.initial)? else {
                    return Err(ValueError::UnsupportedOperation("Native tuple suffix exotic initial index needs default-addition semantics"));
                };
                work.suffix_index = index; frame.phase = P::SuffixNext;
            } else { frame.phase = P::Contains; }
        }
        P::SuffixNext => {
            let work = frame.array.as_ref().expect("array state");
            if read.compare(&ValueRef::Number(work.suffix_index), &read.get(&frame.value, "length")?, "<")? {
                let item = read.property(&frame.value, &WireString::from(number_message(work.suffix_index)))?;
                let index = if let ValueRef::Graph(_) = &work.initial {
                    if work.suffix_index.to_bits() == read.number(&work.initial)?.expect("initial numeric").to_bits() { work.initial.clone() }
                    else { ValueRef::Number(work.suffix_index) }
                } else { ValueRef::Number(work.suffix_index) };
                frame.phase = P::SuffixChild;
                return Ok(Child(work.items.clone(), item, actual_path(&frame.path, index)));
            }
            frame.phase = P::Contains;
        }
        P::SuffixChild => {
            let valid = frame.child_result(child.expect("suffix result"))?; frame.valid = valid && frame.valid;
            let work = frame.array.as_mut().expect("array state"); let next = work.suffix_index + 1.0;
            if next == work.suffix_index { return Err(ValueError::UnsupportedOperation("Native tuple suffix index no longer advances")); }
            work.suffix_index = next; frame.phase = P::SuffixNext;
        }
        P::Items => {
            let work = frame.array.as_ref().expect("array state");
            if read.length_less(work.index, &frame.value)? {
                let item = read.index(&frame.value, work.index)?;
                frame.phase = P::ItemChild;
                return Ok(Child(work.items.clone(), item, index_path(&frame.path, work.index)));
            }
            frame.phase = P::Contains;
        }
        P::ItemChild => {
            let valid = frame.child_result(child.expect("item result"))?; frame.valid = valid && frame.valid;
            let work = frame.array.as_mut().expect("array state");
            work.index = work.index.checked_add(1).ok_or(ValueError::UnsupportedOperation("Checker array index overflow"))?;
            frame.phase = P::Items;
        }
        P::Contains => {
            if !read.is_undefined(&read.get(&frame.schema, "contains")?)? {
                let work = frame.array.as_mut().expect("array state");
                work.minimum = if read.number(&read.get(&frame.schema, "minContains")?)?.is_some() { read.get(&frame.schema, "minContains")? } else { ValueRef::Number(1.0) };
                work.maximum = if read.number(&read.get(&frame.schema, "maxContains")?)?.is_some() { read.get(&frame.schema, "maxContains")? } else { ValueRef::Number(f64::INFINITY) };
                work.count = 0; work.index = 0; frame.phase = P::ContainsNext;
            } else { frame.array = None; frame.phase = P::Primitive; }
        }
        P::ContainsNext => {
            let work = frame.array.as_ref().expect("array state");
            if read.length_less(work.index, &frame.value)? {
                let contains = read.get(&frame.schema, "contains")?;
                let item = read.index(&frame.value, work.index)?;
                frame.phase = P::ContainsChild;
                return Ok(Child(contains, item, index_path(&frame.path, work.index)));
            }
            if read.compare(&ValueRef::Number(work.count as f64), &work.minimum, "<")? {
                push_issue(&mut frame.issues, &frame.path, interpolated("must contain at least ", &read.implicit_string(&work.minimum)?, " matching item(s)"), "contains"); frame.valid = false;
            }
            if read.compare(&ValueRef::Number(work.count as f64), &work.maximum, ">")? {
                push_issue(&mut frame.issues, &frame.path, interpolated("must contain at most ", &read.implicit_string(&work.maximum)?, " matching item(s)"), "maxContains"); frame.valid = false;
            }
            frame.array = None; frame.phase = P::Primitive;
        }
        P::ContainsChild => {
            let valid = child.expect("contains result").result?;
            let work = frame.array.as_mut().expect("array state");
            if valid { work.count += 1; }
            work.index = work.index.checked_add(1).ok_or(ValueError::UnsupportedOperation("Checker array index overflow"))?;
            frame.phase = P::ContainsNext;
        }
        P::Primitive => {
            if let Some(string) = read.string(&frame.value)? {
                frame.valid = validate_string(&frame.schema, &string, &frame.path, read, &mut frame.issues)? && frame.valid;
            }
            if let Some(number) = read.number(&frame.value)?.filter(|v| v.is_finite()) {
                frame.valid = validate_number(&frame.schema, number, &frame.path, read, &mut frame.issues)? && frame.valid;
            }
            return Ok(Done);
        }
    }
    Ok(Again)
}


fn compile_pattern(pattern: &WireString) -> Result<Regex, regress::Error> {
    Regex::from_unicode(pattern.units().iter().copied().map(u32::from), Flags::default())
}

fn validate_string<'a>(
    schema: &ValueRef<'a>,
    value: &WireString,
    path: &[PathPart<'a>],
    read: &ReadContext<'_>,
    issues: &mut Vec<InternalIssue<'a>>,
) -> Result<bool, ValueError> {
    let mut valid = true;
    for (keyword, prefix, operator) in [("minLength", "must be at least ", "<"), ("maxLength", "must be at most ", ">")]
    {
        if read.number(&read.get(schema, keyword)?)?.is_some()
            && read.compare(&ValueRef::Number(value.len() as f64), &read.get(schema, keyword)?, operator)?
        {
            push_issue(
                issues,
                path,
                interpolated(prefix, &read.implicit_string(&read.get(schema, keyword)?)?, " characters"),
                keyword,
            );
            valid = false;
        }
    }
    if read.string(&read.get(schema, "pattern")?)?.is_some() {
        // The second Get and RegExp conversion are inside Native's try.
        // Host/carrier errors never become a successful schema-pattern issue.
        let outcome = (|| {
            let pattern = read.get(schema, "pattern")?;
            let source =
                if read.is_undefined(&pattern)? { WireString::from("") } else { read.implicit_string(&pattern)? };
            let regex =
                compile_pattern(&source).map_err(|_| ValueError::TypeError("Invalid regular expression".into()))?;
            Ok::<_, ValueError>(regex.find_from_ucs2(value.units(), 0).next().is_some())
        })();
        match outcome {
            Ok(true) => {}
            Ok(false) => {
                push_issue(issues, path, "must match pattern", "pattern");
                valid = false;
            }
            Err(error) if error.is_native_exception() => {
                push_issue(issues, path, "schema pattern is invalid", "pattern");
                valid = false;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(valid)
}

fn validate_number<'a>(
    schema: &ValueRef<'a>,
    value: f64,
    path: &[PathPart<'a>],
    read: &ReadContext<'_>,
    issues: &mut Vec<InternalIssue<'a>>,
) -> Result<bool, ValueError> {
    let mut valid = true;
    for (keyword, rejects, message_operator) in [
        ("minimum", "<", ">="),
        ("maximum", ">", "<="),
        ("exclusiveMinimum", "<=", ">"),
        ("exclusiveMaximum", ">=", "<"),
    ] {
        if read.number(&read.get(schema, keyword)?)?.is_some()
            && read.compare(&ValueRef::Number(value), &read.get(schema, keyword)?, rejects)?
        {
            push_issue(
                issues,
                path,
                interpolated(
                    &format!("must be {message_operator} "),
                    &read.implicit_string(&read.get(schema, keyword)?)?,
                    "",
                ),
                keyword,
            );
            valid = false;
        }
    }
    for (keyword, bound_keyword, rejects, message_operator) in
        [("exclusiveMinimum", "minimum", "<=", ">"), ("exclusiveMaximum", "maximum", ">=", "<")]
    {
        if read.boolean(&read.get(schema, keyword)?)? == Some(true)
            && read.number(&read.get(schema, bound_keyword)?)?.is_some()
            && read.compare(&ValueRef::Number(value), &read.get(schema, bound_keyword)?, rejects)?
        {
            push_issue(
                issues,
                path,
                interpolated(
                    &format!("must be {message_operator} "),
                    &read.implicit_string(&read.get(schema, bound_keyword)?)?,
                    "",
                ),
                keyword,
            );
            valid = false;
        }
    }
    if read.number(&read.get(schema, "multipleOf")?)?.is_some()
        && read.compare(&read.get(schema, "multipleOf")?, &ValueRef::Number(0.0), ">")?
    {
        let quotient = value / read.numeric_value(&read.get(schema, "multipleOf")?)?;
        let rounded = if quotient.fract() == -0.5 { quotient.ceil() } else { quotient.round() };
        if (quotient - rounded).abs() > f64::EPSILON * 10.0 {
            push_issue(
                issues,
                path,
                interpolated("must be a multiple of ", &read.implicit_string(&read.get(schema, "multipleOf")?)?, ""),
                "multipleOf",
            );
            valid = false;
        }
    }
    Ok(valid)
}

#[cfg(test)]
mod tests;
