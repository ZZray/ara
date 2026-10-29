//! stdio transport: one child process, newline-delimited JSON-RPC.
//!
//! The child inherits the Host environment plus the configured `env`. On
//! Windows it is placed in a Job object with kill-on-close, so the whole
//! process tree ends with the session or with the Host process.
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::{classify, messages, reply_to_server_request, Incoming, FRAME_MAX};
use crate::text::Redactor;

/// Bytes of stderr kept for diagnostics.
pub(crate) const STDERR_TAIL: usize = 4096;
/// How long a closed-stdin server may take to exit before the tree is killed.
const EXIT_GRACE: Duration = Duration::from_secs(2);

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

pub(crate) struct StdioLink {
    outbound: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    pending: Pending,
    lost: CancellationToken,
    reason: Arc<Mutex<Option<String>>>,
    stderr: Arc<Mutex<VecDeque<u8>>>,
    stderr_done: CancellationToken,
    exited: CancellationToken,
    tree: ProcessTree,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    redactor: Redactor,
}

fn mark_lost(lost: &CancellationToken, reason: &Mutex<Option<String>>, pending: &Pending, why: &str) {
    reason.lock().unwrap().get_or_insert_with(|| why.to_string());
    pending.lock().unwrap().clear();
    lost.cancel();
}

impl StdioLink {
    pub(crate) fn spawn(
        command: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
        redactor: Redactor,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(command);
        cmd.args(args).envs(env).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.as_std_mut().process_group(0);
        }
        let mut child = cmd.spawn().map_err(|error| {
            format!("failed to start server process: {}", redactor.server_text(&error.to_string(), 300))
        })?;
        let tree = match ProcessTree::new(&child) {
            Ok(tree) => tree,
            Err(error) => {
                let _ = child.start_kill();
                return Err(format!("server process started but its process tree could not be owned: {error}"));
            }
        };
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr_pipe = child.stderr.take().expect("piped stderr");
        let (sender, mut receiver) = mpsc::channel::<Vec<u8>>(64);
        let pending: Pending = Arc::default();
        let lost = CancellationToken::new();
        let reason: Arc<Mutex<Option<String>>> = Arc::default();
        let stderr: Arc<Mutex<VecDeque<u8>>> = Arc::default();

