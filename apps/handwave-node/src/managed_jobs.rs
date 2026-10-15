//! Host-authorized, process-owned jobs. Durable receipts are not permission grants.
//! A worker is a detached invocation of this same executable, not a Tokio task in
//! the transport process. A missing worker never causes command replay.
use anyhow::{anyhow, bail, ensure, Context, Result};
use async_trait::async_trait;
use fs2::FileExt;
use rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolExecutionMode, ToolResultPartial};
use rpi_ai::{Schema, Tool};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
};
use tokio_util::sync::CancellationToken;

const VERSION: u32 = 1;
const SCRIPT_LIMIT: usize = 64 * 1024;
const READ_LIMIT: usize = 32 * 1024;
const META_LIMIT: u64 = 256 * 1024;
const START_GRACE: u64 = 10;
const WORKER_FLAG: &str = "--managed-job-worker";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub state_path_hash: String,
    pub device: String,
    pub session: String,
}
impl Owner {
    pub fn new(state: &Path, device: &str, session: &str) -> Result<Self> {
        ensure!(!device.is_empty() && device.len() <= 256, "invalid job device owner");
        ensure!(!session.is_empty() && session.len() <= 256, "invalid job session owner");
        Ok(Self {
            state_path_hash: digest(state.canonicalize()?.to_string_lossy().as_bytes()),
            device: device.into(),
            session: session.into(),
        })
    }
}

/// Constructed only by the Host from an explicit authorized Run/Goal contract.
/// It is never deserialized from a model tool argument or extended by a worker.
#[derive(Clone, Debug)]
pub struct Policy {
    pub owner: Owner,
    pub lease_expires_at: u64,
    pub max_jobs: usize,
    pub max_log_bytes: u64,
    pub max_seconds: u64,
}
impl Policy {
    fn validate(&self) -> Result<()> {
        ensure!((1..=64).contains(&self.max_jobs), "job quota must be 1..64");
        ensure!((1024..=16 * 1024 * 1024).contains(&self.max_log_bytes), "log quota must be 1 KiB..16 MiB");
        ensure!((1..=86400).contains(&self.max_seconds), "job duration must be 1..86400 seconds");
        ensure!(
            self.owner.state_path_hash.len() == 64 && !self.owner.device.is_empty() && !self.owner.session.is_empty(),
            "invalid job owner"
        );
        Ok(())
    }
}

#[derive(Clone)]
pub struct Manager {
    directory: PathBuf,
    work_root: PathBuf,
    policy: Policy,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Spec {
    op_id: String,
    language: String,
    script: String,
    cwd: PathBuf,
    timeout_seconds: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    job_id: String,
    owner: Owner,
    input_hash: String,
    worker_nonce: String,
    created_at: u64,
    lease_expires_at: u64,
    max_log_bytes: u64,
    spec: Spec,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub job_id: String,
    pub state: String,
    pub worker_nonce: String,
    pub worker_pid: Option<u32>,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    pub exit_code: Option<i32>,
    pub reason: String,
    pub effects_unknown: bool,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_sha256: Option<String>,
    pub stderr_sha256: Option<String>,
    pub lease_expires_at: u64,
}
impl Receipt {
    fn initial(request: &Request) -> Self {
        Self {
            job_id: request.job_id.clone(),
            state: "starting".into(),
            worker_nonce: request.worker_nonce.clone(),
            worker_pid: None,
            started_at: None,
            finished_at: None,
            exit_code: None,
            reason: "durable intent; worker startup not yet acknowledged".into(),
            effects_unknown: true,
            stdout_bytes: 0,
            stderr_bytes: 0,
            stdout_sha256: None,
            stderr_sha256: None,
            lease_expires_at: request.lease_expires_at,
        }
    }
    fn terminal(&self) -> bool {
        matches!(
            self.state.as_str(),
            "succeeded" | "failed" | "cancelled" | "timed_out" | "lease_expired" | "log_limit" | "interrupted"
        )
    }
}

impl Manager {
    pub fn new(state: &Path, work_root: &Path, policy: Policy) -> Result<Self> {
        policy.validate()?;
        let owner = Owner::new(state, &policy.owner.device, &policy.owner.session)?;
        ensure!(owner == policy.owner, "job state directory does not match owner");
        let canonical = state.canonicalize()?;
        let root = if canonical.is_dir() {
            canonical.as_path()
        } else {
            canonical.parent().context("job Session has no parent")?
        };
        let directory = root.join("managed-jobs").join(digest(&serde_json::to_vec(&owner)?));
        std::fs::create_dir_all(&directory)?;
        private_dir(&directory)?;
        let work_root = work_root.canonicalize()?;
        ensure!(work_root.is_dir(), "job working directory is not a directory");
        Ok(Self { directory, work_root, policy })
    }

