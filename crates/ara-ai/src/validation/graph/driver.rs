//! Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/ai/src/utils/validation.ts:1679-1731,1899-1905,1930-2138.
//! packages/ai/src/error/validation.ts:18-24 (ToolNotFoundError interpolation).
//! Actual JSON/authored-Ark context tool validation driver.
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

use super::{
    GraphContextValidation, GraphNormalization, Normalizer, coercions, schema_normalizers, shapes, spill_preview,
};
use crate::graph_values::{GraphExecution, GraphValueFactory};
use crate::schema_validator::JsonSchemaValidationResult;
use crate::validation::context::{
    GraphValidationContext, get_graph_validation_context, graph_context_normalization_schema, validate_graph_context,
};
use ara_rpc::WireString;
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, JsonSpace, StringifyMode, ValueError, ValueHandle, ValueHeap,
    ValueKind, ValueRuntime, ValueSnapshot,
};

static TOOL_NAME_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:validation.ts:1900",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: tool_name_callback,
};
fn tool_name_callback(
    heap: &std::sync::Arc<ValueHeap>,
    captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let tool = args.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())?;
    let left = heap.get(&tool, &"name".into(), runtime)?;
    let right = heap.get(&captures[0], &"name".into(), runtime)?;
    let equal = match (heap.snapshot(&left)?, heap.snapshot(&right)?) {
        (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => left == right,
        _ => heap.object_is(&left, &right)?,
    };
    heap.boolean(equal)
}
static FLATTEN_ISSUE_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:validation.ts:1684",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: flatten_issue_callback,
};
fn flatten_issue_callback(
    heap: &std::sync::Arc<ValueHeap>,
    _captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let read = Formatting { execution: GraphExecution::new(heap, runtime) };
    let issue = match args.first() {
        Some(value) => value.clone(),
        None => heap.undefined()?,
    };
    let union = heap.get(&issue, &"fromUnionBranch".into(), runtime)?;
    let union = matches!(heap.snapshot(&union)?, ValueSnapshot::Boolean(true));
    let keyword = heap.get(&issue, &"keyword".into(), runtime)?;
    let additional = read.text(&keyword, "additionalProperties")?;
    let out = heap.object()?;
    let keyword = if additional {
        "unrecognized"
    } else {
        let keyword = heap.get(&issue, &"keyword".into(), runtime)?;
        if read.text(&keyword, "type")? { "type" } else { "other" }
    };
    read.own(&out, "keyword", heap.string(keyword.into())?, runtime)?;
    let path = heap.get(&issue, &"path".into(), runtime)?;
    let path = coercions::path_to_graph_pointer_with_execution(read.execution, &path)?;
    read.own(&out, "instancePath", path, runtime)?;
    let expected = if additional {
        heap.array(vec![])?
    } else {
        let expected = heap.get(&issue, &"expectedTypes".into(), runtime)?;
        if matches!(heap.kind(&expected)?, ValueKind::Null | ValueKind::Undefined) {
            heap.array(vec![])?
        } else {
            expected
        }
    };
    read.own(&out, "expectedTypes", expected, runtime)?;
    read.own(&out, "unionBranch", heap.boolean(union)?, runtime)?;
    Ok(out)
}
static ARK_MESSAGE_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:validation.ts:1722",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: ark_message_callback,
};
static JSON_MESSAGE_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:validation.ts:1731",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: json_message_callback,
};
fn ark_message_callback(
    heap: &std::sync::Arc<ValueHeap>,
    _captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    format_issue(GraphExecution::new(heap, runtime), args)
}
fn json_message_callback(
    heap: &std::sync::Arc<ValueHeap>,
    _captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    format_issue(GraphExecution::new(heap, runtime), args)
}
fn format_issue(execution: GraphExecution<'_>, args: &[ValueHandle]) -> Result<ValueHandle, ValueError> {
    let heap = execution.heap();
    let runtime = execution.runtime();
    let read = Formatting { execution };
    let issue = match args.first() {
        Some(value) => value.clone(),
        None => heap.undefined()?,
    };
    let path = heap.get(&issue, &"path".into(), runtime)?;
    let path = coercions::format_graph_issue_path_with_execution(read.execution, &path)?;
    let mut units = WireString::from("  - ").units().to_vec();
    units.extend(read.implicit(&path, runtime)?.units());
    Driver::append(&mut units, ": ");
    let message = heap.get(&issue, &"message".into(), runtime)?;
    units.extend(read.implicit(&message, runtime)?.units());
    heap.string(WireString::from_units(units))
}
fn graph_messages_with_execution(
    execution: GraphExecution<'_>,
    issues: &ValueHandle,
    spec: &'static CompiledFunctionSpec,
) -> Result<ValueHandle, ValueError> {
    let heap = execution.heap();
    let map = heap.get(issues, &"map".into(), execution.runtime())?;
    let callback = heap.fresh_compiled_function(spec, &[])?;
    heap.call(&map, issues, &[callback], execution.runtime())
}
struct Formatting<'a> {
    execution: GraphExecution<'a>,
}
impl Formatting<'_> {
    fn implicit(&self, value: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<WireString, ValueError> {
        if self.execution.heap().kind(value)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into()));
        }
        self.execution.heap().to_string(value, runtime)
    }
    fn text(&self, value: &ValueHandle, text: &str) -> Result<bool, ValueError> {
        Ok(matches!(self.execution.heap().snapshot(value)?,ValueSnapshot::String(value) if value.equals_ascii(text)))
    }
    fn own(
        &self,
        receiver: &ValueHandle,
        name: &str,
        value: ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> Result<(), ValueError> {
        self.execution.heap().define_own_data(receiver, name.into(), value, runtime)
    }
}

#[derive(Debug)]
pub enum GraphToolValidationError {
    Runtime(ValueError),
    ToolNotFound { name: ValueHandle, message: WireString },
    Validation(WireString),
}
impl From<ValueError> for GraphToolValidationError {
    fn from(error: ValueError) -> Self {
        Self::Runtime(error)
    }
}

