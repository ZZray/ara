//! Original 33 consumers pinned to OMP596f2da, plus bounded b91db66e retry delta.
//! MIT; Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
use super::{Harness, Result};
use ara_ai::{
    ActualAssistantEvent as Event, ActualStreamFailureReceipt, StopReason,
    graph_values::GraphValueFactory,
    retry::{
        RetryBindings,
        oneshot::{NativeCompletionAttempt, OneshotModule, retry_transient_completion},
        result::ActualCompletionStream,
        stream::{
            ReplaySafeCompletionAttempt, ReplaySafeStreamModule, with_replay_safe_stream_retry,
        },
    },
};
use ara_cli::retry_host::NativeRetryBindings;
use ara_rpc::value_heap::{CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

fn check(ok: bool, message: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(ValueError::UnsupportedOperation(message))
    }
}
fn begin(id: &str) {
    eprintln!("ARA-RETRY41 BEGIN {id}");
}
fn pass(id: &str) {
    eprintln!("ARA-RETRY41 PASS {id}");
}
fn modules(h: &Harness) -> Result<(Arc<OneshotModule>, Arc<ReplaySafeStreamModule>)> {
    // Reuses the original Session realm, original frozen calling Run and its
    // constructor domain. It creates no Session, driver, classifier fixture.
    let native = NativeRetryBindings::prepare(
        &h.factory,
        &h.realm,
        h.frozen.clone(),
        ActualStreamFailureReceipt::default(),
    )?;
    let oneshot = native.oneshot_module(&h.factory)?;
    let retryable = h.get(&oneshot.ai_error, "isProviderRetryableError")?;
    let classify_message = h.get(&oneshot.ai_error, "classifyMessage")?;
    let stream = h.with(|_| {
        ReplaySafeStreamModule::prepare(
            &h.factory,
            h.bindings.error_constructor(),
            retryable,
            classify_message,
        )
    })?;
    Ok((oneshot, Arc::new(stream)))
}
fn message(
    h: &Harness,
    status: Option<f64>,
    text: Option<&str>,
    content: &str,
) -> Result<ValueHandle> {
    let mut value = serde_json::json!({"role":"assistant","content":[],"api":"anthropic-messages",
        "provider":"anthropic","model":"claude-sonnet-4-6","timestamp":0,
        "stopReason":if status.is_some(){"error"}else{"stop"},
        "usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,
            "cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}}});
    if !content.is_empty() {
        value["content"] = serde_json::json!([{"type":"text","text":content}]);
    }
    if let Some(status) = status {
        value["errorStatus"] = serde_json::json!(status);
    }
    if let Some(text) = text {
        value["errorMessage"] = serde_json::json!(text);
    }
    h.data(value)
}
fn assistant(h: &Harness, content: &str, failure: bool) -> Result<ValueHandle> {
    let root = message(
        h,
        None,
        if failure {
            Some("The socket connection was closed unexpectedly")
        } else {
            None
        },
        content,
    )?;
    for (key, text) in [
        ("api", "openai-completions"),
        ("provider", "test"),
        ("model", "test-model"),
        ("stopReason", if failure { "error" } else { "stop" }),
    ] {
        h.put(&root, key, h.heap().string(text.into())?)?;
    }
    h.put(&root, "timestamp", h.heap().number(1.0)?)?;
    Ok(root)
}
fn error(h: &Harness, status: Option<f64>, text: &str) -> Result<ValueHandle> {
    let result = h.with(|r| {
        h.heap().construct(
            &h.bindings.error_constructor(),
            &[h.heap().string(text.into())?],
            r,
        )
    })?;
    if let Some(status) = status {
        h.put(&result, "status", h.heap().number(status)?)?;
    }
    Ok(result)
}
static RETURN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original unchanged callback result",
    name: "",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::Arrow,
    body: |_, c, _, _, _| Ok(c[0].clone()),
};
static RUN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original oneshot attempt increments calls and returns Promise.resolve or throws its original error",
    name: "run",
    length: 1,
    capture_count: 3,
    form: CompiledFunctionForm::Arrow,
    body: |heap, c, _, args, r| {
        let old = heap.to_number(&heap.get(&c[0], &"calls".into(), r)?, r)?;
        let count = old + 1.0;
        heap.define_own_data(&c[0], "calls".into(), heap.number(count)?, r)?;
        if !heap.object_is(
            args.first().ok_or(ValueError::InvalidNode)?,
            &heap.number(count)?,
        )? {
            return Err(ValueError::UnsupportedOperation(
                "Original source 1-based attempt argument changed",
            ));
        }
        let hook = heap.get(&c[0], &"onAttempt".into(), r)?;
        if heap.truthy(&hook)? {
            heap.call(&hook, &heap.undefined()?, &[], r)?;
        }
        let length = heap.to_number(&heap.get(&c[1], &"length".into(), r)?, r)?;
        let index = old.min(length - 1.0).to_string();
        let original = heap.get(&c[1], &index.as_str().into(), r)?;
        if heap.truthy(&heap.get(&c[2], &index.as_str().into(), r)?)? {
            return Err(ValueError::Thrown(original));
        }
        let promise = heap.get(&c[0], &"Promise".into(), r)?;
        heap.call(
            &heap.get(&promise, &"resolve".into(), r)?,
            &promise,
            &[original],
            r,
        )
    },
};
static ON_RETRY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original onRetry records its actual info argument",
    name: "onRetry",
    length: 1,
    capture_count: 1,
    form: CompiledFunctionForm::Arrow,
    body: |heap, c, _, args, r| {
        let seen = heap.get(&c[0], &"seen".into(), r)?;
        heap.call(
            &heap.get(&seen, &"push".into(), r)?,
            &seen,
            &[args.first().cloned().ok_or(ValueError::InvalidNode)?],
            r,
        )?;
        heap.undefined()
    },
};
static ABORT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original attempt cancels its original caller controller",
    name: "",
    length: 0,
    capture_count: 2,
    form: CompiledFunctionForm::Arrow,
    body: |heap, c, _, _, r| {
        heap.call(
            &heap.get(&c[0], &"abort".into(), r)?,
            &c[0],
            &[c[1].clone()],
            r,
        )
    },
};
fn state(h: &Harness) -> Result<ValueHandle> {
    h.object(&[
        ("calls", h.heap().number(0.0)?),
        ("seen", h.heap().array(Vec::new())?),
        ("Promise", h.get(&h.bindings.globals(), "Promise")?),
    ])
}
fn attempt(
    h: &Harness,
    state: &ValueHandle,
    results: &[ValueHandle],
    throws: &[bool],
) -> Result<Arc<NativeCompletionAttempt>> {
    let flags = throws
        .iter()
        .map(|b| h.heap().boolean(*b))
        .collect::<Result<Vec<_>>>()?;
    let call = h.heap().fresh_compiled_function(
        &RUN,
        &[
            state.clone(),
            h.heap().array(results.to_vec())?,
            h.heap().array(flags)?,
        ],
    )?;
    Ok(Arc::new(NativeCompletionAttempt::prepare(
        h.factory.clone(),
        h.bindings.clone(),
        call,
    )?))
}
const ONE_IDS: [&str; 20] = [
    "B-e5a6d43119",
    "B-c422c3629f",
    "B-1e785fd4e5",
    "B-5aec2f8932",
    "B-5687cb2964",
    "B-46373b26c2",
    "B-86770b21c1",
    "B-0d5e47a472",
    "B-449653cacc",
    "B-29e40a320c",
    "B-fd84897443",
    "B-03e914d292",
    "B-ad0bb4c654",
    "B-0ad165b224",
    "B-56b36366ae",
    "B-920ed4247d",
    "B-b1c9dccc7a",
    "B-96e65c9ea1",
    "B-f232d10b2e",
    "B-d944818944",
];
async fn original_oneshot20(h: &mut Harness, module: Arc<OneshotModule>) -> Result<()> {
    for (case, id) in ONE_IDS.into_iter().enumerate() {
        begin(id);
        let state = state(h)?;
        let options = h.data(serde_json::json!({"baseDelayMs":1,"maxAttempts":3}))?;
        h.put(
            &options,
            "onRetry",
            h.heap()
                .fresh_compiled_function(&ON_RETRY, &[state.clone()])?,
        )?;
        let success = message(h, None, None, "")?;
        let overloaded = "Anthropic stream error (overloaded_error): Overloaded";
        let rate = "rate_limit_error: too many requests";
        let (status, text, throws, expected_calls, last_success) = match case {
            0 => (Some(529.0), overloaded, false, 3, true),
            1 | 13 | 15 => (Some(429.0), rate, false, 2, true),
            2 => (Some(503.0), "request failed", false, 2, true),
            3 => (Some(529.0), overloaded, false, 3, false),
            4 => (
                Some(400.0),
                "invalid_request_error: messages: at least one message is required",
                false,
                1,
                false,
            ),
            5 => (
                Some(400.0),
                "invalid_request_error: prompt is too long: 3059586 tokens > 1000000 maximum (raw-http-request=/logs/1787022540720-3o503gxo48bvb.json)",
                false,
                1,
                false,
            ),
            6 => (
                Some(413.0),
                "Provider returned error: 413 Payload Too Large",
                false,
                1,
                false,
            ),
            7 => (
                Some(500.0),
                "failed to parse tool call arguments as JSON",
                false,
                1,
                false,
            ),
            8 => (
                Some(529.0),
                "529 overloaded_error: Overloaded",
                true,
                3,
                false,
            ),
            9 => (Some(503.0), "request failed", true, 2, true),
            10 => (
                None,
                "invalid_request_error: bad tool schema",
                true,
                1,
                false,
            ),
            11 | 12 => (Some(529.0), overloaded, false, 1, false),
            14 => (
                Some(429.0),
                "rate_limit_error: please retry in 600s",
                false,
                1,
                false,
            ),
            16 => (
                Some(429.0),
                "rate_limit_error: too many requests retry-after-ms=5",
                false,
                2,
                true,
            ),
            17 => (
                Some(429.0),
                "rate_limit_error: too many requests retry-after-ms=12000",
                false,
                1,
                false,
            ),
            18 => (Some(429.0), rate, false, 1, false),
            19 => (
                Some(529.0),
                "529 overloaded_error: Overloaded",
                true,
                2,
                false,
            ),
            _ => return Err(ValueError::InvalidNode),
        };
        let failure = if throws {
            error(h, status, text)?
        } else {
            message(h, status, Some(text), "")?
        };
        if (4..=7).contains(&case) {
            h.put(&options, "maxAttempts", h.heap().number(5.0)?)?;
        }
        if matches!(case, 14 | 17 | 18) {
            h.put(&options, "maxDelayMs", h.heap().number(1000.0)?)?;
        }
        if matches!(case, 15 | 16 | 19) {
            h.put(&options, "maxAttempts", h.heap().number(2.0)?)?;
        }
        if matches!(case, 15 | 18) {
            let headers = if case == 15 {
                h.data(serde_json::json!({"retry-after-ms":"120"}))?
            } else {
                h.data(serde_json::json!({"retry-after":"300"}))?
            };
            h.put(
                &options,
                "getResponseHeaders",
                h.heap().fresh_compiled_function(&RETURN, &[headers])?,
            )?;
        }
        if case == 19 {
            h.put(
                &failure,
                "headers",
                h.data(serde_json::json!({"retry-after-ms":"90"}))?,
            )?;
        }
        let mut abort_reason = None;
        let mut scheduled_callback = None;
        if matches!(case, 11 | 12) {
            let controller = h.with(|r| h.bindings.signals().controller(&h.factory, r))?;
            let reason = if case == 12 {
                error(h, None, "user pressed escape")?
            } else {
                h.heap().undefined()?
            };
            h.put(&options, "signal", controller.signal.clone())?;
            if case == 11 {
                h.put(
                    &state,
                    "onAttempt",
                    h.heap()
                        .fresh_compiled_function(&ABORT, &[controller.controller, reason])?,
                )?;
            } else {
                h.put(&options, "baseDelayMs", h.heap().number(200.0)?)?;
                abort_reason = Some(reason.clone());
                let factory = h.factory.clone();
                let bindings = h.bindings.clone();
                let armed = bindings.clone();
                let calling = h.frozen.clone();
                let body: Arc<dyn Fn() -> Result<()> + Send + Sync> = Arc::new(move || {
                    let factory = factory.clone();
                    let bindings = bindings.clone();
                    let controller = controller.clone();
                    let reason = reason.clone();
                    let calling = calling.clone();
                    armed.timers().arm(
                        5.0,
                        Arc::new(move || {
                            calling.with_runtime(&factory, &mut |r| {
                                bindings.signals().abort(
                                    &factory,
                                    &controller,
                                    Some(reason.clone()),
                                    r,
                                )?;
                                factory.heap().undefined()
                            })?;
                            Ok(())
                        }),
                    )?;
                    Ok(())
                });
                let callback =
                    h.with(|r| h.bindings.signals().callback(&h.factory, body.clone(), r))?;
                h.put(&state, "onAttempt", callback)?;
                scheduled_callback = Some(body);
            }
        }
        let mut results = vec![failure.clone()];
        let mut flags = vec![throws];
        if last_success {
            if case == 0 {
                results.push(failure.clone());
                flags.push(false);
            }
            results.push(success.clone());
            flags.push(false);
        }
        let run = attempt(h, &state, &results, &flags)?;
        let future = retry_transient_completion(
            h.factory.clone(),
            h.bindings.clone(),
            module.clone(),
            run,
            Some(options),
        );
        let outcome = h.run_until(async { Ok(future.await) }).await?;
        let _scheduled_callback = scheduled_callback;
        check(
            h.n(&h.get(&state, "calls")?)? == f64::from(expected_calls),
            "Original oneshot source call count changed",
        )?;
        if case == 12 {
            check(
                matches!(&outcome,Err(ValueError::Thrown(v))if v.same_identity(abort_reason.as_ref().unwrap())),
                "Backoff cancellation did not preserve original caller reason identity",
            )?;
        } else if throws && !last_success {
            check(
                matches!(&outcome,Err(ValueError::Thrown(v))if v.same_identity(&failure)),
                "Original thrown final provider error identity lost",
            )?;
            check(
                h.s(&h.get(&failure, "message")?)? == text,
                "Original thrown error text changed",
            )?;
        } else {
            let original = outcome?;
            check(
                original.same_identity(if last_success { &success } else { &failure }),
                "Original resolved completion identity changed",
            )?;
            check(
                h.s(&h.get(&original, "stopReason")?)?
                    == if last_success { "stop" } else { "error" },
                "Original oneshot source stopReason changed",
            )?;
            if case == 3 {
                check(
                    h.s(&h.get(&original, "errorMessage")?)?
                        .contains("overloaded_error"),
                    "Exhausted overload text lost",
                )?;
            }
            if case == 6 {
                check(
                    h.s(&h.get(&original, "errorMessage")?)?.contains("413"),
                    "Payload rejection text lost",
                )?;
            }
        }
        let seen = h.get(&state, "seen")?;
        if case == 13 {
            check(
                h.n(&h.get(&seen, "length")?)? == 1.0
                    && h.n(&h.get(&h.get(&seen, "0")?, "attempt")?)? == 1.0,
                "Original onRetry attempts are not exactly [1]",
            )?;
        }
        if matches!(case, 15 | 16 | 19) {
            let last = h.n(&h.get(&seen, "length")?)? as usize - 1;
            let info = h.get(&seen, &last.to_string())?;
            let expected = match case {
                15 => 120.0,
                16 => 5.0,
                _ => 90.0,
            };
            check(
                h.n(&h.get(&info, "delayMs")?)? == expected,
                "Original retry-after delay changed",
            )?;
        }
        pass(id);
    }
    Ok(())
}

