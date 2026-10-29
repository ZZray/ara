//! Fixed cpio.test.ts original assertions; MIT upstream notices apply.
//! Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
fn root_member(i: &Index) -> &str {
    if matches!(entry(i, "root.txt").storage, Some(Storage::ReadMember(_))) { "root.txt" } else { "hard.txt" }
}
#[test]
fn five_variants_metadata_links_and_bytes() {
    for name in ["cpio-newc.cpio", "cpio-crc.cpio", "cpio-odc.cpio", "cpio-bin-le.cpio", "cpio-bin-be.cpio"] {
        let bytes = fixture(name);
        assert!(cpio::sniff(&bytes));
        let i = index(bytes, ReadFormat::Cpio);
        for p in ["root.txt", "hard.txt", "dir", "dir/file.txt", "dir/link"] {
            assert!(i.get(&p.into()).is_some(), "{name}:{p}");
        }
        assert!(entry(&i, "dir").directory);
        assert_eq!(entry(&i, "dir").mode, Some(0o040750));
        assert_eq!(entry(&i, "dir/file.txt").mode, Some(0o100644));
        assert_eq!(entry(&i, "dir/file.txt").mtime, Some(1_700_000_000_000.0));
        assert_eq!(index_member(&i, "dir/file.txt"), b"nested payload\n");
        let p = root_member(&i);
        let link = if p == "root.txt" { "hard.txt" } else { "root.txt" };
        assert_eq!(index_member(&i, p), b"root payload\n");
        assert_link(&i, link, p, false);
        assert_eq!(entry(&i, link).size, 13);
        assert_eq!(entry(&i, "dir/link").mode, Some(0o120755));
        assert_link(&i, "dir/link", "root.txt", true);
    }
}
#[test]
fn bsdtar_portable_and_special_nodes() {
    let i = index(fixture("cpio-bsdtar.cpio"), ReadFormat::Cpio);
    assert_eq!(index_member(&i, "dir/file.txt"), b"nested payload\n");
    assert_link(&i, "dir/link", "root.txt", true);
    let i = index(fixture("cpio-specials.cpio"), ReadFormat::Cpio);
    for p in ["pipe", "tty", "socket"] {
        assert!(i.get(&p.into()).is_none());
    }
    assert_eq!(index_member(&i, "kept.txt"), b"kept\n");
}
#[test]
fn selected_crc_trailer_truncation_and_traversal() {
    let mut bytes = fixture("cpio-crc.cpio");
    let offset = find_bytes(&bytes, b"nested payload\n");
    assert!(offset > 0);
    bytes[offset] ^= 0x20;
    error_contains(index(bytes, ReadFormat::Cpio).read_file(&"dir/file.txt".into()), "invalid CRC checksum");
    let bytes = fixture("cpio-newc.cpio");
    assert!(index_limits(bytes[..200].to_vec(), ReadFormat::Cpio, ReadLimits::default()).is_err());
    let trailer = find_bytes(&bytes, b"TRAILER!!!");
    assert!(trailer > 110);
    error_contains(
        index_limits(bytes[..trailer - 110].to_vec(), ReadFormat::Cpio, ReadLimits::default()),
        "missing TRAILER!!!",
    );
    let i = index(fixture("cpio-traversal.cpio"), ReadFormat::Cpio);
    for p in ["../escape.txt", "escape.txt"] {
        assert!(i.get(&p.into()).is_none());
    }
    assert_eq!(index_member(&i, "safe.txt"), b"safe\n");
}
#[test]
fn unsupported_wire_magic() {
    let mut bytes = fixture("cpio-newc.cpio");
    bytes[..6].copy_from_slice(b"070799");
    assert!(!cpio::sniff(&bytes));
    error_contains(index_limits(bytes, ReadFormat::Cpio, ReadLimits::default()), "unsupported or corrupt magic");
}
#[test]
fn rpm_gzip_headers_and_all_compressors() {
    let bytes = fixture("minimal-gzip.rpm");
    assert!(rpm::sniff(&bytes));
    let i = index(bytes, ReadFormat::Rpm);
    assert!(i.get(&"./root.txt".into()).is_none());
    assert!(i.get(&"root.txt".into()).is_some());
    assert_eq!(index_member(&i, "dir/file.txt"), b"nested payload\n");
    assert_link(&i, "dir/link", "root.txt", true);
    for compressor in ["bzip2", "xz", "zstd", "lzma"] {
        let name = format!("minimal-{compressor}.rpm");
        let i = index(fixture(&name), ReadFormat::Rpm);
        assert_eq!(index_member(&i, root_member(&i)), b"root payload\n", "{name}");
    }
}
#[test]
fn rpm_unknown_lzma_size_and_magic_fallback() {
    let i = index(fixture("minimal-lzma-unknown.rpm"), ReadFormat::Rpm);
    assert_eq!(index_member(&i, "dir/file.txt"), b"nested payload\n");
    let mut bytes = fixture("minimal-gzip.rpm");
    let offset = find_bytes(&bytes, b"gzip\0");
    assert!(offset > 0);
    bytes[offset..offset + 4].copy_from_slice(b"xxxx");
    let i = index(bytes, ReadFormat::Rpm);
    assert_eq!(index_member(&i, root_member(&i)), b"root payload\n");
}
#[test]
fn rpm_package_identity_unsupported_format_truncation_compressor() {
    let original = fixture("minimal-gzip.rpm");
    let mut bytes = original.clone();
    let offset = find_bytes(&bytes, b"cpio\0");
    assert!(offset > 0);
    bytes[offset..offset + 4].copy_from_slice(b"drpm");
    error_contains(
        index_limits(bytes, ReadFormat::Rpm, ReadLimits::default()),
        "RPM package 'fixture-1.0' uses unsupported payload format 'drpm'",
    );
    assert!(index_limits(original[..120].to_vec(), ReadFormat::Rpm, ReadLimits::default()).is_err());
    let mut bytes = original;
    let offset = find_bytes(&bytes, b"gzip\0");
    bytes[offset..offset + 4].copy_from_slice(b"xxxx");
    let n = bytes.len();
    bytes[n - 1] = 0;
    if let Some(p) = bytes.iter().rposition(|b| *b == 0x1f) {
        if bytes.get(p + 1) == Some(&0x8b) {
            bytes[p] = 0;
        }
    }
    error_contains(
        index_limits(bytes, ReadFormat::Rpm, ReadLimits::default()),
        "unsupported payload compressor 'xxxx'",
    );
}
