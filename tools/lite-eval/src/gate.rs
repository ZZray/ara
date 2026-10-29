//! Delivery quality gate (requirement 33, module M-P5-QUALITY): the 11
//! commands of [`GATE_COMMANDS`], each recorded with its exit code, parsed
//! test counts, times and code version. A failing command never stops the
//! others; its complete output is kept for review.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{ensure, Context, Result};

use crate::{
    common::{timestamp_now, write_json_new, write_output_new, Credentials},
    ledger::CodeVersion,
    records::{
        Count, CountLabel, GateCommand, GateCommandId, GateRecord, CARGO_TEST, CLIPPY, FMT, GATE_COMMANDS, NODE_TESTS,
        PYTHON_TESTS,
    },
};

/// Repository-relative directory of gate records.
pub const GATES_DIR: &str = "crates/ara-agent-lite-core/docs/evidence/lite-capability/gates";

/// Output of one finished or failed-to-start process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutput {
    /// `None`: the program could not be started or produced no exit code.
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Runs one process; replaceable in tests.
pub trait CommandRunner {
    fn run(&self, cwd: &Path, program: &str, args: &[String], env: &[(String, OsString)]) -> CommandOutput;
}

/// Real processes, waiting without a time limit (test suites can be long).
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, cwd: &Path, program: &str, args: &[String], env: &[(String, OsString)]) -> CommandOutput {
        let output = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .envs(env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .output();
        match output {
            Ok(output) => {
                CommandOutput { exit_code: output.status.code(), stdout: output.stdout, stderr: output.stderr }
            }
            Err(error) => CommandOutput {
                exit_code: None,
                stdout: Vec::new(),
                stderr: format!("cannot start {program}: {error}").into_bytes(),
            },
        }
    }
}

/// Program, arguments and working directory (repository-relative) of a command.
pub fn argv(id: &GateCommandId) -> (&'static str, Vec<String>, String) {
    let words = |words: &[&str]| words.iter().map(|word| (*word).to_owned()).collect();
    let (program, args) = match id.command {
        FMT => ("cargo", words(&["fmt", "--check"])),
        CLIPPY => ("cargo", words(&["clippy", "--all-targets", "--", "-D", "warnings"])),
        CARGO_TEST => ("cargo", words(&["test"])),
        PYTHON_TESTS => ("python", words(&["-m", "unittest", "discover", "-s", "tests", "-p", "test_*.py"])),
        NODE_TESTS => ("node", words(&["--test"])),
        other => unreachable!("gate command table has no argv for `{other}`"),
    };
    (program, args, format!("crates/{}", id.crate_name))
}

fn number_after(line: &str, marker: &str) -> Option<u64> {
    let rest = &line[line.find(marker)? + marker.len()..];
    rest.trim_start().split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

fn number_before(line: &str, marker: &str) -> Option<u64> {
    let head = line[..line.find(marker)?].trim_end();
    let digits: String = head.chars().rev().take_while(char::is_ascii_digit).collect();
    digits.chars().rev().collect::<String>().parse().ok()
}

/// Sum of every `test result:` line of `cargo test`.
pub fn parse_cargo(output: &str) -> Option<(u64, u64)> {
    let mut totals = None;
    for line in output.lines().filter(|line| line.trim_start().starts_with("test result:")) {
        let passed = number_before(line, " passed")?;
        let failed = number_before(line, " failed")?;
        let (sum_passed, sum_failed) = totals.unwrap_or((0, 0));
        totals = Some((sum_passed + passed, sum_failed + failed));
    }
    totals
}

/// `Ran N tests` plus `FAILED (failures=a, errors=b)`; passed excludes skips.
pub fn parse_unittest(output: &str) -> Option<(u64, u64)> {
    let ran = output
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("Ran ").and_then(|rest| number_after(rest, "")))?;
    let status =
        output.lines().rev().map(str::trim).find(|line| line.starts_with("OK") || line.starts_with("FAILED"))?;
    let field = |name: &str| number_after(status, &format!("{name}=")).unwrap_or(0);
    let failed = field("failures") + field("errors");
    let passed = ran.checked_sub(failed + field("skipped") + field("expected failures"))?;
    Some((passed, failed))
}

/// `# pass N` / `# fail M` (TAP) or `ℹ pass N` / `ℹ fail M` (spec reporter).
pub fn parse_node(output: &str) -> Option<(u64, u64)> {
    let count = |name: &str| {
        output.lines().rev().find_map(|line| {
            let line = line.trim_start().trim_start_matches(['#', 'ℹ']).trim_start();
            line.strip_prefix(name).and_then(|rest| number_after(rest, ""))
        })
    };
    Some((count("pass ")?, count("fail ")?))
}

/// Counts for a finished command, from its combined output.
pub fn counts(id: &GateCommandId, stdout: &[u8], stderr: &[u8]) -> (Count, Count) {
    if !id.counts_tests {
        return (Count::Label(CountLabel::NotApplicable), Count::Label(CountLabel::NotApplicable));
    }
    let text = format!("{}\n{}", String::from_utf8_lossy(stdout), String::from_utf8_lossy(stderr));
    let parsed = match id.command {
        CARGO_TEST => parse_cargo(&text),
        PYTHON_TESTS => parse_unittest(&text),
        NODE_TESTS => parse_node(&text),
        _ => None,
    };
    match parsed {
        Some((passed, failed)) => (Count::Value(passed), Count::Value(failed)),
        None => (Count::Label(CountLabel::Unparsed), Count::Label(CountLabel::Unparsed)),
    }
}

