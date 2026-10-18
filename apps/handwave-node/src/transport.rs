#[path = "message_queue.rs"]
pub(crate) mod message_queue;

use crate::{
    agent::{self, RunRequest},
    config::Resolved,
    connection,
    operations::{self, Packet, Receipts, Sender, Upload},
    wire::{self, Frame},
};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle, time::Instant};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

struct Active {
    frame: Frame,
    cancel: CancellationToken,
    job: Option<JoinHandle<()>>,
    ending: Arc<AtomicBool>,
    grant_until: Instant,
    acks: Option<mpsc::Sender<()>>,
    steering: Arc<Mutex<message_queue::SteeringBuffer>>,
    lease: Option<crate::goal_runtime::Lease>,
}

// The workbench creates one local card before an agent-loop request is sent.
// Keep every update on that identity; task_id still identifies the individual run.
fn local_agent_event(mut event: Value) -> Value {
    event["agent_id"] = json!("local");
    event["agent_name"] = json!("本机");
    event["agent_role"] = json!("operator");
    event
}

async fn run_event(sender: &Sender, run: &Active, event: Value) -> Result<()> {
    if run.ending.load(Ordering::SeqCst) {
        return Ok(());
    }
    let frame = &run.frame;
    let event = if frame.k == "agent-loop" { local_agent_event(event) } else { event };
    sender.event(&frame.id, event).await
}

async fn reject_agent(sender: &Sender, id: &str, status: &str, error: &str, admitted: bool) -> Result<()> {
    sender.event(id, local_agent_event(json!({"type":"agent_end","status":status,"detail":error,"code":-1}))).await?;
    if admitted {
        sender.terminal(id, &[], -1, error).await
    } else {
        // A rejected packet has no authority to settle an active run.
        sender.output(id, &[], -1, error).await
    }
}

// Only the first abort before a model task exists owns the missing terminal.
// A timeout/denial may already have queued its terminal while awaiting drainage.
fn claim_pending_grant_cancel(run: &Active) -> bool {
    run.frame.k == "agent-loop" && run.job.is_none() && !run.ending.swap(true, Ordering::SeqCst)
}

async fn stop(active: &mut Option<Active>) -> Result<()> {
    if let Some(mut run) = active.take() {
        run.cancel.cancel();
        if let Some(job) = run.job.take() {
            // Tools own cancellation and process-tree cleanup. Do not detach them before a new owner.
            tokio::time::timeout(Duration::from_secs(20), job)
                .await
                .context("run cleanup did not settle; refusing to reconnect")?
                .context("run cleanup task failed; refusing to reconnect")?;
        }
        run.steering.lock().unwrap().finish();
    }
    Ok(())
}

fn job_manager(settings: &Resolved, lease: &crate::goal_runtime::Lease) -> Result<crate::managed_jobs::Manager> {
    let state = settings.session_path_for(&lease.controller, &lease.session)?;
    crate::managed_jobs::Manager::new(
        &state,
        &settings.cwd,
        crate::managed_jobs::Policy {
            owner: crate::managed_jobs::Owner::new(&state, &settings.fp, &lease.session)?,
            goal_id: lease.goal_id.clone(),
            lease_expires_at: lease.expires_at,
            max_jobs: 8,
            max_log_bytes: 8 * 1024 * 1024,
            max_seconds: 3600,
        },
    )
}

