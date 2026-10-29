//! Fixed OMP596f2da ar/rar/rar5-decoder.ts native stateful RAR5 LZSS decoder.
//! Decompression structure adapted from unrar5j by Stephane Bury
//! (Copyright 2025, Apache License 2.0). Original notice retained.
//! OMP Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
const MAIN: usize = 306;
const DIST: usize = 80;
const ALIGN: usize = 16;
const LENGTH: usize = 44;
const TABLE: usize = MAIN + DIST + ALIGN + LENGTH;
const MAX_MATCH: usize = 0x1004;
fn fail(detail: impl AsRef<str>) -> ToolError {
    error(format!("Invalid RAR5 archive: {}", detail.as_ref()))
}
struct Bits<'a> {
    bytes: &'a [u8],
    pos: usize,
    bit: usize,
    block_end: usize,
    block_end_bit: usize,
}
impl<'a> Bits<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0, bit: 0, block_end: 0, block_end_bit: 0 }
    }
    fn read(&mut self, count: usize) -> Result<u64> {
        if count == 0 {
            return Ok(0);
        }
        if count > 31 {
            return Err(fail("invalid bit count"));
        }
        let mut value = 0u64;
        let mut left = count;
        while left > 0 {
            if self.pos >= self.bytes.len() {
                return Err(fail("truncated compressed data"));
            }
            let take = left.min(8 - self.bit);
            value = value * (1u64 << take)
                + ((self.bytes[self.pos] as u64 >> (8 - self.bit - take)) & ((1u64 << take) - 1));
            self.bit += take;
            left -= take;
            if self.bit == 8 {
                self.bit = 0;
                self.pos += 1;
            }
        }
        Ok(value)
    }
    fn align(&mut self) {
        if self.bit != 0 {
            self.pos += 1;
            self.bit = 0;
        }
    }
    fn byte(&mut self) -> Result<u8> {
        if self.bit != 0 {
            return Err(fail("unaligned compressed data"));
        }
        let byte = self.bytes.get(self.pos).copied().ok_or_else(|| fail("truncated compressed data"))?;
        self.pos += 1;
        Ok(byte)
    }
    fn at_block_end(&self) -> bool {
        self.pos > self.block_end || (self.pos == self.block_end && self.bit >= self.block_end_bit)
    }
}
struct Huffman {
    size: usize,
    counts: [usize; 16],
    first_code: [usize; 16],
    first_symbol: [usize; 16],
    symbols: Vec<usize>,
    ready: bool,
}
impl Huffman {
    fn new(size: usize) -> Self {
        Self { size, counts: [0; 16], first_code: [0; 16], first_symbol: [0; 16], symbols: vec![0; size], ready: false }
    }
    fn build(&mut self, lengths: &[u8], offset: usize) -> Result<()> {
        self.counts.fill(0);
        for &length in &lengths[offset..offset + self.size] {
            if length > 15 {
                return Err(fail("invalid Huffman code length"));
            }
            if length != 0 {
                self.counts[length as usize] += 1;
            }
        }
        let mut code = 0;
        let mut symbols = 0;
        for bits in 1..=15 {
            code = (code + self.counts[bits - 1]) << 1;
            self.first_code[bits] = code;
            self.first_symbol[bits] = symbols;
            symbols += self.counts[bits];
        }
        if symbols != 0 && code + self.counts[15] != 1 << 15 {
            return Err(fail("incomplete Huffman table"));
        }
        let mut next = self.first_symbol;
        for (symbol, &length) in lengths[offset..offset + self.size].iter().enumerate() {
            if length != 0 {
                self.symbols[next[length as usize]] = symbol;
                next[length as usize] += 1;
            }
        }
        self.ready = symbols != 0;
        Ok(())
    }
    fn decode(&self, bits: &mut Bits<'_>) -> Result<usize> {
        if !self.ready {
            return Err(fail("empty Huffman table"));
        }
        let mut code = 0usize;
        for length in 1..=15 {
            code = (code << 1) | bits.read(1)? as usize;
            if let Some(index) = code.checked_sub(self.first_code[length])
                && index < self.counts[length]
            {
                return Ok(self.symbols[self.first_symbol[length] + index]);
            }
        }
        Err(fail("invalid Huffman symbol"))
    }
}
struct Filter {
    kind: u64,
    channels: usize,
    start: usize,
    size: usize,
}
pub(super) struct Decoder {
    history: Vec<u8>,
    reps: [u64; 4],
    last_length: usize,
    main: Huffman,
    dist: Huffman,
    align: Huffman,
    length: Huffman,
    use_align: bool,
}
impl Default for Decoder {
    fn default() -> Self {
        Self {
            history: Vec::new(),
            reps: [0xffffffff; 4],
            last_length: 0,
            main: Huffman::new(MAIN),
            dist: Huffman::new(DIST),
            align: Huffman::new(ALIGN),
            length: Huffman::new(LENGTH),
            use_align: false,
        }
    }
}
impl Decoder {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }
    fn read_tables(&mut self, bits: &mut Bits<'_>, version: u64) -> Result<bool> {
        bits.align();
        let flags = bits.byte()?;
        let mut checksum = flags ^ bits.byte()?;
        let size_bytes = (flags >> 3) & 3;
        if size_bytes == 3 {
            return Err(fail("invalid compressed block header"));
        }
        let mut block_size = bits.byte()? as usize;
        checksum ^= block_size as u8;
        for index in 0..size_bytes {
            let byte = bits.byte()?;
            checksum ^= byte;
            block_size += (byte as usize) << (8 * (index as usize + 1));
        }
        if checksum != 0x5a {
            return Err(fail("compressed block checksum mismatch"));
        }
        let mut end_bits = (flags & 7) as usize + 1;
        block_size += end_bits >> 3;
        if block_size == 0 {
            return Err(fail("empty compressed block"));
        }
        block_size -= 1;
        end_bits &= 7;
        bits.block_end = bits.pos + block_size;
        bits.block_end_bit = end_bits;
        let last = flags & 0x40 != 0;
        if flags & 0x80 == 0 {
            return Ok(last);
        }
        let mut level_lengths = [0u8; 20];
        let mut index = 0;
        while index < 20 {
            let length = bits.read(4)?;
            if length == 15 {
                let zeros = bits.read(4)?;
                if zeros != 0 {
                    index = 20.min(index + zeros as usize + 2);
                    continue;
                }
            }
            level_lengths[index] = length as u8;
            index += 1;
        }
        let mut level = Huffman::new(20);
        level.build(&level_lengths, 0)?;
        let table_length = if version == 1 { TABLE } else { TABLE - 16 };
        let mut compact = vec![0u8; table_length];
        let mut index = 0;
        while index < table_length {
            let symbol = level.decode(bits)?;
            if symbol < 16 {
                compact[index] = symbol as u8;
                index += 1;
                continue;
            }
            let base = ((symbol - 16) & 1) * 4;
            let count = base * 2 + 3 + bits.read(base + 3)? as usize;
            let value = if symbol < 18 {
                if index == 0 {
                    return Err(fail("invalid Huffman repeat"));
                }
                compact[index - 1]
            } else {
                0
            };
            let end = table_length.min(index + count);
            compact[index..end].fill(value);
            index = end;
        }
        let mut lengths = [0u8; TABLE];
        if version == 0 {
            lengths[..MAIN + 64].copy_from_slice(&compact[..MAIN + 64]);
            lengths[MAIN + DIST..].copy_from_slice(&compact[MAIN + 64..]);
        } else {
            lengths.copy_from_slice(&compact);
        }
        self.main.build(&lengths, 0)?;
        self.dist.build(&lengths, MAIN)?;
        self.align.build(&lengths, MAIN + DIST)?;
        self.length.build(&lengths, MAIN + DIST + ALIGN)?;
        self.use_align = lengths[MAIN + DIST..MAIN + DIST + ALIGN].iter().any(|&length| length != 4);
        Ok(last)
    }
    pub(super) fn decode(
        &mut self,
        packed: &[u8],
        size: usize,
        dictionary: usize,
        solid: bool,
        version: u64,
    ) -> Result<Vec<u8>> {
        if version != 0 && version != 1 {
            return Err(error(format!("Unsupported RAR5 compression algorithm version {version}")));
        }
        if !solid {
            self.reset();
        }
        if dictionary < 128 * 1024 || dictionary as u64 > JS_SAFE {
            return Err(fail("invalid dictionary size"));
        }
        let prior = &self.history[self.history.len().saturating_sub(dictionary)..];
        let mut data = vec![0u8; prior.len() + size + MAX_MATCH];
        data[..prior.len()].copy_from_slice(prior);
        let start = prior.len();
        let end = start + size;
        let mut position = start;
        let mut filters = Vec::new();
        let mut bits = Bits::new(packed);
        let mut last = self.read_tables(&mut bits, version)?;
        while position < end {
            if bits.at_block_end() {
                if bits.pos > bits.block_end || (bits.pos == bits.block_end && bits.bit > bits.block_end_bit) {
                    return Err(fail("compressed block overread"));
                }
                bits.align();
                if last {
                    break;
                }
                last = self.read_tables(&mut bits, version)?;
                continue;
            }
            let symbol = self.main.decode(&mut bits)?;
            if symbol < 256 {
                data[position] = symbol as u8;
                position += 1;
                continue;
            }
            if symbol == 256 {
                let bytes = (bits.read(2)? as usize + 1) * 8;
                let mut filter_start = 0usize;
                for shift in (0..bytes).step_by(8) {
                    filter_start += (bits.read(8)? as usize) << shift;
                }
                let bytes = (bits.read(2)? as usize + 1) * 8;
                let mut filter_size = 0usize;
                for shift in (0..bytes).step_by(8) {
                    filter_size += (bits.read(8)? as usize) << shift;
                }
                let kind = bits.read(3)?;
                let channels = if kind == 0 { bits.read(5)? as usize + 1 } else { 0 };
                if kind > 3 {
                    return Err(error(format!("Unsupported RAR5 filter type {kind}")));
                }
                if filter_size > 1 << 22 || position - start + filter_start + filter_size > size {
                    return Err(fail("invalid filter range"));
                }
                filters.push(Filter { kind, channels, start: position - start + filter_start, size: filter_size });
                continue;
            }
            let mut distance = self.reps[0];
            let length;
            if symbol < 262 {
                if symbol >= 258 {
                    let index = symbol - 258;
                    distance = self.reps[index];
                    for i in (1..=index).rev() {
                        self.reps[i] = self.reps[i - 1];
                    }
                    self.reps[0] = distance;
                    let slot = self.length.decode(&mut bits)?;
                    length = if slot >= 8 { slot_length(&mut bits, slot)? } else { slot } + 2;
                    self.last_length = length;
                } else {
                    length = self.last_length;
                    if length == 0 {
                        continue;
                    }
                }
            } else {
                self.reps[3] = self.reps[2];
                self.reps[2] = self.reps[1];
                self.reps[1] = self.reps[0];
                let slot = symbol - 262;
                let mut value = if slot >= 8 { slot_length(&mut bits, slot)? } else { slot } + 2;
                let mut distance_slot = self.dist.decode(&mut bits)? as u64;
                if distance_slot >= 4 {
                    let extra = ((distance_slot - 2) >> 1) as usize;
                    distance_slot = (2 | (distance_slot & 1)) * (1u64 << extra);
                    if extra < 4 {
                        distance_slot += bits.read(extra)?;
                    } else {
                        value += if extra <= 6 {
                            0
                        } else if extra <= 11 {
                            1
                        } else if extra <= 16 {
                            2
                        } else {
                            3
                        };
                        if self.use_align {
                            distance_slot += bits.read(extra - 4)? * 16 + self.align.decode(&mut bits)? as u64;
                        } else {
                            distance_slot += bits.read(extra)?;
                        }
                    }
                }
                distance = distance_slot + 1;
                self.reps[0] = distance;
                self.last_length = value;
                length = value;
            }
            if distance == 0 || distance > dictionary as u64 || distance > position as u64 {
                return Err(fail("invalid LZ distance"));
            }
            if position + length > data.len() {
                return Err(fail("invalid LZ match length"));
            }
            for index in 0..length {
                data[position + index] = data[position + index - distance as usize];
            }
            position += length;
        }
        if position != end {
            return Err(fail(format!("decompressed size mismatch ({} != {size})", position - start)));
        }
        let mut output = data[start..end].to_vec();
        apply_filters(&mut output, &filters);
        self.history = data[end.saturating_sub(dictionary)..end].to_vec();
        Ok(output)
    }
}
fn slot_length(bits: &mut Bits<'_>, slot: usize) -> Result<usize> {
    let count = (slot >> 2) - 1;
    Ok(((4 | (slot & 3)) << count) + bits.read(count)? as usize)
}
fn read32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn write32(bytes: &mut [u8], offset: usize, value: i32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn apply_filters(output: &mut [u8], filters: &[Filter]) {
    for filter in filters {
        let data = &mut output[filter.start..filter.start + filter.size];
        if filter.kind == 0 {
            let source = data.to_vec();
            let mut source_position = 0;
            for channel in 0..filter.channels {
                let mut previous = 0u8;
                for position in (channel..data.len()).step_by(filter.channels) {
                    previous = previous.wrapping_sub(source[source_position]);
                    source_position += 1;
                    data[position] = previous;
                }
            }
        } else if filter.kind == 1 || filter.kind == 2 {
            const FILE_SIZE: u32 = 1 << 24;
            let mut position = 0;
            while position + 4 < data.len() {
                let opcode = data[position];
                position += 1;
                if opcode != 0xe8 && (filter.kind == 1 || opcode != 0xe9) {
                    continue;
                }
                let offset = ((filter.start + position) & (FILE_SIZE as usize - 1)) as u32;
                let mut address = read32(data, position);
                if (address as u32) < FILE_SIZE {
                    address = address.wrapping_sub(offset as i32);
                } else if (address as u32) >= 0u32.wrapping_sub(offset) {
                    address = address.wrapping_add(FILE_SIZE as i32);
                } else {
                    position += 4;
                    continue;
                }
                write32(data, position, address);
                position += 4;
            }
        } else {
            for position in (0..data.len().saturating_sub(3)).step_by(4) {
                if data[position + 3] != 0xeb {
                    continue;
                }
                let instruction = read32(data, position) as u32;
                let address = (instruction & 0xff000000)
                    | (instruction.wrapping_sub(((filter.start + position) >> 2) as u32) & 0xffffff);
                write32(data, position, address as i32);
            }
        }
    }
}
