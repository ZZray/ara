//! Actual native MCP observations inside the existing MCP coarse entrance.
//! Reuses the existing stdio fixture, grants and process modes; no new test.
use ara_agent::{ActualExecuteRequest, ActualProviderError, ActualToolInvoker, ActualToolSource, AgentMessage};
use ara_cli::{
    actual_tools::BuiltinToolSource,
    mcp_native::{NativeMcpDriver, NativeMcpSource, local::LocalRoutes},
    native_factory::{NativeFactoryImports, PreparedNativeSession},
    native_logger::logger_host::NativeLoggerHostOptions,
    native_realm::{NativeRealm, date::SystemDateHost, promise::NativePromiseRevoker},
};
use ara_mcp::native_graph::{self as graph, NativeMcpImports};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, ValueError, ValueHandle, ValueSnapshot,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

#[path = "../../src/native_observations/mcp_dependencies.rs"]
mod dependencies;

struct Harness {
    realm: Arc<NativeRealm>,
    _prepared: Arc<PreparedNativeSession>,
    source: Arc<NativeMcpSource>,
    imports: NativeMcpImports,
    local_options: ValueHandle,
    revoker: NativePromiseRevoker,
    record: PathBuf,
}
static REVOKE_NEXT_OWNER: Mutex<Option<NativePromiseRevoker>> = Mutex::new(None);
static REVOKE_LOCAL_OWNER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Controlled actual Host callback revokes only Promise admission before returning its local root",
    name: "getArtifactsDir",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: |_, captures, _, _, _| {
        REVOKE_NEXT_OWNER.lock().unwrap().take().expect("controlled original owner").revoke();
        Ok(captures[0].clone())
    },
};
static RETURN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Controlled Host original local protocol option callback",
    name: "getArtifactsDir",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: |_, captures, _, _, _| Ok(captures[0].clone()),
};
static THROW_AFTER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Controlled original MCP getter throws its retained reason",
    name: "get value",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: |_, captures, _, _, _| Err(ValueError::Thrown(captures[0].clone())),
};
static SCHEMA_GETTER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Controlled MCP schema.properties returns its original value then throws on the second Get",
    name: "get properties",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 3,
    body: |heap, captures, _, _, rt| {
        heap.array_push(&captures[0], &[heap.undefined()?], rt)?;
        if heap.array_length(&captures[0], rt)? == 1 {
            Ok(captures[1].clone())
        } else {
            Err(ValueError::Thrown(captures[2].clone()))
        }
    },
};
async fn prepare(root: &Path, mode: &str) -> (Harness, NativeMcpDriver) {
    std::fs::create_dir_all(root.join("artifacts/local")).unwrap();
    std::fs::write(root.join("artifacts/local/note.txt"), "actual MCP local artifact").unwrap();
    let record = root.join("server.json");
    let tools =
        ara_mcp::connect_owned(super::config(mode, &record), root, &["fixture:echo".into()], &[]).await.unwrap();
    let realm = NativeRealm::prepare().unwrap();
    let builtin = BuiltinToolSource::new(&realm.values, ara_tools::ToolContext::new(root.to_path_buf()), &[]).unwrap();
    let prepared = PreparedNativeSession::from_complete_bindings_with_logger(
        realm.clone(),
        NativeFactoryImports {
            source: builtin,
            string_function: realm.globals.string.clone(),
            bun: realm.globals.bun.clone(),
            original_logger: None,
        },
        NativeLoggerHostOptions {
            default_logs_dir: root.join("logs"),
            filename_prefix: "omp".into(),
            date: Arc::new(SystemDateHost),
        },
    )
    .unwrap();
    let heap = realm.values.heap();
    let rt = realm.values.runtime();
    realm.globals.install_mcp_dependencies(rt).unwrap();
    let imports = NativeMcpImports::prepare(heap, realm.globals.environment.clone(), rt).unwrap();
    let options = heap.object().unwrap();
    let callback = heap
        .fresh_compiled_function(&RETURN, &[heap.string(root.join("artifacts").to_str().unwrap().into()).unwrap()])
        .unwrap();
    heap.define_own_data(&options, "getArtifactsDir".into(), callback, rt).unwrap();
    let source = NativeMcpSource::new(
        &realm.values,
        tools,
        imports.clone(),
        LocalRoutes { override_options: Some(options.clone()), main_session_manager: None },
        realm.globals.errors.clone(),
    )
    .unwrap();
    let promise = realm.begin_promise_run().unwrap();
    let revoker = promise.revoker();
    let driver = NativeMcpDriver::new(promise, source.clone()).unwrap();
    (Harness { realm, _prepared: prepared, source, imports, local_options: options, revoker, record }, driver)
}
fn get(h: &Harness, object: &ValueHandle, key: &str) -> ValueHandle {
    h.realm.values.heap().get(object, &key.into(), h.realm.values.runtime()).unwrap()
}
fn string(h: &Harness, value: &ValueHandle) -> String {
    let ValueSnapshot::String(text) = h.realm.values.heap().snapshot(value).unwrap() else {
        panic!("native string expected")
    };
    text.to_utf8().unwrap()
}
fn text(h: &Harness, output: &ValueHandle, index: usize) -> String {
    string(h, &get(h, &get(h, &get(h, output, "content"), &index.to_string()), "text"))
}
fn make_request(
    h: &Harness,
    args: ValueHandle,
    cancel: CancellationToken,
) -> (Arc<dyn ActualToolInvoker>, ActualExecuteRequest) {
    let values = &h.realm.values;
    let heap = values.heap();
    let rt = values.runtime();
    let collection = h.source.current_tools(values).unwrap().unwrap();
    let selected = heap.get(&collection, &"0".into(), rt).unwrap();
    let binding = h.source.bind_selected(&selected, values).unwrap();
    let call = heap
        .import_serde(&serde_json::json!({"type":"toolCall","id":"native-mcp","name":"mcp__fixture__echo"}))
        .unwrap();
    heap.define_own_data(&call, "arguments".into(), args.clone(), rt).unwrap();
    let mut assistant = serde_json::to_value(ara_ai::AssistantMessage::empty("fixture", "fixture", "fixture")).unwrap();
    assistant["role"] = "assistant".into();
    assistant["stopReason"] = "toolUse".into();
    let root = heap.import_serde(&assistant).unwrap();
    heap.define_own_data(&root, "content".into(), heap.array(vec![call.clone()]).unwrap(), rt).unwrap();
    let owner = AgentMessage::from_graph(root, None).unwrap();
    let request = ActualExecuteRequest {
        callable: get(h, &binding.source, "execute"),
        receiver: binding.source,
        arguments_slot: owner.current_arguments_slot(&call).unwrap(),
        owner,
        tool_call: call,
        call_id: "native-mcp".into(),
        args,
        values: values.clone(),
        cancel,
        on_update: Arc::new(|_| Ok(())),
        tool_context: heap.object().unwrap(),
    };
    (binding.capability.actual_invoker().unwrap(), request)
}
fn record(h: &Harness) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(&h.record).unwrap()).unwrap()
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
fn graph_result_identities(h: &Harness) {
    let heap = h.realm.values.heap();
    let rt = h.realm.values.runtime();
    let remote = heap
        .import_serde(&serde_json::json!({"content":[
        {"type":"text","text":"{\"status\":\"ok\"}"},
        {"type":"image","mimeType":"image/png","data":"retained-original-image"}
    ],"structuredContent":{"status":"ok"},"_meta":{"trace":"original-meta"}}))
        .unwrap();
    let raw = get(h, &remote, "content");
    let image = get(h, &raw, "1");
    let meta = get(h, &remote, "_meta");
    let collection = h.source.current_tools(&h.realm.values).unwrap().unwrap();
    let source = get(h, &collection, "0");
    let output = graph::build_result(heap, &h.imports, &remote, &source, rt).unwrap();
    let details = get(h, &output, "details");
    let content = get(h, &output, "content");
    assert!(get(h, &details, "rawContent").same_identity(&raw));
    assert!(get(h, &details, "mcpMeta").same_identity(&meta));
    assert!(get(h, &content, "1").same_identity(&image));
    assert_eq!(
        heap.array_length(&content, rt).unwrap(),
        2,
        "original Bun.deepEquals suppresses duplicate structured text"
    );
}
pub async fn observe() {
    let temp = tempfile::Builder::new().prefix("ara-native-mcp-").tempdir().unwrap().keep();
    eprintln!("Native MCP actual coarse artifacts: {}", temp.display());
    let mut rows = Vec::new();
    // Original graph → normalization/local OS worker → original stringify → real process.
    {
        let root = temp.join("normal");
        let (h, driver) = prepare(&root, "normal").await;
        let observation =
            dependencies::observe_mcp_dependencies(h.realm.values.heap(), &h.realm.globals, h.realm.values.runtime())
                .unwrap();
        assert_eq!(observation.comparison_rows, 17);
        let marker = root.join("dispatched.txt");
        let args = h
            .realm
            .values
            .heap()
            .import_serde(&serde_json::json!({"text":"hello native","i":"harness-only",
            "marker":marker,"extra":{"file":"local://note.txt"}}))
            .unwrap();
        let original_extra = get(&h, &args, "extra");
        let original_file = get(&h, &original_extra, "file");
        let effects = driver.run().effects();
        let (invoker, request) = make_request(&h, args.clone(), CancellationToken::new());
        let output = tokio::time::timeout(Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(text(&h, &output, 0), "echo: hello native");
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "dispatched");
        let receipt = record(&h);
        assert_eq!(receipt["call"]["name"], "echo");
        assert!(receipt["call"]["arguments"].get("i").is_none());
        let resolved = receipt["call"]["arguments"]["extra"]["file"].as_str().unwrap();
        assert_eq!(std::fs::read_to_string(resolved).unwrap(), "actual MCP local artifact");
        assert!(get(&h, &get(&h, &args, "extra"), "file").same_identity(&original_file));
        assert_eq!(string(&h, &original_file), "local://note.txt");
        assert_eq!(receipt["ambient_key_present"], false);
        let settled = effects.settle().await;
        assert_eq!(settled.len(), 2);
        assert!(settled.iter().all(|r| !r.failed() && !r.uncertain()));
        graph_result_identities(&h);
        rows.push(serde_json::json!({"mode":"normal","actual_file":marker,"resolved_file":resolved,
            "comparison_rows":observation.comparison_rows,"effects":settled.iter().map(|r| r.summary()).collect::<Vec<_>>(),
            "raw_content_meta_image_identity":true,"original_args_unchanged":true}));
    }
    // Existing remote resource/structured fixture reaches the original source formatter.
    {
        let root = temp.join("structured");
        let (h, driver) = prepare(&root, "structured").await;
        let args = h.realm.values.heap().object().unwrap();
        let (invoker, request) = make_request(&h, args, CancellationToken::new());
        let effects = driver.run().effects();
        let output = tokio::time::timeout(Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(text(&h, &output, 0), "[Resource: file:///report.txt]\nreport body");
        let rendered = text(&h, &output, 1);
        assert!(rendered.contains("\"status\": \"ok\"") && rendered.contains("\"count\": 2"));
        let raw = get(&h, &get(&h, &output, "details"), "rawContent");
        assert_eq!(string(&h, &get(&h, &get(&h, &raw, "0"), "type")), "resource");
        let settled = effects.settle().await;
        assert_eq!(settled.len(), 1);
        assert!(settled.iter().all(|r| !r.uncertain()));
        rows.push(serde_json::json!({"mode":"structured","text":text(&h, &output, 0),"structured_text":rendered,
            "effects":settled.iter().map(|r| r.summary()).collect::<Vec<_>>() }));
    }
    // Original pre- and post-await getters reject the source Promise before remote publication.
    for location in ["provider", "schema", "intent"] {
        let root = temp.join(format!("original_throw_{location}"));
        let (h, driver) = prepare(&root, "normal").await;
        let heap = h.realm.values.heap();
        let reason = heap.object().unwrap();
        let selected = get(&h, &h.source.current_tools(&h.realm.values).unwrap().unwrap(), "0");
        let args = heap.import_serde(&serde_json::json!({"text":"throw before network","i":"internal"})).unwrap();
        let schema = get(&h, &selected, "parameters");
        let properties = get(&h, &schema, "properties");
        let trace = heap.array(vec![]).unwrap();
        let getter = if location == "schema" {
            heap.fresh_compiled_function(&SCHEMA_GETTER, &[trace.clone(), properties, reason.clone()]).unwrap()
        } else {
            heap.fresh_compiled_function(&THROW_AFTER, &[reason.clone()]).unwrap()
        };
        let ValueSnapshot::Function(target) = heap.snapshot(&getter).unwrap() else { panic!("getter callable") };
        let (object, key) = match location {
            "schema" => (&schema, "properties"),
            "intent" => (&args, "i"),
            _ => (&selected, "provider"),
        };
        heap.define_own(
            object,
            key.into(),
            PropertyDescriptor::Accessor { get: Some(target), set: None, enumerable: true, configurable: true },
            h.realm.values.runtime(),
        )
        .unwrap();
        let (invoker, request) = make_request(&h, args, CancellationToken::new());
        let effects = driver.run().effects();
        let error = tokio::time::timeout(Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(has_reason(&error, &reason));
        assert!(record(&h).get("call").is_none());
        assert!(effects.settle().await.is_empty());
        if location == "schema" {
            assert_eq!(heap.array_length(&trace, h.realm.values.runtime()).unwrap(), 2);
        }
        rows.push(serde_json::json!({"mode":format!("original_throw_{location}"),"same_reason":true,"remote_call":false,"effects":0}));
    }
    // The original local callback can revoke admission before any OS work.
    {
        let root = temp.join("revoked_local");
        let (h, driver) = prepare(&root, "normal").await;
        let heap = h.realm.values.heap();
        let rt = h.realm.values.runtime();
        let rejected_root = root.join("must_not_create_artifacts");
        assert!(!rejected_root.exists());
        *REVOKE_NEXT_OWNER.lock().unwrap() = Some(h.revoker.clone());
        let callback = heap
            .fresh_compiled_function(
                &REVOKE_LOCAL_OWNER,
                &[heap.string(rejected_root.to_str().unwrap().into()).unwrap()],
            )
            .unwrap();
        heap.define_own_data(&h.local_options, "getArtifactsDir".into(), callback, rt).unwrap();
        let args = heap.import_serde(&serde_json::json!({"text":"local://note.txt"})).unwrap();
        let (invoker, request) = make_request(&h, args, CancellationToken::new());
        let effects = driver.run().effects();
        let result =
            tokio::time::timeout(Duration::from_secs(5), driver.drive(invoker.invoke_settled(request))).await.unwrap();
        assert!(match result {
            Err(_) => true,
            Ok(inner) => inner.is_err(),
        });
        assert!(REVOKE_NEXT_OWNER.lock().unwrap().is_none(), "original callback must execute");
        assert!(!rejected_root.exists(), "revoked admission must not enter the local OS worker");
        assert!(record(&h).get("call").is_none());
        assert!(effects.settle().await.is_empty());
        rows.push(serde_json::json!({"mode":"revoked_local","callback_executed":true,
            "local_root":rejected_root,"local_root_created":false,"remote_call":false,"effects":0}));
    }
    // Real local resolver rejects traversal and retains the known local failure receipt.
    {
        let root = temp.join("denied_local");
        let (h, driver) = prepare(&root, "normal").await;
        let args = h.realm.values.heap().import_serde(&serde_json::json!({"text":"local://../forbidden.txt"})).unwrap();
        let (invoker, request) = make_request(&h, args, CancellationToken::new());
        let effects = driver.run().effects();
        let error = tokio::time::timeout(Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        let ActualProviderError::Value(ValueError::Thrown(reason)) = error else {
            panic!("local failure original throw expected")
        };
        assert!(
            h.realm
                .values
                .heap()
                .instance_of(&reason, &h.realm.globals.errors.error, h.realm.values.runtime())
                .unwrap()
        );
        let message = string(&h, &get(&h, &reason, "message"));
        assert!(message.contains("Path traversal"));
        assert!(record(&h).get("call").is_none());
        assert!(!root.join("artifacts/forbidden.txt").exists());
        let settled = effects.settle().await;
        assert_eq!(settled.len(), 1);
        assert!(settled[0].failed() && !settled[0].uncertain());
        rows.push(serde_json::json!({"mode":"denied_local","message":message,"remote_call":false,"effects":settled.iter().map(|r| r.summary()).collect::<Vec<_>>() }));
    }
    // Cancellation occurs only after the actual remote marker proves publication.
    {
        let root = temp.join("published_cancel");
        let (h, driver) = prepare(&root, "hang").await;
        let marker = root.join("dispatched.txt");
        let cancel = CancellationToken::new();
        let args =
            h.realm.values.heap().import_serde(&serde_json::json!({"text":"held call","marker":marker})).unwrap();
        let (invoker, request) = make_request(&h, args.clone(), cancel.clone());
        let effects = driver.run().effects();
        let abort = h.imports.errors.tool_abort_error.clone();
        let observed = Arc::new(Mutex::new(false));
        let original_observed = observed.clone();
        let invocation = async {
            let result = invoker.invoke_settled(request).await;
            if let Err(ActualProviderError::Value(ValueError::Thrown(reason))) = &result {
                *original_observed.lock().unwrap() =
                    h.realm.values.heap().instance_of(reason, &abort, h.realm.values.runtime()).unwrap();
            }
            result
        };
        let trigger = async {
            let start = Instant::now();
            while !(marker.exists()
                && std::fs::read(&h.record)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    .is_some_and(|receipt| receipt["call"]["name"] == "echo"))
            {
                assert!(start.elapsed() < Duration::from_secs(3), "actual MCP process did not publish marker");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            cancel.cancel();
        };
        let result =
            tokio::time::timeout(Duration::from_secs(5), driver.drive(async { tokio::join!(invocation, trigger) }))
                .await
                .unwrap();
        assert!(result.is_err(), "unknown remote effects cannot be a successful drive");
        assert!(*observed.lock().unwrap());
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "dispatched");
        let before = record(&h);
        assert_eq!(before["call"]["name"], "echo");
        let settled = effects.settle().await;
        assert_eq!(settled.len(), 1);
        assert!(settled[0].uncertain());
        let replay_marker = root.join("must-not-replay.txt");
        let replay_args = h
            .realm
            .values
            .heap()
            .import_serde(&serde_json::json!({"text":"must not replay","marker":replay_marker}))
            .unwrap();
        let (again, request) = make_request(&h, replay_args, CancellationToken::new());
        assert!(again.invoke_settled(request).await.is_err());
        assert!(!replay_marker.exists(), "revoked invocation must not enter a second real file effect");
        assert_eq!(record(&h), before, "revoked Run must not replay the remote call");
        rows.push(serde_json::json!({"mode":"published_cancel","original_tool_abort":true,"drive_success":false,
            "remote_marker":marker,"no_replay":true,"effects":settled.iter().map(|r| r.summary()).collect::<Vec<_>>() }));
    }
    // A remote exit formats its source error result, but the independent receipt keeps it unknown.
    {
        let root = temp.join("remote_exit");
        let (h, driver) = prepare(&root, "exit").await;
        let marker = root.join("dispatched.txt");
        let args = h
            .realm
            .values
            .heap()
            .import_serde(&serde_json::json!({"text":"exit after effect","marker":marker}))
            .unwrap();
        let (invoker, request) = make_request(&h, args, CancellationToken::new());
        let effects = driver.run().effects();
        let rendered = Arc::new(Mutex::new(None));
        let original_rendered = rendered.clone();
        let invocation = async {
            let result = invoker.invoke_settled(request).await;
            if let Ok(output) = &result {
                assert!(h.realm.values.heap().truthy(&get(&h, output, "isError")).unwrap());
                *original_rendered.lock().unwrap() = Some(text(&h, output, 0));
            }
            result
        };
        let result = tokio::time::timeout(Duration::from_secs(5), driver.drive(invocation)).await.unwrap();
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "dispatched");
        let message = rendered.lock().unwrap().clone().expect("original formatMCPToolFailure must execute");
        assert!(message.contains("fixture") && message.contains("echo"));
        let settled = effects.settle().await;
        assert_eq!(settled.len(), 1);
        assert!(settled[0].uncertain());
        rows.push(serde_json::json!({"mode":"remote_exit","drive_success":false,"formatted_error":message,
            "effects":settled.iter().map(|r| r.summary()).collect::<Vec<_>>() }));
    }
    std::fs::write(temp.join("native-receipt.json"), serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
    eprintln!("Native MCP actual coarse: {} modes; artifacts {}", rows.len(), temp.display());
}
