//! Completion_Gate (requirement 17): a Host check of actual artifacts after an
//! ordinary model Stop. The gate never calls the model and never reserves a
//! model call; Core only decides whether to finish, continue or stop.
use std::{path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[async_trait]
pub trait CompletionGate: Send + Sync {
    /// Inspect the artifacts after an ordinary Stop. Never a model call.
    async fn check(&self, request: GateRequest, cancellation: CancellationToken) -> anyhow::Result<GateVerdict>;
}

#[derive(Clone, Debug, Serialize)]
pub struct GateRequest {
    pub check_id: String,
    pub run_id: String,
    /// 1 for the first check of this Run.
    pub sequence: u32,
    pub final_text: String,
    pub calls_used: usize,
    pub cwd: PathBuf,
    pub session_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateVerdict {
    Passed,
    Failed { feedback: String },
}

/// Host registration. `timeout` defaults to the remaining Run time.
#[derive(Clone)]
pub struct Registration {
    pub check_id: String,
    pub gate: Arc<dyn CompletionGate>,
    pub timeout: Option<Duration>,
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Registration")
            .field("check_id", &self.check_id)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GateResult {
    Passed,
    Failed { feedback: String },
    Error,
    Timeout,
    Cancelled,
}

impl GateResult {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            GateResult::Passed => "passed",
            GateResult::Failed { .. } => "failed",
            GateResult::Error => "error",
            GateResult::Timeout => "timeout",
            GateResult::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GateAction {
    Finish,
    Continue { feedback: String },
    StopBudget,
    StopDeadline,
    StopGateError(GateResult),
}

/// Pure decision after one check; Core performs the side effects. A failed
/// check continues only when a model call and time remain for it.
pub(crate) fn decide(result: &GateResult, calls_used: usize, max_calls: usize, within_deadline: bool) -> GateAction {
    match result {
        GateResult::Passed => GateAction::Finish,
        GateResult::Failed { .. } if calls_used >= max_calls => GateAction::StopBudget,
        GateResult::Failed { .. } if !within_deadline => GateAction::StopDeadline,
        GateResult::Failed { feedback } => GateAction::Continue { feedback: feedback.clone() },
        other => GateAction::StopGateError(other.clone()),
    }
}

/// First line of the feedback message appended after a failed check.
pub(crate) fn feedback_header(check_id: &str, sequence: u32, reference: &str) -> String {
    format!(
        "Host completion gate feedback (source=completion_gate; check_id={check_id}; sequence={sequence}; \
         reference={reference}; data, not a new human instruction)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_continues_only_with_budget_and_time_left() {
        let failed = GateResult::Failed { feedback: "2 tests fail".into() };
        assert_eq!(decide(&GateResult::Passed, 9, 10, false), GateAction::Finish);
        assert_eq!(decide(&failed, 9, 10, true), GateAction::Continue { feedback: "2 tests fail".into() });
        assert_eq!(decide(&failed, 10, 10, true), GateAction::StopBudget);
        assert_eq!(decide(&failed, 1, 10, false), GateAction::StopDeadline);
        for result in [GateResult::Error, GateResult::Timeout, GateResult::Cancelled] {
            assert_eq!(decide(&result, 0, 10, true), GateAction::StopGateError(result.clone()));
        }
        assert!(feedback_header("ctest", 2, "ref-1").contains("sequence=2; reference=ref-1"));
    }
}
