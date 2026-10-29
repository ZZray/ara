//! Fixed OMP 596f2da display LaTeX rectangular layout.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::super::render_utils;
use super::{is_math_font_command, latex_color, latex_to_unicode, latex_to_unicode_with_color};
#[derive(Clone)]
struct Block {
    lines: Vec<String>,
    baseline: usize,
    width: usize,
}
fn spaces(n: usize) -> String {
    " ".repeat(n)
}
fn pad(line: &str, width: usize) -> String {
    format!("{line}{}", spaces(width.saturating_sub(render_utils::width(line))))
}
fn center(line: &str, width: usize) -> String {
    let extra = width.saturating_sub(render_utils::width(line));
    format!("{}{line}{}", spaces(extra / 2), spaces(extra - extra / 2))
}
fn text_box(text: &str) -> Block {
    let raw: Vec<_> = text.split('\n').collect();
    let width = raw.iter().map(|l| render_utils::width(l)).max().unwrap_or(0);
    Block { lines: raw.iter().map(|l| pad(l, width)).collect(), baseline: raw.len().saturating_sub(1) / 2, width }
}
fn pad_box(b: Block, width: usize, align: char) -> Block {
    if b.width >= width {
        return b;
    }
    Block {
        baseline: b.baseline,
        width,
        lines: b
            .lines
            .iter()
            .map(|l| match align {
                'r' => format!("{}{l}", spaces(width.saturating_sub(render_utils::width(l)))),
                'c' => center(l, width),
                _ => pad(l, width),
            })
            .collect(),
    }
}
fn hcat(boxes: Vec<Block>) -> Block {
    if boxes.len() == 1 {
        return boxes.into_iter().next().unwrap();
    }
    let above = boxes.iter().map(|b| b.baseline).max().unwrap_or(0);
    let below = boxes.iter().map(|b| b.lines.len().saturating_sub(1 + b.baseline)).max().unwrap_or(0);
    let width = boxes.iter().map(|b| b.width).sum();
    let lines = (0..above + below + 1)
        .map(|row| {
            boxes
                .iter()
                .map(|b| {
                    row.checked_sub(above - b.baseline)
                        .and_then(|local| b.lines.get(local))
                        .cloned()
                        .unwrap_or_else(|| spaces(b.width))
                })
                .collect()
        })
        .collect();
    Block { lines, baseline: above, width }
}
fn vcat(boxes: Vec<Block>, align: char) -> Block {
    if boxes.len() == 1 {
        return boxes.into_iter().next().unwrap();
    }
    let width = boxes.iter().map(|b| b.width).max().unwrap_or(0);
    let lines: Vec<_> = boxes
        .iter()
        .flat_map(|b| b.lines.iter().map(|l| if align == 'c' { center(l, width) } else { pad(l, width) }))
        .collect();
    Block { baseline: lines.len().saturating_sub(1) / 2, lines, width }
}
fn fraction(n: Block, d: Block) -> Block {
    let width = n.width.max(d.width) + 2;
    let mut lines: Vec<_> = n.lines.iter().map(|l| center(l, width)).collect();
    let baseline = lines.len();
    lines.push("─".repeat(width));
    lines.extend(d.lines.iter().map(|l| center(l, width)));
    Block { lines, baseline, width }
}
fn delimiter(key: &str, height: usize, baseline: usize) -> Option<Block> {
    if key.is_empty() {
        return None;
    }
    let pieces = match key {
        "(" => Some(["(", "⎛", "⎜", "⎝", ""]),
        ")" => Some([")", "⎞", "⎟", "⎠", ""]),
        "[" => Some(["[", "⎡", "⎢", "⎣", ""]),
        "]" => Some(["]", "⎤", "⎥", "⎦", ""]),
        "{" => Some(["{", "⎧", "⎪", "⎩", "⎨"]),
        "}" => Some(["}", "⎫", "⎪", "⎭", "⎬"]),
        "|" => Some(["|", "│", "│", "│", ""]),
        "‖" => Some(["‖", "║", "║", "║", ""]),
        "⌈" => Some(["⌈", "⎡", "⎢", "⎢", ""]),
        "⌉" => Some(["⌉", "⎤", "⎥", "⎥", ""]),
        "⌊" => Some(["⌊", "⎢", "⎢", "⎣", ""]),
        "⌋" => Some(["⌋", "⎥", "⎥", "⎦", ""]),
        _ => None,
    };
    if height <= 1 {
        return Some(text_box(pieces.map(|p| p[0]).unwrap_or(key)));
    }
    let width = render_utils::width(pieces.map(|p| p[0]).unwrap_or(key));
    let axis = baseline.max(1).min(height.saturating_sub(2));
    let lines = (0..height)
        .map(|row| match pieces {
            None => {
                if row == baseline {
                    key.into()
                } else {
                    spaces(width)
                }
            }
            Some(p) => {
                if row == 0 {
                    p[1].into()
                } else if row == height - 1 {
                    p[3].into()
                } else if row == axis && !p[4].is_empty() {
                    p[4].into()
                } else {
                    p[2].into()
                }
            }
        })
        .collect();
    Some(Block { lines, baseline, width })
}
fn delimit(b: Block, left: &str, right: &str) -> Block {
    let l = delimiter(left, b.lines.len(), b.baseline);
    let r = delimiter(right, b.lines.len(), b.baseline);
    if l.is_none() && r.is_none() {
        return b;
    }
    let tall = b.lines.len() > 1;
    let mut parts = vec![];
    if let Some(l) = l {
        parts.push(l);
    }
    if tall {
        parts.push(text_box(" "));
    }
    parts.push(b);
    if tall {
        parts.push(text_box(" "));
    }
    if let Some(r) = r {
        parts.push(r);
    }
    hcat(parts)
}
fn binomial(top: Block, bottom: Block) -> Block {
    let width = top.width.max(bottom.width);
    let mut lines: Vec<_> = top.lines.iter().map(|l| center(l, width)).collect();
    let baseline = lines.len();
    lines.push(spaces(width));
    lines.extend(bottom.lines.iter().map(|l| center(l, width)));
    delimit(Block { lines, baseline, width }, "(", ")")
}
fn radical(inner: Block, degree: Option<&str>) -> Block {
    let mut lines = vec![format!(" ┌{}", "─".repeat(inner.width + 1))];
    lines.extend(
        inner
            .lines
            .iter()
            .enumerate()
            .map(|(i, l)| format!("{}{l}", if i == inner.lines.len() - 1 { "╲│ " } else { " │ " })),
    );
    let b = Block { lines, baseline: inner.baseline + 1, width: inner.width + 3 };
    let Some(degree) = degree.filter(|d| !d.is_empty()) else {
        return b;
    };
    let deg = latex_to_unicode(&format!("^{{{degree}}}"));
    let width = render_utils::width(&deg);
    hcat(vec![Block { lines: vec![deg, spaces(width)], baseline: 1, width }, b])
}
fn limits(glyph: Block, sub: Option<Block>, sup: Option<Block>) -> Block {
    let width =
        glyph.width.max(sub.as_ref().map(|b| b.width).unwrap_or(0)).max(sup.as_ref().map(|b| b.width).unwrap_or(0));
    let mut lines = vec![];
    if let Some(s) = sup {
        lines.extend(s.lines.iter().map(|l| center(l, width)));
    }
    let baseline = lines.len() + glyph.baseline;
    lines.extend(glyph.lines.iter().map(|l| center(l, width)));
    if let Some(s) = sub {
        lines.extend(s.lines.iter().map(|l| center(l, width)));
    }
    Block { lines, baseline, width }
}
fn hbrace(content: Block, name: &str, label: Option<Block>) -> Block {
    let (l, c, r, over) = match name {
        "overbrace" => ("╭", "┴", "╮", true),
        "underbrace" => ("╰", "┬", "╯", false),
        "overbracket" => ("┌", "─", "┐", true),
        "underbracket" => ("└", "─", "┘", false),
        "overparen" => ("╭", "─", "╮", true),
        _ => ("╰", "─", "╯", false),
    };
    let brace_width = content.width.max(3);
    let width = brace_width.max(label.as_ref().map(|b| b.width).unwrap_or(0));
    let lead = (brace_width - 3) / 2;
    let brace = center(&format!("{l}{}{c}{}{r}", "─".repeat(lead), "─".repeat(brace_width - 3 - lead)), width);
    let contents: Vec<_> = content.lines.iter().map(|l| center(l, width)).collect();
    let labels: Vec<_> = label.iter().flat_map(|b| b.lines.iter().map(|l| center(l, width))).collect();
    let baseline = if over { labels.len() + 1 + content.baseline } else { content.baseline };
    let mut lines = vec![];
    if over {
        lines.extend(labels);
        lines.push(brace);
        lines.extend(contents);
    } else {
        lines.extend(contents);
        lines.push(brace);
        lines.extend(labels);
    }
    Block { lines, baseline, width }
}
fn attach(base: Block, sub: Option<Block>, sup: Option<Block>) -> Block {
    if sub.is_none() && sup.is_none() {
        return base;
    }
    let single = base.lines.len() == 1;
    let width = sub.as_ref().map(|b| b.width).unwrap_or(0).max(sup.as_ref().map(|b| b.width).unwrap_or(0));
    let mut lines = vec![];
    let mut baseline = 0;
    let had_sup = sup.is_some();
    if let Some(s) = sup {
        lines.extend(s.lines.iter().map(|l| pad(l, width)));
        let lift = if single { 1 } else { base.baseline };
        for _ in 0..lift {
            lines.push(spaces(width));
        }
        baseline = lines.len() - 1;
    }
    if let Some(s) = sub {
        let below = base.lines.len() as isize - 1 - base.baseline as isize - (s.lines.len() as isize - 1);
        let mut drop = below.max(if single { 1 } else { 0 }) as usize;
        if had_sup && drop < 1 {
            drop = 1;
        }
        let gap = if lines.is_empty() { drop } else { drop.saturating_sub(1) };
        for _ in 0..gap {
            lines.push(spaces(width));
        }
        lines.extend(s.lines.iter().map(|l| pad(l, width)));
    }
    hcat(vec![base, Block { lines, baseline, width }])
}
fn grid(rows: Vec<Vec<Block>>, align: impl Fn(usize) -> char, gap: impl Fn(usize) -> usize, row_gap: usize) -> Block {
    let ncols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if ncols == 0 {
        return text_box("");
    }
    let mut widths = vec![0; ncols];
    for row in &rows {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(c.width);
        }
    }
    let nrows = rows.len();
    let mut boxes = vec![];
    for row in rows {
        if row_gap > 0 && !boxes.is_empty() {
            for _ in 0..row_gap {
                boxes.push(text_box(""));
            }
        }
        let mut parts = vec![];
        for (j, &w) in widths.iter().enumerate() {
            if j > 0 && gap(j) > 0 {
                parts.push(text_box(&spaces(gap(j))));
            }
            parts.push(pad_box(row.get(j).cloned().unwrap_or_else(|| text_box("")), w, align(j)));
        }
        boxes.push(hcat(parts));
    }
    let mut b = vcat(boxes, 'l');
    if row_gap > 0 && nrows > 1 && b.lines.len() % 2 == 0 {
        b.baseline = b.lines.len() / 2;
        b.lines.push(spaces(b.width));
    }
    b
}

