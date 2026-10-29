//! Fixed OMP read-summary.ts / read-format.ts producer at 596f2da7101178214aa27a753529d15e6b7ad91d.
//! MIT / Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! The tree-sitter/BFS algorithm is reused from ara_ast unchanged.
//! Private cache digest intentionally reuses ara_edit XXH64 instead of Bun Wyhash.

use crate::ToolContext;
use ara_agent::{ToolError, ToolOutput};
use ara_ast::summary::{SummaryOptions, SummaryResult};
use serde_json::json;
use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

pub const MAX_SUMMARY_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_SUMMARY_LINES: usize = 20_000;
pub const SUMMARY_CACHE_MAX: usize = 48;

/// Immutable Host settings snapshot; the fixed schema permits JS numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SummaryPolicy {
    pub enabled: bool,
    /// Invalid native numeric config causes Summary failure after fresh acquisition.
    pub native_options_valid: bool,
    pub prose: bool,
    pub min_body_lines: Option<f64>,
    pub min_comment_lines: Option<f64>,
    pub min_total_lines: f64,
    pub unfold_until: Option<f64>,
    pub unfold_limit: Option<f64>,
}
impl Default for SummaryPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            native_options_valid: true,
            prose: false,
            min_body_lines: Some(4.0),
            min_comment_lines: Some(6.0),
            min_total_lines: 100.0,
            unfold_until: Some(50.0),
            unfold_limit: Some(100.0),
        }
    }
}
impl SummaryPolicy {
    pub fn permits(&self, path: &Path, no_selector: bool) -> bool {
        no_selector && self.enabled && (self.prose || !is_prose_summary_path(path))
    }
    fn fold_key(&self) -> [Option<u64>; 4] {
        [
            self.min_body_lines,
            self.min_comment_lines,
            self.unfold_until,
            self.unfold_limit,
        ]
        .map(|value| {
            value.map(|value| {
                if value == 0.0 {
                    0
                } else if value.is_nan() {
                    f64::NAN.to_bits()
                } else {
                    value.to_bits()
                }
            })
        })
    }
}

pub fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["md", "markdown", "mdx", "mdc", "mkd", "mdown"]
                .iter()
                .any(|candidate| ext.eq_ignore_ascii_case(candidate))
        })
}
pub fn is_prose_summary_path(path: &Path) -> bool {
    is_markdown_path(path)
        || path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
}
pub fn count_text_lines(source: &str) -> usize {
    if source.is_empty() {
        0
    } else {
        source.bytes().filter(|byte| *byte == b'\n').count() + 1
    }
}
/// Fixed napi3.12.2 u32 binding calls napi_get_value_uint32 (ToUint32).
/// Preserve number settings at the Host boundary; convert only at native entry.
fn native_u32(value: f64) -> u32 {
    if !value.is_finite() || value == 0.0 {
        0
    } else {
        value.trunc().rem_euclid(4_294_967_296.0) as u32
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CacheKey {
    path: String,
    hash: u64,
    fold: [Option<u64>; 4],
}
#[derive(Default, Debug)]
pub struct SummaryCache {
    // Most recent at back. None is false memo and retains no source/segments.
    entries: Mutex<VecDeque<(CacheKey, Option<Arc<SummaryResult>>)>>,
}
impl SummaryCache {
    fn summarize(
        &self,
        path: &Path,
        code: String,
        policy: SummaryPolicy,
    ) -> Option<Arc<SummaryResult>> {
        let lines = count_text_lines(&code);
        if !policy.native_options_valid {return None;}
        if lines > MAX_SUMMARY_LINES || (lines as f64) < policy.min_total_lines {
            return None;
        }
        let key = CacheKey {
            path: path.to_string_lossy().into_owned(),
            hash: ara_edit::store::payload_hash(&code),
            fold: policy.fold_key(),
        };
        {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(index) = entries.iter().position(|(candidate, _)| candidate == &key) {
                let entry = entries.remove(index).expect("located summary cache entry");
                let result = entry.1.clone();
                entries.push_back(entry);
                return result;
            }
        }
        // No Session cache lock while parsing; concurrent misses may parse twice.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ara_ast::summary::summarize_code(SummaryOptions {
                code,
                lang: None,
                path: Some(key.path.clone()),
                min_body_lines: policy.min_body_lines.map(native_u32),
                min_comment_lines: policy.min_comment_lines.map(native_u32),
                unfold_until_lines: policy.unfold_until.map(native_u32),
                unfold_limit_lines: policy.unfold_limit.map(native_u32),
            })
        }))
        .ok()?
        .ok()?;
        let usable = (result.parsed && result.elided).then(|| Arc::new(result));
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(index) = entries.iter().position(|(candidate, _)| candidate == &key) {
            entries.remove(index);
        }
        entries.push_back((key, usable.clone()));
        while entries.len() > SUMMARY_CACHE_MAX {
            entries.pop_front();
        }
        usable
    }
}

