//! Coarse RPC Native loop observations called by the existing terminal entrance.
//! No new test entrypoint. Root executes the centrally joined delivered bytes.
use super::*;
use ara_agent::{ActualAgentContextView, ActualProviderError, ActualTurnEndContext, AgentMessageBody};
use ara_ai::{StopReason, graph_values::GraphValueFactory, tool_call_loop_guard::actual};
use ara_cli::{native_factory::PreparedNativeSession, native_realm::NativeRealm};
use ara_rpc::{
    WireString,
    value_heap::{
        CapabilityId, CompiledFunctionForm, CompiledFunctionSpec, NoRuntime, PropertyDescriptor, RuntimeReply,
        RuntimeRequest, ValueError, ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
    },
};
use std::sync::atomic::{AtomicUsize, Ordering};

fn get(values: &SessionValues, value: &ValueHandle, key: &str) -> ValueHandle {
    values.heap().get(value, &key.into(), values.runtime()).unwrap()
}
fn own(values: &SessionValues, value: &ValueHandle, key: &str, member: ValueHandle) {
    values.heap().define_own_data(value, key.into(), member, values.runtime()).unwrap();
}
fn graph(message: &AgentMessage) -> ValueHandle {
    let AgentMessageBody::Graph(root) = message.body_snapshot() else { panic!("current Graph expected") };
    root
}
fn text(values: &SessionValues, value: &ValueHandle) -> WireString {
    let ValueSnapshot::String(text) = values.heap().snapshot(value).unwrap() else {
        panic!("original String expected")
    };
    text
}
fn push(values: &SessionValues, log: &ValueHandle, label: &str) {
    let method = get(values, log, "push");
    values.heap().call(&method, log, &[values.heap().string(label.into()).unwrap()], values.runtime()).unwrap();
}
fn labels(values: &SessionValues, log: &ValueHandle) -> Vec<String> {
    let count = values.heap().array_length(log, values.runtime()).unwrap();
    (0..count).map(|index| text(values, &get(values, log, &index.to_string())).to_utf8().unwrap()).collect()
}
fn accessor(values: &SessionValues, value: &ValueHandle, key: &str, callback: ValueHandle) {
    let ValueSnapshot::Function(callback) = values.heap().snapshot(&callback).unwrap() else {
        panic!("Function expected")
    };
    values
        .heap()
        .define_own(
            value,
            key.into(),
            PropertyDescriptor::Accessor { get: Some(callback), set: None, enumerable: true, configurable: true },
            values.runtime(),
        )
        .unwrap();
}
static GET_LOGGED: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "coarse Native original getter logs and returns its same Graph edge",
    name: "",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 3,
    body: |heap, captures, _, _, runtime| {
        let push = heap.get(&captures[0], &"push".into(), runtime)?;
        heap.call(&push, &captures[0], &[captures[1].clone()], runtime)?;
        Ok(captures[2].clone())
    },
};
static LOGGER_WARN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "coarse original logger observes receiver/arguments and throws exact sentinel",
    name: "warn",
    length: 2,
    form: CompiledFunctionForm::Builtin,
    capture_count: 2,
    body: |heap, captures, receiver, args, runtime| {
        assert!(matches!(heap.snapshot(&heap.get(receiver, &"tag".into(), runtime)?)?,
            ValueSnapshot::String(value) if value.equals_ascii("original logger")));
        assert!(matches!(heap.snapshot(&args[0])?, ValueSnapshot::String(value)
            if value.equals_ascii("cross-turn tool-call loop detected")));
        assert!(matches!(heap.snapshot(&heap.get(&args[1], &"toolName".into(), runtime)?)?,
            ValueSnapshot::String(value) if value.equals_ascii("read")));
        assert!(matches!(heap.snapshot(&heap.get(&args[1], &"count".into(), runtime)?)?, ValueSnapshot::Number(2.0)));
        let push = heap.get(&captures[0], &"push".into(), runtime)?;
        heap.call(&push, &captures[0], &[heap.string("logger-call".into())?], runtime)?;
        if heap.truthy(&heap.get(&captures[1], &"fail".into(), runtime)?)? {
            return Err(ValueError::Thrown(heap.get(&captures[1], &"sentinel".into(), runtime)?));
        }
        heap.number(777.0)
    },
};

