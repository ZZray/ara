//! Fixed OMP edit/blackbox.ts and auto-repair.ts, 596f2da (MIT).
//! Host resolves its existing Session/settings/auth/model. Core validates and
//! owns after-apply ordering; native argument serialization stays in its realm.
use async_trait::async_trait;
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;
use ara_ai::{graph_values::GraphValueFactory, retry::{RetryBindings, oneshot::OneshotModule}, ActualStreamFailureReceipt};
use ara_rpc::value_heap::{ValueError, ValueHandle};
use futures::future::BoxFuture;

/// Narrow resource joins owned by the issuing Host effect. These methods
/// retain native producer receipts and already captured persistence futures.
/// They never evaluate graph code or create a Promise driver.
pub trait EditRepairResources: Send + Sync {
    fn begin_attempt(&self, cancel: CancellationToken) -> Result<u64, ValueError>;
    fn register_started(&self, key: u64, receipt: ActualStreamFailureReceipt) -> Result<(), ValueError>;
    fn no_request(&self, key: u64) -> Result<(), ValueError>;
    fn dispatch_uncertain(&self, key: u64) -> Result<(), ValueError>;
    fn register_usage(&self, key: u64, observation: u64, write: BoxFuture<'static, Result<(), String>>) -> Result<BoxFuture<'static, Result<(), String>>, ValueError>;
}
#[derive(Clone)]
pub struct EditNativeRepairInvocation {
    pub factory: Arc<GraphValueFactory>,
    pub bindings: Arc<dyn RetryBindings>,
    pub module: Arc<OneshotModule>,
    pub signal: ValueHandle,
    pub resources: Arc<dyn EditRepairResources>,
}
#[async_trait]
pub trait EditRepairCompletionDispatcher: Send + Sync {
    async fn complete(&self, completion: Arc<dyn EditRepairCompletion>, prompt: String, cancel: CancellationToken) -> Result<String, String>;
}

#[derive(Clone, Debug)]
pub struct EditBlackboxRecord {
    pub prev: String,
    pub next: String,
    pub model: String,
    pub variant: String,
}

/// Called once per regression, after the apply and after the preceding repair.
/// Actual hosts must append original params as `arg` and perform JSON.stringify
/// in the original realm now; the early setArgsJson is a separate consumer.
#[async_trait]
pub trait EditBlackboxSerializer: Send + Sync {
    async fn stringify(&self, record: EditBlackboxRecord) -> Result<String, String>;
}

pub struct TypedEditBlackboxSerializer(pub Value);
#[async_trait]
impl EditBlackboxSerializer for TypedEditBlackboxSerializer {
    async fn stringify(&self, r: EditBlackboxRecord) -> Result<String, String> {
        Ok(format!(
            "{{\"prev\":{},\"new\":{},\"model\":{},\"variant\":{},\"arg\":{}}}",
            Value::String(r.prev),
            Value::String(r.next),
            Value::String(r.model),
            Value::String(r.variant),
            self.0
        ))
    }
}

#[async_trait]
pub trait EditRepairCompletion: Send + Sync {
    fn model_name(&self) -> String;
    async fn complete(&self, prompt: String, cancel: CancellationToken) -> Result<String, String>;
    /// Native calls never fall back to a typed response when this port is absent.
    fn complete_actual(&self, _invocation: EditNativeRepairInvocation, _prompt: String, _cancel: CancellationToken) -> BoxFuture<'static, Result<String, ValueError>> {
        Box::pin(async { Err(ValueError::UnsupportedOperation("Actual Edit side completion is unavailable")) })
    }
}

#[async_trait]
pub trait EditPostApplyHost: Send + Sync {
    fn blackbox_enabled(&self) -> bool {
        false
    }
    fn agent_dir(&self) -> PathBuf;
    fn active_model_string(&self) -> Option<String> {
        None
    }
    /// Gate -> registry -> smol role -> eager Session-sticky getApiKey.
    /// None returns before reading disk or localizing the broken region.
    async fn prepare_repair(
        &self,
        cancel: Option<CancellationToken>,
    ) -> Result<Option<Arc<dyn EditRepairCompletion>>, String>;
    fn diagnostic(&self, _message: &str) {}
}

