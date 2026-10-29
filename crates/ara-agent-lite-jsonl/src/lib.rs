//! One Task per process, using the accepted Lite Harness through a strict JSONL
//! Host port. Hosts own authorization, operation receipts and final acceptance.
//! Pipes and registered tools are not an OS sandbox. Host-declared model caps
//! are declarations, not provider-directory facts. An unknown callback ends its
//! LOCAL future only; it never proves remote join, rollback or safe replay.

mod evidence;
mod protocol;
mod rpc;
mod runtime;
mod transport;

use std::{
    io::BufReader,
    sync::{Arc, OnceLock},
};

use ara_agent_lite_harness::{
    self as harness,
    core::{rpi_agent::AgentTool, RunRequest},
    BudgetStopReviewPolicy, FencePresence, ResumeChecks, ResumeRefusal, TaskHandle, TaskOptions, TaskOutcome, TaskSpec,
};
use ara_agent_lite_mcp::{McpConfig, McpSession};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use protocol::{Incoming, Start};
use rpc::Bridge;
use runtime::{ExternalHost, Template};
use transport::{read_frame, Output};

/// Stderr contains fixed categories only; never configuration, errors, secrets
/// or provider bodies. A delivered task_result is transport completion, not Task
/// success. Query its original outcome_ref and keep task_accepted false.
pub fn main_entry() -> i32 {
    // Third-party panics must not dump arbitrary provider/Host payloads either.
    std::panic::set_hook(Box::new(|_| eprintln!("sidecar_panic")));
    match stdio() {
        Ok(()) => 0,
        Err(category) => {
            eprintln!("{category}");
            if category == "terminal_delivery_unknown" {
                3
            } else {
                2
            }
        }
    }
}

fn stdio() -> Result<(), &'static str> {
    let mut reader = BufReader::new(std::io::stdin());
    let bytes = read_frame(&mut reader).map_err(|_| "start_frame_rejected")?.ok_or("start_frame_required")?;
    let start: Start = serde_json::from_slice(&bytes).map_err(|_| "start_config_rejected")?;
    let mcp = runtime::validate(&start).map_err(|_| "start_config_rejected")?;
    let token = std::env::var("ARA_AGENT_MODEL_TOKEN").map_err(|_| "model_token_unavailable")?;
    if token.is_empty() || token.len() > 4096 {
        return Err("model_token_rejected");
    }
    // Never pass the model credential to children or accept it on stdin/argv.
    std::env::remove_var("ARA_AGENT_MODEL_TOKEN");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "runtime_unavailable")?
        .block_on(task(start, reader, token, mcp))
}

/// Without `mcp_servers` this is exactly the original Task path. With it, the
/// MCP session is connected before Admission and shut down after the Task.
/// A `resume` start field selects the resume path; without it nothing changes.
async fn task(
    start: Start,
    reader: BufReader<std::io::Stdin>,
    token: String,
    mcp: Option<McpConfig>,
) -> Result<(), &'static str> {
    let session = match mcp {
        // A failed `required` server rejects the frame before any model request.
        Some(config) => Some(runtime::connect_mcp(&start, config).await.map_err(|_| "start_config_rejected")?),
        None => None,
    };
    let tools = session.as_ref().map(McpSession::tools).unwrap_or_default();
    let status = session.as_ref().map(|session| json!({"servers":session.status(),"excluded":session.excluded()}));
    let result = if start.resume.is_some() {
        resumed(start, reader, token, tools, status).await
    } else {
        ordinary(start, reader, token, tools, status).await
    };
    if let Some(session) = session {
        session.shutdown().await;
    }
    result
}

/// What the Task keeps after Admission, shared by both start paths.
struct Prepared {
    emit_events: bool,
    budget_stop_review: BudgetStopReviewPolicy,
    output: Output,
    spec: TaskSpec,
    bridge: Arc<Bridge>,
}

/// Moved into Harness Admission.
type Admit = (RunRequest, Arc<ExternalHost>, TaskOptions);

async fn prepare(
    start: Start,
    token: String,
    mcp_tools: Vec<Arc<dyn AgentTool>>,
    mcp_status: Option<Value>,
) -> Result<(Prepared, Admit), &'static str> {
    let emit_events = start.emit_events;
    let budget_stop_review = start.budget_stop_review;
    let length_stop_review = start.length_stop_review;
    let output = Output::stdout(token.clone());
    let spec: TaskSpec = start.task.into();
    if let Some(mut frame) = mcp_status {
        frame["kind"] = json!("mcp_status");
        frame["version"] = json!(1);
        frame["task_id"] = json!(spec.task_id);
        output.control(frame).await.map_err(|_| "mcp_status_delivery_unknown")?;
    }
    let bridge = Arc::new(Bridge::new(
        output.clone(),
        spec.task_id.clone(),
        spec.evidence_dir.clone(),
        start.run.run_id.clone(),
        start.cancel_grace_ms,
    ));
    let template = Template::new(start.run, token, bridge.clone(), mcp_tools);
    let initial = template.initial();
    let host = Arc::new(ExternalHost { template, bridge: bridge.clone() });
    let options = TaskOptions { budget_stop_review, length_stop_review };
    Ok((Prepared { emit_events, budget_stop_review, output, spec, bridge }, (initial, host, options)))
}

