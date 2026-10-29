//! Actual binary child factory/actor adapter for fixed OMP b91db66e.
//! This is a Native Host API, not a public Peer/Task/ACP protocol.
use super::*;
use serde_json::{Value,json};
use ara_agent::AgentTool;
use ara_cli::child_contract::{ChildContractOwner,ChildContractConfig,ChildContractError,ChildRunObserver,ChildRunFinish,ObservedChildResult,DispatchLease,WorkPoolYieldItem,ContractSnapshot};
use ara_session::PersistedChildInit;
use ara_rpc::{WireString,value_heap::ValueSnapshot};
use std::sync::Mutex;

/// Fixed executor.ts2048-2057: work invalidates an earlier report candidate.
pub(super) enum ReportUpdate {NoUpdate,Report(WireString),Invalidate}
fn has_report_text(text:&WireString)->bool {
    text.units().iter().any(|unit|!matches!(*unit,0x0009..=0x000d|0x0020|0x00a0|0x1680|0x2000..=0x200a|0x2028|0x2029|0x202f|0x205f|0x3000|0xfeff))
}

enum AdmissionReceipt {Unobserved,Admitted(u64),BusyNotDispatched{session_id:String,generation:u64,competing_dispatch:Option<u64>}}
struct TurnState {
    admission:AdmissionReceipt,
    observer:Option<Arc<ChildRunObserver>>,lease:Option<DispatchLease>,
    finished:Option<(bool,bool,Option<WireString>)>,failure:Option<String>,known_model_error:Option<String>,last_report:Option<WireString>,
}
/// Retained receipt owner survives a caller dropping its response wait. Never redispatch on lost ACK.
pub(super) struct ChildTurn {
    pub expected_generation:u64,pub pooled:bool,
    classification_input:Mutex<Option<String>>,state:Mutex<TurnState>,wake:tokio::sync::Notify,forced_final:std::sync::atomic::AtomicBool,continuation:std::sync::atomic::AtomicBool,
}
impl ChildTurn {
    pub fn new(expected_generation:u64,pooled:bool)->Arc<Self>{Arc::new(Self{expected_generation,pooled,classification_input:Mutex::new(None),state:Mutex::new(TurnState{admission:AdmissionReceipt::Unobserved,observer:None,lease:None,finished:None,failure:None,known_model_error:None,last_report:None}),wake:tokio::sync::Notify::new(),forced_final:std::sync::atomic::AtomicBool::new(false),continuation:std::sync::atomic::AtomicBool::new(false)})}
    pub fn classification_input(&self)->Option<String>{self.classification_input.lock().unwrap_or_else(|e|e.into_inner()).clone()}
    pub fn admit(&self,observer:Arc<ChildRunObserver>,lease:DispatchLease)->Result<()> {
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        if state.observer.is_some()||state.failure.is_some(){bail!("Child turn was already admitted or failed");}
        state.admission=AdmissionReceipt::Admitted(lease.id());state.observer=Some(observer);state.lease=Some(lease);drop(state);self.wake.notify_waiters();Ok(())
    }
    pub fn admit_native(&self,bundle:&ara_cli::child_contract::NativeYieldBundle,lease:DispatchLease)->Result<()> {
        bundle.mark_final_yield_forced(self.forced_final());
        let observer=self.state.lock().unwrap_or_else(|e|e.into_inner()).observer.clone();
        if observer.is_some(){
            let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
            if !self.continuation.load(std::sync::atomic::Ordering::Acquire)||state.lease.is_some(){bail!("Child logical turn cannot acquire another Native Run");}
            state.admission=AdmissionReceipt::Admitted(lease.id());state.lease=Some(lease);Ok(())
        }else{self.admit(bundle.begin_turn(lease.id())?,lease)}
    }
    /// Only the actual actor may stamp a known pre-projection busy rejection.
    pub fn reject_busy_not_dispatched(&self,session_id:String,generation:u64,competing_dispatch:Option<u64>){
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        if state.observer.is_none()&&state.lease.is_none(){state.admission=AdmissionReceipt::BusyNotDispatched{session_id,generation,competing_dispatch};}
        drop(state);self.wake.notify_waiters();
    }
    fn busy_not_dispatched(&self,session_id:&str,generation:u64)->bool{
        let state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        state.observer.is_none()&&matches!(&state.admission,AdmissionReceipt::BusyNotDispatched{session_id:id,generation:observed,..}if id==session_id&&*observed==generation)
    }
    pub fn dispatch_id(&self)->Option<u64>{match &self.state.lock().unwrap_or_else(|e|e.into_inner()).admission{AdmissionReceipt::Admitted(id)=>Some(*id),_=>None}}
    #[cfg(feature="test-fixture")]
    fn admission_json(&self)->Value{let state=self.state.lock().unwrap_or_else(|e|e.into_inner());match &state.admission{
        AdmissionReceipt::Unobserved=>json!({"state":"Unobserved"}),AdmissionReceipt::Admitted(id)=>json!({"state":"Admitted","dispatchId":id}),
        AdmissionReceipt::BusyNotDispatched{session_id,generation,competing_dispatch}=>json!({"state":"BusyNotDispatched","sessionId":session_id,"generation":generation,"competingDispatchId":competing_dispatch}),
    }}
    #[cfg(feature="test-fixture")]
    async fn wait_busy_receipt(&self)->Result<()>{loop{let wake=self.wake.notified();tokio::pin!(wake);wake.as_mut().enable();{
        let state=self.state.lock().unwrap_or_else(|e|e.into_inner());match &state.admission{AdmissionReceipt::BusyNotDispatched{..}=>return Ok(()),AdmissionReceipt::Admitted(_)=>bail!("Fixture competitor failed to win actual dispatch"),AdmissionReceipt::Unobserved=>{}}
    }wake.await;}}
    pub fn observer(&self)->Result<Arc<ChildRunObserver>>{self.state.lock().unwrap_or_else(|e|e.into_inner()).observer.clone().context("Child observer not admitted")}
    pub fn prepare_continuation(&self,forced:bool)->Result<()> {
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        if state.finished.as_ref().is_none_or(|(clean,cancelled,_)|!*clean||*cancelled){bail!("Cannot resume a failed/cancelled child Run as a reminder");}
        state.lease.take();state.finished=None;state.known_model_error=None;
        self.forced_final.store(forced,std::sync::atomic::Ordering::Release);self.continuation.store(true,std::sync::atomic::Ordering::Release);Ok(())
    }
    pub fn prepare_host_recovery(&self){
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        state.finished=None;state.known_model_error=None;
        self.continuation.store(true,std::sync::atomic::Ordering::Release);
    }
    pub fn release_recovery_lease(&self){if self.is_continuation(){self.state.lock().unwrap_or_else(|e|e.into_inner()).lease.take();}}
    pub fn require_settled(&self)->Result<()> {
        let state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        if let Some(error)=&state.failure{bail!(error.clone());}
        if state.finished.is_none(){drop(state);self.fail("Native child became idle without a successful completion receipt".into());bail!("Native child became idle without its retained completion receipt");}
        Ok(())
    }
    // Only the actor's persisted, settled RunEnd::Error may stamp this receipt.
    // Generic transport, persistence, cancellation and unknown effects retain Err.
    pub fn note_known_model_error(&self,error:String){
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        if state.failure.is_none()&&matches!(&state.finished,Some((false,false,_))){state.known_model_error=Some(error);}
    }
    fn known_model_error(&self)->Option<String>{
        let state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        if state.failure.is_none()&&matches!(&state.finished,Some((false,false,_))){state.known_model_error.clone()}else{None}
    }
    pub fn forced_final(&self)->bool{self.forced_final.load(std::sync::atomic::Ordering::Acquire)}
    pub fn is_continuation(&self)->bool{self.continuation.load(std::sync::atomic::Ordering::Acquire)}
    pub fn clean(&self)->bool{self.state.lock().unwrap_or_else(|e|e.into_inner()).finished.as_ref().is_some_and(|(clean,cancelled,_)|*clean&&!*cancelled)}
    pub fn admitted(&self)->bool {self.state.lock().unwrap_or_else(|e|e.into_inner()).observer.is_some()}
    pub fn finish_run(&self,clean:bool,cancelled:bool,update:ReportUpdate){
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        match update {ReportUpdate::Report(text)=>state.last_report=Some(text),ReportUpdate::Invalidate=>state.last_report=None,ReportUpdate::NoUpdate=>{}}
        if state.finished.is_none(){let last=state.last_report.clone();state.finished=Some((clean,cancelled,last));}
        drop(state);self.wake.notify_waiters();
    }
    pub fn fail(&self,error:String){let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());state.failure.get_or_insert(error);state.lease.take();drop(state);self.wake.notify_waiters();}
    pub fn finalize(&self,owned_jobs_settled:bool)->Result<ObservedChildResult>{
        let (observer,clean,cancelled,text)={
            let state=self.state.lock().unwrap_or_else(|e|e.into_inner());
            if let Some(error)=&state.failure{bail!(error.clone());}
            let (clean,cancelled,text)=state.finished.clone().context("Child Native Run has not settled")?;
            (state.observer.clone().context("Child Run lacks exact admitted observer")?,clean,cancelled,text)
        };
        let result=observer.finish(ChildRunFinish{clean_exit:clean,cancelled,owned_jobs_settled,last_assistant_text:text})?;
        if result.status!=ara_cli::child_contract::ChildResultStatus::PendingOwnedWork {
            self.state.lock().unwrap_or_else(|e|e.into_inner()).lease.take();self.wake.notify_waiters();
        }
        Ok(result)
    }
}

