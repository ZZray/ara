//! Fixed legacy-dos.test.ts original assertions; MIT upstream notices apply.
//! Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
const REPEAT: &str = "6c569d00a8d8d31338d7f1ac134f0d6cbd014446568063ac0b919ab042e0dcf5";
#[test]
fn lzh_sniff_levels_methods_and_nested_index() {
    assert!(lzh::sniff(&fixture("lzh-level0-lh5.lzh")));
    assert!(!lzh::sniff(&[0; 64]));
    for name in ["lzh-level0-lh5.lzh", "lzh-level1-lh6.lzh", "lzh-level2-lh7.lzh"] {
        let i = index(fixture(name), ReadFormat::Lzh);
        assert_eq!(paths(&i), ["hello.txt", "nested", "nested/mode.sh", "nested/repeat.txt"]);
        assert!(entry(&i, "nested").directory);
        assert_eq!(i.entries[0].mtime, Some(1_700_000_000_000.0));
        assert_eq!(hash(&index_member(&i, "nested/repeat.txt")), REPEAT, "{name}");
    }
    for name in ["lzh-level0-lh4.lzh", "lzh-level0-lh5.lzh", "lzh-level1-lh6.lzh", "lzh-level2-lh7.lzh"] {
        assert_eq!(hash(&index_member(&index(fixture(name), ReadFormat::Lzh), "nested/repeat.txt")), REPEAT, "{name}");
    }
}
#[test]
fn lzh_stored_legacy_unicode_and_traversal() {
    for (name, path) in [("lzh-lz4.lzh", "legacy/lz4.txt"), ("lzh-lzs.lzh", "legacy/lzs.txt")] {
        assert_eq!(index_member(&index(fixture(name), ReadFormat::Lzh), path), b"LArc legacy stream\n");
    }
    let i = index(fixture("lzh-unicode-level2.lzh"), ReadFormat::Lzh);
    assert_eq!(i.entries.len(), 1);
    let e = entry(&i, "unicode/雪.txt");
    assert_eq!(e.mode, Some(0x81a4));
    assert_eq!(e.mtime, Some(1_700_000_000_000.0));
    assert_eq!(index_member(&i, "unicode/雪.txt"), b"Unicode LZH path\n");
    assert!(index(fixture("lzh-traversal.lzh"), ReadFormat::Lzh).entries.is_empty());
}
#[test]
fn lzh_dynamic_huffman_truncated_header_member_crc() {
    error_contains(
        index(fixture("lzh-unsupported-lh1.lzh"), ReadFormat::Lzh).read_file(&"unsupported.txt".into()),
        "unsupported dynamic-Huffman method -lh1-",
    );
    let original = fixture("lzh-level2-stored.lzh");
    assert!(index_limits(original[..12].to_vec(), ReadFormat::Lzh, ReadLimits::default()).is_err());
    let mut corrupt = original;
    let n = corrupt.len();
    corrupt[n - 2] ^= 0x80;
    error_contains(index(corrupt, ReadFormat::Lzh).read_file(&"nested/repeat.txt".into()), "CRC-16 verification");
}
#[test]
fn arj_sniff_and_all_five_methods() {
    assert!(arj::sniff(&fixture("arj-method1.arj")));
    assert!(!arj::sniff(&[0; 64]));
    for method in 0..=4 {
        let name = format!("arj-method{method}.arj");
        let i = index(fixture(&name), ReadFormat::Arj);
        assert_eq!(paths(&i), ["hello.txt", "nested/mode.sh", "nested/repeat.txt"]);
        assert_eq!(hash(&index_member(&i, "nested/repeat.txt")), REPEAT, "{name}");
    }
}
#[test]
fn arj_metadata_unsupported_features_methods_and_traversal() {
    let i = index(fixture("arj-method1.arj"), ReadFormat::Arj);
    assert_eq!(entry(&i, "hello.txt").mode, Some(0x81a4));
    assert_eq!(entry(&i, "hello.txt").mtime, Some(1_700_000_000_000.0));
    assert_eq!(entry(&i, "nested/mode.sh").mode, Some(0x81ed));
    for (name, message) in [
        ("arj-encrypted.arj", "Encrypted ARJ members are unsupported"),
        ("arj-multivolume.arj", "Multi-volume ARJ members are unsupported"),
    ] {
        error_contains(index_limits(fixture(name), ReadFormat::Arj, ReadLimits::default()), message);
    }
    error_contains(
        index(fixture("arj-unsupported.arj"), ReadFormat::Arj).read_file(&"hello.txt".into()),
        "unsupported compression method 7",
    );
    assert_eq!(paths(&index(fixture("arj-traversal.arj"), ReadFormat::Arj)), ["nested/mode.sh", "nested/repeat.txt"]);
}
#[test]
fn arj_truncated_and_header_crc() {
    let original = fixture("arj-method1.arj");
    assert!(index_limits(original[..20].to_vec(), ReadFormat::Arj, ReadLimits::default()).is_err());
    let mut corrupt = original;
    corrupt[12] ^= 1;
    error_contains(index_limits(corrupt, ReadFormat::Arj, ReadLimits::default()), "archive header");
}
