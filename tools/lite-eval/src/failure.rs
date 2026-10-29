//! Failure_Record and classification (requirement 5; design “失败记录与分类”).
//!
//! Every task that did not pass its Oracle gets `tasks/<id>/failure.json`.
//! It references the last 20 events of the task's events files, the
//! artifacts, the Oracle output and an automatic classification. The rules
//! classify only with supporting evidence; otherwise the record is pending
//! and names what is missing. `lite-eval classify` adds a manual class in a
//! new file next to the record and never edits the record itself.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Result};
use ara_agent_lite_harness::TaskStop;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    common::{sha256_file, timestamp_now, write_json_new, Credentials},
    metrics::{load_events, Counts, EventsFile, PathRecord},
};

pub const FAILURE_RECORD_VERSION: u32 = 1;
/// Events referenced from the end of the task's events (5.1).
pub const LAST_EVENTS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    ModelGap,
    CoreDefect,
    WiringGap,
    ToolGap,
}

impl FailureClass {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "model_gap" => Self::ModelGap,
            "core_defect" => Self::CoreDefect,
            "wiring_gap" => Self::WiringGap,
            "tool_gap" => Self::ToolGap,
            _ => bail!("class must be model_gap, core_defect, wiring_gap or tool_gap, not `{text}`"),
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ModelGap => "model_gap",
            Self::CoreDefect => "core_defect",
            Self::WiringGap => "wiring_gap",
            Self::ToolGap => "tool_gap",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceRef {
    Event { events_path: PathBuf, seq: u64 },
    File { path: PathBuf },
    Log { path: PathBuf, line: u64 },
}

impl EvidenceRef {
    /// `event:<events-file>#<seq>`, `file:<path>` or `log:<path>#<line>`.
    pub fn parse(text: &str) -> Result<Self> {
        let (kind, rest) = text.split_once(':').context("evidence must start with event:, file: or log:")?;
        let numbered = |rest: &str| -> Result<(PathBuf, u64)> {
            let (path, number) = rest.rsplit_once('#').context("event: and log: evidence need `<path>#<number>`")?;
            Ok((PathBuf::from(path), number.parse().with_context(|| format!("`{number}` is not a number"))?))
        };
        Ok(match kind {
            "event" => {
                let (events_path, seq) = numbered(rest)?;
                Self::Event { events_path, seq }
            }
            "log" => {
                let (path, line) = numbered(rest)?;
                Self::Log { path, line }
            }
            "file" => Self::File { path: PathBuf::from(rest) },
            _ => bail!("evidence kind `{kind}` is not event, file or log"),
        })
    }