/// Concatenate original Graph text units; no serde/model mirror during result observation.
pub(super) fn report_update(report:&ara_agent::RunReport,projector:&ara_cli::agent_message_projection::AgentMessageProjector)->Result<ReportUpdate>{
    for message in report.messages.iter().rev(){
        if let ara_agent::AgentMessageBody::Graph(root)=message.body_snapshot(){
            let values=projector.actual_values().context("Child result has no Native values")?;
            let heap=values.heap();let runtime=values.runtime();
            let role=heap.get(&root,&"role".into(),runtime)?;
            if !matches!(heap.snapshot(&role)?,ValueSnapshot::String(s)if s.equals_ascii("assistant")){continue;}
            let content=heap.get(&root,&"content".into(),runtime)?;
            let mut units=Vec::new();let mut starts_work=false;let mut text_blocks=0;
            for index in 0..heap.array_length(&content,runtime)?{
                let block=heap.get(&content,&index.to_string().as_str().into(),runtime)?;
                let kind=heap.get(&block,&"type".into(),runtime)?;
                if matches!(heap.snapshot(&kind)?,ValueSnapshot::String(s)if s.equals_ascii("toolCall")){let name=heap.get(&block,&"name".into(),runtime)?;if !matches!(heap.snapshot(&name)?,ValueSnapshot::String(name)if name.equals_ascii("yield")){starts_work=true;}}
                if matches!(heap.snapshot(&kind)?,ValueSnapshot::String(s)if s.equals_ascii("text")){
                    let text=heap.get(&block,&"text".into(),runtime)?;
                    if let ValueSnapshot::String(text)=heap.snapshot(&text)?{if !text.units().is_empty(){if text_blocks>0{units.push(10);}text_blocks+=1;units.extend_from_slice(text.units());}}
                }
            }
            if starts_work{return Ok(ReportUpdate::Invalidate);}
            let text=WireString::from_units(units);
            if !has_report_text(&text){continue;}
            return Ok(ReportUpdate::Report(text));
        }
        if let Some(Message::Assistant(assistant))=message.checked_model_snapshot()? {
            if assistant.tool_calls().any(|call|call.name!="yield"){return Ok(ReportUpdate::Invalidate);}
            let text=assistant.content.iter().filter_map(|block|match block{ara_ai::AssistantBlock::Text(text)if !text.text.is_empty()=>Some(text.text.as_str()),_=>None}).collect::<Vec<_>>().join("\n");
            let text=WireString::from(text);if has_report_text(&text){return Ok(ReportUpdate::Report(text));}
        }
    }
    Ok(ReportUpdate::NoUpdate)
}

pub(super) struct ChildCompletionGuard {turn:Option<Arc<ChildTurn>>}
impl ChildCompletionGuard {
    pub fn new(turn:Option<Arc<ChildTurn>>)->Self{Self{turn}}
    pub fn defer(&mut self){if let Some(turn)=self.turn.take(){turn.prepare_host_recovery();}}
    pub fn disarm(&mut self){self.turn=None;}
    pub fn finish(&mut self,clean:bool,cancelled:bool,update:ReportUpdate){if let Some(turn)=self.turn.take(){turn.finish_run(clean,cancelled,update);}}
}
impl Drop for ChildCompletionGuard {fn drop(&mut self){if let Some(turn)=&self.turn{turn.fail("Child Native Host completion failed before persistence/recovery settlement".into());}}}

#[cfg(feature="test-fixture")]
struct DispatchRaceAttempt {turn:Arc<ChildTurn>,ready:tokio::sync::oneshot::Sender<()>}
#[cfg(feature="test-fixture")]
struct DispatchRaceGate {sender:tokio::sync::mpsc::UnboundedSender<DispatchRaceAttempt>,attempts:std::sync::atomic::AtomicUsize}
#[cfg(feature="test-fixture")]
impl DispatchRaceGate {async fn pause_before_send(&self,turn:Arc<ChildTurn>)->Result<()>{
    self.attempts.fetch_add(1,std::sync::atomic::Ordering::AcqRel);
    let (ready,wait)=tokio::sync::oneshot::channel();self.sender.send(DispatchRaceAttempt{turn,ready}).map_err(|_|anyhow::anyhow!("Fixture dispatch gate driver closed"))?;
    wait.await.context("Fixture dispatch gate driver ended before actual send")
}}

