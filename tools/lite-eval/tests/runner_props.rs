//! Property 8: task scheduling and success judgement (requirements 3.1, 4.2).
//!
//! A controlled executor stands in for the Harness so each task's ending is
//! chosen by the test: Oracle pass, Oracle fail, a model that claims it is
//! done (Oracle fail), a clean stop with an Oracle error, or a Host/Harness
//! error. The runner itself (policy, Task_Set, code version, probe,
//! scheduling, result files, manifest, exit code) is the production code.

mod runner_support;

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use lite_eval::{
    host::WriteCounts,
    manifest::TaskState,
    runner::{run, Correctness, RunInputs, Selection, TaskContext, TaskExecutor, TaskRecord},
};
use proptest::prelude::*;
use runner_support::{Provider, TestRepo, IDS};

#[derive(Clone, Copy, Debug)]
enum Ending {
    OraclePass,
    OracleFail,
    ModelClaimsDone,
    OracleError,
    HarnessError,
}

fn ending() -> impl Strategy<Value = Ending> {
    prop_oneof![
        Just(Ending::OraclePass),
        Just(Ending::OracleFail),
        Just(Ending::ModelClaimsDone),
        Just(Ending::OracleError),
        Just(Ending::HarnessError),
    ]
}

struct Scripted {
    endings: BTreeMap<String, Ending>,
    /// ("start" | "end", task ID) in the order they happened.
    log: Arc<Mutex<Vec<(&'static str, String)>>>,
}

#[async_trait]
impl TaskExecutor for Scripted {
    async fn execute(&self, context: &TaskContext) -> TaskRecord {
        let id = context.entry.task_id.clone();
        self.log.lock().unwrap().push(("start", id.clone()));
        assert!(context.task_dir.is_dir() && std::fs::read_dir(&context.task_dir).unwrap().next().is_none());
        // Yield so an overlapping scheduler would interleave here.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        let judged =
            |passed| Correctness::Judged { passed, public: None, hidden: None, holdout: None, raw_output: None };
        let ending = self.endings[&id];
        let mut record = TaskRecord::not_run("controlled".into());
        record.error = None;
        record.write_counts = WriteCounts::default();
        match ending {
            Ending::OraclePass => record.correctness = judged(true),
            Ending::OracleFail => record.correctness = judged(false),
            Ending::ModelClaimsDone => {
                record.stop = Some(ara_agent_lite_harness::TaskStop::AwaitingHostAcceptance);
                record.detail = "model reported the task complete".into();
                record.correctness = judged(false);
            }
            Ending::OracleError => {
                record.stop = Some(ara_agent_lite_harness::TaskStop::AwaitingHostAcceptance);
                record.correctness = Correctness::OracleError { reason: "exit 0 without a judgement".into() };
            }
            Ending::HarnessError => record.error = Some("Harness did not start".into()),
        }
        self.log.lock().unwrap().push(("end", id));
        record
    }
}

fn selection() -> impl Strategy<Value = Selection> {
    prop_oneof![
        1 => Just(Selection::All),
        4 => Just(IDS.to_vec()).prop_shuffle().prop_flat_map(|ids| (1..=ids.len()).prop_map(move |n| {
            Selection::Ids(ids[..n].iter().map(|id| id.to_string()).collect())
        })),
    ]
}

#[test]
fn tasks_run_in_order_one_at_a_time_and_pass_only_on_an_oracle_pass() {
    // One repository and probe server for every case; each case writes to
    // its own evidence root.
    let provider = Provider::standard();
    let repo = TestRepo::new(&provider.endpoint, "LITE_EVAL_PROPS_KEY", &|_| "correct".into());
    let cases = 256;
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig::with_cases(cases));
    let strategy = (selection(), proptest::collection::vec(ending(), 6));
    let result = runner.run(&strategy, |(selection, endings)| {
        let endings: BTreeMap<String, Ending> =
            IDS.iter().map(|id| id.to_string()).zip(endings.iter().copied()).collect();
        let log = Arc::new(Mutex::new(Vec::new()));
        let executor = Scripted { endings: endings.clone(), log: log.clone() };
        let evidence = tempfile::tempdir().unwrap();
        let env = |name: &str| (name == "LITE_EVAL_PROPS_KEY").then(|| "props-key".to_owned());
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let report = runtime
            .block_on(run(RunInputs {
                repo_root: &repo.root,
                evidence_root: evidence.path(),
                selection: &selection,
                executor: &executor,
                env: &env,
                probe_only: false,
                features: lite_eval::host::Features::V1,
                baseline: false,
            }))
            .unwrap();

        let expected: Vec<String> = match &selection {
            Selection::All => {
                let mut ids: Vec<String> = IDS.iter().map(|id| id.to_string()).collect();
                ids.sort();
                ids
            }
            Selection::Ids(ids) => ids.clone(),
        };
        let log = log.lock().unwrap().clone();
        let mut sequential = Vec::new();
        for id in &expected {
            sequential.push(("start", id.clone()));
            sequential.push(("end", id.clone()));
        }
        prop_assert_eq!(log, sequential, "each task starts after the previous one ended, in order");
        let states: Vec<(String, TaskState)> =
            report.manifest.selected.iter().map(|record| (record.task_id.clone(), record.state)).collect();
        let want: Vec<(String, TaskState)> = expected
            .iter()
            .map(|id| {
                let state =
                    if matches!(endings[id], Ending::OraclePass) { TaskState::Passed } else { TaskState::Failed };
                (id.clone(), state)
            })
            .collect();
        prop_assert_eq!(&states, &want);
        let all_pass = want.iter().all(|(_, state)| *state == TaskState::Passed);
        prop_assert_eq!(report.exit, if all_pass { 0 } else { 1 });
        prop_assert_eq!(report.manifest.summary.passed + report.manifest.summary.failed, expected.len());
        for record in &report.manifest.selected {
            let path = record.result.as_ref().expect("result written");
            let result: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            prop_assert_eq!(result["success"].as_bool(), Some(record.state == TaskState::Passed));
            prop_assert_eq!(result["task_id"].as_str(), Some(record.task_id.as_str()));
        }
        prop_assert!(report.manifest_path.is_file());
        prop_assert_eq!(report.manifest.all_tasks.len(), 6);
        Ok(())
    });
    if let Err(error) = result {
        panic!("{error}");
    }
    assert_eq!(
        provider.probe_requests.load(std::sync::atomic::Ordering::SeqCst),
        usize::try_from(cases).unwrap(),
        "one probe per run"
    );
    assert_eq!(provider.model_requests.load(std::sync::atomic::Ordering::SeqCst), 0);
}
