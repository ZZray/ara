//! Fixed OMP EDIT/apply_patch renderer production branches.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::{
    diff,
    hyperlink::file_hyperlink,
    output_block::{self, State},
    render_utils::*,
    theme::{Theme, language_from_path},
    types::*,
};
use ara_rpc::{WireString, WireValue};
use std::{
    collections::HashSet,
    sync::{Arc, OnceLock},
};
const MISSING_END: &str = "The last line of the patch must be '*** End Patch'";
#[derive(Clone, Default)]
pub(crate) struct InputEntry {
    pub(crate) path: String,
    pub(crate) op: Option<Operation>,
    pub(crate) rename: Option<String>,
    pub(crate) line_edits: bool,
}
#[derive(Clone, Default)]
pub(crate) struct CallFacts {
    pub(crate) path: String,
    pub(crate) rename: Option<String>,
    pub(crate) op: Option<Operation>,
    pub(crate) file_count: usize,
    pub(crate) apply_error: Option<String>,
    pub(crate) hashline_line_edits: bool,
}
fn string(value: &Option<WireString>) -> Option<String> {
    value.as_ref().map(sanitize)
}
fn nonempty(value: &Option<WireString>) -> Option<String> {
    string(value).filter(|s| !s.is_empty())
}
fn normalize_hash_path(value: &str) -> String {
    let value = value.trim();
    let mut end = value.len();
    if value.len() >= 5
        && value.as_bytes()[value.len() - 5] == b'#'
        && value[value.len() - 4..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        end -= 5;
    }
    let value = &value[..end];
    if value.len() >= 2
        && matches!(value.as_bytes()[0], b'\'' | b'"')
        && value.as_bytes()[0] == value.as_bytes()[value.len() - 1]
    {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}
fn hashline_entries(input: &str) -> Vec<InputEntry> {
    let mut entries: Vec<InputEntry> = Vec::new();
    for line in input.trim_start_matches('\u{feff}').split('\n') {
        let line = line.trim_end();
        if let Some(body) = line.strip_prefix('[') {
            let body = body.strip_suffix(']').unwrap_or(body);
            let path = normalize_hash_path(body);
            if !path.is_empty() {
                entries.push(InputEntry { path, ..Default::default() });
            }
            continue;
        }
        let Some(current) = entries.last_mut() else {
            continue;
        };
        let line = line.trim();
        if line == "REM" {
            current.op = Some(Operation::Delete);
        } else if let Some(path) = line.strip_prefix("MV ") {
            current.rename = Some(normalize_hash_path(path));
        } else if line
            .strip_prefix("PUT")
            .or_else(|| line.strip_prefix("CUT"))
            .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(char::is_whitespace))
        {
            current.line_edits = true;
        }
    }
    entries
}
fn partial_json_path(partial: &Option<WireString>) -> Option<String> {
    let raw = partial.as_ref()?.units();
    let key = "\"path\"".encode_utf16().collect::<Vec<_>>();
    let begin = raw.windows(key.len()).position(|w| w == key)? + key.len();
    let mut at = begin;
    while raw.get(at).is_some_and(|u| char::from_u32(*u as u32).is_some_and(char::is_whitespace)) {
        at += 1;
    }
    if raw.get(at) != Some(&58) {
        return None;
    }
    at += 1;
    while raw.get(at).is_some_and(|u| char::from_u32(*u as u32).is_some_and(char::is_whitespace)) {
        at += 1;
    }
    if raw.get(at) != Some(&34) {
        return None;
    }
    at += 1;
    let mut fragment = Vec::new();
    while let Some(&unit) = raw.get(at) {
        if unit == 34 {
            break;
        }
        fragment.push(unit);
        at += 1;
        if unit == 92 {
            if let Some(&next) = raw.get(at) {
                fragment.push(next);
                at += 1;
            } else {
                break;
            }
        }
    }
    if let Some(pos) = fragment.iter().rposition(|u| *u == 92) {
        let tail = &fragment[pos..];
        if tail.len() >= 2
            && tail[1] == 117
            && tail.len() <= 5
            && tail[2..].iter().all(|u| char::from_u32(*u as u32).is_some_and(|c| c.is_ascii_hexdigit()))
        {
            fragment.truncate(pos);
        }
    }
    let trailing = fragment.iter().rev().take_while(|u| **u == 92).count();
    if trailing % 2 == 1 {
        fragment.pop();
    }
    let mut quoted = vec![34];
    quoted.extend_from_slice(&fragment);
    quoted.push(34);
    let value = match &WireValue::parse_units(&quoted) {
        Ok(WireValue::String(value)) => value.clone(),
        _ => WireString::from_units(fragment),
    };
    Some(sanitize(&value))
}
fn input_entries(
    args: &EditRenderArgs,
    options: RenderOptions,
    mode: Option<ara_edit::EditMode>,
) -> (Option<Vec<InputEntry>>, Option<String>) {
    let input = string(&args.input).or_else(|| string(&args.alternate_input));
    match (mode, input) {
        (Some(ara_edit::EditMode::Hashline), Some(input)) => (Some(hashline_entries(&input)), None),
        (Some(ara_edit::EditMode::Sloppy), Some(input)) => {
            use ara_edit::ModeEngine;
            let inspection = ara_edit::modes::sloppy::SloppyEngine { allow_fuzzy: false, fuzzy_threshold: 0.0 }
                .inspect(&ara_edit::ArgSnapshot { input: Some(input), ..Default::default() });
            let mut entries = Vec::new();
            for path in inspection.paths {
                if !entries.iter().any(|entry: &InputEntry| entry.path == path) {
                    entries.push(InputEntry { path, ..Default::default() });
                }
            }
            for op in inspection.file_ops {
                match op {
                    ara_edit::FileOpIntent::Delete { path } => {
                        if let Some(entry) = entries.iter_mut().find(|entry| entry.path == path) {
                            entry.op = Some(Operation::Delete);
                        }
                    }
                    ara_edit::FileOpIntent::Move { from, to } => {
                        if let Some(entry) = entries.iter_mut().find(|entry| entry.path == from) {
                            entry.op = Some(Operation::Update);
                            entry.rename = Some(to);
                        }
                    }
                }
            }
            (if entries.is_empty() { None } else { Some(entries) }, None)
        }
        (Some(ara_edit::EditMode::ApplyPatch) | None, Some(input)) if args.input.is_some() => {
            // Target inspect accepts a partial envelope and reports its ordered
            // path/file-op projection, rather than staging the payload.
            use ara_edit::ModeEngine;
            let inspection=ara_edit::modes::apply_patch::ApplyPatchEngine{allow_fuzzy:true,fuzzy_threshold:0.95}.inspect(&ara_edit::ArgSnapshot{input:Some(input),..Default::default()});
            let mut entries:Vec<InputEntry>= Vec::new();
                    for path in inspection.paths{if !entries.iter().any(|e| e.path ==path) {
                            entries.push(InputEntry { path,..Default::default()});}}
            for intent in inspection.file_ops{let(path, op, rename)=match intent{ara_edit::FileOpIntent::Delete{path}=>(path,Operation::Delete,None),ara_edit::FileOpIntent::Move{from,to}=>(from,Operation::Update,Some(to))};if let Some(entry)=entries.iter_mut().find(|e|e.path==path){entry.op=Some(op);entry.rename=rename;}else{entries.push(InputEntry{path,op:Some(op),rename,..Default::default()})}
                    }
                    (Some(entries), None)
        }
        _ => (None, None),
    }
}
fn count_files(args: &EditRenderArgs) -> usize {
    args.edits
        .as_ref()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.path.as_ref())
                .filter(|p| !p.is_empty())
                .collect::<HashSet<_>>()
                .len()
        })
        .unwrap_or(0)
}
pub(crate) fn call_facts(args: &EditRenderArgs, options: RenderOptions, context: &EditRenderContext) -> CallFacts {
    let (mut entries, error) = input_entries(args, options, context.edit_mode);
    if context.edit_mode==Some(ara_edit::EditMode::Sloppy){if let Some(entries)=&mut entries{for entry in entries{entry.op=None;entry.rename=None;}}}
    call_facts_with_input_summary(args, entries, error)
}
pub(crate) fn call_facts_with_input_summary(
    args: &EditRenderArgs,
    entries: Option<Vec<InputEntry>>,
    error: Option<String>,
) -> CallFacts {
    let first = entries.as_ref().and_then(|e| e.first());
    let edit = args.edits.as_ref().and_then(|e| e.first());
    let path = string(&args.file_path)
        .or_else(|| string(&args.path))
        .or_else(|| edit.and_then(|e| string(&e.path)))
        .or_else(|| partial_json_path(&args.partial_json))
        .or_else(|| first.map(|e| e.path.clone()))
        .unwrap_or_default();
    let rename = string(&args.rename)
        .or_else(|| edit.and_then(|e| string(&e.rename).or_else(|| string(&e.move_to))))
        .or_else(|| first.and_then(|e| e.rename.clone()));
    let op = args.op.or_else(|| edit.and_then(|e| e.op)).or_else(|| first.and_then(|e| e.op));
    let file_count = if args.edits.is_some() { count_files(args) } else { entries.as_ref().map(Vec::len).unwrap_or(0) };
    CallFacts {
        path,
        rename,
        op,
        file_count,
        apply_error: error,
        hashline_line_edits: first.is_some_and(|e| e.line_edits),
    }
}
fn stats_suffix(diff: &str, theme: &Theme) -> String {
    let stats = diff_stats(diff);
    let mut entries = Vec::new();
    if stats.added > 0 {
        entries.push(theme.fg("toolDiffAdded", &format!("+{}", stats.added)));
    }
    if stats.removed > 0 {
        entries.push(theme.fg("toolDiffRemoved", &format!("-{}", stats.removed)));
    }
    if entries.is_empty() {
        String::new()
    } else {
        format!(
            " {}{}{}",
            theme.fg("dim", theme.symbol("format.bracketLeft")),
            entries.join(&theme.fg("dim", "/")),
            theme.fg("dim", theme.symbol("format.bracketRight"))
        )
    }
}
#[derive(Default)]
struct Header<'a> {
    path: &'a str,
    rename: Option<&'a str>,
    op: Option<Operation>,
    line: Option<u32>,
    link_path: Option<&'a str>,
    icon: Option<String>,
    title: Option<&'a str>,
    suffix: &'a str,
}
fn header(columns: usize, theme: &Theme, frame: &FrameContext, options: Header<'_>) -> String {
    let path = format_path(options.path, None);
    let rename = options.rename.filter(|path| !path.is_empty()).map(|path| format_path(path, None));
    let path_width = width(&path) + rename.as_ref().map(|r| width(r)).unwrap_or(0);
    let build = |max: Option<usize>| {
        let path = format_path(options.path, max);
        let display = if options.path.is_empty() {
            theme.fg("toolOutput", "…")
        } else {
            file_hyperlink(
                options.link_path.unwrap_or(options.path),
                &theme.fg("accent", &path),
                options.line,
                frame.hyperlinks,
            )
        };
        let mut description =
            format!("{} {display}", theme.fg("muted", theme.language_icon(language_from_path(options.path))));
        if let Some(rename) = options.rename.filter(|path| !path.is_empty()) {
            description.push_str(&format!(
                " {} {}",
                theme.fg("dim", "→"),
                file_hyperlink(rename, &theme.fg("accent", &format_path(rename, max)), None, frame.hyperlinks)
            ));
        }
        format!(
            "{}{}",
            output_block::status_line(
                options.title.unwrap_or(options.op.unwrap_or_default().title()),
                &description,
                options.icon.as_deref(),
                theme
            ),
            options.suffix
        )
    };
    let full = build(None);
    let budget = columns.saturating_sub(
        width(theme.symbol("boxRound.topLeft"))
            + width(theme.symbol("boxRound.horizontal")) * 3
            + width(theme.symbol("boxRound.topRight"))
            + 2,
    );
    let overflow = width(&full).saturating_sub(budget);
    if overflow == 0 || path_width <= 1 {
        return full;
    }
    let count =
        usize::from(!options.path.is_empty()) + usize::from(options.rename.is_some_and(|path| !path.is_empty()));
    build(Some(path_width.saturating_sub(overflow).checked_div(count.max(1)).unwrap_or(1).max(1)))
}
fn format_path(path: &str, max: Option<usize>) -> String {
    let path = replace_tabs(&shorten_path(path));
    if let Some(max) = max { middle_truncate(&path, max) } else { path }
}
fn inline(
    columns: usize,
    theme: &Theme,
    frame: &FrameContext,
    facts: &CallFacts,
    pending: bool,
    link_path: Option<&str>,
) -> Vec<String> {
    let delete = facts.op == Some(Operation::Delete);
    vec![header(
        columns,
        theme,
        frame,
        Header {
            path: &facts.path,
            rename: facts.rename.as_deref(),
            op: facts.op,
            icon: Some(if pending {
                theme.status("pending", None)
            } else {
                theme.styled_symbol(if delete { "tool.delete" } else { "tool.move" }, "accent")
            }),
            title: Some(if delete { "Delete" } else { "Move" }),
            link_path,
            ..Default::default()
        },
    )]
}
pub fn streaming_fallback(mode: ara_edit::EditMode, args: &EditRenderArgs, theme: &Theme) -> String {
    if mode != ara_edit::EditMode::Replace {
        return String::new();
    }
    nonempty(&args.new_string)
        .or_else(|| nonempty(&args.new_text))
        .map(|text| plain_preview(&text, theme))
        .unwrap_or_default()
}
fn plain_preview(text: &str, theme: &Theme) -> String {
    let text = sanitize(&WireString::from(text));
    let lines = text.split('\n').collect::<Vec<_>>();
    let mut out = "\n\n".to_owned();
    for line in lines.iter().take(6) {
        out.push_str(&theme.fg("toolOutput", &truncate(&replace_tabs(line), 80)));
        out.push('\n');
    }
    if lines.len() > 6 {
        out.push_str(&theme.fg("dim", &format!("… {} more lines", lines.len() - 6)));
    }
    out.trim_end().to_owned()
}
pub fn wrap_edit_line(line: &str, columns: usize) -> Vec<String> {
    if columns == 0 || line.is_empty() {
        return vec![line.to_owned()];
    }
    static START: OnceLock<regex::Regex> = OnceLock::new();
    static GUTTER: OnceLock<regex::Regex> = OnceLock::new();
    static ASCII: OnceLock<regex::Regex> = OnceLock::new();
    let start = START
        .get_or_init(|| regex::Regex::new(r"^((?:\x1b\[[0-9;]*m)*)").unwrap())
        .find(line)
        .map(|m| m.as_str())
        .unwrap_or("");
    let body = &line[start.len()..];
    let body = body.strip_suffix("\x1b[39m").unwrap_or(body);
    let Some(m) =
        GUTTER.get_or_init(|| regex::Regex::new(r"(?s)^(\s*[+\-]?\s*[0-9]*)([|│])(.*)$").unwrap()).captures(body)
    else {
        return wrap(line, columns);
    };
    let gutter = &m[1];
    let separator = &m[2];
    if gutter.is_empty()
        || (separator == "|"
            && !ASCII.get_or_init(|| regex::Regex::new(r"^[+\-\s]\s*[0-9]+$").unwrap()).is_match(gutter))
    {
        return wrap(line, columns);
    }
    let prefix = format!("{gutter}{separator}");
    let prefix_width = width(&prefix);
    let continuation = format!("{}{separator}", " ".repeat(prefix_width.saturating_sub(1)));
    wrap(&m[3], columns.saturating_sub(prefix_width).max(1))
        .into_iter()
        .enumerate()
        .map(|(i, segment)| format!("{start}{}{segment}\x1b[27m\x1b[39m", if i == 0 { &prefix } else { &continuation }))
        .collect()
}
fn streaming_tail(diff: &str, inner: usize, budget: usize) -> (&str, bool) {
    let end = diff.trim_end_matches('\n').len();
    if end == 0 {
        return ("", false);
    }
    let mut start = end;
    let mut cursor = end;
    let mut rows = 0;
    loop {
        let newline = diff[..cursor].rfind('\n');
        let line_start = newline.map(|n| n + 1).unwrap_or(0);
        let line_rows = wrap(&replace_tabs(&diff[line_start..cursor]), inner).len().max(1);
        if rows > 0 && rows + line_rows > budget.max(1) {
            break;
        }
        rows += line_rows;
        start = line_start;
        if let Some(newline) = newline {
            cursor = newline;
        } else {
            break;
        }
    }
    (&diff[start..end], start > 0)
}
fn streaming_diff(
    diff: &str,
    path: &str,
    columns: usize,
    theme: &Theme,
    frame: &FrameContext,
    options: RenderOptions,
    label: &str,
    animate: bool,
) -> String {
    if diff.is_empty() {
        return String::new();
    }
    let budget = if options.expanded { frame.preview_rows() } else { frame.preview_rows().min(12) };
    let (tail, hidden) = streaming_tail(diff, columns.saturating_sub(2).max(1), budget);
    let mut out = "\n\n".to_owned();
    if hidden {
        out.push_str(&theme.fg("dim", "… (content above)"));
        out.push('\n');
    }
    out.push_str(&diff::render_scalar_diff(tail, Some(path), theme));
    let spinner = if animate { options.spinner_frame.map(|frame| theme.status("running", Some(frame))) } else { None };
    let hide = options.expanded && label == "preview";
    if spinner.is_some() || !hide {
        out.push('\n');
        if let Some(spinner) = spinner {
            out.push_str(&spinner);
            if !hide {
                out.push(' ');
            }
        }
        if !hide {
            out.push_str(&theme.fg("dim", &format!("({label})")));
        }
    }
    out
}
fn call_preview(
    args: &EditRenderArgs,
    path: &str,
    columns: usize,
    theme: &Theme,
    frame: &FrameContext,
    options: RenderOptions,
    context: &EditRenderContext,
) -> String {
    if let Some(files) = context.per_file_diff_preview.as_ref().filter(|files| {
        files.len() > 1
            && files.iter().any(|p| {
                p.diff.as_ref().is_some_and(|d| !d.is_empty()) || p.error.as_ref().is_some_and(|e| !e.is_empty())
            })
    }) {
        let mut out = String::new();
        for (i, file) in files.iter().enumerate() {
            let path = sanitize(&file.path.0);
            if file.diff.as_ref().is_none_or(WireString::is_empty)
                && file.error.as_ref().is_none_or(WireString::is_empty)
            {
                continue;
            }
            out.push_str(&theme.fg("dim", &format!("\n\n── {} ──", shorten_path(&path))));
            if let Some(error) = nonempty(&file.error) {
                out.push('\n');
                out.push_str(&theme.fg("error", &replace_tabs(&error)));
            } else if let Some(diff) = nonempty(&file.diff) {
                out.push_str(&streaming_diff(
                    &diff,
                    &path,
                    columns,
                    theme,
                    frame,
                    options,
                    "preview",
                    i + 1 == files.len(),
                ));
            }
        }
        return out;
    }
    if let Some(diff) = nonempty(&args.preview_diff) {
        return streaming_diff(&diff, path, columns, theme, frame, options, "preview", true);
    }
    if let Some(diff) = nonempty(&args.diff) {
        return if args.op.is_some() {
            streaming_diff(&diff, path, columns, theme, frame, options, "streaming", true)
        } else {
            plain_preview(&diff, theme)
        };
    }
    if let Some(text) = nonempty(&args.new_text).or_else(|| nonempty(&args.patch)) {
        return plain_preview(&text, theme);
    }
    if let Some(fallback) = &context.edit_streaming_fallback {
        return fallback.clone();
    }
    if let Some(EditPreview::Error(error)) = &context.edit_diff_preview {
        return theme.fg("error", &replace_tabs(&sanitize(error)));
    }
    String::new()
}

