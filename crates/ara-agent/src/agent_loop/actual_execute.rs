//! Fixed OMP agent-loop.ts actual dispatch/coerce (596f2da7101178214aa27a753529d15e6b7ad91d).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! Original Graph values remain canonical; Host owns the exact invocation,
//! final grant and async settlement. This reuses the existing batch lifecycle.
use super::actual::ActualFinalAssistant;
use super::actual_prepare::PreparedActualCall;
use super::*;
use crate::agent_message::CurrentArgumentsSlot;
use crate::tool::{
    ActualExecuteRequest, ActualToolExecutionContext, ActualUpdateFn, cursor_exec_resolved_key,
};
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueHeap, ValueKind,
        ValueRuntime, ValueSnapshot,
    },
};

fn get(
    values: &SessionValues,
    target: &ValueHandle,
    key: &str,
) -> Result<ValueHandle, ActualProviderError> {
    Ok(values.heap().get(target, &key.into(), values.runtime())?)
}
fn own(
    values: &SessionValues,
    target: &ValueHandle,
    key: &str,
    value: ValueHandle,
) -> Result<(), ActualProviderError> {
    Ok(values
        .heap()
        .define_own_data(target, key.into(), value, values.runtime())?)
}
fn method(
    values: &SessionValues,
    target: &ValueHandle,
    key: &str,
    arguments: &[ValueHandle],
) -> Result<ValueHandle, ActualProviderError> {
    let function = get(values, target, key)?;
    Ok(values
        .heap()
        .call(&function, target, arguments, values.runtime())?)
}
fn ascii(
    values: &SessionValues,
    value: &ValueHandle,
    literal: &str,
) -> Result<bool, ActualProviderError> {
    Ok(
        matches!(values.heap().snapshot(value)?, ValueSnapshot::String(text) if text.equals_ascii(literal)),
    )
}
fn text(values: &SessionValues, value: &ValueHandle) -> Result<WireString, ActualProviderError> {
    match values.heap().snapshot(value)? {
        ValueSnapshot::String(text) => Ok(text),
        _ => Err(ActualProviderError::Protocol(
            "actual dispatch ID/name requires admitted Text",
        )),
    }
}
fn implicit_text(
    values: &SessionValues,
    value: &ValueHandle,
) -> Result<WireString, ActualProviderError> {
    let value = values.heap().to_primitive(value, true, values.runtime())?;
    if values.heap().kind(&value)? == ValueKind::Symbol {
        return Err(
            ValueError::TypeError("Cannot implicitly convert Symbol to string".into()).into(),
        );
    }
    Ok(values.heap().to_string(&value, values.runtime())?)
}
fn nullish(values: &SessionValues, value: &ValueHandle) -> Result<bool, ActualProviderError> {
    Ok(matches!(
        values.heap().kind(value)?,
        ValueKind::Null | ValueKind::Undefined
    ))
}
fn native(error: &ActualProviderError) -> bool {
    match error {
        ActualProviderError::Value(value) => value.is_native_exception(),
        ActualProviderError::Owner(value) => value.is_native_exception(),
        ActualProviderError::Callback(_) => true,
        _ => false,
    }
}
fn combine(primary: &mut Option<Arc<ActualProviderError>>, secondary: ActualProviderError) {
    let secondary = Arc::new(secondary);
    *primary = Some(match primary.take() {
        Some(primary) => Arc::new(ActualProviderError::Compound { primary, secondary }),
        None => secondary,
    });
}
fn guarded<T>(
    slot: &CurrentArgumentsSlot,
    action: impl FnOnce() -> Result<T, ActualProviderError>,
) -> Result<T, ActualProviderError> {
    slot.recheck_current()?;
    let result = action();
    let admission = slot.recheck_current().map_err(ActualProviderError::from);
    match (result, admission) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Err(secondary)) => Err(ActualProviderError::Compound {
            primary: Arc::new(primary),
            secondary: Arc::new(secondary),
        }),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
    }
}
fn readmit<T>(
    slot: &CurrentArgumentsSlot,
    result: Result<T, ActualProviderError>,
) -> Result<T, ActualProviderError> {
    match (
        result,
        slot.recheck_current().map_err(ActualProviderError::from),
    ) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Err(secondary)) => Err(ActualProviderError::Compound {
            primary: Arc::new(primary),
            secondary: Arc::new(secondary),
        }),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
    }
}
fn owner_guarded<T>(
    owner: &AgentMessage,
    action: impl FnOnce() -> Result<T, ActualProviderError>,
) -> Result<T, ActualProviderError> {
    let root = match owner.body_snapshot() {
        AgentMessageBody::Graph(root) => root,
        _ => {
            return Err(ActualProviderError::Protocol(
                "actual owner lost Graph body",
            ));
        }
    };
    owner.graph_identity_controls()?;
    let result = action();
    let admission = (|| {
        if !matches!(owner.body_snapshot(), AgentMessageBody::Graph(current) if current.same_identity(&root))
        {
            return Err(ActualProviderError::Owner(
                crate::agent_message::GraphAdmissionError::StaleRoot.into(),
            ));
        }
        owner.graph_identity_controls()?;
        Ok(())
    })();
    match (result, admission) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Err(secondary)) => Err(ActualProviderError::Compound {
            primary: Arc::new(primary),
            secondary: Arc::new(secondary),
        }),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
    }
}
fn push(
    values: &SessionValues,
    array: &ValueHandle,
    value: ValueHandle,
) -> Result<(), ActualProviderError> {
    method(values, array, "push", &[value])?;
    Ok(())
}
fn text_block(
    values: &SessionValues,
    text: ValueHandle,
) -> Result<ValueHandle, ActualProviderError> {
    let block = values.heap().object()?;
    own(values, &block, "type", values.heap().string("text".into())?)?;
    own(values, &block, "text", text)?;
    Ok(block)
}
fn caught_output(
    values: &SessionValues,
    text: ValueHandle,
) -> Result<ValueHandle, ActualProviderError> {
    let result = values.heap().object()?;
    let block = text_block(values, text)?;
    let content = values.heap().array(vec![block])?;
    own(values, &result, "content", content)?;
    own(values, &result, "details", values.heap().object()?)?;
    Ok(result)
}
fn thrown_text(
    error: &ActualProviderError,
    config: &AgentConfig,
    values: &SessionValues,
) -> Result<ValueHandle, ActualProviderError> {
    match error {
        ActualProviderError::Value(error)
        | ActualProviderError::Owner(crate::agent_message::GraphOwnerError::Native(error)) => {
            config.hooks.tool_error_message_actual(error, values)
        }
        ActualProviderError::Callback(message) => {
            Ok(values.heap().string(message.as_str().into())?)
        }
        _ => Err(error.clone()),
    }
}
fn abort_output(
    values: &SessionValues,
    ctl: &RunControl,
    tool_call_id: &ara_rpc::WireString,
    before_start: bool,
) -> Result<ValueHandle, ActualProviderError> {
    let prefix = if before_start {
        "Tool was not executed because the run was aborted: "
    } else {
        "Tool execution was aborted: "
    };
    let mut message = WireString::from(prefix);
    message.append_str(&ctl.tool_abort_text(tool_call_id));
    if before_start {
        message.append_str(".");
    }
    caught_output(values, values.heap().string(message)?)
}
fn skipped_output(
    values: &SessionValues,
    source: Option<InterruptSource>,
    entered: bool,
) -> Result<ValueHandle, ActualProviderError> {
    let (reason, blocker) = match source {
        Some(InterruptSource::Steering(SteeringSource::User)) => {
            ("queued user message", "queued message")
        }
        Some(InterruptSource::Steering(SteeringSource::Agent)) => {
            ("pending parent steering message", "steering message")
        }
        Some(InterruptSource::Steering(SteeringSource::System)) => {
            ("pending system advisory", "advisory")
        }
        Some(InterruptSource::Irc) => ("pending peer interrupt", "interrupt"),
        _ => ("pending steering message", "queued message"),
    };
    let result = caught_output(values, values.heap().string(format!("Skipped due to {reason}. Do not count this skipped result as completed work or verification. After the {blocker} is handled on the next step, retry the skipped tool if it is still needed.").into())?)?;
    let details = get(values, &result, "details")?;
    own(
        values,
        &details,
        if entered {
            "__interrupted"
        } else {
            "__synthetic"
        },
        values.heap().boolean(true)?,
    )?;
    own(
        values,
        &details,
        "source",
        values.heap().string("interrupt_skipped".into())?,
    )?;
    if entered {
        own(
            values,
            &details,
            "execution",
            values.heap().string("started".into())?,
        )?;
    } else {
        own(values, &details, "executed", values.heap().boolean(false)?)?;
    }
    Ok(result)
}

fn close_error(
    values: &SessionValues,
    iterator: &mut ara_rpc::value_heap::IteratorRecord,
    primary: ActualProviderError,
) -> ActualProviderError {
    match values.heap().iterator_close(iterator, values.runtime()) {
        Ok(()) => primary,
        Err(close) if native(&primary) && close.is_native_exception() => primary,
        Err(secondary) => ActualProviderError::Compound {
            primary: Arc::new(primary),
            secondary: Arc::new(secondary.into()),
        },
    }
}

