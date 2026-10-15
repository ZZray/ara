use crate::{
    local_tools,
    wire::{self, Frame},
};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{io::AsyncReadExt, sync::mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Sender {
    pub tx: mpsc::Sender<Packet>,
    pub epoch: u64,
    pub alive: CancellationToken,
}
pub struct Packet {
    pub epoch: u64,
    pub value: Value,
    pub settled_id: Option<String>,
}
impl Sender {
    pub async fn sig(&self, value: Value) -> Result<()> {
        self.packet(value, None).await
    }
    pub async fn packet(&self, value: Value, settled_id: Option<String>) -> Result<()> {
        tokio::select! {
            _=self.alive.cancelled()=>anyhow::bail!("controller connection closed"),
            result=tokio::time::timeout(Duration::from_secs(5),self.tx.send(Packet{epoch:self.epoch,value,settled_id}))=>{result.context("controller output stalled")?.context("connection closed")}
        }
    }
    pub async fn frame(&self, value: Value) -> Result<()> {
        self.sig(wire::dc(value)).await
    }
    pub async fn output(&self, id: &str, data: &[u8], code: i32, error: &str) -> Result<()> {
        for chunk in data.chunks(wire::CHUNK) {
            self.sig(wire::out(id, chunk, false, 0, "")).await?;
        }
        self.sig(wire::out(id, &[], true, code, error)).await
    }
    pub async fn terminal(&self, id: &str, data: &[u8], code: i32, error: &str) -> Result<()> {
        for chunk in data.chunks(wire::CHUNK) {
            self.sig(wire::out(id, chunk, false, 0, "")).await?;
        }
        self.packet(wire::out(id, &[], true, code, error), Some(id.into())).await
    }
    pub async fn event(&self, id: &str, event: Value) -> Result<()> {
        self.sig(wire::event(id, event)).await
    }
}

pub fn path(cwd: &Path, value: &str) -> PathBuf {
    let value = Path::new(value);
    if value.is_absolute() {
        value.to_path_buf()
    } else {
        cwd.join(value)
    }
}

pub async fn shell(frame: &Frame, cwd: &Path, cancel: CancellationToken, sender: &Sender) -> Result<()> {
    let working = if frame.cwd.is_empty() { cwd.to_path_buf() } else { path(cwd, &frame.cwd) };
    let secs = if frame.timeout == 0 { 600 } else { frame.timeout.min(3600) };
    let (output, code) = local_tools::run_shell(&frame.cmd, &working, cancel, Duration::from_secs(secs)).await?;
    sender.terminal(&frame.id, output.as_bytes(), code, if code == 0 { "" } else { "command failed" }).await
}

pub async fn task(frame: &Frame, cwd: &Path, cancel: CancellationToken, sender: &Sender) -> Result<()> {
    let task = frame.task.as_ref().context("task required")?;
    let steps = task["steps"].as_array().context("steps required")?;
    anyhow::ensure!(!steps.is_empty() && steps.len() <= 8, "task must contain 1-8 steps");
    for step in steps {
        anyhow::ensure!(
            matches!(step["kind"].as_str(), Some("exec" | "read_file" | "write_file")),
            "unsupported task step"
        );
    }
    let mut report = String::new();
    sender
        .event(&frame.id, json!({"type":"task_start","status":"running","title":task["objective"],"total":steps.len()}))
        .await?;
    for (index, step) in steps.iter().enumerate() {
        if cancel.is_cancelled() {
            bail!("task canceled; partial effects may remain");
        }
        sender.event(&frame.id,json!({"type":"step_start","status":"running","step_id":step["id"],"title":step["title"],"index":index+1,"total":steps.len()})).await?;
        let result: Result<String> = async {
            match step["kind"].as_str().unwrap_or_default() {
                "exec" => {
                    let working = path(cwd, step["cwd"].as_str().filter(|p| !p.is_empty()).unwrap_or("."));
                    let (out, code) = local_tools::run_shell(
                        step["cmd"].as_str().context("cmd required")?,
                        &working,
                        cancel.clone(),
                        Duration::from_secs(step["timeout"].as_u64().unwrap_or(600).clamp(1, 3600)),
                    )
                    .await?;
                    anyhow::ensure!(code == 0, "command failed with code {code}: {out}");
                    Ok(out)
                }
                "read_file" => {
                    let file = path(cwd, step["path"].as_str().context("path required")?);
                    anyhow::ensure!(
                        tokio::fs::metadata(&file).await?.len() <= 1024 * 1024,
                        "text file too large; use exec to select log lines"
                    );
                    Ok(tokio::fs::read_to_string(file).await?)
                }
                "write_file" => {
                    let file = path(cwd, step["path"].as_str().context("path required")?);
                    let text = step["content"].as_str().context("content required")?;
                    anyhow::ensure!(text.len() <= 1024 * 1024, "content too large");
                    if let Some(parent) = file.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::write(&file, text).await?;
                    Ok(format!("Wrote {} bytes", text.len()))
                }
                _ => bail!("unsupported step"),
            }
        }
        .await;
        match result {
            Ok(output) => {
                sender.event(&frame.id,json!({"type":"step_finish","status":"completed","step_id":step["id"],"title":step["title"],"output":local_tools::bounded(output.clone(),128*1024)})).await?;
                report.push_str(&format!(
                    "{}\n{}\n",
                    step["title"].as_str().unwrap_or("Step"),
                    local_tools::bounded(output, 128 * 1024)
                ));
            }
            Err(error) => {
                sender
                    .event(&frame.id, json!({"type":"task_finish","status":"failed","detail":error.to_string()}))
                    .await?;
                return Err(error);
            }
        }
    }
    sender.event(&frame.id, json!({"type":"task_finish","status":"completed"})).await?;
    sender.terminal(&frame.id, report.as_bytes(), 0, "").await
}

pub async fn get_file(
    frame: &Frame,
    cwd: &Path,
    cancel: CancellationToken,
    sender: &Sender,
    mut acks: mpsc::Receiver<()>,
) -> Result<()> {
    let mut file = tokio::fs::File::open(path(cwd, &frame.path)).await?;
    let mut buf = vec![0u8; wire::CHUNK];
    loop {
        let count =
            tokio::select! { _=cancel.cancelled()=>bail!("file download canceled"), count=file.read(&mut buf)=>count? };
        if count == 0 {
            break;
        }
        sender.sig(wire::out(&frame.id, &buf[..count], false, 0, "")).await?;
        tokio::select! {
            _=cancel.cancelled()=>bail!("file download canceled"),
            result=tokio::time::timeout(Duration::from_secs(15),acks.recv())=>{result?.context("file receiver disconnected")?;}
        }
    }
    sender.terminal(&frame.id, &[], 0, "").await
}

pub struct Upload {
    pub id: String,
    pub destination: PathBuf,
    pub expected: u64,
    pub bytes: Vec<u8>,
}
impl Upload {
    pub fn new(frame: &Frame, cwd: &Path) -> Result<Self> {
        anyhow::ensure!(
            !frame.path.is_empty() && frame.size <= 16 * 1024 * 1024,
            "file path required, upload limit 16 MiB"
        );
        Ok(Self { id: frame.id.clone(), destination: path(cwd, &frame.path), expected: frame.size, bytes: Vec::new() })
    }
    pub async fn chunk(&mut self, frame: &Frame) -> Result<bool> {
        let bytes = STANDARD.decode(&frame.b64)?;
        anyhow::ensure!(self.bytes.len() as u64 + bytes.len() as u64 <= self.expected, "upload exceeds declared size");
        self.bytes.extend(bytes);
        if !frame.done {
            return Ok(false);
        }
        anyhow::ensure!(self.bytes.len() as u64 == self.expected, "upload size mismatch");
        if let Some(parent) = self.destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        // Write only once a complete upload has arrived; a lost connection cannot truncate the destination.
        let temp = self.destination.with_extension(format!("{}.upload", wire::fresh_id()));
        tokio::fs::write(&temp, &self.bytes).await?;
        tokio::fs::rename(&temp, &self.destination).await?;
        Ok(true)
    }
}

/// Persistent operation receipt prevents re-executing a repeated cloud request after reconnect/restart.
pub struct Receipts {
    path: PathBuf,
    entries: BTreeMap<String, String>,
}
impl Receipts {
    pub fn load(path: PathBuf) -> Result<Self> {
        let entries = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("invalid operation receipts")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path, entries })
    }
    fn save(&self) -> Result<()> {
        use std::io::Write;
        let temp = self.path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(&serde_json::to_vec(&self.entries)?)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(temp, &self.path)?;
        Ok(())
    }
    pub fn begin(&mut self, id: &str) -> Result<()> {
        anyhow::ensure!(!id.is_empty() && id.len() <= 256, "run id required (max 256 bytes)");
        anyhow::ensure!(
            !self.entries.contains_key(id),
            "duplicate run refused; inspect its prior receipt, effects may be unknown"
        );
        anyhow::ensure!(
            self.entries.len() < 10000,
            "operation receipt limit reached; archive explicitly before continuing"
        );
        self.entries.insert(id.into(), "running; effects may be unknown".into());
        self.save()
    }
    pub fn finish(&mut self, id: &str) -> Result<()> {
        self.entries.insert(id.into(), "settled; see run output".into());
        self.save()
    }
}
