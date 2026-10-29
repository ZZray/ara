//! Original issue4593 and pre-response consumers at fixed OMP596f2da.
use super::{Harness, Result};
use ara_ai::{
    event::LocalWorkSource,
    retry::{
        AbortControllerRef,
        idle::{IdleIterator, arm_pre_response_timeout},
    },
};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueKind,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

fn begin(id: &str) {
    eprintln!("ARA-RETRY41 BEGIN {id}");
}
fn pass(id: &str) {
    eprintln!("ARA-RETRY41 PASS {id}");
}
fn check(value: bool, message: &'static str) -> Result<()> {
    if value {
        Ok(())
    } else {
        Err(ValueError::UnsupportedOperation(message))
    }
}
fn controller(h: &Harness) -> Result<AbortControllerRef> {
    h.with(|r| h.bindings.signals().controller(&h.factory, r))
}
fn aborted(h: &Harness, signal: &ValueHandle) -> Result<bool> {
    h.heap().truthy(&h.get(signal, "aborted")?)
}
fn callback(h: &Harness, body: Arc<dyn Fn() -> Result<()> + Send + Sync>) -> Result<ValueHandle> {
    h.with(|r| h.bindings.signals().callback(&h.factory, body, r))
}

static SELF: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original issue4593 generator asyncIterator receiver",
    name: "",
    length: 0,
    capture_count: 0,
    form: CompiledFunctionForm::Builtin,
    body: |_, _, receiver, _, _| Ok(receiver.clone()),
};
static WORK_CONTINUATION: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original source await workDone then busy=false and second yield",
    name: "",
    length: 1,
    capture_count: 2,
    form: CompiledFunctionForm::Arrow,
    body: |heap, c, _, _, r| {
        heap.define_own_data(&c[0], "busy".into(), heap.boolean(false)?, r)?;
        let first = heap.truthy(&heap.get(&c[0], &"firstMode".into(), r)?)?;
        let result = heap.object()?;
        heap.define_own_data(&result, "done".into(), heap.boolean(false)?, r)?;
        heap.define_own_data(
            &result,
            "value".into(),
            heap.string(if first { "first" } else { "second" }.into())?,
            r,
        )?;
        if heap.truthy(&heap.get(&c[0], &"stall".into(), r)?)? {
            Ok(c[1].clone())
        } else {
            Ok(result)
        }
    },
};
static NEXT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original issue4593 source first / await original work / second / EOF",
    name: "next",
    length: 0,
    capture_count: 3,
    form: CompiledFunctionForm::Builtin,
    body: |heap, c, _, _, r| {
        let index = heap.to_number(&heap.get(&c[0], &"calls".into(), r)?, r)?;
        heap.define_own_data(&c[0], "calls".into(), heap.number(index + 1.0)?, r)?;
        let first = heap.truthy(&heap.get(&c[0], &"firstMode".into(), r)?)?;
        if (!first && index == 1.0) || (first && index == 0.0) {
            let then = heap.get(&c[1], &"then".into(), r)?;
            let cont =
                heap.fresh_compiled_function(&WORK_CONTINUATION, &[c[0].clone(), c[2].clone()])?;
            return heap.call(&then, &c[1], &[cont], r);
        }
        let result = heap.object()?;
        let done = if first { index > 0.0 } else { index > 1.0 };
        heap.define_own_data(&result, "done".into(), heap.boolean(done)?, r)?;
        if !done {
            heap.define_own_data(&result, "value".into(), heap.string("first".into())?, r)?;
        }
        let promise = heap.get(&c[0], &"Promise".into(), r)?;
        let resolve = heap.get(&promise, &"resolve".into(), r)?;
        heap.call(&resolve, &promise, &[result], r)
    },
};
static PROBE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original issue4593 expired deadline probe resolves original workDone at threshold",
    name: "hasPendingLocalWork",
    length: 0,
    capture_count: 2,
    form: CompiledFunctionForm::Arrow,
    body: |heap, c, _, _, r| {
        let calls = heap.to_number(&heap.get(&c[0], &"probes".into(), r)?, r)? + 1.0;
        heap.define_own_data(&c[0], "probes".into(), heap.number(calls)?, r)?;
        let threshold = heap.to_number(&heap.get(&c[0], &"threshold".into(), r)?, r)?;
        if calls >= threshold {
            heap.call(&c[1], &heap.undefined()?, &[], r)?;
        }
        heap.get(&c[0], &"busy".into(), r)
    },
};
static RETURN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original generator return is observed once and caught",
    name: "return",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::Builtin,
    body: |heap, c, _, _, r| {
        let n = heap.to_number(&heap.get(&c[0], &"returns".into(), r)?, r)?;
        heap.define_own_data(&c[0], "returns".into(), heap.number(n + 1.0)?, r)?;
        let promise = heap.get(&c[0], &"Promise".into(), r)?;
        let resolve = heap.get(&promise, &"resolve".into(), r)?;
        let result = heap.object()?;
        heap.define_own_data(&result, "done".into(), heap.boolean(true)?, r)?;
        heap.call(&resolve, &promise, &[result], r)
    },
};

/// Upgraded b91 options.localWork representation of the retained original
/// source pending probe. Original case IDs/assertions remain unchanged.
fn source_local_work(h: &Harness, pending: ValueHandle) -> Result<ValueHandle> {
    let ara_rpc::value_heap::ValueSnapshot::Function(getter) = h.heap().snapshot(&pending)? else { return Err(ValueError::InvalidNode) };
    let source = h.object(&[("localWorkSettledAt", h.heap().number(0.0)?)])?;
    h.with(|runtime| h.heap().define_own(&source, "hasPendingLocalWork".into(),
        ara_rpc::value_heap::PropertyDescriptor::Accessor {
            get: Some(getter), set: None, enumerable: true, configurable: true,
        }, runtime))?;
    Ok(source)
}

async fn local_case(h: &mut Harness, first: bool, stall: bool) -> Result<()> {
    let work = h
        .driver
        .admission()
        .prepare_pending(h.driver.root_scope(), h.driver.root_parent())?;
    let never = h
        .driver
        .admission()
        .prepare_pending(h.driver.root_scope(), h.driver.root_parent())?;
    let promise = h.get(&h.bindings.globals(), "Promise")?;
    let state = h.object(&[
        ("busy", h.heap().boolean(true)?),
        ("calls", h.heap().number(0.0)?),
        ("returns", h.heap().number(0.0)?),
        ("probes", h.heap().number(0.0)?),
        ("firstMode", h.heap().boolean(first)?),
        ("stall", h.heap().boolean(stall)?),
        ("threshold", h.heap().number(if stall { 1.0 } else { 2.0 })?),
        ("Promise", promise),
    ])?;
    let source = h.heap().object()?;
    h.with(|r| {
        h.heap().define_own_data(
            &source,
            h.bindings.async_iterator_key(),
            h.heap().fresh_compiled_function(&SELF, &[])?,
            r,
        )
    })?;
    h.put(
        &source,
        "next",
        h.heap()
            .fresh_compiled_function(&NEXT, &[state.clone(), work.promise, never.promise])?,
    )?;
    h.put(
        &source,
        "return",
        h.heap()
            .fresh_compiled_function(&RETURN, &[state.clone()])?,
    )?;
    let fired = Arc::new(AtomicUsize::new(0));
    let count = fired.clone();
    let on_idle: Arc<dyn Fn() -> Result<()> + Send + Sync> = Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let options = h.object(&[
        ("idleTimeoutMs", h.heap().number(50.0)?),
        ("firstItemTimeoutMs", h.heap().number(50.0)?),
        ("errorMessage", h.heap().string("stalled".into())?),
        (
            "firstItemErrorMessage",
            h.heap().string("first event timed out".into())?,
        ),
        (
            "localWork",
            source_local_work(h, h.heap().fresh_compiled_function(&PROBE, &[state.clone(), work.resolve])?)?,
        ),
        ("onIdle", callback(h, on_idle.clone())?),
    ])?;
    let mut idle = IdleIterator::prepare(h.factory.clone(), h.bindings.clone(), source, options)?;
    let outcome = h
        .run_until(async {
            let mut items = Vec::new();
            loop {
                match idle.next().await {
                    Ok(Some(item)) => items.push(item),
                    Ok(None) => return Ok((items, None)),
                    Err(error) => return Ok((items, Some(error))),
                }
            }
        })
        .await?;
    let items = outcome
        .0
        .iter()
        .map(|v| h.s(v))
        .collect::<Result<Vec<_>>>()?;
    check(
        items
            == if first {
                vec!["first"]
            } else if stall {
                vec!["first"]
            } else {
                vec!["first", "second"]
            },
        "Original local work items changed",
    )?;
    check(
        h.n(&h.get(&state, "calls")?)?
            == if stall {
                2.0
            } else if first {
                2.0
            } else {
                3.0
            },
        "Outstanding next was issued twice during local work",
    )?;
    check(
        h.n(&h.get(&state, "returns")?)? == 1.0,
        "Original iterator return count changed",
    )?;
    if stall {
        let Some(ValueError::Thrown(error)) = outcome.1 else {
            return Err(ValueError::UnsupportedOperation(
                "Post-work real silent stall did not throw original native timeout",
            ));
        };
        check(
            h.s(&h.get(&error, "message")?)? == "stalled",
            "Original post-work stall message changed",
        )?;
    } else {
        check(outcome.1.is_none(), "Local work became timeout")?;
        check(
            h.n(&h.get(&state, "probes")?)? >= 2.0,
            "Local work did not exceed two actual deadlines",
        )?;
        check(
            fired.load(Ordering::SeqCst) == 0,
            "Local work fired idle callback",
        )?;
    }
    Ok(())
}

async fn pre_response4(h: &mut Harness) -> Result<()> {
    begin("B-340301adf0");
    let caller = controller(h)?;
    for ms in [None, Some(0.0), Some(-5.0)] {
        let timeout = match ms {
            Some(n) => h.heap().number(n)?,
            None => h.heap().undefined()?,
        };
        let guard = arm_pre_response_timeout(
            &h.factory,
            h.bindings.clone(),
            Some(caller.signal.clone()),
            timeout,
        )?;
        check(
            guard
                .signal
                .as_ref()
                .is_some_and(|s| s.same_identity(&caller.signal)),
            "Disabled pre-response guard replaced caller signal",
        )?;
        guard.clear()?;
    }
    check(
        arm_pre_response_timeout(&h.factory, h.bindings.clone(), None, h.heap().undefined()?)?
            .signal
            .is_none(),
        "Absent caller became signal",
    )?;
    pass("B-340301adf0");
    tokio::time::pause();
    begin("B-ddae8174ac");
    let guard =
        arm_pre_response_timeout(&h.factory, h.bindings.clone(), None, h.heap().number(50.0)?)?;
    let signal = guard.signal.clone().ok_or(ValueError::InvalidNode)?;
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(49)).await;
    // Drain already enqueued graph jobs without advancing the platform timer.
    while matches!(
        h.driver.drain_one()?,
        ara_cli::native_realm::promise::NativeDrain::RanJob { .. }
    ) {}
    check(
        !aborted(h, &signal)?,
        "Pre-response guard aborted before 50ms",
    )?;
    tokio::time::advance(Duration::from_millis(1)).await;
    let token = h.with(|r| h.bindings.signals().token(&h.factory, &signal, r))?;
    h.run_until(async {
        token.cancelled().await;
        Ok(())
    })
    .await?;
    let reason = h.get(&signal, "reason")?;
    check(
        h.s(&h.get(&reason, "name")?)? == "TimeoutError",
        "Pre-response reason name changed",
    )?;
    check(
        h.s(&h.get(&reason, "message")?)? == "The operation timed out.",
        "Pre-response reason message changed",
    )?;
    guard.clear()?;
    pass("B-ddae8174ac");
    begin("B-9dc7d07182");
    let guard =
        arm_pre_response_timeout(&h.factory, h.bindings.clone(), None, h.heap().number(50.0)?)?;
    let signal = guard.signal.clone().ok_or(ValueError::InvalidNode)?;
    guard.clear()?;
    tokio::time::advance(Duration::from_millis(10_000)).await;
    tokio::task::yield_now().await;
    while matches!(
        h.driver.drain_one()?,
        ara_cli::native_realm::promise::NativeDrain::RanJob { .. }
    ) {}
    check(
        !aborted(h, &signal)?,
        "Cleared pre-response guard still aborted",
    )?;
    pass("B-9dc7d07182");
    begin("B-2357e1aa5f");
    let caller = controller(h)?;
    let guard = arm_pre_response_timeout(
        &h.factory,
        h.bindings.clone(),
        Some(caller.signal.clone()),
        h.heap().number(60_000.0)?,
    )?;
    let signal = guard.signal.clone().ok_or(ValueError::InvalidNode)?;
    guard.clear()?;
    let original = h.heap().object()?;
    h.with(|r| {
        h.bindings
            .signals()
            .abort(&h.factory, &caller, Some(original.clone()), r)
    })?;
    check(aborted(h, &signal)?, "Caller abort lost after clear")?;
    check(
        h.get(&signal, "reason")?.same_identity(&original),
        "Caller original reason replaced",
    )?;
    pass("B-2357e1aa5f");
    tokio::time::resume();
    Ok(())
}

pub async fn original8(h: &mut Harness) -> Result<()> {
    begin("B-82a2371a2f");
    local_case(h, false, false).await?;
    pass("B-82a2371a2f");
    begin("B-f9ca06e896");
    local_case(h, false, true).await?;
    pass("B-f9ca06e896");
    begin("B-1178bdc63d");
    local_case(h, true, false).await?;
    pass("B-1178bdc63d");
    // Registry consumer is defined below; no direct provider closure bypass.
    original_lazy(h).await?;
    pre_response4(h).await
}

