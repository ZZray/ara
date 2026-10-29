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

//! Fixed OMP PdfConverter wrapper. Native work stays on READ's blocking worker.
//! Host cache location and screenshot authority belong to their existing owners.

use ara_agent::ToolError;
use crate::read_pdf_native::{PdfMarkdownResult, pdf_to_markdown};

/// Native PDF bridge. Injection retains the original controlled converter tests.
/// Implementations run on the caller's blocking worker, never a Tokio reactor.
pub trait PdfNativeAdapter: Send + Sync {
    fn convert(&self, input: &[u8]) -> Result<PdfMarkdownResult, ToolError>;
}

impl<F> PdfNativeAdapter for F
where
    F: Fn(&[u8]) -> Result<PdfMarkdownResult, ToolError> + Send + Sync,
{
    fn convert(&self, input: &[u8]) -> Result<PdfMarkdownResult, ToolError> { self(input) }
}

#[derive(Debug, Default)]
pub struct NativePdfAdapter;

impl PdfNativeAdapter for NativePdfAdapter {
    fn convert(&self, input: &[u8]) -> Result<PdfMarkdownResult, ToolError> {
        pdf_to_markdown(input)
    }
}

/// Fixed PdfConverter.name for registry/host consumers.
pub const PDF_CONVERTER_NAME: &str = "pdf";

/// Fixed ConversionResult PDF fields. An absent native title stays absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentConversion {
    pub markdown: String,
    pub title: Option<String>,
}

/// Fixed PdfConverter accepts: exact extension, or either PDF MIME prefix.
/// File consumers normalize their extension before calling; the converter does not.
pub fn pdf_accepts(extension: Option<&str>, mimetype: Option<&str>) -> bool {
    extension == Some(".pdf")
        || mimetype.is_some_and(|mime| {
            ["application/pdf", "application/x-pdf"].iter().any(|prefix| mime.starts_with(prefix))
        })
}

pub fn convert_pdf(input: &[u8]) -> Result<DocumentConversion, ToolError> {
    convert_pdf_with(input, &NativePdfAdapter)
}

/// Preserve exact OCR notice and `filter(Boolean).join("\n\n")` behavior.
pub fn convert_pdf_with(
    input: &[u8],
    adapter: &dyn PdfNativeAdapter,
) -> Result<DocumentConversion, ToolError> {
    let result = adapter.convert(input)?;
    let markdown = if result.pages_needing_ocr.is_empty() {
        result.markdown
    } else {
        let pages = result.pages_needing_ocr.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
        let notice = format!("Text extraction is incomplete for PDF pages {pages}. Use the browser tool to render those pages or OCR them.");
        if result.markdown.is_empty() { notice } else { format!("{}\n\n{notice}", result.markdown) }
    };
    Ok(DocumentConversion { markdown, title: result.title })
}

#[cfg(test)]
mod original_converter_tests {
    use super::*;

    // B-b39af4fada, PdfConverter: keeps accepting PDF extensions and MIME types.
    #[test]
    fn keeps_accepting_pdf_extensions_and_mime_types() {
        assert!(pdf_accepts(Some(".pdf"), None));
        assert!(pdf_accepts(None, Some("application/pdf")));
        assert!(pdf_accepts(None, Some("application/pdf; charset=binary")));
        assert!(pdf_accepts(None, Some("application/x-pdf")));
        assert!(!pdf_accepts(Some(".txt"), Some("text/plain")));
    }

    // B-b04b0dac32, original controlled native mock, not a real PDF extraction.
    #[test]
    fn returns_browser_and_ocr_notice_for_image_only_pdf() {
        let adapter = |_input: &[u8]| Ok(PdfMarkdownResult {
            markdown: String::new(), title: None, page_count: 3,
            pages_needing_ocr: vec![1, 3], has_encoding_issues: false,
        });
        let result = convert_pdf_with(b"image-only pdf", &adapter).unwrap();
        assert_eq!(result.markdown, "Text extraction is incomplete for PDF pages 1, 3. Use the browser tool to render those pages or OCR them.");
        assert!(!result.markdown.is_empty());
    }
}
