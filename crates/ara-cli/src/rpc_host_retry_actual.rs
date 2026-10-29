//! Actual Native recovery decisions over the original calling Session records.
//! Fixed OMP596f2da turn-recovery.ts:1213-1417,2063-2110; MIT, THIRD_PARTY_NOTICES.md.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc.

use super::RetryDisposition;
use ara_agent::{ActualProviderError, AgentMessage, AgentMessageBody, SessionValues};
use ara_ai::{
    ContextRecoveryEvidence, Model,
    retry_classification::{RetryClass, flag},
};
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, IteratorRecord, ValueError, ValueHandle, ValueHeap, ValueRuntime,
        ValueSnapshot,
    },
};
use std::{collections::HashSet, sync::Arc};

fn get(
    heap: &Arc<ValueHeap>,
    root: &ValueHandle,
    key: &str,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    heap.get(root, &key.into(), runtime)
}
fn method(
    heap: &Arc<ValueHeap>,
    root: &ValueHandle,
    key: &str,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let callable = get(heap, root, key, runtime)?;
    heap.call(&callable, root, args, runtime)
}
fn ascii(heap: &Arc<ValueHeap>, value: &ValueHandle, expected: &str) -> Result<bool, ValueError> {
    Ok(matches!(heap.snapshot(value)?, ValueSnapshot::String(value) if value.equals_ascii(expected)))
}
fn nullish(heap: &Arc<ValueHeap>, value: &ValueHandle) -> Result<bool, ValueError> {
    Ok(matches!(heap.snapshot(value)?, ValueSnapshot::Null | ValueSnapshot::Undefined))
}
fn text(heap: &Arc<ValueHeap>, value: &ValueHandle) -> Result<WireString, ValueError> {
    match heap.snapshot(value)? {
        ValueSnapshot::String(value) => Ok(value),
        _ => Err(ValueError::TypeError("Native retry identity must be a String".into())),
    }
}

fn current_root(owner: &AgentMessage, values: &SessionValues) -> Result<ValueHandle, ActualProviderError> {
    let AgentMessageBody::Graph(root) = owner.body_snapshot() else {
        return Err(ActualProviderError::Unsupported("Native retry requires the original Graph record"));
    };
    values.check_handle(&root)?;
    owner.recheck_graph_root(&root)?;
    Ok(root)
}

/// Private saga receipt over the canonical original records. Holding it does
/// not reserve effects or authorize dispatch. Host rechecks after each await.
pub(in super::super) struct NativeRetryContextReceipt {
    values: Arc<SessionValues>,
    records: Vec<(AgentMessage, ValueHandle)>,
}
impl NativeRetryContextReceipt {
    pub fn capture(messages: &[AgentMessage], values: Arc<SessionValues>) -> Result<Self, ActualProviderError> {
        let records = messages
            .iter()
            .map(|record| Ok((record.clone(), current_root(record, &values)?)))
            .collect::<Result<_, ActualProviderError>>()?;
        Ok(Self { values, records })
    }
    pub fn append(&mut self, message: &AgentMessage) -> Result<(), ActualProviderError> {
        self.records.push((message.clone(), current_root(message, &self.values)?));
        Ok(())
    }
    pub fn recheck(&self, messages: &[AgentMessage], values: &Arc<SessionValues>) -> Result<(), ActualProviderError> {
        if !Arc::ptr_eq(self.values.factory(), values.factory()) {
            return Err(ActualProviderError::Unsupported("Native retry Session runtime changed"));
        }
        if messages.len() != self.records.len() {
            return Err(ActualProviderError::Unsupported("Native retry canonical context changed"));
        }
        for ((record, root), current) in self.records.iter().zip(messages) {
            if !record.same_record(current) {
                return Err(ActualProviderError::Unsupported("Native retry canonical record changed"));
            }
            values.check_handle(root)?;
            current.recheck_graph_root(root)?;
        }
        Ok(())
    }
}

pub(in super::super) fn last_assistant_actual(
    messages: &[AgentMessage],
    values: &SessionValues,
) -> Result<Option<AgentMessage>, ActualProviderError> {
    for message in messages.iter().rev() {
        let root = current_root(message, values)?;
        let assistant = guarded(&[(message.clone(), root.clone())], || {
            Ok(ascii(values.heap(), &get(values.heap(), &root, "role", values.runtime())?, "assistant")?)
        })?;
        if assistant {
            return Ok(Some(message.clone()));
        }
    }
    Ok(None)
}

/// Fixed pending annotation is attached to the original failed record. The
/// durable entry ID is supplied by the current journal/projector owner, never
/// inferred from JSON resemblance. Host must rewrite/journal this mutation and
/// accept its receipt before publishing auto_retry_end.
pub(in super::super) fn annotate_retry_actual(
    failed: &AgentMessage,
    superseding: Option<&AgentMessage>,
    attempt: usize,
    recovery: &str,
    note: &str,
    recovered_at: Option<&str>,
    values: &SessionValues,
) -> Result<ValueHandle, ActualProviderError> {
    let root = current_root(failed, values)?;
    let mut records = vec![(failed.clone(), root.clone())];
    if let Some(superseding) = superseding {
        records.push((superseding.clone(), current_root(superseding, values)?));
    }
    guarded(&records, || {
        let heap = values.heap();
        let runtime = values.runtime();
        let annotation = heap.object()?;
        for (key, value) in [
            ("kind", "auto-retry"),
            ("status", if superseding.is_some() { "recovered" } else { "superseded" }),
            ("recovery", recovery),
            ("note", note),
        ] {
            heap.define_own_data(&annotation, key.into(), heap.string(value.into())?, runtime)?;
        }
        heap.define_own_data(&annotation, "attempt".into(), heap.number(attempt as f64)?, runtime)?;
        if let Some((_, completed)) = records.get(1) {
            let recovered_at = recovered_at
                .ok_or(ActualProviderError::Unsupported("Native retry recoveredAt Host clock is not bound"))?;
            heap.define_own_data(&annotation, "recoveredAt".into(), heap.string(recovered_at.into())?, runtime)?;
            let by = heap.object()?;
            heap.define_own_data(&by, "timestamp".into(), get(heap, completed, "timestamp", runtime)?, runtime)?;
            let response_id = get(heap, completed, "responseId", runtime)?;
            if !matches!(heap.snapshot(&response_id)?, ValueSnapshot::Undefined) {
                // Fixed conditional checks first, then reads the member again.
                heap.define_own_data(&by, "responseId".into(), get(heap, completed, "responseId", runtime)?, runtime)?;
            }
            for key in ["provider", "model"] {
                heap.define_own_data(&by, key.into(), get(heap, completed, key, runtime)?, runtime)?;
            }
            heap.define_own_data(&annotation, "supersededBy".into(), by, runtime)?;
        }
        heap.assign(&root, "retryRecovery".into(), annotation.clone(), runtime)?;
        Ok(annotation)
    })
}

