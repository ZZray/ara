//! Complete original synchronous timing/tree and shared timing-buffer bodies.
//! `time` requires actual Promise/context and is not replaced by a sync stub.
use super::*;

const THRESHOLD: f64 = 0.5;
pub(super) const EXPORTS: &[(&str, &CompiledFunctionSpec)] = &[
    ("timingModeIncludes", &MODE),
    ("shouldExitAfterTimings", &EXIT),
    ("printTimings", &PRINT),
    ("startTiming", &START),
    ("recordModuleLoadSpan", &RECORD),
    ("endTiming", &END),
    ("openSpanPath", &OPEN),
];
pub(super) const INTERNAL: &[(&str, &CompiledFunctionSpec)] = &[
    ("timingModeIncludes", &MODE),
    ("recordModuleLoadSpan", &RECORD),
    ("spliceModuleLoadBuffer", &SPLICE),
    ("shortenLoadPath", &SHORTEN),
    ("durationOf", &DURATION),
    ("selfTimeOf", &SELF_TIME),
    ("fmtMs", &FMT),
    ("isModuleLoadSpan", &IS_LOAD),
    ("printSpan", &PRINT_SPAN),
    ("printModuleLoadSummary", &PRINT_MODULES),
    ("buildModuleTimingGraph", &BUILD_GRAPH),
    ("compareModuleNodes", &COMPARE_MODULES),
    ("renderModuleTimingNode", &RENDER_MODULE),
    ("isParallel", &PARALLEL),
];
fn root(c: &C<'_>) -> Result<ValueHandle> {
    c.state("gRootSpan")
}
fn parent(c: &C<'_>) -> Result<ValueHandle> {
    let stored = c.method(&c.state("spanStorage")?, "getStore", &[])?;
    if c.nullish(&stored)? { root(c) } else { Ok(stored) }
}
fn number_field(c: &C<'_>, object: &ValueHandle, key: &str) -> Result<f64> {
    c.num(&c.get(object, key)?)
}
fn duration(c: &C<'_>, span: &ValueHandle) -> Result<f64> {
    c.num(&c.helper("durationOf", &[span.clone()])?)
}
fn fmt(c: &C<'_>, ms: f64) -> Result<WireString> {
    c.string(&c.helper("fmtMs", &[c.number(ms)?])?)
}
fn line(c: &C<'_>, lines: &ValueHandle, pieces: &[WireString]) -> Result<()> {
    c.push(lines, c.heap.string(c.join_text(pieces))?)
}
fn indent(c: &C<'_>, depth: usize) -> Result<WireString> {
    c.string(&c.method(&c.text("  ")?, "repeat", &[c.number(depth as f64)?])?)
}
fn depth(c: &C<'_>, args: &[ValueHandle], index: usize) -> Result<usize> {
    Ok(c.num(&c.arg(args, index)?)? as usize)
}
fn sort(c: &C<'_>, values: &ValueHandle, spec: &'static CompiledFunctionSpec) -> Result<ValueHandle> {
    c.method(values, "sort", &[c.heap.fresh_compiled_function(spec, c.captures)?])
}
fn split(c: &C<'_>, children: &ValueHandle) -> Result<(ValueHandle, ValueHandle)> {
    let work = c.array(vec![])?;
    let loads = c.array(vec![])?;
    c.each(children, |child| {
        let is_load = c.helper("isModuleLoadSpan", &[child.clone()])?;
        c.push(if c.truth(&is_load)? { &loads } else { &work }, child)
    })?;
    Ok((work, loads))
}

