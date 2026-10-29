//! Controlled Port assertions through the real Native Task source/action/effect
//! owner. This does not replace ordinary binary ChildHost/isolation acceptance.
#![cfg(feature = "test-fixture")]
use ara_agent::{ActualExecuteRequest,ActualProviderError,ActualToolSource,AgentMessage};
use ara_cli::{actual_tools::{BuiltinKind,BuiltinToolSource},native_factory::{NativeFactoryImports,PreparedNativeSession},native_realm::NativeRealm,native_run::NativeRun,task_contract::{TaskExecutionPort,TaskPolicy,TaskRequest,TaskResult}};
use ara_rpc::value_heap::{ValueHandle,ValueSnapshot};
use ara_tools::ToolContext;
use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc,Mutex,RwLock};
use tokio_util::sync::CancellationToken;

struct RecordingPort { policy:RwLock<TaskPolicy>, requests:Mutex<Vec<TaskRequest>> }
#[async_trait]
impl TaskExecutionPort for RecordingPort {
    fn policy(&self)->TaskPolicy{self.policy.read().unwrap().clone()}
    async fn execute(&self,request:TaskRequest,_:CancellationToken)->anyhow::Result<TaskResult>{
        self.requests.lock().unwrap().push(request);
        Ok(TaskResult{content:"controlled Task Port receipt".into(),details:json!({"controlledPort":true,"accepted":false}),is_error:false})
    }
}
fn prepare(directory:&std::path::Path)->(Arc<PreparedNativeSession>,Arc<BuiltinToolSource>,Arc<RecordingPort>){
    let realm=NativeRealm::prepare().unwrap();
    let source=BuiltinToolSource::new(&realm.values,ToolContext::new(directory.to_owned()),&[BuiltinKind::Task]).unwrap();
    let port=Arc::new(RecordingPort{policy:RwLock::new(TaskPolicy::default()),requests:Mutex::new(Vec::new())});
    let owner=source.task_session();owner.bind_session("exact-task-session").unwrap();owner.bind(port.clone()).unwrap();
    let prepared=PreparedNativeSession::from_complete_bindings(realm.clone(),NativeFactoryImports{source:source.clone(),string_function:realm.globals.string.clone(),bun:realm.globals.bun.clone(),original_logger:None}).unwrap();
    (prepared,source,port)
}
async fn invoke(prepared:&Arc<PreparedNativeSession>,source:&Arc<BuiltinToolSource>,args:ValueHandle)->Result<ValueHandle,ActualProviderError>{
    let values=prepared.values();let heap=values.heap();
    let collection=source.current_tools(values)?.unwrap();let receiver=heap.get(&collection,&"0".into(),values.runtime())?;
    let binding=source.bind_selected(&receiver,values)?;
    let call=heap.import_serde(&json!({"type":"toolCall","id":"task-native-contract","name":"task"}))?;
    heap.define_own_data(&call,"arguments".into(),args.clone(),values.runtime())?;
    let mut assistant=serde_json::to_value(ara_ai::AssistantMessage::empty("fixture","fixture","fixture")).unwrap();
    assistant["role"]="assistant".into();assistant["stopReason"]="toolUse".into();
    let root=heap.import_serde(&assistant)?;heap.define_own_data(&root,"content".into(),heap.array(vec![call.clone()])?,values.runtime())?;
    let owner=AgentMessage::from_graph(root,None)?;let cancel=CancellationToken::new();
    let driver=NativeRun::begin(prepared.clone(),cancel.clone())?;
    let context=heap.object()?;prepared.require_tool_hooks()?.bind_run_context(values,&context)?;
    let request=ActualExecuteRequest{callable:heap.get(&receiver,&"execute".into(),values.runtime())?,receiver,arguments_slot:owner.current_arguments_slot(&call)?,owner,tool_call:call.clone(),call_id:"task-native-contract".into(),args:args.clone(),values:values.clone(),cancel,on_update:Arc::new(|_|Ok(())),tool_context:context};
    let output=tokio::time::timeout(std::time::Duration::from_secs(10),driver.drive(binding.capability.actual_invoker().unwrap().invoke_settled(request))).await.expect("Native Task contract timeout")??;
    assert!(heap.get(&call,&"arguments".into(),values.runtime())?.same_identity(&args),"Native execution must preserve its original args slot");
    Ok(output)
}
#[tokio::test]
async fn native_task_schema_shape_identity_and_real_grant_bank(){
    let directory=tempfile::tempdir().unwrap();let(prepared,source,port)=prepare(directory.path());
    let values=prepared.values();let heap=values.heap();let collection=source.current_tools(values).unwrap().unwrap();
    let tool=heap.get(&collection,&"0".into(),values.runtime()).unwrap();
    assert!(matches!(heap.snapshot(&heap.get(&tool,&"strict".into(),values.runtime()).unwrap()).unwrap(),ValueSnapshot::Boolean(false)));
    assert!(matches!(heap.snapshot(&heap.get(&tool,&"lenientArgValidation".into(),values.runtime()).unwrap()).unwrap(),ValueSnapshot::Boolean(true)));
    // Actual metadata Gets follow live Host settings on the same source.
    for batch in [false,true]{for isolated in [false,true]{for effort in [false,true]{for structured in [false,true]{
        let mut policy=port.policy.write().unwrap();policy.batch_enabled=batch;policy.isolation_enabled=isolated;policy.effort_enabled=effort;policy.structured=structured;drop(policy);
        let params=heap.get(&tool,&"parameters".into(),values.runtime()).unwrap();
        let properties=heap.get(&params,&"properties".into(),values.runtime()).unwrap();
        let properties=if batch{let tasks=heap.get(&properties,&"tasks".into(),values.runtime()).unwrap();let item=heap.get(&tasks,&"items".into(),values.runtime()).unwrap();heap.get(&item,&"properties".into(),values.runtime()).unwrap()}else{properties};
        for(key,expected)in[("isolated",isolated),("effort",effort),("outputSchema",structured)]{
            assert_eq!(!matches!(heap.snapshot(&heap.get(&properties,&key.into(),values.runtime()).unwrap()).unwrap(),ValueSnapshot::Undefined),expected,"dynamic field {key}");
        }
    }}}}
    *port.policy.write().unwrap()=TaskPolicy{batch_enabled:false,structured:true,..TaskPolicy::default()};
    let bad=heap.import_serde(&json!({"context":"wrong shape","tasks":[{"task":"never launch"}]})).unwrap();
    let error=invoke(&prepared,&source,bad).await.unwrap_err().to_string();assert!(error.contains("task.batch is disabled"));assert!(port.requests.lock().unwrap().is_empty());
    let stale=heap.import_serde(&json!({"task":"never launch","schema":{}})).unwrap();assert!(invoke(&prepared,&source,stale).await.unwrap_err().to_string().contains("outputSchema"));
    source.set_granted(BuiltinKind::Task,false).unwrap();
    let denied=heap.import_serde(&json!({"task":"denied","solutionSpace":"test"})).unwrap();assert!(invoke(&prepared,&source,denied).await.is_err());assert!(port.requests.lock().unwrap().is_empty());
    source.set_granted(BuiltinKind::Task,true).unwrap();
    let args=heap.import_serde(&json!({"task":r#"# Role\nReturn \"ok\""#,"solutionSpace":"read only","isolated":false,"effort":"hi","outputSchema":null})).unwrap();
    // An ignored cyclic sibling proves execution never exports/reimports the
    // whole original args object merely to select the Task Host fields.
    heap.define_own_data(&args,"ignoredCycle".into(),args.clone(),values.runtime()).unwrap();
    let output=invoke(&prepared,&source,args).await.unwrap();
    let details=heap.get(&output,&"details".into(),values.runtime()).unwrap();assert!(matches!(heap.snapshot(&heap.get(&details,&"accepted".into(),values.runtime()).unwrap()).unwrap(),ValueSnapshot::Boolean(false)));
    let requests=port.requests.lock().unwrap();assert_eq!(requests.len(),1);assert_eq!(requests[0].items[0].agent,"task");assert_eq!(requests[0].items[0].task,"# Role\nReturn \"ok\"");assert_eq!(requests[0].items[0].isolated,Some(false));assert_eq!(requests[0].items[0].output_schema,Some(serde_json::Value::Null));drop(requests);
    port.policy.write().unwrap().batch_enabled=true;
    for invalid in [json!({"context":"shared","tasks":[]}),json!({"context":"shared","task":"mixed","tasks":[{"task":"a"}]}),json!({"context":"","tasks":[{"task":"a"}]}),json!({"context":"shared","tasks":[{"task":"a","name":"A"},{"task":"b","name":"a"}]}),json!({"context":"shared","tasks":[{"task":"a","effort":"max"}]})]{
        let args=heap.import_serde(&invalid).unwrap();assert!(invoke(&prepared,&source,args).await.is_err());assert_eq!(port.requests.lock().unwrap().len(),1);
    }
    let batch=heap.import_serde(&json!({"context":"shared","isolated":true,"tasks":[{"name":"a","task":"first"},{"name":"b","task":"second","isolated":false,"agent":"scout"}]})).unwrap();
    invoke(&prepared,&source,batch).await.unwrap();
    let requests=port.requests.lock().unwrap();assert_eq!(requests[1].context.as_deref(),Some("shared"));assert_eq!(requests[1].items.len(),2);assert_eq!(requests[1].items[0].isolated,Some(true));assert_eq!(requests[1].items[1].isolated,Some(false));assert_eq!(requests[1].items[1].agent,"scout");drop(requests);
    let other=NativeRealm::prepare().unwrap();assert!(source.task_session().check_values(&other.values).is_err());assert!(source.task_session().bind_session("foreign-task-session").is_err());
    source.task_session().close();assert!(source.task_session().bind(port).is_err());
}
#[test]
fn task_policy_depth_schema_modes_and_selected_prose_bank(){
    let mut policy=TaskPolicy::default();assert!(policy.can_spawn_at_depth(1));assert!(!policy.can_spawn_at_depth(2));policy.maximum_depth=-1.0;assert!(policy.can_spawn_at_depth(100));
    policy.isolation_enabled=true;policy.plan_mode=true;policy.batch_enabled=false;policy.eval_tools_enabled=false;
    let properties=&policy.parameters()["properties"];assert!(properties.get("isolated").is_none());assert!(properties.get("tools").is_none());
    assert_eq!(ara_cli::actual_tools::task_native::repair_prose(r#"# Role\nReturn \"ok\""#.into()),"# Role\nReturn \"ok\"");
    assert_eq!(ara_cli::actual_tools::task_native::repair_prose(r"split on \n".into()),r"split on \n");
}
