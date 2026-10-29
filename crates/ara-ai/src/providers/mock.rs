//! Local mock provider: original OMP596f2da plus bounded b91db66e terminal metadata delta.
//! Source: packages/ai/src/providers/mock.ts (MIT; THIRD_PARTY_NOTICES.md).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc.
//!
//! The Host supplies the original catalog/Error/Promise imports and owns the
//! custom API registry. Actual Context, response, block and thrown values stay
//! in one graph lineage. Rust source/handler futures are language adapters, not
//! JSON fixtures or another Promise driver.

use crate::{
    ActualAssistantEvent, ActualAssistantStream, ActualStreamFailureReceipt,
    AssistantMessageEvent as Event, AssistantStream, CallOptions, Context, EventSink,
    GraphProviderContext, Model, ModelProvider, StopReason, actual_provider::GraphBuilder,
    graph_values::GraphValueFactory,
};
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, PropertyKey, ValueError, ValueHandle,
        ValueKind, ValueSnapshot,
    },
};
use futures::future::BoxFuture;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Instant,
};

type Result<T> = std::result::Result<T, ValueError>;
pub const MOCK_API: &str = "mock";
static HAS_TOOL_CALL: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da mock.ts:412 blocks.some(b => b.type === toolCall)",
    name: "",
    length: 1,
    capture_count: 0,
    form: CompiledFunctionForm::Arrow,
    body: |heap, _, _, args, runtime| {
        let block = args.first().cloned().unwrap_or(heap.undefined()?);
        let kind = heap.get(&block, &"type".into(), runtime)?;
        heap.boolean(matches!(heap.snapshot(&kind)?, ValueSnapshot::String(text) if text.equals_ascii("toolCall")))
    },
};
fn literal(g: &GraphBuilder, value: &ValueHandle, expected: &str) -> Result<bool> {
    Ok(
        matches!(g.0.heap().snapshot(value)?, ValueSnapshot::String(text) if text.equals_ascii(expected)),
    )
}
fn no_handler_error(g: &GraphBuilder, model: &MockModel) -> Result<ValueError> {
    let heap = g.0.heap();
    let id = heap.to_string(&g.get(&model.metadata, "id")?, g.0.runtime())?;
    let count = heap.to_string(&g.get(&model.recorded_calls, "length")?, g.0.runtime())?;
    let mut units = WireString::from("Mock model \"").units().to_vec();
    units.extend_from_slice(id.units());
    units.extend_from_slice(WireString::from("\" received call ").units());
    units.extend_from_slice(count.units());
    units.extend_from_slice(WireString::from(" but no response or handler is configured.").units());
    model
        .bindings
        .validation_error(&g.0, WireString::from_units(units))
}

/// One evaluated mock module's Host imports. The await operation adopts the
/// original returned value in the caller's existing owner; it never pumps.
pub trait MockBindings: Send + Sync {
    fn check_invocation(&self, factory: &Arc<GraphValueFactory>) -> Result<()>;
    fn classify_model(
        &self,
        factory: &Arc<GraphValueFactory>,
        provider: &ValueHandle,
        id: &ValueHandle,
    ) -> Result<ValueHandle>;
    fn zero_cost(&self) -> ValueHandle;
    fn async_iterator_key(&self) -> Option<PropertyKey>;
    fn error_constructor(&self) -> &ValueHandle;
    fn validation_error(
        &self,
        factory: &Arc<GraphValueFactory>,
        message: WireString,
    ) -> Result<ValueError>;
    fn signal_token(
        &self,
        factory: &Arc<GraphValueFactory>,
        signal: &ValueHandle,
    ) -> Result<tokio_util::sync::CancellationToken>;
    fn await_value(
        &self,
        factory: Arc<GraphValueFactory>,
        value: ValueHandle,
    ) -> BoxFuture<'static, Result<ValueHandle>>;
}

pub type MockHandlerFn = dyn Fn(
        ValueHandle,
        Option<ValueHandle>,
        Arc<GraphValueFactory>,
    ) -> BoxFuture<'static, Result<ValueHandle>>
    + Send
    + Sync;
