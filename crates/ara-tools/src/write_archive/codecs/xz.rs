//! Fixed OMP596f2da XZ framing/index/filter/check contracts around Rust codecs.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
fn invalid(message: &str) -> ToolError {
    error(format!("Invalid XZ stream: {message}"))
}
fn read32(bytes: &[u8], offset: usize) -> Result<u32> {
    let data = bytes.get(offset..offset.saturating_add(4)).ok_or_else(|| invalid("truncated integer"))?;
    Ok(u32::from_le_bytes(data.try_into().unwrap()))
}
struct Vars<'a> {
    bytes: &'a [u8],
    pos: usize,
    limit: usize,
}
impl Vars<'_> {
    fn take(&mut self) -> Result<u64> {
        let mut value = 0u64;
        for index in 0..9 {
            if self.pos >= self.limit {
                return Err(invalid("truncated variable-length integer"));
            }
            let byte = self.bytes[self.pos];
            self.pos += 1;
            if index > 0 && byte == 0 {
                return Err(invalid("non-canonical variable-length integer"));
            }
            let next = (byte & 0x7f) as u64;
            value |= next << (index * 7);
            if value > JS_SAFE {
                return Err(error("XZ stream uses sizes too large to read safely"));
            }
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(invalid("variable-length integer is too long"))
    }
    fn padding(&mut self, message: &str) -> Result<()> {
        while self.pos < self.limit {
            if self.bytes[self.pos] != 0 {
                return Err(invalid(message));
            }
            self.pos += 1;
        }
        Ok(())
    }
}
struct Record {
    unpadded: u64,
    uncompressed: u64,
}
struct Stream {
    start: usize,
    index: usize,
    check: u8,
    records: Vec<Record>,
}
fn check_size(check: u8) -> Result<usize> {
    match check {
        0 => Ok(0),
        1 => Ok(4),
        4 => Ok(8),
        10 => Ok(32),
        _ => Err(error(format!("Unsupported XZ integrity check ID 0x{check:x}"))),
    }
}
fn parse_index(bytes: &[u8], start: usize, size: usize) -> Result<Vec<Record>> {
    if size < 8 || start.checked_add(size).is_none_or(|end| end > bytes.len()) {
        return Err(invalid("index range is invalid"));
    }
    let end = start + size;
    if bytes[start] != 0 {
        return Err(invalid("missing index indicator"));
    }
    if crc32fast::hash(&bytes[start..end - 4]) != read32(bytes, end - 4)? {
        return Err(invalid("index CRC32 mismatch"));
    }
    let mut vars = Vars { bytes, pos: start + 1, limit: end - 4 };
    let count = vars.take()?;
    if count > size as u64 / 2 {
        return Err(invalid("impossible index record count"));
    }
    let mut records = Vec::new();
    for _ in 0..count {
        let unpadded = vars.take()?;
        let uncompressed = vars.take()?;
        if unpadded == 0 {
            return Err(invalid("zero unpadded block size"));
        }
        records.push(Record { unpadded, uncompressed });
    }
    vars.padding("non-zero index padding")?;
    Ok(records)
}
fn streams(bytes: &[u8]) -> Result<Vec<Stream>> {
    if bytes.is_empty() || bytes.len() & 3 != 0 {
        return Err(invalid("size is not a multiple of four bytes"));
    }
    let mut end = bytes.len();
    let mut streams = Vec::new();
    while end > 0 {
        while end >= 4 && bytes[end - 4..end].iter().all(|&b| b == 0) {
            end -= 4;
        }
        if end == 0 {
            return Err(invalid("padding without a stream"));
        }
        if end < 24 {
            return Err(invalid("truncated stream framing"));
        }
        let footer = end - 12;
        if bytes[footer + 10..footer + 12] != [0x59, 0x5a] {
            return Err(invalid("footer magic mismatch"));
        }
        if crc32fast::hash(&bytes[footer + 4..footer + 10]) != read32(bytes, footer)? {
            return Err(invalid("footer CRC32 mismatch"));
        }
        let flags = &bytes[footer + 8..footer + 10];
        if flags[0] != 0 || flags[1] & 0xf0 != 0 {
            return Err(error("Unsupported XZ stream flags"));
        }
        let check = flags[1] & 15;
        check_size(check)?;
        let size = (read32(bytes, footer + 4)? as u64 + 1) * 4;
        if size > footer as u64 {
            return Err(invalid("backward index size is invalid"));
        }
        let index = footer - size as usize;
        let records = parse_index(bytes, index, size as usize)?;
        let mut blocks = 0u64;
        for record in &records {
            blocks = blocks
                .checked_add(record.unpadded.div_ceil(4) * 4)
                .filter(|&n| n <= JS_SAFE)
                .ok_or_else(|| error("XZ stream uses sizes too large to read safely"))?;
        }
        let start = (index as u64)
            .checked_sub(blocks)
            .and_then(|n| n.checked_sub(12))
            .filter(|&n| n + 12 <= bytes.len() as u64)
            .ok_or_else(|| invalid("header position or magic is invalid"))? as usize;
        if bytes[start..start + 6] != [0xfd, 0x37, 0x7a, 0x58, 0x5a, 0] {
            return Err(invalid("header position or magic is invalid"));
        }
        if bytes[start + 6..start + 8] != *flags {
            return Err(invalid("header and footer flags differ"));
        }
        if crc32fast::hash(&bytes[start + 6..start + 8]) != read32(bytes, start + 8)? {
            return Err(invalid("header CRC32 mismatch"));
        }
        streams.push(Stream { start, index, check, records });
        end = start;
    }
    streams.reverse();
    Ok(streams)
}
fn lzma2(input: &[u8], property: u8, size: u64) -> Result<Vec<u8>> {
    if property > 40 {
        return Err(error("Unsupported LZMA2 dictionary property"));
    }
    // Validate the exact chunk boundary/declared output before the library can
    // stop at its EOS. The upstream rejects even one trailing compressed byte.
    let mut pos = 0usize;
    let mut total = 0u64;
    let mut reset = true;
    let mut properties = false;
    let take = |pos: &mut usize| -> Result<u8> {
        let byte = *input.get(*pos).ok_or_else(|| error("Invalid LZMA2 stream: truncated chunk header"))?;
        *pos += 1;
        Ok(byte)
    };
    loop {
        let control = take(&mut pos)?;
        if control == 0 {
            break;
        }
        if control < 0x80 {
            if control != 1 && control != 2 {
                return Err(error(format!("Invalid LZMA2 control byte 0x{control:x}")));
            }
            if control == 2 && reset {
                return Err(error("Invalid LZMA2 stream: dictionary was not initialized"));
            }
            let unpack = ((take(&mut pos)? as u64) << 8) + take(&mut pos)? as u64 + 1;
            if unpack > size.saturating_sub(total) || unpack > input.len().saturating_sub(pos) as u64 {
                return Err(error("Invalid LZMA2 uncompressed chunk size"));
            }
            total += unpack;
            pos += unpack as usize;
            if control == 1 {
                reset = false;
                properties = false;
            }
            continue;
        }
        let unpack = (((control & 31) as u64) << 16) + ((take(&mut pos)? as u64) << 8) + take(&mut pos)? as u64 + 1;
        let packed = ((take(&mut pos)? as u64) << 8) + take(&mut pos)? as u64 + 1;
        if reset && control < 0xe0 {
            return Err(error("Invalid LZMA2 stream: dictionary was not initialized"));
        }
        if !properties && control < 0xc0 {
            return Err(error("Invalid LZMA2 stream: properties were not initialized"));
        }
        if control >= 0xc0 {
            let prop = take(&mut pos)?;
            if prop >= 225 {
                return Err(error("Invalid LZMA2 properties"));
            }
            if prop % 9 + prop / 9 % 5 > 4 {
                return Err(error("Invalid LZMA2 literal properties"));
            }
            properties = true;
        }
        if packed > input.len().saturating_sub(pos) as u64 || unpack > size.saturating_sub(total) {
            return Err(error("Invalid LZMA2 chunk size"));
        }
        pos += packed as usize;
        total += unpack;
        reset = false;
    }
    if pos != input.len() {
        return Err(error("Invalid LZMA2 stream: trailing data"));
    }
    let dictionary =
        if property == 40 { u32::MAX } else { (2 | (property as u32 & 1)) << ((property as u32 / 2) + 11) };
    // Every valid distance is <= bytes already produced. Keep the original
    // smaller history ceiling; a larger history cannot matter within this
    // block's complete output. This admits wide dictionaries without a multi-
    // gigabyte allocation for a tiny ZIP member.
    let dictionary = dictionary.min(size.max(4096) as u32);
    let out = bounded(lzma_rust2::Lzma2Reader::new(Cursor::new(input), dictionary, None), size, "LZMA2")
        .map_err(|e| error(format!("Invalid LZMA2 stream: {}", e.0)))?;
    if out.len() as u64 != total {
        return Err(error("Invalid LZMA2 stream: decoded chunk size mismatch"));
    }
    Ok(out)
}
// Exact fixed OMP xz.ts riscvDecode. Its AUIPC predicate intentionally differs
// from the dependency's standards transform; the pinned behavior owns parity.
fn riscv(mut bytes: Vec<u8>, start: u32) -> Vec<u8> {
    if bytes.len() < 8 {
        return bytes;
    }
    let mut index = 0usize;
    while index <= bytes.len() - 8 {
        let first = bytes[index];
        if first == 0xef {
            let byte1 = bytes[index + 1];
            if byte1 & 0x0d != 0 {
                index += 2;
                continue;
            }
            let address =
                (((byte1 as u32 & 0xf0) << 13) | ((bytes[index + 2] as u32) << 9) | ((bytes[index + 3] as u32) << 1))
                    .wrapping_sub(start)
                    .wrapping_sub(index as u32);
            bytes[index + 1] = (byte1 & 0x0f) | ((address >> 8) & 0xf0) as u8;
            bytes[index + 2] = (((address >> 16) & 0x0f) | ((address >> 7) & 0x10) | ((address << 4) & 0xe0)) as u8;
            bytes[index + 3] = (((address >> 4) & 0x7f) | ((address >> 13) & 0x80)) as u8;
            index += 4;
            continue;
        }
        if first & 0x7f != 0x17 {
            index += 2;
            continue;
        }
        let mut instruction = u32::from_le_bytes(bytes[index..index + 4].try_into().unwrap());
        let instruction2;
        if instruction & 0xe80 != 0 {
            let next = u32::from_le_bytes(bytes[index + 4..index + 8].try_into().unwrap());
            if (instruction << 8) ^ (next.wrapping_sub(3) & 0xf8003) != 0 {
                index += 6;
                continue;
            }
            let address = (instruction & 0xfffff000).wrapping_add(next >> 20);
            instruction = 0x17 | (2 << 7) | (next << 12);
            instruction2 = address;
        } else {
            let register = instruction >> 27;
            if instruction.wrapping_sub(0x3117).wrapping_shl(18) >= register & 0x1d {
                index += 4;
                continue;
            }
            let address = u32::from_be_bytes(bytes[index + 4..index + 8].try_into().unwrap())
                .wrapping_sub(start)
                .wrapping_sub(index as u32);
            instruction2 = (instruction >> 12) | (address << 20);
            instruction = 0x17 | (register << 7) | (address.wrapping_add(0x800) & 0xfffff000);
        }
        bytes[index..index + 4].copy_from_slice(&instruction.to_le_bytes());
        bytes[index + 4..index + 8].copy_from_slice(&instruction2.to_le_bytes());
        index += 8;
    }
    bytes
}
fn filter(output: Vec<u8>, id: u64, props: &[u8]) -> Result<Vec<u8>> {
    use lzma_rust2::filter::{bcj::BcjReader, delta::DeltaReader};
    let size = output.len() as u64;
    if id == 3 {
        if props.len() != 1 {
            return Err(error("Invalid XZ Delta filter properties"));
        }
        return bounded(DeltaReader::new(Cursor::new(output), props[0] as usize + 1), size, "XZ");
    }
    if props.len() != 0 && props.len() != 4 {
        return Err(error(format!("Invalid XZ BCJ filter 0x{id:x} properties")));
    }
    let start = if props.len() == 4 { read32(props, 0)? as usize } else { 0 };
    let alignment = match id {
        6 => 16,
        8 | 11 => 2,
        4 => 1,
        _ => 4,
    };
    if start & (alignment - 1) != 0 {
        return Err(error(format!("Invalid XZ BCJ filter 0x{id:x} start offset")));
    }
    if id == 11 {
        return Ok(riscv(output, start as u32));
    }
    let reader = Cursor::new(output);
    let reader = match id {
        4 => BcjReader::new_x86(reader, start),
        5 => BcjReader::new_ppc(reader, start),
        6 => BcjReader::new_ia64(reader, start),
        7 => BcjReader::new_arm(reader, start),
        8 => BcjReader::new_arm_thumb(reader, start),
        9 => BcjReader::new_sparc(reader, start),
        10 => BcjReader::new_arm64(reader, start),
        _ => return Err(error(format!("Unsupported XZ filter ID 0x{id:x}"))),
    };
    bounded(reader, size, "XZ")
}
const fn crc64_table() -> [u64; 256] {
    let mut table = [0; 256];
    let mut slot = 0;
    while slot < 256 {
        let mut value = slot as u64;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 0 { value >> 1 } else { (value >> 1) ^ 0xc96c5795d7870f42 };
            bit += 1;
        }
        table[slot] = value;
        slot += 1;
    }
    table
}
pub(super) fn crc64(bytes: &[u8], seed: u64) -> u64 {
    const TABLE: [u64; 256] = crc64_table();
    let mut crc = !seed;
    for &b in bytes {
        crc = (crc >> 8) ^ TABLE[((crc as u8) ^ b) as usize];
    }
    !crc
}
fn verify(check: u8, output: &[u8], expected: &[u8]) -> Result<()> {
    let matches = match check {
        0 => true,
        1 => read32(expected, 0)? == crc32fast::hash(output),
        4 => crc64(output, 0).to_le_bytes() == expected,
        10 => {
            use sha2::Digest;
            sha2::Sha256::digest(output).as_slice() == expected
        }
        _ => unreachable!(),
    };
    if matches {
        Ok(())
    } else {
        Err(invalid(match check {
            1 => "block CRC32 mismatch",
            4 => "block CRC64 mismatch",
            _ => "block SHA-256 mismatch",
        }))
    }
}
fn block(bytes: &[u8], offset: usize, record: &Record, check: u8) -> Result<Vec<u8>> {
    if offset >= bytes.len() || bytes[offset] == 0 {
        return Err(invalid("missing block header"));
    }
    let header = (bytes[offset] as usize + 1) * 4;
    if header < 8 || offset + header > bytes.len() {
        return Err(invalid("truncated block header"));
    }
    if crc32fast::hash(&bytes[offset..offset + header - 4]) != read32(bytes, offset + header - 4)? {
        return Err(invalid("block header CRC32 mismatch"));
    }
    let flags = bytes[offset + 1];
    if flags & 0x3c != 0 {
        return Err(error("Unsupported XZ block flags"));
    }
    let mut vars = Vars { bytes, pos: offset + 2, limit: offset + header - 4 };
    let declared_compressed = if flags & 0x40 != 0 { Some(vars.take()?) } else { None };
    let declared_uncompressed = if flags & 0x80 != 0 { Some(vars.take()?) } else { None };
    let mut filters = Vec::new();
    for _ in 0..=(flags & 3) {
        let id = vars.take()?;
        let size = vars.take()?;
        if size > vars.limit.saturating_sub(vars.pos) as u64 {
            return Err(invalid("truncated filter properties"));
        }
        let props = &bytes[vars.pos..vars.pos + size as usize];
        vars.pos += size as usize;
        filters.push((id, props));
    }
    vars.padding("non-zero block header padding")?;
    let integrity = check_size(check)?;
    let compressed = record
        .unpadded
        .checked_sub(header as u64 + integrity as u64)
        .filter(|&n| n > 0)
        .ok_or_else(|| invalid("compressed block size is invalid"))?;
    if declared_compressed.is_some_and(|n| n != compressed) {
        return Err(invalid("block compressed size mismatch"));
    }
    if declared_uncompressed.is_some_and(|n| n != record.uncompressed) {
        return Err(invalid("block uncompressed size mismatch"));
    }
    let start = offset + header;
    let end = (start as u64)
        .checked_add(compressed)
        .filter(|&n| n <= bytes.len() as u64)
        .ok_or_else(|| invalid("truncated block data"))? as usize;
    let padding = (4 - (header + compressed as usize) % 4) % 4;
    let check_start = end + padding;
    if check_start + integrity > bytes.len() {
        return Err(invalid("truncated block data"));
    }
    let &(id, props) = filters.last().unwrap();
    if id != 0x21 || props.len() != 1 {
        return Err(error(format!("Unsupported XZ terminal filter ID 0x{id:x} (LZMA2 required)")));
    }
    let mut output = lzma2(&bytes[start..end], props[0], record.uncompressed)?;
    if output.len() as u64 != record.uncompressed {
        return Err(invalid("decoded block size mismatch"));
    }
    for &(id, props) in filters[..filters.len() - 1].iter().rev() {
        output = filter(output, id, props)?;
    }
    if bytes[end..check_start].iter().any(|&b| b != 0) {
        return Err(invalid("non-zero block padding"));
    }
    verify(check, &output, &bytes[check_start..check_start + integrity])?;
    if check_start as u64 + integrity as u64 != offset as u64 + record.unpadded.div_ceil(4) * 4 {
        return Err(invalid("block size does not match its index record"));
    }
    Ok(output)
}
pub(super) fn decode(bytes: &[u8], max: u64) -> Result<Vec<u8>> {
    let streams = streams(bytes)?;
    let mut total = 0u64;
    for stream in &streams {
        for record in &stream.records {
            total = total
                .checked_add(record.uncompressed)
                .filter(|&n| n <= max && n <= JS_SAFE)
                .ok_or_else(|| error("XZ output exceeds its size limit"))?;
        }
    }
    let mut output = Vec::with_capacity(total as usize);
    for stream in streams {
        let mut position = stream.start + 12;
        for record in stream.records {
            output.extend(block(bytes, position, &record, stream.check)?);
            position += record.unpadded.div_ceil(4) as usize * 4;
        }
        if position != stream.index {
            return Err(invalid("blocks do not align with index"));
        }
    }
    Ok(output)
}
