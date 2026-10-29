use std::{
    io::{BufRead, Write},
    sync::{mpsc as std_mpsc, Arc},
    time::Duration,
};

use anyhow::{bail, ensure, Result};
use serde_json::Value;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::protocol::FRAME_BYTES;

pub fn read_frame(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            ensure!(bytes.is_empty(), "unterminated_frame");
            return Ok(None);
        }
        let count = available.iter().position(|b| *b == b'\n').map_or(available.len(), |n| n + 1);
        ensure!(bytes.len() + count <= FRAME_BYTES, "frame_limit");
        let complete = available[count - 1] == b'\n';
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if complete {
            std::str::from_utf8(&bytes)?;
            return Ok(Some(bytes));
        }
    }
}

struct WriteFrame {
    bytes: Vec<u8>,
    ack: Option<oneshot::Sender<bool>>,
}

#[derive(Clone)]
pub struct Output {
    control: std_mpsc::SyncSender<WriteFrame>,
    ui: std_mpsc::SyncSender<WriteFrame>,
    pub fault: CancellationToken,
    secret: Arc<String>,
}

impl Output {
    pub fn stdout(secret: String) -> Self {
        let (control, control_rx) = std_mpsc::sync_channel::<WriteFrame>(16);
        let (ui, ui_rx) = std_mpsc::sync_channel::<WriteFrame>(32);
        let fault = CancellationToken::new();
        let writer_fault = fault.clone();
        // This owner alone touches stdout. A blocked pipe never blocks Tokio;
        // write confirmations have a bounded wait. Never join this OS thread.
        std::thread::spawn(move || {
            let stdout = std::io::stdout();
            let mut writer = stdout.lock();
            loop {
                let frame = match control_rx.try_recv() {
                    Ok(frame) => frame,
                    Err(_) => match ui_rx.try_recv() {
                        Ok(frame) => frame,
                        Err(_) => match control_rx.recv_timeout(Duration::from_millis(10)) {
                            Ok(frame) => frame,
                            Err(std_mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(_) => break,
                        },
                    },
                };
                let ok = writer.write_all(&frame.bytes).and_then(|_| writer.flush()).is_ok();
                if let Some(ack) = frame.ack {
                    let _ = ack.send(ok);
                }
                if !ok {
                    writer_fault.cancel();
                    break;
                }
            }
        });
        Self { control, ui, fault, secret: Arc::new(secret) }
    }

    pub fn encode(&self, value: &Value) -> Result<Vec<u8>> {
        let bytes = self.frame_bytes(value)?;
        ensure!(bytes.len() <= FRAME_BYTES, "outgoing_frame_limit");
        Ok(bytes)
    }

    /// A reference is allowed for size only. Secret/serialization faults must
    /// retain their refusal semantics rather than becoming a reference fallback.
    pub fn needs_reference(&self, value: &Value) -> Result<bool> {
        Ok(self.frame_bytes(value)?.len() > FRAME_BYTES)
    }

    fn frame_bytes(&self, value: &Value) -> Result<Vec<u8>> {
        // Control fields bind exact arguments, paths and hashes. Replacing a
        // token collision would silently change that authority. Refuse instead.
        ensure!(!contains_token(value, &self.secret), "outgoing_secret_collision");
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub async fn control(&self, value: Value) -> Result<()> {
        let bytes = self.encode(&value)?;
        let (ack, receiver) = oneshot::channel();
        if self.control.try_send(WriteFrame { bytes, ack: Some(ack) }).is_err() {
            self.fault.cancel();
            bail!("control_queue_unavailable");
        }
        match tokio::time::timeout(Duration::from_secs(2), receiver).await {
            Ok(Ok(true)) => Ok(()),
            _ => {
                self.fault.cancel();
                bail!("stdout_delivery_unknown")
            }
        }
    }

    pub fn ui(&self, value: Value) {
        if let Ok(bytes) = self.encode(&value) {
            let _ = self.ui.try_send(WriteFrame { bytes, ack: None });
        }
    }
}

fn contains_token(value: &Value, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    match value {
        Value::String(text) => text.contains(token),
        Value::Array(items) => items.iter().any(|v| contains_token(v, token)),
        Value::Object(fields) => fields.iter().any(|(key, value)| key.contains(token) || contains_token(value, token)),
        _ => false,
    }
}
