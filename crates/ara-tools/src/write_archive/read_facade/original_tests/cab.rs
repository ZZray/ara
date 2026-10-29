//! Fixed OMP596f2da packages/utils/test/ar/cab.test.ts, all original bodies.
//! MIT; copyright notices are retained in the parent original_archive_tests module.
use super::*;

fn expected_nested_payload() -> Vec<u8> {
    format!("nested cabinet payload\n{}", "history-window-line\n".repeat(3000)).into_bytes()
}

#[test]
fn indexes_nested_paths_and_extracts_uncompressed_folder_members() {
    let bytes = fixture("cab-none.cab");
    assert!(cab::sniff(&bytes[..bytes.len().min(128 * 1024)]));
    assert!(!cab::sniff(b"not a cabinet"));
    let entries = index(bytes, ReadFormat::Cab);
    assert_eq!(paths(&entries), ["root.txt", "nested/hello.txt", "aa/evil.txt"]);
    let root = entry(&entries, "root.txt");
    assert!(!root.directory);
    assert_eq!(root.size, 17);
    assert_eq!(root.mode, Some(0o100644));
    assert!(root.mtime.is_some());
    assert_eq!(index_member(&entries, "root.txt"), b"CAB root payload\n");
    assert_eq!(index_member(&entries, "nested/hello.txt"), expected_nested_payload());
}

#[test]
fn carries_the_mszip_dictionary_across_cfdata_blocks_and_caches_the_decoded_folder() {
    let bytes = fixture("cab-mszip.cab");
    let fixture_len = bytes.len() as u64;
    let (source, reads) = counting(bytes);
    let entries = index_source(source, ReadFormat::Cab, None, ReadLimits::default()).unwrap();
    let indexed_reads = reads.lock().unwrap().len();
    let indexed_bytes: u64 = reads.lock().unwrap().iter().map(|(start, end)| end - start).sum();
    assert!(indexed_bytes < fixture_len);
    assert_eq!(index_member(&entries, "root.txt"), b"CAB root payload\n");
    assert_eq!(reads.lock().unwrap().len(), indexed_reads + 1);
    assert_eq!(index_member(&entries, "nested/hello.txt"), expected_nested_payload());
    assert_eq!(reads.lock().unwrap().len(), indexed_reads + 1);
}

#[test]
fn extracts_known_good_libmspack_mszip_and_lzx_folders_and_detects_quantum() {
    let entries = index(fixture("cab-mixed-reference.cab"), ReadFormat::Cab);
    let metadata: Vec<_> = entries.entries.iter().map(|e| (utf8(&e.path), e.size)).collect();
    assert_eq!(metadata, vec![("mszip.txt".into(), 57), ("lzx.txt".into(), 187), ("qtm.txt".into(), 59)]);
    assert_eq!(
        hash(&index_member(&entries, "mszip.txt")),
        "6a2d9536b995c42a9b9daa2c2eaabf9a1e13e594669a420f8d3e66150af33cff"
    );
    assert_eq!(
        hash(&index_member(&entries, "lzx.txt")),
        "e978598104671296857e0543f4280f4d4e0506dd3cad5162e9f2a4f604fafc78"
    );
    error_contains(index_member_result(&entries, "qtm.txt"), "Quantum (level 18)");
}

#[test]
fn decodes_lzx_uncompressed_blocks_and_applies_the_intra_frame_e8_transform() {
    let entries = index(fixture("cab-lzx-e8.cab"), ReadFormat::Cab);
    let extracted = index_member(&entries, "e8.bin");
    assert_eq!(hash(&extracted), "ea4ff46bad2ca4bea457b9a5cabbbc353f9446b7b40f83f15fd6fab262192d52");
    assert_eq!(&extracted[5..10], &[0xe8, 15, 0, 0, 0]);
}

#[test]
fn skips_cfheader_cffolder_and_cfdata_reserve_areas_while_verifying_the_block_checksum() {
    let entries = index(fixture("cab-reserved.cab"), ReadFormat::Cab);
    assert_eq!(paths(&entries), ["reserved.txt"]);
    assert_eq!(
        hash(&index_member(&entries, "reserved.txt")),
        "356fccab233a844365ec431e6208ca69cf7a57884f32a5f5f831124102f4fb84"
    );
}

