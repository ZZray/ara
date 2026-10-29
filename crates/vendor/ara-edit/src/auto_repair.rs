//! Fixed OMP 596f2da edit/auto-repair.ts (MIT; THIRD_PARTY_NOTICES.md).
//! Pure localization/validation. Host owns credentials, model calls and writes.

use ara_ast::summary::{SummaryOptions, summarize_code};
use ara_diff::line_runs_str;
use regex::Regex;
use std::future::Future;
use std::sync::LazyLock;

pub const CONTEXT_LINES: usize = 6;
pub const MAX_REGION_LINES: usize = 150;
pub const MAX_PAIR_SEARCH_HUNKS: usize = 24;
pub const MAX_ATTEMPTS: usize = 2;
pub const COMPLETION_MAX_TOKENS: u64 = 8192;
pub const REPAIR_TIMEOUT_MS: u64 = 20_000;

#[derive(Clone, Debug)]
pub struct AppliedEditSnapshot {
    pub path: String,
    pub prev: String,
    pub next: String,
}

#[derive(Clone, Debug)]
pub struct RepairRegion {
    pub b_start: usize,
    pub b_end: usize,
    pub broken_text: String,
    pub reference_text: String,
    pub language: String,
}

#[derive(Clone, Debug)]
pub struct RegionRepair {
    pub content: String,
    pub region: RepairRegion,
    pub attempts: usize,
}

#[derive(Clone, Copy)]
struct Hunk {
    a_start: usize,
    a_end: usize,
    b_start: usize,
    b_end: usize,
}

fn summary(code: &str, path: &str) -> Option<ara_ast::summary::SummaryResult> {
    summarize_code(SummaryOptions {
        code: if code.is_empty() { "\n".into() } else { code.into() },
        lang: None,
        path: Some(path.into()),
        min_body_lines: None,
        min_comment_lines: None,
        unfold_until_lines: None,
        unfold_limit_lines: None,
    })
    .ok()
}

pub fn parses_source(code: &str, path: &str) -> bool {
    summary(code, path).is_some_and(|result| result.parsed)
}

fn build_hunks(prev: &str, next: &str) -> (Vec<Hunk>, Vec<String>, Vec<String>) {
    let a = prev.split('\n').map(str::to_owned).collect();
    let b = next.split('\n').map(str::to_owned).collect();
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut ai, mut bi) = (0, 0);
    for run in line_runs_str(prev, next) {
        let count = run.count as usize;
        if !run.added && !run.removed {
            ai += count;
            bi += count;
            continue;
        }
        let del = if run.removed { count } else { 0 };
        let add = if run.added { count } else { 0 };
        if let Some(last) = hunks.last_mut().filter(|last| last.a_end == ai && last.b_end == bi) {
            last.a_end += del;
            last.b_end += add;
        } else {
            hunks.push(Hunk { a_start: ai, a_end: ai + del, b_start: bi, b_end: bi + add });
        }
        ai += del;
        bi += add;
    }
    (hunks, a, b)
}

fn revert_hunks(a: &[String], b: &[String], hunks: &[Hunk], set: &[usize], out: &mut Vec<String>) -> String {
    // All trials are already ascending; retain the scratch allocation across
    // single/pair/greedy parse attempts (target isolateCulpritHunks).
    out.clear();
    let mut bi = 0;
    for &i in set {
        let h = hunks[i];
        out.extend_from_slice(&b[bi..h.b_start]);
        out.extend_from_slice(&a[h.a_start..h.a_end]);
        bi = h.b_end;
    }
    out.extend_from_slice(&b[bi..]);
    out.join("\n")
}

