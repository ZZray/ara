//! Fixed OMP596f2da7101178214aa27a753529d15e6b7ad91d:
//! agent-loop.ts:1593-1639 and dialect/{catalog,history,rendering}.ts.
//! MIT; Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY.md.
//!
//! Owned request projection over original same-lineage collections. Captured
//! tools and imported JSON/Date objects come from the Host; no transcript,
//! arguments, schemas or images are exported to typed/serde representations.
//! DeepSeek/Hermes/Qwen3 are supported here. Other fixed dialects remain an
//! explicit integration gap, never substituted with a supported dialect.

use super::{Dialect, deepseek, hermes, qwen3};
use crate::graph_history::NativeExecution;
use crate::graph_values::{GraphExecution, GraphValueFactory};
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueHeap, ValueKind, ValueRuntime,
    },
};
use std::sync::Arc;

type Result<T> = std::result::Result<T, ValueError>;

macro_rules! source_arrow {
    ($name:ident, $source:literal, $length:literal, $captures:literal, $body:ident) => {
        static $name: CompiledFunctionSpec = CompiledFunctionSpec {
            source: concat!("OMP596f2da:", $source),
            name: "",
            length: $length,
            form: CompiledFunctionForm::Arrow,
            capture_count: $captures,
            body: $body,
        };
    };
}

fn first(heap: &Arc<ValueHeap>, args: &[ValueHandle]) -> Result<ValueHandle> {
    args.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())
}

fn implicit(ex: NativeExecution<'_>, value: &ValueHandle) -> Result<WireString> {
    if ex.heap.kind(value)? == ValueKind::Symbol {
        return Err(ValueError::TypeError("Cannot implicitly convert Symbol to string".into()));
    }
    ex.heap.to_string(value, ex.runtime)
}

fn template(ex: NativeExecution<'_>, before: &str, value: &ValueHandle, after: &str) -> Result<ValueHandle> {
    let mut text = WireString::from(before).units().to_vec();
    text.extend_from_slice(implicit(ex, value)?.units());
    text.extend(after.encode_utf16());
    ex.text(WireString::from_units(text))
}

fn less_than_length(ex: NativeExecution<'_>, source: &ValueHandle, index: f64) -> Result<bool> {
    let length = ex.get(source, "length")?;
    crate::schema_validator::graph_values_compare_with_execution(
        GraphExecution::new(ex.heap, ex.runtime),
        &ex.heap.number(index)?,
        &length,
        "<",
    )
}

fn literal_append(ex: NativeExecution<'_>, array: &ValueHandle, index: &mut usize, value: ValueHandle) -> Result<()> {
    ex.own(array, &index.to_string(), value)?;
    *index += 1;
    Ok(())
}

fn bigint_replacer(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    let item = args.get(1).cloned().map(Ok).unwrap_or_else(|| heap.undefined())?;
    if heap.kind(&item)? == ValueKind::BigInt { ex.method_lazy(&item, "toString", || Ok(Vec::new())) } else { Ok(item) }
}
source_arrow!(BIGINT_REPLACER, "packages/utils/src/json.ts:stringifyJson.replacer", 2, 0, bigint_replacer);

/// rendering.ts stringifyJson delegates to the original global method with a
/// fresh two-parameter replacer and undefined space, then uses nullish fallback.
fn stringify_arguments(ex: NativeExecution<'_>, json: &ValueHandle, value: ValueHandle) -> Result<ValueHandle> {
    let method = ex.get(json, "stringify")?;
    let replacer = ex.heap.fresh_compiled_function(&BIGINT_REPLACER, &[])?;
    let result = ex.heap.call(&method, json, &[value, replacer, ex.heap.undefined()?], ex.runtime)?;
    if ex.nullish(&result)? { ex.text("null") } else { Ok(result) }
}

fn catalog_entry(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    let json = &captures[0];
    let tool = first(heap, args)?;
    // Callee selection precedes object-literal argument evaluation.
    let stringify = ex.get(json, "stringify")?;
    let declaration = heap.object()?;
    ex.own(&declaration, "type", ex.text("function")?)?;
    let function = heap.object()?;
    ex.own(&function, "name", ex.get(&tool, "name")?)?;
    let description = ex.get(&tool, "description")?;
    let description = if ex.nullish(&description)? { ex.text("")? } else { description };
    ex.own(&function, "description", description)?;
    let schema = crate::schema_wire::graph_tool_wire_schema_with_execution(GraphExecution::new(heap, runtime), &tool)?;
    ex.own(&function, "parameters", schema)?;
    ex.own(&declaration, "function", function)?;
    heap.call(&stringify, json, &[declaration], runtime)
}
source_arrow!(CATALOG_ENTRY, "packages/ai/src/dialect/catalog.ts:12-20", 1, 1, catalog_entry);

