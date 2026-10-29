//! Binary Task execution owner. Fixed OMP b91db66e task/{index,executor}.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! The actor prepares actual ChildHosts; retained workers own their complete
//! turns, isolation captures and completion receipts after observers disappear.
use super::*;
use ara_agent::AgentTool;
use ara_cli::{child_contract::{ChildResultStatus, prompt::ChildPromptInputs},
    host_peers::{RuntimeKey, WakeCompletion, WakeOutcome},
    task_contract::{TaskExecutionPort, TaskPolicy, TaskRequest, TaskResult},
    task_isolation::{IsolationLease, IsolationRequest}};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{mpsc, oneshot, Notify};

pub(super) struct TaskEnvelope {
    pub session_id:String, pub request:TaskRequest, pub cancel:CancellationToken,
    pub reply:oneshot::Sender<Result<TaskResult>>,
}
#[derive(Default)]
pub(super) struct TaskScheduler {active:AtomicUsize, changed:Notify}
struct TaskSlot(Arc<TaskScheduler>);
impl Drop for TaskSlot {fn drop(&mut self){self.0.active.fetch_sub(1,Ordering::AcqRel);self.0.changed.notify_waiters();}}
impl TaskScheduler {
    async fn acquire(self:&Arc<Self>,cwd:&Path,cancel:&CancellationToken)->Result<TaskSlot>{
        loop {
        anyhow::ensure!(!cancel.is_cancelled(),"Task cancelled before model admission");
        let maximum=rpc_host_settings::TaskSettings::load(cwd,&ara_home().join("agent"))?.max_concurrency;
        let maximum=if maximum.is_finite()&&maximum.trunc()>0.0{maximum.trunc().min(usize::MAX as f64)as usize}else{usize::MAX};
            let changed=self.changed.notified();tokio::pin!(changed);changed.as_mut().enable();
            let active=self.active.load(Ordering::Acquire);
            if active<maximum&&self.active.compare_exchange(active,active+1,Ordering::AcqRel,Ordering::Acquire).is_ok(){return Ok(TaskSlot(self.clone()));}
            tokio::select!{_=cancel.cancelled()=>bail!("Task cancelled before model admission"),_=changed=>{}}
        }
    }
}
#[derive(Clone)]
pub(super) struct TaskDispatch {
    pub sender:mpsc::UnboundedSender<TaskEnvelope>, pub scheduler:Arc<TaskScheduler>,
}
/// keepAlive is an execution Host input, not an invented YAML setting.
/// Controlled process banks provide that input through the existing fixture build.
pub(super) fn host_policy(mut policy:TaskPolicy)->Result<TaskPolicy>{
    #[cfg(feature="test-fixture")]
    if let Some(value)=std::env::var_os("ARA_TEST_TASK_KEEP_ALIVE"){
        policy.keep_alive=match value.to_str(){Some("true")=>true,Some("false")=>false,_=>bail!("Controlled Task keepAlive input must be true or false")};
    }
    Ok(policy)
}
struct BinaryTaskPort {
    dispatch:TaskDispatch, session_id:String, cwd:PathBuf,
    plan:Option<Arc<ara_cli::host_edit_session::HostEditSession>>,
}
impl BinaryTaskPort {
    fn current_policy(&self)->Result<TaskPolicy>{
        use ara_tools::edit_host::EditHostPort;
        let mut policy=rpc_host_settings::TaskSettings::load(&self.cwd,&ara_home().join("agent"))?;
        policy.plan_mode=self.plan.as_ref().is_some_and(|plan|plan.path_policy().plan_active);
        host_policy(policy)
    }
}
#[async_trait::async_trait]
impl TaskExecutionPort for BinaryTaskPort {
    fn policy(&self)->TaskPolicy{self.current_policy().unwrap_or_default()}
    async fn execute(&self,request:TaskRequest,cancel:CancellationToken)->Result<TaskResult>{
        // A settings error is an execution error, never an implicit default grant.
        self.current_policy()?;
        let(reply,wait)=oneshot::channel();
        self.dispatch.sender.send(TaskEnvelope{session_id:self.session_id.clone(),request,cancel,reply})
            .map_err(|_|anyhow::anyhow!("Task Host actor is closed before admission"))?;
        wait.await.context("Task result observer lost; inspect original Task receipts before retry")?
    }
}
pub(super) fn bind(dispatch:&TaskDispatch,native:&native_setup::NativeCliOwner,id:&str,cwd:&Path,
    plan:Option<Arc<ara_cli::host_edit_session::HostEditSession>>)->Result<()>{
    native.task_session.bind_session(id)?;
    native.task_session.bind(Arc::new(BinaryTaskPort{dispatch:dispatch.clone(),session_id:id.into(),cwd:cwd.into(),plan}))
}
pub(super) struct PreparedTask {
    execution_id:String, name:Option<String>, agent:String, source:String,
    assignment:String, runtime:RuntimeKey, lease:Option<Arc<IsolationLease>>,
    artifact_dir:PathBuf, transcript:PathBuf, policy_cwd:PathBuf, policy:TaskPolicy, blocking:bool,
}

