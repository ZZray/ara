//! OMP b91 session/stream-guards.ts final EDIT preview validation.
//! MIT; THIRD_PARTY_NOTICES.md. Generated-file rejection belongs to the tool
//! result; previews with byte-identical content do not abort their Run.
use ara_agent::{ActualProviderError, AgentEvent, SessionValues};
use ara_edit::PathPolicy;
use ara_rpc::WireString;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
#[derive(Default)]
struct State {
    abort_triggered: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamingEditAbort {
    pub tool_call_id: WireString,
    pub path: String,
    pub error: String,
    pub generated: bool,
    origin_epoch: Option<u64>,
}
pub struct GeneratedEditPrecheck;
pub struct StreamingEditGuard {
    state: Mutex<State>,
    streaming_abort: AtomicBool,
}
impl StreamingEditGuard {
    pub fn new(_policy: PathPolicy, streaming_abort: bool) -> Self {
        Self {
            state: Mutex::new(State::default()),
            streaming_abort: AtomicBool::new(streaming_abort),
        }
    }
    pub fn set_policy(&self, _policy: PathPolicy, streaming_abort: bool) {
        self.streaming_abort
            .store(streaming_abort, Ordering::Release);
    }
    pub fn abort_triggered(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .abort_triggered
    }
    pub fn reset(&self) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = State::default();
    }
    pub fn pre_cache(
        &self,
        _event: &AgentEvent,
        _values: Option<&SessionValues>,
    ) -> Result<Option<GeneratedEditPrecheck>, ActualProviderError> {
        Ok(None)
    }
    pub async fn check_generated(
        &self,
        _check: GeneratedEditPrecheck,
    ) -> Result<Option<StreamingEditAbort>, ActualProviderError> {
        Ok(None)
    }
    pub fn publish_generated_abort(
        &self,
        _abort: &StreamingEditAbort,
        _publish: impl FnOnce(),
    ) -> bool {
        false
    }
    pub fn maybe_abort(&self, event: &AgentEvent) -> Option<StreamingEditAbort> {
        if !self.streaming_abort.load(Ordering::Acquire) {
            return None;
        }
        let AgentEvent::ToolStreamUpdate {
            tool_call_id,
            tool_name,
            update,
        } = event
        else {
            return None;
        };
        if tool_name != "edit" || update.get("streaming") != Some(&serde_json::Value::Bool(false)) {
            return None;
        }
        let (path, error) = update.get("files")?.as_array()?.iter().find_map(|file| {
            let path = file.get("path")?.as_str()?;
            let error = file.get("error")?.as_str()?;
            (!error.is_empty() && !error.starts_with("No changes would be made"))
                .then(|| (path.to_owned(), error.to_owned()))
        })?;
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.abort_triggered {
            return None;
        }
        state.abort_triggered = true;
        Some(StreamingEditAbort {
            tool_call_id: tool_call_id.as_str().into(),
            path,
            error,
            generated: false,
            origin_epoch: None,
        })
    }
}
#[cfg(test)]
#[path = "streaming_edit_guard_tests.rs"]
mod tests;