pub fn validate_graph_tool_call(
    factory: &GraphValueFactory,
    tools: &ValueHandle,
    tool_call: &ValueHandle,
) -> Result<ValueHandle, GraphToolValidationError> {
    let heap = factory.heap();
    heap.adopt(tools)?;
    heap.adopt(tool_call)?;
    let find = heap.get(tools, &"find".into(), factory.runtime())?;
    let predicate = heap.fresh_compiled_function(&TOOL_NAME_CALLBACK, std::slice::from_ref(tool_call))?;
    let tool = heap.call(&find, tools, &[predicate], factory.runtime())?;
    if !heap.truthy(&tool)? {
        let name = heap.get(tool_call, &"name".into(), factory.runtime())?;
        let mut units = WireString::from("Tool \"").units().to_vec();
        units.extend(Driver { factory }.implicit(&name, factory.runtime())?.units());
        units.extend(WireString::from("\" not found").units());
        return Err(GraphToolValidationError::ToolNotFound { name, message: WireString::from_units(units) });
    }
    validate_graph_tool_arguments(factory, &tool, tool_call)
}

pub fn validate_graph_tool_arguments(
    factory: &GraphValueFactory,
    tool: &ValueHandle,
    tool_call: &ValueHandle,
) -> Result<ValueHandle, GraphToolValidationError> {
    factory.heap().adopt(tool)?;
    factory.heap().adopt(tool_call)?;
    Driver { factory }.arguments(tool, tool_call)
}

pub(in crate::validation) fn flatten_graph_json_issues(
    factory: &GraphValueFactory,
    result: JsonSchemaValidationResult<ValueHandle, ValueHandle>,
) -> Result<ValueHandle, ValueError> {
    let driver = Driver { factory };
    let issues = driver.rich_issues(result)?;
    driver.flatten(&issues)
}

pub(in crate::validation) fn graph_validation_messages(
    factory: &GraphValueFactory,
    issues: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    graph_messages_with_execution(GraphExecution::from_factory(factory), issues, &ARK_MESSAGE_CALLBACK)
}

