//! Thinking-loop detectors and provider stream guard.
//!
//! Ported from OMP `packages/ai/src/utils/thinking-loop.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; see THIRD_PARTY_NOTICES.md).
//! UTF-16 lengths, windows, thresholds and stream latches follow that source.
//! Hosts resolve compatibility/class facts into [`LoopGuardPolicy`]; the shared
//! execution Model does not contain the upstream catalogue's identity/compat.
//! Result-path re-sampling and Gemini header reminders belong to their caller.

use crate::event::{AssistantMessageEvent, AssistantStream};
use crate::retry_classification::{ProviderErrorKind, ProviderFailureEvidence, flag};
use crate::{AssistantMessage, CallOptions, Context, Model, ModelProvider, ProviderError, StopReason};
use regex::Regex;
use std::collections::{HashSet, VecDeque};
use std::sync::LazyLock;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub const THINKING_LOOP_ERROR_MARKER: &str = "Thinking loop detected";
pub const GEMINI_HEADER_RUNAWAY_THRESHOLD: usize = 36;

const EXACT_TAIL_WINDOW: usize = 4096;
const EXACT_MAX_UNIT: usize = 1024;
const EXACT_CHECK_STRIDE: usize = 128;
const EXACT_SHORT_MAX_UNIT: usize = 60;
const EXACT_SHORT_MIN_REPEATED_CHARS: usize = 180;
const EXACT_LONG_MIN_REPEATED_CHARS: usize = 1024;
const SEGMENT_CHAR_CAP: usize = 700;
const SEGMENT_MIN_NORM_CHARS: usize = 60;
const SEGMENT_WINDOW: usize = 16;
const SEGMENT_SIMILARITY: f64 = 0.8;
const SEGMENT_MIN_COUNT: usize = 8;
const SEGMENT_MIN_CLUSTER: usize = 4;
const LEX_NOVELTY_WINDOW: usize = 8;
const LEX_STALL_NOVELTY_FLOOR: f64 = 0.2;
const LEX_STALL_MIN_RUN: usize = 8;
const THINKING_LOOP_MAX_ATTEMPTS: usize = 3;
const THINKING_LOOP_RETRY_BASE_DELAY_MS: u64 = 500;
const THINKING_LOOP_RETRY_MAX_DELAY_MS: u64 = 8_000;

/// Host-resolved policy, independent of model-name substring guesses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopGuardPolicy {
    pub enabled: bool,
    pub semantic_heuristics: bool,
    pub check_assistant_content: bool,
}

impl Default for LoopGuardPolicy {
    fn default() -> Self {
        Self { enabled: true, semantic_heuristics: false, check_assistant_content: true }
    }
}

/// Upstream per-call overrides. `enabled: true` does not opt a model into
/// semantic detection; its resolved compatibility/class policy still owns that.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoopGuardOptions {
    pub enabled: Option<bool>,
    pub check_assistant_content: Option<bool>,
}

impl LoopGuardPolicy {
    pub fn with_overrides(mut self, options: Option<&LoopGuardOptions>) -> Self {
        if let Some(options) = options {
            if let Some(enabled) = options.enabled {
                self.enabled = enabled;
            }
            if let Some(check) = options.check_assistant_content {
                self.check_assistant_content = check;
            }
        }
        self
    }
}

/// Stateful exact-cycle, near-duplicate and vocabulary-stall detector.
/// A caller stops after its first returned reason, as in fixed OMP.
pub struct ThinkingLoopDetector {
    semantic_heuristics: bool,
    tail: Vec<u16>,
    exact_scanned_at: usize,
    pending: Vec<u16>,
    window: VecDeque<HashSet<String>>,
    count: usize,
    word_window: VecDeque<HashSet<String>>,
    lex_stall_run: usize,
    anchor_window: VecDeque<HashSet<String>>,
}

impl Default for ThinkingLoopDetector {
    fn default() -> Self {
        Self::new(true)
    }
}

