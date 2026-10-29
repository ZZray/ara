//! Progress_Ledger (requirement 37, module M-X-LEDGER): an append-only JSONL
//! file with one [`Entry`] per module status change. The current state of a
//! module is its latest entry ([`current`]).
//!
//! [`Ledger::append`] validates the whole existing file first (every line is
//! a valid entry and `seq` runs 1, 2, 3, ... without gaps), then writes one
//! new line in append mode, so every earlier byte stays where it was. The
//! ledger has a single writer; concurrent appenders are not coordinated.
//!
//! A `verified` entry is accepted only when its gate record passed, every
//! required review is complete, and all of them match the current delivery
//! code version (requirement 37.4). [`Ledger::revalidate`] demotes verified
//! modules whose delivery diff changed since (requirement 37.5).

use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    common::{parse_timestamp, timestamp_now, Credentials},
    digest::{worktree_diff, WorktreeDiff, WORKTREE_DIFF_RULE},
    records::{check_review_coverage, GateRecord, ReviewRecord},
};

/// Repository-relative location of the generated summary.
pub const LEDGER_SUMMARY_PATH: &str = "crates/ara-agent-lite-core/docs/evidence/lite-capability/progress-ledger.md";

/// Repository-relative location of the in-repo ledger (design DD-15).
pub const LEDGER_PATH: &str = "crates/ara-agent-lite-core/docs/evidence/lite-capability/progress-ledger.jsonl";

/// Written instead of an empty `uncovered`.
pub const NOTHING_UNCOVERED: &str = "无";

/// Phase of a module; [`Phase::Cross`] is the cross-cutting “横向” group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Phase {
    P0,
    P1,
    P2,
    P3,
    P4,
    P5,
    #[serde(rename = "横向")]
    Cross,
}

/// Module state (requirement 37.2): `open` while unimplemented or blocked,
/// `done` once implemented, `verified` once the delivery checks hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Open,
    Done,
    Verified,
}

/// One module of the design module table.
#[derive(Debug, PartialEq, Eq)]
pub struct Module {
    pub id: &'static str,
    pub phase: Phase,
    /// The requirement this module delivers.
    pub requirement: u8,
    /// Requirement title, used as the module name in notifications.
    pub title: &'static str,
    /// Acceptance criteria of the requirement: the module's progress denominator.
    pub criteria: u32,
}

const fn module_row(id: &'static str, phase: Phase, requirement: u8, title: &'static str, criteria: u32) -> Module {
    Module { id, phase, requirement, title, criteria }
}

/// The modules of the design, in requirement order. Criteria counts are
/// the numbered acceptance criteria of each requirement in requirements.md.
///
/// Rows 1–38 are the original module table. The rows after them were added
/// later (requirement 41.6 for H0/P2-f, spec tasks 67/68 for MCP) and carry
/// [`Phase::Cross`] so the overall progress keeps its original 36-checkpoint
/// basis ([`CHECKPOINTS`]). `M-MCP-CORE` counts all 9 criteria of
/// requirement 42; `M-MCP-JSONL` counts only criterion 42.9, the JSONL part.
pub const MODULES: [Module; 43] = [
    module_row("M-P0-TASKSET", Phase::P0, 1, "固定评估任务集", 7),
    module_row("M-P0-HOLDOUT", Phase::P0, 2, "新增密封 C++ holdout 任务", 8),
    module_row("M-P0-RUNNER", Phase::P0, 3, "一键评估运行器与固定运行策略", 8),
    module_row("M-P0-METRICS", Phase::P0, 4, "结果度量与未知值记录", 8),
    module_row("M-P0-FAILURE", Phase::P0, 5, "失败记录与分类", 7),
    module_row("M-P0-FORK", Phase::P0, 6, "同前缀 Session 分叉（可选）", 7),
    module_row("M-P0-BASELINE", Phase::P0, 7, "基线证据", 7),
    module_row("M-P1-RESUME-BASE", Phase::P1, 8, "已实现基线：Core 中断 Run 结算与 Harness resume", 8),
    module_row("M-P1-JSONL-START", Phase::P1, 9, "JSONL resume 启动帧", 4),
    module_row("M-P1-JSONL-RECONCILE", Phase::P1, 10, "JSONL 结算请求与回复帧", 6),
    module_row("M-P1-PY", Phase::P1, 11, "Python 客户端 reconcile_resume 回调", 5),
    module_row("M-P1-NODE", Phase::P1, 12, "Node 客户端对齐", 5),
    module_row("M-P1-DOCS", Phase::P1, 13, "resume 协议与集成文档", 6),
    module_row("M-P1-PROCESS", Phase::P1, 14, "真实进程中断与恢复", 6),
    module_row("M-P2A-OUTPUT", Phase::P2, 15, "输出上限核实与 Host 策略", 7),
    module_row("M-P2A-LENGTH", Phase::P2, 16, "任务级长度截断续写", 9),
    module_row("M-P2B-GATE", Phase::P2, 17, "完成门", 10),
    module_row("M-P2C-STUCK", Phase::P2, 18, "无进展检测", 10),
    module_row("M-P2D-REFRESH", Phase::P2, 19, "压缩后刷新", 9),
    module_row("M-P2E-EDIT", Phase::P2, 20, "能力测试 Host 使用 edit_file", 7),
    module_row("M-P2-COMPARE", Phase::P2, 21, "改动前后对比记录", 7),
    module_row("M-P3-UNKNOWN", Phase::P3, 22, "类型化未知副作用记录", 8),
    module_row("M-P3-JSONL-UNKNOWN", Phase::P3, 23, "JSONL Unknown 结算类型化", 7),
    module_row("M-P3-REPLAY", Phase::P3, 24, "重放前置条件", 7),
    module_row("M-P3-SUMMARY", Phase::P3, 25, "Harness 内压缩共享 Task 预算", 6),
    module_row("M-P3-BYTES", Phase::P3, 26, "NUL 与 Unicode 字节保留", 6),
    module_row("M-P4-DEPS", Phase::P4, 27, "职责边界与依赖方向", 6),
    module_row("M-P4-EVENTS", Phase::P4, 28, "版本化类型事件枚举", 7),
    module_row("M-P4-STABILITY", Phase::P4, 29, "公共接口稳定性规则", 5),
    module_row("M-P4-MATRIX", Phase::P4, 30, "Full/Lite 支持矩阵", 5),
    module_row("M-P4-CONSUMER", Phase::P4, 31, "树外消费者 crate", 7),
    module_row("M-P5-REVIEW", Phase::P5, 32, "模块独立审查", 5),
    module_row("M-P5-QUALITY", Phase::P5, 33, "交付代码质量门", 4),
    module_row("M-P5-DENY", Phase::P5, 34, "依赖审计现状记录", 5),
    module_row("M-P5-LONG", Phase::P5, 35, "长任务综合可靠性验收", 6),
    module_row("M-P5-STAGE", Phase::P5, 36, "证据保留与阶段验收记录", 7),
    module_row("M-X-LEDGER", Phase::Cross, 37, "进度台账", 5),
    module_row("M-X-NOTIFY", Phase::Cross, 38, "钉钉进度通知", 9),
    module_row("M-H0-HEADROOM", Phase::Cross, 39, "固定版本 Headroom Rust 可行性", 6),
    module_row("M-P2F-COMPRESS", Phase::Cross, 40, "Lite 模型投影压缩与原文回读", 10),
    module_row("M-P2F-COMPARE", Phase::Cross, 41, "独立 A/B 与交付", 6),
    module_row("M-MCP-CORE", Phase::Cross, 42, "Lite 的 MCP 工具来源", 9),
    module_row("M-MCP-JSONL", Phase::Cross, 42, "JSONL 暴露 MCP（42.9）", 1),
];

