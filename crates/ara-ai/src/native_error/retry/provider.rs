//! Fixed OMP596f2da error/retryable.ts and consumed utils/fetch-retry.ts (MIT).
//! Native classes, Get/Call order and regexp/iterator identities stay original.
use super::*;
const CLASSIFY_FUNCTION: usize = 11;
const STATUS_FUNCTION: usize = 12;
const FETCH_STATUS_PATTERNS: usize = 13;
pub(super) fn prepare(
    heap: &Arc<ValueHeap>,
    base: &[ValueHandle],
    namespace: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let mut captures = base.to_vec();
    captures.push(heap.get(namespace, &"classify".into(), runtime)?);
    captures.push(heap.get(namespace, &"status".into(), runtime)?);
    let patterns = [
        ("STATUS_MESSAGE_PATTERNS_0", r"\berror\s*[:=]\s*(\d{3})\b"),
        ("STATUS_MESSAGE_PATTERNS_1", r"error\s*\((\d{3})\)"),
        ("STATUS_MESSAGE_PATTERNS_2", r"status\s*[:=]?\s*(\d{3})"),
        ("STATUS_MESSAGE_PATTERNS_3", r"\bhttp\s*(\d{3})\b"),
        (
            "STATUS_MESSAGE_PATTERNS_4",
            r"\b(\d{3})\s*(?:status|error)\b",
        ),
    ]
    .into_iter()
    .map(|(name, source)| heap.module_regexp("ara-ai.omp596f2da.fetch-retry", name, source, "i"))
    .collect::<Result<Vec<_>>>()?;
    captures.push(heap.array(patterns)?);
    heap.fresh_compiled_function(&PROVIDER_RETRYABLE, &captures)
}
fn lexical(ex: Execution<'_>, capture: usize, args: &[ValueHandle]) -> Result<ValueHandle> {
    ex.heap.call(
        &ex.captures[capture],
        &ex.heap.undefined()?,
        args,
        ex.runtime,
    )
}
fn module_test(
    ex: Execution<'_>,
    name: &'static str,
    pattern: &'static str,
    value: &ValueHandle,
) -> Result<bool> {
    let regexp = ex
        .heap
        .module_regexp("ara-ai.omp596f2da.fetch-retry", name, pattern, "i")?;
    ex.truthy(&ex.method(&regexp, "test", std::slice::from_ref(value))?)
}
fn socket_close(ex: Execution<'_>, value: &ValueHandle) -> Result<bool> {
    let regexp = ex.heap.regexp(
        r"\b(?:the\s+)?socket connection (?:was )?closed unexpectedly\b".into(),
        "i",
    )?;
    ex.truthy(&ex.method(&regexp, "test", std::slice::from_ref(value))?)
}
fn finite(ex: Execution<'_>, value: &ValueHandle) -> Result<bool> {
    ex.truthy(&ex.method(
        &ex.global("Number")?,
        "isFinite",
        std::slice::from_ref(value),
    )?)
}
fn fetch_status_message(ex: Execution<'_>, message: &ValueHandle) -> Result<ValueHandle> {
    let mut iterator = ex
        .heap
        .get_iterator(&ex.captures[FETCH_STATUS_PATTERNS], ex.runtime)?;
    loop {
        let step = match ex.heap.iterator_next(&mut iterator, ex.runtime) {
            Ok(step) => step,
            Err(error) => return Err(close_error(ex, &mut iterator, error)),
        };
        let Some(pattern) = step else {
            return ex.heap.undefined();
        };
        let outcome = (|| {
            let matched = ex.method(&pattern, "exec", std::slice::from_ref(message))?;
            if !ex.truthy(&matched)? {
                return Ok(None);
            }
            let number = ex.global("Number")?;
            let value = ex.heap.call(
                &number,
                &ex.heap.undefined()?,
                &[ex.get(&matched, "1")?],
                ex.runtime,
            )?;
            Ok(
                if finite(ex, &value)?
                    && ex.compare_number(&value, 100.0, ">=")?
                    && ex.compare_number(&value, 599.0, "<=")?
                {
                    Some(value)
                } else {
                    None
                },
            )
        })();
        match outcome {
            Ok(Some(value)) => {
                ex.heap.iterator_close(&mut iterator, ex.runtime)?;
                return Ok(value);
            }
            Ok(None) => {}
            Err(error) => return Err(close_error(ex, &mut iterator, error)),
        }
    }
}
fn fetch_status(ex: Execution<'_>, error: &ValueHandle, depth: u8) -> Result<ValueHandle> {
    if !ex.truthy(error)? || !ex.typeof_object(error)? || depth > 2 {
        return ex.heap.undefined();
    }
    let raw = coalesce(ex, ex.get(error, "status")?, || {
        coalesce(ex, ex.get(error, "statusCode")?, || {
            ex.optional_get(&ex.get(error, "response")?, "status")
        })
    })?;
    let value = match ex.heap.kind(&raw)? {
        ValueKind::Number if finite(ex, &raw)? => raw,
        ValueKind::String => {
            let parsed = ex.heap.call(
                &ex.global("Number")?,
                &ex.heap.undefined()?,
                &[raw],
                ex.runtime,
            )?;
            if finite(ex, &parsed)? {
                parsed
            } else {
                ex.heap.undefined()?
            }
        }
        _ => ex.heap.undefined()?,
    };
    if !ex.undefined(&value)?
        && ex.compare_number(&value, 100.0, ">=")?
        && ex.compare_number(&value, 599.0, "<=")?
    {
        return Ok(value);
    }
    if ex.truthy(&ex.get(error, "message")?)? {
        let value = fetch_status_message(ex, &ex.get(error, "message")?)?;
        if !ex.undefined(&value)? {
            return Ok(value);
        }
    }
    if ex.truthy(&ex.get(error, "cause")?)? {
        return fetch_status(ex, &ex.get(error, "cause")?, depth + 1);
    }
    ex.heap.undefined()
}
fn fetch_retryable(ex: Execution<'_>, error: &ValueHandle) -> Result<bool> {
    let message = coalesce(ex, ex.optional_get(error, "message")?, || ex.string(""))?;
    let name = coalesce(ex, ex.optional_get(error, "name")?, || ex.string(""))?;
    if ex.equals_text(&name, "AbortError")? {
        return Ok(true);
    }
    let timeout = ex.heap.regexp("timeout|timed out|aborted".into(), "i")?;
    if ex.truthy(&ex.method(&timeout, "test", std::slice::from_ref(&message))?)? {
        return Ok(true);
    }
    let status = fetch_status(ex, error, 0)?;
    if !ex.undefined(&status)? {
        if ex.compare_number(&status, 500.0, ">=")?
            || ex.equals_number(&status, 408.0)?
            || ex.equals_number(&status, 429.0)?
        {
            return Ok(true);
        }
        if ex.compare_number(&status, 400.0, ">=")? && ex.compare_number(&status, 500.0, "<")? {
            return Ok(false);
        }
    }
    if module_test(
        ex,
        "VALIDATION_MESSAGE_PATTERN",
        "invalid|validation|bad request|unsupported|schema|missing required|not found|unauthorized|forbidden",
        &message,
    )? {
        return Ok(false);
    }
    Ok(socket_close(ex, &message)?
        || module_test(
            ex,
            "TRANSIENT_MESSAGE_PATTERN",
            "overloaded|rate.?limit|too many requests|service.?unavailable|server error|internal error|connection.?error|unable to connect|fetch failed|network error|stream stall|other side closed|HTTP2(?:StreamReset|RefusedStream|EnhanceYourCalm)",
            &message,
        )?)
}
fn body(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Execution {
        heap,
        captures,
        runtime,
    };
    let error = argument(heap, args, 0)?;
    if !instance(ex, &error, &ex.global("Error")?)? {
        return heap.boolean(false);
    }
    let id = lexical(ex, CLASSIFY_FUNCTION, std::slice::from_ref(&error))?;
    if is_flag(ex, &id, USAGE_LIMIT)? {
        return heap.boolean(false);
    }
    let status = lexical(ex, STATUS_FUNCTION, std::slice::from_ref(&error))?;
    if !ex.undefined(&status)?
        && ex.compare_number(&status, 400.0, ">=")?
        && ex.compare_number(&status, 500.0, "<")?
        && !ex.equals_number(&status, 408.0)?
        && !ex.equals_number(&status, 429.0)?
    {
        return heap.boolean(false);
    }
    let message = ex.method(&ex.get(&error, "message")?, "toLowerCase", &[])?;
    if socket_close(ex, &message)?
        || ex.includes(&message, "tls: bad record mac")?
        || ex.includes(&message, "type=server_error")?
        || ex.test("TRANSIENT_TRANSPORT_PATTERN", &message)?
        || ex.truthy(&ex.method(
            &heap.module_regexp(
                "ara-ai.omp596f2da.error-retryable",
                "PROVIDER_TRANSIENT_EXTRA_PATTERN",
                "bad record mac|stream error.*received from peer|1302",
                "i",
            )?,
            "test",
            std::slice::from_ref(&message),
        )?)?
    {
        return heap.boolean(true);
    }
    // Both source helpers perform their own instanceof and fresh message Gets.
    if instance(ex, &error, &ex.global("Error")?)?
        && ex.test(
            "STREAM_PARSE_TRUNCATION_PATTERN",
            &ex.get(&error, "message")?,
        )?
    {
        return heap.boolean(true);
    }
    if instance(ex, &error, &ex.global("Error")?)?
        && ex.test("STREAM_EVENT_ORDER_PATTERN", &ex.get(&error, "message")?)?
    {
        return heap.boolean(true);
    }
    heap.boolean(fetch_retryable(ex, &error)?)
}
static PROVIDER_RETRYABLE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da error/retryable.ts isProviderRetryableError",
    name: "isProviderRetryableError",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 14,
    body,
};
