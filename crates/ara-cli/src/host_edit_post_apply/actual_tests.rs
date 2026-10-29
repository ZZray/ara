//! Production Completion::complete_actual on one original NativeRun.
//! Controlled provider/resources isolate this caller boundary; the sibling
//! library tests exercise the production effect/resource settlement owner.
use super::*;
use ara_ai::{
    ActualAssistantStream, ActualStreamFailureReceipt, AssistantMessageEvent, GraphProviderContext,
    graph_values::GraphValueFactory, retry::RetryBindings,
};
use ara_cli::{
    actual_tools::BuiltinToolSource,
    native_factory::{NativeFactoryImports, PreparedNativeSession},
    native_realm::NativeRealm,
    native_run::NativeRun,
    retry_host::NativeRetryBindings,
};
use ara_rpc::value_heap::{ValueError, ValueHandle};
use ara_tools::edit_post_apply::{EditNativeRepairInvocation, EditRepairResources};
use futures::{FutureExt, future::Shared};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

type Write = Shared<BoxFuture<'static, Result<(), String>>>;
#[derive(Default)]
struct Resources {
    serial: AtomicUsize,
    receipts: Mutex<Vec<ActualStreamFailureReceipt>>,
    writes: Mutex<Vec<Write>>,
    uncertain: AtomicUsize,
}
impl EditRepairResources for Resources {
    fn begin_attempt(&self, _: CancellationToken) -> Result<u64, ValueError> {
        Ok(self.serial.fetch_add(1, Ordering::SeqCst) as u64)
    }
    fn register_started(
        &self,
        key: u64,
        receipt: ActualStreamFailureReceipt,
    ) -> Result<(), ValueError> {
        let mut receipts = self.receipts.lock().unwrap();
        assert_eq!(key as usize, receipts.len());
        receipts.push(receipt);
        Ok(())
    }
    fn no_request(&self, _: u64) -> Result<(), ValueError> {
        panic!("This controlled provider always publishes its original receipt")
    }
    fn dispatch_uncertain(&self, _: u64) -> Result<(), ValueError> {
        self.uncertain.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn register_usage(
        &self,
        key: u64,
        observation: u64,
        write: BoxFuture<'static, Result<(), String>>,
    ) -> Result<BoxFuture<'static, Result<(), String>>, ValueError> {
        assert_eq!(observation, 0);
        let mut writes = self.writes.lock().unwrap();
        assert_eq!(key as usize, writes.len());
        let captured = write.shared();
        writes.push(captured.clone());
        Ok(Box::pin(captured))
    }
}

struct Provider {
    roots: Vec<ValueHandle>,
    loop_attempts: Vec<usize>,
    calls: AtomicUsize,
    contexts: Mutex<Vec<ValueHandle>>,
}
impl ModelProvider for Provider {
    fn stream(&self, _: &Model, _: &Context, _: CallOptions) -> ara_ai::AssistantStream {
        panic!("Actual Edit must never use the typed completion path")
    }
    fn stream_actual(
        &self,
        model: &Model,
        context: &GraphProviderContext,
        factory: Arc<GraphValueFactory>,
        options: CallOptions,
    ) -> Result<ActualAssistantStream, ValueError> {
        context.check_lineage(&factory)?;
        assert_eq!(options.max_tokens, Some(8192));
        assert!(options.reasoning.as_ref().unwrap().disable_reasoning);
        self.contexts.lock().unwrap().push(
            context
                .original_context
                .clone()
                .expect("Original user Context"),
        );
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        let root = self
            .roots
            .get(index)
            .expect("No extra dispatch permitted")
            .clone();
        let (sink, stream, receipt) = ActualAssistantStream::channel();
        // Guard captured before spawning: even an unpolled task seals its owner.
        let guard = receipt.owner_settlement_guard();
        let is_error = index + 1 < self.roots.len();
        let loops = self.loop_attempts.contains(&index);
        let content_index = factory.heap().number(0.0)?;
        tokio::spawn(async move {
            let _guard = guard;
            let event = if loops {
                // Exercise the production exact-cycle guard, not a fabricated
                // classified terminal. This exceeds its fixed 180-char bound.
                AssistantMessageEvent::ThinkingDelta {
                    content_index,
                    delta: "repeat cycle ".repeat(20).into(),
                    partial: root,
                }
            } else if is_error {
                AssistantMessageEvent::Error {
                    reason: StopReason::Error,
                    error: root,
                }
            } else {
                AssistantMessageEvent::Done {
                    reason: StopReason::Stop,
                    message: root,
                }
            };
            receipt.retain_terminal(&event);
            // One event is published before the guard can consume and close
            // its input. A lost delivery remains an independent failure receipt.
            if !sink.push(event).await {
                receipt.record(ValueError::UnsupportedOperation(
                    "Controlled Edit provider event delivery failed",
                ));
            }
        });
        Ok(if loops {
            ara_ai::thinking_loop::guard_actual_thinking_loop_stream(
                stream,
                model,
                options.cancel.child_token(),
                ara_ai::thinking_loop::LoopGuardPolicy::default(),
                factory,
            )
        } else {
            stream
        })
    }
}

