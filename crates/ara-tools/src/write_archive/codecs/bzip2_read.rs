//! Fixed OMP596f2da ar/codecs/bzip2.ts concatenated streams and exact faults.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;

const BLOCK_MAGIC: [u8; 6] = [0x31, 0x41, 0x59, 0x26, 0x53, 0x59];
const STREAM_END_MAGIC: [u8; 6] = [0x17, 0x72, 0x45, 0x38, 0x50, 0x90];
const RANDOM_NUMBERS: [u16; 512] = [
    619, 720, 127, 481, 931, 816, 813, 233, 566, 247, 985, 724, 205, 454, 863, 491, 741, 242, 949, 214, 733, 859, 335,
    708, 621, 574, 73, 654, 730, 472, 419, 436, 278, 496, 867, 210, 399, 680, 480, 51, 878, 465, 811, 169, 869, 675,
    611, 697, 867, 561, 862, 687, 507, 283, 482, 129, 807, 591, 733, 623, 150, 238, 59, 379, 684, 877, 625, 169, 643,
    105, 170, 607, 520, 932, 727, 476, 693, 425, 174, 647, 73, 122, 335, 530, 442, 853, 695, 249, 445, 515, 909, 545,
    703, 919, 874, 474, 882, 500, 594, 612, 641, 801, 220, 162, 819, 984, 589, 513, 495, 799, 161, 604, 958, 533, 221,
    400, 386, 867, 600, 782, 382, 596, 414, 171, 516, 375, 682, 485, 911, 276, 98, 553, 163, 354, 666, 933, 424, 341,
    533, 870, 227, 730, 475, 186, 263, 647, 537, 686, 600, 224, 469, 68, 770, 919, 190, 373, 294, 822, 808, 206, 184,
    943, 795, 384, 383, 461, 404, 758, 839, 887, 715, 67, 618, 276, 204, 918, 873, 777, 604, 560, 951, 160, 578, 722,
    79, 804, 96, 409, 713, 940, 652, 934, 970, 447, 318, 353, 859, 672, 112, 785, 645, 863, 803, 350, 139, 93, 354, 99,
    820, 908, 609, 772, 154, 274, 580, 184, 79, 626, 630, 742, 653, 282, 762, 623, 680, 81, 927, 626, 789, 125, 411,
    521, 938, 300, 821, 78, 343, 175, 128, 250, 170, 774, 972, 275, 999, 639, 495, 78, 352, 126, 857, 956, 358, 619,
    580, 124, 737, 594, 701, 612, 669, 112, 134, 694, 363, 992, 809, 743, 168, 974, 944, 375, 748, 52, 600, 747, 642,
    182, 862, 81, 344, 805, 988, 739, 511, 655, 814, 334, 249, 515, 897, 955, 664, 981, 649, 113, 974, 459, 893, 228,
    433, 837, 553, 268, 926, 240, 102, 654, 459, 51, 686, 754, 806, 760, 493, 403, 415, 394, 687, 700, 946, 670, 656,
    610, 738, 392, 760, 799, 887, 653, 978, 321, 576, 617, 626, 502, 894, 679, 243, 440, 680, 879, 194, 572, 640, 724,
    926, 56, 204, 700, 707, 151, 457, 449, 797, 195, 791, 558, 945, 679, 297, 59, 87, 824, 713, 663, 412, 693, 342,
    606, 134, 108, 571, 364, 631, 212, 174, 643, 304, 329, 343, 97, 430, 751, 497, 314, 983, 374, 822, 928, 140, 206,
    73, 263, 980, 736, 876, 478, 430, 305, 170, 514, 364, 692, 829, 82, 855, 953, 676, 246, 369, 970, 294, 750, 807,
    827, 150, 790, 288, 923, 804, 378, 215, 828, 592, 281, 565, 555, 710, 82, 896, 831, 547, 261, 524, 462, 293, 465,
    502, 56, 661, 821, 976, 991, 658, 869, 905, 758, 745, 193, 768, 550, 608, 933, 378, 286, 215, 979, 792, 961, 61,
    688, 793, 644, 986, 403, 106, 366, 905, 644, 372, 567, 466, 434, 645, 210, 389, 550, 919, 135, 780, 773, 635, 389,
    707, 100, 626, 958, 165, 504, 920, 176, 193, 713, 857, 265, 203, 50, 668, 108, 645, 990, 626, 197, 510, 357, 358,
    850, 858, 364, 936, 638,
];

