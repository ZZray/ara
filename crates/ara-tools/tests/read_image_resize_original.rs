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

//! All14 fixed OMP image-resize.test.ts originals, using actual Host Bun.Image.
#[path = "support/bun_image_codec.rs"]
mod bun_image_codec;
use ara_tools::read_image::{ImageCodecPort, ImageResizeOptions, resize_image};
use base64::Engine;
use bun_image_codec::{NativeBunCodec, fixture};
use tokio_util::sync::CancellationToken;
const RED_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

#[tokio::test]
async fn original_resize_01_downscales_inputs_larger_than_1568px_on_the_long_edge() {
    let codec = NativeBunCodec::new();
    let result = resize_image(
        &fixture("oversizedPng.bin"),
        Some("image/png"),
        &Default::default(),
        None,
        &codec,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(result.was_resized);
    assert!(result.width <= 1568.0);
    assert!(result.height <= 1568.0);
    assert!((result.width / result.height - 1600.0 / 400.0).abs() < 0.01);
}
#[tokio::test]
async fn original_resize_02_preserves_inputs_already_within_budget_and_dimensions_fast_path() {
    let codec = NativeBunCodec::new();
    let result = resize_image(
        &fixture("smallPng.bin"),
        Some("image/png"),
        &Default::default(),
        None,
        &codec,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(!result.was_resized);
    assert_eq!(result.width, 200.0);
    assert_eq!(result.height, 200.0);
    assert_eq!(result.mime_type, "image/png");
}
#[tokio::test]
async fn original_resize_03_respects_custom_max_width_max_height_overrides_browser_tool_case() {
    let codec = NativeBunCodec::new();
    let options = ImageResizeOptions::screenshot_stage_one();
    let result =
        resize_image(&fixture("oversizedPng.bin"), Some("image/png"), &options, None, &codec, CancellationToken::new())
            .await
            .unwrap();
    assert!(result.was_resized);
    assert!(result.width <= 1024.0);
    assert!(result.height <= 1024.0);
    assert!(result.buffer.len() <= 150 * 1024);
}
#[tokio::test]
async fn original_resize_04_respects_custom_max_bytes_override_even_when_dimensions_already_fit() {
    let codec = NativeBunCodec::new();
    let input = fixture("smallPng.bin");
    let original_bytes = base64::engine::general_purpose::STANDARD.decode(&input).unwrap().len();
    let result = resize_image(
        &input,
        Some("image/png"),
        &ImageResizeOptions { max_bytes: Some(1024.0), ..Default::default() },
        None,
        &codec,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(result.buffer.len() <= original_bytes);
}
#[tokio::test]
async fn original_resize_05_uses_lossy_webp_or_jpeg_not_png_for_oversized_inputs() {
    let codec = NativeBunCodec::new();
    let result = resize_image(
        &fixture("oversizedPng.bin"),
        Some("image/png"),
        &Default::default(),
        None,
        &codec,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(result.was_resized);
    assert!(["image/jpeg", "image/webp"].contains(&result.mime_type.as_str()));
    assert!(result.buffer.len() <= 500 * 1024);
}
#[tokio::test]
async fn original_resize_06_excludes_webp_when_exclude_webp_option_is_true() {
    let codec = NativeBunCodec::new();
    let result = resize_image(
        &fixture("oversizedPng.bin"),
        Some("image/png"),
        &ImageResizeOptions { exclude_webp: Some(true), ..Default::default() },
        None,
        &codec,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(result.was_resized);
    assert!(["image/png", "image/jpeg"].contains(&result.mime_type.as_str()));
    assert_ne!(result.mime_type, "image/webp");
}
#[tokio::test]
async fn original_resize_07_reencodes_webp_source_when_excluded_even_on_fast_path() {
    let codec = NativeBunCodec::new();
    let result = resize_image(
        &fixture("smallWebp.bin"),
        Some("image/webp"),
        &ImageResizeOptions { exclude_webp: Some(true), ..Default::default() },
        None,
        &codec,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_ne!(result.mime_type, "image/webp");
    assert!(["image/png", "image/jpeg"].contains(&result.mime_type.as_str()));
}
#[tokio::test]
async fn original_resize_08_reports_png_header_dimensions_when_bun_image_rejects_after_ihdr() {
    let codec = NativeBunCodec::new();
    let mut png = vec![0; 33];
    png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    png[8..12].copy_from_slice(&13_u32.to_be_bytes());
    png[12..16].copy_from_slice(b"IHDR");
    png[16..20].copy_from_slice(&1900_u32.to_be_bytes());
    png[20..24].copy_from_slice(&2474_u32.to_be_bytes());
    png[24] = 8;
    png[25] = 2;
    let result = resize_image(
        &base64::engine::general_purpose::STANDARD.encode(&png),
        Some("image/png"),
        &Default::default(),
        None,
        &codec,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(result.width, 1900.0);
    assert_eq!(result.height, 2474.0);
    assert_eq!(result.original_width, 1900.0);
    assert_eq!(result.original_height, 2474.0);
    assert!(!result.was_resized);
    assert_eq!(result.buffer.len(), png.len());
}
#[tokio::test]
async fn original_resize_09_reports_jpeg_sof_dimensions_when_bun_image_rejects_after_header() {
    let codec = NativeBunCodec::new();
    let mut jpeg = vec![0; 12];
    jpeg[..4].copy_from_slice(&[0xff, 0xd8, 0xff, 0xc0]);
    jpeg[4..6].copy_from_slice(&8_u16.to_be_bytes());
    jpeg[6] = 8;
    jpeg[7..9].copy_from_slice(&2474_u16.to_be_bytes());
    jpeg[9..11].copy_from_slice(&1900_u16.to_be_bytes());
    jpeg[11] = 3;
    let result = resize_image(
        &base64::engine::general_purpose::STANDARD.encode(&jpeg),
        Some("image/jpeg"),
        &Default::default(),
        None,
        &codec,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(result.width, 1900.0);
    assert_eq!(result.height, 2474.0);
    assert_eq!(result.original_width, 1900.0);
    assert_eq!(result.original_height, 2474.0);
    assert!(!result.was_resized);
    assert_eq!(result.buffer.len(), jpeg.len());
}
#[tokio::test]
async fn original_resize_10_upscales_degenerate_1x1_image_to_200px_floor() {
    let codec = NativeBunCodec::new();
    let result = resize_image(RED_1X1, Some("image/png"), &Default::default(), None, &codec, CancellationToken::new())
        .await
        .unwrap();
    assert!(result.was_resized);
    assert_eq!(result.width, 200.0);
    assert_eq!(result.height, 200.0);
    let metadata = codec
        .decoded_metadata(
            &base64::engine::general_purpose::STANDARD.decode(result.data).unwrap(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(metadata.width >= 200.0);
    assert!(metadata.height >= 200.0);
}
#[tokio::test]
async fn original_resize_11_honors_custom_min_dimension_override() {
    let codec = NativeBunCodec::new();
    let result = resize_image(
        RED_1X1,
        Some("image/png"),
        &ImageResizeOptions { min_dimension: Some(64.0), ..Default::default() },
        None,
        &codec,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(result.width, 64.0);
    assert_eq!(result.height, 64.0);
}
#[tokio::test]
async fn original_resize_12_stretches_degenerate_aspect_ratio_to_floor_within_cap() {
    let codec = NativeBunCodec::new();
    let result = resize_image(
        &fixture("stripPng.bin"),
        Some("image/png"),
        &Default::default(),
        None,
        &codec,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(result.was_resized);
    assert!(result.width >= 200.0);
    assert!(result.height >= 200.0);
    assert!(result.width <= 1568.0);
    assert!(result.height <= 1568.0);
    let metadata = codec
        .decoded_metadata(
            &base64::engine::general_purpose::STANDARD.decode(result.data).unwrap(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(metadata.width >= 200.0);
    assert!(metadata.height >= 200.0);
}
#[tokio::test]
async fn original_resize_13_treats_no_webp_one_set_at_call_time_as_exclusion() {
    let codec = NativeBunCodec::new();
    let result = resize_image(
        &fixture("smallWebp.bin"),
        Some("image/webp"),
        &Default::default(),
        Some("1"),
        &codec,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_ne!(result.mime_type, "image/webp");
}
#[tokio::test]
async fn original_resize_14_treats_empty_and_zero_no_webp_as_not_excluded() {
    let codec = NativeBunCodec::new();
    let input = fixture("smallWebp.bin");
    let empty =
        resize_image(&input, Some("image/webp"), &Default::default(), Some(""), &codec, CancellationToken::new())
            .await
            .unwrap();
    assert_eq!(empty.mime_type, "image/webp");
    let zero =
        resize_image(&input, Some("image/webp"), &Default::default(), Some("0"), &codec, CancellationToken::new())
            .await
            .unwrap();
    assert_eq!(zero.mime_type, "image/webp");
}
