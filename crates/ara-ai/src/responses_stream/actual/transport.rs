//! Actual HTTP/SSE driver. Reuses the existing bounded HTTP retry owner.
//! Raw SSE JSON allocates the original Graph; whole output/context is never
//! converted to serde or a typed AssistantMessage. SOURCE candidate only.
use super::{GraphResponsesError, GraphResponsesFlavor, GraphResponsesOptions, GraphResponsesStreamState};
use crate::actual_provider::GraphBuilder;
use crate::event::ActualStreamFailureReceipt;
use crate::graph_values::GraphValueFactory;
use crate::providers::openai_completions::{PostRetryPolicy, PostRetryState, post_bytes_with_retry_policy};
use crate::providers::openai_responses::StreamOptions;
use crate::{ActualAssistantStream, AssistantMessageEvent, Model};
use ara_rpc::value_heap::{ValueError, ValueHandle};
use futures::StreamExt;
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Clone)]
pub struct GraphResponsesPreparedRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: ValueHandle,
}
pub fn stream_prepared(
    client: reqwest::Client,
    model: Model,
    request: GraphResponsesPreparedRequest,
    factory: Arc<GraphValueFactory>,
    options: StreamOptions,
    flavor: GraphResponsesFlavor,
    graph_options: GraphResponsesOptions,
) -> Result<ActualAssistantStream, ValueError> {
    factory.heap().adopt(&request.body)?;
    // Replays stringify the original request root at the original request
    // boundary again. A prepared Graph is never a typed transcript bridge.
    crate::replay_safe_retry::actual::with_replay_safe_actual_stream_retry(
        model.clone(),
        factory.clone(),
        options.cancel.clone(),
        flavor == GraphResponsesFlavor::Codex,
        move |cancel| {
            let mut options = options.clone();
            options.cancel = cancel;
            attempt(
                client.clone(),
                model.clone(),
                Ok(request.clone()),
                factory.clone(),
                options,
                flavor,
                graph_options.clone(),
            )
        },
    )
}
type RequestBuilder = Arc<dyn Fn() -> Result<GraphResponsesPreparedRequest, ValueError> + Send + Sync>;
pub(crate) fn stream_with_request_builder(
    client: reqwest::Client,
    model: Model,
    build: RequestBuilder,
    factory: Arc<GraphValueFactory>,
    options: StreamOptions,
    flavor: GraphResponsesFlavor,
    graph_options: GraphResponsesOptions,
) -> Result<ActualAssistantStream, ValueError> {
    crate::replay_safe_retry::actual::with_replay_safe_actual_stream_retry(
        model.clone(),
        factory.clone(),
        options.cancel.clone(),
        flavor == GraphResponsesFlavor::Codex,
        move |cancel| {
            let mut options = options.clone();
            options.cancel = cancel;
            let request = build();
            attempt(client.clone(), model.clone(), request, factory.clone(), options, flavor, graph_options.clone())
        },
    )
}
fn attempt(
    client: reqwest::Client,
    model: Model,
    request: Result<GraphResponsesPreparedRequest, ValueError>,
    factory: Arc<GraphValueFactory>,
    options: StreamOptions,
    flavor: GraphResponsesFlavor,
    graph_options: GraphResponsesOptions,
) -> crate::replay_safe_retry::actual::ActualAttemptStream {
    let (sink, events, failures) = ActualAssistantStream::channel();
    let error = Arc::new(Mutex::new(None));
    let recorded = error.clone();
    // Projection has already run. Preserve its original abrupt completion
    // synchronously, before spawn, state initialization or cancellation can
    // select another public settlement path. The later branch only chooses
    // the public outcome and must not record this exception a second time.
    let request = match request {
        Ok(request) => Some(request),
        Err(original) => {
            failures.record(original);
            None
        }
    };
    tokio::spawn(async move {
        let _settlement = failures.settlement_guard();
        let started = Instant::now();
        let history_hash = graph_options.history_id_hash.clone();
        let mut state = match GraphResponsesStreamState::new(&model, factory.clone(), flavor, graph_options) {
            Ok(state) => state,
            Err(error) => {
                failures.record(error);
                return;
            }
        };
        let request_failed = request.is_none();
        let mut retry_blocked = request_failed;
        let mut failure_evidence = None;
        let mut native_chain = None;
        let mut result = if options.cancel.is_cancelled() {
            Err(crate::ProviderError::Aborted)
        } else {
            match request {
                Some(request) => {
                    run(
                        &client,
                        &model,
                        &request,
                        &options,
                        flavor,
                        &mut state,
                        &sink,
                        &failures,
                        &mut retry_blocked,
                        &mut failure_evidence,
                        &mut native_chain,
                    )
                    .await
                }
                None => Err(crate::ProviderError::Config(
                    "Actual Responses Context projection failed; original exception is retained".into(),
                )),
            }
        };
        if request_failed {
            state.veto_context_recovery(crate::ContextRecoveryEvidence::NativeValidation);
        }
        if result.is_ok() && flavor == GraphResponsesFlavor::Compatible {
            if let Some(session) = &options.session_state {
                let completed = if let Some(chain) = &mut native_chain {
                    chain.complete(&model, &state.root, &state.native_items, factory.clone(), history_hash)
                } else {
                    crate::providers::openai_responses::actual::warm_if_replayable(
                        session,
                        &model,
                        &state.native_items,
                        factory.clone(),
                        history_hash,
                    )
                };
                if let Err(error) = completed {
                    retry_blocked = true;
                    state.veto_context_recovery(crate::ContextRecoveryEvidence::NativeValidation);
                    failures.record(error);
                    result = Err(crate::ProviderError::Config(
                        "Responses native replay warmup failed; original exception is retained".into(),
                    ));
                }
            }
        }
        // Earlier chain/projection failures already retain their original
        // ValueError receipts. They cannot certify content-only recovery.
        if !failures.failures().is_empty() {
            state.veto_context_recovery(crate::ContextRecoveryEvidence::NativeValidation);
            retry_blocked = true;
        }
        let mut terminal_can_publish = result.is_ok();
        let builder = GraphBuilder(factory);
        if let Err(cause) = &result {
            let mut evidence = failure_evidence
                .take()
                .unwrap_or_else(|| crate::retry_classification::ProviderFailureEvidence::from_error(cause, false));
            evidence.context_recovery = match evidence.context_recovery {
                Some(previous) => Some(previous.merge(state.context_recovery_evidence)),
                None if evidence.same_route_blocked
                    && state.context_recovery_evidence == crate::ContextRecoveryEvidence::ContentOnly =>
                {
                    (evidence.kind == crate::retry_classification::ProviderErrorKind::Http)
                        .then_some(crate::ContextRecoveryEvidence::UsageAdmission)
                }
                None => Some(state.context_recovery_evidence),
            };
            failure_evidence = Some(crate::retry_classification::finalize_failure_facts(
                cause.stop_reason(),
                Some(&cause.to_string()),
                cause.status(),
                Some(evidence),
                cause,
                retry_blocked || state.replay_unsafe,
                state.context_recovery_evidence == crate::ContextRecoveryEvidence::NativeOutput,
                &model.api,
            ));
        }
        if let Err(error) = builder
            .derived(&state.context_recovery_evidence)
            .and_then(|value| builder.put(&state.root, "terminalContextRecovery", value))
        {
            terminal_can_publish = false;
            retry_blocked = true;
            failures.record(error);
        }
        if let Some(evidence) = &failure_evidence {
            match builder.derived(evidence).and_then(|value| builder.put(&state.root, "failureEvidence", value)) {
                Ok(()) => {}
                Err(error) => {
                    terminal_can_publish = false;
                    retry_blocked = true;
                    failures.record(error);
                }
            }
        }
        if let Err(cause) = &result {
            *recorded.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(crate::replay_safe_retry::AttemptError {
                    cause: cause.clone(),
                    retry_blocked: retry_blocked || state.replay_unsafe,
                });
        }
        let terminal = (|| -> Result<crate::ActualAssistantEvent, ValueError> {
            builder.put(&state.root, "duration", builder.0.heap().number(started.elapsed().as_millis() as f64)?)?;
            match result {
                Ok(()) => Ok(AssistantMessageEvent::Done { reason: state.stop, message: state.root.clone() }),
                Err(cause) => {
                    let reason = cause.stop_reason();
                    builder.put(&state.root, "stopReason", builder.text(reason.as_str())?)?;
                    builder.put(&state.root, "errorMessage", builder.text(cause.to_string())?)?;
                    if let Some(status) = cause.status() {
                        builder.put(&state.root, "errorStatus", builder.0.heap().number(status as f64)?)?;
                    }
                    // Fixed provider catch publishes its finalized ID before
                    // Error; keep the live root and ordinary assignment.
                    if let Some(evidence) = &failure_evidence {
                        builder.put(&state.root, "errorId", builder.0.heap().number(f64::from(evidence.error_id))?)?;
                    }
                    Ok(AssistantMessageEvent::Error { reason, error: state.root.clone() })
                }
            }
        })();
        match terminal {
            Ok(event) => {
                if terminal_can_publish && let Some(chain) = &mut native_chain {
                    // Terminal Graph construction has succeeded. Publish before
                    // waking the receiver so a following turn can see the
                    // baseline; revoke this exact generation if delivery fails.
                    if options.cancel.is_cancelled() {
                        chain.fail_completion();
                    } else {
                        chain.publish();
                    }
                    let delivered = sink.push_or_cancel(event, &options.cancel).await;
                    if !delivered {
                        chain.fail_completion();
                    }
                } else {
                    let _ = sink.push(event).await;
                }
            }
            Err(error) => failures.record(error),
        }
    });
    crate::replay_safe_retry::actual::ActualAttemptStream { events, error }
}
#[allow(clippy::too_many_arguments)]
async fn run(
    client: &reqwest::Client,
    model: &Model,
    request: &GraphResponsesPreparedRequest,
    options: &StreamOptions,
    flavor: GraphResponsesFlavor,
    state: &mut GraphResponsesStreamState,
    sink: &crate::event::EventSink<crate::ActualAssistantEvent>,
    failures: &ActualStreamFailureReceipt,
    retry_blocked: &mut bool,
    failure_evidence: &mut Option<crate::retry_classification::ProviderFailureEvidence>,
    completed_chain: &mut Option<crate::providers::openai_responses::actual::chain::NativeChainRequest>,
) -> Result<(), crate::error::ProviderError> {
    use crate::error::ProviderError;
    if options.cancel.is_cancelled() {
        return Err(ProviderError::Aborted);
    }
    if options.stateful_responses && flavor == GraphResponsesFlavor::Codex {
        return Err(ProviderError::Config("Actual Responses stateful chain owner is not bound".into()));
    }
    let builder = GraphBuilder(state.factory().clone());
    let retain_value_error = |error| {
        failures.record(error);
        ProviderError::Config("Actual Responses chain observation failed; original exception is retained".into())
    };
    let mut chain = if options.stateful_responses {
        Some(
            crate::providers::openai_responses::actual::chain::NativeChainRequest::begin(
                &builder,
                model,
                options,
                &request.body,
            )
            .map_err(|error| {
                *retry_blocked = true;
                retain_value_error(error)
            })?,
        )
    } else {
        None
    };
    let body = chain.as_ref().map_or(&request.body, |chain| chain.body());
    let bytes = builder
        .plain_json(body)
        .and_then(|wire| {
            wire.to_utf8()
                .map(|text| text.into_bytes())
                .map_err(|_| ValueError::UnsupportedOperation("JSON.stringify wire request must contain scalar UTF-8"))
        })
        .map_err(|error| {
            *retry_blocked = true;
            failures.record(error);
            ProviderError::Config("Actual Responses request projection failed; original exception is retained".into())
        })?;
    let first = options.first_event_timeout.map(|duration| Instant::now() + duration);
    let retry = PostRetryPolicy::for_responses(&options.retry, flavor == GraphResponsesFlavor::Codex);
    let mut retry_state = PostRetryState { retry_blocked, failure_evidence };
    let mut posted = match first {
        Some(deadline) => tokio::select! {
            result=post_bytes_with_retry_policy(client,&request.url,&request.headers,&bytes,&retry,&options.cancel,&mut retry_state)=>result,
            _=tokio::time::sleep_until(deadline.into())=>Err(ProviderError::Timeout("Responses stream timed out before its first event".into()).into()),
        },
        None => {
            post_bytes_with_retry_policy(
                client,
                &request.url,
                &request.headers,
                &bytes,
                &retry,
                &options.cancel,
                &mut retry_state,
            )
            .await
        }
    };
    if chain.as_ref().is_some_and(|chain| chain.sent_previous())
        && !options.cancel.is_cancelled()
        && let Some(zero_data_retention) =
            posted.as_ref().err().and_then(crate::providers::openai_responses::stale_previous_response)
    {
        let chain = chain.as_mut().expect("stateful previous request");
        chain.stale_rejection(&builder, zero_data_retention).map_err(|error| {
            *retry_state.retry_blocked = true;
            retain_value_error(error)
        })?;
        let bytes = builder
            .plain_json(chain.body())
            .and_then(|wire| {
                wire.to_utf8().map(|text| text.into_bytes()).map_err(|_| {
                    ValueError::UnsupportedOperation("JSON.stringify wire request must contain scalar UTF-8")
                })
            })
            .map_err(|error| {
                *retry_state.retry_blocked = true;
                retain_value_error(error)
            })?;
        // Only a classified pre-Start rejection replays once, retaining the
        // original headers deadline and existing HTTP retry/auth policy.
        posted = match first {
            Some(deadline) => tokio::select! {
                result = post_bytes_with_retry_policy(client, &request.url, &request.headers, &bytes, &retry, &options.cancel, &mut retry_state) => result,
                _ = tokio::time::sleep_until(deadline.into()) => Err(ProviderError::Timeout("Responses stream timed out before its first event".into()).into()),
            },
            None => {
                post_bytes_with_retry_policy(
                    client,
                    &request.url,
                    &request.headers,
                    &bytes,
                    &retry,
                    &options.cancel,
                    &mut retry_state,
                )
                .await
            }
        };
    }
    let response = posted.map_err(|error| {
        if let Some(evidence) = error.failure_evidence {
            *retry_state.failure_evidence = Some(evidence);
        }
        error.cause
    })?;
    if flavor == GraphResponsesFlavor::Compatible {
        if let Err(error) =
            crate::provider_response::notify_provider_response(options.on_response.as_ref(), &response, model).await
        {
            *retry_state.retry_blocked = true;
            return Err(error);
        }
    }
    if !sink.push_or_cancel(AssistantMessageEvent::Start { partial: state.root.clone() }, &options.cancel).await {
        return Err(ProviderError::Aborted);
    }
    let first = if flavor == GraphResponsesFlavor::Compatible {
        options.first_event_timeout.map(|duration| Instant::now() + duration)
    } else {
        first
    };
    let mut chunks = response.bytes_stream();
    let mut decoder = crate::responses_sse::ResponsesSseDecoder::new();
    let mut progressed = false;
    let mut last = Instant::now();
    loop {
        let deadline = if progressed { options.idle_timeout.map(|duration| last + duration) } else { first };
        let next = match deadline {
            Some(deadline) => tokio::select! {
                next=chunks.next()=>next,
                _=options.cancel.cancelled()=>return Err(ProviderError::Aborted),
                _=tokio::time::sleep_until(deadline.into())=>return Err(ProviderError::Timeout("Responses stream stalled".into())),
            },
            None => {
                tokio::select! {next=chunks.next()=>next,_=options.cancel.cancelled()=>return Err(ProviderError::Aborted)}
            }
        };
        let ended = next.is_none();
        let frames = match next {
            Some(Ok(bytes)) => decoder.feed(&bytes)?,
            Some(Err(error)) => return Err(ProviderError::Transport(format!("Responses stream read failed: {error}"))),
            None => decoder.finish()?.into_iter().collect(),
        };
        for frame in frames {
            if frame.data == "[DONE]" {
                return Err(ProviderError::Incomplete(
                    "OpenAI responses stream closed before a terminal response event was received".into(),
                ));
            }
            let raw = match state.parse_frame(&frame.data) {
                Ok(raw) => raw,
                Err(error) => {
                    *retry_state.retry_blocked = true;
                    state.veto_context_recovery(crate::ContextRecoveryEvidence::NativeValidation);
                    failures.record(error);
                    return Err(ProviderError::Stream("Malformed Responses SSE JSON frame".into()));
                }
            };
            match builder.get(&raw, "type").and_then(|kind| builder.0.heap().snapshot(&kind)) {
                Ok(ara_rpc::value_heap::ValueSnapshot::String(kind)) => {
                    if [
                        "response.created",
                        "response.output_item.added",
                        "response.reasoning_summary_part.added",
                        "response.reasoning_summary_text.delta",
                        "response.reasoning_summary_text.done",
                        "response.reasoning_summary_part.done",
                        "response.reasoning_text.delta",
                        "response.content_part.added",
                        "response.output_text.delta",
                        "response.refusal.delta",
                        "response.function_call_arguments.delta",
                        "response.function_call_arguments.done",
                        "response.custom_tool_call_input.delta",
                        "response.custom_tool_call_input.done",
                        "response.output_item.done",
                        "response.completed",
                        "response.incomplete",
                        "response.failed",
                        "error",
                    ]
                    .iter()
                    .any(|expected| kind.equals_ascii(expected))
                    {
                        progressed = true;
                        last = Instant::now();
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    *retry_state.retry_blocked = true;
                    state.veto_context_recovery(crate::ContextRecoveryEvidence::NativeValidation);
                    failures.record(error);
                    return Err(ProviderError::Stream("Responses progress event observation failed".into()));
                }
            }
            let mut updates = Vec::new();
            let result = state.handle(&raw, &mut updates);
            // Publish the original abrupt-completion receipt before the first
            // await. Cancel/closed during a preceding update flush cannot erase
            // the exception or admit replay of effects already observed.
            let result = match result {
                Err(GraphResponsesError::Value(error)) => {
                    *retry_state.retry_blocked = true;
                    state.replay_unsafe = true;
                    state.veto_context_recovery(crate::ContextRecoveryEvidence::NativeValidation);
                    failures.record(error);
                    Err(ProviderError::Config(
                        "Actual Responses integration failed; original exception is retained".into(),
                    ))
                }
                Err(GraphResponsesError::Provider(error)) => match state.failure_evidence(&error, &raw) {
                    Ok(mut evidence) => {
                        if let Some(previous) = retry_state.failure_evidence.take() {
                            evidence.status = evidence.status.or(previous.status);
                            evidence.code = evidence.code.or(previous.code);
                            evidence.wait_ms = previous.wait_ms;
                            evidence.header_wait_ms = previous.header_wait_ms;
                            evidence.error_id |= previous.error_id;
                            evidence.replay_blocked |= previous.replay_blocked;
                            evidence.same_route_blocked |= previous.same_route_blocked;
                            evidence.context_recovery = previous
                                .context_recovery
                                .map(|previous| evidence.context_recovery.map_or(previous, |next| previous.merge(next)))
                                .or(evidence.context_recovery);
                        }
                        *retry_state.failure_evidence = Some(evidence);
                        Err(error)
                    }
                    Err(original) => {
                        *retry_state.retry_blocked = true;
                        state.replay_unsafe = true;
                        state.veto_context_recovery(crate::ContextRecoveryEvidence::NativeValidation);
                        failures.record(original);
                        Err(ProviderError::Config(
                            "Actual Responses failure evidence observation failed; original exception is retained"
                                .into(),
                        ))
                    }
                },
                Ok(()) => Ok(()),
            };
            // Original block updates preceding an abrupt native completion
            // remain visible; do not discard buffered events at the error.
            for update in updates {
                progressed = true;
                last = Instant::now();
                if !sink.push_or_cancel(update, &options.cancel).await {
                    return Err(ProviderError::Aborted);
                }
            }
            match result {
                Ok(()) => {}
                Err(error) => return Err(error),
            }
            if state.terminal {
                *completed_chain = chain;
                return Ok(());
            }
        }
        if ended {
            break;
        }
    }
    Err(ProviderError::Incomplete("OpenAI responses stream closed before a terminal response event was received".into()))
}
