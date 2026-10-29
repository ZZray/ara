//! Fixed OMP596f2da ar/sevenzip.ts indexed 7z headers, streams and members.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
use sevenzip_decode::{BindPair, Coder, Folder, FolderSource};
const SIGNATURE: &[u8] = &[0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c];
struct HeaderReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> HeaderReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }
    fn byte(&mut self) -> Result<u8> {
        let byte = self.bytes.get(self.pos).copied().ok_or_else(|| error("Invalid 7z header: truncated data"))?;
        self.pos += 1;
        Ok(byte)
    }
    fn bytes(&mut self, size: u64) -> Result<&'a [u8]> {
        if size > JS_SAFE || size > self.remaining() as u64 {
            return Err(error("Invalid 7z header: truncated or invalid-sized data"));
        }
        let bytes = &self.bytes[self.pos..self.pos + size as usize];
        self.pos += size as usize;
        Ok(bytes)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
    fn number_big(&mut self) -> Result<u64> {
        let first = self.byte()?;
        let mut value = 0u64;
        let mut mask = 0x80u8;
        for index in 0..8 {
            if first & mask == 0 {
                return Ok(value | (((first & (mask - 1)) as u64) << (index * 8)));
            }
            value |= (self.byte()? as u64) << (index * 8);
            mask >>= 1;
        }
        Ok(value)
    }
    fn number(&mut self, what: &str) -> Result<u64> {
        let value = self.number_big()?;
        if value > JS_SAFE {
            return Err(error(format!("7z {what} is too large to read safely")));
        }
        Ok(value)
    }
    fn skip_property(&mut self) -> Result<()> {
        let size = self.number("property size")?;
        self.bytes(size)?;
        Ok(())
    }
}
#[derive(Clone)]
struct Substream {
    folder: usize,
    offset: u64,
    size: u64,
    crc: Option<u32>,
}
#[derive(Default)]
struct Streams {
    pack_position: u64,
    pack_sizes: Vec<u64>,
    pack_crcs: Vec<Option<u32>>,
    folders: Vec<Folder>,
    substreams: Vec<Substream>,
}
#[derive(Default)]
struct Metadata {
    names: Vec<WireString>,
    empty_streams: Vec<bool>,
    empty_files: Vec<bool>,
    anti_files: Vec<bool>,
    mtimes: Vec<Option<f64>>,
    attributes: Vec<Option<u32>>,
}
fn js_array_count(count: usize) -> Result<()> {
    if count as u64 > u32::MAX as u64 {
        return Err(error("Invalid 7z archive: Invalid array length"));
    }
    Ok(())
}
fn bool_vector(reader: &mut HeaderReader<'_>, count: usize) -> Result<Vec<bool>> {
    js_array_count(count)?;
    let mut result = Vec::with_capacity(count);
    let mut byte = 0;
    let mut mask = 0u8;
    for _ in 0..count {
        if mask == 0 {
            byte = reader.byte()?;
            mask = 0x80;
        }
        result.push(byte & mask != 0);
        mask >>= 1;
    }
    Ok(result)
}
fn defined_vector(reader: &mut HeaderReader<'_>, count: usize) -> Result<Vec<bool>> {
    if reader.byte()? != 0 {
        js_array_count(count)?;
        Ok(vec![true; count])
    } else {
        bool_vector(reader, count)
    }
}
fn digests(reader: &mut HeaderReader<'_>, count: usize) -> Result<Vec<Option<u32>>> {
    let defined = defined_vector(reader, count)?;
    defined.into_iter().map(|present| if present { reader.u32().map(Some) } else { Ok(None) }).collect()
}
fn expect_id(reader: &mut HeaderReader<'_>, expected: u64, context: &str) -> Result<()> {
    let actual = reader.number("property ID")?;
    if actual != expected {
        return Err(error(format!("Invalid 7z header: expected {context} ID 0x{expected:x}, got 0x{actual:x}")));
    }
    Ok(())
}
fn parse_folder(reader: &mut HeaderReader<'_>, limits: &ReadLimits) -> Result<Folder> {
    let coder_count = reader.number("folder coder count")?;
    if coder_count == 0 || coder_count > limits.max_entries as u64 {
        return Err(error("Invalid 7z folder coder count"));
    }
    let mut folder = Folder::default();
    let mut inputs = 0usize;
    let mut outputs = 0usize;
    for _ in 0..coder_count {
        let flags = reader.byte()?;
        if flags & 0xc0 != 0 {
            return Err(error("Unsupported 7z coder alternatives or reserved flags"));
        }
        let id_size = flags & 0xf;
        if id_size == 0 || id_size > 8 {
            return Err(error("Invalid 7z coder method ID size"));
        }
        let mut id = 0u64;
        for &byte in reader.bytes(id_size as u64)? {
            id = (id << 8) | byte as u64;
        }
        let complex = flags & 0x10 != 0;
        let num_inputs = if complex { reader.number("coder input count")? as usize } else { 1 };
        let num_outputs = if complex { reader.number("coder output count")? as usize } else { 1 };
        if num_inputs == 0
            || num_outputs == 0
            || inputs + num_inputs > limits.max_entries
            || outputs + num_outputs > limits.max_entries
        {
            return Err(error("Invalid or excessive 7z coder stream count"));
        }
        let property_size = if flags & 0x20 != 0 { reader.number("coder properties size")? } else { 0 };
        limits.index(property_size, "coder properties")?;
        let properties = reader.bytes(property_size)?.to_vec();
        folder.coders.push(Coder {
            id,
            properties,
            input_start: inputs,
            output_start: outputs,
            inputs: num_inputs,
            outputs: num_outputs,
        });
        inputs += num_inputs;
        outputs += num_outputs;
    }
    let bind_count =
        outputs.checked_sub(1).filter(|&n| n <= inputs).ok_or_else(|| error("Invalid 7z folder bind-pair count"))?;
    let mut used_inputs = HashSet::new();
    let mut used_outputs = HashSet::new();
    for _ in 0..bind_count {
        let input = reader.number("bind input index")? as usize;
        let output = reader.number("bind output index")? as usize;
        if input >= inputs || output >= outputs || used_inputs.contains(&input) || used_outputs.contains(&output) {
            return Err(error("Invalid 7z folder bind pair"));
        }
        used_inputs.insert(input);
        used_outputs.insert(output);
        folder.bindings.push(BindPair { input, output });
    }
    let packed_count = inputs - bind_count;
    if packed_count == 1 {
        folder.packed_indices.extend((0..inputs).filter(|input| !used_inputs.contains(input)));
    } else {
        for _ in 0..packed_count {
            let packed = reader.number("packed stream index")? as usize;
            if packed >= inputs || used_inputs.contains(&packed) {
                return Err(error("Invalid 7z packed stream index"));
            }
            used_inputs.insert(packed);
            folder.packed_indices.push(packed);
        }
    }
    if folder.packed_indices.len() != packed_count {
        return Err(error("Invalid 7z folder packed-stream map"));
    }
    Ok(folder)
}
fn final_output(folder: &Folder) -> Result<usize> {
    let bound: HashSet<_> = folder.bindings.iter().map(|pair| pair.output).collect();
    let outputs: Vec<_> = folder
        .coders
        .iter()
        .flat_map(|c| c.output_start..c.output_start + c.outputs)
        .filter(|index| !bound.contains(index))
        .collect();
    if outputs.len() != 1 {
        return Err(error("Unsupported 7z folder with multiple final output streams"));
    }
    Ok(outputs[0])
}
fn pack_info(reader: &mut HeaderReader<'_>, streams: &mut Streams, limits: &ReadLimits) -> Result<()> {
    streams.pack_position = reader.number("pack position")?;
    let count = reader.number("pack stream count")? as usize;
    if count > limits.max_entries {
        return Err(error("7z has too many packed streams"));
    }
    loop {
        match reader.number("PackInfo property ID")? {
            0 => break,
            9 => {
                if !streams.pack_sizes.is_empty() {
                    return Err(error("Invalid duplicate 7z pack sizes"));
                }
                for _ in 0..count {
                    streams.pack_sizes.push(reader.number("packed stream size")?);
                }
            }
            10 => {
                if !streams.pack_crcs.is_empty() {
                    return Err(error("Invalid duplicate 7z pack CRCs"));
                }
                streams.pack_crcs = digests(reader, count)?;
            }
            _ => reader.skip_property()?,
        }
    }
    if streams.pack_sizes.len() != count {
        return Err(error("Invalid 7z PackInfo without complete sizes"));
    }
    if streams.pack_crcs.is_empty() {
        streams.pack_crcs = vec![None; count];
    }
    Ok(())
}
fn unpack_info(reader: &mut HeaderReader<'_>, streams: &mut Streams, limits: &ReadLimits) -> Result<()> {
    expect_id(reader, 11, "Folder")?;
    let count = reader.number("folder count")? as usize;
    if count > limits.max_entries {
        return Err(error("7z has too many folders"));
    }
    if reader.byte()? != 0 {
        return Err(error("Unsupported 7z external folder definitions"));
    }
    for _ in 0..count {
        streams.folders.push(parse_folder(reader, limits)?);
    }
    expect_id(reader, 12, "CodersUnpackSize")?;
    for folder in &mut streams.folders {
        let count: usize = folder.coders.iter().map(|c| c.outputs).sum();
        for _ in 0..count {
            folder.unpack_sizes.push(reader.number("coder unpack size")?);
        }
    }
    loop {
        match reader.number("UnpackInfo property ID")? {
            0 => break,
            10 => {
                let digests = digests(reader, count)?;
                for (index, crc) in digests.into_iter().enumerate() {
                    streams.folders[index].crc = crc;
                }
            }
            _ => reader.skip_property()?,
        }
    }
    Ok(())
}
fn substreams_info(reader: &mut HeaderReader<'_>, streams: &mut Streams) -> Result<()> {
    let mut counts = vec![1usize; streams.folders.len()];
    let mut sizes: Option<Vec<u64>> = None;
    let mut raw_digests: Option<Vec<Option<u32>>> = None;
    loop {
        match reader.number("SubStreamsInfo property ID")? {
            0 => break,
            13 => {
                for count in &mut counts {
                    *count = reader.number("folder substream count")? as usize;
                }
            }
            9 => {
                let mut values = Vec::new();
                for &count in &counts {
                    for _ in 1..count {
                        values.push(reader.number("substream size")?);
                    }
                }
                sizes = Some(values);
            }
            10 => {
                let count = counts
                    .iter()
                    .enumerate()
                    .filter(|&(index, &count)| count != 1 || streams.folders[index].crc.is_none())
                    .map(|(_, &count)| count)
                    // Preserve the fixed JS Array length domain. Keep an invalid
                    // sentinel until digests reads its all-defined marker, so a
                    // missing marker still reports the original truncation error.
                    .fold(0usize, |total, count| {
                        total.checked_add(count).unwrap_or(usize::MAX).min(u32::MAX as usize + 1)
                    });
                raw_digests = Some(digests(reader, count)?);
            }
            _ => reader.skip_property()?,
        }
    }
    let mut size_index = 0;
    let mut digest_index = 0;
    for (folder_index, folder) in streams.folders.iter().enumerate() {
        let count = counts[folder_index];
        let mut offset = 0u64;
        let folder_size = folder.unpack_sizes[final_output(folder)?];
        for index in 0..count {
            let size = if index + 1 == count {
                folder_size.checked_sub(offset).ok_or_else(|| error("Invalid 7z substream size total"))?
            } else {
                let size = sizes
                    .as_ref()
                    .and_then(|sizes| sizes.get(size_index))
                    .copied()
                    .ok_or_else(|| error("Invalid 7z SubStreamsInfo without sizes"))?;
                size_index += 1;
                size
            };
            if offset + size > folder_size {
                return Err(error("Invalid 7z substream size total"));
            }
            let crc = if count == 1 && folder.crc.is_some() {
                folder.crc
            } else {
                let crc = raw_digests.as_ref().and_then(|crcs| crcs.get(digest_index)).copied().flatten();
                digest_index += 1;
                crc
            };
            streams.substreams.push(Substream { folder: folder_index, offset, size, crc });
            offset += size;
        }
        if offset != folder_size {
            return Err(error("Invalid 7z substream sizes"));
        }
    }
    Ok(())
}
fn assign_pack_streams(streams: &mut Streams, source_size: u64) -> Result<()> {
    let mut absolute = 32 + streams.pack_position;
    let mut offsets = Vec::new();
    for &size in &streams.pack_sizes {
        if absolute > JS_SAFE || absolute.checked_add(size).is_none_or(|end| end > source_size) {
            return Err(error("Invalid 7z packed-stream range"));
        }
        offsets.push(absolute);
        absolute += size;
    }
    let mut index = 0;
    for folder in &mut streams.folders {
        for _ in &folder.packed_indices {
            if index >= streams.pack_sizes.len() {
                return Err(error("Invalid 7z folder-to-pack-stream mapping"));
            }
            folder.pack_offsets.push(offsets[index]);
            folder.pack_sizes.push(streams.pack_sizes[index]);
            folder.pack_crcs.push(streams.pack_crcs[index]);
            index += 1;
        }
    }
    if index != streams.pack_sizes.len() {
        return Err(error("Invalid unused 7z packed streams"));
    }
    Ok(())
}
fn streams_info(reader: &mut HeaderReader<'_>, source_size: u64, limits: &ReadLimits) -> Result<Streams> {
    let mut streams = Streams::default();
    loop {
        match reader.number("StreamsInfo property ID")? {
            0 => break,
            6 => pack_info(reader, &mut streams, limits)?,
            7 => unpack_info(reader, &mut streams, limits)?,
            8 => substreams_info(reader, &mut streams)?,
            id => return Err(error(format!("Unsupported 7z StreamsInfo property ID 0x{id:x}"))),
        }
    }
    if !streams.folders.is_empty() && streams.substreams.is_empty() {
        for (index, folder) in streams.folders.iter().enumerate() {
            streams.substreams.push(Substream {
                folder: index,
                offset: 0,
                size: folder.unpack_sizes[final_output(folder)?],
                crc: folder.crc,
            });
        }
    }
    assign_pack_streams(&mut streams, source_size)?;
    Ok(streams)
}
fn decode_metadata(source: &Source, streams: Streams, limits: &ReadLimits) -> Result<Vec<Vec<u8>>> {
    let mut buffers = Vec::new();
    for folder in streams.folders {
        let size = folder.unpack_sizes[final_output(&folder)?];
        limits.index(size, "decoded header")?;
        limits.memory(size)?;
        buffers.push(
            FolderSource::new(source.clone(), folder, *limits)
                .read_with_external(|err| error(format!("Invalid 7z archive: {}", err.0)))?
                .as_ref()
                .clone(),
        );
    }
    Ok(buffers)
}
fn property_stream<T>(
    reader: &mut HeaderReader<'_>,
    external: &[Vec<u8>],
    parse: impl FnOnce(&mut HeaderReader<'_>) -> Result<T>,
) -> Result<T> {
    let flag = reader.byte()?;
    if flag == 0 {
        return parse(reader);
    }
    if flag != 1 {
        return Err(error("Invalid 7z external-property flag"));
    }
    let index = reader.number("external property stream index")? as usize;
    let bytes = external.get(index).ok_or_else(|| error("Invalid 7z external property stream index"))?;
    parse(&mut HeaderReader::new(bytes))
}
fn names(
    reader: &mut HeaderReader<'_>,
    count: usize,
    external: &[Vec<u8>],
    limits: &ReadLimits,
) -> Result<Vec<WireString>> {
    property_stream(reader, external, |values| {
        let mut names = Vec::with_capacity(count);
        for _ in 0..count {
            let start = values.pos;
            while values.remaining() >= 2 && (values.bytes[values.pos] != 0 || values.bytes[values.pos + 1] != 0) {
                values.pos += 2;
            }
            let size = values.pos - start;
            if size > limits.max_path * 2 {
                return Err(error(format!("Archive encoded member path exceeds {} bytes", limits.max_path * 2)));
            }
            if values.remaining() < 2 {
                return Err(error("Invalid 7z UTF-16 file-name table"));
            }
            let mut units: Vec<_> =
                values.bytes[start..values.pos].chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
            if units.first() == Some(&0xfeff) {
                units.remove(0);
            }
            let name: WireString = String::from_utf16(&units).map_err(|_| error("Invalid 7z UTF-16 file name"))?.into();
            values.pos += 2;
            limits.path(path_len(&name), "member path")?;
            names.push(name);
        }
        Ok(names)
    })
}
fn times(reader: &mut HeaderReader<'_>, count: usize, external: &[Vec<u8>]) -> Result<Vec<Option<f64>>> {
    let defined = defined_vector(reader, count)?;
    property_stream(reader, external, |values| {
        let mut result = vec![None; count];
        for index in 0..count {
            if defined[index] {
                result[index] = Some((values.u64()? / 10000) as f64 - 11644473600000.0);
            }
        }
        Ok(result)
    })
}
fn attributes(reader: &mut HeaderReader<'_>, count: usize, external: &[Vec<u8>]) -> Result<Vec<Option<u32>>> {
    let defined = defined_vector(reader, count)?;
    property_stream(reader, external, |values| {
        let mut result = vec![None; count];
        for index in 0..count {
            if defined[index] {
                result[index] = Some(values.u32()?);
            }
        }
        Ok(result)
    })
}
fn files_info(reader: &mut HeaderReader<'_>, external: &[Vec<u8>], limits: &ReadLimits) -> Result<Metadata> {
    let count = reader.number("file count")? as usize;
    limits.entries(count)?;
    js_array_count(count)?;
    let mut metadata = Metadata {
        names: vec![WireString::from(""); count],
        empty_streams: vec![false; count],
        empty_files: Vec::new(),
        anti_files: Vec::new(),
        mtimes: vec![None; count],
        attributes: vec![None; count],
    };
    loop {
        let id = reader.number("FilesInfo property ID")?;
        if id == 0 {
            break;
        }
        let size = reader.number("FilesInfo property size")?;
        limits.index(size, "file property")?;
        let mut property = HeaderReader::new(reader.bytes(size)?);
        match id {
            14 => metadata.empty_streams = bool_vector(&mut property, count)?,
            15 => {
                metadata.empty_files =
                    bool_vector(&mut property, metadata.empty_streams.iter().filter(|&&empty| empty).count())?
            }
            16 => {
                metadata.anti_files =
                    bool_vector(&mut property, metadata.empty_streams.iter().filter(|&&empty| empty).count())?
            }
            17 => metadata.names = names(&mut property, count, external, limits)?,
            20 => metadata.mtimes = times(&mut property, count, external)?,
            21 => metadata.attributes = attributes(&mut property, count, external)?,
            25 => {
                // Match fixed source's exact first-byte-only zero check: after
                // one zero it marks the full dummy property consumed.
                while property.remaining() > 0 {
                    if property.byte()? != 0 {
                        return Err(error("Invalid non-zero 7z dummy property"));
                    }
                    property.pos = property.bytes.len();
                }
            }
            _ => {}
        }
        if property.remaining() != 0 {
            return Err(error(format!("Invalid 7z file property 0x{id:x} length")));
        }
    }
    Ok(metadata)
}
fn parse_header(source: &Source, bytes: &[u8], limits: &ReadLimits) -> Result<(Streams, Metadata)> {
    let mut reader = HeaderReader::new(bytes);
    expect_id(&mut reader, 1, "Header")?;
    let mut additional = Vec::new();
    let mut streams = None;
    let mut files = None;
    loop {
        match reader.number("Header property ID")? {
            0 => break,
            2 => loop {
                if reader.number("archive property ID")? == 0 {
                    break;
                }
                reader.skip_property()?;
            },
            3 => additional = decode_metadata(source, streams_info(&mut reader, source.size(), limits)?, limits)?,
            4 => streams = Some(streams_info(&mut reader, source.size(), limits)?),
            5 => files = Some(files_info(&mut reader, &additional, limits)?),
            id => return Err(error(format!("Unsupported 7z header property ID 0x{id:x}"))),
        }
    }
    if reader.remaining() != 0 {
        return Err(error("Invalid 7z trailing header data"));
    }
    Ok((streams.unwrap_or_default(), files.unwrap_or_default()))
}
struct MemberSource {
    folder: Arc<FolderSource>,
    offset: u64,
    size: u64,
    crc: Option<u32>,
}
impl ReadMemberSource for MemberSource {
    fn read(&self, size: u64, path: &WireString) -> Result<Vec<u8>> {
        self.read_with_external(size, path, |err| err)
    }
}
impl MemberSource {
    fn read_with_external(
        &self,
        size: u64,
        path: &WireString,
        wrap: impl FnOnce(ToolError) -> ToolError,
    ) -> Result<Vec<u8>> {
        if size != self.size {
            return Err(error(format!("7z member '{}' declared size changed during extraction", utf8(path))));
        }
        let folder = self.folder.read_with_external(wrap)?;
        if self.offset + size > folder.len() as u64 {
            return Err(error(format!("Invalid 7z member '{}' folder range", utf8(path))));
        }
        let bytes = &folder[self.offset as usize..(self.offset + size) as usize];
        if self.crc.is_some_and(|crc| crc32fast::hash(bytes) != crc) {
            return Err(error(format!("Invalid 7z member '{}' CRC32 mismatch", utf8(path))));
        }
        Ok(bytes.to_vec())
    }
}
struct EmptyMember;
impl ReadMemberSource for EmptyMember {
    fn read(&self, size: u64, path: &WireString) -> Result<Vec<u8>> {
        if size != 0 {
            return Err(error(format!("Invalid empty 7z member '{}' size", utf8(path))));
        }
        Ok(Vec::new())
    }
}
fn canonical_link(path: &WireString, target: &WireString) -> (WireString, bool) {
    let target = portable(target);
    if target.units().first() == Some(&47) {
        return (target, false);
    }
    let mut parts: Vec<Vec<u16>> = path.units().split(|&u| u == 47).map(|part| part.to_vec()).collect();
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
fn entries(source: &Source, streams: Streams, files: Metadata, limits: &ReadLimits) -> Result<Index> {
    let folder_sources: Vec<_> = streams
        .folders
        .into_iter()
        .map(|folder| Arc::new(FolderSource::new(source.clone(), folder, *limits)))
        .collect();
    let mut index = Index::default();
    index.read_limits = Some(*limits);
    let mut empty_index = 0;
    let mut stream_index = 0;
    let mut emitted_count = 0;
    for file_index in 0..files.names.len() {
        let path = normalize(&files.names[file_index], false);
        let empty = files.empty_streams.get(file_index).copied().unwrap_or(false);
        let empty_file = empty && files.empty_files.get(empty_index).copied().unwrap_or(false);
        let anti = empty && files.anti_files.get(empty_index).copied().unwrap_or(false);
        if empty {
            empty_index += 1;
        }
        let stream = if empty {
            None
        } else {
            let stream =
                streams.substreams.get(stream_index).ok_or_else(|| error("Invalid 7z file-to-substream mapping"))?;
            stream_index += 1;
            Some(stream)
        };
        let Some(path) = path.filter(|_| !anti) else {
            continue;
        };
        let attribute = files.attributes.get(file_index).copied().flatten();
        let mode = attribute.map(|a| a >> 16).filter(|&mode| mode != 0);
        let symlink = mode.is_some_and(|mode| mode & 0o170000 == 0o120000);
        let directory = (empty && !empty_file)
            || attribute.is_some_and(|a| a & 0x10 != 0)
            || mode.is_some_and(|m| m & 0o170000 == 0o040000);
        let size = stream.map_or(0, |stream| stream.size);
        limits.member(size, &path)?;
        let mtime = files.mtimes.get(file_index).copied().flatten();
        let storage = if symlink && stream.is_some() {
            limits.path(size as usize, "link target")?;
            let stream = stream.unwrap();
            let member = MemberSource {
                folder: folder_sources[stream.folder].clone(),
                offset: stream.offset,
                size: stream.size,
                crc: stream.crc,
            };
            let bytes = member.read_with_external(size, &path, |_| {
                error(format!("Invalid UTF-8 symlink target in 7z member '{}'", utf8(&path)))
            })?;
            let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
            let target: WireString = std::str::from_utf8(bytes)
                .map_err(|_| error(format!("Invalid UTF-8 symlink target in 7z member '{}'", utf8(&path))))?
                .into();
            limits.path(path_len(&target), "link target")?;
            let (target, resolve) = canonical_link(&path, &target);
            Some(Storage::Link { target, resolve })
        } else if !directory {
            Some(Storage::ReadMember(if let Some(stream) = stream {
                Arc::new(MemberSource {
                    folder: folder_sources[stream.folder].clone(),
                    offset: stream.offset,
                    size: stream.size,
                    crc: stream.crc,
                }) as Arc<dyn ReadMemberSource>
            } else {
                Arc::new(EmptyMember)
            }))
        } else {
            None
        };
        index.upsert(Entry { path, directory, size, mtime, mode, storage })?;
        emitted_count += 1;
        limits.entries(emitted_count)?;
    }
    if stream_index != streams.substreams.len() {
        return Err(error("Invalid unused 7z substreams"));
    }
    index.parents()?;
    Ok(index)
}
pub(super) fn sniff(bytes: &[u8]) -> bool {
    bytes.starts_with(SIGNATURE)
}
pub(super) fn read(source: &Source, limits: &ReadLimits) -> Result<Index> {
    if source.size() < 32 {
        return Err(error("Invalid 7z archive: truncated signature header"));
    }
    let signature =
        source::read_with_source_error(source, 0, 32, |err| error(format!("Invalid 7z archive: {}", err.0)))?;
    if !sniff(&signature) {
        return Err(error("Invalid 7z archive signature"));
    }
    if signature.get(6).copied().unwrap_or(0) != 0 {
        return Err(error(format!("Unsupported 7z major version {}", signature[6])));
    }
    if crc32fast::hash(signature.get(12..32).unwrap_or_default())
        != HeaderReader::new(signature.get(8..12).unwrap_or_default()).u32()?
    {
        return Err(error("Invalid 7z start-header CRC32 mismatch"));
    }
    let mut start = HeaderReader::new(signature.get(12..).unwrap_or_default());
    let next_offset = start.u64()?;
    let next_size = start.u64()?;
    let next_crc = start.u32()?;
    if next_offset > JS_SAFE || next_size > JS_SAFE {
        return Err(error("7z header offsets or sizes are too large to read safely"));
    }
    let header_start = 32 + next_offset;
    limits.index(next_size, "next header")?;
    if header_start > JS_SAFE || header_start + next_size > source.size() {
        return Err(error("Invalid 7z next-header range"));
    }
    let mut header = source::read_with_source_error(source, header_start, next_size, |err| {
        error(format!("Invalid 7z archive: {}", err.0))
    })?;
    if crc32fast::hash(&header) != next_crc {
        return Err(error("Invalid 7z next-header CRC32 mismatch"));
    }
    let first = HeaderReader::new(&header).number("next-header ID")?;
    if first == 23 {
        let mut reader = HeaderReader::new(&header);
        reader.number("number")?;
        let streams = streams_info(&mut reader, source.size(), limits)?;
        if reader.remaining() != 0 {
            return Err(error("Invalid 7z encoded-header trailing data"));
        }
        let mut decoded = decode_metadata(source, streams, limits)?;
        if decoded.len() != 1 {
            return Err(error("Invalid 7z encoded header folder count"));
        }
        header = decoded.remove(0);
        if HeaderReader::new(&header).number("decoded-header ID")? != 1 {
            return Err(error("Invalid decoded 7z header marker"));
        }
    } else if first != 1 {
        return Err(error(format!("Unsupported 7z next-header ID 0x{first:x}")));
    }
    let (streams, files) = parse_header(source, &header, limits)?;
    entries(source, streams, files, limits)
}
