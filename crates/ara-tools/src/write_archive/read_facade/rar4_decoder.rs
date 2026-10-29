//! Fixed OMP596f2da ar/rar/rar4-decoder.ts native RAR 2.9 LZ/Huffman.
//! Decompression structure adapted from unrar5j by Stephane Bury
//! (Copyright 2025, Apache License 2.0). Original notice retained.
//! OMP Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
const MAIN: usize = 299;
const DIST: usize = 60;
const LOW: usize = 17;
const LENGTH: usize = 28;
const TOTAL: usize = MAIN + DIST + LOW + LENGTH;
const LEN_BASE: [usize; 28] =
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 20, 24, 28, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224];
const LEN_BITS: [usize; 28] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5];
const SHORT_BASE: [usize; 8] = [0, 4, 8, 16, 32, 64, 128, 192];
const SHORT_BITS: [usize; 8] = [2, 2, 3, 4, 5, 6, 6, 6];
fn fail(detail: impl AsRef<str>) -> ToolError {
    error(format!("Invalid RAR4 archive: {}", detail.as_ref()))
}
struct Bits<'a> {
    bytes: &'a [u8],
    pos: usize,
    bit: usize,
}
impl<'a> Bits<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0, bit: 0 }
    }
    fn read(&mut self, count: usize) -> Result<u64> {
        let mut value = 0;
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
}
struct Huffman {
    counts: [usize; 16],
    first_code: [usize; 16],
    first_symbol: [usize; 16],
    symbols: Vec<usize>,
    ready: bool,
}
impl Huffman {
    fn new(size: usize) -> Self {
        Self { counts: [0; 16], first_code: [0; 16], first_symbol: [0; 16], symbols: vec![0; size], ready: false }
    }
    fn build(&mut self, lengths: &[u8], offset: usize, size: usize) {
        self.counts.fill(0);
        for &length in &lengths[offset..offset + size] {
            let length = (length & 15) as usize;
            if length != 0 {
                self.counts[length] += 1;
            }
        }
        let mut code = 0;
        let mut symbol_position = 0;
        for bits in 1..=15 {
            code = (code + self.counts[bits - 1]) << 1;
            self.first_code[bits] = code;
            self.first_symbol[bits] = symbol_position;
            symbol_position += self.counts[bits];
        }
        let mut next = self.first_symbol;
        for (symbol, &length) in lengths[offset..offset + size].iter().enumerate() {
            let length = (length & 15) as usize;
            if length != 0 {
                self.symbols[next[length]] = symbol;
                next[length] += 1;
            }
        }
        self.ready = symbol_position != 0;
    }
    fn decode(&self, bits: &mut Bits<'_>) -> Result<usize> {
        if !self.ready {
            return Err(fail("empty Huffman table"));
        }
        let mut code = 0;
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
struct Tables {
    main: Huffman,
    dist: Huffman,
    low: Huffman,
    length: Huffman,
}
impl Tables {
    fn new() -> Self {
        Self {
            main: Huffman::new(MAIN),
            dist: Huffman::new(DIST),
            low: Huffman::new(LOW),
            length: Huffman::new(LENGTH),
        }
    }
    fn read(&mut self, bits: &mut Bits<'_>, decoder: &mut Decoder) -> Result<()> {
        bits.align();
        if bits.read(1)? != 0 {
            return Err(error("Unsupported RAR4 PPMd compressed block"));
        }
        let keep = bits.read(1)? != 0;
        decoder.previous_low = 0;
        decoder.low_repeats = 0;
        if !keep {
            decoder.carried.fill(0);
        }
        let lengths = length_table(bits, &decoder.carried, TOTAL)?;
        self.main.build(&lengths, 0, MAIN);
        self.dist.build(&lengths, MAIN, DIST);
        self.low.build(&lengths, MAIN + DIST, LOW);
        self.length.build(&lengths, MAIN + DIST + LOW, LENGTH);
        decoder.carried.copy_from_slice(&lengths);
        Ok(())
    }
}
struct Filter {
    kind: usize,
    start: usize,
    size: usize,
    channels: i32,
    file_offset: i32,
}
pub(super) struct Decoder {
    history: Vec<u8>,
    old_distances: [usize; 4],
    last_distance: usize,
    last_length: usize,
    previous_low: usize,
    low_repeats: usize,
    carried: [u8; TOTAL],
    filter_types: Vec<usize>,
    filter_lengths: Vec<i64>,
    last_filter: usize,
}
impl Default for Decoder {
    fn default() -> Self {
        Self {
            history: Vec::new(),
            old_distances: [0; 4],
            last_distance: 0,
            last_length: 0,
            previous_low: 0,
            low_repeats: 0,
            carried: [0; TOTAL],
            filter_types: Vec::new(),
            filter_lengths: Vec::new(),
            last_filter: 0,
        }
    }
}
impl Decoder {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }
    fn remember(&mut self, distance: usize) {
        self.old_distances[3] = self.old_distances[2];
        self.old_distances[2] = self.old_distances[1];
        self.old_distances[1] = self.old_distances[0];
        self.old_distances[0] = distance;
    }
    fn filter(&mut self, bits: &mut Bits<'_>, position: usize, start: usize, size: usize) -> Result<Filter> {
        let first = bits.read(8)?;
        let mut code_size = (first & 7) as usize + 1;
        if code_size == 7 {
            code_size = bits.read(8)? as usize + 7;
        } else if code_size == 8 {
            code_size = bits.read(16)? as usize;
        }
        let mut code = Vec::with_capacity(code_size);
        for _ in 0..code_size {
            code.push(bits.read(8)? as u8);
        }
        let mut vm = VmBits::new(&code);
        let filter_index = if first & 0x80 != 0 {
            let index = vm.data()?;
            if index == 0 {
                self.filter_types.clear();
                self.filter_lengths.clear();
                0
            } else {
                index - 1
            }
        } else {
            self.last_filter as i64
        };
        if filter_index < 0 || filter_index as usize > self.filter_types.len() {
            return Err(fail("invalid RarVM filter index"));
        }
        let filter_index = filter_index as usize;
        self.last_filter = filter_index;
        let is_new = filter_index == self.filter_types.len();
        let mut offset = vm.data()?;
        if first & 0x40 != 0 {
            offset += 258;
        }
        offset += (position - start) as i64;
        let block_size =
            if first & 0x20 != 0 { vm.data()? } else { self.filter_lengths.get(filter_index).copied().unwrap_or(0) };
        let mut registers = [0i32; 7];
        registers[3] = 0x3c000;
        registers[4] = block_size as i32;
        if first & 0x10 != 0 {
            let mask = vm.read(7)?;
            for (index, register) in registers.iter_mut().enumerate() {
                if mask & (1 << index) != 0 {
                    *register = vm.data()? as i32;
                }
            }
        }
        let kind = if is_new {
            let program_size = vm.data()?;
            if program_size < 0 {
                return Err(error(format!("Invalid typed array length: {program_size}")));
            }
            let mut program = Vec::new();
            for _ in 0..program_size {
                program.push(vm.read(8)? as u8);
            }
            let kind = identify_filter(&program);
            self.filter_types.push(kind);
            self.filter_lengths.push(block_size);
            kind
        } else {
            self.filter_lengths[filter_index] = block_size;
            self.filter_types[filter_index]
        };
        if kind != 1 && kind != 2 && kind != 6 {
            return Err(error(format!(
                "Unsupported RAR4 RarVM filter type {}",
                if kind == 0 { "unknown".to_owned() } else { kind.to_string() }
            )));
        }
        if offset < 0 || block_size < 0 || offset + block_size > size as i64 {
            return Err(fail("invalid RarVM filter range"));
        }
        Ok(Filter {
            kind,
            start: offset as usize,
            size: block_size as usize,
            channels: registers[0],
            file_offset: registers[6],
        })
    }
    pub(super) fn decode(
        &mut self,
        packed: &[u8],
        size: usize,
        dictionary: usize,
        solid: bool,
        version: u64,
    ) -> Result<Vec<u8>> {
        if version < 29 {
            return Err(error(format!("Unsupported RAR4 compression algorithm version {version}")));
        }
        if !solid {
            self.reset();
        }
        let prior = &self.history[self.history.len().saturating_sub(dictionary)..];
        let mut output = vec![0; prior.len() + size + 260];
        output[..prior.len()].copy_from_slice(prior);
        let start = prior.len();
        let end = start + size;
        let mut position = start;
        let mut bits = Bits::new(packed);
        let mut tables = Tables::new();
        let mut filters = Vec::new();
        let counts = [4, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 14, 0, 12];
        let mut dist_base = [0usize; DIST];
        let mut dist_bits = [0usize; DIST];
        let mut distance = 0;
        let mut slot = 0;
        for (bits, count) in counts.into_iter().enumerate() {
            for _ in 0..count {
                dist_base[slot] = distance;
                dist_bits[slot] = bits;
                distance += 1 << bits;
                slot += 1;
            }
        }
        tables.read(&mut bits, self)?;
        while position < end {
            let symbol = tables.main.decode(&mut bits)?;
            if symbol < 256 {
                output[position] = symbol as u8;
                position += 1;
                continue;
            }
            if symbol == 256 {
                if bits.read(1)? != 0 {
                    tables.read(&mut bits, self)?;
                } else {
                    if bits.read(1)? != 0 {
                        tables.read(&mut bits, self)?;
                    }
                    break;
                }
                continue;
            }
            if symbol == 257 {
                filters.push(self.filter(&mut bits, position, start, size)?);
                continue;
            }
            let (distance, length) = if symbol >= 271 {
                let slot = symbol - 271;
                let mut length = LEN_BASE[slot] + 3 + bits.read(LEN_BITS[slot])? as usize;
                let distance_slot = tables.dist.decode(&mut bits)?;
                let mut distance = dist_base[distance_slot] + 1;
                let extra = dist_bits[distance_slot];
                if extra != 0 {
                    if distance_slot > 9 {
                        if extra > 4 {
                            distance += bits.read(extra - 4)? as usize * 16;
                        }
                        if self.low_repeats > 0 {
                            self.low_repeats -= 1;
                            distance += self.previous_low;
                        } else {
                            let low = tables.low.decode(&mut bits)?;
                            if low == 16 {
                                self.low_repeats = 15;
                                distance += self.previous_low;
                            } else {
                                distance += low;
                                self.previous_low = low;
                            }
                        }
                    } else {
                        distance += bits.read(extra)? as usize;
                    }
                }
                if distance >= 0x2000 {
                    length += 1;
                }
                if distance >= 0x40000 {
                    length += 1;
                }
                self.remember(distance);
                self.last_distance = distance;
                self.last_length = length;
                (distance, length)
            } else if symbol == 258 {
                if self.last_length == 0 {
                    continue;
                }
                (self.last_distance, self.last_length)
            } else if symbol < 263 {
                let index = symbol - 259;
                let distance = self.old_distances[index];
                for i in (1..=index).rev() {
                    self.old_distances[i] = self.old_distances[i - 1];
                }
                self.old_distances[0] = distance;
                let slot = tables.length.decode(&mut bits)?;
                let length = LEN_BASE[slot] + 2 + bits.read(LEN_BITS[slot])? as usize;
                self.last_distance = distance;
                self.last_length = length;
                (distance, length)
            } else {
                let slot = symbol - 263;
                let distance = SHORT_BASE[slot] + 1 + bits.read(SHORT_BITS[slot])? as usize;
                self.remember(distance);
                self.last_distance = distance;
                self.last_length = 2;
                (distance, 2)
            };
            if distance == 0 || distance > dictionary || distance > position {
                return Err(fail("invalid LZ distance"));
            }
            let length = length.min(end - position);
            for i in 0..length {
                output[position + i] = output[position + i - distance];
            }
            position += length;
        }
        if position != end {
            return Err(fail(format!("decompressed size mismatch ({} != {size})", position - start)));
        }
        let mut extracted = output[start..end].to_vec();
        apply_filters(&mut extracted, &filters);
        self.history = output[end.saturating_sub(dictionary)..end].to_vec();
        Ok(extracted)
    }
}
struct VmBits<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> VmBits<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
    fn read(&mut self, count: usize) -> Result<u64> {
        let mut value = 0;
        let mut left = count;
        while left > 0 {
            let pos = self.position >> 3;
            if pos >= self.bytes.len() {
                return Err(fail("truncated RarVM code"));
            }
            let bit = self.position & 7;
            let take = left.min(8 - bit);
            value = value * (1u64 << take) + ((self.bytes[pos] as u64 >> (8 - bit - take)) & ((1u64 << take) - 1));
            self.position += take;
            left -= take;
        }
        Ok(value)
    }
    fn data(&mut self) -> Result<i64> {
        match self.read(2)? {
            0 => Ok(self.read(4)? as i64),
            1 => {
                let first = self.read(4)?;
                if first == 0 {
                    Ok((self.read(8)? as u32 | 0xffffff00) as i32 as i64)
                } else {
                    Ok((first * 16 + self.read(4)?) as i64)
                }
            }
            2 => Ok(self.read(16)? as i64),
            _ => Ok((self.read(16)? * 0x10000 + self.read(16)?) as i64),
        }
    }
}
fn identify_filter(program: &[u8]) -> usize {
    match (program.len(), crc32fast::hash(program)) {
        (53, 0xad576887) => 1,
        (57, 0x3cd7e57e) => 2,
        (29, 0x0e06077d) => 6,
        (120, 0x3769893f) => 3,
        (149, 0x1c2c5dc8) => 4,
        (216, 0xbc85e701) => 5,
        (40, 0x46b9c560) => 7,
        _ => 0,
    }
}
fn apply_filters(output: &mut [u8], filters: &[Filter]) {
    for filter in filters {
        let data = &mut output[filter.start..filter.start + filter.size];
        if filter.kind == 6 {
            let source = data.to_vec();
            let mut source_position = 0;
            // The fixed signed VM register controls this loop: zero/negative channels
            // perform no writes. Positive channels retain the byte subtraction order.
            for channel in 0..filter.channels.max(0) as usize {
                let mut previous = 0u8;
                for position in (channel..data.len()).step_by(filter.channels as usize) {
                    previous = previous.wrapping_sub(source[source_position]);
                    source_position += 1;
                    data[position] = previous;
                }
            }
            continue;
        }
        let compare = if filter.kind == 2 { 0xe9 } else { 0xe8 };
        let mut position = 0;
        while position + 4 < data.len() {
            let opcode = data[position];
            position += 1;
            if opcode != 0xe8 && opcode != compare {
                continue;
            }
            let offset = position as i64 + filter.file_offset as u32 as i64;
            let address = i32::from_le_bytes(data[position..position + 4].try_into().unwrap()) as i64;
            let updated = if address < 0 {
                if address + offset >= 0 { Some(address + 0x1000000) } else { None }
            } else if address - 0x1000000 < 0 {
                Some(address - offset)
            } else {
                None
            };
            if let Some(value) = updated {
                data[position..position + 4].copy_from_slice(&(value as i32).to_le_bytes());
            }
            position += 4;
        }
    }
}
fn length_table(bits: &mut Bits<'_>, previous: &[u8], size: usize) -> Result<Vec<u8>> {
    let mut pre_lengths = [0u8; 20];
    let mut index = 0;
    while index < 20 {
        let length = bits.read(4)? as u8;
        if length == 15 {
            let count = bits.read(4)? as usize;
            if count != 0 {
                index = (index + count + 2).min(20);
                continue;
            }
        }
        pre_lengths[index] = length;
        index += 1;
    }
    let mut table = Huffman::new(20);
    table.build(&pre_lengths, 0, 20);
    let mut lengths = vec![0; size];
    let mut position = 0;
    while position < size {
        let symbol = table.decode(bits)?;
        if symbol < 16 {
            lengths[position] = ((symbol + previous[position] as usize) & 15) as u8;
            position += 1;
        } else {
            let short = symbol == 16 || symbol == 18;
            let count = bits.read(if short { 3 } else { 7 })? as usize + if short { 3 } else { 11 };
            let value = if symbol <= 17 && position != 0 { lengths[position - 1] } else { 0 };
            let end = (position + count).min(size);
            lengths[position..end].fill(value);
            position = end;
        }
    }
    Ok(lengths)
}