// Capture/recheck does not execute Native getters. Both success and failure
// retain root changes; a Native Throw remains the primary receipt.
fn guarded<T>(
    records: &[(AgentMessage, ValueHandle)],
    action: impl FnOnce() -> Result<T, ActualProviderError>,
) -> Result<T, ActualProviderError> {
    let result = action();
    let admission =
        records.iter().try_for_each(|(owner, root)| owner.recheck_graph_root(root).map_err(ActualProviderError::from));
    match (result, admission) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Err(secondary)) => {
            Err(ActualProviderError::Compound { primary: Arc::new(primary), secondary: Arc::new(secondary) })
        }
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
    }
}
fn close_error(
    heap: &Arc<ValueHeap>,
    iterator: &mut IteratorRecord,
    runtime: &dyn ValueRuntime,
    primary: ActualProviderError,
) -> ActualProviderError {
    match heap.iterator_close(iterator, runtime) {
        Ok(()) => primary,
        Err(close)
            if matches!(&primary, ActualProviderError::Value(error) if error.is_native_exception())
                && close.is_native_exception() =>
        {
            primary
        }
        Err(secondary) => {
            ActualProviderError::Compound { primary: Arc::new(primary), secondary: Arc::new(secondary.into()) }
        }
    }
}
fn visit_content<T>(
    values: &SessionValues,
    root: &ValueHandle,
    mut visit: impl FnMut(ValueHandle) -> Result<Option<T>, ActualProviderError>,
) -> Result<Option<T>, ActualProviderError> {
    let heap = values.heap();
    let runtime = values.runtime();
    let content = get(heap, root, "content", runtime)?;
    let mut iterator = heap.get_iterator(&content, runtime)?;
    while let Some(block) = heap.iterator_next(&mut iterator, runtime)? {
        match visit(block) {
            Ok(Some(value)) => {
                heap.iterator_close(&mut iterator, runtime)?;
                return Ok(Some(value));
            }
            Ok(None) => {}
            Err(error) => return Err(close_error(heap, &mut iterator, runtime, error)),
        }
    }
    Ok(None)
}

fn thinking_predicate(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let block = args.first().ok_or(ValueError::UnsupportedOperation("Native thinking predicate argument"))?;
    let kind = get(heap, block, "type", runtime)?;
    let nonempty = if ascii(heap, &kind, "thinking")? {
        let thinking = get(heap, block, "thinking", runtime)?;
        let trimmed = method(heap, &thinking, "trim", &[], runtime)?;
        let length = get(heap, &trimmed, "length", runtime)?;
        heap.to_number(&length, runtime)? > 0.0
    } else {
        false
    };
    heap.boolean(nonempty)
}
static THINKING_PREDICATE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da.turn-recovery.isBoundedThinkingStreamClose.some",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: thinking_predicate,
};
fn unsafe_predicate(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let block = args.first().ok_or(ValueError::UnsupportedOperation("Native unsafe predicate argument"))?;
    for expected in ["toolCall", "image", "anthropicServerTool"] {
        if ascii(heap, &get(heap, block, "type", runtime)?, expected)? {
            return heap.boolean(true);
        }
    }
    if ascii(heap, &get(heap, block, "type", runtime)?, "text")? && heap.truthy(&captures[0])? {
        let text = get(heap, block, "text", runtime)?;
        let trimmed = method(heap, &text, "trim", &[], runtime)?;
        let length = get(heap, &trimmed, "length", runtime)?;
        return heap.boolean(heap.to_number(&length, runtime)? > 0.0);
    }
    heap.boolean(false)
}
static UNSAFE_PREDICATE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da.turn-recovery.hasReplayUnsafeOutput.some",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: unsafe_predicate,
};
fn has_unsafe_output(values: &SessionValues, root: &ValueHandle, committed: bool) -> Result<bool, ValueError> {
    let content = get(values.heap(), root, "content", values.runtime())?;
    let committed = values.heap().boolean(committed)?;
    let callback = values.heap().fresh_compiled_function(&UNSAFE_PREDICATE, &[committed])?;
    let result = method(values.heap(), &content, "some", &[callback], values.runtime())?;
    values.heap().truthy(&result)
}
fn regex_test(values: &SessionValues, source: &str, input: &ValueHandle) -> Result<bool, ValueError> {
    regex_test_flags(values, source, "i", input)
}
fn regex_test_flags(
    values: &SessionValues,
    source: &str,
    flags: &str,
    input: &ValueHandle,
) -> Result<bool, ValueError> {
    let regexp = values.heap().regexp(source.into(), flags)?;
    let result = method(values.heap(), &regexp, "test", &[input.clone()], values.runtime())?;
    values.heap().truthy(&result)
}

