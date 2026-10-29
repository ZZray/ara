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

//! Bounded new ingress/state faults. Real Bun decode, no provider/Chrome calls.
#[path = "support/bun_image_codec.rs"]
mod bun_image_codec;
use ara_tools::read_image::{
    MAX_IMAGE_INPUT_BYTES, ReadImageKind, ReadImagePolicyPort, ReadImageRuntime, image_decode_failure_reason,
    load_in_memory_image, load_read_image,
};
use base64::Engine;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;
const RED: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";
struct Policy(AtomicBool);
impl ReadImagePolicyPort for Policy {
    fn current_state(&self) -> ReadImageRuntime {
        ReadImageRuntime { inspect_image_active: self.0.load(Ordering::SeqCst), ..Default::default() }
    }
}
#[tokio::test]
async fn rejects_invalid_base64_mismatch_decode_failure_and_live_oversize_ingress() {
    let codec = bun_image_codec::NativeBunCodec::new();
    assert_eq!(
        image_decode_failure_reason("!!!!", "image/png", &codec, CancellationToken::new()).await.unwrap().as_deref(),
        Some("invalid base64 image data")
    );
    assert_eq!(
        image_decode_failure_reason("", "image/png", &codec, CancellationToken::new()).await.unwrap().as_deref(),
        Some("empty image data")
    );
    assert_eq!(
        image_decode_failure_reason(RED, "image/jpeg", &codec, CancellationToken::new()).await.unwrap().as_deref(),
        Some("declared image/jpeg but contains image/png")
    );
    let truncated = base64::engine::general_purpose::STANDARD
        .encode(base64::engine::general_purpose::STANDARD.decode(RED).unwrap()[..33].to_vec());
    assert!(
        image_decode_failure_reason(&truncated, "image/png", &codec, CancellationToken::new()).await.unwrap().is_some()
    );
    assert!(
        load_in_memory_image(
            &truncated,
            "image/png",
            std::path::Path::new("broken.png"),
            "Read image file",
            false,
            MAX_IMAGE_INPUT_BYTES,
            None,
            None,
            &codec,
            CancellationToken::new()
        )
        .await
        .unwrap_err()
        .0
        .contains("not a decodable image/png image")
    );
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("large.png");
    std::fs::write(&file, base64::engine::general_purpose::STANDARD.decode(RED).unwrap()).unwrap();
    std::fs::OpenOptions::new().write(true).open(&file).unwrap().set_len(MAX_IMAGE_INPUT_BYTES + 1).unwrap();
    let policy = Policy(AtomicBool::new(true));
    let inspected = load_read_image(
        &file,
        ReadImageKind::Raster,
        None,
        "large.png",
        false,
        Some(&policy),
        None,
        CancellationToken::new(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(inspected.content.len(), 1);
    assert!(
        matches!(&inspected.content[0],ara_ai::UserBlock::Text(text) if text.text.contains("Image metadata:")&&text.text.contains("- Channels: 3")&&text.text.contains("- Alpha: no"))
    );
    policy.0.store(false, Ordering::SeqCst);
    assert!(
        load_read_image(
            &file,
            ReadImageKind::Raster,
            None,
            "large.png",
            false,
            Some(&policy),
            Some(&codec),
            CancellationToken::new()
        )
        .await
        .unwrap_err()
        .0
        .contains("Image file too large:")
    );
}
