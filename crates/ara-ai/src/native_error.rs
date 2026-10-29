//! Rust function bodies for fixed OMP flags.ts::classifyMessage/isContextOverflow.
//! 596f2da7101178214aa27a753529d15e6b7ad91d; MIT, THIRD_PARTY_NOTICES.md.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc.
//!
//! Host prepares one owner per logical Session realm, before publication, and
//! retains/clones that owner on continuation. Heap captures contain NodeIds;
//! this owner never captures a runtime, SessionValues, sink or strong factory.
//! Every body and nested callback borrows the *calling* effective runtime.

use crate::graph_values::{GraphExecution, GraphValueFactory};
use crate::schema_validator::graph_values_compare_with_execution;
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, IteratorRecord, ValueError, ValueHandle, ValueHeap, ValueKind,
        ValueRuntime, ValueSnapshot,
    },
};
use std::sync::{Arc, Weak};

mod patterns;
mod rate_limit;
mod retry;
pub use retry::{NativeRetryClassifierImports, NativeRetryErrorNamespace};
type Result<T> = std::result::Result<T, ValueError>;
const MODULE: &str = "ara-ai.omp596f2da.error.flags";
const CLASS: u32 = 0x1000;
const TRANSIENT: u32 = 0x0002_0000;
const TIMEOUT: u32 = 0x0004_0000;
const USAGE_LIMIT: u32 = 0x0008_0000;
const STALE: u32 = 0x0010_0000;
const MALFORMED: u32 = 0x0020_0000;
const PROVIDER_FINISH: u32 = 0x0040_0000;
const EMPTY: u32 = 0x2000;
const CONTENT_BLOCKED: u32 = 0x8000;
const ACCOUNT_POLICY: u32 = 0x4000;
const OVERFLOW: u32 = 0x0080_0000;
const AUTH_FAILED: u32 = 0x0100_0000;
const GRAMMAR: u32 = 0x1000_0000;
const FAST_MODE: u32 = 0x2000_0000;
const PAYLOAD: u32 = 0x8000_0000;
const KIND_MASK: u32 = 0xffff_e000;
const TOKEN_PATTERNS: usize = 2;
const OVERFLOW_PATTERNS: usize = 3;
const PAYLOAD_PATTERNS: usize = 4;
const STATUS_PATTERNS: usize = 5;
const STALE_PATTERNS: usize = 6;

/// `globals` is the original Host realm's global binding environment. Its
/// Error/JSON/Array/parseInt reads and all selected methods remain dynamic.
/// The imported utility is the original module Function, not a scalar reader.
#[derive(Clone)]
pub struct NativeErrorImports {
    pub globals: ValueHandle,
    pub extract_retry_hint: Option<ValueHandle>,
}

#[derive(Clone)]
pub struct NativeErrorFunctions {
    pub receiver: ValueHandle,
    pub classify_message: ValueHandle,
    pub is_context_overflow: ValueHandle,
}

