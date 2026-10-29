//! Gate and review records (requirements 32 and 33) in the form the ledger's
//! `verified` check reads them. `lite-eval gate` and `lite-eval review-record`
//! (tasks 6.1 and 6.2) write these files; this module owns the format and the
//! rules for when a record counts as passed or complete.

use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{common::parse_timestamp, ledger::CodeVersion};

/// One delivery-gate command (design table “交付代码质量门”).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateCommandId {
    pub crate_name: &'static str,
    pub command: &'static str,
    /// Test runners report pass and fail counts; fmt and clippy do not.
    pub counts_tests: bool,
}

const fn gate_command(crate_name: &'static str, command: &'static str, counts_tests: bool) -> GateCommandId {
    GateCommandId { crate_name, command, counts_tests }
}

pub const FMT: &str = "cargo fmt --check";
pub const CLIPPY: &str = "cargo clippy --all-targets -- -D warnings";
pub const CARGO_TEST: &str = "cargo test";
pub const PYTHON_TESTS: &str = "python -m unittest discover -s tests -p \"test_*.py\"";
pub const NODE_TESTS: &str = "node --test";

/// The 11 commands of requirement 33.1, in execution order.
pub const GATE_COMMANDS: [GateCommandId; 11] = [
    gate_command("ara-agent-lite-core", FMT, false),
    gate_command("ara-agent-lite-core", CLIPPY, false),
    gate_command("ara-agent-lite-core", CARGO_TEST, true),
    gate_command("ara-agent-lite-harness", FMT, false),
    gate_command("ara-agent-lite-harness", CLIPPY, false),
    gate_command("ara-agent-lite-harness", CARGO_TEST, true),
    gate_command("ara-agent-lite-jsonl", FMT, false),
    gate_command("ara-agent-lite-jsonl", CLIPPY, false),
    gate_command("ara-agent-lite-jsonl", CARGO_TEST, true),
    gate_command("ara-agent-lite-jsonl", PYTHON_TESTS, true),
    gate_command("ara-agent-lite-jsonl/clients/node", NODE_TESTS, true),
];

/// Label written instead of a number (requirement 33.2: never 0 for these).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CountLabel {
    /// fmt and clippy run no tests.
    #[serde(rename = "不适用")]
    NotApplicable,
    /// A test runner whose summary could not be parsed; the command failed.
    #[serde(rename = "未解析")]
    Unparsed,
}

/// A pass or fail count, or the reason there is none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Count {
    Value(u64),
    Label(CountLabel),
}

/// Result of one gate command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateCommand {
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub command: String,
    /// `None`: the command never produced an exit code (missing toolchain, killed).
    pub exit_code: Option<i32>,
    pub passed: Count,
    pub failed: Count,
    pub started_at: String,
    pub finished_at: String,
    /// Saved complete output of a failed command (requirement 33.3).
    pub stdout_path: Option<String>,
    pub stderr_path: Option<String>,
}

/// One `lite-eval gate` run for a module.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateRecord {
    pub module: String,
    pub code_version: CodeVersion,
    pub commands: Vec<GateCommand>,
}

impl GateCommand {
    /// Why this command does not count as passed, if it does not.
    fn failure(&self, id: &GateCommandId) -> Option<String> {
        if self.exit_code != Some(0) {
            return Some(match self.exit_code {
                Some(code) => format!("exit code {code}"),
                None => "no exit code".to_owned(),
            });
        }
        match (id.counts_tests, self.passed, self.failed) {
            (true, Count::Value(_), Count::Value(0)) => None,
            (true, Count::Value(_), Count::Value(failed)) => Some(format!("{failed} failed tests")),
            (true, _, _) => Some("test counts missing or unparsed".to_owned()),
            (false, Count::Label(CountLabel::NotApplicable), Count::Label(CountLabel::NotApplicable)) => None,
            (false, _, _) => Some("fmt/clippy counts must be 不适用".to_owned()),
        }
    }
}

impl GateRecord {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("cannot read gate record {}", path.display()))?;
        let record: Self =
            serde_json::from_slice(&bytes).with_context(|| format!("invalid gate record {}", path.display()))?;
        for command in &record.commands {
            parse_timestamp(&command.started_at)?;
            parse_timestamp(&command.finished_at)?;
        }
        Ok(record)
    }

    /// The gate passed: each of the 11 commands appears exactly once, exit 0,
    /// no failed tests and parsed counts where the command runs tests.
    pub fn check_passes(&self) -> Result<()> {
        let mut seen = BTreeMap::new();
        for command in &self.commands {
            let key = (command.crate_name.as_str(), command.command.as_str());
            ensure!(
                GATE_COMMANDS.iter().any(|id| (id.crate_name, id.command) == key),
                "gate record has an unexpected command `{}` in {}",
                command.command,
                command.crate_name
            );
            ensure!(
                seen.insert(key, command).is_none(),
                "gate record repeats `{}` in {}",
                command.command,
                command.crate_name
            );
        }
        let problems: Vec<String> = GATE_COMMANDS
            .iter()
            .filter_map(|id| match seen.get(&(id.crate_name, id.command)) {
                None => Some(format!("{} `{}`: missing", id.crate_name, id.command)),
                Some(command) => command.failure(id).map(|why| format!("{} `{}`: {why}", id.crate_name, id.command)),
            })
            .collect();
        if !problems.is_empty() {
            bail!("gate for {} did not pass: {}", self.module, problems.join("; "));
        }
        Ok(())
    }
}

