use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use anyhow::{bail, ensure, Context, Result};
use ara_agent_lite_harness::ReviewReceipt;
use serde_json::{json, Value};
use tokio::sync::{oneshot, Notify};
use tokio_util::sync::CancellationToken;

use crate::{
    evidence,
    protocol::{Incoming, ToolReply, TOOL_RESULT_BYTES},
    transport::Output,
};

#[derive(Clone)]
pub struct ToolKey {
    pub native_call_id: String,
    pub tool_name: String,
    pub args_sha256: String,
}

struct Expected {
    kind: &'static str,
    run_id: String,
    tool: Option<ToolKey>,
}

pub enum Reply {
    Tool(ToolReply),
    Verify(Box<ReviewReceipt>),
    Prepare { run_id: String, objective: String },
}

struct Pending {
    expected: Expected,
    sender: oneshot::Sender<Result<Reply>>,
}

pub struct Bridge {
    pub output: Output,
    pub task_id: String,
    pub evidence_dir: PathBuf,
    pub fault: CancellationToken,
    pub active_run: Mutex<String>,
    stop: CancellationToken,
    local_calls: AtomicUsize,
    local_done: Notify,
    pending: Mutex<HashMap<String, Pending>>,
    serial: AtomicU64,
    grace: Duration,
}

impl Bridge {
    pub fn new(output: Output, task_id: String, evidence_dir: PathBuf, run_id: String, grace: u64) -> Self {
        Self {
            output,
            task_id,
            evidence_dir,
            fault: CancellationToken::new(),
            active_run: Mutex::new(run_id),
            stop: CancellationToken::new(),
            local_calls: AtomicUsize::new(0),
            local_done: Notify::new(),
            pending: Mutex::new(HashMap::new()),
            serial: AtomicU64::new(0),
            grace: Duration::from_millis(grace),
        }
    }

    pub fn fail(&self) {
        self.fault.cancel();
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        for (_, item) in pending {
            let _ = item.sender.send(Err(anyhow::anyhow!("external_callback_unknown")));
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }

    pub async fn finish_local_callbacks(&self) {
        self.stop.cancel();
        loop {
            let notified = self.local_done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.local_calls.load(Ordering::SeqCst) == 0 {
                break;
            }
            notified.await;
        }
    }

    pub async fn call(
        self: &Arc<Self>,
        kind: &'static str,
        run_id: String,
        request: Value,
        tool: Option<ToolKey>,
        cancellation: CancellationToken,
    ) -> Result<Reply> {
        // Core may drop its tool wait during cancellation. A separately owned
        // local callback survives that waiter and is joined before task_result.
        self.local_calls.fetch_add(1, Ordering::SeqCst);
        let bridge = self.clone();
        let (sender, receiver) = oneshot::channel();
        tokio::spawn(async move {
            let mut guard = LocalCall { bridge: bridge.clone(), completed: false };
            let result = bridge.run_call(kind, run_id, request, tool, cancellation).await;
            guard.completed = true;
            let _ = sender.send(result);
        });
        receiver.await.context("external_callback_unknown")?
    }

    async fn run_call(
        &self,
        kind: &'static str,
        run_id: String,
        mut request: Value,
        tool: Option<ToolKey>,
        cancellation: CancellationToken,
    ) -> Result<Reply> {
        ensure!(!self.fault.is_cancelled() && !cancellation.is_cancelled(), "callback_not_admitted");
        let rpc = format!("rpc-{}", self.serial.fetch_add(1, Ordering::SeqCst) + 1);
        let fields = request.as_object_mut().context("callback_request_invalid")?;
        fields.insert("kind".into(), json!(kind));
        fields.insert("version".into(), json!(1));
        fields.insert("rpc_id".into(), json!(rpc));
        fields.insert("task_id".into(), json!(self.task_id));
        fields.insert("run_id".into(), json!(run_id));
        let expected = Expected { kind, run_id: run_id.clone(), tool };
        let (sender, mut receiver) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            ensure!(pending.len() < 32, "too_many_pending_callbacks");
            pending.insert(rpc.clone(), Pending { expected, sender });
        }
        let intent = (|| -> Result<()> {
            let wire: Value = serde_json::from_slice(&self.output.encode(&request)?)?;
            evidence::write_new(&evidence::rpc_path(&self.evidence_dir, &rpc, "request"), &wire)?;
            Ok(())
        })();
        if intent.is_err() || self.output.control(request).await.is_err() {
            self.fail();
        }
        let result = tokio::select! {
            biased;
            _ = self.fault.cancelled() => Err(anyhow::anyhow!("external_callback_unknown")),
            result = &mut receiver => result.context("external_callback_unknown").and_then(|v| v),
            _ = async { tokio::select! { _ = cancellation.cancelled() => {}, _ = self.stop.cancelled() => {} } } => {
                let cancel = json!({"kind":"callback_cancel","version":1,"rpc_id":rpc,
                    "task_id":self.task_id,"run_id":run_id,"reason":"cancelled"});
                if self.output.control(cancel).await.is_err() {
                    self.fail();
                    Err(anyhow::anyhow!("external_callback_unknown"))
                } else {
                    match tokio::time::timeout(self.grace, &mut receiver).await {
                        Ok(Ok(reply)) => reply,
                        _ => { self.fail(); Err(anyhow::anyhow!("external_callback_unknown")) }
                    }
                }
            },
        };
        self.pending.lock().unwrap().remove(&rpc);
        if result.is_err() {
            // Local future completion is NOT remote join or rollback. The
            // caller's original operation receipt remains started/unknown.
            let _ = evidence::write_new(
                &evidence::rpc_path(&self.evidence_dir, &rpc, "unknown"),
                &json!({"rpc_id":rpc,"task_id":self.task_id,"run_id":run_id,
                    "local_callback":"finished","external_settlement":"unknown","no_replay":true}),
            );
        }
        result
    }