fn catalog(ex: NativeExecution<'_>, tools: &ValueHandle, json: &ValueHandle) -> Result<ValueHandle> {
    let mapped = ex.apply(tools, "map", &CATALOG_ENTRY, &[json.clone()])?;
    ex.method_lazy(&mapped, "join", || Ok(vec![ex.text("\n")?]))
}

fn replace_tools(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    _: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    catalog(NativeExecution { heap, runtime }, &captures[0], &captures[1])
}
source_arrow!(REPLACE_TOOLS, "packages/ai/src/dialect/catalog.ts:29", 0, 2, replace_tools);
fn replace_prompt(
    _: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    _: &[ValueHandle],
    _: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    Ok(captures[0].clone())
}
source_arrow!(REPLACE_PROMPT, "packages/ai/src/dialect/catalog.ts:30", 0, 1, replace_prompt);

fn prompt(ex: NativeExecution<'_>, tools: &ValueHandle, json: &ValueHandle, dialect: Dialect) -> Result<ValueHandle> {
    let definition_prompt = match dialect {
        Dialect::Deepseek => deepseek::DEEPSEEK_PROMPT,
        Dialect::Hermes => hermes::HERMES_PROMPT,
        Dialect::Qwen3 => qwen3::QWEN3_PROMPT,
        _ => {
            return Err(ValueError::UnsupportedOperation("Owned Graph prompt for this fixed dialect is not connected"));
        }
    };
    let prompt = ex.method_lazy(&ex.text(definition_prompt)?, "trim", || Ok(Vec::new()))?;
    let template = ex.text(include_str!("prompt-template.md"))?;
    let rendered = ex.method_lazy(&template, "replace", || {
        Ok(vec![
            ex.text("{{TOOLS}}")?,
            ex.heap.fresh_compiled_function(&REPLACE_TOOLS, &[tools.clone(), json.clone()])?,
        ])
    })?;
    ex.method_lazy(&rendered, "replace", || {
        Ok(vec![ex.text("{{DIALECT}}")?, ex.heap.fresh_compiled_function(&REPLACE_PROMPT, &[prompt])?])
    })
}

fn is_tool_call(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    heap.boolean(ex.eq(&first(heap, args)?, "type", "toolCall")?)
}
fn is_text(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    heap.boolean(ex.eq(&first(heap, args)?, "type", "text")?)
}
fn text_member(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    NativeExecution { heap, runtime }.get(&first(heap, args)?, "text")
}
source_arrow!(IS_TOOL_CALL, "packages/ai/src/dialect/history.ts:45", 1, 0, is_tool_call);
source_arrow!(IS_TEXT, "packages/ai/src/dialect/history.ts:49", 1, 0, is_text);
source_arrow!(TEXT_MEMBER, "packages/ai/src/dialect/history.ts:50", 1, 0, text_member);

fn wrapped_call(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    let call = first(heap, args)?;
    let wrapper = heap.object()?;
    ex.own(&wrapper, "name", ex.get(&call, "name")?)?;
    ex.own(&wrapper, "arguments", ex.get(&call, "arguments")?)?;
    let json = stringify_arguments(ex, &captures[0], wrapper)?;
    template(ex, "<tool_call>\n", &json, "\n</tool_call>")
}
source_arrow!(HERMES_CALL, "packages/ai/src/dialect/hermes.ts:175", 1, 1, wrapped_call);
source_arrow!(QWEN3_CALL, "packages/ai/src/dialect/qwen3.ts:208", 1, 1, wrapped_call);
fn deepseek_call(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    let call = first(heap, args)?;
    let mut text = WireString::from(deepseek::DEEPSEEK_TOOL_CALL_BEGIN).units().to_vec();
    text.extend_from_slice(implicit(ex, &ex.get(&call, "name")?)?.units());
    text.extend(deepseek::DEEPSEEK_TOOL_SEPARATOR.encode_utf16());
    let json = stringify_arguments(ex, &captures[0], ex.get(&call, "arguments")?)?;
    text.extend_from_slice(implicit(ex, &json)?.units());
    text.extend(deepseek::DEEPSEEK_TOOL_CALL_END.encode_utf16());
    ex.text(WireString::from_units(text))
}
source_arrow!(DEEPSEEK_CALL, "packages/ai/src/dialect/deepseek.ts:595", 1, 1, deepseek_call);

