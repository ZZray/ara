//! Fixed OMP596f2da packages/utils/src/logger.ts:365-393,473-483,787-837.
//! MIT; Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! Original compiled timing module bodies. The Host supplies its real imports;
//! this selected module does not install the full Logger or production Session.

use crate::native_realm::NativeRealm;
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueHeap, ValueKind, ValueRuntime,
    },
};
use std::sync::Arc;

type Result<T> = std::result::Result<T, ValueError>;

/// Original module bindings in the same realm. Performance must be the real
/// monotonic Host object, process.env the live environment object, and fs the
/// original synchronous stderr writer. Date.now is not a performance substitute.
pub struct NativeLoggerTimeImports {
    pub performance: ValueHandle,
    pub process: ValueHandle,
    pub fs: ValueHandle,
}

/// Own graph handles only; persistent closures contain checked graph edges and
/// no heap/runtime/Run lease. Module lexical cells are never a public property.
pub struct NativeLoggerTimeModule {
    cells: ValueHandle,
    storage: ValueHandle,
    pub time: ValueHandle,
    pub start_timing: ValueHandle,
    pub startup_marker: ValueHandle,
}

impl NativeLoggerTimeModule {
    pub fn prepare(realm: &NativeRealm, imports: NativeLoggerTimeImports) -> Result<Self> {
        let heap = realm.values.heap();
        let runtime = realm.values.runtime();
        let cells = heap.object_with_prototype(None)?;
        heap.define_own_data(&cells, "recording".into(), heap.boolean(false)?, runtime)?;
        heap.define_own_data(&cells, "root".into(), heap.undefined()?, runtime)?;
        let storage = realm.context().prepare_storage(heap)?;
        Self::prepare_with_shared_cells(realm, imports, &cells, &storage)
    }

    /// Explicit complete-logger assembly. Adopt its original cells and ALS
    /// object without initializing a second timing root/recording state.
    pub fn prepare_with_shared_cells(
        realm: &NativeRealm,
        imports: NativeLoggerTimeImports,
        cells: &ValueHandle,
        storage: &ValueHandle,
    ) -> Result<Self> {
        let heap = realm.values.heap();
        let cells = heap.adopt(cells)?;
        let storage = heap.adopt(storage)?;
        let performance = heap.adopt(&imports.performance)?;
        let process = heap.adopt(&imports.process)?;
        let fs = heap.adopt(&imports.fs)?;
        let errors = [
            realm.globals.errors.type_error.clone(),
            realm.globals.errors.syntax_error.clone(),
            realm.globals.errors.range_error.clone(),
        ];
        let is_promise = realm.promise().prepare_is_promise(heap)?;
        let startup_marker = heap.fresh_compiled_function(
            &STARTUP_MARKER,
            &[process.clone(), fs, errors[0].clone(), errors[1].clone(), errors[2].clone()],
        )?;
        let start_timing = heap.fresh_compiled_function(
            &START_TIMING,
            &[cells.clone(), performance.clone(), errors[0].clone(), errors[1].clone(), errors[2].clone()],
        )?;
        let time = heap.fresh_compiled_function(
            &TIME,
            &[
                cells.clone(),
                storage.clone(),
                performance,
                process,
                startup_marker.clone(),
                is_promise,
                errors[0].clone(),
                errors[1].clone(),
                errors[2].clone(),
            ],
        )?;
        Ok(Self { cells, storage, time, start_timing, startup_marker })
    }

