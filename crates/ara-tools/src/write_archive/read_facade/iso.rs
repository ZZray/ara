//! Fixed OMP596f2da ar/iso.ts lazy ISO9660/Joliet/Rock Ridge reader.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;

const SECTOR: u64 = 2048;
const DESCRIPTOR_START: u64 = 16 * SECTOR;
#[derive(Clone)]
struct Record {
    extent: u64,
    size: u64,
    extended_attributes: u8,
    mtime: Option<f64>,
    flags: u8,
    unit_size: u8,
    gap_size: u8,
    identifier: Vec<u8>,
    system_use: Vec<u8>,
}
struct Volume {
    block_size: u64,
    root: Record,
    joliet: bool,
    rock_ridge_root: Option<Record>,
}
#[derive(Clone)]
struct Extent {
    start: u64,
    size: u64,
    unit_size: u8,
    gap_size: u8,
}
struct Budget {
    read_bytes: u64,
    limits: ReadLimits,
}
fn invalid(detail: impl AsRef<str>) -> ToolError {
    error(format!("Invalid ISO 9660 archive: {}", detail.as_ref()))
}
fn add(left: u64, right: u64, what: &str) -> Result<u64> {
    left.checked_add(right).filter(|&n| n <= JS_SAFE).ok_or_else(|| invalid(format!("{what} is too large")))
}
fn multiply(left: u64, right: u64, what: &str) -> Result<u64> {
    left.checked_mul(right).filter(|&n| n <= JS_SAFE).ok_or_else(|| invalid(format!("{what} is too large")))
}
fn source_range(source: &Source, start: u64, end: u64, what: &str) -> Result<()> {
    if start > JS_SAFE || end > JS_SAFE || end < start || end > source.size() {
        Err(invalid(format!("truncated or invalid {what}")))
    } else {
        Ok(())
    }
}
impl Budget {
    fn read(&mut self, source: &Source, start: u64, end: u64, what: &str) -> Result<Vec<u8>> {
        source_range(source, start, end, what)?;
        self.read_bytes = self.read_bytes.saturating_add(end - start);
        self.limits.index(self.read_bytes, "ISO metadata")?;
        let bytes = source::read_with_source_error(source, start, end - start, |_| invalid("could not parse image"))?;
        if bytes.len() as u64 != end - start {
            return Err(invalid(format!("truncated {what}")));
        }
        Ok(bytes)
    }
}
fn payload(source: &Source, start: u64, size: u64, path: &WireString) -> Result<Vec<u8>> {
    let end = add(start, size, "member extent")?;
    source_range(source, start, end, &format!("extent for '{}'", utf8(path)))?;
    let bytes = source::read_with_source_error(source, start, size, |_| {
        invalid(format!("could not extract member '{}'", utf8(path)))
    })?;
    if bytes.len() as u64 != size {
        return Err(invalid(format!("truncated extent for '{}'", utf8(path))));
    }
    Ok(bytes)
}
struct IsoMemberSource {
    source: Source,
    block_size: u64,
    extents: Vec<Extent>,
}
impl ReadMemberSource for IsoMemberSource {
    fn read(&self, size: u64, path: &WireString) -> Result<Vec<u8>> {
        let mut output = vec![0; size as usize];
        let mut output_offset = 0u64;
        for extent in &self.extents {
            if output_offset + extent.size > size {
                return Err(invalid(format!("invalid multi-extent size for '{}'", utf8(path))));
            }
            let unit = u64::from(extent.unit_size) * self.block_size;
            let gap = u64::from(extent.gap_size) * self.block_size;
            if unit == 0 {
                let bytes = payload(&self.source, extent.start, extent.size, path)?;
                output[output_offset as usize..output_offset as usize + bytes.len()].copy_from_slice(&bytes);
                output_offset += bytes.len() as u64;
                continue;
            }
            let mut remaining = extent.size;
            let mut position = extent.start;
            while remaining > 0 {
                let length = unit.min(remaining);
                let bytes = payload(&self.source, position, length, path)?;
                output[output_offset as usize..(output_offset + length) as usize].copy_from_slice(&bytes);
                output_offset += length;
                remaining -= length;
                position = add(position, unit + gap, "interleaved file offset")?;
            }
        }
        if output_offset != size {
            return Err(invalid(format!("member '{}' produced {output_offset} bytes, expected {size}", utf8(path))));
        }
        Ok(output)
    }
}
fn both16(bytes: &[u8], offset: usize, what: &str) -> Result<u16> {
    let little = u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap());
    if little != u16::from_be_bytes(bytes[offset + 2..offset + 4].try_into().unwrap()) {
        return Err(invalid(format!("{what} has mismatched byte orders")));
    }
    Ok(little)
}
fn both32(bytes: &[u8], offset: usize, what: &str) -> Result<u32> {
    let little = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    if little != u32::from_be_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) {
        return Err(invalid(format!("{what} has mismatched byte orders")));
    }
    Ok(little)
}
fn recording_time(bytes: &[u8], offset: usize) -> Option<f64> {
    let mut year = i64::from(bytes[offset]) + 1900;
    let month = i64::from(bytes[offset + 1]);
    let day = i64::from(bytes[offset + 2]);
    let hour = i64::from(bytes[offset + 3]);
    let minute = i64::from(bytes[offset + 4]);
    let second = i64::from(bytes[offset + 5]);
    let zone = i64::from(bytes[offset + 6] as i8);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
        || !(-48..=52).contains(&zone)
    {
        return None;
    }
    // Date.UTC allows day31 to overflow a shorter month. Count from the month
    // origin, retaining that behavior, then apply signed 15-minute timezone.
    year -= i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let days = era * 146097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 719468;
    Some(((days * 86400 + hour * 3600 + minute * 60 + second.min(59) - zone * 15 * 60) * 1000) as f64)
}
fn record(bytes: &[u8], offset: usize, available: usize, label: &str) -> Result<Record> {
    if available < 34 {
        return Err(invalid(format!("{label} directory record is too short")));
    }
    let length = usize::from(bytes[offset]);
    if length < 34 || length > available {
        return Err(invalid(format!("{label} directory record has an invalid length")));
    }
    let id_length = usize::from(bytes[offset + 32]);
    let padding = usize::from(id_length % 2 == 0);
    let system_use_offset = 33 + id_length + padding;
    if id_length == 0 || system_use_offset > length {
        return Err(invalid(format!("{label} directory record has an invalid identifier")));
    }
    let extent = u64::from(both32(bytes, offset + 2, &format!("{label} extent location"))?);
    let size = u64::from(both32(bytes, offset + 10, &format!("{label} data length"))?);
    both16(bytes, offset + 28, &format!("{label} volume sequence number"))?;
    let unit_size = bytes[offset + 26];
    let gap_size = bytes[offset + 27];
    if (unit_size == 0) != (gap_size == 0) {
        return Err(invalid(format!("{label} has an invalid interleave configuration")));
    }
    Ok(Record {
        extent,
        size,
        extended_attributes: bytes[offset + 1],
        mtime: recording_time(bytes, offset + 18),
        flags: bytes[offset + 25],
        unit_size,
        gap_size,
        identifier: bytes[offset + 33..offset + 33 + id_length].to_vec(),
        system_use: bytes[offset + system_use_offset..offset + length].to_vec(),
    })
}
fn descriptor(bytes: &[u8], joliet: bool) -> Result<Volume> {
    both32(bytes, 80, "volume space size")?;
    both16(bytes, 120, "volume set size")?;
    both16(bytes, 124, "volume sequence number")?;
    let block_size = u64::from(both16(bytes, 128, "logical block size")?);
    both32(bytes, 132, "path table size")?;
    if block_size != SECTOR {
        return Err(error(format!("Unsupported ISO 9660 logical block size {block_size} (expected 2048)")));
    }
    let root = record(bytes, 156, bytes.len() - 156, "root")?;
    if root.flags & 2 == 0 || root.size == 0 || root.identifier != [0] {
        return Err(invalid("invalid root directory record"));
    }
    Ok(Volume { block_size, root, joliet, rock_ridge_root: None })
}
fn ascii_at(bytes: &[u8], offset: usize, value: &[u8]) -> bool {
    bytes.get(offset..offset + value.len()) == Some(value)
}
fn read_volume(source: &Source, budget: &mut Budget, limits: &ReadLimits) -> Result<Volume> {
    if source.size() < DESCRIPTOR_START + SECTOR {
        return Err(invalid("truncated volume descriptor set"));
    }
    let mut position = DESCRIPTOR_START;
    let mut chunk_sectors = 16;
    let mut primary = None;
    let mut joliet = None;
    let mut saw_udf = false;
    let mut saw_high_sierra = false;
    let mut terminated = false;
    let mut scanned = 0;
    while !terminated && position < source.size() {
        let wanted = multiply(chunk_sectors, SECTOR, "volume descriptor scan")?;
        let remaining = limits.max_index.saturating_sub(scanned);
        if remaining < SECTOR {
            return Err(invalid("volume descriptor set exceeds metadata limit"));
        }
        let available_sectors = wanted.min(remaining).min(source.size() - position) / SECTOR;
        if available_sectors < 1 {
            return Err(invalid("truncated volume descriptor"));
        }
        let length = available_sectors * SECTOR;
        let pending = budget.read(source, position, position + length, "volume descriptor set")?;
        for data in pending.chunks_exact(SECTOR as usize) {
            let kind = data[0];
            if ascii_at(data, 1, b"CDROM") || ascii_at(data, 9, b"CDROM") {
                saw_high_sierra = true;
            }
            if [b"BEA01", b"NSR02", b"NSR03", b"TEA01"].iter().any(|&magic| ascii_at(data, 1, magic)) {
                saw_udf = true;
            }
            if ascii_at(data, 1, b"CD001") {
                if data[6] != 1 && data[6] != 2 {
                    return Err(invalid("unsupported volume descriptor version"));
                }
                if kind == 1 {
                    primary = Some(descriptor(data, false)?);
                }
                if kind == 2 && data[88] == 0x25 && data[89] == 0x2f && matches!(data[90], 0x40 | 0x43 | 0x45) {
                    joliet = Some(descriptor(data, true)?);
                }
                if kind == 255 {
                    terminated = true;
                    break;
                }
            }
            scanned += SECTOR;
        }
        position += pending.len() as u64;
        chunk_sectors = (chunk_sectors * 2).min(limits.max_index.div_ceil(SECTOR));
    }
    if primary.is_none() && joliet.is_none() {
        if saw_high_sierra {
            return Err(error("Unsupported High Sierra CD-ROM filesystem (not ISO 9660)"));
        }
        if saw_udf {
            return Err(error("Unsupported UDF-only image (no ISO 9660 volume descriptor)"));
        }
        return Err(invalid("primary volume descriptor not found"));
    }
    if !terminated {
        return Err(invalid("volume descriptor terminator not found"));
    }
    if let Some(mut joliet) = joliet {
        joliet.rock_ridge_root = primary.map(|volume| volume.root);
        return Ok(joliet);
    }
    Ok(primary.unwrap())
}
fn utf8_text(bytes: &[u8]) -> WireString {
    encoding_rs::UTF_8.decode_with_bom_removal(bytes).0.into_owned().into()
}
fn identifier_name(identifier: &[u8], joliet: bool) -> Result<WireString> {
    let mut name = if joliet {
        if identifier.len() % 2 != 0 {
            return Err(invalid("Joliet identifier has an odd byte length"));
        }
        WireString::from_units(identifier.chunks_exact(2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect())
    } else {
        utf8_text(identifier)
    };
    if name.units().ends_with(&[59, 49]) {
        name = WireString::from_units(name.units()[..name.len() - 2].to_vec());
    }
    Ok(name)
}
fn susp_skip(record: &Record) -> Option<usize> {
    let bytes = &record.system_use;
    let mut offset = 0;
    while offset + 7 <= bytes.len() {
        let length = usize::from(bytes[offset + 2]);
        if length < 4 || offset + length > bytes.len() {
            return None;
        }
        if bytes[offset..offset + 2] == *b"SP" && length == 7 && bytes[offset + 4..offset + 6] == [0xbe, 0xef] {
            return Some(usize::from(bytes[offset + 6]));
        }
        offset += length;
    }
    None
}

#[derive(Default)]
struct Susp {
    name: Option<WireString>,
    mode: Option<u32>,
    symlink: Option<WireString>,
    relocation: Option<&'static str>,
}
fn concat(parts: &[WireString]) -> WireString {
    WireString::from_units(parts.iter().flat_map(|part| part.units().iter().copied()).collect())
}
fn parse_susp(
    record: &Record,
    skip: Option<usize>,
    source: &Source,
    block_size: u64,
    budget: &mut Budget,
    limits: &ReadLimits,
) -> Result<Susp> {
    let Some(skip) = skip.filter(|&skip| skip <= record.system_use.len()) else { return Ok(Susp::default()) };
    let mut queue = vec![record.system_use[skip..].to_vec()];
    let mut continuation_ranges = HashSet::new();
    let mut name_parts = Vec::new();
    let mut link_parts = Vec::<WireString>::new();
    let mut component_continues = false;
    let mut mode = None;
    let mut relocation = None;
    let mut queue_index = 0;
    while queue_index < queue.len() {
        let area = std::mem::take(&mut queue[queue_index]);
        queue_index += 1;
        let mut offset = 0;
        while offset + 4 <= area.len() {
            let signature = &area[offset..offset + 2];
            let length = usize::from(area[offset + 2]);
            let version = area[offset + 3];
            if length < 4 || offset + length > area.len() {
                return Err(invalid("malformed SUSP record"));
            }
            if version != 1 && matches!(signature, b"CE" | b"NM" | b"PX" | b"RE" | b"CL" | b"PL" | b"SL" | b"ST") {
                return Err(invalid(format!(
                    "unsupported SUSP {} version {version}",
                    String::from_utf8_lossy(signature)
                )));
            }
            let data = &area[offset + 4..offset + length];
            if signature == b"ST" {
                break;
            }
            if signature == b"CE" {
                if data.len() < 24 {
                    return Err(invalid("short SUSP CE record"));
                }
                let block = u64::from(both32(data, 0, "SUSP continuation block")?);
                let block_offset = u64::from(both32(data, 8, "SUSP continuation offset")?);
                let length = u64::from(both32(data, 16, "SUSP continuation length")?);
                limits.index(length, "ISO SUSP continuation")?;
                let start = add(
                    multiply(block, block_size, "SUSP continuation location")?,
                    block_offset,
                    "SUSP continuation location",
                )?;
                let end = add(start, length, "SUSP continuation range")?;
                let key = (start, end);
                if continuation_ranges.contains(&key) {
                    return Err(invalid("cyclic SUSP continuation"));
                }
                if continuation_ranges.len() >= 32 {
                    return Err(invalid("too many SUSP continuations"));
                }
                continuation_ranges.insert(key);
                queue.push(budget.read(source, start, end, "SUSP continuation")?);
            }
            if signature == b"NM" && !data.is_empty() {
                let flags = data[0];
                if flags & 6 == 0 {
                    name_parts.push(utf8_text(&data[1..]));
                } else if flags & 2 != 0 {
                    name_parts.push(".".into());
                } else if flags & 4 != 0 {
                    name_parts.push("..".into());
                }
            }
            if signature == b"PX" && data.len() >= 8 {
                mode = Some(both32(data, 0, "Rock Ridge PX mode")?);
            }
            relocation = match signature {
                b"RE" => Some("RE"),
                b"CL" => Some("CL"),
                b"PL" => Some("PL"),
                _ => relocation,
            };
            if signature == b"SL" && !data.is_empty() {
                let mut component_offset = 1;
                while component_offset < data.len() {
                    if component_offset + 2 > data.len() {
                        return Err(invalid("malformed Rock Ridge SL component"));
                    }
                    let flags = data[component_offset];
                    let component_length = usize::from(data[component_offset + 1]);
                    component_offset += 2;
                    if component_offset + component_length > data.len() {
                        return Err(invalid("malformed Rock Ridge SL component"));
                    }
                    let component = if flags & 8 != 0 {
                        "".into()
                    } else if flags & 4 != 0 {
                        "..".into()
                    } else if flags & 2 != 0 {
                        ".".into()
                    } else {
                        utf8_text(&data[component_offset..component_offset + component_length])
                    };
                    if component_continues && !link_parts.is_empty() {
                        let last = link_parts.last_mut().unwrap();
                        *last = concat(&[last.clone(), component]);
                    } else {
                        link_parts.push(component);
                    }
                    component_continues = flags & 1 != 0;
                    component_offset += component_length;
                }
            }
            offset += length;
        }
    }
    let name = (!name_parts.is_empty()).then(|| concat(&name_parts));
    let symlink = if link_parts.is_empty() {
        None
    } else {
        let mut units = Vec::new();
        for (i, part) in link_parts.iter().enumerate() {
            if i > 0 {
                units.push(47);
            }
            units.extend_from_slice(part.units());
        }
        // With one root component, JS '/' + [].join('/') still yields '/'.
        if link_parts.len() == 1 && link_parts[0].is_empty() {
            units.push(47);
        }
        Some(WireString::from_units(units))
    };
    Ok(Susp { name, mode, symlink, relocation })
}
fn extent(record: &Record, block_size: u64) -> Result<Extent> {
    let block = add(record.extent, u64::from(record.extended_attributes), "file extent block")?;
    Ok(Extent {
        start: multiply(block, block_size, "file extent offset")?,
        size: record.size,
        unit_size: record.unit_size,
        gap_size: record.gap_size,
    })
}
fn directory_records(
    source: &Source,
    record: &Record,
    block_size: u64,
    budget: &mut Budget,
    limits: &ReadLimits,
) -> Result<Vec<Record>> {
    let start = multiply(
        add(record.extent, u64::from(record.extended_attributes), "directory extent block")?,
        block_size,
        "directory extent offset",
    )?;
    let end = add(start, record.size, "directory extent range")?;
    let bytes = budget.read(source, start, end, "directory extent")?;
    let mut records = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let sector_remaining = block_size as usize - offset % block_size as usize;
        let length = usize::from(bytes[offset]);
        if length == 0 {
            offset += sector_remaining;
            continue;
        }
        if length > sector_remaining || length > bytes.len() - offset {
            return Err(invalid("directory record crosses a logical block"));
        }
        records.push(self::record(&bytes, offset, length, "member")?);
        limits.entries(records.len())?;
        offset += length;
    }
    Ok(records)
}
struct Ancestor {
    key: (u64, u64),
    parent: Option<Arc<Ancestor>>,
}
struct DirectoryWork {
    record: Record,
    parent_path: WireString,
    depth: usize,
    ancestor: Arc<Ancestor>,
}
fn contains_ancestor(ancestor: &Arc<Ancestor>, key: (u64, u64)) -> bool {
    let mut current = Some(ancestor.as_ref());
    while let Some(node) = current {
        if node.key == key {
            return true;
        }
        current = node.parent.as_deref();
    }
    false
}
fn dot(record: &Record) -> bool {
    record.identifier.len() == 1 && (record.identifier[0] == 0 || record.identifier[0] == 1)
}
fn root_work(root: Record) -> DirectoryWork {
    let key = (root.extent, root.size);
    DirectoryWork { record: root, parent_path: "".into(), depth: 0, ancestor: Arc::new(Ancestor { key, parent: None }) }
}
fn child_work(directory: &DirectoryWork, record: &Record, path: WireString) -> Result<DirectoryWork> {
    let key = (record.extent, record.size);
    if contains_ancestor(&directory.ancestor, key) {
        return Err(invalid(format!("cyclic directory at '{}'", utf8(&path))));
    }
    Ok(DirectoryWork {
        record: record.clone(),
        parent_path: path,
        depth: directory.depth + 1,
        ancestor: Arc::new(Ancestor { key, parent: Some(directory.ancestor.clone()) }),
    })
}
fn record_path(
    directory: &DirectoryWork,
    record: &Record,
    susp: &Susp,
    joliet: bool,
    limits: &ReadLimits,
) -> Result<Option<WireString>> {
    let name = match &susp.name {
        Some(name) => name.clone(),
        None => identifier_name(&record.identifier, joliet)?,
    };
    limits.path(path_len(&name), "member name")?;
    let path = normalize(&join(&directory.parent_path, &name), false);
    if let Some(path) = &path {
        limits.path(path_len(path), "member path")?;
    }
    Ok(path)
}
fn symlink_entry(path: &WireString, record: &Record, susp: &Susp, limits: &ReadLimits) -> Result<Option<Entry>> {
    let Some(link) = &susp.symlink else { return Ok(None) };
    limits.path(path_len(link), "link target")?;
    let target = if starts_slash(link) { None } else { normalize(&posix_join(&dirname(path), link), true) };
    Ok(Some(Entry {
        path: path.clone(),
        directory: false,
        size: 0,
        mtime: record.mtime,
        mode: susp.mode,
        storage: Some(Storage::Link { resolve: target.is_some(), target: target.unwrap_or_else(|| link.clone()) }),
    }))
}
fn check_relocation(susp: &Susp) -> Result<()> {
    if let Some(relocation) = susp.relocation {
        return Err(error(format!("Unsupported Rock Ridge relocated directory ({relocation})")));
    }
    Ok(())
}
fn merge_rock_ridge(
    source: &Source,
    root: Record,
    block_size: u64,
    budget: &mut Budget,
    limits: &ReadLimits,
    index: &mut Index,
) -> Result<()> {
    let mut work = vec![root_work(root)];
    let mut skip = None;
    while let Some(directory) = work.pop() {
        if directory.depth > 256 {
            return Err(invalid("directory hierarchy exceeds 256 levels"));
        }
        let records = directory_records(source, &directory.record, block_size, budget, limits)?;
        if directory.depth == 0 && records.first().is_some_and(|r| r.identifier == [0]) {
            skip = susp_skip(&records[0]);
        }
        for record in &records {
            if dot(record) {
                continue;
            }
            let susp = parse_susp(record, skip, source, block_size, budget, limits)?;
            check_relocation(&susp)?;
            let Some(path) = record_path(&directory, record, &susp, false, limits)? else { continue };
            if let Some(entry) = symlink_entry(&path, record, &susp, limits)? {
                index.upsert(entry)?;
                continue;
            }
            if let (Some(&slot), Some(mode)) = (index.by_path.get(&path), susp.mode) {
                index.entries[slot].mode = Some(mode);
            }
            if record.flags & 2 == 0 {
                continue;
            }
            work.push(child_work(&directory, record, path)?);
        }
    }
    Ok(())
}
pub(super) fn read(source: Source, limits: &ReadLimits) -> Result<Index> {
    if source.size() > JS_SAFE {
        return Err(invalid("invalid source size"));
    }
    let mut budget = Budget { read_bytes: 0, limits: *limits };
    let volume = read_volume(&source, &mut budget, limits)?;
    let mut work = vec![root_work(volume.root.clone())];
    let mut index = Index { read_limits: Some(*limits), ..Index::default() };
    let mut skip = None;
    while let Some(directory) = work.pop() {
        if directory.depth > 256 {
            return Err(invalid("directory hierarchy exceeds 256 levels"));
        }
        let records = directory_records(&source, &directory.record, volume.block_size, &mut budget, limits)?;
        if directory.depth == 0 && records.first().is_some_and(|r| r.identifier == [0]) {
            skip = susp_skip(&records[0]);
        }
        let mut record_index = 0;
        while record_index < records.len() {
            let record = &records[record_index];
            if dot(record) {
                record_index += 1;
                continue;
            }
            let mut parts = vec![record];
            if record.flags & 0x80 != 0 {
                if record.flags & 2 != 0 {
                    return Err(invalid("multi-extent directory is unsupported"));
                }
                while parts.last().unwrap().flags & 0x80 != 0 {
                    record_index += 1;
                    let next = records
                        .get(record_index)
                        .filter(|next| next.identifier.len() == record.identifier.len())
                        .ok_or_else(|| invalid("unterminated multi-extent file"))?;
                    if next.identifier != record.identifier {
                        return Err(invalid("non-contiguous multi-extent file"));
                    }
                    if next.flags & !0x80 != record.flags & !0x80 {
                        return Err(invalid("inconsistent multi-extent file flags"));
                    }
                    parts.push(next);
                }
            }
            record_index += 1;
            let susp = parse_susp(record, skip, &source, volume.block_size, &mut budget, limits)?;
            check_relocation(&susp)?;
            let Some(path) = record_path(&directory, record, &susp, volume.joliet, limits)? else { continue };
            if let Some(entry) = symlink_entry(&path, record, &susp, limits)? {
                index.upsert(entry)?;
                continue;
            }
            if record.flags & 2 != 0 {
                index.upsert(Entry {
                    path: path.clone(),
                    directory: true,
                    size: 0,
                    mtime: record.mtime,
                    mode: susp.mode,
                    storage: None,
                })?;
                work.push(child_work(&directory, record, path)?);
                continue;
            }
            let mut total = 0;
            for part in &parts {
                total = add(total, part.size, "multi-extent member size")?;
            }
            limits.member(total, &path)?;
            let extents = if total == 0 {
                Vec::new()
            } else {
                parts.iter().map(|part| extent(part, volume.block_size)).collect::<Result<Vec<_>>>()?
            };
            for extent in &extents {
                source_range(
                    &source,
                    extent.start,
                    add(extent.start, extent.size, "file extent range")?,
                    &format!("extent for '{}'", utf8(&path)),
                )?;
            }
            index.upsert(Entry {
                path,
                directory: false,
                size: total,
                mtime: record.mtime,
                mode: susp.mode,
                storage: Some(Storage::ReadMember(Arc::new(IsoMemberSource {
                    source: source.clone(),
                    block_size: volume.block_size,
                    extents,
                }))),
            })?;
        }
    }
    if let Some(root) = volume.rock_ridge_root {
        merge_rock_ridge(&source, root, volume.block_size, &mut budget, limits, &mut index)?;
    }
    Ok(index)
}
pub(super) fn sniff(bytes: &[u8]) -> bool {
    ascii_at(bytes, DESCRIPTOR_START as usize + 1, b"CD001")
}
