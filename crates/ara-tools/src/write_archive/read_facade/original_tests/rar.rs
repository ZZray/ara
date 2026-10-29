//! Fixed rar.test.ts original assertions; MIT upstream notices apply.
//! Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
const HASHES: [(&str, &str); 4] = [
    ("input/hello.txt", "7f1620bec2523375e14494eade9ebdc362926b11904813809c503f2eb691aeff"),
    ("input/nested/data.bin", "10fc3c51a152e90e5b90319b601d92ccf37290ef53c35ff92507687d8a911a08"),
    ("input/naive-☃.txt", "1dcc4cea49428fe3a0d2df11ba03ed049e3860511144de489f2e5a0cc53c989d"),
    ("input/x86.bin", "2aad26e4b16a8535154aa7948ed00398f04104a65b1dfe34e89bd64235b6999d"),
];
fn expected(path: &str) -> &'static str {
    HASHES.iter().find(|(p, _)| *p == path).unwrap().1
}
fn check_all(i: &Index) {
    for e in &i.entries {
        assert_eq!(hash(&index_member(i, &utf8(&e.path))), expected(&utf8(&e.path)));
    }
}
#[test]
fn stored_rar5_rar4_metadata_and_hashes() {
    for name in ["rar5-store.rar", "rar4-store.rar"] {
        let bytes = fixture(name);
        assert!(rar::sniff(&bytes));
        let i = index(bytes, ReadFormat::Rar);
        assert_eq!(paths(&i), ["input/hello.txt", "input/nested/data.bin", "input/naive-☃.txt"]);
        check_all(&i);
        for e in &i.entries {
            assert!(e.mtime.unwrap() > 1_700_000_000_000.0);
            assert_eq!(e.mode.unwrap() & 0o170000, 0o100000);
        }
    }
}
#[test]
fn ranged_index_does_not_fetch_member_payload() {
    let bytes = fixture("rar5-store.rar");
    let offset = find_bytes(&bytes, b"hello from rar\n") as u64;
    assert!(offset > 0);
    let (source, reads) = counting(bytes);
    let i = registry::read_index(source, ReadFormat::Rar, None, ReadLimits::default(), utc_date).unwrap();
    assert!(!reads.lock().unwrap().iter().any(|&(s, e)| s <= offset && e > offset));
    assert_eq!(hash(&index_member(&i, "input/hello.txt")), expected("input/hello.txt"));
    assert!(reads.lock().unwrap().iter().any(|&(s, e)| s <= offset && e > offset));
}
#[test]
fn rar5_sfx_prefix() {
    let mut bytes = vec![0xcc; 37];
    bytes.extend(fixture("rar5-store.rar"));
    assert!(rar::sniff(&bytes));
    assert_eq!(hash(&index_member(&index(bytes, ReadFormat::Rar), "input/hello.txt")), expected("input/hello.txt"));
}
#[test]
fn compressed_filters_and_solid_out_of_order() {
    for name in ["rar5-default.rar", "rar4-default.rar"] {
        check_all(&index(fixture(name), ReadFormat::Rar));
    }
    let x86 = index(fixture("rar5-x86-filter.rar"), ReadFormat::Rar);
    assert_eq!(hash(&index_member(&x86, "input/x86.bin")), expected("input/x86.bin"));
    let solid = index(fixture("rar5-solid.rar"), ReadFormat::Rar);
    for path in ["input/x86.bin", "input/hello.txt"] {
        assert_eq!(hash(&index_member(&solid, path)), expected(path));
    }
}
#[test]
fn rar5_rar4_unix_symlink_index() {
    for name in ["rar5-symlink.rar", "rar4-symlink.rar"] {
        let i = index(fixture(name), ReadFormat::Rar);
        assert_eq!(entry(&i, "input/links/hello-link").mode.unwrap() & 0o170000, 0o120000);
        assert_link(&i, "input/links/hello-link", "input/hello.txt", true);
    }
}
#[test]
fn encryption_volumes_recovery_and_unsupported() {
    for (name, message) in [
        ("rar5-password.rar", "Encrypted RAR5 member"),
        ("rar5-header-password.rar", "Encrypted RAR5 headers"),
        ("rar5-volume.rar", "multi-volume RAR5"),
        ("rar5-recovery.rar", "RAR5 recovery record"),
        ("rar4-password.rar", "Encrypted RAR4 file data"),
        ("rar4-header-password.rar", "Encrypted RAR4 headers"),
        ("rar4-recovery.rar", "RAR4 recovery record"),
        ("rar4-unsupported.rar", "Unsupported RAR4 compression method 0x36"),
    ] {
        error_contains(index_limits(fixture(name), ReadFormat::Rar, ReadLimits::default()), message);
    }
    for (name, path, message) in [
        ("rar5-unsupported.rar", "input/hello.txt", "Unsupported RAR5 compression method 6"),
        ("rar4-ppm.rar", "input/ppm.txt", "PPMd compressed block"),
    ] {
        let i = index(fixture(name), ReadFormat::Rar);
        error_contains(i.read_file(&path.into()), message);
    }
}
#[test]
fn corruption_traversal_and_limits() {
    let original = fixture("rar5-store.rar");
    let safe = index(fixture("rar5-traversal.rar"), ReadFormat::Rar);
    assert!(!paths(&safe).iter().any(|p| p == "../escape.txtxx"));
    assert_eq!(hash(&index_member(&safe, "input/nested/data.bin")), expected("input/nested/data.bin"));
    assert!(index_limits(original[..original.len() - 3].to_vec(), ReadFormat::Rar, ReadLimits::default()).is_err());
    let mut corrupt = original.clone();
    corrupt[20] ^= 1;
    error_contains(index_limits(corrupt, ReadFormat::Rar, ReadLimits::default()), "header CRC32 mismatch");
    let mut corrupt = original.clone();
    let offset = find_bytes(&corrupt, b"hello from rar\n");
    corrupt[offset] ^= 1;
    error_contains(index(corrupt, ReadFormat::Rar).read_file(&"input/hello.txt".into()), "CRC32 mismatch");
    error_contains(
        index_limits(original.clone(), ReadFormat::Rar, ReadLimits { max_member: 5, ..ReadLimits::default() }),
        "too large",
    );
    error_contains(
        index_limits(original, ReadFormat::Rar, ReadLimits { max_entries: 1, ..ReadLimits::default() }),
        "too many entries",
    );
}
