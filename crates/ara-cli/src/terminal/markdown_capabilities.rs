//! Host-selected Markdown facts from fixed OMP terminal-capabilities.ts.
//! This resolves the three facts used by Markdown. FrameContext remains the
//! owner of hyperlinks; terminal probes, notifications and image rendering are
//! separate surfaces. No renderer reads the environment.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::types::TerminalImageProtocol;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarkdownCapabilities {
    pub true_color: bool,
    pub text_sizing: bool,
    pub image_protocol: Option<TerminalImageProtocol>,
}

/// Only these public terminal markers are read by the Host resolver.
pub const MARKDOWN_CAPABILITY_ENV_KEYS: &[&str] = &[
    "KITTY_WINDOW_ID",
    "GHOSTTY_RESOURCES_DIR",
    "WEZTERM_PANE",
    "ITERM_SESSION_ID",
    "VSCODE_PID",
    "ALACRITTY_WINDOW_ID",
    "TERM_PROGRAM",
    "TERM",
    "COLORTERM",
    "ARA_FORCE_IMAGE_PROTOCOL",
    "WSL_DISTRO_NAME",
    "WSL_INTEROP",
    "PASEO_TERMINAL_ID",
    "HERDR_ENV",
    "HERDR_PANE_ID",
    "HERDR_TAB_ID",
    "HERDR_WORKSPACE_ID",
];

