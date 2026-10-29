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

//! Fixed OMP596f2da test/tools/read-video.test.ts originals01-07.
//! NativeVideoHost is a real OS fixture, not the production Host authority.
#[path = "support/native_video.rs"]
mod native_video;
use ara_agent::ToolOutput;
use ara_ai::UserBlock;
use ara_tools::{ReadImagePolicyPort, ReadImageRuntime, ToolContext, read::ReadTool, read_video::*};
use base64::Engine;
use native_video::NativeVideoHost;
use std::{path::Path, sync::Arc};
use tokio_util::sync::CancellationToken;
struct Inspect(bool);
impl ReadImagePolicyPort for Inspect {
    fn current_state(&self) -> ReadImageRuntime {
        ReadImageRuntime { inspect_image_active: self.0, ..Default::default() }
    }
}
fn text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|part| match part {
            UserBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn image(output: &ToolOutput) -> Option<&ara_ai::ImageContent> {
    output.content.iter().find_map(|part| match part {
        UserBlock::Image(image) => Some(image),
        _ => None,
    })
}
fn tool(root: &Path, host: Arc<NativeVideoHost>, inspect: bool) -> ReadTool {
    ReadTool {
        ctx: ToolContext::new(root)
            .with_read_auto_resize_images(false)
            .with_read_image_policy(Arc::new(Inspect(inspect)))
            .with_read_video_runtime(Arc::new(ReadVideoRuntime::new(host))),
    }
}
async fn fixture() -> (tempfile::TempDir, Arc<NativeVideoHost>) {
    let dir = native_video::test_directory("read-video-");
    let host = Arc::new(NativeVideoHost::new());
    let ffmpeg = NativeVideoHost::binary(MediaBinary::Ffmpeg).expect("ARA_TEST_FFMPEG actual pin must be configured");
    NativeVideoHost::binary(MediaBinary::Ffprobe).expect("ARA_TEST_FFPROBE actual pin must be configured");
    // Preserve fixed beforeEach: create a fresh 12s 320x240 30fps H.264 clip
    // for every original, rather than replacing the fixture with static pixels.
    let result = host
        .execute(
            MediaCommand {
                executable: ffmpeg,
                args: vec![
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc=duration=12:size=320x240:rate=30",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:v",
                    "libx264",
                ]
                .into_iter()
                .map(str::to_owned)
                .chain([dir.path().join("clip.mp4").to_string_lossy().into_owned()])
                .collect(),
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0, "{}", String::from_utf8_lossy(&result.stderr));
    (dir, host)
}
#[tokio::test]
async fn original_read_video_01_preview_grid_plus_metadata() {
    let (dir, host) = fixture().await;
    let output = tool(dir.path(), host, false)
        .execute_os_path(&dir.path().join("clip.mp4").to_string_lossy(), CancellationToken::new())
        .await
        .unwrap();
    let text = text(&output);
    for needle in ["Video preview grid", "320x240", "h264", "FPS"] {
        assert!(text.contains(needle), "{text}");
    }
    let image = image(&output).expect("image");
    assert_eq!(image.mime_type, "image/png");
    let png = base64::engine::general_purpose::STANDARD.decode(&image.data).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    dir.close().unwrap();
}
#[tokio::test]
async fn original_read_video_02_active_nonaborted_signal_completes() {
    let (dir, host) = fixture().await;
    // Fixed timeout is 5000ms for the read, excluding beforeEach clip setup.
    let output = tokio::time::timeout(
        std::time::Duration::from_millis(5000),
        tool(dir.path(), host, false)
            .execute_os_path(&dir.path().join("clip.mp4").to_string_lossy(), CancellationToken::new()),
    )
    .await
    .expect("fixed 5000ms")
    .unwrap();
    assert!(image(&output).is_some());
    dir.close().unwrap();
}
#[tokio::test]
async fn original_read_video_03_tiles_every_extracted_frame() {
    let (dir, host) = fixture().await;
    let output = tool(dir.path(), host.clone(), false)
        .execute_os_path(&dir.path().join("clip.mp4").to_string_lossy(), CancellationToken::new())
        .await
        .unwrap();
    let image = image(&output).expect("image");
    let sheet = dir.path().join("sheet.png");
    std::fs::write(&sheet, base64::engine::general_purpose::STANDARD.decode(&image.data).unwrap()).unwrap();
    // Original oracle: top row SECOND cell, actual grayscale ffmpeg pixels;
    // padding or a fake metadata/header fixture cannot satisfy this assertion.
    let result = host
        .execute(
            MediaCommand {
                executable: NativeVideoHost::binary(MediaBinary::Ffmpeg).unwrap(),
                args: vec![
                    "-hide_banner".into(),
                    "-loglevel".into(),
                    "error".into(),
                    "-i".into(),
                    sheet.to_string_lossy().into_owned(),
                    "-vf".into(),
                    "crop=320:240:320:0".into(),
                    "-frames:v".into(),
                    "1".into(),
                    "-f".into(),
                    "rawvideo".into(),
                    "-pix_fmt".into(),
                    "gray".into(),
                    "pipe:1".into(),
                ],
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout.len(), 320 * 240);
    let mean = result.stdout.iter().map(|pixel| usize::from(*pixel)).sum::<usize>() as f64 / result.stdout.len() as f64;
    assert!(mean > 5.0, "second cell mean {mean}");
    dir.close().unwrap();
}
#[tokio::test]
async fn original_read_video_04_timestamp_single_frame() {
    let (dir, host) = fixture().await;
    let output = tool(dir.path(), host, false)
        .execute_os_path(&format!("{}:0:05", dir.path().join("clip.mp4").display()), CancellationToken::new())
        .await
        .unwrap();
    assert!(text(&output).contains("Video frame"));
    assert!(text(&output).contains("320x240"));
    assert!(image(&output).is_some());
    dir.close().unwrap();
}
#[tokio::test]
async fn original_read_video_05_integer_frame_index() {
    let (dir, host) = fixture().await;
    let output = tool(dir.path(), host, false)
        .execute_os_path(&format!("{}:100", dir.path().join("clip.mp4").display()), CancellationToken::new())
        .await
        .unwrap();
    assert!(text(&output).contains("frame 100"));
    assert!(image(&output).is_some());
    dir.close().unwrap();
}
#[tokio::test]
async fn original_read_video_06_line_range_video_hint() {
    let (dir, host) = fixture().await;
    let error = tool(dir.path(), host, false)
        .execute_os_path(&format!("{}:50-200", dir.path().join("clip.mp4").display()), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(error.0.contains(":<frame>"), "{}", error.0);
    dir.close().unwrap();
}
#[tokio::test]
async fn original_read_video_07_inspect_metadata_without_pixels() {
    let (dir, host) = fixture().await;
    let output = tool(dir.path(), host, true)
        .execute_os_path(&dir.path().join("clip.mp4").to_string_lossy(), CancellationToken::new())
        .await
        .unwrap();
    assert!(text(&output).contains("320x240"));
    assert!(image(&output).is_none());
    dir.close().unwrap();
}
