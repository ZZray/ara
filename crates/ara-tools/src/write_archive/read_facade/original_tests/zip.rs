//! Fixed zip.test.ts original assertions; MIT upstream notices apply.
//! Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
#[test]
fn bounded_central_index_and_stored_deflated_payloads() {
    let bytes = fixture("zip-basic.zip");
    assert_eq!(registry::sniff_archive_format(&bytes), Some(ReadFormat::Zip));
    let (source, reads) = counting(bytes);
    let i = index_source(source, ReadFormat::Zip, None, ReadLimits::default()).unwrap();
    assert!(reads.lock().unwrap().len() <= 3);
    let mut names = paths(&i);
    names.sort();
    assert_eq!(names, ["nested/repeated.txt", "plain.txt"]);
    let reader = Reader::from_index(i, ReadLimits::default()).unwrap();
    assert_eq!(
        reader
            .list_directory(&"".into(), |a, b| a.units().cmp(b.units()))
            .unwrap()
            .iter()
            .map(|e| (utf8(&e.name), e.node.directory))
            .collect::<Vec<_>>(),
        [("nested".into(), true), ("plain.txt".into(), false)]
    );
    assert_eq!(member(&reader, "plain.txt"), b"stored payload\n");
    assert!(String::from_utf8(member(&reader, "nested/repeated.txt")).unwrap().contains("compress me"));
    assert!(node(&reader, "plain.txt").mtime.is_some());
}
#[test]
fn all_five_nondeflate_compression_methods() {
    for name in ["zip-bzip2.zip", "zip-lzma.zip", "zip-zstd.zip", "zip-zstd93.zip", "zip-xz.zip"] {
        assert_eq!(
            index_member(&index(fixture(name), ReadFormat::Zip), "nested/repeated.txt"),
            b"compress me compress me compress me compress me compress me compress me compress me compress me\n",
            "{name}"
        );
    }
}
#[test]
fn zip64_descriptor_and_windows_1252_names() {
    assert_eq!(index_member(&index(fixture("zip-zip64.zip"), ReadFormat::Zip), "zip64.txt"), b"zip64 payload\n");
    assert_eq!(
        index_member(&index(fixture("zip-descriptor.zip"), ReadFormat::Zip), "descriptor.txt"),
        b"descriptor payload\n".repeat(8)
    );
    assert_eq!(index_member(&index(fixture("zip-legacy-name.zip"), ReadFormat::Zip), "café.txt"), b"legacy name\n");
}
#[test]
fn unix_modes_and_indexed_symlink_payload() {
    let i = index(fixture("zip-symlink-mode.zip"), ReadFormat::Zip);
    let executable = entry(&i, "bin/run.sh");
    assert_eq!(executable.mode.unwrap() & 0o170000, 0o100000);
    assert_eq!(executable.mode.unwrap() & 0o111, 0o111);
    assert!(executable.mtime.is_some());
    assert_eq!(entry(&i, "current").mode.unwrap() & 0o170000, 0o120000);
    assert_link(&i, "current", "bin/run.sh", true);
    assert!(String::from_utf8(index_member(&i, "current")).unwrap().contains("zip mode"));
}
#[test]
fn encode_roundtrip_lazy_eager_and_traversal_rejection() {
    let members = vec![
        ("small.bin".into(), vec![0, 1, 2, 255]),
        ("nested/repeated.txt".into(), b"highly compressible ".repeat(30)),
    ];
    let encoded = zip::encode(&members).unwrap();
    assert_eq!(registry::sniff_archive_format(&encoded), Some(ReadFormat::Zip));
    let i = index(encoded.clone(), ReadFormat::Zip);
    assert_eq!(index_member(&i, "small.bin"), [0, 1, 2, 255]);
    // Rust's existing eager materialization is the document-converter equivalent.
    let eager = zip::read(&Source::Memory(Arc::new(encoded))).unwrap().materialize().unwrap();
    assert_eq!(
        eager.ordered.iter().find(|(p, _)| utf8(p) == "nested/repeated.txt").unwrap().1,
        b"highly compressible ".repeat(30)
    );
    assert!(zip::encode(&[("../escape".into(), Vec::new())]).is_err());
}
#[test]
fn encode_zip64_65535_entry_count_boundary() {
    let members = (0..0xffff).map(|n| (format!("empty-{n}").into(), Vec::new())).collect::<Vec<_>>();
    let bytes = zip::encode(&members).unwrap();
    let i = index(bytes, ReadFormat::Zip);
    assert_eq!(i.entries.len(), 0xffff);
    assert_eq!(entry(&i, "empty-65534").size, 0);
}
#[test]
fn traversal_encryption_truncation_unsupported_method_and_crc() {
    assert!(index(fixture("zip-traversal.zip"), ReadFormat::Zip).entries.is_empty());
    error_contains(
        index_limits(fixture("zip-encrypted.zip"), ReadFormat::Zip, ReadLimits::default()),
        "Encrypted ZIP member",
    );
    assert!(index_limits(fixture("zip-basic.zip")[..40].to_vec(), ReadFormat::Zip, ReadLimits::default()).is_err());
    let mut bytes = fixture("zip-descriptor.zip");
    let central = find_bytes(&bytes, &0x02014b50u32.to_le_bytes());
    bytes[8] = 9;
    bytes[9] = 0;
    bytes[central + 10] = 9;
    bytes[central + 11] = 0;
    error_contains(
        index(bytes, ReadFormat::Zip).read_file(&"descriptor.txt".into()),
        "Unsupported ZIP compression method 9",
    );
    let mut bytes = fixture("zip-basic.zip");
    let start = 30 + read16(&bytes, 26) as usize + read16(&bytes, 28) as usize;
    bytes[start] ^= 1;
    error_contains(index(bytes, ReadFormat::Zip).read_file(&"plain.txt".into()), "CRC mismatch");
}
#[test]
fn entry_index_member_and_path_limits() {
    for limits in [
        ReadLimits { max_entries: 1, ..ReadLimits::default() },
        ReadLimits { max_index: 8, ..ReadLimits::default() },
        ReadLimits { max_member: 8, ..ReadLimits::default() },
        ReadLimits { max_path: 4, ..ReadLimits::default() },
    ] {
        assert!(index_limits(fixture("zip-basic.zip"), ReadFormat::Zip, limits).is_err());
    }
}
