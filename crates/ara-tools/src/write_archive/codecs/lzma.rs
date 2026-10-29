//! Exact fixed OMP596f2da codecs/lzma.ts raw LZMA1 consumer for ZIP method14.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! History occupies the declared output; dictionarySize is a logical distance
//! ceiling, never rounded to a decoder allocator's alignment.
use super::*;
struct Range<'a> {
    bytes: &'a [u8],
    pos: usize,
    range: u32,
    code: u32,
}
impl<'a> Range<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() < 5 || bytes[0] != 0 {
            return Err(error("Invalid LZMA range-coded stream"));
        }
        let mut code = 0u32;
        for &byte in &bytes[..5] {
            code = (code << 8) | byte as u32;
        }
        Ok(Self { bytes, pos: 5, range: u32::MAX, code })
    }
    fn normalize(&mut self) -> Result<()> {
        if self.range < 1 << 24 {
            let byte = *self.bytes.get(self.pos).ok_or_else(|| error("Invalid LZMA stream: truncated range data"))?;
            self.pos += 1;
            self.range <<= 8;
            self.code = (self.code << 8) | byte as u32;
        }
        Ok(())
    }
    fn bit(&mut self, probs: &mut [u16], index: usize) -> Result<u32> {
        self.normalize()?;
        let p = probs[index] as u32;
        let bound = (self.range >> 11).wrapping_mul(p);
        if self.code < bound {
            self.range = bound;
            probs[index] = (p + ((2048 - p) >> 5)) as u16;
            Ok(0)
        } else {
            self.range = self.range.wrapping_sub(bound);
            self.code = self.code.wrapping_sub(bound);
            probs[index] = (p - (p >> 5)) as u16;
            Ok(1)
        }
    }
    fn direct(&mut self, count: u32) -> Result<u32> {
        let mut result = 0;
        for _ in 0..count {
            self.normalize()?;
            self.range >>= 1;
            let bit = if self.code >= self.range {
                self.code = self.code.wrapping_sub(self.range);
                1
            } else {
                0
            };
            result = (result << 1) | bit;
        }
        Ok(result)
    }
}
fn probs(size: usize) -> Vec<u16> {
    vec![1024; size]
}
fn tree(range: &mut Range<'_>, probs: &mut [u16], offset: usize, bits: u32) -> Result<u32> {
    let mut symbol = 1u32;
    for _ in 0..bits {
        symbol = (symbol << 1) | range.bit(probs, offset + symbol as usize)?;
    }
    Ok(symbol - (1 << bits))
}
fn reverse(range: &mut Range<'_>, probs: &mut [u16], offset: usize, bits: u32) -> Result<u32> {
    let mut symbol = 1u32;
    let mut result = 0;
    for i in 0..bits {
        let bit = range.bit(probs, offset + symbol as usize)?;
        symbol = (symbol << 1) | bit;
        result |= bit << i;
    }
    Ok(result)
}
struct Length {
    choice: Vec<u16>,
    low: Vec<u16>,
    mid: Vec<u16>,
    high: Vec<u16>,
}
impl Length {
    fn new() -> Self {
        Self { choice: probs(2), low: probs(16 << 3), mid: probs(16 << 3), high: probs(1 << 8) }
    }
    fn decode(&mut self, range: &mut Range<'_>, position: usize) -> Result<u32> {
        if range.bit(&mut self.choice, 0)? == 0 {
            return tree(range, &mut self.low, position << 3, 3);
        }
        if range.bit(&mut self.choice, 1)? == 0 {
            return Ok(8 + tree(range, &mut self.mid, position << 3, 3)?);
        }
        Ok(16 + tree(range, &mut self.high, 0, 8)?)
    }
}
fn distance(distance: u32, available: usize, dictionary: u32) -> Result<usize> {
    if distance < 1 || distance as u64 > available as u64 || distance > dictionary {
        return Err(error(format!("Invalid LZMA match distance {distance} with {available} bytes available")));
    }
    Ok(distance as usize)
}
fn copy(output: &mut [u8], pos: &mut usize, dist: u32, length: u32, dictionary: u32) -> Result<()> {
    let dist = distance(dist, *pos, dictionary)?;
    if length as usize > output.len() - *pos {
        return Err(error("LZMA match exceeds its declared output size"));
    }
    for _ in 0..length {
        output[*pos] = output[*pos - dist];
        *pos += 1;
    }
    Ok(())
}
pub(super) fn decode(properties: &[u8], bytes: &[u8], size: u64) -> Result<Vec<u8>> {
    if properties.len() != 5 {
        return Err(error("Invalid LZMA properties: expected 5 bytes"));
    }
    let packed = properties[0] as u32;
    if packed >= 225 {
        return Err(error("Invalid LZMA properties"));
    }
    let lc = packed % 9;
    let lp = (packed / 9) % 5;
    let pb = packed / 45;
    let dictionary = u32::from_le_bytes(properties[1..5].try_into().unwrap()).max(4096);
    let mut output = vec![0u8; size as usize];
    // Even empty declared output initializes the range coder, as fixed OMP does.
    let mut range = Range::new(bytes)?;
    let mut literal = probs(0x300 << (lc + lp));
    let mut is_match = probs(12 * 16);
    let mut is_rep = probs(12);
    let mut g0 = probs(12);
    let mut g1 = probs(12);
    let mut g2 = probs(12);
    let mut rep0long = probs(12 * 16);
    let mut slots = probs(4 << 6);
    let mut positions = probs(115);
    let mut align = probs(16);
    let mut len = Length::new();
    let mut rep_len = Length::new();
    let mut state = 0usize;
    let (mut rep0, mut rep1, mut rep2, mut rep3) = (1u32, 1u32, 1u32, 1u32);
    let mut pos = 0usize;
    while pos < output.len() {
        let pos_state = pos & ((1 << pb) - 1);
        if range.bit(&mut is_match, (state << 4) + pos_state)? == 0 {
            let previous = if pos == 0 { 0 } else { output[pos - 1] } as usize;
            let context = (((pos & ((1 << lp) - 1)) << lc) + (previous >> (8 - lc))) * 0x300;
            let mut symbol = 1u32;
            if state >= 7 {
                let dist = distance(rep0, pos, dictionary)?;
                let mut match_byte = output[pos - dist] as u32;
                while symbol < 0x100 {
                    let match_bit = (match_byte >> 7) & 1;
                    match_byte = (match_byte << 1) & 0xff;
                    let bit = range.bit(&mut literal, context + (((1 + match_bit) << 8) + symbol) as usize)?;
                    symbol = (symbol << 1) | bit;
                    if match_bit != bit {
                        break;
                    }
                }
            }
            while symbol < 0x100 {
                symbol = (symbol << 1) | range.bit(&mut literal, context + symbol as usize)?;
            }
            output[pos] = symbol as u8;
            pos += 1;
            state = if state < 4 {
                0
            } else if state < 10 {
                state - 3
            } else {
                state - 6
            };
            continue;
        }
        let length;
        if range.bit(&mut is_rep, state)? == 1 {
            if range.bit(&mut g0, state)? == 0 {
                if range.bit(&mut rep0long, (state << 4) + pos_state)? == 0 {
                    state = if state < 7 { 9 } else { 11 };
                    copy(&mut output, &mut pos, rep0, 1, dictionary)?;
                    continue;
                }
            } else {
                let dist;
                if range.bit(&mut g1, state)? == 0 {
                    dist = rep1;
                } else {
                    if range.bit(&mut g2, state)? == 0 {
                        dist = rep2;
                    } else {
                        dist = rep3;
                        rep3 = rep2;
                    }
                    rep2 = rep1;
                }
                rep1 = rep0;
                rep0 = dist;
            }
            length = rep_len.decode(&mut range, pos_state)? + 2;
            state = if state < 7 { 8 } else { 11 };
        } else {
            rep3 = rep2;
            rep2 = rep1;
            rep1 = rep0;
            length = len.decode(&mut range, pos_state)? + 2;
            state = if state < 7 { 7 } else { 10 };
            let slot = tree(&mut range, &mut slots, ((length - 2).min(3) << 6) as usize, 6)?;
            let mut dist;
            if slot < 4 {
                dist = slot;
            } else {
                let bits = (slot >> 1) - 1;
                dist = (2 | (slot & 1)) << bits;
                if slot < 14 {
                    dist = dist.wrapping_add(reverse(&mut range, &mut positions, (dist - slot) as usize, bits)?);
                } else {
                    dist = dist.wrapping_add(range.direct(bits - 4)? << 4);
                    dist = dist.wrapping_add(reverse(&mut range, &mut align, 0, 4)?);
                    if dist == u32::MAX {
                        return Err(error("LZMA stream ended before its declared output size"));
                    }
                }
            }
            rep0 = dist.wrapping_add(1);
        }
        copy(&mut output, &mut pos, rep0, length, dictionary)?;
    }
    Ok(output)
}

