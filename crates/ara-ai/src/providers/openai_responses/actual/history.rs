//! Fixed OMP 596f2da `openai-shared.ts` buildResponsesInput and utils.ts.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! Only temporary wire objects are created. Canonical handles remain intact.
use super::callbacks::{self, execution};
use super::{GraphResponsesFlavor, GraphResponsesRequestPolicy};
use crate::Model;
use crate::actual_provider::{GraphBuilder, GraphProviderContext, concat, trim_empty};
use crate::graph_history::{self, OpenAIHistoryPolicy};
use ara_rpc::{
    WireString,
    value_heap::{PropertyKey, StringifyMode, ValueError, ValueHandle, ValueKind, ValueSnapshot},
};
use std::collections::{HashMap, HashSet};
type Result<T> = std::result::Result<T, ValueError>;

pub(super) fn eq(b: &GraphBuilder, root: &ValueHandle, key: &str, value: &str) -> Result<bool> {
    Ok(matches!(b.0.heap().snapshot(&b.get(root,key)?)?,ValueSnapshot::String(text) if text.equals_ascii(value)))
}
pub(super) fn optional(b: &GraphBuilder, root: &ValueHandle, key: &str) -> Result<Option<WireString>> {
    Ok(match b.0.heap().snapshot(&b.get(root, key)?)? {
        ValueSnapshot::String(text) => Some(text),
        _ => None,
    })
}
fn string(b: &GraphBuilder, root: &ValueHandle, key: &str) -> Result<WireString> {
    b.string(&b.get(root, key)?)
}
pub(super) fn array(b: &GraphBuilder, items: impl IntoIterator<Item = ValueHandle>) -> Result<ValueHandle> {
    b.0.heap().array(items.into_iter().collect())
}
fn join(items: impl IntoIterator<Item = WireString>, sep: &str) -> WireString {
    let mut output = WireString::from("");
    for (index, item) in items.into_iter().enumerate() {
        if index > 0 {
            output.append_str(sep);
        }
        output = concat(&output, &item);
    }
    output
}
fn spread_without(b: &GraphBuilder, item: &ValueHandle, keys: &[&str]) -> Result<ValueHandle> {
    let keys = keys.iter().map(|key| PropertyKey::from(*key)).collect::<Vec<_>>();
    b.0.heap().spread_excluding(item, &keys, b.0.runtime())
}
fn hash(context: &GraphProviderContext, text: &WireString) -> Result<WireString> {
    context
        .history_id_hash
        .as_ref()
        .ok_or(ValueError::UnsupportedOperation("Responses history requires the fixed Host Bun.hash binding"))?
        .hash_string_base36(text)
}
fn prefix(text: &WireString, fallback: &str) -> WireString {
    let units = text.units();
    let Some(end) = units.iter().position(|unit| *unit == 95) else { return fallback.into() };
    if end > 0
        && matches!(units[0],65..=90|97..=122)
        && units[..end].iter().all(|unit| matches!(unit,48..=57|65..=90|97..=122))
    {
        WireString::from_units(units[..end].to_vec())
    } else {
        fallback.into()
    }
}
fn starts(text: &WireString, value: &str) -> bool {
    text.units().starts_with(&value.encode_utf16().collect::<Vec<_>>())
}
fn truncate(context: &GraphProviderContext, id: &WireString, kind: &WireString) -> Result<WireString> {
    if id.units().len() <= 64 { Ok(id.clone()) } else { Ok(concat(&concat(kind, &"_".into()), &hash(context, id)?)) }
}
fn sanitize(id: &WireString) -> WireString {
    let mut units = id
        .units()
        .iter()
        .map(|unit| if matches!(unit,48..=57|65..=90|97..=122|95|45) { *unit } else { 95 })
        .take(64)
        .collect::<Vec<_>>();
    while units.last() == Some(&95) {
        units.pop();
    }
    WireString::from_units(units)
}
fn normalize_id(context: &GraphProviderContext, id: &WireString, kind: &str) -> Result<(WireString, WireString)> {
    let parts = id.units().split(|unit| *unit == 124).take(2).collect::<Vec<_>>();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        let call = WireString::from_units(parts[0].to_vec());
        let item = WireString::from_units(parts[1].to_vec());
        let p = prefix(&item, "");
        let allowed = p.equals_ascii(kind) || (kind == "fc" && p.equals_ascii("fcr"));
        let item = if allowed {
            truncate(context, &item, &p)?
        } else {
            concat(&format!("{kind}_").into(), &hash(context, &item)?)
        };
        return Ok((truncate(context, &call, &prefix(&call, "call"))?, item));
    }
    let digest = hash(context, id)?;
    let call =
        if starts(id, "call_") { truncate(context, id, &"call".into())? } else { concat(&"call_".into(), &digest) };
    Ok((call, concat(&format!("{kind}_").into(), &digest)))
}
fn normalize_transform(
    b: &GraphBuilder,
    model: &Model,
    context: &GraphProviderContext,
    id: &WireString,
    source: &ValueHandle,
    flavor: GraphResponsesFlavor,
) -> Result<WireString> {
    let Some(at) = id.units().iter().position(|unit| *unit == 124) else { return Ok(id.clone()) };
    if flavor == GraphResponsesFlavor::Codex {
        let call = sanitize(&WireString::from_units(id.units()[..at].to_vec()));
        let item =
            WireString::from_units(id.units()[at + 1..].split(|unit| *unit == 124).next().unwrap_or_default().to_vec());
        let mut item = WireString::from_units(
            item.units()
                .iter()
                .map(|unit| if matches!(unit,48..=57|65..=90|97..=122|95|45) { *unit } else { 95 })
                .collect(),
        );
        if !starts(&item, "fc") {
            item = concat(&"fc_".into(), &item);
        }
        let item = sanitize(&item);
        return Ok(concat(&concat(&call, &"|".into()), &item));
    }
    let foreign = optional(b, source, "provider")? != Some(model.provider.clone().into())
        || optional(b, source, "api")? != Some(model.api.clone().into());
    let (call, item) = if foreign {
        let call = sanitize(&WireString::from_units(id.units()[..at].to_vec()));
        let item =
            WireString::from_units(id.units()[at + 1..].split(|unit| *unit == 124).next().unwrap_or_default().to_vec());
        (call, concat(&"fc_".into(), &hash(context, &item)?).slice_prefix(64))
    } else {
        normalize_id(context, id, "fc")?
    };
    Ok(concat(&concat(&call, &"|".into()), &item))
}
pub(super) fn escaped(text: &WireString, enabled: bool, json: bool) -> WireString {
    if !enabled {
        return text.clone();
    }
    // Fixed substitutions operate on UTF-16 and preserve lone units. Reuse
    // the token table's string helper only for scalar text; lone units remain
    // unchanged around ASCII token matches via this lossless replacement.
    let mut result = text.units().to_vec();
    for token in ["start", "end", "message", "channel", "constrain", "return", "call"] {
        let from = format!("<|{token}|>").encode_utf16().collect::<Vec<_>>();
        let slash = if json { "\\\\" } else { "\\" };
        let to = format!("<{slash}|{token}{slash}|>").encode_utf16().collect::<Vec<_>>();
        let mut next = Vec::new();
        let mut at = 0;
        while at < result.len() {
            if result[at..].starts_with(&from) {
                next.extend(&to);
                at += from.len();
            } else {
                next.push(result[at]);
                at += 1;
            }
        }
        result = next;
    }
    WireString::from_units(result)
}
pub(super) fn image(b: &GraphBuilder, item: &ValueHandle, original: bool) -> Result<ValueHandle> {
    callbacks::image(execution(b), item, original)
}
fn input_content(
    b: &GraphBuilder,
    source: &ValueHandle,
    options: &super::super::RequestOptions,
    policy: &GraphResponsesRequestPolicy,
) -> Result<Option<ValueHandle>> {
    let content = b.get(source, "content")?;
    if b.0.heap().kind(&content)? == ValueKind::String {
        if callbacks::trim_is_zero(execution(b), &content)? {
            return Ok(None);
        }
        let raw = callbacks::well_formed(execution(b), &content)?;
        let text = if policy.escape_control_tokens { b.text(escaped(&b.string(&raw)?, true, false))? } else { raw };
        return Ok(Some(array(b, [b.object([("type", b.text("input_text")?), ("text", text)])?])?));
    }
    let text_blocks = callbacks::apply(b, &content, "filter", &callbacks::TEXT_BLOCK, &[])?;
    let image_blocks = callbacks::apply(b, &content, "filter", &callbacks::IMAGE_BLOCK, &[])?;
    let images = if options.supports_images { image_blocks.clone() } else { array(b, [])? };
    let omitted =
        !options.supports_images && b.0.heap().to_number(&b.get(&image_blocks, "length")?, b.0.runtime())? > 0.0;
    let output = b.0.heap().array_with_length(0)?;
    let mut count = 0;
    execution(b).for_of(&text_blocks, |block| {
        let raw = callbacks::well_formed(execution(b), &b.get(block, "text")?)?;
        let text = if policy.escape_control_tokens { b.text(escaped(&b.string(&raw)?, true, false))? } else { raw };
        if callbacks::trim_is_zero(execution(b), &text)? {
            return Ok(true);
        }
        b.push(&output, b.object([("type", b.text("input_text")?), ("text", text)])?)?;
        count += 1;
        Ok(true)
    })?;
    execution(b).for_of(&images, |block| {
        b.push(&output, image(b, block, policy.supports_image_detail_original)?)?;
        count += 1;
        Ok(true)
    })?;
    if omitted {
        b.push(
            &output,
            b.object([
                ("type", b.text("input_text")?),
                ("text", b.text("[image omitted: model does not support vision]")?),
            ])?,
        )?;
        count += 1;
    }
    Ok((count > 0).then_some(output))
}
fn note(b: &GraphBuilder, text: WireString) -> Result<ValueHandle> {
    b.object([("type", b.text("message")?), ("role", b.text("assistant")?), ("content", b.text(text)?)])
}
fn custom_names(b: &GraphBuilder, context: &GraphProviderContext) -> Result<HashMap<WireString, WireString>> {
    let mut names = HashMap::new();
    let tools = context.original_field(b.0.as_ref(), "tools")?;
    if b.is_nullish(&tools)? || !b.0.heap().truthy(&b.get(&tools, "length")?)? {
        return Ok(names);
    }
    execution(b).for_of(&tools, |tool| {
        let wire = b.get(tool, "customWireName")?;
        if b.0.heap().truthy(&wire)? {
            names.insert(b.string(&b.get(tool, "customWireName")?)?, string(b, tool, "name")?);
        }
        Ok(true)
    })?;
    Ok(names)
}
fn custom_name(names: &HashMap<WireString, WireString>, name: WireString) -> WireString {
    names.get(&name).cloned().unwrap_or_else(|| if name.equals_ascii("apply_patch") { "edit".into() } else { name })
}
#[derive(Default)]
struct Calls {
    known: HashSet<WireString>,
    custom: HashSet<WireString>,
    computer: HashSet<WireString>,
}
fn collect(b: &GraphBuilder, items: &[ValueHandle], calls: &mut Calls, clear: bool) -> Result<()> {
    if clear {
        calls.known.clear();
        calls.custom.clear();
        calls.computer.clear();
    }
    for item in items {
        if let Some(id) = optional(b, item, "call_id")? {
            if id.units().is_empty() {
                continue;
            }
            if eq(b, item, "type", "custom_tool_call")? {
                calls.known.insert(id.clone());
                calls.custom.insert(id);
            } else if eq(b, item, "type", "computer_call")? {
                calls.known.insert(id.clone());
                calls.computer.insert(id);
            } else if eq(b, item, "type", "function_call")? {
                calls.known.insert(id);
            }
        }
    }
    Ok(())
}
fn payload(b: &GraphBuilder, message: &ValueHandle, model: &Model, assistant: bool) -> Result<Option<ValueHandle>> {
    if assistant
        && (optional(b, message, "api")? != Some(model.api.clone().into())
            || optional(b, message, "model")? != Some(model.id.clone().into()))
    {
        return Ok(None);
    }
    let payload = b.get(message, "providerPayload")?;
    if b.is_nullish(&payload)? || !eq(b, &payload, "type", "openaiResponsesHistory")? {
        return Ok(None);
    }
    let items = b.get(&payload, "items")?;
    if b.0.heap().kind(&items)? != ValueKind::Array {
        return Ok(None);
    }
    let mut provider = b.get(&payload, "provider")?;
    if b.is_nullish(&provider)? && assistant {
        provider = b.get(message, "provider")?;
    }
    if !b.is_nullish(&provider)?
        && !matches!(b.0.heap().snapshot(&provider)?,ValueSnapshot::String(value) if value==WireString::from(model.provider.clone()))
    {
        return Ok(None);
    }
    Ok(Some(payload))
}
fn boundary(b: &GraphBuilder, item: &ValueHandle) -> Result<bool> {
    let kind = optional(b, item, "type")?;
    if kind.as_ref().is_none_or(|kind| kind.equals_ascii("message")) {
        return Ok(optional(b, item, "role")?.is_some_and(|role| !role.equals_ascii("assistant")));
    }
    let kind = kind.unwrap();
    Ok([
        "input_text",
        "input_image",
        "input_file",
        "input_audio",
        "function_call_output",
        "custom_tool_call_output",
        "computer_call_output",
        "local_shell_call_output",
        "shell_call_output",
        "apply_patch_call_output",
        "mcp_approval_response",
        "compaction",
        "compaction_summary",
        "compaction_trigger",
        "item_reference",
    ]
    .iter()
    .any(|expected| kind.equals_ascii(expected))
        || (kind.equals_ascii("additional_tools") && !eq(b, item, "role", "assistant")?)
        || (kind.equals_ascii("tool_search_output") && !eq(b, item, "execution", "server")?))
}
fn linked_reasoning(b: &GraphBuilder, items: &[ValueHandle], require_output: bool) -> Result<HashSet<usize>> {
    let mut paired = HashSet::new();
    let mut later = HashSet::new();
    if require_output {
        for (index, item) in items.iter().enumerate().rev() {
            if eq(b, item, "type", "computer_call_output")? {
                if let Some(id) = optional(b, item, "call_id")? {
                    later.insert(id);
                }
            } else if eq(b, item, "type", "computer_call")?
                && optional(b, item, "id")?.is_some()
                && optional(b, item, "call_id")?.is_some_and(|id| later.contains(&id))
            {
                paired.insert(index);
            }
        }
    }
    let mut current = Vec::new();
    let mut linked = HashSet::new();
    for (index, item) in items.iter().enumerate() {
        if boundary(b, item)? {
            current.clear();
        } else if eq(b, item, "type", "reasoning")? {
            current.push(index);
        } else if eq(b, item, "type", "computer_call")?
            && optional(b, item, "id")?.is_some()
            && (!require_output || paired.contains(&index))
        {
            linked.extend(current.iter().copied());
        }
    }
    Ok(linked)
}
fn visible_item(b: &GraphBuilder, item: &ValueHandle) -> Result<bool> {
    if eq(b, item, "type", "reasoning")? {
        return Ok(false);
    }
    if !eq(b, item, "type", "message")? || !eq(b, item, "role", "assistant")? {
        return Ok(true);
    }
    let content = b.get(item, "content")?;
    if let ValueSnapshot::String(text) = b.0.heap().snapshot(&content)? {
        return Ok(!trim_empty(&text));
    }
    for part in b.members(&content)? {
        let key = if eq(b, &part, "type", "output_text")? {
            "text"
        } else if eq(b, &part, "type", "refusal")? {
            "refusal"
        } else {
            continue;
        };
        if !trim_empty(&string(b, &part, key)?) {
            return Ok(true);
        }
    }
    Ok(false)
}
fn clamp_replay_image(b: &GraphBuilder, item: ValueHandle, original: bool) -> Result<ValueHandle> {
    if original {
        return Ok(item);
    }
    if eq(b, &item, "type", "input_image")? && eq(b, &item, "detail", "original")? {
        let output = b.0.heap().spread(&item, b.0.runtime())?;
        b.define(&output, "detail", b.text("auto")?)?;
        return Ok(output);
    }
    let content = b.get(&item, "content")?;
    if !eq(b, &item, "type", "message")? || b.0.heap().kind(&content)? != ValueKind::Array {
        return Ok(item);
    }
    let mut changed = false;
    let mut parts = Vec::new();
    for part in b.members(&content)? {
        if matches!(b.0.heap().kind(&part)?, ValueKind::Object)
            && eq(b, &part, "type", "input_image")?
            && eq(b, &part, "detail", "original")?
        {
            let next = b.0.heap().spread(&part, b.0.runtime())?;
            b.define(&next, "detail", b.text("auto")?)?;
            parts.push(next);
            changed = true;
        } else {
            parts.push(part);
        }
    }
    if changed {
        let output = b.0.heap().spread(&item, b.0.runtime())?;
        b.define(&output, "content", array(b, parts)?)?;
        Ok(output)
    } else {
        Ok(item)
    }
}
fn sanitize_replay(
    b: &GraphBuilder,
    context: &GraphProviderContext,
    items: Vec<ValueHandle>,
    options: &super::super::RequestOptions,
    policy: &GraphResponsesRequestPolicy,
    provisional: &mut Vec<ValueHandle>,
) -> Result<Vec<ValueHandle>> {
    let linked = if options.supports_computer_use { linked_reasoning(b, &items, false)? } else { HashSet::new() };
    let mut output = Vec::new();
    let mut ids: HashMap<WireString, WireString> = HashMap::new();
    for (index, item) in items.into_iter().enumerate() {
        if eq(b, &item, "type", "function_call")? {
            let Some(args) = optional(b, &item, "arguments")? else { continue };
            if trim_empty(&args) || ara_rpc::WireValue::parse_units(args.units()).is_err() {
                continue;
            }
        }
        if eq(b, &item, "type", "item_reference")? {
            continue;
        }
        let projected = if eq(b, &item, "type", "image_generation_call")? {
            let (Some(id), Some(result)) = (optional(b, &item, "id")?, optional(b, &item, "result")?) else { continue };
            if result.units().is_empty() {
                continue;
            }
            b.object([
                ("id", b.text(truncate(context, &id, &"ig".into())?)?),
                ("type", b.text("image_generation_call")?),
                ("status", b.text("completed")?),
                ("result", b.text(result)?),
            ])?
        } else if eq(b, &item, "type", "reasoning")? {
            let next = b.object([("type", b.text("reasoning")?)])?;
            if linked.contains(&index) {
                if let Some(id) = optional(b, &item, "id")? {
                    b.define(&next, "id", b.text(id)?)?;
                }
                provisional.push(next.clone());
            }
            for key in ["summary", "content"] {
                let value = b.get(&item, key)?;
                if b.0.heap().kind(&value)? == ValueKind::Array {
                    b.define(&next, key, value)?;
                }
            }
            let encrypted = b.get(&item, "encrypted_content")?;
            if matches!(b.0.heap().kind(&encrypted)?, ValueKind::String | ValueKind::Null) {
                b.define(&next, "encrypted_content", encrypted)?;
            }
            next
        } else {
            let next = spread_without(b, &item, &["id"])?;
            if eq(b, &item, "type", "computer_call")? {
                if let Some(id) = optional(b, &item, "id")? {
                    b.define(&next, "id", b.text(id)?)?;
                }
            }
            if let Some(id) = optional(b, &item, "call_id")? {
                let id = if let Some(found) = ids.get(&id) {
                    found.clone()
                } else {
                    let found = truncate(context, &id, &prefix(&id, "call"))?;
                    ids.insert(id, found.clone());
                    found
                };
                b.define(&next, "call_id", b.text(id)?)?;
            }
            clamp_replay_image(b, next, policy.supports_image_detail_original)?
        };
        let mut rejects_status = false;
        for kind in ["message", "function_call", "custom_tool_call", "compaction", "compaction_summary"] {
            if eq(b, &projected, "type", kind)? {
                rejects_status = true;
                break;
            }
        }
        let projected = if rejects_status {
            // Object.hasOwn is a separate Native observation; never swallow a
            // getter exception when reading type during the actual loop.
            if b.0.heap().has_own(&projected, &"status".into(), b.0.runtime())? {
                spread_without(b, &projected, &["status"])?
            } else {
                projected
            }
        } else {
            projected
        };
        output.push(projected);
    }
    Ok(output)
}
pub(super) fn replayable_native(b: &GraphBuilder, context: &GraphProviderContext, items: &ValueHandle) -> Result<bool> {
    Ok(replayable_native_items(b, context, items)?.is_some())
}
pub(super) fn replayable_native_items(
    b: &GraphBuilder,
    context: &GraphProviderContext,
    items: &ValueHandle,
) -> Result<Option<Vec<ValueHandle>>> {
    let cloned = b.0.heap().structured_clone_json(items, b.0.runtime())?;
    let options = super::super::RequestOptions { supports_computer_use: true, ..Default::default() };
    let policy = GraphResponsesRequestPolicy { supports_image_detail_original: true, ..Default::default() };
    let items = sanitize_replay(b, context, b.members(&cloned)?, &options, &policy, &mut Vec::new())?;
    for item in &items {
        if visible_item(b, item)? {
            return Ok(Some(items));
        }
    }
    Ok(None)
}
fn adapt(
    b: &GraphBuilder,
    items: Vec<ValueHandle>,
    custom: bool,
    computer: bool,
    names: &HashMap<WireString, WireString>,
) -> Result<Vec<ValueHandle>> {
    let mut output = Vec::new();
    for item in items {
        if !custom && eq(b, &item, "type", "custom_tool_call")? {
            let input = b.object([("input", b.get(&item, "input")?)])?;
            let next = b.object([
                ("type", b.text("function_call")?),
                ("call_id", b.get(&item, "call_id")?),
                ("name", b.text(custom_name(names, string(b, &item, "name")?))?),
                ("arguments", b.text(b.plain_json(&input)?)?),
            ])?;
            for key in ["id", "namespace"] {
                let value = b.get(&item, key)?;
                if b.0.heap().truthy(&value)? {
                    b.define(&next, key, value)?;
                }
            }
            output.push(next);
        } else if !custom && eq(b, &item, "type", "custom_tool_call_output")? {
            output.push(b.object([
                ("type", b.text("function_call_output")?),
                ("call_id", b.get(&item, "call_id")?),
                ("output", b.get(&item, "output")?),
            ])?);
        } else if !computer && (eq(b, &item, "type", "computer_call")? || eq(b, &item, "type", "computer_call_output")?)
        {
            let id = optional(b, &item, "call_id")?.unwrap_or_else(|| "unknown".into());
            let kind = if eq(b, &item, "type", "computer_call")? { "call" } else { "result" };
            let text = concat(&concat(&format!("[Previous computer {kind}; call_id=").into(), &id), &"]: ".into());
            output.push(note(b, concat(&text, &b.plain_json(&item)?))?);
        } else {
            output.push(item);
        }
    }
    Ok(output)
}
fn codex_unroll_items(b: &GraphBuilder, items: Vec<ValueHandle>, original: bool) -> Result<Vec<ValueHandle>> {
    // Fixed Codex uses executable function calls/screenshots, whereas Generic
    // Responses demotes unsupported computer items to assistant notes.
    let linked = linked_reasoning(b, &items, false)?;
    let mut retained = HashSet::new();
    let mut current = Vec::new();
    let mut surviving = false;
    for (index, item) in items.iter().enumerate() {
        if boundary(b, item)? {
            if surviving {
                retained.extend(current.iter().copied());
            }
            current.clear();
            surviving = false;
        } else if eq(b, item, "type", "reasoning")? {
            current.push(index);
        } else if !eq(b, item, "type", "computer_call")? && optional(b, item, "id")?.is_some() {
            surviving = true;
        }
    }
    if surviving {
        retained.extend(current);
    }
    let mut output = Vec::new();
    for (index, item) in items.into_iter().enumerate() {
        if eq(b, &item, "type", "reasoning")?
            && optional(b, &item, "id")?.is_some()
            && linked.contains(&index)
            && !retained.contains(&index)
        {
            output.push(spread_without(b, &item, &["id"])?);
            continue;
        }
        if eq(b, &item, "type", "computer_call")? {
            let actions = b.get(&item, "actions")?;
            let actions = if b.is_nullish(&actions)? {
                let action = b.get(&item, "action")?;
                if b.0.heap().truthy(&action)? { array(b, [action])? } else { array(b, [])? }
            } else {
                actions
            };
            let args = b.object([("actions", actions)])?;
            output.push(b.object([
                ("type", b.text("function_call")?),
                ("call_id", b.get(&item, "call_id")?),
                ("name", b.text("computer")?),
                ("arguments", b.text(b.plain_json(&args)?)?),
                ("status", b.get(&item, "status")?),
            ])?);
            continue;
        }
        if eq(b, &item, "type", "computer_call_output")? {
            let screenshot = b.get(&item, "output")?;
            let url = optional(b, &screenshot, "image_url")?.filter(|text| !text.units().is_empty());
            let image = if let Some(url) = url {
                Some(b.object([
                    ("type", b.text("input_image")?),
                    ("detail", b.text(if original { "original" } else { "auto" })?),
                    ("image_url", b.text(url)?),
                ])?)
            } else if let Some(id) = optional(b, &screenshot, "file_id")?.filter(|text| !text.units().is_empty()) {
                Some(b.object([
                    ("type", b.text("input_image")?),
                    ("detail", b.text(if original { "original" } else { "auto" })?),
                    ("file_id", b.text(id)?),
                ])?)
            } else {
                None
            };
            output.push(b.object([
                ("type", b.text("function_call_output")?),
                ("call_id", b.get(&item, "call_id")?),
                ("output", b.text(if image.is_some() { "(see attached image)" } else { "" })?),
            ])?);
            if let Some(image) = image {
                output.push(b.object([
                    ("role", b.text("user")?),
                    (
                        "content",
                        array(
                            b,
                            [
                                b.object([
                                    ("type", b.text("input_text")?),
                                    ("text", b.text("Attached image from computer tool result:")?),
                                ])?,
                                image,
                            ],
                        )?,
                    ),
                ])?);
            }
            continue;
        }
        output.push(item);
    }
    Ok(output)
}
fn codex_unroll_message(b: &GraphBuilder, message: &ValueHandle, assistant: bool) -> Result<ValueHandle> {
    if !assistant {
        let metadata = b.get(message, "providerMetadata")?;
        if !b.is_nullish(&metadata)? && eq(b, &metadata, "type", "computer")? {
            let result = b.0.heap().spread(message, b.0.runtime())?;
            b.0.heap().delete_own(&result, &"providerMetadata".into(), b.0.runtime())?;
            return Ok(result);
        }
        return Ok(message.clone());
    }
    // The lexical changed cell is a retained Graph capture. An overridden map
    // may retain the arrow; invocation always borrows its current runtime.
    let changed = b.object([("changed", b.0.heap().boolean(false)?)])?;
    let content =
        callbacks::apply(b, &b.get(message, "content")?, "map", &callbacks::CODEX_COMPUTER_BLOCK, &[changed.clone()])?;
    if b.0.heap().truthy(&b.get(&changed, "changed")?)? {
        let next = b.0.heap().spread(message, b.0.runtime())?;
        b.define(&next, "content", content)?;
        Ok(next)
    } else {
        Ok(message.clone())
    }
}
fn escape_replayed(b: &GraphBuilder, items: Vec<ValueHandle>, enabled: bool) -> Result<Vec<ValueHandle>> {
    if !enabled {
        return Ok(items);
    }
    let mut output = Vec::new();
    for item in items {
        let output_or_call =
            eq(b, &item, "type", "function_call_output")? || eq(b, &item, "type", "custom_tool_call_output")?;
        let function = eq(b, &item, "type", "function_call")?;
        let custom = eq(b, &item, "type", "custom_tool_call")?;
        let message = optional(b, &item, "type")?.is_none_or(|kind| kind.equals_ascii("message"));
        let key = if output_or_call {
            "output"
        } else if function {
            "arguments"
        } else if custom {
            "input"
        } else if message {
            "content"
        } else {
            output.push(item);
            continue;
        };
        let value = b.get(&item, key)?;
        let assistant = message && eq(b, &item, "role", "assistant")?;
        let rewritten = if let ValueSnapshot::String(text) = b.0.heap().snapshot(&value)? {
            if assistant {
                output.push(item);
                continue;
            }
            b.text(escaped(&text, true, function))?
        } else if b.0.heap().kind(&value)? == ValueKind::Array && !function && !custom {
            if assistant && !b.0.heap().has(&item, &"status".into(), b.0.runtime())? {
                output.push(item);
                continue;
            }
            let mut parts = Vec::new();
            for part in b.members(&value)? {
                let text_key = if eq(b, &part, "type", if assistant { "output_text" } else { "input_text" })? {
                    Some("text")
                } else if assistant && eq(b, &part, "type", "refusal")? {
                    Some("refusal")
                } else {
                    None
                };
                if let Some(key) = text_key {
                    let next = b.0.heap().spread(&part, b.0.runtime())?;
                    b.define(&next, key, b.text(escaped(&string(b, &part, key)?, true, false))?)?;
                    parts.push(next);
                } else {
                    parts.push(part);
                }
            }
            array(b, parts)?
        } else {
            output.push(item);
            continue;
        };
        let next = b.0.heap().spread(&item, b.0.runtime())?;
        b.define(&next, key, rewritten)?;
        output.push(next);
    }
    Ok(output)
}
fn parse_reasoning(b: &GraphBuilder, block: &ValueHandle) -> Result<Option<ValueHandle>> {
    callbacks::parse_reasoning(execution(b), &b.get(block, "thinkingSignature")?)
}
fn text_signature(b: &GraphBuilder, block: &ValueHandle) -> Result<(Option<WireString>, Option<WireString>)> {
    let Some(signature) = optional(b, block, "textSignature")? else { return Ok((None, None)) };
    if signature.units().is_empty() {
        return Ok((None, None));
    }
    if starts(&signature, "{") {
        if let Ok(parsed) = b.0.heap().parse_strict_units(&signature) {
            if matches!(b.0.heap().kind(&parsed)?, ValueKind::Object) {
                let version = b.get(&parsed, "v")?;
                if matches!(b.0.heap().snapshot(&version)?,ValueSnapshot::Number(v) if v==1.0) {
                    let phase = optional(b, &parsed, "phase")?
                        .filter(|phase| phase.equals_ascii("commentary") || phase.equals_ascii("final_answer"));
                    if let Some(id) = optional(b, &parsed, "id")? {
                        return Ok((Some(id), phase));
                    }
                }
            }
        }
    }
    Ok((Some(signature), None))
}
#[allow(clippy::too_many_arguments)]
fn assistant(
    b: &GraphBuilder,
    model: &Model,
    context: &GraphProviderContext,
    message: &ValueHandle,
    msg_index: usize,
    include: bool,
    options: &super::super::RequestOptions,
    policy: &GraphResponsesRequestPolicy,
    calls: &mut Calls,
    names: &HashMap<WireString, WireString>,
    custom: bool,
) -> Result<Vec<ValueHandle>> {
    let replayable = include
        && !eq(b, message, "stopReason", "error")?
        && b.0.heap().truthy(&callbacks::apply(
            b,
            &b.get(message, "content")?,
            "some",
            &callbacks::REPLAYABLE,
            &[],
        )?)?;
    let different = optional(b, message, "model")? != Some(model.id.clone().into())
        && optional(b, message, "provider")? == Some(model.provider.clone().into())
        && optional(b, message, "api")? == Some(model.api.clone().into());
    let require = !eq(b, message, "stopReason", "error")?
        && (policy.requires_reasoning_for_all_turns
            || (policy.requires_reasoning_for_tool_calls
                && b.0.heap().truthy(&callbacks::apply(
                    b,
                    &b.get(message, "content")?,
                    "some",
                    &callbacks::TOOL_CALL,
                    &[],
                )?)?));
    let mut carried = Vec::new();
    let mut reasoning_id = None;
    let mut emitted = false;
    let mut unsigned = 0;
    let mut output = Vec::new();
    for block in b.members(&b.get(message, "content")?)? {
        if eq(b, &block, "type", "thinking")? && !eq(b, message, "stopReason", "error")? {
            if require {
                if reasoning_id.is_none() {
                    reasoning_id = optional(b, &block, "itemId")?;
                }
                if callbacks::trim_nonempty(execution(b), &b.get(&block, "thinking")?)? {
                    carried.push(string(b, &block, "thinking")?);
                }
            }
            if include {
                if let Some(item) = parse_reasoning(b, &block)? {
                    output.push(item);
                    emitted = true;
                }
            }
            continue;
        }
        if eq(b, &block, "type", "text")? {
            let (mut id, phase) = text_signature(b, &block)?;
            if id.as_ref().is_none_or(|id| id.units().is_empty()) {
                if replayable {
                    id = Some(if unsigned == 0 {
                        format!("msg_{msg_index}").into()
                    } else {
                        format!("msg_{msg_index}_{unsigned}").into()
                    });
                    unsigned += 1;
                }
            } else if !policy.preserve_assistant_message_ids && !replayable {
                id = None;
            } else if id.as_ref().is_some_and(|id| id.units().len() > 64) {
                id = Some(concat(&"msg_".into(), &hash(context, id.as_ref().unwrap())?));
            }
            let content = array(
                b,
                [b.object([
                    ("type", b.text("output_text")?),
                    ("text", callbacks::well_formed(execution(b), &b.get(&block, "text")?)?),
                    ("annotations", array(b, [])?),
                ])?],
            )?;
            let item = b.object([
                ("type", b.text("message")?),
                ("role", b.text("assistant")?),
                ("content", content),
                ("status", b.text("completed")?),
            ])?;
            if let Some(id) = id {
                if !id.units().is_empty() {
                    b.define(&item, "id", b.text(id)?)?;
                }
            }
            if let Some(phase) = phase {
                b.define(&item, "phase", b.text(phase)?)?;
            }
            output.push(item);
            continue;
        }
        if !eq(b, &block, "type", "toolCall")? {
            continue;
        }
        let metadata = b.get(&block, "providerMetadata")?;
        if !b.is_nullish(&metadata)? && eq(b, &metadata, "type", "computer")? {
            let (call, _) = normalize_id(context, &string(b, &block, "id")?, "ctc")?;
            if !options.supports_computer_use {
                let text = concat(&concat(&"[Previous computer call; call_id=".into(), &call), &"]: ".into());
                output.push(note(b, concat(&text, &b.plain_json(&b.get(&metadata, "actions")?)?))?);
                continue;
            }
            calls.known.insert(call.clone());
            calls.computer.insert(call.clone());
            let heap = b.0.heap();
            let actions = heap.structured_clone_json(&b.get(&metadata, "actions")?, b.0.runtime())?;
            let checks = heap.structured_clone_json(&b.get(&metadata, "pendingSafetyChecks")?, b.0.runtime())?;
            output.push(b.object([
                ("type", b.text("computer_call")?),
                ("id", b.get(&metadata, "providerItemId")?),
                ("call_id", b.text(call)?),
                ("actions", actions),
                ("pending_safety_checks", checks),
                ("status", b.text("completed")?),
            ])?);
            continue;
        }
        let wire = optional(b, &block, "customWireName")?.filter(|name| !name.units().is_empty());
        let (call, item) = normalize_id(context, &string(b, &block, "id")?, if wire.is_some() { "ctc" } else { "fc" })?;
        let item = if (!replayable || different) && ["fc_", "fcr_", "ctc_"].iter().any(|prefix| starts(&item, prefix)) {
            None
        } else {
            Some(item)
        };
        calls.known.insert(call.clone());
        let arguments = b.get(&block, "arguments")?;
        let projected = if let Some(wire) = wire.as_ref().filter(|_| custom) {
            let input = if b.is_nullish(&arguments)? {
                b.text("")?
            } else {
                let value = b.get(&arguments, "input")?;
                if b.0.heap().kind(&value)? == ValueKind::String { b.get(&arguments, "input")? } else { b.text("")? }
            };
            calls.custom.insert(call.clone());
            b.object([
                ("type", b.text("custom_tool_call")?),
                ("call_id", b.text(call)?),
                ("name", b.text(wire.clone())?),
                ("input", input),
            ])?
        } else {
            let name = if let Some(wire) = wire { b.text(custom_name(names, wire))? } else { b.get(&block, "name")? };
            let encoded =
                b.0.heap()
                    .stringify(&arguments, StringifyMode::Plain, None, b.0.runtime())?
                    .unwrap_or_else(|| "null".into());
            b.object([
                ("type", b.text("function_call")?),
                ("call_id", b.text(call)?),
                ("name", name),
                ("arguments", b.text(encoded)?),
            ])?
        };
        if let Some(id) = item {
            b.define(&projected, "id", b.text(id)?)?;
        }
        output.push(projected);
    }
    if require && !emitted && !output.is_empty() {
        let carried = join(carried, "\n");
        let text = if carried.units().is_empty() { "reasoning unavailable".into() } else { carried };
        let id = if let Some(id) = reasoning_id {
            id
        } else {
            let seed = concat(&format!("{}:{msg_index}:", model.id).into(), &text);
            concat(&"rs_".into(), &hash(context, &seed)?)
        };
        output.insert(
            0,
            b.object([
                ("type", b.text("reasoning")?),
                ("id", b.text(id)?),
                ("summary", array(b, [])?),
                ("content", array(b, [b.object([("type", b.text("reasoning_text")?), ("text", b.text(text)?)])?])?),
            ])?,
        );
    }
    Ok(output)
}
fn limited(text: WireString) -> WireString {
    if text.units().len() > 16000 { concat(&text.slice_prefix(16000), &"\n...[truncated]".into()) } else { text }
}
fn tool_result(
    b: &GraphBuilder,
    context: &GraphProviderContext,
    message: &ValueHandle,
    options: &super::super::RequestOptions,
    policy: &GraphResponsesRequestPolicy,
    calls: &Calls,
    custom: bool,
) -> Result<ValueHandle> {
    let content = b.get(message, "content")?;
    let filtered = callbacks::apply(b, &content, "filter", &callbacks::TEXT_BLOCK, &[])?;
    let texts = callbacks::apply(b, &filtered, "map", &callbacks::TEXT_VALUE, &[])?;
    let text_result = execution(b).method(&texts, "join", &[b.text("\n")?])?;
    let images =
        b.0.heap().truthy(&callbacks::apply(b, &b.get(message, "content")?, "some", &callbacks::IMAGE_BLOCK, &[])?)?;
    let nonempty = b.0.heap().to_number(&b.get(&text_result, "length")?, b.0.runtime())? > 0.0;
    let raw = if images && !options.supports_images {
        let mut parts = Vec::new();
        if nonempty {
            parts.push(text_result.clone());
        }
        parts.push(b.text("[image omitted: model does not support vision]")?);
        execution(b).method(&array(b, parts)?, "join", &[b.text("\n")?])?
    } else if nonempty {
        text_result
    } else if images {
        b.text("(see attached image)")?
    } else {
        b.text("")?
    };
    let raw = callbacks::well_formed(execution(b), &raw)?;
    let text = escaped(&b.string(&raw)?, policy.escape_control_tokens, false);
    let output = if images && options.supports_images {
        callbacks::apply(
            b,
            &b.get(message, "content")?,
            "map",
            &callbacks::OUTPUT_PART,
            &[
                b.0.heap().boolean(policy.supports_image_detail_original)?,
                b.0.heap().boolean(policy.escape_control_tokens)?,
            ],
        )?
    } else if policy.escape_control_tokens {
        b.text(text.clone())?
    } else {
        raw
    };
    let (call, _) = normalize_id(context, &string(b, message, "toolCallId")?, "fc")?;
    let metadata = b.get(message, "providerMetadata")?;
    if !b.is_nullish(&metadata)? && eq(b, &metadata, "type", "computer")? && !options.supports_computer_use {
        let start = concat(&concat(&"[Previous computer result; call_id=".into(), &call), &"]: ".into());
        return note(b, concat(&start, &b.plain_json(&b.get(&metadata, "screenshot")?)?));
    }
    if calls.computer.contains(&call) {
        if b.is_nullish(&metadata)? || !eq(b, &metadata, "type", "computer")? {
            return note(
                b,
                concat(
                    &concat(
                        &concat(&"[Computer tool failed before a screenshot was produced; call_id=".into(), &call),
                        &"]: ".into(),
                    ),
                    &limited(text),
                ),
            );
        }
        if policy.strict_pairing && !calls.known.contains(&call) {
            return note(b, concat(&concat(&"[Orphan computer result; call_id=".into(), &call), &"]".into()));
        }
        let screenshot = b.0.heap().structured_clone_json(&b.get(&metadata, "screenshot")?, b.0.runtime())?;
        let checks = b.0.heap().structured_clone_json(&b.get(&metadata, "acknowledgedSafetyChecks")?, b.0.runtime())?;
        return b.object([
            ("type", b.text("computer_call_output")?),
            ("call_id", b.text(call)?),
            ("output", screenshot),
            ("acknowledged_safety_checks", checks),
        ]);
    }
    if policy.strict_pairing && !calls.known.contains(&call) {
        let name =
            optional(b, message, "toolName")?.filter(|text| !text.units().is_empty()).unwrap_or_else(|| "tool".into());
        let start = concat(
            &concat(&concat(&concat(&"[Orphan ".into(), &name), &" result; call_id=".into()), &call),
            &"]: ".into(),
        );
        return note(b, concat(&start, &limited(text)));
    }
    b.object([
        (
            "type",
            b.text(if custom && calls.custom.contains(&call) {
                "custom_tool_call_output"
            } else {
                "function_call_output"
            })?,
        ),
        ("call_id", b.text(call)?),
        ("output", output),
    ])
}
fn call_kind(b: &GraphBuilder, item: &ValueHandle, output: bool) -> Result<Option<&'static str>> {
    for kind in ["function", "custom", "computer"] {
        let name = if output { format!("{kind}_call_output") } else { format!("{kind}_call") };
        if eq(b, item, "type", &name)? {
            return Ok(Some(kind));
        }
    }
    Ok(None)
}
fn orphan_note(b: &GraphBuilder, item: &ValueHandle, id: &WireString, kind: &str, codex: bool) -> Result<ValueHandle> {
    let raw = b.get(item, "output")?;
    let text = match b.0.heap().snapshot(&raw)? {
        ValueSnapshot::String(text) => text,
        ValueSnapshot::Undefined | ValueSnapshot::Null if !codex => "".into(),
        _ => match b.0.heap().stringify(&raw, StringifyMode::Plain, None, b.0.runtime()) {
            Ok(Some(text)) => text,
            Ok(None) if codex => "undefined".into(),
            Ok(None) => "".into(),
            Err(_) => b.0.heap().to_string(&b.get(item, "output")?, b.0.runtime())?,
        },
    };
    let name = if codex {
        optional(b, item, "name")?.unwrap_or_else(|| "tool".into())
    } else if kind == "computer" {
        "computer".into()
    } else {
        "tool".into()
    };
    let start = concat(
        &concat(
            &concat(
                &concat(&format!("[{} ", if codex { "Previous" } else { "Orphan" }).into(), &name),
                &" result; call_id=".into(),
            ),
            id,
        ),
        &"]: ".into(),
    );
    note(b, concat(&start, &limited(text)))
}
fn placeholder(
    b: &GraphBuilder,
    item: ValueHandle,
    id: WireString,
    kind: &str,
    result: &mut Vec<ValueHandle>,
) -> Result<()> {
    if kind == "computer" {
        result.push(note(
            b,
            concat(
                &concat(&"[Computer call interrupted before a screenshot was recorded; call_id=".into(), &id),
                &"]".into(),
            ),
        )?);
    } else {
        result.push(item);
        result.push(b.object([
            ("type", b.text(if kind == "custom" { "custom_tool_call_output" } else { "function_call_output" })?),
            ("call_id", b.text(id)?),
            (
                "output",
                b.text("[No tool output recorded: the tool call was interrupted before it produced a result.]")?,
            ),
        ])?);
    }
    Ok(())
}
fn batch_kind(b: &GraphBuilder, item: &ValueHandle) -> Result<&'static str> {
    if call_kind(b, item, false)?.is_some() {
        Ok("call")
    } else if call_kind(b, item, true)?.is_some() {
        Ok("output")
    } else if eq(b, item, "type", "message")? && eq(b, item, "role", "assistant")? {
        Ok("message")
    } else {
        Ok("other")
    }
}
fn hoist(b: &GraphBuilder, items: Vec<ValueHandle>) -> Result<Vec<ValueHandle>> {
    let mut moved = HashSet::new();
    let mut insert: HashMap<usize, Vec<usize>> = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        if batch_kind(b, item)? != "output" {
            continue;
        }
        if index > 0 && batch_kind(b, &items[index - 1])? == "output" {
            continue;
        }
        let mut start = index;
        let mut saw = false;
        let mut messages = Vec::new();
        while start > 0 {
            match batch_kind(b, &items[start - 1])? {
                "call" => saw = true,
                "message" => messages.push(start - 1),
                _ => break,
            }
            start -= 1;
        }
        if saw && !messages.is_empty() {
            messages.reverse();
            for index in &messages {
                moved.insert(*index);
            }
            insert.entry(start).or_default().extend(messages);
        }
    }
    let mut output = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if let Some(messages) = insert.get(&index) {
            for index in messages {
                output.push(items[*index].clone());
            }
        }
        if !moved.contains(&index) {
            output.push(item.clone());
        }
    }
    Ok(output)
}
pub(super) fn repair(
    b: &GraphBuilder,
    items: Vec<ValueHandle>,
    codex: bool,
    repair_outputs: bool,
) -> Result<Vec<ValueHandle>> {
    if codex {
        let mut calls = HashMap::new();
        let mut outputs = HashMap::new();
        for item in &items {
            if let Some(id) = optional(b, item, "call_id")? {
                if let Some(kind) = call_kind(b, item, false)? {
                    calls.insert(id.clone(), kind);
                }
                if let Some(kind) = call_kind(b, item, true)? {
                    outputs.insert(id, kind);
                }
            }
        }
        let mut result = Vec::new();
        for item in items {
            let id = optional(b, &item, "call_id")?;
            let output_kind = call_kind(b, &item, true)?;
            let call_kind = call_kind(b, &item, false)?;
            if let (Some(id), Some(kind)) = (id.as_ref(), output_kind) {
                if calls.get(id).copied() != Some(kind) {
                    result.push(orphan_note(b, &item, id, kind, true)?);
                    continue;
                }
            }
            if let (Some(id), Some(kind)) = (id, call_kind) {
                if outputs.get(&id).copied() != Some(kind) {
                    placeholder(b, item, id, kind, &mut result)?;
                    continue;
                }
            }
            result.push(item);
        }
        return Ok(result);
    }
    let items = hoist(b, items)?;
    let mut preceding = HashSet::new();
    let mut outputs = Vec::new();
    for item in items {
        let id = optional(b, &item, "call_id")?;
        if let (Some(id), Some(kind)) = (id.as_ref(), call_kind(b, &item, false)?) {
            if !id.units().is_empty() {
                preceding.insert((kind, id.clone()));
            }
        }
        if repair_outputs {
            if let (Some(id), Some(kind)) = (id.as_ref(), call_kind(b, &item, true)?) {
                if !id.units().is_empty() && !preceding.contains(&(kind, id.clone())) {
                    outputs.push(orphan_note(b, &item, id, kind, false)?);
                    continue;
                }
            }
        }
        outputs.push(item);
    }
    let mut later = HashSet::new();
    let mut orphans = HashSet::new();
    for (index, item) in outputs.iter().enumerate().rev() {
        let id = optional(b, item, "call_id")?;
        if let (Some(id), Some(kind)) = (id.as_ref(), call_kind(b, item, true)?) {
            if !id.units().is_empty() {
                later.insert((kind, id.clone()));
            }
        }
        if let (Some(id), Some(kind)) = (id, call_kind(b, item, false)?) {
            if !id.units().is_empty() && !later.contains(&(kind, id)) {
                orphans.insert(index);
            }
        }
    }
    let mut result = Vec::new();
    for (index, item) in outputs.into_iter().enumerate() {
        if orphans.contains(&index) {
            if let (Some(id), Some(kind)) = (optional(b, &item, "call_id")?, call_kind(b, &item, false)?) {
                placeholder(b, item, id, kind, &mut result)?;
                continue;
            }
        }
        result.push(item);
    }
    Ok(result)
}
pub(super) fn project(
    b: &GraphBuilder,
    model: &Model,
    context: &GraphProviderContext,
    options: &super::super::RequestOptions,
    policy: &GraphResponsesRequestPolicy,
    flavor: GraphResponsesFlavor,
) -> Result<ValueHandle> {
    let history = OpenAIHistoryPolicy {
        prefix_binding: options
            .reasoning_policy
            .as_ref()
            .is_some_and(crate::OpenAIReasoningPolicy::history_prefix_binding),
        reads_foreign_thinking: false,
    };
    let original_messages = context.original_field(b.0.as_ref(), "messages")?;
    let redacted = super::redaction::messages(execution(b), &original_messages, policy.credential_redaction_enabled)?;
    // This existing transformation still returns a derived handle sequence;
    // its remaining source Array.map contracts must be ported independently.
    // The original Context/messages container is never rebuilt from legacy Vec.
    let observed = b.members(&redacted)?;
    let transformed = graph_history::transform_messages(b, &observed, 64, "_dup", |messages| {
        graph_history::normalize_openai_history(
            b,
            messages,
            model,
            &history,
            |id, source| normalize_transform(b, model, context, id, source, flavor),
            |text| graph_history::render_demoted(text, context.history_demotion_dialect),
        )
    })?;
    let codex = flavor == GraphResponsesFlavor::Codex;
    let custom = codex || options.apply_patch_tool_type.as_deref() == Some("freeform");
    let names = custom_names(b, context)?;
    let mut output = Vec::new();
    let mut calls = Calls::default();
    let mut provisional = Vec::new();
    let mut msg_index = 0;
    for message in transformed {
        let user = eq(b, &message, "role", "user")?;
        let developer = if !user { eq(b, &message, "role", "developer")? } else { false };
        if user || developer {
            if let Some(payload) = payload(b, &message, model, false)? {
                let original = b.get(&payload, "items")?;
                let replay = codex
                    || options.native_history_replay.unwrap_or(true)
                    || b.0.heap().truthy(&callbacks::apply(b, &original, "some", &callbacks::COMPACTION, &[])?)?;
                if replay {
                    // Codex client native input is intentionally separate from
                    // Generic replay sanitization; its fixed redaction binding
                    // is required before native input may be replayed.
                    let filtered = if policy.filter_reasoning_history && !codex {
                        callbacks::apply(b, &original, "filter", &callbacks::NON_REASONING, &[])?
                    } else {
                        original
                    };
                    let items = if codex {
                        let redacted = super::redaction::native_items(
                            execution(b),
                            &filtered,
                            policy.credential_redaction_enabled,
                        )?;
                        b.members(&redacted)?
                    } else {
                        sanitize_replay(b, context, b.members(&filtered)?, options, policy, &mut provisional)?
                    };
                    let items = if codex && !options.supports_computer_use {
                        codex_unroll_items(b, items, policy.supports_image_detail_original)?
                    } else {
                        adapt(b, items, custom, options.supports_computer_use, &names)?
                    };
                    let items = escape_replayed(b, items, policy.escape_control_tokens)?;
                    output.extend(items);
                    collect(b, &output, &mut calls, !codex)?;
                    msg_index += 1;
                    continue;
                }
            }
            let Some(content) = input_content(b, &message, options, policy)? else { continue };
            let content = if !codex && policy.developer_string_content && developer {
                if let Some(text) = optional(b, &message, "content")? {
                    b.text(escaped(&graph_history::well_formed(&text), policy.escape_control_tokens, false))?
                } else {
                    content
                }
            } else {
                content
            };
            output.push(b.object([
                ("role", b.text(if codex && developer { "developer" } else { "user" })?),
                ("content", content),
            ])?);
        } else if eq(b, &message, "role", "assistant")? {
            let mut suppress = false;
            if let Some(payload) = payload(b, &message, model, true)? {
                let original = b.get(&payload, "items")?;
                let filtered = if policy.filter_reasoning_history && !codex {
                    callbacks::apply(b, &original, "filter", &callbacks::NON_REASONING, &[])?
                } else {
                    original
                };
                let items = b.members(&filtered)?;
                let items = if codex {
                    let mut replay_policy = policy.clone();
                    replay_policy.supports_image_detail_original = true;
                    let mut replay_options = options.clone();
                    replay_options.supports_computer_use = true;
                    sanitize_replay(b, context, items, &replay_options, &replay_policy, &mut provisional)?
                } else {
                    sanitize_replay(b, context, items, options, policy, &mut provisional)?
                };
                let mut visible = false;
                for item in &items {
                    if visible_item(b, item)? {
                        visible = true;
                        break;
                    }
                }
                if visible && (codex || options.native_history_replay.unwrap_or(true)) {
                    let items = if codex && !options.supports_computer_use {
                        codex_unroll_items(b, items, policy.supports_image_detail_original)?
                    } else {
                        adapt(b, items, custom, options.supports_computer_use, &names)?
                    };
                    let items = escape_replayed(b, items, policy.escape_control_tokens)?;
                    if b.0.heap().truthy(&b.get(&payload, "dt")?)? {
                        output.extend(items);
                    } else {
                        output = items;
                        if !codex {
                            calls.custom.clear();
                            calls.computer.clear();
                        }
                    }
                    collect(b, &output, &mut calls, !codex)?;
                    msg_index += 1;
                    continue;
                }
                if !visible {
                    suppress = true;
                }
            }
            let include = !suppress
                && (codex || (options.native_history_replay.unwrap_or(true) && !policy.filter_reasoning_history));
            let unrolled = if codex && !options.supports_computer_use {
                codex_unroll_message(b, &message, true)?
            } else {
                message.clone()
            };
            let items = assistant(
                b, model, context, &unrolled, msg_index, include, options, policy, &mut calls, &names, custom,
            )?;
            let items = if suppress {
                let mut visible = Vec::new();
                for item in items {
                    if visible_item(b, &item)? {
                        visible.push(item);
                    }
                }
                visible
            } else {
                items
            };
            output.extend(escape_replayed(b, items, policy.escape_control_tokens)?);
        } else if eq(b, &message, "role", "toolResult")? {
            let unrolled = if codex && !options.supports_computer_use {
                codex_unroll_message(b, &message, false)?
            } else {
                message.clone()
            };
            output.push(tool_result(b, context, &unrolled, options, policy, &calls, custom)?);
        }
        msg_index += 1;
    }
    if !codex {
        output = repair(b, output, false, policy.repair_orphan_outputs)?;
        let linked = linked_reasoning(b, &output, true)?;
        for (index, item) in output.iter_mut().enumerate() {
            if provisional.iter().any(|value| value.same_identity(item))
                && !linked.contains(&index)
                && optional(b, item, "id")?.is_some()
            {
                *item = spread_without(b, item, &["id"])?;
            }
        }
    }
    array(b, output)
}
