//! Same coarse Host await family: original after-apply params, per-file order,
//! stringify throw isolation and caller cancellation through the actual driver.
use ara_cli::{
    actual_tools::{BuiltinKind, BuiltinToolSource},
    native_factory::{NativeFactoryImports, PreparedNativeSession},
    native_realm::NativeRealm,
    native_run::NativeRun,
};
use ara_rpc::value_heap::{CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueSnapshot};
use ara_tools::edit_host::{EditClientBridge, EditHostPort};
use ara_tools::{ToolContext, edit_post_apply::*};
use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;
static STAGE: AtomicUsize = AtomicUsize::new(0);
static KEYS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static PARAMS_TO_JSON: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled original params toJSON receives root key then per-file arg key after preceding repair",
    name: "toJSON",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 3,
    body: |heap, captures, receiver, args, runtime| {
        if !receiver.same_identity(&captures[0]) {
            return Err(ValueError::WrongRuntimeReply("original edit params receiver"));
        }
        let ValueSnapshot::String(key) = heap.snapshot(&args[0])? else {
            return Err(ValueError::WrongRuntimeReply("original toJSON key"));
        };
        let key = key.to_utf8().map_err(|_| ValueError::WrongRuntimeReply("fixture key"))?;
        KEYS.lock().unwrap().push(key.clone());
        if key.is_empty() {
            return Ok(captures[1].clone());
        }
        if key != "arg" {
            return Err(ValueError::WrongRuntimeReply("after-apply arg key"));
        }
        if matches!(heap.snapshot(&captures[2])?, ValueSnapshot::Boolean(true)) {
            return Err(ValueError::Thrown(heap.string("original blackbox throw".into())?));
        }
        let result = heap.object()?;
        heap.define_own_data(&result, "key".into(), heap.string(key.into())?, runtime)?;
        heap.define_own_data(&result, "stage".into(), heap.number(STAGE.load(Ordering::SeqCst) as f64)?, runtime)?;
        Ok(result)
    },
};
struct Host {
    dir: PathBuf,
    cancel: Option<CancellationToken>,
    diagnostics: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl EditPostApplyHost for Host {
    fn agent_dir(&self) -> PathBuf {
        self.dir.join("agent")
    }
    fn blackbox_enabled(&self) -> bool {
        true
    }
    fn active_model_string(&self) -> Option<String> {
        Some("fixture/main".into())
    }
    async fn prepare_repair(
        &self,
        _: Option<CancellationToken>,
    ) -> Result<Option<Arc<dyn EditRepairCompletion>>, String> {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
            return Ok(None);
        }
        Ok(Some(Arc::new(Completion)))
    }
    fn diagnostic(&self, message: &str) {
        self.diagnostics.lock().unwrap().push(message.into());
    }
}
struct Completion;
#[async_trait]
impl EditRepairCompletion for Completion {
    fn model_name(&self) -> String {
        "fixture/smol".into()
    }
    fn complete_actual(
        &self,
        _invocation: EditNativeRepairInvocation,
        prompt: String,
        _cancel: CancellationToken,
    ) -> futures::future::BoxFuture<'static, Result<String, ValueError>> {
        // Controlled completion for the original ordering observation. Actual
        // dispatch invokes this port; it must not use the typed fallback.
        Box::pin(async move { Ok(Self::repaired_text(prompt)) })
    }
    async fn complete(&self, prompt: String, _: CancellationToken) -> Result<String, String> {
        Ok(Self::repaired_text(prompt))
    }
}
impl Completion {
    fn repaired_text(prompt: String) -> String {
        STAGE.fetch_add(1, Ordering::SeqCst);
        prompt
            .split("AFTER (broken):")
            .nth(1)
            .unwrap()
            .split("```")
            .nth(1)
            .unwrap()
            .trim()
            .replace("return (;", "return (2);")
    }
}