/// Resolve the entire batch's agent/tool/depth policy before creating any child.
pub(super) async fn prepare(template:&rpc_host::SessionFactory,parent:&SessionJournal,
    request:TaskRequest,policy:TaskPolicy,cancel:&CancellationToken)->Result<Vec<PreparedTask>>{
    anyhow::ensure!(parent.is_persistent()&&!parent.path().as_os_str().is_empty(),"Task requires a persistent owning Session journal; in-memory parent has no durable child/artifact ownership");
    let depth=rpc_host_settings::peer_task_depth(Some(parent))?;
    anyhow::ensure!(policy.can_spawn_at_depth(depth),"Task maximum recursion depth reached");
    anyhow::ensure!(!request.items.is_empty(),"Task batch is empty");
    anyhow::ensure!(policy.batch_enabled||request.items.len()==1,"Task batch is disabled");
    let peers=template.peers.as_ref().context("Task requires an actual Peer ownership Host")?;
    let native=template.tool_owner.native.as_ref().context("Task requires Native ownership")?;
    // Coordination's transport-only WRITE is callable, but is not a delegable full WRITE grant.
    let transport_only_write=template.tool_owner.context.as_ref().is_some_and(|context|context.write_transport_policy().device_only);
    let parent_tools=native.snapshot.advertised_capabilities(native.prepared.values())?.into_iter()
        .map(|tool|tool.definition().name)
        .filter(|name|name!="write"||!transport_only_write).collect::<std::collections::HashSet<_>>();
    let mut preflight=Vec::new();
    for item in request.items {
        let definition=super::task_agents::load(&template.cwd,&ara_home().join("agent"),&item.agent)?;
        anyhow::ensure!(!item.task.trim().is_empty(),"Task assignment is empty");
        anyhow::ensure!(item.isolated.is_none()||policy.isolation_enabled&&!policy.plan_mode,"Task isolation is unavailable in current mode");
        anyhow::ensure!(item.effort.is_none()||policy.effort_enabled,"Task effort selection is disabled");
        anyhow::ensure!(item.output_schema.is_none()&&item.schema_mode.is_none()||policy.structured,"Task structured output is disabled");
        anyhow::ensure!(item.tools.is_none()||policy.eval_tools_enabled&&!policy.plan_mode,"Task caller tool selection is unavailable in current mode");
        let granted=|name:&str|->Result<bool>{
            if let Some(member)=native.builtin.iter().find(|member|member.binding.capability.definition().name==name){return Ok(native.builtin_grant(member.kind)?==Some(true));}
            if let Some(member)=native.rpc.iter().find(|member|member.binding.capability.definition().name==name){return Ok(native.rpc_grant(&member.owner)?==Some(true));}
            if let Some(owner)=template.tool_owner.owned_mcp.iter().find(|owner|owner.definition().name==name){return Ok(native.mcp_grant(owner)?==Some(true));}
            Ok(false)
        };
        let tools=if let Some(requested)=&item.tools{
            // Caller-selected tools are an explicit authority request. Reject
            // the entire preflight before any child or isolation effect.
            for name in requested{
                anyhow::ensure!(parent_tools.contains(name),"Task tool {name:?} is not exposed by the actual parent");
                anyhow::ensure!(granted(name)?,"Task tool {name:?} has no actual parent grant");
            }
            requested.clone()
        }else{
            // Fixed executor.ts4044 declarations describe desired capabilities;
            // createTools/our child factory selects those the real Host owns.
            // Missing find/lsp must not reject a bundled specialist, nor grant it.
            let declared=definition.tools.clone().unwrap_or_else(||parent_tools.iter().cloned().collect());
            let mut selected=Vec::new();
            for name in declared{if parent_tools.contains(&name)&&granted(&name)?{selected.push(name);}}
            selected
        };
        let schema=item.output_schema.clone().or_else(||definition.output_schema.clone());
        let schema_mode=item.schema_mode.clone().or_else(||definition.schema_mode.clone());
        let model=template.provider.route.model();
        let selected=definition.model.as_deref().filter(|model|!model.starts_with('@')).map(str::to_owned)
            .unwrap_or_else(||format!("{}/{}",model.provider,model.id));
        let mut init:ara_session::PersistedChildInit=serde_json::from_value(json!({
            "systemPrompt":definition.prompt,"task":item.task,"tools":tools,"agent":definition.name,
            "modelRole":definition.model_role,"resolvedModel":selected,"readOnly":definition.read_only,
            "spawns":definition.spawns,"readSummarize":definition.read_summarize,
            "outputSchemaMode":schema_mode,"restrictToolNames":item.tools.is_some(),
        }))?;
        init.output_schema=schema;
        init.extra.insert("araTaskDeclaredTools".into(),json!(definition.tools));
        init.extra.insert("araTaskModelPatterns".into(),json!(definition.model_patterns));
        init.extra.insert("araTaskMaxRuntimeMs".into(),json!(policy.max_runtime_ms));
        init.extra.insert("araTaskAgentSource".into(),json!(definition.source));
        init.extra.insert("araTaskAutoloadSkills".into(),json!(definition.autoload_skills));
        init.extra.insert("araTaskPrewalk".into(),json!(definition.prewalk));
        init.extra.insert("araTaskAdvisor".into(),json!(definition.advisor));
        if let Some(thinking)=definition.thinking.as_ref(){init.extra.insert("araTaskThinking".into(),json!(thinking));}
        if let Some(effort)=item.effort.as_ref(){
            init.extra.insert("araTaskEffort".into(),json!(effort));
            init.extra.insert("araTaskEffortCeiling".into(),json!(rpc_host_settings::TaskSettings::max_effort(&template.cwd,&ara_home().join("agent"))?.as_str()));
        }
        let context=request.context.clone().unwrap_or_default();
        let prompt=ChildPromptInputs{base:init.system_prompt.clone(),agent:item.agent.clone(),context,
            structured:policy.structured,output_schema:init.output_schema.clone(),
            output_schema_overrides_agent:item.output_schema.is_some(),..Default::default()};
        // solutionSpace belongs to the prompt dispatch, stored separately from
        // instructions and passed by the actual child action below.
        if let Some(solution)=item.solution_space.as_ref(){init.extra.insert("araTaskSolutionSpace".into(),json!(solution));}
        preflight.push((item,definition,init,prompt));
    }
    let mut prepared=Vec::<PreparedTask>::new();
    for(item,definition,mut init,prompt)in preflight {
        let creating=async{
        if cancel.is_cancelled(){bail!("Task cancelled before child creation");}
        let execution_id=uuid::Uuid::now_v7().to_string();
        init.extra.insert("araTaskExecutionId".into(),json!(execution_id));
        let artifact_dir=parent.path().with_extension("").join(format!("task-{execution_id}"));
        let lease=if item.isolated==Some(true){Some(IsolationLease::create(IsolationRequest{
            repo_root:template.cwd.clone(),resource_root:ara_home().join("task-worktrees"),
            artifacts_dir:artifact_dir.clone(),task_id:execution_id.clone(),merge_mode:policy.merge_mode.clone(),
            backend:policy.backend.clone(),description:Some(item.task.clone()),
        }).await?)}else{None};
        let factory=super::child_host::ChildHostFactory::new(template,parent)?;
        let child=match lease.as_ref(){Some(lease)=>factory.create_isolated(init,prompt,policy.structured,lease.clone(),cancel).await,
            None=>factory.create(init,prompt,policy.structured,cancel).await}?;
        let transcript=child.file.clone();
        let runtime=peers.attach_child_in_root(template.peer_root.as_deref().unwrap_or(parent.session_id()),child)?;
        Ok::<_,anyhow::Error>(PreparedTask{execution_id,name:item.name,agent:item.agent,source:definition.source,
            assignment:item.task,runtime,lease,artifact_dir,transcript,policy_cwd:template.cwd.clone(),policy:policy.clone(),blocking:definition.blocking})
        }.await;
        match creating{Ok(task)=>prepared.push(task),Err(error)=>{
            let mut cleanup=Vec::new();
            for task in &prepared{if let Err(close)=peers.coordinator.lifecycle.release(&task.runtime.peer,false).await{cleanup.push(format!("{}: {close:#}",task.execution_id));}}
            return Err(error.context(format!("Task preparation failed; earlier actual child close receipts: {cleanup:?}")));
        }}
    }
    Ok(prepared)
}