    pub async fn start(&self, args: &Value) -> Result<Receipt> {
        let spec = self.parse_spec(args)?;
        let job_id = digest(format!("{}:{}", digest(&serde_json::to_vec(&self.policy.owner)?), spec.op_id).as_bytes());
        let input_hash = digest(&serde_json::to_vec(&spec)?);
        let dir = self.directory.join(&job_id);
        // Serialize quota admission and creation across all Managers for this owner.
        let admission = lock_file(&self.directory.join("admission.lock"))?;
        admission.lock_exclusive()?;
        if dir.exists() {
            let request = self.request(&dir)?;
            ensure!(
                request.input_hash == input_hash,
                "op_id was already used with different job input; no command replayed"
            );
            drop(admission);
            return self.status(&job_id);
        }
        ensure!(
            now() < self.policy.lease_expires_at,
            "background job contract has expired; Host authorization required"
        );
        let jobs: Vec<_> = std::fs::read_dir(&self.directory)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .collect();
        let count = jobs.len();
        ensure!(count < self.policy.max_jobs, "retained job quota exhausted; Host must archive jobs explicitly");
        for job in jobs {
            let id = job.file_name().to_string_lossy().into_owned();
            let receipt = self.status(&id)?;
            ensure!(
                receipt.terminal() && receipt.state != "interrupted",
                "only one active job is permitted; an interrupted job requires Host reconciliation before new dispatch"
            );
        }
        std::fs::create_dir(&dir)?;
        private_dir(&dir)?;
        let request = Request {
            version: VERSION,
            job_id: job_id.clone(),
            owner: self.policy.owner.clone(),
            input_hash,
            worker_nonce: nonce()?,
            created_at: now(),
            lease_expires_at: self.policy.lease_expires_at,
            max_log_bytes: self.policy.max_log_bytes,
            spec,
        };
        // Request is the durable at-most-once spawn intent. Its existence forbids
        // another worker spawn, including after an ambiguous startup failure.
        write_json(&dir.join("request.json"), &request)?;
        write_json(&dir.join("receipt.json"), &Receipt::initial(&request))?;
        sync_dir(&self.directory)?;
        let spawned = spawn_worker(&dir);
        if let Err(error) = spawned {
            let mut receipt = Receipt::initial(&request);
            receipt.state = "failed".into();
            receipt.effects_unknown = false;
            receipt.finished_at = Some(now());
            receipt.reason = format!("worker could not be spawned: {error:#}");
            write_json(&dir.join("receipt.json"), &receipt)?;
            return Ok(receipt);
        }
        drop(admission);
        for _ in 0..30 {
            let receipt = self.status(&job_id)?;
            if receipt.state != "starting" {
                return Ok(receipt);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.status(&job_id)
    }

    pub fn status(&self, job_id: &str) -> Result<Receipt> {
        let dir = self.job_dir(job_id)?;
        let request = self.request(&dir)?;
        let lock = lock_file(&dir.join("worker.lock"))?;
        let locked = match lock.try_lock_exclusive() {
            Ok(()) => false,
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => true,
            Err(error) => return Err(error.into()),
        };
        let mut receipt: Receipt = read_json(&dir.join("receipt.json"))?;
        ensure!(
            receipt.job_id == request.job_id && receipt.worker_nonce == request.worker_nonce,
            "job receipt identity mismatch"
        );
        if !locked && !receipt.terminal() && now().saturating_sub(request.created_at) > START_GRACE {
            receipt.state = "interrupted".into();
            receipt.effects_unknown = true;
            receipt.reason =
                "no worker owns the durable job lock; effects may remain; command will not be replayed".into();
        }
        if dir.join("cancel.json").exists() && !receipt.terminal() {
            receipt.reason = "cancellation requested; worker cleanup not yet acknowledged".into();
        }
        receipt.stdout_bytes = file_len(&dir.join("stdout.bin"));
        receipt.stderr_bytes = file_len(&dir.join("stderr.bin"));
        Ok(receipt)
    }

    pub fn cancel(&self, job_id: &str) -> Result<Receipt> {
        let dir = self.job_dir(job_id)?;
        self.request(&dir)?;
        let receipt = self.status(job_id)?;
        if !receipt.terminal() || receipt.state == "interrupted" {
            write_json(&dir.join("cancel.json"), &json!({"job_id":job_id,"requested_at":now()}))?;
        }
        self.status(job_id)
    }

    /// Bounded owner-local inventory. Interrupted jobs intentionally remain here:
    /// losing a worker is not evidence that its external effects are resolved.
    pub fn unfinished(&self) -> Result<Vec<Receipt>> {
        let mut jobs = Vec::new();
        let mut count = 0usize;
        for entry in std::fs::read_dir(&self.directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            count += 1;
            ensure!(count <= 64, "job owner inventory exceeds retained quota");
            let id = entry.file_name().to_string_lossy().into_owned();
            let receipt = self.status(&id)?;
            if !receipt.terminal() || receipt.state == "interrupted" {
                jobs.push(receipt);
            }
        }
        jobs.sort_by(|a, b| a.job_id.cmp(&b.job_id));
        Ok(jobs)
    }

    /// Request cleanup only. A subsequent terminal worker receipt is the ACK.
    pub fn cancel_all(&self) -> Result<()> {
        for receipt in self.unfinished()? {
            self.cancel(&receipt.job_id)?;
        }
        Ok(())
    }

    pub fn ready_to_continue(&self) -> Result<bool> {
        Ok(self.unfinished()?.is_empty())
    }
    /// Fresh Host observation of receipt identities; no scripts or log bodies.
    pub fn inventory(&self) -> Result<Vec<Receipt>> {
        let mut items = Vec::new();
        for entry in std::fs::read_dir(&self.directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            ensure!(items.len() < 64, "job inventory exceeds Host quota");
            items.push(self.status(&entry.file_name().to_string_lossy())?);
        }
        items.sort_by(|a, b| a.job_id.cmp(&b.job_id));
        Ok(items)
    }

    pub fn read_log(&self, job_id: &str, stream: &str, cursor: u64, limit: usize) -> Result<Value> {
        ensure!(matches!(stream, "stdout" | "stderr"), "stream must be stdout or stderr");
        ensure!((4..=READ_LIMIT).contains(&limit), "log limit must be 4..32768 bytes");
        let dir = self.job_dir(job_id)?;
        self.request(&dir)?;
        let path = dir.join(format!("{stream}.bin"));
        let status = self.status(job_id)?;
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ensure!(cursor == 0, "cursor beyond available log");
                return Ok(
                    json!({"job_id":job_id,"stream":stream,"cursor":0,"next_cursor":0,"bytes":0,"text":"","eof":status.terminal(),"state":status.state,"invalid_utf8":false}),
                );
            }
            Err(error) => return Err(error.into()),
        };
        let length = file.metadata()?.len();
        ensure!(cursor <= length, "cursor beyond available log");
        file.seek(SeekFrom::Start(cursor))?;
        let mut bytes = Vec::new();
        file.take(limit as u64).read_to_end(&mut bytes)?;
        let (text, consumed, invalid) =
            decode_chunk(&bytes, status.terminal() && cursor + bytes.len() as u64 == length);
        Ok(
            json!({"job_id":job_id,"stream":stream,"cursor":cursor,"next_cursor":cursor+consumed as u64,"bytes":consumed,"text":text,"invalid_utf8":invalid,"eof":status.terminal() && cursor+consumed as u64==length,"available_bytes":length,"state":status.state,"artifact":{"id":format!("{job_id}:{stream}"),"sha256":if stream=="stdout" { status.stdout_sha256 } else { status.stderr_sha256 }}}),
        )
    }

