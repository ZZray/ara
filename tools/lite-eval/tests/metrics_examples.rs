//! Metrics examples (requirements 4.1, 4.7) through the real
//! Capability_Test_Host, Harness and Core against a controlled upstream.

mod runner_support;

use std::sync::Arc;

use lite_eval::runner::{run, HarnessExecutor, RunInputs, RunReport, Selection};
use runner_support::{call, model_reply, text, turn, ProbeMode, Provider, TestRepo};
use serde_json::{json, Value};

const VARIABLE: &str = "LITE_EVAL_METRICS_KEY";

fn execute(repo: &TestRepo, name: &str, task: &str) -> RunReport {
    let evidence = repo.evidence(name);
    let env = |variable: &str| (variable == VARIABLE).then(|| "metrics-key-7c1d".to_owned());
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    runtime
        .block_on(run(RunInputs {
            repo_root: &repo.root,
            evidence_root: &evidence,
            selection: &Selection::Ids(vec![task.to_owned()]),
            executor: &HarnessExecutor,
            env: &env,
            probe_only: false,
            features: lite_eval::host::Features::V1,
            baseline: false,
        }))
        .unwrap()
}

fn result_of(report: &RunReport) -> Value {
    let path = report.manifest.selected[0].result.as_ref().expect("result written");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Requirement 4.1: every metric is recorded whatever the ending.
fn assert_all_metrics(result: &Value) {
    assert!(result["stop"].is_string(), "stop: {result:#}");
    assert!(result["logical_calls"].is_u64(), "logical_calls: {result:#}");
    assert!(result["wall_ms"].is_u64(), "wall_ms: {result:#}");
    assert!(result["success"].is_boolean());
    for key in ["usage", "cost", "correctness"] {
        assert!(result[key]["status"].is_string(), "{key}: {result:#}");
    }
    for key in ["artifacts", "session"] {
        assert!(matches!(result["paths"][key]["status"].as_str(), Some("present" | "missing")), "{key}: {result:#}");
    }
    assert!(!result["paths"]["events"].as_array().unwrap().is_empty());
    for path in result["paths"].to_string().split("\"path\":\"").skip(1) {
        let path = path.split('"').next().unwrap().replace("\\\\", "\\");
        assert!(std::path::Path::new(&path).exists(), "{path} recorded as present");
    }
}

fn usage(prompt: u64, completion: u64) -> Value {
    json!({"prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": prompt + completion})
}

#[test]
fn reported_usage_is_summed_and_omitted_usage_is_unknown() {
    // Both calls report usage.
    let provider = Provider::new(Arc::new(|body: &Value| {
        let mut reply = model_reply(body);
        reply["usage"] = if turn(body) == 0 { usage(100, 7) } else { usage(120, 5) };
        reply
    }));
    let repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    let report = execute(&repo, "reported", "S1");
    let result = result_of(&report);
    assert_all_metrics(&result);
    assert_eq!(result["success"], true);
    assert_eq!(result["logical_calls"], 2);
    assert_eq!(result["usage"]["status"], "complete");
    assert_eq!(
        (result["usage"]["input_tokens"].clone(), result["usage"]["output_tokens"].clone()),
        (json!(220), json!(12))
    );
    assert_eq!(result["usage"]["calls"].as_array().unwrap().len(), 2);
    assert_eq!(result["cost"]["status"], "unknown", "price is null");
    assert!(result["cost"].get("amount").is_none());
    assert_eq!(report.manifest.summary.usage.input_tokens, Some(220));

    // No call reports usage: unknown, never 0.
    let provider = Provider::standard();
    let repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    let result = result_of(&execute(&repo, "omitted", "S1"));
    assert_all_metrics(&result);
    assert_eq!(result["usage"]["status"], "unknown");
    assert_eq!(result["usage"]["unknown_calls"], 2);
    assert!(result["usage"].get("input_tokens").is_none() && result["usage"].get("output_tokens").is_none());
    let calls = result["usage"]["calls"].as_array().unwrap();
    assert!(calls.iter().all(|call| call["status"] == "unknown" && call["reason"].is_string()));

    // Only the first call reports; with a known price the cost is partial too.
    let provider = Provider::new(Arc::new(|body: &Value| {
        let mut reply = model_reply(body);
        if turn(body) == 0 {
            reply["usage"] = usage(1000, 10);
        }
        reply
    }));
    let mut repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    repo.policy["price"] = json!({"currency": "CNY", "input_per_million": 2.0, "output_per_million": 8.0});
    repo.write_policy();
    let report = execute(&repo, "partial", "S1");
    let result = result_of(&report);
    assert_eq!(result["usage"]["status"], "incomplete");
    assert_eq!(
        (result["usage"]["input_tokens"].clone(), result["usage"]["unknown_calls"].clone()),
        (json!(1000), json!(1))
    );
    assert_eq!(result["cost"]["status"], "incomplete");
    let amount = result["cost"]["amount"].as_f64().unwrap();
    assert!((amount - (1000.0 * 2.0 + 10.0 * 8.0) / 1e6).abs() < 1e-12, "{amount}");
    assert_eq!(report.manifest.summary.usage.tasks_with_unknown, 1);
    assert_eq!(serde_json::to_value(report.manifest.summary.usage.status).unwrap(), "incomplete");
}

#[test]
fn budget_limit_timeout_and_harness_error_record_every_metric() {
    // Budget: the public check always fails and only one Run is allowed.
    let provider = Provider::standard();
    let mut repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    repo.set["tasks"][0]["profile"]["public_check"] = json!(["python", "-c", "import sys; sys.exit(1)"]);
    repo.write_set();
    repo.policy["budgets"]["S1"] =
        json!({"max_runs": 1, "max_calls": 4, "deadline_seconds": 120, "run_max_calls": 4, "run_max_seconds": 60});
    repo.write_policy();
    let result = result_of(&execute(&repo, "budget", "S1"));
    assert_all_metrics(&result);
    assert_eq!(result["stop"], "BudgetExhausted", "{result:#}");
    assert_eq!(result["logical_calls"], 2);
    assert_eq!(result["success"], true, "the Oracle, not the Harness stop, decides success");

    // Run call cap reached inside a tool round: the Harness asks for
    // reconciliation; metrics are still complete.
    let provider = Provider::new(Arc::new(|body: &Value| {
        call(&format!("r{}", turn(body)), "ws_read_file", json!({"path": "README.md"}))
    }));
    let mut repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    repo.policy["budgets"]["S1"] =
        json!({"max_runs": 1, "max_calls": 1, "deadline_seconds": 120, "run_max_calls": 1, "run_max_seconds": 60});
    repo.write_policy();
    let result = result_of(&execute(&repo, "run-cap", "S1"));
    assert_all_metrics(&result);
    assert_eq!(result["logical_calls"], 1);
    assert_eq!(result["success"], false);

    // Timeout: the upstream answers after the Task deadline.
    let provider = Provider::new(Arc::new(|body: &Value| {
        let mut reply = text("late");
        reply["delay_ms"] = json!(if turn(body) == 0 { 4000 } else { 0 });
        reply
    }));
    let mut repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    repo.policy["budgets"]["S1"] =
        json!({"max_runs": 1, "max_calls": 4, "deadline_seconds": 2, "run_max_calls": 4, "run_max_seconds": 2});
    repo.write_policy();
    let result = result_of(&execute(&repo, "timeout", "S1"));
    assert_all_metrics(&result);
    // The Harness cuts the Run at the time limit (whole seconds left before
    // the deadline) and reports the cut call for reconciliation.
    assert!(matches!(result["stop"].as_str(), Some("Cancelled" | "ReconciliationRequired")), "{result:#}");
    let wall = result["wall_ms"].as_u64().unwrap();
    assert!((900..4000).contains(&wall), "cut at the limit, not after the 4 s reply: {result:#}");
    assert_eq!(result["usage"]["status"], "unknown", "the cut call has no model_result");
    assert_eq!(result["success"], false);

    // Harness error: the catalog does not know the model the probe accepted,
    // so Core returns an error and the Harness stops for reconciliation.
    let provider = Provider::standard();
    *provider.probe_mode.lock().unwrap() = ProbeMode::ok("ghost-model");
    let mut repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    repo.policy["model"]["id"] = json!("ghost-model");
    repo.write_policy();
    let result = result_of(&execute(&repo, "harness-error", "S1"));
    assert_all_metrics(&result);
    assert_eq!(result["stop"], "ReconciliationRequired", "{result:#}");
    assert_eq!(result["logical_calls"], 0);
    assert_eq!(result["usage"]["status"], "complete", "no Logical_Call: a known zero");
    assert_eq!(result["success"], false);
}

#[test]
fn an_oracle_timeout_or_unparsable_output_leaves_correctness_unknown() {
    let provider = Provider::standard();
    let mut repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    let base = repo.root.parent().unwrap().to_owned();
    std::fs::write(base.join("slow.py"), "import time\ntime.sleep(5)\n").unwrap();
    std::fs::write(base.join("garbage.py"), "print('not a lite-eval-oracle-v1 result')\n").unwrap();
    repo.set["tasks"][0]["oracle"]["command"] = json!(["python", base.join("slow.py")]);
    repo.set["tasks"][1]["oracle"]["command"] = json!(["python", base.join("garbage.py")]);
    repo.write_set();
    repo.policy["oracle_timeout_seconds"] = json!(1);
    repo.write_policy();
    for (task, reason) in [("S1", "exceeded 1 seconds"), ("S2", "without a parsable")] {
        let report = execute(&repo, task, task);
        let result = result_of(&report);
        assert_all_metrics(&result);
        assert_eq!(result["success"], false);
        assert_eq!(result["correctness"]["status"], "oracle_error", "{result:#}");
        assert!(result["correctness"]["reason"].as_str().unwrap().contains(reason), "{result:#}");
        assert!(result["correctness"].get("hidden").is_none() && result["correctness"].get("public").is_none());
        let metrics = report.manifest.selected[0].metrics.as_ref().unwrap();
        assert!(!metrics.correctness.known && metrics.correctness.hidden.is_none());
        assert_eq!(report.manifest.summary.correctness.tasks_with_unknown, 1);
        assert_eq!(serde_json::to_value(report.manifest.summary.correctness.status).unwrap(), "unknown");
    }
}
