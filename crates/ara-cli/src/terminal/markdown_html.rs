//! Fixed OMP terminal HTML normalization; no HTML execution.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
#[derive(Default)]
pub(super) struct HtmlState {
    lists: Vec<(bool, u64)>,
    open: Vec<bool>,
    content: Vec<bool>,
}
impl HtmlState {
    pub fn mark(&mut self, text: &str) {
        if !text.trim().is_empty()
            && let Some(c) = self.content.last_mut()
        {
            *c = true;
        }
    }
    fn indent(&self) -> String {
        "  ".repeat(self.lists.len().saturating_sub(1))
    }
    fn empty_item(&self) -> bool {
        self.open.last() == Some(&true) && self.content.last() != Some(&true)
    }
}
pub(super) fn entities(source: &str) -> String {
    regex::Regex::new(r"(?i)&(amp|lt|gt|quot|apos|nbsp|#\d+|#x[0-9a-f]+);")
        .unwrap()
        .replace_all(source, |caps: &regex::Captures<'_>| {
            let e = caps[1].to_lowercase();
            match e.as_str() {
                "amp" => "&".into(),
                "lt" => "<".into(),
                "gt" => ">".into(),
                "quot" => "\"".into(),
                "apos" => "'".into(),
                "nbsp" => " ".into(),
                _ => {
                    let cp = if let Some(h) = e.strip_prefix("#x") {
                        u32::from_str_radix(h, 16).ok()
                    } else {
                        e.strip_prefix('#').and_then(|n| n.parse().ok())
                    };
                    cp.and_then(char::from_u32).map(|c| c.to_string()).unwrap_or_default()
                }
            }
        })
        .into_owned()
}
fn line_break(out: &mut String, force: bool) {
    let len = out.trim_end_matches([' ', '\t']).len();
    out.truncate(len);
    if force || !out.ends_with('\n') {
        out.push('\n');
    }
}
fn list_break(out: &mut String, state: &HtmlState) {
    if !out.ends_with(&format!("{}\n", state.indent())) {
        line_break(out, false);
    }
}
pub(super) fn normalize(source: &str, state: &mut HtmlState, code: Option<&dyn Fn(&str) -> String>) -> String {
    let source = regex::Regex::new(r"(?s)<!--.*?-->").unwrap().replace_all(source, "");
    let tag = regex::Regex::new(r"(?i)</?(?:br|p|ol|ul|li|span|text|code|hr|blockquote)\b(?:\s[^>]*)?\s*/?>").unwrap();
    let name_re = regex::Regex::new(r"^</?\s*([A-Za-z][A-Za-z0-9:-]*)").unwrap();
    let start_re = regex::Regex::new(r#"(?i)\bstart\s*=\s*(?:"(\d+)"|'(\d+)'|(\d+))"#).unwrap();
    let mut out = String::new();
    let mut last = 0;
    let mut in_code = false;
    for m in tag.find_iter(&source) {
        let raw = m.as_str();
        let before = entities(&source[last..m.start()]);
        let name = name_re.captures(raw).map(|c| c[1].to_lowercase()).unwrap_or_default();
        if name == "span" || name == "text" || in_code || !before.trim().is_empty() {
            if in_code && let Some(code) = code {
                out.push_str(&code(&before));
            } else {
                out.push_str(&before);
            }
            state.mark(&before);
        }
        last = m.end();
        let closing = raw.starts_with("</");
        let self_closing = regex::Regex::new(r"/\s*>$").unwrap().is_match(raw);
        match name.as_str() {
            "span" | "text" => {}
            "code" => {
                if closing {
                    in_code = false;
                } else if !self_closing {
                    in_code = true;
                }
            }
            "br" | "hr" => line_break(&mut out, true),
            "p" | "blockquote" => {
                if closing || !out.trim().is_empty() && !out.ends_with('\n') && !state.empty_item() {
                    line_break(&mut out, false);
                }
            }
            "ol" | "ul" => {
                if closing {
                    state.lists.pop();
                    state.open.pop();
                    state.content.pop();
                } else if !self_closing {
                    if state.open.last() == Some(&true) {
                        list_break(&mut out, state);
                    }
                    let start = if name == "ol" {
                        start_re
                            .captures(raw)
                            .and_then(|c| (1..=3).find_map(|i| c.get(i)))
                            .and_then(|v| v.as_str().parse().ok())
                            .unwrap_or(1)
                    } else {
                        1
                    };
                    state.lists.push((name == "ol", start));
                    state.open.push(false);
                    state.content.push(false);
                }
            }
            "li" => {
                if closing {
                    line_break(&mut out, false);
                    continue;
                }
                if !state.open.is_empty() {
                    let index = state.open.len() - 1;
                    if state.open[index] {
                        list_break(&mut out, state);
                    }
                    state.open[index] = true;
                    state.content[index] = false;
                } else if !out.trim().is_empty() && !out.ends_with('\n') {
                    line_break(&mut out, false);
                }
                let indent = state.indent();
                if let Some((true, next)) = state.lists.last_mut() {
                    out.push_str(&format!("{indent}{next}. "));
                    *next += 1;
                } else {
                    out.push_str(&format!("{indent}• "));
                }
            }
            _ => out.push_str(raw),
        }
    }
    let rest = entities(&source[last..]);
    state.mark(&rest);
    if in_code && let Some(code) = code {
        out.push_str(&code(&rest));
    } else {
        out.push_str(&rest);
    }
    out
}
