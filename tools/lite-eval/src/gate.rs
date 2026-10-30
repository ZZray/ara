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

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    common::{sha256_file, timestamp_now, write_json_new, write_output_new, Credentials},
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

/// Bind every native Python suite to this build and fresh family-specific evidence.
fn run_python(
    runner: &dyn CommandRunner,
    cwd: &Path,
    args: &[String],
    binary: &Path,
    evidence: &Path,
) -> CommandOutput {
    let environment = (|| -> Result<Vec<(String, OsString)>> {
        let binary = crate::workspace::canonical(binary)?;
        let sha = sha256_file(&binary)?;
        // Length tests create method directories; protocol tests create their own root.
        fs::create_dir_all(evidence.join("length"))?;
        let mut env = vec![("ARA_JSONL_TEST_BINARY".into(), binary.into_os_string())];
        for key in
            ["ARA_JSONL_BUDGET_BINARY_SHA256", "ARA_JSONL_GENERATION_BINARY_SHA256", "ARA_JSONL_EXPECTED_BINARY_SHA256"]
        {
            env.push((key.into(), OsString::from(&sha)));
        }
        env.push(("ARA_JSONL_TEST_EVIDENCE".into(), evidence.as_os_str().to_owned()));
        for family in ["BUDGET", "GENERATION", "COMPACTION", "LENGTH", "PROTOCOL", "RELAY"] {
            env.push((
                format!("ARA_JSONL_{family}_EVIDENCE"),
                evidence.join(family.to_ascii_lowercase()).into_os_string(),
            ));
        }
        Ok(env)
    })();
    match environment {
        Ok(env) => runner.run(cwd, "python", args, &env),
        Err(error) => CommandOutput {
            exit_code: None,
            stdout: Vec::new(),
            stderr: format!("cannot prepare Python test binding: {error:#}").into_bytes(),
        },
    }
}

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
            (Ok(binary), true) => run_python(runner, &plan.repo_root.join(&cwd), &args, binary, &plan.python_evidence),
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

/// Save the record of one optional gate part (`matrix`, `stability-demo`,
/// `consumer`) under [`GATES_DIR`] as `<module>-<part>-<diff>-<stamp>.json`.
/// These files are separate from the quality-gate record the ledger reads.
pub fn save_part(
    repo_root: &Path,
    module: &str,
    part: &str,
    code_version: &CodeVersion,
    stamp: &str,
    record: &impl Serialize,
    credentials: &Credentials,
) -> Result<String> {
    let diff = code_version.diff_sha256.as_deref().map_or("clean", |sha| &sha[..8]);
    let relative = format!("{GATES_DIR}/{module}-{part}-{diff}-{stamp}.json");
    let path = repo_root.join(&relative);
    fs::create_dir_all(path.parent().context("gate path has no parent")?)?;
    write_json_new(&path, record, credentials)?;
    Ok(relative)
}

/// Gate conclusion over the quality commands and the Support_Matrix tests
/// (requirements 30.5, 33.2, 33.3): passes only when both parts pass; the
/// error lists every failing command and every failing capability and test.
pub fn conclusion(quality: &GateRecord, matrix: &matrix::MatrixRecord) -> Result<()> {
    let problems: Vec<String> = [quality.check_passes(), matrix.check_passes()]
        .into_iter()
        .filter_map(Result::err)
        .map(|e| format!("{e:#}"))
        .collect();
    if !problems.is_empty() {
        bail!("{}", problems.join("\n"));
    }
    Ok(())
}

/// Save stdout and stderr of one process as `<stem>.stdout.txt` / `.stderr.txt`.
fn save_output(dir: &Path, stem: &str, output: &CommandOutput, credentials: &Credentials) -> Result<(String, String)> {
    let save = |suffix: &str, bytes: &[u8]| -> Result<String> {
        let path = dir.join(format!("{stem}.{suffix}"));
        write_output_new(&path, bytes, credentials)?;
        Ok(path.display().to_string())
    };
    Ok((save("stdout.txt", &output.stdout)?, save("stderr.txt", &output.stderr)?))
}

/// Support_Matrix tests (requirement 30.4, 30.5, task 36.2).
pub mod matrix {
    use super::*;

    /// Repository-relative machine-readable Support_Matrix.
    pub const MATRIX_PATH: &str = "crates/ara-agent-lite-core/docs/support-matrix.json";

    #[derive(Clone, Debug, Deserialize)]
    pub struct Matrix {
        pub categories: Vec<Category>,
    }

    #[derive(Clone, Debug, Deserialize)]
    pub struct Category {
        pub category: String,
        pub items: Vec<Item>,
    }

    #[derive(Clone, Debug, Deserialize)]
    pub struct Item {
        pub id: String,
        pub lite: Lite,
    }

    #[derive(Clone, Debug, Deserialize)]
    pub struct Lite {
        /// `supported`, `partial` or `unsupported`.
        pub status: String,
        #[serde(default)]
        pub tests: Vec<TestRef>,
    }

