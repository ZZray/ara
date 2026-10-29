//! Baseline records (requirement 7; design “基线”).
//!
//! `lite-eval run --baseline` writes its run under
//! `<evidence_root>/baseline/<manifest_id>/` and, in the repository,
//! `baseline/<manifest_id>.json` with the manifest summary and the SHA-256 of
//! every file of that run. Every later run re-verifies every registered
//! baseline (7.4). A baseline run in which some task of the Task_Set has no
//! Oracle judgement is an incomplete baseline (7.6), recorded but not usable.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    common::{sha256_file, write_json_new, Credentials},
    manifest::{ManifestStatus, RunManifest, TaskState},
};

/// Repository-relative directory of the baseline indexes.
pub const BASELINE_INDEX_DIR: &str = "crates/ara-agent-lite-core/docs/evidence/lite-capability/baseline";
pub const BASELINE_INDEX_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingJudgement {
    pub task_id: String,
    pub reason: String,
}

/// What a `--baseline` Run_Manifest says about itself.
#[derive(Clone, Debug, Serialize)]
pub struct BaselineMark {
    /// Repository-relative path of the index written after the manifest.
    pub index: String,
    /// Whether this run can serve as a Baseline_Record.
    pub usable: bool,
    pub missing_judgement: Vec<MissingJudgement>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Relative to the run directory, `/`-separated.
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineStatus {
    Baseline,
    IncompleteBaseline,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BaselineIndex {
    pub version: u32,
    pub manifest_id: String,
    pub status: BaselineStatus,
    pub run_dir: PathBuf,
    pub manifest: FileEntry,
    /// Manifest `code_version`, with whether the saved diff matches its SHA-256 (7.5).
    pub code_version: Value,
    pub diff_matches_manifest: Option<bool>,
    pub summary: Value,
    pub tasks: Vec<Value>,
    pub missing_judgement: Vec<MissingJudgement>,
    /// Every file of the run directory, sorted by path.
    pub files: Vec<FileEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Intact,
    Changed,
    Unreadable,
}

/// Re-verification of one registered baseline (7.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineCheck {
    pub index: PathBuf,
    pub status: CheckStatus,
    pub problems: Vec<String>,
}

/// Tasks of the Task_Set without an Oracle judgement in `manifest` (7.6).
/// A task the Oracle judged failed is a valid baseline result.
pub fn missing_judgement(manifest: &RunManifest) -> Vec<MissingJudgement> {
    let whole_run = match manifest.status {
        ManifestStatus::EnvironmentUnavailable => Some("environment unavailable: the probe failed"),
        ManifestStatus::TaskSetInvalid => Some("Task_Set invalid: no task ran"),
        ManifestStatus::ProbeOnly => Some("probe-only run: no task ran"),
        ManifestStatus::Completed | ManifestStatus::IncompleteBaseline => None,
    };
    manifest
        .all_tasks
        .iter()
        .filter_map(|task| {
            let task_id = task.task_id.clone();
            let reason = if let Some(reason) = whole_run {
                reason.to_owned()
            } else {
                let record = manifest.selected.iter().find(|record| record.task_id == task_id);
                match record {
                    None => "not run (not selected, or the run was interrupted)".to_owned(),
                    Some(record) if record.state == TaskState::Refused => {
                        "refused: the fixture SHA-256 differed or was unreadable".to_owned()
                    }
                    Some(record) if record.state == TaskState::FailureRecordMissing => format!(
                        "Failure_Record missing: {}",
                        record.failure_record_error.as_deref().unwrap_or("not written")
                    ),
                    Some(record) => match &record.metrics {
                        Some(metrics) if metrics.correctness.known => return None,
                        Some(metrics) => format!(
                            "no Oracle judgement: {}",
                            metrics.correctness.reason.as_deref().unwrap_or("unknown")
                        ),
                        None => "no task result was written".to_owned(),
                    },
                }
            };
            Some(MissingJudgement { task_id, reason })
        })
        .collect()
}

/// Every regular file under `dir`, relative and sorted; links and other
/// entries are listed with `sha256 = "not_a_regular_file"`.
pub fn file_entries(dir: &Path) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        for item in fs::read_dir(&next).with_context(|| format!("cannot list {}", next.display()))? {
            let path = item?.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            let relative = path.strip_prefix(dir).expect("under dir");
            let relative = relative.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>();
            let sha256 = if metadata.is_file() { sha256_file(&path)? } else { "not_a_regular_file".to_owned() };
            entries.push(FileEntry { path: relative.join("/"), sha256, bytes: metadata.len() });
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// Repository-relative path of the index of `manifest_id`.
pub fn index_path(manifest_id: &str) -> String {
    format!("{BASELINE_INDEX_DIR}/{manifest_id}.json")
}

/// Write the in-repo index of a finished `--baseline` run (never overwrites).
pub fn write_index(
    repo_root: &Path,
    run_dir: &Path,
    manifest: &RunManifest,
    manifest_path: &Path,
    credentials: &Credentials,
) -> Result<PathBuf> {
    let mark = manifest.baseline.as_ref().context("not a baseline run")?;
    let files = file_entries(run_dir)?;
    let manifest_entry = files.iter().find(|entry| entry.path == "manifest.json").cloned();
    let manifest_entry = manifest_entry.with_context(|| format!("{} was not written", manifest_path.display()))?;
    let diff_matches_manifest = manifest
        .code_version
        .diff_path
        .as_ref()
        .map(|path| sha256_file(path).ok() == manifest.code_version.worktree_diff_sha256);
    let index = BaselineIndex {
        version: BASELINE_INDEX_VERSION,
        manifest_id: manifest.manifest_id.clone(),
        status: if mark.usable { BaselineStatus::Baseline } else { BaselineStatus::IncompleteBaseline },
        run_dir: run_dir.to_owned(),
        manifest: manifest_entry,
        code_version: serde_json::to_value(&manifest.code_version)?,
        diff_matches_manifest,
        summary: serde_json::to_value(&manifest.summary)?,
        tasks: manifest
            .selected
            .iter()
            .map(|record| serde_json::json!({"task_id": record.task_id, "state": record.state}))
            .collect(),
        missing_judgement: mark.missing_judgement.clone(),
        files,
    };
    let path = repo_root.join(&mark.index);
    fs::create_dir_all(path.parent().expect("has parent"))?;
    write_json_new(&path, &index, credentials)?;
    Ok(path)
}

/// Re-verify one index: every listed file still has its SHA-256 and no file
/// was added to the run directory.
pub fn verify(index_path: &Path) -> BaselineCheck {
    let unreadable = |problem: String| BaselineCheck {
        index: index_path.to_owned(),
        status: CheckStatus::Unreadable,
        problems: vec![problem],
    };
    let index = match fs::read(index_path)
        .map_err(anyhow::Error::from)
        .and_then(|bytes| serde_json::from_slice::<BaselineIndex>(&bytes).map_err(anyhow::Error::from))
    {
        Ok(index) => index,
        Err(error) => return unreadable(format!("cannot read baseline index: {error:#}")),
    };
    let current = match file_entries(&index.run_dir) {
        Ok(current) => current,
        Err(error) => return unreadable(format!("cannot list {}: {error:#}", index.run_dir.display())),
    };
    let mut problems = Vec::new();
    for entry in &index.files {
        match current.iter().find(|file| file.path == entry.path) {
            None => problems.push(format!("{} removed", entry.path)),
            Some(file) if file.sha256 != entry.sha256 => problems.push(format!("{} changed", entry.path)),
            Some(_) => {}
        }
    }
    for file in &current {
        if !index.files.iter().any(|entry| entry.path == file.path) {
            problems.push(format!("{} added", file.path));
        }
    }
    let status = if problems.is_empty() { CheckStatus::Intact } else { CheckStatus::Changed };
    BaselineCheck { index: index_path.to_owned(), status, problems }
}

/// [`verify`] every index under the repository's baseline directory.
pub fn verify_all(repo_root: &Path) -> Vec<BaselineCheck> {
    let dir = repo_root.join(BASELINE_INDEX_DIR);
    let mut indexes: Vec<PathBuf> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(|item| item.ok().map(|item| item.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .collect();
    indexes.sort();
    indexes.iter().map(|path| verify(path)).collect()
}
