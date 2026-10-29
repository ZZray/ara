//! Complete Edit G01-G04 source bank: actual original Graph and physical bytes.
//! Candidate test source; no execution/typecheck is claimed.
use super::*;
use crate::{native_factory::{NativeFactoryImports,PreparedNativeSession},native_realm::NativeRealm,native_run::NativeRun};
use ara_agent::AgentMessage;
use ara_tools::edit_runtime_policy::EditRuntimeSnapshot;
use std::time::Duration;
use serde_json::json;
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
        heap.import_serde(&serde_json::json!({"type":"toolCall","id":"edit-upgrade","name":name})).unwrap();
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
            call_id: "edit-upgrade".into(),
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

struct MetadataRuntime {source:Arc<BuiltinToolSource>,base:Arc<dyn ValueRuntime>}
impl ValueRuntime for MetadataRuntime {
    fn resolve(&self,request:RuntimeRequest<'_>)->Result<RuntimeReply,ValueError> {
        match self.source.resolve_context_signal(&request) {Some(reply)=>reply,None=>self.base.resolve(request)}
    }
    fn resolve_effective(&self,request:RuntimeRequest<'_>,effective:&dyn ValueRuntime)->Result<RuntimeReply,ValueError> {
        match self.source.resolve_context_signal(&request) {Some(reply)=>reply,None=>self.base.resolve_effective(request,effective)}
    }
}
static JSON:CompiledFunctionSpec=CompiledFunctionSpec {
    source:"G01 original toJSON observes end args and effective execute args separately",
    name:"toJSON",length:1,form:CompiledFunctionForm::Ordinary,capture_count:2,
    body:|heap,captures,_,_,effective| {
        let count=heap.get(&captures[0],&"reads".into(),effective)?;
        let count=heap.to_number(&count,effective)?;
        heap.define_own_data(&captures[0],"reads".into(),heap.number(count+1.0)?,effective)?;
        if heap.truthy(&heap.get(&captures[0],&"throw".into(),effective)?)? {return Err(ValueError::Thrown(captures[1].clone()));}
        heap.get(&captures[0],&"payload".into(),effective)
    },
};
fn control(realm:&Arc<NativeRealm>,payload:serde_json::Value)->(ValueHandle,ValueHandle,ValueHandle) {
    let heap=realm.values.heap();let runtime=realm.values.runtime();let state=heap.object().unwrap();let reason=heap.object().unwrap();
    for (key,value) in [("reads",heap.number(0.0).unwrap()),("throw",heap.boolean(false).unwrap()),("payload",heap.import_serde(&payload).unwrap())] {
        heap.define_own_data(&state,key.into(),value,runtime).unwrap();
    }
    let args=heap.object().unwrap();heap.define_own_data(&args,"toJSON".into(),heap.fresh_compiled_function(&JSON,&[state.clone(),reason.clone()]).unwrap(),runtime).unwrap();
    (state,args,reason)
}
fn setup(realm:&Arc<NativeRealm>,context:ToolContext)->(Arc<PreparedNativeSession>,Arc<BuiltinToolSource>) {
    let source=BuiltinToolSource::new(&realm.values,context,&[BuiltinKind::Edit]).unwrap();
    let prepared=PreparedNativeSession::from_complete_bindings(realm.clone(),NativeFactoryImports {
        source:source.clone(),string_function:realm.globals.string.clone(),bun:realm.globals.bun.clone(),original_logger:None,
    }).unwrap();(prepared,source)
}
async fn invoke(prepared:&Arc<PreparedNativeSession>,args:ValueHandle)->Result<ValueHandle,ActualProviderError> {
    let cancel=CancellationToken::new();let driver=NativeRun::begin(prepared.clone(),cancel.clone()).unwrap();
    let (invoker,request)=request(prepared,0,"edit",args,cancel);
    let outcome=tokio::time::timeout(Duration::from_secs(8),driver.drive(invoker.invoke_settled(request))).await.unwrap();
    match outcome {Ok(result)=>result,Err(error)=>Err(error)}
}
fn is_error(values:&SessionValues,result:&ValueHandle)->bool {
    values.heap().truthy(&values.heap().get(result,&"isError".into(),values.runtime()).unwrap()).unwrap()
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn effective_streamed_args_original_json_revised_payload_and_throw_bank() {
    for scenario in ["same","revised","throw"] {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("bytes.txt");std::fs::write(&path,"old\n").unwrap();
        let realm=NativeRealm::prepare().unwrap();
        let live=Arc::new(Mutex::new(EditRuntimeSnapshot {mode:ara_edit::EditMode::Replace,display_mode:ara_edit::EditMode::Replace,settings:ara_tools::edit::EditSettings::default(),compact_prompt:false}));
        let current=live.clone();let context=ToolContext::new(dir.path()).with_edit(ara_edit::EditMode::Replace,true)
            .with_edit_runtime_policy(Arc::new(move ||Ok(*current.lock().unwrap())));
        let (prepared,source)=setup(&realm,context);let values=prepared.values();let heap=values.heap();let runtime=values.runtime();
        let preview=json!({"path":"bytes.txt","old_string":"old","new_string":"preview"});
        let (state,args,reason)=control(&realm,preview.clone());
        let Kernel::Edit(tool)=&source.members[0].kernel else {panic!("Edit")};
        let mut receiver=tool.open_arg_stream(ara_agent::ToolArgStreamInit {tool_call_id:"edit-upgrade".into(),tool_name:"edit".into(),custom_wire_name:None,emit:Arc::new(|_|{})}).unwrap().unwrap();
        receiver.push(&preview.to_string()).unwrap();receiver.end_actual(&args,values).unwrap();
        assert_eq!(heap.to_number(&heap.get(&state,&"reads".into(),runtime).unwrap(),runtime).unwrap(),1.0);
        if scenario=="revised" {heap.define_own_data(&state,"payload".into(),heap.import_serde(&json!({"path":"bytes.txt","old_string":"old","new_string":"effective"})).unwrap(),runtime).unwrap();}
        if scenario=="throw" {heap.define_own_data(&state,"throw".into(),heap.boolean(true).unwrap(),runtime).unwrap();}
        let args=if scenario=="same" {
            // Text equality, never identity: a new effective args object with
            // the same serialization must keep the already-open Replace
            // session, even when the live mode for a NEW session is now Patch.
            let effective=heap.object().unwrap();
            heap.define_own_data(&effective,"toJSON".into(),heap.fresh_compiled_function(&JSON,&[state.clone(),reason.clone()]).unwrap(),runtime).unwrap();
            assert!(!effective.same_identity(&args));
            let mut mode=live.lock().unwrap();mode.mode=ara_edit::EditMode::Patch;mode.display_mode=ara_edit::EditMode::Patch;
            effective
        } else {args};
        let result=invoke(&prepared,args).await;
        assert_eq!(heap.to_number(&heap.get(&state,&"reads".into(),runtime).unwrap(),runtime).unwrap(),2.0,"effective execute always stringifies even with a completed stream");
        if scenario=="throw" {assert!(has_reason(&result.unwrap_err(),&reason));assert_eq!(std::fs::read_to_string(path).unwrap(),"old\n");}
        else {assert!(!is_error(values,&result.unwrap()));assert_eq!(std::fs::read_to_string(path).unwrap(),if scenario=="same" {"preview\n"}else{"effective\n"});}
        receiver.cancel().unwrap();
    }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn live_edit_original_metadata_matcher_cache_and_policy_effects_bank() {
    let dir=tempfile::tempdir().unwrap();std::fs::write(dir.path().join("white.txt"),"alpha  beta\n").unwrap();
    let state=Arc::new(Mutex::new(EditRuntimeSnapshot {mode:ara_edit::EditMode::Replace,display_mode:ara_edit::EditMode::Replace,settings:ara_tools::edit::EditSettings::default(),compact_prompt:false}));
    state.lock().unwrap().settings.allow_fuzzy=false;
    let read=state.clone();let context=ToolContext::new(dir.path()).with_edit(ara_edit::EditMode::Replace,true)
        .with_edit_runtime_policy(Arc::new(move ||Ok(*read.lock().unwrap())));
    let realm=NativeRealm::prepare().unwrap();let (prepared,source)=setup(&realm,context.clone());let values=prepared.values();let heap=values.heap();
    let runtime=MetadataRuntime {source:source.clone(),base:realm.runtime.clone()};let selected=&source.members[0].source;
    assert!(matches!(heap.snapshot(&heap.get(selected,&"mode".into(),&runtime).unwrap()).unwrap(),ValueSnapshot::String(text) if text.equals_ascii("replace")));
    let metadata = |mode: ara_edit::EditMode| {
        let examples=heap.get(selected,&"examples".into(),&runtime).unwrap();
        match ara_tools::edit::examples_for_mode(mode) {
            Some(expected)=>assert_eq!(heap.export_serde(&examples).unwrap(),serde_json::to_value(expected).unwrap()),
            None=>assert!(matches!(heap.snapshot(&examples).unwrap(),ValueSnapshot::Undefined)),
        }
        let format=heap.get(selected,&"customFormat".into(),&runtime).unwrap();
        match ara_edit::grammar(mode) {
            Some(definition)=>assert_eq!(heap.export_serde(&format).unwrap(),json!({"syntax":"lark","definition":definition})),
            None=>assert!(matches!(heap.snapshot(&format).unwrap(),ValueSnapshot::Undefined)),
        }
        let wire=heap.get(selected,&"customWireName".into(),&runtime).unwrap();
        if mode==ara_edit::EditMode::ApplyPatch {assert!(matches!(heap.snapshot(&wire).unwrap(),ValueSnapshot::String(text) if text.equals_ascii("apply_patch")));}
        else {assert!(matches!(heap.snapshot(&wire).unwrap(),ValueSnapshot::Undefined));}
    };
    metadata(ara_edit::EditMode::Replace);
    let full=heap.get(selected,&"description".into(),&runtime).unwrap();
    assert!(matches!(heap.snapshot(&full).unwrap(),ValueSnapshot::String(text) if text.to_utf8().unwrap()==ara_edit::description(ara_edit::EditMode::Replace)));
    let (json_state,args,_)=control(&realm,json!({"path":"white.txt","old_string":"alpha beta","new_string":"gamma"}));
    let matcher=heap.get(selected,&"matcherEntries".into(),&runtime).unwrap();
    let first=heap.call(&matcher,selected,&[args.clone()],&runtime).unwrap();let second=heap.call(&matcher,selected,&[args.clone()],&runtime).unwrap();
    assert!(first.same_identity(&second),"same-mode original cache returns the same inspection entries Array");
    assert_eq!(heap.to_number(&heap.get(&json_state,&"reads".into(),&runtime).unwrap(),&runtime).unwrap(),1.0);
    let rejected=invoke(&prepared,heap.import_serde(&json!({"path":"white.txt","old_string":"alpha beta","new_string":"gamma"})).unwrap()).await.unwrap();
    assert!(is_error(values,&rejected));assert_eq!(std::fs::read_to_string(dir.path().join("white.txt")).unwrap(),"alpha  beta\n");
    state.lock().unwrap().settings.allow_fuzzy=true;
    let accepted=invoke(&prepared,heap.import_serde(&json!({"path":"white.txt","old_string":"alpha beta","new_string":"gamma"})).unwrap()).await.unwrap();
    assert!(!is_error(values,&accepted));assert_eq!(std::fs::read_to_string(dir.path().join("white.txt")).unwrap(),"gamma\n");
    {let mut current=state.lock().unwrap();current.mode=ara_edit::EditMode::Patch;current.display_mode=ara_edit::EditMode::Patch;}
    heap.define_own_data(&json_state,"payload".into(),heap.import_serde(&json!({"path":"white.txt","edits":[{"diff":"@@\n-gamma\n+delta\n"}]})).unwrap(),&runtime).unwrap();
    let after=heap.call(&matcher,selected,&[args],&runtime).unwrap();assert!(!first.same_identity(&after),"mode change rebuilds inspection");
    assert_eq!(heap.to_number(&heap.get(&json_state,&"reads".into(),&runtime).unwrap(),&runtime).unwrap(),2.0);
    metadata(ara_edit::EditMode::Patch);
    let parameters=heap.get(selected,&"parameters".into(),&runtime).unwrap();assert!(heap.export_serde(&parameters).unwrap()["properties"].get("edits").is_some());
    let patched=invoke(&prepared,heap.import_serde(&json!({"path":"white.txt","edits":[{"diff":"@@\n-gamma\n+delta\n"}]})).unwrap()).await.unwrap();
    assert!(!is_error(values,&patched));assert_eq!(std::fs::read_to_string(dir.path().join("white.txt")).unwrap(),"delta\n");
    {let mut current=state.lock().unwrap();current.mode=ara_edit::EditMode::Hashline;current.display_mode=ara_edit::EditMode::Hashline;current.compact_prompt=true;}
    metadata(ara_edit::EditMode::Hashline);
    {let mut current=state.lock().unwrap();current.mode=ara_edit::EditMode::ApplyPatch;current.display_mode=ara_edit::EditMode::ApplyPatch;}
    metadata(ara_edit::EditMode::ApplyPatch);
    {let mut current=state.lock().unwrap();current.mode=ara_edit::EditMode::Hashline;current.display_mode=ara_edit::EditMode::Hashline;}
    let compact=heap.get(selected,&"description".into(),&runtime).unwrap();
    assert!(matches!(heap.snapshot(&compact).unwrap(),ValueSnapshot::String(text) if text.to_utf8().unwrap()==include_str!("../../../ara-tools/src/edit_hashline_compact.md").trim()));
    assert!(heap.export_serde(&heap.get(selected,&"parameters".into(),&runtime).unwrap()).unwrap()["properties"].get("input").is_some());
    state.lock().unwrap().compact_prompt=false;
    assert!(matches!(heap.snapshot(&heap.get(selected,&"description".into(),&runtime).unwrap()).unwrap(),ValueSnapshot::String(text) if text.to_utf8().unwrap()==ara_edit::description(ara_edit::EditMode::Hashline)));
    // Target Read accepts inline selectors, not a legacy `limit` argument.
    // Raw bounded reads add no context and retain full snapshot + exact seen lines.
    std::fs::write(dir.path().join("seen.txt"),"one\ntwo\nthree\n").unwrap();
    let read = ara_tools::read::ReadTool {ctx:context.clone()}.execute("seen-read",json!({"path":"seen.txt:raw:1-1"}).as_object().unwrap().clone(),CancellationToken::new(),Arc::new(|_|{})).await.unwrap();
    let shown = read.content.iter().filter_map(|block| match block { ara_ai::UserBlock::Text(text) => Some(text.text.as_str()), _ => None }).collect::<Vec<_>>().join("\n");
    assert_eq!(shown.lines().next(), Some("one"));
    assert!(!shown.lines().any(|line| line == "two" || line == "three"));
    let snapshot = context.edit_store.head(&ara_edit::path_policy::canonical_key(&dir.path().join("seen.txt"))).unwrap();
    assert_eq!(snapshot.text.as_ref(), "one\ntwo\nthree\n");
    assert_eq!(snapshot.seen_lines.as_ref().unwrap().iter().copied().collect::<Vec<_>>(), vec![1]);
    let tag = snapshot.hash;
    let edit=json!({"input":format!("[seen.txt#{tag}]\nPUT 2.=2:\n+changed\n")});
    let blocked=invoke(&prepared,heap.import_serde(&edit).unwrap()).await.unwrap();assert!(is_error(values,&blocked));
    assert_eq!(std::fs::read_to_string(dir.path().join("seen.txt")).unwrap(),"one\ntwo\nthree\n");
    state.lock().unwrap().settings.enforce_seen_lines=false;
    let allowed=invoke(&prepared,heap.import_serde(&edit).unwrap()).await.unwrap();assert!(!is_error(values,&allowed));
    assert_eq!(std::fs::read_to_string(dir.path().join("seen.txt")).unwrap(),"one\nchanged\nthree\n");
}