impl ThinkingLoopDetector {
    pub fn new(semantic_heuristics: bool) -> Self {
        Self {
            semantic_heuristics,
            tail: Vec::new(),
            exact_scanned_at: 0,
            pending: Vec::new(),
            window: VecDeque::new(),
            count: 0,
            word_window: VecDeque::new(),
            lex_stall_run: 0,
            anchor_window: VecDeque::new(),
        }
    }

    pub fn push(&mut self, delta: &str) -> Option<String> {
        if delta.is_empty() {
            return None;
        }
        let units: Vec<u16> = delta.encode_utf16().collect();
        self.tail.extend_from_slice(&units);
        if self.tail.len() > EXACT_TAIL_WINDOW {
            self.tail.drain(..self.tail.len() - EXACT_TAIL_WINDOW);
        }
        self.exact_scanned_at += units.len();
        if self.exact_scanned_at >= EXACT_CHECK_STRIDE || units.len() >= EXACT_CHECK_STRIDE {
            self.exact_scanned_at = 0;
            if let Some(reason) = exact_cycle_reason(&self.tail) {
                return Some(reason);
            }
        }
        if !self.semantic_heuristics {
            return None;
        }
        self.pending.extend_from_slice(&units);
        loop {
            let raw = if let Some((start, end)) = blank_line_boundary(&self.pending) {
                let raw = self.pending[..start].to_vec();
                self.pending.drain(..end);
                raw
            } else if self.pending.len() > SEGMENT_CHAR_CAP {
                self.pending.drain(..SEGMENT_CHAR_CAP).collect()
            } else {
                return None;
            };
            for chunk in raw.chunks(SEGMENT_CHAR_CAP) {
                if let Some(reason) = self.consume_segment(&String::from_utf16_lossy(chunk)) {
                    return Some(reason);
                }
            }
        }
    }

    /// Force the final exact scan and consume unterminated trailing paragraphs.
    pub fn flush(&mut self) -> Option<String> {
        if let Some(reason) = exact_cycle_reason(&self.tail) {
            return Some(reason);
        }
        if !self.semantic_heuristics || self.pending.is_empty() {
            return None;
        }
        let pending = std::mem::take(&mut self.pending);
        for chunk in pending.chunks(SEGMENT_CHAR_CAP) {
            if let Some(reason) = self.consume_segment(&String::from_utf16_lossy(chunk)) {
                return Some(reason);
            }
        }
        None
    }

    fn consume_segment(&mut self, raw: &str) -> Option<String> {
        let segment = strip_summary_titles(raw);
        let normalized = normalize_segment(&segment);
        if normalized.len() < SEGMENT_MIN_NORM_CHARS {
            return None;
        }
        let fingerprint = trigram_shingles(&normalized);
        let cluster =
            1 + self.window.iter().filter(|previous| jaccard(&fingerprint, previous) >= SEGMENT_SIMILARITY).count();
        let words: HashSet<String> = normalized.split(' ').filter(|word| !word.is_empty()).map(str::to_owned).collect();
        let prior_vocab: HashSet<&String> = self.word_window.iter().flat_map(|set| set.iter()).collect();
        let unseen = words.iter().filter(|word| !prior_vocab.contains(word)).count();
        let novelty = if prior_vocab.is_empty() { 1.0 } else { unseen as f64 / words.len() as f64 };
        let anchors: HashSet<String> =
            CONCRETE_ANCHOR.find_iter(&segment).map(|anchor| anchor.as_str().replace('`', "").to_lowercase()).collect();
        let new_anchor =
            anchors.iter().any(|anchor| self.anchor_window.iter().all(|previous| !previous.contains(anchor)));
        if novelty <= LEX_STALL_NOVELTY_FLOOR && !new_anchor {
            self.lex_stall_run += 1;
        } else {
            self.lex_stall_run = 0;
        }
        retain_window(&mut self.window, fingerprint, SEGMENT_WINDOW);
        retain_window(&mut self.word_window, words, LEX_NOVELTY_WINDOW);
        retain_window(&mut self.anchor_window, anchors, LEX_NOVELTY_WINDOW);
        self.count += 1;
        if self.count >= SEGMENT_MIN_COUNT {
            if cluster >= SEGMENT_MIN_CLUSTER {
                return Some(format!("{cluster} near-identical segments within the last {SEGMENT_WINDOW}"));
            }
            if self.lex_stall_run >= LEX_STALL_MIN_RUN {
                return Some(format!("{} low-information segments recycling recent wording", self.lex_stall_run));
            }
        }
        None
    }
}

