//! Fixed OMP OSC8 display boundary. Link policy comes from the terminal Host.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
pub fn file_hyperlink(path: &str, display: &str, line: Option<u32>, enabled: bool) -> String {
    if !enabled || display.contains("\x1b]8;") {
        return display.to_owned();
    }
    let absolute = if std::path::Path::new(path).is_absolute() || (path.as_bytes().get(1) == Some(&b':')) {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path).to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_owned())
    };
    let mut normalized = absolute.replace('\\', "/");
    // Windows pathToFileURL gives a UNC server its URL authority.
    // The path continues after the server; it is not a four-slash pathname.
    let unc = cfg!(windows) && normalized.starts_with("//");
    if unc {
        if let Some(end) = normalized[2..].find('/').map(|end| end + 2) {
            let server = normalized[2..end].to_ascii_lowercase();
            normalized.replace_range(2..end, &server);
        }
    }
    let mut encoded = String::new();
    for b in normalized.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
            encoded.push(char::from(b));
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    let mut uri = if unc {
        format!("file:{encoded}")
    } else if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    };
    if let Some(line) = line.filter(|line| *line > 0) {
        uri.push_str(&format!("?line={line}"));
    }
    let hash = uri.encode_utf16().fold(0u32, |hash, unit| hash.wrapping_mul(31).wrapping_add(u32::from(unit)));
    format!("\x1b]8;id={hash:08x};{uri}\x1b\\{display}\x1b]8;;\x1b\\")
}