    /// Connect only actual exports of this selected original module. The Host
    /// must retain this bundle alongside the rest of its complete logger owner.
    pub fn install_into(&self, logger: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<()> {
        let heap = self.time.heap();
        heap.adopt(logger)?;
        for (name, function) in
            [("time", &self.time), ("startTiming", &self.start_timing), ("startupMarker", &self.startup_marker)]
        {
            heap.define_own_data(logger, name.into(), function.clone(), runtime)?;
        }
        Ok(())
    }

    pub fn root_span(&self, runtime: &dyn ValueRuntime) -> Result<ValueHandle> {
        self.cells.heap().get(&self.cells, &"root".into(), runtime)
    }

    /// Rust Host assembly only; never installed as a user-facing graph field.
    pub fn module_cells(&self) -> &ValueHandle {
        &self.cells
    }

    /// Host observation/complete-module binding, not a new source export.
    pub fn span_storage(&self) -> &ValueHandle {
        &self.storage
    }
}

fn argument(heap: &Arc<ValueHeap>, arguments: &[ValueHandle], index: usize) -> Result<ValueHandle> {
    arguments.get(index).cloned().map(Ok).unwrap_or_else(|| heap.undefined())
}

/// Convert native algorithm failures at their original operation, so a source
/// try/catch sees the original realm Error before executing finish(false).
/// Actual Thrown identities and infrastructure/carrier failures stay intact.
fn native<T>(
    heap: &Arc<ValueHeap>,
    errors: &[ValueHandle],
    runtime: &dyn ValueRuntime,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    operation().map_err(|error| {
        let (index, message) = match error {
            ValueError::TypeError(message) => (0, message),
            ValueError::SyntaxError(message) => (1, message),
            ValueError::RangeError(message) => (2, message),
            other => return other,
        };
        match (|| heap.call(&errors[index], &heap.undefined()?, &[heap.string(message.into())?], runtime))() {
            Ok(reason) => ValueError::Thrown(reason),
            Err(error) => error,
        }
    })
}

fn member(heap: &Arc<ValueHeap>, target: &ValueHandle, name: &str, runtime: &dyn ValueRuntime) -> Result<ValueHandle> {
    heap.get(target, &name.into(), runtime)
}

fn call_member(
    heap: &Arc<ValueHeap>,
    target: &ValueHandle,
    name: &str,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let function = member(heap, target, name, runtime)?;
    heap.call(&function, target, arguments, runtime)
}

fn debug_enabled(heap: &Arc<ValueHeap>, process: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<bool> {
    let env = member(heap, process, "env", runtime)?;
    let flag = member(heap, &env, "PI_DEBUG_STARTUP", runtime)?;
    heap.truthy(&flag)
}

fn template(
    heap: &Arc<ValueHeap>,
    prefix: &str,
    value: &ValueHandle,
    suffix: &str,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let primitive = heap.to_primitive(value, true, runtime)?;
    if heap.kind(&primitive)? == ValueKind::Symbol {
        return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into()));
    }
    let text = heap.to_string(&primitive, runtime)?;
    let mut units = Vec::new();
    units.try_reserve(prefix.len() + text.units().len() + suffix.len()).map_err(|_| ValueError::Allocation)?;
    units.extend(prefix.encode_utf16());
    units.extend_from_slice(text.units());
    units.extend(suffix.encode_utf16());
    heap.string(WireString::from_units(units))
}

fn now(heap: &Arc<ValueHeap>, performance: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<ValueHandle> {
    call_member(heap, performance, "now", &[], runtime)
}

fn rest_arguments(heap: &Arc<ValueHeap>, arguments: &[ValueHandle]) -> Result<ValueHandle> {
    let tail = arguments.get(2..).unwrap_or(&[]);
    let mut values = Vec::new();
    values.try_reserve(tail.len()).map_err(|_| ValueError::Allocation)?;
    values.extend_from_slice(tail);
    heap.array(values)
}

fn spread_call(
    heap: &Arc<ValueHeap>,
    function: &ValueHandle,
    rest: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    // Both source fn(...args) sites use the real rest-array @@iterator. A
    // modified Array.prototype iterator remains observable; dense index reads
    // would silently skip that source behavior. Spread exhaustion has no early
    // normal exit and the source ArgumentListEvaluation adds no IteratorClose.
    let mut iterator = heap.get_iterator(rest, runtime)?;
    let mut arguments = Vec::new();
    while let Some(value) = heap.iterator_next(&mut iterator, runtime)? {
        arguments.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        arguments.push(value);
    }
    heap.call(function, &heap.undefined()?, &arguments, runtime)
}

fn object(heap: &Arc<ValueHeap>, fields: &[(&str, ValueHandle)], runtime: &dyn ValueRuntime) -> Result<ValueHandle> {
    let object = heap.object()?;
    for (key, value) in fields {
        // Object literal CreateDataProperty, never inherited setter assignment.
        heap.define_own_data(&object, (*key).into(), value.clone(), runtime)?;
    }
    Ok(object)
}

fn current_parent(
    heap: &Arc<ValueHeap>,
    cells: &ValueHandle,
    storage: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let active = call_member(heap, storage, "getStore", &[], runtime)?;
    if matches!(heap.kind(&active)?, ValueKind::Undefined | ValueKind::Null) {
        member(heap, cells, "root", runtime)
    } else {
        Ok(active)
    }
}

fn append_child(
    heap: &Arc<ValueHeap>,
    parent: &ValueHandle,
    span: ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    let children = member(heap, parent, "children", runtime)?;
    call_member(heap, &children, "push", &[span], runtime).map(|_| ())
}

static STARTUP_MARKER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da:logger.ts:365-374 startupMarker live env and synchronous stderr",
    name: "startupMarker",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 5,
    body: |heap, captures, _, arguments, runtime| {
        let errors = &captures[2..5];
        if !native(heap, errors, runtime, || debug_enabled(heap, &captures[0], runtime))? {
            return heap.undefined();
        }
        let result = native(heap, errors, runtime, || {
            // Call evaluates fs.writeSync member before fd/text arguments.
            let write = member(heap, &captures[1], "writeSync", runtime)?;
            let descriptor = heap.number(2.0)?;
            let text = template(heap, "[startup] ", &argument(heap, arguments, 0)?, "\n", runtime)?;
            heap.call(&write, &captures[1], &[descriptor, text], runtime)
        });
        match result {
            Ok(_) => heap.undefined(),
            Err(error) if error.is_native_exception() => heap.undefined(),
            Err(error) => Err(error),
        }
    },
};

