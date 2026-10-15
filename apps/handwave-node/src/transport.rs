use crate::{
    agent::{self, RunRequest},
    config::Resolved,
    operations::{self, Packet, Receipts, Sender, Upload},
    proxy,
    wire::{self, Frame},
};
use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
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
    steering: Arc<Mutex<Vec<String>>>,
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

async fn stop(active: &mut Option<Active>) -> Result<()> {
    if let Some(mut run) = active.take() {
        run.cancel.cancel();
        if let Some(job) = run.job.take() {
            // Tools own cancellation and process-tree cleanup. Do not detach them before a new owner.
            tokio::time::timeout(Duration::from_secs(20), job)
                .await
                .context("run cleanup did not settle; refusing to reconnect")??;
        }
    }
    Ok(())
}

pub async fn run(settings: Resolved) -> Result<()> {
    let quit = CancellationToken::new();
    let signal = quit.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal.cancel();
    });
    let mut backoff = 1u64;
    while !quit.is_cancelled() {
        let since = Instant::now();
        match session(&settings, quit.clone()).await {
            Ok(()) => {}
            Err(error) => {
                eprintln!("connection ended: {error:#}");
                if error.to_string().contains("refusing to reconnect") {
                    return Err(error);
                }
            }
        }
        if settings.config.once || quit.is_cancelled() {
            break;
        }
        if since.elapsed() > Duration::from_secs(20) {
            backoff = 1;
        }
        eprintln!("reconnecting in {backoff}s");
        tokio::select! {_=quit.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(backoff))=>{}}
        backoff = (backoff * 2).min(30);
    }
    Ok(())
}

