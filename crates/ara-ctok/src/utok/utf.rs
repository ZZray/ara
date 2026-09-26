//! Encoding-generic text input copied from the pinned OMP tokenizer.
//!
//! No transcoding, no scratch buffers. The pipeline runs natively in the
//! input's own code units. A JS UTF-16 string can be counted directly.
//!
//! Decoding is permissive (xutf semantics): malformed sequences and lone
//! surrogates decode as U+FFFD and consume minimally. Valid text behaves
//! identically across flavors, so counts are flavor-invariant.

use std::hash::Hash;

/// One code unit: `u8` (UTF-8), `u16` (UTF-16 native-endian), `u32` (UTF-32).
pub trait Unit: Copy + Eq + Ord + Hash + 'static {
    /// Decode the codepoint starting at `units[i]`.
    /// Returns `(codepoint, units_consumed)`; permissive on malformed input.
    fn decode(units: &[Self], i: usize) -> (char, usize);

    /// Encode `cp` into `out`, returning the unit count written.
    /// `out` must have room for 4 units.
    fn encode(cp: char, out: &mut [Self]) -> usize;

    /// Identity byte view when this flavor already is UTF-8 (`u8` only).
    /// Lets the engine skip per-piece re-encoding for `str` input.
    fn as_utf8(units: &[Self]) -> Option<&[u8]>;
}

impl Unit for u8 {
    #[inline]
    fn decode(units: &[Self], i: usize) -> (char, usize) {
        let b = units[i];
        if b < 0x80 {
            return (b as char, 1);
        }
        // Permissive multi-byte decode: on malformed input yield U+FFFD and
        // consume one unit.
        let need = match b {
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            _ => return (char::REPLACEMENT_CHARACTER, 1),
        };
        if i + need > units.len() {
            return (char::REPLACEMENT_CHARACTER, 1);
        }
        let mut cp = (b as u32) & (0x7f >> need);
        for k in 1..need {
            let c = units[i + k];
            if c & 0xc0 != 0x80 {
                return (char::REPLACEMENT_CHARACTER, 1);
            }
            cp = cp << 6 | (c & 0x3f) as u32;
        }
        match char::from_u32(cp) {
            Some(c) => (c, need),
            None => (char::REPLACEMENT_CHARACTER, need),
        }
    }

    #[inline]
    fn encode(cp: char, out: &mut [Self]) -> usize {
        cp.encode_utf8(out).len()
    }

    #[inline]
    fn as_utf8(units: &[Self]) -> Option<&[u8]> {
        Some(units)
    }
}

impl Unit for u16 {
    #[inline]
    fn decode(units: &[Self], i: usize) -> (char, usize) {
        let u = units[i];
        if !(0xd800..=0xdfff).contains(&u) {
            // SAFETY-free: non-surrogate u16 is always a valid scalar.
            return (char::from_u32(u as u32).unwrap_or(char::REPLACEMENT_CHARACTER), 1);
        }
        if u < 0xdc00
            && let Some(&lo) = units.get(i + 1)
            && (0xdc00..=0xdfff).contains(&lo)
        {
            let cp = 0x10000 + (((u as u32 - 0xd800) << 10) | (lo as u32 - 0xdc00));
            return (char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER), 2);
        }
        (char::REPLACEMENT_CHARACTER, 1) // lone surrogate
    }

    #[inline]
    fn encode(cp: char, out: &mut [Self]) -> usize {
        cp.encode_utf16(out).len()
    }

    #[inline]
    fn as_utf8(_units: &[Self]) -> Option<&[u8]> {
        None
    }
}

impl Unit for u32 {
    #[inline]
    fn decode(units: &[Self], i: usize) -> (char, usize) {
        (char::from_u32(units[i]).unwrap_or(char::REPLACEMENT_CHARACTER), 1)
    }

    #[inline]
    fn encode(cp: char, out: &mut [Self]) -> usize {
        out[0] = cp as Self;
        1
    }

    #[inline]
    fn as_utf8(_units: &[Self]) -> Option<&[u8]> {
        None
    }
}