static START_TIMING: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da:logger.ts:473-483 idempotent startTiming root graph",
    name: "startTiming",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 5,
    body: |heap, captures, _, _, runtime| {
        native(heap, &captures[2..5], runtime, || {
            if heap.truthy(&member(heap, &captures[0], "recording", runtime)?)? {
                return heap.undefined();
            }
            let op = heap.string("(root)".into())?;
            let start = now(heap, &captures[1], runtime)?;
            let root = object(
                heap,
                &[("op", op), ("start", start), ("parent", heap.undefined()?), ("children", heap.array(Vec::new())?)],
                runtime,
            )?;
            heap.define_own_data(&captures[0], "root".into(), root, runtime)?;
            heap.define_own_data(&captures[0], "recording".into(), heap.boolean(true)?, runtime)?;
            heap.undefined()
        })
    },
};

static RUN_THUNK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da:logger.ts:818 () => fn(...args)",
    name: "",
    length: 0,
    form: CompiledFunctionForm::Arrow,
    capture_count: 5,
    body: |heap, captures, _, _, runtime| {
        native(heap, &captures[2..5], runtime, || spread_call(heap, &captures[0], &captures[1], runtime))
    },
};

static FINISH: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da:logger.ts:813-816 finish original span.end and marker",
    name: "finish",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 7,
    body: |heap, captures, _, arguments, runtime| {
        native(heap, &captures[4..7], runtime, || {
            if heap.truthy(&captures[0])? {
                let end = now(heap, &captures[1], runtime)?;
                heap.assign(&captures[0], "end".into(), end, runtime)?;
            }
            let ok = heap.truthy(&argument(heap, arguments, 0)?)?;
            let marker = template(heap, "", &captures[3], if ok { ":done" } else { ":fail" }, runtime)?;
            heap.call(&captures[2], &heap.undefined()?, &[marker], runtime)?;
            heap.undefined()
        })
    },
};

static FULFILLED: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da:logger.ts:821-824 value => { finish(true); return value }",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 4,
    body: |heap, captures, _, arguments, runtime| {
        native(heap, &captures[1..4], runtime, || {
            heap.call(&captures[0], &heap.undefined()?, &[heap.boolean(true)?], runtime)?;
            argument(heap, arguments, 0)
        })
    },
};

static REJECTED: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da:logger.ts:825-828 error => { finish(false); throw error }",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 4,
    body: |heap, captures, _, arguments, runtime| {
        native(heap, &captures[1..4], runtime, || {
            heap.call(&captures[0], &heap.undefined()?, &[heap.boolean(false)?], runtime)?;
            Err(ValueError::Thrown(argument(heap, arguments, 0)?))
        })
    },
};