#[derive(Default)]
struct RegistryHost {
    custom_registered: AtomicUsize,
    custom_removed: AtomicUsize,
}
impl ara_cli::model_registry_extensions::ExtensionRegistryHost for RegistryHost {
    fn custom_api_registered(
        &self,
        _: &ara_cli::model_registry_extensions::RuntimeApiRegistration,
    ) {
        self.custom_registered.fetch_add(1, Ordering::SeqCst);
    }
    fn custom_api_removed(&self, _: &ara_rpc::WireString) {
        self.custom_removed.fetch_add(1, Ordering::SeqCst);
    }
    fn oauth_registered(&self, _: &ara_cli::model_registry_extensions::RuntimeOAuthEntry) {}
    fn oauth_removed(&self, _: &ara_rpc::WireString) {}
    fn set_runtime_usage(
        &self,
        _: &ara_rpc::WireString,
        _: &ara_cli::model_registry_extensions::OpaqueExtensionHandle,
        _: Option<&str>,
    ) {
    }
    fn remove_runtime_usage(&self, _: &ara_rpc::WireString) {}
    fn install_api_key(
        &self,
        _: &ara_rpc::WireString,
        _: &str,
    ) -> ara_cli::model_registry_extensions::ExtensionResult<()> {
        Ok(())
    }
    fn remove_config_api_key(&self, _: &ara_rpc::WireString) {}
    fn has_full_snapshot(&self) -> bool {
        true
    }
    fn ensure_full_snapshot(&self) -> ara_cli::model_registry_extensions::ExtensionResult<()> {
        Ok(())
    }
    fn reload_static(&self, _: bool) -> ara_cli::model_registry_extensions::ExtensionResult<()> {
        Ok(())
    }
    fn models_registered(
        &self,
        _: &ara_rpc::WireString,
        _: &[ara_cli::custom_models::CustomModelOverlay],
        _: Option<&ara_cli::model_patch::ProviderOverride>,
    ) -> ara_cli::model_registry_extensions::ExtensionResult<()> {
        Ok(())
    }
    fn transport_applied(
        &self,
        _: &ara_rpc::WireString,
        _: &ara_cli::model_patch::ProviderOverride,
    ) -> ara_cli::model_registry_extensions::ExtensionResult<()> {
        Ok(())
    }
    fn invalidate_provider(&self, _: &ara_rpc::WireString) {}
    fn invalidate_all_provider_lookups(&self) {}
}
struct ProbedWork {
    tracker: ara_ai::event::LocalWorkTracker,
    calls: AtomicUsize,
    factory: Arc<ara_ai::graph_values::GraphValueFactory>,
    calling: ara_cli::mock_provider_host::NativeCallingRun,
    original_resolve: ValueHandle,
}
impl LocalWorkSource for ProbedWork {
    fn has_pending_local_work(&self) -> Result<bool> {
        let count = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if count >= 2 {
            self.calling.with_runtime(&self.factory, &mut |r| {
                self.factory.heap().call(
                    &self.original_resolve,
                    &self.factory.heap().undefined()?,
                    &[],
                    r,
                )
            })?;
        }
        self.tracker.has_pending_local_work()
    }
    fn local_work_settled_at_in(
        &self, execution: ara_ai::graph_values::GraphExecution<'_>, globals: &ValueHandle,
    ) -> Result<ValueHandle> {
        self.tracker.local_work_settled_at_in(execution, globals)
    }
}
struct RegisteredProvider {
    source: std::sync::Mutex<Option<ara_cli::retry_host::lazy_provider::NativeLazySource>>,
    signal: std::sync::Mutex<Option<ValueHandle>>,
}
impl ara_cli::retry_host::lazy_provider::NativeLazyProvider for RegisteredProvider {
    fn stream(
        &self,
        factory: &Arc<ara_ai::graph_values::GraphValueFactory>,
        _: &ValueHandle,
        _: &ValueHandle,
        options: &ValueHandle,
        r: &dyn ara_rpc::value_heap::ValueRuntime,
    ) -> Result<ara_cli::retry_host::lazy_provider::NativeLazySource> {
        *self.signal.lock().map_err(|_| ValueError::PoisonedHeap)? =
            Some(factory.heap().get(options, &"signal".into(), r)?);
        self.source
            .lock()
            .map_err(|_| ValueError::PoisonedHeap)?
            .take()
            .ok_or(ValueError::UnsupportedOperation(
                "Original lazy provider invoked twice",
            ))
    }
}
static CLASSIFY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original no-classification fixture import",
    name: "classify",
    length: 2,
    capture_count: 0,
    form: CompiledFunctionForm::Builtin,
    body: |heap, _, _, _, _| heap.undefined(),
};
async fn original_lazy(h: &mut Harness) -> Result<()> {
    use ara_ai::{ActualAssistantEvent, StopReason, retry::result::ActualCompletionStream};
    use ara_cli::{model_registry_extensions::ExtensionRegistry, retry_host::lazy_provider::*};
    begin("B-d4843cd3fe");
    let work = h
        .driver
        .admission()
        .prepare_pending(h.driver.root_scope(), h.driver.root_parent())?;
    let (mut producer, source, _) = ActualCompletionStream::channel(&h.factory);
    let tracker = source.events.local_work_tracker();
    let tracked = tracker
        .track_in_original(|ticket| h.bindings.adopt_local_work(h.factory.clone(), work.promise, ticket));
    let probe = Arc::new(ProbedWork {
        tracker,
        calls: AtomicUsize::new(0),
        factory: h.factory.clone(),
        calling: h.frozen.clone(),
        original_resolve: work.resolve,
    });
    let provider = Arc::new(RegisteredProvider {
        source: std::sync::Mutex::new(Some(NativeLazySource {
            identity: h.heap().object()?,
            original_result: Some(source.original_result.clone()),
            stream: source,
            local_work: Some(probe.clone()),
        })),
        signal: std::sync::Mutex::new(None),
    });
    let registry = ExtensionRegistry::new();
    let registry_host = RegistryHost::default();
    let replaced = Arc::new(RegisteredProvider {
        source: std::sync::Mutex::new(None),
        signal: std::sync::Mutex::new(None),
    });
    set_bedrock_provider_module(
        &registry,
        Arc::new(RegisteredLazyProvider {
            provider: replaced.clone(),
            limits: LazyStreamLimits::default(),
            classify: h.heap().fresh_compiled_function(&CLASSIFY, &[])?,
        }),
    );
    let module = Arc::new(RegisteredLazyProvider {
        provider: provider.clone(),
        limits: LazyStreamLimits::default(),
        classify: h.heap().fresh_compiled_function(&CLASSIFY, &[])?,
    });
    set_bedrock_provider_module(&registry, module.clone());
    // The original builtin API remains reserved even after an override is
    // installed; extension-source cleanup owns only its custom registration.
    check(
        register_lazy_provider(
            &registry,
            &registry_host,
            "bedrock-converse-stream".into(),
            Some("original4593".into()),
            module.clone(),
        ).is_err(),
        "Builtin override bypassed the reserved custom API guard",
    )?;
    register_lazy_provider(
        &registry,
        &registry_host,
        "cleanup-source-api".into(),
        Some("original4593".into()),
        module,
    )
    .expect("Original custom source registry registration");
    registry
        .clear_source_registrations(&"original4593".into(), &registry_host)
        .expect("Original custom source cleanup");
    check(
        registry.snapshot().custom_apis.is_empty()
            && registry_host.custom_registered.load(Ordering::SeqCst) == 1
            && registry_host.custom_removed.load(Ordering::SeqCst) == 1,
        "Builtin module called custom hooks or source cleanup retained custom API",
    )?;
    let model=h.data(serde_json::json!({"id":"mock-bedrock","name":"Mock Bedrock","api":"bedrock-converse-stream","provider":"amazon-bedrock","baseUrl":"https://example.invalid","reasoning":false,"input":["text"],"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"contextWindow":8192,"maxTokens":2048}))?;
    let context = h.data(serde_json::json!({"messages":[]}))?;
    let options = h.data(serde_json::json!({"streamIdleTimeoutMs":50}))?;
    let partial=h.data(serde_json::json!({"role":"assistant","content":[{"type":"text","text":"ok"}],"api":"bedrock-converse-stream","provider":"amazon-bedrock","model":"mock-bedrock","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"stop","timestamp":0}))?;
    let original = partial.clone();
    let invocation = dispatch_registered_lazy(
        &registry,
        &"bedrock-converse-stream".into(),
        h.factory.clone(),
        h.bindings.clone(),
        model,
        context,
        options,
    )
    .expect("Actual registered original lazy source dispatch");
    check(
        replaced.signal.lock().map_err(|_| ValueError::PoisonedHeap)?.is_none(),
        "Bedrock module replacement dispatched the obsolete module",
    )?;
    let original_result = invocation.stream.original_result.clone();
    let receipt = invocation.receipt.clone();
    let delta = "running a local tool".into();
    let content_index = h.heap().number(0.0)?;
    let source_work = async move {
        producer
            .push(ActualAssistantEvent::Start {
                partial: partial.clone(),
            })
            .await?;
        producer
            .push(ActualAssistantEvent::TextDelta {
                content_index,
                delta,
                partial: partial.clone(),
            })
            .await?;
        tracked.wait().await?;
        producer
            .push(ActualAssistantEvent::Done {
                reason: StopReason::Stop,
                message: partial,
            })
            .await?;
        Ok::<_, ValueError>(())
    };
    let events = h
        .run_until(async {
            let (_, events) = tokio::try_join!(source_work, collect_registered_lazy(invocation))?;
            Ok(events)
        })
        .await?;
    let result = h.run_until(original_result.result()).await?;
    check(
        result.same_identity(&original),
        "Lazy result replaced original final root",
    )?;
    check(
        h.s(&h.get(&result, "stopReason")?)? == "stop",
        "Lazy local work failed instead of stop",
    )?;
    check(
        h.heap().kind(&h.get(&result, "errorMessage")?)? == ValueKind::Undefined,
        "Lazy local work acquired errorMessage",
    )?;
    let signal = provider
        .signal
        .lock()
        .map_err(|_| ValueError::PoisonedHeap)?
        .clone()
        .ok_or(ValueError::InvalidNode)?;
    check(
        !aborted(h, &signal)?,
        "Lazy provider signal aborted during tracked work",
    )?;
    check(
        probe.calls.load(Ordering::SeqCst) >= 2,
        "Registered stream getter was not probed twice",
    )?;
    check(
        events.len() == 3 && events[2].is_terminal(),
        "Lazy forwarding lost original event sequence",
    )?;
    check(
        receipt.wait_settled().await.is_empty(),
        "Lazy original owner late failure",
    )?;
    pass("B-d4843cd3fe");
    Ok(())
}

static THROW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original imported callback throws its original reason",
    name: "",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::Arrow,
    body: |_, c, _, _, _| Err(ValueError::Thrown(c[0].clone())),
};
static RETURN_CAPTURE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original source callback returns unchanged captured value",
    name: "",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::Arrow,
    body: |_, c, _, _, _| Ok(c[0].clone()),
};
static ARRAY_NEXT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Actual native source pull returns original queued values then original pending Promise",
    name: "next",
    length: 0,
    capture_count: 2,
    form: CompiledFunctionForm::Builtin,
    body: |heap, c, _, _, r| {
        let index = heap.to_number(&heap.get(&c[0], &"calls".into(), r)?, r)?;
        heap.define_own_data(&c[0], "calls".into(), heap.number(index + 1.0)?, r)?;
        let items = heap.get(&c[0], &"items".into(), r)?;
        let count = heap.to_number(&heap.get(&items, &"length".into(), r)?, r)?;
        if index >= count && heap.truthy(&heap.get(&c[0], &"pending".into(), r)?)? {
            return Ok(c[1].clone());
        }
        let result = heap.object()?;
        heap.define_own_data(&result, "done".into(), heap.boolean(index >= count)?, r)?;
        if index < count {
            heap.define_own_data(
                &result,
                "value".into(),
                heap.get(&items, &index.to_string().as_str().into(), r)?,
                r,
            )?;
        }
        let promise = heap.get(&c[0], &"Promise".into(), r)?;
        let resolve = heap.get(&promise, &"resolve".into(), r)?;
        heap.call(&resolve, &promise, &[result], r)
    },
};
fn queue_source(
    h: &Harness,
    items: Vec<ValueHandle>,
    pending: bool,
) -> Result<(ValueHandle, ValueHandle)> {
    let never = h
        .driver
        .admission()
        .prepare_pending(h.driver.root_scope(), h.driver.root_parent())?;
    let state = h.object(&[
        ("calls", h.heap().number(0.0)?),
        ("returns", h.heap().number(0.0)?),
        ("items", h.heap().array(items)?),
        ("pending", h.heap().boolean(pending)?),
        ("Promise", h.get(&h.bindings.globals(), "Promise")?),
    ])?;
    let source = h.heap().object()?;
    h.with(|r| {
        h.heap().define_own_data(
            &source,
            h.bindings.async_iterator_key(),
            h.heap().fresh_compiled_function(&SELF, &[])?,
            r,
        )
    })?;
    h.put(
        &source,
        "next",
        h.heap()
            .fresh_compiled_function(&ARRAY_NEXT, &[state.clone(), never.promise])?,
    )?;
    h.put(
        &source,
        "return",
        h.heap()
            .fresh_compiled_function(&RETURN, &[state.clone()])?,
    )?;
    Ok((source, state))
}
fn native_now(h: &Harness) -> Result<f64> {
    h.with(|r| {
        let date = h.heap().get(&h.bindings.globals(), &"Date".into(), r)?;
        let now = h.heap().get(&date, &"now".into(), r)?;
        h.heap().to_number(&h.heap().call(&now, &date, &[], r)?, r)
    })
}

async fn abort_contracts(h: &mut Harness) -> Result<()> {
    use ara_ai::retry::abort::{create_abort_source_tracker, race_with_signal, sleep};
    let caller = controller(h)?;
    let tracker = h.with(|r| {
        create_abort_source_tracker(
            &h.factory,
            h.bindings.clone(),
            Some(caller.signal.clone()),
            r,
        )
    })?;
    let local = h.heap().object()?;
    h.with(|r| tracker.abort_locally(&h.factory, local.clone(), r))?;
    check(
        h.with(|r| tracker.get_local_abort_reason(&h.factory, r))?
            .same_identity(&local),
        "Local abort original identity lost",
    )?;
    let caller_reason = h.heap().object()?;
    h.with(|r| {
        h.bindings
            .signals()
            .abort(&h.factory, &caller, Some(caller_reason.clone()), r)
    })?;
    check(
        h.with(|r| tracker.was_caller_abort(&h.factory, r))?,
        "Caller priority lost after local abort",
    )?;
    check(
        h.heap()
            .kind(&h.with(|r| tracker.get_local_abort_reason(&h.factory, r))?)?
            == ValueKind::Undefined,
        "Local reason leaked after caller abort",
    )?;
    let original = h
        .driver
        .admission()
        .prepare_pending(h.driver.root_scope(), h.driver.root_parent())?;
    let caller = controller(h)?;
    let reason = h.heap().object()?;
    let raced = race_with_signal(
        h.factory.clone(),
        h.bindings.clone(),
        original.promise.clone(),
        Some(caller.signal.clone()),
    );
    h.with(|r| {
        h.bindings
            .signals()
            .abort(&h.factory, &caller, Some(reason.clone()), r)
    })?;
    let outcome = h.run_until(async { Ok(raced.await) }).await?;
    check(
        matches!(outcome,Err(ValueError::Thrown(ref error)) if error.same_identity(&reason)),
        "Race replaced arbitrary original abort reason",
    )?;
    let fulfilled = h.heap().object()?;
    h.with(|r| {
        h.heap().call(
            &original.resolve,
            &h.heap().undefined()?,
            &[fulfilled.clone()],
            r,
        )
    })?;
    let recovered = h
        .run_until(h.frozen.await_value(h.factory.clone(), original.promise))
        .await?;
    check(
        recovered.same_identity(&fulfilled),
        "Abort losing waiter canceled original Promise",
    )?;
    let caller = controller(h)?;
    h.with(|r| {
        h.bindings
            .signals()
            .abort(&h.factory, &caller, Some(h.heap().null()?), r)
    })?;
    let original = h
        .driver
        .admission()
        .prepare_pending(h.driver.root_scope(), h.driver.root_parent())?;
    let raced = race_with_signal(
        h.factory.clone(),
        h.bindings.clone(),
        original.promise,
        Some(caller.signal),
    );
    let outcome = h.run_until(async { Ok(raced.await) }).await?;
    let Err(ValueError::Thrown(reason)) = outcome else {
        return Err(ValueError::UnsupportedOperation(
            "Nullish abort did not throw imported AbortError",
        ));
    };
    check(
        h.s(&h.get(&reason, "name")?)? == "AbortError",
        "Nullish abort fallback class changed",
    )?;
    check(
        h.s(&h.get(&reason, "message")?)? == "Request was aborted",
        "Nullish abort fallback message changed",
    )?;
    h.run_until(sleep(h.factory.clone(), h.bindings.clone(), 1.0, None))
        .await?;
    let caller = controller(h)?;
    let original = h.heap().object()?;
    let slept = sleep(
        h.factory.clone(),
        h.bindings.clone(),
        60_000.0,
        Some(caller.signal.clone()),
    );
    h.with(|r| {
        h.bindings
            .signals()
            .abort(&h.factory, &caller, Some(original.clone()), r)
    })?;
    let outcome = h.run_until(async { Ok(slept.await) }).await?;
    check(
        matches!(outcome,Err(ValueError::Thrown(ref reason))if reason.same_identity(&original)),
        "Sleep lost original abort reason",
    )?;
    // Unknown original work is not marked settled merely because its issuing
    // native admission was revoked; independent ticket observation is required.
    let tracker = ara_ai::event::LocalWorkTracker::default();
    let unknown = tracker.track_observed_local_work(|| {
        Ok(Box::pin(async {
            ara_ai::retry::NativeWorkObservation {
                result: Err(ValueError::UnsupportedOperation("Lost actual observation")),
                settled: false,
            }
        }))
    });
    check(
        unknown.wait().await.is_err() && tracker.has_pending_local_work()?,
        "Unknown work became successful settlement",
    )?;
    eprintln!("ARA-RETRY-SOURCE PASS abort");
    Ok(())
}

fn sdk_contracts(h: &Harness) -> Result<()> {
    use ara_ai::retry::sdk_timeout::SdkTimeoutModule;
    let sdk = SdkTimeoutModule::prepare(&h.factory, h.bindings.globals())?;
    let signal = h.heap().object()?;
    for timeout in [
        h.heap().undefined()?,
        h.heap().null()?,
        h.heap().number(0.0)?,
        h.heap().number(-5.0)?,
        h.heap().number(f64::NAN)?,
        h.heap().number(f64::INFINITY)?,
        h.heap().string("50".into())?,
    ] {
        check(
            h.heap()
                .kind(&h.with(|r| sdk.resolve(&h.factory, timeout.clone(), r))?)?
                == ValueKind::Undefined,
            "SDK resolved nonfinite/nonnumber/nonpositive timeout",
        )?;
        let options = h.with(|r| sdk.options(&h.factory, signal.clone(), timeout, r))?;
        check(
            h.get(&options, "signal")?.same_identity(&signal),
            "SDK replaced actual signal",
        )?;
        check(
            h.with(|r| h.heap().own_keys(&options, r))?.len() == 1,
            "Disabled SDK added retry/timeout fields",
        )?;
    }
    for (input, expected) in [(50.9, 50.0), (0.5, 0.0)] {
        let options =
            h.with(|r| sdk.options(&h.factory, signal.clone(), h.heap().number(input)?, r))?;
        check(
            h.n(&h.get(&options, "timeout")?)? == expected,
            "SDK truncation including positive fraction changed",
        )?;
        check(
            h.n(&h.get(&options, "maxRetries")?)? == 0.0,
            "SDK retry suppression missing",
        )?;
        check(
            h.with(|r| h.heap().own_keys(&options, r))?.len() == 3,
            "SDK added unrelated request fields",
        )?;
    }
    let math = h.get(&h.bindings.globals(), "Math")?;
    let original_trunc = h.get(&math, "trunc")?;
    let original_error = h.heap().object()?;
    h.put(
        &math,
        "trunc",
        h.heap()
            .fresh_compiled_function(&THROW, &[original_error.clone()])?,
    )?;
    let result = h.with(|r| sdk.resolve(&h.factory, h.heap().number(10.5)?, r));
    h.put(&math, "trunc", original_trunc)?;
    check(
        matches!(result,Err(ValueError::Thrown(ref v))if v.same_identity(&original_error)),
        "SDK patched import original throw was folded",
    )?;
    eprintln!("ARA-RETRY-SOURCE PASS sdk-stream-timeout");
    Ok(())
}

fn hint_contracts(h: &Harness) -> Result<()> {
    use ara_ai::retry::hints::RetryAfterModule;
    let module = RetryAfterModule::prepare(&h.factory, h.bindings.globals())?;
    let wait = |headers: ValueHandle| {
        h.with(|r| {
            module.invoke(
                &h.factory,
                &module.get_retry_after_ms_from_headers,
                &[headers],
                r,
            )
        })
    };
    let record=h.data(serde_json::json!({"ReTrY-AfTeR-Ms":" 20.1 ","retry-after":"0.1","x-ratelimit-reset-ms":"30","x-ratelimit-reset":"0.2"}))?;
    check(
        h.n(&wait(record.clone())?)? == 200.0,
        "Four header hints did not select maximum",
    )?;
    let native = h.with(|r| h.heap().construct(&h.signals.headers, &[record.clone()], r))?;
    check(
        h.n(&wait(native.clone())?)? == 200.0,
        "Actual Headers branch differed from record",
    )?;
    let original_get = h.get(&native, "get")?;
    let original_error = h.heap().object()?;
    h.put(
        &native,
        "get",
        h.heap()
            .fresh_compiled_function(&THROW, &[original_error.clone()])?,
    )?;
    let result = wait(native.clone());
    h.put(&native, "get", original_get)?;
    check(
        matches!(result,Err(ValueError::Thrown(ref v))if v.same_identity(&original_error)),
        "Actual Headers.get original throw was folded",
    )?;
    for record in [
        serde_json::json!({"retry-after-ms":"0","retry-after":"-1"}),
        serde_json::json!({"retry-after-ms":"NaN"}),
        serde_json::json!({"retry-after-ms":30}),
        serde_json::json!({}),
    ] {
        check(
            h.heap().kind(&wait(h.data(record)?)?)? == ValueKind::Undefined,
            "Invalid header became retry delay",
        )?;
    }
    let error = h.with(|r| {
        h.heap().construct(
            &h.bindings.error_constructor(),
            &[h.heap().string("failed".into())?],
            r,
        )
    })?;
    h.put(
        &error,
        "cause",
        h.object(&[("response", h.object(&[("headers", native.clone())])?)])?,
    )?;
    let found = h.with(|r| {
        module.invoke(
            &h.factory,
            &module.get_headers_from_error,
            &[error.clone()],
            r,
        )
    })?;
    check(
        found.same_identity(&native),
        "Cause response header original identity lost",
    )?;
    let suffix = h.with(|r| {
        module.invoke(
            &h.factory,
            &module.format_error_message_with_retry_after,
            &[error],
            r,
        )
    })?;
    check(
        h.s(&suffix)? == "failed retry-after-ms=200",
        "Canonical header suffix changed",
    )?;
    let already = h.with(|r| {
        h.heap().construct(
            &h.bindings.error_constructor(),
            &[h.heap().string("failed retry-after-ms=3".into())?],
            r,
        )
    })?;
    let formatted = h.with(|r| {
        module.invoke(
            &h.factory,
            &module.format_error_message_with_retry_after,
            &[already, record],
            r,
        )
    })?;
    check(
        h.s(&formatted)? == "failed retry-after-ms=3",
        "Existing canonical suffix duplicated",
    )?;
    let now = native_now(h)?;
    let reset = h.data(serde_json::json!({"x-ratelimit-reset-ms":(now+30_000.0).to_string()}))?;
    let delay = h.n(&wait(reset)?)?;
    check(
        delay > 29_000.0 && delay <= 30_000.0,
        "Actual epoch reset-ms failed",
    )?;
    let date = chrono::DateTime::<chrono::Utc>::from_timestamp_millis((now + 60_000.0) as i64)
        .ok_or(ValueError::InvalidNode)?
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string();
    let delay = h.n(&wait(h.data(serde_json::json!({"retry-after":date}))?)?)?;
    check(
        delay > 58_000.0 && delay <= 60_000.0,
        "Production actual Date.parse HTTP header failed",
    )?;
    eprintln!("ARA-RETRY-SOURCE PASS retry-after");
    Ok(())
}

async fn idle_contracts(h: &mut Harness) -> Result<()> {
    use ara_ai::retry::{abort::sleep, idle::TerminalGraceIterator};
    env_contracts(h)?;
    lazy_source_contracts(h).await?;
    let (source, _) = queue_source(h, Vec::new(), true)?;
    let raw = h
        .heap()
        .string(ara_rpc::WireString::from_units(vec![0xD800, 0x61]))?;
    let options = h.object(&[
        ("firstItemTimeoutMs", h.heap().number(1.0)?),
        ("errorMessage", raw.clone()),
    ])?;
    let mut idle = IdleIterator::prepare(h.factory.clone(), h.bindings.clone(), source, options)?;
    let result = h.run_until(async { Ok(idle.next().await) }).await?;
    let Err(ValueError::Thrown(error)) = result else {
        return Err(ValueError::UnsupportedOperation(
            "Raw UTF16 timeout replaced original error by bridge failure",
        ));
    };
    let message = h.get(&error, "message")?;
    let (
        ara_rpc::value_heap::ValueSnapshot::String(raw),
        ara_rpc::value_heap::ValueSnapshot::String(message),
    ) = (h.heap().snapshot(&raw)?, h.heap().snapshot(&message)?)
    else {
        return Err(ValueError::InvalidNode);
    };
    check(
        raw.units() == message.units(),
        "Timeout message raw UTF16 changed",
    )?;
    let (source, _) = queue_source(h, vec![h.heap().string("start".into())?], true)?;
    let options = h.object(&[
        ("firstItemTimeoutMs", h.heap().number(100.0)?),
        ("idleTimeoutMs", h.heap().number(1.0)?),
        ("errorMessage", h.heap().string("idle".into())?),
        ("firstItemErrorMessage", h.heap().string("first".into())?),
        (
            "isProgressItem",
            h.heap()
                .fresh_compiled_function(&RETURN_CAPTURE, &[h.heap().boolean(false)?])?,
        ),
    ])?;
    let mut idle = IdleIterator::prepare(h.factory.clone(), h.bindings.clone(), source, options)?;
    check(
        h.run_until(idle.next()).await?.is_some(),
        "Nonprogress start lost",
    )?;
    let result = h.run_until(async { Ok(idle.next().await) }).await?;
    let Err(ValueError::Thrown(error)) = result else {
        return Err(ValueError::UnsupportedOperation(
            "Nonprogress first phase did not time out",
        ));
    };
    check(
        h.s(&h.get(&error, "message")?)? == "first",
        "Nonprogress start flipped first phase to idle",
    )?;
    let original = h.heap().object()?;
    let (source, state) = queue_source(h, vec![original.clone()], false)?;
    let thrown = h
        .heap()
        .fresh_compiled_function(&THROW, &[h.heap().object()?])?;
    let options = h.object(&[
        ("idleTimeoutMs", h.heap().number(50.0)?),
        ("errorMessage", h.heap().string("stalled".into())?),
        ("isProgressItem", thrown.clone()),
    ])?;
    let mut idle = IdleIterator::prepare(h.factory.clone(), h.bindings.clone(), source, options)?;
    check(
        h.run_until(idle.next())
            .await?
            .is_some_and(|v| v.same_identity(&original)),
        "Progress callback native throw did not default true",
    )?;
    check(
        h.run_until(idle.next()).await?.is_none(),
        "Original EOF changed",
    )?;
    check(
        h.n(&h.get(&state, "returns")?)? == 1.0,
        "Idle source EOF did not return exactly once",
    )?;
    let (source, state) = queue_source(h, Vec::new(), true)?;
    let options = h.object(&[
        ("idleTimeoutMs", h.heap().number(50.0)?),
        ("errorMessage", h.heap().string("stalled".into())?),
        ("localWork", source_local_work(h, thrown)?),
    ])?;
    let mut idle = IdleIterator::prepare(h.factory.clone(), h.bindings.clone(), source, options)?;
    let result = h.run_until(async { Ok(idle.next().await) }).await?;
    check(
        matches!(result, Err(ValueError::Thrown(_))),
        "Local work original getter throw was lost",
    )?;
    check(
        h.n(&h.get(&state, "calls")?)? == 1.0,
        "Local work exception reissued outstanding next",
    )?;
    let caller = controller(h)?;
    let original = h.with(|r| {
        h.heap().construct(
            &h.bindings.error_constructor(),
            &[h.heap().string("caller".into())?],
            r,
        )
    })?;
    h.with(|r| {
        h.bindings
            .signals()
            .abort(&h.factory, &caller, Some(original.clone()), r)
    })?;
    let (source, state) = queue_source(h, vec![], true)?;
    let options = h.object(&[
        ("abortSignal", caller.signal),
        ("idleTimeoutMs", h.heap().number(0.0)?),
        ("firstItemTimeoutMs", h.heap().number(0.0)?),
    ])?;
    let mut idle = IdleIterator::prepare(h.factory.clone(), h.bindings.clone(), source, options)?;
    let result = h.run_until(async { Ok(idle.next().await) }).await?;
    check(
        matches!(result,Err(ValueError::Thrown(ref v))if v.same_identity(&original)),
        "Early idle abort replaced Error reason",
    )?;
    check(
        h.n(&h.get(&state, "calls")?)? == 0.0 && h.n(&h.get(&state, "returns")?)? == 1.0,
        "Early abort pulled source before close",
    )?;
    let (source, state) = queue_source(
        h,
        vec![
            h.heap().string("tail1".into())?,
            h.heap().string("tail2".into())?,
        ],
        true,
    )?;
    let ended = Arc::new(AtomicUsize::new(0));
    let count = ended.clone();
    let on_end: Arc<dyn Fn() -> Result<()> + Send + Sync> = Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let finished = h.heap().number(native_now(h)?)?;
    let options = h.object(&[
        (
            "finishedAtMs",
            h.heap()
                .fresh_compiled_function(&RETURN_CAPTURE, &[finished])?,
        ),
        ("graceMs", h.heap().number(50.0)?),
        ("onGraceEnd", callback(h, on_end.clone())?),
    ])?;
    let mut grace =
        TerminalGraceIterator::prepare(h.factory.clone(), h.bindings.clone(), source, options)?;
    let first_tail = h
        .run_until(grace.next())
        .await?
        .ok_or(ValueError::InvalidNode)?;
    check(
        h.s(&first_tail)? == "tail1",
        "Grace first trailing item lost",
    )?;
    let second_tail = h
        .run_until(grace.next())
        .await?
        .ok_or(ValueError::InvalidNode)?;
    check(
        h.s(&second_tail)? == "tail2",
        "Grace second trailing item lost",
    )?;
    h.run_until(sleep(h.factory.clone(), h.bindings.clone(), 55.0, None))
        .await?;
    check(
        h.run_until(grace.next()).await?.is_none(),
        "Grace deadline was extended by trailing items",
    )?;
    check(
        ended.load(Ordering::SeqCst) == 1
            && h.n(&h.get(&state, "calls")?)? == 2.0
            && h.n(&h.get(&state, "returns")?)? == 1.0,
        "Fixed grace expiry pulled source or omitted cleanup",
    )?;
    eprintln!("ARA-RETRY-SOURCE PASS idle-iterator");
    Ok(())
}

pub async fn source_groups(h: &mut Harness) -> Result<()> {
    abort_contracts(h).await?;
    idle_contracts(h).await?;
    upgraded_local_work_contracts(h).await?;
    eprintln!("ARA-RETRY-UPGRADE BANK BEGIN reentrant");
    reentrant_local_work_contract(h).await?;
    eprintln!("ARA-RETRY-UPGRADE BANK PASS reentrant");
    eprintln!("ARA-RETRY-UPGRADE BANK BEGIN revoke");
    authoritative_local_work_revoke_contract().await?;
    eprintln!("ARA-RETRY-UPGRADE BANK PASS revoke");
    eprintln!("ARA-RETRY-UPGRADE BANK BEGIN derived");
    derived_local_work_adoption_contract(h).await?;
    eprintln!("ARA-RETRY-UPGRADE BANK PASS derived");
    eprintln!("ARA-RETRY-UPGRADE BANK BEGIN budget-first");
    settled_budget_contract(h, true).await?;
    eprintln!("ARA-RETRY-UPGRADE BANK PASS budget-first");
    eprintln!("ARA-RETRY-UPGRADE BANK BEGIN budget-idle");
    settled_budget_contract(h, false).await?;
    eprintln!("ARA-RETRY-UPGRADE BANK PASS budget-idle");
    eprintln!("ARA-RETRY-UPGRADE BANK BEGIN terminal-return");
    early_terminal_return_contract(h).await?;
    eprintln!("ARA-RETRY-UPGRADE BANK PASS terminal-return");
    eprintln!("ARA-RETRY-UPGRADE BANK BEGIN ordered-idle");
    ordered_idle_deadline_contract(h).await?;
    eprintln!("ARA-RETRY-UPGRADE BANK PASS ordered-idle");
    eprintln!("ARA-RETRY-UPGRADE BANK BEGIN native-exit");
    native_run_terminal_drain_contract().await?;
    eprintln!("ARA-RETRY-UPGRADE BANK PASS native-exit");
    eprintln!("ARA-RETRY-UPGRADE BANK BEGIN cap");
    provider_inflight_source_contracts(h).await?;
    eprintln!("ARA-RETRY-UPGRADE BANK PASS cap");
    eprintln!("ARA-RETRY-UPGRADE BANK BEGIN hints");
    hint_contracts(h)?;
    eprintln!("ARA-RETRY-UPGRADE BANK PASS hints");
    eprintln!("ARA-RETRY-UPGRADE BANK BEGIN sdk");
    sdk_contracts(h)?;
    eprintln!("ARA-RETRY-UPGRADE BANK PASS sdk");
    eprintln!("ARA-RETRY-UPGRADE BANK BEGIN cancel");
    native_run_cancel_contract(false).await?;
    eprintln!("ARA-RETRY-UPGRADE BANK PASS cancel");
    eprintln!("ARA-RETRY-UPGRADE BANK BEGIN cancel-eof");
    native_run_cancel_contract(true).await?;
    eprintln!("ARA-RETRY-UPGRADE BANK PASS cancel-eof");
    Ok(())
}

/// Only the source's $env import is configured; every native capability,
/// Promise, signal, timer, factory and issuing Run remains the original Host.
struct EnvBindings {
    inner: Arc<dyn ara_ai::retry::RetryBindings>,
    environment: std::collections::HashMap<String, String>,
}
impl ara_ai::retry::RetryBindings for EnvBindings {
    fn check_invocation(&self, f: &Arc<ara_ai::graph_values::GraphValueFactory>) -> Result<()> {
        self.inner.check_invocation(f)
    }
    fn with_runtime(
        &self,
        f: &Arc<ara_ai::graph_values::GraphValueFactory>,
        op: &mut dyn FnMut(&dyn ara_rpc::value_heap::ValueRuntime) -> Result<ValueHandle>,
    ) -> Result<ValueHandle> {
        self.inner.with_runtime(f, op)
    }
    fn await_value(
        &self,
        f: Arc<ara_ai::graph_values::GraphValueFactory>,
        v: ValueHandle,
    ) -> futures::future::BoxFuture<'static, Result<ValueHandle>> {
        self.inner.await_value(f, v)
    }
    fn await_observed(
        &self,
        f: Arc<ara_ai::graph_values::GraphValueFactory>,
        v: ValueHandle,
    ) -> futures::future::BoxFuture<'static, ara_ai::retry::NativeWorkObservation> {
        self.inner.await_observed(f, v)
    }
    fn adopt_local_work(
        &self, factory: Arc<ara_ai::graph_values::GraphValueFactory>, value: ValueHandle,
        ticket: ara_ai::event::LocalWorkTicket,
    ) -> Result<futures::future::BoxFuture<'static, ara_ai::retry::NativeWorkObservation>> {
        self.inner.adopt_local_work(factory, value, ticket)
    }
    fn invocation_ended(&self) -> futures::future::BoxFuture<'static, ValueError> {
        self.inner.invocation_ended()
    }
    fn async_iterator_key(&self) -> ara_rpc::value_heap::PropertyKey {
        self.inner.async_iterator_key()
    }
    fn globals(&self) -> ValueHandle {
        self.inner.globals()
    }
    fn error_constructor(&self) -> ValueHandle {
        self.inner.error_constructor()
    }
    fn provider_response_error_constructor(&self) -> ValueHandle {
        self.inner.provider_response_error_constructor()
    }
    fn signals(&self) -> Arc<dyn ara_ai::retry::AbortSignalImports> {
        self.inner.signals()
    }
    fn timers(&self) -> Arc<dyn ara_ai::retry::TimerImports> {
        self.inner.timers()
    }
    fn receipt(&self) -> ara_ai::ActualStreamFailureReceipt {
        self.inner.receipt()
    }
    fn environment(&self, name: &str) -> Option<String> {
        self.environment.get(name).cloned()
    }
}
fn env_contracts(h: &Harness) -> Result<()> {
    use ara_ai::retry::idle::*;
    let import = |entries: &[(&str, &str)]| EnvBindings {
        inner: h.bindings.clone(),
        environment: entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    };
    let bindings = import(&[]);
    check(
        h.with(|r| get_stream_idle_timeout_ms(&h.factory, &bindings, None, r))? == Some(300_000.0),
        "Generic idle default changed",
    )?;
    check(
        h.with(|r| {
            get_stream_first_event_timeout_ms(&h.factory, &bindings, Some(500_000.0), None, r)
        })? == Some(500_000.0),
        "First-event idle floor missing",
    )?;
    let bindings = import(&[
        ("PI_STREAM_IDLE_TIMEOUT_MS", "22.9"),
        ("PI_OPENAI_STREAM_IDLE_TIMEOUT_MS", "55.9"),
    ]);
    check(
        h.with(|r| get_stream_idle_timeout_ms(&h.factory, &bindings, Some(99.0), r))? == Some(22.0),
        "Generic alias precedence/truncation changed",
    )?;
    check(
        h.with(|r| get_openai_stream_idle_timeout_ms(&h.factory, &bindings, Some(99.0), r))?
            == Some(55.0),
        "OpenAI alias precedence changed",
    )?;
    let bindings = import(&[("PI_STREAM_IDLE_TIMEOUT_MS", "Infinity")]);
    check(
        h.with(|r| get_stream_idle_timeout_ms(&h.factory, &bindings, Some(99.0), r))? == Some(99.0),
        "Nonfinite env did not use exact fallback",
    )?;
    let bindings = import(&[
        ("PI_STREAM_IDLE_TIMEOUT_MS", "0"),
        ("PI_STREAM_FIRST_EVENT_TIMEOUT_MS", "0"),
    ]);
    check(
        h.with(|r| get_stream_idle_timeout_ms(&h.factory, &bindings, None, r))?
            .is_none(),
        "Zero env did not disable idle",
    )?;
    check(
        h.with(|r| {
            get_openai_stream_first_event_timeout_ms(&h.factory, &bindings, Some(99.0), None, r)
        })? == Some(0.0),
        "OpenAI zero first-event sentinel was lost",
    )?;
    let bindings = import(&[
        ("PI_STREAM_FIRST_EVENT_TIMEOUT_MS", "12"),
        ("PI_OPENAI_STREAM_FIRST_EVENT_TIMEOUT_MS", "0"),
    ]);
    check(
        h.with(|r| {
            get_openai_stream_first_event_timeout_ms(&h.factory, &bindings, Some(99.0), None, r)
        })? == Some(0.0),
        "Explicit OpenAI zero was floored",
    )?;
    let bindings = import(&[("PI_STREAM_FIRST_EVENT_TIMEOUT_MS", "12")]);
    check(
        h.with(|r| {
            get_openai_stream_first_event_timeout_ms(&h.factory, &bindings, Some(99.0), None, r)
        })? == Some(99.0),
        "OpenAI idle floor ignored resolved caller budget",
    )?;
    Ok(())
}

