//! Rust translation of Bun's active ISO/legacy date tokenizer and composers.
//! Source: oven-sh/WebKit 0f966e81b78c84bb23213e391bc679c4ef83e56b,
//! Source/JavaScriptCore/runtime/JSDateMath-v8.cpp and JSDateMath.cpp.
//! Returns a wall instant and optional explicit offset; the Host owns local
//! timezone conversion and final TimeClip. No evaluator or subprocess is used.

// Copyright 2013 the V8 project authors. All rights reserved.
// Copyright (C) 2005, 2006, 2007, 2008, 2009 Apple Inc. All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
// 1.  Redistributions of source code must retain the above copyright
//     notice, this list of conditions and the following disclaimer.
// 2.  Redistributions in binary form must reproduce the above copyright
//     notice, this list of conditions and the following disclaimer in the
//     documentation and/or other materials provided with the distribution.
//
// THIS SOFTWARE IS PROVIDED BY APPLE INC. AND ITS CONTRIBUTORS ``AS IS'' AND ANY
// EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
// WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL APPLE INC. OR ITS CONTRIBUTORS BE LIABLE FOR ANY
// DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
// (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
// LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
// ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
// (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
// SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ParsedDate {
    pub wall_ms: i64,
    pub offset_seconds: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Invalid,
    Unknown,
    Space,
    Number,
    Symbol,
    End,
    Word,
    Month,
    Zone,
    Separator,
    AmPm,
}
#[derive(Clone, Copy, Debug)]
struct Token {
    kind: Kind,
    value: i32,
    len: usize,
}
impl Token {
    fn new(kind: Kind, value: i32, len: usize) -> Self {
        Self { kind, value, len }
    }
    fn symbol(self, b: u8) -> bool {
        self.kind == Kind::Symbol && self.value == i32::from(b)
    }
    fn sign(self) -> bool {
        self.symbol(b'+') || self.symbol(b'-')
    }
    fn fixed(self, len: usize) -> bool {
        self.kind == Kind::Number && self.len == len
    }
    fn z(self) -> bool {
        self.kind == Kind::Zone && self.len == 1 && self.value == 0
    }
    fn keyword(self) -> bool {
        matches!(
            self.kind,
            Kind::Word | Kind::Month | Kind::Zone | Kind::Separator | Kind::AmPm
        )
    }
    fn milliseconds(self) -> i32 {
        match self.len {
            0..=2 => self.value * 10_i32.pow((3 - self.len) as u32),
            3 => self.value,
            n => self.value / 10_i32.pow((n.min(9) - 3) as u32),
        }
    }
}