/// Native snapshotComputerSafetyChecks: values are read at source positions,
/// fresh result records keep the source String/null handles.
fn safety_checks(
    values: &SessionValues,
    input: &ValueHandle,
) -> Result<Option<ValueHandle>, ActualProviderError> {
    if values.heap().kind(input)? != ValueKind::Array {
        return Ok(None);
    }
    let output = values.heap().array_with_length(0)?;
    let mut iterator = values.heap().get_iterator(input, values.runtime())?;
    loop {
        let item = values
            .heap()
            .iterator_next(&mut iterator, values.runtime())?;
        let Some(item) = item else {
            return Ok(Some(output));
        };
        let step = (|| {
            if values.heap().kind(&item)? != ValueKind::Object {
                return Ok(false);
            }
            if values.heap().kind(&get(values, &item, "id")?)? != ValueKind::String {
                return Ok(false);
            }
            let id = get(values, &item, "id")?;
            if values
                .heap()
                .to_number(&get(values, &id, "length")?, values.runtime())?
                == 0.0
            {
                return Ok(false);
            }
            for key in ["code", "message"] {
                let first = get(values, &item, key)?;
                if values.heap().kind(&first)? != ValueKind::Undefined {
                    let second = get(values, &item, key)?;
                    if values.heap().kind(&second)? != ValueKind::Null
                        && values.heap().kind(&get(values, &item, key)?)? != ValueKind::String
                    {
                        return Ok(false);
                    }
                }
            }
            let check = values.heap().object()?;
            own(values, &check, "id", get(values, &item, "id")?)?;
            for key in ["code", "message"] {
                if values.heap().kind(&get(values, &item, key)?)? != ValueKind::Undefined {
                    own(values, &check, key, get(values, &item, key)?)?;
                }
            }
            push(values, &output, check)?;
            Ok(true)
        })();
        match step {
            Ok(true) => {}
            Ok(false) => {
                values
                    .heap()
                    .iterator_close(&mut iterator, values.runtime())?;
                return Ok(None);
            }
            Err(error) => return Err(close_error(values, &mut iterator, error)),
        }
    }
}
fn nonempty_property(
    values: &SessionValues,
    target: &ValueHandle,
    key: &str,
) -> Result<bool, ActualProviderError> {
    if values.heap().kind(&get(values, target, key)?)? != ValueKind::String {
        return Ok(false);
    }
    let value = get(values, target, key)?;
    Ok(values
        .heap()
        .to_number(&get(values, &value, "length")?, values.runtime())?
        != 0.0)
}
fn result_metadata(
    values: &SessionValues,
    input: &ValueHandle,
) -> Result<(ValueHandle, bool), ActualProviderError> {
    let missing = || values.heap().undefined().map_err(ActualProviderError::from);
    if values.heap().kind(input)? == ValueKind::Undefined {
        return Ok((missing()?, false));
    }
    if values.heap().kind(input)? != ValueKind::Object {
        return Ok((missing()?, true));
    }
    if !ascii(values, &get(values, input, "type")?, "computer")? {
        return Ok((missing()?, true));
    }
    if !values.heap().truthy(&get(values, input, "screenshot")?)? {
        return Ok((missing()?, true));
    }
    if !matches!(
        values.heap().kind(&get(values, input, "screenshot")?)?,
        ValueKind::Object | ValueKind::Array
    ) {
        return Ok((missing()?, true));
    }
    // Native Array.isArray(metadata.screenshot) does an independent Get.
    if values.heap().kind(&get(values, input, "screenshot")?)? == ValueKind::Array {
        return Ok((missing()?, true));
    }
    let screenshot = get(values, input, "screenshot")?;
    let image = values
        .heap()
        .has_own(&screenshot, &"image_url".into(), values.runtime())?;
    let file = values
        .heap()
        .has_own(&screenshot, &"file_id".into(), values.runtime())?;
    if !ascii(
        values,
        &get(values, &screenshot, "type")?,
        "computer_screenshot",
    )? || image == file
    {
        return Ok((missing()?, true));
    }
    let key = if image { "image_url" } else { "file_id" };
    if !nonempty_property(values, &screenshot, key)? {
        return Ok((missing()?, true));
    }
    let checks = safety_checks(values, &get(values, input, "acknowledgedSafetyChecks")?)?;
    let Some(checks) = checks else {
        return Ok((missing()?, true));
    };
    let snap = values.heap().object()?;
    own(
        values,
        &snap,
        "type",
        values.heap().string("computer_screenshot".into())?,
    )?;
    own(values, &snap, key, get(values, &screenshot, key)?)?;
    let metadata = values.heap().object()?;
    own(
        values,
        &metadata,
        "type",
        values.heap().string("computer".into())?,
    )?;
    own(values, &metadata, "screenshot", snap)?;
    own(values, &metadata, "acknowledgedSafetyChecks", checks)?;
    Ok((metadata, false))
}

// The actual sanitization call is bound to fixed Native source by the Host.
// It is not a typed String/serde adapter and must preserve UTF-16 units.
fn sanitized_text(
    config: &AgentConfig,
    values: &SessionValues,
    input: &ValueHandle,
) -> Result<ValueHandle, ActualProviderError> {
    config.hooks.sanitize_tool_result_text_actual(input, values)
}
struct CoercedActualResult {
    result: ValueHandle,
    malformed: bool,
}
fn coerce_actual(
    config: &AgentConfig,
    values: &SessionValues,
    raw: &ValueHandle,
) -> Result<CoercedActualResult, ActualProviderError> {
    values.check_handle(raw)?;
    let object = matches!(
        values.heap().kind(raw)?,
        ValueKind::Object | ValueKind::Array
    );
    let raw_content = if object {
        get(values, raw, "content")?
    } else {
        values.heap().undefined()?
    };
    let details = if object
        && values
            .heap()
            .has(raw, &"details".into(), values.runtime())?
    {
        get(values, raw, "details")?
    } else {
        values.heap().object()?
    };
    let metadata_input = if object
        && values
            .heap()
            .has(raw, &"providerMetadata".into(), values.runtime())?
    {
        get(values, raw, "providerMetadata")?
    } else {
        values.heap().undefined()?
    };
    let (metadata, metadata_malformed) = result_metadata(values, &metadata_input)?;
    let explicit_error = object
        && values
            .heap()
            .has(raw, &"isError".into(), values.runtime())?
        && values.heap().truthy(&get(values, raw, "isError")?)?;
    let useless = object
        && values
            .heap()
            .has(raw, &"useless".into(), values.runtime())?
        && values.heap().truthy(&get(values, raw, "useless")?)?;
    if values.heap().kind(&raw_content)? != ValueKind::Array {
        let result = caught_output(
            values,
            values
                .heap()
                .string("Tool returned an invalid result: missing content array.".into())?,
        )?;
        own(values, &result, "details", details)?;
        own(values, &result, "isError", values.heap().boolean(true)?)?;
        return Ok(CoercedActualResult {
            result,
            malformed: true,
        });
    }
    let content = values.heap().array_with_length(0)?;
    let mut invalid = 0usize;
    let mut iterator = values.heap().get_iterator(&raw_content, values.runtime())?;
    loop {
        let next = values
            .heap()
            .iterator_next(&mut iterator, values.runtime())?;
        let Some(block) = next else {
            break;
        };
        let step = (|| {
            if !matches!(
                values.heap().kind(&block)?,
                ValueKind::Object | ValueKind::Array
            ) || !values
                .heap()
                .has(&block, &"type".into(), values.runtime())?
            {
                return Ok(false);
            }
            if ascii(values, &get(values, &block, "type")?, "text")?
                && values.heap().kind(&get(values, &block, "text")?)? == ValueKind::String
            {
                let text = sanitized_text(config, values, &get(values, &block, "text")?)?;
                push(values, &content, text_block(values, text)?)?;
            } else if ascii(values, &get(values, &block, "type")?, "image")?
                && values.heap().kind(&get(values, &block, "data")?)? == ValueKind::String
                && values.heap().kind(&get(values, &block, "mimeType")?)? == ValueKind::String
            {
                push(values, &content, block.clone())?;
            } else {
                return Ok(false);
            }
            Ok(true)
        })();
        match step {
            Ok(true) => {}
            Ok(false) => invalid += 1,
            Err(error) => return Err(close_error(values, &mut iterator, error)),
        }
    }
    if invalid > 0 {
        let suffix = if invalid == 1 { "" } else { "s" };
        push(
            values,
            &content,
            text_block(
                values,
                values.heap().string(
                    format!(
                        "Tool returned an invalid result: {invalid} content block{suffix} had an unsupported shape."
                    )
                    .into(),
                )?,
            )?,
        )?;
    }
    if metadata_malformed {
        push(
            values,
            &content,
            text_block(
                values,
                values.heap().string(
                    "Tool returned an invalid result: computer providerMetadata had an unsupported shape.".into(),
                )?,
            )?,
        )?;
    }
    let is_error = explicit_error || invalid > 0 || metadata_malformed;
    if is_error {
        let mut substantive = false;
        let mut iterator = values.heap().get_iterator(&content, values.runtime())?;
        loop {
            let next = values
                .heap()
                .iterator_next(&mut iterator, values.runtime())?;
            let Some(block) = next else {
                break;
            };
            let step = (|| {
                if ascii(values, &get(values, &block, "type")?, "image")? {
                    return Ok(true);
                }
                if ascii(values, &get(values, &block, "type")?, "text")? {
                    let text = get(values, &block, "text")?;
                    let trimmed = method(values, &text, "trim", &[])?;
                    return Ok(values
                        .heap()
                        .to_number(&get(values, &trimmed, "length")?, values.runtime())?
                        > 0.0);
                }
                Ok(false)
            })();
            match step {
                Ok(true) => {
                    values
                        .heap()
                        .iterator_close(&mut iterator, values.runtime())?;
                    substantive = true;
                    break;
                }
                Ok(false) => {}
                Err(error) => return Err(close_error(values, &mut iterator, error)),
            }
        }
        if !substantive {
            values.heap().assign(
                &content,
                "length".into(),
                values.heap().number(0.0)?,
                values.runtime(),
            )?;
            push(
                values,
                &content,
                text_block(
                    values,
                    values.heap().string("Tool failed with no output.".into())?,
                )?,
            )?;
        }
    }
    let result = values.heap().object()?;
    own(values, &result, "content", content)?;
    own(values, &result, "details", details)?;
    own(values, &result, "providerMetadata", metadata)?;
    if is_error {
        own(values, &result, "isError", values.heap().boolean(true)?)?;
    }
    if useless && !is_error {
        own(values, &result, "useless", values.heap().boolean(true)?)?;
    }
    Ok(CoercedActualResult {
        result,
        malformed: invalid > 0 || metadata_malformed,
    })
}

