//! Head truncation and the model-facing output notice (OMP
//! `session/streaming-output.ts` `truncateHead` and `tools/output-meta.ts`
//! `formatOutputNotice`).

use crate::format_bytes;

#[derive(Clone, Debug, PartialEq)]
pub struct HeadTruncation {
    pub content: String,
    pub truncated: bool,
    pub by_bytes: bool,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub output_lines: usize,
    pub output_bytes: usize,
}

impl HeadTruncation {
    /// `details.truncation` (upstream `TruncationResult` fields).
    pub fn details(&self) -> serde_json::Value {
        serde_json::json!({
            "truncated": self.truncated,
            "truncatedBy": if self.by_bytes { "bytes" } else { "lines" },
            "totalLines": self.total_lines,
            "totalBytes": self.total_bytes,
            "outputLines": self.output_lines,
            "outputBytes": self.output_bytes,
        })
    }
}

/// Keep whole leading lines within `max_lines` and `max_bytes` (joined with
/// `\n`). A first line over the byte cap yields empty content.
pub fn truncate_head(content: &str, max_lines: usize, max_bytes: usize) -> HeadTruncation {
    let total_bytes = content.len();
    let total_lines = content.matches('\n').count() + 1;
    let mut t = HeadTruncation {
        content: content.to_string(),
        truncated: false,
        by_bytes: false,
        total_lines,
        total_bytes,
        output_lines: total_lines,
        output_bytes: total_bytes,
    };
    if total_lines <= max_lines && total_bytes <= max_bytes {
        return t;
    }
    let mut used = 0usize;
    let mut lines = 0usize;
    let mut cut = 0usize;
    let mut by_bytes = false;
    for (i, line) in content.split('\n').enumerate() {
        if lines >= max_lines {
            break;
        }
        let sep = usize::from(i > 0);
        if used + sep + line.len() > max_bytes {
            by_bytes = true;
            break;
        }
        used += sep + line.len();
        lines += 1;
        cut += sep + line.len();
    }
    if lines >= max_lines && used <= max_bytes {
        by_bytes = false;
    }
    t.content = content[..cut].to_string();
    t.truncated = true;
    t.by_bytes = by_bytes;
    t.output_lines = lines;
    t.output_bytes = used;
    t
}

/// Notice parts collected for one result.
#[derive(Default)]
pub struct Notice {
    parts: Vec<String>,
}

impl Notice {
    /// Head truncation (`formatTruncationMetaNotice`, direction head, start 1).
    pub fn truncation(&mut self, t: &HeadTruncation) -> &mut Self {
        if !t.truncated {
            return self;
        }
        let mut s = if t.output_lines >= 1 {
            format!("Showing lines 1-{} of {}", t.output_lines, t.total_lines)
        } else {
            format!("Showing {} of {} lines", t.output_lines, t.total_lines)
        };
        if t.by_bytes {
            s.push_str(&format!(" ({} limit)", format_bytes(t.output_bytes as u64)));
        }
        s.push_str(&format!(". Use :{} to continue", t.output_lines + 1));
        self.parts.push(s);
        self
    }

    pub fn result_limit(&mut self, reached: usize) -> &mut Self {
        self.parts.push(format!("{reached} results limit reached. Use limit={} for more", reached * 2));
        self
    }

    pub fn column_max(&mut self, max: usize) -> &mut Self {
        self.parts.push(format!("Some lines truncated to {max} chars"));
        self
    }

    /// `\n\n[part. part]`, or empty.
    pub fn render(&self) -> String {
        if self.parts.is_empty() { String::new() } else { format!("\n\n[{}]", self.parts.join(". ")) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_truncation_keeps_whole_lines() {
        let t = truncate_head("aa\nbb\ncc", usize::MAX, 5);
        assert_eq!((t.content.as_str(), t.output_lines, t.by_bytes), ("aa\nbb", 2, true));
        let mut n = Notice::default();
        n.truncation(&t);
        assert_eq!(n.render(), "\n\n[Showing lines 1-2 of 3 (5B limit). Use :3 to continue]");
        let t = truncate_head("aaaaaaa\nb", usize::MAX, 5);
        assert_eq!((t.content.as_str(), t.output_lines), ("", 0));
        let t = truncate_head("a\nb\nc", 2, 100);
        assert_eq!((t.content.as_str(), t.by_bytes), ("a\nb", false));
        assert!(!truncate_head("abc", 5, 5).truncated);
    }
}
