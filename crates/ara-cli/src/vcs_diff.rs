//! Fixed OMP Git patch block parsing and joining, 596f2da (MIT).
//!
//! Sources: crates/pi-vcs/src/git/patch.rs::join_patches and
//! packages/coding-agent/src/commit/git/diff.ts::parseFileDiffs.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//!
//! Host-owned pure facade: it performs no Git command, staging or file effect.
//! A Git-facing Host consumer must exercise it before production integration
//! can be claimed. Agent/Core state and the edit engine are not changed here.

use ara_rpc::WireString;

use crate::js_regex::JsRegExp;

/// Fixed commit/types.ts FileDiff. Content is the complete original block,
/// including every trailing newline and Git binary-patch terminator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    pub filename: WireString,
    pub content: WireString,
    pub additions: usize,
    pub deletions: usize,
    pub is_binary: bool,
}

/// Join patch parts verbatim, adding one final newline only when absent.
/// This is the fixed pi-vcs native Rust operation, including empty parts.
pub fn join_patches(parts: &[String]) -> String {
    let capacity = parts.iter().map(|part| part.len() + usize::from(!part.ends_with('\n'))).sum();
    let mut joined = String::with_capacity(capacity);
    for part in parts {
        joined.push_str(part);
        if !part.ends_with('\n') {
            joined.push('\n');
        }
    }
    joined
}

/// Fixed parseFileDiffs: split before a line-start `diff --git ` header,
/// retaining the delimiter and complete block. No trimming/re-encoding is
/// applied to content. The original header expression and UTF-16 matching
/// use the Host's existing ECMAScript matcher.
pub fn parse_file_diffs(diff: &WireString) -> Vec<FileDiff> {
    let units = diff.units();
    let marker: Vec<u16> = "diff --git ".encode_utf16().collect();
    let binary_marker: Vec<u16> = "Binary files ".encode_utf16().collect();
    let mut boundaries = vec![0];
    for index in 1..units.len() {
        // ECMAScript /^/m recognizes these four line terminators. Splitting
        // on the zero-width lookahead never consumes any preceding newline.
        if matches!(units[index - 1], 0x000a | 0x000d | 0x2028 | 0x2029) && units[index..].starts_with(&marker) {
            boundaries.push(index);
        }
    }
    boundaries.push(units.len());
    let mut header_match = JsRegExp::new("diff --git a\\/(.+?) b\\/(.+)$".into(), "")
        .expect("fixed Git header regular expression compiles");
    let mut sections = Vec::new();
    for boundary in boundaries.windows(2) {
        let part = &units[boundary[0]..boundary[1]];
        let header_end = part.iter().position(|unit| *unit == 0x000a).unwrap_or(part.len());
        let header = WireString::from_units(part[..header_end].to_vec());
        // The fixed source first skips whitespace-only parts. They cannot
        // match this header expression, so the same skip needs no projection.
        let Some(found) = header_match.exec(&header) else { continue };
        let filename = found.captures[2].clone().expect("fixed header match has the destination capture");
        let mut additions = 0;
        let mut deletions = 0;
        let mut is_binary = false;
        for line in part.split(|unit| *unit == 0x000a) {
            if line.starts_with(&binary_marker) {
                is_binary = true;
            }
            if line.starts_with(&[43, 43, 43]) || line.starts_with(&[45, 45, 45]) {
                continue;
            }
            if line.first() == Some(&43) {
                additions += 1;
            } else if line.first() == Some(&45) {
                deletions += 1;
            }
        }
        sections.push(FileDiff {
            filename,
            content: WireString::from_units(part.to_vec()),
            additions,
            deletions,
            // Fixed source tests `Binary files ` only, not `GIT binary patch`.
            // Preserve that public flag while retaining both kinds verbatim.
            is_binary,
        });
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_vcs_join_and_binary_roundtrip_five_original_rows_group() {
        // B-cecdcfc208: keep the final empty context row's space and newline.
        let parts = ["@@ -1,2 +1,2 @@\n", "foo\n", "@@ -10,4 +10,4 @@\n", "line1\n", "-old\n", "+new\n", " \n"]
            .map(str::to_owned);
        let joined = join_patches(&parts);
        assert!(joined.ends_with(" \n"));
        assert_eq!(joined.trim_end_matches(|ch| matches!(ch, ' ' | '\t')), joined);

        // B-03dceb8187: the original fixture checks final newline. Its title
        // does not authorize removing existing multiple newlines in parts.
        let joined = join_patches(&["line1\n".into(), "line2\n".into(), "line3".into()]);
        assert!(joined.ends_with('\n'));
        assert_eq!(join_patches(&["one\n\n".into(), "two".into(), String::new()]), "one\n\ntwo\n\n");

        // B-3839ec5699: give every missing trailing newline exactly one.
        let joined = join_patches(&["line1".into(), "line2".into()]);
        assert!(joined.contains("line1\n"));
        assert!(joined.ends_with("line2\n"));

        let text = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n a\n-b\n+b changed\n";
        let binary = "diff --git a/bin.dat b/bin.dat\nindex 1111111..2222222 100644\nGIT binary patch\nliteral 6\nzc$@zAB0000\n\n";

        // B-5dc95106f8: parser -> native join preserves the last block's
        // binary blank terminator byte-exact, rather than trimming each part.
        let diff = format!("{text}{binary}");
        let sections = parse_file_diffs(&diff.as_str().into());
        let parts: Vec<_> = sections.iter().map(|section| section.content.to_utf8().unwrap()).collect();
        let rebuilt = join_patches(&parts);
        assert_eq!(rebuilt, diff);
        assert!(rebuilt.ends_with("zc$@zAB0000\n\n"));
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].filename, WireString::from("a.txt"));
        assert_eq!((sections[0].additions, sections[0].deletions), (1, 1));
        assert_eq!(sections[1].filename, WireString::from("bin.dat"));
        assert!(!sections[1].is_binary, "preserve the original Binary files-only flag");

        // B-ccf1f805f0: the binary blank terminator also survives before a
        // following text block, without consuming the zero-width delimiter.
        let diff = format!("{binary}{text}");
        let sections = parse_file_diffs(&diff.as_str().into());
        let parts: Vec<_> = sections.iter().map(|section| section.content.to_utf8().unwrap()).collect();
        let rebuilt = join_patches(&parts);
        assert_eq!(rebuilt, diff);
        assert!(rebuilt.contains("zc$@zAB0000\n\ndiff --git a/a.txt"));

        // Related source contracts stay in this same coarse entrance.
        let input = "ignored preamble\ndiff --git a/old b/new\nBinary files a/old and b/new differ\n";
        let sections = parse_file_diffs(&input.into());
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].filename, WireString::from("new"));
        assert!(sections[0].is_binary);
        let mut lone: Vec<u16> = "diff --git a/old b/".encode_utf16().collect();
        lone.extend([0xd800, 0x000a]);
        let source = WireString::from_units(lone);
        let sections = parse_file_diffs(&source);
        assert_eq!(sections[0].filename.units(), &[0xd800]);
        assert_eq!(sections[0].content, source);
    }
}
