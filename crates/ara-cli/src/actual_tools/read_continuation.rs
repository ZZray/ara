//! Fixed OMP596f2da read.ts:612-648,1225-1226, compiled graph continuation.
//! MIT; Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! 2026 Stencil Labs, Inc. See THIRD_PARTY.md.
//! The caller owns the genuine async await and authoritative result adoption.

use crate::native_realm::NativeRealm;
use ara_agent::SessionValues;
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, DeferredValue, ValueError, ValueHandle, ValueHeap, ValueKind,
        ValueRuntime, ValueSnapshot,
    },
};
use std::{cmp::Ordering, sync::Arc};

type Result<T> = std::result::Result<T, ValueError>;
const MODULE: &str = "OMP596f2da:packages/coding-agent/src/tools/read.ts";
const TRACKER: &str = "read.repeatTracker";

/// One source module binding in the original Session realm. Captured compiled
/// functions store heap edges; they never retain a runtime or Run lease.
pub struct NativeReadContinuation {
    helper: ValueHandle,
    errors: [ValueHandle; 3],
    delimited: [ValueHandle; 3],
}

impl NativeReadContinuation {
    pub fn prepare(realm: &NativeRealm) -> Result<Self> {
        let heap = realm.values.heap();
        // Preserve module initialization, not a new private Symbol per call.
        heap.module_symbol(MODULE, TRACKER)?;
        let errors = [
            realm.globals.errors.type_error.clone(),
            realm.globals.errors.syntax_error.clone(),
            realm.globals.errors.range_error.clone(),
        ];
        let helper = heap.fresh_compiled_function(
            &APPEND_REPEAT_HINT,
            &[realm.globals.environment.clone(), errors[0].clone(), errors[1].clone(), errors[2].clone()],
        )?;
        let delimited = [realm.globals.environment.clone(), realm.globals.errors.error.clone(), realm.tool_error_bindings()?.tool_abort_error];
        Ok(Self { helper, errors, delimited })
    }

    pub(crate) fn capture(&self) -> Result<NativeReadContinuationEdges> {
        let heap = self.helper.heap();
        Ok(NativeReadContinuationEdges {
            helper: heap.capture_deferred(&self.helper)?,
            delimited: [heap.capture_deferred(&self.delimited[0])?, heap.capture_deferred(&self.delimited[1])?, heap.capture_deferred(&self.delimited[2])?],
            errors: [
                heap.capture_deferred(&self.errors[0])?,
                heap.capture_deferred(&self.errors[1])?,
                heap.capture_deferred(&self.errors[2])?,
            ],
        })
    }

    pub fn helper(&self) -> &ValueHandle {
        &self.helper
    }

    /// Captures receiver and params edges, never their current properties.
    /// A real await consumer must call this body under its original invocation
    /// parent and saved inner scope, then adopt its result into the public slot.
    pub fn after_inner(&self, receiver: &ValueHandle, params: &ValueHandle) -> Result<ValueHandle> {
        self.helper.heap().fresh_compiled_function(
            &AFTER_INNER,
            &[
                receiver.clone(),
                params.clone(),
                self.helper.clone(),
                self.errors[0].clone(),
                self.errors[1].clone(),
                self.errors[2].clone(),
            ],
        )
    }
}

/// Queue-safe source affiliation. Restore only in the original driver realm.
pub(crate) struct NativeReadContinuationEdges {
    helper: DeferredValue,
    errors: [DeferredValue; 3],
    delimited: [DeferredValue; 3],
}
impl NativeReadContinuationEdges {
    pub(super) fn delimited_bindings(&self, values: &SessionValues) -> Result<super::read_delimited::Bindings> {
        let heap = values.heap();
        Ok(super::read_delimited::Bindings {
            environment: heap.restore_deferred(&self.delimited[0])?,
            error: heap.restore_deferred(&self.delimited[1])?,
            abort: heap.restore_deferred(&self.delimited[2])?,
            native_errors: [heap.restore_deferred(&self.errors[0])?, heap.restore_deferred(&self.errors[1])?, heap.restore_deferred(&self.errors[2])?],
        })
    }

