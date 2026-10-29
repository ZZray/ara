//! Host opt-in Run policies (design “P2 能力缺口”). `Default` turns every
//! policy off and keeps existing event and Session sequences unchanged.
use serde_json::{json, Value};

#[derive(Clone, Default, Debug)]
pub struct RunPolicies {
    pub completion_gate: Option<crate::completion_gate::Registration>,
    pub stuck: Option<crate::stuck_detector::StuckPolicy>,
    pub compaction_refresh: Option<crate::compaction_refresh::RefreshPolicy>,
    /// Harness-owned: emit typed Length stop evidence on `agent_end`.
    pub length_stop_evidence: bool,
}

impl RunPolicies {
    pub fn is_default(&self) -> bool {
        self.completion_gate.is_none()
            && self.stuck.is_none()
            && self.compaction_refresh.is_none()
            && !self.length_stop_evidence
    }

    /// Secret-free declaration for Harness authority hashing; `None` when
    /// no authority-relevant policy is set. `length_stop_evidence` is a
    /// Harness reporting switch and is not declared.
    pub fn declaration(&self) -> Option<Value> {
        if self.completion_gate.is_none() && self.stuck.is_none() && self.compaction_refresh.is_none() {
            return None;
        }
        Some(json!({
            "completion_gate": self.completion_gate.as_ref().map(|gate| json!({"check_id": gate.check_id,
                "timeout_ms": gate.timeout.map(|timeout| timeout.as_millis() as u64)})),
            "stuck": self.stuck.as_ref().map(crate::stuck_detector::StuckPolicy::declaration),
            "compaction_refresh": self.compaction_refresh.as_ref().map(crate::compaction_refresh::RefreshPolicy::declaration),
        }))
    }

    /// Called before any upstream request.
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        if let Some(stuck) = &self.stuck {
            stuck.validate()?;
            anyhow::ensure!(stuck.is_configured(), "stuck_detector policy configures no threshold");
        }
        if let Some(refresh) = &self.compaction_refresh {
            refresh.validate()?;
        }
        if let Some(gate) = &self.completion_gate {
            anyhow::ensure!(!gate.check_id.trim().is_empty(), "completion gate check_id is required");
            anyhow::ensure!(
                gate.timeout.is_none_or(|timeout| !timeout.is_zero()),
                "completion gate timeout must be positive"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_declares_nothing_and_validates() {
        let policies = RunPolicies::default();
        assert!(policies.is_default());
        assert!(policies.declaration().is_none());
        policies.validate().unwrap();
        let reporting = RunPolicies { length_stop_evidence: true, ..RunPolicies::default() };
        assert!(!reporting.is_default() && reporting.declaration().is_none());
        let bad = RunPolicies {
            stuck: Some(crate::stuck_detector::StuckPolicy { repeated_call: Some(1), ..Default::default() }),
            ..RunPolicies::default()
        };
        assert!(bad.validate().is_err());
        let refresh = RunPolicies {
            compaction_refresh: Some(crate::compaction_refresh::RefreshPolicy::new(4, 8192)),
            ..RunPolicies::default()
        };
        assert!(refresh.declaration().unwrap()["compaction_refresh"]["max_files"] == 4);
    }
}
