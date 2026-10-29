//! Fixed OMP596f2da ar/unix-ar.ts READ-only indexed container.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;

const SIGNATURE: &[u8] = b"!<arch>\n";
const HEADER: u64 = 60;

struct Header {
    name: String,
    size: u64,
    mtime: Option<f64>,
    mode: Option<u32>,
    bsd_name_length: Option<u64>,
}
struct Record {
    name: WireString,
    name_bytes: usize,
    offset: u64,
    size: u64,
    mtime: Option<f64>,
    mode: Option<u32>,
}
fn ascii(bytes: &[u8]) -> Result<String> {
    let end = bytes.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
    let field = &bytes[..end];
    if field.iter().any(|&b| !(0x20..=0x7e).contains(&b)) {
        return Err(error("Invalid ar archive header field"));
    }
    Ok(String::from_utf8(field.to_vec()).expect("checked ASCII field"))
}
fn optional_number(value: &str, radix: u32, label: &str) -> Result<Option<u64>> {
    if value.is_empty() || value == "-1" {
        return Ok(None);
    }
    if !value.bytes().all(|b| b.is_ascii_digit() && (radix != 8 || b <= b'7')) {
        return Err(error(format!("Invalid ar archive {label}")));
    }
    let number = u64::from_str_radix(value, radix)
        .ok()
        .filter(|&n| n <= JS_SAFE)
        .ok_or_else(|| error(format!("Invalid ar archive {label}")))?;
    Ok(Some(number))
}
fn header(bytes: &[u8]) -> Result<Header> {
    if bytes.len() != HEADER as usize || &bytes[58..60] != b"`\n" {
        return Err(error("Invalid ar archive member header"));
    }
    let name = ascii(&bytes[..16])?;
    let mtime = optional_number(&ascii(&bytes[16..28])?, 10, "modification time")?.map(|n| n as f64 * 1000.0);
    optional_number(&ascii(&bytes[28..34])?, 10, "user id")?;
    optional_number(&ascii(&bytes[34..40])?, 10, "group id")?;
    let mode = optional_number(&ascii(&bytes[40..48])?, 8, "mode")?.map(|n| n as u32);
    let size = optional_number(&ascii(&bytes[48..58])?, 10, "member size")?
        .ok_or_else(|| error("Invalid ar archive member size"))?;
    let bsd_name_length = if let Some(encoded) = name.strip_prefix("#1/") {
        if encoded.is_empty() || !encoded.bytes().all(|b| b.is_ascii_digit()) {
            return Err(error("Invalid ar archive BSD extended name length"));
        }
        Some(
            encoded
                .parse::<u64>()
                .ok()
                .filter(|&n| n > 0 && n <= JS_SAFE && n <= size)
                .ok_or_else(|| error("Invalid ar archive BSD extended name length"))?,
        )
    } else {
        None
    };
    Ok(Header { name, size, mtime, mode, bsd_name_length })
}
fn exact(source: &Source, start: u64, length: u64, what: &str) -> Result<Vec<u8>> {
    let end = start
        .checked_add(length)
        .filter(|&n| n <= JS_SAFE && n <= source.size())
        .ok_or_else(|| error(format!("Invalid ar archive: truncated {what}")))?;
    if start > JS_SAFE || end < start {
        return Err(error(format!("Invalid ar archive: truncated {what}")));
    }
    let bytes = source.range(start, length)?;
    if bytes.len() as u64 != length {
        return Err(error(format!("Invalid ar archive: truncated {what}")));
    }
    Ok(bytes)
}
fn decoded_name(bytes: &[u8], limits: &ReadLimits) -> Result<WireString> {
    limits.path(bytes.len(), "member path")?;
    let (decoded, _) = encoding_rs::UTF_8.decode_with_bom_removal(bytes);
    Ok(decoded.into_owned().into())
}
fn bsd_name(bytes: &[u8], limits: &ReadLimits) -> Result<(WireString, usize)> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    if end == 0 {
        return Err(error("Invalid ar archive empty BSD extended name"));
    }
    Ok((decoded_name(&bytes[..end], limits)?, end))
}
fn metadata_name(name: &WireString) -> bool {
    ["/", "//", "/SYM64/", "__.SYMDEF", "__.SYMDEF SORTED"].iter().any(|value| name.equals_ascii(value))
}
fn long_name(reference: &str, table: &[u8], limits: &ReadLimits) -> Result<(WireString, usize)> {
    let digits = &reference[1..];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(error(format!("Invalid ar archive member name '{reference}'")));
    }
    let offset = digits
        .parse::<u64>()
        .ok()
        .filter(|&n| n <= JS_SAFE && n < table.len() as u64)
        .ok_or_else(|| error(format!("Invalid ar archive long-name offset '{reference}'")))? as usize;
    let mut end = offset;
    while end < table.len() && !matches!(table[end], 0 | b'\n') {
        end += 1;
    }
    if end == table.len() {
        return Err(error(format!("Unterminated ar archive long name at offset {offset}")));
    }
    let name_end = if table[end] == b'\n' && end > offset && table[end - 1] == b'/' { end - 1 } else { end };
    if name_end == offset {
        return Err(error(format!("Empty ar archive long name at offset {offset}")));
    }
    Ok((decoded_name(&table[offset..name_end], limits)?, name_end - offset))
}
pub(super) fn sniff(bytes: &[u8]) -> bool {
    bytes.starts_with(SIGNATURE)
}
pub(super) fn read(source: &Source, limits: &ReadLimits) -> Result<Index> {
    if source.size() > JS_SAFE || source.size() < SIGNATURE.len() as u64 {
        return Err(error("Invalid ar archive signature"));
    }
    if !sniff(&exact(source, 0, SIGNATURE.len() as u64, "signature")?) {
        return Err(error("Invalid ar archive signature"));
    }
    let mut records = Vec::new();
    let mut long_names = None;
    let mut metadata_size = 0u64;
    let mut position = SIGNATURE.len() as u64;
    while position < source.size() {
        let header = header(&exact(source, position, HEADER, "member header")?)?;
        metadata_size += HEADER;
        limits.index(metadata_size, "index")?;
        let payload_offset = position + HEADER;
        let payload_end = payload_offset
            .checked_add(header.size)
            .filter(|&n| n <= JS_SAFE && n <= source.size())
            .ok_or_else(|| error("Invalid ar archive: truncated member data"))?;
        let mut name = WireString::from(header.name.clone());
        let mut name_bytes = header.name.len();
        let mut offset = payload_offset;
        let mut size = header.size;
        if let Some(length) = header.bsd_name_length {
            metadata_size = metadata_size
                .checked_add(length)
                .filter(|&n| n <= JS_SAFE)
                .ok_or_else(|| error("Invalid archive: index has an invalid size"))?;
            limits.index(metadata_size, "index")?;
            (name, name_bytes) = bsd_name(&exact(source, payload_offset, length, "BSD member name")?, limits)?;
            offset += length;
            size -= length;
        } else if header.name == "//" {
            metadata_size = metadata_size
                .checked_add(header.size)
                .filter(|&n| n <= JS_SAFE)
                .ok_or_else(|| error("Invalid archive: index has an invalid size"))?;
            limits.index(metadata_size, "index")?;
            long_names = Some(exact(source, payload_offset, header.size, "long-name table")?);
        }
        records.push(Record { name, name_bytes, offset, size, mtime: header.mtime, mode: header.mode });
        limits.entries(records.len())?;
        position = payload_end + (header.size & 1);
        if position > source.size() {
            return Err(error("Invalid ar archive: missing alignment byte"));
        }
    }
    let mut index = Index::default();
    index.read_limits = Some(*limits);
    for record in records {
        let mut name = record.name;
        let mut name_bytes = record.name_bytes;
        let text = utf8(&name);
        if let Some(digits) = text.strip_prefix('/')
            && !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
        {
            let table = long_names
                .as_deref()
                .ok_or_else(|| error(format!("Ar archive member '{text}' references a missing long-name table")))?;
            (name, name_bytes) = long_name(&text, table, limits)?;
        } else if !["/", "//", "/SYM64/"].iter().any(|special| name.equals_ascii(special))
            && name.units().last() == Some(&47)
        {
            name = WireString::from_units(name.units()[..name.len() - 1].to_vec());
        }
        if metadata_name(&name) {
            continue;
        }
        limits.path(name_bytes, "member path")?;
        limits.member(record.size, &name)?;
        let Some(path) = normalize(&name, false) else {
            continue;
        };
        let directory = record.mode.is_some_and(|mode| mode & 0o170000 == 0o040000);
        index.upsert(Entry {
            path,
            directory,
            size: if directory { 0 } else { record.size },
            mode: record.mode,
            mtime: record.mtime,
            storage: (!directory).then(|| Storage::Slice {
                source: source.clone(),
                offset: record.offset,
                size: record.size,
                integrity: None,
            }),
        })?;
        limits.entries(index.entries.len())?;
    }
    index.parents()?;
    Ok(index)
}
