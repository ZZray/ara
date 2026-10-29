//! Soft tool directives from fixed OMP Agent types and loop at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; THIRD_PARTY_NOTICES.md).
//! The loop owns event ordering; this module owns only the shared lifecycle.

use crate::agent_loop::{RunEnd, RunStopCause};
use crate::agent_message::AgentMessage;
use crate::tool_directives::{ToolChoiceSelection, ToolDirectiveError};
use ara_ai::{ToolCall, ToolChoice};
use std::sync::{Arc, Mutex};

pub type SoftToolPredicate = Arc<dyn Fn(&ToolCall) -> bool + Send + Sync>;

/// A host reminder is inserted once when its ID becomes active. An unchanged
/// ID keeps its escalation count, including across a successful owner stop.
#[derive(Clone)]
pub struct SoftToolRequirement {
    pub id: String,
    pub tool_name: String,
    pub satisfies: Option<SoftToolPredicate>,
    pub reminder: Vec<AgentMessage>,
}

impl std::fmt::Debug for SoftToolRequirement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoftToolRequirement")
            .field("id", &self.id)
            .field("tool_name", &self.tool_name)
            .field("has_predicate", &self.satisfies.is_some())
            .field("reminder", &self.reminder)
            .finish()
    }
}

/// Additive full hook result. Existing hard-only hooks keep their default
/// adapter and do not acquire a new required configuration field.
#[derive(Clone, Debug)]
pub enum ToolChoiceDirective {
    Hard(ToolChoice),
    Soft(SoftToolRequirement),
}

#[derive(Clone, Debug, Default)]
pub struct ToolChoiceDirectiveSelection {
    pub directive: Option<ToolChoiceDirective>,
    pub static_fallback: Option<ToolChoice>,
    /// Agent facade already resolves queued Soft authoritatively. Direct loop
    /// hooks leave this false and keep Native config.toolChoice hard blocks.
    pub suppress_static_soft_block: bool,
}

impl From<ToolChoiceSelection> for ToolChoiceDirectiveSelection {
    fn from(hard: ToolChoiceSelection) -> Self {
        Self {
            directive: hard.directive.map(ToolChoiceDirective::Hard),
            static_fallback: hard.static_fallback,
            suppress_static_soft_block: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoftToolRequirementSnapshot {
    pub id: Option<String>,
    pub forced_tool_choice: Option<ToolChoice>,
    pub escalations: usize,
}

/// No callback, model request, or event emission runs under this mutex.
#[derive(Clone, Debug, Default)]
pub struct SoftToolRequirementState(Arc<Mutex<SoftToolRequirementSnapshot>>);

impl SoftToolRequirementState {
    pub fn snapshot(&self) -> SoftToolRequirementSnapshot {
        self.0.lock().unwrap().clone()
    }

    pub fn clear(&self) {
        *self.0.lock().unwrap() = SoftToolRequirementSnapshot::default();
    }

    pub(crate) fn activate(&self, requirement: Option<&SoftToolRequirement>) -> Vec<AgentMessage> {
        let mut state = self.0.lock().unwrap();
        let Some(requirement) = requirement else {
            *state = SoftToolRequirementSnapshot::default();
            return Vec::new();
        };
        if state.id.as_deref() == Some(&requirement.id) {
            return Vec::new();
        }
        *state = SoftToolRequirementSnapshot { id: Some(requirement.id.clone()), ..Default::default() };
        // AgentMessage clones share the original body/source allocation.
        requirement.reminder.clone()
    }

    pub(crate) fn effective_choice(&self, selection: &ToolChoiceSelection) -> Option<ToolChoice> {
        selection
            .directive
            .clone()
            .or_else(|| self.0.lock().unwrap().forced_tool_choice.clone())
            .or_else(|| selection.static_fallback.clone())
    }

    pub(crate) fn assistant_finalized(&self) {
        self.0.lock().unwrap().forced_tool_choice = None;
    }

    /// Invoke the predicate with no state lock held. The loop catches host
    /// panics at its existing callback boundary before executing any tools.
    pub(crate) fn noncompliant(
        &self,
        requirement: Option<&SoftToolRequirement>,
        calls: &[ToolCall],
        admitted_static_choice: Option<&ToolChoice>,
    ) -> Result<Option<String>, ToolDirectiveError> {
        let Some(requirement) = requirement else {
            return Ok(None);
        };
        let compliant = !calls.is_empty()
            && calls.iter().all(|call| {
                requirement.satisfies.as_ref().map_or(call.name == requirement.tool_name, |predicate| predicate(call))
            });
        if hard_tool_choice_blocks(admitted_static_choice, &requirement.tool_name) || compliant {
            return Ok(None);
        }
        const MAX_SOFT_TOOL_ESCALATIONS: usize = 3;
        if self.0.lock().unwrap().escalations >= MAX_SOFT_TOOL_ESCALATIONS {
            return Err(ToolDirectiveError(format!(
                "Soft tool requirement '{}' was not satisfied after {MAX_SOFT_TOOL_ESCALATIONS} forced turns; aborting to avoid an unbounded force loop.",
                requirement.tool_name,
            )));
        }
        Ok(Some(format!(
            "Not executed: call the `{}` tool to resolve the pending action before using other tools.",
            requirement.tool_name,
        )))
    }

    /// Called after all skipped results are committed, matching Native order.
    pub(crate) fn record_escalation(&self, tool_name: &str) {
        let mut state = self.0.lock().unwrap();
        state.forced_tool_choice = Some(ToolChoice::Tool(tool_name.into()));
        state.escalations += 1;
    }
}

/// Fixed Native hardToolChoiceBlocks uses the logical required name. Computer
/// marker/capability admission is a separate provider/active-tool contract.
pub(crate) fn hard_tool_choice_blocks(choice: Option<&ToolChoice>, required_tool: &str) -> bool {
    match choice {
        None | Some(ToolChoice::Auto | ToolChoice::Any | ToolChoice::Required) => false,
        Some(ToolChoice::None) => true,
        Some(ToolChoice::Function(name) | ToolChoice::FunctionNested(name) | ToolChoice::Tool(name)) => {
            name != required_tool
        }
        Some(ToolChoice::Computer) => required_tool != "computer",
    }
}

pub(crate) struct SoftToolRequirementRunGuard {
    state: SoftToolRequirementState,
    retain: bool,
}

impl SoftToolRequirementRunGuard {
    pub(crate) fn new(state: SoftToolRequirementState) -> Self {
        Self { state, retain: false }
    }

    /// Native's finally exits before resetting shared Soft state when an aside
    /// discard fails. Only that new observable failure path bypasses this reset.
    pub(crate) fn retain_after_aside_discard_failure(mut self) {
        self.retain = true;
    }

    pub(crate) fn finish(mut self, end: RunEnd, stop_cause: &Option<RunStopCause>) {
        self.retain = end == RunEnd::PreModelStop && matches!(stop_cause, Some(RunStopCause::PreModelStop { .. }));
    }
}

impl Drop for SoftToolRequirementRunGuard {
    fn drop(&mut self) {
        if !self.retain {
            self.state.clear();
        }
    }
}