// Match fixed JS String.trim, using the same ECMA whitespace set as the
// existing Host auth_broker_client::trim_js_whitespace. Rust trim includes
// NEL and omits FEFF, so it is not an equivalent protocol selector.
fn trim_js_whitespace(text: &str) -> &str {
    text.trim_matches(|character: char| matches!(character, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'))
}

/// `platform` uses Node's fixed source names (`win32`, `darwin`, `linux`).
/// An empty environment value is false, as in the fixed JS detection code.
pub fn resolve_markdown_capabilities(
    env: &dyn Fn(&str) -> Option<String>,
    platform: &str,
    is_tty: bool,
) -> MarkdownCapabilities {
    let get = |name: &str| env(name).unwrap_or_default();
    let has = |name: &str| !get(name).is_empty();
    let markers = [
        ("KITTY_WINDOW_ID", "kitty"),
        ("GHOSTTY_RESOURCES_DIR", "ghostty"),
        ("WEZTERM_PANE", "wezterm"),
        ("ITERM_SESSION_ID", "iterm2"),
        ("VSCODE_PID", "vscode"),
        ("ALACRITTY_WINDOW_ID", "alacritty"),
    ];
    let program = get("TERM_PROGRAM").to_lowercase();
    let term = get("TERM").to_lowercase();
    let color = get("COLORTERM").to_lowercase();
    let id = markers.iter().find(|(key, _)| has(key)).map(|(_, id)| *id).unwrap_or_else(|| match program.as_str() {
        "kitty" => "kitty",
        "ghostty" => "ghostty",
        "wezterm" => "wezterm",
        "iterm.app" => "iterm2",
        "vscode" => "vscode",
        "alacritty" => "alacritty",
        "warpterminal" => "warp",
        "orca" => "orca",
        _ if term.contains("ghostty") => "ghostty",
        _ if color == "truecolor" || color == "24bit" => "trueColor",
        _ => "base",
    });
    let raw = get("ARA_FORCE_IMAGE_PROTOCOL");
    let forced = trim_js_whitespace(&raw).to_lowercase();
    let protocol = if !forced.is_empty() {
        // Explicit unknown values, as well as off/none/0/false, disable the
        // protocol. A forced protocol bypasses non-forced Host carve-outs.
        match forced.as_str() {
            "kitty" => Some(TerminalImageProtocol::Kitty),
            "iterm2" | "iterm" => Some(TerminalImageProtocol::Iterm2),
            "sixel" => Some(TerminalImageProtocol::Sixel),
            _ => None,
        }
    } else {
        let mut protocol = match id {
            "kitty" | "ghostty" | "wezterm" => Some(TerminalImageProtocol::Kitty),
            "iterm2" => Some(TerminalImageProtocol::Iterm2),
            "warp" => {
                let windows_host =
                    platform == "win32" || (platform == "linux" && (has("WSL_DISTRO_NAME") || has("WSL_INTEROP")));
                if windows_host { None } else { Some(TerminalImageProtocol::Kitty) }
            }
            _ => None,
        };
        // Warp's unsupported Windows path never enters the generic fallback.
        if id != "warp"
            && protocol.is_none()
            && is_tty
            && id != "vscode"
            && id != "alacritty"
            && (term.contains("screen") || term.contains("tmux") || term.contains("ghostty"))
        {
            protocol = Some(TerminalImageProtocol::Kitty);
        }
        let herdr = get("HERDR_ENV") == "1" || has("HERDR_PANE_ID") || has("HERDR_TAB_ID") || has("HERDR_WORKSPACE_ID");
        if has("PASEO_TERMINAL_ID") || herdr { None } else { protocol }
    };
    MarkdownCapabilities { true_color: id != "base", text_sizing: false, image_protocol: protocol }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_markdown_capability_detection_group() {
        use TerminalImageProtocol::{Iterm2, Kitty, Sixel};
        let resolve = |pairs: &[(&str, &str)], platform: &str, tty: bool| {
            resolve_markdown_capabilities(
                &|key| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string()),
                platform,
                tty,
            )
        };
        let cases: [(&[(&str, &str)], bool, Option<TerminalImageProtocol>); 13] = [
            (&[], false, None),
            (&[("COLORTERM", "24bit")], true, None),
            (&[("KITTY_WINDOW_ID", "0")], true, Some(Kitty)),
            (&[("GHOSTTY_RESOURCES_DIR", "path")], true, Some(Kitty)),
            (&[("WEZTERM_PANE", "1")], true, Some(Kitty)),
            (&[("ITERM_SESSION_ID", "1")], true, Some(Iterm2)),
            (&[("VSCODE_PID", "1"), ("TERM", "screen-256color")], true, None),
            (&[("ALACRITTY_WINDOW_ID", "1"), ("TERM", "tmux")], true, None),
            (&[("TERM_PROGRAM", "GhOsTtY")], true, Some(Kitty)),
            (&[("TERM_PROGRAM", "orca")], true, None),
            (&[("TERM_PROGRAM", "WarpTerminal")], true, None),
            (&[("TERM", "xterm-ghostty")], true, Some(Kitty)),
            (&[("TERM", "screen-256color")], false, Some(Kitty)),
        ];
        for (env, color, image) in cases {
            let caps = resolve(env, "win32", true);
            assert_eq!((caps.true_color, caps.text_sizing, caps.image_protocol), (color, false, image), "{env:?}");
        }
        assert_eq!(resolve(&[("KITTY_WINDOW_ID", ""), ("COLORTERM", "TRUECOLOR")], "linux", true).image_protocol, None);
        assert_eq!(
            resolve(&[("KITTY_WINDOW_ID", "1"), ("VSCODE_PID", "1")], "linux", true).image_protocol,
            Some(Kitty)
        );
        assert_eq!(resolve(&[("TERM_PROGRAM", "WarpTerminal")], "darwin", true).image_protocol, Some(Kitty));
        for wsl in ["WSL_DISTRO_NAME", "WSL_INTEROP"] {
            assert_eq!(resolve(&[("TERM_PROGRAM", "WarpTerminal"), (wsl, "1")], "linux", true).image_protocol, None);
        }
        assert_eq!(resolve(&[("TERM", "screen-256color")], "linux", false).image_protocol, None);
        // Static known support does not depend on isTTY; only fallback does.
        assert_eq!(resolve(&[("TERM_PROGRAM", "kitty")], "linux", false).image_protocol, Some(Kitty));
        for marker in ["PASEO_TERMINAL_ID", "HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID"] {
            assert_eq!(resolve(&[("TERM", "screen-256color"), (marker, "1")], "linux", true).image_protocol, None);
        }
        assert_eq!(resolve(&[("TERM_PROGRAM", "kitty"), ("HERDR_ENV", "1")], "linux", true).image_protocol, None);
        assert_eq!(
            resolve(&[("TERM_PROGRAM", "kitty"), ("HERDR_ENV", "0")], "linux", true).image_protocol,
            Some(Kitty)
        );
        assert_eq!(
            resolve(&[("TERM_PROGRAM", "kitty"), ("HERDR_SOCKET_PATH", "set")], "linux", true).image_protocol,
            Some(Kitty)
        );
        for (value, image) in [
            (" KiTtY ", Some(Kitty)),
            ("iterm", Some(Iterm2)),
            ("iterm2", Some(Iterm2)),
            ("sixel", Some(Sixel)),
            ("off", None),
            ("none", None),
            ("0", None),
            ("false", None),
            ("unknown", None),
        ] {
            assert_eq!(
                resolve(&[("ARA_FORCE_IMAGE_PROTOCOL", value), ("HERDR_ENV", "1")], "win32", true).image_protocol,
                image
            );
        }
        assert_eq!(
            resolve(&[("ARA_FORCE_IMAGE_PROTOCOL", " "), ("TERM_PROGRAM", "kitty")], "linux", true).image_protocol,
            Some(Kitty)
        );
        assert_eq!(
            resolve(&[("ARA_FORCE_IMAGE_PROTOCOL", "\u{feff}kitty\u{feff}")], "linux", true).image_protocol,
            Some(Kitty)
        );
        assert_eq!(
            resolve(&[("ARA_FORCE_IMAGE_PROTOCOL", "\u{0085}kitty\u{0085}")], "linux", true).image_protocol,
            None
        );
        assert_eq!(
            (Kitty.as_sequence_prefix(), Iterm2.as_sequence_prefix(), Sixel.as_sequence_prefix()),
            ("\x1b_G", "\x1b]1337;File=", "\x1bPq")
        );
    }
}