/// Size gate before ACP; fresh full bridge text wins, bridge failure uses disk.
/// Caller may reuse its already-read BOM-stripped disk source. A failed attempt
/// does not donate ACP text to ordinary READ: that route acquires ACP again.
pub async fn try_summarize(
    ctx: &ToolContext,
    path: &Path,
    file_size: u64,
    cancel: &CancellationToken,
    disk_text: Option<String>,
) -> Option<Arc<SummaryResult>> {
    if file_size > MAX_SUMMARY_BYTES || cancel.is_cancelled() {
        return None;
    }
    let bridge = ctx
        .edit_host()
        .and_then(|host| host.client_bridge())
        .filter(|bridge| bridge.can_read_text_file());
    let code = match bridge {
        Some(bridge) => match bridge.read_text_file(path).await {
            Ok(code) => code,
            Err(_) => match disk_text {
                Some(code) => code,
                None => tokio::fs::read_to_string(path).await.ok()?,
            },
        },
        None => match disk_text {
            Some(code) => code,
            None => tokio::fs::read_to_string(path).await.ok()?,
        },
    };
    if cancel.is_cancelled() {
        return None;
    }
    let policy = ctx.read_summary_policy();
    let cache = ctx.read_summary_cache();
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || cache.summarize(&path, code, policy))
        .await
        .ok()
        .flatten()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElidedRange {
    pub start: u32,
    pub end: u32,
}
#[derive(Debug, PartialEq, Eq)]
pub struct RenderedSummary {
    pub text: String,
    pub display_text: String,
    pub elided_ranges: Vec<ElidedRange>,
    pub elided_lines: u32,
}
enum Unit {
    Line {
        line: u32,
        text: String,
    },
    Elided {
        start: u32,
        end: u32,
    },
    Merged {
        start: u32,
        end: u32,
        head: String,
        tail: String,
    },
}
fn can_merge(head: &str, tail: &str) -> bool {
    let head = ara_prompt::js::trim_end(head);
    let tail = ara_prompt::js::trim(tail);
    let closer = match head.chars().last() {
        Some('{') => '}',
        Some('(') => ')',
        Some('[') => ']',
        _ => return false,
    };
    let mut chars = tail.chars();
    chars.next() == Some(closer) && chars.all(|value| matches!(value, ';' | ',' | ')' | ']' | '}'))
}
pub fn render_summary(ctx: &ToolContext, summary: &SummaryResult) -> RenderedSummary {
    let hashes = ctx.hashlines();
    let numbers = !hashes && ctx.line_numbers;
    let mut raw = Vec::new();
    for segment in &summary.segments {
        if segment.kind == "elided" {
            raw.push(Unit::Elided {
                start: segment.start_line,
                end: segment.end_line,
            });
            continue;
        }
        let text = segment.text.as_deref().unwrap_or("");
        if text.is_empty() {
            continue;
        }
        for (offset, text) in text.split('\n').enumerate() {
            raw.push(Unit::Line {
                line: segment.start_line + offset as u32,
                text: text.into(),
            });
        }
    }
    let mut units = Vec::new();
    let mut raw = raw.into_iter().peekable();
    while let Some(unit) = raw.next() {
        if matches!(unit, Unit::Elided { .. }) {
            let merge = match (units.last(), raw.peek()) {
                (Some(Unit::Line { text: head, .. }), Some(Unit::Line { text: tail, .. })) => {
                    can_merge(head, tail)
                }
                _ => false,
            };
            if merge {
                let Some(Unit::Line {
                    line: start,
                    text: head,
                }) = units.pop()
                else {
                    unreachable!()
                };
                let Some(Unit::Line {
                    line: end,
                    text: tail,
                }) = raw.next()
                else {
                    unreachable!()
                };
                units.push(Unit::Merged {
                    start,
                    end,
                    head,
                    tail,
                });
                continue;
            }
        }
        units.push(unit);
    }
    let mut model = Vec::new();
    let mut display = Vec::new();
    let mut ranges = Vec::new();
    let mut elided_lines = 0;
    for unit in units {
        match unit {
            Unit::Line { line, text } => {
                model.push(if hashes {
                    ara_edit::modes::hashline::format::format_numbered_line(line, &text)
                } else if numbers {
                    format!("{line}|{text}")
                } else {
                    text.clone()
                });
                display.push(text);
            }
            Unit::Elided { start, end } => {
                model.push("…".into());
                display.push("…".into());
                ranges.push(ElidedRange { start, end });
                elided_lines += end - start + 1;
            }
            Unit::Merged {
                start,
                end,
                head,
                tail,
            } => {
                let merged = format!(
                    "{} … {}",
                    ara_prompt::js::trim_end(&head),
                    ara_prompt::js::trim(&tail)
                );
                model.push(if hashes {
                    format!("{start}-{end}:{merged}")
                } else if numbers {
                    format!("{start}-{end}|{merged}")
                } else {
                    merged.clone()
                });
                display.push(merged);
                ranges.push(ElidedRange { start, end });
                elided_lines += end.saturating_sub(start).saturating_sub(1);
            }
        }
    }
    RenderedSummary {
        text: model.join("\n"),
        display_text: display.join("\n"),
        elided_ranges: ranges,
        elided_lines,
    }
}
pub fn elision_footer(path: &str, ranges: &[ElidedRange], elided_lines: u32) -> String {
    if ranges.is_empty() {
        return String::new();
    }
    let selected = ranges
        .iter()
        .take(2)
        .map(|range| format!("{}-{}", range.start, range.end))
        .collect::<Vec<_>>()
        .join(",");
    let tail = if ranges.len() > 2 { ", e.g." } else { " with" };
    format!("[…{elided_lines}ln elided; re-read needed ranges{tail} {path}:{selected}]")
}

