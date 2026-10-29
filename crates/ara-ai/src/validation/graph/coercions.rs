//! Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/ai/src/utils/validation.ts:53-657,1586-1641,1703-1705.
//! Actual issue-directed coercion, conservative JSON parsing and JSON pointers.
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

use super::GraphNormalization;
use crate::graph_values::{GraphExecution, GraphValueFactory};
use crate::schema_validator::graph_values_compare_with_execution;
use ara_rpc::WireString;
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, IteratorRecord, PropertyKey, StringifyMode, ValueError, ValueHandle,
    ValueHeap, ValueKind, ValueRuntime, ValueSnapshot,
};

pub fn coerce_graph_args_from_issues(
    factory: &GraphValueFactory,
    args: &ValueHandle,
    issues: &ValueHandle,
) -> Result<GraphNormalization, ValueError> {
    factory.heap().adopt(args)?;
    factory.heap().adopt(issues)?;
    Coercer { execution: GraphExecution::from_factory(factory) }.issues(args, issues)
}
pub fn try_parse_graph_json_for_types(
    factory: &GraphValueFactory,
    value: &ValueHandle,
    expected_types: &ValueHandle,
) -> Result<GraphNormalization, ValueError> {
    factory.heap().adopt(value)?;
    factory.heap().adopt(expected_types)?;
    Coercer { execution: GraphExecution::from_factory(factory) }.parse_types(value, expected_types, 0)
}
pub fn path_to_graph_pointer(factory: &GraphValueFactory, path: &ValueHandle) -> Result<ValueHandle, ValueError> {
    path_to_graph_pointer_with_runtime(factory, path, factory.runtime())
}
pub fn path_to_graph_pointer_with_runtime(
    factory: &GraphValueFactory,
    path: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    factory.heap().adopt(path)?;
    Coercer { execution: GraphExecution::new(factory.heap(), runtime) }.path_pointer(path, true)
}
pub fn format_graph_issue_path(factory: &GraphValueFactory, path: &ValueHandle) -> Result<ValueHandle, ValueError> {
    format_graph_issue_path_with_runtime(factory, path, factory.runtime())
}
pub fn format_graph_issue_path_with_runtime(
    factory: &GraphValueFactory,
    path: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    factory.heap().adopt(path)?;
    Coercer { execution: GraphExecution::new(factory.heap(), runtime) }.path_pointer(path, false)
}
pub fn get_graph_value_at_pointer(
    factory: &GraphValueFactory,
    root: &ValueHandle,
    pointer: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    factory.heap().adopt(root)?;
    factory.heap().adopt(pointer)?;
    Coercer { execution: GraphExecution::from_factory(factory) }.pointer_get(root, pointer)
}
pub fn set_graph_value_at_pointer(
    factory: &GraphValueFactory,
    root: &ValueHandle,
    pointer: &ValueHandle,
    value: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    factory.heap().adopt(root)?;
    factory.heap().adopt(pointer)?;
    factory.heap().adopt(value)?;
    Coercer { execution: GraphExecution::from_factory(factory) }.pointer_set(root, pointer, value)
}
pub fn delete_graph_value_at_pointer(
    factory: &GraphValueFactory,
    root: &ValueHandle,
    pointer: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    factory.heap().adopt(root)?;
    factory.heap().adopt(pointer)?;
    Coercer { execution: GraphExecution::from_factory(factory) }.pointer_delete(root, pointer)
}

static EXPECTED_TYPE_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:validation.ts:54",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: expected_type_callback,
};
fn expected_type_callback(
    heap: &std::sync::Arc<ValueHeap>,
    captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let value = &captures[0];
    let kind = match args.first() {
        Some(value) => value.clone(),
        None => heap.undefined()?,
    };
    let current = Coercer { execution: GraphExecution::new(heap, runtime) };
    let value_kind = heap.kind(value)?;
    let matched = if current.text(&kind, "string")? {
        value_kind == ValueKind::String
    } else if current.text(&kind, "number")? {
        matches!(heap.snapshot(value)?,ValueSnapshot::Number(value) if value.is_finite())
    } else if current.text(&kind, "integer")? {
        matches!(heap.snapshot(value)?,ValueSnapshot::Number(value) if value.is_finite() && value.fract()==0.0)
    } else if current.text(&kind, "boolean")? {
        value_kind == ValueKind::Boolean
    } else if current.text(&kind, "null")? {
        value_kind == ValueKind::Null
    } else if current.text(&kind, "array")? {
        value_kind == ValueKind::Array
    } else if current.text(&kind, "object")? {
        value_kind == ValueKind::Object
    } else {
        false
    };
    heap.boolean(matched)
}
static POINTER_SEGMENT_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:validation.ts:537",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: pointer_segment_callback,
};
static ISSUE_SEGMENT_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:validation.ts:1704",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: issue_segment_callback,
};
static DECODE_SEGMENT_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:validation.ts:549",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: decode_segment_callback,
};
fn first_argument(heap: &std::sync::Arc<ValueHeap>, args: &[ValueHandle]) -> Result<ValueHandle, ValueError> {
    args.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())
}
fn pointer_segment_callback(
    heap: &std::sync::Arc<ValueHeap>,
    _captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let segment = first_argument(heap, args)?;
    let string = heap.string(heap.to_string(&segment, runtime)?)?;
    let read = Coercer { execution: GraphExecution::new(heap, runtime) };
    let escaped = read.replace_literal(&string, "~", "~0")?;
    read.replace_literal(&escaped, "/", "~1")
}
fn issue_segment_callback(
    heap: &std::sync::Arc<ValueHeap>,
    _captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let segment = first_argument(heap, args)?;
    heap.string(heap.to_string(&segment, runtime)?)
}
fn decode_segment_callback(
    heap: &std::sync::Arc<ValueHeap>,
    _captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let segment = first_argument(heap, args)?;
    let read = Coercer { execution: GraphExecution::new(heap, runtime) };
    let slash = read.replace_literal(&segment, "~1", "/")?;
    read.replace_literal(&slash, "~0", "~")
}
pub(super) fn path_to_graph_pointer_with_execution(
    execution: GraphExecution<'_>,
    path: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    execution.heap().adopt(path)?;
    Coercer { execution }.path_pointer(path, true)
}
pub(super) fn format_graph_issue_path_with_execution(
    execution: GraphExecution<'_>,
    path: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    execution.heap().adopt(path)?;
    Coercer { execution }.path_pointer(path, false)
}