    /// One referenced Lite test.
    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "runner", rename_all = "lowercase")]
    pub enum TestRef {
        /// `target` is `lib` for unit tests, else an integration-test target.
        Cargo { manifest: String, target: String, name: String },
        /// `case` is `Class.method` in the module named by the file stem.
        Python { file: String, case: String },
        /// `case` is the full test name.
        Node { file: String, case: String },
    }

    impl TestRef {
        pub fn runner(&self) -> &'static str {
            match self {
                TestRef::Cargo { .. } => "cargo",
                TestRef::Python { .. } => "python",
                TestRef::Node { .. } => "node",
            }
        }

        /// Test name as written in the record: location, target and name.
        pub fn label(&self) -> String {
            match self {
                TestRef::Cargo { manifest, target, name } => format!("{manifest} [{target}] {name}"),
                TestRef::Python { file, case } | TestRef::Node { file, case } => format!("{file} {case}"),
            }
        }

        /// Program, arguments and repository-relative working directory.
        pub fn argv(&self) -> (&'static str, Vec<String>, String) {
            match self {
                TestRef::Cargo { manifest, target, name } => {
                    let mut args = vec!["test".to_owned(), "--manifest-path".to_owned(), manifest.clone()];
                    if target == "lib" {
                        args.push("--lib".to_owned());
                    } else {
                        args.extend(["--test".to_owned(), target.clone()]);
                    }
                    args.extend([name.clone(), "--".to_owned(), "--exact".to_owned()]);
                    ("cargo", args, ".".to_owned())
                }
                TestRef::Python { file, case } => {
                    let (dir, stem) = split_file(file, ".py");
                    ("python", vec!["-m".into(), "unittest".into(), "-v".into(), format!("{stem}.{case}")], dir)
                }
                TestRef::Node { file, case } => {
                    let (dir, _) = split_file(file, ".js");
                    let escaped: String = case
                        .chars()
                        .flat_map(|c| {
                            let special = "\\^$.|?*+()[]{}/".contains(c);
                            special.then_some('\\').into_iter().chain([c])
                        })
                        .collect();
                    let name = file.rsplit('/').next().unwrap_or(file).to_owned();
                    ("node", vec!["--test".into(), format!("--test-name-pattern=^{escaped}$"), name], dir)
                }
            }
        }
    }

    fn split_file(file: &str, extension: &str) -> (String, String) {
        let (dir, name) = file.rsplit_once('/').unwrap_or((".", file));
        (dir.to_owned(), name.strip_suffix(extension).unwrap_or(name).to_owned())
    }

    /// Every test the Lite side must run: each `supported` or `partial`
    /// item's tests in document order; such an item without tests yields one
    /// `None` entry so the gate records it as missing.
    pub fn referenced(matrix: &Matrix) -> Result<Vec<(String, Option<TestRef>)>> {
        let mut out = Vec::new();
        for item in matrix.categories.iter().flat_map(|category| &category.items) {
            match item.lite.status.as_str() {
                "supported" | "partial" if item.lite.tests.is_empty() => out.push((item.id.clone(), None)),
                "supported" | "partial" => {
                    out.extend(item.lite.tests.iter().map(|test| (item.id.clone(), Some(test.clone()))))
                }
                "unsupported" => {}
                other => bail!("support matrix item {} has unknown Lite status `{other}`", item.id),
            }
        }
        Ok(out)
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum TestOutcome {
        Passed,
        Failed,
        /// Ignored by cargo or skipped by unittest / node.
        Skipped,
        /// The selector ran nothing (for cargo `--exact`: the name does not exist).
        ZeroExecuted,
        /// The target, module, file or test case does not exist, or the item lists no test.
        Missing,
        /// The runner could not start or was killed before exiting.
        NoExitCode,
        /// Exit 0 but no summary could be read.
        Unparsed,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct TestCounts {
        pub executed: u64,
        pub passed: u64,
        pub failed: u64,
        pub skipped: u64,
    }

    /// Sum of every `test result:` line; ignored tests are not executed.
    fn cargo_counts(text: &str) -> Option<TestCounts> {
        let mut total: Option<TestCounts> = None;
        for line in text.lines().filter(|line| line.trim_start().starts_with("test result:")) {
            let passed = number_before(line, " passed")?;
            let failed = number_before(line, " failed")?;
            let skipped = number_before(line, " ignored").unwrap_or(0);
            let sum = total.get_or_insert(TestCounts { executed: 0, passed: 0, failed: 0, skipped: 0 });
            sum.passed += passed;
            sum.failed += failed;
            sum.skipped += skipped;
            sum.executed += passed + failed;
        }
        total
    }

    /// `Ran N tests` and the final `OK` / `FAILED (...)` line (none needed for 0 tests).
    fn unittest_counts(text: &str) -> Option<TestCounts> {
        let ran = text
            .lines()
            .find_map(|line| line.trim_start().strip_prefix("Ran ").and_then(|rest| number_after(rest, "")))?;
        let status =
            text.lines().rev().map(str::trim).find(|line| line.starts_with("OK") || line.starts_with("FAILED"));
        let status = match status {
            Some(status) => status,
            None if ran == 0 => "",
            None => return None,
        };
        let field = |name: &str| number_after(status, &format!("{name}=")).unwrap_or(0);
        let skipped = field("skipped");
        let failed = field("failures") + field("errors") + field("unexpected successes");
        let executed = ran.checked_sub(skipped)?;
        Some(TestCounts { executed, passed: executed.checked_sub(failed)?, failed, skipped })
    }

    /// `# pass N`, `# fail N`, `# skipped N`, `# todo N`, `# cancelled N` (TAP or spec reporter).
    fn node_counts(text: &str) -> Option<TestCounts> {
        let count = |name: &str| {
            text.lines().rev().find_map(|line| {
                let line = line.trim_start().trim_start_matches(['#', 'ℹ']).trim_start();
                line.strip_prefix(name).and_then(|rest| number_after(rest, ""))
            })
        };
        let passed = count("pass ")?;
        let failed = count("fail ")? + count("cancelled ").unwrap_or(0);
        let skipped = count("skipped ").unwrap_or(0) + count("todo ").unwrap_or(0);
        Some(TestCounts { executed: passed + failed, passed, failed, skipped })
    }

    const MISSING_MARKERS: [&str; 9] = [
        "no test target named",
        "no library targets found",
        "does not exist",
        "has no attribute",
        "No module named",
        "ModuleNotFoundError",
        "Failed to import test module",
        "Cannot find module",
        "Could not find",
    ];

    /// Outcome of one finished test command, in this order: no exit code,
    /// missing target or case, unreadable summary (a failure when the exit
    /// code is not 0), failed tests, skipped tests, nothing executed, a
    /// non-zero exit code, and only then passed.
    pub fn judge(test: &TestRef, output: &CommandOutput) -> (Option<TestCounts>, TestOutcome) {
        let Some(exit) = output.exit_code else { return (None, TestOutcome::NoExitCode) };
        let text = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        let counts = match test {
            TestRef::Cargo { .. } => cargo_counts(&text),
            TestRef::Python { .. } => unittest_counts(&text),
            TestRef::Node { .. } => node_counts(&text),
        };
        if MISSING_MARKERS.iter().any(|marker| text.contains(marker)) {
            return (counts, TestOutcome::Missing);
        }
        let Some(found) = counts else {
            return (None, if exit == 0 { TestOutcome::Unparsed } else { TestOutcome::Failed });
        };
        let outcome = if found.failed > 0 {
            TestOutcome::Failed
        } else if found.skipped > 0 {
            TestOutcome::Skipped
        } else if found.executed == 0 {
            TestOutcome::ZeroExecuted
        } else if exit != 0 {
            TestOutcome::Failed
        } else {
            TestOutcome::Passed
        };
        (counts, outcome)
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct MatrixTestResult {
        /// Support_Matrix item ID.
        pub capability: String,
        /// `cargo`, `python`, `node`, or `none` for an item without tests.
        pub runner: String,
        pub test: String,
        pub command: String,
        pub exit_code: Option<i32>,
        pub counts: Option<TestCounts>,
        pub outcome: TestOutcome,
        pub started_at: String,
        pub finished_at: String,
        pub stdout_path: Option<String>,
        pub stderr_path: Option<String>,
    }

    /// One run of every referenced test on one code version; `code_version.head`
    /// is the commit the tests ran on (30.4).
    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct MatrixRecord {
        pub module: String,
        pub code_version: CodeVersion,
        pub matrix_sha256: String,
        pub tests: Vec<MatrixTestResult>,
    }

    impl MatrixRecord {
        /// Every referenced test exists, was not skipped, executed at least
        /// one test and passed; otherwise lists capability and test name.
        pub fn check_passes(&self) -> Result<()> {
            ensure!(!self.tests.is_empty(), "support matrix references no Lite tests");
            let problems: Vec<String> = self
                .tests
                .iter()
                .filter(|test| test.outcome != TestOutcome::Passed)
                .map(|test| format!("{} `{}`: {:?}", test.capability, test.test, test.outcome))
                .collect();
            ensure!(problems.is_empty(), "support matrix tests did not pass: {}", problems.join("; "));
            Ok(())
        }
    }

    pub struct MatrixPlan<'a> {
        pub repo_root: &'a Path,
        pub module: &'a str,
        pub code_version: CodeVersion,
        pub matrix: &'a Matrix,
        pub matrix_sha256: String,
        /// New directory for the complete output of every test command.
        pub output_dir: PathBuf,
        /// Same contract as [`GatePlan::jsonl_binary`].
        pub jsonl_binary: std::result::Result<PathBuf, String>,
        /// Each Python test gets its own subdirectory here.
        pub python_evidence: PathBuf,
    }

    /// Run every referenced test in document order; a failure never skips the rest.
    pub fn run_matrix(
        plan: &MatrixPlan<'_>,
        runner: &dyn CommandRunner,
        credentials: &Credentials,
    ) -> Result<MatrixRecord> {
        fs::create_dir_all(&plan.output_dir)
            .with_context(|| format!("cannot create matrix output directory {}", plan.output_dir.display()))?;
        let mut tests = Vec::new();
        for (index, (capability, test)) in referenced(plan.matrix)?.into_iter().enumerate() {
            let started_at = timestamp_now();
            let Some(test) = test else {
                tests.push(MatrixTestResult {
                    capability,
                    runner: "none".into(),
                    test: "(no Lite test listed)".into(),
                    command: String::new(),
                    exit_code: None,
                    counts: None,
                    outcome: TestOutcome::Missing,
                    finished_at: started_at.clone(),
                    started_at,
                    stdout_path: None,
                    stderr_path: None,
                });
                continue;
            };
            let (program, args, cwd) = test.argv();
            let output = match (&test, &plan.jsonl_binary) {
                (TestRef::Python { .. }, Err(build)) => CommandOutput {
                    exit_code: None,
                    stdout: Vec::new(),
                    stderr: format!("not run: JSONL binary build failed: {build}").into_bytes(),
                },
                (TestRef::Python { .. }, Ok(binary)) => {
                    let evidence = plan.python_evidence.join(format!("{:03}", index + 1));
                    super::run_python(runner, &plan.repo_root.join(&cwd), &args, binary, &evidence)
                }
                _ => runner.run(&plan.repo_root.join(&cwd), program, &args, &[]),
            };
            let finished_at = timestamp_now();
            let (counts, outcome) = judge(&test, &output);
            let stem = format!("matrix-{:03}-{}", index + 1, capability.replace(['/', '\\', ':'], "_"));
            let (stdout_path, stderr_path) = save_output(&plan.output_dir, &stem, &output, credentials)?;
            tests.push(MatrixTestResult {
                capability,
                runner: test.runner().to_owned(),
                test: test.label(),
                command: format!("{program} {}", args.join(" ")),
                exit_code: output.exit_code,
                counts,
                outcome,
                started_at,
                finished_at,
                stdout_path: Some(stdout_path),
                stderr_path: Some(stderr_path),
            });
        }
        Ok(MatrixRecord {
            module: plan.module.to_owned(),
            code_version: plan.code_version.clone(),
            matrix_sha256: plan.matrix_sha256.clone(),
            tests,
        })
    }
}

/// Stable-item removal demonstration (requirement 29.5, task 35.3).
///
/// The first stable `RunRequest` field row of `docs/stability.md` is removed
/// from the public surface of a temporary copy of Core by turning its `pub`
/// into `pub(crate)`. The library still compiles, so the only code that can
/// fail is the Stability_Test_Crate (`tests/stability_surface.rs`): the
/// demonstration passes when `cargo check --test stability_surface` succeeds
/// on the unpatched copy and fails on the patched copy with an error naming
/// the field. Deleting the field line instead would break Core itself and
/// prove nothing about the stability test.
pub mod stability {
    use super::*;

    pub const CORE_DIR: &str = "crates/ara-agent-lite-core";
    pub const STABILITY_DOC: &str = "crates/ara-agent-lite-core/docs/stability.md";
    const FIELD_PREFIX: &str = "ara_agent_lite_core::RunRequest.";

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct DemoPatch {
        /// Stability_Doc row, e.g. `ara_agent_lite_core::RunRequest.model_host`.
        pub item: String,
        pub field: String,
        /// Core-relative file and 1-based line of the field declaration.
        pub file: String,
        pub line: usize,
        pub before: String,
        pub after: String,
    }

    /// First stable `RunRequest` field row and its declaration under `src/`.
    pub fn choose_patch(core_dir: &Path, doc: &str) -> Result<DemoPatch> {
        let field = doc
            .lines()
            .filter_map(|line| {
                let cells: Vec<&str> = line.split('|').map(str::trim).collect();
                let item = cells.get(1)?.strip_prefix('`')?.strip_suffix('`')?;
                (cells.get(2) == Some(&"stable")).then_some(item)
            })
            .find_map(|item| item.strip_prefix(FIELD_PREFIX))
            .context("stability.md lists no stable RunRequest field")?
            .to_owned();
        let mut files = Vec::new();
        let mut stack = vec![core_dir.join("src")];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).with_context(|| format!("cannot list {}", dir.display()))? {
                let path = entry?.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    files.push(path);
                }
            }
        }
        files.sort();
        for path in files {
            let text = fs::read_to_string(&path)?;
            let Some(start) = text.lines().position(|line| line.trim_start().starts_with("pub struct RunRequest {"))
            else {
                continue;
            };
            for (offset, line) in text.lines().enumerate().skip(start + 1) {
                if line.trim_start().starts_with('}') {
                    break;
                }
                if line.trim_start().starts_with(&format!("pub {field}:")) {
                    let relative = path.strip_prefix(core_dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                    return Ok(DemoPatch {
                        item: format!("{FIELD_PREFIX}{field}"),
                        after: line.replacen("pub ", "pub(crate) ", 1),
                        before: line.to_owned(),
                        field,
                        file: relative,
                        line: offset + 1,
                    });
                }
            }
            bail!("`pub {field}:` not found in RunRequest in {}", path.display());
        }
        bail!("no `pub struct RunRequest` under {}", core_dir.join("src").display())
    }

    /// Copy `from` to the new directory `to`, skipping `target` and `.git`.
    pub fn copy_tree(from: &Path, to: &Path) -> Result<()> {
        fs::create_dir(to).with_context(|| format!("cannot create {}", to.display()))?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let name = entry.file_name();
            if name == "target" || name == ".git" {
                continue;
            }
            let (source, target) = (entry.path(), to.join(&name));
            if entry.file_type()?.is_dir() {
                copy_tree(&source, &target)?;
            } else {
                fs::copy(&source, &target).with_context(|| format!("cannot copy {}", source.display()))?;
            }
        }
        Ok(())
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct DemoStep {
        pub command: String,
        pub exit_code: Option<i32>,
        pub stdout_path: String,
        pub stderr_path: String,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct StabilityDemoRecord {
        pub module: String,
        pub code_version: CodeVersion,
        pub patch: DemoPatch,
        pub baseline: DemoStep,
        pub patched: DemoStep,
        /// The patched build's output names the field and the stability test.
        pub failure_names_item: bool,
        pub passed: bool,
    }

    pub struct DemoPlan<'a> {
        pub repo_root: &'a Path,
        pub module: &'a str,
        pub code_version: CodeVersion,
        /// New directory; Core is copied to `<work_dir>/ara-agent-lite-core`.
        pub work_dir: PathBuf,
        pub output_dir: PathBuf,
        /// Extra environment, e.g. a shared `CARGO_TARGET_DIR`.
        pub env: Vec<(String, OsString)>,
    }

    pub const CHECK_ARGS: [&str; 5] = ["check", "--locked", "--offline", "--test", "stability_surface"];

    pub fn run_demo(
        plan: &DemoPlan<'_>,
        runner: &dyn CommandRunner,
        credentials: &Credentials,
    ) -> Result<StabilityDemoRecord> {
        fs::create_dir_all(&plan.output_dir)?;
        fs::create_dir_all(&plan.work_dir)?;
        let copy = plan.work_dir.join("ara-agent-lite-core");
        copy_tree(&plan.repo_root.join(CORE_DIR), &copy)?;
        let doc = fs::read_to_string(copy.join("docs/stability.md")).context("cannot read the copied stability.md")?;
        let patch = choose_patch(&copy, &doc)?;
        let args: Vec<String> = CHECK_ARGS.iter().map(|arg| (*arg).to_owned()).collect();
        let command = format!("cargo {}", args.join(" "));
        let step = |stem: &str| -> Result<(DemoStep, String)> {
            let output = runner.run(&copy, "cargo", &args, &plan.env);
            let (stdout_path, stderr_path) = save_output(&plan.output_dir, stem, &output, credentials)?;
            let text =
                format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            Ok((DemoStep { command: command.clone(), exit_code: output.exit_code, stdout_path, stderr_path }, text))
        };
        let (baseline, _) = step("stability-baseline")?;
        let file = copy.join(&patch.file);
        let text = fs::read_to_string(&file)?;
        // Keep the file's own line endings; only the one declaration changes.
        let patched_text = text.replacen(&patch.before, &patch.after, 1);
        ensure!(patched_text != text, "patch did not apply to {}", file.display());
        fs::write(&file, patched_text)?;
        let (patched, output) = step("stability-patched")?;
        let failure_names_item = output.contains(&format!("`{}`", patch.field)) && output.contains("stability_surface");
        let passed =
            baseline.exit_code == Some(0) && matches!(patched.exit_code, Some(code) if code != 0) && failure_names_item;
        Ok(StabilityDemoRecord {
            module: plan.module.to_owned(),
            code_version: plan.code_version.clone(),
            patch,
            baseline,
            patched,
            failure_names_item,
            passed,
        })
    }
}