/// Stable functions of one evaluated fixed-source module in this Host realm.
/// `prepare` is a two-phase Host bootstrap operation, not a per-call factory.
pub struct NativeErrorFunctionOwner {
    factory: Weak<GraphValueFactory>,
    functions: NativeErrorFunctions,
    captures: [ValueHandle; 7],
}
impl NativeErrorFunctionOwner {
    pub fn prepare(factory: &Arc<GraphValueFactory>, imports: NativeErrorImports) -> Result<Self> {
        let heap = factory.heap();
        heap.adopt(&imports.globals)?;
        if let Some(imported) = &imports.extract_retry_hint {
            heap.adopt(imported)?;
            if heap.kind(imported)? != ValueKind::Function {
                return Err(ValueError::TypeError("extractRetryHint import must be callable".into()));
            }
        }
        for &(name, pattern, flags) in patterns::REGEXPS {
            heap.module_regexp(MODULE, name, pattern, flags)?;
        }
        let make_array = |names: &[&'static str]| -> Result<ValueHandle> {
            heap.array(
                names
                    .iter()
                    .map(|name| patterns::regexp(Execution { heap, runtime: factory.runtime(), captures: &[] }, name))
                    .collect::<Result<Vec<_>>>()?,
            )
        };
        let tokens = make_array(patterns::CONTEXT_OVERFLOW_EVIDENCE_PATTERNS)?;
        // Fixed OVERFLOW_PATTERNS = [...tokens, GENERIC_LIMIT_OVERFLOW_PATTERN].
        // Array spread selects the actual iterator; no Array.push replacement.
        let ex = Execution { heap, runtime: factory.runtime(), captures: &[] };
        let mut iterator = heap.get_iterator(&tokens, ex.runtime)?;
        let mut overflow = Vec::new();
        while let Some(item) = heap.iterator_next(&mut iterator, ex.runtime)? {
            overflow.try_reserve(1).map_err(|_| ValueError::Allocation)?;
            overflow.push(item);
        }
        overflow.push(patterns::regexp(ex, "GENERIC_LIMIT_OVERFLOW_PATTERN")?);
        let captures = [
            imports.globals,
            imports.extract_retry_hint.unwrap_or(heap.undefined()?),
            tokens,
            heap.array(overflow)?,
            make_array(patterns::PAYLOAD_REJECTION_PATTERNS)?,
            make_array(patterns::STATUS_MESSAGE_PATTERNS)?,
            make_array(patterns::STALE_RESPONSE_ITEM_PATTERNS)?,
        ];
        let functions = NativeErrorFunctions {
            receiver: heap.undefined()?,
            classify_message: heap.fresh_compiled_function(&CLASSIFY_MESSAGE, &captures)?,
            is_context_overflow: heap.fresh_compiled_function(&IS_CONTEXT_OVERFLOW, &captures)?,
        };
        Ok(Self { factory: Arc::downgrade(factory), functions, captures })
    }

    pub fn check_factory(&self, factory: &Arc<GraphValueFactory>) -> Result<()> {
        let original =
            self.factory.upgrade().ok_or(ValueError::UnsupportedOperation("Native error realm owner expired"))?;
        if !Arc::ptr_eq(&original, factory) {
            return Err(ValueError::UnsupportedOperation("Native error binding requires the original Session factory"));
        }
        factory.heap().adopt(&self.functions.classify_message)?;
        Ok(())
    }

    pub fn functions(&self, factory: &Arc<GraphValueFactory>) -> Result<NativeErrorFunctions> {
        self.check_factory(factory)?;
        Ok(self.functions.clone())
    }

    /// Fixed turn-recovery.ts noteRetryFallbackCooldown, using the original
    /// rate-limit classifier over full Native text and the calling runtime.
    /// Positive imported hints win without reclassifying text.
    pub fn fallback_cooldown_ms(
        &self,
        factory: &Arc<GraphValueFactory>,
        text: &ValueHandle,
        retry_after_ms: Option<f64>,
        random: f64,
        effective_runtime: &dyn ValueRuntime,
    ) -> Result<f64> {
        self.check_factory(factory)?;
        factory.heap().adopt(text)?;
        if let Some(ms) = retry_after_ms.filter(|ms| *ms > 0.0) {
            return Ok(ms);
        }
        let ex = Execution { heap: factory.heap(), runtime: effective_runtime, captures: &self.captures };
        let reason = rate_limit::reason(ex, text)?;
        Ok(match reason {
            rate_limit::Reason::Unknown => 300_000.0,
            rate_limit::Reason::Quota | rate_limit::Reason::Credits => 1_800_000.0,
            rate_limit::Reason::Rate => 30_000.0,
            rate_limit::Reason::Concurrent => 5_000.0,
            rate_limit::Reason::Capacity => 45_000.0 + random * 30_000.0,
            rate_limit::Reason::Server => 20_000.0,
        })
    }

    pub fn classify_message(
        &self,
        factory: &Arc<GraphValueFactory>,
        message: &ValueHandle,
        effective_runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.check_factory(factory)?;
        factory.heap().call(
            &self.functions.classify_message,
            &self.functions.receiver,
            std::slice::from_ref(message),
            effective_runtime,
        )
    }

