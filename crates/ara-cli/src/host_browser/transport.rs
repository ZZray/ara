//! Persistent private Bun transport. The original JS broker owns shared Chrome.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{mpsc, oneshot},
};
use tokio_util::sync::CancellationToken;

use super::{HostBrowserOptions, HostCloseReceipt};

type PendingReply = oneshot::Sender<std::result::Result<Value, String>>;
#[derive(Default)]
struct Replies {
    pending: Mutex<HashMap<String, PendingReply>>,
    failure: Mutex<Option<String>>,
    stderr: Mutex<Vec<u8>>,
}
impl Replies {
    fn fail(&self, reason: String) {
        self.failure.lock().unwrap().get_or_insert(reason.clone());
        for (_, reply) in self.pending.lock().unwrap().drain() {
            let _ = reply.send(Err(reason.clone()));
        }
    }
    fn current_failure(&self) -> Option<String> {
        self.failure.lock().unwrap().clone()
    }
}

pub(super) struct Transport {
    sender: mpsc::Sender<Vec<u8>>,
    replies: Arc<Replies>,
    child: tokio::sync::Mutex<Child>,
    close: tokio::sync::Mutex<Option<std::result::Result<HostCloseReceipt, String>>>,
    admission_closed: Mutex<bool>,
    pub project_dir: PathBuf,
    pub owner_pid: u32,
    pub wrapper_pid: u32,
}