pub(super) struct ChildHost {
    pub id:String,pub file:PathBuf,pub parent_id:String,pub depth:usize,
    pub owner:Arc<ChildContractOwner>,pub native:Arc<native_setup::NativeCliOwner>,
    solution_space:Option<String>,
    runtime:Option<rpc_host::NativeSessionRuntime>,
    #[cfg(feature="test-fixture")] dispatch_race:Mutex<Option<Arc<DispatchRaceGate>>>,
    serial:tokio::sync::Mutex<()>,last_turn:Mutex<Option<Arc<ChildTurn>>>,
}
impl ChildHost {
    pub fn handle(&self)->&rpc_host::NativeSessionHandle{&self.runtime.as_ref().expect("live child Host").handle}
    /// Actual idle waits are bounded by source count, and caller cancellation never replays a send.
    async fn acquire_idle(&self,cancel:&CancellationToken)->Result<()> {
        for _ in 0..3 {
            tokio::select!{_ = cancel.cancelled()=>bail!("Child ownership acquisition cancelled"),result=self.handle().call(rpc_host::NativeAction::WaitIdle)=>{result?;}}
            if !self.owner.dispatch_active(){return Ok(())}
            tokio::select!{_ = cancel.cancelled()=>bail!("Child ownership acquisition cancelled"),_ = self.owner.wait_for_idle()=>{}}
        }
        bail!("Child stayed busy through3ownership waits; contract not installed")
    }
    /// The Host retains this exact task. Dropping a waiter cannot strand publication.
    fn begin_publication(&self,items:Vec<WorkPoolYieldItem>)->Result<tokio::task::JoinHandle<Result<ContractSnapshot>>>{
        let ticket=self.owner.reserve_items(items)?;
        let owner=self.owner.clone();let session_id=self.id.clone();let handle=self.handle().clone();
        Ok(tokio::spawn(async move {
            owner.publish(ticket.clone(),move|snapshot|{
                let session_id=session_id.clone();let handle=handle.clone();let ticket=ticket.clone();
                async move{
                    let result=handle.call(rpc_host::NativeAction::ChildInstall{session_id,ticket,snapshot}).await
                        .map_err(|e|ChildContractError(format!("Child Native publication response: {e}")))?;
                    serde_json::from_value(result["systemPrompt"].clone()).map_err(|e|ChildContractError(format!("Child publication receipt lacks actual prompt: {e}")))
                }
            }).await.map_err(Into::into)
        }))
    }
    pub async fn install_items(&self,items:Vec<WorkPoolYieldItem>,cancel:&CancellationToken)->Result<ContractSnapshot>{
        self.acquire_idle(cancel).await?;
        let publication=self.begin_publication(items)?;
        tokio::select!{
            _=cancel.cancelled()=>bail!("Child publication wait cancelled; retained publication continues and must settle"),
            result=publication=>result.context("Retained child publication task failed")?,
        }
    }
    pub async fn clear_items(&self,cancel:&CancellationToken)->Result<ContractSnapshot>{self.install_items(Vec::new(),cancel).await}
    /// Concrete producer for initial/followup turns. Competing ordinary actor commands use the same dispatch lease.
    pub async fn followup(&self,text:String,items:Vec<WorkPoolYieldItem>,cancel:&CancellationToken)->Result<ObservedChildResult>{
        let _serial=tokio::select!{_ = cancel.cancelled()=>bail!("Child followup cancelled before ownership"),guard=self.serial.lock()=>guard};
        let mut initial_attempts=0;
        'initial_dispatch: loop {
        let mut installed=None;
        for _ in 0..3 {
            let candidate=self.install_items(items.clone(),cancel).await?;
            self.handle().call(rpc_host::NativeAction::State).await?;
            if !self.handle().closed_signal().is_cancelled()&&self.owner.published_snapshot().generation==candidate.generation{installed=Some(candidate);break;}
        }
        let installed=installed.context("Child replaced during all3install/revalidate rounds")?;
        let turn=ChildTurn::new(installed.generation,!items.is_empty());
        *turn.classification_input.lock().unwrap_or_else(|e|e.into_inner())=self.solution_space.clone();
        *self.last_turn.lock().unwrap_or_else(|e|e.into_inner())=Some(turn.clone());
        initial_attempts+=1;
        let mut next=Some(text.clone());let mut reminders=0;
        loop {
            #[cfg(feature="test-fixture")]
            if !turn.admitted(){let gate=self.dispatch_race.lock().unwrap_or_else(|e|e.into_inner()).clone();if let Some(gate)=gate{gate.pause_before_send(turn.clone()).await?;}}
            if let Err(error)=self.handle().call(rpc_host::NativeAction::ChildPrompt{session_id:self.id.clone(),text:next.take(),turn:turn.clone()}).await {
                if turn.busy_not_dispatched(&self.id,installed.generation){
                    // executor2459-2473: exactly three total INITIAL attempts.
                    if initial_attempts>=3{return Err(error.context("Child initial dispatch stayed busy through 3 total attempts"));}
                    // A never admitted an observer/lease. B retains its own entire Run.
                    // Rust reserve_items forbids changing its live contract: join first.
                    self.acquire_idle(cancel).await?;
                    self.clear_items(cancel).await?;
                    continue 'initial_dispatch;
                }
                if !turn.admitted(){return Err(error.context("Child dispatch outcome unknown or rejected; do not replay without its retained receipt"));}
                // A lost actor reply may postdate actual admission. Reconcile this retained owner; never replay.
            }
            // Existing actor idle receipt covers all quota/header/maintenance continuations.
            // It also detects a cancelled recovery with no final Run, without redispatching.
            let settled=tokio::select!{
                _=cancel.cancelled()=>Err(anyhow::anyhow!("Child turn wait cancelled; admitted effects require reconciliation")),
                result=self.handle().call(rpc_host::NativeAction::WaitIdle)=>result.map(|_|()),
            }.and_then(|_|turn.require_settled());
            if let Err(error)=settled {
                // Fixed executor2646-2700 returns a failed outcome for a known
                // terminal model error. The Native handle also reports that
                // error through WaitIdle; preserve the exact settled result.
                if !cancel.is_cancelled()&&turn.require_settled().is_ok(){
                    if let Some(model_error)=turn.known_model_error(){
                        let mut observed=turn.finalize(true)?;
                        observed.error=Some(model_error);
                        return Ok(observed);
                    }
                }
                self.native.bash_session.effects().cancel();let _=self.handle().call(rpc_host::NativeAction::Abort).await;turn.fail(error.to_string());return Err(error);
            }
            if !turn.clean(){return turn.finalize(true)}
            let effects=self.native.bash_session.effects();
            let snapshots=self.native.bash_session.snapshots();
            let pending=effects.pending_count()!=0||snapshots.iter().any(|job|job.status=="running"||job.result.is_none());
            if pending {
                let receipt=turn.finalize(false)?;
                if receipt.status!=ara_cli::child_contract::ChildResultStatus::PendingOwnedWork{bail!("Child result bypassed actual owned-job barrier");}
            }
            let receipts=tokio::select!{
                _ = cancel.cancelled()=>{effects.cancel();let _=self.handle().call(rpc_host::NativeAction::Abort).await;bail!("Child owned work cancelled; result remains unaccepted")},
                receipts=effects.settle()=>receipts,
            };
            if receipts.iter().any(|receipt|receipt.uncertain()){turn.fail("Child owned effect outcome is unknown".into());bail!("Child owned effect outcome is unknown; no accepted result");}
            let _settled_snapshot=self.native.bash_session.snapshots();
            let deliveries=self.native.bash_session.pending_deliveries();
            if !deliveries.is_empty(){
                // Existing CliHooks.asides adopts these exact Bash receipts into this same journal.
                // The old yield cannot speak for newly delivered results; demand a fresh one.
                turn.observer()?.invalidate_for_async_work();
                turn.prepare_continuation(false)?;next=None;continue;
            }
            let observer=turn.observer()?;
            if observer.has_terminal_yield()||!self.owner.config().structured{return turn.finalize(true)}
            if reminders>=3 {
                let mut result=turn.finalize(true)?;
                let warning="SYSTEM WARNING: Subagent exited without calling yield tool after 3 reminders.";
                result.error=Some(match result.error{Some(error)=>format!("{warning}\n{error}"),None=>warning.into()});return Ok(result);
            }
            reminders+=1;
            let reminder=ara_cli::child_contract::prompt::yield_reminder(reminders,3,true)?;
            turn.prepare_continuation(reminders==3)?;next=Some(reminder);
        }
        }
    }

    pub fn last_turn(&self)->Option<Arc<ChildTurn>>{self.last_turn.lock().unwrap_or_else(|e|e.into_inner()).clone()}
    pub async fn close(mut self)->Result<i32>{
        self.native.task_session.close();
        self.native.wait_session.close();
        self.native.bash_session.close();
        let runtime=self.runtime.take().context("Child Host already closed")?;
        runtime.handle.close_admission();
        #[cfg(feature="test-fixture")]
        super::peer_host::fixture_frontier("child-close-before-join",&self.id).await?;
        let code=runtime.close().await?;
        let receipts=self.native.bash_session.effects().settle().await;
        if receipts.iter().any(|receipt|receipt.uncertain()){bail!("Child close retained an unknown owned-effect outcome");}
        Ok(code)
    }
}

impl Drop for ChildHost {fn drop(&mut self){if let Some(runtime)=&self.runtime{self.native.task_session.close();runtime.handle.close_admission();self.native.bash_session.close();}}}