/// A retained call/result component. Width-dependent layout and colored diff
/// cache are separate; callback identity/theme/expansion invalidate only color.
#[derive(Clone)]
pub struct EditComponent {
    pub args: EditRenderArgs,
    pub result: Option<EditResult>,
    pub options: RenderOptions,
    pub context: EditRenderContext,
    pub theme: Arc<Theme>,
    colored_cache: Vec<ColorCache>,
    stats_cache: Vec<(String, u64, u64, String)>,
    pub(crate) captured_call: Option<CallFacts>,
    pub(crate) captured_inline: Option<bool>,
    pub(crate) frame_header_diff: Option<Option<WireString>>,
    frozen_single: Option<(String, Option<Operation>, Option<String>)>,
    raw_single_path: Option<WireString>,
    pub(crate) selected_path: Option<ara_rpc::value_heap::ValueHandle>,
    pub(crate) selected_rename: Option<ara_rpc::value_heap::ValueHandle>,
    pub(crate) selected_apply_error: Option<ara_rpc::value_heap::ValueHandle>,
    pub(crate) graph_call_preview: Option<String>,
    graph_stats_cache: Option<GraphStringCache>,
    graph_section_cache: Option<GraphStringCache>,
    graph_color_cache: Option<GraphStringCache>,
    graph_preview_cache: Option<GraphStringCache>,
    graph_render_callback: Option<ara_rpc::value_heap::ValueHandle>,
    graph_callback_cache: Option<GraphStringCache<ara_rpc::value_heap::ValueHandle>>,
}

