//! Property 9: probe result classification (requirement 3.3).
//!
//! A local server answers the probe with a generated response; the probe
//! sends exactly one request (none without a credential), succeeds exactly
//! when the model echo, OpenAI response shape, reachability and
//! authorization all hold, and otherwise reports the injected failure.

mod runner_support;

use std::sync::atomic::Ordering;

use lite_eval::{
    policy::RunPolicy,
    probe::{probe, ProbeFailure},
};
use proptest::prelude::*;
use runner_support::{policy, ProbeMode, Provider, MODEL};
use serde_json::json;

const ALIAS: &str = "fixture-alias";

#[derive(Clone, Debug)]
enum Fault {
    None,
    EchoAlias,
    EchoOther,
    NoModel,
    NoChoices,
    NotJson,
    Status(u16),
    Dropped,
    Slow,
    NoCredential,
}

fn fault() -> impl Strategy<Value = Fault> {
    prop_oneof![
        Just(Fault::None),
        Just(Fault::EchoAlias),
        Just(Fault::EchoOther),
        Just(Fault::NoModel),
        Just(Fault::NoChoices),
        Just(Fault::NotJson),
        prop::sample::select(vec![401u16, 403, 404, 429, 500, 503]).prop_map(Fault::Status),
        Just(Fault::Dropped),
        Just(Fault::Slow),
        Just(Fault::NoCredential),
    ]
}

fn mode(fault: &Fault) -> ProbeMode {
    let reply = |body: serde_json::Value| ProbeMode::Reply { status: 200, body: body.to_string(), delay_ms: 0 };
    let choices = json!([{"index": 0, "message": {"role": "assistant", "content": "O"}, "finish_reason": "length"}]);
    match fault {
        Fault::None | Fault::NoCredential => ProbeMode::ok(MODEL),
        Fault::EchoAlias => ProbeMode::ok(ALIAS),
        Fault::EchoOther => ProbeMode::ok("someone-else"),
        Fault::NoModel => reply(json!({"object": "chat.completion", "choices": choices})),
        Fault::NoChoices => reply(json!({"object": "chat.completion", "model": MODEL, "choices": []})),
        Fault::NotJson => ProbeMode::Reply { status: 200, body: "<html>gateway</html>".into(), delay_ms: 0 },
        Fault::Status(status) => ProbeMode::Reply { status: *status, body: "{}".into(), delay_ms: 0 },
        Fault::Dropped => ProbeMode::Drop,
        Fault::Slow => match ProbeMode::ok(MODEL) {
            ProbeMode::Reply { status, body, .. } => ProbeMode::Reply { status, body, delay_ms: 1500 },
            other => other,
        },
    }
}

#[test]
fn probe_classifies_every_injected_fault_with_exactly_one_request() {
    let provider = Provider::standard();
    let mut value = policy(&provider.endpoint, "UNUSED");
    value["model"]["accepted_echo"] = json!([ALIAS]);
    value["probe"]["timeout_ms"] = json!(400);
    value["generation"]["generation_policy"] = json!("thinking_type_disabled");
    let policy: RunPolicy = serde_json::from_value(value).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig::with_cases(32));
    let result = runner.run(&fault(), |fault| {
        *provider.probe_mode.lock().unwrap() = mode(&fault);
        let before = provider.probe_requests.load(Ordering::SeqCst);
        let token = if matches!(fault, Fault::NoCredential) { "" } else { "probe-test-key" };
        let outcome = runtime.block_on(probe(&policy, token));
        let sent = provider.probe_requests.load(Ordering::SeqCst) - before;
        prop_assert_eq!(sent, usize::from(!matches!(fault, Fault::NoCredential)), "probe requests for {:?}", fault);
        match (&fault, &outcome) {
            (Fault::None, Ok(receipt)) => prop_assert_eq!(receipt.identity, "identical"),
            (Fault::EchoAlias, Ok(receipt)) => {
                prop_assert_eq!(receipt.identity, "accepted_alias_unverified");
                prop_assert_eq!(receipt.model_echo.as_str(), ALIAS);
            }
            (Fault::EchoOther, Err(ProbeFailure::ModelMismatch { echo, .. })) => {
                prop_assert_eq!(echo.as_str(), "someone-else")
            }
            (Fault::NoModel | Fault::NoChoices | Fault::NotJson, Err(ProbeFailure::ProtocolMismatch { .. })) => {}
            (Fault::Status(401 | 403), Err(ProbeFailure::AuthRejected { http_status })) => {
                prop_assert!(matches!(http_status, 401 | 403))
            }
            (Fault::Status(_), Err(ProbeFailure::Unreachable { .. })) => {}
            (Fault::Dropped, Err(ProbeFailure::Unreachable { .. })) => {}
            (Fault::Slow, Err(ProbeFailure::Timeout { timeout_ms: 400 })) => {}
            (Fault::NoCredential, Err(ProbeFailure::CredentialMissing)) => {}
            (fault, outcome) => prop_assert!(false, "{:?} gave {:?}", fault, outcome),
        }
        if let Err(failure) = &outcome {
            let shown = format!("{failure:?}");
            prop_assert!(!shown.contains("probe-test-key"));
        }
        Ok(())
    });
    if let Err(error) = result {
        panic!("{error}");
    }
    let bodies = provider.probe_bodies.lock().unwrap();
    for body in bodies.iter() {
        assert_eq!(body["max_tokens"], 1);
        assert_eq!(body["stream"], false);
        assert_eq!(body["model"], MODEL);
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    }
    assert!(provider.authorizations.lock().unwrap().iter().all(|value| value == "Bearer probe-test-key"));
    assert_eq!(provider.model_requests.load(Ordering::SeqCst), 0, "the probe is not a task Logical_Call");
}