fn retain_window<T>(window: &mut VecDeque<T>, value: T, cap: usize) {
    window.push_back(value);
    if window.len() > cap {
        window.pop_front();
    }
}

static CONCRETE_ANCHOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"`[^`]+`|(?-u:\b\w{2,}\.[a-zA-Z]\w{0,4}\b|[\w-]+(?:/[\w-]+){2,}|\b\w+_\w+\b|\b[a-z]+[A-Z]\w*\b|\b[A-Z][a-z]+[A-Z]\w*\b)")
        .expect("fixed OMP concrete anchor pattern")
});
static EXACT_MEANINGFUL_UNIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\p{L}|\p{Extended_Pictographic}").expect("fixed OMP letter or pictograph pattern"));
static BOLD_TITLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\*{2,3}[^\n\r\u{2028}\u{2029}]+?\*{2,3}$").expect("fixed OMP bold summary title pattern")
});

/// JavaScript String trim / regex whitespace, rather than Rust's broader
/// Unicode whitespace class (which includes U+0085 and excludes U+FEFF).
fn js_whitespace(unit: u16) -> bool {
    matches!(unit, 0x0009..=0x000d | 0x0020 | 0x00a0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff)
}

fn blank_line_boundary(units: &[u16]) -> Option<(usize, usize)> {
    for (start, unit) in units.iter().enumerate() {
        if *unit != b'\n' as u16 {
            continue;
        }
        let mut end = start + 1;
        let mut last_newline = None;
        while end < units.len() && js_whitespace(units[end]) {
            if units[end] == b'\n' as u16 {
                last_newline = Some(end);
            }
            end += 1;
        }
        if let Some(last) = last_newline {
            return Some((start, last + 1));
        }
    }
    None
}

fn strip_summary_titles(raw: &str) -> String {
    let mut stripped = String::with_capacity(raw.len());
    for line in raw.split_inclusive(['\n', '\r', '\u{2028}', '\u{2029}']) {
        let body = line.trim_end_matches(['\n', '\r', '\u{2028}', '\u{2029}']);
        let trimmed = body.trim_matches([' ', '\t']);
        let heading = trimmed.bytes().take_while(|byte| *byte == b'#').count();
        let atx = (1..=6).contains(&heading) && matches!(trimmed.as_bytes().get(heading), Some(b' ' | b'\t'));
        if !atx && !BOLD_TITLE.is_match(trimmed) {
            stripped.push_str(body);
        }
        stripped.push_str(&line[body.len()..]);
    }
    stripped
}