struct Streams {
    source: Mutex<VecDeque<Result<ActualCompletionStream>>>,
    calls: AtomicUsize,
}
impl ReplaySafeCompletionAttempt for Streams {
    fn start(
        &self,
        _: ValueHandle,
        _: ValueHandle,
        _: ValueHandle,
    ) -> Result<ActualCompletionStream> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.source
            .lock()
            .map_err(|_| ValueError::PoisonedHeap)?
            .pop_front()
            .ok_or(ValueError::UnsupportedOperation(
                "Original fixture exhausted without its source cap",
            ))?
    }
}
static WAIT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original providerRetryWait records its actual delay, optionally aborts, and settles its original Promise",
    name: "providerRetryWait",
    length: 1,
    capture_count: 4,
    form: CompiledFunctionForm::Arrow,
    body: |heap, c, _, args, r| {
        let waits = heap.get(&c[0], &"waits".into(), r)?;
        heap.call(
            &heap.get(&waits, &"push".into(), r)?,
            &waits,
            &[args.first().cloned().ok_or(ValueError::InvalidNode)?],
            r,
        )?;
        if heap.truthy(&c[1])? {
            heap.call(&heap.get(&c[1], &"abort".into(), r)?, &c[1], &[], r)?;
        }
        let promise = heap.get(&c[0], &"Promise".into(), r)?;
        let key = if heap.truthy(&c[2])? {
            "reject"
        } else {
            "resolve"
        };
        heap.call(
            &heap.get(&promise, &key.into(), r)?,
            &promise,
            &[c[3].clone()],
            r,
        )
    },
};
async fn source_stream(
    h: &Harness,
    root: &ValueHandle,
    flavor: usize,
) -> Result<ActualCompletionStream> {
    let (mut producer, stream, _) = ActualCompletionStream::channel(&h.factory);
    producer
        .push(Event::Start {
            partial: root.clone(),
        })
        .await?;
    let index = h.heap().number(0.0)?;
    match flavor {
        1 | 5 => {
            producer
                .push(Event::TextStart {
                    content_index: index.clone(),
                    partial: root.clone(),
                })
                .await?;
            producer
                .push(Event::TextDelta {
                    content_index: index.clone(),
                    delta: if flavor == 5 { "partial" } else { "hello" }.into(),
                    partial: root.clone(),
                })
                .await?;
            if flavor == 1 {
                producer
                    .push(Event::TextEnd {
                        content_index: index,
                        content: "hello".into(),
                        partial: root.clone(),
                    })
                    .await?;
            }
        }
        2 => {
            producer
                .push(Event::ThinkingDelta {
                    content_index: index,
                    delta: "pondering".into(),
                    partial: root.clone(),
                })
                .await?;
        }
        3 => {
            producer
                .push(Event::ThinkingStart {
                    content_index: index,
                    partial: root.clone(),
                })
                .await?;
        }
        _ => {}
    }
    if matches!(flavor, 4 | 5) {
        producer
            .push(Event::Error {
                reason: StopReason::Error,
                error: root.clone(),
            })
            .await?;
    } else {
        producer
            .push(Event::Done {
                reason: StopReason::Stop,
                message: root.clone(),
            })
            .await?;
    }
    drop(producer);
    Ok(stream)
}
const STREAM_IDS: [&str; 13] = [
    "B-f3204ba5ac",
    "B-d6c439c35f",
    "B-c33421f9ce",
    "B-c806dd7ea5",
    "B-86ff424237",
    "B-071514526e",
    "B-d3e7a24950",
    "B-94812de6f0",
    "B-8176bb72d4",
    "B-c1662ae6aa",
    "B-835c791d85",
    "B-d43bbe6198",
    "B-97fba73699",
];
async fn original_stream13(h: &mut Harness, module: Arc<ReplaySafeStreamModule>) -> Result<()> {
    for (case, id) in STREAM_IDS.into_iter().enumerate() {
        begin(id);
        let state = h.object(&[
            ("waits", h.heap().array(Vec::new())?),
            ("Promise", h.get(&h.bindings.globals(), "Promise")?),
        ])?;
        let options = h.heap().object()?;
        let failure = error(
            h,
            None,
            if case == 12 {
                "explicit prompt caching is unsupported"
            } else {
                "wait boom"
            },
        )?;
        let controller = if case == 7 {
            Some(h.with(|r| h.bindings.signals().controller(&h.factory, r))?)
        } else {
            None
        };
        if let Some(c) = &controller {
            h.put(&options, "signal", c.signal.clone())?;
        }
        h.put(
            &options,
            "providerRetryWait",
            h.heap().fresh_compiled_function(
                &WAIT,
                &[
                    state.clone(),
                    controller
                        .as_ref()
                        .map(|c| c.controller.clone())
                        .unwrap_or(h.heap().undefined()?),
                    h.heap().boolean(matches!(case, 6 | 7))?,
                    failure.clone(),
                ],
            )?,
        )?;
        let empty = assistant(h, "", false)?;
        let content = assistant(h, "hello", false)?;
        if case == 1 {
            let usage = h.get(&empty, "usage")?;
            h.put(&usage, "output", h.heap().number(1.0)?)?;
            h.put(&usage, "totalTokens", h.heap().number(1.0)?)?;
        }
        if case == 3 {
            h.put(
                &empty,
                "stopDetails",
                h.data(serde_json::json!({"type":"pause_turn"}))?,
            )?;
        }
        let transient = assistant(h, if case == 11 { "partial" } else { "" }, true)?;
        let mut sources = VecDeque::new();
        match case {
            0 => {
                sources.push_back(Ok(source_stream(h, &empty, 0).await?));
                sources.push_back(Ok(source_stream(h, &empty, 0).await?));
                sources.push_back(Ok(source_stream(h, &content, 1).await?));
            }
            1 | 8 => {
                sources.push_back(Ok(
                    source_stream(h, &empty, if case == 8 { 3 } else { 0 }).await?
                ));
                sources.push_back(Ok(source_stream(h, &content, 1).await?));
            }
            2 => {
                for _ in 0..3 {
                    sources.push_back(Ok(source_stream(h, &empty, 0).await?));
                }
            }
            3 | 6 | 7 => sources.push_back(Ok(source_stream(h, &empty, 0).await?)),
            4 => sources.push_back(Ok(source_stream(h, &content, 1).await?)),
            5 => sources.push_back(Ok(source_stream(h, &empty, 2).await?)),
            10 => {
                sources.push_back(Ok(source_stream(h, &transient, 4).await?));
                sources.push_back(Ok(source_stream(h, &content, 1).await?));
            }
            11 => sources.push_back(Ok(source_stream(h, &transient, 5).await?)),
            12 => sources.push_back(Err(ValueError::Thrown(failure.clone()))),
            9 => {}
            _ => return Err(ValueError::InvalidNode),
        }
        let mut streaming_producer = None;
        if case == 9 {
            h.put(
                &content,
                "content",
                h.data(serde_json::json!([{"type":"text","text":"streamed"}]))?,
            )?;
            let (mut producer, inner, _) = ActualCompletionStream::channel(&h.factory);
            producer
                .push(Event::Start {
                    partial: content.clone(),
                })
                .await?;
            producer
                .push(Event::TextDelta {
                    content_index: h.heap().number(0.0)?,
                    delta: "streamed".into(),
                    partial: content.clone(),
                })
                .await?;
            sources.push_back(Ok(inner));
            streaming_producer = Some(producer);
        }
        let attempts = Arc::new(Streams {
            source: Mutex::new(sources),
            calls: AtomicUsize::new(0),
        });
        let policy = h.data(if case >= 10 {
            serde_json::json!({"retryProviderErrors":true,"maxProviderErrorRetries":1})
        } else {
            serde_json::json!({"retryEmptyCompletion":true})
        })?;
        let (mut outer, forward) = with_replay_safe_stream_retry(
            h.factory.clone(),
            h.bindings.clone(),
            module.clone(),
            h.heap().object()?,
            h.heap().object()?,
            Some(options),
            attempts.clone(),
            policy,
        )?;
        let result = outer.original_result.clone();
        let root = content.clone();
        let consumption = async move {
            let mut events = Vec::new();
            if let Some(mut producer) = streaming_producer {
                let first = outer.recv().await?.ok_or(ValueError::InvalidNode)?;
                let second = outer.recv().await?.ok_or(ValueError::InvalidNode)?;
                check(
                    first.type_name() == "start" && second.type_name() == "text_delta",
                    "Original live stream did not deliver start and delta before its terminal exists",
                )?;
                check(
                    !producer.is_done(),
                    "Live stream terminal existed before observed output",
                )?;
                events.push(first);
                events.push(second);
                producer
                    .push(Event::Done {
                        reason: StopReason::Stop,
                        message: root,
                    })
                    .await?;
                drop(producer);
            }
            while let Some(event) = outer.recv().await? {
                events.push(event);
            }
            Ok::<_, ValueError>(events)
        };
        let outcome = h
            .run_until(async move {
                let (work, events) = futures::future::join(forward, consumption).await;
                Ok((work, events, result.result().await))
            })
            .await?;
        outcome.0?;
        let expected_calls = match case {
            0 | 2 => 3,
            1 | 8 | 10 => 2,
            _ => 1,
        };
        check(
            attempts.calls.load(Ordering::SeqCst) == expected_calls,
            "Original stream source attempt count changed",
        )?;
        let waits = h.get(&state, "waits")?;
        let wait_count = h.n(&h.get(&waits, "length")?)?;
        if case == 6 {
            check(
                matches!(&outcome.1,Err(ValueError::Thrown(v))if v.same_identity(&failure)),
                "Original non-abort wait rejection was hidden by empty success",
            )?;
            check(
                h.s(&h.get(&failure, "message")?)? == "wait boom",
                "Original wait error text lost",
            )?;
        } else if case == 12 {
            check(
                matches!(&outcome.2,Err(ValueError::Thrown(v))if v.same_identity(&failure)),
                "Synchronous original factory error did not settle outer result identically",
            )?;
        } else {
            let events = outcome.1?;
            let original = outcome.2?;
            check(
                events.iter().filter(|e| e.type_name() == "start").count() == 1,
                "Retried start markers leaked",
            )?;
            if matches!(case, 0 | 1 | 8 | 10) {
                check(
                    original.same_identity(&content),
                    "Original successful stream result identity lost",
                )?;
                check(
                    events.iter().any(|e| e.type_name() == "text_delta"),
                    "Original successful content delta lost",
                )?;
                check(
                    h.s(&h.get(&h.get(&h.get(&original, "content")?, "0")?, "text")?)? == "hello",
                    "Original successful content changed",
                )?;
            }
            if matches!(case, 2 | 3 | 5 | 7) {
                check(
                    original.same_identity(&empty),
                    "Original empty/thinking result identity changed",
                )?;
            }
            if matches!(case, 2 | 7) {
                check(
                    h.n(&h.get(&h.get(&original, "content")?, "length")?)? == 0.0,
                    "Original empty result gained content",
                )?;
                check(
                    events.last().map(Event::type_name) == Some("done"),
                    "Original empty terminal did not remain done",
                )?;
            }
            if case == 0 {
                check(
                    events.last().map(Event::type_name) == Some("done"),
                    "Original successful final done missing",
                )?;
            }
            if case == 3 {
                check(
                    h.s(&h.get(&h.get(&original, "stopDetails")?, "type")?)? == "pause_turn",
                    "Original pause_turn detail lost",
                )?;
            }
            if case == 5 {
                check(
                    events.iter().any(|e| e.type_name() == "thinking_delta"),
                    "Original thinking output did not commit",
                )?;
            }
            if case == 8 {
                check(
                    !events.iter().any(|e| e.type_name() == "thinking_start"),
                    "Discarded thinking_start leaked",
                )?;
            }
            if case == 9 {
                check(
                    events.last().map(Event::type_name) == Some("done"),
                    "Original live stream final done missing",
                )?;
            }
            if case == 11 {
                check(
                    original.same_identity(&transient)
                        && events.last().map(Event::type_name) == Some("error"),
                    "Committed transient result did not remain original error",
                )?;
                check(
                    h.s(&h.get(&original, "stopReason")?)? == "error",
                    "Committed error stop reason changed",
                )?;
                check(
                    h.s(&h.get(&h.get(&h.get(&original, "content")?, "0")?, "text")?)? == "partial",
                    "Committed partial content changed",
                )?;
            }
        }
        match case {
            0 | 2 => check(
                wait_count == 2.0,
                "Original empty retry cap wait count changed",
            )?,
            1 => check(
                wait_count == 1.0 && h.n(&h.get(&waits, "0")?)? == 500.0,
                "Original EOS retry wait is not [500]",
            )?,
            3 | 4 | 9 => check(
                wait_count == 0.0,
                "Original non-retrying stream invoked providerRetryWait",
            )?,
            _ => {}
        }
        pass(id);
    }
    Ok(())
}
pub async fn original33(h: &mut Harness) -> Result<()> {
    let (oneshot, stream) = modules(h)?;
    original_oneshot20(h, oneshot).await?;
    original_stream13(h, stream).await
}
pub async fn source_groups(h: &mut Harness) -> Result<()> {
    let (oneshot, stream) = modules(h)?;
    eprintln!("ARA-RETRY-SOURCE BEGIN oneshot");
    oneshot_source_contract(h, oneshot.clone()).await?;
    eprintln!("ARA-RETRY-SOURCE PASS oneshot");
    eprintln!("ARA-RETRY-SOURCE BEGIN stream");
    stream_source_contract(h, stream.clone()).await?;
    eprintln!("ARA-RETRY-SOURCE PASS stream");
    eprintln!("ARA-RETRY-SOURCE BEGIN scheduler");
    default_scheduler_source_contract(h, stream.clone()).await?;
    eprintln!("ARA-RETRY-SOURCE PASS scheduler");
    eprintln!("ARA-RETRY-SOURCE BEGIN native-stream");
    native_stream_source_contract(h, stream).await?;
    eprintln!("ARA-RETRY-SOURCE PASS native-stream");
    eprintln!("ARA-RETRY-SOURCE BEGIN upgraded-hint");
    upgraded_hint_source_contract(h, oneshot).await?;
    eprintln!("ARA-RETRY-SOURCE PASS upgraded-hint");
    eprintln!("ARA-RETRY-SOURCE PASS empty-completion-retry");
    eprintln!("ARA-RETRY-SOURCE PASS oneshot-retry");
    Ok(())
}

