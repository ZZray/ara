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
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::Command,
};
use tokio_util::sync::CancellationToken;

const VERSION: u32 = 1;
const SCRIPT_LIMIT: usize = 64 * 1024;
const READ_LIMIT: usize = 32 * 1024;
const META_LIMIT: u64 = 256 * 1024;
const START_GRACE: u64 = 10;
const WORKER_FLAG: &str = "--managed-job-worker";
const INPUT_LIMIT: usize = 8192;
const INPUT_TOTAL_LIMIT: usize = 65536;
const INPUT_COUNT_LIMIT: usize = 64;
const INPUT_FRAME_LIMIT: usize = 65536;
const TERMINAL_INPUT_COUNT: usize = 2048;
const TERMINAL_INPUT_BYTES: usize = 1024 * 1024;
const INPUT_JOURNAL_LIMIT: u64 = 2 * 1024 * 1024;

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
    pub worker_executable: Option<PathBuf>,
    pub owner: Owner,
    pub goal_id: String,
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
    // Omitting false preserves the serialized input hash of pre-interactive jobs.
    #[serde(default, skip_serializing_if = "is_false")]
    interactive: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    terminal: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    rows: u16,
    #[serde(default, skip_serializing_if = "is_zero")]
    cols: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    job_id: String,
    owner: Owner,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    goal_id: String,
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
    #[serde(default)]
    pub terminal: bool,
    #[serde(default)]
    pub rows: u16,
    #[serde(default)]
    pub cols: u16,
    #[serde(default)]
    pub process_pid: Option<u32>,
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
    #[serde(default)]
    pub interactive: bool,
    #[serde(default)]
    pub stdin_closed: bool,
}
impl Receipt {
    fn initial(request: &Request) -> Self {
        Self {
            job_id: request.job_id.clone(),
            terminal: request.spec.terminal,
            rows: request.spec.rows,
            cols: request.spec.cols,
            process_pid: None,
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
            interactive: request.spec.interactive,
            stdin_closed: false,
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
        let worker_executable = self
            .policy
            .worker_executable
            .as_ref()
            .context("Host has not configured a compatible managed-job worker executable")?;
        ensure!(
            worker_executable.is_absolute() && worker_executable.is_file(),
            "worker executable must be an existing absolute file"
        );
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
            goal_id: self.policy.goal_id.clone(),
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
        let spawned = spawn_worker(worker_executable, &dir);
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
            if receipt.state != "starting"
                && (!request.spec.interactive || receipt.terminal() || dir.join("input-endpoint.json").exists())
            {
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
        receipt.interactive = request.spec.interactive;
        receipt.stdin_closed = request.spec.interactive && (receipt.terminal() || load_inputs(&dir)?.closed);
        receipt.terminal = request.spec.terminal;
        if request.spec.terminal {
            let inputs = load_inputs(&dir)?;
            let resize = inputs.entries.iter().rev().find(|r| r.kind == "resize" && r.state == "written");
            receipt.rows = resize.map_or(request.spec.rows, |r| r.rows);
            receipt.cols = resize.map_or(request.spec.cols, |r| r.cols);
        }
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

    /// Input payloads only travel over the worker's loopback connection. Neither
    /// this call nor the durable input journal stores or returns their text.
    /// A lost ACK must be reconciled with input_status; never invent a new ID to retry.
    pub async fn write_input(&self, job_id: &str, input_id: &str, text: &str, eof: bool) -> Result<Value> {
        validate_input(input_id, text, eof)?;
        self.control(job_id, input_id, text, eof, "input", 0, 0, "").await
    }

    pub async fn resize(&self, job_id: &str, input_id: &str, rows: u16, cols: u16) -> Result<Value> {
        validate_size(rows, cols)?;
        self.control(job_id, input_id, "", false, "resize", rows, cols, "").await
    }

    pub async fn signal(&self, job_id: &str, input_id: &str, signal: &str) -> Result<Value> {
        ensure!(matches!(signal, "interrupt" | "eof"), "unsupported terminal signal");
        self.control(job_id, input_id, "", false, "signal", 0, 0, signal).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn control(
        &self,
        job_id: &str,
        input_id: &str,
        text: &str,
        eof: bool,
        kind: &str,
        rows: u16,
        cols: u16,
        signal: &str,
    ) -> Result<Value> {
        validate_input_id(input_id)?;
        let dir = self.job_dir(job_id)?;
        let request = self.request(&dir)?;
        ensure!(request.spec.interactive, "job was not started with interactive input");
        ensure!(kind == "input" || request.spec.terminal, "operation requires a terminal Job");
        ensure!(!request.spec.terminal || !eof, "terminal EOF uses job_terminal_signal eof, not pipe close");
        ensure!(
            !self.policy.goal_id.is_empty() && request.goal_id == self.policy.goal_id,
            "job input belongs to a different Goal contract"
        );
        ensure!(now() < self.policy.lease_expires_at && now() < request.lease_expires_at, "job input lease expired");
        let message = InputMessage {
            worker_nonce: request.worker_nonce.clone(),
            host_lease_expires_at: self.policy.lease_expires_at.min(request.lease_expires_at),
            input_id: input_id.into(),
            text: text.into(),
            eof,
            kind: kind.into(),
            rows,
            cols,
            signal: signal.into(),
        };
        let hash = message_hash(&message)?;
        // Completed input receipts remain queryable after the command exits. The
        // identity conflict check still applies; this path cannot cause effects.
        if let Some(previous) = load_inputs(&dir)?.entries.iter().find(|entry| entry.input_id == input_id) {
            ensure!(previous.input_hash == hash, "input_id already used with different input; no input replayed");
            return input_value(job_id, previous);
        }
        let status = self.status(job_id)?;
        ensure!(!status.terminal() && status.state == "running", "job is not active; input was not dispatched");
        ensure!(!dir.join("cancel.json").exists(), "job cancellation requested; input was not dispatched");
        let endpoint: InputEndpoint =
            read_json(&dir.join("input-endpoint.json")).context("interactive worker input endpoint is not ready")?;
        ensure!(
            endpoint.worker_nonce == request.worker_nonce && endpoint.port != 0,
            "input endpoint identity mismatch"
        );
        // Do not include socket/request errors (which may carry caller data) in
        // errors returned to the model or Web transport.
        let exchange = async {
            let mut stream = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, endpoint.port)).await?;
            write_frame(&mut stream, &message).await?;
            let response: InputReply = read_frame(&mut stream).await?;
            Ok::<_, anyhow::Error>(response)
        };
        let response = match tokio::time::timeout(Duration::from_secs(5), exchange).await {
            Ok(Ok(response)) => response,
            _ => bail!("input acknowledgement unavailable; effects unknown; query input_status with the same input_id, do not resend under a new ID"),
        };
        ensure!(response.worker_nonce == message.worker_nonce, "input response identity mismatch");
        if let Some(error) = response.error {
            bail!("{error}");
        }
        let receipt = response.receipt.context("input worker returned no receipt")?;
        ensure!(receipt.input_id == input_id && receipt.input_hash == hash, "input response receipt mismatch");
        input_value(job_id, &receipt)
    }

    /// Metadata-only reconciliation after a timeout or lost acknowledgement.
    /// Absent never authorizes a new input ID: the original request may be in flight.
    pub fn input_status(&self, job_id: &str, input_id: &str) -> Result<Value> {
        validate_input_id(input_id)?;
        let dir = self.job_dir(job_id)?;
        self.request(&dir)?;
        if let Some(receipt) = load_inputs(&dir)?.entries.iter().find(|entry| entry.input_id == input_id) {
            return input_value(job_id, receipt);
        }
        Ok(
            json!({"job_id":job_id,"input_id":input_id,"state":"not_observed","effects_unknown":true,"reason":"no durable input receipt observed; an in-flight request is possible; retain the same input_id"}),
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
            object.keys().all(|k| matches!(
                k.as_str(),
                "op_id" | "language" | "script" | "cwd" | "timeout" | "interactive" | "terminal" | "rows" | "cols"
            )),
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
        let terminal =
            args.get("terminal").map(|v| v.as_bool().context("terminal must be boolean")).transpose()?.unwrap_or(false);
        let interactive = terminal
            || args
                .get("interactive")
                .map(|v| v.as_bool().context("interactive must be boolean"))
                .transpose()?
                .unwrap_or(false);
        let rows = if terminal { bounded_size(args, "rows", 24)? } else { 0 };
        let cols = if terminal { bounded_size(args, "cols", 80)? } else { 0 };
        if terminal {
            validate_size(rows, cols)?;
        } else {
            ensure!(!args.get("rows").is_some() && !args.get("cols").is_some(), "terminal size requires terminal=true");
        }
        Ok(Spec { op_id, language, script, cwd, timeout_seconds, interactive, terminal, rows, cols })
    }
}

pub fn tools(manager: Arc<Manager>) -> Vec<Arc<dyn AgentTool>> {
    let worker_enabled = manager.policy.worker_executable.is_some();
    [
        ("job_start", "Start a durable background script (interactive=true keeps piped stdin open; terminal=true instead creates a native PTY/ConPTY with merged stdout/stderr, keyboard input and resize) under an explicit Host job lease; returns immediately with a stable job_id. It can outlive this model Run. Choose a unique op_id; retrying identical input returns the existing job and never reruns it. Existing op_id with different input is rejected. Relative data paths resolve in cwd (the Run working directory by default). The generated script file lives in a private Job directory: Python __file__ and PowerShell PSScriptRoot identify that script directory, not cwd. Use relative input/output paths or explicit data paths instead of locating working files beside the script. No shell sandbox; effects may remain after cancellation. Lease, ownership and quotas are fixed by the Host.", json!({"op_id":{"type":"string"},"language":{"type":"string","enum":["python","powershell","cmd","sh"]},"script":{"type":"string"},"cwd":{"type":"string"},"timeout":{"type":"integer","minimum":1,"maximum":86400},"interactive":{"type":"boolean","default":false},"terminal":{"type":"boolean","default":false},"rows":{"type":"integer","minimum":10,"maximum":300},"cols":{"type":"integer","minimum":20,"maximum":500}}), vec!["op_id","language","script"]),
        ("job_status", "Read a job receipt in this device/session owner scope. Interrupted means worker ownership is missing and effects are unknown; status never restarts a command.", json!({"job_id":{"type":"string"}}), vec!["job_id"]),
        ("job_read_log", "Read a bounded stdout/stderr artifact chunk. Byte cursor and next_cursor support incremental reads; text is UTF-8 with invalid_utf8 disclosure. Full output is retained only within the fixed combined disk quota; exceeding it stops the job explicitly.", json!({"job_id":{"type":"string"},"stream":{"type":"string","enum":["stdout","stderr"]},"cursor":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":4,"maximum":32768}}), vec!["job_id","stream"]),
        ("job_write_input", "Write exact UTF-8 text to an active owned interactive job's input. Include a newline for pipe input or carriage return for terminal line input when needed. Use a stable unique input_id; identical repeats return metadata and never repeat a write, while pending/unknown must be reconciled and never retried under a new ID. eof=true closes stdin after this write. Limit 8 KiB each; pipe jobs allow 64 KiB/64 inputs, terminal jobs 1 MiB/2048 control operations. eof=true is pipe-only; use job_terminal_signal for a terminal. Passwords and authorization codes should use the dedicated Web input channel, not tool arguments or chat. The child may echo inputs into output logs.", json!({"job_id":{"type":"string"},"input_id":{"type":"string"},"text":{"type":"string","maxLength":8192},"eof":{"type":"boolean","default":false}}), vec!["job_id","input_id","text"]),
        ("job_terminal_resize", "Resize an active owned native terminal. Stable input_id makes the operation idempotent; reconcile unknown ACK with job_input_status.", json!({"job_id":{"type":"string"},"input_id":{"type":"string"},"rows":{"type":"integer","minimum":10,"maximum":300},"cols":{"type":"integer","minimum":20,"maximum":500}}), vec!["job_id","input_id","rows","cols"]),
        ("job_terminal_signal", "Send terminal interrupt (Ctrl+C) or EOF control key (Unix Ctrl+D, Windows Ctrl+Z then Enter). This does not cancel the Job or permanently close terminal input. Programs can handle or ignore these keys. Reconcile uncertain ACK using the same input_id.", json!({"job_id":{"type":"string"},"input_id":{"type":"string"},"signal":{"type":"string","enum":["interrupt","eof"]}}), vec!["job_id","input_id","signal"]),
        ("job_input_status", "Read metadata for a stable input_id after a lost acknowledgement. No input text or hashes are returned. pending/unknown means effects may have occurred; not_observed may still be in flight. Never resend using a different ID.", json!({"job_id":{"type":"string"},"input_id":{"type":"string"}}), vec!["job_id","input_id"]),
        ("job_cancel", "Request cancellation of an owned job. Only the worker that holds its OS lock terminates its process tree; cancellation is not confirmed until a terminal receipt. Completed filesystem/network effects are not undone.", json!({"job_id":{"type":"string"}}), vec!["job_id"]),
    ].into_iter().filter(|(name,_,_,_)| worker_enabled || matches!(*name,"job_status"|"job_read_log"|"job_input_status")).map(|(name, description, properties, required)| Arc::new(JobTool { manager: manager.clone(), schema: Tool { name: name.into(), description: description.into(), parameters: Schema(json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})), constrained_sampling: None } }) as Arc<dyn AgentTool>).collect()
}
/// For Plan mode or an owner that has no currently valid background-job lease.
pub fn read_tools(manager: Arc<Manager>) -> Vec<Arc<dyn AgentTool>> {
    tools(manager)
        .into_iter()
        .filter(|tool| matches!(tool.schema().name.as_str(), "job_status" | "job_read_log" | "job_input_status"))
        .collect()
}
/// Continue an already admitted Job on a subsequent Run without admitting a
/// new command. The Host must still authorize the original Goal lease.
pub fn resume_tools(manager: Arc<Manager>) -> Vec<Arc<dyn AgentTool>> {
    tools(manager).into_iter().filter(|tool| tool.schema().name != "job_start").collect()
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
                "job_write_input" => {
                    self.manager
                        .write_input(
                            string(&args, "job_id")?,
                            string(&args, "input_id")?,
                            string(&args, "text")?,
                            args.get("eof")
                                .map(|v| v.as_bool().context("eof must be boolean"))
                                .transpose()?
                                .unwrap_or(false),
                        )
                        .await
                }
                "job_terminal_resize" => {
                    self.manager
                        .resize(
                            string(&args, "job_id")?,
                            string(&args, "input_id")?,
                            bounded_size(&args, "rows", 24)?,
                            bounded_size(&args, "cols", 80)?,
                        )
                        .await
                }
                "job_terminal_signal" => {
                    self.manager
                        .signal(string(&args, "job_id")?, string(&args, "input_id")?, string(&args, "signal")?)
                        .await
                }
                "job_input_status" => self.manager.input_status(string(&args, "job_id")?, string(&args, "input_id")?),
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
            .and_then(|mut value: Value| {
                if let Some(object) = value.as_object_mut() {
                    object.remove("worker_nonce");
                }
                Ok(AgentToolResult::text(serde_json::to_string(&value)?))
            })
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

fn spawn_worker(executable: &Path, dir: &Path) -> Result<()> {
    let mut command = std::process::Command::new(executable);
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
            c.args(["-NoLogo", "-NoProfile"]);
            if !request.spec.interactive {
                c.arg("-NonInteractive");
            }
            c.args(["-ExecutionPolicy", "Bypass", "-File"]).arg(bootstrap).arg(&source);
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
        .stdin(if request.spec.interactive { Stdio::piped() } else { Stdio::null() })
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
    if request.spec.terminal {
        command.env("TERM", "xterm-256color");
        return execute_terminal_worker(dir, request, receipt, command).await;
    }
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
    receipt.process_pid = Some(pid);
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
        None,
    ));
    let stderr = tokio::spawn(pump(
        child.stderr.take().context("missing stderr")?,
        dir.join("stderr.bin"),
        total,
        request.max_log_bytes,
        quota_hit.clone(),
        None,
    ));
    let lease_is_deadline = lease_remaining <= request.spec.timeout_seconds;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(request.spec.timeout_seconds.min(lease_remaining));
    let input_stop = CancellationToken::new();
    let input_task = if request.spec.interactive {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        write_json(
            &dir.join("input-endpoint.json"),
            &InputEndpoint { port: listener.local_addr()?.port(), worker_nonce: request.worker_nonce.clone() },
        )?;
        Some(tokio::spawn(serve_input(
            listener,
            child.stdin.take().context("missing interactive stdin")?,
            dir.to_owned(),
            request.clone(),
            deadline,
            input_stop.clone(),
            None,
        )))
    } else {
        None
    };
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
    // Stop accepting requests first. An already committed write is allowed its
    // bounded settlement even if the root exits immediately because of input.
    input_stop.cancel();
    // Also clean descendants after the root exits. A child retaining pipe handles
    // must not keep a completed job alive or escape its lease.
    tree.terminate();
    if outcome.1.is_none() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    if let Some(mut task) = input_task {
        match tokio::time::timeout(Duration::from_secs(4), &mut task).await {
            Ok(result) => result??,
            Err(_) => {
                task.abort();
                let _ = task.await;
                bail!("interactive input cleanup timed out; pending input effects remain unknown");
            }
        }
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

async fn execute_terminal_worker(dir: &Path, request: &Request, receipt: &mut Receipt, command: Command) -> Result<()> {
    let remaining = request.lease_expires_at.saturating_sub(now());
    ensure!(remaining > 0 && !dir.join("cancel.json").exists(), "terminal authority ended before spawn");
    let lease_is_deadline = remaining <= request.spec.timeout_seconds;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(remaining.min(request.spec.timeout_seconds));
    let mut process =
        crate::terminal_process::TerminalProcess::spawn(&command, request.spec.rows, request.spec.cols).await?;
    receipt.process_pid = Some(process.id());
    write_json(&dir.join("receipt.json"), receipt)?;
    let reader = process.take_reader().context("terminal output unavailable")?;
    let writer = process.take_writer().context("terminal input unavailable")?;
    let fault = CancellationToken::new();
    let control = TerminalControl {
        process: Arc::new(Mutex::new(process)),
        emulator: Arc::new(Mutex::new(crate::terminal_emulator::TerminalEmulator::new(
            request.spec.rows,
            request.spec.cols,
        ))),
    };
    let (responses, response_rx) = tokio::sync::mpsc::channel(128);
    let total = Arc::new(AtomicU64::new(0));
    let quota = CancellationToken::new();
    // A PTY supplies one merged byte stream. Keep the existing stdout artifact
    // and cursor contract; stderr remains an empty artifact, never a duplicate.
    write_bytes(&dir.join("stderr.bin"), b"")?;
    let output = tokio::spawn(pump(
        reader,
        dir.join("stdout.bin"),
        total,
        request.max_log_bytes,
        quota.clone(),
        Some(TerminalOutput { emulator: control.emulator.clone(), responses, fault: fault.clone() }),
    ));

    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    write_json(
        &dir.join("input-endpoint.json"),
        &InputEndpoint { port: listener.local_addr()?.port(), worker_nonce: request.worker_nonce.clone() },
    )?;
    let input_stop = CancellationToken::new();
    let input_dir = dir.to_owned();
    let input_request = request.clone();
    let input_shutdown = input_stop.clone();
    let input_fault = fault.clone();
    let input_control = control.clone();
    let mut input = tokio::spawn(async move {
        let mut guard = TaskFaultGuard { fault: input_fault.clone(), normal: false };
        let result = serve_input(
            listener,
            writer,
            input_dir,
            input_request,
            deadline,
            input_shutdown.clone(),
            Some(TerminalInput {
                control: input_control,
                responses: response_rx,
                replies_closed: false,
                fault: input_fault,
            }),
        )
        .await;
        guard.normal = input_shutdown.is_cancelled() && result.is_ok();
        result
    });
    let mut tick = tokio::time::interval(Duration::from_millis(25));
    let outcome = loop {
        tokio::select! {
            _=tokio::time::sleep_until(deadline)=>break(if lease_is_deadline {"lease_expired"}else{"timed_out"},None),
            _=quota.cancelled()=>break("log_limit",None),
            _=fault.cancelled()=>break("interrupted",None),
            _=tick.tick()=>{
                if dir.join("cancel.json").exists() {break("cancelled",None);}
                if now()>=request.lease_expires_at {break("lease_expired",None);}
                let status = control.process.lock().map_err(|_|anyhow!("terminal lock poisoned"))?.try_wait()?;
                if let Some(status)=status {break("exited",Some(status));}
            },
        }
    };
    input_stop.cancel();
    {
        let mut process = control.process.lock().map_err(|_| anyhow!("terminal lock poisoned"))?;
        process.terminate_tree()?;
        // The output pump remains active while ConPTY closes; Windows can
        // block closing its HPCON until the output pipe is drained.
        process.close_terminal();
    }
    match tokio::time::timeout(Duration::from_secs(4), &mut input).await {
        Ok(result) => result??,
        Err(_) => {
            input.abort();
            let _ = input.await;
            bail!("terminal input cleanup incomplete; input effects unknown");
        }
    }
    tokio::time::timeout(Duration::from_secs(5), output).await.context("terminal output cleanup timed out")???;
    receipt.exit_code = outcome.1.and_then(|s| s.code());
    receipt.state = if quota.is_cancelled() {
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
    receipt.reason =
        "native terminal settled; descendants cleaned; partial external effects may remain; no replay".into();
    receipt.stdin_closed = true;
    Ok(())
}

struct TaskFaultGuard {
    fault: CancellationToken,
    normal: bool,
}
impl Drop for TaskFaultGuard {
    fn drop(&mut self) {
        if !self.normal {
            self.fault.cancel();
        }
    }
}
#[derive(Clone)]
struct TerminalControl {
    process: Arc<Mutex<crate::terminal_process::TerminalProcess>>,
    emulator: Arc<Mutex<crate::terminal_emulator::TerminalEmulator>>,
}
struct TerminalInput {
    control: TerminalControl,
    responses: tokio::sync::mpsc::Receiver<Vec<u8>>,
    replies_closed: bool,
    fault: CancellationToken,
}
struct TerminalOutput {
    emulator: Arc<Mutex<crate::terminal_emulator::TerminalEmulator>>,
    responses: tokio::sync::mpsc::Sender<Vec<u8>>,
    fault: CancellationToken,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputEndpoint {
    port: u16,
    worker_nonce: String,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputMessage {
    worker_nonce: String,
    host_lease_expires_at: u64,
    input_id: String,
    text: String,
    eof: bool,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    rows: u16,
    #[serde(default)]
    cols: u16,
    #[serde(default)]
    signal: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputReceipt {
    input_id: String,
    input_hash: String,
    state: String,
    bytes: Option<usize>,
    eof: bool,
    effects_unknown: bool,
    reason: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    rows: u16,
    #[serde(default)]
    cols: u16,
    #[serde(default)]
    signal: String,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputJournal {
    entries: Vec<InputReceipt>,
    reserved_bytes: usize,
    closed: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputReply {
    worker_nonce: String,
    receipt: Option<InputReceipt>,
    error: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !value
}
fn is_zero(value: &u16) -> bool {
    *value == 0
}
fn validate_size(rows: u16, cols: u16) -> Result<()> {
    ensure!(
        (10..=300).contains(&rows) && (20..=500).contains(&cols),
        "terminal size requires 10..300 rows and 20..500 columns"
    );
    Ok(())
}
fn bounded_size(args: &Value, name: &str, default: u16) -> Result<u16> {
    args.get(name)
        .map(|v| v.as_u64().and_then(|n| u16::try_from(n).ok()).context("invalid terminal dimension"))
        .transpose()
        .map(|v| v.unwrap_or(default))
}
fn validate_input_id(input_id: &str) -> Result<()> {
    ensure!(
        !input_id.is_empty()
            && input_id.len() <= 128
            && input_id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':')),
        "input_id must be 1..128 ASCII letters, digits, or -_.:"
    );
    Ok(())
}
fn validate_input(input_id: &str, text: &str, eof: bool) -> Result<()> {
    validate_input_id(input_id)?;
    ensure!(text.len() <= INPUT_LIMIT, "input exceeds 8192 UTF-8 bytes");
    ensure!(!text.is_empty() || eof, "empty input requires eof=true");
    Ok(())
}
fn input_hash(worker_nonce: &str, input_id: &str, text: &str, eof: bool) -> Result<String> {
    // The private worker nonce prevents guessing short authorization codes by
    // enumerating an otherwise unsalted persisted input hash.
    Ok(digest(&serde_json::to_vec(&(worker_nonce, input_id, text, eof))?))
}
fn message_hash(message: &InputMessage) -> Result<String> {
    if matches!(message.kind.as_str(), "" | "input") {
        input_hash(&message.worker_nonce, &message.input_id, &message.text, message.eof)
    } else {
        Ok(digest(&serde_json::to_vec(&(
            &message.worker_nonce,
            &message.input_id,
            &message.kind,
            message.rows,
            message.cols,
            &message.signal,
        ))?))
    }
}
fn write_inputs(dir: &Path, journal: &InputJournal) -> Result<()> {
    let bytes = serde_json::to_vec(journal)?;
    ensure!(bytes.len() as u64 <= INPUT_JOURNAL_LIMIT, "input journal exceeds bound");
    write_bytes(&dir.join("inputs.json"), &bytes)
}
fn load_inputs(dir: &Path) -> Result<InputJournal> {
    let read = || -> Result<InputJournal> {
        let file = File::open(dir.join("inputs.json"))?;
        ensure!(file.metadata()?.len() <= INPUT_JOURNAL_LIMIT, "input journal exceeds bound");
        let mut bytes = Vec::new();
        file.take(INPUT_JOURNAL_LIMIT + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 <= INPUT_JOURNAL_LIMIT, "input journal exceeds bound");
        Ok(serde_json::from_slice(&bytes)?)
    };
    match read() {
        Ok(value) => Ok(value),
        Err(error)
            if error.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(InputJournal::default())
        }
        Err(error) => Err(error),
    }
}
fn input_value(job_id: &str, receipt: &InputReceipt) -> Result<Value> {
    Ok(json!({"job_id":job_id,"input_id":receipt.input_id,"state":receipt.state,"bytes":receipt.bytes,
        "eof":receipt.eof,"effects_unknown":receipt.effects_unknown,"reason":receipt.reason,
        "kind":if receipt.kind.is_empty(){"input"}else{&receipt.kind},"rows":receipt.rows,"cols":receipt.cols,"signal":receipt.signal}))
}
async fn write_frame(stream: &mut TcpStream, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= INPUT_FRAME_LIMIT, "input frame exceeds limit");
    stream.write_u32(bytes.len() as u32).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}
async fn read_frame<T: serde::de::DeserializeOwned>(stream: &mut TcpStream) -> Result<T> {
    let length = stream.read_u32().await? as usize;
    ensure!(length > 0 && length <= INPUT_FRAME_LIMIT, "input frame exceeds limit");
    let mut bytes = vec![0u8; length];
    stream.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn serve_input<W: tokio::io::AsyncWrite + Unpin + Send>(
    listener: TcpListener,
    stdin: W,
    dir: PathBuf,
    request: Request,
    deadline: tokio::time::Instant,
    stop: CancellationToken,
    mut terminal: Option<TerminalInput>,
) -> Result<()> {
    let mut stdin = Some(stdin);
    let mut journal = load_inputs(&dir)?;
    loop {
        let (mut stream, _) = tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            response = async {
                match terminal.as_mut() {
                    Some(t) if !t.replies_closed => t.responses.recv().await,
                    _ => std::future::pending().await,
                }
            } => {
                if let Some(data) = response {
                    if tokio::time::Instant::now() >= deadline || now() >= request.lease_expires_at || dir.join("cancel.json").exists() { break; }
                    // Terminal protocol replies come only from the live worker
                    // parser, never from replayed browser history or model data.
                    let reply = async {
                        let writer = stdin.as_mut().context("terminal input closed")?;
                        writer.write_all(&data).await?;
                        writer.flush().await?;
                        Ok::<_,anyhow::Error>(())
                    };
                    if !matches!(tokio::time::timeout(Duration::from_secs(2),reply).await,Ok(Ok(()))) {
                        terminal.as_ref().expect("terminal response source").fault.cancel();
                        break;
                    }
                } else if let Some(t) = terminal.as_mut() {
                    t.replies_closed = true;
                }
                continue;
            },
            result = listener.accept() => result?,
        };
        // A client that connects and never sends a frame cannot block Job
        // cancellation, lease cleanup, or command exit.
        let message: InputMessage = tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            result = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut stream)) => match result {
                Ok(Ok(message)) => message,
                _ => continue,
            },
        };
        if message.worker_nonce != request.worker_nonce {
            continue;
        }
        let result = dispatch_control(
            &dir,
            &request,
            deadline,
            &stop,
            &mut stdin,
            &mut journal,
            &message,
            terminal.as_ref().map(|t| &t.control),
        )
        .await;
        let reply = match result {
            Ok(receipt) => {
                InputReply { worker_nonce: request.worker_nonce.clone(), receipt: Some(receipt), error: None }
            }
            // All dispatch errors are local static validation/filesystem errors;
            // deliberately return a fixed message rather than interpolating data.
            Err(_) => InputReply {
                worker_nonce: request.worker_nonce.clone(),
                receipt: None,
                error: Some(
                    "input rejected or receipt persistence failed; query input_status; no automatic replay".into(),
                ),
            },
        };
        // ACK delivery failure is harmless: the durable receipt is authoritative.
        let _ = tokio::time::timeout(Duration::from_millis(500), write_frame(&mut stream, &reply)).await;
    }
    drop(stdin);
    Ok(())
}

#[cfg(test)]
async fn dispatch_input<W: tokio::io::AsyncWrite + Unpin>(
    dir: &Path,
    request: &Request,
    deadline: tokio::time::Instant,
    stop: &CancellationToken,
    stdin: &mut Option<W>,
    journal: &mut InputJournal,
    message: &InputMessage,
) -> Result<InputReceipt> {
    dispatch_control(dir, request, deadline, stop, stdin, journal, message, None).await
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_control<W: tokio::io::AsyncWrite + Unpin>(
    dir: &Path,
    request: &Request,
    deadline: tokio::time::Instant,
    stop: &CancellationToken,
    stdin: &mut Option<W>,
    journal: &mut InputJournal,
    message: &InputMessage,
    terminal: Option<&TerminalControl>,
) -> Result<InputReceipt> {
    validate_input_id(&message.input_id)?;
    let kind = if message.kind.is_empty() { "input" } else { message.kind.as_str() };
    match kind {
        "input" => {
            validate_input(&message.input_id, &message.text, message.eof)?;
            ensure!(!request.spec.terminal || !message.eof, "terminal EOF uses signal");
        }
        "resize" => {
            ensure!(terminal.is_some(), "resize requires terminal");
            validate_size(message.rows, message.cols)?;
        }
        "signal" => ensure!(
            terminal.is_some() && matches!(message.signal.as_str(), "interrupt" | "eof"),
            "unsupported terminal signal"
        ),
        _ => bail!("unsupported terminal control"),
    }
    let hash = message_hash(message)?;
    if let Some(previous) = journal.entries.iter().find(|entry| entry.input_id == message.input_id) {
        ensure!(previous.input_hash == hash, "input ID conflict");
        return Ok(previous.clone());
    }
    ensure!(
        !stop.is_cancelled()
            && tokio::time::Instant::now() < deadline
            && now() < request.lease_expires_at
            && now() < message.host_lease_expires_at
            && !dir.join("cancel.json").exists(),
        "job input inactive"
    );
    ensure!(!journal.closed && stdin.is_some(), "job stdin already closed");
    ensure!(
        journal.entries.len() < if request.spec.terminal { TERMINAL_INPUT_COUNT } else { INPUT_COUNT_LIMIT }
            && journal.reserved_bytes + message.text.len()
                <= if request.spec.terminal { TERMINAL_INPUT_BYTES } else { INPUT_TOTAL_LIMIT },
        "job input quota exhausted"
    );
    let mut receipt = InputReceipt {
        input_id: message.input_id.clone(),
        input_hash: hash,
        state: "pending".into(),
        bytes: None,
        eof: message.eof,
        effects_unknown: true,
        reason: "durable input dispatch intent; pending outcome must not be replayed".into(),
        kind: kind.into(),
        rows: message.rows,
        cols: message.cols,
        signal: message.signal.clone(),
    };
    journal.reserved_bytes += message.text.len();
    // Reserve EOF before dispatch as a lost close acknowledgement cannot prove
    // that stdin remains open. Unknown non-EOF writes likewise close admission.
    journal.closed = true;
    journal.entries.push(receipt.clone());
    ensure!(serde_json::to_vec(&journal)?.len() as u64 <= INPUT_JOURNAL_LIMIT, "input journal capacity exhausted");
    if let Err(error) = write_inputs(dir, journal) {
        journal.closed = true;
        drop(stdin.take());
        return Err(error);
    }
    // Once committed, keep this operation bounded but do not cancel it solely
    // because child.wait completed: a successful input can itself cause exit.
    let write = async {
        if kind == "resize" {
            terminal
                .context("terminal missing")?
                .process
                .lock()
                .map_err(|_| anyhow!("terminal lock poisoned"))?
                .resize(message.rows, message.cols)?;
            terminal
                .context("terminal missing")?
                .emulator
                .lock()
                .map_err(|_| anyhow!("terminal parser lock poisoned"))?
                .resize(message.rows, message.cols);
            return Ok::<_, anyhow::Error>(());
        }
        let writer = stdin.as_mut().context("interactive stdin missing")?;
        let data: &[u8] = if kind == "signal" {
            if message.signal == "interrupt" {
                b"\x03"
            } else if cfg!(windows) {
                b"\x1a\r"
            } else {
                b"\x04"
            }
        } else {
            message.text.as_bytes()
        };
        writer.write_all(data).await?;
        writer.flush().await?;
        if message.eof {
            drop(stdin.take());
        }
        Ok::<_, anyhow::Error>(())
    };
    match tokio::time::timeout(Duration::from_secs(2), write).await {
        Ok(Ok(())) => {
            journal.closed = message.eof;
            receipt.state = "written".into();
            receipt.bytes = Some(message.text.len());
            receipt.effects_unknown = false;
            receipt.reason = "terminal/pipe operation acknowledged; child consumption/effects are not implied".into();
        }
        _ => {
            drop(stdin.take());
            journal.closed = true;
            receipt.state = "unknown".into();
            receipt.reason =
                "stdin write failed or timed out; partial effects unknown; stdin closed; do not replay".into();
        }
    }
    *journal.entries.last_mut().context("input intent missing")? = receipt.clone();
    // If persistence fails, fail closed: the durable pending receipt explicitly
    // forbids replay and the in-memory stdin is no longer admitted for writes.
    if let Err(error) = write_inputs(dir, journal) {
        journal.closed = true;
        drop(stdin.take());
        return Err(error);
    }
    Ok(receipt)
}

async fn pump(
    mut input: impl AsyncRead + Unpin,
    path: PathBuf,
    total: Arc<AtomicU64>,
    maximum: u64,
    stopped: CancellationToken,
    terminal: Option<TerminalOutput>,
) -> Result<()> {
    let mut fault_guard = terminal.as_ref().map(|t| TaskFaultGuard { fault: t.fault.clone(), normal: false });
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
            if let Some(t) = terminal.as_ref().filter(|t| !t.fault.is_cancelled() && !stopped.is_cancelled()) {
                let replies =
                    t.emulator.lock().map_err(|_| anyhow!("terminal parser lock poisoned"))?.process(&bytes[..length]);
                match replies {
                    Ok(replies) => {
                        for reply in replies {
                            if t.responses.try_send(reply).is_err() {
                                t.fault.cancel();
                                break;
                            }
                        }
                    }
                    Err(_) => t.fault.cancel(),
                }
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
    if let Some(guard) = fault_guard.as_mut() {
        guard.normal = true;
        if result.is_err() {
            guard.fault.cancel();
        }
    } else if result.is_err() {
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
    if request.spec.terminal {
        ensure!(request.spec.interactive, "terminal must be interactive");
        validate_size(request.spec.rows, request.spec.cols)?;
    } else {
        ensure!(request.spec.rows == 0 && request.spec.cols == 0, "non-terminal size is invalid");
    }
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
                worker_executable: Some(std::env::current_exe().unwrap()),
                owner: Owner::new(&temp.0, "device-test", session).unwrap(),
                goal_id: "test-goal".into(),
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
            goal_id: manager.policy.goal_id.clone(),
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
            goal_id: manager.policy.goal_id.clone(),
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
        assert_eq!(readonly.len(), 3);
        assert!(readonly
            .iter()
            .all(|tool| matches!(tool.schema().name.as_str(), "job_status" | "job_read_log" | "job_input_status")));
    }

    fn interactive_fixture(manager: &Manager, script: &str) -> (PathBuf, Request) {
        let spec = manager
            .parse_spec(&json!({"op_id":"interactive", "language":"python", "script":script, "interactive":true}))
            .unwrap();
        let id = digest(
            format!("{}:{}", digest(&serde_json::to_vec(&manager.policy.owner).unwrap()), spec.op_id).as_bytes(),
        );
        let dir = manager.directory.join(&id);
        std::fs::create_dir(&dir).unwrap();
        let request = Request {
            version: VERSION,
            job_id: id,
            owner: manager.policy.owner.clone(),
            goal_id: manager.policy.goal_id.clone(),
            input_hash: digest(&serde_json::to_vec(&spec).unwrap()),
            worker_nonce: nonce().unwrap(),
            created_at: now(),
            lease_expires_at: now() + 30,
            max_log_bytes: 4096,
            spec,
        };
        write_json(&dir.join("request.json"), &request).unwrap();
        let mut receipt = Receipt::initial(&request);
        receipt.state = "running".into();
        receipt.worker_pid = Some(std::process::id());
        write_json(&dir.join("receipt.json"), &receipt).unwrap();
        (dir, request)
    }
    async fn endpoint_ready(dir: &Path) {
        for _ in 0..100 {
            if dir.join("input-endpoint.json").exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("interactive endpoint did not become ready");
    }
    #[test]
    fn old_serialized_specs_keep_exact_hash_and_default_input_closed() {
        let old = br#"{"op_id":"a","language":"python","script":"print(1)","cwd":"/tmp","timeout_seconds":30}"#;
        let spec: Spec = serde_json::from_slice(old).unwrap();
        assert!(!spec.interactive);
        assert_eq!(serde_json::to_vec(&spec).unwrap(), old);
        let mut explicit = serde_json::to_value(&spec).unwrap();
        explicit["interactive"] = json!(false);
        let explicit: Spec = serde_json::from_value(explicit).unwrap();
        assert_eq!(digest(&serde_json::to_vec(&explicit).unwrap()), digest(old));
        assert!(validate_input("one", "", false).is_err());
        assert!(validate_input("one", "", true).is_ok());
        assert!(validate_input("one", &"中".repeat(3000), false).is_err());
        assert!(validate_input_id("../one").is_err());
    }

    #[tokio::test]
    async fn real_python_input_is_stable_private_owner_bound_and_eof_closes() {
        let temp = Temp::new();
        let manager = manager(&temp, "interactive");
        let script = "import sys\na=input('first>')\nprint('first:'+a,flush=True)\nb=input('second>')\nprint('second:'+b,flush=True)\nc=sys.stdin.read()\nprint('tail:'+c,flush=True)\n";
        let (dir, request) = interactive_fixture(&manager, script);
        let worker_lock = lock_file(&dir.join("worker.lock")).unwrap();
        worker_lock.lock_exclusive().unwrap();
        let worker_dir = dir.clone();
        let worker_request = request.clone();
        let task = tokio::spawn(async move {
            let mut receipt = Receipt::initial(&worker_request);
            execute_worker(&worker_dir, &worker_request, &mut receipt).await.unwrap();
            write_json(&worker_dir.join("receipt.json"), &receipt).unwrap();
            receipt
        });
        endpoint_ready(&dir).await;
        let id = &request.job_id;
        let mut wrong_goal = manager.clone();
        wrong_goal.policy.goal_id = "another-goal".into();
        assert!(wrong_goal.write_input(id, "denied", "bad\n", false).await.is_err());
        let mut expired = manager.clone();
        expired.policy.lease_expires_at = now() - 1;
        assert!(expired.write_input(id, "denied", "bad\n", false).await.is_err());
        assert!(super::tests::manager(&temp, "other-owner").write_input(id, "denied", "bad\n", false).await.is_err());
        let first = manager.write_input(id, "first", "private-value-one\n", false).await.unwrap();
        assert_eq!(first["state"], "written");
        let repeat = manager.write_input(id, "first", "private-value-one\n", false).await.unwrap();
        assert_eq!(first, repeat);
        assert!(manager.write_input(id, "first", "changed\n", false).await.is_err());
        assert_eq!(manager.write_input(id, "second", "中文🙂\n", false).await.unwrap()["state"], "written");
        assert_eq!(manager.write_input(id, "close", "tail-data", true).await.unwrap()["state"], "written");
        let terminal = tokio::time::timeout(Duration::from_secs(8), task).await.unwrap().unwrap();
        assert_eq!(terminal.state, "succeeded");
        assert!(manager.status(id).unwrap().stdin_closed);
        assert!(manager.write_input(id, "after-close", "bad\n", false).await.is_err());
        assert_eq!(manager.write_input(id, "close", "tail-data", true).await.unwrap()["state"], "written");
        let output = std::fs::read_to_string(dir.join("stdout.bin")).unwrap();
        assert!(
            output.contains("first:private-value-one")
                && output.contains("second:中文🙂")
                && output.contains("tail:tail-data"),
            "{output}"
        );
        let journal = std::fs::read_to_string(dir.join("inputs.json")).unwrap();
        assert!(!journal.contains("private-value-one") && !journal.contains("tail-data"));
        let status = manager.input_status(id, "first").unwrap().to_string();
        assert!(!status.contains("private-value-one") && !status.contains("input_hash"));
        drop(worker_lock);
    }

    #[tokio::test]
    async fn immediate_exit_keeps_input_ack_and_slow_client_does_not_block_cancel() {
        for immediate_exit in [true, false] {
            let temp = Temp::new();
            let manager = manager(&temp, "exit-cancel");
            let (dir, request) = interactive_fixture(
                &manager,
                if immediate_exit {
                    "print('ready',flush=True)\nx=input()\nprint('done:'+x,flush=True)\n"
                } else {
                    "import time\nprint('ready',flush=True)\ntime.sleep(20)\n"
                },
            );
            let worker_dir = dir.clone();
            let worker_request = request.clone();
            let task = tokio::spawn(async move {
                let mut receipt = Receipt::initial(&worker_request);
                execute_worker(&worker_dir, &worker_request, &mut receipt).await.unwrap();
                receipt
            });
            endpoint_ready(&dir).await;
            if immediate_exit {
                let reply = manager.write_input(&request.job_id, "exit-now", "exit-now\n", false).await.unwrap();
                assert_eq!(reply["state"], "written");
            } else {
                let endpoint: InputEndpoint = read_json(&dir.join("input-endpoint.json")).unwrap();
                let _slow = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, endpoint.port)).await.unwrap();
                write_json(&dir.join("cancel.json"), &json!({"requested_at":now()})).unwrap();
                assert_eq!(
                    tokio::time::timeout(Duration::from_secs(3), task).await.unwrap().unwrap().state,
                    "cancelled"
                );
                continue;
            }
            assert_eq!(tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap().state, "succeeded");
            assert_eq!(manager.input_status(&request.job_id, "exit-now").unwrap()["state"], "written");
        }
    }

    #[tokio::test]
    async fn pending_input_is_never_replayed_and_original_job_lease_cannot_be_extended() {
        let temp = Temp::new();
        let manager = manager(&temp, "pending");
        let (dir, mut request) = interactive_fixture(&manager, "print(input())");
        let pending = InputReceipt {
            input_id: "ambiguous".into(),
            input_hash: input_hash(&request.worker_nonce, "ambiguous", "secret\n", false).unwrap(),
            state: "pending".into(),
            bytes: None,
            eof: false,
            effects_unknown: true,
            reason: "dispatch intent".into(),
            ..Default::default()
        };
        write_json(&dir.join("inputs.json"), &InputJournal { entries: vec![pending], reserved_bytes: 7, closed: true })
            .unwrap();
        // No endpoint or live worker exists. The pending result still returns
        // locally, and an identical request never makes a new dispatch attempt.
        let receipt = manager.write_input(&request.job_id, "ambiguous", "secret\n", false).await.unwrap();
        assert_eq!(receipt["state"], "pending");
        assert_eq!(receipt["effects_unknown"], true);
        assert!(manager.write_input(&request.job_id, "ambiguous", "different\n", false).await.is_err());
        assert_eq!(manager.input_status(&request.job_id, "not-seen").unwrap()["state"], "not_observed");
        request.lease_expires_at = now() - 1;
        write_json(&dir.join("request.json"), &request).unwrap();
        assert!(manager.policy.lease_expires_at > now());
        assert!(manager
            .write_input(&request.job_id, "ambiguous", "secret\n", false)
            .await
            .unwrap_err()
            .to_string()
            .contains("lease expired"));
    }

    #[tokio::test]
    async fn per_job_input_limits_and_authentication_are_enforced_by_worker() {
        let temp = Temp::new();
        let manager = manager(&temp, "quota");
        let (dir, request) = interactive_fixture(&manager, "import sys\nsys.stdin.read()\n");
        let worker_dir = dir.clone();
        let worker_request = request.clone();
        let task = tokio::spawn(async move {
            let mut receipt = Receipt::initial(&worker_request);
            execute_worker(&worker_dir, &worker_request, &mut receipt).await.unwrap();
            receipt
        });
        endpoint_ready(&dir).await;
        let endpoint: InputEndpoint = read_json(&dir.join("input-endpoint.json")).unwrap();
        let mut unauthorized = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, endpoint.port)).await.unwrap();
        write_frame(
            &mut unauthorized,
            &InputMessage {
                worker_nonce: "wrong-worker".into(),
                host_lease_expires_at: request.lease_expires_at,
                input_id: "forged".into(),
                text: "secret\n".into(),
                eof: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(1), read_frame::<InputReply>(&mut unauthorized))
            .await
            .unwrap()
            .is_err());
        assert_eq!(manager.input_status(&request.job_id, "forged").unwrap()["state"], "not_observed");
        // This consumes all 64 KiB but fewer than 64 dispatch slots. A new byte
        // must be rejected even though the interpreter keeps draining stdin.
        for i in 0..8 {
            assert_eq!(
                manager.write_input(&request.job_id, &format!("block-{i}"), &"x".repeat(8192), false).await.unwrap()
                    ["state"],
                "written"
            );
        }
        assert!(manager.write_input(&request.job_id, "overflow", "x", false).await.is_err());
        // Zero-byte EOF still works with the byte budget exhausted.
        assert_eq!(manager.write_input(&request.job_id, "end", "", true).await.unwrap()["state"], "written");
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap().state, "succeeded");
        let resume = resume_tools(Arc::new(manager));
        assert_eq!(resume.len(), 7);
        assert!(!resume.iter().any(|tool| tool.schema().name == "job_start"));
        assert!(resume.iter().any(|tool| tool.schema().name == "job_write_input"));
        assert!(resume.iter().any(|tool| tool.schema().name == "job_terminal_resize"));
        assert!(resume.iter().any(|tool| tool.schema().name == "job_terminal_signal"));
    }

    struct PartialFailure(Arc<AtomicU64>);
    impl tokio::io::AsyncWrite for PartialFailure {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            if self.0.load(Ordering::SeqCst) == 0 {
                let used = bytes.len().min(2);
                self.0.store(used as u64, Ordering::SeqCst);
                std::task::Poll::Ready(Ok(used))
            } else {
                std::task::Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "controlled partial failure",
                )))
            }
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }
    #[tokio::test]
    async fn partial_pipe_fault_is_durable_unknown_and_blocks_replay_and_new_inputs() {
        let temp = Temp::new();
        let manager = manager(&temp, "partial-fault");
        let (dir, request) = interactive_fixture(&manager, "unused");
        let observed = Arc::new(AtomicU64::new(0));
        let mut stdin = Some(PartialFailure(observed.clone()));
        let mut journal = InputJournal::default();
        let stop = CancellationToken::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let message = InputMessage {
            worker_nonce: request.worker_nonce.clone(),
            host_lease_expires_at: request.lease_expires_at,
            input_id: "partial".into(),
            text: "private-code\n".into(),
            eof: false,
            ..Default::default()
        };
        let receipt =
            dispatch_input(&dir, &request, deadline, &stop, &mut stdin, &mut journal, &message).await.unwrap();
        assert_eq!(observed.load(Ordering::SeqCst), 2);
        assert_eq!(receipt.state, "unknown");
        assert!(receipt.effects_unknown && receipt.bytes.is_none() && stdin.is_none() && journal.closed);
        let duplicate =
            dispatch_input(&dir, &request, deadline, &stop, &mut stdin, &mut journal, &message).await.unwrap();
        assert_eq!(duplicate.state, "unknown");
        assert_eq!(observed.load(Ordering::SeqCst), 2);
        let next = InputMessage { input_id: "new-input".into(), ..message };
        assert!(dispatch_input(&dir, &request, deadline, &stop, &mut stdin, &mut journal, &next).await.is_err());
        assert_eq!(manager.input_status(&request.job_id, "partial").unwrap()["effects_unknown"], true);
        assert!(!std::fs::read_to_string(dir.join("inputs.json")).unwrap().contains("private-code"));
    }
    #[tokio::test]
    async fn terminal_input_journal_crosses_metadata_limit_and_retains_admission_bounds() {
        let temp = Temp::new();
        let manager = manager(&temp, "terminal-ledger");
        let (dir, mut request) = interactive_fixture(&manager, "unused");
        request.spec.terminal = true;
        request.spec.rows = 24;
        request.spec.cols = 80;
        let mut journal = InputJournal::default();
        // A restored long terminal history must keep accepting raw input after
        // it crosses the ordinary 256 KiB metadata ceiling.
        for i in 0..1000 {
            journal.entries.push(InputReceipt {
                input_id: format!("history-{i}"),
                input_hash: "a".repeat(64),
                state: "written".into(),
                kind: "input".into(),
                bytes: Some(1),
                reason: "terminal/pipe operation acknowledged; child consumption/effects are not implied".into(),
                ..Default::default()
            });
        }
        journal.reserved_bytes = 1000;
        assert!(serde_json::to_vec(&journal).unwrap().len() as u64 > META_LIMIT);
        write_inputs(&dir, &journal).unwrap();
        let mut journal = load_inputs(&dir).unwrap();
        let mut writer = Some(tokio::io::sink());
        let stop = CancellationToken::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        for i in 0..4 {
            let message = InputMessage {
                worker_nonce: request.worker_nonce.clone(),
                host_lease_expires_at: request.lease_expires_at,
                input_id: format!("fresh-{i}"),
                text: "x".into(),
                ..Default::default()
            };
            let ack =
                dispatch_input(&dir, &request, deadline, &stop, &mut writer, &mut journal, &message).await.unwrap();
            assert_eq!(ack.state, "written");
            assert!(!load_inputs(&dir).unwrap().closed);
        }
        assert_eq!(load_inputs(&dir).unwrap().entries.len(), 1004);
        journal.entries.resize(TERMINAL_INPUT_COUNT, journal.entries[0].clone());
        let overflow = InputMessage {
            worker_nonce: request.worker_nonce.clone(),
            host_lease_expires_at: request.lease_expires_at,
            input_id: "over-quota".into(),
            text: "x".into(),
            ..Default::default()
        };
        assert!(dispatch_input(&dir, &request, deadline, &stop, &mut writer, &mut journal, &overflow).await.is_err());
    }
    #[tokio::test]
    async fn terminal_input_service_or_log_fault_interrupts_worker_without_waiting_for_deadline() {
        for broken_input in [true, false] {
            let temp = Temp::new();
            let manager = manager(&temp, if broken_input { "input-fault" } else { "output-fault" });
            let (dir, mut request) = interactive_fixture(&manager, "import time\ntime.sleep(30)\n");
            request.spec.terminal = true;
            request.spec.rows = 24;
            request.spec.cols = 80;
            request.input_hash = digest(&serde_json::to_vec(&request.spec).unwrap());
            write_json(&dir.join("request.json"), &request).unwrap();
            write_json(&dir.join("receipt.json"), &Receipt::initial(&request)).unwrap();
            if broken_input {
                write_bytes(&dir.join("inputs.json"), b"{invalid").unwrap();
            } else {
                write_bytes(&dir.join("stdout.bin"), b"existing log must not be overwritten").unwrap();
            }
            tokio::time::timeout(Duration::from_secs(8), worker(&dir)).await.unwrap().unwrap();
            let receipt: Receipt = read_json(&dir.join("receipt.json")).unwrap();
            assert_eq!(receipt.state, "interrupted");
            assert!(receipt.effects_unknown);
        }
    }
}
