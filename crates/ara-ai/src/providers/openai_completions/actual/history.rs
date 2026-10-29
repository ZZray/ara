//! Fixed OMP Chat convertMessages 1925–2411 (596f2da, MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//! Original Context/containers remain Native handles throughout projection.
use super::callbacks as cb;
use super::{
    GraphBuilder, GraphProviderContext, Model, OpenAICompat, ValueError, ValueHandle, ValueKind, ValueSnapshot, concat,
};
use crate::graph_history::{self, NativeExecution as Ex, OpenAIHistoryPolicy};
type Result<T> = std::result::Result<T, ValueError>;

fn positive(ex: Ex<'_>, value: &ValueHandle) -> Result<bool> {
    crate::schema_validator::graph_values_compare_with_execution(
        crate::graph_values::GraphExecution::new(ex.heap, ex.runtime),
        value,
        &ex.heap.number(0.0)?,
        ">",
    )
}
fn length_positive(ex: Ex<'_>, value: &ValueHandle) -> Result<bool> {
    positive(ex, &ex.get(value, "length")?)
}
fn before_length(ex: Ex<'_>, index: f64, value: &ValueHandle) -> Result<bool> {
    crate::schema_validator::graph_numeric_less_than_with_execution(
        crate::graph_values::GraphExecution::new(ex.heap, ex.runtime),
        index,
        &ex.get(value, "length")?,
    )
}
fn push(ex: Ex<'_>, array: &ValueHandle, value: impl FnOnce() -> Result<ValueHandle>) -> Result<()> {
    let method = ex.get(array, "push")?;
    let value = value()?;
    ex.heap.call(&method, array, &[value], ex.runtime).map(|_| ())
}
fn image(ex: Ex<'_>, source: &ValueHandle, detail: bool) -> Result<ValueHandle> {
    let supplied = ex.get(source, "url")?;
    let url = if ex.nullish(&supplied)? {
        let mime = ex.heap.to_string(&ex.get(source, "mimeType")?, ex.runtime)?;
        let data = ex.heap.to_string(&ex.get(source, "data")?, ex.runtime)?;
        ex.text(concat(&concat(&concat(&"data:".into(), &mime), &";base64,".into()), &data))?
    } else {
        supplied
    };
    let object = ex.object([("url", url)])?;
    if detail
        && ex.heap.truthy(&ex.get(source, "detail")?)?
        && !ex.strict(&ex.get(source, "detail")?, &ex.text("original")?)?
    {
        ex.own(&object, "detail", ex.get(source, "detail")?)?;
    }
    ex.object([("type", ex.text("image_url")?), ("image_url", object)])
}
fn user(ex: Ex<'_>, source: &ValueHandle, compat: &OpenAICompat) -> Result<Option<ValueHandle>> {
    let role = if compat.supports_developer_role && ex.eq(source, "role", "developer")? { "developer" } else { "user" };
    if ex.heap.kind(&ex.get(source, "content")?)? == ValueKind::String {
        let text = ex.method_lazy(&ex.get(source, "content")?, "toWellFormed", || Ok((&[]).to_vec()))?;
        let trimmed = ex.method_lazy(&text, "trim", || Ok((&[]).to_vec()))?;
        if ex.number_is(&ex.get(&trimmed, "length")?, 0.0)? {
            return Ok(None);
        }
        return ex.object([("role", ex.text(role)?), ("content", text)]).map(Some);
    }
    let parts = ex.array([])?;
    let mut omitted = false;
    ex.for_of(&ex.get(source, "content")?, |item| {
        if ex.eq(item, "type", "text")? {
            let text = ex.method_lazy(&ex.get(item, "text")?, "toWellFormed", || Ok((&[]).to_vec()))?;
            let trimmed = ex.method_lazy(&text, "trim", || Ok((&[]).to_vec()))?;
            if ex.number_is(&ex.get(&trimmed, "length")?, 0.0)? {
                return Ok(true);
            }
            push(ex, &parts, || Ok(ex.object([("type", ex.text("text")?), ("text", text)])?))?;
        } else if compat.supports_images {
            push(ex, &parts, || Ok(image(ex, item, true)?))?;
        } else {
            omitted = true;
        }
        Ok(true)
    })?;
    if omitted {
        push(ex, &parts, || {
            Ok(ex
                .object([("type", ex.text("text")?), ("text", ex.text(super::super::NON_VISION_IMAGE_PLACEHOLDER)?)])?)
        })?;
    }
    if ex.number_is(&ex.get(&parts, "length")?, 0.0)? {
        return Ok(None);
    }
    ex.object([("role", ex.text("user")?), ("content", parts)]).map(Some)
}
fn recognized(ex: Ex<'_>, value: &ValueHandle) -> Result<Option<&'static str>> {
    Ok(match ex.heap.snapshot(value)? {
        ValueSnapshot::String(s) if s.equals_ascii("reasoning_content") => Some("reasoning_content"),
        ValueSnapshot::String(s) if s.equals_ascii("reasoning") => Some("reasoning"),
        ValueSnapshot::String(s) if s.equals_ascii("reasoning_text") => Some("reasoning_text"),
        _ => None,
    })
}
fn configured(compat: &OpenAICompat) -> &str {
    compat.reasoning_content_field.as_deref().unwrap_or("reasoning_content")
}
fn thinking_text(ex: Ex<'_>, thinking: &ValueHandle) -> Result<ValueHandle> {
    let mapped = ex.apply(thinking, "map", &cb::THINKING_VALUE, &[])?;
    ex.method_lazy(&mapped, "join", || Ok((&[ex.text("\n")?]).to_vec()))
}
fn first_signature(ex: Ex<'_>, thinking: &ValueHandle, optional: bool) -> Result<ValueHandle> {
    let first = ex.get(thinking, "0")?;
    if optional && ex.nullish(&first)? { ex.heap.undefined() } else { ex.get(&first, "thinkingSignature") }
}
fn has_reasoning(ex: Ex<'_>, message: &ValueHandle) -> Result<bool> {
    for field in ["reasoning_content", "reasoning", "reasoning_text"] {
        if !ex.undefined(&ex.get(message, field)?)? {
            return Ok(true);
        }
    }
    Ok(false)
}
fn assistant(
    ex: Ex<'_>,
    source: &ValueHandle,
    index: f64,
    model: &Model,
    model_value: &ValueHandle,
    context: &GraphProviderContext,
    compat: &OpenAICompat,
    controls: &ValueHandle,
    remapped: &ValueHandle,
    state: &ValueHandle,
    demoter: &ValueHandle,
) -> Result<Option<ValueHandle>> {
    let message = ex.object([("role", ex.text("assistant")?), ("content", ex.heap.null()?)])?;
    let text_blocks = ex.apply(&ex.get(source, "content")?, "filter", &cb::TEXT_BLOCK, &[])?;
    let nonempty = ex.apply(&text_blocks, "filter", &cb::NONEMPTY_TEXT, &[])?;
    if length_positive(ex, &nonempty)? {
        let parts = ex.apply(&nonempty, "map", &cb::MERGED_TEXT, &[nonempty.clone()])?;
        ex.put(&message, "content", ex.method_lazy(&parts, "join", || Ok((&[ex.text("")?]).to_vec()))?)?;
    }
    let thinking_blocks = ex.apply(&ex.get(source, "content")?, "filter", &cb::THINKING_BLOCK, &[])?;
    let thinking = ex.apply(&thinking_blocks, "filter", &cb::NONEMPTY_THINKING, &[])?;
    if length_positive(ex, &thinking)? {
        if compat.requires_thinking_as_text {
            let rendered = ex.apply(&thinking, "map", &cb::DEMOTE_THINKING, &[demoter.clone(), model_value.clone()])?;
            let text = ex.method_lazy(&rendered, "join", || Ok((&[ex.text(" ")?]).to_vec()))?;
            let content_type = ex.get(&message, "content")?;
            let text = if ex.heap.kind(&content_type)? == ValueKind::String
                && length_positive(ex, &ex.get(&message, "content")?)?
            {
                let mut joined = ex.heap.to_string(&text, ex.runtime)?;
                joined.append_str(" ");
                joined = concat(&joined, &ex.heap.to_string(&ex.get(&message, "content")?, ex.runtime)?);
                ex.text(joined)?
            } else {
                text
            };
            ex.put(&message, "content", text)?;
        } else if compat.requires_reasoning_content_for_tool_calls {
            let signature = first_signature(ex, &thinking, false)?;
            if let Some(recognized) = recognized(ex, &signature)? {
                ex.put(
                    &message,
                    if compat.allows_synthetic_reasoning_content_for_tool_calls {
                        recognized
                    } else {
                        configured(compat)
                    },
                    thinking_text(ex, &thinking)?,
                )?;
            }
        } else if compat.thinking_format == "zai" && model.reasoning {
            ex.put(&message, configured(compat), thinking_text(ex, &thinking)?)?;
        } else if compat.replay_reasoning_content {
            let signature = first_signature(ex, &thinking, false)?;
            ex.put(&message, recognized(ex, &signature)?.unwrap_or(configured(compat)), thinking_text(ex, &thinking)?)?;
        }
    }
    if compat.requires_reasoning_content_for_tool_calls {
        let signature = first_signature(ex, &thinking, true)?;
        let field = if compat.allows_synthetic_reasoning_content_for_tool_calls {
            recognized(ex, &signature)?.unwrap_or(configured(compat))
        } else {
            configured(compat)
        };
        let reasoning_content = ex.get(&message, field)?;
        if !ex.heap.truthy(&reasoning_content)? {
            let reasoning = ex.get(&message, "reasoning")?;
            let reasoning_text = ex.get(&message, "reasoning_text")?;
            if ex.heap.truthy(&reasoning)? && field != "reasoning" {
                ex.put(&message, field, reasoning)?;
            } else if ex.heap.truthy(&reasoning_text)? && field != "reasoning_text" {
                ex.put(&message, field, reasoning_text)?;
            } else if length_positive(ex, &thinking)? {
                ex.put(&message, field, thinking_text(ex, &thinking)?)?;
            }
        }
    }
    let calls = ex.apply(&ex.get(source, "content")?, "filter", &cb::TOOL_BLOCK, &[])?;
    let needs = compat.requires_reasoning_content_on_all_assistant_turns || length_positive(ex, &calls)?;
    let mut has = has_reasoning(ex, &message)?;
    if needs
        && !has
        && compat.requires_reasoning_content_for_tool_calls
        && !compat.allows_synthetic_reasoning_content_for_tool_calls
    {
        let all = ex.apply(&ex.get(source, "content")?, "filter", &cb::THINKING_BLOCK, &[])?;
        if length_positive(ex, &all)? {
            let signature = first_signature(ex, &all, false)?;
            if recognized(ex, &signature)?.is_some() {
                ex.put(&message, configured(compat), thinking_text(ex, &all)?)?;
                has = true;
            }
        }
    }
    if needs
        && !has
        && compat.requires_reasoning_content_for_tool_calls
        && !compat.allows_synthetic_reasoning_content_for_tool_calls
    {
        ex.put(&message, configured(compat), ex.text("")?)?;
        has = true;
    }
    if length_positive(ex, &calls)?
        && compat.requires_reasoning_content_for_tool_calls
        && compat.allows_synthetic_reasoning_content_for_tool_calls
        && matches!(compat.thinking_format.as_str(), "openai" | "openrouter" | "zai")
        && !has
    {
        ex.put(&message, configured(compat), ex.text(".")?)?;
        has = true;
    }
    if length_positive(ex, &calls)? {
        let hash = context.history_id_hash_function.clone().unwrap_or(ex.heap.undefined()?);
        let projected = ex.apply(
            &calls,
            "map",
            &cb::REPLAY_CALL,
            &[
                model_value.clone(),
                controls.clone(),
                state.clone(),
                source.clone(),
                ex.heap.number(index)?,
                remapped.clone(),
                hash,
            ],
        )?;
        ex.put(&message, "tool_calls", projected)?;
        let details = ex.apply(&calls, "flatMap", &cb::REASONING_DETAILS, &[])?;
        if length_positive(ex, &details)? {
            ex.put(&message, "reasoning_details", details)?;
        }
    }
    if ex.heap.kind(&ex.get(&message, "content")?)? == ValueKind::Null
        && (has || ex.heap.truthy(&ex.get(&message, "tool_calls")?)?)
    {
        ex.put(&message, "content", ex.text("")?)?;
    }
    let content = ex.get(&message, "content")?;
    let has_content = !ex.nullish(&content)? && length_positive(ex, &content)?;
    if !has_content
        && ex.heap.truthy(&ex.get(&message, "tool_calls")?)?
        && compat.requires_assistant_content_for_tool_calls
    {
        ex.put(&message, "content", ex.text(".")?)?;
    }
    if !has_content && !ex.heap.truthy(&ex.get(&message, "tool_calls")?)? && !has {
        return Ok(None);
    }
    Ok(Some(message))
}
fn consume_id(ex: Ex<'_>, remapped: &ValueHandle, id: &ValueHandle) -> Result<ValueHandle> {
    let queue = ex.method_lazy(remapped, "get", || Ok((&[id.clone()]).to_vec()))?;
    if !ex.heap.truthy(&queue)? || ex.number_is(&ex.get(&queue, "length")?, 0.0)? {
        return ex.heap.null();
    }
    let next = ex.method_lazy(&queue, "shift", || Ok((&[]).to_vec()))?;
    let next = if ex.nullish(&next)? { ex.heap.null()? } else { next };
    if ex.number_is(&ex.get(&queue, "length")?, 0.0)? {
        ex.method_lazy(remapped, "delete", || Ok((&[id.clone()]).to_vec()))?;
    }
    Ok(next)
}
pub(super) fn project_messages(
    builder: &GraphBuilder,
    model: &Model,
    context: &GraphProviderContext,
    compat: &OpenAICompat,
    prefix_binding: bool,
) -> Result<ValueHandle> {
    let ex = Ex { heap: builder.0.heap(), runtime: builder.0.runtime() };
    let policy = OpenAIHistoryPolicy {
        prefix_binding,
        reads_foreign_thinking: model.reasoning && compat.thinking_format == "zai" && !compat.requires_thinking_as_text,
    };
    let model_value = ex.object([
        ("provider", ex.text(model.provider.clone())?),
        ("api", ex.text(model.api.clone())?),
        ("id", ex.text(model.id.clone())?),
    ])?;
    let controls = ex.object([
        ("mistral", ex.heap.boolean(compat.requires_mistral_tool_ids)?),
        ("limit", ex.heap.boolean(compat.uses_openai_tool_call_id_limit)?),
    ])?;
    let normalizer = ex.heap.fresh_compiled_function(&cb::NORMALIZE_ID, &[model_value.clone(), controls.clone()])?;
    let dialect = match context.history_demotion_dialect {
        Some(d) => ex.text(d.as_str())?,
        None => ex.heap.undefined()?,
    };
    let demoter = ex.heap.fresh_compiled_function(&cb::DEMOTE, &[dialect])?;
    let original = context.original_field(builder.0.as_ref(), "messages")?;
    let transformed = graph_history::transform_original_openai_messages(
        builder,
        &original,
        &model_value,
        &policy,
        &normalizer,
        &demoter,
        if compat.requires_mistral_tool_ids {
            9
        } else if compat.uses_openai_tool_call_id_limit {
            40
        } else {
            64
        },
        if compat.requires_mistral_tool_ids { "dup" } else { "_dup" },
    )?;
    let remapped = ex.heap.new_map(&ex.heap.undefined()?, ex.runtime)?;
    let state = ex.object([("counter", ex.heap.number(0.0)?)])?;
    let output = ex.array([])?;
    let prompts = cb::system_prompts(ex, &context.original_field(builder.0.as_ref(), "systemPrompt")?)?;
    if length_positive(ex, &prompts)? {
        let role = if model.reasoning && compat.supports_developer_role { "developer" } else { "system" };
        if compat.supports_multiple_system_messages {
            ex.for_of(&prompts, |prompt| {
                push(ex, &output, || Ok(ex.object([("role", ex.text(role)?), ("content", prompt.clone())])?))?;
                Ok(true)
            })?;
        } else {
            push(ex, &output, || {
                Ok(ex.object([
                    ("role", ex.text(role)?),
                    ("content", ex.method_lazy(&prompts, "join", || Ok((&[ex.text("\n\n")?]).to_vec()))?),
                ])?)
            })?;
        }
    }
    let mut last = ex.heap.null()?;
    let mut index = 0.0;
    while before_length(ex, index, &transformed)? {
        let msg = ex.index(&transformed, index)?;
        if compat.requires_assistant_after_tool_result
            && ex.strict(&last, &ex.text("toolResult")?)?
            && (ex.eq(&msg, "role", "user")? || ex.eq(&msg, "role", "developer")?)
        {
            push(ex, &output, || {
                Ok(ex.object([
                    ("role", ex.text("assistant")?),
                    ("content", ex.text("I have processed the tool results.")?),
                ])?)
            })?;
        }
        if ex.eq(&msg, "role", "user")? || ex.eq(&msg, "role", "developer")? {
            let Some(user) = user(ex, &msg, compat)? else {
                index += 1.0;
                continue;
            };
            push(ex, &output, || Ok(user))?;
        } else if ex.eq(&msg, "role", "assistant")? {
            let Some(assistant) = assistant(
                ex,
                &msg,
                index,
                model,
                &model_value,
                context,
                compat,
                &controls,
                &remapped,
                &state,
                &demoter,
            )?
            else {
                index += 1.0;
                continue;
            };
            push(ex, &output, || Ok(assistant))?;
        } else if ex.eq(&msg, "role", "toolResult")? {
            let images = ex.array([])?;
            let mut j = index;
            while before_length(ex, j, &transformed)? && ex.eq(&ex.index(&transformed, j)?, "role", "toolResult")? {
                let result = ex.index(&transformed, j)?;
                let texts = ex.apply(&ex.get(&result, "content")?, "filter", &cb::TEXT_BLOCK, &[])?;
                let values = ex.apply(&texts, "map", &cb::TEXT_VALUE, &[])?;
                let text = ex.method_lazy(&values, "join", || Ok((&[ex.text("\n")?]).to_vec()))?;
                let has_images = ex.apply(&ex.get(&result, "content")?, "some", &cb::IMAGE_BLOCK, &[])?;
                let has_images = ex.heap.truthy(&has_images)?;
                let omitted = has_images && !compat.supports_images;
                let has_text = length_positive(ex, &text)?;
                let remap = consume_id(ex, &remapped, &ex.get(&result, "toolCallId")?)?;
                let id = if ex.nullish(&remap)? {
                    let raw = ex.get(&result, "toolCallId")?;
                    let name = ex.get(&result, "toolName")?;
                    let name = if ex.nullish(&name)? { ex.text("tool")? } else { name };
                    let mut seed = ex.heap.to_string(&ex.heap.number(j)?, ex.runtime)?;
                    seed.append_str(":");
                    seed = concat(&seed, &ex.heap.to_string(&name, ex.runtime)?);
                    cb::ensure_id(
                        ex,
                        &raw,
                        &ex.text(seed)?,
                        &ex.heap.undefined()?,
                        &model_value,
                        &controls,
                        &state,
                        &context.history_id_hash_function.clone().unwrap_or(ex.heap.undefined()?),
                    )?
                } else {
                    remap
                };
                let content = if omitted {
                    let mut joined = ex.heap.to_string(&text, ex.runtime)?;
                    if length_positive(ex, &text)? {
                        joined.append_str("\n");
                    }
                    joined.append_str(super::super::NON_VISION_IMAGE_PLACEHOLDER);
                    ex.text(joined)?
                } else if has_text {
                    text
                } else if has_images {
                    ex.text("(see attached image)")?
                } else {
                    ex.text("")?
                };
                let projected = ex.object([
                    ("role", ex.text("tool")?),
                    ("content", ex.method_lazy(&content, "toWellFormed", || Ok((&[]).to_vec()))?),
                    ("tool_call_id", cb::mistral(ex, &id, compat.requires_mistral_tool_ids)?),
                ])?;
                if compat.requires_tool_result_name && ex.heap.truthy(&ex.get(&result, "toolName")?)? {
                    ex.put(&projected, "name", ex.get(&result, "toolName")?)?;
                }
                push(ex, &output, || Ok(projected))?;
                if has_images && compat.supports_images {
                    ex.for_of(&ex.get(&result, "content")?, |block| {
                        if ex.eq(block, "type", "image")? {
                            push(ex, &images, || Ok(image(ex, block, false)?))?;
                        }
                        Ok(true)
                    })?;
                }
                j += 1.0;
            }
            index = j;
            if length_positive(ex, &images)? {
                if compat.requires_assistant_after_tool_result {
                    push(ex, &output, || {
                        Ok(ex.object([
                            ("role", ex.text("assistant")?),
                            ("content", ex.text("I have processed the tool results.")?),
                        ])?)
                    })?;
                }
                let content = ex.array([ex.object([
                    ("type", ex.text("text")?),
                    ("text", ex.text("Attached image(s) from tool result:")?),
                ])?])?;
                ex.for_of(&images, |part| {
                    let length = ex.length(&content)?;
                    let key = ex.heap.to_string(&ex.heap.number(length)?, ex.runtime)?;
                    ex.heap.define_own_data(&content, key.into(), part.clone(), ex.runtime)?;
                    Ok(true)
                })?;
                push(ex, &output, || Ok(ex.object([("role", ex.text("user")?), ("content", content)])?))?;
                last = ex.text("user")?;
            } else {
                last = ex.text("toolResult")?;
            }
            continue;
        }
        last = if ex.eq(&msg, "role", "developer")? {
            ex.text(if model.reasoning && compat.supports_developer_role { "developer" } else { "system" })?
        } else {
            ex.get(&msg, "role")?
        };
        index += 1.0;
    }
    Ok(output)
}
