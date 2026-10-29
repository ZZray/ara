//! One-time capture of the pre-`LiteEvent` event samples (task 56.2).
//! Ignored by default; writes only with `LITE_EVENTS_V1_CAPTURE=write` and
//! refuses to overwrite an existing sample set. The compatibility tests read
//! the committed files and never regenerate them.
mod event_scenarios;
mod support;

use event_scenarios::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "one-time sample capture; see DR-P4-1"]
async fn capture_events_v1() {
    assert_eq!(std::env::var("LITE_EVENTS_V1_CAPTURE").as_deref(), Ok("write"), "explicit capture opt-in");
    // A preview directory may be given to inspect scenarios before the capture.
    let dir = std::env::var_os("LITE_EVENTS_V1_OUT").map_or_else(fixtures, std::path::PathBuf::from);
    assert!(!dir.exists(), "events-v1 samples already exist and are never regenerated");
    std::fs::create_dir_all(&dir).unwrap();
    let mut golden = serde_json::Map::new();
    for name in SCENARIOS {
        let runs = scenario(name).await;
        let mut lines = String::new();
        for event in runs.iter().flatten() {
            lines.push_str(&serde_json::to_string(event).unwrap());
            lines.push('\n');
        }
        std::fs::write(dir.join(format!("{name}.jsonl")), lines).unwrap();
        golden.insert((*name).into(), runs.iter().map(|events| shape(events)).collect());
    }
    let body = serde_json::to_string_pretty(&serde_json::json!({"version": 1, "scenarios": golden})).unwrap();
    std::fs::write(dir.join("execute-golden.json"), body + "\n").unwrap();
}
