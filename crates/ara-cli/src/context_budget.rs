//! Host context accounting from fixed OMP 596f2da session-stats.ts.
//! Counts remain estimates where the selected native tokenizer is unavailable.
use ara_agent::{
    AgentTool,
    tokenizer::{MessageCountOptions, TokenCountMode, Tokenizer, TokenizerPolicy, count_messages},
};
use ara_ai::{Message, Model};
use ara_session::{Entry, SessionJournal, SessionReductionAction, SessionReductionEdit, SessionReductionSnapshot};
use serde_json::{Value, json};
use std::sync::Arc;

/// Capture Host policy once per counter; the reusable Core reads no environment.
pub fn tokenizer(model: &Model) -> Tokenizer {
    Tokenizer::with_policy(
        Some(model),
        TokenizerPolicy {
            test_environment: false,
            accurate_unknown: std::env::var("ARA_TOKENIZER_ACCURATE").is_ok_and(|value| value == "1"),
        },
    )
}

pub fn text_tokens(model: &Model, fragments: &[&str]) -> usize {
    tokenizer(model).count_fragments(fragments.iter().copied(), TokenCountMode::Approximate)
}

pub fn non_message_tokens(model: &Model, prompt: &[String], tools: &[Arc<dyn AgentTool>]) -> usize {
    let schemas: Vec<String> = tools
        .iter()
        .map(|tool| {
            let tool = tool.definition();
            serde_json::to_string(
                &json!({"name":tool.name,"description":tool.description,"parameters":tool.parameters}),
            )
            .unwrap_or_default()
        })
        .collect();
    let fragments: Vec<&str> = prompt.iter().chain(&schemas).map(String::as_str).collect();
    text_tokens(model, &fragments)
}

fn positive_number(value: &Value) -> Option<usize> {
    value
        .as_f64()
        .filter(|number| number.is_finite() && *number >= 0.0)
        .map(|number| number.min(usize::MAX as f64) as usize)
}

/// A missing bucket remains unknown. Only observed positive prefix occupancy
/// can anchor the estimate; aborted/error responses never do so.
pub fn prompt_tokens(entry: &Entry) -> Option<usize> {
    let message = &entry.raw["message"];
    if entry.kind != "message"
        || message["role"] != "assistant"
        || matches!(message["stopReason"].as_str(), Some("error" | "aborted"))
    {
        return None;
    }
    let usage = &message["usage"];
    if let Some(tokens) = positive_number(&usage["contextTokens"]).filter(|tokens| *tokens > 0) {
        return Some(tokens);
    }
    let known: Vec<usize> =
        ["input", "cacheRead", "cacheWrite"].iter().filter_map(|key| positive_number(&usage[*key])).collect();
    let lower = known.iter().fold(0usize, |sum, tokens| sum.saturating_add(*tokens));
    if lower > 0 {
        return Some(lower);
    }
    let total = positive_number(&usage["totalTokens"])?;
    let output = positive_number(&usage["output"])?;
    (total > output).then_some(total)
}

pub fn anchor(snapshot: &SessionReductionSnapshot) -> Option<(usize, usize)> {
    let boundary =
        snapshot.entries.iter().rposition(|entry| matches!(entry.kind.as_str(), "compaction" | "reset_boundary"));
    snapshot
        .entries
        .iter()
        .enumerate()
        .rev()
        .take_while(|(index, _)| boundary.is_none_or(|boundary| *index > boundary))
        .find_map(|(index, entry)| prompt_tokens(entry).map(|tokens| (index, tokens)))
}

pub fn anchor_edit(
    snapshot: &SessionReductionSnapshot,
    savings: &[(String, usize)],
    non_message: usize,
) -> Option<SessionReductionEdit> {
    let (anchor_index, prompt_tokens) = anchor(snapshot)?;
    let removed = snapshot.entries[..anchor_index].iter().fold(0usize, |sum, entry| {
        sum.saturating_add(
            savings.iter().filter(|(id, _)| id == &entry.id).fold(0usize, |n, (_, tokens)| n.saturating_add(*tokens)),
        )
    });
    if removed == 0 {
        return None;
    }
    Some(SessionReductionEdit {
        entry_id: snapshot.entries[anchor_index].id.clone(),
        action: SessionReductionAction::RecordAnchoredHistoryRewrite {
            tokens_removed: removed,
            snapshot_if_missing: Some(json!({"promptTokens":prompt_tokens,"nonMessageTokens":non_message,
            "compactionEpoch":snapshot.entries.iter().filter(|entry| entry.kind == "compaction").count()})),
        },
    })
}

pub fn context_tokens(
    journal: &SessionJournal,
    messages: &[Message],
    pending: &[Message],
    non_message: usize,
) -> usize {
    context_tokens_with_counter(journal, messages, pending, non_message, count_messages)
}

pub fn context_tokens_for_model(
    model: &Model,
    journal: &SessionJournal,
    messages: &[Message],
    pending: &[Message],
    non_message: usize,
) -> usize {
    let tokenizer = tokenizer(model);
    context_tokens_with_counter(journal, messages, pending, non_message, |messages, options| {
        tokenizer.count_messages(messages, options)
    })
}

fn context_tokens_with_counter(
    journal: &SessionJournal,
    messages: &[Message],
    pending: &[Message],
    non_message: usize,
    count: impl Fn(&[Message], MessageCountOptions) -> usize,
) -> usize {
    let pending_tokens = count(pending, MessageCountOptions { exclude_encrypted_reasoning: true });
    let stored = non_message
        .saturating_add(count(messages, MessageCountOptions { exclude_encrypted_reasoning: true }))
        .saturating_add(pending_tokens);
    let provider = journal.raw_reduction_snapshot().ok().and_then(|snapshot| {
        let (index, prompt) = anchor(&snapshot)?;
        let entry = &snapshot.entries[index];
        let projected = entry.message()?;
        let position = messages.iter().rposition(|message| message == &projected)?;
        let context = &entry.raw["message"]["contextSnapshot"];
        let base = positive_number(&context["promptTokens"]).unwrap_or(prompt);
        let removed = positive_number(&context["historyRewriteTokensRemoved"]).unwrap_or(0);
        let previous_non_message = positive_number(&context["nonMessageTokens"]).unwrap_or(non_message);
        Some(
            base.saturating_sub(removed)
                .saturating_add(non_message.saturating_sub(previous_non_message))
                .saturating_add(count(&messages[position + 1..], MessageCountOptions::default()))
                .saturating_add(pending_tokens),
        )
    });
    stored.max(provider.unwrap_or(stored))
}