/// Snapshot deliberately uses DISK normalized source, even if summary used ACP.
/// Caller retains suffix-resolution notice/details and source-path metadata.
pub fn summary_output(
    ctx: &ToolContext,
    path: &Path,
    display_path: &str,
    summary: &SummaryResult,
    disk_normalized_text: Option<&str>,
) -> Result<ToolOutput, ToolError> {
    let rendered = render_summary(ctx, summary);
    let footer = elision_footer(display_path, &rendered.elided_ranges, rendered.elided_lines);
    let mut text = if footer.is_empty() {
        rendered.text.clone()
    } else {
        format!("{}\n\n{footer}", rendered.text)
    };
    if ctx.hashlines() {
        let disk = disk_normalized_text
            .ok_or_else(|| ToolError("READ Summary requires the disk hashline source".into()))?;
        let key = ara_edit::path_policy::canonical_key(path);
        let tag = ctx.edit_store.record(&key, disk, None);
        let seen = ara_edit::store::seen_lines_from_body(&rendered.text);
        if !seen.is_empty() {
            ctx.edit_store.record_seen_lines(&key, &tag, &seen);
        }
        let header = ara_edit::modes::hashline::format::format_hashline_header(
            &ctx.hashline_display(path),
            &tag,
        );
        text = format!("{header}\n{text}");
    }
    let mut details = json!({"resolvedPath":path.to_string_lossy(),"displayContent":{"text":rendered.display_text,"startLine":1},
        "summary":{"lines":count_text_lines(&rendered.text),"elidedSpans":rendered.elided_ranges.len(),"elidedLines":rendered.elided_lines}});
    if ctx.read_render_markdown() && is_markdown_path(path) {
        details["contentType"] = json!("text/markdown");
    }
    Ok(ToolOutput::text(text).with_details(details))
}