async fn ordinary(
    start: Start,
    reader: BufReader<std::io::Stdin>,
    token: String,
    mcp_tools: Vec<Arc<dyn AgentTool>>,
    mcp_status: Option<Value>,
) -> Result<(), &'static str> {
    let (prepared, (initial, host, options)) = prepare(start, token, mcp_tools, mcp_status).await?;
    let handle = harness::start_with_options(prepared.spec.clone(), initial, host, options)
        .map_err(|_| "task_admission_rejected")?;
    // The reader starts after Admission, as before the resume path existed.
    let admitted = Arc::new(OnceLock::new());
    let _ = admitted.set(handle.clone());
    let input_task = spawn_dispatch(spawn_reader(reader), prepared.bridge.clone(), admitted);
    finish(prepared, handle, input_task).await
}

/// 9.1, 10.2: the reader runs before Admission so a `reconcile_reply` written
/// while Harness waits for the Host settlement is read. Exactly one
/// `resume_checked`; no new Task is admitted on any path.
async fn resumed(
    start: Start,
    reader: BufReader<std::io::Stdin>,
    token: String,
    mcp_tools: Vec<Arc<dyn AgentTool>>,
    mcp_status: Option<Value>,
) -> Result<(), &'static str> {
    let fence_sha256 = start.resume.as_ref().map(|resume| resume.fence_sha256.clone());
    // 9.4: a refused resume writes zero stdout frames, so the MCP status frame
    // waits until the resume is admitted.
    let (prepared, (initial, host, options)) = prepare(start, token, mcp_tools, None).await?;
    let admitted = Arc::new(OnceLock::new());
    let input_task = spawn_dispatch(spawn_reader(reader), prepared.bridge.clone(), admitted.clone());
    let evidence_existed = prepared.spec.evidence_dir.exists();
    let checks = ResumeChecks { expected_fence_sha256: fence_sha256, refuse_larger_run_caps: true };
    let result = harness::resume_checked(prepared.spec.clone(), initial, host, options, checks).await;
    match result {
        Ok(handle) => {
            let _ = admitted.set(handle.clone());
            if let Some(mut frame) = mcp_status {
                frame["kind"] = json!("mcp_status");
                frame["version"] = json!(1);
                frame["task_id"] = json!(prepared.spec.task_id);
                if prepared.output.control(frame).await.is_err() {
                    handle.cancel();
                }
            }
            finish(prepared, handle, input_task).await
        }
        Err(error) => {
            input_task.abort();
            // Harness creates the new evidence directory just before the Host
            // callback; an earlier refusal has nothing of this attempt to record.
            if !evidence_existed && prepared.spec.evidence_dir.is_dir() {
                let category = rejection_category(&error, prepared.bridge.failure_category());
                let _ = evidence::write_new(
                    &prepared.spec.evidence_dir.join("jsonl-resume-rejected.json"),
                    &json!({"category":category,"no_replay":true}),
                );
            }
            Err("task_admission_rejected")
        }
    }
}

/// The typed refusal names the category; a callback-side failure is named by
/// the Bridge. Error text is never parsed.
fn rejection_category(error: &anyhow::Error, bridge: Option<&'static str>) -> Value {
    match error.downcast_ref::<ResumeRefusal>() {
        Some(ResumeRefusal::DeadlineElapsed) => json!(bridge.unwrap_or("reconcile_timeout")),
        None | Some(ResumeRefusal::HostCallbackFailed) => json!(bridge.unwrap_or("harness_refused")),
        Some(refusal) => json!(refusal),
    }
}

enum ReadFault {
    Closed,
    Invalid,
}

/// Blocking stdin has its own bounded reader. Do not join a thread which may
/// wait forever for a client that keeps stdin open after receiving the result.
fn spawn_reader(reader: BufReader<std::io::Stdin>) -> mpsc::Receiver<Result<(Incoming, Value), ReadFault>> {
    let (input, incoming) = mpsc::channel(16);
    std::thread::spawn(move || {
        let mut reader = reader;
        loop {
            let item = match read_frame(&mut reader) {
                Ok(Some(bytes)) => protocol::decode(&bytes).ok_or(ReadFault::Invalid),
                Ok(None) => Err(ReadFault::Closed),
                Err(_) => Err(ReadFault::Invalid),
            };
            let failed = item.is_err();
            if input.blocking_send(item).is_err() || failed {
                break;
            }
        }
    });
    incoming
}

