//! Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/ai/src/utils/validation.ts:1738-1887,1908-1921.
//! Actual in-band spill repair and UTF-16 argument error previews.
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

use super::{GraphNormalization, Normalizer};
use crate::graph_values::{GraphExecution, GraphValueFactory};
use crate::schema_validator::graph_values_compare_with_runtime;
use ara_rpc::WireString;
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, IteratorRecord, PropertyKey, ValueError, ValueHandle, ValueHeap,
    ValueKind, ValueRuntime, ValueSnapshot,
};

static SPILL_INDEX_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:validation.ts:1778",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: spill_index_callback,
};
fn spill_index_callback(
    heap: &std::sync::Arc<ValueHeap>,
    _captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    _runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let value = args.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())?;
    heap.boolean(!matches!(heap.snapshot(&value)?, ValueSnapshot::Number(value) if value == -1.0))
}
static TRUNCATE_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:validation.ts:1915",
    name: "truncateArgsForError",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 0,
    body: truncate_callback,
};
fn truncate_callback(
    heap: &std::sync::Arc<ValueHeap>,
    _captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let value = args.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())?;
    Preview { execution: GraphExecution::new(heap, runtime) }.run(&value)
}

const KEY_OPEN: &str = "<arg_key>";
const KEY_CLOSE: &str = "</arg_key>";
const VALUE_OPEN: &str = "<arg_value>";
const VALUE_CLOSE: &str = "</arg_value>";
const TOOL_CLOSE: &str = "</tool_call>";

pub fn heal_graph_inband_arg_spill(
    factory: &GraphValueFactory,
    value: &ValueHandle,
) -> Result<GraphNormalization, ValueError> {
    factory.heap().adopt(value)?;
    Normalizer { factory }.spill_heal(value)
}

