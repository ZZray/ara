// Feature: ara-lite-core-capability, Property 40: 门禁结论
//! For any mix of quality-command results (exit codes, pass and fail counts,
//! unreadable summaries, a missing toolchain or no exit code) and
//! Support_Matrix test results (passed, failed, nothing executed, skipped,
//! missing, no exit code, unreadable), every command and test has one record
//! with its complete output, and the gate passes only when every command and
//! every referenced test passed; otherwise the failures are named.

use std::{cell::RefCell, collections::VecDeque, ffi::OsString, fs, path::Path};

use lite_eval::{
    common::{sha256_hex, Credentials},
    digest::WORKTREE_DIFF_RULE,
    gate::{
        conclusion,
        matrix::{run_matrix, Matrix, MatrixPlan, TestOutcome},
        run_gate, CommandOutput, CommandRunner, GatePlan,
    },
    ledger::CodeVersion,
    records::{Count, CountLabel, CARGO_TEST, GATE_COMMANDS, NODE_TESTS, PYTHON_TESTS},
};
use proptest::prelude::*;

/// Hands out prepared outputs in call order.
struct Scripted {
    outputs: RefCell<VecDeque<CommandOutput>>,
}

impl CommandRunner for Scripted {
    fn run(&self, _: &Path, _: &str, _: &[String], _: &[(String, OsString)]) -> CommandOutput {
        self.outputs.borrow_mut().pop_front().expect("more commands than prepared outputs")
    }
}