/// Checkpoints counted by the overall progress (`verified checkpoints / 36`):
/// every P0–P5 module, i.e. all modules except the two cross-cutting ones.
pub const CHECKPOINTS: usize = 36;

/// The module with `id`, if the design lists it.
pub fn module(id: &str) -> Option<&'static Module> {
    MODULES.iter().find(|module| module.id == id)
}

/// Code version under [`WORKTREE_DIFF_RULE`]: HEAD plus the SHA-256 of the
/// complete working-tree diff (`None` when the tree is clean).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeVersion {
    pub head: String,
    pub diff_sha256: Option<String>,
    pub diff_rule: String,
}

impl CodeVersion {
    /// Version of an already computed diff; the diff itself is not saved here.
    pub fn of(diff: &WorktreeDiff) -> Self {
        Self { head: diff.head.clone(), diff_sha256: diff.sha256.clone(), diff_rule: diff.rule.id.to_owned() }
    }

    /// Version of the repository containing `dir`.
    pub fn current(dir: &Path) -> Result<Self> {
        Ok(Self::of(&worktree_diff(dir)?))
    }
}

/// Acceptance criteria met so far out of the module's total.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Criteria {
    pub done: u32,
    pub total: u32,
}

/// One attempted action and what came of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub action: String,
    pub result: String,
}

/// Why a module cannot continue without a user decision, authorization or
/// external resource (requirement 37.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blocker {
    pub reason: String,
    pub attempts: Vec<Attempt>,
    pub decision_needed: String,
}

/// One ledger line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// 1 for the first line, then +1 per line.
    pub seq: u64,
    pub module: String,
    pub phase: Phase,
    pub state: State,
    /// ISO 8601, whole seconds, `±HH:MM` offset.
    pub timestamp: String,
    /// Evidence file or directory; existed when the entry was appended.
    pub evidence: String,
    /// Uncovered items, [`NOTHING_UNCOVERED`] when there are none.
    pub uncovered: String,
    pub code_version: CodeVersion,
    pub criteria: Criteria,
    pub gate_record: Option<String>,
    pub review_records: Vec<String>,
    pub blocker: Option<Blocker>,
}

/// What a caller supplies for a new entry; `seq`, `phase`, `timestamp` and
/// the criteria total are filled in by [`Ledger::append`].
#[derive(Clone, Debug)]
pub struct Append {
    pub module: String,
    pub state: State,
    pub evidence: String,
    /// Empty or blank becomes [`NOTHING_UNCOVERED`].
    pub uncovered: String,
    pub code_version: CodeVersion,
    pub criteria_done: u32,
    pub gate_record: Option<String>,
    pub review_records: Vec<String>,
    pub blocker: Option<Blocker>,
}