/// Out-of-tree consumer check (requirements 31.2 and 31.7, task 58.2).
///
/// # Reference scan
///
/// [`StabilityTable`] reads every `| \`path\` | level | description |` row of
/// the Stability_Doc. A reference is a full Lite path; it is classified as
/// follows (first rule that applies):
///
/// 1. An exact row (`T`, `T::member` or `T.field`) gives its level.
/// 2. A member named in backticks in the description of row `T` gives
///    `T::member` the row's level, or `experimental` / `internal` when the
///    clause (text between `；` or `。`) naming it says so; for example
///    `ModelHost` names `resolve` as required and `capabilities_source` and
///    `output_ceiling` as experimental. A mentioned `Default` also covers
///    `default`. A type with no row of its own but with rows for its members
///    (`RunRequest` through `RunRequest::new` and `RunRequest.id`) is a type
///    row at the best level of those members.
/// 3. Otherwise the longest listed prefix decides. Under a module-like row
///    (lowercase last segment: `rpi_agent`, `rpi_ai`, `core`) every path
///    inherits the row (supplementary rule 4). Under a `module::*` row every
///    path inherits it. Under a type row an UpperCamel member (an enum
///    variant or associated type) inherits the row, and any other member not
///    named in the description is experimental (supplementary rule 5).
/// 4. No row applies: `unlisted` (internal by supplementary rule 3).
///
/// References are collected without type inference, comments and string
/// literals removed first:
///
/// - `use` trees rooted at an `ara_agent_lite_*` crate, with braces, `self`,
///   `as` aliases and globs; the crate root alone and a module that has a
///   `module::*` row are not items themselves and are not reported;
/// - qualified paths in code that start at a Lite crate or at a name the file
///   imported from one (`RunRequest::new`, `RunMode::Execute`);
/// - each `fn` inside `impl <Lite trait> for <type>`, as `Trait::fn`; this is
///   how `fn generation_policy` in `impl ModelHost for …` maps to the row
///   `ara_agent_lite_core::model::ModelHost::generation_policy`;
/// - `.name(` method calls and `.name` field accesses, matched by name
///   against rule 1–2 entries `T::name` / `T.name` of every Lite type the
///   file names. A call matches when some such entry exists; it is reported
///   only when none of the matching entries is stable, so a stable method of
///   another imported type with the same name hides it. Method names under
///   rule 3 alone are not matched, to avoid flagging unrelated `.clone()`.
pub mod consumer {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    pub const CONSUMER_DIR: &str = "tools/lite-consumer-check";

