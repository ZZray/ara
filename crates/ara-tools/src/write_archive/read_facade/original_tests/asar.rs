//! Fixed OMP596f2da packages/utils/test/ar/asar.test.ts, all original bodies.
//! MIT; copyright notices are retained in the parent original_archive_tests module.
use super::*;

fn header_fixture(files: serde_json::Value, payload: &[u8]) -> Vec<u8> {
    let json = serde_json::to_vec(&serde_json::json!({ "files": files })).unwrap();
    let padded = json.len() + (4 - json.len() % 4) % 4;
    let inner = 4 + padded;
    let header = 4 + inner;
    let mut bytes = vec![0; 8 + header + payload.len()];
    write32(&mut bytes, 0, 4);
    write32(&mut bytes, 4, header as u32);
    write32(&mut bytes, 8, inner as u32);
    write32(&mut bytes, 12, json.len() as u32);
    bytes[16..16 + json.len()].copy_from_slice(&json);
    bytes[8 + header..].copy_from_slice(payload);
    bytes
}

#[test]
fn lists_nested_directories_and_lazily_extracts_packed_members() {
    let bytes = fixture("asar-valid.asar");
    assert_eq!(registry::sniff_archive_format(&bytes), Some(ReadFormat::Asar));
    let entries = index(bytes, ReadFormat::Asar);
    let mut metadata: Vec<_> = entries.entries.iter().map(|e| (utf8(&e.path), e.directory, e.size)).collect();
    metadata.sort();
    assert_eq!(
        metadata,
        vec![("docs".into(), true, 0), ("docs/hello.txt".into(), false, 11), ("empty.bin".into(), false, 0)]
    );
    assert_eq!(index_member(&entries, "docs/hello.txt"), b"hello asar\n");
    assert_eq!(index_member(&entries, "empty.bin"), Vec::<u8>::new());
}

#[test]
fn rejects_bad_pickle_oversized_header_and_escaping_link_fixtures_precisely() {
    for (name, expected) in [
        ("asar-bad-pickle.asar", "Invalid ASAR archive: invalid size pickle"),
        ("asar-oversized-header.asar", "ASAR header is too large"),
        ("asar-link-escape.asar", "links outside the archive"),
    ] {
        error_contains(index_limits(fixture(name), ReadFormat::Asar, ReadLimits::default()), expected);
    }
}

#[test]
fn unpacked_sibling_reads_verify_the_declared_size() {
    let root = tempfile::tempdir().unwrap();
    let archive_path = root.path().join("bundle.asar");
    let bytes = fixture("asar-unpacked.asar");
    std::fs::write(&archive_path, &bytes).unwrap();
    let unpacked = root.path().join("bundle.asar.unpacked");
    std::fs::create_dir_all(&unpacked).unwrap();
    std::fs::write(unpacked.join("external.txt"), b"short").unwrap();
    let entries =
        index_source(source::source_from_bytes(bytes), ReadFormat::Asar, Some(&archive_path), ReadLimits::default())
            .unwrap();
    error_contains(
        index_member_result(&entries, "external.txt"),
        "size differs from its archive header (5 != 8 bytes)",
    );
}

#[test]
fn encode_asar_round_trips_nested_and_zero_length_members() {
    let bytes = asar::encode(&[
        ("top.txt".into(), b"top".to_vec()),
        ("nested/data.bin".into(), vec![0, 1, 2, 255]),
        ("nested/zero".into(), vec![]),
    ])
    .unwrap();
    assert_eq!(registry::sniff_archive_format(&bytes), Some(ReadFormat::Asar));
    let entries = index(bytes, ReadFormat::Asar);
    let mut actual = paths(&entries);
    actual.sort();
    assert_eq!(actual, ["nested", "nested/data.bin", "nested/zero", "top.txt"]);
    assert_eq!(index_member(&entries, "top.txt"), b"top");
    assert_eq!(index_member(&entries, "nested/data.bin"), [0, 1, 2, 255]);
}