// Same Host family, no new test entrance: the installed production callback
// reads the current publication after a held original ACP apply settles.
struct LiveModelHooks {
    snapshot: Mutex<ara_agent::ExecutionSnapshot>,
    reads: AtomicUsize,
}
#[async_trait]
impl ara_agent::LoopHooks for LiveModelHooks {
    fn execution_snapshot(&self) -> Option<ara_agent::ExecutionSnapshot> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Some(self.snapshot.lock().unwrap().clone())
    }
}
struct NeverProvider;
impl ara_ai::ModelProvider for NeverProvider {
    fn stream(&self, _: &ara_ai::Model, _: &ara_ai::Context, _: ara_ai::CallOptions) -> ara_ai::AssistantStream {
        panic!("held apply current-model observation must not issue a model request")
    }
}
struct LiveModelHost {
    dir: PathBuf,
    model: crate::host_edit_post_apply::ActiveModelCallback,
}
#[async_trait]
impl EditPostApplyHost for LiveModelHost {
    fn agent_dir(&self) -> PathBuf {
        self.dir.join("agent")
    }
    fn blackbox_enabled(&self) -> bool {
        true
    }
    fn active_model_string(&self) -> Option<String> {
        (self.model)()
    }
    async fn prepare_repair(
        &self,
        _: Option<CancellationToken>,
    ) -> Result<Option<Arc<dyn EditRepairCompletion>>, String> {
        Ok(None)
    }
}
#[derive(Clone, Default)]
struct HeldWriter {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    calls: Arc<AtomicUsize>,
}
impl EditHostPort for HeldWriter {
    fn client_bridge(&self) -> Option<Arc<dyn EditClientBridge>> {
        Some(Arc::new(self.clone()))
    }
}
#[async_trait]
impl EditClientBridge for HeldWriter {
    fn can_write_text_file(&self) -> bool {
        true
    }
    async fn write_text_file(&self, path: &std::path::Path, content: &str) -> std::io::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        std::fs::write(path, content)
    }
}