pub(in super::super) fn effective_max_retries_actual(
    message: &AgentMessage,
    configured: f64,
    values: &SessionValues,
) -> Result<f64, ActualProviderError> {
    let root = current_root(message, values)?;
    guarded(&[(message.clone(), root.clone())], || {
        let heap = values.heap();
        let runtime = values.runtime();
        let content = get(heap, &root, "content", runtime)?;
        let callback = heap.fresh_compiled_function(&THINKING_PREDICATE, &[])?;
        let thinking = method(heap, &content, "some", &[callback], runtime)?;
        if !heap.truthy(&thinking)? {
            return Ok(configured);
        }
        let error = get(heap, &root, "errorMessage", runtime)?;
        let error = if nullish(heap, &error)? { heap.string("".into())? } else { error };
        let provider = get(heap, &root, "provider", runtime)?;
        if ascii(heap, &provider, "openrouter")?
            && regex_test(values, r"server_error:\s*stream closed with reason:\s*error", &error)?
        {
            return Ok(if configured.is_nan() { f64::NAN } else { configured.min(1.0) });
        }
        // Fixed Native OR expression reads provider again on the second arm.
        let provider = get(heap, &root, "provider", runtime)?;
        if ascii(heap, &provider, "github-copilot")? {
            let model = get(heap, &root, "model", runtime)?;
            if ascii(heap, &model, "grok-4.6")? {
                let api = get(heap, &root, "api", runtime)?;
                if ascii(heap, &api, "openai-responses")?
                    && regex_test(
                        values,
                        "OpenAI responses stream closed before a terminal response event was received",
                        &error,
                    )?
                {
                    return Ok(if configured.is_nan() { f64::NAN } else { configured.min(1.0) });
                }
            }
        }
        Ok(configured)
    })
}

/// These are the imported fixed functions in the calling Session runtime.
/// A missing binding must fail explicitly, never fall back to scalar metadata.
pub(in super::super) trait NativeRetryClassifierPort {
    fn classify_message(&self, message: &ValueHandle, values: &SessionValues) -> Result<ValueHandle, ValueError>;
    fn is_context_overflow(
        &self,
        message: &ValueHandle,
        context_window: f64,
        values: &SessionValues,
    ) -> Result<bool, ValueError>;
}
pub(in super::super) struct NativeRetryClassifierFunctions {
    pub receiver: ValueHandle,
    pub classify_message: ValueHandle,
    pub is_context_overflow: ValueHandle,
}
impl NativeRetryClassifierPort for NativeRetryClassifierFunctions {
    fn classify_message(&self, message: &ValueHandle, values: &SessionValues) -> Result<ValueHandle, ValueError> {
        for value in [&self.receiver, &self.classify_message, message] {
            values.check_handle(value)?;
        }
        let id = values.heap().call(&self.classify_message, &self.receiver, &[message.clone()], values.runtime())?;
        if !matches!(values.heap().snapshot(&id)?, ValueSnapshot::Number(_)) {
            return Err(ValueError::TypeError("Fixed Native classifyMessage must return Number".into()));
        }
        Ok(id)
    }
    fn is_context_overflow(
        &self,
        message: &ValueHandle,
        context_window: f64,
        values: &SessionValues,
    ) -> Result<bool, ValueError> {
        for value in [&self.receiver, &self.is_context_overflow, message] {
            values.check_handle(value)?;
        }
        let window = values.heap().number(context_window)?;
        let result = values.heap().call(
            &self.is_context_overflow,
            &self.receiver,
            &[message.clone(), window],
            values.runtime(),
        )?;
        match values.heap().snapshot(&result)? {
            ValueSnapshot::Boolean(result) => Ok(result),
            _ => Err(ValueError::TypeError("Fixed Native isContextOverflow must return Boolean".into())),
        }
    }
}
pub(in super::super) struct NativeRetryMetadata {
    pub class: RetryClass,
    /// The exact original String/null/undefined handle. Project only at output.
    pub error_message: ValueHandle,
    pub context_recovery: Option<ContextRecoveryEvidence>,
}
// ECMA ToUint32 for bitwise flags; Rust's saturating float cast is not JS.
fn number_bits(value: f64) -> u32 {
    if !value.is_finite() || value == 0.0 {
        return 0;
    }
    value.trunc().rem_euclid(4_294_967_296.0) as u32
}
fn retry_bits(id: u32) -> bool {
    id & (flag::CONTENT_BLOCKED | flag::PAYLOAD_REJECTED) == 0
        && id
            & (flag::MALFORMED_FUNCTION_CALL
                | flag::TRANSIENT
                | flag::USAGE_LIMIT
                | flag::THINKING_LOOP
                | flag::STALE_RESPONSES_ITEM
                | flag::PROVIDER_FINISH_ERROR
                | flag::EMPTY_RESPONSE)
            != 0
}
fn classify_active(
    model: &Model,
    root: &ValueHandle,
    values: &SessionValues,
    port: &dyn NativeRetryClassifierPort,
) -> Result<u32, ValueError> {
    let heap = values.heap();
    let runtime = values.runtime();
    let api = get(heap, root, "api", runtime)?;
    let id = if ascii(heap, &api, &model.api)? {
        // The fixed function owns its original Get/Set sequence and UTF16 text.
        port.classify_message(root, values)?
    } else {
        // Fixed active-route override deliberately copies only these values.
        let view = heap.object()?;
        heap.define_own_data(&view, "api".into(), heap.string(model.api.clone().into())?, runtime)?;
        for key in ["errorId", "errorMessage", "errorStatus"] {
            heap.define_own_data(&view, key.into(), get(heap, root, key, runtime)?, runtime)?;
        }
        let id = port.classify_message(&view, values)?;
        heap.assign(root, "errorId".into(), id.clone(), runtime)?;
        id
    };
    match heap.snapshot(&id)? {
        ValueSnapshot::Number(value) => Ok(number_bits(value)),
        _ => Err(ValueError::TypeError("Fixed Native classification id must be Number".into())),
    }
}

