//! Actual Native Task, fixed OMP b91db66e task/index.ts:215-349,558-686 (MIT).
//! Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
use super::*;
use crate::task_contract::{TaskExecutionPort, TaskItem, TaskPolicy, TaskRequest};

pub struct TaskTool;
#[async_trait]
impl AgentTool for TaskTool {
    fn definition(&self) -> Tool { definition(&TaskPolicy::default()) }
    fn lenient_argument_validation(&self) -> bool { true }
    fn concurrency(&self, _: &JsonObject) -> Concurrency { Concurrency::Shared }
    async fn execute(&self, _: &str, _: JsonObject, _: CancellationToken, _: UpdateFn) -> Result<ToolOutput, ToolError> {
        Err(ToolError("Task requires its actual Native Session and Host execution owner".into()))
    }
}
fn definition(policy: &TaskPolicy) -> Tool {
    Tool { name: "task".into(), description: "Spawn subagents to complete delegated tasks. Give each agent complete, self-contained instructions and solutionSpace; batch calls include shared context and tasks. Isolated changes follow the current Host merge and apply policy.".into(), parameters: policy.parameters(), strict: Some(false), ..Default::default() }
}
struct State { session: Option<String>, port: Option<Arc<dyn TaskExecutionPort>>, closed: bool }
pub struct SessionTaskOwner {
    values: Weak<SessionValues>,
    state: Mutex<State>,
    closed: CancellationToken,
}
impl SessionTaskOwner {
    pub fn new(values: &Arc<SessionValues>) -> Arc<Self> {
        Arc::new(Self { values: Arc::downgrade(values), state: Mutex::new(State { session: None, port: None, closed: false }), closed: CancellationToken::new() })
    }
    pub fn check_values(&self, values: &Arc<SessionValues>) -> anyhow::Result<()> {
        anyhow::ensure!(self.values.upgrade().is_some_and(|old|Arc::ptr_eq(&old,values)), "Task belongs to another logical Session");
        Ok(())
    }
    pub fn bind_session(&self, id: &str) -> anyhow::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e|e.into_inner());
        anyhow::ensure!(!state.closed && !id.is_empty(), "Task Session is closed or lacks identity");
        anyhow::ensure!(state.session.as_deref().is_none_or(|old|old==id), "Task cannot replace its original logical Session");
        state.session = Some(id.into()); Ok(())
    }
    pub fn bind(&self, port: Arc<dyn TaskExecutionPort>) -> anyhow::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e|e.into_inner());
        anyhow::ensure!(!state.closed && state.session.is_some() && self.values.upgrade().is_some(), "Task requires a live exact logical Session before Host binding");
        state.port=Some(port); Ok(())
    }
    pub fn policy(&self) -> TaskPolicy {
        let port=self.state.lock().unwrap_or_else(|e|e.into_inner()).port.clone();
        port.map(|port|port.policy()).unwrap_or_default()
    }
    pub fn definition(&self) -> Tool { definition(&self.policy()) }
    pub fn close(&self) {
        self.closed.cancel();
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        state.closed=true; state.port=None;
    }
    pub async fn execute(&self, request: TaskRequest, cancel: CancellationToken) -> Result<ToolOutput, ToolError> {
        let port={let state=self.state.lock().unwrap_or_else(|e|e.into_inner());
            if state.closed || self.values.upgrade().is_none() { return Err(ToolError("Task Session closed or disposed".into())); }
            state.port.clone().ok_or_else(||ToolError("Task Host execution port is unbound".into()))?};
        if cancel.is_cancelled() || self.closed.is_cancelled() { return Err(ToolError("Aborted: Cancelled".into())); }
        let stop=cancel.child_token();
        // Propagate close without dropping a possibly-effectful Host future.
        // The Host remains responsible for stopping/settling its real writers.
        let operation=port.execute(request,stop.clone()); tokio::pin!(operation);
        let result=tokio::select! { biased;
            result=&mut operation=>result,
            _=self.closed.cancelled()=>{stop.cancel();operation.await},
        };
        result.map(|result|result.into_tool_output()).map_err(|error|ToolError(error.to_string()))
    }
}