struct Coercer<'a> {
    execution: GraphExecution<'a>,
}
impl Coercer<'_> {
    fn unchanged(&self, value: &ValueHandle) -> GraphNormalization {
        GraphNormalization { value: value.clone(), changed: false }
    }
    fn changed(&self, value: ValueHandle) -> GraphNormalization {
        GraphNormalization { value, changed: true }
    }
    fn get(&self, value: &ValueHandle, key: &str) -> Result<ValueHandle, ValueError> {
        self.execution.heap().get(value, &key.into(), self.execution.runtime())
    }
    fn key(&self, value: &ValueHandle) -> Result<PropertyKey, ValueError> {
        let heap = self.execution.heap();
        let primitive = heap.to_primitive(value, true, self.execution.runtime())?;
        Ok(if heap.kind(&primitive)? == ValueKind::Symbol {
            PropertyKey::Symbol(heap.symbol_key(&primitive)?)
        } else {
            PropertyKey::String(heap.to_string(&primitive, self.execution.runtime())?)
        })
    }
    fn at(&self, value: &ValueHandle, index: f64) -> Result<ValueHandle, ValueError> {
        self.execution.heap().get(value, &self.key(&self.execution.heap().number(index)?)?, self.execution.runtime())
    }
    fn call(&self, receiver: &ValueHandle, name: &str, args: &[ValueHandle]) -> Result<ValueHandle, ValueError> {
        let method = self.get(receiver, name)?;
        self.execution.heap().call(&method, receiver, args, self.execution.runtime())
    }
    fn text(&self, value: &ValueHandle, expected: &str) -> Result<bool, ValueError> {
        Ok(matches!(self.execution.heap().snapshot(value)?, ValueSnapshot::String(text) if text.equals_ascii(expected)))
    }
    fn equal(&self, left: &ValueHandle, right: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.execution.heap();
        match (heap.snapshot(left)?, heap.snapshot(right)?) {
            (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => Ok(left == right),
            _ => heap.object_is(left, right),
        }
    }
    fn cmp(&self, left: &ValueHandle, right: &ValueHandle, op: &str) -> Result<bool, ValueError> {
        graph_values_compare_with_execution(self.execution, left, right, op)
    }
    fn loop_less(&self, index: f64, receiver: &ValueHandle, offset: f64) -> Result<bool, ValueError> {
        let length = self.get(receiver, "length")?;
        if offset == 0.0 {
            self.cmp(&self.execution.heap().number(index)?, &length, "<")
        } else {
            Ok(index < self.execution.heap().to_number(&length, self.execution.runtime())? - offset)
        }
    }
    fn includes(&self, types: &ValueHandle, expected: &str) -> Result<bool, ValueError> {
        let value = self.call(types, "includes", &[self.execution.heap().string(expected.into())?])?;
        self.execution.heap().truthy(&value)
    }
    fn number(&self, value: &ValueHandle) -> Result<f64, ValueError> {
        let heap = self.execution.heap();
        let value = heap.to_primitive(value, false, self.execution.runtime())?;
        match heap.snapshot(&value)? {
            ValueSnapshot::BigInt(value) => Ok(value.as_str().parse::<f64>().unwrap_or(f64::NAN)),
            _ => heap.to_number(&value, self.execution.runtime()),
        }
    }
    fn parse(&self, value: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.execution.heap();
        if heap.kind(value)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot convert Symbol to a string".into()));
        }
        heap.parse_strict_units(&heap.to_string(value, self.execution.runtime())?)
    }
    fn caught(error: &ValueError) -> bool {
        error.is_native_exception() || matches!(error, ValueError::StrictParse(_))
    }
    fn parse_caught(&self, value: &ValueHandle) -> Result<Option<ValueHandle>, ValueError> {
        match self.parse(value) {
            Ok(value) => Ok(Some(value)),
            Err(error) if Self::caught(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }
    fn close_error(&self, iterator: &mut IteratorRecord, original: ValueError) -> ValueError {
        match self.execution.heap().iterator_close(iterator, self.execution.runtime()) {
            Ok(()) => original,
            Err(close) if original.is_native_exception() && close.is_native_exception() => original,
            Err(close) => ValueError::IteratorClose { original: Box::new(original), close: Box::new(close) },
        }
    }

    fn matches(&self, value: &ValueHandle, types: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.execution.heap();
        let some = self.get(types, "some")?;
        let callback = heap.fresh_compiled_function(&EXPECTED_TYPE_CALLBACK, std::slice::from_ref(value))?;
        let result = heap.call(&some, types, &[callback], self.execution.runtime())?;
        heap.truthy(&result)
    }
    fn numeric_string(&self, value: &ValueHandle, types: &ValueHandle) -> Result<GraphNormalization, ValueError> {
        let heap = self.execution.heap();
        if !self.includes(types, "number")? && !self.includes(types, "integer")? {
            return Ok(self.unchanged(value));
        }
        let trimmed = self.call(value, "trim", &[])?;
        if !heap.truthy(&trimmed)? {
            return Ok(self.unchanged(value));
        }
        let regex = heap.module_regexp(
            "ara-ai.validation",
            "NUMERIC_STRING_PATTERN",
            r"^[+-]?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?$",
            "",
        )?;
        if !heap.truthy(&self.call(&regex, "test", std::slice::from_ref(&trimmed))?)? {
            return Ok(self.unchanged(value));
        }
        let parsed = self.number(&trimmed)?;
        if !parsed.is_finite() {
            return Ok(self.unchanged(value));
        }
        let parsed = heap.number(parsed)?;
        if !self.matches(&parsed, types)? {
            return Ok(self.unchanged(value));
        }
        Ok(self.changed(parsed))
    }
    fn boolean(&self, value: &ValueHandle, types: &ValueHandle) -> Result<GraphNormalization, ValueError> {
        let heap = self.execution.heap();
        if !self.includes(types, "boolean")? {
            return Ok(self.unchanged(value));
        }
        if let ValueSnapshot::Number(number) = heap.snapshot(value)? {
            return Ok(if number == 0.0 {
                self.changed(heap.boolean(false)?)
            } else if number == 1.0 {
                self.changed(heap.boolean(true)?)
            } else {
                self.unchanged(value)
            });
        }
        if heap.kind(value)? != ValueKind::String {
            return Ok(self.unchanged(value));
        }
        let trimmed = self.call(value, "trim", &[])?;
        let lowered = self.call(&trimmed, "toLowerCase", &[])?;
        for spelling in ["true", "1", "yes", "on"] {
            if self.text(&lowered, spelling)? {
                return Ok(self.changed(heap.boolean(true)?));
            }
        }
        for spelling in ["false", "0", "no", "off"] {
            if self.text(&lowered, spelling)? {
                return Ok(self.changed(heap.boolean(false)?));
            }
        }
        Ok(self.unchanged(value))
    }
    fn boolean_number(&self, value: &ValueHandle, types: &ValueHandle) -> Result<GraphNormalization, ValueError> {
        let heap = self.execution.heap();
        if !self.includes(types, "number")? && !self.includes(types, "integer")? {
            return Ok(self.unchanged(value));
        }
        if heap.kind(value)? != ValueKind::Boolean {
            return Ok(self.unchanged(value));
        }
        Ok(self.changed(heap.number(if heap.truthy(value)? { 1.0 } else { 0.0 })?))
    }
    fn string(
        &self,
        value: &ValueHandle,
        types: &ValueHandle,
        allow_lossy: bool,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.execution.heap();
        let kind = heap.kind(value)?;
        if !self.includes(types, "string")?
            || matches!(kind, ValueKind::String | ValueKind::Null | ValueKind::Undefined)
        {
            return Ok(self.unchanged(value));
        }
        if matches!(kind, ValueKind::Array | ValueKind::Object) {
            if !allow_lossy {
                return Ok(self.unchanged(value));
            }
            return match heap.stringify(value, StringifyMode::Plain, None, self.execution.runtime()) {
                Ok(Some(text)) => Ok(self.changed(heap.string(text)?)),
                Ok(None) => Ok(self.unchanged(value)),
                Err(error) if Self::caught(&error) => Ok(self.unchanged(value)),
                Err(error) => Err(error),
            };
        }
        if kind == ValueKind::Function {
            return Ok(self.unchanged(value));
        }
        Ok(self.changed(heap.string(heap.to_string(value, self.execution.runtime())?)?))
    }
    fn coerce(
        &self,
        value: &ValueHandle,
        types: &ValueHandle,
        allow_lossy: bool,
    ) -> Result<GraphNormalization, ValueError> {
        if self.execution.heap().kind(value)? == ValueKind::String {
            let parsed = self.parse_types(value, types, 0)?;
            if parsed.changed {
                return Ok(parsed);
            }
            return self.boolean(value, types);
        }
        let boolean = self.boolean(value, types)?;
        if boolean.changed {
            return Ok(boolean);
        }
        let numeric = self.boolean_number(value, types)?;
        if numeric.changed {
            return Ok(numeric);
        }
        self.string(value, types, allow_lossy)
    }

    fn implicit(&self, value: &ValueHandle) -> Result<WireString, ValueError> {
        if self.execution.heap().kind(value)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot convert Symbol to a string".into()));
        }
        self.execution.heap().to_string(value, self.execution.runtime())
    }
    fn append(&self, result: &ValueHandle, value: &ValueHandle) -> Result<ValueHandle, ValueError> {
        self.execution.heap().add(result, value, self.execution.runtime())
    }
    fn clean_escapes(&self, value: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.execution.heap();
        let mut result = heap.string("".into())?;
        let mut in_string = false;
        let mut index = 0.0;
        while self.loop_less(index, value, 0.0)? {
            let ch = self.at(value, index)?;
            if in_string {
                if self.text(&ch, "\\")? && self.loop_less(index + 1.0, value, 0.0)? {
                    let pair = heap.add(&ch, &self.at(value, index + 1.0)?, self.execution.runtime())?;
                    result = self.append(&result, &pair)?;
                    index += 2.0;
                    continue;
                }
                if self.text(&ch, "\"")? {
                    in_string = false;
                }
                result = self.append(&result, &ch)?;
                index += 1.0;
                continue;
            }
            if self.text(&ch, "\"")? {
                in_string = true;
                result = self.append(&result, &ch)?;
                index += 1.0;
                continue;
            }
            if self.text(&ch, "\\")? && self.loop_less(index + 1.0, value, 0.0)? {
                let next = self.at(value, index + 1.0)?;
                if self.text(&next, "n")? || self.text(&next, "t")? || self.text(&next, "r")? {
                    result = self.append(&result, &heap.string(" ".into())?)?;
                    index += 2.0;
                    continue;
                }
            }
            result = self.append(&result, &ch)?;
            index += 1.0;
        }
        Ok(result)
    }
    fn escape_controls(&self, value: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.execution.heap();
        let mut result = heap.string("".into())?;
        let mut in_string = false;
        let mut escaped = false;
        let mut changed = false;
        let mut index = 0.0;
        while self.loop_less(index, value, 0.0)? {
            let ch = self.at(value, index)?;
            if in_string {
                if escaped {
                    result = self.append(&result, &ch)?;
                    escaped = false;
                    index += 1.0;
                    continue;
                }
                if self.text(&ch, "\\")? {
                    result = self.append(&result, &ch)?;
                    escaped = true;
                    index += 1.0;
                    continue;
                }
                if self.text(&ch, "\"")? {
                    result = self.append(&result, &ch)?;
                    in_string = false;
                    index += 1.0;
                    continue;
                }
                let code = self.call(&ch, "charCodeAt", &[heap.number(0.0)?])?;
                if self.cmp(&code, &heap.number(32.0)?, "<")? {
                    changed = true;
                    let mut replacement = None;
                    for (raw, escaped) in
                        [("\n", "\\n"), ("\r", "\\r"), ("\t", "\\t"), ("\u{8}", "\\b"), ("\u{c}", "\\f")]
                    {
                        if self.text(&ch, raw)? {
                            replacement = Some(heap.string(escaped.into())?);
                            break;
                        }
                    }
                    let replacement = if let Some(replacement) = replacement {
                        replacement
                    } else {
                        let hex = self.call(&code, "toString", &[heap.number(16.0)?])?;
                        let padded = self.call(&hex, "padStart", &[heap.number(4.0)?, heap.string("0".into())?])?;
                        let mut units = WireString::from("\\u").units().to_vec();
                        units.extend(self.implicit(&padded)?.units());
                        heap.string(WireString::from_units(units))?
                    };
                    result = self.append(&result, &replacement)?;
                    index += 1.0;
                    continue;
                }
                result = self.append(&result, &ch)?;
                index += 1.0;
                continue;
            }
            if self.text(&ch, "\"")? {
                in_string = true;
            }
            result = self.append(&result, &ch)?;
            index += 1.0;
        }
        Ok(if changed { result } else { value.clone() })
    }
    fn heal(&self, value: &ValueHandle) -> Result<Option<ValueHandle>, ValueError> {
        let heap = self.execution.heap();
        if let Some(parsed) = self.parse_caught(value)? {
            return Ok(Some(parsed));
        }
        let tail = heap.to_number(&self.get(value, "length")?, self.execution.runtime())? - 7.0;
        let tail = if tail.is_nan() { f64::NAN } else { tail.max(0.0) };
        let mut index = tail;
        while self.loop_less(index, value, 0.0)? {
            let head = self.call(value, "slice", &[heap.number(0.0)?, heap.number(index)?])?;
            let end = self.call(value, "slice", &[heap.number(index + 1.0)?])?;
            let candidate = heap.add(&head, &end, self.execution.runtime())?;
            if let Some(parsed) = self.parse_caught(&candidate)? {
                return Ok(Some(parsed));
            }
            index += 1.0;
        }
        index = tail;
        let brackets = heap.module_string_array("ara-ai.validation", "BRACKET_CHARS", &["[", "]", "{", "}"])?;
        while self.loop_less(index, value, 0.0)? {
            let original = self.at(value, index)?;
            let mut iterator = heap.get_iterator(&brackets, self.execution.runtime())?;
            while let Some(replacement) = heap.iterator_next(&mut iterator, self.execution.runtime())? {
                let result = (|| {
                    if self.equal(&replacement, &original)? {
                        return Ok(None);
                    }
                    let head = self.call(value, "slice", &[heap.number(0.0)?, heap.number(index)?])?;
                    // Left-associative addition converts head/replacement before
                    // evaluating the second slice, exactly as the source does.
                    let candidate = heap.add(&head, &replacement, self.execution.runtime())?;
                    let end = self.call(value, "slice", &[heap.number(index + 1.0)?])?;
                    let candidate = heap.add(&candidate, &end, self.execution.runtime())?;
                    self.parse_caught(&candidate)
                })();
                match result {
                    Ok(Some(parsed)) => {
                        heap.iterator_close(&mut iterator, self.execution.runtime())?;
                        return Ok(Some(parsed));
                    }
                    Ok(None) => (),
                    Err(error) => return Err(self.close_error(&mut iterator, error)),
                }
            }
            index += 1.0;
        }
        Ok(None)
    }
    fn leading(&self, value: &ValueHandle) -> Result<Option<ValueHandle>, ValueError> {
        let heap = self.execution.heap();
        let first = self.at(value, 0.0)?;
        let closing = if self.text(&first, "{")? {
            "}"
        } else if self.text(&first, "[")? {
            "]"
        } else {
            return Ok(None);
        };
        let mut depth = 0.0_f64;
        let mut in_string = false;
        let mut escaped = false;
        let mut index = 0.0;
        while self.loop_less(index, value, 0.0)? {
            let ch = self.at(value, index)?;
            if in_string {
                if escaped {
                    escaped = false;
                    index += 1.0;
                    continue;
                }
                if self.text(&ch, "\\")? {
                    escaped = true;
                    index += 1.0;
                    continue;
                }
                if self.text(&ch, "\"")? {
                    in_string = false;
                }
                index += 1.0;
                continue;
            }
            if self.text(&ch, "\"")? {
                in_string = true;
                index += 1.0;
                continue;
            }
            if self.equal(&ch, &first)? {
                depth += 1.0;
                index += 1.0;
                continue;
            }
            if !self.text(&ch, closing)? {
                index += 1.0;
                continue;
            }
            depth -= 1.0;
            if depth != 0.0 {
                index += 1.0;
                continue;
            }
            let prefix = self.call(value, "slice", &[heap.number(0.0)?, heap.number(index + 1.0)?])?;
            if let Some(parsed) = self.parse_caught(&prefix)? {
                return Ok(Some(parsed));
            }
            let cleaned = self.clean_escapes(&prefix)?;
            if !self.equal(&cleaned, &prefix)?
                && let Some(parsed) = self.parse_caught(&cleaned)?
            {
                return Ok(Some(parsed));
            }
            let escaped = self.escape_controls(&prefix)?;
            if !self.equal(&escaped, &prefix)?
                && let Some(parsed) = self.parse_caught(&escaped)?
            {
                return Ok(Some(parsed));
            }
            return self.heal(&prefix);
        }
        Ok(None)
    }
    fn looks_container(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.execution.heap();
        if heap.kind(value)? != ValueKind::String {
            return Ok(false);
        }
        let trimmed = self.call(value, "trimStart", &[])?;
        if heap.truthy(&self.call(&trimmed, "startsWith", &[heap.string("{".into())?])?)? {
            let body = self.call(&trimmed, "slice", &[heap.number(1.0)?])?;
            let start = self.call(&body, "trimStart", &[])?;
            if heap.truthy(&self.call(&start, "startsWith", &[heap.string("\"".into())?])?)? {
                return Ok(true);
            }
            if heap.truthy(&self.call(&body, "includes", &[heap.string(":".into())?])?)? {
                return Ok(true);
            }
            let start = self.call(&body, "trimStart", &[])?;
            return heap.truthy(&self.call(&start, "startsWith", &[heap.string("}".into())?])?);
        }
        if !heap.truthy(&self.call(&trimmed, "startsWith", &[heap.string("[".into())?])?)? {
            return Ok(false);
        }
        let first = self.call(&trimmed, "slice", &[heap.number(1.0)?])?;
        let first = self.call(&first, "trimStart", &[])?;
        for prefix in ["{", "[", "\"", "]", "true", "false", "null"] {
            if heap.truthy(&self.call(&first, "startsWith", &[heap.string(prefix.into())?])?)? {
                return Ok(true);
            }
        }
        let regex = heap.regexp(r"^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?(?:\s*(?:,|\]|$))".into(), "")?;
        heap.truthy(&self.call(&regex, "test", &[first])?)
    }
    fn accept_parsed(
        &self,
        parsed: &ValueHandle,
        source: &ValueHandle,
        types: &ValueHandle,
        depth: usize,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.execution.heap();
        if heap.kind(parsed)? == ValueKind::Null && self.text(&self.call(source, "trim", &[])?, "null")? {
            return Ok(self.changed(heap.null()?));
        }
        if self.matches(parsed, types)? {
            return Ok(self.changed(parsed.clone()));
        }
        if heap.kind(parsed)? == ValueKind::String && !self.includes(types, "string")? && depth < 3 {
            return self.parse_types(parsed, types, depth + 1);
        }
        Ok(self.unchanged(source))
    }
    fn parse_types(
        &self,
        value: &ValueHandle,
        types: &ValueHandle,
        depth: usize,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.execution.heap();
        let trimmed = self.call(value, "trim", &[])?;
        if !heap.truthy(&trimmed)? {
            return Ok(self.unchanged(value));
        }
        let numeric = self.numeric_string(&trimmed, types)?;
        if numeric.changed {
            return Ok(numeric);
        }
        let object = heap.truthy(&self.call(&trimmed, "startsWith", &[heap.string("{".into())?])?)?
            && self.looks_container(&trimmed)?;
        let array = heap.truthy(&self.call(&trimmed, "startsWith", &[heap.string("[".into())?])?)?
            && self.looks_container(&trimmed)?;
        let string = heap.truthy(&self.call(&trimmed, "startsWith", &[heap.string("\"".into())?])?)?
            && !self.includes(types, "string")?;
        let literal = if self.text(&trimmed, "true")? || self.text(&trimmed, "false")? || self.text(&trimmed, "null")? {
            true
        } else {
            let regex = heap.module_regexp(
                "ara-ai.validation",
                "JSON_NUMBER_PATTERN",
                r"^[+-]?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?$",
                "",
            )?;
            heap.truthy(&self.call(&regex, "test", std::slice::from_ref(&trimmed))?)?
        };
        if !object && !array && !string && !literal {
            return Ok(self.unchanged(value));
        }
        let direct = (|| {
            let parsed = self.parse(&trimmed)?;
            self.accept_parsed(&parsed, &trimmed, types, depth)
        })();
        match direct {
            Ok(accepted) => {
                if accepted.changed {
                    return Ok(accepted);
                }
            }
            Err(error) if Self::caught(&error) => {
                if object || array {
                    let escaped = self.escape_controls(&trimmed)?;
                    if !self.equal(&escaped, &trimmed)? {
                        let attempt = (|| {
                            let parsed = self.parse(&escaped)?;
                            self.accept_parsed(&parsed, &escaped, types, depth)
                        })();
                        match attempt {
                            Ok(accepted) if accepted.changed => return Ok(accepted),
                            Ok(_) => (),
                            Err(error) if Self::caught(&error) => (),
                            Err(error) => return Err(error),
                        }
                    }
                    if let Some(leading) = self.leading(&trimmed)? {
                        let accepted = self.accept_parsed(&leading, &trimmed, types, depth)?;
                        if accepted.changed {
                            return Ok(accepted);
                        }
                    }
                    if let Some(healed) = self.heal(&trimmed)? {
                        let accepted = self.accept_parsed(&healed, &trimmed, types, depth)?;
                        if accepted.changed {
                            return Ok(accepted);
                        }
                    }
                }
                return Ok(self.unchanged(value));
            }
            Err(error) => return Err(error),
        }
        Ok(self.unchanged(value))
    }

    fn replace_literal(
        &self,
        value: &ValueHandle,
        pattern: &str,
        replacement: &str,
    ) -> Result<ValueHandle, ValueError> {
        let heap = self.execution.heap();
        let method = self.get(value, "replace")?;
        let regex = heap.regexp(pattern.into(), "g")?;
        heap.call(&method, value, &[regex, heap.string(replacement.into())?], self.execution.runtime())
    }
    fn path_pointer(&self, path: &ValueHandle, encode: bool) -> Result<ValueHandle, ValueError> {
        let heap = self.execution.heap();
        if self.equal(&self.get(path, "length")?, &heap.number(0.0)?)? {
            return heap.string(if encode { "" } else { "root" }.into());
        }
        let map = self.get(path, "map")?;
        let spec = if encode { &POINTER_SEGMENT_CALLBACK } else { &ISSUE_SEGMENT_CALLBACK };
        let callback = heap.fresh_compiled_function(spec, &[])?;
        let mapped = heap.call(&map, path, &[callback], self.execution.runtime())?;
        let joined = self.call(&mapped, "join", &[heap.string("/".into())?])?;
        if !encode {
            return Ok(joined);
        }
        let mut units = vec![0x2f];
        units.extend(self.implicit(&joined)?.units());
        heap.string(WireString::from_units(units))
    }
    fn decode_pointer(&self, pointer: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.execution.heap();
        if !heap.truthy(pointer)? {
            return heap.array(vec![]);
        }
        let split = self.call(pointer, "split", &[heap.string("/".into())?])?;
        let sliced = self.call(&split, "slice", &[heap.number(1.0)?])?;
        let map = self.get(&sliced, "map")?;
        let callback = heap.fresh_compiled_function(&DECODE_SEGMENT_CALLBACK, &[])?;
        heap.call(&map, &sliced, &[callback], self.execution.runtime())
    }
    fn pointer_get(&self, root: &ValueHandle, pointer: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.execution.heap();
        if !heap.truthy(pointer)? {
            return Ok(root.clone());
        }
        let segments = self.decode_pointer(pointer)?;
        let mut current = root.clone();
        let mut iterator = heap.get_iterator(&segments, self.execution.runtime())?;
        while let Some(segment) = heap.iterator_next(&mut iterator, self.execution.runtime())? {
            let result = (|| {
                if matches!(heap.kind(&current)?, ValueKind::Null | ValueKind::Undefined) {
                    return Ok(false);
                }
                if heap.kind(&current)? == ValueKind::Array {
                    let index = self.number(&segment)?;
                    if !index.is_finite() || index.fract() != 0.0 {
                        return Ok(false);
                    }
                    current = heap.get(&current, &self.key(&heap.number(index)?)?, self.execution.runtime())?;
                    return Ok(true);
                }
                if heap.kind(&current)? != ValueKind::Object {
                    return Ok(false);
                }
                current = heap.get(&current, &self.key(&segment)?, self.execution.runtime())?;
                Ok::<_, ValueError>(true)
            })();
            match result {
                Ok(true) => (),
                Ok(false) => {
                    heap.iterator_close(&mut iterator, self.execution.runtime())?;
                    return heap.undefined();
                }
                Err(error) => return Err(self.close_error(&mut iterator, error)),
            }
        }
        Ok(current)
    }
    fn pointer_set(
        &self,
        root: &ValueHandle,
        pointer: &ValueHandle,
        value: &ValueHandle,
    ) -> Result<ValueHandle, ValueError> {
        let heap = self.execution.heap();
        if !heap.truthy(pointer)? {
            return Ok(value.clone());
        }
        let segments = self.decode_pointer(pointer)?;
        let mut current = root.clone();
        let mut index = 0.0;
        while self.loop_less(index, &segments, 1.0)? {
            let segment = self.at(&segments, index)?;
            if matches!(heap.kind(&current)?, ValueKind::Null | ValueKind::Undefined) {
                return Ok(root.clone());
            }
            if heap.kind(&current)? == ValueKind::Array {
                let at = self.number(&segment)?;
                if !at.is_finite() || at.fract() != 0.0 {
                    return Ok(root.clone());
                }
                current = heap.get(&current, &self.key(&heap.number(at)?)?, self.execution.runtime())?;
                index += 1.0;
                continue;
            }
            if heap.kind(&current)? != ValueKind::Object {
                return Ok(root.clone());
            }
            current = heap.get(&current, &self.key(&segment)?, self.execution.runtime())?;
            index += 1.0;
        }
        let last = heap.to_number(&self.get(&segments, "length")?, self.execution.runtime())? - 1.0;
        let segment = self.at(&segments, last)?;
        if heap.kind(&current)? == ValueKind::Array {
            let at = self.number(&segment)?;
            if !at.is_finite() || at.fract() != 0.0 {
                return Ok(root.clone());
            }
            heap.assign(&current, self.key(&heap.number(at)?)?, value.clone(), self.execution.runtime())?;
            return Ok(root.clone());
        }
        if heap.kind(&current)? != ValueKind::Object {
            return Ok(root.clone());
        }
        heap.assign(&current, self.key(&segment)?, value.clone(), self.execution.runtime())?;
        Ok(root.clone())
    }
    fn pointer_delete(&self, root: &ValueHandle, pointer: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.execution.heap();
        if !heap.truthy(pointer)? {
            return Ok(root.clone());
        }
        let segments = self.decode_pointer(pointer)?;
        if self.equal(&self.get(&segments, "length")?, &heap.number(0.0)?)? {
            return Ok(root.clone());
        }
        self.delete_segment(root, &segments, 0.0)
    }
    fn delete_segment(
        &self,
        node: &ValueHandle,
        segments: &ValueHandle,
        depth: f64,
    ) -> Result<ValueHandle, ValueError> {
        let heap = self.execution.heap();
        let segment = self.at(segments, depth)?;
        let is_leaf = depth == heap.to_number(&self.get(segments, "length")?, self.execution.runtime())? - 1.0;
        if heap.kind(node)? == ValueKind::Array {
            let index = self.number(&segment)?;
            if !index.is_finite()
                || index.fract() != 0.0
                || index < 0.0
                || self.cmp(&heap.number(index)?, &self.get(node, "length")?, ">=")?
            {
                return Ok(node.clone());
            }
            if is_leaf {
                let next = self.call(node, "slice", &[])?;
                self.call(&next, "splice", &[heap.number(index)?, heap.number(1.0)?])?;
                return Ok(next);
            }
            let child = self.delete_segment(&self.at(node, index)?, segments, depth + 1.0)?;
            if self.equal(&child, &self.at(node, index)?)? {
                return Ok(node.clone());
            }
            let next = self.call(node, "slice", &[])?;
            heap.assign(&next, self.key(&heap.number(index)?)?, child, self.execution.runtime())?;
            return Ok(next);
        }
        if heap.kind(node)? != ValueKind::Object {
            return Ok(node.clone());
        }
        if !heap.has_own(node, &self.key(&segment)?, self.execution.runtime())? {
            return Ok(node.clone());
        }
        if is_leaf {
            // Object binding evaluates the selected getter even though its
            // value is omitted; rest then reads all remaining own fields.
            let key = self.key(&segment)?;
            heap.get(node, &key, self.execution.runtime())?;
            return heap.spread_excluding(node, &[key], self.execution.runtime());
        }
        let child = self.delete_segment(
            &heap.get(node, &self.key(&segment)?, self.execution.runtime())?,
            segments,
            depth + 1.0,
        )?;
        if self.equal(&child, &heap.get(node, &self.key(&segment)?, self.execution.runtime())?)? {
            return Ok(node.clone());
        }
        let next = heap.spread(node, self.execution.runtime())?;
        heap.define_own_data(&next, self.key(&segment)?, child, self.execution.runtime())?;
        Ok(next)
    }
    fn issues(&self, args: &ValueHandle, issues: &ValueHandle) -> Result<GraphNormalization, ValueError> {
        let heap = self.execution.heap();
        if self.equal(&self.get(issues, "length")?, &heap.number(0.0)?)? {
            return Ok(self.unchanged(args));
        }
        let mut changed = false;
        let mut owned = false;
        let mut next = args.clone();
        let mut iterator = heap.get_iterator(issues, self.execution.runtime())?;
        while let Some(issue) = heap.iterator_next(&mut iterator, self.execution.runtime())? {
            let result = (|| {
                if self.text(&self.get(&issue, "keyword")?, "unrecognized")? {
                    if heap.truthy(&self.get(&issue, "unionBranch")?)? {
                        return Ok(());
                    }
                    let previous = next.clone();
                    next = self.pointer_delete(&next, &self.get(&issue, "instancePath")?)?;
                    if !self.equal(&next, &previous)? {
                        changed = true;
                    }
                    return Ok(());
                }
                if !self.text(&self.get(&issue, "keyword")?, "type")? {
                    return Ok(());
                }
                if self.equal(&self.get(&self.get(&issue, "expectedTypes")?, "length")?, &heap.number(0.0)?)? {
                    return Ok(());
                }
                let current = self.pointer_get(&next, &self.get(&issue, "instancePath")?)?;
                let result = self.coerce(&current, &self.get(&issue, "expectedTypes")?, true)?;
                let mut coerced = if result.changed { result.value } else { heap.undefined()? };
                if heap.kind(&coerced)? == ValueKind::Undefined
                    && self.includes(&self.get(&issue, "expectedTypes")?, "array")?
                    && !heap.truthy(&self.get(&issue, "unionBranch")?)?
                    && heap.kind(&current)? != ValueKind::Undefined
                    && heap.kind(&current)? != ValueKind::Array
                {
                    let object = if heap.kind(&current)? == ValueKind::String {
                        self.parse_types(&current, &heap.array(vec![heap.string("object".into())?])?, 0)?
                    } else {
                        self.unchanged(&current)
                    };
                    if object.changed || !self.looks_container(&current)? {
                        coerced = heap.array(vec![if object.changed { object.value } else { current }])?;
                    }
                }
                if heap.kind(&coerced)? == ValueKind::Undefined {
                    return Ok(());
                }
                if !owned {
                    next = heap.structured_clone_json(&next, self.execution.runtime())?;
                    owned = true;
                    changed = true;
                }
                next = self.pointer_set(&next, &self.get(&issue, "instancePath")?, &coerced)?;
                Ok::<_, ValueError>(())
            })();
            if let Err(error) = result {
                return Err(self.close_error(&mut iterator, error));
            }
        }
        Ok(GraphNormalization { value: if changed { next } else { args.clone() }, changed })
    }
}
#[cfg(test)]
pub(super) fn observe_graph_coercions_family() {
    observations::run();
    observe_retained_coercions();
}

