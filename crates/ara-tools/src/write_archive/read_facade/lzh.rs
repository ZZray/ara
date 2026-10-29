//! Fixed OMP596f2da ar/lzh.ts LZH/LHA container and selected-member decoders.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64> {
    let value = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    if value > JS_SAFE { Err(error("LZH uses sizes too large to read safely")) } else { Ok(value) }
}
fn range(bytes: &[u8], start: usize, end: usize, what: &str) -> Result<()> {
    if start > end || end > bytes.len() || end as u64 > JS_SAFE {
        Err(error(format!("Invalid LZH archive: truncated {what}")))
    } else {
        Ok(())
    }
}
fn method_valid(method: &str) -> bool {
    matches!(
        method,
        "-lh0-"
            | "-lh1-"
            | "-lh2-"
            | "-lh3-"
            | "-lh4-"
            | "-lh5-"
            | "-lh6-"
            | "-lh7-"
            | "-lhd-"
            | "-lz4-"
            | "-lz5-"
            | "-lzs-"
    )
}
const CRC16_TABLE: [u16; 256] = {
    let mut table = [0u16; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u16;
        let mut bit = 0;
        while bit < 8 {
            value = (value >> 1) ^ if value & 1 != 0 { 0xa001 } else { 0 };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
};
pub(super) fn crc16(bytes: &[u8], seed: u16) -> u16 {
    let mut value = seed;
    for &byte in bytes {
        value = (value >> 8) ^ CRC16_TABLE[((value ^ byte as u16) & 0xff) as usize];
    }
    value
}
fn crc16_zero_range(bytes: &[u8], start: Option<usize>) -> u16 {
    let Some(start) = start else {
        return crc16(bytes, 0);
    };
    crc16(&bytes[start + 2..], crc16(&[0, 0], crc16(&bytes[..start], 0)))
}
pub(super) fn legacy(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    encoding_rs::WINDOWS_1252.decode_with_bom_removal(&bytes[..end]).0.into_owned()
}
fn utf16(bytes: &[u8]) -> Result<String> {
    if bytes.len() & 1 != 0 {
        return Err(error("Invalid LZH Unicode path header: odd UTF-16 length"));
    }
    let mut end = bytes.len();
    while end >= 2 && bytes[end - 2..end] == [0, 0] {
        end -= 2;
    }
    Ok(encoding_rs::UTF_16LE.decode_with_bom_removal(&bytes[..end]).0.into_owned())
}
pub(super) fn dos_time(value: u32) -> Option<f64> {
    if value == 0 {
        return None;
    }
    let year = 1980 + ((value >> 25) & 0x7f) as i64;
    let month = ((value >> 21) & 0x0f) as i64;
    let day = ((value >> 16) & 0x1f) as i64;
    let hour = ((value >> 11) & 0x1f) as i64;
    let minute = ((value >> 5) & 0x3f) as i64;
    let second = (value & 0x1f) as i64 * 2;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    // Date.UTC normalizes day 29..31 into the next month rather than rejecting.
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + (153 * mp + 2) / 5 + day - 1 - 719468;
    Some((days * 86400000 + hour * 3600000 + minute * 60000 + second * 1000) as f64)
}
#[derive(Default)]
struct Extended {
    filename: Option<String>,
    directory: Option<String>,
    unicode_filename: Option<String>,
    unicode_directory: Option<String>,
    mtime: Option<f64>,
    mode: Option<u32>,
    packed_size: Option<u64>,
    size: Option<u64>,
    common_crc: Option<u16>,
    common_crc_offset: Option<usize>,
}
fn extended(kind: u8, data: &[u8], absolute: usize, fields: &mut Extended) -> Result<()> {
    match kind {
        0x00 => {
            if data.len() < 2 {
                return Err(error("Invalid LZH common extended header"));
            }
            fields.common_crc = Some(u16_at(data, 0));
            fields.common_crc_offset = Some(absolute);
        }
        0x01 => fields.filename = Some(legacy(data)),
        0x02 => fields.directory = Some(legacy(data).replace('ÿ', "/")),
        0x39 => return Err(error("Multi-volume LZH archives are unsupported")),
        0x41 => {
            if data.len() >= 16 {
                let time = u32_at(data, 8) as u64 + ((u32_at(data, 12) as u64) << 32);
                if time <= JS_SAFE {
                    fields.mtime = Some(time as f64 / 10000.0 - 11644473600000.0);
                }
            }
        }
        0x42 => {
            if data.len() < 16 {
                return Err(error("Invalid LZH 64-bit size extended header"));
            }
            fields.packed_size = Some(u64_at(data, 0)?);
            fields.size = Some(u64_at(data, 8)?);
        }
        0x44 => fields.unicode_filename = Some(utf16(data)?),
        0x45 => fields.unicode_directory = Some(utf16(data)?.replace('ÿ', "/")),
        0x50 => {
            if data.len() < 2 {
                return Err(error("Invalid LZH Unix permissions extended header"));
            }
            fields.mode = Some(u16_at(data, 0) as u32);
        }
        0x54 => {
            if data.len() < 4 {
                return Err(error("Invalid LZH Unix timestamp extended header"));
            }
            fields.mtime = Some(u32_at(data, 0) as f64 * 1000.0);
        }
        _ => {}
    }
    Ok(())
}
fn path_header(kind: u8, bytes: &[u8], offset: usize, limits: &ReadLimits, fields: &mut Extended) -> Result<()> {
    if matches!(kind, 0x01 | 0x02 | 0x44 | 0x45) {
        limits.path(bytes.len(), "member path")?;
    }
    extended(kind, bytes, offset, fields)
}
struct Header {
    method: String,
    packed_size: usize,
    size: u64,
    data_start: usize,
    next: usize,
    crc: u16,
    path: Option<WireString>,
    mtime: Option<f64>,
    mode: Option<u32>,
}
fn header(bytes: &[u8], offset: usize, limits: &ReadLimits) -> Result<Header> {
    range(bytes, offset, offset + 22, "header")?;
    let level = bytes[offset + 20];
    if level > 2 {
        return Err(error(format!("Unsupported LZH header level {level}")));
    }
    let method: String = bytes[offset + 2..offset + 7].iter().map(|&b| b as char).collect();
    if !method_valid(&method) {
        return Err(error(format!("Invalid LZH compression method '{method}'")));
    }
    let mut packed = u32_at(bytes, offset + 7) as u64;
    let mut size = u32_at(bytes, offset + 11) as u64;
    let crc;
    let mut filename = None;
    let mut mtime;
    let mut os_id = 0;
    let data_start;
    let mut fields = Extended::default();
    if level < 2 {
        let length = bytes[offset] as usize;
        let minimum = if level == 0 { 22 } else { 25 };
        if length < minimum {
            return Err(error(format!("Invalid LZH level-{level} header size")));
        }
        let base_end = offset + length + 2;
        range(bytes, offset, base_end, "header")?;
        let sum = bytes[offset + 2..base_end].iter().fold(0u8, |sum, &b| sum.wrapping_add(b));
        if sum != bytes[offset + 1] {
            return Err(error("Invalid LZH header checksum"));
        }
        let name_length = bytes[offset + 21] as usize;
        if 22 + name_length + 2 > length + 2 {
            return Err(error("Invalid LZH filename length"));
        }
        limits.path(name_length, "member path")?;
        filename = Some(legacy(&bytes[offset + 22..offset + 22 + name_length]));
        crc = u16_at(bytes, offset + 22 + name_length);
        mtime = dos_time(u32_at(bytes, offset + 15));
        if level == 0 {
            let extended_start = offset + 24 + name_length;
            if base_end - extended_start >= 12 {
                let ext = &bytes[extended_start..base_end];
                if matches!(ext[0], 0x55 | 0x4b) && ext[1] == 0 {
                    os_id = ext[0];
                    fields.mtime = Some(u32_at(ext, 2) as f64 * 1000.0);
                    fields.mode = Some(u16_at(ext, ext.len() - 6) as u32);
                }
            }
            data_start = base_end;
        } else {
            // JS Uint8Array access past the file returns undefined; the value
            // only participates in the later LHARK equality check.
            os_id = bytes.get(offset + 24 + name_length).copied().unwrap_or(0);
            let mut extension_size = u16_at(bytes, base_end - 2) as usize;
            let mut cursor = base_end;
            let mut total = 0usize;
            let mut count = 0;
            while extension_size != 0 {
                if extension_size < 3 {
                    return Err(error("Invalid LZH extended header size"));
                }
                range(bytes, cursor, cursor + extension_size, "extended header")?;
                total += extension_size;
                limits.index((length + 2 + total) as u64, "header metadata")?;
                count += 1;
                if count > 65535 {
                    return Err(error("Invalid LZH archive: too many extended headers"));
                }
                let end = cursor + extension_size - 2;
                path_header(bytes[cursor], &bytes[cursor + 1..end], cursor + 1, limits, &mut fields)?;
                let current = extension_size;
                extension_size = u16_at(bytes, end) as usize;
                cursor += current;
            }
            data_start = base_end + total;
            packed = match fields.packed_size {
                Some(value) => value,
                None => packed.checked_sub(total as u64).ok_or_else(|| error("Invalid LZH level-1 packed size"))?,
            };
        }
    } else {
        let length = u16_at(bytes, offset) as usize;
        if length < 26 {
            return Err(error("Invalid LZH level-2 header size"));
        }
        let end = offset + length;
        range(bytes, offset, end, "header")?;
        crc = u16_at(bytes, offset + 21);
        os_id = bytes[offset + 23];
        mtime = Some(u32_at(bytes, offset + 15) as f64 * 1000.0);
        let mut cursor = offset + 24;
        let mut count = 0;
        while cursor + 2 <= end {
            let extension_size = u16_at(bytes, cursor) as usize;
            if extension_size == 0 {
                cursor += 2;
                break;
            }
            if extension_size < 3 || cursor + extension_size > end {
                return Err(error("Invalid LZH extended header size"));
            }
            count += 1;
            if count > 65535 {
                return Err(error("Invalid LZH archive: too many extended headers"));
            }
            path_header(
                bytes[cursor + 2],
                &bytes[cursor + 3..cursor + extension_size],
                cursor + 3,
                limits,
                &mut fields,
            )?;
            cursor += extension_size;
        }
        if cursor != end {
            return Err(error("Invalid LZH level-2 extended header chain"));
        }
        data_start = end;
        packed = fields.packed_size.unwrap_or(packed);
    }
    if level == 1 && os_id == 0x20 && method == "-lh7-" {
        return Err(error("LZH uses the incompatible LHARK -lh7- variant"));
    }
    size = fields.size.unwrap_or(size);
    mtime = fields.mtime.or(mtime);
    if let Some(crc) = fields.common_crc {
        let relative = fields.common_crc_offset.unwrap() - offset;
        if crc16_zero_range(&bytes[offset..data_start], Some(relative)) != crc {
            return Err(error("Invalid LZH common header CRC-16"));
        }
    }
    let filename = fields.unicode_filename.or(fields.filename).or(filename).unwrap_or_default();
    let directory = fields.unicode_directory.or(fields.directory).unwrap_or_default();
    let raw = format!(
        "{directory}{}{filename}",
        if !directory.is_empty() && !directory.ends_with(['/', '\\']) { "/" } else { "" }
    );
    let raw: WireString = raw.into();
    limits.path(path_len(&raw), "member path")?;
    let unnamed = raw.is_empty().then(|| WireString::from("<unnamed>"));
    limits.member(size, unnamed.as_ref().unwrap_or(&raw))?;
    let packed = usize::try_from(packed).map_err(|_| error("LZH uses sizes too large to read safely"))?;
    let next = data_start.checked_add(packed).ok_or_else(|| error("Invalid LZH archive: truncated member data"))?;
    range(bytes, data_start, next, "member data")?;
    Ok(Header {
        method,
        packed_size: packed,
        size,
        data_start,
        next,
        crc,
        path: normalize(&raw, false),
        mtime,
        mode: fields.mode,
    })
}
fn lzs(packed: &[u8], size: usize) -> Result<Vec<u8>> {
    let mut reader = lh_static::MsbBitReader::new(packed, "LZH -lzs-");
    let mut output = vec![0u8; size];
    let mut history = [0x20u8; 2048];
    let mut history_position = 2048 - 17;
    let mut position = 0;
    while position < size {
        if reader.read(1)? != 0 {
            let value = reader.read(8)? as u8;
            output[position] = value;
            position += 1;
            history[history_position] = value;
            history_position = (history_position + 1) & 2047;
        } else {
            let from = reader.read(11)?;
            let length = reader.read(4)? + 2;
            if length > size - position {
                return Err(error("Invalid LZH -lzs- data: match exceeds declared size"));
            }
            for index in 0..length {
                if position >= size {
                    return Err(error("Invalid LZH -lzs- data: output exceeds declared size"));
                }
                let value = history[(from + index) & 2047];
                output[position] = value;
                position += 1;
                history[history_position] = value;
                history_position = (history_position + 1) & 2047;
            }
        }
    }
    reader.zero_padding()?;
    Ok(output)
}
struct LzhMember {
    archive: Arc<Vec<u8>>,
    start: usize,
    packed_size: usize,
    method: String,
    crc: u16,
}
impl ReadMemberSource for LzhMember {
    fn read(&self, size: u64, path: &WireString) -> Result<Vec<u8>> {
        let packed = &self.archive[self.start..self.start + self.packed_size];
        let size = usize::try_from(size).map_err(|_| error("LZH uses sizes too large to read safely"))?;
        let output = match self.method.as_str() {
            "-lh0-" | "-lz4-" => {
                if packed.len() != size {
                    return Err(error(format!("LZH member '{}' has inconsistent stored size", utf8(path))));
                }
                packed.to_vec()
            }
            "-lh4-" => lh_static::decode(packed, size, 1 << 12, 4, 13, "LZH -lh4-")?,
            "-lh5-" => lh_static::decode(packed, size, 1 << 13, 4, 14, "LZH -lh5-")?,
            "-lh6-" => lh_static::decode(packed, size, 1 << 15, 5, 16, "LZH -lh6-")?,
            "-lh7-" => lh_static::decode(packed, size, 1 << 16, 5, 17, "LZH -lh7-")?,
            "-lzs-" => lzs(packed, size)?,
            "-lh1-" => {
                return Err(error(format!(
                    "LZH member '{}' uses unsupported dynamic-Huffman method -lh1-",
                    utf8(path)
                )));
            }
            _ => {
                return Err(error(format!(
                    "LZH member '{}' uses unsupported compression method {}",
                    utf8(path),
                    self.method
                )));
            }
        };
        if output.len() != size {
            return Err(error(format!("LZH member '{}' extracted to an unexpected size", utf8(path))));
        }
        if crc16(&output, 0) != self.crc {
            return Err(error(format!("LZH member '{}' failed CRC-16 verification", utf8(path))));
        }
        Ok(output)
    }
}
pub(super) fn sniff(bytes: &[u8]) -> bool {
    if bytes.len() < 22 || bytes[2] != b'-' || bytes[6] != b'-' || bytes[3] != b'l' {
        return false;
    }
    let method: String = bytes[2..7].iter().map(|&b| b as char).collect();
    method_valid(&method) && bytes[20] <= 2
}
pub(super) fn read(source: &Source, limits: &ReadLimits) -> Result<Index> {
    limits.memory(source.size())?;
    let bytes = Arc::new(source::read_with_source_error(source, 0, source.size(), |err| {
        error(format!("Unable to read LZH archive: {}", err.0))
    })?);
    if bytes.len() as u64 != source.size() {
        return Err(error("Invalid LZH archive: truncated data"));
    }
    if !sniff(&bytes) {
        return Err(error("Invalid LZH archive header"));
    }
    let mut index = Index::default();
    index.read_limits = Some(*limits);
    let mut offset = 0;
    let mut parsed_count = 0;
    let mut metadata = 0u64;
    while offset < bytes.len() && bytes[offset] != 0 {
        let h = header(&bytes, offset, limits)?;
        metadata += (h.data_start - offset) as u64;
        limits.index(metadata, "index")?;
        parsed_count += 1;
        limits.entries(parsed_count)?;
        if h.next <= offset {
            return Err(error("Invalid LZH archive: header did not advance"));
        }
        offset = h.next;
        let Some(path) = h.path else {
            continue;
        };
        let directory = h.method == "-lhd-";
        if directory && h.mode.is_some_and(|mode| mode & 0xf000 == 0xa000) {
            let separator = path
                .units()
                .iter()
                .position(|&unit| unit == b'|' as u16)
                .filter(|&index| index >= 1)
                .ok_or_else(|| error(format!("Invalid LZH symbolic link '{}'", utf8(&path))))?;
            let Some(link_path) = normalize(&WireString::from_units(path.units()[..separator].to_vec()), false) else {
                continue;
            };
            let Some(target) = normalize(&WireString::from_units(path.units()[separator + 1..].to_vec()), false) else {
                continue;
            };
            index.upsert(Entry {
                path: link_path,
                directory: false,
                size: 0,
                mtime: h.mtime,
                mode: h.mode,
                storage: Some(Storage::Link { target, resolve: false }),
            })?;
            continue;
        }
        index.upsert(Entry {
            path,
            directory,
            size: if directory { 0 } else { h.size },
            mtime: h.mtime,
            mode: h.mode,
            storage: (!directory).then(|| {
                Storage::ReadMember(Arc::new(LzhMember {
                    archive: bytes.clone(),
                    start: h.data_start,
                    packed_size: h.packed_size,
                    method: h.method,
                    crc: h.crc,
                }))
            }),
        })?;
    }
    if index.entries.is_empty() && parsed_count == 0 {
        return Err(error("Invalid LZH archive: no members"));
    }
    Ok(index)
}
