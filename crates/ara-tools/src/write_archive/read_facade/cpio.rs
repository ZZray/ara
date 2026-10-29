//! Fixed OMP596f2da ar/cpio.ts materialized stream and lazy CRC members.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;

const FILE_TYPE_MASK: u32 = 0o170000;
const REGULAR: u32 = 0o100000;
const DIRECTORY: u32 = 0o040000;
const SYMLINK: u32 = 0o120000;
struct Header {
    header_size: usize,
    alignment: usize,
    inode: u64,
    mode: u64,
    nlink: u64,
    mtime: u64,
    file_size: u64,
    dev_major: u64,
    dev_minor: u64,
    name_size: u64,
    checksum: Option<u32>,
}
struct Record {
    path: Option<WireString>,
    mode: u32,
    mtime: f64,
    nlink: u64,
    inode: u64,
    dev_major: u64,
    dev_minor: u64,
    file_size: u64,
    data_offset: usize,
    checksum: Option<u32>,
}
fn require_range(bytes: &[u8], start: usize, end: usize, what: &str) -> Result<()> {
    if start > end || end as u64 > JS_SAFE || end > bytes.len() {
        Err(error(format!("Invalid CPIO archive: truncated {what}")))
    } else {
        Ok(())
    }
}
fn digits(bytes: &[u8], offset: usize, length: usize, radix: u64, field: &str) -> Result<u64> {
    require_range(bytes, offset, offset + length, &format!("{field} field"))?;
    let mut value = 0u64;
    for &code in &bytes[offset..offset + length] {
        let digit = match code {
            b'0'..=b'9' => u64::from(code - b'0'),
            b'A'..=b'F' if radix == 16 => u64::from(code - b'A' + 10),
            b'a'..=b'f' if radix == 16 => u64::from(code - b'a' + 10),
            _ => return Err(error(format!("Invalid CPIO archive: {field} is not a valid base-{radix} number"))),
        };
        if digit >= radix {
            return Err(error(format!("Invalid CPIO archive: {field} is not a valid base-{radix} number")));
        }
        value = value
            .checked_mul(radix)
            .and_then(|v| v.checked_add(digit))
            .filter(|&v| v <= JS_SAFE)
            .ok_or_else(|| error(format!("Invalid CPIO archive: {field} is too large")))?;
    }
    Ok(value)
}
fn parse_header(bytes: &[u8], offset: usize) -> Result<Header> {
    require_range(bytes, offset, offset + 2, "header")?;
    let first = bytes[offset];
    let second = bytes[offset + 1];
    if (first == 0xc7 && second == 0x71) || (first == 0x71 && second == 0xc7) {
        require_range(bytes, offset, offset + 26, "old binary header")?;
        let read16 = |field: usize| {
            let pair = [bytes[offset + field], bytes[offset + field + 1]];
            u64::from(if first == 0xc7 { u16::from_le_bytes(pair) } else { u16::from_be_bytes(pair) })
        };
        let words = |field| read16(field) * 0x10000 + read16(field + 2);
        return Ok(Header {
            header_size: 26,
            alignment: 2,
            dev_major: 0,
            dev_minor: read16(2),
            inode: read16(4),
            mode: read16(6),
            nlink: read16(12),
            mtime: words(16),
            name_size: read16(20),
            file_size: words(22),
            checksum: None,
        });
    }
    require_range(bytes, offset, offset + 6, "magic")?;
    let magic = &bytes[offset..offset + 6];
    if magic == b"070701" || magic == b"070702" {
        require_range(bytes, offset, offset + 110, "new ASCII header")?;
        let field = |index: usize, name| digits(bytes, offset + 6 + index * 8, 8, 16, name);
        let checksum = field(12, "checksum")? as u32;
        if magic == b"070701" && checksum != 0 {
            return Err(error("Invalid CPIO archive: newc checksum field must be zero"));
        }
        return Ok(Header {
            header_size: 110,
            alignment: 4,
            inode: field(0, "inode")?,
            mode: field(1, "mode")?,
            nlink: field(4, "link count")?,
            mtime: field(5, "modification time")?,
            file_size: field(6, "file size")?,
            dev_major: field(7, "device major")?,
            dev_minor: field(8, "device minor")?,
            name_size: field(11, "name size")?,
            checksum: (magic == b"070702").then_some(checksum),
        });
    }
    if magic == b"070707" {
        require_range(bytes, offset, offset + 76, "portable ASCII header")?;
        let field = |field_offset, name| digits(bytes, offset + field_offset, 6, 8, name);
        return Ok(Header {
            header_size: 76,
            alignment: 1,
            dev_major: 0,
            dev_minor: field(6, "device")?,
            inode: field(12, "inode")?,
            mode: field(18, "mode")?,
            nlink: field(36, "link count")?,
            mtime: digits(bytes, offset + 48, 11, 8, "modification time")?,
            name_size: field(59, "name size")?,
            file_size: digits(bytes, offset + 65, 11, 8, "file size")?,
            checksum: None,
        });
    }
    Err(error(format!("Invalid CPIO archive: unsupported or corrupt magic at offset {offset}")))
}
fn align(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}
fn decode_utf8(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes).ok().map(|s| s.strip_prefix('\u{feff}').unwrap_or(s))
}
fn zero_padding(bytes: &[u8], start: usize, end: usize, what: &str) -> Result<()> {
    if bytes[start..end].iter().any(|&byte| byte != 0) {
        Err(error(format!("Invalid CPIO archive: non-zero {what} padding")))
    } else {
        Ok(())
    }
}
fn checksum(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0u32, |sum, &byte| sum.wrapping_add(u32::from(byte)))
}

