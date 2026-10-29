//! G09 actual NativeRun lang Get ordering and thrown-identity source bank.
//! Reuses the existing admitted NativeRun harness, never a surrogate evaluator.
use super::*;
use crate::{native_factory::{NativeFactoryImports,PreparedNativeSession},native_realm::NativeRealm,native_run::NativeRun};
use ara_agent::AgentMessage;
use std::{path::PathBuf,sync::atomic::{AtomicUsize,Ordering}};
use tokio::sync::Notify;
use std::time::Duration;
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

struct ScopePort {path:PathBuf,started:Arc<Notify>,release:Arc<Notify>}
#[async_trait]
impl ara_tools::ContentUriPort for ScopePort {
    fn route(&self,url:&str)->ara_tools::ContentUriRoute {
        if url=="artifact://scope" {ara_tools::ContentUriRoute::Registered {writable:false}} else {ara_tools::ContentUriRoute::Unregistered}
    }
    async fn search_path_only(&self,_:&str,cancel:CancellationToken)->Result<Option<PathBuf>,ToolError> {
        self.started.notify_one();
        tokio::select! {_=self.release.notified()=>Ok(Some(self.path.clone())),_=cancel.cancelled()=>Err(ToolError("Aborted: Cancelled".into()))}
    }
    async fn read(&self,_:&str,_:CancellationToken)->Result<ara_tools::UriResource,ToolError> {panic!("scope is pathOnly")}
    async fn write(&self,_:&str,_:&str,_:CancellationToken)->Result<(),ToolError> {panic!("grep cannot write")}
}
struct RetryHost(AtomicUsize);
#[async_trait]
impl ara_tools::grammars::GrammarHostPort for RetryHost {
    async fn ensure_grammars(&self,languages:&[String],_:CancellationToken)->Result<bool,ToolError> {
        assert_eq!(languages,["hcl"]);self.0.fetch_add(1,Ordering::SeqCst);Ok(true)
    }
}
static LANG:CompiledFunctionSpec=CompiledFunctionSpec {
    source:"G09 original params.lang is read after scope and freshly on each search",
    name:"get lang",length:0,form:CompiledFunctionForm::Ordinary,capture_count:3,
    body:|heap,captures,_,_,effective| {
        let ValueSnapshot::Number(reads)=heap.snapshot(&heap.get(&captures[0],&"reads".into(),effective)?)? else {return Err(ValueError::InvalidNode);};
        heap.define_own_data(&captures[0],"reads".into(),heap.number(reads+1.0)?,effective)?;
        if heap.truthy(&captures[2])? {return Err(ValueError::Thrown(captures[1].clone()));}
        let mode=heap.get(&captures[0],&"rerun".into(),effective)?;
        if heap.truthy(&mode)? && reads<2.0 {heap.undefined()} else {heap.string("typescript".into())}
    },
};
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn ast_grep_native_scope_then_lang_gets_retry_and_original_throw_bank() {
    let _serial=crate::host_grammars::GRAMMAR_TEST_LOCK.lock().await;
    for scenario in ["explicit","rerun","throw"] {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join(if scenario=="rerun" {"calls.tf"} else {"calls.ts"});
        std::fs::write(&path,"foo();\n").unwrap();
        let started=Arc::new(Notify::new());let release=Arc::new(Notify::new());let cache=tempfile::tempdir().unwrap();
        ara_ast::language::grammar::set_grammar_dir(cache.path().to_owned());
        let host=Arc::new(RetryHost(AtomicUsize::new(0)));
        let context=ToolContext::new(dir.path()).with_uri_port(Arc::new(ScopePort {path:path.clone(),started:started.clone(),release:release.clone()}));
        if scenario=="rerun" {context.set_grammar_host(host.clone());}
        let realm=NativeRealm::prepare().unwrap();let values=&realm.values;let heap=values.heap();let effective=values.runtime();
        let source=BuiltinToolSource::new(values,context,&[BuiltinKind::AstGrep]).unwrap();
        let prepared=PreparedNativeSession::from_complete_bindings(realm.clone(),NativeFactoryImports {
            source,string_function:realm.globals.string.clone(),bun:realm.globals.bun.clone(),original_logger:None,
        }).unwrap();
        let state=heap.object().unwrap();heap.define_own_data(&state,"reads".into(),heap.number(0.0).unwrap(),effective).unwrap();
        heap.define_own_data(&state,"rerun".into(),heap.boolean(scenario=="rerun").unwrap(),effective).unwrap();
        let reason=heap.object().unwrap();let args=heap.import_serde(&serde_json::json!({"pat":"foo()","path":"artifact://scope"})).unwrap();
        let getter=heap.fresh_compiled_function(&LANG,&[state.clone(),reason.clone(),heap.boolean(scenario=="throw").unwrap()]).unwrap();
        let ValueSnapshot::Function(getter)=heap.snapshot(&getter).unwrap() else {panic!("getter")};
        heap.define_own(&args,"lang".into(),PropertyDescriptor::Accessor {get:Some(getter),set:None,enumerable:true,configurable:true},effective).unwrap();
        let cancel=CancellationToken::new();let driver=NativeRun::begin(prepared.clone(),cancel.clone()).unwrap();
        let (invoker,input)=request(&prepared,0,"ast_grep",args,cancel);
        let release_scope=async {
            started.notified().await;
            assert!(matches!(heap.snapshot(&heap.get(&state,&"reads".into(),effective).unwrap()).unwrap(),ValueSnapshot::Number(0.0)),"lang Get must wait for actual Host scope");
            release.notify_one();
        };
        let driven=tokio::time::timeout(Duration::from_secs(15),driver.drive(async {
            let (result,())=tokio::join!(invoker.invoke_settled(input),release_scope);result
        })).await.unwrap();
        let observed=match driven {Ok(result)=>result,Err(error)=>Err(error)};
        if scenario=="throw" {assert!(has_reason(&observed.unwrap_err(),&reason),"retain exact thrown object");}
        else {
            let output=observed.unwrap();let details=heap.get(&output,&"details".into(),effective).unwrap();
            assert_eq!(heap.export_serde(&details).unwrap()["matchCount"],1);
            assert!(matches!(heap.snapshot(&heap.get(&state,&"reads".into(),effective).unwrap()).unwrap(),ValueSnapshot::Number(3.0)),"truthy/conditional/query or missing/query/rerun Gets");
            assert_eq!(host.0.load(Ordering::SeqCst),usize::from(scenario=="rerun"));
        }
        assert_eq!(std::fs::read_to_string(path).unwrap(),"foo();\n");
    }
}
