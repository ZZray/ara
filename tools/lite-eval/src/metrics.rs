//! Result metrics (requirement 4; design “指标与结果记录”).
//!
//! Usage of every Logical_Call comes from the `model_result.usage_metrics`
//! event the Core wrote into the Harness events file of that Run. A call the
//! provider adapter did not report positive counts for is unknown with a
//! reason; unknown is never counted as 0 in any total. Costs are unknown
//! while the Run_Policy price is `null` or the usage is unknown.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{host::WriteCounts, policy::Price};

/// A value that is either known or unknown with the reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Known<T> {
    Known { value: T },
    Unknown { reason: String },
}

/// Oracle pass and total counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub passed: u64,
    pub total: u64,
}

/// Token counts the provider adapter reported for one Logical_Call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallTokens {
    /// Uncached input plus cache reads and writes.
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Usage of one Logical_Call. `run` is the Run number (0 when the call is
/// known only from the Harness count), `call` the Core call number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallUsage {
    pub run: usize,
    pub call: u64,
    pub phase: String,
    #[serde(flatten)]
    pub usage: Known<CallTokens>,
}

impl CallUsage {
    fn tokens(&self) -> Option<CallTokens> {
        match &self.usage {
            Known::Known { value } => Some(*value),
            Known::Unknown { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
    /// Every part is known.
    Complete,
    /// Some parts are unknown; totals cover only the known parts.
    Incomplete,
    /// Nothing is known; no total is given.
    Unknown,
}

/// Usage of one task. Totals are absent (never 0) when the status is
/// [`Completeness::Unknown`]; when [`Completeness::Incomplete`] they are the
/// subtotal of the reported calls and `unknown_calls` counts the rest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTotals {
    pub status: Completeness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    pub unknown_calls: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<CallUsage>,
}

/// Requirements 4.3 and 4.6. No call at all is a known zero: nothing was
/// sent. Whether the call count itself is known is the caller's concern
/// (see [`task_usage`]).
pub fn aggregate_usage(calls: &[CallUsage]) -> UsageTotals {
    let known: Vec<CallTokens> = calls.iter().filter_map(CallUsage::tokens).collect();
    let unknown_calls = calls.len() - known.len();
    let input = known.iter().fold(0u64, |sum, tokens| sum.saturating_add(tokens.input_tokens));
    let output = known.iter().fold(0u64, |sum, tokens| sum.saturating_add(tokens.output_tokens));
    let (status, reason) = match (known.len(), unknown_calls) {
        (_, 0) => (Completeness::Complete, None),
        (0, n) => {
            (Completeness::Unknown, Some(format!("the provider reported no usage for any of the {n} Logical_Calls")))
        }
        (_, n) => (Completeness::Incomplete, Some(format!("the provider reported no usage for {n} Logical_Calls"))),
    };
    let totals = status != Completeness::Unknown;
    UsageTotals {
        status,
        input_tokens: totals.then_some(input),
        output_tokens: totals.then_some(output),
        unknown_calls,
        reason,
        calls: calls.to_vec(),
    }
}

/// One line of a Harness `run-N-events.jsonl` (an `EventEnvelope`).
#[derive(Clone, Debug, PartialEq)]
pub struct EventLine {
    pub seq: u64,
    pub event: Value,
}

/// A parsed events file. Lines that are not envelopes are skipped and named
/// in `errors`; an unreadable file has no lines and one error.
#[derive(Clone, Debug, PartialEq)]
pub struct EventsFile {
    pub path: PathBuf,
    /// Run number from the file name `run-N-events.jsonl` (0 if absent).
    pub run: usize,
    pub lines: Vec<EventLine>,
    pub errors: Vec<String>,
}

/// Run number of `run-N-events.jsonl`.
pub fn events_run_number(path: &Path) -> Option<usize> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix("run-")?.strip_suffix("-events.jsonl")?.parse().ok()
}

pub fn load_events(path: &Path) -> EventsFile {
    let run = events_run_number(path).unwrap_or(0);
    let mut file = EventsFile { path: path.to_owned(), run, lines: Vec::new(), errors: Vec::new() };
    let text = match fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) => {
            file.errors.push(format!("cannot read {}: {error}", path.display()));
            return file;
        }
    };
    for (index, line) in text.lines().enumerate().filter(|(_, line)| !line.trim().is_empty()) {
        let parsed = serde_json::from_str::<Value>(line).ok().and_then(|value| {
            let seq = value.get("seq")?.as_u64()?;
            Some(EventLine { seq, event: value.get("event")?.clone() })
        });
        match parsed {
            Some(line) => file.lines.push(line),
            None => file.errors.push(format!("{} line {} is not an event envelope", path.display(), index + 1)),
        }
    }
    file
}

