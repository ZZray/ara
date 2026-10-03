//! Reader-reachable admission for fixed OMP Session quota recovery.
//! Feedback belongs to the actual rejected request and settles independently;
//! this token cancels only preparation for a new reset or continuation.
use super::*;

#[derive(Default)]
pub(super) struct QuotaControl {
    current: Option<CancellationToken>,
    queued_stops: usize,
    input_closed: bool,
}

impl QuotaControl {
    pub(super) fn stops_quota(command: &Command) -> bool {
        match command.kind.as_str() {
            "abort" | "abort_retry" => true,
            "new_session" => command.frame.get("parentSession").is_none() || command.string("parentSession").is_ok(),
            "switch_session" => command.string("sessionPath").is_ok(),
            "branch" => command.string("entryId").is_ok(),
            "compact" => {
                command.frame.get("customInstructions").is_none() || command.string("customInstructions").is_ok()
            }
            // A plain prompt replaces the idle retry generation. Explicit
            // streaming queue forms retain their existing input semantics.
            "prompt" => command.frame.get("streamingBehavior").is_none() && command.message().is_ok(),
            "abort_and_prompt" => command.message().is_ok(),
            "set_auto_retry" => matches!(command.frame.get("enabled"), Some(WireValue::Bool(false))),
            _ => false,
        }
    }

    pub(super) fn queue_stop(&mut self) {
        self.queued_stops = self.queued_stops.saturating_add(1);
        if let Some(cancel) = &self.current {
            cancel.cancel();
        }
    }

    pub(super) fn consume_stop(&mut self) {
        self.queued_stops = self.queued_stops.saturating_sub(1);
    }

    pub(super) fn close_input(&mut self) {
        self.input_closed = true;
        if let Some(cancel) = &self.current {
            cancel.cancel();
        }
    }

    pub(super) fn stop_requested(&self) -> bool {
        self.queued_stops > 0 || self.input_closed
    }

    pub(super) fn begin(control: &Arc<Mutex<Self>>, connection: &CancellationToken) -> QuotaGuard {
        let cancel = connection.child_token();
        let mut state = control.lock().unwrap();
        // Installation and accepted input share one lock. A stop read before
        // the serial Host begins preparation must revoke its POST admission.
        state.current = Some(cancel.clone());
        if state.stop_requested() {
            cancel.cancel();
        }
        QuotaGuard { control: control.clone(), cancel }
    }
}

pub(super) struct QuotaGuard {
    control: Arc<Mutex<QuotaControl>>,
    pub(super) cancel: CancellationToken,
}

impl Drop for QuotaGuard {
    fn drop(&mut self) {
        self.control.lock().unwrap().current = None;
    }
}

impl Host {
    pub(super) async fn session_quota_outcome(
        &self,
        active: &ActiveRun,
        message: &AssistantMessage,
        end: RunEnd,
        persistence_ok: bool,
    ) -> Result<Option<super::super::session_quota_recovery::SessionQuotaOutcome>> {
        let Some(observed) = self.sessions.provider.recorded_session_quota(message) else { return Ok(None) };
        let guard = QuotaControl::begin(&self.quota_control, &self.connection);
        let mut class = ara_ai::retry_classification::classify_retry(message, &self.config.model.api);
        // The private proof came from the successfully settled request and
        // matches the complete Agent-retained terminal. The bridge still
        // requires durable feedback before admitting any account recovery.
        if class.usage_limit
            && message.failure_evidence.as_ref().is_some_and(|evidence| {
                evidence.context_recovery == Some(ara_ai::ContextRecoveryEvidence::UsageAdmission)
            })
        {
            class.replay_blocked = false;
        }
        let messages = self.agent.messages().await;
        let has_entry =
            active.sink.entries.lock().unwrap().iter().any(|(_, candidate)| candidate.as_assistant() == Some(message));
        let attempt = self.retry.as_ref().map_or(0, |retry| retry.attempt).saturating_add(1);
        let budget = super::super::rpc_host_retry::effective_max_retries(message, self.retry_policy.max_retries());
        let allow_recovery = persistence_ok
            && matches!(end, RunEnd::Error | RunEnd::Aborted)
            && !guard.cancel.is_cancelled()
            && !active.cancel.is_cancelled()
            && Arc::ptr_eq(&active.sink.session, &self.session)
            && self.retry_policy.enabled()
            && attempt as f64 <= budget
            && has_entry
            && super::super::rpc_host_retry::disposition(message, &messages, &class).is_some();
        let session_id = active.sink.session.header["id"].as_str().context("quota feedback Session has no ID")?;
        self.sessions
            .provider
            .recover_session_quota(observed, &self.config.model, session_id, allow_recovery, &guard.cancel)
            .await
            .map(Some)
    }
}