static THROW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original imported callback throws its original root",
    name: "",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::Arrow,
    body: |_, c, _, _, _| Err(ValueError::Thrown(c[0].clone())),
};
async fn oneshot_source_contract(h: &mut Harness, module: Arc<OneshotModule>) -> Result<()> {
    // Cover authored optional defaults/coercion and callback throws through
    // the actual common owner, supplementing the original 20 resolved/thrown
    // error, hint/cap, onRetry and actual native cancellation consumers.
    for case in 0..5 {
        let state = state(h)?;
        let root = if case == 0 {
            message(h, None, None, "")?
        } else {
            message(h, Some(529.0), Some("529 overloaded_error: Overloaded"), "")?
        };
        let marker = error(h, None, "source callback failure")?;
        let opts = if case == 0 {
            None
        } else {
            let opts = h.data(serde_json::json!({"baseDelayMs":1,"maxAttempts":3}))?;
            match case {
                1 => h.put(&opts, "maxAttempts", h.heap().number(0.0)?)?,
                2 => h.put(&opts, "maxAttempts", h.heap().number(1.5)?)?,
                3 => h.put(
                    &opts,
                    "onRetry",
                    h.heap()
                        .fresh_compiled_function(&THROW, &[marker.clone()])?,
                )?,
                4 => h.put(
                    &opts,
                    "getResponseHeaders",
                    h.heap()
                        .fresh_compiled_function(&THROW, &[marker.clone()])?,
                )?,
                _ => return Err(ValueError::InvalidNode),
            }
            Some(opts)
        };
        let original = attempt(h, &state, &[root.clone()], &[false])?;
        let future = retry_transient_completion(
            h.factory.clone(),
            h.bindings.clone(),
            module.clone(),
            original,
            opts,
        );
        let outcome = h.run_until(async { Ok(future.await) }).await?;
        check(
            h.n(&h.get(&state, "calls")?)? == if case == 2 { 2.0 } else { 1.0 },
            "Optional defaults/fractional attempt source contract changed",
        )?;
        if case >= 3 {
            check(
                matches!(&outcome,Err(ValueError::Thrown(v))if v.same_identity(&marker)),
                "Original onRetry/response-header callback throw identity lost",
            )?;
        } else {
            check(
                outcome?.same_identity(&root),
                "Source default/cap result identity changed",
            )?;
        }
    }
    Ok(())
}
async fn stream_source_contract(
    h: &mut Harness,
    module: Arc<ReplaySafeStreamModule>,
) -> Result<()> {
    use ara_ai::retry::stream::has_visible_assistant_content;
    // Native iterator-based exported content predicate: whitespace and only
    // thinking remain invisible, image/toolCall and nonwhitespace are visible.
    for (blocks, visible) in [
        (serde_json::json!([{"type":"text","text":"  \n"}]), false),
        (
            serde_json::json!([{"type":"thinking","thinking":"thought"}]),
            false,
        ),
        (serde_json::json!([{"type":"text","text":"hello"}]), true),
        (
            serde_json::json!([{"type":"image","data":"x","mimeType":"image/png"}]),
            true,
        ),
        (
            serde_json::json!([{"type":"toolCall","id":"a","name":"t","arguments":{}}]),
            true,
        ),
    ] {
        let root = assistant(h, "", false)?;
        h.put(&root, "content", h.data(blocks)?)?;
        check(
            h.with(|r| has_visible_assistant_content(&h.factory, &root, r))? == visible,
            "Original source visible-content iterator predicate changed",
        )?;
    }
    // acceptEmptyResponse commits immediately; EOF independently forwards the
    // original result; absent source result is never inferred from event EOF.
    for case in 0..2 {
        let root = assistant(h, "", false)?;
        let (mut producer, inner, _) = ActualCompletionStream::channel(&h.factory);
        if case == 0 {
            producer
                .push(Event::Start {
                    partial: root.clone(),
                })
                .await?;
            producer
                .push(Event::Done {
                    reason: StopReason::Stop,
                    message: root.clone(),
                })
                .await?;
        } else {
            producer.end_original(root.clone())?;
        }
        drop(producer);
        let attempts = Arc::new(Streams {
            source: Mutex::new(VecDeque::from([Ok(inner)])),
            calls: AtomicUsize::new(0),
        });
        let options = if case == 0 {
            Some(h.data(serde_json::json!({"acceptEmptyResponse":true}))?)
        } else {
            None
        };
        let (mut stream, work) = with_replay_safe_stream_retry(
            h.factory.clone(),
            h.bindings.clone(),
            module.clone(),
            h.heap().object()?,
            h.heap().object()?,
            options,
            attempts.clone(),
            h.data(serde_json::json!({"retryEmptyCompletion":true}))?,
        )?;
        let result = stream.original_result.clone();
        let consumption = async move {
            let mut count = 0;
            while stream.recv().await?.is_some() {
                count += 1;
            }
            Ok::<_, ValueError>(count)
        };
        let (work, events, result) = h
            .run_until(async move {
                let (work, events) = futures::future::join(work, consumption).await;
                Ok((work, events, result.result().await))
            })
            .await?;
        work?;
        check(
            attempts.calls.load(Ordering::SeqCst) == 1,
            "acceptEmptyResponse/EOF fallback unexpectedly retried",
        )?;
        check(
            events? == if case == 0 { 2 } else { 0 },
            "Original EOF/accept-empty event delivery changed",
        )?;
        check(
            result?.same_identity(&root),
            "Original source EOF result identity lost",
        )?;
    }
    Ok(())
}