/// After Admission every fault is `Bridge::fail`, exactly as before. While a
/// resume is still in Admission the fault is named for the rejection record.
fn spawn_dispatch(
    mut incoming: mpsc::Receiver<Result<(Incoming, Value), ReadFault>>,
    bridge: Arc<Bridge>,
    admitted: Arc<OnceLock<TaskHandle>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let fail = |category: &'static str| match admitted.get() {
            Some(_) => bridge.fail(),
            None => bridge.fail_with(category),
        };
        while let Some(item) = incoming.recv().await {
            match item {
                Ok((Incoming::Cancel { version, task_id }, _)) => {
                    if version == 1 && task_id == bridge.task_id {
                        match admitted.get() {
                            Some(handle) => handle.cancel(),
                            None => {
                                bridge.fail_with("resume_cancelled");
                                break;
                            }
                        }
                    } else {
                        fail(rpc::RPC_MISMATCH);
                        break;
                    }
                }
                Ok((message, raw)) => {
                    if let Err(category) = bridge.receive(message, &raw) {
                        fail(category);
                        break;
                    }
                }
                Err(fault) => {
                    fail(match fault {
                        ReadFault::Closed => "stdin_closed",
                        ReadFault::Invalid => rpc::INVALID_REPLY_FIELDS,
                    });
                    break;
                }
            }
        }
    })
}

async fn finish(
    prepared: Prepared,
    handle: TaskHandle,
    input_task: tokio::task::JoinHandle<()>,
) -> Result<(), &'static str> {
    let Prepared { emit_events, budget_stop_review, output, spec, bridge } = prepared;
    let fault_bridge = bridge.clone();
    let fault_output = output.clone();
    let fault_handle = handle.clone();
    let fault_task = tokio::spawn(async move {
        tokio::select! {
            _ = fault_bridge.fault.cancelled() => {},
            _ = fault_output.fault.cancelled() => fault_bridge.fail(),
        }
        fault_handle.cancel();
    });
    let mut events = handle.subscribe();
    let ui_output = output.clone();
    let ui_task = tokio::spawn(async move {
        if !emit_events {
            return;
        }
        loop {
            match events.recv().await {
                Ok(event) => ui_output.ui(json!({"kind":"event","version":1,"task_id":event.task_id,
                    "run_id":event.run_id,"seq":event.seq,"event_type":event.event["type"],
                    "delivery":"best_effort_not_complete_evidence"})),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
    let outcome = handle.wait().await;
    bridge.finish_local_callbacks().await;
    // All Core/Host callback futures have now locally finished. A remote Host
    // operation may remain unknown; its original receipt and fence are authority.
    input_task.abort();
    fault_task.abort();
    ui_task.abort();
    if bridge.pending_count() != 0 {
        return Err("terminal_delivery_unknown");
    }
    let outcome_path = spec.evidence_dir.join(if outcome.fence_presence == FencePresence::Released {
        "released.json"
    } else {
        "outcome.json"
    });
    let (reference, bytes) = evidence::read_reference(&outcome_path).map_err(|_| "terminal_delivery_unknown")?;
    let stored: TaskOutcome = serde_json::from_slice(&bytes).map_err(|_| "terminal_delivery_unknown")?;
    let expected = serde_json::to_value(outcome.as_ref()).map_err(|_| "terminal_delivery_unknown")?;
    if serde_json::to_value(&stored).map_err(|_| "terminal_delivery_unknown")? != expected
        || serde_json::from_slice::<Value>(&bytes).map_err(|_| "terminal_delivery_unknown")? != expected
    {
        return Err("terminal_delivery_unknown");
    }
    // A terminal stdout failure cannot retroactively assert a fence: Harness
    // may already have safely released it. Clients query the original files.
    let mut terminal = json!({"kind":"task_result","version":1,"task_id":outcome.task_id,
        "stop":outcome.stop,"task_accepted":false,"runs":outcome.runs,
        "observed_calls":outcome.observed_calls,"fence_presence":outcome.fence_presence,
        "reconciliation_fence":outcome.reconciliation_fence,"outcome_ref":reference,
        "local_callbacks_finished":true});
    if budget_stop_review != BudgetStopReviewPolicy::Reject && outcome.last_candidate.is_none() {
        // A reviewed Length stop also has no candidate; it is not a budget review.
        let stop_subject = |receipt: &harness::ReviewReceipt| {
            harness::stop_snapshot_path(&spec.evidence_dir, receipt.basis.run_number).exists()
        };
        if let Some(receipt) = outcome.last_receipt.as_ref().filter(|receipt| !stop_subject(receipt)) {
            let path = harness::budget_review_index_path(&spec.evidence_dir, receipt.basis.run_number);
            if path.exists() {
                terminal["budget_review_ref"] = serde_json::to_value(
                    evidence::checked_budget_review(&spec.evidence_dir, receipt)
                        .map_err(|_| "terminal_delivery_unknown")?,
                )
                .map_err(|_| "terminal_delivery_unknown")?;
            } else if outcome.fence_presence == FencePresence::Released {
                return Err("terminal_delivery_unknown");
            }
        }
    }
    output.control(terminal).await.map_err(|_| "terminal_delivery_unknown")
}
