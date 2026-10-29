//! Task_Set registry (design “Task_Set 登记”, requirement 1).
//!
//! * `tools/lite-eval/task-set.json` is the public registration. [`validate`]
//!   reports every structural violation instead of stopping at the first, so
//!   Run_Manifest can list them all (1.3).
//! * `tools/lite-eval/holdout-log.jsonl` is append-only. [`check_holdout_log`]
//!   derives each task's Holdout_Status from the log and compares it with the
//!   registration, so editing the JSON back to `new holdout` is detected (1.7).
//!   [`mark_not_new`] is the only status transition: `new holdout` (or a task
//!   without a status yet) to `not new holdout`, once.

use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Result};
use ara_agent_lite_harness::TaskKind;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::digest::TREE_RULE;

/// Repository-relative path of the public registration.
pub const TASK_SET_PATH: &str = "tools/lite-eval/task-set.json";
/// Repository-relative path of the append-only holdout log.
pub const HOLDOUT_LOG_PATH: &str = "tools/lite-eval/holdout-log.jsonl";
/// S1, S2, S3, R1, H2 and one Sealed_Holdout (1.1).
pub const TASK_COUNT: usize = 6;
/// Case counts fixed by requirement 1.1 and 1.2.
pub const REQUIRED_CASES: [(&str, CaseField, i64); 3] =
    [("S3", CaseField::Public, 10), ("S3", CaseField::Hidden, 48), ("H2", CaseField::Hidden, 128)];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSet {
    pub version: u32,
    /// Always [`TREE_RULE`]; [`load`] refuses any other rule.
    pub digest_rule: String,
    /// Path and SHA-256 of the out-of-repo `task-set.sealed.json` (DD-02).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<SealedRef>,
    pub tasks: Vec<TaskEntry>,
}

/// One registered task. The seven fields of requirement 1.2 default to empty
/// when absent so that [`validate`] can name each missing one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskEntry {
    #[serde(default)]
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<TaskKind>,
    #[serde(default)]
    pub fixture_path: PathBuf,
    /// [`TREE_RULE`] digest of `fixture_path`.
    #[serde(default)]
    pub fixture_sha256: String,
    #[serde(default)]
    pub oracle: OracleSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holdout: Option<HoldoutStatus>,
    pub profile: HostProfile,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleSpec {
    #[serde(default, rename = "type")]
    pub oracle_type: String,
    /// Signed so that a registered zero or negative count is reported, not a
    /// parse failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_cases: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_cases: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holdout_cases: Option<i64>,
    /// Oracle adapter argv; the runner appends `--artifacts`, `--final-text`
    /// and `--out`.
    #[serde(default)]
    pub command: Vec<String>,
}

