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

//! Fixed OMP596f2da markit-cache.test.ts original wrappers01-04.
//! Copyright (c)2025 Mario Zechner; Copyright (c)2025-2026 Can Bölük;
//! Copyright (c)2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! Controlled converter callback matches Markit.prototype.convert spy.

use ara_agent::ToolError;
use ara_tools::read_document_cache::{
    CONVERTER_VERSION, CachedDocumentConversion, DocumentCache, DocumentCacheStatus, DocumentStreamInfo,
    convert_buffer_with_cache, convert_file_with_cache,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

fn cache(root: &std::path::Path) -> DocumentCache {
    DocumentCache::new(root.join("agent/cache/document-conversions"), CONVERTER_VERSION.into())
}

// MARKIT-CACHE-01: identical bytes, pdf then .pdf, all three original assertions.
#[test]
fn original_markit_cache_01_caches_successful_buffer_conversions_by_content_hash_and_normalized_extension() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache(dir.path());
    let calls = AtomicUsize::new(0);
    let convert = |_: &[u8], _: &DocumentStreamInfo| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok("cached body".into())
    };
    let bytes = b"hello pdf bytes";
    let cancel = CancellationToken::new();
    let first = convert_buffer_with_cache(Some(&cache), bytes, "pdf", &cancel, true, convert).unwrap();
    assert_eq!(
        first,
        CachedDocumentConversion {
            ok: true,
            content: "cached body".into(),
            error: None,
            cache: DocumentCacheStatus::Miss
        }
    );
    let second = convert_buffer_with_cache(Some(&cache), bytes, ".pdf", &cancel, true, convert).unwrap();
    assert_eq!(
        second,
        CachedDocumentConversion {
            ok: true,
            content: "cached body".into(),
            error: None,
            cache: DocumentCacheStatus::Hit
        }
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// MARKIT-CACHE-02: first rejection then recovery; all five original assertions.
#[test]
fn original_markit_cache_02_does_not_cache_failed_conversions() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache(dir.path());
    let calls = AtomicUsize::new(0);
    let convert = |_: &[u8], _: &DocumentStreamInfo| {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 { Err(ToolError("boom".into())) } else { Ok("recovered".into()) }
    };
    let bytes = b"retry me";
    let cancel = CancellationToken::new();
    let first = convert_buffer_with_cache(Some(&cache), bytes, ".pdf", &cancel, true, convert).unwrap();
    assert!(!first.ok);
    let second = convert_buffer_with_cache(Some(&cache), bytes, ".pdf", &cancel, true, convert).unwrap();
    assert!(second.ok);
    assert_eq!(second.content, "recovered");
    assert_eq!(second.cache, DocumentCacheStatus::Miss);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

// MARKIT-CACHE-03: same doc.pdf v1->v2->v2; all seven original assertions.
#[test]
fn original_markit_cache_03_invalidates_file_conversions_by_content_hash() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache(dir.path());
    let calls = AtomicUsize::new(0);
    let convert = |_: &[u8], _: &DocumentStreamInfo| {
        Ok(if calls.fetch_add(1, Ordering::SeqCst) == 0 { "first" } else { "second" }.into())
    };
    let file = dir.path().join("doc.pdf");
    let cancel = CancellationToken::new();
    std::fs::write(&file, b"v1").unwrap();
    let v1 = convert_file_with_cache(Some(&cache), &file, &cancel, None, convert).unwrap();
    assert_eq!(v1.cache, DocumentCacheStatus::Miss);
    assert_eq!(v1.content, "first");
    std::fs::write(&file, b"v2").unwrap();
    let v2 = convert_file_with_cache(Some(&cache), &file, &cancel, None, convert).unwrap();
    assert_eq!(v2.cache, DocumentCacheStatus::Miss);
    assert_eq!(v2.content, "second");
    let v2_again = convert_file_with_cache(Some(&cache), &file, &cancel, None, convert).unwrap();
    assert_eq!(v2_again.cache, DocumentCacheStatus::Hit);
    assert_eq!(v2_again.content, "second");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

// MARKIT-CACHE-04: original docx/imageDir fixture, all three original assertions.
#[test]
fn original_markit_cache_04_skips_cache_for_image_dir_conversions() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache(dir.path());
    let calls = AtomicUsize::new(0);
    let convert = |_: &[u8], _: &DocumentStreamInfo| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok("image body".into())
    };
    let file = dir.path().join("image-doc.docx");
    std::fs::write(&file, b"image bytes").unwrap();
    let image_dir = dir.path().join("images");
    let cancel = CancellationToken::new();
    let first = convert_file_with_cache(Some(&cache), &file, &cancel, Some(&image_dir), convert).unwrap();
    assert_eq!(first.cache, DocumentCacheStatus::Skipped);
    let second = convert_file_with_cache(Some(&cache), &file, &cancel, Some(&image_dir), convert).unwrap();
    assert_eq!(second.cache, DocumentCacheStatus::Skipped);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
