//! Fixed OMP596f2da ar/cab.ts lazy Microsoft Cabinet reader.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! MSZIP and local DOS timestamp conversion are narrow production codec joins.
use super::*;
type Inflate = dyn Fn(&[u8], &[u8], usize) -> Result<Vec<u8>> + Send + Sync;
pub(super) type InflatePort = Arc<Inflate>;
const MAX_OUTPUT: usize = 32768;
#[derive(Clone)]
struct Description {
    start: u64,
    end: u64,
    block_count: usize,
    method: u16,
    parameter: usize,
    required: u64,
}
fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn exact(source: &Source, start: u64, end: u64, cabinet_size: u64) -> Result<Vec<u8>> {
    if start > JS_SAFE || end > JS_SAFE || end < start || end > cabinet_size {
        return Err(error("Invalid CAB archive: metadata range is out of bounds"));
    }
    let bytes = source::read_with_source_error(source, start, end - start, |err| {
        error(format!("Unable to read CAB archive: {}", err.0))
    })?;
    if bytes.len() as u64 != end - start {
        return Err(error("Invalid CAB archive: truncated data"));
    }
    Ok(bytes)
}
fn checksum(bytes: &[u8], seed: u32) -> u32 {
    let mut value = seed;
    let mut offset = 0;
    while offset + 4 <= bytes.len() {
        value ^= u32_at(bytes, offset);
        offset += 4;
    }
    let remainder = match bytes.len() - offset {
        3 => ((bytes[offset] as u32) << 16) | ((bytes[offset + 1] as u32) << 8) | bytes[offset + 2] as u32,
        2 => ((bytes[offset] as u32) << 8) | bytes[offset + 1] as u32,
        1 => bytes[offset] as u32,
        _ => 0,
    };
    value ^ remainder
}
fn decode_name(bytes: &[u8], utf8_name: bool) -> Result<WireString> {
    if utf8_name {
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        let text =
            std::str::from_utf8(bytes).map_err(|_| error("Invalid CAB archive: file name is not valid UTF-8"))?;
        Ok(text.into())
    } else {
        Ok(encoding_rs::WINDOWS_1252.decode_with_bom_removal(bytes).0.into_owned().into())
    }
}
fn timestamp(
    date: u16,
    time: u16,
    convert: &impl Fn(i32, u32, u32, u32, u32, u32) -> Result<f64>,
) -> Result<Option<f64>> {
    if date == 0 && time == 0 {
        return Ok(None);
    }
    let year = 1980 + (date >> 9) as i32;
    let month = (date >> 5) & 0x0f;
    let day = date & 0x1f;
    let hour = time >> 11;
    let minute = (time >> 5) & 0x3f;
    let second = (time & 0x1f) * 2;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return Err(error("Invalid CAB archive: file has an invalid DOS timestamp"));
    }
    // Fixed Date uses the process local timezone and normalizes days 29..31.
    Ok(Some(convert(year, month as u32, day as u32, hour as u32, minute as u32, second as u32)?))
}
fn mode(attributes: u16, directory: bool) -> u32 {
    if directory {
        return 0o040755;
    }
    let mut permissions = if attributes & 1 != 0 { 0o444 } else { 0o644 };
    if attributes & 0x40 != 0 {
        permissions |= 0o111;
    }
    0o100000 | permissions
}
struct Folder {
    source: Source,
    description: Description,
    reserve: usize,
    limits: ReadLimits,
    inflate: InflatePort,
    decoded: std::sync::OnceLock<Result<Arc<Vec<u8>>>>,
}
impl Folder {
    fn read_all(&self) -> Result<Arc<Vec<u8>>> {
        self.decoded.get_or_init(|| self.decode().map(Arc::new)).clone()
    }
    fn decode(&self) -> Result<Vec<u8>> {
        let d = &self.description;
        if d.method == 2 {
            return Err(error(format!("Unsupported CAB compression method: Quantum (level {})", d.parameter)));
        }
        if d.method > 3 {
            return Err(error(format!("Unsupported CAB compression method: {}", d.method)));
        }
        if d.method == 3 && !(15..=21).contains(&d.parameter) {
            return Err(error(format!("Unsupported CAB LZX window size: {} bits (expected 15-21)", d.parameter)));
        }
        self.limits.memory(d.end - d.start)?;
        let bytes = exact(&self.source, d.start, d.end, self.source.size())?;
        let mut position = 0;
        let mut size = 0u64;
        for block in 0..d.block_count {
            if position + 8 + self.reserve > bytes.len() {
                return Err(error("Invalid CAB archive: truncated CFDATA header"));
            }
            let compressed = u16_at(&bytes, position + 4) as usize;
            let uncompressed = u16_at(&bytes, position + 6) as usize;
            if uncompressed == 0 {
                return Err(error("Unsupported multi-volume CAB archive: split CFDATA block"));
            }
            if uncompressed > MAX_OUTPUT {
                return Err(error(format!(
                    "Invalid CAB archive: CFDATA expands to {uncompressed} bytes (maximum 32768)"
                )));
            }
            let start = position + 8 + self.reserve;
            let end = start + compressed;
            if end > bytes.len() {
                return Err(error("Invalid CAB archive: truncated CFDATA payload"));
            }
            let expected = u32_at(&bytes, position);
            if expected != 0 {
                let actual = checksum(&bytes[position + 4..start], checksum(&bytes[start..end], 0));
                if actual != expected {
                    return Err(error(format!("Invalid CAB archive: CFDATA block {block} checksum mismatch")));
                }
            }
            size += uncompressed as u64;
            self.limits.memory(size)?;
            position = end;
        }
        if size < d.required {
            return Err(error("Invalid CAB archive: folder data is shorter than its file table declares"));
        }
        let mut output = vec![0u8; size as usize];
        let mut lzx = if d.method == 3 { Some(lzx::Decoder::new(d.parameter)?) } else { None };
        position = 0;
        let mut output_position: usize = 0;
        for block in 0..d.block_count {
            let compressed = u16_at(&bytes, position + 4) as usize;
            let uncompressed = u16_at(&bytes, position + 6) as usize;
            let start = position + 8 + self.reserve;
            let end = start + compressed;
            let payload = &bytes[start..end];
            let decoded = if d.method == 0 {
                if compressed != uncompressed {
                    return Err(error("Invalid CAB archive: uncompressed CFDATA sizes do not match"));
                }
                payload.to_vec()
            } else if d.method == 1 {
                if !payload.starts_with(b"CK") {
                    return Err(error("Invalid CAB archive: MSZIP block is missing its CK signature"));
                }
                let dictionary = &output[output_position.saturating_sub(MAX_OUTPUT)..output_position];
                (self.inflate)(&payload[2..], dictionary, uncompressed)
                    .map_err(|err| error(format!("Invalid CAB archive: MSZIP decompression failed: {}", err.0)))?
            } else {
                lzx.as_mut().unwrap().frame(payload, uncompressed)?
            };
            if decoded.len() != uncompressed {
                return Err(error(format!(
                    "Invalid CAB archive: CFDATA block {block} produced {} bytes, expected {uncompressed}",
                    decoded.len()
                )));
            }
            output[output_position..output_position + decoded.len()].copy_from_slice(&decoded);
            output_position += decoded.len();
            position = end;
        }
        Ok(output)
    }
}
struct MemberSource {
    folder: Arc<Folder>,
    offset: u64,
    size: u64,
}
impl ReadMemberSource for MemberSource {
    fn read(&self, size: u64, path: &WireString) -> Result<Vec<u8>> {
        if size != self.size {
            return Err(error(format!("Invalid CAB archive: size changed while extracting '{}'", utf8(path))));
        }
        let folder = self.folder.read_all()?;
        let end =
            self.offset.checked_add(size).filter(|&end| end <= JS_SAFE && end <= folder.len() as u64).ok_or_else(
                || error(format!("Invalid CAB archive: member '{}' is outside its folder data", utf8(path))),
            )?;
        Ok(folder[self.offset as usize..end as usize].to_vec())
    }
}
struct FileEntry {
    path: WireString,
    folder_index: usize,
    offset: u64,
    size: u64,
    directory: bool,
    mode: u32,
    mtime: Option<f64>,
}
pub(super) fn sniff(bytes: &[u8]) -> bool {
    bytes.starts_with(b"MSCF")
}
pub(super) fn read(
    source: Source,
    limits: &ReadLimits,
    inflate: InflatePort,
    local_date: impl Fn(i32, u32, u32, u32, u32, u32) -> Result<f64>,
) -> Result<Index> {
    if source.size() < 36 {
        return Err(error("Invalid CAB archive: truncated CFHEADER"));
    }
    let fixed = exact(&source, 0, 36, source.size())?;
    if !sniff(&fixed) {
        return Err(error("Invalid CAB archive: expected MSCF signature"));
    }
    if u32_at(&fixed, 4) != 0 || u32_at(&fixed, 12) != 0 || u32_at(&fixed, 20) != 0 {
        return Err(error("Invalid CAB archive: reserved CFHEADER fields must be zero"));
    }
    let cabinet_size = u32_at(&fixed, 8) as u64;
    if cabinet_size < 36 || cabinet_size > source.size() {
        return Err(error("Invalid CAB archive: declared cabinet size is out of bounds"));
    }
    let file_offset = u32_at(&fixed, 16) as u64;
    if file_offset < 36 || file_offset > cabinet_size {
        return Err(error("Invalid CAB archive: CFFILE table offset is out of bounds"));
    }
    if fixed[24] != 3 || fixed[25] != 1 {
        return Err(error(format!("Unsupported CAB format version {}.{} (expected 1.3)", fixed[25], fixed[24])));
    }
    let folder_count = u16_at(&fixed, 26) as usize;
    let file_count = u16_at(&fixed, 28) as usize;
    let flags = u16_at(&fixed, 30);
    if flags & 3 != 0 {
        return Err(error("Unsupported multi-volume CAB archive (previous/next cabinet link)"));
    }
    limits.entries(folder_count + file_count)?;
    if folder_count == 0 && file_count != 0 {
        return Err(error("Invalid CAB archive: files exist without a folder"));
    }
    let mut folder_reserve = 0usize;
    let mut data_reserve = 0usize;
    let mut folder_offset = 36u64;
    if flags & 4 != 0 {
        let reserve = exact(&source, 36, 40, cabinet_size)?;
        let header_reserve = u16_at(&reserve, 0) as u64;
        folder_reserve = reserve[2] as usize;
        data_reserve = reserve[3] as usize;
        if header_reserve > 60000 {
            return Err(error("Invalid CAB archive: CFHEADER reserve area exceeds 60000 bytes"));
        }
        folder_offset += 4 + header_reserve;
    }
    let folder_record_size = 8 + folder_reserve;
    let folder_end = folder_offset + (folder_count * folder_record_size) as u64;
    if folder_end > JS_SAFE || folder_end > cabinet_size || folder_end > file_offset {
        return Err(error("Invalid CAB archive: CFFOLDER table is out of bounds"));
    }
    limits.index(folder_end, "CAB header")?;
    let header = exact(&source, 0, folder_end, cabinet_size)?;
    let mut descriptions = Vec::with_capacity(folder_count);
    for index in 0..folder_count {
        let offset = folder_offset as usize + index * folder_record_size;
        let kind = u16_at(&header, offset + 6);
        descriptions.push(Description {
            start: u32_at(&header, offset) as u64,
            end: cabinet_size,
            block_count: u16_at(&header, offset + 4) as usize,
            method: kind & 0xf,
            parameter: (kind >> 8) as usize,
            required: 0,
        });
    }
    let starts: Vec<_> = descriptions.iter().map(|d| d.start).collect();
    for d in &mut descriptions {
        if d.start < folder_end || d.start > cabinet_size {
            return Err(error("Invalid CAB archive: CFFOLDER data offset is out of bounds"));
        }
        for &start in &starts {
            if start > d.start && start < d.end {
                d.end = start;
            }
        }
    }
    let first_data = starts.iter().copied().min().unwrap_or(cabinet_size);
    if file_offset > first_data {
        return Err(error("Invalid CAB archive: CFFILE table overlaps folder data"));
    }
    limits.index(folder_end + first_data - file_offset, "CAB index")?;
    let file_table = exact(&source, file_offset, first_data, cabinet_size)?;
    let mut files = Vec::with_capacity(file_count);
    let mut position = 0;
    for _ in 0..file_count {
        if position + 16 > file_table.len() {
            return Err(error("Invalid CAB archive: truncated CFFILE entry"));
        }
        let size = u32_at(&file_table, position) as u64;
        let offset = u32_at(&file_table, position + 4) as u64;
        let folder_index = u16_at(&file_table, position + 8) as usize;
        let date = u16_at(&file_table, position + 10);
        let time = u16_at(&file_table, position + 12);
        let attributes = u16_at(&file_table, position + 14);
        position += 16;
        let name_end = file_table[position..]
            .iter()
            .position(|&b| b == 0)
            .map(|n| position + n)
            .ok_or_else(|| error("Invalid CAB archive: unterminated CFFILE name"))?;
        let name_bytes = &file_table[position..name_end];
        if name_bytes.len() > 256 {
            return Err(error("Invalid CAB archive: CFFILE name exceeds 256 bytes"));
        }
        limits.path(name_bytes.len(), "member path")?;
        position = name_end + 1;
        if folder_index >= 0xfffd {
            return Err(error("Unsupported multi-volume CAB archive: continued file"));
        }
        if folder_index >= folder_count {
            return Err(error("Invalid CAB archive: CFFILE references a missing folder"));
        }
        let name = decode_name(name_bytes, attributes & 0x80 != 0)?;
        limits.member(size, &name)?;
        let end = offset + size;
        if end > JS_SAFE {
            return Err(error("Invalid CAB archive: CFFILE range is too large"));
        }
        descriptions[folder_index].required = descriptions[folder_index].required.max(end);
        let Some(path) = normalize(&name, false) else {
            continue;
        };
        let directory = attributes & 0x10 != 0;
        files.push(FileEntry {
            path,
            folder_index,
            offset,
            size,
            directory,
            mode: mode(attributes, directory),
            mtime: timestamp(date, time, &local_date)?,
        });
    }
    for d in &descriptions {
        limits.memory(d.required)?;
    }
    let folders: Vec<_> = descriptions
        .into_iter()
        .map(|description| {
            Arc::new(Folder {
                source: source.clone(),
                description,
                reserve: data_reserve,
                limits: *limits,
                inflate: inflate.clone(),
                decoded: Default::default(),
            })
        })
        .collect();
    let mut index = Index::default();
    index.read_limits = Some(*limits);
    for file in files {
        index.upsert(Entry {
            path: file.path,
            directory: file.directory,
            size: file.size,
            mode: Some(file.mode),
            mtime: file.mtime,
            storage: (!file.directory).then(|| {
                Storage::ReadMember(Arc::new(MemberSource {
                    folder: folders[file.folder_index].clone(),
                    offset: file.offset,
                    size: file.size,
                }))
            }),
        })?;
    }
    Ok(index)
}