/// Parent identity/policy is read from its actual Host journal, never supplied by child params.
pub(super) struct ChildHostFactory<'a>{template:&'a rpc_host::SessionFactory,parent:&'a SessionJournal,depth:usize}
impl<'a> ChildHostFactory<'a>{
    pub fn new(template:&'a rpc_host::SessionFactory,parent:&'a SessionJournal)->Result<Self>{
        if parent.header()["cwd"].as_str().map(Path::new).is_none_or(|cwd|std::fs::canonicalize(cwd).ok()!=std::fs::canonicalize(&template.cwd).ok()){bail!("Parent Host cwd and actual journal differ");}
        let depth=parent.latest_child_init()?.and_then(|init|init.extra.get("araChildDepth").and_then(Value::as_u64)).unwrap_or(0)as usize;
        Ok(Self{template,parent,depth})
    }
    fn authorize_spawn(&self,agent:&str)->Result<()>{
        if let Some(parent)=self.parent.latest_child_init()? {
            let policy=parent.spawns.as_deref().unwrap_or("").trim();
            if policy!="*"&&!policy.split(',').map(str::trim).any(|allowed|!allowed.is_empty()&&allowed==agent){bail!("Parent child contract does not authorize spawning {agent:?}");}
        }
        Ok(())
    }
    pub async fn create(&self,mut init:PersistedChildInit,prompt:ara_cli::child_contract::prompt::ChildPromptInputs,structured:bool,cancel:&CancellationToken)->Result<ChildHost>{
        self.authorize_spawn(init.agent.as_deref().unwrap_or("task"))?;
        if structured&&!init.tools.iter().any(|name|name=="yield"){init.tools.push("yield".into());}
        init.extra.insert("araChildPromptBase".into(),json!(prompt.base));
        init.extra.insert("araChildPromptInputs".into(),json!({"agent":prompt.agent,"context":prompt.context,"planReference":prompt.plan_reference,"planReferencePath":prompt.plan_reference_path,"worktree":prompt.worktree,"outputSchemaOverridesAgent":prompt.output_schema_overrides_agent}));
        init.extra.insert("araChildDepth".into(),json!(self.depth+1));
        init.extra.insert("araChildParentId".into(),json!(self.parent.session_id()));
        let directory=if self.template.peers.is_some(){self.parent.path().with_extension("")}else{self.template.dir.clone().unwrap_or_else(||default_session_dir(&self.template.cwd))};
        let mut journal=SessionJournal::create_with_parent(&directory,&self.template.cwd,Some(&self.parent.path().to_string_lossy()))?;
        journal.bind_blob_directory(&ara_blobs_directory())?;
        let seed=prepare_child_seed(self.template,&self.template.cwd,init,Some(journal),false,None,cancel).await?;
        seed.spawn(self.parent.session_id().into(),self.depth+1).await
    }
    pub async fn create_isolated(&self,mut init:PersistedChildInit,prompt:ara_cli::child_contract::prompt::ChildPromptInputs,structured:bool,lease:Arc<ara_cli::task_isolation::IsolationLease>,cancel:&CancellationToken)->Result<ChildHost>{
        // No runtime exists until seed.spawn. Pre-spawn errors are known stopped,
        // and can finalize the actual backend; post-spawn errors retain uncertainty.
        let prepared=async {
            self.authorize_spawn(init.agent.as_deref().unwrap_or("task"))?;
            init.isolated=Some(true);if structured&&!init.tools.iter().any(|name|name=="yield"){init.tools.push("yield".into());}
            init.extra.insert("araChildPromptBase".into(),json!(prompt.base));
            init.extra.insert("araChildPromptInputs".into(),json!({"agent":prompt.agent,"context":prompt.context,"planReference":prompt.plan_reference,"planReferencePath":prompt.plan_reference_path,"worktree":lease.cwd().to_string_lossy(),"outputSchemaOverridesAgent":prompt.output_schema_overrides_agent}));
            init.extra.insert("araChildDepth".into(),json!(self.depth+1));init.extra.insert("araChildParentId".into(),json!(self.parent.session_id()));
            let directory=self.parent.path().with_extension("");
            let mut journal=SessionJournal::create_with_parent(&directory,lease.cwd(),Some(&self.parent.path().to_string_lossy()))?;journal.bind_blob_directory(&ara_blobs_directory())?;
            let id=journal.session_id().to_owned();lease.bind_public_agent_id(&id)?;
            let peers=self.template.peers.as_ref().context("Task requires actual Peer owner")?;let root=self.template.peer_root.as_deref().unwrap_or(self.parent.session_id());
            let seed=prepare_child_seed(self.template,lease.cwd(),init,Some(journal),false,None,cancel).await?;
            peers.reserve_task_resource(root,&id,lease.clone())?;
            Ok::<_,anyhow::Error>((seed,id))
        }.await;
        let (seed,id)=match prepared{Ok(value)=>value,Err(error)=>{let cleanup=lease.finalize(Ok(())).await;return Err(error.context(format!("Task preparation failed; known stopped isolation receipt: {cleanup:?}")));}};
        match seed.spawn(self.parent.session_id().into(),self.depth+1).await {
            Ok(child)=>Ok(child),Err(error)=>{
                if let Some(peers)=&self.template.peers{peers.cancel_task_reservation(self.template.peer_root.as_deref().unwrap_or(self.parent.session_id()),&id,&lease);}
                let _=lease.finalize(Err(format!("Task startup/join outcome requires reconciliation: {error}"))).await;Err(error)
            }
        }
    }
    pub async fn open(&self,file:&Path,cancel:&CancellationToken)->Result<ChildHost>{self.open_expected(file,None,cancel).await}
    pub async fn open_peer(&self,record:ara_cli::host_peers::PeerRecord,cancel:&CancellationToken)->Result<ChildHost>{
        let file=record.session_file.clone().context("Peer cold restore requires its exact original transcript")?;
        self.open_expected(&file,Some(ara_cli::host_peers::AttachmentExpectation::Parked(record)),cancel).await
    }
    pub async fn open_warm_isolated(&self,record:ara_cli::host_peers::PeerRecord,lease:Arc<ara_cli::task_isolation::IsolationLease>,cancel:&CancellationToken)->Result<ChildHost>{
        let file=record.session_file.clone().context("Warm Task original transcript missing")?;
        let result=self.open_expected_owned(&file,Some(ara_cli::host_peers::AttachmentExpectation::Parked(record)),Some(lease.clone()),cancel).await;
        if let Err(error)=&result{lease.retain_unsettled_writer(&format!("Warm Task preparation/spawn failed; actual replacement writer settlement requires reconciliation: {error:#}"))?;}
        result
    }
    async fn open_expected(&self,file:&Path,expected:Option<ara_cli::host_peers::AttachmentExpectation>,cancel:&CancellationToken)->Result<ChildHost>{self.open_expected_owned(file,expected,None,cancel).await}
    async fn open_expected_owned(&self,file:&Path,expected:Option<ara_cli::host_peers::AttachmentExpectation>,lease:Option<Arc<ara_cli::task_isolation::IsolationLease>>,cancel:&CancellationToken)->Result<ChildHost>{
        if !file.is_file(){bail!("Child transcript is missing; no replacement conversation was created");}
        let journal=SessionJournal::open_with_blob_directory(file,&ara_blobs_directory())?;
        let init=journal.latest_child_init()?.context("Child transcript has no session_init")?;
        if init.isolated==Some(true)&&lease.is_none(){bail!("Isolated child transcript remains history-only");}
        if !journal.entries().iter().any(|entry|entry.kind=="message"||entry.kind=="custom_message"){bail!("Child transcript has no conversational history");}
        let cwd=PathBuf::from(journal.header()["cwd"].as_str().context("Child cwd missing")?);
        if !cwd.is_dir(){bail!("Child workspace no longer exists");}
        if let Some(lease)=&lease{anyhow::ensure!(std::fs::canonicalize(&cwd)?==std::fs::canonicalize(lease.cwd())?,"Warm isolated Task cwd/resource differs");let Some(ara_cli::host_peers::AttachmentExpectation::Parked(record))=&expected else{bail!("Warm isolated restore requires exact parked generation")};anyhow::ensure!(self.template.peers.as_ref().and_then(|p|p.task_resource(&record.key)).is_some_and(|actual|Arc::ptr_eq(&actual,lease)),"Warm Task lease belongs to another activation");}
        let parent=journal.header()["parentSession"].as_str().context("Child transcript lacks parent ownership")?;
        if std::fs::canonicalize(parent)?!=std::fs::canonicalize(self.parent.path())?{bail!("Child transcript belongs to a different actual parent");}
        self.authorize_spawn(init.agent.as_deref().unwrap_or("task"))?;
        let id=journal.session_id().to_owned();let expected_file=journal.path().to_owned();
        let display_name=match &expected {Some(ara_cli::host_peers::AttachmentExpectation::Parked(record))=>Some(record.display_name.clone()),_=>None};
        let mut seed=prepare_child_seed(self.template,&cwd,init,Some(journal),true,display_name.as_deref(),cancel).await?;
        seed.sessions.peer_attachment=expected;
        if seed.header["id"].as_str()!=Some(id.as_str())||seed.journal.as_ref().map(|j|j.path())!=Some(expected_file.as_path()){bail!("Child cold restore changed transcript identity");}
        seed.spawn(self.parent.session_id().into(),self.depth+1).await
    }
}

