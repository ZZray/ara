//! Local operator JSONL reference Host. No listener, product deployment or
//! background service is installed. External transports must authenticate input.
use anyhow::{ensure, Context, Result};
use ara_lite_host::{
    reference::{ModelConfig, Reconciliation, ReferenceAdapter},
    runtime::{Config, Runtime},
    types::*,
    watchdog,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, Write},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    runtime: Config,
    model: ModelConfig,
}
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Event { event: InputEvent },
    Status,
    Review { task_id: String, review_id: String, snapshot: String, decision: String, reason: String },
    Ack { notice_id: String },
    SettleNotice { notice_id: String, evidence: String },
    SettleEvent { key: String, state: DeliveryState, evidence: String },
    RegisterSource { source: EvidenceSource },
    InspectRecovery { task_id: String },
    Recover { task_id: String, not_started: bool, evidence: String, proof: Option<Reconciliation> },
    Stop,
}

fn emit(value: &impl serde::Serialize) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)?;
    writeln!(stdout)?;
    stdout.flush()?;
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let action = args.next().context("usage: ara-lite-host run|start|watch|stop CONFIG.json")?;
    let path = PathBuf::from(args.next().context("CONFIG.json required")?);
    ensure!(args.next().is_none(), "unexpected arguments");
    ensure!(std::fs::metadata(&path)?.len() <= 65536, "Host configuration too large");
    let settings: Settings = serde_json::from_slice(&std::fs::read(&path)?)?;
    if action == "stop" {
        watchdog::request_stop(&settings.runtime.state_dir)?;
        return Ok(());
    }
    if action == "watch" {
        let report = watchdog::supervise(&watchdog::Config {
            program: std::env::current_exe()?,
            args: vec!["run".into(), path.to_string_lossy().into_owned()],
            state_dir: settings.runtime.state_dir,
            poll_ms: 100,
            stale_ms: 15000,
            max_restarts: 3,
        })?;
        return emit(&report);
    }
    ensure!(action == "run" || action == "start", "unknown Host action");
    if action == "start" {
        watchdog::clear_stop(&settings.runtime.state_dir)?;
    }
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?.block_on(serve(settings))
}

async fn serve(settings: Settings) -> Result<()> {
    let mut runtime = Runtime::open(settings.runtime, Arc::new(ReferenceAdapter(settings.model)))?;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Command>>(32);
    std::thread::spawn(move || {
        let mut input = std::io::stdin().lock();
        loop {
            let mut line = String::new();
            // take bounds allocation even for an unbroken oversized line.
            let read = std::io::Read::take(&mut input, 65538).read_line(&mut line);
            let command = match read {
                Ok(0) => break,
                Ok(_) => {
                    if line.len() > 65536 {
                        // Reject the whole physical line, including any suffix
                        // which happens to look like a separate valid command.
                        if !line.ends_with('\n') {
                            loop {
                                let part = match input.fill_buf() {
                                    Ok(part) => part,
                                    Err(error) => {
                                        let _ = tx.blocking_send(Err(error.into()));
                                        return;
                                    }
                                };
                                if part.is_empty() {
                                    break;
                                }
                                let newline = part.iter().position(|byte| *byte == b'\n');
                                let count = newline.map_or(part.len(), |at| at + 1);
                                input.consume(count);
                                if newline.is_some() {
                                    break;
                                }
                            }
                        }
                        Err(anyhow::anyhow!("command exceeds 64KiB"))
                    } else {
                        serde_json::from_str(&line).map_err(Into::into)
                    }
                }
                Err(error) => {
                    let _ = tx.blocking_send(Err(error.into()));
                    return;
                }
            };
            if tx.blocking_send(command).is_err() {
                break;
            }
        }
    });
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let result=async {
      emit(&json!({"event":"host_ready","transport":"local_operator_stdin","recovery":"explicit_original_receipts"}))?;
      loop {
        let operation:Result<bool>=tokio::select!{
            _=tick.tick()=>{
                if runtime.config.state_dir.join("intentional-stop").exists(){Ok(false)}else{
                    match runtime.tick().await {
                        Ok(())=>{
                            watchdog::heartbeat(&runtime.config.state_dir,"ready")?;
                            for notice in runtime.take_notices()? {emit(&json!({"event":"notice","notice":notice}))?;}
                            Ok(true)
                        },
                        Err(error)=>Err(error),
                    }
                }
            },
            command=rx.recv()=>match command {
                None=>Ok(false),
                Some(Err(error))=>{emit(&json!({"event":"command_error","error":format!("{error:#}")}))?;Ok(true)},
                Some(Ok(Command::Stop))=>{watchdog::request_stop(&runtime.config.state_dir)?;Ok(false)},
                Some(Ok(command))=>{
                    match execute(&mut runtime,command).await {
                        Ok(value)=>emit(&json!({"event":"response","value":value}))?,
                        Err(error)=>emit(&json!({"event":"command_error","error":format!("{error:#}")}))?,
                    }
                    Ok(true)
                }
            }
        };
        match operation{Ok(true)=>{},Ok(false)=>break,Err(error)=>return Err(error)}
      }
      Ok(())
    }.await;
    let shutdown = runtime.shutdown().await;
    result.and(shutdown)
}

