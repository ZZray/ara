//! Fixed OMP `session/stream-guards.ts:186-199,243-299`, 596f2da.
//! This observer owns only Gemini planning-header interruption. The generic
//! provider guard and the serial Host own detection/recovery respectively.

use ara_agent::{ActualProviderError, AgentMessage, AgentMessageBody, SessionValues};
use ara_ai::graph_values::GraphValueFactory;
use ara_ai::thinking_loop::GeminiHeaderRunDetector;
use ara_ai::tool_call_loop_guard::actual::PreparedActualTurn;
use ara_ai::tool_call_loop_guard::{RepeatedToolCallDetection, ToolCallLoopGuard, ToolCallLoopGuardOptions};
use ara_ai::{ActualAssistantEvent, AssistantMessage, AssistantMessageEvent, ToolResultMessage};
use ara_rpc::value_heap::{ValueError, ValueHandle, ValueSnapshot};
use std::sync::{Arc, Weak};

#[derive(Clone)]
pub(super) struct NativeHeaderTarget {
    pub(super) message: AgentMessage,
    pub(super) timestamp: f64,
}
impl NativeHeaderTarget {
    pub(super) fn capture(
        message: &AgentMessage,
        partial: &ValueHandle,
        values: &SessionValues,
    ) -> Result<Self, ActualProviderError> {
        let AgentMessageBody::Graph(root) = message.body_snapshot() else {
            return Err(ActualProviderError::Protocol("Gemini header target requires the original Graph message"));
        };
        values.check_handle(&root)?;
        message.recheck_stream_graph_root(&root)?;
        if !partial.same_identity(&root) {
            return Err(ActualProviderError::Protocol("Gemini header event and current message root differ"));
        }
        let observed = (|| {
            let timestamp = values.heap().get(&root, &"timestamp".into(), values.runtime())?;
            let ValueSnapshot::Number(timestamp) = values.heap().snapshot(&timestamp)? else {
                return Err(ActualProviderError::Protocol("Gemini header timestamp is not Number"));
            };
            Ok(Self { message: message.clone(), timestamp })
        })();
        let admission = message.recheck_stream_graph_root(&root).map_err(ActualProviderError::from);
        match (observed, admission) {
            (Ok(target), Ok(())) => Ok(target),
            (Err(primary), Err(secondary)) => {
                Err(ActualProviderError::Compound { primary: Arc::new(primary), secondary: Arc::new(secondary) })
            }
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        }
    }
}
#[derive(Clone)]
pub(super) enum HeaderTarget {
    Legacy(i64),
    Native(NativeHeaderTarget),
}
#[derive(Clone)]
pub(super) struct HeaderInterruption {
    pub(super) headers: usize,
    pub(super) target: HeaderTarget,
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
        self.allowed.then_some(self.hit.clone()).flatten()
    }

    /// Only detector state is touched while the Host holds its mutex.
    pub(super) fn observe_actual(&mut self, event: &ActualAssistantEvent) -> Option<usize> {
        if !self.enabled || !self.allowed || self.hit.is_some() {
            return None;
        }
        match event {
            AssistantMessageEvent::ThinkingStart { .. } => {
                self.detector = Some(GeminiHeaderRunDetector::default());
            }
            AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                if let Some(detector) = &mut self.detector
                    && detector.push_utf16(delta.units())
                {
                    return Some(detector.count());
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

    /// Original Graph Gets run outside the mutex before this final admission.
    pub(super) fn install_actual_hit(
        &mut self,
        headers: usize,
        target: NativeHeaderTarget,
    ) -> Option<HeaderInterruption> {
        if !self.enabled || !self.allowed || self.hit.is_some() {
            return None;
        }
        let hit = HeaderInterruption { headers, target: HeaderTarget::Native(target), generation: self.generation };
        self.hit = Some(hit.clone());
        Some(hit)
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
                        target: HeaderTarget::Legacy(message.timestamp),
                        generation: self.generation,
                    };
                    self.hit = Some(hit.clone());
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
    native_exempt_set: Option<ValueHandle>,
    native_factory: Option<Weak<GraphValueFactory>>,
    // A retained lease prevents ABA even if a reentrant callback resets the
    // settings and then restores their previous values. No counter overflow.
    revision: Arc<()>,
}

/// Cloneable state token, never a mutex guard. Graph callbacks run after the
/// Host releases its lock; a stale token is refused without replaying them.
pub(super) struct NativeLoopLease {
    revision: Arc<()>,
    requested_settings: super::ToolLoopGuardSettings,
    rebuild: bool,
    factory: Weak<GraphValueFactory>,
    pub(super) exempt_tools: Vec<String>,
    pub(super) exempt_set: Option<ValueHandle>,
}

impl ToolLoopState {
    pub(super) fn configure(&mut self, settings: super::ToolLoopGuardSettings) {
        if !settings.enabled {
            // A disable observation must also invalidate a pending first
            // Native Set construction while the installed state is empty.
            self.revision = Arc::new(());
            self.guard = None;
            self.settings = None;
            self.native_exempt_set = None;
            self.native_factory = None;
            return;
        }
        if self.settings.as_ref().is_none_or(|previous| !previous.same_key(&settings)) {
            self.revision = Arc::new(());
            self.native_exempt_set = None;
            self.native_factory = None;
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
        let guard = self.guard.as_mut()?;
        self.revision = Arc::new(());
        guard.record_turn(message, results)
    }

    pub(super) fn begin_actual(
        &mut self,
        settings: super::ToolLoopGuardSettings,
        factory: &Arc<GraphValueFactory>,
    ) -> Option<NativeLoopLease> {
        if !settings.enabled {
            self.configure(settings);
            return None;
        }
        let rebuild = self.settings.as_ref().is_none_or(|previous| !previous.same_key(&settings));
        // Original activeToolCallLoopGuard assigns guard/key only AFTER new
        // Set succeeds. A constructor Throw preserves the previous settings,
        // hash/count and Set. This pending lease owns no parallel detector.
        Some(NativeLoopLease {
            revision: self.revision.clone(),
            exempt_tools: settings.exempt_tools.clone(),
            exempt_set: if rebuild || !self.same_native_factory(factory) {
                None
            } else {
                self.native_exempt_set.clone()
            },
            requested_settings: settings,
            rebuild,
            factory: Arc::downgrade(factory),
        })
    }

    fn same_native_factory(&self, factory: &Arc<GraphValueFactory>) -> bool {
        self.native_factory.as_ref().and_then(Weak::upgrade).is_some_and(|current| Arc::ptr_eq(&current, factory))
    }

    fn actual_lease(&self) -> NativeLoopLease {
        NativeLoopLease {
            revision: self.revision.clone(),
            requested_settings: self.settings.as_ref().expect("enabled guard settings").clone(),
            rebuild: false,
            factory: self.native_factory.as_ref().expect("installed Native factory").clone(),
            exempt_tools: self.settings.as_ref().expect("enabled guard settings").exempt_tools.clone(),
            exempt_set: self.native_exempt_set.clone(),
        }
    }

    fn check_actual_lease(&self, lease: &NativeLoopLease) -> Result<(), ValueError> {
        if !Arc::ptr_eq(&self.revision, &lease.revision) {
            return Err(ValueError::WrongRuntimeReply("Tool-loop state changed during Native callback; no replay"));
        }
        Ok(())
    }

    /// Set construction may invoke Set.add/iteration callbacks. Root calls it
    /// outside the lock, then installs it here before preparing the turn.
    pub(super) fn install_actual_exempt_set(
        &mut self,
        lease: &NativeLoopLease,
        set: ValueHandle,
    ) -> Result<NativeLoopLease, ValueError> {
        self.check_actual_lease(lease)?;
        let factory =
            lease.factory.upgrade().ok_or(ValueError::WrongRuntimeReply("Native turn factory owner was released"))?;
        factory.heap().adopt(&set)?;
        if !lease.rebuild && self.native_exempt_set.is_some() && self.same_native_factory(&factory) {
            return Err(ValueError::WrongRuntimeReply("Native exempt Set is already installed"));
        }
        if lease.rebuild {
            self.guard = Some(ToolCallLoopGuard::new(ToolCallLoopGuardOptions {
                threshold: lease.requested_settings.threshold,
                exempt_tools: lease.requested_settings.exempt_tools.clone(),
            }));
            self.settings = Some(lease.requested_settings.clone());
        }
        // OMP has one shared JS realm. ARA new/switch uses fresh Session heaps;
        // replace only the realm-local Set after construction succeeds. The
        // long-lived detector hash/count survives a same-settings realm swap.
        self.native_exempt_set = Some(set);
        self.native_factory = Some(Arc::downgrade(&factory));
        self.revision = Arc::new(());
        Ok(self.actual_lease())
    }

    /// Only shared hash/count mutation occurs under the Host lock. A summary
    /// Throw in finish occurs later and cannot roll this commit back.
    pub(super) fn commit_actual_turn(
        &mut self,
        lease: &NativeLoopLease,
        prepared: &PreparedActualTurn,
    ) -> Result<Option<f64>, ValueError> {
        self.check_actual_lease(lease)?;
        let factory =
            lease.factory.upgrade().ok_or(ValueError::WrongRuntimeReply("Native turn factory owner was released"))?;
        if lease.rebuild
            || lease.exempt_set.is_none()
            || self.native_exempt_set.is_none()
            || self.guard.is_none()
            || !self.same_native_factory(&factory)
        {
            return Err(ValueError::WrongRuntimeReply("Native exempt Set must be initialized before turn prepare"));
        }
        let hit = self.guard.as_mut().expect("checked enabled guard").commit_actual_turn(prepared);
        self.revision = Arc::new(());
        Ok(hit)
    }
}