#[derive(Clone)]
struct GraphStringCache<T = String> {
    content: ara_rpc::value_heap::ValueHandle,
    theme: u64,
    revision: u64,
    expanded: bool,
    salt: String,
    value: T,
}
impl<T> GraphStringCache<T> {
    fn matches(
        &self,
        g: &super::graph_renderer::DisplayGraph<'_>,
        theme: &Theme,
        expanded: bool,
        salt: &str,
        content: &ara_rpc::value_heap::ValueHandle,
    ) -> Result<bool, ara_rpc::value_heap::ValueError> {
        Ok(self.theme == theme.identity
            && self.revision == theme.revision
            && self.expanded == expanded
            && self.salt == salt
            && graph_strict_equal(g, &self.content, content)?)
    }
    fn new(theme: &Theme, expanded: bool, salt: String, content: ara_rpc::value_heap::ValueHandle, value: T) -> Self {
        Self { theme: theme.identity, revision: theme.revision, expanded, salt, content, value }
    }
}
fn graph_strict_equal(
    g: &super::graph_renderer::DisplayGraph<'_>,
    left: &ara_rpc::value_heap::ValueHandle,
    right: &ara_rpc::value_heap::ValueHandle,
) -> Result<bool, ara_rpc::value_heap::ValueError> {
    match (g.0.heap().snapshot(left)?, g.0.heap().snapshot(right)?) {
        (ara_rpc::value_heap::ValueSnapshot::Number(a), ara_rpc::value_heap::ValueSnapshot::Number(b)) => Ok(a == b),
        _ => g.0.heap().object_is(left, right),
    }
}