original!(MODE, "timingModeIncludes", 1, mode);
fn mode(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let value = c.env_value("PI_TIMING")?;
    if !c.truth(&value)? {
        return c.heap.boolean(false);
    }
    let option = c.arg(args, 0)?;
    if c.heap.object_is(&value, &option)? {
        return c.heap.boolean(true);
    }
    let mut start = 0usize;
    let mut i = 0usize;
    loop {
        let length = number_field(c, &value, "length")?;
        if i as f64 > length {
            break;
        }
        let code = if i as f64 == number_field(c, &value, "length")? {
            44.0
        } else {
            c.num(&c.method(&value, "charCodeAt", &[c.number(i as f64)?])?)?
        };
        if code == 44.0 || code == 58.0 || code == 59.0 || code == 43.0 || code <= 32.0 {
            if i > start {
                let token = c.method(&value, "slice", &[c.number(start as f64)?, c.number(i as f64)?])?;
                if c.heap.object_is(&token, &option)? {
                    return c.heap.boolean(true);
                }
            }
            start = i + 1;
        }
        i += 1;
    }
    c.heap.boolean(false)
}
original!(EXIT, "shouldExitAfterTimings", 0, should_exit);
fn should_exit(c: &C<'_>, _: &[ValueHandle]) -> Result<ValueHandle> {
    let first = c.helper("timingModeIncludes", &[c.text("x")?])?;
    if c.truth(&first)? { Ok(first) } else { c.helper("timingModeIncludes", &[c.text("full")?]) }
}
original!(START, "startTiming", 0, start);
fn start(c: &C<'_>, _: &[ValueHandle]) -> Result<ValueHandle> {
    if c.truth(&c.state("gRecordTimings")?)? {
        return c.undef();
    }
    c.put(
        "gRootSpan",
        c.object(&[
            ("op", c.text("(root)")?),
            ("start", c.now()?),
            ("parent", c.undef()?),
            ("children", c.array(vec![])?),
        ])?,
    )?;
    c.put("gRecordTimings", c.heap.boolean(true)?)?;
    c.undef()
}
original!(END, "endTiming", 0, end);
fn end(c: &C<'_>, _: &[ValueHandle]) -> Result<ValueHandle> {
    c.put("gRootSpan", c.undef()?)?;
    c.put("gRecordTimings", c.heap.boolean(false)?)?;
    c.undef()
}
original!(RECORD, "recordModuleLoadSpan", 4, record);
fn record(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let imports = c.arg(args, 4)?;
    let imports = if c.undefined(&imports)? { c.array(vec![])? } else { imports };
    if !c.truth(&c.state("gRecordTimings")?)? || !c.truth(&root(c)?)? {
        return c.undef();
    }
    let parent = parent(c)?;
    let path = c.arg(args, 0)?;
    let op = c.interpolate("load:", &c.helper("shortenLoadPath", &[path.clone()])?, "")?;
    let start = c.arg(args, 1)?;
    let end = c.plus(&start, &c.arg(args, 2)?)?;
    let span = c.object(&[
        ("op", op),
        ("start", start),
        ("end", end),
        ("parent", parent.clone()),
        ("children", c.array(vec![])?),
        ("modulePath", path),
        ("moduleBodyMs", c.arg(args, 3)?),
        ("moduleImports", imports),
    ])?;
    c.push(&c.get(&parent, "children")?, span)?;
    c.undef()
}
original!(SHORTEN, "shortenLoadPath", 1, shorten);
fn shorten(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let path = c.arg(args, 0)?;
    let cwd = c.method(&c.env("process")?, "cwd", &[])?;
    let prefix = c.interpolate("", &cwd, "/")?;
    if c.truth(&c.method(&path, "startsWith", &[prefix])?)? {
        let length = c.plus(&c.get(&cwd, "length")?, &c.number(1.0)?)?;
        return c.method(&path, "slice", &[length]);
    }
    let home = c.env_value("HOME")?;
    if c.truth(&home)? && c.truth(&c.method(&path, "startsWith", &[c.interpolate("", &home, "/")?])?)? {
        let length = c.plus(&c.get(&home, "length")?, &c.number(1.0)?)?;
        return c.interpolate("~/", &c.method(&path, "slice", &[length])?, "");
    }
    Ok(path)
}

