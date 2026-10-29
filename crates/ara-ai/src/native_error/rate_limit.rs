//! Reachable fixed OMP rate-limit.ts consumers of classifyText; MIT.
//! Original global/method receivers, repeated reads and catch/for-of scopes.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reason {
    Quota,
    Credits,
    Rate,
    Concurrent,
    Capacity,
    Server,
    Unknown,
}
fn is_quota(reason: Reason) -> bool {
    matches!(reason, Reason::Quota | Reason::Credits)
}

fn is_array(ex: Execution<'_>, value: &ValueHandle) -> Result<bool> {
    let array = ex.global("Array")?;
    ex.truthy(&ex.method(&array, "isArray", std::slice::from_ref(value))?)
}
fn as_record(ex: Execution<'_>, value: &ValueHandle) -> Result<ValueHandle> {
    if ex.typeof_object(value)? && ex.heap.kind(value)? != ValueKind::Null && !is_array(ex, value)? {
        Ok(value.clone())
    } else {
        ex.heap.undefined()
    }
}
fn json_body(ex: Execution<'_>, text: &ValueHandle) -> Result<ValueHandle> {
    let start = ex.method(text, "indexOf", &[ex.string("{")?])?;
    let end = ex.method(text, "lastIndexOf", &[ex.string("}")?])?;
    if ex.compare_number(&start, 0.0, "<")? || ex.compare(&end, &start, "<")? {
        return ex.heap.undefined();
    }
    let parsed = (|| {
        // Call-expression receiver/Function are selected before slice args.
        let json = ex.global("JSON")?;
        let parse = ex.get(&json, "parse")?;
        let slice = ex.get(text, "slice")?;
        let end_after = ex.heap.add(&end, &ex.number(1.0)?, ex.runtime)?;
        let body = ex.heap.call(&slice, text, &[start, end_after], ex.runtime)?;
        let parsed = ex.heap.call(&parse, &json, &[body], ex.runtime)?;
        as_record(ex, &parsed)
    })();
    match parsed {
        Ok(value) => Ok(value),
        Err(error) if error.is_native_exception() => ex.heap.undefined(),
        // Missing carrier/capability/source/Host implementation is not a
        // JavaScript parse rejection, and must not become a valid UNKNOWN.
        Err(error) => Err(error),
    }
}
fn detail_reason(
    ex: Execution<'_>,
    value: &ValueHandle,
    error: &ValueHandle,
    text: &ValueHandle,
) -> Result<Option<Reason>> {
    let detail = as_record(ex, value)?;
    if !ex.equals_text(&ex.optional_get(&detail, "@type")?, "type.googleapis.com/google.rpc.ErrorInfo")?
        || ex.heap.kind(&ex.get(&detail, "reason")?)? != ValueKind::String
    {
        return Ok(None);
    }
    let reason = ex.method(&ex.method(&ex.get(&detail, "reason")?, "trim", &[])?, "toUpperCase", &[])?;
    if ex.equals_text(&reason, "QUOTA_EXHAUSTED")? {
        return Ok(Some(Reason::Quota));
    }
    if ex.equals_text(&reason, "INSUFFICIENT_G1_CREDITS_BALANCE")? {
        return Ok(Some(Reason::Credits));
    }
    if ex.equals_text(&reason, "RATE_LIMIT_EXCEEDED")? {
        if ex.heap.kind(&ex.get(error, "message")?)? == ValueKind::String
            && ex.test("ANTIGRAVITY_MODEL_QUOTA_PATTERN", &ex.get(error, "message")?)?
        {
            return Ok(Some(Reason::Quota));
        }
        let imported = &ex.captures[1];
        if ex.undefined(imported)? {
            return Err(ValueError::UnsupportedOperation("Original imported extractRetryHint is not bound"));
        }
        let delay = ex.heap.call(imported, &ex.heap.undefined()?, &[ex.heap.undefined()?, text.clone()], ex.runtime)?;
        return Ok(Some(if !ex.undefined(&delay)? && ex.compare_number(&delay, 300_000.0, ">=")? {
            Reason::Quota
        } else {
            Reason::Rate
        }));
    }
    Ok(None)
}
fn google_reason(ex: Execution<'_>, text: &ValueHandle) -> Result<Option<Reason>> {
    let body = json_body(ex, text)?;
    let error = as_record(ex, &ex.optional_get(&body, "error")?)?;
    if ex.heap.kind(&ex.optional_get(&error, "status")?)? != ValueKind::String {
        return Ok(None);
    }
    let status = ex.method(&ex.method(&ex.get(&error, "status")?, "trim", &[])?, "toUpperCase", &[])?;
    if !ex.equals_text(&status, "RESOURCE_EXHAUSTED")? {
        return Ok(None);
    }
    // Original Array.isArray Function/receiver are selected before evaluating
    // its error.details argument, which may be an observable getter.
    let array = ex.global("Array")?;
    let is_array = ex.get(&array, "isArray")?;
    let checked_details = ex.get(&error, "details")?;
    if !ex.truthy(&ex.heap.call(&is_array, &array, &[checked_details], ex.runtime)?)? {
        return Ok(None);
    }
    // Array.isArray(error.details) and for-of(error.details) are distinct Gets.
    let details = ex.get(&error, "details")?;
    let mut iterator = ex.heap.get_iterator(&details, ex.runtime)?;
    while let Some(value) = ex.heap.iterator_next(&mut iterator, ex.runtime)? {
        match detail_reason(ex, &value, &error, text) {
            Ok(Some(reason)) => {
                ex.heap.iterator_close(&mut iterator, ex.runtime)?;
                return Ok(Some(reason));
            }
            Ok(None) => {}
            Err(error) => return Err(close_error(ex, &mut iterator, error)),
        }
    }
    Ok(None)
}

