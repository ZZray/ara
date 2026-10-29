//! Fixed OMP utils/sample-profile.ts. MIT; upstream notices in mod.rs.
use super::tree::{
    ProfileNode, RenderTreeContext, count, format_pct, merge_into, render_profile_node,
};
use super::{append, ascii_suffix, cat, fixed, join, max, number, pad, round, space, trim};
use ara_rpc::json::WireString;
use std::{collections::HashMap, sync::OnceLock};

#[derive(Clone, Debug, PartialEq)]
pub struct SampleFrame {
    pub count: f64,
    pub symbol: WireString,
    pub module: Option<WireString>,
    pub children: Vec<SampleFrame>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SampleThread {
    pub id: WireString,
    pub name: Option<WireString>,
    pub total: f64,
    pub roots: Vec<SampleFrame>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SampleHeader {
    pub process: WireString,
    pub pid: f64,
    pub interval_ms: f64,
    pub path: Option<WireString>,
    pub code_type: Option<WireString>,
    pub os_version: Option<WireString>,
    pub footprint: Option<WireString>,
    pub footprint_peak: Option<WireString>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SampleProfile {
    pub header: SampleHeader,
    pub threads: Vec<SampleThread>,
}
pub fn is_sample_profile_path(path: &str) -> bool {
    ascii_suffix(path, ".sample.txt")
}
const WAIT: &[&str] = &[
    "__accept",
    "__psynch_cvwait",
    "__psynch_mutexwait",
    "__psynch_rw_rdlock",
    "__psynch_rw_wrlock",
    "__recvfrom",
    "__select",
    "__semwait_signal",
    "__sigwait",
    "__ulock_wait",
    "__ulock_wait2",
    "__wait4",
    "__workq_kernreturn",
    "kevent",
    "kevent64",
    "kevent_qos",
    "mach_msg2_trap",
    "mach_msg_trap",
    "poll",
    "semaphore_timedwait_trap",
    "semaphore_wait_trap",
    "start_wqthread",
    "swtch_pri",
    "thread_suspend",
    "usleep",
];
// WAIT_SYMBOLS is an ordinary object, not a null-prototype set. Preserve its
// inherited truthy properties for malformed/custom frame names as well.
const INHERITED: &[&str] = &[
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
    "__proto__",
];
fn wait(symbol: &WireString) -> bool {
    WAIT.iter()
        .chain(INHERITED)
        .any(|name| symbol.equals_ascii(name))
}
fn decimal(units: &[u16]) -> f64 {
    String::from_utf16(units)
        .expect("ASCII digits")
        .parse::<f64>()
        .unwrap_or(f64::INFINITY)
}
fn terminator(unit: u16) -> bool {
    matches!(unit, 10 | 13 | 0x2028 | 0x2029)
}
fn find_last(units: &[u16], needle: &str) -> Option<usize> {
    let needle: Vec<u16> = needle.encode_utf16().collect();
    units.windows(needle.len()).rposition(|row| row == needle)
}
fn frame_text(text: &[u16]) -> (WireString, Option<WireString>) {
    let mut text = text;
    if text.last() == Some(&93) {
        if let Some(index) = find_last(text, "  [") {
            text = &text[..index];
        }
    }
    if let Some(index) = find_last(text, " + ") {
        let suffix = &text[index + 3..];
        if suffix.first().is_some_and(|unit| (48..=57).contains(unit))
            && suffix
                .iter()
                .all(|unit| (48..=57).contains(unit) || matches!(*unit, 44 | 46))
        {
            text = &text[..index];
        }
    }
    if text.last() == Some(&41) {
        if let Some(index) = find_last(text, "  (in ") {
            return (
                WireString::from_units(trim(&text[..index]).to_vec()),
                Some(WireString::from_units(
                    text[index + 6..text.len() - 1].to_vec(),
                )),
            );
        }
    }
    (WireString::from_units(trim(text).to_vec()), None)
}
fn frame_at<'a>(roots: &'a mut [SampleFrame], path: &[usize]) -> &'a mut SampleFrame {
    let mut frame = &mut roots[path[0]];
    for &index in &path[1..] {
        frame = &mut frame.children[index];
    }
    frame
}
pub fn parse_sample_profile(text: &str) -> Option<SampleProfile> {
    static ANALYSIS: OnceLock<regex::Regex> = OnceLock::new();
    let analysis_re=ANALYSIS.get_or_init(||regex::Regex::new(r"^Analysis of sampling ([^\n\r\x{2028}\x{2029}]+?) \(pid ([0-9]+)\) every ([0-9]+) milliseconds?").unwrap());
    let lines: Vec<&str> = text.split('\n').collect();
    let analysis = analysis_re.captures(lines.first().copied().unwrap_or(""))?;
    let graph = lines.iter().position(|line| *line == "Call graph:")?;
    let mut header = SampleHeader {
        process: analysis[1].into(),
        pid: analysis[2].parse().unwrap_or(f64::INFINITY),
        interval_ms: analysis[3].parse().unwrap_or(f64::INFINITY),
        path: None,
        code_type: None,
        os_version: None,
        footprint: None,
        footprint_peak: None,
    };
    for line in &lines[1..graph] {
        let units: Vec<u16> = line.encode_utf16().collect();
        let Some(colon) = units.iter().position(|unit| *unit == 58) else {
            continue;
        };
        let key = &units[..colon];
        if key.is_empty()
            || !key.iter().all(|unit| {
                (*unit <= 127 && (char::from(*unit as u8).is_ascii_alphabetic()))
                    || matches!(*unit, 47 | 32 | 40 | 41)
            })
        {
            continue;
        }
        let rest = &units[colon + 1..];
        if !rest.first().is_some_and(|unit| space(*unit)) {
            continue;
        }
        let mut start = 0;
        while start < rest.len() && space(rest[start]) {
            start += 1;
        }
        if rest[start..].iter().any(|unit| terminator(*unit)) {
            continue;
        }
        let value = WireString::from_units(trim(&rest[start..]).to_vec());
        let key = String::from_utf16(key).unwrap();
        match key.as_str() {
            "Path" => header.path = Some(value),
            "Code Type" => header.code_type = Some(value),
            "OS Version" => header.os_version = Some(value),
            "Physical footprint" => header.footprint = Some(value),
            "Physical footprint (peak)" => header.footprint_peak = Some(value),
            _ => {}
        }
    }
    let mut threads: Vec<SampleThread> = Vec::new();
    let mut stack: Vec<(usize, Vec<usize>)> = Vec::new();
    for line in &lines[graph + 1..] {
        if [
            "Total number in stack",
            "Sort by top of stack",
            "Binary Images:",
        ]
        .iter()
        .any(|prefix| line.starts_with(prefix))
        {
            break;
        }
        let Some(body) = line.strip_prefix("    ") else {
            continue;
        };
        let body: Vec<u16> = body.encode_utf16().collect();
        let mut p = 0;
        while p < body.len() && !(48..=57).contains(&body[p]) {
            p += 2;
        }
        if p >= body.len() {
            continue;
        }
        let depth = p / 2;
        let mut end = p;
        while end < body.len() && (48..=57).contains(&body[end]) {
            end += 1;
        }
        if end >= body.len() || !space(body[end]) {
            continue;
        }
        let count = decimal(&body[p..end]);
        while end < body.len() && space(body[end]) {
            end += 1;
        }
        let rest = &body[end..];
        // Fixed outer /^(\d+)\s+(.*)$/ requires the remaining frame
        // capture to reach end-of-input. Do not clip a failed line match.
        if rest.iter().any(|unit| terminator(*unit)) {
            continue;
        }
        if depth == 0 {
            let prefix: Vec<u16> = "Thread_".encode_utf16().collect();
            if !rest.starts_with(&prefix) {
                continue;
            }
            let mut index = prefix.len();
            let start = index;
            while index < rest.len() && !space(rest[index]) && rest[index] != 58 {
                index += 1;
            }
            if index == start {
                continue;
            }
            let id = WireString::from_units(rest[start..index].to_vec());
            if rest.get(index) == Some(&58) {
                index += 1;
            }
            while index < rest.len() && space(rest[index]) {
                index += 1;
            }
            // THREAD_RE consumes the already validated whole-line capture.
            let name_units = trim(&rest[index..]);
            let mut name = Vec::new();
            let mut in_space = false;
            for &unit in name_units {
                if space(unit) {
                    if !in_space {
                        name.push(32);
                    }
                    in_space = true;
                } else {
                    name.push(unit);
                    in_space = false;
                }
            }
            threads.push(SampleThread {
                id,
                name: (!name.is_empty()).then(|| WireString::from_units(name)),
                total: count,
                roots: Vec::new(),
            });
            stack.clear();
            continue;
        }
        let Some(thread) = threads.last_mut() else {
            continue;
        };
        let (symbol, module) = frame_text(rest);
        let frame = SampleFrame {
            count,
            symbol,
            module,
            children: Vec::new(),
        };
        while stack.last().is_some_and(|(level, _)| *level >= depth) {
            stack.pop();
        }
        let mut path = if let Some((_, path)) = stack.last() {
            path.clone()
        } else {
            Vec::new()
        };
        let siblings = if path.is_empty() {
            &mut thread.roots
        } else {
            &mut frame_at(&mut thread.roots, &path).children
        };
        path.push(siblings.len());
        siblings.push(frame);
        stack.push((depth, path));
    }
    (!threads.is_empty()).then_some(SampleProfile { header, threads })
}
fn replace_all(text: &[u16], from: &str, to: &str) -> Vec<u16> {
    let from: Vec<u16> = from.encode_utf16().collect();
    let to: Vec<u16> = to.encode_utf16().collect();
    let mut out = Vec::new();
    let mut index = 0;
    while index < text.len() {
        if text[index..].starts_with(&from) {
            out.extend_from_slice(&to);
            index += from.len();
        } else {
            out.push(text[index]);
            index += 1;
        }
    }
    out
}
fn ident(units: &[u16]) -> bool {
    !units.is_empty()
        && units
            .iter()
            .all(|unit| matches!(*unit,48..=57|65..=90|97..=122|95|46|36))
}
fn v0(raw: &WireString) -> Option<WireString> {
    let text = &raw.units()[2..];
    let mut parts = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let ch = text[index];
        if (49..=57).contains(&ch) {
            let mut end = index;
            while end < text.len() && (48..=57).contains(&text[end]) {
                end += 1;
            }
            let length = decimal(&text[index..end]);
            let mut start = end;
            if text.get(start) == Some(&95) {
                start += 1;
            }
            let stop = if length.is_finite() && length <= usize::MAX as f64 {
                start.saturating_add(length as usize).min(text.len())
            } else {
                text.len()
            };
            let value = &text[start..stop];
            if value.len() as f64 == length && ident(value) {
                parts.push(WireString::from_units(value.to_vec()));
                index = stop;
            } else {
                index = end;
            }
        } else if matches!(ch, 115 | 66) {
            let mut end = index + 1;
            while end < text.len() && matches!(text[end],48..=57|65..=90|97..=122) {
                end += 1;
            }
            index = if text.get(end) == Some(&95) {
                end + 1
            } else {
                index + 1
            };
        } else {
            index += 1;
        }
    }
    (!parts.is_empty()).then(|| join(&parts, "::"))
}
fn legacy(text: &[u16]) -> Option<WireString> {
    let mut parts = Vec::new();
    let mut index = 0;
    while index < text.len() && (48..=57).contains(&text[index]) {
        let mut end = index;
        while end < text.len() && (48..=57).contains(&text[end]) {
            end += 1;
        }
        let length = decimal(&text[index..end]);
        let stop = if length.is_finite() && length <= usize::MAX as f64 {
            end.saturating_add(length as usize).min(text.len())
        } else {
            text.len()
        };
        let value = &text[end..stop];
        if value.len() as f64 != length {
            break;
        }
        let hash = value.len() == 17
            && value[0] == 104
            && value[1..]
                .iter()
                .all(|unit| matches!(*unit,48..=57|97..=102));
        if !hash {
            let mut value = value.to_vec();
            for (from, to) in [
                ("$LT$", "<"),
                ("$GT$", ">"),
                ("$RF$", "&"),
                ("$BP$", "*"),
                ("$C$", ","),
                ("$u20$", " "),
                ("$u27$", "'"),
                ("$u7b$", "{"),
                ("$u7d$", "}"),
                ("..", "::"),
            ] {
                value = replace_all(&value, from, to);
            }
            parts.push(WireString::from_units(value));
        }
        index = stop;
    }
    (!parts.is_empty()).then(|| join(&parts, "::"))
}
pub fn demangle_symbol(raw: &WireString) -> WireString {
    if raw.units().starts_with(&[95, 82]) {
        return v0(raw).unwrap_or_else(|| raw.clone());
    }
    let units = raw.units();
    let start = if units.starts_with(&[95, 95, 90, 78]) {
        Some(4)
    } else if units.starts_with(&[95, 90, 78]) {
        Some(3)
    } else {
        None
    };
    if let Some(start) = start {
        if !units[start..].iter().any(|unit| terminator(*unit)) {
            return legacy(&units[start..]).unwrap_or_else(|| raw.clone());
        }
    }
    raw.clone()
}
fn self_of(frame: &SampleFrame) -> f64 {
    max(
        0.0,
        frame.count
            - frame
                .children
                .iter()
                .fold(0.0, |sum, child| sum + child.count),
    )
}
fn cached(symbol: &WireString, cache: &mut HashMap<WireString, WireString>) -> WireString {
    cache
        .entry(symbol.clone())
        .or_insert_with(|| demangle_symbol(symbol))
        .clone()
}
fn build(
    frame: &SampleFrame,
    cache: &mut HashMap<WireString, WireString>,
    main: &WireString,
) -> ProfileNode {
    let symbol = cached(&frame.symbol, cache);
    let mut children: Vec<ProfileNode> = Vec::new();
    for child in &frame.children {
        let child = build(child, cache, main);
        if let Some(row) = children.iter_mut().find(|row| row.key == child.key) {
            merge_into(row, child);
        } else {
            children.push(child);
        }
    }
    let mut value = if wait(&frame.symbol) {
        0.0
    } else {
        self_of(frame)
    };
    for child in &children {
        value += child.value;
    }
    let label = match &frame.module {
        Some(module) if !module.is_empty() && module != main => {
            cat(&[&symbol, &" (".into(), module, &")".into()])
        }
        _ => symbol.clone(),
    };
    ProfileNode {
        key: symbol,
        label,
        value,
        recursion: 0,
        children,
    }
}
fn totals_visit(
    frame: &SampleFrame,
    cache: &mut HashMap<WireString, WireString>,
    totals: &mut Vec<(WireString, f64, Option<WireString>)>,
) {
    if !wait(&frame.symbol) {
        let value = self_of(frame);
        if value > 0.0 {
            let symbol = cached(&frame.symbol, cache);
            if let Some((_, total, _)) = totals.iter_mut().find(|(key, _, _)| *key == symbol) {
                *total += value;
            } else {
                totals.push((symbol, value, frame.module.clone()));
            }
        }
    }
    for child in &frame.children {
        totals_visit(child, cache, totals);
    }
}
fn dominant_visit(frame: &SampleFrame, best: &mut Option<(WireString, f64)>) {
    if frame.children.is_empty()
        && wait(&frame.symbol)
        && best.as_ref().is_none_or(|(_, count)| frame.count > *count)
    {
        *best = Some((frame.symbol.clone(), frame.count));
    }
    for child in &frame.children {
        dominant_visit(child, best);
    }
}
fn title(thread: &SampleThread) -> WireString {
    let id = cat(&[&"Thread_".into(), &thread.id]);
    match &thread.name {
        Some(name) if !name.is_empty() => cat(&[name, &" (".into(), &id, &")".into()]),
        _ => id,
    }
}
pub fn render_sample_profile(text: &str) -> Option<WireString> {
    let profile = parse_sample_profile(text)?;
    let mut cache = HashMap::new();
    let mut annotated: Vec<(usize, Vec<ProfileNode>, f64)> = profile
        .threads
        .iter()
        .enumerate()
        .map(|(index, thread)| {
            let roots: Vec<_> = thread
                .roots
                .iter()
                .map(|frame| build(frame, &mut cache, &profile.header.process))
                .collect();
            let cpu = roots.iter().fold(0.0, |sum, root| sum + root.value);
            (index, roots, cpu)
        })
        .collect();
    let total = annotated.iter().fold(0.0, |sum, (_, _, cpu)| sum + cpu);
    let max_samples = profile
        .threads
        .iter()
        .fold(0.0, |value, thread| max(value, thread.total));
    let header = &profile.header;
    let mut out = vec![cat(&[
        &"macOS sample profile: ".into(),
        &header.process,
        &" (pid ".into(),
        &number(header.pid),
        &"), sampled every ".into(),
        &number(header.interval_ms),
        &" ms".into(),
    ])];
    let mut meta = Vec::new();
    if let Some(path) = header.path.as_ref().filter(|row| !row.is_empty()) {
        meta.push(path.clone());
    }
    if let Some(code) = header.code_type.as_ref().filter(|row| !row.is_empty()) {
        meta.push(code.clone());
    }
    if let Some(os) = header.os_version.as_ref().filter(|row| !row.is_empty()) {
        let prefix: Vec<u16> = "macOS".encode_utf16().collect();
        let mut units = os.units();
        if units.starts_with(&prefix) && units.get(prefix.len()).is_some_and(|unit| space(*unit)) {
            units = &units[prefix.len()..];
            while units.first().is_some_and(|unit| space(*unit)) {
                units = &units[1..];
            }
        }
        meta.push(cat(&[
            &"macOS ".into(),
            &WireString::from_units(units.to_vec()),
        ]));
    }
    if !meta.is_empty() {
        out.push(join(&meta, " | "));
    }
    let mut statistics = cat(&[
        &"Duration: ~".into(),
        &fixed(max_samples * header.interval_ms / 1000.0, 1),
        &" s (".into(),
        &number(max_samples),
        &" samples/thread)".into(),
    ]);
    if let Some(footprint) = header.footprint.as_ref().filter(|row| !row.is_empty()) {
        append(
            &mut statistics,
            &cat(&[&" | Footprint: ".into(), footprint]),
        );
        if let Some(peak) = header.footprint_peak.as_ref().filter(|row| !row.is_empty()) {
            append(
                &mut statistics,
                &cat(&[&" (peak ".into(), peak, &")".into()]),
            );
        }
    }
    out.push(statistics);
    out.push("".into());
    out.push(cat(&[&"Process total: ".into(),&number(total),&format!(" on-CPU samples across {} threads. Counts and percentages below are on-CPU samples (blocked time excluded).",profile.threads.len()).into()]));
    let mut active = Vec::new();
    let mut idle = Vec::new();
    for (index, _, cpu) in &annotated {
        let threshold = max(10.0, profile.threads[*index].total * 0.002);
        if *cpu >= threshold {
            active.push(*index);
        } else {
            idle.push(*index);
        }
    }
    active.sort_by(|&a, &b| {
        (annotated[b].2 - annotated[a].2)
            .partial_cmp(&0.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for index in active {
        let thread = &profile.threads[index];
        let cpu = annotated[index].2;
        out.push("".into());
        out.push(cat(&[
            &"## ".into(),
            &title(thread),
            &" — ".into(),
            &number(thread.total),
            &" samples, ".into(),
            &number(cpu),
            &" on-CPU (".into(),
            &format_pct(cpu, thread.total),
            &")".into(),
        ]));
        let mut ctx = RenderTreeContext {
            out,
            total: cpu,
            min_value: max(3.0, round(cpu * 0.02)),
            format_value: count,
            value_width: 6,
        };
        let roots = &mut annotated[index].1;
        let mut kept: Vec<usize> = roots
            .iter()
            .enumerate()
            .filter(|(_, root)| root.value >= ctx.min_value)
            .map(|(index, _)| index)
            .collect();
        kept.sort_by(|&a, &b| {
            (roots[b].value - roots[a].value)
                .partial_cmp(&0.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if kept.is_empty() {
            ctx.out.push(cat(&[
                &"  (no call path above ".into(),
                &number(ctx.min_value),
                &" on-CPU samples)".into(),
            ]));
        }
        for root in kept {
            render_profile_node(&mut roots[root], 0, &mut ctx);
        }
        out = ctx.out;
    }
    if !idle.is_empty() {
        out.push("".into());
        out.push(format!("## Idle / negligible threads ({})", idle.len()).into());
        for index in idle {
            let thread = &profile.threads[index];
            let cpu = annotated[index].2;
            let mut best = None;
            for root in &thread.roots {
                dominant_visit(root, &mut best);
            }
            let state = if let Some((symbol, _)) = best {
                cat(&[
                    &"blocked in ".into(),
                    &symbol,
                    &" (".into(),
                    &number(cpu),
                    &" on-CPU)".into(),
                ])
            } else {
                cat(&[
                    &number(cpu),
                    &"/".into(),
                    &number(thread.total),
                    &" samples on-CPU".into(),
                ])
            };
            out.push(cat(&[&"- ".into(), &title(thread), &": ".into(), &state]));
        }
    }
    let mut totals = Vec::new();
    for thread in &profile.threads {
        for root in &thread.roots {
            totals_visit(root, &mut cache, &mut totals);
        }
    }
    totals.sort_by(|a, b| {
        (b.1 - a.1)
            .partial_cmp(&0.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if !totals.is_empty() {
        out.push("".into());
        out.push("## Top functions by self samples (process-wide, blocked time excluded)".into());
        for (symbol, cpu, module) in totals.into_iter().take(20) {
            let suffix = match module {
                Some(module) if !module.is_empty() && module != header.process => {
                    cat(&[&" (".into(), &module, &")".into()])
                }
                _ => "".into(),
            };
            out.push(cat(&[
                &pad(&number(cpu), 6),
                &" ".into(),
                &pad(&format_pct(cpu, total), 6),
                &"  ".into(),
                &symbol,
                &suffix,
            ]));
        }
    }
    out.push("".into());
    out.push("[Summarized view of a macOS `sample` call-tree report. Use ':raw' to read the original file.]".into());
    Some(join(&out, "\n"))
}
