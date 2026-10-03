//! Private live Session quota receipts and owned rejection feedback.
//! Fixed OMP turn-recovery.ts::recordUsageLimitForAssistant and its sibling /
//! reset / earliest-wait order at 596f2da7101178214aa27a753529d15e6b7ad91d.
//! MIT; see THIRD_PARTY_NOTICES.md. No lease or request ID enters a journal.
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.

use ara_ai::{AssistantMessage, Model};
use ara_cli::{
    auth_storage::{AuthRequestContext, AuthStorage, RecordedUsageLimit},
    codex_reset_controller::{CodexResetContext, CodexResetHost},
    model_route::{
        CredentialIdentity, ReceiptError, RequestAuthFailure, RequestAuthFailureKind, RequestAuthLease,
        RequestAuthResolver, RequestIdentity, RequestReceiptObserver,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    sync::{Arc, LazyLock, Mutex},
};
use tokio::{sync::watch, task::JoinHandle};
use tokio_util::sync::CancellationToken;

/// This capability cannot be serialized, logged or constructed by a Host
/// journal read. Only an unambiguous live provider terminal can create it.
pub(crate) struct ObservedQuota {
    pending: Arc<PendingQuota>,
}

#[derive(Default)]
pub(crate) struct SessionQuotaOutcome {
    pub recorded: bool,
    pub switched: bool,
    pub wait_ms: Option<f64>,
}

#[derive(Clone)]
pub(crate) struct QuotaBinding {
    pub model: Model,
    pub generation: u64,
    pub logical_session: Option<String>,
    pub context: AuthRequestContext,
    pub reset_context: CodexResetContext,
    pub host: Arc<super::ReferenceResetHost>,
    pub account: Arc<AuthStorage>,
    pub resolver: Arc<dyn RequestAuthResolver>,
}

#[derive(Clone, Copy)]
enum FeedbackState {
    Pending,
    Settled(Option<RecordedUsageLimit>),
    Failed,
}

struct PendingQuota {
    terminal: AssistantMessage,
    binding: Arc<QuotaBinding>,
    lease: RequestAuthLease,
    unblock_at_ms: f64,
    feedback: watch::Receiver<FeedbackState>,
}

#[derive(Default)]
struct OwnerState {
    records: BTreeMap<uuid::Uuid, Arc<PendingQuota>>,
    seen: BTreeSet<uuid::Uuid>,
    jobs: Vec<JoinHandle<()>>,
    closing: bool,
}

#[derive(Default)]
pub(crate) struct SessionQuotaRecovery {
    state: Mutex<OwnerState>,
}

// Factory errors can leave its local scope before the normal CLI/RPC drain.
// Keep already owned jobs reachable by run() until the account owner closes.
static ORPHANED_FEEDBACK: LazyLock<Mutex<Vec<JoinHandle<()>>>> = LazyLock::new(|| Mutex::new(Vec::new()));

impl Drop for SessionQuotaRecovery {
    fn drop(&mut self) {
        let jobs = std::mem::take(&mut self.state.get_mut().unwrap().jobs);
        ORPHANED_FEEDBACK.lock().unwrap().extend(jobs);
    }
}

pub(crate) async fn settle_orphaned_feedback() {
    loop {
        let jobs = std::mem::take(&mut *ORPHANED_FEEDBACK.lock().unwrap());
        if jobs.is_empty() {
            return;
        }
        for job in jobs {
            let _ = job.await;
        }
    }
}

impl SessionQuotaRecovery {
    pub fn observer(self: &Arc<Self>, binding: QuotaBinding) -> Arc<dyn RequestReceiptObserver> {
        Arc::new(QuotaObserver {
            owner: self.clone(),
            binding: Arc::new(binding),
            requests: Mutex::new(BTreeMap::new()),
        })
    }

    /// Match the actual retained message, including the Agent's sole permitted
    /// unfinished-tool normalization. Similar timestamps/models are not keys.
    pub fn recorded(&self, message: &AssistantMessage) -> Option<ObservedQuota> {
        let mut state = self.state.lock().unwrap();
        let matches: Vec<uuid::Uuid> = state
            .records
            .iter()
            .filter_map(|(id, pending)| {
                (pending.binding.host.owns_context(&pending.binding.reset_context)
                    && matches_retained(&pending.terminal, message))
                .then_some(*id)
            })
            .collect();
        if matches.len() != 1 {
            return None;
        }
        state.records.remove(&matches[0]).map(|pending| ObservedQuota { pending })
    }

    pub async fn settle(&self) {
        let jobs = {
            let mut state = self.state.lock().unwrap();
            state.closing = true;
            state.records.clear();
            std::mem::take(&mut state.jobs)
        };
        for job in jobs {
            let _ = job.await;
        }
    }
}

struct QuotaObserver {
    owner: Arc<SessionQuotaRecovery>,
    binding: Arc<QuotaBinding>,
    requests: Mutex<BTreeMap<uuid::Uuid, Arc<QuotaBinding>>>,
}

impl RequestReceiptObserver for QuotaObserver {
    fn request_auth(&self, call_id: uuid::Uuid) -> Result<Option<Arc<dyn RequestAuthResolver>>, ReceiptError> {
        let mut binding = self.binding.as_ref().clone();
        binding.reset_context = binding.host.context_for_request(&binding.reset_context).ok_or(ReceiptError)?;
        binding.resolver = binding
            .context
            .session_id
            .as_deref()
            .and_then(|id| binding.resolver.for_session(id))
            .unwrap_or_else(|| binding.resolver.clone());
        let auth = binding.resolver.clone();
        self.requests.lock().unwrap().insert(call_id, Arc::new(binding));
        Ok(Some(auth))
    }
    fn started(&self, request: &RequestIdentity) -> Result<(), ReceiptError> {
        let requests = self.requests.lock().unwrap();
        let binding = requests.get(&request.call_id).ok_or(ReceiptError)?;
        if request.provider != binding.model.provider
            || request.model_id != binding.model.id
            || request.route_generation != binding.generation
        {
            return Err(ReceiptError);
        }
        Ok(())
    }
    fn retry_started(&self, previous: &RequestIdentity, request: &RequestIdentity) -> Result<(), ReceiptError> {
        let mut requests = self.requests.lock().unwrap();
        let binding = requests.remove(&previous.call_id).ok_or(ReceiptError)?;
        if request.provider != binding.model.provider
            || request.model_id != binding.model.id
            || request.route_generation != binding.generation
        {
            return Err(ReceiptError);
        }
        requests.insert(request.call_id, binding);
        Ok(())
    }
    fn settled(&self, _: &RequestIdentity, _: &AssistantMessage) -> Result<(), ReceiptError> {
        Ok(())
    }
    fn interrupted(&self, request: &RequestIdentity) {
        self.finished(request.call_id);
    }
    fn finished(&self, call_id: uuid::Uuid) {
        self.requests.lock().unwrap().remove(&call_id);
    }

    fn quota_rejected(
        &self,
        request: &RequestIdentity,
        lease: &RequestAuthLease,
        message: &AssistantMessage,
    ) -> Result<bool, ReceiptError> {
        let binding = self.requests.lock().unwrap().remove(&request.call_id).ok_or(ReceiptError)?;
        if request.provider != binding.model.provider
            || request.model_id != binding.model.id
            || request.route_generation != binding.generation
            || request.credential != *lease.identity()
        {
            return Err(ReceiptError);
        }
        // Runtime/config/environment leases have no ownership of a saved row.
        if !matches!(lease.identity(), CredentialIdentity::Stored { .. }) {
            return Ok(false);
        }
        let mut state = self.owner.state.lock().unwrap();
        if state.closing {
            return Err(ReceiptError);
        }
        if !state.seen.insert(request.call_id) {
            return Ok(true);
        }
        let now = now_ms();
        let classified = ara_ai::retry_classification::classify_retry(message, &binding.model.api);
        let wait = classified
            .wait_ms
            .filter(|ms| ms.is_finite() && *ms > 0.0)
            .unwrap_or_else(|| native_backoff_ms(message.error_message.as_deref().unwrap_or("")));
        let unblock_at_ms = now + wait;
        let (done, feedback) = watch::channel(FeedbackState::Pending);
        let pending = Arc::new(PendingQuota {
            terminal: message.clone(),
            binding,
            lease: lease.clone(),
            unblock_at_ms,
            feedback,
        });
        state.records.insert(request.call_id, pending.clone());
        state.jobs.push(tokio::spawn(async move {
            // A confirmed terminal's feedback survives the Run cancellation.
            // This token never permits reset spending or another model call.
            let owned = CancellationToken::new();
            let result = pending
                .binding
                .account
                .mark_usage_limit_reached_observed(
                    &pending.binding.model.provider,
                    &pending.binding.context,
                    &pending.lease,
                    pending.unblock_at_ms,
                    &owned,
                )
                .await;
            let state = match result {
                Ok(recorded) => FeedbackState::Settled(recorded),
                Err(_) => FeedbackState::Failed,
            };
            done.send_replace(state);
        }));
        Ok(true)
    }
}

pub(crate) async fn recover(
    observed: ObservedQuota,
    model: &Model,
    session_id: &str,
    allow_recovery: bool,
    cancel: &CancellationToken,
) -> anyhow::Result<SessionQuotaOutcome> {
    let pending = observed.pending;
    let mut feedback = pending.feedback.clone();
    let recorded = loop {
        if cancel.is_cancelled() {
            return Ok(SessionQuotaOutcome::default());
        }
        match *feedback.borrow_and_update() {
            FeedbackState::Settled(recorded) => break recorded,
            FeedbackState::Failed => return Ok(SessionQuotaOutcome::default()),
            FeedbackState::Pending => {}
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(SessionQuotaOutcome::default()),
            result = feedback.changed() => {
                if result.is_err() { return Ok(SessionQuotaOutcome::default()); }
            }
        }
    };
    let Some(recorded) = recorded else { return Ok(SessionQuotaOutcome::default()) };
    let binding = &pending.binding;
    let current = || {
        !cancel.is_cancelled()
            && binding.model == *model
            && binding.logical_session.as_deref() == Some(session_id)
            && binding.host.is_current(&binding.reset_context)
    };
    if !current() {
        return Ok(SessionQuotaOutcome::default());
    }
    let mut outcome = SessionQuotaOutcome { recorded: true, ..Default::default() };
    if !allow_recovery {
        return Ok(outcome);
    }
    if !binding.host.freeze_quota_cancellation(&binding.reset_context, cancel) || !current() {
        return Ok(SessionQuotaOutcome::default());
    }
    // Availability is not a switch receipt. Resolve through this binding's
    // actual owner, then check the durable row/type before adoption.
    if recorded.availability.switched {
        let next = binding.resolver.resolve(model, cancel).await;
        if current()
            && let Ok(next) = next
        {
            let different = matches!((pending.lease.identity(), next.identity()),
                (CredentialIdentity::Stored { id: failed, .. }, CredentialIdentity::Stored { id: restored, .. }) if failed != restored);
            outcome.switched = different
                && binding.account.credential_kind_for_lease(&model.provider, &next).ok().flatten()
                    == Some(recorded.credential_kind);
        }
        if !outcome.switched {
            outcome.wait_ms = Some((pending.unblock_at_ms - now_ms()).max(0.0));
        }
        // Native sibling-first also prevents a reset when a sibling resolution
        // subsequently fails. Preserve the original error/window; unavailable
        // resolution cannot authorize a short same-credential retry.
        return Ok(if current() { outcome } else { SessionQuotaOutcome::default() });
    }
    if !current() {
        return Ok(SessionQuotaOutcome::default());
    }
    let failure = RequestAuthFailure {
        kind: RequestAuthFailureKind::Quota,
        retry_after_ms: Some((pending.unblock_at_ms - now_ms()).max(0.0)),
    };
    // Do not call retry(Quota): that driver would record this rejection again.
    let reset = binding.resolver.quota_reset(model, &pending.lease, &failure, cancel).await;
    if !current() {
        return Ok(SessionQuotaOutcome::default());
    }
    if let Ok(Some(reset)) = reset {
        let next = binding.resolver.resolve(model, cancel).await;
        if !current() {
            return Ok(SessionQuotaOutcome::default());
        }
        if let Ok(next) = next
            && reset.matches_resolved_lease(&next)
        {
            outcome.switched = true;
            return Ok(outcome);
        }
    }
    let now = now_ms();
    let window = (pending.unblock_at_ms - now).max(0.0);
    outcome.wait_ms = Some(
        recorded
            .availability
            .retry_at_ms
            .filter(|at| at.is_finite())
            .map(|at| (at + 1_000.0 - now).max(0.0).min(window))
            .unwrap_or(window),
    );
    Ok(outcome)
}

fn now_ms() -> f64 {
    chrono::Utc::now().timestamp_millis() as f64
}

fn matches_retained(raw: &AssistantMessage, retained: &AssistantMessage) -> bool {
    let ids: HashSet<String> =
        retained.content.iter().filter_map(|block| block.as_tool_call().map(|call| call.id.clone())).collect();
    ara_agent::agent_loop::retain_completed_tool_calls(raw.clone(), &ids) == *retained
}

// This private fallback mirrors the fixed Session rate-limit reason ordering.
// The HTTP/body hint parser and usage decision are shared ara-ai components.
// Capacity jitter is needed only for overlapping quota/capacity error text.
fn native_backoff_ms(message: &str) -> f64 {
    use regex::Regex;
    // These fixed JS patterns have no /u flag. Latin case folding, word
    // boundaries and word characters are ASCII; JS whitespace also includes
    // FEFF and excludes NEL. Keep Unicode dot/classes scoped so regex::Regex
    // continues to accept UTF-8 Chinese text under the surrounding ASCII mode.
    const JS_SPACE: &str =
        r"\x09-\x0d\x20\u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}";
    macro_rules! pattern {
        ($name:ident, $text:literal) => {
            static $name: LazyLock<Regex> = LazyLock::new(|| {
                // The fixed corpus has no escaped literal dots. JS's dot
                // excludes all four line terminators, not only LF.
                let pattern = $text
                    .replace("(?i)", "(?i-u)")
                    .replace('.', r"(?u:[^\r\n\u{2028}\u{2029}])")
                    .replace(r"[^()\s]", &format!("(?u:[^(){JS_SPACE}])"))
                    .replace(r"[^\n]", r"(?u:[^\n])")
                    .replace(r"\s", &format!("(?u:[{JS_SPACE}])"));
                Regex::new(&pattern).expect("native Session rate-limit pattern")
            });
        };
    }
    pattern!(RESOURCE, r"(?i)resource.?exhausted");
    pattern!(CN_QUOTA, r"使用.{0,30}?上限|(?:额度|配额)已?(?:用|耗)(?:完|尽)|限额.{0,30}重置|余额不足");
    pattern!(
        CN_TRANSIENT,
        r"速率.{0,30}上限|频率.{0,30}上限|每分钟.{0,30}上限|并发.{0,30}上限|使用.{0,30}(?:速率|频率|每分钟|并发).{0,30}上限"
    );
    pattern!(DASHSCOPE_ANCHOR, r"(?i)error-code[^()\s]*#token-limit");
    pattern!(DASHSCOPE_WORDING, r"(?i)\byou exceeded your current quota, please check your plan and billing details\b");
    pattern!(
        CONCURRENT,
        r"(?i)\btoo many\s+concurren\w*\s+(?:requests?|invocations?)\b|\bconcurren\w*\b[^\n]{0,60}\b(?:limit|quota|exceed\w*|reach\w*)\b|\b(?:limit|quota|exceed\w*|reach\w*)\b[^\n]{0,60}\bconcurren\w*\b|\bconcurren[a-z]*[-_](?:[a-z]+[_-])*(?:limit|quota|exceed\w*|reach\w*)"
    );
    pattern!(ACCOUNT, r"(?i)\baccount(?:'s)?\b[^\n]{0,80}\brate.?limit\b|\brate.?limit\b[^\n]{0,80}\baccount\b");
    pattern!(SPEND, r"(?i)spend.?limit");
    pattern!(
        SUBSCRIPTION,
        r"(?i)\b(?:subscription|plan|membership)\b[^\n]{0,80}\b(?:rate.?limits?|quota|cap)\b|\b(?:rate.?limits?|quota|cap)\b[^\n]{0,80}\b(?:subscription|plan|membership)\b"
    );
    pattern!(INTERVAL, r"(?i)\bper\s+(?:second|minute)\b");
    pattern!(FREE_DAY, r"(?i)\bfree[-_ ]models[-_ ]per[-_ ]day\b");
    pattern!(CLINE, r"(?i)clinepass limit|free limit reached on model");
    pattern!(BALANCE, r"(?i)insufficient.?balance");
    pattern!(
        CREDITS,
        r"(?i)\b(?:exceed\w*|insufficient|not enough)\b[^\n]{0,40}\bcredits?\b|\bcredits?\b[^\n]{0,40}\b(?:exhausted|depleted)\b"
    );
    const QUOTA: f64 = 1_800_000.0;
    if let (Some(start), Some(end)) = (message.find('{'), message.rfind('}'))
        && start <= end
        && let Ok(body) = serde_json::from_str::<serde_json::Value>(&message[start..=end])
        && let Some(error) = body.get("error").filter(|error| {
            error
                .get("status")
                .and_then(|s| s.as_str())
                .is_some_and(|status| status.trim().eq_ignore_ascii_case("RESOURCE_EXHAUSTED"))
        })
        && let Some(details) = error.get("details").and_then(|v| v.as_array())
    {
        for detail in details {
            if detail["@type"].as_str() != Some("type.googleapis.com/google.rpc.ErrorInfo") {
                continue;
            }
            match detail["reason"].as_str().map(|r| r.trim().to_ascii_uppercase()).as_deref() {
                Some("QUOTA_EXHAUSTED" | "INSUFFICIENT_G1_CREDITS_BALANCE") => return QUOTA,
                Some("RATE_LIMIT_EXCEEDED") => {
                    return if error["message"]
                        .as_str()
                        .is_some_and(|text| text.to_lowercase().contains("exhausted your capacity on this model"))
                    {
                        QUOTA
                    } else {
                        30_000.0
                    };
                }
                _ => {}
            }
        }
    }
    let lower = message.to_lowercase();
    let stripped = RESOURCE.replace_all(&lower, "");
    if stripped.contains("quota will reset")
        || stripped.contains("exhausted your capacity")
        || CN_QUOTA.is_match(message) && !CN_TRANSIENT.is_match(message)
    {
        return QUOTA;
    }
    if DASHSCOPE_ANCHOR.is_match(message) && DASHSCOPE_WORDING.is_match(message) {
        return 30_000.0;
    }
    if CONCURRENT.is_match(message) {
        return 5_000.0;
    }
    let capacity = || {
        let random = u64::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap());
        45_000.0 + (random >> 11) as f64 / ((1_u64 << 53) as f64) * 30_000.0
    };
    if ["capacity", "overloaded", "529", "503"].iter().any(|word| stripped.contains(word)) {
        return capacity();
    }
    if ACCOUNT.is_match(message)
        || SPEND.is_match(message)
        || SUBSCRIPTION.is_match(message) && !INTERVAL.is_match(message)
        || FREE_DAY.is_match(message)
        || CLINE.is_match(message)
    {
        return QUOTA;
    }
    if ["per minute", "rate limit", "too many requests", "presque"].iter().any(|word| stripped.contains(word)) {
        return 30_000.0;
    }
    if ["exhausted", "quota", "usage limit", "run out of credits", "out of credits", "spending-limit", "spending limit"]
        .iter()
        .any(|word| stripped.contains(word))
        || BALANCE.is_match(message)
        || CREDITS.is_match(message)
    {
        return QUOTA;
    }
    if ["500", "internal error", "internal server error"].iter().any(|word| stripped.contains(word)) {
        return 20_000.0;
    }
    if stripped != lower {
        return capacity();
    }
    QUOTA
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_ai::{AssistantBlock, StopReason, ToolCall};
    use serde_json::json;

    #[test]
    fn native_session_backoff_and_retained_terminal_ownership_family() {
        for (text, delay) in [
            ("", 1_800_000.0),
            ("usage_limit_reached", 1_800_000.0),
            ("subscription rate limit exceeded", 1_800_000.0),
            ("额度已耗尽", 1_800_000.0),
            ("insufficient balance", 1_800_000.0),
            ("rate limit exceeded per minute", 30_000.0),
            ("too many concurrent requests", 5_000.0),
            ("internal server error 500", 20_000.0),
            // Fixed /i (without /u): Han adjacency arms ASCII word boundaries,
            // while non-ASCII Latin case folds must not impersonate S or K.
            ("usage_limit_reached: 中account rate limit", 1_800_000.0),
            ("usage_limit_reached: account中 rate limit", 1_800_000.0),
            ("usage_limit_reached: 中plan中 rate limit", 1_800_000.0),
            ("usage_limit_reached: 中concurrent中 requests exceeded", 5_000.0),
            ("usage_limit_reached: too many concurrent requests中", 5_000.0),
            ("usage_limit_reached: ſubscription rate limit exceeded", 30_000.0),
            ("usage_limit_reached: too many concurrent requeſts", 1_800_000.0),
            (
                "You exceeded your current quota, please check your plan and billing details. https://fixture/error-code#toKen-limit",
                1_800_000.0,
            ),
            ("usage_limit_reached: too many\u{feff}concurrent requests", 5_000.0),
            ("usage_limit_reached: too many\u{0085}concurrent requests", 1_800_000.0),
            ("usage_limit_reached: subscription rate limit per\u{feff}minute", 30_000.0),
            ("usage_limit_reached: subscription rate limit per\u{0085}minute", 1_800_000.0),
            ("usage_limit_reached: account🙂 rate limit", 1_800_000.0),
            ("使用每分钟的请求数已达上限 rate limit exceeded", 30_000.0),
            ("resource\u{2028}exhausted", 1_800_000.0),
            ("resource\u{2029}exhausted", 1_800_000.0),
            ("resource\rexhausted", 1_800_000.0),
        ] {
            assert_eq!(native_backoff_ms(text), delay, "{text}");
        }
        for text in ["resource_exhausted", "resource中exhausted", "model capacity exhausted", "overloaded 503"] {
            assert!((45_000.0..75_000.0).contains(&native_backoff_ms(text)), "{text}");
        }
        for (reason, delay) in [
            ("QUOTA_EXHAUSTED", 1_800_000.0),
            ("INSUFFICIENT_G1_CREDITS_BALANCE", 1_800_000.0),
            ("RATE_LIMIT_EXCEEDED", 30_000.0),
        ] {
            let body = json!({"error":{"status":"RESOURCE_EXHAUSTED","message":"limit",
                "details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":reason}]}});
            assert_eq!(native_backoff_ms(&body.to_string()), delay);
        }
        let mut raw = AssistantMessage::empty("openai-completions", "fixture", "model");
        raw.stop_reason = StopReason::Error;
        raw.error_message = Some("usage_limit_reached".into());
        raw.content = vec![AssistantBlock::ToolCall(ToolCall {
            id: "partial".into(),
            name: "write_fixture".into(),
            arguments: json!({"incomplete":true}).as_object().unwrap().clone(),
            thought_signature: None,
        })];
        let retained = ara_agent::agent_loop::retain_completed_tool_calls(raw.clone(), &HashSet::new());
        assert!(matches_retained(&raw, &retained), "reproduce the Agent's unfinished tool normalization");
        assert!(matches_retained(&raw, &raw), "a retained completed call still matches exactly");
        let mut unrelated = retained.clone();
        unrelated.error_message = Some("different error with the same model and timestamp".into());
        assert!(!matches_retained(&raw, &unrelated));
        unrelated = retained.clone();
        unrelated.provider = "other-provider".into();
        assert!(!matches_retained(&raw, &unrelated));
        let mut changed = raw.clone();
        changed.content[0] = AssistantBlock::ToolCall(ToolCall {
            id: "partial".into(),
            name: "write_fixture".into(),
            arguments: json!({"incomplete":false}).as_object().unwrap().clone(),
            thought_signature: None,
        });
        assert!(!matches_retained(&raw, &changed), "retained call arguments cannot be rewritten into ownership proof");
    }
}
