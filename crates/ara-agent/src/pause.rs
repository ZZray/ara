//! Fixed OMP process-wide pause gate (`packages/agent/src/pause.ts`,
//! 596f2da7101178214aa27a753529d15e6b7ad91d; MIT, THIRD_PARTY_NOTICES.md).
//! Hosts control the singleton. In-flight work is never cancelled by pausing.

use std::sync::{Arc, LazyLock, Mutex, Weak};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub type AgentPauseListener = Arc<dyn Fn(bool) + Send + Sync>;

#[derive(Default)]
struct PauseState {
    paused: bool,
    paused_at: i64,
    listeners: Vec<AgentPauseListener>,
}

#[derive(Default)]
struct PauseInner {
    state: Mutex<PauseState>,
    resumed: Notify,
}

#[derive(Clone, Default)]
pub struct AgentPauseGate(Arc<PauseInner>);

pub struct PauseSubscription {
    gate: Weak<PauseInner>,
    listener: AgentPauseListener,
}

impl PauseSubscription {
    pub fn unsubscribe(&self) {
        if let Some(gate) = self.gate.upgrade() {
            gate.state.lock().unwrap().listeners.retain(|listener| !Arc::ptr_eq(listener, &self.listener));
        }
    }
}

impl Drop for PauseSubscription {
    fn drop(&mut self) {
        self.unsubscribe();
    }
}

impl AgentPauseGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn paused(&self) -> bool {
        self.0.state.lock().unwrap().paused
    }

    pub fn paused_at(&self) -> Option<i64> {
        let state = self.0.state.lock().unwrap();
        state.paused.then_some(state.paused_at)
    }

    pub fn pause(&self) -> bool {
        let listeners = {
            let mut state = self.0.state.lock().unwrap();
            if state.paused {
                return false;
            }
            state.paused = true;
            state.paused_at = ara_ai::now_ms();
            state.listeners.clone()
        };
        Self::notify(listeners, true);
        true
    }

    pub fn resume(&self) -> Option<i64> {
        let listeners = {
            let mut state = self.0.state.lock().unwrap();
            if !state.paused {
                return None;
            }
            state.paused = false;
            state.listeners.clone()
        };
        self.0.resumed.notify_waiters();
        Self::notify(listeners, false);
        // Native reads the last pause timestamp after listeners: a reentrant
        // listener can pause again. Retain that ordering rather than snapshot it.
        Some(ara_ai::now_ms().saturating_sub(self.0.state.lock().unwrap().paused_at))
    }

    pub fn on_change(&self, listener: AgentPauseListener) -> PauseSubscription {
        {
            let mut state = self.0.state.lock().unwrap();
            if !state.listeners.iter().any(|entry| Arc::ptr_eq(entry, &listener)) {
                state.listeners.push(listener.clone());
            }
        }
        PauseSubscription { gate: Arc::downgrade(&self.0), listener }
    }

    fn notify(listeners: Vec<AgentPauseListener>, paused: bool) {
        // No state/listener lock survives a callback, including its unwind.
        for listener in listeners {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener(paused)));
        }
    }

    pub async fn wait_until_resumed(&self, cancel: Option<&CancellationToken>) {
        loop {
            let notified = self.0.resumed.notified();
            tokio::pin!(notified);
            // Register before reading state, closing the resume-before-await gap.
            notified.as_mut().enable();
            if !self.paused() || cancel.is_some_and(CancellationToken::is_cancelled) {
                return;
            }
            if let Some(cancel) = cancel {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return,
                    _ = &mut notified => {},
                }
            } else {
                notified.await;
            }
            // A resume immediately followed by pause must repark every waiter.
        }
    }
}

pub static AGENT_PAUSE_GATE: LazyLock<AgentPauseGate> = LazyLock::new(AgentPauseGate::new);