static DISPATCH_FILTER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:agent-loop.ts:2390",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: dispatch_filter,
};
fn dispatch_filter(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let call = match arguments.first() {
        Some(call) => call.clone(),
        None => heap.undefined()?,
    };
    let kind = heap.get(&call, &"type".into(), runtime)?;
    let is_call = matches!(heap.snapshot(&kind)?, ValueSnapshot::String(kind) if kind.equals_ascii("toolCall"));
    let resolved = is_call
        && matches!(
            heap.snapshot(&heap.get(&call, &cursor_exec_resolved_key(heap)?, runtime)?)?,
            ValueSnapshot::Boolean(true)
        );
    heap.boolean(is_call && !resolved)
}
static CALL_INFO: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:agent-loop.ts:2395",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: call_info,
};
fn call_info(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let call = match arguments.first() {
        Some(call) => call.clone(),
        None => heap.undefined()?,
    };
    let info = heap.object()?;
    heap.define_own_data(
        &info,
        "id".into(),
        heap.get(&call, &"id".into(), runtime)?,
        runtime,
    )?;
    heap.define_own_data(
        &info,
        "name".into(),
        heap.get(&call, &"name".into(), runtime)?,
        runtime,
    )?;
    Ok(info)
}
#[derive(Clone)]
struct ActualRecord {
    prepared: PreparedActualCall,
    concurrency: Concurrency,
    interruptible: bool,
}
pub(super) fn actual_call_members(
    final_assistant: &ActualFinalAssistant,
) -> Result<Vec<ValueHandle>, ActualProviderError> {
    owner_guarded(&final_assistant.message, || {
        actual_call_members_inner(final_assistant)
    })
}
fn actual_call_members_inner(
    final_assistant: &ActualFinalAssistant,
) -> Result<Vec<ValueHandle>, ActualProviderError> {
    let values = &final_assistant.values;
    let root = match final_assistant.message.body_snapshot() {
        AgentMessageBody::Graph(root) => root,
        _ => {
            return Err(ActualProviderError::Protocol(
                "actual outer turn lost Graph owner",
            ));
        }
    };
    let content = get(values, &root, "content")?;
    let predicate = values
        .heap()
        .fresh_compiled_function(&DISPATCH_FILTER, &[])?;
    let calls = method(values, &content, "filter", &[predicate])?;
    let mut members = Vec::new();
    let mut iterator = values.heap().get_iterator(&calls, values.runtime())?;
    loop {
        let next = values
            .heap()
            .iterator_next(&mut iterator, values.runtime())?;
        let Some(member) = next else {
            return Ok(members);
        };
        match final_assistant
            .message
            .current_arguments_slot(&member)
            .and_then(|slot| slot.recheck_current())
        {
            Ok(()) => members.push(member),
            Err(error) => return Err(close_error(values, &mut iterator, error.into())),
        }
    }
}
pub(super) fn actual_pause_turn(
    final_assistant: &ActualFinalAssistant,
) -> Result<bool, ActualProviderError> {
    owner_guarded(&final_assistant.message, || {
        let root = match final_assistant.message.body_snapshot() {
            AgentMessageBody::Graph(root) => root,
            _ => {
                return Err(ActualProviderError::Protocol(
                    "actual pause-turn lost Graph owner",
                ));
            }
        };
        let details = get(&final_assistant.values, &root, "stopDetails")?;
        Ok(!nullish(&final_assistant.values, &details)?
            && ascii(
                &final_assistant.values,
                &get(&final_assistant.values, &details, "type")?,
                "pause_turn",
            )?)
    })
}
pub(super) fn actual_soft_rejection(
    final_assistant: &ActualFinalAssistant,
    calls: &[ValueHandle],
    state: &SoftToolRequirementState,
    requirement: Option<&crate::soft_tool_requirement::SoftToolRequirement>,
    static_choice: Option<&ToolChoice>,
    config: &AgentConfig,
) -> Result<Option<String>, ActualProviderError> {
    let Some(requirement) = requirement else {
        return Ok(None);
    };
    let mut compliant = !calls.is_empty();
    for call in calls {
        if !compliant {
            break;
        }
        let slot = final_assistant.message.current_arguments_slot(call)?;
        compliant = guarded(&slot, || {
            if requirement.satisfies.is_some() {
                config
                    .hooks
                    .soft_tool_satisfies_actual(requirement, call, &final_assistant.values)
            } else {
                Ok(text(
                    &final_assistant.values,
                    &get(&final_assistant.values, call, "name")?,
                )? == WireString::from(requirement.tool_name.as_str()))
            }
        })?;
    }
    if crate::soft_tool_requirement::hard_tool_choice_blocks(static_choice, &requirement.tool_name)
        || compliant
    {
        return Ok(None);
    }
    if state.snapshot().escalations >= 3 {
        return Err(ActualProviderError::Callback(format!(
            "Soft tool requirement '{}' was not satisfied after 3 forced turns; aborting to avoid an unbounded force loop.",
            requirement.tool_name
        )));
    }
    Ok(Some(format!(
        "Not executed: call the `{}` tool to resolve the pending action before using other tools.",
        requirement.tool_name
    )))
}
pub(super) async fn synthetic_actual_calls(
    final_assistant: &ActualFinalAssistant,
    calls: &[ValueHandle],
    reason: &str,
    explicit_error: Option<&str>,
    sink: &dyn AgentEventSink,
) -> ActualExecutionResult {
    let values = &final_assistant.values;
    let mut results = Vec::new();
    let mut failure = None;
    for call in calls {
        let built = (|| {
            let slot = final_assistant.message.current_arguments_slot(call)?;
            guarded(&slot, || {
                let id = text(values, &get(values, call, "id")?)?;
                let args = slot.read(values.runtime())?;
                let p = PreparedActualCall {
                    call: call.clone(),
                    call_id: id,
                    slot: slot.clone(),
                    tool: None,
                    args,
                    validation_error_message: None,
                    validation_error: None,
                    blocked: false,
                    block_reason: None,
                    prepare_error: None,
                };
                let message = match reason {
                    "aborted" => "Tool execution was aborted",
                    "length" => {
                        "Tool call was not executed because the assistant hit its output token limit (stop_reason: length) before the arguments could complete; the recorded arguments are truncated and unsafe to run. Do NOT retry by re-emitting the same large payload — split the work into several smaller tool calls (e.g. for `write`/`edit`, write the first chunk then append the rest with subsequent `edit` insert ops, or break the file into multiple `write` targets)"
                    }
                    "skipped" => "Tool call was not executed because the assistant ended its turn",
                    _ => {
                        "Tool call was not executed because the provider stream ended with an error before the tool could run"
                    }
                };
                let scoped = (reason == "aborted")
                    .then(|| sink.abort_reason().and_then(|owner| owner.get()))
                    .flatten();
                let scoped_text = scoped.as_ref().map(|reason| reason.for_tool(&p.call_id));
                let upstream = if let Some(error) = scoped_text.or(explicit_error) {
                    Some(values.heap().string(error.into())?)
                } else if reason == "error" {
                    match final_assistant.message.body_snapshot() {
                        AgentMessageBody::Graph(root) => Some(get(values, &root, "errorMessage")?),
                        _ => None,
                    }
                } else {
                    None
                };
                let mut message = WireString::from(message);
                if let Some(upstream) = &upstream
                    && !nullish(values, upstream)?
                {
                    message.append_str(": ");
                    let mut units = message.units().to_vec();
                    units.extend_from_slice(implicit_text(values, upstream)?.units());
                    message = WireString::from_units(units);
                } else {
                    message.append_str(".");
                }
                let output = caught_output(values, values.heap().string(message)?)?;
                let details = get(values, &output, "details")?;
                own(
                    values,
                    &details,
                    "__synthetic",
                    values.heap().boolean(true)?,
                )?;
                own(
                    values,
                    &details,
                    "source",
                    values.heap().string(
                        match reason {
                            "aborted" => "assistant_stop_aborted",
                            "error" => "assistant_stop_error",
                            "length" => "assistant_stop_length",
                            _ => "assistant_stop_skipped",
                        }
                        .into(),
                    )?,
                )?;
                own(values, &details, "executed", values.heap().boolean(false)?)?;
                if reason == "error"
                    && let Some(upstream) = upstream
                    && !nullish(values, &upstream)?
                {
                    own(values, &details, "upstreamError", upstream)?;
                }
                Ok((p, output))
            })
        })();
        let result = match built {
            Ok((p, output)) => emit_result(sink, &p, output, true, false, values).await,
            Err(error) => Err(error),
        };
        match result {
            Ok(result) => results.push(result),
            Err(error) => combine(&mut failure, error),
        }
    }
    ActualExecutionResult { results, failure }
}
fn actual_records(
    final_assistant: &ActualFinalAssistant,
) -> Result<(Vec<ActualRecord>, ValueHandle, WireString), ActualProviderError> {
    owner_guarded(&final_assistant.message, || {
        actual_records_inner(final_assistant)
    })
}
fn actual_records_inner(
    final_assistant: &ActualFinalAssistant,
) -> Result<(Vec<ActualRecord>, ValueHandle, WireString), ActualProviderError> {
    let values = &final_assistant.values;
    let root = match final_assistant.message.body_snapshot() {
        AgentMessageBody::Graph(root) => root,
        _ => {
            return Err(ActualProviderError::Protocol(
                "actual dispatch lost Graph owner",
            ));
        }
    };
    let content = get(values, &root, "content")?;
    let predicate = values
        .heap()
        .fresh_compiled_function(&DISPATCH_FILTER, &[])?;
    let calls = method(values, &content, "filter", &[predicate])?;
    let infos = method(
        values,
        &calls,
        "map",
        &[values.heap().fresh_compiled_function(&CALL_INFO, &[])?],
    )?;
    let timestamp = get(values, &root, "timestamp")?;
    let timestamp = if nullish(values, &timestamp)? {
        values.heap().number(now_ms() as f64)?
    } else {
        timestamp
    };
    let first = get(values, &calls, "0")?;
    let id = if nullish(values, &first)? {
        values.heap().undefined()?
    } else {
        get(values, &first, "id")?
    };
    let id = if nullish(values, &id)? {
        values.heap().string("batch".into())?
    } else {
        id
    };
    let mut batch_id = implicit_text(values, &timestamp)?;
    batch_id.append_str("_");
    let mut units = batch_id.units().to_vec();
    units.extend_from_slice(implicit_text(values, &id)?.units());
    let batch_id = WireString::from_units(units);
    let mut records = Vec::new();
    let mut iterator = values.heap().get_iterator(&calls, values.runtime())?;
    loop {
        let next = values
            .heap()
            .iterator_next(&mut iterator, values.runtime())?;
        let Some(call) = next else {
            break;
        };
        let slot = final_assistant.message.current_arguments_slot(&call)?;
        let step = guarded(&slot, || {
            let id = text(values, &get(values, &call, "id")?)?;
            let mut prepared = final_assistant
                .prepared_dispatch
                .as_ref()
                .and_then(|batch| batch.get(&id))
                .cloned()
                .ok_or(ActualProviderError::Unsupported(
                    "actual dispatch has no prepared receipt for current member",
                ))?;
            // Native Map lookup uses last duplicate ID, but effects always admit
            // this dispatch member's own owner/slot, not its sibling's slot.
            prepared.call = call.clone();
            prepared.call_id = id;
            prepared.slot = slot.clone();
            let mode = if let Some(tool) = &prepared.tool {
                get(values, &tool.source, "interruptible")?
            } else {
                values.heap().undefined()?
            };
            let interruptible = if values.heap().kind(&mode)? == ValueKind::Function {
                match guarded(&slot, || {
                    Ok(values.heap().call(
                        &mode,
                        &values.heap().undefined()?,
                        &[prepared.args.clone()],
                        values.runtime(),
                    )?)
                }) {
                    Ok(mode) => values.heap().truthy(&mode)?,
                    Err(error) if native(&error) => false,
                    Err(error) => return Err(error),
                }
            } else {
                matches!(values.heap().snapshot(&mode)?, ValueSnapshot::Boolean(true))
            };
            Ok(ActualRecord {
                prepared,
                concurrency: Concurrency::Shared,
                interruptible,
            })
        });
        match step {
            Ok(record) => records.push(record),
            Err(error) => return Err(close_error(values, &mut iterator, error)),
        }
    }
    // Native resolves concurrency for all records at scheduling time, after
    // interruptible resolution. A thrown resolver defaults to Exclusive.
    for record in &mut records {
        let mode = guarded(&record.prepared.slot, || match &record.prepared.tool {
            Some(tool) => get(values, &tool.source, "concurrency"),
            None => Ok(values.heap().undefined()?),
        })?;
        let mode = if values.heap().kind(&mode)? == ValueKind::Function {
            match guarded(&record.prepared.slot, || {
                Ok(values.heap().call(
                    &mode,
                    &values.heap().undefined()?,
                    &[record.prepared.args.clone()],
                    values.runtime(),
                )?)
            }) {
                Ok(mode) => mode,
                Err(error) if native(&error) => values.heap().string("exclusive".into())?,
                Err(error) => return Err(error),
            }
        } else {
            mode
        };
        record.concurrency = if ascii(values, &mode, "exclusive")? {
            Concurrency::Exclusive
        } else {
            Concurrency::Shared
        };
    }
    Ok((records, infos, batch_id))
}

