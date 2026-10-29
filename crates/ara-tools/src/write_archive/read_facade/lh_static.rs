//! Fixed OMP596f2da ar/lzh.ts static Huffman LZSS decoder shared by LZH and ARJ.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;

pub(super) struct MsbBitReader<'a> {
    bytes: &'a [u8],
    label: &'a str,
    position: usize,
}
impl<'a> MsbBitReader<'a> {
    pub(super) fn new(bytes: &'a [u8], label: &'a str) -> Self {
        Self { bytes, label, position: 0 }
    }
    pub(super) fn read(&mut self, count: usize) -> Result<usize> {
        if count > 24 || self.position.checked_add(count).is_none_or(|end| end > self.bytes.len() * 8) {
            return Err(error(format!("Invalid {} compressed data: truncated bitstream", self.label)));
        }
        let mut value = 0;
        for _ in 0..count {
            let position = self.position;
            self.position += 1;
            value = value * 2 + ((self.bytes[position >> 3] >> (7 - (position & 7))) & 1) as usize;
        }
        Ok(value)
    }
    pub(super) fn zero_padding(&mut self) -> Result<()> {
        while self.position < self.bytes.len() * 8 {
            if self.read(1)? != 0 {
                return Err(error(format!("Invalid {} compressed data: non-zero trailing bits", self.label)));
            }
        }
        Ok(())
    }
}
struct CanonicalHuffman<'a> {
    zero: Vec<isize>,
    one: Vec<isize>,
    symbol: Vec<isize>,
    label: &'a str,
}
impl<'a> CanonicalHuffman<'a> {
    fn new(label: &'a str) -> Self {
        Self { zero: vec![-1], one: vec![-1], symbol: vec![-1], label }
    }
    fn single(symbol: usize, count: usize, label: &'a str) -> Result<Self> {
        if symbol >= count {
            return Err(error(format!("Invalid {label} Huffman table: symbol is out of range")));
        }
        let mut tree = Self::new(label);
        tree.symbol[0] = symbol as isize;
        Ok(tree)
    }
    fn build(lengths: &[u8], count: usize, label: &'a str) -> Result<Self> {
        let mut counts = [0usize; 17];
        let mut maximum = 0;
        for &length in &lengths[..count] {
            if length > 16 {
                return Err(error(format!("Invalid {label} Huffman table: code is too long")));
            }
            if length != 0 {
                counts[length as usize] += 1;
                maximum = maximum.max(length as usize);
            }
        }
        if maximum == 0 {
            return Err(error(format!("Invalid {label} Huffman table: no symbols")));
        }
        let mut next_codes = [0usize; 17];
        let mut code = 0;
        for length in 1..=16 {
            code = (code + counts[length - 1]) * 2;
            if code + counts[length] > 1 << length {
                return Err(error(format!("Invalid {label} Huffman table: oversubscribed codes")));
            }
            next_codes[length] = code;
        }
        if next_codes[maximum] + counts[maximum] != 1 << maximum {
            return Err(error(format!("Invalid {label} Huffman table: incomplete codes")));
        }
        let mut tree = Self::new(label);
        for (symbol, &length) in lengths[..count].iter().enumerate() {
            if length == 0 {
                continue;
            }
            let length = length as usize;
            let symbol_code = next_codes[length];
            next_codes[length] += 1;
            let mut node = 0usize;
            for bit_index in (0..length).rev() {
                if tree.symbol[node] >= 0 {
                    return Err(error(format!("Invalid {label} Huffman table: prefix collision")));
                }
                let bit = (symbol_code >> bit_index) & 1;
                let mut child = if bit == 0 { tree.zero[node] } else { tree.one[node] };
                if child < 0 {
                    child = tree.symbol.len() as isize;
                    tree.zero.push(-1);
                    tree.one.push(-1);
                    tree.symbol.push(-1);
                    if bit == 0 {
                        tree.zero[node] = child;
                    } else {
                        tree.one[node] = child;
                    }
                }
                node = child as usize;
            }
            if tree.symbol[node] >= 0 || tree.zero[node] >= 0 || tree.one[node] >= 0 {
                return Err(error(format!("Invalid {label} Huffman table: duplicate code")));
            }
            tree.symbol[node] = symbol as isize;
        }
        Ok(tree)
    }
    fn decode(&self, reader: &mut MsbBitReader<'_>) -> Result<usize> {
        let mut node = 0usize;
        for _ in 0..=16 {
            let symbol = self.symbol[node];
            if symbol >= 0 {
                return Ok(symbol as usize);
            }
            let next = if reader.read(1)? == 0 { self.zero[node] } else { self.one[node] };
            if next < 0 {
                return Err(error(format!("Invalid {} Huffman code", self.label)));
            }
            node = next as usize;
        }
        Err(error(format!("Invalid {} Huffman code: excessive depth", self.label)))
    }
}
fn code_length(reader: &mut MsbBitReader<'_>, label: &str) -> Result<u8> {
    let mut length = reader.read(3)?;
    if length == 7 {
        while reader.read(1)? != 0 {
            length += 1;
            if length > 16 {
                return Err(error(format!("Invalid {label} Huffman table: code is too long")));
            }
        }
    }
    Ok(length as u8)
}
fn temporary_tree<'a>(reader: &mut MsbBitReader<'_>, label: &'a str) -> Result<CanonicalHuffman<'a>> {
    let count = reader.read(5)?;
    if count == 0 {
        return CanonicalHuffman::single(reader.read(5)?, 19, label);
    }
    if count > 19 {
        return Err(error(format!("Invalid {label} temporary Huffman table size")));
    }
    let mut lengths = [0u8; 19];
    let mut index = 0;
    while index < count {
        lengths[index] = code_length(reader, label)?;
        index += 1;
        if index == 3 {
            let skipped = reader.read(2)?;
            if index + skipped > count {
                return Err(error(format!("Invalid {label} temporary Huffman table")));
            }
            index += skipped;
        }
    }
    CanonicalHuffman::build(&lengths, 19, label)
}
fn command_tree<'a>(
    reader: &mut MsbBitReader<'_>,
    temporary: &CanonicalHuffman<'_>,
    label: &'a str,
) -> Result<CanonicalHuffman<'a>> {
    let count = reader.read(9)?;
    if count == 0 {
        return CanonicalHuffman::single(reader.read(9)?, 510, label);
    }
    if count > 510 {
        return Err(error(format!("Invalid {label} command Huffman table size")));
    }
    let mut lengths = [0u8; 510];
    let mut index = 0;
    while index < count {
        let code = temporary.decode(reader)?;
        if code <= 2 {
            let skipped = match code {
                0 => 1,
                1 => reader.read(4)? + 3,
                _ => reader.read(9)? + 20,
            };
            if index + skipped > count {
                return Err(error(format!("Invalid {label} command Huffman table")));
            }
            index += skipped;
        } else {
            lengths[index] = (code - 2) as u8;
            index += 1;
        }
    }
    CanonicalHuffman::build(&lengths, 510, label)
}
fn position_tree<'a>(
    reader: &mut MsbBitReader<'_>,
    bits: usize,
    symbols: usize,
    label: &'a str,
) -> Result<CanonicalHuffman<'a>> {
    let count = reader.read(bits)?;
    if count == 0 {
        return CanonicalHuffman::single(reader.read(bits)?, symbols, label);
    }
    if count > symbols {
        return Err(error(format!("Invalid {label} position Huffman table size")));
    }
    let mut lengths = vec![0u8; symbols];
    for length in &mut lengths[..count] {
        *length = code_length(reader, label)?;
    }
    CanonicalHuffman::build(&lengths, symbols, label)
}
pub(super) fn decode(
    packed: &[u8],
    size: usize,
    dictionary: usize,
    position_bits: usize,
    position_symbols: usize,
    label: &str,
) -> Result<Vec<u8>> {
    let mut reader = MsbBitReader::new(packed, label);
    let mut output = vec![0u8; size];
    let mut position = 0;
    let mut block_remaining = 0;
    let mut commands = None;
    let mut positions = None;
    while position < size {
        if block_remaining == 0 {
            block_remaining = reader.read(16)?;
            if block_remaining == 0 {
                return Err(error(format!("Invalid {label} compressed data: empty block")));
            }
            let temporary = temporary_tree(&mut reader, label)?;
            commands = Some(command_tree(&mut reader, &temporary, label)?);
            positions = Some(position_tree(&mut reader, position_bits, position_symbols, label)?);
        }
        block_remaining -= 1;
        let symbol = commands.as_ref().unwrap().decode(&mut reader)?;
        if symbol < 256 {
            output[position] = symbol as u8;
            position += 1;
            continue;
        }
        let length = symbol - 256 + 3;
        if length > size - position {
            return Err(error(format!("Invalid {label} compressed data: match exceeds declared size")));
        }
        let code = positions.as_ref().unwrap().decode(&mut reader)?;
        let distance = if code > 1 { (1 << (code - 1)) + reader.read(code - 1)? } else { code };
        if distance >= dictionary || distance >= position {
            return Err(error(format!("Invalid {label} compressed data: history distance is out of range")));
        }
        let mut source = position - distance - 1;
        for _ in 0..length {
            output[position] = output[source];
            position += 1;
            source += 1;
        }
    }
    if block_remaining != 0 {
        return Err(error(format!("Invalid {label} compressed data: block exceeds declared size")));
    }
    reader.zero_padding()?;
    Ok(output)
}
