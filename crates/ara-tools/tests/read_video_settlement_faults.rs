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

//! Concentrated controlled faults for typed media UNKNOWN integration.
//! Actual D files model already-written outputs; no process/codec/model is
//! executed here. These assertions prove Core settlement policy only, never
//! production Host process ownership, pixel composition, or grant acceptance.
use ara_tools::read_video::*;
use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;
const PNG: &[u8] = b"\x89PNG\r\n\x1a\ncontrolled-output";
#[derive(Clone, Copy)]
enum Fault {
    PendingSeek,
    PendingCleanup,
    KnownSeek,
    UnknownVersion,
    KnownVersion,
}
struct Shared {
    fault: Fault,
    root: PathBuf,
    seeks: AtomicUsize,
    versions: AtomicUsize,
    frames: AtomicUsize,
    tiles: AtomicUsize,
    workspaces: Mutex<Vec<PathBuf>>,
}
struct ControlledHost(Arc<Shared>);
struct Workspace {
    path: PathBuf,
    shared: Arc<Shared>,
}
#[async_trait]
impl ReadVideoWorkspace for Workspace {
    fn path(&self) -> &Path {
        &self.path
    }
    async fn read_named(&self, name: &str) -> Result<Option<Vec<u8>>, VideoError> {
        match std::fs::read(self.path.join(name)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(VideoError::text(error.to_string())),
        }
    }
    async fn nonempty_named(&self, name: &str) -> Result<bool, VideoError> {
        Ok(std::fs::metadata(self.path.join(name)).is_ok_and(|metadata| metadata.len() > 0))
    }
    async fn cleanup(&self) -> Result<(), VideoError> {
        if matches!(self.shared.fault, Fault::PendingSeek | Fault::PendingCleanup) {
            return Err(VideoError::unknown("controlled pending output writer; settlement unknown"));
        }
        std::fs::remove_dir_all(&self.path).map_err(|error| VideoError::text(error.to_string()))
    }
}
#[async_trait]
impl ReadVideoHostPort for ControlledHost {
    async fn find_binary(&self, binary: MediaBinary) -> Result<Option<PathBuf>, VideoError> {
        Ok(Some(PathBuf::from(binary.name())))
    }
    async fn create_workspace(&self, prefix: &str) -> Result<Arc<dyn ReadVideoWorkspace>, VideoError> {
        let path = self.0.root.join(format!("{prefix}{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&path).unwrap();
        self.0.workspaces.lock().unwrap().push(path.clone());
        Ok(Arc::new(Workspace { path, shared: self.0.clone() }))
    }
    async fn execute(
        &self,
        command: MediaCommand,
        _: Option<CancellationToken>,
    ) -> Result<MediaProcessOutput, VideoError> {
        if command.executable == Path::new("ffprobe") {
            return Ok(MediaProcessOutput{stdout:br#"{"streams":[{"codec_type":"video","codec_name":"h264","width":320,"height":240,"avg_frame_rate":"30/1"}],"format":{"duration":"12","format_name":"mov,mp4"}}"#.to_vec(),stderr:vec![],exit_code:0});
        }
        if command.args == ["-version"] {
            self.0.versions.fetch_add(1, Ordering::SeqCst);
            return match self.0.fault {
                Fault::UnknownVersion => {
                    Err(VideoError::unknown("controlled version child still owned; settlement unknown"))
                }
                Fault::KnownVersion => Err(VideoError::text("controlled confirmed spawn rejection")),
                _ => {
                    Ok(MediaProcessOutput { stdout: b"ffmpeg version 9.0.1\n".to_vec(), stderr: vec![], exit_code: 0 })
                }
            };
        }
        let output = PathBuf::from(command.args.last().unwrap());
        std::fs::write(output, PNG).unwrap();
        if command.args.iter().any(|arg| arg == "scale=320:-1") {
            let index = self.0.seeks.fetch_add(1, Ordering::SeqCst);
            if index == 0 {
                match self.0.fault {
                    Fault::PendingSeek => {
                        return Err(VideoError::unknown(
                            "controlled seek wrote PNG but child is pending; settlement unknown",
                        ));
                    }
                    Fault::KnownSeek => return Err(VideoError::text("controlled confirmed ffmpeg exit1")),
                    _ => {}
                }
            }
        } else if command.args.iter().any(|arg| arg == "-filter_complex") {
            self.0.tiles.fetch_add(1, Ordering::SeqCst);
        } else {
            self.0.frames.fetch_add(1, Ordering::SeqCst);
            assert!(command.args.windows(2).any(|args| args == ["-fps_mode", "passthrough"]));
        }
        Ok(MediaProcessOutput { stdout: vec![], stderr: vec![], exit_code: 0 })
    }
}
fn fixture(fault: Fault) -> (tempfile::TempDir, Arc<Shared>, ReadVideoRuntime) {
    let root = Path::new("D:/workspace/env/tmp/ara-video-original");
    std::fs::create_dir_all(root).unwrap();
    let dir = tempfile::Builder::new().prefix("read-video-settlement-").tempdir_in(root).unwrap();
    let shared = Arc::new(Shared {
        fault,
        root: dir.path().to_path_buf(),
        seeks: AtomicUsize::new(0),
        versions: AtomicUsize::new(0),
        frames: AtomicUsize::new(0),
        tiles: AtomicUsize::new(0),
        workspaces: Mutex::new(vec![]),
    });
    let runtime = ReadVideoRuntime::new(Arc::new(ControlledHost(shared.clone())));
    (dir, shared, runtime)
}
#[tokio::test]
async fn fault_unknown_seek_or_cleanup_with_nonempty_png_never_attaches_success() {
    for fault in [Fault::PendingSeek, Fault::PendingCleanup, Fault::KnownSeek] {
        let (dir, state, runtime) = fixture(fault);
        let result = read_video_file(
            &runtime,
            Path::new("clip.mp4"),
            "clip.mp4",
            None,
            42,
            false,
            None,
            None,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(
            state.seeks.load(Ordering::SeqCst),
            6,
            "all initially-started workers settle their controlled receipts"
        );
        if matches!(fault, Fault::KnownSeek) {
            assert!(result.is_ok(), "preserve fixed confirmed seek-error catch");
            assert_eq!(state.tiles.load(Ordering::SeqCst), 1);
        } else {
            let error = result.expect_err("UNKNOWN must not become ToolOutput success");
            assert!(error.is_unknown());
            assert_eq!(
                state.tiles.load(Ordering::SeqCst),
                usize::from(matches!(fault, Fault::PendingCleanup)),
                "pending seek cannot enter tile; cleanup unknown cannot accept an already-created sheet"
            );
            let workspaces = state.workspaces.lock().unwrap();
            assert!(
                workspaces.iter().any(|workspace| std::fs::metadata(workspace.join("thumb-0.png"))
                    .is_ok_and(|metadata| metadata.len() > 0)),
                "pending owned PNG must remain present"
            );
        }
        // Controlled fake has no external live process; only test-owned D data.
        dir.close().unwrap();
    }
}
#[tokio::test]
async fn fault_unknown_version_has_no_modern_fallback_or_automatic_probe_replay() {
    for fault in [Fault::UnknownVersion, Fault::KnownVersion] {
        let (dir, state, runtime) = fixture(fault);
        for _ in 0..2 {
            let result = read_video_file(
                &runtime,
                Path::new("clip.mp4"),
                "clip.mp4",
                Some("100"),
                42,
                false,
                None,
                None,
                CancellationToken::new(),
            )
            .await;
            if matches!(fault, Fault::UnknownVersion) {
                assert!(result.unwrap_err().is_unknown());
            } else {
                assert!(result.is_ok(), "fixed confirmed version failure still uses modern fallback");
            }
        }
        assert_eq!(state.versions.load(Ordering::SeqCst), 1, "same runtime must not replay an uncertain version child");
        assert_eq!(
            state.frames.load(Ordering::SeqCst),
            if matches!(fault, Fault::UnknownVersion) { 0 } else { 2 },
            "UNKNOWN cannot fall back and start frame extraction"
        );
        dir.close().unwrap();
    }
}
