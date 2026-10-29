//! One coarse actual NativeRun group for scope await ordering, original throw
//! identity and cancellation. The native URI host never owns Graph handles.
use super::*;
use crate::{
    native_factory::{NativeFactoryImports, PreparedNativeSession},
    native_realm::NativeRealm,
    native_run::NativeRun,
};
use ara_agent::{ActualToolSource, AgentMessage};
use ara_rpc::value_heap::PropertyDescriptor;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};
use tokio::sync::Notify;

// Native fixture actions are consumed synchronously by the original getter.
// They add no evaluator, Session, worker, or production capability surface.
enum LateHostAction {
    Cancel(CancellationToken),
    Revoke(Arc<BuiltinToolSource>),
    PreviewSettlement {
        source: Arc<BuiltinToolSource>,
        queue: PendingResolutionQueue,
        mode: &'static str,
        cancel: CancellationToken,
    },
}
static NEXT_LATE_ACTION: AtomicU64 = AtomicU64::new(1);
static LATE_ACTIONS: std::sync::OnceLock<Mutex<std::collections::BTreeMap<u64, LateHostAction>>> =
    std::sync::OnceLock::new();
fn late_actions() -> &'static Mutex<std::collections::BTreeMap<u64, LateHostAction>> {
    LATE_ACTIONS.get_or_init(|| Mutex::new(std::collections::BTreeMap::new()))
}