async fn execute(runtime: &mut Runtime, command: Command) -> Result<serde_json::Value> {
    match command {
        Command::Event { event } => Ok(json!(runtime.ingest(event)?)),
        Command::Status => runtime.status(),
        Command::Review { task_id, review_id, snapshot, decision, reason } => {
            runtime.review(&task_id, &review_id, &snapshot, &decision, &reason)?;
            Ok(json!({"reviewed":task_id}))
        }
        Command::Ack { notice_id } => {
            runtime.ack_notice(&notice_id)?;
            Ok(json!({"acknowledged":notice_id}))
        }
        Command::SettleNotice { notice_id, evidence } => {
            runtime.journal.settle_notice(&notice_id, &evidence)?;
            Ok(json!({"settled":notice_id}))
        }
        Command::SettleEvent { key, state, evidence } => {
            runtime.settle_event(&key, state, &evidence)?;
            Ok(json!({"settled":key}))
        }
        Command::RegisterSource { source } => {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(
                &mut std::io::Read::take(
                    std::fs::File::open(&source.path)?,
                    (ara_lite_host::evidence::MAX_SOURCE_BYTES + 1) as u64,
                ),
                &mut bytes,
            )?;
            ensure!(bytes.len() <= ara_lite_host::evidence::MAX_SOURCE_BYTES, "source too large");
            ensure!(source.sha256 == format!("{:x}", Sha256::digest(&bytes)), "source hash mismatch at registration");
            let binding = runtime.journal.binding(&source.task_id)?.context("source owner must be an admitted Task")?;
            ensure!(binding.generation == source.generation, "source generation mismatch");
            for id in &source.allowed_tasks {
                ensure!(runtime.journal.binding(id)?.is_some(), "unknown allowed Task");
            }
            runtime.journal.put_source(&source)?;
            Ok(json!({"registered":source.id}))
        }
        Command::InspectRecovery { task_id } => {
            let binding = runtime.journal.binding(&task_id)?.context("unknown Task")?;
            let mut fence = None;
            for item in std::fs::read_dir(binding.session_path.parent().context("no Task parent")?)? {
                let item = item?;
                let name = item.file_name();
                if name.to_string_lossy().ends_with(".fence.json") {
                    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(item.path())?)?;
                    if value["task_id"] == task_id {
                        fence = Some(value);
                        break;
                    }
                }
            }
            let hash = fence
                .as_ref()
                .map(|value| format!("{:x}", Sha256::digest(serde_json::to_vec(value).expect("fence serializes"))));
            Ok(json!({"binding":binding,"fence":fence,"fence_sha256":hash,
                "interrupted":ara_agent_lite_harness::core::agent::interrupted_run(&binding.session_path,&binding.workspace)?}))
        }
        Command::Recover { task_id, not_started, evidence, proof } => {
            ensure!(!evidence.trim().is_empty(), "operator reconciliation evidence required");
            if let Some(proof) = proof {
                ensure!(proof.task_id == task_id, "proof Task mismatch");
                let binding = runtime.journal.binding(&task_id)?.context("unknown Task")?;
                let path = binding.session_path.parent().context("no Task parent")?.join("reconciliation.json");
                let mut file = std::fs::File::create(path)?;
                file.write_all(&serde_json::to_vec(&proof)?)?;
                file.sync_all()?;
            }
            runtime.recover(&task_id, not_started).await?;
            Ok(json!({"recovered_original_task":task_id}))
        }
        Command::Stop => unreachable!(),
    }
}