fn get(value:&ValueHandle,key:&str,runtime:&dyn ValueRuntime)->Result<ValueHandle,ActualProviderError>{Ok(value.heap().get(value,&key.into(),runtime)?)}
fn string(value:&ValueHandle)->Result<Option<String>,ActualProviderError>{
    Ok(match value.heap().snapshot(value)? {ValueSnapshot::String(value)=>Some(value.to_utf8().map_err(|_|ActualProviderError::Unsupported("Task Host string requires valid UTF-8"))?),_=>None})
}
fn field(value:&ValueHandle,key:&str,runtime:&dyn ValueRuntime)->Result<Option<String>,ActualProviderError>{string(&get(value,key,runtime)?)}
fn present(value:&ValueHandle,key:&str,runtime:&dyn ValueRuntime)->Result<bool,ActualProviderError>{Ok(!matches!(value.heap().snapshot(&get(value,key,runtime)?)?,ValueSnapshot::Undefined))}
fn fault(message:impl Into<String>)->ActualProviderError{ActualProviderError::Callback(message.into())}

/// Selected prose repair only. Identifier/code-bearing fields remain literal.
pub fn repair_prose(value:String)->String {
    let bytes=value.as_bytes(); let mut count=0; let mut structural=false; let mut i=0;
    while i<bytes.len(){if bytes[i]==b'\\'{count+=1;if let Some(next)=bytes.get(i+1){structural|=matches!(next,b'"'|b'\\'|b'/')||*next==b'u'&&bytes.get(i+2..i+6).is_some_and(|v|v.iter().all(u8::is_ascii_hexdigit));}i+=1;}i+=1;}
    if !structural&&count<2{return value;}
    serde_json::from_str::<String>(&format!("\"{value}\"")).unwrap_or(value)
}

