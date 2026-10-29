//! Fixed codecs.test.ts original assertions; MIT upstream notices apply.
//! Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
fn codec_fixture(name: &str) -> Vec<u8> {
    fixture(&format!("codecs/{name}"))
}
fn bzip_sniff(bytes: &[u8]) -> bool {
    registry::sniff_archive_format(bytes) == Some(ReadFormat::TarBz2)
}
#[test]
fn bzip_multi_blocks_levels_one_and_nine() {
    for (name, size, expected) in [
        ("bzip-level-1.txt.bz2", 813_520, "81e63e3c3942b040fbcd62e0dcfcccfa97c95ede85c0d9df1e60d9452574728b"),
        ("bzip-level-9.txt.bz2", 1_829_150, "63d3996fced6e1df4e9346ba5378620800a95ce77d51b5088863db96675a0912"),
    ] {
        let bytes = codec_fixture(name);
        assert!(bzip_sniff(&bytes));
        let output = codecs::read_bzip2(&bytes, size).unwrap();
        assert_eq!(output.len() as u64, size);
        assert_eq!(hash(&output), expected);
    }
}
#[test]
fn bzip_concatenated_streams() {
    let output = codecs::read_bzip2(&codec_fixture("bzip-concatenated.bz2"), 71).unwrap();
    assert_eq!(output, b"first concatenated stream\nsecond concatenated stream\nwith another line\n");
    assert_eq!(hash(&output), "47f7efe1fd83980f616b5e824ce079c39d83d70898a1acc7d3fa9e649d1c3cfd");
}
#[test]
fn bzip_truncated_block_crc_output_limit_sniff() {
    let bytes = codec_fixture("bzip-level-1.txt.bz2");
    error_contains(codecs::read_bzip2(&bytes[..bytes.len() - 1], 1_000_000), "Truncated bzip2 stream");
    let mut corrupt = bytes.clone();
    corrupt[10] ^= 1;
    error_contains(codecs::read_bzip2(&corrupt, 1_000_000), "Bzip2 block CRC mismatch");
    error_contains(codecs::read_bzip2(&bytes, 1000), "Bzip2 output exceeds the 1000-byte limit");
    assert!(!bzip_sniff(&[0x42, 0x5a, 0x68]));
    assert!(!bzip_sniff(&[0x42, 0x5a, 0x68, 0x30]));
}
#[test]
fn lzw_sixteen_bit_block_and_clear_group() {
    let bytes = codec_fixture("ncompress.bin.Z");
    assert!(lzw::is_compress_z(&bytes));
    let output = lzw::decompress(&bytes, 180_000).unwrap();
    assert_eq!(output.len(), 180_000);
    assert_eq!(hash(&output), "35b19599038e534308e21e6c57cb60953ea6fb2e559ced2f870f76c3a16b2dc6");
    assert_eq!(lzw::decompress(&[0x1f, 0x9d, 0x89, 0x41, 0, 2, 0, 0, 0, 0, 0, 0, 0x42, 0], 2).unwrap(), b"AB");
}
#[test]
fn lzw_header_flags_corrupt_codes_truncation_output_limit_sniff() {
    error_contains(lzw::decompress(&[0x1f, 0x9d], 100), "Truncated compress (.Z) header");
    error_contains(lzw::decompress(&[0x1f, 0x9d, 0xa9], 100), "Unsupported compress (.Z) header flags");
    error_contains(lzw::decompress(&[0x1f, 0x9d, 0x90, 1, 1], 100), "dictionary code 257");
    let bytes = codec_fixture("ncompress.bin.Z");
    error_contains(lzw::decompress(&bytes[..bytes.len() - 1], 200_000), "padding");
    error_contains(lzw::decompress(&bytes, 1024), "Compress (.Z) output exceeds the 1024-byte limit");
    assert!(!lzw::is_compress_z(&[0x1f]));
    assert!(lzw::is_compress_z(&[0x1f, 0x9d]));
    assert!(!lzw::is_compress_z(&[0x1f, 0x8b]));
}