    pub fn receive(&self, incoming: Incoming, raw: &Value) -> Result<()> {
        let (version, rpc, task, run, reply_kind, tool, reply) = match incoming {
            Incoming::ToolReply {
                version,
                rpc_id,
                task_id,
                run_id,
                native_call_id,
                tool_name,
                args_sha256,
                result,
            } => {
                ensure!(serde_json::to_vec(&result_value(&result))?.len() <= TOOL_RESULT_BYTES, "tool_result_limit");
                ensure!(!result.operation_receipt.trim().is_empty(), "operation_receipt_required");
                (
                    version,
                    rpc_id,
                    task_id,
                    run_id,
                    "tool_call",
                    Some(ToolKey { native_call_id, tool_name, args_sha256 }),
                    Reply::Tool(result),
                )
            }
            Incoming::VerifyReply { version, rpc_id, task_id, run_id, receipt } => {
                strict_receipt(&raw["receipt"])?;
                (version, rpc_id, task_id, run_id, "verify_request", None, Reply::Verify(Box::new(receipt)))
            }
            Incoming::VerifyBudgetReply { version, rpc_id, task_id, run_id, receipt } => {
                strict_receipt(&raw["receipt"])?;
                (version, rpc_id, task_id, run_id, "verify_budget_request", None, Reply::Verify(Box::new(receipt)))
            }
            Incoming::PrepareReply { version, rpc_id, task_id, run_id, run_id_next, objective } => {
                ensure!(!run_id_next.trim().is_empty() && !objective.trim().is_empty(), "prepare_fields_required");
                (
                    version,
                    rpc_id,
                    task_id,
                    run_id,
                    "prepare_request",
                    None,
                    Reply::Prepare { run_id: run_id_next, objective },
                )
            }
            Incoming::PrepareBudgetReply { version, rpc_id, task_id, run_id, run_id_next, objective } => {
                ensure!(!run_id_next.trim().is_empty() && !objective.trim().is_empty(), "prepare_fields_required");
                (
                    version,
                    rpc_id,
                    task_id,
                    run_id,
                    "prepare_budget_request",
                    None,
                    Reply::Prepare { run_id: run_id_next, objective },
                )
            }
            Incoming::Cancel { .. } => bail!("cancel_is_not_a_reply"),
        };
        ensure!(version == 1 && task == self.task_id, "reply_task_or_version_mismatch");
        let mut pending = self.pending.lock().unwrap();
        let item = pending.get(&rpc).context("unsolicited_or_duplicate_reply")?;
        ensure!(item.expected.kind == reply_kind && item.expected.run_id == run, "reply_kind_or_run_mismatch");
        match (&item.expected.tool, tool) {
            (None, None) => {}
            (Some(expected), Some(actual)) => ensure!(
                expected.native_call_id == actual.native_call_id
                    && expected.tool_name == actual.tool_name
                    && expected.args_sha256 == actual.args_sha256,
                "tool_correlation_mismatch"
            ),
            _ => bail!("reply_tool_kind_mismatch"),
        }
        // Persist the actual correlated reply before the local callback consumes
        // a Host settlement declaration. This is still a trusted Host claim.
        evidence::write_new(&evidence::rpc_path(&self.evidence_dir, &rpc, "reply"), raw)?;
        let item = pending.remove(&rpc).context("pending_reply_lost")?;
        let _ = item.sender.send(Ok(reply));
        Ok(())
    }
}

struct LocalCall {
    bridge: Arc<Bridge>,
    completed: bool,
}

impl Drop for LocalCall {
    fn drop(&mut self) {
        if !self.completed {
            self.bridge.fail();
        }
        self.bridge.local_calls.fetch_sub(1, Ordering::SeqCst);
        self.bridge.local_done.notify_waiters();
    }
}

pub fn result_value(result: &ToolReply) -> Value {
    json!({"value":result.value,"is_error":result.is_error,"settlement":result.settlement,
        "operation_receipt":result.operation_receipt})
}

fn keys(value: &Value, allowed: &[&str]) -> Result<()> {
    ensure!(
        value.as_object().context("object_required")?.keys().all(|key| allowed.contains(&key.as_str())),
        "unknown_nested_field"
    );
    Ok(())
}

fn strict_receipt(value: &Value) -> Result<()> {
    keys(value, &["basis", "committed", "settlement", "verdict", "evidence"])?;
    keys(
        &value["basis"],
        &[
            "task_id",
            "criteria_revision",
            "authorization_revision",
            "run_id",
            "run_number",
            "session_path",
            "events_path",
            "events_sha256",
            "event_count",
            "run_calls",
            "total_calls",
            "journal_head_sha256",
            "raw_start",
            "raw_end",
            "raw_proof",
        ],
    )?;
    if value["verdict"].is_object() {
        keys(&value["verdict"], &["NeedsChanges"])?;
        keys(&value["verdict"]["NeedsChanges"], &["feedback"])?;
    }
    Ok(())
}