#[test]
fn drops_a_member_path_that_escapes_the_archive_root() {
    let mut bytes = fixture("cab-none.cab");
    let offset = find_bytes(&bytes, b"aa\\evil.txt");
    bytes[offset..offset + 11].copy_from_slice(b"..\\evil.txt");
    let entries = index(bytes, ReadFormat::Cab);
    assert!(!paths(&entries).iter().any(|p| p == "aa/evil.txt" || p == "evil.txt"));
}

#[test]
fn honors_utf8_names_and_maps_cab_directory_attributes_to_unix_mode_bits() {
    let mut bytes = fixture("cab-reserved.cab");
    let offset = read32(&bytes, 16) as usize;
    let encoded_name = "résumé.txt".as_bytes();
    assert_eq!(encoded_name.len(), "reserved.txt".len());
    bytes[offset + 16..offset + 16 + encoded_name.len()].copy_from_slice(encoded_name);
    bytes[offset + 14] |= 0x80;
    let entries = index(bytes, ReadFormat::Cab);
    assert_eq!(paths(&entries)[0], "résumé.txt");
    let mut bytes = fixture("cab-reserved.cab");
    let offset = read32(&bytes, 16) as usize;
    write32(&mut bytes, offset, 0);
    bytes[offset + 14] |= 0x10;
    let entries = index(bytes, ReadFormat::Cab);
    let directory = &entries.entries[0];
    assert_eq!(utf8(&directory.path), "reserved.txt");
    assert!(directory.directory);
    assert_eq!(directory.size, 0);
    assert_eq!(directory.mode, Some(0o040755));
    assert!(directory.storage.is_none());
}

#[test]
fn rejects_truncation_multi_volume_links_checksum_damage_and_invalid_mszip_framing() {
    let bytes = fixture("cab-none.cab");
    assert!(index_limits(bytes[..bytes.len() - 1].to_vec(), ReadFormat::Cab, ReadLimits::default()).is_err());
    let mut multi_volume = bytes.clone();
    multi_volume[30] |= 1;
    error_contains(index_limits(multi_volume, ReadFormat::Cab, ReadLimits::default()), "multi-volume CAB archive");
    let mut corrupt = bytes;
    *corrupt.last_mut().unwrap() ^= 0xff;
    let entries = index(corrupt, ReadFormat::Cab);
    error_contains(index_member_result(&entries, "root.txt"), "checksum mismatch");
    let mut mszip = fixture("cab-mszip.cab");
    let data_start = read32(&mszip, 36) as usize;
    mszip[data_start..data_start + 4].fill(0);
    mszip[data_start + 8] = 0;
    let entries = index(mszip, ReadFormat::Cab);
    error_contains(index_member_result(&entries, "root.txt"), "CK signature");
}

#[test]
fn detects_unsupported_compression_parameters_and_verifies_extraction_size() {
    let mut bytes = fixture("cab-none.cab");
    bytes[42] = 2;
    bytes[43] = 18;
    let entries = index(bytes, ReadFormat::Cab);
    error_contains(index_member_result(&entries, "root.txt"), "Quantum (level 18)");
    let entries = index(fixture("cab-mszip.cab"), ReadFormat::Cab);
    error_contains(index_member_sized(&entries, "root.txt", entry(&entries, "root.txt").size + 1), "size changed");
}

#[test]
fn enforces_index_entry_member_path_and_in_memory_folder_limits() {
    let bytes = fixture("cab-none.cab");
    error_contains(
        index_limits(bytes.clone(), ReadFormat::Cab, ReadLimits { max_index: 64, ..ReadLimits::default() }),
        "CAB index",
    );
    error_contains(
        index_limits(bytes.clone(), ReadFormat::Cab, ReadLimits { max_entries: 2, ..ReadLimits::default() }),
        "too many entries",
    );
    error_contains(
        index_limits(bytes.clone(), ReadFormat::Cab, ReadLimits { max_member: 16, ..ReadLimits::default() }),
        "too large to extract",
    );
    error_contains(
        index_limits(bytes.clone(), ReadFormat::Cab, ReadLimits { max_path: 7, ..ReadLimits::default() }),
        "member path exceeds 7 bytes",
    );
    error_contains(
        index_limits(bytes, ReadFormat::Cab, ReadLimits { max_memory: 32 * 1024, ..ReadLimits::default() }),
        "too large to read in memory",
    );
}
