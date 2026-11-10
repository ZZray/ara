//! End-to-end execute coverage for Host steering admission and the next model turn.

mod support;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use ara_agent_lite_core::steering::SteeringBuffer;
use serde_json::json;

use support::{request, run, text, Provider, TempDir};

#[tokio::test]
async fn host_steering_is_admitted_into_the_next_execute_turn_with_a_receipt() {
    let root = TempDir::new();
    let steering = Arc::new(Mutex::new(SteeringBuffer::default()));
    let queued = steering.clone();
    let requests = Arc::new(AtomicUsize::new(0));
    let request_count = requests.clone();
    let provider = Provider::with(support::ProviderConfig {
        on_request: Box::new(move |_| {
            if request_count.fetch_add(1, Ordering::SeqCst) == 0 {
                queued
                    .lock()
                    .unwrap()
                    .submit(
                        "Host steering: include the updated sample state.".into(),
                        json!({"id":"steer-1","source":"host"}),
                    )
                    .unwrap();
            }
        }),
        ..support::ProviderConfig::new(vec![text("initial answer"), text("updated answer")])
    });
    let mut run_request = request(root.path(), &provider, &[], 3, "Report the sample state.");
    run_request.steering = steering.clone();

    let (result, _events) = run(run_request).await;
    assert_eq!(result.code, 0, "steering fixture must finish the Run successfully: code={}", result.code);
    let bodies = provider.bodies();
    assert!(bodies.len() >= 2, "steering should cause a follow-up model request");
    assert!(bodies[1].to_string().contains("Host steering: include the updated sample state."));

    let receipts = steering.lock().unwrap().outcomes_snapshot();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["id"], "steer-1");
    assert_eq!(receipts[0]["status"], "applied");
}
