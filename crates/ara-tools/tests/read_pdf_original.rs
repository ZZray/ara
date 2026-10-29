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
// Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d; native-only ARA adaptation.

//! Complete fixed PDF READ originals: converter2 are in read_document.rs;
//! line-range4 and rendering3 are here, including the Unix literal-colon case.
//! Controlled adapters preserve original mocks; they do not prove real Chromium.

#[path="support/bun_image_codec.rs"] mod bun_image_codec;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}};
use ara_agent::{ToolError, ToolOutput};
use ara_ai::UserBlock;
use ara_tools::{PdfScreenshot, PdfScreenshotPort, ToolContext};
use ara_tools::read::ReadTool;
use ara_tools::read_document::PdfNativeAdapter;
use ara_tools::read_document_cache::{DocumentCache, CONVERTER_VERSION};
use ara_tools::read_pdf_native::PdfMarkdownResult;
use async_trait::async_trait;
use base64::Engine;
use tokio_util::sync::CancellationToken;

struct ConvertedBody {
    content: String,
    calls: Arc<AtomicUsize>,
}
impl PdfNativeAdapter for ConvertedBody {
    fn convert(&self, _input: &[u8]) -> Result<PdfMarkdownResult, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(PdfMarkdownResult {
            markdown: self.content.clone(), title: None, page_count: 3,
            pages_needing_ocr: vec![], has_encoding_issues: false,
        })
    }
}

fn mock_body(content: String) -> (Arc<dyn PdfNativeAdapter>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    (Arc::new(ConvertedBody {content, calls: calls.clone()}), calls)
}

fn text_of(result: &ToolOutput) -> String {
    result.content.iter().filter_map(|entry| match entry {
        UserBlock::Text(text) => Some(text.text.as_str()), _ => None,
    }).collect::<Vec<_>>().join("\n")
}

fn pdf_fixture(dir: &Path) -> PathBuf {
    let path = dir.join("doc.pdf");
    std::fs::write(&path, b"%PDF-stub").unwrap();
    path
}

fn rendering_pdf_fixture(dir: &Path) -> PathBuf {
    let path = dir.join("doc.pdf");
    std::fs::write(&path, format!("%PDF-stub-{}", dir.display())).unwrap();
    path
}

fn converted_200_lines() -> String {
    (1..=200).map(|i| format!("pdf line {i}")).collect::<Vec<_>>().join("\n")
}

async fn read(tool: &ReadTool, path: &str) -> ToolOutput {
    tool.execute_os_path(path, CancellationToken::new()).await.unwrap()
}

// B-1bdd46d963: original converted200 fixture; all four original assertions.
#[tokio::test]
async fn honours_n_m_against_converted_markdown_body() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = pdf_fixture(dir.path());
    let (adapter, _) = mock_body(converted_200_lines());
    let tool = ReadTool { ctx: ToolContext::new(dir.path()).with_pdf_adapter(adapter) };
    let text = text_of(&read(&tool, &format!("{}:120-122", pdf.display())).await);
    assert!(text.contains("pdf line 120"));
    assert!(text.contains("pdf line 122"));
    assert!(!text.contains("pdf line 1\n"));
    assert!(!text.contains("pdf line 5"));
}

// B-dca237d7ee: original multi-range fixture and all five original assertions.
#[tokio::test]
async fn honours_a_b_c_d_against_converted_markdown_body() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = pdf_fixture(dir.path());
    let (adapter, _) = mock_body(converted_200_lines());
    let tool = ReadTool { ctx: ToolContext::new(dir.path()).with_pdf_adapter(adapter) };
    let text = text_of(&read(&tool, &format!("{}:50-52,160-162", pdf.display())).await);
    assert!(text.contains("pdf line 50"));
    assert!(text.contains("pdf line 52"));
    assert!(text.contains("pdf line 160"));
    assert!(text.contains("pdf line 162"));
    assert!(!text.contains("pdf line 100"));
}

// B-c94f227bfe: terminal-LF three-line body, no selector, both assertions.
#[tokio::test]
async fn falls_back_to_full_converted_body_without_selector() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = pdf_fixture(dir.path());
    let (adapter, _) = mock_body("pdf line 1\npdf line 2\npdf line 3\n".into());
    let tool = ReadTool { ctx: ToolContext::new(dir.path()).with_pdf_adapter(adapter) };
    let text = text_of(&read(&tool, &pdf.to_string_lossy()).await);
    assert!(text.contains("pdf line 1"));
    assert!(text.contains("pdf line 3"));
}