/// Current state of every module that has an entry: its latest entry.
pub fn current(entries: &[Entry]) -> BTreeMap<&str, &Entry> {
    // Entries are in seq order, so a later entry replaces an earlier one.
    entries.iter().map(|entry| (entry.module.as_str(), entry)).collect()
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Rules every entry satisfies, whether being appended or read back.
fn check(entry: &Entry) -> Result<()> {
    let module = module(&entry.module).with_context(|| format!("unknown module `{}`", entry.module))?;
    ensure!(entry.phase == module.phase, "module {} belongs to {:?}, not {:?}", module.id, module.phase, entry.phase);
    ensure!(
        entry.criteria.total == module.criteria && entry.criteria.done <= entry.criteria.total,
        "criteria {}/{} of module {} must be at most {} out of {}",
        entry.criteria.done,
        entry.criteria.total,
        module.id,
        module.criteria,
        module.criteria
    );
    parse_timestamp(&entry.timestamp)?;
    ensure!(!entry.evidence.trim().is_empty(), "evidence path is empty");
    ensure!(
        !entry.uncovered.trim().is_empty(),
        "uncovered is empty; write `{NOTHING_UNCOVERED}` when nothing is uncovered"
    );

    let version = &entry.code_version;
    ensure!(
        !version.head.is_empty() && version.head.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "code version head `{}` is not a commit id",
        version.head
    );
    ensure!(
        version.diff_sha256.as_deref().is_none_or(is_sha256_hex),
        "code version diff SHA-256 `{}` is not 64 lowercase hex digits",
        version.diff_sha256.as_deref().unwrap_or_default()
    );
    ensure!(
        version.diff_rule == WORKTREE_DIFF_RULE,
        "code version rule `{}` is not {WORKTREE_DIFF_RULE}",
        version.diff_rule
    );

    ensure!(entry.gate_record.as_deref().is_none_or(|path| !path.trim().is_empty()), "gate record path is empty");
    ensure!(entry.review_records.iter().all(|path| !path.trim().is_empty()), "a review record path is empty");

    if let Some(blocker) = &entry.blocker {
        ensure!(entry.state == State::Open, "a blocker can only be recorded on an open entry");
        ensure!(!blocker.reason.trim().is_empty(), "blocker reason is empty");
        ensure!(!blocker.attempts.is_empty(), "blocker lists no attempted action");
        ensure!(
            blocker
                .attempts
                .iter()
                .all(|attempt| !attempt.action.trim().is_empty() && !attempt.result.trim().is_empty()),
            "every attempted action needs both the action and its result"
        );
        ensure!(!blocker.decision_needed.trim().is_empty(), "blocker names no decision needed to continue");
    }
    Ok(())
}

/// Computes the current delivery code version of the ledger root.
pub type VersionSource = Arc<dyn Fn(&Path) -> Result<CodeVersion> + Send + Sync>;

/// A ledger file plus the directory its relative paths are resolved against.
#[derive(Clone)]
pub struct Ledger {
    path: PathBuf,
    root: PathBuf,
    version_source: VersionSource,
}

impl fmt::Debug for Ledger {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Ledger").field("path", &self.path).field("root", &self.root).finish_non_exhaustive()
    }
}