static SCHEDULER_GET: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original duck signal getter records source observation and returns its original member",
    name: "",
    length: 0,
    capture_count: 3,
    form: CompiledFunctionForm::Ordinary,
    body: |heap, c, _, _, runtime| {
        let push = heap.get(&c[0], &"push".into(), runtime)?;
        heap.call(&push, &c[0], &[c[1].clone()], runtime)?;
        Ok(c[2].clone())
    },
};
static SCHEDULER_LISTENER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original duck signal listener retains exact callback and receiver across add and remove",
    name: "",
    length: 2,
    capture_count: 5,
    form: CompiledFunctionForm::Ordinary,
    body: |heap, c, receiver, args, runtime| {
        check(
            receiver.same_identity(&c[2]),
            "Scheduler duck listener receiver changed",
        )?;
        let event = args.first().ok_or(ValueError::InvalidNode)?;
        check(
            heap.to_string(event, runtime)?
                .to_utf8()
                .map_err(|_| ValueError::InvalidNode)?
                == "abort",
            "Scheduler duck listener event changed",
        )?;
        let listener = args.get(1).ok_or(ValueError::InvalidNode)?;
        let push = heap.get(&c[0], &"push".into(), runtime)?;
        heap.call(&push, &c[0], &[c[1].clone()], runtime)?;
        if heap.truthy(&c[4])? {
            heap.define_own_data(&c[3], "listener".into(), listener.clone(), runtime)?;
        } else {
            check(
                listener.same_identity(&heap.get(&c[3], &"listener".into(), runtime)?),
                "Scheduler remove listener did not receive original callback",
            )?;
        }
        heap.undefined()
    },
};
async fn default_scheduler_source_contract(
    h: &mut Harness,
    module: Arc<ReplaySafeStreamModule>,
) -> Result<()> {
    use ara_ai::retry::stream::scheduler_wait;
    // Source utility without providerRetryWait must actually replay through its
    // default scheduler. One bounded 500ms wait exercises the original branch.
    let empty = assistant(h, "", false)?;
    let complete = assistant(h, "default scheduler replay", false)?;
    let mut sources = VecDeque::new();
    for root in [&empty, &complete] {
        let (mut producer, stream, _) = ActualCompletionStream::channel(&h.factory);
        producer
            .push(Event::Done {
                reason: StopReason::Stop,
                message: root.clone(),
            })
            .await?;
        drop(producer);
        sources.push_back(Ok(stream));
    }
    let attempts = Arc::new(Streams {
        source: Mutex::new(sources),
        calls: AtomicUsize::new(0),
    });
    let (mut outer, work) = with_replay_safe_stream_retry(
        h.factory.clone(),
        h.bindings.clone(),
        module,
        h.heap().object()?,
        h.heap().object()?,
        None,
        attempts.clone(),
        h.data(serde_json::json!({"retryEmptyCompletion":true}))?,
    )?;
    let result = outer.original_result.clone();
    let (worked, forwarded, original_result) = h
        .run_until(async move {
            let consume = async move {
                let mut events = Vec::new();
                while let Some(event) = outer.recv().await? {
                    events.push(event);
                }
                Ok::<_, ValueError>(events)
            };
            let (worked, forwarded) = futures::future::join(work, consume).await;
            Ok((worked, forwarded, result.result().await))
        })
        .await?;
    worked?;
    check(
        attempts.calls.load(Ordering::SeqCst) == 2 && forwarded?.len() == 1,
        "Default scheduler utility failed to suppress empty attempt and replay once",
    )?;
    check(
        original_result?.same_identity(&complete),
        "Default scheduler replay original result identity lost",
    )?;

    // Fixed Bun1.4.0 revision34cbb9a oracle: ordinary duck property order is
    // aborted -> add -> addCall -> remove -> removeCall, with original callback.
    let trace = h.heap().array(Vec::new())?;
    let duck = h.heap().object()?;
    let listener_state = h.heap().object()?;
    for (key, label, value) in [
        ("aborted", "aborted", h.heap().boolean(false)?),
        (
            "addEventListener",
            "add",
            h.heap().fresh_compiled_function(
                &SCHEDULER_LISTENER,
                &[
                    trace.clone(),
                    h.heap().string("addCall".into())?,
                    duck.clone(),
                    listener_state.clone(),
                    h.heap().boolean(true)?,
                ],
            )?,
        ),
        (
            "removeEventListener",
            "remove",
            h.heap().fresh_compiled_function(
                &SCHEDULER_LISTENER,
                &[
                    trace.clone(),
                    h.heap().string("removeCall".into())?,
                    duck.clone(),
                    listener_state.clone(),
                    h.heap().boolean(false)?,
                ],
            )?,
        ),
    ] {
        accessor(
            h,
            &duck,
            key,
            h.heap().fresh_compiled_function(
                &SCHEDULER_GET,
                &[trace.clone(), h.heap().string(label.into())?, value],
            )?,
        )?;
    }
    let factory = h.factory.clone();
    let bindings = h.bindings.clone();
    h.run_until(async move { scheduler_wait(&factory, bindings, 1.0, &duck).await })
        .await?;
    check(
        h.heap().export_serde(&trace)?
            == serde_json::json!(["aborted", "add", "addCall", "remove", "removeCall"]),
        "Default scheduler duck getter/call order changed",
    )?;

    let invalid = h.heap().boolean(false)?;
    let factory = h.factory.clone();
    let bindings = h.bindings.clone();
    let invalid = h
        .run_until(async move { Ok(scheduler_wait(&factory, bindings, 1.0, &invalid).await) })
        .await?;
    let Err(ValueError::Thrown(error)) = invalid else {
        return Err(ValueError::UnsupportedOperation(
            "False scheduler signal was not original TypeError",
        ));
    };
    check(
        h.s(&h.get(&error, "name")?)? == "TypeError"
            && h.s(&h.get(&error, "code")?)? == "ERR_INVALID_ARG_TYPE"
            && h.s(&h.get(&error, "message")?)?
                == "The \"options.signal\" property must be of type AbortSignal. Received type boolean (false)",
        "Default scheduler false-signal error differs from fixed Bun oracle",
    )?;

    for native_mid_abort in [false, true] {
        let reason = h.heap().object()?;
        let controller = if native_mid_abort {
            Some(h.with(|r| h.bindings.signals().controller(&h.factory, r))?)
        } else {
            None
        };
        let signal = if let Some(controller) = &controller {
            controller.signal.clone()
        } else {
            h.object(&[
                ("aborted", h.heap().boolean(true)?),
                ("reason", reason.clone()),
            ])?
        };
        let factory = h.factory.clone();
        let bindings = h.bindings.clone();
        let calling = h.frozen.clone();
        let original_reason = reason.clone();
        let outcome = h
            .run_until(async move {
                let wait = scheduler_wait(&factory, bindings.clone(), 200.0, &signal);
                tokio::pin!(wait);
                if let Some(controller) = controller {
                    check(
                        futures::poll!(&mut wait).is_pending(),
                        "Native scheduler wait did not begin before original abort",
                    )?;
                    calling.with_runtime(&factory, &mut |runtime| {
                        bindings.signals().abort(
                            &factory,
                            &controller,
                            Some(original_reason.clone()),
                            runtime,
                        )?;
                        factory.heap().undefined()
                    })?;
                }
                Ok(wait.await)
            })
            .await?;
        let Err(ValueError::Thrown(error)) = outcome else {
            return Err(ValueError::UnsupportedOperation(
                "Scheduler abort became success or lost native exception",
            ));
        };
        check(
            !error.same_identity(&reason)
                && h.get(&error, "cause")?.same_identity(&reason)
                && h.s(&h.get(&error, "name")?)? == "AbortError"
                && h.s(&h.get(&error, "code")?)? == "ABORT_ERR"
                && h.s(&h.get(&error, "message")?)? == "The operation was aborted",
            "Scheduler must create AbortError envelope with original cause, not throw reason",
        )?;
    }
    Ok(())
}