fn render_calls(
    ex: NativeExecution<'_>,
    calls: &ValueHandle,
    dialect: Dialect,
    json: &ValueHandle,
) -> Result<ValueHandle> {
    let (callback, separator) = match dialect {
        Dialect::Deepseek => {
            if ex.number_is(&ex.get(calls, "length")?, 0.0)? {
                return ex.text("");
            }
            (&DEEPSEEK_CALL, "")
        }
        Dialect::Hermes => (&HERMES_CALL, "\n"),
        Dialect::Qwen3 => (&QWEN3_CALL, "\n"),
        _ => {
            return Err(ValueError::UnsupportedOperation(
                "Owned Graph call renderer for this fixed dialect is not connected",
            ));
        }
    };
    let mapped = ex.apply(calls, "map", callback, &[json.clone()])?;
    let joined = ex.method_lazy(&mapped, "join", || Ok(vec![ex.text(separator)?]))?;
    if dialect == Dialect::Deepseek {
        template(ex, deepseek::DEEPSEEK_TOOL_CALLS_BEGIN, &joined, deepseek::DEEPSEEK_TOOL_CALLS_END)
    } else {
        Ok(joined)
    }
}

fn response_text(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    template(ex, "<tool_response>\n", &ex.get(&first(heap, args)?, "text")?, "\n</tool_response>")
}
source_arrow!(RESPONSE_TEXT, "packages/ai/src/dialect/rendering.ts:6", 1, 0, response_text);
fn deepseek_result(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    template(
        ex,
        deepseek::DEEPSEEK_TOOL_OUTPUT_BEGIN,
        &ex.get(&first(heap, args)?, "text")?,
        deepseek::DEEPSEEK_TOOL_OUTPUT_END,
    )
}
source_arrow!(DEEPSEEK_RESULT, "packages/ai/src/dialect/deepseek.ts:600", 1, 0, deepseek_result);

fn render_results(ex: NativeExecution<'_>, results: &ValueHandle, dialect: Dialect) -> Result<ValueHandle> {
    let spec = match dialect {
        Dialect::Deepseek => &DEEPSEEK_RESULT,
        Dialect::Hermes | Dialect::Qwen3 => &RESPONSE_TEXT,
        _ => {
            return Err(ValueError::UnsupportedOperation(
                "Owned Graph result renderer for this fixed dialect is not connected",
            ));
        }
    };
    let mapped = ex.apply(results, "map", spec, &[])?;
    ex.method_lazy(&mapped, "join", || Ok(vec![ex.text("\n")?]))
}

fn encode_assistant(
    ex: NativeExecution<'_>,
    message: &ValueHandle,
    dialect: Dialect,
    json: &ValueHandle,
) -> Result<ValueHandle> {
    let calls = ex.apply(&ex.get(message, "content")?, "filter", &IS_TOOL_CALL, &[])?;
    if ex.number_is(&ex.get(&calls, "length")?, 0.0)? {
        return Ok(message.clone());
    }
    let texts = ex.apply(&ex.get(message, "content")?, "filter", &IS_TEXT, &[])?;
    let texts = ex.apply(&texts, "map", &TEXT_MEMBER, &[])?;
    let prose = ex.method_lazy(&texts, "join", || Ok(vec![ex.text("\n")?]))?;
    let rendered = render_calls(ex, &calls, dialect, json)?;
    let trimmed = ex.method_lazy(&prose, "trim", || Ok(Vec::new()))?;
    let has_prose = crate::schema_validator::graph_values_compare_with_execution(
        GraphExecution::new(ex.heap, ex.runtime),
        &ex.get(&trimmed, "length")?,
        &ex.heap.number(0.0)?,
        ">",
    )?;
    let text = if has_prose {
        let trimmed_end = ex.method_lazy(&prose, "trimEnd", || Ok(Vec::new()))?;
        let mut text = implicit(ex, &trimmed_end)?.units().to_vec();
        text.push(0x0a);
        text.extend_from_slice(implicit(ex, &rendered)?.units());
        ex.text(WireString::from_units(text))?
    } else {
        rendered
    };
    // Object spread precedes the replacement content literal.
    ex.copy_lazy(message, "content", || {
        let content = ex.heap.array_with_length(0)?;
        let block = ex.heap.object()?;
        ex.own(&block, "type", ex.text("text")?)?;
        ex.own(&block, "text", text)?;
        ex.own(&content, "0", block)?;
        Ok(content)
    })
}

