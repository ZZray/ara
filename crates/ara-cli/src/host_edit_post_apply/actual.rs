//! Actual Edit completion on the issuing NativeRun, fixed OMP596f2da (MIT).
//! Existing provider/auth/guard ownership stays behind ModelProvider. Only the
//! source caller's outer oneshot and inner thinking-loop policy live here.
use super::*;
use ara_ai::{
    AssistantMessageEvent, Cost, GraphProviderContext, Usage,
    retry::{
        RetryBindings,
        oneshot::{CompletionAttempt, retry_transient_completion},
    },
};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueHeap, ValueKind,
    ValueRuntime, ValueSnapshot,
};
use ara_tools::edit_post_apply::EditNativeRepairInvocation;
type Result<T> = std::result::Result<T, ValueError>;
fn native<T>(
    invocation: &EditNativeRepairInvocation,
    operation: impl FnOnce(&dyn ValueRuntime) -> Result<T>,
) -> Result<T> {
    let mut operation = Some(operation);
    let mut result = None;
    invocation
        .bindings
        .with_runtime(&invocation.factory, &mut |runtime| {
            let operation = operation.take().ok_or(ValueError::UnsupportedOperation(
                "Edit native borrowing callback repeated",
            ))?;
            result = Some(operation(runtime)?);
            invocation.factory.heap().undefined()
        })?;
    result.ok_or(ValueError::UnsupportedOperation(
        "Edit native borrowing callback was not called",
    ))
}
struct Attempt {
    invocation: EditNativeRepairInvocation,
    provider: Arc<dyn ModelProvider>,
    model: Model,
    prompt: String,
    usage: UsageCallback,
    cancel: CancellationToken,
}
impl CompletionAttempt for Attempt {
    fn run(&self, _: ValueHandle) -> BoxFuture<'static, Result<ValueHandle>> {
        let invocation = self.invocation.clone();
        let provider = self.provider.clone();
        let model = self.model.clone();
        let prompt = self.prompt.clone();
        let usage = self.usage.clone();
        let cancel = self.cancel.clone();
        Box::pin(async move {
            // Source run() rebuilds Context/options on each outer attempt.
            // Its inner completeSimple retries dispatch with these same roots.
            let context = native(&invocation, |runtime| {
                let heap = invocation.factory.heap();
                let user = heap.object()?;
                heap.define_own_data(&user, "role".into(), heap.string("user".into())?, runtime)?;
                heap.define_own_data(
                    &user,
                    "content".into(),
                    heap.string(prompt.into())?,
                    runtime,
                )?;
                let date = heap.get(&invocation.bindings.globals(), &"Date".into(), runtime)?;
                let now = heap.get(&date, &"now".into(), runtime)?;
                let timestamp = heap.call(&now, &date, &[], runtime)?;
                heap.define_own_data(&user, "timestamp".into(), timestamp, runtime)?;
                let context = heap.object()?;
                heap.define_own_data(
                    &context,
                    "messages".into(),
                    heap.array(vec![user.clone()])?,
                    runtime,
                )?;
                Ok(GraphProviderContext {
                    original_context: Some(context),
                    system_prompt: Vec::new(),
                    messages: vec![user],
                    tools: None,
                    history_demotion_dialect: None,
                    history_id_hash: None,
                    history_id_hash_function: None,
                })
            })?;
            let options = CallOptions {
                cancel: cancel.clone(),
                max_tokens: if model.api == "openai-codex-responses" {
                    None
                } else {
                    Some(8192)
                },
                reasoning: Some(ara_ai::reasoning::ReasoningRequest {
                    disable_reasoning: true,
                    ..Default::default()
                }),
                ..Default::default()
            };
            let mut message =
                dispatch(&provider, &model, &context, &invocation, &options, &usage).await?;
            let mut retry = thinking_loop(&invocation, &message)?;
            for attempt in 1..3 {
                if !retry {
                    return Ok(message);
                }
                throw_if_aborted(&invocation)?;
                wait(&invocation, (500_u64 << (attempt - 1)) as f64).await?;
                message =
                    dispatch(&provider, &model, &context, &invocation, &options, &usage).await?;
                retry = thinking_loop(&invocation, &message)?;
            }
            if retry {
                throw_if_aborted(&invocation)?;
            }
            Ok(message)
        })
    }
}
async fn dispatch(
    provider: &Arc<dyn ModelProvider>,
    model: &Model,
    context: &GraphProviderContext,
    invocation: &EditNativeRepairInvocation,
    options: &CallOptions,
    usage: &UsageCallback,
) -> Result<ValueHandle> {
    invocation.bindings.check_invocation(&invocation.factory)?;
    context.check_lineage(&invocation.factory)?;
    let key = invocation.resources.begin_attempt(options.cancel.clone())?;
    let mut stream =
        match provider.stream_actual(model, context, invocation.factory.clone(), options.clone()) {
            Ok(stream) => stream,
            Err(error) => {
                // No arbitrary factory error establishes that an external request
                // did not start. Close registration with an explicit unknown.
                invocation.resources.dispatch_uncertain(key)?;
                return Err(error);
            }
        };
    let receipt = stream.failure_receipt();
    invocation
        .resources
        .register_started(key, receipt.clone())?;
    let mut root = None;
    while let Some(event) = stream.recv().await {
        match event {
            AssistantMessageEvent::Done { message, .. } => {
                root = Some(message);
                break;
            }
            AssistantMessageEvent::Error { error, .. } => {
                // Source AssistantMessageEventStream.push classifies an error
                // before result()/completeSimple observes this original root.
                native(invocation, |runtime| {
                    let heap = invocation.factory.heap();
                    if heap.object_is(
                        &heap.get(&error, &"stopReason".into(), runtime)?,
                        &heap.string("error".into())?,
                    )? {
                        let namespace = &invocation.module.ai_error;
                        let classify = heap.get(namespace, &"classifyMessage".into(), runtime)?;
                        heap.call(&classify, namespace, &[error.clone()], runtime)?;
                    }
                    Ok(())
                })?;
                root = Some(error);
                break;
            }
            _ => {}
        }
    }
    // The resource owner retains and joins the original producer even though
    // source result resolution can precede its bookkeeping settlement.
    if let Some(error) = stream.integration_failures().first() {
        return Err(error.as_ref().clone());
    }
    let root = root.ok_or(ValueError::UnsupportedOperation(
        "Edit provider ended without its original result",
    ))?;
    let observed = native(invocation, |runtime| {
        observation(invocation.factory.heap(), &root, runtime)
    })?;
    let write = usage(observed);
    invocation
        .resources
        .register_usage(key, 0, write)?
        .await
        .map_err(|_| ValueError::UnsupportedOperation("Edit repair usage persistence failed"))?;
    Ok(root)
}
fn thinking_loop(invocation: &EditNativeRepairInvocation, root: &ValueHandle) -> Result<bool> {
    native(invocation, |runtime| {
        let heap = invocation.factory.heap();
        if !heap.object_is(
            &heap.get(root, &"stopReason".into(), runtime)?,
            &heap.string("error".into())?,
        )? {
            return Ok(false);
        }
        let content = heap.get(root, &"content".into(), runtime)?;
        if !heap.object_is(
            &heap.get(&content, &"length".into(), runtime)?,
            &heap.number(0.0)?,
        )? {
            return Ok(false);
        }
        let namespace = &invocation.module.ai_error;
        let method = heap.get(namespace, &"is".into(), runtime)?;
        let id = heap.get(root, &"errorId".into(), runtime)?;
        let flags = heap.get(namespace, &"Flag".into(), runtime)?;
        let flag = heap.get(&flags, &"ThinkingLoop".into(), runtime)?;
        heap.truthy(&heap.call(&method, namespace, &[id, flag], runtime)?)
    })
}
fn throw_if_aborted(invocation: &EditNativeRepairInvocation) -> Result<()> {
    native(invocation, |runtime| {
        let heap = invocation.factory.heap();
        let method = heap.get(&invocation.signal, &"throwIfAborted".into(), runtime)?;
        heap.call(&method, &invocation.signal, &[], runtime)?;
        Ok(())
    })
}
async fn wait(invocation: &EditNativeRepairInvocation, delay: f64) -> Result<()> {
    ara_ai::retry::stream::scheduler_wait(
        &invocation.factory,
        invocation.bindings.clone(),
        delay,
        &invocation.signal,
    )
    .await
}
pub(super) fn complete(
    provider: Arc<dyn ModelProvider>,
    model: Model,
    usage: UsageCallback,
    invocation: EditNativeRepairInvocation,
    prompt: String,
    cancel: CancellationToken,
) -> BoxFuture<'static, Result<String>> {
    Box::pin(async move {
        let options = native(&invocation, |runtime| {
            let heap = invocation.factory.heap();
            let options = heap.object()?;
            heap.define_own_data(
                &options,
                "signal".into(),
                invocation.signal.clone(),
                runtime,
            )?;
            heap.define_own_data(&options, "provider".into(), heap.string(model.provider.clone().into())?, runtime)?;
            Ok(options)
        })?;
        let run = Arc::new(Attempt {
            invocation: invocation.clone(),
            provider,
            model,
            prompt,
            usage,
            cancel,
        });
        let response = retry_transient_completion(
            invocation.factory.clone(),
            invocation.bindings.clone(),
            invocation.module.clone(),
            run,
            Some(options),
        )
        .await?;
        native(&invocation, |runtime| {
            let heap = invocation.factory.heap();
            if heap.object_is(
                &heap.get(&response, &"stopReason".into(), runtime)?,
                &heap.string("error".into())?,
            )? {
                let message = heap.get(&response, &"errorMessage".into(), runtime)?;
                let message =
                    if matches!(heap.kind(&message)?, ValueKind::Undefined | ValueKind::Null) {
                        heap.string("auto-repair completion failed".into())?
                    } else {
                        message
                    };
                return Err(ValueError::Thrown(heap.construct(
                    &invocation.bindings.error_constructor(),
                    &[message],
                    runtime,
                )?));
            }
            if heap.object_is(
                &heap.get(&response, &"stopReason".into(), runtime)?,
                &heap.string("aborted".into())?,
            )? {
                // Core's shared deadline distinguishes timeout from caller
                // cancellation. Do not read content or consume candidate two.
                return Err(ValueError::Thrown(heap.construct(
                    &invocation.bindings.error_constructor(),
                    &[heap.string("auto-repair aborted".into())?], runtime,
                )?));
            }
            // Source content.map(...).join("") selects both original methods
            // and preserves array holes/getters/UTF-16 until the repair boundary.
            let content = heap.get(&response, &"content".into(), runtime)?;
            let map = heap.get(&content, &"map".into(), runtime)?;
            let callback = heap.fresh_compiled_function(&TEXT_BLOCK, &[])?;
            let mapped = heap.call(&map, &content, &[callback], runtime)?;
            let join = heap.get(&mapped, &"join".into(), runtime)?;
            let text = heap.call(&join, &mapped, &[heap.string("".into())?], runtime)?;
            scalar(heap, &text)
        })
    })
}
static TEXT_BLOCK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da edit/auto-repair.ts complete content mapper",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, args, runtime| {
        let block = args.first().cloned().unwrap_or(heap.undefined()?);
        if heap.object_is(
            &heap.get(&block, &"type".into(), runtime)?,
            &heap.string("text".into())?,
        )? {
            heap.get(&block, &"text".into(), runtime)
        } else {
            heap.string("".into())
        }
    },
};
fn scalar(heap: &Arc<ValueHeap>, value: &ValueHandle) -> Result<String> {
    match heap.snapshot(value)? {
        ValueSnapshot::String(value) => value.to_utf8().map_err(|_| {
            ValueError::LegacyBridge("Edit repair boundary requires Unicode scalar text")
        }),
        _ => Err(ValueError::TypeError(
            "Edit observed scalar must be a string".into(),
        )),
    }
}
fn observation(
    heap: &Arc<ValueHeap>,
    root: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<UsageObservation> {
    let get = |value: &ValueHandle, key: &str| heap.get(value, &key.into(), runtime);
    let text = |key: &str| scalar(heap, &get(root, key)?);
    let optional_text = |key: &str| -> Result<Option<String>> {
        let value = get(root, key)?;
        Ok(
            if matches!(heap.kind(&value)?, ValueKind::Null | ValueKind::Undefined) {
                None
            } else {
                Some(scalar(heap, &value)?)
            },
        )
    };
    let source = get(root, "usage")?;
    let count = |key: &str| -> Result<Option<u64>> {
        if matches!(heap.kind(&source)?, ValueKind::Null | ValueKind::Undefined) {
            return Ok(None);
        }
        Ok(match heap.snapshot(&get(&source, key)?)? {
            ValueSnapshot::Number(value)
                if value.is_finite()
                    && value >= 0.0
                    && value < 18_446_744_073_709_551_616.0
                    && value.fract() == 0.0 =>
            {
                Some(value as u64)
            }
            _ => None,
        })
    };
    let cost_root = if matches!(heap.kind(&source)?, ValueKind::Null | ValueKind::Undefined) {
        heap.undefined()?
    } else {
        get(&source, "cost")?
    };
    let money = |key: &str| -> Result<Option<f64>> {
        if matches!(
            heap.kind(&cost_root)?,
            ValueKind::Null | ValueKind::Undefined
        ) {
            return Ok(None);
        }
        Ok(match heap.snapshot(&get(&cost_root, key)?)? {
            ValueSnapshot::Number(value) if value.is_finite() => Some(value),
            _ => None,
        })
    };
    let cost = match (
        money("input")?,
        money("output")?,
        money("cacheRead")?,
        money("cacheWrite")?,
        money("total")?,
    ) {
        (Some(input), Some(output), Some(cache_read), Some(cache_write), Some(total)) => {
            Some(Cost {
                input,
                output,
                cache_read,
                cache_write,
                total,
            })
        }
        _ => None,
    };
    let usage = Usage {
        input: count("input")?,
        output: count("output")?,
        cache_read: count("cacheRead")?,
        cache_write: count("cacheWrite")?,
        total_tokens: count("totalTokens")?,
        reasoning_tokens: count("reasoningTokens")?,
        cost,
    };
    let stop_reason = match text("stopReason")?.as_str() {
        "stop" => StopReason::Stop,
        "length" => StopReason::Length,
        "toolUse" => StopReason::ToolUse,
        "error" => StopReason::Error,
        "aborted" => StopReason::Aborted,
        _ => {
            return Err(ValueError::TypeError(
                "Edit observed stopReason is unknown".into(),
            ));
        }
    };
    let record = ara_session::ModelUsageRecord {
        purpose: "edit-auto-repair".into(),
        role: Some("smol".into()),
        api: text("api")?,
        provider: text("provider")?,
        model: text("model")?,
        usage,
        stop_reason,
        error_message: optional_text("errorMessage")?,
    };
    let broker = match (
        &record.usage.input,
        &record.usage.output,
        &record.usage.cache_read,
        &record.usage.cache_write,
        &record.usage.cost,
    ) {
        (Some(input), Some(output), Some(cache_read), Some(cache_write), Some(cost)) => {
            let at = match heap.snapshot(&get(root, "timestamp")?)? {
                ValueSnapshot::Number(value)
                    if value.is_finite()
                        && value.fract() == 0.0
                        && (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0)
                            .contains(&value) =>
                {
                    value as i64
                }
                _ => {
                    return Err(ValueError::LegacyBridge(
                        "Edit Broker timestamp is outside i64 milliseconds",
                    ));
                }
            };
            match (
                i64::try_from(*input),
                i64::try_from(*output),
                i64::try_from(*cache_read),
                i64::try_from(*cache_write),
            ) {
                (
                    Ok(input_tokens),
                    Ok(output_tokens),
                    Ok(cache_read_tokens),
                    Ok(cache_write_tokens),
                ) => Some(ara_cli::credential_store::ClientUsageEntry {
                    at,
                    provider: record.provider.clone(),
                    model: record.model.clone(),
                    requests: 1,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    cost_usd: cost.total,
                }),
                _ => None,
            }
        }
        _ => None,
    };
    Ok(UsageObservation::Actual { record, broker })
}
