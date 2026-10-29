//! Fixed OMP utils/profile-tree.ts. MIT; upstream notices in mod.rs.
use super::{append, cat, fixed, join, max, number, pad};
use ara_rpc::json::WireString;

#[derive(Clone, Debug, PartialEq)]
pub struct ProfileNode {
    pub key: WireString,
    pub label: WireString,
    pub value: f64,
    pub recursion: u64,
    pub children: Vec<ProfileNode>,
}
pub struct RenderTreeContext {
    pub out: Vec<WireString>,
    pub total: f64,
    pub min_value: f64,
    pub format_value: fn(f64) -> WireString,
    pub value_width: usize,
}
pub fn merge_into(a: &mut ProfileNode, b: ProfileNode) {
    a.value += b.value;
    a.recursion = a.recursion.max(b.recursion);
    for child in b.children {
        if let Some(existing) = a.children.iter_mut().find(|row| row.key == child.key) {
            merge_into(existing, child);
        } else {
            a.children.push(child);
        }
    }
}
pub fn flatten_recursion(node: &mut ProfileNode) {
    while node.children.iter().any(|child| child.key == node.key) {
        node.recursion += 1;
        let mut next: Vec<ProfileNode> = Vec::new();
        for child in std::mem::take(&mut node.children) {
            let promoted = if child.key == node.key {
                child.children
            } else {
                vec![child]
            };
            for item in promoted {
                if let Some(existing) = next.iter_mut().find(|row| row.key == item.key) {
                    merge_into(existing, item);
                } else {
                    next.push(item);
                }
            }
        }
        node.children = next;
    }
}
pub fn format_pct(value: f64, total: f64) -> WireString {
    if total <= 0.0 {
        "0%".into()
    } else {
        cat(&[&fixed(100.0 * value / total, 1), &"%".into()])
    }
}
fn decorated(node: &ProfileNode) -> WireString {
    let mut label = if node.label.len() > 160 {
        cat(&[&node.label.slice_prefix(159), &"…".into()])
    } else {
        node.label.clone()
    };
    if node.recursion > 0 {
        append(
            &mut label,
            &format!(" [recursive ×{}]", node.recursion + 1).into(),
        );
    }
    label
}
pub fn render_profile_node(node: &mut ProfileNode, indent: usize, ctx: &mut RenderTreeContext) {
    // Retain the original break predicate; own<min is not equivalent for NaN.
    let first_value = node.value;
    let mut labels = Vec::new();
    let mut current = node;
    loop {
        flatten_recursion(current);
        let kept: Vec<usize> = current
            .children
            .iter()
            .enumerate()
            .filter(|(_, child)| child.value >= ctx.min_value)
            .map(|(index, _)| index)
            .collect();
        if current.recursion > 0
            || kept.len() != 1
            || current.value - current.children[kept[0]].value >= ctx.min_value
        {
            break;
        }
        labels.push(decorated(current));
        current = &mut current.children[kept[0]];
    }
    flatten_recursion(current);
    labels.push(decorated(current));
    let path = if labels.len() <= 4 {
        join(&labels, " › ")
    } else {
        cat(&[
            &labels[0],
            &format!(" › ⋯{} frames⋯ › ", labels.len() - 2).into(),
            labels.last().unwrap(),
        ])
    };
    ctx.out.push(cat(&[
        &pad(&(ctx.format_value)(first_value), ctx.value_width),
        &" ".into(),
        &pad(&format_pct(first_value, ctx.total), 6),
        &"  ".into(),
        &"  ".repeat(indent).into(),
        &path,
    ]));
    let mut kept: Vec<usize> = current
        .children
        .iter()
        .enumerate()
        .filter(|(_, child)| child.value >= ctx.min_value)
        .map(|(index, _)| index)
        .collect();
    kept.sort_by(|&a, &b| {
        (current.children[b].value - current.children[a].value)
            .partial_cmp(&0.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for index in kept {
        render_profile_node(&mut current.children[index], indent + 1, ctx);
    }
}

pub(crate) fn millis(value: f64) -> WireString {
    fixed(value / 1000.0, 1)
}
pub(crate) fn count(value: f64) -> WireString {
    number(value)
}
pub(crate) fn minimum(a: f64, b: f64) -> f64 {
    max(a, b)
}