    fn job_dir(&self, id: &str) -> Result<PathBuf> {
        ensure!(
            id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "invalid job_id"
        );
        let dir = self.directory.join(id);
        ensure!(dir.is_dir(), "job not found in this device/session owner scope");
        Ok(dir)
    }
    fn request(&self, dir: &Path) -> Result<Request> {
        let request: Request = read_json(&dir.join("request.json"))?;
        validate_request(&request, dir)?;
        ensure!(request.owner == self.policy.owner, "job owner mismatch");
        Ok(request)
    }
    fn parse_spec(&self, args: &Value) -> Result<Spec> {
        let object = args.as_object().context("job arguments must be an object")?;
        ensure!(
            object.keys().all(|k| matches!(k.as_str(), "op_id" | "language" | "script" | "cwd" | "timeout")),
            "unsupported job argument; lease/owner cannot be supplied by a tool"
        );
        let op_id = string(args, "op_id")?.to_owned();
        ensure!(!op_id.is_empty() && op_id.len() <= 128, "op_id must be 1..128 bytes");
        let language = string(args, "language")?.to_owned();
        ensure!(matches!(language.as_str(), "python" | "powershell" | "cmd" | "sh"), "unsupported job interpreter");
        ensure!(language != "cmd" || cfg!(windows), "cmd jobs require Windows");
        let script = string(args, "script")?.to_owned();
        ensure!(!script.is_empty() && script.len() <= SCRIPT_LIMIT, "job script must be 1..65536 bytes");
        let cwd = match args.get("cwd") {
            Some(value) => {
                let path = Path::new(value.as_str().context("cwd must be a string")?);
                if path.is_absolute() {
                    path.to_owned()
                } else {
                    self.work_root.join(path)
                }
            }
            None => self.work_root.clone(),
        }
        .canonicalize()
        .context("job cwd does not exist")?;
        ensure!(cwd.is_dir(), "job cwd must be a directory");
        let timeout_seconds = args
            .get("timeout")
            .map(|v| v.as_u64().context("timeout must be a positive integer"))
            .transpose()?
            .unwrap_or(self.policy.max_seconds.min(600));
        ensure!(timeout_seconds > 0 && timeout_seconds <= self.policy.max_seconds, "timeout exceeds Host job contract");
        Ok(Spec { op_id, language, script, cwd, timeout_seconds })
    }
}

pub fn tools(manager: Arc<Manager>) -> Vec<Arc<dyn AgentTool>> {
    [
        ("job_start", "Start a durable background script under an explicit Host job lease; returns immediately with a stable job_id. It can outlive this model Run. Choose a unique op_id; retrying identical input returns the existing job and never reruns it. Existing op_id with different input is rejected. Relative data paths resolve in cwd (the Run working directory by default). The generated script file lives in a private Job directory: Python __file__ and PowerShell PSScriptRoot identify that script directory, not cwd. Use relative input/output paths or explicit data paths instead of locating working files beside the script. No shell sandbox; effects may remain after cancellation. Lease, ownership and quotas are fixed by the Host.", json!({"op_id":{"type":"string"},"language":{"type":"string","enum":["python","powershell","cmd","sh"]},"script":{"type":"string"},"cwd":{"type":"string"},"timeout":{"type":"integer","minimum":1,"maximum":86400}}), vec!["op_id","language","script"]),
        ("job_status", "Read a job receipt in this device/session owner scope. Interrupted means worker ownership is missing and effects are unknown; status never restarts a command.", json!({"job_id":{"type":"string"}}), vec!["job_id"]),
        ("job_read_log", "Read a bounded stdout/stderr artifact chunk. Byte cursor and next_cursor support incremental reads; text is UTF-8 with invalid_utf8 disclosure. Full output is retained only within the fixed combined disk quota; exceeding it stops the job explicitly.", json!({"job_id":{"type":"string"},"stream":{"type":"string","enum":["stdout","stderr"]},"cursor":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":4,"maximum":32768}}), vec!["job_id","stream"]),
        ("job_cancel", "Request cancellation of an owned job. Only the worker that holds its OS lock terminates its process tree; cancellation is not confirmed until a terminal receipt. Completed filesystem/network effects are not undone.", json!({"job_id":{"type":"string"}}), vec!["job_id"]),
    ].into_iter().map(|(name, description, properties, required)| Arc::new(JobTool { manager: manager.clone(), schema: Tool { name: name.into(), description: description.into(), parameters: Schema(json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})), constrained_sampling: None } }) as Arc<dyn AgentTool>).collect()
}
/// For Plan mode or an owner that has no currently valid background-job lease.
pub fn read_tools(manager: Arc<Manager>) -> Vec<Arc<dyn AgentTool>> {
    tools(manager)
        .into_iter()
        .filter(|tool| matches!(tool.schema().name.as_str(), "job_status" | "job_read_log"))
        .collect()
}
struct JobTool {
    manager: Arc<Manager>,
    schema: Tool,
}
#[async_trait]
impl AgentTool for JobTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        &self.schema.name
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Sequential
    }
    async fn execute(
        &self,
        _: &str,
        args: Value,
        signal: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        let result = async {
            match self.schema.name.as_str() {
                "job_start" => Ok(serde_json::to_value(self.manager.start(&args).await?)?),
                "job_status" => Ok(serde_json::to_value(self.manager.status(string(&args, "job_id")?)?)?),
                "job_cancel" => Ok(serde_json::to_value(self.manager.cancel(string(&args, "job_id")?)?)?),
                "job_read_log" => self.manager.read_log(
                    string(&args, "job_id")?,
                    string(&args, "stream")?,
                    args.get("cursor")
                        .map(|v| v.as_u64().context("cursor must be nonnegative integer"))
                        .transpose()?
                        .unwrap_or(0),
                    args.get("limit")
                        .map(|v| v.as_u64().context("limit must be positive integer"))
                        .transpose()?
                        .unwrap_or(8192)
                        .try_into()
                        .context("limit overflow")?,
                ),
                _ => bail!("unknown job tool"),
            }
        }
        .await;
        result
            .and_then(|value: Value| Ok(AgentToolResult::text(serde_json::to_string(&value)?)))
            .map_err(|error: anyhow::Error| AgentError::Tool(format!("{error:#}")))
    }
}