struct ScopePort {
    path: PathBuf,
    started: Arc<Notify>,
    release: Arc<Notify>,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl ara_tools::ContentUriPort for ScopePort {
    fn route(&self, url: &str) -> ara_tools::ContentUriRoute {
        if matches!(url, "artifact://first" | "artifact://second") {
            ara_tools::ContentUriRoute::Registered { writable: false }
        } else {
            ara_tools::ContentUriRoute::Unregistered
        }
    }
    async fn search_path_only(&self, _: &str, cancel: CancellationToken) -> Result<Option<PathBuf>, ToolError> {
        let call=self.calls.fetch_add(1, Ordering::SeqCst);
        // G09 apply builds a fresh URI context. The controlled first preview
        // await remains observable; later apply resolution returns live data.
        if call>0 {return Ok(Some(self.path.clone()));}
        self.started.notify_one();
        tokio::select! { _ = self.release.notified() => Ok(Some(self.path.clone())), _ = cancel.cancelled() => Err(ToolError("Aborted: Cancelled".into())) }
    }
    async fn read(&self, _: &str, _: CancellationToken) -> Result<ara_tools::UriResource, ToolError> {
        panic!("scope must use pathOnly and never materialize content")
    }
    async fn write(&self, _: &str, _: &str, _: CancellationToken) -> Result<(), ToolError> {
        panic!("scope cannot write through a URI port")
    }
}
static TRACK_PUSH: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled original Array.push captures expanded scope array before Host await",
    name: "push",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 2,
    body: |heap, captures, receiver, arguments, effective| {
        if arguments.first().is_some_and(|argument| matches!(heap.snapshot(argument), Ok(ValueSnapshot::String(text)) if text.equals_ascii("artifact://first"))) {
            heap.define_own_data(&captures[0], "expanded".into(), receiver.clone(), effective)?;
        }
        if arguments.first().is_some_and(|argument| matches!(heap.snapshot(argument), Ok(ValueSnapshot::String(text)) if text.equals_ascii("after.ts")))
            && heap.truthy(&heap.get(&captures[0], &"bodyThrow".into(), effective)?)? {
            return Err(ValueError::Thrown(heap.get(&captures[0], &"bodyReason".into(), effective)?));
        }
        heap.call(&captures[1], receiver, arguments, effective)
    },
};
static LATE_PATH: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled original scope later getter is observed only after Host await",
    name: "get 1",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 4,
    body: |heap, captures, _, _, effective| {
        heap.define_own_data(&captures[0], "lateObserved".into(), heap.boolean(true)?, effective)?;
        let ValueSnapshot::Number(id) = heap.snapshot(&captures[3])? else {
            return Err(ValueError::WrongRuntimeReply("late fixture action ID"));
        };
        let action = late_actions().lock().unwrap().remove(&(id as u64));
        match action {
            Some(LateHostAction::Cancel(token)) => token.cancel(),
            Some(LateHostAction::Revoke(source)) => source
                .set_granted(BuiltinKind::AstEdit, false)
                .map_err(|_| ValueError::UnsupportedOperation("late fixture Host grant revocation failed"))?,
            Some(LateHostAction::PreviewSettlement { .. }) => {
                return Err(ValueError::WrongRuntimeReply("preview settlement action reached a path getter"));
            }
            None => {}
        }
        if heap.truthy(&captures[2])? { Err(ValueError::Thrown(captures[1].clone())) } else { Ok(captures[1].clone()) }
    },
};
static ITERATOR_WITH_CLOSE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled original array iterator retains an observable return for scope body errors",
    name: "values",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 2,
    body: |heap, captures, receiver, _, effective| {
        let iterator = heap.call(&captures[1], receiver, &[], effective)?;
        let expanded = heap.get(&captures[0], &"expanded".into(), effective)?;
        if receiver.same_identity(&expanded) {
            let close = heap.fresh_compiled_function(&CLOSE_ITERATOR, &[captures[0].clone()])?;
            heap.define_own_data(&iterator, "return".into(), close, effective)?;
        }
        Ok(iterator)
    },
};
static CLOSE_ITERATOR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled original iterator return confirms post-await throw/cancel closing",
    name: "return",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: |heap, captures, _, _, effective| {
        heap.define_own_data(&captures[0], "closed".into(), heap.boolean(true)?, effective)?;
        heap.object()
    },
};
fn request(
    prepared: &PreparedNativeSession,
    index: usize,
    name: &str,
    args: ValueHandle,
    cancel: CancellationToken,
) -> (Arc<dyn ActualToolInvoker>, ActualExecuteRequest) {
    let values = prepared.values();
    let heap = values.heap();
    let effective = values.runtime();
    let hooks = prepared.require_tool_hooks().unwrap();
    let source = hooks.source(values).unwrap();
    let collection = source.current_tools(values).unwrap().unwrap();
    let selected = heap.get(&collection, &index.to_string().as_str().into(), effective).unwrap();
    let binding = source.bind_selected(&selected, values).unwrap();
    let call =
        heap.import_serde(&serde_json::json!({"type":"toolCall","id":"ast-original-await","name":name})).unwrap();
    heap.define_own_data(&call, "arguments".into(), args.clone(), effective).unwrap();
    let mut assistant = serde_json::to_value(ara_ai::AssistantMessage::empty("fixture", "fixture", "fixture")).unwrap();
    assistant["role"] = "assistant".into();
    assistant["stopReason"] = "toolUse".into();
    let root = heap.import_serde(&assistant).unwrap();
    heap.define_own_data(&root, "content".into(), heap.array(vec![call.clone()]).unwrap(), effective).unwrap();
    let owner = AgentMessage::from_graph(root, None).unwrap();
    let tool_context = heap.object().unwrap();
    hooks.bind_run_context(values, &tool_context).unwrap();
    let callable = heap.get(&binding.source, &"execute".into(), effective).unwrap();
    let invoker = binding.capability.actual_invoker().unwrap();
    (
        invoker,
        ActualExecuteRequest {
            callable,
            receiver: binding.source,
            arguments_slot: owner.current_arguments_slot(&call).unwrap(),
            owner,
            tool_call: call,
            call_id: "ast-original-await".into(),
            args,
            values: values.clone(),
            cancel,
            on_update: Arc::new(|_| Ok(())),
            tool_context,
        },
    )
}
fn has_reason(error: &ActualProviderError, reason: &ValueHandle) -> bool {
    match error {
        ActualProviderError::Value(ValueError::Thrown(value)) => value.same_identity(reason),
        ActualProviderError::Compound { primary, secondary } => {
            has_reason(primary, reason) || has_reason(secondary, reason)
        }
        _ => false,
    }
}
static NORMALIZED_REWRITES: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "grouped original patched Object.fromEntries return identity",
    name: "fromEntries",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 2,
    body: |heap, captures, receiver, _, _| {
        if !receiver.same_identity(&captures[1]) {
            return Err(ValueError::WrongRuntimeReply("original Object receiver absent"));
        }
        heap.adopt(&captures[0])
    },
};
static LIVE_REWRITE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "grouped original normalized rewrites getter mutates next native call",
    name: "get rule",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, _, _, runtime| {
        let count = heap.get(&captures[0], &"reads".into(), runtime)?;
        let count = heap.to_number(&count, runtime)?;
        let output = heap.get(&captures[0], &"output".into(), runtime)?;
        if count == 0.0 {
            let second = heap.get(&captures[0], &"second".into(), runtime)?;
            heap.define_own_data(&captures[0], "output".into(), second, runtime)?;
        }
        heap.define_own_data(&captures[0], "reads".into(), heap.number(count + 1.0)?, runtime)?;
        Ok(output)
    },
};
static POST_STAGE_THEN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "grouped original returned-result adoption observes preview before final settlement",
    name: "get then",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: |heap, captures, receiver, _, runtime| {
        // Earlier empty Host receipts and internal promises do not trigger it.
        if heap.own_descriptor(receiver, &"details".into(), runtime)?.is_none() {
            return heap.undefined();
        }
        let details = heap.get(receiver, &"details".into(), runtime)?;
        if heap.own_descriptor(&details, &"totalReplacements".into(), runtime)?.is_none() {
            return heap.undefined();
        }
        let ValueSnapshot::Number(action_id) = heap.snapshot(&captures[3])? else {
            return Err(ValueError::WrongRuntimeReply("preview fixture action ID"));
        };
        let action = late_actions().lock().unwrap().remove(&(action_id as u64));
        let Some(LateHostAction::PreviewSettlement { source, queue, mode, cancel }) = action else {
            return heap.undefined();
        };
        let (id, kind) = queue.pending_head().expect("new preview must already be registered before result adoption");
        assert_eq!(kind, "ast_edit");
        let module =
            source.ast_module().map_err(|_| ValueError::WrongRuntimeReply("preview original module absent"))?;
        let bound = module.staged(&id)?.expect("registered preview has same-object binding before adoption");
        assert!(bound.same_identity(&captures[1]));
        heap.define_own_data(&captures[0], "publishedId".into(), heap.string(id.into())?, runtime)?;
        heap.define_own_data(&captures[0], "boundBeforeFailure".into(), heap.boolean(true)?, runtime)?;
        match mode {
            "cancel" => cancel.cancel(),
            "revoke" => source
                .set_granted(BuiltinKind::AstEdit, false)
                .map_err(|_| ValueError::WrongRuntimeReply("preview fixture revocation failed"))?,
            "throw" => return Err(ValueError::Thrown(captures[2].clone())),
            "waiter_drop" => return Ok(captures[2].clone()),
            _ => return Err(ValueError::WrongRuntimeReply("preview fixture mode")),
        }
        heap.undefined()
    },
};
// Notifications carry scalar fixture scheduling only. The actual thenable
// executes and saves original resolve/reject handles on the sole Graph driver.
static WAITING_THEN_NOTICES: std::sync::OnceLock<Mutex<std::collections::BTreeMap<u64, Arc<Notify>>>> =
    std::sync::OnceLock::new();