enum Outcome {
    Accept(ValueHandle),
    Reject { flat_issues: ValueHandle, messages: ValueHandle },
}
struct Rounds {
    args: ValueHandle,
    result: Outcome,
    changed: bool,
}
struct Driver<'a> {
    factory: &'a GraphValueFactory,
}
impl Driver<'_> {
    fn get(&self, receiver: &ValueHandle, name: &str) -> Result<ValueHandle, ValueError> {
        self.factory.heap().get(receiver, &name.into(), self.factory.runtime())
    }
    fn implicit(&self, value: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<WireString, ValueError> {
        Formatting { execution: GraphExecution::new(self.factory.heap(), runtime) }.implicit(value, runtime)
    }
    fn own(
        &self,
        receiver: &ValueHandle,
        name: &str,
        value: ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> Result<(), ValueError> {
        self.factory.heap().define_own_data(receiver, name.into(), value, runtime)
    }
    fn append(units: &mut Vec<u16>, text: &str) {
        units.extend(WireString::from(text).units());
    }
    fn take(next: &mut ValueHandle, changed: &mut bool, result: GraphNormalization) {
        if result.changed {
            *next = result.value;
            *changed = true;
        }
    }
    fn raw_preview(&self, value: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let length = heap.to_number(&self.get(value, "length")?, runtime)?;
        if length <= 512.0 {
            return Ok(value.clone());
        }
        let slice = self.get(value, "slice")?;
        let prefix = heap.call(&slice, value, &[heap.number(0.0)?, heap.number(512.0)?], runtime)?;
        let mut units = self.implicit(&prefix, runtime)?.units().to_vec();
        Self::append(&mut units, "… [truncated ");
        let excess = heap.to_number(&self.get(value, "length")?, runtime)? - 512.0;
        units.extend(heap.to_string(&heap.number(excess)?, runtime)?.units());
        Self::append(&mut units, " chars]");
        heap.string(WireString::from_units(units))
    }
    fn parse_error(&self, original: &ValueHandle, tool_call: &ValueHandle) -> Result<Option<WireString>, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        if !heap.truthy(original)?
            || !matches!(heap.kind(original)?, ValueKind::Object | ValueKind::Array)
            || !heap.has(original, &"__parseError".into(), runtime)?
        {
            return Ok(None);
        }
        let parse_error = self.get(original, "__parseError")?;
        let raw = self.get(original, "__rawJson")?;
        let raw = if matches!(heap.kind(&raw)?, ValueKind::Undefined | ValueKind::Null) {
            heap.string("".into())?
        } else {
            raw
        };
        // Explicit String(Symbol) is valid; subsequent template interpolation
        // uses implicit ToString and remains distinct.
        let raw = heap.string(heap.to_string(&raw, runtime)?)?;
        let raw = self.raw_preview(&raw)?;
        let mut units = WireString::from("Validation failed for tool \"").units().to_vec();
        units.extend(self.implicit(&self.get(tool_call, "name")?, runtime)?.units());
        Self::append(&mut units, "\": Tool call arguments are not valid JSON.\nParse Error: ");
        units.extend(self.implicit(&parse_error, runtime)?.units());
        Self::append(&mut units, "\nRaw JSON:\n");
        units.extend(self.implicit(&raw, runtime)?.units());
        Ok(Some(WireString::from_units(units)))
    }
    fn rich_issues(
        &self,
        result: JsonSchemaValidationResult<ValueHandle, ValueHandle>,
    ) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        // Adapter only: the shared checker owns its internal issue records.
        // Recreate actual records/paths before any source map/Get/Call.
        let mut records = Vec::with_capacity(result.issues.len());
        for issue in result.issues {
            let record = heap.object()?;
            self.own(&record, "path", heap.array(issue.path)?, runtime)?;
            self.own(&record, "message", heap.string(issue.message)?, runtime)?;
            if let Some(keyword) = issue.keyword {
                self.own(&record, "keyword", heap.string(keyword.into())?, runtime)?;
            }
            if let Some(expected) = issue.expected_types {
                self.own(&record, "expectedTypes", expected, runtime)?;
            }
            // Fixed pushIssue({...options}) has no own false branch marker.
            if issue.from_union_branch {
                self.own(&record, "fromUnionBranch", heap.boolean(true)?, runtime)?;
            }
            records.push(record);
        }
        heap.array(records)
    }
    fn flatten(&self, issues: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let map = self.get(issues, "map")?;
        let callback = heap.fresh_compiled_function(&FLATTEN_ISSUE_CALLBACK, &[])?;
        heap.call(&map, issues, &[callback], self.factory.runtime())
    }
    fn messages(&self, issues: &ValueHandle) -> Result<ValueHandle, ValueError> {
        graph_messages_with_execution(GraphExecution::from_factory(self.factory), issues, &JSON_MESSAGE_CALLBACK)
    }
    fn context(&self, context: &GraphValidationContext, value: &ValueHandle) -> Result<Outcome, ValueError> {
        match validate_graph_context(self.factory, context, value)? {
            GraphContextValidation::Accept(value) => Ok(Outcome::Accept(value)),
            GraphContextValidation::Reject(result) => {
                let issues = self.rich_issues(result)?;
                let flat_issues = self.flatten(&issues)?;
                let messages = self.messages(&issues)?;
                Ok(Outcome::Reject { flat_issues, messages })
            }
            GraphContextValidation::RejectActual { flat_issues, messages } => {
                Ok(Outcome::Reject { flat_issues, messages })
            }
        }
    }
    fn normalization(
        &self,
        schema: &ValueHandle,
        value: &ValueHandle,
        flatten: bool,
    ) -> Result<GraphNormalization, ValueError> {
        let factory = self.factory;
        let mut next = value.clone();
        let mut changed = false;
        let result = shapes::normalize_graph_double_encoded_keys(factory, &next)?;
        Self::take(&mut next, &mut changed, result);
        if flatten {
            let result = shapes::normalize_graph_flattened_array_properties(factory, &next)?;
            Self::take(&mut next, &mut changed, result);
        }
        let result = Normalizer { factory }.normalize(schema, &next, true)?;
        Self::take(&mut next, &mut changed, result);
        let result = schema_normalizers::normalize_graph_enum_string_whitespace(factory, schema, &next)?;
        Self::take(&mut next, &mut changed, result);
        let result = schema_normalizers::normalize_graph_identifier_string_whitespace(factory, &next)?;
        Self::take(&mut next, &mut changed, result);
        let result = schema_normalizers::normalize_graph_string_encoded_array_unions(factory, schema, &next)?;
        Self::take(&mut next, &mut changed, result);
        let result = schema_normalizers::normalize_graph_identifier_string_whitespace(factory, &next)?;
        Self::take(&mut next, &mut changed, result);
        let result = shapes::normalize_graph_single_string_field(factory, schema, &next)?;
        Self::take(&mut next, &mut changed, result);
        Ok(GraphNormalization { value: next, changed })
    }
    fn rounds(
        &self,
        context: &GraphValidationContext,
        args: &ValueHandle,
        initial: Outcome,
    ) -> Result<Rounds, ValueError> {
        // A fresh live ctx.json Get occurs even if the first round stops.
        let schema = graph_context_normalization_schema(self.factory, context)?;
        let mut args = args.clone();
        let mut result = initial;
        let mut changed = false;
        for _ in 0..5 {
            let Outcome::Reject { flat_issues, .. } = &result else {
                break;
            };
            let coercion = coercions::coerce_graph_args_from_issues(self.factory, &args, flat_issues)?;
            if !coercion.changed {
                break;
            }
            args = coercion.value;
            changed = true;
            let normal = self.normalization(&schema, &args, false)?;
            if normal.changed {
                args = normal.value;
            }
            result = self.context(context, &args)?;
        }
        Ok(Rounds { args, result, changed })
    }
    fn arguments(&self, tool: &ValueHandle, tool_call: &ValueHandle) -> Result<ValueHandle, GraphToolValidationError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let original = self.get(tool_call, "arguments")?;
        if let Some(message) = self.parse_error(&original, tool_call)? {
            return Err(GraphToolValidationError::Validation(message));
        }
        let parameters = self.get(tool, "parameters")?;
        let context = get_graph_validation_context(self.factory, &parameters)?;
        let schema = graph_context_normalization_schema(self.factory, &context)?;
        let normal = self.normalization(&schema, &original, true)?;
        let mut args = if normal.changed { normal.value } else { original.clone() };
        let mut changed = normal.changed;
        let initial = self.context(&context, &args)?;
        if let Outcome::Accept(value) = initial {
            return Ok(value);
        }
        let outcome = self.rounds(&context, &args, initial)?;
        args = outcome.args;
        changed |= outcome.changed;
        let mut result = outcome.result;
        if let Outcome::Accept(value) = result {
            return Ok(value);
        }
        let healed = spill_preview::heal_graph_inband_arg_spill(self.factory, &args)?;
        if healed.changed {
            args = healed.value;
            changed = true;
            result = self.context(&context, &args)?;
            if matches!(result, Outcome::Reject { .. }) {
                let outcome = self.rounds(&context, &args, result)?;
                args = outcome.args;
                result = outcome.result;
            }
            if let Outcome::Accept(value) = result {
                return Ok(value);
            }
        }
        // Fixed OMP Task is strict=false and lenientArgValidation=true. A wire
        // shape failure must reach its original runtime shape guard. Preserve
        // the original graph value; exceptions/infrastructure errors above
        // still propagate, and undeclared tools retain strict validation.
        if heap.truthy(&self.get(tool, "lenientArgValidation")?)? {
            return Ok(original);
        }
        let Outcome::Reject { messages, .. } = result else { unreachable!("accept already returned") };
        let join = self.get(&messages, "join")?;
        let errors = heap.call(&join, &messages, &[heap.string("\n".into())?], runtime)?;
        let errors = if heap.truthy(&errors)? { errors } else { heap.string("Unknown validation error".into())? };
        let received = if changed {
            let received = heap.object()?;
            let original = spill_preview::truncate_graph_args_for_error(self.factory, &original)?;
            self.own(&received, "original", original, runtime)?;
            let normalized = spill_preview::truncate_graph_args_for_error(self.factory, &args)?;
            self.own(&received, "normalized", normalized, runtime)?;
            received
        } else {
            spill_preview::truncate_graph_args_for_error(self.factory, &original)?
        };
        let mut units = WireString::from("Validation failed for tool \"").units().to_vec();
        units.extend(self.implicit(&self.get(tool_call, "name")?, runtime)?.units());
        Self::append(&mut units, "\":\n");
        units.extend(self.implicit(&errors, runtime)?.units());
        Self::append(&mut units, "\n\nReceived arguments:\n");
        match heap.stringify(&received, StringifyMode::Plain, Some(&JsonSpace::Number(2.0)), runtime)? {
            Some(json) => units.extend(json.units()),
            None => Self::append(&mut units, "undefined"),
        }
        Err(GraphToolValidationError::Validation(WireString::from_units(units)))
    }
}
#[cfg(test)]
pub(super) fn observe_graph_driver_family() {
    observations::run();
    observe_retained_driver();
}

