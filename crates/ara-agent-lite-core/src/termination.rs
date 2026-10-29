//! Native termination evidence. This says nothing about external effect settlement
//! or business acceptance; a Host must independently check both.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetStopCause {
    CallBudgetExhausted,
}

/// Versioned evidence carried only by a failed `agent_end` after the exact
/// closed-tool-round call limit path. Ordinary final responses omit it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosedBudgetTermination {
    pub version: u32,
    pub cause: BudgetStopCause,
    pub native_round_closed: bool,
    pub model_final_stop: bool,
}

impl ClosedBudgetTermination {
    pub(crate) fn closed_tool_round() -> Self {
        Self {
            version: 1,
            cause: BudgetStopCause::CallBudgetExhausted,
            native_round_closed: true,
            model_final_stop: false,
        }
    }

    pub fn is_reviewable(&self) -> bool {
        self.version == 1
            && self.cause == BudgetStopCause::CallBudgetExhausted
            && self.native_round_closed
            && !self.model_final_stop
    }
}

/// Typed reason for a failed `agent_end`, carried as `stop_evidence` only on
/// the matching path (design “P2 能力缺口”). Says nothing about effects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cause", rename_all = "snake_case", deny_unknown_fields)]
pub enum StopEvidence {
    /// The final assistant message stopped at the output limit without tool calls.
    Length {
        version: u32,
        native_round_closed: bool,
        model_final_stop: bool,
        source_raw_index: usize,
        source_sha256: String,
        effective_output_tokens: u64,
    },
    /// A Stuck_Detector rule reached its threshold again after a correction.
    NoProgress {
        version: u32,
        native_round_closed: bool,
        model_final_stop: bool,
        rule: crate::stuck_detector::StuckRule,
        count: u32,
        threshold: u32,
        native_call_ids: Vec<String>,
    },
    /// No call was left in the Run budget for a needed compaction summary.
    SummaryBudgetExhausted { version: u32, run_calls: usize, run_max_calls: usize },
}

impl StopEvidence {
    /// Length and NoProgress evidence may be sent to Host review when the
    /// native round is closed; a summary-budget stop never is.
    pub fn is_reviewable(&self) -> bool {
        match self {
            StopEvidence::Length { version, native_round_closed, model_final_stop, .. }
            | StopEvidence::NoProgress { version, native_round_closed, model_final_stop, .. } => {
                *version == 1 && *native_round_closed && !*model_final_stop
            }
            StopEvidence::SummaryBudgetExhausted { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stop_evidence_wire_is_tagged_and_only_closed_rounds_are_reviewable() {
        let length = StopEvidence::Length {
            version: 1,
            native_round_closed: true,
            model_final_stop: false,
            source_raw_index: 7,
            source_sha256: "ab".into(),
            effective_output_tokens: 8192,
        };
        let wire = serde_json::to_value(&length).unwrap();
        assert_eq!(wire["cause"], "length");
        assert_eq!(serde_json::from_value::<StopEvidence>(wire).unwrap(), length);
        assert!(length.is_reviewable());
        let summary = StopEvidence::SummaryBudgetExhausted { version: 1, run_calls: 3, run_max_calls: 3 };
        assert_eq!(
            serde_json::to_value(&summary).unwrap(),
            json!({"cause":"summary_budget_exhausted","version":1,"run_calls":3,"run_max_calls":3})
        );
        assert!(!summary.is_reviewable());
        let open = StopEvidence::NoProgress {
            version: 1,
            native_round_closed: false,
            model_final_stop: false,
            rule: crate::stuck_detector::StuckRule::RepeatedCall,
            count: 3,
            threshold: 3,
            native_call_ids: vec!["c1".into()],
        };
        assert!(!open.is_reviewable());
        assert!(serde_json::from_value::<StopEvidence>(json!({"cause":"length","version":1})).is_err());
    }

    #[test]
    fn closed_budget_wire_is_versioned_and_never_claims_a_model_final_stop() {
        let proof = ClosedBudgetTermination::closed_tool_round();
        let wire = serde_json::to_value(&proof).unwrap();
        assert_eq!(
            wire,
            json!({"version":1,"cause":"call_budget_exhausted",
                               "native_round_closed":true,"model_final_stop":false})
        );
        assert!(serde_json::from_value::<ClosedBudgetTermination>(wire).unwrap().is_reviewable());
        for changed in [
            ClosedBudgetTermination { version: 2, ..proof.clone() },
            ClosedBudgetTermination { native_round_closed: false, ..proof.clone() },
            ClosedBudgetTermination { model_final_stop: true, ..proof },
        ] {
            assert!(!changed.is_reviewable());
        }
    }

    #[test]
    fn malformed_or_extended_proofs_are_not_silently_coerced() {
        for value in [
            json!({"version":1,"cause":"timeout","native_round_closed":true,"model_final_stop":false}),
            json!({"version":1,"cause":"call_budget_exhausted","native_round_closed":"true","model_final_stop":false}),
            json!({"version":1,"cause":"call_budget_exhausted","native_round_closed":true}),
            json!({"version":1,"cause":"call_budget_exhausted","native_round_closed":true,"model_final_stop":false,"effects_settled":true}),
            json!(null),
        ] {
            assert!(serde_json::from_value::<ClosedBudgetTermination>(value).is_err());
        }
    }
}
