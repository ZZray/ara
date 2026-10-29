//! Fixed checksums.test.ts original wire values and chained seeds.
//! MIT upstream notices apply.
//! Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
#[test]
fn crc32_ieee_value_seed_chain_empty() {
    let bytes = b"123456789";
    assert_eq!(crc32fast::hash(bytes), 0xcbf43926);
    let mut chained = crc32fast::Hasher::new_with_initial(crc32fast::hash(&bytes[..4]));
    chained.update(&bytes[4..]);
    assert_eq!(chained.finalize(), 0xcbf43926);
    assert_eq!(crc32fast::hash(&[]), 0);
}
#[test]
fn crc64_xz_value_seed_chain_empty() {
    let bytes = b"123456789";
    assert_eq!(codecs::read_crc64(bytes, 0), 0x995dc9bbdf1939fa);
    assert_eq!(codecs::read_crc64(&bytes[4..], codecs::read_crc64(&bytes[..4], 0)), 0x995dc9bbdf1939fa);
    assert_eq!(codecs::read_crc64(&[], 0), 0);
}
#[test]
fn crc16_arc_value_seed_chain_empty() {
    let bytes = b"123456789";
    assert_eq!(lzh::crc16(bytes, 0), 0xbb3d);
    assert_eq!(lzh::crc16(&bytes[4..], lzh::crc16(&bytes[..4], 0)), 0xbb3d);
    assert_eq!(lzh::crc16(&[], 0), 0);
}
