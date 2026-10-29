use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{FencePresence, TaskOutcome, TaskSpec};

pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn file_hash(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// Resolve existing ancestors too, so a future file has one canonical owner.
/// The Host must keep path mappings stable; this is not hostile-filesystem isolation.
pub(crate) fn canonical(path: &Path) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "Host runtime paths must be absolute");
    ensure!(
        !path.components().any(|part| matches!(part, Component::ParentDir)),
        "Host runtime paths must not contain parent traversal"
    );
    if path.exists() {
        return path.canonicalize().context("canonical Host path unavailable");
    }
    let name = path.file_name().context("Host path has no filename")?;
    let parent = canonical(path.parent().context("Host path has no parent")?)?;
    Ok(parent.join(name))
}

pub(crate) fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path.parent().context("no parent directory")?)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub(crate) fn write_new(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    sync_parent(path)
}

pub(crate) struct Admission {
    // File ownership keeps the cross-process advisory lock for the whole task.
    _lock: File,
    pub fence: PathBuf,
    fence_bytes: Vec<u8>,
    pub evidence: PathBuf,
}

/// Exclusive ownership of an existing fence, before the Host settled it.
pub(crate) struct FencedSession {
    lock: File,
    fence: PathBuf,
    bytes: Vec<u8>,
    pub record: Value,
}

fn lock_session(session: &Path) -> Result<(File, PathBuf)> {
    let parent = session.parent().context("Session parent missing")?;
    ensure!(parent.is_dir(), "Session parent must already exist");
    let identity = session.to_str().context("Session path must be Unicode")?;
    #[cfg(windows)]
    let identity = identity.to_lowercase();
    let digest = hash(identity.as_bytes());
    let lock_path = parent.join(format!(".ara-lite-harness-{digest}.lock"));
    let fence = parent.join(format!(".ara-lite-harness-{digest}.fence.json"));
    let lock = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(lock_path)?;
    lock.try_lock_exclusive().context("another task owns this Session")?;
    Ok((lock, fence))
}

pub(crate) fn new_evidence(spec: &TaskSpec) -> Result<PathBuf> {
    let evidence_parent = spec.evidence_dir.parent().context("evidence parent missing")?.canonicalize()?;
    let evidence = evidence_parent.join(spec.evidence_dir.file_name().context("evidence directory name missing")?);
    fs::create_dir(&evidence).context("evidence directory must be new")?;
    sync_parent(&evidence)?;
    Ok(evidence)
}

fn fence_record(
    spec: &TaskSpec,
    session: &Path,
    evidence: &Path,
    authority: &Value,
    resumed: Option<&Path>,
) -> Result<Vec<u8>> {
    let value = serde_json::json!({"version":1,"task_id":spec.task_id,"session_path":session,
        "evidence_dir":evidence,"criteria_revision":spec.criteria_revision,
        "authorization_revision":spec.authorization_revision,"deadline_unix_ms":spec.deadline_unix_ms,
        "max_runs":spec.max_runs,"max_calls":spec.max_calls,"authority":authority,
        "resumed_from":resumed,"state":"active_or_unknown","pid":std::process::id()});
    let mut bytes = serde_json::to_vec(&value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

impl Admission {
    pub fn acquire(spec: &TaskSpec, session: &Path, authority: &Value) -> Result<Self> {
        let (lock, fence) = lock_session(session)?;
        ensure!(!fence.exists(), "Session has an active or unresolved harness fence; reconcile original effects");
        let evidence = new_evidence(spec)?;
        let fence_bytes = fence_record(spec, session, &evidence, authority, None)?;
        let mut marker = OpenOptions::new().write(true).create_new(true).open(&fence)?;
        marker.write_all(&fence_bytes)?;
        marker.sync_all()?;
        sync_parent(&fence)?;
        write_new(&evidence.join("task.json"), spec)?;
        Ok(Self { _lock: lock, fence, fence_bytes, evidence })
    }

    /// Take the lock of a Session whose earlier owner left its fence. The
    /// fence is unchanged until `FencedSession::take_over` after Host settlement.
    pub fn fenced(session: &Path) -> Result<FencedSession> {
        let (lock, fence) = lock_session(session)?;
        ensure!(fence.is_file(), "Session has no harness fence; nothing to resume");
        let bytes = fs::read(&fence)?;
        let record = serde_json::from_slice(&bytes).context("harness fence is not readable JSON")?;
        Ok(FencedSession { lock, fence, bytes, record })
    }
}

impl FencedSession {
    /// SHA-256 of the fence bytes read under the Session lock.
    pub(crate) fn sha256(&self) -> String {
        hash(&self.bytes)
    }
}

impl FencedSession {
    /// Replace the old fence with one owned by this resumed task. Evidence
    /// is published first, so a crash leaves either the old or the new fence.
    pub fn take_over(
        self,
        spec: &TaskSpec,
        session: &Path,
        evidence: PathBuf,
        authority: &Value,
        resume: &impl Serialize,
    ) -> Result<Admission> {
        ensure!(fs::read(&self.fence)? == self.bytes, "Session fence changed during resume");
        let previous = PathBuf::from(self.record["evidence_dir"].as_str().context("fence evidence missing")?);
        write_new(&evidence.join("task.json"), spec)?;
        write_new(&evidence.join("resume.json"), resume)?;
        let fence_bytes = fence_record(spec, session, &evidence, authority, Some(&previous))?;
        let staged = self.fence.with_extension("resume.tmp");
        // A stale staged file is only an unpublished earlier attempt under this lock.
        let mut marker = OpenOptions::new().write(true).create(true).truncate(true).open(&staged)?;
        marker.write_all(&fence_bytes)?;
        marker.sync_all()?;
        drop(marker);
        fs::rename(&staged, &self.fence)?;
        sync_parent(&self.fence)?;
        Ok(Admission { _lock: self.lock, fence: self.fence, fence_bytes, evidence })
    }
}

impl Admission {
    pub fn finish(&mut self, outcome: &mut TaskOutcome, settled: bool) -> Result<()> {
        outcome.reconciliation_fence = Some(self.fence.clone());
        outcome.fence_presence = FencePresence::Present;
        // Publish outcome before clearing a known-settled task's active marker.
        write_new(&self.evidence.join("outcome.json"), outcome)?;
        if settled {
            ensure!(fs::read(&self.fence)? == self.fence_bytes, "Session fence changed during task");
            let released = (|| -> Result<()> {
                fs::remove_file(&self.fence)?;
                sync_parent(&self.fence)?;
                outcome.reconciliation_fence = None;
                outcome.fence_presence = FencePresence::Released;
                // The first record remains conservative across release errors.
                write_new(&self.evidence.join("released.json"), outcome)
            })();
            if let Err(error) = released {
                // No external work is running, but preserve the failed publication
                // for reconciliation rather than presenting an unrecorded release.
                outcome.reconciliation_fence = Some(self.fence.clone());
                outcome.fence_presence = FencePresence::Unknown;
                if self.fence.exists() {
                    ensure!(fs::read(&self.fence)? == self.fence_bytes, "fence changed during failed release");
                } else {
                    let mut marker = OpenOptions::new().write(true).create_new(true).open(&self.fence)?;
                    marker.write_all(&self.fence_bytes)?;
                    marker.sync_all()?;
                }
                sync_parent(&self.fence)?;
                outcome.fence_presence = FencePresence::Present;
                return Err(error);
            }
        }
        Ok(())
    }
}
