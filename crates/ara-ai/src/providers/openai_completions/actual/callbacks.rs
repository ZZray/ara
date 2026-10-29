//! Fixed OMP596f2da Chat callbacks (MIT; see THIRD_PARTY_NOTICES.md).
//! Captures are original heap edges; bodies always borrow invocation runtime.
use crate::graph_history::{NativeExecution as Ex, render_demoted};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, StringifyMode, ValueError, ValueHandle, ValueHeap, ValueKind,
    ValueRuntime, ValueSnapshot,
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;
fn parse_native_json(heap: &Arc<ValueHeap>, source: &ara_rpc::WireString) -> Result<ValueHandle> {
    heap.parse_strict_units(source).map_err(|error| match error {
        ValueError::StrictParse(error) => ValueError::SyntaxError(error.to_string()),
        error => error,
    })
}
pub(super) fn first(heap: &Arc<ValueHeap>, args: &[ValueHandle]) -> Result<ValueHandle> {
    args.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())
}
fn arg(heap: &Arc<ValueHeap>, args: &[ValueHandle], index: usize) -> Result<ValueHandle> {
    args.get(index).cloned().map(Ok).unwrap_or_else(|| heap.undefined())
}
macro_rules! spec {
    ($name:ident,$source:literal,$length:literal,$body:ident,$captures:literal) => {
        pub(super) static $name: CompiledFunctionSpec = CompiledFunctionSpec {
            source: $source,
            name: "",
            length: $length,
            form: CompiledFunctionForm::Arrow,
            capture_count: $captures,
            body: $body,
        };
    };
}
fn text_block(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.boolean(Ex { heap, runtime }.eq(&first(heap, args)?, "type", "text")?)
}
fn thinking_block(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.boolean(Ex { heap, runtime }.eq(&first(heap, args)?, "type", "thinking")?)
}
fn tool_block(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.boolean(Ex { heap, runtime }.eq(&first(heap, args)?, "type", "toolCall")?)
}
fn image_block(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.boolean(Ex { heap, runtime }.eq(&first(heap, args)?, "type", "image")?)
}
fn nonempty(ex: Ex<'_>, block: &ValueHandle, key: &str) -> Result<ValueHandle> {
    let text = ex.get(block, key)?;
    if !ex.heap.truthy(&text)? {
        return Ok(text);
    }
    let trimmed = ex.method_lazy(&ex.get(block, key)?, "trim", || Ok((&[]).to_vec()))?;
    ex.heap.boolean(ex.length(&trimmed)? > 0.0)
}
fn nonempty_text(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    nonempty(Ex { heap, runtime }, &first(heap, args)?, "text")
}
fn nonempty_thinking(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    nonempty(Ex { heap, runtime }, &first(heap, args)?, "thinking")
}
fn text_value(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    Ex { heap, runtime }.get(&first(heap, args)?, "text")
}
fn thinking_value(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    Ex { heap, runtime }.get(&first(heap, args)?, "thinking")
}
fn system_prompt(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    Ex { heap, runtime }.method_lazy(&first(heap, args)?, "toWellFormed", || Ok((&[]).to_vec()))
}
fn nonempty_prompt(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, runtime };
    let trimmed = ex.method_lazy(&first(heap, args)?, "trim", || Ok((&[]).to_vec()))?;
    heap.boolean(ex.length(&trimmed)? > 0.0)
}
fn merged_text(
    heap: &Arc<ValueHeap>,
    c: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, runtime };
    let b = first(heap, args)?;
    let text = ex.method_lazy(&ex.get(&b, "text")?, "toWellFormed", || Ok((&[]).to_vec()))?;
    let marker =
        heap.get(&b, &heap.module_symbol("ara-ai.block-symbols", "provider.block.demotedThinking")?, runtime)?;
    if matches!(heap.snapshot(&marker)?, ValueSnapshot::Boolean(true))
        && heap.to_number(&arg(heap, args, 1)?, runtime)? < ex.length(&c[0])? - 1.0
    {
        ex.text(crate::actual_provider::concat(&heap.to_string(&text, runtime)?, &"\n".into()))
    } else {
        Ok(text)
    }
}
fn demote(
    heap: &Arc<ValueHeap>,
    c: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, runtime };
    let dialect = if ex.nullish(&c[0])? {
        None
    } else {
        let name = ex.string(&c[0])?;
        crate::actual_provider::graph_history_demotion_dialect(
            name.to_utf8().map_err(|_| ValueError::UnsupportedOperation("Dialect control is not scalar"))?.as_str(),
        )
    };
    heap.string(render_demoted(&ex.string(&arg(heap, args, 1)?)?, dialect)?)
}
fn demote_thinking(
    heap: &Arc<ValueHeap>,
    c: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, runtime };
    heap.call(&c[0], &heap.undefined()?, &[ex.get(&c[1], "id")?, ex.get(&first(heap, args)?, "thinking")?], runtime)
}
pub(super) fn mistral(ex: Ex<'_>, id: &ValueHandle, enabled: bool) -> Result<ValueHandle> {
    if !enabled {
        return Ok(id.clone());
    }
    let mut normalized =
        ex.method_lazy(id, "replace", || Ok((&[ex.heap.regexp("[^a-zA-Z0-9]".into(), "g")?, ex.text("")?]).to_vec()))?;
    if ex.length(&normalized)? < 9.0 {
        let padding = ex.method_lazy(&ex.text("ABCDEFGHI")?, "slice", || {
            Ok((&[ex.heap.number(0.0)?, ex.heap.number(9.0 - ex.length(&normalized)?)?]).to_vec())
        })?;
        normalized = ex.text(crate::actual_provider::concat(
            &ex.heap.to_string(&normalized, ex.runtime)?,
            &ex.heap.to_string(&padding, ex.runtime)?,
        ))?;
    } else if ex.length(&normalized)? > 9.0 {
        normalized =
            ex.method_lazy(&normalized, "slice", || Ok((&[ex.heap.number(0.0)?, ex.heap.number(9.0)?]).to_vec()))?;
    }
    Ok(normalized)
}
pub(super) fn normalize_id(
    ex: Ex<'_>,
    id: &ValueHandle,
    source: &ValueHandle,
    model: &ValueHandle,
    controls: &ValueHandle,
) -> Result<ValueHandle> {
    if ex.heap.truthy(&ex.get(controls, "mistral")?)? {
        return mistral(ex, id, true);
    }
    let same = !ex.undefined(source)?
        && ex.strict(&ex.get(source, "provider")?, &ex.get(model, "provider")?)?
        && ex.strict(&ex.get(source, "api")?, &ex.get(model, "api")?)?
        && ex.strict(&ex.get(source, "model")?, &ex.get(model, "id")?)?;
    if !same && ex.heap.truthy(&ex.method_lazy(id, "includes", || Ok((&[ex.text("|")?]).to_vec()))?)? {
        let split = ex.method_lazy(id, "split", || Ok((&[ex.text("|")?]).to_vec()))?;
        let mut first = ex.heap.undefined()?;
        ex.for_of(&split, |value| {
            first = value.clone();
            Ok(false)
        })?;
        let replaced = ex.method_lazy(&first, "replace", || {
            Ok((&[ex.heap.regexp("[^a-zA-Z0-9_-]".into(), "g")?, ex.text("_")?]).to_vec())
        })?;
        return ex.method_lazy(&replaced, "slice", || Ok((&[ex.heap.number(0.0)?, ex.heap.number(40.0)?]).to_vec()));
    }
    if ex.heap.truthy(&ex.get(controls, "limit")?)? && ex.length(id)? > 40.0 {
        ex.method_lazy(id, "slice", || Ok((&[ex.heap.number(0.0)?, ex.heap.number(40.0)?]).to_vec()))
    } else {
        Ok(id.clone())
    }
}
fn normalize_callback(
    heap: &Arc<ValueHeap>,
    c: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    normalize_id(Ex { heap, runtime }, &first(heap, args)?, &arg(heap, args, 2)?, &c[0], &c[1])
}
pub(super) fn system_prompts(ex: Ex<'_>, original: &ValueHandle) -> Result<ValueHandle> {
    if ex.nullish(original)? {
        return ex.array([]);
    }
    let prompts = match ex.heap.kind(original)? {
        ValueKind::Array => original.clone(),
        ValueKind::String => ex.array([original.clone()])?,
        _ => ex.array([])?,
    };
    let normalized = ex.apply(&prompts, "map", &SYSTEM_PROMPT, &[])?;
    ex.apply(&normalized, "filter", &NONEMPTY_PROMPT, &[])
}
pub(super) fn arguments(ex: Ex<'_>, value: &ValueHandle) -> Result<ValueHandle> {
    let stringify = |value: &ValueHandle| -> Result<ValueHandle> {
        match ex.heap.stringify(value, StringifyMode::Plain, None, ex.runtime)? {
            Some(s) => ex.text(s),
            None => ex.heap.undefined(),
        }
    };
    if ex.heap.kind(value)? == ValueKind::Object {
        return match stringify(value) {
            Err(e) if e.is_native_exception() => ex.text("{}"),
            r => r,
        };
    }
    if ex.heap.kind(value)? == ValueKind::String {
        let trimmed = ex.method_lazy(value, "trim", || Ok((&[]).to_vec()))?;
        if ex.number_is(&ex.get(&trimmed, "length")?, 0.0)? {
            return ex.text("{}");
        }
        let result = (|| {
            let parsed = parse_native_json(ex.heap, &ex.heap.to_string(&trimmed, ex.runtime)?)?;
            if ex.heap.truthy(&parsed)? && ex.heap.kind(&parsed)? == ValueKind::Object {
                stringify(&parsed).map(Some)
            } else {
                Ok(None)
            }
        })();
        match result {
            Ok(Some(value)) => return Ok(value),
            Err(e) if !e.is_native_exception() => return Err(e),
            _ => {}
        }
    }
    ex.text("{}")
}
pub(super) fn gemini(ex: Ex<'_>, value: &ValueHandle) -> Result<Option<ValueHandle>> {
    if !matches!(ex.heap.kind(value)?, ValueKind::Object | ValueKind::Array) {
        return Ok(None);
    }
    let namespaces =
        ex.heap.module_string_array("ara-ai.chat", "GEMINI_THOUGHT_SIGNATURE_NAMESPACES", &["google", "vertex"])?;
    let mut output = None;
    ex.for_of(&namespaces, |namespace| {
        let key = ex.string(namespace)?;
        let provider = ex.heap.get(value, &key.clone().into(), ex.runtime)?;
        if !matches!(ex.heap.kind(&provider)?, ValueKind::Object | ValueKind::Array) {
            return Ok(true);
        }
        let signature = ex.get(&provider, "thought_signature")?;
        if ex.heap.kind(&signature)? != ValueKind::String || ex.number_is(&ex.get(&signature, "length")?, 0.0)? {
            return Ok(true);
        }
        let result = ex.heap.object()?;
        ex.heap.define_own_data(&result, key.into(), ex.object([("thought_signature", signature)])?, ex.runtime)?;
        output = Some(result);
        Ok(false)
    })?;
    Ok(output)
}
fn parse_extra(ex: Ex<'_>, value: &ValueHandle) -> Result<Option<ValueHandle>> {
    if !ex.heap.truthy(value)? {
        return Ok(None);
    }
    let attempt = (|| {
        let parsed = parse_native_json(ex.heap, &ex.heap.to_string(value, ex.runtime)?)?;
        gemini(ex, &parsed)
    })();
    match attempt {
        Err(e) if e.is_native_exception() => Ok(None),
        result => result,
    }
}
pub(super) fn ensure_id(
    ex: Ex<'_>,
    raw: &ValueHandle,
    seed: &ValueHandle,
    source: &ValueHandle,
    model: &ValueHandle,
    controls: &ValueHandle,
    state: &ValueHandle,
    hash: &ValueHandle,
) -> Result<ValueHandle> {
    let normalized = normalize_id(ex, raw, source, model, controls)?;
    let trimmed = ex.method_lazy(&normalized, "trim", || Ok((&[]).to_vec()))?;
    if ex.length(&trimmed)? > 0.0 {
        return Ok(normalized);
    }
    let counter = ex.heap.to_number(&ex.get(state, "counter")?, ex.runtime)? + 1.0;
    ex.put(state, "counter", ex.heap.number(counter)?)?;
    if ex.undefined(hash)? {
        return Err(ValueError::UnsupportedOperation(
            "Retained Chat fallback id requires original Host Bun.hash Function binding",
        ));
    }
    let mut input = ex.heap.to_string(&ex.get(model, "provider")?, ex.runtime)?;
    input.append_str(":");
    input = crate::actual_provider::concat(&input, &ex.heap.to_string(&ex.get(model, "id")?, ex.runtime)?);
    input.append_str(":");
    input = crate::actual_provider::concat(&input, &ex.heap.to_string(seed, ex.runtime)?);
    input.append_str(":");
    input = crate::actual_provider::concat(&input, &ex.heap.to_string(&ex.heap.number(counter)?, ex.runtime)?);
    let hash = ex.heap.call(hash, &ex.heap.undefined()?, &[ex.text(input)?], ex.runtime)?;
    let base36 = ex.method_lazy(&hash, "toString", || Ok((&[ex.heap.number(36.0)?]).to_vec()))?;
    ex.text(crate::actual_provider::concat(&"call_".into(), &ex.heap.to_string(&base36, ex.runtime)?))
}
fn replay_call(
    heap: &Arc<ValueHeap>,
    c: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, runtime };
    let tc = first(heap, args)?;
    let raw_id = ex.get(&tc, "id")?;
    let mut seed = heap.to_string(&c[4], runtime)?;
    seed.append_str(":");
    seed = crate::actual_provider::concat(&seed, &heap.to_string(&arg(heap, args, 1)?, runtime)?);
    seed.append_str(":");
    seed = crate::actual_provider::concat(&seed, &heap.to_string(&ex.get(&tc, "name")?, runtime)?);
    let id = ensure_id(ex, &raw_id, &ex.text(seed)?, &c[3], &c[0], &c[1], &c[2], &c[6])?;
    let raw = ex.get(&tc, "id")?;
    let queue = ex.method_lazy(&c[5], "get", || Ok((&[raw.clone()]).to_vec()))?;
    if heap.truthy(&queue)? {
        ex.method_lazy(&queue, "push", || Ok((&[id.clone()]).to_vec()))?;
    } else {
        ex.method_lazy(&c[5], "set", || Ok((&[raw, ex.array([id.clone()])?]).to_vec()))?;
    }
    let call = ex.object([
        ("id", mistral(ex, &id, heap.truthy(&ex.get(&c[1], "mistral")?)?)?),
        ("type", ex.text("function")?),
        (
            "function",
            ex.object([("name", ex.get(&tc, "name")?), ("arguments", arguments(ex, &ex.get(&tc, "arguments")?)?)])?,
        ),
    ])?;
    if let Some(extra) = parse_extra(ex, &ex.get(&tc, "thoughtSignature")?)? {
        ex.put(&call, "extra_content", extra)?;
    }
    Ok(call)
}
fn reasoning_details(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, runtime };
    let signature = ex.get(&first(heap, args)?, "thoughtSignature")?;
    if !heap.truthy(&signature)? {
        return ex.array([]);
    }
    let attempt = (|| {
        let parsed = parse_native_json(heap, &heap.to_string(&signature, runtime)?)?;
        if gemini(ex, &parsed)?.is_some() { ex.array([]) } else { ex.array([parsed]) }
    })();
    match attempt {
        Err(e) if e.is_native_exception() => ex.array([]),
        result => result,
    }
}
fn tool_adapt(
    heap: &Arc<ValueHeap>,
    c: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, runtime };
    let tool = first(heap, args)?;
    let strict = !heap.truthy(&c[0])?
        && heap.truthy(&c[1])?
        && !matches!(heap.snapshot(&ex.get(&tool, "strict")?)?, ValueSnapshot::Boolean(false));
    let mut base = crate::schema_wire::graph_tool_wire_schema_with_execution(
        crate::graph_values::GraphExecution::new(heap, runtime),
        &tool,
    )?;
    if heap.truthy(&c[2])? {
        base = crate::schema_wire::flatten_graph_exclusive_required_root_union_with_execution(
            crate::graph_values::GraphExecution::new(heap, runtime),
            &base,
        )?;
    }
    let (parameters, strict) = crate::providers::openai_responses::actual::adapt_chat_schema(
        crate::graph_values::GraphExecution::new(heap, runtime),
        &base,
        strict,
    )?;
    ex.object([("tool", tool), ("baseParameters", base), ("parameters", parameters), ("strict", heap.boolean(strict)?)])
}
fn tool_is_strict(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    Ex { heap, runtime }.get(&first(heap, args)?, "strict")
}
fn match_wire_tool(
    heap: &Arc<ValueHeap>,
    c: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, runtime };
    let tool = first(heap, args)?;
    heap.boolean(ex.eq(&tool, "type", "function")? && ex.strict(&ex.get(&ex.get(&tool, "function")?, "name")?, &c[0])?)
}
spec!(TEXT_BLOCK, "openai-completions.ts:2075; 2340", 1, text_block, 0);
spec!(THINKING_BLOCK, "openai-completions.ts:2102; 2245", 1, thinking_block, 0);
spec!(TOOL_BLOCK, "openai-completions.ts:449; 2185", 1, tool_block, 0);
spec!(IMAGE_BLOCK, "openai-completions.ts:2344", 1, image_block, 0);
spec!(NONEMPTY_TEXT, "openai-completions.ts:2077", 1, nonempty_text, 0);
spec!(NONEMPTY_THINKING, "openai-completions.ts:2104", 1, nonempty_thinking, 0);
spec!(TEXT_VALUE, "openai-completions.ts:2341", 1, text_value, 0);
spec!(THINKING_VALUE, "openai-completions.ts:2126", 1, thinking_value, 0);
spec!(SYSTEM_PROMPT, "utils.ts:32", 1, system_prompt, 0);
spec!(NONEMPTY_PROMPT, "utils.ts:33", 1, nonempty_prompt, 0);
spec!(MERGED_TEXT, "openai-completions.ts:2092", 2, merged_text, 1);
spec!(DEMOTE, "dialect/demotion.ts:renderDemotedThinking", 2, demote, 1);
spec!(DEMOTE_THINKING, "openai-completions.ts:2108", 1, demote_thinking, 2);
spec!(NORMALIZE_ID, "openai-completions.ts:1966", 3, normalize_callback, 2);
spec!(REPLAY_CALL, "openai-completions.ts:2276", 2, replay_call, 7);
spec!(REASONING_DETAILS, "openai-completions.ts:2293", 1, reasoning_details, 0);
spec!(TOOL_ADAPT, "openai-completions.ts:2423-2436", 1, tool_adapt, 3);
spec!(TOOL_IS_STRICT, "openai-completions.ts:2443", 1, tool_is_strict, 0);
spec!(MATCH_WIRE_TOOL, "openai-completions.ts:1743; 1786", 1, match_wire_tool, 1);
