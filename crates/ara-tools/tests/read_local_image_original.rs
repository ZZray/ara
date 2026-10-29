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

//! Fixed read-local-image.test.ts image/text originals01-05.
//! Video06-07 and Host binary08/Unix containment09 remain explicit mapping rows.
#[path = "support/bun_image_codec.rs"]
mod bun_image_codec;
use ara_agent::ToolError;
use ara_ai::UserBlock;
use ara_tools::read::ReadTool;
use ara_tools::{
    ContentUriPort, ContentUriRoute, ReadImagePolicyPort, ReadImageRuntime, ToolContext, UriFileResource, UriResource,
};
use async_trait::async_trait;
use base64::Engine;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;
const TINY_PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
const TINY_SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"12\" height=\"7\"><rect width=\"12\" height=\"7\" fill=\"red\"/></svg>";
struct LocalFixture {
    root: PathBuf,
}
#[async_trait]
impl ContentUriPort for LocalFixture {
    fn route(&self, _: &str) -> ContentUriRoute {
        ContentUriRoute::Registered { writable: false }
    }
    async fn read_file(&self, url: &str, _: CancellationToken) -> Result<Option<UriFileResource>, ToolError> {
        // Controlled Host file capability fixture, not proof of native Host's
        // realpath/containment implementation (original09 remains separate).
        Ok(Some(UriFileResource {
            path: self.root.join(url.strip_prefix("local://").unwrap()),
            content_type: "application/octet-stream".into(),
            max_inline_bytes: None,
            immutable: false,
        }))
    }
    async fn read(&self, _: &str, _: CancellationToken) -> Result<UriResource, ToolError> {
        Err(ToolError("Fixture always provides a file capability".into()))
    }
    async fn write(&self, _: &str, _: &str, _: CancellationToken) -> Result<(), ToolError> {
        Err(ToolError("Read-only fixture".into()))
    }
}
struct Inspect(bool);
impl ReadImagePolicyPort for Inspect {
    fn current_state(&self) -> ReadImageRuntime {
        ReadImageRuntime { inspect_image_active: self.0, ..Default::default() }
    }
}
fn local_root(dir: &Path) -> PathBuf {
    let root = dir.join("artifacts/local");
    std::fs::create_dir_all(&root).unwrap();
    root
}
fn tool(dir: &Path, root: PathBuf, inspect: bool, needs_codec: bool) -> ReadTool {
    let mut ctx =
        ToolContext::new(dir).with_read_auto_resize_images(false).with_read_image_policy(Arc::new(Inspect(inspect)));
    ctx.set_uri_port(Arc::new(LocalFixture { root }));
    if needs_codec {
        ctx = ctx.with_read_image_codec(Arc::new(bun_image_codec::NativeBunCodec::new()));
    }
    ReadTool { ctx }
}
fn text(output: &ara_agent::ToolOutput) -> String {
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
#[tokio::test]
async fn original_local_image_01_decodes_local_png_into_inline_image_block() {
    let dir = tempfile::tempdir().unwrap();
    let root = local_root(dir.path());
    std::fs::write(root.join("clifford.png"), base64::engine::general_purpose::STANDARD.decode(TINY_PNG).unwrap())
        .unwrap();
    let result = tool(dir.path(), root, false, true)
        .execute_os_path("local://clifford.png", CancellationToken::new())
        .await
        .unwrap();
    let image = result.content.iter().find_map(|part| match part {
        UserBlock::Image(image) => Some(image),
        _ => None,
    });
    assert!(image.is_some());
    assert_eq!(image.unwrap().mime_type, "image/png");
    assert!(!text(&result).contains("\u{fffd}PNG"));
}
#[tokio::test]
async fn original_local_image_02_rasterizes_local_svg_into_png_attachment_when_img_selected() {
    let dir = tempfile::tempdir().unwrap();
    let root = local_root(dir.path());
    let path = root.join("diagram.svg");
    std::fs::write(&path, TINY_SVG).unwrap();
    let result = tool(dir.path(), root, false, true)
        .execute_os_path(&format!("{}:img", path.display()), CancellationToken::new())
        .await
        .unwrap();
    let image = result
        .content
        .iter()
        .find_map(|part| match part {
            UserBlock::Image(image) => Some(image),
            _ => None,
        })
        .unwrap();
    assert_eq!(image.mime_type, "image/png");
    let png = base64::engine::general_purpose::STANDARD.decode(&image.data).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert!(text(&result).contains("Read SVG file [image/png]"));
}
#[tokio::test]
async fn original_local_image_03_directs_selected_svg_through_inspect_image_without_native_vision() {
    let dir = tempfile::tempdir().unwrap();
    let root = local_root(dir.path());
    std::fs::write(root.join("diagram.svg"), TINY_SVG).unwrap();
    let result = tool(dir.path(), root, true, false)
        .execute_os_path("local://diagram.svg:img", CancellationToken::new())
        .await
        .unwrap();
    assert!(!result.content.iter().any(|part| matches!(part, UserBlock::Image(_))));
    assert!(text(&result).contains("diagram.svg:img\""));
}
#[tokio::test]
async fn original_local_image_04_keeps_local_svg_as_text_without_img() {
    let dir = tempfile::tempdir().unwrap();
    let root = local_root(dir.path());
    std::fs::write(root.join("diagram.svg"), TINY_SVG).unwrap();
    let result = tool(dir.path(), root, false, false)
        .execute_os_path("local://diagram.svg", CancellationToken::new())
        .await
        .unwrap();
    assert!(!result.content.iter().any(|part| matches!(part, UserBlock::Image(_))));
    assert!(text(&result).contains("<svg"));
}
#[tokio::test]
async fn original_local_image_05_still_reads_local_text_file_as_text() {
    let dir = tempfile::tempdir().unwrap();
    let root = local_root(dir.path());
    std::fs::write(root.join("notes.txt"), "hello world").unwrap();
    let result = tool(dir.path(), root, false, false)
        .execute_os_path("local://notes.txt", CancellationToken::new())
        .await
        .unwrap();
    assert!(!result.content.iter().any(|part| matches!(part, UserBlock::Image(_))));
    assert!(text(&result).contains("hello world"));
}