#[derive(Clone)]
pub enum MockHandler {
    Response(ValueHandle),
    Callable(ValueHandle),
    Rust(Arc<MockHandlerFn>),
}
impl MockHandler {
    fn native(factory: &GraphValueFactory, value: ValueHandle) -> Result<Self> {
        Ok(if factory.heap().kind(&value)? == ValueKind::Function {
            Self::Callable(value)
        } else {
            Self::Response(value)
        })
    }
}
/// Each invocation pulls exactly one source entry. Implementations invoke
/// `next` when this function is called, before awaiting its returned future.
pub trait MockSource: Send + Sync {
    fn next(
        &self,
        factory: Arc<GraphValueFactory>,
    ) -> BoxFuture<'static, Result<Option<MockHandler>>>;
}
/// A Rust synchronous iterator adapter; arbitrary Set/custom iterators work.
pub struct SyncMockSource<I>(Mutex<I>);
impl<I> SyncMockSource<I> {
    pub fn new(iterator: I) -> Self {
        Self(Mutex::new(iterator))
    }
}
impl<I: Iterator<Item = Result<MockHandler>> + Send + 'static> MockSource for SyncMockSource<I> {
    fn next(&self, _: Arc<GraphValueFactory>) -> BoxFuture<'static, Result<Option<MockHandler>>> {
        let result = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .next()
            .transpose();
        Box::pin(async move { result })
    }
}
pub struct AsyncMockSource<F>(pub F);
impl<F> MockSource for AsyncMockSource<F>
where
    F: Fn(Arc<GraphValueFactory>) -> BoxFuture<'static, Result<Option<MockHandler>>> + Send + Sync,
{
    fn next(
        &self,
        factory: Arc<GraphValueFactory>,
    ) -> BoxFuture<'static, Result<Option<MockHandler>>> {
        (self.0)(factory)
    }
}
/// Language adapter for a source implementing both iterator protocols.
pub fn preferred_mock_source(
    asynchronous: Option<Arc<dyn MockSource>>,
    synchronous: Arc<dyn MockSource>,
) -> Arc<dyn MockSource> {
    asynchronous.unwrap_or(synchronous)
}
struct NativeSource {
    iterator: ValueHandle,
    bindings: Arc<dyn MockBindings>,
}
impl NativeSource {
    fn from_source(
        factory: &Arc<GraphValueFactory>,
        bindings: Arc<dyn MockBindings>,
        source: &ValueHandle,
    ) -> Result<Self> {
        let heap = factory.heap();
        // Source iteratorOf checks `Symbol.asyncIterator in source`, not the
        // truthiness of the method; an undefined selected method must throw.
        let key = match bindings.async_iterator_key() {
            Some(key) if heap.has(source, &key, factory.runtime())? => key,
            _ => heap.iterator_key(),
        };
        let method = heap.get(source, &key, factory.runtime())?;
        let iterator = heap.call(&method, source, &[], factory.runtime())?;
        Ok(Self { iterator, bindings })
    }
}
impl MockSource for NativeSource {
    fn next(
        &self,
        factory: Arc<GraphValueFactory>,
    ) -> BoxFuture<'static, Result<Option<MockHandler>>> {
        // Unlike a for-of record, mock.ts Get-Calls `iterator.next` each time.
        let result = (|| {
            let next = factory
                .heap()
                .get(&self.iterator, &"next".into(), factory.runtime())?;
            factory
                .heap()
                .call(&next, &self.iterator, &[], factory.runtime())
        })();
        // Promise.resolve(next()) adopts (and Gets then) before the source
        // async function yields; only the resulting wait belongs in the future.
        let adopted: BoxFuture<'static, Result<ValueHandle>> = match result {
            Ok(value) => self.bindings.await_value(factory.clone(), value),
            Err(error) => Box::pin(async move { Err(error) }),
        };
        Box::pin(async move {
            let result = adopted.await?;
            let done = factory
                .heap()
                .get(&result, &"done".into(), factory.runtime())?;
            if factory.heap().truthy(&done)? {
                return Ok(None);
            }
            let value = factory
                .heap()
                .get(&result, &"value".into(), factory.runtime())?;
            Ok(Some(MockHandler::native(&factory, value)?))
        })
    }
}

