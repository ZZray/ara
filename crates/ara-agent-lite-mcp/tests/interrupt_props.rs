//! Property 49: interrupted calls carry structured unknown effects unless the
//! tool is read-only, and are never re-sent (requirement 42.6).
mod common;

use std::time::Duration;

use ara_agent_lite_core::{effect_status::reports_unknown, rpi_agent::AgentError};
use common::*;
use proptest::prelude::*;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy)]
enum Interrupt {
    Timeout,
    Cancel,
    Disconnect,
}

fn run_case(kind: Interrupt, read_only: bool, at_ms: u64) -> Result<(), TestCaseError> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.txt");
        let mut opts = options();
        opts.call_timeout = match kind {
            Interrupt::Timeout => Duration::from_millis(at_ms),
            _ => Duration::from_secs(20),
        };
        let session = connect(config(vec![("fake", fake(&[], Some(&log), &[]))]), opts).await;
        let (name, args) = match (kind, read_only) {
            (Interrupt::Disconnect, false) => ("die", json!({"ms": at_ms})),
            (Interrupt::Disconnect, true) => ("die_ro", json!({"ms": at_ms})),
            (_, false) => ("slow", json!({"ms": at_ms + 3000})),
            (_, true) => ("slow_ro", json!({"ms": at_ms + 3000})),
        };
        let cancel = CancellationToken::new();
        if let Interrupt::Cancel = kind {
            let trigger = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(at_ms)).await;
                trigger.cancel();
            });
        }
        let outcome = run(&tool(&session, &format!("mcp__fake__{name}")), args, cancel).await;
        let source = match kind {
            Interrupt::Timeout => "mcp_call_timeout",
            Interrupt::Cancel => "mcp_call_cancelled",
            Interrupt::Disconnect => "mcp_connection_lost",
        };
        match (&outcome, read_only) {
            (Ok(result), false) => {
                prop_assert!(reports_unknown(&result.details));
                prop_assert_eq!(&result.details["effects_unknown"], &json!(true));
                prop_assert_eq!(&result.details["source"], &json!(source));
            }
            (Err(AgentError::Tool(text)), true) => prop_assert!(text.contains("read-only"), "{}", text),
            other => prop_assert!(false, "{:?} read_only={} {:?}", kind, read_only, other),
        }
        if !matches!(kind, Interrupt::Disconnect) {
            wait_for(&log, "notification notifications/cancelled").await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let lines = log_lines(&log);
        prop_assert_eq!(count(&lines, &format!("call {name} ")), 1, "{:?}", lines);
        prop_assert_eq!(count(&lines, "call "), 1, "{:?}", lines);
        session.shutdown().await;
        Ok(())
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn interrupted_calls_mark_unknown_effects(
        kind in prop_oneof![Just(Interrupt::Timeout), Just(Interrupt::Cancel), Just(Interrupt::Disconnect)],
        read_only in any::<bool>(),
        at_ms in 20u64..400,
    ) {
        run_case(kind, read_only, at_ms)?;
    }
}