impl Normalizer<'_> {
    fn spill_equal(&self, left: &ValueHandle, right: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        match (heap.snapshot(left)?, heap.snapshot(right)?) {
            (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => Ok(left == right),
            _ => heap.object_is(left, right),
        }
    }
    fn spill_cmp(&self, left: &ValueHandle, right: &ValueHandle, operator: &str) -> Result<bool, ValueError> {
        graph_values_compare_with_runtime(self.factory, left, right, operator, self.factory.runtime())
    }
    fn spill_offset(&self, at: &ValueHandle, offset: usize) -> Result<ValueHandle, ValueError> {
        self.factory.heap().add(at, &self.factory.heap().number(offset as f64)?, self.factory.runtime())
    }
    fn spill_starts(&self, text: &ValueHandle, token: &str, at: &ValueHandle) -> Result<bool, ValueError> {
        let result = self.method(text, "startsWith", &[self.factory.heap().string(token.into())?, at.clone()])?;
        self.factory.heap().truthy(&result)
    }
    fn spill_index(
        &self,
        text: &ValueHandle,
        token: &str,
        from: Option<&ValueHandle>,
    ) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let method = self.get(text, "indexOf")?;
        let token = heap.string(token.into())?;
        let args = match from {
            Some(from) => vec![token, from.clone()],
            None => vec![token],
        };
        heap.call(&method, text, &args, self.factory.runtime())
    }
    fn spill_index_after(
        &self,
        text: &ValueHandle,
        token: &str,
        at: &ValueHandle,
        offset: usize,
    ) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let method = self.get(text, "indexOf")?;
        let from = self.spill_offset(at, offset)?;
        heap.call(&method, text, &[heap.string(token.into())?, from], self.factory.runtime())
    }
    fn spill_slice_after(
        &self,
        text: &ValueHandle,
        at: &ValueHandle,
        offset: usize,
    ) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let method = self.get(text, "slice")?;
        let from = self.spill_offset(at, offset)?;
        heap.call(&method, text, &[from], self.factory.runtime())
    }
    fn spill_space_at(&self, text: &ValueHandle, at: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        let whitespace = heap.string(" \n\t\r".into())?;
        let method = self.get(&whitespace, "includes")?;
        let character = heap.get(text, &self.property_key(at)?, self.factory.runtime())?;
        let result = heap.call(&method, &whitespace, &[character], self.factory.runtime())?;
        heap.truthy(&result)
    }
    fn spill_skip_space(&self, text: &ValueHandle, from: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let mut at = from.clone();
        while self.spill_cmp(&at, &self.get(text, "length")?, "<")? && self.spill_space_at(text, &at)? {
            at = self.factory.heap().increment(&at, self.factory.runtime())?;
        }
        Ok(at)
    }
    fn spill_key_valid(&self, text: &ValueHandle, start: &ValueHandle, end: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        let pattern = heap.module_regexp("ara-ai.validation", "SPILL_KEY_PATTERN", r"^[\w.$-]{1,128}$", "")?;
        let test = self.get(&pattern, "test")?;
        let key = self.method(text, "slice", &[start.clone(), end.clone()])?;
        let result = heap.call(&test, &pattern, &[key], self.factory.runtime())?;
        heap.truthy(&result)
    }
    fn spill_pair_start(&self, text: &ValueHandle, at: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        if !self.spill_starts(text, KEY_OPEN, at)? {
            return Ok(false);
        }
        let start = self.spill_offset(at, KEY_OPEN.len())?;
        let end = self.spill_index(text, KEY_CLOSE, Some(&start))?;
        if self.spill_equal(&end, &heap.number(-1.0)?)? || !self.spill_key_valid(text, &start, &end)? {
            return Ok(false);
        }
        let at = self.spill_skip_space(text, &self.spill_offset(&end, KEY_CLOSE.len())?)?;
        self.spill_starts(text, VALUE_OPEN, &at)
    }
    fn spill_value_end(
        &self,
        text: &ValueHandle,
        from: &ValueHandle,
    ) -> Result<(ValueHandle, ValueHandle), ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let close = self.spill_index(text, VALUE_CLOSE, Some(from))?;
        let mut wrong = self.spill_index(text, KEY_CLOSE, Some(from))?;
        let mut open = self.spill_index(text, KEY_OPEN, Some(from))?;
        loop {
            let input = heap.array(vec![close.clone(), wrong.clone(), open.clone()])?;
            let filter = self.get(&input, "filter")?;
            let predicate = heap.fresh_compiled_function(&SPILL_INDEX_CALLBACK, &[])?;
            let candidates = heap.call(&filter, &input, &[predicate], runtime)?;
            if self.spill_equal(&self.get(&candidates, "length")?, &heap.number(0.0)?)? {
                return Ok((self.get(text, "length")?, self.get(text, "length")?));
            }
            // Math.min argument spread completes iteration before conversions.
            let mut iterator = heap.get_iterator(&candidates, runtime)?;
            let mut arguments = Vec::new();
            while let Some(argument) = heap.iterator_next(&mut iterator, runtime)? {
                arguments.push(argument);
            }
            let mut min = f64::INFINITY;
            for argument in arguments {
                let value = heap.to_number(&argument, runtime)?;
                if value.is_nan() {
                    min = f64::NAN;
                } else if !min.is_nan() && (value < min || value == 0.0 && min == 0.0 && value.is_sign_negative()) {
                    min = value;
                }
            }
            let at = heap.number(min)?;
            if self.spill_equal(&at, &close)? {
                return Ok((at.clone(), self.spill_offset(&at, VALUE_CLOSE.len())?));
            }
            if self.spill_equal(&at, &wrong)? {
                let follow = self.spill_skip_space(text, &self.spill_offset(&at, KEY_CLOSE.len())?)?;
                if self.spill_cmp(&follow, &self.get(text, "length")?, ">=")?
                    || self.spill_starts(text, KEY_OPEN, &follow)?
                    || self.spill_starts(text, TOOL_CLOSE, &follow)?
                {
                    return Ok((at.clone(), self.spill_offset(&at, KEY_CLOSE.len())?));
                }
                wrong = self.spill_index_after(text, KEY_CLOSE, &at, 1)?;
                continue;
            }
            if self.spill_pair_start(text, &at)? {
                let mut end = at.clone();
                while self.spill_cmp(&end, from, ">")? {
                    let previous = heap.number(heap.to_number(&end, runtime)? - 1.0)?;
                    if !self.spill_space_at(text, &previous)? {
                        break;
                    }
                    // end originates in Math.min and is always Number here.
                    end = previous;
                }
                return Ok((end, at));
            }
            open = self.spill_index_after(text, KEY_OPEN, &at, 1)?;
        }
    }
    fn spill_parse_pairs(&self, text: &ValueHandle) -> Result<Option<ValueHandle>, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let pairs = heap.array(vec![])?;
        let mut at = self.spill_skip_space(text, &heap.number(0.0)?)?;
        while self.spill_cmp(&at, &self.get(text, "length")?, "<")? {
            if self.spill_starts(text, TOOL_CLOSE, &at)? {
                at = self.spill_skip_space(text, &self.spill_offset(&at, TOOL_CLOSE.len())?)?;
                return Ok(if self.spill_cmp(&at, &self.get(text, "length")?, ">=")? { Some(pairs) } else { None });
            }
            if !self.spill_starts(text, KEY_OPEN, &at)? {
                return Ok(None);
            }
            let start = self.spill_offset(&at, KEY_OPEN.len())?;
            let end = self.spill_index(text, KEY_CLOSE, Some(&start))?;
            if self.spill_equal(&end, &heap.number(-1.0)?)? {
                return Ok(None);
            }
            let key = self.method(text, "slice", &[start, end.clone()])?;
            let pattern = heap.module_regexp("ara-ai.validation", "SPILL_KEY_PATTERN", r"^[\w.$-]{1,128}$", "")?;
            if !heap.truthy(&self.method(&pattern, "test", std::slice::from_ref(&key))?)? {
                return Ok(None);
            }
            at = self.spill_skip_space(text, &self.spill_offset(&end, KEY_CLOSE.len())?)?;
            if !self.spill_starts(text, VALUE_OPEN, &at)? {
                return Ok(None);
            }
            at = self.spill_offset(&at, VALUE_OPEN.len())?;
            let (end, next) = self.spill_value_end(text, &at)?;
            let push = self.get(&pairs, "push")?;
            let value = self.method(text, "slice", &[at, end])?;
            let pair = heap.array(vec![key, value])?;
            heap.call(&push, &pairs, &[pair], runtime)?;
            at = self.spill_skip_space(text, &next)?;
        }
        Ok(Some(pairs))
    }
    fn spill_split(&self, text: &ValueHandle) -> Result<Option<(ValueHandle, ValueHandle)>, ValueError> {
        let heap = self.factory.heap();
        let minus_one = heap.number(-1.0)?;
        let zero = heap.number(0.0)?;
        let mut wrong = self.spill_index(text, KEY_CLOSE, None)?;
        let mut open = self.spill_index(text, KEY_OPEN, None)?;
        while !self.spill_equal(&wrong, &minus_one)? || !self.spill_equal(&open, &minus_one)? {
            if !self.spill_equal(&wrong, &minus_one)?
                && (self.spill_equal(&open, &minus_one)? || self.spill_cmp(&wrong, &open, "<")?)
            {
                let suffix = self.spill_slice_after(text, &wrong, KEY_CLOSE.len())?;
                if let Some(pairs) = self.spill_parse_pairs(&suffix)? {
                    return Ok(Some((self.method(text, "slice", &[zero.clone(), wrong.clone()])?, pairs)));
                }
                wrong = self.spill_index_after(text, KEY_CLOSE, &wrong, 1)?;
                continue;
            }
            if self.spill_pair_start(text, &open)? {
                let suffix = self.method(text, "slice", std::slice::from_ref(&open))?;
                if let Some(pairs) = self.spill_parse_pairs(&suffix)?
                    && self.spill_cmp(&self.get(&pairs, "length")?, &zero, ">")?
                {
                    let head = self.method(text, "slice", &[zero.clone(), open.clone()])?;
                    return Ok(Some((self.method(&head, "trimEnd", &[])?, pairs)));
                }
            }
            open = self.spill_index_after(text, KEY_OPEN, &open, 1)?;
        }
        Ok(None)
    }
    fn spill_heal(&self, value: &ValueHandle) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        if self.null(value)? || !self.typeof_object(value)? || self.array(value)? {
            return Ok(self.unchanged(value));
        }
        let out = heap.spread(value, runtime)?;
        let recovered = heap.array(vec![])?;
        let mut changed = false;
        // Existing for-in engine supplies enumerable inherited string names.
        for key in heap.for_in_string_keys(value, runtime)? {
            let entry = heap.get(value, &key.clone().into(), runtime)?;
            if heap.kind(&entry)? != ValueKind::String {
                continue;
            }
            if !heap.truthy(&self.method(&entry, "includes", &[heap.string(KEY_OPEN.into())?])?)?
                && !heap.truthy(&self.method(&entry, "includes", &[heap.string(KEY_CLOSE.into())?])?)?
            {
                continue;
            }
            let Some((head, pairs)) = self.spill_split(&entry)? else {
                continue;
            };
            heap.assign(&out, key.into(), head, runtime)?;
            let push = self.get(&recovered, "push")?;
            let mut iterator = heap.get_iterator(&pairs, runtime)?;
            let mut arguments = Vec::new();
            while let Some(argument) = heap.iterator_next(&mut iterator, runtime)? {
                arguments.push(argument);
            }
            heap.call(&push, &recovered, &arguments, runtime)?;
            changed = true;
        }
        if !changed {
            return Ok(self.unchanged(value));
        }
        let mut iterator = heap.get_iterator(&recovered, runtime)?;
        while let Some(entry) = heap.iterator_next(&mut iterator, runtime)? {
            let result = (|| {
                let (key, entry) = self.pair(&entry)?;
                if !heap.has(&out, &self.property_key(&key)?, runtime)? {
                    heap.assign(&out, self.property_key(&key)?, entry, runtime)?;
                }
                Ok::<_, ValueError>(())
            })();
            if let Err(error) = result {
                return Err(self.close_error(&mut iterator, error));
            }
        }
        Ok(GraphNormalization { value: out, changed: true })
    }
}