/// Call at the beginning of main, before Config::parse/load or Node state lock.
/// Returns None for ordinary Node startup. The hidden entry point accepts only a
/// previously committed request directory; it cannot create a new job contract.
pub fn worker_entry_from_args() -> Option<Result<()>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).and_then(|v| v.to_str()) != Some(WORKER_FLAG) {
        return None;
    }
    Some((|| {
        ensure!(args.len() == 3, "worker requires exactly one job directory");
        let dir = PathBuf::from(&args[2]);
        ensure!(dir.is_absolute(), "worker path must be absolute");
        tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(worker(&dir))
    })())
}

fn spawn_worker(dir: &Path) -> Result<()> {
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command.arg(WORKER_FLAG).arg(dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000008); // DETACHED_PROCESS | CREATE_NO_WINDOW
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command.spawn().context("spawn independent job worker")?;
    // Reap only the worker handle without tying its lifetime to the Node. This
    // thread may disappear when Node exits; the OS reparents a living worker.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

async fn worker(dir: &Path) -> Result<()> {
    let request: Request = read_json(&dir.join("request.json"))?;
    validate_request(&request, dir)?;
    let lock = lock_file(&dir.join("worker.lock"))?;
    // Manager status holds this lock only briefly. Retry that startup race, but
    // reject a duplicate worker instead of blocking for the whole job lifetime.
    let mut owns_lock = false;
    for _ in 0..40 {
        match lock.try_lock_exclusive() {
            Ok(()) => {
                owns_lock = true;
                break;
            }
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
            Err(error) => return Err(error.into()),
        }
    }
    ensure!(owns_lock, "another worker already owns this job; no replay");
    let mut receipt: Receipt = read_json(&dir.join("receipt.json"))?;
    ensure!(
        receipt.job_id == request.job_id && receipt.worker_nonce == request.worker_nonce,
        "worker receipt identity mismatch"
    );
    // A duplicate hidden invocation must never start a second command, even if
    // the original worker crashed and released its lock.
    ensure!(receipt.state == "starting" && receipt.worker_pid.is_none(), "job worker was already admitted; no replay");
    receipt.worker_pid = Some(std::process::id());
    receipt.started_at = Some(now());
    receipt.state = "running".into();
    receipt.reason = "worker owns OS lock; process startup pending".into();
    write_json(&dir.join("receipt.json"), &receipt)?;
    let result = execute_worker(dir, &request, &mut receipt).await;
    if let Err(error) = result {
        receipt.state = "interrupted".into();
        receipt.effects_unknown = true;
        receipt.reason = format!("worker failure; no automatic replay: {error:#}");
    }
    receipt.finished_at = Some(now());
    receipt.stdout_bytes = file_len(&dir.join("stdout.bin"));
    receipt.stderr_bytes = file_len(&dir.join("stderr.bin"));
    receipt.stdout_sha256 = hash_file(&dir.join("stdout.bin")).ok();
    receipt.stderr_sha256 = hash_file(&dir.join("stderr.bin")).ok();
    write_json(&dir.join("receipt.json"), &receipt)?;
    drop(lock);
    Ok(())
}

async fn execute_worker(dir: &Path, request: &Request, receipt: &mut Receipt) -> Result<()> {
    if dir.join("cancel.json").exists() || now() >= request.lease_expires_at {
        receipt.state = if dir.join("cancel.json").exists() { "cancelled" } else { "lease_expired" }.into();
        receipt.effects_unknown = false;
        receipt.reason = "stopped before process dispatch".into();
        return Ok(());
    }
    let extension = match request.spec.language.as_str() {
        "python" => "py",
        "powershell" => "ps1",
        "cmd" => "cmd",
        "sh" => "sh",
        _ => bail!("unsupported job language"),
    };
    let source = dir.join(format!("source.{extension}"));
    let source_text = match extension {
        "ps1" => format!("\u{feff}{}", request.spec.script.trim_start_matches('\u{feff}')),
        "cmd" => format!(
            "@echo off\r\nchcp 65001 >nul\r\n{}\r\n",
            request.spec.script.replace("\r\n", "\n").replace('\n', "\r\n")
        ),
        _ => request.spec.script.clone(),
    };
    write_bytes(&source, source_text.as_bytes())?;
    let mut command = match request.spec.language.as_str() {
        "python" => {
            let mut c = Command::new(if cfg!(windows) { "python" } else { "python3" });
            c.arg("-u").arg(&source);
            c
        }
        "powershell" => {
            let bootstrap = dir.join("bootstrap.ps1");
            write_bytes(&bootstrap, b"param([string]$SourcePath)\n$ErrorActionPreference='Stop'\n[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false)\n$OutputEncoding=[Console]::OutputEncoding\n$global:LASTEXITCODE=0\n& $SourcePath\nif ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }\n")?;
            let mut c = Command::new(if cfg!(windows) { "powershell.exe" } else { "pwsh" });
            c.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(bootstrap)
                .arg(&source);
            c
        }
        "cmd" => {
            let mut c = Command::new("cmd.exe");
            c.args(["/d", "/c"]).arg(&source);
            c
        }
        "sh" => {
            let mut c = Command::new("sh");
            c.arg(&source);
            c
        }
        _ => bail!("unsupported interpreter"),
    };
    command
        .current_dir(&request.spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // The worker only needs its fixed job contract. Node enrollment/model grants
    // must not be inherited by an arbitrary user script.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("HW_") {
            command.env_remove(key);
        }
    }
    command.env("PYTHONIOENCODING", "utf-8");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    #[cfg(windows)]
    command.creation_flags(0x08000004); // suspended until in JobObject
                                        // Recheck after script/metadata fsync and fix a monotonic upper bound so a
                                        // later wall-clock rollback cannot silently extend the admitted lease.
    let lease_remaining = request.lease_expires_at.saturating_sub(now());
    if lease_remaining == 0 || dir.join("cancel.json").exists() {
        receipt.state = if lease_remaining == 0 { "lease_expired" } else { "cancelled" }.into();
        receipt.effects_unknown = false;
        receipt.reason = "stopped before process dispatch".into();
        return Ok(());
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            receipt.state = "failed".into();
            receipt.effects_unknown = false;
            receipt.reason = format!("interpreter could not be spawned; no process dispatched: {error}");
            return Ok(());
        }
    };
    let pid = child.id().context("managed process has no PID")?;
    let tree = match ProcessTree::new(pid) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(error);
        }
    };
    #[cfg(windows)]
    if let Err(error) = resume_process(pid) {
        tree.terminate();
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Err(error);
    }
    let total = Arc::new(AtomicU64::new(0));
    let quota_hit = CancellationToken::new();
    let stdout = tokio::spawn(pump(
        child.stdout.take().context("missing stdout")?,
        dir.join("stdout.bin"),
        total.clone(),
        request.max_log_bytes,
        quota_hit.clone(),
    ));
    let stderr = tokio::spawn(pump(
        child.stderr.take().context("missing stderr")?,
        dir.join("stderr.bin"),
        total,
        request.max_log_bytes,
        quota_hit.clone(),
    ));
    let lease_is_deadline = lease_remaining <= request.spec.timeout_seconds;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(request.spec.timeout_seconds.min(lease_remaining));
    let outcome = loop {
        tokio::select! {
            result=child.wait()=>break ("exited",Some(result?)),
            _=tokio::time::sleep_until(deadline)=>break (if lease_is_deadline { "lease_expired" } else { "timed_out" },None),
            _=quota_hit.cancelled()=>break ("log_limit",None),
            _=tokio::time::sleep(Duration::from_millis(100))=> {
                if dir.join("cancel.json").exists() { break ("cancelled",None); }
                if now()>=request.lease_expires_at { break ("lease_expired",None); }
            }
        }
    };
    // Also clean descendants after the root exits. A child retaining pipe handles
    // must not keep a completed job alive or escape its lease.
    tree.terminate();
    if outcome.1.is_none() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    let drained = tokio::time::timeout(Duration::from_secs(5), async {
        stdout.await??;
        stderr.await??;
        Ok::<_, anyhow::Error>(())
    })
    .await;
    match drained {
        Ok(result) => result?,
        Err(_) => bail!("job output cleanup timed out; external effects may remain"),
    }
    receipt.exit_code = outcome.1.and_then(|s| s.code());
    receipt.state = if quota_hit.is_cancelled() {
        "log_limit"
    } else if outcome.0 == "exited" {
        if receipt.exit_code == Some(0) {
            "succeeded"
        } else {
            "failed"
        }
    } else {
        outcome.0
    }
    .into();
    receipt.effects_unknown = !matches!(receipt.state.as_str(), "succeeded" | "failed");
    receipt.reason = if receipt.state == "succeeded" {
        "process exited 0; descendants cleaned; this is not Goal acceptance"
    } else if receipt.state == "failed" {
        "process exited nonzero; partial effects may remain"
    } else {
        "process tree stopped; partial effects may remain; no automatic replay"
    }
    .into();
    Ok(())
}

