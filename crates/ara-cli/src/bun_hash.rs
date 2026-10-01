//! Fixed Bun 1.4.0 hash/string behavior for OMP catalog fingerprints.
//! Oracle revision: 34cbb9a40b4bd1bd767d134a7065e66c2432a676.
//! Wyhash final4 algorithm: https://github.com/wangyi-fudan/wyhash (public domain).
//! Bun source evidence: src/wyhash/lib.rs at the above exact revision (MIT).

use ara_rpc::WireString;

const SECRET: [u64; 4] = [0xa0761d6478bd642f, 0xe7037ed1a0b428db, 0x8ebc6af09c88c6e3, 0x589965cc75374cc3];
fn mix(a: u64, b: u64) -> u64 {
    let value = u128::from(a) * u128::from(b);
    value as u64 ^ (value >> 64) as u64
}
fn read4(input: &[u8], at: usize) -> u64 {
    u64::from(u32::from_le_bytes(input[at..at + 4].try_into().expect("bounded hash read")))
}
fn read8(input: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(input[at..at + 8].try_into().expect("bounded hash read"))
}

pub fn hash_bytes(input: &[u8]) -> u64 {
    let seed = mix(SECRET[0], SECRET[1]);
    let mut state = [seed; 3];
    let len = input.len();
    let (a, b);
    if len <= 16 {
        if len >= 4 {
            let end = len - 4;
            let quarter = (len >> 3) << 2;
            a = (read4(input, 0) << 32) | read4(input, quarter);
            b = (read4(input, end) << 32) | read4(input, end - quarter);
        } else if len > 0 {
            a = (u64::from(input[0]) << 16) | (u64::from(input[len >> 1]) << 8) | u64::from(input[len - 1]);
            b = 0;
        } else {
            a = 0;
            b = 0;
        }
    } else {
        let mut at = 0;
        if len >= 48 {
            while at + 48 < len {
                for lane in 0..3 {
                    state[lane] = mix(
                        read8(input, at + lane * 16) ^ SECRET[lane + 1],
                        read8(input, at + lane * 16 + 8) ^ state[lane],
                    );
                }
                at += 48;
            }
            state[0] ^= state[1] ^ state[2];
        }
        while at + 16 < len {
            state[0] = mix(read8(input, at) ^ SECRET[1], read8(input, at + 8) ^ state[0]);
            at += 16;
        }
        a = read8(input, len - 16);
        b = read8(input, len - 8);
    }
    let product = u128::from(a ^ SECRET[1]) * u128::from(b ^ state[0]);
    mix(product as u64 ^ SECRET[0] ^ len as u64, (product >> 64) as u64 ^ SECRET[1])
}
/// Bun.hash strings are USVString encoded as UTF-8, including lone-surrogate replacement.
pub fn hash_string(value: &WireString) -> u64 {
    hash_bytes(String::from_utf16_lossy(value.units()).as_bytes())
}
pub fn base36(mut value: u64) -> String {
    if value == 0 {
        return "0".to_owned();
    }
    let mut digits = Vec::new();
    while value > 0 {
        digits.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(value % 36) as usize]);
        value /= 36;
    }
    digits.reverse();
    String::from_utf8(digits).expect("ASCII hash digits")
}
pub fn hash_string_base36(value: &WireString) -> String {
    base36(hash_string(value))
}

/// bun:sqlite's native TEXT binder at the fixed oracle revision consumes a
/// surrogate and the following unit as a pair, even for malformed UTF-16.
/// A final isolated surrogate is emitted as WTF-8. Keep this distinct from
/// Bun.hash/TextEncoder's USVString conversion. Bind these bytes as SQLite TEXT.
pub fn sqlite_text_bytes(value: &WireString) -> Vec<u8> {
    let mut out = Vec::new();
    let mut units = value.units().iter().copied();
    while let Some(unit) = units.next() {
        let point = if (0xd800..=0xdfff).contains(&unit) {
            units
                .next()
                .map_or(u32::from(unit), |next| 0x10000 + ((u32::from(unit) & 0x3ff) << 10) + (u32::from(next) & 0x3ff))
        } else {
            u32::from(unit)
        };
        if point < 0x80 {
            out.push(point as u8);
        } else if point < 0x800 {
            out.extend_from_slice(&[(0xc0 | (point >> 6)) as u8, (0x80 | (point & 0x3f)) as u8]);
        } else if point < 0x10000 {
            out.extend_from_slice(&[
                (0xe0 | (point >> 12)) as u8,
                (0x80 | ((point >> 6) & 0x3f)) as u8,
                (0x80 | (point & 0x3f)) as u8,
            ]);
        } else {
            out.extend_from_slice(&[
                (0xf0 | (point >> 18)) as u8,
                (0x80 | ((point >> 12) & 0x3f)) as u8,
                (0x80 | ((point >> 6) & 0x3f)) as u8,
                (0x80 | (point & 0x3f)) as u8,
            ]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_bun_string_oracle() {
        for (units, expected, encoded) in [
            (vec![], 290873116282709081, "27k1wwwhf13t"),
            (vec![0x61], 2941419223392617777, "mcie3cqm6cz5"),
            ("hello".encode_utf16().collect(), 1019145960556548909, "7qqx3auqfzfx"),
            (vec![0xe9], 1783465187472633034, "djspn8dun5ca"),
            (vec![0x100], 14893838494796840138, "355mp43r3s98q"),
            (vec![0xd83d, 0xde00], 15496182819936353887, "39qdmcuni8va7"),
            (vec![0xd800], 6000221778706169387, "19l4l3rpu9dez"),
            (vec![0x78, 0xd800, 0x79], 2474134706523809736, "ispbk17j6jm0"),
        ] {
            let value = WireString::from_units(units);
            assert_eq!(hash_string(&value), expected);
            assert_eq!(hash_string_base36(&value), encoded);
        }
    }
    #[test]
    fn fixed_bun_sqlite_binding_oracle() {
        for (units, bytes) in [
            (vec![0xd800], vec![0xed, 0xa0, 0x80]),
            (vec![0x78, 0xd800, 0x79], vec![0x78, 0xf0, 0x90, 0x81, 0xb9]),
            (vec![0xd800, 0xd800], vec![0xf0, 0x90, 0x80, 0x80]),
            (vec![0xdc00, 0x61], vec![0xf0, 0x90, 0x81, 0xa1]),
        ] {
            assert_eq!(sqlite_text_bytes(&WireString::from_units(units)), bytes);
        }
    }
    #[test]
    fn fixed_bun_round_boundaries_oracle() {
        for (len, expected) in [
            (4, 10204389674083730068),
            (7, 10617948783525562313),
            (8, 10832427242299425384),
            (15, 12758313179215513646),
            (16, 3942126890664611248),
            (17, 10396640819404160124),
            (47, 6756994484968869833),
            (48, 4522089241930378199),
            (49, 3464988544681768254),
            (95, 9526116502287605786),
            (96, 12320342245329993612),
            (97, 10986065543042628950),
            (200, 9583113423185900620),
            (4096, 1000582568507522151),
        ] {
            let bytes: Vec<u8> = (0..len).map(|i| ((i * 31 + 7) % 256) as u8).collect();
            assert_eq!(hash_bytes(&bytes), expected, "length {len}");
        }
    }
}