/// Usage of one call from its `usage_metrics` object (design: only positive
/// adapter counts are reported; anything else is unknown).
pub fn call_tokens(metrics: &Value) -> Known<CallTokens> {
    let status = metrics["status"].as_str().unwrap_or("absent");
    if status != "adapter_reported_positive_counts" {
        return Known::Unknown { reason: format!("adapter reported no positive usage counts (status {status})") };
    }
    let positive = |key: &str| metrics[key].as_u64().filter(|n| *n > 0);
    let Some(output_tokens) = positive("output_tokens") else {
        return Known::Unknown { reason: "adapter reported no positive output token count".into() };
    };
    let input_tokens = ["input_uncached_tokens", "cache_read_tokens", "cache_write_tokens"]
        .iter()
        .filter_map(|key| positive(key))
        .fold(0u64, u64::saturating_add);
    if input_tokens == 0 {
        return Known::Unknown { reason: "adapter reported no positive input token count".into() };
    }
    Known::Known { value: CallTokens { input_tokens, output_tokens } }
}

/// Every Logical_Call (`model_call` reservation) of one events file with the
/// usage of its matching `model_result`.
pub fn calls_in(file: &EventsFile) -> Vec<CallUsage> {
    let mut calls: Vec<CallUsage> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    for line in &file.lines {
        let event = &line.event;
        let call = event["call"].as_u64().unwrap_or(0);
        let phase = event["phase"].as_str().unwrap_or("").to_owned();
        match event["type"].as_str() {
            Some("model_call") => {
                open.push(calls.len());
                calls.push(CallUsage {
                    run: file.run,
                    call,
                    phase,
                    usage: Known::Unknown { reason: "no model_result event for this call".into() },
                });
            }
            Some("model_result") => {
                if let Some(position) = open.iter().rposition(|i| calls[*i].call == call && calls[*i].phase == phase) {
                    let index = open.remove(position);
                    calls[index].usage = call_tokens(&event["usage_metrics"]);
                }
            }
            _ => {}
        }
    }
    calls
}

/// Usage of a task from its events files. `logical_calls` is the Harness
/// count; `None` means it is unknown, which makes the usage unknown. Calls
/// the Harness counted but no readable events file shows are unknown.
pub fn task_usage(events: &[EventsFile], logical_calls: Option<usize>) -> UsageTotals {
    let mut calls: Vec<CallUsage> = events.iter().flat_map(calls_in).collect();
    let Some(count) = logical_calls else {
        return UsageTotals {
            status: Completeness::Unknown,
            input_tokens: None,
            output_tokens: None,
            unknown_calls: calls.len(),
            reason: Some("the Harness reported no Logical_Call count (it did not finish normally)".into()),
            calls,
        };
    };
    while calls.len() < count {
        calls.push(CallUsage {
            run: 0,
            call: 0,
            phase: "unknown".into(),
            usage: Known::Unknown {
                reason: "Logical_Call counted by the Harness but not found in a readable events file".into(),
            },
        });
    }
    aggregate_usage(&calls)
}

/// Cost of one task (requirements 4.4, 4.6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostRecord {
    pub status: Completeness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub fn cost(usage: &UsageTotals, price: Option<&Price>) -> CostRecord {
    let unknown = |reason: String| CostRecord {
        status: Completeness::Unknown,
        amount: None,
        currency: None,
        basis: None,
        reason: Some(reason),
    };
    let rates =
        price.and_then(|price| Some((price.currency.clone()?, price.input_per_million?, price.output_per_million?)));
    let Some((currency, input_rate, output_rate)) = rates else {
        return unknown("model price unknown (Run_Policy price has a null item)".into());
    };
    let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens) else {
        return unknown(format!("usage unknown: {}", usage.reason.as_deref().unwrap_or("no usage reported")));
    };
    let amount = (input as f64 * input_rate + output as f64 * output_rate) / 1_000_000.0;
    let (status, basis) = match usage.status {
        Completeness::Incomplete => {
            (Completeness::Incomplete, format!("reported calls only; {} calls unknown", usage.unknown_calls))
        }
        _ => (Completeness::Complete, "all calls reported".to_owned()),
    };
    CostRecord { status, amount: Some(amount), currency: Some(currency), basis: Some(basis), reason: None }
}

/// Oracle correctness as recorded in a task's metrics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectnessMetrics {
    /// The Oracle completed a judgement.
    pub known: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public: Option<Counts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<Counts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holdout: Option<Counts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Per-task metrics kept in the Run_Manifest so a comparison needs only the