fn accessor(h: &Harness, object: &ValueHandle, key: &str, body: ValueHandle) -> Result<()> {
    use ara_rpc::value_heap::{PropertyDescriptor, ValueSnapshot};
    let ValueSnapshot::Function(target) = h.heap().snapshot(&body)? else {
        return Err(ValueError::InvalidNode);
    };
    h.with(|r| {
        h.heap().define_own(
            object,
            key.into(),
            PropertyDescriptor::Accessor {
                get: Some(target),
                set: None,
                enumerable: false,
                configurable: true,
            },
            r,
        )
    })
}
fn bump(
    heap: &Arc<ara_rpc::value_heap::ValueHeap>,
    state: &ValueHandle,
    key: &str,
    r: &dyn ara_rpc::value_heap::ValueRuntime,
) -> Result<f64> {
    let count = heap.to_number(&heap.get(state, &key.into(), r)?, r)?;
    heap.define_own_data(state, key.into(), heap.number(count + 1.0)?, r)?;
    Ok(count)
}
static RAW_GET: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original getter records its actual Get and returns or throws its original captured value",
    name: "get",
    length: 0,
    capture_count: 4,
    form: CompiledFunctionForm::Builtin,
    body: |heap, c, _, _, r| {
        let key = heap
            .to_string(&c[2], r)?
            .to_utf8()
            .map_err(|_| ValueError::InvalidNode)?;
        bump(heap, &c[0], &key, r)?;
        if heap.truthy(&c[3])? {
            Err(ValueError::Thrown(c[1].clone()))
        } else {
            Ok(c[1].clone())
        }
    },
};
static RAW_FACTORY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original stream factory receives unchanged model context options and returns native stream",
    name: "stream",
    length: 3,
    capture_count: 2,
    form: CompiledFunctionForm::Arrow,
    body: |heap, c, _, args, r| {
        let expected = heap.get(&c[0], &"inputs".into(), r)?;
        for index in 0..3 {
            if !args
                .get(index)
                .ok_or(ValueError::InvalidNode)?
                .same_identity(&heap.get(&expected, &index.to_string().as_str().into(), r)?)
            {
                return Err(ValueError::UnsupportedOperation(
                    "Native factory source input identity changed",
                ));
            }
        }
        Ok(c[1].clone())
    },
};
static RAW_ITERATOR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original async/sync iterator method keeps its source receiver",
    name: "iterator",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::Builtin,
    body: |heap, c, receiver, _, r| {
        if !receiver.same_identity(&heap.get(&c[0], &"source".into(), r)?) {
            return Err(ValueError::InvalidNode);
        }
        bump(heap, &c[0], "iterators", r)?;
        Ok(receiver.clone())
    },
};
static RAW_NEXT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original cached next returns iterator result Promise and replaces next to prove original caching",
    name: "next",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::Builtin,
    body: |heap, c, receiver, _, r| {
        if !receiver.same_identity(&heap.get(&c[0], &"source".into(), r)?) {
            return Err(ValueError::InvalidNode);
        }
        let index = bump(heap, &c[0], "nextCalls", r)?;
        if index == 0.0 {
            heap.define_own_data(
                receiver,
                "next".into(),
                heap.fresh_compiled_function(&THROW, &[heap.get(&c[0], &"primary".into(), r)?])?,
                r,
            )?;
        }
        if heap.truthy(&heap.get(&c[0], &"throwNext".into(), r)?)? {
            return Err(ValueError::Thrown(heap.get(&c[0], &"primary".into(), r)?));
        }
        let events = heap.get(&c[0], &"events".into(), r)?;
        let length = heap.to_number(&heap.get(&events, &"length".into(), r)?, r)?;
        let result = heap.object()?;
        heap.define_own_data(&result, "done".into(), heap.boolean(index >= length)?, r)?;
        let event = if index < length {
            heap.get(&events, &index.to_string().as_str().into(), r)?
        } else {
            heap.undefined()?
        };
        let promise = heap.get(&c[0], &"Promise".into(), r)?;
        let resolve = heap.get(&promise, &"resolve".into(), r)?;
        let synchronous = heap.truthy(&heap.get(&c[0], &"synchronous".into(), r)?)?;
        // Async-from-sync must await the actual yielded Promise value.
        let value = if heap.truthy(&heap.get(&c[0], &"rejectYield".into(), r)?)? {
            heap.call(
                &heap.get(&promise, &"reject".into(), r)?,
                &promise,
                &[heap.get(&c[0], &"primary".into(), r)?],
                r,
            )?
        } else if synchronous {
            heap.call(&resolve, &promise, &[event], r)?
        } else {
            event
        };
        heap.define_own_data(&result, "value".into(), value, r)?;
        if synchronous {
            Ok(result)
        } else {
            heap.call(&resolve, &promise, &[result], r)
        }
    },
};
static RAW_RETURN_SETTLED: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original return Promise continuation records actual settlement before fulfilling",
    name: "",
    length: 1,
    capture_count: 1,
    form: CompiledFunctionForm::Arrow,
    body: |heap, c, _, args, r| {
        bump(heap, &c[0], "returnsSettled", r)?;
        Ok(args.first().cloned().unwrap_or(heap.undefined()?))
    },
};
static RAW_RETURN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original IteratorClose return is called and its original Promise awaited",
    name: "return",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::Builtin,
    body: |heap, c, receiver, _, r| {
        if !receiver.same_identity(&heap.get(&c[0], &"source".into(), r)?) {
            return Err(ValueError::InvalidNode);
        }
        bump(heap, &c[0], "returns", r)?;
        if heap.truthy(&heap.get(&c[0], &"throwReturn".into(), r)?)? {
            return Err(ValueError::Thrown(heap.get(
                &c[0],
                &"secondary".into(),
                r,
            )?));
        }
        let result = heap.object()?;
        heap.define_own_data(&result, "done".into(), heap.boolean(true)?, r)?;
        if heap.truthy(&heap.get(&c[0], &"rejectYield".into(), r)?)? {
            // Bun1.4.0 original sync rejected-yield IteratorClose observation
            // only requires the original return Call and original reason.
            return Ok(result);
        }
        let promise = heap.get(&c[0], &"Promise".into(), r)?;
        let synchronous = heap.truthy(&heap.get(&c[0], &"synchronous".into(), r)?)?;
        let resolved = heap.call(
            &heap.get(&promise, &"resolve".into(), r)?,
            &promise,
            &[if synchronous {
                heap.undefined()?
            } else {
                result.clone()
            }],
            r,
        )?;
        let returned = heap.call(
            &heap.get(&resolved, &"then".into(), r)?,
            &resolved,
            &[heap.fresh_compiled_function(&RAW_RETURN_SETTLED, &[c[0].clone()])?],
            r,
        )?;
        if synchronous {
            heap.define_own_data(&result, "value".into(), returned, r)?;
            Ok(result)
        } else {
            Ok(returned)
        }
    },
};
static RAW_RESULT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original result method retains receiver and returns original result Promise only at EOF",
    name: "result",
    length: 0,
    capture_count: 2,
    form: CompiledFunctionForm::Builtin,
    body: |heap, c, receiver, _, r| {
        if !receiver.same_identity(&heap.get(&c[0], &"source".into(), r)?) {
            return Err(ValueError::InvalidNode);
        }
        bump(heap, &c[0], "results", r)?;
        let promise = heap.get(&c[0], &"Promise".into(), r)?;
        heap.call(
            &heap.get(&promise, &"resolve".into(), r)?,
            &promise,
            &[c[1].clone()],
            r,
        )
    },
};
fn raw_getter(
    h: &Harness,
    state: &ValueHandle,
    object: &ValueHandle,
    key: &str,
    counter: &str,
    value: ValueHandle,
    throws: bool,
) -> Result<()> {
    accessor(
        h,
        object,
        key,
        h.heap().fresh_compiled_function(
            &RAW_GET,
            &[
                state.clone(),
                value,
                h.heap().string(counter.into())?,
                h.heap().boolean(throws)?,
            ],
        )?,
    )
}
async fn native_stream_source_contract(
    h: &mut Harness,
    module: Arc<ReplaySafeStreamModule>,
) -> Result<()> {
    use ara_ai::retry::stream::NativeReplaySafeCompletionAttempt;
    for case in 0..6 {
        let source = h.heap().object()?;
        let model = h.heap().object()?;
        let context = h.heap().object()?;
        let options = h.heap().object()?;
        let root = assistant(h, "hello", false)?;
        let primary = error(h, None, "original native event/next failure")?;
        let secondary = error(h, None, "original native return failure")?;
        let state = h.object(&[
            ("source", source.clone()),
            (
                "inputs",
                h.heap()
                    .array(vec![model.clone(), context.clone(), options.clone()])?,
            ),
            ("iterators", h.heap().number(0.0)?),
            ("nextGets", h.heap().number(0.0)?),
            ("nextCalls", h.heap().number(0.0)?),
            ("returns", h.heap().number(0.0)?),
            ("returnsSettled", h.heap().number(0.0)?),
            ("results", h.heap().number(0.0)?),
            ("typeGets", h.heap().number(0.0)?),
            ("deltaGets", h.heap().number(0.0)?),
            ("beforeGets", h.heap().number(0.0)?),
            ("afterGets", h.heap().number(0.0)?),
            ("Promise", h.get(&h.bindings.globals(), "Promise")?),
            ("throwNext", h.heap().boolean(case == 3)?),
            ("throwReturn", h.heap().boolean(case == 2)?),
            ("synchronous", h.heap().boolean(matches!(case, 4 | 5))?),
            ("rejectYield", h.heap().boolean(case == 5)?),
            ("primary", primary.clone()),
            ("secondary", secondary),
        ])?;
        let before = h.object(&[("privateMetadata", h.heap().object()?)])?;
        let after = h.heap().object()?;
        for (event, counter) in [(&before, "beforeGets"), (&after, "afterGets")] {
            raw_getter(
                h,
                &state,
                event,
                "type",
                counter,
                h.heap().string("custom_marker".into())?,
                false,
            )?;
            accessor(
                h,
                event,
                "toJSON",
                h.heap()
                    .fresh_compiled_function(&THROW, &[primary.clone()])?,
            )?;
        }
        let delta = h.heap().object()?;
        raw_getter(
            h,
            &state,
            &delta,
            "type",
            "typeGets",
            h.heap().string("text_delta".into())?,
            false,
        )?;
        raw_getter(
            h,
            &state,
            &delta,
            "delta",
            "deltaGets",
            if case == 2 {
                primary.clone()
            } else {
                h.heap().string("hello".into())?
            },
            case == 2,
        )?;
        let terminal = h.object(&[
            ("type", h.heap().string("done".into())?),
            ("reason", h.heap().string("stop".into())?),
            ("message", root.clone()),
        ])?;
        let originals = if case == 1 {
            vec![before.clone()]
        } else if case == 2 {
            vec![delta.clone()]
        } else {
            vec![
                before.clone(),
                delta.clone(),
                after.clone(),
                terminal.clone(),
            ]
        };
        h.put(&state, "events", h.heap().array(originals.clone())?)?;
        let iterator = h
            .heap()
            .fresh_compiled_function(&RAW_ITERATOR, &[state.clone()])?;
        h.with(|r| {
            h.heap().define_own_data(
                &source,
                h.bindings.async_iterator_key(),
                if matches!(case, 4 | 5) {
                    h.heap().null()?
                } else {
                    iterator.clone()
                },
                r,
            )
        })?;
        h.with(|r| {
            h.heap()
                .define_own_data(&source, h.heap().iterator_key(), iterator, r)
        })?;
        let next = h
            .heap()
            .fresh_compiled_function(&RAW_NEXT, &[state.clone()])?;
        raw_getter(h, &state, &source, "next", "nextGets", next, false)?;
        h.put(
            &source,
            "return",
            h.heap()
                .fresh_compiled_function(&RAW_RETURN, &[state.clone()])?,
        )?;
        h.put(
            &source,
            "result",
            h.heap()
                .fresh_compiled_function(&RAW_RESULT, &[state.clone(), root.clone()])?,
        )?;
        let callable = h
            .heap()
            .fresh_compiled_function(&RAW_FACTORY, &[state.clone(), source])?;
        let attempt = Arc::new(NativeReplaySafeCompletionAttempt::prepare(
            h.factory.clone(),
            h.bindings.clone(),
            callable,
        )?);
        let (mut stream, work) = with_replay_safe_stream_retry(
            h.factory.clone(),
            h.bindings.clone(),
            module.clone(),
            model,
            context,
            Some(options),
            attempt,
            h.data(serde_json::json!({"retryEmptyCompletion":true}))?,
        )?;
        let result = stream.original_result.clone();
        let consumption = async move {
            let mut events = Vec::new();
            while let Some(event) = stream.recv_original_event().await? {
                events.push(event);
            }
            Ok::<_, ValueError>(events)
        };
        let (work, events, result) = h
            .run_until(async move {
                let (work, events) = futures::future::join(work, consumption).await;
                Ok((work, events, result.result().await))
            })
            .await?;
        work?;
        check(
            h.n(&h.get(&state, "iterators")?)? == 1.0 && h.n(&h.get(&state, "nextGets")?)? == 1.0,
            "Original native iterator receiver/next getter caching changed",
        )?;
        if matches!(case, 2 | 3 | 5) {
            check(
                matches!(&result,Err(ValueError::Thrown(v))if v.same_identity(&primary)),
                "Original native primary error identity lost to return/projection",
            )?;
            check(
                matches!(&events,Err(ValueError::Thrown(v))if v.same_identity(&primary)),
                "Original native iteration error did not reach raw consumer",
            )?;
            check(
                h.n(&h.get(&state, "returns")?)? == if case == 3 { 0.0 } else { 1.0 },
                "Body error/IteratorNext abrupt close behavior changed",
            )?;
        } else {
            let events = events?;
            check(
                events.len() == originals.len()
                    && events
                        .iter()
                        .zip(&originals)
                        .all(|(a, b)| a.same_identity(b)),
                "Native unknown/getter events were projected, lost or reordered",
            )?;
            check(
                result?.same_identity(&root),
                "Native terminal/EOF original result identity changed",
            )?;
            check(
                h.n(&h.get(&state, "returns")?)? == if case == 1 { 0.0 } else { 1.0 },
                "Native normal EOF/terminal IteratorClose changed",
            )?;
            check(
                h.n(&h.get(&state, "returnsSettled")?)? == if case == 1 { 0.0 } else { 1.0 },
                "Original terminal return Promise was not actually awaited",
            )?;
            // Full fixed pipeline: retry utility 3 or 2 Gets plus outer.push
            // error check / isComplete done+error Gets (three) for each event.
            check(
                h.n(&h.get(&state, "beforeGets")?)? == 6.0,
                "Buffered unknown event source+outer getter read sequence changed",
            )?;
            if case != 1 {
                check(
                    h.n(&h.get(&state, "afterGets")?)? == 5.0
                        && h.n(&h.get(&state, "typeGets")?)? == 6.0
                        && h.n(&h.get(&state, "deltaGets")?)? == 1.0,
                    "Committed/raw-delta fixed pipeline getter observations changed",
                )?;
            }
        }
        check(
            h.n(&h.get(&state, "results")?)? == if case == 1 { 1.0 } else { 0.0 },
            "Original native result method called outside sole EOF fallback",
        )?;
    }
    Ok(())
}