struct NativeFixture {
    owner: Arc<PreparedNativeSession>,
    log: ValueHandle,
    control: ValueHandle,
    logger: ValueHandle,
}
fn fixture_native() -> NativeFixture {
    let realm = NativeRealm::prepare().unwrap();
    let values = &realm.values;
    let heap = values.heap();
    let log = heap.array(Vec::new()).unwrap();
    let control = heap.object().unwrap();
    own(values, &control, "fail", heap.boolean(false).unwrap());
    own(values, &control, "sentinel", heap.object().unwrap());
    let logger = heap.object().unwrap();
    own(values, &logger, "tag", heap.string("original logger".into()).unwrap());
    let warn = heap.fresh_compiled_function(&LOGGER_WARN, &[log.clone(), control.clone()]).unwrap();
    let getter = heap
        .fresh_compiled_function(&GET_LOGGED, &[log.clone(), heap.string("logger-get".into()).unwrap(), warn])
        .unwrap();
    accessor(values, &logger, "warn", getter);
    let owner = PreparedNativeSession::from_realm(realm, Some(logger.clone())).unwrap();
    let render = get(owner.values(), owner.prompt().prompt(), "render");
    let getter = owner
        .values()
        .heap()
        .fresh_compiled_function(
            &GET_LOGGED,
            &[log.clone(), owner.values().heap().string("render-get".into()).unwrap(), render],
        )
        .unwrap();
    accessor(owner.values(), owner.prompt().prompt(), "render", getter);
    NativeFixture { owner, log, control, logger }
}
fn messages(values: &SessionValues) -> (AgentMessage, Vec<AgentMessage>) {
    let heap = values.heap();
    let assistant = heap
        .import_serde(&json!({"role":"assistant","content":[
        {"type":"toolCall","id":"id","name":"read","arguments":{"path":"before"}}],
        "stopReason":"toolUse","timestamp":1}))
        .unwrap();
    let result = heap
        .import_serde(&json!({"role":"toolResult","toolCallId":"id","toolName":"read",
        "isError":false,"content":[{"type":"text","text":"original result"}],"timestamp":2}))
        .unwrap();
    let block = get(values, &get(values, &result, "content"), "0");
    own(values, &block, "text", heap.string(WireString::from_units(vec![0x41, 0xd800, 0x42])).unwrap());
    (AgentMessage::from_graph(assistant, None).unwrap(), vec![AgentMessage::from_graph(result, None).unwrap()])
}
struct MissingActualBase;
impl LoopHooks for MissingActualBase {}

struct ActualBase {
    inputs: Vec<AgentMessage>,
    log: ValueHandle,
    stop: Option<&'static str>,
}
#[async_trait]
impl LoopHooks for ActualBase {
    fn supports_actual_turn_end(&self) -> bool {
        true
    }
    async fn turn_end_actual(
        &self,
        message: &AgentMessage,
        _: &[AgentMessage],
        _: &CancellationToken,
        values: &SessionValues,
    ) -> Result<Vec<AgentMessage>, ActualProviderError> {
        push(values, &self.log, "base");
        let root = graph(message);
        let call = get(values, &get(values, &root, "content"), "0");
        let arguments = get(values, &call, "arguments");
        own(values, &arguments, "path", values.heap().string("after".into()).unwrap());
        if let Some(stop) = self.stop {
            own(values, &root, "stopReason", values.heap().string(stop.into()).unwrap());
        }
        Ok(self.inputs.clone())
    }
}
fn make_hooks(fixture: &NativeFixture, state: Arc<Mutex<ToolLoopState>>, base_input: AgentMessage) -> ToolLoopHooks {
    let (host, _) = super::tests::fixture();
    ToolLoopHooks {
        base: Arc::new(ActualBase { inputs: vec![base_input], log: fixture.log.clone(), stop: None }),
        native_session: Some(fixture.owner.clone()),
        state,
        settings: ToolLoopGuardSettings { enabled: true, threshold: 2.0, exempt_tools: vec!["hub".into()] },
        output: host.output.clone(),
        session_id: json!("native-loop-coarse"),
        run_cancel: CancellationToken::new(),
    }
}
async fn context_call(
    hooks: &ToolLoopHooks,
    assistant: &AgentMessage,
    results: &[AgentMessage],
    values: &SessionValues,
    reason: Option<StopReason>,
) -> Result<Vec<AgentMessage>, ActualProviderError> {
    let all = Vec::new();
    let prompt = Vec::new();
    let cancel = CancellationToken::new();
    hooks
        .turn_end_actual_context(
            ActualTurnEndContext {
                assistant_message: assistant,
                tool_results: results,
                context: ActualAgentContextView { system_prompt: &prompt, messages: &all, tool_source: None },
                will_continue: true,
                stop_reason: reason,
                stop_cause: None,
                values,
            },
            &cancel,
        )
        .await
}