struct CountResult {
    calls: Arc<AtomicUsize>,
    original: Arc<dyn ara_ai::retry::result::OriginalStreamResult>,
}
impl ara_ai::retry::result::OriginalStreamResult for CountResult {
    fn result(&self) -> futures::future::BoxFuture<'static, Result<ValueHandle>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.original.result()
    }
}
fn dispatch_source(
    h: &Harness,
    source: ara_ai::retry::result::ActualCompletionStream,
    result: Option<Arc<dyn ara_ai::retry::result::OriginalStreamResult>>,
) -> Result<ara_cli::retry_host::lazy_provider::LazyForwardRun> {
    use ara_cli::{model_registry_extensions::ExtensionRegistry, retry_host::lazy_provider::*};
    let provider = Arc::new(RegisteredProvider {
        source: std::sync::Mutex::new(Some(NativeLazySource {
            identity: h.heap().object()?,
            stream: source,
            local_work: None,
            original_result: result,
        })),
        signal: std::sync::Mutex::new(None),
    });
    let registry = ExtensionRegistry::new();
    set_bedrock_provider_module(
        &registry,
        Arc::new(RegisteredLazyProvider {
            provider,
            limits: LazyStreamLimits::default(),
            classify: h.heap().fresh_compiled_function(&CLASSIFY, &[])?,
        }),
    );
    Ok(dispatch_registered_lazy(&registry,&"bedrock-converse-stream".into(),h.factory.clone(),h.bindings.clone(),h.data(serde_json::json!({"id":"source","api":"bedrock-converse-stream","provider":"amazon-bedrock"}))?,h.data(serde_json::json!({"messages":[]}))?,h.data(serde_json::json!({"streamIdleTimeoutMs":1000}))?).expect("Actual source registry consumer dispatch"))
}
async fn lazy_source_contracts(h: &mut Harness) -> Result<()> {
    use ara_ai::retry::result::ActualCompletionStream;
    use ara_cli::retry_host::lazy_provider::collect_registered_lazy;
    let (mut producer, source, _) = ActualCompletionStream::channel(&h.factory);
    let original = h.heap().object()?;
    producer.end_original(original.clone())?;
    drop(producer);
    let calls = Arc::new(AtomicUsize::new(0));
    let result = Arc::new(CountResult {
        calls: calls.clone(),
        original: source.original_result.clone(),
    });
    let invocation = dispatch_source(h, source, Some(result))?;
    let result = invocation.stream.original_result.clone();
    let receipt = invocation.receipt.clone();
    check(
        h.run_until(collect_registered_lazy(invocation))
            .await?
            .is_empty(),
        "Result-only source manufactured terminal event",
    )?;
    check(
        h.run_until(result.result()).await?.same_identity(&original)
            && calls.load(Ordering::SeqCst) == 1,
        "Lazy EOF did not await source actual result exactly once",
    )?;
    check(
        receipt.wait_settled().await.is_empty(),
        "Result-only source lost owner settlement",
    )?;

    let (mut producer, source, _) = ActualCompletionStream::channel(&h.factory);
    let original_error = h.with(|r| {
        h.heap().construct(
            &h.bindings.error_constructor(),
            &[h.heap().string("original iterator failure".into())?],
            r,
        )
    })?;
    producer.fail_original(ValueError::Thrown(original_error.clone()))?;
    drop(producer);
    let calls = Arc::new(AtomicUsize::new(0));
    let result = Arc::new(CountResult {
        calls: calls.clone(),
        original: source.original_result.clone(),
    });
    let invocation = dispatch_source(h, source, Some(result))?;
    let receipt = invocation.receipt.clone();
    let events = h.run_until(collect_registered_lazy(invocation)).await?;
    check(
        events.len() == 1 && events[0].is_terminal() && calls.load(Ordering::SeqCst) == 0,
        "Original iterator throw was replaced by EOF/result fallback",
    )?;
    check(
        h.s(&h.get(events[0].partial(), "errorMessage")?)? == "original iterator failure",
        "Original iterator failure message changed",
    )?;
    check(
        receipt
            .wait_settled()
            .await
            .iter()
            .any(|e| matches!(&**e,ValueError::Thrown(v)if v.same_identity(&original_error))),
        "Original iterator throw receipt lost",
    )?;

    let (mut producer, source, source_receipt) = ActualCompletionStream::channel(&h.factory);
    producer.end_original(h.heap().object()?)?;
    let result = source.original_result.clone();
    let invocation = dispatch_source(h, source, Some(result))?;
    let receipt = invocation.receipt.clone();
    let events = h.run_until(collect_registered_lazy(invocation)).await?;
    check(
        events.is_empty() && !receipt.is_settled(),
        "EOF was incorrectly treated as provider resource settlement",
    )?;
    let original_late = h.heap().object()?;
    source_receipt.record(ValueError::Thrown(original_late.clone()));
    drop(producer);
    check(
        receipt
            .wait_settled()
            .await
            .iter()
            .any(|e| matches!(&**e,ValueError::Thrown(v)if v.same_identity(&original_late))),
        "Late source failure vanished after EOF",
    )?;

    let (mut producer, source, _) = ActualCompletionStream::channel(&h.factory);
    producer.end_original(h.heap().object()?)?;
    drop(producer);
    let invocation = dispatch_source(h, source, None)?;
    let result = invocation.stream.original_result.clone();
    check(
        h.run_until(collect_registered_lazy(invocation))
            .await?
            .is_empty(),
        "Source without result method manufactured event",
    )?;
    let outcome = h.run_until(async { Ok(result.result().await) }).await?;
    let Err(ValueError::Thrown(error)) = outcome else {
        return Err(ValueError::UnsupportedOperation(
            "Result-less source end became success",
        ));
    };
    check(
        h.s(&h.get(&error, "message")?)? == "Stream ended without a final result",
        "Missing result original envelope message changed",
    )?;
    Ok(())
}