struct BitReader<'a> {
    bytes: &'a [u8],
    bit_position: usize,
}
impl BitReader<'_> {
    fn bit(&mut self) -> Result<u32> {
        if self.bit_position >= self.bytes.len() * 8 {
            return Err(error("Truncated bzip2 stream"));
        }
        let byte = self.bytes[self.bit_position >> 3];
        let bit = (byte >> (7 - (self.bit_position & 7))) & 1;
        self.bit_position += 1;
        Ok(u32::from(bit))
    }
    fn bits(&mut self, count: usize) -> Result<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            value = (value << 1) | self.bit()?;
        }
        Ok(value)
    }
    fn marker(&mut self) -> Result<[u8; 6]> {
        let mut marker = [0; 6];
        for byte in &mut marker {
            *byte = self.bits(8)? as u8;
        }
        Ok(marker)
    }
    fn align_to_byte(&mut self) -> Result<()> {
        while self.bit_position & 7 != 0 {
            if self.bit()? != 0 {
                return Err(error("Invalid non-zero bzip2 stream padding"));
            }
        }
        Ok(())
    }
}
struct BoundedOutput {
    bytes: Vec<u8>,
    limit: u64,
}
impl BoundedOutput {
    fn new(limit: u64, input_size: usize) -> Result<Self> {
        let capacity = limit.min(64.max((input_size as u64).saturating_mul(2).min(64 * 1024))) as usize;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(capacity).map_err(|err| error(err.to_string()))?;
        Ok(Self { bytes, limit })
    }
    fn repeat(&mut self, byte: u8, count: usize) -> Result<()> {
        let needed = self
            .bytes
            .len()
            .checked_add(count)
            .filter(|&n| n as u64 <= self.limit && n as u64 <= JS_SAFE)
            .ok_or_else(|| error(format!("Bzip2 output exceeds the {}-byte limit", self.limit)))?;
        if needed > self.bytes.capacity() {
            let capacity = (needed as u64).max(self.limit.min(64.max(self.bytes.capacity() as u64 * 2))) as usize;
            self.bytes.try_reserve_exact(capacity - self.bytes.len()).map_err(|err| error(err.to_string()))?;
        }
        self.bytes.resize(needed, byte);
        Ok(())
    }
}
struct HuffmanTable {
    counts: [usize; 21],
    first_codes: [u32; 21],
    offsets: [usize; 21],
    symbols: Vec<usize>,
}
impl HuffmanTable {
    fn new(lengths: &[usize]) -> Result<Self> {
        let mut table =
            Self { counts: [0; 21], first_codes: [0; 21], offsets: [0; 21], symbols: vec![0; lengths.len()] };
        for &length in lengths {
            if !(1..=20).contains(&length) {
                return Err(error("Invalid bzip2 Huffman code length"));
            }
            table.counts[length] += 1;
        }
        let mut code = 0;
        let mut offset = 0;
        for length in 1..=20 {
            code = (code + table.counts[length - 1] as u32) * 2;
            if code + table.counts[length] as u32 > 1 << length {
                return Err(error("Oversubscribed bzip2 Huffman table"));
            }
            table.first_codes[length] = code;
            table.offsets[length] = offset;
            offset += table.counts[length];
        }
        let mut next = table.offsets;
        for (symbol, &length) in lengths.iter().enumerate() {
            table.symbols[next[length]] = symbol;
            next[length] += 1;
        }
        Ok(table)
    }
    fn decode(&self, reader: &mut BitReader<'_>) -> Result<usize> {
        let mut code = 0;
        for length in 1..=20 {
            code = code * 2 + reader.bit()?;
            if let Some(relative) = code.checked_sub(self.first_codes[length]) {
                if relative < self.counts[length] as u32 {
                    return Ok(self.symbols[self.offsets[length] + relative as usize]);
                }
            }
        }
        Err(error("Invalid bzip2 Huffman code"))
    }
}
fn stream_header(reader: &mut BitReader<'_>) -> Result<usize> {
    if reader.bits(8)? != 0x42 || reader.bits(8)? != 0x5a || reader.bits(8)? != 0x68 {
        return Err(error("Invalid bzip2 stream header"));
    }
    let level = reader.bits(8)? as i32 - 0x30;
    if !(1..=9).contains(&level) {
        return Err(error("Invalid bzip2 block-size level"));
    }
    Ok(level as usize * 100_000)
}
fn huffman_tables(reader: &mut BitReader<'_>, used_bytes: &[u8]) -> Result<(Vec<usize>, Vec<HuffmanTable>)> {
    let groups = reader.bits(3)? as usize;
    if !(2..=6).contains(&groups) {
        return Err(error("Invalid bzip2 Huffman group count"));
    }
    let selector_count = reader.bits(15)? as usize;
    if !(1..=18_002).contains(&selector_count) {
        return Err(error("Invalid bzip2 selector count"));
    }
    let mut selectors = vec![0; selector_count];
    let mut mtf: Vec<_> = (0..groups).collect();
    for selector in &mut selectors {
        let mut position = 0;
        while reader.bit()? != 0 {
            position += 1;
            if position >= groups {
                return Err(error("Invalid bzip2 selector MTF value"));
            }
        }
        *selector = mtf[position];
        mtf.copy_within(..position, 1);
        mtf[0] = *selector;
    }
    let alpha_size = used_bytes.len() + 2;
    let mut tables = Vec::new();
    for _ in 0..groups {
        let mut lengths = vec![0; alpha_size];
        let mut length = reader.bits(5)? as i32;
        for value in &mut lengths {
            while reader.bit()? != 0 {
                length += if reader.bit()? == 0 { 1 } else { -1 };
                if !(1..=20).contains(&length) {
                    return Err(error("Invalid bzip2 Huffman code length"));
                }
            }
            *value = length as usize;
        }
        tables.push(HuffmanTable::new(&lengths)?);
    }
    Ok((selectors, tables))
}
fn block_data(
    reader: &mut BitReader<'_>,
    limit: usize,
    used_bytes: &[u8],
    selectors: &[usize],
    tables: &[HuffmanTable],
) -> Result<Vec<u8>> {
    let mut mtf = used_bytes.to_vec();
    let mut block = Vec::with_capacity(limit);
    let mut selector_index = 0;
    let mut group_remaining = 0;
    let mut table_index = 0;
    let mut next_symbol = || -> Result<usize> {
        if group_remaining == 0 {
            if selector_index >= selectors.len() {
                return Err(error("Bzip2 block exhausted its Huffman selectors"));
            }
            table_index = selectors[selector_index];
            selector_index += 1;
            group_remaining = 50;
        }
        group_remaining -= 1;
        tables[table_index].decode(reader)
    };
    let mut append = |byte, count| -> Result<()> {
        if count > limit - block.len() {
            return Err(error("Bzip2 block exceeds its declared block-size level"));
        }
        block.resize(block.len() + count, byte);
        Ok(())
    };
    let end_symbol = used_bytes.len() + 1;
    let mut symbol = next_symbol()?;
    while symbol != end_symbol {
        if symbol == 0 || symbol == 1 {
            let mut run_length = 0;
            let mut power = 1;
            loop {
                run_length += if symbol == 0 { power } else { power * 2 };
                if run_length > limit || power > limit {
                    return Err(error("Invalid bzip2 RLE run length"));
                }
                power *= 2;
                symbol = next_symbol()?;
                if symbol != 0 && symbol != 1 {
                    break;
                }
            }
            append(mtf[0], run_length)?;
            if symbol == end_symbol {
                break;
            }
        }
        let position =
            symbol.checked_sub(1).filter(|&i| i < mtf.len()).ok_or_else(|| error("Invalid bzip2 MTF symbol"))?;
        let byte = mtf[position];
        mtf.copy_within(..position, 1);
        mtf[0] = byte;
        append(byte, 1)?;
        symbol = next_symbol()?;
    }
    Ok(block)
}
fn inverse_bwt(block: &[u8], original_pointer: usize) -> Result<Vec<u8>> {
    if block.is_empty() || original_pointer >= block.len() {
        return Err(error("Invalid bzip2 BWT origin pointer"));
    }
    let mut counts = [0usize; 257];
    for &byte in block {
        counts[usize::from(byte) + 1] += 1;
    }
    for index in 1..counts.len() {
        counts[index] += counts[index - 1];
    }
    let mut positions = counts;
    let mut next = vec![0; block.len()];
    for (index, &byte) in block.iter().enumerate() {
        next[positions[usize::from(byte)]] = index;
        positions[usize::from(byte)] += 1;
    }
    let mut decoded = vec![0; block.len()];
    let mut position = next[original_pointer];
    for byte in &mut decoded {
        *byte = block[position];
        position = next[position];
    }
    Ok(decoded)
}
fn update_crc(crc: u32, byte: u8) -> u32 {
    static TABLE: std::sync::LazyLock<[u32; 256]> = std::sync::LazyLock::new(|| {
        let mut table = [0; 256];
        for (index, entry) in table.iter_mut().enumerate() {
            let mut value = (index as u32) << 24;
            for _ in 0..8 {
                value = if value & 0x8000_0000 != 0 { (value << 1) ^ 0x04c1_1db7 } else { value << 1 };
            }
            *entry = value;
        }
        table
    });
    TABLE[(((crc >> 24) ^ u32::from(byte)) & 0xff) as usize] ^ (crc << 8)
}
fn append_rle1(decoded: &[u8], randomized: bool, output: &mut BoundedOutput) -> Result<u32> {
    let mut crc = u32::MAX;
    let mut previous: Option<u8> = None;
    let mut repetitions = 0;
    let mut random_index = 0;
    let mut random_remaining = 0;
    for &raw in decoded {
        let mut byte = raw;
        if randomized {
            if random_remaining == 0 {
                random_remaining = RANDOM_NUMBERS[random_index];
                random_index = (random_index + 1) & 511;
            }
            random_remaining -= 1;
            if random_remaining == 1 {
                byte ^= 1;
            }
        }
        if repetitions == 4 {
            let previous = previous.unwrap();
            output.repeat(previous, usize::from(byte))?;
            for _ in 0..byte {
                crc = update_crc(crc, previous);
            }
            repetitions = 0;
            continue;
        }
        output.repeat(byte, 1)?;
        crc = update_crc(crc, byte);
        if Some(byte) == previous {
            repetitions += 1;
        } else {
            previous = Some(byte);
            repetitions = 1;
        }
    }
    if repetitions == 4 {
        return Err(error("Truncated bzip2 RLE run"));
    }
    Ok(crc ^ u32::MAX)
}
pub(super) fn decode(bytes: &[u8], max: u64) -> Result<Vec<u8>> {
    if max > JS_SAFE {
        return Err(error("Invalid bzip2 output limit"));
    }
    let mut reader = BitReader { bytes, bit_position: 0 };
    let mut output = BoundedOutput::new(max, bytes.len())?;
    loop {
        let block_limit = stream_header(&mut reader)?;
        let mut combined_crc = 0u32;
        loop {
            let marker = reader.marker()?;
            if marker == STREAM_END_MAGIC {
                if combined_crc != reader.bits(32)? {
                    return Err(error("Bzip2 stream CRC mismatch"));
                }
                reader.align_to_byte()?;
                break;
            }
            if marker != BLOCK_MAGIC {
                return Err(error("Invalid bzip2 block marker"));
            }
            let expected_crc = reader.bits(32)?;
            let randomized = reader.bit()? != 0;
            let origin = reader.bits(24)? as usize;
            let ranges = reader.bits(16)?;
            let mut used = Vec::new();
            for range in 0..16 {
                if ranges & (0x8000 >> range) == 0 {
                    continue;
                }
                for low in 0..16 {
                    if reader.bit()? != 0 {
                        used.push(range * 16 + low);
                    }
                }
            }
            if used.is_empty() {
                return Err(error("Bzip2 block has an empty symbol map"));
            }
            let (selectors, tables) = huffman_tables(&mut reader, &used)?;
            let block = block_data(&mut reader, block_limit, &used, &selectors, &tables)?;
            let decoded = inverse_bwt(&block, origin)?;
            let crc = append_rle1(&decoded, randomized, &mut output)?;
            if crc != expected_crc {
                return Err(error("Bzip2 block CRC mismatch"));
            }
            combined_crc = combined_crc.rotate_left(1) ^ crc;
        }
        if reader.bit_position == reader.bytes.len() * 8 {
            return Ok(output.bytes);
        }
    }
}