struct CpioMemberSource {
    bytes: Arc<Vec<u8>>,
    offset: usize,
    size: u64,
    checksum: Option<u32>,
}
impl ReadMemberSource for CpioMemberSource {
    fn read(&self, size: u64, path: &WireString) -> Result<Vec<u8>> {
        if size != self.size {
            return Err(error(format!("CPIO member '{}' has an inconsistent declared size", utf8(path))));
        }
        let end = self.offset + self.size as usize;
        if end > self.bytes.len() {
            return Err(error(format!("CPIO member '{}' is truncated", utf8(path))));
        }
        let bytes = &self.bytes[self.offset..end];
        if self.checksum.is_some_and(|expected| checksum(bytes) != expected) {
            return Err(error(format!("CPIO member '{}' has an invalid CRC checksum", utf8(path))));
        }
        Ok(bytes.to_vec())
    }
}
fn member(bytes: &Arc<Vec<u8>>, record: &Record) -> Storage {
    Storage::ReadMember(Arc::new(CpioMemberSource {
        bytes: bytes.clone(),
        offset: record.data_offset,
        size: record.file_size,
        checksum: record.checksum,
    }))
}
fn link_target(path: &WireString, target: &[u8], limits: &ReadLimits) -> Result<(WireString, bool)> {
    limits.path(target.len(), "link target")?;
    let target = decode_utf8(target)
        .filter(|s| !s.contains('\0'))
        .ok_or_else(|| error(format!("Invalid CPIO archive: symlink '{}' has an invalid UTF-8 target", utf8(path))))?;
    let portable_target = portable(&WireString::from(target));
    if starts_slash(&portable_target) {
        return Ok((portable_target, false));
    }
    let normalized = normalize(&posix_join(&dirname(path), &portable_target), true);
    Ok(match normalized {
        Some(target) => (target, true),
        None => (portable_target, false),
    })
}

