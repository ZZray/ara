//! Fixed OMP596f2da ar/registry.ts + ar/open.ts recognition and path candidates.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadFormat {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
    TarZ,
    Asar,
    Rar,
    SevenZip,
    Iso,
    Cab,
    Cpio,
    Rpm,
    Ar,
    Deb,
    Lzh,
    Arj,
    Gz,
    Bz2,
    Xz,
    Zst,
    Z,
    Lzma,
}
const GROUPS: &[(ReadFormat, &[&str])] = &[
    (ReadFormat::Zip, &["zip", "jar", "war", "ear", "apk", "whl", "ipa", "xpi", "vsix", "nupkg", "cbz"]),
    (ReadFormat::Tar, &["tar"]),
    (ReadFormat::TarGz, &["tar.gz", "tgz"]),
    (ReadFormat::TarBz2, &["tar.bz2", "tbz2", "tbz"]),
    (ReadFormat::TarXz, &["tar.xz", "txz"]),
    (ReadFormat::TarZst, &["tar.zst", "tzst"]),
    (ReadFormat::TarZ, &["tar.z"]),
    (ReadFormat::Asar, &["asar"]),
    (ReadFormat::Rar, &["rar", "cbr"]),
    (ReadFormat::SevenZip, &["7z"]),
    (ReadFormat::Iso, &["iso"]),
    (ReadFormat::Cab, &["cab"]),
    (ReadFormat::Cpio, &["cpio"]),
    (ReadFormat::Rpm, &["rpm"]),
    (ReadFormat::Ar, &["ar", "a", "lib"]),
    (ReadFormat::Deb, &["deb"]),
    (ReadFormat::Lzh, &["lzh", "lha"]),
    (ReadFormat::Arj, &["arj"]),
    (ReadFormat::Gz, &["gz"]),
    (ReadFormat::Bz2, &["bz2"]),
    (ReadFormat::Xz, &["xz"]),
    (ReadFormat::Zst, &["zst"]),
    (ReadFormat::Z, &["z"]),
    (ReadFormat::Lzma, &["lzma"]),
];
fn extension_table() -> &'static Vec<(&'static str, ReadFormat)> {
    static TABLE: std::sync::LazyLock<Vec<(&str, ReadFormat)>> = std::sync::LazyLock::new(|| {
        let mut table: Vec<_> =
            GROUPS.iter().flat_map(|(format, names)| names.iter().map(|&name| (name, *format))).collect();
        table.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
        table
    });
    &TABLE
}
pub(crate) fn format_from_path(path: &str) -> Option<ReadFormat> {
    let normalized = path.to_lowercase();
    extension_table()
        .iter()
        .find_map(|(extension, format)| normalized.ends_with(&format!(".{extension}")).then_some(*format))
}
pub(crate) struct PathCandidate {
    pub(crate) archive_path: String,
    pub(crate) sub_path: String,
}
pub(crate) fn path_candidates(path: &str) -> Vec<PathCandidate> {
    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        let alternation =
            extension_table().iter().map(|(extension, _)| regex::escape(extension)).collect::<Vec<_>>().join("|");
        // Check the zero-width boundary separately: Rust regex has no lookahead.
        // Fixed JS gi without u uses ASCII folding for these ASCII extensions.
        regex::Regex::new(&format!("(?i-u:\\.(?:{alternation}))")).expect("fixed archive extensions")
    });
    let normalized = path.replace('\\', "/");
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for found in PATTERN.find_iter(&normalized) {
        let end = found.end();
        if normalized.as_bytes().get(end).is_some_and(|&next| next != b':') {
            continue;
        }
        let archive_path = path[..end].to_owned();
        let sub_path = normalized[end..].trim_start_matches(':').to_owned();
        if seen.insert((archive_path.clone(), sub_path.clone())) {
            candidates.push(PathCandidate { archive_path, sub_path });
        }
    }
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.archive_path.len()));
    candidates
}
pub(super) fn stem_member_name(path: Option<&Path>) -> WireString {
    let Some(path) = path else {
        return "data".into();
    };
    let normalized = path.to_string_lossy().replace('\\', "/");
    let base = normalized.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let base_wire = WireString::from(base);
    let lower = base.to_lowercase();
    for (extension, _) in extension_table() {
        if lower.encode_utf16().count() > extension.len() + 1 && lower.ends_with(&format!(".{extension}")) {
            return WireString::from_units(base_wire.units()[..base_wire.len() - extension.len() - 1].to_vec());
        }
    }
    if base.is_empty() { "data".into() } else { base.into() }
}