struct Scanner<'a> {
    bytes: &'a [u8],
    at: usize,
    peek: Token,
}
impl<'a> Scanner<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        let mut s = Self {
            bytes,
            at: 0,
            peek: Token::new(Kind::End, 0, 0),
        };
        s.peek = s.scan();
        s
    }
    fn ch(&self) -> u8 {
        self.bytes.get(self.at).copied().unwrap_or(0)
    }
    fn next(&mut self) -> Token {
        let t = self.peek;
        self.peek = self.scan();
        t
    }
    fn skip(&mut self, b: u8) -> bool {
        if self.peek.symbol(b) {
            self.next();
            true
        } else {
            false
        }
    }
    fn scan(&mut self) -> Token {
        let start = self.at;
        let ch = self.ch();
        if ch == 0 {
            return Token::new(Kind::End, 0, 0);
        }
        if ch.is_ascii_digit() {
            while self.ch() == b'0' {
                self.at += 1;
            }
            let mut value = 0;
            let mut significant = 0;
            while self.ch().is_ascii_digit() {
                if significant < 9 {
                    value = value * 10 + i32::from(self.ch() - b'0');
                }
                significant += 1;
                self.at += 1;
            }
            return Token::new(Kind::Number, value, self.at - start);
        }
        if matches!(ch, b':' | b'-' | b'+' | b'.' | b')') {
            self.at += 1;
            return Token::new(Kind::Symbol, i32::from(ch), 1);
        }
        if ch >= b'A' && !ascii_space(ch) {
            let mut prefix = [0; 3];
            while self.ch() >= b'A' && !ascii_space(self.ch()) {
                let len = self.at - start;
                if len < 3 {
                    prefix[len] = self.ch() | 0x20;
                }
                self.at += 1;
            }
            let len = self.at - start;
            let (kind, value) = keyword(prefix, len);
            return Token::new(kind, value, len);
        }
        if ascii_space(ch) {
            self.at += 1;
            return Token::new(Kind::Space, 0, 1);
        }
        if ch == b'(' {
            let mut balance = 0;
            loop {
                match self.ch() {
                    b'(' => balance += 1,
                    b')' => balance -= 1,
                    _ => {}
                }
                self.at += 1;
                if balance == 0 || self.ch() == 0 {
                    break;
                }
            }
        } else {
            self.at += 1;
        }
        Token::new(Kind::Unknown, 0, 1)
    }
}
fn ascii_space(ch: u8) -> bool {
    matches!(ch, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}
fn keyword(pre: [u8; 3], len: usize) -> (Kind, i32) {
    let months = [
        *b"jan", *b"feb", *b"mar", *b"apr", *b"may", *b"jun", *b"jul", *b"aug", *b"sep", *b"oct",
        *b"nov", *b"dec",
    ];
    if let Some(i) = months.iter().position(|m| *m == pre) {
        return (Kind::Month, i as i32 + 1);
    }
    if len > 3 {
        return (Kind::Word, 0);
    }
    match &pre {
        b"am\0" => (Kind::AmPm, 0),
        b"pm\0" => (Kind::AmPm, 12),
        b"ut\0" | b"utc" | b"z\0\0" | b"gmt" => (Kind::Zone, 0),
        b"cdt" | b"est" => (Kind::Zone, -5),
        b"cst" | b"mdt" => (Kind::Zone, -6),
        b"edt" => (Kind::Zone, -4),
        b"mst" | b"pdt" => (Kind::Zone, -7),
        b"pst" => (Kind::Zone, -8),
        b"t\0\0" => (Kind::Separator, 0),
        _ => (Kind::Word, 0),
    }
}
#[derive(Default)]
struct Day {
    fields: [i32; 3],
    len: usize,
    named: Option<i32>,
    iso: bool,
}
impl Day {
    fn add(&mut self, n: i32) -> bool {
        if self.len == 3 {
            return false;
        }
        self.fields[self.len] = n;
        self.len += 1;
        true
    }
    fn write(mut self) -> Option<(i32, u8, u8)> {
        if self.len == 0 {
            return None;
        }
        while self.len < 3 {
            self.add(1);
        }
        // Preserve the actual bundled composer, including its padded index.
        let [a, b, c] = self.fields;
        let (mut y, m, d) = match self.named {
            None if self.iso || !(1..=31).contains(&a) => (a, b, c),
            None => (c, a, b),
            Some(m) if !(1..=31).contains(&a) => (a, m, b),
            Some(m) => (b, m, a),
        };
        if !self.iso {
            if (0..=49).contains(&y) {
                y += 2000;
            } else if (50..=99).contains(&y) {
                y += 1900;
            }
        }
        if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            return None;
        }
        Some((y, m as u8, d as u8))
    }
}
#[derive(Default)]
struct Time {
    fields: [i32; 4],
    len: usize,
    hour_offset: Option<i32>,
}
impl Time {
    fn add(&mut self, n: i32) -> bool {
        if self.len == 4 {
            return false;
        }
        self.fields[self.len] = n;
        self.len += 1;
        true
    }
    fn final_value(&mut self, n: i32) -> bool {
        if !self.add(n) {
            return false;
        }
        while self.len < 4 {
            self.add(0);
        }
        true
    }
    fn expecting(&self, n: i32) -> bool {
        match self.len {
            1 | 2 => (0..=59).contains(&n),
            3 => (0..=999).contains(&n),
            _ => false,
        }
    }
    fn write(mut self) -> Option<i64> {
        while self.len < 4 {
            self.add(0);
        }
        let [mut h, m, s, ms] = self.fields;
        if let Some(offset) = self.hour_offset {
            if !(0..=12).contains(&h) {
                return None;
            }
            h = h % 12 + offset;
        }
        if !((0..=23).contains(&h)
            && (0..=59).contains(&m)
            && (0..=59).contains(&s)
            && (0..=999).contains(&ms))
            && !(h == 24 && m == 0 && s == 0 && ms == 0)
        {
            return None;
        }
        Some(i64::from(h) * 3_600_000 + i64::from(m) * 60_000 + i64::from(s) * 1000 + i64::from(ms))
    }
}
#[derive(Default)]
struct Zone {
    sign: Option<i32>,
    hour: Option<i32>,
    minute: Option<i32>,
}
impl Zone {
    fn set(&mut self, hour: i32) {
        self.sign = Some(if hour < 0 { -1 } else { 1 });
        self.hour = Some(hour.abs());
        self.minute = Some(0);
    }
    fn utc(&self) -> bool {
        self.hour == Some(0) && self.minute == Some(0)
    }
    fn expecting(&self, n: i32) -> bool {
        self.hour.is_some() && self.minute.is_none() && (0..=59).contains(&n)
    }
    fn write(&self) -> Option<Option<i32>> {
        let Some(sign) = self.sign else {
            return Some(None);
        };
        let total = (self.hour.unwrap_or(0) as u32)
            .wrapping_mul(3600)
            .wrapping_add((self.minute.unwrap_or(0) as u32).wrapping_mul(60));
        if total > i32::MAX as u32 {
            return None;
        }
        Some(Some(total as i32 * sign))
    }
}

