// MIT License
//
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
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

//! Real OS primitive for fixed video originals. Controlled test Host only;
//! production permissions, Tool receipts, Session grants remain separate gates.
use ara_tools::read_video_port::*;
use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{io::AsyncReadExt, process::Command, sync::Notify};
use tokio_util::sync::CancellationToken;

struct Owner {
    active: AtomicUsize,
    settled: Notify,
    workspaces: Mutex<Vec<Arc<Workspace>>>,
}
pub struct NativeVideoHost {
    owner: Arc<Owner>,
}
impl NativeVideoHost {
    pub fn new() -> Self {
        Self {
            owner: Arc::new(Owner {
                active: AtomicUsize::new(0),
                settled: Notify::new(),
                workspaces: Mutex::new(vec![]),
            }),
        }
    }
    pub fn binary(binary: MediaBinary) -> Option<PathBuf> {
        let key = match binary {
            MediaBinary::Ffmpeg => "ARA_TEST_FFMPEG",
            MediaBinary::Ffprobe => "ARA_TEST_FFPROBE",
        };
        // Exact runtime pin supplied by the main executor. No hidden downloads
        // or test skip: missing configured primitive is an explicit failure.
        let configured = std::env::var_os(key).map(PathBuf::from);
        configured.filter(|path| path.is_file())
    }
}
struct Workspace {
    path: PathBuf,
    temp: Mutex<Option<tempfile::TempDir>>,
    owner: std::sync::Weak<Owner>,
}
#[async_trait]
impl ReadVideoWorkspace for Workspace {
    fn path(&self) -> &Path {
        &self.path
    }
    async fn read_named(&self, name: &str) -> Result<Option<Vec<u8>>, VideoError> {
        tokio::fs::read(self.path.join(name)).await.map(Some).or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(VideoError::text(error.to_string()))
            }
        })
    }
    async fn nonempty_named(&self, name: &str) -> Result<bool, VideoError> {
        tokio::fs::metadata(self.path.join(name)).await.map(|metadata| metadata.len() > 0).or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(VideoError::text(error.to_string()))
            }
        })
    }
    async fn cleanup(&self) -> Result<(), VideoError> {
        if let Some(owner) = self.owner.upgrade() {
            loop {
                let changed = owner.settled.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if owner.active.load(Ordering::Acquire) == 0 {
                    break;
                }
                changed.await;
            }
        }
        let temp = self.temp.lock().unwrap().take();
        if let Some(temp) = temp {
            temp.close().map_err(|error| VideoError::text(error.to_string()))?;
        }
        Ok(())
    }
}
#[async_trait]
impl ReadVideoHostPort for NativeVideoHost {
    async fn find_binary(&self, binary: MediaBinary) -> Result<Option<PathBuf>, VideoError> {
        Ok(Self::binary(binary))
    }
    async fn execute(
        &self,
        command: MediaCommand,
        cancel: Option<CancellationToken>,
    ) -> Result<MediaProcessOutput, VideoError> {
        let owner = self.owner.clone();
        owner.active.fetch_add(1, Ordering::AcqRel);
        // Detached owner actor retains process settlement when an awaiting
        // caller is dropped; shared workspaces live in that owner until joined.
        tokio::spawn(async move {
            let result = async {
                let mut process = Command::new(command.executable);
                process
                    .args(command.args)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true);
                #[cfg(windows)]
                process.creation_flags(0x08000000);
                let mut child = process.spawn().map_err(|error| VideoError::text(error.to_string()))?;
                let mut stdout = child.stdout.take().unwrap();
                let mut stderr = child.stderr.take().unwrap();
                let drain_out = tokio::spawn(async move {
                    let mut bytes = vec![];
                    stdout.read_to_end(&mut bytes).await.map(|_| bytes)
                });
                let drain_err = tokio::spawn(async move {
                    let mut bytes = vec![];
                    stderr.read_to_end(&mut bytes).await.map(|_| bytes)
                });
                let status = if let Some(cancel) = cancel {
                    tokio::select! {
                        status=child.wait()=>status,
                        _=cancel.cancelled()=> { let _=child.start_kill(); child.wait().await }
                    }
                } else {
                    child.wait().await
                };
                let out = drain_out.await.map_err(|error| VideoError::text(error.to_string()))?;
                let err = drain_err.await.map_err(|error| VideoError::text(error.to_string()))?;
                Ok(MediaProcessOutput {
                    stdout: out.map_err(|error| VideoError::text(error.to_string()))?,
                    stderr: err.map_err(|error| VideoError::text(error.to_string()))?,
                    exit_code: status.map_err(|error| VideoError::text(error.to_string()))?.code().unwrap_or(-1),
                })
            }
            .await;
            owner.active.fetch_sub(1, Ordering::AcqRel);
            owner.settled.notify_waiters();
            result
        })
        .await
        .map_err(|error| VideoError::text(error.to_string()))?
    }
    async fn create_workspace(&self, prefix: &str) -> Result<Arc<dyn ReadVideoWorkspace>, VideoError> {
        let temp = test_directory(prefix);
        let workspace = Arc::new(Workspace {
            path: temp.path().to_path_buf(),
            temp: Mutex::new(Some(temp)),
            owner: Arc::downgrade(&self.owner),
        });
        self.owner.workspaces.lock().unwrap().push(workspace.clone());
        Ok(workspace)
    }
}

pub fn test_directory(prefix: &str) -> tempfile::TempDir {
    let root = std::env::var_os("ARA_TEST_VIDEO_TEMP_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("D:/workspace/env/tmp/ara-video-original"));
    assert!(root.to_string_lossy().to_ascii_lowercase().starts_with("d:"), "video fixture/temp must stay on D");
    std::fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new().prefix(prefix).tempdir_in(root).unwrap()
}
