use super::worktree::{self, BranchResult, Delta, NestedPatch, WorktreeBaseline};
use anyhow::{Context, Result, bail};
use ara_task_vcs::{git::GitRepo, types::ApplyOptions};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NestedPatchArtifact {
    pub relative_path: PathBuf,
    pub patch_path: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaptureReceipt {
    pub resource_id: String,
    pub capture_id: String,
    pub fingerprint: String,
    pub patch_path: PathBuf,
    pub nested_patch_paths: Vec<NestedPatchArtifact>,
    pub branch: Option<BranchResult>,
    pub changed: bool,
    pub merge_mode: String,
    pub durable_receipt: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApplyReceipt {
    pub resource_id: String,
    pub capture_id: String,
    pub status: String,
    pub changes_applied: bool,
    pub had_any_changes: bool,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    pub durable_receipt: PathBuf,
}

pub(crate) fn cancelled_before_apply(
    resource: &str,
    receipt: &CaptureReceipt,
) -> Result<ApplyReceipt> {
    let result = ApplyReceipt {
        resource_id: resource.to_owned(),
        capture_id: receipt.capture_id.clone(),
        status: "cancelled_before_mutation".into(),
        changes_applied: false,
        had_any_changes: false,
        warnings: Vec::new(),
        error: Some(
            "Task cancelled before parent apply admission; no parent mutation started".into(),
        ),
        durable_receipt: receipt.durable_receipt.with_file_name("apply.json"),
    };
    write_new(
        &result.durable_receipt,
        &serde_json::to_vec_pretty(&result)?,
    )?;
    Ok(result)
}

pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create durable artifact {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
pub(crate) fn fingerprint(delta: &Delta) -> String {
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    fn field(context: &mut ring::digest::Context, value: &[u8]) {
        context.update(&(value.len() as u64).to_le_bytes());
        context.update(value);
    }
    field(&mut context, delta.root_patch.as_bytes());
    for patch in &delta.nested_patches {
        field(
            &mut context,
            patch.relative_path.to_string_lossy().as_bytes(),
        );
        field(&mut context, patch.patch.as_bytes());
    }
    context
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub(crate) fn persist_capture(
    copy: &Path,
    baseline: &WorktreeBaseline,
    artifacts: &Path,
    task: &str,
    resource: &str,
    merge_mode: &str,
    description: Option<&str>,
    previous: Option<&CaptureReceipt>,
    final_release: bool,
    scratch: &Path,
) -> Result<CaptureReceipt> {
    let delta = worktree::capture_delta(copy, baseline, scratch)?;
    let fingerprint = fingerprint(&delta);
    if let Some(previous) = previous.filter(|p| p.fingerprint == fingerprint) {
        return Ok(previous.clone());
    }
    let capture_id = uuid::Uuid::new_v4().simple().to_string();
    let dir = artifacts.join(resource).join(&capture_id);
    fs::create_dir_all(&dir)?;
    let patch_path = dir.join("root.patch");
    write_new(&patch_path, delta.root_patch.as_bytes())?;
    let mut nested_patch_paths = Vec::new();
    for (i, patch) in delta.nested_patches.iter().enumerate() {
        let patch_path = dir.join(format!("nested-{i}.patch"));
        write_new(&patch_path, patch.patch.as_bytes())?;
        nested_patch_paths.push(NestedPatchArtifact {
            relative_path: patch.relative_path.clone(),
            patch_path,
        });
    }
    // Patch and nested durability precede branch publication. A branch failure cannot lose patches.
    let branch = if merge_mode == "branch" || final_release {
        Some(worktree::commit_to_branch(
            copy,
            baseline,
            task,
            resource,
            description,
            &delta,
            scratch,
        )?)
    } else {
        None
    };
    let receipt = CaptureReceipt {
        resource_id: resource.into(),
        capture_id,
        fingerprint,
        patch_path,
        nested_patch_paths,
        branch,
        changed: !delta.root_patch.trim().is_empty() || !delta.nested_patches.is_empty(),
        merge_mode: merge_mode.into(),
        durable_receipt: dir.join("capture.json"),
    };
    write_new(
        &receipt.durable_receipt,
        &serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(receipt)
}

fn read_nested(receipt: &CaptureReceipt) -> Result<Vec<NestedPatch>> {
    receipt
        .nested_patch_paths
        .iter()
        .map(|patch| {
            worktree::safe_relative(&patch.relative_path)?;
            Ok(NestedPatch {
                relative_path: patch.relative_path.clone(),
                patch: fs::read_to_string(&patch.patch_path)?,
            })
        })
        .collect()
}

/// Explicit run-end integration only. Caller holds both lease gate and canonical parent gate.
pub(crate) fn apply(root: &Path, resource: &str, receipt: &CaptureReceipt) -> Result<ApplyReceipt> {
    if receipt.resource_id != resource {
        bail!("capture belongs to a different isolation resource");
    }
    let repo = GitRepo::require(root)?;
    let mut result = ApplyReceipt {
        resource_id: resource.into(),
        capture_id: receipt.capture_id.clone(),
        status: "running".into(),
        changes_applied: false,
        had_any_changes: false,
        warnings: Vec::new(),
        error: None,
        durable_receipt: receipt.durable_receipt.with_file_name("apply.json"),
    };
    write_new(
        &receipt.durable_receipt.with_file_name("apply-started.json"),
        &serde_json::to_vec_pretty(
            &serde_json::json!({"resource_id":resource,"capture_id":receipt.capture_id,"status":"running","rule":"unknown effects must never be replayed"}),
        )?,
    )?;
    let operation = (|| -> Result<()> {
        if receipt.merge_mode == "branch" {
            if let Some(branch) = &receipt.branch {
                if let Some(name) = &branch.branch_name {
                    // Ref may have changed externally since capture: never apply a successor's branch.
                    if repo.resolve_ref(&format!("refs/heads/{name}"))? != branch.tip {
                        bail!(
                            "captured branch tip changed before apply; recovery ref {:?}",
                            branch.rescue_ref
                        );
                    }
                    for revision in repo.rev_list_range(&branch.base_sha, name)? {
                        match repo.cherry_pick(&revision) {
                            Ok(()) => result.had_any_changes = true,
                            Err(ara_task_vcs::Error::EmptyCherryPick { .. }) => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                }
            }
        } else {
            let patch = fs::read_to_string(&receipt.patch_path)?;
            if !patch.trim().is_empty() {
                let normalized = if patch.ends_with('\n') {
                    patch
                } else {
                    format!("{patch}\n")
                };
                let reverse = repo
                    .can_apply_patch(
                        &normalized,
                        &ApplyOptions {
                            reverse: true,
                            ..Default::default()
                        },
                    )
                    .unwrap_or(false);
                let forward = repo
                    .can_apply_patch(&normalized, &ApplyOptions::default())
                    .unwrap_or(false);
                if !(reverse && !forward) {
                    if !forward {
                        bail!(
                            "root patch does not apply; artifact {}",
                            receipt.patch_path.display()
                        );
                    }
                    repo.apply_patch(&normalized, &ApplyOptions::default())?;
                    result.had_any_changes = true;
                }
            }
        }
        // Nested patches are attempted only after root integration succeeded, including nested-only output.
        let nested = read_nested(receipt)?;
        result.had_any_changes |= !nested.is_empty();
        result
            .warnings
            .extend(worktree::apply_nested(root, &nested, resource)?);
        if receipt.merge_mode == "branch" {
            if let Some(branch) = &receipt.branch {
                if !worktree::cleanup_branch(root, resource, branch)? {
                    result
                        .warnings
                        .push("Task branch ownership/tip changed; branch retained".into());
                }
            }
        }
        Ok(())
    })();
    match operation {
        Ok(()) => {
            result.changes_applied = true;
            result.status = "applied".into();
        }
        Err(error) => {
            result.error = Some(format!("{error:#}"));
            result.status = "failed".into();
        }
    }
    // An existing apply receipt means this effect has already been attempted. Never overwrite/replay it.
    write_new(
        &result.durable_receipt,
        &serde_json::to_vec_pretty(&result)?,
    )?;
    Ok(result)
}