/// Repository-relative directory of review records.
pub const REVIEWS_DIR: &str = "crates/ara-agent-lite-core/docs/evidence/lite-capability/reviews";

/// Independent reviewer run outside the implementation session (requirement 32.1).
pub const INDEPENDENT_REVIEWER: &str = "codex";
pub const GIT_REVIEW: &str = "ara-git-review";
pub const RUST_CORE_REVIEW: &str = "ara-rust-core-review";
pub const PROVIDER_REVIEW: &str = "ara-provider-review";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewScope {
    pub base: String,
    pub target: String,
    pub files: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Fixed,
    FalsePositive,
    Deferred,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub location: String,
    pub description: String,
    pub disposition: Disposition,
    /// Required for `deferred`.
    pub reason: Option<String>,
}

/// The review did not happen or produced nothing (requirement 32.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewGap {
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRecord {
    pub tool: String,
    pub scope: ReviewScope,
    /// Diff SHA-256 of the reviewed code; `None` for a clean tree.
    pub diff_sha256: Option<String>,
    pub session_id: Option<String>,
    pub implementation_session_id: Option<String>,
    pub findings: Vec<Finding>,
    pub gap: Option<ReviewGap>,
    pub started_at: String,
    pub finished_at: String,
}

impl ReviewRecord {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("cannot read review record {}", path.display()))?;
        let record: Self =
            serde_json::from_slice(&bytes).with_context(|| format!("invalid review record {}", path.display()))?;
        parse_timestamp(&record.started_at)?;
        parse_timestamp(&record.finished_at)?;
        Ok(record)
    }

    /// The review ran, every finding has a disposition (deferred ones a
    /// reason), and the independent reviewer names two different sessions.
    pub fn check_complete(&self) -> Result<()> {
        if let Some(gap) = &self.gap {
            bail!("{} review is a recorded gap: {}", self.tool, gap.reason);
        }
        for finding in &self.findings {
            ensure!(
                !finding.location.trim().is_empty() && !finding.description.trim().is_empty(),
                "{} review has a finding without location or description",
                self.tool
            );
            ensure!(
                finding.disposition != Disposition::Deferred
                    || finding.reason.as_deref().is_some_and(|reason| !reason.trim().is_empty()),
                "{} review defers {} without a reason",
                self.tool,
                finding.location
            );
        }
        if self.tool == INDEPENDENT_REVIEWER {
            let session = self.session_id.as_deref().filter(|id| !id.trim().is_empty());
            let implementation = self.implementation_session_id.as_deref().filter(|id| !id.trim().is_empty());
            ensure!(
                session.is_some() && implementation.is_some() && session != implementation,
                "independent review must name its own session and a different implementation session"
            );
        }
        Ok(())
    }
}