#[derive(Clone)]
pub struct MockCall {
    pub context: ValueHandle,
    pub options: Option<ValueHandle>,
}
struct State {
    extras: VecDeque<MockHandler>,
    exhausted: bool,
    source: Option<Arc<dyn MockSource>>,
    fallback: Option<MockHandler>,
    counter: f64,
}
/// Constructed instances are the brand. A copied graph object is not a model.
#[derive(Clone)]
pub struct MockModel {
    metadata: ValueHandle,
    recorded_calls: ValueHandle,
    bindings: Arc<dyn MockBindings>,
    state: Arc<Mutex<State>>,
}
#[derive(Clone)]
pub enum MockModelHandle {
    Mock(Arc<MockModel>),
    Foreign(ValueHandle, Arc<dyn MockBindings>),
}
pub fn is_mock_model(model: &MockModelHandle) -> bool {
    matches!(model, MockModelHandle::Mock(_))
}
impl MockModel {
    pub fn model(self: &Arc<Self>) -> MockModelHandle {
        MockModelHandle::Mock(self.clone())
    }
    pub fn model_value(&self) -> &ValueHandle {
        &self.metadata
    }
    pub fn calls_value(&self) -> &ValueHandle {
        &self.recorded_calls
    }
    pub fn calls(&self, factory: &Arc<GraphValueFactory>) -> Result<Vec<MockCall>> {
        let g = GraphBuilder(factory.clone());
        factory.heap().adopt(&self.recorded_calls)?;
        g.members(&self.recorded_calls)?
            .into_iter()
            .map(|entry| {
                let context = g.get(&entry, "context")?;
                let options = g.get(&entry, "options")?;
                let options = if factory.heap().kind(&options)? == ValueKind::Undefined {
                    None
                } else {
                    Some(options)
                };
                Ok(MockCall { context, options })
            })
            .collect()
    }
    pub fn push(&self, handler: MockHandler) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extras
            .push_back(handler);
    }
    pub fn reset(&self, factory: &Arc<GraphValueFactory>) -> Result<()> {
        factory.heap().adopt(&self.metadata)?;
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extras
            .clear();
        // Original order: extras length=0, calls length=0, then counter=0.
        factory.heap().assign(
            &self.recorded_calls,
            "length".into(),
            factory.heap().number(0.0)?,
            factory.runtime(),
        )?;
        self.state.lock().unwrap_or_else(|e| e.into_inner()).counter = 0.0;
        Ok(())
    }
    pub fn stream(
        self: &Arc<Self>,
        _model: &MockModelHandle,
        factory: Arc<GraphValueFactory>,
        context: ValueHandle,
        original_options: Option<ValueHandle>,
        options: CallOptions,
    ) -> ActualAssistantStream {
        stream_mock(&self.model(), factory, context, original_options, options)
    }
    fn pull(
        self: &Arc<Self>,
        factory: Arc<GraphValueFactory>,
    ) -> BoxFuture<'static, Result<Option<MockHandler>>> {
        let source = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.exhausted {
                None
            } else {
                state.source.clone()
            }
        };
        let next = source.map(|source| source.next(factory));
        let owner = self.clone();
        Box::pin(async move {
            if let Some(next) = next {
                match next.await? {
                    Some(handler) => return Ok(Some(handler)),
                    None => {
                        owner
                            .state
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .exhausted = true
                    }
                }
            }
            let mut state = owner.state.lock().unwrap_or_else(|e| e.into_inner());
            Ok(state.extras.pop_front().or_else(|| state.fallback.clone()))
        })
    }
}

pub fn create_mock_model(
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn MockBindings>,
    options: ValueHandle,
) -> Result<Arc<MockModel>> {
    create(factory, bindings, options, None)
}
/// Explicit Rust iterator/future adaptation; native options still carry all
/// model fields. This replaces only the language's source/handler representation.
pub fn create_mock_model_with_source(
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn MockBindings>,
    options: ValueHandle,
    source: Arc<dyn MockSource>,
    fallback: Option<MockHandler>,
) -> Result<Arc<MockModel>> {
    create(factory, bindings, options, Some((source, fallback)))
}
fn create(
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn MockBindings>,
    options: ValueHandle,
    override_source: Option<(Arc<dyn MockSource>, Option<MockHandler>)>,
) -> Result<Arc<MockModel>> {
    let g = GraphBuilder(factory.clone());
    let heap = factory.heap();
    heap.adopt(&options)?;
    let nullish = |key: &str, default: ValueHandle| -> Result<ValueHandle> {
        let value = g.get(&options, key)?;
        Ok(if g.is_nullish(&value)? {
            default
        } else {
            value
        })
    };
    let id = nullish("id", g.text("mock-model")?)?;
    // The original reads options.id twice for id and name.
    let name = nullish("id", g.text("mock-model")?)?;
    let provider = nullish("provider", g.text("mock")?)?;
    let identity = bindings.classify_model(&factory, &provider, &id)?;
    let base_url = nullish("baseUrl", g.text("mock://")?)?;
    let reasoning = nullish("reasoning", heap.boolean(false)?)?;
    let cost = nullish("cost", bindings.zero_cost())?;
    let context_window = nullish("contextWindow", heap.number(200_000.0)?)?;
    let max_tokens = nullish("maxTokens", heap.number(32_768.0)?)?;
    let source = if let Some((source, _)) = &override_source {
        Some(source.clone())
    } else if heap.kind(&g.get(&options, "responses")?)? == ValueKind::Undefined {
        None
    } else {
        Some(Arc::new(NativeSource::from_source(
            &factory,
            bindings.clone(),
            &g.get(&options, "responses")?,
        )?) as Arc<dyn MockSource>)
    };
    let exhausted = if override_source.is_some() {
        false
    } else {
        heap.kind(&g.get(&options, "responses")?)? == ValueKind::Undefined
    };
    let fallback = if let Some((_, fallback)) = override_source {
        fallback
    } else {
        let handler = g.get(&options, "handler")?;
        if heap.kind(&handler)? == ValueKind::Undefined {
            None
        } else {
            Some(MockHandler::native(&factory, handler)?)
        }
    };
    let metadata = g.object([
        ("id", id),
        ("name", name),
        ("api", g.text(MOCK_API)?),
        ("provider", provider),
        ("identity", identity),
        ("baseUrl", base_url),
        ("reasoning", reasoning),
        ("input", heap.array(vec![g.text("text")?])?),
        ("cost", cost),
        ("contextWindow", context_window),
        ("maxTokens", max_tokens),
        ("compat", heap.undefined()?),
    ])?;
    let recorded_calls = heap.array_with_length(0)?;
    g.define(&metadata, "calls", recorded_calls.clone())?;
    Ok(Arc::new(MockModel {
        metadata,
        recorded_calls,
        bindings,
        state: Arc::new(Mutex::new(State {
            extras: VecDeque::new(),
            exhausted,
            source,
            fallback,
            counter: 0.0,
        })),
    }))
}