    /// The two commands of requirement 31.2, run in [`CONSUMER_DIR`].
    pub const CONSUMER_COMMANDS: [&[&str]; 2] = [&["build"], &["clippy", "--", "-D", "warnings"]];

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Class {
        Stable,
        Experimental,
        Internal,
        /// Not in the Stability_Doc; treated as internal.
        Unlisted,
    }

    fn level(text: &str) -> Option<Class> {
        match text {
            "stable" => Some(Class::Stable),
            "experimental" => Some(Class::Experimental),
            "internal" => Some(Class::Internal),
            _ => None,
        }
    }

    fn upper(segment: &str) -> bool {
        segment.chars().next().is_some_and(char::is_uppercase)
    }

    #[derive(Clone, Debug, Default)]
    pub struct StabilityTable {
        rows: BTreeMap<String, Class>,
        members: BTreeMap<String, Class>,
    }

    impl StabilityTable {
        pub fn parse(doc: &str) -> Result<Self> {
            let mut table = Self::default();
            for line in doc.lines() {
                let cells: Vec<&str> = line.split('|').collect();
                if cells.len() < 4 {
                    continue;
                }
                let Some(item) = cells[1].trim().strip_prefix('`').and_then(|item| item.strip_suffix('`')) else {
                    continue;
                };
                if !item.starts_with("ara_agent_lite_") {
                    continue;
                }
                let class = level(cells[2].trim())
                    .with_context(|| format!("stability row `{item}` has level `{}`", cells[2].trim()))?;
                ensure!(table.rows.insert(item.to_owned(), class).is_none(), "stability row `{item}` is repeated");
                let description = cells[3..cells.len() - 1].join("|");
                for clause in description.split(['；', '。']) {
                    let clause_class = if clause.contains("experimental") {
                        Class::Experimental
                    } else if clause.contains("internal") {
                        Class::Internal
                    } else {
                        class
                    };
                    for code in clause.split('`').skip(1).step_by(2) {
                        let ident: String = code.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                        if ident.is_empty() {
                            continue;
                        }
                        let mut names = vec![ident.clone()];
                        if ident == "Default" {
                            names.push("default".to_owned());
                        }
                        for name in names {
                            let entry = table.members.entry(format!("{item}::{name}")).or_insert(clause_class);
                            *entry = (*entry).max(clause_class);
                        }
                    }
                }
            }
            ensure!(!table.rows.is_empty(), "the Stability_Doc has no Lite rows");
            // Types known only through their member rows (RunRequest).
            let implied: Vec<(String, Class)> = table
                .rows
                .iter()
                .filter_map(|(path, class)| {
                    let parent = match path.rsplit_once('.') {
                        Some((parent, field)) if !field.contains("::") => parent,
                        _ => path.rsplit_once("::")?.0,
                    };
                    let last = parent.rsplit("::").next()?;
                    (upper(last) && !table.rows.contains_key(parent)).then(|| (parent.to_owned(), *class))
                })
                .collect();
            for (parent, class) in implied {
                let entry = table.rows.entry(parent).or_insert(class);
                *entry = (*entry).min(class);
            }
            Ok(table)
        }

