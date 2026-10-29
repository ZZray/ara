//! Fixed transform-messages.ts:420-582 credential redaction (OMP596f2da, MIT).
//! Disabled is an identity pass-through. Only outbound Graph copies are made.
use super::callbacks::{self, Exec};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueHeap, ValueKind, ValueRuntime,
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;
const MODULE: &str = "ara-ai.transform-messages";
const TOKEN_PATTERN: &str = r"(?<![a-zA-Z0-9_*-])(gh[opusr]_[a-zA-Z0-9_*]{36,}|github_pat_[a-zA-Z0-9_*]{36,}|glpat-[a-zA-Z0-9_*-]{20,}|sk-proj-[a-zA-Z0-9_*-]{36,}|sk-ant-[a-zA-Z0-9_*-]{36,}|sk-[a-zA-Z0-9_*-]{48,})(?![a-zA-Z0-9_*-])";
pub(super) fn prepare(heap: &Arc<ValueHeap>) -> Result<()> {
    heap.module_regexp(MODULE, "SENSITIVE_TOKEN_RE", TOKEN_PATTERN, "gi")?;
    Ok(())
}
fn starts(ex: Exec<'_>, text: &ValueHandle, prefix: &str) -> Result<bool> {
    Ok(ex.heap.truthy(&ex.method(text, "startsWith", &[ex.text(prefix)?])?)?)
}
fn plausible(ex: Exec<'_>, token: &ValueHandle) -> Result<bool> {
    let lower = ex.method(token, "toLowerCase", &[])?;
    let prefix = if starts(ex, &lower, "github_pat_")? {
        11
    } else if starts(ex, &lower, "glpat-")? {
        6
    } else if starts(ex, &lower, "sk-proj-")? {
        8
    } else if starts(ex, &lower, "sk-ant-")? {
        7
    } else if starts(ex, &lower, "gh")? {
        4
    } else {
        3
    };
    let secret = ex.method(token, "slice", &[ex.heap.number(f64::from(prefix))?])?;
    let stars = ex.heap.regexp(r"^\*+$".into(), "")?;
    if ex.heap.truthy(&ex.method(&stars, "test", &[secret.clone()])?)? {
        return Ok(true);
    }
    let patterns = ex.array([
        ex.heap.regexp("[a-z]".into(), "")?,
        ex.heap.regexp("[A-Z]".into(), "")?,
        ex.heap.regexp(r"\d".into(), "")?,
        ex.heap.regexp("[_-]".into(), "")?,
    ])?;
    let matched = ex.apply(&patterns, "filter", &ENTROPY_PATTERN, &[secret])?;
    Ok(ex.heap.to_number(&ex.get(&matched, "length")?, ex.runtime)? >= 2.0)
}
pub(super) fn credentials(ex: Exec<'_>, text: &ValueHandle, enabled: bool) -> Result<ValueHandle> {
    if !enabled {
        return Ok(text.clone());
    }
    let method = ex.get(text, "replace")?;
    let pattern = ex.heap.module_regexp(MODULE, "SENSITIVE_TOKEN_RE", TOKEN_PATTERN, "gi")?;
    let callback = ex.heap.fresh_compiled_function(&TOKEN_REPLACEMENT, &[])?;
    ex.heap.call(&method, text, &[pattern, callback], ex.runtime)
}
fn changed(ex: Exec<'_>) -> Result<ValueHandle> {
    ex.object([("changed", ex.heap.boolean(false)?)])
}
fn mark(ex: Exec<'_>, state: &ValueHandle) -> Result<()> {
    ex.heap.assign(state, "changed".into(), ex.heap.boolean(true)?, ex.runtime)
}
pub(super) fn in_object(ex: Exec<'_>, value: &ValueHandle, enabled: bool) -> Result<(ValueHandle, bool)> {
    if !enabled {
        return Ok((value.clone(), false));
    }
    if ex.heap.kind(value)? == ValueKind::String {
        let redacted = credentials(ex, value, true)?;
        let changed = !callbacks::strict_equal(ex.heap, &redacted, value)?;
        return Ok((redacted, changed));
    }
    if ex.heap.kind(value)? == ValueKind::Array {
        let state = changed(ex)?;
        let result = ex.apply(value, "map", &OBJECT_ITEM, &[state.clone()])?;
        return Ok((result, ex.heap.truthy(&ex.get(&state, "changed")?)?));
    }
    if ex.heap.kind(value)? == ValueKind::Object {
        let mut any = false;
        let result = ex.heap.object()?;
        let entries = ex.heap.object_entries(value, ex.runtime)?;
        // EnumerableOwnProperties completes all Gets before the source for-of
        // starts, exactly as Object.entries. Subsequent recursion is ordered.
        let mut iterator = ex.heap.get_iterator(&entries, ex.runtime)?;
        while let Some(pair) = ex.heap.iterator_next(&mut iterator, ex.runtime)? {
            let attempt = (|| {
                let mut binding = ex.heap.get_iterator(&pair, ex.runtime)?;
                let key = ex.heap.iterator_next(&mut binding, ex.runtime)?.unwrap_or(ex.heap.undefined()?);
                let member = ex.heap.iterator_next(&mut binding, ex.runtime)?.unwrap_or(ex.heap.undefined()?);
                ex.heap.iterator_close(&mut binding, ex.runtime)?;
                let (sub, hit) = in_object(ex, &member, true)?;
                if hit {
                    any = true;
                }
                let key = ex.heap.to_string(&key, ex.runtime)?;
                // Assignment retains the fixed __proto__/inherited setter
                // behavior; it is deliberately not CreateDataProperty.
                ex.heap.assign(&result, key.into(), sub, ex.runtime)
            })();
            if let Err(primary) = attempt {
                let _ = ex.heap.iterator_close(&mut iterator, ex.runtime);
                return Err(primary);
            }
        }
        return Ok((result, any));
    }
    Ok((value.clone(), false))
}
pub(super) fn native_items(ex: Exec<'_>, items: &ValueHandle, enabled: bool) -> Result<ValueHandle> {
    Ok(in_object(ex, items, enabled)?.0)
}
pub(super) fn messages(ex: Exec<'_>, messages: &ValueHandle, enabled: bool) -> Result<ValueHandle> {
    if enabled { ex.apply(messages, "map", &MESSAGE, &[]) } else { Ok(messages.clone()) }
}
fn with_content(ex: Exec<'_>, message: &ValueHandle, content: ValueHandle) -> Result<ValueHandle> {
    let next = ex.heap.spread(message, ex.runtime)?;
    ex.own(&next, "content", content)?;
    Ok(next)
}
fn entropy_pattern(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    ex.method(&callbacks::first(heap, args)?, "test", &[captures[0].clone()])
}
fn token_replacement(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let token = callbacks::first(heap, args)?;
    if !plausible(ex, &token)? {
        return Ok(token);
    }
    let lower = ex.method(&token, "toLowerCase", &[])?;
    ex.text(if starts(ex, &lower, "gh")? {
        "[github_token_redacted]"
    } else if starts(ex, &lower, "gl")? {
        "[gitlab_token_redacted]"
    } else if starts(ex, &lower, "sk-ant-")? {
        "[anthropic_token_redacted]"
    } else if starts(ex, &lower, "sk")? {
        "[openai_token_redacted]"
    } else {
        "[token_redacted]"
    })
}
fn object_item(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let (result, hit) = in_object(ex, &callbacks::first(heap, args)?, true)?;
    if hit {
        mark(ex, &captures[0])?;
    }
    Ok(result)
}
fn text_block(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let block = callbacks::first(heap, args)?;
    if ex.eq(&block, "type", "text")? {
        let redacted = credentials(ex, &ex.get(&block, "text")?, true)?;
        if !callbacks::strict_equal(heap, &redacted, &ex.get(&block, "text")?)? {
            mark(ex, &captures[0])?;
            let next = heap.spread(&block, runtime)?;
            ex.own(&next, "text", redacted)?;
            return Ok(next);
        }
    }
    Ok(block)
}
fn assistant_block(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let block = callbacks::first(heap, args)?;
    if ex.eq(&block, "type", "text")? {
        let redacted = credentials(ex, &ex.get(&block, "text")?, true)?;
        if !callbacks::strict_equal(heap, &redacted, &ex.get(&block, "text")?)? {
            mark(ex, &captures[0])?;
            let next = heap.spread(&block, runtime)?;
            ex.own(&next, "text", redacted)?;
            return Ok(next);
        }
    } else if ex.eq(&block, "type", "thinking")? {
        let redacted = credentials(ex, &ex.get(&block, "thinking")?, true)?;
        if !callbacks::strict_equal(heap, &redacted, &ex.get(&block, "thinking")?)? {
            mark(ex, &captures[0])?;
            let next = heap.spread(&block, runtime)?;
            ex.own(&next, "thinking", redacted)?;
            ex.own(&next, "thinkingSignature", heap.undefined()?)?;
            return Ok(next);
        }
    } else if ex.eq(&block, "type", "toolCall")? && heap.truthy(&ex.get(&block, "arguments")?)? {
        let (result, hit) = in_object(ex, &ex.get(&block, "arguments")?, true)?;
        if hit {
            mark(ex, &captures[0])?;
            let cast = if heap.truthy(&result)? && heap.kind(&result)? == ValueKind::Object {
                result
            } else {
                heap.undefined()?
            };
            let next = heap.spread(&block, runtime)?;
            ex.own(&next, "arguments", cast)?;
            ex.own(&next, "thoughtSignature", heap.undefined()?)?;
            return Ok(next);
        }
    }
    Ok(block)
}
fn message(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let msg = callbacks::first(heap, args)?;
    if ex.eq(&msg, "role", "user")? || ex.eq(&msg, "role", "developer")? {
        if heap.kind(&ex.get(&msg, "content")?)? == ValueKind::String {
            let redacted = credentials(ex, &ex.get(&msg, "content")?, true)?;
            if callbacks::strict_equal(heap, &redacted, &ex.get(&msg, "content")?)? {
                return Ok(msg);
            }
            return with_content(ex, &msg, redacted);
        }
        let content = ex.get(&msg, "content")?;
        let state = changed(ex)?;
        let projected = ex.apply(&content, "map", &TEXT_BLOCK, &[state.clone()])?;
        if heap.truthy(&ex.get(&state, "changed")?)? {
            return with_content(ex, &msg, projected);
        }
        return Ok(msg);
    }
    if ex.eq(&msg, "role", "toolResult")? {
        let state = changed(ex)?;
        let content = ex.apply(&ex.get(&msg, "content")?, "map", &TEXT_BLOCK, &[state.clone()])?;
        if heap.truthy(&ex.get(&state, "changed")?)? {
            return with_content(ex, &msg, content);
        }
        return Ok(msg);
    }
    if ex.eq(&msg, "role", "assistant")? {
        let state = changed(ex)?;
        let content = ex.apply(&ex.get(&msg, "content")?, "map", &ASSISTANT_BLOCK, &[state.clone()])?;
        if heap.truthy(&ex.get(&state, "changed")?)? {
            return with_content(ex, &msg, content);
        }
        return Ok(msg);
    }
    Ok(msg)
}
macro_rules! spec {
    ($name:ident,$body:ident,$captures:literal,$source:literal) => {
        static $name: CompiledFunctionSpec = CompiledFunctionSpec {
            source: $source,
            name: "",
            length: 1,
            form: CompiledFunctionForm::Arrow,
            capture_count: $captures,
            body: $body,
        };
    };
}
spec!(ENTROPY_PATTERN, entropy_pattern, 1, "transform-messages.ts:439");
spec!(TOKEN_REPLACEMENT, token_replacement, 0, "transform-messages.ts:457");
spec!(OBJECT_ITEM, object_item, 1, "transform-messages.ts:485");
spec!(TEXT_BLOCK, text_block, 1, "transform-messages.ts:517/533");
spec!(ASSISTANT_BLOCK, assistant_block, 1, "transform-messages.ts:549");
spec!(MESSAGE, message, 0, "transform-messages.ts:507");
