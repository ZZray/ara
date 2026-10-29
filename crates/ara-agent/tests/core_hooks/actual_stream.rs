//! Observation helper called only by the existing coarse R family.
//! Controlled actual port + real fixed H2 scanner; no real transport/model proof.
use super::*;
use ara_ai::dialect::{DeepSeekInbandUnitsScanner, InbandNameValue, InbandScanEvent, Qwen3InbandUnitsScanner};
use ara_rpc::{
    WireString,
    value_heap::{
        CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueError,
        ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
    },
};

#[derive(Default)]
struct Trace {
    started: Notify,
    owner: Mutex<Option<AgentMessage>>,
    order: Mutex<Vec<&'static str>>,
    events: Mutex<Vec<AgentEvent>>,
    final_root: Mutex<Option<ValueHandle>>,
    arguments: Mutex<Option<ValueHandle>>,
    tools: Mutex<Option<Arc<[ValueHandle]>>>,
    original_tools: Mutex<Option<ValueHandle>>,
    source: Mutex<Option<Arc<dyn ActualToolSource>>>,
    source_admissions: AtomicUsize,
    source_retained_at_end: AtomicUsize,
    names: AtomicUsize,
    tools_retained_at_end: AtomicUsize,
}
struct FixtureRuntime {
    heap: Arc<ValueHeap>,
    trace: Arc<Trace>,
}
impl ValueRuntime for FixtureRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        match request {
            RuntimeRequest::Call { capability: CapabilityId(701), .. } => {
                let count = self.trace.names.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(RuntimeReply::Value(if count.is_multiple_of(3) {
                    self.heap.number(17.0)?
                } else {
                    self.heap.string("echo".into())?
                }))
            }
            RuntimeRequest::Call { capability: CapabilityId(702), .. } => {
                Ok(RuntimeReply::Value(self.heap.string("{}".into())?))
            }
            request => Err(ValueError::MissingCapability(request.capability())),
        }
    }
}
fn put(values: &SessionValues, target: &ValueHandle, key: &str, value: ValueHandle) {
    values.heap().define_own_data(target, key.into(), value, &NoRuntime).unwrap();
}
fn get(values: &SessionValues, target: &ValueHandle, key: &str) -> ValueHandle {
    values.heap().get(target, &key.into(), values.runtime()).unwrap()
}
fn root(values: &SessionValues, model: &Model) -> ValueHandle {
    let empty = AssistantMessage::empty(&model.api, &model.provider, &model.id);
    values.heap().import_serde(&serde_json::to_value(Message::Assistant(empty)).unwrap()).unwrap()
}
fn member(
    values: &SessionValues,
    id: WireString,
    name: InbandNameValue<ValueHandle>,
    arguments: ValueHandle,
) -> ValueHandle {
    let call = values.heap().object().unwrap();
    put(values, &call, "type", values.heap().string("toolCall".into()).unwrap());
    put(values, &call, "id", values.heap().string(id).unwrap());
    put(
        values,
        &call,
        "name",
        match name {
            InbandNameValue::Source(name) => values.heap().string(name).unwrap(),
            InbandNameValue::Actual(name) => name,
        },
    );
    put(values, &call, "arguments", arguments);
    call
}
fn produce_call(values: &SessionValues, third: bool, split: bool) -> (ValueHandle, ValueHandle) {
    let input = WireString::from(if third {
        "<tool_call>{}</tool_call>"
    } else {
        "<|DSML|tool_calls><|DSML|invoke name=\"echo\"><|DSML|parameter name=\"text\" string=\"true\">parsed</|DSML|parameter></|DSML|invoke></|DSML|tool_calls>"
    });
    let chunks = if split {
        input.units().iter().map(|unit| WireString::from_units(vec![*unit])).collect::<Vec<_>>()
    } else {
        vec![input]
    };
    let mut events = Vec::new();
    if third {
        let mut scanner = Qwen3InbandUnitsScanner::new(values.factory().clone());
        for chunk in chunks {
            events.extend(scanner.try_feed_units(&chunk).unwrap());
        }
        events.extend(scanner.try_flush_units().unwrap());
    } else {
        let mut scanner = DeepSeekInbandUnitsScanner::new(values.factory().clone()).unwrap();
        for chunk in chunks {
            events.extend(scanner.try_feed_units(&chunk).unwrap());
        }
        events.extend(scanner.try_flush_units().unwrap());
    }
    let (id, name, arguments) = events
        .into_iter()
        .find_map(|event| match event {
            InbandScanEvent::ToolEnd { id, name, arguments, .. } => Some((id, name, arguments)),
            _ => None,
        })
        .expect("real H2 produced one terminal tool call");
    if third {
        assert!(
            matches!(&name, InbandNameValue::Actual(name) if matches!(values.heap().snapshot(name).unwrap(), ValueSnapshot::Number(17.0)))
        );
    }
    (member(values, id, name, arguments.clone()), arguments)
}