fn culprit_hunks(path: &str, a: &[String], b: &[String], hunks: &[Hunk]) -> Option<Vec<usize>> {
    let n = hunks.len();
    if n == 0 { return None; }
    let mut scratch = Vec::new();
    let mut trial = Vec::with_capacity(n);
    let mut parses = |set: &[usize]| parses_source(&revert_hunks(a, b, hunks, set, &mut scratch), path);
    for i in 0..n {
        if parses(&[i]) { return Some(vec![i]); }
    }
    if n <= MAX_PAIR_SEARCH_HUNKS {
        for i in 0..n {
            for j in i + 1..n {
                if parses(&[i, j]) { return Some(vec![i, j]); }
            }
        }
    }
    let mut keep_bits = vec![0xff_u8; n.div_ceil(8)];
    let mut keep_parses = false;
    for i in 0..n {
        trial.clear();
        trial.extend((0..n).filter(|&k| keep_bits[k >> 3] & (1 << (k & 7)) != 0 && k != i));
        if parses(&trial) {
            keep_bits[i >> 3] &= !(1 << (i & 7));
            keep_parses = true;
        }
    }
    let culprits: Vec<usize> = (0..n).filter(|&k| keep_bits[k >> 3] & (1 << (k & 7)) != 0).collect();
    if culprits.is_empty() || (!keep_parses && !parses(&culprits)) { return None; }
    Some(culprits)
}

fn compute_region(snapshot: &AppliedEditSnapshot) -> Option<(RepairRegion, Vec<String>)> {
    let (hunks, a, b) = build_hunks(&snapshot.prev, &snapshot.next);
    let culprits = culprit_hunks(&snapshot.path, &a, &b, &hunks)?;
    let hs: Vec<Hunk> = culprits.iter().map(|i| hunks[*i]).collect();
    let b_start = hs.first()?.b_start.saturating_sub(CONTEXT_LINES);
    let b_end = (hs.last()?.b_end + CONTEXT_LINES).min(b.len());
    if b_end - b_start > MAX_REGION_LINES {
        return None;
    }
    let mut reference = Vec::new();
    let mut bi = b_start;
    for h in hs {
        reference.extend_from_slice(&b[bi..h.b_start]);
        reference.extend_from_slice(&a[h.a_start..h.a_end]);
        bi = h.b_end;
    }
    reference.extend_from_slice(&b[bi..b_end]);
    Some((RepairRegion {
        b_start,
        b_end,
        broken_text: b[b_start..b_end].join("\n"),
        reference_text: reference.join("\n"),
        language: summary(&snapshot.prev, &snapshot.path).and_then(|r| r.language).unwrap_or_else(|| "source".into()),
    }, b))
}

pub fn compute_repair_region(snapshot: &AppliedEditSnapshot) -> Option<RepairRegion> {
    compute_region(snapshot).map(|(region, _)| region)
}

fn splice_region_lines(b: &[String], region: &RepairRegion, text: &str) -> String {
    b[..region.b_start].iter().map(String::as_str)
        .chain(text.split('\n'))
        .chain(b[region.b_end..].iter().map(String::as_str))
        .collect::<Vec<_>>().join("\n")
}

pub fn splice_region(next: &str, region: &RepairRegion, text: &str) -> String {
    let b: Vec<&str> = next.split('\n').collect();
    b[..region.b_start]
        .iter()
        .copied()
        .chain(text.split('\n'))
        .chain(b[region.b_end..].iter().copied())
        .collect::<Vec<_>>()
        .join("\n")
}

// ECMAScript trim/\s differs from Rust's Unicode White_Space (notably BOM/NEL).
fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}
fn js_trim(text: &str) -> &str {
    text.trim_matches(js_space)
}
fn normalized(text: &str) -> String {
    let mut out = String::new();
    let mut spaced = false;
    for c in js_trim(text).chars() {
        if js_space(c) {
            spaced = true;
        } else {
            if spaced {
                out.push(' ');
            }
            out.push(c);
            spaced = false;
        }
    }
    out
}
fn realign(src: &[&str], candidate: &str) -> String {
    let out: Vec<&str> = candidate.split('\n').collect();
    let s = src.iter().map(|l| js_trim(l)).collect::<Vec<_>>().join("\n");
    let o = out.iter().map(|l| js_trim(l)).collect::<Vec<_>>().join("\n");
    let mut merged = Vec::new();
    let (mut si, mut oi) = (0, 0);
    for run in line_runs_str(&s, &o) {
        let count = run.count as usize;
        if !run.added && !run.removed {
            merged.extend_from_slice(&src[si..si + count]);
            si += count;
            oi += count;
        } else if run.removed {
            si += count;
        } else {
            merged.extend_from_slice(&out[oi..oi + count]);
            oi += count;
        }
    }
    merged.join("\n")
}
fn strip_fence(text: &str) -> String {
    static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)^```[^\n]*\n(.*?)\n?```$").expect("fixed fence"));
    let trimmed = js_trim(text);
    FENCE.captures(trimmed).map_or_else(|| trimmed.into(), |c| c[1].into())
}