/// Reads original graph operands using their effective runtime. No JSON export
/// or reconstruction of the whole original call/args is permitted here.
pub(super) fn prepare(owner:&SessionTaskOwner,args:&ValueHandle,runtime:&dyn ValueRuntime)->Result<TaskRequest,ActualProviderError>{
    let heap=args.heap();let policy=owner.policy();
    if !matches!(heap.kind(args)?,ara_rpc::value_heap::ValueKind::Object|ara_rpc::value_heap::ValueKind::Array){return Err(fault("Task arguments must be an object"));}
    if heap.own_descriptor(args,&"schema".into(),runtime)?.is_some(){return Err(fault("The task tool uses `outputSchema`; rename the stale `schema` field."));}
    if !policy.batch_enabled&&(present(args,"tasks",runtime)?||present(args,"context",runtime)?){return Err(fault("task.batch is disabled, so the task tool does not accept `tasks` or `context`. Spawn one agent per call with `task`, or enable the task.batch setting."));}
    let task=field(args,"task",runtime)?;
    let tasks=get(args,"tasks",runtime)?;
    let batch=policy.batch_enabled&&!matches!(heap.snapshot(&tasks)?,ValueSnapshot::Undefined);
    let mut context=field(args,"context",runtime)?.map(repair_prose);
    let mut rows=Vec::new();
    if batch {
        let ValueSnapshot::Array=heap.snapshot(&tasks)? else{return Err(fault("Missing `tasks`. Provide at least one task item ({ name?, agent?, task })."));};
        // Native arrays can carry accessors; Gets remain in the actual realm.
        let length=get(&tasks,"length",runtime)?;
        let ValueSnapshot::Number(length)=heap.snapshot(&length)? else{return Err(fault("Task tasks length is not a number"));};
        if length<1.0{return Err(fault("Missing `tasks`. Provide at least one task item ({ name?, agent?, task })."));}
        if task.as_ref().is_some_and(|task|!task.trim().is_empty()){return Err(fault("Top-level `task` is not part of the batch shape. Put the work in `tasks[]` items."));}
        for index in 0..length as usize {rows.push(get(&tasks,&index.to_string(),runtime)?);}
        if context.as_ref().is_none_or(|context|context.trim().is_empty()){return Err(fault("Missing `context`. Provide the shared background for this batch — goal, constraints, and any contract the tasks share."));}
    }else{rows.push(args.clone());if task.as_ref().is_none_or(|task|task.trim().is_empty()){return Err(fault(if policy.batch_enabled{"Missing `tasks`. Provide a `tasks` array (one subagent per item) with a shared `context`."}else{"Missing `task`. Provide complete, self-contained instructions for the agent."}));}if !policy.batch_enabled{context=None;}}
    let mut names=HashMap::new();let mut items=Vec::new();
    let top_isolated=get(args,"isolated",runtime)?;
    for (index,row) in rows.iter().enumerate(){
        if !matches!(heap.kind(row)?,ara_rpc::value_heap::ValueKind::Object|ara_rpc::value_heap::ValueKind::Array){return Err(fault(format!("Task {} is missing `task`. Every task needs complete, self-contained instructions.",index+1)));}
        let name=field(row,"name",runtime)?;
        let task=field(row,"task",runtime)?.map(repair_prose).filter(|task|!task.trim().is_empty()).ok_or_else(||fault(format!("Task {} is missing `task`. Every task needs complete, self-contained instructions.",index+1)))?;
        let agent=field(row,"agent",runtime)?.map(|v|v.trim().to_owned()).filter(|v|!v.is_empty()).unwrap_or_else(||policy.default_agent.clone());
        let effort_value=get(row,"effort",runtime)?;let effort=string(&effort_value)?;
        if !matches!(heap.snapshot(&effort_value)?,ValueSnapshot::Undefined)&&!effort.as_ref().is_some_and(|v|matches!(v.as_str(),"lo"|"med"|"hi")){return Err(fault(format!("Task {} has an invalid `effort` value. Use \"lo\", \"med\", or \"hi\".",index+1)));}
        if batch {if let Some(name)=name.as_ref().map(|v|v.trim()).filter(|v|!v.is_empty()){if let Some(old)=names.insert(name.to_lowercase(),name.to_owned()){return Err(fault(format!("Duplicate task name `{old}` / `{name}`. Provided names must be unique within a call (case-insensitive).")));}}}
        let isolated=get(row,"isolated",runtime)?;
        let isolated=if matches!(heap.snapshot(&isolated)?,ValueSnapshot::Undefined){&top_isolated}else{&isolated};
        let isolated=match heap.snapshot(isolated)?{ValueSnapshot::Undefined=>None,ValueSnapshot::Boolean(value)=>Some(value),_=>return Err(fault("Task `isolated` must be a boolean"))};
        let schema_mode=if policy.structured{field(row,"schemaMode",runtime)?}else{None};
        if schema_mode.as_ref().is_some_and(|v|!matches!(v.as_str(),"strict"|"permissive")){return Err(fault("Task schemaMode must be strict or permissive"));}
        let output_schema=if policy.structured&&present(row,"outputSchema",runtime)?{
            let value=get(row,"outputSchema",runtime)?;
            if !matches!(heap.kind(&value)?,ara_rpc::value_heap::ValueKind::Object|ara_rpc::value_heap::ValueKind::Boolean|ara_rpc::value_heap::ValueKind::String|ara_rpc::value_heap::ValueKind::Null){return Err(fault("Task outputSchema must be an object, boolean, string or null"));}
            let selected=heap.stringify(&value,StringifyMode::Plain,None,runtime)?.ok_or_else(||fault("Task outputSchema is not serializable"))?;
            Some(serde_json::from_str(&selected.to_utf8().map_err(|_|fault("Task outputSchema UTF-8 boundary"))?).map_err(|error|fault(error.to_string()))?)
        }else{None};
        let tools=if policy.eval_tools_enabled&&present(row,"tools",runtime)?{
            let tools=get(row,"tools",runtime)?;
            if !matches!(heap.kind(&tools)?,ara_rpc::value_heap::ValueKind::Array){return Err(fault("Task tools must be a string array"));}
            let ValueSnapshot::Number(length)=heap.snapshot(&get(&tools,"length",runtime)?)?else{return Err(fault("Task tools length is invalid"));};
            let mut selected=Vec::new();for i in 0..length as usize{selected.push(field(&tools,&i.to_string(),runtime)?.ok_or_else(||fault("Task tools must be a string array"))?);}Some(selected)
        }else{None};
        items.push(TaskItem{name,agent,task,solution_space:field(row,"solutionSpace",runtime)?,output_schema,schema_mode,tools,isolated,effort});
    }
    Ok(TaskRequest{items,context})
}