/// Actual route classification keeps all original reads and Native callbacks.
/// Host abort/dispose/streaming-edit and authorization gates remain separate.
pub(in super::super) fn same_route_class_actual(
    model: &Model,
    message: &AgentMessage,
    values: &SessionValues,
    port: &dyn NativeRetryClassifierPort,
) -> Result<Option<NativeRetryMetadata>, ActualProviderError> {
    let root = current_root(message, values)?;
    guarded(&[(message.clone(), root.clone())], || {
        let heap = values.heap();
        let runtime = values.runtime();
        let stop = get(heap, &root, "stopReason", runtime)?;
        let errored = ascii(heap, &stop, "error")?;
        let aborted = ascii(heap, &stop, "aborted")?;
        if !errored && !aborted {
            return Ok(None);
        }
        if errored {
            let error = get(heap, &root, "errorMessage", runtime)?;
            if !nullish(heap, &error)? {
                let prefix =
                    method(heap, &error, "startsWith", &[heap.string("Usage preflight blocked:".into())?], runtime)?;
                if matches!(heap.snapshot(&prefix)?, ValueSnapshot::Boolean(true)) {
                    return Ok(None);
                }
            }
            if model.api == "anthropic-messages" {
                let status = get(heap, &root, "errorStatus", runtime)?;
                let mut status_400 = matches!(heap.snapshot(&status)?, ValueSnapshot::Number(400.0));
                if !status_400 {
                    let id = get(heap, &root, "errorId", runtime)?;
                    status_400 = matches!(heap.snapshot(&id)?, ValueSnapshot::Number(400.0));
                }
                if !status_400 {
                    let error = get(heap, &root, "errorMessage", runtime)?;
                    if !nullish(heap, &error)? {
                        let prefix = method(heap, &error, "startsWith", &[heap.string("400 ".into())?], runtime)?;
                        status_400 = matches!(heap.snapshot(&prefix)?, ValueSnapshot::Boolean(true));
                    }
                }
                if status_400 {
                    let error = get(heap, &root, "errorMessage", runtime)?;
                    let error = if nullish(heap, &error)? { heap.string("".into())? } else { error };
                    if regex_test_flags(
                        values,
                        r"messages\.\d+\.content\.\d+.*\b(?:thinking|redacted_thinking)\b.*\blatest assistant message cannot be modified\b",
                        "is",
                        &error,
                    )? {
                        return Ok(None);
                    }
                }
            }
        }
        let mut id = classify_active(model, &root, values, port)?;
        let overflow = port.is_context_overflow(&root, model.context_window.unwrap_or(0.0), values)?;
        // Fixed isRetryableError classifies (including Native errorId Set),
        // checks overflow, then observes the current isClassifierRefusal fields.
        let refusal = if errored && !overflow && ascii(heap, &get(heap, &root, "stopReason", runtime)?, "error")? {
            let details = get(heap, &root, "stopDetails", runtime)?;
            if nullish(heap, &details)? {
                false
            } else {
                let kind = get(heap, &details, "type", runtime)?;
                ascii(heap, &kind, "refusal")? || ascii(heap, &kind, "sensitive")?
            }
        } else {
            false
        };
        let error = get(heap, &root, "errorMessage", runtime)?;
        let generic = ascii(heap, &error, "Request was aborted")? || ascii(heap, &error, "Request was aborted.")?;
        let reasonless = aborted && id & flag::ABORT != 0 || generic;
        if reasonless && generic {
            id = flag::CLASS | flag::ABORT;
            heap.assign(&root, "errorId".into(), heap.number(f64::from(id))?, runtime)?;
        }
        let error_view = if nullish(heap, &error)? { heap.string("".into())? } else { error.clone() };
        let interrupted = errored
            && (regex_test(values, "stream stall", &error_view)?
                || regex_test(
                    values,
                    r"stream closed with error code\s+nghttp2_(?:internal_error|refused_stream)|nghttp2_(?:internal_error|refused_stream)|HTTP2(?:StreamReset|RefusedStream)",
                    &error_view,
                )?
                || regex_test(
                    values,
                    "stream closed before a (?:finish_reason|terminal response event)",
                    &error_view,
                )?)
            && retry_bits(id);
        let mut class = RetryClass {
            error_id: id,
            overflow,
            stale_responses: id & flag::STALE_RESPONSES_ITEM != 0,
            usage_limit: id & flag::USAGE_LIMIT != 0,
            malformed: id & flag::MALFORMED_FUNCTION_CALL != 0,
            abort: reasonless,
            interrupted_stream: interrupted,
            retriable: errored && !overflow && (refusal || id & flag::ACCOUNT_POLICY != 0 || retry_bits(id)),
            ..Default::default()
        };
        let evidence = get(heap, &root, "failureEvidence", runtime)?;
        let mut context_recovery = None;
        if !nullish(heap, &evidence)? {
            let blocked = get(heap, &evidence, "sameRouteBlocked", runtime)?;
            class.replay_blocked = matches!(heap.snapshot(&blocked)?, ValueSnapshot::Boolean(true));
            let context = get(heap, &evidence, "contextRecovery", runtime)?;
            context_recovery = if ascii(heap, &context, "contentOnly")? {
                Some(ContextRecoveryEvidence::ContentOnly)
            } else if ascii(heap, &context, "usageAdmission")? {
                Some(ContextRecoveryEvidence::UsageAdmission)
            } else if !nullish(heap, &context)? {
                class.context_recovery_blocked = true;
                None
            } else {
                None
            };
            let wait = get(heap, &evidence, "waitMs", runtime)?;
            if let ValueSnapshot::Number(value) = heap.snapshot(&wait)? {
                class.wait_ms = (value.is_finite() && value >= 0.0).then_some(value);
            }
        }
        class.context_recovery_blocked |= class.usage_limit;
        Ok(Some(NativeRetryMetadata { class, error_message: error, context_recovery }))
    })
}