pub fn truncate_graph_args_for_error(
    factory: &GraphValueFactory,
    value: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    factory.heap().adopt(value)?;
    Preview { execution: GraphExecution::from_factory(factory) }.run(value)
}

// A map callback must keep its effective runtime when it recursively calls a
// second map or an overridden source method. Do not replace it with the base.
struct Preview<'a> {
    execution: GraphExecution<'a>,
}

impl Preview<'_> {
    fn get(&self, value: &ValueHandle, key: &str) -> Result<ValueHandle, ValueError> {
        self.execution.heap().get(value, &key.into(), self.execution.runtime())
    }
    fn property_key(&self, value: &ValueHandle) -> Result<PropertyKey, ValueError> {
        let heap = self.execution.heap();
        let primitive = heap.to_primitive(value, true, self.execution.runtime())?;
        Ok(if heap.kind(&primitive)? == ValueKind::Symbol {
            PropertyKey::Symbol(heap.symbol_key(&primitive)?)
        } else {
            PropertyKey::String(heap.to_string(&primitive, self.execution.runtime())?)
        })
    }
    fn close_error(&self, iterator: &mut IteratorRecord, error: ValueError) -> ValueError {
        match self.execution.heap().iterator_close(iterator, self.execution.runtime()) {
            Ok(()) => error,
            Err(close) if error.is_native_exception() && close.is_native_exception() => error,
            Err(close) => ValueError::IteratorClose { original: Box::new(error), close: Box::new(close) },
        }
    }
    fn pair(&self, entry: &ValueHandle) -> Result<(ValueHandle, ValueHandle), ValueError> {
        let heap = self.execution.heap();
        let mut iterator = heap.get_iterator(entry, self.execution.runtime())?;
        let pair = (|| {
            let key = match heap.iterator_next(&mut iterator, self.execution.runtime())? {
                Some(value) => value,
                None => heap.undefined()?,
            };
            let value = match heap.iterator_next(&mut iterator, self.execution.runtime())? {
                Some(value) => value,
                None => heap.undefined()?,
            };
            Ok((key, value))
        })();
        match pair {
            Ok(pair) => {
                heap.iterator_close(&mut iterator, self.execution.runtime())?;
                Ok(pair)
            }
            Err(error) => Err(self.close_error(&mut iterator, error)),
        }
    }
    fn run(&self, value: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.execution.heap();
        match heap.kind(value)? {
            ValueKind::String => {
                let length = heap.to_number(&self.get(value, "length")?, self.execution.runtime())?;
                if length <= 256.0 {
                    return Ok(value.clone());
                }
                let slice = self.get(value, "slice")?;
                let head =
                    heap.call(&slice, value, &[heap.number(0.0)?, heap.number(256.0)?], self.execution.runtime())?;
                if heap.kind(&head)? == ValueKind::Symbol {
                    return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into()));
                }
                let head = heap.to_string(&head, self.execution.runtime())?;
                let excess = heap.to_number(&self.get(value, "length")?, self.execution.runtime())? - 256.0;
                let excess = heap.to_string(&heap.number(excess)?, self.execution.runtime())?;
                let mut units = head.units().to_vec();
                units.extend(WireString::from("… [truncated ").units());
                units.extend(excess.units());
                units.extend(WireString::from(" chars]").units());
                heap.string(WireString::from_units(units))
            }
            ValueKind::Array => {
                let method = self.get(value, "map")?;
                let callback =
                    heap.module_compiled_function("ara-ai.validation", "truncateArgsForError", &TRUNCATE_CALLBACK)?;
                heap.call(&method, value, &[callback], self.execution.runtime())
            }
            ValueKind::Object => {
                let out = heap.object()?;
                let entries = heap.object_entries(value, self.execution.runtime())?;
                let mut iterator = heap.get_iterator(&entries, self.execution.runtime())?;
                while let Some(entry) = heap.iterator_next(&mut iterator, self.execution.runtime())? {
                    let result = (|| {
                        let (key, entry) = self.pair(&entry)?;
                        // Assignment's computed LHS key is evaluated before RHS.
                        let key = self.property_key(&key)?;
                        let entry = self.run(&entry)?;
                        heap.assign(&out, key, entry, self.execution.runtime())
                    })();
                    if let Err(error) = result {
                        return Err(self.close_error(&mut iterator, error));
                    }
                }
                Ok(out)
            }
            _ => Ok(value.clone()),
        }
    }
}