async fn pump(
    mut input: impl AsyncRead + Unpin,
    path: PathBuf,
    total: Arc<AtomicU64>,
    maximum: u64,
    stopped: CancellationToken,
) -> Result<()> {
    let result = async {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = tokio::fs::File::from_std(options.open(path)?);
        let mut bytes = [0u8; 8192];
        loop {
            let length = input.read(&mut bytes).await?;
            if length == 0 {
                break;
            }
            let before = total.fetch_add(length as u64, Ordering::SeqCst);
            let keep = maximum.saturating_sub(before).min(length as u64) as usize;
            if keep > 0 {
                output.write_all(&bytes[..keep]).await?;
            }
            if keep < length {
                stopped.cancel();
            }
        }
        output.flush().await?;
        output.sync_all().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if result.is_err() {
        stopped.cancel();
    }
    result
}

fn validate_request(request: &Request, dir: &Path) -> Result<()> {
    ensure!(request.version == VERSION && request.job_id.len() == 64, "unsupported job request");
    ensure!(
        dir.file_name().and_then(|s| s.to_str()) == Some(request.job_id.as_str()),
        "job directory identity mismatch"
    );
    ensure!(request.input_hash == digest(&serde_json::to_vec(&request.spec)?), "job request input hash mismatch");
    ensure!(request.worker_nonce.len() == 32, "invalid worker identity");
    ensure!(
        request.max_log_bytes >= 1024
            && request.max_log_bytes <= 16 * 1024 * 1024
            && request.spec.timeout_seconds > 0
            && request.spec.timeout_seconds <= 86400,
        "invalid persisted job limits"
    );
    ensure!(request.spec.script.len() <= SCRIPT_LIMIT && request.spec.cwd.is_absolute(), "invalid persisted job input");
    Ok(())
}
fn string<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| anyhow!("missing string: {key}"))
}
fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn nonce() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("random nonce: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn lock_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|s| s.len()).unwrap_or(0)
}
fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut b = [0u8; 8192];
    loop {
        let n = file.read(&mut b)?;
        if n == 0 {
            break;
        }
        hash.update(&b[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let file = File::open(path)?;
    ensure!(file.metadata()?.len() <= META_LIMIT, "job metadata too large");
    Ok(serde_json::from_reader(file.take(META_LIMIT + 1))?)
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() as u64 <= META_LIMIT, "job metadata exceeds quota");
    write_bytes(path, &bytes)
}
fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension(format!("{}.tmp", nonce()?));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)?;
        sync_dir(path.parent().context("metadata parent")?)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}
