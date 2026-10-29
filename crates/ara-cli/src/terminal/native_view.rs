//! Source-shaped semantic Edit views. OMP b91db66e packages/tui/src/tools/
//! {edit,ast-edit,native-view}.ts. Copyright (c) 2026 Stencil Labs, Inc. MIT.
//! Evaluated in the Session display task; only owned nodes cross to OS paint.
use super::{
    edit_renderer,
    render_utils::{self, sanitize},
    theme::language_from_path,
    tsp_backend::{Action, Node},
    types::*,
};
use ara_rpc::WireString;
use serde_json::{Value, json};
fn s(value: &Option<WireString>) -> Option<String> {
    value.as_ref().map(sanitize)
}
fn span(text: impl Into<String>, token: &str) -> Value {
    json!({"t":text.into(),"s":token})
}
fn text(key: &str, value: &str, token: &str, role: &str) -> Node {
    Node::new(
        key,
        "text",
        json!({"spans":[span(value,token)],"wrap":"word","role":role}),
        vec![],
    )
}
pub(crate) fn display_path(path: &str) -> String {
    let path = std::path::Path::new(path);
    if path.is_absolute() {
        if let Ok(cwd) = std::env::current_dir() {
            if let Ok(relative) = path.strip_prefix(cwd) {
                if !relative.as_os_str().is_empty() {
                    return relative.to_string_lossy().replace('\\', "/");
                }
            }
        }
    }
    render_utils::shorten_path(&path.to_string_lossy())
}
fn stats(diff: &str) -> (usize, usize) {
    let x = render_utils::diff_stats(diff);
    (x.added, x.removed)
}
fn stats_meta(added: usize, removed: usize) -> Option<String> {
    let mut v = vec![];
    if added > 0 {
        v.push(format!("+{added}"))
    }
    if removed > 0 {
        v.push(format!("−{removed}"))
    }
    (!v.is_empty()).then(|| v.join(" "))
}
fn error(message: &str) -> Node {
    let message = sanitize(&message.into()).trim().to_owned();
    if message.contains('\n') && message.starts_with(['{', '[']) {
        Node::new(
            "error",
            "code",
            json!({"text":message,"lang":"json","tone":"error","role":"omp.tool.error"}),
            vec![],
        )
    } else {
        text("error", &message, "error", "omp.tool.error")
    }
}
fn diff(value:&str,path:&str)->Node{
    // AST supplies a real unified diff; its native helper uses text directly.
    Node::new("diff","diff",json!({"text":sanitize(&value.into()),"lang":language_from_path(path)}),vec![])
}
fn edit_diff(value:&str,path:&str)->Node{
    // chrome/diff.ts nativeDiff: canonical numbered gutters become hunks;
    // accumulated delta preserves old/new starts across omitted regions.
    let value=sanitize(&value.into());let mut hunks=vec![];let mut lines:Vec<String>=vec![];
    let(mut old_start,mut new_start,mut delta,mut region_delta,mut next_old)=(0i64,0i64,0i64,0i64,1i64);
    let close=|hunks:&mut Vec<Value>,lines:&mut Vec<String>,old:i64,new:i64,delta:&mut i64,region:&mut i64|{if !lines.is_empty(){hunks.push(json!({"oldStart":old,"newStart":new,"lines":std::mem::take(lines)}));}*delta+=*region;*region=0;};
    for raw in value.split('\n'){
        let parsed=super::diff::parse(raw);let omitted=parsed.as_ref().is_some_and(|p|p.prefix==' '&&matches!(p.content.as_str(),"..."|"…"));
        if parsed.is_none()||omitted{if raw.trim().is_empty()||matches!(raw.trim(),"..."|"…"){close(&mut hunks,&mut lines,old_start,new_start,&mut delta,&mut region_delta);}continue}
        let parsed=parsed.unwrap();let number=parsed.number.trim().parse::<i64>().ok();
        if lines.is_empty(){if let Some(number)=number{old_start=if parsed.prefix=='+'{number-delta}else{number};new_start=if parsed.prefix=='+'{number}else{number+delta};}else{old_start=next_old;new_start=next_old+delta;}}
        if parsed.prefix=='+'{region_delta+=1;}else{if parsed.prefix=='-'{region_delta-=1;}next_old=number.unwrap_or(next_old)+1;}
        lines.push(format!("{}{}",parsed.prefix,parsed.content));
    }
    close(&mut hunks,&mut lines,old_start,new_start,&mut delta,&mut region_delta);
    Node::new("diff","diff",json!({"hunks":hunks,"lang":language_from_path(path)}),vec![])
}
fn file_section(
    path: &str,
    rename: Option<&str>,
    added: usize,
    removed: usize,
    body: Vec<Node>,
    failed: bool,
    role: &str,
) -> Node {
    let mut head = vec![span(display_path(path), "path")];
    if let Some(rename) = rename {
        head.push(span(" → ", "muted"));
        head.push(span(display_path(rename), "path"));
    }
    if added > 0 {
        head.push(span(format!(" +{added}"), "ins"))
    }
    if removed > 0 {
        head.push(span(format!(" −{removed}"), "del"))
    }
    Node::new(
        path,
        "section",
        json!({"head":head,"role":role,"tone":if failed{Some("error")}else{None}}),
        body,
    )
}
fn diagnostics(value: &Diagnostics) -> Option<Node> {
    if value.messages.is_empty() {
        return None;
    }
    let re =
        regex::Regex::new(r"^(.+?):(\d+):(\d+)\s+\[(\w+)\]\s+(?:\[([^\]]+)\]\s+)?(.+?)(?:\s+\(([^)]+)\))?$").unwrap();
    let mut parsed = vec![];
    let mut rest = vec![];
    for message in &value.messages {
        let message = sanitize(message).trim().to_owned();
        if let Some(m) = re.captures(&message) {
            parsed.push((
                m[1].to_owned(),
                m[2].parse::<u64>().unwrap_or(0),
                m[3].parse::<u64>().unwrap_or(0),
                m[4].to_owned(),
                m[6].to_owned(),
                m.get(5).map(|m| m.as_str().to_owned()),
                m.get(7).map(|m| m.as_str().to_owned()),
            ))
        } else if !message.is_empty() {
            rest.push(message)
        }
    }
    let rank = |severity: &str| match severity {
        "error" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    };
    parsed.sort_by_key(|d| (rank(&d.3), d.1, d.2));
    let with_path = parsed
        .iter()
        .map(|d| &d.0)
        .collect::<std::collections::HashSet<_>>()
        .len()
        > 1;
    let mut rows = vec![];
    for (i, d) in parsed.iter().enumerate() {
        let (icon, tone) = match d.3.as_str() {
            "error" => ("x-circle", "error"),
            "warning" => ("triangle-alert", "warning"),
            "info" => ("info", "muted"),
            _ => ("circle-help", "muted"),
        };
        let where_ = format!(
            "{}{}:{}",
            if with_path {
                format!("{}:", display_path(&d.0))
            } else {
                String::new()
            },
            d.1,
            d.2
        );
        let message = [Some(d.4.clone()), d.5.clone(), d.6.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        rows.push(Node::new(
            format!("d{i}"),
            "row",
            json!({"role":"omp.tool.diagnostic","gap":"sm","align":"baseline"}),
            vec![
                Node::new("severity", "icon", json!({"name":icon,"tone":tone}), vec![]),
                text("at", &where_, "muted", ""),
                text("message", &message, "", ""),
            ],
        ));
    }
    for (i, message) in rest.iter().enumerate() {
        rows.push(text(&format!("u{i}"), message, "", "omp.tool.diagnostic"))
    }
    let hidden = rows.len().saturating_sub(5);
    rows.truncate(5);
    if hidden > 0 {
        rows.push(text("more", &format!("+{hidden} more"), "muted", "omp.tool.stats"))
    }
    Some(Node::new(
        "diagnostics",
        "section",
        json!({"head":[span("Diagnostics","")],"tone":if value.errored{"error"}else{"warning"},"role":"omp.tool.diagnostics"}),
        rows,
    ))
}
#[derive(Default)]
struct File {
    path: String,
    resolved: Option<String>,
    rename: Option<String>,
    op: Option<Operation>,
    line: Option<u32>,
    added: usize,
    removed: usize,
    failed: bool,
    diagnostics: Option<Diagnostics>,
    body: Vec<Node>,
}
fn file_parts(args: &EditRenderArgs, result: &EditResult, context: &EditRenderContext) -> File {
    let d = result.details.as_ref();
    let e = args.edits.as_ref().and_then(|e| e.first());
    let f = edit_renderer::call_facts(args, RenderOptions::default(), context);
    let path = d
        .and_then(|d| s(&d.source_path))
        .or_else(|| s(&args.file_path))
        .or_else(|| s(&args.path))
        .or_else(|| e.and_then(|e| s(&e.path)))
        .or_else(|| d.and_then(|d| s(&d.path)))
        .unwrap_or(f.path);
    let resolved = d.and_then(|d| s(&d.path));
    let rename = s(&args.rename)
        .or_else(|| e.and_then(|e| s(&e.rename).or_else(|| s(&e.move_to))))
        .or_else(|| d.and_then(|d| s(&d.move_to)));
    let op = args.op.or(e.and_then(|e| e.op)).or(d.and_then(|d| d.op));
    let failed = result.is_error.or(d.and_then(|d| d.is_error)).unwrap_or(false);
    let preview = if d.is_none() {
        context.edit_diff_preview.as_ref()
    } else {
        None
    };
    let delta = if failed {
        None
    } else {
        d.map(|d| sanitize(&d.diff.0))
            .filter(|s| !s.is_empty())
            .or_else(|| match preview {
                Some(EditPreview::Diff { diff, .. }) => Some(sanitize(diff)),
                _ => None,
            })
    };
    let line = d
        .filter(|_| !failed)
        .and_then(|d| d.first_changed_line)
        .or_else(|| match preview {
            Some(EditPreview::Diff { first_changed_line, .. }) => *first_changed_line,
            _ => None,
        });
    let mut body = vec![];
    if failed {
        let message = d
            .and_then(|d| {
                s(&d.display_error_text)
                    .filter(|s| !s.is_empty())
                    .or_else(|| s(&d.error_text).filter(|s| !s.is_empty()))
            })
            .unwrap_or_else(|| result.content.iter().map(sanitize).collect::<Vec<_>>().join("\n"));
        if !message.is_empty() {
            body.push(error(&message))
        }
    } else if let Some(delta) = &delta {
        body.push(edit_diff(delta, resolved.as_deref().unwrap_or(&path)))
    } else if let Some(EditPreview::Error(message)) = preview {
        body.push(error(&sanitize(message)))
    } else if d.is_some() && !matches!(op, Some(Operation::Create | Operation::Delete)) && rename.is_none() {
        let shown = display_path(resolved.as_deref().unwrap_or(&path));
        body.push(text(
            "no-change",
            &format!(
                "No changes were made{}.",
                if shown.is_empty() {
                    String::new()
                } else {
                    format!(" to {shown}")
                }
            ),
            "muted",
            "",
        ));
    }
    let diagnostics = d.and_then(|d| d.diagnostics.clone());
    if let Some(n) = diagnostics.as_ref().and_then(self::diagnostics) {
        body.push(n)
    }
    let (added, removed) = delta.as_deref().map(stats).unwrap_or_default();
    File {
        path,
        resolved,
        rename,
        op,
        line,
        added,
        removed,
        failed,
        diagnostics,
        body,
    }
}
fn head(file: &File, files: usize, diagnostics: &[Option<Diagnostics>]) -> Value {
    let multi = files > 1;
    let mut meta = vec![];
    if let Some(stats) = stats_meta(file.added, file.removed) {
        meta.push(stats)
    }
    if !multi {
        if let Some(rename) = &file.rename {
            meta.push(format!("→ {}", display_path(rename)))
        }
    }
    let count: usize = diagnostics.iter().flatten().map(|d| d.messages.len()).sum();
    let errored = diagnostics.iter().flatten().any(|d| d.errored);
    let badge = if count > 0 {
        json!([{"text":format!("{count} diagnostic{}",if count==1{""}else{"s"}),"tone":if errored{"error"}else{"warning"}}])
    } else {
        Value::Null
    };
    let target = if multi {
        Some(format!("{files} files"))
    } else if file.path.is_empty() {
        None
    } else {
        Some(format!(
            "{}{}",
            display_path(&file.path),
            file.line
                .filter(|n| *n > 0)
                .map(|n| format!(":{n}"))
                .unwrap_or_default()
        ))
    };
    let href = if multi {
        None
    } else {
        file.resolved
            .as_deref()
            .filter(|p| std::path::Path::new(p).is_absolute())
            .and_then(|p| Some(file_href(p)))
    };
    json!({"title":if multi{"Edit"}else{file.op.unwrap_or_default().title()},"target":target,"targetKind":if multi{"text"}else{"path"},"href":href,"meta":meta,"badges":badge})
}
pub(crate) fn edit(
    args: &EditRenderArgs,
    result: Option<&EditResult>,
    options: RenderOptions,
    context: &EditRenderContext,
) -> Node {
    edit_with_facts(args,result,options,context,None)
}
pub(crate) fn edit_with_facts(args:&EditRenderArgs,result:Option<&EditResult>,options:RenderOptions,context:&EditRenderContext,facts:Option<edit_renderer::CallFacts>)->Node{
    let facts = facts.unwrap_or_else(||edit_renderer::call_facts(args, options, context));
    let mut body = vec![];
    let mut file = File {
        path: facts.path.clone(),
        rename: facts.rename.clone(),
        op: facts.op,
        ..Default::default()
    };
    let mut files = facts.file_count;
    let mut diagnostic_files = vec![];
    if let Some(result) = result {
        let total = facts.file_count.max(args
            .edits
            .as_ref()
            .map(|e| {
                e.iter()
                    .filter_map(|e| e.path.as_ref())
                    .filter(|p| !p.is_empty())
                    .collect::<std::collections::HashSet<_>>()
                    .len()
            })
            .unwrap_or(0));
        if let Some(results) = result
            .details
            .as_ref()
            .and_then(|d| d.per_file_results.as_ref())
            .filter(|r| r.len() > 1 || total > 1)
        {
            files = total.max(results.len());
            file = File::default();
            for d in results {
                let f = file_parts(
                    &EditRenderArgs::default(),
                    &EditResult {
                        details: Some(d.clone()),
                        is_error: d.is_error,
                        ..Default::default()
                    },
                    context,
                );
                file.added += f.added;
                file.removed += f.removed;
                file.failed |= f.failed;
                diagnostic_files.push(f.diagnostics.clone());
                body.push(file_section(
                    &f.path,
                    f.rename.as_deref(),
                    f.added,
                    f.removed,
                    f.body,
                    f.failed,
                    "omp.tool.edit.file",
                ));
            }
            let remaining = total.saturating_sub(results.len());
            if remaining > 0 {
                body.push(Node::new("remaining","spinner",json!({"label":[span(format!("{remaining} more file{} pending…",if remaining==1{""}else{"s"}),"muted")]}),vec![]))
            }
        } else {
            file = file_parts(args, result, context);
            body = std::mem::take(&mut file.body);
            diagnostic_files.push(file.diagnostics.clone());
            files = 0;
        }
    } else if let Some(multi) = context.per_file_diff_preview.as_ref().filter(|p| {
        p.len() > 1
            && p.iter().any(|p| {
                p.diff.as_ref().is_some_and(|d| !d.is_empty()) || p.error.as_ref().is_some_and(|e| !e.is_empty())
            })
    }) {
        files = files.max(multi.len());
        for p in multi {
            let delta = s(&p.diff).unwrap_or_default();
            let (a, r) = stats(&delta);
            file.added += a;
            file.removed += r;
            let fail = p.error.as_ref().is_some_and(|e| !e.is_empty());
            let child = if fail {
                error(&s(&p.error).unwrap())
            } else {
                edit_diff(&delta, &sanitize(&p.path.0))
            };
            body.push(file_section(
                &sanitize(&p.path.0),
                None,
                a,
                r,
                vec![child],
                fail,
                "omp.tool.edit.file",
            ));
        }
    } else {
        let mut delta = None;
        if args.preview_diff.as_ref().is_some_and(|v|!v.is_empty()) || (args.diff.as_ref().is_some_and(|v|!v.is_empty()) && args.op.is_some()) {
            let d = s(&args.preview_diff).or_else(|| s(&args.diff)).unwrap_or_default();
            body.push(edit_diff(&d, &file.path));
            delta = Some(d)
        } else if [&args.diff,&args.new_text,&args.patch].into_iter().any(|v|v.as_ref().is_some_and(|v|!v.is_empty())) {
            let value=s(&args.diff).or_else(||s(&args.new_text)).or_else(||s(&args.patch)).unwrap_or_default();
            body.push(Node::new(
                "input",
                "code",
                json!({"text":value,"lang":language_from_path(&file.path)}),
                vec![],
            ))
        } else if let Some(preview) = &context.edit_diff_preview {
            match preview {
                EditPreview::Error(message) => body.push(error(&sanitize(message))),
                EditPreview::Diff {
                    diff: d,
                    first_changed_line,
                } => {
                    let d = sanitize(d);
                    body.push(edit_diff(&d, &file.path));
                    delta = Some(d);
                    file.line = *first_changed_line;
                }
            }
        }
        if let Some(delta) = delta {
            (file.added, file.removed) = stats(&delta)
        }
    }
    if let Some(message) = facts.apply_error {
        body.push(error(&message));
        file.failed = true
    }
    let mut props = head(&file, files, &diagnostic_files);
    props["tone"] = if file.failed { json!("error") } else { Value::Null };
    Node::new("tool", "tool", props, body)
}
pub(crate) fn decorate(
    mut node: Node,
    id: WireString,
    generation: u64,
    options: RenderOptions,
    started: bool,
    sealed: bool,
    result: Option<&EditResult>,
) -> Node {
    let failed = result.and_then(|r| r.is_error).unwrap_or(false);
    let status = if result.is_some() && !options.is_partial {
        if failed { "error" } else { "done" }
    } else if sealed {
        "cancelled"
    } else if started || result.is_some() {
        "running"
    } else {
        "pending"
    };
    node.props["status"] = json!(status);
    node.props["frame"] = json!("card");
    node.props["collapsible"] = json!(!node.children.is_empty());
    node.props["collapsed"] = json!(!options.expanded && !matches!(status, "running" | "pending"));
    node.props["preview"] = json!({"lines":10});
    node.action = Some(Action::Tool { generation, id });
    node
}
// AST's displayContent uses a visual gutter and deliberately excludes model hash anchors.
pub(crate) fn ast(args: &Value, result: Option<&Value>) -> Node {
    let ops = args["ops"].as_array();
    let count = args["__rewriteCount"].as_u64().map(|n|n as usize).unwrap_or_else(||ops.map_or(0,Vec::len));
    let pattern = ops
        .filter(|o| o.len() == 1)
        .and_then(|o| o[0]["pat"].as_str())
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty());
    let mut props = json!({"title":"AST Edit","target":if count>1{Some(format!("{count} rewrites"))}else{pattern.clone()},"targetKind":if count>1{"text"}else{"pattern"}});
    let mut body = vec![];
    let Some(result) = result else {
        if let Some(paths) = args["paths"].as_array().filter(|p| !p.is_empty()) {
            props["meta"] = json!([format!(
                "in {}",
                paths
                    .iter()
                    .filter_map(Value::as_str)
                    .map(display_path)
                    .collect::<Vec<_>>()
                    .join(", ")
            )])
        }
        return Node::new("tool", "tool", props, body);
    };
    let d = &result["details"];
    let content = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if result["isError"] == true {
        props["tone"] = json!("error");
        body.push(error(if content.is_empty() { "Unknown error" } else { &content }));
        return Node::new("tool", "tool", props, body);
    }
    let total = d["totalReplacements"].as_u64().unwrap_or(0);
    let parse = d["parseErrors"].as_array().cloned().unwrap_or_default();
    let parse_note = if parse.is_empty() {
        None
    } else {
        let count = d["parseErrorsTotal"].as_u64().unwrap_or(parse.len() as u64);
        Some(Node::new(
            "parse-errors",
            "section",
            json!({"head":[span(format!("{count} parse issue{}",if count==1{""}else{"s"}),"warning")],"collapsible":true,"collapsed":true}),
            vec![Node::new(
                "errors",
                "code",
                json!({"text":parse.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")}),
                vec![],
            )],
        ))
    };
    let mut scope = vec![];
    if let Some(path) = d["scopePath"].as_str().filter(|p| !p.is_empty()) {
        scope.push(format!("in {path}"))
    }
    if let Some(n) = d["filesSearched"].as_u64().filter(|n| *n > 0) {
        scope.push(format!("searched {n} file{}", if n == 1 { "" } else { "s" }))
    }
    if d.get("__scopeLimitReached").unwrap_or(&d["limitReached"]).as_bool() == Some(true) {
        scope.push("limit reached; narrow path".into())
    }
    if total == 0 {
        props["meta"] = json!(["0 replacements"]);
        props["tone"] = json!("warning")
    } else {
        let source = d["displayContent"].as_str().unwrap_or(&content);
        let kept = source
            .split("\n\n")
            .filter(|group| !group.starts_with("Safety cap reached") && !group.starts_with("Parse issues:"))
            .collect::<Vec<_>>()
            .join("\n");
        let header = regex::Regex::new(r"^(#+)\s+(.*)$").unwrap();
        let suffix = regex::Regex::new(r"\s+\([^)]*\)\s*$").unwrap();
        let change = regex::Regex::new(r"^([+\- ])\s*(\d+)(?:│|[:|])(.*)$").unwrap();
        let mut dirs: Vec<String> = vec![];
        let mut files: Vec<(String, Vec<String>, usize, usize, Option<u64>)> = vec![];
        let mut file: Option<(String, Vec<String>, usize, usize, Option<u64>)> = None;
        for line in kept.lines() {
            if let Some(m) = header.captures(line) {
                if let Some(f) = file.take().filter(|f| !f.1.is_empty()) {
                    files.push(f)
                }
                let depth = m[1].len();
                let rest = suffix.replace(m[2].trim_end(), "").into_owned();
                dirs.resize(depth.saturating_sub(1), String::new());
                if let Some(dir) = rest.strip_suffix('/') {
                    dirs.push(dir.into());
                    continue;
                }
                let prefix = dirs
                    .iter()
                    .filter(|p| !p.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("/");
                file = Some((
                    if prefix.is_empty() {
                        rest
                    } else {
                        format!("{prefix}/{rest}")
                    },
                    vec![],
                    0,
                    0,
                    None,
                ));
                continue;
            }
            if let (Some(f), Some(m)) = (&mut file, change.captures(line)) {
                let n = m[2].parse::<u64>().unwrap_or(0);
                if f.4.is_none_or(|last| n > last + 1 || n < last) {
                    f.1.push(format!("@@ -{n} +{n} @@"))
                }
                f.4 = Some(n);
                if &m[1] == "+" {
                    f.2 += 1
                } else if &m[1] == "-" {
                    f.3 += 1
                }
                f.1.push(format!("{}{}", &m[1], &m[3]));
            }
        }
        if let Some(f) = file.filter(|f| !f.1.is_empty()) {
            files.push(f)
        }
        let count = (d["filesTouched"].as_u64().unwrap_or(0) as usize).max(files.len());
        let single = count == 1 && files.len() == 1;
        let a = files.iter().map(|f| f.2).sum();
        let r = files.iter().map(|f| f.3).sum();
        props["target"] = if single {
            json!(display_path(&files[0].0))
        } else {
            json!(format!("{count} file{}", if count == 1 { "" } else { "s" }))
        };
        props["targetKind"] = json!(if single { "path" } else { "text" });
        props["meta"] = json!(
            [
                stats_meta(a, r),
                Some(format!("{total} replacement{}", if total == 1 { "" } else { "s" }))
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
        );
        if d["applied"] != true {
            props["badges"] = json!([{"text":"proposed","tone":"warning"}])
        }
        if d["limitReached"] == true {
            props["tone"] = json!("warning")
        }
        if let Some(pattern) = pattern {
            let mut n = text("pattern", &pattern, "code", "omp.tool.context");
            n.props["lines"] = json!(1);
            n.props["truncate"] = json!("end");
            body.push(n)
        }
        if files.is_empty() {
            body.push(text("note", &kept, "muted", ""))
        } else {
            for f in files {
                let child = diff(&f.1.join("\n"), &f.0);
                if single {
                    body.push(child)
                } else {
                    body.push(file_section(
                        &f.0,
                        None,
                        f.2,
                        f.3,
                        vec![child],
                        false,
                        "omp.tool.ast_edit.file",
                    ))
                }
            }
        }
    }
    if let Some(note) = parse_note {
        body.push(note)
    }
    if !scope.is_empty() {
        body.push(text("scope", &scope.join(" · "), "muted", "omp.tool.stats"))
    }
    Node::new("tool", "tool", props, body)
}

fn file_href(path: &str) -> String {
    let raw = path.replace('\\', "/");
    let mut uri = if raw.starts_with('/') {
        "file://".to_owned()
    } else {
        "file:///".to_owned()
    };
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~:".contains(&byte) {
            uri.push(byte as char)
        } else {
            uri.push_str(&format!("%{byte:02X}"))
        }
    }
    uri
}
/// Resolve AST's real direct/xdev roots in the existing Session runtime.
/// This reads declared renderer operands only; it does not export arbitrary Graphs.
pub(crate) fn ast_graph(
    g: &super::graph_renderer::DisplayGraph<'_>,
    name: &WireString,
    args: &ara_rpc::value_heap::ValueHandle,
    result: Option<&ara_rpc::value_heap::ValueHandle>,
    started: bool,
) -> Result<Option<Node>, ara_rpc::value_heap::ValueError> {
    use ara_rpc::value_heap::{ValueHandle, ValueSnapshot};
    fn primitive(
        g: &super::graph_renderer::DisplayGraph<'_>,
        v: &ValueHandle,
    ) -> Result<Value, ara_rpc::value_heap::ValueError> {
        Ok(match g.0.heap().snapshot(v)? {
            ValueSnapshot::String(s) => json!(sanitize(&s)),
            ValueSnapshot::Number(n) if n>=0.0 && n.fract()==0.0 && n<(u64::MAX as f64) => json!(n as u64),
            ValueSnapshot::Number(n) => json!(n),
            ValueSnapshot::Boolean(b) => json!(b),
            _ => Value::Null,
        })
    }
    let mut args = args.clone();
    let mut inner_details = None;
    if name.equals_ascii("write") {
        if let Some(root) = result {
            let details = g.get(root, "details")?;
            if g.nullish(&details)? {
                return Ok(None);
            }
            let dispatch = g.get(&details, "xdev")?;
            if g.nullish(&dispatch)? || !g.is(&dispatch, "tool", "ast_edit")? || g.is(&dispatch, "mode", "help")? {
                return Ok(None);
            }
            args = g.get(&dispatch, "args")?;
            inner_details = Some(g.get(&dispatch, "inner")?);
        } else {
            let path = g.get(&args, "path")?;
            let Some(path) = g.optional(&path)? else {
                return Ok(None);
            };
            if !crate::actual_tools::xdev_native::parse_xd(&path).is_some_and(|s| s.equals_ascii("ast_edit")) {
                return Ok(None);
            }
            if !started {
                return Ok(None);
            }
            let content = g.get(&args, "content")?;
            let Some(content) = g.optional(&content)? else {
                return Ok(None);
            };
            args = super::write_renderer::parse_graph_args(g, &content)?;
        }
    } else if !name.equals_ascii("ast_edit") {
        return Ok(None);
    }
    // Follow describeCall/describeResult's conditional original Gets. This
    // adapter never enumerates an operation or reads its unused `out` value.
    fn pattern(g:&super::graph_renderer::DisplayGraph<'_>,args:&ValueHandle)->Result<(Value,Value),ara_rpc::value_heap::ValueError>{
        let ops=g.get(args,"ops")?;
        let count=if g.nullish(&ops)?{Value::Null}else{primitive(g,&g.get(&ops,"length")?)?};
        let mut owned=json!({"__rewriteCount":count.as_u64().unwrap_or(0)});
        if count.as_f64()==Some(1.0) {
            let ops=g.get(args,"ops")?;
            if !g.nullish(&ops)? {let op=g.get(&ops,"0")?;if !g.nullish(&op)? {owned["ops"]=json!([{"pat":primitive(g,&g.get(&op,"pat")?)?}]);}}
        }
        Ok((owned,count))
    }
    fn result_content(g:&super::graph_renderer::DisplayGraph<'_>,root:&ValueHandle)->Result<Value,ara_rpc::value_heap::ValueError>{
        let content=g.get(root,"content")?;let mut blocks=vec![];
        if !g.nullish(&content)? {for block in g.members(&content)? {if g.is(&block,"type","text")? && g.optional(&g.get(&block,"text")?)?.is_some() {blocks.push(json!({"type":"text","text":primitive(g,&g.get(&block,"text")?)?}));}}}
        Ok(json!(blocks))
    }
    fn detail(g:&super::graph_renderer::DisplayGraph<'_>,root:&ValueHandle,key:&str)->Result<ValueHandle,ara_rpc::value_heap::ValueError>{
        if g.nullish(root)? {g.0.heap().undefined()}else{g.get(root,key)}
    }
    fn scope(g:&super::graph_renderer::DisplayGraph<'_>,root:&ValueHandle,out:&mut Value)->Result<(),ara_rpc::value_heap::ValueError>{
        for key in ["scopePath","filesSearched"] {let value=detail(g,root,key)?;if g.truth(&value)? {out[key]=primitive(g,&detail(g,root,key)?)?;}}
        out["__scopeLimitReached"]=json!(g.truth(&detail(g,root,"limitReached")?)?);Ok(())
    }
    // The target obtains result.details before its pattern head and isError.
    let details=if let Some(root)=result {Some(match inner_details {Some(d)=>d,None=>g.get(root,"details")?})}else{None};
    let (mut owned,_)=pattern(g,&args)?;
    let result=if let Some(root)=result {
        let mut out=json!({});let is_error=g.get(root,"isError")?;out["isError"]=json!(g.truth(&is_error)?);
        if g.truth(&is_error)? {out["content"]=result_content(g,root)?;}
        else {
            let details=details.as_ref().unwrap();let mut d=json!({});
            let errors=detail(g,details,"parseErrors")?;
            if !g.nullish(&errors)? {
                let length=g.get(&errors,"length")?;
                if g.0.heap().to_number(&length,g.runtime())?>0.0 {
                    let total=detail(g,details,"parseErrorsTotal")?;
                    d["parseErrorsTotal"]=if g.nullish(&total)? {primitive(g,&g.get(&errors,"length")?)?}else{primitive(g,&total)?};
                    let joined=g.method(&errors,"join",&[g.0.heap().string("\n".into())?])?;
                    d["parseErrors"]=json!([primitive(g,&joined)?]);
                }
            }
            let total=detail(g,details,"totalReplacements")?;
            d["totalReplacements"]=if g.nullish(&total)?{json!(0)}else{primitive(g,&total)?};
            if d["totalReplacements"].as_f64()==Some(0.0) {scope(g,details,&mut d)?;}
            else {
                let limit=detail(g,details,"limitReached")?;d["limitReached"]=json!(g.truth(&limit)?);
                let display=detail(g,details,"displayContent")?;
                if g.nullish(&display)? {out["content"]=result_content(g,root)?;}else{d["displayContent"]=primitive(g,&display)?;}
                d["filesTouched"]=primitive(g,&detail(g,details,"filesTouched")?)?;
                // Nonzero results read the single pattern again for context.
                let (latest,_)=pattern(g,&args)?;owned=latest;
                d["applied"]=json!(g.truth(&detail(g,details,"applied")?)?);
                scope(g,details,&mut d)?;
            }
            out["details"]=d;
        }
        Some(out)
    }else{
        let paths=g.get(&args,"paths")?;
        if !g.nullish(&paths)? && g.truth(&g.get(&paths,"length")?)? {
            static DISPLAY_PATH:ara_rpc::value_heap::CompiledFunctionSpec=ara_rpc::value_heap::CompiledFunctionSpec{
                source:"fixed AST describeCall paths.map(displayPath)",name:"displayPath",length:1,form:ara_rpc::value_heap::CompiledFunctionForm::Ordinary,capture_count:0,
                body:|heap,_,_,args,runtime|{let value=args.first().cloned().unwrap_or(heap.undefined()?);let text=heap.to_string(&value,runtime)?;heap.string(display_path(&sanitize(&text)).into())},
            };
            let paths=g.get(&args,"paths")?;let mapped=g.method(&paths,"map",&[g.0.heap().fresh_compiled_function(&DISPLAY_PATH,&[])?])?;
            let joined=g.method(&mapped,"join",&[g.0.heap().string(", ".into())?])?;
            let mut node=ast(&owned,None);node.props["meta"]=json!([format!("in {}",sanitize(&g.string(&joined)?))]);return Ok(Some(node));
        }
        None
    };
    Ok(Some(ast(&owned, result.as_ref())))
}