/// Actual NativeRun cancellation with no caller signal and no watchdogs. The
/// sole fresh Session Run driver is production NativeRun::drive, not Harness.
async fn native_run_cancel_contract(eof_pending_result: bool) -> Result<()> {
    use ara_ai::retry::RetryBindings;
    use ara_ai::retry::result::{ActualCompletionStream, NativeOriginalStreamResult};
    use ara_cli::{
        actual_tools::BuiltinToolSource,
        model_registry_extensions::ExtensionRegistry,
        native_factory::{NativeFactoryImports, PreparedNativeSession},
        native_realm::NativeRealm,
        native_run::NativeRun,
        retry_host::{NativeRetryBindings, lazy_provider::*},
    };
    let realm = NativeRealm::prepare()?;
    let root = std::env::current_dir().map_err(|_| {
        ValueError::UnsupportedOperation("Native cancellation fixture cwd unavailable")
    })?;
    let source = BuiltinToolSource::new(&realm.values, ara_tools::ToolContext::new(root), &[])
        .expect("Actual original builtin Host preparation");
    let prepared = PreparedNativeSession::from_complete_bindings(
        realm.clone(),
        NativeFactoryImports {
            source,
            string_function: realm.globals.string.clone(),
            bun: realm.globals.bun.clone(),
            original_logger: None,
        },
    )
    .expect("Actual retained Native Session bundle");
    let cancel = tokio_util::sync::CancellationToken::new();
    let native_run = NativeRun::begin(prepared, cancel.clone()).expect("One original Native Run");
    let lease = native_run.lease();
    let factory = realm.values.factory().clone();
    let calling = native_run.calling_run(&factory)?;
    let bindings = NativeRetryBindings::prepare(
        &factory,
        &realm,
        calling.clone(),
        ara_ai::ActualStreamFailureReceipt::default(),
    )?;
    let (mut producer, source, original_receipt) = ActualCompletionStream::channel(&factory);
    let result_calls = Arc::new(AtomicUsize::new(0));
    let original_result = if eof_pending_result {
        // Original native method returns an actual never-settled Promise.
        // Event EOF is already delivered, independently of that Promise.
        static PENDING_EXECUTOR: CompiledFunctionSpec = CompiledFunctionSpec {
            source: "Original result Promise executor retains an unresolved result",
            name: "",
            length: 2,
            capture_count: 0,
            form: CompiledFunctionForm::Arrow,
            body: |heap, _, _, _, _| heap.undefined(),
        };
        let native_source = calling.with_runtime(&factory, &mut |runtime| {
            let heap = factory.heap();
            let promise = heap.get(&bindings.globals(), &"Promise".into(), runtime)?;
            let pending = heap.construct(
                &promise,
                &[heap.fresh_compiled_function(&PENDING_EXECUTOR, &[])?],
                runtime,
            )?;
            let object = heap.object()?;
            heap.define_own_data(
                &object,
                "result".into(),
                heap.fresh_compiled_function(&RETURN_CAPTURE, &[pending])?,
                runtime,
            )?;
            Ok(object)
        })?;
        let imported: Arc<dyn ara_ai::retry::RetryBindings> = bindings.clone();
        let original: Arc<dyn ara_ai::retry::result::OriginalStreamResult> = Arc::new(
            NativeOriginalStreamResult::prepare(&factory, &imported, native_source)?,
        );
        producer.end_original(factory.heap().object()?)?;
        Arc::new(CountResult {
            calls: result_calls.clone(),
            original,
        }) as Arc<dyn ara_ai::retry::result::OriginalStreamResult>
    } else {
        source.original_result.clone()
    };
    let provider = Arc::new(RegisteredProvider {
        source: std::sync::Mutex::new(Some(NativeLazySource {
            identity: factory.heap().object()?,
            original_result: Some(original_result),
            stream: source,
            local_work: None,
        })),
        signal: std::sync::Mutex::new(None),
    });
    let registry = ExtensionRegistry::new();
    set_bedrock_provider_module(
        &registry,
        Arc::new(RegisteredLazyProvider {
            provider,
            limits: LazyStreamLimits {
                provider_handles_stream_timeouts: true,
                ..LazyStreamLimits::default()
            },
            classify: factory.heap().fresh_compiled_function(&CLASSIFY, &[])?,
        }),
    );
    let invocation=dispatch_registered_lazy_in_run(&native_run,&registry,&"bedrock-converse-stream".into(),factory.clone(),bindings,factory.heap().import_serde(&serde_json::json!({"id":"never-emit","api":"bedrock-converse-stream","provider":"amazon-bedrock"}))?,factory.heap().import_serde(&serde_json::json!({"messages":[]}))?,factory.heap().object()?).expect("Actual reserved Run source dispatch");
    let original_failure = factory.heap().object()?;
    let source_failure = original_failure.clone();
    let source_receipt = original_receipt.clone();
    let source_cancel = cancel.clone();
    let producer_owner = tokio::spawn(async move {
        // Pure Rust resource owner: it never emits an event and holds the
        // original settlement guard until the reader is actually closed.
        if eof_pending_result {
            // Events have ended but the actual resource owner is still live.
            source_cancel.cancelled().await;
            source_receipt.record(ValueError::Thrown(source_failure));
        } else {
            producer.closed().await;
            producer
                .fail_original(ValueError::Thrown(source_failure))
                .expect("Original producer cancellation failure retained");
        }
        drop(producer);
        source_receipt.is_settled()
    });
    let drive = native_run.drive(collect_registered_lazy(invocation));
    tokio::pin!(drive);
    check(
        futures::poll!(drive.as_mut()).is_pending(),
        "Never-emitting source was already completed before actual Run cancel",
    )?;
    check(
        !original_receipt.is_settled(),
        "Original producer was settled before reader cancellation",
    )?;
    check(
        result_calls.load(Ordering::SeqCst) == usize::from(eof_pending_result),
        "Original EOF result method was not entered exactly once before cancel",
    )?;
    cancel.cancel();
    let outcome=tokio::time::timeout(Duration::from_secs(5),drive).await.map_err(|_|ValueError::UnsupportedOperation("Actual NativeRun cancel did not close never-emitting source without a signal/timeout"))?;
    check(outcome.is_err(), "Cancelled real Run became success")?;
    check(
        producer_owner
            .await
            .map_err(|_| ValueError::UnsupportedOperation("Original producer owner panicked"))?,
        "Original producer receipt was not sealed after its resource owner ended",
    )?;
    check(
        original_receipt
            .wait_settled()
            .await
            .iter()
            .any(|e| matches!(&**e,ValueError::Thrown(v)if v.same_identity(&original_failure))),
        "Original Run cancellation lost late source error identity",
    )?;
    check(
        lease.settle().await.is_err(),
        "Independent original lease lost cancellation source receipt on second settlement",
    )?;
    check(
        result_calls.load(Ordering::SeqCst) == usize::from(eof_pending_result),
        "Original result was replayed during Run cancellation",
    )?;
    eprintln!(
        "ARA-RETRY SOURCE PASS {}",
        if eof_pending_result {
            "native-run-eof-result-cancel"
        } else {
            "native-run-cancel"
        }
    );
    Ok(())
}


static LOCAL_CLOCK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "b91 local work original Date.now receiver/raw return and zero-only call observer",
    name: "now", length: 0, capture_count: 3, form: CompiledFunctionForm::Builtin,
    body: |heap, captures, receiver, _, runtime| {
        if !receiver.same_identity(&captures[2]) {
            return Err(ValueError::UnsupportedOperation("LocalWork clock lost Date receiver"));
        }
        let count = heap.to_number(&heap.get(&captures[0], &"calls".into(), runtime)?, runtime)?;
        heap.define_own_data(&captures[0], "calls".into(), heap.number(count + 1.0)?, runtime)?;
        Ok(captures[1].clone())
    },
};

/// One original-driver bank: real adopted Promise settlement, raw clock value,
/// finally rejection identity and subsequent driver usability. No worker clock.
async fn upgraded_local_work_contracts(h: &mut Harness) -> Result<()> {
    use ara_ai::{event::LocalWorkTracker, graph_values::GraphExecution};
    let tracker = LocalWorkTracker::default();
    let date = h.get(&h.bindings.globals(), "Date")?;
    let original_now = h.get(&date, "now")?;
    let clock_state = h.object(&[("calls", h.heap().number(0.0)?)])?;
    let coercion_error = h.heap().object()?;
    let raw_clock = h.object(&[("valueOf", h.heap().fresh_compiled_function(&THROW, &[coercion_error.clone()])?)])?;
    let clock = h.heap().fresh_compiled_function(&LOCAL_CLOCK,
        &[clock_state.clone(), raw_clock.clone(), date.clone()])?;
    h.put(&date, "now", clock)?;
    let first = h.driver.admission().prepare_pending(h.driver.root_scope(), h.driver.root_parent())?;
    let second = h.driver.admission().prepare_pending(h.driver.root_scope(), h.driver.root_parent())?;
    let tracked_first = tracker.track_in_original(|ticket| h.bindings.adopt_local_work(h.factory.clone(), first.promise, ticket));
    let tracked_second = tracker.track_in_original(|ticket| h.bindings.adopt_local_work(h.factory.clone(), second.promise, ticket));
    let first_value = h.heap().object()?;
    h.with(|r| h.heap().call(&first.resolve, &h.heap().undefined()?, &[first_value.clone()], r))?;
    let got = h.run_until(tracked_first.wait()).await?;
    check(got.same_identity(&first_value) && tracker.has_pending_local_work()?
        && h.n(&h.get(&clock_state, "calls")?)? == 0.0,
        "Nonzero LocalWork transition read clock or changed result")?;
    let original_rejection = h.heap().object()?;
    h.with(|r| h.heap().call(&second.reject, &h.heap().undefined()?, &[original_rejection.clone()], r))?;
    let outcome = h.run_until(async { Ok(tracked_second.wait().await) }).await?;
    check(matches!(outcome,Err(ValueError::Thrown(ref v)) if v.same_identity(&original_rejection))
        && !tracker.has_pending_local_work()? && h.n(&h.get(&clock_state, "calls")?)? == 1.0,
        "Raw Date.now was coerced early or original rejection/count was lost")?;
    let timestamp = h.with(|r| tracker.local_work_settled_at_in(GraphExecution::new(h.heap(), r), &h.bindings.globals()));
    check(matches!(timestamp,Err(ValueError::Thrown(ref v)) if v.same_identity(&coercion_error)),
        "Original Math.max did not coerce the retained raw clock value")?;
    h.put(&raw_clock, "valueOf", h.heap().fresh_compiled_function(&RETURN_CAPTURE, &[h.heap().number(7000.0)?])?)?;
    let timestamp = h.with(|r| tracker.local_work_settled_at_in(GraphExecution::new(h.heap(), r), &h.bindings.globals()))?;
    check(h.n(&timestamp)? == 7000.0, "LocalWork stored a copy instead of original raw clock identity")?;

    let clock_error = h.heap().object()?;
    h.put(&date, "now", h.heap().fresh_compiled_function(&THROW, &[clock_error.clone()])?)?;
    for reject_original in [false, true] {
        let work = h.driver.admission().prepare_pending(h.driver.root_scope(), h.driver.root_parent())?;
        let tracked = tracker.track_in_original(|ticket| h.bindings.adopt_local_work(h.factory.clone(), work.promise, ticket));
        let resolver = if reject_original { work.reject } else { work.resolve };
        h.with(|r| h.heap().call(&resolver, &h.heap().undefined()?, &[h.heap().object()?], r))?;
        let outcome = h.run_until(async { Ok(tracked.wait().await) }).await?;
        check(matches!(outcome,Err(ValueError::Thrown(ref v)) if v.same_identity(&clock_error)) && !tracker.has_pending_local_work()?,
            "Original finally clock throw did not override tracked outcome")?;
    }
    // An ordinary JS finally rejection must not revoke the entire driver.
    h.put(&date, "now", original_now.clone())?;
    let still = h.driver.admission().prepare_pending(h.driver.root_scope(), h.driver.root_parent())?;
    h.with(|r| h.heap().call(&still.resolve, &h.heap().undefined()?, &[h.heap().number(17.0)?], r))?;
    let value = h.run_until(h.bindings.await_value(h.factory.clone(), still.promise)).await?;
    check(h.n(&value)? == 17.0, "Clock throw revoked original Promise driver")?;
    let retained = h.with(|r| tracker.local_work_settled_at_in(GraphExecution::new(h.heap(), r), &h.bindings.globals()))?;
    check(h.n(&retained)? == 7000.0, "Clock throw erased previous settlement timestamp")?;
    let delegate_calls = Arc::new(AtomicUsize::new(0));
    tracker.forward_local_work_from(Some(Arc::new(DelegateWork {
        pending: Err(ValueError::Thrown(coercion_error.clone())),
        timestamp: Ok(h.heap().number(9000.0)?), calls: delegate_calls.clone(),
    })));
    let outstanding = tracker.begin_local_work();
    check(tracker.has_pending_local_work()? && delegate_calls.load(Ordering::SeqCst) == 0,
        "Local pending did not short-circuit delegate getter")?;
    // Complete this declared local operation in the same runtime, retaining the
    // previous raw clock fixture so subsequent delegate max is deterministic.
    h.put(&date, "now", h.heap().fresh_compiled_function(&RETURN_CAPTURE, &[raw_clock.clone()])?)?;
    let observation = h.with(|r| Ok(outstanding.finish_in_original(GraphExecution::new(h.heap(), r), &h.bindings.globals(), Ok(h.heap().undefined()?))))?;
    observation.result?;
    check(matches!(tracker.has_pending_local_work(), Err(ValueError::Thrown(ref v)) if v.same_identity(&coercion_error)),
        "Delegate pending getter exception was swallowed")?;
    let delegated = h.with(|r| tracker.local_work_settled_at_in(GraphExecution::new(h.heap(), r), &h.bindings.globals()))?;
    check(h.n(&delegated)? == 9000.0, "Local and delegate did not use original Math.max")?;
    tracker.forward_local_work_from(Some(Arc::new(DelegateWork {
        pending: Ok(false), timestamp: Ok(h.heap().number(f64::NAN)?), calls: delegate_calls.clone(),
    })));
    let nan = h.with(|r| tracker.local_work_settled_at_in(GraphExecution::new(h.heap(), r), &h.bindings.globals()))?;
    check(h.n(&nan)?.is_nan(), "Native Math.max NaN was suppressed by Rust max")?;
    tracker.forward_local_work_from(Some(Arc::new(DelegateWork {
        pending: Ok(false), timestamp: Err(ValueError::Thrown(coercion_error.clone())), calls: delegate_calls,
    })));
    let error = h.with(|r| tracker.local_work_settled_at_in(GraphExecution::new(h.heap(), r), &h.bindings.globals()));
    check(matches!(error,Err(ValueError::Thrown(ref v)) if v.same_identity(&coercion_error)), "Delegate timestamp getter throw was swallowed")?;
    tracker.forward_local_work_from(None);
    let retained = h.with(|r| tracker.local_work_settled_at_in(GraphExecution::new(h.heap(), r), &h.bindings.globals()))?;
    check(h.n(&retained)? == 7000.0, "Delegate detach erased own settlement")?;
    h.put(&date, "now", original_now)?;
    eprintln!("ARA-RETRY-UPGRADE PASS localwork-original-finally-rawclock");
    Ok(())
}


