//! Fixed OMP pi-utils sanitize-text.ts:23-39 at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; THIRD_PARTY_NOTICES.md).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc.
//! Host supplies its actual Bun object; Core never invents a global environment.
use super::*;
use ara_rpc::value_heap::{CompiledFunctionForm, CompiledFunctionSpec, ValueHeap, ValueRuntime, ValueSnapshot};

#[derive(Clone, Debug)]
pub struct ActualToolResultSanitizer {
    function: ValueHandle,
}
impl ActualToolResultSanitizer {
    pub fn new(bun: ValueHandle, values: &SessionValues) -> Result<Self, ActualProviderError> {
        values.check_handle(&bun)?;
        let heap = values.heap();
        let control =
            heap.module_regexp("ara-agent.sanitize-text", "CONTROL_RE", r"[\x00-\x08\x0B-\x1F\x7F-\x9F]", "g")?;
        let well_formed = heap.fresh_compiled_function(&SANITIZE_WELL_FORMED, &[control, bun])?;
        let function = heap.fresh_compiled_function(&SANITIZE, &[well_formed])?;
        Ok(Self { function })
    }
    pub fn function(&self) -> &ValueHandle {
        &self.function
    }
    pub fn sanitize(&self, text: &ValueHandle, values: &SessionValues) -> Result<ValueHandle, ActualProviderError> {
        values.check_handle(&self.function)?;
        values.check_handle(text)?;
        Ok(values.heap().call(&self.function, &values.heap().undefined()?, &[text.clone()], values.runtime())?)
    }
}
fn method(
    heap: &Arc<ValueHeap>,
    receiver: &ValueHandle,
    name: &str,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let function = heap.get(receiver, &name.into(), runtime)?;
    heap.call(&function, receiver, arguments, runtime)
}
static SANITIZE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:packages/utils/src/sanitize-text.ts:23-29",
    name: "sanitizeText",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: sanitize,
};
static SANITIZE_WELL_FORMED: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:packages/utils/src/sanitize-text.ts:31-39",
    name: "sanitizeWellFormedText",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 2,
    body: sanitize_well_formed,
};
fn sanitize(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let text = arguments.first().cloned().unwrap_or(heap.undefined()?);
    let well_formed = method(heap, &text, "toWellFormed", &[], runtime)?;
    let output = if heap.object_is(&well_formed, &text)? {
        text
    } else {
        method(heap, &well_formed, "replaceAll", &[heap.string("\u{fffd}".into())?, heap.string("".into())?], runtime)?
    };
    heap.call(&captures[0], &heap.undefined()?, &[output], runtime)
}
fn sanitize_well_formed(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let text = arguments.first().cloned().unwrap_or(heap.undefined()?);
    let control = &captures[0];
    let bun = &captures[1];
    heap.assign(control, "lastIndex".into(), heap.number(0.0)?, runtime)?;
    let probe = method(heap, control, "exec", &[text.clone()], runtime)?;
    if matches!(heap.snapshot(&probe)?, ValueSnapshot::Null) {
        return Ok(text);
    }
    let position = method(heap, &text, "indexOf", &[heap.string("\x1b".into())?], runtime)?;
    let stripped = if matches!(heap.snapshot(&position)?, ValueSnapshot::Number(value) if value == -1.0) {
        text
    } else {
        method(heap, bun, "stripANSI", &[text], runtime)?
    };
    heap.assign(control, "lastIndex".into(), heap.number(0.0)?, runtime)?;
    method(heap, &stripped, "replace", &[control.clone(), heap.string("".into())?], runtime)
}
