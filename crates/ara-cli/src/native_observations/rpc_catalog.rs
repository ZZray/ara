//! Mixed catalog/RPC observations inside the existing CLI coarse entrance.
//! The controlled Host uses the production ToolBridge and the sole NativeRun.
use crate::rpc_host_tools::{ToolBridge, normalize_host_tool_definitions};
use ara_agent::{ActualExecuteRequest, ActualToolSource, AgentMessage, SessionValues};
use ara_cli::{
    actual_tools::{BuiltinKind, BuiltinToolSource},
    native_factory::{
        NativeCatalogFactoryImports, PreparedNativeSession,
        catalog::{
            NativeCatalogMember, NativeCatalogOwner, NativeCatalogSnapshot, NativeCatalogSources, NativeToolCatalog,
        },
    },
    native_realm::NativeRealm,
    native_rpc_host::{NativeRpcHostOwner, NativeRpcHostPreparation, NativeRpcHostRawUpdate, NativeRpcHostSource},
    native_run::NativeRun,
};
use ara_rpc::{
    WireString, WireValue,
    value_heap::{ValueError, ValueHandle},
};
use ara_tools::ToolContext;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio_util::sync::CancellationToken;

fn wire(text: &str) -> WireValue {
    WireValue::parse(text).unwrap()
}
fn observe_wire(values: &SessionValues, value: &ValueHandle) -> WireValue {
    let encoded = values.heap().stringify_native(value, None, None, values.runtime()).unwrap().unwrap();
    WireValue::parse_units(encoded.units()).unwrap()
}
fn result_frame(id: WireValue, result: WireValue, is_error: bool) -> WireValue {
    WireValue::object(vec![
        ("type", WireValue::String("host_tool_result".into())),
        ("id", id),
        ("result", result),
        ("isError", WireValue::Bool(is_error)),
    ])
}
fn snapshot(
    values: &Arc<SessionValues>,
    builtin: &Arc<BuiltinToolSource>,
    rpc: &Arc<NativeRpcHostSource>,
) -> Arc<NativeCatalogSnapshot> {
    let mut registry = Vec::new();
    let mut members = Vec::new();
    for (owner, source) in [
        (NativeCatalogOwner::Builtin(builtin.clone()), builtin.clone() as Arc<dyn ActualToolSource>),
        (NativeCatalogOwner::Rpc(rpc.clone()), rpc.clone() as Arc<dyn ActualToolSource>),
    ] {
        let collection = source.current_tools(values).unwrap().unwrap();
        for index in 0..values.heap().array_length(&collection, values.runtime()).unwrap() {
            let selected =
                values.heap().get(&collection, &index.to_string().as_str().into(), values.runtime()).unwrap();
            registry.push(NativeCatalogMember::bind(owner.clone(), &selected, values).unwrap());
            members.push(selected);
        }
    }
    NativeCatalogSnapshot::new(
        values,
        values.heap().array(members).unwrap(),
        registry,
        NativeCatalogSources { builtin: builtin.clone(), mcp: None, rpc: Some(rpc.clone()) },
    )
    .unwrap()
}
fn request(
    prepared: &PreparedNativeSession,
    args: ValueHandle,
    cancel: CancellationToken,
    update: ara_agent::ActualUpdateFn,
) -> (Arc<dyn ara_agent::ActualToolInvoker>, ActualExecuteRequest) {
    let values = prepared.values();
    let heap = values.heap();
    let hooks = prepared.require_tool_hooks().unwrap();
    let source = hooks.source(values).unwrap();
    let collection = source.current_tools(values).unwrap().unwrap();
    let selected = heap.get(&collection, &"1".into(), values.runtime()).unwrap();
    let binding = source.bind_selected(&selected, values).unwrap();
    let name = heap.get(&selected, &"name".into(), values.runtime()).unwrap();
    let call = heap.import_strict(wire(r#"{"type":"toolCall","id":"catalog-rpc"}"#)).unwrap();
    heap.define_own_data(&call, "name".into(), name, values.runtime()).unwrap();
    heap.define_own_data(&call, "arguments".into(), args.clone(), values.runtime()).unwrap();
    let root = heap
        .import_serde(&serde_json::to_value(ara_ai::AssistantMessage::empty("fixture", "fixture", "fixture")).unwrap())
        .unwrap();
    heap.define_own_data(&root, "role".into(), heap.string("assistant".into()).unwrap(), values.runtime()).unwrap();
    heap.define_own_data(&root, "content".into(), heap.array(vec![call.clone()]).unwrap(), values.runtime()).unwrap();
    heap.define_own_data(&root, "stopReason".into(), heap.string("toolUse".into()).unwrap(), values.runtime()).unwrap();
    let owner = AgentMessage::from_graph(root, None).unwrap();
    let tool_context = heap.object().unwrap();
    hooks.bind_run_context(values, &tool_context).unwrap();
    let request = ActualExecuteRequest {
        callable: heap.get(&binding.source, &"execute".into(), values.runtime()).unwrap(),
        receiver: binding.source,
        arguments_slot: owner.current_arguments_slot(&call).unwrap(),
        owner,
        tool_call: call,
        call_id: "catalog-rpc".into(),
        args,
        values: values.clone(),
        cancel,
        on_update: update,
        tool_context,
    };
    (binding.capability.actual_invoker().unwrap(), request)
}
fn definitions(name: &str) -> Vec<crate::rpc_host_tools::HostToolDefinition> {
    normalize_host_tool_definitions(&WireValue::Array(vec![WireValue::object(vec![
        ("name", WireValue::String(name.into())),
        ("description", WireValue::String("controlled actual graph Host".into())),
        ("parameters", wire(r#"{"type":"object"}"#)),
    ])]))
    .unwrap()
}
fn setup(
    realm: &Arc<NativeRealm>,
    builtin: &Arc<BuiltinToolSource>,
    rpc: &Arc<NativeRpcHostSource>,
) -> Arc<PreparedNativeSession> {
    let draft = PreparedNativeSession::from_realm(realm.clone(), None).unwrap();
    let state = draft.session_state().clone();
    let catalog = NativeToolCatalog::new(&realm.values, snapshot(&realm.values, builtin, rpc)).unwrap();
    let prepared = draft
        .finish_catalog_bindings(NativeCatalogFactoryImports {
            catalog,
            string_function: realm.globals.string.clone(),
            bun: realm.globals.bun.clone(),
            original_logger: None,
        })
        .unwrap();
    assert!(prepared.session_state().same_identity(&state));
    assert!(Arc::ptr_eq(prepared.values(), &realm.values));
    prepared
}

/// Control runs at the existing encoder seam, after original graph stringify.
/// The underlying binary still owns the pending ID, wire reply and publication.
struct ReentrantOwner {
    inner: Arc<dyn NativeRpcHostOwner>,
    bridge: Arc<ToolBridge>,
    source: Arc<OnceLock<Weak<NativeRpcHostSource>>>,
    reply: bool,
}
impl NativeRpcHostOwner for ReentrantOwner {
    fn definition(&self) -> ara_ai::Tool {
        self.inner.definition()
    }
    fn graph_definition(&self) -> WireValue {
        self.inner.graph_definition()
    }
    fn prepare_native(
        &self,
        call_id: &WireString,
        cancel: CancellationToken,
        update: NativeRpcHostRawUpdate,
        encode: &mut dyn FnMut(&WireString) -> Result<WireValue, ValueError>,
    ) -> Result<NativeRpcHostPreparation, ValueError> {
        self.inner.prepare_native(call_id, cancel.clone(), update, &mut |id| {
            let frame = encode(id)?;
            if self.reply {
                assert!(self.bridge.consume(&result_frame(
                    WireValue::String(id.clone()),
                    wire(r#"{"content":[{"type":"text","text":"known host failure"}]}"#),
                    true
                )));
                cancel.cancel();
            }
            self.source.get().and_then(Weak::upgrade).unwrap().set_enabled(0, false).unwrap();
            Ok(frame)
        })
    }
}

pub async fn observe() {
    let dir = tempfile::Builder::new().prefix("ara-native-rpc-catalog-").tempdir().unwrap().keep();
    println!("native mixed catalog/RPC retained artifacts: {}", dir.display());
    let realm = NativeRealm::prepare().unwrap();
    let values = &realm.values;
    let heap = values.heap();
    let builtin = BuiltinToolSource::new(values, ToolContext::new(dir.clone()), &[BuiltinKind::Read]).unwrap();
    let frames = Arc::new(Mutex::new(Vec::new()));
    let bridge_slot: Arc<OnceLock<Weak<ToolBridge>>> = Arc::new(OnceLock::new());
    let slot = bridge_slot.clone();
    let recorded = frames.clone();
    let partial = wire(
        r#"{"content":[{"type":"text","text":"\ud800","textSignature":"partial"}],"details":{"__proto__":{"kept":true}}}"#,
    );
    let terminal = wire(
        r#"{"content":[{"type":"text","text":"\udfff","textSignature":"terminal"},{"type":"image","data":"AA==","mimeType":"image/png"}],"providerMetadata":{"opaque":"\ud800"},"details":{"proof":true}}"#,
    );
    let partial_reply = partial.clone();
    let terminal_reply = terminal.clone();
    let bridge = ToolBridge::new(Arc::new(move |frame| {
        recorded.lock().unwrap().push(frame.clone());
        let id = frame.get("id").unwrap().clone();
        let bridge = slot.get().and_then(Weak::upgrade).unwrap();
        assert!(bridge.consume(&WireValue::object(vec![
            ("type", WireValue::String("host_tool_update".into())),
            ("id", id.clone()),
            ("partialResult", partial_reply.clone()),
        ])));
        assert!(bridge.consume(&result_frame(id, terminal_reply.clone(), false)));
    }));
    bridge_slot.set(Arc::downgrade(&bridge)).unwrap();
    let rpc = NativeRpcHostSource::new(
        values,
        bridge.native_owners(&definitions("host_a")),
        realm.globals.environment.clone(),
        realm.globals.errors.clone(),
    )
    .unwrap();
    let prepared = setup(&realm, &builtin, &rpc);
    let original_state = prepared.session_state().clone();
    let driver = NativeRun::begin(prepared.clone(), CancellationToken::new()).unwrap();
    let lease = driver.lease();
    let updates = Arc::new(Mutex::new(Vec::new()));
    let captured = updates.clone();
    let update_values = values.clone();
    let update: ara_agent::ActualUpdateFn = Arc::new(move |value| {
        captured.lock().unwrap().push(observe_wire(&update_values, &value));
        Ok(())
    });
    let args = heap.import_strict(wire(r#"{"text":"\ud800","__proto__":{"kept":true}}"#)).unwrap();
    let (first, first_request) = request(&prepared, args.clone(), CancellationToken::new(), update.clone());
    let refreshed = NativeRpcHostSource::new(
        values,
        bridge.native_owners(&definitions("host_b")),
        realm.globals.environment.clone(),
        realm.globals.errors.clone(),
    )
    .unwrap();
    let next = snapshot(values, &builtin, &refreshed);
    let future = async {
        let result = first.invoke_settled(first_request).await.unwrap();
        assert_eq!(observe_wire(values, &result), terminal);
        rpc.set_enabled(0, false).unwrap();
        prepared.publish_catalog_snapshot(next, Some(&lease)).unwrap();
        let (second, second_request) = request(&prepared, args, CancellationToken::new(), update);
        let result = second.invoke_settled(second_request).await.unwrap();
        assert_eq!(observe_wire(values, &result), terminal);
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), driver.drive(future)).await.unwrap().unwrap();
    assert_eq!(updates.lock().unwrap().as_slice(), &[partial.clone(), partial]);
    let calls = frames.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].get("toolName"), Some(&WireValue::String("host_a".into())));
    assert_eq!(calls[1].get("toolName"), Some(&WireValue::String("host_b".into())));
    assert_eq!(
        calls[0].get("arguments").unwrap().get("text").and_then(WireValue::as_string).unwrap().units(),
        &[0xd800]
    );
    std::fs::write(dir.join("normal-wire.json"), WireValue::Array(calls.clone()).stringify()).unwrap();
    std::fs::write(
        dir.join("normal-results.json"),
        WireValue::object(vec![
            ("terminal", terminal.clone()),
            ("updates", WireValue::Array(updates.lock().unwrap().clone())),
        ])
        .stringify(),
    )
    .unwrap();
    drop(calls);
    assert!(prepared.session_state().same_identity(&original_state));
    assert_eq!(lease.effect_owners().len(), 3, "same Run retains both RPC snapshots");
    for owner in lease.effect_owners() {
        assert!(owner.settle().await.iter().all(|receipt| !receipt.uncertain()));
    }

    for known in [true, false] {
        let realm = NativeRealm::prepare().unwrap();
        let values = &realm.values;
        let heap = values.heap();
        let builtin = BuiltinToolSource::new(values, ToolContext::new(dir.clone()), &[BuiltinKind::Read]).unwrap();
        let frames = Arc::new(Mutex::new(Vec::new()));
        let recorded = frames.clone();
        let bridge = ToolBridge::new(Arc::new(move |frame| {
            recorded.lock().unwrap().push(frame);
        }));
        let source_slot: Arc<OnceLock<Weak<NativeRpcHostSource>>> = Arc::new(OnceLock::new());
        let owner = Arc::new(ReentrantOwner {
            inner: bridge.native_owners(&definitions("reentrant")).remove(0),
            bridge: bridge.clone(),
            source: source_slot.clone(),
            reply: known,
        });
        let rpc = NativeRpcHostSource::new(
            values,
            vec![owner],
            realm.globals.environment.clone(),
            realm.globals.errors.clone(),
        )
        .unwrap();
        source_slot.set(Arc::downgrade(&rpc)).unwrap();
        let prepared = setup(&realm, &builtin, &rpc);
        let driver = NativeRun::begin(prepared.clone(), CancellationToken::new()).unwrap();
        let lease = driver.lease();
        let (invoker, request) =
            request(&prepared, heap.object().unwrap(), CancellationToken::new(), Arc::new(|_| Ok(())));
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
                .await
                .unwrap()
                .unwrap();
        assert!(result.is_err());
        assert!(frames.lock().unwrap().is_empty(), "known reply or withdrawn grant must not publish/replay");
        let mut receipts = Vec::new();
        for owner in lease.effect_owners() {
            receipts.extend(owner.settle().await);
        }
        std::fs::write(
            dir.join(if known { "known-complete-receipts.json" } else { "not-dispatched-receipts.json" }),
            serde_json::to_vec_pretty(
                &receipts
                    .iter()
                    .map(|receipt| {
                        serde_json::json!({
                            "summary": receipt.summary(), "failed": receipt.failed(), "uncertain": receipt.uncertain(),
                        })
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(receipts.iter().all(|receipt| !receipt.uncertain()));
        if known {
            assert!(!receipts.is_empty(), "authoritative Complete remains represented after grant/cancel");
            assert!(receipts.iter().any(|receipt| receipt.failed()));
        }
    }

    // Actual publication followed by transport loss remains Unknown. There is
    // one request and no replacement invocation after settlement.
    let realm = NativeRealm::prepare().unwrap();
    let values = &realm.values;
    let builtin = BuiltinToolSource::new(values, ToolContext::new(dir.clone()), &[BuiltinKind::Read]).unwrap();
    let frames = Arc::new(Mutex::new(Vec::new()));
    let recorded = frames.clone();
    let slot: Arc<OnceLock<Weak<ToolBridge>>> = Arc::new(OnceLock::new());
    let captured = slot.clone();
    let bridge = ToolBridge::new(Arc::new(move |frame| {
        recorded.lock().unwrap().push(frame);
        captured.get().and_then(Weak::upgrade).unwrap().close("controlled transport loss");
    }));
    slot.set(Arc::downgrade(&bridge)).unwrap();
    let rpc = NativeRpcHostSource::new(
        values,
        bridge.native_owners(&definitions("unknown")),
        realm.globals.environment.clone(),
        realm.globals.errors.clone(),
    )
    .unwrap();
    let prepared = setup(&realm, &builtin, &rpc);
    let driver = NativeRun::begin(prepared.clone(), CancellationToken::new()).unwrap();
    let lease = driver.lease();
    let (invoker, request) =
        request(&prepared, values.heap().object().unwrap(), CancellationToken::new(), Arc::new(|_| Ok(())));
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
        .await
        .unwrap();
    assert!(result.is_err(), "Unknown remains independently unsettled as success");
    let mut unknown = false;
    for owner in lease.effect_owners() {
        unknown |= owner.settle().await.iter().any(|receipt| receipt.uncertain());
    }
    assert!(unknown);
    assert_eq!(frames.lock().unwrap().len(), 1, "transport loss must not replay");
    std::fs::write(dir.join("unknown-wire.json"), WireValue::Array(frames.lock().unwrap().clone()).stringify())
        .unwrap();
    println!(
        "native mixed catalog/RPC: raw UTF16 update/result, active refresh, known failure with cancellation/grant withdrawal, NotDispatched, Unknown/no replay observed"
    );
}