fn persist(path:&Path,value:&Value)->Result<()>{
    use std::io::Write;
    std::fs::create_dir_all(path.parent().context("Task receipt parent missing")?)?;
    let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;file.sync_all()?;Ok(())
}
fn publish(path:&Path,bytes:&[u8])->Result<()>{
    use std::io::Write;
    std::fs::create_dir_all(path.parent().context("Task publication parent missing")?)?;
    let temporary=path.with_extension(format!("{}.tmp",uuid::Uuid::now_v7()));
    let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
    file.write_all(bytes)?;file.sync_all()?;drop(file);std::fs::rename(temporary,path)?;Ok(())
}
pub(super) fn publish_capture(peers:&peer_host::BinaryPeerHost,key:&ara_cli::host_peers::PeerKey,transcript:&Path,capture:&ara_cli::task_isolation::CaptureReceipt)->Result<()>{
    let current=peers.coordinator.registry.get(&key.root,&key.id);
    if current.as_ref().is_some_and(|record|record.key!=*key){bail!("Task capture publication belongs to an old Peer generation; durable capture retained");}
    let Some(record)=current else{return Ok(())};
    anyhow::ensure!(record.session_file.as_deref()==Some(transcript),"Task capture original transcript changed");
    let paths=ara_cli::host_peers::roster::artifact_paths(transcript,&key.id);
    publish(&paths.patch,&std::fs::read(&capture.patch_path)?)?;
    let mut history=record.history.as_object().cloned().unwrap_or_default();
    history.insert("patchPath".into(),json!(paths.patch));
    history.insert("isolation".into(),serde_json::to_value(capture)?);
    history.insert("resourceId".into(),json!(capture.resource_id));
    history.insert("branch".into(),json!(capture.branch));
    anyhow::ensure!(peers.coordinator.registry.set_history(key,Value::Object(history),Some(transcript)),"Task exact history owner changed during publication; durable capture retained");
    Ok(())
}
async fn run(task:PreparedTask,peers:Arc<super::peer_host::BinaryPeerHost>,
    scheduler:Arc<TaskScheduler>,cancel:CancellationToken,job:ara_cli::host_peers::WakeJobId,owned_cancel:CancellationToken)->Result<Value>{
    let started=std::time::Instant::now();
    let stop=owned_cancel.clone();
    let caller_cancel=cancel.clone();
    let cancellation=tokio::spawn(async move{tokio::select!{_=cancel.cancelled()=>stop.cancel(),_=stop.cancelled()=>{}}});
    let mut retained_admission=None;
    let mut run_succeeded=false;
    let operation=async{
        let _slot=scheduler.acquire(&task.policy_cwd,&owned_cancel).await?;
        retained_admission=match &task.lease{Some(lease)=>Some(lease.admit().await?),None=>None};
        let slot=peers.child_slot(&task.runtime)?;
        let child=slot.lock().await;
        let child=child.as_ref().context("Task exact ChildHost already closed")?;
        peers.coordinator.registry.set_status(&task.runtime.peer,ara_cli::host_peers::PeerStatus::Running);
        let observed=child.followup(task.assignment.clone(),Vec::new(),&owned_cancel).await;
        // Keep the permit through the real Native idle + all deferred effects.
        // An error path must join/cancel the SAME child before taking a capture.
        if let Err(error)=observed.as_ref(){
            return Err(anyhow::anyhow!("Task child turn failed; resource remains owned for close reconciliation: {error:#}"));
        }
        let observed=observed?;
        let success=observed.status==ChildResultStatus::Success&&observed.error.is_none();
        run_succeeded=success;
        let data=observed.data.as_ref().map(|data|->Result<Value>{let values=child.owner.values();
            let encoded=values.heap().stringify(data,ara_rpc::value_heap::StringifyMode::Plain,None,values.runtime())?
                .context("Task child result is undefined")?.to_utf8()?;Ok(serde_json::from_str(&encoded)?)}).transpose()?;
        if !success{if let Some(lease)=&task.lease{lease.seal_admission()?;}}
        let cancelled_before_capture=owned_cancel.is_cancelled()||caller_cancel.is_cancelled();
        let capture=if success&&!cancelled_before_capture{match(&task.lease,&retained_admission){(Some(lease),Some(admission))=>Some(lease.capture_run_end(admission).await?),_=>None}}else{None};
        drop(retained_admission.take());
        let apply=if success&&!owned_cancel.is_cancelled()&&!caller_cancel.is_cancelled()&&task.policy.apply_changes {
            match(&task.lease,&capture){(Some(lease),Some(capture))=>Some(lease.apply_run_end_cancellable(capture,owned_cancel.clone(),Some(caller_cancel.clone())).await?),_=>None}
        }else{None};
        let cancelled=owned_cancel.is_cancelled()||caller_cancel.is_cancelled();
        let integrated=!cancelled&&apply.as_ref().is_none_or(|receipt|receipt.error.is_none());
        let error=observed.error.clone().or_else(||apply.as_ref().and_then(|receipt|receipt.error.clone()))
            .or_else(||cancelled.then(||"Task cancelled after model completion; no new parent apply admitted".into()));
        let value=json!({"taskExecutionId":task.execution_id,"agentId":task.runtime.peer.id,"runtime":task.runtime,
            "name":task.name,"agent":task.agent,"agentSource":task.source,"data":data,
            "runStatus":format!("{:?}",observed.status),"runSucceeded":success,"integrationSucceeded":integrated,"cancelled":cancelled,"acceptedYield":observed.accepted&&task.policy.structured,"taskAccepted":false,
            "error":error,"schemaOverridden":observed.schema_overridden,
            "capture":capture,"apply":apply,"artifactSucceeded":true,"elapsedMs":started.elapsed().as_millis()});
        Ok::<_,anyhow::Error>(value)
    }.await;
    if operation.as_ref().is_err(){if let Some(lease)=&task.lease{let _=lease.seal_admission();}}
    drop(retained_admission.take());
    cancellation.abort();
    // Admission and child slot guards have ended before real close/release.
    let mut value=match operation{Ok(value)=>value,Err(error)=>json!({"taskExecutionId":task.execution_id,
        "agentId":task.runtime.peer.id,"runtime":task.runtime,"runSucceeded":run_succeeded,"integrationSucceeded":false,"cancelled":owned_cancel.is_cancelled()||caller_cancel.is_cancelled(),"taskAccepted":false,
        "error":format!("{error:#}"),"elapsedMs":started.elapsed().as_millis()})};
    let release_needed=!task.policy.keep_alive||value["runSucceeded"]!=true||!value["error"].is_null()||owned_cancel.is_cancelled();
    if release_needed {
        let close=peers.close_runtime(&task.runtime).await;
        let resource=peers.release_task_resource(&task.runtime.peer).await;
        if let Err(error)=close.and(resource){value["releaseError"]=json!(format!("{error:#}"));value["error"]=json!(format!("Task release remains unsettled: {error:#}"));value["releaseSucceeded"]=json!(false);}
    }
    if release_needed&&value["releaseError"].is_null(){value["releaseSucceeded"]=json!(true);}
    if owned_cancel.is_cancelled()||caller_cancel.is_cancelled(){value["cancelled"]=json!(true);if value["error"].is_null(){value["error"]=json!("Task cancelled after Run completion; original effect receipts retained");}}
    let successful=value["runSucceeded"]==true;
    let receipt_path=task.artifact_dir.join("result.json");
    let canonical=ara_cli::host_peers::roster::artifact_paths(&task.transcript,&task.runtime.peer.id);
    let artifact=(||->Result<()>{
        let data=value.get("data").cloned().unwrap_or(Value::Null);
        let text=data.as_str().map(str::to_owned).unwrap_or(serde_json::to_string_pretty(&data)?);
        publish(&canonical.markdown,text.as_bytes())?;publish(&canonical.json,serde_json::to_string_pretty(&data)?.as_bytes())?;
        if !value["capture"].is_null(){let capture:ara_cli::task_isolation::CaptureReceipt=serde_json::from_value(value["capture"].clone())?;publish_capture(&peers,&task.runtime.peer,&task.transcript,&capture)?;}
        persist(&receipt_path,&value)
    })();
    let output_path=canonical.markdown;
    if let Err(error)=artifact{value["error"]=json!(format!("Task result persistence failed: {error:#}"));value["artifactSucceeded"]=json!(false);}
    peers.coordinator.registry.set_status(&task.runtime.peer,ara_cli::host_peers::PeerStatus::Idle);
    let outcome=WakeOutcome{accepted:successful&&value["error"].is_null(),accepted_yield:successful&&value["error"].is_null()&&value["acceptedYield"]==true,
        answer:Some(serde_json::to_string_pretty(&value)?),output_path:Some(output_path.clone()),
        error:value["error"].as_str().map(str::to_owned),cancelled:owned_cancel.is_cancelled()||caller_cancel.is_cancelled(),..Default::default()};
    peers.coordinator.jobs.settle(WakeCompletion{job:job.clone(),runtime:task.runtime.clone(),outcome})?;
    peers.coordinator.jobs.finish_replies(&job);
    if release_needed{
        if let Err(error)=peers.coordinator.lifecycle.release(&task.runtime.peer,false).await{
            value["releaseError"]=json!(format!("{error:#}"));value["releaseSucceeded"]=json!(false);
        }
    }
    Ok(value)
}