struct ActualPort {
    values: Arc<SessionValues>,
    trace: Arc<Trace>,
    mode: &'static str,
    streams: AtomicUsize,
}
impl OwnedProviderContextPort for ActualPort {
    fn project_context(&self, _: &Model, _: Context, _: bool) -> Result<Context, ProviderContextError> {
        panic!("actual caller used typed projection")
    }
    fn stream(&self, _: &dyn ModelProvider, _: &Model, _: &Context, _: CallOptions) -> AssistantStream {
        panic!("actual caller used typed stream")
    }
    fn project_context_actual(
        &self,
        _: &Model,
        context: ActualProviderContext,
        side: bool,
        values: &SessionValues,
    ) -> Result<ActualProviderContext, ActualProviderError> {
        assert!(!side);
        assert!(Arc::ptr_eq(values.factory(), self.values.factory()));
        assert!(Arc::ptr_eq(context.tools.as_ref().unwrap(), self.trace.tools.lock().unwrap().as_ref().unwrap()));
        self.trace.order.lock().unwrap().push("owned-project");
        Ok(context)
    }
    fn stream_prepared_original_actual(
        &self,
        _: &dyn ModelProvider,
        model: &Model,
        context: &ActualProviderContext,
        tools: Option<ValueHandle>,
        values: Arc<SessionValues>,
        options: CallOptions,
    ) -> Result<ActualAssistantStream, ActualProviderError> {
        assert!(Arc::ptr_eq(&values, &self.values));
        let tools = if matches!(self.mode, "native" | "native-empty") {
            assert!(tools.is_none(), "native default must not capture prompt-tool snapshot");
            assert!(context.tools.is_some());
            get(&values, context.original_context.as_ref().unwrap(), "tools")
        } else {
            assert!(context.tools.is_none());
            tools.expect("owned snapshot")
        };
        assert!(tools.same_identity(self.trace.original_tools.lock().unwrap().as_ref().unwrap()));
        if self.mode == "native-empty" {
            assert_eq!(values.heap().array_length(&tools, values.runtime()).unwrap(), 0);
        } else {
            assert!(matches!(
                values.heap().snapshot(&get(&values, &get(&values, &tools, "0"), "postHook")).unwrap(),
                ValueSnapshot::Boolean(true)
            ));
        }
        self.streams.fetch_add(1, Ordering::SeqCst);
        self.trace.order.lock().unwrap().push("actual-stream");
        if self.mode == "producer-error" {
            return Err(ActualProviderError::Callback("actual producer error receipt".into()));
        }
        if self.mode == "producer-panic" {
            panic!("actual producer panic receipt");
        }
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let initial = root(&values, model);
        let trace = self.trace.clone();
        let mode = self.mode;
        tokio::spawn(async move {
            if tx.send(ActualAssistantEvent::Start { partial: initial.clone() }).await.is_err() {
                return;
            }
            trace.started.notified().await;
            let final_root = initial.clone();
            let content = get(&values, &final_root, "content");
            if matches!(mode, "tool" | "tool-unit" | "third-name" | "cancel" | "deadline") {
                let (call, arguments) = produce_call(&values, mode == "third-name", mode == "tool-unit");
                *trace.arguments.lock().unwrap() = Some(arguments.clone());
                put(&values, &content, "0", call.clone());
                if matches!(mode, "cancel" | "deadline") {
                    let pending = member(
                        &values,
                        "pending".into(),
                        InbandNameValue::Source("echo".into()),
                        values.heap().object().unwrap(),
                    );
                    put(&values, &content, "1", pending);
                }
                if tx
                    .send(ActualAssistantEvent::ToolcallEnd {
                        content_index: values.heap().number(0.0).unwrap(),
                        tool_call: call,
                        partial: final_root.clone(),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
                if matches!(mode, "cancel" | "deadline") {
                    options.cancel.cancelled().await;
                    return;
                }
                put(&values, &final_root, "stopReason", values.heap().string("toolUse".into()).unwrap());
            } else {
                let block = values.heap().import_serde(&json!({"type":"text","text":"actual text"})).unwrap();
                put(&values, &content, "0", block);
                if tx
                    .send(ActualAssistantEvent::TextDelta {
                        content_index: if mode == "foreign-index" {
                            ValueHeap::new().unwrap().number(-1.0).unwrap()
                        } else {
                            values.heap().number(0.0).unwrap()
                        },
                        delta: "actual text".into(),
                        partial: final_root.clone(),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
                put(&values, &final_root, "stopReason", values.heap().string("stop".into()).unwrap());
            }
            if mode == "eof" {
                return;
            }
            if mode == "bad-final" {
                put(&values, &final_root, "stopReason", values.heap().number(9.0).unwrap());
            }
            let final_root = if mode == "foreign" {
                ValueHeap::new()
                    .unwrap()
                    .import_serde(&json!({"role":"assistant","content":[],"stopReason":"stop"}))
                    .unwrap()
            } else {
                final_root
            };
            *trace.final_root.lock().unwrap() = Some(final_root.clone());
            let event = if mode == "error-event" {
                put(&values, &final_root, "stopReason", values.heap().string("error".into()).unwrap());
                ActualAssistantEvent::Error { reason: StopReason::Error, error: final_root }
            } else {
                ActualAssistantEvent::Done { reason: StopReason::Stop, message: final_root }
            };
            let _ = tx.send(event).await;
        });
        Ok(rx.into())
    }
}
struct HostSource {
    collection: ValueHandle,
    binding: ActualToolBinding,
    lookups: AtomicUsize,
    binds: AtomicUsize,
    fallbacks: AtomicUsize,
}
impl ActualToolSource for HostSource {
    fn current_tools(&self, values: &SessionValues) -> Result<Option<ValueHandle>, ActualProviderError> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        values.check_handle(&self.collection)?;
        Ok(Some(self.collection.clone()))
    }
    fn bind_selected(
        &self,
        selected: &ValueHandle,
        values: &SessionValues,
    ) -> Result<ActualToolBinding, ActualProviderError> {
        self.binds.fetch_add(1, Ordering::SeqCst);
        self.binding.check_selected(selected, values)?;
        Ok(self.binding.clone())
    }
    fn resolve_fallback(
        &self,
        _: &ValueHandle,
        _: &SessionValues,
    ) -> Result<Option<ActualToolBinding>, ActualProviderError> {
        self.fallbacks.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
}
struct ActualHooks {
    binding: ProviderContextBinding,
    values: Arc<SessionValues>,
    trace: Arc<Trace>,
    mode: &'static str,
    conversions: AtomicUsize,
    source: Arc<HostSource>,
}
#[async_trait]
impl LoopHooks for ActualHooks {
    fn actual_tool_source(
        &self,
        values: &SessionValues,
    ) -> Result<Option<Arc<dyn ActualToolSource>>, ActualProviderError> {
        assert!(Arc::ptr_eq(values.factory(), self.values.factory()));
        self.trace.source_admissions.fetch_add(1, Ordering::SeqCst);
        if self.mode == "source-admission-error" {
            return Err(ActualProviderError::Callback("source admission rejected".into()));
        }
        if self.mode == "source-none" {
            return Ok(None);
        }
        let source: Arc<dyn ActualToolSource> = self.source.clone();
        *self.trace.source.lock().unwrap() = Some(source.clone());
        self.trace.order.lock().unwrap().push("source-admission");
        Ok(Some(source))
    }
    fn provider_context_binding(&self) -> Option<ProviderContextBinding> {
        Some(self.binding.clone())
    }
    async fn convert_agent_messages_to_llm(&self, _: &[AgentMessage]) -> Result<Vec<Message>, RawContextError> {
        panic!("actual request used default typed conversion")
    }
    async fn convert_agent_messages_to_llm_actual(
        &self,
        messages: &[AgentMessage],
        values: &SessionValues,
    ) -> Result<Vec<ValueHandle>, ActualProviderError> {
        assert!(Arc::ptr_eq(values.factory(), self.values.factory()));
        self.conversions.fetch_add(1, Ordering::SeqCst);
        self.trace.order.lock().unwrap().push("actual-convert");
        messages
            .iter()
            .map(|message| match message.body_snapshot() {
                AgentMessageBody::Graph(root) => {
                    values.check_handle(&root)?;
                    Ok(root)
                }
                _ => Err(ActualProviderError::Unsupported("fixture actual history is Graph only")),
            })
            .collect()
    }
    async fn transform_provider_context_actual(
        &self,
        context: ActualProviderContext,
        _: &Model,
        values: &SessionValues,
    ) -> Result<ActualProviderContext, ActualProviderError> {
        let tools = context.tools.as_ref().unwrap().clone();
        if let Some(tool) = tools.first() {
            put(values, tool, "postHook", values.heap().boolean(true).unwrap());
        }
        *self.trace.tools.lock().unwrap() = Some(tools);
        *self.trace.original_tools.lock().unwrap() =
            Some(get(values, context.original_context.as_ref().unwrap(), "tools"));
        self.trace.order.lock().unwrap().push("actual-provider-transform");
        Ok(context)
    }
    async fn transform_assistant_graph(
        &self,
        owner: &AgentMessage,
        _: &CancellationToken,
        values: &SessionValues,
    ) -> Result<(), ActualProviderError> {
        assert!(self.trace.owner.lock().unwrap().as_ref().unwrap().same_record(owner));
        self.trace.order.lock().unwrap().push("actual-assistant-transform");
        let AgentMessageBody::Graph(root) = owner.body_snapshot() else { panic!("same Graph owner") };
        put(values, &root, "transformed", values.heap().boolean(true).unwrap());
        if self.mode == "transform-panic" {
            panic!("actual transform panic receipt");
        }
        if self.mode == "transform-error" {
            return Err(ActualProviderError::Value(ValueError::Thrown(root)));
        }
        Ok(())
    }
    fn supports_actual_turn_end(&self) -> bool {
        self.mode != "missing-turn-end"
    }
    async fn turn_end_actual(
        &self,
        owner: &AgentMessage,
        _: &[AgentMessage],
        _: &CancellationToken,
        values: &SessionValues,
    ) -> Result<Vec<AgentMessage>, ActualProviderError> {
        assert!(self.trace.owner.lock().unwrap().as_ref().unwrap().same_record(owner));
        assert_eq!(self.trace.order.lock().unwrap().last(), Some(&"turn_end"));
        self.trace.order.lock().unwrap().push("actual-turn-end-hook");
        if self.mode == "turn-end-error" {
            let AgentMessageBody::Graph(root) = owner.body_snapshot() else { panic!("Graph callback failure") };
            return Err(ActualProviderError::Value(ValueError::Thrown(root)));
        }
        assert!(Arc::ptr_eq(values.factory(), self.values.factory()));
        Ok(Vec::new())
    }
}
struct MissingHooks {
    binding: ProviderContextBinding,
}
#[async_trait]
impl LoopHooks for MissingHooks {
    fn provider_context_binding(&self) -> Option<ProviderContextBinding> {
        Some(self.binding.clone())
    }
}
struct ActualSink {
    trace: Arc<Trace>,
    cancel: CancellationToken,
    mode: &'static str,
}
#[async_trait]
impl AgentEventSink for ActualSink {
    async fn emit(&self, event: AgentEvent) {
        match &event {
            AgentEvent::MessageStart { message } => {
                if let AgentMessageBody::Graph(root) = message.body_snapshot()
                    && matches!(root.heap().snapshot(&root.heap().get(&root, &"role".into(), &NoRuntime).unwrap()).unwrap(), ValueSnapshot::String(role) if role.equals_ascii("assistant"))
                {
                    *self.trace.owner.lock().unwrap() = Some(message.clone());
                    self.trace.order.lock().unwrap().push("message_start");
                    self.trace.started.notify_one();
                }
            }
            AgentEvent::MessageEnd { message } | AgentEvent::ActualMessageEnd { message, .. } => {
                if self.trace.owner.lock().unwrap().as_ref().is_some_and(|owner| owner.same_record(message)) {
                    if let Some(tools) = self.trace.tools.lock().unwrap().as_ref()
                        && !matches!(self.mode, "missing-hooks" | "missing-turn-end" | "final-gate")
                    {
                        // Observe the original assistant, excluding input messages.
                        // The synchronous producer has released its tools argument.
                        // Besides this trace, Core must still retain the SAME Arc.
                        assert!(
                            Arc::strong_count(tools) >= 2,
                            "{} lost captured wire Arc before finalization",
                            self.mode
                        );
                        self.trace.tools_retained_at_end.fetch_add(1, Ordering::SeqCst);
                        if let Some(source) = self.trace.source.lock().unwrap().as_ref() {
                            // Local fixture + hook + trace own three references.
                            // Retained stream/final state must own the fourth SAME Arc.
                            assert!(Arc::strong_count(source) >= 4, "{} lost original Host source", self.mode);
                            self.trace.source_retained_at_end.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    self.trace.order.lock().unwrap().push("message_end");
                }
            }
            AgentEvent::ActualMessageUpdate { message, event } => {
                assert!(self.trace.owner.lock().unwrap().as_ref().unwrap().same_record(message));
                if self.mode == "cancel" && matches!(event, AssistantMessageEvent::ToolcallEnd { .. }) {
                    self.cancel.cancel();
                }
            }
            AgentEvent::ActualTurnEnd { .. } => self.trace.order.lock().unwrap().push("turn_end"),
            AgentEvent::AgentEnd { .. } | AgentEvent::ActualAgentEnd { .. } => {
                self.trace.order.lock().unwrap().push("agent_end")
            }
            _ => {}
        }
        self.trace.events.lock().unwrap().push(event);
    }
}

pub(super) async fn observe_existing_loop_actual_stream_to_owner() {
    for mode in [
        "healthy",
        "source-none",
        "source-admission-error",
        "tool",
        "tool-unit",
        "third-name",
        "cancel",
        "deadline",
        "eof",
        "error-event",
        "producer-error",
        "producer-panic",
        "foreign",
        "foreign-index",
        "native",
        "native-empty",
        "bad-final",
        "transform-error",
        "transform-panic",
        "turn-end-error",
        "missing-turn-end",
        "missing-hooks",
        "final-gate",
    ] {
        let heap = ValueHeap::new().unwrap();
        let trace = Arc::new(Trace::default());
        let runtime = Arc::new(FixtureRuntime { heap: heap.clone(), trace: trace.clone() });
        if mode == "third-name" {
            for (key, capability) in [("name", 701), ("arguments", 702)] {
                heap.define_own(
                    &heap.object_prototype(),
                    key.into(),
                    PropertyDescriptor::Accessor {
                        get: Some(CallableTarget::Host(CapabilityId(capability))),
                        set: None,
                        enumerable: true,
                        configurable: true,
                    },
                    &NoRuntime,
                )
                .unwrap();
            }
        }
        let values = Arc::new(SessionValues::new(heap.clone(), runtime).unwrap());
        let port =
            Arc::new(ActualPort { values: values.clone(), trace: trace.clone(), mode, streams: AtomicUsize::new(0) });
        let binding = ProviderContextBinding {
            actual_values: Some(values.clone()),
            owned_dialect: !matches!(mode, "native" | "native-empty"),
            owned_port: Some(port.clone()),
            ..Default::default()
        };
        let effects = Arc::new(Mutex::new(Vec::new()));
        let host_capability: Arc<dyn AgentTool> = tool(&effects);
        let original_source =
            heap.import_serde(&json!({"name":"echo","parameters":{"type":"object"},"original":true})).unwrap();
        let host_source = Arc::new(HostSource {
            collection: heap.array(vec![original_source.clone()]).unwrap(),
            binding: ActualToolBinding { source: original_source, capability: host_capability.clone() },
            lookups: AtomicUsize::new(0),
            binds: AtomicUsize::new(0),
            fallbacks: AtomicUsize::new(0),
        });
        let hooks = Arc::new(ActualHooks {
            binding: binding.clone(),
            values: values.clone(),
            trace: trace.clone(),
            mode,
            conversions: AtomicUsize::new(0),
            source: host_source.clone(),
        });
        let provider = Provider::new(Vec::new());
        let selected_hooks: Arc<dyn LoopHooks> =
            if mode == "missing-hooks" { Arc::new(MissingHooks { binding }) } else { hooks.clone() };
        let mut execution = config(provider.clone(), host_capability, selected_hooks);
        if mode == "native-empty" {
            execution.tools.clear();
        }
        if mode == "deadline" {
            execution.deadline = Some(std::time::Instant::now() + Duration::from_millis(200));
        }
        let cancel = CancellationToken::new();
        let sink = Arc::new(ActualSink { trace: trace.clone(), cancel: cancel.clone(), mode });
        let input_root = heap.import_serde(&json!({"role":"user","content":"actual input","timestamp":1})).unwrap();
        let input = AgentMessage::from_graph(input_root.clone(), None).unwrap();
        let agent = Agent::new_raw(execution, Vec::new()).unwrap();
        let final_gate_visits = Arc::new(Mutex::new(Vec::new()));
        if mode == "final-gate" {
            agent.set_final_provider_gate(Some(FinalCallback::new("typed-final-gate", &final_gate_visits, |_, _| {
                panic!("actual final prepared-request gate used typed callback")
            })));
        }
        let report = bounded(agent.prompt_raw(vec![input.clone()], cancel, sink)).await.unwrap();
        let expected = match mode {
            "healthy" | "source-none" | "native" | "native-empty" => RunEnd::Completed,
            "cancel" => RunEnd::Aborted,
            "deadline" => RunEnd::Deadline,
            _ => RunEnd::Error,
        };
        assert_eq!(report.end, expected, "{mode}");
        assert!(effects.lock().unwrap().is_empty(), "A never dispatched tools: {mode}");
        assert_eq!(host_source.lookups.load(Ordering::SeqCst), 0, "retention must not read source metadata: {mode}");
        assert_eq!(host_source.binds.load(Ordering::SeqCst), 0, "{mode}");
        assert_eq!(host_source.fallbacks.load(Ordering::SeqCst), 0, "{mode}");
        assert_eq!(
            trace.source_admissions.load(Ordering::SeqCst),
            usize::from(!matches!(mode, "missing-hooks" | "missing-turn-end")),
            "{mode}"
        );
        assert!(provider.contexts.lock().unwrap().is_empty(), "no typed provider fallback: {mode}");
        let current = agent.raw_messages().await;
        assert!(current[0].same_record(&input));
        assert!(matches!(current[0].body_snapshot(), AgentMessageBody::Graph(root) if root.same_identity(&input_root)));
        let events = trace.events.lock().unwrap();
        if mode == "foreign-index" {
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    AgentEvent::ActualTurnEnd { preparation_error: Some(error), .. }
                        if matches!(error.as_ref(), ActualProviderError::Value(ValueError::ForeignHeap))
                )),
                "foreign native index retains its lineage error"
            );
            assert!(
                !events.iter().any(|event| matches!(event, AgentEvent::ActualMessageUpdate { .. })),
                "foreign index is rejected before an update is published"
            );
        }
        assert_eq!(events.last().unwrap().type_name(), "agent_end", "{mode}");
        assert_eq!(events.iter().filter(|event| event.type_name() == "turn_end").count(), 1, "{mode}");
        let tool_events: Vec<_> =
            events.iter().filter(|event| event.type_name().starts_with("tool_execution")).collect();
        if matches!(mode, "tool" | "tool-unit" | "cancel" | "deadline") {
            // Full outer-turn handling publishes failed synthetic receipts for
            // retained calls. It never invokes this fixture's typed capability.
            let owner = trace.owner.lock().unwrap().as_ref().unwrap().clone();
            let AgentMessageBody::Graph(root) = owner.body_snapshot() else { panic!("original Graph assistant") };
            let call = get(&values, &get(&values, &root, "content"), "0");
            let ValueSnapshot::String(id) = heap.snapshot(&get(&values, &call, "id")).unwrap() else {
                panic!("original admitted call ID")
            };
            assert_eq!(tool_events.len(), 2, "only synthetic start/end: {mode}");
            assert!(
                matches!(tool_events[0], AgentEvent::ActualToolExecutionStart { tool_call_id, tool_name, args, .. }
                if *tool_call_id == id && tool_name.equals_ascii("echo")
                    && args.same_identity(trace.arguments.lock().unwrap().as_ref().unwrap())),
                "{mode}"
            );
            let AgentEvent::ActualToolExecutionEnd { tool_call_id, tool_name, result, is_error } = tool_events[1]
            else {
                panic!("synthetic terminal receipt: {mode}")
            };
            assert_eq!(*tool_call_id, id, "{mode}");
            assert!(tool_name.equals_ascii("echo") && *is_error, "{mode}");
            let details = get(&values, result, "details");
            assert!(
                matches!(heap.snapshot(&get(&values, &details, "__synthetic")).unwrap(), ValueSnapshot::Boolean(true)),
                "{mode}"
            );
            assert!(
                matches!(heap.snapshot(&get(&values, &details, "executed")).unwrap(), ValueSnapshot::Boolean(false)),
                "{mode}"
            );
            let expected_source =
                if matches!(mode, "cancel" | "deadline") { "assistant_stop_aborted" } else { "assistant_stop_error" };
            assert!(
                matches!(heap.snapshot(&get(&values, &details, "source")).unwrap(), ValueSnapshot::String(source) if source.equals_ascii(expected_source)),
                "{mode}"
            );
            let (result_owner, result_root) = events.iter().find_map(|event| {
                let AgentEvent::MessageEnd { message } = event else { return None };
                let AgentMessageBody::Graph(root) = message.body_snapshot() else { return None };
                matches!(heap.snapshot(&get(&values, &root, "role")).unwrap(), ValueSnapshot::String(role) if role.equals_ascii("toolResult"))
                    .then_some((message, root))
            }).expect("same synthetic result published as a Graph message");
            assert!(get(&values, &result_root, "toolCallId").same_identity(&get(&values, &call, "id")), "{mode}");
            assert!(get(&values, &result_root, "details").same_identity(&details), "{mode}");
            assert!(get(&values, &result_root, "content").same_identity(&get(&values, result, "content")), "{mode}");
            assert!(current.iter().any(|message| message.same_record(result_owner)), "{mode}");
            assert!(report.messages.iter().any(|message| message.same_record(result_owner)), "{mode}");
            assert!(events.iter().any(|event| matches!(event, AgentEvent::ActualTurnEnd { message, tool_results, .. }
                if message.same_record(&owner) && tool_results.len() == 1 && tool_results[0].same_record(result_owner))), "{mode}");
        } else {
            assert!(tool_events.is_empty(), "{mode}");
        }
        if matches!(mode, "missing-hooks" | "missing-turn-end" | "final-gate" | "source-admission-error") {
            assert_eq!(port.streams.load(Ordering::SeqCst), 0);
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::ActualTurnEnd { preparation_error: Some(_), .. }))
            );
            if mode == "final-gate" {
                assert!(final_gate_visits.lock().unwrap().is_empty());
                assert_eq!(hooks.conversions.load(Ordering::SeqCst), 1);
                assert!(events.iter().any(|event| matches!(
                    event,
                    AgentEvent::ActualTurnEnd {
                        preparation_error: Some(error),
                        ..
                    } if matches!(error.as_ref(), ActualProviderError::Unsupported(
                        "actual final prepared-request gate"
                    ))
                )));
                assert!(!trace.order.lock().unwrap().contains(&"actual-stream"));
            }
            continue;
        }
        assert_eq!(hooks.conversions.load(Ordering::SeqCst), 1);
        assert_eq!(port.streams.load(Ordering::SeqCst), 1);
        assert_eq!(trace.tools_retained_at_end.load(Ordering::SeqCst), 1, "{mode}");
        assert_eq!(trace.source_retained_at_end.load(Ordering::SeqCst), usize::from(mode != "source-none"), "{mode}");
        if matches!(mode, "producer-error" | "producer-panic" | "eof" | "cancel" | "deadline") {
            assert!(!trace.order.lock().unwrap().contains(&"actual-assistant-transform"), "{mode}");
        }
        let owner = trace.owner.lock().unwrap().as_ref().unwrap().clone();
        assert!(current.iter().any(|message| message.same_record(&owner)));
        assert!(report.messages.iter().any(|message| message.same_record(&owner)));
        assert!(events.iter().any(|event| matches!(event, AgentEvent::MessageEnd { message } | AgentEvent::ActualMessageEnd { message, .. } if message.same_record(&owner))));
        if matches!(mode, "healthy" | "source-none" | "turn-end-error") {
            let order = trace.order.lock().unwrap();
            assert_eq!(&order[order.len() - 3..], &["turn_end", "actual-turn-end-hook", "agent_end"]);
            if mode == "turn-end-error" {
                assert!(
                    matches!(events.last().unwrap(), AgentEvent::ActualAgentEnd { failure, .. } if matches!(failure.as_ref(), ActualProviderError::Value(ValueError::Thrown(_))))
                );
            } else {
                let projected = events.last().unwrap().full().unwrap();
                assert_eq!(projected["type"], "agent_end");
            }
        }
        if matches!(mode, "tool" | "tool-unit" | "third-name" | "cancel" | "deadline") {
            let AgentMessageBody::Graph(root) = owner.body_snapshot() else { panic!("actual final owner") };
            let content = get(&values, &root, "content");
            let call = get(&values, &content, "0");
            assert!(get(&values, &call, "arguments").same_identity(trace.arguments.lock().unwrap().as_ref().unwrap()));
            if mode == "third-name" {
                assert!(matches!(heap.snapshot(&get(&values, &call, "name")).unwrap(), ValueSnapshot::Number(17.0)));
                assert_eq!(
                    trace.names.load(Ordering::SeqCst),
                    3,
                    "execution admission did not repeat/coerce H2 name Gets"
                );
            }
            if matches!(mode, "cancel" | "deadline") {
                assert_eq!(heap.array_length(&content, &NoRuntime).unwrap(), 1, "only completed calls retained");
                assert!(
                    matches!(heap.snapshot(&get(&values, &root, "stopReason")).unwrap(), ValueSnapshot::String(reason) if reason.equals_ascii("aborted"))
                );
                assert_eq!(
                    report.stop_cause,
                    Some(if mode == "cancel" { RunStopCause::ExternalAbort } else { RunStopCause::Deadline })
                );
            }
            if matches!(mode, "cancel" | "deadline") {
                assert!(
                    events
                        .iter()
                        .any(|event| matches!(event, AgentEvent::ActualTurnEnd { preparation_error: None, .. })),
                    "clean abort is not a preparation failure: {mode}"
                );
            } else if matches!(mode, "tool" | "tool-unit") {
                assert!(events.iter().any(|event| matches!(event, AgentEvent::ActualTurnEnd { preparation_error: Some(error), .. }
                    if matches!(error.as_ref(), ActualProviderError::Unsupported("full actual tool preparation hooks")))), "{mode}");
            } else {
                assert!(
                    events
                        .iter()
                        .any(|event| matches!(event, AgentEvent::ActualTurnEnd { preparation_error: Some(_), .. })),
                    "{mode}"
                );
            }
        }
        if mode == "transform-error" {
            assert!(events.iter().any(|event| matches!(event, AgentEvent::ActualTurnEnd { preparation_error: Some(error), .. } if matches!(error.as_ref(), ActualProviderError::Value(ValueError::Thrown(root)) if matches!(owner.body_snapshot(), AgentMessageBody::Graph(current) if current.same_identity(root))))));
        }
        if matches!(mode, "third-name" | "bad-final") {
            assert!(events.iter().any(
                |event| matches!(event, AgentEvent::ActualMessageEnd { message, .. } if message.same_record(&owner))
            ));
            assert!(unpaired_raw_tool_call_tail(std::slice::from_ref(&owner)).unwrap_err().0.contains("Graph tail"));
        }
    }
    // A Graph user tail reaches actual conversion, while an externally changed
    // adopted role is rejected before any provider/dispatch consumer.
    let heap = ValueHeap::new().unwrap();
    let root = heap.import_serde(&json!({"role":"user","content":"role receipt"})).unwrap();
    let input = AgentMessage::from_graph(root.clone(), None).unwrap();
    assert!(unpaired_raw_tool_call_tail(std::slice::from_ref(&input)).unwrap().is_none());
    heap.define_own_data(&root, "role".into(), heap.string("assistant".into()).unwrap(), &NoRuntime).unwrap();
    assert!(unpaired_raw_tool_call_tail(std::slice::from_ref(&input)).unwrap_err().0.contains("BoundaryChanged"));
}