fn version() -> CodeVersion {
    CodeVersion {
        head: "56d55d209ecea00e80dfefecdd3e9d47fb62c47a".into(),
        diff_sha256: Some(sha256_hex(b"gate-props")),
        diff_rule: WORKTREE_DIFF_RULE.into(),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Body {
    Good,
    Failed(u64),
    Unparsed,
}

fn quality_case() -> impl Strategy<Value = (Option<i32>, Body)> {
    let exit = prop_oneof![4 => Just(Some(0)), 1 => Just(Some(1)), 1 => Just(Some(101)), 1 => Just(None)];
    let body = prop_oneof![4 => Just(Body::Good), 1 => (1u64..4).prop_map(Body::Failed), 1 => Just(Body::Unparsed)];
    (exit, body)
}

fn quality_output(command: &str, exit: Option<i32>, body: &Body) -> CommandOutput {
    let text = match (command, body) {
        (CARGO_TEST, Body::Good) => "test result: ok. 4 passed; 0 failed; 0 ignored\n".to_owned(),
        (CARGO_TEST, Body::Failed(n)) => format!("test result: FAILED. 4 passed; {n} failed; 0 ignored\n"),
        (PYTHON_TESTS, Body::Good) => "Ran 4 tests in 1.0s\n\nOK\n".to_owned(),
        (PYTHON_TESTS, Body::Failed(n)) => format!("Ran 4 tests in 1.0s\n\nFAILED (failures={n})\n"),
        (NODE_TESTS, Body::Good) => "# tests 4\n# pass 4\n# fail 0\n".to_owned(),
        (NODE_TESTS, Body::Failed(n)) => format!("# tests 5\n# pass 1\n# fail {n}\n"),
        (_, Body::Unparsed) => "error: could not compile\n".to_owned(),
        _ => String::new(),
    };
    match exit {
        // The toolchain could not be started: no exit code, a reason on stderr.
        None => {
            CommandOutput { exit_code: None, stdout: text.into_bytes(), stderr: b"cannot start: not found".to_vec() }
        }
        Some(code) => {
            CommandOutput { exit_code: Some(code), stdout: text.into_bytes(), stderr: b"\xd6\xd0 log".to_vec() }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Case {
    Pass,
    Fail,
    Zero,
    Skipped,
    Missing,
    NoExit,
    Unparsed,
}

fn case() -> impl Strategy<Value = Case> {
    prop_oneof![
        6 => Just(Case::Pass),
        1 => Just(Case::Fail),
        1 => Just(Case::Zero),
        1 => Just(Case::Skipped),
        1 => Just(Case::Missing),
        1 => Just(Case::NoExit),
        1 => Just(Case::Unparsed),
    ]
}

fn expected(case: Case) -> TestOutcome {
    match case {
        Case::Pass => TestOutcome::Passed,
        Case::Fail => TestOutcome::Failed,
        Case::Zero => TestOutcome::ZeroExecuted,
        Case::Skipped => TestOutcome::Skipped,
        Case::Missing => TestOutcome::Missing,
        Case::NoExit => TestOutcome::NoExitCode,
        Case::Unparsed => TestOutcome::Unparsed,
    }
}

fn test_output(python: bool, case: Case) -> CommandOutput {
    let (code, text): (Option<i32>, &str) = match (python, case) {
        (false, Case::Pass) => (Some(0), "running 1 test\ntest t ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out\n"),
        (false, Case::Fail) => (Some(101), "test t ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored\n"),
        (false, Case::Zero) => (Some(0), "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out\n"),
        (false, Case::Skipped) => (Some(0), "test t ... ignored\n\ntest result: ok. 0 passed; 0 failed; 1 ignored\n"),
        (false, Case::Missing) => (Some(101), "error: no test target named `nope` in `x` package\n"),
        (true, Case::Pass) => (Some(0), "test_1 (m.C.test_1) ... ok\n\nRan 1 test in 0.1s\n\nOK\n"),
        (true, Case::Fail) => (Some(1), "Ran 1 test in 0.1s\n\nFAILED (failures=1)\n"),
        (true, Case::Zero) => (Some(5), "\nRan 0 tests in 0.000s\n\nNO TESTS RAN\n"),
        (true, Case::Skipped) => (Some(0), "Ran 1 test in 0.1s\n\nOK (skipped=1)\n"),
        (true, Case::Missing) => {
            (Some(1), "AttributeError: type object 'C' has no attribute 'test_9'\n\nRan 1 test\n\nFAILED (errors=1)\n")
        }
        (_, Case::NoExit) => (None, ""),
        (_, Case::Unparsed) => (Some(0), "garbled"),
    };
    CommandOutput { exit_code: code, stdout: text.as_bytes().to_vec(), stderr: b"stderr bytes \xff".to_vec() }
}

proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(256) })]

    #[test]
    fn gate_passes_iff_every_command_and_every_matrix_test_passes(
        quality in prop::collection::vec(quality_case(), 11),
        tests in prop::collection::vec((any::<bool>(), case()), 1..6),
        item_without_test in prop::bool::weighted(0.15),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let credentials = Credentials::default();

        let outputs: VecDeque<CommandOutput> = GATE_COMMANDS
            .iter()
            .zip(&quality)
            .map(|(id, (exit, body))| quality_output(id.command, *exit, body))
            .collect();
        let runner = Scripted { outputs: RefCell::new(outputs.clone()) };
        std::fs::write(dir.path().join("ara-agent-lite-jsonl.exe"), b"controlled binary bytes").unwrap();
        let plan = GatePlan {
            repo_root: dir.path(),
            module: "M-P4-MATRIX",
            code_version: version(),
            output_dir: dir.path().join("quality"),
            jsonl_binary: Ok(dir.path().join("ara-agent-lite-jsonl.exe")),
            python_evidence: dir.path().join("py"),
        };
        let gate = run_gate(&plan, &runner, &credentials).unwrap();
        prop_assert!(runner.outputs.borrow().is_empty());
        prop_assert_eq!(gate.commands.len(), 11);
        let quality_error = gate.check_passes().err().map(|error| format!("{error:#}")).unwrap_or_default();
        let mut quality_ok = true;
        for ((id, (exit, body)), (command, output)) in
            GATE_COMMANDS.iter().zip(&quality).zip(gate.commands.iter().zip(&outputs))
        {
            if !id.counts_tests {
                prop_assert_eq!(command.passed, Count::Label(CountLabel::NotApplicable));
                prop_assert_eq!(command.failed, Count::Label(CountLabel::NotApplicable));
            }
            prop_assert_eq!(fs::read(command.stdout_path.as_ref().unwrap()).unwrap(), output.stdout.clone());
            prop_assert_eq!(fs::read(command.stderr_path.as_ref().unwrap()).unwrap(), output.stderr.clone());
            let fails = *exit != Some(0) || (id.counts_tests && *body != Body::Good);
            quality_ok &= !fails;
            let named = quality_error.contains(&format!("{} `{}`", id.crate_name, id.command));
            prop_assert_eq!(named, fails, "{} `{}`: {}", id.crate_name, id.command, quality_error);
        }

        let mut items: Vec<serde_json::Value> = tests
            .iter()
            .enumerate()
            .map(|(index, (python, _))| {
                let test = if *python {
                    serde_json::json!({"runner": "python", "file": format!("crates/j/tests/test_m{index}.py"), "case": format!("C.test_{index}")})
                } else {
                    serde_json::json!({"runner": "cargo", "manifest": "crates/c/Cargo.toml", "target": "lib", "name": format!("m::t{index}")})
                };
                serde_json::json!({"id": format!("cap.{index}"), "lite": {"status": if index % 2 == 0 { "supported" } else { "partial" }, "tests": [test]}})
            })
            .collect();
        items.push(serde_json::json!({"id": "cap.unsupported", "lite": {"status": "unsupported"}}));
        if item_without_test {
            items.push(serde_json::json!({"id": "cap.untested", "lite": {"status": "supported"}}));
        }
        let matrix: Matrix =
            serde_json::from_value(serde_json::json!({"categories": [{"category": "tools", "items": items}]})).unwrap();
        let outputs: VecDeque<CommandOutput> = tests.iter().map(|(python, case)| test_output(*python, *case)).collect();
        let runner = Scripted { outputs: RefCell::new(outputs.clone()) };
        let plan = MatrixPlan {
            repo_root: dir.path(),
            module: "M-P4-MATRIX",
            code_version: version(),
            matrix: &matrix,
            matrix_sha256: sha256_hex(b"matrix"),
            output_dir: dir.path().join("matrix"),
            jsonl_binary: Ok(dir.path().join("ara-agent-lite-jsonl.exe")),
            python_evidence: dir.path().join("py-matrix"),
        };
        let record = run_matrix(&plan, &runner, &credentials).unwrap();
        prop_assert!(runner.outputs.borrow().is_empty(), "every referenced test runs once");
        prop_assert_eq!(record.tests.len(), tests.len() + usize::from(item_without_test));
        prop_assert_eq!(&record.code_version.head, &version().head);

        let error = conclusion(&gate, &record).err().map(|error| format!("{error:#}"));
        let text = error.clone().unwrap_or_default();
        let mut matrix_ok = !item_without_test;
        for (((python, case), result), output) in tests.iter().zip(&record.tests).zip(&outputs) {
            prop_assert_eq!(result.outcome, expected(*case), "{:?} python={}", case, python);
            prop_assert_eq!(fs::read(result.stdout_path.as_ref().unwrap()).unwrap(), output.stdout.clone());
            prop_assert_eq!(fs::read(result.stderr_path.as_ref().unwrap()).unwrap(), output.stderr.clone());
            if result.outcome == TestOutcome::Passed {
                prop_assert!(result.counts.is_some_and(|counts| counts.executed > 0 && counts.failed == 0));
            } else {
                matrix_ok = false;
                prop_assert!(
                    text.contains(&format!("{} `{}`", result.capability, result.test)),
                    "{} {} missing from: {}", result.capability, result.test, text
                );
            }
        }
        if item_without_test {
            let last = record.tests.last().unwrap();
            prop_assert_eq!((last.capability.as_str(), last.outcome), ("cap.untested", TestOutcome::Missing));
            prop_assert!(text.contains("cap.untested"));
        }
        prop_assert!(!text.contains("cap.unsupported"));
        prop_assert_eq!(error.is_none(), quality_ok && matrix_ok, "{}", text);
    }
}