impl Ledger {
    /// Ledger at `path`; relative evidence and record paths resolve against
    /// `root`, whose git working tree gives the current delivery code version.
    pub fn new(path: impl Into<PathBuf>, root: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), root: root.into(), version_source: Arc::new(CodeVersion::current) }
    }

    /// Replace how the current delivery code version is computed (tests).
    pub fn with_version_source(mut self, source: VersionSource) -> Self {
        self.version_source = source;
        self
    }

    /// The in-repo ledger at [`LEDGER_PATH`], with repository-relative paths.
    pub fn in_repo(repo_root: &Path) -> Self {
        Self::new(repo_root.join(LEDGER_PATH), repo_root)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// All entries in seq order; a missing file is an empty ledger. Fails on
    /// any line that is not a valid entry, a seq gap, or a missing final newline.
    pub fn entries(&self) -> Result<Vec<Entry>> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).with_context(|| format!("cannot read ledger {}", self.path.display())),
        };
        let Some(body) = bytes.strip_suffix(b"\n") else {
            ensure!(bytes.is_empty(), "ledger {} ends with an incomplete line", self.path.display());
            return Ok(Vec::new());
        };
        body.split(|byte| *byte == b'\n')
            .zip(1u64..)
            .map(|(line, number)| {
                let entry: Entry = serde_json::from_slice(line)
                    .with_context(|| format!("ledger {} line {number} is not a valid entry", self.path.display()))?;
                ensure!(
                    entry.seq == number,
                    "ledger {} line {number} has seq {}; seq must run 1, 2, 3, ... without gaps",
                    self.path.display(),
                    entry.seq
                );
                check(&entry).with_context(|| format!("ledger {} line {number}", self.path.display()))?;
                Ok(entry)
            })
            .collect()
    }

    /// Validate `request` and the existing ledger, then append one entry with
    /// the next seq and the current timestamp. Nothing is written on failure.
    pub fn append(&self, request: Append, credentials: &Credentials) -> Result<Entry> {
        let module = module(&request.module).with_context(|| format!("unknown module `{}`", request.module))?;
        let uncovered =
            if request.uncovered.trim().is_empty() { NOTHING_UNCOVERED.to_owned() } else { request.uncovered };
        let mut entry = Entry {
            seq: 0,
            module: module.id.to_owned(),
            phase: module.phase,
            state: request.state,
            timestamp: timestamp_now(),
            evidence: request.evidence,
            uncovered,
            code_version: request.code_version,
            criteria: Criteria { done: request.criteria_done, total: module.criteria },
            gate_record: request.gate_record,
            review_records: request.review_records,
            blocker: request.blocker,
        };
        check(&entry)?;

        let references = std::iter::once(("evidence", &entry.evidence))
            .chain(entry.gate_record.iter().map(|path| ("gate record", path)))
            .chain(entry.review_records.iter().map(|path| ("review record", path)));
        for (label, path) in references {
            let resolved = self.root.join(path);
            let exists =
                resolved.try_exists().with_context(|| format!("cannot check {label} {}", resolved.display()))?;
            ensure!(exists, "{label} {} does not exist", resolved.display());
        }
        if entry.state == State::Verified {
            self.check_verified(&entry)
                .with_context(|| format!("module {} cannot be marked verified", entry.module))?;
        }

        entry.seq = self.entries()?.len() as u64 + 1;
        let mut line = serde_json::to_vec(&entry).context("cannot encode ledger entry")?;
        line.push(b'\n');
        if credentials.occurs_in(&line) {
            bail!("refusing to append to ledger {}: the entry contains a known credential value", self.path.display());
        }
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)
            .with_context(|| format!("cannot open ledger {} for appending", self.path.display()))?;
        file.write_all(&line).with_context(|| format!("cannot append to ledger {}", self.path.display()))?;
        file.sync_all().with_context(|| format!("cannot sync ledger {}", self.path.display()))?;
        Ok(entry)
    }

    /// Requirement 37.4: the gate passed, every required review is complete,
    /// and the entry, gate and reviews all match the current delivery version.
    fn check_verified(&self, entry: &Entry) -> Result<()> {
        let current = (self.version_source)(&self.root).context("cannot compute the current delivery code version")?;
        ensure!(
            entry.code_version == current,
            "entry code version (diff {}) is not the current delivery version (diff {})",
            short(&entry.code_version),
            short(&current)
        );
        let gate_path = entry.gate_record.as_deref().context("a verified entry needs a gate record")?;
        let gate = GateRecord::load(&self.root.join(gate_path))?;
        ensure!(gate.module == entry.module, "gate record belongs to {}, not {}", gate.module, entry.module);
        ensure!(
            gate.code_version == current,
            "gate record was run on diff {}, not the current delivery diff {}",
            short(&gate.code_version),
            short(&current)
        );
        gate.check_passes()?;

        let reviews = entry
            .review_records
            .iter()
            .map(|path| ReviewRecord::load(&self.root.join(path)))
            .collect::<Result<Vec<_>>>()?;
        for review in &reviews {
            review.check_complete()?;
            ensure!(
                review.diff_sha256 == current.diff_sha256,
                "{} review covers diff {}, not the current delivery diff {}",
                review.tool,
                review.diff_sha256.as_deref().map_or("clean", prefix),
                short(&current)
            );
        }
        check_review_coverage(&reviews)
    }

    /// Requirement 37.5: append a `done` entry for every module whose current
    /// state is `verified` but whose delivery code version has changed.
    /// Returns the appended entries.
    pub fn revalidate(&self, credentials: &Credentials) -> Result<Vec<Entry>> {
        let now = (self.version_source)(&self.root).context("cannot compute the current delivery code version")?;
        let entries = self.entries()?;
        let stale: Vec<Entry> = current(&entries)
            .into_values()
            .filter(|entry| entry.state == State::Verified && entry.code_version != now)
            .cloned()
            .collect();
        stale
            .into_iter()
            .map(|entry| {
                let uncovered = format!(
                    "交付代码版本已变化（diff {} → {}），需在当前版本重新验证",
                    short(&entry.code_version),
                    short(&now)
                );
                self.append(
                    Append {
                        module: entry.module,
                        state: State::Done,
                        evidence: entry.evidence,
                        uncovered,
                        code_version: now.clone(),
                        criteria_done: entry.criteria.done,
                        gate_record: entry.gate_record,
                        review_records: entry.review_records,
                        blocker: None,
                    },
                    credentials,
                )
            })
            .collect()
    }

    /// Markdown summary: one row per design module, then open blockers.
    pub fn render(&self) -> Result<String> {
        let entries = self.entries()?;
        let states = current(&entries);
        let count = |state: State| states.values().filter(|entry| entry.state == state).count();
        let verified_checkpoints =
            states.values().filter(|entry| entry.state == State::Verified && entry.phase != Phase::Cross).count();
        let cell = |text: &str| text.replace('|', "\\|").replace(['\r', '\n'], " ");
        let mut out = String::from("# 进度台账摘要\n\n");
        out.push_str(&format!(
            "由 `lite-eval ledger render` 从 `progress-ledger.jsonl` 生成，共 {} 条记录。\n\n",
            entries.len()
        ));
        out.push_str(&format!(
            "- 总进度：verified 检查点 {verified_checkpoints}/{CHECKPOINTS}\n- 模块状态：verified {}，done {}，open {}，未开始 {}\n\n",
            count(State::Verified),
            count(State::Done),
            count(State::Open),
            MODULES.len() - states.len()
        ));
        out.push_str(
            "| 模块 | 阶段 | 需求 | 状态 | 完成标准 | 更新时间 | 证据 | 未覆盖 |\n|---|---|---|---|---|---|---|---|\n",
        );
        for module in &MODULES {
            let phase = serde_json::to_value(module.phase)?;
            let phase = phase.as_str().unwrap_or_default();
            match states.get(module.id) {
                Some(entry) => out.push_str(&format!(
                    "| {} | {phase} | {} {} | {} | {}/{} | {} | {} | {} |\n",
                    module.id,
                    module.requirement,
                    cell(module.title),
                    serde_json::to_value(entry.state)?.as_str().unwrap_or_default(),
                    entry.criteria.done,
                    entry.criteria.total,
                    entry.timestamp,
                    cell(&entry.evidence),
                    cell(&entry.uncovered)
                )),
                None => out.push_str(&format!(
                    "| {} | {phase} | {} {} | 未开始 | 0/{} | — | — | — |\n",
                    module.id,
                    module.requirement,
                    cell(module.title),
                    module.criteria
                )),
            }
        }
        let blockers: Vec<_> =
            states.values().filter_map(|entry| entry.blocker.as_ref().map(|blocker| (entry, blocker))).collect();
        out.push_str("\n## 未解除的阻断\n\n");
        if blockers.is_empty() {
            out.push_str("无\n");
        }
        for (entry, blocker) in blockers {
            out.push_str(&format!(
                "- **{}**（seq {}）：{}；需要的决定：{}\n",
                entry.module,
                entry.seq,
                cell(&blocker.reason),
                cell(&blocker.decision_needed)
            ));
            for attempt in &blocker.attempts {
                out.push_str(&format!("  - 已尝试：{} → {}\n", cell(&attempt.action), cell(&attempt.result)));
            }
        }
        Ok(out)
    }

    /// Write [`Ledger::render`] to `path` (a derived view, replaced each time).
    pub fn render_to(&self, path: &Path, credentials: &Credentials) -> Result<()> {
        let text = self.render()?;
        ensure!(!credentials.occurs_in(text.as_bytes()), "ledger summary contains a known credential value");
        fs::write(path, text).with_context(|| format!("cannot write ledger summary {}", path.display()))
    }
}