// Reuse the existing native iterator/Promise script, not a new Host driver.
fn upgraded_raw_stream(
    h: &Harness,
    root: &ValueHandle,
    events: Vec<ValueHandle>,
) -> Result<ActualCompletionStream> {
    let source = h.heap().object()?;
    let state = h.object(&[
        ("source", source.clone()),
        ("events", h.heap().array(events)?),
        ("iterators", h.heap().number(0.0)?),
        ("nextCalls", h.heap().number(0.0)?),
        ("returns", h.heap().number(0.0)?),
        ("returnsSettled", h.heap().number(0.0)?),
        ("results", h.heap().number(0.0)?),
        ("Promise", h.get(&h.bindings.globals(), "Promise")?),
    ])?;
    let iterator = h.heap().fresh_compiled_function(&RAW_ITERATOR, &[state.clone()])?;
    h.with(|runtime| {
        h.heap().define_own_data(&source, h.bindings.async_iterator_key(), iterator, runtime)
    })?;
    h.put(&source, "next", h.heap().fresh_compiled_function(&RAW_NEXT, &[state.clone()])?)?;
    h.put(&source, "return", h.heap().fresh_compiled_function(&RAW_RETURN, &[state.clone()])?)?;
    h.put(&source, "result", h.heap().fresh_compiled_function(&RAW_RESULT, &[state, root.clone()])?)?;
    ActualCompletionStream::from_native(&h.factory, &h.bindings, source)
}

async fn upgraded_marker_stream(
    h: &Harness,
    root: &ValueHandle,
    call: &ValueHandle,
    delta: Option<&str>,
    failed: bool,
    original: bool,
) -> Result<ActualCompletionStream> {
    let terminal = if failed { "error" } else { "done" };
    let kinds = if delta.is_some() {
        vec!["start", "toolcall_start", "toolcall_delta", "toolcall_end", terminal]
    } else {
        vec!["start", terminal]
    };
    let index = h.heap().number(0.0)?;
    if original {
        // Authored original event records retain the same terminal/partial
        // handles; no production typed-to-native projection is involved.
        let mut events = Vec::new();
        for kind in kinds {
            let event = h.object(&[("type", h.heap().string(kind.into())?)])?;
            if kind == "done" || kind == "error" {
                h.put(&event, "reason", h.heap().string(if failed { "error" } else { "stop" }.into())?)?;
                h.put(&event, if failed { "error" } else { "message" }, root.clone())?;
            } else {
                h.put(&event, "partial", root.clone())?;
                h.put(&event, "contentIndex", index.clone())?;
                if kind == "toolcall_delta" {
                    h.put(&event, "delta", h.heap().string(delta.ok_or(ValueError::InvalidNode)?.into())?)?;
                } else if kind == "toolcall_end" {
                    h.put(&event, "toolCall", call.clone())?;
                }
            }
            events.push(event);
        }
        return upgraded_raw_stream(h, root, events);
    }
    let (mut producer, stream, _) = ActualCompletionStream::channel(&h.factory);
    for kind in kinds {
        let event = match kind {
            "start" => Event::Start { partial: root.clone() },
            "toolcall_start" => Event::ToolcallStart { content_index: index.clone(), partial: root.clone() },
            "toolcall_delta" => Event::ToolcallDelta {
                content_index: index.clone(),
                delta: h.heap().string(delta.ok_or(ValueError::InvalidNode)?.into())?,
                partial: root.clone(),
            },
            "toolcall_end" => Event::ToolcallEnd {
                content_index: index.clone(), tool_call: call.clone(), partial: root.clone(),
            },
            "error" => Event::Error { reason: StopReason::Error, error: root.clone() },
            "done" => Event::Done { reason: StopReason::Stop, message: root.clone() },
            _ => return Err(ValueError::InvalidNode),
        };
        producer.push(event).await?;
    }
    drop(producer);
    Ok(stream)
}

static UPGRADED_STOP_DETAILS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP b91db66e empty retry independently Gets stopDetails for pause_turn then compaction",
    name: "get stopDetails",
    length: 0,
    capture_count: 3,
    form: CompiledFunctionForm::Builtin,
    body: |heap, captures, _, _, runtime| {
        let count = bump(heap, &captures[0], "stopDetailsGets", runtime)?;
        Ok(captures[if count == 0.0 { 1 } else { 2 }].clone())
    },
};

