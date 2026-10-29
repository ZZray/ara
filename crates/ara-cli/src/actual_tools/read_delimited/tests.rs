//! Original source Graph body and real Native admission/child dispatch groups.
use super::*;
use crate::{
    native_factory::{NativeFactoryImports, PreparedNativeSession},
    native_realm::NativeRealm,
    native_run::NativeRun,
};
use ara_agent::{ActualToolSource, AgentMessage};
use ara_rpc::value_heap::{PropertyDescriptor, StringifyMode};
use std::path::Path;
fn own(values: &SessionValues, v: &ValueHandle, k: &str, x: ValueHandle) {
    values
        .heap()
        .define_own_data(v, k.into(), x, values.runtime())
        .unwrap();
}
fn string(v: &ValueHandle) -> String {
    match v.heap().snapshot(v).unwrap() {
        ValueSnapshot::String(s) => s.to_utf8().unwrap(),
        _ => panic!("expected string"),
    }
}
fn json(values: &SessionValues, v: &ValueHandle) -> serde_json::Value {
    serde_json::from_str(
        &values
            .heap()
            .stringify(v, StringifyMode::Plain, None, values.runtime())
            .unwrap()
            .unwrap()
            .to_utf8()
            .unwrap(),
    )
    .unwrap()
}
static EXECUTE_GET: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "READ delimiter original this.execute Get receipt",
    name: "get execute",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 2,
    body: |heap, c, _, _, r| {
        let push = heap.get(&c[1], &"push".into(), r)?;
        heap.call(&push, &c[1], &[heap.string("execute Get".into())?], r)?;
        Ok(c[0].clone())
    },
};
fn prepare(
    ctx: ToolContext,
) -> (
    Arc<NativeRealm>,
    Arc<BuiltinToolSource>,
    Arc<PreparedNativeSession>,
    ValueHandle,
    ValueHandle,
) {
    let realm = NativeRealm::prepare().unwrap();
    let values = &realm.values;
    let heap = values.heap();
    let source = BuiltinToolSource::new(values, ctx, &[BuiltinKind::Read]).unwrap();
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
    let collection = source.current_tools(values).unwrap().unwrap();
    let selected = get(heap, &collection, "0", values.runtime()).unwrap();
    let trace = heap.array(Vec::new()).unwrap();
    let execute = get(heap, &selected, "execute", values.runtime()).unwrap();
    let getter = heap
        .fresh_compiled_function(&EXECUTE_GET, &[execute, trace.clone()])
        .unwrap();
    heap.define_own(
        &selected,
        "execute".into(),
        PropertyDescriptor::Accessor {
            get: Some(match heap.snapshot(&getter).unwrap() {
                ValueSnapshot::Function(target) => target,
                _ => panic!("getter must be callable"),
            }),
            set: None,
            enumerable: true,
            configurable: true,
        },
        values.runtime(),
    )
    .unwrap();
    (realm, source, prepared, selected, trace)
}
async fn read(
    realm: &Arc<NativeRealm>,
    source: &Arc<BuiltinToolSource>,
    prepared: &Arc<PreparedNativeSession>,
    selected: &ValueHandle,
    path: &str,
) -> Result<ValueHandle, ActualProviderError> {
    let values = &realm.values;
    let heap = values.heap();
    let cancel = CancellationToken::new();
    let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
    let args = heap
        .import_serde(&serde_json::json!({"path":path}))
        .unwrap();
    let call = heap
        .import_serde(&serde_json::json!({"type":"toolCall","id":"read-outer","name":"read"}))
        .unwrap();
    own(values, &call, "arguments", args.clone());
    let mut assistant = serde_json::to_value(ara_ai::AssistantMessage::empty(
        "fixture", "fixture", "fixture",
    ))
    .unwrap();
    assistant["role"] = "assistant".into();
    assistant["stopReason"] = "toolUse".into();
    let root = heap.import_serde(&assistant).unwrap();
    own(
        values,
        &root,
        "content",
        heap.array(vec![call.clone()]).unwrap(),
    );
    let owner = AgentMessage::from_graph(root, None).unwrap();
    let context = heap.object().unwrap();
    prepared
        .require_tool_hooks()
        .unwrap()
        .bind_run_context(values, &context)
        .unwrap();
    let binding = source.bind_selected(selected, values).unwrap();
    let request = ActualExecuteRequest {
        callable: get(heap, selected, "execute", values.runtime()).unwrap(),
        receiver: selected.clone(),
        arguments_slot: owner.current_arguments_slot(&call).unwrap(),
        owner,
        tool_call: call,
        call_id: "read-outer".into(),
        args,
        values: values.clone(),
        cancel,
        on_update: Arc::new(|_| Ok(())),
        tool_context: context,
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        driver.drive(
            binding
                .capability
                .actual_invoker()
                .unwrap()
                .invoke_settled(request),
        ),
    )
    .await
    .unwrap()
    .unwrap()
}
fn file(root: &Path, name: &str, text: &str) {
    if let Some(parent) = root.join(name).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(root.join(name), text).unwrap();
}
static SESSION_ATTACHMENTS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original Session getImageAttachments Host authority fixture",
    name: "getImageAttachments",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 3,
    body: |heap, c, receiver, args, r| {
        assert!(receiver.same_identity(&c[0]));
        assert!(args.is_empty());
        let push = heap.get(&c[2], &"push".into(), r)?;
        heap.call(&push, &c[2], &[heap.string("lookup".into())?], r)?;
        Ok(c[1].clone())
    },
};
#[tokio::test]
async fn native_read_delimited_original_full_dispatch_group() {
    let temp = tempfile::tempdir().unwrap();
    file(temp.path(), "a.txt", "alpha\n");
    file(temp.path(), "b.txt", "beta\n");
    let (realm, source, prepared, selected, trace) = prepare(ToolContext::new(temp.path()));
    let path = "a.txt;b.txt;missing.txt";
    for _ in 0..2 {
        let result = read(&realm, &source, &prepared, &selected, path)
            .await
            .unwrap();
        let row = json(&realm.values, &result);
        let text = row["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("Note: interpreted as 3 paths: a.txt, b.txt, missing.txt"));
        assert!(
            text.contains("alpha")
                && text.contains("beta")
                && text.contains("[Could not read missing.txt:")
        );
        assert_eq!(
            row["details"]["displayReadTargets"],
            serde_json::json!(["a.txt", "b.txt", "missing.txt"])
        );
        assert_eq!(row["details"]["notes"].as_array().unwrap().len(), 2);
        assert!(row.get("isError").is_none());
    }
    // Two roots + all six authentic child execute Gets; root suppliers only
    // accept their three exact READ operands for a read-delimited-part request.
    assert_eq!(
        realm
            .values
            .heap()
            .array_length(&trace, realm.values.runtime())
            .unwrap(),
        8
    );
    let session = get(
        realm.values.heap(),
        &selected,
        "session",
        realm.values.runtime(),
    )
    .unwrap();
    let key = realm
        .values
        .heap()
        .module_symbol(
            "OMP596f2da:packages/coding-agent/src/tools/read.ts",
            "read.repeatTracker",
        )
        .unwrap();
    let tracker = realm
        .values
        .heap()
        .get(&session, &key, realm.values.runtime())
        .unwrap();
    let method = get(realm.values.heap(), &tracker, "get", realm.values.runtime()).unwrap();
    for tracked in ["a.txt", "b.txt", path] {
        let entry = realm
            .values
            .heap()
            .call(
                &method,
                &tracker,
                &[realm.values.heap().string(tracked.into()).unwrap()],
                realm.values.runtime(),
            )
            .unwrap();
        assert_eq!(
            json(
                &realm.values,
                &get(realm.values.heap(), &entry, "count", realm.values.runtime()).unwrap()
            ),
            serde_json::json!(2)
        );
    }

    let lookups = realm.values.heap().array(Vec::new()).unwrap();
    let entries = realm
        .values
        .heap()
        .import_serde(&serde_json::json!([
            {"uri":"attachment://1","sourcePath":temp.path().join("a.txt")},
            {"uri":"attachment://2","sourcePath":temp.path().join("b.txt")},
            {"uri":"attachment://3","sourcePath":"a.txt;b.txt"}
        ]))
        .unwrap();
    let getter = realm
        .values
        .heap()
        .fresh_compiled_function(
            &SESSION_ATTACHMENTS,
            &[session.clone(), entries, lookups.clone()],
        )
        .unwrap();
    own(&realm.values, &session, "getImageAttachments", getter);
    let result = read(
        &realm,
        &source,
        &prepared,
        &selected,
        "attachment://1;attachment://2",
    )
    .await
    .unwrap();
    let row = json(&realm.values, &result);
    assert_eq!(
        row["details"]["displayReadTargets"],
        serde_json::json!(["attachment://1", "attachment://2"])
    );
    assert_eq!(row["details"]["notes"].as_array().unwrap().len(), 1);
    let text = row["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("alpha") && text.contains("beta"));
    assert_eq!(
        realm
            .values
            .heap()
            .array_length(&lookups, realm.values.runtime())
            .unwrap(),
        2
    );

    // Formal getImageAttachments may return a sourcePath without an exists
    // guarantee. Fixed READ continues parsing that rewritten filesystem path,
    // and its delimiter children must retain the original Native this.execute.
    assert!(!temp.path().join("a.txt;b.txt").exists());
    let before_rewritten = realm
        .values
        .heap()
        .array_length(&trace, realm.values.runtime())
        .unwrap();
    let result = read(&realm, &source, &prepared, &selected, "attachment://3")
        .await
        .unwrap();
    let row = json(&realm.values, &result);
    assert_eq!(
        row["details"]["displayReadTargets"],
        serde_json::json!(["a.txt", "b.txt"])
    );
    assert_eq!(
        row["details"]["notes"],
        serde_json::json!(["Note: interpreted as 2 paths: a.txt, b.txt"])
    );
    let text = row["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("Note: interpreted as 2 paths: a.txt, b.txt"));
    assert!(text.contains("alpha") && text.contains("beta"));
    assert_eq!(
        realm
            .values
            .heap()
            .array_length(&trace, realm.values.runtime())
            .unwrap(),
        before_rewritten + 3 // original attachment root + two fresh Native child Gets
    );
    assert_eq!(
        realm
            .values
            .heap()
            .array_length(&lookups, realm.values.runtime())
            .unwrap(),
        3
    );

    // Native local supplier resolves both URLs independently before ordinary
    // filesystem execution. This exercises the actual session-owned LocalPlan.
    let journal = ara_session::SessionJournal::create(temp.path(), temp.path()).unwrap();
    let artifacts = crate::session_artifacts::SessionArtifacts::for_journal(&journal);
    let host = crate::host_edit_session::HostEditSession::new(
        temp.path().join("home"),
        Default::default(),
    );
    host.bind(&journal, artifacts).unwrap();
    let local = host.local_root().unwrap();
    file(&local, "one.txt", "first\nsecond\n");
    file(&local, "two.txt", "third\n");
    let (realm, source, prepared, selected, trace) =
        prepare(ToolContext::new(temp.path()).with_edit_host(host));
    let result = read(
        &realm,
        &source,
        &prepared,
        &selected,
        "local://one.txt:raw:2-2;local://two.txt",
    )
    .await
    .unwrap();
    let row = json(&realm.values, &result);
    let text = row["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("second") && text.contains("third"));
    assert!(!text.contains("first\n"));
    assert_eq!(
        row["details"]["displayReadTargets"],
        serde_json::json!(["local://one.txt:raw:2-2", "local://two.txt"])
    );
    assert_eq!(
        realm
            .values
            .heap()
            .array_length(&trace, realm.values.runtime())
            .unwrap(),
        3
    );
}
#[test]
fn native_read_delimited_graph_identity_and_error_carriers_group() {
    let realm = NativeRealm::prepare().unwrap();
    let heap = realm.values.heap();
    let r = realm.values.runtime();
    let module = read_continuation::NativeReadContinuation::prepare(&realm).unwrap();
    let bindings = module
        .capture()
        .unwrap()
        .delimited_bindings(&realm.values)
        .unwrap();
    assert!(
        bindings
            .abort
            .same_identity(&realm.tool_error_bindings().unwrap().tool_abort_error)
    );
    let parts = vec!["a".into(), "b".into()];
    let mut aggregate = Aggregate::new(heap, &parts).unwrap();
    let image = heap
        .import_serde(&serde_json::json!({"type":"image","data":"original","mimeType":"image/png"}))
        .unwrap();
    let suffix = heap.object().unwrap();
    let child=heap.import_serde(&serde_json::json!({"content":[{"type":"text","text":"before"}],"details":{"suffixResolution":{}},"isError":true,"providerMetadata":{"original":true}})).unwrap();
    let content = get(heap, &child, "content", r).unwrap();
    push(heap, &content, image.clone(), r).unwrap();
    push(
        heap,
        &content,
        heap.import_serde(&serde_json::json!({"type":"text","text":"after"}))
            .unwrap(),
        r,
    )
    .unwrap();
    let details = get(heap, &child, "details", r).unwrap();
    let resolution = get(heap, &details, "suffixResolution", r).unwrap();
    heap.define_own_data(&resolution, "to".into(), suffix.clone(), r)
        .unwrap();
    aggregate.accept(heap, "a", &child, r).unwrap();
    let ordinary = heap
        .construct(
            &bindings.error,
            &[heap.string("ordinary failure".into()).unwrap()],
            r,
        )
        .unwrap();
    aggregate
        .failed(
            heap,
            &bindings,
            &heap.undefined().unwrap(),
            "b",
            ValueError::Thrown(ordinary),
            r,
        )
        .unwrap();
    let result = aggregate.done(heap, r).unwrap();
    let content = get(heap, &result, "content", r).unwrap();
    assert_eq!(heap.array_length(&content, r).unwrap(), 3);
    assert!(get(heap, &content, "1", r).unwrap().same_identity(&image));
    assert_eq!(
        string(&get(heap, &get(heap, &content, "0", r).unwrap(), "text", r).unwrap()),
        "Note: interpreted as 2 paths: a, b\n\nbefore"
    );
    assert_eq!(
        string(&get(heap, &get(heap, &content, "2", r).unwrap(), "text", r).unwrap()),
        "after\n\n[Could not read b: ordinary failure]"
    );
    let details = get(heap, &result, "details", r).unwrap();
    let display = get(heap, &details, "displayReadTargets", r).unwrap();
    assert!(get(heap, &display, "0", r).unwrap().same_identity(&suffix));
    assert_eq!(
        heap.kind(&get(heap, &result, "isError", r).unwrap())
            .unwrap(),
        ValueKind::Undefined
    );
    assert_eq!(
        heap.kind(&get(heap, &result, "providerMetadata", r).unwrap())
            .unwrap(),
        ValueKind::Undefined
    );
    // True imported ToolAbortError, then an arbitrary original thrown reason
    // with signal.aborted. Both preserve the reason's graph identity exactly.
    for signal_abort in [false, true] {
        let mut aggregate = Aggregate::new(heap, &parts).unwrap();
        let reason = if signal_abort {
            heap.object().unwrap()
        } else {
            heap.construct(&bindings.abort, &[], r).unwrap()
        };
        let signal = heap
            .import_serde(&serde_json::json!({"aborted":signal_abort}))
            .unwrap();
        let error = aggregate
            .failed(
                heap,
                &bindings,
                &signal,
                "a",
                ValueError::Thrown(reason.clone()),
                r,
            )
            .unwrap_err();
        assert!(matches!(error,ValueError::Thrown(original) if original.same_identity(&reason)));
        assert_eq!(heap.array_length(&aggregate.display, r).unwrap(), 0);
    }
}

static DYNAMIC_GET: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "READ source fresh this.execute per-part fixture",
    name: "get execute",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 5,
    body: |heap, c, _, _, r| {
        let n = heap.array_length(&c[4], r)?;
        let push = heap.get(&c[4], &"push".into(), r)?;
        heap.call(&push, &c[4], &[heap.number(n as f64)?], r)?;
        Ok(c[(n as usize).min(3)].clone())
    },
};
static DYNAMIC_PART: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "READ source changed this.execute sync/Promise/throw fixture",
    name: "execute",
    length: 3,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 6,
    body: |heap, c, receiver, args, r| {
        assert!(receiver.same_identity(&c[0]));
        assert_eq!(args.len(), 3);
        assert!(
            matches!(heap.snapshot(&args[0])?,ValueSnapshot::String(id) if id.equals_ascii("read-delimited-part"))
        );
        assert!(heap.object_is(&heap.get(&args[1], &"path".into(), r)?, &c[3])?);
        let push = heap.get(&c[2], &"push".into(), r)?;
        heap.call(&push, &c[2], &[args[2].clone()], r)?;
        match heap.snapshot(&c[5])? {
            ValueSnapshot::Number(0.0) => Ok(c[1].clone()),
            ValueSnapshot::Number(1.0) => {
                let promise = heap.get(&c[4], &"Promise".into(), r)?;
                let resolve = heap.get(&promise, &"resolve".into(), r)?;
                heap.call(&resolve, &promise, &[c[1].clone()], r)
            }
            ValueSnapshot::Number(2.0) => Err(ValueError::Thrown(c[1].clone())),
            ValueSnapshot::Number(3.0) => heap.call(&c[1], receiver, args, r),
            _ => Err(ValueError::InvalidNode),
        }
    },
};
#[tokio::test]
async fn native_read_delimited_fresh_dynamic_execute_and_original_promise_group() {
    let temp = tempfile::tempdir().unwrap();
    let (realm, source, prepared, selected, _) = prepare(ToolContext::new(temp.path()));
    let values = &realm.values;
    let heap = values.heap();
    let r = values.runtime();
    let canonical = source.members[0].execute.clone();
    let gets = heap.array(Vec::new()).unwrap();
    let signals = heap.array(Vec::new()).unwrap();
    let first=heap.import_serde(&serde_json::json!({"content":[{"type":"text","text":"dynamic first"},{"type":"image","data":"kept","mimeType":"image/png"}]})).unwrap();
    let image = get(heap, &get(heap, &first, "content", r).unwrap(), "1", r).unwrap();
    let second=heap.import_serde(&serde_json::json!({"content":[{"type":"text","text":"dynamic second"}],"details":{"suffixResolution":{"to":"resolved-b"}}})).unwrap();
    let third = heap
        .construct(
            &realm.globals.errors.error,
            &[heap.string("dynamic third error".into()).unwrap()],
            r,
        )
        .unwrap();
    let mut functions = vec![canonical];
    for (i, result) in [first, second, third].into_iter().enumerate() {
        functions.push(
            heap.fresh_compiled_function(
                &DYNAMIC_PART,
                &[
                    selected.clone(),
                    result,
                    signals.clone(),
                    heap.string(["a", "b", "c"][i].into()).unwrap(),
                    realm.globals.environment.clone(),
                    heap.number(i as f64).unwrap(),
                ],
            )
            .unwrap(),
        );
    }
    functions.push(gets.clone());
    let getter = heap
        .fresh_compiled_function(&DYNAMIC_GET, &functions)
        .unwrap();
    heap.define_own(
        &selected,
        "execute".into(),
        PropertyDescriptor::Accessor {
            get: Some(match heap.snapshot(&getter).unwrap() {
                ValueSnapshot::Function(target) => target,
                _ => panic!("getter must be callable"),
            }),
            set: None,
            enumerable: true,
            configurable: true,
        },
        r,
    )
    .unwrap();
    let result = read(&realm, &source, &prepared, &selected, "a;b;c")
        .await
        .unwrap();
    let row = json(values, &result);
    assert_eq!(
        row["details"]["displayReadTargets"],
        serde_json::json!(["a", "resolved-b", "c"])
    );
    assert_eq!(
        row["details"]["notes"],
        serde_json::json!([
            "Note: interpreted as 3 paths: a, b, c",
            "Could not read c: dynamic third error"
        ])
    );
    let content = get(heap, &result, "content", r).unwrap();
    assert!(get(heap, &content, "1", r).unwrap().same_identity(&image));
    assert_eq!(
        row["content"][2]["text"],
        "dynamic second\n\n[Could not read c: dynamic third error]"
    );
    assert_eq!(heap.array_length(&gets, r).unwrap(), 4);
    assert_eq!(heap.array_length(&signals, r).unwrap(), 3);
    assert!(
        get(heap, &signals, "0", r)
            .unwrap()
            .same_identity(&get(heap, &signals, "1", r).unwrap())
    );
    assert!(
        get(heap, &signals, "1", r)
            .unwrap()
            .same_identity(&get(heap, &signals, "2", r).unwrap())
    );

    file(temp.path(), "a", "wrapped alpha\n");
    file(temp.path(), "b", "wrapped beta\n");
    let (realm, source, prepared, selected, _) = prepare(ToolContext::new(temp.path()));
    let values = &realm.values;
    let heap = values.heap();
    let r = values.runtime();
    let canonical = source.members[0].execute.clone();
    let gets = heap.array(Vec::new()).unwrap();
    let signals = heap.array(Vec::new()).unwrap();
    let mut functions = vec![canonical.clone()];
    for path in ["a", "b"] {
        functions.push(
            heap.fresh_compiled_function(
                &DYNAMIC_PART,
                &[
                    selected.clone(),
                    canonical.clone(),
                    signals.clone(),
                    heap.string(path.into()).unwrap(),
                    realm.globals.environment.clone(),
                    heap.number(3.0).unwrap(),
                ],
            )
            .unwrap(),
        );
    }
    functions.push(functions[2].clone());
    functions.push(gets.clone());
    let getter = heap
        .fresh_compiled_function(&DYNAMIC_GET, &functions)
        .unwrap();
    heap.define_own(
        &selected,
        "execute".into(),
        PropertyDescriptor::Accessor {
            get: Some(match heap.snapshot(&getter).unwrap() {
                ValueSnapshot::Function(target) => target,
                _ => panic!("getter must be callable"),
            }),
            set: None,
            enumerable: true,
            configurable: true,
        },
        r,
    )
    .unwrap();
    let result = read(&realm, &source, &prepared, &selected, "a;b")
        .await
        .unwrap();
    let row = json(values, &result);
    let text = row["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("wrapped alpha") && text.contains("wrapped beta"));
    assert_eq!(heap.array_length(&gets, r).unwrap(), 3);
    assert_eq!(heap.array_length(&signals, r).unwrap(), 2);
}
