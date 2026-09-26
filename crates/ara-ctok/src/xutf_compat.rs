//! Stable-Rust replacements for the xutf normalization/category calls used by
//! the pinned Claude counter. The original xutf 1.5 source requires nightly
//! `portable_simd`; ARA's Core builds with stable Rust.

pub use unicode_general_category::GeneralCategory;
use unicode_general_category::get_general_category;
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeneralCategoryGroup {
    Letter,
    Mark,
    Number,
    Punctuation,
    Symbol,
    Separator,
    Other,
}

pub trait Ucd {
    fn general_category(self) -> GeneralCategory;
    fn general_category_group(self) -> GeneralCategoryGroup;
}

impl Ucd for char {
    fn general_category(self) -> GeneralCategory {
        get_general_category(self)
    }

    fn general_category_group(self) -> GeneralCategoryGroup {
        match get_general_category(self).abbreviation().as_bytes()[0] {
            b'L' => GeneralCategoryGroup::Letter,
            b'M' => GeneralCategoryGroup::Mark,
            b'N' => GeneralCategoryGroup::Number,
            b'P' => GeneralCategoryGroup::Punctuation,
            b'S' => GeneralCategoryGroup::Symbol,
            b'Z' => GeneralCategoryGroup::Separator,
            _ => GeneralCategoryGroup::Other,
        }
    }
}

pub trait ToUnicodeNormalized {
    fn to_nfc(&self) -> String;
}

impl ToUnicodeNormalized for str {
    fn to_nfc(&self) -> String {
        self.nfc().collect()
    }
}

pub trait IntoUnicodeNormalized {
    fn into_nfc(self) -> String;
}

impl IntoUnicodeNormalized for String {
    fn into_nfc(self) -> String {
        self.nfc().collect()
    }
}

pub fn canonical_combining_class(cp: u32) -> u8 {
    char::from_u32(cp).map_or(0, unicode_normalization::char::canonical_combining_class)
}

pub fn is_nfc(text: &str) -> bool {
    unicode_normalization::is_nfc(text)
}

pub fn is_nfc_codepoints(codepoints: impl Iterator<Item = u32>) -> bool {
    let text: String = codepoints.map(|cp| char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER)).collect();
    unicode_normalization::is_nfc(&text)
}
