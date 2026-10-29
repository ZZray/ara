//! Fixed OMP596f2da packages/utils/test/ar/iso.test.ts, all original bodies.
//! MIT; copyright notices are retained in the parent original_archive_tests module.
use super::*;

fn iso_fixture() -> Vec<u8> {
    fixture("rock-ridge-joliet.iso")
}
fn ucs2be(value: &str) -> Vec<u8> {
    value.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

#[test]
fn prefers_joliet_names_and_lists_deep_directories_and_rock_ridge_links() {
    let bytes = iso_fixture();
    assert!(iso::sniff(&bytes[..bytes.len().min(128 * 1024)]));
    let entries = index(bytes, ReadFormat::Iso);
    assert_eq!(
        paths(&entries),
        [
            "empty-dir",
            "hello.txt",
            "nested",
            "nested/deep",
            "nested/日本語-long-name.txt",
            "nested/deep/level",
            "nested/deep/level/three",
            "nested/deep/level/three/end.bin",
            "nested/hello-link"
        ]
    );
    for path in ["empty-dir", "nested/deep/level/three"] {
        let e = entry(&entries, path);
        assert!(e.directory);
        assert_eq!(e.size, 0);
    }
    assert_eq!(entry(&entries, "hello.txt").mode.unwrap_or(0) & 0o170000, 0o100000);
    assert_link(&entries, "nested/hello-link", "hello.txt", true);
    let link = entry(&entries, "nested/hello-link");
    assert_eq!(link.mode.unwrap_or(0) & 0o170000, 0o120000);
    assert!(link.mtime.is_some());
}

#[test]
fn extracts_file_extents_lazily_and_verifies_declared_sizes() {
    let bytes = iso_fixture();
    let fixture_len = bytes.len() as u64;
    let (source, reads) = counting(bytes);
    let entries = index_source(source, ReadFormat::Iso, None, ReadLimits::default()).unwrap();
    let indexed_reads = reads.lock().unwrap().len();
    let indexed_bytes: u64 = reads.lock().unwrap().iter().map(|(start, end)| end - start).sum();
    assert!(indexed_bytes < fixture_len);
    assert_eq!(index_member(&entries, "hello.txt"), b"hello from iso\n");
    assert_eq!(index_member(&entries, "nested/日本語-long-name.txt"), b"unicode payload\n");
    assert_eq!(index_member(&entries, "nested/deep/level/three/end.bin"), b"deep end\n");
    let extracted_bytes: u64 = reads.lock().unwrap().iter().map(|(start, end)| end - start).sum();
    assert!(extracted_bytes > indexed_bytes);
    assert!(reads.lock().unwrap().len() > indexed_reads);
    error_contains(
        index_member_sized(&entries, "hello.txt", entry(&entries, "hello.txt").size + 1),
        "produced 15 bytes, expected 16",
    );
}

#[test]
fn drops_a_joliet_path_that_escapes_the_archive_root() {
    let mut bytes = iso_fixture();
    let offset = find_bytes(&bytes, &ucs2be("hello.txt"));
    let replacement = ucs2be("../bad.txt");
    bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
    let entries = index(bytes, ReadFormat::Iso);
    assert!(!paths(&entries).iter().any(|p| p == "hello.txt" || p == "bad.txt"));
}

#[test]
fn rejects_truncation_mismatched_both_endian_fields_and_rock_ridge_relocation() {
    let bytes = iso_fixture();
    assert!(index_limits(bytes[..64 * 1024].to_vec(), ReadFormat::Iso, ReadLimits::default()).is_err());
    let mut endian = bytes.clone();
    let supplementary = (16..)
        .map(|sector| sector * 2048)
        .take_while(|offset| offset + 7 <= endian.len())
        .find(|&offset| endian[offset] == 2 && &endian[offset + 1..offset + 6] == b"CD001")
        .expect("original supplementary descriptor");
    endian[supplementary + 130] ^= 1;
    error_contains(
        index_limits(endian, ReadFormat::Iso, ReadLimits::default()),
        "logical block size has mismatched byte orders",
    );
    let mut relocation = bytes;
    let sl_offset = find_bytes(&relocation, &[0x53, 0x4c, 0x12, 0x01]);
    relocation[sl_offset] = 0x43;
    error_contains(
        index_limits(relocation, ReadFormat::Iso, ReadLimits::default()),
        "Rock Ridge relocated directory (CL)",
    );
}

fn rejects_signature(signature: &[u8], signature_offset: usize, message: &str) {
    let mut bytes = vec![0; 17 * 2048];
    bytes[16 * 2048] = 0;
    bytes[16 * 2048 + signature_offset..16 * 2048 + signature_offset + 5].copy_from_slice(signature);
    bytes[16 * 2048 + signature_offset + 5] = 1;
    error_contains(index_limits(bytes, ReadFormat::Iso, ReadLimits::default()), message);
}
// The original for-loop retains both parameter identities as independently runnable tests.
#[test]
fn detects_high_sierra_with_precise_error() {
    rejects_signature(b"CDROM", 9, "High Sierra");
}
#[test]
fn detects_udf_only_with_precise_error() {
    rejects_signature(b"NSR02", 1, "UDF-only");
}

#[test]
fn enforces_metadata_entry_member_and_path_limits() {
    let bytes = iso_fixture();
    error_contains(
        index_limits(bytes.clone(), ReadFormat::Iso, ReadLimits { max_index: 2047, ..ReadLimits::default() }),
        "metadata limit",
    );
    error_contains(
        index_limits(bytes.clone(), ReadFormat::Iso, ReadLimits { max_entries: 2, ..ReadLimits::default() }),
        "too many entries",
    );
    error_contains(
        index_limits(bytes.clone(), ReadFormat::Iso, ReadLimits { max_member: 8, ..ReadLimits::default() }),
        "too large to extract",
    );
    error_contains(
        index_limits(bytes, ReadFormat::Iso, ReadLimits { max_path: 5, ..ReadLimits::default() }),
        "member name exceeds 5 bytes",
    );
}

#[test]
fn accepts_zero_length_records_with_junk_extents_bsdtar_symlink_joliet_entries() {
    // Original bsdtar zero-length Joliet symlink has an unallocated junk extent.
    let entries = index(fixture("minimal-symlink.iso"), ReadFormat::Iso);
    match entry(&entries, "link.txt").storage.as_ref().unwrap() {
        Storage::Link { target, .. } => assert_eq!(utf8(target), "real.txt"),
        _ => panic!("expected original Rock Ridge link"),
    }
    assert_eq!(index_member(&entries, "real.txt"), b"target\n");
}
