//! Gate runner with stub processes: a failure in the middle never stops the
//! remaining commands, counts are parsed per runner, and a failed JSONL build
//! keeps the Python tests from running against a stale binary.

use std::{
    cell::RefCell,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use lite_eval::{
    common::{sha256_hex, Credentials},
    digest::WORKTREE_DIFF_RULE,
    gate::{run_gate, save_record, CommandOutput, CommandRunner, GatePlan},
    ledger::CodeVersion,
    records::{Count, CountLabel, GATE_COMMANDS},
};

/// Working directory, program, arguments and environment keys of one call.
type Call = (PathBuf, String, Vec<String>, Vec<String>);

struct Stub {
    calls: RefCell<Vec<Call>>,
}

impl CommandRunner for Stub {
    fn run(&self, cwd: &Path, program: &str, args: &[String], env: &[(String, OsString)]) -> CommandOutput {
        if program == "python" {
            let bindings: std::collections::BTreeMap<_, _> =
                env.iter().map(|(key, value)| (key.as_str(), value)).collect();
            let expected = OsString::from(sha256_hex(b"controlled binary bytes"));
            for key in [
                "ARA_JSONL_BUDGET_BINARY_SHA256",
                "ARA_JSONL_GENERATION_BINARY_SHA256",
                "ARA_JSONL_EXPECTED_BINARY_SHA256",
            ] {
                assert_eq!(bindings[key], &expected, "{key} must bind this image");
            }
            assert!(Path::new(bindings["ARA_JSONL_TEST_BINARY"]).is_absolute());
            assert!(Path::new(bindings["ARA_JSONL_LENGTH_EVIDENCE"]).is_dir());
            let protocol = Path::new(bindings["ARA_JSONL_PROTOCOL_EVIDENCE"]);
            assert!(!protocol.exists(), "the protocol suite owns its root creation");
            assert!(protocol.parent().unwrap().is_dir());
        }
        self.calls.borrow_mut().push((
            cwd.to_path_buf(),
            program.to_owned(),
            args.to_vec(),
            env.iter().map(|(key, _)| key.clone()).collect(),
        ));
        let index = self.calls.borrow().len();
        let ok = |stdout: &str| CommandOutput { exit_code: Some(0), stdout: stdout.into(), stderr: Vec::new() };
        match (index, program) {
            // Core clippy fails; everything after it must still run.
            // Code-page bytes (not UTF-8) must be kept exactly.
            (2, _) => {
                CommandOutput { exit_code: Some(101), stdout: Vec::new(), stderr: b"error: \xd6\xd0 unused".to_vec() }
            }
            (_, "cargo") if args[0] == "test" => ok("test result: ok. 7 passed; 0 failed; 0 ignored\n"),
            (_, "cargo") => ok(""),
            (_, "node") => ok("# pass 14\n# fail 0\n"),
            _ => ok("garbled"),
        }
    }
}

fn version() -> CodeVersion {
    CodeVersion {
        head: "56d55d209ecea00e80dfefecdd3e9d47fb62c47a".into(),
        diff_sha256: Some(sha256_hex(b"gate")),
        diff_rule: WORKTREE_DIFF_RULE.into(),
    }
}

#[test]
fn failures_do_not_stop_the_gate_and_every_command_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("ara-agent-lite-jsonl.exe"), b"controlled binary bytes").unwrap();
    let stub = Stub { calls: RefCell::default() };
    let plan = GatePlan {
        repo_root: dir.path(),
        module: "M-P5-QUALITY",
        code_version: version(),
        output_dir: dir.path().join("out"),
        jsonl_binary: Ok(dir.path().join("ara-agent-lite-jsonl.exe")),
        python_evidence: dir.path().join("py"),
    };
    let record = run_gate(&plan, &stub, &Credentials::default()).unwrap();
    assert_eq!(stub.calls.borrow().len(), 11, "all commands run after the clippy failure");
    assert_eq!(record.commands.len(), 11);
    assert_eq!(record.commands[1].exit_code, Some(101));
    assert_eq!(record.commands[2].passed, Count::Value(7));
    assert_eq!(record.commands[0].passed, Count::Label(CountLabel::NotApplicable));
    // Python printed no unittest summary: unparsed, so the command fails.
    assert_eq!(record.commands[9].passed, Count::Label(CountLabel::Unparsed));
    assert_eq!(record.commands[10].passed, Count::Value(14));
    let error = format!("{:#}", record.check_passes().unwrap_err());
    assert!(error.contains("exit code 101") && error.contains("unparsed"), "{error}");
    let stderr = fs::read(record.commands[1].stderr_path.as_ref().unwrap()).unwrap();
    assert_eq!(stderr, b"error: \xd6\xd0 unused", "complete output is kept byte for byte");

    let calls = stub.calls.borrow();
    for ((cwd, program, args, env), id) in calls.iter().zip(&GATE_COMMANDS) {
        assert!(cwd.ends_with(format!("crates/{}", id.crate_name)), "{cwd:?}");
        let rebuilt = format!("{program} {}", args.join(" ")).replace("test_*.py", "\"test_*.py\"");
        assert_eq!(rebuilt, id.command);
        let python = program == "python";
        assert_eq!(env.contains(&"ARA_JSONL_TEST_BINARY".to_owned()), python);
        assert_eq!(env.contains(&"ARA_JSONL_TEST_EVIDENCE".to_owned()), python);
    }
    drop(calls);

    let path = save_record(dir.path(), &record, "20261029T060000Z", &Credentials::default()).unwrap();
    assert!(path.contains("/M-P5-QUALITY-") && path.ends_with("-20261029T060000Z.json"), "{path}");
    assert!(save_record(dir.path(), &record, "20261029T060000Z", &Credentials::default()).is_err(), "no overwrite");
}

#[test]
fn a_failed_jsonl_build_never_runs_python_against_an_old_binary() {
    let dir = tempfile::tempdir().unwrap();
    let stub = Stub { calls: RefCell::default() };
    let plan = GatePlan {
        repo_root: dir.path(),
        module: "M-P5-QUALITY",
        code_version: version(),
        output_dir: dir.path().join("out"),
        jsonl_binary: Err("cargo build exited 101".into()),
        python_evidence: dir.path().join("py"),
    };
    let record = run_gate(&plan, &stub, &Credentials::default()).unwrap();
    assert_eq!(stub.calls.borrow().len(), 10, "python is skipped, the other 10 still run");
    assert!(stub.calls.borrow().iter().all(|(_, program, _, _)| program != "python"));
    assert_eq!(record.commands[9].exit_code, None);
    let stderr = fs::read_to_string(record.commands[9].stderr_path.as_ref().unwrap()).unwrap();
    assert!(stderr.contains("JSONL binary build failed"), "{stderr}");
}