/// Render only tokens present in the fixed template; inserted source is never
/// scanned again, so source text containing {{after}} remains byte-for-byte.
pub fn render_prompt(region: &RepairRegion, previous: Option<&str>) -> String {
    const TEMPLATE: &str = include_str!("../prompts/auto-repair.md");
    let mut out = String::new();
    let mut rest = TEMPLATE;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        if rest.starts_with("{{#if previousAttempt}}") {
            let end = rest.find("{{/if}}").expect("fixed prompt conditional");
            let body = &rest["{{#if previousAttempt}}".len()..end];
            if previous.is_some_and(|p| !p.is_empty()) {
                render_tokens(body, region, previous, &mut out);
            }
            rest = &rest[end + "{{/if}}".len()..];
        } else {
            let end = rest.find("}}").expect("fixed prompt token") + 2;
            render_tokens(&rest[..end], region, previous, &mut out);
            rest = &rest[end..];
        }
    }
    out.push_str(rest);
    out
}
fn render_tokens(mut text: &str, r: &RepairRegion, p: Option<&str>, out: &mut String) {
    while let Some(start) = text.find("{{") {
        out.push_str(&text[..start]);
        let end = text[start..].find("}}").expect("fixed token") + start + 2;
        out.push_str(match &text[start..end] {
            "{{lang}}" => &r.language,
            "{{before}}" => &r.reference_text,
            "{{after}}" => &r.broken_text,
            "{{previousAttempt}}" => p.unwrap_or(""),
            other => other,
        });
        text = &text[end..];
    }
    out.push_str(text);
}

