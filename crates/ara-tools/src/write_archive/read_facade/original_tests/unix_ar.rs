//! Fixed OMP596f2da packages/utils/test/ar/unix-ar.test.ts, all original bodies.
//! MIT; copyright notices are retained in the parent original_archive_tests module.
use super::*;

#[test]
fn reads_bsd_short_and_extended_names_from_bsdtar() {
    let bytes = fixture("unix-bsdtar.a");
    assert!(unix_ar::sniff(&bytes));
    let entries = index(bytes, ReadFormat::Ar);
    assert_eq!(paths(&entries), ["short.txt", "deep.txt", "this-is-a-very-long-member-name.txt"]);
    assert_eq!(index_member(&entries, "short.txt"), b"short member\n");
    assert_eq!(index_member(&entries, "deep.txt"), b"nested member\n");
    assert_eq!(index_member(&entries, "this-is-a-very-long-member-name.txt"), b"long member\n");
    let long = entry(&entries, "this-is-a-very-long-member-name.txt");
    assert_eq!(long.mode, Some(0o100644));
    assert!(long.mtime.unwrap() > 0.0);
}

#[test]
fn handles_darwin_ar_nul_padded_bsd_names_and_skips_its_symbol_table() {
    // Buffer API has no distinct Rust entry point; invoke the real decoder on the same bytes.
    let entries = index(fixture("unix-bsd.a"), ReadFormat::Ar);
    assert_eq!(paths(&entries), ["short.txt", "this-is-a-very-long-member-name.txt"]);
    assert!(!paths(&entries).iter().any(|p| p.contains("SYMDEF")));
    assert!(index_member(&entries, "short.txt").starts_with(b"short member\n"));
}

#[test]
fn resolves_gnu_long_names_creates_parents_and_drops_traversal() {
    let entries = index(fixture("unix-gnu.a"), ReadFormat::Ar);
    let mut actual = paths(&entries);
    actual.sort();
    assert_eq!(actual, ["nested", "nested/really-long-file-name.txt", "short.txt"]);
    let long = entry(&entries, "nested/really-long-file-name.txt");
    assert_eq!(long.mode, Some(0o100644));
    assert_eq!(long.mtime, Some(1_700_000_000_000.0));
    assert_eq!(index_member(&entries, "nested/really-long-file-name.txt"), b"gnu long\n");
}

#[test]
fn accepts_blank_optional_numeric_fields() {
    let mut bytes = fixture("unix-gnu.a");
    let offset = 192;
    bytes[offset + 16..offset + 28].fill(0x20);
    bytes[offset + 40..offset + 48].fill(0x20);
    let entries = index(bytes, ReadFormat::Ar);
    let short = entry(&entries, "short.txt");
    assert_eq!(short.mtime, None);
    assert_eq!(short.mode, None);
    assert_eq!(index_member(&entries, "short.txt"), b"short gnu\n");
}

#[test]
fn lists_coff_import_library_members_without_parsing_coff_payloads() {
    let entries = index(fixture("windows-import.lib"), ReadFormat::Ar);
    assert_eq!(paths(&entries), ["short.obj", "very-long-windows-object-name.obj"]);
    assert_eq!(index_member(&entries, "short.obj"), b"COFF-short");
    assert_eq!(index_member(&entries, "very-long-windows-object-name.obj"), b"COFF-long");
}

#[test]
fn indexes_with_ranges_and_defers_ordinary_member_payload_reads() {
    let bytes = fixture("unix-gnu.a");
    let fixture_len = bytes.len() as u64;
    let (source, reads) = counting(bytes);
    let entries = index_source(source, ReadFormat::Ar, None, ReadLimits::default()).unwrap();
    let indexed_bytes: u64 = reads.lock().unwrap().iter().map(|(start, end)| end - start).sum();
    assert!(indexed_bytes < fixture_len);
    index_member(&entries, "short.txt");
    let extracted_bytes: u64 = reads.lock().unwrap().iter().map(|(start, end)| end - start).sum();
    assert!(extracted_bytes > indexed_bytes);
}