#[derive(Clone)]
struct ColorCache {
    diff: String,
    path: String,
    theme: u64,
    revision: u64,
    expanded: bool,
    callback: usize,
    rendered: String,
}
impl EditComponent {
    pub fn call(args: EditRenderArgs, options: RenderOptions, context: EditRenderContext, theme: Arc<Theme>) -> Self {
        Self {
            args,
            result: None,
            options,
            context,
            theme,
            colored_cache: Vec::new(),
            stats_cache: Vec::new(),
            captured_call: None,
            captured_inline: None,
            frame_header_diff: None,
            frozen_single: None,
            raw_single_path: None,
            selected_path: None,
            selected_rename: None,
            selected_apply_error: None,
            graph_call_preview: None,
            graph_stats_cache: None,
            graph_section_cache: None,
            graph_color_cache: None,
            graph_preview_cache: None,
            graph_render_callback: None,
            graph_callback_cache: None,
        }
    }
    pub fn result(
        result: EditResult,
        args: EditRenderArgs,
        options: RenderOptions,
        context: EditRenderContext,
        theme: Arc<Theme>,
    ) -> Self {
        let mut this = Self::call(args, options, context, theme);
        this.result = Some(result);
        this
    }
    pub(crate) fn capture_single(&mut self) {
        let details = self.result.as_ref().and_then(|r| r.details.as_ref());
        let edit = self.args.edits.as_ref().and_then(|e| e.first());
        let path = details
            .and_then(|d| string(&d.source_path))
            .or_else(|| string(&self.args.file_path))
            .or_else(|| string(&self.args.path))
            .or_else(|| edit.and_then(|e| string(&e.path)))
            .or_else(|| details.and_then(|d| string(&d.path)))
            .or_else(|| {
                if self.context.edit_mode == Some(ara_edit::EditMode::Hashline) {
                    string(&self.args.input)
                        .or_else(|| string(&self.args.alternate_input))
                        .and_then(|input| hashline_entries(&input).first().map(|h| h.path.clone()))
                } else {
                    None
                }
            })
            .unwrap_or_default();
        let op = self.args.op.or_else(|| edit.and_then(|e| e.op)).or_else(|| details.and_then(|d| d.op));
        let rename = string(&self.args.rename)
            .or_else(|| edit.and_then(|e| string(&e.rename).or_else(|| string(&e.move_to))))
            .or_else(|| details.and_then(|d| string(&d.move_to)));
        // Callback options use the construction operand before display sanitize.
        self.raw_single_path = details
            .and_then(|d| d.source_path.as_ref())
            .or(self.args.file_path.as_ref())
            .or(self.args.path.as_ref())
            .or_else(|| edit.and_then(|e| e.path.as_ref()))
            .or_else(|| details.and_then(|d| d.path.as_ref()))
            .cloned()
            .or_else(|| Some(path.clone().into()));
        self.frozen_single = Some((path, op, rename));
    }
    pub(crate) fn check_selected_header(
        &self,
        g: &super::graph_renderer::DisplayGraph<'_>,
    ) -> Result<(), ara_rpc::value_heap::ValueError> {
        // getLanguageFromPath calls node:path.basename at this source consumer.
        if let Some(path) = &self.selected_path {
            g.string(path).map_err(|_| {
                ara_rpc::value_heap::ValueError::TypeError("node:path.basename requires a string path".into())
            })?;
        }
        if let Some(rename) = &self.selected_rename {
            if g.truth(rename)? {
                g.string(rename).map_err(|_| {
                    ara_rpc::value_heap::ValueError::TypeError("fileHyperlink path requires a string".into())
                })?;
            }
        }
        Ok(())
    }
    pub fn replace_args(&mut self, args: EditRenderArgs) {
        self.args = args;
    }
    fn colored(&mut self, slot: usize, diff: &str, path: &str) -> String {
        let callback = self.context.render_diff.as_ref().map(|f| Arc::as_ptr(f) as *const () as usize).unwrap_or(0);
        let key = ColorCache {
            diff: diff.to_owned(),
            path: path.to_owned(),
            theme: self.theme.identity,
            revision: self.theme.revision,
            expanded: self.options.expanded,
            callback,
            rendered: String::new(),
        };
        if let Some(existing) = self.colored_cache.get(slot) {
            if existing.diff == key.diff
                && existing.path == key.path
                && existing.theme == key.theme
                && existing.revision == key.revision
                && existing.expanded == key.expanded
                && existing.callback == callback
            {
                return existing.rendered.clone();
            }
        }
        let rendered = if let Some(render) = &self.context.render_diff {
            render(diff, Some(path))
        } else {
            diff::render_scalar_diff(diff, Some(path), &self.theme)
        };
        let mut key = key;
        key.rendered = rendered.clone();
        if self.colored_cache.len() <= slot {
            self.colored_cache.resize_with(slot + 1, || key.clone());
        }
        self.colored_cache[slot] = key;
        rendered
    }