/// Reviews required for `verified`: the independent reviewer, the git review
/// and a Rust review (provider-adapter changes use the provider review).
pub fn check_review_coverage(records: &[ReviewRecord]) -> Result<()> {
    let has = |tool: &str| records.iter().any(|record| record.tool == tool);
    let mut missing = Vec::new();
    if !has(INDEPENDENT_REVIEWER) {
        missing.push(INDEPENDENT_REVIEWER);
    }
    if !has(GIT_REVIEW) {
        missing.push(GIT_REVIEW);
    }
    if !has(RUST_CORE_REVIEW) && !has(PROVIDER_REVIEW) {
        missing.push("ara-rust-core-review or ara-provider-review");
    }
    ensure!(missing.is_empty(), "missing required reviews: {}", missing.join(", "));
    Ok(())
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub fn passing_gate(module: &str, code_version: &CodeVersion) -> GateRecord {
        let at = "2026-10-29T13:00:00+08:00".to_owned();
        GateRecord {
            module: module.to_owned(),
            code_version: code_version.clone(),
            commands: GATE_COMMANDS
                .iter()
                .map(|id| GateCommand {
                    crate_name: id.crate_name.to_owned(),
                    command: id.command.to_owned(),
                    exit_code: Some(0),
                    passed: if id.counts_tests { Count::Value(3) } else { Count::Label(CountLabel::NotApplicable) },
                    failed: if id.counts_tests { Count::Value(0) } else { Count::Label(CountLabel::NotApplicable) },
                    started_at: at.clone(),
                    finished_at: at.clone(),
                    stdout_path: None,
                    stderr_path: None,
                })
                .collect(),
        }
    }

    pub fn review(tool: &str, diff_sha256: Option<String>) -> ReviewRecord {
        let at = "2026-10-29T13:00:00+08:00".to_owned();
        let independent = tool == INDEPENDENT_REVIEWER;
        ReviewRecord {
            tool: tool.to_owned(),
            scope: ReviewScope { base: "head".into(), target: "worktree".into(), files: vec!["src/x.rs".into()] },
            diff_sha256,
            session_id: independent.then(|| "review-session".to_owned()),
            implementation_session_id: independent.then(|| "implementation-session".to_owned()),
            findings: vec![Finding {
                location: "src/x.rs:1".into(),
                description: "d".into(),
                disposition: Disposition::Fixed,
                reason: None,
            }],
            gap: None,
            started_at: at.clone(),
            finished_at: at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fixtures::*, *};
    use crate::{common::sha256_hex, digest::WORKTREE_DIFF_RULE};

    fn version() -> CodeVersion {
        CodeVersion {
            head: "56d55d209ecea00e80dfefecdd3e9d47fb62c47a".into(),
            diff_sha256: Some(sha256_hex(b"diff")),
            diff_rule: WORKTREE_DIFF_RULE.into(),
        }
    }

    #[test]
    fn counts_serialize_as_numbers_or_chinese_labels() {
        assert_eq!(serde_json::to_string(&Count::Value(7)).unwrap(), "7");
        assert_eq!(serde_json::to_string(&Count::Label(CountLabel::NotApplicable)).unwrap(), "\"不适用\"");
        assert_eq!(serde_json::from_str::<Count>("\"未解析\"").unwrap(), Count::Label(CountLabel::Unparsed));
        assert!(serde_json::from_str::<Count>("\"0\"").is_err());
        assert!(serde_json::from_str::<Count>("null").is_err());
    }

    #[test]
    fn gate_passes_only_with_every_command_clean() {
        let gate = passing_gate("M-X-LEDGER", &version());
        gate.check_passes().unwrap();
        let text = serde_json::to_string(&gate).unwrap();
        assert!(text.contains("\"crate\":\"ara-agent-lite-core\""), "{text}");
        assert_eq!(serde_json::from_str::<GateRecord>(&text).unwrap(), gate);

        type Change = fn(&mut GateRecord);
        let broken: [(&str, Change); 7] = [
            ("exit code 1", |g| g.commands[2].exit_code = Some(1)),
            ("no exit code", |g| g.commands[9].exit_code = None),
            ("2 failed", |g| g.commands[5].failed = Count::Value(2)),
            ("unparsed", |g| g.commands[10].passed = Count::Label(CountLabel::Unparsed)),
            ("不适用", |g| g.commands[0].passed = Count::Value(0)),
            ("missing", |g| {
                g.commands.remove(4);
            }),
            ("repeats", |g| {
                let first = g.commands[0].clone();
                g.commands.push(first);
            }),
        ];
        for (needle, change) in broken {
            let mut record = gate.clone();
            change(&mut record);
            let error = format!("{:#}", record.check_passes().unwrap_err());
            assert!(error.contains(needle), "{needle}: {error}");
        }
        let mut extra = gate.clone();
        extra.commands[0].command = "cargo fmt".into();
        assert!(format!("{:#}", extra.check_passes().unwrap_err()).contains("unexpected command"));
    }

    #[test]
    fn reviews_need_dispositions_sessions_and_full_coverage() {
        for tool in [INDEPENDENT_REVIEWER, GIT_REVIEW, RUST_CORE_REVIEW] {
            review(tool, None).check_complete().unwrap();
        }
        let mut gap = review(GIT_REVIEW, None);
        gap.gap = Some(ReviewGap { reason: "tool unavailable".into() });
        assert!(format!("{:#}", gap.check_complete().unwrap_err()).contains("gap"));

        let mut deferred = review(GIT_REVIEW, None);
        deferred.findings[0].disposition = Disposition::Deferred;
        assert!(deferred.check_complete().is_err());
        deferred.findings[0].reason = Some("out of scope; tracked as task 30".into());
        deferred.check_complete().unwrap();

        let mut same_session = review(INDEPENDENT_REVIEWER, None);
        same_session.session_id = same_session.implementation_session_id.clone();
        assert!(same_session.check_complete().is_err());
        let mut no_session = review(INDEPENDENT_REVIEWER, None);
        no_session.session_id = None;
        assert!(no_session.check_complete().is_err());

        let all = [review(INDEPENDENT_REVIEWER, None), review(GIT_REVIEW, None), review(PROVIDER_REVIEW, None)];
        check_review_coverage(&all).unwrap();
        let error = format!("{:#}", check_review_coverage(&all[1..]).unwrap_err());
        assert!(error.contains("codex"), "{error}");
        let error = format!("{:#}", check_review_coverage(&all[..2]).unwrap_err());
        assert!(error.contains("ara-rust-core-review or ara-provider-review"), "{error}");
    }
}