#[derive(Clone)]
struct Context {
    wrappers: Vec<(String, String)>,
    true_color: bool,
}
impl Default for Context {
    fn default() -> Self {
        Self { wrappers: vec![], true_color: true }
    }
}
impl Context {
    fn unicode(&self, source: &str) -> String {
        latex_to_unicode_with_color(source, self.true_color)
    }
    fn wrap(&self, run: &str) -> String {
        let mut out = run.to_owned();
        for (l, r) in self.wrappers.iter().rev() {
            out = format!("{l}{out}{r}");
        }
        out
    }
    fn prepend(&self, color: &str) -> Self {
        let mut c = self.clone();
        if !color.is_empty() {
            c.wrappers.push((color.into(), String::new()));
        }
        c
    }
    fn scoped(&self, color: &str, prefix: &str) -> Self {
        let mut c = self.clone();
        c.wrappers.push((format!("{color}{prefix}{{"), "}".into()));
        c
    }
}
struct Source {
    chars: Vec<char>,
}
impl Source {
    fn new(s: &str) -> Self {
        Self { chars: s.chars().collect() }
    }
    fn len(&self) -> usize {
        self.chars.len()
    }
    fn get(&self, i: usize) -> Option<char> {
        self.chars.get(i).copied()
    }
    fn slice(&self, l: usize, r: usize) -> String {
        self.chars[l.min(self.len())..r.min(self.len())].iter().collect()
    }
    fn starts(&self, i: usize, t: &str) -> bool {
        self.chars.get(i..).is_some_and(|s| s.iter().take(t.chars().count()).copied().eq(t.chars()))
    }
    fn spaces(&self, mut i: usize, all: bool) -> usize {
        while self.get(i).is_some_and(|c| c == ' ' || all && matches!(c, '\t' | '\n')) {
            i += 1;
        }
        i
    }
    fn name(&self, mut i: usize) -> (String, usize) {
        let start = i;
        while self.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
            i += 1;
        }
        (self.slice(start, i), i)
    }
    fn bracket_end(&self, i: usize) -> usize {
        (i + 1..self.len()).find(|&p| self.get(p) == Some(']')).map(|p| p + 1).unwrap_or(self.len())
    }
    fn brace(&self, i: usize) -> (String, usize) {
        let mut depth = 0;
        let mut out = String::new();
        let mut j = i;
        while let Some(c) = self.get(j) {
            j += 1;
            if c == '\\' {
                out.push(c);
                if let Some(n) = self.get(j) {
                    out.push(n);
                    j += 1;
                }
                continue;
            }
            if c == '{' {
                depth += 1;
                if depth > 1 {
                    out.push(c);
                }
                continue;
            }
            if c == '}' {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                out.push(c);
                continue;
            }
            out.push(c);
        }
        (out, j)
    }
    fn arg(&self, i: usize) -> (String, usize) {
        let i = self.spaces(i, true);
        let Some(c) = self.get(i) else {
            return (String::new(), i);
        };
        if c == '{' {
            return self.brace(i);
        }
        if c != '\\' {
            return (c.to_string(), i + 1);
        }
        let (name, j) = self.name(i + 1);
        if name == "begin"
            && let Some(env) = self.environment(i)
        {
            return (self.slice(i, env.end), env.end);
        }
        if name.is_empty() {
            return (self.slice(i, i + 2), i + 2);
        }
        let mut end = j;
        if let Some(arity) = arity(&name) {
            loop {
                end = self.spaces(end, true);
                if self.get(end) != Some('[') {
                    break;
                }
                end = self.bracket_end(end);
            }
            for _ in 0..arity {
                end = self.arg(end).1;
            }
        } else {
            while matches!(self.get(end), Some('[' | '{')) {
                end = if self.get(end) == Some('{') { self.brace(end).1 } else { self.bracket_end(end) };
            }
        }
        (self.slice(i, end), end)
    }
    fn delim_token(&self, i: usize) -> Option<(String, usize)> {
        let i = self.spaces(i, false);
        let c = self.get(i)?;
        if c != '\\' {
            return Some((c.to_string(), i + 1));
        }
        if !self.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic()) {
            return Some((self.slice(i, i + 2), i + 2));
        }
        let (_, end) = self.name(i + 1);
        Some((self.slice(i, end), end))
    }
    fn left_right(&self, start: usize) -> Option<LeftRight> {
        let (left, mut k) = self.delim_token(start + 5)?;
        let mut segments = vec![];
        let mut middles = vec![];
        let mut depth = 1;
        let mut seg_start = k;
        while k < self.len() {
            if self.get(k) != Some('\\') {
                k += 1;
                continue;
            }
            if self.starts(k, "\\left") && !self.get(k + 5).is_some_and(|c| c.is_ascii_alphabetic()) {
                depth += 1;
                k = self.delim_token(k + 5).map(|t| t.1).unwrap_or(k + 5);
                continue;
            }
            if self.starts(k, "\\right") && !self.get(k + 6).is_some_and(|c| c.is_ascii_alphabetic()) {
                depth -= 1;
                let tok = self.delim_token(k + 6).unwrap_or((".".into(), k + 6));
                if depth == 0 {
                    segments.push(self.slice(seg_start, k));
                    return Some(LeftRight { left, segments, middles, right: tok.0, end: tok.1 });
                }
                k = tok.1;
                continue;
            }
            if depth == 1 && self.starts(k, "\\middle") && !self.get(k + 7).is_some_and(|c| c.is_ascii_alphabetic()) {
                segments.push(self.slice(seg_start, k));
                let (token, end) = self.delim_token(k + 7).unwrap_or(("|".into(), k + 7));
                middles.push(token);
                k = end;
                seg_start = end;
                continue;
            }
            k += 2;
        }
        None
    }
    fn match_delim(&self, i: usize, open: char, close: char) -> Option<usize> {
        let mut depth = 0;
        let mut k = i;
        while let Some(c) = self.get(k) {
            if c == '\\' {
                k += 2;
                continue;
            }
            if c == '{' {
                k = self.brace(k).1;
                continue;
            }
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            k += 1;
        }
        None
    }
    fn environment(&self, start: usize) -> Option<Environment> {
        let i = self.spaces(start + 6, false);
        if self.get(i) != Some('{') {
            return None;
        }
        let (env, body_start) = self.brace(i);
        let mut k = body_start;
        let mut depth = 1;
        let mut body_end = self.len();
        while k < self.len() && depth > 0 {
            if self.starts(k, "\\begin") {
                depth += 1;
                k += 6;
                continue;
            }
            if self.starts(k, "\\end") {
                depth -= 1;
                if depth == 0 {
                    body_end = k;
                }
                k = self.spaces(k + 4, false);
                if self.get(k) == Some('{') {
                    k = self.brace(k).1;
                }
                if depth == 0 {
                    break;
                }
                continue;
            }
            k += 1;
        }
        Some(Environment { env: env.trim().into(), body_start, body_end, end: k })
    }
    fn script(&self, i: usize) -> (String, usize) {
        let p = self.spaces(i + 1, false);
        let end = if self.get(p) == Some('{') {
            self.brace(p).1
        } else if self.get(p) == Some('\\') {
            let (name, j) = self.name(p + 1);
            if name.is_empty() { p + 2 } else { j }
        } else if p < self.len() {
            p + 1
        } else {
            p
        };
        (self.slice(i, end), end)
    }
    fn scripts(&self, start: usize) -> (Option<String>, Option<String>, usize) {
        let mut sub = None;
        let mut sup = None;
        let mut m = start;
        loop {
            let n = self.spaces(m, false);
            if self.get(n) == Some('_') && sub.is_none() {
                let (t, end) = self.arg(n + 1);
                sub = Some(t);
                m = end;
            } else if self.get(n) == Some('^') && sup.is_none() {
                let (t, end) = self.arg(n + 1);
                sup = Some(t);
                m = end;
            } else {
                break;
            }
        }
        (sub, sup, m)
    }
}
struct LeftRight {
    left: String,
    segments: Vec<String>,
    middles: Vec<String>,
    right: String,
    end: usize,
}
struct Environment {
    env: String,
    body_start: usize,
    body_end: usize,
    end: usize,
}
fn arity(name: &str) -> Option<usize> {
    match name {
        "frac" | "dfrac" | "tfrac" | "cfrac" | "binom" | "dbinom" | "tbinom" | "overset" | "underset" | "stackrel" => {
            Some(2)
        }
        "sqrt" | "overbrace" | "underbrace" | "overbracket" | "underbracket" | "overparen" | "underparen" => Some(1),
        _ => None,
    }
}
fn delim_key(t: &str) -> String {
    match t {
        "\\{" | "\\lbrace" => "{".into(),
        "\\}" | "\\rbrace" => "}".into(),
        "\\vert" | "\\lvert" | "\\rvert" => "|".into(),
        "\\|" | "\\Vert" | "\\lVert" | "\\rVert" => "‖".into(),
        "\\langle" | "<" => "⟨".into(),
        "\\rangle" | ">" => "⟩".into(),
        "\\lceil" => "⌈".into(),
        "\\rceil" => "⌉".into(),
        "\\lfloor" => "⌊".into(),
        "\\rfloor" => "⌋".into(),
        "\\lbrack" => "[".into(),
        "\\rbrack" => "]".into(),
        "." => String::new(),
        _ => {
            if t.starts_with('\\') {
                latex_to_unicode(t).trim().into()
            } else {
                t.into()
            }
        }
    }
}
fn split(source: &str, cells: bool, newlines: bool) -> Vec<String> {
    let s = Source::new(source);
    let mut braces = 0;
    let mut envs = 0;
    let mut last = 0;
    let mut i = 0;
    let mut out = vec![];
    while let Some(c) = s.get(i) {
        if s.starts(i, "\\begin") {
            envs += 1;
            i += 6;
            continue;
        }
        if s.starts(i, "\\end") {
            envs -= 1;
            i += 4;
            continue;
        }
        if c == '\\' {
            if !cells && s.get(i + 1) == Some('\\') && braces == 0 && envs == 0 {
                out.push(s.slice(last, i));
                i = s.spaces(i + 2, false);
                if s.get(i) == Some('[') {
                    i = s.bracket_end(i);
                }
                last = i;
                continue;
            }
            i += 2;
            continue;
        }
        if c == '{' {
            braces += 1;
        } else if c == '}' {
            braces -= 1;
        } else if braces == 0
            && envs == 0
            && ((cells && c == '&') || (newlines && c == '\n' && braces_owed(&s.slice(last, i)) == 0))
        {
            out.push(s.slice(last, i));
            last = i + 1;
        }
        i += 1;
    }
    out.push(s.slice(last, s.len()));
    if cells { out.into_iter().map(|s| s.trim().into()).collect() } else { out }
}
fn braces_owed(source: &str) -> usize {
    let s = Source::new(source);
    let mut pending: Vec<usize> = vec![];
    let consume = |p: &mut Vec<usize>| {
        if let Some(last) = p.last_mut() {
            if *last == 1 {
                p.pop();
            } else {
                *last -= 1;
            }
        }
    };
    let mut i = 0;
    while let Some(c) = s.get(i) {
        if c == '\\' {
            let (name, mut j) = s.name(i + 1);
            consume(&mut pending);
            let count = arity(&name).unwrap_or(0);
            let mut attached = 0;
            if !name.is_empty() {
                while matches!(s.get(j), Some('[' | '{')) {
                    if s.get(j) == Some('{') {
                        j = s.brace(j).1;
                        if attached < count {
                            attached += 1;
                        }
                    } else {
                        j = s.bracket_end(j);
                    }
                }
            } else {
                j = i + 2;
            }
            if count > attached {
                pending.push(count - attached);
            }
            i = j;
            continue;
        }
        if c == '{' {
            i = s.brace(i).1;
            consume(&mut pending);
            continue;
        }
        if c == '^' || c == '_' {
            pending.push(1);
            i += 1;
            continue;
        }
        if !matches!(c, ' ' | '\t' | '\n') {
            consume(&mut pending);
        }
        i += 1;
    }
    pending.iter().sum()
}
fn parse_environment(s: &Source, start: usize, ctx: &Context) -> Option<(Block, usize)> {
    let env = s.environment(start)?;
    let starred = env.env.ends_with('*');
    let base = env.env.trim_end_matches('*');
    let delims = match base {
        "matrix" | "smallmatrix" | "array" => Some(("", "")),
        "pmatrix" => Some(("(", ")")),
        "bmatrix" => Some(("[", "]")),
        "Bmatrix" => Some(("{", "}")),
        "vmatrix" => Some(("|", "|")),
        "Vmatrix" => Some(("‖", "‖")),
        "cases" | "dcases" => Some(("{", "")),
        "rcases" | "drcases" => Some(("", "}")),
        _ => None,
    };
    if let Some((l, r)) = delims {
        let mut p = s.spaces(env.body_start, true);
        if starred && s.get(p) == Some('[') {
            let close = s.bracket_end(p);
            if close <= env.body_end {
                p = s.spaces(close, true);
            }
        }
        let mut col_spec = None;
        if base == "array" && s.get(p) == Some('{') {
            let (spec, end) = s.brace(p);
            col_spec = Some(spec.chars().filter(|c| matches!(c, 'l' | 'c' | 'r')).collect::<Vec<_>>());
            p = end;
        }
        let rows = split(&s.slice(p, env.body_end), false, false)
            .into_iter()
            .filter(|r| !r.trim().is_empty())
            .map(|row| split(row.trim(), true, false).iter().map(|c| parse_expr(c, ctx)).collect())
            .collect();
        let cases = matches!(base, "cases" | "dcases" | "rcases" | "drcases");
        let b = grid(
            rows,
            |col| {
                col_spec.as_ref().map(|cs| cs.get(col).copied().unwrap_or('c')).unwrap_or(if cases { 'l' } else { 'c' })
            },
            |_| 2,
            1,
        );
        return Some((delimit(b, l, r), env.end));
    }
    if !matches!(
        base,
        "equation"
            | "eqnarray"
            | "align"
            | "aligned"
            | "alignat"
            | "alignedat"
            | "flalign"
            | "split"
            | "gather"
            | "gathered"
            | "gatheredat"
            | "multline"
            | "displaymath"
            | "math"
    ) {
        return Some((text_box(&ctx.unicode(&ctx.wrap(&s.slice(start, env.end)))), env.end));
    }
    let mut body_start = env.body_start;
    if matches!(base, "alignat" | "alignedat" | "gatheredat") {
        let p = s.spaces(body_start, true);
        if s.get(p) == Some('{') {
            body_start = s.brace(p).1;
        }
    }
    let rows: Vec<_> = split(&s.slice(body_start, env.body_end), false, false)
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    if rows.is_empty() {
        return Some((text_box(""), env.end));
    }
    let cells: Vec<_> = rows.iter().map(|row| split(row, true, false)).collect();
    let ncols = cells.iter().map(Vec::len).max().unwrap_or(0);
    let b = if ncols <= 1 {
        vcat(
            rows.iter().map(|r| parse_expr(r, ctx)).collect(),
            if matches!(base, "gather" | "gathered" | "multline") { 'c' } else { 'l' },
        )
    } else {
        grid(
            cells.iter().map(|r| r.iter().map(|c| parse_expr(c, ctx)).collect()).collect(),
            |col| if col % 2 == 0 { 'r' } else { 'l' },
            |col| if col % 2 == 1 { 1 } else { 3 },
            0,
        )
    };
    Some((b, env.end))
}
fn paint(mut b: Block, fg: Option<&str>) -> Block {
    if let Some(fg) = fg {
        for l in &mut b.lines {
            *l = format!("{fg}{}\x1b[39m", l.replace("\x1b[39m", fg));
        }
    }
    b
}
fn flush(boxes: &mut Vec<Block>, inline: &mut String, color: &str, ctx: &Context) {
    if !inline.is_empty() {
        boxes.push(text_box(&ctx.unicode(&ctx.wrap(&format!("{color}{inline}")))));
        inline.clear();
    }
}
fn script_arg(raw: &str) -> &str {
    let arg = raw[1..].trim_start();
    if arg.starts_with('{') && arg.ends_with('}') { &arg[1..arg.len() - 1] } else { arg }
}
fn limit_operator(name: &str) -> bool {
    matches!(
        name,
        "sum"
            | "prod"
            | "coprod"
            | "bigcup"
            | "bigcap"
            | "bigsqcup"
            | "bigvee"
            | "bigwedge"
            | "bigoplus"
            | "bigotimes"
            | "bigodot"
            | "biguplus"
            | "lim"
            | "limsup"
            | "liminf"
            | "projlim"
            | "injlim"
            | "varlimsup"
            | "varliminf"
            | "varprojlim"
            | "varinjlim"
            | "max"
            | "min"
            | "sup"
            | "inf"
            | "det"
            | "gcd"
            | "Pr"
            | "argmax"
            | "argmin"
    )
}
fn integral_operator(name: &str) -> bool {
    matches!(
        name,
        "int" | "iint" | "iiint" | "iiiint" | "oint" | "oiint" | "oiiint" | "idotsint" | "intop" | "smallint"
    )
}
fn parse_expr(source: &str, ctx: &Context) -> Block {
    let s = Source::new(source);
    let mut boxes: Vec<Block> = vec![];
    let mut inline = String::new();
    let mut color = String::new();
    let mut color_scope: Option<String> = None;
    let mut i = 0;
    while let Some(c) = s.get(i) {
        let child = ctx.prepend(&color);
        if c == '\\' {
            let (name, j) = s.name(i + 1);
            if matches!(name.as_str(), "frac" | "dfrac" | "tfrac" | "cfrac" | "binom" | "dbinom" | "tbinom") {
                flush(&mut boxes, &mut inline, &color, ctx);
                let (a, k) = s.arg(j);
                let (b, end) = s.arg(k);
                let a = parse_expr(&a, &child);
                let b = parse_expr(&b, &child);
                boxes.push(paint(
                    if name.ends_with("frac") { fraction(a, b) } else { binomial(a, b) },
                    color_scope.as_deref(),
                ));
                i = end;
                continue;
            }
            if matches!(
                name.as_str(),
                "overbrace" | "underbrace" | "overbracket" | "underbracket" | "overparen" | "underparen"
            ) {
                flush(&mut boxes, &mut inline, &color, ctx);
                let (a, k) = s.arg(j);
                let (sub, sup, end) = s.scripts(k);
                let over = name.starts_with("over");
                let (label, other) = if over { (sup, sub) } else { (sub, sup) };
                let mut b = hbrace(parse_expr(&a, &child), &name, label.as_deref().map(|t| parse_expr(t, &child)));
                if let Some(other) = other {
                    let other = parse_expr(&other, &child);
                    b = if over { attach(b, Some(other), None) } else { attach(b, None, Some(other)) };
                }
                boxes.push(paint(b, color_scope.as_deref()));
                i = end;
                continue;
            }
            if matches!(name.as_str(), "overset" | "underset" | "stackrel") {
                flush(&mut boxes, &mut inline, &color, ctx);
                let (a, k) = s.arg(j);
                let (b, end) = s.arg(k);
                let annotation = parse_expr(&a, &child);
                let base = parse_expr(&b, &child);
                boxes.push(paint(
                    if name == "underset" {
                        limits(base, Some(annotation), None)
                    } else {
                        limits(base, None, Some(annotation))
                    },
                    color_scope.as_deref(),
                ));
                i = end;
                continue;
            }
            if name == "sqrt" {
                let mut k = s.spaces(j, false);
                let mut degree = None;
                if s.get(k) == Some('[') {
                    let end = s.bracket_end(k);
                    degree =
                        Some(s.slice(k + 1, if s.get(end.saturating_sub(1)) == Some(']') { end - 1 } else { end }));
                    k = end;
                }
                let (arg, end) = s.arg(k);
                flush(&mut boxes, &mut inline, &color, ctx);
                boxes.push(paint(radical(parse_expr(&arg, &child), degree.as_deref()), color_scope.as_deref()));
                i = end;
                continue;
            }
            if name == "left"
                && let Some(lr) = s.left_right(i)
            {
                let segments: Vec<_> = lr.segments.iter().map(|seg| parse_expr(seg, &child)).collect();
                let above = segments.iter().map(|b| b.baseline).max().unwrap_or(0);
                let below = segments.iter().map(|b| b.lines.len() - 1 - b.baseline).max().unwrap_or(0);
                let height = above + below + 1;
                if height == 1 {
                    inline.push_str(&s.slice(i, lr.end));
                    i = lr.end;
                    continue;
                }
                flush(&mut boxes, &mut inline, &color, ctx);
                let mut parts = vec![];
                if let Some(l) = delimiter(&delim_key(&lr.left), height, above) {
                    parts.push(l);
                }
                for (index, seg) in segments.into_iter().enumerate() {
                    parts.push(seg);
                    if let Some(m) = lr.middles.get(index)
                        && let Some(col) = delimiter(&delim_key(m), height, above)
                    {
                        parts.push(col);
                    }
                }
                if let Some(r) = delimiter(&delim_key(&lr.right), height, above) {
                    parts.push(r);
                }
                boxes.push(paint(hcat(parts), color_scope.as_deref()));
                i = lr.end;
                continue;
            }
            if limit_operator(&name) || integral_operator(&name) {
                let mut k = s.spaces(j, false);
                let mut stack = limit_operator(&name);
                let mut resume = j;
                if s.starts(k, "\\limits") && !s.get(k + 7).is_some_and(|c| c.is_ascii_alphabetic()) {
                    stack = true;
                    k += 7;
                    resume = k;
                } else if s.starts(k, "\\nolimits") && !s.get(k + 9).is_some_and(|c| c.is_ascii_alphabetic()) {
                    stack = false;
                    resume = k + 9;
                }
                if stack {
                    let (sub, sup, end) = s.scripts(k);
                    if sub.is_some() || sup.is_some() {
                        flush(&mut boxes, &mut inline, &color, ctx);
                        let glyph = text_box(&ctx.unicode(&ctx.wrap(&format!("{color}\\{name}"))));
                        boxes.push(paint(
                            limits(
                                glyph,
                                sub.as_deref().map(|t| parse_expr(t, &child)),
                                sup.as_deref().map(|t| parse_expr(t, &child)),
                            ),
                            color_scope.as_deref(),
                        ));
                        i = end;
                        continue;
                    }
                }
                inline.push_str(&format!("\\{name}"));
                i = resume;
                continue;
            }
            if name == "color" || name == "normalcolor" {
                flush(&mut boxes, &mut inline, &color, ctx);
                if name == "normalcolor" {
                    color.clear();
                    color_scope = None;
                    i = j;
                    continue;
                }
                let mut k = s.spaces(j, false);
                let mut opt = String::new();
                if s.get(k) == Some('[') {
                    let end = s.bracket_end(k);
                    if s.get(end.saturating_sub(1)) == Some(']') {
                        opt = s.slice(k, end);
                        k = s.spaces(end, false);
                    }
                }
                if s.get(k) == Some('{') {
                    let (spec, end) = s.brace(k);
                    color = format!("\\color{opt}{{{spec}}}");
                    color_scope = latex_color(
                        if opt.is_empty() { None } else { Some(opt[1..opt.len() - 1].trim()) },
                        &spec,
                        ctx.true_color,
                    )
                    .map(|c| c.0);
                    i = end;
                } else {
                    color.clear();
                    color_scope = None;
                    i = k;
                }
                continue;
            }
            if name == "begin"
                && let Some((b, end)) = parse_environment(&s, i, &child)
            {
                flush(&mut boxes, &mut inline, &color, ctx);
                boxes.push(paint(b, color_scope.as_deref()));
                i = end;
                continue;
            }
            if is_math_font_command(&name) || name == "textcolor" {
                let mut k = s.spaces(j, false);
                let mut prefix = format!("\\{name}");
                let mut scope = None;
                if name == "textcolor" {
                    let mut model = None;
                    if s.get(k) == Some('[') {
                        let end = s.bracket_end(k);
                        if s.get(end.saturating_sub(1)) == Some(']') {
                            model = Some(s.slice(k + 1, end - 1).trim().to_owned());
                            prefix.push_str(&s.slice(k, end));
                            k = s.spaces(end, false);
                        }
                    }
                    if s.get(k) != Some('{') {
                        inline.push_str(&format!("\\{name}"));
                        i = j;
                        continue;
                    }
                    let (spec, end) = s.brace(k);
                    prefix.push_str(&format!("{{{spec}}}"));
                    scope = latex_color(model.as_deref(), &spec, ctx.true_color).map(|c| c.0);
                    k = s.spaces(end, false);
                }
                if s.get(k) == Some('{') {
                    let (content, end) = s.brace(k);
                    flush(&mut boxes, &mut inline, &color, ctx);
                    let b = paint(parse_expr(&content, &ctx.scoped(&color, &prefix)), scope.as_deref());
                    boxes.push(paint(b, color_scope.as_deref()));
                    i = end;
                    continue;
                }
            }
            if name.is_empty() {
                inline.push_str(&s.slice(i, j + 1));
                i = j + 1;
                continue;
            }
            inline.push_str(&format!("\\{name}"));
            i = j;
            while matches!(s.get(i), Some('[' | '{')) {
                let end = if s.get(i) == Some('{') { s.brace(i).1 } else { s.bracket_end(i) };
                inline.push_str(&s.slice(i, end));
                i = end;
            }
            continue;
        }
        if c == '^' || c == '_' {
            let (first, first_end) = s.script(i);
            let n = s.spaces(first_end, false);
            let second = if s.get(n) == Some(if c == '^' { '_' } else { '^' }) { Some(s.script(n)) } else { None };
            let end = second.as_ref().map(|s| s.1).unwrap_or(first_end);
            let (sup, sub) = if c == '^' {
                (Some(first.as_str()), second.as_ref().map(|s| s.0.as_str()))
            } else {
                (second.as_ref().map(|s| s.0.as_str()), Some(first.as_str()))
            };
            let sup_box = sup.map(|t| parse_expr(script_arg(t), &child));
            let sub_box = sub.map(|t| parse_expr(script_arg(t), &child));
            let unconvertible = |t: Option<&str>| {
                t.is_some_and(|raw| {
                    let flat = ctx.unicode(raw);
                    flat.starts_with('^') || flat.starts_with('_')
                })
            };
            let ragged = |t: Option<&str>| {
                t.is_some_and(|raw| {
                    regex::Regex::new(r"\\[A-Za-z]+")
                        .unwrap()
                        .replace_all(script_arg(raw), "")
                        .chars()
                        .filter(|c| c.is_ascii_alphabetic())
                        .count()
                        >= 2
                })
            };
            let tall = sup_box.as_ref().is_some_and(|b| b.lines.len() > 1)
                || sub_box.as_ref().is_some_and(|b| b.lines.len() > 1);
            if tall || unconvertible(sup) || unconvertible(sub) || ragged(sup) || ragged(sub) {
                flush(&mut boxes, &mut inline, &color, ctx);
                let base = boxes.pop().unwrap_or_else(|| text_box(""));
                boxes.push(paint(attach(base, sub_box, sup_box), color_scope.as_deref()));
                i = end;
                continue;
            }
            if inline.is_empty() && boxes.last().is_some_and(|b| b.lines.len() > 1) {
                let corner = |raw: &str| text_box(&ctx.unicode(&ctx.wrap(&format!("{color}{raw}"))));
                let base = boxes.pop().unwrap();
                boxes.push(paint(attach(base, sub.map(corner), sup.map(corner)), color_scope.as_deref()));
                i = end;
                continue;
            }
            inline.push_str(&s.slice(i, end));
            i = end;
            continue;
        }
        if c == '{' {
            let (group, end) = s.brace(i);
            flush(&mut boxes, &mut inline, &color, ctx);
            boxes.push(paint(parse_expr(&group, &child), color_scope.as_deref()));
            i = end;
            continue;
        }
        if (c == '(' || c == '[')
            && let Some(close) = s.match_delim(i, c, if c == '(' { ')' } else { ']' })
        {
            let inner = parse_expr(&s.slice(i + 1, close), &child);
            if inner.lines.len() > 1 {
                flush(&mut boxes, &mut inline, &color, ctx);
                boxes.push(paint(
                    delimit(inner, &c.to_string(), if c == '(' { ")" } else { "]" }),
                    color_scope.as_deref(),
                ));
                i = close + 1;
                continue;
            }
        }
        inline.push(c);
        i += 1;
    }
    flush(&mut boxes, &mut inline, &color, ctx);
    if boxes.is_empty() { text_box("") } else { hcat(boxes) }
}
pub(super) fn latex_to_block(source: &str) -> Vec<String> {
    latex_to_block_with_color(source, true)
}
pub(super) fn latex_to_block_with_color(source: &str, true_color: bool) -> Vec<String> {
    if source.trim().is_empty() {
        return vec![];
    }
    let re = regex::Regex::new(r"[ \t]*\n[ \t]*").unwrap();
    let rows = split(source.trim(), false, true)
        .iter()
        .map(|row| re.replace_all(row, " ").trim().to_owned())
        .filter(|row| !row.is_empty())
        .map(|row| parse_expr(&row, &Context { true_color, ..Context::default() }))
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return vec![];
    }
    let mut lines = vcat(rows, 'l').lines;
    while lines.len() > 1 && lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    while lines.len() > 1 && lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    lines
}
