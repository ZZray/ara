//! Fixed OMP utils/cpuprofile.ts. MIT; upstream notices in mod.rs.
use super::tree::{
    ProfileNode, RenderTreeContext, format_pct, merge_into, millis, render_profile_node,
};
use super::{
    ProfileError, add, append, ascii_suffix, cat, fixed, join, max, number, pad, round, to_number,
    truthy,
};
use ara_rpc::json::{WireString, WireValue};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
pub struct CpuProfileNode {
    pub id: f64,
    pub call_frame: WireValue,
    pub hit_count: Option<WireValue>,
    pub children: Option<WireValue>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct CpuProfile {
    pub nodes: Vec<CpuProfileNode>,
    pub start_time: f64,
    pub end_time: f64,
    pub samples: Option<Vec<WireValue>>,
    pub time_deltas: Option<Vec<WireValue>>,
}
pub fn is_cpu_profile_path(path: &str) -> bool {
    ascii_suffix(path, ".cpuprofile")
}
pub fn parse_cpu_profile(text: &str) -> Option<CpuProfile> {
    let mut data = WireValue::parse(text).ok()?;
    if data.get("nodes").is_none()
        && data
            .get("profile")
            .is_some_and(|v| matches!(v, WireValue::Object(_) | WireValue::Array(_)))
    {
        data = data.get("profile")?.clone();
    }
    if !matches!(data, WireValue::Object(_) | WireValue::Array(_)) {
        return None;
    }
    let raw_nodes = data.get("nodes")?.as_array()?;
    if raw_nodes.is_empty() {
        return None;
    }
    let start_time = data.get("startTime")?.as_number()?;
    let end_time = data.get("endTime")?.as_number()?;
    let mut nodes = Vec::new();
    for node in raw_nodes {
        let id = node.get("id")?.as_number()?;
        let frame = node.get("callFrame")?;
        frame.get("functionName")?.as_string()?;
        nodes.push(CpuProfileNode {
            id,
            call_frame: frame.clone(),
            hit_count: node.get("hitCount").cloned(),
            children: node.get("children").cloned(),
        });
    }
    Some(CpuProfile {
        nodes,
        start_time,
        end_time,
        samples: data
            .get("samples")
            .and_then(WireValue::as_array)
            .map(<[_]>::to_vec),
        time_deltas: data
            .get("timeDeltas")
            .and_then(WireValue::as_array)
            .map(<[_]>::to_vec),
    })
}
fn key(id: f64) -> u64 {
    if id == 0.0 {
        0
    } else if id.is_nan() {
        f64::NAN.to_bits()
    } else {
        id.to_bits()
    }
}
fn numeric_children(node: &CpuProfileNode) -> Result<Vec<f64>, ProfileError> {
    match node.children.as_ref() {
        None | Some(WireValue::Null) => Ok(Vec::new()),
        Some(WireValue::Array(values)) => {
            Ok(values.iter().filter_map(WireValue::as_number).collect())
        }
        // Strings are iterable; their string-valued characters cannot match
        // the required numeric node ids. Other arbitrary values must throw.
        Some(WireValue::String(_)) => Ok(Vec::new()),
        Some(_) => Err(ProfileError::type_error("node.children is not iterable")),
    }
}
fn short_url(url: &WireString) -> WireString {
    let mut units = url.units();
    let prefix: Vec<u16> = "file://".encode_utf16().collect();
    if units.starts_with(&prefix) {
        units = &units[prefix.len()..];
    }
    let marker: Vec<u16> = "node_modules/".encode_utf16().collect();
    if let Some(index) = units.windows(marker.len()).rposition(|row| row == marker) {
        if index > 0 {
            return WireString::from_units(units[index..].to_vec());
        }
    }
    let parts: Vec<&[u16]> = units.split(|unit| *unit == 47).collect();
    if parts.len() > 4 {
        let tail = parts[parts.len() - 3..]
            .iter()
            .map(|row| WireString::from_units(row.to_vec()))
            .collect::<Vec<_>>();
        cat(&[&"…/".into(), &join(&tail, "/")])
    } else {
        WireString::from_units(units.to_vec())
    }
}
fn frame_label(frame: &WireValue) -> Result<WireString, ProfileError> {
    let function = frame
        .get("functionName")
        .and_then(WireValue::as_string)
        .expect("validated callFrame");
    let name = if function.is_empty() {
        "(anonymous)".into()
    } else {
        function.clone()
    };
    if !truthy(frame.get("url")) {
        return Ok(name);
    }
    let url = frame
        .get("url")
        .and_then(WireValue::as_string)
        .ok_or_else(|| ProfileError::type_error("url.startsWith is not a function"))?;
    let line = match frame.get("lineNumber").and_then(WireValue::as_number) {
        Some(line) if line >= 0.0 => cat(&[&":".into(), &number(line + 1.0)]),
        _ => "".into(),
    };
    Ok(cat(&[
        &name,
        &" (".into(),
        &short_url(url),
        &line,
        &")".into(),
    ]))
}
fn self_micros(profile: &CpuProfile) -> Result<HashMap<u64, f64>, ProfileError> {
    let mut values = HashMap::new();
    if let (Some(samples), Some(deltas)) = (&profile.samples, &profile.time_deltas) {
        if !samples.is_empty() {
            for (id, delta) in samples.iter().zip(deltas) {
                let Some(delta) = delta.as_number() else {
                    continue;
                };
                if delta <= 0.0 {
                    continue;
                }
                // Unknown/non-numeric sample IDs never match any validated
                // numeric node id; retain original full samples length in header.
                if let Some(id) = id.as_number() {
                    *values.entry(key(id)).or_insert(0.0) += delta;
                }
            }
            return Ok(values);
        }
    }
    let mut hits = WireValue::Number(0.0);
    for node in &profile.nodes {
        let value = node
            .hit_count
            .as_ref()
            .filter(|row| !matches!(row, WireValue::Null));
        hits = add(&hits, value.unwrap_or(&WireValue::Number(0.0)))?;
    }
    if hits.as_number() == Some(0.0) {
        return Ok(values);
    }
    let interval = (profile.end_time - profile.start_time) / to_number(Some(&hits))?;
    for node in &profile.nodes {
        if truthy(node.hit_count.as_ref()) {
            values.insert(key(node.id), to_number(node.hit_count.as_ref())? * interval);
        }
    }
    Ok(values)
}
fn build(
    index: usize,
    profile: &CpuProfile,
    by_id: &HashMap<u64, usize>,
    values: &HashMap<u64, f64>,
    visited: &mut HashSet<u64>,
) -> Result<ProfileNode, ProfileError> {
    let node = &profile.nodes[index];
    visited.insert(key(node.id));
    let mut children: Vec<ProfileNode> = Vec::new();
    for id in numeric_children(node)? {
        let Some(&index) = by_id.get(&key(id)) else {
            continue;
        };
        if visited.contains(&key(id)) {
            continue;
        }
        let child = build(index, profile, by_id, values, visited)?;
        if let Some(existing) = children.iter_mut().find(|row| row.key == child.key) {
            merge_into(existing, child);
        } else {
            children.push(child);
        }
    }
    let idle = node
        .call_frame
        .get("functionName")
        .and_then(WireValue::as_string)
        .is_some_and(|name| name.equals_ascii("(idle)"));
    let mut value = if idle {
        0.0
    } else {
        *values.get(&key(node.id)).unwrap_or(&0.0)
    };
    for child in &children {
        value += child.value;
    }
    let label = frame_label(&node.call_frame)?;
    Ok(ProfileNode {
        key: label.clone(),
        label,
        value,
        recursion: 0,
        children,
    })
}
pub fn render_cpu_profile(text: &str) -> Result<Option<WireString>, ProfileError> {
    let Some(profile) = parse_cpu_profile(text) else {
        return Ok(None);
    };
    let mut by_id = HashMap::new();
    let mut referenced = HashSet::new();
    for (index, node) in profile.nodes.iter().enumerate() {
        by_id.insert(key(node.id), index);
        for id in numeric_children(node)? {
            referenced.insert(key(id));
        }
    }
    let values = self_micros(&profile)?;
    let mut visited = HashSet::new();
    let mut roots = Vec::new();
    for (index, node) in profile.nodes.iter().enumerate() {
        if referenced.contains(&key(node.id)) || visited.contains(&key(node.id)) {
            continue;
        }
        let built = build(index, &profile, &by_id, &values, &mut visited)?;
        if node
            .call_frame
            .get("functionName")
            .and_then(WireValue::as_string)
            .is_some_and(|name| name.equals_ascii("(root)"))
        {
            roots.extend(built.children);
        } else {
            roots.push(built);
        }
    }
    let total = roots.iter().fold(0.0, |sum, node| sum + node.value);
    if total <= 0.0 {
        return Ok(None);
    }
    let duration = max(0.0, profile.end_time - profile.start_time);
    let samples = profile.samples.as_ref().map_or(0, Vec::len);
    let interval = if samples > 0 {
        duration / samples as f64
    } else {
        0.0
    };
    let mut header = cat(&[
        &"V8 CPU profile: ".into(),
        &fixed(duration / 1e6, 2),
        &" s wall clock".into(),
    ]);
    if samples > 0 {
        append(
            &mut header,
            &cat(&[
                &format!(", {samples} samples (avg interval ").into(),
                &number(round(interval)),
                &" µs)".into(),
            ]),
        );
    }
    let mut ctx = RenderTreeContext {
        out: vec![
            header,
            cat(&[
                &"On-CPU total: ".into(),
                &fixed(total / 1e6, 2),
                &" s (".into(),
                &format_pct(total, duration),
                &" of wall clock). Values below are on-CPU milliseconds (idle time excluded)."
                    .into(),
            ]),
        ],
        total,
        min_value: max(3.0 * interval, total * 0.02),
        format_value: millis,
        value_width: 8.max(millis(total).len()),
    };
    ctx.out.push("".into());
    ctx.out.push("## Hot paths".into());
    let mut kept: Vec<_> = roots
        .into_iter()
        .filter(|node| node.value >= ctx.min_value)
        .collect();
    kept.sort_by(|a, b| {
        (b.value - a.value)
            .partial_cmp(&0.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if kept.is_empty() {
        ctx.out.push(cat(&[
            &"  (no call path above ".into(),
            &millis(ctx.min_value),
            &" ms on-CPU)".into(),
        ]));
    }
    for node in &mut kept {
        render_profile_node(node, 0, &mut ctx);
    }
    let mut totals: Vec<(WireString, f64)> = Vec::new();
    for node in &profile.nodes {
        let micros = *values.get(&key(node.id)).unwrap_or(&0.0);
        if micros <= 0.0 {
            continue;
        }
        let name = node
            .call_frame
            .get("functionName")
            .and_then(WireValue::as_string)
            .unwrap();
        if name.equals_ascii("(idle)") || name.equals_ascii("(root)") {
            continue;
        }
        let label = frame_label(&node.call_frame)?;
        if let Some((_, value)) = totals.iter_mut().find(|(name, _)| *name == label) {
            *value += micros;
        } else {
            totals.push((label, micros));
        }
    }
    totals.sort_by(|a, b| {
        (b.1 - a.1)
            .partial_cmp(&0.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if !totals.is_empty() {
        ctx.out.push("".into());
        ctx.out
            .push("## Top functions by self time (idle time excluded)".into());
        for (label, value) in totals.into_iter().take(20) {
            ctx.out.push(cat(&[
                &pad(&millis(value), ctx.value_width),
                &" ".into(),
                &pad(&format_pct(value, total), 6),
                &"  ".into(),
                &label,
            ]));
        }
    }
    ctx.out.push("".into());
    ctx.out.push(
        "[Summarized view of a V8 .cpuprofile. Use ':raw' to read the original JSON.]".into(),
    );
    Ok(Some(join(&ctx.out, "\n")))
}