        /// Rule 1–2 entry for exactly this path, if any.
        fn entry(&self, path: &str) -> Option<Class> {
            self.rows.get(path).or_else(|| self.members.get(path)).copied()
        }

        /// A module with a `module::*` row (not an item itself).
        fn is_glob_module(&self, path: &str) -> bool {
            self.rows.contains_key(&format!("{path}::*"))
        }

        /// Classification of a full Lite path (see the module documentation).
        pub fn classify(&self, path: &str) -> Class {
            if let Some(class) = self.entry(path) {
                return class;
            }
            let path = path.strip_suffix("::*").unwrap_or(path);
            if let Some(class) = self.entry(path) {
                return class;
            }
            if path.contains('.') {
                return Class::Unlisted;
            }
            let segments: Vec<&str> = path.split("::").collect();
            for length in (1..segments.len()).rev() {
                let prefix = segments[..length].join("::");
                if let Some(class) = self.rows.get(&prefix) {
                    if !upper(segments[length - 1]) {
                        return *class;
                    }
                    let member = segments[length];
                    return match self.members.get(&format!("{prefix}::{member}")) {
                        Some(class) => *class,
                        None if upper(member) => *class,
                        None => Class::Experimental,
                    };
                }
                if let Some(class) = self.rows.get(&format!("{prefix}::*")) {
                    return *class;
                }
            }
            if self.is_glob_module(path) {
                return self.rows[&format!("{path}::*")];
            }
            Class::Unlisted
        }
    }

    /// A reference that is not stable.
    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
    pub struct ScanFinding {
        pub name: String,
        pub class: Class,
        /// `file:line:column`, 1-based.
        pub location: String,
    }