/// two manifests. The full record is `tasks/<id>/result.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskMetrics {
    /// Requirement 4.2: only an Oracle pass.
    pub success: bool,
    pub stop: Option<String>,
    pub correctness: CorrectnessMetrics,
    pub logical_calls: Option<usize>,
    pub wall_ms: Option<u64>,
    /// Totals only; the per-call list is in `result.json`.
    pub usage: UsageTotals,
    pub cost: CostRecord,
    pub write_counts: WriteCounts,
    /// Automatic Failure_Class (`model_gap` …), `pending`, or `None` when
    /// the task passed or its Failure_Record is missing.
    pub failure_class: Option<String>,
}

/// One summary total over the tasks of a run (requirement 4.8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Total {
    pub status: Completeness,
    /// Tasks that took part.
    pub tasks: usize,
    /// Tasks whose value is unknown or incomplete.
    pub tasks_with_unknown: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

impl Total {
    fn new(statuses: &[Completeness]) -> Self {
        let tasks_with_unknown = statuses.iter().filter(|status| **status != Completeness::Complete).count();
        let status = if statuses.is_empty() || statuses.iter().all(|status| *status == Completeness::Unknown) {
            Completeness::Unknown
        } else if tasks_with_unknown > 0 {
            Completeness::Incomplete
        } else {
            Completeness::Complete
        };
        Self {
            status,
            tasks: statuses.len(),
            tasks_with_unknown,
            input_tokens: None,
            output_tokens: None,
            amount: None,
            currency: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetricsSummary {
    pub usage: Total,
    pub cost: Total,
    pub correctness: Total,
}

/// Requirement 4.8: a total over tasks with unknown or incomplete values is
/// incomplete and names how many such tasks there are; unknown values never
/// enter a sum. With no task at all every total is unknown.
pub fn summarize(tasks: &[TaskMetrics]) -> MetricsSummary {
    let statuses = |f: &dyn Fn(&TaskMetrics) -> Completeness| tasks.iter().map(f).collect::<Vec<_>>();
    let mut usage = Total::new(&statuses(&|task| task.usage.status));
    if usage.status != Completeness::Unknown {
        let sum = |f: &dyn Fn(&UsageTotals) -> Option<u64>| {
            tasks.iter().filter_map(|task| f(&task.usage)).fold(0u64, u64::saturating_add)
        };
        usage.input_tokens = Some(sum(&|usage| usage.input_tokens));
        usage.output_tokens = Some(sum(&|usage| usage.output_tokens));
    }
    let mut cost = Total::new(&statuses(&|task| task.cost.status));
    if cost.status != Completeness::Unknown {
        cost.amount = Some(tasks.iter().filter_map(|task| task.cost.amount).sum());
        cost.currency = tasks.iter().find_map(|task| task.cost.currency.clone());
    }
    let correctness = Total::new(&statuses(&|task| {
        if task.correctness.known {
            Completeness::Complete
        } else {
            Completeness::Unknown
        }
    }));
    MetricsSummary { usage, cost, correctness }
}

/// An evidence path, checked for existence when the result is written (4.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PathRecord {
    Present { path: PathBuf },
    Missing { reason: String },
}

impl PathRecord {
    pub fn of(path: &Path, missing: &str) -> Self {
        if path.exists() {
            Self::Present { path: path.to_owned() }
        } else {
            Self::Missing { reason: missing.to_owned() }
        }
    }
}

/// Paths every task result names (requirement 4.5).
pub const REQUIRED_PATHS: [&str; 3] = ["artifacts", "session", "events"];

/// Re-check every `{"status":"present","path":…}` in `paths` right before
/// the result is written, and add each missing required entry with
/// `reason`. A present path that no longer exists becomes missing.
pub fn checked_paths(paths: Value, reason: &str) -> Value {
    fn walk(value: &mut Value) {
        match value {
            Value::Object(map) => {
                let present = map.get("status").and_then(Value::as_str) == Some("present");
                let gone = map.get("path").and_then(Value::as_str).is_some_and(|path| !Path::new(path).exists());
                if present && gone {
                    *value = serde_json::json!({"status": "missing",
                        "reason": "the path no longer existed when the result was written"});
                    return;
                }
                map.values_mut().for_each(walk);
            }
            Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    let mut paths = match paths {
        Value::Object(map) => Value::Object(map),
        _ => Value::Object(Default::default()),
    };
    walk(&mut paths);
    let missing = serde_json::to_value(PathRecord::Missing { reason: reason.to_owned() }).expect("serializable");
    for key in REQUIRED_PATHS {
        let entry = &mut paths[key];
        let empty = entry.is_null() || entry.as_array().is_some_and(Vec::is_empty);
        if empty {
            *entry = if key == "events" { Value::Array(vec![missing.clone()]) } else { missing.clone() };
        }
    }
    paths
}
