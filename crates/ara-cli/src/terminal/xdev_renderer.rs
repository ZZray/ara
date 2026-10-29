//! Fixed OMP596f2da xdev.ts:487-663 / default-renderer.ts / json-tree.ts.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! Required original Write consumer: retained same-realm renderer components,
//! mounted callbacks before static renderer selection, and generic fallback.
use super::{
    graph_renderer::DisplayGraph, render_utils as text, theme::Theme, types::RenderOptions,
    write_renderer::WriteArgs,
};
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueKind,
        ValueSnapshot,
    },
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;
fn utf8(value: &WireString) -> String {
    String::from_utf16_lossy(value.units())
}
fn scalar(g: &DisplayGraph<'_>, value: &ValueHandle, max: usize) -> Result<String> {
    let heap = g.0.heap();
    Ok(match heap.kind(value)? {
        ValueKind::String => format!(
            "\"{}\"",
            text::truncate(
                &utf8(&g.string(value)?)
                    .replace('\n', "\\n")
                    .replace('\t', "\\t"),
                max
            )
        ),
        ValueKind::Array => format!(
            "[{} items]",
            utf8(&heap.to_string(&g.get(value, "length")?, g.runtime())?)
        ),
        ValueKind::Object => format!(
            "{{{} keys}}",
            heap.own_enumerable_string_keys(value, g.runtime())?.len()
        ),
        _ => utf8(&heap.to_string(value, g.runtime())?),
    })
}
fn record(g: &DisplayGraph<'_>, value: &ValueHandle) -> Result<bool> {
    Ok(g.0.heap().kind(value)? == ValueKind::Object)
}
fn hidden(g: &DisplayGraph<'_>) -> Result<ValueHandle> {
    let heap = g.0.heap();
    let object = heap.object()?;
    for key in ["i", "__partialJson"] {
        heap.define_own_data(&object, key.into(), heap.number(1.0)?, g.runtime())?;
    }
    Ok(object)
}
fn visible_keys(
    g: &DisplayGraph<'_>,
    value: &ValueHandle,
    hidden: &ValueHandle,
) -> Result<Vec<WireString>> {
    let heap = g.0.heap();
    let mut result = Vec::new();
    for key in heap.for_in_string_keys(value, g.runtime())? {
        if !heap.has(hidden, &key.clone().into(), g.runtime())? {
            result.push(key)
        }
    }
    Ok(result)
}
fn inline(
    g: &DisplayGraph<'_>,
    args: &ValueHandle,
    max: usize,
    hidden: &ValueHandle,
) -> Result<String> {
    let keys = visible_keys(g, args, hidden)?;
    let mut result = String::new();
    let mut width = 0i64;
    for (i, key) in keys.iter().enumerate() {
        let value = g.0.heap().get(args, &key.clone().into(), g.runtime())?;
        let sep = if width > 0 { ", " } else { "" };
        let current = width + text::width(sep) as i64;
        let cap = max as i64 - current - text::width("…") as i64;
        if cap <= 0 {
            return Ok(format!("{result}…"));
        }
        let reserve = keys[i + 1..]
            .iter()
            .map(|key| 2 + text::width(&utf8(key)) as i64 + 1 + 4)
            .sum::<i64>();
        let budget = cap.min(max as i64 - current - reserve);
        let value_max = (budget - text::width(&utf8(key)) as i64 - 3).max(1) as usize;
        let piece = format!("{}={}", utf8(key), scalar(g, &value, value_max)?);
        let piece_width = text::width(&piece) as i64;
        if piece_width > budget {
            return Ok(format!(
                "{result}{sep}{}",
                text::truncate(&piece, cap as usize)
            ));
        }
        result.push_str(sep);
        result.push_str(&piece);
        width = current + piece_width;
    }
    Ok(result)
}
struct Tree<'a, 'b> {
    g: &'a DisplayGraph<'b>,
    theme: &'a Theme,
    depth: usize,
    max: usize,
    scalar: usize,
    lines: Vec<String>,
    truncated: bool,
}
impl Tree<'_, '_> {
    fn push(&mut self, line: String) {
        if self.lines.len() >= self.max {
            self.truncated = true
        } else {
            self.lines.push(line)
        }
    }
    fn prefix(&self, ancestors: &[bool]) -> String {
        ancestors
            .iter()
            .map(|next| {
                if *next {
                    format!("{}  ", self.theme.symbol("tree.vertical"))
                } else {
                    "   ".into()
                }
            })
            .collect()
    }
    fn node(
        &mut self,
        value: &ValueHandle,
        key: Option<String>,
        ancestors: &mut Vec<bool>,
        last: bool,
        depth: usize,
    ) -> Result<()> {
        if self.lines.len() >= self.max {
            self.truncated = true;
            return Ok(());
        }
        let connector = self
            .theme
            .symbol(if last { "tree.last" } else { "tree.branch" });
        let prefix = format!(
            "{}{} ",
            self.prefix(ancestors),
            self.theme.fg("dim", connector)
        );
        ancestors.push(!last);
        let result = (|| {
            let heap = self.g.0.heap();
            let kind = heap.kind(value)?;
            if !matches!(kind, ValueKind::Object | ValueKind::Array) {
                let label = self.theme.fg(
                    "muted",
                    key.as_deref().filter(|s| !s.is_empty()).unwrap_or("value"),
                );
                let icon = self.theme.styled_symbol("icon.file", "muted");
                if let ValueSnapshot::String(string) = heap.snapshot(value)? {
                    if string.units().contains(&10) {
                        let string = utf8(&string);
                        let parts = string.split('\n').collect::<Vec<_>>();
                        let count = parts
                            .len()
                            .min(self.max.saturating_sub(self.lines.len() + 1).max(1));
                        let continuation = self.prefix(ancestors);
                        self.push(format!(
                            "{prefix}{icon} {label}: {}",
                            self.theme.fg(
                                "dim",
                                &format!("\"{}", text::truncate(parts[0], self.scalar))
                            )
                        ));
                        for line in &parts[1..count] {
                            if self.lines.len() >= self.max {
                                self.truncated = true;
                                break;
                            }
                            self.push(format!(
                                "{continuation}   {}",
                                self.theme
                                    .fg("dim", &format!(" {}", text::truncate(line, self.scalar)))
                            ))
                        }
                        if parts.len() > count {
                            self.truncated = true;
                            self.push(format!(
                                "{continuation}   {}",
                                self.theme.fg(
                                    "dim",
                                    &format!(" …({} more lines)\"", parts.len() - count)
                                )
                            ))
                        } else if let Some(line) = self.lines.last_mut() {
                            line.push_str(&self.theme.fg("dim", "\""))
                        }
                        return Ok(());
                    }
                }
                self.push(format!(
                    "{prefix}{icon} {label}: {}",
                    self.theme.fg("dim", &scalar(self.g, value, self.scalar)?)
                ));
                return Ok(());
            }
            let array = kind == ValueKind::Array;
            let label = self.theme.fg(
                "muted",
                key.as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(if array { "array" } else { "object" }),
            );
            let icon = self
                .theme
                .styled_symbol(if array { "icon.package" } else { "icon.folder" }, "muted");
            self.push(format!("{prefix}{icon} {label}"));
            // Object.keys belongs after the source object's depth cut-off;
            // preserve that order for observable proxies/property operations.
            if !array && depth >= self.depth {
                self.push(format!(
                    "{}{} {}",
                    self.prefix(ancestors),
                    self.theme.fg("dim", self.theme.symbol("tree.last")),
                    self.theme.fg("dim", "…")
                ));
                return Ok(());
            }
            if array {
                let length = self.g.get(value, "length")?;
                if matches!(heap.snapshot(&length)?,ValueSnapshot::Number(n) if n==0.0) {
                    self.push(format!(
                        "{}{} {}",
                        self.prefix(ancestors),
                        self.theme.fg("dim", self.theme.symbol("tree.last")),
                        self.theme.fg("dim", "[]")
                    ));
                    return Ok(());
                }
                if depth >= self.depth {
                    self.push(format!(
                        "{}{} {}",
                        self.prefix(ancestors),
                        self.theme.fg("dim", self.theme.symbol("tree.last")),
                        self.theme.fg("dim", "…")
                    ));
                    return Ok(());
                }
                let mut i = 0u64;
                while (i as f64)
                    < heap.to_number(&self.g.get(value, "length")?, self.g.runtime())?
                {
                    let child = heap.get(
                        value,
                        &WireString::from(i.to_string()).into(),
                        self.g.runtime(),
                    )?;
                    let last = (i as f64)
                        == heap.to_number(&self.g.get(value, "length")?, self.g.runtime())? - 1.0;
                    self.node(&child, Some(format!("[{i}]")), ancestors, last, depth + 1)?;
                    if self.lines.len() >= self.max {
                        self.truncated = true;
                        return Ok(());
                    }
                    i += 1;
                }
                return Ok(());
            }
            let keys = heap.own_enumerable_string_keys(value, self.g.runtime())?;
            if keys.is_empty() {
                let body = "{}";
                self.push(format!(
                    "{}{} {}",
                    self.prefix(ancestors),
                    self.theme.fg("dim", self.theme.symbol("tree.last")),
                    self.theme.fg("dim", body)
                ));
                return Ok(());
            }
            for (i, key) in keys.iter().enumerate() {
                let child = heap.get(value, &key.clone().into(), self.g.runtime())?;
                self.node(
                    &child,
                    Some(if array {
                        format!("[{}]", utf8(key))
                    } else {
                        utf8(key)
                    }),
                    ancestors,
                    i + 1 == keys.len(),
                    depth + 1,
                )?;
                if self.lines.len() >= self.max {
                    self.truncated = true;
                    return Ok(());
                }
            }
            Ok(())
        })();
        ancestors.pop();
        result
    }
}
fn tree(
    g: &DisplayGraph<'_>,
    value: &ValueHandle,
    theme: &Theme,
    expanded: bool,
    hidden: &ValueHandle,
) -> Result<(Vec<String>, bool)> {
    let mut tree = Tree {
        g,
        theme,
        depth: if expanded { 6 } else { 2 },
        max: if expanded { 200 } else { 6 },
        scalar: if expanded { 2000 } else { 60 },
        lines: Vec::new(),
        truncated: false,
    };
    let heap = g.0.heap();
    let kind = heap.kind(value)?;
    if kind == ValueKind::Object {
        for key in visible_keys(g, value, hidden)? {
            let child = heap.get(value, &key.clone().into(), g.runtime())?;
            tree.node(&child, Some(utf8(&key)), &mut Vec::new(), true, 1)?;
            if tree.lines.len() >= tree.max {
                tree.truncated = true;
                break;
            }
        }
    } else if kind == ValueKind::Array {
        let mut i = 0u64;
        while (i as f64) < heap.to_number(&g.get(value, "length")?, g.runtime())? {
            let child = heap.get(value, &WireString::from(i.to_string()).into(), g.runtime())?;
            let last = (i as f64) == heap.to_number(&g.get(value, "length")?, g.runtime())? - 1.0;
            tree.node(&child, Some(format!("[{i}]")), &mut Vec::new(), last, 1)?;
            if tree.lines.len() >= tree.max {
                tree.truncated = true;
                break;
            }
            i += 1
        }
    } else {
        tree.node(value, None, &mut Vec::new(), true, 0)?
    }
    Ok((tree.lines, tree.truncated))
}
#[derive(Clone)]
struct GenericCard {
    label: WireString,
    args: ValueHandle,
    text: Option<WireString>,
    error: bool,
    options: RenderOptions,
    hidden: ValueHandle,
    cache: Option<(usize, Vec<String>)>,
}
impl GenericCard {
    fn render(&mut self, g: &DisplayGraph<'_>, width: usize, theme: &Theme) -> Result<Vec<String>> {
        if let Some((prior, rows)) = &self.cache {
            if *prior == width {
                return Ok(rows.clone());
            }
        }
        let content_width = width.saturating_sub(2).max(1);
        let options = self.options;
        let icon = if options.is_partial {
            if options.spinner_frame.is_some() {
                "running"
            } else {
                "pending"
            }
        } else if self.error {
            "error"
        } else {
            "done"
        };
        let mut lines = vec![super::output_block::status_line(
            &utf8(&self.label),
            "",
            Some(&theme.status(icon, options.spinner_frame)),
            theme,
        )];
        if !options.expanded
            && record(g, &self.args)?
            && !g
                .0
                .heap()
                .own_enumerable_string_keys(&self.args, g.runtime())?
                .is_empty()
        {
            let preview = inline(
                g,
                &self.args,
                content_width
                    .saturating_sub(text::width(theme.symbol("tree.last")) + 2)
                    .max(20),
                &self.hidden,
            )?;
            if !preview.is_empty() {
                lines.push(format!(
                    " {} {}",
                    theme.fg("dim", theme.symbol("tree.last")),
                    theme.fg("dim", &preview)
                ))
            }
        }
        if options.expanded && g.0.heap().kind(&self.args)? != ValueKind::Undefined {
            lines.push("".into());
            lines.push(theme.fg("dim", "Args"));
            let (rows, truncated) = tree(g, &self.args, theme, true, &self.hidden)?;
            lines.extend(rows);
            if truncated {
                lines.push(theme.fg("dim", "…"))
            }
            lines.push("".into());
        }
        if let Some(output) = &self.text {
            let output = output.units();
            let end = output
                .iter()
                .rposition(|u| !char::from_u32(*u as u32).is_some_and(ara_prompt::js::is_space))
                .map_or(0, |i| i + 1);
            let output = WireString::from_units(output[..end].to_vec());
            if output.is_empty() {
                lines.push(theme.fg("dim", "(no output)"))
            } else {
                let mut json_rendered = false;
                if output
                    .units()
                    .first()
                    .is_some_and(|u| matches!(u, 123 | 91))
                {
                    let parsed_tree = (|| {
                        let json = g.global("JSON")?;
                        let parse = g.get(&json, "parse")?;
                        let parsed = g.0.heap().call(
                            &parse,
                            &json,
                            &[g.0.heap().string(output.clone())?],
                            g.runtime(),
                        )?;
                        tree(g, &parsed, theme, options.expanded, &self.hidden)
                    })();
                    match parsed_tree {
                        Ok((rows, truncated)) => {
                            if !rows.is_empty() {
                                lines.extend(rows);
                                if !options.expanded {
                                    lines.push(text::expand_hint(theme))
                                } else if truncated {
                                    lines.push(theme.fg("dim", "…"))
                                }
                                json_rendered = true
                            }
                        }
                        Err(error) if error.is_native_exception() => {}
                        Err(error) => return Err(error),
                    }
                }
                if !json_rendered {
                    let string = utf8(&output);
                    let parts = string.split('\n').collect::<Vec<_>>();
                    let max = if options.expanded { 12 } else { 4 };
                    for line in parts.iter().take(max) {
                        lines.push(theme.fg(
                            "toolOutput",
                            &text::truncate(&text::replace_tabs(line), content_width),
                        ))
                    }
                    if parts.len() > max {
                        lines.push(format!(
                            "{} {}",
                            theme.fg("dim", &format!("… {} more lines", parts.len() - max)),
                            text::expand_hint(theme)
                        ))
                    } else if !options.expanded {
                        lines.push(text::expand_hint(theme))
                    }
                }
            }
        }
        // Original WidthAwareText(1,1), ignoreTight=true, unbordered tinted Text.
        let bg = theme.bg_ansi(if options.is_partial {
            "toolPendingBg"
        } else if self.error {
            "toolErrorBg"
        } else {
            "toolSuccessBg"
        });
        let mut rows = vec![format!("{bg}{}\x1b[49m", " ".repeat(width))];
        // Text wraps the complete formatted content, carrying ANSI state across
        // line breaks. Its background consumer reopens nested background resets.
        for row in text::wrap(&text::replace_tabs(&lines.join("\n")), content_width) {
            let margins = format!(" {row} ");
            let pad = " ".repeat(width.saturating_sub(text::width(&margins)));
            let painted = format!("{margins}{pad}").replace("\x1b[49m", &format!("\x1b[49m{bg}"));
            rows.push(format!("{bg}{painted}\x1b[49m"));
        }
        rows.push(format!("{bg}{}\x1b[49m", " ".repeat(width)));
        self.cache = Some((width, rows.clone()));
        Ok(rows)
    }
}
#[derive(Clone)]
enum View {
    Empty,
    Text(WireString),
    /// Dedicated original plain Text preserves its status-line ANSI directly.
    ReportCall(WireString),
    Original(Vec<ValueHandle>),
    Generic(GenericCard),
    AstCall(super::github_renderer::Component),
}
#[derive(Clone)]
pub(crate) struct XdevComponent {
    view: View,
    option_roots: Vec<(ValueHandle, Option<bool>)>,
    queued: bool,
}
pub(crate) fn could_be_device(path: &WireString) -> bool {
    const PREFIX: &[u16] = &[120, 100, 58, 47, 47];
    let lower = path
        .units()
        .iter()
        .map(|u| if *u == 88 || *u == 68 { *u + 32 } else { *u })
        .collect::<Vec<_>>();
    if lower.len() <= PREFIX.len() {
        PREFIX.starts_with(&lower)
    } else {
        lower.starts_with(PREFIX)
    }
}