static TIME: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da:logger.ts:787-837 time original point/sync/native-Promise branches",
    name: "time",
    length: 2,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 9,
    body: |heap, captures, _, arguments, runtime| {
        let errors = &captures[6..9];
        let op = argument(heap, arguments, 0)?;
        let function = argument(heap, arguments, 1)?;
        let rest = rest_arguments(heap, arguments)?;
        let recording = heap.truthy(&member(heap, &captures[0], "recording", runtime)?)?
            && heap.kind(&member(heap, &captures[0], "root", runtime)?)? != ValueKind::Undefined;
        if heap.kind(&function)? == ValueKind::Undefined {
            return native(heap, errors, runtime, || {
                heap.call(&captures[4], &heap.undefined()?, &[op.clone()], runtime)?;
                if recording {
                    let parent = current_parent(heap, &captures[0], &captures[1], runtime)?;
                    let time = now(heap, &captures[2], runtime)?;
                    let point = object(
                        heap,
                        &[
                            ("op", op),
                            ("start", time.clone()),
                            ("end", time),
                            ("parent", parent.clone()),
                            ("children", heap.array(Vec::new())?),
                            ("point", heap.boolean(true)?),
                        ],
                        runtime,
                    )?;
                    append_child(heap, &parent, point, runtime)?;
                }
                heap.undefined()
            });
        }
        if !recording && !native(heap, errors, runtime, || debug_enabled(heap, &captures[3], runtime))? {
            return native(heap, errors, runtime, || spread_call(heap, &function, &rest, runtime));
        }
        let span = native(heap, errors, runtime, || {
            let marker = template(heap, "", &op, ":start", runtime)?;
            heap.call(&captures[4], &heap.undefined()?, &[marker], runtime)?;
            if !recording {
                return heap.undefined();
            }
            let parent = current_parent(heap, &captures[0], &captures[1], runtime)?;
            let start = now(heap, &captures[2], runtime)?;
            let span = object(
                heap,
                &[
                    ("op", op.clone()),
                    ("start", start),
                    ("parent", parent.clone()),
                    ("children", heap.array(Vec::new())?),
                ],
                runtime,
            )?;
            append_child(heap, &parent, span.clone(), runtime)?;
            Ok(span)
        })?;
        let finish = heap.fresh_compiled_function(
            &FINISH,
            &[
                span.clone(),
                captures[2].clone(),
                captures[4].clone(),
                op,
                errors[0].clone(),
                errors[1].clone(),
                errors[2].clone(),
            ],
        )?;
        let result = native(heap, errors, runtime, || {
            let result = if heap.truthy(&span)? {
                // Dynamic storage.run Get happens before allocating its arrow.
                let run = member(heap, &captures[1], "run", runtime)?;
                let thunk = heap.fresh_compiled_function(
                    &RUN_THUNK,
                    &[function.clone(), rest.clone(), errors[0].clone(), errors[1].clone(), errors[2].clone()],
                )?;
                heap.call(&run, &captures[1], &[span.clone(), thunk], runtime)?
            } else {
                spread_call(heap, &function, &rest, runtime)?
            };
            if heap.truthy(&heap.call(&captures[5], &heap.undefined()?, &[result.clone()], runtime)?)? {
                // Node-native brand check precedes this dynamic Get. A plain
                // thenable takes the sync finish branch without a then read.
                let then = member(heap, &result, "then", runtime)?;
                let handlers = [
                    heap.fresh_compiled_function(
                        &FULFILLED,
                        &[finish.clone(), errors[0].clone(), errors[1].clone(), errors[2].clone()],
                    )?,
                    heap.fresh_compiled_function(
                        &REJECTED,
                        &[finish.clone(), errors[0].clone(), errors[1].clone(), errors[2].clone()],
                    )?,
                ];
                return heap.call(&then, &result, &handlers, runtime);
            }
            heap.call(&finish, &heap.undefined()?, &[heap.boolean(true)?], runtime)?;
            Ok(result)
        });
        match result {
            Ok(value) => Ok(value),
            Err(error) if error.is_native_exception() => {
                // Preserve source exception precedence: a failing finish(false)
                // replaces the original error; it is not a Compound carrier.
                heap.call(&finish, &heap.undefined()?, &[heap.boolean(false)?], runtime)?;
                Err(error)
            }
            Err(error) => Err(error),
        }
    },
};