    /// The original Session realm TypeError binding also serves source-native
    /// Bash numeric conversion. This never inspects or replaces global members.
    pub(crate) fn native_type_error(
        &self,
        values: &SessionValues,
        message: &str,
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        let heap = values.heap();
        let constructor = heap.restore_deferred(&self.errors[0])?;
        heap.call(&constructor, &heap.undefined()?, &[heap.string(message.into())?], runtime)
    }

    pub(crate) fn after_inner(
        &self,
        values: &SessionValues,
        receiver: &ValueHandle,
        params: &ValueHandle,
    ) -> Result<ValueHandle> {
        let heap = values.heap();
        heap.fresh_compiled_function(
            &AFTER_INNER,
            &[
                receiver.clone(),
                params.clone(),
                heap.restore_deferred(&self.helper)?,
                heap.restore_deferred(&self.errors[0])?,
                heap.restore_deferred(&self.errors[1])?,
                heap.restore_deferred(&self.errors[2])?,
            ],
        )
    }
}

fn argument(heap: &Arc<ValueHeap>, args: &[ValueHandle], index: usize) -> Result<ValueHandle> {
    args.get(index).cloned().map(Ok).unwrap_or_else(|| heap.undefined())
}

/// At this native algorithm origin only. Original Thrown/infra carriers keep
/// their identities; converting a native TypeError uses this realm's original
/// captured constructor, without inspecting Display text or global overrides.
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

fn strict_equal(heap: &Arc<ValueHeap>, left: &ValueHandle, right: &ValueHandle) -> Result<bool> {
    match (heap.snapshot(left)?, heap.snapshot(right)?) {
        // SameValue's NaN/signed-zero rules do not implement ===.
        (ValueSnapshot::Number(a), ValueSnapshot::Number(b)) => Ok(a == b),
        _ => heap.object_is(left, right),
    }
}

fn nullish(heap: &Arc<ValueHeap>, value: &ValueHandle) -> Result<bool> {
    Ok(matches!(heap.kind(value)?, ValueKind::Undefined | ValueKind::Null))
}

/// The two original relational operands are positive integer Number literals.
/// Keep BigInt comparison exact and NaN unordered; do not round a BigInt to f64.
fn compare_positive(
    heap: &Arc<ValueHeap>,
    value: &ValueHandle,
    limit: u32,
    runtime: &dyn ValueRuntime,
) -> Result<Option<Ordering>> {
    let primitive = heap.to_primitive(value, false, runtime)?;
    if let ValueSnapshot::BigInt(value) = heap.snapshot(&primitive)? {
        let digits = value.as_str();
        if digits.starts_with('-') {
            return Ok(Some(Ordering::Less));
        }
        let limit = limit.to_string();
        return Ok(Some(digits.len().cmp(&limit.len()).then_with(|| digits.cmp(limit.as_str()))));
    }
    Ok(heap.to_number(&primitive, runtime)?.partial_cmp(&f64::from(limit)))
}

fn implicit_string(heap: &Arc<ValueHeap>, value: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<WireString> {
    let primitive = heap.to_primitive(value, true, runtime)?;
    if heap.kind(&primitive)? == ValueKind::Symbol {
        return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into()));
    }
    heap.to_string(&primitive, runtime)
}

static FIND_FIRST_TEXT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da:read.ts:631 entry => entry.type === text",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, args, runtime| {
        let entry = argument(heap, args, 0)?;
        let kind = heap.get(&entry, &"type".into(), runtime)?;
        heap.boolean(matches!(heap.snapshot(&kind)?, ValueSnapshot::String(text) if text.equals_ascii("text")))
    },
};