fn normalize_segment(segment: &str) -> String {
    segment
        .to_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| token.bytes().any(|byte| byte.is_ascii_lowercase()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn trigram_shingles(normalized: &str) -> HashSet<String> {
    let words: Vec<&str> = normalized.split(' ').filter(|word| !word.is_empty()).collect();
    if words.len() < 3 {
        return if words.is_empty() { HashSet::new() } else { HashSet::from([words.join(" ")]) };
    }
    words.windows(3).map(|trigram| trigram.join(" ")).collect()
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (small, large) = if a.len() < b.len() { (a, b) } else { (b, a) };
    let intersection = small.iter().filter(|word| large.contains(*word)).count();
    let union = a.len() + b.len() - intersection;
    if union == 0 { 0.0 } else { intersection as f64 / union as f64 }
}

fn exact_cycle_reason(text: &[u16]) -> Option<String> {
    let (unit, count) = detect_exact_suffix_cycle(text)?;
    Some(format!("repeated an exact {unit}-character cycle {count}× back-to-back"))
}

/// Reversed UTF-16 Z-array, retaining OMP's first qualifying suffix period.
fn detect_exact_suffix_cycle(text: &[u16]) -> Option<(usize, usize)> {
    if text.len() < EXACT_SHORT_MIN_REPEATED_CHARS {
        return None;
    }
    let reversed: Vec<u16> = text.iter().rev().copied().collect();
    let mut z = vec![0usize; reversed.len()];
    let mut left = 0;
    let mut right = 0;
    for i in 1..reversed.len() {
        if i <= right {
            z[i] = (right - i + 1).min(z[i - left]);
        }
        while i + z[i] < reversed.len() && reversed[z[i]] == reversed[i + z[i]] {
            z[i] += 1;
        }
        // The source uses a signed -1 when z[i] is zero. Do not underflow.
        if z[i] > 0 && i + z[i] - 1 > right {
            left = i;
            right = i + z[i] - 1;
        }
    }
    let max_unit = EXACT_MAX_UNIT.min(reversed.len() / 3);
    for length in 2..=max_unit {
        let count = 1 + z[length] / length;
        let short = length <= EXACT_SHORT_MAX_UNIT;
        let min_count = if short { 4 } else { 3 };
        let min_chars = if short { EXACT_SHORT_MIN_REPEATED_CHARS } else { EXACT_LONG_MIN_REPEATED_CHARS };
        if count < min_count || length * count < min_chars {
            continue;
        }
        if EXACT_MEANINGFUL_UNIT.is_match(&String::from_utf16_lossy(&text[text.len() - length..])) {
            return Some((length, count));
        }
    }
    None
}

/// Whole-line markdown / bold Gemini thought-summary header. The caller passes
/// a trimmed line; inline emphasis never matches the bold alternative.
pub fn is_reasoning_summary_header(line: &str) -> bool {
    let heading = line.bytes().take_while(|byte| *byte == b'#').count();
    if (1..=6).contains(&heading) {
        let rest = &line[heading..];
        let after_space = rest.trim_start_matches([' ', '\t']);
        if after_space.len() != rest.len()
            && after_space
                .chars()
                .next()
                .is_some_and(|character| character as u32 > 0xffff || !js_whitespace(character as u16))
        {
            return true;
        }
    }
    BOLD_TITLE.is_match(line)
}

/// Independent summary-header runaway detector, as exported by fixed OMP.
/// Paragraphs between titles do not reset its run. A Host resets on a fresh
/// thinking block/prose/tool call and owns any tool-call reminder.
#[derive(Default)]
pub struct GeminiHeaderRunDetector {
    pending: String,
    count: usize,
    fired: bool,
}

impl GeminiHeaderRunDetector {
    pub fn push(&mut self, delta: &str) -> bool {
        if self.fired || delta.is_empty() {
            return false;
        }
        self.pending.push_str(delta);
        while let Some(newline) = self.pending.find('\n') {
            let line = self.pending[..newline]
                .trim_matches(|character: char| character as u32 <= 0xffff && js_whitespace(character as u16));
            if !line.is_empty() && is_reasoning_summary_header(line) {
                self.count += 1;
                if self.count >= GEMINI_HEADER_RUNAWAY_THRESHOLD {
                    self.fired = true;
                    return true;
                }
            }
            self.pending.drain(..newline + 1);
        }
        false
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.count = 0;
        self.fired = false;
    }
}

/// Apply the guard to one provider dispatch. The child token inherits caller
/// cancellation without allowing a detected loop to cancel the caller/Run.
pub fn with_thinking_loop_guard(
    model: &Model,
    mut options: CallOptions,
    policy: LoopGuardPolicy,
    dispatch: impl FnOnce(CallOptions) -> AssistantStream,
) -> AssistantStream {
    let policy = policy.with_overrides(options.loop_guard.as_ref());
    if std::env::var("ARA_NO_THINKING_LOOP_GUARD").is_ok_and(|value| value == "1") || !policy.enabled {
        return dispatch(options);
    }
    let controller = options.cancel.child_token();
    options.cancel = controller.clone();
    guard_thinking_loop_stream(dispatch(options), model, controller, policy)
}

/// Wrap an already-dispatched stream whose cancellation was wired to this
/// guard-owned controller. Emits one empty failed terminal on a loop hit.
pub fn guard_thinking_loop_stream(
    mut inner: AssistantStream,
    model: &Model,
    controller: CancellationToken,
    policy: LoopGuardPolicy,
) -> AssistantStream {
    let (tx, outer) = mpsc::channel(256);
    let model = model.clone();
    tokio::spawn(async move {
        let _settlement = controller.clone().drop_guard();
        let mut thinking_detector = ThinkingLoopDetector::new(policy.semantic_heuristics);
        let mut text_detector = ThinkingLoopDetector::new(policy.semantic_heuristics);
        let mut thinking_armed = true;
        let mut text_armed = policy.check_assistant_content;
        let mut text_started = false;
        loop {
            let event = tokio::select! {
                _ = tx.closed() => return,
                event = inner.recv() => event,
            };
            let Some(event) = event else {
                // Unlike OMP's result-bearing EventStream, a Rust receiver has
                // no separate inner.result(). EOF is explicitly not success.
                let mut error = AssistantMessage::empty(&model.api, &model.provider, &model.id);
                error.stop_reason = StopReason::Error;
                error.error_message = Some("Provider stream ended without a terminal event".into());
                error.failure_evidence = Some(ProviderFailureEvidence {
                    kind: ProviderErrorKind::Incomplete,
                    status: None,
                    code: None,
                    replay_blocked: true,
                    same_route_blocked: true,
                    wait_ms: None,
                    error_id: 0,
                });
                let _ = tx.send(AssistantMessageEvent::Error { reason: StopReason::Error, error }).await;
                return;
            };
            let detail = match &event {
                AssistantMessageEvent::ThinkingDelta { delta, .. } if !text_started => {
                    thinking_armed = true;
                    thinking_detector.push(delta)
                }
                AssistantMessageEvent::ThinkingEnd { .. } if thinking_armed => thinking_detector.flush(),
                AssistantMessageEvent::TextDelta { delta, .. } => {
                    if !delta.is_empty() {
                        thinking_armed = false;
                        text_started = true;
                    }
                    if text_armed { text_detector.push(delta) } else { None }
                }
                AssistantMessageEvent::ToolcallStart { .. } | AssistantMessageEvent::ToolcallDelta { .. } => {
                    text_armed = false;
                    None
                }
                AssistantMessageEvent::Done { .. } => {
                    let thinking = if thinking_armed { thinking_detector.flush() } else { None };
                    thinking.or_else(|| if text_armed { text_detector.flush() } else { None })
                }
                _ => None,
            };
            if let Some(detail) = detail {
                controller.cancel();
                let error = build_thinking_loop_error(&model, &detail, event.partial());
                let _ = tx.send(AssistantMessageEvent::Error { reason: StopReason::Error, error }).await;
                return;
            }
            let terminal = event.is_terminal();
            if tx.send(event).await.is_err() || terminal {
                return;
            }
        }
    });
    outer
}

fn build_thinking_loop_error(model: &Model, detail: &str, observed: &AssistantMessage) -> AssistantMessage {
    let mut error = AssistantMessage::empty(&model.api, &model.provider, &model.id);
    // ARA intentionally retains observed billing evidence; absent usage stays
    // unknown instead of the upstream synthetic all-zero usage/cost object.
    error.usage = observed.usage.clone();
    error.duration = observed.duration;
    error.ttft = observed.ttft;
    error.stop_reason = StopReason::Error;
    error.error_message = Some(format!(
        "{THINKING_LOOP_ERROR_MARKER}: the model repeated near-identical content ({detail}). Treating as a stream stall and retrying."
    ));
    error.failure_evidence = Some(ProviderFailureEvidence {
        kind: ProviderErrorKind::Stream,
        status: None,
        code: None,
        replay_blocked: false,
        same_route_blocked: false,
        wait_ms: None,
        error_id: flag::THINKING_LOOP | flag::CLASS,
    });
    error
}

/// Await an already-guarded provider result, re-sampling only an empty thinking
/// loop failure. This ports OMP `stream.ts:1048-1106` (`complete` result path).
/// All three attempts retain the original call options; detection is never
/// disabled as a fallback. The provider's existing dispatch owner must install
/// the stream guard, avoiding two competing guards around one provider call.
///
/// A normal provider Error terminal remains `Ok(message)`. `Err(Aborted)` maps
/// the source's rejected abort Promise; a caller abort cannot return a stale
/// thinking-loop receipt. A Rust channel EOF maps to a rejected result as well.
pub async fn complete_with_thinking_loop_retry(
    provider: &dyn ModelProvider,
    model: &Model,
    context: &Context,
    options: CallOptions,
) -> Result<AssistantMessage, ProviderError> {
    complete_with_thinking_loop_retry_observed(provider, model, context, options, |_| {}).await
}

/// The fixed `completeSimple` onAttempt contract: observe every completed
/// attempt, including a loop receipt that will be retried. The caller owns any
/// receipt storage. This function never commits or promotes those attempts.
pub async fn complete_with_thinking_loop_retry_observed(
    provider: &dyn ModelProvider,
    model: &Model,
    context: &Context,
    options: CallOptions,
    mut on_attempt: impl FnMut(&AssistantMessage),
) -> Result<AssistantMessage, ProviderError> {
    let mut message = completion_result(provider.stream(model, context, options.clone())).await?;
    on_attempt(&message);
    for attempt in 1..THINKING_LOOP_MAX_ATTEMPTS {
        if !is_retryable_thinking_loop(&message) {
            return Ok(message);
        }
        if options.cancel.is_cancelled() {
            return Err(ProviderError::Aborted);
        }
        let delay =
            THINKING_LOOP_RETRY_BASE_DELAY_MS.saturating_mul(1 << (attempt - 1)).min(THINKING_LOOP_RETRY_MAX_DELAY_MS);
        tokio::select! {
            biased;
            _ = options.cancel.cancelled() => return Err(ProviderError::Aborted),
            _ = tokio::time::sleep(Duration::from_millis(delay)) => {},
        }
        message = completion_result(provider.stream(model, context, options.clone())).await?;
        on_attempt(&message);
    }
    if is_retryable_thinking_loop(&message) && options.cancel.is_cancelled() {
        return Err(ProviderError::Aborted);
    }
    Ok(message)
}

fn is_retryable_thinking_loop(message: &AssistantMessage) -> bool {
    message.stop_reason == StopReason::Error
        && message.content.is_empty()
        && message.failure_evidence.as_ref().is_some_and(|evidence| evidence.error_id & flag::THINKING_LOOP != 0)
}

async fn completion_result(mut stream: AssistantStream) -> Result<AssistantMessage, ProviderError> {
    while let Some(event) = stream.recv().await {
        match event {
            AssistantMessageEvent::Done { message, .. } => return Ok(message),
            AssistantMessageEvent::Error { error, .. } => return Ok(error),
            _ => {}
        }
    }
    Err(ProviderError::Incomplete("Provider stream ended without a terminal event".into()))
}