static BUDGET_NEXT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Provider responds after old timeout but within full budget after local settlement; later genuinely stalls",
    name: "next", length: 0, capture_count: 3, form: CompiledFunctionForm::Builtin,
    body: |heap, c, _, _, r| {
        let count = heap.to_number(&heap.get(&c[0], &"calls".into(), r)?, r)?;
        heap.define_own_data(&c[0], "calls".into(), heap.number(count + 1.0)?, r)?;
        let first = heap.truthy(&heap.get(&c[0], &"firstMode".into(), r)?)?;
        if count == (if first { 0.0 } else { 1.0 }) { return Ok(c[1].clone()); }
        if first || count > 1.0 { return Ok(c[2].clone()); }
        let promise = heap.get(&c[0], &"Promise".into(), r)?;
        let resolve = heap.get(&promise, &"resolve".into(), r)?;
        let item = heap.object()?;
        heap.define_own_data(&item, "done".into(), heap.boolean(false)?, r)?;
        heap.define_own_data(&item, "value".into(), heap.string("initial".into())?, r)?;
        heap.call(&resolve, &promise, &[item], r)
    },
};
async fn settled_budget_contract(h: &mut Harness, first: bool) -> Result<()> {
    let work = h.driver.admission().prepare_pending(h.driver.root_scope(), h.driver.root_parent())?;
    let never = h.driver.admission().prepare_pending(h.driver.root_scope(), h.driver.root_parent())?;
    let local = h.object(&[("hasPendingLocalWork", h.heap().boolean(false)?), ("localWorkSettledAt", h.heap().number(0.0)?)])?;
    let state = h.object(&[("calls", h.heap().number(0.0)?), ("returns", h.heap().number(0.0)?),
        ("firstMode", h.heap().boolean(first)?), ("Promise", h.get(&h.bindings.globals(), "Promise")?)])?;
    let source = h.heap().object()?;
    h.with(|r| h.heap().define_own_data(&source, h.bindings.async_iterator_key(), h.heap().fresh_compiled_function(&SELF, &[])?, r))?;
    h.put(&source, "next", h.heap().fresh_compiled_function(&BUDGET_NEXT, &[state.clone(), work.promise, never.promise])?)?;
    h.put(&source, "return", h.heap().fresh_compiled_function(&RETURN, &[state.clone()])?)?;
    let options = h.object(&[("idleTimeoutMs", h.heap().number(80.0)?), ("firstItemTimeoutMs", h.heap().number(80.0)?),
        ("localWork", local.clone()), ("errorMessage", h.heap().string("real-provider-stall".into())?)])?;
    let mut idle = IdleIterator::prepare(h.factory.clone(), h.bindings.clone(), source, options)?;
    if !first { check(h.run_until(idle.next()).await?.is_some(), "Idle budget initial item missing")?; }
    let factory = h.factory.clone(); let calling = h.frozen.clone(); let globals = h.bindings.globals();
    let settlement = h.bindings.timers().arm(55.0, Arc::new(move || {
        calling.with_runtime(&factory, &mut |runtime| {
            let heap = factory.heap();
            let date = heap.get(&globals, &"Date".into(), runtime)?;
            let now = heap.get(&date, &"now".into(), runtime)?;
            let value = heap.call(&now, &date, &[], runtime)?;
            heap.define_own_data(&local, "localWorkSettledAt".into(), value, runtime)?;
            heap.undefined()
        })?; Ok(())
    }))?;
    let factory = h.factory.clone(); let calling = h.frozen.clone(); let resolver = work.resolve;
    let response = h.bindings.timers().arm(100.0, Arc::new(move || {
        calling.with_runtime(&factory, &mut |runtime| {
            let heap = factory.heap(); let item = heap.object()?;
            heap.define_own_data(&item, "done".into(), heap.boolean(false)?, runtime)?;
            heap.define_own_data(&item, "value".into(), heap.string("after-local-work".into())?, runtime)?;
            heap.call(&resolver, &heap.undefined()?, &[item], runtime)
        })?; Ok(())
    }))?;
    let value = h.run_until(idle.next()).await?.ok_or(ValueError::InvalidNode)?;
    check(h.s(&value)? == "after-local-work", "Completed local work did not restore full first/idle budget")?;
    let result = h.run_until(async { Ok(idle.next().await) }).await?;
    check(matches!(result, Err(ValueError::Thrown(_))), "Provider stall after work budget became infinite success")?;
    settlement.clear()?; response.clear()?;
    check(h.n(&h.get(&state, "returns")?)? == 1.0, "Budget extension duplicated source return")?;
    eprintln!("ARA-RETRY-UPGRADE PASS localwork-full-budget first={first}");
    Ok(())
}

async fn early_terminal_return_contract(h: &mut Harness) -> Result<()> {
    use ara_ai::retry::idle::TerminalGraceIterator;
    for await_drain in [false, true] {
        let (source, state) = queue_source(h, vec![h.heap().string("accepted".into())?, h.heap().string("tail".into())?], true)?;
        let options = h.object(&[("finishedAtMs", h.heap().fresh_compiled_function(&RETURN_CAPTURE, &[h.heap().number(native_now(h)?)?])?),
            ("graceMs", h.heap().number(30.0)?), ("awaitDrainOnReturn", h.heap().boolean(await_drain)?)])?;
        let mut iterator = TerminalGraceIterator::prepare(h.factory.clone(), h.bindings.clone(), source, options)?;
        check(h.run_until(iterator.next()).await?.is_some(), "Terminal accepted yield missing")?;
        let returned = iterator.prepare_consumer_return()?;
        check(returned.await_drain == await_drain && h.n(&h.get(&state, "calls")?)? == 1.0,
            "Consumer return pulled tail during synchronous guard or ignored await flag")?;
        // Both branches are driven by this SAME original driver. The Host
        // production owner retains the false branch after logical completion.
        h.run_until(returned.drain).await?;
        check(h.n(&h.get(&state, "calls")?)? == 3.0 && h.n(&h.get(&state, "returns")?)? == 1.0,
            "Early return did not drain original source through fixed grace and one return")?;
    }
    for throw_in_wait_get in [false, true] {
        let (source, state) = queue_source(h, vec![h.heap().string("accepted".into())?], true)?;
        let error = h.heap().object()?;
        let options = h.object(&[("finishedAtMs", h.heap().fresh_compiled_function(&RETURN_CAPTURE, &[h.heap().number(native_now(h)?)?])?),
            ("graceMs", h.heap().number(1000.0)?), ("awaitDrainOnReturn", h.heap().boolean(false)?)])?;
        let mut iterator = TerminalGraceIterator::prepare(h.factory.clone(), h.bindings.clone(), source, options.clone())?;
        check(h.run_until(iterator.next()).await?.is_some(), "Guard fixture missing accepted yield")?;
        let throwing = h.heap().fresh_compiled_function(&THROW, &[error.clone()])?;
        if throw_in_wait_get { source_accessor(h, &options, "awaitDrainOnReturn", throwing)?; }
        else { h.put(&options, "finishedAtMs", throwing)?; }
        let failed = iterator.prepare_consumer_return();
        check(matches!(failed, Err(ValueError::Thrown(ref v)) if v.same_identity(&error))
            && h.n(&h.get(&state, "returns")?)? == 0.0,
            "Throwing source finally guard invented iterator.return or replaced original error")?;
    }
    eprintln!("ARA-RETRY-UPGRADE PASS original-terminal-return-drain");
    Ok(())
}


struct DelegateWork {
    pending: Result<bool>,
    timestamp: Result<ValueHandle>,
    calls: Arc<AtomicUsize>,
}
impl LocalWorkSource for DelegateWork {
    fn has_pending_local_work(&self) -> Result<bool> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.pending.clone()
    }
    fn local_work_settled_at_in(&self, _: ara_ai::graph_values::GraphExecution<'_>, _: &ValueHandle) -> Result<ValueHandle> {
        self.timestamp.clone()
    }
}

fn cap_io(error: impl std::fmt::Display) -> ValueError {
    ValueError::TypeError(format!("Provider cap Host fixture: {error}"))
}
fn cap_temp() -> Result<tempfile::TempDir> {
    let parent = std::env::var_os("ARA_RETRY_CAP_TEST_ROOT").map(std::path::PathBuf::from)
        .unwrap_or_else(|| if cfg!(windows) { "D:/workspace/env/cache/ara-retry-cap-tests".into() } else { std::env::temp_dir() });
    std::fs::create_dir_all(&parent).map_err(cap_io)?;
    tempfile::Builder::new().prefix("source-").tempdir_in(parent).map_err(cap_io)
}
async fn cap_paths(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    let mut paths = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await.map_err(cap_io)?;
    while let Some(entry) = entries.next_entry().await.map_err(cap_io)? {
        if entry.file_type().await.map_err(cap_io)?.is_dir() { paths.push(entry.path()); }
    }
    Ok(paths)
}
async fn cap_notice(notice: &tokio::sync::Notify) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(3), notice.notified()).await.map_err(cap_io)
}
async fn cap_until(mut condition: impl FnMut() -> bool) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() { tokio::time::sleep(Duration::from_millis(2)).await; }
    }).await.map_err(cap_io)
}
fn cap_model() -> ara_ai::Model {
    ara_ai::Model { id: "m".into(), api: "openai-completions".into(), provider: "cap-source-bank".into(),
        base_url: "http://controlled-provider.invalid".into(), reasoning: false, max_tokens: None,
        context_window: None, tokenizer: None }
}
fn cap_context() -> ara_ai::Context {
    ara_ai::Context { system_prompt: vec![], messages: vec![], tools: None }
}
struct CapConfiguration(ara_cli::provider_inflight::ProviderInFlightConfiguration);
impl Drop for CapConfiguration {
    fn drop(&mut self) { ara_cli::provider_inflight::configure_provider_inflight(self.0.clone()); }
}
fn cap_configure(owner: Arc<ara_cli::provider_inflight::ProviderInFlightOwner>) -> CapConfiguration {
    use ara_cli::provider_inflight::*;
    let previous = configuration();
    configure_provider_inflight(ProviderInFlightConfiguration {
        owner, limits: [("cap-source-bank".into(), 1.0)].into_iter().collect(),
    });
    CapConfiguration(previous)
}
/// Controlled provider work is gated, never provider admission. Every permit,
/// lock, heartbeat, stale cleanup and wake below uses the production OS owner.
struct CapSource {
    calls: AtomicUsize, active: Arc<AtomicUsize>, maximum: AtomicUsize,
    gate: Option<Arc<tokio::sync::Notify>>, error: bool,
}
impl CapSource {
    fn new(gate: Option<Arc<tokio::sync::Notify>>, error: bool) -> Arc<Self> {
        Arc::new(Self { calls: AtomicUsize::new(0), active: Arc::new(AtomicUsize::new(0)), maximum: AtomicUsize::new(0), gate, error })
    }
}
impl ara_ai::ModelProvider for CapSource {
    fn stream(&self, model: &ara_ai::Model, _: &ara_ai::Context, options: ara_ai::CallOptions) -> ara_ai::AssistantStream {
        assert!(options.wait_for_terminal_drain, "real cap did not propagate awaited transport contract");
        self.calls.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(active, Ordering::SeqCst);
        let count = self.active.clone(); let gate = self.gate.clone(); let error = self.error;
        let mut message = ara_ai::AssistantMessage::empty(&model.api, &model.provider, &model.id);
        message.content.push(ara_ai::AssistantBlock::text("accepted-original-provider"));
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            if let Some(gate) = gate { gate.notified().await; }
            count.fetch_sub(1, Ordering::SeqCst);
            let event = if error {
                message.stop_reason = ara_ai::StopReason::Error;
                message.error_message = Some("original-provider-failure".into());
                ara_ai::AssistantMessageEvent::Error { reason: ara_ai::StopReason::Error, error: message }
            } else {
                message.stop_reason = ara_ai::StopReason::Stop;
                ara_ai::AssistantMessageEvent::Done { reason: ara_ai::StopReason::Stop, message }
            };
            let _ = tx.send(event).await;
        });
        rx
    }
}
async fn cap_terminal(mut stream: ara_ai::AssistantStream) -> Result<ara_ai::AssistantMessageEvent> {
    tokio::time::timeout(Duration::from_secs(4), async {
        let mut terminal = None;
        while let Some(event) = stream.recv().await { if event.is_terminal() { terminal = Some(event); } }
        terminal.ok_or(ValueError::UnsupportedOperation("Real cap lost source terminal"))
    }).await.map_err(cap_io)?
}
async fn cap_info(path: &std::path::Path, pid: u32, timestamp: f64, token: &str) -> Result<()> {
    tokio::fs::create_dir_all(path).await.map_err(cap_io)?;
    tokio::fs::write(path.join("info.json"), serde_json::to_vec(&serde_json::json!({"pid":pid,"timestamp":timestamp,"token":token})).map_err(cap_io)?).await.map_err(cap_io)
}