original!(MODULE_BUFFER, "moduleLoadBuffer", 0, buffer);
fn buffer(c: &C<'_>, _: &[ValueHandle]) -> Result<ValueHandle> {
    let store = c.env("globalThis")?;
    let key = PropertyKey::Symbol(c.heap.symbol_key(&c.state("moduleBufferKey")?)?);
    let mut buffer = c.heap.get(&store, &key, c.runtime)?;
    if !c.truth(&buffer)? {
        buffer = c.array(vec![])?;
        c.heap.assign(&store, key, buffer.clone(), c.runtime)?;
    }
    Ok(buffer)
}
original!(DRAIN_BUFFER, "drainModuleLoadEvents", 0, drain);
fn drain(c: &C<'_>, _: &[ValueHandle]) -> Result<ValueHandle> {
    let store = c.env("globalThis")?;
    let key = PropertyKey::Symbol(c.heap.symbol_key(&c.state("moduleBufferKey")?)?);
    let buffer = c.heap.get(&store, &key, c.runtime)?;
    if !c.truth(&buffer)?
        || matches!(c.heap.snapshot(&c.get(&buffer, "length")?)?, ValueSnapshot::Number(n) if n == 0.0)
    {
        return c.array(vec![]);
    }
    c.heap.assign(&store, key, c.array(vec![])?, c.runtime)?;
    Ok(buffer)
}
original!(SPLICE, "spliceModuleLoadBuffer", 0, splice);
fn splice(c: &C<'_>, _: &[ValueHandle]) -> Result<ValueHandle> {
    if !c.truth(&root(c)?)? {
        return c.undef();
    }
    let events = c.helper("drainModuleLoadEvents", &[])?;
    if matches!(c.heap.snapshot(&c.get(&events, "length")?)?, ValueSnapshot::Number(n) if n == 0.0) {
        return c.undef();
    }
    let mut earliest = c.get(&root(c)?, "start")?;
    c.each(&events, |event| {
        c.helper(
            "recordModuleLoadSpan",
            &[
                c.get(&event, "path")?,
                c.get(&event, "start")?,
                c.get(&event, "durationMs")?,
                c.get(&event, "bodyMs")?,
                c.get(&event, "imports")?,
            ],
        )?;
        if c.num(&c.get(&event, "start")?)? < c.num(&earliest)? {
            earliest = c.get(&event, "start")?;
        }
        Ok(())
    })?;
    c.set(&root(c)?, "start", earliest)?;
    c.undef()
}
original!(OPEN, "openSpanPath", 0, open);
fn open(c: &C<'_>, _: &[ValueHandle]) -> Result<ValueHandle> {
    let ops = c.array(vec![])?;
    let mut node = root(c)?;
    while c.truth(&node)? {
        let children = c.get(&node, "children")?;
        let mut i = number_field(c, &children, "length")? as i64 - 1;
        let mut next = None;
        while i >= 0 {
            let child = c.get(&c.get(&node, "children")?, &i.to_string())?;
            if c.undefined(&c.get(&child, "end")?)? {
                next = Some(c.get(&c.get(&node, "children")?, &i.to_string())?);
                break;
            }
            i -= 1;
        }
        let Some(child) = next else { break };
        c.push(&ops, c.get(&child, "op")?)?;
        node = child;
    }
    Ok(ops)
}
original!(DURATION, "durationOf", 1, duration_body);
fn duration_body(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let span = c.arg(args, 0)?;
    if c.truth(&c.get(&span, "point")?)? || c.undefined(&c.get(&span, "end")?)? {
        return c.number(0.0);
    }
    c.number(number_field(c, &span, "end")? - number_field(c, &span, "start")?)
}
original!(SELF_TIME, "selfTimeOf", 1, self_time);
fn self_time(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let span = c.arg(args, 0)?;
    let dur = duration(c, &span)?;
    if number_field(c, &c.get(&span, "children")?, "length")? == 0.0 || c.truth(&c.get(&span, "point")?)? {
        return c.number(dur);
    }
    let children = c.get(&span, "children")?;
    let filtered = c.method(&children, "filter", &[c.heap.fresh_compiled_function(&FILTER_INTERVAL, c.captures)?])?;
    let mapped = c.method(&filtered, "map", &[c.heap.fresh_compiled_function(&MAP_INTERVAL, c.captures)?])?;
    let intervals = sort(c, &mapped, &COMPARE_INTERVAL)?;
    if number_field(c, &intervals, "length")? == 0.0 {
        return c.number(dur);
    }
    let first = c.get(&intervals, "0")?;
    let mut cur_start = number_field(c, &first, "0")?;
    let mut cur_end = number_field(c, &first, "1")?;
    let mut union = 0.0;
    let mut i = 1usize;
    while (i as f64) < number_field(c, &intervals, "length")? {
        let pair = c.get(&intervals, &i.to_string())?;
        let mut iterator = c.heap.get_iterator(&pair, c.runtime)?;
        let s = c.heap.iterator_next(&mut iterator, c.runtime)?.unwrap_or(c.undef()?);
        let e = c.heap.iterator_next(&mut iterator, c.runtime)?.unwrap_or(c.undef()?);
        c.heap.iterator_close(&mut iterator, c.runtime)?;
        let s = c.num(&s)?;
        let e = c.num(&e)?;
        if s > cur_end {
            union += cur_end - cur_start;
            cur_start = s;
            cur_end = e;
        } else if e > cur_end {
            cur_end = e;
        }
        i += 1;
    }
    union += cur_end - cur_start;
    c.method(&c.env("Math")?, "max", &[c.number(0.0)?, c.number(dur - union)?])
}
macro_rules! arrow {
    ($spec:ident,$len:expr,$body:expr) => {
        static $spec: CompiledFunctionSpec = CompiledFunctionSpec {
            source: "OMP596f2da original logger timing helper arrow",
            name: "",
            length: $len,
            form: CompiledFunctionForm::Arrow,
            capture_count: BASE_CAPTURES,
            body: |heap, captures, _, args, runtime| {
                let c = C { heap, captures, runtime };
                ($body)(&c, args).map_err(|e| c.original_error(e))
            },
        };
    };
}
arrow!(FILTER_INTERVAL, 1, |c: &C<'_>, args: &[ValueHandle]| {
    let child = c.arg(args, 0)?;
    c.heap.boolean(!c.truth(&c.get(&child, "point")?)? && !c.undefined(&c.get(&child, "end")?)?)
});
arrow!(MAP_INTERVAL, 1, |c: &C<'_>, args: &[ValueHandle]| {
    let child = c.arg(args, 0)?;
    c.array(vec![c.get(&child, "start")?, c.get(&child, "end")?])
});
arrow!(COMPARE_INTERVAL, 2, |c: &C<'_>, args: &[ValueHandle]| {
    c.number(number_field(c, &c.arg(args, 0)?, "0")? - number_field(c, &c.arg(args, 1)?, "0")?)
});
arrow!(COMPARE_START, 2, |c: &C<'_>, args: &[ValueHandle]| {
    c.number(number_field(c, &c.arg(args, 0)?, "start")? - number_field(c, &c.arg(args, 1)?, "start")?)
});
arrow!(COMPARE_TOTAL, 2, |c: &C<'_>, args: &[ValueHandle]| {
    c.number(duration(c, &c.get(&c.arg(args, 1)?, "span")?)? - duration(c, &c.get(&c.arg(args, 0)?, "span")?)?)
});
arrow!(ROOT_NODE, 1, |c: &C<'_>, args: &[ValueHandle]| {
    c.heap
        .boolean(matches!(c.heap.snapshot(&c.get(&c.arg(args, 0)?, "parents")?)?, ValueSnapshot::Number(n) if n == 0.0))
});
original!(FMT, "fmtMs", 1, fmt_body);
fn fmt_body(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let ms = c.arg(args, 0)?;
    let digits = if c.num(&ms)? < 1.0 {
        2.0
    } else if c.num(&ms)? < 100.0 {
        1.0
    } else {
        0.0
    };
    let fixed = c.method(&ms, "toFixed", &[c.number(digits)?])?;
    c.interpolate("", &fixed, "ms")
}
original!(IS_LOAD, "isModuleLoadSpan", 1, is_load);
fn is_load(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    c.method(&c.get(&c.arg(args, 0)?, "op")?, "startsWith", &[c.text("load:")?])
}
original!(PARALLEL, "isParallel", 1, parallel);
fn parallel(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let span = c.arg(args, 0)?;
    let parent = c.get(&span, "parent")?;
    if !c.truth(&parent)? || c.undefined(&c.get(&span, "end")?)? {
        return c.heap.boolean(false);
    }
    let mut iterator = c.heap.get_iterator(&c.get(&parent, "children")?, c.runtime)?;
    while let Some(sibling) = c.heap.iterator_next(&mut iterator, c.runtime)? {
        if sibling.same_identity(&span)
            || c.undefined(&c.get(&sibling, "end")?)?
            || c.truth(&c.get(&sibling, "point")?)?
        {
            continue;
        }
        if number_field(c, &sibling, "start")? < number_field(c, &span, "end")?
            && number_field(c, &span, "start")? < number_field(c, &sibling, "end")?
        {
            c.heap.iterator_close(&mut iterator, c.runtime)?;
            return c.heap.boolean(true);
        }
    }
    c.heap.boolean(false)
}
original!(PRINT_SPAN, "printSpan", 3, print_span);
fn print_span(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let span = c.arg(args, 0)?;
    let d = depth(c, args, 1)?;
    let lines = c.arg(args, 2)?;
    let ind = indent(c, d)?;
    if c.truth(&c.get(&span, "point")?)? {
        line(c, &lines, &[ind, "• ".into(), c.string(&c.get(&span, "op")?)?])?;
        return c.undef();
    }
    let dur = duration(c, &span)?;
    if dur < THRESHOLD && number_field(c, &c.get(&span, "children")?, "length")? == 0.0 {
        return c.undef();
    }
    let parallel = c.truth(&c.helper("isParallel", &[span.clone()])?)?;
    let own = c.num(&c.helper("selfTimeOf", &[span.clone()])?)?;
    let self_str = if number_field(c, &c.get(&span, "children")?, "length")? > 0.0 && own > THRESHOLD {
        c.join_text(&[" (self ".into(), fmt(c, own)?, ")".into()])
    } else {
        "".into()
    };
    line(
        c,
        &lines,
        &[
            ind,
            c.string(&c.get(&span, "op")?)?,
            ": ".into(),
            fmt(c, dur)?,
            self_str,
            (if parallel { " [parallel]" } else { "" }).into(),
        ],
    )?;
    let (work, loads) = split(c, &c.get(&span, "children")?)?;
    let sorted = sort(c, &work, &COMPARE_START)?;
    c.each(&sorted, |child| {
        c.helper("printSpan", &[child, c.number((d + 1) as f64)?, lines.clone()])?;
        Ok(())
    })?;
    if number_field(c, &loads, "length")? > 0.0 {
        c.helper("printModuleLoadSummary", &[loads, c.number((d + 1) as f64)?, lines])?;
    }
    c.undef()
}