fn root(factory: &Arc<GraphValueFactory>, failure: Option<&str>) -> ValueHandle {
    // Input fixture construction only; production consumes original handles.
    let mut value = serde_json::json!({
        "role":"assistant", "api":"openai-completions", "provider":"fixture", "model":"repair",
        "content":if failure.is_some() { serde_json::json!([]) } else { serde_json::json!([{"type":"text", "text":"fixed source"}]) },
        "stopReason":if failure.is_some() { "error" } else { "stop" }, "timestamp":1700000000000_i64,
        "usage":{"input":3,"output":2,"cacheRead":0,"cacheWrite":0,"totalTokens":5,
            "cost":{"input":0.001,"output":0.001,"cacheRead":0,"cacheWrite":0,"total":0.002}}
    });
    if let Some(message) = failure {
        value["errorMessage"] = message.into();
    }
    assert!(
        value.get("errorId").is_none(),
        "Controlled provider input has no classified terminal; the original classifier/guard must supply it"
    );
    factory.heap().import_serde(&value).unwrap()
}

#[tokio::test]
async fn actual_edit_retry_original_run_usage_and_cancel() {
    for mode in ["success", "usage-failure", "cancel"] {
        let realm = NativeRealm::prepare().expect("Original realm");
        let source = BuiltinToolSource::new(
            &realm.values,
            ara_tools::ToolContext::new(std::env::current_dir().unwrap()),
            &[],
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
        let run = NativeRun::begin(prepared, CancellationToken::new()).unwrap();
        let factory = realm.values.factory().clone();
        let calling = run.calling_run(&factory).unwrap();
        let bindings = NativeRetryBindings::prepare(
            &factory,
            &realm,
            calling.clone(),
            ActualStreamFailureReceipt::default(),
        )
        .unwrap();
        let module = bindings.oneshot_module(&factory).unwrap();
        let controller = {
            let mut controller = None;
            calling
                .with_runtime(&factory, &mut |runtime| {
                    controller = Some(bindings.signals().controller(&factory, runtime)?);
                    factory.heap().undefined()
                })
                .unwrap();
            controller.unwrap()
        };
        let roots = match mode {
            "success" => vec![
                root(&factory, Some("429 Too Many Requests")),
                root(&factory, None),
                root(&factory, None),
                root(&factory, None),
            ],
            "cancel" => vec![
                root(&factory, Some("429 Too Many Requests")),
                root(&factory, None),
            ],
            _ => vec![root(&factory, None)],
        };
        let expected = if mode == "success" { 4 } else { 1 };
        let provider = Arc::new(Provider {
            roots,
            loop_attempts: if mode == "success" {
                vec![1, 2]
            } else {
                Vec::new()
            },
            calls: AtomicUsize::new(0),
            contexts: Mutex::new(Vec::new()),
        });
        let resources = Arc::new(Resources::default());
        let records = Arc::new(Mutex::new(Vec::new()));
        let written = Arc::new(Notify::new());
        let executed_writes = Arc::new(AtomicUsize::new(0));
        let usage: UsageCallback = {
            let records = records.clone();
            let written = written.clone();
            let executed_writes = executed_writes.clone();
            Arc::new(move |observation| {
                assert!(matches!(&observation, UsageObservation::Actual { .. }));
                records.lock().unwrap().push(observation.record());
                let written = written.clone();
                let executed_writes = executed_writes.clone();
                Box::pin(async move {
                    executed_writes.fetch_add(1, Ordering::SeqCst);
                    written.notify_one();
                    if mode == "usage-failure" {
                        Err("controlled journal write failure".into())
                    } else {
                        Ok(())
                    }
                })
            })
        };
        let completion = Completion {
            provider: provider.clone(),
            usage,
            model: Model {
                id: "repair".into(),
                api: "openai-completions".into(),
                provider: "fixture".into(),
                base_url: "http://unused.invalid".into(),
                reasoning: false,
                max_tokens: None,
                context_window: None,
                tokenizer: None,
            },
        };
        let cancel = CancellationToken::new();
        let invocation = EditNativeRepairInvocation {
            factory: factory.clone(),
            bindings: bindings.clone(),
            module,
            signal: controller.signal.clone(),
            resources: resources.clone(),
        };
        let reason = factory.heap().object().unwrap();
        let observation = async {
            let work = completion.complete_actual(
                invocation,
                "repair the original source".into(),
                cancel.clone(),
            );
            tokio::pin!(work);
            if mode == "cancel" {
                tokio::select! {
                    result=&mut work => panic!("Cancelled case returned before original backoff: {result:?}"),
                    _=written.notified()=>{}
                }
                // First attempt is observed; allow its source backoff to begin.
                tokio::select! {
                    result=&mut work => panic!("Source transient backoff missing: {result:?}"),
                    _=tokio::time::sleep(Duration::from_millis(20))=>{}
                }
                calling
                    .with_runtime(&factory, &mut |runtime| {
                        bindings.signals().abort(
                            &factory,
                            &controller,
                            Some(reason.clone()),
                            runtime,
                        )?;
                        factory.heap().undefined()
                    })
                    .unwrap();
                cancel.cancel();
            }
            work.await
        };
        let result = tokio::time::timeout(Duration::from_secs(20), run.drive(observation))
            .await
            .expect("Original Run bound")
            .expect("Original driver/resource settlement");
        match mode {
            "success" => assert_eq!(result.unwrap(), "fixed source"),
            "cancel" => assert!(
                matches!(result, Err(ValueError::Thrown(ref thrown)) if thrown.same_identity(&reason))
            ),
            _ => assert!(
                result.is_err(),
                "Failed usage write must prevent completion success"
            ),
        }
        assert_eq!(provider.calls.load(Ordering::SeqCst), expected);
        assert_eq!(resources.uncertain.load(Ordering::SeqCst), 0);
        let receipts = resources.receipts.lock().unwrap().clone();
        assert_eq!(receipts.len(), expected);
        for (index, receipt) in receipts.into_iter().enumerate() {
            assert!(
                tokio::time::timeout(Duration::from_secs(2), receipt.wait_settled())
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert!(receipt.is_settled(), "Every original producer joined");
            if mode == "success" && matches!(index, 1 | 2) {
                let terminal = receipt.known_terminals();
                assert_eq!(terminal.len(), 1, "Production guard supplies one terminal");
                let root = terminal[0].partial();
                assert!(!root.same_identity(&provider.roots[index]));
                let id = factory
                    .heap()
                    .get(root, &"errorId".into(), factory.runtime())
                    .unwrap();
                let mask = ara_ai::retry_classification::flag::THINKING_LOOP
                    | ara_ai::retry_classification::flag::CLASS;
                assert!(
                    matches!(
                        factory.heap().snapshot(&id).unwrap(),
                        ara_rpc::value_heap::ValueSnapshot::Number(value) if (value as u32 & mask) == mask
                    ),
                    "Production guard emits source thinking-loop/class bits"
                );
            }
        }
        let writes = resources.writes.lock().unwrap().clone();
        assert_eq!(writes.len(), expected);
        for write in writes {
            assert_eq!(write.await.is_err(), mode == "usage-failure");
        }
        assert_eq!(
            executed_writes.load(Ordering::SeqCst),
            expected,
            "Same captured usage writes execute once"
        );
        let rows = records.lock().unwrap();
        assert_eq!(rows.len(), expected);
        assert!(
            rows.iter()
                .all(|row| row.usage.total_tokens == Some(5) && row.purpose == "edit-auto-repair")
        );
        if mode == "success" {
            assert_eq!(
                rows.iter()
                    .map(|row| row.stop_reason.clone())
                    .collect::<Vec<_>>(),
                [
                    StopReason::Error,
                    StopReason::Error,
                    StopReason::Error,
                    StopReason::Stop
                ]
            );
            let contexts = provider.contexts.lock().unwrap();
            assert!(
                !contexts[0].same_identity(&contexts[1]),
                "Outer oneshot rebuilds Context"
            );
            assert!(
                contexts[1].same_identity(&contexts[2]) && contexts[2].same_identity(&contexts[3]),
                "Inner thinking-loop preserves original Context"
            );
        }
        eprintln!("ARA-RETRY-EDIT PASS {mode} original-run consumer and receipts");
    }
}


static FORBIDDEN_ABORT_CONTENT: ara_rpc::value_heap::CompiledFunctionSpec = ara_rpc::value_heap::CompiledFunctionSpec {
    source:"b91db66e repair aborted terminal must not read content",name:"get content",length:0,
    form:ara_rpc::value_heap::CompiledFunctionForm::Builtin,capture_count:0,
    body:|_,_,_,_,_|Err(ValueError::UnsupportedOperation("aborted completion content was read")),
};
#[tokio::test]
async fn actual_repair_aborted_before_content_and_provider_retry_options() {
    for mode in ["aborted","provider"] {
        let realm=NativeRealm::prepare().unwrap();
        let source=BuiltinToolSource::new(&realm.values,ara_tools::ToolContext::new(std::env::current_dir().unwrap()),&[]).unwrap();
        let prepared=PreparedNativeSession::from_complete_bindings(realm.clone(),NativeFactoryImports {source,string_function:realm.globals.string.clone(),bun:realm.globals.bun.clone(),original_logger:None}).unwrap();
        let run=NativeRun::begin(prepared,CancellationToken::new()).unwrap();
        let factory=realm.values.factory().clone();let calling=run.calling_run(&factory).unwrap();
        let bindings=NativeRetryBindings::prepare(&factory,&realm,calling.clone(),ActualStreamFailureReceipt::default()).unwrap();
        let module=bindings.oneshot_module(&factory).unwrap();
        let controller={let mut controller=None;calling.with_runtime(&factory,&mut|runtime|{controller=Some(bindings.signals().controller(&factory,runtime)?);factory.heap().undefined()}).unwrap();controller.unwrap()};
        let roots=if mode=="aborted" {
            let response=root(&factory,None);let heap=factory.heap();
            heap.define_own_data(&response,"stopReason".into(),heap.string("aborted".into()).unwrap(),factory.runtime()).unwrap();
            let getter=heap.fresh_compiled_function(&FORBIDDEN_ABORT_CONTENT,&[]).unwrap();
            let ara_rpc::value_heap::ValueSnapshot::Function(getter)=heap.snapshot(&getter).unwrap() else {panic!("compiled getter")};
            heap.define_own(&response,"content".into(),ara_rpc::value_heap::PropertyDescriptor::Accessor {get:Some(getter),set:None,enumerable:true,configurable:true},factory.runtime()).unwrap();
            vec![response]
        } else {
            let reset=chrono::Utc::now()+chrono::Duration::hours(8)+chrono::Duration::seconds(3);
            vec![root(&factory,Some(&format!("429 rate limit will reset at {}",reset.format("%Y-%m-%d %H:%M:%S")))),root(&factory,None)]
        };
        let provider=Arc::new(Provider {roots,loop_attempts:vec![],calls:AtomicUsize::new(0),contexts:Mutex::new(vec![])});
        let resources=Arc::new(Resources::default());let records=Arc::new(Mutex::new(Vec::new()));
        let usage:UsageCallback={let records=records.clone();Arc::new(move|observation|{records.lock().unwrap().push(observation.record());Box::pin(async{Ok(())})})};
        let completion=Completion {provider:provider.clone(),usage,model:Model {id:"repair".into(),api:"openai-completions".into(),provider:"zai".into(),base_url:"http://unused.invalid".into(),reasoning:false,max_tokens:None,context_window:None,tokenizer:None}};
        let invocation=EditNativeRepairInvocation {factory:factory.clone(),bindings:bindings.clone(),module,signal:controller.signal.clone(),resources:resources.clone()};
        let result=tokio::time::timeout(Duration::from_secs(7),run.drive(completion.complete_actual(invocation,"repair original source".into(),CancellationToken::new()))).await.expect("provider-aware hint must stay below7s").unwrap();
        if mode=="aborted" {
            let Err(ValueError::Thrown(reason))=result else {panic!("expected original Error for aborted completion")};
            let message=factory.heap().get(&reason,&"message".into(),factory.runtime()).unwrap();
            let ara_rpc::value_heap::ValueSnapshot::String(message)=factory.heap().snapshot(&message).unwrap() else {panic!("Error.message")};
            assert_eq!(message.to_utf8().unwrap(),"auto-repair aborted");assert_eq!(provider.calls.load(Ordering::SeqCst),1);
        } else {assert_eq!(result.unwrap(),"fixed source");assert_eq!(provider.calls.load(Ordering::SeqCst),2);}
        let count=provider.calls.load(Ordering::SeqCst);assert_eq!(records.lock().unwrap().len(),count);
        for receipt in resources.receipts.lock().unwrap().clone() {assert!(tokio::time::timeout(Duration::from_secs(2),receipt.wait_settled()).await.unwrap().is_empty());}
        assert_eq!(resources.uncertain.load(Ordering::SeqCst),0);
    }
}
