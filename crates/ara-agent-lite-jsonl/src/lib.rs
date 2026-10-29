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

use std::{io::BufReader, sync::Arc};

use ara_agent_lite_harness::{
    self as harness, BudgetStopReviewPolicy, FencePresence, TaskOptions, TaskOutcome, TaskSpec,
};
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
    runtime::validate(&start).map_err(|_| "start_config_rejected")?;
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
        .block_on(task(start, reader, token))
}

async fn task(start: Start, reader: BufReader<std::io::Stdin>, token: String) -> Result<(), &'static str> {
    let emit_events = start.emit_events;
    let budget_stop_review = start.budget_stop_review;
    let output = Output::stdout(token.clone());
    let spec: TaskSpec = start.task.into();
    let bridge = Arc::new(Bridge::new(
        output.clone(),
        spec.task_id.clone(),
        spec.evidence_dir.clone(),
        start.run.run_id.clone(),
        start.cancel_grace_ms,
    ));
    let template = Template::new(start.run, token, bridge.clone());
    let initial = template.initial();
    let host = Arc::new(ExternalHost { template, bridge: bridge.clone() });
    let handle = harness::start_with_options(
        spec.clone(),
        initial,
        host,
        TaskOptions { budget_stop_review, ..TaskOptions::default() },
    )
    .map_err(|_| "task_admission_rejected")?;

    // Blocking stdin has its own bounded reader. Do not join a thread which may
    // wait forever for a client that keeps stdin open after receiving the result.
    let (input, mut incoming) = mpsc::channel::<Result<(Incoming, Value), ()>>(16);
    std::thread::spawn(move || {
        let mut reader = reader;
        loop {
            let item = read_frame(&mut reader)
                .ok()
                .flatten()
                .and_then(|bytes| {
                    // Deserialize the typed frame directly as well: deriving
                    // from Value alone would erase duplicate JSON object keys.
                    let message: Incoming = serde_json::from_slice(&bytes).ok()?;
                    let value: Value = serde_json::from_slice(&bytes).ok()?;
                    Some((message, value))
                })
                .ok_or(());
            let failed = item.is_err();
            if input.blocking_send(item).is_err() || failed {
                break;
            }
        }
    });
    let input_bridge = bridge.clone();
    let input_handle = handle.clone();
    let input_task = tokio::spawn(async move {
        while let Some(item) = incoming.recv().await {
            match item {
                Ok((Incoming::Cancel { version, task_id }, _)) => {
                    if version == 1 && task_id == input_bridge.task_id {
                        input_handle.cancel();
                    } else {
                        input_bridge.fail();
                        break;
                    }
                }
                Ok((message, raw)) => {
                    if input_bridge.receive(message, &raw).is_err() {
                        input_bridge.fail();
                        break;
                    }
                }
                Err(()) => {
                    input_bridge.fail();
                    break;
                }
            }
        }
    });
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
        if let Some(receipt) = &outcome.last_receipt {
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