fn subscription_cap(ex: Execution<'_>, text: &ValueHandle) -> Result<bool> {
    Ok(ex.test("SUBSCRIPTION_CAP_PATTERN", text)? && !ex.test("TRANSIENT_INTERVAL_RATE_LIMIT_PATTERN", text)?)
}
pub(super) fn dashscope(ex: Execution<'_>, text: &ValueHandle) -> Result<bool> {
    Ok(ex.test("DASHSCOPE_TOKEN_LIMIT_DOC_PATTERN", text)? && ex.test("DASHSCOPE_TOKEN_LIMIT_MESSAGE_PATTERN", text)?)
}

pub(super) fn reason(ex: Execution<'_>, text: &ValueHandle) -> Result<Reason> {
    if let Some(reason) = google_reason(ex, text)? {
        return Ok(reason);
    }
    let lower_with_status = ex.method(text, "toLowerCase", &[])?;
    let resource = patterns::regexp(ex, "RESOURCE_EXHAUSTED_PATTERN")?;
    let lower = ex.method(&lower_with_status, "replace", &[resource, ex.string("")?])?;
    let resource_status = !ex.strict_equal(&lower, &lower_with_status)?;
    if ex.includes(&lower, "quota will reset")? || ex.includes(&lower, "exhausted your capacity")? {
        return Ok(Reason::Quota);
    }
    if ex.test("CN_QUOTA_EXHAUSTED_PATTERN", text)? && !ex.test("CN_TRANSIENT_CAP_PATTERN", text)? {
        return Ok(Reason::Quota);
    }
    if dashscope(ex, text)? {
        return Ok(Reason::Rate);
    }
    if ex.test("CONCURRENT_LIMIT_PATTERN", text)? {
        return Ok(Reason::Concurrent);
    }
    if ex.includes(&lower, "capacity")?
        || ex.includes(&lower, "overloaded")?
        || ex.includes(&lower, "529")?
        || ex.includes(&lower, "503")?
    {
        return Ok(Reason::Capacity);
    }
    if ex.test("ACCOUNT_RATE_LIMIT_PATTERN", text)? {
        return Ok(Reason::Quota);
    }
    if ex.test("SPEND_LIMIT_PATTERN", text)? {
        return Ok(Reason::Quota);
    }
    if subscription_cap(ex, text)? {
        return Ok(Reason::Quota);
    }
    if ex.test("OPENROUTER_DAILY_FREE_LIMIT_PATTERN", text)? {
        return Ok(Reason::Quota);
    }
    if ex.test("CLINE_PASS_QUOTA_PATTERN", text)? {
        return Ok(Reason::Quota);
    }
    if ex.includes(&lower, "per minute")?
        || ex.includes(&lower, "rate limit")?
        || ex.includes(&lower, "too many requests")?
        || ex.includes(&lower, "presque")?
    {
        return Ok(Reason::Rate);
    }
    if ex.includes(&lower, "exhausted")?
        || ex.includes(&lower, "quota")?
        || ex.includes(&lower, "usage limit")?
        || ex.includes(&lower, "run out of credits")?
        || ex.includes(&lower, "out of credits")?
        || ex.includes(&lower, "spending-limit")?
        || ex.includes(&lower, "spending limit")?
        || ex.test("INSUFFICIENT_BALANCE_PATTERN", text)?
        || ex.test("CREDITS_EXHAUSTED_PATTERN", text)?
    {
        return Ok(Reason::Quota);
    }
    if ex.includes(&lower, "500")?
        || ex.includes(&lower, "internal error")?
        || ex.includes(&lower, "internal server error")?
    {
        return Ok(Reason::Server);
    }
    if resource_status {
        return Ok(Reason::Capacity);
    }
    Ok(Reason::Unknown)
}