    /// Why this reference does not point at existing evidence, if it does not.
    pub fn problem(&self) -> Option<String> {
        match self {
            Self::File { path } => (!path.exists()).then(|| format!("file {} does not exist", path.display())),
            Self::Event { events_path, seq } => {
                let file = load_events(events_path);
                if let Some(error) = file.errors.first().filter(|_| file.lines.is_empty()) {
                    return Some(error.clone());
                }
                (!file.lines.iter().any(|line| line.seq == *seq))
                    .then(|| format!("event {seq} does not exist in {}", events_path.display()))
            }
            Self::Log { path, line } => match fs::read(path) {
                Err(error) => Some(format!("log {} cannot be read: {error}", path.display())),
                Ok(bytes) => {
                    let lines = String::from_utf8_lossy(&bytes).lines().count() as u64;
                    (*line == 0 || *line > lines)
                        .then(|| format!("log {} has {lines} lines, not line {line}", path.display()))
                }
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Classification {
    Classified { class: FailureClass, evidence: Vec<EvidenceRef> },
    Pending { missing: Vec<String> },
}

impl Classification {
    /// `model_gap` … or `pending`.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Classified { class, .. } => class.as_str(),
            Self::Pending { .. } => "pending",
        }
    }
}

/// Requirement 5.2/5.3: a class has at least one reference and every
/// reference exists; a pending classification names at least one missing item.
pub fn validate_refs(classification: &Classification) -> Result<(), Vec<String>> {
    let problems: Vec<String> = match classification {
        Classification::Classified { evidence, .. } if evidence.is_empty() => {
            vec!["a Failure_Class needs at least one evidence reference".into()]
        }
        Classification::Classified { evidence, .. } => evidence.iter().filter_map(EvidenceRef::problem).collect(),
        Classification::Pending { missing } if missing.is_empty() => {
            vec!["a pending classification must name at least one missing piece of evidence".into()]
        }
        Classification::Pending { .. } => Vec::new(),
    };
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// Oracle output of a failed task (5.5, 5.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OracleRecord {
    pub executed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public: Option<Counts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<Counts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holdout: Option<Counts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_output: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Event sequence numbers referenced in one events file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventGroup {
    pub events_path: PathBuf,
    pub seq: Vec<u64>,
}

/// The last `min(20, n)` events over the task's events files in Run order.
pub fn last_event_refs(events: &[EventsFile]) -> Vec<EventGroup> {
    let all: Vec<(usize, u64)> = events
        .iter()
        .enumerate()
        .flat_map(|(index, file)| file.lines.iter().map(move |line| (index, line.seq)))
        .collect();
    let mut groups: Vec<EventGroup> = Vec::new();
    for (index, seq) in &all[all.len().saturating_sub(LAST_EVENTS)..] {
        let path = &events[*index].path;
        match groups.last_mut() {
            Some(group) if group.events_path == *path => group.seq.push(*seq),
            _ => groups.push(EventGroup { events_path: path.clone(), seq: vec![*seq] }),
        }
    }
    groups
}

/// What the classification rules look at.
#[derive(Clone, Debug)]
pub struct FailureContext {
    pub task_id: String,
    pub run_ids: Vec<String>,
    /// `None` when the Harness never reported an outcome.
    pub stop: Option<TaskStop>,
    pub detail: String,
    pub error: Option<String>,
    pub events: Vec<EventsFile>,
    /// The Harness evidence directory (`run-N-core-error.json`, observations).
    pub harness_evidence: Option<PathBuf>,
    pub artifacts: PathRecord,
    pub oracle: OracleRecord,
    /// Tools registered for the task (an unregistered call is a tool gap).
    pub registered_tools: Vec<String>,
}

fn harness_files(context: &FailureContext, suffix: &str) -> Vec<PathBuf> {
    let Some(dir) = &context.harness_evidence else { return Vec::new() };
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|item| item.ok().map(|item| item.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("run-") && name.ends_with(suffix))
        })
        .collect();
    files.sort();
    files
}

fn first_event(context: &FailureContext, matches: impl Fn(&Value) -> bool) -> Option<EvidenceRef> {
    context.events.iter().find_map(|file| {
        file.lines
            .iter()
            .find(|line| matches(&line.event))
            .map(|line| EvidenceRef::Event { events_path: file.path.clone(), seq: line.seq })
    })
}

fn last_event(context: &FailureContext, matches: impl Fn(&Value) -> bool) -> Option<EvidenceRef> {
    context.events.iter().rev().find_map(|file| {
        file.lines
            .iter()
            .rev()
            .find(|line| matches(&line.event))
            .map(|line| EvidenceRef::Event { events_path: file.path.clone(), seq: line.seq })
    })
}

/// The design's rule table, first match wins; no supporting evidence means
/// pending with the missing evidence named.
pub fn auto_classify(context: &FailureContext) -> Classification {
    let classified = |class, evidence| Classification::Classified { class, evidence };
    // Provider error after a successful probe: the route or authorization.
    if let Some(event) =
        first_event(context, |event| event["type"] == "model_result" && event["stop_reason"] == "error")
    {
        return classified(FailureClass::WiringGap, vec![event]);
    }
    // Core returned an error, or the event/receipt protocol failed.
    if let Some(path) = harness_files(context, "-core-error.json").into_iter().next() {
        return classified(FailureClass::CoreDefect, vec![EvidenceRef::File { path }]);
    }
    let protocol_failure = harness_files(context, "-observation.json").into_iter().find(|path| {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|value| value["event_failure"] == true)
    });
    if let Some(path) = protocol_failure {
        return classified(FailureClass::CoreDefect, vec![EvidenceRef::File { path }]);
    }
    // A call of a tool the Host did not register.
    let registered = &context.registered_tools;
    if let Some(event) = first_event(context, |event| {
        let title = event["title"].as_str().unwrap_or("");
        event["type"] == "tool_end"
            && event["status"] == "failed"
            && (event["output"].as_str().is_some_and(|output| output.contains(&format!("Tool {title} not found")))
                || (!registered.is_empty() && !registered.iter().any(|name| name == title)))
    }) {
        return classified(FailureClass::ToolGap, vec![event]);
    }
    let mut missing = Vec::new();
    match context.stop {
        Some(TaskStop::BudgetExhausted) => missing.push(
            "budget exhausted: needs a judgement whether the Run_Policy budget was sufficient for this task".into(),
        ),
        Some(TaskStop::Cancelled) => {
            missing.push("cancelled or deadline reached: needs evidence whether the time budget was sufficient".into())
        }
        Some(TaskStop::ReconciliationRequired) => {
            missing.push("reconciliation required without a Core error file: needs the effect reconciliation".into())
        }
        Some(_) => {
            let end = last_event(context, |event| event["type"] == "agent_end");
            let oracle_file = context.oracle.raw_output.clone().filter(|path| path.exists());
            match (context.oracle.passed, oracle_file, end) {
                (Some(false), Some(path), Some(end)) => {
                    return classified(FailureClass::ModelGap, vec![EvidenceRef::File { path }, end]);
                }
                (Some(false), None, _) => missing.push("Oracle raw output file".into()),
                (Some(false), _, None) => missing.push("final agent_end event of the Run".into()),
                _ => missing.push(format!(
                    "Oracle judgement: {}",
                    context.oracle.reason.as_deref().unwrap_or("the Oracle gave no judgement")
                )),
            }
        }
        None => missing.push(format!(
            "Harness outcome: the task ended before the Harness reported a stop ({})",
            context.error.as_deref().unwrap_or("no reason recorded")
        )),
    }
    if context.events.is_empty() {
        missing.push("events file of the task".into());
    }
    Classification::Pending { missing }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FailureRecord {
    pub version: u32,
    pub task_id: String,
    pub run_ids: Vec<String>,
    /// Harness stop reason; `None` when the Harness never reported one.
    pub stop: Option<TaskStop>,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub last_events: Vec<EventGroup>,
    /// Events files or lines that could not be read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events_unreadable: Vec<String>,
    pub artifacts: PathRecord,
    pub oracle: OracleRecord,
    pub classification: Classification,
}

impl FailureRecord {
    pub fn build(context: &FailureContext) -> Self {
        let mut classification = auto_classify(context);
        if let Err(problems) = validate_refs(&classification) {
            classification = Classification::Pending { missing: problems };
        }
        Self {
            version: FAILURE_RECORD_VERSION,
            task_id: context.task_id.clone(),
            run_ids: context.run_ids.clone(),
            stop: context.stop,
            detail: context.detail.clone(),
            error: context.error.clone(),
            last_events: last_event_refs(&context.events),
            events_unreadable: context.events.iter().flat_map(|file| file.errors.clone()).collect(),
            artifacts: context.artifacts.clone(),
            oracle: context.oracle.clone(),
            classification,
        }
    }
}

/// Build and write the Failure_Record at `path` (never overwrites).
pub fn write(context: &FailureContext, path: &Path, credentials: &Credentials) -> Result<FailureRecord> {
    let record = FailureRecord::build(context);
    let value: Value = serde_json::from_str(&credentials.redact(&serde_json::to_string(&record)?))?;
    write_json_new(path, &value, credentials)?;
    Ok(record)
}

/// A manual classification (`<record>.classification-<M>.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManualClassification {
    pub version: u32,
    pub failure_record: PathBuf,
    /// SHA-256 of the record when it was classified; the record is unchanged.
    pub failure_record_sha256: String,
    pub original: Classification,
    pub classification: Classification,
    pub classified_at: String,
}

fn classification_path(record: &Path, number: u32) -> PathBuf {
    let stem = record.file_stem().and_then(|stem| stem.to_str()).unwrap_or("failure");
    record.with_file_name(format!("{stem}.classification-{number}.json"))
}

/// Requirement 5.2: append a manual class with existing evidence in a new
/// file; the Failure_Record is read only.
pub fn classify(
    record_path: &Path,
    class: FailureClass,
    evidence: Vec<EvidenceRef>,
    credentials: &Credentials,
) -> Result<PathBuf> {
    let bytes = fs::read(record_path).with_context(|| format!("cannot read {}", record_path.display()))?;
    let record: FailureRecord =
        serde_json::from_slice(&bytes).with_context(|| format!("{} is not a Failure_Record", record_path.display()))?;
    let classification = Classification::Classified { class, evidence };
    if let Err(problems) = validate_refs(&classification) {
        bail!("evidence does not point at existing evidence: {}", problems.join("; "));
    }
    let manual = ManualClassification {
        version: FAILURE_RECORD_VERSION,
        failure_record: record_path.to_owned(),
        failure_record_sha256: sha256_file(record_path)?,
        original: record.classification,
        classification,
        classified_at: timestamp_now(),
    };
    for number in 1..=9999 {
        let path = classification_path(record_path, number);
        if path.exists() {
            continue;
        }
        write_json_new(&path, &manual, credentials)?;
        ensure!(fs::read(record_path)? == bytes, "the Failure_Record changed while it was classified");
        return Ok(path);
    }
    bail!("no unused classification file name left next to {}", record_path.display())
}

/// The latest manual class of a Failure_Record, else its own classification
/// label; `None` when the record cannot be read.
pub fn current_class(record_path: &Path) -> Option<String> {
    let manual = (1..=9999)
        .map(|number| classification_path(record_path, number))
        .take_while(|path| path.exists())
        .last()
        .and_then(|path| serde_json::from_slice::<ManualClassification>(&fs::read(path).ok()?).ok());
    if let Some(manual) = manual {
        return Some(manual.classification.label().to_owned());
    }
    let record: FailureRecord = serde_json::from_slice(&fs::read(record_path).ok()?).ok()?;
    Some(record.classification.label().to_owned())
}
