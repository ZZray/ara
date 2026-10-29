//! Actual native Read/Host await observations under the existing coarse gate.
use ara_agent::{
    ActualExecuteRequest, ActualProviderError, ActualToolInvoker, ActualToolSource, AgentMessage, SessionValues,
};
use ara_cli::{
    actual_tools::{BuiltinKind, BuiltinToolSource},
    native_factory::{NativeFactoryImports, PreparedNativeSession},
    native_realm::NativeRealm,
    native_run::NativeRun,
};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, ValueError, ValueHandle, ValueSnapshot,
};
use ara_tools::ToolContext;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio_util::sync::CancellationToken;

pub(crate) fn prepared(directory: &std::path::Path) -> Arc<PreparedNativeSession> {
    prepare_all(directory).0
}
fn prepare_all(directory: &std::path::Path) -> (Arc<PreparedNativeSession>, Arc<BuiltinToolSource>) {
    let realm = NativeRealm::prepare().unwrap();
    let source = BuiltinToolSource::new(
        &realm.values,
        ToolContext::new(directory.to_path_buf()),
        &[BuiltinKind::Read, BuiltinKind::Bash],
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
    (prepared, source)
}
pub(crate) fn make_request(
    prepared: &PreparedNativeSession,
    name: &str,
    index: usize,
    args: ValueHandle,
    cancel: CancellationToken,
    update: ara_agent::ActualUpdateFn,
) -> (Arc<dyn ActualToolInvoker>, ActualExecuteRequest) {
    make_request_with_context_binding(prepared, name, index, args, cancel, update, true)
}
fn make_request_with_context_binding(
    prepared: &PreparedNativeSession,
    name: &str,
    index: usize,
    args: ValueHandle,
    cancel: CancellationToken,
    update: ara_agent::ActualUpdateFn,
    bind_native: bool,
) -> (Arc<dyn ActualToolInvoker>, ActualExecuteRequest) {
    let values = prepared.values();
    let heap = values.heap();
    let hooks = prepared.require_tool_hooks().unwrap();
    let source = hooks.source(values).unwrap();
    let collection = source.current_tools(values).unwrap().unwrap();
    let selected = heap.get(&collection, &index.to_string().as_str().into(), values.runtime()).unwrap();
    let binding = source.bind_selected(&selected, values).unwrap();
    let call = heap.import_serde(&serde_json::json!({"type":"toolCall","id":"native-await","name":name})).unwrap();
    heap.define_own_data(&call, "arguments".into(), args.clone(), values.runtime()).unwrap();
    let mut assistant = serde_json::to_value(ara_ai::AssistantMessage::empty("fixture", "fixture", "fixture")).unwrap();
    assistant["role"] = "assistant".into();
    assistant["stopReason"] = "toolUse".into();
    let root = heap.import_serde(&assistant).unwrap();
    heap.define_own_data(&root, "content".into(), heap.array(vec![call.clone()]).unwrap(), values.runtime()).unwrap();
    let owner = AgentMessage::from_graph(root, None).unwrap();
    let tool_context = heap.object().unwrap();
    if bind_native {
        hooks.bind_run_context(values, &tool_context).unwrap();
    }
    let request = ActualExecuteRequest {
        callable: heap.get(&binding.source, &"execute".into(), values.runtime()).unwrap(),
        receiver: binding.source,
        arguments_slot: owner.current_arguments_slot(&call).unwrap(),
        owner,
        tool_call: call,
        call_id: "native-await".into(),
        args,
        values: values.clone(),
        cancel,
        on_update: update,
        tool_context,
    };
    (binding.capability.actual_invoker().unwrap(), request)
}
fn output_text(values: &SessionValues, result: &ValueHandle) -> String {
    let heap = values.heap();
    let content = heap.get(result, &"content".into(), values.runtime()).unwrap();
    let first = heap.get(&content, &"0".into(), values.runtime()).unwrap();
    let text = heap.get(&first, &"text".into(), values.runtime()).unwrap();
    let ValueSnapshot::String(text) = heap.snapshot(&text).unwrap() else { panic!("native Read text expected") };
    text.to_utf8().unwrap()
}
static THROW_AFTER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled post-await receiver.session getter retains exact original throw",
    name: "get session",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |_, captures, _, _, _| Err(ValueError::Thrown(captures[0].clone())),
};
fn has_reason(error: &ActualProviderError, reason: &ValueHandle) -> bool {
    match error {
        ActualProviderError::Value(ValueError::Thrown(value)) => value.same_identity(reason),
        ActualProviderError::Compound { primary, secondary } => {
            has_reason(primary, reason) || has_reason(secondary, reason)
        }
        _ => false,
    }
}
pub async fn observe() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("read.txt"), "actual native await body").unwrap();
    let (prepared, original_source) = prepare_all(dir.path());
    let values = prepared.values();
    let heap = values.heap();
    let original_session = prepared.session_state().clone();
    let hooks = prepared.require_tool_hooks().unwrap();
    hooks.bind_read_session(&original_session, values).unwrap();
    assert!(
        hooks.bind_read_session(&heap.object().unwrap(), values).is_err(),
        "Host cannot replace a Read Session binding"
    );
    let other_realm = NativeRealm::prepare().unwrap();
    assert!(hooks.bind_read_session(&other_realm.values.heap().object().unwrap(), values).is_err());
    let tools = original_source.current_tools(values).unwrap().unwrap();
    let read_receiver = heap.get(&tools, &"0".into(), values.runtime()).unwrap();
    assert!(heap.get(&read_receiver, &"session".into(), values.runtime()).unwrap().same_identity(&original_session));
    for count in 1..=3 {
        let cancel = CancellationToken::new();
        let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        let args = heap.import_serde(&serde_json::json!({"path":"read.txt"})).unwrap();
        let (invoker, request) = make_request(&prepared, "read", 0, args, cancel, Arc::new(|_| Ok(())));
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        let text = output_text(values, &result);
        assert!(text.contains("actual native await body"));
        assert_eq!(text.contains("You have received this identical output"), count >= 3);
        assert!(prepared.session_state().same_identity(&original_session));
    }
    // A final grant denial follows all graph preparation and must not leave
    // an authoritative pending ticket with no effect capable of settling it.
    original_source.set_granted(BuiltinKind::Read, false).unwrap();
    let cancel = CancellationToken::new();
    let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
    let args = heap.import_serde(&serde_json::json!({"path":"read.txt"})).unwrap();
    let (invoker, request) = make_request(&prepared, "read", 0, args, cancel, Arc::new(|_| Ok(())));
    let denial = tokio::time::timeout(std::time::Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
        .await
        .unwrap()
        .unwrap();
    assert!(denial.is_err());
    original_source.set_granted(BuiltinKind::Read, true).unwrap();
    // A successful OS kernel cannot turn a post-await original graph throw into
    // success. The authoritative execute ticket follows the compiled continuation.
    let source = prepared.require_tool_hooks().unwrap().source(values).unwrap();
    let collection = source.current_tools(values).unwrap().unwrap();
    let receiver = heap.get(&collection, &"0".into(), values.runtime()).unwrap();
    let reason = heap.object().unwrap();
    let getter = heap.fresh_compiled_function(&THROW_AFTER, &[reason.clone()]).unwrap();
    let ValueSnapshot::Function(getter) = heap.snapshot(&getter).unwrap() else { panic!("getter") };
    heap.define_own(
        &receiver,
        "session".into(),
        PropertyDescriptor::Accessor { get: Some(getter), set: None, enumerable: true, configurable: true },
        values.runtime(),
    )
    .unwrap();
    let cancel = CancellationToken::new();
    let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
    let args = heap.import_serde(&serde_json::json!({"path":"read.txt"})).unwrap();
    let (invoker, request) = make_request(&prepared, "read", 0, args, cancel, Arc::new(|_| Ok(())));
    let error = tokio::time::timeout(std::time::Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(has_reason(&error, &reason), "post-await same-reason identity must survive");

    // Native Bash partials are materialized by the driver while the real process
    // is active. Cancellation prevents its trailing file effect and still joins it.
    let cancel = CancellationToken::new();
    let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
    let partials = Arc::new(AtomicUsize::new(0));
    let observed = partials.clone();
    let stop = cancel.clone();
    let update: ara_agent::ActualUpdateFn = Arc::new(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
        stop.cancel();
        Ok(())
    });
    let args = heap.import_serde(&serde_json::json!({"command":"printf native-partial; sleep 1; printf forbidden > native-late.txt","timeout":2})).unwrap();
    let (invoker, request) = make_request(&prepared, "bash", 1, args, cancel, update);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
        .await
        .unwrap();
    assert!(partials.load(Ordering::SeqCst) > 0, "native partial must arrive before process settlement");
    assert!(!dir.path().join("native-late.txt").exists());

    // Infrastructure errors and panics in an actual live partial callback must
    // survive the graph reply boundary and still cancel/join the entered child.
    for panic_callback in [false, true] {
        let cancel = CancellationToken::new();
        let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        let update: ara_agent::ActualUpdateFn = Arc::new(move |_| {
            if panic_callback {
                panic!("native-partial-panic-sentinel");
            }
            Err(ActualProviderError::Callback("native-partial-error-sentinel".into()))
        });
        let args = heap
            .import_serde(&serde_json::json!({
                "command":"printf native-partial; sleep 1; printf forbidden > native-callback-late.txt", "timeout":2
            }))
            .unwrap();
        let (invoker, request) = make_request(&prepared, "bash", 1, args, cancel, update);
        let error =
            tokio::time::timeout(std::time::Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
                .await
                .unwrap()
                .unwrap_err();
        let sentinel = if panic_callback { "native-partial-panic-sentinel" } else { "native-partial-error-sentinel" };
        assert!(error.to_string().contains(sentinel), "original partial infrastructure failure must survive: {error}");
        assert!(!dir.path().join("native-callback-late.txt").exists());
    }
    observe_write_module().await;
    observe_fs_scan_cache().await;
    #[cfg(feature = "test-fixture")]
    observe_real_lsp_supplier().await;
    observe_bash_module().await;
    eprintln!(
        "Native Host await: actual Read repeat continuation x3, same post-await thrown reason, live Bash partial/cancel/join executed"
    );
}

/// Real Native WRITE/EDIT effects consume and invalidate one process-shared
/// scanner. Seed only after graph preparation so the observations stay in TTL.
async fn observe_fs_scan_cache() {
    use ara_cli::host_edit_session::{EditSuppliedServices, HostEditSession};
    use ara_tools::engine::{glob_with_cache, invalidate_fs_scan_cache, Budget, GlobRequest};
    use serde_json::json;
    use std::{
        path::Path,
        time::{Duration, Instant},
    };

    fn list(root: &Path) -> Vec<ara_tools::engine::GlobEntry> {
        glob_with_cache(
            &GlobRequest {
                root,
                pattern: "*.txt",
                recursive: true,
                include_hidden: true,
                use_gitignore: false,
                max_results: usize::MAX,
            },
            &Budget { cancel: CancellationToken::new(), deadline: Instant::now() + Duration::from_secs(3) },
            true,
        )
        .unwrap()
    }
    fn prepare(root: &Path, host: Arc<HostEditSession>) -> Arc<PreparedNativeSession> {
        let realm = NativeRealm::prepare().unwrap();
        let context = ToolContext::new(root).with_edit(ara_edit::EditMode::Hashline, true).with_edit_host(host);
        let source = BuiltinToolSource::new(&realm.values, context, &[BuiltinKind::Write, BuiltinKind::Edit]).unwrap();
        PreparedNativeSession::from_complete_bindings(
            realm.clone(),
            NativeFactoryImports {
                source,
                string_function: realm.globals.string.clone(),
                bun: realm.globals.bun.clone(),
                original_logger: None,
            },
        )
        .unwrap()
    }
    let directory = tempfile::Builder::new().prefix("ara-fs-cache-native-").tempdir().unwrap();
    let root = directory.path().join("work");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("sentinel.txt"), "sentinel\n").unwrap();
    let host = HostEditSession::new(
        directory.path().to_owned(),
        EditSuppliedServices { enable_lsp: Some(false), ..Default::default() },
    );
    let journal = ara_session::SessionJournal::create(&directory.path().join("sessions"), &root).unwrap();
    host.bind(&journal, ara_cli::session_artifacts::SessionArtifacts::for_journal(&journal)).unwrap();
    let fresh = host.fresh();
    let fresh_journal = ara_session::SessionJournal::create(&directory.path().join("fresh-sessions"), &root).unwrap();
    fresh.bind(&fresh_journal, ara_cli::session_artifacts::SessionArtifacts::for_journal(&fresh_journal)).unwrap();
    let original = prepare(&root, host);
    let replacement = prepare(&root, fresh);
    assert!(!original.session_state().same_identity(replacement.session_state()));
    let ttl = ara_tools::engine::fs_scan_cache_ttl_ms();
    assert!(ttl > 0, "this observation requires the shared cache enabled");
    let mut report = Vec::new();
    let mut edit_mtime = None;
    for step in 0..4 {
        let prepared = if step < 2 { &original } else { &replacement };
        let (name, index, arguments) = match step {
            0 => ("write", 0, json!({"path":"a.txt","content":"first\n"})),
            1 => {
                // A deterministic old mtime makes cached metadata replacement
                // observable independently of filesystem timestamp granularity.
                let file = std::fs::File::options().write(true).open(root.join("a.txt")).unwrap();
                file.set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(10)))
                    .unwrap();
                invalidate_fs_scan_cache(Some(&root.join("a.txt")));
                (
                    "edit",
                    1,
                    json!({"input":format!("[a.txt#{}]\nPUT 1.=1:\n+second\n", ara_edit::store::file_hash("first\n"))}),
                )
            }
            2 => (
                "edit",
                1,
                json!({"input":format!("[a.txt#{}]\nMV nested/b.txt\n", ara_edit::store::file_hash("second\n"))}),
            ),
            _ => (
                "edit",
                1,
                json!({"input":format!("[nested/b.txt#{}]\nREM\n", ara_edit::store::file_hash("second\n"))}),
            ),
        };
        let started = Instant::now();
        let before = list(&root);
        if step == 1 {
            edit_mtime = Some(before.iter().find(|entry| entry.relative == "a.txt").unwrap().mtime_ms);
        }
        let marker = format!("external-{step}.txt");
        std::fs::write(root.join(&marker), "external observer\n").unwrap();
        assert!(!list(&root).iter().any(|entry| entry.relative == marker), "seeded raw scan is a real cache hit");
        let values = prepared.values();
        let heap = values.heap();
        let cancel = CancellationToken::new();
        let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        let effects = driver.lease().effects().clone();
        let args = heap.import_serde(&arguments).unwrap();
        let (invoker, request) = make_request(prepared, name, index, args, cancel, Arc::new(|_| Ok(())));
        let output = tokio::time::timeout(Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let receipts = effects.settle().await;
        let body = heap.export_serde(&output).unwrap();
        assert!(body.get("isError").and_then(serde_json::Value::as_bool) != Some(true), "{body}");
        assert!(receipts.iter().all(|receipt| !receipt.failed() && !receipt.uncertain()));
        assert!(!receipts.is_empty(), "actual admitted mutation must have a settlement receipt");
        let after = list(&root);
        assert!(started.elapsed() < Duration::from_millis(ttl), "mutation and readback must precede TTL expiry");
        assert!(
            after.iter().any(|entry| entry.relative == marker),
            "production Host hook discarded the raw cached scan"
        );
        match step {
            0 => assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "first\n"),
            1 => {
                assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "second\n");
                assert!(after.iter().find(|entry| entry.relative == "a.txt").unwrap().mtime_ms > edit_mtime.unwrap());
            }
            2 => {
                assert!(!after.iter().any(|entry| entry.relative == "a.txt"));
                assert!(after.iter().any(|entry| entry.relative == "nested/b.txt"));
                assert_eq!(std::fs::read_to_string(root.join("nested/b.txt")).unwrap(), "second\n");
            }
            _ => {
                assert!(!after.iter().any(|entry| entry.relative == "nested/b.txt"));
                assert!(!root.join("nested/b.txt").exists());
            }
        }
        report.push(json!({"step":(["write","edit","rename-fresh-session","delete-fresh-session"][step]),
            "elapsedMs":started.elapsed().as_millis(),"paths":after.iter().map(|entry|&entry.relative).collect::<Vec<_>>(),
            "diskBytes":if root.join("a.txt").exists(){Some(std::fs::read(root.join("a.txt")).unwrap())}else{None},
            "receipts":receipts.iter().map(|receipt|receipt.summary()).collect::<Vec<_>>()}));
    }
    // A pre-effect refusal keeps authoritative disk bytes unchanged. It cannot
    // be reported as an unknown write, or as cache invalidation after success.
    for mode in ["pre-cancel", "callback-cancel", "callback-error"] {
        invalidate_fs_scan_cache(Some(&root));
        let started = Instant::now();
        let _ = list(&root);
        let marker = format!("{mode}.txt");
        std::fs::write(root.join(&marker), "external\n").unwrap();
        let cancel = CancellationToken::new();
        let driver = NativeRun::begin(original.clone(), cancel.clone()).unwrap();
        let effects = driver.lease().effects().clone();
        let update: ara_agent::ActualUpdateFn = Arc::new({
            let cancel = cancel.clone();
            move |_| {
                if mode == "callback-error" {
                    return Err(ActualProviderError::Callback("fs-cache-before-effect".into()));
                }
                cancel.cancel();
                Ok(())
            }
        });
        let args = original
            .values()
            .heap()
            .import_serde(&json!({"path":"refused.txt","content":"must not appear\n"}))
            .unwrap();
        let (invoker, request) = make_request(&original, "write", 0, args, cancel.clone(), update);
        if mode == "pre-cancel" {
            cancel.cancel();
        }
        let result =
            tokio::time::timeout(Duration::from_secs(5), driver.drive(invoker.invoke_settled(request))).await.unwrap();
        let error = match result {
            Ok(Err(error)) | Err(error) => error,
            Ok(Ok(_)) => panic!("refused mutation succeeded"),
        };
        if mode == "callback-error" {
            assert!(error.to_string().contains("fs-cache-before-effect"));
        }
        let receipts = effects.settle().await;
        assert!(receipts.iter().all(|receipt| !receipt.uncertain()));
        if mode != "pre-cancel" {
            assert!(!receipts.is_empty());
            assert!(receipts.iter().all(|receipt| receipt.failed() && receipt.summary().contains("not_dispatched")));
        }
        assert!(!root.join("refused.txt").exists());
        assert!(!list(&root).iter().any(|entry| entry.relative == marker));
        assert!(started.elapsed() < Duration::from_millis(ttl));
        report.push(json!({"mode":mode,"fileAbsent":true,"receipts":receipts.iter().map(|receipt|receipt.summary()).collect::<Vec<_>>()}));
    }
    eprintln!(
        "Native Fs scan cache receipt: {}",
        json!({"fixedOmp":"596f2da7101178214aa27a753529d15e6b7ad91d",
        "ttlMs":ttl,"actualProcess":std::process::id(),"cases":report,"boundary":"shared scan invalidation supplier only; complete EDIT/WRITE parents and unknown external mutation failure remain OPEN"})
    );
}