    pub fn is_context_overflow(
        &self,
        factory: &Arc<GraphValueFactory>,
        message: &ValueHandle,
        context_window: &ValueHandle,
        effective_runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.check_factory(factory)?;
        factory.heap().call(
            &self.functions.is_context_overflow,
            &self.functions.receiver,
            &[message.clone(), context_window.clone()],
            effective_runtime,
        )
    }
}

#[derive(Clone, Copy)]
struct Execution<'a> {
    heap: &'a Arc<ValueHeap>,
    runtime: &'a dyn ValueRuntime,
    captures: &'a [ValueHandle],
}
impl Execution<'_> {
    fn get(self, value: &ValueHandle, key: &str) -> Result<ValueHandle> {
        self.heap.get(value, &WireString::from(key).into(), self.runtime)
    }
    fn method(self, value: &ValueHandle, key: &str, args: &[ValueHandle]) -> Result<ValueHandle> {
        let method = self.get(value, key)?;
        self.heap.call(&method, value, args, self.runtime)
    }
    fn global(self, key: &str) -> Result<ValueHandle> {
        self.get(&self.captures[0], key)
    }
    fn truthy(self, value: &ValueHandle) -> Result<bool> {
        self.heap.truthy(value)
    }
    fn nullish(self, value: &ValueHandle) -> Result<bool> {
        Ok(matches!(self.heap.kind(value)?, ValueKind::Null | ValueKind::Undefined))
    }
    fn undefined(self, value: &ValueHandle) -> Result<bool> {
        Ok(self.heap.kind(value)? == ValueKind::Undefined)
    }
    fn typeof_object(self, value: &ValueHandle) -> Result<bool> {
        Ok(matches!(self.heap.kind(value)?, ValueKind::Null | ValueKind::Object | ValueKind::Array))
    }
    fn number(self, n: f64) -> Result<ValueHandle> {
        self.heap.number(n)
    }
    fn string(self, text: &str) -> Result<ValueHandle> {
        self.heap.string(text.into())
    }
    fn boolean(self, value: bool) -> Result<ValueHandle> {
        self.heap.boolean(value)
    }
    fn equals_text(self, value: &ValueHandle, text: &str) -> Result<bool> {
        Ok(matches!(self.heap.snapshot(value)?, ValueSnapshot::String(value) if value == WireString::from(text)))
    }
    fn equals_number(self, value: &ValueHandle, number: f64) -> Result<bool> {
        Ok(matches!(self.heap.snapshot(value)?, ValueSnapshot::Number(value) if value == number))
    }
    fn strict_equal(self, left: &ValueHandle, right: &ValueHandle) -> Result<bool> {
        if let (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) =
            (self.heap.snapshot(left)?, self.heap.snapshot(right)?)
        {
            return Ok(left == right);
        }
        self.heap.object_is(left, right)
    }
    fn compare(self, left: &ValueHandle, right: &ValueHandle, operator: &str) -> Result<bool> {
        graph_values_compare_with_execution(GraphExecution::new(self.heap, self.runtime), left, right, operator)
    }
    fn compare_number(self, value: &ValueHandle, n: f64, operator: &str) -> Result<bool> {
        self.compare(value, &self.number(n)?, operator)
    }
    fn test(self, name: &'static str, value: &ValueHandle) -> Result<bool> {
        let regex = patterns::regexp(self, name)?;
        self.truthy(&self.method(&regex, "test", std::slice::from_ref(value))?)
    }
    fn includes(self, value: &ValueHandle, text: &str) -> Result<bool> {
        self.truthy(&self.method(value, "includes", &[self.string(text)?])?)
    }
    fn optional_get(self, value: &ValueHandle, key: &str) -> Result<ValueHandle> {
        if self.nullish(value)? { self.heap.undefined() } else { self.get(value, key) }
    }
    fn some_pattern_value(self, group: usize, text: &ValueHandle) -> Result<ValueHandle> {
        let array = &self.captures[group];
        // Evaluate `.some` before allocating the arrow, as in the source call.
        let method = self.get(array, "some")?;
        let callback = self.heap.fresh_compiled_function(&PATTERN_PREDICATE, std::slice::from_ref(text))?;
        self.heap.call(&method, array, &[callback], self.runtime)
    }
    fn some_pattern(self, group: usize, text: &ValueHandle) -> Result<bool> {
        self.truthy(&self.some_pattern_value(group, text)?)
    }
}