async fn provider_inflight_source_contracts(h:&mut Harness) -> Result<()> {
    use ara_cli::provider_inflight::{ProviderInFlightOwner as Owner, ProviderInFlightTestHooks as Hooks, wrap_model_provider};
    use tokio_util::sync::CancellationToken;
    // CAP21-01: source calls serialized by actual shared filesystem capacity.
    {
        let root = cap_temp()?; let waited = Arc::new(tokio::sync::Notify::new()); let notice = waited.clone();
        let owner = Owner::new(root.path().into()).with_test_hooks(Hooks { wait_observer: Some(Arc::new(move |_| notice.notify_one())), ..Default::default() });
        let _configuration = cap_configure(owner.clone());
        let gate = Arc::new(tokio::sync::Notify::new()); let source = CapSource::new(Some(gate.clone()), false);
        let provider = wrap_model_provider(source.clone());
        let a = provider.stream(&cap_model(), &cap_context(), Default::default());
        cap_until(|| source.calls.load(Ordering::SeqCst) == 1).await?;
        let b = provider.stream(&cap_model(), &cap_context(), Default::default());
        cap_notice(&waited).await?;
        check(source.calls.load(Ordering::SeqCst) == 1, "Same-provider request bypassed filesystem cap")?;
        gate.notify_one(); cap_terminal(a).await?;
        cap_until(|| source.calls.load(Ordering::SeqCst) == 2).await?;
        gate.notify_one(); cap_terminal(b).await?;
        check(source.maximum.load(Ordering::SeqCst) == 1, "Real cap allowed overlapping provider source calls")?;
        eprintln!("ARA-CAP21 PASS 01");
    }
    // CAP21-03/04/05: explicit release before outer terminal, and cleanup faults
    // preserve the source response/error rather than manufacturing a retry.
    for failure in [None, Some(false), Some(true)] {
        let root = cap_temp()?;
        let removals = Arc::new(AtomicUsize::new(0)); let count = removals.clone();
        let owner = Owner::new(root.path().into()).with_test_hooks(Hooks {
            lease_remover: Some(Arc::new(move |path| { let count = count.clone(); Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                if failure.is_some() { Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "controlled lease cleanup failure")) }
                else { tokio::fs::remove_dir_all(path).await }
            }) })), ..Default::default()
        });
        let _configuration = cap_configure(owner.clone());
        let source = CapSource::new(None, failure == Some(true)); let provider = wrap_model_provider(source.clone());
        let terminal = cap_terminal(provider.stream(&cap_model(), &cap_context(), Default::default())).await?;
        check(removals.load(Ordering::SeqCst) == 1 && source.calls.load(Ordering::SeqCst) == 1,
            "Outer terminal preceded one authentic release or retried provider")?;
        let message = terminal.partial();
        match failure {
            None => {
                check(cap_paths(&owner.provider_dir("cap-source-bank")).await?.is_empty(), "Terminal retained completed lease")?;
                cap_terminal(provider.stream(&cap_model(), &cap_context(), Default::default())).await?;
                eprintln!("ARA-CAP21 PASS 03");
            }
            Some(false) => { check(message.stop_reason == ara_ai::StopReason::Stop && message.text() == "accepted-original-provider", "Cleanup error replaced accepted response")?; eprintln!("ARA-CAP21 PASS 04"); }
            Some(true) => { check(message.stop_reason == ara_ai::StopReason::Error && message.error_message.as_deref() == Some("original-provider-failure"), "Cleanup error replaced original provider failure")?; eprintln!("ARA-CAP21 PASS 05"); }
        }
    }
    // CAP21-06/07: late heartbeat cannot recreate or refresh stopped lease.
    for retain_directory in [false, true] {
        let root = cap_temp()?; let entered = Arc::new(tokio::sync::Notify::new()); let release = Arc::new(tokio::sync::Notify::new()); let done = Arc::new(tokio::sync::Notify::new());
        let start = entered.clone(); let gate = release.clone(); let finish = done.clone();
        let owner = Owner::new(root.path().into()).with_test_hooks(Hooks {
            heartbeat_ms: Some(1), heartbeat_flush_timeout_ms: Some(1),
            heartbeat_writer: Some(Arc::new(move |write| { let start=start.clone(); let gate=gate.clone(); let finish=finish.clone(); Box::pin(async move {
                start.notify_one(); gate.notified().await; let result=write.await; finish.notify_one(); result
            }) })),
            lease_remover: if retain_directory { Some(Arc::new(|_| Box::pin(async { Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "retain stopped lease")) }))) } else { None },
            ..Default::default()
        });
        let permit = owner.acquire("heartbeat", 1, CancellationToken::new()).await.map_err(cap_io)?;
        cap_notice(&entered).await?;
        let path = cap_paths(&owner.provider_dir("heartbeat")).await?.pop().ok_or(ValueError::InvalidNode)?;
        let before = tokio::fs::read(path.join("info.json")).await.map_err(cap_io)?;
        let released = permit.release_once().await;
        check(released.is_err() == retain_directory, "Heartbeat cleanup result changed")?;
        release.notify_one(); cap_notice(&done).await?;
        if retain_directory { check(tokio::fs::read(path.join("info.json")).await.map_err(cap_io)? == before, "Stopped late heartbeat refreshed surviving lease")?; }
        else { check(!path.exists(), "Stopped late heartbeat recreated released lease")?; }
        eprintln!("ARA-CAP21 PASS {}", if retain_directory { "07" } else { "06" });
    }
    // CAP21-08/10/11/13: queue cancellation, no spurious wake and unreadable
    // fresh lease conservatively counts active. Real files remain the gate.
    for malformed in [false, true] {
        let root = cap_temp()?; let waited = Arc::new(tokio::sync::Notify::new()); let notice = waited.clone();
        let owner = Owner::new(root.path().into()).with_test_hooks(Hooks { wait_observer: Some(Arc::new(move |_| notice.notify_one())), ..Default::default() });
        let directory = owner.provider_dir("cap-source-bank");
        let holder = if malformed {
            tokio::fs::create_dir_all(directory.join("fresh-unreadable")).await.map_err(cap_io)?;
            tokio::fs::write(directory.join("fresh-unreadable/info.json"), b"{not-json").await.map_err(cap_io)?;
            cap_old_directory(&directory.join("fresh-unreadable"))?;
            None
        } else {
            let permit = owner.acquire("cap-source-bank", 1, CancellationToken::new()).await.map_err(cap_io)?;
            check(!directory.join(".wakeup").exists(), "Acquiring an unused slot signaled waiters")?;
            eprintln!("ARA-CAP21 PASS 11"); Some(permit)
        };
        let _configuration = cap_configure(owner.clone()); let source = CapSource::new(None, false); let provider = wrap_model_provider(source.clone());
        let cancel = CancellationToken::new();
        let stream = provider.stream(&cap_model(), &cap_context(), ara_ai::CallOptions { cancel: cancel.clone(), ..Default::default() });
        cap_notice(&waited).await?; cancel.cancel();
        let terminal = cap_terminal(stream).await?;
        check(source.calls.load(Ordering::SeqCst) == 0 && terminal.partial().stop_reason == ara_ai::StopReason::Aborted,
            "Aborted queued request dispatched or lost abort category")?;
        check(!directory.join(".wakeup").exists(), "Full-limit attempt signaled without freeing slot")?;
        if let Some(holder) = holder { holder.release_once().await.map_err(cap_io)?; eprintln!("ARA-CAP21 PASS 08 10"); }
        else { check(directory.join("fresh-unreadable/info.json").exists(), "Fresh malformed lease was reaped")?; eprintln!("ARA-CAP21 PASS 13"); }
    }
    // CAP21-12/14/15: live PID wins over old timestamp; stale observation and
    // failed-info cleanup cannot delete a replacement real directory identity.
    {
        let root=cap_temp()?; let waited=Arc::new(tokio::sync::Notify::new()); let notice=waited.clone();
        let owner=Owner::new(root.path().into()).with_test_hooks(Hooks { wait_observer:Some(Arc::new(move |_|notice.notify_one())),..Default::default() });
        let lock=owner.provider_dir("lock").with_extension("lock");
        cap_info(&lock,std::process::id(),0.0,"retained-live-owner").await?;
        let cancel=CancellationToken::new(); let worker_owner=owner.clone(); let child_cancel=cancel.clone();
        let worker=tokio::spawn(async move { worker_owner.acquire("lock",1,child_cancel).await });
        cap_notice(&waited).await?; cancel.cancel();
        check(worker.await.map_err(cap_io)?.is_err() && lock.exists(), "Old timestamp reaped live lock")?;
        eprintln!("ARA-CAP21 PASS 12");
        cap_info(&lock,i32::MAX as u32,0.0,"old-dead-owner").await?;
        let stale=owner.capture_stale_lock_release("lock").await.map_err(cap_io)?.ok_or(ValueError::InvalidNode)?;
        tokio::fs::remove_dir_all(&lock).await.map_err(cap_io)?;
        cap_info(&lock,std::process::id(),0.0,"new-live-owner").await?; stale.await;
        check(lock.exists(), "Stale token deleted replacement lock")?;
        eprintln!("ARA-CAP21 PASS 14");
        tokio::fs::remove_dir_all(&lock).await.map_err(cap_io)?; tokio::fs::create_dir(&lock).await.map_err(cap_io)?;
        let cleanup=owner.capture_lock_identity_release("lock").await.ok_or(ValueError::InvalidNode)?;
        // Keep old inode/fileID allocated while creating the replacement.
        let retired=lock.with_extension("retired"); tokio::fs::rename(&lock,&retired).await.map_err(cap_io)?;
        tokio::fs::create_dir(&lock).await.map_err(cap_io)?; cleanup.await;
        check(lock.exists(), "Old identity cleanup deleted replacement empty lock")?;
        let cleanup=owner.capture_lock_identity_release("lock").await.ok_or(ValueError::InvalidNode)?;
        tokio::fs::remove_dir_all(&lock).await.map_err(cap_io)?;
        cap_info(&lock,std::process::id(),0.0,"replacement-owner").await?;cleanup.await;
        let info:serde_json::Value=serde_json::from_slice(&tokio::fs::read(lock.join("info.json")).await.map_err(cap_io)?).map_err(cap_io)?;
        check(info.get("token").and_then(serde_json::Value::as_str)==Some("replacement-owner"),"Old write-failure cleanup removed replacement owner info")?;
        eprintln!("ARA-CAP21 PASS 15");
    }
    // CAP21-16/17: another process removes a just-created/identified lock.
    for identified in [false,true] {
        let root=cap_temp()?; let calls=Arc::new(AtomicUsize::new(0)); let count=calls.clone();
        let hook=Arc::new(move |path:std::path::PathBuf| -> futures::future::BoxFuture<'static,std::io::Result<()>> { let count=count.clone(); Box::pin(async move {
            if count.fetch_add(1,Ordering::SeqCst)==0 { tokio::fs::remove_dir_all(path).await?; } Ok(())
        }) });
        let hooks=if identified { Hooks { lock_identified:Some(hook),..Default::default() } } else { Hooks { lock_created:Some(hook),..Default::default() } };
        let owner=Owner::new(root.path().into()).with_test_hooks(hooks);
        let permit=owner.acquire("removed-lock",1,CancellationToken::new()).await.map_err(cap_io)?;
        check(calls.load(Ordering::SeqCst)>=2 && cap_paths(&owner.provider_dir("removed-lock")).await?.len()==1,
            "Removed lock did not retry to a genuine permit")?;
        permit.release_once().await.map_err(cap_io)?;
        eprintln!("ARA-CAP21 PASS {}",if identified { "17" } else { "16" });
    }
    // CAP21-18/19: exact Win32 permission retry stage, real mkdir on success.
    for persistent in [false,true] {
        let root=cap_temp()?; let count=Arc::new(AtomicUsize::new(0)); let calls=count.clone();
        let owner=Owner::new(root.path().into()).with_test_hooks(Hooks {
            lock_platform_win32:Some(true),lock_retry_budget_ms:Some(if persistent {5}else{100}),lock_retry_initial_ms:Some(1),lock_retry_max_ms:Some(1),
            lock_mkdir:Some(Arc::new(move |path| { let calls=calls.clone(); Box::pin(async move {
                let attempt=calls.fetch_add(1,Ordering::SeqCst);
                if persistent || attempt<2 { Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied,"controlled EPERM mkdir")) }
                else { tokio::fs::create_dir(path).await }
            }) })),..Default::default()
        });
        let result=owner.acquire("mkdir-retry",1,CancellationToken::new()).await;
        if persistent { check(result.as_ref().err().is_some_and(|e|e.kind()==std::io::ErrorKind::PermissionDenied),"Persistent EPERM was swallowed/disabled cap")?; eprintln!("ARA-CAP21 PASS 19"); }
        else { result.map_err(cap_io)?.release_once().await.map_err(cap_io)?; check(count.load(Ordering::SeqCst)==3,"Transient mkdir did not preserve two failures then real third mkdir")?; eprintln!("ARA-CAP21 PASS 18"); }
    }
    // CAP21-20/21: cancel AFTER genuine acquisition before source dispatch,
    // and provider labels cannot become filesystem path components.
    {
        let root=cap_temp()?; let cancel=CancellationToken::new(); let abort=cancel.clone();
        let owner=Owner::new(root.path().into()).with_test_hooks(Hooks { after_acquire:Some(Arc::new(move ||abort.cancel())),..Default::default() });
        let _configuration=cap_configure(owner.clone()); let source=CapSource::new(None,false); let provider=wrap_model_provider(source.clone());
        let terminal=cap_terminal(provider.stream(&cap_model(),&cap_context(),ara_ai::CallOptions {cancel,..Default::default()})).await?;
        check(source.calls.load(Ordering::SeqCst)==0 && terminal.partial().stop_reason==ara_ai::StopReason::Aborted
            && cap_paths(&owner.provider_dir("cap-source-bank")).await?.is_empty(),"Acquire-abort race dispatched or leaked real permit")?;
        eprintln!("ARA-CAP21 PASS 20");
        let owner=Owner::new(root.path().join("opaque")); let raw="../../odd/\\name";
        let permit=owner.acquire(raw,1,CancellationToken::new()).await.map_err(cap_io)?;
        let directory=owner.provider_dir(raw); let segment=directory.file_name().and_then(|s|s.to_str()).ok_or(ValueError::InvalidNode)?;
        check(directory.parent()==Some(root.path().join("opaque").as_path()) && segment.len()==43
            && segment.bytes().all(|b|b.is_ascii_alphanumeric() || b==b'-' || b==b'_'),"Raw provider id escaped opaque provider directory")?;
        permit.release_once().await.map_err(cap_io)?;
        eprintln!("ARA-CAP21 PASS 21");
    }
    provider_cap_receiver_drop_contract(false).await?;
    provider_cap_receiver_drop_contract(true).await?;
    lazy_cap_failure_join_bank(h).await?;
    // These IDs require genuine OS processes and physical HTTP dispatch.
    for mode in ["typed", "actual", "builtin", "custom"] { provider_cap_two_process(mode).await?; }
    eprintln!("ARA-CAP21 PASS 02 09");
    eprintln!("ARA-CAP21 COMPLETE 21/21");
    Ok(())
}


static REENTER_CLOCK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Date.now reenters trackLocalWork before assigning its raw return",
    name: "now", length: 0, capture_count: 3, form: CompiledFunctionForm::Builtin,
    body: |heap, c, receiver, _, runtime| {
        if !receiver.same_identity(&c[2]) { return Err(ValueError::InvalidNode); }
        heap.call(&c[0], &heap.undefined()?, &[], runtime)?;
        Ok(c[1].clone())
    },
};
async fn reentrant_local_work_contract(h: &mut Harness) -> Result<()> {
    use ara_ai::{event::LocalWorkTracker, graph_values::GraphExecution};
    let tracker = LocalWorkTracker::default();
    let child = h.driver.admission().prepare_pending(h.driver.root_scope(), h.driver.root_parent())?;
    let outer = h.driver.admission().prepare_pending(h.driver.root_scope(), h.driver.root_parent())?;
    let retained = Arc::new(std::sync::Mutex::new(None::<ara_ai::event::TrackedLocalWork>));
    let weak_result = Arc::downgrade(&retained);
    let weak_factory = Arc::downgrade(&h.factory);
    let weak_bindings = Arc::downgrade(&h.bindings);
    let original_child = h.heap().capture_deferred(&child.promise)?;
    let once = Arc::new(AtomicUsize::new(0));
    let reentrant_tracker = tracker.clone();
    let on_clock_owner: Arc<dyn Fn() -> Result<()> + Send + Sync> = Arc::new(move || {
        if once.fetch_add(1, Ordering::SeqCst) != 0 { return Ok(()); }
        let factory = weak_factory.upgrade().ok_or(ValueError::InvalidNode)?;
        let bindings = weak_bindings.upgrade().ok_or(ValueError::InvalidNode)?;
        let promise = factory.heap().restore_deferred(&original_child)?;
        let tracked = reentrant_tracker.track_in_original(|ticket| bindings.adopt_local_work(factory, promise, ticket));
        let target = weak_result.upgrade().ok_or(ValueError::InvalidNode)?;
        *target.lock().map_err(|_| ValueError::PoisonedHeap)? = Some(tracked);
        Ok(())
    });
    let on_clock = callback(h, on_clock_owner.clone())?;
    let date = h.get(&h.bindings.globals(), "Date")?;
    let original = h.get(&date, "now")?;
    h.put(&date, "now", h.heap().fresh_compiled_function(&REENTER_CLOCK, &[on_clock, h.heap().number(7777.0)?, date.clone()])?)?;
    let tracked = tracker.track_in_original(|ticket| h.bindings.adopt_local_work(h.factory.clone(), outer.promise, ticket));
    h.with(|r| h.heap().call(&outer.resolve, &h.heap().undefined()?, &[h.heap().number(1.0)?], r))?;
    h.run_until(tracked.wait()).await?;
    let timestamp = h.with(|r| tracker.local_work_settled_at_in(GraphExecution::new(h.heap(), r), &h.bindings.globals()))?;
    check(tracker.has_pending_local_work()? && h.n(&timestamp)? == 7777.0,
        "Reentrant clock deadlocked or rechecked zero instead of assigning raw return")?;
    let child_wait = retained.lock().map_err(|_| ValueError::PoisonedHeap)?.take().ok_or(ValueError::InvalidNode)?;
    h.with(|r| h.heap().call(&child.resolve, &h.heap().undefined()?, &[h.heap().number(2.0)?], r))?;
    h.run_until(child_wait.wait()).await?;
    check(!tracker.has_pending_local_work()?, "Reentrant child work lost its own authoritative finally")?;
    h.put(&date, "now", original)?;
    drop(on_clock_owner);
    eprintln!("ARA-RETRY-UPGRADE PASS reentrant-original-localwork-clock");
    Ok(())
}


// Integrate into the EXISTING owner_b.rs. No product CLI command is added.
// Root aggregate early hook, before Harness::new():
// if tokio::time::timeout(Duration::from_secs(15), owner_b::provider_cap_child()).await.expect("bounded child timeout").expect("bounded cap child role") { return; }
// Parent calls provider_cap_two_process("typed"|"actual"|"builtin"|"custom").

