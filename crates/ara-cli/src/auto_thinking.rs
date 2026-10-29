//! Host-owned per-user-input Auto thinking classification.
//!
//! Ported from fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d:
//! coding-agent/src/{auto-thinking/classifier.ts,thinking.ts},
//! tiny/message-preproc.ts, modes/{markdown-prose,magic-keyword-boundary}.ts,
//! and ai/src/oneshot-retry.ts. Exact prompt assets carry LICENSE.omp.
//!
//! The Host owns role/auth preparation, the shared four-second deadline,
//! captured Session/branch usage ownership, and generation-checked installation.
//! A classifier never changes a main route or journals an assistant message.
//! Local parsing and its required adapter port are provided here; this module
//! does not implement or claim acceptance of a local inference runtime.
//!
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in
//! all copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
//! THE SOFTWARE.

use std::{
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

use ara_ai::{
    AssistantBlock, AssistantMessage, AssistantMessageEvent, CallOptions, Context, Message, Model, ModelProvider,
    ProviderResponseCallback, ReasoningEffort, ReasoningRequest, StopReason, UserMessage,
    retry_classification::{self, flag},
};
use ara_rpc::WireString;
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::js_regex::{JsRegExp, JsRegexMatch};

pub const CLASSIFICATION_TIMEOUT: Duration = Duration::from_secs(4);
pub const ONLINE_MAX_TOKENS: u64 = 4096;
pub const MAX_TINY_MESSAGE_CHARS: usize = 2000;
const ONLINE_PROMPT: &str = include_str!("../data/auto-thinking/auto-thinking-difficulty.md");
const LOCAL_PROMPT: &str = include_str!("../data/auto-thinking/auto-thinking-difficulty-local.md");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalClassifierModel {
    Qwen3_1_7B,
    Llama3_2_3B,
    Gemma3_1B,
    Qwen2_5_1_5B,
    Lfm2_1_2B,
}

impl LocalClassifierModel {
    pub fn parse(key: &str) -> Option<Self> {
        match key {
            "qwen3-1.7b" => Some(Self::Qwen3_1_7B),
            "llama3.2:3b" => Some(Self::Llama3_2_3B),
            "gemma-3-1b" => Some(Self::Gemma3_1B),
            "qwen2.5-1.5b" => Some(Self::Qwen2_5_1_5B),
            "lfm2-1.2b" => Some(Self::Lfm2_1_2B),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Qwen3_1_7B => "qwen3-1.7b",
            Self::Llama3_2_3B => "llama3.2:3b",
            Self::Gemma3_1B => "gemma-3-1b",
            Self::Qwen2_5_1_5B => "qwen2.5-1.5b",
            Self::Lfm2_1_2B => "lfm2-1.2b",
        }
    }

    pub fn max_tokens(self) -> u64 {
        if self == Self::Qwen3_1_7B { 1024 } else { 16 }
    }

    /// Fixed Native tiny/models.ts refuses the Qwen3 ONNX export; MLX only.
    pub fn onnx_unsupported_reason(self) -> Option<&'static str> {
        (self == Self::Qwen3_1_7B).then_some(
            "onnxruntime-node does not support Qwen3 RotaryEmbedding cache updates in onnx-community/Qwen3-1.7B-ONNX",
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoThinkingBackend {
    Online,
    Local(LocalClassifierModel),
}

impl AutoThinkingBackend {
    pub fn parse(key: &str) -> Option<Self> {
        if key == "online" { Some(Self::Online) } else { LocalClassifierModel::parse(key).map(Self::Local) }
    }
}

pub fn auto_ceiling(backend: AutoThinkingBackend, max_opt_in: bool, supported: &[ReasoningEffort]) -> ReasoningEffort {
    if backend == AutoThinkingBackend::Online && max_opt_in && supported.contains(&ReasoningEffort::Max) {
        ReasoningEffort::Max
    } else {
        ReasoningEffort::XHigh
    }
}

/// The Low floor is selected BEFORE applying the ceiling. In particular,
/// [Minimal, Max] under XHigh has no legal Auto tier; it must not pick Minimal.
/// The materialized model policy supplies its ordered supported ladder.
pub fn clamp_auto_effort(
    supported: &[ReasoningEffort],
    requested: ReasoningEffort,
    ceiling: ReasoningEffort,
) -> Option<ReasoningEffort> {
    let has_low = supported.iter().any(|effort| *effort >= ReasoningEffort::Low);
    let mut pool =
        supported.iter().copied().filter(|effort| (!has_low || *effort >= ReasoningEffort::Low) && *effort <= ceiling);
    let mut chosen = pool.next()?;
    for candidate in pool {
        if candidate > requested {
            break;
        }
        chosen = candidate;
    }
    Some(chosen)
}

pub fn provisional_effort(supported: &[ReasoningEffort], default: Option<ReasoningEffort>) -> Option<ReasoningEffort> {
    let preferred = default.unwrap_or(ReasoningEffort::High);
    let requested = if preferred == ReasoningEffort::Max { ReasoningEffort::XHigh } else { preferred };
    clamp_auto_effort(supported, requested, ReasoningEffort::XHigh)
}

/// Native preserves an incompatible request when there is no supported tier
/// under the Session ceiling. The Host must reject that route, including actual
/// mandatory normalization, rather than silently admitting an upward snap.
pub fn apply_session_ceiling(
    supported: &[ReasoningEffort],
    requested: Option<ReasoningEffort>,
    ceiling: Option<ReasoningEffort>,
) -> Option<ReasoningEffort> {
    let (Some(requested), Some(ceiling)) = (requested, ceiling) else {
        return requested;
    };
    if requested <= ceiling {
        return Some(requested);
    }
    supported.iter().rev().find(|effort| **effort <= ceiling).copied().or(Some(requested))
}

pub fn resolve_auto_effort(
    supported: &[ReasoningEffort],
    resolved: Option<ReasoningEffort>,
    prior_auto_resolved: Option<ReasoningEffort>,
    default: Option<ReasoningEffort>,
    session_ceiling: Option<ReasoningEffort>,
) -> Option<ReasoningEffort> {
    apply_session_ceiling(
        supported,
        resolved.or(prior_auto_resolved).or_else(|| provisional_effort(supported, default)),
        session_ceiling,
    )
}

fn js_regex(pattern: &str, flags: &str) -> JsRegExp {
    JsRegExp::new(WireString::from(pattern), flags).expect("fixed Native regular expression must compile")
}

fn replace_all_js(input: &WireString, pattern: &str, replace: impl Fn(&JsRegexMatch) -> WireString) -> WireString {
    let mut regex = js_regex(pattern, "g");
    let mut output = Vec::new();
    let mut end = 0;
    while let Some(matched) = regex.exec(input) {
        output.extend_from_slice(&input.units()[end..matched.index]);
        output.extend_from_slice(replace(&matched).units());
        end = matched.end;
    }
    output.extend_from_slice(&input.units()[end..]);
    WireString::from_units(output)
}

fn js_trim(input: &WireString) -> WireString {
    replace_all_js(input, r"^\s+|\s+$", |_| WireString::from(""))
}

/// Exact Native two-pass omission marker and JavaScript/UTF-16 slicing. Lone
/// surrogates at a split are retained; converting this value to a provider's
/// current String-only Context requires an explicit wire representation port.
pub fn truncate_tiny_message(input: &WireString) -> WireString {
    if input.len() <= MAX_TINY_MESSAGE_CHARS {
        return input.clone();
    }
    let mut omitted = input.len() - MAX_TINY_MESSAGE_CHARS;
    let (mut head, mut tail) = (0, 0);
    for _ in 0..2 {
        let marker = format!("\n[… {omitted} chars omitted …]\n");
        let kept = MAX_TINY_MESSAGE_CHARS.saturating_sub(marker.encode_utf16().count());
        head = (kept * 2).div_ceil(3);
        tail = kept - head;
        omitted = input.len() - head - tail;
    }
    let mut output = input.units()[..head].to_vec();
    output.extend(format!("\n[… {omitted} chars omitted …]\n").encode_utf16());
    output.extend_from_slice(&input.units()[input.len() - tail..]);
    WireString::from_units(output)
}

pub fn preprocess_tiny_message(message: &str) -> WireString {
    let input = WireString::from(message);
    let input = replace_all_js(&input, r"\u001b\[[0-9;]*m", |_| WireString::from(""));
    let input = replace_all_js(&input, r"<([a-zA-Z][\w-]*)(?:\s[^>]*)?>[\s\S]*?<\/\1>", |_| WireString::from(" "));
    let input = replace_all_js(&input, r"\b[0-9a-fA-F]{12,}\b", |matched| {
        matched.captures[0].as_ref().expect("whole match").slice_prefix(7)
    });
    let stripped = replace_all_js(&input, r"```+[\s\S]*?(?:```+|$)", |_| WireString::from(" "));
    let stripped = replace_all_js(&stripped, r"[ \t]+", |_| WireString::from(" "));
    let stripped = replace_all_js(&stripped, r"\n{3,}", |_| WireString::from("\n\n"));
    let stripped = js_trim(&stripped);
    truncate_tiny_message(if stripped.len() >= 12 { &stripped } else { &input })
}

fn earliest_label(text: &str, labels: &[(&str, ReasoningEffort)]) -> Option<ReasoningEffort> {
    let lower = WireString::from(text.to_lowercase());
    let mut best: Option<(usize, ReasoningEffort)> = None;
    for (pattern, effort) in labels {
        if let Some(matched) = js_regex(pattern, "").exec(&lower)
            && best.is_none_or(|(index, _)| matched.index < index)
        {
            best = Some((matched.index, *effort));
        }
    }
    best.map(|(_, effort)| effort)
}

pub fn parse_online_effort(text: &str) -> Option<ReasoningEffort> {
    earliest_label(
        text,
        &[
            (r"x[\s_-]?high", ReasoningEffort::XHigh),
            (r"\bmax\b", ReasoningEffort::Max),
            (r"\bhigh\b", ReasoningEffort::High),
            (r"\bmed(?:ium)?\b", ReasoningEffort::Medium),
            (r"\blow\b", ReasoningEffort::Low),
        ],
    )
}

pub fn parse_local_effort(text: &str) -> Option<ReasoningEffort> {
    earliest_label(
        text,
        &[
            (r"\btrivial\b", ReasoningEffort::Low),
            (r"\bmoderate\b", ReasoningEffort::High),
            (r"\bhard\b", ReasoningEffort::XHigh),
        ],
    )
}

fn is_ascii_letter(unit: u16) -> bool {
    matches!(unit, 0x41..=0x5a | 0x61..=0x7a)
}

fn tag_name_end(units: &[u16], start: usize) -> Option<usize> {
    if !units.get(start).copied().is_some_and(is_ascii_letter) {
        return None;
    }
    let mut end = start + 1;
    while units.get(end).is_some_and(|unit| is_ascii_letter(*unit) || matches!(*unit, 0x30..=0x39 | 0x2d)) {
        end += 1;
    }
    Some(end)
}

fn tag_end(units: &[u16], start: usize) -> Option<usize> {
    let mut quote = None;
    for (index, unit) in units.iter().copied().enumerate().skip(start) {
        if let Some(current) = quote {
            if unit == current {
                quote = None;
            }
        } else {
            match unit {
                0x22 | 0x27 => quote = Some(unit),
                0x3e => return Some(index),
                0x3c => return None,
                _ => {}
            }
        }
    }
    None
}

fn matching_tag_close(units: &[u16], start: usize, name: &[u16], masked: &[bool]) -> Option<usize> {
    let mut depth = 1;
    let mut index = start;
    while index < units.len() {
        if masked[index] || units[index] != 0x3c {
            index += 1;
            continue;
        }
        let closing = units.get(index + 1) == Some(&0x2f);
        let name_start = index + if closing { 2 } else { 1 };
        let Some(name_end) = tag_name_end(units, name_start) else {
            index += 1;
            continue;
        };
        let Some(gt) = tag_end(units, name_end) else {
            index += 1;
            continue;
        };
        let same =
            units[name_start..name_end].iter().zip(name).all(|(a, b)| (*a as u8).eq_ignore_ascii_case(&(*b as u8)))
                && name_end - name_start == name.len();
        if same {
            if closing {
                depth -= 1;
                if depth == 0 {
                    return Some(gt + 1);
                }
            } else if units[gt - 1] != 0x2f {
                depth += 1;
            }
        }
        index = gt + 1;
    }
    None
}

fn starts_ascii(units: &[u16], start: usize, literal: &str) -> bool {
    units
        .get(start..start + literal.len())
        .is_some_and(|part| part.iter().zip(literal.bytes()).all(|(unit, byte)| *unit == u16::from(byte)))
}

fn find_ascii(units: &[u16], start: usize, literal: &str) -> Option<usize> {
    (start..units.len()).find(|index| starts_ascii(units, *index, literal))
}

fn mask_tag_at(units: &[u16], start: usize, masked: &mut [bool]) -> usize {
    if starts_ascii(units, start, "<!--") {
        let stop = find_ascii(units, start + 4, "-->").map_or(units.len(), |end| end + 3);
        masked[start..stop].fill(true);
        return stop;
    }
    let closing = units.get(start + 1) == Some(&0x2f);
    let name_start = start + if closing { 2 } else { 1 };
    let Some(name_end) = tag_name_end(units, name_start) else {
        return start;
    };
    let Some(gt) = tag_end(units, name_end) else {
        return start;
    };
    let end = gt + 1;
    masked[start..end].fill(true);
    if closing || units[gt - 1] == 0x2f {
        return end;
    }
    let Some(close) = matching_tag_close(units, end, &units[name_start..name_end], masked) else {
        return end;
    };
    masked[end..close].fill(true);
    close
}

fn backtick_run_end(units: &[u16], start: usize) -> usize {
    let mut end = start;
    while units.get(end) == Some(&0x60) {
        end += 1;
    }
    end
}

fn matching_backtick_close(units: &[u16], start: usize, run_len: usize, masked: &[bool]) -> Option<usize> {
    let mut index = start;
    while index < units.len() {
        if !masked[index] && units[index] == 0x60 {
            let end = backtick_run_end(units, index);
            if end - index == run_len {
                return Some(end);
            }
            index = end;
        } else {
            index += 1;
        }
    }
    None
}

/// Length-preserving Native markdown-prose.ts mask. Masking uses UTF-16 units
/// so every original regex index still addresses the same user text.
pub fn mask_non_prose(text: &WireString) -> WireString {
    let units = text.units();
    if !units.contains(&0x60) && !units.contains(&0x3c) && find_ascii(units, 0, "~~~").is_none() {
        return text.clone();
    }
    let mut masked = vec![false; units.len()];
    let mut fence: Option<(u16, usize)> = None;
    let mut line_start = 0;
    while line_start <= units.len() {
        let newline = units[line_start..]
            .iter()
            .position(|unit| *unit == 0x0a)
            .map_or(units.len(), |relative| line_start + relative);
        let line = &units[line_start..newline];
        let mut indent = 0;
        while indent < line.len() && line[indent] == 0x20 {
            indent += 1;
        }
        let mut marker_end = indent;
        if indent <= 3 {
            while marker_end < line.len() && matches!(line[marker_end], 0x60 | 0x7e) {
                marker_end += 1;
            }
        }
        let open = (marker_end.saturating_sub(indent) >= 3).then_some((indent, marker_end));
        if let Some((character, length)) = fence {
            masked[line_start..newline].fill(true);
            if let Some((indent, end)) = open
                && line[indent] == character
                && end - indent >= length
                && js_trim(&WireString::from_units(line[end..].to_vec())).is_empty()
            {
                fence = None;
            }
        } else if let Some((indent, end)) = open {
            let character = line[indent];
            if !(character == 0x60 && line[end..].contains(&0x60)) {
                fence = Some((character, end - indent));
                masked[line_start..newline].fill(true);
            }
        }
        if newline == units.len() {
            break;
        }
        line_start = newline + 1;
    }
    let mut index = 0;
    while index < units.len() {
        if masked[index] {
            index += 1;
            continue;
        }
        match units[index] {
            0x60 => {
                let run_end = backtick_run_end(units, index);
                if let Some(close) = matching_backtick_close(units, run_end, run_end - index, &masked) {
                    masked[index..close].fill(true);
                    index = close;
                } else {
                    index = run_end;
                }
            }
            0x3c => {
                let end = mask_tag_at(units, index, &mut masked);
                index = if end > index { end } else { index + 1 };
            }
            _ => index += 1,
        }
    }
    WireString::from_units(
        units
            .iter()
            .copied()
            .enumerate()
            .map(|(index, unit)| if masked[index] && unit != 0x0a { 0x20 } else { unit })
            .collect(),
    )
}

pub fn has_ultrathink(text: &str) -> bool {
    let word = r"(?<![\p{L}\p{N}_./\\-])(?<!::)ultrathink(?![\p{L}\p{N}_/\\-])(?!\.[\p{L}\p{N}_-])(?!\()";
    let text = WireString::from(text);
    let mut regex = js_regex(word, "u");
    regex.exec(&text).is_some() && regex.exec(&mask_non_prose(&text)).is_some()
}

pub fn ultrathink_effort(text: &str, enabled: bool, supported: &[ReasoningEffort]) -> Option<ReasoningEffort> {
    if enabled && has_ultrathink(text) {
        // Fixed ModelControls deliberately bypasses the configured classifier
        // ceiling here. Session ceiling is still applied by its caller.
        clamp_auto_effort(supported, ReasoningEffort::Max, ReasoningEffort::Max)
    } else {
        None
    }
}

pub fn online_system_prompt(allow_max: bool) -> String {
    // Reuse the fixed prompt engine's standalone-tag stripping, noEscape and
    // post-render formatting rather than hand-expanding the conditional.
    ara_prompt::render(ONLINE_PROMPT, &serde_json::json!({ "allowMax": allow_max }))
        .expect("fixed classifier system prompt must render")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassifierError {
    Cancelled,
    Incomplete,
    Online(String),
    Unparseable {
        backend: &'static str,
    },
    Receipt(String),
    /// Native can retain a lone surrogate at the UTF-16 split; ara-ai Context
    /// currently has String user content. This is a required wire port, not a
    /// lossy preprocessing replacement or evidence of Native parity.
    Utf16WireRepresentationRequired,
    LocalRuntimeRequired(LocalClassifierModel),
    Local(String),
}

impl fmt::Display for ClassifierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("auto-thinking: classification cancelled"),
            Self::Incomplete => formatter.write_str("auto-thinking: online stream ended without a terminal"),
            Self::Online(error) => write!(formatter, "auto-thinking: online classification failed: {error}"),
            Self::Unparseable { backend } => write!(formatter, "auto-thinking: unparseable {backend} classification"),
            Self::Receipt(error) => write!(formatter, "auto-thinking: usage receipt failed: {error}"),
            Self::Utf16WireRepresentationRequired => {
                formatter.write_str("auto-thinking: UTF-16 provider input representation required")
            }
            Self::LocalRuntimeRequired(model) => {
                write!(formatter, "auto-thinking: real local inference runtime required for {}", model.as_str())
            }
            Self::Local(error) => write!(formatter, "auto-thinking: local classification failed: {error}"),
        }
    }
}

impl std::error::Error for ClassifierError {}

#[derive(Clone, Debug)]
pub struct ClassifierAttempt {
    pub attempt: u32,
    pub role: String,
    /// Actual observed terminal, including provider-reported nullable usage.
    /// No terminal is invented when cancellation wins before observation.
    pub message: AssistantMessage,
    /// The Native Codex simple adapter omits the logical output cap on wire.
    pub codex_output_cap_omitted: bool,
}

pub type ClassifierAttemptCallback =
    Arc<dyn Fn(ClassifierAttempt) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

#[derive(Clone)]
pub struct OnlineClassificationRequest {
    /// Host-prepared SIDE route: resolver, metadata and auth are already bound.
    pub provider: Arc<dyn ModelProvider>,
    pub model: Model,
    pub options: CallOptions,
    pub role: String,
    pub prompt: String,
    pub allow_max: bool,
    /// Linked Host deadline, caller cancellation and supersession token. The
    /// deadline starts before model-role/auth preparation, not in this helper.
    pub cancel: CancellationToken,
}

struct CancelOnDrop(CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Real side-route stream collector plus Native three-attempt transient retry.
/// Already observed terminal callbacks are awaited even if installation has
/// been cancelled. The Host must retain the owning job through receipt settle.
pub async fn classify_online(
    request: OnlineClassificationRequest,
    on_attempt: ClassifierAttemptCallback,
) -> Result<Option<ReasoningEffort>, ClassifierError> {
    let cleaned = preprocess_tiny_message(&request.prompt);
    let input = cleaned.to_utf8().map_err(|_| ClassifierError::Utf16WireRepresentationRequired)?;
    let context = Context {
        system_prompt: vec![online_system_prompt(request.allow_max)],
        messages: vec![Message::User(UserMessage::text(input))],
        tools: None,
    };
    let mut observed_attempt = 0;
    for oneshot_attempt in 1..=3 {
        // completeSimple owns a separate guarded-resampling budget. Its final
        // result is then classified by the outer transient oneshot policy.
        let completion = async {
            let mut completed = None;
            for thinking_attempt in 1..=3 {
                observed_attempt += 1;
                let (terminal, headers) =
                    collect_online_attempt(&request, &context, observed_attempt, &on_attempt).await?;
                let thinking_loop = terminal.stop_reason == StopReason::Error
                    && terminal.content.is_empty()
                    && terminal
                        .failure_evidence
                        .as_ref()
                        .is_some_and(|evidence| evidence.error_id & flag::THINKING_LOOP != 0);
                if !thinking_loop || thinking_attempt == 3 {
                    completed = Some((terminal, headers));
                    break;
                }
                classifier_wait(&request, Duration::from_millis(500 * (1 << (thinking_attempt - 1)))).await?;
            }
            Ok::<_, ClassifierError>(completed.expect("third thinking-loop attempt returns a result"))
        }
        .await;
        let (terminal, response_headers) = match completion {
            Ok(completed) => completed,
            // Native stream.result() rejects an incomplete stream. The outer
            // oneshot catches that transport fault and retries. There was no
            // observed terminal, so this path creates no model_usage receipt.
            Err(ClassifierError::Incomplete) if oneshot_attempt < 3 => {
                classifier_wait(
                    &request,
                    Duration::from_millis(backoff_delay_ms(oneshot_attempt, jitter_fraction()) as u64),
                )
                .await?;
                continue;
            }
            Err(error) => return Err(error),
        };
        if terminal.stop_reason != StopReason::Error {
            let text = terminal
                .content
                .iter()
                .filter_map(|block| match block {
                    AssistantBlock::Text(text) => Some(text.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ");
            return parse_online_effort(text.trim())
                .map(Some)
                .ok_or(ClassifierError::Unparseable { backend: "online" });
        }
        let class = retry_classification::classify_retry(&terminal, &request.model.api);
        if oneshot_attempt == 3 || !retryable_oneshot(&terminal, &class) {
            return Err(ClassifierError::Online(terminal.error_message.unwrap_or_else(|| "unknown error".into())));
        }
        let header_wait = retry_classification::retry_wait_ms(&response_headers, ara_ai::now_ms() as f64);
        let Some(delay) = retry_delay(&terminal, &request.model.api, oneshot_attempt, header_wait, jitter_fraction())
        else {
            return Err(ClassifierError::Online(terminal.error_message.unwrap_or_else(|| "unknown error".into())));
        };
        classifier_wait(&request, delay).await?;
    }
    unreachable!("the third terminal either succeeds or returns its failure")
}

async fn classifier_wait(request: &OnlineClassificationRequest, delay: Duration) -> Result<(), ClassifierError> {
    tokio::select! {
        biased;
        _ = request.cancel.cancelled() => Err(ClassifierError::Cancelled),
        _ = request.options.cancel.cancelled() => Err(ClassifierError::Cancelled),
        _ = tokio::time::sleep(delay) => Ok(()),
    }
}

async fn collect_online_attempt(
    request: &OnlineClassificationRequest,
    context: &Context,
    attempt: u32,
    on_attempt: &ClassifierAttemptCallback,
) -> Result<(AssistantMessage, reqwest::header::HeaderMap), ClassifierError> {
    if request.cancel.is_cancelled() || request.options.cancel.is_cancelled() {
        return Err(ClassifierError::Cancelled);
    }
    let attempt_cancel = request.cancel.child_token();
    let _cancel_guard = CancelOnDrop(attempt_cancel.clone());
    let response_headers = Arc::new(Mutex::new(reqwest::header::HeaderMap::new()));
    let captured = response_headers.clone();
    let existing = request.options.on_response.clone();
    let mut options = request.options.clone();
    options.cancel = attempt_cancel.clone();
    options.tool_choice = None;
    let codex = request.model.api == "openai-codex-responses";
    // Native logical cap is 4096. Its Codex adapter explicitly omits it on
    // wire (openai-codex-responses.ts:1552). Purpose-bounded omission only.
    options.max_tokens = if codex { None } else { Some(ONLINE_MAX_TOKENS) };
    options.reasoning =
        Some(ReasoningRequest { disable_reasoning: true, force_reasoning_off: false, ..ReasoningRequest::default() });
    options.loop_guard = None;
    options.on_response = Some(ProviderResponseCallback::new(move |response, model| {
        let captured = captured.clone();
        let existing = existing.clone();
        Box::pin(async move {
            let mut headers = reqwest::header::HeaderMap::new();
            for (name, value) in &response.headers {
                if let (Ok(name), Ok(value)) = (
                    reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                    reqwest::header::HeaderValue::from_str(value),
                ) {
                    headers.insert(name, value);
                }
            }
            *captured.lock().expect("classifier header mutex") = headers;
            if let Some(existing) = existing {
                existing.notify(response, model).await?;
            }
            Ok(())
        })
    }));
    let mut stream = request.provider.stream(&request.model, context, options);
    let mut settling_cancel = false;
    let terminal = loop {
        // Fixed provider adapters publish their real aborted terminal after
        // cancellation. Retain this already-started stream through its receipt;
        // the Host owns this job even when its effort result is superseded.
        let event = if settling_cancel {
            stream.recv().await
        } else {
            tokio::select! {
                biased;
                event = stream.recv() => event,
                _ = request.cancel.cancelled() => {
                    attempt_cancel.cancel();
                    settling_cancel = true;
                    continue;
                },
                _ = request.options.cancel.cancelled() => {
                    attempt_cancel.cancel();
                    settling_cancel = true;
                    continue;
                },
            }
        };
        match event {
            Some(AssistantMessageEvent::Done { message, .. }) => break message,
            Some(AssistantMessageEvent::Error { error, .. }) => break error,
            Some(_) => {}
            None => return Err(if settling_cancel { ClassifierError::Cancelled } else { ClassifierError::Incomplete }),
        }
    };
    on_attempt(ClassifierAttempt {
        attempt,
        role: request.role.clone(),
        message: terminal.clone(),
        codex_output_cap_omitted: codex,
    })
    .await
    .map_err(ClassifierError::Receipt)?;
    if request.cancel.is_cancelled()
        || request.options.cancel.is_cancelled()
        || terminal.stop_reason == StopReason::Aborted
    {
        return Err(ClassifierError::Cancelled);
    }
    let headers = response_headers.lock().expect("classifier header mutex").clone();
    Ok((terminal, headers))
}

pub fn retryable_oneshot(message: &AssistantMessage, class: &retry_classification::RetryClass) -> bool {
    let text = message.error_message.as_deref().unwrap_or("").to_lowercase();
    let deterministic_local = text.contains("failed to parse tool call arguments as json")
        || text.contains("[json.exception.parse_error.101]");
    let excluded = flag::ABORT | flag::CONTENT_BLOCKED | flag::CONTEXT_OVERFLOW | flag::PAYLOAD_REJECTED;
    let status = message.failure_evidence.as_ref().and_then(|evidence| evidence.status).or(message.error_status);
    let transient_status = status.is_some_and(|status| status == 408 || status == 429 || status >= 500);
    !deterministic_local
        && !class.abort
        && class.error_id & excluded == 0
        && (class.retriable || transient_status || class.error_id & (flag::TRANSIENT | flag::USAGE_LIMIT) != 0)
}

pub fn retry_delay(
    message: &AssistantMessage,
    api: &str,
    attempt: u32,
    header_wait: Option<f64>,
    jitter: f64,
) -> Option<Duration> {
    let class = retry_classification::classify_retry(message, api);
    let mut text_only = message.clone();
    text_only.failure_evidence = None;
    let text_wait = retry_classification::classify_retry(&text_only, api).wait_ms;
    let suffix_wait = js_regex(r"(?:^|\s)retry-after-ms=([0-9]+(?:\.[0-9]+)?)(?=\s|$)", "i")
        .exec(&WireString::from(message.error_message.as_deref().unwrap_or("")))
        .and_then(|matched| matched.captures.get(1).and_then(Option::as_ref).and_then(|value| value.to_utf8().ok()))
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(f64::ceil);
    let hint = [header_wait, class.wait_ms, text_wait, suffix_wait]
        .into_iter()
        .flatten()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .reduce(f64::max);
    if hint.is_some_and(|hint| hint > 30_000.0) {
        return None;
    }
    let backoff = backoff_delay_ms(attempt, jitter);
    Some(Duration::from_secs_f64(hint.unwrap_or(0.0).max(backoff).min(30_000.0) / 1000.0))
}

pub fn backoff_delay_ms(attempt: u32, jitter: f64) -> f64 {
    let growth = (500.0 * 2_f64.powi(attempt.saturating_sub(1).min(30) as i32)).min(8_000.0);
    (growth * (0.75 + jitter.clamp(0.0, 1.0) * 0.25)).round()
}

pub fn jitter_fraction() -> f64 {
    let random = uuid::Uuid::new_v4();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&random.as_bytes()[8..]);
    // 53 random bits; avoid the UUID version and variant bits.
    ((u64::from_be_bytes(bytes) & ((1_u64 << 53) - 1)) as f64) / ((1_u64 << 53) as f64)
}

#[derive(Clone)]
pub struct LocalClassificationRequest {
    pub model: LocalClassifierModel,
    pub prompt: WireString,
    pub max_tokens: u64,
    pub cancel: CancellationToken,
}

/// Requires actual on-device inference. A fake implements fault-path tests,
/// never full Local acceptance. There is no Local-to-online automatic reroute.
pub trait LocalClassifier: Send + Sync {
    fn complete(&self, request: LocalClassificationRequest) -> BoxFuture<'static, Result<String, String>>;
}

pub async fn classify_local(
    prompt: &str,
    model: LocalClassifierModel,
    runtime: Option<Arc<dyn LocalClassifier>>,
    cancel: CancellationToken,
) -> Result<Option<ReasoningEffort>, ClassifierError> {
    let runtime = runtime.ok_or(ClassifierError::LocalRuntimeRequired(model))?;
    let cleaned = preprocess_tiny_message(prompt);
    let input = cleaned.to_utf8().map_err(|_| ClassifierError::Utf16WireRepresentationRequired)?;
    let rendered = ara_prompt::render(LOCAL_PROMPT, &serde_json::json!({ "prompt": input }))
        .expect("fixed local classifier prompt must render");
    let completion = runtime.complete(LocalClassificationRequest {
        model,
        prompt: WireString::from(rendered),
        max_tokens: model.max_tokens(),
        cancel: cancel.clone(),
    });
    let text = tokio::select! {
        _ = cancel.cancelled() => return Err(ClassifierError::Cancelled),
        text = completion => text.map_err(ClassifierError::Local)?,
    };
    if cancel.is_cancelled() {
        return Err(ClassifierError::Cancelled);
    }
    parse_local_effort(&text).map(Some).ok_or(ClassifierError::Unparseable { backend: "local" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_ai::{EventSink, ThinkingContent, Usage};
    use std::collections::VecDeque;

    #[test]
    fn native_labels_use_earliest_ascii_boundaries_and_local_mapping() {
        for (text, expected) in [
            ("choose LOW, maybe HIGH", Some(ReasoningEffort::Low)),
            ("x-high then low", Some(ReasoningEffort::XHigh)),
            ("max or xhigh", Some(ReasoningEffort::Max)),
            ("med; x_high", Some(ReasoningEffort::Medium)),
            // Native JS \b uses ASCII word characters, unlike Rust's default.
            ("高high低", Some(ReasoningEffort::High)),
            ("lowest maximum mediumish", None),
        ] {
            assert_eq!(parse_online_effort(text), expected, "{text}");
        }
        assert_eq!(parse_local_effort("moderate then trivial"), Some(ReasoningEffort::High));
        assert_eq!(parse_local_effort("hard; moderate"), Some(ReasoningEffort::XHigh));
        assert_eq!(parse_local_effort("triviality hardly"), None);
        let ordinary = online_system_prompt(false);
        let maximum = online_system_prompt(true);
        assert!(!ordinary.contains("{{"));
        assert!(!ordinary.contains("`max`"));
        assert!(maximum.contains("`max`: meets `xhigh`"));
    }

    #[test]
    fn auto_floor_sparse_ceiling_provisional_and_ultrathink_are_distinct() {
        use ReasoningEffort::*;
        assert_eq!(clamp_auto_effort(&[Minimal, Max], Low, XHigh), None);
        assert_eq!(clamp_auto_effort(&[Max], XHigh, XHigh), None);
        assert_eq!(clamp_auto_effort(&[Minimal], Low, XHigh), Some(Minimal));
        assert_eq!(clamp_auto_effort(&[Minimal, Medium, XHigh], Low, XHigh), Some(Medium));
        assert_eq!(provisional_effort(&[Low, High, XHigh, Max], Some(Max)), Some(XHigh));
        assert_eq!(provisional_effort(&[Max], Some(Max)), None);
        assert_eq!(provisional_effort(&[], None), None);
        assert_eq!(auto_ceiling(AutoThinkingBackend::Online, false, &[Low, Max]), XHigh);
        assert_eq!(auto_ceiling(AutoThinkingBackend::Online, true, &[Low, Max]), Max);
        assert_eq!(auto_ceiling(AutoThinkingBackend::Local(LocalClassifierModel::Lfm2_1_2B), true, &[Low, Max]), XHigh);
        assert_eq!(resolve_auto_effort(&[Low, Medium, High], None, Some(High), Some(Low), Some(Medium)), Some(Medium));
        assert_eq!(apply_session_ceiling(&[High], Some(High), Some(Low)), Some(High));
        assert_eq!(ultrathink_effort("ultrathink", true, &[Low, Max]), Some(Max));
        assert_eq!(ultrathink_effort("ultrathink", false, &[Low, Max]), None);
    }

    #[test]
    fn tiny_cleanup_and_utf16_middle_slice_preserve_original_signal() {
        let original = "\u{1b}[31mPlease fix this issue\u{1b}[0m <user>hidden</user> 0123456789abcdef\n```rust\nnoise\n```\nKeep the final request";
        let cleaned = preprocess_tiny_message(original).to_utf8().unwrap();
        assert_eq!(cleaned, "Please fix this issue 0123456\n \nKeep the final request");
        assert!(original.contains("hidden"));
        assert_eq!(preprocess_tiny_message("```x```"), WireString::from("```x```"));
        let long = format!("{}FINAL USER REQUEST", "😀".repeat(2000));
        let sliced = preprocess_tiny_message(&long);
        assert_eq!(sliced.len(), 2000);
        assert!(String::from_utf16_lossy(sliced.units()).ends_with("FINAL USER REQUEST"));
        assert!(String::from_utf16_lossy(sliced.units()).contains("chars omitted"));
        // JS slicing can split a surrogate pair. Pure policy preserves it;
        // String-only model Context is a separately declared required port.
        let split = truncate_tiny_message(&WireString::from(format!("{}END", "😀".repeat(1200))));
        assert_eq!(split.len(), 2000);
        assert!(split.to_utf8().is_err(), "retain the Native lone surrogate rather than replacing it");
    }

    #[test]
    fn ultrathink_only_matches_native_prose_and_identifier_boundaries() {
        for text in [
            "ultrathink",
            "Please ultrathink!",
            "(ultrathink)",
            "ultrathink.",
            "`unfinished ultrathink",
            "<unclosed> prose ultrathink",
        ] {
            assert!(has_ultrathink(text), "{text}");
        }
        for text in [
            "Ultrathink",
            "ultrathinking",
            "中文ultrathink",
            "ultrathink.ts",
            "foo::ultrathink",
            "ultrathink()",
            "./ultrathink",
            "ultrathink/foo",
            "ultrathink-test",
            "`ultrathink`",
            "``ultrathink``",
            "```\nultrathink\n```",
            "~~~text\nultrathink\n~~~",
            "<user>ultrathink</user>",
            "<a><a>ultrathink</a></a>",
            "<!-- ultrathink -->",
            "<tag attribute='ultrathink'>ordinary</tag>",
        ] {
            assert!(!has_ultrathink(text), "{text}");
        }
        assert!(has_ultrathink("<tag>ultrathink</tag> actual ultrathink"));
        let text = WireString::from("😀 `ultrathink`\nprose");
        let masked = mask_non_prose(&text);
        assert_eq!(masked.len(), text.len());
        assert_eq!(masked.units()[15], text.units()[15]);
    }

    #[test]
    fn native_retry_hints_and_fault_exclusions_have_distinguishing_cases() {
        let mut message = AssistantMessage::empty("openai-completions", "fake", "tiny");
        message.stop_reason = StopReason::Error;
        message.error_status = Some(408);
        message.error_message = Some("retry-after-ms=1200".into());
        let class = retry_classification::classify_retry(&message, "openai-completions");
        assert!(retryable_oneshot(&message, &class));
        assert_eq!(
            retry_delay(&message, "openai-completions", 1, Some(1500.0), 0.0),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(retry_delay(&message, "openai-completions", 1, Some(30_001.0), 1.0), None);
        message.error_message = Some("failed to parse tool call arguments as JSON".into());
        message.error_status = Some(500);
        assert!(!retryable_oneshot(&message, &retry_classification::classify_retry(&message, "openai-completions")));
        message.error_message = Some("maximum context length exceeded".into());
        assert!(!retryable_oneshot(&message, &retry_classification::classify_retry(&message, "openai-completions")));
        message.error_message = Some("503 overloaded".into());
        assert_eq!(retry_delay(&message, "openai-completions", 1, None, 0.0), Some(Duration::from_millis(375)));
        assert_eq!(retry_delay(&message, "openai-completions", 2, None, 1.0), Some(Duration::from_millis(1000)));
        assert_eq!(retry_delay(&message, "openai-completions", 8, None, 1.0), Some(Duration::from_millis(8000)));
    }

    struct ObservedProvider {
        replies: Mutex<VecDeque<Option<AssistantMessage>>>,
        calls: Mutex<Vec<(Context, CallOptions)>>,
    }

    impl ObservedProvider {
        fn new(replies: Vec<Option<AssistantMessage>>) -> Self {
            Self { replies: Mutex::new(replies.into()), calls: Mutex::new(Vec::new()) }
        }
    }

    impl ModelProvider for ObservedProvider {
        fn stream(&self, _: &Model, context: &Context, options: CallOptions) -> ara_ai::AssistantStream {
            self.calls.lock().unwrap().push((context.clone(), options));
            let reply = self.replies.lock().unwrap().pop_front().expect("scripted classifier call");
            let (sink, stream) = EventSink::channel();
            tokio::spawn(async move {
                if let Some(message) = reply {
                    let event = if matches!(message.stop_reason, StopReason::Error | StopReason::Aborted) {
                        AssistantMessageEvent::Error { reason: message.stop_reason, error: message }
                    } else {
                        AssistantMessageEvent::Done { reason: message.stop_reason, message }
                    };
                    sink.push(event).await;
                }
            });
            stream
        }
    }

    fn request(provider: Arc<ObservedProvider>, api: &str) -> OnlineClassificationRequest {
        OnlineClassificationRequest {
            provider,
            model: Model {
                id: "tiny".into(),
                api: api.into(),
                provider: "fake".into(),
                base_url: "http://unused".into(),
                reasoning: true,
                max_tokens: None,
                context_window: None,
                tokenizer: None,
            },
            options: CallOptions {
                max_tokens: Some(1),
                reasoning: Some(ReasoningRequest {
                    effort: Some(ReasoningEffort::Max),
                    force_reasoning_off: true,
                    ..Default::default()
                }),
                ..CallOptions::default()
            },
            role: "tiny".into(),
            prompt: "Please explain the scope of this change".into(),
            allow_max: false,
            cancel: CancellationToken::new(),
        }
    }

    fn recording_attempts(ledger: Arc<Mutex<Vec<ClassifierAttempt>>>) -> ClassifierAttemptCallback {
        Arc::new(move |attempt| {
            ledger.lock().unwrap().push(attempt);
            Box::pin(async { Ok(()) })
        })
    }

    #[tokio::test]
    async fn actual_provider_stream_receipts_retry_and_codex_purpose_omission() {
        let mut failed = AssistantMessage::empty("openai-completions", "fake", "tiny");
        failed.stop_reason = StopReason::Error;
        failed.error_status = Some(503);
        failed.error_message = Some("503 overloaded".into());
        failed.usage.output = Some(2);
        let mut success = AssistantMessage::empty("openai-completions", "fake", "tiny");
        success
            .content
            .push(AssistantBlock::Thinking(ThinkingContent { thinking: "max".into(), thinking_signature: None }));
        success.content.push(AssistantBlock::text("low"));
        let provider = Arc::new(ObservedProvider::new(vec![Some(failed), Some(success.clone())]));
        let ledger = Arc::new(Mutex::new(Vec::new()));
        assert_eq!(
            classify_online(request(provider.clone(), "openai-completions"), recording_attempts(ledger.clone()))
                .await
                .unwrap(),
            Some(ReasoningEffort::Low)
        );
        {
            let receipts = ledger.lock().unwrap();
            assert_eq!(receipts.len(), 2);
            assert_eq!(receipts[0].message.usage.output, Some(2));
            assert_eq!(receipts[0].message.usage.input, None);
            assert_eq!(receipts[1].message.usage, Usage::unknown());
        }
        for (context, options) in provider.calls.lock().unwrap().iter() {
            assert_eq!(context.messages.len(), 1);
            assert!(context.tools.is_none());
            assert_eq!(options.max_tokens, Some(4096));
            let reasoning = options.reasoning.as_ref().unwrap();
            assert!(reasoning.disable_reasoning);
            assert!(!reasoning.force_reasoning_off);
            assert_eq!(reasoning.effort, None);
        }
        let provider = Arc::new(ObservedProvider::new(vec![Some(success)]));
        let ledger = Arc::new(Mutex::new(Vec::new()));
        classify_online(request(provider.clone(), "openai-codex-responses"), recording_attempts(ledger.clone()))
            .await
            .unwrap();
        assert_eq!(provider.calls.lock().unwrap()[0].1.max_tokens, None);
        assert!(ledger.lock().unwrap()[0].codex_output_cap_omitted);
    }

    #[tokio::test]
    async fn native_thinking_loop_and_transient_budgets_are_independent() {
        let mut loop_failure = AssistantMessage::empty("openai-completions", "fake", "tiny");
        loop_failure.stop_reason = StopReason::Error;
        loop_failure.error_message = Some("Thinking loop detected".into());
        let mut evidence = ara_ai::retry_classification::ProviderFailureEvidence::from_error(
            &ara_ai::ProviderError::Stream("Thinking loop detected".into()),
            false,
        );
        evidence.error_id = flag::CLASS | flag::THINKING_LOOP;
        loop_failure.failure_evidence = Some(evidence);
        let mut success = AssistantMessage::empty("openai-completions", "fake", "tiny");
        success.content.push(AssistantBlock::text("medium"));
        let provider = Arc::new(ObservedProvider::new(vec![
            Some(loop_failure.clone()),
            Some(loop_failure.clone()),
            Some(loop_failure),
            Some(success),
        ]));
        let ledger = Arc::new(Mutex::new(Vec::new()));
        let result = tokio::time::timeout(
            CLASSIFICATION_TIMEOUT,
            classify_online(request(provider.clone(), "openai-completions"), recording_attempts(ledger.clone())),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, Some(ReasoningEffort::Medium));
        assert_eq!(provider.calls.lock().unwrap().len(), 4);
        assert_eq!(ledger.lock().unwrap().iter().map(|receipt| receipt.attempt).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn observed_terminal_survives_cancellation_and_missing_terminal_invents_no_usage() {
        let mut success = AssistantMessage::empty("openai-completions", "fake", "tiny");
        success.content.push(AssistantBlock::text("high"));
        success.usage.input = Some(7);
        let provider = Arc::new(ObservedProvider::new(vec![Some(success)]));
        let classifier_request = request(provider, "openai-completions");
        let cancel = classifier_request.cancel.clone();
        let ledger = Arc::new(Mutex::new(Vec::new()));
        let recorded = ledger.clone();
        let callback: ClassifierAttemptCallback = Arc::new(move |attempt| {
            recorded.lock().unwrap().push(attempt);
            cancel.cancel();
            Box::pin(async {
                tokio::task::yield_now().await;
                Ok(())
            })
        });
        assert_eq!(classify_online(classifier_request, callback).await, Err(ClassifierError::Cancelled));
        assert_eq!(ledger.lock().unwrap()[0].message.usage.input, Some(7));
        let provider = Arc::new(ObservedProvider::new(vec![None, None, None]));
        let ledger = Arc::new(Mutex::new(Vec::new()));
        assert_eq!(
            classify_online(request(provider, "openai-completions"), recording_attempts(ledger.clone())).await,
            Err(ClassifierError::Incomplete)
        );
        assert!(ledger.lock().unwrap().is_empty());
        assert_eq!(
            classify_local("task", LocalClassifierModel::Lfm2_1_2B, None, CancellationToken::new()).await,
            Err(ClassifierError::LocalRuntimeRequired(LocalClassifierModel::Lfm2_1_2B))
        );
    }
}