#[cfg(unix)]
fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
#[cfg(not(unix))]
fn sync_dir(_: &Path) -> Result<()> {
    Ok(())
}
#[cfg(unix)]
fn private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}
#[cfg(not(unix))]
fn private_dir(_: &Path) -> Result<()> {
    Ok(())
}

fn decode_chunk(bytes: &[u8], final_chunk: bool) -> (String, usize, bool) {
    let mut text = String::new();
    let mut used = 0;
    let mut invalid = false;
    // Bound the UTF-8 payload itself, including replacement characters, rather
    // than merely bounding raw input (invalid bytes could otherwise expand 3x).
    let budget = bytes.len().max(3);
    while used < bytes.len() {
        let first = bytes[used];
        let width = match first {
            0..=0x7f => 1,
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => 0,
        };
        if width > 0 && used + width > bytes.len() && !final_chunk {
            break;
        }
        let valid = width > 0 && used + width <= bytes.len() && std::str::from_utf8(&bytes[used..used + width]).is_ok();
        if valid {
            if text.len() + width > budget {
                break;
            }
            text.push_str(std::str::from_utf8(&bytes[used..used + width]).expect("validated UTF-8"));
            used += width;
        } else {
            if text.len() + 3 > budget {
                break;
            }
            text.push('\u{fffd}');
            used += 1;
            invalid = true;
        }
    }
    (text, used, invalid)
}