async fn emit_start(
    sink: &dyn AgentEventSink,
    p: &PreparedActualCall,
    values: &SessionValues,
) -> Result<(), ActualProviderError> {
    let (id, name, intent) = guarded(&p.slot, || {
        Ok((
            text(values, &get(values, &p.call, "id")?)?,
            text(values, &get(values, &p.call, "name")?)?,
            get(values, &p.call, "intent")?,
        ))
    })?;
    sink.emit(AgentEvent::ActualToolExecutionStart {
        tool_call_id: id,
        tool_name: name,
        args: p.args.clone(),
        intent,
    })
    .await;
    p.slot.recheck_current()?;
    Ok(())
}
async fn emit_result(
    sink: &dyn AgentEventSink,
    p: &PreparedActualCall,
    output: ValueHandle,
    error: bool,
    started: bool,
    values: &SessionValues,
) -> Result<AgentMessage, ActualProviderError> {
    if !started {
        emit_start(sink, p, values).await?;
    }
    let (id, name) = guarded(&p.slot, || {
        Ok((
            text(values, &get(values, &p.call, "id")?)?,
            text(values, &get(values, &p.call, "name")?)?,
        ))
    })?;
    sink.emit(AgentEvent::ActualToolExecutionEnd {
        tool_call_id: id,
        tool_name: name,
        result: output.clone(),
        is_error: error,
    })
    .await;
    p.slot.recheck_current()?;
    let root = guarded(&p.slot, || {
        let root = values.heap().object()?;
        own(
            values,
            &root,
            "role",
            values.heap().string("toolResult".into())?,
        )?;
        own(values, &root, "toolCallId", get(values, &p.call, "id")?)?;
        own(values, &root, "toolName", get(values, &p.call, "name")?)?;
        own(values, &root, "content", get(values, &output, "content")?)?;
        own(values, &root, "details", get(values, &output, "details")?)?;
        own(
            values,
            &root,
            "providerMetadata",
            get(values, &output, "providerMetadata")?,
        )?;
        own(values, &root, "isError", values.heap().boolean(error)?)?;
        if values.heap().truthy(&get(values, &output, "useless")?)? && !error {
            own(values, &root, "useless", values.heap().boolean(true)?)?;
        }
        own(
            values,
            &root,
            "timestamp",
            values.heap().number(now_ms() as f64)?,
        )?;
        Ok(root)
    })?;
    let record = AgentMessage::from_graph(root, None)?;
    sink.emit(AgentEvent::MessageStart {
        message: record.clone(),
    })
    .await;
    sink.emit(AgentEvent::MessageEnd {
        message: record.clone(),
    })
    .await;
    p.slot.recheck_current()?;
    Ok(record)
}

