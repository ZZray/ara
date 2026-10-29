//! Fixed OMP596f2da utils/retry-after.ts executable Rust bodies (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! Original Headers/Number/Math/Date/Object/JSON imports retain their receivers.
use crate::graph_values::{GraphExecution, GraphValueFactory};
use crate::schema_validator::graph_values_compare_with_execution;
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, IteratorRecord, ValueError, ValueHandle,
    ValueHeap, ValueKind, ValueRuntime,
};
use std::sync::{Arc, Weak};

type Result<T> = std::result::Result<T, ValueError>;
pub struct RetryAfterModule {
    factory: Weak<GraphValueFactory>,
    pub get_retry_after_ms_from_headers: ValueHandle,
    pub get_headers_from_error: ValueHandle,
    pub format_error_message_with_retry_after: ValueHandle,
}
impl RetryAfterModule {
    pub fn prepare(factory: &Arc<GraphValueFactory>, globals: ValueHandle) -> Result<Self> {
        factory.heap().adopt(&globals)?;
        let captures = [globals];
        Ok(Self {
            factory: Arc::downgrade(factory),
            get_retry_after_ms_from_headers: factory.heap().fresh_compiled_function(&GET_WAIT, &captures)?,
            get_headers_from_error: factory.heap().fresh_compiled_function(&GET_HEADERS, &captures)?,
            format_error_message_with_retry_after: factory.heap().fresh_compiled_function(&FORMAT_ERROR, &captures)?,
        })
    }
    pub fn check_factory(&self, factory: &Arc<GraphValueFactory>) -> Result<()> {
        let original = self.factory.upgrade().ok_or(ValueError::UnsupportedOperation("Retry-after imports expired"))?;
        if !Arc::ptr_eq(&original, factory) {
            return Err(ValueError::UnsupportedOperation("Retry-after requires original Session factory"));
        }
        Ok(())
    }
    pub fn invoke(
        &self, factory: &Arc<GraphValueFactory>, function: &ValueHandle,
        arguments: &[ValueHandle], effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.check_factory(factory)?;
        factory.heap().call(function, &factory.heap().undefined()?, arguments, effective)
    }
}

#[derive(Clone, Copy)]
struct Ex<'a> { heap: &'a Arc<ValueHeap>, globals: &'a ValueHandle, runtime: &'a dyn ValueRuntime }
impl Ex<'_> {
    fn get(self, target: &ValueHandle, name: &str) -> Result<ValueHandle> {
        self.heap.get(target, &name.into(), self.runtime)
    }
    fn method(self, target: &ValueHandle, name: &str, arguments: &[ValueHandle]) -> Result<ValueHandle> {
        let method = self.get(target, name)?;
        self.heap.call(&method, target, arguments, self.runtime)
    }
    fn global(self, name: &str) -> Result<ValueHandle> { self.get(self.globals, name) }
    fn math(self, method: &str, values: &[ValueHandle]) -> Result<ValueHandle> {
        self.method(&self.global("Math")?, method, values)
    }
    fn finite(self, value: &ValueHandle) -> Result<bool> {
        self.heap.truthy(&self.method(&self.global("Number")?, "isFinite", std::slice::from_ref(value))?)
    }
    fn compare(self, value: &ValueHandle, number: f64, op: &str) -> Result<bool> {
        graph_values_compare_with_execution(GraphExecution::new(self.heap, self.runtime), value, &self.heap.number(number)?, op)
    }
    fn undefined(self, value: &ValueHandle) -> Result<bool> { Ok(self.heap.kind(value)? == ValueKind::Undefined) }
    fn nullish(self, value: &ValueHandle) -> Result<bool> {
        Ok(matches!(self.heap.kind(value)?, ValueKind::Null | ValueKind::Undefined))
    }
    fn object(self, value: &ValueHandle) -> Result<bool> {
        Ok(matches!(self.heap.kind(value)?, ValueKind::Null | ValueKind::Object | ValueKind::Array))
    }
    fn string(self, text: &str) -> Result<ValueHandle> { self.heap.string(text.into()) }
    fn number_from_trim(self, value: &ValueHandle) -> Result<ValueHandle> {
        let number = self.global("Number")?;
        let trimmed = self.method(value, "trim", &[])?;
        self.heap.call(&number, &self.heap.undefined()?, &[trimmed], self.runtime)
    }
    fn now(self) -> Result<f64> {
        let now = self.method(&self.global("Date")?, "now", &[])?;
        self.heap.to_number(&now, self.runtime)
    }
}