/// Everything the gate needs that is not a command result.
pub struct GatePlan<'a> {
    pub repo_root: &'a Path,
    pub module: &'a str,
    pub code_version: CodeVersion,
    /// New directory for the complete stdout/stderr of every command.
    pub output_dir: PathBuf,
    /// Environment of the Python tests: the JSONL binary built for this gate
    /// (`Err` with the build failure, so a stale binary is never tested) and
    /// a new evidence directory.
    pub jsonl_binary: std::result::Result<PathBuf, String>,
    pub python_evidence: PathBuf,
}

/// Run all 11 commands in order; a failure never skips the remaining ones.
pub fn run_gate(plan: &GatePlan<'_>, runner: &dyn CommandRunner, credentials: &Credentials) -> Result<GateRecord> {
    fs::create_dir_all(&plan.output_dir)
        .with_context(|| format!("cannot create gate output directory {}", plan.output_dir.display()))?;
    let mut commands = Vec::new();
    for (index, id) in GATE_COMMANDS.iter().enumerate() {
        let (program, args, cwd) = argv(id);
        let started_at = timestamp_now();
        let output = match (&plan.jsonl_binary, id.command == PYTHON_TESTS) {
            (Err(build), true) => CommandOutput {
                exit_code: None,
                stdout: Vec::new(),
                stderr: format!("not run: JSONL binary build failed: {build}").into_bytes(),
            },
            (Ok(binary), true) => {
                let env = vec![
                    ("ARA_JSONL_TEST_BINARY".to_owned(), binary.clone().into_os_string()),
                    ("ARA_JSONL_TEST_EVIDENCE".to_owned(), plan.python_evidence.clone().into_os_string()),
                ];
                runner.run(&plan.repo_root.join(&cwd), program, &args, &env)
            }
            (_, false) => runner.run(&plan.repo_root.join(&cwd), program, &args, &[]),
        };
        let finished_at = timestamp_now();
        let (passed, failed) = counts(id, &output.stdout, &output.stderr);
        let stem = format!("{:02}-{}-{}", index + 1, id.crate_name.replace('/', "_"), program);
        let save = |suffix: &str, bytes: &[u8]| -> Result<String> {
            let path = plan.output_dir.join(format!("{stem}.{suffix}"));
            write_output_new(&path, bytes, credentials)?;
            Ok(path.display().to_string())
        };
        commands.push(GateCommand {
            crate_name: id.crate_name.to_owned(),
            command: id.command.to_owned(),
            exit_code: output.exit_code,
            passed,
            failed,
            started_at,
            finished_at,
            stdout_path: Some(save("stdout.txt", &output.stdout)?),
            stderr_path: Some(save("stderr.txt", &output.stderr)?),
        });
    }
    Ok(GateRecord { module: plan.module.to_owned(), code_version: plan.code_version.clone(), commands })
}

/// Save `record` under [`GATES_DIR`]; returns the repository-relative path.
pub fn save_record(repo_root: &Path, record: &GateRecord, stamp: &str, credentials: &Credentials) -> Result<String> {
    ensure!(record.commands.len() == GATE_COMMANDS.len(), "gate record must hold all 11 commands");
    let diff = record.code_version.diff_sha256.as_deref().map_or("clean", |sha| &sha[..8]);
    let relative = format!("{GATES_DIR}/{}-{diff}-{stamp}.json", record.module);
    let path = repo_root.join(&relative);
    fs::create_dir_all(path.parent().context("gate path has no parent")?)?;
    write_json_new(&path, record, credentials)?;
    Ok(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_counts_are_summed_over_every_test_binary() {
        let text = "running 3 tests\ntest result: ok. 3 passed; 0 failed; 1 ignored; 0 measured\n\
                    test result: FAILED. 10 passed; 2 failed; 0 ignored\n   test result: ok. 0 passed; 0 failed;\n";
        assert_eq!(parse_cargo(text), Some((13, 2)));
        assert_eq!(parse_cargo("error: could not compile"), None);
    }

    #[test]
    fn unittest_counts_handle_ok_failures_errors_and_skips() {
        assert_eq!(parse_unittest("....\n------\nRan 4 tests in 1.2s\n\nOK\n"), Some((4, 0)));
        assert_eq!(parse_unittest("Ran 10 tests in 3s\n\nFAILED (failures=2, errors=1)\n"), Some((7, 3)));
        assert_eq!(parse_unittest("Ran 5 tests in 1s\n\nOK (skipped=2)\n"), Some((3, 0)));
        assert_eq!(parse_unittest("Traceback: ImportError\n"), None);
        assert_eq!(parse_unittest("Ran 3 tests\n"), None, "a run without a final status is unparsed");
    }

    #[test]
    fn node_counts_accept_tap_and_spec_reporters() {
        assert_eq!(parse_node("# tests 14\n# pass 13\n# fail 1\n"), Some((13, 1)));
        assert_eq!(parse_node("ℹ tests 14\nℹ pass 14\nℹ fail 0\n"), Some((14, 0)));
        assert_eq!(parse_node("SyntaxError"), None);
    }

    #[test]
    fn argv_covers_every_gate_command() {
        for id in &GATE_COMMANDS {
            let (program, args, cwd) = argv(id);
            assert!(cwd.starts_with("crates/ara-agent-lite-"), "{cwd}");
            assert_eq!(format!("{program} {}", args.join(" ")).replace("test_*.py", "\"test_*.py\""), id.command);
        }
    }
}