#[cfg(unix)]
struct ProcessTree(u32, std::sync::atomic::AtomicBool);
#[cfg(unix)]
impl ProcessTree {
    fn new(pid: u32) -> Result<Self> {
        Ok(Self(pid, std::sync::atomic::AtomicBool::new(false)))
    }
    fn terminate(&self) {
        if !self.1.swap(true, Ordering::SeqCst) {
            unsafe {
                libc::kill(-(self.0 as i32), libc::SIGKILL);
            }
        }
    }
}
#[cfg(unix)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(windows)]
struct ProcessTree(isize);
#[cfg(windows)]
unsafe impl Send for ProcessTree {}
#[cfg(windows)]
impl ProcessTree {
    fn new(pid: u32) -> Result<Self> {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::{JobObjects::*, Threading::*},
        };
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
            {
                let e = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(e.into());
            }
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                let e = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(e.into());
            }
            let assigned = AssignProcessToJobObject(job, process);
            let error = std::io::Error::last_os_error();
            CloseHandle(process);
            if assigned == 0 {
                CloseHandle(job);
                return Err(error.into());
            }
            Ok(Self(job as isize))
        }
    }
    fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0 as _, 1);
        }
    }
}
#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0 as _);
        }
    }
}
#[cfg(windows)]
fn resume_process(pid: u32) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{Diagnostics::ToolHelp::*, Threading::*},
    };
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        ensure!(snapshot != INVALID_HANDLE_VALUE, "thread snapshot failed");
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut available = Thread32First(snapshot, &mut entry);
        let mut resumed = false;
        let mut failure = None;
        while available != 0 {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if thread.is_null() {
                    failure = Some(std::io::Error::last_os_error());
                    break;
                }
                let result = ResumeThread(thread);
                let error = std::io::Error::last_os_error();
                CloseHandle(thread);
                if result == u32::MAX {
                    failure = Some(error);
                    break;
                }
                resumed = true;
            }
            available = Thread32Next(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        if let Some(error) = failure {
            return Err(error.into());
        }
        ensure!(resumed, "no suspended managed thread found");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("hw-jobs-{}", nonce().unwrap()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn manager(temp: &Temp, session: &str) -> Manager {
        Manager::new(
            &temp.0,
            &temp.0,
            Policy {
                owner: Owner::new(&temp.0, "device-test", session).unwrap(),
                lease_expires_at: now() + 60,
                max_jobs: 4,
                max_log_bytes: 4096,
                max_seconds: 30,
            },
        )
        .unwrap()
    }
    #[test]
    fn utf8_cursor_preserves_partial_codepoint_and_discloses_invalid() {
        let b = "中🙂".as_bytes();
        let (text, n, invalid) = decode_chunk(&b[..5], false);
        assert_eq!((text, n, invalid), ("中".into(), 3, false));
        let (text, n, invalid) = decode_chunk(&[255, b'x', b'y', b'z'], true);
        assert!(invalid && n == 2 && text.ends_with('x') && text.len() == 4);
    }
    #[test]
    fn lease_owner_and_path_arguments_are_host_owned() {
        let temp = Temp::new();
        let manager = manager(&temp, "one");
        let args = json!({"op_id":"o1","language":"python","script":"print(1)"});
        assert!(manager.parse_spec(&args).is_ok());
        let mut bad = args.clone();
        bad["lease_expires_at"] = json!(u64::MAX);
        assert!(manager.parse_spec(&bad).is_err());
        assert!(manager.job_dir("../../x").is_err());
        assert_ne!(manager.directory, super::tests::manager(&temp, "two").directory);
    }
    #[test]
    fn repeated_intent_never_replays_and_input_conflict_is_rejected() {
        let temp = Temp::new();
        let manager = manager(&temp, "one");
        let spec = manager.parse_spec(&json!({"op_id":"once","language":"python","script":"print(1)"})).unwrap();
        let id = digest(
            format!("{}:{}", digest(&serde_json::to_vec(&manager.policy.owner).unwrap()), spec.op_id).as_bytes(),
        );
        let dir = manager.directory.join(&id);
        std::fs::create_dir(&dir).unwrap();
        let request = Request {
            version: VERSION,
            job_id: id.clone(),
            owner: manager.policy.owner.clone(),
            input_hash: digest(&serde_json::to_vec(&spec).unwrap()),
            worker_nonce: nonce().unwrap(),
            created_at: now() - 20,
            lease_expires_at: now() + 10,
            max_log_bytes: 4096,
            spec,
        };
        write_json(&dir.join("request.json"), &request).unwrap();
        write_json(&dir.join("receipt.json"), &Receipt::initial(&request)).unwrap();
        assert_eq!(manager.status(&id).unwrap().state, "interrupted");
        let other = super::tests::manager(&temp, "two");
        assert!(other.status(&id).is_err());
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let same = json!({"op_id":"once","language":"python","script":"print(1)"});
        assert_eq!(rt.block_on(manager.start(&same)).unwrap().state, "interrupted");
        let changed = json!({"op_id":"once","language":"python","script":"print(2)"});
        assert!(rt.block_on(manager.start(&changed)).is_err());
        let another = json!({"op_id":"another","language":"python","script":"print(3)"});
        assert!(rt.block_on(manager.start(&another)).unwrap_err().to_string().contains("interrupted"));
        assert!(!manager.ready_to_continue().unwrap());
        manager.cancel_all().unwrap();
        assert!(dir.join("cancel.json").exists());
    }

    #[test]
    fn lock_inventory_and_incremental_log_reads_do_not_rely_on_pid() {
        let temp = Temp::new();
        let mut manager = manager(&temp, "logs");
        let spec = manager.parse_spec(&json!({"op_id":"logs","language":"python","script":"print(1)"})).unwrap();
        let id = digest(
            format!("{}:{}", digest(&serde_json::to_vec(&manager.policy.owner).unwrap()), spec.op_id).as_bytes(),
        );
        let dir = manager.directory.join(&id);
        std::fs::create_dir(&dir).unwrap();
        let request = Request {
            version: VERSION,
            job_id: id.clone(),
            owner: manager.policy.owner.clone(),
            input_hash: digest(&serde_json::to_vec(&spec).unwrap()),
            worker_nonce: nonce().unwrap(),
            created_at: now() - 20,
            lease_expires_at: now() + 10,
            max_log_bytes: 4096,
            spec,
        };
        write_json(&dir.join("request.json"), &request).unwrap();
        let mut receipt = Receipt::initial(&request);
        receipt.state = "running".into();
        receipt.worker_pid = Some(u32::MAX);
        write_json(&dir.join("receipt.json"), &receipt).unwrap();
        let worker_lock = lock_file(&dir.join("worker.lock")).unwrap();
        worker_lock.lock_exclusive().unwrap();
        assert_eq!(manager.status(&id).unwrap().state, "running");
        assert_eq!(manager.unfinished().unwrap().len(), 1);
        assert!(!manager.ready_to_continue().unwrap());
        write_bytes(&dir.join("stdout.bin"), "中🙂".as_bytes()).unwrap();
        let first = manager.read_log(&id, "stdout", 0, 4).unwrap();
        assert_eq!(first["text"], "中");
        assert_eq!(first["next_cursor"], 3);
        let second = manager.read_log(&id, "stdout", 3, 4).unwrap();
        assert_eq!(second["text"], "🙂");
        assert_eq!(second["next_cursor"], 7);
        assert!(manager.read_log(&id, "stdout", 8, 4).is_err());
        assert!(manager.read_log(&id, "../request", 0, 4).is_err());
        manager.cancel_all().unwrap();
        assert!(dir.join("cancel.json").exists());
        drop(worker_lock);
        assert_eq!(manager.status(&id).unwrap().state, "interrupted");
        assert!(!manager.ready_to_continue().unwrap());
        receipt.state = "cancelled".into();
        receipt.finished_at = Some(now());
        write_json(&dir.join("receipt.json"), &receipt).unwrap();
        manager.policy.lease_expires_at = now() - 1;
        assert!(manager.ready_to_continue().unwrap());
        assert_eq!(manager.status(&id).unwrap().state, "cancelled");
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        assert!(rt
            .block_on(manager.start(&json!({"op_id":"expired-new","language":"python","script":"print(2)"})))
            .unwrap_err()
            .to_string()
            .contains("expired"));
        let readonly = read_tools(Arc::new(manager));
        assert_eq!(readonly.len(), 2);
        assert!(readonly.iter().all(|tool| matches!(tool.schema().name.as_str(), "job_status" | "job_read_log")));
    }
}
