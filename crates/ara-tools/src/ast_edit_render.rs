//! Typed renderer contract for fixed OMP 596f2da astEditToolRenderer (MIT).
//! Theme/OSC8/width are Host rendering responsibilities. These values preserve
//! the fixed state, independent display text, whole-output header context and
//! group budget, so a Host never parses the model-only proposal notice.

use ara_agent::ToolOutput;
use ara_ai::UserBlock;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AstRenderState {
    Pending,
    Success,
    Warning,
    Error,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AstLineStyle {
    Directory,
    RootFile,
    NestedFile,
    Added,
    Removed,
    Output,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AstRenderLine {
    pub text: String,
    pub style: AstLineStyle,
    pub hyperlink: Option<PathBuf>,
}
#[derive(Clone, Debug)]
pub struct AstEditRenderContract {
    pub title: &'static str,
    pub icon: AstRenderState,
    pub frame_state: AstRenderState,
    pub proposed_badge: bool,
    pub description: Option<String>,
    pub meta: Vec<String>,
    pub groups: Vec<Vec<AstRenderLine>>,
    pub extra: Vec<String>,
    pub inline: bool,
    pub merge_call_and_result: bool,
}
fn n(data: &Value, key: &str) -> usize {
    data[key].as_u64().unwrap_or_default() as usize
}
fn text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .find_map(|c| match c {
            UserBlock::Text(content) => Some(content.text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}
pub fn pattern_preview(pattern: &str) -> Option<String> {
    let text = pattern.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}
pub fn call(ops: &[(String, String)], paths: &[String]) -> AstEditRenderContract {
    let mut meta = Vec::new();
    if !paths.is_empty() {
        meta.push(format!("in {}", paths.join(", ")));
    }
    if ops.len() > 1 {
        meta.push(format!("{} rewrites", ops.len()));
    }
    let description = if ops.len() == 1 {
        pattern_preview(&ops[0].0)
    } else {
        Some(if ops.is_empty() { "?".into() } else { format!("{} rewrites", ops.len()) })
    };
    AstEditRenderContract {
        title: "AST Edit",
        icon: AstRenderState::Pending,
        frame_state: AstRenderState::Pending,
        proposed_badge: false,
        description,
        meta,
        groups: Vec::new(),
        extra: Vec::new(),
        inline: true,
        merge_call_and_result: true,
    }
}
pub fn result(output: &ToolOutput, ops: &[(String, String)], partial: bool) -> AstEditRenderContract {
    let empty = Value::Null;
    let data = output.details.as_ref().unwrap_or(&empty);
    let mut render = call(ops, &[]);
    render.meta.clear();
    render.description = if ops.len() == 1 { pattern_preview(&ops[0].0) } else { None };
    if output.is_error {
        render.icon = AstRenderState::Error;
        render.frame_state = AstRenderState::Error;
        render.groups = vec![vec![AstRenderLine { text: text(output), style: AstLineStyle::Output, hyperlink: None }]];
        return render;
    }
    let replacements = n(data, "totalReplacements");
    let files = n(data, "filesTouched");
    let searched = n(data, "filesSearched");
    render.meta.push(format!("{replacements} replacement{}", if replacements == 1 { "" } else { "s" }));
    if replacements > 0 {
        render.meta.push(format!("{files} file{}", if files == 1 { "" } else { "s" }));
    }
    if let Some(scope) = data["scopePath"].as_str() {
        render.meta.push(format!("in {scope}"));
    }
    if replacements > 0 || searched > 0 {
        render.meta.push(format!("searched {searched}"));
    }
    let errors = data["parseErrors"].as_array().cloned().unwrap_or_default();
    let total = data["parseErrorsTotal"].as_u64().map_or(errors.len(), |n| n as usize);
    if replacements == 0 {
        render.icon = AstRenderState::Warning;
        render.frame_state = AstRenderState::Warning;
        let mut body: Vec<_> = errors
            .iter()
            .take(20)
            .filter_map(Value::as_str)
            .map(|s| AstRenderLine { text: format!("  - {s}"), style: AstLineStyle::Output, hyperlink: None })
            .collect();
        if total > body.len() {
            body.push(AstRenderLine {
                text: format!("  … {} more", total - body.len()),
                style: AstLineStyle::Output,
                hyperlink: None,
            });
        }
        if !body.is_empty() {
            render.groups.push(body);
        }
        return render;
    }
    let limit = data["limitReached"].as_bool() == Some(true);
    render.icon = if limit { AstRenderState::Warning } else { AstRenderState::Success };
    render.frame_state = if partial { AstRenderState::Pending } else { AstRenderState::Success };
    render.proposed_badge = true;
    if limit {
        render.meta.push("limit reached".into());
        render.extra.push("limit reached; narrow path".into());
    }
    if !errors.is_empty() {
        render.extra.push(if total > 20 {
            format!("20 / {total} parse issues")
        } else {
            format!("{total} parse issue{}", if total == 1 { "" } else { "s" })
        });
    }
    let content = data["displayContent"].as_str().map(str::to_owned).unwrap_or_else(|| text(output));
    let base = data["cwd"].as_str().or_else(|| data["searchPath"].as_str()).map(PathBuf::from);
    let mut dirs: Vec<(usize, PathBuf)> = Vec::new();
    let mut group = Vec::new();
    for raw in content.split('\n') {
        if raw.trim().is_empty() {
            if !group.is_empty() {
                render.groups.push(std::mem::take(&mut group));
            }
            continue;
        }
        if group.is_empty() && (raw.starts_with("Safety cap reached") || raw.starts_with("Parse issues:")) {
            continue;
        }
        let trimmed = raw.trim_start();
        let depth = trimmed.chars().take_while(|c| *c == '#').count();
        let (style, hyperlink) = if depth > 0 && trimmed.as_bytes().get(depth) == Some(&b' ') {
            let name = &trimmed[depth + 1..];
            let is_dir = name.ends_with('/');
            let name = name.split(" (").next().unwrap_or(name).split('#').next().unwrap_or(name).trim_end_matches('/');
            dirs.retain(|(level, _)| *level < depth);
            let parent = dirs.last().map(|(_, p)| p.clone()).or_else(|| base.clone());
            let path = parent.map(|p| p.join(name));
            if is_dir {
                if let Some(path) = &path {
                    dirs.push((depth, path.clone()));
                }
            }
            (
                if is_dir {
                    AstLineStyle::Directory
                } else if depth == 1 {
                    AstLineStyle::RootFile
                } else {
                    AstLineStyle::NestedFile
                },
                path,
            )
        } else {
            (
                if raw.starts_with('+') {
                    AstLineStyle::Added
                } else if raw.starts_with('-') {
                    AstLineStyle::Removed
                } else {
                    AstLineStyle::Output
                },
                None,
            )
        };
        group.push(AstRenderLine { text: raw.replacen('│', " ", 1), style, hyperlink });
    }
    if !group.is_empty() {
        render.groups.push(group);
    }
    render
}
impl AstEditRenderContract {
    /// The fixed collapsed six-line budget always keeps the first full group.
    /// The Host truncates styled lines to its frame's inner width afterwards.
    pub fn body(&self, expanded: bool) -> Vec<AstRenderLine> {
        let mut lines = Vec::new();
        let mut shown = 0;
        for (i, group) in self.groups.iter().enumerate() {
            let sep = usize::from(shown > 0);
            let reserved = usize::from(!expanded && i + 1 < self.groups.len());
            if !expanded && shown > 0 && lines.len() + sep + group.len() + reserved > 6 {
                break;
            }
            if sep > 0 {
                lines.push(AstRenderLine { text: String::new(), style: AstLineStyle::Output, hyperlink: None });
            }
            lines.extend(group.iter().cloned());
            shown += 1;
        }
        if !expanded && shown < self.groups.len() {
            let n = self.groups.len() - shown;
            lines.push(AstRenderLine {
                text: format!("… {n} more change{}", if n == 1 { "" } else { "s" }),
                style: AstLineStyle::Output,
                hyperlink: None,
            });
        }
        lines.extend(self.extra.iter().map(|s| AstRenderLine {
            text: s.clone(),
            style: AstLineStyle::Output,
            hyperlink: None,
        }));
        lines
    }
}
