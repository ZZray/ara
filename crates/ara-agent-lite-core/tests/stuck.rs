//! Requirement 18 controlled tests over the real `execute`: repeated errors
//! and repeated observations produce a rule hit, one sourced correction and a
//! NoProgress stop with exact event fields; no events without thresholds; an
//! invalid threshold is refused before any upstream request.
mod golden_support;
mod support;

use ara_agent_lite_core::{execute, stuck_detector::StuckPolicy};
use serde_json::{json, Value};
use support::*;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn stuck_events(events: &[Value]) -> Vec<Value> {
    of_type(events, "stuck_detector").into_iter().cloned().collect()
}

#[tokio::test]
async fn repeated_errors_hit_correct_then_stop_with_no_progress() {
    let root = TempDir::new();
    let script = (1..=4).map(|n| call(&format!("e{n}"), "build", json!({"attempt": n}))).chain([text("never")]);
    let provider = Provider::new(script.collect());
    let build = ScriptedTool::new("build", |_| Err("exit 1: missing header foo.h".into()));
    let mut request = request(root.path(), &provider, std::slice::from_ref(&build), 8, "Build the project.");
    request.policies.stuck = Some(StuckPolicy { repeated_error: Some(2), ..StuckPolicy::default() });
    let (result, events) = run(request).await;
    assert_eq!(result.error, "no_progress: repeated_error");
    assert_eq!(build.count(), 4);
    assert_eq!(provider.request_count(), 4);
    assert_eq!(
        stuck_events(&events),
        vec![
            json!({"type":"stuck_detector","action":"rule_hit","rule":"repeated_error","count":2,"threshold":2,
                   "native_call_ids":["e1","e2"],"calls_used":2}),
            json!({"type":"stuck_detector","action":"correction","rule":"repeated_error","count":2,"threshold":2,
                   "native_call_ids":["e1","e2"],"calls_used":2}),
            json!({"type":"stuck_detector","action":"no_progress","rule":"repeated_error","count":2,"threshold":2,
                   "native_call_ids":["e3","e4"],"calls_used":4}),
        ]
    );
    // The correction is injected once, before the third request, after the second result.
    let bodies: Vec<String> = provider.bodies().iter().map(Value::to_string).collect();
    let marker = "source=stuck_detector; rule=repeated_error; native_call_ids=e1,e2";
    assert!(!bodies[1].contains(marker));
    assert_eq!(bodies[2].matches(marker).count(), 1);
    assert!(bodies[2].find("missing header").unwrap() < bodies[2].find(marker).unwrap());
    assert_eq!(bodies[3].matches("source=stuck_detector").count(), 1, "no second correction");
    let evidence = &of_type(&events, "agent_end")[0]["stop_evidence"];
    assert_eq!(
        evidence,
        &json!({"cause":"no_progress","version":1,"native_round_closed":true,"model_final_stop":false,
                "rule":"repeated_error","count":2,"threshold":2,"native_call_ids":["e3","e4"]})
    );
}

#[tokio::test]
async fn repeated_observations_count_only_listed_tools() {
    let root = TempDir::new();
    // Interleaved `note` calls with identical results are not observation tools.
    let mut script = Vec::new();
    for n in 1..=5 {
        script.push(calls(&[
            (format!("o{n}").as_str(), "look", json!({"path": format!("f{n}")})),
            (format!("n{n}").as_str(), "note", json!({"n": n})),
        ]));
    }
    script.push(text("never"));
    let provider = Provider::new(script);
    let look = ScriptedTool::new("look", |_| Ok("same directory listing".into()));
    let note = ScriptedTool::new("note", |_| Ok("same directory listing".into()));
    let mut request = request(root.path(), &provider, &[look.clone(), note.clone()], 8, "Inspect the files.");
    request.policies.stuck = Some(StuckPolicy {
        repeated_observation: Some(2),
        observation_tools: ["look".to_owned()].into(),
        ..StuckPolicy::default()
    });
    let (result, events) = run(request).await;
    assert_eq!(result.error, "no_progress: repeated_observation");
    let actions: Vec<(String, Value)> = stuck_events(&events)
        .iter()
        .map(|event| (event["action"].as_str().unwrap().to_owned(), event["native_call_ids"].clone()))
        .collect();
    assert_eq!(
        actions,
        vec![
            ("rule_hit".into(), json!(["o2", "o3"])),
            ("correction".into(), json!(["o2", "o3"])),
            ("no_progress".into(), json!(["o4", "o5"])),
        ]
    );
    assert_eq!(look.count(), 5);
    assert_eq!(note.count(), 5);
}

#[tokio::test]
async fn unconfigured_detector_emits_nothing_and_injects_nothing() {
    let root = TempDir::new();
    let script = (1..=4).map(|n| call(&format!("e{n}"), "build", json!({}))).chain([text("gave up")]);
    let provider = Provider::new(script.collect());
    let build = ScriptedTool::new("build", |_| Err("exit 1".into()));
    let (result, events) = run(request(root.path(), &provider, std::slice::from_ref(&build), 8, "Build.")).await;
    assert_eq!(result.code, 0, "{}", result.error);
    assert!(stuck_events(&events).is_empty());
    assert!(provider.bodies().iter().all(|body| !body.to_string().contains("source=stuck_detector")));
    assert!(of_type(&events, "agent_end")[0].get("stop_evidence").is_none());
    // Requirement 18.8: same event types and Session messages as tests/golden/p2c-no-stuck/.
    let mut normalizer = golden_support::Normalizer::new(root.path(), &provider.endpoint);
    golden_support::check("p2c-no-stuck", "events.json", &golden_support::event_types(&events));
    let session = golden_support::journal_entries(&root.path().join("session.json"));
    golden_support::check("p2c-no-stuck", "session.json", &normalizer.value(&session));
}

#[tokio::test]
async fn invalid_thresholds_are_refused_before_any_request() {
    for policy in [
        StuckPolicy { repeated_call: Some(1), ..StuckPolicy::default() },
        StuckPolicy { repeated_error: Some(1001), ..StuckPolicy::default() },
        StuckPolicy { repeated_observation: Some(0), ..StuckPolicy::default() },
    ] {
        let root = TempDir::new();
        let provider = Provider::new(vec![]);
        let mut request = request(root.path(), &provider, &[], 2, "Build.");
        request.policies.stuck = Some(policy);
        let (sender, _receiver) = mpsc::channel(8);
        let error = execute(request, CancellationToken::new(), sender).await.err().expect("refused");
        assert!(error.to_string().contains("outside 2..=1000"), "{error}");
        assert_eq!(provider.request_count(), 0);
        assert_eq!(provider.catalog_requests.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(!root.path().join("session.json").exists());
    }
}
