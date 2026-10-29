//! Flat width scanner specialized from exact xutf 1.5.0 src/width.rs.
//! Copyright (c) 2026 Can Bölük. MIT; see LICENSE.xutf.
//! Retain ASCII bulk admission, keycap/VS16 hold-back, simple-run and rejected
//! scalar boundaries. Their widths differ from summing complete graphemes for
//! some legal UTF-8 inputs. Only decoding and printable-prefix acceleration
//! are specialized; the fixed scanner's order and width transitions remain.
use super::{
    props::{CB_LV, CB_LVT, CB_MASK, CB_OTHER, EPIC_BIT, WIDTH_EMOJI_TEXT, WIDTH_SHIFT, props},
    scalar::{ClusterState, width_value},
};

/// `true` for codepoints that provably form single-codepoint clusters next to
/// each other: break class Other/LV/LVT, not `Extended_Pictographic`, and no
/// pending VS16/keycap promotion. Two adjacent such codepoints always have a
/// boundary between them, so runs (CJK prose, precomposed Hangul) are summed
/// without join-state churn.
#[inline(always)]
const fn simple(p: u8) -> bool {
    matches!(p & CB_MASK, CB_OTHER | CB_LV | CB_LVT) && p & EPIC_BIT == 0 && (p >> WIDTH_SHIFT) & 3 != WIDTH_EMOJI_TEXT
}

#[inline(always)]
const fn promotable_ascii(cp: u32) -> bool {
    matches!(cp, 0x23 | 0x2a | 0x30..=0x39)
}

/// Visible width of `input` in terminal cells (extended grapheme clusters,
/// UAX #11 plus emoji presentation rules, with an ASCII bulk path).
fn width(input: &str) -> usize {
    measure_width::<false>(input, 0).expect("unbounded width scan cannot fail")
}

#[inline(always)]
fn commit_width<const BOUNDED: bool>(measured: &mut usize, width: usize, max_width: usize) -> bool {
    if BOUNDED && width > max_width - *measured {
        false
    } else {
        *measured += width;
        true
    }
}

/// Shared flat scanner for full and bounded measurement. `BOUNDED` lets the
/// optimizer erase all limit bookkeeping from [`width`].
fn measure_width<const BOUNDED: bool>(input: &str, max_width: usize) -> Option<usize> {
    let mut measured = 0usize;
    let mut rest = input;

    'bulk: loop {
        {
            // Fixed UTF-8 specialization: original E::FOREIGN is false.
            // A bounded scan inspects at most budget + 1 plain units, retaining
            // early exit on long ASCII. An unbounded scan takes the whole run.
            let window = if BOUNDED {
                let budget = max_width - measured;
                rest.len().min(budget.saturating_add(1))
            } else {
                rest.len()
            };
            let run = plain_prefix(&rest.as_bytes()[..window]);
            if run > 0 {
                // Only `#`, `*`, and digits need the last unit held for
                // possible VS16/keycap promotion. Other printable ASCII is
                // additive even when an extending codepoint follows.
                let promote = promotable_ascii(u32::from(rest.as_bytes()[run - 1]));
                let take = if run < rest.len() && promote { run - 1 } else { run };
                if !commit_width::<BOUNDED>(&mut measured, take, max_width) {
                    return None;
                }
                rest = &rest[take..];
            }
        }
        if rest.is_empty() {
            return Some(measured);
        }

        let mut cp = decode(&mut rest);
        let mut p = props(cp);
        'cluster: loop {
            let mut state = 'simple: {
                // Simple runs need one decode and one property load per scalar.
                // A successor confirms the boundary of the held codepoint.
                while simple(p) {
                    if rest.is_empty() {
                        return commit_width::<BOUNDED>(&mut measured, width_value(p), max_width).then_some(measured);
                    }
                    let mut peek = rest;
                    let next = decode(&mut peek);
                    if next.wrapping_sub(0x20) <= 0x5e {
                        if !commit_width::<BOUNDED>(&mut measured, width_value(p), max_width) {
                            return None;
                        }
                        continue 'bulk;
                    }
                    let np = props(next);
                    if !simple(np) {
                        let mut state = ClusterState::start(cp, p);
                        rest = peek;
                        if state.try_join(next, np) {
                            break 'simple state;
                        }
                        if !commit_width::<BOUNDED>(&mut measured, state.finish(), max_width) {
                            return None;
                        }
                        break 'simple ClusterState::start(next, np);
                    }
                    if !commit_width::<BOUNDED>(&mut measured, width_value(p), max_width) {
                        return None;
                    }
                    cp = next;
                    p = np;
                    rest = peek;
                }
                break 'simple ClusterState::start(cp, p);
            };

            // The rejected boundary scalar is retained as the next base, so
            // cluster transitions never decode or load its properties twice.
            loop {
                if rest.is_empty() {
                    return commit_width::<BOUNDED>(&mut measured, state.finish(), max_width).then_some(measured);
                }
                let mut peek = rest;
                let next = decode(&mut peek);
                if next.wrapping_sub(0x20) <= 0x5e && !state.joins_plain() {
                    if !commit_width::<BOUNDED>(&mut measured, state.finish(), max_width) {
                        return None;
                    }
                    continue 'bulk;
                }
                let np = props(next);
                if state.try_join(next, np) {
                    rest = peek;
                    continue;
                }
                if !commit_width::<BOUNDED>(&mut measured, state.finish(), max_width) {
                    return None;
                }
                rest = peek;
                if simple(np) {
                    cp = next;
                    p = np;
                    continue 'cluster;
                }
                state = ClusterState::start(next, np);
            }
        }
    }
}

/// [`width`] over UTF-8 `str`.
#[inline]
pub(super) fn width_str(input: &str) -> usize {
    width(input)
}

// Exact scalar predicate of xutf src/simd.rs:275-317. ASCII bytes are
// UTF-8 boundaries, including a prefix truncated by the bounded byte window.
#[inline]
fn plain_prefix(input: &[u8]) -> usize {
    input.iter().take_while(|&&byte| (0x20..=0x7e).contains(&byte)).count()
}

// Callers admit valid Rust UTF-8 str, so its next char is the same codepoint
// and consumed byte count as fixed xutf's UTF-8 decoder on this input domain.
#[inline]
fn decode(input: &mut &str) -> u32 {
    let rest = *input;
    let character = rest.chars().next().expect("width scanner admits a nonempty valid UTF-8 remainder");
    *input = &rest[character.len_utf8()..];
    character as u32
}