#[cfg(test)]
pub(super) fn observe_graph_spill_preview_family() {
    observations::run();
    observe_retained_spill_preview();
}

#[cfg(test)]
fn observe_retained_spill_preview() {
    use super::driver::retained_observations::{call, capture, text};
    use ara_rpc::value_heap::NoRuntime;
    let weak = {
        let heap = ValueHeap::new().unwrap();
        let input = heap.string("payload</arg_value>".into()).unwrap();
        let from = heap.number(0.0).unwrap();
        let saved = capture(&heap, "filter", |factory| {
            Normalizer { factory }.spill_value_end(&input, &from).unwrap();
        });
        assert!(!saved.is_empty());
        assert!(!heap.truthy(&call(&heap, &saved[0], &heap.number(-1.0).unwrap(), &NoRuntime)).unwrap());
        assert!(heap.truthy(&call(&heap, &saved[0], &heap.string("-1".into()).unwrap(), &NoRuntime)).unwrap());
        let input = heap.array(vec![heap.string("short".into()).unwrap()]).unwrap();
        let saved = capture(&heap, "map", |factory| {
            truncate_graph_args_for_error(factory, &input).unwrap();
            truncate_graph_args_for_error(factory, &input).unwrap();
        });
        assert_eq!(saved.len(), 2);
        assert!(saved[0].same_identity(&saved[1]));
        assert!(
            text(&heap, &heap.get(&saved[0], &"name".into(), &NoRuntime).unwrap()).equals_ascii("truncateArgsForError")
        );
        let long = heap.string("x".repeat(258).into()).unwrap();
        let result = call(&heap, &saved[0], &long, &NoRuntime);
        assert!(text(&heap, &result).units().len() > 256);
        let nested = heap.array(vec![heap.array(vec![long]).unwrap()]).unwrap();
        let result = call(&heap, &saved[0], &nested, &NoRuntime);
        assert_eq!(heap.kind(&result).unwrap(), ValueKind::Array);
        std::sync::Arc::downgrade(&heap)
    };
    assert!(weak.upgrade().is_none());
}

