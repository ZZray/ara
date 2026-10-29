// MIT License
//
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can B枚l眉k
// Copyright (c) 2026 Stencil Labs, Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Native fixture harness for the exact private Host codec RPC, never fake pixels.
use ara_agent::ToolError;
use ara_tools::read_image::{DecodedImageMetadata, EncodedImage, ImageCodecPort, ImageEncodeBatch, ImageEncoding};
use async_trait::async_trait;
use base64::Engine;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

struct Bridge {
    child: Child,
    stdin: ChildStdin,
    receiver: mpsc::Receiver<String>,
    sequence: u64,
    _root: tempfile::TempDir,
}
impl Bridge {
    fn request(&mut self, mut request: Value, cancel: &CancellationToken) -> Result<Value, ToolError> {
        self.sequence += 1;
        let id = format!("image-original-{}-{}", std::process::id(), self.sequence);
        request["requestId"] = json!(id);
        writeln!(self.stdin, "{request}")
            .and_then(|_| self.stdin.flush())
            .map_err(|error| ToolError(error.to_string()))?;
        let started = Instant::now();
        let mut cancel_sent = false;
        loop {
            if cancel.is_cancelled() && !cancel_sent {
                let message = json!({"requestId":format!("cancel-{id}"),"action":"cancel","cancelRequestId":id});
                writeln!(self.stdin, "{message}")
                    .and_then(|_| self.stdin.flush())
                    .map_err(|error| ToolError(error.to_string()))?;
                cancel_sent = true;
            }
            if started.elapsed() > Duration::from_secs(60) {
                return Err(ToolError("Native image codec original request exceeded 60s; no retry".into()));
            }
            let line = match self.receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => return Err(ToolError("Native image codec stdout closed".into())),
            };
            let receipt: Value =
                serde_json::from_str(&line).map_err(|error| ToolError(format!("Native codec receipt: {error}")))?;
            if receipt["requestId"].as_str() != Some(id.as_str()) {
                continue;
            }
            if cancel_sent {
                return Err(ToolError("Native image codec original request was aborted".into()));
            }
            if receipt["ok"] != true {
                return Err(ToolError(receipt["error"]["message"].as_str().unwrap_or("Native codec error").into()));
            }
            return Ok(receipt["result"].clone());
        }
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.request(json!({"action":"host_close"}), &CancellationToken::new());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Clone)]
pub struct NativeBunCodec {
    bridge: Arc<Mutex<Bridge>>,
}
impl NativeBunCodec {
    pub fn new() -> Self {
        let bun = std::env::var_os("ARA_TEST_BUN").expect("Root must bind the installed actual Bun executable");
        let entry = std::env::var_os("ARA_TEST_IMAGE_HOST_ENTRY").expect("Root must bind P's exact Host entry.ts");
        let project =
            std::env::var_os("ARA_TEST_IMAGE_PROJECT_DIR").expect("Root must bind the actual canonical project");
        let root = tempfile::Builder::new().prefix("ara-native-image-originals-").tempdir().unwrap();
        assert!(
            root.path().to_string_lossy().to_lowercase().starts_with("d:"),
            "Native fixture Host cache/temp must be on D"
        );
        let mut child = Command::new(bun)
            .args(["--no-env-file"])
            .arg(entry)
            .arg("--host")
            .env("ARA_PDF_HOST_ROOT", root.path())
            .env("ARA_PDF_PROJECT_DIR", project)
            .env("ARA_PDF_HOST_PID", std::process::id().to_string())
            .env("PI_NATIVE_VARIANT", "baseline")
            .env("TMP", root.path())
            .env("TEMP", root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let ready = receiver
            .recv_timeout(Duration::from_secs(30))
            .ok()
            .and_then(|line| serde_json::from_str::<Value>(&line).ok());
        if !ready.is_some_and(|ready| ready["type"] == "ready" && ready["browserStarted"] == false) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Native codec Host did not return its ready receipt before 30s");
        }
        Self { bridge: Arc::new(Mutex::new(Bridge { child, stdin, receiver, sequence: 0, _root: root })) }
    }
    async fn request(&self, request: Value, cancel: CancellationToken) -> Result<Value, ToolError> {
        let bridge = self.bridge.clone();
        tokio::task::spawn_blocking(move || bridge.lock().unwrap().request(request, &cancel))
            .await
            .map_err(|error| ToolError(error.to_string()))?
    }
}
#[async_trait]
impl ImageCodecPort for NativeBunCodec {
    async fn decoded_metadata(
        &self,
        bytes: &[u8],
        cancel: CancellationToken,
    ) -> Result<DecodedImageMetadata, ToolError> {
        let result=self.request(json!({"action":"decoded_metadata","inputBase64":base64::engine::general_purpose::STANDARD.encode(bytes)}),cancel).await?;
        Ok(DecodedImageMetadata {
            width: result["width"].as_f64().ok_or_else(|| ToolError("Missing decoded width".into()))?,
            height: result["height"].as_f64().ok_or_else(|| ToolError("Missing decoded height".into()))?,
            format: result["format"].as_str().map(str::to_owned),
        })
    }
    async fn decode_probe_1x1(&self, bytes: &[u8], cancel: CancellationToken) -> Result<(), ToolError> {
        self.request(
            json!({"action":"decode_probe_1x1","inputBase64":base64::engine::general_purpose::STANDARD.encode(bytes)}),
            cancel,
        )
        .await?;
        Ok(())
    }
    async fn encode_many(
        &self,
        bytes: &[u8],
        batch: ImageEncodeBatch,
        cancel: CancellationToken,
    ) -> Result<Vec<EncodedImage>, ToolError> {
        let operations:Vec<_>=batch.formats.iter().map(|format|json!({"width":batch.width,"height":batch.height,"quality":batch.quality,"mimeType":match format{ImageEncoding::Png=>"image/png",ImageEncoding::Jpeg=>"image/jpeg",ImageEncoding::WebP=>"image/webp"}})).collect();
        let result=self.request(json!({"action":"encode_many","inputBase64":base64::engine::general_purpose::STANDARD.encode(bytes),"operations":operations}),cancel).await?;
        result["encodedResults"]
            .as_array()
            .ok_or_else(|| ToolError("Missing encodedResults".into()))?
            .iter()
            .map(|entry| {
                let buffer = base64::engine::general_purpose::STANDARD
                    .decode(entry["outputBase64"].as_str().ok_or_else(|| ToolError("Missing codec output".into()))?)
                    .map_err(|error| ToolError(error.to_string()))?;
                if entry["bytes"].as_u64() != Some(buffer.len() as u64) {
                    return Err(ToolError("Host codec output length differs from actual bytes".into()));
                }
                Ok(EncodedImage {
                    buffer,
                    mime_type: entry["mimeType"].as_str().ok_or_else(|| ToolError("Missing codec MIME".into()))?.into(),
                })
            })
            .collect()
    }
}

pub fn fixture(name: &str) -> String {
    let root = std::env::var_os("ARA_TEST_IMAGE_FIXTURE_DIR")
        .expect("Root must generate fixed Bun.Image nearest fixtures once");
    base64::engine::general_purpose::STANDARD.encode(std::fs::read(std::path::PathBuf::from(root).join(name)).unwrap())
}