#[test]
fn rejects_truncation_malformed_headers_missing_long_tables_and_configured_limits() {
    let bytes = fixture("unix-gnu.a");
    assert!(index_limits(bytes[..bytes.len() - 1].to_vec(), ReadFormat::Ar, ReadLimits::default()).is_err());
    let mut bad_header = bytes.clone();
    bad_header[8 + 58] = 0;
    error_contains(index_limits(bad_header, ReadFormat::Ar, ReadLimits::default()), "member header");
    let mut missing_table = bytes.clone();
    missing_table[8 + 60 + 4..8 + 60 + 4 + 16].copy_from_slice(b"not-table       ");
    error_contains(index_limits(missing_table, ReadFormat::Ar, ReadLimits::default()), "missing long-name table");
    error_contains(
        index_limits(bytes.clone(), ReadFormat::Ar, ReadLimits { max_member: 8, ..ReadLimits::default() }),
        "too large to extract",
    );
    error_contains(
        index_limits(bytes, ReadFormat::Ar, ReadLimits { max_path: 10, ..ReadLimits::default() }),
        "member path exceeds 10 bytes",
    );
}

#[test]
fn presents_debian_binary_prefixed_control_files_and_data_at_root() {
    let bytes = fixture("tiny.deb");
    assert!(deb::sniff(&bytes));
    let entries = index(bytes, ReadFormat::Deb);
    let mut actual = paths(&entries);
    actual.sort();
    assert_eq!(
        actual,
        [
            "control",
            "control/control",
            "control/escape-link",
            "control/missing-link",
            "control/postinst",
            "debian-binary",
            "usr",
            "usr/share",
            "usr/share/demo",
            "usr/share/demo/hello.txt",
        ]
    );
    assert_eq!(index_member(&entries, "debian-binary"), b"2.0\n");
    assert!(String::from_utf8(index_member(&entries, "control/control")).unwrap().contains("Package: tiny\n"));
    assert_eq!(entry(&entries, "control/postinst").mode.unwrap() & 0o777, 0o755);
    assert_link(&entries, "control/missing-link", "control/missing-target", false);
    assert_link(&entries, "control/escape-link", "../../outside", false);
    assert_eq!(index_member(&entries, "usr/share/demo/hello.txt"), b"hello from deb\n");
}

fn expands_control_tar_members(archive_name: &str) {
    let entries = index(fixture(archive_name), ReadFormat::Deb);
    assert!(String::from_utf8(index_member(&entries, "control/control")).unwrap().contains("Package: tiny\n"));
    assert_eq!(index_member(&entries, "usr/share/demo/hello.txt"), b"hello from deb\n");
}
// Original dynamic test registrations remain separately runnable for every compression.
#[test]
fn expands_uncompressed_control_tar_members() {
    expands_control_tar_members("tiny-uncompressed.deb");
}
#[test]
fn expands_bzip2_control_tar_members() {
    expands_control_tar_members("tiny-bz2.deb");
}
#[test]
fn expands_xz_control_tar_members() {
    expands_control_tar_members("tiny-xz.deb");
}
#[test]
fn expands_zstd_control_tar_members() {
    expands_control_tar_members("tiny-zst.deb");
}
#[test]
fn expands_lzma_alone_control_tar_members() {
    expands_control_tar_members("tiny-lzma.deb");
}

#[test]
fn requires_debian_binary_first_and_rejects_unsupported_control_compression() {
    let bytes = fixture("unix-gnu.a");
    assert!(!deb::sniff(&bytes));
    error_contains(index_limits(bytes, ReadFormat::Deb, ReadLimits::default()), "first member is not debian-binary");
    let mut bytes = fixture("tiny.deb");
    let offset = find_bytes(&bytes, b"control.tar.gz");
    assert!(offset > 0);
    bytes[offset..offset + 14].copy_from_slice(b"control.tar.zz");
    error_contains(index_limits(bytes, ReadFormat::Deb, ReadLimits::default()), "Unsupported deb tar compression");
}