#[cfg(test)]
mod observations {
    use super::*;
    use ara_rpc::value_heap::{CapabilityId, NoRuntime, RuntimeReply, RuntimeRequest, ValueHeap};
    use std::sync::{Arc, Mutex};

    fn own(heap: &Arc<ValueHeap>, value: &ValueHandle, key: &str, entry: ValueHandle) {
        heap.define_own_data(value, key.into(), entry, &NoRuntime).unwrap();
    }
    fn get(heap: &Arc<ValueHeap>, value: &ValueHandle, key: &str) -> ValueHandle {
        heap.get(value, &key.into(), &NoRuntime).unwrap()
    }
    fn text(heap: &Arc<ValueHeap>, value: &ValueHandle) -> WireString {
        let ValueSnapshot::String(value) = heap.snapshot(value).unwrap() else { panic!("string") };
        value
    }
    struct Runtime {
        heap: Arc<ValueHeap>,
        index: ValueHandle,
        push: ValueHandle,
        thrown: ValueHandle,
        events: Mutex<Vec<&'static str>>,
    }
    impl ValueRuntime for Runtime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let RuntimeRequest::Call { capability, receiver, arguments } = request else {
                return Err(ValueError::MissingCapability(request.capability()));
            };
            let value = match capability.0 {
                1 => {
                    self.events.lock().unwrap().push("indexOf");
                    self.heap.call(&self.index, receiver, arguments, self)?
                }
                2 => {
                    self.events.lock().unwrap().push("push");
                    self.heap.call(&self.push, receiver, arguments, self)?
                }
                3 => return Err(ValueError::Thrown(self.thrown.clone())),
                4 => self.heap.symbol(Some("slice Symbol".into()))?,
                // A dynamic map override returns its actual result, even when
                // it has a different shape and never invokes the callback.
                5 => self.heap.string("map override result".into())?,
                _ => return Err(ValueError::MissingCapability(capability)),
            };
            Ok(RuntimeReply::Value(value))
        }
    }

    fn spill() {
        let heap = ValueHeap::new().unwrap();
        let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
        for (contaminated, head, task) in [
            (
                "done</arg_key>\n<arg_key>task</arg_key>\n<arg_value>Unify history column property",
                "done",
                "Unify history column property",
            ),
            (
                "done\n<arg_key>task</arg_key>\n<arg_value>Unify history column property</arg_value>",
                "done",
                "Unify history column property",
            ),
            ("done </arg_key>\n<arg_key>task</arg_key><arg_value>task", "done ", "task"),
        ] {
            let source = heap.object().unwrap();
            own(&heap, &source, "op", heap.string(contaminated.into()).unwrap());
            let result = heal_graph_inband_arg_spill(&factory, &source).unwrap();
            assert!(result.changed && !result.value.same_identity(&source));
            assert!(text(&heap, &get(&heap, &result.value, "op")).equals_ascii(head));
            assert!(text(&heap, &get(&heap, &result.value, "task")).equals_ascii(task));
        }
        let source = heap.object().unwrap();
        own(&heap, &source, "op", heap.string("view</arg_key>\n</tool_call>".into()).unwrap());
        assert!(
            text(&heap, &get(&heap, &heal_graph_inband_arg_spill(&factory, &source).unwrap().value, "op"))
                .equals_ascii("view")
        );
        let source = heap.object().unwrap();
        own(&heap, &source, "op", heap.string("done  <arg_key>task</arg_key><arg_value>first  <arg_key>count</arg_key><arg_value>3</arg_value></tool_call>".into()).unwrap());
        let result = heal_graph_inband_arg_spill(&factory, &source).unwrap();
        assert!(result.changed);
        assert!(text(&heap, &get(&heap, &result.value, "op")).equals_ascii("done"));
        assert!(text(&heap, &get(&heap, &result.value, "task")).equals_ascii("first"));
        assert!(text(&heap, &get(&heap, &result.value, "count")).equals_ascii("3"));
        // Recovered values remain strings. The validation driver owns retries.
        for key in ["task", "toString", "constructor", "__proto__"] {
            let source = heap.object().unwrap();
            let entry = format!("done</arg_key><arg_key>{key}</arg_key><arg_value>spilled");
            own(&heap, &source, "op", heap.string(entry.into()).unwrap());
            if key == "task" {
                own(&heap, &source, key, heap.string("original".into()).unwrap());
            }
            let expected = get(&heap, &source, key);
            let result = heal_graph_inband_arg_spill(&factory, &source).unwrap();
            assert!(result.changed && get(&heap, &result.value, key).same_identity(&expected));
            if key != "task" {
                assert!(!heap.has_own(&result.value, &key.into(), &NoRuntime).unwrap());
            }
        }
        let source = heap.object().unwrap();
        own(
            &heap,
            &source,
            "op",
            heap.string(
                "done<arg_key>task</arg_key><arg_value>first</arg_value><arg_key>task</arg_key><arg_value>second"
                    .into(),
            )
            .unwrap(),
        );
        let result = heal_graph_inband_arg_spill(&factory, &source).unwrap();
        assert!(text(&heap, &get(&heap, &result.value, "task")).equals_ascii("first"));
        let prototype = heap.object().unwrap();
        own(
            &heap,
            &prototype,
            "op",
            heap.string("done</arg_key><arg_key>task</arg_key><arg_value>inherited".into()).unwrap(),
        );
        let source = heap.object_with_prototype(Some(&prototype)).unwrap();
        let result = heal_graph_inband_arg_spill(&factory, &source).unwrap();
        assert!(result.changed && heap.has_own(&result.value, &"op".into(), &NoRuntime).unwrap());
        assert!(text(&heap, &get(&heap, &result.value, "task")).equals_ascii("inherited"));
        for value in [
            "plain",
            "done<arg_key>invalid key</arg_key><arg_value>text",
            "done<arg_key>task</arg_key>ordinary",
            "done<arg_key>task</arg_key><arg_value>text</arg_value>garbage",
            "done</arg_key><arg_key>task</arg_key><arg_value>text</arg_value></tool_call>garbage",
        ] {
            let source = heap.object().unwrap();
            own(&heap, &source, "op", heap.string(value.into()).unwrap());
            let result = heal_graph_inband_arg_spill(&factory, &source).unwrap();
            assert!(!result.changed && result.value.same_identity(&source));
        }
        let sample = heap.string("done</arg_key><arg_key>task</arg_key><arg_value>text".into()).unwrap();
        let string_proto = heap.prototype(&sample, &NoRuntime).unwrap().unwrap();
        let runtime = Arc::new(Runtime {
            heap: heap.clone(),
            index: get(&heap, &sample, "indexOf"),
            push: get(&heap, &heap.array_prototype(), "push"),
            thrown: heap.symbol(Some("original throw".into())).unwrap(),
            events: Mutex::new(Vec::new()),
        });
        let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
        own(&heap, &string_proto, "indexOf", heap.function(CapabilityId(1)).unwrap());
        own(&heap, &heap.array_prototype(), "push", heap.function(CapabilityId(2)).unwrap());
        let source = heap.object().unwrap();
        own(&heap, &source, "op", sample);
        let result = heal_graph_inband_arg_spill(&factory, &source).unwrap();
        assert!(result.changed);
        assert!(runtime.events.lock().unwrap().contains(&"indexOf"));
        assert_eq!(runtime.events.lock().unwrap().iter().filter(|event| **event == "push").count(), 2);
        own(&heap, &string_proto, "indexOf", heap.function(CapabilityId(3)).unwrap());
        assert!(
            matches!(heal_graph_inband_arg_spill(&factory, &source), Err(ValueError::Thrown(thrown)) if thrown.same_identity(&runtime.thrown))
        );
        // Preview uses dynamic slice/map and propagates implicit Symbol errors.
        let long = heap.string(WireString::from_units(vec![0x61; 257])).unwrap();
        own(&heap, &string_proto, "slice", heap.function(CapabilityId(4)).unwrap());
        assert!(matches!(truncate_graph_args_for_error(&factory, &long), Err(ValueError::TypeError(_))));
        let array = heap.array(vec![long]).unwrap();
        own(&heap, &array, "map", heap.function(CapabilityId(5)).unwrap());
        assert!(
            text(&heap, &truncate_graph_args_for_error(&factory, &array).unwrap()).equals_ascii("map override result")
        );
        let foreign = ValueHeap::new().unwrap().object().unwrap();
        assert!(matches!(heal_graph_inband_arg_spill(&factory, &foreign), Err(ValueError::ForeignHeap)));
        assert!(matches!(truncate_graph_args_for_error(&factory, &foreign), Err(ValueError::ForeignHeap)));
    }

    pub fn run() {
        spill();
        let heap = ValueHeap::new().unwrap();
        let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
        for length in [0, 255, 256] {
            let source = heap.string(WireString::from_units(vec![0x61; length])).unwrap();
            assert!(truncate_graph_args_for_error(&factory, &source).unwrap().same_identity(&source));
        }
        let mut units = vec![0x61; 255];
        units.extend([0xd83d, 0xde00, 0xd800]);
        let source = heap.string(WireString::from_units(units)).unwrap();
        let result = truncate_graph_args_for_error(&factory, &source).unwrap();
        let ValueSnapshot::String(text) = heap.snapshot(&result).unwrap() else { panic!("preview string") };
        assert_eq!(text.units()[255], 0xd83d);
        assert_eq!(text.units()[256], 0x2026);
        assert_eq!(&text.units()[256..], WireString::from("… [truncated 2 chars]").units());
        let array = heap.array_with_length(2).unwrap();
        heap.define_own_data(&array, "1".into(), source, &NoRuntime).unwrap();
        let result = truncate_graph_args_for_error(&factory, &array).unwrap();
        assert!(!heap.has_own(&result, &"0".into(), &NoRuntime).unwrap());
        assert!(heap.has_own(&result, &"1".into(), &NoRuntime).unwrap());
        let object = heap.object().unwrap();
        let child = heap.object().unwrap();
        heap.define_own_data(&child, "x".into(), heap.number(1.0).unwrap(), &NoRuntime).unwrap();
        heap.define_own_data(&object, "__proto__".into(), child, &NoRuntime).unwrap();
        let result = truncate_graph_args_for_error(&factory, &object).unwrap();
        assert!(!heap.has_own(&result, &"__proto__".into(), &NoRuntime).unwrap());
        let prototype = heap.prototype(&result, &NoRuntime).unwrap().unwrap();
        assert!(heap.has_own(&prototype, &"x".into(), &NoRuntime).unwrap());
    }
}
