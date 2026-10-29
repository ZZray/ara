//! Fixed OMP596f2da ar/codecs/lzx.ts stateful Microsoft LZX decoder for CAB.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
const FRAME_SIZE: usize = 32768;
const POSITION_SLOTS: [usize; 7] = [30, 32, 34, 36, 38, 42, 50];
struct BitReader<'a> {
    bytes: &'a [u8],
    offset: usize,
    word: u16,
    remaining: usize,
}
impl<'a> BitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0, word: 0, remaining: 0 }
    }
    fn bits(&mut self, count: usize) -> Result<u32> {
        let mut value = 0u32;
        let mut needed = count;
        while needed > 0 {
            if self.remaining == 0 {
                if self.offset + 2 > self.bytes.len() {
                    return Err(error("Invalid CAB archive: truncated LZX bitstream"));
                }
                self.word = u16::from_le_bytes(self.bytes[self.offset..self.offset + 2].try_into().unwrap());
                self.offset += 2;
                self.remaining = 16;
            }
            let take = needed.min(self.remaining);
            value = value * (1 << take) + (((self.word as u32) >> (self.remaining - take)) & ((1 << take) - 1));
            self.remaining -= take;
            needed -= take;
        }
        Ok(value)
    }
    fn align_word(&mut self) {
        self.remaining = 0;
    }
    fn byte(&mut self) -> Result<u8> {
        if self.remaining != 0 {
            return Err(error("Invalid CAB archive: misaligned LZX byte stream"));
        }
        let value =
            self.bytes.get(self.offset).copied().ok_or_else(|| error("Invalid CAB archive: truncated LZX data"))?;
        self.offset += 1;
        Ok(value)
    }
    fn u32_le(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes([self.byte()?, self.byte()?, self.byte()?, self.byte()?]))
    }
}
struct Huffman {
    counts: [u32; 17],
    first_codes: [u32; 17],
    first_symbols: [usize; 17],
    symbols: Vec<usize>,
    empty: bool,
}
impl Huffman {
    fn new(lengths: &[u8], allow_empty: bool) -> Result<Self> {
        let mut counts = [0u32; 17];
        let mut symbol_count = 0;
        for &length in lengths {
            if length > 16 {
                return Err(error("Invalid CAB archive: invalid LZX Huffman code length"));
            }
            if length != 0 {
                counts[length as usize] += 1;
                symbol_count += 1;
            }
        }
        let empty = symbol_count == 0;
        if empty && !allow_empty {
            return Err(error("Invalid CAB archive: empty LZX Huffman tree"));
        }
        let mut code = 0;
        let mut offset = 0;
        let mut first_codes = [0u32; 17];
        let mut first_symbols = [0usize; 17];
        for length in 1..=16 {
            code = (code + counts[length - 1]) * 2;
            if code + counts[length] > 1 << length {
                return Err(error("Invalid CAB archive: oversubscribed LZX Huffman tree"));
            }
            first_codes[length] = code;
            first_symbols[length] = offset;
            offset += counts[length] as usize;
        }
        let mut symbols = vec![0; symbol_count];
        let mut next = first_symbols;
        for (symbol, &length) in lengths.iter().enumerate() {
            if length != 0 {
                symbols[next[length as usize]] = symbol;
            }
            next[length as usize] += 1;
        }
        Ok(Self { counts, first_codes, first_symbols, symbols, empty })
    }
    fn decode(&self, reader: &mut BitReader<'_>) -> Result<usize> {
        if self.empty {
            return Err(error("Invalid CAB archive: LZX stream uses an empty Huffman tree"));
        }
        let mut code = 0;
        for length in 1..=16 {
            code = code * 2 + reader.bits(1)?;
            if let Some(relative) = code.checked_sub(self.first_codes[length])
                && relative < self.counts[length]
            {
                return Ok(self.symbols[self.first_symbols[length] + relative as usize]);
            }
        }
        Err(error("Invalid CAB archive: invalid LZX Huffman symbol"))
    }
}
fn code_lengths(reader: &mut BitReader<'_>, lengths: &mut [u8], first: usize, last: usize) -> Result<()> {
    let mut pretree_lengths = [0u8; 20];
    for value in &mut pretree_lengths {
        *value = reader.bits(4)? as u8;
    }
    let pretree = Huffman::new(&pretree_lengths, false)?;
    let mut index = first;
    while index < last {
        let symbol = pretree.decode(reader)?;
        if symbol == 17 || symbol == 18 {
            let run = reader.bits(if symbol == 17 { 4 } else { 5 })? as usize + if symbol == 17 { 4 } else { 20 };
            if index + run > last {
                return Err(error("Invalid CAB archive: LZX code-length run exceeds its tree"));
            }
            lengths[index..index + run].fill(0);
            index += run;
        } else if symbol == 19 {
            let run = reader.bits(1)? as usize + 4;
            if index + run > last {
                return Err(error("Invalid CAB archive: LZX code-length run exceeds its tree"));
            }
            let delta = pretree.decode(reader)?;
            // JS signed remainder, then Uint8Array assignment wraps negatives.
            let length = ((lengths[index] as i32 - delta as i32 + 17) % 17) as u8;
            lengths[index..index + run].fill(length);
            index += run;
        } else {
            lengths[index] = ((lengths[index] as i32 - symbol as i32 + 17) % 17) as u8;
            index += 1;
        }
    }
    Ok(())
}
pub(super) struct Decoder {
    window: Vec<u8>,
    position_base: Vec<u32>,
    extra_bits: Vec<usize>,
    main_lengths: Vec<u8>,
    length_lengths: Vec<u8>,
    main_table: Option<Huffman>,
    length_table: Option<Huffman>,
    aligned_table: Option<Huffman>,
    window_position: usize,
    decoded_size: u64,
    frame: u32,
    r0: u32,
    r1: u32,
    r2: u32,
    header_read: bool,
    intel_file_size: i64,
    intel_started: bool,
    block_type: u32,
    block_length: usize,
    block_remaining: usize,
    uncompressed_padding: bool,
}
impl Decoder {
    pub(super) fn new(window_bits: usize) -> Result<Self> {
        if !(15..=21).contains(&window_bits) {
            return Err(error(format!("Unsupported CAB LZX window size: {window_bits} bits (expected 15-21)")));
        }
        let slots = POSITION_SLOTS[window_bits - 15];
        let mut extra_bits = vec![0; slots];
        let mut position_base = vec![0u32; slots];
        for slot in 0..slots {
            extra_bits[slot] = if slot < 4 { 0 } else { 17.min(slot / 2 - 1) };
            if slot > 0 {
                position_base[slot] = position_base[slot - 1] + (1 << extra_bits[slot - 1]);
            }
        }
        Ok(Self {
            window: vec![0; 1 << window_bits],
            position_base,
            extra_bits,
            main_lengths: vec![0; 256 + slots * 8],
            length_lengths: vec![0; 249],
            main_table: None,
            length_table: None,
            aligned_table: None,
            window_position: 0,
            decoded_size: 0,
            frame: 0,
            r0: 1,
            r1: 1,
            r2: 1,
            header_read: false,
            intel_file_size: 0,
            intel_started: false,
            block_type: 0,
            block_length: 0,
            block_remaining: 0,
            uncompressed_padding: false,
        })
    }
    pub(super) fn frame(&mut self, bytes: &[u8], size: usize) -> Result<Vec<u8>> {
        if size > FRAME_SIZE {
            return Err(error(format!("Invalid CAB archive: LZX frame size {size} exceeds 32768 bytes")));
        }
        if size == 0 {
            return Ok(Vec::new());
        }
        let mut reader = BitReader::new(bytes);
        if !self.header_read {
            if reader.bits(1)? != 0 {
                let high = reader.bits(16)?;
                let low = reader.bits(16)?;
                self.intel_file_size = ((high << 16 | low) as i32) as i64;
            }
            self.header_read = true;
        }
        let mut raw = vec![0u8; size];
        let mut position = 0;
        while position < size {
            if self.block_remaining == 0 {
                self.block_header(&mut reader)?;
            }
            let run = self.block_remaining.min(size - position);
            let produced = self.decode_run(&mut reader, &mut raw, position, run)?;
            if produced != run {
                return Err(error("Invalid CAB archive: LZX block produced the wrong byte count"));
            }
            position += produced;
            self.block_remaining -= produced;
        }
        if self.block_remaining == 0 && self.block_type == 3 && self.uncompressed_padding {
            reader.byte()?;
            self.uncompressed_padding = false;
        }
        reader.align_word();
        self.translate_e8(&mut raw);
        self.frame += 1;
        Ok(raw)
    }
    fn block_header(&mut self, reader: &mut BitReader<'_>) -> Result<()> {
        if self.block_type == 3 && self.uncompressed_padding {
            reader.byte()?;
        }
        self.uncompressed_padding = false;
        self.block_type = reader.bits(3)?;
        self.block_length = reader.bits(16)? as usize * 256 + reader.bits(8)? as usize;
        if self.block_length == 0 {
            return Err(error("Invalid CAB archive: zero-length LZX block"));
        }
        self.block_remaining = self.block_length;
        if self.block_type == 1 || self.block_type == 2 {
            if self.block_type == 2 {
                let mut lengths = [0u8; 8];
                for length in &mut lengths {
                    *length = reader.bits(3)? as u8;
                }
                self.aligned_table = Some(Huffman::new(&lengths, false)?);
            }
            code_lengths(reader, &mut self.main_lengths, 0, 256)?;
            let last = self.main_lengths.len();
            code_lengths(reader, &mut self.main_lengths, 256, last)?;
            self.main_table = Some(Huffman::new(&self.main_lengths, false)?);
            if self.main_lengths[0xe8] != 0 {
                self.intel_started = true;
            }
            let last = self.length_lengths.len();
            code_lengths(reader, &mut self.length_lengths, 0, last)?;
            self.length_table = Some(Huffman::new(&self.length_lengths, true)?);
            return Ok(());
        }
        if self.block_type == 3 {
            self.intel_started = true;
            reader.align_word();
            self.r0 = reader.u32_le()?;
            self.r1 = reader.u32_le()?;
            self.r2 = reader.u32_le()?;
            if self.r0 == 0 || self.r1 == 0 || self.r2 == 0 {
                return Err(error("Invalid CAB archive: invalid LZX repeated offset"));
            }
            self.uncompressed_padding = self.block_length & 1 != 0;
            return Ok(());
        }
        Err(error(format!("Invalid CAB archive: unsupported LZX block type {}", self.block_type)))
    }
    fn decode_run(
        &mut self,
        reader: &mut BitReader<'_>,
        output: &mut [u8],
        start: usize,
        count: usize,
    ) -> Result<usize> {
        if self.block_type == 3 {
            for index in 0..count {
                let byte = reader.byte()?;
                self.write_byte(byte, output, start + index);
            }
            return Ok(count);
        }
        if self.main_table.is_none() || self.length_table.is_none() {
            return Err(error("Invalid CAB archive: missing LZX decode trees"));
        }
        let mut produced = 0;
        while produced < count {
            let element = self.main_table.as_ref().unwrap().decode(reader)?;
            if element < 256 {
                self.write_byte(element as u8, output, start + produced);
                produced += 1;
                continue;
            }
            let value = element - 256;
            let mut length = value & 7;
            if length == 7 {
                length += self.length_table.as_ref().unwrap().decode(reader)?;
            }
            length += 2;
            if length > count - produced || length > self.block_remaining - produced {
                return Err(error("Invalid CAB archive: LZX match crosses a frame or block boundary"));
            }
            let slot = value >> 3;
            let offset = match slot {
                0 => self.r0,
                1 => {
                    let offset = self.r1;
                    self.r1 = self.r0;
                    self.r0 = offset;
                    offset
                }
                2 => {
                    let offset = self.r2;
                    self.r2 = self.r0;
                    self.r0 = offset;
                    offset
                }
                _ => {
                    // Fixed main alphabet is 256 + slots*8, so any decoded
                    // position slot is necessarily below position_base.len().
                    if slot >= self.position_base.len() {
                        return Err(error("Invalid CAB archive: LZX position slot is out of range"));
                    }
                    let extra = self.extra_bits[slot];
                    let mut offset = self.position_base[slot] - 2;
                    if self.block_type == 2 && extra >= 3 {
                        if extra > 3 {
                            offset += reader.bits(extra - 3)? * 8;
                        }
                        let table = self
                            .aligned_table
                            .as_ref()
                            .ok_or_else(|| error("Invalid CAB archive: missing LZX aligned tree"))?;
                        offset += table.decode(reader)? as u32;
                    } else if extra != 0 {
                        offset += reader.bits(extra)?;
                    }
                    self.r2 = self.r1;
                    self.r1 = self.r0;
                    self.r0 = offset;
                    offset
                }
            };
            if offset == 0 || offset as u64 > self.decoded_size.min(self.window.len() as u64) {
                return Err(error("Invalid CAB archive: LZX match offset exceeds available history"));
            }
            for index in 0..length {
                let source = (self.window_position + self.window.len() - offset as usize) % self.window.len();
                self.write_byte(self.window[source], output, start + produced + index);
            }
            produced += length;
        }
        Ok(produced)
    }
    fn write_byte(&mut self, byte: u8, output: &mut [u8], position: usize) {
        output[position] = byte;
        self.window[self.window_position] = byte;
        self.window_position = (self.window_position + 1) % self.window.len();
        self.decoded_size += 1;
    }
    fn translate_e8(&self, raw: &mut [u8]) {
        if !self.intel_started || self.intel_file_size == 0 || self.frame >= 32768 || raw.len() <= 10 {
            return;
        }
        let mut position = 0;
        let mut current = (self.decoded_size - raw.len() as u64) as i64;
        let end = raw.len() - 10;
        while position < end {
            let byte = raw[position];
            position += 1;
            if byte != 0xe8 {
                current += 1;
                continue;
            }
            let absolute = i32::from_le_bytes(raw[position..position + 4].try_into().unwrap()) as i64;
            if absolute >= -current && absolute < self.intel_file_size {
                let relative = if absolute >= 0 { absolute - current } else { absolute + self.intel_file_size };
                raw[position..position + 4].copy_from_slice(&(relative as i32).to_le_bytes());
            }
            position += 4;
            current += 5;
        }
    }
}
