//! `@path` include expansion for context files (OMP `discovery/at-imports.ts`).
//!
//! `@` must start the line or follow a space/tab; relative paths resolve
//! against the importing file's directory, `~/` against home; fenced code
//! and inline code spans stay verbatim; recursion stops after
//! [`MAX_AT_IMPORT_DEPTH`] hops and cycles are left as their literal token;
//! an unreadable target keeps its token.

use crate::fs::FsCache;
use crate::paths::{resolve, resolve_from};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Maximum recursive `@`-import hops (Claude Code's documented cap).
pub const MAX_AT_IMPORT_DEPTH: usize = 5;

/// `(^|[ \t])@([./~A-Za-z0-9_-][^\s]*)` with JavaScript's `\s` set.
static AT_IMPORT: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(^|[ \t])@([./~A-Za-z0-9_-][^\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}]*)",
    )
    .unwrap()
});

fn strip_trailing_punct(token: &str) -> &str {
    token.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\''])
}

pub struct Expander<'a> {
    pub fs: &'a FsCache,
    pub home: PathBuf,
    pub max_depth: usize,
}

impl Expander<'_> {
    /// Expand `@` references in `content`, which was read from `file_path`.
    pub fn expand(&self, content: &str, file_path: &Path) -> String {
        let source = resolve(file_path);
        let mut visited = HashSet::from([source.clone()]);
        let base = source.parent().unwrap_or(Path::new("/")).to_path_buf();
        self.expand_at(content, &base, 0, &mut visited)
    }

    fn expand_at(&self, content: &str, base: &Path, depth: usize, visited: &mut HashSet<PathBuf>) -> String {
        if depth >= self.max_depth {
            return content.to_string();
        }
        split_markdown_segments(content)
            .into_iter()
            .map(|(is_code, text)| {
                if is_code {
                    text
                } else {
                    text.split('\n')
                        .map(|line| self.expand_line(line, base, depth, visited))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            })
            .collect()
    }

    fn expand_line(&self, line: &str, base: &Path, depth: usize, visited: &mut HashSet<PathBuf>) -> String {
        if !line.contains('@') {
            return line.to_string();
        }
        let mut matches = Vec::new();
        for caps in AT_IMPORT.captures_iter(line) {
            let at = caps.get(0).map_or(0, |m| m.start()) + caps[1].len();
            if is_inside_inline_code(line, at) {
                continue;
            }
            let token = strip_trailing_punct(&caps[2]);
            if token.is_empty() {
                continue;
            }
            matches.push((at, at + 1 + token.len(), token.to_string()));
        }
        if matches.is_empty() {
            return line.to_string();
        }
        let mut out = String::new();
        let mut cursor = 0;
        for (start, end, import) in matches {
            out.push_str(&line[cursor..start]);
            match self.resolve_and_expand(&import, base, depth, visited) {
                Some(expanded) => out.push_str(&expanded),
                None => out.push_str(&line[start..end]),
            }
            cursor = end;
        }
        out.push_str(&line[cursor..]);
        out
    }

    fn resolve_and_expand(
        &self,
        import: &str,
        base: &Path,
        depth: usize,
        visited: &mut HashSet<PathBuf>,
    ) -> Option<String> {
        let resolved = if import == "~" {
            resolve(&self.home)
        } else if let Some(rest) = import.strip_prefix("~/") {
            resolve_from(&self.home, Path::new(rest))
        } else {
            resolve_from(base, Path::new(import))
        };
        if visited.contains(&resolved) {
            return None;
        }
        let content = self.fs.read_file(&resolved)?;
        // Shared across the whole tree so cycles through any file break.
        visited.insert(resolved.clone());
        let dir = resolved.parent().unwrap_or(Path::new("/")).to_path_buf();
        Some(self.expand_at(&content, &dir, depth + 1, visited))
    }
}

/// A line opening or closing a fence: 3+ backticks or tildes after spaces/tabs.
fn match_fence(line: &str) -> Option<(char, usize)> {
    let rest = line.trim_start_matches([' ', '\t']);
    let c = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let len = rest.chars().take_while(|x| *x == c).count();
    (len >= 3).then_some((c, len))
}

/// Alternating text/code segments; each keeps its lines' newlines.
fn split_markdown_segments(content: &str) -> Vec<(bool, String)> {
    let mut segments = Vec::new();
    let mut buffer = String::new();
    let mut in_code = false;
    let mut fence: Option<(char, usize)> = None;
    let lines: Vec<&str> = content.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let is_last = i == lines.len() - 1;
        let text = if is_last { (*line).to_string() } else { format!("{line}\n") };
        let found = match_fence(line);
        if found.is_some() && !in_code {
            if !buffer.is_empty() {
                segments.push((in_code, std::mem::take(&mut buffer)));
            }
            in_code = true;
            buffer.push_str(&text);
            fence = found;
        } else if let (Some((c, len)), true, Some((open_c, open_len))) = (found, in_code, fence)
            && c == open_c
            && len >= open_len
        {
            buffer.push_str(&text);
            segments.push((true, std::mem::take(&mut buffer)));
            in_code = false;
            fence = None;
        } else {
            buffer.push_str(&text);
        }
        if is_last && !buffer.is_empty() {
            segments.push((in_code, std::mem::take(&mut buffer)));
        }
    }
    segments
}

/// Backtick-parity scan: `position` lies inside an open inline code span.
fn is_inside_inline_code(line: &str, position: usize) -> bool {
    let bytes = line.as_bytes();
    let mut in_span = false;
    let mut i = 0;
    while i < position && i < bytes.len() {
        if bytes[i] == b'`' {
            while i < bytes.len() && bytes[i] == b'`' {
                i += 1;
            }
            in_span = !in_span;
        } else {
            i += 1;
        }
    }
    in_span
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_and_inline_code() {
        let segs = split_markdown_segments("a\n```\nb\n```\nc");
        assert_eq!(segs, [(false, "a\n".into()), (true, "```\nb\n```\n".into()), (false, "c".into())]);
        assert!(is_inside_inline_code("x `@a` y", 3));
        assert!(!is_inside_inline_code("x `a` @b", 6));
        assert_eq!(strip_trailing_punct("./guide.md,"), "./guide.md");
    }
}
