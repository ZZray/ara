//! Fixed OMP `session/stream-guards.ts:186-199,243-299`, 596f2da.
//! This observer owns only Gemini planning-header interruption. The generic
//! provider guard and the serial Host own detection/recovery respectively.

use ara_ai::thinking_loop::GeminiHeaderRunDetector;
use ara_ai::tool_call_loop_guard::{RepeatedToolCallDetection, ToolCallLoopGuard, ToolCallLoopGuardOptions};
use ara_ai::{AssistantMessage, AssistantMessageEvent, ToolResultMessage};

#[derive(Clone, Copy)]
pub(super) struct HeaderInterruption {
    pub(super) headers: usize,
    pub(super) target_timestamp: i64,
    pub(super) generation: u64,
}

pub(super) struct GeminiHeaderGuard {
    enabled: bool,
    allowed: bool,
    generation: u64,
    detector: Option<GeminiHeaderRunDetector>,
    hit: Option<HeaderInterruption>,
}

impl GeminiHeaderGuard {
    pub(super) fn new(enabled: bool, generation: u64) -> Self {
        Self { enabled, allowed: true, generation, detector: None, hit: None }
    }

    pub(super) fn revoke(&mut self) {
        self.allowed = false;
        self.detector = None;
    }

    pub(super) fn hit(&self) -> Option<HeaderInterruption> {
        self.allowed.then_some(self.hit).flatten()
    }

    pub(super) fn observe(
        &mut self,
        message: &AssistantMessage,
        event: &AssistantMessageEvent,
    ) -> Option<HeaderInterruption> {
        if !self.enabled || !self.allowed || self.hit.is_some() {
            return None;
        }
        match event {
            AssistantMessageEvent::ThinkingStart { .. } => {
                self.detector = Some(GeminiHeaderRunDetector::default());
            }
            AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                if let Some(detector) = &mut self.detector
                    && detector.push(delta)
                {
                    let hit = HeaderInterruption {
                        headers: detector.count(),
                        target_timestamp: message.timestamp,
                        generation: self.generation,
                    };
                    self.hit = Some(hit);
                    return Some(hit);
                }
            }
            AssistantMessageEvent::TextStart { .. } | AssistantMessageEvent::ToolcallStart { .. } => {
                if let Some(detector) = &mut self.detector {
                    detector.reset();
                }
            }
            _ => {}
        }
        None
    }
}

pub(super) fn guard_enabled(settings: super::LoopGuardSettings) -> bool {
    settings.enabled && std::env::var("ARA_NO_THINKING_LOOP_GUARD").as_deref() != Ok("1")
}

/// Fixed AgentSession constructs LoopGuards once. Preserve this state across
/// prompt/new/switch; disable or a raw settings-key change rebuilds it.
#[derive(Default)]
pub(super) struct ToolLoopState {
    settings: Option<super::ToolLoopGuardSettings>,
    guard: Option<ToolCallLoopGuard>,
}

impl ToolLoopState {
    pub(super) fn configure(&mut self, settings: super::ToolLoopGuardSettings) {
        if !settings.enabled {
            self.guard = None;
            self.settings = None;
            return;
        }
        if self.settings.as_ref().is_none_or(|previous| !previous.same_key(&settings)) {
            self.guard = Some(ToolCallLoopGuard::new(ToolCallLoopGuardOptions {
                threshold: settings.threshold,
                exempt_tools: settings.exempt_tools.clone(),
            }));
            self.settings = Some(settings);
        }
    }

    pub(super) fn record_turn(
        &mut self,
        message: &AssistantMessage,
        results: &[ToolResultMessage],
    ) -> Option<RepeatedToolCallDetection> {
        self.guard.as_mut()?.record_turn(message, results)
    }
}