fn argument(heap: &Arc<ValueHeap>, args: &[ValueHandle], index: usize) -> Result<ValueHandle> {
    args.get(index).cloned().map(Ok).unwrap_or_else(|| heap.undefined())
}
fn pattern_predicate(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Execution { heap, runtime, captures };
    ex.method(&argument(heap, args, 0)?, "test", std::slice::from_ref(&captures[0]))
}
static PATTERN_PREDICATE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da flags.ts p=>p.test(text)",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: pattern_predicate,
};

// ECMAScript ToInt32: modulo 2^32 after truncation, not saturating Rust cast.
fn number_bits(value: f64) -> u32 {
    if !value.is_finite() || value == 0.0 {
        return 0;
    }
    value.trunc().rem_euclid(4_294_967_296.0) as u32
}
fn bits(ex: Execution<'_>, value: &ValueHandle) -> Result<u32> {
    Ok(number_bits(ex.heap.to_number(value, ex.runtime)?))
}
fn is_flag(ex: Execution<'_>, value: &ValueHandle, flag: u32) -> Result<bool> {
    Ok((bits(ex, value)? & flag) != 0)
}
fn status_from_id(ex: Execution<'_>, value: &ValueHandle) -> Result<ValueHandle> {
    if ex.truthy(value)? && !is_flag(ex, value, CLASS)? { Ok(value.clone()) } else { ex.heap.undefined() }
}
fn classified(ex: Execution<'_>, kinds: u32) -> Result<ValueHandle> {
    ex.number(f64::from((kinds | CLASS) as i32))
}
fn coalesce(ex: Execution<'_>, left: ValueHandle, right: impl FnOnce() -> Result<ValueHandle>) -> Result<ValueHandle> {
    if ex.nullish(&left)? { right() } else { Ok(left) }
}

fn status_property(ex: Execution<'_>, object: &ValueHandle, key: &str) -> Result<Option<ValueHandle>> {
    if ex.heap.kind(&ex.get(object, key)?)? == ValueKind::Number
        && ex.compare_number(&ex.get(object, key)?, 100.0, ">=")?
        && ex.compare_number(&ex.get(object, key)?, 599.0, "<=")?
    {
        return Ok(Some(ex.get(object, key)?));
    }
    Ok(None)
}