pub(crate) struct BlackboxRecorder {
    path: PathBuf,
    model: String,
    variant: String,
    serializer: Option<Arc<dyn EditBlackboxSerializer>>,
}
impl BlackboxRecorder {
    pub(crate) fn capture(
        host: &dyn EditPostApplyHost,
        variant: &str,
        serializer: Option<Arc<dyn EditBlackboxSerializer>>,
    ) -> Option<Self> {
        host.blackbox_enabled().then(|| Self {
            path: host.agent_dir().join("edit-blackbox.jsonl"),
            model: host.active_model_string().unwrap_or_else(|| "unknown".into()),
            variant: variant.into(),
            serializer,
        })
    }
    pub(crate) async fn record(&self, snapshot: &ara_edit::auto_repair::AppliedEditSnapshot) -> Result<(), String> {
        let serializer = self.serializer.as_ref().ok_or("Edit blackbox original-params serializer unavailable")?;
        let mut line = serializer
            .stringify(EditBlackboxRecord {
                prev: snapshot.prev.clone(),
                next: snapshot.next.clone(),
                model: self.model.clone(),
                variant: self.variant.clone(),
            })
            .await?;
        line.push('\n');
        // Fixed appendFile does not create agentDir and preserves existing JSONL.
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(line.as_bytes())
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
    }
}

pub(crate) struct RepairOutcome {
    pub diff: String,
    pub model: String,
    pub attempts: usize,
}

pub(crate) async fn attempt_repair(
    host: &dyn EditPostApplyHost,
    snapshot: &ara_edit::auto_repair::AppliedEditSnapshot,
    caller: Option<CancellationToken>,
    writer: &super::edit::FsWriter,
    dispatcher: Option<Arc<dyn EditRepairCompletionDispatcher>>,
) -> Result<Option<RepairOutcome>, String> {
    let Some(completion) = host.prepare_repair(caller.clone()).await? else { return Ok(None) };
    let path = PathBuf::from(&snapshot.path);
    let current = match tokio::fs::read_to_string(&path).await {
        Ok(current) => current,
        Err(_) => return Ok(None),
    };
    if ara_edit::auto_repair::parses_source(&current, &snapshot.path) {
        return Ok(None);
    }
    // Target budget begins after eager key resolution and disk safety checks,
    // once for all localization/completion attempts, never once per retry.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(ara_edit::auto_repair::REPAIR_TIMEOUT_MS);
    let work = caller.as_ref().map(CancellationToken::child_token).unwrap_or_default();
    let stop = work.clone().drop_guard();
    let live = ara_edit::auto_repair::AppliedEditSnapshot { next: current.clone(), ..snapshot.clone() };
    let repairing = ara_edit::auto_repair::repair_parse_regression(&live, |prompt| {
        let dispatcher = dispatcher.clone();
        let completion = completion.clone();
        let work = work.clone();
        async move {
            if work.is_cancelled() { return Err("auto-repair aborted".into()); }
            match dispatcher {
                // NativeRun retains the original Graph future and model/usage
                // receipts when this receiver times out. No Graph task moves.
                Some(dispatcher) => dispatcher.complete(completion, prompt, work).await,
                // Typed completion has no original-realm polling dependency.
                // Keep the admitted provider/usage future owned to settlement
                // even when the repair caller reaches its deadline.
                None => tokio::spawn(async move { completion.complete(prompt, work).await })
                    .await.map_err(|error| format!("auto-repair completion task failed: {error}"))?,
            }
        }
    });
    tokio::pin!(repairing);
    let caller_cancelled = async {
        match &caller {
            Some(caller) => caller.cancelled().await,
            None => std::future::pending::<()>().await,
        }
    };
    let repair = tokio::select! {
        biased;
        _ = caller_cancelled => {
            work.cancel();
            Err("auto-repair aborted".into())
        },
        _ = tokio::time::sleep_until(deadline) => {
            work.cancel();
            Err(format!("auto-repair timed out after {}ms waiting on {}", ara_edit::auto_repair::REPAIR_TIMEOUT_MS, completion.model_name()))
        },
        result = &mut repairing => result,
    };
    drop(stop);
    let Some(repair) = repair? else { return Ok(None) };
    writer.write_repair(&path, &repair.content, caller).await.map_err(|e| e.to_string())?;
    host.diagnostic(&format!(
        "Edit auto-repair applied: {} (attempts={}, regionLines={})",
        snapshot.path,
        repair.attempts,
        repair.region.b_end - repair.region.b_start
    ));
    let diff = ara_edit::diff_string::generate_diff_string(
        &current,
        &repair.content,
        None,
        &ara_edit::diff_string::BlockContextSource { path: Some(&snapshot.path), lang: None, streaming: false },
    );
    Ok(Some(RepairOutcome { diff: diff.diff, model: completion.model_name(), attempts: repair.attempts }))
}