#[tokio::test]
async fn upgraded_commitment_and_compaction_group() {
    use ara_ai::retry::result::CompletionEvent;
    let mut h = Harness::new().expect("existing native Retry imports");
    let (_, module) = modules(&h).expect("existing source retry module");
    // Both event carriers exercise the native utility's distinct meaningful
    // branches. Published providers have separate existing HTTP filters.
    for original in [false, true] {
        for case in 0..5 {
            let state = h.object(&[
                ("waits", h.heap().array(Vec::new()).unwrap()),
                ("Promise", h.get(&h.bindings.globals(), "Promise").unwrap()),
                ("stopDetailsGets", h.heap().number(0.0).unwrap()),
            ]).unwrap();
            let options = h.heap().object().unwrap();
            h.put(&options, "providerRetryWait", h.heap().fresh_compiled_function(&WAIT, &[
                state.clone(), h.heap().undefined().unwrap(), h.heap().boolean(false).unwrap(),
                h.heap().undefined().unwrap(),
            ]).unwrap()).unwrap();
            let failed = matches!(case, 0 | 2);
            let root = assistant(&h, "", failed).unwrap();
            let call = h.data(serde_json::json!({"type":"toolCall","id":"call-1","name":"read",
                "arguments": if case == 1 { serde_json::json!({"path":"/x"}) } else { serde_json::json!({}) }
            })).unwrap();
            if matches!(case, 1 | 2) {
                h.put(&root, "content", h.heap().array(vec![call.clone()]).unwrap()).unwrap();
            }
            if case == 3 {
                h.put(&root, "stopDetails", h.data(serde_json::json!({"type":"compaction"})).unwrap()).unwrap();
            } else if case == 4 {
                // A cached single Get would miss the second compaction value
                // and retry; preserve the original short-circuit read order.
                accessor(&h, &root, "stopDetails", h.heap().fresh_compiled_function(
                    &UPGRADED_STOP_DETAILS, &[
                        state.clone(),
                        h.data(serde_json::json!({"type":"other"})).unwrap(),
                        h.data(serde_json::json!({"type":"compaction"})).unwrap(),
                    ],
                ).unwrap()).unwrap();
            }
            let delta = match case {
                0 => Some(""), 1 => Some("{\"path\":\"/x\"}"), 2 => Some("{}"), _ => None,
            };
            let mut sources = VecDeque::from([Ok(upgraded_marker_stream(
                &h, &root, &call, delta, failed, original,
            ).await.unwrap())]);
            let recovered = assistant(&h, "hello", false).unwrap();
            if case == 0 {
                let recovery = if original {
                    let index = h.heap().number(0.0).unwrap();
                    let mut events = Vec::new();
                    for kind in ["start", "text_start", "text_delta", "text_end", "done"] {
                        let event = h.object(&[("type", h.heap().string(kind.into()).unwrap())]).unwrap();
                        if kind == "done" {
                            h.put(&event, "reason", h.heap().string("stop".into()).unwrap()).unwrap();
                            h.put(&event, "message", recovered.clone()).unwrap();
                        } else {
                            h.put(&event, "partial", recovered.clone()).unwrap();
                            h.put(&event, "contentIndex", index.clone()).unwrap();
                            if kind == "text_delta" || kind == "text_end" {
                                h.put(&event, if kind == "text_delta" { "delta" } else { "content" },
                                    h.heap().string("hello".into()).unwrap()).unwrap();
                            }
                        }
                        events.push(event);
                    }
                    upgraded_raw_stream(&h, &recovered, events).unwrap()
                } else {
                    source_stream(&h, &recovered, 1).await.unwrap()
                };
                sources.push_back(Ok(recovery));
            }
            let attempts = Arc::new(Streams { source: Mutex::new(sources), calls: AtomicUsize::new(0) });
            let (mut outer, work) = with_replay_safe_stream_retry(
                h.factory.clone(), h.bindings.clone(), module.clone(),
                h.heap().object().unwrap(), h.heap().object().unwrap(), Some(options), attempts.clone(),
                h.data(serde_json::json!({"retryEmptyCompletion":true,"retryProviderErrors":true,
                    "maxProviderErrorRetries":1})).unwrap(),
            ).unwrap();
            let result = outer.original_result.clone();
            // Read the selected carrier explicitly, retaining original event
            // roots until after its forward future settles.
            let consumption = async move {
                let mut events = Vec::new();
                if original {
                    while let Some(event) = outer.recv_original_event().await? {
                        events.push(CompletionEvent::Original(event));
                    }
                } else {
                    while let Some(event) = outer.recv().await? {
                        events.push(CompletionEvent::Actual(event));
                    }
                }
                Ok::<_, ValueError>(events)
            };
            let (work, events, result) = h.run_until(async move {
                let (work, events) = futures::future::join(work, consumption).await;
                Ok((work, events, result.result().await))
            }).await.unwrap();
            work.unwrap();
            let events = events.unwrap();
            let result = result.unwrap();
            let kinds = events.iter().map(|event| match event {
                CompletionEvent::Actual(event) => event.type_name().to_owned(),
                CompletionEvent::Original(event) => h.s(&h.get(event, "type").unwrap()).unwrap(),
            }).collect::<Vec<_>>();
            assert_eq!(attempts.calls.load(Ordering::SeqCst), if case == 0 { 2 } else { 1 },
                "original={original} case={case}");
            assert_eq!(h.n(&h.get(&h.get(&state, "waits").unwrap(), "length").unwrap()).unwrap(),
                if case == 0 { 1.0 } else { 0.0 });
            if case == 0 {
                assert_eq!(kinds, ["start", "text_start", "text_delta", "text_end", "done"]);
                assert!(result.same_identity(&recovered));
                assert_eq!(h.s(&h.get(&h.get(&h.get(&result, "content").unwrap(), "0").unwrap(), "text").unwrap()).unwrap(), "hello");
            } else if case <= 2 {
                assert_eq!(kinds, ["start", "toolcall_start", "toolcall_delta", "toolcall_end",
                    if case == 2 { "error" } else { "done" }]);
                assert!(result.same_identity(&root));
                assert_eq!(h.s(&h.get(&result, "stopReason").unwrap()).unwrap(),
                    if case == 2 { "error" } else { "stop" });
            } else {
                assert_eq!(kinds, ["start", "done"]);
                assert!(result.same_identity(&root));
                if case == 4 {
                    assert_eq!(h.n(&h.get(&state, "stopDetailsGets").unwrap()).unwrap(), 2.0);
                }
            }
            eprintln!("ARA-RETRY UPGRADE b91db66e PASS original={original} case={case}");
        }
    }
    eprintln!("ARA-RETRY UPGRADE b91db66e commitment-compaction COMPLETE");
}