async fn session(settings: &Resolved, quit: CancellationToken) -> Result<()> {
    let mut socket =
        tokio::time::timeout(Duration::from_secs(20), proxy::connect(&settings.websocket_url(), &settings.token))
            .await??;
    socket.send(Message::Text(json!({"k":"join","fp":settings.fp,"info":settings.hello()}).to_string().into())).await?;
    eprintln!("connected; device={} waiting for controller approval", settings.fp);
    let (tx, mut rx) = mpsc::channel::<Packet>(128);
    let mut owner = String::new();
    let mut epoch = 0u64;
    let mut alive = CancellationToken::new();
    let mut active: Option<Active> = None;
    let mut upload: Option<Upload> = None;
    let mut queued = VecDeque::<Frame>::new();
    let mut receipts: Option<Receipts> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let result:Result<()> = async {
        loop {
            tokio::select!{
                _=quit.cancelled()=>break,
                _=tick.tick()=>{
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
                },
                outgoing=rx.recv()=>{
                    let Some(packet)=outgoing else {break;};
                    if packet.epoch!=epoch || owner.is_empty(){continue;}
                    let done=packet.settled_id;
                    tokio::time::timeout(Duration::from_secs(10),socket.send(Message::Text(packet.value.to_string().into()))).await??;
                    if let Some(id)=done {
                        if active.as_ref().is_some_and(|run|run.frame.id==id){
                            if active.as_ref().is_some_and(|run|run.frame.k=="agent-loop") {
                                socket.send(Message::Text(json!({"k":"model-grant-end","run":id,"fp":settings.fp}).to_string().into())).await?;
                            }
                            stop(&mut active).await?;
                            if let Some(receipts)=&mut receipts{receipts.finish(&id)?;}
                            if let Some(next)=queued.pop_front(){
                                start(next,settings,&owner,epoch,&tx,alive.clone(),&mut active,receipts.as_mut().context("missing receipts")?).await?;
                            }
                        }
                    }
                },
                message=socket.next()=>{
                    let Some(message)=message else {break;};
                    let message=message?;
                    let raw=match message {
                        Message::Text(text)=>text.as_bytes().to_vec(),
                        Message::Ping(bytes)=>{socket.send(Message::Pong(bytes)).await?;continue;},
                        Message::Close(_)=>break,
                        _=>continue,
                    };
                    if raw.len()>wire::MAX_FRAME*2{anyhow::bail!("signal too large");}
                    let sig:Value=serde_json::from_slice(&raw).context("malformed HandWave signal")?;
                    match sig["k"].as_str().unwrap_or_default(){
                        "pair-req"=>{
                            alive.cancel();
                            stop(&mut active).await?;
                            alive=CancellationToken::new();
                            epoch+=1;queued.clear();upload=None;
                            owner=sig["ctlfp"].as_str().context("controller identity missing")?.into();
                            let state=settings.session_path(&owner)?;
                            receipts=Some(Receipts::load(state.with_extension("receipts.json"))?);
                            eprintln!("controller paired: owner={owner}");
                            // The cloud workbench uses relay; also support relay fallback on auto-pair.
                            socket.send(Message::Text(wire::dc(json!({"k":"node-hello","fp":settings.fp,"info":settings.hello()})).to_string().into())).await?;
                        },
                        "unpaired"|"pair-fail"=>{
                            alive.cancel();
                            stop(&mut active).await?;epoch+=1;owner.clear();queued.clear();upload=None;receipts=None;
                            eprintln!("controller unpaired; pending effects will not be replayed");
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
                                continue;
                            }
                            let frame=run.frame.clone();
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
                            let request=RunRequest{
                                id:frame.id.clone(),objective:objective(&frame),device_fp:settings.fp.clone(),
                                model:if frame.model.is_empty(){"default".into()}else{frame.model.clone()},
                                endpoint:settings.endpoint(),token,route,max_output,cwd:settings.cwd.clone(),state_path,
                                brief:frame.brief.clone(),max_calls:settings.config.max_calls,max_seconds:settings.config.max_seconds.min(ttl.min(decision_budget)-10),
                                steering:run.steering.clone(),
                            };
                            eprintln!("agent start: id={} model={} budget={}s",frame.id,request.model,request.max_seconds);
                            run.job=Some(tokio::spawn(async move{
                                let (events,mut event_rx)=mpsc::channel::<Value>(64);
                                let forward=sender.clone();let id=frame.id.clone();let forward_ending=ending.clone();
                                let event_task=tokio::spawn(async move{
                                    while let Some(event)=event_rx.recv().await{
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
                                "bye"=>{
                                    alive.cancel();
                                    stop(&mut active).await?;epoch+=1;owner.clear();queued.clear();upload=None;receipts=None;
                                    socket.send(Message::Text(json!({"k":"bye"}).to_string().into())).await?;
                                },
                                "cancel"|"agent-abort"|"agent-bash-abort"=>{
                                    if let Some(run)=&active{if frame.id.is_empty() || run.frame.id==frame.id{run.cancel.cancel();}}
                                    queued.clear();
                                },
                                "agent-steer"=>{
                                    if let Some(run)=&active{
                                        if run.frame.k=="agent-loop" && !frame.msg.trim().is_empty(){
                                            run.steering.lock().unwrap().push(frame.msg.clone());
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
                                    if let Some(run)=&active{if queued.len()<16{if run.frame.k=="agent-loop"{run_event(&sender,run,json!({"type":"steer","status":"queued","output":"Follow-up queued"})).await?;}queued.push_back(next);}else{sender.output(&frame.id,&[],-1,"follow-up queue full").await?;}}
                                    else{start(next,settings,&owner,epoch,&tx,alive.clone(),&mut active,receipts.as_mut().context("missing receipts")?).await?;}
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
                                            run_event(&sender,run,json!({"type":"steer","status":"queued","output":"Follow-up queued"})).await?;
                                            queued.push_back(frame);
                                        }else {sender.output(&frame.id,&[],-1,"node busy; use follow-up or cancel current run").await?;}
                                    }
                                    else{start(frame,settings,&owner,epoch,&tx,alive.clone(),&mut active,receipts.as_mut().context("missing receipts")?).await?;}
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
    alive.cancel();
    stop(&mut active).await?;
    drop(upload);
    let _ = tokio::time::timeout(Duration::from_secs(2), socket.close(None)).await;
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
}

fn prepare_follow(frame: &Frame, active: Option<&Frame>) -> Result<Frame> {
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

#[allow(clippy::too_many_arguments)] // Connection-scoped ownership and receipts remain explicit.
async fn start(
    mut frame: Frame,
    settings: &Resolved,
    owner: &str,
    epoch: u64,
    tx: &mpsc::Sender<Packet>,
    alive: CancellationToken,
    active: &mut Option<Active>,
    receipts: &mut Receipts,
) -> Result<()> {
    let sender = Sender { tx: tx.clone(), epoch, alive };
    if frame.id.is_empty() {
        frame.id = wire::fresh_id();
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
    let steering = Arc::new(Mutex::new(Vec::new()));
    let mut run = Active {
        frame: frame.clone(),
        cancel: cancel.clone(),
        job: None,
        ending: Arc::new(AtomicBool::new(false)),
        grant_until: Instant::now() + Duration::from_secs(12),
        acks: None,
        steering,
    };
    if frame.k == "agent-loop" {
        let summary: String = objective(&frame).chars().take(2000).collect();
        sender.sig(json!({"k":"model-grant","id":frame.id,"run":frame.id,"fp":settings.fp,"smart":frame.smart,"binding":previous.binding,"brief":summary,"requires_tools":true})).await?;
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
}
