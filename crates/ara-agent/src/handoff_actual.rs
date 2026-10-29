//! Original-domain handoff terminal consumer, fixed OMP596f2da
//! packages/agent/src/compaction/compaction.ts:1045-1122 (MIT; repository notices).
//! Request history and returned tool arguments stay in their original realm.
//! Only the existing summary/usage receipt crosses a checked scalar boundary.
use super::*;
use crate::provider_context::{ActualProviderError, PreparedActualSideRequest, SessionValues};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
};
use serde::de::DeserializeOwned;
use std::sync::Arc;

#[derive(Debug)]
pub enum ActualHandoffError {
    Summary(SummaryCallError),
    Native(ActualProviderError),
}
impl std::fmt::Display for ActualHandoffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Summary(error) => error.fmt(f),
            Self::Native(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for ActualHandoffError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::Summary(error) => error,
            Self::Native(error) => error,
        })
    }
}
impl From<SummaryCallError> for ActualHandoffError {
    fn from(error: SummaryCallError) -> Self {
        Self::Summary(error)
    }
}
impl From<ActualProviderError> for ActualHandoffError {
    fn from(error: ActualProviderError) -> Self {
        Self::Native(error)
    }
}
impl From<ValueError> for ActualHandoffError {
    fn from(error: ValueError) -> Self {
        Self::Native(error.into())
    }
}

fn get(values: &SessionValues, root: &ValueHandle, key: &str) -> Result<ValueHandle, ValueError> {
    values.heap().get(root, &key.into(), values.runtime())
}
fn nullish(values: &SessionValues, value: &ValueHandle) -> Result<bool, ValueError> {
    Ok(matches!(values.heap().snapshot(value)?, ValueSnapshot::Undefined | ValueSnapshot::Null))
}
fn scalar<T: DeserializeOwned>(values: &SessionValues, value: &ValueHandle) -> Result<T, ActualProviderError> {
    let value = values.heap().export_serde(value)?;
    serde_json::from_value(value).map_err(|error| ActualProviderError::Callback(error.to_string()))
}
fn optional<T: DeserializeOwned>(
    values: &SessionValues,
    root: &ValueHandle,
    key: &str,
) -> Result<Option<T>, ActualProviderError> {
    let value = get(values, root, key)?;
    if nullish(values, &value)? { Ok(None) } else { scalar(values, &value).map(Some) }
}
fn selected(
    values: &SessionValues,
    root: &ValueHandle,
    fields: &[&str],
) -> Result<serde_json::Value, ActualProviderError> {
    let mut result = serde_json::Map::new();
    for key in fields {
        let value = get(values, root, key)?;
        if !nullish(values, &value)? {
            result.insert((*key).into(), values.heap().export_serde(&value)?);
        }
    }
    Ok(serde_json::Value::Object(result))
}

/// This deliberately does not inspect content, arguments, providerPayload,
/// callbacks, extensions or the canonical body. Missing buckets stay unknown.
fn observe_receipt(
    values: &SessionValues,
    root: &ValueHandle,
    model: &Model,
) -> Result<AssistantMessage, ActualProviderError> {
    values.check_handle(root)?;
    let mut message = AssistantMessage::empty(&model.api, &model.provider, &model.id);
    message.stop_reason = scalar(values, &get(values, root, "stopReason")?)?;
    message.response_id = optional(values, root, "responseId")?;
    message.error_message = optional(values, root, "errorMessage")?;
    message.error_status = optional(values, root, "errorStatus")?;
    message.duration = optional(values, root, "duration")?;
    message.ttft = optional(values, root, "ttft")?;
    message.terminal_context_recovery = optional(values, root, "terminalContextRecovery")?;
    let usage = get(values, root, "usage")?;
    if !nullish(values, &usage)? {
        let mut observed = selected(
            values,
            &usage,
            &["input", "output", "cacheRead", "cacheWrite", "totalTokens", "reasoningTokens"],
        )?;
        let cost = get(values, &usage, "cost")?;
        if !nullish(values, &cost)? {
            observed["cost"] = selected(values, &cost, &["input", "output", "cacheRead", "cacheWrite", "total"])?;
        }
        message.usage =
            serde_json::from_value(observed).map_err(|error| ActualProviderError::Callback(error.to_string()))?;
    }
    let evidence = get(values, root, "failureEvidence")?;
    if !nullish(values, &evidence)? {
        message.failure_evidence = Some(
            serde_json::from_value(selected(
                values,
                &evidence,
                &[
                    "kind",
                    "status",
                    "code",
                    "replayBlocked",
                    "sameRouteBlocked",
                    "contextRecovery",
                    "waitMs",
                    "headerWaitMs",
                    "errorId",
                ],
            )?)
            .map_err(|error| ActualProviderError::Callback(error.to_string()))?,
        );
    }
    Ok(message)
}

