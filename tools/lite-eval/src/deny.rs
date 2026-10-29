//! Dependency audit against the recorded baseline (requirement 34, module
//! M-P5-DENY). Each of the three Lite manifests is checked with cargo-deny
//! per check (`--format json`), failures are keyed by
//! `(check, crate, version, rule_or_advisory_id)`, and anything not in the
//! baseline is reported as new, never folded into the known items.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    common::{sha256_hex, timestamp_now, write_output_new, Credentials},
    gate::CommandRunner,
    ledger::CodeVersion,
};

pub const CHECKS: [&str; 4] = ["bans", "licenses", "sources", "advisories"];
pub const MANIFESTS: [&str; 3] = ["ara-agent-lite-core", "ara-agent-lite-harness", "ara-agent-lite-jsonl"];
pub const BASELINE_PATH: &str = "crates/ara-agent-lite-core/docs/evidence/lite-capability/deny-baseline.json";
/// Package sets of the baseline lockfiles, captured while they were unchanged.
pub const BASELINE_PACKAGES_PATH: &str =
    "crates/ara-agent-lite-core/docs/evidence/lite-capability/deny-baseline-packages.json";
pub const DENY_DIR: &str = "crates/ara-agent-lite-core/docs/evidence/lite-capability/deny";

/// `(name, version)` pairs of a lockfile set.
pub type Packages = BTreeSet<(String, String)>;

/// Identity of one failure entry (requirement 34.4).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FailureKey {
    pub check: String,
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub version: String,
    pub rule_or_advisory_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Passed,
    Failed,
    /// The check did not complete (cargo-deny missing, advisory database
    /// unavailable, output not understood). Never counted as passed.
    NotExecuted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub manifest: String,
    pub check: String,
    pub status: CheckStatus,
    pub exit_code: Option<i32>,
    pub failures: Vec<FailureKey>,
    pub reason: Option<String>,
    pub started_at: String,
    pub finished_at: String,
    pub stdout_path: String,
    pub stderr_path: String,
}

/// Error diagnostics of one `cargo deny --format json check <check>` run.
/// Returns the keys, or `None` when an error line could not be understood.
pub fn parse_failures(check: &str, stderr: &[u8]) -> Option<Vec<FailureKey>> {
    let mut keys = Vec::new();
    for line in String::from_utf8_lossy(stderr).lines().filter(|line| line.trim_start().starts_with('{')) {
        let value: Value = serde_json::from_str(line).ok()?;
        if value["type"] != "diagnostic" || value["fields"]["severity"] != "error" {
            continue;
        }
        let fields = &value["fields"];
        let krate = &fields["graphs"][0]["Krate"];
        let rule = fields["advisory"]["id"].as_str().or_else(|| fields["code"].as_str())?;
        keys.push(FailureKey {
            check: check.to_owned(),
            crate_name: krate["name"].as_str()?.to_owned(),
            version: krate["version"].as_str()?.to_owned(),
            rule_or_advisory_id: rule.to_owned(),
        });
    }
    Some(keys)
}

/// Status of one check from its exit code and diagnostics (requirement 34.5).
pub fn judge(check: &str, exit_code: Option<i32>, stderr: &[u8]) -> (CheckStatus, Vec<FailureKey>, Option<String>) {
    let Some(code) = exit_code else {
        return (CheckStatus::NotExecuted, Vec::new(), Some("cargo-deny produced no exit code".into()));
    };
    let Some(failures) = parse_failures(check, stderr) else {
        return (CheckStatus::NotExecuted, Vec::new(), Some("an error diagnostic could not be parsed".into()));
    };
    match (code, failures.is_empty()) {
        (0, true) => (CheckStatus::Passed, failures, None),
        (0, false) => (CheckStatus::NotExecuted, Vec::new(), Some("exit 0 with error diagnostics".into())),
        (_, false) => (CheckStatus::Failed, failures, None),
        (code, true) => (
            CheckStatus::NotExecuted,
            Vec::new(),
            Some(format!("exit {code} without error diagnostics (for example, advisory database unavailable)")),
        ),
    }
}

