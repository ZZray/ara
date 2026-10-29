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

//! Fixed OMP read-pdf.ts Host boundary. Browser/session/artifacts are Host-owned.
//! This port does not provide a browser implementation or confer authorization.
use ara_agent::ToolError;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct PdfScreenshot {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
}

#[async_trait]
pub trait PdfScreenshotPort: Send + Sync {
    /// Invoked only after the Host's normal tool admission. Implementations
    /// own the fixed shared Chromium broker, tab lease and cancellation cleanup.
    async fn render_pdf_page(
        &self,
        authorized_path: &Path,
        page: u64,
        cancel: CancellationToken,
    ) -> Result<PdfScreenshot, ToolError>;
}

/// Fixed greedy PDF member and safe-positive page parsing. A recognized READ
/// line/raw/img selector is excluded by the caller before legacy dispatch.
pub(crate) fn split_pdf_image_path(path: &str) -> Option<(String, u64)> {
    static MEMBER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)^(.*\.pdf):(.*)$").expect("fixed PDF member regex")
    });
    static PAGE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)^(?:p|page[-_]?)(\d+)(?:[-_].*)?\.png$")
            .expect("fixed PDF page regex")
    });
    let capture = MEMBER.captures(path)?;
    let pdf = capture.get(1)?.as_str();
    if pdf.is_empty() {
        return None;
    }
    let member = capture.get(2)?.as_str();
    let page = PAGE
        .captures(member)
        .and_then(|matched| matched.get(1))
        .and_then(|number| number.as_str().parse::<u64>().ok())
        .filter(|page| *page > 0 && *page <= 9_007_199_254_740_991)
        .unwrap_or(1);
    Some((pdf.to_owned(), page))
}