fn cancel_goal_jobs(settings: &Resolved, owner: &str, id: &str, goals: &mut crate::goal_runtime::Runner) -> Result<()> {
    // Persist the dispatch fence first; cancellation markers request cleanup,
    // never claim that a worker or its external effects are already gone.
    let contracts = goals.matching_contracts(owner, id);
    goals.cancel_matching(owner, id)?;
    for lease in contracts {
        if lease.allow_jobs {
            job_manager(settings, &lease)?.cancel_all()?;
        }
        let path = settings.session_path_for(owner, &lease.session)?;
        let _ = agent::suspend_goal(&path, &settings.cwd, &lease.goal_id, "canceled");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn finalize_retired(
    socket: &mut connection::Socket,
    retired: &mut HashMap<String, Frame>,
    settings: &Resolved,
    owner: &str,
    receipts: &mut Option<Receipts>,
    goals: &mut crate::goal_runtime::Runner,
) -> Result<()> {
    for (id, frame) in retired.drain() {
        if frame.k == "agent-loop" {
            socket
                .send(Message::Text(json!({"k":"model-grant-end","run":id,"fp":settings.fp}).to_string().into()))
                .await?;
        }
        if let Some(receipts) = receipts.as_mut() {
            receipts.finish(&id)?;
        }
        if frame.k == "agent-loop" {
            let path = settings.session_path_for(owner, &frame.session_key)?;
            if let Ok(Some(goal)) = agent::session_goal(&path, &settings.cwd) {
                goals.observe(&frame.session_key, &goal)?;
                socket.send(Message::Text(json!({"k":"goal-state","writer_epoch":goals.writer_epoch,"session_key":frame.session_key,"goal":goal}).to_string().into())).await?;
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn goal_command(
    sig: &Value,
    settings: &Resolved,
    owner: &str,
    active: &mut Option<Active>,
    goals: &mut crate::goal_runtime::Runner,
    retired: &mut HashMap<String, Frame>,
) -> Result<crate::execution::GoalState> {
    let string = |key: &str| sig[key].as_str().unwrap_or_default();
    let id = string("command_id");
    anyhow::ensure!(!id.is_empty() && id.len() <= 192, "invalid goal command identity");
    let goal_id = string("goal_id");
    let key = string("session_key");
    let command = string("command");
    anyhow::ensure!(matches!(command, "pause" | "resume" | "accept" | "stop"), "unsupported goal command");
    let lease = goals.contract(goal_id, owner, key)?;
    let path = settings.session_path_for(owner, key)?;
    let revision = sig["expected_revision"].as_u64().context("goal command requires expected_revision")?;
    let mut apply_revision = revision;
    let intent_path = settings.state.join(format!("goal-control-{}.json", crate::context_memory::hash(id.as_bytes())));
    if intent_path.exists() {
        let saved: Value = serde_json::from_slice(&std::fs::read(&intent_path)?)?;
        anyhow::ensure!(saved["request"] == *sig, "goal command ID reused with different content");
        apply_revision = saved["apply_revision"]
            .as_u64()
            .context("goal control interrupted while stopping; inspect effects before reconciliation")?;
    }
    if let Some(run) = active.as_ref().filter(|r| r.frame.session_key == key) {
        anyhow::ensure!(
            matches!(command, "pause" | "stop"),
            "goal is running; wait for its settled result before this command"
        );
        let before = agent::session_goal(&path, &settings.cwd)?.context("goal unavailable")?;
        anyhow::ensure!(before.id == goal_id && before.revision == revision, "goal revision changed; refresh the card");
        crate::session_journal::atomic_write(
            &intent_path,
            &serde_json::to_vec(&json!({"request":sig,"apply_revision":null}))?,
        )?;
        run.cancel.cancel();
        let stopped_frame = run.frame.clone();
        let settlement = stop(active).await;
        let cleanup =
            if lease.allow_jobs { job_manager(settings, &lease).and_then(|jobs| jobs.cancel_all()) } else { Ok(()) };
        settlement?;
        cleanup?;
        retired.insert(stopped_frame.id.clone(), stopped_frame);
        let settled = agent::session_goal(&path, &settings.cwd)?.context("goal unavailable after stopping")?;
        anyhow::ensure!(settled.id == goal_id, "goal identity changed while stopping");
        apply_revision = settled.revision;
        crate::session_journal::atomic_write(
            &intent_path,
            &serde_json::to_vec(&json!({"request":sig,"apply_revision":apply_revision}))?,
        )?;
    }
    if matches!(command, "pause" | "stop") && lease.allow_jobs {
        job_manager(settings, &lease)?.cancel_all()?;
    }
    let goal = agent::control_goal(
        &path,
        &settings.cwd,
        goal_id,
        command,
        id,
        Some(apply_revision),
        string("reviewed_run_id"),
    )?;
    goals.observe(key, &goal)?;
    Ok(goal)
}

pub async fn run(settings: Resolved) -> Result<()> {
    let mut goals = crate::goal_runtime::Runner::load(&settings.state)?;
    let quit = CancellationToken::new();
    let signal = quit.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal.cancel();
    });
    let mut backoff = connection::Backoff::default();
    while !quit.is_cancelled() {
        let mut healthy = false;
        let result = session(&settings, quit.clone(), &mut goals, &mut healthy).await;
        if settings.config.once {
            return result;
        }
        match result {
            Ok(()) => {}
            Err(error) => {
                eprintln!("connection ended: {error:#}");
                if error.to_string().contains("refusing to reconnect") {
                    return Err(error);
                }
            }
        }
        if quit.is_cancelled() {
            break;
        }
        let mut random = [0; 8];
        let _ = getrandom::fill(&mut random);
        let delay = backoff.next(healthy, u64::from_le_bytes(random));
        eprintln!("reconnecting in {:.2}s; pending operations are not replayed", delay.as_secs_f64());
        tokio::select! {_=quit.cancelled()=>break,_=tokio::time::sleep(delay)=>{}}
    }
    Ok(())
}

async fn session(
    settings: &Resolved,
    quit: CancellationToken,
    goals: &mut crate::goal_runtime::Runner,
    healthy: &mut bool,
) -> Result<()> {
    let mut socket = connection::Socket::connect(&settings.websocket_url(), &settings.token, quit.clone()).await?;
    crate::node_identity::authenticate(&mut socket, settings, quit.clone()).await?;
    eprintln!("connected; device={} waiting for controller approval", settings.fp);
    let (tx, mut rx) = mpsc::channel::<Packet>(128);
    let mut owner = String::new();
    let mut epoch = 0u64;
    let mut alive = CancellationToken::new();
    let mut active: Option<Active> = None;
    let mut retired = HashMap::<String, Frame>::new();
    let mut upload: Option<Upload> = None;
    let mut queued = message_queue::Queue::new();
    let mut receipts: Option<Receipts> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut heartbeat = connection::Heartbeat::new(Instant::now());
    let result:Result<()> = async {
        loop {
            tokio::select!{
                _=quit.cancelled()=>break,
                _=tick.tick()=>{
                    if let Some(payload) = heartbeat.tick(Instant::now())? { socket.send(Message::Ping(payload.into())).await?; }
                    for receipt in queued.drain_outcomes(){send_queue_frame(&mut socket,receipt).await?;}
                    finalize_retired(&mut socket,&mut retired,settings,&owner,&mut receipts,goals).await?;
                    if active.is_none() { for lease in goals.expired_dispatches(&owner) {
                        goals.wait(&lease.goal_id,"waiting_authorization",60)?;
                        let path=settings.session_path_for(&owner,&lease.session)?;
                        if let Ok(goal)=agent::suspend_goal(&path,&settings.cwd,&lease.goal_id,"waiting_authorization") {
                            socket.send(Message::Text(json!({"k":"goal-state","writer_epoch":goals.writer_epoch,"session_key":lease.session,"goal":goal}).to_string().into())).await?;
                        }
                    } }
                    for lease in goals.waiting_jobs(&owner) {
                        match job_manager(settings,&lease).and_then(|jobs|{let receipts=jobs.unfinished()?;anyhow::ensure!(!receipts.iter().any(|r|r.state=="interrupted"),"Job worker interrupted; effects require reconciliation");jobs.ready_to_continue()}) {
                            Ok(true)=>goals.wait(&lease.goal_id,"yielded",0)?,
                            Ok(false)=>goals.wait(&lease.goal_id,"waiting_job",2)?,
                            Err(error)=>{
                                eprintln!("goal Job recovery fenced: {error:#}");goals.wait(&lease.goal_id,"waiting_recovery",60)?;
                                let path=settings.session_path_for(&owner,&lease.session)?;
                                if active.is_none() { if let Ok(goal)=agent::suspend_goal(&path,&settings.cwd,&lease.goal_id,"waiting_recovery") {
                                    socket.send(Message::Text(json!({"k":"goal-state","writer_epoch":goals.writer_epoch,"session_key":lease.session,"goal":goal}).to_string().into())).await?;
                                } }
                            },
                        }
                    }
                    if let Some(run)=active.as_mut() {
                        if !run.ending.load(Ordering::SeqCst) && run.job.is_none() && (run.cancel.is_cancelled() || Instant::now()>=run.grant_until) {
                            // Fence late grants before the terminal packets enter the outbound queue.
                            run.ending.store(true,Ordering::SeqCst);
                            let id=run.frame.id.clone();
                            let error=if run.cancel.is_cancelled(){"cancelled while waiting for model grant"}else{"model grant timeout"};
                            let status=if run.cancel.is_cancelled(){"canceled"}else{"failed"};
                            let sender=Sender{tx:tx.clone(),epoch,alive:alive.clone()};
                            reject_agent(&sender,&id,status,error,true).await?;
                        }
                    }
                    if active.is_none() && !queued.is_empty() && !owner.is_empty(){if let Some(next)=queued.pop_front(){
                        start(next,settings,&owner,epoch,&tx,alive.clone(),&mut active,receipts.as_mut().context("missing receipts")?,&mut queued,&mut socket).await?;
                    }}
                    if active.is_none() && queued.is_empty() && !owner.is_empty() {
                        if let Some(next)=goals.next(&owner)? {
                            start(next,settings,&owner,epoch,&tx,alive.clone(),&mut active,receipts.as_mut().context("missing receipts")?,&mut queued,&mut socket).await?;
                        }
                    }
                },
                outgoing=rx.recv()=>{
                    let Some(packet)=outgoing else {break;};
                    if packet.epoch!=epoch || owner.is_empty(){continue;}
                    for receipt in queued.drain_outcomes(){send_queue_frame(&mut socket,receipt).await?;}
                    let done=packet.settled_id;
                    socket.send(Message::Text(packet.value.to_string().into())).await?;
                    if let Some(id)=done {
                        let finishing_active=active.as_ref().is_some_and(|run|run.frame.id==id);
                        let finished=if finishing_active {active.as_ref().map(|run|run.frame.clone())} else {retired.remove(&id)};
                        if let Some(finished)=finished {
                            if finished.k=="agent-loop" {
                                socket.send(Message::Text(json!({"k":"model-grant-end","run":id,"fp":settings.fp}).to_string().into())).await?;
                            }
                            if finishing_active { stop(&mut active).await?; }
                            for receipt in queued.drain_outcomes(){send_queue_frame(&mut socket,receipt).await?;}
                            if let Some(receipts)=&mut receipts{receipts.finish(&id)?;}
                            if finished.k=="agent-loop" {
                                let path=settings.session_path_for(&owner,&finished.session_key)?;
                                match agent::session_goal(&path,&settings.cwd) {
                                    Ok(Some(goal))=>{
                                        if goal.last_settled_run==finished.id || goal.run==finished.id { goals.observe(&finished.session_key,&goal)?; }
                                        socket.send(Message::Text(json!({"k":"goal-state","writer_epoch":goals.writer_epoch,"session_key":finished.session_key,"goal":goal}).to_string().into())).await?;
                                    },
                                    Ok(None)=>{},
                                    Err(error)=>{
                                        if !finished.goal_id.is_empty(){goals.wait(&finished.goal_id,"waiting_recovery",60)?;}
                                        eprintln!("goal recovery fenced: {error:#}");
                                    },
                                }
                            }
                            if active.is_none() { if let Some(next)=queued.pop_front(){
                                start(next,settings,&owner,epoch,&tx,alive.clone(),&mut active,receipts.as_mut().context("missing receipts")?,&mut queued,&mut socket).await?;
                            } else {queued.sync_active(None);publish_queue(&mut socket,&queued,&active,"",None).await?;} }
                        }
                    }
                },
                message=socket.next()=>{
                    let Some(message)=message else {break;};
                    let message=message?;
                    let raw=match message {
                        Message::Text(text)=>text.as_bytes().to_vec(),
                        Message::Ping(bytes)=>{socket.send(Message::Pong(bytes)).await?;continue;},
                        Message::Pong(bytes)=>{heartbeat.pong(&bytes, Instant::now());continue;},
                        Message::Close(_)=>break,
                        _=>continue,
                    };
                    if raw.len()>wire::MAX_FRAME*2{anyhow::bail!("signal too large");}
                    let sig:Value=serde_json::from_slice(&raw).context("malformed HandWave signal")?;
                    match sig["k"].as_str().unwrap_or_default(){
                        "pair-req"=>{
                            let incoming=sig["ctlfp"].as_str().context("controller identity missing")?;
                            let retain=owner==incoming && active.as_ref().is_some_and(|run|run.lease.as_ref().is_some_and(|lease|
                                lease.controller==owner && lease.session==run.frame.session_key && lease.expires_at>crate::goal_runtime::now()));
                            if retain {
                                queued.renew_epoch();queued.sync_active(active.as_ref().map(|r|&r.frame));
                                // Returning to the workbench observes the same native Run;
                                // it must not revoke an explicit background contract.
                                socket.send(Message::Text(wire::dc(json!({"k":"node-hello","fp":settings.fp,"info":settings.hello()})).to_string().into())).await?;
                                continue;
                            }
                            alive.cancel();
                            stop(&mut active).await?;
                            alive=CancellationToken::new();
                            epoch+=1;queued.reset();upload=None;
                            owner=sig["ctlfp"].as_str().context("controller identity missing")?.into();
                            let state=settings.session_path(&owner)?;
                            receipts=Some(Receipts::load(state.with_extension("receipts.json"))?);
                            for key in goals.sessions(&owner) {
                                let recovered=settings.session_path_for(&owner,&key).and_then(|path|agent::session_goal(&path,&settings.cwd));
                                goals.recover(&key,recovered.as_ref().ok().and_then(|g|g.as_ref()))?;
                            }
                            eprintln!("controller paired: owner={owner}");
                            // The cloud workbench uses relay; also support relay fallback on auto-pair.
                            socket.send(Message::Text(wire::dc(json!({"k":"node-hello","fp":settings.fp,"info":settings.hello()})).to_string().into())).await?;
                        },
                        "unpaired"|"pair-fail"=>{
                            cancel_goal_jobs(settings,&owner,"",goals)?;
                            let stopped_frame=active.as_ref().map(|r|r.frame.clone());
                            let settlement=stop(&mut active).await;
                            let cleanup=cancel_goal_jobs(settings,&owner,"",goals);
                            settlement?;cleanup?;
                            if let Some(frame)=stopped_frame {
                                if frame.k=="agent-loop" { socket.send(Message::Text(json!({"k":"model-grant-end","run":frame.id,"fp":settings.fp}).to_string().into())).await?; }
                                if let Some(receipts)=&mut receipts{receipts.finish(&frame.id)?;}
                            }
                            alive.cancel();epoch+=1;owner.clear();queued.reset();upload=None;receipts=None;retired.clear();
                            eprintln!("controller unpaired; pending effects will not be replayed");
                        },
                        "controller-detached"=>{
                            queued.clear();queued.sync_active(active.as_ref().map(|r|&r.frame));upload=None;
                            let active_authorized=active.as_ref().is_none_or(|run|run.lease.as_ref().is_some_and(|lease|
                                lease.controller==owner && lease.device==settings.fp && lease.session==run.frame.session_key
                                && lease.goal_id==if run.frame.goal_id.is_empty(){run.frame.id.as_str()}else{run.frame.goal_id.as_str()}
                                && lease.expires_at>crate::goal_runtime::now()));
                            if !active_authorized {
                                let stopped_frame=active.as_ref().map(|r|r.frame.clone());stop(&mut active).await?;
                                if let Some(frame)=stopped_frame{retired.insert(frame.id.clone(),frame);}
                                finalize_retired(&mut socket,&mut retired,settings,&owner,&mut receipts,goals).await?;
                            }
                            if goals.has_lease(&owner) {
                                eprintln!("controller detached; continuing only the explicit unexpired goal contract");
                            } else {
                                alive.cancel();stop(&mut active).await?;epoch+=1;owner.clear();queued.clear();receipts=None;
                            }
                        },
                        "job-command"=>{
                            let result=crate::job_control::handle(&sig,settings,&owner,goals).await;
                            let reply=crate::job_control::reply(&sig,&settings.fp,result);
                            socket.send(Message::Text(reply.to_string().into())).await?;
                        },
                        "goal-command"=>{
                            let result=goal_command(&sig,settings,&owner,&mut active,goals,&mut retired).await;
                            if result.as_ref().err().is_some_and(|e|e.to_string().contains("refusing to reconnect")){return Err(result.err().unwrap());}
                            if matches!(sig["command"].as_str(),Some("pause"|"stop")){queued.remove_session(sig["session_key"].as_str().unwrap_or_default());}
                            finalize_retired(&mut socket,&mut retired,settings,&owner,&mut receipts,goals).await?;
                            queued.sync_active(active.as_ref().map(|r|&r.frame));publish_queue(&mut socket,&queued,&active,"",None).await?;
                            let mut reply=json!({"k":"goal-command-result","writer_epoch":goals.writer_epoch,"command_id":sig["command_id"],"goal_id":sig["goal_id"],"session_key":sig["session_key"]});
                            match result {
                                Ok(goal)=>{
                                    reply["goal"]=serde_json::to_value(&goal)?;
                                    socket.send(Message::Text(json!({"k":"goal-state","writer_epoch":goals.writer_epoch,"session_key":sig["session_key"],"goal":goal}).to_string().into())).await?;
                                },
                                Err(error)=>reply["err"]=json!(error.to_string()),
                            }
                            socket.send(Message::Text(reply.to_string().into())).await?;
                        },
                        "model-grant"=>{
                            if owner.is_empty(){continue;}
                            let Some(run)=active.as_mut() else {continue;};
                            if run.ending.load(Ordering::SeqCst) || run.job.is_some() || sig["id"]!=run.frame.id{continue;}
                            if run.cancel.is_cancelled(){
                                run.ending.store(true,Ordering::SeqCst);
                                let sender=Sender{tx:tx.clone(),epoch,alive:alive.clone()};
                                reject_agent(&sender,&run.frame.id,"canceled","cancelled while waiting for model grant",true).await?;
                                continue;
                            }
                            let token=sig["d"].as_str().unwrap_or_default().to_string();
                            let ttl=sig["ttl"].as_u64().unwrap_or(0);
                            if token.is_empty() || ttl<=10 || !sig["err"].as_str().unwrap_or_default().is_empty(){
                                run.ending.store(true,Ordering::SeqCst);
                                let sender=Sender{tx:tx.clone(),epoch,alive:alive.clone()};
                                reject_agent(&sender,&run.frame.id,"failed","model grant denied or too short",true).await?;
                                if !run.frame.goal_id.is_empty(){goals.wait(&run.frame.goal_id,"waiting_authorization",60)?;}
                                continue;
                            }
                            let mut frame=run.frame.clone();
                            let lease:Option<crate::goal_runtime::Lease>=sig.get("goal_lease").filter(|v|!v.is_null()).cloned().map(serde_json::from_value).transpose()?;
                            if let Some(lease)=&lease {
                                let goal_id=if frame.goal_id.is_empty(){&frame.id}else{&frame.goal_id};
                                if let Err(error)=lease.validate(&owner,&settings.fp,&frame.session_key,goal_id,crate::goal_runtime::now()) {
                                    run.ending.store(true,Ordering::SeqCst);
                                    let sender=Sender{tx:tx.clone(),epoch,alive:alive.clone()};
                                    reject_agent(&sender,&frame.id,"failed",&error.to_string(),true).await?;continue;
                                }
                                anyhow::ensure!(frame.execution_mode=="goal","goal lease supplied outside Goal mode");
                                // Hub has verified the original explicit selection or chosen Auto
                                // once; its trusted binding is authority for every slice.
                                frame.smart=true;frame.model.clear();frame.protocol.clear();
                                frame.goal_background=true;
                                goals.register(&frame,lease)?;
                            } else if frame.goal_policy.is_some() || goals.requires_lease(&frame.goal_id) {
                                run.ending.store(true,Ordering::SeqCst);
                                let sender=Sender{tx:tx.clone(),epoch,alive:alive.clone()};
                                reject_agent(&sender,&frame.id,"waiting_authorization","Hub did not provide the requested goal lease",true).await?;
                                goals.wait(&frame.goal_id,"waiting_authorization",60)?;continue;
                            }
                            run.frame=frame.clone();run.lease=lease.clone();
                            let state_path=settings.session_path_for(&owner,&frame.session_key)?;
                            let selection=agent::session_route(&state_path,&settings.cwd)
                                .and_then(|previous|crate::gateway::grant_route(&frame,&sig,&previous));
                            let (route,max_output)=match selection {
                                Ok(selected)=>selected,
                                Err(error)=>{
                                    run.ending.store(true,Ordering::SeqCst);
                                    let sender=Sender{tx:tx.clone(),epoch,alive:alive.clone()};
                                    reject_agent(&sender,&frame.id,"failed",&error.to_string(),true).await?;
                                    continue;
                                }
                            };
                            let decision_budget=if frame.smart {
                                chrono::DateTime::parse_from_rfc3339(sig["bound_decision_exp"].as_str().unwrap_or_default())?
                                    .signed_duration_since(chrono::Utc::now()).num_seconds().max(0) as u64
                            }else{ttl};
                            if decision_budget<=10 {
                                run.ending.store(true,Ordering::SeqCst);
                                let sender=Sender{tx:tx.clone(),epoch,alive:alive.clone()};
                                reject_agent(&sender,&frame.id,"failed","model decision expires too soon",true).await?;
                                continue;
                            }
                            let sender=Sender{tx:tx.clone(),epoch,alive:alive.clone()};
                            let cancel=run.cancel.clone();
                            let ending=run.ending.clone();
                            let resume_job_lease=if matches!(frame.execution_mode.as_str(),""|"execute") {
                                crate::job_control::resume_lease(settings,&owner,&frame.session_key,goals).ok().flatten()
                            } else {None};
                            let request=RunRequest{
                                id:frame.id.clone(),objective:objective(&frame),device_fp:settings.fp.clone(),
                                model:if frame.model.is_empty(){"default".into()}else{frame.model.clone()},
                                endpoint:settings.endpoint(),token,route,max_output,cwd:settings.cwd.clone(),state_path,
                                environment_state:settings.state.clone(),global_knowledge:settings.config.knowledge_dir.clone(),
                                brief:frame.brief.clone(),max_calls:settings.config.max_calls,max_seconds:settings.config.max_seconds.min(ttl.min(decision_budget)-10),
                                steering:run.steering.clone(),
                                mode:crate::execution::RunMode::parse(&frame.execution_mode)?,goal_criteria:frame.goal_criteria.clone(),goal_id:frame.goal_id.clone(),
                                goal_lease:lease,resume_job_lease,session_key:frame.session_key.clone(),expected_revision:frame.expected_revision,reviewed_run_id:frame.reviewed_run_id.clone(),
                            };
                            eprintln!("agent start: id={} model={} budget={}s",frame.id,request.model,request.max_seconds);
                            let writer_epoch=goals.writer_epoch.clone();
                            run.job=Some(tokio::spawn(async move{
                                let (events,mut event_rx)=mpsc::channel::<Value>(64);
                                let forward=sender.clone();let id=frame.id.clone();let forward_ending=ending.clone();let session_key=frame.session_key.clone();
                                let event_task=tokio::spawn(async move{
                                    while let Some(event)=event_rx.recv().await{
                                        if matches!(event["type"].as_str(),Some("goal_update"|"goal_status")) && event.get("goal").is_some()
                                            && forward.sig(json!({"k":"goal-state","writer_epoch":writer_epoch,"session_key":session_key,"goal":event["goal"]})).await.is_err(){break;}
                                        if event["type"]=="agent_end"{forward_ending.store(true,Ordering::SeqCst);}
                                        if forward.event(&id,local_agent_event(event)).await.is_err(){break;}
                                    }
                                });
                                let result=agent::execute(request,cancel.clone(),events).await;
                                let _=event_task.await;
                                match result{
                                    Ok(result)=>{let _=sender.terminal(&frame.id,result.text.as_bytes(),result.code,&result.error).await;},
                                    Err(error)=>{
                                        ending.store(true,Ordering::SeqCst);
                                        let message=format!("{error:#}");
                                        let status=if cancel.is_cancelled(){"canceled"}else{"failed"};
                                        let _=reject_agent(&sender,&frame.id,status,&message,true).await;
                                    }
                                }
                                eprintln!("agent settled: id={}",frame.id);
                            }));
                        },
                        "dc"=>{
                            if owner.is_empty(){continue;}
                            let frame=match wire::decode_dc(&sig){Ok(frame)=>frame,Err(_)=>{continue;}};
                            let sender=Sender{tx:tx.clone(),epoch,alive:alive.clone()};
                            match frame.k.as_str(){
                                "ping"=>sender.frame(json!({"k":"pong"})).await?,
                                "agent-queue"=>{
                                    queued.sync_active(active.as_ref().map(|r|&r.frame));
                                    let live=active.as_ref().is_some_and(|r|!r.ending.load(Ordering::SeqCst)&&!r.cancel.is_cancelled());
                                    let (reply,submitted)=queued.command(&frame,active.as_ref().map(|r|&r.frame),live,active.as_ref().map(|r|&r.steering));
                                    send_queue_frame(&mut socket,reply).await?;
                                    if let Some(receipt)=submitted{send_queue_frame(&mut socket,receipt).await?;}
                                },
                                "bye"=>{
                                    cancel_goal_jobs(settings,&owner,"",goals)?;
                                    let stopped_frame=active.as_ref().map(|r|r.frame.clone());
                                    let settlement=stop(&mut active).await;
                                    let cleanup=cancel_goal_jobs(settings,&owner,"",goals);
                                    settlement?;cleanup?;
                                    if let Some(frame)=stopped_frame {
                                        if frame.k=="agent-loop" { socket.send(Message::Text(json!({"k":"model-grant-end","run":frame.id,"fp":settings.fp}).to_string().into())).await?; }
                                        if let Some(receipts)=&mut receipts{receipts.finish(&frame.id)?;}
                                    }
                                    alive.cancel();epoch+=1;owner.clear();queued.reset();upload=None;receipts=None;retired.clear();
                                    socket.send(Message::Text(json!({"k":"bye"}).to_string().into())).await?;
                                },
                                "cancel"|"agent-abort"|"agent-bash-abort"=>{
                                    let canceled_contracts=goals.matching_contracts(&owner,&frame.id);
                                    cancel_goal_jobs(settings,&owner,&frame.id,goals)?;
                                    if active.as_ref().is_some_and(|run|frame.id.is_empty() || run.frame.id==frame.id) {
                                        if frame.k=="agent-abort" && claim_pending_grant_cancel(active.as_ref().unwrap()) {
                                            let id=active.as_ref().unwrap().frame.id.clone();
                                            reject_agent(&sender,&id,"canceled","cancelled while waiting for model grant",true).await?;
                                        }
                                        active.as_ref().unwrap().cancel.cancel();
                                        let stopped_frame=active.as_ref().unwrap().frame.clone();
                                        let settlement=stop(&mut active).await;
                                        let cleanup=cancel_goal_jobs(settings,&owner,&frame.id,goals);
                                        settlement?;cleanup?;
                                        retired.insert(stopped_frame.id.clone(),stopped_frame);
                                        finalize_retired(&mut socket,&mut retired,settings,&owner,&mut receipts,goals).await?;
                                    } else {
                                        // An idle waiting-Job Goal has no Active/terminal event
                                        // to publish its durable cancellation to the Hub mirror.
                                        for lease in canceled_contracts {
                                            let path=settings.session_path_for(&owner,&lease.session)?;
                                            if let Some(goal)=agent::session_goal(&path,&settings.cwd)?
                                                .filter(|goal|goal.id==lease.goal_id && goal.status=="canceled") {
                                                goals.observe(&lease.session,&goal)?;
                                                socket.send(Message::Text(json!({"k":"goal-state","writer_epoch":goals.writer_epoch,"session_key":lease.session,"goal":goal}).to_string().into())).await?;
                                            }
                                        }
                                    }
                                    queued.clear();queued.sync_active(active.as_ref().map(|r|&r.frame));publish_queue(&mut socket,&queued,&active,"",None).await?;
                                },
                                "agent-steer"=>{
                                    if let Some(run)=&active{
                                        if run.frame.k=="agent-loop" && !frame.msg.trim().is_empty(){
                                            if (!frame.session_key.is_empty()&&frame.session_key!=run.frame.session_key)||(!frame.target_run_id.is_empty()&&frame.target_run_id!=run.frame.id)||run.ending.load(Ordering::SeqCst){
                                                sender.output(&frame.id,&[],-1,"steering target changed or is ending").await?;continue;
                                            }
                                            let submitted=run.steering.lock().unwrap().submit_legacy(frame.msg.clone());
                                            if let Err(error)=submitted{sender.output(&frame.id,&[],-1,&error.to_string()).await?;continue;}
                                            run_event(&sender,run,json!({"type":"steer","status":"queued","output":frame.msg})).await?;
                                        }
                                    }else{sender.output(&frame.id,&[],-1,"no active agent run").await?;}
                                },
                                "agent-follow"=>{
                                    let next=match prepare_follow(&frame,active.as_ref().map(|run|&run.frame)){
                                        Ok(next)=>next,
                                        Err(error)=>{reject_agent(&sender,&frame.id,"failed",&error.to_string(),false).await?;continue;}
                                    };
                                    if let Some(run)=&active{if next.id==run.frame.id || queued.iter().any(|queued|queued.id==next.id){run_event(&sender,run,json!({"type":"retry","status":"running","detail":"duplicate follow-up ignored; active run unchanged"})).await?;continue;}}
                                    if let Some(run)=&active{if queued.len()<16{if run.frame.k=="agent-loop"{run_event(&sender,run,json!({"type":"steer","status":"queued","output":"Follow-up queued"})).await?;}match queued.push_back(next){Ok(())=>publish_queue(&mut socket,&queued,&active,"",None).await?,Err(error)=>{sender.output(&frame.id,&[],-1,&error.to_string()).await?;send_queue_frame(&mut socket,queued.snapshot(&frame.session_key,"",Some(&error.to_string()))).await?;}}}else{sender.output(&frame.id,&[],-1,"follow-up queue full").await?;}}
                                    else{start(next,settings,&owner,epoch,&tx,alive.clone(),&mut active,receipts.as_mut().context("missing receipts")?,&mut queued,&mut socket).await?;}
                                },
                                "ack"=>{if let Some(run)=&active{if frame.id==run.frame.id{if let Some(acks)=&run.acks{let _=acks.try_send(());}}}},
                                "put"=>{
                                    if upload.is_some(){sender.output(&frame.id,&[],-1,"an upload is already active").await?;continue;}
                                    match receipts.as_mut().context("missing receipts")?.begin(&frame.id).and_then(|_|Upload::new(&frame,&settings.cwd)){
                                        Ok(put)=>{upload=Some(put);sender.sig(wire::out(&frame.id,&[],false,0,"")).await?;},
                                        Err(error)=>sender.output(&frame.id,&[],-1,&error.to_string()).await?,
                                    }
                                },
                                "chunk"=>{
                                    let Some(put)=upload.as_mut().filter(|put|put.id==frame.id) else {sender.output(&frame.id,&[],-1,"upload not found").await?;continue;};
                                    match put.chunk(&frame).await{
                                        Ok(done)=>{
                                            sender.frame(json!({"k":"ack","id":frame.id})).await?;
                                            if done{receipts.as_mut().unwrap().finish(&frame.id)?;upload=None;sender.output(&frame.id,&[],0,"").await?;}
                                        },
                                        Err(error)=>{upload=None;sender.output(&frame.id,&[],-1,&error.to_string()).await?;}
                                    }
                                },
                                "agent-loop"|"exec"|"agent-bash"|"agent-task"|"get"=>{
                                    if let Some(run)=&active {
                                        if frame.id==run.frame.id || queued.iter().any(|queued|queued.id==frame.id) {
                                            run_event(&sender,run,json!({"type":"retry","status":"running","detail":"duplicate request ignored; active run unchanged"})).await?;
                                        } else if frame.k=="agent-loop" && run.frame.k=="agent-loop" && queued.len()<16 {
                                            let mut next = match prepare_follow(&frame, Some(&run.frame)) {
                                                Ok(next) => next,
                                                Err(error) => { reject_agent(&sender, &frame.id, "failed", &error.to_string(), false).await?; continue; }
                                            };
                                            // agent-loop already owns its controller-issued request ID;
                                            // inherit the active authority without renaming its receipt.
                                            next.id = frame.id.clone();
                                            run_event(&sender,run,json!({"type":"steer","status":"queued","output":"Follow-up queued"})).await?;
                                            match queued.push_back(next){Ok(())=>publish_queue(&mut socket,&queued,&active,"",None).await?,Err(error)=>{sender.output(&frame.id,&[],-1,&error.to_string()).await?;send_queue_frame(&mut socket,queued.snapshot(&frame.session_key,"",Some(&error.to_string()))).await?;}}
                                        }else {sender.output(&frame.id,&[],-1,"node busy; use follow-up or cancel current run").await?;}
                                    }
                                    else{start(frame,settings,&owner,epoch,&tx,alive.clone(),&mut active,receipts.as_mut().context("missing receipts")?,&mut queued,&mut socket).await?;}
                                },
                                _=>sender.output(&frame.id,&[],-1,"unsupported command in the Rust node").await?,
                            }
                        },
                        _=>{},
                    }
                }
            }
        }
        Ok(())
    }.await;
    // First invalidate outbound generation and signal every in-flight task; then await settlement.
    *healthy = socket.was_healthy() && heartbeat.proven();
    alive.cancel();
    stop(&mut active).await?;
    drop(upload);
    let _ = tokio::time::timeout(Duration::from_secs(2), socket.close()).await;
    result
}

fn objective(frame: &Frame) -> String {
    frame.task.as_ref().and_then(|task| task["objective"].as_str()).unwrap_or(&frame.msg).to_string()
}

fn inherit_follow(next: &mut Frame, active: &Frame) {
    next.model = active.model.clone();
    next.protocol = active.protocol.clone();
    next.smart = active.smart;
    next.brief = crate::context_memory::follow_brief(&active.brief);
    next.session_key = active.session_key.clone();
    next.execution_mode = active.execution_mode.clone();
    next.goal_criteria = active.goal_criteria.clone();
    next.goal_background = active.goal_background || active.goal_policy.is_some();
    next.goal_policy = None;
    next.goal_id = if active.execution_mode == "goal" && active.goal_id.is_empty() {
        active.id.clone()
    } else {
        active.goal_id.clone()
    };
}

fn prepare_follow(frame: &Frame, active: Option<&Frame>) -> Result<Frame> {
    if let Some(active) = active {
        anyhow::ensure!(
            frame.execution_mode.is_empty() || frame.execution_mode == active.execution_mode,
            "follow-up mode differs from active run"
        );
        let active_goal = if active.execution_mode == "goal" && active.goal_id.is_empty() {
            active.id.as_str()
        } else {
            active.goal_id.as_str()
        };
        anyhow::ensure!(
            frame.goal_id.is_empty() || frame.goal_id == active_goal,
            "follow-up Goal differs from active run"
        );
    }
    if !frame.session_key.is_empty() {
        if let Some(active) = active {
            anyhow::ensure!(frame.session_key == active.session_key, "follow-up session differs from active run");
        }
        if !frame.id.is_empty() {
            anyhow::ensure!(
                frame.id.len() <= 256
                    && frame.id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
                "invalid explicit follow-up id"
            );
        }
    }
    let mut next = frame.clone();
    next.k = "agent-loop".into();
    if frame.session_key.is_empty() || frame.id.is_empty() {
        next.id = wire::fresh_id();
    }
    if let Some(active) = active {
        inherit_follow(&mut next, active);
    }
    Ok(next)
}

async fn send_queue_frame(socket: &mut connection::Socket, value: Value) -> Result<()> {
    socket.send(Message::Text(wire::dc(value).to_string().into())).await?;
    Ok(())
}
async fn publish_queue(
    socket: &mut connection::Socket,
    queue: &message_queue::Queue,
    _active: &Option<Active>,
    id: &str,
    error: Option<&str>,
) -> Result<()> {
    if !queue.session().is_empty() {
        send_queue_frame(socket, queue.snapshot(queue.session(), id, error)).await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // The connection actor remains the sole dispatch owner.
async fn start(
    mut frame: Frame,
    settings: &Resolved,
    owner: &str,
    epoch: u64,
    tx: &mpsc::Sender<Packet>,
    alive: CancellationToken,
    active: &mut Option<Active>,
    receipts: &mut Receipts,
    queued: &mut message_queue::Queue,
    socket: &mut connection::Socket,
) -> Result<()> {
    if frame.id.is_empty() {
        frame.id = wire::fresh_id();
    }
    queued.sync_active(active.as_ref().map(|r| &r.frame));
    // Even a busy->idle race receives a genuine accepted pending item before
    // it is dequeued. A browser can bind handover only to a known queued ID.
    if frame.k == "agent-loop"
        && !objective(&frame).trim().is_empty()
        && objective(&frame).len() <= message_queue::MAX_MESSAGE_BYTES
    {
        queued.push_back(frame.clone())?;
        publish_queue(socket, queued, active, "", None).await?;
        // This dispatch was already selected by the caller. Other pending items
        // retain their relative order; remove this exact identity, not the head.
        queued.take_dispatch(&frame.id);
    }
    let result = start_inner(frame, settings, owner, epoch, tx, alive, active, receipts, queued.buffer()).await;
    queued.sync_active(active.as_ref().map(|r| &r.frame));
    // Direct socket ordering precedes draining the model-grant/event channel.
    publish_queue(socket, queued, active, "", None).await?;
    result
}

#[allow(clippy::too_many_arguments)] // Connection-scoped ownership and receipts remain explicit.
async fn start_inner(
    mut frame: Frame,
    settings: &Resolved,
    owner: &str,
    epoch: u64,
    tx: &mpsc::Sender<Packet>,
    alive: CancellationToken,
    active: &mut Option<Active>,
    receipts: &mut Receipts,
    queue_buffer: message_queue::SteeringBuffer,
) -> Result<()> {
    let sender = Sender { tx: tx.clone(), epoch, alive };
    if frame.id.is_empty() {
        frame.id = wire::fresh_id();
    }
    if frame.k == "agent-loop" {
        if let Err(error) = crate::execution::RunMode::parse(&frame.execution_mode)
            .and_then(|_| crate::execution::validate_criteria(&frame.goal_criteria))
        {
            reject_agent(&sender, &frame.id, "failed", &error.to_string(), false).await?;
            return Ok(());
        }
    }
    if frame.k == "agent-loop" && objective(&frame).trim().is_empty() {
        reject_agent(&sender, &frame.id, "failed", "objective required", false).await?;
        return Ok(());
    }
    let previous = if frame.k == "agent-loop" {
        let result = settings
            .session_path_for(owner, &frame.session_key)
            .and_then(|path| agent::session_route(&path, &settings.cwd));
        match result {
            Ok(route) => route,
            Err(error) => {
                reject_agent(&sender, &frame.id, "failed", &error.to_string(), false).await?;
                return Ok(());
            }
        }
    } else {
        crate::gateway::Route::default()
    };
    if frame.smart && (!frame.model.trim().is_empty() || !frame.protocol.trim().is_empty()) {
        if frame.k == "agent-loop" {
            reject_agent(&sender, &frame.id, "failed", "smart mode conflicts with explicit model/protocol", false)
                .await?;
        } else {
            sender.output(&frame.id, &[], -1, "smart mode conflicts with explicit model/protocol").await?;
        }
        return Ok(());
    }
    if frame.k == "agent-loop"
        && ((!frame.smart && !previous.binding.is_empty())
            || (frame.smart && !previous.model.is_empty() && previous.binding.is_empty()))
    {
        reject_agent(
            &sender,
            &frame.id,
            "failed",
            "session mode differs from saved binding; use a new state directory",
            false,
        )
        .await?;
        return Ok(());
    }
    if let Err(error) = receipts.begin(&frame.id) {
        if frame.k == "agent-loop" {
            reject_agent(&sender, &frame.id, "failed", &error.to_string(), false).await?;
        } else {
            sender.output(&frame.id, &[], -1, &error.to_string()).await?;
        }
        return Ok(());
    }
    let cancel = CancellationToken::new();
    let steering = Arc::new(Mutex::new(queue_buffer));
    let mut run = Active {
        frame: frame.clone(),
        cancel: cancel.clone(),
        job: None,
        ending: Arc::new(AtomicBool::new(false)),
        grant_until: Instant::now() + Duration::from_secs(12),
        acks: None,
        steering,
        lease: None,
    };
    if frame.k == "agent-loop" {
        let summary: String = objective(&frame).chars().take(2000).collect();
        let lease_goal = if frame.goal_policy.is_some() || frame.goal_background {
            if frame.goal_id.is_empty() {
                frame.id.clone()
            } else {
                frame.goal_id.clone()
            }
        } else {
            String::new()
        };
        sender.sig(json!({"k":"model-grant","id":frame.id,"run":frame.id,"fp":settings.fp,"smart":frame.smart,"model":frame.model,"protocol":frame.protocol,"binding":previous.binding,"brief":summary,"requires_tools":true,"goal_id":lease_goal,"session_key":frame.session_key,"goal_policy":frame.goal_policy})).await?;
    } else {
        let cwd = settings.cwd.clone();
        let (ack_tx, ack_rx) = mpsc::channel(1);
        run.acks = Some(ack_tx);
        run.job = Some(tokio::spawn(async move {
            let result = match frame.k.as_str() {
                "exec" | "agent-bash" => operations::shell(&frame, &cwd, cancel, &sender).await,
                "agent-task" => operations::task(&frame, &cwd, cancel, &sender).await,
                "get" => operations::get_file(&frame, &cwd, cancel, &sender, ack_rx).await,
                _ => unreachable!(),
            };
            if let Err(error) = result {
                let _ = sender.terminal(&frame.id, &[], -1, &error.to_string()).await;
            }
        }));
    }
    eprintln!("run admitted: owner={owner} id={} kind={}", run.frame.id, run.frame.k);
    *active = Some(run);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_follow_keeps_active_session_even_when_ui_selection_changes() {
        let active = Frame {
            id: "first-goal-run".into(),
            execution_mode: "goal".into(),
            goal_criteria: vec!["verified".into()],
            session_key: "workspace-a/session-a".into(),
            model: "model-a".into(),
            brief: "HANDWAVE_CLOUD_SNAPSHOT_V1\n{}\nHANDWAVE_BRIEF\nlegacy".into(),
            ..Frame::default()
        };
        let mut next =
            Frame { session_key: "workspace-b/session-b".into(), model: "model-b".into(), ..Frame::default() };
        inherit_follow(&mut next, &active);
        assert_eq!(next.session_key, "workspace-a/session-a");
        assert_eq!(next.model, "model-a");
        assert_eq!(next.brief, "legacy");
        assert_eq!(next.execution_mode, "goal");
        assert_eq!(next.goal_id, "first-goal-run");
        assert_eq!(next.goal_criteria, vec!["verified"]);
        inherit_follow(&mut next, &Frame::default());
        assert!(next.session_key.is_empty());
    }
    #[test]
    fn explicit_follow_keeps_supplied_id_and_rejects_wrong_session_or_id() {
        let active = Frame { session_key: "workspace/session-a".into(), ..Frame::default() };
        let follow = Frame { id: "FOLLOW_ID_123".into(), session_key: active.session_key.clone(), ..Frame::default() };
        let prepared = prepare_follow(&follow, Some(&active)).unwrap();
        assert_eq!(prepared.id, follow.id);
        assert_eq!(prepared.session_key, active.session_key);
        let wrong = Frame { session_key: "workspace/session-b".into(), ..follow.clone() };
        assert!(prepare_follow(&wrong, Some(&active)).is_err());
        let invalid = Frame { id: "invalid/id".into(), ..follow.clone() };
        assert!(prepare_follow(&invalid, Some(&active)).is_err());
        let overlong = Frame { id: "x".repeat(257), ..follow.clone() };
        assert!(prepare_follow(&overlong, Some(&active)).is_err());
        let legacy = Frame { id: "legacy-id".into(), ..Frame::default() };
        assert_ne!(prepare_follow(&legacy, Some(&active)).unwrap().id, legacy.id);
        let missing = Frame { id: String::new(), ..follow };
        assert!(!prepare_follow(&missing, Some(&active)).unwrap().id.is_empty());
    }
    #[test]
    fn explicit_follow_cannot_change_mode_or_goal_even_through_legacy_session() {
        let active = Frame {
            id: "goal-first".into(),
            session_key: "w/s".into(),
            execution_mode: "goal".into(),
            goal_background: true,
            goal_policy: Some(json!({"max_calls":12})),
            goal_criteria: vec!["verify actual receipts".into()],
            model: "fixed-model".into(),
            ..Frame::default()
        };
        for key in ["w/s", ""] {
            let requested = Frame {
                id: "queued".into(),
                session_key: key.into(),
                execution_mode: "execute".into(),
                ..Frame::default()
            };
            assert!(prepare_follow(&requested, Some(&active)).is_err());
            let requested = Frame { execution_mode: "goal".into(), goal_id: "another-goal".into(), ..requested };
            assert!(prepare_follow(&requested, Some(&active)).is_err());
        }
        let follow = Frame {
            id: "queued".into(),
            session_key: "w/s".into(),
            msg: "continue".into(),
            execution_mode: "goal".into(),
            goal_id: "goal-first".into(),
            ..Frame::default()
        };
        let prepared = prepare_follow(&follow, Some(&active)).unwrap();
        assert_eq!(prepared.goal_id, "goal-first");
        assert_eq!(prepared.model, "fixed-model");
        assert_eq!(prepared.goal_criteria, active.goal_criteria);
        assert!(prepared.goal_background);
        assert!(prepared.goal_policy.is_none());
    }
    #[test]
    fn pending_grant_cancel_terminal_has_one_owner() {
        let mut run = Active {
            frame: Frame { k: "agent-loop".into(), id: "waiting".into(), ..Frame::default() },
            cancel: CancellationToken::new(),
            job: None,
            ending: Arc::new(AtomicBool::new(false)),
            grant_until: Instant::now(),
            acks: None,
            steering: Arc::new(Mutex::new(message_queue::SteeringBuffer::default())),
            lease: None,
        };
        assert!(claim_pending_grant_cancel(&run));
        assert!(!claim_pending_grant_cancel(&run));
        run.ending.store(true, Ordering::SeqCst); // Timeout/denial already owns a queued terminal.
        assert!(!claim_pending_grant_cancel(&run));
        run.ending.store(false, Ordering::SeqCst);
        run.frame.k = "exec".into();
        assert!(!claim_pending_grant_cancel(&run));
        assert!(!run.ending.load(Ordering::SeqCst));
    }
}