#[allow(clippy::too_many_arguments)]
async fn run_actual_tool(
    record: &ActualRecord,
    index: usize,
    batch_id: &WireString,
    infos: &ValueHandle,
    total: usize,
    final_assistant: &ActualFinalAssistant,
    messages: &[AgentMessage],
    config: &AgentConfig,
    ctl: &RunControl,
    batch: &BatchInterrupt,
    sink: &dyn AgentEventSink,
) -> Result<Option<AgentMessage>, ActualProviderError> {
    if record.interruptible && batch.source().is_some() {
        return Ok(None);
    }
    let signal = if record.interruptible {
        batch.hard.child_token()
    } else {
        ctl.token.child_token()
    };
    AGENT_PAUSE_GATE.wait_until_resumed(Some(&signal)).await;
    let mut p = record.prepared.clone();
    let values = &final_assistant.values;
    p.slot
        .require_owned_for(&final_assistant.message, &p.call)?;
    if let Some(message) = &p.validation_error_message {
        let result = caught_output(values, message.clone())?;
        let details = get(values, &result, "details")?;
        own(values, &details, "isError", values.heap().boolean(true)?)?;
        own(values, &details, "error", message.clone())?;
        return emit_result(sink, &p, result, true, false, values)
            .await
            .map(Some);
    }
    if signal.is_cancelled() {
        return emit_result(
            sink,
            &p,
            abort_output(values, ctl, &p.call_id, true)?,
            true,
            false,
            values,
        )
        .await
        .map(Some);
    }
    emit_start(sink, &p, values).await?;
    let mut entered = false;
    let mut returned = false;
    let execution = async {
        let Some(tool) = p.tool.clone() else {
            let name = guarded(&p.slot, || {
                Ok(values
                    .heap()
                    .to_string(&get(values, &p.call, "name")?, values.runtime())?)
            })?;
            let mut message = WireString::from("Tool ");
            let mut units = message.units().to_vec();
            units.extend_from_slice(name.units());
            message = WireString::from_units(units);
            message.append_str(" not found");
            return Ok((caught_output(values, values.heap().string(message)?)?, true));
        };
        if signal.is_cancelled() {
            return Ok((abort_output(values, ctl, &p.call_id, false)?, true));
        }
        if let Some(error) = &p.prepare_error {
            return Err(error.as_ref().clone());
        }
        if p.blocked {
            let message = match &p.block_reason {
                Some(message) if !nullish(values, message)? => {
                    values.heap().string(implicit_text(values, message)?)?
                }
                _ => values.heap().string("Tool execution was blocked".into())?,
            };
            return Ok((caught_output(values, message)?, true));
        }
        let name = guarded(&p.slot, || text(values, &get(values, &p.call, "name")?))?;
        p.args = guarded(&p.slot, || {
            config
                .hooks
                .transform_tool_call_arguments_actual(p.args.clone(), &name, values)
        })?;
        values.check_handle(&p.args)?;
        let provider_metadata = guarded(&p.slot, || get(values, &p.call, "providerMetadata"))?;
        let tool_context = guarded(&p.slot, || {
            config.hooks.get_tool_context_actual(
                &ActualToolExecutionContext {
                    batch_id: batch_id.clone(),
                    index,
                    total,
                    tool_calls: infos.clone(),
                    provider_metadata,
                    steering_signal: batch.soft.clone(),
                },
                values,
            )
        })?;
        values.check_handle(&tool_context)?;
        let (tx, mut rx) =
            tokio::sync::mpsc::unbounded_channel::<(WireString, WireString, ValueHandle)>();
        let callback_failures = Arc::new(Mutex::new(Vec::<ActualProviderError>::new()));
        let update_slot = p.slot.clone();
        let update_values = values.clone();
        let update_failures = callback_failures.clone();
        let update_call = p.call.clone();
        let update_config = config.clone();
        let update: ActualUpdateFn = Arc::new(move |partial| {
            let result = guarded(&update_slot, || {
                let id = text(&update_values, &get(&update_values, &update_call, "id")?)?;
                let name = text(&update_values, &get(&update_values, &update_call, "name")?)?;
                let partial = coerce_actual(&update_config, &update_values, &partial)?.result;
                Ok((id, name, partial))
            });
            match result {
                Ok(partial) => {
                    let _ = tx.send(partial);
                    Ok(())
                }
                Err(error) => {
                    if !native(&error) {
                        update_failures.lock().unwrap().push(error.clone());
                    }
                    Err(error)
                }
            }
        });
        // Source executionStarted precedes the execute method Get and original
        // argument-expression Gets. The Host alone Calls the exact method.
        entered = true;
        let callable = guarded(&p.slot, || get(values, &tool.source, "execute"))?;
        let call_id = guarded(&p.slot, || text(values, &get(values, &p.call, "id")?))?;
        let invoker = tool
            .capability
            .actual_invoker()
            .ok_or(ActualProviderError::Unsupported(
                "original Host capability has no actual invoker",
            ))?;
        let request = ActualExecuteRequest {
            callable,
            receiver: tool.source.clone(),
            owner: final_assistant.message.clone(),
            tool_call: p.call.clone(),
            arguments_slot: p.slot.clone(),
            call_id,
            args: p.args.clone(),
            values: values.clone(),
            cancel: signal.clone(),
            on_update: update,
            tool_context,
        };
        request.check_admission()?;
        let future = std::panic::AssertUnwindSafe(invoker.invoke_settled(request)).catch_unwind();
        tokio::pin!(future);
        let mut publication_failures = Vec::new();
        let result = loop {
            tokio::select! {
                result = &mut future => break result,
                Some((id, name, partial)) = rx.recv() => {
                    let published = std::panic::AssertUnwindSafe(sink.emit(AgentEvent::ActualToolExecutionUpdate { tool_call_id: id, tool_name: name, args: p.args.clone(), partial })).catch_unwind().await;
                    if let Err(panic) = published { publication_failures.push(ActualProviderError::Callback(format!("Actual partial sink panicked: {}", panic_text(panic)))); }
                }
            }
        };
        // No cancellation arm drops an entered method Future. A known return
        // remains known before coercion and before checking a late abort.
        if matches!(&result, Ok(Ok(_))) {
            returned = true;
        }
        while let Ok((id, name, partial)) = rx.try_recv() {
            let published =
                std::panic::AssertUnwindSafe(sink.emit(AgentEvent::ActualToolExecutionUpdate {
                    tool_call_id: id,
                    tool_name: name,
                    args: p.args.clone(),
                    partial,
                }))
                .catch_unwind()
                .await;
            if let Err(panic) = published {
                publication_failures.push(ActualProviderError::Callback(format!(
                    "Actual partial sink panicked: {}",
                    panic_text(panic)
                )));
            }
        }
        let mut integration_failure = None;
        for failure in publication_failures {
            combine(&mut integration_failure, failure);
        }
        for failure in callback_failures.lock().unwrap().drain(..) {
            combine(&mut integration_failure, failure);
        }
        let result = match result {
            Ok(result) => result,
            Err(panic) => Err(ActualProviderError::Callback(format!(
                "Actual tool invocation panicked: {}",
                panic_text(panic)
            ))),
        };
        let result = match (result, integration_failure) {
            (Ok(result), None) => Ok(result),
            (Err(primary), Some(secondary)) => Err(ActualProviderError::Compound {
                primary: Arc::new(primary),
                secondary,
            }),
            (Err(error), None) => Err(error),
            (Ok(_), Some(error)) => Err(error.as_ref().clone()),
        };
        let raw = readmit(&p.slot, result)?;
        let coerced = guarded(&p.slot, || coerce_actual(config, values, &raw))?;
        let is_error = coerced.malformed
            || values
                .heap()
                .truthy(&get(values, &coerced.result, "isError")?)?;
        Ok((coerced.result, is_error))
    };
    let execution = std::panic::AssertUnwindSafe(execution).catch_unwind().await;
    let execution = match execution {
        Ok(result) => result,
        Err(panic) => Err(ActualProviderError::Callback(format!(
            "Actual tool execution panicked: {}",
            panic_text(panic)
        ))),
    };
    let (mut output, mut is_error) = match execution {
        Ok(result) => result,
        Err(error) if native(&error) => (
            guarded(&p.slot, || {
                caught_output(values, thrown_text(&error, config, values)?)
            })?,
            true,
        ),
        Err(error) => return Err(error),
    };
    if !signal.is_cancelled() || returned {
        let after = std::panic::AssertUnwindSafe(config.hooks.after_tool_call_actual(
            ActualAfterToolCallContext {
                assistant_message: &final_assistant.message,
                tool_call: &p.call,
                args: &p.args,
                result: &output,
                is_error,
                context: ActualAgentContextView {
                    system_prompt: &config.system_prompt,
                    messages,
                    tool_source: final_assistant.actual_tool_source.as_deref(),
                },
                terminal: ctl.terminal_handle(&p.call_id.to_utf8().map_err(|_| {
                    ActualProviderError::Unsupported("terminal handle requires scalar call ID")
                })?),
                values,
            },
            &signal,
        ))
        .catch_unwind()
        .await;
        let after = match after {
            Ok(result) => result,
            Err(panic) => Err(ActualProviderError::Callback(format!(
                "Actual after-tool hook panicked: {}",
                panic_text(panic)
            ))),
        };
        let after = readmit(&p.slot, after);
        let revised = match after {
            Err(error) => Err(error),
            Ok(after) => guarded(&p.slot, || {
                let Some(after) = after else {
                    return Ok(None);
                };
                values.check_handle(&after)?;
                if !values.heap().truthy(&after)? {
                    return Ok(None);
                }
                let raw = values.heap().object()?;
                for key in [
                    "content",
                    "details",
                    "isError",
                    "providerMetadata",
                    "useless",
                ] {
                    let value = get(values, &after, key)?;
                    own(
                        values,
                        &raw,
                        key,
                        if nullish(values, &value)? {
                            get(values, &output, key)?
                        } else {
                            value
                        },
                    )?;
                }
                let coerced = coerce_actual(config, values, &raw)?;
                // Native performs a second after.isError Get after coercion.
                let after_error = get(values, &after, "isError")?;
                let effective = if nullish(values, &after_error)? {
                    is_error
                } else {
                    values.heap().truthy(&after_error)?
                };
                Ok(Some((coerced.result, coerced.malformed || effective)))
            }),
        };
        match revised {
            Ok(Some((result, error))) => {
                output = result;
                is_error = error;
            }
            Ok(None) => {}
            Err(error) if native(&error) => {
                output = guarded(&p.slot, || {
                    caught_output(values, thrown_text(&error, config, values)?)
                })?;
                is_error = true;
            }
            Err(error) => return Err(error),
        }
    }
    if batch.source().is_some() && signal.is_cancelled() && is_error && !returned {
        output = skipped_output(values, batch.source(), entered)?;
    }
    emit_result(sink, &p, output, is_error, true, values)
        .await
        .map(Some)
}