struct ChildSeed {config:AgentConfig,skills:Vec<ara_discovery::LoadedSkill>,messages:Vec<AgentMessage>,journal:Option<SessionJournal>,header:Value,max_time:Option<f64>,sessions:rpc_host::SessionFactory,native:Arc<native_setup::NativeCliOwner>}
impl ChildSeed{
    async fn spawn(self,parent_id:String,depth:usize)->Result<ChildHost>{
        let id=self.header["id"].as_str().context("Child header id missing")?.to_owned();
        let file=self.journal.as_ref().context("Actual child journal missing")?.path().to_owned();
        let owner=self.native.child_contract.clone().context("Actual child contract owner missing")?;
        let native=self.native.clone();
        let solution_space=self.journal.as_ref().map(|journal|journal.latest_child_init()).transpose()?.flatten()
            .and_then(|init|init.extra.get("araTaskSolutionSpace").and_then(Value::as_str).map(str::to_owned));
        let receipt_id=id.clone();
        let runtime=rpc_host::spawn_native_session(self.config,self.skills,self.messages,self.journal,self.header,self.max_time,self.sessions,Arc::new(move|frame|{
            #[cfg(feature="test-fixture")]
            if let Some(directory)=std::env::var_os("ARA_TEST_CHILD_ARTIFACT_DIR"){
                use std::io::Write;
                let path=PathBuf::from(directory).join(format!("{receipt_id}.events.jsonl"));
                let mut output=std::fs::OpenOptions::new().append(true).create(true).open(path).expect("fixture raw child event file");
                writeln!(output,"{}",frame.stringify()).expect("fixture raw child event receipt");
            }
            #[cfg(not(feature="test-fixture"))]
            let _=(&receipt_id,frame);
        }));
        if let Err(error)=runtime.handle.call(rpc_host::NativeAction::State).await {let _=runtime.close().await;return Err(error.context("Child Native actor failed before adoption"));}
        Ok(ChildHost{id,file,parent_id,depth,owner,native,solution_space,runtime:Some(runtime),#[cfg(feature="test-fixture")]dispatch_race:Mutex::new(None),serial:tokio::sync::Mutex::new(()),last_turn:Mutex::new(None)})
    }
}

pub(super) fn prompt_for_snapshot(init:&PersistedChildInit,snapshot:&ContractSnapshot,structured:bool)->Result<Vec<String>>{
    let Some(base)=init.extra.get("araChildPromptBase") else{return Ok(init.system_prompt.clone())};
    let base:Vec<String>=serde_json::from_value(base.clone()).context("Invalid actual child prompt base metadata")?;
    let inputs=init.extra.get("araChildPromptInputs");
    let field=|key:&str|inputs.and_then(|v|v.get(key)).and_then(Value::as_str).unwrap_or("").to_owned();
    ara_cli::child_contract::prompt::ChildPromptInputs{base,agent:field("agent"),context:field("context"),plan_reference:field("planReference"),plan_reference_path:field("planReferencePath"),worktree:field("worktree"),structured,output_schema:init.output_schema.clone(),output_schema_overrides_agent:inputs.and_then(|v|v.get("outputSchemaOverridesAgent")).and_then(Value::as_bool).unwrap_or(false)}.render(&snapshot.items).map_err(Into::into)
}

/// Observe current configured eligibility without running a helper or refreshing login.
fn child_route_has_auth(route:&Route,args:&Args,agent:&rpc_host::HostAgentContext,account:Option<&Arc<ara_cli::auth_storage::AuthStorage>>)->Result<bool>{
    if agent.auth_scope.oauth_account_pools.contains_key(&route.model.provider) {
        let Some(account)=account else{return Ok(false)};
        let context=agent.auth_scope.context_for(&route.model.provider,&ara_cli::auth_storage::AuthRequestContext::default());
        return Ok(account.has_auth_with_context(&route.model.provider,&context)?);
    }
    use ara_cli::{daily_model_config::DailyAuthSource,model_route::{CredentialIdentity,is_credential_header}};
    let Some(registry)=&route.registry else{return Ok(true)};
    let reference=match route.daily.as_ref().and_then(|selection|selection.catalog_model.clone()){
        Some(model)=>Some(model),None=>registry.find_reference(&route.model.provider.as_str().into(),&route.model.id.as_str().into())?,
    };
    let configured=reference.as_ref().is_some_and(|model|registry.has_configured_auth(model))
        ||registry.is_keyless_provider(&route.model.provider.as_str().into());
    // Keep actual explicit CLI/environment/header sources as selected by the same
    // daily resolver; catalog presence alone must not discard their current owner.
    let explicit=args.api_key_env.as_ref().is_some_and(|name|std::env::var(name).is_ok_and(|value|!value.is_empty()))
        ||args.headers.iter().filter_map(|header|header.split_once(':')).any(|(name,value)|is_credential_header(name.trim())&&!value.trim().is_empty())
        ||route.daily.as_ref().is_some_and(|selection|match &selection.auth_source{
        DailyAuthSource::Fixed(lease)=>lease.identity()!=&CredentialIdentity::Keyless,
        DailyAuthSource::Configured(spec)=>spec.base.identity()!=&CredentialIdentity::Keyless||spec.key_config.is_some()
            ||spec.header_sources.iter().flatten().chain(spec.cli_headers.iter()).any(|(name,_)|is_credential_header(name)),
        DailyAuthSource::OpenAiCodex=>false,
    });
    Ok(configured||explicit)
}

async fn prepare_child_seed(template:&rpc_host::SessionFactory,cwd:&Path,mut init:PersistedChildInit,stored:Option<SessionJournal>,cold:bool,display_name:Option<&str>,connection:&CancellationToken)->Result<ChildSeed>{
    let parent_native=template.tool_owner.native.as_ref().context("Actual parent Native owner missing")?;
    let actual_id=stored.as_ref().context("Child creation/open must own its actual journal")?.session_id().to_owned();
    let raw_agent=init.agent.clone();
    let normalized=raw_agent.as_deref().map(ara_prompt::js::trim).unwrap_or_default();
    let effective_name=raw_agent.as_ref().filter(|raw|!raw.is_empty()&&!normalized.eq_ignore_ascii_case("main")&&!normalized.eq_ignore_ascii_case("sub"))
        .cloned().unwrap_or_else(||display_name.unwrap_or(&actual_id).to_owned());
    let pools=rpc_host_settings::AgentAccountPools::load(cwd,&ara_home().join("agent"))?.for_definition(raw_agent.as_deref()).unwrap_or_default();
    let agent_context=Arc::new(rpc_host::HostAgentContext{
        identity:rpc_host::HostAgentIdentity{definition_name:raw_agent,effective_name},
        auth_scope:Arc::new(ara_cli::auth_storage::AuthSessionScope{primary_session_id:actual_id,oauth_account_pools:pools}),
    });
    let current=template.provider.route.model();let current_key=format!("{}/{}",current.provider,current.id);
    // Catalog identity can differ from requestModelId; raw wire IDs stay opaque.
    let catalog_id=template.provider.metadata.as_ref().and_then(|metadata|metadata["catalogModelId"].as_str()).unwrap_or(&current.id);
    let catalog=template.provider.registry.as_ref().map(|registry|registry.find_reference(&current.provider.as_str().into(),&catalog_id.into())).transpose()?.flatten();
    let catalog_key=match catalog.as_ref(){Some(model)=>Some(format!("{}/{}",startup_selection::model_text(model,"provider")?,startup_selection::model_text(model,"id")?)),None=>None};
    let policy=rpc_host_settings::RetryPolicy::load(&ara_home().join("agent"))?;
    let role=init.model_role.as_deref().filter(|role|*role!="default").and_then(|role|policy.model_role(role));
    let mut selectors=Vec::new();
    if let Some(patterns)=init.extra.get("araTaskModelPatterns").and_then(Value::as_array){
        for pattern in patterns.iter().filter_map(Value::as_str){
            if let Some(role)=pattern.strip_prefix('@'){if let Some(model)=policy.model_role(role){selectors.push(model);}}
            else{selectors.push(pattern.to_owned());}
        }
    }
    selectors.extend(role.into_iter().chain(init.resolved_model.clone()));selectors.dedup();
    if selectors.is_empty(){bail!("Child contract requires a currently resolvable model");}
    let mut resolved=None;let mut failures=Vec::new();
    // Fixed persisted-revive.ts180-183/212 and sdk.ts3073-3088 keep the saved
    // selector after an unavailable role. Each candidate uses current registry/auth.
    for selected in selectors {
        let mut args=template.args.clone();args.cwd=Some(cwd.to_owned());args.mode=Mode::Rpc;args.resume=None;args.continue_session=false;
        if let Some(thinking)=init.extra.get("araTaskThinking").and_then(Value::as_str){args.thinking=vec![thinking.to_owned()];}
        if let Some(milliseconds)=init.extra.get("araTaskMaxRuntimeMs").and_then(Value::as_f64).filter(|value|value.is_finite()&&*value>0.0){args.max_time=Some(milliseconds/1000.0);}
        let same_current=selected==current_key||selected==current.id||catalog_key.as_deref()==Some(selected.as_str());
        let force_catalog=same_current&&catalog_key.is_some();
        if same_current {args.model=Some(catalog_key.clone().unwrap_or_else(||current.id.clone()));args.provider=Some(current.provider.clone());}
        else {args.model=Some(selected.clone());args.provider=None;args.base_url=None;args.api=None;args.api_key_env=None;args.headers.clear();}
        let mut shared_account=template.provider.shared_account.clone();
        match resolve_startup_route_scoped(&mut args,cwd,&mut shared_account,connection,stored.as_ref(),Some(agent_context.clone()),template.provider.registry.clone(),force_catalog).await {
            Ok(route)=>match child_route_has_auth(&route,&args,&agent_context,shared_account.as_ref()){
                Ok(true)=>{resolved=Some((args,route,shared_account));break;}
                Ok(false)=>failures.push(format!("{selected}: current model has no configured authorization")),
                Err(error)=>failures.push(format!("{selected}: current authorization observation failed: {error:#}")),
            },
            Err(error)=>failures.push(format!("{selected}: {error:#}")),
        }
    }
    let (mut args,mut route,shared_account)=resolved.with_context(||format!("No child role or saved model resolved through current registry/auth: {}",failures.join("; ")))?;
    let requested=init.tools.iter().filter(|name|name.as_str()!="yield"&&(init.read_only!=Some(true)||name.as_str()!="write")).cloned().collect::<std::collections::HashSet<_>>();
    let allowed_builtins=parent_native.builtin.iter().filter(|member|requested.contains(&member.binding.capability.definition().name)).map(|member|member.binding.capability.definition().name).collect::<Vec<_>>();
    args.tools=Some(allowed_builtins.join(","));
    let selected_api=args.api();
    let session_dir=if template.peers.is_some(){stored.as_ref().and_then(|journal|journal.path().parent()).context("Peer child journal parent missing")?.to_owned()}else{args.session_dir.clone().unwrap_or_else(||default_session_dir(cwd))};
    let mut journal=stored.context("Child creation/open must own its actual journal")?;
    journal.bind_blob_directory(&ara_blobs_directory())?;
    if cold {recover_session(&mut journal)?;init=journal.latest_child_init()?.context("Child init vanished after actual open")?;}
    let selected_key=match route.daily.as_ref().and_then(|selection|selection.catalog_model.as_ref()) {
        Some(model)=>format!("{}/{}",startup_selection::model_text(model,"provider")?,startup_selection::model_text(model,"id")?),
        None=>format!("{}/{}",route.model.provider,route.model.id),
    };
    if journal.current_model()?.as_deref()!=Some(selected_key.as_str()){journal.append_model_change(&selected_key)?;}
    let structured=init.tools.iter().any(|name|name=="yield");
    let mut setup=Box::pin(prepare_cli_setup(&args,cwd,&route.model,route.daily.as_ref().map(|s|&s.metadata),&None,None,ToolOverlay{peer_coordination:template.peers.is_some(),peer_task_depth:init.extra.get("araChildDepth").and_then(Value::as_u64).unwrap_or(0)as usize,peer_enable_irc:Some(template.peer_messaging_enabled),read_host:template.tool_owner.read_host.clone(),read_video:template.tool_owner.read_video.clone(),child_read_summarize:init.read_summarize,child_restrict_lsp:init.restrict_tool_names==Some(true),..Default::default()})).await?;
    let coordination=template.peers.is_some()&&setup.tool_owner.context.as_ref().is_some_and(|context|context.write_transport_policy().device_only)&&allowed_builtins.contains(&"read".to_owned());
    setup.tool_owner.owned_builtins.retain(|tool|allowed_builtins.contains(&tool.definition().name)||(coordination&&tool.definition().name=="write"));
    setup.tools=setup.tool_owner.owned_builtins.clone();
    let restricted=init.restrict_tool_names==Some(true);
    setup.tool_owner.owned_mcp=if restricted{Vec::new()}else{template.tool_owner.owned_mcp.iter().filter(|tool|requested.contains(&tool.definition().name)).cloned().collect()};
    setup.tools.extend(setup.tool_owner.owned_mcp.iter().cloned().map(|tool|tool as Arc<dyn ara_agent::AgentTool>));
    let captured_rpc=if restricted{Vec::new()}else{parent_native.rpc.iter().filter(|member|requested.contains(&member.binding.capability.definition().name)).map(|member|(member.binding.capability.clone(),member.owner.clone())).collect::<Vec<_>>()};
    setup.tools.extend(captured_rpc.iter().map(|(tool,_)|tool.clone()));
    let draft=ara_cli::native_factory::PreparedNativeSession::prepare()?;
    let items=init.work_pool_yield_items.clone().unwrap_or_default().into_iter().map(|item|WorkPoolYieldItem{id:item.id,index:item.index}).collect::<Vec<_>>();
    let base=init.extra.get("araChildPromptBase").cloned().map(serde_json::from_value).transpose()?.unwrap_or_else(||init.system_prompt.clone());
    let owner=ChildContractOwner::new(draft.values().clone(),ChildContractConfig{system_prompt:base,output_schema:init.output_schema.clone(),output_schema_mode:init.output_schema_mode.clone().unwrap_or_else(||"permissive".into()),structured,work_pool_items:items})?;
    let prompt=if cold{init.system_prompt.clone()}else{prompt_for_snapshot(&init,&owner.published_snapshot(),structured)?};
    setup.system_prompt=prompt.clone();
    let mut options=native_setup_options(&setup,&draft,&captured_rpc)?;
    options.grants.builtin=setup.tool_owner.owned_builtins.iter().map(|tool|{
        let kind=native_setup::builtin_kind(tool)?;
        if kind==ara_cli::actual_tools::BuiltinKind::Write && setup.tool_owner.context.as_ref().is_some_and(|context|context.write_transport_policy().device_only) {
            parent_native.builtin_grant(ara_cli::actual_tools::BuiltinKind::Read)?.context("Child coordination transport needs current parent READ grant")
        } else {parent_native.builtin_grant(kind)?.context("Child builtin absent from actual parent source")}
    }).collect::<Result<Vec<_>>>()?;
    options.grants.mcp=setup.tool_owner.owned_mcp.iter().map(|tool|parent_native.mcp_grant(tool)?.context("Child MCP owner absent from actual parent source")).collect::<Result<Vec<_>>>()?;
    options.grants.rpc=captured_rpc.iter().map(|(_,owner)|parent_native.rpc_grant(owner)?.context("Child RPC owner absent from actual parent source")).collect::<Result<Vec<_>>>()?;
    let publication=native_setup::prepare_cli_native_child(&mut setup,draft,captured_rpc,cwd,&route.model,options,owner)?;
    let native=publication.native_owner.clone();let prepared=publication.prepared.clone();
    // A foreign/cold init owns finalized blocks, including any existing device docs.
    if cold {setup.system_prompt=init.system_prompt.clone();}
    native.child_contract.as_ref().context("Prepared child owner missing")?.initialize_published_prompt(setup.system_prompt.clone())?;
    if let Some(read)=&setup.tool_owner.read_session{read.bind_session(journal.session_id()).map_err(|e|anyhow::Error::msg(e.0))?;}
    if let Some(edit)=&setup.tool_owner.edit_session{edit.bind(&journal,ara_cli::session_artifacts::SessionArtifacts::for_journal(&journal))?;}
    prepared.publish_catalog_snapshot(publication.pending_snapshot.clone(),None)?;
    bind_native_journal(&setup.projector,&mut journal,prepared.values())?;
    init.system_prompt=setup.system_prompt.clone();init.resolved_model=Some(selected_key);
    init.tools=setup.tools.iter().map(|tool|tool.definition().name).filter(|name|init.read_only!=Some(true)||name!="write").collect();
    if journal.latest_child_init()?.as_ref()!=Some(&init){journal.append_child_init(&init)?;}
    journal.materialize()?;
    let header=journal.header().clone();
    let mut provider=if let Some(selection)=route.daily.take(){ProviderFactory::daily_scoped(template.provider.client.clone(),selection,header["id"].as_str().map(str::to_owned),cwd,connection,shared_account.clone(),route.registry.clone(),Some(agent_context.clone())).await?}else{ProviderFactory::startup(template.provider.client.clone(),route.model.clone(),selected_api,route.stream_options,args.anthropic_strict_tools,args.responses_stateful)?};
    provider.agent_context=Some(agent_context.clone());
    // Fixed tui/thinking.ts295: coarse effort chooses the ordered supported
    // range's low / lower middle / high. No surface means no invented override.
    if let Some(effort)=init.extra.get("araTaskEffort").and_then(Value::as_str){
        if let Some(policy)=provider.route.reasoning_policy(){
            let supported=policy.supported_efforts();
            if !supported.is_empty(){
                let index=match effort{"lo"=>0,"med"=>(supported.len()-1)/2,"hi"=>supported.len()-1,_=>bail!("Invalid Task effort selector")};
                let ceiling=init.extra.get("araTaskEffortCeiling").and_then(Value::as_str).and_then(ara_ai::ReasoningEffort::parse).unwrap_or(ara_ai::ReasoningEffort::Max);
                let allowed=supported.iter().rev().find(|candidate|**candidate<=ceiling).copied().context("Task effort ceiling is below this model's minimum supported effort")?;
                let selected=supported[index].min(allowed);args.thinking=vec![selected.as_str().into()];
                if let Some(startup)=route.startup.as_mut(){startup.thinking=ara_cli::session_thinking::parse_cli(selected.as_str());}
            }
        }
    }
    if provider.shared_account.is_none(){provider.shared_account=shared_account;}
    let messages=provider.context_for(&journal,&setup.projector)?;
    let config=AgentConfig{model:route.model,provider:provider.build(),system_prompt:setup.system_prompt,tools:setup.tools,tool_choice:None,max_tokens:args.max_tokens,temperature:args.temperature,deadline:None,max_model_calls:args.max_model_calls,hooks:setup.hooks,interrupt_mode:rpc_host_settings::load_interrupt_mode(&ara_home().join("agent"))?};
    let max_time=args.max_time;
    let sessions=rpc_host::SessionFactory{peer_messaging_enabled:template.peer_messaging_enabled,agent_context:Some(agent_context),dir:Some(session_dir),cwd:cwd.to_owned(),provider,args,explicit_generation:template.explicit_generation.clone(),mcp_config:None,tool_owner:setup.tool_owner,uri_port:template.tool_owner.uri_base.clone(),startup:route.startup,context_controller:Some(setup.context_controller),actual_values_factory:Some(Arc::new(||Ok(rpc_host::PreparedActualSession::Native(ara_cli::native_factory::PreparedNativeSession::prepare()?)))),initial_native_session:Some(prepared),native_jobs:template.native_jobs.clone(),peers:template.peers.clone(),peer_root:template.peer_root.clone(),peer_attachment:None,acp_plan:None};
    Ok(ChildSeed{config,skills:setup.skills,messages,journal:Some(journal),header,max_time,sessions,native})
}


/// Fixture entry exercises the production factory and typed actor; no public RPC extension.
#[cfg(feature="test-fixture")]
pub(super) async fn run_fixture(sessions:rpc_host::SessionFactory,journal:Option<SessionJournal>,scenario:&Path)->Result<i32>{
    let mut parent=journal.context("Child fixture requires actual parent journal")?;
    parent.materialize()?;
    let factory=ChildHostFactory::new(&sessions,&parent)?;
    let operations:Vec<Value>=serde_json::from_slice(&std::fs::read(scenario)?)?;
    let cancel=CancellationToken::new();
    let mut children=std::collections::HashMap::<String,ChildHost>::new();
    let mut files=std::collections::HashMap::<String,PathBuf>::new();
    let mut receipts=Vec::<Value>::new();
    let mut original_transcripts=std::collections::HashMap::<String,Vec<u8>>::new();
    for (index,operation) in operations.into_iter().enumerate(){
        let alias=operation["alias"].as_str().unwrap_or("child").to_owned();
        let action=operation["op"].as_str().context("Child fixture op missing")?.to_owned();
        let outcome:Result<Value>=async {
            match action.as_str(){
                "create"=>{
                    let mut init:PersistedChildInit=serde_json::from_value(operation["init"].clone())?;
                    if init.resolved_model.is_none(){let model=sessions.provider.route.model();init.resolved_model=Some(format!("{}/{}",model.provider,model.id));}
                    let prompt=ara_cli::child_contract::prompt::ChildPromptInputs{base:init.system_prompt.clone(),agent:init.agent.clone().unwrap_or_default(),context:operation["context"].as_str().unwrap_or("").into(),plan_reference:operation["planReference"].as_str().unwrap_or("").into(),plan_reference_path:operation["planReferencePath"].as_str().unwrap_or("").into(),worktree:operation["worktree"].as_str().unwrap_or("").into(),output_schema_overrides_agent:operation["schemaOverrides"].as_bool().unwrap_or(false),..Default::default()};
                    if children.contains_key(&alias){bail!("Fixture would overwrite live child");}
                    let child=factory.create(init,prompt,operation["structured"].as_bool().unwrap_or(true),&cancel).await?;
                    files.insert(alias.clone(),child.file.clone());let result=json!({"id":child.id,"file":child.file,"parentId":child.parent_id,"depth":child.depth});children.insert(alias.clone(),child);Ok(result)
                }
                "revoke_parent"=>{
                    let name=operation["tool"].as_str().context("Revoke fixture tool missing")?;
                    let native=sessions.tool_owner.native.as_ref().context("Fixture parent Native source missing")?;
                    let member=native.builtin.iter().find(|member|member.binding.capability.definition().name==name).context("Revoke fixture exact parent builtin missing")?;
                    native.snapshot.builtin().set_granted(member.kind,false)?;
                    Ok(json!({"tool":name,"currentGrant":native.builtin_grant(member.kind)?}))
                }
                "open"=>{if children.contains_key(&alias){bail!("Fixture child already live");}let child=factory.open(files.get(&alias).context("No prior actual child file")?,&cancel).await?;let result=json!({"id":child.id,"file":child.file});children.insert(alias.clone(),child);Ok(result)}
                "close"=>{let child=children.remove(&alias).context("Fixture child missing")?;let id=child.id.clone();let file=child.file.clone();let code=child.close().await?;Ok(json!({"id":id,"file":file,"code":code}))}
                "inspect"=>{
                    let file=files.get(&alias).context("Fixture child path missing")?;
                    let journal=SessionJournal::open_with_blob_directory(file,&ara_blobs_directory())?;
                    Ok(json!({"id":journal.session_id(),"file":file,"header":journal.header(),"init":journal.latest_child_init()?,"rawJournal":std::fs::read_to_string(file)?}))
                }
                "last_turn_receipt"=>{
                    let child=children.get(&alias).context("Fixture child missing")?;
                    Ok(child.last_turn().context("Fixture has no retained turn")?.admission_json())
                }
                "dispatch_race"=>{
                    let child=children.get(&alias).context("Fixture child missing")?;
                    let collisions=operation["collisions"].as_u64().unwrap_or(1) as usize;
                    if collisions>3{bail!("Fixture supports only the source-bounded three initial attempts");}
                    let close_before_send=operation["closeBeforeSend"].as_bool().unwrap_or(false);
                    let (sender,mut attempts)=tokio::sync::mpsc::unbounded_channel::<DispatchRaceAttempt>();
                    let gate=Arc::new(DispatchRaceGate{sender,attempts:std::sync::atomic::AtomicUsize::new(0)});
                    *child.dispatch_race.lock().unwrap_or_else(|e|e.into_inner())=Some(gate.clone());
                    let handle=child.handle().clone();let driver_handle=handle.clone();let owner=child.owner.clone();
                    let directory=PathBuf::from(std::env::var_os("ARA_TEST_CHILD_ARTIFACT_DIR").context("Race fixture artifacts required")?);
                    let receipts=Arc::new(Mutex::new(Vec::<Value>::new()));let driver_receipts=receipts.clone();
                    let race_cancel=cancel.child_token();let driver_cancel=race_cancel.clone();
                    let driver=async move{
                        let driven:Result<()>=async {
                            if close_before_send {
                                let attempt=attempts.recv().await.context("Fixture initial dispatch never reached actual gate")?;
                                driver_handle.close_admission();attempt.ready.send(()).map_err(|_|anyhow::anyhow!("Fixture A stopped before closed-handle send"))?;
                                return Ok(());
                            }
                            for index in 0..collisions {
                                let attempt=attempts.recv().await.context("Fixture initial retry never reached actual gate")?;
                                let acknowledgement=driver_handle.call(rpc_host::NativeAction::Prompt{text:format!("Competing B ordinary run {}",index+1),images:Vec::new()}).await?;
                                attempt.ready.send(()).map_err(|_|anyhow::anyhow!("Fixture A dropped its dispatch pause"))?;
                                tokio::time::timeout(Duration::from_secs(10),attempt.turn.wait_busy_receipt()).await.context("Actual actor busy receipt never arrived")??;
                                let receipt=json!({"round":index+1,"stage":"actual-actor-busy-before-dispatch","aReceipt":attempt.turn.admission_json(),"bAcknowledgement":acknowledgement});
                                {use std::io::Write;let mut output=std::fs::OpenOptions::new().append(true).create(true).open(directory.join("dispatch-race.jsonl"))?;writeln!(output,"{}",receipt)?;}
                                driver_receipts.lock().unwrap_or_else(|e|e.into_inner()).push(receipt);
                                // The test's HTTP gate releases B only after A's real rejection.
                                std::fs::write(directory.join(format!("race-release-{}",index+1)),b"actual busy receipt recorded")?;
                                driver_handle.call(rpc_host::NativeAction::WaitIdle).await?;
                                if owner.dispatch_active(){bail!("Competing B idle receipt leaked its actual dispatch lease");}
                            }
                            if collisions<3 {let attempt=attempts.recv().await.context("A failed to retry after known busy rejection")?;attempt.ready.send(()).map_err(|_|anyhow::anyhow!("Fixture A stopped before winning retry"))?;}
                            Ok(())
                        }.await;
                        if driven.is_err(){driver_cancel.cancel();let _=driver_handle.call(rpc_host::NativeAction::Abort).await;}
                        driven
                    };
                    let text=operation["text"].as_str().unwrap_or("Lost race assignment A").to_owned();
                    let joined=tokio::time::timeout(Duration::from_secs(30),async{tokio::join!(child.followup(text,Vec::new(),&race_cancel),driver)}).await;
                    *child.dispatch_race.lock().unwrap_or_else(|e|e.into_inner())=None;
                    let (outcome,driver_result)=match joined{Ok(result)=>result,Err(error)=>{race_cancel.cancel();let _=handle.call(rpc_host::NativeAction::Abort).await;let _=tokio::time::timeout(Duration::from_secs(5),handle.call(rpc_host::NativeAction::WaitIdle)).await;return Err(error.into());}};
                    driver_result?;
                    let attempts=gate.attempts.load(std::sync::atomic::Ordering::Acquire);
                    let retained=child.last_turn().map(|turn|turn.admission_json());let raw_receipts=receipts.lock().unwrap_or_else(|e|e.into_inner()).clone();
                    let result=match outcome {
                        Ok(observed)=>{
                            let data=observed.data.as_ref().map(|data|->Result<Value>{let values=child.owner.values();let text=values.heap().stringify(data,ara_rpc::value_heap::StringifyMode::Plain,None,values.runtime())?.context("Race data is not JSON serializable")?;Ok(serde_json::from_str(&text.to_utf8()?)?)}).transpose()?;
                            json!({"succeeded":true,"accepted":observed.accepted,"status":format!("{:?}",observed.status),"data":data,"error":observed.error})
                        }
                        Err(error)=>json!({"succeeded":false,"accepted":false,"error":format!("{error:#}")}),
                    };
                    Ok(json!({"id":child.id,"file":child.file,"attempts":attempts,"retained":retained,"busyReceipts":raw_receipts,"outcome":result}))
                }
                "report_extractor_bank"=>{
                    let mut observations=serde_json::Map::new();
                    for actual_graph in [false,true] {
                        let projector=ara_cli::agent_message_projection::AgentMessageProjector::default();
                        let mut scratch=SessionJournal::in_memory(parent.header().clone())?;
                        if actual_graph {let native=sessions.tool_owner.native.as_ref().context("Extractor fixture Native parent missing")?;bind_native_journal(&projector,&mut scratch,native.prepared.values())?;}
                        let turn=ChildTurn::new(0,false);let mut candidates=Vec::<Value>::new();
                        for bodies in operation["turns"].as_array().context("Extractor fixture turns missing")? {
                            let mut messages=Vec::new();
                            for blocks in bodies.as_array().context("Extractor fixture messages missing")? {
                                let mut assistant=ara_ai::AssistantMessage::empty("openai-completions","controlled-source-fixture","source-only");
                                assistant.content=serde_json::from_value(blocks.clone())?;
                                let message=Message::Assistant(assistant);
                                messages.push(if actual_graph{projector.new_legacy_message(serde_json::to_value(message)?,None)?}else{AgentMessage::from(message)});
                            }
                            let report=ara_agent::RunReport{messages,end:RunEnd::Completed,stop_cause:None};
                            turn.prepare_host_recovery();turn.finish_run(true,false,report_update(&report,&projector)?);
                            let candidate=turn.state.lock().unwrap_or_else(|e|e.into_inner()).last_report.clone();
                            candidates.push(candidate.map(|value|value.to_utf8()).transpose()?.map(Value::String).unwrap_or(Value::Null));
                        }
                        observations.insert(if actual_graph{"graphCandidates"}else{"typedCandidates"}.into(),Value::Array(candidates));
                    }
                    Ok(json!({"boundary":"controlled RunReport typed and actual Graph source state; no native yield, model or acceptance proof","candidates":observations["graphCandidates"],"typedCandidates":observations["typedCandidates"]}))
                }
                "append_init"=>{
                    if children.contains_key(&alias){bail!("Fixture init append requires a closed actual child");}
                    let file=files.get(&alias).context("Fixture child path missing")?;
                    let mut journal=SessionJournal::open_with_blob_directory(file,&ara_blobs_directory())?;
                    let mut value=serde_json::to_value(journal.latest_child_init()?.context("Fixture child init missing")?)?;
                    let object=value.as_object_mut().context("Fixture init object missing")?;
                    for (key,value) in operation.get("patch").and_then(Value::as_object).into_iter().flatten(){object.insert(key.clone(),value.clone());}
                    if let Some(remove)=operation["removeFields"].as_array(){for field in remove{object.remove(field.as_str().context("Fixture remove field must be a string")?);}}
                    let init:PersistedChildInit=serde_json::from_value(value)?;
                    let entry=journal.append_child_init(&init)?;
                    Ok(json!({"id":journal.session_id(),"file":file,"entry":entry,"init":journal.latest_child_init()?,"rawJournal":std::fs::read_to_string(file)?}))
                }
                "transcript_fault"=>{
                    if children.contains_key(&alias){bail!("Fixture transcript fault requires a closed actual child");}
                    let file=files.get(&alias).context("Fixture child path missing")?;
                    let kind=operation["kind"].as_str().context("Fixture transcript fault kind missing")?;
                    let before=std::fs::read(file).ok();
                    let hash=|bytes:&[u8]|ring::digest::digest(&ring::digest::SHA256,bytes).as_ref().iter().map(|b|format!("{b:02x}")).collect::<String>();
                    if kind=="restore" {
                        std::fs::write(file,original_transcripts.get(&alias).context("Fixture original transcript missing")?)?;
                    }else{
                        let bytes=before.clone().context("Fixture actual transcript missing before fault")?;
                        original_transcripts.entry(alias.clone()).or_insert_with(||bytes.clone());
                        match kind {
                            "missing"=>std::fs::remove_file(file)?,
                            "truncated"=>std::fs::write(file,b"{\"type\":\"session\"")?,
                            "wrong-cwd"=>{
                                let text=String::from_utf8(bytes)?;let mut lines=Vec::new();let mut changed=false;
                                for line in text.lines(){
                                    if let Ok(mut record)=serde_json::from_str::<Value>(line)&&record["type"]=="session" {
                                        record["cwd"]=json!(sessions.cwd.join("fixture-missing-child-cwd"));lines.push(serde_json::to_string(&record)?);changed=true;
                                    }else{lines.push(line.to_owned());}
                                }
                                if !changed{bail!("Fixture did not find actual Session header");}
                                std::fs::write(file,format!("{}\n",lines.join("\n")))?;
                            }
                            _=>bail!("Unknown fixture transcript fault {kind}"),
                        }
                    }
                    let after=std::fs::read(file).ok();
                    Ok(json!({"file":file,"kind":kind,"beforeHash":before.as_deref().map(hash),"beforeLength":before.as_ref().map(Vec::len),"afterHash":after.as_deref().map(hash),"afterLength":after.as_ref().map(Vec::len)}))
                }
                "journal_path_fault"=>{
                    let child=children.get(&alias).context("Fixture journal fault requires a live actual child")?;
                    let file=&child.file;let saved=file.with_extension("jsonl.fixture-saved");
                    let hash=|path:&Path|->Result<String>{let bytes=std::fs::read(path)?;Ok(ring::digest::digest(&ring::digest::SHA256,&bytes).as_ref().iter().map(|b|format!("{b:02x}")).collect())};
                    let enabled=operation["enable"].as_bool().context("Fixture journal fault enable missing")?;
                    let before_hash;
                    if enabled {
                        if !file.is_file()||saved.exists(){bail!("Fixture journal fault targets are not pristine");}
                        before_hash=hash(file)?;std::fs::rename(file,&saved)?;
                        if let Err(error)=std::fs::create_dir(file){std::fs::rename(&saved,file)?;return Err(error.into());}
                    }else{
                        if !file.is_dir()||!saved.is_file(){bail!("Fixture journal fault lacks exact saved file/blocking directory");}
                        before_hash=hash(&saved)?;std::fs::remove_dir(file)?;std::fs::rename(&saved,file)?;
                    }
                    let actual=if enabled{&saved}else{file};
                    Ok(json!({"id":child.id,"file":file,"saved":saved,"enabled":enabled,"beforeHash":before_hash,"afterHash":hash(actual)?,"bytes":std::fs::metadata(actual)?.len()}))
                }
                "wait_idle"=>{
                    let child=children.get(&alias).context("Fixture child missing")?;
                    child.acquire_idle(&cancel).await?;Ok(json!({"id":child.id,"dispatchActive":child.owner.dispatch_active()}))
                }
                "install"|"clear"|"drop_install_waiter"=>{
                    let child=children.get(&alias).context("Fixture child missing")?;
                    let items:Vec<WorkPoolYieldItem>=if action=="clear"{Vec::new()}else{serde_json::from_value(operation["items"].clone())?};
                    let snapshot=if action=="drop_install_waiter"{
                        child.acquire_idle(&cancel).await?;
                        let retained=child.begin_publication(items)?;
                        drop(retained); // Same publication task continues; never resubmit unknown effects.
                        child.owner.when_settled().await;
                        if let Some(error)=child.owner.publication_error(){bail!(error);}
                        child.owner.published_snapshot()
                    }else{child.install_items(items,&cancel).await?};
                    Ok(json!({"id":child.id,"generation":snapshot.generation,"items":snapshot.items,"systemPrompt":snapshot.system_prompt,"dispatchActive":child.owner.dispatch_active()}))
                }
                "ordinary_probe"=>{
                    let child=children.get(&alias).context("Fixture child missing")?;
                    let turn=ChildTurn::new(child.owner.published_snapshot().generation,false);
                    child.handle().call(rpc_host::NativeAction::ChildPrompt{session_id:child.id.clone(),text:Some("Ordinary wake must defer while pool is installed".into()),turn}).await
                }
                "followup"=>{
                    let child=children.get(&alias).context("Fixture child missing")?;
                    let items=serde_json::from_value(operation.get("items").cloned().unwrap_or(json!([])))?;
                    let turn_cancel=cancel.child_token();
                    if let Some(delay)=operation["cancelAfterMs"].as_u64(){let signal=turn_cancel.clone();tokio::spawn(async move{tokio::time::sleep(Duration::from_millis(delay)).await;signal.cancel();});}
                    let observed=child.followup(operation["text"].as_str().unwrap_or("Perform child task").into(),items,&turn_cancel).await?;
                    let data=observed.data.as_ref().map(|data|->Result<Value>{let values=child.owner.values();let wire=values.heap().stringify(data,ara_rpc::value_heap::StringifyMode::Plain,None,values.runtime())?.context("Fixture data is not JSON serializable")?;Ok(serde_json::from_str(&wire.to_utf8()?)?)}).transpose()?;
                    Ok(json!({"id":child.id,"file":child.file,"status":format!("{:?}",observed.status),"accepted":observed.accepted,"error":observed.error,"data":data,"schemaOverridden":observed.schema_overridden}))
                }
                _=>bail!("Unknown child fixture op {action}"),
            }
        }.await;
        let expected_error=operation["expectedError"].as_bool().unwrap_or(false);
        let receipt=match outcome {Ok(result)=>{if expected_error{bail!("Child fixture {index} unexpectedly succeeded");}json!({"index":index,"alias":alias,"op":action,"success":true,"result":result})},Err(error)=>{if !expected_error{return Err(error.context(format!("Child fixture operation {index} {action}")));}json!({"index":index,"alias":alias,"op":action,"success":false,"error":format!("{error:#}")})}};
        if let Some(directory)=std::env::var_os("ARA_TEST_CHILD_ARTIFACT_DIR") {use std::io::Write;let mut output=std::fs::OpenOptions::new().append(true).create(true).open(PathBuf::from(directory).join("operations.jsonl"))?;writeln!(output,"{}",receipt)?;}
        receipts.push(receipt);
    }
    for (_,child) in children{child.close().await?;}
    println!("CHILD_FIXTURE_RECEIPTS={}",serde_json::to_string(&receipts)?);
    Ok(0)
}
