//! Fixed OMP596f2da ar/rpm.ts lead/headers and composed CPIO reader.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;

struct HeaderIntro {
    index_count: usize,
    data_size: usize,
    body_size: usize,
    total_size: usize,
}
#[derive(Default)]
struct Metadata {
    name: Option<String>,
    version: Option<String>,
    payload_format: Option<String>,
    payload_compressor: Option<String>,
    payload_flags: Option<String>,
}
fn be32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn exact(source: &Source, start: u64, end: u64, what: &str) -> Result<Vec<u8>> {
    if start > JS_SAFE || end > JS_SAFE || end < start || end > source.size() {
        return Err(error(format!("Invalid RPM package: truncated {what}")));
    }
    let bytes = source.range(start, end - start)?;
    if bytes.len() as u64 != end - start {
        return Err(error(format!("Invalid RPM package: truncated {what}")));
    }
    Ok(bytes)
}
fn intro(bytes: &[u8], limits: &ReadLimits, what: &str) -> Result<HeaderIntro> {
    if bytes.len() != 16 || be32(bytes, 0) != 0x8eade801 {
        return Err(error(format!("Invalid RPM package: corrupt {what} header magic")));
    }
    if bytes[4..8].iter().any(|&b| b != 0) {
        return Err(error(format!("Invalid RPM package: corrupt {what} header reserved bytes")));
    }
    let index_count = be32(bytes, 8) as usize;
    let data_size = be32(bytes, 12) as usize;
    limits.entries(index_count)?;
    let body_size = index_count * 16 + data_size;
    if body_size as u64 > JS_SAFE {
        return Err(error(format!("Invalid RPM package: {what} header is too large")));
    }
    let total_size = 16 + body_size;
    limits.index(total_size as u64, &format!("RPM {what} header"))?;
    Ok(HeaderIntro { index_count, data_size, body_size, total_size })
}
fn validate_body(body: &[u8], intro: &HeaderIntro, what: &str) -> Result<()> {
    let index_size = intro.index_count * 16;
    if body.len() != intro.body_size {
        return Err(error(format!("Invalid RPM package: truncated {what} header")));
    }
    for i in 0..intro.index_count {
        let record = i * 16;
        let tag = be32(body, record);
        let kind = be32(body, record + 4);
        let offset = be32(body, record + 8) as usize;
        let count = be32(body, record + 12) as usize;
        if offset > intro.data_size {
            return Err(error(format!("Invalid RPM package: tag {tag} points outside header data")));
        }
        let remaining = intro.data_size - offset;
        let element_size = match kind {
            1 | 2 | 7 => 1,
            3 => 2,
            4 => 4,
            5 => 8,
            0 => {
                if count != 0 {
                    return Err(error(format!("Invalid RPM package: null tag {tag} has values")));
                }
                continue;
            }
            6 | 8 | 9 => {
                let string_count = if kind == 6 { 1 } else { count };
                if kind == 6 && count != 1 {
                    return Err(error(format!("Invalid RPM package: string tag {tag} has an invalid count")));
                }
                if string_count > remaining {
                    return Err(error(format!("Invalid RPM package: string tag {tag} exceeds header data")));
                }
                let mut cursor = index_size + offset;
                let limit = index_size + intro.data_size;
                for _ in 0..string_count {
                    while cursor < limit && body[cursor] != 0 {
                        cursor += 1;
                    }
                    if cursor == limit {
                        return Err(error(format!("Invalid RPM package: string tag {tag} is not NUL-terminated")));
                    }
                    cursor += 1;
                }
                continue;
            }
            _ => return Err(error(format!("Invalid RPM package: tag {tag} uses unknown data type {kind}"))),
        };
        if offset % element_size != 0 {
            return Err(error(format!("Invalid RPM package: tag {tag} data is misaligned")));
        }
        if count * element_size > remaining {
            return Err(error(format!("Invalid RPM package: tag {tag} exceeds header data")));
        }
    }
    Ok(())
}
fn utf8_bom(bytes: &[u8]) -> std::result::Result<&str, std::str::Utf8Error> {
    std::str::from_utf8(bytes).map(|s| s.strip_prefix('\u{feff}').unwrap_or(s))
}
fn main_header(body: &[u8], intro: &HeaderIntro) -> Result<Metadata> {
    validate_body(body, intro, "main")?;
    let index_size = intro.index_count * 16;
    let mut metadata = Metadata::default();
    for i in 0..intro.index_count {
        let record = i * 16;
        let tag = be32(body, record);
        let kind = be32(body, record + 4);
        let offset = be32(body, record + 8) as usize;
        let count = be32(body, record + 12);
        if !matches!(tag, 1000 | 1001 | 1124 | 1125 | 1126) {
            continue;
        }
        if kind != 6 || count != 1 {
            return Err(error(format!("Invalid RPM package: tag {tag} must contain one string")));
        }
        let start = index_size + offset;
        let limit = index_size + intro.data_size;
        let mut end = start;
        while end < limit && body[end] != 0 {
            end += 1;
        }
        if end == limit {
            return Err(error(format!("Invalid RPM package: tag {tag} string is not NUL-terminated")));
        }
        if end - start > 4096 {
            return Err(error(format!("Invalid RPM package: tag {tag} string is too large")));
        }
        let value = utf8_bom(&body[start..end])
            .map_err(|_| error(format!("Invalid RPM package: tag {tag} is not valid UTF-8")))?
            .to_owned();
        match tag {
            1000 => metadata.name = Some(value),
            1001 => metadata.version = Some(value),
            1124 => metadata.payload_format = Some(value),
            1125 => metadata.payload_compressor = Some(value),
            1126 => metadata.payload_flags = Some(value),
            _ => unreachable!(),
        }
    }
    Ok(metadata)
}
pub(super) fn sniff_lzma_alone(bytes: &[u8]) -> bool {
    if bytes.len() < 13 || bytes[0] > 224 {
        return false;
    }
    let dictionary = u32::from_le_bytes(bytes[1..5].try_into().unwrap()) as u64;
    if dictionary < 4096 {
        return false;
    }
    let rounded = dictionary.next_power_of_two();
    dictionary == rounded || dictionary == rounded - rounded / 4
}
fn compressor(method: Option<&str>, bytes: &[u8], identity: &str) -> Result<String> {
    let trimmed = method.map(|s| {
        s.trim_matches(|c| {
            matches!(c,
        '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
        '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
        '\u{205f}' | '\u{3000}' | '\u{feff}')
        })
        .to_lowercase()
    });
    if let Some(method) = &trimmed {
        if matches!(method.as_str(), "gzip" | "gz" | "bzip2" | "bzip" | "xz" | "lzma" | "zstd" | "zstdio" | "none") {
            return Ok(method.clone());
        }
    }
    let method = if bytes.starts_with(&[0x1f, 0x8b]) {
        "gzip"
    } else if bytes.len() >= 4 && bytes.starts_with(b"BZh") && (b'1'..=b'9').contains(&bytes[3]) {
        "bzip2"
    } else if bytes.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
        "xz"
    } else if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        "zstd"
    } else if sniff_lzma_alone(bytes) {
        "lzma"
    } else if cpio::sniff(bytes) {
        "none"
    } else {
        return Err(error(format!(
            "RPM package '{identity}' uses unsupported payload compressor '{}'",
            method.unwrap_or("unknown")
        )));
    };
    Ok(method.to_owned())
}
pub(super) fn read(
    source: Source,
    limits: &ReadLimits,
    decode: impl Fn(&str, &[u8], u64) -> Result<Vec<u8>>,
) -> Result<Index> {
    let initial = exact(&source, 0, 112, "lead and signature header")?;
    if !sniff(&initial) {
        return Err(error("Invalid RPM package: bad lead magic"));
    }
    let package_type = u16::from_be_bytes(initial[6..8].try_into().unwrap());
    if initial[4] < 3 || package_type > 1 {
        return Err(error("Unsupported RPM package lead version or type"));
    }
    let signature_type = u16::from_be_bytes(initial[78..80].try_into().unwrap());
    if signature_type != 5 {
        return Err(error(format!(
            "Unsupported RPM signature type {signature_type}; only header signatures are supported"
        )));
    }
    let name = &initial[10..76];
    let name = &name[..name.iter().position(|&byte| byte == 0).unwrap_or(name.len())];
    let lead_name = utf8_bom(name).ok().filter(|s| !s.is_empty()).unwrap_or("unknown package");
    let signature = intro(&initial[96..], limits, "signature")?;
    let signature_end = 96 + signature.total_size;
    let main_offset = signature_end.div_ceil(8) * 8;
    let signature_body = exact(&source, 112, main_offset as u64, "signature header")?;
    if signature_body.len() < signature.body_size {
        return Err(error("Invalid RPM package: truncated signature header"));
    }
    validate_body(&signature_body[..signature.body_size], &signature, "signature")?;
    if signature_body[signature.body_size..].iter().any(|&byte| byte != 0) {
        return Err(error("Invalid RPM package: non-zero signature alignment padding"));
    }
    let main_intro_bytes = exact(&source, main_offset as u64, main_offset as u64 + 16, "main header intro")?;
    let main = intro(&main_intro_bytes, limits, "main")?;
    limits.index((signature.total_size + main.total_size) as u64, "RPM headers")?;
    let body_offset = main_offset + 16;
    let body = exact(&source, body_offset as u64, (body_offset + main.body_size) as u64, "main header")?;
    let metadata = main_header(&body, &main)?;
    let identity = match (&metadata.name, &metadata.version) {
        (Some(name), Some(version)) if !name.is_empty() && !version.is_empty() => format!("{name}-{version}"),
        (Some(name), _) => name.clone(),
        _ => lead_name.to_owned(),
    };
    if let Some(format) = &metadata.payload_format {
        if !format.is_empty() && format.to_lowercase() != "cpio" {
            return Err(error(format!("RPM package '{identity}' uses unsupported payload format '{format}'")));
        }
    }
    let payload_offset = (main_offset + main.total_size) as u64;
    let payload_size =
        source.size().checked_sub(payload_offset).ok_or_else(|| error("Archive is too large to read safely"))?;
    limits.memory(payload_size)?;
    let payload = exact(&source, payload_offset, source.size(), "payload")?;
    let method = compressor(metadata.payload_compressor.as_deref(), &payload, &identity)?;
    let cpio = match method.as_str() {
        "none" => {
            if !cpio::sniff(&payload) {
                return Err(error(format!("RPM package '{identity}' has an invalid uncompressed CPIO payload")));
            }
            payload
        }
        "lzma" => {
            if !sniff_lzma_alone(&payload) {
                return Err(error(format!("RPM package '{identity}' has a malformed LZMA payload")));
            }
            decode("lzma", &payload, limits.max_memory)?
        }
        "gzip" | "gz" => decode("gzip", &payload, limits.max_memory)?,
        "bzip2" | "bzip" => decode("bzip2", &payload, limits.max_memory)?,
        "zstd" | "zstdio" => decode("zstd", &payload, limits.max_memory)?,
        "xz" => decode("xz", &payload, limits.max_memory)?,
        _ => return Err(error(format!("RPM package '{identity}' uses unsupported payload compressor '{method}'"))),
    };
    limits.memory(cpio.len() as u64)?;
    cpio::read_buffer(cpio, limits)
}
pub(super) fn sniff(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xed, 0xab, 0xee, 0xdb])
}
