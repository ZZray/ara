//! Stage_Acceptance_Record (requirement 36, module M-P5-STAGE): the four
//! stage acceptance goals split into sub-items, each listed as covered or
//! uncovered exactly once with its evidence and verification kind.

use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const STAGE_DIR: &str = "crates/ara-agent-lite-core/docs/evidence/lite-capability";
/// Claimed evidence per sub-item, maintained as modules land.
pub const CLAIMS_PATH: &str = "crates/ara-agent-lite-core/docs/evidence/lite-capability/stage-claims.json";

/// Contents of [`CLAIMS_PATH`]: claims keyed by sub-item name, plus the
/// successful Runs and Task verdicts known so far.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claims {
    #[serde(default)]
    pub items: Vec<ClaimedItem>,
    #[serde(default)]
    pub runs: Vec<RunEvidence>,
    #[serde(default)]
    pub tasks: Vec<TaskVerdict>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimedItem {
    pub name: String,
    pub claims: Vec<EvidenceClaim>,
}

/// Record for `phase` from the claims file (missing file: nothing claimed).
pub fn build_from_repo(root: &Path, phase: &str) -> Result<StageRecord> {
    let claims: Claims = match fs::read(root.join(CLAIMS_PATH)) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("invalid stage-claims.json")?,
        Err(_) => Claims::default(),
    };
    let mut items = sub_items();
    for claimed in claims.items {
        let item = items
            .iter_mut()
            .find(|item| item.name == claimed.name)
            .with_context(|| format!("stage-claims.json names an unknown sub-item `{}`", claimed.name))?;
        item.claims.extend(claimed.claims);
    }
    Ok(build(phase, items, &|path| evidence_exists(root, path), claims.runs, claims.tasks))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    ControlledTest,
    Demo,
    RealProcess,
    RealModelTrial,
}

impl Verification {
    /// Controlled tests and demos never complete a goal on their own (36.5).
    pub fn counts_for_acceptance(self) -> bool {
        matches!(self, Self::RealProcess | Self::RealModelTrial)
    }

    fn label(self) -> &'static str {
        match self {
            Self::ControlledTest => "受控测试",
            Self::Demo => "演示",
            Self::RealProcess => "真实进程",
            Self::RealModelTrial => "有界真实模型试验",
        }
    }
}

/// One claimed piece of evidence for a sub-item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceClaim {
    pub requirement: u8,
    pub path: String,
    pub verification: Verification,
}

/// Sub-item of a stage goal with whatever evidence has been claimed for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubItem {
    pub goal: u8,
    pub name: String,
    pub claims: Vec<EvidenceClaim>,
}

/// The four stage goals and their sub-items (requirements.md 阶段验收目标).
pub fn sub_items() -> Vec<SubItem> {
    let item = |goal: u8, name: &str| SubItem { goal, name: name.to_owned(), claims: Vec::new() };
    vec![
        item(1, "多文件 C++ 编码真实产物与独立检查"),
        item(1, "逻辑推理真实产物与独立检查"),
        item(1, "逆向分析真实产物与独立检查"),
        item(1, "同模型同任务同预算前后对比"),
        item(2, "压缩"),
        item(2, "多次续写"),
        item(2, "长度截断"),
        item(2, "进程中断"),
        item(2, "回执核对"),
        item(3, "职责边界与依赖方向"),
        item(3, "稳定接口与树外消费者"),
        item(4, "回归测试"),
        item(4, "独立审查"),
        item(4, "依赖门禁"),
        item(4, "失败记录与未覆盖项保留"),
    ]
}

/// A Run that ended successfully: evidence awaiting review (36.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEvidence {
    pub run_id: String,
    pub task_id: String,
}

