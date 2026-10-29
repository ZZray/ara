//! Fixed OMP596f2da ar/registry.ts private READ decoder composition.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
use formats::ReadFormat;

fn decompress(method: &str, bytes: &[u8], maximum: u64) -> Result<Vec<u8>> {
    match method {
        "gzip" => codecs::gunzip(bytes, maximum),
        "bzip2" => codecs::read_bzip2(bytes, maximum),
        "xz" => codecs::read_xz(bytes, maximum),
        "zstd" => codecs::unzstd(bytes, maximum),
        "compress" => lzw::decompress(bytes, maximum),
        "lzma" => codecs::read_lzma_alone(bytes, maximum),
        _ => Err(error(format!("Unsupported archive compression '{method}'"))),
    }
}
struct BufferMember {
    bytes: Arc<Vec<u8>>,
}
impl ReadMemberSource for BufferMember {
    fn read(&self, _size: u64, _path: &WireString) -> Result<Vec<u8>> {
        Ok(self.bytes.as_ref().clone())
    }
}
fn compressed(source: Source, method: &str, archive_path: Option<&Path>, limits: ReadLimits) -> Result<Index> {
    limits.memory(source.size())?;
    let inner = decompress(method, &source.range(0, source.size())?, limits.max_memory)?;
    limits.memory(inner.len() as u64)?;
    if tar::sniff_for_read(&inner, limits) {
        return tar::read_for_read(inner, limits);
    }
    let mut index = Index { read_limits: Some(limits), ..Index::default() };
    index.upsert(Entry {
        path: formats::stem_member_name(archive_path),
        directory: false,
        size: inner.len() as u64,
        mode: None,
        mtime: None,
        storage: Some(Storage::ReadMember(Arc::new(BufferMember { bytes: Arc::new(inner) }))),
    })?;
    Ok(index)
}
fn read_tar_source(source: Source, limits: ReadLimits) -> Result<Index> {
    limits.memory(source.size())?;
    let bytes = read_with_source_error(&source, 0, source.size(), |err| err)?;
    if bytes.len() as u64 != source.size() {
        return Err(error("Invalid archive: truncated data"));
    }
    tar::read_for_read(bytes, limits)
}
pub(super) fn read_index(
    source: Source,
    format: ReadFormat,
    archive_path: Option<&Path>,
    limits: ReadLimits,
    local_date: impl Fn(i32, u32, u32, u32, u32, u32) -> Result<f64>,
) -> Result<Index> {
    match format {
        ReadFormat::Zip => zip::read_for_read(&source, limits, &local_date),
        ReadFormat::Tar => read_tar_source(source, limits),
        ReadFormat::TarGz | ReadFormat::Gz => compressed(source, "gzip", archive_path, limits),
        ReadFormat::TarBz2 | ReadFormat::Bz2 => compressed(source, "bzip2", archive_path, limits),
        ReadFormat::TarXz | ReadFormat::Xz => compressed(source, "xz", archive_path, limits),
        ReadFormat::TarZst | ReadFormat::Zst => compressed(source, "zstd", archive_path, limits),
        ReadFormat::TarZ | ReadFormat::Z => compressed(source, "compress", archive_path, limits),
        ReadFormat::Lzma => compressed(source, "lzma", archive_path, limits),
        ReadFormat::Asar => asar::read_for_read(&source, archive_path, limits),
        ReadFormat::Rar => rar::read(source, limits, local_date),
        ReadFormat::SevenZip => sevenzip::read(&source, &limits),
        ReadFormat::Iso => iso::read(source, &limits),
        ReadFormat::Cab => cab::read(source, &limits, Arc::new(mszip::inflate), local_date),
        ReadFormat::Cpio => cpio::read(source, &limits),
        ReadFormat::Rpm => rpm::read(source, &limits, decompress),
        ReadFormat::Ar => unix_ar::read(&source, &limits),
        ReadFormat::Deb => deb::read(source, &limits, decompress, |bytes, limits| tar::read_for_read(bytes, *limits)),
        ReadFormat::Lzh => lzh::read(&source, &limits),
        ReadFormat::Arj => arj::read(&source, &limits),
    }
}
pub(crate) fn sniff_archive_format(bytes: &[u8]) -> Option<ReadFormat> {
    if bytes.len() >= 4 {
        let signature = u32::from_le_bytes(bytes[..4].try_into().unwrap());
        if matches!(signature, 0x04034b50 | 0x06054b50 | 0x08074b50) { return Some(ReadFormat::Zip); }
    }
    let probes: &[(ReadFormat, fn(&[u8]) -> bool)] = &[
        (ReadFormat::Rar, rar::sniff), (ReadFormat::SevenZip, sevenzip::sniff),
        (ReadFormat::Cab, cab::sniff), (ReadFormat::Rpm, rpm::sniff),
        (ReadFormat::Arj, arj::sniff), (ReadFormat::Lzh, lzh::sniff),
        (ReadFormat::Cpio, cpio::sniff), (ReadFormat::Deb, deb::sniff),
        (ReadFormat::Ar, unix_ar::sniff),
    ];
    for &(format, sniff) in probes { if sniff(bytes) { return Some(format); } }
    if bytes.len() >= 17 {
        let word = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let json_size = word(12) as u64;
        let payload = word(8) as u64;
        if word(0) == 4 && word(4) >= 8 && word(4) as u64 == payload + 4
            && payload == 4 + json_size.div_ceil(4) * 4 && json_size > 0 && bytes[16] == b'{' {
            return Some(ReadFormat::Asar);
        }
    }
    if bytes.starts_with(&[0x1f, 0x8b]) { return Some(ReadFormat::TarGz); }
    if bytes.len() >= 4 && bytes.starts_with(b"BZh") && (b'1'..=b'9').contains(&bytes[3]) { return Some(ReadFormat::TarBz2); }
    if bytes.starts_with(&[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0]) { return Some(ReadFormat::TarXz); }
    if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) { return Some(ReadFormat::TarZst); }
    if lzw::is_compress_z(bytes) { return Some(ReadFormat::TarZ); }
    if iso::sniff(bytes) { return Some(ReadFormat::Iso); }
    if tar::sniff_for_read(bytes, ReadLimits::default()) { return Some(ReadFormat::Tar); }
    if bytes.len() >= 22 {
        for offset in (bytes.len().saturating_sub(22 + 65535)..=bytes.len() - 22).rev() {
            if bytes[offset..offset + 4] == [0x50, 0x4b, 0x05, 0x06] { return Some(ReadFormat::Zip); }
        }
    }
    None
}