fn waiting_then_notices() -> &'static Mutex<std::collections::BTreeMap<u64, Arc<Notify>>> {
    WAITING_THEN_NOTICES.get_or_init(|| Mutex::new(std::collections::BTreeMap::new()))
}
static WAITING_RESULT_THEN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "grouped original unresolved result thenable keeps real waiter pending",
    name: "then",
    length: 2,
    form: CompiledFunctionForm::Builtin,
    capture_count: 2,
    body: |heap, captures, _, args, runtime| {
        assert_eq!(args.len(), 2);
        heap.define_own_data(&captures[0], "thenResolve".into(), args[0].clone(), runtime)?;
        heap.define_own_data(&captures[0], "thenReject".into(), args[1].clone(), runtime)?;
        heap.define_own_data(&captures[0], "thenCalled".into(), heap.boolean(true)?, runtime)?;
        let ValueSnapshot::Number(id) = heap.snapshot(&captures[1])? else {
            return Err(ValueError::WrongRuntimeReply("waiting then fixture ID"));
        };
        if let Some(notice) = waiting_then_notices().lock().unwrap().remove(&(id as u64)) {
            notice.notify_one();
        }
        // Returning normally without either callback leaves actual adoption pending.
        heap.undefined()
    },
};
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ast_edit_actual_native_scope_host_await_late_iteration_throw_cancel_group() {
    for scenario in ["success", "next_throw", "body_throw", "cancel", "late_cancel", "revoke"] {
        let directory = tempfile::tempdir().unwrap();
        for file in ["first.ts", "before.ts", "after.ts"] {
            std::fs::write(directory.path().join(file), "foo();\n").unwrap();
        }
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let port = Arc::new(ScopePort {
            path: directory.path().join("first.ts"),
            started: started.clone(),
            release: release.clone(),
            calls: calls.clone(),
        });
        let realm = NativeRealm::prepare().unwrap();
        let values = &realm.values;
        let heap = values.heap();
        let effective = values.runtime();
        let source = BuiltinToolSource::new(
            values,
            ToolContext::new(directory.path()).with_uri_port(port),
            &[BuiltinKind::AstEdit, BuiltinKind::Write],
        )
        .unwrap();
        let prepared = PreparedNativeSession::from_complete_bindings(
            realm.clone(),
            NativeFactoryImports {
                source: source.clone(),
                string_function: realm.globals.string.clone(),
                bun: realm.globals.bun.clone(),
                original_logger: None,
            },
        )
        .unwrap();
        let original_session = prepared.session_state().clone();
        let state = heap.object().unwrap();
        for key in ["lateObserved", "closed"] {
            heap.define_own_data(&state, key.into(), heap.boolean(false).unwrap(), effective).unwrap();
        }
        let reason = heap.object().unwrap();
        heap.define_own_data(&state, "bodyThrow".into(), heap.boolean(scenario == "body_throw").unwrap(), effective)
            .unwrap();
        heap.define_own_data(&state, "bodyReason".into(), reason.clone(), effective).unwrap();
        let array_proto = heap.array_prototype();
        let original_push = heap.get(&array_proto, &"push".into(), effective).unwrap();
        let tracked_push = heap.fresh_compiled_function(&TRACK_PUSH, &[state.clone(), original_push]).unwrap();
        heap.define_own_data(&array_proto, "push".into(), tracked_push, effective).unwrap();
        let original_iterator = heap.get(&array_proto, &heap.iterator_key(), effective).unwrap();
        let iterator = heap.fresh_compiled_function(&ITERATOR_WITH_CLOSE, &[state.clone(), original_iterator]).unwrap();
        heap.define_own_data(&array_proto, heap.iterator_key(), iterator, effective).unwrap();
        let args = heap
            .import_serde(
                &serde_json::json!({"ops":[{"pat":"foo()","out":"bar()"}],"paths":["artifact://first","before.ts"]}),
            )
            .unwrap();
        let cancel = CancellationToken::new();
        let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        let (invoker, input) = request(&prepared, 0, "ast_edit", args, cancel.clone());
        let mutate_while_host_waits = async {
            started.notified().await;
            assert!(!heap.truthy(&heap.get(&state, &"lateObserved".into(), effective).unwrap()).unwrap());
            let expanded = heap.get(&state, &"expanded".into(), effective).unwrap();
            let returned = if scenario == "next_throw" {
                reason.clone()
            } else {
                heap.string(
                    if matches!(scenario, "late_cancel" | "revoke") { "artifact://second" } else { "after.ts" }.into(),
                )
                .unwrap()
            };
            let action_id = if matches!(scenario, "late_cancel" | "revoke") {
                let id = NEXT_LATE_ACTION.fetch_add(1, Ordering::SeqCst);
                let action = if scenario == "late_cancel" {
                    LateHostAction::Cancel(cancel.clone())
                } else {
                    LateHostAction::Revoke(source.clone())
                };
                late_actions().lock().unwrap().insert(id, action);
                id
            } else {
                0
            };
            let getter = heap
                .fresh_compiled_function(
                    &LATE_PATH,
                    &[
                        state.clone(),
                        returned,
                        heap.boolean(scenario == "next_throw").unwrap(),
                        heap.number(action_id as f64).unwrap(),
                    ],
                )
                .unwrap();
            let ValueSnapshot::Function(getter) = heap.snapshot(&getter).unwrap() else { panic!("getter") };
            heap.define_own(
                &expanded,
                "1".into(),
                PropertyDescriptor::Accessor { get: Some(getter), set: None, enumerable: true, configurable: true },
                effective,
            )
            .unwrap();
            if scenario == "cancel" {
                cancel.cancel();
            } else {
                release.notify_one();
            }
        };
        let driven = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            driver.drive(async {
                let (result, ()) = tokio::join!(invoker.invoke_settled(input), mutate_while_host_waits);
                result
            }),
        )
        .await
        .unwrap();
        let observed = match driven {
            Ok(result) => result,
            Err(error) => Err(error),
        };
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(prepared.session_state().same_identity(&original_session));
        assert_eq!(std::fs::read_to_string(directory.path().join("before.ts")).unwrap(), "foo();\n");
        if scenario == "success" {
            let output = observed.unwrap();
            let details = heap.get(&output, &"details".into(), effective).unwrap();
            assert_eq!(heap.export_serde(&details).unwrap()["totalReplacements"], 2);
            assert!(heap.truthy(&heap.get(&state, &"lateObserved".into(), effective).unwrap()).unwrap());
            let cancel = CancellationToken::new();
            let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
            let args = heap
                .import_serde(&serde_json::json!({"path":"xd://resolve","content":"accept late backing-path source"}))
                .unwrap();
            let (invoker, input) = request(&prepared, 1, "write", args, cancel);
            tokio::time::timeout(std::time::Duration::from_secs(15), driver.drive(invoker.invoke_settled(input)))
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            for file in ["first.ts", "after.ts"] {
                assert_eq!(std::fs::read_to_string(directory.path().join(file)).unwrap(), "bar();\n");
            }
            // Fixed JS slice(0,120) can end between a surrogate pair. The
            // original Native model and display leaves retain that code unit,
            // rather than dropping it or inserting U+FFFD in a typed shortcut.
            let source = format!("foo(\"{}😀\");\n", "a".repeat(114));
            std::fs::write(directory.path().join("unicode.ts"), source).unwrap();
            let cancel = CancellationToken::new();
            let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
            let args = heap
                .import_serde(&serde_json::json!({"ops":[{"pat":"foo($A)","out":"bar($A)"}],"paths":["unicode.ts"]}))
                .unwrap();
            let (invoker, input) = request(&prepared, 0, "ast_edit", args, cancel);
            let output =
                tokio::time::timeout(std::time::Duration::from_secs(15), driver.drive(invoker.invoke_settled(input)))
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
            let content = heap.get(&output, &"content".into(), effective).unwrap();
            let first = heap.get(&content, &"0".into(), effective).unwrap();
            let model = heap.get(&first, &"text".into(), effective).unwrap();
            let details = heap.get(&output, &"details".into(), effective).unwrap();
            let display = heap.get(&details, &"displayContent".into(), effective).unwrap();
            for value in [model, display] {
                let ValueSnapshot::String(text) = heap.snapshot(&value).unwrap() else { panic!("original text leaf") };
                assert!(text.units().contains(&0xd83d));
                assert!(!text.units().contains(&0xde00));
                assert!(!text.units().contains(&0xfffd));
            }
        } else {
            let error = observed.unwrap_err();
            if matches!(scenario, "next_throw" | "body_throw") {
                assert!(has_reason(&error, &reason));
            }
            // Abrupt iterator.next marks done and does not call return. An
            // abrupt loop body or rejected Host await closes the open record.
            let closed = heap.truthy(&heap.get(&state, &"closed".into(), effective).unwrap()).unwrap();
            assert_eq!(closed, scenario != "next_throw", "{scenario}: {error:?}");
            if scenario == "revoke" {
                assert!(format!("{error:?}").contains("blocked by the current Host grant"), "{scenario}: {error:?}");
                assert!(heap.truthy(&heap.get(&state, &"lateObserved".into(), effective).unwrap()).unwrap());
            }
            for file in ["first.ts", "after.ts"] {
                assert_eq!(std::fs::read_to_string(directory.path().join(file)).unwrap(), "foo();\n");
            }
        }
    }
    // Complete the same coarse original Native group with the normalized
    // object spanning URI await, distinct target calls and a later write Run.
    // No ordinary-flow assertion above is changed or represented as full AST.
    {
        let directory = tempfile::tempdir().unwrap();
        for file in ["first.ts", "after.ts"] {
            std::fs::write(directory.path().join(file), "foo();\n").unwrap();
        }
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let port = Arc::new(ScopePort {
            path: directory.path().join("first.ts"),
            started: started.clone(),
            release: release.clone(),
            calls: calls.clone(),
        });
        let realm = NativeRealm::prepare().unwrap();
        let values = &realm.values;
        let heap = values.heap();
        let runtime = values.runtime();
        let source = BuiltinToolSource::new(
            values,
            ToolContext::new(directory.path()).with_uri_port(port),
            &[BuiltinKind::AstEdit, BuiltinKind::Write],
        )
        .unwrap();
        let prepared = PreparedNativeSession::from_complete_bindings(
            realm.clone(),
            NativeFactoryImports {
                source,
                string_function: realm.globals.string.clone(),
                bun: realm.globals.bun.clone(),
                original_logger: None,
            },
        )
        .unwrap();
        let session = prepared.session_state().clone();
        let state = heap.object().unwrap();
        for (key, value) in [
            ("reads", heap.number(0.0).unwrap()),
            ("output", heap.string("early()".into()).unwrap()),
            ("second", heap.string("second()".into()).unwrap()),
        ] {
            heap.define_own_data(&state, key.into(), value, runtime).unwrap();
        }
        let rewrites = heap.object().unwrap();
        let getter = heap.fresh_compiled_function(&LIVE_REWRITE, &[state.clone()]).unwrap();
        let ValueSnapshot::Function(getter) = heap.snapshot(&getter).unwrap() else { panic!() };
        heap.define_own(
            &rewrites,
            "foo()".into(),
            PropertyDescriptor::Accessor { get: Some(getter), set: None, enumerable: true, configurable: true },
            runtime,
        )
        .unwrap();
        let object = heap.get(&realm.ast.environment, &"Object".into(), runtime).unwrap();
        heap.define_own_data(
            &object,
            "fromEntries".into(),
            heap.fresh_compiled_function(&NORMALIZED_REWRITES, &[rewrites.clone(), object.clone()]).unwrap(),
            runtime,
        )
        .unwrap();
        let cancel = CancellationToken::new();
        let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        let args = heap
            .import_serde(
                &serde_json::json!({"ops":[{"pat":"foo()","out":"unused()"}],"paths":["artifact://first","after.ts"]}),
            )
            .unwrap();
        let (invoker, input) = request(&prepared, 0, "ast_edit", args, cancel);
        let mutate = async {
            started.notified().await;
            assert_eq!(heap.to_number(&heap.get(&state, &"reads".into(), runtime).unwrap(), runtime).unwrap(), 0.0);
            heap.define_own_data(&state, "output".into(), heap.string("late()".into()).unwrap(), runtime).unwrap();
            release.notify_one();
        };
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            driver.drive(async {
                let (result, ()) = tokio::join!(invoker.invoke_settled(input), mutate);
                result
            }),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(heap.to_number(&heap.get(&state, &"reads".into(), runtime).unwrap(), runtime).unwrap(), 2.0);
        assert!(prepared.session_state().same_identity(&session));
        let content = heap.get(&output, &"content".into(), runtime).unwrap();
        let first = heap.get(&content, &"0".into(), runtime).unwrap();
        let ValueSnapshot::String(model) = heap.snapshot(&heap.get(&first, &"text".into(), runtime).unwrap()).unwrap()
        else {
            panic!()
        };
        let model = model.to_utf8().unwrap();
        assert!(model.contains("late()"));
        assert!(model.contains("second()"));
        assert!(!model.contains("early()"));
        for file in ["first.ts", "after.ts"] {
            assert_eq!(std::fs::read_to_string(directory.path().join(file)).unwrap(), "foo();\n");
        }
        for (key, value) in [
            ("reads", heap.number(0.0).unwrap()),
            ("output", heap.string("applyLate()".into()).unwrap()),
            ("second", heap.string("applySecond()".into()).unwrap()),
        ] {
            heap.define_own_data(&state, key.into(), value, runtime).unwrap();
        }
        let cancel = CancellationToken::new();
        let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        let args = heap
            .import_serde(
                &serde_json::json!({"path":"xd://resolve","content":"accept original live normalized rewrite object"}),
            )
            .unwrap();
        let (invoker, input) = request(&prepared, 1, "write", args, cancel);
        tokio::time::timeout(std::time::Duration::from_secs(15), driver.drive(invoker.invoke_settled(input)))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(heap.to_number(&heap.get(&state, &"reads".into(), runtime).unwrap(), runtime).unwrap(), 2.0);
        assert!(prepared.session_state().same_identity(&session));
        assert_eq!(calls.load(Ordering::SeqCst), 2, "preview and current apply each resolve the URI");
        assert_eq!(std::fs::read_to_string(directory.path().join("first.ts")).unwrap(), "applyLate();\n");
        assert_eq!(std::fs::read_to_string(directory.path().join("after.ts")).unwrap(), "applySecond();\n");
    }

    // Keep an earlier accepted preview while a new one fails AFTER its local
    // registration/binding and during original Promise result adoption.
    for mode in ["cancel", "revoke", "throw"] {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("first.ts");
        std::fs::write(&file, "foo();\n").unwrap();
        let realm = NativeRealm::prepare().unwrap();
        let values = &realm.values;
        let heap = values.heap();
        let runtime = values.runtime();
        let ctx = ToolContext::new(directory.path());
        let queue = ctx.pending_resolution.clone();
        let source = BuiltinToolSource::new(values, ctx.clone(), &[BuiltinKind::AstEdit, BuiltinKind::Write]).unwrap();
        let prepared = PreparedNativeSession::from_complete_bindings(
            realm.clone(),
            NativeFactoryImports {
                source: source.clone(),
                string_function: realm.globals.string.clone(),
                bun: realm.globals.bun.clone(),
                original_logger: None,
            },
        )
        .unwrap();
        let cancel = CancellationToken::new();
        let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        let args = heap
            .import_serde(&serde_json::json!({"ops":[{"pat":"foo()","out":"older()"}],"paths":["first.ts"]}))
            .unwrap();
        let (invoker, input) = request(&prepared, 0, "ast_edit", args, cancel);
        tokio::time::timeout(std::time::Duration::from_secs(15), driver.drive(invoker.invoke_settled(input)))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let previous = queue.pending_head().unwrap();
        let previous_binding = realm.ast.staged(&previous.0).unwrap().unwrap();
        let selection = queue.native_ast_head().unwrap();
        // A worker completion late in cancellation has no queue publication.
        let receipt = Arc::new(Mutex::new(None));
        execute_finish(
            Arc::new(AstEditTool { ctx: ctx.clone() }),
            AstFinishStep::Preview {
                scope: selection.stage.scope.clone(),
                max_files: selection.stage.max_files,
                result: AstResult::default(),
            },
            receipt.clone(),
        )
        .await
        .unwrap();
        assert!(matches!(receipt.lock().unwrap().take(), Some(ScopeReceipt::UnpublishedPreview { .. })));
        assert_eq!(queue.pending_head(), Some(previous.clone()));
        // Exact guard compensation before/after binding covers output/heap
        // infrastructure faults, without pretending this is allocator acceptance.
        for bind in [false, true] {
            let result = ast_edit_engine::run(
                &selection.stage.scope,
                &[("foo()".into(), "temporary()".into())],
                true,
                selection.stage.max_files,
                CancellationToken::new(),
            )
            .unwrap();
            let output = AstEditTool { ctx: ctx.clone() }
                .finish_native_preview(selection.stage.scope.clone(), selection.stage.max_files, result)
                .unwrap();
            let id = output.pending_id.unwrap();
            {
                let _guard = AstPreviewPublication {
                    id: id.clone(),
                    queue: queue.clone(),
                    module: realm.ast.clone(),
                    accepted: false,
                };
                let failed: Result<(), ValueError> = (|| {
                    if bind {
                        realm.ast.stage(id.clone(), &previous_binding)?;
                    }
                    Err(ValueError::Allocation)
                })();
                assert!(failed.is_err());
            }
            assert_eq!(queue.pending_head(), Some(previous.clone()));
            assert!(realm.ast.staged(&id).unwrap().is_none());
            assert!(realm.ast.staged(&previous.0).unwrap().unwrap().same_identity(&previous_binding));
        }
        let state = heap.object().unwrap();
        heap.define_own_data(&state, "publishedId".into(), heap.string("".into()).unwrap(), runtime).unwrap();
        heap.define_own_data(&state, "boundBeforeFailure".into(), heap.boolean(false).unwrap(), runtime).unwrap();
        let rewrites = heap.object().unwrap();
        heap.define_own_data(&rewrites, "foo()".into(), heap.string("newer()".into()).unwrap(), runtime).unwrap();
        let object = heap.get(&realm.ast.environment, &"Object".into(), runtime).unwrap();
        heap.define_own_data(
            &object,
            "fromEntries".into(),
            heap.fresh_compiled_function(&NORMALIZED_REWRITES, &[rewrites.clone(), object.clone()]).unwrap(),
            runtime,
        )
        .unwrap();
        let reason = heap.object().unwrap();
        let cancel = CancellationToken::new();
        let action_id = NEXT_LATE_ACTION.fetch_add(1, Ordering::SeqCst);
        late_actions().lock().unwrap().insert(
            action_id,
            LateHostAction::PreviewSettlement {
                source: source.clone(),
                queue: queue.clone(),
                mode,
                cancel: cancel.clone(),
            },
        );
        let accessor = heap
            .fresh_compiled_function(
                &POST_STAGE_THEN,
                &[state.clone(), rewrites, reason.clone(), heap.number(action_id as f64).unwrap()],
            )
            .unwrap();
        let ValueSnapshot::Function(accessor) = heap.snapshot(&accessor).unwrap() else { panic!("then getter") };
        heap.define_own(
            &heap.object_prototype(),
            "then".into(),
            PropertyDescriptor::Accessor { get: Some(accessor), set: None, enumerable: false, configurable: true },
            runtime,
        )
        .unwrap();
        let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        let args = heap
            .import_serde(&serde_json::json!({"ops":[{"pat":"foo()","out":"unused()"}],"paths":["first.ts"]}))
            .unwrap();
        let (invoker, input) = request(&prepared, 0, "ast_edit", args, cancel);
        let driven =
            tokio::time::timeout(std::time::Duration::from_secs(15), driver.drive(invoker.invoke_settled(input)))
                .await
                .unwrap();
        let error = match driven {
            Ok(result) => result.unwrap_err(),
            Err(error) => error,
        };
        assert!(
            heap.truthy(&heap.get(&state, &"boundBeforeFailure".into(), runtime).unwrap()).unwrap(),
            "{mode}: {error:?}"
        );
        if mode == "throw" {
            assert!(has_reason(&error, &reason), "{error:?}");
        }
        let ValueSnapshot::String(id) =
            heap.snapshot(&heap.get(&state, &"publishedId".into(), runtime).unwrap()).unwrap()
        else {
            panic!("published ID")
        };
        let id = id.to_utf8().unwrap();
        assert_ne!(id, previous.0);
        assert_eq!(queue.pending_head(), Some(previous.clone()), "{mode}: {error:?}");
        assert!(realm.ast.staged(&id).unwrap().is_none());
        assert!(realm.ast.staged(&previous.0).unwrap().unwrap().same_identity(&previous_binding));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "foo();\n");
    }

    // Invocation-future Drop must compensate now while the same Run and a
    // genuine unresolved thenable retain the original runtime/parent reaction.
    // Stage-before Drop must also fence all subsequent original callbacks.
    for mode in ["pending_thenable", "before_stage"] {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("first.ts");
        std::fs::write(&file, "foo();\n").unwrap();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let uri_calls = Arc::new(AtomicUsize::new(0));
        let port = Arc::new(ScopePort {
            path: file.clone(),
            started: started.clone(),
            release: release.clone(),
            calls: uri_calls.clone(),
        });
        let realm = NativeRealm::prepare().unwrap();
        let values = &realm.values;
        let heap = values.heap();
        let runtime = values.runtime();
        let ctx = ToolContext::new(directory.path()).with_uri_port(port);
        let queue = ctx.pending_resolution.clone();
        let source = BuiltinToolSource::new(values, ctx, &[BuiltinKind::AstEdit, BuiltinKind::Write]).unwrap();
        let prepared = PreparedNativeSession::from_complete_bindings(
            realm.clone(),
            NativeFactoryImports {
                source: source.clone(),
                string_function: realm.globals.string.clone(),
                bun: realm.globals.bun.clone(),
                original_logger: None,
            },
        )
        .unwrap();
        let cancel = CancellationToken::new();
        let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        let args = heap
            .import_serde(&serde_json::json!({"ops":[{"pat":"foo()","out":"older()"}],"paths":["first.ts"]}))
            .unwrap();
        let (invoker, input) = request(&prepared, 0, "ast_edit", args, cancel);
        tokio::time::timeout(std::time::Duration::from_secs(15), driver.drive(invoker.invoke_settled(input)))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let previous = queue.pending_head().unwrap();
        let previous_binding = realm.ast.staged(&previous.0).unwrap().unwrap();
        let state = heap.object().unwrap();
        for (key, value) in [
            ("reads", heap.number(0.0).unwrap()),
            ("output", heap.string("newer()".into()).unwrap()),
            ("second", heap.string("newer()".into()).unwrap()),
            ("thenCalled", heap.boolean(false).unwrap()),
            ("boundBeforeFailure", heap.boolean(false).unwrap()),
            ("publishedId", heap.string("".into()).unwrap()),
        ] {
            heap.define_own_data(&state, key.into(), value, runtime).unwrap();
        }
        let rewrites = heap.object().unwrap();
        let accessor = heap.fresh_compiled_function(&LIVE_REWRITE, &[state.clone()]).unwrap();
        let ValueSnapshot::Function(accessor) = heap.snapshot(&accessor).unwrap() else {
            panic!("live rewrite getter")
        };
        heap.define_own(
            &rewrites,
            "foo()".into(),
            PropertyDescriptor::Accessor { get: Some(accessor), set: None, enumerable: true, configurable: true },
            runtime,
        )
        .unwrap();
        let object = heap.get(&realm.ast.environment, &"Object".into(), runtime).unwrap();
        heap.define_own_data(
            &object,
            "fromEntries".into(),
            heap.fresh_compiled_function(&NORMALIZED_REWRITES, &[rewrites.clone(), object.clone()]).unwrap(),
            runtime,
        )
        .unwrap();
        let cancel = CancellationToken::new();
        let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        let root_runtime = driver.root_runtime().unwrap();
        let original_run = source.bound_run().unwrap().unwrap();
        let then_started = Arc::new(Notify::new());
        if mode == "pending_thenable" {
            let id = NEXT_LATE_ACTION.fetch_add(1, Ordering::SeqCst);
            waiting_then_notices().lock().unwrap().insert(id, then_started.clone());
            let then = heap
                .fresh_compiled_function(&WAITING_RESULT_THEN, &[state.clone(), heap.number(id as f64).unwrap()])
                .unwrap();
            late_actions().lock().unwrap().insert(
                id,
                LateHostAction::PreviewSettlement {
                    source: source.clone(),
                    queue: queue.clone(),
                    mode: "waiter_drop",
                    cancel: cancel.clone(),
                },
            );
            let accessor = heap
                .fresh_compiled_function(
                    &POST_STAGE_THEN,
                    &[state.clone(), rewrites, then, heap.number(id as f64).unwrap()],
                )
                .unwrap();
            let ValueSnapshot::Function(accessor) = heap.snapshot(&accessor).unwrap() else {
                panic!("pending then getter")
            };
            heap.define_own(
                &heap.object_prototype(),
                "then".into(),
                PropertyDescriptor::Accessor { get: Some(accessor), set: None, enumerable: false, configurable: true },
                runtime,
            )
            .unwrap();
        }
        let path = if mode == "before_stage" { "artifact://first" } else { "first.ts" };
        let args =
            heap.import_serde(&serde_json::json!({"ops":[{"pat":"foo()","out":"unused()"}],"paths":[path]})).unwrap();
        let (invoker, input) = request(&prepared, 0, "ast_edit", args, cancel);
        let agent = async {
            let mut waiter = invoker.invoke_settled(input);
            if mode == "pending_thenable" {
                tokio::select! {
                    _=then_started.notified()=>{},
                    result=&mut waiter=>panic!("actual unresolved thenable unexpectedly settled: {result:?}"),
                }
                assert!(heap.truthy(&heap.get(&state, &"thenCalled".into(), runtime).unwrap()).unwrap());
                assert!(heap.truthy(&heap.get(&state, &"boundBeforeFailure".into(), runtime).unwrap()).unwrap());
                let ValueSnapshot::String(id) =
                    heap.snapshot(&heap.get(&state, &"publishedId".into(), runtime).unwrap()).unwrap()
                else {
                    panic!("pending ID")
                };
                let id = id.to_utf8().unwrap();
                assert_ne!(id, previous.0);
                // NativeRun is still active, and resolve/reject handles are retained.
                original_run.check_live().unwrap();
                drop(waiter);
                assert_eq!(queue.pending_head(), Some(previous.clone()));
                assert!(realm.ast.staged(&id).unwrap().is_none());
                assert!(realm.ast.staged(&previous.0).unwrap().unwrap().same_identity(&previous_binding));
                original_run.check_live().unwrap();
                let resolve = heap.get(&state, &"thenResolve".into(), runtime).unwrap();
                assert_eq!(heap.kind(&resolve).unwrap(), ara_rpc::value_heap::ValueKind::Function);
                heap.call(&resolve, &heap.undefined().unwrap(), &[heap.undefined().unwrap()], &root_runtime).unwrap();
                assert_eq!(queue.pending_head(), Some(previous.clone()));
            } else {
                tokio::select! {
                    _=started.notified()=>{},
                    result=&mut waiter=>panic!("URI await unexpectedly settled before waiter Drop: {result:?}"),
                }
                assert_eq!(heap.to_number(&heap.get(&state, &"reads".into(), runtime).unwrap(), runtime).unwrap(), 0.0);
                drop(waiter);
                assert_eq!(queue.pending_head(), Some(previous.clone()));
                release.notify_one();
                // Await only scalar native-job completion while the existing
                // production driver resumes every admitted original callback.
                loop {
                    let wake = original_run.wake();
                    let notice = wake.notified();
                    tokio::pin!(notice);
                    notice.as_mut().enable();
                    if original_run.outstanding() == 0 {
                        break;
                    }
                    notice.await;
                }
                assert_eq!(uri_calls.load(Ordering::SeqCst), 1);
                assert_eq!(heap.to_number(&heap.get(&state, &"reads".into(), runtime).unwrap(), runtime).unwrap(), 1.0);
                original_run.check_live().unwrap();
                assert_eq!(queue.pending_head(), Some(previous.clone()));
                assert!(realm.ast.staged(&previous.0).unwrap().unwrap().same_identity(&previous_binding));
            }
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "foo();\n");
        };
        tokio::time::timeout(std::time::Duration::from_secs(15), driver.drive(agent)).await.unwrap().unwrap();
        assert_eq!(queue.pending_head(), Some(previous.clone()));
        assert!(realm.ast.staged(&previous.0).unwrap().unwrap().same_identity(&previous_binding));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "foo();\n");
    }
}