/// `(name, version)` of every `[[package]]` in a Cargo.lock.
pub fn lock_packages(text: &str) -> Packages {
    let mut packages = BTreeSet::new();
    let mut name = None;
    for line in text.lines().map(str::trim) {
        if line == "[[package]]" {
            name = None;
        } else if let Some(value) = line.strip_prefix("name = ") {
            name = Some(value.trim_matches('"').to_owned());
        } else if let (Some(value), Some(current)) = (line.strip_prefix("version = "), name.take()) {
            packages.insert((current, value.trim_matches('"').to_owned()));
        }
    }
    packages
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comparison {
    /// Packages in the current lockfiles but not in the baseline (34.3).
    pub new_dependencies: Vec<(String, String)>,
    /// Failures absent from the baseline (34.4); never listed as known.
    pub new_failures: Vec<FailureKey>,
    /// New failures whose crate is a new dependency (34.3).
    pub new_failures_from_new_dependencies: Vec<FailureKey>,
    pub known_failures: Vec<FailureKey>,
}

/// Property 42: set differences, nothing else.
pub fn compare(
    baseline: &BTreeSet<FailureKey>,
    current: &[FailureKey],
    baseline_packages: &Packages,
    current_packages: &Packages,
) -> Comparison {
    let current: BTreeSet<_> = current.iter().cloned().collect();
    let new_dependencies: Vec<_> = current_packages.difference(baseline_packages).cloned().collect();
    let (known, new): (Vec<_>, Vec<_>) = current.into_iter().partition(|key| baseline.contains(key));
    let new_failures_from_new_dependencies = new
        .iter()
        .filter(|key| new_dependencies.iter().any(|(name, version)| *name == key.crate_name && *version == key.version))
        .cloned()
        .collect();
    Comparison { new_dependencies, new_failures: new, new_failures_from_new_dependencies, known_failures: known }
}

/// Known failure keys of the recorded baseline.
pub fn baseline_failures(path: &Path) -> Result<BTreeSet<FailureKey>> {
    let value: Value =
        serde_json::from_slice(&fs::read(path).with_context(|| format!("cannot read {}", path.display()))?)?;
    let known = value["known_failures"].as_array().context("baseline has no known_failures list")?;
    known
        .iter()
        .map(|item| {
            Ok(FailureKey {
                check: item["check"].as_str().context("check")?.to_owned(),
                crate_name: item["crate"].as_str().context("crate")?.to_owned(),
                version: item["version"].as_str().context("version")?.to_owned(),
                rule_or_advisory_id: item["rule_or_advisory_id"].as_str().context("rule")?.to_owned(),
            })
        })
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DenyRecord {
    pub code_version: CodeVersion,
    pub checks: Vec<CheckResult>,
    pub comparison: Comparison,
    /// Manifests whose baseline package set is unknown (lockfile changed
    /// before the snapshot was taken): their new dependencies are not listed.
    pub package_baseline_missing: Vec<String>,
}

/// Run all 12 checks; outputs go to `out_dir`.
pub fn run_checks(
    repo_root: &Path,
    out_dir: &Path,
    runner: &dyn CommandRunner,
    credentials: &Credentials,
) -> Result<Vec<CheckResult>> {
    fs::create_dir_all(out_dir)?;
    let config = repo_root.join("deny.toml");
    let mut results = Vec::new();
    for manifest in MANIFESTS {
        let manifest_path = repo_root.join("crates").join(manifest).join("Cargo.toml");
        for check in CHECKS {
            let args = [
                "deny".to_owned(),
                "--manifest-path".to_owned(),
                manifest_path.display().to_string(),
                "--format".to_owned(),
                "json".to_owned(),
                "--config".to_owned(),
                config.display().to_string(),
                "check".to_owned(),
                check.to_owned(),
            ];
            let started_at = timestamp_now();
            let output = runner.run(repo_root, "cargo", &args, &[]);
            let finished_at = timestamp_now();
            let save = |suffix: &str, bytes: &[u8]| -> Result<String> {
                let path: PathBuf = out_dir.join(format!("{manifest}.{check}.{suffix}"));
                write_output_new(&path, bytes, credentials)?;
                Ok(path.display().to_string())
            };
            let (status, failures, reason) = judge(check, output.exit_code, &output.stderr);
            results.push(CheckResult {
                manifest: manifest.to_owned(),
                check: check.to_owned(),
                status,
                exit_code: output.exit_code,
                failures,
                reason,
                started_at,
                finished_at,
                stdout_path: save("stdout.txt", &output.stdout)?,
                stderr_path: save("stderr.txt", &output.stderr)?,
            });
        }
    }
    Ok(results)
}

/// Baseline package sets per manifest, captured once while each lockfile
/// still matches the baseline SHA-256 recorded in deny-baseline.json.
pub fn baseline_packages(repo_root: &Path) -> Result<(Packages, Vec<String>)> {
    let snapshot_path = repo_root.join(BASELINE_PACKAGES_PATH);
    let mut snapshot: Value = match fs::read(&snapshot_path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(_) => serde_json::json!({"version": 1, "manifests": {}}),
    };
    let baseline: Value = serde_json::from_slice(&fs::read(repo_root.join(BASELINE_PATH))?)?;
    let mut packages = BTreeSet::new();
    let mut missing = Vec::new();
    let mut changed = false;
    for manifest in MANIFESTS {
        if let Some(list) = snapshot["manifests"][manifest].as_array() {
            packages.extend(
                list.iter().filter_map(|pair| Some((pair[0].as_str()?.to_owned(), pair[1].as_str()?.to_owned()))),
            );
            continue;
        }
        let lock = fs::read(repo_root.join("crates").join(manifest).join("Cargo.lock"))?;
        let recorded = baseline["lockfiles"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["crate"] == manifest))
            .and_then(|item| item["sha256_after"].as_str());
        if recorded == Some(sha256_hex(&lock).as_str()) {
            let set = lock_packages(&String::from_utf8_lossy(&lock));
            snapshot["manifests"][manifest] = serde_json::to_value(&set)?;
            packages.extend(set);
            changed = true;
        } else {
            missing.push(manifest.to_owned());
        }
    }
    if changed {
        snapshot["captured_at"] = Value::String(timestamp_now());
        fs::write(&snapshot_path, serde_json::to_vec_pretty(&snapshot)?)?;
    }
    Ok((packages, missing))
}

/// Package set of the current lockfiles.
pub fn current_packages(repo_root: &Path) -> Result<Packages> {
    let mut packages = BTreeSet::new();
    for manifest in MANIFESTS {
        let lock = fs::read_to_string(repo_root.join("crates").join(manifest).join("Cargo.lock"))?;
        packages.extend(lock_packages(&lock));
    }
    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCES: &str = r#"{"fields":{"code":"source-not-allowed","graphs":[{"Krate":{"name":"rpi-agent","version":"0.3.23"}}],"message":"m","severity":"error"},"type":"diagnostic"}
{"fields":{"code":"duplicate","graphs":[{"Krate":{"name":"syn","version":"2.0.1"}}],"message":"m","severity":"warning"},"type":"diagnostic"}
{"type":"summary","fields":{"errors":1}}"#;

    #[test]
    fn only_error_diagnostics_become_failure_keys() {
        let keys = parse_failures("sources", SOURCES.as_bytes()).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(
            (keys[0].crate_name.as_str(), keys[0].rule_or_advisory_id.as_str()),
            ("rpi-agent", "source-not-allowed")
        );
        let advisory = r#"{"fields":{"advisory":{"id":"RUSTSEC-2024-0001"},"code":"vulnerability","graphs":[{"Krate":{"name":"x","version":"1.0.0"}}],"severity":"error"},"type":"diagnostic"}"#;
        assert_eq!(
            parse_failures("advisories", advisory.as_bytes()).unwrap()[0].rule_or_advisory_id,
            "RUSTSEC-2024-0001"
        );
        assert!(parse_failures("bans", b"{\"type\":\"diagnostic\",\"fields\":{\"severity\":\"error\"}}").is_none());
    }

    #[test]
    fn incomplete_checks_are_never_passed() {
        assert_eq!(judge("bans", Some(0), b"").0, CheckStatus::Passed);
        assert_eq!(judge("sources", Some(8), SOURCES.as_bytes()).0, CheckStatus::Failed);
        assert_eq!(
            judge("advisories", Some(1), b"error: failed to fetch advisory database").0,
            CheckStatus::NotExecuted
        );
        assert_eq!(judge("bans", None, b"").0, CheckStatus::NotExecuted);
        assert_eq!(judge("sources", Some(0), SOURCES.as_bytes()).0, CheckStatus::NotExecuted);
    }

    #[test]
    fn lockfile_packages_are_name_version_pairs() {
        let lock = "version = 4\n\n[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n\n[[package]]\nname = \"b\"\nversion = \"0.2.1\"\nsource = \"x\"\n";
        let packages = lock_packages(lock);
        assert_eq!(packages.len(), 2);
        assert!(packages.contains(&("b".to_owned(), "0.2.1".to_owned())));
    }
}