        let writer = {
            let (lost, reason, pending) = (lost.clone(), reason.clone(), pending.clone());
            tokio::spawn(async move {
                let mut stdin = stdin;
                while let Some(frame) = receiver.recv().await {
                    let written = async {
                        stdin.write_all(&frame).await?;
                        stdin.flush().await
                    };
                    if written.await.is_err() {
                        mark_lost(&lost, &reason, &pending, "server stdin closed");
                        return;
                    }
                }
                // All senders dropped: stdin closes here (shutdown step 1).
            })
        };
        let reader = {
            let (lost, reason, pending) = (lost.clone(), reason.clone(), pending.clone());
            let replies = sender.downgrade();
            tokio::spawn(async move {
                let mut stdout = BufReader::new(stdout);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    let read = (&mut stdout).take(FRAME_MAX as u64 + 1).read_until(b'\n', &mut line).await;
                    let why = match read {
                        Ok(0) => "server closed stdout",
                        Ok(_) if line.len() > FRAME_MAX => "server frame exceeds size limit",
                        Ok(_) => {
                            let Ok(value) = serde_json::from_slice::<Value>(&line) else { continue };
                            for message in messages(value) {
                                match classify(message) {
                                    Incoming::Response(id, message) => {
                                        if let Some(waiter) = pending.lock().unwrap().remove(&id) {
                                            let _ = waiter.send(message);
                                        }
                                    }
                                    Incoming::Request(request) => {
                                        let mut frame = reply_to_server_request(&request).to_string().into_bytes();
                                        frame.push(b'\n');
                                        if let Some(replies) = replies.upgrade() {
                                            let _ = replies.send(frame).await;
                                        }
                                    }
                                    Incoming::Other => {}
                                }
                            }
                            continue;
                        }
                        Err(_) => "server stdout read failed",
                    };
                    mark_lost(&lost, &reason, &pending, why);
                    return;
                }
            })
        };
        // A descendant may keep stdout open after the server exits, so the
        // process exit itself also ends the connection once stdout had a
        // moment to deliver the final frames.
        let exited = CancellationToken::new();
        let waiter = {
            let (lost, reason, pending) = (lost.clone(), reason.clone(), pending.clone());
            let exited = exited.clone();
            let mut child = child;
            tokio::spawn(async move {
                let _ = child.wait().await;
                exited.cancel();
                let _ = tokio::time::timeout(Duration::from_millis(500), lost.cancelled()).await;
                mark_lost(&lost, &reason, &pending, "server process exited");
            })
        };
        let stderr_done = CancellationToken::new();
        let collector = {
            let stderr = stderr.clone();
            let done = stderr_done.clone().drop_guard();
            tokio::spawn(async move {
                let _done = done;
                let mut pipe = stderr_pipe;
                let mut chunk = [0u8; 4096];
                while let Ok(n) = pipe.read(&mut chunk).await {
                    if n == 0 {
                        break;
                    }
                    let mut tail = stderr.lock().unwrap();
                    tail.extend(&chunk[..n]);
                    let excess = tail.len().saturating_sub(STDERR_TAIL);
                    tail.drain(..excess);
                }
            })
        };
        Ok(Self {
            outbound: Mutex::new(Some(sender)),
            pending,
            lost,
            reason,
            stderr,
            stderr_done,
            exited,
            tree,
            tasks: Mutex::new(vec![writer, reader, collector, waiter]),
            redactor,
        })
    }

    pub(crate) fn lost(&self) -> &CancellationToken {
        &self.lost
    }

    pub(crate) fn lost_reason(&self) -> String {
        self.reason.lock().unwrap().clone().unwrap_or_else(|| "connection closed".into())
    }

    /// Bounded, redacted stderr tail. After the connection is lost the
    /// collector gets a short moment to drain what the process wrote.
    pub(crate) async fn stderr_tail(&self) -> String {
        if self.lost.is_cancelled() {
            let _ = tokio::time::timeout(Duration::from_millis(500), self.stderr_done.cancelled()).await;
        }
        let bytes: Vec<u8> = self.stderr.lock().unwrap().iter().copied().collect();
        self.redactor.server_text(&String::from_utf8_lossy(&bytes), STDERR_TAIL)
    }

    pub(crate) fn register(&self, id: u64) -> oneshot::Receiver<Value> {
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);
        receiver
    }

    pub(crate) fn unregister(&self, id: u64) {
        self.pending.lock().unwrap().remove(&id);
    }

    /// Queue one message. `Err` means it was not handed to the writer.
    pub(crate) async fn send(&self, message: &Value) -> Result<(), String> {
        if self.lost.is_cancelled() {
            return Err(self.lost_reason());
        }
        let mut frame = message.to_string().into_bytes();
        if frame.len() > FRAME_MAX {
            return Err("request frame exceeds size limit".into());
        }
        frame.push(b'\n');
        let sender = self.outbound.lock().unwrap().clone().ok_or("session closed")?;
        sender.send(frame).await.map_err(|_| self.lost_reason())
    }

    /// Close stdin, wait briefly, then end the whole process tree.
    pub(crate) async fn shutdown(&self) {
        self.outbound.lock().unwrap().take();
        let graceful = tokio::time::timeout(EXIT_GRACE, self.exited.cancelled()).await.is_ok();
        // Descendants are ended even after a graceful exit of the server.
        self.tree.terminate();
        if !graceful {
            let _ = tokio::time::timeout(EXIT_GRACE, self.exited.cancelled()).await;
        }
        mark_lost(&self.lost, &self.reason, &self.pending, "session closed");
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
    }
}

#[cfg(windows)]
struct ProcessTree(isize);

#[cfg(windows)]
impl ProcessTree {
    fn new(child: &Child) -> std::io::Result<Self> {
        use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
        let process = child.raw_handle().ok_or_else(|| std::io::Error::other("server exited before tree admission"))?;
        // SAFETY: the job handle is owned here until wrapped or closed; the
        // child handle stays owned by Tokio and is only borrowed.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            );
            if ok == 0 || AssignProcessToJobObject(job, process as _) == 0 {
                let error = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(error);
            }
            Ok(Self(job as isize))
        }
    }

    fn terminate(&self) {
        // SAFETY: the job handle is owned by self and still open.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0 as _, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        // SAFETY: closing the owned job handle kills the remaining tree.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0 as _);
        }
    }
}

#[cfg(unix)]
struct ProcessTree(Option<i32>);

#[cfg(unix)]
impl ProcessTree {
    fn new(child: &Child) -> std::io::Result<Self> {
        Ok(Self(child.id().map(|pid| pid as i32)))
    }

    fn terminate(&self) {
        if let Some(group) = self.0 {
            // SAFETY: signals the process group created for this server.
            unsafe {
                libc::kill(-group, libc::SIGKILL);
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