/// One complete supplier observation within the existing Native Host coarse
/// group. The server is a built executable child, with real IPC and files.
#[cfg(feature = "test-fixture")]
async fn observe_real_lsp_supplier() {
    use ara_cli::{
        host_edit_session::HostEditSession,
        host_lsp::{HostLspStartup, LspSettings, LspStartupSnapshot},
    };
    use ara_tools::edit_host::{
        EditDeferredDiagnostics, EditDeferredDiagnosticsFactory, EditDiagnosticsTransform, EditHostPort, EditLspBatch,
        EditLspOptions, EditLspWriteRequest,
    };
    use serde_json::json;
    use std::{
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    let executable = std::env::var_os("ARA_LSP_FIXTURE_PATH").map(PathBuf::from).unwrap_or_else(|| {
        let test = std::env::current_exe().unwrap();
        test.parent().unwrap().parent().unwrap().join(if cfg!(windows) {
            "ara-lsp-fixture.exe"
        } else {
            "ara-lsp-fixture"
        })
    });
    assert!(executable.is_file(), "real compiled LSP fixture required: {}", executable.display());
    let directory = tempfile::Builder::new().prefix("ara-lsp-supplier-").tempdir().unwrap();
    let root = directory.path();
    let agent_dir = root.join("agent");
    std::fs::create_dir(&agent_dir).unwrap();
    let log = root.join("lsp-frames.jsonl");
    let configure = |root: &Path, mode: &str, log: &Path| {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(
            root.join("lsp.json"),
            serde_json::to_vec(&json!({"servers":{"fixture":{
                "command":executable,"args":["--mode",mode,"--log",log],"fileTypes":["fixture"],"rootMarkers":["."]
            }}}))
            .unwrap(),
        )
        .unwrap();
    };
    configure(root, "push", &log);
    let settings = LspSettings { shared: false, ..Default::default() };
    let startup = HostLspStartup::new(LspStartupSnapshot::load(root, &agent_dir, settings).unwrap());
    let host = HostEditSession::new(root.to_owned(), startup.supplied());
    let journal = ara_session::SessionJournal::create(&root.join("sessions"), root).unwrap();
    host.bind(&journal, ara_cli::session_artifacts::SessionArtifacts::for_journal(&journal)).unwrap();
    assert!(!host.lsp_options().enable_diagnostics && host.write_lsp_options().enable_diagnostics);
    assert!(!log.exists(), "default lazy construction must not spawn a child");
    startup.schedule_warmup(false);
    assert!(!log.exists(), "print/script has no warmup regardless of lazy");

    let realm = NativeRealm::prepare().unwrap();
    let source = BuiltinToolSource::new(
        &realm.values,
        ToolContext::new(root.to_owned()).with_edit_host(host.clone()),
        &[BuiltinKind::Write],
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
    let values = prepared.values();
    let heap = values.heap();
    let driver = NativeRun::begin(prepared.clone(), CancellationToken::new()).unwrap();
    let effects = driver.lease().effects().clone();
    let args = heap.import_serde(&json!({"path":"native.fixture","content":"ERROR native\n"})).unwrap();
    let (invoker, request) = make_request(&prepared, "write", 0, args, CancellationToken::new(), Arc::new(|_| Ok(())));
    let output = tokio::time::timeout(Duration::from_secs(8), driver.drive(invoker.invoke_settled(request)))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let native_body = heap.export_serde(&output).unwrap();
    let native_diagnostics = &native_body["details"]["diagnostics"];
    eprintln!("Native LSP WRITE body receipt: {native_body}");
    assert!(
        native_diagnostics["messages"].as_array().is_some_and(|messages| messages
            .iter()
            .any(|message| message.as_str().is_some_and(|text| text.contains("fixture issue")))),
        "real child diagnostics reach actual Native WRITE details: {native_body}"
    );
    assert_eq!(
        native_body["details"]["meta"]["diagnostics"]["messages"], native_diagnostics["messages"],
        "Native WRITE preserves the diagnostic rendering carrier"
    );
    let receipts = effects.settle().await;
    assert_eq!(receipts.len(), 1);
    assert!(!receipts[0].failed() && !receipts[0].uncertain());
    assert_eq!(std::fs::read_to_string(root.join("native.fixture")).unwrap(), "ERROR native\n");

    let writer = host.lsp_writethrough().unwrap();
    let diagnostics = host.deferred_diagnostics_host().unwrap();
    let deferred = EditDeferredDiagnostics::new(diagnostics.clone(), true);
    let get_deferred: EditDeferredDiagnosticsFactory = Arc::new({
        let deferred = deferred.clone();
        move |path| Some(deferred.begin(path))
    });
    let transform: EditDiagnosticsTransform = Arc::new({
        let diagnostics = diagnostics.clone();
        move |path, value| diagnostics.reduce_diagnostics(path, value)
    });
    let options = host.write_lsp_options();
    let target = root.join("native.fixture");
    let repeat = writer
        .write_through(EditLspWriteRequest {
            cwd: root,
            absolute_path: &target,
            content: "ERROR native\n",
            options,
            batch: None,
            cancel: None,
            get_deferred: Some(get_deferred.clone()),
            transform_diagnostics: Some(transform.clone()),
            file_fallbacks: None,
            mutation_session_id: host.session_id(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(repeat["messages"], json!([]), "same Session inline ledger deduplicates across Native/direct consumers");
    let replacement = host.fresh();
    assert!(Arc::ptr_eq(&writer, &replacement.lsp_writethrough().unwrap()), "fresh Session retains process supplier");
    assert_eq!(replacement.diagnostics_host().unwrap().get_file_mutation_version(&target), 0);
    assert_ne!(diagnostics.get_file_mutation_version(&target), 0);

    // EDIT+WRITE share one batch, but bytes land before the final post-process.
    let first = root.join("first.fixture");
    let last = root.join("last.fixture");
    let format_options =
        EditLspOptions { enable_format: true, enable_diagnostics: false, deduplicate_diagnostics: true };
    writer
        .write_through(EditLspWriteRequest {
            cwd: root,
            absolute_path: &first,
            content: "UNFORMATTED ERROR\n",
            options: format_options,
            batch: Some(EditLspBatch { id: "mixed-native".into(), flush: false }),
            cancel: None,
            get_deferred: None,
            transform_diagnostics: None,
            file_fallbacks: None,
            mutation_session_id: host.session_id(),
        })
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "UNFORMATTED ERROR\n");
    std::fs::write(&first, "UNFORMATTED WARNING fresh\n").unwrap();
    let merged = writer
        .write_through(EditLspWriteRequest {
            cwd: root,
            absolute_path: &last,
            content: "clean last\n",
            options,
            batch: Some(EditLspBatch { id: "mixed-native".into(), flush: true }),
            cancel: None,
            get_deferred: Some(get_deferred.clone()),
            transform_diagnostics: Some(transform.clone()),
            file_fallbacks: None,
            mutation_session_id: host.session_id(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&first).unwrap(),
        "FORMATTED WARNING fresh\n",
        "flush formats fresh bytes, not the obsolete edit snapshot"
    );
    assert_eq!(merged["formatter"], "formatted");
    assert!(merged["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message.as_str().unwrap().contains("fixture warning")));
    assert!(!merged["messages"].to_string().contains("fixture issue"));

    let slow = root.join("late.fixture");
    let started = Instant::now();
    let inline = writer
        .write_through(EditLspWriteRequest {
            cwd: root,
            absolute_path: &slow,
            content: "SLOW ERROR\n",
            options,
            batch: None,
            cancel: None,
            get_deferred: Some(get_deferred.clone()),
            transform_diagnostics: Some(transform.clone()),
            file_fallbacks: None,
            mutation_session_id: host.session_id(),
        })
        .await
        .unwrap();
    assert!(inline.is_none() && started.elapsed() < Duration::from_millis(1500));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !host.has_pending() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(host.has_pending(), "same fetch returns raw diagnostics through the actual Session late owner");
    let projector = Arc::new(ara_cli::agent_message_projection::AgentMessageProjector::default());
    let late = host.idle_message(projector.clone(), None).unwrap().unwrap().json().unwrap();
    assert_eq!(late["customType"], "lsp_late_diagnostics");
    assert!(late["details"]["files"].to_string().contains("fixture issue"));

    let stale = root.join("stale.fixture");
    writer
        .write_through(EditLspWriteRequest {
            cwd: root,
            absolute_path: &stale,
            content: "SLOW ERROR\n",
            options,
            batch: None,
            cancel: None,
            get_deferred: Some(get_deferred.clone()),
            transform_diagnostics: Some(transform.clone()),
            file_fallbacks: None,
            mutation_session_id: host.session_id(),
        })
        .await
        .unwrap();
    writer
        .write_through(EditLspWriteRequest {
            cwd: root,
            absolute_path: &stale,
            content: "clean newer\n",
            options,
            batch: None,
            cancel: None,
            get_deferred: Some(get_deferred.clone()),
            transform_diagnostics: Some(transform.clone()),
            file_fallbacks: None,
            mutation_session_id: host.session_id(),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(!host.has_pending(), "new mutation cancels/discards old late results");
    assert!(host.idle_message(projector, None).unwrap().is_none());

    let cancelled_target = root.join("pre-cancel.fixture");
    let token = CancellationToken::new();
    token.cancel();
    let failure = writer
        .write_through(EditLspWriteRequest {
            cwd: root,
            absolute_path: &cancelled_target,
            content: "must not be written",
            options,
            batch: None,
            cancel: Some(token),
            get_deferred: None,
            transform_diagnostics: None,
            file_fallbacks: None,
            mutation_session_id: host.session_id(),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind(), std::io::ErrorKind::Interrupted);
    assert!(!cancelled_target.exists());
    startup.shutdown().await.unwrap();
    let frames = std::fs::read_to_string(&log).unwrap();
    assert!(
        frames.contains("textDocument/didChange")
            && frames.contains("textDocument/didSave")
            && frames.contains("workspace/didChangeWatchedFiles")
    );
    assert!(frames.contains("\"event\":\"exit\""), "real child graceful shutdown is observed");

    for mode in ["pull-error", "exit-format", "init-hang"] {
        let path = root.join(mode);
        let log = path.join("frames.jsonl");
        configure(&path, mode, &log);
        let options = EditLspOptions {
            enable_format: mode != "pull-error",
            enable_diagnostics: mode == "pull-error",
            deduplicate_diagnostics: false,
        };
        let startup = HostLspStartup::new(
            LspStartupSnapshot::load(&path, &agent_dir, LspSettings { shared: false, ..Default::default() }).unwrap(),
        );
        let supplied = startup.supplied().lsp.unwrap();
        let target = path.join("failure.fixture");
        let result = tokio::time::timeout(
            Duration::from_secs(8),
            supplied.write_through(EditLspWriteRequest {
                cwd: &path,
                absolute_path: &target,
                content: "ERROR\n",
                options,
                batch: None,
                cancel: None,
                get_deferred: None,
                transform_diagnostics: None,
                file_fallbacks: None,
                mutation_session_id: None,
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "ERROR\n",
            "LSP failure does not lose or replay original bytes"
        );
        if mode == "exit-format" {
            assert_eq!(result.unwrap()["formatter"], "failed");
        } else {
            assert!(result.is_none(), "unknown report must not become clean success");
        }
        let _ = startup.shutdown().await;
    }
    eprintln!(
        "Real LSP supplier coarse receipt: {}",
        json!({"fixedOmp":"596f2da7101178214aa27a753529d15e6b7ad91d",
        "child":executable,"cases":["lazy startup","Native WRITE inline","same ledger","fresh Session independent ledger",
        "mixed EDIT/WRITE fresh flush","format+save+watch","late raw delivery","new mutation stale discard","pre-cancel",
        "pull error unknown","child exit settlement","hung initialize bound","graceful child exit"],
        "boundary":"controlled process only; real gopls/CAS and full parents require Root evidence"})
    );
}
fn has_native_error_message(values: &SessionValues, error: &ActualProviderError, expected: &str) -> bool {
    match error {
        ActualProviderError::Value(ValueError::Thrown(reason)) => {
            let message = values.heap().get(reason, &"message".into(), values.runtime()).unwrap();
            matches!(values.heap().snapshot(&message).unwrap(), ValueSnapshot::String(text) if text.to_utf8().unwrap().contains(expected))
        }
        ActualProviderError::Compound { primary, secondary } => {
            has_native_error_message(values, primary, expected) || has_native_error_message(values, secondary, expected)
        }
        _ => false,
    }
}

/// Same coarse production NativeRun entrance; no new test or fake completion.
async fn observe_write_module() {
    let directory = tempfile::tempdir().unwrap();
    let mut report = Vec::new();
    for mode in [
        "success",
        "generated-guard",
        "path-guard",
        "pre-cancel",
        "throw",
        "infrastructure",
        "panic",
        "callback-cancel",
        "callback-revoke",
    ] {
        let root = directory.path().join(mode);
        std::fs::create_dir(&root).unwrap();
        let realm = NativeRealm::prepare().unwrap();
        let source =
            BuiltinToolSource::new(&realm.values, ToolContext::new(root.clone()), &[BuiltinKind::Write]).unwrap();
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
        let values = prepared.values();
        let heap = values.heap();
        let filename = if mode == "generated-guard" { "guard.ts" } else { "file.txt" };
        let target = root.join("nested").join(filename);
        if mode == "generated-guard" {
            std::fs::create_dir(target.parent().unwrap()).unwrap();
            std::fs::write(&target, "// Code generated by sqlc. DO NOT EDIT.\noriginal").unwrap();
        }
        let reason = heap.object().unwrap();
        let cancel = CancellationToken::new();
        let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        let lease = driver.lease();
        let effects = lease.effects().clone();
        let count = Arc::new(AtomicUsize::new(0));
        let partials = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let update: ara_agent::ActualUpdateFn = Arc::new({
            let count = count.clone();
            let partials = partials.clone();
            let values = values.clone();
            let target = target.clone();
            let reason = reason.clone();
            let cancel = cancel.clone();
            let lease = lease.clone();
            move |partial| {
                assert!(!target.parent().unwrap().exists(), "original update must return before mkdir");
                assert!(!target.exists(), "original update must return before write");
                count.fetch_add(1, Ordering::SeqCst);
                let actual = values.heap().export_serde(&partial)?;
                assert_eq!(
                    actual["content"][0]["text"],
                    format!("Writing 4 bytes to {}...", std::path::Path::new("nested").join("file.txt").display())
                );
                assert_eq!(actual["details"]["resolvedPath"], serde_json::json!(target.to_string_lossy()));
                partials.lock().unwrap().push(actual);
                match mode {
                    "throw" => Err(ValueError::Thrown(reason.clone()).into()),
                    "infrastructure" => {
                        Err(ActualProviderError::Callback("write-original-infrastructure-sentinel".into()))
                    }
                    "panic" => panic!("write-original-panic-sentinel"),
                    "callback-cancel" => {
                        cancel.cancel();
                        Ok(())
                    }
                    "callback-revoke" => {
                        lease.revoke();
                        Ok(())
                    }
                    _ => Ok(()),
                }
            }
        });
        let path = match mode {
            "path-guard" => "xd/near-miss",
            "generated-guard" => "nested/guard.ts",
            _ => "nested/file.txt",
        };
        let args = heap.import_serde(&serde_json::json!({"path":path,"content":"hé😀"})).unwrap();
        let (invoker, request) = make_request(&prepared, "write", 0, args.clone(), cancel.clone(), update);
        let original_call = request.tool_call.clone();
        let original_receiver = request.receiver.clone();
        let original_callable = request.callable.clone();
        if mode == "pre-cancel" {
            cancel.cancel();
        }
        let driven =
            tokio::time::timeout(std::time::Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
                .await
                .unwrap();
        let result = match driven {
            Ok(result) => result,
            Err(error) => Err(error),
        };
        let receipts = effects.settle().await;
        assert!(heap.get(&original_call, &"arguments".into(), values.runtime()).unwrap().same_identity(&args));
        assert!(heap
            .get(&original_receiver, &"execute".into(), values.runtime())
            .unwrap()
            .same_identity(&original_callable));
        if mode == "success" {
            let output = result.unwrap();
            // b91db66e ordinary WRITE reports the persisted UTF8 byte count.
            // This fixture's h + accented e + emoji is 7 UTF8 bytes (4 UTF16 units).
            assert!(output_text(values, &output).contains("Successfully wrote 7 bytes"),
                "ordinary WRITE reports persisted UTF8 bytes: {}", output_text(values, &output));
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "hé😀");
            assert_eq!(count.load(Ordering::SeqCst), 1);
            assert_eq!(receipts.len(), 1);
            assert!(!receipts[0].failed());
            assert!(!receipts[0].uncertain());
        } else {
            let error = result.unwrap_err();
            let expected_count = if matches!(mode, "generated-guard" | "path-guard" | "pre-cancel") { 0 } else { 1 };
            assert_eq!(count.load(Ordering::SeqCst), expected_count);
            if mode == "generated-guard" {
                assert!(has_native_error_message(values, &error, "auto-generated"));
                assert_eq!(
                    std::fs::read_to_string(&target).unwrap(),
                    "// Code generated by sqlc. DO NOT EDIT.\noriginal"
                );
            } else {
                assert!(!target.parent().unwrap().exists(), "{mode} cannot create a parent");
                assert!(!target.exists(), "{mode} cannot write a file");
            }
            if mode == "path-guard" {
                assert!(!root.join("xd").exists());
            }
            if mode == "throw" {
                assert!(has_reason(&error, &reason), "exact original throw must survive");
            }
            if mode == "infrastructure" {
                assert!(error.to_string().contains("write-original-infrastructure-sentinel"));
            }
            if mode == "panic" {
                assert!(error.to_string().contains("write-original-panic-sentinel"));
            }
            if matches!(mode, "throw" | "infrastructure" | "panic" | "callback-cancel" | "callback-revoke") {
                assert_eq!(receipts.len(), 1, "entered Writing worker must settle");
                assert!(receipts[0].failed());
                assert!(!receipts[0].uncertain(), "{mode} has authoritative pre-effect refusal");
                assert!(receipts[0].summary().contains("not_dispatched"));
            }
        }
        report.push(serde_json::json!({"mode":mode,"updates":count.load(Ordering::SeqCst),"partials":partials.lock().unwrap().clone(),"receipts":receipts.iter().map(|r|r.summary()).collect::<Vec<_>>() }));
    }
    observe_write_batch_order().await;
    // An already released owner slot cannot admit an acknowledgement. This
    // observes the closed admission branch without giving workers Graph data.
    let owner = Arc::new(ara_cli::actual_tools::async_effect::HostEffectSettlementOwner::new());
    let reservation = owner.reserve().unwrap();
    let publisher = reservation.publisher();
    drop(reservation);
    assert!(publisher.acknowledge_write_progress(ara_agent::ToolOutput::text("never admitted")).await.is_err());
    assert!(publisher.prepare_write_batch().await.is_err());
    let closed = owner.settle().await;
    assert_eq!(closed.len(), 1);
    assert!(!closed[0].uncertain());
    eprintln!(
        "Native WRITE coarse receipt: {}",
        serde_json::json!({"fixedOmp":"596f2da7101178214aa27a753529d15e6b7ad91d","cases":report,"closedAdmission":closed[0].summary(),"scope":"ordinary local Writing acknowledgement; complete WRITE and legacy Boundary OPEN"})
    );
}

// Original toolCall Getter, driven only by the admitted kind7 continuation.
static WRITE_BATCH_GETTER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Fixed WRITE original batch context Getter order observation",
    name: "get toolCall",
    length: 0,
    form: CompiledFunctionForm::Arrow,
    capture_count: 3,
    body: |heap, captures, _, _, effective| {
        let previous = heap.get(&captures[0], &"count".into(), effective)?;
        let next = heap.add(&previous, &heap.number(1.0)?, effective)?;
        heap.define_own_data(&captures[0], "count".into(), next, effective)?;
        if heap.truthy(&captures[1])? {
            return Err(ValueError::Thrown(captures[1].clone()));
        }
        Ok(captures[2].clone())
    },
};

async fn observe_write_batch_order() {
    let directory = tempfile::tempdir().unwrap();
    for mode in ["success", "batch-throw", "path-guard", "generated-guard", "uri-early"] {
        let root = directory.path().join(mode);
        std::fs::create_dir(&root).unwrap();
        let realm = NativeRealm::prepare().unwrap();
        let source =
            BuiltinToolSource::new(&realm.values, ToolContext::new(root.clone()), &[BuiltinKind::Write]).unwrap();
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
        let values = prepared.values();
        let heap = values.heap();
        let cancel = CancellationToken::new();
        let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        let effects = driver.lease().effects().clone();
        let counter = heap.import_serde(&serde_json::json!({"count":0})).unwrap();
        let reason = if mode == "batch-throw" { heap.object().unwrap() } else { heap.undefined().unwrap() };
        let context_call = heap
            .import_serde(
                &serde_json::json!({"batchId":"mixed","index":1,"toolCalls":[{"name":"edit"},{"name":"write"}]}),
            )
            .unwrap();
        let getter = heap
            .fresh_compiled_function(&WRITE_BATCH_GETTER, &[counter.clone(), reason.clone(), context_call])
            .unwrap();
        let ValueSnapshot::Function(getter) = heap.snapshot(&getter).unwrap() else { panic!("getter") };
        let updates = Arc::new(AtomicUsize::new(0));
        let update: ara_agent::ActualUpdateFn = Arc::new({
            let counter = counter.clone();
            let values = values.clone();
            let updates = updates.clone();
            move |_| {
                assert!(
                    matches!(values.heap().snapshot(&values.heap().get(&counter, &"count".into(), values.runtime())?)?, ValueSnapshot::Number(value) if value == 1.0)
                );
                updates.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });
        let path = match mode {
            "path-guard" => "xd/near-miss",
            "uri-early" => "skill://absent/read-only",
            "generated-guard" => "target.go",
            _ => "target.txt",
        };
        if mode == "generated-guard" {
            std::fs::write(root.join(path), "// Code generated by sqlc. DO NOT EDIT.\nold").unwrap();
        }
        let args = heap.import_serde(&serde_json::json!({"path":path,"content":"new"})).unwrap();
        let (invoker, request) = make_request(&prepared, "write", 0, args, cancel, update);
        heap.define_own(
            &request.tool_context,
            "toolCall".into(),
            PropertyDescriptor::Accessor { get: Some(getter), set: None, enumerable: true, configurable: true },
            values.runtime(),
        )
        .unwrap();
        let driven =
            tokio::time::timeout(std::time::Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
                .await
                .unwrap();
        let result = match driven {
            Ok(result) => result,
            Err(error) => Err(error),
        };
        let receipts = effects.settle().await;
        let expected = if matches!(mode, "path-guard" | "uri-early") { 0.0 } else { 1.0 };
        assert!(
            matches!(heap.snapshot(&heap.get(&counter,&"count".into(),values.runtime()).unwrap()).unwrap(),ValueSnapshot::Number(value) if value==expected)
        );
        if mode == "success" {
            assert!(result.is_ok());
            assert_eq!(updates.load(Ordering::SeqCst), 1);
            assert_eq!(std::fs::read_to_string(root.join(path)).unwrap(), "new");
        } else {
            let error = result.unwrap_err();
            assert_eq!(updates.load(Ordering::SeqCst), 0);
            if mode == "batch-throw" {
                assert!(has_reason(&error, &reason));
                assert!(!root.join(path).exists());
                assert_eq!(receipts.len(), 1);
                assert!(!receipts[0].uncertain());
            }
        }
    }
}

fn prepared_with_bash_owner(
    directory: &std::path::Path,
    owner: Arc<ara_cli::actual_tools::bash_native::SessionBashOwner>,
) -> (Arc<PreparedNativeSession>, Arc<BuiltinToolSource>) {
    let realm = NativeRealm::prepare().unwrap();
    let source = BuiltinToolSource::new_with_bash_session(
        &realm.values,
        ToolContext::new(directory.to_path_buf()),
        &[BuiltinKind::Read, BuiltinKind::Bash],
        owner,
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
    (prepared, source)
}
async fn bash_request(
    prepared: &Arc<PreparedNativeSession>,
    args: ValueHandle,
    update: ara_agent::ActualUpdateFn,
) -> Result<Result<ValueHandle, ActualProviderError>, ActualProviderError> {
    let cancel = CancellationToken::new();
    let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
    let (invoker, request) = make_request(prepared, "bash", 1, args, cancel, update);
    tokio::time::timeout(std::time::Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
        .await
        .unwrap()
}
async fn wait_marker(path: &std::path::Path) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !path.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
async fn observe_bash_module() {
    use ara_cli::actual_tools::bash_native::{BashSessionSettings, SessionBashOwner};
    let directory = tempfile::tempdir().unwrap().keep();
    eprintln!("Native Bash module artifacts: {}", directory.display());
    let owner = SessionBashOwner::new(BashSessionSettings::default());
    let (prepared, source) = prepared_with_bash_owner(&directory, owner.clone());
    let values = prepared.values();
    let heap = values.heap();
    // The retained compatibility entrance has two awaits but no native Run.
    // A successful process result must not be mistaken for another preflight.
    let legacy_owner = SessionBashOwner::new(BashSessionSettings::default());
    let (legacy_prepared, _) = prepared_with_bash_owner(&directory, legacy_owner.clone());
    let args = legacy_prepared
        .values()
        .heap()
        .import_serde(&serde_json::json!({"command":"printf legacy_stdout","timeout":3}))
        .unwrap();
    let (invoker, request) = make_request_with_context_binding(
        &legacy_prepared,
        "bash",
        1,
        args,
        CancellationToken::new(),
        Arc::new(|_| Ok(())),
        false,
    );
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), invoker.invoke_settled(request))
        .await
        .unwrap()
        .unwrap();
    assert!(output_text(legacy_prepared.values(), &result).starts_with("legacy_stdout\n\nWall time: "));
    legacy_owner.close_and_settle().await;
    // Reuses the actual Native Host entry, real process owner and original args.
    for timeout in [serde_json::json!("1.5"), serde_json::Value::Null, serde_json::json!(false), serde_json::json!(0)] {
        let args = heap
            .import_serde(&serde_json::json!({"command":"printf timeout_body","pty":true,"timeout":timeout}))
            .unwrap();
        let requested = heap.get(&args, &"timeout".into(), values.runtime()).unwrap();
        let result = bash_request(&prepared, args, Arc::new(|_| Ok(()))).await.unwrap().unwrap();
        let text = output_text(values, &result);
        assert!(text.contains("timeout_body") && text.contains("pty requested but unavailable"));
        let details = heap.get(&result, &"details".into(), values.runtime()).unwrap();
        assert!(heap
            .get(&details, &"requestedTimeoutSeconds".into(), values.runtime())
            .unwrap()
            .same_identity(&requested));
    }
    // Missing cwd settles before the original timeout conversion. With a valid
    // cwd, the exact original valueOf throw survives the real preflight await.
    let reason = heap.object().unwrap();
    let conversion = heap.fresh_compiled_function(&THROW_AFTER, &[reason.clone()]).unwrap();
    let timeout = heap.object().unwrap();
    heap.define_own_data(&timeout, "valueOf".into(), conversion, values.runtime()).unwrap();
    for missing in [true, false] {
        let args = heap
            .import_serde(&serde_json::json!({"command":"printf forbidden > timeout_must_not_run","pty":true}))
            .unwrap();
        heap.define_own_data(&args, "timeout".into(), timeout.clone(), values.runtime()).unwrap();
        if missing {
            heap.define_own_data(&args, "cwd".into(), heap.string("missing_cwd".into()).unwrap(), values.runtime())
                .unwrap();
        }
        let error = bash_request(&prepared, args, Arc::new(|_| Ok(()))).await.unwrap().unwrap_err();
        assert_eq!(has_reason(&error, &reason), !missing, "Bash timeout throw/cwd precedence");
        assert!(!directory.join("timeout_must_not_run").exists());
    }
    // Fractional timeout is not rounded by the native OS boundary. The deadline
    // result is an error; the independent receipt still fences unknown effects.
    let cancel = CancellationToken::new();
    let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
    let lease = driver.lease();
    let args = heap.import_serde(&serde_json::json!({"command":"printf fraction_started > fraction.started; sleep 2; printf forbidden > fraction.late","pty":true,"timeout":1.25})).unwrap();
    let (invoker, request) = make_request(&prepared, "bash", 1, args, cancel, Arc::new(|_| Ok(())));
    let started = std::time::Instant::now();
    let outcome =
        tokio::time::timeout(std::time::Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
            .await
            .unwrap();
    assert!(outcome.is_err(), "timeout effects remain explicitly unknown");
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(1) && started.elapsed() < std::time::Duration::from_secs(2)
    );
    assert!(directory.join("fraction.started").exists() && !directory.join("fraction.late").exists());
    assert!(lease.effects().settle().await.iter().any(|receipt| receipt.uncertain()));
    // A real explicit job outlives the originating Run, and the same Session
    // catalog refresh retains exactly that owner. A new Session is independent.
    let args = heap.import_serde(&serde_json::json!({"command":"printf background_started > bg.started; sleep 1; printf background_done > bg.done","async":true,"pty":true,"timeout":3})).unwrap();
    let cancel = CancellationToken::new();
    let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
    let (invoker, request) = make_request(&prepared, "bash", 1, args, cancel, Arc::new(|_| Ok(())));
    let forbidden_ui = heap.fresh_compiled_function(&THROW_AFTER, &[reason.clone()]).unwrap();
    let ValueSnapshot::Function(forbidden_ui) = heap.snapshot(&forbidden_ui).unwrap() else {
        panic!("UI getter");
    };
    heap.define_own(
        &request.tool_context,
        "hasUI".into(),
        PropertyDescriptor::Accessor { get: Some(forbidden_ui), set: None, enumerable: true, configurable: true },
        values.runtime(),
    )
    .unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(output_text(values, &result).contains("Backgrounded as job bg_"));
    assert!(!output_text(values, &result).contains("pty requested"));
    wait_marker(&directory.join("bg.started")).await;
    assert!(!directory.join("bg.done").exists());
    let refreshed = BuiltinToolSource::new_with_bash_session(
        values,
        ToolContext::new(directory.clone()),
        &[BuiltinKind::Bash],
        owner.clone(),
    )
    .unwrap();
    assert!(Arc::ptr_eq(&source.bash_session(), &refreshed.bash_session()));
    assert!(SessionBashOwner::new(BashSessionSettings::default()).pending_deliveries().is_empty());
    tokio::time::timeout(std::time::Duration::from_secs(4), owner.effects().settle()).await.unwrap();
    assert!(directory.join("bg.done").exists());
    let deliveries = owner.pending_deliveries();
    assert_eq!(deliveries.len(), 1);
    assert!(owner.commit_delivery(&deliveries[0].id));
    assert!(!owner.commit_delivery(&deliveries[0].id));
    // Live foreground progress, auto-backgrounding, and running-cap fallback
    // share the same runner. No individual new test entry is introduced.
    let auto_owner = SessionBashOwner::new(BashSessionSettings {
        auto_background_threshold_ms: 400.0,
        max_running_jobs: 1,
        ..Default::default()
    });
    let (auto_prepared, _) = prepared_with_bash_owner(&directory, auto_owner.clone());
    let auto_heap = auto_prepared.values().heap();
    let partials = Arc::new(AtomicUsize::new(0));
    let seen = partials.clone();
    let args = auto_heap.import_serde(&serde_json::json!({"command":"printf auto_live; printf entered > auto.started; sleep 2; printf forbidden > auto.late","timeout":4})).unwrap();
    let result = bash_request(
        &auto_prepared,
        args,
        Arc::new(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(partials.load(Ordering::SeqCst) > 0 && output_text(auto_prepared.values(), &result).contains("auto_live"));
    assert!(!directory.join("auto.late").exists());
    let args = auto_heap.import_serde(&serde_json::json!({"command":"printf cap_foreground","timeout":3})).unwrap();
    let result = bash_request(&auto_prepared, args, Arc::new(|_| Ok(()))).await.unwrap().unwrap();
    assert!(output_text(auto_prepared.values(), &result).contains("cap_foreground"));
    assert!(!output_text(auto_prepared.values(), &result).contains("Backgrounded as job"));
    let receipts =
        tokio::time::timeout(std::time::Duration::from_secs(3), auto_owner.close_and_settle()).await.unwrap();
    #[cfg(windows)]
    {
        assert!(receipts.iter().any(|receipt| receipt.failed() && receipt.confirmed_bash_settlement()));
        assert!(receipts.iter().all(|receipt| !receipt.uncertain()));
    }
    #[cfg(not(windows))]
    assert!(receipts.iter().any(|receipt| receipt.uncertain()));
    assert!(!directory.join("auto.late").exists() && auto_owner.pending_deliveries().is_empty());
    // Two admitted automatic calls contend for the one Session slot. The loser
    // keeps its exact prepared command/permit and completes in the foreground.
    let race_owner = SessionBashOwner::new(BashSessionSettings {
        auto_background_threshold_ms: 200.0,
        max_running_jobs: 1,
        ..Default::default()
    });
    let (race_prepared, _) = prepared_with_bash_owner(&directory, race_owner.clone());
    let race_heap = race_prepared.values().heap();
    let cancel = CancellationToken::new();
    let driver = NativeRun::begin(race_prepared.clone(), cancel.clone()).unwrap();
    let left = race_heap
        .import_serde(&serde_json::json!({
            "command":"sleep 2; printf left_done > race.left.done; printf left_stdout", "timeout":4,
        }))
        .unwrap();
    let right = race_heap
        .import_serde(&serde_json::json!({
            "command":"sleep 2; printf right_done > race.right.done; printf right_stdout", "timeout":4,
        }))
        .unwrap();
    let (left_invoker, left_request) =
        make_request(&race_prepared, "bash", 1, left, cancel.clone(), Arc::new(|_| Ok(())));
    let (right_invoker, right_request) = make_request(&race_prepared, "bash", 1, right, cancel, Arc::new(|_| Ok(())));
    let (left, right) = tokio::time::timeout(
        std::time::Duration::from_secs(6),
        driver.drive(async {
            futures::join!(left_invoker.invoke_settled(left_request), right_invoker.invoke_settled(right_request))
        }),
    )
    .await
    .unwrap()
    .unwrap();
    let texts =
        [output_text(race_prepared.values(), &left.unwrap()), output_text(race_prepared.values(), &right.unwrap())];
    assert_eq!(texts.iter().filter(|text| text.contains("Backgrounded as job")).count(), 1);
    race_owner.effects().settle().await;
    assert!(directory.join("race.left.done").exists() && directory.join("race.right.done").exists());
    assert_eq!(race_owner.snapshots().len(), 1);
    race_owner.close_and_settle().await;
    let disabled = SessionBashOwner::new(BashSessionSettings { async_enabled: false, ..Default::default() });
    let (disabled_prepared, _) = prepared_with_bash_owner(&directory, disabled.clone());
    let args = disabled_prepared
        .values()
        .heap()
        .import_serde(&serde_json::json!({"command":"printf forbidden > disabled.late","async":true}))
        .unwrap();
    let error = bash_request(&disabled_prepared, args, Arc::new(|_| Ok(()))).await.unwrap().unwrap_err();
    assert!(
        error.to_string().contains("Async bash execution is disabled") && !directory.join("disabled.late").exists()
    );
    let report = serde_json::json!({"fixedOmp":"596f2da7101178214aa27a753529d15e6b7ad91d","branches":["legacy_two_await_completion","raw_timeout_and_no_ui_pty","preflight_before_original_throw","fractional_timeout_unknown","explicit_job_outlives_run_and_skips_pty_ui","same_session_owner","delivery_receipt","live_auto_background","capacity_foreground","concurrent_capacity_foreground","close_join_no_replay","async_disabled"],"fractionalOutcome":outcome.err().unwrap().to_string(),"unknownReceipts":receipts.iter().map(|receipt|receipt.summary()).collect::<Vec<_>>(),"livePartials":partials.load(Ordering::SeqCst),"concurrentResults":texts});
    std::fs::write(directory.join("native-bash-receipt.json"), serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    owner.close_and_settle().await;
    disabled.close_and_settle().await;
    eprintln!("Native Bash coarse module receipt: {}", directory.join("native-bash-receipt.json").display());
}
