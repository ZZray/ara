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

//! Fixed read-local-image.test.ts video originals06-07. Controlled authorized
//! file capability plus real ffprobe; production containment09 is separate.
#[path = "support/native_video.rs"]
mod native_video;
use ara_agent::ToolError;
use ara_tools::{
    ContentUriPort, ContentUriRoute, ToolContext, UriFileResource, UriResource, read::ReadTool, read_video::*,
};
use async_trait::async_trait;
use native_video::NativeVideoHost;
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;
struct LocalFixture {
    root: PathBuf,
}
#[async_trait]
impl ContentUriPort for LocalFixture {
    fn route(&self, _: &str) -> ContentUriRoute {
        ContentUriRoute::Registered { writable: false }
    }
    async fn read_file(&self, url: &str, _: CancellationToken) -> Result<Option<UriFileResource>, ToolError> {
        Ok(Some(UriFileResource {
            path: self.root.join(url.strip_prefix("local://").unwrap()),
            content_type: "application/octet-stream".into(),
            max_inline_bytes: None,
            immutable: false,
        }))
    }
    async fn read(&self, _: &str, _: CancellationToken) -> Result<UriResource, ToolError> {
        Err(ToolError("Fixture always grants a file capability".into()))
    }
    async fn write(&self, _: &str, _: &str, _: CancellationToken) -> Result<(), ToolError> {
        Err(ToolError("Read-only fixture".into()))
    }
}
async fn corrupt(name: &str, bytes: &[u8]) {
    let dir = native_video::test_directory("read-local-video-");
    let root = dir.path().join("artifacts/local");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(name), bytes).unwrap();
    let mut ctx = ToolContext::new(dir.path())
        .with_read_auto_resize_images(false)
        .with_read_video_runtime(Arc::new(ReadVideoRuntime::new(Arc::new(NativeVideoHost::new()))));
    ctx.set_uri_port(Arc::new(LocalFixture { root }));
    let error =
        ReadTool { ctx }.execute_os_path(&format!("local://{name}"), CancellationToken::new()).await.unwrap_err();
    if NativeVideoHost::binary(MediaBinary::Ffprobe).is_some() {
        assert!(error.0.contains("Could not probe video"), "{}", error.0);
        assert!(error.0.contains(name), "{}", error.0);
    } else {
        assert!(error.0.contains("requires ffprobe"), "{}", error.0);
    }
    assert!(!error.0.contains('\0'));
    dir.close().unwrap();
}
#[tokio::test]
async fn original_local_video_06_corrupt_six_byte_clip_probe_failure() {
    corrupt("clip.mp4", &[0, 1, 2, 3, 4, 5]).await;
}
#[tokio::test]
async fn original_local_video_07_large_256kib_nul_probe_failure() {
    corrupt("video.mp4", &vec![0; 256 * 1024]).await;
}