/// No content/argument serialization and no timestamp/content resemblance.
/// Canonical Agent ownership identifies exactly one original failed record;
/// all later results are original same-Session execution receipts.
pub(in super::super) fn replay_disposition_actual(
    message: &AgentMessage,
    messages: &[AgentMessage],
    class: &RetryClass,
    values: &SessionValues,
    text_output_committed: bool,
) -> Result<Option<RetryDisposition>, ActualProviderError> {
    if class.overflow || class.replay_blocked {
        return Ok(None);
    }
    let matches: Vec<usize> =
        messages.iter().enumerate().filter_map(|(at, owner)| owner.same_record(message).then_some(at)).collect();
    if matches.len() != 1 {
        return Ok(None);
    }
    let at = matches[0];
    let mut records = vec![(message.clone(), current_root(message, values)?)];
    for owner in &messages[at + 1..] {
        records.push((owner.clone(), current_root(owner, values)?));
    }
    guarded(&records, || {
        let heap = values.heap();
        let runtime = values.runtime();
        let root = &records[0].1;
        let mut calls = HashSet::<(WireString, WireString)>::new();
        let mut unsafe_output = false;
        let preserved_interruption = class.interrupted_stream && class.retriable || class.abort;
        visit_content::<()>(values, root, |block| {
            let kind = get(heap, &block, "type", runtime)?;
            if ascii(heap, &kind, "toolCall")? {
                let id = get(heap, &block, "id", runtime)?;
                let name = get(heap, &block, "name", runtime)?;
                calls.insert((text(heap, &id)?, text(heap, &name)?));
                return Ok(None);
            }
            if preserved_interruption {
                return Ok(None);
            }
            // Fixed source re-reads dynamic block.type at each OR/AND arm.
            let kind = get(heap, &block, "type", runtime)?;
            if ascii(heap, &kind, "image")? {
                unsafe_output = true;
                return Ok(None);
            }
            let kind = get(heap, &block, "type", runtime)?;
            if ascii(heap, &kind, "anthropicServerTool")? {
                unsafe_output = true;
                return Ok(None);
            }
            let kind = get(heap, &block, "type", runtime)?;
            if ascii(heap, &kind, "text")? && text_output_committed {
                let body = get(heap, &block, "text", runtime)?;
                unsafe_output |= regex_test(values, r"\S", &body)?;
            }
            Ok(None)
        })?;
        if calls.is_empty() {
            return Ok((at + 1 == messages.len() && !has_unsafe_output(values, root, text_output_committed)?)
                .then_some(RetryDisposition::RemoveFailed));
        }
        let mut received = HashSet::new();
        let mut all_unexecuted = true;
        for (_, root) in records.iter().skip(1) {
            let role = get(heap, root, "role", runtime)?;
            if !ascii(heap, &role, "toolResult")? {
                return Ok(None);
            }
            let id = get(heap, root, "toolCallId", runtime)?;
            let name = get(heap, root, "toolName", runtime)?;
            let key = (text(heap, &id)?, text(heap, &name)?);
            if !calls.contains(&key) {
                return Ok(None);
            }
            let details = get(heap, root, "details", runtime)?;
            let (synthetic, unexecuted) = if nullish(heap, &details)? {
                (false, false)
            } else {
                let executed = get(heap, &details, "executed", runtime)?;
                if ascii(heap, &executed, "unknown")? {
                    return Ok(None);
                }
                // Actual Core marks entered-but-unsettled execution this way.
                // A matching ID alone cannot prove its external effect known.
                let execution = get(heap, &details, "execution", runtime)?;
                if ascii(heap, &execution, "started")? || ascii(heap, &execution, "unknown")? {
                    return Ok(None);
                }
                let interrupted = get(heap, &details, "__interrupted", runtime)?;
                if matches!(heap.snapshot(&interrupted)?, ValueSnapshot::Boolean(true))
                    && !matches!(heap.snapshot(&executed)?, ValueSnapshot::Boolean(false | true))
                {
                    return Ok(None);
                }
                let panicked = get(heap, &details, "panicked", runtime)?;
                if matches!(heap.snapshot(&panicked)?, ValueSnapshot::Boolean(true)) {
                    return Ok(None);
                }
                let synthetic = get(heap, &details, "__synthetic", runtime)?;
                (
                    matches!(heap.snapshot(&synthetic)?, ValueSnapshot::Boolean(true)),
                    matches!(heap.snapshot(&executed)?, ValueSnapshot::Boolean(false)),
                )
            };
            if synthetic && !unexecuted {
                return Ok(None);
            }
            all_unexecuted &= synthetic && unexecuted;
            received.insert(key);
        }
        if calls.iter().any(|key| !received.contains(key)) {
            return Ok(None);
        }
        if preserved_interruption {
            return Ok(Some(RetryDisposition::PreserveFailed));
        }
        Ok((all_unexecuted && !unsafe_output).then_some(RetryDisposition::PreserveFailed))
    })
}
pub(in super::super) fn disposition_actual(
    message: &AgentMessage,
    messages: &[AgentMessage],
    class: &RetryClass,
    values: &SessionValues,
    text_output_committed: bool,
) -> Result<Option<RetryDisposition>, ActualProviderError> {
    let Some(disposition) = replay_disposition_actual(message, messages, class, values, text_output_committed)? else {
        return Ok(None);
    };
    let same_route = match disposition {
        RetryDisposition::RemoveFailed => class.retriable || class.abort,
        RetryDisposition::PreserveFailed => class.retriable || class.abort || class.malformed,
    };
    Ok(same_route.then_some(disposition))
}