pub async fn repair_parse_regression<F, Fut>(
    snapshot: &AppliedEditSnapshot,
    mut complete: F,
) -> Result<Option<RegionRepair>, String>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    let Some((region, post_lines)) = compute_region(snapshot) else {
        return Ok(None);
    };
    let reference = normalized(&region.reference_text);
    let broken: Vec<&str> = region.broken_text.split('\n').collect();
    let before: Vec<&str> = region.reference_text.split('\n').collect();
    let mut previous = None;
    for attempt in 1..=MAX_ATTEMPTS {
        let candidate = strip_fence(&complete(render_prompt(&region, previous.as_deref())).await?);
        previous = Some(candidate.clone());
        let variants = [realign(&broken, &candidate), realign(&before, &candidate), candidate];
        for (i, text) in variants.iter().enumerate() {
            if variants[..i].contains(text) || normalized(text) == reference {
                continue;
            }
            let content = splice_region_lines(&post_lines, &region, text);
            if parses_source(&content, &snapshot.path) {
                return Ok(Some(RegionRepair { content, region, attempts: attempt }));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn base() -> String {
        let mut lines =
            vec!["export function alpha(a: number): number {".into(), "\treturn a + 1;".into(), "}".into(), "".into()];
        lines.extend((0..10).map(|i| format!("export const pad{i} = {i};")));
        lines.extend(
            ["", "export function beta(b: number): number {", "\tconst doubled = b * 2;", "\treturn doubled;", "}", ""]
                .map(str::to_owned),
        );
        lines.extend((0..10).map(|i| format!("export const tail{i} = {i};")));
        lines.extend(["", "export function gamma(c: number): number {", "\treturn c - 1;", "}"].map(str::to_owned));
        lines.join("\n")
    }
    fn after(prompt: &str) -> String {
        prompt.split("AFTER (broken):").nth(1).unwrap().split("```").nth(1).unwrap().trim().into()
    }
    #[tokio::test]
    async fn fixed_repair_family_seven_original_rows() {
        let path = "/repo/src/sample.ts";
        let prev = base();
        let broken = prev.replace("const doubled = b * 2;", "const doubled = (b * 2;");
        // B-b581a0a430: isolate one culprit while retaining a distant valid hunk.
        let multi = AppliedEditSnapshot {
            path: path.into(),
            prev: prev.clone(),
            next: broken.replace("return a + 1;", "return a + 2;"),
        };
        let r = compute_repair_region(&multi).unwrap();
        assert!(!r.broken_text.contains("return a + 2;"));
        assert_eq!(r.language, "typescript");
        let spliced = splice_region(&multi.next, &r, &r.reference_text);
        assert!(parses_source(&spliced, path));
        assert!(spliced.contains("return a + 2;"));
        // B-a7153bab56: cap applies to the culprit region, including context.
        let body = (0..200).map(|i| format!("\tconst v{i} = {i};")).collect::<Vec<_>>().join("\n");
        let big = AppliedEditSnapshot {
            path: path.into(),
            prev: format!("export function big(): void {{\n{body}\n}}\n"),
            next: format!("export function big(: void {{\n{}\n}}\n", body.replace("const", "let")),
        };
        assert!(parses_source(&big.prev, path));
        assert!(!parses_source(&big.next, path));
        assert!(compute_repair_region(&big).is_none());
        let s = AppliedEditSnapshot { path: path.into(), prev, next: broken };
        // B-e979f1d435: preserve intent, parse whole splice.
        let repaired = repair_parse_regression(&s, |p| async move { Ok(after(&p).replace("(b * 2;", "(b * 2);")) })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(repaired.attempts, 1);
        assert!(parses_source(&repaired.content, path));
        assert!(repaired.content.contains("const doubled = (b * 2);"));
        // B-a85ba92bb8: whitespace-normalized reverts consume two attempts.
        let mut calls = 0;
        let revert = repair_parse_regression(&s, |p| {
            calls += 1;
            async move {
                Ok(p.split("BEFORE (valid typescript):").nth(1).unwrap().split("```").nth(1).unwrap().trim().into())
            }
        })
        .await
        .unwrap();
        assert!(revert.is_none());
        assert_eq!(calls, 2);
        // B-c4e390fd5e: echoed context indentation is restored first.
        let aligned = repair_parse_regression(&s, |p| async move {
            Ok(after(&p).replace("(b * 2;", "(b * 2);").lines().map(str::trim_start).collect::<Vec<_>>().join("\n"))
        })
        .await
        .unwrap()
        .unwrap();
        assert!(aligned.content.contains("\treturn doubled;"));
        assert!(aligned.content.contains("\treturn a + 1;"));
        // B-d5eb2c61f1 and B-f3d0648784: one feedback retry, then stop.
        let mut calls = 0;
        let retry = repair_parse_regression(&s, |p| {
            calls += 1;
            let c = calls;
            async move {
                if c == 1 {
                    Ok("const doubled = (b * 2;".into())
                } else {
                    assert!(p.contains("PREVIOUS ATTEMPT (rejected):"));
                    Ok(after(&p).replace("(b * 2;", "(b * 2);"))
                }
            }
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(retry.attempts, 2);
        let mut calls = 0;
        assert!(
            repair_parse_regression(&s, |_| {
                calls += 1;
                async { Ok("const doubled = (b * 2; // still broken".into()) }
            })
            .await
            .unwrap()
            .is_none()
        );
        assert_eq!(calls, 2);
    }
}

#[cfg(test)]
mod upgraded_region_rows {
    use super::*;
    #[test]
    fn target_isolates_two_broken_hunks_among_fifteen_sixteen_thirty() {
        for count in [15, 16, 30] {
            let source = |broken: bool| (0..count).map(|i| {
                let body = if !broken { format!("return compute({i});") }
                    else if i == 4 || i == 11 { format!("return computeNext({i};") }
                    else { format!("return computeNext({i});") };
                format!("function f{i}() {{\n\t{body}\n}}\n\nconst pad{i} = {i};\n")
            }).collect::<Vec<_>>().join("\n");
            let snapshot = AppliedEditSnapshot {path:"edited.ts".into(), prev:source(false),next:source(true)};
            let region = compute_repair_region(&snapshot).expect("target two-culprit region");
            assert!(region.broken_text.contains("computeNext(4;"));
            assert!(region.broken_text.contains("computeNext(11;"));
            assert!(region.reference_text.contains("return compute(4);"));
            assert!(region.reference_text.contains("return compute(11);"));
            assert!(parses_source(&splice_region(&snapshot.next, &region, &region.reference_text), &snapshot.path));
        }
    }
}