impl OracleSpec {
    pub fn cases(&self, field: CaseField) -> Option<i64> {
        match field {
            CaseField::Public => self.public_cases,
            CaseField::Hidden => self.hidden_cases,
            CaseField::Holdout => self.holdout_cases,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseField {
    Public,
    Hidden,
    Holdout,
}

impl CaseField {
    pub const ALL: [CaseField; 3] = [CaseField::Public, CaseField::Hidden, CaseField::Holdout];

    pub fn name(self) -> &'static str {
        match self {
            CaseField::Public => "oracle.public_cases",
            CaseField::Hidden => "oracle.hidden_cases",
            CaseField::Holdout => "oracle.holdout_cases",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", deny_unknown_fields)]
pub enum HoldoutStatus {
    #[serde(rename = "new holdout")]
    New { registered: NaiveDate },
    #[serde(rename = "not new holdout")]
    NotNew { first_adaptive_use: NaiveDate, changes: Vec<String> },
}

impl HoldoutStatus {
    fn describe(status: Option<&HoldoutStatus>) -> String {
        match status {
            None => "no status".into(),
            Some(HoldoutStatus::New { registered }) => format!("new holdout registered {registered}"),
            Some(HoldoutStatus::NotNew { first_adaptive_use, changes }) => {
                format!("not new holdout since {first_adaptive_use} ({})", changes.join("; "))
            }
        }
    }
}

/// Capability_Test_Host profile of a task: run mode, registered tools and the
/// public check command (design “Task_Set 公开登记”).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostProfile {
    pub mode: ProfileMode,
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub public_check: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileMode {
    Execute,
    Plan,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedRef {
    pub path: PathBuf,
    pub sha256: String,
}

/// At least this many Oracle feature strings are registered (2.5).
pub const MIN_FEATURES: usize = 3;

/// The out-of-repo `task-set.sealed.json` of the Sealed_Holdout (2.5). Its
/// feature strings never enter the repository or a model-visible text.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedTask {
    pub task_id: String,
    pub oracle_path: PathBuf,
    pub oracle_sha256: String,
    pub features: Vec<String>,
    pub registered: NaiveDate,
}

impl std::fmt::Debug for SealedTask {
    /// Feature strings are counted, never printed, so error messages and
    /// test failures cannot carry them into evidence.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedTask")
            .field("task_id", &self.task_id)
            .field("oracle_path", &self.oracle_path)
            .field("oracle_sha256", &self.oracle_sha256)
            .field("features", &self.features.len())
            .field("registered", &self.registered)
            .finish()
    }
}

/// Read the sealed registration named by `reference`, refusing it when its
/// SHA-256 differs from the public registration or it is malformed.
pub fn load_sealed(reference: &SealedRef) -> Result<SealedTask> {
    let bytes = fs::read(&reference.path)
        .with_context(|| format!("cannot read sealed registration {}", reference.path.display()))?;
    let actual = crate::common::sha256_hex(&bytes);
    ensure!(
        actual == reference.sha256,
        "sealed registration {} has SHA-256 {actual}, registered {}",
        reference.path.display(),
        reference.sha256
    );
    let sealed: SealedTask = serde_json::from_slice(&bytes)
        .with_context(|| format!("sealed registration {} is not valid", reference.path.display()))?;
    ensure!(is_sha256(&sealed.oracle_sha256), "sealed Oracle SHA-256 is not 64 lowercase hex digits");
    let features = sealed.features.iter().filter(|feature| !feature.trim().is_empty()).count();
    ensure!(
        features >= MIN_FEATURES,
        "sealed registration lists {features} feature strings, at least {MIN_FEATURES} required"
    );
    Ok(sealed)
}

/// One Task_Set compliance problem, written to `task_set.violations` of the
/// Run_Manifest. `task` is the task ID, or `#<1-based position>` when the
/// entry has no ID.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "violation", rename_all = "snake_case")]
pub enum Violation {
    TaskCount {
        expected: usize,
        actual: usize,
    },
    DuplicateTaskId {
        task_id: String,
    },
    MissingField {
        task: String,
        field: &'static str,
    },
    MalformedSha256 {
        task: String,
        value: String,
    },
    NonPositiveCases {
        task: String,
        field: &'static str,
        value: i64,
    },
    WrongCaseCount {
        task: String,
        field: &'static str,
        expected: i64,
        actual: Option<i64>,
    },
    /// `seq` of the entry at 1-based `position` is not `position`.
    HoldoutLogSequence {
        position: usize,
        seq: u64,
    },
    HoldoutInvalidEvent {
        seq: u64,
        task_id: String,
        reason: &'static str,
    },
    /// The registration and the status derived from the log differ.
    HoldoutMismatch {
        task: String,
        registered: String,
        logged: String,
    },
}

fn label(entry: &TaskEntry, index: usize) -> String {
    if entry.task_id.trim().is_empty() {
        format!("#{}", index + 1)
    } else {
        entry.task_id.clone()
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Read and parse the registration. Only unreadable or malformed JSON, an
/// unsupported `version` or a different digest rule are errors; missing task
/// fields are left for [`validate`].
pub fn load(path: &Path) -> Result<TaskSet> {
    let bytes = fs::read(path).with_context(|| format!("cannot read Task_Set {}", path.display()))?;
    let set: TaskSet =
        serde_json::from_slice(&bytes).with_context(|| format!("Task_Set {} is not valid", path.display()))?;
    ensure!(set.version == 1, "Task_Set {} has unsupported version {}", path.display(), set.version);
    ensure!(
        set.digest_rule == TREE_RULE,
        "Task_Set {} uses digest rule `{}`; only `{TREE_RULE}` is supported",
        path.display(),
        set.digest_rule
    );
    Ok(set)
}

/// Write the registration as pretty JSON through a temporary file, so a crash
/// leaves either the old or the new content.
pub fn save(set: &TaskSet, path: &Path) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(set)?;
    bytes.push(b'\n');
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, &bytes).with_context(|| format!("cannot write {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("cannot replace Task_Set {}", path.display()))
}

/// Structural checks of requirements 1.1–1.3. Pure; the order is: task
/// count, duplicate IDs in order of first occurrence, then each task in order.
pub fn validate(set: &TaskSet) -> Vec<Violation> {
    let mut violations = Vec::new();
    if set.tasks.len() != TASK_COUNT {
        violations.push(Violation::TaskCount { expected: TASK_COUNT, actual: set.tasks.len() });
    }
    let mut seen = HashSet::new();
    let mut reported = HashSet::new();
    for entry in set.tasks.iter().filter(|entry| !entry.task_id.trim().is_empty()) {
        if !seen.insert(entry.task_id.as_str()) && reported.insert(entry.task_id.as_str()) {
            violations.push(Violation::DuplicateTaskId { task_id: entry.task_id.clone() });
        }
    }
    for (index, entry) in set.tasks.iter().enumerate() {
        let task = label(entry, index);
        let missing = |field| Violation::MissingField { task: task.clone(), field };
        if entry.task_id.trim().is_empty() {
            violations.push(missing("task_id"));
        }
        if entry.kind.is_none() {
            violations.push(missing("kind"));
        }
        if entry.fixture_path.as_os_str().is_empty() {
            violations.push(missing("fixture_path"));
        }
        if entry.fixture_sha256.is_empty() {
            violations.push(missing("fixture_sha256"));
        } else if !is_sha256(&entry.fixture_sha256) {
            violations.push(Violation::MalformedSha256 { task: task.clone(), value: entry.fixture_sha256.clone() });
        }
        if entry.oracle.oracle_type.trim().is_empty() {
            violations.push(missing("oracle.type"));
        }
        if CaseField::ALL.iter().all(|field| entry.oracle.cases(*field).is_none()) {
            violations.push(missing("oracle.cases"));
        }
        for field in CaseField::ALL {
            if let Some(value) = entry.oracle.cases(field).filter(|value| *value <= 0) {
                violations.push(Violation::NonPositiveCases { task: task.clone(), field: field.name(), value });
            }
        }
        for (id, field, expected) in REQUIRED_CASES {
            let actual = entry.oracle.cases(field);
            if entry.task_id == id && actual != Some(expected) {
                violations.push(Violation::WrongCaseCount {
                    task: task.clone(),
                    field: field.name(),
                    expected,
                    actual,
                });
            }
        }
        match &entry.holdout {
            None => violations.push(missing("holdout")),
            Some(HoldoutStatus::NotNew { changes, .. }) if changes.iter().all(|change| change.trim().is_empty()) => {
                violations.push(missing("holdout.changes"))
            }
            Some(_) => {}
        }
    }
    violations
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldoutEventKind {
    RegisteredNew,
    MarkedNotNew,
}

/// One line of `holdout-log.jsonl`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoldoutEvent {
    pub seq: u64,
    pub task_id: String,
    pub event: HoldoutEventKind,
    pub date: NaiveDate,
    /// Commit IDs or change notes; at least one for `marked_not_new`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Append-only holdout log. A file that does not exist yet is an empty log;
/// an unparsable line (including a torn last line) is an error, never skipped.
pub struct HoldoutLog {
    path: PathBuf,
    events: Vec<HoldoutEvent>,
}

impl HoldoutLog {
    pub fn load(path: &Path) -> Result<Self> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error).with_context(|| format!("cannot read holdout log {}", path.display())),
        };
        let events = text
            .lines()
            .enumerate()
            .map(|(index, line)| {
                serde_json::from_str(line)
                    .with_context(|| format!("holdout log {} line {} is not a valid event", path.display(), index + 1))
            })
            .collect::<Result<_>>()?;
        Ok(Self { path: path.to_owned(), events })
    }

    pub fn events(&self) -> &[HoldoutEvent] {
        &self.events
    }

    /// Append one event with the next `seq` and flush it to disk.
    pub fn append(
        &mut self,
        task_id: &str,
        event: HoldoutEventKind,
        date: NaiveDate,
        changes: Vec<String>,
        reason: Option<String>,
    ) -> Result<&HoldoutEvent> {
        let entry = HoldoutEvent {
            seq: self.events.len() as u64 + 1,
            task_id: task_id.to_owned(),
            event,
            date,
            changes,
            reason,
        };
        let mut line = serde_json::to_vec(&entry)?;
        line.push(b'\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("cannot open holdout log {}", self.path.display()))?;
        file.write_all(&line).with_context(|| format!("cannot append to holdout log {}", self.path.display()))?;
        file.sync_all().with_context(|| format!("cannot sync holdout log {}", self.path.display()))?;
        self.events.push(entry);
        Ok(self.events.last().expect("just pushed"))
    }
}

/// Compare the registration with the status derived from the log (1.6, 1.7).
/// A task without a registered status and without log events is reported by
/// [`validate`] as a missing `holdout` field, not here.
pub fn check_holdout_log(set: &TaskSet, log: &[HoldoutEvent]) -> Vec<Violation> {
    let mut violations = Vec::new();
    let mut derived: BTreeMap<&str, HoldoutStatus> = BTreeMap::new();
    for (index, event) in log.iter().enumerate() {
        if event.seq != index as u64 + 1 {
            violations.push(Violation::HoldoutLogSequence { position: index + 1, seq: event.seq });
        }
        let invalid =
            |reason| Violation::HoldoutInvalidEvent { seq: event.seq, task_id: event.task_id.clone(), reason };
        if !set.tasks.iter().any(|entry| entry.task_id == event.task_id) {
            violations.push(invalid("task is not registered"));
            continue;
        }
        let next = match (derived.get(event.task_id.as_str()), event.event) {
            (Some(HoldoutStatus::NotNew { .. }), _) => Err("event after the task became not new holdout"),
            (Some(HoldoutStatus::New { .. }), HoldoutEventKind::RegisteredNew) => Err("task registered twice"),
            (_, HoldoutEventKind::RegisteredNew) => Ok(HoldoutStatus::New { registered: event.date }),
            (_, HoldoutEventKind::MarkedNotNew) if event.changes.iter().all(|change| change.trim().is_empty()) => {
                Err("marked not new without a change identifier")
            }
            (_, HoldoutEventKind::MarkedNotNew) => {
                Ok(HoldoutStatus::NotNew { first_adaptive_use: event.date, changes: event.changes.clone() })
            }
        };
        match next {
            Ok(status) => {
                derived.insert(&event.task_id, status);
            }
            Err(reason) => violations.push(invalid(reason)),
        }
    }
    for (index, entry) in set.tasks.iter().enumerate() {
        let logged = derived.get(entry.task_id.as_str());
        if entry.holdout.as_ref() != logged && (entry.holdout.is_some() || logged.is_some()) {
            violations.push(Violation::HoldoutMismatch {
                task: label(entry, index),
                registered: HoldoutStatus::describe(entry.holdout.as_ref()),
                logged: HoldoutStatus::describe(logged),
            });
        }
    }
    violations
}

/// Record that the Oracle results of `task_id` were used for tuning (1.6):
/// append `marked_not_new` to the log, then set the registered status. The
/// caller saves `set`. Refuses, without writing, when the log and the
/// registration already disagree, when the task is already `not new
/// holdout` (its status and date never change, 1.7), or when no change
/// identifier is given.
pub fn mark_not_new(
    set: &mut TaskSet,
    log: &mut HoldoutLog,
    task_id: &str,
    date: NaiveDate,
    changes: Vec<String>,
    reason: Option<String>,
) -> Result<()> {
    let violations = check_holdout_log(set, log.events());
    ensure!(violations.is_empty(), "Task_Set and holdout log disagree; refusing to mark {task_id}: {violations:?}");
    let changes: Vec<String> =
        changes.into_iter().map(|change| change.trim().to_owned()).filter(|change| !change.is_empty()).collect();
    ensure!(!changes.is_empty(), "marking {task_id} not new holdout needs at least one change identifier");
    let entry = set.tasks.iter_mut().find(|entry| entry.task_id == task_id);
    let entry = entry.with_context(|| format!("task {task_id} is not registered"))?;
    if let Some(HoldoutStatus::NotNew { first_adaptive_use, .. }) = &entry.holdout {
        bail!("{task_id} is already not new holdout since {first_adaptive_use}; its status and date cannot change");
    }
    log.append(task_id, HoldoutEventKind::MarkedNotNew, date, changes.clone(), reason)?;
    entry.holdout = Some(HoldoutStatus::NotNew { first_adaptive_use: date, changes });
    Ok(())
}

/// Register the Sealed_Holdout as `new holdout` before its first real-model
/// Run (2.5): append `registered_new`, then set the status. Only a task
/// without any status yet can be registered, so a `not new holdout` task
/// never returns to `new holdout`.
pub fn register_new(set: &mut TaskSet, log: &mut HoldoutLog, task_id: &str, date: NaiveDate) -> Result<()> {
    let violations = check_holdout_log(set, log.events());
    ensure!(violations.is_empty(), "Task_Set and holdout log disagree; refusing to register {task_id}: {violations:?}");
    let entry = set.tasks.iter_mut().find(|entry| entry.task_id == task_id);
    let entry = entry.with_context(|| format!("task {task_id} is not registered"))?;
    ensure!(
        entry.holdout.is_none(),
        "{task_id} already has holdout status: {}",
        HoldoutStatus::describe(entry.holdout.as_ref())
    );
    log.append(task_id, HoldoutEventKind::RegisteredNew, date, Vec::new(), None)?;
    entry.holdout = Some(HoldoutStatus::New { registered: date });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> NaiveDate {
        text.parse().unwrap()
    }

    fn entry(id: &str) -> TaskEntry {
        TaskEntry {
            task_id: id.into(),
            kind: Some(TaskKind::Coding),
            fixture_path: PathBuf::from("D:/fixtures").join(id),
            fixture_sha256: "a".repeat(64),
            oracle: OracleSpec { oracle_type: "cpp".into(), hidden_cases: Some(13), ..OracleSpec::default() },
            holdout: None,
            profile: HostProfile {
                mode: ProfileMode::Execute,
                tools: vec!["ws_read_file".into()],
                public_check: vec![],
            },
        }
    }

    fn set(ids: &[&str]) -> TaskSet {
        TaskSet {
            version: 1,
            digest_rule: TREE_RULE.into(),
            sealed: None,
            tasks: ids.iter().map(|id| entry(id)).collect(),
        }
    }

    #[test]
    fn registration_json_round_trips_with_the_documented_field_names() {
        let mut registered = set(&["S3"]);
        registered.tasks[0].oracle = OracleSpec {
            oracle_type: "cmake_ctest_plus_private_cli".into(),
            public_cases: Some(10),
            hidden_cases: Some(48),
            holdout_cases: None,
            command: vec!["python".into(), "tools/lite-eval/oracles/s3.py".into()],
        };
        registered.tasks[0].holdout =
            Some(HoldoutStatus::NotNew { first_adaptive_use: date("2026-10-23"), changes: vec!["reused".into()] });
        let value = serde_json::to_value(&registered).unwrap();
        let task = &value["tasks"][0];
        assert_eq!(task["kind"], "Coding");
        assert_eq!(task["oracle"]["type"], "cmake_ctest_plus_private_cli");
        assert_eq!(task["holdout"]["status"], "not new holdout");
        assert_eq!(task["holdout"]["first_adaptive_use"], "2026-10-23");
        assert_eq!(task["profile"]["mode"], "execute");
        assert_eq!(serde_json::from_value::<TaskSet>(value).unwrap(), registered);
    }

    #[test]
    fn missing_fields_parse_and_are_reported_by_name() {
        let parsed: TaskSet = serde_json::from_str(
            r#"{"version":1,"digest_rule":"lite-eval-tree-v1",
                "tasks":[{"profile":{"mode":"plan","tools":[]}}]}"#,
        )
        .unwrap();
        let fields: Vec<_> = validate(&parsed)
            .into_iter()
            .filter_map(|violation| match violation {
                Violation::MissingField { task, field } => Some((task, field)),
                _ => None,
            })
            .collect();
        let expected = ["task_id", "kind", "fixture_path", "fixture_sha256", "oracle.type", "oracle.cases", "holdout"];
        assert_eq!(fields, expected.map(|field| ("#1".to_owned(), field)));
    }

    #[test]
    fn load_rejects_other_versions_and_digest_rules() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("task-set.json");
        for (version, rule) in [(2, TREE_RULE), (1, "other-rule")] {
            let mut bad = set(&["S1"]);
            bad.version = version;
            bad.digest_rule = rule.into();
            fs::write(&path, serde_json::to_vec(&bad).unwrap()).unwrap();
            assert!(load(&path).is_err());
        }
        save(&set(&["S1"]), &path).unwrap();
        assert_eq!(load(&path).unwrap(), set(&["S1"]));
    }

    #[test]
    fn fixed_case_counts_and_sha_format_are_enforced() {
        let mut registered = set(&["S1", "S2", "S3", "R1", "H2", "SH1"]);
        for entry in &mut registered.tasks {
            entry.holdout = Some(HoldoutStatus::New { registered: date("2026-10-28") });
        }
        registered.tasks[0].fixture_sha256 = "A".repeat(64);
        registered.tasks[1].oracle.hidden_cases = Some(0);
        registered.tasks[2].oracle.public_cases = Some(10);
        registered.tasks[4].oracle.hidden_cases = Some(127);
        assert_eq!(
            validate(&registered),
            vec![
                Violation::MalformedSha256 { task: "S1".into(), value: "A".repeat(64) },
                Violation::NonPositiveCases { task: "S2".into(), field: "oracle.hidden_cases", value: 0 },
                Violation::WrongCaseCount {
                    task: "S3".into(),
                    field: "oracle.hidden_cases",
                    expected: 48,
                    actual: Some(13)
                },
                Violation::WrongCaseCount {
                    task: "H2".into(),
                    field: "oracle.hidden_cases",
                    expected: 128,
                    actual: Some(127)
                },
            ]
        );
    }

    #[test]
    fn marking_appends_once_and_later_changes_are_refused_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holdout-log.jsonl");
        let mut registered = set(&["S3"]);
        let mut log = HoldoutLog::load(&path).unwrap();
        assert!(mark_not_new(&mut registered, &mut log, "S3", date("2026-10-23"), vec![" ".into()], None).is_err());
        assert!(!path.exists());
        mark_not_new(&mut registered, &mut log, "S3", date("2026-10-23"), vec!["reused".into()], Some("why".into()))
            .unwrap();
        let bytes = fs::read(&path).unwrap();
        for day in ["2026-10-23", "2026-10-24"] {
            assert!(mark_not_new(&mut registered, &mut log, "S3", date(day), vec!["again".into()], None).is_err());
        }
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let reloaded = HoldoutLog::load(&path).unwrap();
        assert_eq!(reloaded.events().len(), 1);
        assert!(check_holdout_log(&registered, reloaded.events()).is_empty());
        // Editing the registration back is a mismatch, and marking stays refused.
        registered.tasks[0].holdout = Some(HoldoutStatus::New { registered: date("2026-10-22") });
        assert!(matches!(
            check_holdout_log(&registered, reloaded.events()).as_slice(),
            [Violation::HoldoutMismatch { .. }]
        ));
        let mut log = reloaded;
        assert!(mark_not_new(&mut registered, &mut log, "S3", date("2026-10-23"), vec!["x".into()], None).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn registering_new_only_applies_to_tasks_without_status() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holdout-log.jsonl");
        let mut registered = set(&["SH1", "S3"]);
        let mut log = HoldoutLog::load(&path).unwrap();
        register_new(&mut registered, &mut log, "SH1", date("2026-10-29")).unwrap();
        assert_eq!(registered.tasks[0].holdout, Some(HoldoutStatus::New { registered: date("2026-10-29") }));
        assert!(register_new(&mut registered, &mut log, "SH1", date("2026-10-30")).is_err());
        mark_not_new(&mut registered, &mut log, "S3", date("2026-10-23"), vec!["reused".into()], None).unwrap();
        assert!(register_new(&mut registered, &mut log, "S3", date("2026-10-30")).is_err());
        mark_not_new(&mut registered, &mut log, "SH1", date("2026-10-31"), vec!["leak".into()], None).unwrap();
        assert!(register_new(&mut registered, &mut log, "SH1", date("2026-11-01")).is_err());
        assert_eq!(HoldoutLog::load(&path).unwrap().events().len(), 3);
        assert!(check_holdout_log(&registered, log.events()).is_empty());
    }

    #[test]
    fn sealed_registration_is_bound_by_sha_and_needs_three_features() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("task-set.sealed.json");
        let write = |features: &[&str]| {
            let sealed = SealedTask {
                task_id: "SH1".into(),
                oracle_path: dir.path().join("oracle.py"),
                oracle_sha256: "b".repeat(64),
                features: features.iter().map(|feature| (*feature).to_owned()).collect(),
                registered: date("2026-10-29"),
            };
            let bytes = serde_json::to_vec(&sealed).unwrap();
            fs::write(&path, &bytes).unwrap();
            SealedRef { path: path.clone(), sha256: crate::common::sha256_hex(&bytes) }
        };
        let reference = write(&["f-one", "f-two", "f-three"]);
        let sealed = load_sealed(&reference).unwrap();
        assert!(!format!("{sealed:?}").contains("f-one"));
        assert!(load_sealed(&SealedRef { sha256: "c".repeat(64), ..reference }).is_err());
        assert!(load_sealed(&write(&["f-one", "f-two", " "])).is_err());
    }

    #[test]
    fn torn_log_lines_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holdout-log.jsonl");
        fs::write(&path, b"{\"seq\":1,\"task_id\":\"S3\",\"event\":\"marked_not_new\",\"date\":\"2026-10-23\"")
            .unwrap();
        assert!(HoldoutLog::load(&path).is_err());
    }
}
