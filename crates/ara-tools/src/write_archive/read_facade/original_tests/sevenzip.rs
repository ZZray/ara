//! Fixed sevenzip.test.ts original assertions; MIT upstream notices apply.
//! Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
const MESSAGE: &str = "5ef6774c4d7f61a40b8ff89151687d63129888e89efffe183d0de94da95dfd8c";
#[test]
fn metadata_nested_empty_and_sniff() {
    let bytes = fixture("sevenzip-default.7z");
    assert!(sevenzip::sniff(&bytes));
    assert!(!sevenzip::sniff(&[0x37, 0x7a]));
    let i = index(bytes, ReadFormat::SevenZip);
    assert_eq!(
        i.entries.iter().map(|e| (utf8(&e.path), e.directory)).collect::<Vec<_>>(),
        [("nested".into(), true), ("empty.txt".into(), false), ("nested/message.txt".into(), false)]
    );
    assert_eq!(hash(&index_member(&i, "nested/message.txt")), MESSAGE);
    assert!(index_member(&i, "empty.txt").is_empty());
    assert!(entry(&i, "nested/message.txt").mtime.unwrap() > 1_700_000_000_000.0);
    assert_eq!(entry(&i, "nested/message.txt").mode.unwrap() & 0o170000, 0o100000);
    assert_eq!(entry(&i, "nested").mode.unwrap() & 0o170000, 0o040000);
}
#[test]
fn copy_lzma_lzma2_delta_bcj_byte_identical() {
    for (name, path, expected) in [
        ("sevenzip-copy.7z", "nested/message.txt", MESSAGE),
        ("sevenzip-lzma.7z", "nested/message.txt", MESSAGE),
        ("sevenzip-default.7z", "nested/message.txt", MESSAGE),
        ("sevenzip-delta.7z", "delta.bin", "0b74a8aff5d7381fa418fe1dcf8b84de32344acc6bea8f1ed3dac26754ba281c"),
        ("sevenzip-bcj.7z", "x86.bin", "3ecc97b751572f0c2078683f95b56e0468b5d75a581531b6dee209727c3d2f48"),
    ] {
        assert_eq!(hash(&index_member(&index(fixture(name), ReadFormat::SevenZip), path)), expected, "{name}");
    }
}
#[test]
fn solid_out_of_order_one_folder_fetch() {
    let (source, reads) = counting(fixture("sevenzip-solid.7z"));
    let i = registry::read_index(source, ReadFormat::SevenZip, None, ReadLimits::default(), utc_date).unwrap();
    let baseline = reads.lock().unwrap().len();
    assert_eq!(hash(&index_member(&i, "nested/message.txt")), MESSAGE);
    assert_eq!(reads.lock().unwrap().len(), baseline + 1);
    assert_eq!(
        hash(&index_member(&i, "alpha.txt")),
        "a5db0353cdd8fda86cfc74087d4e52b2826e29f72b2bcc2485fa977947a8d8f1"
    );
    assert_eq!(hash(&index_member(&i, "beta.txt")), "5e78ec08a413b9961fae10bf2220968746fa770c80299f66d74a158bf5b6ac84");
    assert_eq!(reads.lock().unwrap().len(), baseline + 1);
}
#[test]
fn symlinks_and_traversal_stream_mapping() {
    let i = index(fixture("sevenzip-symlink.7z"), ReadFormat::SevenZip);
    assert_eq!(entry(&i, "link-to-message").mode.unwrap() & 0o170000, 0o120000);
    assert_link(&i, "link-to-message", "nested/message.txt", true);
    assert!(index(fixture("sevenzip-traversal.7z"), ReadFormat::SevenZip).entries.is_empty());
}
#[test]
fn encrypted_ppmd_truncation_corrupt_payload_and_limit() {
    error_contains(
        index_limits(fixture("sevenzip-encrypted.7z"), ReadFormat::SevenZip, ReadLimits::default()),
        "7zAES",
    );
    error_contains(
        index(fixture("sevenzip-ppmd.7z"), ReadFormat::SevenZip).read_file(&"nested/message.txt".into()),
        "PPMd",
    );
    let original = fixture("sevenzip-copy.7z");
    assert!(index_limits(original[..20].to_vec(), ReadFormat::SevenZip, ReadLimits::default()).is_err());
    let mut corrupt = original.clone();
    corrupt[32] ^= 0x80;
    assert!(index(corrupt, ReadFormat::SevenZip).read_file(&"nested/message.txt".into()).is_err());
    error_contains(
        index_limits(original, ReadFormat::SevenZip, ReadLimits { max_member: 1, ..ReadLimits::default() }),
        "too large",
    );
}
