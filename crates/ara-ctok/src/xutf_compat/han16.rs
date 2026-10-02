//! Script=Han ranges for Unicode 16.0.0, matching pinned xutf's default UCD.
//!
//! Copied from regex-syntax 0.8.11 `src/unicode_tables/script.rs::HAN`.
//! That table was generated with `ucd-generate script ucd-16.0.0 --chars`.
//! Its source SHA256 is
//! 41bd424f1e3a03290cf4995ced678dcf24c94b38c905c62f6819bf67e098a2ec.
//! Notices: `data/LICENSE-UNICODE` and `data/LICENSE.regex-syntax`.

const HAN: &[(char, char)] = &[
    ('\u{2e80}', '\u{2e99}'),
    ('\u{2e9b}', '\u{2ef3}'),
    ('\u{2f00}', '\u{2fd5}'),
    ('\u{3005}', '\u{3005}'),
    ('\u{3007}', '\u{3007}'),
    ('\u{3021}', '\u{3029}'),
    ('\u{3038}', '\u{303b}'),
    ('\u{3400}', '\u{4dbf}'),
    ('\u{4e00}', '\u{9fff}'),
    ('\u{f900}', '\u{fa6d}'),
    ('\u{fa70}', '\u{fad9}'),
    ('\u{16fe2}', '\u{16fe3}'),
    ('\u{16ff0}', '\u{16ff1}'),
    ('\u{20000}', '\u{2a6df}'),
    ('\u{2a700}', '\u{2b739}'),
    ('\u{2b740}', '\u{2b81d}'),
    ('\u{2b820}', '\u{2cea1}'),
    ('\u{2ceb0}', '\u{2ebe0}'),
    ('\u{2ebf0}', '\u{2ee5d}'),
    ('\u{2f800}', '\u{2fa1d}'),
    ('\u{30000}', '\u{3134a}'),
    ('\u{31350}', '\u{323af}'),
];

#[inline]
pub fn is_han(c: char) -> bool {
    let index = HAN.partition_point(|&(_, end)| end < c);
    HAN.get(index).is_some_and(|&(start, _)| start <= c)
}