pub(super) fn read_buffer(bytes: Vec<u8>, limits: &ReadLimits) -> Result<Index> {
    limits.memory(bytes.len() as u64)?;
    let bytes = Arc::new(bytes);
    let mut records = Vec::<Record>::new();
    let mut offset = 0;
    let mut metadata_size = 0;
    let mut found_trailer = false;
    while offset < bytes.len() {
        let header = parse_header(&bytes, offset)?;
        if header.mode > 0xffff {
            return Err(error("Invalid CPIO archive: mode exceeds 16 bits"));
        }
        if header.name_size < 1 {
            return Err(error("Invalid CPIO archive: name size must include a NUL terminator"));
        }
        limits.path(header.name_size as usize - 1, "member path")?;
        limits.member(header.file_size, &WireString::from("(CPIO entry)"))?;
        let name_start = offset + header.header_size;
        let name_end = name_start + header.name_size as usize;
        let data_offset = align(name_end, header.alignment);
        let data_end = data_offset + header.file_size as usize;
        let next_offset = align(data_end, header.alignment);
        require_range(&bytes, name_start, name_end, "member name")?;
        require_range(&bytes, data_offset, data_end, "member data")?;
        require_range(&bytes, data_end, next_offset, "member padding")?;
        if bytes[name_end - 1] != 0 {
            return Err(error("Invalid CPIO archive: member name is not NUL-terminated"));
        }
        if bytes[name_start..name_end - 1].contains(&0) {
            return Err(error("Invalid CPIO archive: member name contains an embedded NUL"));
        }
        zero_padding(&bytes, name_end, data_offset, "name")?;
        zero_padding(&bytes, data_end, next_offset, "data")?;
        metadata_size += data_offset - offset;
        limits.index(metadata_size as u64, "CPIO index")?;
        let raw_name = decode_utf8(&bytes[name_start..name_end - 1]);
        if raw_name == Some("TRAILER!!!") {
            if header.file_size != 0 {
                return Err(error("Invalid CPIO archive: TRAILER!!! has non-empty data"));
            }
            found_trailer = true;
            offset = next_offset;
            break;
        }
        let file_type = header.mode as u32 & FILE_TYPE_MASK;
        if (file_type == DIRECTORY || file_type == 0o010000) && header.file_size != 0 {
            return Err(error("Invalid CPIO archive: directory or FIFO has non-empty data"));
        }
        limits.entries(records.len() + 1)?;
        records.push(Record {
            path: raw_name.and_then(|name| normalize(&WireString::from(name), false)),
            mode: header.mode as u32,
            mtime: header.mtime as f64 * 1000.0,
            nlink: header.nlink,
            inode: header.inode,
            dev_major: header.dev_major,
            dev_minor: header.dev_minor,
            file_size: header.file_size,
            data_offset,
            checksum: header.checksum,
        });
        offset = next_offset;
    }
    if !found_trailer {
        return Err(error("Invalid CPIO archive: missing TRAILER!!! terminator"));
    }
    zero_padding(&bytes, offset, bytes.len(), "trailing")?;
    let mut index = Index { read_limits: Some(*limits), ..Index::default() };
    let mut handled = HashSet::new();
    let mut group_indices = HashMap::new();
    let mut groups = Vec::<Vec<usize>>::new();
    for (i, record) in records.iter().enumerate() {
        if record.mode & FILE_TYPE_MASK != REGULAR || record.nlink <= 1 {
            continue;
        }
        let key = (record.dev_major, record.dev_minor, record.inode);
        let group = *group_indices.entry(key).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[group].push(i);
    }
    for group in &groups {
        if group.len() < 2 {
            continue;
        }
        handled.extend(group.iter().copied());
        let retained: Vec<_> = group.iter().copied().filter(|&i| records[i].path.is_some()).collect();
        if retained.is_empty() {
            continue;
        }
        let payload = group.iter().copied().find(|&i| records[i].file_size != 0).unwrap_or(group[0]);
        let canonical = retained.iter().copied().find(|&i| i == payload).unwrap_or(retained[0]);
        for i in retained {
            let record = &records[i];
            index.upsert(Entry {
                path: record.path.clone().unwrap(),
                directory: false,
                size: records[payload].file_size,
                mtime: Some(record.mtime),
                mode: Some(record.mode),
                storage: Some(if i == canonical {
                    member(&bytes, &records[payload])
                } else {
                    Storage::Link { target: records[canonical].path.clone().unwrap(), resolve: false }
                }),
            })?;
        }
    }
    for (i, record) in records.iter().enumerate() {
        let Some(path) = &record.path else { continue };
        if handled.contains(&i) {
            continue;
        }
        let file_type = record.mode & FILE_TYPE_MASK;
        let (directory, size, storage) = match file_type {
            DIRECTORY => (true, 0, None),
            SYMLINK => {
                let target_bytes = &bytes[record.data_offset..record.data_offset + record.file_size as usize];
                if record.checksum.is_some_and(|expected| checksum(target_bytes) != expected) {
                    return Err(error(format!("CPIO symlink '{}' has an invalid CRC checksum", utf8(path))));
                }
                let (target, resolve) = link_target(path, target_bytes, limits)?;
                (false, 0, Some(Storage::Link { target, resolve }))
            }
            REGULAR => (false, record.file_size, Some(member(&bytes, record))),
            _ => continue,
        };
        index.upsert(Entry {
            path: path.clone(),
            directory,
            size,
            mode: Some(record.mode),
            mtime: Some(record.mtime),
            storage,
        })?;
    }
    index.parents()?;
    Ok(index)
}
pub(super) fn read(source: Source, limits: &ReadLimits) -> Result<Index> {
    limits.memory(source.size())?;
    read_buffer(source.range(0, source.size())?, limits)
}
pub(super) fn sniff(bytes: &[u8]) -> bool {
    let Ok(header) = parse_header(bytes, 0) else { return false };
    if header.mode > 0xffff || header.name_size < 1 {
        return false;
    }
    let name_end = header.header_size + header.name_size as usize;
    name_end <= bytes.len() && bytes[name_end - 1] == 0
}
