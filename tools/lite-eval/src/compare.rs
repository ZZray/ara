//! Before/after comparison of P2 items (requirements 7.7, 20.7, 21; design
//! “P2 对比”).
//!
//! Input is two sets of Run_Manifests: the baseline side and the current
//! side, each manifest one sample. Conclusions are given only when every
//! comparability item agrees with the first baseline manifest and no run had
//! an unavailable environment. A metric regresses only when both sides know
//! it; an unknown side makes it not comparable and names that side. With one
//! sample on a side the conclusion is marked `single_sample`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    common::{sha256_file, sha256_hex},
    failure,
    host::WriteCounts,
    metrics::{Completeness, TaskMetrics},
    policy::RunPolicy,
};

/// Repository-relative directory of Comparison_Records.
pub const COMPARISONS_DIR: &str = "crates/ara-agent-lite-core/docs/evidence/lite-capability/comparisons";
pub const COMPARISON_VERSION: u32 = 1;
pub const P2_ITEMS: [&str; 5] = ["P2-a", "P2-b", "P2-c", "P2-d", "P2-e"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyRef {
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeView {
    pub head: String,
    pub dirty: bool,
    #[serde(default)]
    pub worktree_diff_sha256: Option<String>,
    #[serde(default)]
    pub diff_path: Option<PathBuf>,
    #[serde(default)]
    pub capability_host_features: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskView {
    pub task_id: String,
    pub fixture_sha256: Option<String>,
    pub holdout_status_at_start: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectedView {
    pub task_id: String,
    pub state: String,
    #[serde(default)]
    pub metrics: Option<TaskMetrics>,
    #[serde(default)]
    pub failure_record: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BaselineView {
    pub usable: bool,
}

/// The parts of a Run_Manifest a comparison reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManifestView {
    pub manifest_id: String,
    pub status: String,
    pub run_policy: PolicyRef,
    #[serde(default)]
    pub policy: Option<RunPolicy>,
    pub code_version: CodeView,
    pub all_tasks: Vec<TaskView>,
    pub requested: Vec<String>,
    pub selected: Vec<SelectedView>,
    #[serde(default)]
    pub baseline: Option<BaselineView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestRef {
    pub manifest: PathBuf,
    pub manifest_id: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    pub reference: ManifestRef,
    pub view: ManifestView,
}

pub fn load(path: &Path) -> Result<Loaded> {
    let bytes = fs::read(path).with_context(|| format!("cannot read Run_Manifest {}", path.display()))?;
    let view: ManifestView =
        serde_json::from_slice(&bytes).with_context(|| format!("{} is not a Run_Manifest", path.display()))?;
    let reference =
        ManifestRef { manifest: path.to_owned(), manifest_id: view.manifest_id.clone(), sha256: sha256_hex(&bytes) };
    Ok(Loaded { reference, view })
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Mismatch {
    pub field: String,
    pub baseline: Value,
    pub current: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Comparability {
    Comparable,
    NotComparable {
        mismatches: Vec<Mismatch>,
    },
    /// A run's probe failed (21.7): no conclusion and no regression.
    EnvironmentUnavailable {
        manifests: Vec<String>,
    },
}

/// Task_Set version: the task IDs with their fixture SHA-256. The Task_Set
/// file itself is not used because Holdout_Status changes rewrite it, and
/// Holdout_Status is compared side by side instead (21.2).
pub fn task_set_version(view: &ManifestView) -> String {
    let mut tasks: Vec<(String, String)> = view
        .all_tasks
        .iter()
        .map(|task| (task.task_id.clone(), task.fixture_sha256.clone().unwrap_or_else(|| "unavailable".into())))
        .collect();
    tasks.sort();
    sha256_hex(serde_json::to_string(&tasks).expect("serializable").as_bytes())
}

/// Requirement 21.3: every differing comparability item with both values.
pub fn mismatches(base: &ManifestView, cur: &ManifestView) -> Vec<Mismatch> {
    let mut out = Vec::new();
    let mut check = |field: String, baseline: Value, current: Value| {
        if baseline != current {
            out.push(Mismatch { field, baseline, current });
        }
    };
    check("run_policy.sha256".into(), json!(base.run_policy.sha256), json!(cur.run_policy.sha256));
    let policy = |view: &ManifestView, f: &dyn Fn(&RunPolicy) -> Value| view.policy.as_ref().map_or(Value::Null, f);
    check("model.id".into(), policy(base, &|p| json!(p.model.id)), policy(cur, &|p| json!(p.model.id)));
    check(
        "model.protocol".into(),
        policy(base, &|p| json!(p.model.protocol)),
        policy(cur, &|p| json!(p.model.protocol)),
    );
    check("model.route".into(), policy(base, &|p| json!(p.model.route)), policy(cur, &|p| json!(p.model.route)));
    check("generation".into(), policy(base, &|p| json!(p.generation)), policy(cur, &|p| json!(p.generation)));
    let ids = |view: &ManifestView| view.requested.iter().cloned().collect::<BTreeSet<_>>();
    check("task_ids".into(), json!(ids(base)), json!(ids(cur)));
    for id in ids(base).union(&ids(cur)) {
        let budget = |view: &ManifestView| policy(view, &|p| json!(p.budgets.get(id)));
        check(format!("budgets.{id}"), budget(base), budget(cur));
    }
    let fixtures = |view: &ManifestView| -> BTreeMap<String, Value> {
        view.all_tasks.iter().map(|task| (task.task_id.clone(), json!(task.fixture_sha256))).collect()
    };
    let (base_fixtures, cur_fixtures) = (fixtures(base), fixtures(cur));
    let all: BTreeSet<&String> = base_fixtures.keys().chain(cur_fixtures.keys()).collect();
    for id in all {
        let value = |map: &BTreeMap<String, Value>| map.get(id).cloned().unwrap_or(Value::Null);
        check(format!("fixture_sha256.{id}"), value(&base_fixtures), value(&cur_fixtures));
    }
    out
}

/// Comparability of every manifest against the first baseline manifest.
pub fn comparability(bases: &[Loaded], currents: &[Loaded]) -> Comparability {
    let unavailable: Vec<String> = bases
        .iter()
        .chain(currents)
        .filter(|loaded| loaded.view.status == "environment_unavailable")
        .map(|loaded| loaded.view.manifest_id.clone())
        .collect();
    if !unavailable.is_empty() {
        return Comparability::EnvironmentUnavailable { manifests: unavailable };
    }
    let first = &bases[0].view;
    let several = bases.len() + currents.len() > 2;
    let mut all = Vec::new();
    for other in bases[1..].iter().chain(currents) {
        for mut mismatch in mismatches(first, &other.view) {
            if several {
                mismatch.field = format!("{}@{}", mismatch.field, other.view.manifest_id);
            }
            all.push(mismatch);
        }
    }
    if all.is_empty() {
        Comparability::Comparable
    } else {
        Comparability::NotComparable { mismatches: all }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BaselineValidity {
    Valid,
    /// Requirement 7.7: every conclusion against this baseline is invalid.
    Invalid {
        included_p2_items: Vec<String>,
        reasons: Vec<String>,
    },
}

/// Whether the baseline manifests can serve as a Baseline_Record. A P2 item
/// counts as included when the manifest's Capability_Test_Host features list
/// it; the saved diff must still match the manifest's diff SHA-256 so the
/// recorded code version can be checked against the commit.
pub fn baseline_validity(bases: &[Loaded]) -> BaselineValidity {
    let mut items = BTreeSet::new();
    let mut reasons = Vec::new();
    for loaded in bases {
        let (view, id) = (&loaded.view, &loaded.view.manifest_id);
        items.extend(view.code_version.capability_host_features.iter().cloned());
        match &view.baseline {
            None => reasons.push(format!("{id} is not a --baseline run")),
            Some(mark) if !mark.usable => reasons.push(format!("{id} is an incomplete baseline")),
            Some(_) => {}
        }
        if view.status != "completed" {
            reasons.push(format!("{id} has status {}", view.status));
        }
        if view.code_version.dirty {
            let saved = view.code_version.diff_path.as_ref().and_then(|path| sha256_file(path).ok());
            match saved {
                None => reasons
                    .push(format!("{id}: the saved worktree diff is missing; the code version cannot be checked")),
                Some(sha) if Some(&sha) != view.code_version.worktree_diff_sha256.as_ref() => {
                    reasons.push(format!("{id}: the saved worktree diff does not match the manifest SHA-256"))
                }
                Some(_) => {}
            }
        }
    }
    if !items.is_empty() {
        reasons.push(format!(
            "the baseline code includes P2 items: {}",
            items.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if reasons.is_empty() {
        BaselineValidity::Valid
    } else {
        BaselineValidity::Invalid { included_p2_items: items.into_iter().collect(), reasons }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    Success,
    PublicPassed,
    HiddenPassed,
    HoldoutPassed,
    LogicalCalls,
    WallMs,
    UsageInputTokens,
    UsageOutputTokens,
    Cost,
}

impl Metric {
    pub const ALL: [Metric; 9] = [
        Metric::Success,
        Metric::PublicPassed,
        Metric::HiddenPassed,
        Metric::HoldoutPassed,
        Metric::LogicalCalls,
        Metric::WallMs,
        Metric::UsageInputTokens,
        Metric::UsageOutputTokens,
        Metric::Cost,
    ];

    fn higher_is_better(self) -> bool {
        matches!(self, Metric::Success | Metric::PublicPassed | Metric::HiddenPassed | Metric::HoldoutPassed)
    }

    fn name(self) -> &'static str {
        match self {
            Metric::Success => "success",
            Metric::PublicPassed => "public_passed",
            Metric::HiddenPassed => "hidden_passed",
            Metric::HoldoutPassed => "holdout_passed",
            Metric::LogicalCalls => "logical_calls",
            Metric::WallMs => "wall_ms",
            Metric::UsageInputTokens => "usage_input_tokens",
            Metric::UsageOutputTokens => "usage_output_tokens",
            Metric::Cost => "cost",
        }
    }
}

/// One side's value of a metric over its samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SideValue {
    /// Mean over the samples.
    Known(f64),
    Unknown,
    /// The task has no such count (for example no public tests).
    Absent,
}

pub fn metric_value(metrics: &TaskMetrics, metric: Metric) -> SideValue {
    let known = |value: Option<f64>| value.map_or(SideValue::Unknown, SideValue::Known);
    let counts = |counts: Option<crate::metrics::Counts>| {
        if !metrics.correctness.known {
            SideValue::Unknown
        } else {
            counts.map_or(SideValue::Absent, |counts| SideValue::Known(counts.passed as f64))
        }
    };
    let complete = |status: Completeness, value: Option<f64>| {
        if status == Completeness::Complete {
            known(value)
        } else {
            SideValue::Unknown
        }
    };
    match metric {
        Metric::Success => known(metrics.correctness.known.then_some(if metrics.success { 1.0 } else { 0.0 })),
        Metric::PublicPassed => counts(metrics.correctness.public),
        Metric::HiddenPassed => counts(metrics.correctness.hidden),
        Metric::HoldoutPassed => counts(metrics.correctness.holdout),
        Metric::LogicalCalls => known(metrics.logical_calls.map(|n| n as f64)),
        Metric::WallMs => known(metrics.wall_ms.map(|n| n as f64)),
        Metric::UsageInputTokens => complete(metrics.usage.status, metrics.usage.input_tokens.map(|n| n as f64)),
        Metric::UsageOutputTokens => complete(metrics.usage.status, metrics.usage.output_tokens.map(|n| n as f64)),
        Metric::Cost => complete(metrics.cost.status, metrics.cost.amount),
    }
}

/// Mean over samples; any unknown sample, or a mix of absent and known,
/// makes the side unknown.
pub fn side_value(samples: &[&TaskMetrics], metric: Metric) -> SideValue {
    let values: Vec<SideValue> = samples.iter().map(|metrics| metric_value(metrics, metric)).collect();
    if values.iter().all(|value| *value == SideValue::Absent) {
        return SideValue::Absent;
    }
    let known: Vec<f64> =
        values.iter().filter_map(|value| if let SideValue::Known(v) = value { Some(*v) } else { None }).collect();
    if known.len() < values.len() {
        return SideValue::Unknown;
    }
    SideValue::Known(known.iter().sum::<f64>() / known.len() as f64)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Baseline,
    Current,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Delta {
    Improved,
    Regressed,
    Unchanged,
    NotComparable,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MetricComparison {
    pub baseline: Value,
    pub current: Value,
    pub delta: Delta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unknown_side: Option<Side>,
}

fn render(metric: Metric, value: SideValue) -> Value {
    match value {
        SideValue::Unknown => json!("unknown"),
        SideValue::Absent => json!("absent"),
        SideValue::Known(x) if metric == Metric::Success && (x == 0.0 || x == 1.0) => json!(x == 1.0),
        SideValue::Known(x) if metric != Metric::Cost && x.fract() == 0.0 && x.abs() < 9.0e15 => json!(x as i64),
        SideValue::Known(x) => json!(x),
    }
}

/// `None` when neither side has the metric.
pub fn compare_metric(metric: Metric, baseline: SideValue, current: SideValue) -> Option<MetricComparison> {
    let side = |b: bool, c: bool| match (b, c) {
        (true, true) => Side::Both,
        (true, false) => Side::Baseline,
        _ => Side::Current,
    };
    let (delta, unknown_side) = match (baseline, current) {
        (SideValue::Absent, SideValue::Absent) => return None,
        (SideValue::Known(b), SideValue::Known(c)) => {
            let better = if metric.higher_is_better() { c > b } else { c < b };
            let worse = if metric.higher_is_better() { c < b } else { c > b };
            (
                if better {
                    Delta::Improved
                } else if worse {
                    Delta::Regressed
                } else {
                    Delta::Unchanged
                },
                None,
            )
        }
        (b, c) => {
            (Delta::NotComparable, Some(side(!matches!(b, SideValue::Known(_)), !matches!(c, SideValue::Known(_)))))
        }
    };
    Some(MetricComparison { baseline: render(metric, baseline), current: render(metric, current), delta, unknown_side })
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Presence {
    Both,
    /// “缺失”: the side has no task record (20.7).
    Missing {
        side: Side,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Samples {
    pub baseline: usize,
    pub current: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Regression {
    pub metric: Metric,
    pub baseline: Value,
    pub current: Value,
    /// Failure_Class of the current run (`pending` included), or
    /// `not_applicable` when the current run still passes the Oracle.
    pub failure_class: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TaskComparison {
    pub task_id: String,
    pub presence: Presence,
    pub samples: Samples,
    pub single_sample: bool,
    pub metrics: BTreeMap<&'static str, MetricComparison>,
    pub write_counts: BTreeMap<&'static str, Vec<WriteCounts>>,
    pub regressions: Vec<Regression>,
    pub holdout_status: BTreeMap<&'static str, Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Conclusions {
    /// Comparable and the baseline is valid.
    Valid,
    /// Comparable but the baseline is not a valid Baseline_Record (7.7).
    Invalid,
    /// Not comparable or environment unavailable: no conclusion.
    None,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ComparisonRecord {
    pub version: u32,
    pub item: String,
    /// P2 items the current code version includes (its Host features).
    pub included_items: Vec<String>,
    pub baseline: Vec<ManifestRef>,
    pub current: Vec<ManifestRef>,
    pub comparability: Comparability,
    pub baseline_validity: BaselineValidity,
    pub conclusions: Conclusions,
    pub model: BTreeMap<&'static str, Vec<Value>>,
    pub task_set_version: BTreeMap<&'static str, Vec<String>>,
    /// Empty unless the sides are comparable.
    pub tasks: Vec<TaskComparison>,
}

fn samples<'a>(side: &'a [Loaded], task_id: &str) -> Vec<(&'a Loaded, &'a SelectedView, &'a TaskMetrics)> {
    side.iter()
        .flat_map(|loaded| {
            loaded
                .view
                .selected
                .iter()
                .filter(|record| record.task_id == task_id)
                .filter_map(move |record| record.metrics.as_ref().map(|metrics| (loaded, record, metrics)))
        })
        .collect()
}

fn failure_class(current: &[(&Loaded, &SelectedView, &TaskMetrics)]) -> String {
    if current.iter().all(|(_, _, metrics)| metrics.success) {
        return "not_applicable".into();
    }
    let (_, record, metrics) = current.iter().rev().find(|(_, _, metrics)| !metrics.success).expect("one failed");
    record
        .failure_record
        .as_deref()
        .and_then(failure::current_class)
        .or_else(|| metrics.failure_class.clone())
        .unwrap_or_else(|| "failure_record_missing".into())
}

fn compare_task(bases: &[Loaded], currents: &[Loaded], task_id: &str) -> TaskComparison {
    let (base, cur) = (samples(bases, task_id), samples(currents, task_id));
    let presence = match (base.is_empty(), cur.is_empty()) {
        (false, false) => Presence::Both,
        (true, false) => Presence::Missing { side: Side::Baseline },
        (false, true) => Presence::Missing { side: Side::Current },
        (true, true) => Presence::Missing { side: Side::Both },
    };
    let mut metrics = BTreeMap::new();
    let mut regressions = Vec::new();
    if presence == Presence::Both {
        let base_metrics: Vec<&TaskMetrics> = base.iter().map(|(_, _, metrics)| *metrics).collect();
        let cur_metrics: Vec<&TaskMetrics> = cur.iter().map(|(_, _, metrics)| *metrics).collect();
        for metric in Metric::ALL {
            let compared = compare_metric(metric, side_value(&base_metrics, metric), side_value(&cur_metrics, metric));
            if let Some(compared) = compared {
                if compared.delta == Delta::Regressed {
                    regressions.push(Regression {
                        metric,
                        baseline: compared.baseline.clone(),
                        current: compared.current.clone(),
                        failure_class: failure_class(&cur),
                    });
                }
                metrics.insert(metric.name(), compared);
            }
        }
    }
    let holdout = |side: &[Loaded]| {
        side.iter()
            .rev()
            .find_map(|loaded| loaded.view.all_tasks.iter().find(|task| task.task_id == task_id))
            .map_or(Value::Null, |task| {
                task.holdout_status_at_start.get("status").cloned().unwrap_or(task.holdout_status_at_start.clone())
            })
    };
    let counts =
        |side: &[(&Loaded, &SelectedView, &TaskMetrics)]| side.iter().map(|(_, _, m)| m.write_counts).collect();
    TaskComparison {
        task_id: task_id.to_owned(),
        presence,
        samples: Samples { baseline: base.len(), current: cur.len() },
        single_sample: base.len() == 1 || cur.len() == 1,
        metrics,
        write_counts: BTreeMap::from([("baseline", counts(&base)), ("current", counts(&cur))]),
        regressions,
        holdout_status: BTreeMap::from([("baseline", holdout(bases)), ("current", holdout(currents))]),
    }
}

/// Requirements 20.7 and 21.1–21.7.
pub fn compare(bases: &[Loaded], currents: &[Loaded], item: &str) -> ComparisonRecord {
    assert!(!bases.is_empty() && !currents.is_empty(), "both sides need a manifest");
    let comparability = comparability(bases, currents);
    let baseline_validity = baseline_validity(bases);
    let comparable = comparability == Comparability::Comparable;
    let tasks = if comparable {
        let ids: BTreeSet<&String> =
            bases.iter().chain(currents).flat_map(|loaded| loaded.view.requested.iter()).collect();
        ids.into_iter().map(|id| compare_task(bases, currents, id)).collect()
    } else {
        Vec::new()
    };
    let conclusions = match (comparable, &baseline_validity) {
        (false, _) => Conclusions::None,
        (true, BaselineValidity::Valid) => Conclusions::Valid,
        (true, BaselineValidity::Invalid { .. }) => Conclusions::Invalid,
    };
    let model = |side: &[Loaded]| {
        side.iter().map(|loaded| loaded.view.policy.as_ref().map_or(Value::Null, |p| json!(p.model.id))).collect()
    };
    let versions = |side: &[Loaded]| side.iter().map(|loaded| task_set_version(&loaded.view)).collect();
    let included: BTreeSet<String> =
        currents.iter().flat_map(|loaded| loaded.view.code_version.capability_host_features.iter().cloned()).collect();
    ComparisonRecord {
        version: COMPARISON_VERSION,
        item: item.to_owned(),
        included_items: included.into_iter().collect(),
        baseline: bases.iter().map(|loaded| loaded.reference.clone()).collect(),
        current: currents.iter().map(|loaded| loaded.reference.clone()).collect(),
        comparability,
        baseline_validity,
        conclusions,
        model: BTreeMap::from([("baseline", model(bases)), ("current", model(currents))]),
        task_set_version: BTreeMap::from([("baseline", versions(bases)), ("current", versions(currents))]),
        tasks,
    }
}

pub fn check_item(item: &str) -> Result<()> {
    ensure!(P2_ITEMS.contains(&item), "item must be one of {}, not `{item}`", P2_ITEMS.join(", "));
    Ok(())
}