/// Spawn before returning an ACK. The worker owns both blocking and background
/// operations, so loss of the caller's oneshot cannot abandon their lifecycle.
pub(super) fn launch(tasks:Vec<PreparedTask>,peers:Arc<super::peer_host::BinaryPeerHost>,
    dispatch:TaskDispatch,cancel:CancellationToken,reply:oneshot::Sender<Result<TaskResult>>){
    tokio::spawn(async move{
        let mut blocking=Vec::new();let mut running=Vec::new();
        for task in tasks {
            let asynchronous=task.policy.async_enabled&&!task.blocking;
            let parent=peers.coordinator.registry.get_exact(&task.runtime.peer).and_then(|row|row.parent_id).unwrap_or_default();
            let job=peers.coordinator.jobs.next_id(task.runtime.peer.clone(),parent);
            let owned_cancel=match peers.coordinator.jobs.begin_turn(job.clone(),task.runtime.clone(),asynchronous){
                Ok(cancel)=>cancel,Err(error)=>{let _=reply.send(Err(error));return;}
            };
            let identity=json!({"taskExecutionId":task.execution_id,"agentId":task.runtime.peer.id,"runtime":task.runtime,
                "taskAccepted":false,"state":"running","jobId":job.child.id});
            let peers=peers.clone();let scheduler=dispatch.scheduler.clone();let cancel=cancel.clone();
            let worker=tokio::spawn(run(task,peers,scheduler,cancel,job,owned_cancel));
            if asynchronous{running.push(identity);}else{blocking.push(worker);}
        }
        let mut results=Vec::new();let mut is_error=false;
        for worker in blocking {match worker.await{
            Ok(Ok(value))=>{is_error|=value["runSucceeded"]!=true||!value["error"].is_null()||!value["releaseError"].is_null();results.push(value);},
            Ok(Err(error))=>{is_error=true;results.push(json!({"error":format!("{error:#}"),"taskAccepted":false}));},
            Err(error)=>{is_error=true;results.push(json!({"error":format!("Task retained worker failed: {error}"),"taskAccepted":false}));},
        }}
        let async_details=running.first().map(|first|json!({"state":"running","jobId":first["jobId"],"type":"task"}));
        let details=json!({"results":results,"running":running,"async":async_details,"taskAccepted":false});
        let result=TaskResult{content:serde_json::to_string_pretty(&details).unwrap_or_else(|e|e.to_string()),details,is_error};
        let _=reply.send(Ok(result));
    });
}