fn text_predicate(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let block = arguments.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())?;
    let kind = heap.get(&block, &"type".into(), runtime)?;
    heap.boolean(matches!(heap.snapshot(&kind)?, ValueSnapshot::String(kind) if kind.equals_ascii("text")))
}
fn text_member(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let block = arguments.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())?;
    heap.get(&block, &"text".into(), runtime)
}
static TEXT_PREDICATE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp596f2da.compaction.handoff.text-filter",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: text_predicate,
};
static TEXT_MEMBER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp596f2da.compaction.handoff.text-map",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: text_member,
};
fn extract_text(values: &SessionValues, root: &ValueHandle) -> Result<String, ActualProviderError> {
    let heap = values.heap();
    let content = get(values, root, "content")?;
    // Original filter -> map -> join reads/calls, including overrides, holes,
    // callback Gets and native UTF16 join coercions. No returned tool is run.
    let filter = get(values, &content, "filter")?;
    let predicate = heap.fresh_compiled_function(&TEXT_PREDICATE, &[])?;
    let filtered = heap.call(&filter, &content, &[predicate], values.runtime())?;
    let map = get(values, &filtered, "map")?;
    let member = heap.fresh_compiled_function(&TEXT_MEMBER, &[])?;
    let mapped = heap.call(&map, &filtered, &[member], values.runtime())?;
    let join = get(values, &mapped, "join")?;
    let text = heap.call(&join, &mapped, &[heap.string("\n".into())?], values.runtime())?;
    scalar(values, &text)
}

type Terminal = (StopReason, ValueHandle, AssistantMessage);
fn terminal(
    values: &SessionValues,
    event: ara_ai::ActualAssistantEvent,
    model: &Model,
) -> Result<Option<Terminal>, ActualProviderError> {
    match event {
        ara_ai::ActualAssistantEvent::Done { reason, message: root }
        | ara_ai::ActualAssistantEvent::Error { reason, error: root } => {
            let observation = observe_receipt(values, &root, model)?;
            Ok(Some((reason, root, observation)))
        }
        _ => Ok(None),
    }
}

