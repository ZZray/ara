//! Fixed tar.test.ts original assertions; MIT upstream notices apply.
//! Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
#[test]
fn ustar_paths_modes_mtimes_directories_and_bytes() {
    let bytes = fixture("tar-ustar.tar");
    assert!(tar::sniff_for_read(&bytes, ReadLimits::default()));
    let i = index(bytes, ReadFormat::Tar);
    assert_eq!(paths(&i), ["nested", "hello.txt", "nested/data.bin"]);
    assert_eq!(entry(&i, "hello.txt").mode, Some(0o640));
    assert_eq!(entry(&i, "hello.txt").mtime, Some(1_700_000_000_000.0));
    assert_eq!(index_member(&i, "hello.txt"), b"hello tar\n");
    assert!(entry(&i, "nested").directory);
    assert_eq!(entry(&i, "nested/data.bin").mode, Some(0o755));
    assert_eq!(index_member(&i, "nested/data.bin"), [0, 1, 2, 255]);
}
#[test]
fn whole_byte_source_once_and_short_source_rejection() {
    let bytes = fixture("tar-ustar.tar");
    let (source, reads) = counting(bytes.clone());
    let i = index_source(source, ReadFormat::Tar, None, ReadLimits::default()).unwrap();
    assert_eq!(reads.lock().unwrap().len(), 1);
    assert_eq!(index_member(&i, "hello.txt"), b"hello tar\n");
    let short = Source::Read(Arc::new(Counting { bytes, reads: Arc::new(Mutex::new(Vec::new())), short: true }));
    error_contains(index_source(short, ReadFormat::Tar, None, ReadLimits::default()), "truncated");
}
#[test]
fn pax_local_long_and_global_set_delete() {
    let i = index(fixture("tar-pax-long.tar"), ReadFormat::Tar);
    let path = paths(&i).into_iter().find(|p| p.ends_with("/payload.txt")).unwrap();
    assert!(path.len() > 160);
    assert_eq!(index_member(&i, &path), b"pax long path\n");
    let i = index(fixture("tar-pax-global.tar"), ReadFormat::Tar);
    assert_eq!(index_member(&i, "global.txt"), b"global\n");
    assert_eq!(index_member(&i, "literal.txt"), b"literal\n");
    assert!(i.get(&"ignored.txt".into()).is_none());
}
#[test]
fn gnu_l_k_long_records() {
    let i = index(fixture("tar-gnu-longlink.tar"), ReadFormat::Tar);
    let p = paths(&i).into_iter().find(|p| p.ends_with("payload.txt")).unwrap();
    assert!(p.len() > 150);
    assert_eq!(index_member(&i, &p), b"gnu long link payload\n");
    let i = index(fixture("tar-gnu-long-links.tar"), ReadFormat::Tar);
    assert_eq!(index_member(&i, "pkg/current"), b"long target\n");
}
#[test]
fn hard_links_relative_symlinks_directory_alias_and_cycles() {
    let i = index(fixture("tar-links.tar"), ReadFormat::Tar);
    assert_eq!(index_member(&i, "pkg/hard.txt"), b"shared content\n");
    assert_eq!(index_member(&i, "pkg/bin/tool"), b"export const tool = true;\n");
    assert!(entry(&i, "pkg/current").directory);
    assert_link(&i, "pkg/current", "pkg/lib", false);
    assert_link(&i, "pkg/dangling", "pkg/missing-target", false);
    error_contains(
        index_limits(fixture("tar-cyclic-links.tar"), ReadFormat::Tar, ReadLimits::default()),
        "cyclic or unsupported links",
    );
    assert!(matches!(entry(&i, "pkg/dangling").storage, Some(Storage::Link { .. })));
}
#[test]
fn sparse_pax_versions_and_oldgnu_continuations() {
    for name in ["tar-sparse-pax-0.0.tar", "tar-sparse-pax-0.1.tar", "tar-sparse-pax.tar", "tar-sparse-oldgnu.tar"] {
        let i = index(fixture(name), ReadFormat::Tar);
        if name != "tar-sparse-oldgnu.tar" {
            assert_eq!(entry(&i, "data/sparse.bin").size, 33_554_432);
        }
        error_contains(i.read_file(&"data/sparse.bin".into()), "sparse file");
        assert_eq!(index_member(&i, "after.txt"), b"after sparse\n", "{name}");
    }
}
#[test]
fn base256_signed_checksums_oldgnu_rename() {
    let i = index(fixture("tar-base256.tar"), ReadFormat::Tar);
    assert_eq!(entry(&i, "binary.txt").size, 3);
    assert_eq!(entry(&i, "binary.txt").mtime, Some(42_000.0));
    assert_eq!(index_member(&i, "binary.txt"), b"bin");
    assert_eq!(index_member(&index(fixture("tar-signed-checksum.tar"), ReadFormat::Tar), "signed.txt"), b"signed");
    let i = index(fixture("tar-oldgnu-rename.tar"), ReadFormat::Tar);
    assert!(i.get(&"old/file.txt".into()).is_none());
    assert_eq!(index_member(&i, "moved/file.txt"), b"renamed\n");
    assert_eq!(index_member(&i, "outside.txt"), b"renamed\n");
}
#[test]
fn traversal_corrupt_truncated_unterminated_multivolume() {
    let i = index(fixture("tar-traversal.tar"), ReadFormat::Tar);
    assert_eq!(paths(&i), ["safe.txt"]);
    assert_eq!(index_member(&i, "safe.txt"), b"safe");
    error_contains(index_limits(fixture("tar-truncated.tar"), ReadFormat::Tar, ReadLimits::default()), "truncated");
    let valid = fixture("tar-ustar.tar");
    error_contains(
        index_limits(valid[..valid.len() - 1024].to_vec(), ReadFormat::Tar, ReadLimits::default()),
        "missing terminating zero block",
    );
    let mut corrupt = valid;
    corrupt[0] ^= 1;
    assert!(!tar::sniff_for_read(&corrupt, ReadLimits::default()));
    error_contains(index_limits(corrupt, ReadFormat::Tar, ReadLimits::default()), "corrupt tar archive header");
    error_contains(
        index_limits(fixture("tar-unsupported-multivolume.tar"), ReadFormat::Tar, ReadLimits::default()),
        "multi-volume tar members are not supported",
    );
}
#[test]
fn archive_member_path_entry_metadata_limits() {
    let bytes = fixture("tar-ustar.tar");
    for (limits, message) in [
        (ReadLimits { max_memory: bytes.len() as u64 - 1, ..ReadLimits::default() }, "too large to read in memory"),
        (ReadLimits { max_member: 3, ..ReadLimits::default() }, "too large to extract"),
        (ReadLimits { max_path: 5, ..ReadLimits::default() }, "exceeds 5 bytes"),
        (ReadLimits { max_entries: 1, ..ReadLimits::default() }, "too many entries"),
    ] {
        error_contains(index_limits(bytes.clone(), ReadFormat::Tar, limits), message);
    }
    error_contains(
        index_limits(
            fixture("tar-pax-long.tar"),
            ReadFormat::Tar,
            ReadLimits { max_index: 8, ..ReadLimits::default() },
        ),
        "PAX metadata is too large",
    );
}
#[test]
fn writer_determinism_parent_directories_and_pax_roundtrip() {
    let long = format!("long/{}/payload.txt", "segment".repeat(30));
    let members = vec![
        ("tree/repeat.txt".into(), vec![0x41; 1025]),
        ("tree/deep/note.txt".into(), b"nested".to_vec()),
        (long.as_str().into(), b"long name".to_vec()),
    ];
    let first = tar::encode(&members).unwrap();
    assert_eq!(first, tar::encode(&members).unwrap());
    assert_eq!(first.len() % 512, 0);
    assert!(tar::sniff_for_read(&first, ReadLimits::default()));
    let i = index(first, ReadFormat::Tar);
    for p in ["tree", "tree/deep"] {
        assert!(entry(&i, p).directory);
    }
    assert_eq!(entry(&i, "tree/repeat.txt").mode, Some(0o644));
    assert_eq!(entry(&i, "tree").mode, Some(0o755));
    assert_eq!(index_member(&i, "tree/repeat.txt"), members[0].1);
    assert_eq!(index_member(&i, &long), b"long name");
}
#[test]
fn writer_explicit_empty_directory_unsafe_conflicting_paths() {
    let bytes = tar::encode(&[("empty/".into(), Vec::new())]).unwrap();
    assert!(entry(&index(bytes, ReadFormat::Tar), "empty").directory);
    assert!(tar::encode(&[("../escape".into(), Vec::new())]).is_err());
    error_contains(tar::encode(&[("node".into(), Vec::new()), ("node/child".into(), Vec::new())]), "not a directory");
    error_contains(tar::encode(&[("empty/".into(), vec![1])]), "cannot contain file data");
}
#[test]
fn writer_empty_two_zero_blocks() {
    let bytes = tar::encode(&[]).unwrap();
    assert_eq!(bytes.len(), 1024);
    assert!(tar::sniff_for_read(&bytes, ReadLimits::default()));
    assert!(index(bytes, ReadFormat::Tar).entries.is_empty());
}