// B-04b82508c3: same unchanged PDF full then selector; five original assertions.
// An explicit isolated Host cache replaces setAgentDir/env globals and teardown.
#[tokio::test]
async fn reuses_cached_markdown_across_full_and_selector_reads() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = pdf_fixture(dir.path());
    let (adapter, calls) = mock_body("pdf line 1\npdf line 2\npdf line 3\n".into());
    let cache = Arc::new(DocumentCache::new(dir.path().join("agent/cache/markit"), CONVERTER_VERSION.into()));
    let tool = ReadTool { ctx: ToolContext::new(dir.path()).with_pdf_adapter(adapter).with_document_cache(cache) };
    let full = text_of(&read(&tool, &pdf.to_string_lossy()).await);
    assert!(full.contains("pdf line 1"));
    assert!(full.contains("pdf line 3"));
    let selected = text_of(&read(&tool, &format!("{}:2-3", pdf.display())).await);
    assert!(selected.contains("pdf line 2"));
    assert!(selected.contains("pdf line 3"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

const ONE_PX_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

struct ControlledScreenshot {
    path: PathBuf,
    requests: Arc<Mutex<Vec<(PathBuf, u64, bool)>>>,
}
#[async_trait]
impl PdfScreenshotPort for ControlledScreenshot {
    async fn render_pdf_page(&self, authorized_path: &Path, page: u64, cancel: CancellationToken) -> Result<PdfScreenshot, ToolError> {
        self.requests.lock().unwrap().push((authorized_path.to_owned(), page, cancel.is_cancelled()));
        Ok(PdfScreenshot {
            path: self.path.clone(),
            bytes: base64::engine::general_purpose::STANDARD.decode(ONE_PX_PNG_BASE64).unwrap(),
            mime_type: "image/png".into(), width: 1, height: 1,
        })
    }
}

// B-94184adc90: original two page requests; image/text/path/last-call assertions.
// Original approval assertions are preserved in the paired CLI test candidate:
// exactly doc.pdf:p1-img0.png -> exec and doc.pdf:2-2 -> read through the existing
// dynamic WireString native approval function, NOT a replacement Rust classifier.
#[tokio::test]
async fn renders_former_image_member_reads_through_chromium_contract() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = rendering_pdf_fixture(dir.path());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let screenshot = dir.path().join("rendered.png");
    std::fs::write(&screenshot, base64::engine::general_purpose::STANDARD.decode(ONE_PX_PNG_BASE64).unwrap()).unwrap();
    let port = Arc::new(ControlledScreenshot {path: screenshot, requests: requests.clone()});
    let tool = ReadTool { ctx: ToolContext::new(dir.path()).with_pdf_screenshot_port(port)
        .with_read_auto_resize_images(false).with_read_image_codec(Arc::new(bun_image_codec::NativeBunCodec::new())) };
    for (path, page) in [(format!("{}:", pdf.display()), 1), (format!("{}:p2-img0.png", pdf.display()), 2)] {
        let result = read(&tool, &path).await;
        assert!(result.content.iter().any(|entry| matches!(entry, UserBlock::Image(image) if image.mime_type == "image/png")));
        assert!(text_of(&result).contains("Read image file [image/png]"));
        assert_eq!(result.details.as_ref().unwrap()["resolvedPath"].as_str(), Some(pdf.to_string_lossy().as_ref()));
        assert_eq!(requests.lock().unwrap().last(), Some(&(pdf.clone(), page, false)));
    }
}

// B-50b69b61f2: actual literal-colon file wins. Execute on native Unix; this
// identity is NOT compiled/credited on Windows and has no early-pass path.
#[cfg(unix)]
#[tokio::test]
async fn preserves_literal_filename_looking_like_pdf_image_listing() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = rendering_pdf_fixture(dir.path());
    let literal = format!("{}:", pdf.display());
    std::fs::write(&literal, "literal colon path wins\n").unwrap();
    let tool = ReadTool { ctx: ToolContext::new(dir.path()) };
    let result = read(&tool, &literal).await;
    assert!(text_of(&result).contains("literal colon path wins"));
}

// B-d1c8b99161: original normal document conversion: call count1 and selected line.
#[tokio::test]
async fn routes_pdf_line_selectors_through_normal_document_conversion() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = rendering_pdf_fixture(dir.path());
    let (adapter, calls) = mock_body("first line\nselected line\nthird line\n".into());
    let tool = ReadTool { ctx: ToolContext::new(dir.path()).with_pdf_adapter(adapter) };
    let result = read(&tool, &format!("{}:2-2", pdf.display())).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(text_of(&result).contains("selected line"));
}
