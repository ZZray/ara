//! Fixed OMP596f2da ar/rar.ts metadata-only RAR4/5 index and lazy solid extraction.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
const RAR4: &[u8] = &[0x52, 0x61, 0x72, 0x21, 0x1a, 0x07, 0];
const RAR5: &[u8] = &[0x52, 0x61, 0x72, 0x21, 0x1a, 0x07, 1, 0];
const SCAN: u64 = 1024 * 1024 + 8;
fn fail(what: impl AsRef<str>) -> ToolError {
    error(format!("Invalid RAR archive: {}", what.as_ref()))
}
fn checked_end(start: u64, size: u64, limit: u64, what: &str) -> Result<u64> {
    if start > JS_SAFE || size > JS_SAFE {
        return Err(fail(format!("invalid {what} range")));
    }
    start
        .checked_add(size)
        .filter(|&end| end <= JS_SAFE && end <= limit)
        .ok_or_else(|| fail(format!("truncated {what}")))
}
trait Bytes {
    fn size(&self) -> u64;
    fn byte(&self, offset: u64) -> Result<u8>;
    fn slice(&self, start: u64, end: u64) -> Result<&[u8]>;
    fn u16(&self, offset: u64) -> Result<u16> {
        Ok(self.byte(offset)? as u16 | ((self.byte(offset + 1)? as u16) << 8))
    }
    fn u32(&self, offset: u64) -> Result<u32> {
        Ok(self.byte(offset)? as u32
            | ((self.byte(offset + 1)? as u32) << 8)
            | ((self.byte(offset + 2)? as u32) << 16)
            | ((self.byte(offset + 3)? as u32) << 24))
    }
}
struct Slice<'a>(&'a [u8]);
impl Bytes for Slice<'_> {
    fn size(&self) -> u64 {
        self.0.len() as u64
    }
    fn byte(&self, offset: u64) -> Result<u8> {
        // Fixed Uint8Array out-of-bounds undefined is zero in bitwise readers.
        // Sparse byte access, unlike ordinary header buffers, remains strict.
        Ok(self.0.get(offset as usize).copied().unwrap_or(0))
    }
    fn slice(&self, start: u64, end: u64) -> Result<&[u8]> {
        self.0
            .get(start as usize..end as usize)
            .ok_or_else(|| fail("RAR parser accessed member payload while indexing"))
    }
}
struct Segment {
    start: u64,
    bytes: Vec<u8>,
}
struct Sparse {
    size: u64,
    segments: Vec<Segment>,
}
impl Bytes for Sparse {
    fn size(&self) -> u64 {
        self.size
    }
    fn byte(&self, offset: u64) -> Result<u8> {
        Ok(self.slice(offset, offset + 1)?[0])
    }
    fn slice(&self, start: u64, end: u64) -> Result<&[u8]> {
        let upper = self.segments.partition_point(|segment| segment.start <= start);
        for segment in self.segments[..upper].iter().rev() {
            if start >= segment.start && end <= segment.start + segment.bytes.len() as u64 {
                return Ok(&segment.bytes[(start - segment.start) as usize..(end - segment.start) as usize]);
            }
        }
        Err(fail("RAR parser accessed member payload while indexing"))
    }
}
fn vint(bytes: &impl Bytes, cursor: &mut u64, end: u64, what: &str) -> Result<u64> {
    let mut value = 0u128;
    let mut factor = 1u128;
    for _ in 0..10 {
        if *cursor >= end {
            return Err(fail(format!("truncated {what}")));
        }
        let byte = bytes.byte(*cursor)?;
        *cursor += 1;
        value += (byte & 0x7f) as u128 * factor;
        if value > JS_SAFE as u128 {
            return Err(fail(format!("{what} is too large")));
        }
        if byte & 0x80 == 0 {
            return Ok(value as u64);
        }
        factor *= 128;
    }
    Err(fail(format!("{what} vint is too long")))
}
pub(super) fn sniff(bytes: &[u8]) -> bool {
    marker(bytes).is_some()
}
fn marker(bytes: &[u8]) -> Option<(u8, u64)> {
    for offset in 0..bytes.len().min(SCAN as usize) {
        if bytes.get(offset..offset + 8) == Some(RAR5) {
            return Some((5, offset as u64));
        }
        if bytes.get(offset..offset + 7) == Some(RAR4) {
            return Some((4, offset as u64));
        }
    }
    None
}
fn range(source: &Source, start: u64, end: u64) -> Result<Vec<u8>> {
    source::read_with_source_error(source, start, end - start, |err| err)
}
fn metadata(source: &Source, limits: &ReadLimits) -> Result<(Sparse, u8, u64)> {
    let size = source.size();
    if size > JS_SAFE {
        return Err(error("Archive is too large to read safely"));
    }
    let mut probe_end = size.min(8);
    let mut probe = range(source, 0, probe_end)?;
    let mut found = marker(&probe);
    while found.is_none() && probe_end < size.min(SCAN) {
        let next_end = size.min(SCAN).min((64 * 1024).max(probe_end * 2));
        limits.index(next_end, "RAR signature scan")?;
        let next = range(source, probe_end, next_end)?;
        let mut combined = vec![0; next_end as usize];
        combined[..probe.len()].copy_from_slice(&probe);
        combined[probe_end as usize..probe_end as usize + next.len()].copy_from_slice(&next);
        probe = combined;
        probe_end = next_end;
        found = marker(&probe);
    }
    let (version, marker) = found.ok_or_else(|| error("Invalid RAR archive: signature not found"))?;
    let mut amount = probe.len() as u64;
    limits.index(amount, "RAR metadata")?;
    let mut sparse = Sparse { size, segments: vec![Segment { start: 0, bytes: probe }] };
    let mut offset = marker + if version == 5 { 8 } else { 7 };
    while offset < size {
        if version == 5 {
            if offset + 5 > size {
                return Err(fail("truncated RAR5 header"));
            }
            let prefix = range(source, offset, size.min(offset + 16))?;
            let mut cursor = 4;
            let header_size = vint(&Slice(&prefix), &mut cursor, prefix.len() as u64, "header size")?;
            if header_size > 2 * 1024 * 1024 {
                return Err(fail("RAR5 header exceeds format limit"));
            }
            let header_end = checked_end(offset, cursor + header_size, size, "RAR5 header")?;
            limits.index(amount + header_end - offset, "RAR metadata")?;
            let header = range(source, offset, header_end)?;
            amount += header.len() as u64;
            limits.index(amount, "RAR metadata")?;
            let view = Slice(&header);
            let kind = vint(&view, &mut cursor, header.len() as u64, "header type")?;
            let flags = vint(&view, &mut cursor, header.len() as u64, "header flags")?;
            if flags & 1 != 0 {
                vint(&view, &mut cursor, header.len() as u64, "extra area size")?;
            }
            let data_size =
                if flags & 2 != 0 { vint(&view, &mut cursor, header.len() as u64, "data size")? } else { 0 };
            sparse.segments.push(Segment { start: offset, bytes: header });
            offset = checked_end(header_end, data_size, size, "RAR5 data area")?;
            if kind == 4 || kind == 5 {
                break;
            }
        } else {
            if offset + 7 > size {
                return Err(fail("truncated RAR4 base header"));
            }
            let prefix = range(source, offset, size.min(offset + 11))?;
            let view = Slice(&prefix);
            let kind = view.byte(2)?;
            let flags = view.u16(3)?;
            let header_size = view.u16(5)? as u64;
            if header_size < 7 {
                return Err(fail("invalid RAR4 header size"));
            }
            let header_end = checked_end(offset, header_size, size, "RAR4 header")?;
            limits.index(amount + header_end - offset, "RAR metadata")?;
            let header = range(source, offset, header_end)?;
            amount += header.len() as u64;
            limits.index(amount, "RAR metadata")?;
            let view = Slice(&header);
            let data_size = if flags & 0x8000 != 0 { view.u32(7)? as u64 } else { 0 };
            let data_end = checked_end(header_end, data_size, size, "RAR4 data area")?;
            if kind == 0x74 && data_size <= limits.max_path as u64 {
                let fields = if flags & 0x8000 != 0 { 11 } else { 7 };
                if fields + 21 <= header.len() as u64 {
                    let host = view.byte(fields + 4)?;
                    let attributes = view.u32(fields + 17)?;
                    if host == 3 && attributes & 0xf000 == 0xa000 {
                        limits.index(amount + data_size, "RAR metadata")?;
                        sparse
                            .segments
                            .push(Segment { start: header_end, bytes: range(source, header_end, data_end)? });
                        amount += data_size;
                        limits.index(amount, "RAR metadata")?;
                    }
                }
            }
            sparse.segments.push(Segment { start: offset, bytes: header });
            offset = data_end;
            if kind == 0x7b || (kind == 0x73 && flags & 0x80 != 0) {
                break;
            }
        }
    }
    sparse.segments.sort_by(|a, b| a.start.cmp(&b.start).then(b.bytes.len().cmp(&a.bytes.len())));
    Ok((sparse, version, marker))
}
#[derive(Clone)]
struct Record {
    format: u8,
    path: WireString,
    data_start: u64,
    packed: u64,
    unpacked: u64,
    method: u64,
    version: u64,
    dictionary: u64,
    solid: bool,
    crc: Option<u32>,
    directory: bool,
    mtime: Option<f64>,
    mode: Option<u32>,
    raw_mode: Option<u64>,
    link: Option<(WireString, bool)>,
}
struct Archive {
    source: Source,
    records: Vec<Record>,
    limits: ReadLimits,
    cache: Mutex<HashMap<usize, Vec<u8>>>,
}
impl Archive {
    fn decode(&self, index: usize, cache: &mut HashMap<usize, Vec<u8>>) -> Result<()> {
        let target = self.records.get(index).ok_or_else(|| error("Invalid RAR member index"))?;
        let mut start = index;
        if target.solid {
            while start > 0 && self.records[start].solid && self.records[start - 1].format == target.format {
                start -= 1;
            }
        }
        let mut rar4 = rar4_decoder::Decoder::default();
        let mut rar5 = rar5_decoder::Decoder::default();
        for current in start..=index {
            let record = &self.records[current];
            if record.directory {
                continue;
            }
            self.limits.member(record.unpacked, &record.path)?;
            if record.method == 0 && record.packed != record.unpacked {
                return Err(fail("stored member size mismatch"));
            }
            if record.method > 5 {
                return Err(error(if record.format == 4 {
                    format!(
                        "Unsupported RAR4 compression method 0x{:x} for '{}'",
                        record.method + 0x30,
                        utf8(&record.path)
                    )
                } else {
                    format!("Unsupported RAR5 compression method {} for '{}'", record.method, utf8(&record.path))
                }));
            }
            if record.method != 0 {
                self.limits.memory(record.packed + 2 * record.unpacked + 2 * record.dictionary + 8192)?;
            }
            let end = checked_end(record.data_start, record.packed, self.source.size(), "member data")?;
            let packed = range(&self.source, record.data_start, end)?;
            let output = if record.method == 0 {
                if record.solid {
                    if record.format == 4 {
                        rar4.reset();
                    } else {
                        rar5.reset();
                    }
                }
                packed
            } else if record.format == 5 {
                rar5.decode(
                    &packed,
                    record.unpacked as usize,
                    record.dictionary as usize,
                    record.solid,
                    record.version,
                )?
            } else {
                rar4.decode(
                    &packed,
                    record.unpacked as usize,
                    record.dictionary as usize,
                    record.solid,
                    record.version,
                )?
            };
            if output.len() as u64 != record.unpacked {
                return Err(fail(format!("member '{}' size mismatch", utf8(&record.path))));
            }
            if record.crc.is_some_and(|expected| crc32fast::hash(&output) != expected) {
                return Err(error(format!("RAR member '{}' CRC32 mismatch", utf8(&record.path))));
            }
            cache.insert(current, output);
        }
        Ok(())
    }
}
struct MemberSource {
    archive: Arc<Archive>,
    index: usize,
}
impl ReadMemberSource for MemberSource {
    fn read(&self, size: u64, path: &WireString) -> Result<Vec<u8>> {
        let mut cache = self.archive.cache.lock().map_err(|_| error("RAR extraction queue failed"))?;
        if !cache.contains_key(&self.index) {
            self.archive.decode(self.index, &mut cache)?;
        }
        let bytes =
            cache.get(&self.index).ok_or_else(|| error(format!("Failed to extract RAR member '{}'", utf8(path))))?;
        if bytes.len() as u64 != size {
            return Err(error(format!("RAR member '{}' size mismatch ({} != {size})", utf8(path), bytes.len())));
        }
        Ok(bytes.clone())
    }
}
fn latin1(bytes: &[u8]) -> WireString {
    encoding_rs::WINDOWS_1252.decode_with_bom_removal(bytes).0.into_owned().into()
}
fn utf8_fatal(bytes: &[u8], message: &str) -> Result<WireString> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    Ok(std::str::from_utf8(bytes).map_err(|_| error(message))?.into())
}
fn canonical(path: &WireString, target: &WireString) -> (WireString, bool) {
    let target = portable(target);
    if target.units().first() == Some(&47) {
        return (target, false);
    }
    let mut parts: Vec<Vec<u16>> = path.units().split(|&u| u == 47).map(|p| p.to_vec()).collect();
    parts.pop();
    for part in target.units().split(|&u| u == 47) {
        if part.is_empty() || part == [46] {
            continue;
        }
        if part == [46, 46] {
            if parts.is_empty() {
                return (target, false);
            }
            parts.pop();
        } else {
            parts.push(part.to_vec());
        }
    }
    (WireString::from_units(parts.join(&[47][..])), true)
}
fn filetime(bytes: &impl Bytes, offset: u64) -> Result<f64> {
    Ok((bytes.u32(offset)? as f64 + bytes.u32(offset + 4)? as f64 * 4294967296.0) / 10000.0 - 11644473600000.0)
}
fn dos_time(value: u32, local: &impl Fn(i32, u32, u32, u32, u32, u32) -> Result<f64>) -> Result<f64> {
    local(
        ((value >> 25) & 0x7f) as i32 + 1980,
        (value >> 21) & 15,
        (value >> 16) & 31,
        (value >> 11) & 31,
        (value >> 5) & 63,
        (value & 31) * 2,
    )
}
fn unicode_name(bytes: &[u8]) -> Result<WireString> {
    let Some(separator) = bytes.iter().position(|&b| b == 0) else {
        return Ok(latin1(bytes));
    };
    let mut separator = separator + 1;
    if separator >= bytes.len() {
        return Ok(latin1(&bytes[..separator - 1]));
    }
    let high = bytes[separator] as u16;
    separator += 1;
    let mut decoded = 0;
    let mut flags = 0u8;
    let mut flag_bits = 0;
    let mut output = Vec::new();
    while separator < bytes.len() {
        if flag_bits == 0 {
            flags = bytes[separator];
            separator += 1;
            flag_bits = 8;
            if separator >= bytes.len() {
                break;
            }
        }
        match flags >> 6 {
            0 => {
                output.push(bytes[separator] as u16);
                separator += 1;
                decoded += 1;
            }
            1 => {
                output.push(bytes[separator] as u16 + (high << 8));
                separator += 1;
                decoded += 1;
            }
            2 => {
                if separator + 1 >= bytes.len() {
                    return Err(fail("truncated RAR4 Unicode name"));
                }
                output.push(u16::from_le_bytes([bytes[separator], bytes[separator + 1]]));
                separator += 2;
                decoded += 1;
            }
            _ => {
                let mut count = bytes[separator] as usize;
                separator += 1;
                if count & 0x80 != 0 {
                    if separator >= bytes.len() {
                        return Err(fail("truncated RAR4 Unicode name correction"));
                    }
                    let correction = bytes[separator];
                    separator += 1;
                    count = (count & 0x7f) + 2;
                    for _ in 0..count {
                        if decoded >= bytes.len() {
                            break;
                        }
                        output.push((high << 8) | bytes[decoded].wrapping_add(correction) as u16);
                        decoded += 1;
                    }
                } else {
                    count += 2;
                    for _ in 0..count {
                        if decoded >= bytes.len() {
                            break;
                        }
                        output.push(bytes[decoded] as u16);
                        decoded += 1;
                    }
                }
            }
        }
        flags = flags.wrapping_shl(2);
        flag_bits -= 2;
    }
    Ok(WireString::from_units(output))
}
fn extra_records(bytes: &impl Bytes, start: u64, end: u64) -> Result<Vec<(u64, u64, u64)>> {
    let mut cursor = start;
    let mut records = Vec::new();
    while cursor < end {
        let size = vint(bytes, &mut cursor, end, "extra record size")?;
        let record_end = checked_end(cursor, size, end, "extra record")?;
        let kind = vint(bytes, &mut cursor, record_end, "extra record type")?;
        records.push((kind, cursor, record_end));
        cursor = record_end;
    }
    Ok(records)
}
fn parse5(bytes: &Sparse, marker: u64, limits: &ReadLimits) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    let mut offset = marker + 8;
    let mut main = false;
    while offset < bytes.size() {
        if offset + 5 > bytes.size() {
            return Err(fail("truncated RAR5 header"));
        }
        let expected = bytes.u32(offset)?;
        let size_at = offset + 4;
        let mut size_cursor = size_at;
        let header_size = vint(bytes, &mut size_cursor, bytes.size(), "header size")?;
        if header_size > 2 * 1024 * 1024 {
            return Err(fail("RAR5 header exceeds format limit"));
        }
        limits.index(header_size, "RAR5 header")?;
        let header_end = checked_end(size_cursor, header_size, bytes.size(), "RAR5 header")?;
        if crc32fast::hash(bytes.slice(size_at, header_end)?) != expected {
            return Err(fail("RAR5 header CRC32 mismatch"));
        }
        let mut cursor = size_cursor;
        let kind = vint(bytes, &mut cursor, header_end, "header type")?;
        let flags = vint(bytes, &mut cursor, header_end, "header flags")?;
        let extra_size = if flags & 1 != 0 { vint(bytes, &mut cursor, header_end, "extra area size")? } else { 0 };
        let data_size = if flags & 2 != 0 { vint(bytes, &mut cursor, header_end, "data size")? } else { 0 };
        if flags & 0x18 != 0 {
            return Err(error("Unsupported multi-volume RAR5 archive"));
        }
        let data_end = checked_end(header_end, data_size, bytes.size(), "RAR5 data area")?;
        if extra_size > header_end - cursor {
            return Err(fail("invalid RAR5 extra area size"));
        }
        let extra_start = header_end - extra_size;
        match kind {
            4 => return Err(error("Encrypted RAR5 headers are not supported")),
            1 => {
                let archive_flags = vint(bytes, &mut cursor, extra_start, "archive flags")?;
                if archive_flags & 1 != 0 {
                    return Err(error("Unsupported multi-volume RAR5 archive"));
                }
                if archive_flags & 8 != 0 {
                    return Err(error("Unsupported RAR5 recovery record"));
                }
                if archive_flags & 2 != 0 {
                    vint(bytes, &mut cursor, extra_start, "volume number")?;
                }
                main = true;
            }
            2 | 3 => {
                let file_flags = vint(bytes, &mut cursor, extra_start, "file flags")?;
                let unpacked = vint(bytes, &mut cursor, extra_start, "unpacked size")?;
                if file_flags & 8 != 0 {
                    return Err(error("RAR5 member with unknown unpacked size is not supported"));
                }
                let attributes = vint(bytes, &mut cursor, extra_start, "file attributes")?;
                let mut mtime = None;
                let mut crc = None;
                if file_flags & 2 != 0 {
                    checked_end(cursor, 4, extra_start, "RAR5 modification time")?;
                    mtime = Some(bytes.u32(cursor)? as f64 * 1000.0);
                    cursor += 4;
                }
                if file_flags & 4 != 0 {
                    checked_end(cursor, 4, extra_start, "RAR5 data CRC32")?;
                    crc = Some(bytes.u32(cursor)?);
                    cursor += 4;
                }
                let compression = vint(bytes, &mut cursor, extra_start, "compression information")? as u32;
                let mut version = (compression & 0x3f) as u64;
                if version == 1 && compression & 0x100000 != 0 {
                    version = 0;
                }
                let solid = compression & 0x40 != 0;
                let method = ((compression >> 7) & 7) as u64;
                let power = (compression >> 10) & 31;
                let mut dictionary = 128 * 1024u64 * (1u64 << power);
                if compression & 0x3f == 1 {
                    dictionary += dictionary * ((compression >> 15) & 31) as u64 / 32;
                }
                if dictionary > JS_SAFE || dictionary > limits.max_memory {
                    return Err(error(format!("RAR5 dictionary is too large ({dictionary} bytes)")));
                }
                let host = vint(bytes, &mut cursor, extra_start, "host OS")?;
                let name_size = vint(bytes, &mut cursor, extra_start, "file name size")?;
                limits.path(name_size as usize, "member path")?;
                checked_end(cursor, name_size, extra_start, "RAR5 file name")?;
                let raw_path = utf8_fatal(bytes.slice(cursor, cursor + name_size)?, "Invalid RAR5 UTF-8 member name")?;
                cursor += name_size;
                let mut link = None;
                for (extra_kind, start, end) in extra_records(bytes, extra_start, header_end)? {
                    let mut extra = start;
                    if extra_kind == 1 {
                        return Err(error(format!("Encrypted RAR5 member '{}' is not supported", utf8(&raw_path))));
                    }
                    if extra_kind == 3 {
                        let flags = vint(bytes, &mut extra, end, "time flags")?;
                        if flags & 2 != 0 {
                            if flags & 1 != 0 {
                                checked_end(extra, 4, end, "Unix modification time")?;
                                mtime = Some(bytes.u32(extra)? as f64 * 1000.0);
                            } else {
                                checked_end(extra, 8, end, "Windows modification time")?;
                                mtime = Some(filetime(bytes, extra)?);
                            }
                        }
                    } else if extra_kind == 5 {
                        let redirection = vint(bytes, &mut extra, end, "redirection type")?;
                        vint(bytes, &mut extra, end, "redirection flags")?;
                        let target_size = vint(bytes, &mut extra, end, "link target size")?;
                        limits.path(target_size as usize, "link target")?;
                        checked_end(extra, target_size, end, "link target")?;
                        if !(1..=5).contains(&redirection) {
                            return Err(error(format!("Unsupported RAR5 redirection type {redirection}")));
                        }
                        link = Some(utf8_fatal(
                            bytes.slice(extra, extra + target_size)?,
                            "Invalid RAR5 UTF-8 link target",
                        )?);
                    }
                }
                if kind == 2 {
                    limits.member(unpacked, &raw_path)?;
                    if let Some(path) = normalize(&raw_path, false) {
                        let link = link.map(|target| canonical(&path, &target));
                        records.push(Record {
                            format: 5,
                            path,
                            data_start: header_end,
                            packed: data_size,
                            unpacked,
                            method,
                            version,
                            dictionary,
                            solid,
                            crc,
                            directory: file_flags & 1 != 0,
                            mtime,
                            mode: if host == 1 { Some(attributes as u32) } else { None },
                            raw_mode: (host == 1 && attributes > u32::MAX as u64).then_some(attributes),
                            link,
                        });
                        limits.entries(records.len())?;
                    }
                } else if raw_path == WireString::from("RR") {
                    return Err(error("Unsupported RAR5 recovery record"));
                }
            }
            5 => {
                let flags = vint(bytes, &mut cursor, extra_start, "end flags")?;
                if flags & 1 != 0 {
                    return Err(error("Unsupported multi-volume RAR5 archive"));
                }
                break;
            }
            _ => {
                if flags & 4 == 0 {
                    return Err(error(format!("Unsupported RAR5 header type {kind}")));
                }
            }
        }
        offset = data_end;
    }
    if !main {
        return Err(fail("RAR5 main header is missing"));
    }
    Ok(records)
}
fn parse4(
    bytes: &Sparse,
    marker: u64,
    limits: &ReadLimits,
    local: &impl Fn(i32, u32, u32, u32, u32, u32) -> Result<f64>,
) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    let mut offset = marker + 7;
    let mut main = false;
    while offset < bytes.size() {
        checked_end(offset, 7, bytes.size(), "RAR4 base header")?;
        let header_crc = bytes.u16(offset)?;
        let kind = bytes.byte(offset + 2)?;
        let flags = bytes.u16(offset + 3)?;
        let header_size = bytes.u16(offset + 5)? as u64;
        if header_size < 7 {
            return Err(fail("invalid RAR4 header size"));
        }
        limits.index(header_size, "RAR4 header")?;
        let header_end = checked_end(offset, header_size, bytes.size(), "RAR4 header")?;
        if (crc32fast::hash(bytes.slice(offset + 2, header_end)?) & 0xffff) as u16 != header_crc {
            return Err(fail("RAR4 header CRC mismatch"));
        }
        let mut data_size = 0;
        let mut cursor = offset + 7;
        if flags & 0x8000 != 0 {
            checked_end(cursor, 4, header_end, "RAR4 additional size")?;
            data_size = bytes.u32(cursor)? as u64;
            cursor += 4;
        }
        let mut data_end = checked_end(header_end, data_size, bytes.size(), "RAR4 data area")?;
        match kind {
            0x73 => {
                if flags & 1 != 0 {
                    return Err(error("Unsupported multi-volume RAR4 archive"));
                }
                if flags & 0x40 != 0 {
                    return Err(error("Unsupported RAR4 recovery record"));
                }
                if flags & 0x80 != 0 {
                    return Err(error("Encrypted RAR4 headers are not supported"));
                }
                main = true;
            }
            0x74 => {
                checked_end(cursor, 21, header_end, "RAR4 file header")?;
                let low = bytes.u32(cursor)?;
                cursor += 4;
                let host = bytes.byte(cursor)?;
                cursor += 1;
                let crc = bytes.u32(cursor)?;
                cursor += 4;
                let dos = bytes.u32(cursor)?;
                cursor += 4;
                let version = bytes.byte(cursor)? as u64;
                cursor += 1;
                let method_byte = bytes.byte(cursor)?;
                cursor += 1;
                let name_size = bytes.u16(cursor)? as u64;
                cursor += 2;
                let attributes = bytes.u32(cursor)?;
                cursor += 4;
                let mut packed = data_size;
                let mut unpacked = low as u64;
                if flags & 0x100 != 0 {
                    checked_end(cursor, 8, header_end, "RAR4 high sizes")?;
                    packed += bytes.u32(cursor)? as u64 * 0x100000000;
                    cursor += 4;
                    unpacked += bytes.u32(cursor)? as u64 * 0x100000000;
                    cursor += 4;
                    data_end = checked_end(header_end, packed, bytes.size(), "RAR4 file data")?;
                }
                if flags & 3 != 0 {
                    return Err(error("Unsupported multi-volume RAR4 member"));
                }
                if flags & 4 != 0 {
                    return Err(error("Encrypted RAR4 file data is not supported"));
                }
                if !(0x30..=0x35).contains(&method_byte) {
                    return Err(error(format!("Unsupported RAR4 compression method 0x{method_byte:x}")));
                }
                limits.path(name_size as usize, "member path")?;
                checked_end(cursor, name_size, header_end, "RAR4 file name")?;
                let name = bytes.slice(cursor, cursor + name_size)?;
                let raw_path = if flags & 0x200 != 0 { unicode_name(name)? } else { latin1(name) };
                cursor += name_size;
                if flags & 0x400 != 0 {
                    checked_end(cursor, 8, header_end, "RAR4 salt")?;
                    cursor += 8;
                }
                let mut mtime = dos_time(dos, local)?;
                if flags & 0x1000 != 0 {
                    checked_end(cursor, 2, header_end, "RAR4 extended time flags")?;
                    let time_flags = bytes.u16(cursor)?;
                    cursor += 2;
                    for time_index in 0..4 {
                        let mode = (time_flags >> ((3 - time_index) * 4)) & 15;
                        if mode & 8 == 0 {
                            continue;
                        }
                        let mut time = dos;
                        if time_index != 0 {
                            checked_end(cursor, 4, header_end, "RAR4 extended time")?;
                            time = bytes.u32(cursor)?;
                            cursor += 4;
                        }
                        let mut precise = dos_time(time, local)? + if mode & 4 != 0 { 1000.0 } else { 0.0 };
                        let mut remainder = 0u32;
                        let count = (mode & 3) as u64;
                        checked_end(cursor, count, header_end, "RAR4 extended time precision")?;
                        for index in 0..count {
                            remainder |= (bytes.byte(cursor)? as u32) << ((index + 3 - count) * 8);
                            cursor += 1;
                        }
                        precise += remainder as f64 / 10000.0;
                        if time_index == 0 {
                            mtime = precise;
                        }
                    }
                }
                limits.member(unpacked, &raw_path)?;
                if let Some(path) = normalize(&raw_path, false) {
                    let directory = (flags >> 5) & 7 == 7;
                    let mode = if host == 3 { Some(attributes) } else { None };
                    let mut link = None;
                    if mode.is_some_and(|mode| mode & 0xf000 == 0xa000) {
                        if method_byte != 0x30 || packed != unpacked {
                            return Err(error(format!("Unsupported compressed RAR4 symlink '{}'", utf8(&path))));
                        }
                        limits.path(packed as usize, "link target")?;
                        link = Some(canonical(&path, &latin1(bytes.slice(header_end, data_end)?)));
                    }
                    let index = (flags >> 5) & 7;
                    let dictionary = if index == 7 { 4 * 1024 * 1024 } else { 64 * 1024 * (1u64 << index) };
                    records.push(Record {
                        format: 4,
                        path,
                        data_start: header_end,
                        packed,
                        unpacked,
                        method: (method_byte - 0x30) as u64,
                        version,
                        dictionary,
                        solid: flags & 0x10 != 0,
                        crc: Some(crc),
                        directory,
                        mtime: Some(mtime),
                        mode,
                        raw_mode: None,
                        link,
                    });
                    limits.entries(records.len())?;
                }
            }
            0x78 => return Err(error("Unsupported RAR4 recovery record")),
            0x7b => {
                if flags & 1 != 0 {
                    return Err(error("Unsupported multi-volume RAR4 archive"));
                }
                break;
            }
            _ => {}
        }
        offset = data_end;
    }
    if !main {
        return Err(fail("RAR4 main header is missing"));
    }
    Ok(records)
}
pub(super) fn read(
    source: Source,
    limits: ReadLimits,
    local: impl Fn(i32, u32, u32, u32, u32, u32) -> Result<f64>,
) -> Result<Index> {
    let (sparse, version, marker) = metadata(&source, &limits)?;
    let records =
        if version == 5 { parse5(&sparse, marker, &limits)? } else { parse4(&sparse, marker, &limits, &local)? };
    let archive = Arc::new(Archive { source, records, limits, cache: Mutex::new(HashMap::new()) });
    let mut index = Index::default();
    index.read_limits = Some(limits);
    for (record_index, record) in archive.records.iter().enumerate() {
        let storage = if let Some((target, resolve)) = &record.link {
            Some(Storage::Link { target: target.clone(), resolve: *resolve })
        } else if !record.directory {
            Some(Storage::ReadMember(Arc::new(MemberSource { archive: archive.clone(), index: record_index })))
        } else {
            None
        };
        let inherits_old_mode =
            index.get(&record.path).is_some_and(|old| old.directory == record.directory && record.mode.is_none());
        let accepted = index.upsert(Entry {
            path: record.path.clone(),
            directory: record.directory,
            size: record.unpacked,
            mode: record.mode,
            mtime: record.mtime,
            storage,
        })?;
        if accepted.is_none() {
            // Fixed upsert ignores a later directory over an existing file.
            continue;
        }
        if let Some(mode) = record.raw_mode {
            index.read_mode_overrides.insert(record.path.clone(), mode);
        } else if !inherits_old_mode {
            // New normal metadata or a directory-to-file replacement clears
            // the old override. Same-kind absent metadata inherits it above.
            index.read_mode_overrides.remove(&record.path);
        }
    }
    Ok(index)
}