// Generalized fixed LzmaDecoder owner for READ alone/LZMA2 streams. Stable raw
// ZIP decode above remains byte-identical; arithmetic/probability helpers reuse
// the same fixed native definitions.
struct Decoder {
    output: Vec<u8>,
    dictionary: u32,
    output_pos: usize,
    dictionary_start: usize,
    processed: usize,
    lc: u32,
    lp: u32,
    pb: u32,
    state: usize,
    rep0: u32,
    rep1: u32,
    rep2: u32,
    rep3: u32,
    literal: Vec<u16>,
    is_match: Vec<u16>,
    is_rep: Vec<u16>,
    g0: Vec<u16>,
    g1: Vec<u16>,
    g2: Vec<u16>,
    rep0long: Vec<u16>,
    slots: Vec<u16>,
    positions: Vec<u16>,
    align: Vec<u16>,
    len: Length,
    rep_len: Length,
}
impl Decoder {
    fn new(output: Vec<u8>, dictionary: u32, lc: u32, lp: u32, pb: u32) -> Result<Self> {
        let mut decoder = Self {
            output, dictionary, output_pos: 0, dictionary_start: 0, processed: 0,
            lc: 0, lp: 0, pb: 0, state: 0, rep0: 1, rep1: 1, rep2: 1, rep3: 1,
            literal: probs(0x300), is_match: probs(12 * 16), is_rep: probs(12),
            g0: probs(12), g1: probs(12), g2: probs(12), rep0long: probs(12 * 16),
            slots: probs(4 << 6), positions: probs(115), align: probs(16),
            len: Length::new(), rep_len: Length::new(),
        };
        decoder.set_properties(lc, lp, pb)?;
        decoder.reset_dictionary();
        Ok(decoder)
    }
    fn set_properties(&mut self, lc: u32, lp: u32, pb: u32) -> Result<()> {
        if lc > 8 || lp > 4 || pb > 4 || lc + lp > 12 {
            return Err(error("Invalid LZMA properties"));
        }
        self.lc = lc;
        self.lp = lp;
        self.pb = pb;
        let count = 0x300usize << (lc + lp);
        if self.literal.len() != count {
            self.literal = probs(count);
        }
        Ok(())
    }
    fn reset_dictionary(&mut self) {
        self.dictionary_start = self.output_pos;
        self.processed = 0;
        self.reset_state();
    }
    fn reset_state(&mut self) {
        self.state = 0;
        self.rep0 = 1;
        self.rep1 = 1;
        self.rep2 = 1;
        self.rep3 = 1;
        for probabilities in [
            &mut self.literal, &mut self.is_match, &mut self.is_rep,
            &mut self.g0, &mut self.g1, &mut self.g2, &mut self.rep0long,
            &mut self.slots, &mut self.positions, &mut self.align,
            &mut self.len.choice, &mut self.len.low, &mut self.len.mid, &mut self.len.high,
            &mut self.rep_len.choice, &mut self.rep_len.low, &mut self.rep_len.mid, &mut self.rep_len.high,
        ] {
            probabilities.fill(1024);
        }
    }
    fn copy_match(&mut self, dist: u32, length: u32, limit: usize) -> Result<()> {
        let dist = distance(dist, self.output_pos - self.dictionary_start, self.dictionary)?;
        if length as usize > limit - self.output_pos {
            return Err(error("LZMA match exceeds its declared output size"));
        }
        for _ in 0..length {
            self.output[self.output_pos] = self.output[self.output_pos - dist];
            self.output_pos += 1;
        }
        self.processed += length as usize;
        Ok(())
    }
    fn append_uncompressed(&mut self, bytes: &[u8], reset_dictionary: bool) -> Result<()> {
        if reset_dictionary {
            self.reset_dictionary();
        }
        if bytes.len() > self.output.len() - self.output_pos {
            return Err(error("LZMA2 output exceeds its limit"));
        }
        self.output[self.output_pos..self.output_pos + bytes.len()].copy_from_slice(bytes);
        self.output_pos += bytes.len();
        self.processed += bytes.len();
        Ok(())
    }
    fn decode_chunk(&mut self, bytes: &[u8], output_size: u64, allow_end_marker: bool) -> Result<bool> {
        if output_size > JS_SAFE || output_size > (self.output.len() - self.output_pos) as u64 {
            return Err(error("LZMA output exceeds its declared size"));
        }
        let limit = self.output_pos + output_size as usize;
        let mut range = Range::new(bytes)?;
        let pos_mask = (1usize << self.pb) - 1;
        let literal_pos_mask = (1usize << self.lp) - 1;
        while self.output_pos < limit || allow_end_marker {
            let pos_state = self.processed & pos_mask;
            if range.bit(&mut self.is_match, (self.state << 4) + pos_state)? == 0 {
                let previous = if self.output_pos == self.dictionary_start { 0 } else { self.output[self.output_pos - 1] } as usize;
                let context = (((self.processed & literal_pos_mask) << self.lc) + (previous >> (8 - self.lc))) * 0x300;
                let mut symbol = 1u32;
                if self.state >= 7 {
                    let dist = distance(self.rep0, self.output_pos - self.dictionary_start, self.dictionary)?;
                    let mut match_byte = self.output[self.output_pos - dist] as u32;
                    while symbol < 0x100 {
                        let match_bit = (match_byte >> 7) & 1;
                        match_byte = (match_byte << 1) & 0xff;
                        let bit = range.bit(&mut self.literal, context + (((1 + match_bit) << 8) + symbol) as usize)?;
                        symbol = (symbol << 1) | bit;
                        if match_bit != bit { break; }
                    }
                }
                while symbol < 0x100 {
                    symbol = (symbol << 1) | range.bit(&mut self.literal, context + symbol as usize)?;
                }
                if self.output_pos >= limit {
                    return Err(error("LZMA output exceeds its size limit before the end marker"));
                }
                self.output[self.output_pos] = symbol as u8;
                self.output_pos += 1;
                self.processed += 1;
                self.state = if self.state < 4 { 0 } else if self.state < 10 { self.state - 3 } else { self.state - 6 };
                continue;
            }
            let length;
            if range.bit(&mut self.is_rep, self.state)? == 1 {
                if range.bit(&mut self.g0, self.state)? == 0 {
                    if range.bit(&mut self.rep0long, (self.state << 4) + pos_state)? == 0 {
                        self.state = if self.state < 7 { 9 } else { 11 };
                        self.copy_match(self.rep0, 1, limit)?;
                        continue;
                    }
                } else {
                    let dist;
                    if range.bit(&mut self.g1, self.state)? == 0 {
                        dist = self.rep1;
                    } else {
                        if range.bit(&mut self.g2, self.state)? == 0 { dist = self.rep2; }
                        else { dist = self.rep3; self.rep3 = self.rep2; }
                        self.rep2 = self.rep1;
                    }
                    self.rep1 = self.rep0;
                    self.rep0 = dist;
                }
                length = self.rep_len.decode(&mut range, pos_state)? + 2;
                self.state = if self.state < 7 { 8 } else { 11 };
            } else {
                self.rep3 = self.rep2;
                self.rep2 = self.rep1;
                self.rep1 = self.rep0;
                length = self.len.decode(&mut range, pos_state)? + 2;
                self.state = if self.state < 7 { 7 } else { 10 };
                let slot = tree(&mut range, &mut self.slots, ((length - 2).min(3) << 6) as usize, 6)?;
                let mut dist;
                if slot < 4 { dist = slot; }
                else {
                    let bits = (slot >> 1) - 1;
                    dist = (2 | (slot & 1)) << bits;
                    if slot < 14 {
                        dist = dist.wrapping_add(reverse(&mut range, &mut self.positions, (dist - slot) as usize, bits)?);
                    } else {
                        dist = dist.wrapping_add(range.direct(bits - 4)? << 4);
                        dist = dist.wrapping_add(reverse(&mut range, &mut self.align, 0, 4)?);
                        if dist == u32::MAX {
                            if allow_end_marker { return Ok(true); }
                            return Err(error("LZMA stream ended before its declared output size"));
                        }
                    }
                }
                self.rep0 = dist.wrapping_add(1);
            }
            self.copy_match(self.rep0, length, limit)?;
        }
        Ok(false)
    }
}
fn properties(props: &[u8]) -> Result<(u32, u32, u32, u32)> {
    if props.len() != 5 { return Err(error("Invalid LZMA properties: expected 5 bytes")); }
    let packed = props[0] as u32;
    if packed >= 225 { return Err(error("Invalid LZMA properties")); }
    let lc = packed % 9;
    let lp = (packed / 9) % 5;
    let pb = packed / 45;
    let dictionary = u32::from_le_bytes(props[1..5].try_into().unwrap()).max(4096);
    Ok((lc, lp, pb, dictionary))
}
pub(super) fn decode_alone(bytes: &[u8], max: u64) -> Result<Vec<u8>> {
    if bytes.len() < 13 { return Err(error("Invalid LZMA-alone stream: truncated header")); }
    if max > JS_SAFE { return Err(error("Invalid LZMA output limit")); }
    let unknown = bytes[5..13].iter().all(|&byte| byte == 0xff);
    let declared = u64::from_le_bytes(bytes[5..13].try_into().unwrap());
    if !unknown {
        if declared > JS_SAFE || declared > max { return Err(error("LZMA-alone output exceeds its size limit")); }
        return decode(&bytes[..5], &bytes[13..], declared);
    }
    let (lc, lp, pb, dictionary) = properties(&bytes[..5])?;
    let mut decoder = Decoder::new(vec![0u8; max as usize], dictionary, lc, lp, pb)?;
    if !decoder.decode_chunk(&bytes[13..], max, true)? {
        return Err(error("LZMA-alone output exceeds its size limit before the end marker"));
    }
    decoder.output.truncate(decoder.output_pos);
    Ok(decoder.output)
}
pub(super) fn decode_lzma2(property: u8, bytes: &[u8], max: u64) -> Result<Vec<u8>> {
    if property > 40 { return Err(error("Unsupported LZMA2 dictionary property")); }
    if max > JS_SAFE { return Err(error("Invalid LZMA2 output limit")); }
    let dictionary = if property == 40 { u32::MAX } else { (2 | u32::from(property & 1)) << (u32::from(property / 2) + 11) };
    let mut decoder = Decoder::new(vec![0u8; max as usize], dictionary, 3, 0, 2)?;
    let mut pos = 0;
    let mut need_dictionary_reset = true;
    let mut properties_set = false;
    let take_byte = |pos: &mut usize| -> Result<u32> {
        let byte = *bytes.get(*pos).ok_or_else(|| error("Invalid LZMA2 stream: truncated chunk header"))?;
        *pos += 1;
        Ok(u32::from(byte))
    };
    loop {
        let control = take_byte(&mut pos)?;
        if control == 0 { break; }
        if control < 0x80 {
            if control != 1 && control != 2 { return Err(error(format!("Invalid LZMA2 control byte 0x{control:x}"))); }
            if control == 2 && need_dictionary_reset { return Err(error("Invalid LZMA2 stream: dictionary was not initialized")); }
            let unpack_size = ((take_byte(&mut pos)? << 8) + take_byte(&mut pos)? + 1) as usize;
            if unpack_size > decoder.output.len() - decoder.output_pos || unpack_size > bytes.len() - pos {
                return Err(error("Invalid LZMA2 uncompressed chunk size"));
            }
            decoder.append_uncompressed(&bytes[pos..pos + unpack_size], control == 1)?;
            pos += unpack_size;
            if control == 1 { need_dictionary_reset = false; properties_set = false; }
            continue;
        }
        let unpack_size = u64::from(((control & 0x1f) << 16) + (take_byte(&mut pos)? << 8) + take_byte(&mut pos)? + 1);
        let pack_size = ((take_byte(&mut pos)? << 8) + take_byte(&mut pos)? + 1) as usize;
        let resets_dictionary = control >= 0xe0;
        let resets_state = control >= 0xa0;
        let sets_properties = control >= 0xc0;
        if need_dictionary_reset && !resets_dictionary { return Err(error("Invalid LZMA2 stream: dictionary was not initialized")); }
        if !properties_set && !sets_properties { return Err(error("Invalid LZMA2 stream: properties were not initialized")); }
        if sets_properties {
            let property = take_byte(&mut pos)?;
            if property >= 225 { return Err(error("Invalid LZMA2 properties")); }
            let lc = property % 9;
            let lp = (property / 9) % 5;
            let pb = property / 45;
            if lc + lp > 4 { return Err(error("Invalid LZMA2 literal properties")); }
            decoder.set_properties(lc, lp, pb)?;
            properties_set = true;
        }
        if pack_size > bytes.len() - pos || unpack_size > (decoder.output.len() - decoder.output_pos) as u64 {
            return Err(error("Invalid LZMA2 chunk size"));
        }
        if resets_dictionary { decoder.reset_dictionary(); }
        else if resets_state { decoder.reset_state(); }
        decoder.decode_chunk(&bytes[pos..pos + pack_size], unpack_size, false)?;
        pos += pack_size;
        need_dictionary_reset = false;
    }
    if pos != bytes.len() { return Err(error("Invalid LZMA2 stream: trailing data")); }
    decoder.output.truncate(decoder.output_pos);
    Ok(decoder.output)
}