#[test]
fn maps_links_and_executable_flags_and_verifies_integrity() {
    let payload = b"payload";
    let digest = hash(payload);
    let mut bytes = header_fixture(
        serde_json::json!({
            "bin": {"size": payload.len(), "offset": "0", "executable": true,
                "integrity": {"algorithm": "SHA256", "hash": digest, "blockSize": 4 * 1024 * 1024, "blocks": [digest]}},
            "alias": {"link": "bin"},
        }),
        payload,
    );
    let entries = index(bytes.clone(), ReadFormat::Asar);
    assert_eq!(entry(&entries, "bin").mode, Some(0o100755));
    assert_link(&entries, "alias", "bin", true);
    assert_eq!(index_member(&entries, "bin"), payload);
    *bytes.last_mut().unwrap() ^= 1;
    let changed = index(bytes, ReadFormat::Asar);
    error_contains(index_member_result(&changed, "bin"), "failed SHA256 integrity verification");
}

#[test]
fn rejects_traversal_malformed_records_truncation_and_non_zero_pickle_padding() {
    for (identity, files) in [
        ("traversal", serde_json::json!({"../escape": {"size": 0, "offset": "0"}})),
        ("negative-size", serde_json::json!({"bad": {"size": -1, "offset": "0"}})),
        ("missing-payload", serde_json::json!({"bad": {"size": 1, "offset": "0"}})),
        ("numeric-offset", serde_json::json!({"bad": {"size": 0, "offset": 1}})),
    ] {
        assert!(
            index_limits(header_fixture(files, &[]), ReadFormat::Asar, ReadLimits::default()).is_err(),
            "{identity}"
        );
    }
    let mut valid = header_fixture(serde_json::json!({"abc": {"size": 0, "offset": "0"}}), &[]);
    error_contains(index_limits(valid[..15].to_vec(), ReadFormat::Asar, ReadLimits::default()), "truncated header");
    let padding_offset = 16 + read32(&valid, 12) as usize;
    if padding_offset < 8 + read32(&valid, 4) as usize {
        valid[padding_offset] = 1;
        error_contains(index_limits(valid, ReadFormat::Asar, ReadLimits::default()), "non-zero pickle string padding");
    }
}

#[test]
fn enforces_header_entry_member_and_path_limits_before_indexing() {
    let bytes =
        header_fixture(serde_json::json!({"one": {"size": 1, "offset": "0"}, "two": {"size": 0, "offset": "1"}}), &[1]);
    error_contains(
        index_limits(bytes.clone(), ReadFormat::Asar, ReadLimits { max_index: 8, ..ReadLimits::default() }),
        "ASAR header is too large",
    );
    error_contains(
        index_limits(bytes.clone(), ReadFormat::Asar, ReadLimits { max_entries: 1, ..ReadLimits::default() }),
        "too many entries",
    );
    error_contains(
        index_limits(bytes, ReadFormat::Asar, ReadLimits { max_member: 0, ..ReadLimits::default() }),
        "too large to extract",
    );
    error_contains(
        index_limits(
            header_fixture(serde_json::json!({"lengthy": {"size": 0, "offset": "0"}}), &[]),
            ReadFormat::Asar,
            ReadLimits { max_path: 3, ..ReadLimits::default() },
        ),
        "entry name exceeds 3 bytes",
    );
}

#[test]
fn encode_asar_rejects_unsafe_duplicate_and_file_crossing_paths() {
    for (identity, names) in [
        ("traversal", vec!["../escape"]),
        ("nul", vec!["bad\0name"]),
        ("duplicate", vec!["same", "same"]),
        ("file-crossing", vec!["file", "file/child"]),
    ] {
        let members: Vec<_> = names.into_iter().map(|p| (p.into(), Vec::<u8>::new())).collect();
        assert!(asar::encode(&members).is_err(), "{identity}");
    }
}
