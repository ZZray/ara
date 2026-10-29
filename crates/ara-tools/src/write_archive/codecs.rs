//! Fixed OMP archive codec admission wrappers; MIT, see THIRD_PARTY_NOTICES.md.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc.
use super::*;
use std::io::{Cursor, Write};
mod lzma;
mod xz;
mod bzip2_read;
fn bounded(mut reader: impl Read, max: u64, label: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    reader.by_ref().take(max.saturating_add(1)).read_to_end(&mut out).map_err(io_error)?;
    if out.len() as u64 > max {
        return Err(error(format!("{label} output exceeds its size limit")));
    }
    Ok(out)
}
pub(super) fn gunzip(input: &[u8], max: u64) -> Result<Vec<u8>> {
    bounded(flate2::read::MultiGzDecoder::new(input), max.max(1), "gzip")
}
pub(super) fn unzstd(input: &[u8], max: u64) -> Result<Vec<u8>> {
    bounded(zstd::stream::read::Decoder::new(input).map_err(io_error)?, max.max(1), "zstd")
}
pub(super) fn gzip(input: &[u8]) -> Result<Vec<u8>> {
    let mut out = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    out.write_all(input).map_err(io_error)?;
    out.finish().map_err(io_error)
}
pub(super) fn zstd(input: &[u8]) -> Result<Vec<u8>> {
    zstd::stream::encode_all(input, 0).map_err(io_error)
}
pub(super) fn deflate(input: &[u8]) -> Result<Vec<u8>> {
    let mut out = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    out.write_all(input).map_err(io_error)?;
    out.finish().map_err(io_error)
}
pub(super) fn zip(input: &[u8], method: u16, size: u64, path: &WireString) -> Result<Vec<u8>> {
    match method {
        0 => Ok(input.to_vec()),
        8 => bounded(flate2::read::DeflateDecoder::new(input), size.max(1), "ZIP")
            .map_err(|e| error(format!("Failed to decompress ZIP member '{}': {}", utf8(path), e.0))),
        12 => bounded(bzip2::read::MultiBzDecoder::new(input), size, "bzip2"),
        14 => {
            if input.len() < 9 || input[2] != 5 || input[3] != 0 {
                return Err(error(format!("Invalid ZIP archive: malformed LZMA properties for '{}'", utf8(path))));
            }
            lzma::decode(&input[4..9], &input[9..], size)
        }
        20 | 93 => unzstd(input, size),
        95 => xz::decode(input, size),
        _ => Err(error(format!("Unsupported ZIP compression method {method} for '{}'", utf8(path)))),
    }
}

// Private READ admission; all pre-existing WRITE/ZIP codec paths remain unchanged.
pub(super) fn read_bzip2(input: &[u8], max: u64) -> Result<Vec<u8>> {
    bzip2_read::decode(input, max)
}
pub(super) fn read_crc64(input: &[u8], seed: u64) -> u64 {
    xz::crc64(input, seed)
}
pub(super) fn read_lzma_alone(input: &[u8], max: u64) -> Result<Vec<u8>> {
    lzma::decode_alone(input, max)
}
pub(super) fn read_lzma_raw(properties: &[u8], input: &[u8], size: u64) -> Result<Vec<u8>> {
    if size > JS_SAFE { return Err(error("Invalid LZMA output size")); }
    lzma::decode(properties, input, size)
}
pub(super) fn read_lzma2(property: u8, input: &[u8], max: u64) -> Result<Vec<u8>> {
    lzma::decode_lzma2(property, input, max)
}
pub(super) fn read_xz(input: &[u8], max: u64) -> Result<Vec<u8>> {
    if max > JS_SAFE { return Err(error("Invalid XZ output limit")); }
    xz::decode(input, max)
}