pub async fn provider_cap_child() -> Result<bool> {
    use ara_ai::{ModelProvider, CallOptions};
    use ara_cli::provider_inflight::*;
    let Ok(mode) = std::env::var("ARA_RETRY_CAP_CHILD_ROLE") else { return Ok(false); };
    if !matches!(mode.as_str(), "typed" | "actual" | "builtin" | "custom") {
        return Err(ValueError::UnsupportedOperation("Unknown bounded cap test role"));
    }
    let root = std::path::PathBuf::from(std::env::var_os("ARA_RETRY_CAP_ROOT").ok_or(ValueError::InvalidNode)?);
    let base_url = std::env::var("ARA_RETRY_CAP_URL").map_err(|_| ValueError::InvalidNode)?;
    let waiting = std::env::var_os("ARA_RETRY_CAP_WAITING").map(std::path::PathBuf::from);
    let owner = ProviderInFlightOwner::new(root).with_test_hooks(ProviderInFlightTestHooks {
        wait_observer: waiting.map(|path| Arc::new(move |_| { std::fs::write(&path, std::process::id().to_string()).expect("real child waiter receipt"); }) as Arc<dyn Fn(std::path::PathBuf) + Send + Sync>),
        ..Default::default()
    });
    let mut limits = ara_ai::provider_dispatch::ProviderLimits::new(); limits.insert("cap-child".into(), 1.0);
    configure_provider_inflight(ProviderInFlightConfiguration { owner, limits });
    let model = ara_ai::Model { id:"cap-model".into(), api:"openai-completions".into(), provider:"cap-child".into(), base_url,
        reasoning:false, max_tokens:None, context_window:None, tokenizer:None };
    let typed = ara_ai::Context { system_prompt:vec![], messages:vec![ara_ai::Message::User(ara_ai::UserMessage::text("real child request"))], tools:None };
    let protocol = ara_cli::model_route::ProtocolOptions::Completions(ara_ai::providers::openai_completions::StreamOptions { retry: ara_ai::providers::openai_completions::RetryPolicy { max_attempts:1, ..Default::default() }, ..Default::default() });
    let route = ara_cli::model_route::PreparedRoute::new(model.clone(), protocol,
        Arc::new(ara_cli::model_route::FixedRequestAuth::new(ara_cli::model_route::RequestAuthLease::new(ara_cli::model_route::CredentialIdentity::Runtime, Some("local-fixture".into())))),0)
        .map_err(|_| ValueError::InvalidNode)?;
    let bound = route.bind(reqwest::Client::new(), None);
    if mode == "typed" {
        let mut stream = bound.stream(&model, &typed, CallOptions::default());
        let mut terminal = None;
        while let Some(event) = stream.recv().await { if event.is_terminal() { terminal = Some(event); } }
        check(terminal.is_some_and(|event| event.partial().stop_reason == ara_ai::StopReason::Stop), "Child typed request not successful")?;
    } else {
        let realm = ara_cli::native_realm::NativeRealm::prepare()?;
        let factory = realm.values.factory().clone();
        let json = serde_json::json!({"systemPrompt":[],"messages":[{"role":"user","content":"real child request","timestamp":0}]}).to_string();
        let context = factory.heap().import_strict(ara_rpc::WireValue::parse_units(&json.encode_utf16().collect::<Vec<_>>()).map_err(ValueError::StrictParse)?)?;
        let graph = ara_ai::GraphProviderContext { original_context:Some(context.clone()), system_prompt:vec![], messages:vec![], tools:None, history_demotion_dialect:None, history_id_hash:None, history_id_hash_function:None };
        if mode == "actual" {
            let mut stream = bound.stream_actual(&model, &graph, factory.clone(), CallOptions::default())?;
            let receipt = stream.failure_receipt(); let mut terminal = None;
            while let Some(event) = stream.recv().await { if event.is_terminal() { terminal=Some(event); } }
            let terminal=terminal.ok_or(ValueError::UnsupportedOperation("Child actual request has no terminal"))?;
            let stop=factory.heap().get(terminal.partial(),&"stopReason".into(),factory.runtime())?;
            check(matches!(factory.heap().snapshot(&stop)?,ara_rpc::value_heap::ValueSnapshot::String(ref value) if value.to_utf8().ok().as_deref()==Some("stop")),"Child actual produced error terminal")?;
            check(receipt.wait_settled().await.is_empty(), "Child actual producer receipt failed")?;
        } else {
            // Real custom/builtin registration dispatch shares the SAME OS
            // owner. It directly calls actual Chat; no typed response import.
            let source = ara_cli::actual_tools::BuiltinToolSource::new(&realm.values, ara_tools::ToolContext::new(std::env::current_dir().map_err(|_| ValueError::InvalidNode)?), &[]).map_err(|_| ValueError::InvalidNode)?;
            let prepared = ara_cli::native_factory::PreparedNativeSession::from_complete_bindings(realm.clone(), ara_cli::native_factory::NativeFactoryImports { source, string_function:realm.globals.string.clone(), bun:realm.globals.bun.clone(), original_logger:None }).map_err(|_| ValueError::InvalidNode)?;
            let run = ara_cli::native_run::NativeRun::begin(prepared,tokio_util::sync::CancellationToken::new()).map_err(|_| ValueError::InvalidNode)?;
            let bindings = ara_cli::retry_host::NativeRetryBindings::prepare(&factory,&realm,run.calling_run(&factory)?,ara_ai::ActualStreamFailureReceipt::default())?;
            let registration = Arc::new(ara_cli::retry_host::lazy_provider::RegisteredLazyProvider { provider:Arc::new(CapChildHttpProvider { model:model.clone(), context:graph, bindings:bindings.clone() }), limits:ara_cli::retry_host::lazy_provider::LazyStreamLimits { provider_handles_stream_timeouts:true,provider_handles_first_event_timeouts:true,..Default::default() }, classify:factory.heap().fresh_compiled_function(&CLASSIFY,&[])? });
            let registry = ara_cli::model_registry_extensions::ExtensionRegistry::new();
            let api: ara_rpc::WireString = if mode == "builtin" { "bedrock-converse-stream".into() } else { "cap-custom".into() };
            if mode == "builtin" { ara_cli::retry_host::lazy_provider::set_bedrock_provider_module(&registry,registration); }
            else { ara_cli::retry_host::lazy_provider::register_lazy_provider(&registry,&RegistryHost::default(),api.clone(),Some("cap-child".into()),registration).map_err(|_|ValueError::InvalidNode)?; }
            let model_json=serde_json::json!({"api":"openai-completions","provider":"cap-child","id":"cap-model","name":"cap-model","baseUrl":model.base_url,"reasoning":false,"contextWindow":8192,"maxTokens":2048,"input":["text"],"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0}}).to_string();
            let model_root=factory.heap().import_strict(ara_rpc::WireValue::parse_units(&model_json.encode_utf16().collect::<Vec<_>>()).map_err(ValueError::StrictParse)?)?;
            let invocation=ara_cli::retry_host::lazy_provider::dispatch_registered_lazy_in_run(&run,&registry,&api,factory.clone(),bindings,model_root,context,factory.heap().object()?).map_err(|_|ValueError::InvalidNode)?;
            let events=ara_cli::retry_host::lazy_provider::consume_registered_lazy(run,invocation).await.map_err(|_|ValueError::InvalidNode)?;
            let terminal=events.last().filter(|event|event.is_terminal()).ok_or(ValueError::UnsupportedOperation("Child lazy request missing terminal"))?;
            let stop=factory.heap().get(terminal.partial(),&"stopReason".into(),factory.runtime())?;
            check(matches!(factory.heap().snapshot(&stop)?,ara_rpc::value_heap::ValueSnapshot::String(ref value) if value.to_utf8().ok().as_deref()==Some("stop")),"Child lazy produced error terminal")?;
        }
    }
    eprintln!("ARA-RETRY-CAP CHILD PASS {mode} pid={}",std::process::id());
    Ok(true)
}

struct CapChildHttpProvider {
    model: ara_ai::Model,
    context: ara_ai::GraphProviderContext,
    bindings: Arc<ara_cli::retry_host::NativeRetryBindings>,
}

struct CapServerTask(tokio::task::JoinHandle<()>);
impl Drop for CapServerTask { fn drop(&mut self) { self.0.abort(); } }

pub async fn provider_cap_two_process(mode: &str) -> Result<()> {
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};
    use std::process::Stdio;
    let fixture = cap_temp()?;
    let root = fixture.path().join("provider-inflight");
    let waiting = fixture.path().join("B.waiting");
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(cap_io)?;
    let url = format!("http://{}/v1", listener.local_addr().map_err(cap_io)?);
    let requests = Arc::new(AtomicUsize::new(0));
    let first_seen = Arc::new(tokio::sync::Notify::new());
    let finish_gate = Arc::new(tokio::sync::Notify::new());
    let finish_seen = Arc::new(tokio::sync::Notify::new());
    let tail = Arc::new(tokio::sync::Notify::new());
    let eof = Arc::new(tokio::sync::Notify::new());
    let times = Arc::new(std::sync::Mutex::new(Vec::<std::time::Instant>::new()));
    let (seen2, first2, gate2, finished2, tail2, eof2, times2) =
        (requests.clone(), first_seen.clone(), finish_gate.clone(), finish_seen.clone(), tail.clone(), eof.clone(), times.clone());
    let _server = CapServerTask(tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            let (seen, first, gate, finished, tail, eof, times) =
                (seen2.clone(), first2.clone(), gate2.clone(), finished2.clone(), tail2.clone(), eof2.clone(), times2.clone());
            connections.spawn(async move {
                let mut data=Vec::new(); let mut buffer=[0u8;4096];
                loop {
                    let n=socket.read(&mut buffer).await.unwrap_or(0); if n==0 { return; }
                    data.extend_from_slice(&buffer[..n]);
                    if let Some(end)=data.windows(4).position(|w|w==b"\r\n\r\n") {
                        let headers=String::from_utf8_lossy(&data[..end]);
                        let length=headers.lines().find_map(|line|line.split_once(':').filter(|(key,_)|key.eq_ignore_ascii_case("content-length")).and_then(|(_,v)|v.trim().parse::<usize>().ok())).unwrap_or(0);
                        if data.len()>=end+4+length { break; }
                    }
                }
                let index=seen.fetch_add(1,Ordering::SeqCst);
                times.lock().unwrap().push(std::time::Instant::now());
                if index==0 {
                    // Let B finish initialization and enter the real shared
                    // lease wait BEFORE starting A's fixed terminal grace.
                    first.notify_one(); gate.notified().await;
                }
                if socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").await.is_err() { return; }
                // A separate empty-choices usage frame forces actual consumer
                // early return. Merely finish+usage in one choice can remain in
                // the old trailing-usage loop and would miss the new contract.
                let content=b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"accepted\"}}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n";
                if socket.write_all(content).await.is_err() { return; }
                if index==0 { finished.notify_one(); tail.notified().await; }
                let _=socket.write_all(b"data: [DONE]\n\n").await;
                let _=socket.shutdown().await;
                if index==0 { eof.notify_one(); }
            });
        }
        // Dropping the JoinSet aborts every accepted connection owner as well.
    }));
    let spawn = |label: &str| -> Result<tokio::process::Child> {
        let mut command=tokio::process::Command::new(std::env::current_exe().map_err(cap_io)?);
        command.args(["--exact", "original_41_and_six_source_contracts", "--nocapture"])
            .env("ARA_RETRY_CAP_CHILD_ROLE", mode).env("ARA_RETRY_CAP_ROOT", &root).env("ARA_RETRY_CAP_URL", &url)
            .stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        if label=="B" { command.env("ARA_RETRY_CAP_WAITING", &waiting); }
        command.spawn().map_err(cap_io)
    };
    // Both same-executable children retain kill-on-drop ownership until joined.
    // A timeout drops/join-cancels the SAME handles, never an unbounded blocking
    // wait detached after taking them out of a cleanup guard.
    tokio::time::timeout(Duration::from_secs(15), async {
        let a=spawn("A")?; let a_pid=a.id().ok_or(ValueError::InvalidNode)?;
        cap_notice(&first_seen).await?;
        let b=spawn("B")?; let b_pid=b.id().ok_or(ValueError::InvalidNode)?;
        tokio::time::timeout(Duration::from_secs(5), async {
            while !waiting.exists() { tokio::time::sleep(Duration::from_millis(5)).await; }
        }).await.map_err(cap_io)?;
        check(requests.load(Ordering::SeqCst)==1, "ChildB bypassed live external process lease")?;
        finish_gate.notify_one(); cap_notice(&finish_seen).await?;
        tokio::time::sleep(Duration::from_millis(300)).await;
        check(requests.load(Ordering::SeqCst)==1, "ChildB dispatched on accepted terminal before original tail drain")?;
        let released_at=std::time::Instant::now(); tail.notify_one(); cap_notice(&eof).await?;
        let (a,b)=tokio::try_join!(a.wait_with_output(), b.wait_with_output()).map_err(cap_io)?;
        if !a.status.success() || !b.status.success() {
            eprintln!("A:{:?}\n{}\nB:{:?}\n{}",a.status,String::from_utf8_lossy(&a.stderr),b.status,String::from_utf8_lossy(&b.stderr));
            return Err(ValueError::UnsupportedOperation("Crossprocess child request failed"));
        }
        let times=times.lock().map_err(|_|ValueError::PoisonedHeap)?;
        check(requests.load(Ordering::SeqCst)==2 && times.len()==2 && times[1]>=released_at,
            "Two-process HTTP admission preceded real original DONE release")?;
        eprintln!("ARA-RETRY-CAP CROSSPROCESS PASS {mode} A={a_pid} B={b_pid} requests=2 second_after_done_us={}",times[1].duration_since(released_at).as_micros());
        Ok(())
    }).await.map_err(cap_io)?
}
impl ara_cli::retry_host::lazy_provider::NativeLazyProvider for CapChildHttpProvider {
    fn stream(&self,factory:&Arc<ara_ai::graph_values::GraphValueFactory>,_:&ValueHandle,_:&ValueHandle,options:&ValueHandle,runtime:&dyn ara_rpc::value_heap::ValueRuntime)->Result<ara_cli::retry_host::lazy_provider::NativeLazySource> {
        use ara_ai::{ModelProvider,retry::RetryBindings};
        let signal=factory.heap().get(options,&"signal".into(),runtime)?;
        let cancel=self.bindings.signals().token(factory,&signal,runtime)?;
        let wait=factory.heap().truthy(&factory.heap().get(options,&"waitForTerminalDrain".into(),runtime)?)?;
        let provider=ara_ai::OpenAICompletionsProvider {client:reqwest::Client::new(),base:ara_ai::providers::openai_completions::StreamOptions {api_key:Some("local-fixture".into()),..Default::default()}};
        let mut actual=provider.stream_actual(&self.model,&self.context,factory.clone(),ara_ai::CallOptions {cancel,wait_for_terminal_drain:wait,..Default::default()})?;
        let (mut producer,stream,receipt)=ara_ai::retry::result::ActualCompletionStream::channel(factory);
        receipt.inherit_stream(&actual);
        let original_result=stream.original_result.clone();
        // Channel forwarding retains actual handles; no Graph operation is
        // evaluated in this transport-only bridge task.
        tokio::spawn(async move {while let Some(event)=actual.recv().await {if !producer.push(event).await.unwrap_or(false){break;}}});
        Ok(ara_cli::retry_host::lazy_provider::NativeLazySource {identity:factory.heap().object()?,stream,local_work:None,original_result:Some(original_result)})
    }
}



fn source_accessor(h: &Harness, object: &ValueHandle, key: &str, getter: ValueHandle) -> Result<()> {
    let ara_rpc::value_heap::ValueSnapshot::Function(getter) = h.heap().snapshot(&getter)? else { return Err(ValueError::InvalidNode) };
    h.with(|runtime| h.heap().define_own(object, key.into(), ara_rpc::value_heap::PropertyDescriptor::Accessor {
        get: Some(getter), set: None, enumerable: true, configurable: true,
    }, runtime))
}
static DEADLINE_IDLE_GET: CompiledFunctionSpec = CompiledFunctionSpec {
    source:"b91 currentDeadlineMs repeats idleTimeoutMs Gets around Math.max",name:"get idleTimeoutMs",length:0,capture_count:1,form:CompiledFunctionForm::Builtin,
    body:|heap,c,_,_,r| {
        let n=heap.to_number(&heap.get(&c[0],&"reads".into(),r)?,r)?;
        let stage=heap.to_number(&heap.get(&c[0],&"stage".into(),r)?,r)?;
        let expected=if n==0.0 {0.0} else if n==1.0 {1.0} else {5.0};
        if stage!=expected || n>2.0 {return Err(ValueError::UnsupportedOperation("idleTimeoutMs Get was cached, reordered or duplicated"));}
        heap.define_own_data(&c[0],"reads".into(),heap.number(n+1.0)?,r)?;
        heap.define_own_data(&c[0],"stage".into(),heap.number(if n==2.0 {6.0}else{stage+1.0})?,r)?;
        heap.number(1000.0+n)
    },
};
static DEADLINE_MATH_GET: CompiledFunctionSpec = CompiledFunctionSpec {
    source:"b91 original Math.max Get before localWork argument",name:"get max",length:0,capture_count:2,form:CompiledFunctionForm::Builtin,
    body:|heap,c,_,_,r| {
        let stage=heap.to_number(&heap.get(&c[0],&"stage".into(),r)?,r)?;
        if stage!=2.0 {return Err(ValueError::UnsupportedOperation("Math.max Get not after two idle guards"));}
        heap.define_own_data(&c[0],"stage".into(),heap.number(3.0)?,r)?;Ok(c[1].clone())
    },
};
static DEADLINE_SETTLED_GET: CompiledFunctionSpec = CompiledFunctionSpec {
    source:"b91 localWorkSettledAt is evaluated after Math.max method Get",name:"get localWorkSettledAt",length:0,capture_count:1,form:CompiledFunctionForm::Builtin,
    body:|heap,c,_,_,r| {
        let stage=heap.to_number(&heap.get(&c[0],&"stage".into(),r)?,r)?;
        if stage!=3.0 {return Err(ValueError::UnsupportedOperation("local work timestamp Get moved before original Math.max Get"));}
        heap.define_own_data(&c[0],"stage".into(),heap.number(4.0)?,r)?;heap.number(0.0)
    },
};
static DEADLINE_MATH_CALL: CompiledFunctionSpec = CompiledFunctionSpec {
    source:"b91 original Math.max Call precedes final idleTimeoutMs Get",name:"max",length:2,capture_count:3,form:CompiledFunctionForm::Builtin,
    body:|heap,c,receiver,args,r| {
        let stage=heap.to_number(&heap.get(&c[0],&"stage".into(),r)?,r)?;
        if stage!=4.0 || !receiver.same_identity(&c[2]) {return Err(ValueError::UnsupportedOperation("Math.max Call/Get/receiver order changed"));}
        heap.define_own_data(&c[0],"stage".into(),heap.number(5.0)?,r)?;
        heap.call(&c[1],receiver,args,r)
    },
};
async fn ordered_idle_deadline_contract(h: &mut Harness) -> Result<()> {
    let (source,_)=queue_source(h,vec![h.heap().string("initial".into())?],false)?;
    let local=h.object(&[("localWorkSettledAt",h.heap().number(0.0)?)])?;
    let options=h.object(&[("idleTimeoutMs",h.heap().number(1000.0)?),("firstItemTimeoutMs",h.heap().number(1000.0)?),("localWork",local.clone()),("errorMessage",h.heap().string("stall".into())?)])?;
    let mut idle=IdleIterator::prepare(h.factory.clone(),h.bindings.clone(),source,options.clone())?;
    check(h.run_until(idle.next()).await?.is_some(),"Ordered idle fixture lost initial progress")?;
    let state=h.object(&[("reads",h.heap().number(0.0)?),("stage",h.heap().number(0.0)?)])?;
    let math=h.get(&h.bindings.globals(),"Math")?;let original=h.get(&math,"max")?;
    let called=h.heap().fresh_compiled_function(&DEADLINE_MATH_CALL,&[state.clone(),original.clone(),math.clone()])?;
    source_accessor(h,&options,"idleTimeoutMs",h.heap().fresh_compiled_function(&DEADLINE_IDLE_GET,&[state.clone()])?)?;
    source_accessor(h,&local,"localWorkSettledAt",h.heap().fresh_compiled_function(&DEADLINE_SETTLED_GET,&[state.clone()])?)?;
    source_accessor(h,&math,"max",h.heap().fresh_compiled_function(&DEADLINE_MATH_GET,&[state.clone(),called])?)?;
    let result=h.run_until(idle.next()).await;
    h.put(&math,"max",original)?;
    check(result?.is_none() && h.n(&h.get(&state,"stage")?)?==6.0 && h.n(&h.get(&state,"reads")?)?==3.0,
        "Native idle deadline did not preserve repeated mutable Gets/Math call order")?;
    eprintln!("ARA-RETRY-UPGRADE PASS ordered-idle-deadline-get-call");Ok(())
}


async fn provider_cap_receiver_drop_contract(identified: bool) -> Result<()> {
    use ara_cli::provider_inflight::{ProviderInFlightOwner as Owner, ProviderInFlightTestHooks as Hooks, wrap_model_provider};
    let root=cap_temp()?;let entered=Arc::new(tokio::sync::Notify::new());let resume=Arc::new(tokio::sync::Notify::new());
    let seen=entered.clone();let gate=resume.clone();
    let hook=Arc::new(move |_:std::path::PathBuf|->futures::future::BoxFuture<'static,std::io::Result<()>> {
        let seen=seen.clone();let gate=gate.clone();Box::pin(async move {seen.notify_one();gate.notified().await;Ok(())})
    });
    let hooks=if identified {Hooks{lock_identified:Some(hook),..Default::default()}} else {Hooks{lock_created:Some(hook),..Default::default()}};
    let owner=Owner::new(root.path().into()).with_test_hooks(hooks);
    let _configuration=cap_configure(owner.clone());let source=CapSource::new(None,false);let provider=wrap_model_provider(source.clone());
    let stream=provider.stream(&cap_model(),&cap_context(),Default::default());
    cap_notice(&entered).await?;drop(stream);resume.notify_one();
    let directory=owner.provider_dir("cap-source-bank");let lock=directory.with_extension("lock");
    cap_until(|| !lock.exists() && std::fs::read_dir(&directory).map(|entries|entries.filter_map(std::result::Result::ok).all(|entry|!entry.path().is_dir())).unwrap_or(!directory.exists())).await?;
    check(source.calls.load(Ordering::SeqCst)==0,"Receiver-close dropped acquisition future or dispatched abandoned provider")?;
    eprintln!("ARA-RETRY-CAP PASS receiver-close-during-real-lock identified={identified}");Ok(())
}

static TERMINAL_NEVER_EXECUTOR: CompiledFunctionSpec = CompiledFunctionSpec {
    source:"Original terminal source remains pending until fixed grace timer",name:"",length:2,capture_count:0,form:CompiledFunctionForm::Arrow,
    body:|heap,_,_,_,_|heap.undefined(),
};
async fn native_run_terminal_drain_contract() -> Result<()> {
    use ara_ai::retry::{RetryBindings,idle::TerminalGraceIterator};
    use ara_cli::{actual_tools::BuiltinToolSource,native_factory::{NativeFactoryImports,PreparedNativeSession},native_realm::NativeRealm,native_run::NativeRun,retry_host::NativeRetryBindings};
    let realm=NativeRealm::prepare()?;
    let source=BuiltinToolSource::new(&realm.values,ara_tools::ToolContext::new(std::env::current_dir().map_err(cap_io)?),&[]).map_err(cap_io)?;
    let prepared=PreparedNativeSession::from_complete_bindings(realm.clone(),NativeFactoryImports {source,string_function:realm.globals.string.clone(),bun:realm.globals.bun.clone(),original_logger:None}).map_err(cap_io)?;
    let run=NativeRun::begin(prepared,tokio_util::sync::CancellationToken::new()).map_err(cap_io)?;
    let lease=run.lease();let factory=realm.values.factory().clone();let calling=run.calling_run(&factory)?;
    let bindings=NativeRetryBindings::prepare(&factory,&realm,calling.clone(),ara_ai::ActualStreamFailureReceipt::default())?;
    let returns=Arc::new(AtomicUsize::new(0));let returned=returns.clone();let ended=Arc::new(AtomicUsize::new(0));let expired=ended.clone();
    let release_owner:Arc<dyn Fn()->Result<()>+Send+Sync>=Arc::new({let returned=returned.clone();move ||{returned.fetch_add(1,Ordering::SeqCst);Ok(())}});
    let grace_end_owner:Arc<dyn Fn()->Result<()>+Send+Sync>=Arc::new({let expired=expired.clone();move ||{expired.fetch_add(1,Ordering::SeqCst);Ok(())}});
    let release=calling.with_runtime(&factory,&mut |r|bindings.signals().callback(&factory,release_owner.clone(),r))?;
    let on_end=calling.with_runtime(&factory,&mut |r|bindings.signals().callback(&factory,grace_end_owner.clone(),r))?;
    let mut options=None;
    let iterable=calling.with_runtime(&factory,&mut |r| {
        let heap=factory.heap();let globals=bindings.globals();let promise=heap.get(&globals,&"Promise".into(),r)?;
        let pending=heap.construct(&promise,&[heap.fresh_compiled_function(&TERMINAL_NEVER_EXECUTOR,&[])?],r)?;
        let state=heap.object()?;
        for (key,value) in [("calls",heap.number(0.0)?),("pending",heap.boolean(true)?),("Promise",promise),("items",heap.array(vec![heap.string("accepted".into())?])?)] {heap.define_own_data(&state,key.into(),value,r)?;}
        let source=heap.object()?;
        heap.define_own_data(&source,bindings.async_iterator_key(),heap.fresh_compiled_function(&SELF,&[])?,r)?;
        heap.define_own_data(&source,"next".into(),heap.fresh_compiled_function(&ARRAY_NEXT,&[state,pending])?,r)?;
        heap.define_own_data(&source,"return".into(),release.clone(),r)?;
        let date=heap.get(&globals,&"Date".into(),r)?;let now=heap.get(&date,&"now".into(),r)?;let stamp=heap.call(&now,&date,&[],r)?;
        let prepared=heap.object()?;
        for (key,value) in [("finishedAtMs",heap.fresh_compiled_function(&RETURN_CAPTURE,&[stamp])?),("graceMs",heap.number(50.0)?),("awaitDrainOnReturn",heap.boolean(false)?),("onGraceEnd",on_end.clone())] {heap.define_own_data(&prepared,key.into(),value,r)?;}
        options=Some(prepared);Ok(source)
    })?;
    let original:Arc<dyn RetryBindings>=bindings;
    let mut iterator=TerminalGraceIterator::prepare(factory,original,iterable,options.ok_or(ValueError::InvalidNode)?)?;
    let logical=Arc::new(AtomicUsize::new(0));let completed=logical.clone();
    tokio::time::timeout(Duration::from_secs(3),run.drive(async move {
        check(iterator.next().await?.is_some(),"Native exit fixture lost accepted yield")?;
        lease.prepare_terminal_return(iterator)?.await?;
        completed.store(1,Ordering::SeqCst);
        Ok::<_,ValueError>(())
    })).await.map_err(cap_io)?.map_err(cap_io)??;
    check(logical.load(Ordering::SeqCst)==1 && returns.load(Ordering::SeqCst)==1 && ended.load(Ordering::SeqCst)==1,
        "Agent completion stopped original driver before admitted terminal drain/clock settled")?;
    drop((release_owner,grace_end_owner));
    eprintln!("ARA-RETRY-UPGRADE PASS native-run-agent-exit-original-drain");Ok(())
}