fn close_error(ex: Execution<'_>, iterator: &mut IteratorRecord, error: ValueError) -> ValueError {
    match ex.heap.iterator_close(iterator, ex.runtime) {
        Ok(()) => error,
        Err(close) if error.is_native_exception() && close.is_native_exception() => error,
        Err(close) => ValueError::IteratorClose { original: Box::new(error), close: Box::new(close) },
    }
}
fn status_internal(ex: Execution<'_>, error: &ValueHandle, depth: usize) -> Result<ValueHandle> {
    if depth > 2 || ex.nullish(error)? {
        return ex.heap.undefined();
    }
    if ex.typeof_object(error)? {
        if let Some(status) = status_property(ex, error, "status")? {
            return Ok(status);
        }
        if let Some(status) = status_property(ex, error, "statusCode")? {
            return Ok(status);
        }
        if ex.typeof_object(&ex.get(error, "response")?)?
            && ex.heap.kind(&ex.get(error, "response")?)? != ValueKind::Null
        {
            let response = ex.get(error, "response")?;
            if let Some(status) = status_property(ex, &response, "status")? {
                return Ok(status);
            }
        }
        if ex.heap.has(error, &"cause".into(), ex.runtime)? {
            let nested = status_internal(ex, &ex.get(error, "cause")?, depth + 1)?;
            if !ex.undefined(&nested)? {
                return Ok(nested);
            }
        }
    }
    let constructor = ex.global("Error")?;
    if ex.heap.instance_of(error, &constructor, ex.runtime)?
        || (ex.typeof_object(error)?
            && ex.heap.kind(error)? != ValueKind::Null
            && ex.heap.has(error, &"message".into(), ex.runtime)?)
    {
        let message = ex.get(error, "message")?;
        if ex.heap.kind(&message)? == ValueKind::String {
            let mut iterator = ex.heap.get_iterator(&ex.captures[STATUS_PATTERNS], ex.runtime)?;
            while let Some(pattern) = ex.heap.iterator_next(&mut iterator, ex.runtime)? {
                let step = (|| {
                    let matched = ex.method(&pattern, "exec", std::slice::from_ref(&message))?;
                    if ex.truthy(&matched)? {
                        let parse_int = ex.global("parseInt")?;
                        let capture = ex.get(&matched, "1")?;
                        let code = ex.heap.call(
                            &parse_int,
                            &ex.heap.undefined()?,
                            &[capture, ex.number(10.0)?],
                            ex.runtime,
                        )?;
                        if ex.compare_number(&code, 100.0, ">=")? && ex.compare_number(&code, 599.0, "<=")? {
                            return Ok(Some(code));
                        }
                    }
                    Ok(None)
                })();
                match step {
                    Ok(Some(code)) => {
                        ex.heap.iterator_close(&mut iterator, ex.runtime)?;
                        return Ok(code);
                    }
                    Ok(None) => {}
                    Err(error) => return Err(close_error(ex, &mut iterator, error)),
                }
            }
        }
    }
    ex.heap.undefined()
}
fn status_text(ex: Execution<'_>, text: &ValueHandle) -> Result<ValueHandle> {
    let literal = ex.heap.object()?;
    ex.heap.define_own_data(&literal, "message".into(), text.clone(), ex.runtime)?;
    status_internal(ex, &literal, 0)
}

