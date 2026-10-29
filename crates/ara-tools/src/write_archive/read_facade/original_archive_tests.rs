//! Fixed OMP596f2da packages/utils/test/ar assertion ports.
//! Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! Rust execution and original-case coverage are recorded separately by Root.
use super::*;
use sha2::{Digest, Sha256};
use std::sync::Mutex;

#[path = "original_tests/asar.rs"]
mod asar_cases;
#[path = "original_tests/cab.rs"]
mod cab_cases;
#[path = "original_tests/checksums.rs"]
mod checksums_cases;
#[path = "original_tests/codecs.rs"]
mod codecs_cases;
#[path = "original_tests/cpio.rs"]
mod cpio_cases;
#[path = "original_tests/iso.rs"]
mod iso_cases;
#[path = "original_tests/legacy_dos.rs"]
mod legacy_dos_cases;
#[path = "original_tests/rar.rs"]
mod rar_cases;
#[path = "original_tests/sevenzip.rs"]
mod sevenzip_cases;
#[path = "original_tests/source.rs"]
mod source_cases;
#[path = "original_tests/tar.rs"]
mod tar_cases;
#[path = "original_tests/unix_ar.rs"]
mod unix_ar_cases;
#[path = "original_tests/xz.rs"]
mod xz_cases;
#[path = "original_tests/zip.rs"]
mod zip_cases;

fn fixture(name: &str) -> Vec<u8> {
    // Shared WRITE gzip/TAR loads the pinned corpus; oracles are original assertions.
    static FIXTURES: std::sync::LazyLock<Index> = std::sync::LazyLock::new(|| {
        tar::read(codecs::gunzip(include_bytes!("fixtures/ar.tar.gz"), 64 * 1024 * 1024).unwrap()).unwrap()
    });
    FIXTURES.read_file(&name.into()).unwrap()
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
// Deterministic UTC parser oracle, distinct from Root's real Host/TZ trials.
fn utc_date(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Result<f64> {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    Ok((days * 86400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second)) as f64 * 1000.0)
}
fn open_result(bytes: Vec<u8>, format: ReadFormat) -> Result<Reader> {
    Reader::open_source(source::source_from_bytes(bytes), format, None, ReadLimits::default(), utc_date)
}
fn open(bytes: Vec<u8>, format: ReadFormat) -> Reader {
    open_result(bytes, format).unwrap()
}
fn member(reader: &Reader, path: &str) -> Vec<u8> {
    reader.read_file(&path.into()).unwrap().bytes
}
fn node(reader: &Reader, path: &str) -> Node {
    reader.get_node(&path.into()).unwrap().unwrap()
}
fn list(reader: &Reader, path: &str) -> Vec<String> {
    reader
        .list_directory(&path.into(), |a, b| a.units().cmp(b.units()))
        .unwrap()
        .iter()
        .map(|e| utf8(&e.name))
        .collect()
}
fn index_limits(bytes: Vec<u8>, format: ReadFormat, limits: ReadLimits) -> Result<Index> {
    registry::read_index(source::source_from_bytes(bytes), format, None, limits, utc_date)
}
fn index(bytes: Vec<u8>, format: ReadFormat) -> Index {
    index_limits(bytes, format, ReadLimits::default()).unwrap()
}
fn index_source(source: Source, format: ReadFormat, archive_path: Option<&Path>, limits: ReadLimits) -> Result<Index> {
    registry::read_index(source, format, archive_path, limits, utc_date)
}
fn entry<'a>(index: &'a Index, path: &str) -> &'a Entry {
    index.get(&path.into()).expect("original fixture entry")
}
fn index_member(index: &Index, path: &str) -> Vec<u8> {
    index.read_file(&path.into()).unwrap()
}
fn index_member_result(index: &Index, path: &str) -> Result<Vec<u8>> {
    index.read_file(&path.into())
}
fn index_member_sized(index: &Index, path: &str, size: u64) -> Result<Vec<u8>> {
    match entry(index, path).storage.as_ref().unwrap() {
        Storage::ReadMember(member) => member.read(size, &path.into()),
        Storage::Zip(member) => member.read(size, &path.into()),
        _ => Err(error("original sized member needs selected member source")),
    }
}
fn paths(index: &Index) -> Vec<String> {
    index.entries.iter().map(|e| utf8(&e.path)).collect()
}
fn assert_link(index: &Index, path: &str, target: &str, resolve: bool) {
    match entry(index, path).storage.as_ref().unwrap() {
        Storage::Link { target: actual, resolve: actual_resolve } => {
            assert_eq!(utf8(actual), target);
            assert_eq!(*actual_resolve, resolve);
        }
        _ => panic!("expected original link {path}"),
    }
}
fn error_contains<T>(result: Result<T>, expected: &str) {
    let actual = result.err().expect("expected archive failure").0;
    assert!(actual.contains(expected), "expected {expected:?}, got {actual:?}");
}
fn find_bytes(bytes: &[u8], needle: &[u8]) -> usize {
    bytes.windows(needle.len()).position(|w| w == needle).expect("original fixture needle")
}
fn read16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn read32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn write32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
struct Counting {
    bytes: Vec<u8>,
    reads: Arc<Mutex<Vec<(u64, u64)>>>,
    short: bool,
}
impl ReadByteSource for Counting {
    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
    fn range(&self, start: u64, len: u64) -> Result<Vec<u8>> {
        let end = start + len;
        self.reads.lock().unwrap().push((start, end));
        if end > self.size() {
            return Err(error("test range out of bounds"));
        }
        let end = if self.short && len > 0 { end - 1 } else { end };
        Ok(self.bytes[start as usize..end as usize].to_vec())
    }
}
fn counting(bytes: Vec<u8>) -> (Source, Arc<Mutex<Vec<(u64, u64)>>>) {
    let reads = Arc::new(Mutex::new(Vec::new()));
    (Source::Read(Arc::new(Counting { bytes, reads: reads.clone(), short: false })), reads)
}
