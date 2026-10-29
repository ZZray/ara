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

//! Exact fixed OMP596f2da test/utils/video.test.ts originals01-13.
use ara_tools::read_video::*;
fn frame(n: f64) -> Option<VideoSelector> {
    Some(VideoSelector::Frame { frame: n })
}
fn time(s: f64, raw: &str) -> Option<VideoSelector> {
    Some(VideoSelector::Time { seconds: s, raw: raw.into() })
}
#[test]
fn original_video_utils_01_reads_bare_integer_as_frame_index() {
    assert_eq!(parse_video_selector(Some("412")), frame(412.0));
}
#[test]
fn original_video_utils_02_reads_f_and_frame_prefixed_integers() {
    assert_eq!(parse_video_selector(Some("f7")), frame(7.0));
    assert_eq!(parse_video_selector(Some("frame102")), frame(102.0));
}
#[test]
fn original_video_utils_03_reads_unit_timestamps() {
    for (raw, s) in [("90s", 90.0), ("1h5m42s", 3942.0), ("5m30s", 330.0)] {
        assert_eq!(parse_video_selector(Some(raw)), time(s, raw));
    }
}
#[test]
fn original_video_utils_04_reads_colon_timestamps() {
    for (raw, s) in [("01:23", 83.0), ("1:02:03", 3723.0)] {
        assert_eq!(parse_video_selector(Some(raw)), time(s, raw));
    }
}
#[test]
fn original_video_utils_05_fractional_seconds_not_frame() {
    assert_eq!(parse_video_selector(Some("42.5")), time(42.5, "42.5"));
}
#[test]
fn original_video_utils_06_rejects_line_range_shapes() {
    for raw in [Some("50-200"), Some("raw"), Some("img"), None] {
        assert_eq!(parse_video_selector(raw), None);
    }
}
#[test]
fn original_video_utils_07_parses_minute_only() {
    assert_eq!(parse_video_timestamp("5m"), Some(300.0));
}
#[test]
fn original_video_utils_08_rejects_nontimestamp() {
    assert_eq!(parse_video_timestamp("412"), None);
    assert_eq!(parse_video_timestamp("abc"), None);
}
#[test]
fn original_video_utils_09_formats_subhour() {
    assert_eq!(format_video_timestamp(83.0), "1:23");
}
#[test]
fn original_video_utils_10_formats_hour() {
    assert_eq!(format_video_timestamp(3942.0), "1h5m42s");
}
#[test]
fn original_video_utils_11_splits_timestamp_suffix() {
    assert_eq!(split_video_read_target("clip.mp4:1h5m42s"), Some(("clip.mp4".into(), "1h5m42s".into())));
}
#[test]
fn original_video_utils_12_splits_frame_suffix() {
    assert_eq!(split_video_read_target("clip.mp4:412"), Some(("clip.mp4".into(), "412".into())));
}
#[test]
fn original_video_utils_13_leaves_nonvideo_and_nonselector() {
    for path in ["notes.txt:12", "clip.mp4:50-200", "clip.mp4"] {
        assert_eq!(split_video_read_target(path), None);
    }
}

/// Concentrated regression for the independent source review counterexample.
/// This uses real directory routing; no media runtime can hide a bad dispatch.
#[tokio::test]
async fn regression_video_named_directory_preserves_timestamp_frame_and_line_routes() {
    let root = std::path::Path::new("D:/workspace/env/tmp/ara-video-original");
    std::fs::create_dir_all(root).unwrap();
    let dir = tempfile::Builder::new().prefix("read-video-directory-").tempdir_in(root).unwrap();
    let target = dir.path().join("directory.mp4");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("child.txt"), "child").unwrap();
    let tool = ara_tools::read::ReadTool { ctx: ara_tools::ToolContext::new(dir.path()) };
    for selector in ["0:05", "f7", "50-200"] {
        let result = tool
            .execute_os_path(&format!("{}:{selector}", target.display()), tokio_util::sync::CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            result.details.as_ref().and_then(|details| details.get("isDirectory")).and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert!(!result.content.iter().any(|content| matches!(content, ara_ai::UserBlock::Image(_))));
        if selector != "50-200" {
            assert!(
                result
                    .content
                    .iter()
                    .any(|content| matches!(content,ara_ai::UserBlock::Text(text) if text.text.contains("child.txt")))
            );
        }
    }
    dir.close().unwrap();
}