fn close_error(ex: Ex<'_>, iterator: &mut IteratorRecord, original: ValueError) -> ValueError {
    match ex.heap.iterator_close(iterator, ex.runtime) {
        Ok(()) => original,
        Err(close) if original.is_native_exception() && close.is_native_exception() => original,
        Err(close) => ValueError::IteratorClose { original: Box::new(original), close: Box::new(close) },
    }
}
fn pair(ex: Ex<'_>, entry: &ValueHandle) -> Result<(ValueHandle, ValueHandle)> {
    let mut iterator = ex.heap.get_iterator(entry, ex.runtime)?;
    let result = (|| {
        let key = ex.heap.iterator_next(&mut iterator, ex.runtime)?.unwrap_or(ex.heap.undefined()?);
        let value = ex.heap.iterator_next(&mut iterator, ex.runtime)?.unwrap_or(ex.heap.undefined()?);
        Ok((key, value))
    })();
    match result {
        Ok(pair) => { ex.heap.iterator_close(&mut iterator, ex.runtime)?; Ok(pair) }
        Err(error) => Err(close_error(ex, &mut iterator, error)),
    }
}
fn header_value(ex: Ex<'_>, headers: &ValueHandle, name: &str) -> Result<ValueHandle> {
    if ex.heap.instance_of(headers, &ex.global("Headers")?, ex.runtime)? {
        let value = ex.method(headers, "get", &[ex.string(name)?])?;
        return if ex.nullish(&value)? { ex.heap.undefined() } else { Ok(value) };
    }
    let target = ex.method(&ex.string(name)?, "toLowerCase", &[])?;
    let entries = ex.method(&ex.global("Object")?, "entries", std::slice::from_ref(headers))?;
    let mut iterator = ex.heap.get_iterator(&entries, ex.runtime)?;
    while let Some(entry) = ex.heap.iterator_next(&mut iterator, ex.runtime)? {
        let result = (|| {
            let (key, value) = pair(ex, &entry)?;
            let lower = ex.method(&key, "toLowerCase", &[])?;
            Ok(if ex.heap.object_is(&lower, &target)? && ex.heap.kind(&value)? == ValueKind::String { Some(value) } else { None })
        })();
        match result {
            Ok(Some(value)) => { ex.heap.iterator_close(&mut iterator, ex.runtime)?; return Ok(value); }
            Ok(None) => {}
            Err(error) => return Err(close_error(ex, &mut iterator, error)),
        }
    }
    ex.heap.undefined()
}
fn parse_ms(ex: Ex<'_>, value: &ValueHandle) -> Result<ValueHandle> {
    if !ex.heap.truthy(value)? { return ex.heap.undefined(); }
    let ms = ex.number_from_trim(value)?;
    if !ex.finite(&ms)? || ex.compare(&ms, 0.0, "<=")? { return ex.heap.undefined(); }
    ex.math("ceil", &[ms])
}
fn parse_seconds(ex: Ex<'_>, value: &ValueHandle) -> Result<ValueHandle> {
    if !ex.heap.truthy(value)? { return ex.heap.undefined(); }
    let trimmed = ex.method(value, "trim", &[])?;
    if !ex.heap.truthy(&trimmed)? { return ex.heap.undefined(); }
    let numeric = ex.heap.call(&ex.global("Number")?, &ex.heap.undefined()?, std::slice::from_ref(&trimmed), ex.runtime)?;
    if ex.finite(&numeric)? {
        if ex.compare(&numeric, 0.0, "<=")? { return ex.heap.undefined(); }
        let math = ex.global("Math")?;
        let ceil = ex.get(&math, "ceil")?;
        let delay = ex.heap.to_number(&numeric, ex.runtime)? * 1000.0;
        return ex.heap.call(&ceil, &math, &[ex.heap.number(delay)?], ex.runtime);
    }
    let date = ex.method(&ex.global("Date")?, "parse", &[trimmed])?;
    if !ex.heap.truthy(&ex.method(&ex.global("Number")?, "isNaN", std::slice::from_ref(&date))?)? {
        let delay = ex.heap.to_number(&date, ex.runtime)? - ex.now()?;
        if delay > 0.0 { return ex.math("ceil", &[ex.heap.number(delay)?]); }
    }
    ex.heap.undefined()
}
fn parse_reset(ex: Ex<'_>, value: &ValueHandle, seconds: bool) -> Result<ValueHandle> {
    if !ex.heap.truthy(value)? { return ex.heap.undefined(); }
    let numeric = ex.heap.call(&ex.global("Number")?, &ex.heap.undefined()?, std::slice::from_ref(value), ex.runtime)?;
    if !ex.finite(&numeric)? || ex.compare(&numeric, 0.0, "<=")? { return ex.heap.undefined(); }
    let now = ex.now()?;
    let target = if ex.compare(&numeric, 1e12, ">")? {
        ex.heap.to_number(&numeric, ex.runtime)?
    } else if ex.compare(&numeric, 1e9, ">")? {
        ex.heap.to_number(&numeric, ex.runtime)? * 1000.0
    } else {
        return ex.math("ceil", &[if seconds { ex.heap.number(ex.heap.to_number(&numeric, ex.runtime)? * 1000.0)? } else { numeric }]);
    };
    if target <= now { return ex.heap.undefined(); }
    ex.math("ceil", &[ex.heap.number(target - now)?])
}
fn wait(ex: Ex<'_>, headers: &ValueHandle) -> Result<ValueHandle> {
    if !ex.heap.truthy(headers)? { return ex.heap.undefined(); }
    let candidates = ex.heap.array(vec![
        parse_ms(ex, &header_value(ex, headers, "retry-after-ms")?)?,
        parse_seconds(ex, &header_value(ex, headers, "retry-after")?)?,
        parse_reset(ex, &header_value(ex, headers, "x-ratelimit-reset-ms")?, false)?,
        parse_reset(ex, &header_value(ex, headers, "x-ratelimit-reset")?, true)?,
    ])?;
    let filter = ex.get(&candidates, "filter")?;
    let callback = ex.heap.fresh_compiled_function(&DEFINED, &[])?;
    let candidates = ex.heap.call(&filter, &candidates, &[callback], ex.runtime)?;
    if ex.heap.object_is(&ex.get(&candidates, "length")?, &ex.heap.number(0.0)?)? { return ex.heap.undefined(); }
    let math = ex.global("Math")?;
    let maximum = ex.get(&math, "max")?;
    let mut iterator = ex.heap.get_iterator(&candidates, ex.runtime)?;
    let mut supplied = Vec::new();
    while let Some(value) = ex.heap.iterator_next(&mut iterator, ex.runtime)? { supplied.push(value); }
    ex.heap.call(&maximum, &math, &supplied, ex.runtime)
}
fn extract_headers(ex: Ex<'_>, value: ValueHandle) -> Result<ValueHandle> {
    if !ex.heap.truthy(&value)? { return ex.heap.undefined(); }
    if ex.heap.instance_of(&value, &ex.global("Headers")?, ex.runtime)? || ex.object(&value)? { Ok(value) } else { ex.heap.undefined() }
}
fn headers(ex: Ex<'_>, error: &ValueHandle) -> Result<ValueHandle> {
    if !ex.heap.truthy(error)? || !ex.object(error)? { return ex.heap.undefined(); }
    let mut direct = extract_headers(ex, ex.get(error, "headers")?)?;
    if ex.nullish(&direct)? {
        let response = ex.get(error, "response")?;
        let raw = if ex.nullish(&response)? { ex.heap.undefined()? } else { ex.get(&response, "headers")? };
        direct = extract_headers(ex, raw)?;
    }
    if ex.heap.truthy(&direct)? { return Ok(direct); }
    if ex.heap.truthy(&ex.get(error, "cause")?)? { return headers(ex, &ex.get(error, "cause")?); }
    ex.heap.undefined()
}

