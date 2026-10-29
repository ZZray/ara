//! Local lossless text for fixed OMP snapcompact JavaScript-string boundaries.
//!
//! Source contract: OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/snapcompact/src/snapcompact.ts (MIT).
//!
//! JavaScript slicing counts UTF16 units and can keep a lone surrogate. This
//! carrier owns only text data; it has no RPC, Session or executable state.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Utf16CodePoint {
    Scalar(char),
    Lone(u16),
}

impl Utf16CodePoint {
    pub fn len_utf16(self) -> usize {
        match self {
            Self::Scalar(c) => c.len_utf16(),
            Self::Lone(_) => 1,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Utf16Text {
    units: Vec<u16>,
}

impl Utf16Text {
    pub fn from_units(units: Vec<u16>) -> Self {
        Self { units }
    }

    pub fn as_units(&self) -> &[u16] {
        &self.units
    }

    /// JavaScript string length, not Unicode scalar or grapheme count.
    pub fn len(&self) -> usize {
        self.units.len()
    }

    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// Exact unit slicing for already-resolved nonnegative indices.
    pub fn slice(&self, start: usize, end: usize) -> Self {
        let start = start.min(self.len());
        let end = end.min(self.len()).max(start);
        Self::from_units(self.units[start..end].to_vec())
    }

    pub fn append(&mut self, text: &Self) {
        self.units.extend_from_slice(text.as_units());
    }

    pub fn push_str(&mut self, text: &str) {
        self.units.extend(text.encode_utf16());
    }

    pub fn join(parts: &[Self], separator: &str) -> Self {
        let mut text = Self::default();
        for (index, part) in parts.iter().enumerate() {
            if index > 0 {
                text.push_str(separator);
            }
            text.append(part);
        }
        text
    }

    pub fn codepoints(&self) -> impl Iterator<Item = Utf16CodePoint> + '_ {
        char::decode_utf16(self.units.iter().copied()).map(|point| match point {
            Ok(c) => Utf16CodePoint::Scalar(c),
            Err(error) => Utf16CodePoint::Lone(error.unpaired_surrogate()),
        })
    }

    /// Checked scalar projection; a lone unit is an explicit error.
    pub fn to_scalar(&self) -> Result<String, std::string::FromUtf16Error> {
        String::from_utf16(&self.units)
    }

    /// Explicit compatibility/native UTF8 conversion, never an exact archive
    /// projection. Legitimate U+FFFD scalars remain distinct before this call.
    pub fn to_lossy_string(&self) -> String {
        String::from_utf16_lossy(&self.units)
    }
}

impl From<&str> for Utf16Text {
    fn from(text: &str) -> Self {
        Self::from_units(text.encode_utf16().collect())
    }
}

impl From<String> for Utf16Text {
    fn from(text: String) -> Self {
        Self::from(text.as_str())
    }
}
