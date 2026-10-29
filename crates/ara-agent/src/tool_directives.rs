//! Bounded hard tool-choice ownership from OMP `packages/agent/src/agent.ts`
//! at 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; see THIRD_PARTY_NOTICES.md).
//!
//! Hard choices retain all eight Native input shapes. Availability is checked
//! against admitted logical names and the authored native computer marker.
//! Soft requirements remain a separate required port.

use crate::agent_loop::{RunEnd, RunReport, RunStopCause};
use crate::tool::AgentTool;
use ara_ai::{NativeToolMarker, ToolChoice};
use std::sync::{Arc, Mutex};

/// One logical-turn selection. Direct loops inherit the admitted static choice
/// when there is no dynamic directive. The Agent facade returns an authoritative
/// filtered choice with no fallback, including when its choice is unavailable.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolChoiceSelection {
    pub directive: Option<ToolChoice>,
    pub static_fallback: Option<ToolChoice>,
}

impl ToolChoiceSelection {
    pub fn inherit(admitted_static_choice: Option<&ToolChoice>) -> Self {
        Self { directive: None, static_fallback: admitted_static_choice.cloned() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolDirectiveError(pub String);

impl std::fmt::Display for ToolDirectiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ToolDirectiveError {}

pub(crate) fn active_tool_choice(choice: ToolChoice, active_tools: &[Arc<dyn AgentTool>]) -> Option<ToolChoice> {
    // Logical names, not a custom provider wire alias.
    match &choice {
        ToolChoice::Function(name) | ToolChoice::FunctionNested(name) | ToolChoice::Tool(name)
            if !active_tools.iter().any(|tool| tool.definition().name == *name) =>
        {
            None
        }
        ToolChoice::Computer
            if !active_tools.iter().any(|tool| tool.definition().native == Some(NativeToolMarker::Computer)) =>
        {
            None
        }
        _ => Some(choice),
    }
}

struct DeferredHardChoice {
    choice: ToolChoice,
    origin: Arc<()>,
}

#[derive(Default)]
pub(crate) struct DeferredToolChoices {
    retained: Mutex<Option<DeferredHardChoice>>,
}
impl DeferredToolChoices {
    pub(crate) fn take(&self) -> Option<ToolChoice> {
        self.retained.lock().unwrap().take().map(|deferred| deferred.choice)
    }
    pub(crate) fn clear(&self) {
        *self.retained.lock().unwrap() = None;
    }
}

/// A private origin allocation identifies only this Run's staged choice. It
/// does not fence Host mutations or introduce a new catalogue/version owner.
pub(crate) struct RunToolChoiceClaim {
    owner: Arc<DeferredToolChoices>,
    origin: Arc<()>,
    claimed: Mutex<Option<ToolChoice>>,
}
impl RunToolChoiceClaim {
    pub(crate) fn new(owner: Arc<DeferredToolChoices>) -> Arc<Self> {
        Arc::new(Self { owner, origin: Arc::new(()), claimed: Mutex::new(None) })
    }
    pub(crate) fn clear_claim(&self) {
        *self.claimed.lock().unwrap() = None;
    }
    pub(crate) fn claim(&self, choice: &ToolChoice) {
        *self.claimed.lock().unwrap() = Some(choice.clone());
    }
    pub(crate) fn stage_rejected(&self) {
        let choice = self.claimed.lock().unwrap().clone();
        if let Some(choice) = choice {
            *self.owner.retained.lock().unwrap() = Some(DeferredHardChoice { choice, origin: self.origin.clone() });
        }
    }
    fn clear_staged(&self) {
        let mut retained = self.owner.retained.lock().unwrap();
        if retained.as_ref().is_some_and(|choice| Arc::ptr_eq(&choice.origin, &self.origin)) {
            *retained = None;
        }
    }
}

/// Early Result returns and panic unwinding clear only this Run's staged slot.
/// A successful owner stop is committed after the awaited final Run receipt,
/// so cancellation during input/AgentEnd emission cannot retain the choice.
pub(crate) struct RunToolChoiceGuard {
    claim: Arc<RunToolChoiceClaim>,
    retain: bool,
}
impl RunToolChoiceGuard {
    pub(crate) fn new(claim: Arc<RunToolChoiceClaim>) -> Self {
        Self { claim, retain: false }
    }
    pub(crate) fn finish(mut self, report: &RunReport) {
        self.retain =
            report.end == RunEnd::PreModelStop && matches!(report.stop_cause, Some(RunStopCause::PreModelStop { .. }));
    }
}
impl Drop for RunToolChoiceGuard {
    fn drop(&mut self) {
        self.claim.clear_claim();
        if !self.retain {
            self.claim.clear_staged();
        }
    }
}
