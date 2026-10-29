//! OMP b91db66e progress-hud.ts DownloadActivityHud and its row helpers.
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::{
    render_utils::{replace_tabs, sanitize, width},
    text::{self, Ellipsis},
    theme::Theme,
    tsp_backend::Node,
};
use crate::download_activity::{
    DownloadActivity, DownloadNotice, DownloadState, DownloadSubscription,
};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
const REVEAL: Duration = Duration::from_millis(150);
struct Row {
    activity: DownloadActivity,
    started: Instant,
    expires: Option<Instant>,
}
pub struct DownloadHud {
    rows: Vec<Row>,
    shown: Vec<u64>,
    theme: Arc<Theme>,
    cached: Option<Node>,
}
impl Default for DownloadHud {
    fn default() -> Self {
        Self::new(Arc::new(Theme::dark()))
    }
}
impl DownloadHud {
    pub fn new(theme: Arc<Theme>) -> Self {
        Self {
            rows: Vec::new(),
            shown: Vec::new(),
            theme,
            cached: None,
        }
    }
    fn update(&mut self, activity: DownloadActivity, now: Instant) {
        let index = self
            .rows
            .iter()
            .position(|row| row.activity.id == activity.id)
            .unwrap_or_else(|| {
                self.rows.push(Row {
                    activity: activity.clone(),
                    started: now,
                    expires: None,
                });
                self.rows.len() - 1
            });
        self.cached = None;
        let row = &mut self.rows[index];
        row.activity = activity;
        if row.activity.state != DownloadState::Running {
            if row.activity.state == DownloadState::Done
                && now.saturating_duration_since(row.started) < REVEAL
            {
                self.rows.remove(index);
                return;
            }
            row.expires = Some(
                now + Duration::from_millis(if row.activity.state == DownloadState::Failed {
                    10_000
                } else {
                    1_500
                }),
            );
        }
    }
    fn observe(&mut self, notice: DownloadNotice) {
        self.update(notice.activity, notice.emitted_at);
    }
    pub fn pump(&mut self, subscription: &DownloadSubscription, now: Instant) -> bool {
        let mut changed = false;
        for notice in subscription.drain() {
            self.observe(notice);
            changed = true;
        }
        changed |= self.advance(now);
        if changed {
            self.cached = None;
        }
        changed
    }
    fn advance(&mut self, now: Instant) -> bool {
        let before = self.rows.len();
        self.rows
            .retain(|row| row.expires.is_none_or(|end| now < end));
        let mut changed = before != self.rows.len();
        let shown = self
            .visible(now)
            .map(|row| row.activity.id)
            .collect::<Vec<_>>();
        changed |= shown != self.shown;
        self.shown = shown;
        if changed {
            self.cached = None;
        }
        changed
    }
    fn visible(&self, now: Instant) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(move |row| {
            row.activity.state != DownloadState::Running
                || now.saturating_duration_since(row.started) >= REVEAL
        })
    }
    pub fn render(&self, columns: usize, now: Instant) -> Vec<String> {
        if columns == 0 {
            return Vec::new();
        }
        self.visible(now)
            .map(|row| self.render_row(&row.activity, columns))
            .collect()
    }
    fn render_row(&self, activity: &DownloadActivity, columns: usize) -> String {
        let (label, tail, fallback, bar) = match activity.state {
            DownloadState::Failed => {
                let reason = format!(
                    "failed: {}",
                    status(activity.error.as_deref().unwrap_or("unknown error"))
                );
                (
                    activity.label.clone(),
                    self.theme
                        .fg("warning", &truncate(&reason, 8.max(columns * 6 / 10), true)),
                    self.theme.fg("warning", &reason),
                    None,
                )
            }
            DownloadState::Done => (
                activity.label.clone(),
                self.theme.fg("success", "ready"),
                self.theme
                    .fg("success", &format!("{} ready", activity.label)),
                None,
            ),
            DownloadState::Running => {
                let received = activity.loaded.unwrap_or(0);
                if let Some(total) = activity.total.filter(|total| *total > 0) {
                    let label = activity
                        .detail
                        .as_ref()
                        .filter(|s| !s.is_empty())
                        .map(|detail| format!("{} · {detail}", activity.label))
                        .unwrap_or_else(|| activity.label.clone());
                    (
                        label,
                        bold(
                            &self
                                .theme
                                .fg("text", &format!("{} / {}", bytes(received), bytes(total))),
                        ),
                        bold(&self.theme.fg(
                            "text",
                            &format!("{}%", ((received as f64 / total as f64) * 100.0).floor()),
                        )),
                        Some((received, total)),
                    )
                } else {
                    let step = if received > 0 {
                        bytes(received)
                    } else {
                        activity
                            .detail
                            .clone()
                            .unwrap_or_else(|| "downloading…".into())
                    };
                    (
                        activity.label.clone(),
                        self.theme.fg("text", &step),
                        self.theme.fg("text", &format!("{} {step}", activity.label)),
                        None,
                    )
                }
            }
        };
        hud_row(&self.theme, columns, &label, tail, &fallback, bar)
    }
    pub fn describe(&mut self, now: Instant) -> Node {
        if let Some(node) = &self.cached {
            return node.clone();
        }
        let rows=self.visible(now).map(|row|{
            let activity=&row.activity;let key=activity.id.to_string();
            let mut label=activity.label.clone();
            let (tail,tone,bar)=match activity.state {
                DownloadState::Failed=>(format!("failed: {}",status(activity.error.as_deref().unwrap_or("unknown error"))),"warning",None),
                DownloadState::Done=>("ready".into(),"success",None),
                DownloadState::Running=>{
                    let received=activity.loaded.unwrap_or(0);
                    if let Some(total)=activity.total.filter(|n|*n>0) {
                        if let Some(detail)=activity.detail.as_ref().filter(|s|!s.is_empty()) {label=format!("{label} · {detail}");}
                        (format!("{} / {}",bytes(received),bytes(total)),"strong",Some((received as f64/total as f64).clamp(0.0,1.0)))
                    }else{(if received>0 {bytes(received)}else{activity.detail.clone().unwrap_or_else(||"downloading…".into())},"text",None)}
                }
            };
            let mut children=vec![Node::new(format!("{key}-label"),"text",json!({"spans":[{"t":status(&label),"s":"dim"}],"wrap":"none","truncate":"end","shrink":1}),vec![])];
            if let Some(value)=bar {children.push(Node::new(format!("{key}-bar"),"progress",json!({"value":value,"max":{"w":"18ch"}}),vec![]));}
            children.push(Node::new(format!("{key}-tail"),"text",json!({"spans":[{"t":tail,"s":tone}],"wrap":"none"}),vec![]));
            Node::new(key,"row",json!({"justify":"end","gap":"sm","align":"center","role":"omp.hud.progress"}),children)
        }).collect();
        let node = Node::new(
            "download-hud",
            "col",
            json!({"role":"omp.hud.downloads"}),
            rows,
        );
        self.cached = Some(node.clone());
        node
    }
}
fn status(value: &str) -> String {
    replace_tabs(&sanitize(&value.into()))
        .replace(['\r', '\n'], " ")
        .split(' ')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}