pub(super) fn opaque(ex: Execution<'_>, text: &ValueHandle) -> Result<bool> {
    // Original inline regex literals are fresh for every invocation. Preserve
    // each selected replace before evaluating that call's literal arguments.
    let replace = ex.get(text, "replace")?;
    let digits = ex.heap.regexp(r"\b(?:429|402)\b".into(), "g")?;
    let cleaned = ex.heap.call(&replace, text, &[digits, ex.string("")?], ex.runtime)?;
    let replace = ex.get(&cleaned, "replace")?;
    let framing = ex.heap.regexp(r"\b(?:http|https|status|error|code|response|message)\b".into(), "gi")?;
    let cleaned = ex.heap.call(&replace, &cleaned, &[framing, ex.string("")?], ex.runtime)?;
    let replace = ex.get(&cleaned, "replace")?;
    let no_body = ex.heap.regexp(r"\(?\bno body\b\)?".into(), "gi")?;
    let cleaned = ex.heap.call(&replace, &cleaned, &[no_body, ex.string("")?], ex.runtime)?;
    let latin = ex.heap.regexp(r"[a-z\d]{3,}".into(), "i")?;
    Ok(!ex.truthy(&ex.method(&latin, "test", std::slice::from_ref(&cleaned))?)?
        && !ex.test("CN_QUOTA_EXHAUSTED_PATTERN", &cleaned)?
        && !ex.test("CN_TRANSIENT_CAP_PATTERN", &cleaned)?
        && !ex.test("CN_THROTTLE_PATTERN", &cleaned)?)
}
pub(super) fn usage_status(ex: Execution<'_>, status: &ValueHandle) -> Result<bool> {
    Ok(ex.equals_number(status, 429.0)? || ex.equals_number(status, 402.0)?)
}
pub(super) fn billing_402(ex: Execution<'_>, text: &ValueHandle) -> Result<bool> {
    if ex.undefined(text)? || opaque(ex, text)? {
        return Ok(true);
    }
    if ex.test("STATUS_402_QUOTA_PATTERN", text)? {
        return Ok(true);
    }
    let reason = reason(ex, text)?;
    Ok(is_quota(reason) || reason == Reason::Concurrent)
}
pub(super) fn usage_text(ex: Execution<'_>, text: &ValueHandle) -> Result<bool> {
    if let Some(reason) = google_reason(ex, text)? {
        return Ok(is_quota(reason));
    }
    if dashscope(ex, text)? {
        return Ok(false);
    }
    Ok(ex.test("USAGE_LIMIT_PATTERN", text)?
        || ex.test("CREDITS_EXHAUSTED_PATTERN", text)?
        || (ex.test("CN_QUOTA_EXHAUSTED_PATTERN", text)? && !ex.test("CN_TRANSIENT_CAP_PATTERN", text)?)
        || ex.test("SPEND_LIMIT_PATTERN", text)?
        || ex.test("ACCOUNT_RATE_LIMIT_PATTERN", text)?
        || subscription_cap(ex, text)?
        || ex.test("OPENROUTER_DAILY_FREE_LIMIT_PATTERN", text)?)
}