impl Transport {
    pub async fn start(options: &HostBrowserOptions, project: &Path) -> Result<Arc<Self>> {
        let project_dir = std::fs::canonicalize(project).context("PDF Host project directory")?;
        let mut command = Command::new(&options.bun);
        command
            .arg("--no-env-file")
            .arg(&options.entry)
            .arg("--host")
            .current_dir(&project_dir)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Never reuse LSP's per-client kill-on-drop Windows Job/process group:
        // shared broker/Chrome descendants must survive another client closing.
        command.kill_on_drop(false);
        for key in [
            "PATH",
            "Path",
            "SystemRoot",
            "WINDIR",
            "COMSPEC",
            "PATHEXT",
            "HOME",
            "USERPROFILE",
            "LOCALAPPDATA",
            "APPDATA",
            "OMP_NO_WEBP",
            "PUPPETEER_EXECUTABLE_PATH",
            "PI_NATIVE_VARIANT",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let owner_pid = std::process::id();
        command
            .env("ARA_PDF_HOST_ROOT", &options.runtime_root)
            .env("ARA_PDF_PROJECT_DIR", &project_dir)
            .env("ARA_PDF_HOST_PID", owner_pid.to_string())
            .env("TEMP", &options.temp_root)
            .env("TMP", &options.temp_root)
            .env("TMPDIR", &options.temp_root);
        let mut child = command
            .spawn()
            .context("start pinned private PDF/image Host")?;
        let wrapper_pid = child.id().context("PDF Host child PID missing")?;
        let mut input = child.stdin.take().context("PDF Host stdin missing")?;
        let output = child.stdout.take().context("PDF Host stdout missing")?;
        let mut error_output = child.stderr.take().context("PDF Host stderr missing")?;
        let replies = Arc::new(Replies::default());
        let (sender, mut outgoing) = mpsc::channel::<Vec<u8>>(16);
        let (ready_tx, ready_rx) = oneshot::channel();
        let reader_replies = replies.clone();
        let expected_project = project_dir.clone();
        let expected_root = options.runtime_root.clone();
        tokio::spawn(async move {
            let mut ready = Some(ready_tx);
            let mut lines = BufReader::new(output).lines();
            let error = loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let value: Value = match serde_json::from_str(&line) {
                            Ok(value) => value,
                            Err(error) => break format!("PDF Host emitted invalid JSON: {error}"),
                        };
                        if value["type"] == "ready" {
                            if let Some(reply) = ready.take() {
                                let matches = value["ownerPid"].as_u64()
                                    == Some(u64::from(owner_pid))
                                    && value["browserStarted"] == false
                                    && value["projectDir"].as_str().is_some_and(|path| {
                                        same_path(Path::new(path), &expected_project)
                                    })
                                    && value["root"].as_str().is_some_and(|path| {
                                        same_path(Path::new(path), &expected_root)
                                    });
                                let _ = reply.send(if matches {
                                    Ok(())
                                } else {
                                    Err("PDF Host startup identity/root receipt mismatch"
                                        .to_owned())
                                });
                                if !matches {
                                    break "PDF Host startup receipt mismatch".to_owned();
                                }
                                continue;
                            }
                            break "PDF Host emitted duplicate ready receipt".to_owned();
                        }
                        if let Some(id) = value["requestId"].as_str() {
                            let reply = reader_replies.pending.lock().unwrap().remove(id);
                            if let Some(reply) = reply {
                                let _ = reply.send(Ok(value));
                            } else {
                                break "PDF Host emitted an unowned request receipt".to_owned();
                            }
                        } else if value["type"] != "wrapper_cleanup" {
                            break "PDF Host emitted an uncorrelated failure receipt".to_owned();
                        }
                    }
                    Ok(None) => {
                        break "PDF Host disconnected; request effects may be unknown".to_owned();
                    }
                    Err(error) => {
                        break format!(
                            "PDF Host stdout failed; request effects may be unknown: {error}"
                        );
                    }
                }
            };
            if let Some(ready) = ready {
                let _ = ready.send(Err(error.clone()));
            }
            reader_replies.fail(error);
        });
        let writer_replies = replies.clone();
        tokio::spawn(async move {
            while let Some(frame) = outgoing.recv().await {
                if let Err(error) = input.write_all(&frame).await {
                    writer_replies.fail(format!(
                        "PDF Host stdin failed; request effects may be unknown: {error}"
                    ));
                    break;
                }
            }
            let _ = input.shutdown().await;
        });
        let stderr_replies = replies.clone();
        tokio::spawn(async move {
            let mut buffer = [0_u8; 4096];
            while let Ok(count) = error_output.read(&mut buffer).await {
                if count == 0 {
                    break;
                }
                let mut tail = stderr_replies.stderr.lock().unwrap();
                tail.extend_from_slice(&buffer[..count]);
                let excess = tail.len().saturating_sub(64 * 1024);
                if excess > 0 {
                    tail.drain(..excess);
                }
            }
        });
        let transport = Arc::new(Self {
            sender,
            replies,
            child: tokio::sync::Mutex::new(child),
            close: Default::default(),
            admission_closed: Mutex::new(false),
            project_dir,
            owner_pid,
            wrapper_pid,
        });
        match tokio::time::timeout(std::time::Duration::from_secs(30), ready_rx).await {
            Ok(Ok(Ok(()))) => Ok(transport),
            result => {
                transport.replies.fail(format!(
                    "PDF Host startup lacks accepted presence receipt: {result:?}"
                ));
                // Close only wrapper stdin. Do not terminate any shared tree or
                // remove a live Rust parent presence after uncertain startup.
                bail!(
                    "PDF Host startup failed; wrapper PID {wrapper_pid}, effects unknown: {result:?}"
                )
            }
        }
    }

    pub fn is_closed(&self) -> bool {
        self.replies.current_failure().is_some()
    }

    async fn enqueue(
        &self,
        mut request: Value,
        cancel: &CancellationToken,
        deadline: tokio::time::Instant,
        host_close: bool,
    ) -> Result<(
        String,
        oneshot::Receiver<std::result::Result<Value, String>>,
    )> {
        if let Some(error) = self.replies.current_failure() {
            bail!(error);
        }
        let id = uuid::Uuid::now_v7().to_string();
        request["requestId"] = json!(id);
        let mut frame = serde_json::to_vec(&request)?;
        frame.push(b'\n');
        // Reserve before registering: cancellation/backpressure/deadline cannot
        // leave an unsent request in the pending table. Sending after reservation
        // and registration is synchronous, so receipt ownership precedes writes.
        let permit = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("PDF Host request was aborted before admission"),
            _ = tokio::time::sleep_until(deadline) => bail!("PDF Host request deadline elapsed before admission"),
            result = self.sender.reserve() => result.context("PDF Host writer closed before admission")?,
        };
        if cancel.is_cancelled() {
            bail!("PDF Host request was aborted before admission");
        }
        let (reply, receiver) = oneshot::channel();
        {
            // This same gate closes ordinary admission before host_close. No
            // await occurs while held: any accepted release/request is sent
            // before close can close the gate; a late lease release is rejected.
            let admission_closed = self.admission_closed.lock().unwrap();
            if *admission_closed && !host_close {
                bail!("PDF Host admission is closed; explicit host_close owns remaining resources");
            }
            let mut pending = self.replies.pending.lock().unwrap();
            // Reader failure either precedes this check or drains our inserted
            // ownership after the lock is released; it cannot outrun insertion.
            if let Some(error) = self.replies.current_failure() {
                bail!(error);
            }
            pending.insert(id.clone(), reply);
            permit.send(frame);
        }
        Ok((id, receiver))
    }

    pub async fn request(
        self: &Arc<Self>,
        request: Value,
        cancel: CancellationToken,
    ) -> Result<Value> {
        if cancel.is_cancelled() {
            bail!("PDF/image request was aborted before admission");
        }
        let end = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        let (id, mut receiver) = self.enqueue(request, &cancel, end, false).await?;
        let deadline = tokio::time::sleep_until(end);
        tokio::pin!(deadline);
        let mut cancelled = false;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled(), if !cancelled => {
                    cancelled = true;
                    // Correlate the cancellation ACK independently; the original
                    // terminal receipt remains owned and is never replayed.
                    let transport = self.clone();
                    let cancel_id = id.clone();
                    tokio::spawn(async move {
                        if let Ok((_, ack)) = transport.enqueue(json!({"action":"cancel", "cancelRequestId":cancel_id}), &CancellationToken::new(), end, false).await { let _ = ack.await; }
                    });
                }
                result = &mut receiver => {
                    let value = result.context("PDF Host receipt owner closed; effects unknown")?.map_err(anyhow::Error::msg)?;
                    if cancelled || cancel.is_cancelled() { bail!("PDF/image request was aborted; terminal receipt drained"); }
                    if value["ok"] != true {
                        bail!("{}", value["error"]["message"].as_str().unwrap_or("PDF Host operation failed"));
                    }
                    return Ok(value);
                }
                _ = &mut deadline => {
                    // A timeout is unknown, never a successful tool result.
                    // Keep correlation so a late terminal receipt can drain.
                    tokio::spawn(async move { let _ = receiver.await; });
                    bail!("PDF Host request deadline elapsed; effects unknown (request {id})");
                }
            }
        }
    }

    pub async fn close(&self) -> Result<HostCloseReceipt> {
        let mut settled = self.close.lock().await;
        if let Some(result) = &*settled {
            return result.clone().map_err(anyhow::Error::msg);
        }
        *self.admission_closed.lock().unwrap() = true;
        let result = async {
            let end = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
            let (_, receiver) = self
                .enqueue(
                    json!({"action":"host_close"}),
                    &CancellationToken::new(),
                    end,
                    true,
                )
                .await?;
            let value = tokio::time::timeout_at(end, receiver)
                .await
                .context("PDF Host close deadline; cleanup effects unknown")?
                .context("PDF Host close receipt missing; cleanup effects unknown")?
                .map_err(anyhow::Error::msg)?;
            let cleanup = value["cleanup"].clone();
            if value["ok"] != true
                || cleanup["remainingTabs"] != 0
                || cleanup["remainingBrowsers"] != 0
                || cleanup["errors"]
                    .as_array()
                    .is_none_or(|errors| !errors.is_empty())
                || cleanup["presenceRemoved"] != true
            {
                bail!("PDF Host cleanup failed: {cleanup}");
            }
            let mut child = self.child.lock().await;
            let exit = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait())
                .await
                .context("PDF Host cleanup receipt received but wrapper exit unknown")??;
            if !exit.success() {
                bail!("PDF Host wrapper exited unsuccessfully after cleanup: {exit}");
            }
            Ok(HostCloseReceipt {
                owner_pid: self.owner_pid,
                wrapper_pid: self.wrapper_pid,
                project_dir: self.project_dir.clone(),
                cleanup,
            })
        }
        .await;
        *settled = Some(
            result
                .as_ref()
                .map(Clone::clone)
                .map_err(|error: &anyhow::Error| format!("{error:#}")),
        );
        result
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    std::fs::canonicalize(left)
        .ok()
        .zip(std::fs::canonicalize(right).ok())
        .is_some_and(|(left, right)| left == right)
}