fn bytes(value: u64) -> String {
    let n = value as f64;
    if value < 1024 {
        format!("{value}B")
    } else if value < 1024 * 1024 {
        format!("{:.1}KB", n / 1024.0)
    } else if value < 1024 * 1024 * 1024 {
        format!("{:.1}MB", n / (1024.0 * 1024.0))
    } else {
        format!("{:.1}GB", n / (1024.0 * 1024.0 * 1024.0))
    }
}
fn bold(value: &str) -> String {
    format!("\x1b[1m{value}\x1b[22m")
}
fn truncate(value: &str, columns: usize, ellipsis: bool) -> String {
    String::from_utf16_lossy(&text::truncate_to_width(
        &value.encode_utf16().collect::<Vec<_>>(),
        columns.min(u32::MAX as usize) as u32,
        Some(if ellipsis {
            Ellipsis::Unicode
        } else {
            Ellipsis::Omit
        }),
        None,
        3,
    ))
}
fn hud_row(
    theme: &Theme,
    columns: usize,
    label: &str,
    mut tail: String,
    fallback: &str,
    bar: Option<(u64, u64)>,
) -> String {
    let mut remaining = columns.saturating_sub(width(&tail));
    if let Some((value, max)) = bar.filter(|_| remaining > 1) {
        let cells = 18.min(remaining - 1);
        let filled =
            ((value as f64 / max.max(1) as f64).clamp(0.0, 1.0) * cells as f64).round() as usize;
        tail = format!(
            "{}{} {tail}",
            theme.fg("accent", &"━".repeat(filled)),
            theme.fg("dim", &"─".repeat(cells - filled))
        );
        remaining = columns.saturating_sub(width(&tail));
    }
    if remaining > 1 {
        let label = truncate(&status(label), remaining - 1, false);
        if !label.is_empty() {
            tail = format!("{} {tail}", theme.fg("dim", &label));
        }
    }
    if width(&tail) > columns {
        tail = truncate(fallback, columns, false);
    }
    format!("{}{tail}", " ".repeat(columns.saturating_sub(width(&tail))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn download_hud_source_reveal_retention_width_native_and_failures_bank() {
        let now = Instant::now();
        let mut hud = DownloadHud::default();
        let mut activity = DownloadActivity {
            id: 1,
            label: "INI grammar".into(),
            detail: Some("tree-sitter-ini.wasm".into()),
            loaded: None,
            total: None,
            state: DownloadState::Running,
            error: None,
        };
        hud.update(activity.clone(), now);
        assert!(hud.render(80, now).is_empty());
        let shown = now + REVEAL;
        assert_eq!(hud.render(80, shown).len(), 1);
        assert!(hud.render(80, shown)[0].contains("tree-sitter-ini.wasm"));
        activity.loaded = Some(1024);
        activity.total = Some(2048);
        hud.update(activity.clone(), shown);
        assert!(hud.render(80, shown)[0].contains("1.0KB / 2.0KB"));
        for columns in [0, 1, 4, 12, 40, 80] {
            assert!(
                hud.render(columns, shown)
                    .iter()
                    .all(|line| width(line) <= columns)
            );
        }
        let node = hud.describe(shown);
        assert_eq!(node.props["role"], "omp.hud.downloads");
        assert_eq!(node.children[0].children[1].kind, "progress");
        activity.state = DownloadState::Done;
        hud.update(activity.clone(), shown);
        assert!(hud.render(80, shown)[0].contains("ready"));
        hud.advance(shown + Duration::from_millis(1500));
        assert!(
            hud.render(80, shown + Duration::from_millis(1500))
                .is_empty()
        );
        activity.id = 2;
        activity.state = DownloadState::Running;
        hud.update(activity.clone(), now);
        activity.state = DownloadState::Done;
        hud.update(activity.clone(), now);
        assert!(hud.render(80, shown).is_empty());
        activity.id = 3;
        activity.state = DownloadState::Failed;
        activity.error = Some("bad\n\x1b[31mhash".into());
        hud.update(activity.clone(), now);
        assert!(hud.render(80, now)[0].contains("failed: bad hash"));
        hud.advance(now + Duration::from_millis(10000));
        assert!(
            hud.render(80, now + Duration::from_millis(10000))
                .is_empty()
        );
        let mut stalled = DownloadHud::default();
        let mut activity = DownloadActivity {
            id: 7,
            label: "slow grammar".into(),
            detail: None,
            loaded: None,
            total: None,
            state: DownloadState::Running,
            error: None,
        };
        stalled.observe(DownloadNotice {
            activity: activity.clone(),
            emitted_at: now,
        });
        activity.state = DownloadState::Done;
        stalled.observe(DownloadNotice {
            activity,
            emitted_at: now + Duration::from_millis(200),
        });
        // Both notices can be drained in one later paint; source duration is
        // still200ms, so ready must not be mistaken for an immediate success.
        stalled.advance(now + Duration::from_millis(1000));
        assert!(stalled.render(80, now + Duration::from_millis(1000))[0].contains("ready"));
        stalled.advance(now + Duration::from_millis(1700));
        assert!(
            stalled
                .render(80, now + Duration::from_millis(1700))
                .is_empty()
        );
    }
}
