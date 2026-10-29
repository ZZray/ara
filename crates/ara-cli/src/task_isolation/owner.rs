//! Process-local exact generation ownership. Nothing on disk can manufacture this capability.
use super::{
    capture::{self, ApplyReceipt, CaptureReceipt},
    worktree::{self, WorktreeBaseline},
};
use anyhow::{Context, Result, bail};
use ara_iso::BackendKind;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{OwnedMutexGuard, watch};

#[derive(Clone, Debug)]
pub struct IsolationRequest {
    pub repo_root: PathBuf,
    pub resource_root: PathBuf,
    pub artifacts_dir: PathBuf,
    pub task_id: String,
    pub merge_mode: String,
    pub backend: String,
    pub description: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Marker {
    resource_id: String,
    task_id: String,
    pid: u32,
    parent_root: PathBuf,
    private_leaf: PathBuf,
    backend: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseReceipt {
    pub resource_id: String,
    pub status: String,
    pub workspace: PathBuf,
    pub backend: String,
    pub capture: Option<CaptureReceipt>,
    pub error: Option<String>,
    pub durable_receipt: PathBuf,
}
#[derive(Clone, Debug, Serialize)]
struct EffectReceipt {
    id: String,
    phase: String,
    status: String,
    error: Option<String>,
}
struct State {
    peer: Option<crate::host_peers::PeerKey>,
    public_agent_id: Option<String>,
    last_capture: Option<CaptureReceipt>,
    effects: Vec<EffectReceipt>,
    retain_reason: Option<String>,
    applies: HashMap<String, watch::Receiver<Option<std::result::Result<ApplyReceipt, String>>>>,
    release: Option<watch::Receiver<Option<std::result::Result<ReleaseReceipt, String>>>>,
}
pub struct IsolationLease {
    id: String,
    req: IsolationRequest,
    leaf: PathBuf,
    cwd: PathBuf,
    backend: BackendKind,
    fallback_reason: Option<String>,
    baseline: WorktreeBaseline,
    gate: Arc<tokio::sync::Mutex<()>>,
    parent_gate: Arc<tokio::sync::Mutex<()>>,
    sealed: Arc<AtomicBool>,
    state: Mutex<State>,
}
struct AdmissionInner {
    resource_id: String,
    _guard: OwnedMutexGuard<()>,
}
/// Runtime must retain this permit until all deferred writers/hooks are known settled.
pub struct LeaseAdmission {
    inner: Arc<AdmissionInner>,
}
impl LeaseAdmission {
    pub fn resource_id(&self) -> &str {
        &self.inner.resource_id
    }
}
/// Short exclusive publication capability; no filesystem/Git/await while registry holds it.
pub struct PublishPermit<'a> {
    state: std::sync::MutexGuard<'a, State>,
}
impl PublishPermit<'_> {
    pub fn commit(mut self, peer: &crate::host_peers::PeerKey) {
        // prepare_publish validates existing ownership; the attached registry closure supplies exact key.
        self.state.peer = Some(peer.clone());
        self.state
            .public_agent_id
            .get_or_insert_with(|| peer.id.clone());
    }
}

fn state(lease: &IsolationLease) -> std::sync::MutexGuard<'_, State> {
    lease.state.lock().unwrap_or_else(|e| e.into_inner())
}
fn reparse(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}
fn private_root(path: &Path) -> Result<PathBuf> {
    fs::create_dir_all(path)?;
    let root = fs::canonicalize(path)?;
    if reparse(&fs::symlink_metadata(path)?) {
        bail!("resource root cannot be a symlink/reparse point");
    }
    Ok(root)
}
fn validate_leaf(leaf: &Path, root: &Path, id: &str) -> Result<Marker> {
    if leaf.parent() != Some(root)
        || reparse(&fs::symlink_metadata(leaf)?)
        || fs::canonicalize(leaf)? != leaf
    {
        bail!("private isolation leaf authority changed");
    }
    let marker_path = leaf.join("owner.json");
    if reparse(&fs::symlink_metadata(&marker_path)?) {
        bail!("owner marker is a reparse point");
    }
    let marker: Marker = serde_json::from_slice(&fs::read(marker_path)?)?;
    if marker.resource_id != id || marker.private_leaf != leaf {
        bail!("resource owner marker mismatch");
    }
    let merged = leaf.join("m");
    if let Ok(meta) = fs::symlink_metadata(merged) {
        if reparse(&meta) {
            bail!("merged isolation root was replaced by a reparse point");
        }
    }
    Ok(marker)
}