fn iso(s: &mut Scanner<'_>, day: &mut Day, time: &mut Time, zone: &mut Zone) -> Token {
    let invalid = Token::new(Kind::Invalid, 0, 0);
    if s.peek.sign() {
        let sign = s.next();
        if !s.peek.fixed(6) {
            return sign;
        }
        let y = s.next().value;
        if sign.symbol(b'-') && y == 0 {
            return sign;
        }
        day.add(if sign.symbol(b'-') { -y } else { y });
    } else if s.peek.fixed(4) {
        day.add(s.next().value);
    } else {
        return s.next();
    }
    if s.skip(b'-') {
        if !s.peek.fixed(2) || !(1..=12).contains(&s.peek.value) {
            return s.next();
        }
        day.add(s.next().value);
        if s.skip(b'-') {
            if !s.peek.fixed(2) || !(1..=31).contains(&s.peek.value) {
                return s.next();
            }
            day.add(s.next().value);
        }
    }
    if s.peek.kind != Kind::Separator {
        if s.peek.kind != Kind::End {
            return s.next();
        }
    } else {
        s.next();
        if !s.peek.fixed(2) || !(0..=24).contains(&s.peek.value) {
            return invalid;
        }
        let hour24 = s.peek.value == 24;
        time.add(s.next().value);
        if !s.skip(b':') {
            return invalid;
        }
        if !s.peek.fixed(2) || !(0..=59).contains(&s.peek.value) || (hour24 && s.peek.value > 0) {
            return invalid;
        }
        time.add(s.next().value);
        if s.skip(b':') {
            if !s.peek.fixed(2) || !(0..=59).contains(&s.peek.value) || (hour24 && s.peek.value > 0)
            {
                return invalid;
            }
            time.add(s.next().value);
            if s.skip(b'.') {
                if s.peek.kind != Kind::Number || (hour24 && s.peek.value > 0) {
                    return invalid;
                }
                time.add(s.next().milliseconds());
            }
        }
        if s.peek.z() {
            s.next();
            zone.set(0);
        } else if s.peek.sign() {
            zone.sign = Some(if s.next().symbol(b'+') { 1 } else { -1 });
            if s.peek.fixed(4) {
                let n = s.next().value;
                let (h, m) = (n / 100, n % 100);
                if !(0..=23).contains(&h) || !(0..=59).contains(&m) {
                    return invalid;
                }
                zone.hour = Some(h);
                zone.minute = Some(m);
            } else {
                if !s.peek.fixed(2) || !(0..=23).contains(&s.peek.value) {
                    return invalid;
                }
                zone.hour = Some(s.next().value);
                if !s.skip(b':') {
                    return invalid;
                }
                if !s.peek.fixed(2) || !(0..=59).contains(&s.peek.value) {
                    return invalid;
                }
                zone.minute = Some(s.next().value);
            }
        }
        if s.peek.kind != Kind::End {
            return invalid;
        }
    }
    if zone.hour.is_none() && time.len == 0 {
        zone.set(0);
    }
    day.iso = true;
    Token::new(Kind::End, 0, 0)
}

