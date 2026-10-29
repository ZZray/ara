//! Fixed OMP596f2da ar/codecs/lzw.ts ncompress decoding.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;

struct LsbCodeReader<'a> {
    bytes: &'a [u8],
    bit_position: usize,
    group_start: usize,
}
impl<'a> LsbCodeReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, bit_position: 0, group_start: 0 }
    }
    fn read(&mut self, width: usize) -> Option<usize> {
        if self.bytes.len() * 8 - self.bit_position < width {
            return None;
        }
        let mut value = 0;
        for bit in 0..width {
            let position = self.bit_position + bit;
            value |= usize::from((self.bytes[position >> 3] >> (position & 7)) & 1) << bit;
        }
        self.bit_position += width;
        Some(value)
    }
    fn align_code_group(&mut self, width: usize) -> Result<()> {
        let group_bits = width * 8;
        let aligned = self.group_start + (self.bit_position - self.group_start).div_ceil(group_bits) * group_bits;
        if aligned > self.bytes.len() * 8 {
            return Err(error("Truncated compress (.Z) code group"));
        }
        self.bit_position = aligned;
        self.group_start = aligned;
        Ok(())
    }
    fn assert_final_padding(&self) -> Result<()> {
        for position in self.bit_position..self.bytes.len() * 8 {
            if ((self.bytes[position >> 3] >> (position & 7)) & 1) != 0 {
                return Err(error("Invalid non-zero compress (.Z) padding"));
            }
        }
        Ok(())
    }
}

pub(super) fn is_compress_z(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1f, 0x9d])
}

pub(super) fn decompress(bytes: &[u8], max_output: u64) -> Result<Vec<u8>> {
    if max_output > JS_SAFE {
        return Err(error("Invalid compress (.Z) output limit"));
    }
    if bytes.len() < 3 {
        return Err(error("Truncated compress (.Z) header"));
    }
    if !is_compress_z(bytes) {
        return Err(error("Invalid compress (.Z) header"));
    }
    let flags = bytes[2];
    if flags & 0x60 != 0 {
        return Err(error("Unsupported compress (.Z) header flags"));
    }
    let max_bits = usize::from(flags & 0x1f);
    if !(9..=16).contains(&max_bits) {
        return Err(error(format!("Invalid compress (.Z) maximum code width {max_bits}")));
    }
    let block_mode = flags & 0x80 != 0;
    let dictionary_limit = 1usize << max_bits;
    let mut parents = vec![0u16; dictionary_limit];
    let mut suffixes = vec![0u8; dictionary_limit];
    let mut stack = vec![0u8; dictionary_limit];
    let mut reader = LsbCodeReader::new(&bytes[3..]);
    let capacity = max_output.min(64.max((bytes.len() as u64).saturating_mul(2).min(64 * 1024))) as usize;
    let mut output = Vec::new();
    output.try_reserve_exact(capacity).map_err(|err| error(err.to_string()))?;
    let mut width = 9;
    let mut dictionary_head = if block_mode { 257 } else { 256 };
    let mut needs_previous_suffix = false;
    loop {
        let Some(code) = reader.read(width) else {
            reader.assert_final_padding()?;
            return Ok(output);
        };
        if code >= dictionary_head {
            return Err(error(format!("Corrupt compress (.Z) dictionary code {code}")));
        }
        if block_mode && code == 256 {
            reader.align_code_group(width)?;
            width = 9;
            dictionary_head = 257;
            needs_previous_suffix = false;
            continue;
        }
        let mut current = code;
        let mut stack_length = 0;
        while current >= 256 {
            if current >= dictionary_head || stack_length >= stack.len() - 1 {
                return Err(error("Corrupt compress (.Z) dictionary chain"));
            }
            stack[stack_length] = suffixes[current];
            stack_length += 1;
            current = usize::from(parents[current]);
        }
        stack[stack_length] = current as u8;
        stack_length += 1;
        if needs_previous_suffix {
            suffixes[dictionary_head - 1] = current as u8;
            if code == dictionary_head - 1 {
                stack[0] = current as u8;
            }
        }
        let needed = output.len().checked_add(stack_length).filter(|&n| n as u64 <= max_output && n as u64 <= JS_SAFE);
        let Some(needed) = needed else {
            return Err(error(format!("Compress (.Z) output exceeds the {max_output}-byte limit")));
        };
        if needed > output.capacity() {
            let capacity = (needed as u64).max(max_output.min(64.max(output.capacity() as u64 * 2))) as usize;
            output.try_reserve_exact(capacity - output.len()).map_err(|err| error(err.to_string()))?;
        }
        output.extend(stack[..stack_length].iter().rev());
        if dictionary_head < dictionary_limit {
            needs_previous_suffix = true;
            parents[dictionary_head] = code as u16;
            dictionary_head += 1;
            if dictionary_head > 1usize << width && width < max_bits {
                reader.align_code_group(width)?;
                width += 1;
            }
        } else {
            needs_previous_suffix = false;
        }
    }
}
