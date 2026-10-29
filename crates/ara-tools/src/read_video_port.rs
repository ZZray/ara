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

//! Fixed OMP596f2da media primitives. MIT notices: THIRD_PARTY_NOTICES.md.
//! Host owns permission, binary/PATH discovery, subprocesses and workspaces;
//! Core owns selectors, ffprobe interpretation, argv and image/text policy.
use ara_agent::ToolError;
use ara_rpc::WireString;
use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaBinary {
    Ffmpeg,
    Ffprobe,
}
impl MediaBinary {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::Ffprobe => "ffprobe",
        }
    }
}
#[derive(Clone, Debug)]
pub struct MediaCommand {
    pub executable: PathBuf,
    pub args: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct MediaProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoErrorKind {
    Ordinary,
    Aborted,
    Unknown,
}
#[derive(Clone, Debug)]
pub struct VideoError {
    pub message: WireString,
    pub kind: VideoErrorKind,
}
impl VideoError {
    pub fn text(message: impl Into<String>) -> Self {
        Self { message: message.into().into(), kind: VideoErrorKind::Ordinary }
    }
    pub fn unknown(message: impl Into<String>) -> Self {
        Self { message: message.into().into(), kind: VideoErrorKind::Unknown }
    }
    pub fn aborted(message: impl Into<String>) -> Self {
        Self { message: message.into().into(), kind: VideoErrorKind::Aborted }
    }
    pub fn is_unknown(&self) -> bool {
        self.kind == VideoErrorKind::Unknown
    }
    pub fn is_aborted(&self) -> bool {
        self.kind == VideoErrorKind::Aborted
    }
    /// The OS ToolError boundary cannot encode an unpaired UTF16 surrogate.
    /// Native callers must throw message directly; never silently replace it.
    pub fn into_os_tool_error(self) -> ToolError {
        match self.message.to_utf8() {
            Ok(message) => ToolError(message),
            Err(_) => ToolError("Video error requires the native UTF16 throw boundary".into()),
        }
    }
}
impl From<ToolError> for VideoError {
    fn from(error: ToolError) -> Self {
        Self::text(error.0)
    }
}

#[async_trait]
pub trait ReadVideoWorkspace: Send + Sync {
    fn path(&self) -> &Path;
    async fn read_named(&self, name: &str) -> Result<Option<Vec<u8>>, VideoError>;
    async fn nonempty_named(&self, name: &str) -> Result<bool, VideoError>;
    /// Join retained media children before deleting. Drop must retain cleanup
    /// ownership if the caller disappears; cleanup failures remain observable.
    async fn cleanup(&self) -> Result<(), VideoError>;
}
#[async_trait]
pub trait ReadVideoHostPort: Send + Sync {
    /// Fixed $which Cached lookup, key including live PATH. Resolve before
    /// probe's pre-aborted check; no extra Run-cancel behavior at discovery.
    async fn find_binary(&self, binary: MediaBinary) -> Result<Option<PathBuf>, VideoError>;
    /// Direct argv only, separate stdout/stderr drained concurrently. Return
    /// success and Ordinary only after child/pipe settlement. Unknown may
    /// return early while the Host retains its registered child/pipe/workspace
    /// owners; Core must propagate it as failure and must not replay that
    /// uncertain operation. Aborted requires settlement, otherwise use Unknown.
    /// Some(cancel) kills/joins owned children. None is the fixed ffmpeg version
    /// probe, independent of Run cancellation.
    /// Host shutdown/drop still owns its process settlement. No shell/Chrome.
    async fn execute(
        &self,
        command: MediaCommand,
        cancel: Option<CancellationToken>,
    ) -> Result<MediaProcessOutput, VideoError>;
    /// Fixed prefixes were relative cwd prefixes. Host-selected D workspace
    /// placement is an explicit authorized cache-location difference.
    async fn create_workspace(&self, prefix: &str) -> Result<Arc<dyn ReadVideoWorkspace>, VideoError>;
}