#[cfg(test)]
mod production_tests {
    use super::*;
    fn policy() -> SummaryPolicy {
        SummaryPolicy {
            min_total_lines: 0.0,
            unfold_until: Some(0.0),
            unfold_limit: Some(0.0),
            ..Default::default()
        }
    }
    const CODE: &str = "export function alpha(): string {\n\tconst clean = 'alpha';\n\tconst other = clean.trim();\n\treturn other;\n}\n";

    #[test]
    fn summary_producer_settings_numberdomain_and_fresh_lru_false_memo() {
        assert_eq!(
            SummaryPolicy::default(),
            SummaryPolicy {
                enabled: true,
            native_options_valid: true,
                prose: false,
                min_body_lines: Some(4.0),
                min_comment_lines: Some(6.0),
                min_total_lines: 100.0,
                unfold_until: Some(50.0),
                unfold_limit: Some(100.0)
            }
        );
        for (input, expected) in [
            (4.9, 4),
            (-1.0, u32::MAX),
            (4_294_967_297.9, 1),
            (-4_294_967_297.9, u32::MAX),
            (f64::INFINITY, 0),
            (f64::NAN, 0),
            (-0.0, 0),
        ] {
            assert_eq!(native_u32(input), expected);
        }
        assert_eq!(count_text_lines(""), 0);
        assert_eq!(count_text_lines("x\n"), 2);
        for extension in ["md", "markdown", "mdx", "mdc", "mkd", "mdown", "TXT"] {
            assert!(!SummaryPolicy::default().permits(Path::new(&format!("f.{extension}")), true));
            assert!(
                SummaryPolicy {
                    prose: true,
                    ..Default::default()
                }
                .permits(Path::new(&format!("f.{extension}")), true)
            );
        }
        assert!(!policy().permits(Path::new("f.ts"), false));
        let owner = ToolContext::new(Path::new("."));
        let rebound = owner.clone().with_read_summary_policy(policy());
        assert!(Arc::ptr_eq(&owner.read_summary_cache(),&rebound.read_summary_cache()));
        assert!(!Arc::ptr_eq(&owner.read_summary_cache(),&ToolContext::new(Path::new(".")).read_summary_cache()));
        let nullable = SummaryPolicy {min_body_lines:None,min_comment_lines:None,unfold_until:None,unfold_limit:None,..policy()};
        assert_ne!(nullable.fold_key(),policy().fold_key());
        let cache = SummaryCache::default();
        let first = cache
            .summarize(Path::new("first.ts"), CODE.into(), policy())
            .unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &cache
                .summarize(Path::new("first.ts"), CODE.into(), policy())
                .unwrap()
        ));
        // Fresh bytes, path and ORIGINAL four fold values independently change keys.
        assert!(!Arc::ptr_eq(
            &first,
            &cache
                .summarize(
                    Path::new("first.ts"),
                    CODE.replace("alpha", "beta"),
                    policy()
                )
                .unwrap()
        ));
        assert!(!Arc::ptr_eq(
            &first,
            &cache
                .summarize(Path::new("other.ts"), CODE.into(), policy())
                .unwrap()
        ));
        let changed = SummaryPolicy {
            min_body_lines: Some(4.1),
            ..policy()
        };
        assert!(!Arc::ptr_eq(
            &first,
            &cache
                .summarize(Path::new("first.ts"), CODE.into(), changed)
                .unwrap()
        ));
        let gate = SummaryPolicy {
            min_total_lines: 6.1,
            ..policy()
        };
        assert!(
            cache
                .summarize(Path::new("first.ts"), CODE.into(), gate)
                .is_none()
        );
        // Gate-only settings are not part of the native parse memo key.
        assert!(Arc::ptr_eq(
            &first,
            &cache
                .summarize(Path::new("first.ts"), CODE.into(), policy())
                .unwrap()
        ));
        assert!(
            cache
                .summarize(
                    Path::new("bad.ts"),
                    "export function broken( {\n".into(),
                    policy()
                )
                .is_none()
        );
        assert!(
            cache
                .summarize(
                    Path::new("flat.ts"),
                    "export const x = 1;\n".into(),
                    policy()
                )
                .is_none()
        );
        {
            let entries = cache.entries.lock().unwrap();
            assert!(
                entries
                    .iter()
                    .filter(|(key, _)| key.path == "bad.ts" || key.path == "flat.ts")
                    .all(|(_, memo)| memo.is_none())
            );
        }
        for index in 0..SUMMARY_CACHE_MAX {
            cache
                .summarize(
                    Path::new(&format!("evict{index}.ts")),
                    CODE.into(),
                    policy(),
                )
                .unwrap();
        }
        assert_eq!(cache.entries.lock().unwrap().len(), SUMMARY_CACHE_MAX);
        assert!(!Arc::ptr_eq(
            &first,
            &cache
                .summarize(Path::new("first.ts"), CODE.into(), policy())
                .unwrap()
        ));
        // A gated oversized source never changes the cache or retains its text.
        let old = cache.entries.lock().unwrap().len();
        assert!(
            cache
                .summarize(
                    Path::new("huge.ts"),
                    "\n".repeat(MAX_SUMMARY_LINES),
                    policy()
                )
                .is_none()
        );
        assert_eq!(cache.entries.lock().unwrap().len(), old);
        let zero = policy().fold_key();
        assert_eq!(
            zero,
            SummaryPolicy {
                unfold_until: Some(-0.0),
                ..policy()
            }
            .fold_key()
        );
    }

    #[test]
    fn summary_producer_modes_footer_and_disk_snapshot_seen_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.ts");
        let ctx = ToolContext::new(dir.path());
        let summary = ctx
            .read_summary_cache()
            .summarize(&path, CODE.into(), policy())
            .unwrap();
        let rendered = render_summary(&ctx, &summary);
        assert!(
            rendered
                .text
                .contains("1-5:export function alpha(): string { … }")
        );
        assert_eq!(rendered.elided_ranges, [ElidedRange { start: 1, end: 5 }]);
        assert_eq!(rendered.elided_lines, 3);
        let disk = CODE.replace("alpha", "disk");
        let output = summary_output(&ctx, &path, "fixture.ts", &summary, Some(&disk)).unwrap();
        let body = match &output.content[0] {
            ara_ai::UserBlock::Text(block) => &block.text,
            _ => panic!("text output required"),
        };
        let tag = ara_edit::store::file_hash(&disk);
        assert!(body.starts_with(&format!("[fixture.ts#{tag}]\n")));
        assert!(body.contains("alpha"));
        let key = ara_edit::path_policy::canonical_key(&path);
        let snapshot = ctx.edit_store.by_hash(&key, &tag).unwrap();
        assert_eq!(&*snapshot.text, disk);
        assert_eq!(
            snapshot.seen_lines.unwrap().iter().copied().collect::<Vec<_>>(),
            [1, 5]
        );
        assert_eq!(
            output.details.as_ref().unwrap()["displayContent"]["startLine"],
            1
        );
        assert!(output.details.as_ref().unwrap().get("truncation").is_none());
        let mut numbered = ctx.clone().with_edit(ara_edit::EditMode::Hashline, false);
        numbered.line_numbers = true;
        assert!(
            render_summary(&numbered, &summary)
                .text
                .contains("1-5|export function")
        );
        numbered.line_numbers = false;
        assert!(
            render_summary(&numbered, &summary)
                .text
                .starts_with("export function")
        );
        let ranges = [
            ElidedRange { start: 1, end: 5 },
            ElidedRange { start: 7, end: 11 },
            ElidedRange { start: 13, end: 17 },
        ];
        assert_eq!(
            elision_footer("src/f.ts", &ranges, 9),
            "[…9ln elided; re-read needed ranges, e.g. src/f.ts:1-5,7-11]"
        );
        assert_eq!(
            elision_footer("src/f.ts", &ranges[..2], 6),
            "[…6ln elided; re-read needed ranges with src/f.ts:1-5,7-11]"
        );
        assert_eq!(elision_footer("f.ts", &[], 0), "");
        assert!(can_merge("object {\u{feff}", "\u{feff}});\u{feff}"));
        assert!(!can_merge("object {", "} // trailing comment"));
        assert!(!can_merge("def f():", "return value"));
    }
}
