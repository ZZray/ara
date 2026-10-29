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

//! Fixed OMP596f2da image-loading.ts / image-resize.ts / utils/mime.ts.
//! Copyright (c)2025 Mario Zechner; Copyright (c)2025-2026 Can Bölük;
//! Copyright (c)2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! Core owns policy; the Host supplies the exact Bun.Image codec primitives.

use ara_agent::ToolError;
use ara_ai::{ImageContent, UserBlock};
use async_trait::async_trait;
use base64::Engine;
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

pub const MAX_IMAGE_INPUT_BYTES: u64 = 20 * 1024 * 1024;
pub const IMAGE_METADATA_HEADER_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedImageMetadata {
    pub width: f64,
    pub height: f64,
    pub format: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageEncoding {
    Png,
    Jpeg,
    WebP,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageEncodeBatch {
    pub width: f64,
    pub height: f64,
    pub quality: f64,
    /// Ordered like the fixed Promise.all: PNG, JPEG, WebP, or JPEG, WebP.
    pub formats: Vec<ImageEncoding>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedImage {
    pub buffer: Vec<u8>,
    pub mime_type: String,
}

#[async_trait]
pub trait ImageCodecPort: Send + Sync {
    async fn decoded_metadata(
        &self,
        bytes: &[u8],
        cancel: CancellationToken,
    ) -> Result<DecodedImageMetadata, ToolError>;
    /// Full Bun.Image decode, terminating at resize(1,1).png().bytes().
    async fn decode_probe_1x1(&self, bytes: &[u8], cancel: CancellationToken) -> Result<(), ToolError>;
    /// One current attempt only. Run its formats together; any rejection fails
    /// the whole batch. Do not run a later quality/scale attempt speculatively.
    async fn encode_many(
        &self,
        bytes: &[u8],
        batch: ImageEncodeBatch,
        cancel: CancellationToken,
    ) -> Result<Vec<EncodedImage>, ToolError>;
}

/// Host computes actual tool/xdev/mode/model availability at call time.
/// false model_excludes_webp means no override, preserving the env fallback.
#[derive(Clone, Debug, Default)]
pub struct ReadImageRuntime {
    pub inspect_image_active: bool,
    pub model_excludes_webp: bool,
    pub no_webp_env: Option<String>,
}
pub trait ReadImagePolicyPort: Send + Sync {
    fn current_state(&self) -> ReadImageRuntime;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImageResizeOptions {
    pub max_width: Option<f64>,
    pub max_height: Option<f64>,
    pub min_dimension: Option<f64>,
    pub max_bytes: Option<f64>,
    pub jpeg_quality: Option<f64>,
    pub exclude_webp: Option<bool>,
}
impl ImageResizeOptions {
    /// Fixed browser/PDF screenshot first stage; ordinary load is stage two.
    pub fn screenshot_stage_one() -> Self {
        Self {
            max_width: Some(1024.0),
            max_height: Some(1024.0),
            max_bytes: Some(150.0 * 1024.0),
            jpeg_quality: Some(70.0),
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResizedImage {
    pub buffer: Vec<u8>,
    pub mime_type: String,
    pub original_width: f64,
    pub original_height: f64,
    pub width: f64,
    pub height: f64,
    pub was_resized: bool,
    pub decode_failed: bool,
    pub data: String,
}

fn webp_excluded(options: &ImageResizeOptions, env: Option<&str>) -> bool {
    options.exclude_webp.unwrap_or_else(|| {
        env.is_some_and(|value| value.eq_ignore_ascii_case("1") || value.eq_ignore_ascii_case("true"))
    })
}
fn js_round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 { floor + 1.0 } else { floor }
}
fn js_min(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() { f64::NAN } else { left.min(right) }
}
fn js_max(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() { f64::NAN } else { left.max(right) }
}
fn cancel_check(cancel: &CancellationToken) -> Result<(), ToolError> {
    if cancel.is_cancelled() { Err(ToolError("Read image was aborted".into())) } else { Ok(()) }
}

/// Buffer.from(data,"base64") behavior at the resize boundary. Ingress has a
/// separate strict/canonical validator and cannot use this forgiving decoder.
fn node_base64_bytes(data: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut accumulator = 0_u32;
    let mut bits = 0;
    for byte in data.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        };
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }
    out
}

async fn encode_smallest(
    codec: &dyn ImageCodecPort,
    input: &[u8],
    width: f64,
    height: f64,
    quality: f64,
    lossy: bool,
    exclude: bool,
    cancel: &CancellationToken,
) -> Result<EncodedImage, ToolError> {
    cancel_check(cancel)?;
    let mut formats = if lossy { vec![ImageEncoding::Jpeg] } else { vec![ImageEncoding::Png, ImageEncoding::Jpeg] };
    if !exclude {
        formats.push(ImageEncoding::WebP);
    }
    let expected = formats.len();
    let candidates =
        codec.encode_many(input, ImageEncodeBatch { width, height, quality, formats }, cancel.clone()).await?;
    cancel_check(cancel)?;
    if candidates.len() != expected {
        return Err(ToolError("Host image codec returned an incomplete encoder batch".into()));
    }
    // reduce keeps the first on a tie, preserving PNG/JPEG/WebP ordering.
    candidates
        .into_iter()
        .reduce(|best, candidate| if candidate.buffer.len() < best.buffer.len() { candidate } else { best })
        .ok_or_else(|| ToolError("Host image codec returned no encoder candidate".into()))
}

fn encoded_result(image: EncodedImage, original: &DecodedImageMetadata, width: f64, height: f64) -> ResizedImage {
    let data = base64::engine::general_purpose::STANDARD.encode(&image.buffer);
    ResizedImage {
        buffer: image.buffer,
        mime_type: image.mime_type,
        original_width: original.width,
        original_height: original.height,
        width,
        height,
        was_resized: true,
        decode_failed: false,
        data,
    }
}

pub async fn resize_image(
    data: &str,
    mime_type: Option<&str>,
    options: &ImageResizeOptions,
    no_webp_env: Option<&str>,
    codec: &dyn ImageCodecPort,
    cancel: CancellationToken,
) -> Result<ResizedImage, ToolError> {
    let exclude = webp_excluded(options, no_webp_env);
    let input = node_base64_bytes(data);
    let attempt: Result<ResizedImage, ToolError> = async {
        cancel_check(&cancel)?;
        let original = codec.decoded_metadata(&input, cancel.clone()).await?;
        cancel_check(&cancel)?;
        let source_mime = original
            .format
            .as_ref()
            .filter(|format| !format.is_empty())
            .map(|format| format!("image/{format}"))
            .unwrap_or_else(|| mime_type.unwrap_or("").into());
        let max_width = options.max_width.unwrap_or(1568.0);
        let max_height = options.max_height.unwrap_or(1568.0);
        let max_bytes = options.max_bytes.unwrap_or(500.0 * 1024.0);
        let quality = options.jpeg_quality.unwrap_or(80.0);
        let floor = js_min(js_min(options.min_dimension.unwrap_or(200.0), max_width), max_height);
        if original.width >= floor
            && original.height >= floor
            && original.width <= max_width
            && original.height <= max_height
            && input.len() as f64 <= max_bytes / 4.0
            && !(exclude && source_mime == "image/webp")
        {
            return Ok(ResizedImage {
                buffer: input.clone(),
                mime_type: source_mime,
                original_width: original.width,
                original_height: original.height,
                width: original.width,
                height: original.height,
                was_resized: false,
                decode_failed: false,
                data: data.into(),
            });
        }
        let mut target_width = original.width;
        let mut target_height = original.height;
        if target_width > max_width {
            target_height = js_round(target_height * max_width / target_width);
            target_width = max_width;
        }
        if target_height > max_height {
            target_width = js_round(target_width * max_height / target_height);
            target_height = max_height;
        }
        if target_width < floor || target_height < floor {
            let short = js_min(target_width, target_height);
            let upscale = js_min(js_min(floor / short, max_width / target_width), max_height / target_height);
            if upscale > 1.0 {
                target_width = js_round(target_width * upscale);
                target_height = js_round(target_height * upscale);
            }
            target_width = js_min(max_width, js_max(floor, target_width));
            target_height = js_min(max_height, js_max(floor, target_height));
        }
        let mut final_width = target_width;
        let mut final_height = target_height;
        let mut best =
            encode_smallest(codec, &input, target_width, target_height, quality, false, exclude, &cancel).await?;
        if best.buffer.len() as f64 <= max_bytes {
            return Ok(encoded_result(best, &original, final_width, final_height));
        }
        for quality in [70.0, 60.0, 50.0, 40.0] {
            best = encode_smallest(codec, &input, target_width, target_height, quality, true, exclude, &cancel).await?;
            if best.buffer.len() as f64 <= max_bytes {
                return Ok(encoded_result(best, &original, final_width, final_height));
            }
        }
        for scale in [1.0, 0.75, 0.5, 0.35, 0.25] {
            final_width = js_round(target_width * scale);
            final_height = js_round(target_height * scale);
            // Fixed quirk: dimensions update before this break; best is the last
            // attempt, not a global minimum. Preserve both source behaviors.
            if final_width < 100.0 || final_height < 100.0 {
                break;
            }
            for quality in [70.0, 60.0, 50.0, 40.0] {
                best =
                    encode_smallest(codec, &input, final_width, final_height, quality, true, exclude, &cancel).await?;
                if best.buffer.len() as f64 <= max_bytes {
                    return Ok(encoded_result(best, &original, final_width, final_height));
                }
            }
        }
        Ok(encoded_result(best, &original, final_width, final_height))
    }
    .await;
    match attempt {
        Ok(result) => Ok(result),
        Err(error) => {
            cancel_check(&cancel)?;
            let header = resize_header_dimensions(&input);
            let fallback = mime_type
                .map(str::to_owned)
                .or_else(|| header.as_ref().map(|value| value.2.clone()))
                .unwrap_or_else(|| "application/octet-stream".into());
            if exclude && (fallback == "image/webp" || (mime_type.is_none_or(str::is_empty) && header.is_none())) {
                return Err(ToolError(
                    "resizeImage: failed to decode image and cannot honor excludeWebP for a WebP source".into(),
                ));
            }
            let (width, height) = header.map(|value| (value.0, value.1)).unwrap_or((0.0, 0.0));
            let _ = error;
            Ok(ResizedImage {
                buffer: input,
                mime_type: fallback,
                original_width: width,
                original_height: height,
                width,
                height,
                was_resized: false,
                decode_failed: true,
                data: data.into(),
            })
        }
    }
}

pub fn format_dimension_note(result: &ResizedImage) -> Option<String> {
    if !result.was_resized
        || [result.original_width, result.original_height, result.width, result.height]
            .iter()
            .any(|value| *value == 0.0 || value.is_nan())
        || (result.width == result.original_width && result.height == result.original_height)
    {
        return None;
    }
    let ratio = result.original_width / result.width;
    // Exact binary64 rational, choosing the larger decimal integer on a tie
    // like Number.toFixed(2). Dimensions supplied by the codec are raster sizes.
    let ratio_text = if ratio > 0.0 && ratio.is_finite() && ratio < 1e21 {
        let bits = ratio.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as i32;
        let mantissa = u128::from(bits & ((1_u64 << 52) - 1)) | if exponent == 0 { 0 } else { 1_u128 << 52 };
        let shift = if exponent == 0 { 1074 } else { 1075 - exponent };
        let scaled = mantissa * 100;
        let hundredths = if shift >= 128 {
            0
        } else if shift > 0 {
            let denominator = 1_u128 << shift;
            (scaled + denominator / 2) / denominator
        } else {
            scaled << (-shift)
        };
        format!("{}.{:02}", hundredths / 100, hundredths % 100)
    } else {
        ratio.to_string()
    };
    Some(format!(
        "[Image: original {}x{}, displayed at {}x{}. Multiply coordinates by {ratio_text} to map to original image.]",
        result.original_width, result.original_height, result.width, result.height
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageMetadata {
    pub mime_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub channels: Option<u8>,
    pub has_alpha: Option<bool>,
}
impl ImageMetadata {
    fn mime(mime: &str) -> Self {
        Self { mime_type: mime.into(), width: None, height: None, channels: None, has_alpha: None }
    }
}
fn be16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}
fn be32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn le16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}
fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn magic(bytes: &[u8], offset: usize, expected: &[u8]) -> bool {
    bytes.get(offset..offset + expected.len()) == Some(expected)
}

pub fn parse_image_metadata(header: &[u8]) -> Option<ImageMetadata> {
    if magic(header, 0, b"\x89PNG\r\n\x1a\n") {
        let mut meta = ImageMetadata::mime("image/png");
        if !magic(header, 12, b"IHDR") || header.len() < 26 {
            return Some(meta);
        }
        meta.width = Some(be32(header, 16));
        meta.height = Some(be32(header, 20));
        match header[25] {
            0 => {
                meta.channels = Some(1);
                meta.has_alpha = Some(false);
            }
            2 => {
                meta.channels = Some(3);
                meta.has_alpha = Some(false);
            }
            3 => meta.channels = Some(3),
            4 => {
                meta.channels = Some(2);
                meta.has_alpha = Some(true);
            }
            6 => {
                meta.channels = Some(4);
                meta.has_alpha = Some(true);
            }
            _ => {}
        }
        return Some(meta);
    }
    if magic(header, 0, b"\xff\xd8\xff") {
        let mut meta = ImageMetadata::mime("image/jpeg");
        let mut offset = 2;
        while offset + 9 < header.len() {
            if header[offset] != 0xff {
                offset += 1;
                continue;
            }
            let mut marker_offset = offset + 1;
            while marker_offset < header.len() && header[marker_offset] == 0xff {
                marker_offset += 1;
            }
            if marker_offset >= header.len() {
                break;
            }
            let marker = header[marker_offset];
            let segment = marker_offset + 1;
            if marker == 0xd8 || marker == 0xd9 || marker == 1 || (0xd0..=0xd7).contains(&marker) {
                offset = segment;
                continue;
            }
            if segment + 1 >= header.len() {
                break;
            }
            let length = usize::from(be16(header, segment));
            if length < 2 {
                break;
            }
            if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
                if segment + 7 >= header.len() {
                    break;
                }
                meta.width = Some(u32::from(be16(header, segment + 5)));
                meta.height = Some(u32::from(be16(header, segment + 3)));
                meta.channels = Some(header[segment + 7]);
                meta.has_alpha = Some(false);
                return Some(meta);
            }
            offset = segment + length;
        }
        return Some(meta);
    }
    if magic(header, 0, b"GIF87a") || magic(header, 0, b"GIF89a") {
        let mut meta = ImageMetadata::mime("image/gif");
        if header.len() >= 10 {
            meta.width = Some(u32::from(le16(header, 6)));
            meta.height = Some(u32::from(le16(header, 8)));
            meta.channels = Some(3);
        }
        return Some(meta);
    }
    if magic(header, 0, b"RIFF") && magic(header, 8, b"WEBP") {
        let mut meta = ImageMetadata::mime("image/webp");
        if header.len() < 30 {
            return Some(meta);
        }
        if magic(header, 12, b"VP8X") {
            let alpha = header[20] & 0x10 != 0;
            meta.has_alpha = Some(alpha);
            meta.channels = Some(if alpha { 4 } else { 3 });
            meta.width =
                Some((u32::from(header[24]) | (u32::from(header[25]) << 8) | (u32::from(header[26]) << 16)) + 1);
            meta.height =
                Some((u32::from(header[27]) | (u32::from(header[28]) << 8) | (u32::from(header[29]) << 16)) + 1);
        } else if magic(header, 12, b"VP8L") {
            let bits = le32(header, 21);
            let alpha = (bits >> 28) & 1 == 1;
            meta.width = Some((bits & 0x3fff) + 1);
            meta.height = Some(((bits >> 14) & 0x3fff) + 1);
            meta.channels = Some(if alpha { 4 } else { 3 });
            meta.has_alpha = Some(alpha);
        } else if magic(header, 12, b"VP8 ") {
            meta.width = Some(u32::from(le16(header, 26) & 0x3fff));
            meta.height = Some(u32::from(le16(header, 28) & 0x3fff));
            meta.channels = Some(3);
            meta.has_alpha = Some(false);
        }
        return Some(meta);
    }
    None
}

fn resize_header_dimensions(bytes: &[u8]) -> Option<(f64, f64, String)> {
    if bytes.len() >= 24 && magic(bytes, 0, b"\x89PNG\r\n\x1a\n") && be32(bytes, 8) == 13 && magic(bytes, 12, b"IHDR") {
        let width = be32(bytes, 16);
        let height = be32(bytes, 20);
        if width > 0 && height > 0 {
            return Some((f64::from(width), f64::from(height), "image/png".into()));
        }
    }
    if bytes.len() < 4 || !magic(bytes, 0, b"\xff\xd8") {
        return None;
    }
    let mut offset = 2;
    while offset + 3 < bytes.len() {
        if bytes[offset] != 0xff {
            offset += 1;
            continue;
        }
        while offset < bytes.len() && bytes[offset] == 0xff {
            offset += 1;
        }
        if offset >= bytes.len() {
            return None;
        }
        let marker = bytes[offset];
        offset += 1;
        if marker == 0xd9 || marker == 0xda {
            return None;
        }
        if marker == 1 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        if offset + 1 >= bytes.len() {
            return None;
        }
        let length = usize::from(be16(bytes, offset));
        if length < 2 {
            return None;
        }
        if (0xc0..=0xc3).contains(&marker)
            || (0xc5..=0xc7).contains(&marker)
            || (0xc9..=0xcb).contains(&marker)
            || (0xcd..=0xcf).contains(&marker)
        {
            if offset + 7 >= bytes.len() {
                return None;
            }
            let height = be16(bytes, offset + 3);
            let width = be16(bytes, offset + 5);
            if width == 0 || height == 0 {
                return None;
            }
            return Some((f64::from(width), f64::from(height), "image/jpeg".into()));
        }
        offset += length;
    }
    None
}

pub async fn read_image_metadata(path: &Path) -> Result<Option<ImageMetadata>, ToolError> {
    let file = tokio::fs::File::open(path).await.map_err(|error| ToolError(error.to_string()))?;
    let mut bytes = Vec::new();
    file.take(IMAGE_METADATA_HEADER_BYTES)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| ToolError(error.to_string()))?;
    Ok(parse_image_metadata(&bytes))
}

pub async fn image_decode_failure_reason(
    data: &str,
    mime_type: &str,
    codec: &dyn ImageCodecPort,
    cancel: CancellationToken,
) -> Result<Option<String>, ToolError> {
    let usable = data.trim_end_matches('=');
    let padding = data.len() - usable.len();
    if padding > 2 || !usable.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/') {
        return Ok(Some("invalid base64 image data".into()));
    }
    let bytes = node_base64_bytes(data);
    if bytes.is_empty() {
        return Ok(Some("empty image data".into()));
    }
    if base64::engine::general_purpose::STANDARD_NO_PAD.encode(&bytes) != usable {
        return Ok(Some("invalid base64 image data".into()));
    }
    if let Some(detected) = parse_image_metadata(&bytes)
        && detected.mime_type != mime_type.to_lowercase()
    {
        return Ok(Some(format!("declared {mime_type} but contains {}", detected.mime_type)));
    }
    cancel_check(&cancel)?;
    let result = codec.decode_probe_1x1(&bytes, cancel.clone()).await;
    cancel_check(&cancel)?;
    Ok(result.err().map(|error| error.0))
}

#[derive(Clone, Debug)]
pub struct LoadedImageInput {
    pub resolved_path: PathBuf,
    pub mime_type: String,
    pub data: String,
    pub text_note: String,
    pub dimension_note: Option<String>,
    pub bytes: usize,
}
fn size_error(bytes: u64, maximum: u64) -> ToolError {
    ToolError(format!(
        "Image file too large: {} exceeds {} limit.",
        crate::read::format_bytes(bytes),
        crate::read::format_bytes(maximum)
    ))
}

pub async fn load_in_memory_image(
    data: &str,
    mime_type: &str,
    resolved_path: &Path,
    text_prefix: &str,
    auto_resize: bool,
    maximum: u64,
    exclude_webp: Option<bool>,
    no_webp_env: Option<&str>,
    codec: &dyn ImageCodecPort,
    cancel: CancellationToken,
) -> Result<LoadedImageInput, ToolError> {
    let usable = data.len()
        - if data.ends_with("==") {
            2
        } else if data.ends_with('=') {
            1
        } else {
            0
        };
    let input_bytes = usable * 3 / 4;
    if input_bytes as u64 > maximum {
        return Err(size_error(input_bytes as u64, maximum));
    }
    if let Some(reason) = image_decode_failure_reason(data, mime_type, codec, cancel.clone()).await? {
        return Err(ToolError(format!("{} is not a decodable {mime_type} image: {reason}", resolved_path.display())));
    }
    let mut output_data = data.to_owned();
    let mut output_mime = mime_type.to_owned();
    let mut output_bytes = input_bytes;
    let mut dimension_note = None;
    if auto_resize || (exclude_webp == Some(true) && mime_type == "image/webp") {
        match resize_image(
            data,
            Some(mime_type),
            &ImageResizeOptions { exclude_webp, ..Default::default() },
            no_webp_env,
            codec,
            cancel.clone(),
        )
        .await
        {
            Ok(resized) => {
                dimension_note = format_dimension_note(&resized);
                output_data = resized.data;
                output_mime = resized.mime_type;
                output_bytes = resized.buffer.len();
            }
            Err(_) => {
                cancel_check(&cancel)?;
            } // Fixed load layer preserves original on resize failure.
        }
    }
    let mut note = format!("{text_prefix} [{output_mime}]");
    if let Some(dimension) = &dimension_note {
        note.push('\n');
        note.push_str(dimension);
    }
    Ok(LoadedImageInput {
        resolved_path: resolved_path.into(),
        mime_type: output_mime,
        data: output_data,
        text_note: note,
        dimension_note,
        bytes: output_bytes,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadImageKind {
    Raster,
    Svg,
}
#[derive(Clone, Debug)]
pub struct ReadImageOutput {
    pub content: Vec<UserBlock>,
    pub source_path: PathBuf,
    pub content_type: String,
    pub file_size: u64,
}

pub async fn load_read_image(
    path: &Path,
    kind: ReadImageKind,
    detected_mime: Option<&str>,
    inspect_path: &str,
    auto_resize: bool,
    policy: Option<&dyn ReadImagePolicyPort>,
    codec: Option<&dyn ImageCodecPort>,
    cancel: CancellationToken,
) -> Result<Option<ReadImageOutput>, ToolError> {
    let metadata = if kind == ReadImageKind::Svg { None } else { read_image_metadata(path).await? };
    let mime = metadata
        .as_ref()
        .map(|meta| meta.mime_type.as_str())
        .or(detected_mime)
        .or(if kind == ReadImageKind::Svg { Some("image/svg+xml") } else { None });
    let Some(mime) = mime else {
        return Ok(None);
    };
    let file_size = tokio::fs::metadata(path).await.map_err(|error| ToolError(error.to_string()))?.len();
    let state = policy.map(|policy| policy.current_state()).unwrap_or_default();
    // This branch precedes the ingress cap, full read and all decode/codec work.
    if state.inspect_image_active {
        let dimensions = metadata
            .as_ref()
            .and_then(|meta| Some(format!("{}x{}", meta.width?, meta.height?)))
            .unwrap_or_else(|| "unknown".into());
        let channels = metadata
            .as_ref()
            .and_then(|meta| meta.channels)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".into());
        let alpha = match metadata.as_ref().and_then(|meta| meta.has_alpha) {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        };
        let text = format!(
            "Image metadata:\n- MIME: {mime}\n- Bytes: {file_size} ({})\n- Dimensions: {dimensions}\n- Channels: {channels}\n- Alpha: {alpha}\n\nIf you want to analyze the image, call inspect_image with path=\"{inspect_path}\" and a question describing what to inspect and the desired output format.",
            crate::read::format_bytes(file_size)
        );
        return Ok(Some(ReadImageOutput {
            content: vec![UserBlock::text(text)],
            source_path: path.into(),
            content_type: mime.into(),
            file_size,
        }));
    }
    if file_size > MAX_IMAGE_INPUT_BYTES {
        return Err(size_error(file_size, MAX_IMAGE_INPUT_BYTES));
    }
    cancel_check(&cancel)?;
    let bytes = tokio::fs::read(path).await.map_err(|error| ToolError(error.to_string()))?;
    if bytes.len() as u64 > MAX_IMAGE_INPUT_BYTES {
        return Err(size_error(bytes.len() as u64, MAX_IMAGE_INPUT_BYTES));
    }
    cancel_check(&cancel)?;
    let (bytes, input_mime, prefix) = if kind == ReadImageKind::Svg {
        let work = cancel.child_token();
        let _stop_on_drop = work.clone().drop_guard();
        // This bounded native codec is not interruptible while rasterizing.
        // Dropping/cancelling the waiter marks its child; discard late output.
        let job = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, ToolError> {
            cancel_check(&work)?;
            let png = crate::read_svg_native::rasterize_svg(&bytes, 2048, 2048)
                .map_err(|error| ToolError(format!("Could not rasterize SVG: {}", error.0)))?;
            cancel_check(&work)?;
            Ok(png)
        });
        let png = tokio::select! {
            result = job => result.map_err(|error| ToolError(format!("Rasterize SVG failed: {error}")))??,
            _ = cancel.cancelled() => return Err(ToolError("Rasterize SVG was aborted".into())),
        };
        cancel_check(&cancel)?;
        (png, "image/png", "Read SVG file")
    } else {
        (bytes, mime, "Read image file")
    };
    let codec = codec.ok_or_else(|| ToolError("Host Bun.Image codec is not installed".into()))?;
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    let loaded = load_in_memory_image(
        &data,
        input_mime,
        path,
        prefix,
        auto_resize,
        MAX_IMAGE_INPUT_BYTES,
        state.model_excludes_webp.then_some(true),
        state.no_webp_env.as_deref(),
        codec,
        cancel,
    )
    .await?;
    Ok(Some(ReadImageOutput {
        content: vec![
            UserBlock::text(loaded.text_note),
            UserBlock::Image(ImageContent {
                detail: None,
                compaction_frame: false,
                data: loaded.data,
                mime_type: loaded.mime_type.clone(),
            }),
        ],
        source_path: loaded.resolved_path,
        content_type: loaded.mime_type,
        file_size,
    }))
}
