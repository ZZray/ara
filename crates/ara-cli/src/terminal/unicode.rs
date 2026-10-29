//! Stable UTF-8 terminal primitives from fixed OMP's xutf 1.5.0 dependency.
//! Copyright (c) 2026 Can Bölük. MIT; see unicode/LICENSE.xutf.
//! The exact Unicode 17 property tables and scalar join/width state are retained.
//! Only the three operations used by this Host are exposed; the surrounding
//! text wrapper still owns controls, ANSI sequences, tabs and Hangul overrides.

mod props;
mod scalar;
mod width;

use props::props;
use scalar::{ClusterState, width_value};

/// Standalone cell width, with the original packed width-class interpretation.
#[inline]
pub fn width_char(c: char) -> usize {
    width_value(props(c as u32))
}

/// Cells measured by the original flat width scanner, including its ASCII
/// bulk boundaries. Grapheme widths are not additive for every legal string.
pub fn width_str(input: &str) -> usize {
    width::width_str(input)
}

/// Borrowed extended grapheme clusters at valid UTF-8 byte boundaries.
pub fn graphemes_str(input: &str) -> impl Iterator<Item = &str> {
    Clusters { rest: input }.map(|(cluster, _)| cluster)
}

struct Clusters<'a> {
    rest: &'a str,
}

impl<'a> Iterator for Clusters<'a> {
    type Item = (&'a str, usize);

    fn next(&mut self) -> Option<Self::Item> {
        let mut characters = self.rest.char_indices();
        let (_, first) = characters.next()?;
        let mut state = ClusterState::start(first as u32, props(first as u32));
        let mut end = self.rest.len();
        for (offset, character) in characters {
            if !state.try_join(character as u32, props(character as u32)) {
                end = offset;
                break;
            }
        }
        let (cluster, rest) = self.rest.split_at(end);
        self.rest = rest;
        Some((cluster, state.finish()))
    }
}