fn encode_results(
    ex: NativeExecution<'_>,
    results: &ValueHandle,
    dialect: Dialect,
    date: &ValueHandle,
) -> Result<ValueHandle> {
    let dialect_results = ex.heap.array_with_length(0)?;
    let images = ex.heap.array_with_length(0)?;
    let mut index = 0.0;
    while less_than_length(ex, results, index)? {
        let result = ex.index(results, index)?;
        let mut text = ex.text("")?;
        ex.for_of(&ex.get(&result, "content")?, |block| {
            if ex.eq(block, "type", "text")? {
                text = ex.heap.add(&text, &ex.get(block, "text")?, ex.runtime)?;
            } else if ex.eq(block, "type", "image")? {
                ex.method_lazy(&images, "push", || Ok(vec![block.clone()]))?;
            }
            Ok(true)
        })?;
        ex.method_lazy(&dialect_results, "push", || {
            let projection = ex.heap.object()?;
            ex.own(&projection, "id", ex.get(&result, "toolCallId")?)?;
            ex.own(&projection, "name", ex.get(&result, "toolName")?)?;
            ex.own(&projection, "index", ex.heap.number(index)?)?;
            ex.own(&projection, "text", text)?;
            ex.own(&projection, "isError", ex.get(&result, "isError")?)?;
            Ok(vec![projection])
        })?;
        index += 1.0;
    }
    let content = ex.heap.array_with_length(0)?;
    let block = ex.heap.object()?;
    ex.own(&block, "type", ex.text("text")?)?;
    ex.own(&block, "text", render_results(ex, &dialect_results, dialect)?)?;
    let mut position = 0;
    literal_append(ex, &content, &mut position, block)?;
    ex.for_of(&images, |image| {
        literal_append(ex, &content, &mut position, image.clone())?;
        Ok(true)
    })?;
    let message = ex.heap.object()?;
    ex.own(&message, "role", ex.text("user")?)?;
    ex.own(&message, "content", content)?;
    let first = ex.index(results, 0.0)?;
    let timestamp = if ex.nullish(&first)? { ex.heap.undefined()? } else { ex.get(&first, "timestamp")? };
    let timestamp = if ex.nullish(&timestamp)? { ex.method_lazy(date, "now", || Ok(Vec::new()))? } else { timestamp };
    ex.own(&message, "timestamp", timestamp)?;
    Ok(message)
}

fn encode_history(
    ex: NativeExecution<'_>,
    messages: &ValueHandle,
    dialect: Dialect,
    json: &ValueHandle,
    date: &ValueHandle,
) -> Result<ValueHandle> {
    let output = ex.heap.array_with_length(0)?;
    let mut index = 0.0;
    while less_than_length(ex, messages, index)? {
        let message = ex.index(messages, index)?;
        if ex.eq(&message, "role", "assistant")? {
            ex.method_lazy(&output, "push", || Ok(vec![encode_assistant(ex, &message, dialect, json)?]))?;
        } else if ex.eq(&message, "role", "toolResult")? {
            let results = ex.heap.array_with_length(0)?;
            let mut end = index;
            while less_than_length(ex, messages, end)? && ex.eq(&ex.index(messages, end)?, "role", "toolResult")? {
                ex.method_lazy(&results, "push", || Ok(vec![ex.index(messages, end)?]))?;
                end += 1.0;
            }
            ex.method_lazy(&output, "push", || Ok(vec![encode_results(ex, &results, dialect, date)?]))?;
            index = end - 1.0;
        } else {
            ex.method_lazy(&output, "push", || Ok(vec![message]))?;
        }
        index += 1.0;
    }
    Ok(output)
}

/// The Core already performed the fixed tools truthiness/length/capture gate.
/// This body starts at the source object spread, consuming that captured tools
/// handle even if later root.tools getters produce a different collection.
pub fn project_owned_context(
    factory: &Arc<GraphValueFactory>,
    context: &ValueHandle,
    tools: &ValueHandle,
    dialect: Dialect,
    json: &ValueHandle,
    date: &ValueHandle,
) -> Result<ValueHandle> {
    if !matches!(dialect, Dialect::Deepseek | Dialect::Hermes | Dialect::Qwen3) {
        return Err(ValueError::UnsupportedOperation("Owned Graph projection for this fixed dialect is not connected"));
    }
    let heap = factory.heap();
    for value in [context, tools, json, date] {
        heap.adopt(value)?;
    }
    let ex = NativeExecution { heap, runtime: factory.runtime() };
    let projected = heap.spread(context, ex.runtime)?;
    let system_prompt = heap.array_with_length(0)?;
    let source_prompt = ex.get(context, "systemPrompt")?;
    let source_prompt = if ex.nullish(&source_prompt)? { heap.array_with_length(0)? } else { source_prompt };
    let mut position = 0;
    ex.for_of(&source_prompt, |text| {
        literal_append(ex, &system_prompt, &mut position, text.clone())?;
        Ok(true)
    })?;
    literal_append(ex, &system_prompt, &mut position, prompt(ex, tools, json, dialect)?)?;
    ex.own(&projected, "systemPrompt", system_prompt)?;
    let messages = encode_history(ex, &ex.get(context, "messages")?, dialect, json, date)?;
    ex.own(&projected, "messages", messages)?;
    ex.own(&projected, "tools", heap.undefined()?)?;
    Ok(projected)
}

#[cfg(test)]
mod observations;
#[cfg(test)]
pub(super) fn observe_owned_projection_family() {
    observations::observe_owned_projection_family();
}
