//! Fixed OMP596f2da ZIP/ZIP64 read-for-rewrite and deterministic encoder.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
use super::read_facade::{ReadLimits, legacy_limits::Policy};
const LOCAL: u32 = 0x04034b50;
const CENTRAL: u32 = 0x02014b50;
const EOCD: u32 = 0x06054b50;
const ZIP64: u32 = 0x06064b50;
const LOCATOR: u32 = 0x07064b50;
const U32_MAX: u64 = 0xffff_ffff;
const UTF8: u16 = 0x800;
const ENCRYPTED: u16 = 0x41;
fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64> {
    let value = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    if value > JS_SAFE { Err(error("Archive uses offsets or sizes too large to read safely")) } else { Ok(value) }
}
fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn checked_end(start: i128, size: u64, archive: u64, what: &str) -> Result<u64> {
    if start < 0 || start > JS_SAFE as i128 || size > JS_SAFE {
        return Err(error(format!("Invalid ZIP archive: {what} has an invalid range")));
    }
    let end = start + size as i128;
    if end > JS_SAFE as i128 || end > archive as i128 {
        return Err(error(format!("Invalid ZIP archive: {what} exceeds archive size")));
    }
    Ok(end as u64)
}
struct Directory {
    entries: u64,
    size: u64,
    offset: u64,
    physical_end: u64,
    archive_offset: i128,
}
fn zip64_info(source: &Source, tail: &[u8], tail_start: u64, eocd: u64, policy: Policy) -> Result<Option<Directory>> {
    let Some(locator_offset) = eocd.checked_sub(20) else {
        return Ok(None);
    };
    let locator = if locator_offset >= tail_start {
        tail[(locator_offset - tail_start) as usize..(locator_offset - tail_start + 20) as usize].to_vec()
    } else {
        read_range(source, locator_offset, 20, policy)?
    };
    if policy.0.is_some() && locator.len() != 20 { return Ok(None); }
    if u32_at(&locator, 0) != LOCATOR {
        return Ok(None);
    }
    if u32_at(&locator, 4) != 0 || u32_at(&locator, 16) != 1 {
        return Err(error("Multi-volume ZIP archives are not supported"));
    }
    let declared = u64_at(&locator, 8)?;
    let mut candidates = vec![declared];
    if let Some(adjacent) = locator_offset.checked_sub(56) {
        if adjacent != declared {
            candidates.push(adjacent);
        }
    }
    for candidate in candidates {
        if candidate.checked_add(56).is_none_or(|end| end > source.size()) {
            continue;
        }
        let record = read_range(source, candidate, 56, policy)?;
        if policy.0.is_some() && record.len() != 56 { continue; }
        if u32_at(&record, 0) != ZIP64 {
            continue;
        }
        let extended = u64_at(&record, 4)?;
        if extended < 44 || candidate.checked_add(12).and_then(|n| n.checked_add(extended)) != Some(locator_offset) {
            continue;
        }
        if u32_at(&record, 16) != 0 || u32_at(&record, 20) != 0 {
            return Err(error("Multi-volume ZIP archives are not supported"));
        }
        let entries = u64_at(&record, 32)?;
        if u64_at(&record, 24)? != entries {
            return Err(error("Multi-volume ZIP archives are not supported"));
        }
        return Ok(Some(Directory {
            entries,
            size: u64_at(&record, 40)?,
            offset: u64_at(&record, 48)?,
            physical_end: candidate,
            archive_offset: 0,
        }));
    }
    Err(error("Invalid ZIP archive: missing ZIP64 end of central directory"))
}
fn directory_info(source: &Source, policy: Policy) -> Result<Directory> {
    if source.size() < 22 {
        return Err(error("Invalid ZIP archive: missing end of central directory"));
    }
    let length = source.size().min(22 + 65535);
    let start = source.size() - length;
    let tail = read_range(source, start, length, policy)?;
    if policy.0.is_some() && tail.len() as u64 != length {
        return Err(error("Invalid ZIP archive: truncated end of central directory"));
    }
    let eocd = (0..=tail.len() - 22)
        .rev()
        .find(|&offset| {
            u32_at(&tail, offset) == EOCD && offset + 22 + u16_at(&tail, offset + 20) as usize == tail.len()
        })
        .ok_or_else(|| error("Invalid ZIP archive: missing end of central directory"))?;
    let disk = u16_at(&tail, eocd + 4);
    let central_disk = u16_at(&tail, eocd + 6);
    let on_disk = u16_at(&tail, eocd + 8);
    let entries = u16_at(&tail, eocd + 10);
    let size = u32_at(&tail, eocd + 12) as u64;
    let offset = u32_at(&tail, eocd + 16) as u64;
    if disk != 0 || central_disk != 0 || (on_disk != 65535 && entries != 65535 && on_disk != entries) {
        return Err(error("Multi-volume ZIP archives are not supported"));
    }
    let needs64 = on_disk == 65535 || entries == 65535 || size == U32_MAX || offset == U32_MAX;
    let mut info = if let Some(info) = zip64_info(source, &tail, start, start + eocd as u64, policy)? {
        info
    } else {
        if needs64 {
            return Err(error("Invalid ZIP archive: missing ZIP64 central-directory metadata"));
        }
        Directory { entries: entries as u64, size, offset, physical_end: start + eocd as u64, archive_offset: 0 }
    };
    policy.count(usize::try_from(info.entries).unwrap_or(usize::MAX))?;
    policy.index(info.size, "ZIP central directory")?;
    if info.entries > info.size / 46 {
        return Err(error("Invalid ZIP archive: truncated central directory"));
    }
    let declared = info.offset;
    if info.entries != 0 {
        let mut candidates = vec![info.offset as i128];
        let adjacent = info.physical_end as i128 - info.size as i128;
        if adjacent != info.offset as i128 {
            candidates.push(adjacent);
        }
        let mut located = None;
        for offset in candidates {
            if offset < 0 || offset + 4 > source.size() as i128 || offset + info.size as i128 > source.size() as i128 {
                continue;
            }
            let signature = read_range(source, offset as u64, 4, policy)?;
            if (policy.0.is_none() || signature.len() == 4) && u32_at(&signature, 0) == CENTRAL {
                located = Some(offset as u64);
                break;
            }
        }
        info.offset =
            located.ok_or_else(|| error("Invalid ZIP archive: central directory is out of bounds or malformed"))?;
    }
    info.archive_offset = info.offset as i128 - declared as i128;
    checked_end(info.offset as i128, info.size, source.size(), "central directory")?;
    Ok(info)
}
#[derive(Default)]
struct Extra {
    zip64: Option<Vec<u8>>,
    unicode: Option<WireString>,
    mtime: Option<f64>,
}
fn ntfs_mtime(data: &[u8]) -> Result<Option<f64>> {
    if data.len() < 4 {
        return Ok(None);
    }
    let mut offset = 4;
    while offset + 4 <= data.len() {
        let tag = u16_at(data, offset);
        let size = u16_at(data, offset + 2) as usize;
        let end = offset + 4 + size;
        if end > data.len() {
            return Err(error("Invalid ZIP archive: malformed NTFS extra field"));
        }
        if tag == 1 && size >= 8 {
            let value = u64::from_le_bytes(data[offset + 4..offset + 12].try_into().unwrap());
            let milliseconds = (value / 10000) as i128 - 11_644_473_600_000i128;
            return Ok((milliseconds.unsigned_abs() <= JS_SAFE as u128).then_some(milliseconds as f64));
        }
        offset = end;
    }
    Ok(None)
}
fn extra_fields(bytes: &[u8], raw_name: &[u8]) -> Result<Extra> {
    let mut out = Extra::default();
    let mut offset = 0;
    while offset < bytes.len() {
        if offset + 4 > bytes.len() {
            return Err(error("Invalid ZIP archive: truncated extra-field header"));
        }
        let id = u16_at(bytes, offset);
        let size = u16_at(bytes, offset + 2) as usize;
        let end = offset + 4 + size;
        if end > bytes.len() {
            return Err(error("Invalid ZIP archive: malformed extra field"));
        }
        let data = &bytes[offset + 4..end];
        match id {
            1 => out.zip64 = Some(data.to_vec()),
            0x7075 => {
                if data.len() < 5 {
                    return Err(error("Invalid ZIP archive: Unicode path extra field is too small"));
                }
                if data[0] == 1 && u32_at(data, 1) == crc32fast::hash(raw_name) {
                    if let Some(decoded) = encoding_rs::UTF_8.decode_without_bom_handling_and_without_replacement(
                        data[5..].strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&data[5..]),
                    ) {
                        out.unicode = Some(decoded.into_owned().into());
                    }
                }
            }
            0x5455 => {
                if data.is_empty() || (data[0] & 1 != 0 && data.len() < 5) {
                    return Err(error("Invalid ZIP archive: extended timestamp extra field is too small"));
                }
                if data[0] & 1 != 0 {
                    out.mtime = Some((u32_at(data, 1) as i32) as f64 * 1000.0);
                }
            }
            0x000a if out.mtime.is_none() => out.mtime = ntfs_mtime(data)?,
            _ => {}
        }
        offset = end;
    }
    Ok(out)
}
fn values64(extra: Option<&[u8]>, size: u64, compressed: u64, offset: u64, disk: u32) -> Result<(u64, u64, u64, u32)> {
    if size != U32_MAX && compressed != U32_MAX && offset != U32_MAX && disk != 65535 {
        return Ok((size, compressed, offset, disk));
    }
    let bytes = extra.ok_or_else(|| error("Invalid ZIP archive: missing ZIP64 extra field"))?;
    let mut pos = 0;
    let mut next = |current: u64| -> Result<u64> {
        if current != U32_MAX {
            return Ok(current);
        }
        if pos + 8 > bytes.len() {
            return Err(error("Invalid ZIP archive: malformed ZIP64 extra field"));
        }
        let value = u64_at(bytes, pos)?;
        pos += 8;
        Ok(value)
    };
    let size = next(size)?;
    let compressed = next(compressed)?;
    let offset = next(offset)?;
    let disk = if disk == 65535 {
        if pos + 4 > bytes.len() {
            return Err(error("Invalid ZIP archive: malformed ZIP64 extra field"));
        }
        u32_at(bytes, pos)
    } else {
        disk
    };
    Ok((size, compressed, offset, disk))
}
#[derive(Clone)]
pub(super) struct Member {
    source: Source,
    compressed: u64,
    method: u16,
    flags: u16,
    crc: u32,
    local_offset: i128,
    policy: Policy,
}
impl Member {
    pub(super) fn read(&self, size: u64, path: &WireString) -> Result<Vec<u8>> {
        self.policy.member(size.max(self.compressed), path)?;
        let path = utf8(path);
        if self.flags & ENCRYPTED != 0 || self.method == 99 {
            return Err(error(format!("Encrypted ZIP member '{path}' is not supported")));
        }
        if !matches!(self.method, 0 | 8 | 12 | 14 | 20 | 93 | 95) {
            return Err(error(format!("Unsupported ZIP compression method {} for '{path}'", self.method)));
        }
        checked_end(self.local_offset, 30, self.source.size(), &format!("local header for '{path}'"))?;
        let header = self.read_range(self.local_offset as u64, 30, &path)?;
        if (self.policy.0.is_some() && header.len() != 30) || u32_at(&header, 0) != LOCAL {
            return Err(error(format!("Invalid ZIP archive: malformed local header for '{path}'")));
        }
        if u16_at(&header, 6) & ENCRYPTED != 0 {
            return Err(error(format!("Encrypted ZIP member '{path}' is not supported")));
        }
        if u16_at(&header, 8) != self.method {
            return Err(error(format!(
                "Invalid ZIP archive: local and central compression methods disagree for '{path}'"
            )));
        }
        let start = self.local_offset + 30 + u16_at(&header, 26) as i128 + u16_at(&header, 28) as i128;
        checked_end(start, self.compressed, self.source.size(), &format!("data for '{path}'"))?;
        if self.method == 0 && self.compressed != size {
            return Err(error(format!(
                "Invalid ZIP archive: size mismatch for '{path}' (expected {size}, got {})",
                self.compressed
            )));
        }
        let bytes = self.read_range(start as u64, self.compressed, &path)?;
        if self.policy.0.is_some() && bytes.len() as u64 != self.compressed {
            return Err(error(format!("Invalid ZIP archive: truncated data for '{path}'")));
        }
        let decoded = if self.policy.0.is_some() && self.method == 12 {
            codecs::read_bzip2(&bytes, size)?
        } else {
            codecs::zip(&bytes, self.method, size, &path.as_str().into())?
        };
        if decoded.len() as u64 != size {
            return Err(error(format!(
                "Invalid ZIP archive: size mismatch for '{path}' (expected {size}, got {})",
                decoded.len()
            )));
        }
        if crc32fast::hash(&decoded) != self.crc {
            return Err(error(format!("Invalid ZIP archive: CRC mismatch for '{path}'")));
        }
        Ok(decoded)
    }
    fn read_range(&self, start: u64, length: u64, path: &str) -> Result<Vec<u8>> {
        if self.policy.0.is_none() { return self.source.range(start, length); }
        super::read_facade::read_with_source_error(&self.source, start, length,
            |err| error(format!("Failed to read ZIP member '{path}': {}", err.0)))
    }
}
fn read_range(source: &Source, start: u64, length: u64, policy: Policy) -> Result<Vec<u8>> {
    if policy.0.is_none() { return source.range(start, length); }
    super::read_facade::read_with_source_error(source, start, length,
        |err| error(format!("Failed to read ZIP archive: {}", err.0)))
}
pub(super) fn read(source: &Source) -> Result<Index> {
    read_impl(source, Policy::default(), None)
}
pub(super) fn read_for_read(
    source: &Source,
    limits: ReadLimits,
    local_date: &impl Fn(i32, u32, u32, u32, u32, u32) -> Result<f64>,
) -> Result<Index> {
    read_impl(source, Policy(Some(limits)), Some(local_date))
}
fn dos_mtime(time: u16, date: u16, convert: &dyn Fn(i32, u32, u32, u32, u32, u32) -> Result<f64>) -> Result<Option<f64>> {
    if date == 0 { return Ok(None); }
    let year = 1980 + (date >> 9) as i32;
    let month = (date >> 5) & 0x0f;
    let day = date & 0x1f;
    let hour = time >> 11;
    let minute = (time >> 5) & 0x3f;
    let second = (time & 0x1f) * 2;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return Ok(None);
    }
    let milliseconds = convert(year, month as u32, day as u32, hour as u32, minute as u32, second as u32)?;
    Ok(milliseconds.is_finite().then_some(milliseconds))
}
fn read_impl(
    source: &Source,
    policy: Policy,
    local_date: Option<&dyn Fn(i32, u32, u32, u32, u32, u32) -> Result<f64>>,
) -> Result<Index> {
    let info = directory_info(source, policy)?;
    let mut index = Index::default();
    index.read_limits = policy.0;
    if info.size == 0 {
        return Ok(index);
    }
    let directory = read_range(source, info.offset, info.size, policy)?;
    if policy.0.is_some() && directory.len() as u64 != info.size {
        return Err(error("Invalid ZIP archive: truncated central directory"));
    }
    let mut offset = 0usize;
    let mut parsed = Vec::new();
    for _ in 0..info.entries {
        if offset + 46 > directory.len() {
            return Err(error("Invalid ZIP archive: truncated central directory"));
        }
        if u32_at(&directory, offset) != CENTRAL {
            return Err(error("Invalid ZIP archive: malformed central directory"));
        }
        let flags = u16_at(&directory, offset + 8);
        let method = u16_at(&directory, offset + 10);
        let attributes = u32_at(&directory, offset + 38);
        let name_length = u16_at(&directory, offset + 28) as usize;
        let extra_length = u16_at(&directory, offset + 30) as usize;
        let comment_length = u16_at(&directory, offset + 32) as usize;
        let name_start = offset + 46;
        let extra_start = name_start + name_length;
        let comment_start = extra_start + extra_length;
        let end = comment_start + comment_length;
        if end > directory.len() {
            return Err(error("Invalid ZIP archive: truncated central-directory entry"));
        }
        policy.path_bytes(name_length, "member path")?;
        let name_bytes = &directory[name_start..extra_start];
        let extra = extra_fields(&directory[extra_start..comment_start], name_bytes)?;
        let (size, compressed, local, disk) = values64(
            extra.zip64.as_deref(),
            u32_at(&directory, offset + 24) as u64,
            u32_at(&directory, offset + 20) as u64,
            u32_at(&directory, offset + 42) as u64,
            u16_at(&directory, offset + 34) as u32,
        )?;
        if disk != 0 {
            return Err(error("Multi-volume ZIP archives are not supported"));
        }
        let raw = extra.unicode.unwrap_or_else(|| {
            let value = if flags & UTF8 != 0 {
                if policy.0.is_some() { encoding_rs::UTF_8.decode_with_bom_removal(name_bytes).0 }
                else { encoding_rs::UTF_8.decode(name_bytes).0 }
            } else {
                encoding_rs::WINDOWS_1252.decode_without_bom_handling(name_bytes).0
            };
            value.into_owned().into()
        });
        policy.path_string(&raw, "member path")?;
        if flags & ENCRYPTED != 0 || method == 99 {
            return Err(error(format!("Encrypted ZIP member '{}' is not supported", utf8(&raw))));
        }
        if let Some(path) = normalize(&raw, false) {
            policy.member(size.max(compressed), &path)?;
            let host = u16_at(&directory, offset + 4) >> 8;
            let mode = matches!(host, 3 | 19).then_some(attributes >> 16);
            let kind = mode.unwrap_or(0) & 0o170000;
            let is_directory =
                matches!(raw.units().last(), Some(47 | 92)) || kind == 0o040000 || attributes & 0x10 != 0;
            let symlink = kind == 0o120000 && !is_directory;
            let local_offset = local as i128 + info.archive_offset;
            checked_end(local_offset, 30, source.size(), &format!("local header for '{}'", utf8(&path)))?;
            let member = Member {
                source: source.clone(),
                compressed,
                method,
                flags,
                crc: u32_at(&directory, offset + 16),
                local_offset,
                policy,
            };
            parsed.push((
                Entry {
                    path,
                    directory: is_directory,
                    size: if is_directory { 0 } else { size },
                    mode: mode.filter(|&m| m != 0),
                    mtime: match (extra.mtime, local_date) {
                        (None, Some(local_date)) => dos_mtime(u16_at(&directory, offset + 12), u16_at(&directory, offset + 14), local_date)?,
                        (mtime, _) => mtime,
                    },
                    storage: (!is_directory).then_some(Storage::Zip(member)),
                },
                symlink,
            ));
            policy.count(parsed.len())?;
        }
        offset = end;
    }
    for (mut entry, symlink) in parsed {
        if symlink {
            policy.path_bytes(entry.size as usize, "symlink target")?;
            let Some(Storage::Zip(member)) = &entry.storage else {
                return Err(error("Invalid ZIP archive: symlink has no payload"));
            };
            let bytes = member.read(entry.size, &entry.path)?;
            let target = if policy.0.is_some() { encoding_rs::UTF_8.decode_with_bom_removal(&bytes).0 } else { encoding_rs::UTF_8.decode(&bytes).0 };
            let target: WireString = target.into_owned().into();
            policy.path_string(&target, "symlink target")?;
            let target = portable(&target);
            let absolute = starts_slash(&target) || drive(&target, false) || target.units().contains(&0);
            let resolved = if absolute { None } else { normalize(&posix_join(&dirname(&entry.path), &target), true) };
            entry.size = 0;
            entry.storage = Some(Storage::Link { resolve: resolved.is_some(), target: resolved.unwrap_or(target) });
        }
        index.upsert(entry)?;
    }
    Ok(index)
}
pub(super) fn encode(members: &[(WireString, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut local = Vec::new();
    let mut central = Vec::new();
    let mut count = 0u64;
    for (input, data) in members {
        let portable = portable(input);
        let normalized = normalize(&portable, false);
        let comparison =
            WireString::from_units(portable.units().strip_prefix(&[46, 47]).unwrap_or(portable.units()).to_vec());
        let name = normalized
            .filter(|name| {
                name == &comparison
                    && !starts_slash(&portable)
                    && !drive(&portable, false)
                    && !portable.units().contains(&0)
            })
            .ok_or_else(|| error(format!("Cannot write unsafe ZIP member path '{}'", utf8(input))))?;
        let name_bytes = utf8(&name).into_bytes();
        if name_bytes.len() > 65535 {
            return Err(error(format!("ZIP member path '{}' is too long to write", utf8(&name))));
        }
        if data.len() as u64 >= U32_MAX {
            return Err(error(format!("ZIP member '{}' is too large to write", utf8(&name))));
        }
        let deflated = if data.is_empty() { None } else { Some(codecs::deflate(data)?) };
        let (payload, method) = match &deflated {
            Some(bytes) if bytes.len() < data.len() => (bytes.as_slice(), 8),
            _ => (data.as_slice(), 0),
        };
        if payload.len() as u64 >= U32_MAX || local.len() as u64 >= U32_MAX {
            return Err(error("ZIP archive is too large to write member offsets safely"));
        }
        let crc = crc32fast::hash(data);
        let local_offset = local.len();
        let mut header = vec![0; 30 + name_bytes.len()];
        put32(&mut header, 0, LOCAL);
        put16(&mut header, 4, 20);
        put16(&mut header, 6, UTF8);
        put16(&mut header, 8, method);
        put16(&mut header, 12, 0x21);
        put32(&mut header, 14, crc);
        put32(&mut header, 18, payload.len() as u32);
        put32(&mut header, 22, data.len() as u32);
        put16(&mut header, 26, name_bytes.len() as u16);
        header[30..].copy_from_slice(&name_bytes);
        local.extend_from_slice(&header);
        local.extend_from_slice(payload);
        let mut header = vec![0; 46 + name_bytes.len()];
        put32(&mut header, 0, CENTRAL);
        put16(&mut header, 4, (3 << 8) | 20);
        put16(&mut header, 6, 20);
        put16(&mut header, 8, UTF8);
        put16(&mut header, 10, method);
        put16(&mut header, 14, 0x21);
        put32(&mut header, 16, crc);
        put32(&mut header, 20, payload.len() as u32);
        put32(&mut header, 24, data.len() as u32);
        put16(&mut header, 28, name_bytes.len() as u16);
        put32(&mut header, 38, 0o100644 << 16);
        put32(&mut header, 42, local_offset as u32);
        header[46..].copy_from_slice(&name_bytes);
        central.extend_from_slice(&header);
        count += 1;
        if local.len() as u64 + central.len() as u64 > JS_SAFE || count > U32_MAX {
            return Err(error("ZIP archive is too large to write"));
        }
    }
    if local.len() as u64 >= U32_MAX || central.len() as u64 >= U32_MAX {
        return Err(error("ZIP archive is too large to write member offsets safely"));
    }
    let local_size = local.len() as u64;
    let central_size = central.len() as u64;
    let zip64 = count >= 65535;
    let trailer = 22 + if zip64 { 76 } else { 0 };
    if local_size + central_size + trailer as u64 > JS_SAFE {
        return Err(error("ZIP archive is too large to write"));
    }
    local.extend_from_slice(&central);
    if zip64 {
        let record_offset = local.len() as u64;
        let mut record = vec![0; 56];
        put32(&mut record, 0, ZIP64);
        put64(&mut record, 4, 44);
        put16(&mut record, 12, 45);
        put16(&mut record, 14, 45);
        put64(&mut record, 24, count);
        put64(&mut record, 32, count);
        put64(&mut record, 40, central_size);
        put64(&mut record, 48, local_size);
        local.extend_from_slice(&record);
        let mut record = vec![0; 20];
        put32(&mut record, 0, LOCATOR);
        put64(&mut record, 8, record_offset);
        put32(&mut record, 16, 1);
        local.extend_from_slice(&record);
    }
    let mut record = vec![0; 22];
    put32(&mut record, 0, EOCD);
    let end_count = if zip64 { 65535 } else { count as u16 };
    put16(&mut record, 8, end_count);
    put16(&mut record, 10, end_count);
    put32(&mut record, 12, central_size as u32);
    put32(&mut record, 16, local_size as u32);
    local.extend_from_slice(&record);
    Ok(local)
}
