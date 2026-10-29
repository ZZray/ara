//! Fixed xz.test.ts original assertions; MIT upstream notices apply.
//! Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
const TEXT: &str = "ee8e31cbfe3ffd471a71273fc17c68c50db0aa63d48d6b6b294f5560bfc08c2a";
#[test]
fn sniff_check_types_and_multiblock() {
    for check in ["crc32", "crc64", "sha256"] {
        let bytes = fixture(&format!("xz-{check}.xz"));
        assert_eq!(registry::sniff_archive_format(&bytes), Some(ReadFormat::TarXz));
        assert_eq!(hash(&codecs::read_xz(&bytes, 4096).unwrap()), TEXT);
    }
    assert_ne!(registry::sniff_archive_format(&[0xfd, 0x37]), Some(ReadFormat::TarXz));
    assert_eq!(hash(&codecs::read_xz(&fixture("xz-multiblock.xz"), 4096).unwrap()), TEXT);
}
#[test]
fn delta_bcj_and_unknown_size_lzma_alone() {
    for (name, expected) in [
        ("xz-delta.xz", "0b74a8aff5d7381fa418fe1dcf8b84de32344acc6bea8f1ed3dac26754ba281c"),
        ("xz-x86.xz", "3ecc97b751572f0c2078683f95b56e0468b5d75a581531b6dee209727c3d2f48"),
    ] {
        assert_eq!(hash(&codecs::read_xz(&fixture(name), 4096).unwrap()), expected);
    }
    let bytes = fixture("lzma-alone.lzma");
    assert_eq!(&bytes[5..13], &[0xff; 8]);
    assert_eq!(hash(&codecs::read_lzma_alone(&bytes, 215).unwrap()), TEXT);
    error_contains(codecs::read_lzma_alone(&bytes, 10), "size limit");
}
#[test]
fn corrupt_checks_truncation_terminal_filter_and_output_overflow() {
    let original = fixture("xz-crc32.xz");
    let mut corrupt = original.clone();
    let footer = corrupt.len() - 12;
    let index_size = (read32(&corrupt, footer + 4) as usize + 1) * 4;
    corrupt[footer - index_size - 1] ^= 1;
    error_contains(codecs::read_xz(&corrupt, 4096), "CRC32 mismatch");
    assert!(codecs::read_xz(&original[..original.len() - 4], 4096).is_err());
    error_contains(codecs::read_xz(&original, 10), "size limit");
    let mut unsupported = original;
    let header_end = 12 + (unsupported[12] as usize + 1) * 4;
    let id = (14..header_end - 4).find(|&i| unsupported[i] == 0x21).unwrap();
    unsupported[id] = 0x22;
    let crc = crc32fast::hash(&unsupported[12..header_end - 4]);
    write32(&mut unsupported, header_end - 4, crc);
    error_contains(codecs::read_xz(&unsupported, 4096), "terminal filter ID 0x22");
}