fn overflow_value(ex: Execution<'_>, text: &ValueHandle) -> Result<ValueHandle> {
    let matched = ex.some_pattern_value(OVERFLOW_PATTERNS, text)?;
    if ex.truthy(&matched)? {
        return Ok(matched);
    }
    ex.method(&patterns::regexp(ex, "OVERFLOW_NO_BODY_PATTERN")?, "test", std::slice::from_ref(text))
}
fn token_overflow(ex: Execution<'_>, text: &ValueHandle) -> Result<bool> {
    ex.some_pattern(TOKEN_PATTERNS, text)
}
fn payload_rejection(ex: Execution<'_>, text: &ValueHandle) -> Result<bool> {
    if !ex.some_pattern(PAYLOAD_PATTERNS, text)? {
        return Ok(false);
    }
    if token_overflow(ex, text)? {
        return Ok(false);
    }
    Ok(true)
}
fn codex_model_identity(ex: Execution<'_>, model: &ValueHandle) -> Result<ValueHandle> {
    if ex.heap.kind(model)? != ValueKind::String {
        return ex.heap.undefined();
    }
    let separator = ex.method(model, "lastIndexOf", &[ex.string("/")?])?;
    let bare = if ex.equals_number(&separator, -1.0)? {
        model.clone()
    } else {
        let slice = ex.get(model, "slice")?;
        let start = ex.heap.add(&separator, &ex.number(1.0)?, ex.runtime)?;
        ex.heap.call(&slice, model, &[start], ex.runtime)?
    };
    let bare = ex.method(&ex.method(&bare, "trim", &[])?, "toLowerCase", &[])?;
    if !ex.truthy(&bare)? || ex.compare_number(&ex.get(&bare, "length")?, 256.0, ">")? || ex.includes(&bare, "\0")? {
        ex.heap.undefined()
    } else {
        Ok(bare)
    }
}
fn codex_denied_model(ex: Execution<'_>, text: &ValueHandle) -> Result<ValueHandle> {
    let matched = ex.method(
        &patterns::regexp(ex, "CODEX_CHATGPT_ACCOUNT_MODEL_POLICY_PATTERN")?,
        "exec",
        std::slice::from_ref(text),
    )?;
    let captured = ex.optional_get(&matched, "1")?;
    let model = if ex.nullish(&captured)? { ex.heap.undefined()? } else { ex.method(&captured, "trim", &[])? };
    if ex.undefined(&codex_model_identity(ex, &model)?)? { ex.heap.undefined() } else { Ok(model) }
}
fn codex_policy(ex: Execution<'_>, text: &ValueHandle, provider: &ValueHandle, model: &ValueHandle) -> Result<bool> {
    if !ex.equals_text(provider, "openai-codex")? {
        return Ok(false);
    }
    let denied = codex_denied_model(ex, text)?;
    let denied = codex_model_identity(ex, &denied)?;
    let requested = codex_model_identity(ex, model)?;
    Ok(!ex.undefined(&denied)? && ex.strict_equal(&denied, &requested)?)
}
fn cursor_policy(ex: Execution<'_>, text: &ValueHandle) -> Result<bool> {
    Ok(ex.test("CURSOR_PLAN_POLICY_MARKER_PATTERN", text)? && ex.test("CURSOR_PLAN_POLICY_PATTERN", text)?)
}
fn stale_response(ex: Execution<'_>, text: &ValueHandle) -> Result<bool> {
    let array = &ex.captures[STALE_PATTERNS];
    let first = ex.get(array, "0")?;
    if ex.truthy(&ex.method(&first, "test", std::slice::from_ref(text))?)? {
        return Ok(true);
    }
    let second = ex.get(array, "1")?;
    Ok(ex.truthy(&ex.method(&second, "test", std::slice::from_ref(text))?)?
        && ex.test("STALE_RESPONSE_ITEM_DETAIL_PATTERN", text)?)
}
fn transient_text(ex: Execution<'_>, text: &ValueHandle) -> Result<bool> {
    // This regexp is a fresh literal in isUnexpectedSocketCloseMessage.
    let socket = ex.heap.regexp(r"\b(?:the\s+)?socket connection (?:was )?closed unexpectedly\b".into(), "i")?;
    Ok(ex.truthy(&ex.method(&socket, "test", std::slice::from_ref(text))?)?
        || ex.test("STREAM_READ_ERROR_PATTERN", text)?
        || (ex.test("TRANSIENT_ENVELOPE_PATTERN", text)? && ex.test("TRANSIENT_ENVELOPE_TRUNCATION_PATTERN", text)?)
        || ex.test("TRANSIENT_TRANSPORT_PATTERN", text)?)
}
fn strict_tools(ex: Execution<'_>, text: &ValueHandle, status: &ValueHandle) -> Result<bool> {
    if !ex.equals_number(status, 400.0)? {
        return Ok(false);
    }
    if ex.test("ANTHROPIC_STRICT_FIELD_PATTERN", text)? && ex.test("EXTRA_INPUTS_NOT_PERMITTED_PATTERN", text)? {
        return Ok(true);
    }
    if ex.test("STRUCTURED_OUTPUTS_PATTERN", text)? && ex.test("FEATURE_NOT_SUPPORTED_PATTERN", text)? {
        return Ok(true);
    }
    if !ex.test("INVALID_REQUEST_PATTERN", text)? {
        return Ok(false);
    }
    let grammar = ex.test("GRAMMAR_TOO_LARGE_PATTERN", text)? && ex.test("GRAMMAR_TOO_LARGE_DETAIL_PATTERN", text)?;
    // Original initializes both local variables before the final ||.
    let schema = ex.test("SCHEMA_TOO_COMPLEX_PATTERN", text)?
        && ex.test("SCHEMA_TOO_COMPLEX_DETAIL_PATTERN", text)?
        && ex.test("SCHEMA_COMPILE_PATTERN", text)?;
    Ok(grammar || schema)
}
fn fast_mode(ex: Execution<'_>, text: &ValueHandle, status: &ValueHandle) -> Result<bool> {
    if !ex.equals_number(status, 400.0)? && !ex.equals_number(status, 429.0)? {
        return Ok(false);
    }
    if ex.equals_number(status, 400.0)?
        && ex.test("INVALID_REQUEST_PATTERN", text)?
        && ex.test("FAST_MODE_SPEED_PARAM_PATTERN", text)?
        && ex.test("FAST_MODE_NOT_SUPPORTED_PATTERN", text)?
    {
        return Ok(true);
    }
    Ok(ex.equals_number(status, 429.0)?
        && ex.test("FAST_MODE_RATE_LIMIT_PATTERN", text)?
        && ex.test("FAST_MODE_ENTITLEMENT_PATTERN", text)?)
}