impl IsolationLease {
    pub async fn create(mut req: IsolationRequest) -> Result<Arc<Self>> {
        if !matches!(req.merge_mode.as_str(), "patch" | "branch") {
            bail!("unknown task merge mode");
        }
        // Public branch identity must be a safe single component; physical leaf never embeds it.
        if req.task_id.is_empty()
            || req
                .task_id
                .chars()
                .any(|c| !(c.is_ascii_alphanumeric() || "-_".contains(c)))
        {
            bail!("invalid task id for isolation branch");
        }
        let preferred = if req.backend == "auto" {
            None
        } else {
            Some(BackendKind::from_str(&req.backend).context("unknown isolation backend")?)
        };
        let id = uuid::Uuid::new_v4().simple().to_string();
        let result = tokio::task::spawn_blocking(move || -> Result<Arc<Self>> {
            req.repo_root = worktree::repo_root(&req.repo_root)?;
            let parent_gate = worktree::repo_gate(&req.repo_root)?;
            let _parent_mutation = parent_gate.blocking_lock();
            req.resource_root = private_root(&req.resource_root)?;
            if req.resource_root.starts_with(&req.repo_root) { bail!("isolation resource root must be outside the source checkout"); }
            fs::create_dir_all(&req.artifacts_dir)?; req.artifacts_dir = fs::canonicalize(&req.artifacts_dir)?;
            // Artifacts must survive backend teardown.
            if req.artifacts_dir.starts_with(&req.resource_root) { bail!("artifacts directory must be outside isolation resource root"); }
            let leaf = req.resource_root.join(format!("t{}", &id[..20])); fs::create_dir(&leaf)?;
            let cwd = leaf.join("m");
            let mut marker = Marker { resource_id: id.clone(), task_id: req.task_id.clone(), pid: std::process::id(), parent_root: req.repo_root.clone(), private_leaf: leaf.clone(), backend: None };
            capture::write_new(&leaf.join("owner.json"), &serde_json::to_vec_pretty(&marker)?)?;
            let resolution = ara_iso::resolve(preferred); let mut reason = resolution.reason;
            let mut actual = None;
            for kind in resolution.candidates {
                validate_leaf(&leaf, &req.resource_root, &id)?;
                // A durable selected-backend record precedes the actual start effect.
                marker.backend = Some(kind.to_string());
                capture::write_new(&leaf.join(format!("backend-{}.json",kind.as_str())), &serde_json::to_vec_pretty(&marker)?)?;
                match ara_iso::backend(kind).start(&req.repo_root, &cwd) {
                    Ok(()) => { actual = Some(kind); break; },
                    Err(error) => {
                        let unavailable = error.is_unavailable();
                        let stop = ara_iso::backend(kind).stop(&cwd);
                        if !unavailable || stop.is_err() {
                            let error = format!("isolation startup failed: {error}; stop result {stop:?}; retained {}", leaf.display());
                            let _ = capture::write_new(&leaf.join("startup-retained.json"), error.as_bytes()); bail!("{error}");
                        }
                        reason.get_or_insert_with(|| error.to_string());
                    }
                }
            }
            let backend = actual.context("no isolation backend is available; owner marker retained")?;
            let preparation = (|| -> Result<_> {
                let source = ara_task_vcs::git::GitRepo::require(&req.repo_root)?;
                ara_task_vcs::git::detach_git_dir(&cwd, &source.info().common_dir)?;
                // Nested linked metadata needs the same detachment as the root.
                for path in worktree::discover_nested(&cwd)? {
                    let source_path = req.repo_root.join(&path);
                    let source_nested = ara_task_vcs::git::GitRepo::require(&source_path)?;
                    ara_task_vcs::git::detach_git_dir(&cwd.join(path), &source_nested.info().common_dir)?;
                }
                let baseline = worktree::capture_baseline(&cwd, &req.repo_root, worktree::BASELINE_MAX_CONTENT_BYTES)?;
                capture::write_new(&leaf.join("baseline.json"), &serde_json::to_vec_pretty(&baseline)?)?;
                Ok(baseline)
            })();
            let baseline = match preparation { Ok(value) => value, Err(error) => {
                // Known failed preparation still owns the actual backend. Never unconditional rm.
                let stop = ara_iso::backend(backend).stop(&cwd);
                let message = format!("isolation baseline/preparation failed: {error:#}; stop {stop:?}; retained {}", leaf.display());
                let _ = capture::write_new(&leaf.join("startup-retained.json"), message.as_bytes()); bail!("{message}");
            }};
            Ok(Arc::new(Self { id, req, leaf, cwd, backend, fallback_reason: reason, baseline, gate: Arc::new(tokio::sync::Mutex::new(())), parent_gate: parent_gate.clone(),
                sealed: Arc::new(AtomicBool::new(false)), state: Mutex::new(State { peer: None, public_agent_id: None, last_capture: None, effects: vec![], retain_reason: None, applies: HashMap::new(), release: None }) }))
        }).await.context("isolation creation worker outcome unknown; inspect retained owner markers")??;
        Ok(result)
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
    /// Linearizes final release against initial/warm registry publication; repeated sealing is harmless.
    pub fn seal_admission(&self) -> Result<()> {
        let _publication = state(self);
        self.sealed.store(true, Ordering::Release);
        Ok(())
    }
    /// A failed warm/startup writer has no known join witness. Preserve the exact resource and prohibit replay.
    pub fn retain_unsettled_writer(&self, reason: &str) -> Result<()> {
        let mut locked = state(self);
        self.sealed.store(true, Ordering::Release);
        locked
            .retain_reason
            .get_or_insert_with(|| reason.to_owned());
        Ok(())
    }
    pub fn bind_public_agent_id(&self, agent_id: &str) -> Result<()> {
        if agent_id.is_empty()
            || agent_id
                .chars()
                .any(|c| !(c.is_ascii_alphanumeric() || "-_".contains(c)))
        {
            bail!("invalid public Agent id");
        }
        let _writer = self
            .gate
            .try_lock()
            .context("Agent identity binding cannot race an admitted writer")?;
        let mut locked = state(self);
        if self.sealed.load(Ordering::Acquire)
            || locked.peer.is_some()
            || !locked.effects.is_empty()
        {
            bail!("Agent identity binding must precede publication/admission effects");
        }
        if locked
            .public_agent_id
            .as_deref()
            .is_some_and(|old| old != agent_id)
        {
            bail!("public Agent id already bound");
        }
        locked.public_agent_id = Some(agent_id.to_owned());
        Ok(())
    }
    pub fn prepare_publish(&self) -> Result<PublishPermit<'_>> {
        let locked = state(self);
        if self.sealed.load(Ordering::Acquire) {
            bail!("isolation lease sealed");
        }
        if locked.peer.is_some() {
            bail!("isolation lease already published; use publish for same-key confirmation");
        }
        Ok(PublishPermit { state: locked })
    }
    pub fn prepare_republish(
        &self,
        expected: &crate::host_peers::PeerKey,
    ) -> Result<PublishPermit<'_>> {
        let locked = state(self);
        if self.sealed.load(Ordering::Acquire) || locked.peer.as_ref() != Some(expected) {
            bail!("warm publication requires exact live unsealed PeerKey owner");
        }
        Ok(PublishPermit { state: locked })
    }
    pub fn publish(&self, peer: &crate::host_peers::PeerKey) -> Result<()> {
        let mut state = state(self);
        if self.sealed.load(Ordering::Acquire) {
            bail!("isolation lease sealed");
        }
        if state.peer.as_ref().is_some_and(|old| old != peer) {
            bail!("isolation lease already belongs to another exact PeerKey");
        }
        if state.peer.is_some() {
            return Ok(());
        }
        state.public_agent_id.get_or_insert_with(|| peer.id.clone());
        state.peer = Some(peer.clone());
        Ok(())
    }
    pub async fn admit(self: &Arc<Self>) -> Result<LeaseAdmission> {
        if self.sealed.load(Ordering::Acquire) {
            bail!("isolation lease sealed");
        }
        let guard = self.gate.clone().lock_owned().await;
        if self.sealed.load(Ordering::Acquire) {
            bail!("isolation lease sealed during admission");
        }
        validate_leaf(&self.leaf, &self.req.resource_root, &self.id)?;
        Ok(LeaseAdmission {
            inner: Arc::new(AdmissionInner {
                resource_id: self.id.clone(),
                _guard: guard,
            }),
        })
    }
    fn effect_start(&self, phase: &str) -> Result<String> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let effect = EffectReceipt {
            id: id.clone(),
            phase: phase.into(),
            status: "running".into(),
            error: None,
        };
        let path = self
            .req
            .artifacts_dir
            .join(&self.id)
            .join("effects")
            .join(format!("{id}-started.json"));
        if let Err(error) = capture::write_new(&path, &serde_json::to_vec_pretty(&effect)?) {
            state(self).retain_reason =
                Some(format!("could not persist effect admission: {error:#}"));
            return Err(error);
        }
        state(self).effects.push(effect);
        Ok(id)
    }
    fn effect_end(&self, id: &str, result: &std::result::Result<(), String>) {
        let mut state = state(self);
        if let Some(effect) = state.effects.iter_mut().find(|e| e.id == id) {
            effect.status = if result.is_ok() {
                "settled"
            } else {
                "failed_or_unknown"
            }
            .into();
            effect.error = result.as_ref().err().cloned();
        }
        if let Err(error) = result {
            state.retain_reason.get_or_insert_with(|| error.clone());
        }
        if let Some(effect) = state.effects.iter().find(|e| e.id == id) {
            let path = self
                .req
                .artifacts_dir
                .join(&self.id)
                .join("effects")
                .join(format!("{id}-settled.json"));
            let persisted = serde_json::to_vec_pretty(effect)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| capture::write_new(&path, &bytes));
            if let Err(error) = persisted {
                state
                    .retain_reason
                    .get_or_insert_with(|| format!("effect receipt persistence failed: {error:#}"));
            }
        }
    }
    async fn capture(
        self: &Arc<Self>,
        admission: Arc<AdmissionInner>,
        final_release: bool,
    ) -> Result<CaptureReceipt> {
        if let Some(reason) = state(self).retain_reason.clone() {
            bail!("isolation effect requires reconciliation; no capture replay: {reason}");
        }
        let lease = self.clone();
        let effect = self.effect_start("capture")?;
        // Retained worker owns admission; cancelling this observer cannot let a writer race its Git effects.
        let worker = tokio::spawn(async move {
            let _admission = admission;
            let _parent = lease.parent_gate.clone().lock_owned().await;
            let previous_failure = { state(&lease).retain_reason.clone() };
            if let Some(reason) = previous_failure {
                let error = format!(
                    "earlier isolation effect requires reconciliation; capture not replayed: {reason}"
                );
                lease.effect_end(&effect, &Err(error.clone()));
                return Err(anyhow::Error::msg(error));
            }
            let (previous, public_agent_id) = {
                let locked = state(&lease);
                (
                    locked.last_capture.clone(),
                    locked
                        .public_agent_id
                        .clone()
                        .unwrap_or_else(|| lease.req.task_id.clone()),
                )
            };
            let owner = lease.clone();
            let result = tokio::task::spawn_blocking(move || {
                validate_leaf(&owner.leaf, &owner.req.resource_root, &owner.id)?;
                capture::persist_capture(
                    &owner.cwd,
                    &owner.baseline,
                    &owner.req.artifacts_dir,
                    &public_agent_id,
                    &owner.id,
                    &owner.req.merge_mode,
                    owner.req.description.as_deref(),
                    previous.as_ref(),
                    final_release,
                    &owner.leaf.join("indexes"),
                )
            })
            .await
            .map_err(|e| anyhow::anyhow!("capture worker outcome unknown: {e}"))
            .and_then(|result| result);
            lease.effect_end(
                &effect,
                &result.as_ref().map(|_| ()).map_err(|e| format!("{e:#}")),
            );
            if let Ok(receipt) = &result {
                state(&lease).last_capture = Some(receipt.clone());
            }
            result
        });
        worker
            .await
            .context("capture observer lost worker; resource retained")?
    }
    pub async fn capture_run_end(
        self: &Arc<Self>,
        permit: &LeaseAdmission,
    ) -> Result<CaptureReceipt> {
        if permit.resource_id() != self.id {
            bail!("wrong lease admission");
        }
        self.capture(permit.inner.clone(), false).await
    }
    pub async fn apply_run_end(&self, capture: &CaptureReceipt) -> Result<ApplyReceipt> {
        self.apply_run_end_cancellable(capture, tokio_util::sync::CancellationToken::new(), None)
            .await
    }
    pub async fn apply_run_end_cancellable(
        &self,
        capture: &CaptureReceipt,
        cancel: tokio_util::sync::CancellationToken,
        request_cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<ApplyReceipt> {
        // This API intentionally cannot be invoked by final release. Serialize and retain once per capture.
        if capture.resource_id != self.id {
            bail!("wrong capture resource");
        }
        let mut receiver = {
            let mut locked = state(self);
            if let Some(receiver) = locked.applies.get(&capture.capture_id) {
                receiver.clone()
            } else {
                if self.sealed.load(Ordering::Acquire) {
                    bail!("cannot apply a finalized isolation lease");
                }
                let (sender, receiver) = watch::channel(None);
                locked
                    .applies
                    .insert(capture.capture_id.clone(), receiver.clone());
                let gate = self.gate.clone();
                let parent_gate = self.parent_gate.clone();
                let root = self.req.repo_root.clone();
                let id = self.id.clone();
                let capture = capture.clone();
                let sealed = self.sealed.clone();
                tokio::spawn(async move {
                    let _gate = gate.lock_owned().await;
                    let _parent = parent_gate.lock_owned().await;
                    let result = if sealed.load(Ordering::Acquire) {
                        Err(
                            "isolation lease sealed before parent apply began; apply not attempted"
                                .into(),
                        )
                    } else {
                        tokio::task::spawn_blocking(move || {
                            // Both exact lease and canonical repository gates remain held by this retained
                            // worker. Check at the mutation frontier, including time spent queued for Git.
                            if cancel.is_cancelled() || request_cancel.as_ref().is_some_and(|signal|signal.is_cancelled()) {
                                capture::cancelled_before_apply(&id, &capture)
                            } else {
                                capture::apply(&root, &id, &capture)
                            }
                        })
                        .await
                        .map_err(|e| format!("apply worker outcome unknown; do not replay: {e}"))
                        .and_then(|r| r.map_err(|e| format!("{e:#}")))
                    };
                    sender.send_replace(Some(result));
                });
                receiver
            }
        };
        loop {
            if let Some(result) = receiver.borrow().clone() {
                return result.map_err(anyhow::Error::msg);
            }
            receiver
                .changed()
                .await
                .context("apply receipt channel closed; outcome unknown")?;
        }
    }
    pub async fn finalize(
        self: &Arc<Self>,
        stopped: std::result::Result<(), String>,
    ) -> Result<ReleaseReceipt> {
        let mut receiver = {
            let mut locked = state(self);
            self.sealed.store(true, Ordering::Release);
            if let Some(receiver) = &locked.release {
                receiver.clone()
            } else {
                let (sender, receiver) = watch::channel(None);
                locked.release = Some(receiver.clone());
                let lease = self.clone();
                tokio::spawn(async move {
                    let result = lease.finish(stopped).await.map_err(|e| format!("{e:#}"));
                    sender.send_replace(Some(result));
                });
                receiver
            }
        };
        loop {
            if let Some(result) = receiver.borrow().clone() {
                return result.map_err(anyhow::Error::msg);
            }
            receiver
                .changed()
                .await
                .context("release receipt channel closed; outcome unknown")?;
        }
    }
    async fn finish(
        self: &Arc<Self>,
        stopped: std::result::Result<(), String>,
    ) -> Result<ReleaseReceipt> {
        // Observe all original parent effect flights before taking the writer gate. Never start a new apply.
        let applies: Vec<_> = state(self).applies.values().cloned().collect();
        let mut apply_unknown = None;
        for mut receiver in applies {
            loop {
                let result = receiver.borrow().clone();
                if let Some(result) = result {
                    if let Err(error) = result {
                        apply_unknown.get_or_insert(error);
                    }
                    break;
                }
                if receiver.changed().await.is_err() {
                    apply_unknown
                        .get_or_insert_with(|| "parent apply flight outcome unknown".into());
                    break;
                }
            }
        }
        let guard = self.gate.clone().lock_owned().await;
        let admission = Arc::new(AdmissionInner {
            resource_id: self.id.clone(),
            _guard: guard,
        });
        let mut receipt = ReleaseReceipt {
            resource_id: self.id.clone(),
            status: "retained".into(),
            workspace: self.cwd.clone(),
            backend: self.backend.to_string(),
            capture: state(self).last_capture.clone(),
            error: None,
            durable_receipt: self.req.artifacts_dir.join(&self.id).join("release.json"),
        };
        // stopped comes from the SAME retained close/effects flight as BinaryHost.close, never a fresh callback.
        let blocker = stopped
            .err()
            .or(apply_unknown)
            .or_else(|| state(self).retain_reason.clone());
        if let Some(error) = blocker {
            receipt.error = Some(error);
        } else {
            match self.capture(admission.clone(), true).await {
                Err(error) => receipt.error = Some(format!("final capture failed: {error:#}")),
                Ok(capture) => {
                    receipt.capture = Some(capture);
                    let owner = self.clone();
                    let effect = self.effect_start("backend_stop_and_remove")?;
                    let outcome = tokio::task::spawn_blocking(move || -> Result<()> {
                        validate_leaf(&owner.leaf, &owner.req.resource_root, &owner.id)?;
                        ara_iso::backend(owner.backend)
                            .stop(&owner.cwd)
                            .context("backend stop failed; retained in place")?;
                        validate_leaf(&owner.leaf, &owner.req.resource_root, &owner.id)?;
                        fs::remove_dir_all(&owner.leaf)?;
                        Ok(())
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("backend stop outcome unknown: {e}"))
                    .and_then(|result| result);
                    self.effect_end(
                        &effect,
                        &outcome.as_ref().map(|_| ()).map_err(|e| format!("{e:#}")),
                    );
                    match outcome {
                        Ok(()) => receipt.status = "released".into(),
                        Err(error) => receipt.error = Some(format!("{error:#}")),
                    }
                }
            }
        }
        // Keep mounted/native resource identity in place on every unknown/failure; no rename.
        capture::write_new(
            &receipt.durable_receipt,
            &serde_json::to_vec_pretty(&receipt)?,
        )?;
        Ok(receipt)
    }
    pub fn snapshot(&self) -> serde_json::Value {
        let state = state(self);
        serde_json::json!({"resource_id":self.id,"task_id":self.req.task_id,"public_agent_id":state.public_agent_id,"cwd":self.cwd,"artifact_root":self.req.artifacts_dir.join(&self.id),"backend":self.backend.to_string(),"fallback_reason":self.fallback_reason,
            "peer":state.peer,"sealed":self.sealed.load(Ordering::Acquire),"capture":state.last_capture,"effects":state.effects,"apply_flights":state.applies.len(),"retain_reason":state.retain_reason})
    }
}