pub type MockStreamFn = fn(
    &MockModelHandle,
    Arc<GraphValueFactory>,
    ValueHandle,
    Option<ValueHandle>,
    CallOptions,
) -> ActualAssistantStream;
#[derive(Clone, Copy)]
pub struct MockApi {
    pub stream: MockStreamFn,
    pub stream_simple: MockStreamFn,
}
pub trait MockApiRegistrar {
    fn register_mock_api(&self, api: MockApi, source_id: WireString) -> Result<()>;
}
pub fn register_mock_api(
    registrar: &dyn MockApiRegistrar,
    source_id: Option<WireString>,
) -> Result<()> {
    registrar.register_mock_api(
        MockApi {
            stream: stream_wrapper,
            stream_simple: stream_mock,
        },
        source_id.unwrap_or_else(|| "pi-ai/mock".into()),
    )
}
fn stream_wrapper(
    model: &MockModelHandle,
    factory: Arc<GraphValueFactory>,
    context: ValueHandle,
    raw: Option<ValueHandle>,
    options: CallOptions,
) -> ActualAssistantStream {
    stream_mock(model, factory, context, raw, options)
}
/// Formal actual provider entry. Its stream failures remain separate from
/// configured model terminal errors (exact OMP stream.fail versus push(error)).
pub fn stream_mock(
    model: &MockModelHandle,
    factory: Arc<GraphValueFactory>,
    context: ValueHandle,
    raw_options: Option<ValueHandle>,
    options: CallOptions,
) -> ActualAssistantStream {
    let (sink, stream, receipt) = ActualAssistantStream::channel();
    let guard = receipt.owner_settlement_guard();
    let prepared = (|| {
        let model = match model {
            MockModelHandle::Mock(model) => model,
            MockModelHandle::Foreign(_, bindings) => return Err(bindings.validation_error(&factory, "streamMock called with a model not produced by createMockModel(). Pass a MockModel instance.".into())?),
        };
        model.bindings.check_invocation(&factory)?;
        factory.heap().adopt(&context)?;
        if let Some(raw) = &raw_options {
            factory.heap().adopt(raw)?;
        }
        factory.heap().adopt(&model.metadata)?;
        let g = GraphBuilder(factory.clone());
        let entry = g.object([
            ("context", context.clone()),
            (
                "options",
                raw_options.clone().unwrap_or(factory.heap().undefined()?),
            ),
        ])?;
        g.push(&model.recorded_calls, entry)?;
        let started = crate::types::now_ms() as f64;
        let perf = Instant::now();
        let pull = model.pull(factory.clone());
        Ok((model.clone(), started, perf, pull))
    })();
    tokio::spawn(async move {
        let _guard = guard;
        let result = match prepared {
            Ok((model, started, perf, pull)) => {
                run(
                    &sink,
                    &receipt,
                    model,
                    factory,
                    context,
                    raw_options,
                    options,
                    started,
                    perf,
                    pull,
                )
                .await
            }
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            receipt.record(error);
        }
    });
    stream
}
impl ModelProvider for MockModel {
    fn stream(&self, _: &Model, _: &Context, _: CallOptions) -> AssistantStream {
        // This facade cannot preserve raw tool arguments/partial graph identity.
        // Actual consumers must select stream_actual or stream_mock.
        let (sink, events) = EventSink::channel();
        tokio::spawn(async move {
            let mut message = crate::AssistantMessage::empty(MOCK_API, "mock", "mock-model");
            message.stop_reason = StopReason::Error;
            message.error_message = Some(
                "Mock provider requires its actual graph entry; typed facade is unsupported".into(),
            );
            sink.push(Event::Error {
                reason: StopReason::Error,
                error: message,
            })
            .await;
        });
        events
    }
    fn stream_actual(
        &self,
        _: &Model,
        context: &GraphProviderContext,
        factory: Arc<GraphValueFactory>,
        options: CallOptions,
    ) -> Result<ActualAssistantStream> {
        context.check_lineage(&factory)?;
        let original = context
            .original_context
            .clone()
            .ok_or(ValueError::UnsupportedOperation(
                "Mock provider needs original Context",
            ))?;
        Ok(stream_mock(
            &MockModelHandle::Mock(Arc::new(self.clone())),
            factory,
            original,
            None,
            options,
        ))
    }
}