#[allow(clippy::too_many_arguments)]
async fn attempt_actual(
    context: &PreparedActualSideRequest,
    model: &Model,
    provider: &dyn ModelProvider,
    tool_choice: ToolChoice,
    max_tokens: Option<u64>,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<Terminal, ActualHandoffError> {
    if cancel.is_cancelled() {
        return Err(failed(SummaryCallErrorKind::Cancelled, None, &model.api).into());
    }
    if Instant::now() >= deadline {
        return Err(failed(SummaryCallErrorKind::Deadline, None, &model.api).into());
    }
    let provider_cancel = cancel.child_token();
    let _guard = provider_cancel.clone().drop_guard();
    let mut events = context.stream(
        provider,
        model,
        CallOptions {
            cancel: provider_cancel.clone(),
            max_in_flight_requests: None,
            wait_for_terminal_drain: false,
            tool_choice: Some(tool_choice),
            max_tokens,
            suppress_native_tool_choice: false,
            temperature: None,
            loop_guard: None,
            on_response: None,
            reasoning: None,
            include: None,
            responses_lite: None,
        },
    )?;
    let timeout = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
    tokio::pin!(timeout);
    loop {
        let (event, interruption) = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                provider_cancel.cancel();
                (events.take_terminal_on_close().await, Some(SummaryCallErrorKind::Cancelled))
            }
            _ = &mut timeout => {
                provider_cancel.cancel();
                (events.take_terminal_on_close().await, Some(SummaryCallErrorKind::Deadline))
            }
            event = events.recv() => (event, None),
        };
        if let Some(error) = events.integration_failures().first() {
            return Err(ActualProviderError::Value((**error).clone()).into());
        }
        let eof = event.is_none();
        let observed = event.map(|event| terminal(&context.values, event, model)).transpose()?.flatten();
        if let Some(kind) = interruption {
            let terminal = observed.as_ref().map(|(reason, _, response)| (*reason, response));
            let mut failure = failed(kind, terminal, &model.api);
            failure.invocations.push(invocation(Some(kind), terminal, &model.api, false));
            return Err(failure.into());
        }
        if let Some(observed) = observed {
            let interruption = if cancel.is_cancelled() {
                Some(SummaryCallErrorKind::Cancelled)
            } else if Instant::now() >= deadline {
                Some(SummaryCallErrorKind::Deadline)
            } else {
                None
            };
            if let Some(kind) = interruption {
                let receipt = Some((observed.0, &observed.2));
                let mut failure = failed(kind, receipt, &model.api);
                failure.invocations.push(invocation(Some(kind), receipt, &model.api, false));
                return Err(failure.into());
            }
            return Ok(observed);
        }
        if eof {
            let mut failure = failed(SummaryCallErrorKind::StreamEndedWithoutTerminal, None, &model.api);
            failure.invocations.push(invocation(Some(failure.kind), None, &model.api, false));
            return Err(failure.into());
        }
    }
}

/// Fixed cache-first None plus at most one Auto-only retry. The same original
/// Context is reused; the result is text and receipts, never executable tools.
pub async fn generate_handoff_from_actual_context(
    context: &PreparedActualSideRequest,
    model: &Model,
    provider: &dyn ModelProvider,
    max_tokens: Option<u64>,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<AcceptedSummary, ActualHandoffError> {
    let mut invocations = Vec::new();
    for index in 0..2 {
        let choice = if index == 0 { ToolChoice::None } else { ToolChoice::Auto };
        let (reason, root, response) =
            match attempt_actual(context, model, provider, choice, max_tokens, deadline, cancel).await {
                Ok(terminal) => terminal,
                Err(ActualHandoffError::Summary(mut failure)) => {
                    invocations.append(&mut failure.invocations);
                    failure.invocations = invocations;
                    return Err(failure.into());
                }
                Err(error) => return Err(error),
            };
        let kind = if reason == StopReason::Aborted || response.stop_reason == StopReason::Aborted {
            Some(SummaryCallErrorKind::Cancelled)
        } else if reason == StopReason::Error || response.stop_reason == StopReason::Error {
            Some(SummaryCallErrorKind::ProviderError)
        } else {
            None
        };
        let retry = index == 0 && response.stop_reason == StopReason::Error && should_retry_auto(&response);
        invocations.push(invocation(kind, Some((reason, &response)), &model.api, retry));
        if let Some(kind) = kind {
            if retry && kind == SummaryCallErrorKind::ProviderError {
                continue;
            }
            let mut failure = failed(kind, Some((reason, &response)), &model.api);
            failure.invocations = invocations;
            return Err(failure.into());
        }
        let text = extract_text(&context.values, &root)?;
        let interruption = if cancel.is_cancelled() {
            Some(SummaryCallErrorKind::Cancelled)
        } else if Instant::now() >= deadline {
            Some(SummaryCallErrorKind::Deadline)
        } else {
            None
        };
        if let Some(kind) = interruption {
            invocations.last_mut().expect("actual handoff invocation").error_kind = Some(kind);
            let mut failure = failed(kind, Some((reason, &response)), &model.api);
            failure.invocations = invocations;
            return Err(failure.into());
        }
        return Ok(AcceptedSummary {
            text,
            terminal_reason: reason,
            window_source_entry_ids: Vec::new(),
            model_id: model.id.clone(),
            response_id: response.response_id,
            usage: response.usage,
            duration_ms: response.duration,
            ttft_ms: response.ttft,
            invocations,
        });
    }
    unreachable!("handoff permits at most one auto-only retry")
}