#[cfg(test)]
#[test]
fn report_issue_call_preview_grouped(){
    let realm=crate::native_realm::NativeRealm::prepare().unwrap();let callbacks=realm.begin_display_callbacks().unwrap();let runtime=callbacks.runtime().unwrap();let graph=DisplayGraph(&realm.values,&runtime);let heap=realm.values.heap();let theme=Arc::new(Theme::dark());
    for (content,expected) in [(serde_json::json!("  read:\tissue\nsecond line  "),"read:   issue"),(serde_json::json!({"bad":"object"}),""),(serde_json::json!("x".repeat(100)),"x")]{
        let args=super::write_renderer::args_from_graph(&graph,&heap.import_serde(&serde_json::json!({"path":"xd://report_issue","content":content})).unwrap()).unwrap();
        for started in [false,true]{
            let mut card=XdevComponent::build(&graph,&args,None,RenderOptions{is_partial:true,..Default::default()},started,false,&theme,None).unwrap().unwrap();let rows=card.render(&graph,160,&theme).unwrap();let plain=rows.iter().map(|r|text::sanitize(&r.as_str().into())).collect::<Vec<_>>().join("\n");
            assert!(plain.contains("Report Tool Issue")&&plain.contains(expected),"{plain}");assert!(!plain.contains("queued")&&!plain.contains("second line")&&!plain.contains("bad="),"{plain}");assert!(!plain.contains(&"x".repeat(73)));assert_eq!(card.render(&graph,160,&theme).unwrap(),rows);
        }
    }
    let args=super::write_renderer::args_from_graph(&graph,&heap.import_serde(&serde_json::json!({"path":"xd://report_issue"})).unwrap()).unwrap();let mut card=XdevComponent::build(&graph,&args,None,RenderOptions::default(),false,false,&theme,None).unwrap().unwrap();assert!(card.render(&graph,80,&theme).unwrap().is_empty());
    println!("report-issue-renderer:pass:plain-firstline-tabs72-pending-nonstring-unsettled");
}
static TEXT_BLOCK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da xdev.ts:619 content.map(block=>block.type==='text'?block.text:'')",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, args, runtime| {
        let block = args.first().cloned().unwrap_or(heap.undefined()?);
        let kind = heap.get(&block, &"type".into(), runtime)?;
        if matches!(heap.snapshot(&kind)?,ValueSnapshot::String(kind) if kind.equals_ascii("text"))
        {
            heap.get(&block, &"text".into(), runtime)
        } else {
            heap.string("".into())
        }
    },
};
fn result_text(g: &DisplayGraph<'_>, result: &ValueHandle) -> Result<WireString> {
    let heap = g.0.heap();
    let content = g.get(result, "content")?;
    let map = g.get(&content, "map")?;
    let callback = heap.fresh_compiled_function(&TEXT_BLOCK, &[])?;
    let mapped = heap.call(&map, &content, &[callback], g.runtime())?;
    let filter = g.get(&mapped, "filter")?;
    let filtered = heap.call(&filter, &mapped, &[g.global("Boolean")?], g.runtime())?;
    let join = g.get(&filtered, "join")?;
    g.string(&heap.call(&join, &filtered, &[heap.string("\n".into())?], g.runtime())?)
}
fn label(
    g: &DisplayGraph<'_>,
    name: &WireString,
    mounted: Option<&ValueHandle>,
    mcp: bool,
) -> Result<WireString> {
    if let Some(tool) = mounted {
        let label = g.get(tool, "label")?;
        if (mcp && g.truth(&label)?) || (!mcp && !g.nullish(&label)?) {
            return g.0.heap().to_string(&label, g.runtime());
        }
    }
    if mcp {
        let units = name.units();
        if units.starts_with(&[109, 99, 112, 95, 95]) {
            if let Some(index) = units[5..].iter().position(|u| *u == 95) {
                let index = index + 5;
                let mut value = units[5..index].to_vec();
                value.push(47);
                value.extend_from_slice(&units[index + 1..]);
                return Ok(WireString::from_units(value));
            }
        }
    }
    Ok(name.clone())
}
pub(crate) fn options_value(
    g: &DisplayGraph<'_>,
    options: RenderOptions,
    started: bool,
    complete: bool,
) -> Result<ValueHandle> {
    let heap = g.0.heap();
    let value = heap.object()?;
    for (key, flag) in [
        ("expanded", options.expanded),
        ("isPartial", options.is_partial),
        ("executionStarted", started),
        ("argsComplete", complete),
    ] {
        heap.define_own_data(&value, key.into(), heap.boolean(flag)?, g.runtime())?;
    }
    let spinner = options
        .spinner_frame
        .map(|v| heap.number(v as f64))
        .transpose()?
        .unwrap_or(heap.undefined()?);
    heap.define_own_data(&value, "spinnerFrame".into(), spinner, g.runtime())?;
    Ok(value)
}
fn refresh_options(
    g: &DisplayGraph<'_>, value:&ValueHandle, options:RenderOptions, started:bool, complete:bool,
) -> Result<()> {
    let heap=g.0.heap();
    for (key,flag) in [("expanded",options.expanded),("isPartial",options.is_partial),("executionStarted",started),("argsComplete",complete)] {
        heap.assign(value,key.into(),heap.boolean(flag)?,g.runtime())?;
    }
    let spinner=options.spinner_frame.map(|frame|heap.number(frame as f64)).transpose()?.unwrap_or(heap.undefined()?);
    heap.assign(value,"spinnerFrame".into(),spinner,g.runtime())?;
    Ok(())
}
fn callback_parts(
    g: &DisplayGraph<'_>,
    tool: &ValueHandle,
    args: &ValueHandle,
    result: Option<&ValueHandle>,
    options: RenderOptions,
    started: bool,
    complete: bool,
    theme: &Arc<Theme>,
    option_root: &ValueHandle,
    option_roots: &mut Vec<(ValueHandle, Option<bool>)>,
) -> Result<(bool,Option<Vec<ValueHandle>>)> {
    // Original own callback short-circuit test before static lookup. Retain the
    // exact receiver and every original returned Component; no serde adapter.
    let call = g.get(tool, "renderCall")?;
    let present = g.truth(&call)? || g.truth(&g.get(tool, "renderResult")?)?;
    if !present {
        return Ok((false,None));
    }
    let theme = g.device_theme(theme)?;
    let mut parts = Vec::new();
    if let Some(result) = result {
        let render_result = g.get(tool, "renderResult")?;
        if g.truth(&render_result)? {
            if !g.truth(&g.get(tool, "mergeCallAndResult")?)? {
                let call = g.get(tool, "renderCall")?;
                if g.truth(&call)? {
                    let call_options = options_value(
                        g,
                        RenderOptions {
                            is_partial: false,
                            ..options
                        },
                        started,
                        complete,
                    )?;
                    option_roots.push((call_options.clone(),Some(false)));
                    let value = g.0.heap().call(
                        &call,
                        tool,
                        &[args.clone(), call_options, theme.clone()],
                        g.runtime(),
                    )?;
                    if g.truth(&value)? {
                        parts.push(value)
                    }
                }
            }
            let value = g.0.heap().call(
                &render_result,
                tool,
                &[result.clone(), option_root.clone(), theme, args.clone()],
                g.runtime(),
            )?;
            if g.truth(&value)? {
                parts.push(value)
            }
            if !parts.is_empty() {
                return Ok((true,Some(parts)));
            }
        }
    } else {
        let call = g.get(tool, "renderCall")?;
        if g.truth(&call)? {
            let value = g.0.heap().call(
                &call,
                tool,
                &[args.clone(), option_root.clone(), theme],
                g.runtime(),
            )?;
            return Ok((true,Some(if g.truth(&value)? {
                vec![value]
            } else {
                Vec::new()
            })));
        }
    }
    Ok((true,None))
}
impl XdevComponent {
    pub(crate) fn build(
        g: &DisplayGraph<'_>,
        args: &WriteArgs,
        result: Option<&ValueHandle>,
        options: RenderOptions,
        started: bool,
        complete: bool,
        theme: &Arc<Theme>,
        retained_options: Option<&ValueHandle>,
    ) -> Result<Option<Self>> {
        let heap = g.0.heap();
        let option_root=if let Some(value)=retained_options {value.clone()}else{options_value(g,options,started,complete)?};
        if retained_options.is_some(){refresh_options(g,&option_root,options,started,complete)?;}
        let mut option_roots=vec![(option_root.clone(),None)];
        let dispatch = if let Some(result) = result {
            let details = g.get(result, "details")?;
            if g.nullish(&details)? {
                None
            } else {
                let value = g.get(&details, "xdev")?;
                if g.truth(&value)? { Some(value) } else { None }
            }
        } else {
            None
        };
        let (name, inner_args, text, error) = if let Some(dispatch) = &dispatch {
            let result = result.unwrap();
            let text = result_text(g, result)?;
            if g.is(dispatch, "mode", "help")? {
                return Ok(Some(Self {
                    view: if text.is_empty() {
                        View::Empty
                    } else {
                        View::Text(text)
                    },
                    option_roots, queued:false,
                }));
            }
            let name = g.string(&g.get(dispatch, "tool")?)?;
            let args = g.get(dispatch, "args")?;
            let args = if g.nullish(&args)? {
                heap.object()?
            } else {
                args
            };
            (name, args, Some(text), g.truth(&g.get(result, "isError")?)?)
        } else {
            // A WRITE result delegates only with details.xdev. An outer
            // denial without dispatch retains its ordinary WRITE error.
            if result.is_some() {
                return Ok(None);
            }
            let Some(path) = args
                .path
                .as_ref()
                .filter(|p| !p.is_empty() && could_be_device(p))
            else {
                return Ok(None);
            };
            let Some(name) =
                crate::actual_tools::xdev_native::parse_xd(path).filter(|s| !s.is_empty())
            else {
                return Ok(Some(Self { view: View::Empty, option_roots, queued:false }));
            };
            if !args.content_present {
                return Ok(Some(Self { view: View::Empty, option_roots, queued:false }));
            }
            // Fixed write.ts1637 / report-tool-issue.ts57: the dedicated
            // plain-text preview precedes generic JSON and queued devices.
            if name.equals_ascii(crate::report_tool_issue::DEVICE_NAME){
                let body=if args.content_is_string{utf8(&args.content.0).trim_matches(ara_prompt::js::is_space).split('\n').next().unwrap_or("").to_owned()}else{String::new()};
                let body=text::truncate(&text::replace_tabs(&body),72);
                let line=super::output_block::status_line("Report Tool Issue",&body,Some(&theme.status("pending",None)),theme);
                return Ok(Some(Self{view:View::ReportCall(line.into()),option_roots,queued:false}));
            }
            let has_raw = args.content_is_string && !args.content.0.is_empty();
            let parsed = if has_raw {
                super::write_renderer::parse_graph_args(g, &args.content.0)?
            } else {
                heap.object()?
            };
            let decoded = if matches!(heap.kind(&parsed)?, ValueKind::Object | ValueKind::Array) {
                heap.spread(&parsed, g.runtime())?
            } else {
                heap.object()?
            };
            if has_raw {
                heap.define_own_data(
                    &decoded,
                    "__partialJson".into(),
                    heap.string(args.content.0.clone())?,
                    g.runtime(),
                )?;
            }
            (name, decoded, None, false)
        };
        let mounted = g.mounted_tool(&name)?;
        if dispatch.is_none() && !started {
            let label = crate::actual_tools::xdev_native::concat(&[
                "queued ".into(),
                label(g, &name, mounted.as_ref(), true)?,
            ]);
            let display_args = heap.spread(&inner_args, g.runtime())?;
            heap.delete_own(&display_args, &"__partialJson".into(), g.runtime())?;
            return Ok(Some(Self {
                view: View::Generic(GenericCard {
                    label,
                    args: display_args,
                    text: None,
                    error: false,
                    options: RenderOptions {
                        is_partial: true,
                        spinner_frame: None,
                        ..options
                    },
                    hidden: hidden(g)?,
                    cache: None,
                }),
                option_roots, queued:true,
            }));
        }
        let inner_result = if let Some(dispatch) = &dispatch {
            let result = result.unwrap();
            let inner = heap.object()?;
            for (key, value) in [
                ("content", g.get(result, "content")?),
                ("details", g.get(dispatch, "inner")?),
                ("isError", g.get(result, "isError")?),
            ] {
                heap.define_own_data(&inner, key.into(), value, g.runtime())?;
            }
            Some(inner)
        } else {
            None
        };
        let mut allow_static=true;
        if let Some(tool) = &mounted {
            let (selected,parts) = callback_parts(
                g,
                tool,
                &inner_args,
                inner_result.as_ref(),
                options,
                started,
                complete,
                theme,
                &option_root,
                &mut option_roots,
            )?;
            allow_static=!selected;
            if let Some(parts)=parts {
                return Ok(Some(Self {
                    view: View::Original(parts),
                    option_roots, queued:false,
                }));
            }
        }
        // Fixed own callbacks take precedence; the original static Github
        // renderer supplier is consulted only after their no-output fallback.
        if allow_static && name.equals_ascii("github") {
            let renderer=g.github_renderer()?;
            if let Some(parts)=callback_parts(g,&renderer,&inner_args,inner_result.as_ref(),options,started,complete,theme,&option_root,&mut option_roots)?.1 {
                return Ok(Some(Self{view:View::Original(parts),option_roots,queued:false}));
            }
        }
        if allow_static && name.equals_ascii("ast_edit") && inner_result.is_none() {
            let ui_theme=g.device_theme(theme)?;
            let component=super::ast_edit_renderer::call(g,&inner_args,&ui_theme)?;
            return Ok(Some(Self{view:View::AstCall(component),option_roots,queued:false}));
        }
        // Other original static renderer entries remain visible integration gaps.
        Ok(Some(Self {
            view: View::Generic(GenericCard {
                label: label(g, &name, mounted.as_ref(), false)?,
                args: inner_args,
                text,
                error,
                options,
                hidden: hidden(g)?,
                cache: None,
            }),
            option_roots, queued:false,
        }))
    }
    pub(crate) fn update_options(&mut self,g:&DisplayGraph<'_>,options:RenderOptions,started:bool,complete:bool)->Result<()> {
        for (value,partial) in &self.option_roots {
            refresh_options(g,value,RenderOptions{is_partial:partial.unwrap_or(options.is_partial),..options},started,complete)?;
        }
        if let View::Generic(card)=&mut self.view {
            let options=if self.queued {RenderOptions{is_partial:true,spinner_frame:None,..options}}else{options};
            if card.options!=options {card.options=options;card.cache=None;}
        }
        Ok(())
    }
    pub(crate) fn render(
        &mut self,
        g: &DisplayGraph<'_>,
        width: usize,
        theme: &Theme,
    ) -> Result<Vec<String>> {
        match &mut self.view {
            View::Empty => Ok(Vec::new()),
            View::ReportCall(line)=>Ok(text::wrap(&utf8(line),width)),
            View::Text(text) => Ok(text::wrap(
                &theme.fg("toolOutput", &text::replace_tabs(&utf8(text))),
                width,
            )),
            View::Generic(card) => card.render(g, width, theme),
            View::AstCall(component) => {
                let rows=component.render(g,g.0.heap().number(width as f64)?)?;
                g.members(&rows)?.iter().map(|row|g.string(row).map(|value|utf8(&value))).collect()
            },
            View::Original(parts) => {
                let mut rows = Vec::new();
                for component in parts {
                    let render = g.get(component, "render")?;
                    let value = g.0.heap().call(
                        &render,
                        component,
                        &[g.0.heap().number(width as f64)?],
                        g.runtime(),
                    )?;
                    for row in g.members(&value)? {
                        rows.push(utf8(&g.string(&row)?))
                    }
                }
                Ok(rows)
            }
        }
    }
    pub(crate) fn invalidate(&mut self, g: &DisplayGraph<'_>) -> Result<()> {
        match &mut self.view {
            View::Generic(card) => {
                card.cache = None;
                Ok(())
            }
            View::AstCall(component) => {component.invalidate();Ok(())},
            View::Original(parts) => {
                for component in parts {
                    let invalidate = g.get(component, "invalidate")?;
                    g.0.heap().call(&invalidate, component, &[], g.runtime())?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}


#[cfg(test)]
fn exercise_github_renderer_source_observations(g:&DisplayGraph<'_>,theme:&Arc<Theme>){
    let heap=g.0.heap();let renderer=g.github_renderer().unwrap();let theme=g.device_theme(theme).unwrap();
    let call=g.get(&renderer,"renderCall").unwrap();let result=g.get(&renderer,"renderResult").unwrap();
    let options=heap.import_serde(&serde_json::json!({"expanded":false,"isPartial":false})).unwrap();
    let render=|component:&ValueHandle,width:f64|{
        let rows=g.method(component,"render",&[heap.number(width).unwrap()]).unwrap();
        let plain=g.members(&rows).unwrap().into_iter().map(|row|text::sanitize(&g.string(&row).unwrap())).collect::<Vec<_>>().join("\n");(rows,plain)
    };
    // Actual source renderer and original Graph Component. This authored
    // observation is not the original local throwing fixture or original22.
    let args=heap.import_serde(&serde_json::json!({"op":"repo_view"})).unwrap();
    let error=heap.import_serde(&serde_json::json!({"content":[{"type":"text","text":"gh: Not Found (HTTP 404)"}],"isError":true})).unwrap();
    let component=heap.call(&result,&renderer,&[error,options.clone(),theme.clone(),args],g.runtime()).unwrap();
    let (first,plain)=render(&component,80.0);
    assert!(plain.contains("GitHub Repo")&&plain.contains("gh: Not Found (HTTP 404)")&&!plain.contains("Write"),"{plain}");
    assert!(g.truth(&heap.get(&component,&heap.module_symbol("OMP596f2da tui/output-block.ts","framedBlockComponent").unwrap(),g.runtime()).unwrap()).unwrap());
    let (again,_)=render(&component,80.0);assert!(again.same_identity(&first),"framed cache returns original array");
    assert!(!g.string(&g.members(&first).unwrap()[0]).unwrap().units().windows(5).any(|u|u==[27,91,52,57,109]),"applyBg=false must not emit inherited background reset");
    for (op,title)in [("repo_view","GitHub Repo"),("pr_checkout","GitHub PR Checkout"),("pr_push","GitHub PR Push"),("search_issues","GitHub Search Issues"),("search_prs","GitHub Search PRs"),("search_code","GitHub Search Code"),("search_commits","GitHub Search Commits"),("search_repos","GitHub Search Repos"),("unknown","GitHub")]{
        let args=heap.import_serde(&serde_json::json!({"op":op,"pr":["123","https://host/pull/42","x","y"],"repo":"owner/repo","branch":"main","query":"needle"})).unwrap();
        let component=heap.call(&call,&renderer,&[args,options.clone(),theme.clone()],g.runtime()).unwrap();
        let(_,plain)=render(&component,120.0);assert!(plain.contains(title),"{op}: {plain}");
        if op=="pr_checkout"||op=="pr_push"{assert!(plain.contains("#123, #42, x, +1 more"),"{plain}")}
    }
    let watch_args=heap.import_serde(&serde_json::json!({"op":"run_watch","run":"123"})).unwrap();
    heap.define_own_data(&options,"spinnerFrame".into(),heap.number(0.0).unwrap(),g.runtime()).unwrap();
    let component=heap.call(&call,&renderer,&[watch_args,options.clone(),theme.clone()],g.runtime()).unwrap();
    let(_,plain)=render(&component,80.0);assert!(plain.contains("GitHub Run Watch")&&plain.contains("#123")&&plain.contains("waiting for workflow data..."),"{plain}");
    let watch=heap.import_serde(&serde_json::json!({"content":[],"details":{"watch":{"mode":"commit","state":"watching","repo":"owner/repo","headSha":"1234567890abcdef","note":"watch note","runs":[{"id":1,"workflowName":"CI","branch":"main","jobs":[{"name":"ok","conclusion":"success","durationSeconds":2},{"name":"bad","conclusion":"failure"},{"name":"running","status":"in_progress"},{"name":"queued","status":"queued"},{"name":"unknown"}]}],"failedLogs":[{"workflowName":"CI","runId":1,"jobName":"bad","available":true,"tail":"one\ntwo\nthree\nfour"},{"runId":2,"jobName":"missing","available":false}]}}})).unwrap();
    let component=heap.call(&result,&renderer,&[watch,options.clone(),theme.clone()],g.runtime()).unwrap();
    let(_,plain)=render(&component,80.0);assert!(plain.contains("1234567890ab")&&plain.contains("failed logs")&&plain.contains("1 more log lines")&&plain.contains("log tail unavailable"),"{plain}");
    heap.define_own_data(&options,"expanded".into(),heap.boolean(true).unwrap(),g.runtime()).unwrap();
    let(_,expanded)=render(&component,80.0);assert!(expanded.contains("one")&&!expanded.contains("more log lines"),"{expanded}");
    let _=render(&component,8.0);let _=render(&component,0.0);
    for error in [false,true]{
        let output=heap.import_serde(&serde_json::json!({"content":[],"isError":error})).unwrap();
        let component=heap.call(&result,&renderer,&[output,options.clone(),theme.clone()],g.runtime()).unwrap();
        let(_,plain)=render(&component,80.0);assert!(plain.contains(if error{"request failed"}else{"no output"}),"{plain}");
    }
    let output=heap.import_serde(&serde_json::json!({"content":[{"type":"text","text":"ok"}]})).unwrap();
    let component=heap.call(&result,&renderer,&[output,options.clone(),theme.clone()],g.runtime()).unwrap();
    assert!(!g.truth(&heap.get(&component,&heap.module_symbol("OMP596f2da tui/output-block.ts","framedBlockComponent").unwrap(),g.runtime()).unwrap()).unwrap(),"one-line success stays original plain Text");
    let(_,plain)=render(&component,80.0);assert!(plain.contains("ok"));
}

/// Supplemental source-consumer checks joined into the existing coarse native
/// protocol entrance. These are not the original22 process or renderer4 gate;
/// they supply no fake mounted producer or production WebSearch/Debug supplier.
#[cfg(test)]
pub(crate) fn observe_renderer_protocol_edges() {
    super::ast_edit_renderer::observe_original_call();
    let realm = crate::native_realm::NativeRealm::prepare().unwrap();
    let callbacks = realm.begin_display_callbacks().unwrap();
    let runtime = callbacks.runtime().unwrap();
    let graph = DisplayGraph(&realm.values, &runtime);
    let heap = realm.values.heap();
    let theme = Arc::new(Theme::dark());
    // Fixed exposed Theme methods retain receiver brands, optional language,
    // and source interpolation. This stays in the existing coarse entrance.
    eprintln!("[xdev-debug] renderer domain ready / theme begin");
    exercise_github_renderer_source_observations(&graph,&theme);
    let theme_value = graph.device_theme(&theme).unwrap();
    let fg = graph.get(&theme_value, "fg").unwrap();
    let color = heap.string("accent".into()).unwrap();
    let numeric = heap.number(7.0).unwrap();
    let rendered = heap.call(&fg, &theme_value, &[color.clone(), numeric], &runtime).unwrap();
    assert_eq!(graph.string(&rendered).unwrap(), WireString::from(theme.fg("accent", "7")));
    assert!(matches!(heap.call(&fg, &heap.object().unwrap(), &[color, heap.string("x".into()).unwrap()], &runtime), Err(ValueError::TypeError(_))));
    let lang = graph.get(&theme_value, "getLangIcon").unwrap();
    for argument in [None, Some(heap.undefined().unwrap()), Some(heap.boolean(false).unwrap())] {
        let arguments = argument.into_iter().collect::<Vec<_>>();
        assert_eq!(graph.string(&heap.call(&lang, &theme_value, &arguments, &runtime).unwrap()).unwrap(), WireString::from(theme.language_icon(None)));
    }
    assert!(matches!(heap.call(&lang, &theme_value, &[heap.number(1.0).unwrap()], &runtime), Err(ValueError::TypeError(_))));
    let symbol = graph.get(&theme_value, "symbol").unwrap();
    for key in [heap.string("authored-missing-symbol".into()).unwrap(), heap.symbol(None).unwrap()] {
        assert_eq!(heap.kind(&heap.call(&symbol,&theme_value,&[key],&runtime).unwrap()).unwrap(), ValueKind::Undefined);
    }
    let error=heap.call(&fg,&theme_value,&[heap.string("authored-missing-color".into()).unwrap(),heap.string("x".into()).unwrap()],&runtime).unwrap_err();
    let ValueError::Thrown(error)=error else {panic!("fixed original Error constructor")};
    assert!(graph.string(&graph.get(&error,"message").unwrap()).unwrap().equals_ascii("Unknown theme color: authored-missing-color"));

    eprintln!("[xdev-debug] renderer theme done / custom spinner begin");
    // Authored custom callback fixture: closures retain the same original
    // options handle across paints; constructor count must stay one.
    static RENDER_SPINNER: CompiledFunctionSpec = CompiledFunctionSpec {
        source:"authored original custom component reads captured options.spinnerFrame",name:"render",length:1,form:CompiledFunctionForm::Ordinary,capture_count:1,
        body:|heap,captures,_,_,runtime|{
            let frame=heap.get(&captures[0],&"spinnerFrame".into(),runtime)?;
            let rows=heap.array_with_length(1)?;
            heap.define_own_data(&rows,"0".into(),heap.string(heap.to_string(&frame,runtime)?)?,runtime)?;
            Ok(rows)
        },
    };
    static BUILD_SPINNER: CompiledFunctionSpec = CompiledFunctionSpec {
        source:"authored original custom renderCall retains options and counts construction",name:"renderCall",length:3,form:CompiledFunctionForm::Ordinary,capture_count:1,
        body:|heap,captures,_,args,runtime|{
            let calls=heap.to_number(&heap.get(&captures[0],&"calls".into(),runtime)?,runtime)?;
            heap.define_own_data(&captures[0],"calls".into(),heap.number(calls+1.0)?,runtime)?;
            let component=heap.object()?;
            heap.define_own_data(&component,"render".into(),heap.fresh_compiled_function(&RENDER_SPINNER,&[args[1].clone()])?,runtime)?;
            Ok(component)
        },
    };
    let constructions=heap.import_serde(&serde_json::json!({"calls":0})).unwrap();
    let tool=heap.object().unwrap();
    heap.define_own_data(&tool,"renderCall".into(),heap.fresh_compiled_function(&BUILD_SPINNER,&[constructions.clone()]).unwrap(),&runtime).unwrap();
    let initial=RenderOptions{is_partial:true,spinner_frame:Some(0),..Default::default()};
    let option_root=options_value(&graph,initial,true,true).unwrap();
    let mut roots=vec![(option_root.clone(),None)];
    let parts=callback_parts(&graph,&tool,&heap.object().unwrap(),None,initial,true,true,&theme,&option_root,&mut roots).unwrap().1.unwrap();
    let mut component=XdevComponent{view:View::Original(parts),option_roots:roots,queued:false};
    for frame in [0,1,2] {
        component.update_options(&graph,RenderOptions{spinner_frame:Some(frame),..initial},true,true).unwrap();
        assert!(component.option_roots[0].0.same_identity(&option_root));
        assert_eq!(component.render(&graph,80,&theme).unwrap(),vec![frame.to_string()]);
    }
    assert_eq!(heap.to_number(&graph.get(&constructions,"calls").unwrap(),&runtime).unwrap(),1.0);
    let options = RenderOptions { is_partial: true, ..Default::default() };
    let args = |value| {
        super::write_renderer::args_from_graph(&graph, &heap.import_serde(&value).unwrap()).unwrap()
    };
    eprintln!("[xdev-debug] renderer custom spinner done / hidden+queued begin");
    let incomplete = args(serde_json::json!({"path":"xd://ast_e"}));
    let mut hidden = XdevComponent::build(&graph, &incomplete, None, options, false, false, &theme, None)
        .unwrap().unwrap();
    assert!(hidden.render(&graph, 80, &theme).unwrap().is_empty());

    let mcp = args(serde_json::json!({"path":"xd://mcp__ecoport_search","content":"{\"action\":\"grep_all\",\"pattern\":\"Broken\",\"scope\":\"game.StarterPlayer\",\"studio\":\"AED Content Development\",\"maxResults\":20}"}));
    let mut queued = XdevComponent::build(&graph, &mcp, None, options, false, true, &theme, None)
        .unwrap().unwrap();
    let rows = queued.render(&graph, 120, &theme).unwrap();
    let plain = rows.iter().map(|row| text::sanitize(&row.as_str().into())).collect::<Vec<_>>().join("\n");
    assert!(plain.contains("queued") && plain.contains("ecoport/search") && plain.contains("Broken"), "{plain}");
    assert_eq!(queued.render(&graph, 120, &theme).unwrap(), rows);

    // Original B-47706ffcce, fixed write-xdev-dispatch.test.ts:343-365.
    // Its plain render-only payload requires no MCP connection or supplier.
    eprintln!("[original22] B-47706ffcce begin");
    let original_args = heap.import_serde(&serde_json::json!({"path":"xd://mcp__ecoport_search","content":"{\"action\":\"grep_all\",\"pattern\":\"Broken\",\"scope\":\"game.StarterPlayer\",\"studio\":\"AED Content Development\",\"maxResults\":20}"})).unwrap();
    let original_write_args = super::write_renderer::args_from_graph(&graph, &original_args).unwrap();
    let original_component = XdevComponent::build(&graph, &original_write_args, None, options, false, false, &theme, None).unwrap();
    assert!(original_component.is_some(), "B-47706ffcce original queued component is defined");
    let direct_rows = original_component.unwrap().render(&graph, 120, &theme).unwrap();
    // The production WRITE Host calls that exact Component constructor from
    // rebuild_xdev_in; require its render to match before using original assertions.
    let mut original_write = super::tool_execution::ToolExecutionComponent::new(
        "B-47706ffcce".into(), "write".into(), super::types::EditRenderArgs::default(),
        ara_edit::EditMode::Hashline, theme.clone(),
    );
    original_write.graph_args_root = Some(original_args);
    original_write.component.options = options;
    assert!(original_write.rebuild_in(&graph).is_empty(), "B-47706ffcce WRITE renderer construction failed");
    assert!(!original_write.args_complete && !original_write.execution_started);
    let original_rows = original_write.render_in(&graph, 120, &super::types::FrameContext::default(), 0, usize::MAX, std::time::Instant::now()).unwrap();
    assert_eq!(original_rows, direct_rows, "B-47706ffcce original Component and production WRITE rows differ");
    let original_text = original_rows.iter().map(|row| text::sanitize(&row.as_str().into())).collect::<Vec<_>>().join("\n");
    assert!(original_text.contains("queued"), "B-47706ffcce: {original_text}");
    assert!(original_text.contains("ecoport/search"), "B-47706ffcce: {original_text}");
    assert!(original_text.contains("Broken"), "B-47706ffcce: {original_text}");
    eprintln!("[original22] B-47706ffcce assertions passed");

    eprintln!("[xdev-debug] renderer hidden+queued done / refusal begin");
    // Outer refusal is not a device-dispatch receipt. Retain source WRITE error
    // presentation instead of manufacturing a queued inner result.
    let denied = heap.import_serde(&serde_json::json!({"content":[{"type":"text","text":"Tool call denied by user"}],"isError":true})).unwrap();
    let final_options = RenderOptions::default();
    assert!(XdevComponent::build(&graph, &mcp, Some(&denied), final_options, false, true, &theme, None).unwrap().is_none());
    let mut write = super::write_renderer::WriteComponent::default();
    write.update_args(mcp);
    write.result = Some(super::write_renderer::WriteResult {
        error: true, text: super::types::WireText("Tool call denied by user".into()), ..Default::default()
    });
    let plain = write.render(80, &super::types::FrameContext::default(), final_options, &theme)
        .iter().map(|row| text::sanitize(&row.as_str().into())).collect::<Vec<_>>().join("\n");
    assert!(plain.contains("Tool call denied by user"), "{plain}");

    // Generic cards use nullish fallback while queued display labels use truth.
    let empty_label = heap.import_serde(&serde_json::json!({"label":""})).unwrap();
    assert!(label(&graph, &"device".into(), Some(&empty_label), false).unwrap().is_empty());
    assert!(label(&graph, &"device".into(), Some(&empty_label), true).unwrap().equals_ascii("device"));

    // Empty-string and absent/nonstring content remain distinct without invoking
    // coercion; presence changes invalidate the retained Write arguments.
    let mut write = super::write_renderer::WriteComponent::default();
    write.update_args(incomplete);
    let revision = write.revision;
    write.update_args(args(serde_json::json!({"path":"xd://ast_e","content":""})));
    assert!(write.revision > revision && write.args.content_present && write.args.content_is_string);
    let nonstring = args(serde_json::json!({"path":"xd://ast_e","content":{"value":1}}));
    assert!(nonstring.content_present && !nonstring.content_is_string && nonstring.content.0.is_empty());

    eprintln!("[xdev-debug] renderer refusal+args done / ActualStart begin");
    // ActualToolExecutionStart first upserts a queued WRITE in the production
    // controller, then changes started state without a second args Get. The
    // next real render must use the captured state transition to delegate.
    static GET_CONTENT: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "authored source-consumer fixture: count original content getter; return or throw captured value",
        name: "get content", length: 0, form: CompiledFunctionForm::Ordinary, capture_count: 3,
        body: |heap, captures, _, _, runtime| {
            let prior = heap.get(&captures[0], &"reads".into(), runtime)?;
            let next = heap.to_number(&prior, runtime)? + 1.0;
            heap.define_own_data(&captures[0], "reads".into(), heap.number(next)?, runtime)?;
            if heap.truthy(&captures[2])? { Err(ValueError::Thrown(captures[1].clone())) }
            else { Ok(captures[1].clone()) }
        },
    };
    eprintln!("[xdev-debug] renderer ActualStart loop begin");
    for throws in [false, true] {
        let reads = heap.import_serde(&serde_json::json!({"reads":0})).unwrap();
        let reply = if throws { heap.object().unwrap() } else { heap.string("{\"query\":\"Tokyo\"}".into()).unwrap() };
        let getter = heap.fresh_compiled_function(&GET_CONTENT, &[reads.clone(), reply, heap.boolean(throws).unwrap()]).unwrap();
        let ValueSnapshot::Function(target) = heap.snapshot(&getter).unwrap() else { panic!("original compiled getter") };
        let call_args = heap.import_serde(&serde_json::json!({"path":"xd://authored_preview"})).unwrap();
        heap.define_own(&call_args, "content".into(), ara_rpc::value_heap::PropertyDescriptor::Accessor {
            get: Some(target), set: None, enumerable: true, configurable: true,
        }, &runtime).unwrap();
        let mut controller = super::event_controller::EventController::new(theme.clone(), ara_edit::EditMode::Hashline);
        controller.begin_run(ara_edit::EditMode::Hashline);
        let start = ara_agent::AgentEvent::ActualToolExecutionStart {
            tool_call_id: "state-key-start".into(), tool_name: "write".into(), args: call_args,
            intent: heap.undefined().unwrap(),
        };
        eprintln!("[xdev-debug] renderer ActualStart throws={throws} consume begin");
        controller.consume_in(&start, Some(&realm.values), Some(&runtime)).unwrap();
        eprintln!("[xdev-debug] renderer ActualStart throws={throws} consume done");
        let count = || heap.to_number(&heap.get(&reads, &"reads".into(), &runtime).unwrap(), &runtime).unwrap();
        assert_eq!(count(), 1.0, "Start does not replay the source content getter");
        let tool = controller.tools.get_mut(&WireString::from("state-key-start")).unwrap();
        assert!(tool.execution_started && tool.args_complete);
        let mut prior_rows=None;
        for spinner in [0, 1, 2] {
            eprintln!("[xdev-debug] renderer ActualStart throws={throws} spinner={spinner} render begin");
            let rendered = tool.render_in(&graph, 120, &super::types::FrameContext::default(), spinner, usize::MAX, std::time::Instant::now()).unwrap();
            eprintln!("[xdev-debug] renderer ActualStart throws={throws} spinner={spinner} render done");
            let plain = rendered.iter().map(|row| text::sanitize(&row.as_str().into())).collect::<Vec<_>>().join("\n");
            assert!(!plain.contains("queued"), "Start state transition retained old queued view: {plain}");
            assert_eq!(count(), 1.0, "draw cannot replay original successful or failed content getter");
            if !throws {
                if let Some(prior)=&prior_rows {assert_ne!(prior,&rendered,"generic spinner options must advance on paint");}
                prior_rows=Some(rendered);
            }
        }
        eprintln!("[xdev-debug] renderer ActualStart throws={throws} assertions done");
        if throws {
            assert!(tool.constructor_failed);
            controller.consume_in(&start, Some(&realm.values), Some(&runtime)).unwrap();
            assert_eq!(count(), 1.0, "same failed args root remains fenced on another Start event");
        }
    }
}