fn classify_text(
    ex: Execution<'_>,
    text: &ValueHandle,
    status: &ValueHandle,
    prior_token_overflow: bool,
    api: &ValueHandle,
    provider: &ValueHandle,
    model: &ValueHandle,
) -> Result<ValueHandle> {
    let mut kinds = 0_u32;
    if ex.truthy(text)? {
        if ex.truthy(&overflow_value(ex, text)?)? {
            kinds |= OVERFLOW;
        }
        if payload_rejection(ex, text)? {
            kinds |= PAYLOAD;
        }
        if ex.test("MALFORMED_FUNCTION_CALL_PATTERN", text)? {
            kinds |= MALFORMED;
        }
        if ex.test("PROVIDER_FINISH_ERROR_PATTERN", text)? {
            kinds |= PROVIDER_FINISH;
        }
        if ex.test("EMPTY_RESPONSE_PATTERN", text)? {
            kinds |= EMPTY | TRANSIENT;
        }
        if ex.test("CONTENT_FILTER_PATTERN", text)? {
            kinds |= CONTENT_BLOCKED;
        }
        if ex.test("ACCOUNT_POLICY_PATTERN", text)?
            || codex_policy(ex, text, provider, model)?
            || (ex.equals_text(provider, "cursor")? && cursor_policy(ex, text)?)
        {
            kinds |= ACCOUNT_POLICY | CONTENT_BLOCKED;
        }
        if ex.test("AUTH_FAILURE_PATTERN", text)? {
            kinds |= AUTH_FAILED;
        }

        let clean_status = if ex.truthy(status)? { status.clone() } else { status_text(ex, text)? };
        let opaque = rate_limit::opaque(ex, text)?;
        let limit_status = rate_limit::usage_status(ex, &clean_status)?;
        let reason = rate_limit::reason(ex, text)?;
        let billing_cap = ex.equals_number(&clean_status, 402.0)? && rate_limit::billing_402(ex, text)?;
        let concurrency_excluded =
            reason == rate_limit::Reason::Concurrent && !ex.equals_number(&clean_status, 402.0)?;
        if !concurrency_excluded
            && (billing_cap
                || rate_limit::usage_text(ex, text)?
                || ((ex.equals_number(&clean_status, 403.0)? || ex.undefined(&clean_status)?)
                    && ex.test("ACCOUNT_SCOPED_403_PATTERN", text)?)
                || (limit_status && (opaque || reason == rate_limit::Reason::Quota)))
        {
            kinds |= USAGE_LIMIT;
        }
        if ex.test("TIMEOUT_PATTERN", text)? {
            kinds |= TRANSIENT | TIMEOUT;
        } else if transient_text(ex, text)? {
            kinds |= TRANSIENT;
        }
        if reason == rate_limit::Reason::Concurrent {
            kinds |= TRANSIENT;
        }
        if (ex.equals_text(api, "openai-responses")? || ex.equals_text(api, "openai-codex-responses")?)
            && stale_response(ex, text)?
        {
            kinds |= STALE;
        }
        if ex.equals_number(&clean_status, 400.0)? && ex.test("GENERATION_NAN_PATTERN", text)? {
            kinds |= TRANSIENT;
        }
        if strict_tools(ex, text, &clean_status)? {
            kinds |= GRAMMAR;
        }
        if fast_mode(ex, text, &clean_status)? {
            kinds |= FAST_MODE;
        }
    }
    let status_evidence =
        coalesce(ex, status.clone(), || if ex.truthy(text)? { status_text(ex, text) } else { ex.heap.undefined() })?;
    if ex.equals_number(&status_evidence, 413.0)?
        && !prior_token_overflow
        && !(ex.truthy(text)? && token_overflow(ex, text)?)
    {
        kinds |= PAYLOAD;
    }
    if ex.equals_number(&status_evidence, 402.0)? && (ex.undefined(text)? || rate_limit::opaque(ex, text)?) {
        kinds |= USAGE_LIMIT;
    }
    if kinds != 0 {
        return classified(ex, kinds);
    }
    let fallback =
        coalesce(ex, status.clone(), || if ex.truthy(text)? { status_text(ex, text) } else { ex.heap.undefined() })?;
    if ex.equals_number(&fallback, 401.0)? || ex.equals_number(&fallback, 403.0)? {
        return classified(ex, AUTH_FAILED);
    }
    coalesce(ex, fallback, || ex.number(0.0))
}