/// First 8 hex digits of a code version's diff, or `clean`.
fn short(version: &CodeVersion) -> &str {
    version.diff_sha256.as_deref().map_or("clean", prefix)
}

/// First 8 characters; a malformed short value from a record file is shown whole.
fn prefix(sha: &str) -> &str {
    sha.get(..8).unwrap_or(sha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{common::sha256_hex, digest::DiffRule};

    const HEAD: &str = "56d55d209ecea00e80dfefecdd3e9d47fb62c47a";

    fn version() -> CodeVersion {
        CodeVersion {
            head: HEAD.to_owned(),
            diff_sha256: Some(sha256_hex(b"diff")),
            diff_rule: WORKTREE_DIFF_RULE.into(),
        }
    }

    /// Temp root with an evidence file `gates/g.json` and a ledger path in it.
    fn setup() -> (tempfile::TempDir, Ledger) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("gates")).unwrap();
        fs::write(dir.path().join("gates/g.json"), b"{}\n").unwrap();
        let ledger = Ledger::new(dir.path().join("progress-ledger.jsonl"), dir.path())
            .with_version_source(Arc::new(|_: &Path| Ok(version())));
        (dir, ledger)
    }

    /// Write a passing gate and the three required reviews for `module` at
    /// `code_version`; returns the gate path and review paths.
    fn accepted_records(root: &Path, module: &str, code_version: &CodeVersion) -> (String, Vec<String>) {
        use crate::records::{fixtures, GIT_REVIEW, INDEPENDENT_REVIEWER, RUST_CORE_REVIEW};
        let tag = short(code_version).to_owned();
        fs::create_dir_all(root.join("reviews")).unwrap();
        let gate = format!("gates/{module}-{tag}.json");
        fs::write(root.join(&gate), serde_json::to_vec(&fixtures::passing_gate(module, code_version)).unwrap())
            .unwrap();
        let reviews = [INDEPENDENT_REVIEWER, GIT_REVIEW, RUST_CORE_REVIEW]
            .map(|tool| {
                let path = format!("reviews/{module}-{tag}-{tool}.json");
                let record = fixtures::review(tool, code_version.diff_sha256.clone());
                fs::write(root.join(&path), serde_json::to_vec(&record).unwrap()).unwrap();
                path
            })
            .to_vec();
        (gate, reviews)
    }

    fn request(module: &str, state: State) -> Append {
        Append {
            module: module.to_owned(),
            state,
            evidence: "gates/g.json".to_owned(),
            uncovered: String::new(),
            code_version: version(),
            criteria_done: 0,
            gate_record: None,
            review_records: Vec::new(),
            blocker: None,
        }
    }

    fn blocker() -> Blocker {
        Blocker {
            reason: "model route unavailable".to_owned(),
            attempts: vec![Attempt { action: "probe CAS route".to_owned(), result: "HTTP 403".to_owned() }],
            decision_needed: "authorize the OpenRouter fallback".to_owned(),
        }
    }

    fn append(ledger: &Ledger, request: Append) -> Result<Entry> {
        ledger.append(request, &Credentials::default())
    }

    fn error(result: Result<Entry>) -> String {
        format!("{:#}", result.expect_err("append must be refused"))
    }

    #[test]
    fn module_table_follows_the_design() {
        assert_eq!(MODULES.len(), 43);
        let mut ids: Vec<_> = MODULES.iter().map(|module| module.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 43, "module ids must be unique");
        let requirements: Vec<u8> = (1..=42).chain([42]).collect();
        for (module, &requirement) in MODULES.iter().zip(&requirements) {
            assert_eq!(module.requirement, requirement, "{}", module.id);
            let phase = match requirement {
                1..=7 => Phase::P0,
                8..=14 => Phase::P1,
                15..=21 => Phase::P2,
                22..=26 => Phase::P3,
                27..=31 => Phase::P4,
                32..=36 => Phase::P5,
                _ => Phase::Cross,
            };
            assert_eq!(module.phase, phase, "{}", module.id);
            assert!(module.criteria > 0 && !module.title.is_empty(), "{}", module.id);
        }
        assert_eq!(MODULES.iter().filter(|module| module.phase != Phase::Cross).count(), CHECKPOINTS);
        assert_eq!(MODULES.iter().take(38).map(|module| module.criteria).sum::<u32>(), 256);
        assert_eq!(MODULES.iter().map(|module| module.criteria).sum::<u32>(), 256 + 6 + 10 + 6 + 9 + 1);
        assert_eq!(module("M-MCP-JSONL").map(|module| (module.requirement, module.criteria)), Some((42, 1)));
        assert_eq!(module("M-X-LEDGER").map(|module| (module.phase, module.criteria)), Some((Phase::Cross, 5)));
        assert_eq!(module("M-P2B-GATE").map(|module| (module.requirement, module.criteria)), Some((17, 10)));
        assert!(module("M-P9-NOPE").is_none());
    }

    #[test]
    fn appends_preserve_existing_bytes_and_number_entries_from_one() {
        let (_dir, ledger) = setup();
        assert!(ledger.entries().unwrap().is_empty(), "a missing ledger is empty");

        let first = append(&ledger, request("M-X-LEDGER", State::Open)).unwrap();
        assert_eq!((first.seq, first.phase, first.criteria), (1, Phase::Cross, Criteria { done: 0, total: 5 }));
        let before = fs::read(ledger.path()).unwrap();
        assert!(before.ends_with(b"\n") && before.iter().filter(|byte| **byte == b'\n').count() == 1);

        let second = append(&ledger, Append { criteria_done: 3, ..request("M-P0-TASKSET", State::Done) }).unwrap();
        assert_eq!((second.seq, second.phase, second.criteria), (2, Phase::P0, Criteria { done: 3, total: 7 }));
        let after = fs::read(ledger.path()).unwrap();
        assert!(after.starts_with(&before), "earlier bytes must stay in place");
        let tail: Entry = serde_json::from_slice(after[before.len()..].strip_suffix(b"\n").unwrap()).unwrap();
        assert_eq!(tail, second);
        assert_eq!(ledger.entries().unwrap(), [first, second]);

        let json: serde_json::Value = serde_json::from_slice(&before[..before.len() - 1]).unwrap();
        // Key order depends on serde_json features; compare the key set.
        let mut keys: Vec<_> = json.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut expected = [
            "seq",
            "module",
            "phase",
            "state",
            "timestamp",
            "evidence",
            "uncovered",
            "code_version",
            "criteria",
            "gate_record",
            "review_records",
            "blocker",
        ];
        expected.sort_unstable();
        assert_eq!(keys, expected);
        assert_eq!(json["phase"], "横向");
        assert_eq!(json["state"], "open");
        assert_eq!(json["blocker"], serde_json::Value::Null);
    }

    #[test]
    fn seq_gaps_and_damaged_lines_are_refused_without_writing() {
        let (_dir, ledger) = setup();
        append(&ledger, request("M-X-LEDGER", State::Open)).unwrap();
        append(&ledger, request("M-X-NOTIFY", State::Open)).unwrap();
        let good = fs::read_to_string(ledger.path()).unwrap();

        let damaged = [
            ("gap", good.replacen("\"seq\":2", "\"seq\":3", 1)),
            ("restart", good.replacen("\"seq\":1", "\"seq\":2", 1)),
            ("incomplete last line", good.trim_end_matches('\n').to_owned()),
            ("blank line", format!("{good}\n")),
            ("unknown field", good.replacen("\"seq\":1,", "\"seq\":1,\"extra\":true,", 1)),
            ("unknown module", good.replacen("M-X-NOTIFY", "M-X-OTHER", 1)),
        ];
        for (case, text) in damaged {
            assert_ne!(text, good, "{case}: test setup must damage the ledger");
            fs::write(ledger.path(), &text).unwrap();
            let error = format!("{:#}", ledger.entries().unwrap_err());
            assert!(error.contains("ledger"), "{case}: {error}");
            assert!(append(&ledger, request("M-P0-RUNNER", State::Open)).is_err(), "{case}");
            assert_eq!(fs::read_to_string(ledger.path()).unwrap(), text, "{case}: nothing may be written");
        }
    }

    #[test]
    fn timestamps_have_whole_seconds_and_an_offset() {
        let (_dir, ledger) = setup();
        let entry = append(&ledger, request("M-X-LEDGER", State::Open)).unwrap();
        assert_eq!(entry.timestamp.len(), "2026-10-27T09:15:02+08:00".len(), "{}", entry.timestamp);
        assert_eq!(crate::common::format_timestamp(&parse_timestamp(&entry.timestamp).unwrap()), entry.timestamp);

        let text = fs::read_to_string(ledger.path()).unwrap();
        let utc = text.replacen(&entry.timestamp, &format!("{}Z", &entry.timestamp[..19]), 1);
        fs::write(ledger.path(), utc).unwrap();
        assert!(format!("{:#}", ledger.entries().unwrap_err()).contains("ISO 8601"));
    }

    #[test]
    fn current_state_is_the_latest_entry_of_each_module() {
        let (_dir, ledger) = setup();
        append(&ledger, request("M-P0-TASKSET", State::Open)).unwrap();
        append(&ledger, request("M-X-LEDGER", State::Open)).unwrap();
        append(&ledger, Append { blocker: Some(blocker()), ..request("M-P0-TASKSET", State::Open) }).unwrap();
        append(&ledger, Append { criteria_done: 5, ..request("M-X-LEDGER", State::Done) }).unwrap();
        let latest = append(&ledger, Append { criteria_done: 7, ..request("M-P0-TASKSET", State::Done) }).unwrap();

        let entries = ledger.entries().unwrap();
        let states = current(&entries);
        assert_eq!(states.len(), 2);
        assert_eq!(states["M-P0-TASKSET"], &latest);
        assert_eq!((states["M-X-LEDGER"].seq, states["M-X-LEDGER"].state), (4, State::Done));
        assert!(!states.contains_key("M-X-NOTIFY"));
    }

    #[test]
    fn unknown_modules_and_unsupported_verified_entries_are_refused() {
        let (_dir, ledger) = setup();
        assert!(error(append(&ledger, request("M-P9-NOPE", State::Open))).contains("unknown module `M-P9-NOPE`"));
        assert!(error(append(&ledger, request("m-x-ledger", State::Open))).contains("unknown module"));
        assert!(error(append(&ledger, request("M-X-LEDGER", State::Verified))).contains("needs a gate record"));
        assert!(!ledger.path().exists(), "a refused append must not create the ledger");
    }

    #[test]
    fn verified_needs_a_passing_gate_and_complete_reviews_on_the_current_diff() {
        use crate::records::{fixtures, ReviewGap, RUST_CORE_REVIEW};
        let (dir, ledger) = setup();
        let (gate, reviews) = accepted_records(dir.path(), "M-X-LEDGER", &version());
        let verified = |gate: &str, reviews: &[String]| Append {
            gate_record: Some(gate.to_owned()),
            review_records: reviews.to_vec(),
            criteria_done: 5,
            ..request("M-X-LEDGER", State::Verified)
        };
        // Each missing piece is refused without writing.
        assert!(error(append(&ledger, verified(&gate, &reviews[..2]))).contains("ara-rust-core-review"));
        let mut failing = fixtures::passing_gate("M-X-LEDGER", &version());
        failing.commands[2].exit_code = Some(101);
        fs::write(dir.path().join("gates/failing.json"), serde_json::to_vec(&failing).unwrap()).unwrap();
        assert!(error(append(&ledger, verified("gates/failing.json", &reviews))).contains("exit code 101"));
        let mut other = fixtures::passing_gate("M-X-NOTIFY", &version());
        other.module = "M-X-NOTIFY".into();
        fs::write(dir.path().join("gates/other.json"), serde_json::to_vec(&other).unwrap()).unwrap();
        assert!(error(append(&ledger, verified("gates/other.json", &reviews))).contains("belongs to M-X-NOTIFY"));
        let mut gap = fixtures::review(RUST_CORE_REVIEW, version().diff_sha256);
        gap.gap = Some(ReviewGap { reason: "skill unavailable".into() });
        fs::write(dir.path().join("reviews/gap.json"), serde_json::to_vec(&gap).unwrap()).unwrap();
        let with_gap = [reviews[0].clone(), reviews[1].clone(), "reviews/gap.json".to_owned()];
        assert!(error(append(&ledger, verified(&gate, &with_gap))).contains("gap"));
        let stale = CodeVersion { diff_sha256: Some(sha256_hex(b"older")), ..version() };
        let (old_gate, old_reviews) = accepted_records(dir.path(), "M-X-LEDGER", &stale);
        assert!(error(append(&ledger, verified(&old_gate, &reviews))).contains("gate record was run on diff"));
        assert!(error(append(&ledger, verified(&gate, &old_reviews))).contains("review covers diff"));
        let behind = Append { code_version: stale, ..verified(&gate, &reviews) };
        assert!(error(append(&ledger, behind)).contains("not the current delivery version"));
        // A malformed short diff in a record file is an error, not a panic.
        let mut short_diff = fixtures::review(RUST_CORE_REVIEW, Some("abc".into()));
        short_diff.tool = RUST_CORE_REVIEW.into();
        fs::write(dir.path().join("reviews/short.json"), serde_json::to_vec(&short_diff).unwrap()).unwrap();
        let with_short = [reviews[0].clone(), reviews[1].clone(), "reviews/short.json".to_owned()];
        assert!(error(append(&ledger, verified(&gate, &with_short))).contains("covers diff abc"));
        assert!(!ledger.path().exists());

        let entry = append(&ledger, verified(&gate, &reviews)).unwrap();
        assert_eq!((entry.seq, entry.state), (1, State::Verified));
    }

    #[test]
    fn revalidate_demotes_verified_modules_whose_diff_changed() {
        let (dir, ledger) = setup();
        let (gate, reviews) = accepted_records(dir.path(), "M-X-LEDGER", &version());
        let verified = Append {
            gate_record: Some(gate),
            review_records: reviews,
            criteria_done: 5,
            ..request("M-X-LEDGER", State::Verified)
        };
        append(&ledger, verified.clone()).unwrap();
        append(&ledger, request("M-X-NOTIFY", State::Done)).unwrap();
        assert!(ledger.revalidate(&Credentials::default()).unwrap().is_empty(), "unchanged diff keeps verified");

        let changed = CodeVersion { diff_sha256: Some(sha256_hex(b"next module")), ..version() };
        let moved = changed.clone();
        let ledger = ledger.with_version_source(Arc::new(move |_: &Path| Ok(moved.clone())));
        let demoted = ledger.revalidate(&Credentials::default()).unwrap();
        assert_eq!(demoted.len(), 1);
        assert_eq!((demoted[0].module.as_str(), demoted[0].state, demoted[0].seq), ("M-X-LEDGER", State::Done, 3));
        assert_eq!(demoted[0].code_version, changed);
        assert!(demoted[0].uncovered.contains("重新验证"), "{}", demoted[0].uncovered);
        assert!(ledger.revalidate(&Credentials::default()).unwrap().is_empty(), "already demoted");
        // The old records no longer match, so verified stays refused.
        let again = Append { code_version: changed, ..verified };
        assert!(error(append(&ledger, again)).contains("gate record was run on diff"));
    }

    #[test]
    fn render_lists_every_module_and_open_blockers() {
        let (_dir, ledger) = setup();
        append(&ledger, Append { criteria_done: 2, ..request("M-P0-TASKSET", State::Done) }).unwrap();
        append(
            &ledger,
            Append { blocker: Some(blocker()), uncovered: "a|b".into(), ..request("M-P0-RUNNER", State::Open) },
        )
        .unwrap();
        let text = ledger.render().unwrap();
        assert_eq!(text.lines().filter(|line| line.starts_with("| M-")).count(), MODULES.len());
        assert!(text.contains("verified 检查点 0/36"), "{text}");
        assert!(text.contains("| M-P0-TASKSET | P0 | 1 固定评估任务集 | done | 2/7 |"), "{text}");
        assert!(text.contains("| M-X-NOTIFY | 横向 | 38 钉钉进度通知 | 未开始 | 0/9 |"), "{text}");
        assert!(text.contains("a\\|b"), "pipes are escaped: {text}");
        assert!(text.contains("**M-P0-RUNNER**（seq 2）：model route unavailable"), "{text}");
        assert!(text.contains("已尝试：probe CAS route → HTTP 403"), "{text}");

        let out = ledger.path().with_extension("md");
        ledger.render_to(&out, &Credentials::default()).unwrap();
        assert_eq!(fs::read_to_string(&out).unwrap(), text);
        let leak = Credentials::new(["model route".to_owned()]);
        assert!(ledger.render_to(&out, &leak).is_err());
    }

    #[test]
    fn evidence_and_records_must_exist() {
        let (dir, ledger) = setup();
        let missing = Append { evidence: "gates/missing.json".to_owned(), ..request("M-X-LEDGER", State::Open) };
        assert!(error(append(&ledger, missing)).contains("evidence"));
        let empty = Append { evidence: "  ".to_owned(), ..request("M-X-LEDGER", State::Open) };
        assert!(error(append(&ledger, empty)).contains("evidence path is empty"));
        let gate = Append { gate_record: Some("gates/none.json".to_owned()), ..request("M-X-LEDGER", State::Done) };
        assert!(error(append(&ledger, gate)).contains("gate record"));
        let review =
            Append { review_records: vec!["reviews/none.json".to_owned()], ..request("M-X-LEDGER", State::Done) };
        assert!(error(append(&ledger, review)).contains("review record"));
        assert!(!ledger.path().exists());

        // Relative paths resolve against the root; absolute paths and directories work too.
        let absolute = dir.path().join("gates/g.json").to_str().unwrap().to_owned();
        let directory = Append {
            evidence: "gates".to_owned(),
            gate_record: Some(absolute.clone()),
            review_records: vec!["gates/g.json".to_owned()],
            ..request("M-X-LEDGER", State::Done)
        };
        let entry = append(&ledger, directory).unwrap();
        assert_eq!((entry.evidence.as_str(), entry.gate_record.as_deref()), ("gates", Some(absolute.as_str())));
        append(&ledger, Append { evidence: absolute, ..request("M-X-NOTIFY", State::Open) }).unwrap();
        assert_eq!(ledger.entries().unwrap().len(), 2);
    }

    #[test]
    fn empty_uncovered_is_written_as_nothing() {
        let (_dir, ledger) = setup();
        for uncovered in ["", "  \n"] {
            let entry =
                append(&ledger, Append { uncovered: uncovered.to_owned(), ..request("M-X-LEDGER", State::Open) });
            assert_eq!(entry.unwrap().uncovered, "无");
        }
        let kept = "`resume_checked` 待实现";
        let entry = append(&ledger, Append { uncovered: kept.to_owned(), ..request("M-P1-RESUME-BASE", State::Done) });
        assert_eq!(entry.unwrap().uncovered, kept);
        assert!(ledger.entries().unwrap().iter().all(|entry| !entry.uncovered.is_empty()));
    }

    #[test]
    fn blockers_need_reason_attempts_with_results_and_decision() {
        let (_dir, ledger) = setup();
        let open = |blocker| Append { blocker: Some(blocker), ..request("M-P0-RUNNER", State::Open) };
        let entry = append(&ledger, open(blocker())).unwrap();
        assert_eq!(entry.blocker, Some(blocker()));

        let refused = [
            (Blocker { reason: " ".into(), ..blocker() }, "reason"),
            (Blocker { attempts: Vec::new(), ..blocker() }, "no attempted action"),
            (
                Blocker { attempts: vec![Attempt { action: "probe".into(), result: String::new() }], ..blocker() },
                "result",
            ),
            (
                Blocker { attempts: vec![Attempt { action: String::new(), result: "403".into() }], ..blocker() },
                "result",
            ),
            (Blocker { decision_needed: String::new(), ..blocker() }, "decision"),
        ];
        for (blocker, needle) in refused {
            assert!(error(append(&ledger, open(blocker))).contains(needle), "{needle}");
        }
        let on_done = Append { blocker: Some(blocker()), ..request("M-P0-RUNNER", State::Done) };
        assert!(error(append(&ledger, on_done)).contains("open entry"));
        assert_eq!(ledger.entries().unwrap().len(), 1);
    }

    #[test]
    fn criteria_and_code_version_are_checked() {
        let (_dir, ledger) = setup();
        let too_many = Append { criteria_done: 6, ..request("M-X-LEDGER", State::Done) };
        assert!(error(append(&ledger, too_many)).contains("criteria 6/5"));
        let bad_versions = [
            CodeVersion { head: String::new(), ..version() },
            CodeVersion { head: "not-a-commit".into(), ..version() },
            CodeVersion { diff_sha256: Some("ABC".into()), ..version() },
            CodeVersion { diff_sha256: Some(sha256_hex(b"x").to_uppercase()), ..version() },
            CodeVersion { diff_rule: "other-rule".into(), ..version() },
        ];
        for code_version in bad_versions {
            assert!(error(append(&ledger, Append { code_version, ..request("M-X-LEDGER", State::Open) }))
                .contains("code version"));
        }
        let clean = CodeVersion { diff_sha256: None, ..version() };
        let entry = append(&ledger, Append { code_version: clean.clone(), ..request("M-X-LEDGER", State::Open) });
        assert_eq!(entry.unwrap().code_version, clean);
    }

    #[test]
    fn code_version_comes_from_the_worktree_diff() {
        let diff = WorktreeDiff {
            repo_root: PathBuf::from("."),
            head: HEAD.to_owned(),
            dirty: true,
            sha256: Some(sha256_hex(b"bytes")),
            bytes: b"bytes".to_vec(),
            tracked_bytes: 5,
            untracked_files: Vec::new(),
            rule: DiffRule::current(),
        };
        assert_eq!(
            CodeVersion::of(&diff),
            CodeVersion {
                head: HEAD.to_owned(),
                diff_sha256: Some(sha256_hex(b"bytes")),
                diff_rule: "lite-eval-worktree-diff-v1".into()
            }
        );
        let clean = WorktreeDiff { dirty: false, sha256: None, bytes: Vec::new(), tracked_bytes: 0, ..diff };
        assert_eq!(CodeVersion::of(&clean).diff_sha256, None);
    }

    #[test]
    fn credentials_never_reach_the_ledger() {
        let (_dir, ledger) = setup();
        let secret = "sk-test-0123456789abcdef";
        let credentials = Credentials::new([secret.to_owned()]);
        let leaking = Append {
            blocker: Some(Blocker { reason: format!("401 for {secret}"), ..blocker() }),
            ..request("M-P0-RUNNER", State::Open)
        };
        let message = format!("{:#}", ledger.append(leaking, &credentials).unwrap_err());
        assert!(message.contains("known credential value") && !message.contains(secret), "{message}");
        assert!(!ledger.path().exists());
        ledger.append(request("M-P0-RUNNER", State::Open), &credentials).unwrap();
        assert!(!credentials.occurs_in(&fs::read(ledger.path()).unwrap()));
    }
}