// b91db66e fetch-retry.ts:79-248 and ai/utils/retry-after.ts:7-12.
// These source consumers extend the existing coarse source_groups target.
static HINT_CLOCK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original Date.now observes its Date receiver and returns the pinned clock",
    name: "now", length: 0, capture_count: 2, form: CompiledFunctionForm::Builtin,
    body: |heap, c, receiver, _, r| {
        if !receiver.same_identity(&c[0]) { return Err(ValueError::InvalidNode); }
        bump(heap, &c[1], "nowCalls", r)?;
        heap.get(&c[1], &"clock".into(), r)
    },
};
static HINT_PARSE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original Date.parse records Get/Call order and delegates to its actual import",
    name: "parse", length: 1, capture_count: 3, form: CompiledFunctionForm::Builtin,
    body: |heap, c, receiver, args, r| {
        if !receiver.same_identity(&c[0]) { return Err(ValueError::InvalidNode); }
        bump(heap, &c[1], "parseCalls", r)?;
        heap.define_own_data(&c[1], "parseArgument".into(), args.first().cloned().unwrap_or(heap.undefined()?), r)?;
        heap.call(&c[2], receiver, args, r)
    },
};
static HINT_HEADER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original Headers-like get records ordered keys and returns the original entry",
    name: "get", length: 1, capture_count: 2, form: CompiledFunctionForm::Builtin,
    body: |heap, c, receiver, args, r| {
        if !receiver.same_identity(&c[0]) { return Err(ValueError::InvalidNode); }
        let key = args.first().ok_or(ValueError::InvalidNode)?;
        let seen = heap.get(&c[0], &"seen".into(), r)?;
        heap.call(&heap.get(&seen, &"push".into(), r)?, &seen, &[key.clone()], r)?;
        let key = heap.to_string(key, r)?;
        let value = heap.get(&c[1], &key.into(), r)?;
        if heap.kind(&value)? == ara_rpc::value_heap::ValueKind::Undefined { heap.null() } else { Ok(value) }
    },
};
fn hint_property_descriptor(h: &Harness, object: &ValueHandle, key: &str) -> Result<ara_rpc::value_heap::PropertyDescriptor> {
    h.with(|r| h.heap().own_descriptor(object, &key.into(), r))?
        .ok_or(ValueError::UnsupportedOperation("Original Date own descriptor missing"))
}
fn hint_restore_descriptor(h: &Harness, object: &ValueHandle, key: &str, descriptor: &ara_rpc::value_heap::PropertyDescriptor) -> Result<()> {
    h.with(|r| h.heap().define_own(object, key.into(), descriptor.clone(), r))
}
fn hint_raw_get(h: &Harness, stats: &ValueHandle, key: &str, result: ValueHandle, throws: bool) -> Result<ValueHandle> {
    h.heap().fresh_compiled_function(&RAW_GET, &[stats.clone(), result, h.heap().string(key.into())?, h.heap().boolean(throws)?])
}
fn hint_extract(h: &Harness, module: &OneshotModule, source: Option<ValueHandle>, body: &str, options: Option<ValueHandle>) -> Result<ValueHandle> {
    h.with(|r| h.heap().call(&module.extract_retry_hint, &h.heap().undefined()?, &[source.unwrap_or(h.heap().undefined()?), h.heap().string(body.into())?, options.unwrap_or(h.heap().undefined()?)], r))
}
fn hint_number(h: &Harness, value: &ValueHandle, expected: Option<f64>) -> Result<()> {
    match expected {
        Some(expected) => check(matches!(h.heap().snapshot(value)?, ara_rpc::value_heap::ValueSnapshot::Number(actual) if actual == expected || (actual.is_nan() && expected.is_nan())), "Native retry hint numeric outcome differs from fixed source"),
        None => check(h.heap().kind(value)? == ara_rpc::value_heap::ValueKind::Undefined, "Native retry hint lost source undefined"),
    }
}
fn hint_headers(h: &Harness, entries: &[(&str, &str)]) -> Result<(ValueHandle, ValueHandle)> {
    let headers = h.object(&[("seen", h.heap().array(Vec::new())?)])?;
    let values = h.object(&entries.iter().map(|(key, value)| Ok((*key, h.heap().string((*value).into())?))).collect::<Result<Vec<_>>>()?)?;
    h.put(&headers, "get", h.heap().fresh_compiled_function(&HINT_HEADER, &[headers.clone(), values])?)?;
    Ok((h.object(&[("headers", headers.clone())])?, headers))
}
async fn upgraded_hint_source_contract(h: &mut Harness, module: Arc<OneshotModule>) -> Result<()> {
    let date = h.get(&h.bindings.globals(), "Date")?;
    let now_descriptor = hint_property_descriptor(h, &date, "now")?;
    let parse_descriptor = hint_property_descriptor(h, &date, "parse")?;
    let original_parse = h.get(&date, "parse")?;
    let clock = h.with(|r| h.heap().call(&original_parse, &date, &[h.heap().string("2099-09-01T00:00:00Z".into())?], r))?;
    let now = h.n(&clock)?;
    let stats = h.data(serde_json::json!({"nowCalls":0,"nowGets":0,"parseCalls":0,"parseGets":0,"optionGets":0}))?;
    h.put(&stats, "clock", clock)?;
    let now_fn = h.heap().fresh_compiled_function(&HINT_CLOCK, &[date.clone(), stats.clone()])?;
    let parse_fn = h.heap().fresh_compiled_function(&HINT_PARSE, &[date.clone(), stats.clone(), original_parse.clone()])?;
    accessor(h, &date, "now", hint_raw_get(h, &stats, "nowGets", now_fn, false)?)?;
    accessor(h, &date, "parse", hint_raw_get(h, &stats, "parseGets", parse_fn, false)?)?;
    let result = async {
        check(h.n(&h.get(&module.extract_retry_hint, "length")?)? == 3.0, "Raw extractor Function.length must be three")?;
        // Every fixed upstream extractRetryHint test is represented below;
        // clock-sensitive assertions use the actual Native Date.parse import.
        let cases: &[(&str, &str, Option<f64>)] = &[
            ("devin13min", "Your limit will reset in 13 minutes", Some(780_000.0)),
            ("reset2h", "will reset in 2h", Some(7_200_000.0)),
            ("opencode45min", "429 5-hour usage limit reached. Resets in 45min. To continue using this model now, enable usage from your available balance: https://opencode.ai/workspace/wrk_1/go", Some(2_700_000.0)),
            ("opencode2hr15min", "429 5-hour usage limit reached. Resets in 2hr 15min. To continue using this model now, enable usage from your available balance: https://opencode.ai/workspace/wrk_1/go", Some(8_100_000.0)),
            ("days3", "429 Weekly usage limit reached. Resets in 3 days.", Some(259_200_000.0)),
            ("day1", "429 Monthly usage limit reached. Resets in 1 day.", Some(86_400_000.0)),
            ("account-longer", "Please retry in 5s. Your limit will reset in 13 minutes", Some(780_000.0)),
            ("appended-longer", "429 quota exceeded. Your limit will reset in 5 minutes. retry-after-ms=3600000", Some(3_600_000.0)),
            ("legacy-longer", "429 quota exceeded. reset in 5 minutes; retry-after=3600", Some(3_600_000.0)),
            ("legacy-epoch", "rate limited, x-ratelimit-reset=9999999999", Some(9_999_999_999_000.0 - now)),
            ("body-ms", "429 {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"code\":\"1310\"}} retry-after-ms=98497000", Some(98_497_000.0)),
            ("colon-ms", "429 quota exceeded. retry-after-ms: 7200000", Some(7_200_000.0)),
            ("spaced-ms", "429 quota exceeded. retry-after-ms = 7200000", Some(7_200_000.0)),
            ("colon-longer", "429 quota exceeded. reset in 5 minutes. retry-after-ms: 3600000", Some(3_600_000.0)),
            ("naive-space", "Your limit will reset at 2099-09-01 09:44:51", Some(35_091_000.0)),
            ("naive-iso", "Your limit will reset at 2099-09-01T09:44:51", Some(35_091_000.0)),
            ("explicit-offset", "Your limit will reset at 2099-09-01 09:44:51+08:00", Some(6_291_000.0)),
            ("absolute-longer", "Your limit will reset at 2099-09-01T01:00:00Z retry-after-ms=5000", Some(3_600_000.0)),
            ("relative-over-longer-naive", "Your limit will reset at 2099-09-01 01:00:00 retry-after-ms=5000", Some(5000.0)),
            ("cn-utc", "已达到使用上限。您的限额将在 2099-09-01 09:44:51 重置。", Some(35_091_000.0)),
            ("explicit-zero-ms", "429 quota exceeded. retry-after-ms=0", Some(0.0)),
            ("explicit-zero-legacy", "rate limited; retry-after=0", Some(0.0)),
            ("reset-over-zero", "429 quota exceeded. reset in 5 minutes. retry-after-ms=0", Some(300_000.0)),
            ("elapsed-account", "Your limit will reset at 2099-08-31T23:59:00Z", None),
            ("quota-hms", "Your quota will reset after 1h2m3s", Some(3_723_000.0)),
            ("retry-ms", "Please retry in 250ms", Some(250.0)),
            ("retry-field", "{\"retryDelay\": \"34.074824224s\"}", Some(34.074824224_f64 * 1000.0)),
            ("again-min", "try again in ~158 min.", Some(9_480_000.0)),
            ("legacy-reset-delta", "x-ratelimit-reset=12", Some(12_000.0)),
            ("legacy-reset-ms-delta", "x-ratelimit-reset-ms=345", Some(345.0)),
            ("legacy-date", "retry-after=2099-09-01T00:00:02Z", Some(2000.0)),
            ("naive-over-zero", "reset at 2099-09-01 01:00:00 retry-after-ms=0", Some(3_600_000.0)),
            ("unknown", "rate limited without timing", None),
        ];
        for (label, body, expected) in cases {
            eprintln!("ARA-RETRY-UPGRADE HINT BEGIN {label}");
            hint_number(h, &hint_extract(h, &module, None, body, None)?, *expected)?;
            eprintln!("ARA-RETRY-UPGRADE HINT PASS {label}");
        }
        for body in [format!("rate limited, x-ratelimit-reset-ms={}", now - 60_000.0), format!("rate limited, x-ratelimit-reset={}", now / 1000.0 - 60.0)] {
            hint_number(h, &hint_extract(h, &module, None, &body, None)?, Some(0.0))?;
        }
        for (body, offset, expected) in [
            ("已达到使用上限。您的限额将在 2099-09-01 09:44:51 重置。", "+08:00", 6_291_000.0),
            ("Your limit will reset at 2099-09-01 09:00:00 retry-after-ms=5000", "+08:00", 3_600_000.0),
            ("Your limit will reset at 2099-09-01T01:00:00Z", "garbage", 3_600_000.0),
        ] {
            hint_number(h, &hint_extract(h, &module, None, body, Some(h.object(&[("naiveResetTimezoneOffset", h.heap().string(offset.into())?)])?))?, Some(expected))?;
        }
        // An explicit empty offset retains actual local Date.parse semantics.
        let local = h.with(|r| h.heap().call(&original_parse, &date, &[h.heap().string("2099-09-01T23:00:00".into())?], r))?;
        hint_number(h, &hint_extract(h, &module, None, "reset at 2099-09-01 23:00:00 retry-after-ms=1", Some(h.object(&[("naiveResetTimezoneOffset", h.heap().string("".into())?)])?))?, Some(h.n(&local)? - now))?;

        // Ordered headers: each source branch wins before consulting body.
        for (entries, expected, gets) in [
            (vec![("retry-after-ms", "12"), ("retry-after", "99")], 12.0, 1),
            (vec![("retry-after-ms", "0")], 0.0, 1),
            (vec![("retry-after-ms", "bad"), ("retry-after", "2")], 2000.0, 2),
            (vec![("retry-after", "-1")], 0.0, 2),
            (vec![("retry-after", "2099-09-01T00:00:01Z")], 1000.0, 2),
            (vec![("x-ratelimit-reset-ms", "250")], 250.0, 3),
            (vec![("x-ratelimit-reset-after", "3")], 3000.0, 5),
        ] {
            let (source, headers) = hint_headers(h, &entries)?;
            hint_number(h, &hint_extract(h, &module, Some(source), "retry-after-ms=99999", None)?, Some(expected))?;
            check(h.n(&h.get(&h.get(&headers, "seen")?, "length")?)? == gets as f64, "Header ordered early exit changed")?;
        }
        for (name, value, expected) in [
            ("x-ratelimit-reset-ms", (now + 2000.0).to_string(), 2000.0),
            ("x-ratelimit-reset-ms", (now / 1000.0 + 2.0).to_string(), 2000.0),
            ("x-ratelimit-reset", (now / 1000.0 + 2.0).to_string(), 2000.0),
            ("x-ratelimit-reset-ms", (now - 2000.0).to_string(), 99999.0),
        ] {
            let (source, _) = hint_headers(h, &[(name, &value)])?;
            hint_number(h, &hint_extract(h, &module, Some(source), "retry-after-ms=99999", None)?, Some(expected))?;
        }
        for key in ["nowGets", "nowCalls", "parseGets", "parseCalls"] { h.put(&stats, key, h.heap().number(0.0)?)?; }
        hint_number(h, &hint_extract(h, &module, None, "reset at 2099-09-01T01:00:00Z", None)?, Some(3_600_000.0))?;
        check(h.n(&h.get(&stats, "parseGets")?)? == 1.0 && h.n(&h.get(&stats, "parseCalls")?)? == 1.0 && h.n(&h.get(&stats, "nowGets")?)? == 2.0 && h.n(&h.get(&stats, "nowCalls")?)? == 2.0, "Date parse/now original receiver and repeated Get/Call changed")?;

        // A patched clock's NaN must survive Math.max, rather than become zero.
        h.put(&stats, "clock", h.heap().number(f64::NAN)?)?;
        let (source, _) = hint_headers(h, &[("retry-after", "2099-09-01T00:00:01Z")])?;
        hint_number(h, &hint_extract(h, &module, Some(source), "", None)?, Some(f64::NAN))?;
        h.put(&stats, "clock", h.heap().number(now)?)?;

        // options is read even for explicit offset; throw identity is original.
        let marker = h.heap().object()?;
        let options = h.heap().object()?;
        accessor(h, &options, "naiveResetTimezoneOffset", hint_raw_get(h, &stats, "optionGets", marker.clone(), true)?)?;
        let failure = hint_extract(h, &module, None, "reset at 2099-09-01T01:00:00Z", Some(options));
        check(matches!(failure, Err(ValueError::Thrown(ref value)) if value.same_identity(&marker)), "Offset getter exception lost identity")?;
        for property in ["headers", "get"] {
            let object = h.heap().object()?;
            accessor(h, &object, property, h.heap().fresh_compiled_function(&THROW, &[marker.clone()])?)?;
            let source = if property == "headers" { object } else { h.object(&[("headers", object)])? };
            let failure = hint_extract(h, &module, Some(source), "retry-after-ms=5", None);
            check(matches!(failure, Err(ValueError::Thrown(ref value)) if value.same_identity(&marker)), "Original header getter exception lost identity")?;
        }
        for object_symbol in [false, true] {
            for key in ["nowGets", "nowCalls", "parseGets", "parseCalls"] { h.put(&stats, key, h.heap().number(0.0)?)?; }
            let symbol = h.heap().symbol(Some("offset".into()))?;
            let offset = if object_symbol { h.object(&[("toString", h.heap().fresh_compiled_function(&RETURN, &[symbol])?)])? } else { symbol };
            let failure = hint_extract(h, &module, None, "reset at 2099-09-01 01:00:00", Some(h.object(&[("naiveResetTimezoneOffset", offset)])?));
            check(matches!(failure, Err(ValueError::TypeError(_))), "Template literal Symbol must fail original ToString")?;
            check(h.n(&h.get(&stats, "parseGets")?)? == 1.0 && h.n(&h.get(&stats, "parseCalls")?)? == 0.0 && h.n(&h.get(&stats, "nowGets")?)? == 0.0, "Date.parse Get must precede template argument throw, without Call/Date.now")?;
        }
        // The same native oneshot owner must use the exact provider policy.
        // Generic six-hour fallback exceeds cap; Beijing providers read past
        // timestamps as no hint and complete their one-millisecond retry.
        for (provider, expected_calls) in [("test", 1.0), ("zai", 2.0), ("zhipu-coding-plan", 2.0), ("ZAI", 1.0)] {
            let calls = state(h)?;
            let failed = message(h, Some(429.0), Some("rate_limit_error: reset at 2099-09-01 06:00:00"), "")?;
            let success = message(h, None, None, "done")?;
            let opts = h.data(serde_json::json!({"provider":provider,"baseDelayMs":1,"maxAttempts":2,"maxDelayMs":10_800_000}))?;
            let original = attempt(h, &calls, &[failed.clone(), success.clone()], &[false, false])?;
            let future = retry_transient_completion(h.factory.clone(), h.bindings.clone(), module.clone(), original, Some(opts));
            let result = h.run_until(future).await?;
            check(h.n(&h.get(&calls, "calls")?)? == expected_calls && result.same_identity(if expected_calls == 1.0 { &failed } else { &success }), "Provider reset timezone did not reach actual native oneshot dispatch")?;
        }
        eprintln!("ARA-RETRY-UPGRADE SOURCE PASS native-hint-headers-body-options-policy");
        Ok(())
    }.await;
    let restore_now = hint_restore_descriptor(h, &date, "now", &now_descriptor);
    let restore_parse = hint_restore_descriptor(h, &date, "parse", &parse_descriptor);
    result?;
    restore_now?;
    restore_parse?;
    Ok(())
}