fn classify_message_body(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    effective_runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Execution { heap, runtime: effective_runtime, captures };
    let message = argument(heap, args, 0)?;
    let existing = ex.get(&message, "errorId")?;
    let status = coalesce(ex, ex.get(&message, "errorStatus")?, || status_from_id(ex, &existing))?;
    let overflow_only =
        !ex.undefined(&existing)? && is_flag(ex, &existing, OVERFLOW)? && !is_flag(ex, &existing, PAYLOAD)?;
    let text = coalesce(ex, ex.get(&message, "errorClassificationMessage")?, || ex.get(&message, "errorMessage"))?;
    let api = ex.get(&message, "api")?;
    let provider = ex.get(&message, "provider")?;
    let model = ex.get(&message, "model")?;
    let text_id = classify_text(ex, &text, &status, overflow_only, &api, &provider, &model)?;
    let mut kinds = (bits(ex, &existing)? | bits(ex, &text_id)?) & KIND_MASK;
    if ex.equals_number(&status, 413.0)?
        && ex.truthy(&text)?
        && token_overflow(ex, &text)?
        && (bits(ex, &text_id)? & PAYLOAD) == 0
    {
        kinds &= !PAYLOAD;
    }
    if ex.truthy(&text)? && ex.test("LLAMA_CPP_TOOL_CALL_PARSE_PATTERN", &text)? {
        kinds &= !TRANSIENT;
    }
    let id = if kinds != 0 {
        classified(ex, kinds)?
    } else {
        coalesce(ex, status_from_id(ex, &text_id)?, || {
            coalesce(ex, status_from_id(ex, &existing)?, || coalesce(ex, status.clone(), || ex.number(0.0)))
        })?
    };
    heap.assign(&message, "errorId".into(), id.clone(), effective_runtime)?;
    Ok(id)
}
static CLASSIFY_MESSAGE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da flags.ts:697-739 classifyMessage",
    name: "classifyMessage",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 7,
    body: classify_message_body,
};

fn usage_overflow(ex: Execution<'_>, message: &ValueHandle, window: &ValueHandle) -> Result<bool> {
    if !ex.truthy(window)? {
        return Ok(false);
    }
    let input = ex.get(&ex.get(message, "usage")?, "input")?;
    let cache_read = ex.get(&ex.get(message, "usage")?, "cacheRead")?;
    let sum = ex.heap.add(&input, &cache_read, ex.runtime)?;
    let cache_write = ex.get(&ex.get(message, "usage")?, "cacheWrite")?;
    let sum = ex.heap.add(&sum, &cache_write, ex.runtime)?;
    ex.compare(&sum, window, ">")
}
fn context_overflow_body(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    effective_runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Execution { heap, runtime: effective_runtime, captures };
    let message = argument(heap, args, 0)?;
    let window = argument(heap, args, 1)?;
    if is_flag(ex, &ex.get(&message, "errorId")?, OVERFLOW)? {
        return ex.boolean(true);
    }
    if usage_overflow(ex, &message, &window)? {
        return ex.boolean(true);
    }
    if !ex.equals_text(&ex.get(&message, "stopReason")?, "error")? {
        return ex.boolean(false);
    }
    if !ex.truthy(&ex.get(&message, "errorMessage")?)? {
        return ex.boolean(false);
    }
    overflow_value(ex, &ex.get(&message, "errorMessage")?)
}
static IS_CONTEXT_OVERFLOW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da flags.ts:747-759 isContextOverflow",
    name: "isContextOverflow",
    length: 2,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 7,
    body: context_overflow_body,
};