    /// Replace comments, string and character literals with spaces, keeping
    /// every line break, so positions stay the same.
    fn strip(text: &str) -> Vec<char> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = chars.clone();
        let blank = |out: &mut Vec<char>, from: usize, to: usize| {
            for c in &mut out[from..to.min(chars.len())] {
                if *c != '\n' {
                    *c = ' ';
                }
            }
        };
        let ident = |c: char| c.is_alphanumeric() || c == '_';
        let mut i = 0;
        while i < chars.len() {
            let next = chars.get(i + 1).copied();
            if chars[i] == '/' && next == Some('/') {
                let end = chars[i..].iter().position(|c| *c == '\n').map_or(chars.len(), |p| i + p);
                blank(&mut out, i, end);
                i = end;
            } else if chars[i] == '/' && next == Some('*') {
                let (mut depth, mut j) = (1, i + 2);
                while j < chars.len() && depth > 0 {
                    if chars[j] == '/' && chars.get(j + 1) == Some(&'*') {
                        depth += 1;
                        j += 2;
                    } else if chars[j] == '*' && chars.get(j + 1) == Some(&'/') {
                        depth -= 1;
                        j += 2;
                    } else {
                        j += 1;
                    }
                }
                blank(&mut out, i, j);
                i = j;
            } else if chars[i] == 'r' && (i == 0 || !ident(chars[i - 1])) && matches!(next, Some('"' | '#')) {
                let hashes = chars[i + 1..].iter().take_while(|c| **c == '#').count();
                if chars.get(i + 1 + hashes) != Some(&'"') {
                    i += 1;
                    continue;
                }
                let close: String = std::iter::once('"').chain(std::iter::repeat_n('#', hashes)).collect();
                let body = i + 2 + hashes;
                let rest: String = chars[body..].iter().collect();
                let end = rest.find(&close).map_or(chars.len(), |p| body + rest[..p].chars().count() + close.len());
                blank(&mut out, i, end);
                i = end;
            } else if chars[i] == '"' {
                let mut j = i + 1;
                while j < chars.len() && chars[j] != '"' {
                    j += if chars[j] == '\\' { 2 } else { 1 };
                }
                blank(&mut out, i, j + 1);
                i = j + 1;
            } else if chars[i] == '\'' {
                // A character literal closes within a few characters; a lifetime does not.
                let close = if next == Some('\\') {
                    chars[i + 2..].iter().take(10).position(|c| *c == '\'').map(|p| i + 2 + p)
                } else {
                    (chars.get(i + 2) == Some(&'\'')).then_some(i + 2)
                };
                match close {
                    Some(end) => {
                        blank(&mut out, i, end + 1);
                        i = end + 1;
                    }
                    None => i += 1,
                }
            } else {
                i += 1;
            }
        }
        out
    }

    #[derive(Clone, Debug)]
    struct Token {
        text: String,
        line: usize,
        column: usize,
    }

    fn tokens(chars: &[char]) -> Vec<Token> {
        let mut out = Vec::new();
        let (mut line, mut column, mut i) = (1, 1, 0);
        while i < chars.len() {
            let c = chars[i];
            let start = (line, column);
            let length = if c == '\n' {
                line += 1;
                column = 1;
                i += 1;
                continue;
            } else if c.is_whitespace() {
                1
            } else if c.is_alphanumeric() || c == '_' {
                chars[i..].iter().take_while(|c| c.is_alphanumeric() || **c == '_').count()
            } else if (c == ':' && chars.get(i + 1) == Some(&':')) || (c == '.' && chars.get(i + 1) == Some(&'.')) {
                2
            } else {
                1
            };
            if !c.is_whitespace() {
                out.push(Token { text: chars[i..i + length].iter().collect(), line: start.0, column: start.1 });
            }
            i += length;
            column += length;
        }
        out
    }

    fn is_ident(token: Option<&Token>) -> bool {
        token.is_some_and(|token| token.text.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_'))
    }

    /// One `use` tree; pushes `(full path, imported name, token)` leaves.
    fn use_tree(tokens: &[Token], pos: &mut usize, prefix: &[String], out: &mut Vec<(String, Option<String>, Token)>) {
        let mut segments = prefix.to_vec();
        let mut last = None;
        while let Some(token) = tokens.get(*pos) {
            if token.text == "::" {
                *pos += 1;
            } else if is_ident(Some(token)) && token.text != "as" {
                segments.push(token.text.clone());
                last = Some(token.clone());
                *pos += 1;
                if tokens.get(*pos).is_none_or(|token| token.text != "::") {
                    break;
                }
            } else {
                break;
            }
        }
        match tokens.get(*pos).map(|token| token.text.as_str()) {
            Some("{") => {
                *pos += 1;
                while tokens.get(*pos).is_some_and(|token| token.text != "}") {
                    let before = *pos;
                    use_tree(tokens, pos, &segments, out);
                    if tokens.get(*pos).is_some_and(|token| token.text == ",") || *pos == before {
                        *pos += 1;
                    }
                }
                *pos += 1;
                return;
            }
            Some("*") => {
                let token = tokens[*pos].clone();
                *pos += 1;
                out.push((format!("{}::*", segments.join("::")), None, token));
                return;
            }
            _ => {}
        }
        let Some(mut token) = last else { return };
        if segments.last().is_some_and(|segment| segment == "self") {
            segments.pop();
            token = tokens[pos.saturating_sub(1)].clone();
        }
        let mut name = segments.last().cloned();
        if tokens.get(*pos).is_some_and(|token| token.text == "as") {
            name = tokens.get(*pos + 1).map(|token| token.text.clone()).filter(|alias| alias != "_");
            *pos += 2;
        }
        out.push((segments.join("::"), name, token));
    }

    /// Qualified path starting at `tokens[start]`: segments and the index after it.
    fn path_at(tokens: &[Token], start: usize) -> (Vec<String>, usize) {
        let mut segments = vec![tokens[start].text.clone()];
        let mut end = start + 1;
        while tokens.get(end).is_some_and(|token| token.text == "::") && is_ident(tokens.get(end + 1)) {
            segments.push(tokens[end + 1].text.clone());
            end += 2;
        }
        (segments, end)
    }

    /// Every non-stable Lite reference in one source file.
    pub fn scan_source(file: &str, text: &str, table: &StabilityTable) -> Vec<ScanFinding> {
        let tokens = tokens(&strip(text));
        let mut imports: BTreeMap<String, String> = BTreeMap::new();
        let mut known: BTreeSet<String> = BTreeSet::new();
        let mut references: Vec<(String, Token)> = Vec::new();
        let mut in_use = vec![false; tokens.len()];
        let lite = |path: &str| path.starts_with("ara_agent_lite_");

        for start in 0..tokens.len() {
            if tokens[start].text != "use" || (start > 0 && tokens[start - 1].text == "::") {
                continue;
            }
            let mut pos = start + 1;
            let mut leaves = Vec::new();
            use_tree(&tokens, &mut pos, &[], &mut leaves);
            in_use[start..pos.min(tokens.len())].iter_mut().for_each(|flag| *flag = true);
            for (path, name, token) in leaves.into_iter().filter(|(path, _, _)| lite(path)) {
                if let Some(name) = name {
                    imports.insert(name, path.clone());
                }
                known.insert(path.clone());
                references.push((path, token));
            }
        }

        let resolve = |segments: &[String], imports: &BTreeMap<String, String>| -> Option<String> {
            if lite(&segments[0]) {
                Some(segments.join("::"))
            } else {
                let base = imports.get(&segments[0])?;
                Some(
                    std::iter::once(base.as_str())
                        .chain(segments[1..].iter().map(String::as_str))
                        .collect::<Vec<_>>()
                        .join("::"),
                )
            }
        };
        let mut members: Vec<(String, Token, bool)> = Vec::new();
        let mut index = 0;
        while index < tokens.len() {
            let token = &tokens[index];
            let after_path = index > 0 && matches!(tokens[index - 1].text.as_str(), "::" | "." | "..");
            if in_use[index] {
                index += 1;
                continue;
            }
            if token.text == "impl" {
                let mut pos = index + 1;
                if tokens.get(pos).is_some_and(|token| token.text == "<") {
                    let mut depth = 0;
                    while let Some(token) = tokens.get(pos) {
                        depth += match token.text.as_str() {
                            "<" => 1,
                            ">" => -1,
                            _ => 0,
                        };
                        pos += 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
                if is_ident(tokens.get(pos)) {
                    let (segments, end) = path_at(&tokens, pos);
                    let trait_path = resolve(&segments, &imports);
                    if let (Some(trait_path), true) =
                        (trait_path, tokens.get(end).is_some_and(|token| token.text == "for"))
                    {
                        known.insert(trait_path.clone());
                        let open = (end..tokens.len()).find(|i| tokens[*i].text == "{");
                        if let Some(open) = open {
                            let mut depth = 0;
                            for i in open..tokens.len() {
                                match tokens[i].text.as_str() {
                                    "{" => depth += 1,
                                    "}" => depth -= 1,
                                    "fn" if depth == 1 && is_ident(tokens.get(i + 1)) => references
                                        .push((format!("{trait_path}::{}", tokens[i + 1].text), tokens[i + 1].clone())),
                                    _ => {}
                                }
                                if depth == 0 {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            if is_ident(Some(token)) && !after_path {
                let (segments, end) = path_at(&tokens, index);
                if segments.len() >= 2 {
                    if let Some(path) = resolve(&segments, &imports) {
                        let parts: Vec<&str> = path.split("::").collect();
                        for length in 2..=parts.len() {
                            known.insert(parts[..length].join("::"));
                        }
                        references.push((path, token.clone()));
                    }
                }
                index = end;
                continue;
            }
            if token.text == "." && is_ident(tokens.get(index + 1)) && tokens[index + 1].text != "await" {
                let call = tokens.get(index + 2).is_some_and(|token| token.text == "(" || token.text == "::");
                members.push((tokens[index + 1].text.clone(), tokens[index + 1].clone(), call));
            }
            index += 1;
        }

        let mut findings = BTreeSet::new();
        let location = |token: &Token| format!("{file}:{}:{}", token.line, token.column);
        for (path, token) in references {
            let segments = path.split("::").count();
            let path_module = path.strip_suffix("::*").unwrap_or(&path);
            if segments == 1 || (!path.ends_with("::*") && table.is_glob_module(path_module)) {
                continue;
            }
            let class = table.classify(&path);
            if class != Class::Stable {
                findings.insert(ScanFinding { name: path, class, location: location(&token) });
            }
        }
        for (name, token, call) in members {
            let separator = if call { "::" } else { "." };
            let candidates: BTreeMap<String, Class> = known
                .iter()
                .filter_map(|base| {
                    let path = format!("{base}{separator}{name}");
                    table.entry(&path).map(|class| (path, class))
                })
                .collect();
            if candidates.values().all(|class| *class != Class::Stable) {
                if let Some((path, class)) = candidates.into_iter().next() {
                    findings.insert(ScanFinding { name: path, class, location: location(&token) });
                }
            }
        }
        findings.into_iter().collect()
    }

    /// Every `.rs` file under `root` (excluding `target`), repository-style
    /// relative names, and the non-stable references in them.
    pub fn scan_dir(root: &Path, table: &StabilityTable) -> Result<(Vec<String>, Vec<ScanFinding>)> {
        let mut files = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).with_context(|| format!("cannot list {}", dir.display()))? {
                let entry = entry?;
                let path = entry.path();
                if entry.file_type()?.is_dir() {
                    if entry.file_name() != "target" && entry.file_name() != ".git" {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    files.push(path);
                }
            }
        }
        files.sort();
        let mut names = Vec::new();
        let mut findings = Vec::new();
        for path in files {
            let name = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            let text = fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
            findings.extend(scan_source(&name, &text, table));
            names.push(name);
        }
        Ok((names, findings))
    }

    /// Output lines that are cargo or rustc diagnostics (`warning:`, `error[E…]:`).
    pub fn diagnostics(stdout: &[u8], stderr: &[u8]) -> Vec<String> {
        let text = format!("{}\n{}", String::from_utf8_lossy(stdout), String::from_utf8_lossy(stderr));
        text.lines()
            .map(str::trim)
            .filter(|line| {
                ["warning", "error"].iter().any(|kind| {
                    line.strip_prefix(kind).is_some_and(|rest| {
                        rest.starts_with(':') || (rest.starts_with('[') && rest.split_once("]:").is_some())
                    })
                })
            })
            .map(str::to_owned)
            .collect()
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ConsumerCommand {
        pub command: String,
        pub exit_code: Option<i32>,
        pub diagnostics: Vec<String>,
        pub started_at: String,
        pub finished_at: String,
        pub stdout_path: String,
        pub stderr_path: String,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ConsumerRecord {
        pub module: String,
        pub code_version: CodeVersion,
        pub commands: Vec<ConsumerCommand>,
        pub scanned_files: Vec<String>,
        pub findings: Vec<ScanFinding>,
    }

    impl ConsumerRecord {
        /// Both commands exit 0 without diagnostics, and no reference is
        /// experimental, internal or unlisted (31.2, 31.7).
        pub fn check_passes(&self) -> Result<()> {
            let mut problems = Vec::new();
            for command in &self.commands {
                if command.exit_code != Some(0) {
                    problems.push(format!("`{}` exit code {:?}", command.command, command.exit_code));
                }
                if !command.diagnostics.is_empty() {
                    problems.push(format!("`{}` printed {}", command.command, command.diagnostics.join(" | ")));
                }
            }
            if self.scanned_files.is_empty() {
                problems.push("no consumer source file was scanned".to_owned());
            }
            problems.extend(
                self.findings
                    .iter()
                    .map(|finding| format!("{:?} `{}` at {}", finding.class, finding.name, finding.location)),
            );
            ensure!(problems.is_empty(), "consumer check failed: {}", problems.join("; "));
            Ok(())
        }
    }

    pub struct ConsumerPlan<'a> {
        pub repo_root: &'a Path,
        pub module: &'a str,
        pub code_version: CodeVersion,
        pub output_dir: PathBuf,
    }

    pub fn run_consumer(
        plan: &ConsumerPlan<'_>,
        runner: &dyn CommandRunner,
        credentials: &Credentials,
    ) -> Result<ConsumerRecord> {
        fs::create_dir_all(&plan.output_dir)?;
        let dir = plan.repo_root.join(CONSUMER_DIR);
        let mut commands = Vec::new();
        for (index, words) in CONSUMER_COMMANDS.iter().enumerate() {
            let args: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
            let started_at = timestamp_now();
            let output = runner.run(&dir, "cargo", &args, &[]);
            let finished_at = timestamp_now();
            let (stdout_path, stderr_path) =
                save_output(&plan.output_dir, &format!("consumer-{}-{}", index + 1, words[0]), &output, credentials)?;
            commands.push(ConsumerCommand {
                command: format!("cargo {}", args.join(" ")),
                exit_code: output.exit_code,
                diagnostics: diagnostics(&output.stdout, &output.stderr),
                started_at,
                finished_at,
                stdout_path,
                stderr_path,
            });
        }
        let doc =
            fs::read_to_string(plan.repo_root.join(stability::STABILITY_DOC)).context("cannot read stability.md")?;
        let (scanned_files, findings) = scan_dir(&dir, &StabilityTable::parse(&doc)?)?;
        Ok(ConsumerRecord {
            module: plan.module.to_owned(),
            code_version: plan.code_version.clone(),
            commands,
            scanned_files,
            findings,
        })
    }
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

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn real_consumer_uses_only_stable_items_and_trait_rows_map() {
        use consumer::{scan_dir, scan_source, Class, StabilityTable, CONSUMER_DIR};
        let doc = fs::read_to_string(repo().join(stability::STABILITY_DOC)).unwrap();
        let table = StabilityTable::parse(&doc).unwrap();
        let (files, findings) = scan_dir(&repo().join(CONSUMER_DIR), &table).unwrap();
        assert!(files.contains(&"src/lib.rs".to_owned()), "{files:?}");
        assert!(findings.is_empty(), "{findings:#?}");

        let core = "ara_agent_lite_core";
        assert_eq!(table.classify(&format!("{core}::model::ModelHost::generation_policy")), Class::Stable);
        assert_eq!(table.classify(&format!("{core}::model::ModelHost::output_ceiling")), Class::Experimental);
        assert_eq!(table.classify(&format!("{core}::model::ModelHost::resolve")), Class::Stable);
        assert_eq!(table.classify(&format!("{core}::RunRequest")), Class::Stable);
        assert_eq!(table.classify(&format!("{core}::RunRequest.policies")), Class::Experimental);
        assert_eq!(table.classify(&format!("{core}::RunMode::Execute")), Class::Stable);
        assert_eq!(table.classify(&format!("{core}::rpi_agent::AgentTool::execute")), Class::Stable);
        assert_eq!(table.classify(&format!("{core}::model::resolve_catalog")), Class::Experimental);
        assert_eq!(table.classify(&format!("{core}::agent::RunRequest")), Class::Experimental);
        assert_eq!(table.classify(&format!("{core}::model::GenerationPolicy::apply")), Class::Experimental);
        assert_eq!(table.classify(&format!("{core}::no_such_item")), Class::Unlisted);
        assert_eq!(table.classify("ara_agent_lite_harness::core::RunRequest"), Class::Experimental);
        assert_eq!(table.classify("ara_agent_lite_harness::TaskHandle::wait"), Class::Stable);

        let source = "use ara_agent_lite_core::{model::{ModelHost, Route}, RunRequest};\n\
                      struct H;\n\
                      impl ModelHost for H {\n    fn output_ceiling(&self) {}\n    fn generation_policy(&self) {}\n}\n\
                      fn f(r: RunRequest, h: &dyn ModelHost) { r.policies; h.output_ceiling(); r.brief; }\n\
                      // ara_agent_lite_core::fresh_id is only mentioned\n\
                      const S: &str = \"ara_agent_lite_core::fresh_id\";\n";
        let found: Vec<(String, String)> =
            scan_source("x.rs", source, &table).into_iter().map(|f| (f.name, f.location)).collect();
        assert_eq!(
            found,
            [
                (format!("{core}::RunRequest.policies"), "x.rs:7:44".to_owned()),
                (format!("{core}::model::ModelHost::output_ceiling"), "x.rs:4:8".to_owned()),
                (format!("{core}::model::ModelHost::output_ceiling"), "x.rs:7:56".to_owned()),
            ]
        );
    }

    #[test]
    fn stability_demo_patches_a_stable_run_request_field() {
        let core = repo().join(stability::CORE_DIR);
        let doc = fs::read_to_string(repo().join(stability::STABILITY_DOC)).unwrap();
        let patch = stability::choose_patch(&core, &doc).unwrap();
        assert_eq!(patch.item, "ara_agent_lite_core::RunRequest.model_host");
        assert_eq!(patch.file, "src/agent.rs");
        assert!(patch.before.trim_start().starts_with("pub model_host:"), "{patch:?}");
        assert!(patch.after.trim_start().starts_with("pub(crate) model_host:"), "{patch:?}");
    }

    #[test]
    fn matrix_argv_and_judgement() {
        use matrix::{judge, TestOutcome, TestRef};
        let cargo =
            TestRef::Cargo { manifest: "crates/a/Cargo.toml".into(), target: "lib".into(), name: "m::t".into() };
        let (program, args, cwd) = cargo.argv();
        assert_eq!((program, cwd.as_str()), ("cargo", "."));
        assert_eq!(args.join(" "), "test --manifest-path crates/a/Cargo.toml --lib m::t -- --exact");
        let python = TestRef::Python { file: "crates/j/tests/test_x.py".into(), case: "C.test_y".into() };
        let (_, args, cwd) = python.argv();
        assert_eq!((args.join(" "), cwd), ("-m unittest -v test_x.C.test_y".into(), "crates/j/tests".into()));
        let out =
            |code: Option<i32>, text: &str| CommandOutput { exit_code: code, stdout: text.into(), stderr: Vec::new() };
        let ok = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out";
        assert_eq!(judge(&cargo, &out(Some(0), ok)).1, TestOutcome::Passed);
        assert_eq!(
            judge(&cargo, &out(Some(0), "test result: ok. 0 passed; 0 failed; 0 ignored")).1,
            TestOutcome::ZeroExecuted
        );
        assert_eq!(
            judge(&cargo, &out(Some(0), "test result: ok. 0 passed; 0 failed; 1 ignored")).1,
            TestOutcome::Skipped
        );
        assert_eq!(judge(&cargo, &out(Some(101), "error: no test target named `x`")).1, TestOutcome::Missing);
        assert_eq!(judge(&python, &out(Some(0), "Ran 1 test in 0s\n\nOK (skipped=1)\n")).1, TestOutcome::Skipped);
        assert_eq!(judge(&python, &out(Some(5), "Ran 0 tests in 0s\n\nNO TESTS RAN\n")).1, TestOutcome::ZeroExecuted);
        assert_eq!(judge(&python, &out(Some(0), "Ran 1 test in 0s\n\nOK\n")).1, TestOutcome::Passed);
        assert_eq!(judge(&python, &out(None, "")).1, TestOutcome::NoExitCode);
    }

    #[test]
    fn consumer_diagnostics_are_warning_and_error_lines() {
        let stderr =
            b"   Compiling x\nwarning: unused import\nerror[E0425]: cannot find\nnote: warning: nested\n    Finished";
        assert_eq!(consumer::diagnostics(b"", stderr), ["warning: unused import", "error[E0425]: cannot find"]);
        assert!(consumer::diagnostics(b"    Finished `dev` profile", b"").is_empty());
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
