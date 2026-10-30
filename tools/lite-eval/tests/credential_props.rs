//! Property 10: the credential never appears in any output (requirement 3.6).
//!
//! Each case runs the real `lite-eval run` binary in a throw-away repository
//! whose Run_Policy names a generated credential variable and points at a
//! controlled upstream. Two tasks run through the real Capability_Test_Host,
//! Harness and Core: one passes its Oracle, one fails it. The credential
//! value must reach the upstream as the bearer token and must occur zero
//! times in the Run_Manifest, every file under the run directory, stdout
//! and stderr.

mod runner_support;

use std::process::Command;

use proptest::prelude::*;
use runner_support::{files, Provider, TestRepo};

const VARIABLE: &str = "LITE_EVAL_CREDENTIAL_PROPS_KEY";

fn occurs(haystack: &[u8], needle: &[u8]) -> usize {
    haystack.windows(needle.len()).filter(|window| *window == needle).count()
}

#[test]
fn the_credential_occurs_nowhere_in_the_run_outputs() {
    let provider = Provider::standard();
    // S1 expects the scripted answer (pass); S2 expects another (fail).
    let repo =
        TestRepo::new(&provider.endpoint, VARIABLE, &|id| if id == "S2" { "other".into() } else { "correct".into() });
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig::with_cases(256));
    let result = runner.run(&"[A-Za-z0-9_-]{24,48}", |token| {
        let evidence = tempfile::tempdir().unwrap();
        let before = provider.authorizations.lock().unwrap().len();
        let output = Command::new(env!("CARGO_BIN_EXE_lite-eval"))
            .current_dir(&repo.root)
            .args(["run", "--tasks", "S1,S2", "--evidence-root"])
            .arg(evidence.path())
            .env(VARIABLE, &token)
            .output()
            .unwrap();
        prop_assert_eq!(
            output.status.code(),
            Some(1),
            "S2 fails its Oracle: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bearer = format!("Bearer {token}");
        let seen = provider.authorizations.lock().unwrap()[before..].iter().filter(|value| **value == bearer).count();
        prop_assert!(seen >= 3, "probe, catalog and model calls carry the credential ({seen})");
        let needle = token.as_bytes();
        prop_assert_eq!(occurs(&output.stdout, needle), 0, "stdout");
        prop_assert_eq!(occurs(&output.stderr, needle), 0, "stderr");
        let all = files(evidence.path());
        prop_assert!(all.iter().any(|path| path.ends_with("manifest.json")));
        prop_assert!(all.iter().any(|path| path.ends_with("session.json")), "the Core Session was written");
        for path in all {
            let bytes = std::fs::read(&path).unwrap();
            prop_assert_eq!(occurs(&bytes, needle), 0, "{}", path.display());
        }
        let manifest_path = String::from_utf8_lossy(&output.stdout).lines().next().unwrap_or_default().to_owned();
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(manifest_path.trim()).unwrap()).unwrap();
        let states: Vec<_> = manifest["selected"].as_array().unwrap().iter().map(|r| r["state"].clone()).collect();
        prop_assert_eq!(states, vec![serde_json::json!("passed"), serde_json::json!("failed")]);
        Ok(())
    });
    if let Err(error) = result {
        panic!("{error}");
    }
}
