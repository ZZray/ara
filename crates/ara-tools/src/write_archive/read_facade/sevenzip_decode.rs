//! Fixed OMP596f2da ar/sevenzip/decode.ts lazy 7z folder graph and solid cache.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
#[derive(Clone)]
pub(super) struct Coder {
    pub(super) id: u64,
    pub(super) properties: Vec<u8>,
    pub(super) input_start: usize,
    pub(super) output_start: usize,
    pub(super) inputs: usize,
    pub(super) outputs: usize,
}
#[derive(Clone)]
pub(super) struct BindPair {
    pub(super) input: usize,
    pub(super) output: usize,
}
#[derive(Clone, Default)]
pub(super) struct Folder {
    pub(super) coders: Vec<Coder>,
    pub(super) bindings: Vec<BindPair>,
    pub(super) packed_indices: Vec<usize>,
    pub(super) unpack_sizes: Vec<u64>,
    pub(super) pack_offsets: Vec<u64>,
    pub(super) pack_sizes: Vec<u64>,
    pub(super) pack_crcs: Vec<Option<u32>>,
    pub(super) crc: Option<u32>,
}
fn method_name(id: u64) -> String {
    match id {
        0 => "Copy",
        3 => "Delta",
        0x21 => "LZMA2",
        0x30101 => "LZMA",
        0x30401 => "PPMd",
        0x3030103 => "BCJ x86",
        0x303011b => "BCJ2",
        0x6f10701 => "7zAES",
        0x40108 => "Deflate",
        0x40109 => "Deflate64",
        0x40202 => "BZip2",
        0x3030205 => "BCJ PowerPC",
        0x3030401 => "BCJ IA64",
        0x3030501 => "BCJ ARM",
        0x3030701 => "BCJ ARM Thumb",
        0x3030805 => "BCJ SPARC",
        0xa => "BCJ ARM64",
        0xb => "BCJ RISC-V",
        _ => return format!("method 0x{id:x}"),
    }
    .into()
}
fn read32(bytes: &[u8], offset: usize) -> Result<u32> {
    if offset + 4 > bytes.len() {
        return Err(error("Invalid 7z BCJ properties"));
    }
    Ok(u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()))
}
fn delta(bytes: &mut [u8], distance: usize) {
    let mut history = [0u8; 256];
    let mut position = 0usize;
    for byte in bytes {
        let value = byte.wrapping_add(history[(distance + position) & 0xff]);
        history[position] = value;
        *byte = value;
        position = position.wrapping_sub(1) & 0xff;
    }
}
fn x86(bytes: &mut [u8], start: u32) -> Result<()> {
    const MASK_TO_BIT: [u32; 5] = [0, 1, 2, 2, 3];
    let mut previous_mask = 0u32;
    let mut previous_position = 0xfffffffbu32;
    if bytes.len() < 5 {
        return Ok(());
    }
    let mut position = 0usize;
    let limit = bytes.len() - 5;
    while position <= limit {
        let byte = bytes[position];
        if byte != 0xe8 && byte != 0xe9 {
            position += 1;
            continue;
        }
        let absolute = start.wrapping_add(position as u32);
        let offset = absolute.wrapping_sub(previous_position);
        previous_position = absolute;
        if offset > 5 {
            previous_mask = 0;
        } else {
            for _ in 0..offset {
                previous_mask = (previous_mask & 0x77) << 1;
            }
        }
        let byte = bytes[position + 4];
        if (byte == 0 || byte == 0xff) && previous_mask >> 1 <= 4 && previous_mask >> 1 != 3 {
            let mut source = read32(bytes, position + 1)?;
            let destination = loop {
                let destination = source.wrapping_sub(absolute).wrapping_sub(5);
                if previous_mask == 0 {
                    break destination;
                }
                let bit_index = MASK_TO_BIT[(previous_mask >> 1) as usize] * 8;
                let test_byte = (destination >> (24 - bit_index)) & 0xff;
                if test_byte != 0 && test_byte != 0xff {
                    break destination;
                }
                let low_mask = if bit_index == 0 { 0xffffffff } else { (1 << (32 - bit_index)) - 1 };
                source = destination ^ low_mask;
            };
            bytes[position + 4] = !((((destination >> 24) & 1) as i32) - 1) as u8;
            bytes[position + 3] = (destination >> 16) as u8;
            bytes[position + 2] = (destination >> 8) as u8;
            bytes[position + 1] = destination as u8;
            position += 5;
            previous_mask = 0;
        } else {
            position += 1;
            previous_mask |= 1;
            if byte == 0 || byte == 0xff {
                previous_mask |= 0x10;
            }
        }
    }
    Ok(())
}
pub(super) struct FolderSource {
    source: Source,
    folder: Folder,
    limits: ReadLimits,
    cached: std::sync::OnceLock<std::result::Result<Arc<Vec<u8>>, source::ReadSourceFailure>>,
}
impl FolderSource {
    pub(super) fn new(source: Source, folder: Folder, limits: ReadLimits) -> Self {
        Self { source, folder, limits, cached: Default::default() }
    }
    pub(super) fn read(&self) -> Result<Arc<Vec<u8>>> {
        self.read_with_external(|err| err)
    }
    pub(super) fn read_with_external(&self, wrap: impl FnOnce(ToolError) -> ToolError) -> Result<Arc<Vec<u8>>> {
        match self.cached.get_or_init(|| self.decode_classified()) {
            Ok(bytes) => Ok(bytes.clone()),
            Err(source::ReadSourceFailure::Archive(err)) => Err(err.clone()),
            Err(source::ReadSourceFailure::External(err)) => Err(wrap(err.clone())),
        }
    }
    fn packed_range(&self) -> Result<(u64, u64)> {
        let f = &self.folder;
        if f.packed_indices.len() != 1 || f.pack_offsets.len() != 1 || f.pack_sizes.len() != 1 {
            if f.coders.iter().any(|c| c.id == 0x303011b) {
                return Err(error("Unsupported 7z coder BCJ2 (multi-input folder)"));
            }
            return Err(error("Unsupported 7z folder graph with multiple packed input streams"));
        }
        let offset = f.pack_offsets[0];
        let size = f.pack_sizes[0];
        if offset > JS_SAFE || size > JS_SAFE || offset.checked_add(size).is_none_or(|end| end > self.source.size()) {
            return Err(error("Invalid 7z packed-stream range"));
        }
        Ok((offset, size))
    }
    fn decode_classified(&self) -> std::result::Result<Arc<Vec<u8>>, source::ReadSourceFailure> {
        use source::ReadSourceFailure::Archive;
        let (offset, size) = self.packed_range().map_err(Archive)?;
        validate_range(offset, size).map_err(Archive)?;
        let bytes = match &self.source {
            Source::Read(source) => source.range_classified(offset, size)?,
            Source::Memory(_) => self.source.range(offset, size).map_err(Archive)?,
            Source::File { .. } => {
                return Err(Archive(error("READ archive source must retain read-only source ownership")));
            }
        };
        self.decode_bytes(bytes).map(Arc::new).map_err(Archive)
    }
    fn decode_bytes(&self, mut bytes: Vec<u8>) -> Result<Vec<u8>> {
        let f = &self.folder;
        if f.pack_crcs[0].is_some_and(|crc| crc32fast::hash(&bytes) != crc) {
            return Err(error("Invalid 7z packed-stream CRC32 mismatch"));
        }
        let mut input_index = f.packed_indices[0];
        let mut visited = HashSet::new();
        loop {
            let index = f
                .coders
                .iter()
                .position(|c| input_index >= c.input_start && input_index < c.input_start + c.inputs)
                .filter(|&index| !visited.contains(&index))
                .ok_or_else(|| error("Invalid 7z folder binding graph"))?;
            visited.insert(index);
            let c = &f.coders[index];
            if c.inputs != 1 || c.outputs != 1 {
                return Err(error(format!("Unsupported 7z coder {} stream topology", method_name(c.id))));
            }
            let output_size =
                *f.unpack_sizes.get(c.output_start).ok_or_else(|| error("Invalid 7z coder unpack size"))?;
            self.limits.memory(output_size)?;
            match c.id {
                0 => {
                    if !c.properties.is_empty() || bytes.len() as u64 != output_size {
                        return Err(error("Invalid 7z Copy coder size or properties"));
                    }
                }
                0x30101 => bytes = codecs::read_lzma_raw(&c.properties, &bytes, output_size)?,
                0x21 => {
                    if c.properties.len() != 1 {
                        return Err(error("Invalid 7z LZMA2 coder properties"));
                    }
                    bytes = codecs::read_lzma2(c.properties[0], &bytes, output_size)?;
                    if bytes.len() as u64 != output_size {
                        return Err(error("7z LZMA2 coder output size mismatch"));
                    }
                }
                3 => {
                    if c.properties.len() != 1 || bytes.len() as u64 != output_size {
                        return Err(error("Invalid 7z Delta coder properties or size"));
                    }
                    delta(&mut bytes, c.properties[0] as usize + 1);
                }
                0x3030103 => {
                    if !c.properties.is_empty() && c.properties.len() != 4 {
                        return Err(error("Invalid 7z BCJ x86 coder properties"));
                    }
                    if bytes.len() as u64 != output_size {
                        return Err(error("7z BCJ x86 coder output size mismatch"));
                    }
                    let start = if c.properties.len() == 4 { read32(&c.properties, 0)? } else { 0 };
                    x86(&mut bytes, start)?;
                }
                0x30401 => return Err(error("Unsupported 7z coder PPMd")),
                0x6f10701 => return Err(error("Encrypted 7z archives use unsupported coder 7zAES")),
                _ => return Err(error(format!("Unsupported 7z coder {}", method_name(c.id)))),
            }
            let Some(binding) = f.bindings.iter().find(|b| b.output == c.output_start) else {
                break;
            };
            input_index = binding.input;
        }
        if visited.len() != f.coders.len() {
            return Err(error("Unsupported 7z folder graph with disconnected coders"));
        }
        if f.crc.is_some_and(|crc| crc32fast::hash(&bytes) != crc) {
            return Err(error("Invalid 7z folder CRC32 mismatch"));
        }
        Ok(bytes)
    }
}