/// Date.parse and the imported extractRetryHint keep the actual Host runtime
/// binding, full UTF16 input and original errors. Missing ports are explicit.
/// A display DTO is never supplied to these callbacks.
pub(in super::super) trait NativeRetryHintPort {
    fn date_parse(&self, source: &ValueHandle, values: &SessionValues) -> Result<f64, ValueError>;
    fn extract_retry_hint(
        &self,
        source: &ValueHandle,
        now_ms: f64,
        values: &SessionValues,
    ) -> Result<Option<f64>, ValueError>;
}
pub(in super::super) struct NativeRetryHintFunctions {
    pub date_receiver: ValueHandle,
    pub extract_retry_hint: ValueHandle,
}
impl NativeRetryHintPort for NativeRetryHintFunctions {
    fn date_parse(&self, source: &ValueHandle, values: &SessionValues) -> Result<f64, ValueError> {
        for value in [&self.date_receiver, source] {
            values.check_handle(value)?;
        }
        let value = method(values.heap(), &self.date_receiver, "parse", &[source.clone()], values.runtime())?;
        match values.heap().snapshot(&value)? {
            ValueSnapshot::Number(value) => Ok(value),
            _ => Err(ValueError::TypeError("Fixed Native Date.parse must return a Number".into())),
        }
    }
    fn extract_retry_hint(
        &self,
        source: &ValueHandle,
        _now_ms: f64,
        values: &SessionValues,
    ) -> Result<Option<f64>, ValueError> {
        values.check_handle(&self.extract_retry_hint)?;
        values.check_handle(source)?;
        let absent = values.heap().undefined()?;
        let value = values.heap().call(
            &self.extract_retry_hint,
            &absent,
            &[absent.clone(), source.clone()],
            values.runtime(),
        )?;
        match values.heap().snapshot(&value)? {
            ValueSnapshot::Undefined => Ok(None),
            ValueSnapshot::Number(value) => Ok(Some(value)),
            _ => Err(ValueError::TypeError("Fixed Native extractRetryHint must return Number or undefined".into())),
        }
    }
}
fn regex_capture(values: &SessionValues, source: &str, input: &ValueHandle) -> Result<Option<ValueHandle>, ValueError> {
    let regexp = values.heap().regexp(source.into(), "i")?;
    let matched = method(values.heap(), &regexp, "exec", &[input.clone()], values.runtime())?;
    if !values.heap().truthy(&matched)? {
        return Ok(None);
    }
    get(values.heap(), &matched, "1", values.runtime()).map(Some)
}
pub(in super::super) fn session_retry_hint_actual(
    message: &AgentMessage,
    now_ms: f64,
    values: &SessionValues,
    port: &dyn NativeRetryHintPort,
) -> Result<Option<f64>, ActualProviderError> {
    let root = current_root(message, values)?;
    guarded(&[(message.clone(), root.clone())], || {
        let heap = values.heap();
        let runtime = values.runtime();
        let error = get(heap, &root, "errorMessage", runtime)?;
        let mut error = if nullish(heap, &error)? { heap.string("".into())? } else { error };
        let evidence = get(heap, &root, "failureEvidence", runtime)?;
        if !nullish(heap, &evidence)? {
            let header = get(heap, &evidence, "headerWaitMs", runtime)?;
            if let ValueSnapshot::Number(wait) = heap.snapshot(&header)? {
                if wait.is_finite() && wait > 0.0 {
                    let marker = heap.string("retry-after-ms=".into())?;
                    let included = method(heap, &error, "includes", &[marker], runtime)?;
                    if !heap.truthy(&included)? {
                        let mut formatted = heap.to_string(&error, runtime)?;
                        formatted.append_str(" retry-after-ms=");
                        formatted.append_str(&ara_prompt::js::f64_to_string(wait));
                        error = heap.string(formatted)?;
                    }
                }
            }
        }
        if let Some(value) = regex_capture(values, r"retry-after-ms\s*[:=]\s*(\d+)", &error)? {
            return Ok(Some(heap.to_number(&value, runtime)?.max(0.0)));
        }
        if let Some(value) = regex_capture(values, r"retry-after\s*[:=]\s*([^\s,;]+)", &error)? {
            let seconds = heap.to_number(&value, runtime)?;
            if !seconds.is_nan() {
                return Ok(Some((seconds * 1000.0).max(0.0)));
            }
            let date = port.date_parse(&value, values)?;
            if !date.is_nan() {
                return Ok(Some((date - now_ms).max(0.0)));
            }
        }
        if let Some(wait) = port.extract_retry_hint(&error, now_ms, values)? {
            return Ok(Some(wait));
        }
        if let Some(value) = regex_capture(values, r"x-ratelimit-reset-ms\s*[:=]\s*(\d+)", &error)? {
            let reset = heap.to_number(&value, runtime)?;
            if !reset.is_nan() {
                return Ok(Some(if reset > 1_000_000_000_000.0 { (reset - now_ms).max(0.0) } else { reset.max(0.0) }));
            }
        }
        if let Some(value) = regex_capture(values, r"x-ratelimit-reset\s*[:=]\s*(\d+)", &error)? {
            let reset = heap.to_number(&value, runtime)?;
            if !reset.is_nan() {
                return Ok(Some(if reset > 1_000_000_000.0 {
                    (reset * 1000.0 - now_ms).max(0.0)
                } else {
                    (reset * 1000.0).max(0.0)
                }));
            }
        }
        Ok(None)
    })
}