static DEFINED: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da retry-after.ts:33 value => value !== undefined",
    name: "", length: 1, capture_count: 0, form: CompiledFunctionForm::Arrow,
    body: |heap, _, _, args, _| heap.boolean(heap.kind(&args.first().cloned().unwrap_or(heap.undefined()?))? != ValueKind::Undefined),
};
static GET_WAIT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da retry-after.ts:24-37 getRetryAfterMsFromHeaders",
    name: "getRetryAfterMsFromHeaders", length: 1, capture_count: 1, form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, _, args, runtime| wait(Ex { heap, globals: &captures[0], runtime }, &args.first().cloned().unwrap_or(heap.undefined()?)),
};
static GET_HEADERS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da retry-after.ts:40-47 getHeadersFromError",
    name: "getHeadersFromError", length: 1, capture_count: 1, form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, _, args, runtime| headers(Ex { heap, globals: &captures[0], runtime }, &args.first().cloned().unwrap_or(heap.undefined()?)),
};
static FORMAT_ERROR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da retry-after.ts:5-21 formatErrorMessageWithRetryAfter",
    name: "formatErrorMessageWithRetryAfter", length: 2, capture_count: 1, form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, _, args, runtime| {
        let ex = Ex { heap, globals: &captures[0], runtime };
        let error = args.first().cloned().unwrap_or(heap.undefined()?);
        let message = if heap.instance_of(&error, &ex.global("Error")?, runtime)? {
            ex.get(&error, "message")?
        } else { ex.method(&ex.global("JSON")?, "stringify", std::slice::from_ref(&error))? };
        if heap.truthy(&ex.method(&message, "includes", &[ex.string("retry-after-ms=")?])?)? { return Ok(message); }
        let supplied = args.get(1).cloned().unwrap_or(heap.undefined()?);
        let supplied = if ex.nullish(&supplied)? { headers(ex, &error)? } else { supplied };
        let retry = wait(ex, &supplied)?;
        if ex.undefined(&retry)? { return Ok(message); }
        let mut units = heap.to_string(&message, runtime)?.units().to_vec();
        units.extend_from_slice(ara_rpc::WireString::from(" retry-after-ms=").units());
        units.extend_from_slice(heap.to_string(&retry, runtime)?.units());
        heap.string(ara_rpc::WireString::from_units(units))
    },
};