async fn observe_held_apply_current_model() {
    const SOURCE: &str = "export function value(): number {\n\treturn 1;\n}\n";
    const BROKEN: &str = "export function value(): number {\n\treturn (;\n}\n";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("held.ts");
    let log = dir.path().join("agent/edit-blackbox.jsonl");
    std::fs::create_dir(dir.path().join("agent")).unwrap();
    std::fs::write(&path, SOURCE).unwrap();
    let model = ara_ai::Model {
        id: "A".into(),
        api: "openai-completions".into(),
        provider: "fixture".into(),
        base_url: "http://unused.invalid".into(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    };
    let owner = Arc::new(LiveModelHooks {
        snapshot: Mutex::new(ara_agent::ExecutionSnapshot {
            tools: Vec::new(),
            system_prompt: Vec::new(),
            model: Some(ara_agent::ModelExecutionSnapshot {
                model: model.clone(),
                provider: Arc::new(NeverProvider),
                tool_choice: None,
                max_tokens: None,
                temperature: None,
            }),
            provider_context: None,
        }),
        reads: AtomicUsize::new(0),
    });
    let hooks: Arc<dyn ara_agent::LoopHooks> = owner.clone();
    let callback = crate::host_edit_post_apply::active_model_for_hooks(&model, &hooks);
    let writer = Arc::new(HeldWriter::default());
    let context =
        ToolContext::new(dir.path()).with_edit(ara_edit::EditMode::Replace, true).with_edit_host(writer.clone());
    context.set_edit_post_apply_host(Arc::new(LiveModelHost { dir: dir.path().into(), model: callback.clone() }));
    let realm = NativeRealm::prepare().unwrap();
    let source = BuiltinToolSource::new(&realm.values, context, &[BuiltinKind::Edit]).unwrap();
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
    let original_session = prepared.session_state().clone();
    let arg = serde_json::json!({"path":"held.ts","old_string":"return 1;","new_string":"return (;"});
    let params = heap.import_serde(&arg).unwrap();
    let cancel = CancellationToken::new();
    let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
    let (invoker, request) = crate::native_host_await_observations::make_request(
        &prepared,
        "edit",
        0,
        params.clone(),
        cancel,
        Arc::new(|_| Ok(())),
    );
    let member = request.tool_call.clone();
    let switch = async {
        writer.entered.notified().await;
        let before = (owner.reads.load(Ordering::SeqCst), log.exists(), std::fs::read_to_string(&path));
        owner.snapshot.lock().unwrap().model.as_mut().unwrap().model.id = "B".into();
        writer.release.notify_one();
        before
    };
    let (outcome, before) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(driver.drive(invoker.invoke_settled(request)), switch)
    })
    .await
    .unwrap();
    assert_eq!(before.0, 0, "recorder must not capture model at Run entry or before apply");
    assert!(!before.1);
    assert_eq!(before.2.unwrap(), SOURCE);
    let result_handle = outcome.unwrap().unwrap();
    let is_error = heap.get(&result_handle, &"isError".into(), values.runtime()).unwrap();
    assert!(!heap.truthy(&is_error).unwrap());
    let result = heap.export_serde(&result_handle).unwrap();
    assert_eq!(result["details"]["newText"], BROKEN);
    assert!(prepared.session_state().same_identity(&original_session));
    assert!(heap.get(&member, &"arguments".into(), values.runtime()).unwrap().same_identity(&params));
    assert_eq!(writer.calls.load(Ordering::SeqCst), 1, "successful original apply is not replayed");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), BROKEN);
    assert_eq!(owner.reads.load(Ordering::SeqCst), 1, "capture reads current model once for the recorder");
    let records: Vec<serde_json::Value> =
        std::fs::read_to_string(&log).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(
        records,
        [serde_json::json!({"prev":SOURCE,"new":BROKEN,"model":"fixture/B","variant":"replace","arg":arg})]
    );
    let weak = Arc::downgrade(&owner);
    drop(hooks);
    drop(owner);
    assert!(weak.upgrade().is_none(), "installed callback must not retain hooks or their tools");
    assert!(callback().is_none(), "closed Host must not revive its Run-entry model");
}
pub async fn observe() {
    const SOURCE: &str = "export function value(): number {\n\treturn 1;\n}\n";
    for scenario in ["ordered", "throw", "cancel"] {
        STAGE.store(0, Ordering::SeqCst);
        KEYS.lock().unwrap().clear();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("agent")).unwrap();
        std::fs::write(dir.path().join("a.ts"), SOURCE).unwrap();
        std::fs::write(dir.path().join("b.ts"), SOURCE).unwrap();
        let cancel = CancellationToken::new();
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        let context = ToolContext::new(dir.path()).with_edit(ara_edit::EditMode::ApplyPatch, true);
        context.set_edit_post_apply_host(Arc::new(Host {
            dir: dir.path().into(),
            cancel: (scenario == "cancel").then(|| cancel.clone()),
            diagnostics: diagnostics.clone(),
        }));
        let realm = NativeRealm::prepare().unwrap();
        let source = BuiltinToolSource::new(&realm.values, context, &[BuiltinKind::Edit]).unwrap();
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
        let original_session = prepared.session_state().clone();
        let mut input = "*** Begin Patch\n*** Update File: a.ts\n@@\n-\treturn 1;\n+\treturn (;\n".to_owned();
        if scenario == "ordered" {
            input.push_str("*** Update File: b.ts\n@@\n-\treturn 1;\n+\treturn (;\n");
        }
        input.push_str("*** End Patch\n");
        let wire = heap.import_serde(&serde_json::json!({"input":input})).unwrap();
        let params = heap.object().unwrap();
        let function = heap
            .fresh_compiled_function(
                &PARAMS_TO_JSON,
                &[params.clone(), wire, heap.boolean(scenario == "throw").unwrap()],
            )
            .unwrap();
        heap.define_own_data(&params, "toJSON".into(), function, values.runtime()).unwrap();
        let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        let (invoker, request) = crate::native_host_await_observations::make_request(
            &prepared,
            "edit",
            0,
            params.clone(),
            cancel.clone(),
            Arc::new(|_| Ok(())),
        );
        let member = request.tool_call.clone();
        let outcome =
            tokio::time::timeout(std::time::Duration::from_secs(5), driver.drive(invoker.invoke_settled(request)))
                .await
                .unwrap();
        assert!(prepared.session_state().same_identity(&original_session));
        assert!(heap.get(&member, &"arguments".into(), values.runtime()).unwrap().same_identity(&params));
        if scenario == "cancel" {
            assert!(cancel.is_cancelled());
            assert_eq!(STAGE.load(Ordering::SeqCst), 0);
            assert!(std::fs::read_to_string(dir.path().join("a.ts")).unwrap().contains("return (;"));
            continue;
        }
        let result = outcome.unwrap().unwrap();
        let is_error = heap.get(&result, &"isError".into(), values.runtime()).unwrap();
        assert!(!heap.truthy(&is_error).unwrap());
        let log = dir.path().join("agent/edit-blackbox.jsonl");
        if scenario == "ordered" {
            assert_eq!(*KEYS.lock().unwrap(), ["", "arg", "arg"]);
            let lines: Vec<serde_json::Value> =
                std::fs::read_to_string(log).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
            assert_eq!(lines.len(), 2);
            assert_eq!(lines[0]["arg"], serde_json::json!({"key":"arg","stage":0}));
            assert_eq!(lines[1]["arg"], serde_json::json!({"key":"arg","stage":1}));
            for file in ["a.ts", "b.ts"] {
                assert!(std::fs::read_to_string(dir.path().join(file)).unwrap().contains("return (2);"));
            }
        } else if scenario == "throw" {
            assert_eq!(*KEYS.lock().unwrap(), ["", "arg"]);
            assert!(!log.exists());
            assert!(diagnostics.lock().unwrap().iter().any(|s| s.contains("blackbox recording failed")));
            assert!(std::fs::read_to_string(dir.path().join("a.ts")).unwrap().contains("return (2);"));
        }
    }
    observe_held_apply_current_model().await;
}