async fn authoritative_local_work_revoke_contract() -> Result<()> {
    use ara_ai::event::LocalWorkTracker;
    use ara_cli::native_realm::promise::NativeDrain;
    for (complete,clock_throws) in [(true,false),(true,true),(false,false)] {
        let mut h=Harness::new()?;let tracker=LocalWorkTracker::default();let original=h.heap().object()?;
        if clock_throws {
            let date=h.get(&h.bindings.globals(),"Date")?;
            h.put(&date,"now",h.heap().fresh_compiled_function(&THROW,&[original.clone()])?)?;
        }
        let work=h.driver.admission().prepare_pending(h.driver.root_scope(),h.driver.root_parent())?;
        let observation=h.bindings.adopt_local_work(h.factory.clone(),work.promise,tracker.begin_local_work())?;
        if complete {
            h.with(|r|h.heap().call(&work.resolve,&h.heap().undefined()?,&[original.clone()],r))?;
            let mut jobs=0;
            while matches!(h.driver.drain_one()?,NativeDrain::RanJob{..}) {jobs+=1;check(jobs<64,"Native local finally unexpectedly failed to quiesce")?;}
            check(!tracker.has_pending_local_work()?,"Authoritative callback did not complete before explicit revoke")?;
        }
        h.driver.revoker().revoke();
        let observed=tokio::time::timeout(Duration::from_secs(2),observation).await.map_err(cap_io)?;
        check(observed.settled==complete && tracker.has_pending_local_work()? != complete,
            "Revoke erased authoritative completion or manufactured completion before callback")?;
        if complete {
            if clock_throws {check(matches!(observed.result,Err(ValueError::Thrown(ref v))if v.same_identity(&original)),"Postcallback revoke lost authoritative clock throw")?;}
            else {check(observed.result?.same_identity(&original),"Postcallback revoke lost original settled value")?;}
        }
    }
    eprintln!("ARA-RETRY-UPGRADE PASS authoritative-finally-before-revoke");Ok(())
}


static FINALLY_INSTALLS_THENABLE: CompiledFunctionSpec = CompiledFunctionSpec {
    source:"Original Date.now reentry adds then to fulfilled object before async tracked return resolves",
    name:"now",length:0,capture_count:3,form:CompiledFunctionForm::Builtin,
    body:|heap,c,_,_,runtime| {
        heap.define_own_data(&c[0],"then".into(),c[1].clone(),runtime)?;
        Ok(c[2].clone())
    },
};
static DERIVED_THENABLE: CompiledFunctionSpec = CompiledFunctionSpec {
    source:"Derived async return assimilates newly installed original thenable before caller result",
    name:"then",length:2,capture_count:1,form:CompiledFunctionForm::Builtin,
    body:|heap,c,_,args,runtime| {
        heap.define_own_data(&c[0],"called".into(),heap.boolean(true)?,runtime)?;
        heap.define_own_data(&c[0],"resolve".into(),args.first().ok_or(ValueError::InvalidNode)?.clone(),runtime)?;
        heap.undefined()
    },
};
async fn derived_local_work_adoption_contract(h:&mut Harness)->Result<()> {
    let tracker=ara_ai::event::LocalWorkTracker::default();
    let work=h.driver.admission().prepare_pending(h.driver.root_scope(),h.driver.root_parent())?;
    let fulfilled=h.heap().object()?;let state=h.object(&[("called",h.heap().boolean(false)?)])?;
    let then=h.heap().fresh_compiled_function(&DERIVED_THENABLE,&[state.clone()])?;
    let date=h.get(&h.bindings.globals(),"Date")?;let original_now=h.get(&date,"now")?;
    h.put(&date,"now",h.heap().fresh_compiled_function(&FINALLY_INSTALLS_THENABLE,&[fulfilled.clone(),then,h.heap().number(9000.0)?])?)?;
    let mut observation=h.bindings.adopt_local_work(h.factory.clone(),work.promise,tracker.begin_local_work())?;
    h.with(|r|h.heap().call(&work.resolve,&h.heap().undefined()?,&[fulfilled],r))?;
    h.run_until(async {tokio::task::yield_now().await;Ok(())}).await?;
    check(h.heap().truthy(&h.get(&state,"called")?)? && !tracker.has_pending_local_work()?,
        "Finally did not run before derived thenable assimilation")?;
    check(matches!(futures::poll!(observation.as_mut()),std::task::Poll::Pending),
        "Tracked caller received original work result before derived thenable settled")?;
    let resolver=h.get(&state,"resolve")?;let derived=h.heap().object()?;
    h.with(|r|h.heap().call(&resolver,&h.heap().undefined()?,&[derived.clone()],r))?;
    let result=h.run_until(async {Ok(observation.await)}).await?;
    h.put(&date,"now",original_now)?;
    check(result.settled && result.result?.same_identity(&derived),
        "Derived tracked return lost its original thenable adoption outcome")?;
    eprintln!("ARA-RETRY-UPGRADE PASS derived-async-finally-thenable-adoption");Ok(())
}


fn cap_old_directory(path:&std::path::Path)->Result<()> {
    let mut options=std::fs::OpenOptions::new();options.read(true);
    #[cfg(windows)] {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_WRITE_ATTRIBUTES,FILE_FLAG_BACKUP_SEMANTICS};
        options.access_mode(FILE_WRITE_ATTRIBUTES).custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    options.open(path).map_err(cap_io)?.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()-Duration::from_secs(60))).map_err(cap_io)
}


// Integrate into existing owner_b source_groups as ONE coarse table bank.
// Uses existing Harness/RegistryHost/CLASSIFY/THROW/cap_io helpers.
#[derive(Clone, Copy)]
enum LazyCapFault { QueuedOuterClose, QueuedOriginalAbort, ActiveOuterClose, ForwardError, PostCallSetupThrow }
struct HeldLazyCapProvider {
    source: std::sync::Mutex<Option<(ara_ai::retry::result::ActualCompletionProducer, ara_ai::retry::result::ActualCompletionStream, ara_ai::EventSink<ara_ai::ActualAssistantEvent>, ara_ai::event::ActualStreamSettlementGuard)>>,
    bindings: Arc<dyn ara_ai::retry::RetryBindings>,
    mode: LazyCapFault,
    calls: Arc<AtomicUsize>,
    started: Arc<tokio::sync::Notify>,
    aborted: Arc<tokio::sync::Notify>,
    release_resource: Arc<tokio::sync::Notify>,
    published: Arc<AtomicUsize>,
    buffered: Arc<tokio::sync::Notify>,
    message: ValueHandle,
    setup_throw: ValueHandle,
}
impl ara_cli::retry_host::lazy_provider::NativeLazyProvider for HeldLazyCapProvider {
    fn stream(&self, factory:&Arc<ara_ai::graph_values::GraphValueFactory>, model:&ValueHandle, _: &ValueHandle, options:&ValueHandle, runtime:&dyn ara_rpc::value_heap::ValueRuntime)->Result<ara_cli::retry_host::lazy_provider::NativeLazySource> {
        self.calls.fetch_add(1,Ordering::SeqCst);
        let signal=factory.heap().get(options,&"signal".into(),runtime)?;
        let request_cancel=self.bindings.signals().token(factory,&signal,runtime)?;
        let (mut producer,source,bounded_sink,physical_guard)=self.source.lock().map_err(|_|ValueError::PoisonedHeap)?.take().ok_or(ValueError::InvalidNode)?;
        if matches!(self.mode,LazyCapFault::PostCallSetupThrow) {
            let getter=factory.heap().fresh_compiled_function(&THROW,&[self.setup_throw.clone()])?;
            let ara_rpc::value_heap::ValueSnapshot::Function(target)=factory.heap().snapshot(&getter)? else {return Err(ValueError::InvalidNode)};
            // This getter is consumed by the real Host AFTER source.stream
            // returned and its native Run receipt was attached.
            factory.heap().define_own(model,"compat".into(),ara_rpc::value_heap::PropertyDescriptor::Accessor {get:Some(target),set:None,enumerable:true,configurable:true},runtime)?;
        }
        let original_result=source.original_result.clone();
        let (started,aborted,release,message,mode)=(self.started.clone(),self.aborted.clone(),self.release_resource.clone(),self.message.clone(),self.mode);
        let published=self.published.clone();let buffered=self.buffered.clone();
        tokio::spawn(async move {
            let _physical_guard=physical_guard;
            started.notify_one();
            if matches!(mode,LazyCapFault::ForwardError) {
                // Genuine stream failure plus a still-held producer/resource
                // receipt. No terminal Boolean stands for that settlement.
                producer.fail_original(ValueError::UnsupportedOperation("controlled original forward failure")).expect("original failure publication");
            } else {
                for _ in 0..300 {if !bounded_sink.push_or_cancel(ara_ai::ActualAssistantEvent::Start {partial:message.clone()},&request_cancel).await{break;}published.fetch_add(1,Ordering::SeqCst);}
                buffered.notify_one();
            }
            drop(bounded_sink);
            request_cancel.cancelled().await;
            aborted.notify_one();
            release.notified().await;
            // The authentic ActualCompletionProducer guard seals only here.
            drop(producer);
        });
        Ok(ara_cli::retry_host::lazy_provider::NativeLazySource {identity:factory.heap().object()?,stream:source,local_work:None,original_result:Some(original_result)})
    }
}

async fn lazy_cap_failure_join_bank(h:&mut Harness)->Result<()> {
    use ara_cli::{provider_inflight::*,model_registry_extensions::ExtensionRegistry,retry_host::lazy_provider::*};
    use tokio_util::sync::CancellationToken;
    let previous=configuration();
    struct Restore(ProviderInFlightConfiguration);
    impl Drop for Restore {fn drop(&mut self){configure_provider_inflight(self.0.clone());}}
    let _restore=Restore(previous);
    for (label,mode) in [("queued-outer-close",LazyCapFault::QueuedOuterClose),("queued-original-abort",LazyCapFault::QueuedOriginalAbort),("active-outer-close",LazyCapFault::ActiveOuterClose),("forward-error-held-resource",LazyCapFault::ForwardError),("postcall-setup-throw",LazyCapFault::PostCallSetupThrow)] {
        let root=cap_temp()?;
        let waiting=Arc::new(tokio::sync::Notify::new()); let wait=waiting.clone();
        let owner=ProviderInFlightOwner::new(root.path().to_owned()).with_test_hooks(ProviderInFlightTestHooks {wait_observer:Some(Arc::new(move |_|wait.notify_one())),..Default::default()});
        let mut limits=ara_ai::provider_dispatch::ProviderLimits::new();limits.insert("lazy-fault".into(),1.0);
        configure_provider_inflight(ProviderInFlightConfiguration {owner:owner.clone(),limits});
        let held=if matches!(mode,LazyCapFault::QueuedOuterClose|LazyCapFault::QueuedOriginalAbort){Some(owner.acquire("lazy-fault",1,CancellationToken::new()).await.map_err(cap_io)?)}else{None};
        let (producer,mut source,_original_receipt)=ara_ai::retry::result::ActualCompletionStream::channel(&h.factory);
        // Use the real bounded256 transport lane, rather than utility
        // EventStream's unbounded source queue, to exercise >256 close flow.
        let (bounded_sink,bounded_source,resource_receipt)=ara_ai::ActualAssistantStream::channel();
        resource_receipt.inherit_stream(&source.events);
        let physical_guard=resource_receipt.owner_settlement_guard();
        source.events=bounded_source;
        let calls=Arc::new(AtomicUsize::new(0));let started=Arc::new(tokio::sync::Notify::new());let aborted=Arc::new(tokio::sync::Notify::new());let release=Arc::new(tokio::sync::Notify::new());
        let message=h.data(serde_json::json!({"role":"assistant","content":[],"api":"bedrock-converse-stream","provider":"lazy-fault","model":"fixture","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"stop","timestamp":0}))?;
        let published=Arc::new(AtomicUsize::new(0));let buffered=Arc::new(tokio::sync::Notify::new());
        let provider=Arc::new(HeldLazyCapProvider {source:std::sync::Mutex::new(Some((producer,source,bounded_sink,physical_guard))),bindings:h.bindings.clone(),mode,calls:calls.clone(),started:started.clone(),aborted:aborted.clone(),release_resource:release.clone(),published:published.clone(),buffered:buffered.clone(),message,setup_throw:h.heap().object()?});
        let registry=ExtensionRegistry::new();set_bedrock_provider_module(&registry,Arc::new(RegisteredLazyProvider {provider:provider.clone(),limits:LazyStreamLimits {provider_handles_stream_timeouts:true,provider_handles_first_event_timeouts:true,..Default::default()},classify:h.heap().fresh_compiled_function(&CLASSIFY,&[])?}));
        let model=h.data(serde_json::json!({"api":"bedrock-converse-stream","provider":"lazy-fault","id":"fixture","compat":{}}))?;
        let original_abort=controller(h)?;
        let abort_reason=h.with(|runtime|h.heap().construct(&h.bindings.error_constructor(),&[h.heap().string("cancel queued waiter".into())?],runtime))?;
        let options=if matches!(mode,LazyCapFault::QueuedOriginalAbort){h.object(&[("signal",original_abort.signal.clone())])?}else{h.heap().object()?};
        let invocation=dispatch_registered_lazy(&registry,&"bedrock-converse-stream".into(),h.factory.clone(),h.bindings.clone(),model,h.heap().object()?,options).map_err(cap_io)?;
        let expected_setup=provider.setup_throw.clone();let factory=h.factory.clone();let bindings=h.bindings.clone();
        let mut outer=invocation.stream;let forward=invocation.forward;
        let observed=resource_receipt.clone();let dir=owner.provider_dir("lazy-fault");
        h.run_until(async move {
            tokio::pin!(forward);
            if matches!(mode,LazyCapFault::QueuedOuterClose|LazyCapFault::QueuedOriginalAbort) {
                tokio::select! {result=&mut forward=>{let _=result;return Err(ValueError::UnsupportedOperation("Queued lazy forward completed before real cap wait"))},_=waiting.notified()=>{}}
                let reason_expected=matches!(mode,LazyCapFault::QueuedOriginalAbort);
                if reason_expected {
                    bindings.with_runtime(&factory,&mut |runtime| {
                        bindings.signals().abort(&factory,&original_abort,Some(abort_reason.clone()),runtime)?;
                        factory.heap().undefined()
                    })?;
                } else {outer.events.close();}
                let outcome=tokio::time::timeout(Duration::from_secs(3),&mut forward).await.map_err(cap_io)?;
                check(calls.load(Ordering::SeqCst)==0,"Closed/aborted queued stream physically dispatched")?;
                if reason_expected {
                    check(matches!(outcome,Err(ValueError::Thrown(ref value))if value.same_identity(&abort_reason)),"Queued cap lost original Native signal reason")?;
                    let result=outer.result().await;
                    check(matches!(result,Err(ValueError::Thrown(ref value))if value.same_identity(&abort_reason)),"Queued outer result replaced original abort reason")?;
                }
                return Ok(());
            }
            tokio::select! {result=&mut forward=>{let _=result;return Err(ValueError::UnsupportedOperation("Forward released before genuine source resource"))},_=started.notified()=>{}}
            if matches!(mode,LazyCapFault::ActiveOuterClose){
                tokio::select! {result=&mut forward=>{let _=result;return Err(ValueError::UnsupportedOperation("Active bounded source ended before 300 queued events"))},_=buffered.notified()=>{}}
                check(published.load(Ordering::SeqCst)==300,"Active close did not exercise genuine >256 event buffering")?;
                outer.events.close();
            }
            tokio::select! {result=&mut forward=>{let _=result;return Err(ValueError::UnsupportedOperation("Failed/closed source released without original abort/resource settlement"))},_=aborted.notified()=>{}}
            check(calls.load(Ordering::SeqCst)==1&&!observed.is_settled(),"Abort notification substituted for authentic producer settlement")?;
            let mut lease_count=0;let mut entries=tokio::fs::read_dir(&dir).await.map_err(cap_io)?;while let Some(entry)=entries.next_entry().await.map_err(cap_io)?{if entry.metadata().await.map_err(cap_io)?.is_dir(){lease_count+=1;}}
            check(lease_count==1,"Cap released while real source resource remained held")?;
            release.notify_one();
            let outcome=tokio::time::timeout(Duration::from_secs(3),&mut forward).await.map_err(cap_io)?;
            check(observed.is_settled(),"Permit completion preceded source resource receipt")?;
            if matches!(mode,LazyCapFault::PostCallSetupThrow){check(matches!(outcome,Err(ValueError::Thrown(ref value))if value.same_identity(&expected_setup)),"Post-call setup replaced original getter throw identity")?;}
            if matches!(mode,LazyCapFault::ForwardError){check(matches!(outcome,Err(ValueError::UnsupportedOperation("controlled original forward failure"))),"Forward failure replaced original source error")?;}
            let mut entries=tokio::fs::read_dir(&dir).await.map_err(cap_io)?;while let Some(entry)=entries.next_entry().await.map_err(cap_io)?{check(!entry.metadata().await.map_err(cap_io)?.is_dir(),"Released cap left live lease")?;}
            Ok(())
        }).await?;
        if let Some(held)=held{held.release_once().await.map_err(cap_io)?;}
        eprintln!("ARA-RETRY-UPGRADE PASS lazy-cap-fault {label}");
    }
    Ok(())
}
