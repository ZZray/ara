//! Fixed OMP596f2da ar/arj.ts CRC-framed ARJ container and selected extraction.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;

const SIGNATURE: &[u8] = &[0x60, 0xea];
const MAX_BASIC_HEADER: usize = 2600;
fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn range(bytes: &[u8], start: usize, end: usize, what: &str) -> Result<()> {
    if start > end || end > bytes.len() || end as u64 > JS_SAFE {
        Err(error(format!("Invalid ARJ archive: truncated {what}")))
    } else {
        Ok(())
    }
}
fn cstring(bytes: &[u8], start: usize, end: usize, what: &str) -> Result<(String, usize)> {
    let terminator = bytes[start..end]
        .iter()
        .position(|&b| b == 0)
        .map(|offset| start + offset)
        .ok_or_else(|| error(format!("Invalid ARJ {what}: missing terminator")))?;
    Ok((lzh::legacy(&bytes[start..terminator]), terminator + 1))
}
struct Block {
    start: usize,
    size: usize,
    next: usize,
    metadata: usize,
    end: bool,
}
fn block(bytes: &[u8], offset: usize, limits: &ReadLimits) -> Result<Block> {
    range(bytes, offset, offset + 4, "header signature")?;
    if !bytes[offset..].starts_with(SIGNATURE) {
        return Err(error("Invalid ARJ header signature"));
    }
    let size = u16_at(bytes, offset + 2) as usize;
    if size == 0 {
        return Ok(Block { start: offset + 4, size: 0, next: offset + 4, metadata: 4, end: true });
    }
    if !(30..=MAX_BASIC_HEADER).contains(&size) {
        return Err(error("Invalid ARJ basic header size"));
    }
    let start = offset + 4;
    let end = start + size;
    range(bytes, start, end + 4, "basic header")?;
    if crc32fast::hash(&bytes[start..end]) != u32_at(bytes, end) {
        return Err(error("Invalid ARJ basic header CRC32"));
    }
    let mut cursor = end + 4;
    let mut count = 0;
    loop {
        range(bytes, cursor, cursor + 2, "extended header size")?;
        let extension_size = u16_at(bytes, cursor) as usize;
        cursor += 2;
        if extension_size == 0 {
            break;
        }
        count += 1;
        if count > 65535 {
            return Err(error("Invalid ARJ archive: too many extended headers"));
        }
        range(bytes, cursor, cursor + extension_size + 4, "extended header")?;
        if crc32fast::hash(&bytes[cursor..cursor + extension_size]) != u32_at(bytes, cursor + extension_size) {
            return Err(error("Invalid ARJ extended header CRC32"));
        }
        cursor += extension_size + 4;
        limits.index((cursor - offset) as u64, "header metadata")?;
    }
    Ok(Block { start, size, next: cursor, metadata: cursor - offset, end: false })
}
fn method4(packed: &[u8], size: usize) -> Result<Vec<u8>> {
    let mut reader = lh_static::MsbBitReader::new(packed, "ARJ method-4");
    let mut output = vec![0u8; size];
    let mut position = 0;
    while position < size {
        let mut length_code = 0;
        let mut width = 0;
        while width < 7 {
            if reader.read(1)? == 0 {
                break;
            }
            length_code += 1 << width;
            width += 1;
        }
        if width != 0 {
            length_code += reader.read(width)?;
        }
        if length_code == 0 {
            output[position] = reader.read(8)? as u8;
            position += 1;
            continue;
        }
        let length = length_code + 2;
        if length > size - position {
            return Err(error("Invalid ARJ method-4 compressed data: match exceeds declared size"));
        }
        let mut code = 0;
        let mut width = 9;
        while width < 13 {
            if reader.read(1)? == 0 {
                break;
            }
            code += 1 << width;
            width += 1;
        }
        code += reader.read(width)?;
        if code >= 26624 || code >= position {
            return Err(error("Invalid ARJ method-4 compressed data: history distance is out of range"));
        }
        let mut source = position - code - 1;
        for _ in 0..length {
            output[position] = output[source];
            position += 1;
            source += 1;
        }
    }
    reader.zero_padding()?;
    Ok(output)
}
struct ArjMember {
    archive: Arc<Vec<u8>>,
    start: usize,
    packed_size: usize,
    method: u8,
    crc: u32,
}
impl ReadMemberSource for ArjMember {
    fn read(&self, size: u64, path: &WireString) -> Result<Vec<u8>> {
        let packed = &self.archive[self.start..self.start + self.packed_size];
        let size = usize::try_from(size).map_err(|_| error("Archive is too large to read safely"))?;
        let output = match self.method {
            0 => {
                if packed.len() != size {
                    return Err(error(format!("ARJ member '{}' has inconsistent stored size", utf8(path))));
                }
                packed.to_vec()
            }
            1..=3 => lh_static::decode(packed, size, 26624, 5, 17, &format!("ARJ method {}", self.method))?,
            4 => method4(packed, size)?,
            8 | 9 => {
                if size != 0 || !packed.is_empty() {
                    return Err(error(format!("ARJ member '{}' has invalid no-data method sizes", utf8(path))));
                }
                Vec::new()
            }
            _ => {
                return Err(error(format!(
                    "ARJ member '{}' uses unsupported compression method {}",
                    utf8(path),
                    self.method
                )));
            }
        };
        if output.len() != size {
            return Err(error(format!("ARJ member '{}' extracted to an unexpected size", utf8(path))));
        }
        if self.method != 8 && crc32fast::hash(&output) != self.crc {
            return Err(error(format!("ARJ member '{}' failed CRC32 verification", utf8(path))));
        }
        Ok(output)
    }
}
pub(super) fn sniff(bytes: &[u8]) -> bool {
    if bytes.len() < 34 || !bytes.starts_with(SIGNATURE) {
        return false;
    }
    let size = u16_at(bytes, 2) as usize;
    if !(30..=MAX_BASIC_HEADER).contains(&size) || bytes.len() < size + 8 {
        return false;
    }
    let body = &bytes[4..4 + size];
    body[0] >= 30 && body[0] as usize <= size && body[6] == 2 && crc32fast::hash(body) == u32_at(bytes, 4 + size)
}
pub(super) fn read(source: &Source, limits: &ReadLimits) -> Result<Index> {
    limits.memory(source.size())?;
    let bytes = Arc::new(source::read_with_source_error(source, 0, source.size(), |err| {
        error(format!("Unable to read ARJ archive: {}", err.0))
    })?);
    if bytes.len() as u64 != source.size() {
        return Err(error("Invalid ARJ archive: truncated data"));
    }
    if !sniff(&bytes) {
        return Err(error("Invalid ARJ archive header"));
    }
    let main = block(&bytes, 0, limits)?;
    if main.end {
        return Err(error("Invalid ARJ archive: missing main header"));
    }
    let first_size = bytes[main.start] as usize;
    if first_size < 30 || first_size > main.size || bytes[main.start + 6] != 2 {
        return Err(error("Invalid ARJ main header"));
    }
    let main_flags = bytes[main.start + 4];
    if main_flags & 1 != 0 {
        return Err(error("Encrypted ARJ archives are unsupported"));
    }
    if main_flags & 4 != 0 {
        return Err(error("Multi-volume ARJ archives are unsupported"));
    }
    let mut index = Index::default();
    index.read_limits = Some(*limits);
    let mut offset = main.next;
    let mut metadata = main.metadata as u64;
    let mut parsed_count = 0;
    loop {
        let b = block(&bytes, offset, limits)?;
        metadata += b.metadata as u64;
        limits.index(metadata, "index")?;
        if b.end {
            break;
        }
        parsed_count += 1;
        limits.entries(parsed_count)?;
        let first_size = bytes[b.start] as usize;
        if first_size < 30 || first_size > b.size {
            return Err(error("Invalid ARJ local header"));
        }
        let host_os = bytes[b.start + 3];
        let flags = bytes[b.start + 4];
        let method = bytes[b.start + 5];
        let file_type = bytes[b.start + 6];
        if flags & 1 != 0 {
            return Err(error("Encrypted ARJ members are unsupported"));
        }
        if flags & 0x0c != 0 {
            return Err(error("Multi-volume ARJ members are unsupported"));
        }
        let packed_size = u32_at(&bytes, b.start + 12) as usize;
        let size = u32_at(&bytes, b.start + 16) as u64;
        let crc = u32_at(&bytes, b.start + 20);
        let access_mode = u16_at(&bytes, b.start + 26) as u32;
        let (filename, next) = cstring(&bytes, b.start + first_size, b.start + b.size, "filename")?;
        cstring(&bytes, next, b.start + b.size, "comment")?;
        limits.path(next - (b.start + first_size) - 1, "member path")?;
        let mut raw = filename.replace('\\', "/");
        if host_os == 1 {
            raw = raw.replace('>', "/");
        } else if host_os == 4 {
            raw = raw.replace(':', "/");
        }
        let raw: WireString = raw.into();
        limits.path(path_len(&raw), "member path")?;
        let unnamed = raw.is_empty().then(|| WireString::from("<unnamed>"));
        limits.member(size, unnamed.as_ref().unwrap_or(&raw))?;
        let data_start = b.next;
        range(&bytes, data_start, data_start + packed_size, "member data")?;
        offset = data_start + packed_size;
        let Some(path) = normalize(&raw, false) else {
            continue;
        };
        let directory = file_type == 3;
        let unix = matches!(host_os, 2 | 8);
        let mode =
            (unix && access_mode != 0).then_some((if directory { 0x4000 } else { 0x8000 }) | (access_mode & 0x0fff));
        let time = u32_at(&bytes, b.start + 8);
        let mtime = if unix { (time != 0).then_some(time as f64 * 1000.0) } else { lzh::dos_time(time) };
        index.upsert(Entry {
            path,
            directory,
            size: if directory { 0 } else { size },
            mode,
            mtime,
            storage: (!directory).then(|| {
                Storage::ReadMember(Arc::new(ArjMember {
                    archive: bytes.clone(),
                    start: data_start,
                    packed_size,
                    method,
                    crc,
                }))
            }),
        })?;
    }
    if parsed_count == 0 {
        return Err(error("Invalid ARJ archive: no members"));
    }
    Ok(index)
}