#[cfg(test)]
mod observations {
    use super::*;
    use crate::schema_validator::JsonSchemaValidationIssue;
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueHeap,
    };
    use std::sync::{Arc, Mutex};

    fn own(heap: &Arc<ValueHeap>, target: &ValueHandle, key: &str, value: ValueHandle) {
        heap.define_own_data(target, key.into(), value, &NoRuntime).unwrap();
    }
    fn get(heap: &Arc<ValueHeap>, target: &ValueHandle, key: &str) -> ValueHandle {
        heap.get(target, &key.into(), &NoRuntime).unwrap()
    }
    fn parse(heap: &Arc<ValueHeap>, value: &str) -> ValueHandle {
        heap.parse_strict_units(&value.into()).unwrap()
    }
    fn make_tool(heap: &Arc<ValueHeap>, name: &str, schema: ValueHandle) -> ValueHandle {
        let tool = heap.object().unwrap();
        own(heap, &tool, "name", heap.string(name.into()).unwrap());
        own(heap, &tool, "parameters", schema);
        tool
    }
    fn call(heap: &Arc<ValueHeap>, name: &str, args: ValueHandle) -> ValueHandle {
        let call = heap.object().unwrap();
        own(heap, &call, "name", heap.string(name.into()).unwrap());
        own(heap, &call, "arguments", args);
        call
    }
    fn contains(text: &WireString, needle: &str) -> bool {
        let needle = WireString::from(needle);
        text.units().windows(needle.units().len()).any(|part| part == needle.units())
    }
    fn accessor(heap: &Arc<ValueHeap>, target: &ValueHandle, key: &str, id: u64) {
        heap.define_own(
            target,
            key.into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(id))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            &NoRuntime,
        )
        .unwrap();
    }
    struct Runtime {
        schema: ValueHandle,
        args: ValueHandle,
        name: ValueHandle,
        thrown: ValueHandle,
        captured: Mutex<Option<ValueHandle>>,
        events: Mutex<Vec<&'static str>>,
    }
    impl ValueRuntime for Runtime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let RuntimeRequest::Call { capability, .. } = request else {
                return Err(ValueError::MissingCapability(request.capability()));
            };
            let value = match capability.0 {
                1 => {
                    self.events.lock().unwrap().push("tool-name");
                    self.name.clone()
                }
                2 => {
                    self.events.lock().unwrap().push("call-name");
                    self.name.clone()
                }
                3 => {
                    self.events.lock().unwrap().push("second-tool");
                    return Err(ValueError::MissingCapability(CapabilityId(333)));
                }
                4 => {
                    self.events.lock().unwrap().push("arguments");
                    self.args.clone()
                }
                5 => {
                    self.events.lock().unwrap().push("parameters");
                    self.schema.clone()
                }
                6 => {
                    self.events.lock().unwrap().push("unexpected parameters");
                    return Err(ValueError::MissingCapability(CapabilityId(666)));
                }
                7 => return Err(ValueError::Thrown(self.thrown.clone())),
                8 => return Err(ValueError::MissingCapability(CapabilityId(404))),
                9 => {
                    self.events.lock().unwrap().push("map");
                    self.captured.lock().unwrap().clone().unwrap()
                }
                10 => {
                    self.events.lock().unwrap().push("missing-name-to-string");
                    self.name.clone()
                }
                11 => {
                    self.events.lock().unwrap().push("throwing-name-to-string");
                    return Err(ValueError::Thrown(self.thrown.clone()));
                }
                12 => {
                    self.events.lock().unwrap().push("missing-call-name");
                    self.captured.lock().unwrap().clone().unwrap()
                }
                _ => return Err(ValueError::MissingCapability(capability)),
            };
            Ok(RuntimeReply::Value(value))
        }
    }
    fn binding(heap: &Arc<ValueHeap>, schema: ValueHandle, args: ValueHandle) -> (GraphValueFactory, Arc<Runtime>) {
        let runtime = Arc::new(Runtime {
            schema,
            args,
            name: heap.string("chosen".into()).unwrap(),
            thrown: heap.symbol(Some("original throw".into())).unwrap(),
            captured: Mutex::new(None),
            events: Mutex::new(Vec::new()),
        });
        (GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap(), runtime)
    }

    #[test]
    fn declared_lenient_failure_preserves_exact_args_but_not_runtime_errors() {
        let heap=ValueHeap::new().unwrap();
        let factory=GraphValueFactory::new(heap.clone(),Arc::new(NoRuntime)).unwrap();
        // Flat Task requires both strings; the single-string repair must not turn context into task.
        let schema=parse(&heap,r#"{"type":"object","properties":{"task":{"type":"string"},"solutionSpace":{"type":"string"}},"required":["task","solutionSpace"]}"#);
        let tool=make_tool(&heap,"task",schema);
        let original=parse(&heap,r#"{"context":"batch while disabled","tasks":[{"task":"work"}]}"#);
        let request=call(&heap,"task",original.clone());
        assert!(matches!(validate_graph_tool_arguments(&factory,&tool,&request),Err(GraphToolValidationError::Validation(_))));
        own(&heap,&tool,"lenientArgValidation",heap.boolean(true).unwrap());
        let forwarded=validate_graph_tool_arguments(&factory,&tool,&request).unwrap();
        assert!(forwarded.same_identity(&original));
        assert!(get(&heap,&request,"arguments").same_identity(&original));
        // A missing real callback is an infrastructure failure, not lenience.
        accessor(&heap,&tool,"lenientArgValidation",991);
        assert!(matches!(validate_graph_tool_arguments(&factory,&tool,&request),Err(GraphToolValidationError::Runtime(ValueError::MissingCapability(_)))));
    }
    pub(super) fn run() {
        let heap = ValueHeap::new().unwrap();
        let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
        let schema = parse(&heap, r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#);
        let tool = make_tool(&heap, "paths", schema);
        let original = parse(&heap, r#"{"path":"file.txt","sibling":{"x":1}}"#);
        let result = validate_graph_tool_arguments(&factory, &tool, &call(&heap, "paths", original.clone())).unwrap();
        assert!(result.same_identity(&original));
        let encoded = parse(&heap, r#"{"\"path\"":"file.txt\n"}"#);
        let result = validate_graph_tool_arguments(&factory, &tool, &call(&heap, "paths", encoded)).unwrap();
        assert!(
            matches!(heap.snapshot(&get(&heap,&result,"path")).unwrap(),ValueSnapshot::String(value) if value.equals_ascii("file.txt"))
        );
        let schema = parse(
            &heap,
            r#"{"type":"object","properties":{"questions":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}}},"required":["questions"]}"#,
        );
        let nested = make_tool(&heap, "questions", schema);
        let flat = parse(&heap, r#"{"questions[0].id":"x"}"#);
        let result = validate_graph_tool_arguments(&factory, &nested, &call(&heap, "questions", flat.clone())).unwrap();
        assert!(matches!(heap.kind(&get(&heap, &result, "questions")).unwrap(), ValueKind::Array));
        let encoded =
            heap.string(heap.stringify(&flat, StringifyMode::Plain, None, &NoRuntime).unwrap().unwrap()).unwrap();
        assert!(matches!(
            validate_graph_tool_arguments(&factory, &nested, &call(&heap, "questions", encoded)),
            Err(GraphToolValidationError::Validation(_))
        ));
        rounds(&heap, &factory);
        spill_gate(&heap, &factory);
        parse_failure_and_find();
        actual_issue_maps();
    }

    fn rounds(heap: &Arc<ValueHeap>, factory: &GraphValueFactory) {
        for depth in [4, 5, 6] {
            let mut schema = parse(heap, r#"{"type":"object","properties":{"n":{"type":"integer"}},"required":["n"]}"#);
            let mut value = parse(heap, r#"{"n":1}"#);
            for _ in 0..depth {
                let properties = heap.object().unwrap();
                own(heap, &properties, "child", schema);
                schema = heap.object().unwrap();
                own(heap, &schema, "type", heap.string("object".into()).unwrap());
                own(heap, &schema, "properties", properties);
                own(heap, &schema, "required", heap.array(vec![heap.string("child".into()).unwrap()]).unwrap());
                let object = heap.object().unwrap();
                own(heap, &object, "child", value);
                value = heap
                    .string(heap.stringify(&object, StringifyMode::Plain, None, &NoRuntime).unwrap().unwrap())
                    .unwrap();
            }
            let tool = make_tool(heap, "rounds", schema);
            let result = validate_graph_tool_arguments(factory, &tool, &call(heap, "rounds", value));
            if depth == 6 {
                assert!(matches!(result, Err(GraphToolValidationError::Validation(_))));
            } else {
                let mut value = result.unwrap();
                for _ in 0..depth {
                    value = get(heap, &value, "child");
                }
                assert!(matches!(heap.snapshot(&get(heap, &value, "n")).unwrap(), ValueSnapshot::Number(1.0)));
            }
        }
        let scalar = heap.symbol(Some("actual context value".into())).unwrap();
        let tool = make_tool(heap, "any", heap.object().unwrap());
        assert!(
            validate_graph_tool_arguments(factory, &tool, &call(heap, "any", scalar.clone()))
                .unwrap()
                .same_identity(&scalar)
        );
        let tool = make_tool(heap, "integer", parse(heap, r#"{"type":"integer"}"#));
        let changed = heap.string("null".into()).unwrap();
        let Err(GraphToolValidationError::Validation(message)) =
            validate_graph_tool_arguments(factory, &tool, &call(heap, "integer", changed))
        else {
            panic!("reject null integer");
        };
        assert!(contains(&message, "Validation failed for tool \"integer\""));
        assert!(contains(&message, "\"original\": \"null\"") && contains(&message, "\"normalized\": null"));
        let long = heap.string(WireString::from_units(vec![0x61; 257])).unwrap();
        let Err(GraphToolValidationError::Validation(message)) =
            validate_graph_tool_arguments(factory, &tool, &call(heap, "integer", long))
        else {
            panic!("reject long string");
        };
        assert!(contains(&message, "… [truncated 1 chars]"));
        assert!(!contains(&message, "\"original\""));
    }
    fn spill_gate(heap: &Arc<ValueHeap>, factory: &GraphValueFactory) {
        let valid = make_tool(
            heap,
            "content",
            parse(heap, r#"{"type":"object","properties":{"content":{"type":"string"}},"required":["content"]}"#),
        );
        let args = heap.object().unwrap();
        own(
            heap,
            &args,
            "content",
            heap.string("done</arg_key><arg_key>task</arg_key><arg_value>text".into()).unwrap(),
        );
        assert!(
            validate_graph_tool_arguments(factory, &valid, &call(heap, "content", args.clone()))
                .unwrap()
                .same_identity(&args)
        );
        let schema = parse(
            heap,
            r#"{"type":"object","properties":{"op":{"type":"string","enum":["read"]},"count":{"type":"integer"}},"required":["op","count"]}"#,
        );
        let healing = make_tool(heap, "read", schema);
        let args = heap.object().unwrap();
        own(heap, &args, "op", heap.string("read</arg_key><arg_key>count</arg_key><arg_value>3".into()).unwrap());
        let result = validate_graph_tool_arguments(factory, &healing, &call(heap, "read", args)).unwrap();
        assert!(matches!(heap.snapshot(&get(heap, &result, "count")).unwrap(), ValueSnapshot::Number(3.0)));
        assert!(
            matches!(heap.snapshot(&get(heap,&result,"op")).unwrap(),ValueSnapshot::String(value) if value.equals_ascii("read"))
        );
    }
    fn parse_failure_and_find() {
        let heap = ValueHeap::new().unwrap();
        let schema = heap.object().unwrap();
        let args = heap.object().unwrap();
        let (factory, runtime) = binding(&heap, schema, args.clone());
        let first = heap.object().unwrap();
        accessor(&heap, &first, "name", 1);
        accessor(&heap, &first, "parameters", 5);
        let second = heap.object().unwrap();
        accessor(&heap, &second, "name", 3);
        let request = heap.object().unwrap();
        accessor(&heap, &request, "name", 2);
        accessor(&heap, &request, "arguments", 4);
        let result =
            validate_graph_tool_call(&factory, &heap.array(vec![first.clone(), second]).unwrap(), &request).unwrap();
        assert!(result.same_identity(&args));
        assert_eq!(*runtime.events.lock().unwrap(), ["tool-name", "call-name", "arguments", "parameters"]);
        let missing = heap.symbol(Some("missing name identity".into())).unwrap();
        let request = heap.object().unwrap();
        own(&heap, &request, "name", missing.clone());
        assert!(matches!(
            validate_graph_tool_call(&factory, &heap.array(vec![]).unwrap(), &request),
            Err(GraphToolValidationError::Runtime(ValueError::TypeError(_)))
        ));
        let missing = heap.string("absent".into()).unwrap();
        own(&heap, &request, "name", missing.clone());
        assert!(matches!(
            validate_graph_tool_call(&factory, &heap.array(vec![]).unwrap(), &request),
            Err(GraphToolValidationError::ToolNotFound { name, message })
                if name.same_identity(&missing) && message.equals_ascii("Tool \"absent\" not found")
        ));
        let object_name = heap.object().unwrap();
        own(&heap, &object_name, "toString", heap.function(CapabilityId(10)).unwrap());
        *runtime.captured.lock().unwrap() = Some(object_name.clone());
        accessor(&heap, &request, "name", 12);
        let unmatched = make_tool(&heap, "other", heap.object().unwrap());
        runtime.events.lock().unwrap().clear();
        assert!(matches!(
            validate_graph_tool_call(&factory, &heap.array(vec![unmatched]).unwrap(), &request),
            Err(GraphToolValidationError::ToolNotFound { name, message })
                if name.same_identity(&object_name) && message.equals_ascii("Tool \"chosen\" not found")
        ));
        assert_eq!(
            *runtime.events.lock().unwrap(),
            ["missing-call-name", "missing-call-name", "missing-name-to-string"]
        );
        own(&heap, &object_name, "toString", heap.function(CapabilityId(11)).unwrap());
        runtime.events.lock().unwrap().clear();
        assert!(matches!(
            validate_graph_tool_call(&factory, &heap.array(vec![]).unwrap(), &request),
            Err(GraphToolValidationError::Runtime(ValueError::Thrown(thrown)))
                if thrown.same_identity(&runtime.thrown)
        ));
        assert_eq!(*runtime.events.lock().unwrap(), ["missing-call-name", "throwing-name-to-string"]);
        accessor(&heap, &first, "name", 7);
        assert!(
            matches!(validate_graph_tool_call(&factory,&heap.array(vec![first]).unwrap(),&request),Err(GraphToolValidationError::Runtime(ValueError::Thrown(thrown))) if thrown.same_identity(&runtime.thrown))
        );
        let prototype = heap.object().unwrap();
        own(&heap, &prototype, "__parseError", heap.boolean(false).unwrap());
        let mut units = vec![0x61; 511];
        units.extend([0xd83d, 0xde00, 0xd800]);
        own(&heap, &prototype, "__rawJson", heap.string(WireString::from_units(units)).unwrap());
        let original = heap.object_with_prototype(Some(&prototype)).unwrap();
        let tool = heap.object().unwrap();
        accessor(&heap, &tool, "parameters", 6);
        let request = call(&heap, "parse", original);
        runtime.events.lock().unwrap().clear();
        let Err(GraphToolValidationError::Validation(message)) =
            validate_graph_tool_arguments(&factory, &tool, &request)
        else {
            panic!("parse marker rejects before context");
        };
        assert!(contains(&message, "Parse Error: false") && contains(&message, "… [truncated 2 chars]"));
        let tail = WireString::from("Raw JSON:\n");
        let at = message.units().windows(tail.units().len()).position(|part| part == tail.units()).unwrap()
            + tail.units().len();
        assert_eq!(message.units()[at + 511], 0xd83d);
        assert_eq!(message.units()[at + 512], 0x2026);
        assert!(runtime.events.lock().unwrap().is_empty());
        let ark = heap.function(CapabilityId(8)).unwrap();
        own(&heap, &ark, "toJsonSchema", heap.function(CapabilityId(8)).unwrap());
        own(&heap, &ark, "assert", heap.function(CapabilityId(8)).unwrap());
        let tool = make_tool(&heap, "ark", ark);
        assert!(matches!(
            validate_graph_tool_arguments(&factory, &tool, &call(&heap, "ark", heap.object().unwrap())),
            Err(GraphToolValidationError::Runtime(ValueError::MissingCapability(CapabilityId(404))))
        ));
    }
    fn actual_issue_maps() {
        let heap = ValueHeap::new().unwrap();
        let (factory, runtime) = binding(&heap, heap.object().unwrap(), heap.object().unwrap());
        let driver = Driver { factory: &factory };
        let expected = heap.array(vec![heap.string("number".into()).unwrap()]).unwrap();
        let path = heap.symbol(Some("path Symbol".into())).unwrap();
        let result = JsonSchemaValidationResult {
            success: false,
            issues: vec![JsonSchemaValidationIssue {
                path: vec![path],
                message: "wrong type".into(),
                expected_types: Some(expected.clone()),
                keyword: Some("type"),
                from_union_branch: true,
            }],
        };
        let issues = driver.rich_issues(result).unwrap();
        let flat = driver.flatten(&issues).unwrap();
        let record = get(&heap, &flat, "0");
        assert!(get(&heap, &record, "expectedTypes").same_identity(&expected));
        assert!(
            matches!(heap.snapshot(&get(&heap,&record,"instancePath")).unwrap(),ValueSnapshot::String(value) if value.equals_ascii("/Symbol(path Symbol)"))
        );
        assert!(matches!(heap.snapshot(&get(&heap, &record, "unionBranch")).unwrap(), ValueSnapshot::Boolean(true)));
        let messages = driver.messages(&issues).unwrap();
        assert!(
            matches!(heap.snapshot(&get(&heap,&messages,"0")).unwrap(),ValueSnapshot::String(value) if value.equals_ascii("  - Symbol(path Symbol): wrong type"))
        );
        let returned = heap.symbol(Some("actual map result".into())).unwrap();
        *runtime.captured.lock().unwrap() = Some(returned.clone());
        own(&heap, &issues, "map", heap.function(CapabilityId(9)).unwrap());
        assert!(driver.flatten(&issues).unwrap().same_identity(&returned));
        assert!(driver.messages(&issues).unwrap().same_identity(&returned));
        assert_eq!(*runtime.events.lock().unwrap(), ["map", "map"]);
        let foreign = ValueHeap::new().unwrap().object().unwrap();
        assert!(matches!(
            validate_graph_tool_call(&factory, &foreign, &heap.object().unwrap()),
            Err(GraphToolValidationError::Runtime(ValueError::ForeignHeap))
        ));
    }
}

#[cfg(test)]
fn observe_retained_driver() {
    use ara_rpc::value_heap::{CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor};
    use retained_observations::{LaterRuntime, call, capture, own, text};
    let weak = {
        let heap = ValueHeap::new().unwrap();
        let tool = heap.import_serde(&serde_json::json!({"name":"tool","parameters":{}})).unwrap();
        let request = heap.import_serde(&serde_json::json!({"name":"tool","arguments":{}})).unwrap();
        let tools = heap.array(vec![tool.clone()]).unwrap();
        let saved = capture(&heap, "find", |factory| {
            // b91 stamps require an object key; keep primitive rejection as a
            // separate direct-arguments check without adding a captured find.
            let primitive = heap.import_serde(&serde_json::json!({"name":"tool","parameters":true})).unwrap();
            assert!(matches!(validate_graph_tool_arguments(factory, &primitive, &request),
                Err(GraphToolValidationError::Runtime(ValueError::TypeError(_)))));
            validate_graph_tool_call(factory, &tools, &request).unwrap();
        });
        assert_eq!(saved.len(), 1);
        own(&heap, &request, "name", heap.string("later".into()).unwrap());
        assert!(!heap.truthy(&call(&heap, &saved[0], &tool, &NoRuntime)).unwrap());
        for (receiver, id) in [(&tool, 7100), (&request, 7101)] {
            heap.define_own(
                receiver,
                "name".into(),
                PropertyDescriptor::Accessor {
                    get: Some(CallableTarget::Host(CapabilityId(id))),
                    set: None,
                    enumerable: true,
                    configurable: true,
                },
                &NoRuntime,
            )
            .unwrap();
        }
        let runtime = LaterRuntime::new(&heap);
        *runtime.reenter.lock().unwrap() = Some((saved[0].clone(), tool.clone()));
        assert!(heap.truthy(&call(&heap, &saved[0], &tool, &runtime)).unwrap());
        assert_eq!(*runtime.events.lock().unwrap(), vec![7100, 7100, 7101, 7101]);
        let thrown = heap.object().unwrap();
        *runtime.thrown.lock().unwrap() = Some(thrown.clone());
        heap.define_own(
            &tool,
            "name".into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(7102))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            &NoRuntime,
        )
        .unwrap();
        assert!(matches!(heap.call(&saved[0], &heap.undefined().unwrap(), std::slice::from_ref(&tool), &runtime),
            Err(ValueError::Thrown(value)) if value.same_identity(&thrown)));
        assert_eq!(*runtime.events.lock().unwrap(), vec![7100, 7100, 7101, 7101, 7102]);
        let issue = heap
            .import_serde(
                &serde_json::json!({"keyword":"type","path":["field"],"expectedTypes":["number"],"message":"old"}),
            )
            .unwrap();
        let issues = heap.array(vec![issue.clone()]).unwrap();
        let flat = capture(&heap, "map", |factory| {
            Driver { factory }.flatten(&issues).unwrap();
        });
        assert_eq!(flat.len(), 2); // outer1684 then nested537
        let result = call(&heap, &flat[0], &issue, &NoRuntime);
        assert!(text(&heap, &heap.get(&result, &"instancePath".into(), &NoRuntime).unwrap()).equals_ascii("/field"));
        let ark = capture(&heap, "map", |factory| {
            graph_validation_messages(factory, &issues).unwrap();
        });
        let json = capture(&heap, "map", |factory| {
            Driver { factory }.messages(&issues).unwrap();
        });
        assert_eq!(ark.len(), 2);
        assert_eq!(json.len(), 2);
        assert!(!ark[0].same_identity(&json[0]));
        own(&heap, &issue, "message", heap.string("later".into()).unwrap());
        assert!(text(&heap, &call(&heap, &ark[0], &issue, &NoRuntime)).equals_ascii("  - field: later"));
        assert!(text(&heap, &call(&heap, &json[0], &issue, &NoRuntime)).equals_ascii("  - field: later"));
        std::sync::Arc::downgrade(&heap)
    };
    assert!(weak.upgrade().is_none());
}

#[cfg(test)]
pub(in crate::validation::graph) mod retained_observations {
    use super::*;
    use ara_rpc::value_heap::{CapabilityId, NoRuntime, RuntimeReply, RuntimeRequest};
    use std::sync::{Arc, Mutex, Weak};
    struct CaptureRuntime {
        heap: Weak<ValueHeap>,
        original: ValueHandle,
        saved: Mutex<Vec<ValueHandle>>,
    }
    impl ValueRuntime for CaptureRuntime {
        fn resolve(&self, _request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            Err(ValueError::UnsupportedOperation("retained observation has no extra Host capability"))
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            args: &[ValueHandle],
            runtime: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            if capability != CapabilityId(7000) {
                return Err(ValueError::MissingCapability(capability));
            }
            self.saved.lock().unwrap().push(args[0].clone());
            self.heap.upgrade().unwrap().call(&self.original, receiver, args, runtime).map(RuntimeReply::Value)
        }
    }
    pub(in crate::validation::graph) fn own(
        heap: &Arc<ValueHeap>,
        receiver: &ValueHandle,
        key: &str,
        value: ValueHandle,
    ) {
        heap.define_own_data(receiver, key.into(), value, &NoRuntime).unwrap();
    }
    pub(in crate::validation::graph) fn capture(
        heap: &Arc<ValueHeap>,
        method: &str,
        action: impl FnOnce(&GraphValueFactory),
    ) -> Vec<ValueHandle> {
        let prototype = heap.array_prototype();
        let original = heap.get(&prototype, &method.into(), &NoRuntime).unwrap();
        let runtime = Arc::new(CaptureRuntime {
            heap: Arc::downgrade(heap),
            original: original.clone(),
            saved: Mutex::new(Vec::new()),
        });
        let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
        own(heap, &prototype, method, heap.function(CapabilityId(7000)).unwrap());
        action(&factory);
        own(heap, &prototype, method, original);
        drop(factory);
        let saved = std::mem::take(&mut *runtime.saved.lock().unwrap());
        drop(runtime); // saved functions have outlived both the old factory and runtime
        saved
    }
    pub(in crate::validation::graph) fn call(
        heap: &Arc<ValueHeap>,
        function: &ValueHandle,
        argument: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueHandle {
        heap.call(function, &heap.undefined().unwrap(), std::slice::from_ref(argument), runtime).unwrap()
    }
    pub(in crate::validation::graph) fn text(heap: &Arc<ValueHeap>, value: &ValueHandle) -> WireString {
        match heap.snapshot(value).unwrap() {
            ValueSnapshot::String(value) => value,
            _ => panic!("expected actual String"),
        }
    }
    pub(in crate::validation::graph) struct LaterRuntime {
        heap: Weak<ValueHeap>,
        pub(in crate::validation::graph) events: Mutex<Vec<u64>>,
        pub(in crate::validation::graph) reenter: Mutex<Option<(ValueHandle, ValueHandle)>>,
        pub(in crate::validation::graph) value: Mutex<Option<ValueHandle>>,
        pub(in crate::validation::graph) thrown: Mutex<Option<ValueHandle>>,
    }
    impl LaterRuntime {
        pub(in crate::validation::graph) fn new(heap: &Arc<ValueHeap>) -> Self {
            Self {
                heap: Arc::downgrade(heap),
                events: Mutex::new(Vec::new()),
                reenter: Mutex::new(None),
                value: Mutex::new(None),
                thrown: Mutex::new(None),
            }
        }
    }
    impl ValueRuntime for LaterRuntime {
        fn resolve(&self, _request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            Err(ValueError::UnsupportedOperation("later runtime extra capability is unavailable"))
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            _receiver: &ValueHandle,
            _args: &[ValueHandle],
            runtime: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            self.events.lock().unwrap().push(capability.0);
            let heap = self.heap.upgrade().unwrap();
            match capability.0 {
                7100 => {
                    let reenter = self.reenter.lock().unwrap().take();
                    if let Some((function, argument)) = reenter {
                        let nested = heap.call(&function, &heap.undefined()?, &[argument], runtime)?;
                        assert!(heap.truthy(&nested)?);
                    }
                    heap.string("later".into()).map(RuntimeReply::Value)
                }
                7101 => heap.string("later".into()).map(RuntimeReply::Value),
                7102 => Err(ValueError::Thrown(self.thrown.lock().unwrap().as_ref().unwrap().clone())),
                7103 => heap.boolean(false).map(RuntimeReply::Value),
                7104 => Ok(RuntimeReply::Value(self.value.lock().unwrap().as_ref().unwrap().clone())),
                _ => Err(ValueError::MissingCapability(capability)),
            }
        }
    }
}