original!(BUILD_GRAPH, "buildModuleTimingGraph", 1, build_graph);
fn build_graph(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let nodes = c.construct("Map", &[])?;
    c.each(&c.arg(args, 0)?, |span| {
        if !c.truth(&c.get(&span, "modulePath")?)? || c.undefined(&c.get(&span, "end")?)? {
            return Ok(());
        }
        let path = c.get(&span, "modulePath")?;
        let body = c.get(&span, "moduleBodyMs")?;
        let body = if c.nullish(&body)? { c.number(0.0)? } else { body };
        let node =
            c.object(&[("span", span), ("children", c.array(vec![])?), ("parents", c.number(0.0)?), ("body", body)])?;
        c.method(&nodes, "set", &[path, node])?;
        Ok(())
    })?;
    c.each(&c.method(&nodes, "values", &[])?, |node| {
        let imports = c.get(&c.get(&node, "span")?, "moduleImports")?;
        let imports = if c.nullish(&imports)? { c.array(vec![])? } else { imports };
        c.each(&imports, |path| {
            let child = c.method(&nodes, "get", &[path])?;
            if !c.truth(&child)? || child.same_identity(&node) {
                return Ok(());
            }
            c.push(&c.get(&node, "children")?, child.clone())?;
            c.set(&child, "parents", c.number(number_field(c, &child, "parents")? + 1.0)?)
        })
    })?;
    c.each(&c.method(&nodes, "values", &[])?, |node| {
        c.method(&c.get(&node, "children")?, "sort", &[c.state("compareModuleNodes")?])?;
        Ok(())
    })?;
    c.array(c.list(&c.method(&nodes, "values", &[])?)?)
}
original!(COMPARE_MODULES, "compareModuleNodes", 2, compare_modules);
fn compare_modules(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let a = c.arg(args, 0)?;
    let b = c.arg(args, 1)?;
    let diff = number_field(c, &b, "body")? - number_field(c, &a, "body")?;
    let absolute = c.method(&c.env("Math")?, "abs", &[c.number(diff)?])?;
    if c.num(&absolute)? > 0.001 {
        return c.number(diff);
    }
    c.number(duration(c, &c.get(&b, "span")?)? - duration(c, &c.get(&a, "span")?)?)
}
original!(PRINT_MODULES, "printModuleLoadSummary", 3, print_modules);
fn print_modules(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let loads = c.arg(args, 0)?;
    let d = depth(c, args, 1)?;
    let lines = c.arg(args, 2)?;
    let child_indent = indent(c, d)?;
    let grand_indent = indent(c, d + 1)?;
    let mut union_start = f64::INFINITY;
    let mut union_end = 0.0;
    c.each(&loads, |span| {
        if c.undefined(&c.get(&span, "end")?)? {
            return Ok(());
        }
        if number_field(c, &span, "start")? < union_start {
            union_start = number_field(c, &span, "start")?;
        }
        if number_field(c, &span, "end")? > union_end {
            union_end = number_field(c, &span, "end")?;
        }
        Ok(())
    })?;
    let wall = if union_end > union_start { union_end - union_start } else { 0.0 };
    let nodes = c.helper("buildModuleTimingGraph", &[loads.clone()])?;
    line(
        c,
        &lines,
        &[
            child_indent,
            "(modules): ".into(),
            c.string(&c.get(&loads, "length")?)?,
            " loaded, wall ".into(),
            fmt(c, wall)?,
        ],
    )?;
    if number_field(c, &nodes, "length")? == 0.0 {
        return c.undef();
    }
    let show_all = c.truth(&c.helper("timingModeIncludes", &[c.text("full")?])?)?;
    let by_body = c.array(c.list(&nodes)?)?;
    c.method(&by_body, "sort", &[c.state("compareModuleNodes")?])?;
    let top_body =
        if show_all { by_body.clone() } else { c.method(&by_body, "slice", &[c.number(0.0)?, c.number(10.0)?])? };
    line(c, &lines, &[grand_indent.clone(), "top body/TLA:".into()])?;
    let mut top_iterator = c.heap.get_iterator(&top_body, c.runtime)?;
    while let Some(node) = c.heap.iterator_next(&mut top_iterator, c.runtime)? {
        if !show_all && number_field(c, &node, "body")? < THRESHOLD {
            c.heap.iterator_close(&mut top_iterator, c.runtime)?;
            break;
        }
        let span = c.get(&node, "span")?;
        line(
            c,
            &lines,
            &[
                grand_indent.clone(),
                "  ".into(),
                c.string(&c.get(&span, "op")?)?,
                ": body ".into(),
                fmt(c, number_field(c, &node, "body")?)?,
                " (total ".into(),
                fmt(c, duration(c, &span)?)?,
                ")".into(),
            ],
        )?;
    }
    if !show_all && number_field(c, &by_body, "length")? > 10.0 {
        line(
            c,
            &lines,
            &[
                grand_indent.clone(),
                format!("  … {} more (PI_TIMING=full to show all)", number_field(c, &by_body, "length")? - 10.0).into(),
            ],
        )?;
    }
    let roots = c.method(&nodes, "filter", &[c.heap.fresh_compiled_function(&ROOT_NODE, c.captures)?])?;
    let tree_roots = sort(c, if number_field(c, &roots, "length")? > 0.0 { &roots } else { &nodes }, &COMPARE_TOTAL)?;
    let visible =
        if show_all { tree_roots.clone() } else { c.method(&tree_roots, "slice", &[c.number(0.0)?, c.number(5.0)?])? };
    line(c, &lines, &[grand_indent.clone(), "tree:".into()])?;
    let rendered = c.construct("Set", &[])?;
    c.each(&visible, |node| {
        c.helper(
            "renderModuleTimingNode",
            &[
                node,
                c.number((d + 2) as f64)?,
                lines.clone(),
                rendered.clone(),
                c.construct("Set", &[])?,
                c.heap.boolean(show_all)?,
            ],
        )?;
        Ok(())
    })?;
    if !show_all && number_field(c, &tree_roots, "length")? > 5.0 {
        line(
            c,
            &lines,
            &[
                grand_indent,
                format!(
                    "  … {} more roots (PI_TIMING=full to show all)",
                    number_field(c, &tree_roots, "length")? - 5.0
                )
                .into(),
            ],
        )?;
    }
    c.undef()
}
original!(RENDER_MODULE, "renderModuleTimingNode", 6, render_module);
fn render_module(c: &C<'_>, args: &[ValueHandle]) -> Result<ValueHandle> {
    let node = c.arg(args, 0)?;
    let d = depth(c, args, 1)?;
    let lines = c.arg(args, 2)?;
    let rendered = c.arg(args, 3)?;
    let ancestors = c.arg(args, 4)?;
    let show_all = c.truth(&c.arg(args, 5)?)?;
    let span = c.get(&node, "span")?;
    let path = c.get(&span, "modulePath")?;
    if !c.truth(&path)? {
        return c.undef();
    }
    let ind = indent(c, d)?;
    let total = duration(c, &span)?;
    if !show_all && total < THRESHOLD && number_field(c, &c.get(&node, "children")?, "length")? == 0.0 {
        return c.undef();
    }
    let wait = c.num(&c.method(
        &c.env("Math")?,
        "max",
        &[c.number(0.0)?, c.number(total - number_field(c, &node, "body")?)?],
    )?)?;
    let shared = if number_field(c, &node, "parents")? > 1.0 { " [shared]" } else { "" };
    let timing = if number_field(c, &node, "body")? > THRESHOLD
        || number_field(c, &c.get(&node, "children")?, "length")? > 0.0
    {
        c.join_text(&[
            " (body ".into(),
            fmt(c, number_field(c, &node, "body")?)?,
            ", wait ".into(),
            fmt(c, wait)?,
            ")".into(),
        ])
    } else {
        "".into()
    };
    let already = c.truth(&c.method(&rendered, "has", &[path.clone()])?)?;
    let cycle = c.truth(&c.method(&ancestors, "has", &[path.clone()])?)?;
    let suffix = if cycle {
        " [cycle]"
    } else if already {
        " [already shown]"
    } else {
        ""
    };
    line(
        c,
        &lines,
        &[
            ind.clone(),
            c.string(&c.get(&span, "op")?)?,
            ": ".into(),
            fmt(c, total)?,
            timing,
            shared.into(),
            suffix.into(),
        ],
    )?;
    if cycle || already {
        return c.undef();
    }
    c.method(&rendered, "add", &[path.clone()])?;
    c.method(&ancestors, "add", &[path.clone()])?;
    if !show_all && number_field(c, &ancestors, "size")? >= 5.0 {
        if number_field(c, &c.get(&node, "children")?, "length")? > 0.0 {
            line(
                c,
                &lines,
                &[
                    ind,
                    format!(
                        "  … {} imports deeper (PI_TIMING=full to show all)",
                        number_field(c, &c.get(&node, "children")?, "length")?
                    )
                    .into(),
                ],
            )?;
        }
        c.method(&ancestors, "delete", &[path])?;
        return c.undef();
    }
    let children = c.get(&node, "children")?;
    let visible = if show_all { children } else { c.method(&children, "slice", &[c.number(0.0)?, c.number(8.0)?])? };
    c.each(&visible, |child| {
        c.helper(
            "renderModuleTimingNode",
            &[
                child,
                c.number((d + 1) as f64)?,
                lines.clone(),
                rendered.clone(),
                ancestors.clone(),
                c.heap.boolean(show_all)?,
            ],
        )?;
        Ok(())
    })?;
    if !show_all && number_field(c, &c.get(&node, "children")?, "length")? > 8.0 {
        line(
            c,
            &lines,
            &[
                ind,
                format!(
                    "  … {} more imports (PI_TIMING=full to show all)",
                    number_field(c, &c.get(&node, "children")?, "length")? - 8.0
                )
                .into(),
            ],
        )?;
    }
    c.method(&ancestors, "delete", &[path])?;
    c.undef()
}
original!(PRINT, "printTimings", 0, print);
fn print(c: &C<'_>, _: &[ValueHandle]) -> Result<ValueHandle> {
    if !c.truth(&c.state("gRecordTimings")?)? || !c.truth(&root(c)?)? {
        c.method(&c.env("console")?, "error", &[c.text("\n--- Startup Timings ---\n(no markers)\n")?])?;
        return c.undef();
    }
    c.set(&root(c)?, "end", c.now()?)?;
    c.helper("spliceModuleLoadBuffer", &[])?;
    let lines = c.array(vec![])?;
    c.push(&lines, c.text("")?)?;
    c.push(&lines, c.text("--- Startup timings (hierarchical) ---")?)?;
    if number_field(c, &root(c)?, "start")? > THRESHOLD {
        line(
            c,
            &lines,
            &[
                "(before instrumentation): ".into(),
                fmt(c, number_field(c, &root(c)?, "start")?)?,
                " [runtime init + module load]".into(),
            ],
        )?;
    }
    let (work, loads) = split(c, &c.get(&root(c)?, "children")?)?;
    let sorted = sort(c, &work, &COMPARE_START)?;
    c.each(&sorted, |child| {
        c.helper("printSpan", &[child, c.number(0.0)?, lines.clone()])?;
        Ok(())
    })?;
    if number_field(c, &loads, "length")? > 0.0 {
        c.helper("printModuleLoadSummary", &[loads, c.number(0.0)?, lines.clone()])?;
    }
    let own = c.num(&c.helper("selfTimeOf", &[root(c)?])?)?;
    if number_field(c, &c.get(&root(c)?, "children")?, "length")? > 0.0 && own > THRESHOLD {
        line(c, &lines, &["(unattributed self): ".into(), fmt(c, own)?])?;
    }
    let total = c.number(number_field(c, &root(c)?, "end")? - number_field(c, &root(c)?, "start")?)?;
    let fixed = c.method(&total, "toFixed", &[c.number(1.0)?])?;
    line(c, &lines, &["Total: ".into(), c.string(&fixed)?, "ms (since first marker)".into()])?;
    c.push(&lines, c.text("--------------------------------------")?)?;
    c.push(&lines, c.text("")?)?;
    let console = c.env("console")?;
    let error = c.get(&console, "error")?;
    let output = c.method(&lines, "join", &[c.text("\n")?])?;
    c.heap.call(&error, &console, &[output], c.runtime)?;
    c.set(&root(c)?, "end", c.undef()?)?;
    c.undef()
}