#[cfg(test)]
pub(in super::super) fn observe_native_retry_family() {
    crate::rpc_host::observe_native_retry_wait_guard_family();
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest,
    };
    fn own(values: &SessionValues, root: &ValueHandle, key: &str, value: ValueHandle) {
        values.heap().define_own_data(root, key.into(), value, values.runtime()).unwrap();
    }
    fn owner(values: &SessionValues, content: Vec<ValueHandle>) -> AgentMessage {
        let root = values.heap().object().unwrap();
        for (key, value) in [
            ("role", "assistant"),
            ("provider", "openrouter"),
            ("model", "fixture"),
            ("api", "openai-responses"),
            ("stopReason", "error"),
            ("errorMessage", "server_error: stream closed with reason: error retry-after-ms=9000"),
        ] {
            own(values, &root, key, values.heap().string(value.into()).unwrap());
        }
        own(values, &root, "content", values.heap().array(content).unwrap());
        AgentMessage::from_graph(root, None).unwrap()
    }
    fn block(values: &SessionValues, kind: &str) -> ValueHandle {
        let root = values.heap().object().unwrap();
        own(values, &root, "type", values.heap().string(kind.into()).unwrap());
        root
    }
    fn result(
        values: &SessionValues,
        executed: ValueHandle,
        synthetic: bool,
        panicked: bool,
        name: &str,
    ) -> AgentMessage {
        let root = values.heap().object().unwrap();
        for (key, value) in [("role", "toolResult"), ("toolCallId", "call"), ("toolName", name)] {
            own(values, &root, key, values.heap().string(value.into()).unwrap());
        }
        let details = values.heap().object().unwrap();
        own(values, &details, "executed", executed);
        own(values, &details, "__synthetic", values.heap().boolean(synthetic).unwrap());
        own(values, &details, "panicked", values.heap().boolean(panicked).unwrap());
        own(values, &root, "details", details);
        AgentMessage::from_graph(root, None).unwrap()
    }
    struct Hints;
    impl NativeRetryHintPort for Hints {
        fn date_parse(&self, _: &ValueHandle, _: &SessionValues) -> Result<f64, ValueError> {
            panic!("numeric hint reached Date.parse")
        }
        fn extract_retry_hint(&self, _: &ValueHandle, _: f64, _: &SessionValues) -> Result<Option<f64>, ValueError> {
            Ok(None)
        }
    }
    let values = SessionValues::new(ValueHeap::new().unwrap(), Arc::new(NoRuntime)).unwrap();
    let thinking = block(&values, "thinking");
    own(&values, &thinking, "thinking", values.heap().string(WireString::from_units(vec![0xfeff, 0xd800])).unwrap());
    let bounded = owner(&values, vec![thinking]);
    assert_eq!(effective_max_retries_actual(&bounded, 10.0, &values).unwrap(), 1.0);
    assert_eq!(effective_max_retries_actual(&bounded, 0.25, &values).unwrap(), 0.25);
    assert_eq!(session_retry_hint_actual(&bounded, 0.0, &values, &Hints).unwrap(), Some(9000.0));
    assert_eq!(number_bits(-2_147_479_552.0), 0x8000_1000);
    assert_eq!(number_bits(4_294_967_296.0 + f64::from(flag::ABORT)), flag::ABORT);
    assert_eq!(number_bits(f64::INFINITY), 0);
    let values = Arc::new(values);
    let receipt = NativeRetryContextReceipt::capture(&[bounded.clone()], values.clone()).unwrap();
    receipt.recheck(&[bounded.clone()], &values).unwrap();
    let foreign_runtime = Arc::new(SessionValues::new(values.heap().clone(), Arc::new(NoRuntime)).unwrap());
    assert!(
        receipt.recheck(&[bounded.clone()], &foreign_runtime).is_err(),
        "same heap with another runtime cannot resume this saga"
    );
    struct Classifier {
        original: ValueHandle,
        error: ValueHandle,
        active: Model,
        direct: bool,
    }
    impl NativeRetryClassifierPort for Classifier {
        fn classify_message(&self, message: &ValueHandle, values: &SessionValues) -> Result<ValueHandle, ValueError> {
            assert_eq!(message.same_identity(&self.original), self.direct);
            assert!(ascii(values.heap(), &get(values.heap(), message, "api", values.runtime())?, &self.active.api)?);
            assert!(get(values.heap(), message, "errorMessage", values.runtime())?.same_identity(&self.error));
            if !self.direct {
                for key in ["provider", "model", "errorClassificationMessage"] {
                    assert!(matches!(
                        values.heap().snapshot(&get(values.heap(), message, key, values.runtime())?)?,
                        ValueSnapshot::Undefined
                    ));
                }
            }
            let id = values.heap().number(f64::from(flag::CLASS | flag::TRANSIENT))?;
            values.heap().assign(message, "errorId".into(), id.clone(), values.runtime())?;
            Ok(id)
        }
        fn is_context_overflow(&self, message: &ValueHandle, _: f64, _: &SessionValues) -> Result<bool, ValueError> {
            assert!(message.same_identity(&self.original));
            Ok(false)
        }
    }
    let AgentMessageBody::Graph(bounded_root) = bounded.body_snapshot() else { unreachable!() };
    let original_error = get(values.heap(), &bounded_root, "errorMessage", values.runtime()).unwrap();
    let native_model = Model {
        id: "active-native".into(),
        provider: "openrouter".into(),
        api: "openai-responses".into(),
        base_url: String::new(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    };
    let direct = Classifier {
        original: bounded_root.clone(),
        error: original_error.clone(),
        active: native_model.clone(),
        direct: true,
    };
    let metadata = same_route_class_actual(&native_model, &bounded, &values, &direct).unwrap().unwrap();
    assert!(metadata.class.retriable);
    assert!(metadata.error_message.same_identity(&original_error));
    let mut active = native_model.clone();
    active.api = "anthropic-messages".into();
    let override_api = Classifier {
        original: bounded_root.clone(),
        error: original_error.clone(),
        active: active.clone(),
        direct: false,
    };
    assert!(same_route_class_actual(&active, &bounded, &values, &override_api).unwrap().unwrap().class.retriable);
    // The imported classifier's original Set may mutate refusal metadata.
    // Observe that mutation only after classification and overflow, and skip
    // the original details Getter entirely for overflow. This verifies the
    // caller contract; the complete production classifier remains separate.
    struct RefusalRuntime {
        details: ValueHandle,
        events: Arc<std::sync::Mutex<Vec<&'static str>>>,
    }
    impl ValueRuntime for RefusalRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let RuntimeRequest::Call { capability, .. } = request else {
                return Err(ValueError::MissingCapability(request.capability()));
            };
            let heap = self.details.heap();
            match capability {
                CapabilityId(8002) => {
                    self.events.lock().unwrap().push("set");
                    heap.define_own_data(&self.details, "type".into(), heap.string("refusal".into())?, self)?;
                    Ok(RuntimeReply::Value(heap.undefined()?))
                }
                CapabilityId(8003) => {
                    self.events.lock().unwrap().push("details");
                    Ok(RuntimeReply::Value(self.details.clone()))
                }
                _ => Err(ValueError::MissingCapability(capability)),
            }
        }
    }
    struct RefusalClassifier {
        events: Arc<std::sync::Mutex<Vec<&'static str>>>,
        overflow: bool,
    }
    impl NativeRetryClassifierPort for RefusalClassifier {
        fn classify_message(&self, message: &ValueHandle, values: &SessionValues) -> Result<ValueHandle, ValueError> {
            self.events.lock().unwrap().push("classify");
            let id = values.heap().number(f64::from(flag::CLASS | flag::CONTENT_BLOCKED))?;
            values.heap().assign(message, "errorId".into(), id.clone(), values.runtime())?;
            Ok(id)
        }
        fn is_context_overflow(&self, _: &ValueHandle, _: f64, _: &SessionValues) -> Result<bool, ValueError> {
            self.events.lock().unwrap().push("overflow");
            Ok(self.overflow)
        }
    }
    let refusal_heap = ValueHeap::new().unwrap();
    let details = refusal_heap.object().unwrap();
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let refusal_values = SessionValues::new(
        refusal_heap.clone(),
        Arc::new(RefusalRuntime { details: details.clone(), events: events.clone() }),
    )
    .unwrap();
    own(&refusal_values, &details, "type", refusal_heap.string("other".into()).unwrap());
    let refusal_owner = owner(&refusal_values, vec![]);
    let AgentMessageBody::Graph(refusal_root) = refusal_owner.body_snapshot() else { unreachable!() };
    refusal_heap
        .define_own(
            &refusal_root,
            "errorId".into(),
            PropertyDescriptor::Accessor {
                get: None,
                set: Some(CallableTarget::Host(CapabilityId(8002))),
                enumerable: true,
                configurable: true,
            },
            refusal_values.runtime(),
        )
        .unwrap();
    refusal_heap
        .define_own(
            &refusal_root,
            "stopDetails".into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(8003))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            refusal_values.runtime(),
        )
        .unwrap();
    let classification = RefusalClassifier { events: events.clone(), overflow: false };
    assert!(
        same_route_class_actual(&native_model, &refusal_owner, &refusal_values, &classification)
            .unwrap()
            .unwrap()
            .class
            .retriable
    );
    assert_eq!(*events.lock().unwrap(), ["classify", "set", "overflow", "details"]);
    events.lock().unwrap().clear();
    let classification = RefusalClassifier { events: events.clone(), overflow: true };
    assert!(
        !same_route_class_actual(&native_model, &refusal_owner, &refusal_values, &classification)
            .unwrap()
            .unwrap()
            .class
            .retriable
    );
    assert_eq!(*events.lock().unwrap(), ["classify", "set", "overflow"]);
    let class = RetryClass { retriable: true, ..Default::default() };
    assert_eq!(
        disposition_actual(&bounded, &[bounded.clone()], &class, &values, true).unwrap(),
        Some(RetryDisposition::RemoveFailed)
    );
    assert!(
        disposition_actual(&bounded, &[bounded.clone(), bounded.clone()], &class, &values, true).unwrap().is_none(),
        "ambiguous canonical record occurrence"
    );
    let visible = block(&values, "text");
    own(&values, &visible, "text", values.heap().string("buffered".into()).unwrap());
    let buffered = owner(&values, vec![visible]);
    assert!(disposition_actual(&buffered, &[buffered.clone()], &class, &values, true).unwrap().is_none());
    assert_eq!(
        disposition_actual(&buffered, &[buffered.clone()], &class, &values, false).unwrap(),
        Some(RetryDisposition::RemoveFailed)
    );
    let call = block(&values, "toolCall");
    own(&values, &call, "id", values.heap().string("call".into()).unwrap());
    own(&values, &call, "name", values.heap().string("write".into()).unwrap());
    let failed = owner(&values, vec![call]);
    let unexecuted = result(&values, values.heap().boolean(false).unwrap(), true, false, "write");
    assert_eq!(
        disposition_actual(&failed, &[failed.clone(), unexecuted], &class, &values, true).unwrap(),
        Some(RetryDisposition::PreserveFailed)
    );
    for receipt in [
        result(&values, values.heap().string("unknown".into()).unwrap(), false, false, "write"),
        result(&values, values.heap().boolean(true).unwrap(), false, true, "write"),
        result(&values, values.heap().undefined().unwrap(), true, false, "write"),
        result(&values, values.heap().boolean(false).unwrap(), true, false, "other"),
    ] {
        assert!(
            replay_disposition_actual(
                &failed,
                &[failed.clone(), receipt],
                &RetryClass { interrupted_stream: true, ..class },
                &values,
                true
            )
            .unwrap()
            .is_none()
        );
    }
    let completed = result(&values, values.heap().boolean(true).unwrap(), false, false, "write");
    assert_eq!(
        replay_disposition_actual(
            &failed,
            &[failed.clone(), completed.clone()],
            &RetryClass { interrupted_stream: true, ..class },
            &values,
            true
        )
        .unwrap(),
        Some(RetryDisposition::PreserveFailed)
    );
    assert!(replay_disposition_actual(&failed, &[failed.clone(), completed], &class, &values, true).unwrap().is_none());
    let started = result(&values, values.heap().undefined().unwrap(), false, false, "write");
    let AgentMessageBody::Graph(started_root) = started.body_snapshot() else { unreachable!() };
    let details = get(values.heap(), &started_root, "details", values.runtime()).unwrap();
    own(&values, &details, "__interrupted", values.heap().boolean(true).unwrap());
    own(&values, &details, "execution", values.heap().string("started".into()).unwrap());
    assert!(
        replay_disposition_actual(
            &failed,
            &[failed.clone(), started],
            &RetryClass { interrupted_stream: true, ..class },
            &values,
            true
        )
        .unwrap()
        .is_none(),
        "entered execution has unknown effect until actual settlement"
    );
    let foreign = SessionValues::new(ValueHeap::new().unwrap(), Arc::new(NoRuntime)).unwrap();
    assert!(effective_max_retries_actual(&failed, 10.0, &foreign).is_err());
    struct Throws {
        thrown: ValueHandle,
    }
    impl ValueRuntime for Throws {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            if matches!(request, RuntimeRequest::Call { capability: CapabilityId(8001), .. }) {
                return Err(ValueError::Thrown(self.thrown.clone()));
            }
            Err(ValueError::MissingCapability(request.capability()))
        }
    }
    let heap = ValueHeap::new().unwrap();
    let thrown = heap.object().unwrap();
    let throwing = SessionValues::new(heap.clone(), Arc::new(Throws { thrown: thrown.clone() })).unwrap();
    let text = block(&throwing, "text");
    heap.define_own(
        &text,
        "text".into(),
        PropertyDescriptor::Accessor {
            get: Some(CallableTarget::Host(CapabilityId(8001))),
            set: None,
            enumerable: true,
            configurable: true,
        },
        &NoRuntime,
    )
    .unwrap();
    let owner = owner(&throwing, vec![text]);
    assert!(
        matches!(replay_disposition_actual(&owner, &[owner.clone()], &class, &throwing, true), Err(ActualProviderError::Value(ValueError::Thrown(value))) if value.same_identity(&thrown)),
        "original Native Throw survives decision/readmission"
    );
}