#[cfg(test)]
fn observe_retained_coercions() {
    use super::driver::retained_observations::{call, capture, own, text};
    use ara_rpc::value_heap::NoRuntime;
    let weak = {
        let heap = ValueHeap::new().unwrap();
        let value = heap.object().unwrap();
        let types = heap.array(vec![heap.string("object".into()).unwrap()]).unwrap();
        let saved = capture(&heap, "some", |factory| {
            let read = Coercer { execution: GraphExecution::from_factory(factory) };
            assert!(read.matches(&value, &types).unwrap());
        });
        assert_eq!(saved.len(), 1);
        assert!(heap.truthy(&call(&heap, &saved[0], &heap.string("object".into()).unwrap(), &NoRuntime)).unwrap());
        assert!(!heap.truthy(&call(&heap, &saved[0], &heap.string("array".into()).unwrap(), &NoRuntime)).unwrap());
        own(&heap, &value, "later", heap.boolean(true).unwrap());
        assert!(heap.truthy(&call(&heap, &saved[0], &heap.string("object".into()).unwrap(), &NoRuntime)).unwrap());
        for encode in [true, false] {
            let path = heap.array(vec![heap.string("a/b~c".into()).unwrap()]).unwrap();
            let saved = capture(&heap, "map", |factory| {
                if encode {
                    path_to_graph_pointer(factory, &path).unwrap();
                } else {
                    format_graph_issue_path(factory, &path).unwrap();
                }
            });
            assert_eq!(saved.len(), 1);
            let segment = heap.string("x/y~z".into()).unwrap();
            let result = call(&heap, &saved[0], &segment, &NoRuntime);
            assert!(text(&heap, &result).equals_ascii(if encode { "x~1y~0z" } else { "x/y~z" }));
        }
        let pointer = heap.string("/a~1b~0c".into()).unwrap();
        let saved = capture(&heap, "map", |factory| {
            Coercer { execution: GraphExecution::from_factory(factory) }.decode_pointer(&pointer).unwrap();
        });
        assert_eq!(saved.len(), 1);
        assert!(
            text(&heap, &call(&heap, &saved[0], &heap.string("x~1y~0z".into()).unwrap(), &NoRuntime))
                .equals_ascii("x/y~z")
        );
        std::sync::Arc::downgrade(&heap)
    };
    assert!(weak.upgrade().is_none());
}