async fn publish(
    sink: &EventSink<ActualAssistantEvent>,
    receipt: &ActualStreamFailureReceipt,
    event: ActualAssistantEvent,
) {
    receipt.retain_terminal(&event);
    // Local mock has no external effect to replay. A dropped consumer may stop
    // delivery, but cannot erase the known terminal/failure settlement receipt.
    sink.push(event).await;
}
#[allow(clippy::too_many_arguments)]
async fn run(
    sink: &EventSink<ActualAssistantEvent>,
    receipt: &ActualStreamFailureReceipt,
    model: Arc<MockModel>,
    factory: Arc<GraphValueFactory>,
    context: ValueHandle,
    raw_options: Option<ValueHandle>,
    options: CallOptions,
    started: f64,
    perf: Instant,
    pull: BoxFuture<'static, Result<Option<MockHandler>>>,
) -> Result<()> {
    let g = GraphBuilder(factory.clone());
    let heap = factory.heap();
    let handler = pull.await?;
    model.bindings.check_invocation(&factory)?;
    let handler = match handler {
        Some(MockHandler::Response(value)) if heap.kind(&value)? == ValueKind::Undefined => {
            return Err(no_handler_error(&g, &model)?);
        }
        Some(handler) => handler,
        None => return Err(no_handler_error(&g, &model)?),
    };
    let response = match handler {
        MockHandler::Response(value) => value,
        MockHandler::Callable(callable) => {
            let args = [
                context.clone(),
                raw_options.clone().unwrap_or(heap.undefined()?),
            ];
            let value = heap.call(&callable, &heap.undefined()?, &args, factory.runtime())?;
            model.bindings.await_value(factory.clone(), value).await?
        }
        MockHandler::Rust(handler) => {
            handler(context, raw_options.clone(), factory.clone()).await?
        }
    };
    model.bindings.check_invocation(&factory)?;
    heap.adopt(&response)?;
    notify(&g, &model, &response, raw_options.as_ref(), &options).await?;
    model.bindings.check_invocation(&factory)?;
    let delay = g.get(&response, "delayMs")?;
    if heap.truthy(&delay)?
        && heap.to_number(&g.get(&response, "delayMs")?, factory.runtime())? > 0.0
    {
        let ms = heap.to_number(&g.get(&response, "delayMs")?, factory.runtime())?;
        let ms = if !ms.is_finite() || ms > 2_147_483_647.0 || ms < 1.0 {
            1.0
        } else {
            ms.trunc()
        };
        let cancel = if let Some(raw) = &raw_options {
            let signal = g.get(raw, "signal")?;
            if heap.kind(&signal)? == ValueKind::Undefined {
                tokio_util::sync::CancellationToken::new()
            } else {
                model.bindings.signal_token(&factory, &signal)?
            }
        } else {
            options.cancel.clone()
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                return terminal(sink, receipt, &g, &model, started, perf, StopReason::Aborted, "Mock aborted during delay.".into()).await;
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs_f64(ms / 1000.0)) => {}
        }
    }
    model.bindings.check_invocation(&factory)?;
    let thrown = g.get(&response, "throw")?;
    if heap.kind(&thrown)? != ValueKind::Undefined {
        let message = if heap.kind(&g.get(&response, "throw")?)? == ValueKind::String {
            g.string(&g.get(&response, "throw")?)?
        } else if heap.instance_of(
            &g.get(&response, "throw")?,
            model.bindings.error_constructor(),
            factory.runtime(),
        )? {
            g.string(&g.get(&g.get(&response, "throw")?, "message")?)?
        } else {
            heap.to_string(&g.get(&response, "throw")?, factory.runtime())?
        };
        return terminal(
            sink,
            receipt,
            &g,
            &model,
            started,
            perf,
            StopReason::Error,
            message,
        )
        .await;
    }
    let content = heap.array_with_length(0)?;
    let partial = g.object([
        ("role", g.text("assistant")?),
        ("content", content.clone()),
        ("api", g.get(&model.metadata, "api")?),
        ("provider", g.get(&model.metadata, "provider")?),
        ("model", g.get(&model.metadata, "id")?),
        ("responseId", g.get(&response, "responseId")?),
        ("usage", empty_usage(&g)?),
        ("stopReason", g.text("stop")?),
        ("timestamp", heap.number(started)?),
    ])?;
    publish(
        sink,
        receipt,
        Event::Start {
            partial: partial.clone(),
        },
    )
    .await;
    let inputs = g.get(&response, "content")?;
    let inputs = if g.is_nullish(&inputs)? {
        heap.array_with_length(0)?
    } else {
        inputs
    };
    let mut iterator = heap.get_iterator(&inputs, factory.runtime())?;
    while let Some(input) = heap.iterator_next(&mut iterator, factory.runtime())? {
        let result: Result<()> = async {
            let block = normalize(&g, &model, input)?;
            g.push(&content, block.clone())?;
            let length = heap.array_length(&content, factory.runtime())?;
            let index = heap.number(f64::from(length) - 1.0)?;
            if literal(&g, &g.get(&block, "type")?, "text")? {
                publish(
                    sink,
                    receipt,
                    Event::TextStart {
                        content_index: index.clone(),
                        partial: partial.clone(),
                    },
                )
                .await;
                let text = g.string(&g.get(&block, "text")?)?;
                publish(
                    sink,
                    receipt,
                    Event::TextDelta {
                        content_index: index.clone(),
                        delta: text,
                        partial: partial.clone(),
                    },
                )
                .await;
                let text = g.string(&g.get(&block, "text")?)?;
                publish(
                    sink,
                    receipt,
                    Event::TextEnd {
                        content_index: index,
                        content: text,
                        partial: partial.clone(),
                    },
                )
                .await;
            } else if literal(&g, &g.get(&block, "type")?, "thinking")? {
                publish(
                    sink,
                    receipt,
                    Event::ThinkingStart {
                        content_index: index.clone(),
                        partial: partial.clone(),
                    },
                )
                .await;
                let text = g.string(&g.get(&block, "thinking")?)?;
                publish(
                    sink,
                    receipt,
                    Event::ThinkingDelta {
                        content_index: index.clone(),
                        delta: text,
                        partial: partial.clone(),
                    },
                )
                .await;
                let text = g.string(&g.get(&block, "thinking")?)?;
                publish(
                    sink,
                    receipt,
                    Event::ThinkingEnd {
                        content_index: index,
                        content: text,
                        partial: partial.clone(),
                    },
                )
                .await;
            } else {
                let args = g.get(&block, "arguments")?;
                let serialized = if heap.kind(&args)? == ValueKind::String {
                    g.get(&block, "arguments")?
                } else {
                    heap.stringify(
                        &g.get(&block, "arguments")?,
                        ara_rpc::value_heap::StringifyMode::Plain,
                        None,
                        factory.runtime(),
                    )?
                    .map(|text| heap.string(text))
                    .transpose()?
                    .unwrap_or(heap.undefined()?)
                };
                publish(
                    sink,
                    receipt,
                    Event::ToolcallStart {
                        content_index: index.clone(),
                        partial: partial.clone(),
                    },
                )
                .await;
                publish(
                    sink,
                    receipt,
                    Event::ToolcallDelta {
                        content_index: index.clone(),
                        delta: serialized,
                        partial: partial.clone(),
                    },
                )
                .await;
                publish(
                    sink,
                    receipt,
                    Event::ToolcallEnd {
                        content_index: index,
                        tool_call: block,
                        partial: partial.clone(),
                    },
                )
                .await;
            }
            Ok(())
        }
        .await;
        if let Err(primary) = result {
            // Body abrupt completion closes the selected content iterator; an
            // already thrown primary completion retains original identity.
            let _ = heap.iterator_close(&mut iterator, factory.runtime());
            return Err(primary);
        }
    }
    let some = g.get(&content, "some")?;
    let predicate = heap.fresh_compiled_function(&HAS_TOOL_CALL, &[])?;
    let has_tool = heap.truthy(&heap.call(&some, &content, &[predicate], factory.runtime())?)?;
    let configured = g.get(&response, "stopReason")?;
    let reason = if g.is_nullish(&configured)? {
        if has_tool {
            StopReason::ToolUse
        } else {
            StopReason::Stop
        }
    } else {
        parse_reason(&g.string(&configured)?)?
    };
    g.put(&partial, "stopReason", g.text(reason.as_str())?)?;
    g.put(&partial, "stopDetails", g.get(&response, "stopDetails")?)?;
    g.put(&partial, "fallbackCreditHandle", g.get(&response, "fallbackCreditHandle")?)?;
    g.put(&partial, "errorMessage", g.get(&response, "errorMessage")?)?;
    g.put(
        &partial,
        "usage",
        merge_usage(&g, &g.get(&response, "usage")?)?,
    )?;
    g.put(&partial, "serviceTier", g.get(&response, "serviceTier")?)?;
    g.put(
        &partial,
        "duration",
        heap.number(perf.elapsed().as_secs_f64() * 1000.0)?,
    )?;
    if matches!(reason, StopReason::Error | StopReason::Aborted) {
        let error = heap.spread(&partial, factory.runtime())?;
        let message = g.get(&partial, "errorMessage")?;
        g.define(
            &error,
            "errorMessage",
            if g.is_nullish(&message)? {
                g.text("mock error")?
            } else {
                message
            },
        )?;
        publish(sink, receipt, Event::Error { reason, error }).await;
    } else {
        publish(
            sink,
            receipt,
            Event::Done {
                reason,
                message: partial,
            },
        )
        .await;
    }
    Ok(())
}
fn parse_reason(value: &WireString) -> Result<StopReason> {
    Ok(if value.equals_ascii("stop") {
        StopReason::Stop
    } else if value.equals_ascii("length") {
        StopReason::Length
    } else if value.equals_ascii("toolUse") {
        StopReason::ToolUse
    } else if value.equals_ascii("error") {
        StopReason::Error
    } else if value.equals_ascii("aborted") {
        StopReason::Aborted
    } else {
        return Err(ValueError::TypeError("Mock stopReason is invalid".into()));
    })
}
fn assistant(
    g: &GraphBuilder,
    model: &MockModel,
    content: ValueHandle,
    timestamp: f64,
) -> Result<ValueHandle> {
    g.object([
        ("role", g.text("assistant")?),
        ("content", content),
        ("api", g.get(&model.metadata, "api")?),
        ("provider", g.get(&model.metadata, "provider")?),
        ("model", g.get(&model.metadata, "id")?),
        ("usage", empty_usage(g)?),
        ("stopReason", g.text("stop")?),
        ("timestamp", g.0.heap().number(timestamp)?),
    ])
}
async fn terminal(
    sink: &EventSink<ActualAssistantEvent>,
    receipt: &ActualStreamFailureReceipt,
    g: &GraphBuilder,
    model: &MockModel,
    started: f64,
    perf: Instant,
    reason: StopReason,
    message: WireString,
) -> Result<()> {
    let failure = assistant(g, model, g.0.heap().array_with_length(0)?, started)?;
    g.put(&failure, "stopReason", g.text(reason.as_str())?)?;
    g.define(&failure, "errorMessage", g.text(message)?)?;
    g.define(
        &failure,
        "duration",
        g.0.heap().number(perf.elapsed().as_secs_f64() * 1000.0)?,
    )?;
    publish(
        sink,
        receipt,
        Event::Start {
            partial: failure.clone(),
        },
    )
    .await;
    publish(
        sink,
        receipt,
        Event::Error {
            reason,
            error: failure,
        },
    )
    .await;
    Ok(())
}
fn normalize(g: &GraphBuilder, model: &MockModel, input: ValueHandle) -> Result<ValueHandle> {
    let heap = g.0.heap();
    if heap.kind(&input)? == ValueKind::String {
        return g.object([("type", g.text("text")?), ("text", input)]);
    }
    if literal(g, &g.get(&input, "type")?, "toolCall")? {
        let id = g.get(&input, "id")?;
        let id = if g.is_nullish(&id)? {
            let n = {
                let mut state = model.state.lock().unwrap_or_else(|e| e.into_inner());
                state.counter += 1.0;
                state.counter
            };
            g.text(crate::actual_provider::concat(
                &"mock-tc-".into(),
                &heap.to_string(&heap.number(n)?, g.0.runtime())?,
            ))?
        } else {
            id
        };
        let name = g.get(&input, "name")?;
        let args = g.get(&input, "arguments")?;
        let args = if heap.kind(&args)? == ValueKind::String {
            g.get(&input, "arguments")?
        } else {
            heap.spread(&g.get(&input, "arguments")?, g.0.runtime())?
        };
        return g.object([
            ("type", g.text("toolCall")?),
            ("id", id),
            ("name", name),
            ("arguments", args),
        ]);
    }
    Ok(input)
}
fn empty_usage(g: &GraphBuilder) -> Result<ValueHandle> {
    let zero = g.0.heap().number(0.0)?;
    let cost = g.object([
        ("input", zero.clone()),
        ("output", zero.clone()),
        ("cacheRead", zero.clone()),
        ("cacheWrite", zero.clone()),
        ("total", zero.clone()),
    ])?;
    g.object([
        ("input", zero.clone()),
        ("output", zero.clone()),
        ("cacheRead", zero.clone()),
        ("cacheWrite", zero.clone()),
        ("totalTokens", zero),
        ("cost", cost),
    ])
}
fn merge_usage(g: &GraphBuilder, partial: &ValueHandle) -> Result<ValueHandle> {
    let heap = g.0.heap();
    let base = empty_usage(g)?;
    if !heap.truthy(partial)? {
        return Ok(base);
    }
    let merged = heap.spread(&base, g.0.runtime())?;
    heap.spread_into(&merged, partial, g.0.runtime())?;
    let cost = g.get(partial, "cost")?;
    let cost_provided = heap.kind(&cost)? != ValueKind::Undefined;
    if cost_provided {
        let cost = heap.spread(&g.get(&base, "cost")?, g.0.runtime())?;
        heap.spread_into(&cost, &g.get(partial, "cost")?, g.0.runtime())?;
        g.put(&merged, "cost", cost)?;
    }
    if heap.kind(&g.get(partial, "totalTokens")?)? == ValueKind::Undefined {
        let orchestration = g.get(&merged, "orchestration")?;
        let component = |key: &str| -> Result<ValueHandle> {
            let v = if g.is_nullish(&orchestration)? {
                heap.undefined()?
            } else {
                g.get(&orchestration, key)?
            };
            Ok(if g.is_nullish(&v)? {
                heap.number(0.0)?
            } else {
                v
            })
        };
        let mut total = g.get(&merged, "input")?;
        for key in ["output", "cacheRead", "cacheWrite"] {
            total = heap.add(&total, &g.get(&merged, key)?, g.0.runtime())?;
        }
        for key in ["input", "output", "cacheRead"] {
            total = heap.add(&total, &component(key)?, g.0.runtime())?;
        }
        g.put(&merged, "totalTokens", total)?;
    }
    if cost_provided {
        let partial_cost = g.get(partial, "cost")?;
        let total = if g.is_nullish(&partial_cost)? {
            heap.undefined()?
        } else {
            g.get(&partial_cost, "total")?
        };
        if heap.kind(&total)? == ValueKind::Undefined {
            let cost = g.get(&merged, "cost")?;
            let mut total = g.get(&cost, "input")?;
            for key in ["output", "cacheRead", "cacheWrite"] {
                total = heap.add(&total, &g.get(&cost, key)?, g.0.runtime())?;
            }
            g.put(&cost, "total", total)?;
        }
    }
    Ok(merged)
}
async fn notify(
    g: &GraphBuilder,
    model: &MockModel,
    response: &ValueHandle,
    raw_options: Option<&ValueHandle>,
    options: &CallOptions,
) -> Result<()> {
    let heap = g.0.heap();
    let raw_headers = g.get(response, "responseHeaders")?;
    if !heap.truthy(&raw_headers)? {
        return Ok(());
    }
    let raw_callback = match raw_options {
        Some(raw) => g.get(raw, "onResponse")?,
        None => heap.undefined()?,
    };
    if !heap.truthy(&raw_callback)? && options.on_response.is_none() {
        return Ok(());
    }
    let headers = heap.object()?;
    // Object.entries snapshots values before calling toLowerCase in the loop.
    let raw_headers = g.get(response, "responseHeaders")?;
    let entries = heap
        .own_enumerable_string_keys(&raw_headers, g.0.runtime())?
        .into_iter()
        .map(|key| {
            heap.get(
                &raw_headers,
                &PropertyKey::String(key.clone()),
                g.0.runtime(),
            )
            .map(|value| (key, value))
        })
        .collect::<Result<Vec<_>>>()?;
    for (key, value) in entries {
        let key = g.text(key)?;
        let method = g.get(&key, "toLowerCase")?;
        let lower = heap.call(&method, &key, &[], g.0.runtime())?;
        let lower = g.string(&lower)?;
        heap.assign(&headers, lower.into(), value, g.0.runtime())?;
    }
    let status = g.get(response, "responseStatus")?;
    let status = if g.is_nullish(&status)? {
        heap.number(200.0)?
    } else {
        status
    };
    let metadata = g.object([("status", status.clone()), ("headers", headers.clone())])?;
    let request_id = g.get(response, "responseRequestId")?;
    if heap.kind(&request_id)? != ValueKind::Undefined {
        g.define(
            &metadata,
            "requestId",
            g.get(response, "responseRequestId")?,
        )?;
    }
    if heap.truthy(&raw_callback)? {
        let raw_callback = g.get(
            raw_options.expect("selected original callback has original options"),
            "onResponse",
        )?;
        let value = heap.call(
            &raw_callback,
            raw_options.expect("selected original callback receiver"),
            &[metadata, model.metadata.clone()],
            g.0.runtime(),
        )?;
        model.bindings.await_value(g.0.clone(), value).await?;
    } else if let Some(callback) = &options.on_response {
        // Derived transport metadata is the sole legacy typed adapter. No
        // Context, content, arguments or exception identity enters serde.
        let mut observed = std::collections::BTreeMap::new();
        for key in heap.own_enumerable_string_keys(&headers, g.0.runtime())? {
            let value = heap.get(&headers, &PropertyKey::String(key.clone()), g.0.runtime())?;
            observed.insert(
                String::from_utf16_lossy(key.units()),
                String::from_utf16_lossy(g.string(&value)?.units()),
            );
        }
        let typed = model_endpoint(g, model)?;
        callback
            .notify(
                crate::ProviderResponseMetadata {
                    status: heap.to_number(&status, g.0.runtime())? as u16,
                    headers: observed,
                    request_id: if heap.kind(&request_id)? == ValueKind::Undefined {
                        None
                    } else {
                        Some(Some(String::from_utf16_lossy(
                            g.string(&request_id)?.units(),
                        )))
                    },
                    metadata: None,
                },
                Some(typed),
            )
            .await
            .map_err(|_| {
                ValueError::UnsupportedOperation("Mock typed onResponse callback failed")
            })?;
    }
    Ok(())
}
fn model_endpoint(g: &GraphBuilder, model: &MockModel) -> Result<Model> {
    Ok(Model {
        id: String::from_utf16_lossy(g.string(&g.get(&model.metadata, "id")?)?.units()),
        api: MOCK_API.into(),
        provider: String::from_utf16_lossy(g.string(&g.get(&model.metadata, "provider")?)?.units()),
        base_url: String::from_utf16_lossy(g.string(&g.get(&model.metadata, "baseUrl")?)?.units()),
        reasoning: g.0.heap().truthy(&g.get(&model.metadata, "reasoning")?)?,
        max_tokens: Some(
            g.0.heap()
                .to_number(&g.get(&model.metadata, "maxTokens")?, g.0.runtime())?
                as u64,
        ),
        context_window: Some(
            g.0.heap()
                .to_number(&g.get(&model.metadata, "contextWindow")?, g.0.runtime())?,
        ),
        tokenizer: None,
    })
}