/// A Task and the Host's acceptance conclusion, if any (36.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskVerdict {
    pub task_id: String,
    pub host_accepted: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoveredItem {
    pub goal: u8,
    pub name: String,
    pub evidence: Vec<EvidenceClaim>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UncoveredItem {
    pub goal: u8,
    pub name: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalStatus {
    pub goal: u8,
    /// False when any sub-item is uncovered or all its evidence is controlled/demo.
    pub overall_accepted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageRecord {
    pub phase: String,
    pub covered: Vec<CoveredItem>,
    pub uncovered: Vec<UncoveredItem>,
    pub goals: Vec<GoalStatus>,
    pub pending_review_runs: Vec<RunEvidence>,
    pub tasks: Vec<TaskVerdict>,
}

/// Property 43: each sub-item lands in exactly one list; evidence that does
/// not exist or is empty moves it to uncovered; goals with uncovered items or
/// only controlled/demo evidence are not accepted overall.
pub fn build(
    phase: &str,
    items: Vec<SubItem>,
    evidence_ok: &dyn Fn(&str) -> bool,
    runs: Vec<RunEvidence>,
    tasks: Vec<TaskVerdict>,
) -> StageRecord {
    let mut covered = Vec::new();
    let mut uncovered = Vec::new();
    for item in items {
        let (good, missing): (Vec<_>, Vec<_>) = item.claims.into_iter().partition(|claim| evidence_ok(&claim.path));
        if !good.is_empty() {
            covered.push(CoveredItem { goal: item.goal, name: item.name, evidence: good });
        } else if !missing.is_empty() {
            let paths: Vec<_> = missing.iter().map(|claim| claim.path.as_str()).collect();
            uncovered.push(UncoveredItem {
                goal: item.goal,
                name: item.name,
                reason: format!("证据缺失：{}", paths.join("、")),
            });
        } else {
            uncovered.push(UncoveredItem { goal: item.goal, name: item.name, reason: "尚无证据".into() });
        }
    }
    let mut goal_ids: Vec<u8> =
        covered.iter().map(|item| item.goal).chain(uncovered.iter().map(|item| item.goal)).collect();
    goal_ids.sort_unstable();
    goal_ids.dedup();
    let goals = goal_ids
        .into_iter()
        .map(|goal| {
            let any_uncovered = uncovered.iter().any(|item| item.goal == goal);
            let real = covered
                .iter()
                .filter(|item| item.goal == goal)
                .flat_map(|item| &item.evidence)
                .any(|claim| claim.verification.counts_for_acceptance());
            GoalStatus { goal, overall_accepted: !any_uncovered && real }
        })
        .collect();
    StageRecord { phase: phase.to_owned(), covered, uncovered, goals, pending_review_runs: runs, tasks }
}

/// Evidence exists and is not empty (a directory must contain something).
pub fn evidence_exists(root: &Path, path: &str) -> bool {
    let path = root.join(path);
    match fs::metadata(&path) {
        Ok(meta) if meta.is_file() => meta.len() > 0,
        Ok(meta) if meta.is_dir() => fs::read_dir(&path).map(|mut dir| dir.next().is_some()).unwrap_or(false),
        _ => false,
    }
}

/// Markdown view of a record.
pub fn render(record: &StageRecord) -> String {
    let mut out = format!("# 阶段验收记录：{}\n\n## 阶段验收目标\n\n", record.phase);
    for goal in &record.goals {
        out.push_str(&format!(
            "- 目标 {}：{}\n",
            goal.goal,
            if goal.overall_accepted { "已完成整体验收" } else { "未完成整体验收" }
        ));
    }
    out.push_str("\n## 已覆盖\n\n| 目标 | 子项 | 需求 | 证据 | 验证方式 |\n|---|---|---|---|---|\n");
    for item in &record.covered {
        for claim in &item.evidence {
            out.push_str(&format!(
                "| {} | {} | {} | `{}` | {} |\n",
                item.goal,
                item.name,
                claim.requirement,
                claim.path,
                claim.verification.label()
            ));
        }
    }
    out.push_str("\n## 未覆盖\n\n| 目标 | 子项 | 原因 |\n|---|---|---|\n");
    for item in &record.uncovered {
        out.push_str(&format!("| {} | {} | {} |\n", item.goal, item.name, item.reason));
    }
    out.push_str("\n## 待审查的成功 Run\n\n");
    if record.pending_review_runs.is_empty() {
        out.push_str("无\n");
    }
    for run in &record.pending_review_runs {
        out.push_str(&format!("- Run `{}`（Task `{}`）：待审查证据\n", run.run_id, run.task_id));
    }
    out.push_str("\n## Task 验收\n\n");
    if record.tasks.is_empty() {
        out.push_str("无\n");
    }
    for task in &record.tasks {
        let state = match task.host_accepted {
            Some(true) => "Host 已验收",
            Some(false) => "Host 未通过",
            None => "未验收（无 Host 结论）",
        };
        out.push_str(&format!("- Task `{}`：{state}\n", task.task_id));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_items_cover_the_four_goals_and_split_goal_two() {
        let items = sub_items();
        let goals: std::collections::BTreeSet<_> = items.iter().map(|item| item.goal).collect();
        assert_eq!(goals.into_iter().collect::<Vec<_>>(), [1, 2, 3, 4]);
        let two: Vec<_> = items.iter().filter(|item| item.goal == 2).map(|item| item.name.as_str()).collect();
        assert_eq!(two, ["压缩", "多次续写", "长度截断", "进程中断", "回执核对"]);
    }
}