#[cfg(test)]
mod observations {
    use super::*;
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueHeap,
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    fn own(heap: &Arc<ValueHeap>, value: &ValueHandle, key: &str, entry: ValueHandle) {
        heap.define_own_data(value, key.into(), entry, &NoRuntime).unwrap();
    }
    fn get(heap: &Arc<ValueHeap>, value: &ValueHandle, key: &str) -> ValueHandle {
        heap.get(value, &key.into(), &NoRuntime).unwrap()
    }
    fn string(heap: &Arc<ValueHeap>, value: &ValueHandle) -> WireString {
        let ValueSnapshot::String(value) = heap.snapshot(value).unwrap() else { panic!("String") };
        value
    }
    fn number(heap: &Arc<ValueHeap>, value: &ValueHandle) -> f64 {
        let ValueSnapshot::Number(value) = heap.snapshot(value).unwrap() else { panic!("Number") };
        value
    }
    fn types(heap: &Arc<ValueHeap>, values: &[&str]) -> ValueHandle {
        heap.array(values.iter().map(|value| heap.string((*value).into()).unwrap()).collect()).unwrap()
    }
    fn issue(heap: &Arc<ValueHeap>, keyword: &str, path: &str, expected: ValueHandle, union: bool) -> ValueHandle {
        let issue = heap.object().unwrap();
        own(heap, &issue, "keyword", heap.string(keyword.into()).unwrap());
        own(heap, &issue, "instancePath", heap.string(path.into()).unwrap());
        own(heap, &issue, "expectedTypes", expected);
        own(heap, &issue, "unionBranch", heap.boolean(union).unwrap());
        issue
    }
    fn apply(factory: &GraphValueFactory, value: &ValueHandle, expected: &[&str]) -> GraphNormalization {
        let heap = factory.heap();
        let issues = heap.array(vec![issue(heap, "type", "", types(heap, expected), false)]).unwrap();
        coerce_graph_args_from_issues(factory, value, &issues).unwrap()
    }
    fn accessor(heap: &Arc<ValueHeap>, value: &ValueHandle, key: &str, cap: u64) {
        heap.define_own(
            value,
            key.into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(cap))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            &NoRuntime,
        )
        .unwrap();
    }
    struct Runtime {
        heap: Arc<ValueHeap>,
        some: ValueHandle,
        iterator: ValueHandle,
        thrown: ValueHandle,
        captured: Mutex<Option<ValueHandle>>,
        events: Mutex<Vec<&'static str>>,
        keywords: AtomicUsize,
    }
    impl ValueRuntime for Runtime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let RuntimeRequest::Call { capability, receiver, arguments } = request else {
                return Err(ValueError::MissingCapability(request.capability()));
            };
            let heap = &self.heap;
            let value = match capability.0 {
                1 => {
                    self.events.lock().unwrap().push("some");
                    heap.call(&self.some, receiver, arguments, self)?
                }
                2 => {
                    self.events.lock().unwrap().push("iterator");
                    heap.call(&self.iterator, receiver, arguments, self)?
                }
                3 => {
                    self.events.lock().unwrap().push("keyword");
                    self.keywords.fetch_add(1, Ordering::SeqCst);
                    heap.string("type".into())?
                }
                4 => {
                    self.events.lock().unwrap().push("expectedTypes");
                    self.captured.lock().unwrap().clone().unwrap()
                }
                5 => {
                    self.events.lock().unwrap().push("toJSON-throw");
                    heap.define_own_data(receiver, "side".into(), heap.boolean(true)?, &NoRuntime)?;
                    return Err(ValueError::Thrown(self.thrown.clone()));
                }
                6 => return Err(ValueError::MissingCapability(CapabilityId(404))),
                7 => {
                    self.events.lock().unwrap().push("selected");
                    heap.undefined()?
                }
                8 => return Err(ValueError::Thrown(self.thrown.clone())),
                9 => {
                    self.events.lock().unwrap().push("map override");
                    self.captured.lock().unwrap().clone().unwrap()
                }
                10 => {
                    self.events.lock().unwrap().push("getter");
                    heap.number(9.0)?
                }
                11 => {
                    self.events.lock().unwrap().push("slice override");
                    self.captured.lock().unwrap().clone().unwrap()
                }
                12 => {
                    self.events.lock().unwrap().push("selected child");
                    self.captured.lock().unwrap().clone().unwrap()
                }
                _ => return Err(ValueError::MissingCapability(capability)),
            };
            Ok(RuntimeReply::Value(value))
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            arguments: &[ValueHandle],
            effective_runtime: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            if capability.0 == 1 {
                self.events.lock().unwrap().push("some");
                return self.heap.call(&self.some, receiver, arguments, effective_runtime).map(RuntimeReply::Value);
            }
            self.resolve(RuntimeRequest::Call { capability, receiver, arguments })
        }
    }
    fn binding(heap: &Arc<ValueHeap>) -> (GraphValueFactory, Arc<Runtime>) {
        let runtime = Arc::new(Runtime {
            heap: heap.clone(),
            some: get(heap, &heap.array_prototype(), "some"),
            iterator: heap.get(&heap.array_prototype(), &heap.iterator_key(), &NoRuntime).unwrap(),
            thrown: heap.symbol(Some("original throw".into())).unwrap(),
            captured: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            keywords: AtomicUsize::new(0),
        });
        (GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap(), runtime)
    }

    pub fn run() {
        let heap = ValueHeap::new().unwrap();
        let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
        for (source, expected) in
            [("+001.25e+2", 125.0), ("9007199254740993", 9_007_199_254_740_992.0), ("  3.0  ", 3.0)]
        {
            let result = apply(&factory, &heap.string(source.into()).unwrap(), &["number"]);
            assert!(result.changed);
            assert_eq!(number(&heap, &result.value), expected);
        }
        let result = apply(&factory, &heap.string("-0".into()).unwrap(), &["integer"]);
        assert!(result.changed && number(&heap, &result.value).is_sign_negative());
        for source in ["0x10", "Infinity", "1e999", "1.5", "not numeric"] {
            let value = heap.string(source.into()).unwrap();
            let result = apply(&factory, &value, &["integer"]);
            assert!(!result.changed && result.value.same_identity(&value));
        }
        for spelling in [" YES ", "true", "ON", "1"] {
            let result = apply(&factory, &heap.string(spelling.into()).unwrap(), &["boolean"]);
            assert!(result.changed && matches!(heap.snapshot(&result.value).unwrap(), ValueSnapshot::Boolean(true)));
        }
        for spelling in [" NO ", "false", "Off", "0"] {
            let result = apply(&factory, &heap.string(spelling.into()).unwrap(), &["boolean"]);
            assert!(result.changed && matches!(heap.snapshot(&result.value).unwrap(), ValueSnapshot::Boolean(false)));
        }
        let value = heap.number(2.0).unwrap();
        assert!(!apply(&factory, &value, &["boolean"]).changed);
        assert_eq!(number(&heap, &apply(&factory, &heap.boolean(false).unwrap(), &["integer"]).value), 0.0);
        assert!(
            string(&heap, &apply(&factory, &heap.bigint_decimal("11").unwrap(), &["string"]).value).equals_ascii("11")
        );
        assert!(
            string(&heap, &apply(&factory, &heap.symbol(Some("leaf".into())).unwrap(), &["string"]).value)
                .equals_ascii("Symbol(leaf)")
        );
        let null = apply(&factory, &heap.string("null".into()).unwrap(), &["string"]);
        assert!(null.changed && heap.kind(&null.value).unwrap() == ValueKind::Null);
        let value = heap.string("3".into()).unwrap();
        assert!(!apply(&factory, &value, &["string"]).changed);
        let parse = |source: &str, expected: &[&str]| {
            try_parse_graph_json_for_types(&factory, &heap.string(source.into()).unwrap(), &types(&heap, expected))
                .unwrap()
        };
        for source in [
            "[{\"target\":\"fn_foo#ABCD\",\"content\":\"code}\"}]}]",
            "[{\"target\":\"fn_bar#1234\",\"content\":\"return 1}\"}}",
            "[{\"target\":\"fn\",\"content\":\"return 1;\\n\"}\\n]",
            "[{\"target\":\"fn\",\"op\":\"replace\"}]\n</invoke>",
            "[{\"content\":\"line1\nline2\"}]",
        ] {
            let result = parse(source, &["array"]);
            assert!(result.changed && heap.kind(&result.value).unwrap() == ValueKind::Array);
        }
        for source in ["[a-z]*.rs", "[{\"target\":invalid json here}]", "plain", "[foo"] {
            assert!(!parse(source, &["array"]).changed);
        }
        // The default control branch, previously missed by narrow newline tests.
        let controls = parse("{\"x\":\"a\u{b}b\"}", &["object"]);
        assert!(controls.changed && string(&heap, &get(&heap, &controls.value, "x")).equals_ascii("a\u{b}b"));
        let mut encoded = heap.string("[1,2]".into()).unwrap();
        for _ in 0..3 {
            encoded = heap
                .string(heap.stringify(&encoded, StringifyMode::Plain, None, &NoRuntime).unwrap().unwrap())
                .unwrap();
        }
        assert!(try_parse_graph_json_for_types(&factory, &encoded, &types(&heap, &["array"])).unwrap().changed);
        encoded =
            heap.string(heap.stringify(&encoded, StringifyMode::Plain, None, &NoRuntime).unwrap().unwrap()).unwrap();
        assert!(!try_parse_graph_json_for_types(&factory, &encoded, &types(&heap, &["array"])).unwrap().changed);
        pointers_and_ownership(&heap, &factory);
        faults_and_live_methods();
    }

    fn pointers_and_ownership(heap: &Arc<ValueHeap>, factory: &GraphValueFactory) {
        let path = heap.array(vec![heap.string("a/b~c".into()).unwrap(), heap.number(0.0).unwrap()]).unwrap();
        assert!(string(heap, &path_to_graph_pointer(factory, &path).unwrap()).equals_ascii("/a~1b~0c/0"));
        assert!(string(heap, &format_graph_issue_path(factory, &path).unwrap()).equals_ascii("a/b~c/0"));
        assert!(
            string(heap, &format_graph_issue_path(factory, &heap.array(vec![]).unwrap()).unwrap()).equals_ascii("root")
        );
        let root = heap.object().unwrap();
        let array = heap.array(vec![heap.number(1.0).unwrap(), heap.number(2.0).unwrap()]).unwrap();
        own(heap, &root, "a/b~c", array.clone());
        assert_eq!(
            number(
                heap,
                &get_graph_value_at_pointer(factory, &root, &heap.string("/a~1b~0c/ 0 ".into()).unwrap()).unwrap()
            ),
            1.0
        );
        let payload = heap.symbol(Some("payload".into())).unwrap();
        set_graph_value_at_pointer(factory, &root, &heap.string("/a~1b~0c/-1".into()).unwrap(), &payload).unwrap();
        assert!(get(heap, &array, "-1").same_identity(&payload));
        let original = heap.object().unwrap();
        assert!(
            set_graph_value_at_pointer(factory, &original, &heap.string("/missing/leaf".into()).unwrap(), &payload)
                .unwrap()
                .same_identity(&original)
        );
        assert!(!heap.has_own(&original, &"missing".into(), &NoRuntime).unwrap());
        assert!(
            set_graph_value_at_pointer(factory, &original, &heap.string("".into()).unwrap(), &payload)
                .unwrap()
                .same_identity(&payload)
        );
        assert!(
            get_graph_value_at_pointer(factory, &original, &heap.string("".into()).unwrap())
                .unwrap()
                .same_identity(&original)
        );
        let deleted =
            delete_graph_value_at_pointer(factory, &root, &heap.string("/a~1b~0c/0".into()).unwrap()).unwrap();
        assert!(!deleted.same_identity(&root));
        assert_eq!(number(heap, &get(heap, &get(heap, &deleted, "a/b~c"), "0")), 2.0);
        assert_eq!(heap.array_length(&array, &NoRuntime).unwrap(), 2);
        let args = heap.object().unwrap();
        let child = heap.object().unwrap();
        let sibling = heap.object().unwrap();
        own(heap, &child, "extra", heap.number(1.0).unwrap());
        own(heap, &child, "count", heap.string("3".into()).unwrap());
        own(heap, &args, "child", child.clone());
        own(heap, &args, "sibling", sibling.clone());
        let deletion = issue(heap, "unrecognized", "/child/extra", types(heap, &[]), false);
        let result =
            coerce_graph_args_from_issues(factory, &args, &heap.array(vec![deletion.clone()]).unwrap()).unwrap();
        assert!(result.changed && !get(heap, &result.value, "child").same_identity(&child));
        assert!(get(heap, &result.value, "sibling").same_identity(&sibling));
        assert!(heap.has_own(&child, &"extra".into(), &NoRuntime).unwrap());
        let numeric = issue(heap, "type", "/child/count", types(heap, &["number"]), false);
        let result =
            coerce_graph_args_from_issues(factory, &args, &heap.array(vec![deletion, numeric]).unwrap()).unwrap();
        assert!(result.changed && !get(heap, &result.value, "sibling").same_identity(&sibling));
        assert_eq!(number(heap, &get(heap, &get(heap, &result.value, "child"), "count")), 3.0);
        assert!(string(heap, &get(heap, &child, "count")).equals_ascii("3"));
        let union_deletion = issue(heap, "unrecognized", "/child/extra", types(heap, &[]), true);
        assert!(
            !coerce_graph_args_from_issues(factory, &args, &heap.array(vec![union_deletion]).unwrap()).unwrap().changed
        );
        let wrap = issue(heap, "type", "/child", types(heap, &["array"]), false);
        let result = coerce_graph_args_from_issues(factory, &args, &heap.array(vec![wrap]).unwrap()).unwrap();
        assert!(result.changed && get(heap, &get(heap, &result.value, "child"), "0").same_identity(&child));
        let union_wrap = issue(heap, "type", "/child", types(heap, &["array"]), true);
        assert!(
            !coerce_graph_args_from_issues(factory, &args, &heap.array(vec![union_wrap]).unwrap()).unwrap().changed
        );
        let union_string = issue(heap, "type", "/child", types(heap, &["string"]), true);
        assert!(
            coerce_graph_args_from_issues(factory, &args, &heap.array(vec![union_string]).unwrap()).unwrap().changed
        );
        let malformed = heap.string("[{\"x\":invalid}]".into()).unwrap();
        assert!(!apply(factory, &malformed, &["array"]).changed);
    }

    fn faults_and_live_methods() {
        let heap = ValueHeap::new().unwrap();
        let (factory, runtime) = binding(&heap);
        let expected = types(&heap, &["number"]);
        own(&heap, &expected, "some", heap.function(CapabilityId(1)).unwrap());
        *runtime.captured.lock().unwrap() = Some(expected.clone());
        let item = issue(&heap, "type", "", expected.clone(), false);
        accessor(&heap, &item, "keyword", 3);
        accessor(&heap, &item, "expectedTypes", 4);
        let issues = heap.array(vec![item]).unwrap();
        own(&heap, &issues, "unused", heap.symbol(None).unwrap());
        heap.define_own_data(&issues, heap.iterator_key(), heap.function(CapabilityId(2)).unwrap(), &NoRuntime)
            .unwrap();
        let result = coerce_graph_args_from_issues(&factory, &heap.string("3".into()).unwrap(), &issues).unwrap();
        assert!(result.changed && number(&heap, &result.value) == 3.0);
        assert_eq!(runtime.keywords.load(Ordering::SeqCst), 2);
        assert_eq!(
            *runtime.events.lock().unwrap(),
            ["iterator", "keyword", "keyword", "expectedTypes", "expectedTypes", "some"]
        );
        // Two type writes share one clone. A source getter is traversed only
        // once; a prior deletion was covered separately as shallow ownership.
        runtime.events.lock().unwrap().clear();
        let args = heap.object().unwrap();
        own(&heap, &args, "count", heap.string("3".into()).unwrap());
        own(&heap, &args, "enabled", heap.string("YES".into()).unwrap());
        accessor(&heap, &args, "probe", 10);
        let issues = heap
            .array(vec![
                issue(&heap, "type", "/count", types(&heap, &["number"]), false),
                issue(&heap, "type", "/enabled", types(&heap, &["boolean"]), false),
            ])
            .unwrap();
        let result = coerce_graph_args_from_issues(&factory, &args, &issues).unwrap();
        assert!(result.changed && number(&heap, &get(&heap, &result.value, "count")) == 3.0);
        assert!(matches!(heap.snapshot(&get(&heap, &result.value, "enabled")).unwrap(), ValueSnapshot::Boolean(true)));
        assert_eq!(*runtime.events.lock().unwrap(), ["getter"]);
        assert!(string(&heap, &get(&heap, &args, "count")).equals_ascii("3"));
        // structuredClone's Native rejection falls back to JSON, retaining
        // its first traversal effects before the second getter read.
        runtime.events.lock().unwrap().clear();
        let args = heap.object().unwrap();
        accessor(&heap, &args, "probe", 10);
        own(&heap, &args, "callback", heap.function(CapabilityId(8)).unwrap());
        own(&heap, &args, "count", heap.string("3".into()).unwrap());
        let issues = heap.array(vec![issue(&heap, "type", "/count", types(&heap, &["number"]), false)]).unwrap();
        let result = coerce_graph_args_from_issues(&factory, &args, &issues).unwrap();
        assert!(result.changed && !heap.has_own(&result.value, &"callback".into(), &NoRuntime).unwrap());
        assert_eq!(*runtime.events.lock().unwrap(), ["getter", "getter"]);
        let args = heap.object().unwrap();
        own(&heap, &args, "toJSON", heap.function(CapabilityId(5)).unwrap());
        own(&heap, &args, "count", heap.string("3".into()).unwrap());
        assert!(
            matches!(coerce_graph_args_from_issues(&factory, &args, &issues), Err(ValueError::Thrown(thrown)) if thrown.same_identity(&runtime.thrown))
        );
        assert!(heap.has_own(&args, &"side".into(), &NoRuntime).unwrap());
        let object = heap.object().unwrap();
        own(&heap, &object, "toJSON", heap.function(CapabilityId(5)).unwrap());
        let result = apply(&factory, &object, &["string"]);
        assert!(!result.changed && result.value.same_identity(&object));
        assert!(heap.has_own(&object, &"side".into(), &NoRuntime).unwrap());
        own(&heap, &object, "toJSON", heap.function(CapabilityId(6)).unwrap());
        let issues = heap.array(vec![issue(&heap, "type", "", types(&heap, &["string"]), false)]).unwrap();
        assert!(matches!(
            coerce_graph_args_from_issues(&factory, &object, &issues),
            Err(ValueError::MissingCapability(CapabilityId(404)))
        ));
        let source = heap.object().unwrap();
        accessor(&heap, &source, "drop", 7);
        own(&heap, &source, "keep", heap.number(1.0).unwrap());
        runtime.events.lock().unwrap().clear();
        let deleted = delete_graph_value_at_pointer(&factory, &source, &heap.string("/drop".into()).unwrap()).unwrap();
        assert!(!deleted.same_identity(&source) && !heap.has_own(&deleted, &"drop".into(), &NoRuntime).unwrap());
        assert_eq!(*runtime.events.lock().unwrap(), ["selected"]);
        let child = heap.object().unwrap();
        *runtime.captured.lock().unwrap() = Some(child);
        let parent = heap.object().unwrap();
        accessor(&heap, &parent, "child", 12);
        runtime.events.lock().unwrap().clear();
        let deleted =
            delete_graph_value_at_pointer(&factory, &parent, &heap.string("/child/missing".into()).unwrap()).unwrap();
        assert!(deleted.same_identity(&parent));
        assert_eq!(*runtime.events.lock().unwrap(), ["selected child", "selected child"]);
        let selected = heap.array(vec![heap.number(9.0).unwrap(), heap.number(8.0).unwrap()]).unwrap();
        *runtime.captured.lock().unwrap() = Some(selected.clone());
        let array =
            heap.array(vec![heap.number(1.0).unwrap(), heap.number(2.0).unwrap(), heap.number(3.0).unwrap()]).unwrap();
        own(&heap, &array, "slice", heap.function(CapabilityId(11)).unwrap());
        runtime.events.lock().unwrap().clear();
        let deleted = delete_graph_value_at_pointer(&factory, &array, &heap.string("/1".into()).unwrap()).unwrap();
        assert!(deleted.same_identity(&selected) && heap.array_length(&selected, &NoRuntime).unwrap() == 1);
        assert_eq!(heap.array_length(&array, &NoRuntime).unwrap(), 3);
        assert_eq!(*runtime.events.lock().unwrap(), ["slice override"]);
        own(&heap, &selected, "splice", heap.function(CapabilityId(8)).unwrap());
        assert!(
            matches!(delete_graph_value_at_pointer(&factory, &array, &heap.string("/0".into()).unwrap()), Err(ValueError::Thrown(thrown)) if thrown.same_identity(&runtime.thrown))
        );
        let sample = heap.string("3".into()).unwrap();
        let proto = heap.prototype(&sample, &NoRuntime).unwrap().unwrap();
        own(&heap, &proto, "trim", heap.function(CapabilityId(8)).unwrap());
        assert!(
            matches!(coerce_graph_args_from_issues(&factory,&sample,&heap.array(vec![issue(&heap,"type","",expected,false)]).unwrap()),Err(ValueError::Thrown(thrown)) if thrown.same_identity(&runtime.thrown))
        );
        // A map override controls the decode result; the path writer consumes
        // its actual returned segments rather than a Rust eager split.
        let heap = ValueHeap::new().unwrap();
        let (factory, runtime) = binding(&heap);
        *runtime.captured.lock().unwrap() = Some(types(&heap, &["redirected"]));
        own(&heap, &heap.array_prototype(), "map", heap.function(CapabilityId(9)).unwrap());
        let source = heap.object().unwrap();
        let payload = heap.number(42.0).unwrap();
        own(&heap, &source, "redirected", payload.clone());
        assert!(
            get_graph_value_at_pointer(&factory, &source, &heap.string("/original".into()).unwrap())
                .unwrap()
                .same_identity(&payload)
        );
        assert_eq!(*runtime.events.lock().unwrap(), ["map override"]);
        let foreign = ValueHeap::new().unwrap().object().unwrap();
        assert!(matches!(
            coerce_graph_args_from_issues(&factory, &foreign, &heap.array(vec![]).unwrap()),
            Err(ValueError::ForeignHeap)
        ));
    }
}