/// Shared Gregorian arithmetic retained from the existing retry ISO parser.
pub(super) fn days_from_civil(year: i64, month: u8, day: u8) -> i64 {
    const BEFORE: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let previous = year - 1;
    let leap = year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0);
    365 * (year - 1970) + previous.div_euclid(4) - 492 - (previous.div_euclid(100) - 19)
        + previous.div_euclid(400)
        - 4
        + BEFORE[usize::from(month - 1)]
        + i64::from(leap && month > 2)
        + i64::from(day - 1)
}

pub(super) fn parse(units: &[u16]) -> Option<ParsedDate> {
    // JSDateMath's UTF16 conversion rejects lone surrogates and folds precisely
    // ECMAScript WhiteSpace (not Unicode LS/PS) before the byte tokenizer.
    let input = String::from_utf16(units).ok()?;
    let folded: String = input
        .chars()
        .map(|c| match c {
            '\u{0009}'
            | '\u{000b}'
            | '\u{000c}'
            | '\u{0020}'
            | '\u{00a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}' => ' ',
            _ => c,
        })
        .collect();
    let mut scanner = Scanner::new(folded.as_bytes());
    let mut day = Day::default();
    let mut time = Time::default();
    let mut zone = Zone::default();
    let mut token = iso(&mut scanner, &mut day, &mut time, &mut zone);
    if token.kind == Kind::Invalid {
        return None;
    }
    let mut has_number = day.len != 0;
    while token.kind != Kind::End {
        if token.kind == Kind::Number {
            has_number = true;
            let n = token.value;
            if scanner.skip(b':') {
                if scanner.skip(b':') {
                    if time.len != 0 {
                        return None;
                    }
                    time.add(n);
                    time.add(0);
                } else {
                    if !time.add(n) {
                        return None;
                    }
                    if scanner.peek.symbol(b'.') {
                        scanner.next();
                    }
                }
            } else if scanner.skip(b'.') && time.expecting(n) {
                time.add(n);
                if scanner.peek.kind != Kind::Number {
                    return None;
                }
                time.final_value(scanner.next().milliseconds());
            } else if zone.expecting(n) {
                zone.minute = Some(n);
            } else if time.expecting(n) {
                time.final_value(n);
                let peek = scanner.peek;
                if !matches!(peek.kind, Kind::End | Kind::Space) && !peek.z() && !peek.sign() {
                    return None;
                }
            } else {
                if !day.add(n) {
                    return None;
                }
                scanner.skip(b'-');
            }
        } else if token.keyword() {
            match token.kind {
                Kind::AmPm if time.len != 0 => time.hour_offset = Some(token.value),
                Kind::Month => {
                    day.named = Some(token.value);
                    scanner.skip(b'-');
                }
                Kind::Zone if has_number => zone.set(token.value),
                _ => {
                    if has_number || scanner.peek.kind == Kind::Number {
                        return None;
                    }
                }
            }
        } else if token.sign() && (zone.utc() || time.len != 0) {
            zone.sign = Some(if token.symbol(b'+') { 1 } else { -1 });
            let number = if scanner.peek.kind == Kind::Number {
                scanner.next()
            } else {
                Token::new(Kind::Number, 0, 0)
            };
            has_number = true;
            if scanner.peek.symbol(b':') {
                zone.hour = Some(number.value);
                zone.minute = None;
            } else if (1..=2).contains(&number.len) {
                zone.hour = Some(number.value);
                zone.minute = Some(0);
            } else if (3..=4).contains(&number.len) {
                zone.hour = Some(number.value / 100);
                zone.minute = Some(number.value % 100);
            } else {
                return None;
            }
        } else if (token.sign() || token.symbol(b')')) && has_number {
            return None;
        }
        token = scanner.next();
    }
    let (year, month, date) = day.write()?;
    let time_ms = time.write()?;
    let offset_seconds = zone.write()?;
    if !(-1_000_000..=1_000_000).contains(&year) {
        return None;
    }
    let wall_ms = days_from_civil(i64::from(year), month, date) * 86_400_000 + time_ms;
    if offset_seconds.is_none() && wall_ms.unsigned_abs() > 8_640_002_592_000_000 {
        return None;
    }
    Some(ParsedDate {
        wall_ms,
        offset_seconds,
    })
}