pub(super) struct ActualExecutionResult {
    pub results: Vec<AgentMessage>,
    pub failure: Option<Arc<ActualProviderError>>,
}
pub(super) async fn execute_actual_calls(
    final_assistant: &ActualFinalAssistant,
    messages: &[AgentMessage],
    config: &AgentConfig,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
) -> ActualExecutionResult {
    let (records, infos, batch_id) = match actual_records(final_assistant) {
        Ok(records) => records,
        Err(error) => {
            return ActualExecutionResult {
                results: Vec::new(),
                failure: Some(Arc::new(error)),
            };
        }
    };
    let batch = BatchInterrupt::new(config, ctl);
    let _watch_guard = batch.watcher_cancel.clone().drop_guard();
    let results = tokio::sync::Mutex::new(Vec::new());
    let skipped = tokio::sync::Mutex::new(Vec::new());
    let failures = tokio::sync::Mutex::new(Vec::new());
    let total = records.len();
    let execution = async {
        let mut index = 0usize;
        while index < records.len() {
            let end = if records[index].concurrency == Concurrency::Exclusive {
                index + 1
            } else {
                let mut end = index;
                while end < records.len() && records[end].concurrency != Concurrency::Exclusive {
                    end += 1;
                }
                end
            };
            let futures = records[index..end]
                .iter()
                .enumerate()
                .map(|(offset, record)| {
                    let (batch, results, skipped, failures, batch_id, infos) =
                        (&batch, &results, &skipped, &failures, &batch_id, &infos);
                    async move {
                        // Fixed Native Promise.allSettled keeps every already
                        // started sibling alive when this job rejects. Include
                        // start/end publication and the post-result batch check:
                        // an unwind escaping join_all would drop sibling effects.
                        let job = async {
                            let result = run_actual_tool(
                                record,
                                index + offset,
                                batch_id,
                                infos,
                                total,
                                final_assistant,
                                messages,
                                config,
                                ctl,
                                batch,
                                sink,
                            )
                            .await?;
                            match result {
                                Some(result) => {
                                    results.lock().await.push(result);
                                    let _ = batch.check().await;
                                }
                                None => skipped.lock().await.push(index + offset),
                            }
                            Ok::<(), ActualProviderError>(())
                        };
                        let result = match std::panic::AssertUnwindSafe(job).catch_unwind().await {
                            Ok(result) => result,
                            Err(panic) => readmit(
                                &record.prepared.slot,
                                Err(ActualProviderError::Callback(format!(
                                    "Actual tool batch job panicked: {}",
                                    panic_text(panic)
                                ))),
                            ),
                        };
                        if let Err(error) = result {
                            failures.lock().await.push(error);
                        }
                    }
                });
            futures::future::join_all(futures).await;
            index = end;
        }
        batch.watcher_cancel.cancel();
    };
    tokio::join!(batch.watch_events(), batch.watch_timer(), execution);
    let mut results = results.into_inner();
    let mut skipped = skipped.into_inner();
    skipped.sort_unstable();
    let mut failure = None;
    for error in failures.into_inner() {
        combine(&mut failure, error);
    }
    for index in skipped {
        let record = &records[index];
        let result = match skipped_output(&final_assistant.values, batch.source(), false) {
            Ok(output) => {
                emit_result(
                    sink,
                    &record.prepared,
                    output,
                    true,
                    false,
                    &final_assistant.values,
                )
                .await
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(result) => results.push(result),
            Err(error) => combine(&mut failure, error),
        }
    }
    ActualExecutionResult { results, failure }
}

/// One grouped controlled-Host observation called by the existing prepare
/// family. This is an actual Core flow, not a provider/model acceptance claim.
#[cfg(test)]
pub(super) async fn observe_actual_execution_group() {
    use crate::tool::{ActualToolBinding, ActualToolInvoker, ActualToolResultSanitizer};
    use ara_rpc::value_heap::{CapabilityId, NoRuntime};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Invoker {
        receiver: ValueHandle,
        callable: ValueHandle,
        result: ValueHandle,
        thrown: ValueHandle,
        mode: &'static str,
        calls: AtomicUsize,
    }
    #[async_trait]
    impl ActualToolInvoker for Invoker {
        async fn invoke_settled(
            &self,
            request: ActualExecuteRequest,
        ) -> Result<ValueHandle, ActualProviderError> {
            request.check_admission()?;
            assert!(request.receiver.same_identity(&self.receiver));
            assert!(request.callable.same_identity(&self.callable));
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.mode == "owned-i" {
                let intent = get(&request.values, &request.args, "i")?;
                assert!(
                    matches!(
                        request.values.heap().snapshot(&intent)?,
                        ValueSnapshot::Number(42.0)
                    ),
                    "actual execute receives the schema-owned parameter with tracing enabled"
                );
                assert_eq!(
                    request.values.heap().kind(&get(
                        &request.values,
                        &request.tool_call,
                        "intent"
                    )?)?,
                    ValueKind::Undefined
                );
            }
            (request.on_update)(self.result.clone())?;
            if self.mode == "late-cancel" {
                request.cancel.cancel();
            }
            if self.mode == "throw" {
                return Err(ValueError::Thrown(self.thrown.clone()).into());
            }
            Ok(self.result.clone())
        }
    }
    struct Tool {
        invoker: Arc<Invoker>,
    }
    #[async_trait]
    impl AgentTool for Tool {
        fn definition(&self) -> ara_ai::Tool {
            panic!("actual dispatch called typed definition")
        }
        fn actual_invoker(&self) -> Option<Arc<dyn ActualToolInvoker>> {
            Some(self.invoker.clone())
        }
        async fn execute(
            &self,
            _: &str,
            _: JsonObject,
            _: CancellationToken,
            _: crate::tool::UpdateFn,
        ) -> Result<ToolOutput, crate::tool::ToolError> {
            panic!("actual dispatch used typed execute")
        }
    }
    struct Source {
        tool: ValueHandle,
        collection: ValueHandle,
        capability: Arc<dyn AgentTool>,
    }
    impl crate::tool::ActualToolSource for Source {
        fn current_tools(
            &self,
            _: &SessionValues,
        ) -> Result<Option<ValueHandle>, ActualProviderError> {
            Ok(Some(self.collection.clone()))
        }
        fn bind_selected(
            &self,
            selected: &ValueHandle,
            values: &SessionValues,
        ) -> Result<ActualToolBinding, ActualProviderError> {
            let binding = ActualToolBinding {
                source: self.tool.clone(),
                capability: self.capability.clone(),
            };
            binding.check_selected(selected, values)?;
            Ok(binding)
        }
        fn has_fallback(&self) -> bool {
            false
        }
    }
    struct Provider;
    impl ModelProvider for Provider {
        fn stream(&self, _: &Model, _: &Context, _: CallOptions) -> ara_ai::AssistantStream {
            panic!("execution observation is not a model trial")
        }
    }
    struct Hooks {
        sanitizer: ActualToolResultSanitizer,
        after_calls: AtomicUsize,
        turn_calls: AtomicUsize,
        mode: &'static str,
    }
    #[async_trait]
    impl LoopHooks for Hooks {
        fn supports_actual_tool_hooks(&self) -> bool {
            true
        }
        fn supports_actual_turn_end(&self) -> bool {
            true
        }
        async fn transform_assistant_graph(
            &self,
            _: &AgentMessage,
            _: &CancellationToken,
            _: &SessionValues,
        ) -> Result<(), ActualProviderError> {
            Ok(())
        }
        async fn before_tool_dispatch_actual(
            &self,
            _: ActualBeforeToolCallContext<'_>,
            _: &CancellationToken,
        ) -> Result<Option<ValueHandle>, ActualProviderError> {
            Ok(None)
        }
        async fn before_tool_call_actual(
            &self,
            _: ActualBeforeToolCallContext<'_>,
            _: &CancellationToken,
        ) -> Result<ActualToolDecision, ActualProviderError> {
            Ok(ActualToolDecision::Allow)
        }
        fn transform_tool_call_arguments_actual(
            &self,
            args: ValueHandle,
            _: &WireString,
            _: &SessionValues,
        ) -> Result<ValueHandle, ActualProviderError> {
            Ok(args)
        }
        fn get_tool_context_actual(
            &self,
            context: &ActualToolExecutionContext,
            values: &SessionValues,
        ) -> Result<ValueHandle, ActualProviderError> {
            if self.mode == "shared-panic" {
                assert_eq!(context.total, 2);
                assert!(context.index < 2);
                assert_eq!(context.batch_id, WireString::from("71_A"));
            } else {
                assert_eq!(context.total, 1);
                assert_eq!(context.index, 0);
                assert_eq!(context.batch_id, WireString::from("71_call"));
            }
            Ok(values.heap().undefined()?)
        }
        fn tool_error_message_actual(
            &self,
            error: &ValueError,
            values: &SessionValues,
        ) -> Result<ValueHandle, ActualProviderError> {
            match error {
                ValueError::Thrown(thrown) => Ok(values
                    .heap()
                    .string(values.heap().to_string(thrown, values.runtime())?)?),
                _ => Ok(values.heap().string(error.to_string().into())?),
            }
        }
        fn sanitize_tool_result_text_actual(
            &self,
            text: &ValueHandle,
            values: &SessionValues,
        ) -> Result<ValueHandle, ActualProviderError> {
            self.sanitizer.sanitize(text, values)
        }
        async fn after_tool_call_actual(
            &self,
            context: ActualAfterToolCallContext<'_>,
            _: &CancellationToken,
        ) -> Result<Option<ValueHandle>, ActualProviderError> {
            self.after_calls.fetch_add(1, Ordering::SeqCst);
            if self.mode == "after-malformed" {
                return Ok(Some(
                    context.values.heap().import_serde(&json!({"content":3}))?,
                ));
            }
            if self.mode == "after-inplace" {
                let details = get(context.values, context.result, "details")?;
                own(
                    context.values,
                    &details,
                    "mutated",
                    context.values.heap().boolean(true)?,
                )?;
            }
            Ok(None)
        }
        async fn turn_end_actual_context(
            &self,
            context: ActualTurnEndContext<'_>,
            _: &CancellationToken,
        ) -> Result<Vec<AgentMessage>, ActualProviderError> {
            assert_eq!(context.tool_results.len(), 1);
            assert_eq!(context.context.messages.len(), 2);
            assert!(context.will_continue);
            self.turn_calls.fetch_add(1, Ordering::SeqCst);
            if matches!(
                self.mode,
                "turn-end-content" | "turn-end-control" | "turn-end-control-error"
            ) {
                let AgentMessageBody::Graph(result) = context.tool_results[0].body_snapshot()
                else {
                    panic!("turn-end lost its actual result owner");
                };
                if self.mode == "turn-end-content" {
                    let details = get(context.values, &result, "details")?;
                    own(
                        context.values,
                        &details,
                        "turnEndMutation",
                        context.values.heap().boolean(true)?,
                    )?;
                } else {
                    own(
                        context.values,
                        &result,
                        "toolCallId",
                        context.values.heap().string("retargeted".into())?,
                    )?;
                    if self.mode == "turn-end-control-error" {
                        return Err(ActualProviderError::Callback(
                            "turn-end returned error".into(),
                        ));
                    }
                    let input = context.values.heap().import_serde(&json!({
                        "role":"user","content":"must not commit after owner violation","timestamp":72
                    }))?;
                    return Ok(vec![AgentMessage::from_graph(input, None)?]);
                }
            }
            Ok(Vec::new())
        }
    }
    for mode in [
        "normal",
        "owned-i",
        "late-cancel",
        "throw",
        "malformed",
        "after-malformed",
        "after-inplace",
        "turn-end-content",
        "turn-end-control",
        "turn-end-control-error",
    ] {
        let heap = ValueHeap::new().unwrap();
        let values = Arc::new(SessionValues::new(heap.clone(), Arc::new(NoRuntime)).unwrap());
        let bun = heap.object().unwrap();
        let sanitizer = ActualToolResultSanitizer::new(bun, &values).unwrap();
        let lone = heap
            .string(WireString::from_units(vec![0xd800, 0xfffd, b'x' as u16]))
            .unwrap();
        assert_eq!(
            text(&values, &sanitizer.sanitize(&lone, &values).unwrap()).unwrap(),
            WireString::from("x")
        );
        let original_details = heap.object().unwrap();
        own(
            &values,
            &original_details,
            "large",
            heap.bigint_decimal("123456789012345678901234567890")
                .unwrap(),
        )
        .unwrap();
        let image = heap
            .import_serde(
                &json!({"type":"image","data":"eA==","mimeType":"image/png","extension":9}),
            )
            .unwrap();
        let raw = heap.object().unwrap();
        own(&values, &raw, "details", original_details.clone()).unwrap();
        if mode != "malformed" {
            own(
                &values,
                &raw,
                "content",
                heap.array(vec![
                    text_block(&values, heap.string("hi\r\n".into()).unwrap()).unwrap(),
                    image.clone(),
                ])
                .unwrap(),
            )
            .unwrap();
        }
        let source_tool = heap
            .import_serde(&json!({"name":"echo","parameters":{"type":"object","properties":{}}}))
            .unwrap();
        if mode == "owned-i" {
            own(&values,&source_tool,"parameters",heap.import_serde(&json!({"type":"object",
                "properties":{"i":{"type":"integer"}},"required":["i"],"additionalProperties":false})).unwrap()).unwrap();
        }
        let callable = heap.function(CapabilityId(1701)).unwrap();
        own(&values, &source_tool, "execute", callable.clone()).unwrap();
        let thrown = heap.symbol(Some("original execute throw".into())).unwrap();
        let invoker = Arc::new(Invoker {
            receiver: source_tool.clone(),
            callable,
            result: raw,
            thrown,
            mode,
            calls: AtomicUsize::new(0),
        });
        let source = Arc::new(Source {
            tool: source_tool.clone(),
            collection: heap.array(vec![source_tool]).unwrap(),
            capability: Arc::new(Tool {
                invoker: invoker.clone(),
            }),
        });
        let hooks = Arc::new(Hooks {
            sanitizer,
            after_calls: AtomicUsize::new(0),
            turn_calls: AtomicUsize::new(0),
            mode,
        });
        let config = AdmittedExecution {
            config: AgentConfig {
                model: Model {
                    id: "actual-execution".into(),
                    api: "openai-completions".into(),
                    provider: "fixture".into(),
                    base_url: String::new(),
                    reasoning: false,
                    max_tokens: None,
                    context_window: None,
                    tokenizer: None,
                },
                provider: Arc::new(Provider),
                system_prompt: vec![],
                tools: vec![],
                tool_choice: None,
                max_tokens: None,
                temperature: None,
                deadline: None,
                max_model_calls: Some(1),
                hooks: hooks.clone(),
                interrupt_mode: InterruptMode::Immediate,
            },
            provider_context: (mode == "owned-i").then(|| ProviderContextBinding {
                build_options: crate::BuildOptions {
                    intent_tracing: true,
                    ..Default::default()
                },
                ..Default::default()
            }),
        };
        let arguments = if mode == "owned-i" {
            json!({"i":42})
        } else {
            json!({})
        };
        let root = heap.import_serde(&json!({"role":"assistant","stopReason":"toolUse","timestamp":71,"content":[{"type":"toolCall","id":"call","name":"echo","arguments":arguments}]})).unwrap();
        let ctl = RunControl::new(&CancellationToken::new(), None);
        let sink = crate::RecordingSink::default();
        let mut context = Vec::new();
        let mut stream =
            super::actual::ActualStreamState::new(values.clone(), None, Some(source), None);
        stream
            .update(
                &mut context,
                &sink,
                ara_ai::ActualAssistantEvent::Start {
                    partial: root.clone(),
                },
            )
            .await
            .unwrap();
        let final_assistant = stream
            .finish(
                &mut context,
                &config,
                &ctl,
                &sink,
                Some(root),
                None,
                true,
                false,
            )
            .await;
        assert!(final_assistant.failure.is_none());
        let mut added = vec![final_assistant.message.clone()];
        let mut paused = 0;
        let processed = super::actual::process_actual_turn(
            &mut context,
            &mut added,
            final_assistant,
            &config,
            &ctl,
            &sink,
            &SoftToolRequirementState::default(),
            None,
            None,
            &mut paused,
        )
        .await;
        if matches!(mode, "turn-end-control" | "turn-end-control-error") {
            let error = processed.expect_err("turn-end cannot retarget the Owned tool result");
            if mode == "turn-end-control" {
                assert!(matches!(error.as_ref(), ActualProviderError::Owner(_)));
            } else {
                assert!(
                    matches!(error.as_ref(), ActualProviderError::Compound {primary,secondary}
                    if matches!(primary.as_ref(),ActualProviderError::Callback(message) if message=="turn-end returned error")
                    && matches!(secondary.as_ref(),ActualProviderError::Owner(_))),
                    "hook error stays primary and owner violation stays secondary"
                );
            }
            assert_eq!(invoker.calls.load(Ordering::SeqCst), 1);
            assert_eq!(hooks.after_calls.load(Ordering::SeqCst), 1);
            assert_eq!(hooks.turn_calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                context.len(),
                2,
                "rejected hook return cannot commit the extra input"
            );
            assert_eq!(
                added.len(),
                2,
                "rejected hook return cannot enter new_messages"
            );
            continue;
        }
        let (end, more) = processed.unwrap();
        assert_eq!(end, None);
        assert!(more);
        assert_eq!(invoker.calls.load(Ordering::SeqCst), 1);
        assert_eq!(hooks.after_calls.load(Ordering::SeqCst), 1);
        assert_eq!(hooks.turn_calls.load(Ordering::SeqCst), 1);
        let AgentMessageBody::Graph(result) = context.last().unwrap().body_snapshot() else {
            panic!("actual result was reprojected");
        };
        assert!(
            get(&values, &result, "details")
                .unwrap()
                .same_identity(&original_details)
                || mode == "throw"
        );
        let error = heap
            .truthy(&get(&values, &result, "isError").unwrap())
            .unwrap();
        assert_eq!(
            error,
            matches!(mode, "throw" | "malformed" | "after-malformed")
        );
        if matches!(
            mode,
            "normal" | "late-cancel" | "after-inplace" | "turn-end-content"
        ) {
            let content = get(&values, &result, "content").unwrap();
            assert!(get(&values, &content, "1").unwrap().same_identity(&image));
            assert_eq!(
                text(
                    &values,
                    &get(&values, &get(&values, &content, "0").unwrap(), "text").unwrap()
                )
                .unwrap(),
                WireString::from("hi\n")
            );
        }
        if mode == "turn-end-content" {
            assert!(
                heap.truthy(&get(&values, &original_details, "turnEndMutation").unwrap())
                    .unwrap(),
                "legal original details mutation remains admitted"
            );
        }
        let events = sink.events.lock().await;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AgentEvent::ActualToolExecutionStart { .. }))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AgentEvent::ActualToolExecutionEnd { .. }))
                .count(),
            1
        );
        assert!(
            events.iter().any(
                |event| matches!(event, AgentEvent::ActualTurnEnd { tool_results, .. } if tool_results.len() == 1)
            )
        );
    }

    // A real entered Future is parked while a sibling's publication panics.
    // A must settle before the batch returns; a dropped Future or implicit
    // retry is an observable failure, independent of the error string.
    #[derive(Default)]
    struct SharedState {
        entered: CancellationToken,
        release: CancellationToken,
        partial_published: CancellationToken,
        calls: AtomicUsize,
        settled: AtomicUsize,
        dropped_entered: AtomicUsize,
    }
    struct EnteredGuard {
        state: Arc<SharedState>,
        settled: bool,
    }
    impl Drop for EnteredGuard {
        fn drop(&mut self) {
            if !self.settled {
                self.state.dropped_entered.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    struct BlockingInvoker {
        receiver: ValueHandle,
        callable: ValueHandle,
        result: ValueHandle,
        state: Arc<SharedState>,
    }
    #[async_trait]
    impl ActualToolInvoker for BlockingInvoker {
        async fn invoke_settled(
            &self,
            request: ActualExecuteRequest,
        ) -> Result<ValueHandle, ActualProviderError> {
            request.check_admission()?;
            assert!(request.receiver.same_identity(&self.receiver));
            assert!(request.callable.same_identity(&self.callable));
            self.state.calls.fetch_add(1, Ordering::SeqCst);
            let mut guard = EnteredGuard {
                state: self.state.clone(),
                settled: false,
            };
            self.state.entered.cancel();
            (request.on_update)(self.result.clone())?;
            self.state.release.cancelled().await;
            self.state.settled.fetch_add(1, Ordering::SeqCst);
            guard.settled = true;
            Ok(self.result.clone())
        }
    }
    struct SharedTool(Arc<dyn ActualToolInvoker>);
    #[async_trait]
    impl AgentTool for SharedTool {
        fn definition(&self) -> ara_ai::Tool {
            panic!("shared actual dispatch used typed definition")
        }
        fn actual_invoker(&self) -> Option<Arc<dyn ActualToolInvoker>> {
            Some(self.0.clone())
        }
        async fn execute(
            &self,
            _: &str,
            _: JsonObject,
            _: CancellationToken,
            _: crate::tool::UpdateFn,
        ) -> Result<ToolOutput, crate::tool::ToolError> {
            panic!("shared actual dispatch used typed execute")
        }
    }
    struct SharedSource {
        collection: ValueHandle,
        bindings: Vec<ActualToolBinding>,
    }
    impl crate::tool::ActualToolSource for SharedSource {
        fn current_tools(
            &self,
            _: &SessionValues,
        ) -> Result<Option<ValueHandle>, ActualProviderError> {
            Ok(Some(self.collection.clone()))
        }
        fn bind_selected(
            &self,
            selected: &ValueHandle,
            values: &SessionValues,
        ) -> Result<ActualToolBinding, ActualProviderError> {
            let binding = self
                .bindings
                .iter()
                .find(|binding| binding.source.same_identity(selected))
                .ok_or(ActualProviderError::Protocol(
                    "shared fixture selected an unknown source",
                ))?
                .clone();
            binding.check_selected(selected, values)?;
            Ok(binding)
        }
        fn has_fallback(&self) -> bool {
            false
        }
    }
    struct PanicSink {
        events: tokio::sync::Mutex<Vec<AgentEvent>>,
        state: Arc<SharedState>,
        panicked: CancellationToken,
        at_start: bool,
        mutate_owner: bool,
        values: Arc<SessionValues>,
        b_call: ValueHandle,
    }
    #[async_trait]
    impl AgentEventSink for PanicSink {
        async fn emit(&self, event: AgentEvent) {
            let a_partial = matches!(&event, AgentEvent::ActualToolExecutionUpdate { tool_call_id, .. }
                if tool_call_id.equals_ascii("A"));
            let b_boundary = if self.at_start {
                matches!(&event, AgentEvent::ActualToolExecutionStart { tool_call_id, .. }
                    if tool_call_id.equals_ascii("B"))
            } else {
                matches!(&event, AgentEvent::ActualToolExecutionEnd { tool_call_id, .. }
                    if tool_call_id.equals_ascii("B"))
            };
            self.events.lock().await.push(event);
            if a_partial {
                self.state.partial_published.cancel();
            }
            if b_boundary {
                self.state.entered.cancelled().await;
                self.state.partial_published.cancelled().await;
                if self.mutate_owner {
                    own(
                        &self.values,
                        &self.b_call,
                        "id",
                        self.values.heap().string("B-mutated".into()).unwrap(),
                    )
                    .unwrap();
                }
                self.panicked.cancel();
                panic!("shared B publication panic");
            }
        }
    }
    fn callback_with_owner_secondary(error: &ActualProviderError) -> bool {
        match error {
            ActualProviderError::Compound { primary, secondary } => {
                (matches!(primary.as_ref(), ActualProviderError::Callback(message)
                    if message.contains("shared B publication panic"))
                    && matches!(secondary.as_ref(), ActualProviderError::Owner(_)))
                    || callback_with_owner_secondary(primary)
                    || callback_with_owner_secondary(secondary)
            }
            _ => false,
        }
    }
    for (at_start, mutate_owner) in [(true, false), (false, false), (false, true)] {
        let heap = ValueHeap::new().unwrap();
        let values = Arc::new(SessionValues::new(heap.clone(), Arc::new(NoRuntime)).unwrap());
        let state = Arc::new(SharedState::default());
        let output = caught_output(&values, heap.string("A settled".into()).unwrap()).unwrap();
        let a_source = heap
            .import_serde(&json!({"name":"a","concurrency":"shared",
            "parameters":{"type":"object","properties":{}}}))
            .unwrap();
        let b_source = heap
            .import_serde(&json!({"name":"b","concurrency":"shared",
            "parameters":{"type":"object","properties":{}}}))
            .unwrap();
        let a_callable = heap.function(CapabilityId(1711)).unwrap();
        let b_callable = heap.function(CapabilityId(1712)).unwrap();
        own(&values, &a_source, "execute", a_callable.clone()).unwrap();
        own(&values, &b_source, "execute", b_callable.clone()).unwrap();
        let a_invoker = Arc::new(BlockingInvoker {
            receiver: a_source.clone(),
            callable: a_callable,
            result: output.clone(),
            state: state.clone(),
        });
        let b_invoker = Arc::new(Invoker {
            receiver: b_source.clone(),
            callable: b_callable,
            result: caught_output(&values, heap.string("B settled".into()).unwrap()).unwrap(),
            thrown: heap.symbol(Some("unused shared throw".into())).unwrap(),
            mode: "normal",
            calls: AtomicUsize::new(0),
        });
        let source = Arc::new(SharedSource {
            collection: heap
                .array(vec![a_source.clone(), b_source.clone()])
                .unwrap(),
            bindings: vec![
                ActualToolBinding {
                    source: a_source,
                    capability: Arc::new(SharedTool(a_invoker)),
                },
                ActualToolBinding {
                    source: b_source,
                    capability: Arc::new(SharedTool(b_invoker.clone())),
                },
            ],
        });
        let hooks = Arc::new(Hooks {
            sanitizer: ActualToolResultSanitizer::new(heap.object().unwrap(), &values).unwrap(),
            after_calls: AtomicUsize::new(0),
            turn_calls: AtomicUsize::new(0),
            mode: "shared-panic",
        });
        let config = AdmittedExecution {
            config: AgentConfig {
                model: Model {
                    id: "shared-panic".into(),
                    api: "openai-completions".into(),
                    provider: "fixture".into(),
                    base_url: String::new(),
                    reasoning: false,
                    max_tokens: None,
                    context_window: None,
                    tokenizer: None,
                },
                provider: Arc::new(Provider),
                system_prompt: vec![],
                tools: vec![],
                tool_choice: None,
                max_tokens: None,
                temperature: None,
                deadline: None,
                max_model_calls: Some(1),
                hooks,
                interrupt_mode: InterruptMode::Immediate,
            },
            provider_context: None,
        };
        let root = heap
            .import_serde(
                &json!({"role":"assistant","stopReason":"toolUse","timestamp":71,
            "content":[{"type":"toolCall","id":"A","name":"a","arguments":{}},
                       {"type":"toolCall","id":"B","name":"b","arguments":{}}]}),
            )
            .unwrap();
        let b_call = get(&values, &get(&values, &root, "content").unwrap(), "1").unwrap();
        let sink = PanicSink {
            events: tokio::sync::Mutex::new(Vec::new()),
            state: state.clone(),
            panicked: CancellationToken::new(),
            at_start,
            mutate_owner,
            values: values.clone(),
            b_call,
        };
        let ctl = RunControl::new(&CancellationToken::new(), None);
        let mut context = Vec::new();
        let mut stream =
            super::actual::ActualStreamState::new(values.clone(), None, Some(source), None);
        stream
            .update(
                &mut context,
                &sink,
                ara_ai::ActualAssistantEvent::Start {
                    partial: root.clone(),
                },
            )
            .await
            .unwrap();
        let final_assistant = stream
            .finish(
                &mut context,
                &config,
                &ctl,
                &sink,
                Some(root),
                None,
                true,
                false,
            )
            .await;
        assert!(final_assistant.failure.is_none());
        let execution = execute_actual_calls(&final_assistant, &context, &config, &ctl, &sink);
        tokio::pin!(execution);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::select! {
                _ = execution.as_mut() => panic!("batch returned before entered A settled"),
                _ = sink.panicked.cancelled() => {},
            }
        })
        .await
        .expect("B must reach its publication panic while A is entered");
        assert!(
            futures::poll!(execution.as_mut()).is_pending(),
            "allSettled waits for entered A"
        );
        assert!(state.entered.is_cancelled() && state.partial_published.is_cancelled());
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
        assert_eq!(state.settled.load(Ordering::SeqCst), 0);
        assert_eq!(state.dropped_entered.load(Ordering::SeqCst), 0);
        state.release.cancel();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), execution.as_mut())
            .await
            .expect("entered A must settle despite B sink panic");
        assert_eq!(
            state.calls.load(Ordering::SeqCst),
            1,
            "A was never replayed"
        );
        assert_eq!(state.settled.load(Ordering::SeqCst), 1);
        assert_eq!(
            state.dropped_entered.load(Ordering::SeqCst),
            0,
            "entered A was never dropped"
        );
        assert_eq!(
            b_invoker.calls.load(Ordering::SeqCst),
            usize::from(!at_start),
            "B was never replayed"
        );
        let failure = outcome
            .failure
            .as_ref()
            .expect("sink panic must remain an explicit failure");
        if mutate_owner {
            assert!(
                callback_with_owner_secondary(failure),
                "callback is primary; owner failure remains secondary"
            );
            assert!(
                outcome.results.is_empty(),
                "stale owner does not admit a result despite known settlement"
            );
        } else {
            assert!(
                matches!(failure.as_ref(), ActualProviderError::Callback(message)
                if message.contains("shared B publication panic"))
            );
            assert_eq!(outcome.results.len(), 1);
            let AgentMessageBody::Graph(result) = outcome.results[0].body_snapshot() else {
                panic!("A lost Graph result")
            };
            assert_eq!(
                text(&values, &get(&values, &result, "toolCallId").unwrap()).unwrap(),
                WireString::from("A")
            );
            assert!(
                !heap
                    .truthy(&get(&values, &result, "isError").unwrap())
                    .unwrap()
            );
            assert!(
                get(&values, &result, "details")
                    .unwrap()
                    .same_identity(&get(&values, &output, "details").unwrap())
            );
            let events = sink.events.lock().await;
            assert!(events.iter().any(
                |event| matches!(event, AgentEvent::ActualToolExecutionUpdate {tool_call_id,..}
                if tool_call_id.equals_ascii("A"))
            ));
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::ActualToolExecutionEnd {tool_call_id,is_error:false,..}
                if tool_call_id.equals_ascii("A"))),
                "A remains known successful, not skipped/unknown"
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, AgentEvent::ActualToolExecutionStart {tool_call_id,..}
                if tool_call_id.equals_ascii("A")))
                    .count(),
                1
            );
        }
    }
}