static APPEND_REPEAT_HINT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da:packages/coding-agent/src/tools/read.ts:630-648",
    name: "appendRepeatReadHint",
    length: 3,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 4,
    body: |heap, captures, _, args, runtime| {
        native(heap, &captures[1..], runtime, || {
            let session = argument(heap, args, 0)?;
            let path = argument(heap, args, 1)?;
            let result = argument(heap, args, 2)?;
            let content = heap.get(&result, &"content".into(), runtime)?;
            let block = if nullish(heap, &content)? {
                heap.undefined()?
            } else {
                let find = heap.get(&content, &"find".into(), runtime)?;
                let predicate = heap.fresh_compiled_function(&FIND_FIRST_TEXT, &[])?;
                heap.call(&find, &content, &[predicate], runtime)?
            };
            if !heap.truthy(&block)? {
                return heap.undefined();
            }
            if heap.kind(&heap.get(&block, &"text".into(), runtime)?)? != ValueKind::String {
                return heap.undefined();
            }
            let current_text = heap.get(&block, &"text".into(), runtime)?;
            let length = heap.get(&current_text, &"length".into(), runtime)?;
            if matches!(heap.snapshot(&length)?, ValueSnapshot::Number(n) if n == 0.0) {
                return heap.undefined();
            }
            if heap.truthy(&heap.get(&result, &"isError".into(), runtime)?)? {
                return heap.undefined();
            }

            let key = heap.module_symbol(MODULE, TRACKER)?;
            let current = heap.get(&session, &key, runtime)?;
            if nullish(heap, &current)? {
                let map = heap.get(&captures[0], &"Map".into(), runtime)?;
                let tracker = heap.construct(&map, &[], runtime)?;
                heap.assign(&session, key.clone(), tracker, runtime)?;
            }
            // The source does a fresh Get even when ??= just assigned the map.
            let tracker = heap.get(&session, &key, runtime)?;
            let size = heap.get(&tracker, &"size".into(), runtime)?;
            if compare_positive(heap, &size, 64, runtime)? == Some(Ordering::Greater) {
                let clear = heap.get(&tracker, &"clear".into(), runtime)?;
                heap.call(&clear, &tracker, &[], runtime)?;
            }
            let bun = heap.get(&captures[0], &"Bun".into(), runtime)?;
            let hash_object = heap.get(&bun, &"hash".into(), runtime)?;
            let hash_method = heap.get(&hash_object, &"xxHash64".into(), runtime)?;
            let text = heap.get(&block, &"text".into(), runtime)?;
            let hash = heap.call(&hash_method, &hash_object, &[text], runtime)?;
            let get = heap.get(&tracker, &"get".into(), runtime)?;
            let entry = heap.call(&get, &tracker, &[path.clone()], runtime)?;
            if !heap.truthy(&entry)? || !strict_equal(heap, &heap.get(&entry, &"hash".into(), runtime)?, &hash)? {
                let set = heap.get(&tracker, &"set".into(), runtime)?;
                let record = heap.object()?;
                heap.define_own_data(&record, "hash".into(), hash, runtime)?;
                heap.define_own_data(&record, "count".into(), heap.number(1.0)?, runtime)?;
                heap.call(&set, &tracker, &[path, record], runtime)?;
                return heap.undefined();
            }
            let count = heap.get(&entry, &"count".into(), runtime)?;
            let incremented = heap.increment(&count, runtime)?;
            heap.assign(&entry, "count".into(), incremented, runtime)?;
            let count = heap.get(&entry, &"count".into(), runtime)?;
            if compare_positive(heap, &count, 3, runtime)? == Some(Ordering::Less) {
                return heap.undefined();
            }

            // += reads its left value before the template's count/path conversions.
            let text = heap.get(&block, &"text".into(), runtime)?;
            let count = heap.get(&entry, &"count".into(), runtime)?;
            let count = implicit_string(heap, &count, runtime)?;
            let path = implicit_string(heap, &path, runtime)?;
            let mut hint = "\n\n[You have received this identical output ".encode_utf16().collect::<Vec<_>>();
            hint.extend_from_slice(count.units());
            hint.extend(" times. Re-reading '".encode_utf16());
            hint.extend_from_slice(path.units());
            hint.extend(
                "' will not change it — use a narrower selector (path:A-B), or proceed with the edit.]".encode_utf16(),
            );
            let hint = heap.string(WireString::from_units(hint))?;
            let combined = heap.add(&text, &hint, runtime)?;
            heap.assign(&block, "text".into(), combined, runtime)?;
            heap.undefined()
        })
    },
};

static AFTER_INNER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da:packages/coding-agent/src/tools/read.ts:1225-1226",
    name: "readAfterExecuteInner",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 6,
    body: |heap, captures, _, args, runtime| {
        native(heap, &captures[3..], runtime, || {
            let result = argument(heap, args, 0)?;
            let session = heap.get(&captures[0], &"session".into(), runtime)?;
            let path = heap.get(&captures[1], &"path".into(), runtime)?;
            heap.call(&captures[2], &heap.undefined()?, &[session, path, result.clone()], runtime)?;
            Ok(result)
        })
    },
};