    pub(crate) fn streaming_preview_in(
        &mut self,
        g: &super::graph_renderer::DisplayGraph<'_>,
        value: &ara_rpc::value_heap::ValueHandle,
        columns: usize,
        frame: &FrameContext,
        label: &str,
    ) -> Result<String, ara_rpc::value_heap::ValueError> {
        if !g.truth(value)? {
            return Ok(String::new());
        }
        let inner = columns.saturating_sub(2).max(1);
        let budget = if self.options.expanded { frame.preview_rows() } else { frame.preview_rows().min(12) };
        let path = self.captured_call.as_ref().map(|facts| facts.path.as_str()).unwrap_or("");
        let salt = format!("{path}:{inner}:{budget}");
        let core = if let Some(cache) = self
            .graph_preview_cache
            .as_ref()
            .filter(|cache| cache.matches(g, &self.theme, self.options.expanded, &salt, value).unwrap_or(false))
        {
            cache.value.clone()
        } else {
            let (tail, hidden) = g.streaming_tail_value(value, inner, budget)?;
            let text = g.string(&g.sanitize_value(&tail)?)?;
            let mut core = "\n\n".to_owned();
            if hidden {
                core.push_str(&self.theme.fg("dim", "… (content above)"));
                core.push('\n');
            }
            core.push_str(&diff::render_scalar_diff(&sanitize(&text), Some(path), &self.theme));
            self.graph_preview_cache =
                Some(GraphStringCache::new(&self.theme, self.options.expanded, salt, value.clone(), core.clone()));
            core
        };
        let mut out = core;
        let spinner = self.options.spinner_frame.map(|frame| self.theme.status("running", Some(frame)));
        let hide = self.options.expanded && label == "preview";
        if spinner.is_some() || !hide {
            out.push('\n');
            if let Some(spinner) = spinner {
                out.push_str(&spinner);
                if !hide {
                    out.push(' ');
                }
            }
            if !hide {
                out.push_str(&self.theme.fg("dim", &format!("({label})")));
            }
        }
        Ok(out)
    }
    fn stats_in(
        &mut self,
        g: &super::graph_renderer::DisplayGraph<'_>,
        value: &ara_rpc::value_heap::ValueHandle,
    ) -> Result<String, ara_rpc::value_heap::ValueError> {
        if let Some(cache) = &self.graph_stats_cache {
            if cache.matches(g, &self.theme, false, "", value)? {
                return Ok(cache.value.clone());
            }
        }
        let stats = g.diff_stats_value(value)?;
        let mut entries = Vec::new();
        if stats.added > 0 {
            entries.push(self.theme.fg("toolDiffAdded", &format!("+{}", stats.added)));
        }
        if stats.removed > 0 {
            entries.push(self.theme.fg("toolDiffRemoved", &format!("-{}", stats.removed)));
        }
        let text = if entries.is_empty() {
            String::new()
        } else {
            format!(
                " {}{}{}",
                self.theme.fg("dim", self.theme.symbol("format.bracketLeft")),
                entries.join(&self.theme.fg("dim", "/")),
                self.theme.fg("dim", self.theme.symbol("format.bracketRight"))
            )
        };
        self.graph_stats_cache =
            Some(GraphStringCache::new(&self.theme, false, String::new(), value.clone(), text.clone()));
        Ok(text)
    }
    fn refresh_graph_render_callback(
        &mut self,
        g: &super::graph_renderer::DisplayGraph<'_>,
    ) -> Result<(), ara_rpc::value_heap::ValueError> {
        let selected = match &self.context.render_diff_value {
            Some(value) if !g.nullish(value)? => Some(value.clone()),
            _ => None,
        };
        let same = match (&self.graph_render_callback, &selected) {
            (Some(old), Some(new)) => graph_strict_equal(g, old, new)?,
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.graph_section_cache = None;
            self.graph_color_cache = None;
            self.graph_callback_cache = None;
        }
        self.graph_render_callback = selected;
        Ok(())
    }
    fn graph_callback_path(
        &self,
        g: &super::graph_renderer::DisplayGraph<'_>,
        path: &str,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_rpc::value_heap::ValueError> {
        if let Some(value) = &self.selected_path {
            return Ok(value.clone());
        }
        g.0.heap().string(self.raw_single_path.clone().unwrap_or_else(|| path.into()))
    }
    fn diff_section_in(
        &mut self,
        g: &super::graph_renderer::DisplayGraph<'_>,
        slot: usize,
        value: &ara_rpc::value_heap::ValueHandle,
        path: &str,
        inner: usize,
    ) -> Result<String, ara_rpc::value_heap::ValueError> {
        let callback = if self.context.render_diff_value.is_none() {
            self.context.render_diff.as_ref().map(|f| Arc::as_ptr(f) as *const () as usize).unwrap_or(0)
        } else {
            0
        };
        let salt = format!("{path}:{inner}:{callback}");
        if let Some(cache) = &self.graph_section_cache {
            if cache.matches(g, &self.theme, self.options.expanded, &salt, value)? {
                return Ok(cache.value.clone());
            }
        }
        let (truncated, hidden_hunks, hidden_lines) = if self.options.expanded {
            (value.clone(), 0, 0)
        } else {
            g.truncate_diff_value(value, COLLAPSED_HUNKS, COLLAPSED_LINES)?
        };
        let color_salt = format!("{path}:{callback}");
        let text = if let Some(function) = self.graph_render_callback.clone() {
            let cached = if let Some(cache) = &self.graph_callback_cache {
                cache.matches(g, &self.theme, self.options.expanded, &color_salt, &truncated)?
            } else {
                false
            };
            let rendered = if cached {
                self.graph_callback_cache.as_ref().unwrap().value.clone()
            } else {
                let options = g.0.heap().object()?;
                g.0.heap().define_own_data(&options, "filePath".into(), self.graph_callback_path(g, path)?, g.1)?;
                // Original unbound local function Call: no receiver conversion,
                // no selected argument string projection or replacement carrier.
                let rendered =
                    g.0.heap().call(&function, &g.0.heap().undefined()?, &[truncated.clone(), options], g.1)?;
                self.graph_callback_cache = Some(GraphStringCache::new(
                    &self.theme,
                    self.options.expanded,
                    color_salt,
                    truncated.clone(),
                    rendered.clone(),
                ));
                rendered
            };
            self.finish_graph_diff_section(g, &rendered, hidden_hunks, hidden_lines, inner)?
        } else {
            let cached = if let Some(cache) = &self.graph_color_cache {
                cache.matches(g, &self.theme, self.options.expanded, &color_salt, &truncated)?
            } else {
                false
            };
            let rendered = if cached {
                self.graph_color_cache.as_ref().unwrap().value.clone()
            } else {
                let text =
                    if callback == 0 { g.string(&g.sanitize_value(&truncated)?)? } else { g.string(&truncated)? };
                let rendered = if callback == 0 {
                    diff::render_scalar_diff(&sanitize(&text), Some(path), &self.theme)
                } else {
                    self.colored(slot, &sanitize(&text), path)
                };
                self.graph_color_cache = Some(GraphStringCache::new(
                    &self.theme,
                    self.options.expanded,
                    color_salt,
                    truncated.clone(),
                    rendered.clone(),
                ));
                rendered
            };
            self.finish_diff_section(rendered, hidden_hunks, hidden_lines, inner)
        };
        self.graph_section_cache =
            Some(GraphStringCache::new(&self.theme, self.options.expanded, salt, value.clone(), text.clone()));
        Ok(text)
    }
    fn finish_graph_diff_section(
        &self,
        g: &super::graph_renderer::DisplayGraph<'_>,
        rendered: &ara_rpc::value_heap::ValueHandle,
        hidden_hunks: usize,
        hidden_lines: usize,
        inner: usize,
    ) -> Result<String, ara_rpc::value_heap::ValueError> {
        let (visible, hidden_visual) = if self.options.expanded {
            (sanitize(&g.0.heap().to_string(rendered, g.1)?), 0)
        } else {
            // Keep the original callback result in the color cache. On a new
            // width, source consumes that same result's split method again.
            let lines = g.method(rendered, "split", &[g.0.heap().string("\n".into())?])?;
            let mut rows = Vec::new();
            let mut complete = 0;
            for line in g.members(&lines)? {
                let line = g.string(&line)?;
                let wrapped = wrap_edit_line(&sanitize(&line), inner);
                let remaining = COLLAPSED_LINES.saturating_sub(rows.len());
                if wrapped.len() <= remaining {
                    rows.extend(wrapped);
                    complete += 1;
                } else {
                    rows.extend(wrapped.into_iter().take(remaining));
                    break;
                }
            }
            let count = g.0.heap().to_number(&g.get(&lines, "length")?, g.1)?.max(0.0) as usize;
            (rows.join("\n"), count.saturating_sub(complete))
        };
        Ok(self.finish_visible_diff_section(visible, hidden_visual, hidden_hunks, hidden_lines))
    }

    fn stats(&mut self, slot: usize, diff: &str) -> String {
        if let Some((old, theme, revision, value)) = self.stats_cache.get(slot) {
            if old == diff && *theme == self.theme.identity && *revision == self.theme.revision {
                return value.clone();
            }
        }
        let value = stats_suffix(diff, &self.theme);
        let entry = (diff.to_owned(), self.theme.identity, self.theme.revision, value.clone());
        if self.stats_cache.len() <= slot {
            self.stats_cache.resize(slot + 1, entry.clone());
        }
        self.stats_cache[slot] = entry;
        value
    }
    fn diff_section(&mut self, slot: usize, diff: &str, path: &str, inner: usize) -> String {
        let truncated = if self.options.expanded {
            TruncatedDiff { text: diff.to_owned(), hidden_hunks: 0, hidden_lines: 0 }
        } else {
            truncate_diff_by_hunk(diff, COLLAPSED_HUNKS, COLLAPSED_LINES)
        };
        let rendered = self.colored(slot, &truncated.text, path);
        self.finish_diff_section(rendered, truncated.hidden_hunks, truncated.hidden_lines, inner)
    }
    fn finish_diff_section(&self, rendered: String, hidden_hunks: usize, hidden_lines: usize, inner: usize) -> String {
        let (visible, hidden_visual) = if self.options.expanded {
            (rendered, 0)
        } else {
            let lines = rendered.split('\n').collect::<Vec<_>>();
            let mut rows = Vec::new();
            let mut complete = 0;
            for line in &lines {
                let wrapped = wrap_edit_line(line, inner);
                let remaining = COLLAPSED_LINES.saturating_sub(rows.len());
                if wrapped.len() <= remaining {
                    rows.extend(wrapped);
                    complete += 1;
                } else {
                    rows.extend(wrapped.into_iter().take(remaining));
                    break;
                }
            }
            (rows.join("\n"), lines.len() - complete)
        };
        self.finish_visible_diff_section(visible, hidden_visual, hidden_hunks, hidden_lines)
    }
    fn finish_visible_diff_section(
        &self,
        visible: String,
        hidden_visual: usize,
        hidden_hunks: usize,
        hidden_lines: usize,
    ) -> String {
        let mut out = format!("\n{visible}");
        let hidden = hidden_lines + hidden_visual;
        if !self.options.expanded && (hidden > 0 || hidden_hunks > 0) {
            let mut remainder = Vec::new();
            if hidden_hunks > 0 {
                remainder.push(format!("{} more hunks", hidden_hunks));
            }
            if hidden > 0 {
                remainder.push(format!("{hidden} more lines"));
            }
            out.push_str(
                &self.theme.fg("toolOutput", &format!("\n… ({}) {}", remainder.join(", "), expand_hint(&self.theme))),
            );
        }
        out
    }
    fn render_call(&self, columns: usize, frame: &FrameContext) -> Vec<String> {
        let facts = self.captured_call.clone().unwrap_or_else(|| call_facts(&self.args, self.options, &self.context));
        let payload = self.context.per_file_diff_preview.as_ref().is_some_and(|p| {
            p.len() > 1 && p.iter().any(|p| p.diff.as_ref().is_some_and(|d| !d.is_empty()) || p.error.is_some())
        }) || nonempty(&self.args.preview_diff).is_some()
            || nonempty(&self.args.diff).is_some()
            || nonempty(&self.args.new_text).is_some()
            || nonempty(&self.args.patch).is_some()
            || self.args.edits.as_ref().is_some_and(|e| !e.is_empty())
            || self.context.edit_streaming_fallback.as_ref().is_some_and(|s| !s.is_empty())
            || facts.hashline_line_edits;
        if self.captured_inline.unwrap_or(
            facts.file_count <= 1
                && facts.apply_error.is_none()
                && (facts.op == Some(Operation::Delete) || (facts.rename.is_some() && !payload)),
        ) {
            return inline(columns, &self.theme, frame, &facts, true, None);
        }
        let suffix = if facts.file_count > 1 {
            self.theme.fg("dim", &format!(" (+{} more)", facts.file_count - 1))
        } else {
            String::new()
        };
        let header = header(
            columns,
            &self.theme,
            frame,
            Header {
                path: &facts.path,
                rename: facts.rename.as_deref(),
                op: facts.op,
                suffix: &suffix,
                ..Default::default()
            },
        );
        let mut body = self.graph_call_preview.clone().unwrap_or_else(|| {
            call_preview(&self.args, &facts.path, columns, &self.theme, frame, self.options, &self.context)
        });
        if let Some(error) = &facts.apply_error {
            body.push_str(&format!(
                "\n{}",
                self.theme.fg("error", &truncate(&replace_tabs(error), columns.saturating_sub(2).max(1)))
            ));
        }
        let mut lines =
            if body.is_empty() { Vec::new() } else { body.split('\n').map(str::to_owned).collect::<Vec<_>>() };
        while lines.first().is_some_and(|line| line.trim().is_empty()) {
            lines.remove(0);
        }
        output_block::framed(
            &header,
            &lines,
            columns,
            if facts.apply_error.is_some() { State::Error } else { State::Pending },
            &self.theme,
        )
    }
    fn render_single(
        &mut self,
        slot: usize,
        result: &EditResult,
        args: &EditRenderArgs,
        columns: usize,
        frame: &FrameContext,
    ) -> Vec<String> {
        self.render_single_with(slot, result, args, columns, frame, None, None)
            .expect("owned renderer performs no fallible Graph work")
    }
    pub(crate) fn render_single_in(
        &mut self,
        g: &super::graph_renderer::DisplayGraph<'_>,
        details: Option<&ara_rpc::value_heap::ValueHandle>,
        columns: usize,
        frame: &FrameContext,
    ) -> Result<Vec<String>, ara_rpc::value_heap::ValueError> {
        let result = self.result.clone().unwrap_or_default();
        let args = self.args.clone();
        self.refresh_graph_render_callback(g)?;
        self.render_single_with(0, &result, &args, columns, frame, details.map(|details| (g, details)), Some(g))
    }
    fn render_single_with(
        &mut self,
        slot: usize,
        result: &EditResult,
        args: &EditRenderArgs,
        columns: usize,
        frame: &FrameContext,
        dynamic: Option<(&super::graph_renderer::DisplayGraph<'_>, &ara_rpc::value_heap::ValueHandle)>,
        graph: Option<&super::graph_renderer::DisplayGraph<'_>>,
    ) -> Result<Vec<String>, ara_rpc::value_heap::ValueError> {
        let details = result.details.as_ref();
        let error = result.is_error.or_else(|| details.and_then(|d| d.is_error)).unwrap_or(false);
        let edit = args.edits.as_ref().and_then(|e| e.first());
        let hash = if self.context.edit_mode == Some(ara_edit::EditMode::Hashline) {
            string(&args.input)
                .or_else(|| string(&args.alternate_input))
                .and_then(|input| hashline_entries(&input).first().cloned())
        } else {
            None
        };
        let path = details
            .and_then(|d| string(&d.source_path))
            .or_else(|| string(&args.file_path))
            .or_else(|| string(&args.path))
            .or_else(|| edit.and_then(|e| string(&e.path)))
            .or_else(|| details.and_then(|d| string(&d.path)))
            .or_else(|| hash.map(|h| h.path))
            .unwrap_or_default();
        let op = args.op.or_else(|| edit.and_then(|e| e.op)).or_else(|| details.and_then(|d| d.op));
        let rename = string(&args.rename)
            .or_else(|| edit.and_then(|e| string(&e.rename).or_else(|| string(&e.move_to))))
            .or_else(|| details.and_then(|d| string(&d.move_to)));
        let (path, op, rename) = self.frozen_single.clone().unwrap_or((path, op, rename));
        let rename = rename.filter(|r| !r.is_empty());
        let link_path = details.and_then(|d| string(&d.path));
        let detail_diff = details.map(|d| sanitize(&d.diff.0)).unwrap_or_default();
        if self.captured_inline.unwrap_or(
            !error
                && detail_diff.is_empty()
                && details.is_none_or(|d| d.diagnostics.is_none())
                && (op == Some(Operation::Delete) || rename.is_some()),
        ) {
            return Ok(inline(
                columns,
                &self.theme,
                frame,
                &CallFacts { path, rename, op, ..Default::default() },
                false,
                link_path.as_deref(),
            ));
        }
        let preview = if details.is_none() { self.context.edit_diff_preview.clone() } else { None };
        let preview_diff = preview
            .as_ref()
            .and_then(|p| if let EditPreview::Diff { diff, .. } = p { Some(sanitize(diff)) } else { None });
        let dynamic_line = if let Some((g, root)) = dynamic {
            if error {
                None
            } else {
                match g.0.heap().snapshot(&g.get(root, "firstChangedLine")?)? {
                    ara_rpc::value_heap::ValueSnapshot::Number(n) if n.is_finite() && n > 0.0 => Some(n as u32),
                    _ => None,
                }
            }
        } else {
            details.and_then(|d| d.first_changed_line)
        };
        let link_path =
            if let Some((g, root)) = dynamic { g.present(root, "path")?.as_ref().map(sanitize) } else { link_path };
        let header_diff = if error {
            None
        } else if !detail_diff.is_empty() {
            Some(detail_diff.clone())
        } else {
            preview_diff.clone()
        };
        let stats = if let Some((g, root)) = dynamic {
            if error {
                String::new()
            } else {
                let selected = g.get(root, "diff")?;
                if g.truth(&selected)? {
                    self.stats_in(g, &selected)?
                } else {
                    preview_diff.as_ref().map(|diff| self.stats(slot, diff)).unwrap_or_default()
                }
            }
        } else {
            header_diff.as_ref().map(|diff| self.stats(slot, diff)).unwrap_or_default()
        };
        let line = match &preview {
            Some(EditPreview::Diff { first_changed_line, .. }) => *first_changed_line,
            _ => None,
        }
        .filter(|l| *l > 0)
        .or(dynamic_line);
        let icon = if error {
            self.theme.status("error", None)
        } else if !self.options.is_partial {
            self.theme.styled_symbol("tool.edit", "accent")
        } else {
            self.theme.status("success", None)
        };
        if let Some((g, _)) = dynamic {
            self.check_selected_header(g)?;
        }
        let header = header(
            columns,
            &self.theme,
            frame,
            Header {
                path: &path,
                rename: rename.as_deref(),
                op,
                line,
                link_path: link_path.as_deref(),
                icon: Some(icon),
                suffix: &stats,
                ..Default::default()
            },
        );
        let inner = columns.saturating_sub(2).max(1);
        let selected_body = if !error {
            if let Some((g, root)) = dynamic {
                if g.truth(&g.get(root, "diff")?)? { Some(g.get(root, "diff")?) } else { None }
            } else {
                None
            }
        } else {
            None
        };
        let body_diff = if dynamic.is_none() { detail_diff.clone() } else { String::new() };
        let mut body = if error {
            details
                .and_then(|d| nonempty(&d.display_error_text).or_else(|| nonempty(&d.error_text)))
                .or_else(|| result.content.first().map(sanitize))
                .map(|text| self.theme.fg("error", &replace_tabs(&text)))
                .unwrap_or_default()
        } else if let Some(selected) = selected_body {
            self.diff_section_in(dynamic.unwrap().0, slot, &selected, &path, inner)?
        } else if !body_diff.is_empty() {
            self.diff_section(slot, &body_diff, &path, inner)
        } else if details.is_some() {
            if op != Some(Operation::Delete) && op != Some(Operation::Create) && rename.is_none() {
                let no_change = link_path.as_deref().filter(|p| !p.is_empty()).unwrap_or(&path);
                self.theme.fg(
                    "dim",
                    &format!(
                        "No changes were made{}.",
                        if no_change.is_empty() { String::new() } else { format!(" to {}", shorten_path(no_change)) }
                    ),
                )
            } else {
                String::new()
            }
        } else {
            match preview {
                Some(EditPreview::Error(error)) => self.theme.fg("error", &replace_tabs(&sanitize(&error))),
                Some(EditPreview::Diff { diff, .. }) if !diff.is_empty() => {
                    if let Some(g) = graph {
                        self.diff_section_in(g, slot, &g.0.heap().string(diff)?, &path, inner)?
                    } else {
                        self.diff_section(slot, &sanitize(&diff), &path, inner)
                    }
                }
                _ => String::new(),
            }
        };
        let dynamic_diag = if let Some((g, root)) = dynamic {
            let guard = g.get(root, "diagnostics")?;
            if g.truth(&guard)? {
                Some(super::event_controller::diagnostics_from_graph(g, &g.get(root, "diagnostics")?)?)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(diag) =
            if dynamic.is_some() { dynamic_diag.as_ref() } else { details.and_then(|d| d.diagnostics.as_ref()) }
        {
            body.push_str(&diagnostics(diag, self.options.expanded, &self.theme));
        }
        let mut lines = if body.is_empty() {
            Vec::new()
        } else {
            body.split('\n').flat_map(|line| wrap_edit_line(line, inner)).collect::<Vec<_>>()
        };
        while lines.first().is_some_and(|line| line.trim().is_empty()) {
            lines.remove(0);
        }
        Ok(output_block::framed(
            &header,
            &lines,
            columns,
            if error {
                State::Error
            } else if self.options.is_partial {
                State::Pending
            } else {
                State::Success
            },
            &self.theme,
        ))
    }
}
impl Component for EditComponent {
    fn render(&mut self, columns: usize, frame: &FrameContext) -> Vec<String> {
        let Some(result) = self.result.clone() else {
            return self.render_call(columns, frame);
        };
        let total = count_files(&self.args);
        if let Some(files) =
            result.details.as_ref().and_then(|d| d.per_file_results.as_ref()).filter(|f| f.len() > 1 || total > 1)
        {
            let mut out = Vec::new();
            for (i, file) in files.iter().enumerate() {
                if i > 0 {
                    out.push(String::new());
                }
                out.extend(self.render_single(
                    i,
                    &EditResult { content: Vec::new(), details: Some(file.clone()), is_error: file.is_error },
                    &EditRenderArgs::default(),
                    columns,
                    frame,
                ));
            }
            let remaining = total.saturating_sub(files.len());
            if remaining > 0 {
                if !out.is_empty() {
                    out.push(String::new());
                }
                let icon = self.options.spinner_frame.map(|s| self.theme.status("running", Some(s)));
                out.push(output_block::status_line(
                    "Edit",
                    &self
                        .theme
                        .fg("dim", &format!("{remaining} more file{} pending…", if remaining > 1 { "s" } else { "" })),
                    icon.as_deref(),
                    &self.theme,
                ));
            }
            out
        } else {
            let args = self.args.clone();
            self.render_single(0, &result, &args, columns, frame)
        }
    }
    fn invalidate(&mut self) {
        self.colored_cache.clear();
        self.stats_cache.clear();
        self.graph_stats_cache = None;
        self.graph_section_cache = None;
        self.graph_callback_cache = None;
        self.graph_color_cache = None;
        self.graph_preview_cache = None;
    }
}
pub fn is_edit_renderer(name: &WireString) -> bool {
    name.equals_ascii("edit") || name.equals_ascii("apply_patch")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditRendererRegistryEntry;
pub static EDIT_RENDERER: EditRendererRegistryEntry = EditRendererRegistryEntry;
pub fn renderer(name: &WireString) -> Option<&'static EditRendererRegistryEntry> {
    is_edit_renderer(name).then_some(&EDIT_RENDERER)
}