pub(super) async fn observe_actual_loop_hooks() {
    let fixture = fixture_native();
    let values = fixture.owner.values();
    let (assistant, results) = messages(values);
    let original_arguments = get(values, &get(values, &get(values, &graph(&assistant), "content"), "0"), "arguments");
    let base_input = AgentMessage::from_graph(
        values.heap().import_serde(&json!({"role":"user","content":"base","timestamp":0})).unwrap(),
        None,
    )
    .unwrap();
    let mut hooks = make_hooks(&fixture, Arc::new(Mutex::new(ToolLoopState::default())), base_input.clone());
    assert!(hooks.supports_actual_turn_end());
    let first = context_call(&hooks, &assistant, &results, values, Some(StopReason::ToolUse)).await.unwrap();
    assert_eq!(first.len(), 1);
    assert!(first[0].same_record(&base_input));
    let second = hooks.turn_end_actual(&assistant, &results, &CancellationToken::new(), values).await.unwrap();
    assert_eq!(second.len(), 2);
    assert!(second[0].same_record(&base_input));
    let notice = graph(&second[1]);
    assert!(text(values, &get(values, &notice, "content")).units().contains(&0xd800));
    let details = get(values, &notice, "details");
    assert_eq!(text(values, &get(values, &details, "argumentsSummary")).to_utf8().unwrap(), r#"{"path":"after"}"#);
    let current_arguments = get(values, &get(values, &get(values, &graph(&assistant), "content"), "0"), "arguments");
    assert!(current_arguments.same_identity(&original_arguments));
    assert_eq!(labels(values, &fixture.log), vec!["base", "base", "logger-get", "logger-call", "render-get"]);
    assert_eq!(hooks.turn_end_actual(&assistant, &results, &CancellationToken::new(), values).await.unwrap().len(), 1);
    // Observed context stop is preserved; the non-context form reads the body
    // AFTER the base has changed it, and suppresses the errored turn.
    hooks.state = Arc::new(Mutex::new(ToolLoopState::default()));
    hooks.base =
        Arc::new(ActualBase { inputs: vec![base_input.clone()], log: fixture.log.clone(), stop: Some("error") });
    assert_eq!(hooks.turn_end_actual(&assistant, &results, &CancellationToken::new(), values).await.unwrap().len(), 1);
    assert_eq!(context_call(&hooks, &assistant, &results, values, Some(StopReason::Error)).await.unwrap().len(), 1);
    own(values, &graph(&assistant), "stopReason", values.heap().string("toolUse".into()).unwrap());
    hooks.base = Arc::new(ActualBase { inputs: vec![base_input.clone()], log: fixture.log.clone(), stop: None });
    hooks.native_session = None;
    assert!(!hooks.supports_actual_turn_end());
    assert!(matches!(
        hooks.turn_end_actual(&assistant, &results, &CancellationToken::new(), values).await,
        Err(ActualProviderError::Unsupported("RPC Native Session prompt/logger owner"))
    ));
    let bare = PreparedNativeSession::from_realm(NativeRealm::prepare().unwrap(), None).unwrap();
    let (bare_assistant, bare_results) = messages(bare.values());
    hooks.native_session = Some(bare.clone());
    hooks.base = Arc::new(ActualBase { inputs: vec![], log: bare.values().heap().array(vec![]).unwrap(), stop: None });
    assert!(!hooks.supports_actual_turn_end());
    assert!(matches!(
        hooks.turn_end_actual(&bare_assistant, &bare_results, &CancellationToken::new(), bare.values()).await,
        Err(ActualProviderError::Unsupported("RPC original Native logger binding"))
    ));
    hooks.settings.enabled = false;
    assert!(hooks.supports_actual_turn_end());
    assert!(
        hooks
            .turn_end_actual(&bare_assistant, &bare_results, &CancellationToken::new(), bare.values())
            .await
            .unwrap()
            .is_empty()
    );
    hooks.settings.enabled = true;
    hooks.native_session = Some(fixture.owner.clone());
    assert!(matches!(
        hooks.turn_end_actual(&bare_assistant, &bare_results, &CancellationToken::new(), bare.values()).await,
        Err(ActualProviderError::Protocol("RPC Native loop owner/current values factory mismatch"))
    ));
    hooks.base = Arc::new(MissingActualBase);
    assert!(!hooks.supports_actual_turn_end());
    assert!(matches!(
        hooks.turn_end_actual(&bare_assistant, &bare_results, &CancellationToken::new(), bare.values()).await,
        Err(ActualProviderError::Unsupported("actual turn-end consumer"))
    ));
    // logger Throw retains exact same-heap exception and committed count;
    // render is never read, and the next repeated batch does not hit again.
    let thrown = fixture_native();
    let thrown_values = thrown.owner.values();
    let (message, tool_results) = messages(thrown_values);
    let base = AgentMessage::from_graph(
        thrown_values.heap().import_serde(&json!({"role":"user","content":"base"})).unwrap(),
        None,
    )
    .unwrap();
    let throwing_hooks = make_hooks(&thrown, Arc::new(Mutex::new(ToolLoopState::default())), base);
    own(thrown_values, &thrown.control, "fail", thrown_values.heap().boolean(true).unwrap());
    let sentinel = get(thrown_values, &thrown.control, "sentinel");
    assert_eq!(
        throwing_hooks
            .turn_end_actual(&message, &tool_results, &CancellationToken::new(), thrown_values)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        matches!(context_call(&throwing_hooks,&message,&tool_results,thrown_values,Some(StopReason::ToolUse)).await,
        Err(ActualProviderError::Value(ValueError::Thrown(original))) if original.same_identity(&sentinel))
    );
    assert_eq!(labels(thrown_values, &thrown.log), vec!["base", "base", "logger-get", "logger-call"]);
    assert_eq!(
        throwing_hooks
            .turn_end_actual(&message, &tool_results, &CancellationToken::new(), thrown_values)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(thrown.owner.logger().unwrap().same_identity(&thrown.logger));
    observe_state_rebind();
}

struct ReentryRuntime {
    state: Weak<Mutex<ToolLoopState>>,
    mode: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
    sentinel: ValueHandle,
    add: ValueHandle,
}
impl ValueRuntime for ReentryRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        match request {
            RuntimeRequest::Call { capability: CapabilityId(28001), receiver, arguments } => {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let state = self.state.upgrade().unwrap();
                let mut guard = state.try_lock().expect("Native Set callback runs outside Host mutex");
                let mode = self.mode.load(Ordering::SeqCst);
                if mode == 2 {
                    guard.configure(ToolLoopGuardSettings { enabled: false, ..Default::default() });
                }
                drop(guard);
                if mode == 1 {
                    return Err(ValueError::Thrown(self.sentinel.clone()));
                }
                Ok(RuntimeReply::Value(receiver.heap().call(&self.add, receiver, arguments, self)?))
            }
            request => NoRuntime.resolve(request),
        }
    }
}
fn turn(factory: &GraphValueFactory) -> ValueHandle {
    factory
        .heap()
        .import_serde(&json!({"message":{"role":"assistant","content":[
        {"type":"toolCall","id":"id","name":"read","arguments":{"path":"after"}}]},"toolResults":[]}))
        .unwrap()
}
fn observe_state_rebind() {
    let settings = ToolLoopGuardSettings { enabled: true, threshold: 2.0, exempt_tools: vec!["hub".into()] };
    let state = Arc::new(Mutex::new(ToolLoopState::default()));
    let a = Arc::new(GraphValueFactory::new(ValueHeap::new().unwrap(), Arc::new(NoRuntime)).unwrap());
    let lease = state.lock().unwrap().begin_actual(settings.clone(), &a).unwrap();
    let set_a = actual::create_exempt_set(&a, &lease.exempt_tools).unwrap();
    let lease = state.lock().unwrap().install_actual_exempt_set(&lease, set_a.clone()).unwrap();
    let prepared = actual::prepare_turn(&a, &turn(&a), lease.exempt_set.as_ref().unwrap()).unwrap();
    assert_eq!(state.lock().unwrap().commit_actual_turn(&lease, &prepared).unwrap(), None);
    let heap = ValueHeap::new().unwrap();
    let mode = Arc::new(AtomicUsize::new(0));
    let add = heap.get(&heap.set_intrinsic_prototype(), &"add".into(), &NoRuntime).unwrap();
    let sentinel = heap.object().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = Arc::new(ReentryRuntime {
        state: Arc::downgrade(&state),
        mode: mode.clone(),
        calls: calls.clone(),
        sentinel: sentinel.clone(),
        add: add.clone(),
    });
    let b = Arc::new(GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap());
    heap.define_own_data(
        &heap.set_intrinsic_prototype(),
        "add".into(),
        heap.function(CapabilityId(28001)).unwrap(),
        runtime.as_ref(),
    )
    .unwrap();
    let pending = state.lock().unwrap().begin_actual(settings.clone(), &b).unwrap();
    assert!(pending.exempt_set.is_none());
    assert!(
        matches!(state.lock().unwrap().commit_actual_turn(&pending, &prepared), Err(ValueError::WrongRuntimeReply(_))),
        "foreign pending lease cannot commit using the installed old-realm Set"
    );
    mode.store(1, Ordering::SeqCst);
    assert!(
        matches!(actual::create_exempt_set(&b,&pending.exempt_tools),Err(ValueError::Thrown(original)) if original.same_identity(&sentinel))
    );
    let retained = state.lock().unwrap().begin_actual(settings.clone(), &a).unwrap();
    assert!(retained.exempt_set.as_ref().unwrap().same_identity(&set_a));
    mode.store(0, Ordering::SeqCst);
    let replacement = actual::create_exempt_set(&b, &pending.exempt_tools).unwrap();
    let installed = state.lock().unwrap().install_actual_exempt_set(&pending, replacement).unwrap();
    let prepared = actual::prepare_turn(&b, &turn(&b), installed.exempt_set.as_ref().unwrap()).unwrap();
    assert_eq!(
        state.lock().unwrap().commit_actual_turn(&installed, &prepared).unwrap(),
        Some(2.0),
        "same-key realm swap preserves hash/count"
    );
    let returning = state.lock().unwrap().begin_actual(settings.clone(), &a).unwrap();
    assert!(returning.exempt_set.is_none());
    let replacement = actual::create_exempt_set(&a, &returning.exempt_tools).unwrap();
    let returning = state.lock().unwrap().install_actual_exempt_set(&returning, replacement).unwrap();
    let prepared = actual::prepare_turn(&a, &turn(&a), returning.exempt_set.as_ref().unwrap()).unwrap();
    assert_eq!(
        state.lock().unwrap().commit_actual_turn(&returning, &prepared).unwrap(),
        None,
        "third repeated turn after switch-back is not a new detector"
    );
    let changed = ToolLoopGuardSettings { threshold: 1.0, ..settings.clone() };
    let changed = state.lock().unwrap().begin_actual(changed, &a).unwrap();
    let replacement = actual::create_exempt_set(&a, &changed.exempt_tools).unwrap();
    let changed = state.lock().unwrap().install_actual_exempt_set(&changed, replacement).unwrap();
    let prepared = actual::prepare_turn(&a, &turn(&a), changed.exempt_set.as_ref().unwrap()).unwrap();
    assert_eq!(
        state.lock().unwrap().commit_actual_turn(&changed, &prepared).unwrap(),
        Some(1.0),
        "a real settings-key change rebuilds the detector"
    );
    // No callbacks are repeated after a reentrant disable invalidates the
    // pending first Set, even while the installed state is still empty.
    state.lock().unwrap().configure(ToolLoopGuardSettings { enabled: false, ..Default::default() });
    let pending = state.lock().unwrap().begin_actual(settings, &b).unwrap();
    mode.store(2, Ordering::SeqCst);
    let before_calls = calls.load(Ordering::SeqCst);
    let set = actual::create_exempt_set(&b, &pending.exempt_tools).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), before_calls + 1);
    assert!(matches!(
        state.lock().unwrap().install_actual_exempt_set(&pending, set),
        Err(ValueError::WrongRuntimeReply(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), before_calls + 1, "stale receipt refuses without callback replay");
}
