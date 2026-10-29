//! Original Graph buildParams/Codex transformer. Fixed OMP 596f2da (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
use super::{
    GraphResponsesFlavor, GraphResponsesRequestPolicy,
    callbacks::{self, execution},
    grammar, history, schema, strict_schema,
};
use crate::actual_provider::{GraphBuilder, GraphProviderContext, trim_empty};
use crate::{Model, ToolChoice};
use ara_rpc::{
    WireString,
    value_heap::{ValueError, ValueHandle, ValueKind},
};
type Result<T> = std::result::Result<T, ValueError>;
fn delete(b: &GraphBuilder, root: &ValueHandle, key: &str) -> Result<()> {
    if !b.0.heap().delete_own(root, &key.into(), b.0.runtime())? {
        return Err(ValueError::TypeError(format!("Cannot delete provider request field {key}")));
    }
    Ok(())
}
fn text_message(b: &GraphBuilder, role: &str, text: WireString) -> Result<ValueHandle> {
    b.object([
        ("type", b.text("message")?),
        ("role", b.text(role)?),
        ("content", history::array(b, [b.object([("type", b.text("input_text")?), ("text", b.text(text)?)])?])?),
    ])
}
fn description(b: &GraphBuilder, tool: &ValueHandle) -> Result<ValueHandle> {
    let value = b.get(tool, "description")?;
    if b.0.heap().truthy(&value)? { Ok(value) } else { b.text("") }
}
fn native_computer(b: &GraphBuilder, tool: &ValueHandle) -> Result<bool> {
    let native = b.get(tool, "native")?;
    Ok(!b.is_nullish(&native)? && history::eq(b, &native, "type", "computer")?)
}
fn tools(
    b: &GraphBuilder,
    context: &GraphProviderContext,
    options: &super::super::RequestOptions,
    policy: &GraphResponsesRequestPolicy,
) -> Result<Vec<ValueHandle>> {
    let mut output = Vec::new();
    let original = context.original_field(b.0.as_ref(), "tools")?;
    if b.is_nullish(&original)? {
        return Ok(output);
    }
    execution(b).for_of(&original,|tool|{
        if native_computer(b,tool)?&&options.supports_computer_use{output.push(b.object([("type",b.text("computer")?)])?);return Ok(true);}
        let custom=b.get(tool,"customFormat")?;
        if options.apply_patch_tool_type.as_deref()==Some("freeform")&&b.0.heap().truthy(&custom)?{
            let name=b.get(tool,"customWireName")?;let name=if b.is_nullish(&name)?{b.get(tool,"name")?}else{name};
            let syntax=b.get(&custom,"syntax")?;let definition=b.get(&custom,"definition")?;
            let definition=grammar::compact(execution(b),&syntax,&definition)?;
            let format=b.object([("type",b.text("grammar")?),("syntax",syntax),("definition",definition)])?;
            output.push(b.object([("type",b.text("custom")?),("name",name),("description",description(b,tool)?),("format",format)])?);return Ok(true);
        }
        let strict_mode=!policy.disable_strict_tools&&policy.supports_strict_mode;
        let strict=!policy.no_strict&&strict_mode&&!matches!(b.0.heap().snapshot(&b.get(tool,"strict")?)?,ara_rpc::value_heap::ValueSnapshot::Boolean(false));
        let base=crate::graph_tool_wire_schema(b.0.as_ref(),tool)?;
        let sanitized=schema::sanitize(execution(b),&base)?;
        let provider=if policy.reject_root_object_union{crate::flatten_graph_exclusive_required_root_union(b.0.as_ref(),&sanitized)?}else{sanitized};
        if policy.tool_schema_flavor.as_ref().is_some_and(|flavor|flavor.equals_ascii("moonshot-mfjs")){return Err(ValueError::UnsupportedOperation("Fixed Graph Moonshot MFJS post-Responses schema normalization remains unbound"));}
        let (parameters,effective_strict)=strict_schema::adapt(execution(b),&provider,strict)?;
        if let Some(path)=schema::violation(execution(b),&parameters,&"#".into(),policy.reject_root_object_union)?{
            let name=b.get(tool,"name")?;
            if let Some(diagnostic)=&policy.on_quarantine{diagnostic.quarantined(&name,&path,b.0.as_ref())?;}
            else{let name=b.0.heap().to_string(&name,b.0.runtime())?;eprintln!("Tool \"{}\" omitted from the openai-responses request: its parameter schema is invalid for this provider at {} (an enum/const value cannot match its declared type). Other tools are unaffected.",String::from_utf16_lossy(name.units()),String::from_utf16_lossy(path.units()));}
            return Ok(true);
        }
        let projected=b.object([("type",b.text("function")?),("name",b.get(tool,"name")?),("description",description(b,tool)?),("parameters",parameters)])?;
        if effective_strict{b.define(&projected,"strict",b.0.heap().boolean(true)?)?;}
        else if !policy.no_strict&&strict_mode&&matches!(b.0.heap().snapshot(&b.get(tool,"strict")?)?,ara_rpc::value_heap::ValueSnapshot::Boolean(false)){b.define(&projected,"strict",b.0.heap().boolean(false)?)?;}
        output.push(projected);Ok(true)
    })?;
    Ok(output)
}
fn choice(
    b: &GraphBuilder,
    context: &GraphProviderContext,
    emitted: &[ValueHandle],
    options: &super::super::RequestOptions,
) -> Result<Option<ValueHandle>> {
    let Some(choice) = &options.tool_choice else { return Ok(None) };
    if emitted.is_empty() {
        return Ok(None);
    }
    let simple = match choice {
        ToolChoice::Auto => Some("auto"),
        ToolChoice::None => Some("none"),
        ToolChoice::Any | ToolChoice::Required => Some("required"),
        _ => None,
    };
    if let Some(value) = simple {
        return Ok(Some(b.text(value)?));
    }
    let tools = context.original_field(b.0.as_ref(), "tools")?;
    if b.is_nullish(&tools)? {
        return Ok(None);
    }
    for tool in b.members(&tools)?.iter() {
        let native = native_computer(b, tool)?;
        let name = history::optional(b, tool, "name")?;
        let wire = history::optional(b, tool, "customWireName")?;
        let matches = match choice {
            ToolChoice::Computer => native,
            _ => choice.logical_name().is_some_and(|requested| {
                name.as_ref().is_some_and(|name| name == &WireString::from(requested))
                    || wire.as_ref().is_some_and(|name| name == &WireString::from(requested))
            }),
        };
        if !matches {
            continue;
        }
        if native && options.supports_computer_use {
            for emitted in emitted {
                if history::eq(b, emitted, "type", "computer")? {
                    return Ok(Some(b.object([("type", b.text("computer")?)])?));
                }
            }
            continue;
        }
        let custom = options.apply_patch_tool_type.as_deref() == Some("freeform")
            && !b.is_nullish(&b.get(tool, "customFormat")?)?;
        let mapped = if custom { wire.or(name) } else { name };
        let Some(mapped) = mapped else { continue };
        for emitted in emitted {
            if history::optional(b, emitted, "name")? == Some(mapped.clone()) {
                return Ok(Some(b.object([
                    ("type", b.text(if custom { "custom" } else { "function" })?),
                    ("name", b.text(mapped.clone())?),
                ])?));
            }
        }
    }
    Ok(None)
}
fn metadata_choice(b: &GraphBuilder, choice: Option<&ValueHandle>) -> Result<Option<serde_json::Value>> {
    match choice {
        None => Ok(None),
        Some(choice) => Ok(Some(
            if let Some(text) = match b.0.heap().snapshot(choice)? {
                ara_rpc::value_heap::ValueSnapshot::String(text) => Some(text),
                _ => None,
            } {
                serde_json::json!(
                    text.to_utf8().map_err(|_| ValueError::UnsupportedOperation("Derived tool-choice spelling"))?
                )
            } else {
                serde_json::json!({"type":"function","name":"derived-forcing-marker"})
            },
        )),
    }
}
pub(super) fn build(
    b: &GraphBuilder,
    model: &Model,
    context: &GraphProviderContext,
    options: &super::super::RequestOptions,
    policy: &GraphResponsesRequestPolicy,
    flavor: GraphResponsesFlavor,
) -> Result<ValueHandle> {
    context.check_lineage(b.0.as_ref())?;
    let codex = flavor == GraphResponsesFlavor::Codex;
    let projected_tools = tools(b, context, options, policy)?;
    let projected_choice = choice(b, context, &projected_tools, options)?;
    // This serde value contains only derived request knobs and newly generated
    // Juice scaffolding. Original Context/history/schema/arguments never enter it.
    let mut knobs = serde_json::json!({"model":model.id,"stream":true,"store":false,"input":[]});
    if let Some(choice) = metadata_choice(b, projected_choice.as_ref())? {
        knobs["tool_choice"] = choice;
    }
    if !options.include.is_empty() {
        let mut seen = std::collections::HashSet::new();
        knobs["include"] =
            serde_json::json!(options.include.iter().filter(|item| seen.insert((*item).clone())).collect::<Vec<_>>());
    }
    if codex {
        crate::reasoning::apply_codex(&mut knobs, model, options.reasoning.as_ref(), options.reasoning_policy.as_ref())
            .map_err(|error| ValueError::TypeError(error.to_string()))?;
        let mut include = options.include.clone();
        include.push("reasoning.encrypted_content".into());
        let mut seen = std::collections::HashSet::new();
        include.retain(|item| seen.insert(item.clone()));
        knobs["include"] = serde_json::json!(include);
    } else {
        if let Some(max) = options.max_tokens {
            knobs["max_output_tokens"] = serde_json::json!(max);
        }
        if let Some(temp) = options.temperature {
            knobs["temperature"] = serde_json::json!(temp);
        }
        crate::reasoning::apply_responses(
            &mut knobs,
            model,
            options.reasoning.as_ref(),
            options.reasoning_policy.as_ref(),
        )
        .map_err(|error| ValueError::TypeError(error.to_string()))?;
    }
    if let Some(key) = &options.prompt_cache_key {
        knobs["prompt_cache_key"] = serde_json::json!(key);
    }
    let derived_scaffolding = knobs.as_object_mut().unwrap().remove("input").unwrap();
    let trailing_scaffolding_items = derived_scaffolding.as_array().map_or(0, Vec::len);
    let keep_choice = knobs.get("tool_choice").is_some();
    knobs.as_object_mut().unwrap().remove("tool_choice");
    let body = b.derived(&knobs)?;
    let mut input = history::project(b, model, context, options, policy, flavor)?;
    let prompt_array = callbacks::system_prompts(
        execution(b),
        &context.original_field(b.0.as_ref(), "systemPrompt")?,
        policy.credential_redaction_enabled,
    )?;
    let prompts = b.members(&prompt_array)?.iter().map(|prompt| b.string(prompt)).collect::<Result<Vec<_>>>()?;
    if codex {
        let mut items = Vec::new();
        for item in b.members(&input)? {
            if history::eq(b, &item, "type", "item_reference")? {
                continue;
            }
            if history::eq(b, &item, "type", "computer_call")? || b.is_nullish(&b.get(&item, "id")?)? {
                items.push(item);
            } else {
                items.push(b.0.heap().spread_excluding(&item, &["id".into()], b.0.runtime())?);
            }
        }
        items = history::repair(b, items, true, true)?;
        let mut prefix = Vec::new();
        for prompt in prompts.iter().skip(1) {
            prefix.push(text_message(b, "developer", prompt.clone())?);
        }
        prefix.extend(items);
        let mut final_instruction = prompts.iter().skip(1).rev().find(|prompt| !trim_empty(prompt)).cloned();
        if final_instruction.is_none() {
            for item in prefix.iter().rev() {
                if !history::eq(b, item, "role", "developer")? {
                    continue;
                }
                let content = b.get(item, "content")?;
                if let ara_rpc::value_heap::ValueSnapshot::String(text) = b.0.heap().snapshot(&content)? {
                    if !trim_empty(&text) {
                        final_instruction = Some(text);
                        break;
                    }
                } else if b.0.heap().kind(&content)? == ValueKind::Array {
                    for part in b.members(&content)?.iter().rev() {
                        if history::eq(b, part, "type", "input_text")? {
                            if let Some(text) = history::optional(b, part, "text")? {
                                if !trim_empty(&text) {
                                    final_instruction = Some(text);
                                    break;
                                }
                            }
                        }
                    }
                    if final_instruction.is_some() {
                        break;
                    }
                }
            }
        }
        if let Some(prompt) = prompts.first() {
            b.define(&body, "instructions", b.text(prompt.clone())?)?;
            if final_instruction.is_none() {
                final_instruction = Some(prompt.clone());
            }
        }
        if let Some(instruction) = final_instruction {
            let mut visible = false;
            for item in &prefix {
                if !history::eq(b, item, "role", "developer")? {
                    visible = true;
                    break;
                }
            }
            if !visible {
                prefix.push(text_message(b, "user", instruction)?);
            }
        }
        input = history::array(b, prefix)?;
    } else if policy.system_role_developer {
        let mut items = Vec::new();
        for prompt in &prompts {
            items.push(b.object([("role", b.text("developer")?), ("content", b.text(prompt.clone())?)])?);
        }
        items.extend(b.members(&input)?);
        input = history::array(b, items)?;
    } else if !prompts.is_empty() {
        b.define(&body, "instructions", execution(b).method(&prompt_array, "join", &[b.text("\n\n")?])?)?;
    }
    for scaffolding in b.members(&b.derived(&derived_scaffolding)?)? {
        b.push(&input, scaffolding)?;
    }
    b.define(&body, "input", input)?;
    // Temporary protocol metadata. Symbols never enter the HTTP JSON body or
    // the successful structuredCloneJSON baseline. Canonical Context is intact.
    b.0.heap().define_own_data(
        &body,
        super::chain::scaffolding_key(b)?,
        b.0.heap().number(trailing_scaffolding_items as f64)?,
        b.0.runtime(),
    )?;
    if b.0.heap().truthy(&context.original_field(b.0.as_ref(), "tools")?)? && (!codex || !projected_tools.is_empty()) {
        b.define(&body, "tools", history::array(b, projected_tools)?)?;
    }
    if keep_choice {
        if let Some(choice) = projected_choice {
            b.define(&body, "tool_choice", choice)?;
        }
    }
    if codex && options.responses_lite {
        lite(b, &body)?;
        let reasoning = b.get(&body, "reasoning")?;
        let reasoning = if b.is_nullish(&reasoning)? { b.object([])? } else { reasoning };
        b.put(&reasoning, "context", b.text("all_turns")?)?;
        b.put(&body, "reasoning", reasoning)?;
    }
    Ok(body)
}
fn lite(b: &GraphBuilder, body: &ValueHandle) -> Result<()> {
    let input = b.get(body, "input")?;
    for item in b.members(&input)? {
        for field in ["content", "output"] {
            let parts = b.get(&item, field)?;
            if b.0.heap().kind(&parts)? != ValueKind::Array {
                continue;
            }
            for part in b.members(&parts)? {
                if matches!(b.0.heap().kind(&part)?, ValueKind::Object | ValueKind::Array)
                    && history::eq(b, &part, "type", "input_image")?
                    && b.0.heap().has(&part, &"detail".into(), b.0.runtime())?
                {
                    b.put(&part, "detail", b.0.heap().undefined()?)?;
                }
            }
        }
    }
    b.put(body, "parallel_tool_calls", b.0.heap().boolean(false)?)?;
    let declared = b.get(body, "tools")?;
    let tools = if b.0.heap().kind(&declared)? == ValueKind::Array { b.members(&declared)? } else { Vec::new() };
    let choice = b.get(body, "tool_choice")?;
    let mut selected = None;
    if matches!(b.0.heap().kind(&choice)?, ValueKind::Object) {
        for tool in &tools {
            if (history::eq(b, &choice, "type", "computer")? && history::eq(b, tool, "type", "computer")?)
                || (history::eq(b, &choice, "type", "function")?
                    && history::eq(b, tool, "type", "function")?
                    && history::optional(b, &choice, "name")?.is_some()
                    && history::optional(b, &choice, "name")? == history::optional(b, tool, "name")?)
            {
                selected = Some(tool.clone());
                break;
            }
        }
    }
    let tools = if let Some(selected) = selected {
        b.put(body, "tool_choice", b.text("required")?)?;
        vec![selected]
    } else {
        tools
    };
    let mut prefix = vec![b.object([
        ("type", b.text("additional_tools")?),
        ("role", b.text("developer")?),
        ("tools", history::array(b, tools)?),
    ])?];
    if let Some(instruction) = history::optional(b, body, "instructions")? {
        if !instruction.units().is_empty() {
            prefix.push(text_message(b, "developer", instruction)?);
        }
    }
    prefix.extend(b.members(&input)?);
    b.put(body, "input", history::array(b, prefix)?)?;
    if !history::eq(b, body, "tool_choice", "none")? && !history::eq(b, body, "tool_choice", "required")? {
        b.put(body, "tool_choice", b.text("auto")?)?;
    }
    delete(b, body, "instructions")?;
    delete(b, body, "tools")
}
